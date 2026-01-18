# Instruktion: Bygg intel-gpu-stats Rust-bibliotek

## Projektbeskrivning

Skapa ett cross-platform Rust-bibliotek för att läsa Intel GPU-statistik i realtid. Biblioteket ska kunna användas för att monitorera GPU-användning i broadcast/media-applikationer, specifikt för att visa Quick Sync encoder/decoder-belastning.

## Målplattformar

- **Linux** (primärt): Via i915 PMU och perf_event_open syscall
- **Windows** (sekundärt): Via D3DKMT API

## Metrics att samla in

### Engine Utilization (procent busy över tid)

- **Render/3D** - OpenGL/Vulkan rendering
- **Video** - Quick Sync decoder
- **VideoEnhance** - Quick Sync encoder och video processing
- **Blitter** - Copy operations
- **Compute** (om tillgängligt på nyare Intel Arc)

### Frequency

- Aktuell GPU-frekvens (MHz)
- Begärd frekvens
- Min/Max frekvens

### Power (via RAPL om tillgängligt)

- GPU power draw (Watt)
- Package power

### RC6 Residency

- Tid i power-saving state (procent)

## Teknisk implementation - Linux

### Kärnkoncept

i915-drivern exponerar PMU events via Linux perf subsystem. `intel_gpu_top` använder detta och är referensimplementation.

### Steg 1: Hitta Intel GPU

```rust
// Sök i /sys/class/drm/ efter i915-enheter
// Eller parsa /sys/devices/pci*/*/drm/card*/device/vendor
// Intel vendor ID: 0x8086
```

### Steg 2: Hitta PMU

```rust
// PMU finns under /sys/bus/event_source/devices/i915/
// Läs /sys/bus/event_source/devices/i915/type för PMU type ID
// Events definieras i /sys/bus/event_source/devices/i915/events/
```

### Steg 3: Öppna perf events

Använd `perf_event_open` syscall (syscall nummer 298 på x86_64):

```rust
use std::os::raw::{c_int, c_long, c_ulong};

#[repr(C)]
struct perf_event_attr {
    type_: u32,
    size: u32,
    config: u64,
    // ... fler fält, se linux/perf_event.h
}

// Syscall wrapper
unsafe fn perf_event_open(
    attr: *const perf_event_attr,
    pid: i32,
    cpu: i32,
    group_fd: i32,
    flags: u32,
) -> i32 {
    libc::syscall(libc::SYS_perf_event_open, attr, pid, cpu, group_fd, flags) as i32
}
```

### Steg 4: Event config format

i915 PMU config format (från i915_pmu.c):

```
config = (engine_class << 16) | (engine_instance << 8) | sample_type

Engine classes:
  0 = RENDER
  1 = COPY (Blitter)  
  2 = VIDEO
  3 = VIDEO_ENHANCE
  4 = COMPUTE

Sample types:
  0 = I915_SAMPLE_BUSY
  1 = I915_SAMPLE_WAIT
  2 = I915_SAMPLE_SEMA
```

Exempel configs:

- `0x00000000` = Render busy
- `0x00020000` = Video busy (class 2)
- `0x00030000` = VideoEnhance busy (class 3)

Speciella events (utan engine):

- `frequency-requested` = config från events-filen
- `frequency-actual` = config från events-filen
- `rc6-residency` = config från events-filen

### Steg 5: Läsa events

```rust
// Läs 8 bytes (u64) från file descriptor
// Värdet är kumulativt - ta diff mellan två läsningar
// Dividera med tidsdiff för att få rate
```

### Referenskod att studera

1. **intel_gpu_top source**: https://gitlab.freedesktop.org/drm/igt-gpu-tools/-/blob/master/tools/intel_gpu_top.c
1. **i915_pmu.c kernel driver**: https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/i915/i915_pmu.c
1. **igpu-monitor (enkel C-implementation)**: https://github.com/Karim-Alii/igpu-monitor

## Teknisk implementation - Windows

Använd D3DKMT API via `gdi32.dll`:

```rust
// D3DKMTQueryStatistics för GPU stats
// D3DKMTQueryAdapterInfo för adapter info
// Kräver windows-rs crate
```

## API Design

```rust
pub struct IntelGpu {
    // Intern state
}

impl IntelGpu {
    /// Detektera och öppna första Intel GPU
    pub fn detect() -> Result<Self>;
    
    /// Öppna specifik GPU via sysfs path eller DRM node
    pub fn open(path: &str) -> Result<Self>;
    
    /// Lista alla tillgängliga Intel GPUs
    pub fn list_gpus() -> Result<Vec<GpuInfo>>;
    
    /// Läs aktuell statistik (snapshot)
    pub fn read_stats(&self) -> Result<GpuStats>;
    
    /// Starta kontinuerlig sampling med callback
    pub fn start_sampling<F>(&self, interval: Duration, callback: F) -> Result<SamplingHandle>
    where F: FnMut(GpuStats) + Send + 'static;
}

pub struct GpuStats {
    pub timestamp: Instant,
    pub engines: EngineStats,
    pub frequency: FrequencyStats,
    pub power: Option<PowerStats>,
    pub rc6: Option<Rc6Stats>,
}

pub struct EngineStats {
    pub render: EngineUtilization,
    pub video: EngineUtilization,
    pub video_enhance: EngineUtilization,
    pub blitter: EngineUtilization,
    pub compute: Option<EngineUtilization>,
}

pub struct EngineUtilization {
    pub busy_percent: f64,
    pub wait_percent: f64,
    pub sema_percent: f64,
}

pub struct FrequencyStats {
    pub actual_mhz: u32,
    pub requested_mhz: u32,
}

pub struct PowerStats {
    pub gpu_watts: f64,
    pub package_watts: Option<f64>,
}

pub struct Rc6Stats {
    pub residency_percent: f64,
}
```

## Cargo.toml

```toml
[package]
name = "intel-gpu-stats"
version = "0.1.0"
edition = "2021"
license = "Apache-2.0"
description = "Cross-platform Intel GPU statistics monitoring"
repository = "https://github.com/AUR/intel-gpu-stats"
keywords = ["intel", "gpu", "monitoring", "quicksync", "vaapi"]
categories = ["hardware-support", "os"]

[dependencies]
libc = "0.2"
thiserror = "1.0"

[target.'cfg(windows)'.dependencies]
windows = { version = "0.58", features = ["Win32_Graphics_Direct3D"] }

[dev-dependencies]
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time"] }
```

## Testning

1. Bygg och kör på en maskin med Intel GPU
1. Verifiera mot `intel_gpu_top -J` output
1. Testa med och utan root/CAP_PERFMON
1. Testa i Docker container med `/dev/dri` mounted

## Felhantering

- Returnera tydliga fel om ingen Intel GPU finns
- Hantera permission denied (föreslå CAP_PERFMON eller render group)
- Hantera äldre kernels som saknar vissa PMU events gracefully
- Logga varningar för events som inte stöds men fortsätt med de som fungerar

## Bonus features (om tid finns)

1. **Async support** med tokio
1. **Process-level stats** - vilken process använder GPU:n
1. **Memory bandwidth** via Uncore IMC
1. **Temperature** via hwmon sysfs
1. **JSON output** för enkel integration

## Användarfall

Detta bibliotek ska användas i Ström (https://github.com/Eyevinn/strom), en GStreamer-baserad broadcast-applikation, för att visa GPU-belastning i realtid när Quick Sync används för video encoding/decoding.

-----

Börja med Linux-implementation då det är primärt mål. Windows kan komma senare.
Fokusera på att få engine utilization att fungera först, sedan frekvens och power.