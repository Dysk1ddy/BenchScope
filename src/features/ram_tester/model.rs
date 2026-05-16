#[derive(Clone, Copy, Debug, Default)]
struct RamMemoryInfo {
    total_physical_bytes: u64,
    available_physical_bytes: u64,
    memory_load_percent: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RamAllocation {
    Auto,
    Mib256,
    Mib512,
    Gib1,
    Gib2,
    Gib4,
    Gib8,
    Gib16,
    Gib32,
}

impl RamAllocation {
    const ALL: [RamAllocation; 9] = [
        RamAllocation::Auto,
        RamAllocation::Mib256,
        RamAllocation::Mib512,
        RamAllocation::Gib1,
        RamAllocation::Gib2,
        RamAllocation::Gib4,
        RamAllocation::Gib8,
        RamAllocation::Gib16,
        RamAllocation::Gib32,
    ];

    fn label(self) -> &'static str {
        match self {
            RamAllocation::Auto => "Auto safe",
            RamAllocation::Mib256 => "256 MiB",
            RamAllocation::Mib512 => "512 MiB",
            RamAllocation::Gib1 => "1 GiB",
            RamAllocation::Gib2 => "2 GiB",
            RamAllocation::Gib4 => "4 GiB",
            RamAllocation::Gib8 => "8 GiB",
            RamAllocation::Gib16 => "16 GiB",
            RamAllocation::Gib32 => "32 GiB",
        }
    }

    fn requested_bytes(self) -> Option<u64> {
        match self {
            RamAllocation::Auto => None,
            RamAllocation::Mib256 => Some(256 * 1024 * 1024),
            RamAllocation::Mib512 => Some(512 * 1024 * 1024),
            RamAllocation::Gib1 => Some(RAM_GIB_BYTES),
            RamAllocation::Gib2 => Some(2 * RAM_GIB_BYTES),
            RamAllocation::Gib4 => Some(4 * RAM_GIB_BYTES),
            RamAllocation::Gib8 => Some(8 * RAM_GIB_BYTES),
            RamAllocation::Gib16 => Some(16 * RAM_GIB_BYTES),
            RamAllocation::Gib32 => Some(32 * RAM_GIB_BYTES),
        }
    }

    fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase().replace(' ', "");
        match normalized.as_str() {
            "auto" | "autosafe" | "safe" => Some(RamAllocation::Auto),
            "256m" | "256mb" | "256mib" => Some(RamAllocation::Mib256),
            "512m" | "512mb" | "512mib" => Some(RamAllocation::Mib512),
            "1g" | "1gb" | "1gib" => Some(RamAllocation::Gib1),
            "2g" | "2gb" | "2gib" => Some(RamAllocation::Gib2),
            "4g" | "4gb" | "4gib" => Some(RamAllocation::Gib4),
            "8g" | "8gb" | "8gib" => Some(RamAllocation::Gib8),
            "16g" | "16gb" | "16gib" => Some(RamAllocation::Gib16),
            "32g" | "32gb" | "32gib" => Some(RamAllocation::Gib32),
            _ => None,
        }
    }
}

impl fmt::Display for RamAllocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RamTestStatus {
    Passed,
    Failed,
    TimeLimited,
}

impl RamTestStatus {
    fn label(self) -> &'static str {
        match self {
            RamTestStatus::Passed => "PASS",
            RamTestStatus::Failed => "FAIL",
            RamTestStatus::TimeLimited => "TIME LIMITED",
        }
    }
}

#[derive(Clone, Debug)]
struct RamFailure {
    test: String,
    pass: usize,
    byte_offset: u64,
    word_index: usize,
    expected: u64,
    actual: u64,
    diff: u64,
    failed_bit: Option<u32>,
    repeatable: bool,
}

#[derive(Clone, Debug)]
struct RamTestResult {
    status: RamTestStatus,
    tested_bytes: u64,
    installed_bytes: u64,
    available_at_start_bytes: u64,
    elapsed_ms: f64,
    budget_seconds: f64,
    checks: u64,
    error_count: usize,
    completed_phases: usize,
    total_phases: usize,
    first_failure: Option<RamFailure>,
    notes: Vec<String>,
}

#[derive(Clone, Debug)]
struct RamTestConfig {
    allocation: RamAllocation,
    memory_info: RamMemoryInfo,
}

#[derive(Clone, Debug)]
struct RamTestProgress {
    phase: String,
    pass: usize,
    progress: f32,
    elapsed_s: f64,
    eta_s: Option<f64>,
    tested_bytes: u64,
    checks: u64,
    errors: usize,
}

#[derive(Debug)]
enum RamWorkerEvent {
    Progress(RamTestProgress),
    Log(String),
    Done(Result<RamTestResult, String>),
}

struct RamTestState {
    memory_info: RamMemoryInfo,
    allocation: RamAllocation,
    results: Vec<RamTestResult>,
    log: Vec<String>,
    status: String,
    phase: String,
    progress: f32,
    eta_text: String,
    rx: Receiver<RamWorkerEvent>,
    tx: Sender<RamWorkerEvent>,
    cancel: Option<Arc<AtomicBool>>,
    running: bool,
}

impl RamTestState {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let memory_info = detect_ram_memory_info().unwrap_or_default();
        Self {
            memory_info,
            allocation: RamAllocation::Auto,
            results: Vec::new(),
            log: vec!["RAM tester ready".to_owned()],
            status: "Ready".to_owned(),
            phase: String::new(),
            progress: 0.0,
            eta_text: String::new(),
            rx,
            tx,
            cancel: None,
            running: false,
        }
    }

    fn log(&mut self, message: impl Into<String>) {
        self.log.push(message.into());
    }

    fn refresh_memory_info(&mut self) {
        match detect_ram_memory_info() {
            Ok(info) => {
                self.memory_info = info;
                self.status = format!(
                    "Memory refreshed: {} installed, {} available",
                    format_bytes(info.total_physical_bytes),
                    format_bytes(info.available_physical_bytes)
                );
                self.log(self.status.clone());
            }
            Err(err) => {
                self.status = format!("Could not read memory status: {err:#}");
                self.log(self.status.clone());
            }
        }
    }

    fn planned_bytes(&self) -> u64 {
        planned_ram_test_bytes(self.memory_info, self.allocation)
    }

    fn start(&mut self) {
        if self.running {
            return;
        }
        self.refresh_memory_info();
        if self.memory_info.total_physical_bytes == 0
            || self.memory_info.available_physical_bytes == 0
        {
            self.status = "Cannot start RAM test without memory status".to_owned();
            self.log(self.status.clone());
            return;
        }

        let planned_bytes = self.planned_bytes();
        if planned_bytes < RAM_MIN_TEST_BYTES {
            self.status = format!(
                "Not enough available physical memory for a useful RAM test ({})",
                format_bytes(planned_bytes)
            );
            self.log(self.status.clone());
            return;
        }

        let config = RamTestConfig {
            allocation: self.allocation,
            memory_info: self.memory_info,
        };
        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.cancel = Some(cancel);
        self.running = true;
        self.progress = 0.0;
        self.eta_text = "ETA: estimating".to_owned();
        self.phase = "Allocating test buffer".to_owned();
        self.status = format!(
            "Running RAM test against {}...",
            format_bytes(planned_bytes)
        );
        self.log(format!(
            "Starting Memtest-style RAM test: allocation {}, planned {}, budget {}",
            config.allocation,
            format_bytes(planned_bytes),
            format_elapsed(ram_time_budget_seconds(
                config.memory_info.total_physical_bytes
            ))
        ));

        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                run_ram_test(config, worker_cancel, tx.clone())
            }))
            .map_err(|panic| format!("RAM test panicked: {}", panic_message(&*panic)))
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(RamWorkerEvent::Done(result));
        });
    }

    fn cancel(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
            self.status = "Cancel requested; stopping RAM test...".to_owned();
            self.log("Cancel requested for RAM test");
        }
    }

    fn poll_worker_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                RamWorkerEvent::Progress(progress) => {
                    self.phase = if progress.pass > 0 {
                        format!("{} (pass {})", progress.phase, progress.pass)
                    } else {
                        progress.phase
                    };
                    self.progress = progress.progress;
                    self.eta_text = format_eta(progress.eta_s);
                    self.status = format!(
                        "{} - {} tested, {} check(s), {} error(s), elapsed {}",
                        self.phase,
                        format_bytes(progress.tested_bytes),
                        progress.checks,
                        progress.errors,
                        format_elapsed(progress.elapsed_s)
                    );
                }
                RamWorkerEvent::Log(message) => self.log(message),
                RamWorkerEvent::Done(result) => {
                    self.running = false;
                    self.cancel = None;
                    match result {
                        Ok(result) => {
                            self.progress = 1.0;
                            self.eta_text = "ETA: complete".to_owned();
                            self.status = format!(
                                "RAM test {}: {} error(s), {} checks, elapsed {}",
                                result.status.label(),
                                result.error_count,
                                result.checks,
                                format_ms(Some(result.elapsed_ms))
                            );
                            self.log(self.status.clone());
                            if let Some(failure) = &result.first_failure {
                                self.log(format!(
                                    "First RAM failure: {}",
                                    format_ram_failure(failure)
                                ));
                            }
                            self.results.push(result);
                        }
                        Err(err) => {
                            if err.to_ascii_lowercase().contains("canceled") {
                                self.progress = 0.0;
                                self.eta_text = "ETA: canceled".to_owned();
                            } else {
                                self.eta_text.clear();
                            }
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
            }
        }
    }
}
