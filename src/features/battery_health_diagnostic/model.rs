#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BatteryReportDuration {
    Days3,
    Days7,
    Days14,
    Days30,
}

impl BatteryReportDuration {
    const ALL: [BatteryReportDuration; 4] = [
        BatteryReportDuration::Days3,
        BatteryReportDuration::Days7,
        BatteryReportDuration::Days14,
        BatteryReportDuration::Days30,
    ];

    fn days(self) -> u32 {
        match self {
            BatteryReportDuration::Days3 => 3,
            BatteryReportDuration::Days7 => 7,
            BatteryReportDuration::Days14 => 14,
            BatteryReportDuration::Days30 => 30,
        }
    }

    fn label(self) -> &'static str {
        match self {
            BatteryReportDuration::Days3 => "3 days",
            BatteryReportDuration::Days7 => "7 days",
            BatteryReportDuration::Days14 => "14 days",
            BatteryReportDuration::Days30 => "30 days",
        }
    }
}

impl fmt::Display for BatteryReportDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BatteryHealthGrade {
    Excellent,
    Good,
    Fair,
    Poor,
    Failed,
    Unknown,
}

impl BatteryHealthGrade {
    fn label(self) -> &'static str {
        match self {
            BatteryHealthGrade::Excellent => "Excellent",
            BatteryHealthGrade::Good => "Good",
            BatteryHealthGrade::Fair => "Fair",
            BatteryHealthGrade::Poor => "Poor",
            BatteryHealthGrade::Failed => "Failed",
            BatteryHealthGrade::Unknown => "Unknown",
        }
    }

    fn color(self) -> egui::Color32 {
        match self {
            BatteryHealthGrade::Excellent | BatteryHealthGrade::Good => egui::Color32::GREEN,
            BatteryHealthGrade::Fair => egui::Color32::YELLOW,
            BatteryHealthGrade::Poor | BatteryHealthGrade::Failed => egui::Color32::RED,
            BatteryHealthGrade::Unknown => egui::Color32::GRAY,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BatteryWarningSeverity {
    Info,
    Warning,
    Critical,
}

impl BatteryWarningSeverity {
    fn color(self) -> egui::Color32 {
        match self {
            BatteryWarningSeverity::Info => egui::Color32::LIGHT_BLUE,
            BatteryWarningSeverity::Warning => egui::Color32::YELLOW,
            BatteryWarningSeverity::Critical => egui::Color32::RED,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct BatteryInfo {
    id: Option<String>,
    manufacturer: Option<String>,
    serial_number: Option<String>,
    chemistry: Option<String>,
    design_capacity_mwh: Option<f64>,
    full_charge_capacity_mwh: Option<f64>,
    cycle_count: Option<u32>,
}

#[derive(Clone, Debug)]
struct BatteryCapacityPoint {
    label: String,
    design_capacity_mwh: Option<f64>,
    full_charge_capacity_mwh: Option<f64>,
    cycle_count: Option<u32>,
}

#[derive(Clone, Debug)]
struct BatteryUsagePoint {
    label: String,
    ac_connected: Option<bool>,
    charge_capacity_mwh: Option<f64>,
    discharge_mwh: Option<f64>,
    full_charge_capacity_mwh: Option<f64>,
}

#[derive(Clone, Debug)]
struct BatteryLiveSample {
    captured_at: Instant,
    ac_connected: Option<bool>,
    status: String,
    percent: Option<f32>,
    remaining_capacity_mwh: Option<f64>,
    charge_rate_watts: Option<f64>,
    discharge_rate_watts: Option<f64>,
    windows_runtime_minutes: Option<f64>,
}

#[derive(Clone, Debug)]
struct BatteryRuntimeAccuracy {
    label: String,
    error_percent: f64,
    observed_minutes: f64,
    windows_minutes: f64,
}

#[derive(Clone, Debug)]
struct BatteryWarning {
    severity: BatteryWarningSeverity,
    title: String,
    detail: String,
}

#[derive(Clone, Debug)]
struct BatteryReport {
    generated_at: Option<String>,
    batteries: Vec<BatteryInfo>,
    capacity_history: Vec<BatteryCapacityPoint>,
    recent_usage: Vec<BatteryUsagePoint>,
    live_sample: Option<BatteryLiveSample>,
    warnings: Vec<BatteryWarning>,
    notes: Vec<String>,
}

impl BatteryReport {
    fn primary_battery(&self) -> Option<&BatteryInfo> {
        self.batteries.first()
    }

    fn full_charge_capacity_mwh(&self) -> Option<f64> {
        self.primary_battery()
            .and_then(|battery| battery.full_charge_capacity_mwh)
            .or_else(|| {
                self.capacity_history
                    .iter()
                    .rev()
                    .find_map(|point| point.full_charge_capacity_mwh)
            })
    }
}

#[derive(Debug)]
enum BatteryWorkerEvent {
    ScanDone(Result<BatteryReport, String>),
    LiveSample(BatteryLiveSample),
    Log(String),
}

struct BatteryDiagnosticState {
    duration: BatteryReportDuration,
    latest_report: Option<BatteryReport>,
    live_samples: VecDeque<BatteryLiveSample>,
    log: Vec<String>,
    status: String,
    rx: Receiver<BatteryWorkerEvent>,
    tx: Sender<BatteryWorkerEvent>,
    scan_cancel: Option<Arc<AtomicBool>>,
    live_cancel: Option<Arc<AtomicBool>>,
    scanning: bool,
    live_running: bool,
}

impl BatteryDiagnosticState {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            duration: BatteryReportDuration::Days14,
            latest_report: None,
            live_samples: VecDeque::new(),
            log: vec!["Battery diagnostic ready".to_owned()],
            status: "Ready".to_owned(),
            rx,
            tx,
            scan_cancel: None,
            live_cancel: None,
            scanning: false,
            live_running: false,
        }
    }

    fn log(&mut self, message: impl Into<String>) {
        self.log.push(message.into());
    }

    fn start_scan(&mut self) {
        if self.scanning {
            return;
        }
        let duration = self.duration;
        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.scan_cancel = Some(cancel);
        self.scanning = true;
        self.status = format!("Generating {} battery report...", duration);
        self.log(self.status.clone());

        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                run_battery_report_scan(duration, worker_cancel)
            }))
            .map_err(|panic| format!("Battery scan panicked: {}", panic_message(&*panic)))
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(BatteryWorkerEvent::ScanDone(result));
        });
    }

    fn cancel_scan(&mut self) {
        if let Some(cancel) = &self.scan_cancel {
            cancel.store(true, Ordering::Relaxed);
            self.status = "Cancel requested for battery scan".to_owned();
            self.log(self.status.clone());
        }
    }

    fn start_live_sampling(&mut self) {
        if self.live_running {
            return;
        }
        let full_charge_capacity_mwh = self
            .latest_report
            .as_ref()
            .and_then(BatteryReport::full_charge_capacity_mwh);
        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.live_cancel = Some(cancel);
        self.live_running = true;
        self.status = "Live battery sampling started".to_owned();
        self.log(self.status.clone());

        thread::spawn(move || {
            while !worker_cancel.load(Ordering::Relaxed) {
                match collect_battery_live_sample(full_charge_capacity_mwh) {
                    Ok(sample) => {
                        let _ = tx.send(BatteryWorkerEvent::LiveSample(sample));
                    }
                    Err(err) => {
                        let _ = tx.send(BatteryWorkerEvent::Log(format!(
                            "Live battery sample unavailable: {err:#}"
                        )));
                    }
                }

                let sleep_until = Instant::now() + Duration::from_millis(BATTERY_LIVE_SAMPLE_MS);
                while Instant::now() < sleep_until {
                    if worker_cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        });
    }

    fn stop_live_sampling(&mut self) {
        if let Some(cancel) = &self.live_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.live_cancel = None;
        if self.live_running {
            self.status = "Live battery sampling stopped".to_owned();
            self.log(self.status.clone());
        }
        self.live_running = false;
    }

    fn stop_all(&mut self) {
        self.cancel_scan();
        self.stop_live_sampling();
    }

    fn push_live_sample(&mut self, sample: BatteryLiveSample) {
        self.live_samples.push_back(sample);
        while self.live_samples.len() > BATTERY_LIVE_SAMPLE_LIMIT {
            self.live_samples.pop_front();
        }
    }

    fn latest_live_sample(&self) -> Option<&BatteryLiveSample> {
        self.live_samples.back().or_else(|| {
            self.latest_report
                .as_ref()
                .and_then(|report| report.live_sample.as_ref())
        })
    }

    fn poll_worker_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                BatteryWorkerEvent::ScanDone(result) => {
                    self.scanning = false;
                    self.scan_cancel = None;
                    match result {
                        Ok(report) => {
                            if let Some(sample) = report.live_sample.clone() {
                                self.push_live_sample(sample);
                            }
                            if report.batteries.is_empty() {
                                self.status = "No battery detected on this system".to_owned();
                            } else {
                                self.status = "Battery diagnostic scan complete".to_owned();
                            }
                            self.log(self.status.clone());
                            for note in &report.notes {
                                self.log(note.clone());
                            }
                            self.latest_report = Some(report);
                        }
                        Err(err) => {
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
                BatteryWorkerEvent::LiveSample(sample) => {
                    self.status = format!(
                        "Live sample: {}, charge {}",
                        sample.status,
                        format_optional_percent(sample.percent)
                    );
                    self.push_live_sample(sample);
                }
                BatteryWorkerEvent::Log(message) => self.log(message),
            }
        }
    }
}

