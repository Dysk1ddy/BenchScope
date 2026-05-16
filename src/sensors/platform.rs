fn sensor_helper_enabled() -> bool {
    env_flag_enabled(SENSOR_HELPER_ENABLE_ENV)
}

fn sensor_permission_prompt_needed() -> bool {
    sensor_helper_path().is_some() && !sensor_helper_enabled()
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
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

#[cfg(test)]
fn helper_snapshot_needs_elevation(snapshot: &SensorSnapshot) -> bool {
    snapshot.helper_elevated == Some(false)
        && [
            snapshot.cpu.as_ref(),
            snapshot.gpu.as_ref(),
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

fn sensor_reading_has_gap(reading: Option<&SensorReading>) -> bool {
    match reading {
        Some(reading) => !reading.has_temperature() || !reading.has_utilization(),
        None => true,
    }
}

fn merge_sensor_snapshots(
    primary: Option<SensorSnapshot>,
    fallback: Option<SensorSnapshot>,
) -> Option<SensorSnapshot> {
    match (primary, fallback) {
        (Some(primary), Some(fallback)) => Some(SensorSnapshot {
            cpu: prefer_sensor_reading(primary.cpu, fallback.cpu),
            gpu: prefer_sensor_reading(primary.gpu, fallback.gpu),
            drive: prefer_sensor_reading(primary.drive, fallback.drive),
            memory: prefer_sensor_reading(primary.memory, fallback.memory),
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

fn merge_sensor_reading(mut primary: SensorReading, fallback: SensorReading) -> SensorReading {
    if primary.temperature_c.is_none() && fallback.temperature_c.is_some() {
        primary.temperature_c = fallback.temperature_c;
        if primary.label.is_empty() || !primary.has_utilization() {
            primary.label = fallback.label.clone();
        }
    }
    if primary.utilization_percent.is_none() && fallback.utilization_percent.is_some() {
        primary.utilization_percent = fallback.utilization_percent;
    }
    if !primary.is_ok()
        && (primary.temperature_c.is_some() || primary.utilization_percent.is_some())
    {
        primary.status = SensorStatus::Ok;
    }
    if primary.provider != fallback.provider
        && (fallback.temperature_c.is_some() || fallback.utilization_percent.is_some())
    {
        primary.provider = format!("{} + {}", primary.provider, fallback.provider);
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
    snapshot.gpu = Some(SensorReading {
        kind: SensorKind::Gpu,
        label: "iGPU shared CPU package".to_owned(),
        temperature_c: Some(cpu_temperature),
        utilization_percent,
        provider: format!("Shared CPU package ({})", cpu.provider),
        updated_at: Instant::now(),
        status: SensorStatus::Ok,
    });
    snapshot
}

fn sensor_helper_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(dir) = exe_path.parent() {
            candidates.push(dir.join("BenchScope.SensorHelper.exe"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("target/release/BenchScope.SensorHelper.exe"));
        candidates.push(cwd.join("target/debug/BenchScope.SensorHelper.exe"));
        candidates.push(
            cwd.join("sensor-helper/bin/Release/net10.0-windows/BenchScope.SensorHelper.exe"),
        );
        candidates
            .push(cwd.join("sensor-helper/bin/Debug/net10.0-windows/BenchScope.SensorHelper.exe"));
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn parse_helper_snapshot(line: &str) -> Option<SensorSnapshot> {
    if !line.contains("\"timestampUtc\"") && !line.contains("\"cpu\"") {
        return None;
    }

    Some(SensorSnapshot {
        cpu: parse_helper_reading(line, "cpu", SensorKind::Cpu, "CPU"),
        gpu: parse_helper_reading(line, "gpu", SensorKind::Gpu, "GPU"),
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
    let temperature_c = json_number_for_key(object, "temperatureC");
    let utilization_percent = json_number_for_key(object, "utilizationPercent").map(clamp_percent);
    let status = helper_sensor_status(&status_text, message);

    Some(SensorReading {
        kind,
        label,
        temperature_c,
        utilization_percent,
        provider,
        updated_at: Instant::now(),
        status,
    })
}

fn helper_sensor_status(status: &str, message: Option<String>) -> SensorStatus {
    match status.to_ascii_lowercase().as_str() {
        "ok" => SensorStatus::Ok,
        "permissiondenied" | "permission_denied" | "permission" => SensorStatus::PermissionDenied,
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
    let mut cpu = query_cpu_temperature();
    cpu.utilization_percent = query_cpu_utilization();
    let mut gpu = query_gpu_temperature();
    gpu.utilization_percent = query_gpu_utilization();
    let mut drive = query_drive_temperature(drive_letter);
    drive.utilization_percent = query_drive_utilization(drive_letter);
    let memory = query_memory_sensor();

    SensorSnapshot {
        cpu: Some(cpu),
        gpu: Some(gpu),
        drive: Some(drive),
        memory: Some(memory),
        helper_elevated: None,
    }
}

fn query_cpu_temperature() -> SensorReading {
    #[cfg(windows)]
    {
        let script = r#"
$zone = Get-CimInstance -Namespace root/wmi -ClassName MSAcpi_ThermalZoneTemperature -ErrorAction SilentlyContinue | Select-Object -First 1
if ($zone -and $zone.CurrentTemperature) {
    [math]::Round(($zone.CurrentTemperature / 10) - 273.15, 1)
}
"#;
        match run_powershell_sensor_script(script) {
            Ok(output) => parse_first_temperature(&output)
                .map(|temp| SensorReading::ok(SensorKind::Cpu, "CPU", temp, "ACPI thermal zone"))
                .unwrap_or_else(|| {
                    SensorReading::unavailable(
                        SensorKind::Cpu,
                        "CPU",
                        "ACPI thermal zone",
                        SensorStatus::Unsupported,
                    )
                }),
            Err(err) => SensorReading::unavailable(
                SensorKind::Cpu,
                "CPU",
                "ACPI thermal zone",
                sensor_error_status(err),
            ),
        }
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
            Err(err) => SensorReading::unavailable(
                SensorKind::Gpu,
                "GPU",
                "NVML/nvidia-smi",
                sensor_error_status(err),
            ),
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
$partition = Get-Partition -DriveLetter $letter | Select-Object -First 1
if ($partition) {{
    $disk = $partition | Get-Disk
    if ($disk) {{
        $physical = Get-PhysicalDisk |
            Where-Object {{ $_.DeviceId -eq "$($disk.Number)" -or $_.SerialNumber -eq $disk.SerialNumber }} |
            Select-Object -First 1
        if ($physical) {{
            $counter = $physical | Get-StorageReliabilityCounter
            if ($counter -and $counter.Temperature) {{
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
        Ok(info) => SensorReading {
            kind: SensorKind::Memory,
            label: "System RAM".to_owned(),
            temperature_c: None,
            utilization_percent: Some(clamp_percent(info.memory_load_percent as f32)),
            provider: "Windows memory status".to_owned(),
            updated_at: Instant::now(),
            status: SensorStatus::Ok,
        },
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
        let script = r#"
$counter = Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor -Filter "Name='_Total'" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($counter) {
    [math]::Round([math]::Min(100, [math]::Max(0, $counter.PercentProcessorTime)), 1)
}
"#;
        run_powershell_sensor_script(script)
            .ok()
            .and_then(|output| parse_first_utilization(&output))
            .or_else(|| {
                let counter_script = r#"
$sample = Get-Counter '\Processor(_Total)\% Processor Time' -ErrorAction SilentlyContinue
if ($sample -and $sample.CounterSamples.Count -gt 0) {
    [math]::Round([math]::Min(100, [math]::Max(0, $sample.CounterSamples[0].CookedValue)), 1)
}
"#;
                run_powershell_sensor_script(counter_script)
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
            .or_else(|| {
                let counter_script = r#"
$sample = Get-Counter '\GPU Engine(*)\Utilization Percentage' -ErrorAction SilentlyContinue
if ($sample) {
    $sum = ($sample.CounterSamples | Measure-Object -Property CookedValue -Sum).Sum
    [math]::Round([math]::Min(100, [math]::Max(0, $sum)), 1)
}
"#;
                run_powershell_sensor_script(counter_script)
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

    match run_command_no_window("nvidia-smi", NVIDIA_SMI_ARGS) {
        Ok(output) => Ok(output),
        Err(path_err) => {
            let fallback = r"C:\Program Files\NVIDIA Corporation\NVSMI\nvidia-smi.exe";
            run_command_no_window(fallback, NVIDIA_SMI_ARGS).map_err(|fallback_err| {
                anyhow!(
                    "nvidia-smi unavailable on PATH ({path_err}) or at default install path ({fallback_err})"
                )
            })
        }
    }
}

#[cfg(windows)]
fn run_command_no_window(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW_RAW)
        .output()
        .with_context(|| format!("failed to start {program}"))?;
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

