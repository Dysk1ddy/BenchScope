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

#[derive(Clone, Debug)]
struct SensorReading {
    kind: SensorKind,
    label: String,
    temperature_c: Option<f32>,
    utilization_percent: Option<f32>,
    provider: String,
    updated_at: Instant,
    status: SensorStatus,
}

impl SensorReading {
    fn ok(kind: SensorKind, label: impl Into<String>, temperature_c: f32, provider: &str) -> Self {
        Self {
            kind,
            label: label.into(),
            temperature_c: Some(temperature_c),
            utilization_percent: None,
            provider: provider.to_owned(),
            updated_at: Instant::now(),
            status: SensorStatus::Ok,
        }
    }

    fn unavailable(
        kind: SensorKind,
        label: impl Into<String>,
        provider: &str,
        status: SensorStatus,
    ) -> Self {
        Self {
            kind,
            label: label.into(),
            temperature_c: None,
            utilization_percent: None,
            provider: provider.to_owned(),
            updated_at: Instant::now(),
            status,
        }
    }

    fn mark_stale(mut self) -> Self {
        if self.temperature_c.is_some() || self.utilization_percent.is_some() {
            self.status = SensorStatus::Stale;
        }
        self
    }

    fn is_ok(&self) -> bool {
        matches!(self.status, SensorStatus::Ok | SensorStatus::Partial(_))
            && (self.temperature_c.is_some() || self.utilization_percent.is_some())
    }

    fn has_temperature(&self) -> bool {
        matches!(self.status, SensorStatus::Ok | SensorStatus::Partial(_))
            && self.temperature_c.is_some()
    }

    fn has_utilization(&self) -> bool {
        matches!(self.status, SensorStatus::Ok | SensorStatus::Partial(_))
            && self.utilization_percent.is_some()
    }
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
                        service_snapshot.clone().or_else(|| helper_snapshot.clone());
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
                        *latest = snapshot;
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
