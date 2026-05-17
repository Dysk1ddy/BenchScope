fn sensor_helper_enabled() -> bool {
    false
}

fn sensor_service_enabled() -> bool {
    true
}

#[cfg(windows)]
fn is_process_elevated() -> bool {
    let script = r#"
$principal = [Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    'true'
} else {
    'false'
}
"#;
    run_powershell_sensor_script(script)
        .map(|output| output.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn is_process_elevated() -> bool {
    true
}

#[cfg(windows)]
fn restart_app_as_admin() -> Result<()> {
    let exe = std::env::current_exe().context("failed to locate BenchScope executable")?;
    let file = powershell_single_quote(&exe.display().to_string());
    let args = std::env::args()
        .skip(1)
        .map(|arg| powershell_single_quote(&arg))
        .collect::<Vec<_>>();
    let script = if args.is_empty() {
        format!("Start-Process -FilePath {file} -Verb RunAs")
    } else {
        format!(
            "Start-Process -FilePath {file} -ArgumentList @({}) -Verb RunAs",
            args.join(", ")
        )
    };
    let command_args = [
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script.as_str(),
    ];
    run_command_no_window_timeout(
        "powershell",
        &command_args,
        Duration::from_millis(ELEVATION_COMMAND_TIMEOUT_MS),
    )
    .map(|_| ())
}

#[cfg(not(windows))]
fn restart_app_as_admin() -> Result<()> {
    Err(anyhow!("administrator restart is only available on Windows"))
}

#[cfg(windows)]
fn powershell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn start_sensor_helper_reader() -> Option<Receiver<SensorSnapshot>> {
    let helper_path = sensor_helper_path()?;
    let mut command = Command::new(&helper_path);
    command
        .arg("--stream")
        .arg("--parent-pid")
        .arg(std::process::id().to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW_RAW);

    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();

    let _ = thread::Builder::new()
        .name("benchscope-sensor-helper".to_owned())
        .spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if let Some(snapshot) = parse_helper_snapshot(&line) {
                    let _ = tx.send(snapshot);
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        });

    Some(rx)
}

fn start_sensor_service_reader() -> Option<Receiver<SensorSnapshot>> {
    let service_path = sensor_service_path()?;
    let mut command = Command::new(&service_path);
    command
        .arg("--stream")
        .arg("--interval-ms")
        .arg(SENSOR_POLL_MS.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW_RAW);

    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();

    let _ = thread::Builder::new()
        .name("benchscope-sensor-service-bridge".to_owned())
        .spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if let Some(snapshot) = parse_helper_snapshot(&line) {
                    let _ = tx.send(snapshot);
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        });

    Some(rx)
}

#[cfg(test)]
fn helper_snapshot_needs_elevation(snapshot: &SensorSnapshot) -> bool {
    snapshot.helper_elevated == Some(false)
        && [
            snapshot.cpu.as_ref(),
            snapshot.gpu.as_ref(),
            snapshot.gpu_memory.as_ref(),
            snapshot.drive.as_ref(),
            snapshot.memory.as_ref(),
        ]
        .into_iter()
        .flatten()
        .any(|reading| {
            reading.temperature_c.is_none()
                && reading
                    .provider
                    .eq_ignore_ascii_case("LibreHardwareMonitor")
        })
}

fn helper_snapshot_has_gaps(snapshot: &SensorSnapshot) -> bool {
    sensor_reading_has_gap(snapshot.cpu.as_ref())
        || sensor_reading_has_gap(snapshot.gpu.as_ref())
        || sensor_reading_has_gap(snapshot.drive.as_ref())
        || sensor_reading_has_gap(snapshot.memory.as_ref())
}

fn sensor_snapshot_needs_fallback(snapshot: &SensorSnapshot, now: Instant) -> bool {
    helper_snapshot_has_gaps(snapshot) || sensor_snapshot_has_stale_data(snapshot, now)
}

fn sensor_snapshot_has_stale_data(snapshot: &SensorSnapshot, now: Instant) -> bool {
    [
        snapshot.cpu.as_ref(),
        snapshot.gpu.as_ref(),
        snapshot.gpu_memory.as_ref(),
        snapshot.drive.as_ref(),
        snapshot.memory.as_ref(),
    ]
    .into_iter()
    .any(|reading| sensor_reading_is_stale(reading, now))
}

fn sensor_reading_has_gap(reading: Option<&SensorReading>) -> bool {
    match reading {
        Some(reading) => !reading.has_temperature() || !reading.has_utilization(),
        None => true,
    }
}

fn sensor_reading_is_stale(reading: Option<&SensorReading>, now: Instant) -> bool {
    reading.is_some_and(|reading| {
        reading.has_any_value()
            && now.saturating_duration_since(reading.updated_at)
                > Duration::from_millis(SENSOR_STALE_AFTER_MS)
    })
}

fn merge_sensor_snapshots(
    primary: Option<SensorSnapshot>,
    fallback: Option<SensorSnapshot>,
) -> Option<SensorSnapshot> {
    match (primary, fallback) {
        (Some(primary), Some(fallback)) => Some(SensorSnapshot {
            cpu: prefer_sensor_reading(primary.cpu, fallback.cpu),
            gpu: prefer_sensor_reading(primary.gpu, fallback.gpu),
            gpu_memory: prefer_sensor_reading(primary.gpu_memory, fallback.gpu_memory),
            drive: prefer_sensor_reading(primary.drive, fallback.drive),
            memory: prefer_sensor_reading(primary.memory, fallback.memory),
            helper_elevated: primary.helper_elevated,
        }),
        (Some(primary), None) => Some(primary),
        (None, Some(fallback)) => Some(fallback),
        (None, None) => None,
    }
}

fn merge_sensor_snapshots_prefer_fresh(
    primary: Option<SensorSnapshot>,
    fallback: Option<SensorSnapshot>,
    now: Instant,
) -> Option<SensorSnapshot> {
    match (primary, fallback) {
        (Some(primary), Some(fallback)) => Some(SensorSnapshot {
            cpu: prefer_sensor_reading_prefer_fresh(primary.cpu, fallback.cpu, now),
            gpu: prefer_sensor_reading_prefer_fresh(primary.gpu, fallback.gpu, now),
            gpu_memory: prefer_sensor_reading_prefer_fresh(
                primary.gpu_memory,
                fallback.gpu_memory,
                now,
            ),
            drive: prefer_sensor_reading_prefer_fresh(primary.drive, fallback.drive, now),
            memory: prefer_sensor_reading_prefer_fresh(primary.memory, fallback.memory, now),
            helper_elevated: primary.helper_elevated,
        }),
        (Some(primary), None) => Some(primary),
        (None, Some(fallback)) => Some(fallback),
        (None, None) => None,
    }
}

fn prefer_sensor_reading(
    primary: Option<SensorReading>,
    fallback: Option<SensorReading>,
) -> Option<SensorReading> {
    match (primary, fallback) {
        (Some(primary), Some(fallback)) => Some(merge_sensor_reading(primary, fallback)),
        (Some(primary), None) => Some(primary),
        (None, fallback) => fallback,
    }
}

fn prefer_sensor_reading_prefer_fresh(
    primary: Option<SensorReading>,
    fallback: Option<SensorReading>,
    now: Instant,
) -> Option<SensorReading> {
    match (primary, fallback) {
        (Some(primary), Some(fallback))
            if sensor_reading_is_stale(Some(&primary), now) && fallback.has_any_value() =>
        {
            Some(fallback)
        }
        (Some(primary), Some(fallback)) => Some(merge_sensor_reading(primary, fallback)),
        (Some(primary), None) => Some(primary),
        (None, fallback) => fallback,
    }
}

fn merge_sensor_reading(mut primary: SensorReading, fallback: SensorReading) -> SensorReading {
    let fallback_status = fallback.status.clone();
    let fallback_provider = fallback.provider.clone();
    let fallback_has_data = fallback.has_any_value();
    let mut filled_temperature = false;
    let mut filled_utilization = false;
    if primary.temperature_c.is_none() && fallback.temperature_c.is_some() {
        primary.temperature_c = fallback.temperature_c;
        if primary.label.is_empty() || !primary.has_utilization() {
            primary.label = fallback.label.clone();
        }
        filled_temperature = true;
    }
    if primary.utilization_percent.is_none() && fallback.utilization_percent.is_some() {
        primary.utilization_percent = fallback.utilization_percent;
        filled_utilization = true;
    }
    for metric in fallback.metrics {
        primary.upsert_metric(metric);
    }
    primary.sync_legacy_metrics();
    if filled_temperature || primary.temperature_c.is_some() {
        primary.status = SensorStatus::Ok;
    } else if filled_utilization && matches!(&fallback_status, SensorStatus::Partial(_)) {
        primary.status = fallback_status;
    } else if !primary.is_ok()
        && primary.has_any_value()
    {
        primary.status = SensorStatus::Ok;
    }
    if primary.provider != fallback_provider && fallback_has_data {
        primary.provider = format!("{} + {}", primary.provider, fallback_provider);
    }
    primary
}

fn apply_integrated_gpu_temperature_fallback(
    mut snapshot: SensorSnapshot,
    enabled: bool,
) -> SensorSnapshot {
    if !enabled {
        return snapshot;
    }

    let Some(cpu) = snapshot.cpu.as_ref() else {
        return snapshot;
    };
    if !cpu_temperature_is_package_like(cpu) {
        return snapshot;
    }
    let Some(cpu_temperature) = cpu.temperature_c else {
        return snapshot;
    };
    if snapshot
        .gpu
        .as_ref()
        .is_some_and(|reading| reading.temperature_c.is_some())
    {
        return snapshot;
    }

    let utilization_percent = snapshot
        .gpu
        .as_ref()
        .and_then(|reading| reading.utilization_percent);
    let mut gpu = SensorReading {
        kind: SensorKind::Gpu,
        label: "iGPU shared CPU package".to_owned(),
        temperature_c: Some(cpu_temperature),
        utilization_percent,
        metrics: Vec::new(),
        provider: format!("Shared CPU package ({})", cpu.provider),
        updated_at: Instant::now(),
        status: SensorStatus::Ok,
    };
    gpu.sync_legacy_metrics();
    snapshot.gpu = Some(gpu);
    snapshot
}

fn cpu_temperature_is_package_like(reading: &SensorReading) -> bool {
    if !reading.has_temperature() {
        return false;
    }

    let provider = reading.provider.to_ascii_lowercase();
    let label = reading.label.to_ascii_lowercase();
    provider.contains("librehardwaremonitor")
        || provider.contains("openhardwaremonitor")
        || provider.contains("hwinfo")
        || label.contains("package")
        || label.contains("core")
        || label.contains("tctl")
        || label.contains("tdie")
}

fn sensor_helper_path() -> Option<PathBuf> {
    find_existing_tool_path(&[
        "BenchScope.SensorHelper.exe",
        "target/release/BenchScope.SensorHelper.exe",
        "target/debug/BenchScope.SensorHelper.exe",
        "sensor-helper/bin/Release/net10.0-windows/BenchScope.SensorHelper.exe",
        "sensor-helper/bin/Debug/net10.0-windows/BenchScope.SensorHelper.exe",
    ])
}

fn sensor_service_path() -> Option<PathBuf> {
    find_existing_tool_path(&[
        "benchscope_sensor_service.exe",
        "benchscope_sensor_service",
        "target/debug/benchscope_sensor_service.exe",
        "target/release/benchscope_sensor_service.exe",
        "target/debug/benchscope_sensor_service",
        "target/release/benchscope_sensor_service",
    ])
}

fn find_existing_tool_path(relative_paths: &[&str]) -> Option<PathBuf> {
    tool_search_roots()
        .into_iter()
        .flat_map(|root| relative_paths.iter().map(move |relative| root.join(relative)))
        .find(|path| path.is_file())
}

fn tool_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(dir) = exe_path.parent() {
            roots.extend(dir.ancestors().map(PathBuf::from));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.extend(cwd.ancestors().map(PathBuf::from));
    }
    roots
}

fn parse_helper_snapshot(line: &str) -> Option<SensorSnapshot> {
    if !line.contains("\"timestampUtc\"") && !line.contains("\"cpu\"") {
        return None;
    }

    Some(SensorSnapshot {
        cpu: parse_helper_reading(line, "cpu", SensorKind::Cpu, "CPU"),
        gpu: parse_helper_reading(line, "gpu", SensorKind::Gpu, "GPU"),
        gpu_memory: parse_helper_reading(line, "gpuMemory", SensorKind::GpuMemory, "VRAM")
            .or_else(|| parse_helper_reading(line, "vram", SensorKind::GpuMemory, "VRAM")),
        drive: parse_helper_reading(line, "drive", SensorKind::Drive, "SSD"),
        memory: parse_helper_reading(line, "memory", SensorKind::Memory, "RAM"),
        helper_elevated: json_bool_for_key(line, "isElevated"),
    })
}

fn parse_helper_reading(
    line: &str,
    key: &str,
    kind: SensorKind,
    fallback_label: &str,
) -> Option<SensorReading> {
    let object = json_object_for_key(line, key)?;
    let label = json_string_for_key(object, "label").unwrap_or_else(|| fallback_label.to_owned());
    let provider = json_string_for_key(object, "provider")
        .unwrap_or_else(|| "LibreHardwareMonitor".to_owned());
    let status_text =
        json_string_for_key(object, "status").unwrap_or_else(|| "unsupported".to_owned());
    let message = json_string_for_key(object, "message");
    let metrics = parse_helper_metrics(object);
    let temperature_c = json_number_for_key(object, "temperatureC")
        .or_else(|| first_metric_value(&metrics, SensorMetricKind::Temperature));
    let utilization_percent = json_number_for_key(object, "utilizationPercent")
        .or_else(|| first_metric_value(&metrics, SensorMetricKind::Utilization))
        .map(clamp_percent);
    let status = helper_sensor_status(&status_text, message);

    let mut reading = SensorReading {
        kind,
        label,
        temperature_c,
        utilization_percent,
        metrics,
        provider,
        updated_at: Instant::now(),
        status,
    };
    reading.sync_legacy_metrics();
    Some(reading)
}

fn first_metric_value(metrics: &[SensorMetric], kind: SensorMetricKind) -> Option<f32> {
    metrics
        .iter()
        .find(|metric| metric.kind == kind)
        .and_then(|metric| metric.value)
}

fn parse_helper_metrics(object: &str) -> Vec<SensorMetric> {
    let Some(array) = json_array_for_key(object, "metrics") else {
        return Vec::new();
    };
    json_objects_in_array(array)
        .into_iter()
        .filter_map(parse_helper_metric)
        .collect()
}

fn parse_helper_metric(object: &str) -> Option<SensorMetric> {
    let kind_text = json_string_for_key(object, "kind")
        .or_else(|| json_string_for_key(object, "category"))?;
    let kind = sensor_metric_kind_from_json(&kind_text)?;
    let label = json_string_for_key(object, "label")
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| kind.default_label().to_owned());
    let value = json_number_for_key(object, "value");
    let min = json_number_for_key(object, "min");
    let max = json_number_for_key(object, "max");
    Some(SensorMetric::new(kind, label, value).with_range(min, max))
}

fn sensor_metric_kind_from_json(value: &str) -> Option<SensorMetricKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "temperature" | "temperatures" | "temp" => Some(SensorMetricKind::Temperature),
        "utilization" | "utilisation" | "load" | "loads" | "usage" => {
            Some(SensorMetricKind::Utilization)
        }
        "memoryusage" | "memory_usage" | "vram" | "vramusage" | "vram_usage" => {
            Some(SensorMetricKind::MemoryUsage)
        }
        "voltage" | "voltages" | "volt" | "volts" => Some(SensorMetricKind::Voltage),
        "power" | "powers" | "watt" | "watts" => Some(SensorMetricKind::Power),
        "clock" | "clocks" | "frequency" | "frequencies" => Some(SensorMetricKind::Clock),
        _ => None,
    }
}

fn helper_sensor_status(status: &str, message: Option<String>) -> SensorStatus {
    match status.to_ascii_lowercase().as_str() {
        "ok" => SensorStatus::Ok,
        "partial" => SensorStatus::Partial(
            message.unwrap_or_else(|| "Partial sensor data".to_owned()),
        ),
        "permissiondenied" | "permission_denied" | "permission" => SensorStatus::PermissionDenied,
        "unavailable" => SensorStatus::Unsupported,
        "unsupported" => message
            .map(SensorStatus::Error)
            .unwrap_or(SensorStatus::Unsupported),
        "stale" => SensorStatus::Stale,
        "error" => SensorStatus::Error(message.unwrap_or_else(|| "Sensor helper error".to_owned())),
        _ => SensorStatus::Error(
            message.unwrap_or_else(|| format!("Unknown helper status: {status}")),
        ),
    }
}

fn json_object_for_key<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pattern = format!("\"{key}\":");
    let start = line.find(&pattern)? + pattern.len();
    let bytes = line.as_bytes();
    let mut index = start;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if bytes.get(index).copied()? != b'{' {
        return None;
    }

    let object_start = index;
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in line[object_start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let end = object_start + offset + ch.len_utf8();
                    return Some(&line[object_start..end]);
                }
            }
            _ => {}
        }
    }
    None
}

fn json_array_for_key<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pattern = format!("\"{key}\":");
    let start = line.find(&pattern)? + pattern.len();
    let bytes = line.as_bytes();
    let mut index = start;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if bytes.get(index).copied()? != b'[' {
        return None;
    }

    let array_start = index;
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in line[array_start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    let end = array_start + offset + ch.len_utf8();
                    return Some(&line[array_start..end]);
                }
            }
            _ => {}
        }
    }
    None
}

fn json_objects_in_array(array: &str) -> Vec<&str> {
    let mut objects = Vec::new();
    let mut object_start = None;
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in array.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    object_start = Some(index);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(start) = object_start.take() {
                        objects.push(&array[start..index + ch.len_utf8()]);
                    }
                }
            }
            _ => {}
        }
    }

    objects
}

fn json_string_for_key(object: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{key}\":");
    let start = object.find(&pattern)? + pattern.len();
    let mut index = start;
    let bytes = object.as_bytes();
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if bytes.get(index).copied()? != b'"' {
        return None;
    }
    index += 1;
    let mut value = String::new();
    let mut escaped = false;
    for ch in object[index..].chars() {
        if escaped {
            value.push(match ch {
                '"' => '"',
                '\\' => '\\',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(value);
        } else {
            value.push(ch);
        }
    }
    None
}

fn json_number_for_key(object: &str, key: &str) -> Option<f32> {
    let pattern = format!("\"{key}\":");
    let start = object.find(&pattern)? + pattern.len();
    let mut value = String::new();
    let mut started = false;
    for ch in object[start..].chars() {
        if ch.is_ascii_digit() || ch == '.' || ch == '-' {
            value.push(ch);
            started = true;
        } else if started {
            break;
        } else if ch == 'n' {
            return None;
        } else if !ch.is_ascii_whitespace() {
            return None;
        }
    }
    value.parse::<f32>().ok()
}

fn json_bool_for_key(object: &str, key: &str) -> Option<bool> {
    let pattern = format!("\"{key}\":");
    let start = object.find(&pattern)? + pattern.len();
    let value = object[start..].trim_start();
    if value.starts_with("true") {
        Some(true)
    } else if value.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn stale_checked_reading(
    reading: Option<SensorReading>,
    now: Instant,
    stale_after: Duration,
) -> Option<SensorReading> {
    reading.map(|reading| {
        if now.duration_since(reading.updated_at) > stale_after {
            reading.mark_stale()
        } else {
            reading
        }
    })
}

fn sensor_temperature(reading: Option<&SensorReading>) -> Option<f32> {
    reading
        .filter(|reading| reading.is_ok())
        .and_then(|reading| reading.temperature_c)
}

fn collect_sensor_snapshot(drive_letter: Option<char>) -> SensorSnapshot {
    let (cpu, gpu, gpu_memory, drive, memory) = thread::scope(|scope| {
        let cpu = scope.spawn(|| {
            let mut cpu = query_cpu_temperature();
            attach_utilization(
                &mut cpu,
                query_cpu_utilization(),
                "Windows performance counter",
                "CPU temperature unavailable; utilization is live",
            );
            cpu
        });
        let gpu = scope.spawn(|| {
            let mut gpu = query_gpu_temperature();
            attach_utilization(
                &mut gpu,
                query_gpu_utilization(),
                "Windows GPU Engine counter",
                "GPU temperature unavailable; utilization is live",
            );
            gpu
        });
        let gpu_memory = scope.spawn(query_gpu_memory_sensor);
        let drive = scope.spawn(|| {
            let mut drive = query_drive_temperature(drive_letter);
            drive.utilization_percent = query_drive_utilization(drive_letter);
            drive
        });
        let memory = scope.spawn(query_memory_sensor);

        (
            cpu.join().unwrap_or_else(|_| {
                SensorReading::unavailable(
                    SensorKind::Cpu,
                    "CPU",
                    "Windows sensors",
                    SensorStatus::Error("CPU sensor worker panicked".to_owned()),
                )
            }),
            gpu.join().unwrap_or_else(|_| {
                SensorReading::unavailable(
                    SensorKind::Gpu,
                    "GPU",
                    "Windows sensors",
                    SensorStatus::Error("GPU sensor worker panicked".to_owned()),
                )
            }),
            gpu_memory.join().unwrap_or_else(|_| {
                SensorReading::unavailable(
                    SensorKind::GpuMemory,
                    "VRAM",
                    "NVML/nvidia-smi",
                    SensorStatus::Error("VRAM sensor worker panicked".to_owned()),
                )
            }),
            drive.join().unwrap_or_else(|_| {
                SensorReading::unavailable(
                    SensorKind::Drive,
                    "SSD",
                    "Windows Storage",
                    SensorStatus::Error("Drive sensor worker panicked".to_owned()),
                )
            }),
            memory.join().unwrap_or_else(|_| {
                SensorReading::unavailable(
                    SensorKind::Memory,
                    "System RAM",
                    "Windows memory status",
                    SensorStatus::Error("Memory sensor worker panicked".to_owned()),
                )
            }),
        )
    });

    SensorSnapshot {
        cpu: Some(cpu),
        gpu: Some(gpu),
        gpu_memory: Some(gpu_memory),
        drive: Some(drive),
        memory: Some(memory),
        helper_elevated: None,
    }
}

fn attach_utilization(
    reading: &mut SensorReading,
    utilization_percent: Option<f32>,
    utilization_provider: &str,
    partial_message: &str,
) {
    let Some(utilization_percent) = utilization_percent else {
        return;
    };
    reading.utilization_percent = Some(utilization_percent);
    reading.upsert_metric(SensorMetric::new(
        SensorMetricKind::Utilization,
        SensorMetricKind::Utilization.default_label(),
        Some(utilization_percent),
    ));
    if reading.provider != utilization_provider {
        reading.provider = if reading.provider.is_empty() {
            utilization_provider.to_owned()
        } else {
            format!("{} + {}", reading.provider, utilization_provider)
        };
    }
    if !reading.has_temperature() {
        reading.status = SensorStatus::Partial(partial_message.to_owned());
    } else if !reading.is_ok() {
        reading.status = SensorStatus::Ok;
    }
}

fn query_cpu_temperature() -> SensorReading {
    #[cfg(windows)]
    {
        query_external_hardware_temperature(
            SensorKind::Cpu,
            "CPU",
            external_cpu_temperature_script(),
        )
        .unwrap_or_else(|| {
            SensorReading::unavailable(
                SensorKind::Cpu,
                "CPU",
                "Windows safe sensors",
                SensorStatus::Unsupported,
            )
        })
    }

    #[cfg(not(windows))]
    {
        SensorReading::unavailable(
            SensorKind::Cpu,
            "CPU",
            "Windows sensors",
            SensorStatus::Unsupported,
        )
    }
}

fn query_gpu_temperature() -> SensorReading {
    #[cfg(windows)]
    {
        match run_nvidia_smi_temperature_query() {
            Ok(output) => parse_first_temperature(&output)
                .map(|temp| SensorReading::ok(SensorKind::Gpu, "GPU", temp, "NVML/nvidia-smi"))
                .unwrap_or_else(|| {
                    SensorReading::unavailable(
                        SensorKind::Gpu,
                        "GPU",
                        "NVML/nvidia-smi",
                        SensorStatus::Unsupported,
                    )
                }),
            Err(err) => query_external_hardware_temperature(
                SensorKind::Gpu,
                "GPU",
                external_gpu_temperature_script(),
            )
            .unwrap_or_else(|| {
                SensorReading::unavailable(
                    SensorKind::Gpu,
                    "GPU",
                    "NVML/nvidia-smi",
                    sensor_error_status(err),
                )
            }),
        }
    }

    #[cfg(not(windows))]
    {
        SensorReading::unavailable(
            SensorKind::Gpu,
            "GPU",
            "Windows sensors",
            SensorStatus::Unsupported,
        )
    }
}

fn query_gpu_memory_sensor() -> SensorReading {
    #[cfg(windows)]
    {
        match run_nvidia_smi_gpu_memory_query() {
            Ok(output) => parse_nvidia_smi_gpu_memory_reading(&output).unwrap_or_else(|| {
                SensorReading::unavailable(
                    SensorKind::GpuMemory,
                    "VRAM",
                    "NVML/nvidia-smi",
                    SensorStatus::Unsupported,
                )
            }),
            Err(err) => SensorReading::unavailable(
                SensorKind::GpuMemory,
                "VRAM",
                "NVML/nvidia-smi",
                sensor_error_status(err),
            ),
        }
    }

    #[cfg(not(windows))]
    {
        SensorReading::unavailable(
            SensorKind::GpuMemory,
            "VRAM",
            "Windows sensors",
            SensorStatus::Unsupported,
        )
    }
}

fn parse_nvidia_smi_gpu_memory_reading(output: &str) -> Option<SensorReading> {
    let line = output.lines().find(|line| !line.trim().is_empty())?;
    let columns = line.split(',').map(str::trim).collect::<Vec<_>>();
    let has_temperature_column = columns.len() >= 4;
    let temperature_c = has_temperature_column
        .then(|| columns.get(1).and_then(|value| parse_metric_number(value)))
        .flatten()
        .filter(|value| (-40.0..=130.0).contains(value));
    let used_mib_column = if has_temperature_column { 2 } else { 1 };
    let total_mib_column = if has_temperature_column { 3 } else { 2 };
    let used_gb = columns
        .get(used_mib_column)
        .and_then(|value| parse_metric_number(value))
        .map(mib_to_gb);
    let total_gb = columns
        .get(total_mib_column)
        .and_then(|value| parse_metric_number(value))
        .map(mib_to_gb)
        .filter(|value| *value > 0.0);
    if temperature_c.is_none() && used_gb.is_none() {
        return None;
    }

    let mut metrics = Vec::new();
    if let Some(value) = temperature_c {
        metrics.push(SensorMetric::new(
            SensorMetricKind::Temperature,
            "VRAM",
            Some(value),
        ));
    }
    if let Some(value) = used_gb {
        metrics.push(SensorMetric::new(
            SensorMetricKind::MemoryUsage,
            SensorMetricKind::MemoryUsage.default_label(),
            Some(value),
        )
        .with_range(None, total_gb));
    }

    let mut reading = SensorReading {
        kind: SensorKind::GpuMemory,
        label: "VRAM".to_owned(),
        temperature_c,
        utilization_percent: None,
        metrics,
        provider: "NVML/nvidia-smi".to_owned(),
        updated_at: Instant::now(),
        status: SensorStatus::Ok,
    };
    reading.sync_legacy_metrics();
    Some(reading)
}

#[cfg(windows)]
fn query_external_hardware_temperature(
    kind: SensorKind,
    fallback_label: &str,
    script: &str,
) -> Option<SensorReading> {
    let output = run_powershell_sensor_script(script).ok()?;
    let mut parts = output.trim().split('\t');
    let temperature = parts.next()?.trim().parse::<f32>().ok()?;
    if !(-40.0..=130.0).contains(&temperature) {
        return None;
    }
    let label = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback_label);
    let namespace = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("external hardware WMI");
    Some(SensorReading::ok(
        kind,
        label,
        temperature,
        &format!("External hardware WMI ({namespace})"),
    ))
}

#[cfg(windows)]
fn external_cpu_temperature_script() -> &'static str {
    r#"
$ErrorActionPreference = 'SilentlyContinue'
$namespaces = @('root\LibreHardwareMonitor', 'root\OpenHardwareMonitor')
foreach ($namespace in $namespaces) {
    $sensors = Get-CimInstance -Namespace $namespace -ClassName Sensor -ErrorAction SilentlyContinue |
        Where-Object { $_.SensorType -eq 'Temperature' -and $null -ne $_.Value }
    if (-not $sensors) { continue }
    $candidates = $sensors | Where-Object {
        "$($_.Identifier) $($_.HardwareType) $($_.Parent) $($_.Name)" -match '(?i)(/cpu|intelcpu|amdcpu|cpu)'
    }
    $preferred = $candidates |
        Sort-Object @{ Expression = { if ($_.Name -match '(?i)(package|tctl|tdie|ccd|core max)') { 0 } else { 1 } } },
                    @{ Expression = { [double]$_.Value }; Descending = $true } |
        Select-Object -First 1
    if ($preferred) {
        "$([math]::Round([double]$preferred.Value, 1))`t$($preferred.Name)`t$namespace"
        break
    }
}
exit 0
"#
}

#[cfg(windows)]
fn external_gpu_temperature_script() -> &'static str {
    r#"
$ErrorActionPreference = 'SilentlyContinue'
$namespaces = @('root\LibreHardwareMonitor', 'root\OpenHardwareMonitor')
foreach ($namespace in $namespaces) {
    $sensors = Get-CimInstance -Namespace $namespace -ClassName Sensor -ErrorAction SilentlyContinue |
        Where-Object { $_.SensorType -eq 'Temperature' -and $null -ne $_.Value }
    if (-not $sensors) { continue }
    $candidates = $sensors | Where-Object {
        "$($_.Identifier) $($_.HardwareType) $($_.Parent) $($_.Name)" -match '(?i)(/gpu|nvidia|radeon|amd|graphics|intel.*gpu|intel.*graphics)'
    }
    $preferred = $candidates |
        Sort-Object @{ Expression = { if ($_.Name -match '(?i)(gpu core|core|hot spot|junction)') { 0 } else { 1 } } },
                    @{ Expression = { [double]$_.Value }; Descending = $true } |
        Select-Object -First 1
    if ($preferred) {
        "$([math]::Round([double]$preferred.Value, 1))`t$($preferred.Name)`t$namespace"
        break
    }
}
exit 0
"#
}

fn query_drive_temperature(drive_letter: Option<char>) -> SensorReading {
    let Some(drive_letter) = drive_letter else {
        return SensorReading::unavailable(
            SensorKind::Drive,
            "SSD",
            "Windows Storage",
            SensorStatus::Unsupported,
        );
    };
    let label = format!("SSD {drive_letter}:");

    #[cfg(windows)]
    {
        let script = format!(
            r#"
$ErrorActionPreference = 'Stop'
$letter = '{drive_letter}'
$partition = Get-Partition -DriveLetter $letter -ErrorAction Stop | Select-Object -First 1
if ($partition) {{
    $disk = $partition | Get-Disk -ErrorAction Stop
    if ($disk) {{
        $physical = Get-PhysicalDisk -ErrorAction Stop |
            Where-Object {{
                $_.DeviceId -eq "$($disk.Number)" -or
                ($disk.SerialNumber -and $_.SerialNumber -eq $disk.SerialNumber) -or
                ($disk.FriendlyName -and $_.FriendlyName -eq $disk.FriendlyName)
            }} |
            Select-Object -First 1
        if ($physical) {{
            $counter = $physical | Get-StorageReliabilityCounter -ErrorAction Stop
            if ($counter -and $null -ne $counter.Temperature) {{
                [math]::Round($counter.Temperature, 1)
            }}
        }}
    }}
}}
"#
        );
        match run_powershell_sensor_script(&script) {
            Ok(output) => parse_first_temperature(&output)
                .map(|temp| {
                    SensorReading::ok(
                        SensorKind::Drive,
                        label.clone(),
                        temp,
                        "Windows Storage SMART",
                    )
                })
                .unwrap_or_else(|| {
                    SensorReading::unavailable(
                        SensorKind::Drive,
                        label.clone(),
                        "Windows Storage SMART",
                        SensorStatus::Unsupported,
                    )
                }),
            Err(err) => SensorReading::unavailable(
                SensorKind::Drive,
                label,
                "Windows Storage SMART",
                sensor_error_status(err),
            ),
        }
    }

    #[cfg(not(windows))]
    {
        let _ = drive_letter;
        SensorReading::unavailable(
            SensorKind::Drive,
            label,
            "Windows Storage SMART",
            SensorStatus::Unsupported,
        )
    }
}

fn query_memory_sensor() -> SensorReading {
    match detect_ram_memory_info() {
        Ok(info) => {
            let mut reading = SensorReading {
            kind: SensorKind::Memory,
            label: "System RAM".to_owned(),
            temperature_c: None,
            utilization_percent: Some(clamp_percent(info.memory_load_percent as f32)),
            metrics: Vec::new(),
            provider: "Windows memory status".to_owned(),
            updated_at: Instant::now(),
            status: SensorStatus::Ok,
            };
            reading.sync_legacy_metrics();
            reading
        }
        Err(err) => SensorReading::unavailable(
            SensorKind::Memory,
            "System RAM",
            "Windows memory status",
            sensor_error_status(err),
        ),
    }
}

fn query_cpu_utilization() -> Option<f32> {
    #[cfg(windows)]
    {
        let counter_script = r#"
$sample = Get-Counter '\Processor(_Total)\% Processor Time' -ErrorAction Stop
if ($sample -and $sample.CounterSamples.Count -gt 0) {
    [math]::Round([math]::Min(100, [math]::Max(0, $sample.CounterSamples[0].CookedValue)), 1)
}
"#;
        run_powershell_sensor_script(counter_script)
            .ok()
            .and_then(|output| parse_first_utilization(&output))
            .or_else(|| {
                let script = r#"
$counter = Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor -Filter "Name='_Total'" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($counter) {
    [math]::Round([math]::Min(100, [math]::Max(0, $counter.PercentProcessorTime)), 1)
}
"#;
                run_powershell_sensor_script(script)
                    .ok()
                    .and_then(|output| parse_first_utilization(&output))
            })
    }

    #[cfg(not(windows))]
    {
        None
    }
}

fn query_gpu_utilization() -> Option<f32> {
    #[cfg(windows)]
    {
        let counter_script = r#"
$sample = Get-Counter '\GPU Engine(*)\Utilization Percentage' -ErrorAction Stop
if ($sample) {
    $sum = ($sample.CounterSamples | Measure-Object -Property CookedValue -Sum).Sum
    [math]::Round([math]::Min(100, [math]::Max(0, $sum)), 1)
}
"#;
        run_powershell_sensor_script(counter_script)
            .ok()
            .and_then(|output| parse_first_utilization(&output))
            .or_else(|| {
                let script = r#"
$engines = Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine -ErrorAction SilentlyContinue
if ($engines) {
    $sum = ($engines | Measure-Object -Property UtilizationPercentage -Sum).Sum
    [math]::Round([math]::Min(100, [math]::Max(0, $sum)), 1)
}
"#;
                run_powershell_sensor_script(script)
                    .ok()
                    .and_then(|output| parse_first_utilization(&output))
            })
    }

    #[cfg(not(windows))]
    {
        None
    }
}

fn query_drive_utilization(drive_letter: Option<char>) -> Option<f32> {
    let drive_letter = drive_letter?;

    #[cfg(windows)]
    {
        let script = format!(
            r#"
$counter = Get-CimInstance Win32_PerfFormattedData_PerfDisk_LogicalDisk -Filter "Name='{drive_letter}:'" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($counter) {{
    [math]::Round([math]::Min(100, [math]::Max(0, $counter.PercentDiskTime)), 1)
}}
"#
        );
        run_powershell_sensor_script(&script)
            .ok()
            .and_then(|output| parse_first_utilization(&output))
            .or_else(|| {
                let counter_script = format!(
                    r#"
$sample = Get-Counter '\LogicalDisk({drive_letter}:)\% Disk Time' -ErrorAction SilentlyContinue
if ($sample -and $sample.CounterSamples.Count -gt 0) {{
    [math]::Round([math]::Min(100, [math]::Max(0, $sample.CounterSamples[0].CookedValue)), 1)
}}
"#
                );
                run_powershell_sensor_script(&counter_script)
                    .ok()
                    .and_then(|output| parse_first_utilization(&output))
            })
    }

    #[cfg(not(windows))]
    {
        let _ = drive_letter;
        None
    }
}

fn parse_first_temperature(output: &str) -> Option<f32> {
    output
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.' || ch == '-'))
        .filter(|token| !token.is_empty())
        .filter_map(|token| token.parse::<f32>().ok())
        .find(|value| (-40.0..=130.0).contains(value))
}

fn parse_first_utilization(output: &str) -> Option<f32> {
    output
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.' || ch == '-'))
        .filter(|token| !token.is_empty())
        .filter_map(|token| token.parse::<f32>().ok())
        .find(|value| (0.0..=10_000.0).contains(value))
        .map(clamp_percent)
}

fn parse_metric_number(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.' || ch == '-'))
        .filter(|token| !token.is_empty())
        .filter_map(|token| token.parse::<f32>().ok())
        .find(|value| value.is_finite())
}

fn mib_to_gb(value: f32) -> f32 {
    ((value / 1024.0) * 10.0).round() / 10.0
}

fn clamp_percent(value: f32) -> f32 {
    (value.clamp(0.0, 100.0) * 10.0).round() / 10.0
}

fn sensor_error_status(err: anyhow::Error) -> SensorStatus {
    let message = err.to_string();
    let lower = message.to_ascii_lowercase();
    if lower.contains("access is denied") || lower.contains("permission") {
        SensorStatus::PermissionDenied
    } else if lower.contains("not found")
        || lower.contains("not recognized")
        || lower.contains("cannot find")
        || lower.contains("os error 2")
    {
        SensorStatus::Unsupported
    } else {
        SensorStatus::Error(message)
    }
}

#[cfg(windows)]
fn run_powershell_sensor_script(script: &str) -> Result<String> {
    run_command_no_window(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ],
    )
}

#[cfg(windows)]
fn run_nvidia_smi_temperature_query() -> Result<String> {
    const NVIDIA_SMI_ARGS: &[&str] = &[
        "--query-gpu=temperature.gpu",
        "--format=csv,noheader,nounits",
    ];

    run_nvidia_smi_query(NVIDIA_SMI_ARGS)
}

#[cfg(windows)]
fn run_nvidia_smi_gpu_memory_query() -> Result<String> {
    const NVIDIA_SMI_MEMORY_ARGS: &[&str] = &[
        "--query-gpu=name,temperature.memory,memory.used,memory.total",
        "--format=csv,noheader,nounits",
    ];
    const NVIDIA_SMI_MEMORY_FALLBACK_ARGS: &[&str] = &[
        "--query-gpu=name,memory.used,memory.total",
        "--format=csv,noheader,nounits",
    ];

    run_nvidia_smi_query(NVIDIA_SMI_MEMORY_ARGS)
        .or_else(|_| run_nvidia_smi_query(NVIDIA_SMI_MEMORY_FALLBACK_ARGS))
}

#[cfg(windows)]
fn run_nvidia_smi_query(args: &[&str]) -> Result<String> {
    match run_command_no_window("nvidia-smi", args) {
        Ok(output) => Ok(output),
        Err(path_err) => {
            for fallback in nvidia_smi_fallback_paths() {
                if fallback.is_file() {
                    if let Ok(output) = run_command_no_window(&fallback.display().to_string(), args)
                    {
                        return Ok(output);
                    }
                }
            }
            Err(anyhow!("nvidia-smi unavailable on PATH ({path_err}) or at known install paths"))
        }
    }
}

#[cfg(windows)]
fn nvidia_smi_fallback_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(system_root) = std::env::var("SystemRoot") {
        paths.push(PathBuf::from(system_root).join("System32/nvidia-smi.exe"));
    }
    for key in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(program_files) = std::env::var(key) {
            paths.push(PathBuf::from(program_files).join("NVIDIA Corporation/NVSMI/nvidia-smi.exe"));
        }
    }
    paths
}

#[cfg(windows)]
fn run_command_no_window(program: &str, args: &[&str]) -> Result<String> {
    run_command_no_window_timeout(
        program,
        args,
        Duration::from_millis(SENSOR_COMMAND_TIMEOUT_MS),
    )
}

#[cfg(windows)]
fn run_command_no_window_timeout(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<String> {
    let mut child = Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW_RAW)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start {program}"))?;
    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .with_context(|| format!("failed to query {program} status"))?
            .is_some()
        {
            break;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(
                "{} timed out after {} ms",
                program,
                timeout.as_millis()
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }

    let output = child
        .wait_with_output()
        .with_context(|| format!("failed to collect {program} output"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(anyhow!(
            "{} failed{}",
            program,
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {stderr}")
            }
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

