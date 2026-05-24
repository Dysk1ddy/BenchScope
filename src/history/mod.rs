const HISTORY_SCHEMA_VERSION: u32 = 1;
const HISTORY_EVENT_FILE: &str = "events.jsonl";
const HISTORY_BASELINES_FILE: &str = "baselines.json";
const HISTORY_RECENT_LIMIT: usize = 500;
const HISTORY_BUNDLE_EVENT_LIMIT: usize = 250;
const HISTORY_BETTER_HIGHER: &str = "higher";
const HISTORY_BETTER_LOWER: &str = "lower";
const HISTORY_BETTER_ZERO: &str = "zero";
const HISTORY_BETTER_NEUTRAL: &str = "neutral";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct HistoryMetric {
    name: String,
    value: f64,
    display: String,
    unit: String,
    better: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct HistoryPair {
    key: String,
    value: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct HistoryEvent {
    schema_version: u32,
    event_id: String,
    captured_at_unix_ms: u64,
    app_version: String,
    category: String,
    title: String,
    profile_key: String,
    summary: String,
    metrics: Vec<HistoryMetric>,
    details: Vec<HistoryPair>,
    warnings: Vec<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct PinnedBaseline {
    category: String,
    profile_key: String,
    event_id: String,
    label: String,
    pinned_at_unix_ms: u64,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct BaselineIndex {
    schema_version: u32,
    pinned: Vec<PinnedBaseline>,
}

#[derive(Clone, Debug)]
struct HistoryDelta {
    metric: String,
    baseline: String,
    current: String,
    delta: String,
    direction: String,
    severity: String,
}

#[derive(Clone, Debug)]
struct HistoryComparison {
    category: String,
    profile_key: String,
    baseline_title: String,
    current_title: String,
    deltas: Vec<HistoryDelta>,
    notes: Vec<String>,
}

#[derive(Clone, Debug)]
struct HardwareChange {
    field: String,
    previous: String,
    current: String,
}

#[derive(Clone, Debug)]
struct RedactionOptions {
    include_sensitive_ids: bool,
    include_local_paths: bool,
    include_network_addresses: bool,
    include_wifi_names: bool,
}

impl Default for RedactionOptions {
    fn default() -> Self {
        Self {
            include_sensitive_ids: false,
            include_local_paths: false,
            include_network_addresses: false,
            include_wifi_names: false,
        }
    }
}

struct HistoryState {
    root_dir: PathBuf,
    history_dir: PathBuf,
    bundles_dir: PathBuf,
    logs_dir: PathBuf,
    events: Vec<HistoryEvent>,
    baselines: BaselineIndex,
    selected_category: String,
    redaction: RedactionOptions,
    last_status: String,
    last_error: Option<String>,
    last_bundle_path: Option<PathBuf>,
    confirm_delete: bool,
    confirm_bundle_export: bool,
}

impl HistoryState {
    fn new() -> Self {
        let root_dir = history_app_data_root();
        let history_dir = root_dir.join("history");
        let bundles_dir = root_dir.join("bundles");
        let logs_dir = root_dir.join("logs");
        let mut last_error = None;
        for dir in [&root_dir, &history_dir, &bundles_dir, &logs_dir] {
            if let Err(err) = fs::create_dir_all(dir) {
                last_error = Some(format!("Could not create {}: {err}", dir.display()));
            }
        }

        let mut events = Vec::new();
        let mut status = match read_history_events(&history_dir.join(HISTORY_EVENT_FILE)) {
            Ok(loaded) => {
                events = loaded;
                format!("Loaded {} history event(s)", events.len())
            }
            Err(err) => {
                last_error = Some(format!("Could not load history: {err:#}"));
                "History will start empty for this session".to_owned()
            }
        };
        if events.len() > HISTORY_RECENT_LIMIT {
            events = events[events.len() - HISTORY_RECENT_LIMIT..].to_vec();
        }

        let baselines = read_baselines(&history_dir.join(HISTORY_BASELINES_FILE))
            .unwrap_or_else(|err| {
                last_error = Some(format!("Could not load baselines: {err:#}"));
                BaselineIndex {
                    schema_version: HISTORY_SCHEMA_VERSION,
                    pinned: Vec::new(),
                }
            });

        if status.is_empty() {
            status = "History ready".to_owned();
        }

        Self {
            root_dir,
            history_dir,
            bundles_dir,
            logs_dir,
            events,
            baselines,
            selected_category: "all".to_owned(),
            redaction: RedactionOptions::default(),
            last_status: status,
            last_error,
            last_bundle_path: None,
            confirm_delete: false,
            confirm_bundle_export: false,
        }
    }

    fn append_event(&mut self, mut event: HistoryEvent) {
        if event.event_id.is_empty() {
            event.event_id = self.next_event_id();
        }
        if event.app_version.is_empty() {
            event.app_version = env!("CARGO_PKG_VERSION").to_owned();
        }
        match append_history_event(&self.history_dir.join(HISTORY_EVENT_FILE), &event) {
            Ok(()) => {
                self.events.push(event);
                if self.events.len() > HISTORY_RECENT_LIMIT {
                    let overflow = self.events.len() - HISTORY_RECENT_LIMIT;
                    self.events.drain(0..overflow);
                }
                self.last_status = format!("Saved history event ({} total)", self.events.len());
                self.last_error = None;
            }
            Err(err) => {
                self.last_error = Some(format!("History write failed: {err:#}"));
            }
        }
    }

    fn next_event_id(&self) -> String {
        format!(
            "{}-{}",
            history_now_unix_ms(),
            self.events.len().saturating_add(1)
        )
    }

    fn latest_event(&self, category: &str) -> Option<&HistoryEvent> {
        self.events
            .iter()
            .rev()
            .find(|event| category == "all" || event.category == category)
    }

    fn latest_event_for_category(&self, category: &str) -> Option<&HistoryEvent> {
        self.events
            .iter()
            .rev()
            .find(|event| event.category == category)
    }

    fn previous_comparable_event(&self, current: &HistoryEvent) -> Option<&HistoryEvent> {
        self.events.iter().rev().skip(1).find(|event| {
            event.category == current.category && event.profile_key == current.profile_key
        })
    }

    fn pinned_event_for(&self, current: &HistoryEvent) -> Option<&HistoryEvent> {
        let pinned = self
            .baselines
            .pinned
            .iter()
            .rev()
            .find(|baseline| {
                baseline.category == current.category && baseline.profile_key == current.profile_key
            })?;
        self.events
            .iter()
            .find(|event| event.event_id == pinned.event_id)
    }

    fn pin_latest_selected(&mut self) {
        let category = self.selected_category.clone();
        let Some(event) = self.latest_event(&category).cloned() else {
            self.last_error = Some("No history event is available to pin".to_owned());
            return;
        };
        self.baselines.pinned.retain(|baseline| {
            !(baseline.category == event.category && baseline.profile_key == event.profile_key)
        });
        self.baselines.pinned.push(PinnedBaseline {
            category: event.category.clone(),
            profile_key: event.profile_key.clone(),
            event_id: event.event_id.clone(),
            label: event.title.clone(),
            pinned_at_unix_ms: history_now_unix_ms(),
        });
        match write_baselines(&self.history_dir.join(HISTORY_BASELINES_FILE), &self.baselines) {
            Ok(()) => {
                self.last_status = format!("Pinned baseline: {}", event.title);
                self.last_error = None;
            }
            Err(err) => {
                self.last_error = Some(format!("Could not save baseline: {err:#}"));
            }
        }
    }

    fn clear_history(&mut self) {
        self.events.clear();
        self.baselines.pinned.clear();
        for file in [
            self.history_dir.join(HISTORY_EVENT_FILE),
            self.history_dir.join(HISTORY_BASELINES_FILE),
        ] {
            if file.exists() {
                let _ = fs::remove_file(file);
            }
        }
        self.last_status = "History and pinned baselines deleted".to_owned();
        self.last_error = None;
        self.confirm_delete = false;
    }

    fn selected_events(&self) -> Vec<&HistoryEvent> {
        let mut events = self
            .events
            .iter()
            .rev()
            .filter(|event| self.selected_category == "all" || event.category == self.selected_category)
            .take(80)
            .collect::<Vec<_>>();
        events.reverse();
        events
    }

    fn comparisons_for_selected(&self) -> Vec<HistoryComparison> {
        let categories = if self.selected_category == "all" {
            history_categories()
                .iter()
                .map(|category| category.id.to_owned())
                .collect::<Vec<_>>()
        } else {
            vec![self.selected_category.clone()]
        };
        let mut comparisons = Vec::new();
        for category in categories {
            let Some(current) = self.latest_event_for_category(&category) else {
                continue;
            };
            if let Some(previous) = self.previous_comparable_event(current) {
                comparisons.push(compare_history_events(previous, current));
            }
            if let Some(pinned) = self.pinned_event_for(current) {
                if pinned.event_id != current.event_id {
                    let mut comparison = compare_history_events(pinned, current);
                    comparison.notes.push("Compared with pinned baseline.".to_owned());
                    comparisons.push(comparison);
                }
            }
        }
        comparisons
    }

    fn hardware_changes(&self) -> Vec<HardwareChange> {
        let device_events = self
            .events
            .iter()
            .filter(|event| event.category == "device_info")
            .collect::<Vec<_>>();
        if device_events.len() < 2 {
            return Vec::new();
        }
        let previous = device_events[device_events.len() - 2];
        let current = device_events[device_events.len() - 1];
        diff_history_details(previous, current)
    }
}

#[derive(Clone, Copy)]
struct HistoryCategory {
    id: &'static str,
    label: &'static str,
}

fn history_categories() -> [HistoryCategory; 13] {
    [
        HistoryCategory {
            id: "all",
            label: "All",
        },
        HistoryCategory {
            id: "matrix_benchmark",
            label: "Matrix",
        },
        HistoryCategory {
            id: "matrix_stress",
            label: "Stress",
        },
        HistoryCategory {
            id: "drive_benchmark",
            label: "Drive",
        },
        HistoryCategory {
            id: "gpu_memory_benchmark",
            label: "GPU memory",
        },
        HistoryCategory {
            id: "ai_training_benchmark",
            label: "AI training",
        },
        HistoryCategory {
            id: "ram_test",
            label: "RAM",
        },
        HistoryCategory {
            id: "battery_diagnostic",
            label: "Battery",
        },
        HistoryCategory {
            id: "network_diagnostic",
            label: "Network",
        },
        HistoryCategory {
            id: "storage_health",
            label: "Storage health",
        },
        HistoryCategory {
            id: "device_info",
            label: "Device info",
        },
        HistoryCategory {
            id: "sensor_snapshot",
            label: "Sensors",
        },
        HistoryCategory {
            id: "thermal_timeline",
            label: "Thermal timeline",
        },
    ]
}

fn history_app_data_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("BenchScope"))
        .unwrap_or_else(|| std::env::temp_dir().join("BenchScope"))
}

fn history_now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn history_now_unix_seconds() -> u64 {
    history_now_unix_ms() / 1000
}

fn read_history_events(path: &PathBuf) -> Result<Vec<HistoryEvent>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<HistoryEvent>(&line) {
            events.push(event);
        }
    }
    Ok(events)
}

fn append_history_event(path: &PathBuf, event: &HistoryEvent) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    serde_json::to_writer(&mut file, event)?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

fn read_baselines(path: &PathBuf) -> Result<BaselineIndex> {
    if !path.exists() {
        return Ok(BaselineIndex {
            schema_version: HISTORY_SCHEMA_VERSION,
            pinned: Vec::new(),
        });
    }
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut baselines: BaselineIndex = serde_json::from_str(&text)?;
    baselines.schema_version = HISTORY_SCHEMA_VERSION;
    Ok(baselines)
}

fn write_baselines(path: &PathBuf, baselines: &BaselineIndex) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, serde_json::to_vec_pretty(baselines)?)
        .with_context(|| format!("writing {}", temp.display()))?;
    fs::rename(&temp, path).with_context(|| format!("moving {}", temp.display()))?;
    Ok(())
}

fn history_base_event(category: &str, title: impl Into<String>, profile_key: impl Into<String>) -> HistoryEvent {
    HistoryEvent {
        schema_version: HISTORY_SCHEMA_VERSION,
        event_id: String::new(),
        captured_at_unix_ms: history_now_unix_ms(),
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        category: category.to_owned(),
        title: title.into(),
        profile_key: profile_key.into(),
        summary: String::new(),
        metrics: Vec::new(),
        details: Vec::new(),
        warnings: Vec::new(),
    }
}

fn history_metric(
    name: impl Into<String>,
    value: f64,
    display: impl Into<String>,
    unit: impl Into<String>,
    better: &str,
) -> HistoryMetric {
    HistoryMetric {
        name: name.into(),
        value,
        display: display.into(),
        unit: unit.into(),
        better: better.to_owned(),
    }
}

fn history_pair(key: impl Into<String>, value: impl Into<String>) -> HistoryPair {
    HistoryPair {
        key: key.into(),
        value: value.into(),
    }
}

fn history_event_from_matrix_result(result: &BenchmarkResult) -> HistoryEvent {
    let mut event = history_base_event(
        "matrix_benchmark",
        format!("Matrix {} on {}", result.size, result.adapter),
        format!(
            "size={} adapter={} path={} cpu_est={} intensity={}",
            result.size, result.adapter, result.gpu_path, result.cpu_estimated, result.gpu_intensity
        ),
    );
    event.summary = format!(
        "CPU {}, GPU total {}, speedup {}",
        format_cpu_ms(result),
        format_ms(Some(result.gpu_total_ms)),
        format_speedup(result.speedup)
    );
    event.metrics.push(history_metric(
        "CPU time",
        result.cpu_ms,
        format_cpu_ms(result),
        "ms",
        HISTORY_BETTER_LOWER,
    ));
    if let Some(value) = result.gpu_compute_ms {
        event.metrics.push(history_metric(
            "GPU compute time",
            value,
            format_ms(Some(value)),
            "ms",
            HISTORY_BETTER_LOWER,
        ));
    }
    event.metrics.push(history_metric(
        "GPU total time",
        result.gpu_total_ms,
        format_ms(Some(result.gpu_total_ms)),
        "ms",
        HISTORY_BETTER_LOWER,
    ));
    event.metrics.push(history_metric(
        "Speedup",
        result.speedup,
        format_speedup(result.speedup),
        "x",
        HISTORY_BETTER_HIGHER,
    ));
    event.details.push(history_pair("Adapter", &result.adapter));
    event.details.push(history_pair("CPU", &result.cpu_model));
    event.details.push(history_pair("GPU path", result.gpu_path.to_string()));
    event.details.push(history_pair("Validation", &result.validation));
    event.details.push(history_pair(
        "CPU temperature",
        format_temperature_summary(&result.cpu_temperature),
    ));
    event.details.push(history_pair(
        "GPU temperature",
        format_temperature_summary(&result.gpu_temperature),
    ));
    event
}

fn history_event_from_repeat_progress(progress: &RepeatProgress) -> HistoryEvent {
    let mut event = history_base_event(
        "matrix_stress",
        format!("{} stress {}x{}", progress.mode, progress.size, progress.size),
        format!(
            "mode={} size={} duration={:?}",
            progress.mode, progress.size, progress.duration_s
        ),
    );
    event.summary = format!(
        "{} iteration(s), {}, avg {}",
        progress.iterations,
        format_stress_rate_per_min(progress.iterations, progress.elapsed_s),
        format_ms(Some(progress.average_total_ms))
    );
    event.metrics.push(history_metric(
        "Iterations",
        progress.iterations as f64,
        progress.iterations.to_string(),
        "count",
        HISTORY_BETTER_HIGHER,
    ));
    event.metrics.push(history_metric(
        "Average total time",
        progress.average_total_ms,
        format_ms(Some(progress.average_total_ms)),
        "ms",
        HISTORY_BETTER_LOWER,
    ));
    if let Some(value) = progress.average_compute_ms {
        event.metrics.push(history_metric(
            "Average compute time",
            value,
            format_ms(Some(value)),
            "ms",
            HISTORY_BETTER_LOWER,
        ));
    }
    if let Some(tflops) = progress.throughput_tflops() {
        event.metrics.push(history_metric(
            "Throughput",
            tflops,
            format!("{tflops:.2}"),
            "TFLOP/s",
            HISTORY_BETTER_HIGHER,
        ));
    }
    if progress.canceled {
        event.warnings.push("Stress test was canceled.".to_owned());
    }
    event
}

fn history_event_from_drive_result(result: &DriveBenchmarkResult, drive_label: &str) -> HistoryEvent {
    let mut event = history_base_event(
        "drive_benchmark",
        format!("{} on {}", result.test.label(), drive_label),
        format!(
            "drive={} test={} mode={} file={}",
            drive_label,
            result.test.label(),
            result.io_mode.label(),
            result.file_size_bytes
        ),
    );
    event.summary = format!(
        "{} MB/s, {}, p95 {}, {}",
        format_drive_speed(result),
        format_optional_iops(result.iops),
        format_optional_latency(result.p95_latency_ms),
        result.io_mode.label()
    );
    let speed = if result.test.is_read() {
        result.read_mbps
    } else {
        result.write_mbps
    };
    if let Some(value) = speed {
        event.metrics.push(history_metric(
            "Throughput",
            value,
            format_drive_speed(result),
            "MB/s",
            HISTORY_BETTER_HIGHER,
        ));
    }
    if let Some(value) = result.iops {
        event.metrics.push(history_metric(
            "IOPS",
            value,
            format_optional_iops(Some(value)),
            "iops",
            HISTORY_BETTER_HIGHER,
        ));
    }
    if let Some(value) = result.avg_latency_ms {
        event.metrics.push(history_metric(
            "Average latency",
            value,
            format_optional_latency(Some(value)),
            "ms",
            HISTORY_BETTER_LOWER,
        ));
    }
    if let Some(value) = result.p95_latency_ms {
        event.metrics.push(history_metric(
            "P95 latency",
            value,
            format_optional_latency(Some(value)),
            "ms",
            HISTORY_BETTER_LOWER,
        ));
    }
    event.details.push(history_pair("Drive", drive_label));
    event.details.push(history_pair("I/O mode", result.io_mode.label()));
    event.details.push(history_pair("File size", format_bytes(result.file_size_bytes)));
    event.details.push(history_pair(
        "SSD temperature",
        format_temperature_summary(&result.ssd_temperature),
    ));
    event.warnings.extend(result.notes.clone());
    event
}

fn history_event_from_gpu_memory_result(result: &GpuMemoryBenchmarkResult) -> HistoryEvent {
    let mut event = history_base_event(
        "gpu_memory_benchmark",
        format!("{} on {}", result.test.label(), result.adapter),
        format!(
            "adapter={} test={} buffer={} iterations={}",
            result.adapter,
            result.test.label(),
            result.buffer_size_bytes,
            result.iterations
        ),
    );
    event.summary = format!(
        "Best {} GB/s, average {} GB/s",
        format_gpu_memory_bandwidth(result.best_bandwidth_gbps),
        format_gpu_memory_bandwidth(result.average_bandwidth_gbps)
    );
    event.metrics.push(history_metric(
        "Best bandwidth",
        result.best_bandwidth_gbps,
        format_gpu_memory_bandwidth(result.best_bandwidth_gbps),
        "GB/s",
        HISTORY_BETTER_HIGHER,
    ));
    event.metrics.push(history_metric(
        "Average bandwidth",
        result.average_bandwidth_gbps,
        format_gpu_memory_bandwidth(result.average_bandwidth_gbps),
        "GB/s",
        HISTORY_BETTER_HIGHER,
    ));
    event.metrics.push(history_metric(
        "Elapsed",
        result.elapsed_ms,
        format_ms(Some(result.elapsed_ms)),
        "ms",
        HISTORY_BETTER_LOWER,
    ));
    event.details.push(history_pair("Adapter", &result.adapter));
    event.details.push(history_pair("Buffer", format_bytes(result.buffer_size_bytes)));
    event.details.push(history_pair("Timing", result.timing_source.label()));
    event.details.push(history_pair("Validation", &result.validation));
    event.details.push(history_pair(
        "GPU temperature",
        format_temperature_summary(&result.gpu_temperature),
    ));
    event.warnings.extend(result.notes.clone());
    event
}

fn history_event_from_ai_training_result(result: &AiTrainingResult) -> HistoryEvent {
    let mut event = history_base_event(
        "ai_training_benchmark",
        format!("{} {} {}", result.backend, result.workload, result.precision),
        format!(
            "backend={} workload={} preset={} precision={} shape={}",
            result.backend, result.workload, result.preset, result.precision, result.shape
        ),
    );
    event.summary = result
        .throughput_value
        .map(|value| format!("{value:.1} {}", result.throughput_label))
        .unwrap_or_else(|| "AI training benchmark complete".to_owned());
    if let Some(value) = result.throughput_value {
        event.metrics.push(history_metric(
            "Throughput",
            value,
            format!("{value:.1}"),
            result.throughput_label,
            HISTORY_BETTER_HIGHER,
        ));
    }
    if let Some(value) = result.avg_step_ms {
        event.metrics.push(history_metric(
            "Average step",
            value,
            format_ms(Some(value)),
            "ms",
            HISTORY_BETTER_LOWER,
        ));
    }
    if let Some(value) = result.p95_step_ms {
        event.metrics.push(history_metric(
            "P95 step",
            value,
            format_ms(Some(value)),
            "ms",
            HISTORY_BETTER_LOWER,
        ));
    }
    if let Some(value) = result.compute_tflops {
        event.metrics.push(history_metric(
            "Compute",
            value,
            format!("{value:.2}"),
            "TFLOP/s",
            HISTORY_BETTER_HIGHER,
        ));
    }
    event.details.push(history_pair("GPU", result.gpu_names.join(", ")));
    event.details.push(history_pair("Shape", &result.shape));
    event.details.push(history_pair("Memory", format_bytes(result.memory_bytes)));
    event.details.push(history_pair("Validation", &result.validation));
    if !result.notes.trim().is_empty() {
        event.warnings.push(result.notes.clone());
    }
    event
}

fn history_event_from_ram_result(result: &RamTestResult) -> HistoryEvent {
    let mut event = history_base_event(
        "ram_test",
        format!("RAM test {}", result.status.label()),
        format!(
            "installed={} tested={} phases={}",
            result.installed_bytes, result.tested_bytes, result.total_phases
        ),
    );
    event.summary = format!(
        "{} error(s), {} checks, {} tested",
        result.error_count,
        result.checks,
        format_bytes(result.tested_bytes)
    );
    event.metrics.push(history_metric(
        "Errors",
        result.error_count as f64,
        result.error_count.to_string(),
        "count",
        HISTORY_BETTER_ZERO,
    ));
    event.metrics.push(history_metric(
        "Checks",
        result.checks as f64,
        result.checks.to_string(),
        "count",
        HISTORY_BETTER_HIGHER,
    ));
    event.metrics.push(history_metric(
        "Elapsed",
        result.elapsed_ms,
        format_ms(Some(result.elapsed_ms)),
        "ms",
        HISTORY_BETTER_LOWER,
    ));
    event.details.push(history_pair("Status", result.status.label()));
    event.details.push(history_pair("Tested", format_bytes(result.tested_bytes)));
    event.details.push(history_pair("Installed", format_bytes(result.installed_bytes)));
    event.details.push(history_pair(
        "Available at start",
        format_bytes(result.available_at_start_bytes),
    ));
    if let Some(failure) = &result.first_failure {
        event
            .warnings
            .push(format!("First failure: {}", format_ram_failure(failure)));
    }
    event.warnings.extend(result.notes.clone());
    event
}

fn history_event_from_battery_report(report: &BatteryReport) -> HistoryEvent {
    let primary = report.primary_battery();
    let manufacturer = primary
        .and_then(|battery| battery.manufacturer.as_deref())
        .unwrap_or("Unknown battery");
    let mut event = history_base_event(
        "battery_diagnostic",
        format!("Battery diagnostic {manufacturer}"),
        format!(
            "battery={} design={:?}",
            manufacturer,
            primary.and_then(|battery| battery.design_capacity_mwh)
        ),
    );
    let health = battery_health_percent(primary);
    let wear = battery_wear_percent(primary);
    event.summary = format!(
        "Health {}, wear {}, {} warning(s)",
        format_optional_percent(health.map(|value| value as f32)),
        format_optional_percent(wear.map(|value| value as f32)),
        report.warnings.len()
    );
    if let Some(value) = health {
        event.metrics.push(history_metric(
            "Battery health",
            f64::from(value),
            format!("{value:.1}%"),
            "%",
            HISTORY_BETTER_HIGHER,
        ));
    }
    if let Some(value) = wear {
        event.metrics.push(history_metric(
            "Battery wear",
            f64::from(value),
            format!("{value:.1}%"),
            "%",
            HISTORY_BETTER_LOWER,
        ));
    }
    if let Some(cycles) = primary.and_then(|battery| battery.cycle_count) {
        event.metrics.push(history_metric(
            "Cycle count",
            cycles as f64,
            cycles.to_string(),
            "cycles",
            HISTORY_BETTER_LOWER,
        ));
    }
    event.details.push(history_pair("Generated", report.generated_at.clone().unwrap_or_else(|| "N/A".to_owned())));
    event.details.push(history_pair("Batteries", report.batteries.len().to_string()));
    for warning in &report.warnings {
        event
            .warnings
            .push(format!("{}: {}", battery_warning_severity_label(warning.severity), warning.title));
    }
    event.warnings.extend(report.notes.clone());
    event
}

fn history_event_from_network_state(state: &NetworkDiagnosticState) -> HistoryEvent {
    let adapter_label = state.selected_adapter_label();
    let mut event = history_base_event(
        "network_diagnostic",
        format!("Network diagnostic {adapter_label}"),
        format!("adapter={adapter_label}"),
    );
    event.summary = format!(
        "{} finding(s), {} probe result(s)",
        state.findings.len(),
        state.probe_results.len()
    );
    if let Some(adapter) = state.selected_adapter() {
        event.details.push(history_pair("Adapter", &adapter.name));
        event.details.push(history_pair("Type", adapter.kind.label()));
        event.details.push(history_pair("Connected", adapter.connected.to_string()));
        event.details.push(history_pair(
            "Link speed",
            format_link_speed(adapter.link_speed_bps),
        ));
        if let Some(wifi) = &adapter.wifi {
            if let Some(signal) = wifi.signal_quality_percent {
                event.metrics.push(history_metric(
                    "Wi-Fi signal",
                    signal as f64,
                    format!("{signal}%"),
                    "%",
                    HISTORY_BETTER_HIGHER,
                ));
            }
        }
    }
    let avg_loss = average_network_loss(&state.probe_results);
    if let Some(value) = avg_loss {
        event.metrics.push(history_metric(
            "Packet loss",
            value as f64,
            format_loss_percent(value),
            "%",
            HISTORY_BETTER_LOWER,
        ));
    }
    let avg_latency = average_network_latency(&state.probe_results);
    if let Some(value) = avg_latency {
        event.metrics.push(history_metric(
            "Average latency",
            value,
            format_optional_latency(Some(value)),
            "ms",
            HISTORY_BETTER_LOWER,
        ));
    }
    if let Some(speed) = &state.speed_result {
        if let Some(value) = speed.download_mbps {
            event.metrics.push(history_metric(
                "Download speed",
                value,
                format_network_speed_mbps(Some(value)),
                "Mbps",
                HISTORY_BETTER_HIGHER,
            ));
        }
        if let Some(value) = speed.upload_mbps {
            event.metrics.push(history_metric(
                "Upload speed",
                value,
                format_network_speed_mbps(Some(value)),
                "Mbps",
                HISTORY_BETTER_HIGHER,
            ));
        }
    }
    for finding in &state.findings {
        event.warnings.push(format!(
            "{}: {}",
            finding.severity.label(),
            finding.title
        ));
    }
    event
}

fn history_event_from_storage_health_state(state: &StorageHealthState) -> Option<HistoryEvent> {
    let snapshot = state.snapshot.as_ref()?;
    let mut event = history_base_event(
        "storage_health",
        format!("Storage health {}", snapshot.drive_label),
        format!(
            "drive={} model={} firmware={}",
            snapshot.drive_label,
            snapshot.model,
            snapshot.firmware.as_deref().unwrap_or("unknown")
        ),
    );
    event.summary = format!(
        "{} status, {}, {} warning(s)",
        snapshot.status,
        format_storage_health_percent(snapshot.health_percent),
        snapshot.warnings.len()
    );
    if let Some(value) = snapshot.health_percent {
        event.metrics.push(history_metric(
            "Health",
            value as f64,
            format!("{value:.0}%"),
            "%",
            HISTORY_BETTER_HIGHER,
        ));
    }
    if let Some(value) = snapshot.temperature_c {
        event.metrics.push(history_metric(
            "Temperature",
            value as f64,
            format_temperature_value(Some(value)),
            "C",
            HISTORY_BETTER_LOWER,
        ));
    }
    if let Some(value) = snapshot.media_errors {
        event.metrics.push(history_metric(
            "Media errors",
            value as f64,
            value.to_string(),
            "count",
            HISTORY_BETTER_ZERO,
        ));
    }
    if let Some(value) = snapshot.unsafe_shutdowns {
        event.metrics.push(history_metric(
            "Unsafe shutdowns",
            value as f64,
            value.to_string(),
            "count",
            HISTORY_BETTER_LOWER,
        ));
    }
    event.details.push(history_pair("Drive", &snapshot.drive_label));
    event.details.push(history_pair("Model", &snapshot.model));
    event.details.push(history_pair("Firmware", history_option(snapshot.firmware.as_deref())));
    event.details.push(history_pair("Bus", &snapshot.bus_type));
    event.details.push(history_pair("Media", &snapshot.media_type));
    for warning in &snapshot.warnings {
        event
            .warnings
            .push(format!("{}: {}", warning.severity.label(), warning.title));
    }
    event.warnings.extend(snapshot.provider_notes.clone());
    Some(event)
}

fn history_event_from_device_info(snapshot: &DeviceInfoSnapshot) -> HistoryEvent {
    let system_label = snapshot
        .system
        .as_ref()
        .and_then(|system| system.model.as_deref().or(system.manufacturer.as_deref()))
        .unwrap_or("Device");
    let mut event = history_base_event(
        "device_info",
        format!("Device inventory {system_label}"),
        format!(
            "system={} bios={}",
            system_label,
            snapshot
                .bios
                .as_ref()
                .and_then(|bios| bios.version.as_deref())
                .unwrap_or("unknown")
        ),
    );
    event.summary = format!(
        "{} CPU(s), {}, {} disk(s), {} GPU(s), {} driver record(s)",
        snapshot.cpus.len(),
        format_optional_bytes(snapshot.total_ram_bytes()),
        snapshot.disks.len(),
        snapshot.gpus.len().max(snapshot.wgpu_adapters.len()),
        snapshot.drivers.len()
    );
    if let Some(value) = snapshot.total_ram_bytes() {
        event.metrics.push(history_metric(
            "RAM",
            value as f64,
            format_bytes(value),
            "bytes",
            HISTORY_BETTER_NEUTRAL,
        ));
    }
    event.details.push(history_pair(
        "os.build",
        snapshot
            .os
            .as_ref()
            .and_then(|os| os.build_number.clone())
            .unwrap_or_else(|| "N/A".to_owned()),
    ));
    event.details.push(history_pair(
        "bios.version",
        snapshot
            .bios
            .as_ref()
            .and_then(|bios| bios.version.clone())
            .unwrap_or_else(|| "N/A".to_owned()),
    ));
    event.details.push(history_pair(
        "bios.date",
        snapshot
            .bios
            .as_ref()
            .and_then(|bios| bios.release_date.clone())
            .unwrap_or_else(|| "N/A".to_owned()),
    ));
    event.details.push(history_pair(
        "board.product",
        snapshot
            .baseboard
            .as_ref()
            .and_then(|board| board.product.clone())
            .unwrap_or_else(|| "N/A".to_owned()),
    ));
    event.details.push(history_pair(
        "cpu.summary",
        snapshot
            .cpus
            .iter()
            .map(|cpu| cpu.name.clone())
            .collect::<Vec<_>>()
            .join("; "),
    ));
    event.details.push(history_pair(
        "ram.bytes",
        snapshot
            .total_ram_bytes()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "N/A".to_owned()),
    ));
    for gpu in &snapshot.gpus {
        event.details.push(history_pair(
            format!("gpu.driver.{}", gpu.name),
            format!(
                "{} {} ({})",
                history_option(gpu.driver_provider.as_deref()),
                history_option(gpu.driver_version.as_deref()),
                history_option(gpu.driver_date.as_deref())
            ),
        ));
    }
    for adapter in &snapshot.network_adapters {
        event.details.push(history_pair(
            format!("network.driver.{}", adapter.name),
            format!(
                "{} {} ({})",
                history_option(adapter.driver_provider.as_deref()),
                history_option(adapter.driver_version.as_deref()),
                history_option(adapter.driver_date.as_deref())
            ),
        ));
    }
    for disk in &snapshot.disks {
        event.details.push(history_pair(
            format!("storage.{}", disk.model),
            format!(
                "{} {} {}",
                history_option(disk.bus_type.as_deref().or(disk.interface_type.as_deref())),
                history_option(disk.firmware.as_deref()),
                history_option(disk.health_status.as_deref().or(disk.status.as_deref()))
            ),
        ));
    }
    event.warnings.extend(snapshot.provider_notes.clone());
    event
}

fn history_event_from_sensor_snapshot(snapshot: &SensorSnapshot) -> HistoryEvent {
    let mut event = history_base_event(
        "sensor_snapshot",
        "Sensor provider snapshot",
        "current-sensors",
    );
    let readings = [
        ("CPU", snapshot.cpu.as_ref()),
        ("GPU", snapshot.gpu.as_ref()),
        ("VRAM", snapshot.gpu_memory.as_ref()),
        ("Drive", snapshot.drive.as_ref()),
        ("RAM", snapshot.memory.as_ref()),
    ];
    event.summary = readings
        .iter()
        .filter_map(|(label, reading)| reading.map(|reading| format!("{label}: {}", reading.status.detail())))
        .collect::<Vec<_>>()
        .join(", ");
    for (label, reading) in readings {
        let Some(reading) = reading else {
            event.warnings.push(format!("{label}: unavailable"));
            continue;
        };
        if let Some(value) = reading.temperature_c {
            event.metrics.push(history_metric(
                format!("{label} temperature"),
                value as f64,
                format_temperature_value(Some(value)),
                "C",
                HISTORY_BETTER_LOWER,
            ));
        }
        if let Some(value) = reading.utilization_percent {
            event.metrics.push(history_metric(
                format!("{label} utilization"),
                value as f64,
                format_utilization_value(Some(value)),
                "%",
                HISTORY_BETTER_NEUTRAL,
            ));
        }
        event.details.push(history_pair(
            format!("{label} provider"),
            format!("{} ({})", reading.provider, reading.status.detail()),
        ));
    }
    if let Some(elevated) = snapshot.helper_elevated {
        event.details.push(history_pair("Sensor helper elevated", elevated.to_string()));
    }
    event
}

fn history_event_from_timeline_summary(summary: &TimelineSummary) -> HistoryEvent {
    let mut event = history_base_event(
        "thermal_timeline",
        format!("{} thermal timeline", summary.scope.label()),
        format!("scope={} title={}", summary.scope.label(), summary.title),
    );
    event.summary = format!(
        "{} samples, confidence {}, {}",
        summary.sample_count,
        summary.confidence,
        summary
            .throughput_drop_percent
            .map(|drop| format!("{drop:.1}% throughput drop"))
            .unwrap_or_else(|| "no throughput drop estimate".to_owned())
    );
    event.metrics.push(history_metric(
        "Samples",
        summary.sample_count as f64,
        summary.sample_count.to_string(),
        "count",
        HISTORY_BETTER_NEUTRAL,
    ));
    if let Some(drop) = summary.throughput_drop_percent {
        event.metrics.push(history_metric(
            "Throughput drop",
            drop,
            format!("{drop:.1}%"),
            "%",
            HISTORY_BETTER_LOWER,
        ));
    }
    for (name, value) in [
        ("Peak CPU temp", summary.peak_cpu_temp_c),
        ("Peak GPU temp", summary.peak_gpu_temp_c),
        ("Peak VRAM temp", summary.peak_gpu_memory_temp_c),
        ("Peak SSD temp", summary.peak_drive_temp_c),
        ("Peak RAM temp", summary.peak_memory_temp_c),
    ] {
        if let Some(value) = value {
            event.metrics.push(history_metric(
                name,
                f64::from(value),
                format_temperature_value(Some(value)),
                "C",
                HISTORY_BETTER_LOWER,
            ));
        }
    }
    event.details.push(history_pair("Run ID", &summary.run_id));
    event.details.push(history_pair("Scope", summary.scope.label()));
    event.details.push(history_pair(
        "Duration",
        format_elapsed(summary.duration_ms as f64 / 1000.0),
    ));
    event.details.push(history_pair("Confidence", &summary.confidence));
    if let Some(throughput) = &summary.final_throughput {
        event.details.push(history_pair(
            "Final throughput",
            format!("{:.2} {}", throughput.value, throughput.unit),
        ));
    }
    for finding in &summary.findings {
        event
            .warnings
            .push(format!("{}: {}", finding.severity, finding.message));
    }
    event
}

fn history_app_environment_event(
    adapters: &[AdapterInfo],
    cpu_info: &CpuInfo,
    sensors: &SensorSnapshot,
    history_root: &PathBuf,
) -> HistoryEvent {
    let mut event = history_base_event("app_environment", "BenchScope app environment", "app");
    event.summary = format!(
        "BenchScope {}, CPU {}, {} adapter(s)",
        env!("CARGO_PKG_VERSION"),
        cpu_info.label(),
        adapters.len()
    );
    event.details.push(history_pair("BenchScope version", env!("CARGO_PKG_VERSION")));
    event.details.push(history_pair("CPU", cpu_info.label()));
    event.details.push(history_pair("GPU adapters", adapters.len().to_string()));
    event.details.push(history_pair("History root", history_root.display().to_string()));
    for adapter in adapters {
        event.details.push(history_pair(
            format!("adapter.{}", adapter.name),
            format!(
                "vendor {:04X} device {:04X} {} {}",
                adapter.vendor,
                adapter.device,
                device_type_label(adapter.device_type),
                empty_to_unknown(&adapter.driver)
            ),
        ));
    }
    event.warnings.extend(history_event_from_sensor_snapshot(sensors).warnings);
    event
}

fn history_option(value: Option<&str>) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("N/A")
        .to_owned()
}

fn compare_history_events(baseline: &HistoryEvent, current: &HistoryEvent) -> HistoryComparison {
    let mut deltas = Vec::new();
    let mut notes = Vec::new();
    if baseline.category != current.category || baseline.profile_key != current.profile_key {
        notes.push("Events are not strictly comparable because category or profile differs.".to_owned());
    }
    for current_metric in &current.metrics {
        let Some(baseline_metric) = baseline.metrics.iter().find(|metric| {
            metric.name == current_metric.name && metric.unit == current_metric.unit
        }) else {
            continue;
        };
        let raw_delta = current_metric.value - baseline_metric.value;
        let percent_delta = if baseline_metric.value.abs() > f64::EPSILON {
            Some(raw_delta / baseline_metric.value * 100.0)
        } else {
            None
        };
        let direction = history_delta_direction(raw_delta, &current_metric.better);
        let severity = history_delta_severity(percent_delta, &direction, &current_metric.better);
        let delta = match percent_delta {
            Some(percent) => format!("{raw_delta:+.2} {} ({percent:+.1}%)", current_metric.unit),
            None => format!("{raw_delta:+.2} {}", current_metric.unit),
        };
        deltas.push(HistoryDelta {
            metric: current_metric.name.clone(),
            baseline: baseline_metric.display.clone(),
            current: current_metric.display.clone(),
            delta,
            direction,
            severity,
        });
    }
    if deltas.is_empty() {
        notes.push("No matching numeric metrics were available for this comparison.".to_owned());
    }
    HistoryComparison {
        category: current.category.clone(),
        profile_key: current.profile_key.clone(),
        baseline_title: baseline.title.clone(),
        current_title: current.title.clone(),
        deltas,
        notes,
    }
}

fn history_delta_direction(raw_delta: f64, better: &str) -> String {
    if raw_delta.abs() <= f64::EPSILON || better == HISTORY_BETTER_NEUTRAL {
        return "neutral".to_owned();
    }
    match better {
        HISTORY_BETTER_HIGHER => {
            if raw_delta > 0.0 { "better" } else { "worse" }.to_owned()
        }
        HISTORY_BETTER_LOWER | HISTORY_BETTER_ZERO => {
            if raw_delta < 0.0 { "better" } else { "worse" }.to_owned()
        }
        _ => "neutral".to_owned(),
    }
}

fn history_delta_severity(percent_delta: Option<f64>, direction: &str, better: &str) -> String {
    if direction == "neutral" || better == HISTORY_BETTER_NEUTRAL {
        return "info".to_owned();
    }
    let magnitude = percent_delta.unwrap_or(0.0).abs();
    match direction {
        "worse" if magnitude >= 25.0 || better == HISTORY_BETTER_ZERO => "warning",
        "worse" if magnitude >= 10.0 => "caution",
        "better" if magnitude >= 10.0 => "better",
        _ => "info",
    }
    .to_owned()
}

fn diff_history_details(previous: &HistoryEvent, current: &HistoryEvent) -> Vec<HardwareChange> {
    let mut changes = Vec::new();
    for current_pair in &current.details {
        let Some(previous_pair) = previous
            .details
            .iter()
            .find(|pair| pair.key == current_pair.key)
        else {
            changes.push(HardwareChange {
                field: current_pair.key.clone(),
                previous: "not present".to_owned(),
                current: current_pair.value.clone(),
            });
            continue;
        };
        if previous_pair.value != current_pair.value {
            changes.push(HardwareChange {
                field: current_pair.key.clone(),
                previous: previous_pair.value.clone(),
                current: current_pair.value.clone(),
            });
        }
    }
    changes
}

fn average_network_loss(probes: &[NetworkProbeResult]) -> Option<f32> {
    if probes.is_empty() {
        return None;
    }
    Some(probes.iter().map(|probe| probe.loss_percent).sum::<f32>() / probes.len() as f32)
}

fn average_network_latency(probes: &[NetworkProbeResult]) -> Option<f64> {
    let values = probes
        .iter()
        .filter_map(|probe| probe.avg_latency_ms)
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn battery_warning_severity_label(value: BatteryWarningSeverity) -> &'static str {
    match value {
        BatteryWarningSeverity::Info => "Info",
        BatteryWarningSeverity::Warning => "Warning",
        BatteryWarningSeverity::Critical => "Critical",
    }
}

fn render_history_comparisons_markdown(comparisons: &[HistoryComparison]) -> String {
    let mut report = String::new();
    report.push_str("# BenchScope History Comparisons\n\n");
    if comparisons.is_empty() {
        report.push_str("No comparable baselines are available yet.\n");
        return report;
    }
    for comparison in comparisons {
        report.push_str(&format!(
            "## {}: {} vs {}\n\n",
            markdown_escape(&comparison.category),
            markdown_escape(&comparison.current_title),
            markdown_escape(&comparison.baseline_title)
        ));
        report.push_str("| Metric | Baseline | Current | Delta | Direction | Severity |\n");
        report.push_str("| --- | ---: | ---: | ---: | --- | --- |\n");
        for delta in &comparison.deltas {
            report.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} |\n",
                markdown_escape(&delta.metric),
                markdown_escape(&delta.baseline),
                markdown_escape(&delta.current),
                markdown_escape(&delta.delta),
                markdown_escape(&delta.direction),
                markdown_escape(&delta.severity)
            ));
        }
        for note in &comparison.notes {
            report.push_str(&format!("- Note: {}\n", markdown_escape(note)));
        }
        report.push('\n');
    }
    report
}

fn render_hardware_changes_markdown(changes: &[HardwareChange]) -> String {
    let mut report = String::new();
    report.push_str("# BenchScope Hardware and Driver Changes\n\n");
    if changes.is_empty() {
        report.push_str("No hardware or driver changes were detected between the last two device-information snapshots.\n");
        return report;
    }
    report.push_str("| Field | Previous | Current |\n");
    report.push_str("| --- | --- | --- |\n");
    for change in changes {
        report.push_str(&format!(
            "| {} | {} | {} |\n",
            markdown_escape(&change.field),
            markdown_escape(&change.previous),
            markdown_escape(&change.current)
        ));
    }
    report
}

fn render_matrix_benchmark_report(results: &[BenchmarkResult]) -> String {
    let mut report = String::new();
    report.push_str("# BenchScope Matrix Benchmark Results\n\n");
    if results.is_empty() {
        report.push_str("No matrix benchmark results were recorded in this session.\n");
        return report;
    }
    report.push_str("| Size | Adapter | CPU | GPU total | GPU compute | Speedup | Path | Validation |\n");
    report.push_str("| ---: | --- | ---: | ---: | ---: | ---: | --- | --- |\n");
    for result in results {
        report.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            result.size,
            markdown_escape(&result.adapter),
            format_cpu_ms(result),
            format_ms(Some(result.gpu_total_ms)),
            format_ms(result.gpu_compute_ms),
            format_speedup(result.speedup),
            result.gpu_path,
            markdown_escape(&result.validation)
        ));
    }
    report
}

fn render_drive_benchmark_report(results: &[DriveBenchmarkResult], drive_label: &str) -> String {
    let mut report = String::new();
    report.push_str("# BenchScope Drive Benchmark Results\n\n");
    report.push_str(&format!("- Drive: {}\n\n", markdown_escape(drive_label)));
    if results.is_empty() {
        report.push_str("No drive benchmark results were recorded in this session.\n");
        return report;
    }
    report.push_str("| Test | MB/s | IOPS | Avg latency | P95 latency | Mode | File size | Notes |\n");
    report.push_str("| --- | ---: | ---: | ---: | ---: | --- | ---: | --- |\n");
    for result in results {
        report.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            result.test.label(),
            format_drive_speed(result),
            format_optional_iops(result.iops),
            format_optional_latency(result.avg_latency_ms),
            format_optional_latency(result.p95_latency_ms),
            result.io_mode.label(),
            format_bytes(result.file_size_bytes),
            markdown_escape(&result.notes.join(", "))
        ));
    }
    report
}

fn render_gpu_memory_benchmark_report(results: &[GpuMemoryBenchmarkResult]) -> String {
    let mut report = String::new();
    report.push_str("# BenchScope GPU Memory Bandwidth Results\n\n");
    if results.is_empty() {
        report.push_str("No GPU memory benchmark results were recorded in this session.\n");
        return report;
    }
    report.push_str("| Test | Adapter | Buffer | Iterations | Best GB/s | Avg GB/s | Timing | Validation |\n");
    report.push_str("| --- | --- | ---: | ---: | ---: | ---: | --- | --- |\n");
    for result in results {
        report.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            result.test.label(),
            markdown_escape(&result.adapter),
            format_bytes(result.buffer_size_bytes),
            result.iterations,
            format_gpu_memory_bandwidth(result.best_bandwidth_gbps),
            format_gpu_memory_bandwidth(result.average_bandwidth_gbps),
            result.timing_source.label(),
            markdown_escape(&result.validation)
        ));
    }
    report
}

fn render_ai_training_benchmark_report(results: &[AiTrainingResult]) -> String {
    let mut report = String::new();
    report.push_str("# BenchScope AI Training Benchmark Results\n\n");
    if results.is_empty() {
        report.push_str("No AI training benchmark results were recorded in this session.\n");
        return report;
    }
    report.push_str("| Backend | Workload | Precision | GPU | Shape | Throughput | Avg step | P95 step | Memory | Validation |\n");
    report.push_str("| --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | --- |\n");
    for result in results {
        let throughput = result
            .throughput_value
            .map(|value| format!("{value:.1} {}", result.throughput_label))
            .unwrap_or_else(|| "N/A".to_owned());
        report.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            result.backend,
            result.workload,
            result.precision,
            markdown_escape(&result.gpu_names.join(", ")),
            markdown_escape(&result.shape),
            throughput,
            format_ms(result.avg_step_ms),
            format_ms(result.p95_step_ms),
            format_bytes(result.memory_bytes),
            markdown_escape(&result.validation)
        ));
    }
    report
}

fn render_ram_test_report(results: &[RamTestResult]) -> String {
    let mut report = String::new();
    report.push_str("# BenchScope RAM Test Results\n\n");
    if results.is_empty() {
        report.push_str("No RAM test results were recorded in this session.\n");
        return report;
    }
    report.push_str("| Status | Tested | Installed | Checks | Errors | Phases | Elapsed | Notes |\n");
    report.push_str("| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |\n");
    for result in results {
        report.push_str(&format!(
            "| {} | {} | {} | {} | {} | {}/{} | {} | {} |\n",
            result.status.label(),
            format_bytes(result.tested_bytes),
            format_bytes(result.installed_bytes),
            result.checks,
            result.error_count,
            result.completed_phases,
            result.total_phases,
            format_ms(Some(result.elapsed_ms)),
            markdown_escape(&result.notes.join(", "))
        ));
    }
    report
}

fn render_battery_diagnostic_report(report: Option<&BatteryReport>) -> String {
    let mut output = String::new();
    output.push_str("# BenchScope Battery Diagnostic Report\n\n");
    let Some(report) = report else {
        output.push_str("No battery diagnostic scan has completed in this session.\n");
        return output;
    };
    output.push_str(&format!(
        "- Generated: {}\n",
        markdown_escape(&report.generated_at.clone().unwrap_or_else(|| "N/A".to_owned()))
    ));
    if let Some(battery) = report.primary_battery() {
        output.push_str(&format!(
            "- Manufacturer: {}\n",
            markdown_escape(&history_option(battery.manufacturer.as_deref()))
        ));
        output.push_str(&format!(
            "- Chemistry: {}\n",
            markdown_escape(&history_option(battery.chemistry.as_deref()))
        ));
        output.push_str(&format!(
            "- Design capacity: {}\n",
            format_optional_energy_mwh(battery.design_capacity_mwh)
        ));
        output.push_str(&format!(
            "- Full charge capacity: {}\n",
            format_optional_energy_mwh(battery.full_charge_capacity_mwh)
        ));
        output.push_str(&format!(
            "- Cycle count: {}\n",
            battery
                .cycle_count
                .map(|value| value.to_string())
                .unwrap_or_else(|| "N/A".to_owned())
        ));
    }
    output.push_str("\n## Warnings\n\n");
    if report.warnings.is_empty() {
        output.push_str("No battery warnings were reported.\n");
    } else {
        for warning in &report.warnings {
            output.push_str(&format!(
                "- **{}**: {} - {}\n",
                battery_warning_severity_label(warning.severity),
                markdown_escape(&warning.title),
                markdown_escape(&warning.detail)
            ));
        }
    }
    output.push_str("\n## Capacity History\n\n");
    if report.capacity_history.is_empty() {
        output.push_str("No capacity history was available.\n");
    } else {
        output.push_str("| Label | Design | Full charge | Cycles |\n");
        output.push_str("| --- | ---: | ---: | ---: |\n");
        for point in &report.capacity_history {
            output.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                markdown_escape(&point.label),
                format_optional_energy_mwh(point.design_capacity_mwh),
                format_optional_energy_mwh(point.full_charge_capacity_mwh),
                point
                    .cycle_count
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "N/A".to_owned())
            ));
        }
    }
    output
}

fn render_sensor_provider_report(snapshot: &SensorSnapshot) -> String {
    let mut report = String::new();
    report.push_str("# BenchScope Sensor Provider Status\n\n");
    report.push_str("| Device | Label | Temperature | Utilization | Provider | Status |\n");
    report.push_str("| --- | --- | ---: | ---: | --- | --- |\n");
    for (label, reading) in [
        ("CPU", snapshot.cpu.as_ref()),
        ("GPU", snapshot.gpu.as_ref()),
        ("VRAM", snapshot.gpu_memory.as_ref()),
        ("SSD", snapshot.drive.as_ref()),
        ("RAM", snapshot.memory.as_ref()),
    ] {
        if let Some(reading) = reading {
            report.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} |\n",
                label,
                markdown_escape(&reading.label),
                format_temperature_value(reading.temperature_c),
                format_utilization_value(reading.utilization_percent),
                markdown_escape(&reading.provider),
                markdown_escape(&reading.status.detail())
            ));
        } else {
            report.push_str(&format!("| {label} | N/A | N/A | N/A | N/A | unavailable |\n"));
        }
    }
    if let Some(elevated) = snapshot.helper_elevated {
        report.push_str(&format!("\n- Sensor helper elevated: {elevated}\n"));
    }
    report
}

fn render_history_summary_report(
    events: &[HistoryEvent],
    comparisons: &[HistoryComparison],
    changes: &[HardwareChange],
) -> String {
    let mut report = String::new();
    report.push_str("# BenchScope Support Summary\n\n");
    report.push_str(&format!("- Generated: {}\n", history_now_unix_seconds()));
    report.push_str(&format!("- History events included: {}\n\n", events.len()));
    report.push_str("## Latest Runs\n\n");
    for category in history_categories() {
        if category.id == "all" {
            continue;
        }
        if let Some(event) = events.iter().rev().find(|event| event.category == category.id) {
            report.push_str(&format!(
                "- **{}**: {} - {}\n",
                category.label,
                markdown_escape(&event.title),
                markdown_escape(&event.summary)
            ));
        }
    }
    report.push_str("\n## Notable Deltas\n\n");
    if comparisons.is_empty() {
        report.push_str("No comparable baseline deltas are available yet.\n");
    } else {
        for comparison in comparisons {
            for delta in &comparison.deltas {
                if delta.severity != "info" {
                    report.push_str(&format!(
                        "- {} / {}: {} ({})\n",
                        markdown_escape(&comparison.category),
                        markdown_escape(&delta.metric),
                        markdown_escape(&delta.delta),
                        markdown_escape(&delta.severity)
                    ));
                }
            }
        }
    }
    report.push_str("\n## Hardware and Driver Changes\n\n");
    if changes.is_empty() {
        report.push_str("No hardware or driver changes were detected between the last two device inventory snapshots.\n");
    } else {
        for change in changes {
            report.push_str(&format!(
                "- {} changed from `{}` to `{}`\n",
                markdown_escape(&change.field),
                markdown_escape(&change.previous),
                markdown_escape(&change.current)
            ));
        }
    }
    report.push_str("\nThis bundle is a local diagnostic export. Redacted fields are intentionally omitted for privacy.\n");
    report
}

fn export_support_bundle(
    history: &mut HistoryState,
    mut reports: Vec<(String, String)>,
    session_log: &[String],
) -> Result<PathBuf> {
    fs::create_dir_all(&history.bundles_dir)
        .with_context(|| format!("creating {}", history.bundles_dir.display()))?;
    fs::create_dir_all(&history.logs_dir)
        .with_context(|| format!("creating {}", history.logs_dir.display()))?;
    let timestamp = history_now_unix_seconds();
    let folder_name = format!("benchscope-support-{timestamp}");
    let folder = history.bundles_dir.join(&folder_name);
    if folder.exists() {
        fs::remove_dir_all(&folder).with_context(|| format!("clearing {}", folder.display()))?;
    }
    fs::create_dir_all(folder.join("reports"))?;
    fs::create_dir_all(folder.join("history"))?;
    fs::create_dir_all(folder.join("logs"))?;

    let recent_events = history
        .events
        .iter()
        .rev()
        .take(HISTORY_BUNDLE_EVENT_LIMIT)
        .cloned()
        .collect::<Vec<_>>();
    let comparisons = history.comparisons_for_selected();
    let changes = history.hardware_changes();
    let summary = render_history_summary_report(&recent_events, &comparisons, &changes);
    reports.insert(0, ("summary.md".to_owned(), summary));
    reports.push((
        "history-comparisons.md".to_owned(),
        render_history_comparisons_markdown(&comparisons),
    ));
    reports.push((
        "hardware-driver-changes.md".to_owned(),
        render_hardware_changes_markdown(&changes),
    ));

    let mut report_names = Vec::new();
    for (name, content) in reports {
        let safe_name = sanitize_bundle_entry_name(&name);
        let path = folder.join("reports").join(&safe_name);
        let redacted = redact_markdown_report(&content, &history.redaction);
        fs::write(&path, redacted).with_context(|| format!("writing {}", path.display()))?;
        report_names.push(format!("reports/{safe_name}"));
    }

    let events_path = folder.join("history").join("recent-events.redacted.jsonl");
    let mut events_file = File::create(&events_path)
        .with_context(|| format!("creating {}", events_path.display()))?;
    for event in recent_events.iter().rev() {
        let json = serde_json::to_string(event)?;
        let redacted = redact_text(&json, &history.redaction);
        events_file.write_all(redacted.as_bytes())?;
        events_file.write_all(b"\n")?;
    }

    let baselines_path = folder.join("history").join("baselines.redacted.json");
    let baseline_json = serde_json::to_string_pretty(&history.baselines)?;
    fs::write(
        &baselines_path,
        redact_text(&baseline_json, &history.redaction),
    )?;

    let log_path = folder.join("logs").join("session.redacted.log");
    fs::write(
        &log_path,
        redact_text(&session_log.join("\n"), &history.redaction),
    )?;

    let manifest = serde_json::json!({
        "schemaVersion": HISTORY_SCHEMA_VERSION,
        "generatedAtUnixSeconds": timestamp,
        "benchScopeVersion": env!("CARGO_PKG_VERSION"),
        "redaction": {
            "includeSensitiveIds": history.redaction.include_sensitive_ids,
            "includeLocalPaths": history.redaction.include_local_paths,
            "includeNetworkAddresses": history.redaction.include_network_addresses,
            "includeWifiNames": history.redaction.include_wifi_names,
        },
        "eventCount": recent_events.len(),
        "reports": report_names,
    });
    fs::write(
        folder.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    fs::write(
        folder.join("README.md"),
        "# BenchScope Support Bundle\n\nOpen `reports/summary.md` first. The bundle uses privacy redaction by default.\n",
    )?;

    let session_log_path = history.logs_dir.join(format!("session-{timestamp}.log"));
    let _ = fs::write(
        &session_log_path,
        redact_text(&session_log.join("\n"), &history.redaction),
    );

    let zip_path = folder.with_extension("zip");
    compress_support_folder(&folder, &zip_path)?;
    history.last_bundle_path = Some(zip_path.clone());
    history.last_status = format!("Support bundle exported: {}", zip_path.display());
    history.last_error = None;
    Ok(zip_path)
}

fn sanitize_bundle_entry_name(name: &str) -> String {
    let mut output = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    if output.trim_matches('.').is_empty() {
        output = "report.md".to_owned();
    }
    output
}

fn compress_support_folder(folder: &PathBuf, zip_path: &PathBuf) -> Result<()> {
    if zip_path.exists() {
        fs::remove_file(zip_path).with_context(|| format!("removing {}", zip_path.display()))?;
    }
    #[cfg(windows)]
    {
        let script = format!(
            "Compress-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
            powershell_quote(&folder.display().to_string()),
            powershell_quote(&zip_path.display().to_string())
        );
        let output = Command::new("powershell")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script])
            .output()
            .with_context(|| "starting PowerShell Compress-Archive")?;
        if !output.status.success() {
            return Err(anyhow!(
                "Compress-Archive failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = zip_path;
        Err(anyhow!(
            "zip export currently requires Windows PowerShell; bundle folder is {}",
            folder.display()
        ))
    }
}

fn powershell_quote(value: &str) -> String {
    value.replace('\'', "''")
}

fn redact_markdown_report(text: &str, options: &RedactionOptions) -> String {
    let mut output = Vec::new();
    let mut table_headers: Option<Vec<String>> = None;
    for line in text.lines() {
        if line.starts_with('|') {
            let cells = markdown_table_cells(line);
            if cells.iter().any(|cell| !cell.chars().all(|ch| matches!(ch, '-' | ':' | ' '))) {
                if table_headers.is_none()
                    || cells
                        .iter()
                        .any(|cell| sensitive_header(cell, options))
                {
                    table_headers = Some(cells.clone());
                    output.push(redact_text(line, options));
                    continue;
                }
            }
            if let Some(headers) = &table_headers {
                if cells.len() == headers.len()
                    && !cells.iter().all(|cell| cell.chars().all(|ch| matches!(ch, '-' | ':' | ' ')))
                {
                    let redacted = cells
                        .iter()
                        .zip(headers.iter())
                        .map(|(cell, header)| {
                            if sensitive_header(header, options) {
                                "[redacted]".to_owned()
                            } else {
                                redact_text(cell, options)
                            }
                        })
                        .collect::<Vec<_>>();
                    output.push(format!("| {} |", redacted.join(" | ")));
                    continue;
                }
            }
        } else if line.trim().is_empty() {
            table_headers = None;
        }

        let lower = line.to_ascii_lowercase();
        if !options.include_sensitive_ids
            && (lower.contains("serial:")
                || lower.contains("processor id:")
                || lower.contains("device id:")
                || lower.contains("pnp device id:"))
        {
            let prefix = line.split_once(':').map(|(prefix, _)| prefix).unwrap_or(line);
            output.push(format!("{prefix}: [redacted]"));
        } else if !options.include_wifi_names && lower.contains("wi-fi ssid:") {
            let prefix = line.split_once(':').map(|(prefix, _)| prefix).unwrap_or(line);
            output.push(format!("{prefix}: [redacted]"));
        } else {
            output.push(redact_text(line, options));
        }
    }
    output.join("\n")
}

fn markdown_table_cells(line: &str) -> Vec<String> {
    line.trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect()
}

fn sensitive_header(header: &str, options: &RedactionOptions) -> bool {
    let header = header.to_ascii_lowercase();
    (!options.include_sensitive_ids
        && (header.contains("serial")
            || header.contains("device id")
            || header.contains("hardware id")
            || header.contains("mac")
            || header.contains("bssid")))
        || (!options.include_network_addresses
            && (header.contains("ip")
                || header.contains("ipv4")
                || header.contains("ipv6")
                || header.contains("dns")
                || header.contains("gateway")
                || header.contains("target")))
        || (!options.include_wifi_names && header.contains("ssid"))
        || (!options.include_local_paths && (header.contains("path") || header.contains("root")))
}

fn redact_text(text: &str, options: &RedactionOptions) -> String {
    let mut redacted = text.to_owned();
    if !options.include_local_paths {
        for key in ["USERPROFILE", "HOME"] {
            if let Some(value) = std::env::var_os(key).and_then(|value| value.into_string().ok()) {
                if !value.is_empty() {
                    redacted = redacted.replace(&value, "[local-path]");
                }
            }
        }
        if let Some(username) = std::env::var_os("USERNAME").and_then(|value| value.into_string().ok()) {
            if !username.is_empty() {
                redacted = redacted.replace(&username, "[user]");
            }
        }
    }
    if !options.include_network_addresses {
        redacted = redact_ipv4_tokens(&redacted);
        redacted = redact_mac_tokens(&redacted);
    }
    redacted
}

fn redact_ipv4_tokens(text: &str) -> String {
    text.split_whitespace()
        .map(|token| {
            let trimmed = token.trim_matches(|ch: char| matches!(ch, ',' | ';' | ')' | '(' | '[' | ']' | '"' | '\''));
            if is_ipv4_address(trimmed) {
                token.replace(trimmed, "[ip-address]")
            } else {
                token.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_ipv4_address(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() == 4
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= 3
                && part.chars().all(|ch| ch.is_ascii_digit())
                && part.parse::<u8>().is_ok()
        })
}

fn redact_mac_tokens(text: &str) -> String {
    text.split_whitespace()
        .map(|token| {
            let trimmed = token.trim_matches(|ch: char| matches!(ch, ',' | ';' | ')' | '(' | '[' | ']' | '"' | '\''));
            if is_mac_address(trimmed) {
                token.replace(trimmed, "[mac-address]")
            } else {
                token.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_mac_address(value: &str) -> bool {
    let separator = if value.contains('-') { '-' } else { ':' };
    let parts = value.split(separator).collect::<Vec<_>>();
    parts.len() == 6
        && parts
            .iter()
            .all(|part| part.len() == 2 && part.chars().all(|ch| ch.is_ascii_hexdigit()))
}
