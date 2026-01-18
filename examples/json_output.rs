//! Example: JSON output for integration
//!
//! This example demonstrates how to output GPU statistics in JSON format
//! for easy integration with other tools and dashboards.
//!
//! Run with: cargo run --example json_output
//!
//! Note: This is a simple example without the serde dependency.
//! For production use, consider adding serde with the "derive" feature.

use intel_gpu_stats::{GpuStats, IntelGpu, Result};
use std::thread;
use std::time::Duration;

/// Format GPU stats as JSON string
fn stats_to_json(stats: &GpuStats) -> String {
    let mut json = String::from("{\n");

    // Timestamp (as nanoseconds since some epoch)
    json.push_str(&format!(
        "  \"sample_duration_ns\": {},\n",
        stats.sample_duration_ns
    ));

    // Engines
    json.push_str("  \"engines\": {\n");
    json.push_str(&format!(
        "    \"render\": {{ \"busy\": {:.2}, \"wait\": {:.2}, \"sema\": {:.2} }},\n",
        stats.engines.render.busy_percent,
        stats.engines.render.wait_percent,
        stats.engines.render.sema_percent
    ));
    json.push_str(&format!(
        "    \"video\": {{ \"busy\": {:.2}, \"wait\": {:.2}, \"sema\": {:.2} }},\n",
        stats.engines.video.busy_percent,
        stats.engines.video.wait_percent,
        stats.engines.video.sema_percent
    ));
    json.push_str(&format!(
        "    \"video_enhance\": {{ \"busy\": {:.2}, \"wait\": {:.2}, \"sema\": {:.2} }},\n",
        stats.engines.video_enhance.busy_percent,
        stats.engines.video_enhance.wait_percent,
        stats.engines.video_enhance.sema_percent
    ));
    json.push_str(&format!(
        "    \"blitter\": {{ \"busy\": {:.2}, \"wait\": {:.2}, \"sema\": {:.2} }}",
        stats.engines.blitter.busy_percent,
        stats.engines.blitter.wait_percent,
        stats.engines.blitter.sema_percent
    ));

    if let Some(ref compute) = stats.engines.compute {
        json.push_str(",\n");
        json.push_str(&format!(
            "    \"compute\": {{ \"busy\": {:.2}, \"wait\": {:.2}, \"sema\": {:.2} }}\n",
            compute.busy_percent, compute.wait_percent, compute.sema_percent
        ));
    } else {
        json.push('\n');
    }
    json.push_str("  },\n");

    // Frequency
    json.push_str("  \"frequency\": {\n");
    json.push_str(&format!(
        "    \"actual_mhz\": {},\n",
        stats.frequency.actual_mhz
    ));
    json.push_str(&format!(
        "    \"requested_mhz\": {}\n",
        stats.frequency.requested_mhz
    ));
    json.push_str("  }");

    // RC6
    if let Some(ref rc6) = stats.rc6 {
        json.push_str(",\n  \"rc6\": {\n");
        json.push_str(&format!(
            "    \"residency_percent\": {:.2}\n",
            rc6.residency_percent
        ));
        json.push_str("  }");
    }

    // Power (if we had it)
    if let Some(ref power) = stats.power {
        json.push_str(",\n  \"power\": {\n");
        json.push_str(&format!("    \"gpu_watts\": {:.2}", power.gpu_watts));
        if let Some(package) = power.package_watts {
            json.push_str(&format!(",\n    \"package_watts\": {:.2}", package));
        }
        json.push_str("\n  }");
    }

    json.push_str("\n}");
    json
}

fn main() -> Result<()> {
    let mut gpu = IntelGpu::detect()?;

    // Initial read to establish baseline
    let _ = gpu.read_stats()?;
    thread::sleep(Duration::from_millis(100));

    // Output 10 samples
    for i in 0..10 {
        let stats = gpu.read_stats()?;

        if i > 0 {
            println!(","); // Separator between JSON objects
        }
        print!("{}", stats_to_json(&stats));

        thread::sleep(Duration::from_millis(500));
    }

    println!(); // Final newline

    Ok(())
}
