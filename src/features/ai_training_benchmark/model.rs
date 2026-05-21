#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AiTrainingWorkload {
    LinearLayer,
    Mlp,
    TransformerBlock,
    OptimizerStress,
}

impl AiTrainingWorkload {
    const ALL: [AiTrainingWorkload; 4] = [
        AiTrainingWorkload::LinearLayer,
        AiTrainingWorkload::Mlp,
        AiTrainingWorkload::TransformerBlock,
        AiTrainingWorkload::OptimizerStress,
    ];

    fn label(self) -> &'static str {
        match self {
            AiTrainingWorkload::LinearLayer => "Linear layer training",
            AiTrainingWorkload::Mlp => "MLP training",
            AiTrainingWorkload::TransformerBlock => "Transformer training proxy",
            AiTrainingWorkload::OptimizerStress => "Optimizer stress",
        }
    }

    fn description_for_backend(self, backend: AiTrainingBackend) -> &'static str {
        match self {
            AiTrainingWorkload::LinearLayer => match backend {
                AiTrainingBackend::PyTorchCuda => {
                    "Single PyTorch linear layer with real forward, loss, backward, and AdamW update."
                }
                AiTrainingBackend::PortableWgpu => {
                    "Portable GEMM proxy: forward, weight-gradient, input-gradient, and SGD-style update kernels."
                }
            },
            AiTrainingWorkload::Mlp => match backend {
                AiTrainingBackend::PyTorchCuda => {
                    "PyTorch two-layer MLP with GELU, loss, backward pass, and AdamW update."
                }
                AiTrainingBackend::PortableWgpu => {
                    "Portable GEMM proxy: two dense training-shaped blocks with SGD-style updates."
                }
            },
            AiTrainingWorkload::TransformerBlock => match backend {
                AiTrainingBackend::PyTorchCuda => {
                    "PyTorch transformer block with layer norm, attention, MLP, loss, backward pass, and AdamW update."
                }
                AiTrainingBackend::PortableWgpu => {
                    "Portable GEMM proxy for transformer-like projection, attention, and MLP work."
                }
            }
            AiTrainingWorkload::OptimizerStress => {
                "Portable memory/update pressure pass over a large optimizer-style parameter set."
            }
        }
    }

    fn throughput_label(self) -> &'static str {
        match self {
            AiTrainingWorkload::LinearLayer | AiTrainingWorkload::Mlp => "samples/s",
            AiTrainingWorkload::TransformerBlock => "tokens/s",
            AiTrainingWorkload::OptimizerStress => "parameters/s",
        }
    }
}

impl fmt::Display for AiTrainingWorkload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AiTrainingBackend {
    PortableWgpu,
    PyTorchCuda,
}

impl AiTrainingBackend {
    const ALL: [AiTrainingBackend; 2] = [
        AiTrainingBackend::PyTorchCuda,
        AiTrainingBackend::PortableWgpu,
    ];

    fn label(self) -> &'static str {
        match self {
            AiTrainingBackend::PortableWgpu => "Portable wgpu proxy",
            AiTrainingBackend::PyTorchCuda => "PyTorch CUDA training",
        }
    }

    fn description(self) -> &'static str {
        match self {
            AiTrainingBackend::PortableWgpu => {
                "Cross-vendor fallback for synthetic training-shaped GEMM/update proxies. Use Matrix Stress for raw sustained GEMM."
            }
            AiTrainingBackend::PyTorchCuda => {
                "Primary AI path. Runs real PyTorch model training steps on a selected NVIDIA CUDA device."
            }
        }
    }
}

impl fmt::Display for AiTrainingBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AiTrainingProfile {
    Quick,
    Balanced,
    Thorough,
}

impl AiTrainingProfile {
    const ALL: [AiTrainingProfile; 3] = [
        AiTrainingProfile::Quick,
        AiTrainingProfile::Balanced,
        AiTrainingProfile::Thorough,
    ];

    fn label(self) -> &'static str {
        match self {
            AiTrainingProfile::Quick => "Quick",
            AiTrainingProfile::Balanced => "Balanced",
            AiTrainingProfile::Thorough => "Thorough",
        }
    }

    fn warmup_steps(self) -> usize {
        match self {
            AiTrainingProfile::Quick => 2,
            AiTrainingProfile::Balanced => 5,
            AiTrainingProfile::Thorough => 10,
        }
    }

    fn measured_steps(self) -> usize {
        match self {
            AiTrainingProfile::Quick => 5,
            AiTrainingProfile::Balanced => 20,
            AiTrainingProfile::Thorough => 60,
        }
    }

    fn time_limit_s(self) -> f64 {
        match self {
            AiTrainingProfile::Quick => 10.0,
            AiTrainingProfile::Balanced => 30.0,
            AiTrainingProfile::Thorough => 90.0,
        }
    }
}

impl fmt::Display for AiTrainingProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AiTrainingPrecision {
    F32,
    Bf16,
    F16,
}

impl AiTrainingPrecision {
    const ALL: [AiTrainingPrecision; 3] = [
        AiTrainingPrecision::F32,
        AiTrainingPrecision::Bf16,
        AiTrainingPrecision::F16,
    ];

    fn label(self) -> &'static str {
        match self {
            AiTrainingPrecision::F32 => "f32",
            AiTrainingPrecision::Bf16 => "bf16",
            AiTrainingPrecision::F16 => "f16",
        }
    }

    fn bytes_per_value(self) -> u64 {
        match self {
            AiTrainingPrecision::F32 => 4,
            AiTrainingPrecision::Bf16 => 2,
            AiTrainingPrecision::F16 => 2,
        }
    }
}

impl fmt::Display for AiTrainingPrecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AiTrainingPreset {
    Tiny,
    Small,
    Medium,
    Large,
    Rtx5090,
    Custom,
}

impl AiTrainingPreset {
    const ALL: [AiTrainingPreset; 6] = [
        AiTrainingPreset::Tiny,
        AiTrainingPreset::Small,
        AiTrainingPreset::Medium,
        AiTrainingPreset::Large,
        AiTrainingPreset::Rtx5090,
        AiTrainingPreset::Custom,
    ];

    fn label(self) -> &'static str {
        match self {
            AiTrainingPreset::Tiny => "Tiny",
            AiTrainingPreset::Small => "Small",
            AiTrainingPreset::Medium => "Medium",
            AiTrainingPreset::Large => "Large",
            AiTrainingPreset::Rtx5090 => "RTX 5090",
            AiTrainingPreset::Custom => "Custom",
        }
    }
}

impl fmt::Display for AiTrainingPreset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Debug)]
struct AiTrainingDimensions {
    batch_size: usize,
    input_dim: usize,
    output_dim: usize,
    sequence_len: usize,
    hidden_size: usize,
    attention_heads: usize,
    parameter_count: usize,
}

impl AiTrainingDimensions {
    fn for_preset(workload: AiTrainingWorkload, preset: AiTrainingPreset) -> Self {
        match workload {
            AiTrainingWorkload::LinearLayer => match preset {
                AiTrainingPreset::Tiny => Self::linear(128, 512, 512),
                AiTrainingPreset::Small => Self::linear(256, 1024, 1024),
                AiTrainingPreset::Medium => Self::linear(512, 2048, 2048),
                AiTrainingPreset::Large => Self::linear(1024, 4096, 4096),
                AiTrainingPreset::Rtx5090 => Self::linear(8192, 8192, 8192),
                AiTrainingPreset::Custom => Self::linear(256, 1024, 1024),
            },
            AiTrainingWorkload::Mlp => match preset {
                AiTrainingPreset::Tiny => Self::mlp(128, 512, 1024),
                AiTrainingPreset::Small => Self::mlp(256, 1024, 2048),
                AiTrainingPreset::Medium => Self::mlp(512, 2048, 4096),
                AiTrainingPreset::Large => Self::mlp(1024, 4096, 8192),
                AiTrainingPreset::Rtx5090 => Self::mlp(8192, 8192, 32768),
                AiTrainingPreset::Custom => Self::mlp(256, 1024, 2048),
            },
            AiTrainingWorkload::TransformerBlock => match preset {
                AiTrainingPreset::Tiny => Self::transformer(4, 128, 512, 8),
                AiTrainingPreset::Small => Self::transformer(4, 256, 768, 12),
                AiTrainingPreset::Medium => Self::transformer(2, 512, 1024, 16),
                AiTrainingPreset::Large => Self::transformer(1, 1024, 2048, 16),
                AiTrainingPreset::Rtx5090 => Self::transformer(1, 4096, 4096, 32),
                AiTrainingPreset::Custom => Self::transformer(4, 256, 768, 12),
            },
            AiTrainingWorkload::OptimizerStress => match preset {
                AiTrainingPreset::Tiny => Self::optimizer(16_000_000),
                AiTrainingPreset::Small => Self::optimizer(64_000_000),
                AiTrainingPreset::Medium => Self::optimizer(128_000_000),
                AiTrainingPreset::Large => Self::optimizer(256_000_000),
                AiTrainingPreset::Rtx5090 => Self::optimizer(1_000_000_000),
                AiTrainingPreset::Custom => Self::optimizer(64_000_000),
            },
        }
    }

    fn linear(batch_size: usize, input_dim: usize, output_dim: usize) -> Self {
        Self {
            batch_size,
            input_dim,
            output_dim,
            sequence_len: 1,
            hidden_size: input_dim.max(output_dim),
            attention_heads: 1,
            parameter_count: input_dim.saturating_mul(output_dim),
        }
    }

    fn mlp(batch_size: usize, hidden_size: usize, expansion_dim: usize) -> Self {
        Self {
            batch_size,
            input_dim: hidden_size,
            output_dim: expansion_dim,
            sequence_len: 1,
            hidden_size,
            attention_heads: 1,
            parameter_count: hidden_size
                .saturating_mul(expansion_dim)
                .saturating_add(expansion_dim.saturating_mul(hidden_size)),
        }
    }

    fn transformer(
        batch_size: usize,
        sequence_len: usize,
        hidden_size: usize,
        attention_heads: usize,
    ) -> Self {
        let mlp_dim = hidden_size.saturating_mul(4);
        let attention_params = hidden_size.saturating_mul(hidden_size).saturating_mul(4);
        let mlp_params = hidden_size
            .saturating_mul(mlp_dim)
            .saturating_add(mlp_dim.saturating_mul(hidden_size));
        Self {
            batch_size,
            input_dim: hidden_size,
            output_dim: mlp_dim,
            sequence_len,
            hidden_size,
            attention_heads: attention_heads.max(1),
            parameter_count: attention_params.saturating_add(mlp_params),
        }
    }

    fn optimizer(parameter_count: usize) -> Self {
        Self {
            batch_size: 1,
            input_dim: 1,
            output_dim: 1,
            sequence_len: 1,
            hidden_size: 1,
            attention_heads: 1,
            parameter_count,
        }
    }
}

#[derive(Clone, Debug)]
struct AiTrainingStepTimings {
    forward_loss_ms: f64,
    backward_ms: f64,
    optimizer_ms: f64,
}

#[derive(Clone, Debug)]
struct AiTrainingResult {
    backend: AiTrainingBackend,
    workload: AiTrainingWorkload,
    preset: AiTrainingPreset,
    precision: AiTrainingPrecision,
    gpu_names: Vec<String>,
    shape: String,
    flops_per_step: f64,
    measured_steps: usize,
    compute_tflops: Option<f64>,
    end_to_end_tflops: Option<f64>,
    throughput_value: Option<f64>,
    throughput_label: &'static str,
    avg_step_ms: Option<f64>,
    p95_step_ms: Option<f64>,
    step_timings: Option<AiTrainingStepTimings>,
    memory_bytes: u64,
    validation: String,
    notes: String,
}

#[derive(Clone, Debug)]
struct AiTrainingProgress {
    phase: String,
    progress: f32,
    elapsed_s: f64,
    eta_s: Option<f64>,
    completed_steps: usize,
    total_steps: usize,
}

#[derive(Clone, Debug)]
struct AiTrainingConfig {
    backend: AiTrainingBackend,
    pytorch_python: String,
    pytorch_cuda_device: usize,
    adapter: AdapterInfo,
    workload: AiTrainingWorkload,
    profile: AiTrainingProfile,
    precision: AiTrainingPrecision,
    preset: AiTrainingPreset,
    dimensions: AiTrainingDimensions,
    warmup_steps: usize,
    measured_steps: usize,
    time_limit_s: f64,
    gpu_intensity: GpuIntensity,
    validation_enabled: bool,
    smoke_test: bool,
}

#[derive(Debug)]
enum AiTrainingWorkerEvent {
    Progress(AiTrainingProgress),
    Log(String),
    PyTorchProbeDone(Result<PyTorchCudaEnvironment, String>),
    PyTorchInstallDone(Result<PyTorchCudaEnvironment, String>),
    Done(Result<AiTrainingResult, String>),
    BatchDone(Result<Vec<AiTrainingResult>, String>),
}

struct AiTrainingBenchmarkState {
    backend: AiTrainingBackend,
    workload: AiTrainingWorkload,
    profile: AiTrainingProfile,
    precision: AiTrainingPrecision,
    preset: AiTrainingPreset,
    dimensions: AiTrainingDimensions,
    warmup_steps: usize,
    measured_steps: usize,
    time_limit_s: f64,
    results: Vec<AiTrainingResult>,
    log: Vec<String>,
    status: String,
    progress: f32,
    phase: String,
    eta_text: String,
    rx: Receiver<AiTrainingWorkerEvent>,
    tx: Sender<AiTrainingWorkerEvent>,
    cancel: Option<Arc<AtomicBool>>,
    running: bool,
    pytorch_python: String,
    pytorch_cuda_device: usize,
    pytorch_probe: Option<PyTorchCudaEnvironment>,
    pytorch_probe_running: bool,
    pytorch_install_running: bool,
    pending_pytorch_install: bool,
}

impl AiTrainingBenchmarkState {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let workload = AiTrainingWorkload::LinearLayer;
        let profile = AiTrainingProfile::Quick;
        let preset = AiTrainingPreset::Small;
        Self {
            workload,
            profile,
            precision: AiTrainingPrecision::F32,
            preset,
            dimensions: AiTrainingDimensions::for_preset(workload, preset),
            warmup_steps: profile.warmup_steps(),
            measured_steps: profile.measured_steps(),
            time_limit_s: profile.time_limit_s(),
            results: Vec::new(),
            log: vec!["AI training GPU benchmark feature shell ready".to_owned()],
            status: "Ready".to_owned(),
            progress: 0.0,
            phase: "Not running".to_owned(),
            eta_text: String::new(),
            rx,
            tx,
            cancel: None,
            running: false,
            backend: AiTrainingBackend::PyTorchCuda,
            pytorch_python: default_pytorch_python_executable(),
            pytorch_cuda_device: 0,
            pytorch_probe: None,
            pytorch_probe_running: false,
            pytorch_install_running: false,
            pending_pytorch_install: false,
        }
    }

    fn set_backend(&mut self, backend: AiTrainingBackend) {
        if self.backend != backend {
            self.backend = backend;
            if backend == AiTrainingBackend::PyTorchCuda && !self.pytorch_cuda_can_run_selection() {
                self.workload = AiTrainingWorkload::LinearLayer;
                self.apply_preset();
                self.log("PyTorch CUDA selected; switched to a supported training workload");
            }
            self.log(format!("Selected backend: {}", backend.label()));
        }
    }

    fn set_workload(&mut self, workload: AiTrainingWorkload) {
        if self.workload != workload {
            self.workload = workload;
            if self.preset == AiTrainingPreset::Custom {
                self.preset = AiTrainingPreset::Small;
            }
            self.apply_preset();
            self.log(format!("Selected workload: {}", workload.label()));
        }
    }

    fn set_profile(&mut self, profile: AiTrainingProfile) {
        if self.profile != profile {
            self.profile = profile;
            self.warmup_steps = profile.warmup_steps();
            self.measured_steps = profile.measured_steps();
            self.time_limit_s = profile.time_limit_s();
            self.log(format!("Selected profile: {}", profile.label()));
        }
    }

    fn set_preset(&mut self, preset: AiTrainingPreset) {
        if self.preset != preset {
            self.preset = preset;
            if preset != AiTrainingPreset::Custom {
                self.apply_preset();
            }
            self.log(format!("Selected preset: {}", preset.label()));
        }
    }

    fn apply_preset(&mut self) {
        self.dimensions = AiTrainingDimensions::for_preset(self.workload, self.preset);
    }

    fn mark_custom(&mut self) {
        self.preset = AiTrainingPreset::Custom;
    }

    fn log(&mut self, message: impl Into<String>) {
        self.log.push(message.into());
    }

    fn config(&self, adapter: AdapterInfo, gpu_intensity: GpuIntensity) -> AiTrainingConfig {
        AiTrainingConfig {
            backend: self.backend,
            pytorch_python: self.pytorch_python.trim().to_owned(),
            pytorch_cuda_device: self.pytorch_cuda_device,
            adapter,
            workload: self.workload,
            profile: self.profile,
            precision: self.precision,
            preset: self.preset,
            dimensions: self.dimensions.clone(),
            warmup_steps: self.warmup_steps,
            measured_steps: self.measured_steps,
            time_limit_s: self.time_limit_s,
            gpu_intensity,
            validation_enabled: true,
            smoke_test: false,
        }
    }

    fn can_run(&self) -> bool {
        match self.backend {
            AiTrainingBackend::PortableWgpu => {
                self.precision == AiTrainingPrecision::F32
                    && self.measured_steps > 0
                    && !self.running
                    && !self.pytorch_probe_running
                    && !self.pytorch_install_running
            }
            AiTrainingBackend::PyTorchCuda => {
                self.pytorch_cuda_can_run_selection()
                    && self.measured_steps > 0
                    && !self.running
                    && !self.pytorch_probe_running
                    && !self.pytorch_install_running
                    && !self.pytorch_python.trim().is_empty()
                    && self.pytorch_cuda_ready()
            }
        }
    }

    fn pytorch_cuda_can_run_selection(&self) -> bool {
        matches!(
            self.workload,
            AiTrainingWorkload::LinearLayer
                | AiTrainingWorkload::Mlp
                | AiTrainingWorkload::TransformerBlock
        )
    }

    fn pytorch_cuda_ready(&self) -> bool {
        self.pytorch_probe.as_ref().is_some_and(|environment| {
            environment.cuda_available
                && pytorch_cuda_environment_has_device(environment, self.pytorch_cuda_device)
        })
    }

    fn selected_pytorch_cuda_memory_bytes(&self) -> Option<u64> {
        self.pytorch_probe.as_ref().and_then(|environment| {
            environment
                .devices
                .iter()
                .find(|device| device.index == self.pytorch_cuda_device)
                .map(|device| device.total_memory_bytes)
        })
    }

    fn start(&mut self, adapter: AdapterInfo, gpu_intensity: GpuIntensity) {
        if self.running {
            return;
        }
        if self.backend == AiTrainingBackend::PyTorchCuda && !self.pytorch_cuda_can_run_selection() {
            self.status =
                "PyTorch CUDA supports linear, MLP, and transformer training workloads".to_owned();
            self.log(self.status.clone());
            return;
        }
        if self.backend == AiTrainingBackend::PortableWgpu
            && self.precision != AiTrainingPrecision::F32
        {
            self.status = "Portable wgpu currently supports f32 precision".to_owned();
            self.log(self.status.clone());
            return;
        }
        if self.measured_steps == 0 {
            self.status = "Measured steps must be at least 1".to_owned();
            self.log(self.status.clone());
            return;
        }

        let mut config = self.config(adapter, gpu_intensity);
        let adapter_memory_limit = if config.backend == AiTrainingBackend::PyTorchCuda {
            self.selected_pytorch_cuda_memory_bytes()
        } else {
            adapter_memory_limit_bytes(&config.adapter).map(|(bytes, _)| bytes)
        };
        let mut sizing_notes = Vec::new();
        if let Some(note) = auto_size_ai_training_config_for_limits(
            &mut config,
            adapter_memory_limit,
            None,
        ) {
            sizing_notes.push(note);
        }
        let memory_bytes = config_memory_bytes(&config);
        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.cancel = Some(cancel);
        self.running = true;
        self.progress = 0.0;
        self.phase = "Starting AI training benchmark".to_owned();
        self.eta_text = "ETA: estimating".to_owned();
        self.status = "Running AI training benchmark...".to_owned();
        let target_label = if config.backend == AiTrainingBackend::PyTorchCuda {
            format!("CUDA device {}", config.pytorch_cuda_device)
        } else {
            config.adapter.label()
        };
        self.log(format!(
            "Starting {} {} on {} using {} preset, {} precision, {} profile",
            config.backend,
            config.workload,
            target_label,
            config.preset,
            config.precision,
            config.profile
        ));
        self.log(format!(
            "Estimated workload memory: {}; FLOPs per step: {}",
            format_bytes(memory_bytes),
            format_flops_per_step(config_flops_per_step(&config))
        ));
        for note in &sizing_notes {
            self.log(note.clone());
        }

        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                run_ai_training_benchmark(config, worker_cancel, tx.clone())
            }))
            .map_err(|panic| {
                format!(
                    "AI training benchmark panicked: {}",
                    panic_message(&*panic)
                )
            })
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(AiTrainingWorkerEvent::Done(result));
        });
    }

    fn start_smoke_test(&mut self, adapter: AdapterInfo, gpu_intensity: GpuIntensity) {
        if self.running {
            return;
        }
        if self.backend == AiTrainingBackend::PyTorchCuda && !self.pytorch_cuda_can_run_selection() {
            self.status =
                "PyTorch CUDA smoke tests support linear, MLP, and transformer training".to_owned();
            self.log(self.status.clone());
            return;
        }

        let mut config = ai_training_smoke_config_for_workload(
            adapter,
            gpu_intensity,
            self.workload,
        );
        config.backend = self.backend;
        config.pytorch_python = self.pytorch_python.trim().to_owned();
        config.pytorch_cuda_device = self.pytorch_cuda_device;
        config.precision = self.precision;
        let mut sizing_notes = Vec::new();
        let adapter_memory_limit = if config.backend == AiTrainingBackend::PyTorchCuda {
            self.selected_pytorch_cuda_memory_bytes()
        } else {
            adapter_memory_limit_bytes(&config.adapter).map(|(bytes, _)| bytes)
        };
        if let Some(note) = auto_size_ai_training_config_for_limits(
            &mut config,
            adapter_memory_limit,
            None,
        ) {
            sizing_notes.push(note);
        }

        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.cancel = Some(cancel);
        self.running = true;
        self.progress = 0.0;
        self.phase = "Starting AI training smoke test".to_owned();
        self.eta_text = "ETA: estimating".to_owned();
        self.status = "Running AI training smoke test...".to_owned();
        let target_label = if config.backend == AiTrainingBackend::PyTorchCuda {
            format!("CUDA device {}", config.pytorch_cuda_device)
        } else {
            config.adapter.label()
        };
        self.log(format!(
            "Starting {} smoke test on {} with {}",
            config.workload,
            target_label,
            ai_training_shape_label(config.workload, &config.dimensions)
        ));
        for note in &sizing_notes {
            self.log(note.clone());
        }

        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                run_ai_training_benchmark(config, worker_cancel, tx.clone())
            }))
            .map_err(|panic| {
                format!(
                    "AI training smoke test panicked: {}",
                    panic_message(&*panic)
                )
            })
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(AiTrainingWorkerEvent::Done(result));
        });
    }

    fn start_pytorch_probe(&mut self) {
        if self.pytorch_probe_running || self.pytorch_install_running || self.running {
            return;
        }

        let python = self.pytorch_python.trim().to_owned();
        if python.is_empty() {
            self.status = "Python executable is required for PyTorch CUDA probing".to_owned();
            self.log(self.status.clone());
            return;
        }

        let tx = self.tx.clone();
        self.pytorch_probe_running = true;
        self.status = "Probing PyTorch CUDA environment...".to_owned();
        self.phase = "PyTorch CUDA probe".to_owned();
        self.progress = 0.0;
        self.eta_text = "ETA: estimating".to_owned();
        self.log(format!("Probing PyTorch CUDA with {python}"));

        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| probe_pytorch_cuda(&python)))
                .map_err(|panic| {
                    format!(
                        "PyTorch CUDA probe panicked: {}",
                        panic_message(&*panic)
                    )
                })
                .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(AiTrainingWorkerEvent::PyTorchProbeDone(result));
        });
    }

    fn request_pytorch_install(&mut self) {
        if self.running || self.pytorch_probe_running || self.pytorch_install_running {
            return;
        }
        self.pending_pytorch_install = true;
    }

    fn start_pytorch_install(&mut self) {
        if self.running || self.pytorch_probe_running || self.pytorch_install_running {
            return;
        }
        let python = self.pytorch_python.trim().to_owned();
        if python.is_empty() {
            self.status = "Python executable is required before installing PyTorch CUDA".to_owned();
            self.log(self.status.clone());
            return;
        }

        let tx = self.tx.clone();
        self.pending_pytorch_install = false;
        self.pytorch_install_running = true;
        self.pytorch_probe_running = true;
        self.status = "Installing PyTorch CUDA...".to_owned();
        self.phase = "PyTorch CUDA install".to_owned();
        self.progress = 0.0;
        self.eta_text = "Large download in progress".to_owned();
        self.log(format!(
            "User approved PyTorch CUDA install via {}",
            pytorch_cuda_install_command_preview(&python)
        ));

        thread::spawn(move || {
            let log_tx = tx.clone();
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                install_pytorch_cuda(&python, |line| {
                    let _ = log_tx.send(AiTrainingWorkerEvent::Log(format!(
                        "PyTorch install: {line}"
                    )));
                })
            }))
            .map_err(|panic| {
                format!(
                    "PyTorch CUDA install panicked: {}",
                    panic_message(&*panic)
                )
            })
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(AiTrainingWorkerEvent::PyTorchInstallDone(result));
        });
    }

    fn start_precision_sweep(&mut self, adapter: AdapterInfo, gpu_intensity: GpuIntensity) {
        if self.running || self.backend != AiTrainingBackend::PyTorchCuda {
            return;
        }
        if !self.pytorch_cuda_can_run_selection() {
            self.status =
                "PyTorch CUDA precision sweeps support linear, MLP, and transformer training"
                    .to_owned();
            self.log(self.status.clone());
            return;
        }
        if !self.pytorch_cuda_ready() {
            self.status = "Probe PyTorch CUDA before running a precision sweep".to_owned();
            self.log(self.status.clone());
            return;
        }

        let mut base_config = self.config(adapter, gpu_intensity);
        let mut sizing_notes = Vec::new();
        if let Some(note) = auto_size_ai_training_config_for_limits(
            &mut base_config,
            self.selected_pytorch_cuda_memory_bytes(),
            None,
        ) {
            sizing_notes.push(note);
        }
        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.cancel = Some(cancel);
        self.running = true;
        self.progress = 0.0;
        self.phase = "Starting precision sweep".to_owned();
        self.eta_text = "ETA: estimating".to_owned();
        self.status = "Running PyTorch CUDA precision sweep...".to_owned();
        self.log(format!(
            "Starting PyTorch CUDA precision sweep for {} on CUDA device {}",
            base_config.workload, base_config.pytorch_cuda_device
        ));
        for note in &sizing_notes {
            self.log(note.clone());
        }

        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                run_ai_training_precision_sweep(base_config, worker_cancel, tx.clone())
            }))
            .map_err(|panic| {
                format!(
                    "AI training precision sweep panicked: {}",
                    panic_message(&*panic)
                )
            })
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(AiTrainingWorkerEvent::BatchDone(result));
        });
    }

    fn cancel(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
            self.status = "Cancel requested; stopping AI training benchmark...".to_owned();
            self.log("Cancel requested for AI training benchmark");
        }
    }

    fn poll_worker_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                AiTrainingWorkerEvent::Progress(progress) => {
                    self.progress = progress.progress;
                    self.phase = progress.phase.clone();
                    self.eta_text = format_eta(progress.eta_s);
                    self.status = format!(
                        "{} - step {}/{} elapsed {}",
                        progress.phase,
                        progress.completed_steps,
                        progress.total_steps,
                        format_elapsed(progress.elapsed_s)
                    );
                }
                AiTrainingWorkerEvent::Log(message) => self.log(message),
                AiTrainingWorkerEvent::PyTorchProbeDone(result) => {
                    self.pytorch_probe_running = false;
                    self.progress = 1.0;
                    self.phase = "PyTorch CUDA probe complete".to_owned();
                    self.eta_text.clear();
                    match result {
                        Ok(environment) => {
                            if environment.cuda_available
                                && !pytorch_cuda_environment_has_device(
                                    &environment,
                                    self.pytorch_cuda_device,
                                )
                            {
                                self.pytorch_cuda_device = environment
                                    .devices
                                    .first()
                                    .map(|device| device.index)
                                    .unwrap_or(0);
                                self.log(format!(
                                    "Selected CUDA device {}",
                                    self.pytorch_cuda_device
                                ));
                            }
                            self.status = if environment.cuda_available {
                                format!(
                                    "PyTorch CUDA ready: {} CUDA device(s)",
                                    environment.device_count
                                )
                            } else if let Some(error) = &environment.error {
                                format!("PyTorch CUDA unavailable: {error}")
                            } else {
                                "PyTorch imported, but CUDA is unavailable".to_owned()
                            };
                            self.log(self.status.clone());
                            for line in environment.summary_lines() {
                                self.log(line);
                            }
                            self.pytorch_probe = Some(environment);
                        }
                        Err(err) => {
                            self.progress = 0.0;
                            self.phase = "PyTorch CUDA probe failed".to_owned();
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
                AiTrainingWorkerEvent::PyTorchInstallDone(result) => {
                    self.pytorch_install_running = false;
                    self.pytorch_probe_running = false;
                    self.pending_pytorch_install = false;
                    self.progress = 1.0;
                    self.phase = "PyTorch CUDA install complete".to_owned();
                    self.eta_text.clear();
                    match result {
                        Ok(environment) => {
                            self.pytorch_python = environment.python_executable.clone();
                            if environment.cuda_available
                                && !pytorch_cuda_environment_has_device(
                                    &environment,
                                    self.pytorch_cuda_device,
                                )
                            {
                                self.pytorch_cuda_device = environment
                                    .devices
                                    .first()
                                    .map(|device| device.index)
                                    .unwrap_or(0);
                            }
                            self.status = format!(
                                "PyTorch CUDA installed and ready: {} CUDA device(s)",
                                environment.device_count
                            );
                            self.log(self.status.clone());
                            for line in environment.summary_lines() {
                                self.log(line);
                            }
                            self.pytorch_probe = Some(environment);
                        }
                        Err(err) => {
                            self.progress = 0.0;
                            self.phase = "PyTorch CUDA install failed".to_owned();
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
                AiTrainingWorkerEvent::Done(result) => {
                    self.running = false;
                    self.cancel = None;
                    self.eta_text.clear();
                    match result {
                        Ok(result) => {
                            self.progress = 1.0;
                            self.phase = "Complete".to_owned();
                            self.status = result
                                .throughput_value
                                .map(|value| {
                                    format!(
                                        "AI training benchmark complete: {value:.1} {}",
                                        result.throughput_label
                                    )
                                })
                                .unwrap_or_else(|| "AI training benchmark complete".to_owned());
                            self.log(self.status.clone());
                            self.results.push(result);
                        }
                        Err(err) => {
                            if err.to_ascii_lowercase().contains("canceled") {
                                self.progress = 0.0;
                                self.eta_text = "ETA: canceled".to_owned();
                            }
                            self.phase = "Stopped".to_owned();
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
                AiTrainingWorkerEvent::BatchDone(result) => {
                    self.running = false;
                    self.cancel = None;
                    self.eta_text.clear();
                    match result {
                        Ok(results) => {
                            self.progress = 1.0;
                            self.phase = "Complete".to_owned();
                            self.status =
                                format!("AI precision sweep complete: {} result(s)", results.len());
                            self.log(self.status.clone());
                            self.results.extend(results);
                        }
                        Err(err) => {
                            if err.to_ascii_lowercase().contains("canceled") {
                                self.progress = 0.0;
                                self.eta_text = "ETA: canceled".to_owned();
                            }
                            self.phase = "Stopped".to_owned();
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
            }
        }
    }

    fn estimated_flops_per_step(&self) -> f64 {
        let dims = &self.dimensions;
        match self.workload {
            AiTrainingWorkload::LinearLayer => {
                let b = dims.batch_size as f64;
                let i = dims.input_dim as f64;
                let o = dims.output_dim as f64;
                6.0 * b * i * o + 2.0 * i * o
            }
            AiTrainingWorkload::Mlp => {
                let b = dims.batch_size as f64;
                let h = dims.hidden_size as f64;
                let e = dims.output_dim as f64;
                12.0 * b * h * e + 4.0 * h * e + 4.0 * b * e
            }
            AiTrainingWorkload::TransformerBlock => {
                let b = dims.batch_size as f64;
                let s = dims.sequence_len as f64;
                let h = dims.hidden_size as f64;
                let tokens = b * s;
                let block_flops = transformer_linear_block_specs(dims)
                    .map(|specs| {
                        specs
                            .iter()
                            .map(|spec| {
                                6.0 * spec.batch as f64 * spec.input as f64 * spec.output as f64
                                    + 2.0 * spec.input as f64 * spec.output as f64
                            })
                            .sum::<f64>()
                    })
                    .unwrap_or(0.0);
                let norm_softmax_activation = 12.0 * tokens * h + 5.0 * b * s * s;
                block_flops + norm_softmax_activation
            }
            AiTrainingWorkload::OptimizerStress => 2.0 * dims.parameter_count as f64,
        }
    }

    fn estimated_memory_bytes(&self) -> u64 {
        let dims = &self.dimensions;
        let value_bytes = self.precision.bytes_per_value();
        match self.workload {
            AiTrainingWorkload::LinearLayer => {
                let activations = dims
                    .batch_size
                    .saturating_mul(dims.input_dim.saturating_add(dims.output_dim))
                    as u64;
                let weights = dims.input_dim.saturating_mul(dims.output_dim) as u64;
                activations
                    .saturating_mul(value_bytes)
                    .saturating_add(weights.saturating_mul(value_bytes).saturating_mul(4))
            }
            AiTrainingWorkload::Mlp => {
                let activations = dims.batch_size.saturating_mul(
                    dims.hidden_size
                        .saturating_add(dims.output_dim)
                        .saturating_add(dims.hidden_size),
                ) as u64;
                let params = dims.parameter_count as u64;
                activations
                    .saturating_mul(value_bytes)
                    .saturating_add(params.saturating_mul(value_bytes).saturating_mul(4))
            }
            AiTrainingWorkload::TransformerBlock => {
                let tokens = dims.batch_size.saturating_mul(dims.sequence_len) as u64;
                let hidden = dims.hidden_size as u64;
                let attention = dims
                    .batch_size
                    .saturating_mul(dims.attention_heads)
                    .saturating_mul(dims.sequence_len)
                    .saturating_mul(dims.sequence_len) as u64;
                let activations = tokens
                    .saturating_mul(hidden)
                    .saturating_mul(8)
                    .saturating_add(attention);
                let params = dims.parameter_count as u64;
                activations
                    .saturating_mul(value_bytes)
                    .saturating_add(params.saturating_mul(value_bytes).saturating_mul(4))
            }
            AiTrainingWorkload::OptimizerStress => {
                let params = dims.parameter_count as u64;
                params.saturating_mul(value_bytes).saturating_mul(3)
            }
        }
    }
}

fn format_flops_per_step(flops: f64) -> String {
    if flops >= 1.0e12 {
        format!("{:.2} TFLOP", flops / 1.0e12)
    } else if flops >= 1.0e9 {
        format!("{:.2} GFLOP", flops / 1.0e9)
    } else if flops >= 1.0e6 {
        format!("{:.2} MFLOP", flops / 1.0e6)
    } else {
        format!("{flops:.0} FLOP")
    }
}

fn format_optional_tflops(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "N/A".to_owned())
}
