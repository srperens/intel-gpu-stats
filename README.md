# intel-gpu-stats

A Rust library for reading Intel GPU statistics in real-time via the i915 PMU interface.

Designed for monitoring GPU usage in broadcast/media applications, specifically for showing Quick Sync encoder/decoder load.

## Features

- **Engine utilization**: Render/3D, Video (decoder), VideoEnhance (encoder), Blitter, Compute (Arc)
- **GPU frequency**: Actual and requested MHz
- **RC6 residency**: Power-saving state percentage
- **Continuous sampling**: Callback-based monitoring
- **GPU enumeration**: Detect and list all Intel GPUs

## Requirements

### System
- Linux with Intel GPU (integrated or discrete)
- i915 kernel driver loaded
- Kernel 4.16+ (for PMU support)

### Permissions

Reading GPU statistics requires one of:
```bash
# Option 1: Add user to render group (recommended)
sudo usermod -aG render $USER
# Log out and back in

# Option 2: Grant CAP_PERFMON capability to binary
sudo setcap cap_perfmon+ep ./target/release/your_app

# Option 3: Run as root (not recommended for production)
sudo ./target/release/your_app
```

## Installation

Add to your `Cargo.toml`:
```toml
[dependencies]
intel-gpu-stats = "0.1"
```

Or clone and build:
```bash
git clone https://github.com/AUR/intel-gpu-stats
cd intel-gpu-stats
cargo build --release
```

## Quick Start

```rust
use intel_gpu_stats::IntelGpu;
use std::time::Duration;
use std::thread;

fn main() -> intel_gpu_stats::Result<()> {
    // Detect and open the first Intel GPU
    let mut gpu = IntelGpu::detect()?;

    // Initial read to establish baseline
    let _ = gpu.read_stats()?;
    thread::sleep(Duration::from_millis(100));

    // Read statistics
    let stats = gpu.read_stats()?;

    println!("Render:       {:.1}%", stats.engines.render.busy_percent);
    println!("Video:        {:.1}%", stats.engines.video.busy_percent);
    println!("VideoEnhance: {:.1}%", stats.engines.video_enhance.busy_percent);
    println!("Frequency:    {} MHz", stats.frequency.actual_mhz);

    if let Some(rc6) = &stats.rc6 {
        println!("RC6:          {:.1}%", rc6.residency_percent);
    }

    Ok(())
}
```

## Continuous Monitoring

```rust
use intel_gpu_stats::IntelGpu;
use std::time::Duration;

fn main() -> intel_gpu_stats::Result<()> {
    let gpu = IntelGpu::detect()?;

    // Start sampling every 100ms
    let handle = gpu.start_sampling(Duration::from_millis(100), |stats| {
        println!("Quick Sync: {:.1}%", stats.engines.quicksync_utilization());
    })?;

    // Do other work...
    std::thread::sleep(Duration::from_secs(10));

    // Stop sampling
    handle.stop();
    Ok(())
}
```

## Examples

```bash
# Real-time terminal monitor
cargo run --example monitor

# List all Intel GPUs
cargo run --example list_gpus

# JSON output for integration
cargo run --example json_output
```

## Comparison with intel_gpu_top

This library uses the same i915 PMU interface as `intel_gpu_top`. You can verify readings:

```bash
# Install intel-gpu-tools
sudo apt install intel-gpu-tools  # Debian/Ubuntu
sudo pacman -S intel-gpu-tools    # Arch

# Compare outputs
intel_gpu_top -J  # JSON output
cargo run --example json_output
```

## Platform Support

| Platform | Status | Backend |
|----------|--------|---------|
| Linux    | ✅ Supported | i915 PMU via perf_event_open |
| Windows  | 🚧 Planned | D3DKMT API |

## License

Apache-2.0

## Related Projects

- [intel_gpu_top](https://gitlab.freedesktop.org/drm/igt-gpu-tools) - Reference implementation
- [Ström](https://github.com/Eyevinn/strom) - GStreamer broadcast application using this library
