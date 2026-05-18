#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DriveProfile {
    Quick,
    Balanced,
    Thorough,
}

impl DriveProfile {
    const ALL: [DriveProfile; 3] = [
        DriveProfile::Quick,
        DriveProfile::Balanced,
        DriveProfile::Thorough,
    ];

    fn label(self) -> &'static str {
        match self {
            DriveProfile::Quick => "Quick",
            DriveProfile::Balanced => "Balanced",
            DriveProfile::Thorough => "Thorough",
        }
    }

    fn target_duration(self) -> Duration {
        match self {
            DriveProfile::Quick => Duration::from_secs(4),
            DriveProfile::Balanced => Duration::from_secs(8),
            DriveProfile::Thorough => Duration::from_secs(15),
        }
    }
}

impl fmt::Display for DriveProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DriveFileSize {
    Auto,
    Mib256,
    Mib512,
    Gib1,
    Gib4,
    Gib8,
}

impl DriveFileSize {
    const ALL: [DriveFileSize; 6] = [
        DriveFileSize::Auto,
        DriveFileSize::Mib256,
        DriveFileSize::Mib512,
        DriveFileSize::Gib1,
        DriveFileSize::Gib4,
        DriveFileSize::Gib8,
    ];

    fn label(self) -> &'static str {
        match self {
            DriveFileSize::Auto => "Auto",
            DriveFileSize::Mib256 => "256 MiB",
            DriveFileSize::Mib512 => "512 MiB",
            DriveFileSize::Gib1 => "1 GiB",
            DriveFileSize::Gib4 => "4 GiB",
            DriveFileSize::Gib8 => "8 GiB",
        }
    }

    fn bytes(self, profile: DriveProfile) -> u64 {
        match self {
            DriveFileSize::Auto => auto_drive_file_size(profile),
            DriveFileSize::Mib256 => 256 * 1024 * 1024,
            DriveFileSize::Mib512 => 512 * 1024 * 1024,
            DriveFileSize::Gib1 => 1024 * 1024 * 1024,
            DriveFileSize::Gib4 => 4 * 1024 * 1024 * 1024,
            DriveFileSize::Gib8 => 8 * 1024 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DriveTestKind {
    SequentialRead,
    SequentialWrite,
    RandomRead4K,
    RandomWrite4K,
}

impl DriveTestKind {
    fn label(self) -> &'static str {
        match self {
            DriveTestKind::SequentialRead => "Sequential read",
            DriveTestKind::SequentialWrite => "Sequential write",
            DriveTestKind::RandomRead4K => "Random 4 KiB read",
            DriveTestKind::RandomWrite4K => "Random 4 KiB write",
        }
    }

    fn is_read(self) -> bool {
        matches!(
            self,
            DriveTestKind::SequentialRead | DriveTestKind::RandomRead4K
        )
    }

    fn is_write(self) -> bool {
        matches!(
            self,
            DriveTestKind::SequentialWrite | DriveTestKind::RandomWrite4K
        )
    }
}

impl fmt::Display for DriveTestKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DriveIoMode {
    Direct,
    Cached,
}

impl DriveIoMode {
    fn label(self) -> &'static str {
        match self {
            DriveIoMode::Direct => "Direct I/O",
            DriveIoMode::Cached => "Cached I/O",
        }
    }
}

struct DriveOpenFile {
    file: File,
    io_mode: DriveIoMode,
    fallback_note: Option<String>,
}

struct AlignedBuffer {
    storage: Vec<u8>,
    offset: usize,
    len: usize,
}

impl AlignedBuffer {
    fn new(len: usize, alignment: usize) -> Self {
        let alignment = alignment.max(1);
        let storage = vec![0_u8; len + alignment];
        let ptr = storage.as_ptr() as usize;
        let offset = (alignment - (ptr % alignment)) % alignment;
        Self {
            storage,
            offset,
            len,
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn as_slice(&self) -> &[u8] {
        &self.storage[self.offset..self.offset + self.len]
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.storage[self.offset..self.offset + self.len]
    }
}

#[derive(Clone, Debug)]
struct DriveInfo {
    root: PathBuf,
    label: String,
    device_name: Option<String>,
}

impl DriveInfo {
    fn with_device_name(root: PathBuf, device_name: Option<String>) -> Self {
        let clean_name = device_name.and_then(|name| {
            let name = name.trim();
            (!name.is_empty()).then(|| name.to_owned())
        });
        let label = clean_name
            .as_ref()
            .map(|name| format!("{} - {}", root.display(), name))
            .unwrap_or_else(|| format!("{} drive", root.display()));
        Self {
            root,
            label,
            device_name: clean_name,
        }
    }
}

#[derive(Clone, Debug)]
struct DriveBenchmarkConfig {
    target_folder: PathBuf,
    file_size_bytes: u64,
    profile: DriveProfile,
    selected_tests: Vec<DriveTestKind>,
}

#[derive(Clone, Debug)]
struct DriveBenchmarkResult {
    test: DriveTestKind,
    read_mbps: Option<f64>,
    write_mbps: Option<f64>,
    iops: Option<f64>,
    avg_latency_ms: Option<f64>,
    p95_latency_ms: Option<f64>,
    duration_ms: f64,
    file_size_bytes: u64,
    io_mode: DriveIoMode,
    notes: Vec<String>,
    ssd_temperature: TemperatureSummary,
}

#[derive(Clone, Debug)]
struct DriveProgress {
    current_test: String,
    current_progress: f32,
    suite_progress: f32,
    elapsed_s: f64,
    eta_s: Option<f64>,
    bytes_processed: u64,
    operations: u64,
}

#[derive(Debug)]
enum DriveWorkerEvent {
    Progress(DriveProgress),
    Log(String),
    Done(Result<Vec<DriveBenchmarkResult>, String>),
}

struct DriveBenchmarkState {
    drives: Vec<DriveInfo>,
    selected_drive: usize,
    target_folder_text: String,
    profile: DriveProfile,
    file_size: DriveFileSize,
    run_seq_read: bool,
    run_seq_write: bool,
    run_random_read: bool,
    run_random_write: bool,
    results: Vec<DriveBenchmarkResult>,
    log: Vec<String>,
    status: String,
    current_progress: f32,
    suite_progress: f32,
    eta_text: String,
    rx: Receiver<DriveWorkerEvent>,
    tx: Sender<DriveWorkerEvent>,
    cancel: Option<Arc<AtomicBool>>,
    running: bool,
}

impl DriveBenchmarkState {
    fn with_drives(drives: Vec<DriveInfo>) -> Self {
        let (tx, rx) = mpsc::channel();
        let target_folder = std::env::temp_dir();
        let selected_drive = selected_drive_for_path(&drives, &target_folder).unwrap_or(0);

        Self {
            drives,
            selected_drive,
            target_folder_text: target_folder.display().to_string(),
            profile: DriveProfile::Quick,
            file_size: DriveFileSize::Auto,
            run_seq_read: true,
            run_seq_write: true,
            run_random_read: true,
            run_random_write: true,
            results: Vec::new(),
            log: vec!["Drive benchmark tool ready".to_owned()],
            status: "Ready".to_owned(),
            current_progress: 0.0,
            suite_progress: 0.0,
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

    fn selected_tests(&self) -> Vec<DriveTestKind> {
        let mut tests = Vec::new();
        if self.run_seq_read {
            tests.push(DriveTestKind::SequentialRead);
        }
        if self.run_seq_write {
            tests.push(DriveTestKind::SequentialWrite);
        }
        if self.run_random_read {
            tests.push(DriveTestKind::RandomRead4K);
        }
        if self.run_random_write {
            tests.push(DriveTestKind::RandomWrite4K);
        }
        tests
    }

    fn planned_file_size(&self) -> u64 {
        self.file_size.bytes(self.profile)
    }

    fn planned_write_bytes(&self) -> u64 {
        let tests = self.selected_tests();
        let write_tests = tests.iter().filter(|test| test.is_write()).count() as u64;
        self.planned_file_size().saturating_mul(write_tests)
    }

    fn selected_drive_label(&self) -> String {
        self.drives
            .get(self.selected_drive)
            .map(|drive| drive.label.clone())
            .unwrap_or_else(|| "No drives detected".to_owned())
    }

    fn selected_drive_device_name(&self) -> Option<&str> {
        self.drives
            .get(self.selected_drive)
            .and_then(|drive| drive.device_name.as_deref())
    }

    fn select_drive(&mut self, index: usize) {
        if let Some(drive) = self.drives.get(index) {
            self.selected_drive = index;
            self.target_folder_text = drive.root.display().to_string();
            self.status = format!("Selected {}", drive.label);
        }
    }

    fn refresh_drives(&mut self) {
        self.drives = detect_drives();
        let target_folder = PathBuf::from(self.target_folder_text.trim());
        self.selected_drive = selected_drive_for_path(&self.drives, &target_folder).unwrap_or(0);
        self.log(format!("Detected {} drive(s)", self.drives.len()));
    }

    fn sync_selected_drive_to_target(&mut self) {
        let target_folder = PathBuf::from(self.target_folder_text.trim());
        if let Some(index) = selected_drive_for_path(&self.drives, &target_folder) {
            self.selected_drive = index;
        }
    }

    fn start(&mut self) {
        if self.running {
            return;
        }

        let target_folder = PathBuf::from(self.target_folder_text.trim());
        if target_folder.as_os_str().is_empty() {
            self.status = "Choose a target folder".to_owned();
            self.log("Drive benchmark target folder is empty");
            return;
        }
        if !target_folder.is_dir() {
            self.status = "Target folder does not exist".to_owned();
            self.log(format!(
                "Target folder does not exist: {}",
                target_folder.display()
            ));
            return;
        }

        let selected_tests = self.selected_tests();
        if selected_tests.is_empty() {
            self.status = "Select at least one drive test".to_owned();
            self.log("No drive tests selected");
            return;
        }

        let config = DriveBenchmarkConfig {
            target_folder,
            file_size_bytes: self.planned_file_size(),
            profile: self.profile,
            selected_tests,
        };

        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.cancel = Some(cancel);
        self.running = true;
        self.current_progress = 0.0;
        self.suite_progress = 0.0;
        self.eta_text = "ETA: estimating".to_owned();
        self.status = "Running drive benchmark...".to_owned();
        self.log(format!(
            "Starting drive benchmark in {} with {} test file using {} profile",
            config.target_folder.display(),
            format_bytes(config.file_size_bytes),
            config.profile
        ));

        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                run_drive_benchmark(config, worker_cancel, tx.clone())
            }))
            .map_err(|panic| format!("Drive benchmark panicked: {}", panic_message(&*panic)))
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(DriveWorkerEvent::Done(result));
        });
    }

    fn cancel(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
            self.status = "Cancel requested; stopping drive benchmark...".to_owned();
            self.log("Cancel requested for drive benchmark");
        }
    }

    fn poll_worker_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                DriveWorkerEvent::Progress(progress) => {
                    self.current_progress = progress.current_progress;
                    self.suite_progress = progress.suite_progress;
                    self.eta_text = format_eta(progress.eta_s);
                    self.status = format!(
                        "{} - {} processed, {} op(s), elapsed {}",
                        progress.current_test,
                        format_bytes(progress.bytes_processed),
                        progress.operations,
                        format_elapsed(progress.elapsed_s)
                    );
                }
                DriveWorkerEvent::Log(message) => self.log(message),
                DriveWorkerEvent::Done(result) => {
                    self.running = false;
                    self.cancel = None;
                    self.eta_text.clear();
                    match result {
                        Ok(results) => {
                            self.current_progress = 1.0;
                            self.suite_progress = 1.0;
                            self.status =
                                format!("Drive benchmark complete: {} result(s)", results.len());
                            self.log(self.status.clone());
                            self.results.extend(results);
                        }
                        Err(err) => {
                            if err.to_ascii_lowercase().contains("canceled") {
                                self.current_progress = 0.0;
                                self.eta_text = "ETA: canceled".to_owned();
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

