//! Intel GPU Statistics Library
//!
//! A cross-platform Rust library for reading Intel GPU statistics in real-time.
//! Designed for monitoring GPU usage in broadcast/media applications, specifically
//! for showing Quick Sync encoder/decoder load.
//!
//! # Platform Support
//!
//! - **Linux** (primary): Via i915 PMU and `perf_event_open` syscall
//! - **Windows** (planned): Via D3DKMT API
//!
//! # Features
//!
//! - Engine utilization (Render, Video, VideoEnhance, Blitter, Compute)
//! - GPU frequency (actual and requested)
//! - RC6 power-saving state residency
//! - Continuous sampling with callbacks
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use intel_gpu_stats::IntelGpu;
//!
//! // Detect and open the first Intel GPU
//! let mut gpu = IntelGpu::detect()?;
//!
//! // Read current statistics
//! let stats = gpu.read_stats()?;
//!
//! println!("Render: {:.1}%", stats.engines.render.busy_percent);
//! println!("Video: {:.1}%", stats.engines.video.busy_percent);
//! println!("VideoEnhance: {:.1}%", stats.engines.video_enhance.busy_percent);
//! println!("Frequency: {} MHz", stats.frequency.actual_mhz);
//! # Ok::<(), intel_gpu_stats::Error>(())
//! ```
//!
//! # Permissions
//!
//! On Linux, reading GPU statistics requires one of:
//! - Root privileges
//! - Membership in the `render` group
//! - The `CAP_PERFMON` capability
//!
//! # Example with Continuous Sampling
//!
//! ```rust,no_run
//! use intel_gpu_stats::IntelGpu;
//! use std::time::Duration;
//!
//! let gpu = IntelGpu::detect()?;
//!
//! // Start sampling every 100ms
//! let handle = gpu.start_sampling(Duration::from_millis(100), |stats| {
//!     println!("Quick Sync: {:.1}%", stats.engines.quicksync_utilization());
//! })?;
//!
//! // Do other work...
//! std::thread::sleep(Duration::from_secs(5));
//!
//! // Stop sampling
//! handle.stop();
//! # Ok::<(), intel_gpu_stats::Error>(())
//! ```

#![warn(missing_docs)]
#![warn(rust_2018_idioms)]

pub mod error;
pub mod types;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "windows")]
pub mod windows;

// Re-export main types at crate root
pub use error::{Error, Result};
pub use types::*;

#[cfg(target_os = "linux")]
pub use linux::{IntelGpu, SamplingHandle};

// Placeholder for Windows support
#[cfg(target_os = "windows")]
compile_error!("Windows support is not yet implemented");

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Check if the current platform is supported
pub fn is_platform_supported() -> bool {
    cfg!(target_os = "linux")
}

/// Get a human-readable description of the current platform support status
pub fn platform_support_status() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "Linux: Fully supported via i915 PMU"
    }

    #[cfg(target_os = "windows")]
    {
        "Windows: Not yet implemented"
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        "This platform is not supported"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn test_platform_support() {
        let status = platform_support_status();
        assert!(!status.is_empty());
    }
}
