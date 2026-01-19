//! DXGI adapter enumeration for finding Intel GPUs

use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, DXGI_ADAPTER_DESC1,
};

use crate::error::{Error, Result};
use crate::types::GpuInfo;

/// Intel vendor ID
const INTEL_VENDOR_ID: u32 = 0x8086;

/// DXGI factory wrapper for GPU enumeration
pub struct DxgiEnumerator {
    factory: IDXGIFactory1,
}

impl DxgiEnumerator {
    /// Create a new DXGI enumerator
    pub fn new() -> Result<Self> {
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(|e| Error::Io {
            context: format!("Failed to create DXGI factory: {}", e),
            source: std::io::Error::new(std::io::ErrorKind::Other, e.to_string()),
        })?;

        Ok(Self { factory })
    }

    /// Enumerate all Intel GPUs
    pub fn enumerate_intel_gpus(&self) -> Result<Vec<GpuInfo>> {
        let mut gpus = Vec::new();
        let mut adapter_index = 0u32;

        loop {
            let adapter: IDXGIAdapter1 = match unsafe { self.factory.EnumAdapters1(adapter_index) }
            {
                Ok(adapter) => adapter,
                Err(_) => break, // No more adapters
            };

            if let Ok(desc) = unsafe { adapter.GetDesc1() } {
                // Check if this is an Intel GPU
                if desc.VendorId == INTEL_VENDOR_ID {
                    let gpu_info = adapter_desc_to_gpu_info(&desc, adapter_index);
                    gpus.push(gpu_info);
                }
            }

            adapter_index += 1;
        }

        Ok(gpus)
    }

    /// Enumerate all GPUs (including non-Intel)
    #[allow(dead_code)]
    pub fn enumerate_all_gpus(&self) -> Result<Vec<GpuInfo>> {
        let mut gpus = Vec::new();
        let mut adapter_index = 0u32;

        loop {
            let adapter: IDXGIAdapter1 = match unsafe { self.factory.EnumAdapters1(adapter_index) }
            {
                Ok(adapter) => adapter,
                Err(_) => break,
            };

            if let Ok(desc) = unsafe { adapter.GetDesc1() } {
                let gpu_info = adapter_desc_to_gpu_info(&desc, adapter_index);
                gpus.push(gpu_info);
            }

            adapter_index += 1;
        }

        Ok(gpus)
    }

    /// Get adapter by index
    #[allow(dead_code)]
    pub fn get_adapter(&self, index: u32) -> Result<IDXGIAdapter1> {
        unsafe { self.factory.EnumAdapters1(index) }.map_err(|e| Error::DeviceNotFound {
            path: format!("adapter{}: {}", index, e).into(),
        })
    }

    /// Get adapter by LUID
    #[allow(dead_code)]
    pub fn get_adapter_by_luid(&self, luid: i64) -> Result<IDXGIAdapter1> {
        let mut adapter_index = 0u32;

        loop {
            let adapter: IDXGIAdapter1 = match unsafe { self.factory.EnumAdapters1(adapter_index) }
            {
                Ok(adapter) => adapter,
                Err(_) => break,
            };

            if let Ok(desc) = unsafe { adapter.GetDesc1() } {
                let adapter_luid =
                    ((desc.AdapterLuid.HighPart as i64) << 32) | (desc.AdapterLuid.LowPart as i64);
                if adapter_luid == luid {
                    return Ok(adapter);
                }
            }

            adapter_index += 1;
        }

        Err(Error::DeviceNotFound {
            path: format!("LUID:{}", luid).into(),
        })
    }
}

/// Convert DXGI adapter description to GpuInfo
fn adapter_desc_to_gpu_info(desc: &DXGI_ADAPTER_DESC1, adapter_index: u32) -> GpuInfo {
    // Convert wide string description to Rust string
    let device_name = wchar_to_string(&desc.Description);

    // Create a unique ID from the LUID
    let luid = ((desc.AdapterLuid.HighPart as i64) << 32) | (desc.AdapterLuid.LowPart as i64);
    let id = format!("adapter{}", adapter_index);

    // Create PCI-style path from LUID
    let pci_path = format!("LUID:{:016x}", luid);

    GpuInfo {
        id,
        pci_path,
        device_name: Some(device_name),
        vendor_id: desc.VendorId as u16,
        device_id: desc.DeviceId as u16,
        render_node: None, // Not applicable on Windows
        card_node: None,   // Not applicable on Windows
        driver: None,      // Windows uses unified driver
    }
}

/// Convert a null-terminated wide character array to a Rust string
fn wchar_to_string(wchar: &[u16]) -> String {
    let len = wchar.iter().position(|&c| c == 0).unwrap_or(wchar.len());
    String::from_utf16_lossy(&wchar[..len])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wchar_to_string() {
        let wchar: [u16; 10] = [
            'H' as u16, 'e' as u16, 'l' as u16, 'l' as u16, 'o' as u16, 0, 0, 0, 0, 0,
        ];
        assert_eq!(wchar_to_string(&wchar), "Hello");
    }

    #[test]
    fn test_wchar_to_string_no_null() {
        let wchar: [u16; 5] = ['H' as u16, 'e' as u16, 'l' as u16, 'l' as u16, 'o' as u16];
        assert_eq!(wchar_to_string(&wchar), "Hello");
    }
}
