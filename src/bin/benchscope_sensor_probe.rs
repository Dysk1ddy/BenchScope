#[path = "../sensor_driver_client.rs"]
mod sensor_driver_client;

use sensor_driver_client::{
    BENCHSCOPE_SENSOR_ADVANCED_HAS_ENERGY, BENCHSCOPE_SENSOR_ADVANCED_HAS_TEMPERATURE,
    BENCHSCOPE_SENSOR_ADVANCED_HAS_THERMAL_LIMIT, BENCHSCOPE_SENSOR_ADVANCED_THERMAL_THROTTLED,
    BENCHSCOPE_SENSOR_ADVANCED_USER_MODE_PROVIDER, BENCHSCOPE_SENSOR_READING_HAS_TEMPERATURE,
    BENCHSCOPE_SENSOR_READING_HAS_UTILIZATION, DeviceHandle, kind_from_driver, status_from_driver,
    status_json_value, wide_to_string,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn run() -> Result<(), String> {
    Err("benchscope_sensor_probe is only supported on Windows".to_owned())
}

#[cfg(windows)]
fn run() -> Result<(), String> {
    let device = DeviceHandle::open_default().map_err(|error| {
        format!(
            "{error}. Is the BenchScopeSensorDriver service running, and is this process elevated?"
        )
    })?;

    let version = device.version()?;
    println!(
        "Driver version: protocol {} driver {}.{}.{}",
        version.protocol_version, version.driver_major, version.driver_minor, version.driver_patch
    );

    let capabilities = device.capabilities()?;
    println!(
        "Capabilities: cpu_temp={} gpu_temp={} drive_temp={} utilization={}",
        yes_no(capabilities.supports_cpu_temperature),
        yes_no(capabilities.supports_gpu_temperature),
        yes_no(capabilities.supports_drive_temperature),
        yes_no(capabilities.supports_utilization)
    );

    let snapshot = device.snapshot()?;
    println!(
        "Snapshot: protocol {} sequence {} readings {}",
        snapshot.protocol_version, snapshot.sequence, snapshot.reading_count
    );

    for reading in snapshot
        .readings
        .iter()
        .take(snapshot.reading_count.min(snapshot.readings.len() as u32) as usize)
    {
        println!(
            "  {}: status={} flags=0x{:08x} temp={} util={} provider={}",
            wide_to_string(&reading.label),
            status_json_value(status_from_driver(reading.status)),
            reading.flags,
            value_or_na(
                reading.flags & BENCHSCOPE_SENSOR_READING_HAS_TEMPERATURE,
                reading.temperature_milli_c,
                "C"
            ),
            value_or_na(
                reading.flags & BENCHSCOPE_SENSOR_READING_HAS_UTILIZATION,
                reading.utilization_milli_percent,
                "%"
            ),
            wide_to_string(&reading.provider)
        );
        let _ = kind_from_driver(reading.kind);
    }

    match device.advanced_telemetry() {
        Ok(advanced) => {
            println!(
                "Advanced telemetry: provider_mask=0x{:08x} sequence {} readings {}",
                advanced.provider_mask, advanced.sequence, advanced.reading_count
            );
            for reading in advanced
                .readings
                .iter()
                .take(advanced.reading_count.min(advanced.readings.len() as u32) as usize)
            {
                println!(
                    "  {}: status={} flags=0x{:08x} temp={} limit={} energy={} throttled={} user_mode={} provider={} detail={}",
                    wide_to_string(&reading.label),
                    status_json_value(status_from_driver(reading.status)),
                    reading.flags,
                    value_or_na(
                        reading.flags & BENCHSCOPE_SENSOR_ADVANCED_HAS_TEMPERATURE,
                        reading.temperature_milli_c,
                        "C"
                    ),
                    value_or_na(
                        reading.flags & BENCHSCOPE_SENSOR_ADVANCED_HAS_THERMAL_LIMIT,
                        reading.thermal_limit_milli_c,
                        "C"
                    ),
                    energy_or_na(
                        reading.flags & BENCHSCOPE_SENSOR_ADVANCED_HAS_ENERGY,
                        reading.energy_milli_joules
                    ),
                    yes_no(reading.flags & BENCHSCOPE_SENSOR_ADVANCED_THERMAL_THROTTLED),
                    yes_no(reading.flags & BENCHSCOPE_SENSOR_ADVANCED_USER_MODE_PROVIDER),
                    wide_to_string(&reading.provider),
                    wide_to_string(&reading.detail)
                );
                let _ = kind_from_driver(reading.kind);
            }
        }
        Err(error) => {
            println!("Advanced telemetry: unavailable ({error})");
        }
    }

    Ok(())
}

fn yes_no(value: u32) -> &'static str {
    if value == 0 { "no" } else { "yes" }
}

fn value_or_na(has_value: u32, milli_value: i32, unit: &str) -> String {
    if has_value == 0 {
        "N/A".to_owned()
    } else {
        format!("{:.1}{unit}", milli_value as f32 / 1000.0)
    }
}

fn energy_or_na(has_value: u32, milli_joules: u64) -> String {
    if has_value == 0 {
        "N/A".to_owned()
    } else {
        format!("{:.3}J", milli_joules as f64 / 1000.0)
    }
}
