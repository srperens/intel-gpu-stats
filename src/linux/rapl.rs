//! RAPL (Running Average Power Limit) power monitoring
//!
//! This module reads GPU power consumption from the Linux powercap RAPL interface.
//! Intel GPUs may expose power data through various RAPL domains.
//!
//! The power data is typically found at:
//! - /sys/class/powercap/intel-rapl:0/ (package power)
//! - /sys/class/powercap/intel-rapl:0:2/ (uncore/GPU power, if available)
//!
//! Some discrete GPUs also expose power via hwmon.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::types::PowerStats;

/// RAPL power reader
#[derive(Debug)]
pub struct RaplReader {
    /// Path to package energy file
    package_energy_path: Option<PathBuf>,
    /// Path to GPU/uncore energy file (if available)
    gpu_energy_path: Option<PathBuf>,
    /// Path to hwmon power file (discrete GPUs)
    hwmon_power_path: Option<PathBuf>,
    /// Last package energy reading (microjoules)
    last_package_uj: u64,
    /// Last GPU energy reading (microjoules)
    last_gpu_uj: u64,
    /// Last read timestamp
    last_timestamp: Instant,
}

impl RaplReader {
    /// Create a new RAPL reader
    ///
    /// Searches for available power measurement interfaces.
    pub fn new(pci_path: &str) -> Self {
        let (package_path, gpu_path) = find_rapl_paths();
        let hwmon_path = find_hwmon_power(pci_path);

        let mut reader = Self {
            package_energy_path: package_path,
            gpu_energy_path: gpu_path,
            hwmon_power_path: hwmon_path,
            last_package_uj: 0,
            last_gpu_uj: 0,
            last_timestamp: Instant::now(),
        };

        // Initialize with current readings
        if let Some(ref path) = reader.package_energy_path {
            reader.last_package_uj = read_energy_uj(path).unwrap_or(0);
        }
        if let Some(ref path) = reader.gpu_energy_path {
            reader.last_gpu_uj = read_energy_uj(path).unwrap_or(0);
        }
        reader.last_timestamp = Instant::now();

        reader
    }

    /// Check if any power monitoring is available
    pub fn is_available(&self) -> bool {
        self.package_energy_path.is_some()
            || self.gpu_energy_path.is_some()
            || self.hwmon_power_path.is_some()
    }

    /// Check if GPU-specific power is available
    pub fn has_gpu_power(&self) -> bool {
        self.gpu_energy_path.is_some() || self.hwmon_power_path.is_some()
    }

    /// Read current power consumption
    ///
    /// Returns power in watts calculated from energy delta since last read.
    pub fn read(&mut self) -> Option<PowerStats> {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_timestamp);
        let elapsed_us = elapsed.as_micros() as f64;

        if elapsed_us < 1000.0 {
            // Need at least 1ms between reads
            return None;
        }

        // First try hwmon direct power reading (discrete GPUs)
        if let Some(ref path) = self.hwmon_power_path {
            if let Some(power_uw) = read_power_uw(path) {
                let gpu_watts = power_uw as f64 / 1_000_000.0;

                // Also read package if available
                let package_watts = self.read_package_watts(elapsed_us);

                self.last_timestamp = now;
                return Some(PowerStats::new(gpu_watts, package_watts));
            }
        }

        // Fall back to RAPL energy counters
        let package_watts = self.read_package_watts(elapsed_us);

        let gpu_watts = if let Some(ref path) = self.gpu_energy_path {
            if let Some(current_uj) = read_energy_uj(path) {
                let delta = current_uj.saturating_sub(self.last_gpu_uj);
                self.last_gpu_uj = current_uj;
                Some(delta as f64 / elapsed_us) // uJ/us = W
            } else {
                None
            }
        } else {
            None
        };

        self.last_timestamp = now;

        // Return stats if we have any power reading
        if gpu_watts.is_some() || package_watts.is_some() {
            Some(PowerStats::new(gpu_watts.unwrap_or(0.0), package_watts))
        } else {
            None
        }
    }

    /// Read package power in watts
    fn read_package_watts(&mut self, elapsed_us: f64) -> Option<f64> {
        if let Some(ref path) = self.package_energy_path {
            if let Some(current_uj) = read_energy_uj(path) {
                let delta = current_uj.saturating_sub(self.last_package_uj);
                self.last_package_uj = current_uj;
                return Some(delta as f64 / elapsed_us); // uJ/us = W
            }
        }
        None
    }
}

/// Find RAPL sysfs paths
fn find_rapl_paths() -> (Option<PathBuf>, Option<PathBuf>) {
    let powercap_base = Path::new("/sys/class/powercap");
    if !powercap_base.exists() {
        return (None, None);
    }

    let mut package_path = None;
    let mut gpu_path = None;

    // Look for intel-rapl domains
    let entries = match fs::read_dir(powercap_base) {
        Ok(e) => e,
        Err(_) => return (None, None),
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with("intel-rapl:") {
            let domain_path = entry.path();

            // Check what type of domain this is
            let name_file = domain_path.join("name");
            if let Ok(domain_name) = fs::read_to_string(&name_file) {
                let domain_name = domain_name.trim();

                if domain_name == "package-0" || domain_name.starts_with("package") {
                    let energy_path = domain_path.join("energy_uj");
                    if energy_path.exists() {
                        package_path = Some(energy_path);
                    }
                }

                // Look for GPU/uncore domain
                if domain_name == "uncore" || domain_name.contains("gpu") {
                    let energy_path = domain_path.join("energy_uj");
                    if energy_path.exists() {
                        gpu_path = Some(energy_path);
                    }
                }
            }

            // Also check subdirectories for uncore
            if let Ok(subentries) = fs::read_dir(&domain_path) {
                for subentry in subentries.flatten() {
                    let subname = subentry.file_name();
                    let subname_str = subname.to_string_lossy();

                    if subname_str.contains("intel-rapl:") {
                        let sub_path = subentry.path();
                        let name_file = sub_path.join("name");

                        if let Ok(sub_domain_name) = fs::read_to_string(&name_file) {
                            if sub_domain_name.trim() == "uncore" {
                                let energy_path = sub_path.join("energy_uj");
                                if energy_path.exists() {
                                    gpu_path = Some(energy_path);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    (package_path, gpu_path)
}

/// Find hwmon power interface for discrete GPUs
fn find_hwmon_power(pci_path: &str) -> Option<PathBuf> {
    let hwmon_base = Path::new("/sys/class/hwmon");
    if !hwmon_base.exists() {
        return None;
    }

    let entries = fs::read_dir(hwmon_base).ok()?;

    for entry in entries.flatten() {
        let hwmon_path = entry.path();

        // Check if this hwmon belongs to our GPU
        let device_link = hwmon_path.join("device");
        if let Ok(target) = fs::read_link(&device_link) {
            let target_str = target.to_string_lossy();
            if !pci_path.is_empty() && target_str.contains(pci_path) {
                // Found the right hwmon, look for power
                let power_path = hwmon_path.join("power1_input");
                if power_path.exists() {
                    return Some(power_path);
                }
            }
        }
    }

    None
}

/// Read energy in microjoules from a RAPL energy file
fn read_energy_uj(path: &Path) -> Option<u64> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Read power in microwatts from an hwmon power file
fn read_power_uw(path: &Path) -> Option<u64> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_power_stats() {
        let stats = PowerStats::new(15.5, Some(45.0));
        assert!((stats.gpu_watts - 15.5).abs() < 0.01);
        assert!((stats.package_watts.unwrap() - 45.0).abs() < 0.01);
    }

    #[test]
    fn test_rapl_reader_creation() {
        // Just test that creation doesn't panic
        let reader = RaplReader::new("");
        // Can't test much without actual hardware
        let _ = reader.is_available();
    }
}
