fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.2} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

fn format_optional_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "N/A".to_owned())
}

fn format_percent_value(value: Option<f32>) -> String {
    value
        .map(|value| format!("{value:.0}%"))
        .unwrap_or_else(|| "N/A".to_owned())
}

fn format_percent_u64(value: Option<u64>) -> String {
    value
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "N/A".to_owned())
}

fn format_hex_u64(value: Option<u64>) -> String {
    value
        .map(|value| format!("0x{value:02x}"))
        .unwrap_or_else(|| "N/A".to_owned())
}

fn format_optional_u64_minutes(value: Option<u64>) -> String {
    value
        .map(|value| format!("{value} min"))
        .unwrap_or_else(|| "N/A".to_owned())
}

fn format_storage_health_percent(value: Option<f32>) -> String {
    value
        .map(|value| format!("{value:.0}% health"))
        .unwrap_or_else(|| "Health N/A".to_owned())
}

fn format_optional_bytes(bytes: Option<u64>) -> String {
    bytes
        .map(format_bytes)
        .unwrap_or_else(|| "unknown".to_owned())
}

fn format_optional_percent(value: Option<f32>) -> String {
    value
        .map(|value| format!("{value:.0}%"))
        .unwrap_or_else(|| "N/A".to_owned())
}

fn format_optional_energy_mwh(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.0} mWh"))
        .unwrap_or_else(|| "N/A".to_owned())
}

fn format_eta(value: Option<f64>) -> String {
    match value {
        Some(seconds) if seconds <= 0.5 => "ETA: <1s".to_owned(),
        Some(seconds) => format!("ETA: {}", format_elapsed(seconds)),
        None => "ETA: estimating".to_owned(),
    }
}

fn format_elapsed(seconds: f64) -> String {
    if seconds >= 3600.0 {
        let hours = (seconds / 3600.0).floor();
        let minutes = ((seconds % 3600.0) / 60.0).floor();
        format!("{hours:.0}h {minutes:.0}m")
    } else if seconds >= 60.0 {
        let minutes = (seconds / 60.0).floor();
        let secs = seconds % 60.0;
        format!("{minutes:.0}m {secs:.0}s")
    } else if seconds >= 10.0 {
        format!("{seconds:.0}s")
    } else {
        format!("{seconds:.1}s")
    }
}

fn format_ms(value: Option<f64>) -> String {
    match value {
        Some(value) if value >= 1000.0 => format!("{value:.1}"),
        Some(value) => format!("{value:.3}"),
        None => "N/A".to_owned(),
    }
}

fn format_cpu_ms(result: &BenchmarkResult) -> String {
    let value = format_ms(Some(result.cpu_ms));
    if result.cpu_estimated {
        format!("Est. {value}")
    } else {
        value
    }
}

fn format_speedup(value: f64) -> String {
    if value.is_infinite() {
        "inf".to_owned()
    } else {
        format!("{value:.2}x")
    }
}

fn format_optional_rate(value: Option<f64>) -> String {
    match value {
        Some(value) if value >= 1000.0 => format!("{value:.0}"),
        Some(value) if value >= 100.0 => format!("{value:.1}"),
        Some(value) => format!("{value:.2}"),
        None => "N/A".to_owned(),
    }
}

fn format_stress_rate_per_min(iterations: u64, elapsed_s: f64) -> String {
    if elapsed_s <= 0.0 {
        return "N/A".to_owned();
    }
    let rate = iterations as f64 * 60.0 / elapsed_s;
    if rate >= 1.0e12 {
        format!("{:.2}T/min", rate / 1.0e12)
    } else if rate >= 1.0e9 {
        format!("{:.2}B/min", rate / 1.0e9)
    } else if rate >= 1.0e6 {
        format!("{:.2}M/min", rate / 1.0e6)
    } else if rate >= 1.0e3 {
        format!("{:.2}K/min", rate / 1.0e3)
    } else {
        format!("{rate:.0}/min")
    }
}

fn format_stress_iterations_per_second(value: Option<f64>) -> String {
    match value {
        Some(value) if value >= 1.0e12 => format!("{:.2}T/s", value / 1.0e12),
        Some(value) if value >= 1.0e9 => format!("{:.2}B/s", value / 1.0e9),
        Some(value) if value >= 1.0e6 => format!("{:.2}M/s", value / 1.0e6),
        Some(value) if value >= 1.0e3 => format!("{:.2}K/s", value / 1.0e3),
        Some(value) if value >= 100.0 => format!("{value:.1}/s"),
        Some(value) => format!("{value:.2}/s"),
        None => "N/A".to_owned(),
    }
}

fn format_optional_percent_f64(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.1}%"))
        .unwrap_or_else(|| "N/A".to_owned())
}

fn format_drive_speed(result: &DriveBenchmarkResult) -> String {
    if result.test.is_read() {
        format_optional_rate(result.read_mbps)
    } else {
        format_optional_rate(result.write_mbps)
    }
}

fn format_optional_iops(value: Option<f64>) -> String {
    match value {
        Some(value) if value >= 1_000_000.0 => format!("{:.2}M", value / 1_000_000.0),
        Some(value) if value >= 10_000.0 => format!("{:.0}K", value / 1_000.0),
        Some(value) if value >= 1000.0 => format!("{:.1}K", value / 1_000.0),
        Some(value) => format!("{value:.0}"),
        None => "N/A".to_owned(),
    }
}

fn format_optional_latency(value: Option<f64>) -> String {
    match value {
        Some(value) if value < 1.0 => format!("{:.0} us", value * 1000.0),
        Some(value) if value < 100.0 => format!("{value:.2} ms"),
        Some(value) => format!("{value:.1} ms"),
        None => "N/A".to_owned(),
    }
}

fn format_temperature_value(value: Option<f32>) -> String {
    value
        .map(|value| format!("{value:.0} C"))
        .unwrap_or_else(|| "N/A".to_owned())
}

fn format_utilization_value(value: Option<f32>) -> String {
    value
        .map(|value| format!("{value:.0}%"))
        .unwrap_or_else(|| "N/A".to_owned())
}

fn format_temperature_summary(summary: &TemperatureSummary) -> String {
    if !summary.has_any_value() {
        return "N/A".to_owned();
    }
    format!(
        "{} -> {} (max {})",
        format_temperature_value(summary.start_c),
        format_temperature_value(summary.end_c),
        format_temperature_value(summary.max_c)
    )
}

fn format_temperature_run_report(report: &TemperatureRunReport) -> String {
    let mut parts = Vec::new();
    if report.scope == TemperatureScope::Matrix {
        parts.push(format!("CPU {}", format_temperature_summary(&report.cpu)));
        parts.push(format!("GPU {}", format_temperature_summary(&report.gpu)));
    } else {
        parts.push(format!("SSD {}", format_temperature_summary(&report.drive)));
    }
    parts.join(", ")
}
