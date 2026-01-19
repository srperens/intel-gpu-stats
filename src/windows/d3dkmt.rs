//! D3DKMT API bindings for querying GPU statistics
//!
//! D3DKMT (Direct3D Kernel Mode Thunk) provides low-level access to GPU
//! performance counters and statistics on Windows.

use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::null_mut;

use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID, NTSTATUS};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};

use crate::error::{Error, Result};
use crate::types::*;

// D3DKMT constants
const STATUS_SUCCESS: i32 = 0;

// D3DKMT statistics types
const D3DKMT_QUERYSTATISTICS_ADAPTER: u32 = 0;
const D3DKMT_QUERYSTATISTICS_NODE: u32 = 4;

// Engine type mappings for Intel GPUs
// These are typical node ordinals for Intel GPU engines
const ENGINE_NODE_3D: u32 = 0; // Render/3D
const ENGINE_NODE_COPY: u32 = 1; // Blitter/Copy
const ENGINE_NODE_VIDEO: u32 = 2; // Video decode
const ENGINE_NODE_VIDEO_ENHANCE: u32 = 3; // Video encode/enhance
const ENGINE_NODE_COMPUTE: u32 = 4; // Compute (Arc GPUs)

// FFI structures for D3DKMT
#[repr(C)]
#[derive(Clone, Copy)]
struct D3DKMT_OPENADAPTERFROMLUID {
    adapter_luid: LUID,
    h_adapter: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct D3DKMT_CLOSEADAPTER {
    h_adapter: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct D3DKMT_QUERYADAPTERINFO {
    h_adapter: u32,
    info_type: u32,
    private_driver_data: *mut c_void,
    private_driver_data_size: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct D3DKMT_QUERYSTATISTICS {
    query_type: u32,
    adapter_luid: LUID,
    h_process: HANDLE,
    query_result: D3DKMT_QUERYSTATISTICS_RESULT,
}

#[repr(C)]
#[derive(Clone, Copy)]
union D3DKMT_QUERYSTATISTICS_RESULT {
    adapter_info: D3DKMT_QUERYSTATISTICS_ADAPTER_INFORMATION,
    node_info: D3DKMT_QUERYSTATISTICS_NODE_INFORMATION,
    process_info: D3DKMT_QUERYSTATISTICS_PROCESS_INFORMATION,
    _padding: [u8; 512], // Ensure union is large enough
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct D3DKMT_QUERYSTATISTICS_ADAPTER_INFORMATION {
    node_count: u32,
    segment_count: u32,
    _reserved: [u64; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct D3DKMT_QUERYSTATISTICS_NODE_INFORMATION {
    global_info: D3DKMT_QUERYSTATISTICS_NODE_GLOBAL_INFO,
    _reserved: [u64; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct D3DKMT_QUERYSTATISTICS_NODE_GLOBAL_INFO {
    running_time: u64,   // 100ns units
    context_switch: u32, // Number of context switches
    _reserved: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct D3DKMT_QUERYSTATISTICS_PROCESS_INFORMATION {
    node_count: u32,
    segment_count: u32,
    _reserved: [u64; 8],
}

// D3DKMT function signatures
type FnD3DKMTOpenAdapterFromLuid =
    unsafe extern "system" fn(*mut D3DKMT_OPENADAPTERFROMLUID) -> NTSTATUS;
type FnD3DKMTCloseAdapter = unsafe extern "system" fn(*const D3DKMT_CLOSEADAPTER) -> NTSTATUS;
type FnD3DKMTQueryStatistics = unsafe extern "system" fn(*mut D3DKMT_QUERYSTATISTICS) -> NTSTATUS;
type FnD3DKMTQueryAdapterInfo = unsafe extern "system" fn(*mut D3DKMT_QUERYADAPTERINFO) -> NTSTATUS;

/// D3DKMT function pointers loaded from gdi32.dll
struct D3dkmtFunctions {
    open_adapter: FnD3DKMTOpenAdapterFromLuid,
    close_adapter: FnD3DKMTCloseAdapter,
    query_statistics: FnD3DKMTQueryStatistics,
    #[allow(dead_code)]
    query_adapter_info: FnD3DKMTQueryAdapterInfo,
}

impl D3dkmtFunctions {
    fn load() -> Result<Self> {
        use windows::core::PCSTR;
        use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

        let gdi32 =
            unsafe { LoadLibraryW(windows::core::w!("gdi32.dll")) }.map_err(|e| Error::Io {
                context: format!("Failed to load gdi32.dll: {}", e),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string()),
            })?;

        unsafe {
            let open_adapter = GetProcAddress(
                gdi32,
                PCSTR(b"D3DKMTOpenAdapterFromLuid\0".as_ptr()),
            )
            .ok_or_else(|| Error::Io {
                context: "D3DKMTOpenAdapterFromLuid not found".into(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "Function not found"),
            })?;

            let close_adapter = GetProcAddress(gdi32, PCSTR(b"D3DKMTCloseAdapter\0".as_ptr()))
                .ok_or_else(|| Error::Io {
                    context: "D3DKMTCloseAdapter not found".into(),
                    source: std::io::Error::new(std::io::ErrorKind::NotFound, "Function not found"),
                })?;

            let query_statistics = GetProcAddress(
                gdi32,
                PCSTR(b"D3DKMTQueryStatistics\0".as_ptr()),
            )
            .ok_or_else(|| Error::Io {
                context: "D3DKMTQueryStatistics not found".into(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "Function not found"),
            })?;

            let query_adapter_info = GetProcAddress(
                gdi32,
                PCSTR(b"D3DKMTQueryAdapterInfo\0".as_ptr()),
            )
            .ok_or_else(|| Error::Io {
                context: "D3DKMTQueryAdapterInfo not found".into(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "Function not found"),
            })?;

            Ok(Self {
                open_adapter: std::mem::transmute(open_adapter),
                close_adapter: std::mem::transmute(close_adapter),
                query_statistics: std::mem::transmute(query_statistics),
                query_adapter_info: std::mem::transmute(query_adapter_info),
            })
        }
    }
}

// Thread-local D3DKMT functions
thread_local! {
    static D3DKMT: std::cell::RefCell<Option<D3dkmtFunctions>> = const { std::cell::RefCell::new(None) };
}

fn with_d3dkmt<T, F: FnOnce(&D3dkmtFunctions) -> T>(f: F) -> Result<T> {
    D3DKMT.with(|cell| {
        let mut opt = cell.borrow_mut();
        if opt.is_none() {
            *opt = Some(D3dkmtFunctions::load()?);
        }
        Ok(f(opt.as_ref().unwrap()))
    })
}

/// D3DKMT adapter handle wrapper
pub struct D3dkmtAdapter {
    h_adapter: u32,
    adapter_luid: LUID,
    node_count: u32,
}

impl D3dkmtAdapter {
    /// Open a D3DKMT adapter from GpuInfo
    pub fn open(gpu_info: &GpuInfo) -> Result<Self> {
        // Get the LUID from DXGI
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(|e| Error::Io {
            context: format!("Failed to create DXGI factory: {}", e),
            source: std::io::Error::new(std::io::ErrorKind::Other, e.to_string()),
        })?;

        // Find the adapter matching our GPU
        let mut adapter_index = 0u32;
        let mut found_luid: Option<LUID> = None;

        loop {
            let adapter = match unsafe { factory.EnumAdapters1(adapter_index) } {
                Ok(a) => a,
                Err(_) => break,
            };

            if let Ok(desc) = unsafe { adapter.GetDesc1() } {
                let id = format!("adapter{}", adapter_index);
                if id == gpu_info.id {
                    found_luid = Some(desc.AdapterLuid);
                    break;
                }
            }
            adapter_index += 1;
        }

        let adapter_luid = found_luid.ok_or_else(|| Error::DeviceNotFound {
            path: gpu_info.id.clone().into(),
        })?;

        // Open the D3DKMT adapter
        let mut open_adapter = D3DKMT_OPENADAPTERFROMLUID {
            adapter_luid,
            h_adapter: 0,
        };

        with_d3dkmt(|funcs| {
            let status = unsafe { (funcs.open_adapter)(&mut open_adapter) };
            if status.0 != STATUS_SUCCESS {
                return Err(Error::Io {
                    context: format!("D3DKMTOpenAdapterFromLuid failed: 0x{:08x}", status.0),
                    source: std::io::Error::new(std::io::ErrorKind::Other, "D3DKMT error"),
                });
            }
            Ok(())
        })??;

        // Query adapter info to get node count
        let node_count = Self::query_node_count(adapter_luid)?;

        Ok(Self {
            h_adapter: open_adapter.h_adapter,
            adapter_luid,
            node_count,
        })
    }

    /// Query the number of GPU nodes
    fn query_node_count(adapter_luid: LUID) -> Result<u32> {
        let mut query: D3DKMT_QUERYSTATISTICS = unsafe { zeroed() };
        query.query_type = D3DKMT_QUERYSTATISTICS_ADAPTER;
        query.adapter_luid = adapter_luid;
        query.h_process = HANDLE(null_mut());

        with_d3dkmt(|funcs| {
            let status = unsafe { (funcs.query_statistics)(&mut query) };
            if status.0 != STATUS_SUCCESS {
                return Err(Error::Io {
                    context: format!("D3DKMTQueryStatistics (adapter) failed: 0x{:08x}", status.0),
                    source: std::io::Error::new(std::io::ErrorKind::Other, "D3DKMT error"),
                });
            }
            Ok(unsafe { query.query_result.adapter_info.node_count })
        })?
    }

    /// Query the mapping of engine classes to node ordinals
    pub fn query_node_mapping(&self) -> Result<HashMap<EngineClass, u32>> {
        let mut mapping = HashMap::new();

        // Intel GPUs typically have a fixed node layout
        // Node 0: 3D/Render
        // Node 1: Copy/Blitter
        // Node 2: Video (decode)
        // Node 3: VideoEnhance (encode)
        // Node 4+: Compute (on Arc GPUs)

        if self.node_count > ENGINE_NODE_3D {
            mapping.insert(EngineClass::Render, ENGINE_NODE_3D);
        }
        if self.node_count > ENGINE_NODE_COPY {
            mapping.insert(EngineClass::Copy, ENGINE_NODE_COPY);
        }
        if self.node_count > ENGINE_NODE_VIDEO {
            mapping.insert(EngineClass::Video, ENGINE_NODE_VIDEO);
        }
        if self.node_count > ENGINE_NODE_VIDEO_ENHANCE {
            mapping.insert(EngineClass::VideoEnhance, ENGINE_NODE_VIDEO_ENHANCE);
        }
        if self.node_count > ENGINE_NODE_COMPUTE {
            mapping.insert(EngineClass::Compute, ENGINE_NODE_COMPUTE);
        }

        Ok(mapping)
    }

    /// Get the adapter LUID
    #[allow(dead_code)]
    pub fn luid(&self) -> LUID {
        self.adapter_luid
    }

    /// Get the adapter handle
    #[allow(dead_code)]
    pub fn handle(&self) -> u32 {
        self.h_adapter
    }
}

impl Drop for D3dkmtAdapter {
    fn drop(&mut self) {
        let close = D3DKMT_CLOSEADAPTER {
            h_adapter: self.h_adapter,
        };

        let _ = with_d3dkmt(|funcs| {
            let _ = unsafe { (funcs.close_adapter)(&close) };
        });
    }
}

/// Statistics query helper
pub struct D3dkmtQueryStatistics<'a> {
    adapter: &'a D3dkmtAdapter,
}

impl<'a> D3dkmtQueryStatistics<'a> {
    /// Create a new query helper
    pub fn new(adapter: &'a D3dkmtAdapter) -> Self {
        Self { adapter }
    }

    /// Query running time for a specific node (in nanoseconds)
    pub fn query_node_running_time(&self, node_id: u32) -> Result<u64> {
        // Use the query_node structure with proper node_id
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct QueryNodeInput {
            query_type: u32,
            adapter_luid: LUID,
            h_process: HANDLE,
            node_id: u32,
        }

        let mut query_bytes = [0u8; size_of::<D3DKMT_QUERYSTATISTICS>()];

        // Set up the input part
        let input = QueryNodeInput {
            query_type: D3DKMT_QUERYSTATISTICS_NODE,
            adapter_luid: self.adapter.adapter_luid,
            h_process: HANDLE(null_mut()),
            node_id,
        };

        // Copy input to query bytes
        unsafe {
            std::ptr::copy_nonoverlapping(
                &input as *const _ as *const u8,
                query_bytes.as_mut_ptr(),
                size_of::<QueryNodeInput>(),
            );
        }

        with_d3dkmt(|funcs| {
            let status = unsafe {
                (funcs.query_statistics)(query_bytes.as_mut_ptr() as *mut D3DKMT_QUERYSTATISTICS)
            };

            if status.0 != STATUS_SUCCESS {
                return Err(Error::Io {
                    context: format!(
                        "D3DKMTQueryStatistics (node {}) failed: 0x{:08x}",
                        node_id, status.0
                    ),
                    source: std::io::Error::new(std::io::ErrorKind::Other, "D3DKMT error"),
                });
            }

            // Read the result - running_time is at a known offset in the result
            // The result structure starts after the input fields
            let result_offset = size_of::<QueryNodeInput>();
            let result_ptr = query_bytes.as_ptr().wrapping_add(result_offset)
                as *const D3DKMT_QUERYSTATISTICS_NODE_INFORMATION;
            let node_info = unsafe { *result_ptr };

            // Convert from 100ns units to nanoseconds
            Ok(node_info.global_info.running_time * 100)
        })?
    }

    /// Query GPU frequency (if available)
    pub fn query_frequency(&self) -> Result<FrequencyStats> {
        // D3DKMT doesn't directly expose frequency
        // Return zeros - frequency monitoring is limited on Windows
        Ok(FrequencyStats::new(0, 0))
    }

    /// Query temperature (if available)
    pub fn query_temperature(&self) -> Option<TemperatureStats> {
        // Temperature is not directly available through D3DKMT
        // Would need to use WMI or Intel-specific APIs
        None
    }

    /// Query power consumption (if available)
    pub fn query_power(&self) -> Option<PowerStats> {
        // Power is not directly available through D3DKMT
        // Would need to use WMI or Intel-specific APIs
        None
    }
}

/// List all processes using GPU resources
pub fn list_gpu_processes() -> Result<Vec<DrmClient>> {
    // This is a simplified implementation
    // A full implementation would enumerate all processes and check GPU usage

    let mut clients = Vec::new();

    // Try to enumerate processes with GPU handles
    // This requires admin privileges for full enumeration
    if let Ok(processes) = enumerate_gpu_processes() {
        for (pid, name) in processes {
            let mut client = DrmClient::new(pid, name);
            // Query per-process GPU usage if available
            if let Ok(usage) = query_process_gpu_usage(pid) {
                client.render_ns = usage.render_ns;
                client.video_ns = usage.video_ns;
                client.video_enhance_ns = usage.video_enhance_ns;
                client.copy_ns = usage.copy_ns;
                client.compute_ns = usage.compute_ns;
            }
            clients.push(client);
        }
    }

    Ok(clients)
}

/// Simple struct to hold process GPU usage
struct ProcessGpuUsage {
    render_ns: u64,
    video_ns: u64,
    video_enhance_ns: u64,
    copy_ns: u64,
    compute_ns: u64,
}

/// Enumerate processes that might be using the GPU
fn enumerate_gpu_processes() -> Result<Vec<(u32, String)>> {
    use windows::Win32::System::ProcessStatus::{EnumProcesses, GetModuleBaseNameW};

    let mut processes = Vec::new();
    let mut pids = [0u32; 4096];
    let mut bytes_returned = 0u32;

    unsafe {
        if !EnumProcesses(
            pids.as_mut_ptr(),
            (pids.len() * 4) as u32,
            &mut bytes_returned,
        )
        .is_ok()
        {
            return Ok(processes);
        }
    }

    let count = bytes_returned as usize / 4;

    for &pid in &pids[..count] {
        if pid == 0 {
            continue;
        }

        // Try to open the process
        let handle =
            unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) };

        if let Ok(handle) = handle {
            let mut name_buf = [0u16; 260];
            let len = unsafe { GetModuleBaseNameW(handle, None, &mut name_buf) };

            if len > 0 {
                let name = String::from_utf16_lossy(&name_buf[..len as usize]);
                processes.push((pid, name));
            }

            let _ = unsafe { CloseHandle(handle) };
        }
    }

    Ok(processes)
}

/// Query GPU usage for a specific process
fn query_process_gpu_usage(_pid: u32) -> Result<ProcessGpuUsage> {
    // Per-process GPU usage requires D3DKMT process-specific queries
    // This is a placeholder - full implementation would query D3DKMT
    // with PROCESS_QUERY_INFORMATION access to the target process

    Ok(ProcessGpuUsage {
        render_ns: 0,
        video_ns: 0,
        video_enhance_ns: 0,
        copy_ns: 0,
        compute_ns: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_success() {
        assert_eq!(STATUS_SUCCESS, 0);
    }
}
