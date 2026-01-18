//! GPU throttle detection
//!
//! This module reads throttle reasons from sysfs for Intel GPUs.
//! The throttle information is exposed at:
//! /sys/class/drm/card0/gt/gt0/throttle_reason_*
//!
//! Common throttle reasons:
//! - status: General throttle status
//! - pl1: Power Limit 1 exceeded
//! - thermal: Thermal throttling
//! - prochot: PROCHOT signal from CPU
//! - ratl: Running Average Thermal Limit
//! - vr_thermalert: VR thermal alert
//! - vr_tdc: VR Thermal Design Current

use std::fs;
use std::path::{Path, PathBuf};

use crate::types::ThrottleInfo;

/// Find the GT (Graphics Tile) path for a card
fn find_gt_path(card_id: &str) -> Option<PathBuf> {
    // Try gt0 first (most common)
    let gt0_path = format!("/sys/class/drm/{}/gt/gt0", card_id);
    if Path::new(&gt0_path).exists() {
        return Some(PathBuf::from(gt0_path));
    }

    // Try direct gt path (older kernels)
    let gt_path = format!("/sys/class/drm/{}/gt", card_id);
    if Path::new(&gt_path).exists() {
        return Some(PathBuf::from(gt_path));
    }

    // Try device path (some drivers)
    let device_gt = format!("/sys/class/drm/{}/device/gt", card_id);
    if Path::new(&device_gt).exists() {
        return Some(PathBuf::from(device_gt));
    }

    None
}

/// Read a throttle reason file (returns true if throttle is active)
fn read_throttle_file(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

/// Read throttle information from sysfs
pub fn read_throttle_info(card_id: &str) -> Option<ThrottleInfo> {
    let gt_path = find_gt_path(card_id)?;

    let mut info = ThrottleInfo::new();

    // Read each throttle reason file
    let status_path = gt_path.join("throttle_reason_status");
    if status_path.exists() {
        info.status = read_throttle_file(&status_path);
    }

    let pl1_path = gt_path.join("throttle_reason_pl1");
    if pl1_path.exists() {
        info.power_limit = read_throttle_file(&pl1_path);
    }

    let thermal_path = gt_path.join("throttle_reason_thermal");
    if thermal_path.exists() {
        info.thermal = read_throttle_file(&thermal_path);
    }

    let prochot_path = gt_path.join("throttle_reason_prochot");
    if prochot_path.exists() {
        info.prochot = read_throttle_file(&prochot_path);
    }

    let ratl_path = gt_path.join("throttle_reason_ratl");
    if ratl_path.exists() {
        info.ratl = read_throttle_file(&ratl_path);
    }

    let vr_thermal_path = gt_path.join("throttle_reason_vr_thermalert");
    if vr_thermal_path.exists() {
        info.vr_thermal = read_throttle_file(&vr_thermal_path);
    }

    let vr_tdc_path = gt_path.join("throttle_reason_vr_tdc");
    if vr_tdc_path.exists() {
        info.vr_tdc = read_throttle_file(&vr_tdc_path);
    }

    // Set overall throttled flag
    info.is_throttled = info.any_throttling();

    Some(info)
}

/// Throttle reader for continuous monitoring
#[derive(Debug)]
pub struct ThrottleReader {
    /// Card ID (e.g., "card0")
    card_id: String,
    /// Path to the GT directory
    gt_path: Option<PathBuf>,
}

impl ThrottleReader {
    /// Create a new throttle reader for a card
    pub fn new(card_id: &str) -> Self {
        let gt_path = find_gt_path(card_id);
        Self {
            card_id: card_id.to_string(),
            gt_path,
        }
    }

    /// Check if throttle monitoring is available
    pub fn is_available(&self) -> bool {
        self.gt_path.is_some()
    }

    /// Read current throttle information
    pub fn read(&self) -> Option<ThrottleInfo> {
        read_throttle_info(&self.card_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_throttle_info_default() {
        let info = ThrottleInfo::new();
        assert!(!info.any_throttling());
        assert!(!info.is_throttled);
    }

    #[test]
    fn test_throttle_info_any() {
        let mut info = ThrottleInfo::new();
        assert!(!info.any_throttling());

        info.thermal = true;
        assert!(info.any_throttling());

        info.thermal = false;
        info.power_limit = true;
        assert!(info.any_throttling());
    }
}
