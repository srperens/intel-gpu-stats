//! Error types for intel-gpu-stats

use std::io;
use std::path::PathBuf;
use thiserror::Error;

/// Result type alias for intel-gpu-stats operations
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur when working with Intel GPU statistics
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum Error {
    /// No Intel GPU was found on this system
    #[error("No Intel GPU found on this system")]
    NoGpuFound,

    /// The specified GPU device was not found
    #[error("GPU device not found: {path}")]
    DeviceNotFound {
        /// The path that was not found
        path: PathBuf,
    },

    /// The i915 PMU (Performance Monitoring Unit) is not available
    #[error("i915 PMU not available - ensure you have an Intel GPU with i915 driver loaded")]
    PmuNotAvailable,

    /// A specific PMU event is not supported by this GPU/driver
    #[error("PMU event not supported: {event}")]
    EventNotSupported {
        /// The event name that is not supported
        event: String,
    },

    /// Permission denied when accessing GPU statistics
    #[error("Permission denied: {message}. Try running as root, adding user to 'render' group, or granting CAP_PERFMON capability")]
    PermissionDenied {
        /// Description of the permission error
        message: String,
    },

    /// Error opening a perf event
    #[error("Failed to open perf event for {event}: {source}")]
    PerfEventOpen {
        /// The event that failed to open
        event: String,
        /// The underlying IO error
        source: io::Error,
    },

    /// Error reading from a perf event file descriptor
    #[error("Failed to read perf event: {0}")]
    PerfEventRead(#[from] io::Error),

    /// Error parsing sysfs data
    #[error("Failed to parse sysfs data at {path}: {message}")]
    SysfsParse {
        /// The sysfs path that failed to parse
        path: PathBuf,
        /// Description of the parse error
        message: String,
    },

    /// The GPU was disconnected or became unavailable
    #[error("GPU became unavailable during operation")]
    GpuUnavailable,

    /// Sampling is already active
    #[error("Sampling is already active - stop current sampling before starting new")]
    SamplingAlreadyActive,

    /// Sampling is not active
    #[error("No active sampling to stop")]
    SamplingNotActive,

    /// Invalid configuration
    #[error("Invalid configuration: {message}")]
    InvalidConfig {
        /// Description of the configuration error
        message: String,
    },

    /// Platform not supported
    #[error("This platform is not currently supported")]
    PlatformNotSupported,

    /// Engine instance not found
    #[error("Engine {class}:{instance} not found")]
    EngineNotFound {
        /// The engine class
        class: u16,
        /// The engine instance
        instance: u16,
    },
}

impl Error {
    /// Returns true if this error is due to insufficient permissions
    pub fn is_permission_error(&self) -> bool {
        matches!(self, Error::PermissionDenied { .. })
    }

    /// Returns true if the error indicates a missing GPU
    pub fn is_gpu_missing(&self) -> bool {
        matches!(
            self,
            Error::NoGpuFound | Error::DeviceNotFound { .. } | Error::GpuUnavailable
        )
    }

    /// Create a permission denied error from an IO error
    pub(crate) fn permission_denied(source: &io::Error) -> Self {
        Error::PermissionDenied {
            message: source.to_string(),
        }
    }

    /// Create a sysfs parse error
    pub(crate) fn sysfs_parse(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Error::SysfsParse {
            path: path.into(),
            message: message.into(),
        }
    }
}
