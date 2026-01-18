//! Hardware monitoring (hwmon) support for GPU temperature
//!
//! This module reads GPU temperature from the Linux hwmon subsystem.
//! Intel GPUs (especially discrete Arc GPUs) expose temperature via hwmon.

use std::fs;
use std::path::{Path, PathBuf};

use crate::types::TemperatureStats;

/// Find the hwmon path for an Intel GPU
///
/// Searches /sys/class/hwmon/ for a device that matches the GPU's PCI path
/// and has a name like "i915" or "xe".
pub fn find_gpu_hwmon(pci_path: &str) -> Option<PathBuf> {
    let hwmon_base = Path::new("/sys/class/hwmon");
    if !hwmon_base.exists() {
        return None;
    }

    let entries = fs::read_dir(hwmon_base).ok()?;

    for entry in entries.flatten() {
        let hwmon_path = entry.path();

        // Check the device symlink to see if it points to our GPU
        let device_link = hwmon_path.join("device");
        if let Ok(target) = fs::read_link(&device_link) {
            let target_str = target.to_string_lossy();

            // Check if this hwmon belongs to our GPU by matching PCI path
            if !pci_path.is_empty() && target_str.contains(pci_path) {
                return Some(hwmon_path);
            }
        }

        // Also check by name - look for i915 or xe
        let name_path = hwmon_path.join("name");
        if let Ok(name) = fs::read_to_string(&name_path) {
            let name = name.trim();
            if name == "i915" || name == "xe" {
                // Verify it's connected to a GPU by checking for temp inputs
                if hwmon_path.join("temp1_input").exists() {
                    return Some(hwmon_path);
                }
            }
        }
    }

    None
}

/// Read GPU temperature from hwmon
///
/// Returns the temperature in Celsius, or None if not available.
pub fn read_temperature(hwmon_path: &Path) -> Option<TemperatureStats> {
    // Try temp1_input first (most common)
    let temp_path = hwmon_path.join("temp1_input");
    if let Ok(temp_str) = fs::read_to_string(&temp_path) {
        if let Ok(millicelsius) = temp_str.trim().parse::<i64>() {
            // hwmon reports temperature in millidegrees Celsius
            let celsius = millicelsius as f64 / 1000.0;
            return Some(TemperatureStats::new(celsius));
        }
    }

    None
}

/// GPU hwmon reader
#[derive(Debug)]
pub struct HwmonReader {
    /// Path to the hwmon directory
    hwmon_path: Option<PathBuf>,
}

impl HwmonReader {
    /// Create a new hwmon reader for a GPU
    pub fn new(pci_path: &str) -> Self {
        let hwmon_path = find_gpu_hwmon(pci_path);
        Self { hwmon_path }
    }

    /// Check if hwmon is available for this GPU
    pub fn is_available(&self) -> bool {
        self.hwmon_path.is_some()
    }

    /// Read the current temperature
    pub fn read(&self) -> Option<TemperatureStats> {
        self.hwmon_path.as_ref().and_then(|p| read_temperature(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_temperature_stats() {
        let temp = TemperatureStats::new(45.0);
        assert!(!temp.is_high());
        assert!(!temp.is_critical());

        let temp = TemperatureStats::new(85.0);
        assert!(temp.is_high());
        assert!(!temp.is_critical());

        let temp = TemperatureStats::new(95.0);
        assert!(temp.is_high());
        assert!(temp.is_critical());
    }
}
