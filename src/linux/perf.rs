//! Linux perf_event_open syscall wrapper
//!
//! Provides safe wrappers around the perf_event_open syscall for reading
//! i915 PMU counters.

use std::fs::File;
use std::io::{self, Read};
use std::mem;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};

use crate::error::{Error, Result};

/// perf_event_attr structure for perf_event_open syscall
///
/// This is a simplified version containing only the fields we need.
/// The full structure is much larger but we only use a subset.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct PerfEventAttr {
    /// Major type: hardware/software/tracepoint/etc
    pub type_: u32,
    /// Size of the attr structure
    pub size: u32,
    /// Type-specific configuration
    pub config: u64,
    /// Sample period or frequency
    pub sample_period_or_freq: u64,
    /// Sampling type
    pub sample_type: u64,
    /// Reading format
    pub read_format: u64,
    /// Flags (disabled, inherit, etc)
    pub flags: u64,
    /// Wakeup events/watermark
    pub wakeup_events_or_watermark: u32,
    /// Breakpoint type
    pub bp_type: u32,
    /// Config1 (extension)
    pub config1: u64,
    /// Config2 (extension)
    pub config2: u64,
    /// Branch sample type
    pub branch_sample_type: u64,
    /// Sample regs user
    pub sample_regs_user: u64,
    /// Sample stack user
    pub sample_stack_user: u32,
    /// Clock ID
    pub clockid: i32,
    /// Sample regs intr
    pub sample_regs_intr: u64,
    /// Aux watermark
    pub aux_watermark: u32,
    /// Sample max stack
    pub sample_max_stack: u16,
    /// Reserved
    pub __reserved_2: u16,
    /// Aux sample size
    pub aux_sample_size: u32,
    /// Reserved
    pub __reserved_3: u32,
    /// Sig data
    pub sig_data: u64,
    /// Config3
    pub config3: u64,
}

impl Default for PerfEventAttr {
    fn default() -> Self {
        Self {
            type_: 0,
            size: mem::size_of::<Self>() as u32,
            config: 0,
            sample_period_or_freq: 0,
            sample_type: 0,
            read_format: 0,
            flags: 0,
            wakeup_events_or_watermark: 0,
            bp_type: 0,
            config1: 0,
            config2: 0,
            branch_sample_type: 0,
            sample_regs_user: 0,
            sample_stack_user: 0,
            clockid: 0,
            sample_regs_intr: 0,
            aux_watermark: 0,
            sample_max_stack: 0,
            __reserved_2: 0,
            aux_sample_size: 0,
            __reserved_3: 0,
            sig_data: 0,
            config3: 0,
        }
    }
}

impl PerfEventAttr {
    /// Create a new PerfEventAttr for an i915 PMU event
    pub fn new_i915(pmu_type: u32, config: u64) -> Self {
        Self {
            type_: pmu_type,
            config,
            ..Self::default()
        }
    }
}

/// Flag bits for perf_event_open
pub mod flags {
    /// On by default
    pub const DISABLED: u64 = 1 << 0;
    /// Children inherit it
    pub const INHERIT: u64 = 1 << 1;
    /// Must always be on PMU
    pub const PINNED: u64 = 1 << 2;
    /// Only group on PMU
    pub const EXCLUSIVE: u64 = 1 << 3;
    /// Don't count user
    pub const EXCLUDE_USER: u64 = 1 << 4;
    /// Don't count kernel
    pub const EXCLUDE_KERNEL: u64 = 1 << 5;
    /// Don't count hypervisor
    pub const EXCLUDE_HV: u64 = 1 << 6;
    /// Don't count when idle
    pub const EXCLUDE_IDLE: u64 = 1 << 7;
}

/// PERF_FLAG_* constants for perf_event_open
pub mod perf_flags {
    /// Close on exec
    pub const FD_CLOEXEC: u32 = 1;
    /// Event fd output
    pub const FD_NO_GROUP: u32 = 2;
    /// O_NONBLOCK
    pub const FD_OUTPUT: u32 = 4;
    /// PID/CGroup filtering
    pub const PID_CGROUP: u32 = 8;
}

/// Wrapper for a perf event file descriptor
#[derive(Debug)]
pub struct PerfEvent {
    file: File,
    event_name: String,
}

impl PerfEvent {
    /// Open a new perf event
    ///
    /// # Arguments
    ///
    /// * `attr` - Event attributes
    /// * `pid` - Process ID (-1 for all processes)
    /// * `cpu` - CPU number (-1 for all CPUs)
    /// * `group_fd` - Group leader FD (-1 for new group)
    /// * `flags` - perf_event_open flags
    /// * `event_name` - Human-readable event name for error messages
    pub fn open(
        attr: &PerfEventAttr,
        pid: i32,
        cpu: i32,
        group_fd: i32,
        flags: u32,
        event_name: impl Into<String>,
    ) -> Result<Self> {
        let event_name = event_name.into();

        let fd = unsafe {
            perf_event_open(
                attr as *const PerfEventAttr,
                pid,
                cpu,
                group_fd,
                flags as libc::c_ulong,
            )
        };

        if fd < 0 {
            let err = io::Error::last_os_error();
            return match err.raw_os_error() {
                Some(libc::EACCES) | Some(libc::EPERM) => {
                    Err(Error::PermissionDenied {
                        message: format!(
                            "Cannot open perf event '{}'. Try: run as root, add user to 'render' group, or grant CAP_PERFMON",
                            event_name
                        ),
                    })
                }
                Some(libc::ENOENT) => Err(Error::EventNotSupported { event: event_name }),
                _ => Err(Error::PerfEventOpen {
                    event: event_name,
                    source: err,
                }),
            };
        }

        let file = unsafe { File::from_raw_fd(fd) };

        Ok(Self { file, event_name })
    }

    /// Read the current counter value
    ///
    /// Returns the cumulative counter value as a u64.
    pub fn read_value(&mut self) -> Result<u64> {
        let mut buf = [0u8; 8];
        self.file
            .read_exact(&mut buf)
            .map_err(|e| Error::PerfEventOpen {
                event: self.event_name.clone(),
                source: e,
            })?;
        Ok(u64::from_ne_bytes(buf))
    }

    /// Get the raw file descriptor
    pub fn as_raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    /// Get the event name
    pub fn event_name(&self) -> &str {
        &self.event_name
    }

    /// Enable the event counter
    pub fn enable(&self) -> Result<()> {
        let ret = unsafe { libc::ioctl(self.file.as_raw_fd(), PERF_EVENT_IOC_ENABLE, 0) };
        if ret < 0 {
            return Err(Error::PerfEventRead(io::Error::last_os_error()));
        }
        Ok(())
    }

    /// Disable the event counter
    pub fn disable(&self) -> Result<()> {
        let ret = unsafe { libc::ioctl(self.file.as_raw_fd(), PERF_EVENT_IOC_DISABLE, 0) };
        if ret < 0 {
            return Err(Error::PerfEventRead(io::Error::last_os_error()));
        }
        Ok(())
    }

    /// Reset the event counter
    pub fn reset(&self) -> Result<()> {
        let ret = unsafe { libc::ioctl(self.file.as_raw_fd(), PERF_EVENT_IOC_RESET, 0) };
        if ret < 0 {
            return Err(Error::PerfEventRead(io::Error::last_os_error()));
        }
        Ok(())
    }
}

/// ioctl commands for perf events
const PERF_EVENT_IOC_ENABLE: libc::c_ulong = 0x2400;
const PERF_EVENT_IOC_DISABLE: libc::c_ulong = 0x2401;
const PERF_EVENT_IOC_RESET: libc::c_ulong = 0x2403;

/// Wrapper for the perf_event_open syscall
///
/// # Safety
///
/// This function makes a raw syscall. The caller must ensure:
/// - `attr` points to a valid PerfEventAttr structure
/// - The other parameters are valid for the syscall
unsafe fn perf_event_open(
    attr: *const PerfEventAttr,
    pid: libc::pid_t,
    cpu: libc::c_int,
    group_fd: libc::c_int,
    flags: libc::c_ulong,
) -> libc::c_int {
    libc::syscall(libc::SYS_perf_event_open, attr, pid, cpu, group_fd, flags) as libc::c_int
}

/// Helper to open an i915 PMU event with default settings
pub fn open_i915_event(
    pmu_type: u32,
    config: u64,
    event_name: impl Into<String>,
) -> Result<PerfEvent> {
    let attr = PerfEventAttr::new_i915(pmu_type, config);
    // pid=-1, cpu=0, group_fd=-1, flags=0
    // We use cpu=0 as i915 PMU events are system-wide
    PerfEvent::open(&attr, -1, 0, -1, 0, event_name)
}

/// A group of related perf events that can be read together
#[derive(Debug)]
pub struct PerfEventGroup {
    /// The leader event
    leader: PerfEvent,
    /// Member events (opened with leader as group_fd)
    members: Vec<PerfEvent>,
}

impl PerfEventGroup {
    /// Create a new event group with the given leader event
    pub fn new(leader: PerfEvent) -> Self {
        Self {
            leader,
            members: Vec::new(),
        }
    }

    /// Add a member event to the group
    pub fn add_member(
        &mut self,
        pmu_type: u32,
        config: u64,
        event_name: impl Into<String>,
    ) -> Result<()> {
        let attr = PerfEventAttr::new_i915(pmu_type, config);
        let event = PerfEvent::open(&attr, -1, 0, self.leader.as_raw_fd(), 0, event_name)?;
        self.members.push(event);
        Ok(())
    }

    /// Read all values from the group
    pub fn read_all(&mut self) -> Result<Vec<u64>> {
        let mut values = vec![self.leader.read_value()?];
        for member in &mut self.members {
            values.push(member.read_value()?);
        }
        Ok(values)
    }

    /// Enable all events in the group
    pub fn enable_all(&self) -> Result<()> {
        self.leader.enable()?;
        for member in &self.members {
            member.enable()?;
        }
        Ok(())
    }

    /// Disable all events in the group
    pub fn disable_all(&self) -> Result<()> {
        self.leader.disable()?;
        for member in &self.members {
            member.disable()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_perf_event_attr_size() {
        // The structure should be properly sized for the syscall
        let attr = PerfEventAttr::default();
        assert!(attr.size > 0);
        assert_eq!(attr.size as usize, mem::size_of::<PerfEventAttr>());
    }

    #[test]
    fn test_new_i915_attr() {
        let attr = PerfEventAttr::new_i915(10, 0x30000);
        assert_eq!(attr.type_, 10);
        assert_eq!(attr.config, 0x30000);
    }
}
