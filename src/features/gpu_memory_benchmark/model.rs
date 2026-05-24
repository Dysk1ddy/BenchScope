const GPU_MEMORY_MIB: u64 = 1024 * 1024;
const GPU_MEMORY_GIB: u64 = 1024 * GPU_MEMORY_MIB;
const GPU_MEMORY_DEFAULT_FALLBACK_BYTES: u64 = 256 * GPU_MEMORY_MIB;
const GPU_MEMORY_AUTO_MAX_BYTES: u64 = 512 * GPU_MEMORY_MIB;
const GPU_MEMORY_AUTO_DIVISOR: u64 = 8;
const GPU_MEMORY_MIN_BUFFER_BYTES: u64 = 16 * 1024 * 1024;
const GPU_MEMORY_PATTERN_CHUNK_BYTES: usize = 8 * 1024 * 1024;
const GPU_MEMORY_SAMPLE_BYTES: u64 = 256;
const GPU_MEMORY_WORKGROUP_SIZE: u32 = 256;
const GPU_MEMORY_MAX_ITERATIONS: u32 = 50;
const GPU_MEMORY_PATTERN_A_SEED: u32 = 0xA341_316C;
const GPU_MEMORY_PATTERN_B_SEED: u32 = 0xC801_3EA4;
const GPU_MEMORY_INTERNAL_ADDEND: u32 = 0x9E37_79B9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GpuMemoryTestKind {
    InternalReadWrite,
    DeviceCopy,
    Upload,
    Readback,
    RoundTrip,
}

impl GpuMemoryTestKind {
    fn label(self) -> &'static str {
        match self {
            GpuMemoryTestKind::InternalReadWrite => "Internal read/write",
            GpuMemoryTestKind::DeviceCopy => "GPU buffer copy",
            GpuMemoryTestKind::Upload => "CPU -> GPU upload",
            GpuMemoryTestKind::Readback => "GPU -> CPU readback",
            GpuMemoryTestKind::RoundTrip => "Round trip",
        }
    }

    fn description(self) -> &'static str {
        match self {
            GpuMemoryTestKind::InternalReadWrite => {
                "Compute shader streams through GPU-resident buffers; closest to raw GPU memory bandwidth."
            }
            GpuMemoryTestKind::DeviceCopy => {
                "Copies one GPU buffer into another through the GPU copy path."
            }
            GpuMemoryTestKind::Upload => {
                "Uploads deterministic host data into a GPU buffer with queue.write_buffer."
            }
            GpuMemoryTestKind::Readback => {
                "Copies GPU data into a mappable buffer and waits for CPU visibility."
            }
            GpuMemoryTestKind::RoundTrip => {
                "Uploads host data, copies it through the GPU, then maps it back on the CPU."
            }
        }
    }

    fn needs_storage_binding(self) -> bool {
        matches!(self, GpuMemoryTestKind::InternalReadWrite)
    }

    fn bytes_per_iteration(self, buffer_size_bytes: u64) -> u64 {
        match self {
            GpuMemoryTestKind::InternalReadWrite => {
                let vec4_count = gpu_memory_vec4_count(buffer_size_bytes);
                vec4_count.saturating_mul(48)
            }
            GpuMemoryTestKind::DeviceCopy
            | GpuMemoryTestKind::Upload
            | GpuMemoryTestKind::Readback => buffer_size_bytes,
            GpuMemoryTestKind::RoundTrip => buffer_size_bytes.saturating_mul(2),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GpuMemoryBufferSize {
    Auto,
    Mib64,
    Mib256,
    Mib512,
    Gib1,
    Gib2,
}

impl GpuMemoryBufferSize {
    const ALL: [GpuMemoryBufferSize; 6] = [
        GpuMemoryBufferSize::Auto,
        GpuMemoryBufferSize::Mib64,
        GpuMemoryBufferSize::Mib256,
        GpuMemoryBufferSize::Mib512,
        GpuMemoryBufferSize::Gib1,
        GpuMemoryBufferSize::Gib2,
    ];

    fn label(self) -> &'static str {
        match self {
            GpuMemoryBufferSize::Auto => "Auto",
            GpuMemoryBufferSize::Mib64 => "64 MiB",
            GpuMemoryBufferSize::Mib256 => "256 MiB",
            GpuMemoryBufferSize::Mib512 => "512 MiB",
            GpuMemoryBufferSize::Gib1 => "1 GiB",
            GpuMemoryBufferSize::Gib2 => "2 GiB",
        }
    }

    fn bytes(self, adapter: Option<&AdapterInfo>) -> u64 {
        match self {
            GpuMemoryBufferSize::Auto => gpu_memory_auto_buffer_size(adapter),
            GpuMemoryBufferSize::Mib64 => 64 * GPU_MEMORY_MIB,
            GpuMemoryBufferSize::Mib256 => 256 * GPU_MEMORY_MIB,
            GpuMemoryBufferSize::Mib512 => 512 * GPU_MEMORY_MIB,
            GpuMemoryBufferSize::Gib1 => GPU_MEMORY_GIB,
            GpuMemoryBufferSize::Gib2 => 2 * GPU_MEMORY_GIB,
        }
    }

    #[cfg(test)]
    fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase().replace(' ', "");
        match normalized.as_str() {
            "auto" => Some(GpuMemoryBufferSize::Auto),
            "64m" | "64mb" | "64mib" => Some(GpuMemoryBufferSize::Mib64),
            "256m" | "256mb" | "256mib" => Some(GpuMemoryBufferSize::Mib256),
            "512m" | "512mb" | "512mib" => Some(GpuMemoryBufferSize::Mib512),
            "1g" | "1gb" | "1gib" => Some(GpuMemoryBufferSize::Gib1),
            "2g" | "2gb" | "2gib" => Some(GpuMemoryBufferSize::Gib2),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GpuMemoryTimingSource {
    GpuTimestamp,
    CpuObserved,
}

impl GpuMemoryTimingSource {
    fn label(self) -> &'static str {
        match self {
            GpuMemoryTimingSource::GpuTimestamp => "GPU timestamp",
            GpuMemoryTimingSource::CpuObserved => "CPU observed",
        }
    }
}

#[derive(Clone, Debug)]
struct GpuMemoryBenchmarkConfig {
    adapter: AdapterInfo,
    requested_buffer_size_bytes: u64,
    iterations: u32,
    selected_tests: Vec<GpuMemoryTestKind>,
}

#[derive(Clone, Debug)]
struct GpuMemoryBenchmarkResult {
    test: GpuMemoryTestKind,
    adapter: String,
    buffer_size_bytes: u64,
    iterations: u32,
    bytes_processed: u64,
    elapsed_ms: f64,
    best_bandwidth_gbps: f64,
    average_bandwidth_gbps: f64,
    timing_source: GpuMemoryTimingSource,
    validation: String,
    notes: Vec<String>,
    gpu_temperature: TemperatureSummary,
}

#[derive(Clone, Debug)]
struct GpuMemoryProgress {
    current_test: String,
    current_progress: f32,
    suite_progress: f32,
    elapsed_s: f64,
    eta_s: Option<f64>,
    bytes_processed: u64,
}

#[derive(Debug)]
enum GpuMemoryWorkerEvent {
    Progress(GpuMemoryProgress),
    Log(String),
    Done(Result<Vec<GpuMemoryBenchmarkResult>, String>),
}

struct GpuMemoryBenchmarkState {
    buffer_size: GpuMemoryBufferSize,
    iterations: u32,
    run_internal_read_write: bool,
    run_device_copy: bool,
    run_upload: bool,
    run_readback: bool,
    run_round_trip: bool,
    results: Vec<GpuMemoryBenchmarkResult>,
    log: Vec<String>,
    status: String,
    current_progress: f32,
    suite_progress: f32,
    eta_text: String,
    timeline_current_test: String,
    timeline_elapsed_s: f64,
    timeline_bytes_processed: u64,
    rx: Receiver<GpuMemoryWorkerEvent>,
    tx: Sender<GpuMemoryWorkerEvent>,
    cancel: Option<Arc<AtomicBool>>,
    running: bool,
}

impl GpuMemoryBenchmarkState {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            buffer_size: GpuMemoryBufferSize::Auto,
            iterations: 5,
            run_internal_read_write: true,
            run_device_copy: true,
            run_upload: true,
            run_readback: true,
            run_round_trip: false,
            results: Vec::new(),
            log: vec!["GPU memory bandwidth benchmark ready".to_owned()],
            status: "Ready".to_owned(),
            current_progress: 0.0,
            suite_progress: 0.0,
            eta_text: String::new(),
            timeline_current_test: String::new(),
            timeline_elapsed_s: 0.0,
            timeline_bytes_processed: 0,
            rx,
            tx,
            cancel: None,
            running: false,
        }
    }

    fn log(&mut self, message: impl Into<String>) {
        self.log.push(message.into());
    }

    fn planned_buffer_size(&self, adapter: Option<&AdapterInfo>) -> u64 {
        self.buffer_size.bytes(adapter)
    }

    fn selected_tests(&self) -> Vec<GpuMemoryTestKind> {
        let mut tests = Vec::new();
        if self.run_internal_read_write {
            tests.push(GpuMemoryTestKind::InternalReadWrite);
        }
        if self.run_device_copy {
            tests.push(GpuMemoryTestKind::DeviceCopy);
        }
        if self.run_upload {
            tests.push(GpuMemoryTestKind::Upload);
        }
        if self.run_readback {
            tests.push(GpuMemoryTestKind::Readback);
        }
        if self.run_round_trip {
            tests.push(GpuMemoryTestKind::RoundTrip);
        }
        tests
    }

    fn start(&mut self, adapter: AdapterInfo) {
        if self.running {
            return;
        }

        let selected_tests = self.selected_tests();
        if selected_tests.is_empty() {
            self.status = "Select at least one GPU memory test".to_owned();
            self.log(self.status.clone());
            return;
        }

        let requested_buffer_size_bytes = self.planned_buffer_size(Some(&adapter));
        let iterations = self.iterations.clamp(1, GPU_MEMORY_MAX_ITERATIONS);
        let config = GpuMemoryBenchmarkConfig {
            adapter,
            requested_buffer_size_bytes,
            iterations,
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
        self.timeline_current_test.clear();
        self.timeline_elapsed_s = 0.0;
        self.timeline_bytes_processed = 0;
        self.status = "Running GPU memory benchmark...".to_owned();
        self.log(format!(
            "Starting GPU memory benchmark on {} with requested buffer {}, {} iteration(s)",
            config.adapter.label(),
            format_bytes(config.requested_buffer_size_bytes),
            iterations
        ));

        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                run_gpu_memory_benchmark(config, worker_cancel, tx.clone())
            }))
            .map_err(|panic| format!("GPU memory benchmark panicked: {}", panic_message(&*panic)))
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(GpuMemoryWorkerEvent::Done(result));
        });
    }

    fn cancel(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
            self.status = "Cancel requested; stopping GPU memory benchmark...".to_owned();
            self.log("Cancel requested for GPU memory benchmark");
        }
    }

    fn poll_worker_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                GpuMemoryWorkerEvent::Progress(progress) => {
                    self.current_progress = progress.current_progress;
                    self.suite_progress = progress.suite_progress;
                    self.eta_text = format_eta(progress.eta_s);
                    self.timeline_current_test = progress.current_test.clone();
                    self.timeline_elapsed_s = progress.elapsed_s;
                    self.timeline_bytes_processed = progress.bytes_processed;
                    self.status = format!(
                        "{} - {}, elapsed {}",
                        progress.current_test,
                        format_bytes(progress.bytes_processed),
                        format_elapsed(progress.elapsed_s)
                    );
                }
                GpuMemoryWorkerEvent::Log(message) => self.log(message),
                GpuMemoryWorkerEvent::Done(result) => {
                    self.running = false;
                    self.cancel = None;
                    self.eta_text.clear();
                    match result {
                        Ok(results) => {
                            self.current_progress = 1.0;
                            self.suite_progress = 1.0;
                            self.status = format!(
                                "GPU memory benchmark complete: {} result(s)",
                                results.len()
                            );
                            self.log(self.status.clone());
                            self.results.extend(results);
                        }
                        Err(err) => {
                            if err.to_ascii_lowercase().contains("canceled") {
                                self.current_progress = 0.0;
                                self.suite_progress = 0.0;
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

fn gpu_memory_auto_buffer_size(adapter: Option<&AdapterInfo>) -> u64 {
    let Some(adapter) = adapter else {
        return GPU_MEMORY_DEFAULT_FALLBACK_BYTES;
    };
    let Some((limit, _)) = adapter_memory_limit_bytes(adapter) else {
        return GPU_MEMORY_DEFAULT_FALLBACK_BYTES;
    };
    align_gpu_memory_buffer_size(
        (limit / GPU_MEMORY_AUTO_DIVISOR)
            .clamp(GPU_MEMORY_MIN_BUFFER_BYTES, GPU_MEMORY_AUTO_MAX_BYTES),
    )
}

fn align_gpu_memory_buffer_size(bytes: u64) -> u64 {
    bytes.max(16) / 16 * 16
}

fn gpu_memory_vec4_count(buffer_size_bytes: u64) -> u64 {
    align_gpu_memory_buffer_size(buffer_size_bytes) / 16
}

fn gpu_memory_bandwidth_gbps(bytes_processed: u64, elapsed_ms: f64) -> f64 {
    if elapsed_ms <= 0.0 {
        return f64::INFINITY;
    }
    bytes_processed as f64 / (elapsed_ms / 1000.0) / 1_000_000_000.0
}

fn format_gpu_memory_bandwidth(value: f64) -> String {
    if value.is_infinite() {
        "inf".to_owned()
    } else if value >= 100.0 {
        format!("{value:.0}")
    } else if value >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

fn gpu_memory_pattern_word(word_index: u64, seed: u32) -> u32 {
    let mut value = (word_index as u32).wrapping_mul(0x7477_962D) ^ seed;
    value ^= value.rotate_left(13);
    value = value.wrapping_mul(0x85EB_CA6B);
    value ^ value.rotate_right(16)
}

fn gpu_memory_internal_word(word_index: u64) -> u32 {
    (gpu_memory_pattern_word(word_index, GPU_MEMORY_PATTERN_A_SEED)
        ^ gpu_memory_pattern_word(word_index, GPU_MEMORY_PATTERN_B_SEED))
    .wrapping_add(GPU_MEMORY_INTERNAL_ADDEND)
}
