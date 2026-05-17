#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SensorKind {
    Cpu,
    Gpu,
    Drive,
    Memory,
}

impl SensorKind {
    fn warning_c(self) -> f32 {
        match self {
            SensorKind::Cpu => 85.0,
            SensorKind::Gpu => 80.0,
            SensorKind::Drive => 60.0,
            SensorKind::Memory => 70.0,
        }
    }

    fn critical_c(self) -> f32 {
        match self {
            SensorKind::Cpu => 95.0,
            SensorKind::Gpu => 90.0,
            SensorKind::Drive => 70.0,
            SensorKind::Memory => 85.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SensorStatus {
    Ok,
    Partial(String),
    Unsupported,
    PermissionDenied,
    Stale,
    Error(String),
}

impl SensorStatus {
    fn detail(&self) -> String {
        match self {
            SensorStatus::Ok => "OK".to_owned(),
            SensorStatus::Partial(message) => message.clone(),
            SensorStatus::Unsupported => "Unsupported".to_owned(),
            SensorStatus::PermissionDenied => "Permission denied".to_owned(),
            SensorStatus::Stale => "Stale reading".to_owned(),
            SensorStatus::Error(message) => message.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum SensorMetricKind {
    Temperature,
    Utilization,
    Voltage,
    Power,
    Clock,
}

impl SensorMetricKind {
    fn group_label(self) -> &'static str {
        match self {
            SensorMetricKind::Temperature => "Temperatures",
            SensorMetricKind::Utilization => "Utilization",
            SensorMetricKind::Voltage => "Voltages",
            SensorMetricKind::Power => "Powers",
            SensorMetricKind::Clock => "Clocks",
        }
    }

    fn default_label(self) -> &'static str {
        match self {
            SensorMetricKind::Temperature => "Temperature",
            SensorMetricKind::Utilization => "Utilization",
            SensorMetricKind::Voltage => "Voltage",
            SensorMetricKind::Power => "Power",
            SensorMetricKind::Clock => "Clock",
        }
    }
}

#[derive(Clone, Debug)]
struct SensorMetric {
    kind: SensorMetricKind,
    label: String,
    value: Option<f32>,
    min: Option<f32>,
    max: Option<f32>,
}

impl SensorMetric {
    fn new(kind: SensorMetricKind, label: impl Into<String>, value: Option<f32>) -> Self {
        Self {
            kind,
            label: label.into(),
            value,
            min: None,
            max: None,
        }
    }

    fn with_range(mut self, min: Option<f32>, max: Option<f32>) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    fn has_value(&self) -> bool {
        self.value.is_some()
    }
}

#[derive(Clone, Debug)]
struct SensorReading {
    kind: SensorKind,
    label: String,
    temperature_c: Option<f32>,
    utilization_percent: Option<f32>,
    metrics: Vec<SensorMetric>,
    provider: String,
    updated_at: Instant,
    status: SensorStatus,
}

impl SensorReading {
    fn ok(kind: SensorKind, label: impl Into<String>, temperature_c: f32, provider: &str) -> Self {
        let mut reading = Self {
            kind,
            label: label.into(),
            temperature_c: Some(temperature_c),
            utilization_percent: None,
            metrics: Vec::new(),
            provider: provider.to_owned(),
            updated_at: Instant::now(),
            status: SensorStatus::Ok,
        };
        reading.sync_legacy_metrics();
        reading
    }

    fn unavailable(
        kind: SensorKind,
        label: impl Into<String>,
        provider: &str,
        status: SensorStatus,
    ) -> Self {
        let mut reading = Self {
            kind,
            label: label.into(),
            temperature_c: None,
            utilization_percent: None,
            metrics: Vec::new(),
            provider: provider.to_owned(),
            updated_at: Instant::now(),
            status,
        };
        reading.sync_legacy_metrics();
        reading
    }

    fn mark_stale(mut self) -> Self {
        if self.has_any_value() {
            self.status = SensorStatus::Stale;
        }
        self
    }

    fn is_ok(&self) -> bool {
        matches!(self.status, SensorStatus::Ok | SensorStatus::Partial(_))
            && self.has_any_value()
    }

    fn has_temperature(&self) -> bool {
        matches!(self.status, SensorStatus::Ok | SensorStatus::Partial(_))
            && self.temperature_c.is_some()
    }

    fn has_utilization(&self) -> bool {
        matches!(self.status, SensorStatus::Ok | SensorStatus::Partial(_))
            && self.utilization_percent.is_some()
    }

    fn has_any_value(&self) -> bool {
        self.temperature_c.is_some()
            || self.utilization_percent.is_some()
            || self.metrics.iter().any(SensorMetric::has_value)
    }

    fn metrics_for(&self, kind: SensorMetricKind) -> impl Iterator<Item = &SensorMetric> {
        self.metrics
            .iter()
            .filter(move |metric| metric.kind == kind && metric.has_value())
    }

    fn sync_legacy_metrics(&mut self) {
        if let Some(value) = self.temperature_c {
            let label = if self.label.trim().is_empty() {
                SensorMetricKind::Temperature.default_label().to_owned()
            } else {
                self.label.clone()
            };
            self.upsert_metric(SensorMetric::new(
                SensorMetricKind::Temperature,
                label,
                Some(value),
            ));
        }
        if let Some(value) = self.utilization_percent {
            self.upsert_metric(SensorMetric::new(
                SensorMetricKind::Utilization,
                SensorMetricKind::Utilization.default_label(),
                Some(value),
            ));
        }
    }

    fn upsert_metric(&mut self, metric: SensorMetric) {
        if !metric.has_value() {
            return;
        }
        if let Some(existing) = self
            .metrics
            .iter_mut()
            .find(|existing| sensor_metric_slots_match(existing, &metric))
        {
            if existing.value.is_none() {
                existing.value = metric.value;
            }
            if existing.min.is_none() {
                existing.min = metric.min;
            }
            if existing.max.is_none() {
                existing.max = metric.max;
            }
            if sensor_metric_label_is_generic(&existing.label)
                && !sensor_metric_label_is_generic(&metric.label)
            {
                existing.label = metric.label;
            }
        } else {
            self.metrics.push(metric);
        }
    }
}

fn sensor_metric_slots_match(left: &SensorMetric, right: &SensorMetric) -> bool {
    if left.kind != right.kind {
        return false;
    }
    left.label.eq_ignore_ascii_case(&right.label)
        || (matches!(
            left.kind,
            SensorMetricKind::Temperature | SensorMetricKind::Utilization
        ) && (sensor_metric_label_is_generic(&left.label)
            || sensor_metric_label_is_generic(&right.label)))
}

fn sensor_metric_label_is_generic(label: &str) -> bool {
    let label = label.trim().to_ascii_lowercase();
    matches!(
        label.as_str(),
        "" | "cpu"
            | "gpu"
            | "ssd"
            | "ram"
            | "temperature"
            | "temperatures"
            | "utilization"
            | "load"
            | "loads"
            | "voltage"
            | "voltages"
            | "power"
            | "powers"
            | "clock"
            | "clocks"
    )
}

#[derive(Clone, Debug, Default)]
struct SensorSnapshot {
    cpu: Option<SensorReading>,
    gpu: Option<SensorReading>,
    drive: Option<SensorReading>,
    memory: Option<SensorReading>,
    helper_elevated: Option<bool>,
}

impl SensorSnapshot {
    fn stale_checked(&self, now: Instant) -> Self {
        let stale_after = Duration::from_millis(SENSOR_STALE_AFTER_MS);
        Self {
            cpu: stale_checked_reading(self.cpu.clone(), now, stale_after),
            gpu: stale_checked_reading(self.gpu.clone(), now, stale_after),
            drive: stale_checked_reading(self.drive.clone(), now, stale_after),
            memory: stale_checked_reading(self.memory.clone(), now, stale_after),
            helper_elevated: self.helper_elevated,
        }
    }

    fn with_tracked_metric_ranges(mut self, previous: Option<&SensorSnapshot>) -> Self {
        track_reading_metric_ranges(&mut self.cpu, previous.and_then(|snapshot| snapshot.cpu.as_ref()));
        track_reading_metric_ranges(&mut self.gpu, previous.and_then(|snapshot| snapshot.gpu.as_ref()));
        track_reading_metric_ranges(
            &mut self.drive,
            previous.and_then(|snapshot| snapshot.drive.as_ref()),
        );
        track_reading_metric_ranges(
            &mut self.memory,
            previous.and_then(|snapshot| snapshot.memory.as_ref()),
        );
        self
    }
}

fn track_reading_metric_ranges(
    reading: &mut Option<SensorReading>,
    previous: Option<&SensorReading>,
) {
    let Some(reading) = reading else {
        return;
    };
    for metric in &mut reading.metrics {
        let Some(value) = metric.value else {
            continue;
        };
        let previous_metric = previous.and_then(|previous| {
            previous
                .metrics
                .iter()
                .find(|candidate| sensor_metric_slots_match(candidate, metric))
        });
        metric.min = Some(
            metric
                .min
                .or_else(|| previous_metric.and_then(|metric| metric.min))
                .map_or(value, |current| current.min(value)),
        );
        metric.max = Some(
            metric
                .max
                .or_else(|| previous_metric.and_then(|metric| metric.max))
                .map_or(value, |current| current.max(value)),
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TemperatureScope {
    Matrix,
    Drive,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct TemperatureSummary {
    start_c: Option<f32>,
    end_c: Option<f32>,
    max_c: Option<f32>,
}

impl TemperatureSummary {
    fn begin(value: Option<f32>) -> Self {
        Self {
            start_c: value,
            end_c: None,
            max_c: value,
        }
    }

    fn observe(&mut self, value: Option<f32>) {
        if let Some(value) = value {
            self.max_c = Some(self.max_c.map_or(value, |current| current.max(value)));
        }
    }

    fn finish(&mut self, value: Option<f32>) {
        self.end_c = value;
        self.observe(value);
    }

    fn has_any_value(&self) -> bool {
        self.start_c.is_some() || self.end_c.is_some() || self.max_c.is_some()
    }
}
#[derive(Clone, Debug)]
struct TemperatureRunTracker {
    scope: TemperatureScope,
    cpu: TemperatureSummary,
    gpu: TemperatureSummary,
    drive: TemperatureSummary,
}

impl TemperatureRunTracker {
    fn start(scope: TemperatureScope, snapshot: &SensorSnapshot) -> Self {
        Self {
            scope,
            cpu: TemperatureSummary::begin(sensor_temperature(snapshot.cpu.as_ref())),
            gpu: TemperatureSummary::begin(sensor_temperature(snapshot.gpu.as_ref())),
            drive: TemperatureSummary::begin(sensor_temperature(snapshot.drive.as_ref())),
        }
    }

    fn observe(&mut self, snapshot: &SensorSnapshot) {
        self.cpu.observe(sensor_temperature(snapshot.cpu.as_ref()));
        self.gpu.observe(sensor_temperature(snapshot.gpu.as_ref()));
        self.drive
            .observe(sensor_temperature(snapshot.drive.as_ref()));
    }

    fn finish(mut self, snapshot: &SensorSnapshot) -> TemperatureRunReport {
        self.cpu.finish(sensor_temperature(snapshot.cpu.as_ref()));
        self.gpu.finish(sensor_temperature(snapshot.gpu.as_ref()));
        self.drive
            .finish(sensor_temperature(snapshot.drive.as_ref()));
        TemperatureRunReport {
            scope: self.scope,
            cpu: self.cpu,
            gpu: self.gpu,
            drive: self.drive,
        }
    }
}
struct TemperatureRunReport {
    scope: TemperatureScope,
    cpu: TemperatureSummary,
    gpu: TemperatureSummary,
    drive: TemperatureSummary,
}

struct SensorManager {
    latest: Arc<RwLock<SensorSnapshot>>,
    target_drive_letter: Arc<RwLock<Option<char>>>,
    target_gpu_uses_shared_cpu_temperature: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
}

impl SensorManager {
    fn new(initial_drive_letter: Option<char>) -> Self {
        let latest = Arc::new(RwLock::new(SensorSnapshot::default()));
        let target_drive_letter = Arc::new(RwLock::new(initial_drive_letter));
        let target_gpu_uses_shared_cpu_temperature = Arc::new(AtomicBool::new(false));
        let service_enabled = sensor_service_enabled();
        let helper_enabled = sensor_helper_enabled();
        let shutdown = Arc::new(AtomicBool::new(false));

        let thread_latest = Arc::clone(&latest);
        let thread_target_drive_letter = Arc::clone(&target_drive_letter);
        let thread_target_gpu_uses_shared_cpu_temperature =
            Arc::clone(&target_gpu_uses_shared_cpu_temperature);
        let thread_shutdown = Arc::clone(&shutdown);

        let _ = thread::Builder::new()
            .name("benchscope-sensors".to_owned())
            .spawn(move || {
                let mut service_rx: Option<Receiver<SensorSnapshot>> = None;
                let mut service_start_attempted = false;
                let mut service_snapshot: Option<SensorSnapshot> = None;
                let mut helper_rx: Option<Receiver<SensorSnapshot>> = None;
                let mut helper_start_attempted = false;
                let mut helper_snapshot: Option<SensorSnapshot> = None;
                while !thread_shutdown.load(Ordering::Relaxed) {
                    if service_rx.is_none() && !service_start_attempted && service_enabled {
                        service_rx = start_sensor_service_reader();
                        service_start_attempted = true;
                    }
                    if helper_rx.is_none() && !helper_start_attempted && helper_enabled {
                        helper_rx = start_sensor_helper_reader();
                        helper_start_attempted = true;
                    }
                    let drive_letter = thread_target_drive_letter
                        .read()
                        .map(|guard| *guard)
                        .unwrap_or(None);
                    let use_shared_gpu_temperature =
                        thread_target_gpu_uses_shared_cpu_temperature.load(Ordering::Relaxed);
                    if let Some(helper_rx) = &helper_rx {
                        while let Ok(snapshot) = helper_rx.try_recv() {
                            helper_snapshot = Some(snapshot);
                        }
                    }
                    if let Some(service_rx) = &service_rx {
                        while let Ok(snapshot) = service_rx.try_recv() {
                            service_snapshot = Some(snapshot);
                        }
                    }

                    let primary_snapshot =
                        merge_sensor_snapshots(service_snapshot.clone(), helper_snapshot.clone());
                    let fallback_snapshot = primary_snapshot.as_ref().and_then(|snapshot| {
                        helper_snapshot_has_gaps(snapshot)
                            .then(|| collect_sensor_snapshot(drive_letter))
                    });
                    let snapshot = merge_sensor_snapshots(primary_snapshot, fallback_snapshot)
                        .unwrap_or_else(|| collect_sensor_snapshot(drive_letter));
                    let snapshot = apply_integrated_gpu_temperature_fallback(
                        snapshot,
                        use_shared_gpu_temperature,
                    );
                    if let Ok(mut latest) = thread_latest.write() {
                        *latest = snapshot.with_tracked_metric_ranges(Some(&*latest));
                    }

                    let sleep_for = Duration::from_millis(SENSOR_POLL_MS);
                    let sleep_until = Instant::now() + sleep_for;
                    while Instant::now() < sleep_until {
                        if thread_shutdown.load(Ordering::Relaxed) {
                            return;
                        }
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            });

        Self {
            latest,
            target_drive_letter,
            target_gpu_uses_shared_cpu_temperature,
            shutdown,
        }
    }

    fn latest(&self) -> SensorSnapshot {
        self.latest
            .read()
            .map(|snapshot| snapshot.stale_checked(Instant::now()))
            .unwrap_or_default()
    }

    fn set_target_drive_letter(&self, drive_letter: Option<char>) {
        if let Ok(mut target) = self.target_drive_letter.write() {
            *target = drive_letter.map(|letter| letter.to_ascii_uppercase());
        }
    }

    fn set_target_gpu_uses_shared_cpu_temperature(&self, value: bool) {
        self.target_gpu_uses_shared_cpu_temperature
            .store(value, Ordering::Relaxed);
    }
}

impl Drop for SensorManager {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}
