//! Intel GPU PMU (Performance Monitoring Unit) discovery and configuration
//!
//! Both i915 and xe drivers expose GPU performance counters via the Linux perf subsystem.
//! This module handles discovering the PMU and its available events for both drivers.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::types::{EngineClass, GpuDriver, GpuInfo};

/// Base path for PMU event sources
const PMU_BASE_PATH: &str = "/sys/bus/event_source/devices";

/// Intel vendor ID
pub const INTEL_VENDOR_ID: u16 = 0x8086;

/// Intel GPU PMU information
#[derive(Debug, Clone)]
pub struct PmuInfo {
    /// PMU type ID for perf_event_open
    pub type_id: u32,
    /// Path to the PMU sysfs directory
    pub path: PathBuf,
    /// Available events and their configs
    pub events: HashMap<String, u64>,
    /// Card ID this PMU belongs to (e.g., "card0")
    pub card_id: String,
    /// Driver type (i915 or xe)
    pub driver: GpuDriver,
}

impl PmuInfo {
    /// Get the config value for a named event
    pub fn event_config(&self, name: &str) -> Option<u64> {
        self.events.get(name).copied()
    }

    /// Build config for an engine busy event
    ///
    /// Config format: (class << 16) | (instance << 8) | sample_type
    pub fn engine_config(class: EngineClass, instance: u16, sample_type: u8) -> u64 {
        ((class as u64) << 16) | ((instance as u64) << 8) | (sample_type as u64)
    }

    /// Check if a specific event is available
    pub fn has_event(&self, name: &str) -> bool {
        self.events.contains_key(name)
    }
}

/// Discover Intel GPU PMU devices (both i915 and xe)
pub fn discover_pmu() -> Result<Vec<PmuInfo>> {
    let mut pmus = Vec::new();

    let pmu_base = Path::new(PMU_BASE_PATH);
    if !pmu_base.exists() {
        return Err(Error::PmuNotAvailable);
    }

    let entries = fs::read_dir(pmu_base).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            Error::permission_denied(&e)
        } else {
            Error::PmuNotAvailable
        }
    })?;

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();

        // Check for i915 PMU: "i915" or "i915-0000:00:02.0"
        if name.starts_with("i915") {
            if let Ok(pmu) = read_pmu_info(&entry.path(), &name, GpuDriver::I915) {
                pmus.push(pmu);
            }
        }
        // Check for xe PMU: "xe_0000_00_02.0" format
        else if name.starts_with("xe_") {
            if let Ok(pmu) = read_pmu_info(&entry.path(), &name, GpuDriver::Xe) {
                pmus.push(pmu);
            }
        }
    }

    if pmus.is_empty() {
        return Err(Error::PmuNotAvailable);
    }

    Ok(pmus)
}

/// Read PMU information from sysfs
fn read_pmu_info(path: &Path, name: &str, driver: GpuDriver) -> Result<PmuInfo> {
    // Read PMU type ID
    let type_path = path.join("type");
    let type_str = fs::read_to_string(&type_path)
        .map_err(|e| Error::sysfs_parse(&type_path, format!("failed to read type: {}", e)))?;
    let type_id: u32 = type_str
        .trim()
        .parse()
        .map_err(|e| Error::sysfs_parse(&type_path, format!("invalid type id: {}", e)))?;

    // Parse card ID from PMU name
    let card_id = parse_card_id(name, driver);

    // Read available events
    let events = read_pmu_events(path)?;

    Ok(PmuInfo {
        type_id,
        path: path.to_path_buf(),
        events,
        card_id,
        driver,
    })
}

/// Parse card ID from PMU name
///
/// PMU names can be:
/// - "i915" (single GPU, i915 driver)
/// - "i915-0000:00:02.0" (multi-GPU with PCI address, i915 driver)
/// - "xe_0000_00_02.0" (xe driver, uses underscores in PCI address)
fn parse_card_id(name: &str, driver: GpuDriver) -> String {
    match driver {
        GpuDriver::I915 => {
            if name == "i915" {
                return "card0".to_string();
            }
            // Try to find the card by PCI address: "i915-0000:00:02.0"
            if let Some(pci_addr) = name.strip_prefix("i915-") {
                if let Ok(card) = find_card_by_pci(pci_addr) {
                    return card;
                }
            }
        }
        GpuDriver::Xe => {
            // xe PMU names are like "xe_0000_00_02.0" (underscores instead of colons)
            if let Some(pci_part) = name.strip_prefix("xe_") {
                // Convert underscores to colons: "0000_00_02.0" -> "0000:00:02.0"
                let pci_addr = pci_part.replacen('_', ":", 2);
                if let Ok(card) = find_card_by_pci(&pci_addr) {
                    return card;
                }
            }
        }
    }

    "card0".to_string()
}

/// Find card ID by PCI address
fn find_card_by_pci(pci_addr: &str) -> Result<String> {
    let drm_path = Path::new("/sys/class/drm");
    if !drm_path.exists() {
        return Err(Error::NoGpuFound);
    }

    for entry in fs::read_dir(drm_path)
        .map_err(|_| Error::NoGpuFound)?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }

        // Check if this card matches the PCI address
        let device_link = entry.path().join("device");
        if let Ok(target) = fs::read_link(&device_link) {
            let target_str = target.to_string_lossy();
            if target_str.contains(pci_addr) {
                return Ok(name);
            }
        }
    }

    Err(Error::NoGpuFound)
}

/// Read PMU events from sysfs
fn read_pmu_events(pmu_path: &Path) -> Result<HashMap<String, u64>> {
    let events_path = pmu_path.join("events");
    let mut events = HashMap::new();

    if !events_path.exists() {
        return Ok(events);
    }

    let entries = match fs::read_dir(&events_path) {
        Ok(e) => e,
        Err(_) => return Ok(events),
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let event_path = entry.path();

        if let Ok(config_str) = fs::read_to_string(&event_path) {
            if let Some(config) = parse_event_config(&config_str) {
                events.insert(name, config);
            }
        }
    }

    Ok(events)
}

/// Parse event config from sysfs format
///
/// Format examples:
/// - "config=0x1"
/// - "config=1"
fn parse_event_config(config_str: &str) -> Option<u64> {
    let config_str = config_str.trim();

    // Look for "config=" or "config1=" etc.
    for part in config_str.split(',') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("config=") {
            return parse_hex_or_dec(value);
        }
    }

    // If no "config=" found, try parsing the whole string
    parse_hex_or_dec(config_str)
}

/// Parse a hex (0x...) or decimal number
fn parse_hex_or_dec(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        s.parse().ok()
    }
}

/// Discover Intel GPUs in the system
pub fn discover_gpus() -> Result<Vec<GpuInfo>> {
    let mut gpus = Vec::new();
    let drm_path = Path::new("/sys/class/drm");

    if !drm_path.exists() {
        return Err(Error::NoGpuFound);
    }

    let entries = fs::read_dir(drm_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            Error::permission_denied(&e)
        } else {
            Error::NoGpuFound
        }
    })?;

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();

        // Only look at card entries (not renderD*)
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }

        if let Ok(gpu) = read_gpu_info(&entry.path(), &name) {
            if gpu.is_intel() {
                gpus.push(gpu);
            }
        }
    }

    if gpus.is_empty() {
        return Err(Error::NoGpuFound);
    }

    Ok(gpus)
}

/// Read GPU information from sysfs
fn read_gpu_info(card_path: &Path, card_id: &str) -> Result<GpuInfo> {
    let device_path = card_path.join("device");

    // Read vendor ID
    let vendor_path = device_path.join("vendor");
    let vendor_str = fs::read_to_string(&vendor_path)
        .map_err(|e| Error::sysfs_parse(&vendor_path, format!("failed to read vendor: {}", e)))?;
    let vendor_id = parse_hex_or_dec(vendor_str.trim())
        .ok_or_else(|| Error::sysfs_parse(&vendor_path, "invalid vendor id"))?
        as u16;

    // Read device ID
    let device_id_path = device_path.join("device");
    let device_str = fs::read_to_string(&device_id_path).map_err(|e| {
        Error::sysfs_parse(&device_id_path, format!("failed to read device: {}", e))
    })?;
    let device_id = parse_hex_or_dec(device_str.trim())
        .ok_or_else(|| Error::sysfs_parse(&device_id_path, "invalid device id"))?
        as u16;

    // Get PCI path from device symlink
    let pci_path = fs::read_link(&device_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    // Detect driver in use
    let driver = detect_gpu_driver(&device_path);

    // Find render node
    let render_node = find_render_node(card_id);

    // Find card node
    let card_num = card_id.strip_prefix("card").unwrap_or("0");
    let card_node = format!("/dev/dri/card{}", card_num);
    let card_node = if Path::new(&card_node).exists() {
        Some(card_node)
    } else {
        None
    };

    // Try to get device name
    let device_name = get_device_name(device_id);

    Ok(GpuInfo {
        id: card_id.to_string(),
        pci_path,
        device_name,
        vendor_id,
        device_id,
        render_node,
        card_node,
        driver,
    })
}

/// Detect which kernel driver is in use for a GPU
fn detect_gpu_driver(device_path: &Path) -> Option<GpuDriver> {
    // The driver symlink points to the kernel driver module
    let driver_link = device_path.join("driver");
    if let Ok(target) = fs::read_link(&driver_link) {
        let driver_name = target
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        return match driver_name.as_str() {
            "i915" => Some(GpuDriver::I915),
            "xe" => Some(GpuDriver::Xe),
            _ => None,
        };
    }
    None
}

/// Find the render node for a card
fn find_render_node(card_id: &str) -> Option<String> {
    let card_num: u32 = card_id.strip_prefix("card")?.parse().ok()?;

    // Render nodes start at 128
    let render_num = 128 + card_num;
    let render_path = format!("/dev/dri/renderD{}", render_num);

    if Path::new(&render_path).exists() {
        Some(render_path)
    } else {
        None
    }
}

/// Get device name from device ID (basic mapping)
fn get_device_name(device_id: u16) -> Option<String> {
    // This is a simplified mapping - in practice you'd want a more complete database
    let name = match device_id {
        // Intel UHD Graphics (various generations)
        0x3e90..=0x3e92 | 0x3e98 => "Intel UHD Graphics 630",
        0x5917 => "Intel UHD Graphics 620",
        0x9a49 => "Intel UHD Graphics (11th Gen)",
        0x9a40 => "Intel UHD Graphics (11th Gen)",
        0x4680 => "Intel UHD Graphics 770",
        0x4692 => "Intel UHD Graphics 730",

        // Intel Iris
        0x8a52 => "Intel Iris Plus Graphics G7",
        0x8a56 => "Intel Iris Plus Graphics G1",
        0x9a78 => "Intel Iris Xe Graphics",
        0x46a6 => "Intel Iris Xe Graphics",

        // Intel Arc
        0x5690 => "Intel Arc A770M",
        0x5691 => "Intel Arc A730M",
        0x5692 => "Intel Arc A550M",
        0x56a0 => "Intel Arc A770",
        0x56a1 => "Intel Arc A750",
        0x56a5 => "Intel Arc A380",

        _ => return None,
    };

    Some(name.to_string())
}

/// Get available engine instances for a GPU
pub fn get_engine_instances(pmu: &PmuInfo) -> HashMap<EngineClass, Vec<u16>> {
    let mut engines: HashMap<EngineClass, Vec<u16>> = HashMap::new();

    for event_name in pmu.events.keys() {
        match pmu.driver {
            GpuDriver::I915 => {
                // i915 events: render-busy, video-busy, vcs0-busy, etc.
                if !event_name.ends_with("-busy") {
                    continue;
                }
                let prefix = event_name.strip_suffix("-busy").unwrap();
                match prefix {
                    "render" | "rcs0" => {
                        engines.entry(EngineClass::Render).or_default().push(0);
                    }
                    "blitter" | "bcs0" => {
                        engines.entry(EngineClass::Copy).or_default().push(0);
                    }
                    "video" | "vcs0" => {
                        engines.entry(EngineClass::Video).or_default().push(0);
                    }
                    "vcs1" => {
                        engines.entry(EngineClass::Video).or_default().push(1);
                    }
                    "video_enhance" | "vecs0" => {
                        engines
                            .entry(EngineClass::VideoEnhance)
                            .or_default()
                            .push(0);
                    }
                    "vecs1" => {
                        engines
                            .entry(EngineClass::VideoEnhance)
                            .or_default()
                            .push(1);
                    }
                    "compute" | "ccs0" => {
                        engines.entry(EngineClass::Compute).or_default().push(0);
                    }
                    _ if prefix.starts_with("ccs") => {
                        if let Ok(instance) = prefix[3..].parse::<u16>() {
                            engines
                                .entry(EngineClass::Compute)
                                .or_default()
                                .push(instance);
                        }
                    }
                    _ => {}
                }
            }
            GpuDriver::Xe => {
                // xe events: render-group-busy-gt0, copy-group-busy-gt0, media-group-busy-gt0, etc.
                if event_name.contains("-group-busy") {
                    if event_name.starts_with("render-group-busy") {
                        engines.entry(EngineClass::Render).or_default().push(0);
                    } else if event_name.starts_with("copy-group-busy") {
                        engines.entry(EngineClass::Copy).or_default().push(0);
                    } else if event_name.starts_with("media-group-busy") {
                        // xe uses "media" instead of "video"
                        engines.entry(EngineClass::Video).or_default().push(0);
                        engines
                            .entry(EngineClass::VideoEnhance)
                            .or_default()
                            .push(0);
                    } else if event_name.starts_with("compute-group-busy") {
                        engines.entry(EngineClass::Compute).or_default().push(0);
                    }
                }
            }
        }
    }

    // Deduplicate instances
    for instances in engines.values_mut() {
        instances.sort();
        instances.dedup();
    }

    // If no events found, assume basic engines exist
    if engines.is_empty() {
        engines.insert(EngineClass::Render, vec![0]);
        engines.insert(EngineClass::Copy, vec![0]);
        engines.insert(EngineClass::Video, vec![0]);
        engines.insert(EngineClass::VideoEnhance, vec![0]);
    }

    engines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_or_dec() {
        assert_eq!(parse_hex_or_dec("0x10"), Some(16));
        assert_eq!(parse_hex_or_dec("0X10"), Some(16));
        assert_eq!(parse_hex_or_dec("16"), Some(16));
        assert_eq!(parse_hex_or_dec("0xabc"), Some(0xabc));
        assert_eq!(parse_hex_or_dec("invalid"), None);
    }

    #[test]
    fn test_parse_event_config() {
        assert_eq!(parse_event_config("config=0x1"), Some(1));
        assert_eq!(parse_event_config("config=1"), Some(1));
        assert_eq!(parse_event_config("config=0x30000"), Some(0x30000));
    }

    #[test]
    fn test_engine_config() {
        // Render busy: class 0, instance 0, sample 0
        assert_eq!(PmuInfo::engine_config(EngineClass::Render, 0, 0), 0);

        // Video busy: class 2, instance 0, sample 0
        assert_eq!(PmuInfo::engine_config(EngineClass::Video, 0, 0), 0x20000);

        // VideoEnhance busy: class 3, instance 0, sample 0
        assert_eq!(
            PmuInfo::engine_config(EngineClass::VideoEnhance, 0, 0),
            0x30000
        );

        // Video wait: class 2, instance 0, sample 1
        assert_eq!(PmuInfo::engine_config(EngineClass::Video, 0, 1), 0x20001);
    }
}
