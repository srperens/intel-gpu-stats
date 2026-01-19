//! Windows-specific implementation using DXGI and D3DKMT APIs
//!
//! This module provides access to Intel GPU statistics on Windows systems
//! through the DXGI adapter enumeration and D3DKMT performance queries.

mod d3dkmt;
mod dxgi;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::types::*;

use d3dkmt::{D3dkmtAdapter, D3dkmtQueryStatistics};
use dxgi::DxgiEnumerator;

/// Handle for controlling background sampling
pub struct SamplingHandle {
    stop_flag: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SamplingHandle {
    /// Stop the background sampling
    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// Check if sampling is still running
    pub fn is_running(&self) -> bool {
        !self.stop_flag.load(Ordering::SeqCst)
    }
}

impl Drop for SamplingHandle {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Tracks engine usage over time for utilization calculation
struct EngineTracker {
    last_running_time: u64,
    last_timestamp: Instant,
}

impl EngineTracker {
    fn new() -> Self {
        Self {
            last_running_time: 0,
            last_timestamp: Instant::now(),
        }
    }

    fn update(&mut self, current_running_time: u64, now: Instant) -> f64 {
        let elapsed = now.duration_since(self.last_timestamp);
        let elapsed_ns = elapsed.as_nanos() as u64;

        let delta = current_running_time.saturating_sub(self.last_running_time);

        self.last_running_time = current_running_time;
        self.last_timestamp = now;

        if elapsed_ns > 0 {
            (delta as f64 / elapsed_ns as f64 * 100.0).min(100.0)
        } else {
            0.0
        }
    }
}

/// Intel GPU statistics reader for Windows
///
/// This struct provides access to Intel GPU statistics on Windows through
/// the D3DKMT API for performance queries.
pub struct IntelGpu {
    /// GPU information
    gpu_info: GpuInfo,
    /// D3DKMT adapter handle
    adapter: D3dkmtAdapter,
    /// Engine trackers for utilization calculation
    engine_trackers: HashMap<EngineClass, EngineTracker>,
    /// Last read timestamp
    last_timestamp: Instant,
    /// Whether compute engine is available
    has_compute: bool,
    /// Available node ordinals for each engine type
    node_mapping: HashMap<EngineClass, u32>,
}

impl IntelGpu {
    /// Detect and open the first available Intel GPU
    pub fn detect() -> Result<Self> {
        let gpus = Self::list_gpus()?;
        let gpu = gpus.into_iter().next().ok_or(Error::NoGpuFound)?;

        Self::open_gpu(gpu)
    }

    /// Open a specific GPU by card ID (e.g., "adapter0" or the LUID string)
    pub fn open(card_id: &str) -> Result<Self> {
        let gpus = Self::list_gpus()?;
        let gpu =
            gpus.into_iter()
                .find(|g| g.id == card_id)
                .ok_or_else(|| Error::DeviceNotFound {
                    path: card_id.into(),
                })?;

        Self::open_gpu(gpu)
    }

    /// List all available Intel GPUs
    pub fn list_gpus() -> Result<Vec<GpuInfo>> {
        let enumerator = DxgiEnumerator::new()?;
        enumerator.enumerate_intel_gpus()
    }

    /// Internal: open GPU with the given info
    fn open_gpu(gpu_info: GpuInfo) -> Result<Self> {
        // Open D3DKMT adapter
        let adapter = D3dkmtAdapter::open(&gpu_info)?;

        // Query adapter capabilities to determine available engines
        let node_mapping = adapter.query_node_mapping()?;
        let has_compute = node_mapping.contains_key(&EngineClass::Compute);

        // Initialize engine trackers
        let mut engine_trackers = HashMap::new();
        for engine_class in node_mapping.keys() {
            engine_trackers.insert(*engine_class, EngineTracker::new());
        }

        let mut gpu = Self {
            gpu_info,
            adapter,
            engine_trackers,
            last_timestamp: Instant::now(),
            has_compute,
            node_mapping,
        };

        // Prime the trackers with initial values
        let _ = gpu.read_stats();

        Ok(gpu)
    }

    /// Read current GPU statistics
    ///
    /// Returns a snapshot of the current GPU state. The utilization percentages
    /// are calculated based on the time elapsed since the last read.
    pub fn read_stats(&mut self) -> Result<GpuStats> {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_timestamp);
        let elapsed_ns = elapsed.as_nanos() as u64;

        let mut stats = GpuStats::new(now, elapsed_ns);

        // Read engine utilizations using D3DKMT
        let query = D3dkmtQueryStatistics::new(&self.adapter);

        // Query each engine type
        for (engine_class, node_ordinal) in &self.node_mapping {
            if let Ok(running_time) = query.query_node_running_time(*node_ordinal) {
                if let Some(tracker) = self.engine_trackers.get_mut(engine_class) {
                    let busy_percent = tracker.update(running_time, now);
                    let utilization = EngineUtilization::new(busy_percent, 0.0, 0.0);

                    match engine_class {
                        EngineClass::Render => stats.engines.render = utilization,
                        EngineClass::Video => stats.engines.video = utilization,
                        EngineClass::VideoEnhance => stats.engines.video_enhance = utilization,
                        EngineClass::Copy => stats.engines.blitter = utilization,
                        EngineClass::Compute => stats.engines.compute = Some(utilization),
                    }
                }
            }
        }

        // Query frequency if available
        if let Ok(freq) = query.query_frequency() {
            stats.frequency = freq;
        }

        // Query temperature if available (via WMI or driver-specific API)
        stats.temperature = query.query_temperature();

        // Query power if available
        stats.power = query.query_power();

        // Note: RC6 and detailed throttle info are not available through D3DKMT
        // These are Linux-specific concepts

        self.last_timestamp = now;

        Ok(stats)
    }

    /// Start continuous sampling with a callback
    ///
    /// The callback will be called with GPU statistics at the specified interval.
    /// Returns a handle that can be used to stop sampling.
    pub fn start_sampling<F>(
        mut self,
        interval: Duration,
        mut callback: F,
    ) -> Result<SamplingHandle>
    where
        F: FnMut(GpuStats) + Send + 'static,
    {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_clone = stop_flag.clone();

        let thread = thread::spawn(move || {
            while !stop_flag_clone.load(Ordering::SeqCst) {
                thread::sleep(interval);

                match self.read_stats() {
                    Ok(stats) => callback(stats),
                    Err(e) => {
                        eprintln!("Error reading GPU stats: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(SamplingHandle {
            stop_flag,
            thread: Some(thread),
        })
    }

    /// Get information about this GPU
    pub fn gpu_info(&self) -> &GpuInfo {
        &self.gpu_info
    }

    /// Check if compute engine is available (Intel Arc GPUs)
    pub fn has_compute_engine(&self) -> bool {
        self.has_compute
    }

    /// Get the driver type in use
    ///
    /// On Windows, we report as I915 for compatibility, though the actual
    /// driver is the Intel Graphics Driver for Windows.
    pub fn driver(&self) -> GpuDriver {
        // Windows uses a unified driver, report as I915 for API compatibility
        GpuDriver::I915
    }

    /// Check if temperature monitoring is available
    pub fn has_temperature(&self) -> bool {
        // Temperature monitoring may be available through WMI
        D3dkmtQueryStatistics::new(&self.adapter)
            .query_temperature()
            .is_some()
    }

    /// Check if fan speed monitoring is available
    pub fn has_fan(&self) -> bool {
        // Fan monitoring is typically not available on Windows for Intel GPUs
        false
    }

    /// Check if throttle monitoring is available
    pub fn has_throttle(&self) -> bool {
        // Detailed throttle info is not available through D3DKMT
        false
    }

    /// Check if power monitoring is available
    pub fn has_power(&self) -> bool {
        D3dkmtQueryStatistics::new(&self.adapter)
            .query_power()
            .is_some()
    }

    /// List all processes using the GPU
    ///
    /// Returns a list of processes that are using GPU resources.
    /// On Windows, this uses D3DKMT process queries.
    pub fn list_drm_clients() -> Vec<DrmClient> {
        d3dkmt::list_gpu_processes().unwrap_or_default()
    }

    /// Find processes using Quick Sync (video encode/decode)
    ///
    /// Returns only processes that are actively using the video
    /// or video_enhance engines.
    pub fn find_quicksync_clients() -> Vec<DrmClient> {
        Self::list_drm_clients()
            .into_iter()
            .filter(|c| c.is_using_quicksync())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_tracker() {
        let mut tracker = EngineTracker::new();
        let now = Instant::now();

        // Simulate 50% utilization over 100ms
        std::thread::sleep(Duration::from_millis(100));
        let percent = tracker.update(50_000_000, Instant::now()); // 50ms of running time

        // Should be roughly 50% (with some tolerance for timing)
        assert!(percent >= 40.0 && percent <= 60.0);
    }
}
