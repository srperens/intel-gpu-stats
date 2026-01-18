//! DRM fdinfo parser for per-process GPU usage
//!
//! This module reads GPU usage per process from /proc/<pid>/fdinfo/
//! when a process has an open file descriptor to a DRM render node.
//!
//! The fdinfo format for i915 contains lines like:
//! ```text
//! drm-driver:     i915
//! drm-client-id:  123
//! drm-engine-render:      12345678 ns
//! drm-engine-copy:        0 ns
//! drm-engine-video:       0 ns
//! drm-engine-video-enhance:       0 ns
//! drm-memory-resident:    1234567
//! ```

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::types::DrmClient;

/// Parse fdinfo for a specific file descriptor
fn parse_fdinfo(pid: u32, fd: &str) -> Option<FdinfoData> {
    let fdinfo_path = format!("/proc/{}/fdinfo/{}", pid, fd);
    let file = File::open(&fdinfo_path).ok()?;
    let reader = BufReader::new(file);

    let mut data = FdinfoData::default();
    let mut is_i915_or_xe = false;

    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim();

        if line.starts_with("drm-driver:") {
            let driver = line.split(':').nth(1)?.trim();
            if driver == "i915" || driver == "xe" {
                is_i915_or_xe = true;
            }
        } else if line.starts_with("drm-client-id:") {
            data.client_id = line.split(':').nth(1)?.trim().parse().ok();
        } else if line.starts_with("drm-engine-render:") {
            data.render_ns = parse_engine_ns(line);
        } else if line.starts_with("drm-engine-copy:") {
            data.copy_ns = parse_engine_ns(line);
        } else if line.starts_with("drm-engine-video:") && !line.contains("video-enhance") {
            data.video_ns = parse_engine_ns(line);
        } else if line.starts_with("drm-engine-video-enhance:") {
            data.video_enhance_ns = parse_engine_ns(line);
        } else if line.starts_with("drm-engine-compute:") {
            data.compute_ns = parse_engine_ns(line);
        } else if line.starts_with("drm-memory-resident:") {
            data.memory_bytes = parse_memory_bytes(line);
        }
    }

    if is_i915_or_xe {
        Some(data)
    } else {
        None
    }
}

/// Parse engine time in nanoseconds from a line like "drm-engine-render: 12345 ns"
fn parse_engine_ns(line: &str) -> u64 {
    line.split(':')
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Parse memory in bytes from a line like "drm-memory-resident: 1234567"
fn parse_memory_bytes(line: &str) -> u64 {
    line.split(':')
        .nth(1)
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Get the process name from /proc/<pid>/comm
fn get_process_name(pid: u32) -> String {
    fs::read_to_string(format!("/proc/{}/comm", pid))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| format!("pid:{}", pid))
}

/// Check if fd points to a DRM render node
fn is_drm_render_fd(pid: u32, fd: &str) -> bool {
    let link_path = format!("/proc/{}/fd/{}", pid, fd);
    if let Ok(target) = fs::read_link(&link_path) {
        let target_str = target.to_string_lossy();
        target_str.contains("/dev/dri/renderD") || target_str.contains("/dev/dri/card")
    } else {
        false
    }
}

/// Internal fdinfo data
#[derive(Default)]
struct FdinfoData {
    client_id: Option<u64>,
    render_ns: u64,
    copy_ns: u64,
    video_ns: u64,
    video_enhance_ns: u64,
    compute_ns: u64,
    memory_bytes: u64,
}

/// List all DRM clients (processes using the GPU)
///
/// This reads /proc to find all processes with open DRM render node
/// file descriptors and parses their fdinfo to get GPU usage.
pub fn list_drm_clients() -> Vec<DrmClient> {
    let mut clients: HashMap<u32, DrmClient> = HashMap::new();

    let proc_path = Path::new("/proc");
    let entries = match fs::read_dir(proc_path) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Only process numeric directories (PIDs)
        let pid: u32 = match name_str.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };

        // Read the fd directory for this process
        let fd_path = format!("/proc/{}/fd", pid);
        let fd_entries = match fs::read_dir(&fd_path) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for fd_entry in fd_entries.flatten() {
            let fd = fd_entry.file_name();
            let fd_str = fd.to_string_lossy();

            // Check if this fd is a DRM render node
            if !is_drm_render_fd(pid, &fd_str) {
                continue;
            }

            // Parse the fdinfo
            if let Some(data) = parse_fdinfo(pid, &fd_str) {
                let client = clients.entry(pid).or_insert_with(|| {
                    let name = get_process_name(pid);
                    DrmClient::new(pid, name)
                });

                // Accumulate usage (a process may have multiple DRM fds)
                client.render_ns = client.render_ns.saturating_add(data.render_ns);
                client.copy_ns = client.copy_ns.saturating_add(data.copy_ns);
                client.video_ns = client.video_ns.saturating_add(data.video_ns);
                client.video_enhance_ns = client
                    .video_enhance_ns
                    .saturating_add(data.video_enhance_ns);
                client.compute_ns = client.compute_ns.saturating_add(data.compute_ns);
                client.memory_bytes = client.memory_bytes.max(data.memory_bytes);
            }
        }
    }

    // Convert to vec and sort by total usage (descending)
    let mut result: Vec<_> = clients.into_values().collect();
    result.sort_by_key(|c| std::cmp::Reverse(c.total_usage_ns()));
    result
}

/// Find DRM clients using Quick Sync (video encode/decode)
pub fn find_quicksync_clients() -> Vec<DrmClient> {
    list_drm_clients()
        .into_iter()
        .filter(|c| c.is_using_quicksync())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_engine_ns() {
        assert_eq!(
            parse_engine_ns("drm-engine-render:      12345678 ns"),
            12345678
        );
        assert_eq!(parse_engine_ns("drm-engine-copy:        0 ns"), 0);
        assert_eq!(parse_engine_ns("drm-engine-video:       999 ns"), 999);
    }

    #[test]
    fn test_parse_memory_bytes() {
        assert_eq!(
            parse_memory_bytes("drm-memory-resident:    1234567"),
            1234567
        );
        assert_eq!(parse_memory_bytes("drm-memory-resident:    0"), 0);
    }

    #[test]
    fn test_drm_client() {
        let mut client = DrmClient::new(1234, "test".to_string());
        client.video_ns = 1000;
        assert!(client.is_using_quicksync());
        assert_eq!(client.total_usage_ns(), 1000);

        client.render_ns = 500;
        assert_eq!(client.total_usage_ns(), 1500);
    }
}
