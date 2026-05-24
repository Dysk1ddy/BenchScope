#[derive(Clone, Debug)]
struct BenchmarkResult {
    size: usize,
    adapter: String,
    cpu_model: String,
    cpu_ms: f64,
    cpu_estimated: bool,
    gpu_compute_ms: Option<f64>,
    gpu_total_ms: f64,
    transfer_sync_ms: Option<f64>,
    gpu_path: GpuPath,
    gpu_intensity: GpuIntensity,
    dispatch_count: usize,
    tile_shape: String,
    last_dispatch_ms: Option<f64>,
    avg_dispatch_ms: Option<f64>,
    max_dispatch_ms: Option<f64>,
    backoff_count: usize,
    speedup: f64,
    validation: String,
    cpu_temperature: TemperatureSummary,
    gpu_temperature: TemperatureSummary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GpuPath {
    DirectFullBuffer,
    SmallTile,
    PyTorchCuda,
    PyTorchRocm,
    PyTorchXpu,
    OptimizedWgpu,
    ArchivedWgpu,
    PersistentPanelized,
    StreamingBlocked,
}

impl GpuPath {
    fn label(self) -> &'static str {
        match self {
            GpuPath::DirectFullBuffer => "Direct",
            GpuPath::SmallTile => "Small Tile",
            GpuPath::PyTorchCuda => "PyTorch CUDA",
            GpuPath::PyTorchRocm => "PyTorch ROCm",
            GpuPath::PyTorchXpu => "PyTorch XPU",
            GpuPath::OptimizedWgpu => "Optimized WGPU",
            GpuPath::ArchivedWgpu => "Archived WGPU",
            GpuPath::PersistentPanelized => "Panelized",
            GpuPath::StreamingBlocked => "Streaming",
        }
    }
}

impl fmt::Display for GpuPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Debug)]
struct GpuDispatchStats {
    path: GpuPath,
    tile_shape: String,
    dispatch_count: usize,
    avg_dispatch_ms: Option<f64>,
    max_dispatch_ms: Option<f64>,
    last_dispatch_ms: Option<f64>,
    backoff_count: usize,
}

impl GpuDispatchStats {
    fn new(
        path: GpuPath,
        tile_shape: impl Into<String>,
        dispatch_times_ms: &[f64],
        backoff_count: usize,
    ) -> Self {
        let dispatch_count = dispatch_times_ms.len();
        let avg_dispatch_ms = (!dispatch_times_ms.is_empty())
            .then(|| dispatch_times_ms.iter().sum::<f64>() / dispatch_times_ms.len() as f64);
        let max_dispatch_ms = dispatch_times_ms.iter().copied().reduce(f64::max);
        let last_dispatch_ms = dispatch_times_ms.last().copied();

        Self {
            path,
            tile_shape: tile_shape.into(),
            dispatch_count,
            avg_dispatch_ms,
            max_dispatch_ms,
            last_dispatch_ms,
            backoff_count,
        }
    }
}

#[derive(Clone, Debug)]
struct SingleProgress {
    cpu_progress: f32,
    gpu_progress: f32,
    elapsed_s: f64,
    eta_s: Option<f64>,
    phase: String,
}

#[derive(Clone, Debug)]
struct RepeatProgress {
    mode: RepeatMode,
    size: usize,
    duration_s: Option<f64>,
    elapsed_s: f64,
    iterations: u64,
    latest_ms: f64,
    average_total_ms: f64,
    average_compute_ms: Option<f64>,
    theoretical_fp16_tc_fp32_accum_tflops: Option<MetricRange>,
    canceled: bool,
}

impl RepeatProgress {
    fn iterations_per_second(&self) -> Option<f64> {
        (self.elapsed_s > 0.0).then_some(self.iterations as f64 / self.elapsed_s)
    }

    fn throughput_tflops(&self) -> Option<f64> {
        let average_ms = self
            .average_compute_ms
            .unwrap_or(self.average_total_ms);
        if self.iterations == 0 || average_ms <= 0.0 {
            return None;
        }

        let n = self.size as f64;
        let flops_per_iteration = 2.0 * n * n * n;
        Some(flops_per_iteration / (average_ms / 1000.0) / 1.0e12)
    }

    fn fp16_tensor_core_efficiency_percent(&self) -> Option<MetricRange> {
        let throughput = self.throughput_tflops()?;
        let theoretical = self.theoretical_fp16_tc_fp32_accum_tflops?;
        Some(MetricRange::new(
            throughput / theoretical.max * 100.0,
            throughput / theoretical.min * 100.0,
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StressGpuBackend {
    AutoOptimized,
    OptimizedWgpu,
    ArchivedWgpu,
}

impl StressGpuBackend {
    const ALL: [StressGpuBackend; 3] = [
        StressGpuBackend::AutoOptimized,
        StressGpuBackend::OptimizedWgpu,
        StressGpuBackend::ArchivedWgpu,
    ];

    fn label(self) -> &'static str {
        match self {
            StressGpuBackend::AutoOptimized => "Auto optimized",
            StressGpuBackend::OptimizedWgpu => "Optimized WGPU",
            StressGpuBackend::ArchivedWgpu => "Archived WGPU",
        }
    }

    fn description(self) -> &'static str {
        match self {
            StressGpuBackend::AutoOptimized => {
                "Uses the selected adapter's native PyTorch backend when available (CUDA, ROCm, or XPU), otherwise falls back to optimized WGPU kernels."
            }
            StressGpuBackend::OptimizedWgpu => {
                "Uses BenchScope's optimized cross-vendor WGPU kernels on the selected adapter without Python or vendor ML runtimes."
            }
            StressGpuBackend::ArchivedWgpu => {
                "Keeps the previous tiny-matrix WGPU stress shader for comparison."
            }
        }
    }

    fn uses_optimized_wgpu(self) -> bool {
        matches!(
            self,
            StressGpuBackend::AutoOptimized | StressGpuBackend::OptimizedWgpu
        )
    }

    fn can_try_native_pytorch(self) -> bool {
        self == StressGpuBackend::AutoOptimized
    }
}

impl fmt::Display for StressGpuBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RepeatMode {
    Gpu,
    Cpu,
}

impl fmt::Display for RepeatMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepeatMode::Gpu => f.write_str("GPU"),
            RepeatMode::Cpu => f.write_str("CPU"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GpuIntensity {
    Safe,
    Balanced,
    High,
}

impl GpuIntensity {
    const ALL: [GpuIntensity; 3] = [
        GpuIntensity::Safe,
        GpuIntensity::Balanced,
        GpuIntensity::High,
    ];

    fn label(self) -> &'static str {
        match self {
            GpuIntensity::Safe => "Safe",
            GpuIntensity::Balanced => "Balanced",
            GpuIntensity::High => "High",
        }
    }

    fn description(self) -> &'static str {
        match self {
            GpuIntensity::Safe => {
                "Default. Smaller GPU submissions with short pauses to reduce driver timeout and power-spike risk."
            }
            GpuIntensity::Balanced => {
                "Larger GPU submissions with lighter pauses for faster large runs."
            }
            GpuIntensity::High => {
                "Largest submissions. Use only after the system is stable under Safe/Balanced mode."
            }
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "safe" | "low" | "conservative" => Some(GpuIntensity::Safe),
            "balanced" | "normal" | "medium" => Some(GpuIntensity::Balanced),
            "high" | "max" | "maximum" => Some(GpuIntensity::High),
            _ => None,
        }
    }
}

impl fmt::Display for GpuIntensity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RepeatDuration {
    OneMinute,
    FiveMinutes,
    Infinite,
}

impl RepeatDuration {
    fn seconds(self) -> Option<f64> {
        match self {
            RepeatDuration::OneMinute => Some(60.0),
            RepeatDuration::FiveMinutes => Some(300.0),
            RepeatDuration::Infinite => None,
        }
    }

    fn duration(self) -> Option<Duration> {
        self.seconds().map(Duration::from_secs_f64)
    }

    fn is_infinite(self) -> bool {
        matches!(self, RepeatDuration::Infinite)
    }

    fn run_label(self) -> &'static str {
        match self {
            RepeatDuration::OneMinute => "for 1 minute",
            RepeatDuration::FiveMinutes => "for 5 minutes",
            RepeatDuration::Infinite => "until canceled",
        }
    }
}

impl fmt::Display for RepeatDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepeatDuration::OneMinute => f.write_str("1 minute"),
            RepeatDuration::FiveMinutes => f.write_str("5 minutes"),
            RepeatDuration::Infinite => f.write_str("infinite"),
        }
    }
}
