#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StorageHealthStatus {
    Good,
    Caution,
    Critical,
    Unknown,
}

impl StorageHealthStatus {
    fn label(self) -> &'static str {
        match self {
            StorageHealthStatus::Good => "Good",
            StorageHealthStatus::Caution => "Caution",
            StorageHealthStatus::Critical => "Critical",
            StorageHealthStatus::Unknown => "Unknown",
        }
    }
}

impl fmt::Display for StorageHealthStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HealthSeverity {
    Info,
    Warning,
    Critical,
}

impl HealthSeverity {
    fn label(self) -> &'static str {
        match self {
            HealthSeverity::Info => "Info",
            HealthSeverity::Warning => "Warning",
            HealthSeverity::Critical => "Critical",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StorageScanMode {
    Quick,
    Balanced,
}

impl StorageScanMode {
    const ALL: [StorageScanMode; 2] = [StorageScanMode::Quick, StorageScanMode::Balanced];

    fn label(self) -> &'static str {
        match self {
            StorageScanMode::Quick => "Quick scan",
            StorageScanMode::Balanced => "Balanced scan",
        }
    }

    fn sample_count(self) -> usize {
        match self {
            StorageScanMode::Quick => STORAGE_HEALTH_QUICK_SCAN_SAMPLES,
            StorageScanMode::Balanced => STORAGE_HEALTH_BALANCED_SCAN_SAMPLES,
        }
    }
}

impl fmt::Display for StorageScanMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Debug)]
struct StorageAttribute {
    id: Option<u16>,
    name: String,
    current: Option<u64>,
    worst: Option<u64>,
    threshold: Option<u64>,
    raw: Option<u64>,
    display_value: String,
    interpretation: String,
    severity: HealthSeverity,
}

#[derive(Clone, Debug)]
struct HealthWarning {
    severity: HealthSeverity,
    title: String,
    detail: String,
}

#[derive(Clone, Debug)]
struct StorageHealthSnapshot {
    drive_label: String,
    root: PathBuf,
    model: String,
    serial: Option<String>,
    firmware: Option<String>,
    bus_type: String,
    media_type: String,
    capacity_bytes: Option<u64>,
    free_bytes: Option<u64>,
    file_system: Option<String>,
    health_status_text: Option<String>,
    operational_status: Option<String>,
    smart_passed: Option<bool>,
    temperature_c: Option<f32>,
    utilization_percent: Option<f32>,
    remaining_life_percent: Option<f32>,
    power_on_hours: Option<u64>,
    power_cycle_count: Option<u64>,
    reallocated_sectors: Option<u64>,
    pending_sectors: Option<u64>,
    uncorrectable_sectors: Option<u64>,
    media_errors: Option<u64>,
    read_errors_total: Option<u64>,
    write_errors_total: Option<u64>,
    available_spare_percent: Option<u64>,
    available_spare_threshold_percent: Option<u64>,
    critical_warning_flags: Option<u64>,
    unsafe_shutdowns: Option<u64>,
    controller_busy_time_minutes: Option<u64>,
    host_read_commands: Option<u64>,
    host_write_commands: Option<u64>,
    warning_temperature_time_minutes: Option<u64>,
    critical_temperature_time_minutes: Option<u64>,
    thermal_management_temp1_transition_count: Option<u64>,
    thermal_management_temp2_transition_count: Option<u64>,
    nvme_temperature_sensors_c: [Option<f32>; 8],
    data_read_bytes: Option<u64>,
    data_written_bytes: Option<u64>,
    attributes: Vec<StorageAttribute>,
    warnings: Vec<HealthWarning>,
    provider_notes: Vec<String>,
    status: StorageHealthStatus,
    health_percent: Option<f32>,
    refreshed_at: SystemTime,
}

impl StorageHealthSnapshot {
    fn unknown(drive: &DriveInfo, note: impl Into<String>) -> Self {
        Self {
            drive_label: drive.label.clone(),
            root: drive.root.clone(),
            model: drive
                .device_name
                .clone()
                .unwrap_or_else(|| "Unknown drive".to_owned()),
            serial: None,
            firmware: None,
            bus_type: "Unknown".to_owned(),
            media_type: "Unknown".to_owned(),
            capacity_bytes: None,
            free_bytes: None,
            file_system: None,
            health_status_text: None,
            operational_status: None,
            smart_passed: None,
            temperature_c: None,
            utilization_percent: None,
            remaining_life_percent: None,
            power_on_hours: None,
            power_cycle_count: None,
            reallocated_sectors: None,
            pending_sectors: None,
            uncorrectable_sectors: None,
            media_errors: None,
            read_errors_total: None,
            write_errors_total: None,
            available_spare_percent: None,
            available_spare_threshold_percent: None,
            critical_warning_flags: None,
            unsafe_shutdowns: None,
            controller_busy_time_minutes: None,
            host_read_commands: None,
            host_write_commands: None,
            warning_temperature_time_minutes: None,
            critical_temperature_time_minutes: None,
            thermal_management_temp1_transition_count: None,
            thermal_management_temp2_transition_count: None,
            nvme_temperature_sensors_c: [None; 8],
            data_read_bytes: None,
            data_written_bytes: None,
            attributes: Vec::new(),
            warnings: Vec::new(),
            provider_notes: vec![note.into()],
            status: StorageHealthStatus::Unknown,
            health_percent: None,
            refreshed_at: SystemTime::now(),
        }
    }
}

#[derive(Clone, Debug)]
struct StorageScanProgress {
    mode: StorageScanMode,
    regions_done: usize,
    regions_total: usize,
    bytes_scanned: u64,
    read_errors: u64,
    slow_regions: u64,
    elapsed_s: f64,
    eta_s: Option<f64>,
}

#[derive(Clone, Debug)]
struct StorageScanResult {
    mode: StorageScanMode,
    bytes_scanned: u64,
    regions_scanned: usize,
    read_errors: u64,
    slow_regions: u64,
    avg_latency_ms: Option<f64>,
    worst_latency_ms: Option<f64>,
    duration_ms: f64,
    notes: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StorageHealthTask {
    Snapshot,
    Scan,
    Benchmark,
}

#[derive(Debug)]
enum StorageHealthEvent {
    Snapshot(Result<StorageHealthSnapshot, String>),
    ScanProgress(StorageScanProgress),
    ScanDone(Result<StorageScanResult, String>),
    BenchmarkDone(Result<Vec<DriveBenchmarkResult>, String>),
    Log(String),
}

struct StorageHealthState {
    drives: Vec<DriveInfo>,
    selected_drive: usize,
    snapshot: Option<StorageHealthSnapshot>,
    scan_result: Option<StorageScanResult>,
    benchmark_results: Vec<DriveBenchmarkResult>,
    log: Vec<String>,
    status: String,
    progress: f32,
    eta_text: String,
    rx: Receiver<StorageHealthEvent>,
    tx: Sender<StorageHealthEvent>,
    cancel: Option<Arc<AtomicBool>>,
    running: bool,
    active_task: Option<StorageHealthTask>,
    last_report_path: Option<PathBuf>,
}

impl StorageHealthState {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let drives = detect_drives();
        let system_root = std::env::current_dir()
            .ok()
            .and_then(|path| drive_root_for_path(&path))
            .unwrap_or_else(|| PathBuf::from("C:\\"));
        let selected_drive = selected_drive_for_path(&drives, &system_root).unwrap_or(0);

        Self {
            drives,
            selected_drive,
            snapshot: None,
            scan_result: None,
            benchmark_results: Vec::new(),
            log: vec!["SSD / HDD health checker ready".to_owned()],
            status: "Ready".to_owned(),
            progress: 0.0,
            eta_text: String::new(),
            rx,
            tx,
            cancel: None,
            running: false,
            active_task: None,
            last_report_path: None,
        }
    }

    fn log(&mut self, message: impl Into<String>) {
        self.log.push(message.into());
    }

    fn selected_drive(&self) -> Option<&DriveInfo> {
        self.drives.get(self.selected_drive)
    }

    fn selected_drive_label(&self) -> String {
        self.selected_drive()
            .map(|drive| drive.label.clone())
            .unwrap_or_else(|| "No drives detected".to_owned())
    }

    fn selected_drive_root(&self) -> Option<PathBuf> {
        self.selected_drive().map(|drive| drive.root.clone())
    }

    fn select_drive(&mut self, index: usize) {
        if let Some(drive) = self.drives.get(index) {
            self.selected_drive = index;
            self.snapshot = None;
            self.scan_result = None;
            self.benchmark_results.clear();
            self.status = format!("Selected {}", drive.label);
        }
    }

    fn refresh_drives(&mut self) {
        let previous_root = self.selected_drive_root();
        self.drives = detect_drives();
        self.selected_drive = previous_root
            .as_ref()
            .and_then(|root| selected_drive_for_path(&self.drives, root))
            .unwrap_or(0);
        self.log(format!("Detected {} drive(s)", self.drives.len()));
    }

    fn start_refresh(&mut self) {
        if self.running {
            return;
        }
        let Some(drive) = self.selected_drive().cloned() else {
            self.status = "No drive selected".to_owned();
            self.log(self.status.clone());
            return;
        };
        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.cancel = Some(cancel);
        self.running = true;
        self.active_task = Some(StorageHealthTask::Snapshot);
        self.progress = 0.05;
        self.eta_text = "ETA: estimating".to_owned();
        self.status = format!("Reading SMART and storage health for {}", drive.label);
        self.log(self.status.clone());

        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                if worker_cancel.load(Ordering::Relaxed) {
                    return Err(anyhow!("storage health refresh canceled"));
                }
                query_storage_health_snapshot(&drive)
            }))
            .map_err(|panic| {
                format!(
                    "Storage health refresh panicked: {}",
                    panic_message(&*panic)
                )
            })
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(StorageHealthEvent::Snapshot(result));
        });
    }

    fn start_scan(&mut self, mode: StorageScanMode) {
        if self.running {
            return;
        }
        let Some(drive) = self.selected_drive().cloned() else {
            self.status = "No drive selected".to_owned();
            self.log(self.status.clone());
            return;
        };
        let capacity_bytes = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.capacity_bytes);
        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.cancel = Some(cancel);
        self.running = true;
        self.active_task = Some(StorageHealthTask::Scan);
        self.progress = 0.0;
        self.eta_text = "ETA: estimating".to_owned();
        self.status = format!("Running {mode} on {}", drive.label);
        self.log(self.status.clone());

        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                run_storage_surface_scan(&drive, capacity_bytes, mode, worker_cancel, tx.clone())
            }))
            .map_err(|panic| format!("Storage scan panicked: {}", panic_message(&*panic)))
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(StorageHealthEvent::ScanDone(result));
        });
    }

    fn start_quick_benchmark(&mut self) {
        if self.running {
            return;
        }
        let Some(drive) = self.selected_drive().cloned() else {
            self.status = "No drive selected".to_owned();
            self.log(self.status.clone());
            return;
        };
        if !drive.root.is_dir() {
            self.status = "Selected drive root is not accessible".to_owned();
            self.log(self.status.clone());
            return;
        }
        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.cancel = Some(cancel);
        self.running = true;
        self.active_task = Some(StorageHealthTask::Benchmark);
        self.progress = 0.0;
        self.eta_text = "ETA: estimating".to_owned();
        self.status = format!("Running quick read/write benchmark on {}", drive.label);
        self.log("Quick health benchmark writes a temporary 256 MiB file, then cleans it up.");

        thread::spawn(move || {
            let (drive_tx, drive_rx) = mpsc::channel();
            let forward_tx = tx.clone();
            let forwarder = thread::spawn(move || {
                while let Ok(event) = drive_rx.recv() {
                    match event {
                        DriveWorkerEvent::Progress(progress) => {
                            let _ = forward_tx.send(StorageHealthEvent::ScanProgress(
                                StorageScanProgress {
                                    mode: StorageScanMode::Quick,
                                    regions_done: (progress.suite_progress * 100.0) as usize,
                                    regions_total: 100,
                                    bytes_scanned: progress.bytes_processed,
                                    read_errors: 0,
                                    slow_regions: 0,
                                    elapsed_s: progress.elapsed_s,
                                    eta_s: progress.eta_s,
                                },
                            ));
                        }
                        DriveWorkerEvent::Log(message) => {
                            let _ = forward_tx.send(StorageHealthEvent::Log(message));
                        }
                        DriveWorkerEvent::Done(_) => {}
                    }
                }
            });

            let config = DriveBenchmarkConfig {
                target_folder: drive.root.clone(),
                file_size_bytes: 256 * 1024 * 1024,
                profile: DriveProfile::Quick,
                selected_tests: vec![
                    DriveTestKind::SequentialWrite,
                    DriveTestKind::SequentialRead,
                    DriveTestKind::RandomWrite4K,
                    DriveTestKind::RandomRead4K,
                ],
            };
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                run_drive_benchmark(config, worker_cancel, drive_tx)
            }))
            .map_err(|panic| {
                format!(
                    "Storage health quick benchmark panicked: {}",
                    panic_message(&*panic)
                )
            })
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = forwarder.join();
            let _ = tx.send(StorageHealthEvent::BenchmarkDone(result));
        });
    }

    fn cancel(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
            self.status = "Cancel requested; stopping storage health task...".to_owned();
            self.log("Cancel requested for storage health task");
        }
    }

    fn export_report(&mut self) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            self.status = "Refresh health data before exporting a report".to_owned();
            self.log(self.status.clone());
            return;
        };
        match export_storage_health_report(
            snapshot,
            self.scan_result.as_ref(),
            &self.benchmark_results,
        ) {
            Ok(path) => {
                self.status = format!("Exported report to {}", path.display());
                self.last_report_path = Some(path.clone());
                self.log(self.status.clone());
            }
            Err(err) => {
                self.status = format!("Report export failed: {err:#}");
                self.log(self.status.clone());
            }
        }
    }

    fn poll_worker_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                StorageHealthEvent::Snapshot(result) => {
                    self.running = false;
                    self.active_task = None;
                    self.cancel = None;
                    self.progress = 1.0;
                    self.eta_text.clear();
                    match result {
                        Ok(snapshot) => {
                            self.status =
                                format!("Health refresh complete: {}", snapshot.status.label());
                            self.log(format!(
                                "Health refresh complete for {}: {} warning(s)",
                                snapshot.drive_label,
                                snapshot.warnings.len()
                            ));
                            self.snapshot = Some(snapshot);
                        }
                        Err(err) => {
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
                StorageHealthEvent::ScanProgress(progress) => {
                    self.progress = if progress.regions_total > 0 {
                        progress.regions_done as f32 / progress.regions_total as f32
                    } else {
                        0.0
                    }
                    .clamp(0.0, 1.0);
                    self.eta_text = format_eta(progress.eta_s);
                    self.status = format!(
                        "{}: {}/{} region(s), {}, {} read error(s), {} slow region(s), elapsed {}",
                        progress.mode,
                        progress.regions_done,
                        progress.regions_total,
                        format_bytes(progress.bytes_scanned),
                        progress.read_errors,
                        progress.slow_regions,
                        format_elapsed(progress.elapsed_s)
                    );
                }
                StorageHealthEvent::ScanDone(result) => {
                    self.running = false;
                    self.active_task = None;
                    self.cancel = None;
                    self.eta_text.clear();
                    match result {
                        Ok(result) => {
                            self.progress = 1.0;
                            self.status = format!(
                                "{} complete: {} read error(s), {} slow region(s)",
                                result.mode, result.read_errors, result.slow_regions
                            );
                            self.log(self.status.clone());
                            self.scan_result = Some(result);
                        }
                        Err(err) => {
                            if err.to_ascii_lowercase().contains("canceled") {
                                self.progress = 0.0;
                                self.eta_text = "ETA: canceled".to_owned();
                            }
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
                StorageHealthEvent::BenchmarkDone(result) => {
                    self.running = false;
                    self.active_task = None;
                    self.cancel = None;
                    self.eta_text.clear();
                    match result {
                        Ok(results) => {
                            self.progress = 1.0;
                            self.status =
                                format!("Quick benchmark complete: {} result(s)", results.len());
                            self.log(self.status.clone());
                            self.benchmark_results.extend(results);
                        }
                        Err(err) => {
                            if err.to_ascii_lowercase().contains("canceled") {
                                self.progress = 0.0;
                                self.eta_text = "ETA: canceled".to_owned();
                            }
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
                StorageHealthEvent::Log(message) => self.log(message),
            }
        }
    }
}

