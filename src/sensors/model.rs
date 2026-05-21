#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SensorKind {
    Cpu,
    Gpu,
    GpuMemory,
    Drive,
    Memory,
}

impl SensorKind {
    fn warning_c(self) -> f32 {
        match self {
            SensorKind::Cpu => 85.0,
            SensorKind::Gpu => 80.0,
            SensorKind::GpuMemory => 90.0,
            SensorKind::Drive => 60.0,
            SensorKind::Memory => 70.0,
        }
    }

    fn critical_c(self) -> f32 {
        match self {
            SensorKind::Cpu => 95.0,
            SensorKind::Gpu => 90.0,
            SensorKind::GpuMemory => 100.0,
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
    MemoryUsage,
    Voltage,
    Power,
    Clock,
}

impl SensorMetricKind {
    fn default_label(self) -> &'static str {
        match self {
            SensorMetricKind::Temperature => "Temperature",
            SensorMetricKind::Utilization => "Utilization",
            SensorMetricKind::MemoryUsage => "VRAM Used",
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
            SensorMetricKind::Temperature
                | SensorMetricKind::Utilization
                | SensorMetricKind::MemoryUsage
        ) && (sensor_metric_label_is_generic(&left.label)
            || sensor_metric_label_is_generic(&right.label)))
}

fn sensor_metric_label_is_generic(label: &str) -> bool {
    let label = label.trim().to_ascii_lowercase();
    matches!(
        label.as_str(),
        "" | "cpu"
            | "gpu"
            | "vram"
            | "gpu memory"
            | "ssd"
            | "ram"
            | "temperature"
            | "temperatures"
            | "utilization"
            | "memory usage"
            | "vram used"
            | "used"
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
    gpu_memory: Option<SensorReading>,
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
            gpu_memory: stale_checked_reading(self.gpu_memory.clone(), now, stale_after),
            drive: stale_checked_reading(self.drive.clone(), now, stale_after),
            memory: stale_checked_reading(self.memory.clone(), now, stale_after),
            helper_elevated: self.helper_elevated,
        }
    }

    fn with_tracked_metric_ranges(mut self, previous: Option<&SensorSnapshot>) -> Self {
        track_reading_metric_ranges(&mut self.cpu, previous.and_then(|snapshot| snapshot.cpu.as_ref()));
        track_reading_metric_ranges(&mut self.gpu, previous.and_then(|snapshot| snapshot.gpu.as_ref()));
        track_reading_metric_ranges(
            &mut self.gpu_memory,
            previous.and_then(|snapshot| snapshot.gpu_memory.as_ref()),
        );
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

    fn with_reset_metric_ranges(mut self) -> Self {
        self.reset_metric_ranges();
        self
    }

    fn reset_metric_ranges(&mut self) {
        reset_reading_metric_ranges(&mut self.cpu);
        reset_reading_metric_ranges(&mut self.gpu);
        reset_reading_metric_ranges(&mut self.gpu_memory);
        reset_reading_metric_ranges(&mut self.drive);
        reset_reading_metric_ranges(&mut self.memory);
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
            previous_metric
                .and_then(|metric| metric.min)
                .or(metric.min)
                .map_or(value, |current| current.min(value)),
        );
        metric.max = Some(
            previous_metric
                .and_then(|metric| metric.max)
                .or(metric.max)
                .map_or(value, |current| current.max(value)),
        );
    }
}

fn reset_reading_metric_ranges(reading: &mut Option<SensorReading>) {
    let Some(reading) = reading else {
        return;
    };
    for metric in &mut reading.metrics {
        let Some(value) = metric.value else {
            metric.min = None;
            metric.max = None;
            continue;
        };
        metric.min = Some(value);
        if metric.kind != SensorMetricKind::MemoryUsage || metric.max.is_none() {
            metric.max = Some(value);
        }
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

struct SensorFallbackWorker {
    snapshot: Option<SensorSnapshot>,
    rx: Option<Receiver<SensorSnapshot>>,
    target_drive_letter: Option<char>,
    pending_drive_letter: Option<char>,
    started_at: Option<Instant>,
    last_finished_at: Option<Instant>,
}

impl SensorFallbackWorker {
    fn new() -> Self {
        Self {
            snapshot: None,
            rx: None,
            target_drive_letter: None,
            pending_drive_letter: None,
            started_at: None,
            last_finished_at: None,
        }
    }

    fn refresh_target_drive(&mut self, drive_letter: Option<char>) {
        if self.target_drive_letter == drive_letter {
            return;
        }
        self.target_drive_letter = drive_letter;
        self.pending_drive_letter = None;
        self.snapshot = None;
        self.rx = None;
        self.started_at = None;
        self.last_finished_at = None;
    }

    fn collect_finished(&mut self, now: Instant) {
        let result = self.rx.as_ref().map(|receiver| receiver.try_recv());
        match result {
            Some(Ok(snapshot)) => {
                if self.pending_drive_letter == self.target_drive_letter {
                    self.snapshot = Some(snapshot);
                }
                self.rx = None;
                self.pending_drive_letter = None;
                self.started_at = None;
                self.last_finished_at = Some(now);
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.rx = None;
                self.pending_drive_letter = None;
                self.started_at = None;
                self.last_finished_at = Some(now);
            }
            Some(Err(mpsc::TryRecvError::Empty)) => {
                if self.started_at.is_some_and(|started_at| {
                    now.duration_since(started_at)
                        > Duration::from_millis(SENSOR_FALLBACK_ABANDON_MS)
                }) {
                    self.rx = None;
                    self.pending_drive_letter = None;
                    self.started_at = None;
                    self.last_finished_at = Some(now);
                }
            }
            None => {}
        }
    }

    fn maybe_start(&mut self, drive_letter: Option<char>, now: Instant) {
        self.refresh_target_drive(drive_letter);
        self.collect_finished(now);
        if self.rx.is_some() {
            return;
        }
        if self.last_finished_at.is_some_and(|finished_at| {
            now.duration_since(finished_at) < Duration::from_millis(SENSOR_FALLBACK_REFRESH_MS)
        }) {
            return;
        }

        let (tx, rx) = mpsc::channel();
        let spawn_result = thread::Builder::new()
            .name("benchscope-sensors-fallback".to_owned())
            .spawn(move || {
                let _ = tx.send(collect_sensor_snapshot(drive_letter));
            });
        if spawn_result.is_ok() {
            self.rx = Some(rx);
            self.pending_drive_letter = drive_letter;
            self.started_at = Some(now);
        } else {
            self.last_finished_at = Some(now);
        }
    }

    fn latest(&self) -> Option<SensorSnapshot> {
        self.snapshot.clone()
    }
}

fn drain_sensor_bridge_receiver(
    rx: &mut Option<Receiver<SensorSnapshot>>,
    snapshot: &mut Option<SensorSnapshot>,
) -> bool {
    let mut disconnected = false;
    if let Some(receiver) = rx.as_ref() {
        loop {
            match receiver.try_recv() {
                Ok(next_snapshot) => {
                    *snapshot = Some(next_snapshot);
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
    }

    if disconnected {
        *rx = None;
        *snapshot = None;
    }

    disconnected
}

struct SensorManager {
    latest: Arc<RwLock<SensorSnapshot>>,
    target_drive_letter: Arc<RwLock<Option<char>>>,
    target_gpu_uses_shared_cpu_temperature: Arc<AtomicBool>,
    reset_metric_ranges_requested: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
}

impl SensorManager {
    fn new(initial_drive_letter: Option<char>) -> Self {
        let latest = Arc::new(RwLock::new(SensorSnapshot::default()));
        let target_drive_letter = Arc::new(RwLock::new(initial_drive_letter));
        let target_gpu_uses_shared_cpu_temperature = Arc::new(AtomicBool::new(false));
        let reset_metric_ranges_requested = Arc::new(AtomicBool::new(false));
        let service_enabled = sensor_service_enabled();
        let helper_enabled = sensor_helper_enabled();
        let shutdown = Arc::new(AtomicBool::new(false));

        let thread_latest = Arc::clone(&latest);
        let thread_target_drive_letter = Arc::clone(&target_drive_letter);
        let thread_target_gpu_uses_shared_cpu_temperature =
            Arc::clone(&target_gpu_uses_shared_cpu_temperature);
        let thread_reset_metric_ranges_requested = Arc::clone(&reset_metric_ranges_requested);
        let thread_shutdown = Arc::clone(&shutdown);

        let _ = thread::Builder::new()
            .name("benchscope-sensors".to_owned())
            .spawn(move || {
                let mut service_rx: Option<Receiver<SensorSnapshot>> = None;
                let mut service_snapshot: Option<SensorSnapshot> = None;
                let mut helper_rx: Option<Receiver<SensorSnapshot>> = None;
                let mut helper_snapshot: Option<SensorSnapshot> = None;
                let mut fallback_worker = SensorFallbackWorker::new();
                let mut next_service_start = Instant::now();
                let mut next_helper_start = Instant::now();
                while !thread_shutdown.load(Ordering::Relaxed) {
                    let now = Instant::now();
                    if service_rx.is_none() && service_enabled && now >= next_service_start {
                        service_rx = start_sensor_service_reader();
                        next_service_start =
                            now + Duration::from_millis(SENSOR_BRIDGE_RESTART_BACKOFF_MS);
                    }
                    if helper_rx.is_none() && helper_enabled && now >= next_helper_start {
                        helper_rx = start_sensor_helper_reader();
                        next_helper_start =
                            now + Duration::from_millis(SENSOR_BRIDGE_RESTART_BACKOFF_MS);
                    }
                    let drive_letter = thread_target_drive_letter
                        .read()
                        .map(|guard| *guard)
                        .unwrap_or(None);
                    let use_shared_gpu_temperature =
                        thread_target_gpu_uses_shared_cpu_temperature.load(Ordering::Relaxed);
                    if drain_sensor_bridge_receiver(&mut helper_rx, &mut helper_snapshot) {
                        next_helper_start =
                            Instant::now() + Duration::from_millis(SENSOR_BRIDGE_RESTART_BACKOFF_MS);
                    }
                    if drain_sensor_bridge_receiver(&mut service_rx, &mut service_snapshot) {
                        next_service_start =
                            Instant::now() + Duration::from_millis(SENSOR_BRIDGE_RESTART_BACKOFF_MS);
                    }

                    let primary_snapshot =
                        merge_sensor_snapshots(service_snapshot.clone(), helper_snapshot.clone());
                    let snapshot_now = Instant::now();
                    let needs_fallback = match primary_snapshot.as_ref() {
                        None => true,
                        Some(snapshot) => sensor_snapshot_needs_fallback(snapshot, snapshot_now),
                    };
                    if needs_fallback {
                        fallback_worker.maybe_start(drive_letter, snapshot_now);
                    } else {
                        fallback_worker.refresh_target_drive(drive_letter);
                        fallback_worker.collect_finished(snapshot_now);
                    }
                    let fallback_snapshot = fallback_worker.latest();
                    let snapshot = merge_sensor_snapshots_prefer_fresh(
                        primary_snapshot,
                        fallback_snapshot,
                        snapshot_now,
                    )
                    .unwrap_or_default();
                    let snapshot = apply_integrated_gpu_temperature_fallback(
                        snapshot,
                        use_shared_gpu_temperature,
                    );
                    if let Ok(mut latest) = thread_latest.write() {
                        *latest =
                            if thread_reset_metric_ranges_requested.swap(false, Ordering::Relaxed) {
                                snapshot.with_reset_metric_ranges()
                            } else {
                                snapshot.with_tracked_metric_ranges(Some(&*latest))
                            };
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
            reset_metric_ranges_requested,
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

    fn reset_metric_ranges(&self) {
        self.reset_metric_ranges_requested
            .store(true, Ordering::Relaxed);
        if let Ok(mut latest) = self.latest.write() {
            latest.reset_metric_ranges();
        }
    }
}

impl Drop for SensorManager {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}
