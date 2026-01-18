//! Data types for Intel GPU statistics

use std::time::Instant;

/// Information about a detected Intel GPU
#[derive(Debug, Clone)]
pub struct GpuInfo {
    /// Unique identifier for this GPU (e.g., "card0")
    pub id: String,
    /// PCI device path
    pub pci_path: String,
    /// Device name if available
    pub device_name: Option<String>,
    /// PCI vendor ID (should be 0x8086 for Intel)
    pub vendor_id: u16,
    /// PCI device ID
    pub device_id: u16,
    /// DRM render node path (e.g., /dev/dri/renderD128)
    pub render_node: Option<String>,
    /// DRM card node path (e.g., /dev/dri/card0)
    pub card_node: Option<String>,
}

impl GpuInfo {
    /// Returns true if this is an Intel GPU
    pub fn is_intel(&self) -> bool {
        self.vendor_id == 0x8086
    }
}

/// Complete GPU statistics snapshot
#[derive(Debug, Clone)]
pub struct GpuStats {
    /// When this snapshot was taken
    pub timestamp: Instant,
    /// Time elapsed since the last sample (for rate calculations)
    pub sample_duration_ns: u64,
    /// Engine utilization statistics
    pub engines: EngineStats,
    /// GPU frequency information
    pub frequency: FrequencyStats,
    /// Power consumption (if available via RAPL)
    pub power: Option<PowerStats>,
    /// RC6 power-saving state residency
    pub rc6: Option<Rc6Stats>,
}

impl GpuStats {
    /// Create a new GpuStats with the given timestamp
    pub fn new(timestamp: Instant, sample_duration_ns: u64) -> Self {
        Self {
            timestamp,
            sample_duration_ns,
            engines: EngineStats::default(),
            frequency: FrequencyStats::default(),
            power: None,
            rc6: None,
        }
    }
}

/// Statistics for all GPU engines
#[derive(Debug, Clone, Default)]
pub struct EngineStats {
    /// Render/3D engine (OpenGL/Vulkan)
    pub render: EngineUtilization,
    /// Video decode engine (Quick Sync decoder)
    pub video: EngineUtilization,
    /// Video enhance engine (Quick Sync encoder and video processing)
    pub video_enhance: EngineUtilization,
    /// Blitter/Copy engine
    pub blitter: EngineUtilization,
    /// Compute engine (Intel Arc and newer)
    pub compute: Option<EngineUtilization>,
}

impl EngineStats {
    /// Returns the overall maximum utilization across all engines
    pub fn max_utilization(&self) -> f64 {
        let mut max = self
            .render
            .busy_percent
            .max(self.video.busy_percent)
            .max(self.video_enhance.busy_percent)
            .max(self.blitter.busy_percent);

        if let Some(ref compute) = self.compute {
            max = max.max(compute.busy_percent);
        }

        max
    }

    /// Returns the Quick Sync utilization (video + video_enhance combined)
    pub fn quicksync_utilization(&self) -> f64 {
        self.video.busy_percent.max(self.video_enhance.busy_percent)
    }
}

/// Utilization statistics for a single GPU engine
#[derive(Debug, Clone, Default)]
pub struct EngineUtilization {
    /// Percentage of time the engine was actively processing (0.0 - 100.0)
    pub busy_percent: f64,
    /// Percentage of time the engine was waiting for memory (0.0 - 100.0)
    pub wait_percent: f64,
    /// Percentage of time the engine was waiting on semaphores (0.0 - 100.0)
    pub sema_percent: f64,
}

impl EngineUtilization {
    /// Create a new EngineUtilization with the given values
    pub fn new(busy_percent: f64, wait_percent: f64, sema_percent: f64) -> Self {
        Self {
            busy_percent,
            wait_percent,
            sema_percent,
        }
    }

    /// Returns true if this engine is idle
    pub fn is_idle(&self) -> bool {
        self.busy_percent < 0.1
    }

    /// Returns true if this engine is heavily loaded (>90% busy)
    pub fn is_busy(&self) -> bool {
        self.busy_percent > 90.0
    }
}

/// GPU frequency statistics
#[derive(Debug, Clone, Default)]
pub struct FrequencyStats {
    /// Actual current GPU frequency in MHz
    pub actual_mhz: u32,
    /// Requested GPU frequency in MHz
    pub requested_mhz: u32,
}

impl FrequencyStats {
    /// Create a new FrequencyStats
    pub fn new(actual_mhz: u32, requested_mhz: u32) -> Self {
        Self {
            actual_mhz,
            requested_mhz,
        }
    }

    /// Returns the frequency efficiency (actual / requested)
    pub fn efficiency(&self) -> f64 {
        if self.requested_mhz == 0 {
            0.0
        } else {
            (self.actual_mhz as f64 / self.requested_mhz as f64) * 100.0
        }
    }
}

/// Power consumption statistics
#[derive(Debug, Clone)]
pub struct PowerStats {
    /// GPU power draw in Watts
    pub gpu_watts: f64,
    /// Package power draw in Watts (if available)
    pub package_watts: Option<f64>,
}

impl PowerStats {
    /// Create a new PowerStats
    pub fn new(gpu_watts: f64, package_watts: Option<f64>) -> Self {
        Self {
            gpu_watts,
            package_watts,
        }
    }
}

/// RC6 power-saving state statistics
#[derive(Debug, Clone)]
pub struct Rc6Stats {
    /// Percentage of time in RC6 power-saving state (0.0 - 100.0)
    pub residency_percent: f64,
}

impl Rc6Stats {
    /// Create a new Rc6Stats
    pub fn new(residency_percent: f64) -> Self {
        Self { residency_percent }
    }

    /// Returns the active percentage (100 - residency)
    pub fn active_percent(&self) -> f64 {
        100.0 - self.residency_percent
    }
}

/// Engine class identifiers as defined in i915 driver
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum EngineClass {
    /// Render/3D engine
    Render = 0,
    /// Copy/Blitter engine
    Copy = 1,
    /// Video decode engine
    Video = 2,
    /// Video enhance/encode engine
    VideoEnhance = 3,
    /// Compute engine (Intel Arc)
    Compute = 4,
}

impl EngineClass {
    /// Get the engine class from a numeric value
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(EngineClass::Render),
            1 => Some(EngineClass::Copy),
            2 => Some(EngineClass::Video),
            3 => Some(EngineClass::VideoEnhance),
            4 => Some(EngineClass::Compute),
            _ => None,
        }
    }

    /// Get the display name for this engine class
    pub fn name(&self) -> &'static str {
        match self {
            EngineClass::Render => "Render/3D",
            EngineClass::Copy => "Blitter",
            EngineClass::Video => "Video",
            EngineClass::VideoEnhance => "VideoEnhance",
            EngineClass::Compute => "Compute",
        }
    }
}

/// Sample type identifiers for PMU events
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SampleType {
    /// Engine busy time
    Busy = 0,
    /// Engine wait time
    Wait = 1,
    /// Engine semaphore wait time
    Sema = 2,
}

impl SampleType {
    /// Get the sample type from a numeric value
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(SampleType::Busy),
            1 => Some(SampleType::Wait),
            2 => Some(SampleType::Sema),
            _ => None,
        }
    }
}
