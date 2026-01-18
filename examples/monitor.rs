//! Example: Real-time GPU monitoring
//!
//! This example shows how to continuously monitor Intel GPU statistics
//! and display them in a terminal-friendly format.
//!
//! Run with: cargo run --example monitor
//!
//! Note: Requires appropriate permissions (root, render group, or CAP_PERFMON)

use intel_gpu_stats::{IntelGpu, Result};
use std::io::{self, Write};
use std::thread;
use std::time::Duration;

fn main() -> Result<()> {
    println!("Intel GPU Statistics Monitor");
    println!("============================");
    println!();

    // Detect and open the GPU
    let mut gpu = match IntelGpu::detect() {
        Ok(gpu) => gpu,
        Err(e) => {
            eprintln!("Error: {}", e);
            if e.is_permission_error() {
                eprintln!();
                eprintln!("To fix permission issues, try one of:");
                eprintln!("  1. Run as root: sudo cargo run --example monitor");
                eprintln!("  2. Add user to render group: sudo usermod -aG render $USER");
                eprintln!("  3. Grant CAP_PERFMON capability");
            }
            return Err(e);
        }
    };

    // Show GPU info
    let info = gpu.gpu_info();
    println!(
        "GPU: {} ({})",
        info.id,
        info.device_name.as_deref().unwrap_or("Unknown")
    );
    println!("Device ID: 0x{:04x}", info.device_id);
    println!("Driver: {}", gpu.driver());
    if let Some(ref node) = info.render_node {
        println!("Render node: {}", node);
    }
    if gpu.has_temperature() {
        println!("Temperature: available");
    }
    println!();

    // Initial read to establish baseline
    let _ = gpu.read_stats()?;
    thread::sleep(Duration::from_millis(100));

    println!("Press Ctrl+C to exit");
    println!();

    // Continuous monitoring loop
    loop {
        let stats = gpu.read_stats()?;

        // Clear line and move cursor to beginning
        print!("\x1B[2K\r");

        // Format and print statistics
        print!(
            "Render: {:5.1}% | Video: {:5.1}% | VidEnhance: {:5.1}% | Blitter: {:5.1}%",
            stats.engines.render.busy_percent,
            stats.engines.video.busy_percent,
            stats.engines.video_enhance.busy_percent,
            stats.engines.blitter.busy_percent,
        );

        if let Some(ref compute) = stats.engines.compute {
            print!(" | Compute: {:5.1}%", compute.busy_percent);
        }

        if stats.frequency.actual_mhz > 0 {
            print!(" | Freq: {} MHz", stats.frequency.actual_mhz);
        }

        if let Some(ref rc6) = stats.rc6 {
            print!(" | RC6: {:5.1}%", rc6.residency_percent);
        }

        if let Some(ref temp) = stats.temperature {
            print!(" | Temp: {:.0}C", temp.gpu_celsius);
        }

        io::stdout().flush().unwrap();

        thread::sleep(Duration::from_millis(500));
    }
}
