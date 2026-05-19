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
    PersistentPanelized,
    StreamingBlocked,
}

impl GpuPath {
    fn label(self) -> &'static str {
        match self {
            GpuPath::DirectFullBuffer => "Direct",
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
    canceled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StressGpuBackend {
    Optimized,
    ArchivedWgpu,
}

impl StressGpuBackend {
    const ALL: [StressGpuBackend; 2] = [
        StressGpuBackend::Optimized,
        StressGpuBackend::ArchivedWgpu,
    ];

    fn label(self) -> &'static str {
        match self {
            StressGpuBackend::Optimized => "Optimized",
            StressGpuBackend::ArchivedWgpu => "Archived WGPU",
        }
    }

    fn description(self) -> &'static str {
        match self {
            StressGpuBackend::Optimized => {
                "For 4x4, uses CUDA tensor-core equivalent work when available, otherwise a register-heavy WGPU microkernel."
            }
            StressGpuBackend::ArchivedWgpu => {
                "Keeps the previous tiny-matrix WGPU stress shader for comparison."
            }
        }
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
