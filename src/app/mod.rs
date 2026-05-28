#[derive(Clone, Debug)]
struct StartupProgress {
    step: String,
    progress: f32,
}

enum StartupEvent {
    Progress(StartupProgress),
    Ready(Box<StartupData>),
    Failed(String),
}

struct StartupData {
    adapters: Vec<AdapterInfo>,
    cpu_info: CpuInfo,
    setup_detection: SetupDetection,
    drive: DriveBenchmarkState,
    storage_health: StorageHealthState,
    ram: RamTestState,
    battery: BatteryDiagnosticState,
    network: NetworkDiagnosticState,
    device_info: DeviceInfoState,
    ai_training: AiTrainingBenchmarkState,
    gpu_memory: GpuMemoryBenchmarkState,
}

#[derive(Clone, Debug)]
struct SetupDetection {
    elevated: bool,
    vcruntime_available: bool,
    nvidia_smi_available: Option<bool>,
    hardware_monitor_wmi_available: bool,
    sensor_service_available: bool,
    managed_pytorch_python: Option<String>,
    managed_pytorch_install_base_available: bool,
}

struct BenchScopeRoot {
    startup_rx: Receiver<StartupEvent>,
    startup_progress: StartupProgress,
    app: Option<BenchScopeApp>,
    startup_error: Option<String>,
}

struct BenchScopeApp {
    view: AppView,
    main_menu_category: Option<MenuCategory>,
    main_menu_search_text: String,
    adapters: Vec<AdapterInfo>,
    cpu_info: CpuInfo,
    setup_detection: SetupDetection,
    selected_adapter: usize,
    size_text: String,
    stress_size_text: String,
    gpu_intensity: GpuIntensity,
    stress_gpu_backend: StressGpuBackend,
    validate_output: bool,
    estimate_cpu_time: bool,
    repeat_mode: RepeatMode,
    repeat_duration: RepeatDuration,
    pytorch_python: String,
    pytorch_probe: Option<PyTorchCudaEnvironment>,
    pytorch_probe_running: bool,
    pytorch_install_running: bool,
    pending_pytorch_install: bool,
    results: Vec<BenchmarkResult>,
    repeat_progress: Option<RepeatProgress>,
    log: Vec<String>,
    status: String,
    progress: f32,
    cpu_progress: f32,
    gpu_progress: f32,
    eta_text: String,
    rx: Receiver<WorkerEvent>,
    tx: Sender<WorkerEvent>,
    cancel: Option<Arc<AtomicBool>>,
    running: bool,
    repeat_running: bool,
    pending_vram_warning: Option<PendingVramWarning>,
    matrix_back_confirm: bool,
    stress_back_confirm: bool,
    drive_back_confirm: bool,
    ram_back_confirm: bool,
    battery_back_confirm: bool,
    network_back_confirm: bool,
    ai_training_back_confirm: bool,
    gpu_memory_back_confirm: bool,
    device_info: DeviceInfoState,
    drive: DriveBenchmarkState,
    storage_health_back_confirm: bool,
    storage_health: StorageHealthState,
    ram: RamTestState,
    battery: BatteryDiagnosticState,
    network: NetworkDiagnosticState,
    ai_training: AiTrainingBenchmarkState,
    gpu_memory: GpuMemoryBenchmarkState,
    history: HistoryState,
    timeline: TimelineState,
    sensors: SensorManager,
    temperature_run: Option<TemperatureRunTracker>,
    sensor_window_minimized: bool,
    fullscreen: bool,
}

include!("view.rs");
include!("setup.rs");
include!("common.rs");
include!("window.rs");
include!("runtime.rs");
include!("history_reports.rs");
include!("timeline_integration.rs");
