const TIMELINE_SAMPLE_INTERVAL_MS: u64 = 1_000;
const TIMELINE_MAX_SAMPLES: usize = 3_600;
const TIMELINE_COMPLETED_LIMIT: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimelineScope {
    MatrixBenchmark,
    MatrixStress,
    GpuMemory,
    DriveBenchmark,
    AiTraining,
}

impl TimelineScope {
    fn label(self) -> &'static str {
        match self {
            Self::MatrixBenchmark => "Matrix benchmark",
            Self::MatrixStress => "Matrix stress",
            Self::GpuMemory => "GPU memory",
            Self::DriveBenchmark => "Drive benchmark",
            Self::AiTraining => "AI training",
        }
    }
}

#[derive(Clone, Debug)]
struct TimelineThroughputSample {
    label: String,
    value: f64,
    unit: String,
}

#[derive(Clone, Debug, Default)]
struct TimelineSensorSample {
    cpu_temp_c: Option<f32>,
    gpu_temp_c: Option<f32>,
    gpu_memory_temp_c: Option<f32>,
    drive_temp_c: Option<f32>,
    memory_temp_c: Option<f32>,
    cpu_util_percent: Option<f32>,
    gpu_util_percent: Option<f32>,
    drive_util_percent: Option<f32>,
    memory_util_percent: Option<f32>,
    cpu_clock_mhz: Option<f32>,
    gpu_clock_mhz: Option<f32>,
    cpu_power_w: Option<f32>,
    gpu_power_w: Option<f32>,
}

#[derive(Clone, Debug)]
struct TimelineSample {
    elapsed_ms: u64,
    sensor: TimelineSensorSample,
    throughput: Option<TimelineThroughputSample>,
    phase: String,
}

#[derive(Clone, Debug)]
struct RunTimeline {
    run_id: String,
    title: String,
    scope: TimelineScope,
    started_at: SystemTime,
    started_instant: Instant,
    last_sample_at: Option<Instant>,
    samples: Vec<TimelineSample>,
    max_samples: usize,
}

#[derive(Clone, Debug)]
struct TimelineFinding {
    severity: String,
    message: String,
}

#[derive(Clone, Debug)]
struct TimelineSummary {
    run_id: String,
    title: String,
    scope: TimelineScope,
    duration_ms: u64,
    sample_count: usize,
    peak_cpu_temp_c: Option<f32>,
    peak_gpu_temp_c: Option<f32>,
    peak_gpu_memory_temp_c: Option<f32>,
    peak_drive_temp_c: Option<f32>,
    peak_memory_temp_c: Option<f32>,
    first_throughput: Option<TimelineThroughputSample>,
    peak_throughput: Option<TimelineThroughputSample>,
    final_throughput: Option<TimelineThroughputSample>,
    throughput_drop_percent: Option<f64>,
    confidence: String,
    findings: Vec<TimelineFinding>,
}

#[derive(Clone, Debug)]
struct CompletedTimeline {
    timeline: RunTimeline,
    summary: TimelineSummary,
}

struct TimelineState {
    active: Option<RunTimeline>,
    completed: Vec<CompletedTimeline>,
    show_temperatures: bool,
    show_utilization: bool,
    show_cpu: bool,
    show_gpu: bool,
    show_vram: bool,
    show_drive: bool,
    show_memory: bool,
}

impl TimelineState {
    fn new() -> Self {
        Self {
            active: None,
            completed: Vec::new(),
            show_temperatures: true,
            show_utilization: true,
            show_cpu: true,
            show_gpu: true,
            show_vram: true,
            show_drive: true,
            show_memory: true,
        }
    }

    fn start(&mut self, scope: TimelineScope, title: impl Into<String>) {
        self.active = Some(RunTimeline {
            run_id: format!("{}-{}", timeline_now_unix_ms(), scope.label().replace(' ', "-")),
            title: title.into(),
            scope,
            started_at: SystemTime::now(),
            started_instant: Instant::now(),
            last_sample_at: None,
            samples: Vec::new(),
            max_samples: TIMELINE_MAX_SAMPLES,
        });
    }

    fn active_scope(&self) -> Option<TimelineScope> {
        self.active.as_ref().map(|timeline| timeline.scope)
    }

    fn observe(
        &mut self,
        sensor_snapshot: &SensorSnapshot,
        throughput: Option<TimelineThroughputSample>,
        phase: impl Into<String>,
        force: bool,
    ) {
        let Some(timeline) = &mut self.active else {
            return;
        };
        let now = Instant::now();
        if !force
            && timeline
                .last_sample_at
                .is_some_and(|last| now.duration_since(last).as_millis() < u128::from(TIMELINE_SAMPLE_INTERVAL_MS))
        {
            return;
        }
        timeline.last_sample_at = Some(now);
        let elapsed_ms = now.duration_since(timeline.started_instant).as_millis() as u64;
        timeline.samples.push(TimelineSample {
            elapsed_ms,
            sensor: timeline_sensor_sample(sensor_snapshot),
            throughput,
            phase: phase.into(),
        });
        downsample_timeline_samples(timeline);
    }

    fn finish(&mut self, status: &str) -> Option<TimelineSummary> {
        let mut timeline = self.active.take()?;
        if timeline.samples.is_empty() {
            timeline.samples.push(TimelineSample {
                elapsed_ms: timeline
                    .started_instant
                    .elapsed()
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
                sensor: TimelineSensorSample::default(),
                throughput: None,
                phase: status.to_owned(),
            });
        }
        if let Some(last) = timeline.samples.last_mut() {
            if last.phase.trim().is_empty() {
                last.phase = status.to_owned();
            }
        }
        let summary = analyze_timeline(&timeline);
        self.completed.push(CompletedTimeline {
            timeline,
            summary: summary.clone(),
        });
        while self.completed.len() > TIMELINE_COMPLETED_LIMIT {
            self.completed.remove(0);
        }
        Some(summary)
    }

    fn timeline_for_scope(&self, scope: TimelineScope) -> Option<&RunTimeline> {
        self.active
            .as_ref()
            .filter(|timeline| timeline.scope == scope)
            .or_else(|| {
                self.completed
                    .iter()
                    .rev()
                    .find(|record| record.timeline.scope == scope)
                    .map(|record| &record.timeline)
            })
    }

    fn summary_for_scope(&self, scope: TimelineScope) -> Option<TimelineSummary> {
        self.active
            .as_ref()
            .filter(|timeline| timeline.scope == scope)
            .map(analyze_timeline)
            .or_else(|| {
                self.completed
                    .iter()
                    .rev()
                    .find(|record| record.summary.scope == scope)
                    .map(|record| record.summary.clone())
            })
    }
}

fn timeline_now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn timeline_sensor_sample(snapshot: &SensorSnapshot) -> TimelineSensorSample {
    TimelineSensorSample {
        cpu_temp_c: snapshot.cpu.as_ref().and_then(|reading| reading.temperature_c),
        gpu_temp_c: snapshot.gpu.as_ref().and_then(|reading| reading.temperature_c),
        gpu_memory_temp_c: snapshot
            .gpu_memory
            .as_ref()
            .and_then(|reading| reading.temperature_c),
        drive_temp_c: snapshot.drive.as_ref().and_then(|reading| reading.temperature_c),
        memory_temp_c: snapshot.memory.as_ref().and_then(|reading| reading.temperature_c),
        cpu_util_percent: snapshot
            .cpu
            .as_ref()
            .and_then(|reading| reading.utilization_percent),
        gpu_util_percent: snapshot
            .gpu
            .as_ref()
            .and_then(|reading| reading.utilization_percent),
        drive_util_percent: snapshot
            .drive
            .as_ref()
            .and_then(|reading| reading.utilization_percent),
        memory_util_percent: snapshot
            .memory
            .as_ref()
            .and_then(|reading| reading.utilization_percent),
        cpu_clock_mhz: snapshot
            .cpu
            .as_ref()
            .and_then(|reading| first_timeline_metric(reading, SensorMetricKind::Clock)),
        gpu_clock_mhz: snapshot
            .gpu
            .as_ref()
            .and_then(|reading| first_timeline_metric(reading, SensorMetricKind::Clock)),
        cpu_power_w: snapshot
            .cpu
            .as_ref()
            .and_then(|reading| first_timeline_metric(reading, SensorMetricKind::Power)),
        gpu_power_w: snapshot
            .gpu
            .as_ref()
            .and_then(|reading| first_timeline_metric(reading, SensorMetricKind::Power)),
    }
}

fn first_timeline_metric(reading: &SensorReading, kind: SensorMetricKind) -> Option<f32> {
    reading.metrics_for(kind).next().and_then(|metric| metric.value)
}

fn downsample_timeline_samples(timeline: &mut RunTimeline) {
    if timeline.samples.len() <= timeline.max_samples {
        return;
    }
    if timeline.max_samples == 0 {
        timeline.samples.clear();
        return;
    }
    if timeline.max_samples == 1 {
        timeline.samples.truncate(1);
        return;
    }

    let first_keep = 300
        .min((timeline.max_samples / 4).max(1))
        .min(timeline.samples.len());
    let last_keep = 900
        .min((timeline.max_samples / 2).max(1))
        .min(timeline.samples.len().saturating_sub(first_keep))
        .min(timeline.max_samples.saturating_sub(first_keep));
    let middle_budget = timeline.max_samples.saturating_sub(first_keep + last_keep);
    let middle_start = first_keep;
    let middle_end = timeline.samples.len().saturating_sub(last_keep);
    let mut next = Vec::with_capacity(timeline.max_samples);
    next.extend_from_slice(&timeline.samples[..first_keep]);
    let middle_len = middle_end.saturating_sub(middle_start);
    if middle_budget > 0 && middle_len > 0 {
        if middle_len <= middle_budget {
            next.extend_from_slice(&timeline.samples[middle_start..middle_end]);
        } else {
            for slot in 0..middle_budget {
                let index = middle_start + (slot * middle_len / middle_budget);
                next.push(timeline.samples[index].clone());
            }
        }
    }
    if last_keep > 0 {
        for sample in &timeline.samples[timeline.samples.len() - last_keep..] {
            next.push(sample.clone());
        }
    }
    timeline.samples = next;
}

fn analyze_timeline(timeline: &RunTimeline) -> TimelineSummary {
    let peak_cpu_temp_c = peak_timeline_value(timeline.samples.iter().map(|sample| sample.sensor.cpu_temp_c));
    let peak_gpu_temp_c = peak_timeline_value(timeline.samples.iter().map(|sample| sample.sensor.gpu_temp_c));
    let peak_gpu_memory_temp_c = peak_timeline_value(
        timeline
            .samples
            .iter()
            .map(|sample| sample.sensor.gpu_memory_temp_c),
    );
    let peak_drive_temp_c =
        peak_timeline_value(timeline.samples.iter().map(|sample| sample.sensor.drive_temp_c));
    let peak_memory_temp_c =
        peak_timeline_value(timeline.samples.iter().map(|sample| sample.sensor.memory_temp_c));
    let throughput_samples = timeline
        .samples
        .iter()
        .filter_map(|sample| sample.throughput.clone())
        .filter(|sample| sample.value.is_finite() && sample.value >= 0.0)
        .collect::<Vec<_>>();
    let first_throughput = throughput_samples.first().cloned();
    let final_throughput = throughput_samples.last().cloned();
    let peak_throughput = throughput_samples
        .iter()
        .cloned()
        .max_by(|left, right| left.value.total_cmp(&right.value));
    let throughput_drop_percent = timeline_throughput_drop_percent(&throughput_samples);
    let max_temp_rise = timeline_max_temperature_rise(timeline);
    let peak_temp_warning = timeline_peak_warning(timeline.scope, peak_cpu_temp_c, peak_gpu_temp_c, peak_gpu_memory_temp_c, peak_drive_temp_c, peak_memory_temp_c);
    let mut findings = Vec::new();
    if let Some(drop) = throughput_drop_percent {
        if drop >= 20.0 {
            findings.push(TimelineFinding {
                severity: "warning".to_owned(),
                message: format!("Throughput dropped by {drop:.1}% during the run."),
            });
        } else if drop >= 10.0 {
            findings.push(TimelineFinding {
                severity: "caution".to_owned(),
                message: format!("Throughput dipped by {drop:.1}% during the run."),
            });
        }
    }
    if let Some((label, value)) = peak_temp_warning {
        findings.push(TimelineFinding {
            severity: "caution".to_owned(),
            message: format!("{label} peaked at {value:.0} C."),
        });
    }
    if let Some(rise) = max_temp_rise {
        if rise >= 8.0 {
            findings.push(TimelineFinding {
                severity: "info".to_owned(),
                message: format!("Temperature rose by up to {rise:.0} C while the workload was active."),
            });
        }
    }
    let confidence = timeline_confidence(throughput_drop_percent, max_temp_rise, peak_temp_warning.is_some());
    if findings.is_empty() {
        findings.push(TimelineFinding {
            severity: "info".to_owned(),
            message: if throughput_samples.is_empty() {
                "No throughput samples were available for thermal correlation.".to_owned()
            } else {
                "No strong heat-correlated performance drop was detected.".to_owned()
            },
        });
    }

    TimelineSummary {
        run_id: timeline.run_id.clone(),
        title: timeline.title.clone(),
        scope: timeline.scope,
        duration_ms: timeline
            .samples
            .last()
            .map(|sample| sample.elapsed_ms)
            .unwrap_or(0),
        sample_count: timeline.samples.len(),
        peak_cpu_temp_c,
        peak_gpu_temp_c,
        peak_gpu_memory_temp_c,
        peak_drive_temp_c,
        peak_memory_temp_c,
        first_throughput,
        peak_throughput,
        final_throughput,
        throughput_drop_percent,
        confidence,
        findings,
    }
}

fn peak_timeline_value(values: impl Iterator<Item = Option<f32>>) -> Option<f32> {
    values.flatten().max_by(|left, right| left.total_cmp(right))
}

fn timeline_throughput_drop_percent(samples: &[TimelineThroughputSample]) -> Option<f64> {
    if samples.len() < 5 {
        return None;
    }
    let baseline_len = samples.len().min(5);
    let baseline =
        samples.iter().take(baseline_len).map(|sample| sample.value).sum::<f64>() / baseline_len as f64;
    if baseline <= f64::EPSILON {
        return None;
    }
    let window = 3.min(samples.len());
    let mut lowest = f64::INFINITY;
    for slice in samples.windows(window) {
        let avg = slice.iter().map(|sample| sample.value).sum::<f64>() / slice.len() as f64;
        lowest = lowest.min(avg);
    }
    (lowest < baseline).then_some((baseline - lowest) / baseline * 100.0)
}

fn timeline_max_temperature_rise(timeline: &RunTimeline) -> Option<f32> {
    let series = [
        timeline
            .samples
            .iter()
            .map(|sample| sample.sensor.cpu_temp_c)
            .collect::<Vec<_>>(),
        timeline
            .samples
            .iter()
            .map(|sample| sample.sensor.gpu_temp_c)
            .collect::<Vec<_>>(),
        timeline
            .samples
            .iter()
            .map(|sample| sample.sensor.gpu_memory_temp_c)
            .collect::<Vec<_>>(),
        timeline
            .samples
            .iter()
            .map(|sample| sample.sensor.drive_temp_c)
            .collect::<Vec<_>>(),
        timeline
            .samples
            .iter()
            .map(|sample| sample.sensor.memory_temp_c)
            .collect::<Vec<_>>(),
    ];
    series
        .iter()
        .filter_map(|values| {
            let first = values.iter().flatten().next().copied()?;
            let peak = values.iter().flatten().copied().max_by(|left, right| left.total_cmp(right))?;
            Some(peak - first)
        })
        .max_by(|left, right| left.total_cmp(right))
}

fn timeline_peak_warning(
    scope: TimelineScope,
    cpu: Option<f32>,
    gpu: Option<f32>,
    vram: Option<f32>,
    drive: Option<f32>,
    memory: Option<f32>,
) -> Option<(&'static str, f32)> {
    let mut candidates = Vec::new();
    if let Some(value) = cpu.filter(|value| *value >= SensorKind::Cpu.warning_c()) {
        candidates.push(("CPU", value));
    }
    if let Some(value) = gpu.filter(|value| *value >= SensorKind::Gpu.warning_c()) {
        candidates.push(("GPU", value));
    }
    if let Some(value) = vram.filter(|value| *value >= SensorKind::GpuMemory.warning_c()) {
        candidates.push(("VRAM", value));
    }
    if let Some(value) = drive.filter(|value| *value >= SensorKind::Drive.warning_c()) {
        candidates.push(("SSD", value));
    }
    if let Some(value) = memory.filter(|value| *value >= SensorKind::Memory.warning_c()) {
        candidates.push(("RAM", value));
    }
    if scope == TimelineScope::DriveBenchmark {
        candidates.sort_by_key(|(label, _)| if *label == "SSD" { 0 } else { 1 });
    }
    candidates.into_iter().max_by(|left, right| left.1.total_cmp(&right.1))
}

fn timeline_confidence(
    drop_percent: Option<f64>,
    temp_rise: Option<f32>,
    threshold_crossed: bool,
) -> String {
    let drop = drop_percent.unwrap_or(0.0);
    let rise = temp_rise.unwrap_or(0.0);
    if drop >= 20.0 && (threshold_crossed || rise >= 12.0) {
        "High".to_owned()
    } else if drop >= 10.0 && (threshold_crossed || rise >= 8.0) {
        "Medium".to_owned()
    } else if drop >= 10.0 {
        "Low".to_owned()
    } else {
        "None".to_owned()
    }
}

fn render_timeline_report(records: &[CompletedTimeline]) -> String {
    let mut report = String::new();
    report.push_str("# BenchScope Thermal Timeline Summary\n\n");
    if records.is_empty() {
        report.push_str("No thermal timelines have completed in this session.\n");
        return report;
    }
    for record in records.iter().rev() {
        let summary = &record.summary;
        report.push_str(&format!(
            "## {} - {}\n\n",
            summary.scope.label(),
            markdown_escape(&summary.title)
        ));
        report.push_str(&format!(
            "- Started: {}\n",
            timeline_system_time_unix_seconds(record.timeline.started_at)
        ));
        report.push_str(&format!(
            "- Duration: {}\n",
            format_elapsed(summary.duration_ms as f64 / 1000.0)
        ));
        report.push_str(&format!("- Samples: {}\n", summary.sample_count));
        report.push_str(&format!("- Confidence: {}\n", summary.confidence));
        report.push_str(&format!(
            "- Peak CPU/GPU/VRAM/SSD/RAM temps: {} / {} / {} / {} / {}\n",
            format_temperature_value(summary.peak_cpu_temp_c),
            format_temperature_value(summary.peak_gpu_temp_c),
            format_temperature_value(summary.peak_gpu_memory_temp_c),
            format_temperature_value(summary.peak_drive_temp_c),
            format_temperature_value(summary.peak_memory_temp_c)
        ));
        let peak_cpu_clock_mhz =
            peak_timeline_value(record.timeline.samples.iter().map(|sample| sample.sensor.cpu_clock_mhz));
        let peak_gpu_clock_mhz =
            peak_timeline_value(record.timeline.samples.iter().map(|sample| sample.sensor.gpu_clock_mhz));
        let peak_cpu_power_w =
            peak_timeline_value(record.timeline.samples.iter().map(|sample| sample.sensor.cpu_power_w));
        let peak_gpu_power_w =
            peak_timeline_value(record.timeline.samples.iter().map(|sample| sample.sensor.gpu_power_w));
        if peak_cpu_clock_mhz.is_some() || peak_gpu_clock_mhz.is_some() {
            report.push_str(&format!(
                "- Peak CPU/GPU clocks: {} / {}\n",
                format_optional_mhz(peak_cpu_clock_mhz),
                format_optional_mhz(peak_gpu_clock_mhz)
            ));
        }
        if peak_cpu_power_w.is_some() || peak_gpu_power_w.is_some() {
            report.push_str(&format!(
                "- Peak CPU/GPU power: {} / {}\n",
                format_timeline_optional_watts(peak_cpu_power_w),
                format_timeline_optional_watts(peak_gpu_power_w)
            ));
        }
        if let Some(drop) = summary.throughput_drop_percent {
            report.push_str(&format!("- Throughput drop: {drop:.1}%\n"));
        }
        if let Some(throughput) = &summary.first_throughput {
            report.push_str(&format!(
                "- Initial throughput: {} {:.2} {}\n",
                markdown_escape(&throughput.label),
                throughput.value,
                throughput.unit
            ));
        }
        if let Some(throughput) = &summary.peak_throughput {
            report.push_str(&format!(
                "- Peak throughput: {} {:.2} {}\n",
                markdown_escape(&throughput.label),
                throughput.value,
                throughput.unit
            ));
        }
        if let Some(throughput) = &summary.final_throughput {
            report.push_str(&format!(
                "- Final throughput: {} {:.2} {}\n",
                markdown_escape(&throughput.label),
                throughput.value,
                throughput.unit
            ));
        }
        report.push_str("\n### Findings\n\n");
        for finding in &summary.findings {
            report.push_str(&format!(
                "- **{}**: {}\n",
                markdown_escape(&finding.severity),
                markdown_escape(&finding.message)
            ));
        }
        report.push('\n');
    }
    report
}

fn format_optional_mhz(value: Option<f32>) -> String {
    value
        .map(|value| format!("{value:.0} MHz"))
        .unwrap_or_else(|| "N/A".to_owned())
}

fn format_timeline_optional_watts(value: Option<f32>) -> String {
    value
        .map(|value| format!("{value:.1} W"))
        .unwrap_or_else(|| "N/A".to_owned())
}

fn timeline_system_time_unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn timeline_throughput(label: &str, value: f64, unit: &str) -> TimelineThroughputSample {
    TimelineThroughputSample {
        label: label.to_owned(),
        value,
        unit: unit.to_owned(),
    }
}
