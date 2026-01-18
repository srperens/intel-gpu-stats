//! Linux-specific implementation using i915/xe PMU via perf_event_open
//!
//! This module provides access to Intel GPU statistics on Linux systems
//! through the i915 or xe driver's PMU (Performance Monitoring Unit) interface.

pub mod fdinfo;
pub mod hwmon;
pub mod perf;
pub mod pmu;
pub mod rapl;
pub mod throttle;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::types::*;

use hwmon::HwmonReader;
use perf::{open_i915_event, PerfEvent};
use pmu::{discover_gpus, discover_pmu, get_engine_instances, PmuInfo};
use rapl::RaplReader;
use throttle::ThrottleReader;

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

/// Tracks the state of a single engine's counters
struct EngineCounters {
    busy: Option<PerfEvent>,
    wait: Option<PerfEvent>,
    sema: Option<PerfEvent>,
    last_busy: u64,
    last_wait: u64,
    last_sema: u64,
}

impl EngineCounters {
    fn new() -> Self {
        Self {
            busy: None,
            wait: None,
            sema: None,
            last_busy: 0,
            last_wait: 0,
            last_sema: 0,
        }
    }

    fn read_utilization(&mut self, elapsed_ns: u64) -> Result<EngineUtilization> {
        // Read busy delta
        let busy = if let Some(ref mut event) = self.busy {
            let current = event.read_value()?;
            let delta = current.saturating_sub(self.last_busy);
            self.last_busy = current;
            delta
        } else {
            0
        };

        // Read wait delta
        let wait = if let Some(ref mut event) = self.wait {
            let current = event.read_value()?;
            let delta = current.saturating_sub(self.last_wait);
            self.last_wait = current;
            delta
        } else {
            0
        };

        // Read sema delta
        let sema = if let Some(ref mut event) = self.sema {
            let current = event.read_value()?;
            let delta = current.saturating_sub(self.last_sema);
            self.last_sema = current;
            delta
        } else {
            0
        };

        let elapsed_ns = elapsed_ns as f64;
        let busy_percent = if elapsed_ns > 0.0 {
            (busy as f64 / elapsed_ns * 100.0).min(100.0)
        } else {
            0.0
        };
        let wait_percent = if elapsed_ns > 0.0 {
            (wait as f64 / elapsed_ns * 100.0).min(100.0)
        } else {
            0.0
        };
        let sema_percent = if elapsed_ns > 0.0 {
            (sema as f64 / elapsed_ns * 100.0).min(100.0)
        } else {
            0.0
        };

        Ok(EngineUtilization::new(
            busy_percent,
            wait_percent,
            sema_percent,
        ))
    }
}

/// Intel GPU statistics reader
///
/// This struct provides access to Intel GPU statistics on Linux through
/// the i915 or xe driver's PMU interface.
pub struct IntelGpu {
    /// PMU information
    pmu: PmuInfo,
    /// GPU information
    gpu_info: GpuInfo,
    /// Engine counters
    engines: HashMap<EngineClass, EngineCounters>,
    /// Frequency requested event
    freq_req: Option<PerfEvent>,
    /// Frequency actual event
    freq_act: Option<PerfEvent>,
    /// RC6 residency event
    rc6: Option<PerfEvent>,
    /// Last frequency requested value
    last_freq_req: u64,
    /// Last frequency actual value
    last_freq_act: u64,
    /// Last RC6 value
    last_rc6: u64,
    /// Last read timestamp
    last_timestamp: Instant,
    /// Whether compute engine is available
    has_compute: bool,
    /// Hwmon reader for temperature and fan speed
    hwmon: HwmonReader,
    /// Throttle reader
    throttle_reader: ThrottleReader,
    /// RAPL power reader
    rapl_reader: RaplReader,
}

impl IntelGpu {
    /// Detect and open the first available Intel GPU
    pub fn detect() -> Result<Self> {
        let gpus = discover_gpus()?;
        let gpu = gpus.into_iter().next().ok_or(Error::NoGpuFound)?;

        let pmus = discover_pmu()?;
        let pmu = pmus
            .into_iter()
            .find(|p| p.card_id == gpu.id)
            .or_else(|| {
                // Fallback: use the first PMU
                discover_pmu().ok()?.into_iter().next()
            })
            .ok_or(Error::PmuNotAvailable)?;

        Self::open_with_pmu(gpu, pmu)
    }

    /// Open a specific GPU by card ID (e.g., "card0")
    pub fn open(card_id: &str) -> Result<Self> {
        let gpus = discover_gpus()?;
        let gpu =
            gpus.into_iter()
                .find(|g| g.id == card_id)
                .ok_or_else(|| Error::DeviceNotFound {
                    path: card_id.into(),
                })?;

        let pmus = discover_pmu()?;
        let pmu = pmus
            .into_iter()
            .find(|p| p.card_id == gpu.id)
            .or_else(|| discover_pmu().ok()?.into_iter().next())
            .ok_or(Error::PmuNotAvailable)?;

        Self::open_with_pmu(gpu, pmu)
    }

    /// List all available Intel GPUs
    pub fn list_gpus() -> Result<Vec<GpuInfo>> {
        discover_gpus()
    }

    /// Internal: open GPU with specific PMU
    fn open_with_pmu(gpu_info: GpuInfo, pmu: PmuInfo) -> Result<Self> {
        let available_engines = get_engine_instances(&pmu);
        let has_compute = available_engines.contains_key(&EngineClass::Compute);

        // Initialize hwmon reader for temperature and fan speed
        let hwmon = HwmonReader::new(&gpu_info.pci_path);

        // Initialize throttle reader
        let throttle_reader = ThrottleReader::new(&gpu_info.id);

        // Initialize RAPL power reader
        let rapl_reader = RaplReader::new(&gpu_info.pci_path);

        let mut gpu = Self {
            pmu,
            gpu_info,
            engines: HashMap::new(),
            freq_req: None,
            freq_act: None,
            rc6: None,
            last_freq_req: 0,
            last_freq_act: 0,
            last_rc6: 0,
            last_timestamp: Instant::now(),
            has_compute,
            hwmon,
            throttle_reader,
            rapl_reader,
        };

        // Open engine events
        gpu.open_engine_events(&available_engines)?;

        // Open frequency events
        gpu.open_frequency_events()?;

        // Open RC6 event
        gpu.open_rc6_event()?;

        Ok(gpu)
    }

    /// Open perf events for all available engines
    fn open_engine_events(
        &mut self,
        available_engines: &HashMap<EngineClass, Vec<u16>>,
    ) -> Result<()> {
        let engine_classes = [
            EngineClass::Render,
            EngineClass::Copy,
            EngineClass::Video,
            EngineClass::VideoEnhance,
            EngineClass::Compute,
        ];

        for class in engine_classes {
            if let Some(instances) = available_engines.get(&class) {
                // Use instance 0 (primary) for each engine type
                if instances.contains(&0) {
                    if let Err(e) = self.open_engine(class, 0) {
                        // Log warning but continue - some engines may not be available
                        eprintln!("Warning: Could not open {} engine: {}", class.name(), e);
                    }
                }
            }
        }

        Ok(())
    }

    /// Open perf events for a specific engine
    fn open_engine(&mut self, class: EngineClass, instance: u16) -> Result<()> {
        let mut counters = EngineCounters::new();

        // Try to open busy counter (required)
        let busy_config = PmuInfo::engine_config(class, instance, 0);
        let busy_name = format!("{}-busy", class.name());
        counters.busy = Some(open_i915_event(self.pmu.type_id, busy_config, &busy_name)?);

        // Try to open wait counter (optional)
        let wait_config = PmuInfo::engine_config(class, instance, 1);
        let wait_name = format!("{}-wait", class.name());
        if let Ok(event) = open_i915_event(self.pmu.type_id, wait_config, &wait_name) {
            counters.wait = Some(event);
        }

        // Try to open sema counter (optional)
        let sema_config = PmuInfo::engine_config(class, instance, 2);
        let sema_name = format!("{}-sema", class.name());
        if let Ok(event) = open_i915_event(self.pmu.type_id, sema_config, &sema_name) {
            counters.sema = Some(event);
        }

        // Initialize last values
        if let Some(ref mut busy) = counters.busy {
            counters.last_busy = busy.read_value().unwrap_or(0);
        }
        if let Some(ref mut wait) = counters.wait {
            counters.last_wait = wait.read_value().unwrap_or(0);
        }
        if let Some(ref mut sema) = counters.sema {
            counters.last_sema = sema.read_value().unwrap_or(0);
        }

        self.engines.insert(class, counters);
        Ok(())
    }

    /// Open frequency events
    fn open_frequency_events(&mut self) -> Result<()> {
        // Try named events first
        if let Some(config) = self.pmu.event_config("actual-frequency") {
            if let Ok(event) = open_i915_event(self.pmu.type_id, config, "actual-frequency") {
                self.freq_act = Some(event);
            }
        }

        if let Some(config) = self.pmu.event_config("requested-frequency") {
            if let Ok(event) = open_i915_event(self.pmu.type_id, config, "requested-frequency") {
                self.freq_req = Some(event);
            }
        }

        // Initialize last values
        if let Some(ref mut freq) = self.freq_act {
            self.last_freq_act = freq.read_value().unwrap_or(0);
        }
        if let Some(ref mut freq) = self.freq_req {
            self.last_freq_req = freq.read_value().unwrap_or(0);
        }

        Ok(())
    }

    /// Open RC6 residency event
    fn open_rc6_event(&mut self) -> Result<()> {
        if let Some(config) = self.pmu.event_config("rc6-residency") {
            if let Ok(event) = open_i915_event(self.pmu.type_id, config, "rc6-residency") {
                self.rc6 = Some(event);
                if let Some(ref mut rc6) = self.rc6 {
                    self.last_rc6 = rc6.read_value().unwrap_or(0);
                }
            }
        }

        Ok(())
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

        // Read engine utilizations
        if let Some(counters) = self.engines.get_mut(&EngineClass::Render) {
            stats.engines.render = counters.read_utilization(elapsed_ns)?;
        }
        if let Some(counters) = self.engines.get_mut(&EngineClass::Video) {
            stats.engines.video = counters.read_utilization(elapsed_ns)?;
        }
        if let Some(counters) = self.engines.get_mut(&EngineClass::VideoEnhance) {
            stats.engines.video_enhance = counters.read_utilization(elapsed_ns)?;
        }
        if let Some(counters) = self.engines.get_mut(&EngineClass::Copy) {
            stats.engines.blitter = counters.read_utilization(elapsed_ns)?;
        }
        if let Some(counters) = self.engines.get_mut(&EngineClass::Compute) {
            stats.engines.compute = Some(counters.read_utilization(elapsed_ns)?);
        }

        // Read frequency
        stats.frequency = self.read_frequency(elapsed_ns)?;

        // Read RC6
        stats.rc6 = self.read_rc6(elapsed_ns)?;

        // Read temperature (and fan speed if available)
        stats.temperature = self.hwmon.read();

        // Read throttle information
        stats.throttle = self.throttle_reader.read();

        // Read power consumption
        stats.power = self.rapl_reader.read();

        self.last_timestamp = now;

        Ok(stats)
    }

    /// Read frequency statistics
    fn read_frequency(&mut self, elapsed_ns: u64) -> Result<FrequencyStats> {
        let mut actual_mhz = 0u32;
        let mut requested_mhz = 0u32;

        if let Some(ref mut freq) = self.freq_act {
            let current = freq.read_value()?;
            let delta = current.saturating_sub(self.last_freq_act);
            self.last_freq_act = current;

            // Frequency is reported in MHz * ns, so divide by elapsed ns to get MHz
            if elapsed_ns > 0 {
                actual_mhz = (delta / elapsed_ns) as u32;
            }
        }

        if let Some(ref mut freq) = self.freq_req {
            let current = freq.read_value()?;
            let delta = current.saturating_sub(self.last_freq_req);
            self.last_freq_req = current;

            if elapsed_ns > 0 {
                requested_mhz = (delta / elapsed_ns) as u32;
            }
        }

        Ok(FrequencyStats::new(actual_mhz, requested_mhz))
    }

    /// Read RC6 residency
    fn read_rc6(&mut self, elapsed_ns: u64) -> Result<Option<Rc6Stats>> {
        if let Some(ref mut rc6) = self.rc6 {
            let current = rc6.read_value()?;
            let delta = current.saturating_sub(self.last_rc6);
            self.last_rc6 = current;

            let residency_percent = if elapsed_ns > 0 {
                (delta as f64 / elapsed_ns as f64 * 100.0).min(100.0)
            } else {
                0.0
            };

            Ok(Some(Rc6Stats::new(residency_percent)))
        } else {
            Ok(None)
        }
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
    pub fn driver(&self) -> GpuDriver {
        self.pmu.driver
    }

    /// Check if temperature monitoring is available
    pub fn has_temperature(&self) -> bool {
        self.hwmon.is_available()
    }

    /// Check if fan speed monitoring is available
    pub fn has_fan(&self) -> bool {
        self.hwmon.has_fan()
    }

    /// Check if throttle monitoring is available
    pub fn has_throttle(&self) -> bool {
        self.throttle_reader.is_available()
    }

    /// Check if power monitoring is available
    pub fn has_power(&self) -> bool {
        self.rapl_reader.is_available()
    }

    /// List all processes using the GPU (DRM clients)
    ///
    /// Returns a list of processes that have open file descriptors
    /// to the GPU's DRM render node, along with their GPU usage.
    pub fn list_drm_clients() -> Vec<DrmClient> {
        fdinfo::list_drm_clients()
    }

    /// Find processes using Quick Sync (video encode/decode)
    ///
    /// Returns only processes that are actively using the video
    /// or video_enhance engines.
    pub fn find_quicksync_clients() -> Vec<DrmClient> {
        fdinfo::find_quicksync_clients()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_utilization() {
        let util = EngineUtilization::new(50.0, 10.0, 5.0);
        assert!(!util.is_idle());
        assert!(!util.is_busy());
    }

    #[test]
    fn test_frequency_stats() {
        let freq = FrequencyStats::new(1000, 1200);
        assert!((freq.efficiency() - 83.33).abs() < 1.0);
    }
}
