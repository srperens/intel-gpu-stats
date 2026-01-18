//! Example: List available Intel GPUs
//!
//! This example shows how to enumerate all Intel GPUs in the system
//! and display their information.
//!
//! Run with: cargo run --example list_gpus

use intel_gpu_stats::{IntelGpu, Result};

fn main() -> Result<()> {
    println!("Intel GPU Discovery");
    println!("===================");
    println!();

    match IntelGpu::list_gpus() {
        Ok(gpus) => {
            println!("Found {} Intel GPU(s):", gpus.len());
            println!();

            for (i, gpu) in gpus.iter().enumerate() {
                println!("GPU #{}: {}", i, gpu.id);
                println!(
                    "  Vendor ID:   0x{:04x} ({})",
                    gpu.vendor_id,
                    if gpu.is_intel() { "Intel" } else { "Unknown" }
                );
                println!("  Device ID:   0x{:04x}", gpu.device_id);

                if let Some(ref name) = gpu.device_name {
                    println!("  Device Name: {}", name);
                }

                println!("  PCI Path:    {}", gpu.pci_path);

                if let Some(driver) = gpu.driver {
                    println!("  Driver:      {}", driver);
                }

                if let Some(ref node) = gpu.card_node {
                    println!("  Card Node:   {}", node);
                }

                if let Some(ref node) = gpu.render_node {
                    println!("  Render Node: {}", node);
                }

                println!();
            }

            // Try to open each GPU and check capabilities
            println!("Checking GPU capabilities...");
            println!();

            for gpu in &gpus {
                match IntelGpu::open(&gpu.id) {
                    Ok(opened) => {
                        println!("{}: OK", gpu.id);
                        if opened.has_compute_engine() {
                            println!("  - Has Compute engine (Intel Arc)");
                        }
                    }
                    Err(e) => {
                        println!("{}: Error - {}", gpu.id, e);
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Error discovering GPUs: {}", e);

            if e.is_gpu_missing() {
                eprintln!();
                eprintln!("No Intel GPU found. Make sure:");
                eprintln!("  1. You have an Intel GPU (integrated or discrete)");
                eprintln!("  2. The i915 driver is loaded (check with: lsmod | grep i915)");
                eprintln!("  3. The DRM subsystem is available (/sys/class/drm exists)");
            }

            return Err(e);
        }
    }

    Ok(())
}
