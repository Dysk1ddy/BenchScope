use std::{
    any::Any,
    collections::HashMap,
    fmt,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    panic::{self, AssertUnwindSafe},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
use std::os::windows::{fs::OpenOptionsExt, process::CommandExt};

use anyhow::{Context, Result, anyhow};
use bytemuck::{Pod, Zeroable};
use eframe::egui;
use wgpu::util::DeviceExt;

const DEFAULT_SIZES: &[usize] = &[
    4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384,
];
const TILE_SIZE: u32 = 16;
const CANCEL_CHECK_INTERVAL: usize = 1_048_576;
const PROGRESS_SAMPLE_MS: u64 = 200;
const CPU_ESTIMATE_MIN_SAMPLE_SIZE: usize = 128;
const CPU_ESTIMATE_BASE_SAMPLE_SIZE: usize = 512;
#[cfg(test)]
const CPU_ESTIMATE_MID_SAMPLE_SIZE: usize = 768;
#[cfg(test)]
const CPU_ESTIMATE_MAX_SAMPLE_SIZE: usize = 1024;
const CPU_ESTIMATE_BASE_ROW_CELLS: usize = 131_072;
const CPU_ESTIMATE_MID_ROW_CELLS: usize = 196_608;
const CPU_ESTIMATE_HIGH_ROW_CELLS: usize = 262_144;
const CPU_ESTIMATE_MAX_ROWS: usize = 128;
const CPU_ESTIMATE_TARGET_MS: f64 = 2_000.0;
const VALIDATION_SAMPLE_POINTS: usize = 256;
const GPU_SAFE_CHUNK_ROWS: usize = 16;
const GPU_BALANCED_CHUNK_ROWS: usize = 64;
const GPU_HIGH_CHUNK_ROWS: usize = 128;
const GPU_SAFE_BLOCK_ROWS: usize = 32;
const GPU_SAFE_BLOCK_COLS: usize = 512;
const GPU_BALANCED_BLOCK_ROWS: usize = 64;
const GPU_BALANCED_BLOCK_COLS: usize = 1024;
const GPU_HIGH_BLOCK_ROWS: usize = 128;
const GPU_HIGH_BLOCK_COLS: usize = 2048;
const GPU_WAIT_POLL_MS: u64 = 1;
const WGPU_MAX_QUERY_COUNT: usize = 4096;
const FORCE_BLOCKED_GPU_ENV: &str = "BENCHSCOPE_FORCE_BLOCKED_GPU";
const DRIVE_BENCHMARK_FILE_NAME: &str = "benchscope_drive_benchmark.tmp";
const DRIVE_MAX_TEST_SECONDS: f64 = 30.0;
const DRIVE_SEQUENTIAL_BLOCK_BYTES: usize = 8 * 1024 * 1024;
const DRIVE_RANDOM_BLOCK_BYTES: usize = 4 * 1024;
const DRIVE_LATENCY_SAMPLE_LIMIT: usize = 200_000;
const DECIMAL_MB: f64 = 1_000_000.0;
const RESULT_HEADER_TEXT_SIZE: f32 = 18.0;
const RESULT_CELL_TEXT_SIZE: f32 = 17.0;
const LOG_TEXT_SIZE: f32 = 13.0;
const SENSOR_POLL_MS: u64 = 1_000;
const SENSOR_STALE_AFTER_MS: u64 = 5_000;
const SENSOR_BOX_RESERVED_HEIGHT: f32 = 112.0;
const PANEL_VERTICAL_CHROME_HEIGHT: f32 = 92.0;
const MIN_LOG_HEIGHT: f32 = 64.0;
const MIN_CONTENT_HEIGHT: f32 = 96.0;
const CONTROLS_ACTION_HEIGHT: f32 = 104.0;
const CREATE_NO_WINDOW_RAW: u32 = 0x0800_0000;
#[cfg(windows)]
const FILE_FLAG_WRITE_THROUGH_RAW: u32 = 0x8000_0000;
#[cfg(windows)]
const FILE_FLAG_NO_BUFFERING_RAW: u32 = 0x2000_0000;
#[cfg(windows)]
const FILE_FLAG_RANDOM_ACCESS_RAW: u32 = 0x1000_0000;
#[cfg(windows)]
const FILE_FLAG_SEQUENTIAL_SCAN_RAW: u32 = 0x0800_0000;

const MATMUL_SHADER: &str = r#"
const TILE: u32 = 16u;

struct Params {
    n: u32,
    row_offset: u32,
    row_count: u32,
    _pad2: u32,
}

@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read> b: array<f32>;
@group(0) @binding(2) var<storage, read_write> c: array<f32>;
@group(0) @binding(3) var<uniform> params: Params;

var<workgroup> tile_a: array<array<f32, 16>, 16>;
var<workgroup> tile_b: array<array<f32, 16>, 16>;

@compute @workgroup_size(16, 16, 1)
fn main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>
) {
    let row = gid.y + params.row_offset;
    let col = gid.x;
    let row_in_chunk = gid.y < params.row_count;
    var sum = 0.0;

    var tile = 0u;
    loop {
        if (tile >= params.n) {
            break;
        }

        let a_col = tile + lid.x;
        let b_row = tile + lid.y;

        if (row_in_chunk && row < params.n && a_col < params.n) {
            tile_a[lid.y][lid.x] = a[row * params.n + a_col];
        } else {
            tile_a[lid.y][lid.x] = 0.0;
        }

        if (b_row < params.n && col < params.n) {
            tile_b[lid.y][lid.x] = b[b_row * params.n + col];
        } else {
            tile_b[lid.y][lid.x] = 0.0;
        }

        workgroupBarrier();

        for (var k = 0u; k < TILE; k = k + 1u) {
            sum = sum + tile_a[lid.y][k] * tile_b[k][lid.x];
        }

        workgroupBarrier();
        tile = tile + TILE;
    }

    if (row_in_chunk && row < params.n && col < params.n) {
        c[row * params.n + col] = sum;
    }
}
"#;

const BLOCKED_MATMUL_SHADER: &str = r#"
const TILE: u32 = 16u;

struct BlockParams {
    n: u32,
    rows: u32,
    cols: u32,
    _pad: u32,
}

@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read> b: array<f32>;
@group(0) @binding(2) var<storage, read_write> c: array<f32>;
@group(0) @binding(3) var<uniform> params: BlockParams;

var<workgroup> tile_a: array<array<f32, 16>, 16>;
var<workgroup> tile_b: array<array<f32, 16>, 16>;

@compute @workgroup_size(16, 16, 1)
fn main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>
) {
    let row = gid.y;
    let col = gid.x;
    var sum = 0.0;

    var tile = 0u;
    loop {
        if (tile >= params.n) {
            break;
        }

        let a_col = tile + lid.x;
        let b_row = tile + lid.y;

        if (row < params.rows && a_col < params.n) {
            tile_a[lid.y][lid.x] = a[row * params.n + a_col];
        } else {
            tile_a[lid.y][lid.x] = 0.0;
        }

        if (b_row < params.n && col < params.cols) {
            tile_b[lid.y][lid.x] = b[b_row * params.cols + col];
        } else {
            tile_b[lid.y][lid.x] = 0.0;
        }

        workgroupBarrier();

        for (var k = 0u; k < TILE; k = k + 1u) {
            sum = sum + tile_a[lid.y][k] * tile_b[k][lid.x];
        }

        workgroupBarrier();
        tile = tile + TILE;
    }

    if (row < params.rows && col < params.cols) {
        c[row * params.cols + col] = sum;
    }
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    n: u32,
    row_offset: u32,
    row_count: u32,
    _pad2: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BlockParams {
    n: u32,
    rows: u32,
    cols: u32,
    _pad: u32,
}

#[derive(Clone, Debug)]
struct AdapterInfo {
    index: usize,
    name: String,
    backend: wgpu::Backend,
    device_type: wgpu::DeviceType,
    vendor: u32,
    device: u32,
    driver: String,
    timestamp_query: bool,
    dedicated_vram_bytes: Option<u64>,
    dedicated_system_memory_bytes: Option<u64>,
    shared_system_memory_bytes: Option<u64>,
}

impl AdapterInfo {
    fn label(&self) -> String {
        format!(
            "{} - {} - {:?}",
            self.name,
            device_type_label(self.device_type),
            self.backend
        )
    }
}

#[derive(Clone, Debug)]
struct DxgiMemoryInfo {
    name: String,
    vendor: u32,
    device: u32,
    dedicated_vram_bytes: u64,
    dedicated_system_memory_bytes: u64,
    shared_system_memory_bytes: u64,
}

#[derive(Clone, Debug)]
struct CpuInfo {
    model: String,
    logical_processors: usize,
}

impl CpuInfo {
    fn label(&self) -> String {
        format!(
            "{} ({} logical processor{})",
            self.model,
            self.logical_processors,
            if self.logical_processors == 1 {
                ""
            } else {
                "s"
            }
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SensorKind {
    Cpu,
    Gpu,
    Drive,
}

impl SensorKind {
    fn warning_c(self) -> f32 {
        match self {
            SensorKind::Cpu => 85.0,
            SensorKind::Gpu => 80.0,
            SensorKind::Drive => 60.0,
        }
    }

    fn critical_c(self) -> f32 {
        match self {
            SensorKind::Cpu => 95.0,
            SensorKind::Gpu => 90.0,
            SensorKind::Drive => 70.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SensorStatus {
    Ok,
    Unsupported,
    PermissionDenied,
    Stale,
    Error(String),
}

impl SensorStatus {
    fn detail(&self) -> String {
        match self {
            SensorStatus::Ok => "OK".to_owned(),
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
        self.status == SensorStatus::Ok
            && (self.temperature_c.is_some() || self.utilization_percent.is_some())
    }

    fn has_temperature(&self) -> bool {
        self.status == SensorStatus::Ok && self.temperature_c.is_some()
    }

    fn has_utilization(&self) -> bool {
        self.status == SensorStatus::Ok && self.utilization_percent.is_some()
    }
}

#[derive(Clone, Debug, Default)]
struct SensorSnapshot {
    cpu: Option<SensorReading>,
    gpu: Option<SensorReading>,
    drive: Option<SensorReading>,
    helper_elevated: Option<bool>,
}

impl SensorSnapshot {
    fn stale_checked(&self, now: Instant) -> Self {
        let stale_after = Duration::from_millis(SENSOR_STALE_AFTER_MS);
        Self {
            cpu: stale_checked_reading(self.cpu.clone(), now, stale_after),
            gpu: stale_checked_reading(self.gpu.clone(), now, stale_after),
            drive: stale_checked_reading(self.drive.clone(), now, stale_after),
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

#[derive(Clone, Debug)]
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
        let shutdown = Arc::new(AtomicBool::new(false));

        let thread_latest = Arc::clone(&latest);
        let thread_target_drive_letter = Arc::clone(&target_drive_letter);
        let thread_target_gpu_uses_shared_cpu_temperature =
            Arc::clone(&target_gpu_uses_shared_cpu_temperature);
        let thread_shutdown = Arc::clone(&shutdown);

        let _ = thread::Builder::new()
            .name("benchscope-sensors".to_owned())
            .spawn(move || {
                let helper_rx = start_sensor_helper_reader();
                let mut helper_snapshot: Option<SensorSnapshot> = None;
                let elevated_helper_path = start_elevated_sensor_helper_file();
                let mut elevated_helper_snapshot: Option<SensorSnapshot> = None;
                let mut elevated_helper_modified: Option<SystemTime> = None;
                while !thread_shutdown.load(Ordering::Relaxed) {
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
                    if let Some(path) = &elevated_helper_path {
                        if let Some(snapshot) =
                            read_sensor_helper_snapshot_file(path, &mut elevated_helper_modified)
                        {
                            elevated_helper_snapshot = Some(snapshot);
                        }
                    }

                    let helper_preferred_snapshot = elevated_helper_snapshot
                        .clone()
                        .or_else(|| helper_snapshot.clone());
                    let fallback_snapshot =
                        helper_preferred_snapshot.as_ref().and_then(|snapshot| {
                            helper_snapshot_has_gaps(snapshot)
                                .then(|| collect_sensor_snapshot(drive_letter))
                        });
                    let snapshot =
                        merge_sensor_snapshots(helper_preferred_snapshot, fallback_snapshot)
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
    duration_s: f64,
    elapsed_s: f64,
    iterations: u64,
    latest_ms: f64,
    average_total_ms: f64,
    average_compute_ms: Option<f64>,
    canceled: bool,
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
}

impl RepeatDuration {
    fn seconds(self) -> f64 {
        match self {
            RepeatDuration::OneMinute => 60.0,
            RepeatDuration::FiveMinutes => 300.0,
        }
    }

    fn duration(self) -> Duration {
        Duration::from_secs_f64(self.seconds())
    }
}

impl fmt::Display for RepeatDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepeatDuration::OneMinute => f.write_str("1 minute"),
            RepeatDuration::FiveMinutes => f.write_str("5 minutes"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppView {
    MainMenu,
    MatrixBenchmark,
    MatrixStressTest,
    DriveBenchmark,
}

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
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let target_folder = std::env::temp_dir();
        let drives = detect_drives();
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

#[derive(Debug)]
enum WorkerEvent {
    SingleProgress(SingleProgress),
    SingleDone(Result<BenchmarkResult, String>),
    RepeatProgress(RepeatProgress),
    RepeatDone(Result<RepeatProgress, String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunAction {
    Single,
    Repeat,
}

#[derive(Clone, Debug)]
struct PendingVramWarning {
    action: RunAction,
    size: usize,
    adapter: AdapterInfo,
    gpu_intensity: GpuIntensity,
    validate_output: bool,
    estimate_cpu_time: bool,
    repeat_mode: RepeatMode,
    repeat_duration: RepeatDuration,
    estimated_gpu_bytes: u64,
    limit_bytes: u64,
    limit_label: String,
}

#[derive(Debug)]
struct GpuTiming {
    compute_ms: Option<f64>,
    total_ms: f64,
    transfer_sync_ms: Option<f64>,
    stats: GpuDispatchStats,
    output: Vec<f32>,
}

struct BlockGpuTiming {
    compute_ms: Option<f64>,
    observed_ms: f64,
    output: Vec<f32>,
}

#[derive(Clone, Debug)]
struct ColumnPanel {
    col_offset: usize,
    cols: usize,
    element_offset: usize,
}

struct GpuWorkGovernor {
    row_extent: usize,
    min_row_extent: usize,
    hard_backoff_ms: f64,
    backoff_count: usize,
}

impl GpuWorkGovernor {
    fn new(row_extent: usize, min_row_extent: usize, gpu_intensity: GpuIntensity) -> Self {
        Self {
            row_extent: row_extent.max(1),
            min_row_extent: min_row_extent.max(1),
            hard_backoff_ms: gpu_hard_backoff_ms(gpu_intensity),
            backoff_count: 0,
        }
    }

    fn row_extent(&self, remaining: usize) -> usize {
        self.row_extent.min(remaining).max(1)
    }

    fn record_dispatch(&mut self, observed_ms: f64) {
        if observed_ms > self.hard_backoff_ms && self.row_extent > self.min_row_extent {
            self.row_extent = align_block_extent((self.row_extent / 2).max(self.min_row_extent));
            self.backoff_count += 1;
        }
    }
}

struct SingleProgressTracker {
    tx: Option<Sender<WorkerEvent>>,
    started: Instant,
    last_emit: Instant,
    cpu_progress: f32,
    gpu_progress: f32,
    phase: String,
    gpu_estimate_s: f64,
    gpu_started: Option<Instant>,
}

impl SingleProgressTracker {
    fn new(
        size: usize,
        adapter: &AdapterInfo,
        gpu_intensity: GpuIntensity,
        tx: Option<Sender<WorkerEvent>>,
    ) -> Self {
        Self {
            tx,
            started: Instant::now(),
            last_emit: Instant::now() - Duration::from_secs(1),
            cpu_progress: 0.0,
            gpu_progress: 0.0,
            phase: "Preparing benchmark".to_owned(),
            gpu_estimate_s: estimate_gpu_seconds(size, adapter, gpu_intensity),
            gpu_started: None,
        }
    }

    fn set_phase(&mut self, phase: impl Into<String>, force: bool) {
        self.phase = phase.into();
        self.emit(force);
    }

    fn set_cpu_progress(&mut self, progress: f32, force: bool) {
        self.cpu_progress = progress.clamp(0.0, 1.0);
        self.emit(force);
    }

    fn set_gpu_progress(&mut self, progress: f32, force: bool) {
        self.gpu_progress = progress.clamp(0.0, 1.0);
        self.emit(force);
    }

    fn start_cpu_ticker(
        &mut self,
        completed_blocks: Arc<AtomicUsize>,
        total_blocks: usize,
    ) -> Option<ProgressTicker> {
        let tx = self.tx.clone()?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let started = self.started;
        let gpu_estimate_s = self.gpu_estimate_s;
        let gpu_progress = self.gpu_progress;
        let total_blocks = total_blocks.max(1);
        let handle = thread::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(PROGRESS_SAMPLE_MS));
                if worker_stop.load(Ordering::Relaxed) {
                    break;
                }
                let cpu_progress = (completed_blocks.load(Ordering::Relaxed) as f32
                    / total_blocks as f32)
                    .clamp(0.0, 1.0);
                let elapsed_s = started.elapsed().as_secs_f64();
                let eta_s = if cpu_progress > 0.001 && cpu_progress < 1.0 {
                    let cpu_total_estimate = elapsed_s / cpu_progress as f64;
                    Some((cpu_total_estimate - elapsed_s).max(0.0) + gpu_estimate_s)
                } else {
                    Some(gpu_estimate_s)
                };
                let _ = tx.send(WorkerEvent::SingleProgress(SingleProgress {
                    cpu_progress,
                    gpu_progress,
                    elapsed_s,
                    eta_s,
                    phase: "CPU computing".to_owned(),
                }));
            }
        });
        Some(ProgressTicker {
            stop,
            handle: Some(handle),
        })
    }

    fn emit(&mut self, force: bool) {
        let now = Instant::now();
        if !force && now.duration_since(self.last_emit) < Duration::from_millis(PROGRESS_SAMPLE_MS)
        {
            return;
        }
        self.last_emit = now;
        if let Some(tx) = &self.tx {
            let _ = tx.send(WorkerEvent::SingleProgress(SingleProgress {
                cpu_progress: self.cpu_progress,
                gpu_progress: self.gpu_progress,
                elapsed_s: self.started.elapsed().as_secs_f64(),
                eta_s: self.eta_s(),
                phase: self.phase.clone(),
            }));
        }
    }

    fn eta_s(&self) -> Option<f64> {
        if self.cpu_progress < 1.0 {
            if self.cpu_progress > 0.001 {
                let elapsed = self.started.elapsed().as_secs_f64();
                let cpu_total_estimate = elapsed / self.cpu_progress as f64;
                Some((cpu_total_estimate - elapsed).max(0.0) + self.gpu_estimate_s)
            } else {
                Some(self.gpu_estimate_s)
            }
        } else if self.gpu_progress < 1.0 {
            self.gpu_started.map(|started| {
                let elapsed = started.elapsed().as_secs_f64();
                if self.gpu_progress > 0.001 {
                    let estimated_total = elapsed / self.gpu_progress as f64;
                    (estimated_total - elapsed).max(0.0)
                } else {
                    self.gpu_estimate_s
                }
            })
        } else {
            None
        }
    }
}

struct ProgressTicker {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl ProgressTicker {
    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ProgressTicker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct GpuRunner {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    blocked_pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    timestamp_query: bool,
    max_storage_buffer_binding_size: u64,
    max_buffer_size: u64,
    min_storage_buffer_offset_alignment: u32,
}

impl GpuRunner {
    fn new(adapter_index: usize) -> Result<Self> {
        let instance = wgpu::Instance::default();
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        let adapter = adapters
            .into_iter()
            .nth(adapter_index)
            .ok_or_else(|| anyhow!("GPU adapter index {adapter_index} is no longer available"))?;

        let adapter_features = adapter.features();
        let timestamp_query = adapter_features.contains(wgpu::Features::TIMESTAMP_QUERY);
        let required_features = if timestamp_query {
            wgpu::Features::TIMESTAMP_QUERY
        } else {
            wgpu::Features::empty()
        };

        let requested_limits =
            wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());
        let mut descriptor = wgpu::DeviceDescriptor::default();
        descriptor.label = Some("BenchScope device");
        descriptor.required_features = required_features;
        descriptor.required_limits = requested_limits.clone();

        let (device, queue) = pollster::block_on(adapter.request_device(&descriptor))
            .context("requesting wgpu device")?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Tiled matrix multiplication shader"),
            source: wgpu::ShaderSource::Wgsl(MATMUL_SHADER.into()),
        });
        let blocked_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Blocked matrix multiplication shader"),
            source: wgpu::ShaderSource::Wgsl(BLOCKED_MATMUL_SHADER.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Matrix multiplication bind group layout"),
            entries: &[
                storage_entry(0, true),
                storage_entry(1, true),
                storage_entry(2, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Matrix multiplication pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Matrix multiplication compute pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let blocked_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Blocked matrix multiplication compute pipeline"),
            layout: Some(&pipeline_layout),
            module: &blocked_shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let runner = Self {
            device,
            queue,
            pipeline,
            blocked_pipeline,
            bind_group_layout,
            timestamp_query,
            max_storage_buffer_binding_size: requested_limits.max_storage_buffer_binding_size,
            max_buffer_size: requested_limits.max_buffer_size,
            min_storage_buffer_offset_alignment: requested_limits
                .min_storage_buffer_offset_alignment,
        };
        runner.warm_up()?;
        Ok(runner)
    }

    fn warm_up(&self) -> Result<()> {
        let a = vec![1.0_f32];
        let b = vec![1.0_f32];
        self.multiply(1, &a, &b, false).map(|_| ())
    }

    fn multiply(&self, n: usize, a: &[f32], b: &[f32], use_timestamps: bool) -> Result<GpuTiming> {
        self.multiply_cancelable(n, a, b, use_timestamps, GpuIntensity::Safe, None, None)
    }

    fn multiply_cancelable(
        &self,
        n: usize,
        a: &[f32],
        b: &[f32],
        use_timestamps: bool,
        gpu_intensity: GpuIntensity,
        cancel: Option<&AtomicBool>,
        mut progress: Option<&mut SingleProgressTracker>,
    ) -> Result<GpuTiming> {
        let elements = n
            .checked_mul(n)
            .ok_or_else(|| anyhow!("matrix size overflow"))?;
        if a.len() != elements || b.len() != elements {
            return Err(anyhow!("matrix data length does not match {n}x{n}"));
        }
        let n_u32 = u32::try_from(n).context("matrix size exceeds GPU shader limits")?;

        let byte_len = elements
            .checked_mul(std::mem::size_of::<f32>())
            .ok_or_else(|| anyhow!("matrix byte length overflow"))?
            as wgpu::BufferAddress;
        if self.needs_blocked_path(byte_len) {
            if self.can_use_panelized_path(n, byte_len, gpu_intensity)? {
                return self.multiply_panelized(
                    n,
                    n_u32,
                    a,
                    b,
                    byte_len,
                    use_timestamps,
                    gpu_intensity,
                    cancel,
                    progress,
                );
            }
            return self.multiply_blocked(
                n,
                n_u32,
                a,
                b,
                use_timestamps,
                gpu_intensity,
                cancel,
                progress,
            );
        }

        let total_start = Instant::now();
        if let Some(progress) = progress.as_deref_mut() {
            progress.gpu_started = Some(Instant::now());
            progress.set_phase("GPU computing and readback", true);
            progress.set_gpu_progress(0.0, true);
        }

        let a_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Matrix A"),
                contents: bytemuck::cast_slice(a),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let b_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Matrix B"),
                contents: bytemuck::cast_slice(b),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let c_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Matrix C GPU output"),
            size: byte_len,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Matrix C readback"),
            size: byte_len,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let params = Params {
            n: n_u32,
            row_offset: 0,
            row_count: 0,
            _pad2: 0,
        };
        let params_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Matrix params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Matrix multiplication bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: a_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: b_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: c_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        let chunk_rows = gpu_dispatch_chunk_rows(n, gpu_intensity);
        let min_chunk_rows = gpu_min_dispatch_rows(gpu_intensity).min(chunk_rows).max(1);
        let max_chunk_count = n.div_ceil(min_chunk_rows);
        let mut governor = GpuWorkGovernor::new(chunk_rows, min_chunk_rows, gpu_intensity);
        let timestamp_plan = (self.timestamp_query && use_timestamps)
            .then(|| timestamp_query_plan(max_chunk_count))
            .flatten();
        let query_set = timestamp_plan.map(|(timestamp_query_count, _)| {
            self.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("GPU compute timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: timestamp_query_count,
            })
        });
        let timestamp_resolve = timestamp_plan.map(|(_, timestamp_buffer_size)| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Timestamp resolve"),
                size: timestamp_buffer_size,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        });
        let timestamp_readback = timestamp_plan.map(|(_, timestamp_buffer_size)| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Timestamp readback"),
                size: timestamp_buffer_size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });

        let mut observed_dispatch_ms = Vec::new();
        let mut chunk_index = 0usize;
        let mut row_offset = 0usize;
        while row_offset < n {
            self.check_gpu_canceled(cancel)?;
            let rows_this_chunk = governor.row_extent(n - row_offset);
            let params = Params {
                n: n_u32,
                row_offset: row_offset as u32,
                row_count: rows_this_chunk as u32,
                _pad2: 0,
            };
            self.queue
                .write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));

            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Matrix multiplication chunk encoder"),
                });

            {
                let timestamp_writes =
                    query_set
                        .as_ref()
                        .map(|query_set| wgpu::ComputePassTimestampWrites {
                            query_set,
                            beginning_of_pass_write_index: Some((chunk_index * 2) as u32),
                            end_of_pass_write_index: Some((chunk_index * 2 + 1) as u32),
                        });
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Matrix multiplication chunk pass"),
                    timestamp_writes,
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                let groups_x = n_u32.div_ceil(TILE_SIZE);
                let groups_y = (rows_this_chunk as u32).div_ceil(TILE_SIZE);
                pass.dispatch_workgroups(groups_x, groups_y, 1);
            }

            let dispatch_start = Instant::now();
            let submission = self.queue.submit([encoder.finish()]);
            self.wait_for_submission(submission, cancel, "waiting for GPU matrix chunk to finish")?;
            let observed_ms = dispatch_start.elapsed().as_secs_f64() * 1000.0;
            observed_dispatch_ms.push(observed_ms);
            governor.record_dispatch(observed_ms);
            row_offset += rows_this_chunk;
            chunk_index += 1;
            if let Some(progress) = progress.as_deref_mut() {
                progress.set_gpu_progress(row_offset as f32 / n as f32 * 0.97, false);
            }
            if row_offset < n {
                pause_between_gpu_submissions(gpu_intensity, cancel)?;
            }
        }

        self.check_gpu_canceled(cancel)?;
        if let Some(progress) = progress.as_deref_mut() {
            progress.set_phase("GPU readback", true);
            progress.set_gpu_progress(0.98, true);
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Matrix readback encoder"),
            });
        if let (Some(query_set), Some(resolve), Some(readback)) =
            (&query_set, &timestamp_resolve, &timestamp_readback)
        {
            let used_query_count = (chunk_index * 2) as u32;
            let used_timestamp_buffer_size = (used_query_count as u64) * 8;
            encoder.resolve_query_set(query_set, 0..used_query_count, resolve, 0);
            encoder.copy_buffer_to_buffer(resolve, 0, readback, 0, used_timestamp_buffer_size);
        }
        encoder.copy_buffer_to_buffer(&c_buffer, 0, &readback_buffer, 0, byte_len);

        let submission = self.queue.submit([encoder.finish()]);
        self.wait_for_submission(submission, cancel, "waiting for GPU readback copy")?;

        let output = read_f32_buffer_cancelable(&self.device, &readback_buffer, elements, cancel)
            .context("reading GPU result buffer")?;
        if let Some(progress) = progress.as_deref_mut() {
            progress.set_gpu_progress(1.0, true);
        }
        let timestamp_pairs = if let Some(readback) = &timestamp_readback {
            read_timestamps(&self.device, readback, chunk_index, cancel).ok()
        } else {
            None
        };
        let (compute_ms, dispatch_times_ms) = dispatch_stats_from_timestamps(
            timestamp_pairs,
            &observed_dispatch_ms,
            self.queue.get_timestamp_period() as f64,
        );

        let total_ms = total_start.elapsed().as_secs_f64() * 1000.0;
        let transfer_sync_ms = compute_ms.map(|ms| (total_ms - ms).max(0.0));
        let stats = GpuDispatchStats::new(
            GpuPath::DirectFullBuffer,
            format!("{chunk_rows}x{n}"),
            &dispatch_times_ms,
            governor.backoff_count,
        );

        Ok(GpuTiming {
            compute_ms,
            total_ms,
            transfer_sync_ms,
            stats,
            output,
        })
    }

    fn multiply_panelized(
        &self,
        n: usize,
        n_u32: u32,
        a: &[f32],
        b: &[f32],
        byte_len: u64,
        use_timestamps: bool,
        gpu_intensity: GpuIntensity,
        cancel: Option<&AtomicBool>,
        mut progress: Option<&mut SingleProgressTracker>,
    ) -> Result<GpuTiming> {
        let elements = n
            .checked_mul(n)
            .ok_or_else(|| anyhow!("matrix size overflow"))?;
        let total_start = Instant::now();
        let (row_block, col_block) = self.block_dimensions(n, gpu_intensity)?;
        let min_row_block =
            align_block_extent(gpu_min_dispatch_rows(gpu_intensity).min(row_block).max(1));
        let mut governor = GpuWorkGovernor::new(row_block, min_row_block, gpu_intensity);

        if let Some(progress) = progress.as_deref_mut() {
            progress.gpu_started = Some(Instant::now());
            progress.set_phase(
                format!("GPU persistent panel compute ({row_block}x{col_block} target)"),
                true,
            );
            progress.set_gpu_progress(0.0, true);
        }

        let (b_packed, panels) = pack_column_panels(b, n, col_block, cancel)?;
        self.ensure_panelized_offsets_aligned(n, min_row_block, &panels)?;

        let a_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Persistent matrix A"),
                contents: bytemuck::cast_slice(a),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let b_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Persistent packed matrix B panels"),
                contents: bytemuck::cast_slice(&b_packed),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let c_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Persistent packed matrix C panels"),
            size: byte_len,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Persistent packed matrix C readback"),
            size: byte_len,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let params_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Persistent panel matrix params"),
                contents: bytemuck::bytes_of(&BlockParams {
                    n: n_u32,
                    rows: 0,
                    cols: 0,
                    _pad: 0,
                }),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

        let max_dispatch_count = panels
            .iter()
            .map(|_| n.div_ceil(min_row_block))
            .sum::<usize>()
            .max(1);
        let timestamp_plan = (self.timestamp_query && use_timestamps)
            .then(|| timestamp_query_plan(max_dispatch_count))
            .flatten();
        let query_set = timestamp_plan.map(|(timestamp_query_count, _)| {
            self.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("Persistent panel GPU timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: timestamp_query_count,
            })
        });
        let timestamp_resolve = timestamp_plan.map(|(_, timestamp_buffer_size)| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Persistent panel timestamp resolve"),
                size: timestamp_buffer_size,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        });
        let timestamp_readback = timestamp_plan.map(|(_, timestamp_buffer_size)| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Persistent panel timestamp readback"),
                size: timestamp_buffer_size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });

        let mut observed_dispatch_ms = Vec::new();
        let mut completed_cells = 0usize;
        let mut query_pair_index = 0usize;
        for panel in &panels {
            self.check_gpu_canceled(cancel)?;
            let b_offset = buffer_len_bytes(panel.element_offset)?;
            let b_bytes = buffer_len_bytes(
                n.checked_mul(panel.cols)
                    .ok_or_else(|| anyhow!("B panel size overflow"))?,
            )?;
            let b_binding_size =
                wgpu::BufferSize::new(b_bytes).ok_or_else(|| anyhow!("empty B panel"))?;

            let mut row_offset = 0usize;
            while row_offset < n {
                self.check_gpu_canceled(cancel)?;
                let rows = governor.row_extent(n - row_offset);
                let a_elements = rows
                    .checked_mul(n)
                    .ok_or_else(|| anyhow!("A panel row size overflow"))?;
                let c_elements = rows
                    .checked_mul(panel.cols)
                    .ok_or_else(|| anyhow!("C panel row size overflow"))?;
                let a_offset = buffer_len_bytes(
                    row_offset
                        .checked_mul(n)
                        .ok_or_else(|| anyhow!("A panel offset overflow"))?,
                )?;
                let c_offset = buffer_len_bytes(
                    panel
                        .element_offset
                        .checked_add(
                            row_offset
                                .checked_mul(panel.cols)
                                .ok_or_else(|| anyhow!("C panel row offset overflow"))?,
                        )
                        .ok_or_else(|| anyhow!("C panel offset overflow"))?,
                )?;
                let a_bytes = buffer_len_bytes(a_elements)?;
                let c_bytes = buffer_len_bytes(c_elements)?;
                let a_binding_size =
                    wgpu::BufferSize::new(a_bytes).ok_or_else(|| anyhow!("empty A panel"))?;
                let c_binding_size =
                    wgpu::BufferSize::new(c_bytes).ok_or_else(|| anyhow!("empty C panel"))?;

                let params = BlockParams {
                    n: n_u32,
                    rows: u32::try_from(rows).context("panel row block exceeds shader limits")?,
                    cols: u32::try_from(panel.cols)
                        .context("panel column block exceeds shader limits")?,
                    _pad: 0,
                };
                self.queue
                    .write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));

                let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Persistent panel matrix bind group"),
                    layout: &self.bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &a_buffer,
                                offset: a_offset,
                                size: Some(a_binding_size),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &b_buffer,
                                offset: b_offset,
                                size: Some(b_binding_size),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &c_buffer,
                                offset: c_offset,
                                size: Some(c_binding_size),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: params_buffer.as_entire_binding(),
                        },
                    ],
                });

                let mut encoder =
                    self.device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("Persistent panel matrix encoder"),
                        });
                {
                    let timestamp_writes =
                        query_set
                            .as_ref()
                            .map(|query_set| wgpu::ComputePassTimestampWrites {
                                query_set,
                                beginning_of_pass_write_index: Some((query_pair_index * 2) as u32),
                                end_of_pass_write_index: Some((query_pair_index * 2 + 1) as u32),
                            });
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("Persistent panel matrix pass"),
                        timestamp_writes,
                    });
                    pass.set_pipeline(&self.blocked_pipeline);
                    pass.set_bind_group(0, &bind_group, &[]);
                    pass.dispatch_workgroups(
                        (panel.cols as u32).div_ceil(TILE_SIZE),
                        (rows as u32).div_ceil(TILE_SIZE),
                        1,
                    );
                }

                let dispatch_start = Instant::now();
                let submission = self.queue.submit([encoder.finish()]);
                self.wait_for_submission(submission, cancel, "waiting for persistent panel chunk")?;
                let observed_ms = dispatch_start.elapsed().as_secs_f64() * 1000.0;
                observed_dispatch_ms.push(observed_ms);
                governor.record_dispatch(observed_ms);
                query_pair_index += 1;
                completed_cells += c_elements;
                row_offset += rows;

                if let Some(progress) = progress.as_deref_mut() {
                    progress.set_gpu_progress(
                        (completed_cells as f32 / elements as f32 * 0.97).clamp(0.0, 0.97),
                        false,
                    );
                }
                if completed_cells < elements {
                    pause_between_gpu_submissions(gpu_intensity, cancel)?;
                }
            }
        }

        self.check_gpu_canceled(cancel)?;
        if let Some(progress) = progress.as_deref_mut() {
            progress.set_phase("GPU panel readback", true);
            progress.set_gpu_progress(0.98, true);
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Persistent panel readback encoder"),
            });
        if let (Some(query_set), Some(resolve), Some(readback)) =
            (&query_set, &timestamp_resolve, &timestamp_readback)
        {
            let used_query_count = (query_pair_index * 2) as u32;
            let used_timestamp_buffer_size = (used_query_count as u64) * 8;
            encoder.resolve_query_set(query_set, 0..used_query_count, resolve, 0);
            encoder.copy_buffer_to_buffer(resolve, 0, readback, 0, used_timestamp_buffer_size);
        }
        encoder.copy_buffer_to_buffer(&c_buffer, 0, &readback_buffer, 0, byte_len);
        let submission = self.queue.submit([encoder.finish()]);
        self.wait_for_submission(submission, cancel, "waiting for persistent panel readback")?;

        let packed_output =
            read_f32_buffer_cancelable(&self.device, &readback_buffer, elements, cancel)
                .context("reading persistent panel GPU result buffer")?;
        let output = unpack_column_panels(&packed_output, n, &panels, cancel)?;
        if let Some(progress) = progress.as_deref_mut() {
            progress.set_gpu_progress(1.0, true);
        }

        let timestamp_pairs = if let Some(readback) = &timestamp_readback {
            read_timestamps(&self.device, readback, query_pair_index, cancel).ok()
        } else {
            None
        };
        let (compute_ms, dispatch_times_ms) = dispatch_stats_from_timestamps(
            timestamp_pairs,
            &observed_dispatch_ms,
            self.queue.get_timestamp_period() as f64,
        );
        let total_ms = total_start.elapsed().as_secs_f64() * 1000.0;
        let transfer_sync_ms = compute_ms.map(|ms| (total_ms - ms).max(0.0));
        let stats = GpuDispatchStats::new(
            GpuPath::PersistentPanelized,
            format!("{row_block}x{col_block}"),
            &dispatch_times_ms,
            governor.backoff_count,
        );

        Ok(GpuTiming {
            compute_ms,
            total_ms,
            transfer_sync_ms,
            stats,
            output,
        })
    }

    fn multiply_blocked(
        &self,
        n: usize,
        n_u32: u32,
        a: &[f32],
        b: &[f32],
        use_timestamps: bool,
        gpu_intensity: GpuIntensity,
        cancel: Option<&AtomicBool>,
        mut progress: Option<&mut SingleProgressTracker>,
    ) -> Result<GpuTiming> {
        let elements = n
            .checked_mul(n)
            .ok_or_else(|| anyhow!("matrix size overflow"))?;
        let total_start = Instant::now();
        let (row_block, col_block) = self.block_dimensions(n, gpu_intensity)?;
        let mut output = vec![0.0_f32; elements];
        let row_blocks = n.div_ceil(row_block);
        let col_blocks = n.div_ceil(col_block);
        let total_blocks = row_blocks
            .checked_mul(col_blocks)
            .unwrap_or(usize::MAX)
            .max(1);
        let mut completed_blocks = 0usize;
        let timestamp_enabled = self.timestamp_query && use_timestamps;
        let mut total_compute_ms = 0.0;
        let mut compute_block_count = 0usize;
        let mut dispatch_times_ms = Vec::new();

        if let Some(progress) = progress.as_deref_mut() {
            progress.gpu_started = Some(Instant::now());
            progress.set_phase(
                format!("GPU blocked compute ({row_block}x{col_block} blocks)"),
                true,
            );
            progress.set_gpu_progress(0.0, true);
        }

        for col_offset in (0..n).step_by(col_block) {
            self.check_gpu_canceled(cancel)?;
            let cols = (n - col_offset).min(col_block);
            let b_block = pack_column_block(b, n, col_offset, cols, cancel)?;

            for row_offset in (0..n).step_by(row_block) {
                self.check_gpu_canceled(cancel)?;
                let rows = (n - row_offset).min(row_block);
                let a_block = pack_row_block(a, n, row_offset, rows, cancel)?;
                let block = self.multiply_block(
                    n_u32,
                    rows,
                    cols,
                    &a_block,
                    &b_block,
                    timestamp_enabled,
                    cancel,
                )?;
                dispatch_times_ms.push(block.compute_ms.unwrap_or(block.observed_ms));
                if let Some(compute_ms) = block.compute_ms {
                    total_compute_ms += compute_ms;
                    compute_block_count += 1;
                }

                for row in 0..rows {
                    if row % 8 == 0 {
                        check_canceled(cancel)?;
                    }
                    let output_start = (row_offset + row) * n + col_offset;
                    let block_start = row * cols;
                    output[output_start..output_start + cols]
                        .copy_from_slice(&block.output[block_start..block_start + cols]);
                }
                completed_blocks += 1;
                if let Some(progress) = progress.as_deref_mut() {
                    progress.set_gpu_progress(completed_blocks as f32 / total_blocks as f32, false);
                }
                if completed_blocks < total_blocks {
                    pause_between_gpu_submissions(gpu_intensity, cancel)?;
                }
            }
        }

        if let Some(progress) = progress.as_deref_mut() {
            progress.set_gpu_progress(1.0, true);
        }

        let total_ms = total_start.elapsed().as_secs_f64() * 1000.0;
        let compute_ms = (compute_block_count > 0).then_some(total_compute_ms);
        let stats = GpuDispatchStats::new(
            GpuPath::StreamingBlocked,
            format!("{row_block}x{col_block}"),
            &dispatch_times_ms,
            0,
        );
        Ok(GpuTiming {
            compute_ms,
            total_ms,
            transfer_sync_ms: compute_ms.map(|ms| (total_ms - ms).max(0.0)),
            stats,
            output,
        })
    }

    fn multiply_block(
        &self,
        n: u32,
        rows: usize,
        cols: usize,
        a_block: &[f32],
        b_block: &[f32],
        use_timestamps: bool,
        cancel: Option<&AtomicBool>,
    ) -> Result<BlockGpuTiming> {
        let rows_u32 = u32::try_from(rows).context("row block exceeds GPU shader limits")?;
        let cols_u32 = u32::try_from(cols).context("column block exceeds GPU shader limits")?;
        let a_bytes = buffer_len_bytes(a_block.len())?;
        let b_bytes = buffer_len_bytes(b_block.len())?;
        let c_elements = rows
            .checked_mul(cols)
            .ok_or_else(|| anyhow!("output block size overflow"))?;
        let c_bytes = buffer_len_bytes(c_elements)?;
        self.ensure_block_buffer_fits("A row block", a_bytes)?;
        self.ensure_block_buffer_fits("B column block", b_bytes)?;
        self.ensure_block_buffer_fits("C output block", c_bytes)?;

        let a_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Blocked matrix A rows"),
                contents: bytemuck::cast_slice(a_block),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let b_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Blocked matrix B columns"),
                contents: bytemuck::cast_slice(b_block),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let c_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Blocked matrix C output"),
            size: c_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Blocked matrix C readback"),
            size: c_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let params = BlockParams {
            n,
            rows: rows_u32,
            cols: cols_u32,
            _pad: 0,
        };
        let params_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Blocked matrix params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blocked matrix multiplication bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: a_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: b_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: c_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        let timestamp_enabled = self.timestamp_query && use_timestamps;
        let query_set = timestamp_enabled.then(|| {
            self.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("Blocked GPU compute timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            })
        });
        let timestamp_resolve = timestamp_enabled.then(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Blocked timestamp resolve"),
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        });
        let timestamp_readback = timestamp_enabled.then(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Blocked timestamp readback"),
                size: 16,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Blocked matrix multiplication encoder"),
            });
        {
            let timestamp_writes =
                query_set
                    .as_ref()
                    .map(|query_set| wgpu::ComputePassTimestampWrites {
                        query_set,
                        beginning_of_pass_write_index: Some(0),
                        end_of_pass_write_index: Some(1),
                    });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Blocked matrix multiplication pass"),
                timestamp_writes,
            });
            pass.set_pipeline(&self.blocked_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(
                cols_u32.div_ceil(TILE_SIZE),
                rows_u32.div_ceil(TILE_SIZE),
                1,
            );
        }
        if let (Some(query_set), Some(resolve), Some(readback)) =
            (&query_set, &timestamp_resolve, &timestamp_readback)
        {
            encoder.resolve_query_set(query_set, 0..2, resolve, 0);
            encoder.copy_buffer_to_buffer(resolve, 0, readback, 0, 16);
        }
        encoder.copy_buffer_to_buffer(&c_buffer, 0, &readback_buffer, 0, c_bytes);

        let dispatch_start = Instant::now();
        let submission = self.queue.submit([encoder.finish()]);
        self.wait_for_submission(submission, cancel, "waiting for blocked GPU matrix chunk")?;
        let observed_ms = dispatch_start.elapsed().as_secs_f64() * 1000.0;
        let output = read_f32_buffer_cancelable(&self.device, &readback_buffer, c_elements, cancel)
            .context("reading blocked GPU result buffer")?;
        let compute_ms = if let Some(readback) = &timestamp_readback {
            read_timestamps(&self.device, readback, 1, cancel)
                .ok()
                .and_then(|timestamps| timestamps.into_iter().next())
                .map(|[start, end]| {
                    let delta = end.saturating_sub(start);
                    (delta as f64 * self.queue.get_timestamp_period() as f64) / 1_000_000.0
                })
        } else {
            None
        };

        Ok(BlockGpuTiming {
            compute_ms,
            observed_ms,
            output,
        })
    }

    fn needs_blocked_path(&self, matrix_byte_len: u64) -> bool {
        std::env::var_os(FORCE_BLOCKED_GPU_ENV).is_some()
            || matrix_byte_len > self.max_storage_buffer_binding_size
            || matrix_byte_len > self.max_buffer_size
    }

    fn can_use_panelized_path(
        &self,
        n: usize,
        matrix_byte_len: u64,
        gpu_intensity: GpuIntensity,
    ) -> Result<bool> {
        if matrix_byte_len > self.max_buffer_size {
            return Ok(false);
        }
        let (row_block, col_block) = self.block_dimensions(n, gpu_intensity)?;
        let min_row_block =
            align_block_extent(gpu_min_dispatch_rows(gpu_intensity).min(row_block).max(1));
        let panels = column_panel_descriptors(n, col_block)?;
        Ok(self
            .panelized_offsets_aligned(n, min_row_block, &panels)
            .is_ok())
    }

    fn block_dimensions(&self, n: usize, gpu_intensity: GpuIntensity) -> Result<(usize, usize)> {
        let limit_bytes = self
            .max_storage_buffer_binding_size
            .min(self.max_buffer_size)
            .max(std::mem::size_of::<f32>() as u64);
        let limit_floats = (limit_bytes / std::mem::size_of::<f32>() as u64) as usize;
        let max_rows_or_cols = (limit_floats / n).max(1);
        let (target_rows, target_cols) = gpu_block_targets(gpu_intensity);
        let rows = align_block_extent(target_rows.min(max_rows_or_cols));
        let cols = align_block_extent(target_cols.min(max_rows_or_cols));

        let a_bytes = buffer_len_bytes(
            rows.checked_mul(n)
                .ok_or_else(|| anyhow!("A block overflow"))?,
        )?;
        let b_bytes = buffer_len_bytes(
            n.checked_mul(cols)
                .ok_or_else(|| anyhow!("B block overflow"))?,
        )?;
        let c_bytes = buffer_len_bytes(
            rows.checked_mul(cols)
                .ok_or_else(|| anyhow!("C block overflow"))?,
        )?;
        self.ensure_block_buffer_fits("A row block", a_bytes)?;
        self.ensure_block_buffer_fits("B column block", b_bytes)?;
        self.ensure_block_buffer_fits("C output block", c_bytes)?;
        Ok((rows, cols))
    }

    fn ensure_panelized_offsets_aligned(
        &self,
        n: usize,
        row_block: usize,
        panels: &[ColumnPanel],
    ) -> Result<()> {
        self.panelized_offsets_aligned(n, row_block, panels)
    }

    fn panelized_offsets_aligned(
        &self,
        n: usize,
        row_block: usize,
        panels: &[ColumnPanel],
    ) -> Result<()> {
        let alignment = self.min_storage_buffer_offset_alignment;
        for panel in panels {
            let b_offset = buffer_len_bytes(panel.element_offset)?;
            if !aligned_storage_offset(b_offset, alignment) {
                return Err(anyhow!(
                    "packed B panel offset {} is not aligned to {} bytes",
                    b_offset,
                    alignment
                ));
            }
            let mut row_offset = 0usize;
            while row_offset < n {
                let a_offset = buffer_len_bytes(
                    row_offset
                        .checked_mul(n)
                        .ok_or_else(|| anyhow!("A panel offset overflow"))?,
                )?;
                let c_offset = buffer_len_bytes(
                    panel
                        .element_offset
                        .checked_add(
                            row_offset
                                .checked_mul(panel.cols)
                                .ok_or_else(|| anyhow!("C panel row offset overflow"))?,
                        )
                        .ok_or_else(|| anyhow!("C panel offset overflow"))?,
                )?;
                if !aligned_storage_offset(a_offset, alignment) {
                    return Err(anyhow!(
                        "A panel offset {} is not aligned to {} bytes",
                        a_offset,
                        alignment
                    ));
                }
                if !aligned_storage_offset(c_offset, alignment) {
                    return Err(anyhow!(
                        "packed C panel offset {} is not aligned to {} bytes",
                        c_offset,
                        alignment
                    ));
                }
                row_offset += row_block.min(n - row_offset).max(1);
            }
        }
        Ok(())
    }

    fn ensure_block_buffer_fits(&self, label: &str, bytes: u64) -> Result<()> {
        if bytes > self.max_storage_buffer_binding_size {
            return Err(anyhow!(
                "{label} requires {}, above this adapter's storage binding limit of {}",
                format_bytes(bytes),
                format_bytes(self.max_storage_buffer_binding_size)
            ));
        }
        if bytes > self.max_buffer_size {
            return Err(anyhow!(
                "{label} requires {}, above this adapter's buffer size limit of {}",
                format_bytes(bytes),
                format_bytes(self.max_buffer_size)
            ));
        }
        Ok(())
    }

    fn wait_for_submission(
        &self,
        _submission: wgpu::SubmissionIndex,
        cancel: Option<&AtomicBool>,
        context: &'static str,
    ) -> Result<()> {
        let (done_tx, done_rx) = mpsc::channel();
        self.queue.on_submitted_work_done(move || {
            let _ = done_tx.send(());
        });

        loop {
            self.check_gpu_canceled(cancel)?;
            match done_rx.try_recv() {
                Ok(()) => return Ok(()),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(anyhow!("GPU completion callback channel closed")).context(context);
                }
            }
            match self.device.poll(wgpu::PollType::Poll) {
                Ok(_) => {}
                Err(err) => return Err(anyhow!(err)).context(context),
            }
            match done_rx.try_recv() {
                Ok(()) => return Ok(()),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(anyhow!("GPU completion callback channel closed")).context(context);
                }
            }
            thread::sleep(Duration::from_millis(GPU_WAIT_POLL_MS));
        }
    }

    fn check_gpu_canceled(&self, cancel: Option<&AtomicBool>) -> Result<()> {
        if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
            self.device.destroy();
            Err(anyhow!("Benchmark canceled while GPU work was running"))
        } else {
            Ok(())
        }
    }
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn gpu_dispatch_chunk_rows(size: usize, gpu_intensity: GpuIntensity) -> usize {
    if size <= 1024 {
        size.max(1)
    } else {
        let rows = match gpu_intensity {
            GpuIntensity::Safe => GPU_SAFE_CHUNK_ROWS,
            GpuIntensity::Balanced => GPU_BALANCED_CHUNK_ROWS,
            GpuIntensity::High => GPU_HIGH_CHUNK_ROWS,
        };
        rows.min(size).max(1)
    }
}

fn gpu_min_dispatch_rows(gpu_intensity: GpuIntensity) -> usize {
    match gpu_intensity {
        GpuIntensity::Safe => 8,
        GpuIntensity::Balanced => 16,
        GpuIntensity::High => 32,
    }
}

fn gpu_block_targets(gpu_intensity: GpuIntensity) -> (usize, usize) {
    match gpu_intensity {
        GpuIntensity::Safe => (GPU_SAFE_BLOCK_ROWS, GPU_SAFE_BLOCK_COLS),
        GpuIntensity::Balanced => (GPU_BALANCED_BLOCK_ROWS, GPU_BALANCED_BLOCK_COLS),
        GpuIntensity::High => (GPU_HIGH_BLOCK_ROWS, GPU_HIGH_BLOCK_COLS),
    }
}

fn gpu_submission_pause(gpu_intensity: GpuIntensity) -> Duration {
    match gpu_intensity {
        GpuIntensity::Safe => Duration::from_millis(3),
        GpuIntensity::Balanced => Duration::from_millis(1),
        GpuIntensity::High => Duration::from_millis(0),
    }
}

fn gpu_hard_backoff_ms(gpu_intensity: GpuIntensity) -> f64 {
    match gpu_intensity {
        GpuIntensity::Safe => 500.0,
        GpuIntensity::Balanced => 750.0,
        GpuIntensity::High => 1000.0,
    }
}

fn pause_between_gpu_submissions(
    gpu_intensity: GpuIntensity,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    let pause = gpu_submission_pause(gpu_intensity);
    if pause.is_zero() {
        check_canceled(cancel)?;
        return Ok(());
    }

    let started = Instant::now();
    while started.elapsed() < pause {
        check_canceled(cancel)?;
        thread::sleep(Duration::from_millis(1));
    }
    check_canceled(cancel)
}

fn dispatch_stats_from_timestamps(
    timestamp_pairs: Option<Vec<[u64; 2]>>,
    observed_dispatch_ms: &[f64],
    timestamp_period: f64,
) -> (Option<f64>, Vec<f64>) {
    if let Some(timestamp_pairs) = timestamp_pairs {
        let dispatch_times = timestamp_pairs
            .into_iter()
            .map(|[start, end]| {
                let delta = end.saturating_sub(start);
                (delta as f64 * timestamp_period) / 1_000_000.0
            })
            .collect::<Vec<_>>();
        let compute_ms = (!dispatch_times.is_empty()).then(|| dispatch_times.iter().sum());
        (compute_ms, dispatch_times)
    } else {
        (None, observed_dispatch_ms.to_vec())
    }
}

fn timestamp_query_plan(pair_count: usize) -> Option<(u32, u64)> {
    let query_count = pair_count.checked_mul(2)?;
    if query_count > WGPU_MAX_QUERY_COUNT {
        return None;
    }
    let query_count = u32::try_from(query_count).ok()?;
    let buffer_size = u64::from(query_count).checked_mul(8)?;
    Some((query_count, buffer_size))
}

fn aligned_storage_offset(offset: u64, alignment: u32) -> bool {
    let alignment = u64::from(alignment.max(1));
    offset % alignment == 0
}

fn align_block_extent(value: usize) -> usize {
    if value >= TILE_SIZE as usize {
        (value / TILE_SIZE as usize).max(1) * TILE_SIZE as usize
    } else {
        value.max(1)
    }
}

fn buffer_len_bytes(elements: usize) -> Result<u64> {
    elements
        .checked_mul(std::mem::size_of::<f32>())
        .map(|bytes| bytes as u64)
        .ok_or_else(|| anyhow!("buffer byte length overflow"))
}

fn pack_row_block(
    source: &[f32],
    size: usize,
    row_offset: usize,
    rows: usize,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<f32>> {
    let mut block = Vec::with_capacity(rows * size);
    for row in 0..rows {
        if row % 8 == 0 {
            check_canceled(cancel)?;
        }
        let start = (row_offset + row) * size;
        block.extend_from_slice(&source[start..start + size]);
    }
    Ok(block)
}

fn pack_column_block(
    source: &[f32],
    size: usize,
    col_offset: usize,
    cols: usize,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<f32>> {
    let mut block = Vec::with_capacity(size * cols);
    for row in 0..size {
        if row % 32 == 0 {
            check_canceled(cancel)?;
        }
        let start = row * size + col_offset;
        block.extend_from_slice(&source[start..start + cols]);
    }
    Ok(block)
}

fn column_panel_descriptors(size: usize, panel_cols: usize) -> Result<Vec<ColumnPanel>> {
    if panel_cols == 0 {
        return Err(anyhow!("panel column count must be positive"));
    }
    let mut panels = Vec::new();
    let mut element_offset = 0usize;
    for col_offset in (0..size).step_by(panel_cols) {
        let cols = (size - col_offset).min(panel_cols);
        panels.push(ColumnPanel {
            col_offset,
            cols,
            element_offset,
        });
        element_offset = element_offset
            .checked_add(
                size.checked_mul(cols)
                    .ok_or_else(|| anyhow!("column panel size overflow"))?,
            )
            .ok_or_else(|| anyhow!("column panel offset overflow"))?;
    }
    Ok(panels)
}

fn pack_column_panels(
    source: &[f32],
    size: usize,
    panel_cols: usize,
    cancel: Option<&AtomicBool>,
) -> Result<(Vec<f32>, Vec<ColumnPanel>)> {
    let panels = column_panel_descriptors(size, panel_cols)?;
    let expected = size
        .checked_mul(size)
        .ok_or_else(|| anyhow!("packed panel size overflow"))?;
    let mut packed = Vec::with_capacity(expected);
    for panel in &panels {
        for row in 0..size {
            if row % 32 == 0 {
                check_canceled(cancel)?;
            }
            let start = row * size + panel.col_offset;
            packed.extend_from_slice(&source[start..start + panel.cols]);
        }
    }
    Ok((packed, panels))
}

fn unpack_column_panels(
    packed: &[f32],
    size: usize,
    panels: &[ColumnPanel],
    cancel: Option<&AtomicBool>,
) -> Result<Vec<f32>> {
    let mut output = vec![0.0_f32; size * size];
    for panel in panels {
        for row in 0..size {
            if row % 32 == 0 {
                check_canceled(cancel)?;
            }
            let source_start = panel.element_offset + row * panel.cols;
            let output_start = row * size + panel.col_offset;
            output[output_start..output_start + panel.cols]
                .copy_from_slice(&packed[source_start..source_start + panel.cols]);
        }
    }
    Ok(output)
}

fn read_f32_buffer_cancelable(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
    elements: usize,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<f32>> {
    let slice = buffer.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result.map_err(|err| err.to_string()));
    });
    wait_for_map_callback(device, &rx, cancel, "polling mapped result buffer")?;
    let data = slice.get_mapped_range();
    let output = bytemuck::cast_slice::<u8, f32>(&data)
        .iter()
        .copied()
        .take(elements)
        .collect();
    drop(data);
    buffer.unmap();
    Ok(output)
}

fn read_timestamps(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
    pair_count: usize,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<[u64; 2]>> {
    let slice = buffer.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result.map_err(|err| err.to_string()));
    });
    wait_for_map_callback(device, &rx, cancel, "polling mapped timestamp buffer")?;
    let data = slice.get_mapped_range();
    let timestamps = bytemuck::cast_slice::<u8, u64>(&data);
    let result = timestamps
        .chunks_exact(2)
        .take(pair_count)
        .map(|pair| [pair[0], pair[1]])
        .collect();
    drop(data);
    buffer.unmap();
    Ok(result)
}

fn wait_for_map_callback(
    device: &wgpu::Device,
    rx: &Receiver<Result<(), String>>,
    cancel: Option<&AtomicBool>,
    context: &'static str,
) -> Result<()> {
    loop {
        check_canceled(cancel)?;
        match rx.try_recv() {
            Ok(result) => return result.map_err(|err| anyhow!(err)),
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(anyhow!("map callback channel closed"));
            }
        }
        match device.poll(wgpu::PollType::Poll) {
            Ok(_) => {}
            Err(err) => return Err(anyhow!(err)).context(context),
        }
        match rx.try_recv() {
            Ok(result) => return result.map_err(|err| anyhow!(err)),
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(anyhow!("map callback channel closed"));
            }
        }
        thread::sleep(Duration::from_millis(GPU_WAIT_POLL_MS));
    }
}

fn enumerate_adapters() -> Vec<AdapterInfo> {
    let instance = wgpu::Instance::default();
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    let dxgi_memory = query_dxgi_memory_info();
    adapters
        .into_iter()
        .enumerate()
        .map(|(index, adapter)| {
            let info = adapter.get_info();
            let features = adapter.features();
            let memory = match_dxgi_memory_info(&info, &dxgi_memory);
            AdapterInfo {
                index,
                name: info.name,
                backend: info.backend,
                device_type: info.device_type,
                vendor: info.vendor,
                device: info.device,
                driver: info.driver,
                timestamp_query: features.contains(wgpu::Features::TIMESTAMP_QUERY),
                dedicated_vram_bytes: memory.map(|memory| memory.dedicated_vram_bytes),
                dedicated_system_memory_bytes: memory
                    .map(|memory| memory.dedicated_system_memory_bytes),
                shared_system_memory_bytes: memory.map(|memory| memory.shared_system_memory_bytes),
            }
        })
        .collect()
}

#[cfg(windows)]
fn query_dxgi_memory_info() -> Vec<DxgiMemoryInfo> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};

    let mut infos = Vec::new();
    let factory = unsafe { CreateDXGIFactory1::<IDXGIFactory1>() };
    let Ok(factory) = factory else {
        return infos;
    };

    for index in 0..128 {
        let adapter = unsafe { factory.EnumAdapters1(index) };
        let Ok(adapter) = adapter else {
            break;
        };
        if let Ok(desc) = unsafe { adapter.GetDesc1() } {
            infos.push(DxgiMemoryInfo {
                name: String::from_utf16_lossy(&desc.Description)
                    .trim_end_matches('\0')
                    .trim()
                    .to_owned(),
                vendor: desc.VendorId,
                device: desc.DeviceId,
                dedicated_vram_bytes: desc.DedicatedVideoMemory as u64,
                dedicated_system_memory_bytes: desc.DedicatedSystemMemory as u64,
                shared_system_memory_bytes: desc.SharedSystemMemory as u64,
            });
        }
    }

    infos
}

#[cfg(not(windows))]
fn query_dxgi_memory_info() -> Vec<DxgiMemoryInfo> {
    Vec::new()
}

fn match_dxgi_memory_info<'a>(
    info: &wgpu::AdapterInfo,
    memory_infos: &'a [DxgiMemoryInfo],
) -> Option<&'a DxgiMemoryInfo> {
    if info.device != 0 {
        if let Some(memory) = memory_infos
            .iter()
            .find(|memory| memory.vendor == info.vendor && memory.device == info.device)
        {
            return Some(memory);
        }
    }

    let adapter_name = normalize_adapter_name(&info.name);
    memory_infos
        .iter()
        .find(|memory| {
            memory.vendor == info.vendor && normalize_adapter_name(&memory.name) == adapter_name
        })
        .or_else(|| {
            memory_infos.iter().find(|memory| {
                memory.vendor == info.vendor
                    && (normalize_adapter_name(&memory.name).contains(&adapter_name)
                        || adapter_name.contains(&normalize_adapter_name(&memory.name)))
            })
        })
}

fn normalize_adapter_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn detect_cpu_info() -> CpuInfo {
    CpuInfo {
        model: cpu_model_name().unwrap_or_else(|| "Unknown CPU".to_owned()),
        logical_processors: thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1),
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn cpu_model_name() -> Option<String> {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::__cpuid;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::__cpuid;

    let max_extended = __cpuid(0x8000_0000).eax;
    if max_extended < 0x8000_0004 {
        return None;
    }

    let mut bytes = Vec::with_capacity(48);
    for leaf in 0x8000_0002..=0x8000_0004 {
        let result = __cpuid(leaf);
        for register in [result.eax, result.ebx, result.ecx, result.edx] {
            bytes.extend_from_slice(&register.to_le_bytes());
        }
    }

    String::from_utf8(bytes)
        .ok()
        .map(|value| value.trim_matches('\0').trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn cpu_model_name() -> Option<String> {
    None
}

#[cfg(test)]
fn generate_matrices(size: usize) -> Result<(Vec<f32>, Vec<f32>)> {
    generate_matrices_cancelable(size, None)
}

fn generate_matrices_cancelable(
    size: usize,
    cancel: Option<&AtomicBool>,
) -> Result<(Vec<f32>, Vec<f32>)> {
    let elements = size
        .checked_mul(size)
        .ok_or_else(|| anyhow!("matrix size overflow"))?;
    let mut a = Vec::with_capacity(elements);
    let mut b = Vec::with_capacity(elements);
    for i in 0..elements {
        if i % CANCEL_CHECK_INTERVAL == 0 {
            check_canceled(cancel)?;
        }
        a.push((i % 97) as f32 / 97.0);
        b.push(((i * 3 + 1) % 89) as f32 / 89.0);
    }
    Ok((a, b))
}

#[cfg(test)]
fn cpu_multiply(size: usize, a: &[f32], b: &[f32]) -> (Vec<f32>, f64) {
    cpu_multiply_cancelable(size, a, b, None, None)
        .expect("uncancelable CPU multiply cannot be canceled")
}

fn cpu_multiply_cancelable(
    size: usize,
    a: &[f32],
    b: &[f32],
    cancel: Option<&AtomicBool>,
    mut progress: Option<&mut SingleProgressTracker>,
) -> Result<(Vec<f32>, f64)> {
    let mut c = vec![0.0_f32; size * size];
    let tile = 32usize;
    let blocks_per_dim = size.div_ceil(tile);
    let total_blocks = (blocks_per_dim * blocks_per_dim * blocks_per_dim).max(1);
    let completed_blocks = Arc::new(AtomicUsize::new(0));
    let start = Instant::now();

    if let Some(progress) = progress.as_deref_mut() {
        progress.set_phase("CPU computing", true);
        progress.set_cpu_progress(0.0, true);
    }
    let ticker = progress.as_deref_mut().and_then(|progress| {
        progress.start_cpu_ticker(Arc::clone(&completed_blocks), total_blocks)
    });

    let worker_count = cpu_worker_count(size);
    let rows_per_worker = size.div_ceil(worker_count);
    let result = thread::scope(|scope| -> Result<()> {
        let mut handles = Vec::new();
        for (worker_index, c_rows) in c.chunks_mut(rows_per_worker * size).enumerate() {
            let row_start = worker_index * rows_per_worker;
            let row_end = row_start + c_rows.len() / size;
            let completed_blocks = Arc::clone(&completed_blocks);
            handles.push(scope.spawn(move || -> Result<()> {
                for ii in (row_start..row_end).step_by(tile) {
                    check_canceled(cancel)?;
                    let i_end = (ii + tile).min(row_end);
                    for kk in (0..size).step_by(tile) {
                        check_canceled(cancel)?;
                        let k_end = (kk + tile).min(size);
                        for jj in (0..size).step_by(tile) {
                            check_canceled(cancel)?;
                            let j_end = (jj + tile).min(size);
                            for i in ii..i_end {
                                let c_row = (i - row_start) * size;
                                let a_row = i * size;
                                for k in kk..k_end {
                                    let a_val = a[a_row + k];
                                    let b_row = k * size;
                                    for j in jj..j_end {
                                        c_rows[c_row + j] += a_val * b[b_row + j];
                                    }
                                }
                            }
                            completed_blocks.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
                Ok(())
            }));
        }

        for handle in handles {
            handle
                .join()
                .map_err(|_| anyhow!("CPU worker thread panicked"))??;
        }
        Ok(())
    });

    if let Some(ticker) = ticker {
        ticker.stop();
    }
    result?;

    if let Some(progress) = progress.as_deref_mut() {
        progress.set_cpu_progress(1.0, true);
    }

    Ok((c, start.elapsed().as_secs_f64() * 1000.0))
}

fn cpu_worker_count(size: usize) -> usize {
    if size < 256 {
        return 1;
    }

    thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .saturating_sub(1)
        .max(1)
        .min(size)
}

fn cpu_multiply_row_sample_cancelable(
    size: usize,
    a: &[f32],
    b: &[f32],
    row_offset: usize,
    row_count: usize,
    cancel: Option<&AtomicBool>,
) -> Result<f64> {
    let elements = size
        .checked_mul(size)
        .ok_or_else(|| anyhow!("matrix size overflow"))?;
    if a.len() != elements || b.len() != elements {
        return Err(anyhow!("matrix data length does not match {size}x{size}"));
    }
    if row_offset >= size {
        return Err(anyhow!("row offset exceeds matrix size"));
    }
    let row_count = row_count.min(size - row_offset).max(1);
    let mut c = vec![0.0_f32; row_count * size];
    let tile = 32usize;
    let worker_count = cpu_worker_count(size).min(row_count).max(1);
    let rows_per_worker = row_count.div_ceil(worker_count);
    let start = Instant::now();

    thread::scope(|scope| -> Result<()> {
        let mut handles = Vec::new();
        for (worker_index, c_rows) in c.chunks_mut(rows_per_worker * size).enumerate() {
            let row_start = worker_index * rows_per_worker;
            let row_end = row_start + c_rows.len() / size;
            handles.push(scope.spawn(move || -> Result<()> {
                for ii in (row_start..row_end).step_by(tile) {
                    check_canceled(cancel)?;
                    let i_end = (ii + tile).min(row_end);
                    for kk in (0..size).step_by(tile) {
                        check_canceled(cancel)?;
                        let k_end = (kk + tile).min(size);
                        for jj in (0..size).step_by(tile) {
                            check_canceled(cancel)?;
                            let j_end = (jj + tile).min(size);
                            for i in ii..i_end {
                                let c_row = (i - row_start) * size;
                                let a_row = (row_offset + i) * size;
                                for k in kk..k_end {
                                    let a_val = a[a_row + k];
                                    let b_row = k * size;
                                    for j in jj..j_end {
                                        c_rows[c_row + j] += a_val * b[b_row + j];
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(())
            }));
        }

        for handle in handles {
            handle
                .join()
                .map_err(|_| anyhow!("CPU estimate worker thread panicked"))??;
        }
        Ok(())
    })?;

    Ok(start.elapsed().as_secs_f64() * 1000.0)
}

fn estimate_cpu_multiply_ms(
    size: usize,
    a: &[f32],
    b: &[f32],
    cpu_info: &CpuInfo,
    cancel: Option<&AtomicBool>,
    mut progress: Option<&mut SingleProgressTracker>,
) -> Result<f64> {
    if let Some(progress) = progress.as_deref_mut() {
        progress.set_phase(
            format!("Estimating CPU baseline on {}", cpu_info.model),
            true,
        );
        progress.set_cpu_progress(0.0, true);
    }

    let warm_size = CPU_ESTIMATE_MIN_SAMPLE_SIZE.min(size);
    if warm_size >= 2 {
        let warm_a = copy_top_left_submatrix(a, size, warm_size, cancel)?;
        let warm_b = copy_top_left_submatrix(b, size, warm_size, cancel)?;
        let _ = cpu_multiply_cancelable(warm_size, &warm_a, &warm_b, cancel, None)?;
    }

    check_canceled(cancel)?;
    let estimate_ms = if size <= CPU_ESTIMATE_BASE_SAMPLE_SIZE {
        let (_, elapsed_ms) = cpu_multiply_cancelable(size, a, b, cancel, None)?;
        elapsed_ms
    } else {
        let mut batch_rows = cpu_estimate_row_sample_count(size, cpu_info);
        let mut row_offset = 0usize;
        let mut completed_rows = 0usize;
        let mut elapsed_ms = 0.0;
        if let Some(progress) = progress.as_deref_mut() {
            progress.set_phase(
                format!(
                    "Estimating CPU baseline for ~{}",
                    format_elapsed(CPU_ESTIMATE_TARGET_MS / 1000.0)
                ),
                true,
            );
        }

        while row_offset < size && elapsed_ms < CPU_ESTIMATE_TARGET_MS {
            check_canceled(cancel)?;
            let rows_this_batch = batch_rows.min(size - row_offset).max(1);
            let batch_ms = cpu_multiply_row_sample_cancelable(
                size,
                a,
                b,
                row_offset,
                rows_this_batch,
                cancel,
            )?;
            elapsed_ms += batch_ms;
            completed_rows += rows_this_batch;
            row_offset += rows_this_batch;

            if let Some(progress) = progress.as_deref_mut() {
                progress.set_cpu_progress(
                    (elapsed_ms / CPU_ESTIMATE_TARGET_MS).min(0.95) as f32,
                    false,
                );
            }

            if elapsed_ms > 0.0 && elapsed_ms < CPU_ESTIMATE_TARGET_MS && row_offset < size {
                let ms_per_row = elapsed_ms / completed_rows as f64;
                let remaining_target_ms = CPU_ESTIMATE_TARGET_MS - elapsed_ms;
                let target_next_rows = (remaining_target_ms / ms_per_row).ceil() as usize;
                batch_rows = target_next_rows.clamp(
                    1,
                    cpu_estimate_row_sample_count(size, cpu_info)
                        .saturating_mul(2)
                        .max(1),
                );
            }
        }

        elapsed_ms * (size as f64 / completed_rows.max(1) as f64)
    };

    if let Some(progress) = progress.as_deref_mut() {
        progress.set_cpu_progress(1.0, true);
    }

    Ok(estimate_ms)
}

#[cfg(test)]
fn cpu_estimate_sample_size(size: usize, cpu_info: &CpuInfo) -> usize {
    if size < 32 {
        return size.max(1);
    }

    let model = cpu_info.model.to_ascii_lowercase();
    let target = if model.contains("threadripper")
        || model.contains("ryzen 9")
        || model.contains("core(tm) i9")
        || model.contains("core ultra 9")
        || cpu_info.logical_processors >= 24
    {
        CPU_ESTIMATE_MAX_SAMPLE_SIZE
    } else if model.contains("ryzen 7")
        || model.contains("core(tm) i7")
        || model.contains("core ultra 7")
        || cpu_info.logical_processors >= 12
    {
        CPU_ESTIMATE_MID_SAMPLE_SIZE
    } else {
        CPU_ESTIMATE_BASE_SAMPLE_SIZE
    };

    let sample_size = target.min(size).max(CPU_ESTIMATE_MIN_SAMPLE_SIZE.min(size));
    (sample_size / 32).max(1) * 32
}

fn cpu_estimate_row_sample_count(size: usize, cpu_info: &CpuInfo) -> usize {
    if size <= CPU_ESTIMATE_BASE_SAMPLE_SIZE {
        return size.max(1);
    }

    let model = cpu_info.model.to_ascii_lowercase();
    let target_cells = if model.contains("threadripper")
        || model.contains("ryzen 9")
        || model.contains("core(tm) i9")
        || model.contains("core ultra 9")
        || cpu_info.logical_processors >= 24
    {
        CPU_ESTIMATE_HIGH_ROW_CELLS
    } else if model.contains("ryzen 7")
        || model.contains("core(tm) i7")
        || model.contains("core ultra 7")
        || cpu_info.logical_processors >= 12
    {
        CPU_ESTIMATE_MID_ROW_CELLS
    } else {
        CPU_ESTIMATE_BASE_ROW_CELLS
    };

    let worker_floor = cpu_worker_count(size).min(size).max(1);
    let rows = target_cells.div_ceil(size).max(worker_floor).min(size);
    rows.min(CPU_ESTIMATE_MAX_ROWS).max(1)
}

fn copy_top_left_submatrix(
    source: &[f32],
    source_size: usize,
    sample_size: usize,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<f32>> {
    if sample_size > source_size {
        return Err(anyhow!("sample size exceeds source matrix size"));
    }
    let mut sample = Vec::with_capacity(sample_size * sample_size);
    for row in 0..sample_size {
        if row % 32 == 0 {
            check_canceled(cancel)?;
        }
        let start = row * source_size;
        sample.extend_from_slice(&source[start..start + sample_size]);
    }
    Ok(sample)
}

#[cfg(test)]
fn validate(cpu: &[f32], gpu: &[f32], size: usize) -> String {
    validate_cancelable(cpu, gpu, size, None).expect("uncancelable validation cannot be canceled")
}

fn validate_cancelable(
    cpu: &[f32],
    gpu: &[f32],
    size: usize,
    cancel: Option<&AtomicBool>,
) -> Result<String> {
    if cpu.len() != gpu.len() {
        return Ok(format!(
            "Failed: CPU len {}, GPU len {}",
            cpu.len(),
            gpu.len()
        ));
    }

    let mut max_abs = 0.0_f32;
    let mut max_rel = 0.0_f32;
    for (index, (&cpu_value, &gpu_value)) in cpu.iter().zip(gpu.iter()).enumerate() {
        if index % CANCEL_CHECK_INTERVAL == 0 {
            check_canceled(cancel)?;
        }
        let diff = (cpu_value - gpu_value).abs();
        max_abs = max_abs.max(diff);
        max_rel = max_rel.max(diff / cpu_value.abs().max(1.0));
    }

    let abs_tol = 0.02_f32.max(size as f32 * 0.00005);
    let rel_tol = 0.0025_f32;
    if max_abs <= abs_tol || max_rel <= rel_tol {
        Ok(format!(
            "Passed (max abs {max_abs:.5}, max rel {max_rel:.5})"
        ))
    } else {
        Ok(format!(
            "Failed (max abs {max_abs:.5}, max rel {max_rel:.5})"
        ))
    }
}

fn validate_sampled(
    a: &[f32],
    b: &[f32],
    gpu: &[f32],
    size: usize,
    cancel: Option<&AtomicBool>,
) -> Result<String> {
    if gpu.len() != size * size {
        return Ok(format!(
            "Failed: GPU len {}, expected {}",
            gpu.len(),
            size * size
        ));
    }

    let sample_count = VALIDATION_SAMPLE_POINTS.min(size * size).max(1);
    let mut max_abs = 0.0_f32;
    let mut max_rel = 0.0_f32;

    for index in 0..sample_count {
        check_canceled(cancel)?;
        let row = sample_index(index, size, 0x9E37_79B9);
        let col = sample_index(index, size, 0x85EB_CA6B);
        let mut expected = 0.0_f32;
        let a_row = row * size;
        for k in 0..size {
            expected += a[a_row + k] * b[k * size + col];
        }
        let actual = gpu[row * size + col];
        let diff = (expected - actual).abs();
        max_abs = max_abs.max(diff);
        max_rel = max_rel.max(diff / expected.abs().max(1.0));
    }

    let abs_tol = 0.02_f32.max(size as f32 * 0.00005);
    let rel_tol = 0.0025_f32;
    if max_abs <= abs_tol || max_rel <= rel_tol {
        Ok(format!(
            "Sampled pass ({sample_count} points, max abs {max_abs:.5}, max rel {max_rel:.5})"
        ))
    } else {
        Ok(format!(
            "Sampled fail ({sample_count} points, max abs {max_abs:.5}, max rel {max_rel:.5})"
        ))
    }
}

fn sample_index(index: usize, size: usize, salt: usize) -> usize {
    if size == 1 {
        0
    } else {
        let mixed = index
            .wrapping_mul(1_103_515_245usize)
            .wrapping_add(12_345usize)
            ^ salt;
        mixed % size
    }
}

fn run_single(
    size: usize,
    adapter: AdapterInfo,
    validate_output: bool,
    estimate_cpu_time: bool,
    gpu_intensity: GpuIntensity,
) -> Result<BenchmarkResult> {
    let cancel = AtomicBool::new(false);
    run_single_cancelable(
        size,
        adapter,
        validate_output,
        estimate_cpu_time,
        gpu_intensity,
        &cancel,
        None,
    )
}

fn run_single_cancelable(
    size: usize,
    adapter: AdapterInfo,
    validate_output: bool,
    estimate_cpu_time: bool,
    gpu_intensity: GpuIntensity,
    cancel: &AtomicBool,
    progress_tx: Option<Sender<WorkerEvent>>,
) -> Result<BenchmarkResult> {
    let mut progress = SingleProgressTracker::new(size, &adapter, gpu_intensity, progress_tx);
    let cpu_info = detect_cpu_info();
    progress.set_phase("Generating matrices", true);
    let (a, b) = generate_matrices_cancelable(size, Some(cancel))?;
    let (cpu_output, cpu_ms, cpu_estimated) = if estimate_cpu_time {
        let cpu_ms =
            estimate_cpu_multiply_ms(size, &a, &b, &cpu_info, Some(cancel), Some(&mut progress))?;
        (None, cpu_ms, true)
    } else {
        let (cpu_output, cpu_ms) =
            cpu_multiply_cancelable(size, &a, &b, Some(cancel), Some(&mut progress))?;
        (Some(cpu_output), cpu_ms, false)
    };
    check_canceled(Some(cancel))?;
    progress.set_phase("Preparing GPU", true);
    let runner = GpuRunner::new(adapter.index)?;
    check_canceled(Some(cancel))?;
    let gpu = runner.multiply_cancelable(
        size,
        &a,
        &b,
        true,
        gpu_intensity,
        Some(cancel),
        Some(&mut progress),
    )?;
    progress.set_gpu_progress(1.0, true);
    check_canceled_with(
        Some(cancel),
        "Benchmark canceled after the current GPU dispatch completed",
    )?;
    let validation = if validate_output {
        progress.set_phase("Validating GPU output", true);
        if let Some(cpu_output) = cpu_output.as_deref() {
            validate_cancelable(cpu_output, &gpu.output, size, Some(cancel))?
        } else {
            validate_sampled(&a, &b, &gpu.output, size, Some(cancel))?
        }
    } else {
        "Skipped".to_owned()
    };
    progress.set_phase("Benchmark complete", true);
    let speedup = if gpu.total_ms > 0.0 {
        cpu_ms / gpu.total_ms
    } else {
        f64::INFINITY
    };
    Ok(BenchmarkResult {
        size,
        adapter: adapter.label(),
        cpu_model: cpu_info.label(),
        cpu_ms,
        cpu_estimated,
        gpu_compute_ms: gpu.compute_ms,
        gpu_total_ms: gpu.total_ms,
        transfer_sync_ms: gpu.transfer_sync_ms,
        gpu_path: gpu.stats.path,
        gpu_intensity,
        dispatch_count: gpu.stats.dispatch_count,
        tile_shape: gpu.stats.tile_shape,
        last_dispatch_ms: gpu.stats.last_dispatch_ms,
        avg_dispatch_ms: gpu.stats.avg_dispatch_ms,
        max_dispatch_ms: gpu.stats.max_dispatch_ms,
        backoff_count: gpu.stats.backoff_count,
        speedup,
        validation,
        cpu_temperature: TemperatureSummary::default(),
        gpu_temperature: TemperatureSummary::default(),
    })
}

fn check_canceled(cancel: Option<&AtomicBool>) -> Result<()> {
    check_canceled_with(cancel, "Benchmark canceled")
}

fn check_canceled_with(cancel: Option<&AtomicBool>, message: &str) -> Result<()> {
    if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
        Err(anyhow!(message.to_owned()))
    } else {
        Ok(())
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_owned()
    } else {
        "unknown panic payload".to_owned()
    }
}

fn run_repeat(
    size: usize,
    adapter: AdapterInfo,
    mode: RepeatMode,
    gpu_intensity: GpuIntensity,
    cancel: Arc<AtomicBool>,
    tx: Sender<WorkerEvent>,
    duration: Duration,
) -> Result<RepeatProgress> {
    let (a, b) = generate_matrices_cancelable(size, Some(&cancel))?;
    let deadline = Instant::now() + duration;
    let started = Instant::now();
    let duration_s = duration.as_secs_f64();
    let mut iterations = 0_u64;
    let mut total_ms = 0.0;
    let mut total_compute_ms = 0.0;
    let mut compute_count = 0_u64;
    let mut latest_ms = 0.0;
    let mut last_emit = Instant::now() - Duration::from_secs(1);

    let mut emit = |iterations: u64,
                    latest_ms: f64,
                    total_ms: f64,
                    total_compute_ms: f64,
                    compute_count: u64,
                    canceled: bool,
                    force: bool| {
        let now = Instant::now();
        let elapsed_s = (now - started).as_secs_f64().min(duration.as_secs_f64());
        let progress = RepeatProgress {
            mode,
            size,
            duration_s,
            elapsed_s: elapsed_s.min(duration_s),
            iterations,
            latest_ms,
            average_total_ms: if iterations == 0 {
                0.0
            } else {
                total_ms / iterations as f64
            },
            average_compute_ms: if compute_count == 0 {
                None
            } else {
                Some(total_compute_ms / compute_count as f64)
            },
            canceled,
        };
        if force || now.duration_since(last_emit) >= Duration::from_millis(PROGRESS_SAMPLE_MS) {
            let _ = tx.send(WorkerEvent::RepeatProgress(progress.clone()));
            last_emit = now;
        }
        progress
    };

    match mode {
        RepeatMode::Cpu => {
            while Instant::now() < deadline && !cancel.load(Ordering::Relaxed) {
                let (_, elapsed_ms) =
                    match cpu_multiply_cancelable(size, &a, &b, Some(&cancel), None) {
                        Ok(result) => result,
                        Err(_) if cancel.load(Ordering::Relaxed) => break,
                        Err(err) => return Err(err),
                    };
                latest_ms = elapsed_ms;
                total_ms += elapsed_ms;
                iterations += 1;
                emit(
                    iterations,
                    latest_ms,
                    total_ms,
                    total_compute_ms,
                    compute_count,
                    false,
                    false,
                );
            }
        }
        RepeatMode::Gpu => {
            let runner = GpuRunner::new(adapter.index)?;
            while Instant::now() < deadline && !cancel.load(Ordering::Relaxed) {
                check_canceled(Some(&cancel))?;
                let timing = match runner.multiply_cancelable(
                    size,
                    &a,
                    &b,
                    true,
                    gpu_intensity,
                    Some(&cancel),
                    None,
                ) {
                    Ok(timing) => timing,
                    Err(_) if cancel.load(Ordering::Relaxed) => break,
                    Err(err) => return Err(err),
                };
                latest_ms = timing.total_ms;
                total_ms += timing.total_ms;
                if let Some(compute_ms) = timing.compute_ms {
                    total_compute_ms += compute_ms;
                    compute_count += 1;
                }
                iterations += 1;
                emit(
                    iterations,
                    latest_ms,
                    total_ms,
                    total_compute_ms,
                    compute_count,
                    false,
                    false,
                );
            }
        }
    }

    Ok(emit(
        iterations,
        latest_ms,
        total_ms,
        total_compute_ms,
        compute_count,
        cancel.load(Ordering::Relaxed),
        true,
    ))
}

struct BenchScopeApp {
    view: AppView,
    adapters: Vec<AdapterInfo>,
    cpu_info: CpuInfo,
    selected_adapter: usize,
    size_text: String,
    gpu_intensity: GpuIntensity,
    validate_output: bool,
    estimate_cpu_time: bool,
    repeat_mode: RepeatMode,
    repeat_duration: RepeatDuration,
    results: Vec<BenchmarkResult>,
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
    drive: DriveBenchmarkState,
    sensors: SensorManager,
    temperature_run: Option<TemperatureRunTracker>,
    fullscreen: bool,
}

impl BenchScopeApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_ui_style(&cc.egui_ctx);
        let (tx, rx) = mpsc::channel();
        let adapters = enumerate_adapters();
        let cpu_info = detect_cpu_info();
        let selected_adapter = adapters
            .iter()
            .position(|adapter| adapter.device_type != wgpu::DeviceType::Cpu)
            .unwrap_or(0);
        let drive = DriveBenchmarkState::new();
        let sensors = SensorManager::new(drive_letter_for_path(&PathBuf::from(
            drive.target_folder_text.trim(),
        )));
        let mut app = Self {
            view: AppView::MainMenu,
            adapters,
            cpu_info,
            selected_adapter,
            size_text: DEFAULT_SIZES[6].to_string(),
            gpu_intensity: GpuIntensity::Safe,
            validate_output: true,
            estimate_cpu_time: false,
            repeat_mode: RepeatMode::Gpu,
            repeat_duration: RepeatDuration::OneMinute,
            results: Vec::new(),
            log: Vec::new(),
            status: "Ready".to_owned(),
            progress: 0.0,
            cpu_progress: 0.0,
            gpu_progress: 0.0,
            eta_text: String::new(),
            rx,
            tx,
            cancel: None,
            running: false,
            repeat_running: false,
            pending_vram_warning: None,
            matrix_back_confirm: false,
            stress_back_confirm: false,
            drive_back_confirm: false,
            drive,
            sensors,
            temperature_run: None,
            fullscreen: false,
        };
        app.log("Application started");
        if app.adapters.is_empty() {
            app.status = "No wgpu adapters found".to_owned();
            app.log("No wgpu adapters found");
        } else {
            app.log(format!("CPU: {}", app.cpu_info.label()));
            app.log(format!("Found {} adapter(s)", app.adapters.len()));
            for adapter in app.adapters.clone() {
                app.log(format!(
                    "{} | vendor {:04X} device {:04X} | driver {} | timestamp {}",
                    adapter.label(),
                    adapter.vendor,
                    adapter.device,
                    empty_to_unknown(&adapter.driver),
                    if adapter.timestamp_query { "yes" } else { "no" }
                ));
                if let Some((limit, label)) = adapter_memory_limit_bytes(&adapter) {
                    app.log(format!(
                        "  memory limit estimate: {} ({label})",
                        format_bytes(limit)
                    ));
                }
            }
        }
        app
    }

    fn log(&mut self, message: impl Into<String>) {
        self.log.push(message.into());
    }

    fn begin_temperature_run(&mut self, scope: TemperatureScope) {
        let snapshot = self.sensors.latest();
        self.temperature_run = Some(TemperatureRunTracker::start(scope, &snapshot));
    }

    fn observe_temperature_run(&mut self) {
        let snapshot = self.sensors.latest();
        if let Some(run) = &mut self.temperature_run {
            run.observe(&snapshot);
        }
    }

    fn finish_temperature_run(&mut self) -> Option<TemperatureRunReport> {
        let snapshot = self.sensors.latest();
        self.temperature_run.take().map(|run| run.finish(&snapshot))
    }

    fn finish_and_log_temperature_run(&mut self) -> Option<TemperatureRunReport> {
        let report = self.finish_temperature_run()?;
        let summary = format_temperature_run_report(&report);
        self.log(format!("Temperature: {summary}"));
        Some(report)
    }

    fn sync_sensor_state(&mut self) {
        self.sensors
            .set_target_drive_letter(drive_letter_for_path(&PathBuf::from(
                self.drive.target_folder_text.trim(),
            )));
        let gpu_uses_shared_cpu_temperature = self
            .adapters
            .get(self.selected_adapter)
            .is_some_and(adapter_uses_shared_cpu_temperature);
        self.sensors
            .set_target_gpu_uses_shared_cpu_temperature(gpu_uses_shared_cpu_temperature);
    }

    fn ui_sensor_panel(&mut self, ui: &mut egui::Ui) {
        let snapshot = self.sensors.latest();
        let rows: Vec<(&str, Option<&SensorReading>)> = match self.view {
            AppView::MatrixBenchmark | AppView::MatrixStressTest => {
                vec![
                    ("CPU", snapshot.cpu.as_ref()),
                    ("GPU", snapshot.gpu.as_ref()),
                ]
            }
            AppView::DriveBenchmark => vec![("SSD", snapshot.drive.as_ref())],
            AppView::MainMenu => return,
        };

        ui.with_layout(egui::Layout::right_to_left(egui::Align::BOTTOM), |ui| {
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_min_width(210.0);
                    ui.label(egui::RichText::new("Sensors").strong());
                    ui.horizontal(|ui| {
                        ui.set_min_width(188.0);
                        ui.label(egui::RichText::new("").monospace());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new("Util %").small());
                            ui.add_space(12.0);
                            ui.label(egui::RichText::new("Temp").small());
                        });
                    });
                    ui.add_space(3.0);
                    for (label, reading) in rows {
                        ui_sensor_row(ui, label, reading);
                    }
                });
        });
    }

    fn selected_size(&self) -> Result<usize> {
        let size = self
            .size_text
            .trim()
            .parse::<usize>()
            .context("matrix size must be an integer")?;
        if size == 0 {
            return Err(anyhow!("matrix size must be positive"));
        }
        if size > 16384 {
            return Err(anyhow!("matrix size is capped at 16384 for this version"));
        }
        Ok(size)
    }

    fn selected_adapter(&self) -> Result<AdapterInfo> {
        self.adapters
            .get(self.selected_adapter)
            .cloned()
            .ok_or_else(|| anyhow!("no GPU adapter selected"))
    }

    fn start_single(&mut self) {
        self.start_single_checked(false);
    }

    fn start_single_checked(&mut self, ignore_vram_warning: bool) {
        let size = match self.selected_size() {
            Ok(size) => size,
            Err(err) => {
                self.status = err.to_string();
                self.log(err.to_string());
                return;
            }
        };
        let adapter = match self.selected_adapter() {
            Ok(adapter) => adapter,
            Err(err) => {
                self.status = err.to_string();
                self.log(err.to_string());
                return;
            }
        };

        if !ignore_vram_warning {
            if let Some(warning) = self.vram_warning_for(
                RunAction::Single,
                size,
                adapter.clone(),
                self.gpu_intensity,
                self.validate_output,
                self.estimate_cpu_time,
                self.repeat_mode,
                self.repeat_duration,
            ) {
                self.status = "VRAM warning: confirm before running this benchmark".to_owned();
                self.pending_vram_warning = Some(warning);
                return;
            }
        }

        self.launch_single(
            size,
            adapter,
            self.gpu_intensity,
            self.validate_output,
            self.estimate_cpu_time,
        );
    }

    fn launch_single(
        &mut self,
        size: usize,
        adapter: AdapterInfo,
        gpu_intensity: GpuIntensity,
        validate: bool,
        estimate_cpu_time: bool,
    ) {
        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.begin_temperature_run(TemperatureScope::Matrix);
        self.cancel = Some(cancel);
        self.running = true;
        self.progress = 0.0;
        self.cpu_progress = 0.0;
        self.gpu_progress = 0.0;
        self.eta_text = "ETA: estimating".to_owned();
        self.status = format!("Running {size}x{size} benchmark...");
        self.log(format!(
            "Starting benchmark on {} with {} GPU intensity and {} CPU timing",
            adapter.label(),
            gpu_intensity,
            if estimate_cpu_time {
                "estimated"
            } else {
                "exact"
            }
        ));
        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                run_single_cancelable(
                    size,
                    adapter,
                    validate,
                    estimate_cpu_time,
                    gpu_intensity,
                    &worker_cancel,
                    Some(tx.clone()),
                )
            }))
            .map_err(|panic| format!("Benchmark panicked: {}", panic_message(&*panic)))
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(WorkerEvent::SingleDone(result));
        });
    }

    fn start_repeat(&mut self) {
        self.start_repeat_checked(false);
    }

    fn start_repeat_checked(&mut self, ignore_vram_warning: bool) {
        let size = match self.selected_size() {
            Ok(size) => size,
            Err(err) => {
                self.status = err.to_string();
                self.log(err.to_string());
                return;
            }
        };
        let adapter = match self.selected_adapter() {
            Ok(adapter) => adapter,
            Err(err) => {
                self.status = err.to_string();
                self.log(err.to_string());
                return;
            }
        };

        if !ignore_vram_warning {
            if let Some(warning) = self.vram_warning_for(
                RunAction::Repeat,
                size,
                adapter.clone(),
                self.gpu_intensity,
                self.validate_output,
                self.estimate_cpu_time,
                self.repeat_mode,
                self.repeat_duration,
            ) {
                self.status = "VRAM warning: confirm before starting the stress test".to_owned();
                self.pending_vram_warning = Some(warning);
                return;
            }
        }

        self.launch_repeat(
            size,
            adapter,
            self.gpu_intensity,
            self.repeat_mode,
            self.repeat_duration,
        );
    }

    fn launch_repeat(
        &mut self,
        size: usize,
        adapter: AdapterInfo,
        gpu_intensity: GpuIntensity,
        mode: RepeatMode,
        duration: RepeatDuration,
    ) {
        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.begin_temperature_run(TemperatureScope::Matrix);
        self.cancel = Some(cancel);
        self.running = true;
        self.repeat_running = true;
        self.progress = 0.0;
        self.status = format!("Running {mode} stress test for {duration}...");
        self.log(format!(
            "Starting {mode} {duration} stress test at {size}x{size} on {} with {} GPU intensity",
            adapter.label(),
            gpu_intensity
        ));
        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                run_repeat(
                    size,
                    adapter,
                    mode,
                    gpu_intensity,
                    worker_cancel,
                    tx.clone(),
                    duration.duration(),
                )
            }))
            .map_err(|panic| format!("Repeat test panicked: {}", panic_message(&*panic)))
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(WorkerEvent::RepeatDone(result));
        });
    }

    fn vram_warning_for(
        &self,
        action: RunAction,
        size: usize,
        adapter: AdapterInfo,
        gpu_intensity: GpuIntensity,
        validate_output: bool,
        estimate_cpu_time: bool,
        repeat_mode: RepeatMode,
        repeat_duration: RepeatDuration,
    ) -> Option<PendingVramWarning> {
        if action == RunAction::Repeat && repeat_mode == RepeatMode::Cpu {
            return None;
        }
        let estimated_gpu_bytes = gpu_working_set_bytes(size)?;
        let (limit_bytes, limit_label) = adapter_memory_limit_bytes(&adapter)?;
        (estimated_gpu_bytes > limit_bytes).then(|| PendingVramWarning {
            action,
            size,
            adapter,
            gpu_intensity,
            validate_output,
            estimate_cpu_time,
            repeat_mode,
            repeat_duration,
            estimated_gpu_bytes,
            limit_bytes,
            limit_label: limit_label.to_owned(),
        })
    }

    fn continue_pending_vram_warning(&mut self) {
        let Some(warning) = self.pending_vram_warning.take() else {
            return;
        };
        self.log(format!(
            "User chose to run {}x{} despite estimated GPU memory {} exceeding {} ({})",
            warning.size,
            warning.size,
            format_bytes(warning.estimated_gpu_bytes),
            warning.limit_label,
            format_bytes(warning.limit_bytes)
        ));
        match warning.action {
            RunAction::Single => self.launch_single(
                warning.size,
                warning.adapter,
                warning.gpu_intensity,
                warning.validate_output,
                warning.estimate_cpu_time,
            ),
            RunAction::Repeat => self.launch_repeat(
                warning.size,
                warning.adapter,
                warning.gpu_intensity,
                warning.repeat_mode,
                warning.repeat_duration,
            ),
        }
    }

    fn cancel_single(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
            self.status = "Cancel requested; stopping benchmark...".to_owned();
            self.log("Cancel requested for single benchmark");
        }
    }

    fn cancel_repeat(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
            self.status = "Cancel requested; stopping stress test...".to_owned();
            self.log("Cancel requested");
        }
    }

    fn poll_worker_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                WorkerEvent::SingleProgress(progress) => {
                    self.cpu_progress = progress.cpu_progress;
                    self.gpu_progress = progress.gpu_progress;
                    self.progress =
                        ((progress.cpu_progress + progress.gpu_progress) / 2.0).clamp(0.0, 1.0);
                    self.eta_text = format_eta(progress.eta_s);
                    self.status = format!(
                        "{} - elapsed {}",
                        progress.phase,
                        format_elapsed(progress.elapsed_s)
                    );
                }
                WorkerEvent::SingleDone(result) => {
                    self.running = false;
                    self.repeat_running = false;
                    self.cancel = None;
                    match result {
                        Ok(mut result) => {
                            self.progress = 1.0;
                            self.cpu_progress = 1.0;
                            self.gpu_progress = 1.0;
                            self.eta_text = "ETA: complete".to_owned();
                            self.status = "Benchmark complete".to_owned();
                            if let Some(report) = self.finish_and_log_temperature_run() {
                                result.cpu_temperature = report.cpu;
                                result.gpu_temperature = report.gpu;
                            }
                            self.log(format!(
                                "Benchmark complete: CPU {} ms ({}, {}), GPU total {} ms, GPU compute {} ms, path {}, dispatches {}, max dispatch {} ms",
                                format_cpu_ms(&result),
                                if result.cpu_estimated {
                                    "estimated"
                                } else {
                                    "exact"
                                },
                                result.cpu_model,
                                format_ms(Some(result.gpu_total_ms)),
                                format_ms(result.gpu_compute_ms),
                                result.gpu_path,
                                result.dispatch_count,
                                format_ms(result.max_dispatch_ms)
                            ));
                            self.results.push(result);
                        }
                        Err(err) => {
                            let _ = self.finish_and_log_temperature_run();
                            if err.to_ascii_lowercase().contains("canceled") {
                                self.progress = 0.0;
                                self.eta_text = "ETA: canceled".to_owned();
                            } else {
                                self.progress = 1.0;
                                self.eta_text.clear();
                            }
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
                WorkerEvent::RepeatProgress(progress) => {
                    self.progress =
                        (progress.elapsed_s / progress.duration_s).clamp(0.0, 1.0) as f32;
                    self.eta_text =
                        format_eta(Some((progress.duration_s - progress.elapsed_s).max(0.0)));
                    self.status = format!(
                        "{} stress: {:.1}s, {} iteration(s), latest {} ms, avg {} ms, compute avg {} ms",
                        progress.mode,
                        progress.elapsed_s,
                        progress.iterations,
                        format_ms(Some(progress.latest_ms)),
                        format_ms(Some(progress.average_total_ms)),
                        format_ms(progress.average_compute_ms)
                    );
                }
                WorkerEvent::RepeatDone(result) => {
                    self.running = false;
                    self.repeat_running = false;
                    self.cancel = None;
                    self.eta_text.clear();
                    match result {
                        Ok(progress) => {
                            let _ = self.finish_and_log_temperature_run();
                            if !progress.canceled {
                                self.progress = 1.0;
                            }
                            let state = if progress.canceled {
                                "canceled"
                            } else {
                                "complete"
                            };
                            self.status = format!(
                                "Stress test {state}: {} iteration(s), avg {} ms",
                                progress.iterations,
                                format_ms(Some(progress.average_total_ms))
                            );
                            self.log(format!(
                                "Stress test {state}: mode {}, size {}, iterations {}, avg {} ms, compute avg {} ms",
                                progress.mode,
                                progress.size,
                                progress.iterations,
                                format_ms(Some(progress.average_total_ms)),
                                format_ms(progress.average_compute_ms)
                            ));
                        }
                        Err(err) => {
                            let _ = self.finish_and_log_temperature_run();
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
            }
        }
    }

    fn ui_main_menu(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                self.ui_fullscreen_button(ui);
            });
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.heading(egui::RichText::new("BenchScope").size(34.0));
                ui.add_space(24.0);
                if ui
                    .add_sized(
                        [360.0, 58.0],
                        egui::Button::new(
                            egui::RichText::new("Matrix CPU/GPU Benchmark").size(19.0),
                        ),
                    )
                    .clicked()
                {
                    self.view = AppView::MatrixBenchmark;
                }
                ui.add_space(10.0);
                if ui
                    .add_sized(
                        [360.0, 58.0],
                        egui::Button::new(egui::RichText::new("Matrix Stress Test").size(19.0)),
                    )
                    .clicked()
                {
                    self.view = AppView::MatrixStressTest;
                }
                ui.add_space(10.0);
                if ui
                    .add_sized(
                        [360.0, 58.0],
                        egui::Button::new(egui::RichText::new("Drive Benchmark").size(19.0)),
                    )
                    .clicked()
                {
                    self.view = AppView::DriveBenchmark;
                }
                ui.add_space(22.0);
                ui.small(format!("CPU: {}", self.cpu_info.label()));
                ui.small(format!("GPU adapters detected: {}", self.adapters.len()));
            });
        });
    }

    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.fullscreen = !self.fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
    }

    fn handle_global_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F11)) {
            self.toggle_fullscreen(ctx);
        }
    }

    fn ui_fullscreen_button(&mut self, ui: &mut egui::Ui) {
        let label = if self.fullscreen {
            "Exit fullscreen"
        } else {
            "Fullscreen"
        };
        let response = ui
            .add_sized(
                [156.0, 38.0],
                egui::Button::new(egui::RichText::new(label).size(17.0)),
            )
            .on_hover_text("Toggle fullscreen (F11)");
        if response.clicked() {
            self.toggle_fullscreen(ui.ctx());
        }
    }

    fn request_matrix_back_to_menu(&mut self) {
        if self.running {
            self.matrix_back_confirm = true;
        } else {
            self.view = AppView::MainMenu;
        }
    }

    fn request_stress_back_to_menu(&mut self) {
        if self.repeat_running {
            self.stress_back_confirm = true;
        } else {
            self.view = AppView::MainMenu;
        }
    }

    fn request_drive_back_to_menu(&mut self) {
        if self.drive.running {
            self.drive_back_confirm = true;
        } else {
            self.view = AppView::MainMenu;
        }
    }

    fn ui_drive_benchmark(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        egui::Panel::top("drive_top_panel").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui_large_back_button(ui).clicked() {
                    self.request_drive_back_to_menu();
                }
                self.ui_fullscreen_button(ui);
                ui.separator();
                ui.heading("Drive Benchmark");
                ui.separator();
                ui.label(&self.drive.status);
                if !self.drive.eta_text.is_empty() {
                    ui.separator();
                    ui.label(&self.drive.eta_text);
                }
            });
            ui.horizontal(|ui| {
                ui.label("Current");
                ui.add(
                    egui::ProgressBar::new(self.drive.current_progress)
                        .show_percentage()
                        .desired_width(260.0),
                );
                ui.label("Suite");
                ui.add(
                    egui::ProgressBar::new(self.drive.suite_progress)
                        .show_percentage()
                        .desired_width(260.0),
                );
            });
        });

        egui::Panel::left("drive_controls")
            .resizable(false)
            .min_size(350.0)
            .show_inside(ui, |ui| {
                let settings_height = (ui.available_height() - CONTROLS_ACTION_HEIGHT).max(120.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), settings_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("drive_controls_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.heading("Controls");
                                ui.add_space(8.0);

                                self.drive.sync_selected_drive_to_target();
                                ui.label("Target drive");
                                ui.add_enabled_ui(!self.drive.running, |ui| {
                                    let mut selected_drive = self.drive.selected_drive;
                                    egui::ComboBox::from_id_salt("drive_picker_combo")
                                        .selected_text(self.drive.selected_drive_label())
                                        .width(320.0)
                                        .show_ui(ui, |ui| {
                                            for (index, drive) in self.drive.drives.iter().enumerate() {
                                                ui.selectable_value(&mut selected_drive, index, &drive.label);
                                            }
                                        });
                                    if selected_drive != self.drive.selected_drive {
                                        self.drive.select_drive(selected_drive);
                                    }
                                    if ui.button("Refresh drives").clicked() {
                                        self.drive.refresh_drives();
                                    }
                                });
                                if let Some(name) = self.drive.selected_drive_device_name() {
                                    ui.small(format!("Device: {name}"));
                                }
                                ui.small("Selecting a drive sets the target folder to that drive root.");

                ui.label("Target folder");
                ui.add_enabled_ui(!self.drive.running, |ui| {
                    ui.text_edit_singleline(&mut self.drive.target_folder_text);
                });
                let target_path = PathBuf::from(self.drive.target_folder_text.trim());
                if target_path.is_dir() {
                    ui.small(format!("Benchmark file: {}", DRIVE_BENCHMARK_FILE_NAME));
                } else {
                    ui.colored_label(egui::Color32::YELLOW, "Target folder is not valid.");
                }
                ui.small("If the drive root is protected, enter a writable folder on that drive.");

                ui.add_space(8.0);
                ui.label("Profile");
                ui.add_enabled_ui(!self.drive.running, |ui| {
                    ui.horizontal(|ui| {
                        for profile in DriveProfile::ALL {
                            ui.selectable_value(&mut self.drive.profile, profile, profile.label());
                        }
                    });
                });
                ui.small(format!(
                    "Measured target: {} per test, hard cap: {:.0}s",
                    format_elapsed(self.drive.profile.target_duration().as_secs_f64()),
                    DRIVE_MAX_TEST_SECONDS
                ));

                ui.add_space(8.0);
                ui.label("Test file size");
                ui.add_enabled_ui(!self.drive.running, |ui| {
                    egui::ComboBox::from_id_salt("drive_file_size_combo")
                        .selected_text(self.drive.file_size.label())
                        .show_ui(ui, |ui| {
                            for size in DriveFileSize::ALL {
                                ui.selectable_value(&mut self.drive.file_size, size, size.label());
                            }
                        });
                });
                ui.small(format!(
                    "Planned file size: {}",
                    format_bytes(self.drive.planned_file_size())
                ));

                ui.separator();
                ui.label("Tests");
                ui.add_enabled_ui(!self.drive.running, |ui| {
                    ui.checkbox(&mut self.drive.run_seq_read, "Sequential read");
                    ui.checkbox(&mut self.drive.run_seq_write, "Sequential write");
                    ui.checkbox(&mut self.drive.run_random_read, "Random 4 KiB read");
                    ui.checkbox(&mut self.drive.run_random_write, "Random 4 KiB write");
                });
                ui.small("Mode: direct I/O preferred; cached fallback is labeled in results.");
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Write tests create temporary data on the selected drive.",
                );
                let planned_writes = self.drive.planned_write_bytes();
                if planned_writes >= 4 * 1024 * 1024 * 1024 {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        format!(
                            "Selected write tests may write at least {}.",
                            format_bytes(planned_writes)
                        ),
                    );
                }

                            });
                    },
                );
                ui.separator();
                ui.add_enabled_ui(!self.drive.running, |ui| {
                    if ui.button("Run drive benchmark").clicked() {
                        let was_running = self.drive.running;
                        self.drive.start();
                        if !was_running && self.drive.running {
                            self.begin_temperature_run(TemperatureScope::Drive);
                        }
                    }
                });
                ui.add_enabled_ui(self.drive.running, |ui| {
                    if ui.button("Cancel drive benchmark").clicked() {
                        self.drive.cancel();
                    }
                });
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            let available_height = ui.available_height();
            let (results_height, log_height) =
                panel_content_log_heights(available_height, 0.18, 150.0);

            ui.heading("Drive Results");
            ui.add_space(6.0);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), results_height),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            egui::Grid::new("drive_results_grid")
                                .striped(true)
                                .num_columns(10)
                                .show(ui, |ui| {
                                    result_header(ui, "Test");
                                    result_header(ui, "Speed MB/s");
                                    result_header(ui, "IOPS");
                                    result_header(ui, "Avg latency");
                                    result_header(ui, "P95 latency");
                                    result_header(ui, "Duration");
                                    result_header(ui, "File size");
                                    result_header(ui, "Mode");
                                    result_header(ui, "SSD temp");
                                    result_header(ui, "Notes");
                                    ui.end_row();

                                    for result in &self.drive.results {
                                        result_cell(ui, result.test.label());
                                        result_cell(ui, format_drive_speed(result));
                                        result_cell(ui, format_optional_iops(result.iops));
                                        result_cell(
                                            ui,
                                            format_optional_latency(result.avg_latency_ms),
                                        );
                                        result_cell(
                                            ui,
                                            format_optional_latency(result.p95_latency_ms),
                                        );
                                        result_cell(ui, format_ms(Some(result.duration_ms)));
                                        result_cell(ui, format_bytes(result.file_size_bytes));
                                        result_cell(ui, result.io_mode.label());
                                        result_cell(
                                            ui,
                                            format_temperature_summary(&result.ssd_temperature),
                                        );
                                        result_cell(
                                            ui,
                                            if result.notes.is_empty() {
                                                String::new()
                                            } else {
                                                result.notes.join(", ")
                                            },
                                        );
                                        ui.end_row();
                                    }
                                });
                        });
                },
            );

            ui.separator();
            ui.heading("Log");
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .max_height(log_height)
                .show(ui, |ui| {
                    for line in &self.drive.log {
                        ui_log_line(ui, line);
                    }
                });
            ui.add_space(6.0);
            self.ui_sensor_panel(ui);
        });

        if self.drive_back_confirm {
            egui::Window::new("Return to main menu?")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label("A drive benchmark is currently running.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Stay").clicked() {
                            self.drive_back_confirm = false;
                        }
                        if ui.button("Cancel and return").clicked() {
                            self.drive.cancel();
                            self.drive_back_confirm = false;
                            self.view = AppView::MainMenu;
                        }
                    });
                });
        }
    }

    fn ui_matrix_stress_test(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        egui::Panel::top("stress_top_panel").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui_large_back_button(ui).clicked() {
                    self.request_stress_back_to_menu();
                }
                self.ui_fullscreen_button(ui);
                ui.separator();
                ui.heading("Matrix Stress Test");
                ui.separator();
                ui.label(&self.status);
                if !self.eta_text.is_empty() {
                    ui.separator();
                    ui.label(&self.eta_text);
                }
            });
            ui.add(
                egui::ProgressBar::new(self.progress)
                    .show_percentage()
                    .text("Stress test elapsed"),
            );
        });

        egui::Panel::left("stress_controls")
            .resizable(false)
            .min_size(330.0)
            .show_inside(ui, |ui| {
                let settings_height = (ui.available_height() - CONTROLS_ACTION_HEIGHT).max(120.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), settings_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("stress_controls_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.heading("Stress Controls");
                                ui.add_space(8.0);

                ui.small(format!("CPU: {}", self.cpu_info.label()));
                ui.add_space(4.0);

                ui.label("GPU adapter");
                egui::ComboBox::from_id_salt("stress_adapter_combo")
                    .selected_text(
                        self.adapters
                            .get(self.selected_adapter)
                            .map(AdapterInfo::label)
                            .unwrap_or_else(|| "No adapters found".to_owned()),
                    )
                    .show_ui(ui, |ui| {
                        for (index, adapter) in self.adapters.iter().enumerate() {
                            ui.selectable_value(&mut self.selected_adapter, index, adapter.label());
                        }
                    });

                if let Some(adapter) = self.adapters.get(self.selected_adapter) {
                    ui.small(format!(
                        "Vendor {:04X}, device {:04X}, driver {}, timestamp queries {}",
                        adapter.vendor,
                        adapter.device,
                        empty_to_unknown(&adapter.driver),
                        if adapter.timestamp_query {
                            "supported"
                        } else {
                            "unavailable"
                        }
                    ));
                    if let Some((limit, label)) = adapter_memory_limit_bytes(adapter) {
                        ui.small(format!("Memory limit estimate: {} ({label})", format_bytes(limit)));
                    } else {
                        ui.small("Memory limit estimate: unavailable for this adapter/backend");
                    }
                }

                ui.add_space(6.0);
                ui.label("GPU intensity");
                ui.add_enabled_ui(!self.running, |ui| {
                    ui.horizontal(|ui| {
                        for intensity in GpuIntensity::ALL {
                            ui.selectable_value(&mut self.gpu_intensity, intensity, intensity.label());
                        }
                    });
                });
                ui.small(self.gpu_intensity.description());
                if self.gpu_intensity == GpuIntensity::High {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "High mode can stress the driver, PSU, and thermals during large matrices.",
                    );
                }

                if ui.button("Refresh GPUs").clicked() && !self.running {
                    self.adapters = enumerate_adapters();
                    self.selected_adapter = 0;
                    self.status = format!("Found {} adapter(s)", self.adapters.len());
                    self.log(self.status.clone());
                }

                ui.separator();
                ui.label("Matrix size");
                egui::ComboBox::from_id_salt("stress_size_combo")
                    .selected_text(self.size_text.clone())
                    .show_ui(ui, |ui| {
                        for size in DEFAULT_SIZES {
                            ui.selectable_value(&mut self.size_text, size.to_string(), size.to_string());
                        }
                    });
                ui.text_edit_singleline(&mut self.size_text);

                if let Ok(size) = self.selected_size() {
                    if let (Some(matrix_bytes), Some(gpu_bytes)) =
                        (matrix_buffers_bytes(size, 3), gpu_working_set_bytes(size))
                    {
                        ui.small(format!(
                            "A/B/C: {}; GPU run estimate: {}",
                            format_bytes(matrix_bytes),
                            format_bytes(gpu_bytes)
                        ));

                        if let Some(adapter) = self.adapters.get(self.selected_adapter) {
                            if let Some((limit, label)) = adapter_memory_limit_bytes(adapter) {
                                if gpu_bytes > limit {
                                    ui.colored_label(
                                        egui::Color32::RED,
                                        format!(
                                            "Estimated GPU memory exceeds {label}: {} > {}.",
                                            format_bytes(gpu_bytes),
                                            format_bytes(limit)
                                        ),
                                    );
                                }
                            }
                        }
                    }
                    if size >= 8192 {
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            "Large stress tests can heavily load the selected processor for the full duration.",
                        );
                    }
                }

                ui.separator();
                ui.label("Processor");
                ui.add_enabled_ui(!self.running, |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.repeat_mode, RepeatMode::Gpu, "GPU");
                        ui.selectable_value(&mut self.repeat_mode, RepeatMode::Cpu, "CPU");
                    });
                });
                ui.label("Duration");
                ui.add_enabled_ui(!self.running, |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut self.repeat_duration,
                            RepeatDuration::OneMinute,
                            "1 min",
                        );
                        ui.selectable_value(
                            &mut self.repeat_duration,
                            RepeatDuration::FiveMinutes,
                            "5 min",
                        );
                    });
                });
                            });
                    },
                );
                ui.separator();

                ui.add_enabled_ui(!self.running && !self.adapters.is_empty(), |ui| {
                    if ui.button("Start stress test").clicked() {
                        self.start_repeat();
                    }
                });
                ui.add_enabled_ui(self.repeat_running, |ui| {
                    if ui.button("Cancel stress test").clicked() {
                        self.cancel_repeat();
                    }
                });
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            let available_height = ui.available_height();
            let (summary_height, log_height) =
                panel_content_log_heights(available_height, 0.35, 220.0);

            ui.heading("Stress Test");
            ui.add_space(6.0);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), summary_height),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.label(format!("Mode: {}", self.repeat_mode));
                    ui.label(format!("Duration: {}", self.repeat_duration));
                    ui.label(format!("Progress: {:.0}%", self.progress * 100.0));
                    if !self.eta_text.is_empty() {
                        ui.label(&self.eta_text);
                    }
                    ui.label(&self.status);
                },
            );

            ui.separator();
            ui.heading("Log");
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .max_height(log_height)
                .show(ui, |ui| {
                    for line in &self.log {
                        ui_log_line(ui, line);
                    }
                });
            ui.add_space(6.0);
            self.ui_sensor_panel(ui);
        });

        if let Some(warning) = self.pending_vram_warning.clone() {
            egui::Window::new("VRAM limit exceeded")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label(format!(
                        "{}x{} is estimated to need {} of GPU memory.",
                        warning.size,
                        warning.size,
                        format_bytes(warning.estimated_gpu_bytes)
                    ));
                    ui.label(format!(
                        "The selected adapter's {} is {}.",
                        warning.limit_label,
                        format_bytes(warning.limit_bytes)
                    ));
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "Running anyway may fail, trigger driver paging, or make the result misleading.",
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.pending_vram_warning = None;
                            self.status = "Run canceled before exceeding the VRAM estimate".to_owned();
                            self.log("Canceled run after VRAM warning");
                        }
                        if ui.button("Run anyway").clicked() {
                            self.continue_pending_vram_warning();
                        }
                    });
                });
        }

        if self.stress_back_confirm {
            egui::Window::new("Return to main menu?")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label("A matrix stress test is currently running.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Stay").clicked() {
                            self.stress_back_confirm = false;
                        }
                        if ui.button("Cancel and return").clicked() {
                            self.cancel_repeat();
                            self.stress_back_confirm = false;
                            self.view = AppView::MainMenu;
                        }
                    });
                });
        }
    }
}

impl eframe::App for BenchScopeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_global_shortcuts(&ctx);
        self.sync_sensor_state();
        let drive_was_running = self.drive.running;
        let drive_result_count_before = self.drive.results.len();
        self.poll_worker_events();
        self.drive.poll_worker_events();
        self.observe_temperature_run();
        if drive_was_running && !self.drive.running {
            if let Some(report) = self.finish_and_log_temperature_run() {
                for result in self
                    .drive
                    .results
                    .iter_mut()
                    .skip(drive_result_count_before)
                {
                    result.ssd_temperature = report.drive;
                }
            }
        }
        self.sync_sensor_state();
        if self.running || self.drive.running {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else if self.view != AppView::MainMenu {
            ctx.request_repaint_after(Duration::from_millis(SENSOR_POLL_MS));
        }

        match self.view {
            AppView::MainMenu => {
                self.ui_main_menu(ui);
                return;
            }
            AppView::DriveBenchmark => {
                self.ui_drive_benchmark(ui);
                return;
            }
            AppView::MatrixStressTest => {
                self.ui_matrix_stress_test(ui);
                return;
            }
            AppView::MatrixBenchmark => {}
        }

        egui::Panel::top("top_panel").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui_large_back_button(ui).clicked() {
                    self.request_matrix_back_to_menu();
                }
                self.ui_fullscreen_button(ui);
                ui.separator();
                ui.heading("BenchScope");
                ui.separator();
                ui.label(&self.status);
                if !self.eta_text.is_empty() {
                    ui.separator();
                    ui.label(&self.eta_text);
                }
            });
            ui.horizontal(|ui| {
                ui.label("CPU");
                ui.add(
                    egui::ProgressBar::new(self.cpu_progress)
                        .show_percentage()
                        .desired_width(260.0),
                );
                ui.label("GPU");
                ui.add(
                    egui::ProgressBar::new(self.gpu_progress)
                        .show_percentage()
                        .desired_width(260.0),
                );
            });
        });

        egui::Panel::left("controls")
            .resizable(false)
            .min_size(330.0)
            .show_inside(ui, |ui| {
                let settings_height = (ui.available_height() - CONTROLS_ACTION_HEIGHT).max(120.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), settings_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("matrix_controls_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.heading("Controls");
                                ui.add_space(8.0);

                ui.small(format!("CPU: {}", self.cpu_info.label()));
                ui.add_space(4.0);

                ui.label("GPU adapter");
                egui::ComboBox::from_id_salt("adapter_combo")
                    .selected_text(
                        self.adapters
                            .get(self.selected_adapter)
                            .map(AdapterInfo::label)
                            .unwrap_or_else(|| "No adapters found".to_owned()),
                    )
                    .show_ui(ui, |ui| {
                        for (index, adapter) in self.adapters.iter().enumerate() {
                            ui.selectable_value(&mut self.selected_adapter, index, adapter.label());
                        }
                    });

                if let Some(adapter) = self.adapters.get(self.selected_adapter) {
                    ui.small(format!(
                        "Vendor {:04X}, device {:04X}, driver {}, timestamp queries {}",
                        adapter.vendor,
                        adapter.device,
                        empty_to_unknown(&adapter.driver),
                        if adapter.timestamp_query {
                            "supported"
                        } else {
                            "unavailable"
                        }
                    ));
                    if let Some((limit, label)) = adapter_memory_limit_bytes(adapter) {
                        ui.small(format!("Memory limit estimate: {} ({label})", format_bytes(limit)));
                    } else {
                        ui.small("Memory limit estimate: unavailable for this adapter/backend");
                    }
                    ui.small(format!(
                        "Reported memory: VRAM {}, dedicated system {}, shared {}",
                        format_optional_bytes(adapter.dedicated_vram_bytes),
                        format_optional_bytes(adapter.dedicated_system_memory_bytes),
                        format_optional_bytes(adapter.shared_system_memory_bytes)
                    ));
                }

                ui.add_space(6.0);
                ui.label("GPU intensity");
                ui.add_enabled_ui(!self.running, |ui| {
                    ui.horizontal(|ui| {
                        for intensity in GpuIntensity::ALL {
                            ui.selectable_value(&mut self.gpu_intensity, intensity, intensity.label());
                        }
                    });
                });
                ui.small(self.gpu_intensity.description());
                if self.gpu_intensity == GpuIntensity::High {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "High mode can stress the driver, PSU, and thermals during large matrices.",
                    );
                }

                if ui.button("Refresh GPUs").clicked() && !self.running {
                    self.adapters = enumerate_adapters();
                    self.selected_adapter = 0;
                    self.status = format!("Found {} adapter(s)", self.adapters.len());
                    self.log(self.status.clone());
                }

                ui.separator();
                ui.label("Matrix size");
                egui::ComboBox::from_id_salt("size_combo")
                    .selected_text(self.size_text.clone())
                    .show_ui(ui, |ui| {
                        for size in DEFAULT_SIZES {
                            ui.selectable_value(&mut self.size_text, size.to_string(), size.to_string());
                        }
                });
                ui.text_edit_singleline(&mut self.size_text);
                ui.checkbox(&mut self.validate_output, "Validate GPU output");
                ui.checkbox(&mut self.estimate_cpu_time, "Estimate CPU time");

                if let Ok(size) = self.selected_size() {
                    if let (Some(matrix_bytes), Some(gpu_bytes)) =
                        (matrix_buffers_bytes(size, 3), gpu_working_set_bytes(size))
                    {
                        ui.small(format!(
                            "A/B/C: {}; GPU run estimate: {}",
                            format_bytes(matrix_bytes),
                            format_bytes(gpu_bytes)
                        ));

                        if let Some(adapter) = self.adapters.get(self.selected_adapter) {
                            if let Some((limit, label)) = adapter_memory_limit_bytes(adapter) {
                                if gpu_bytes > limit {
                                    ui.colored_label(
                                        egui::Color32::RED,
                                        format!(
                                            "Estimated GPU memory exceeds {label}: {} > {}.",
                                            format_bytes(gpu_bytes),
                                            format_bytes(limit)
                                        ),
                                    );
                                }
                            }
                        }
                    }
                    if size >= 4096 {
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            if self.estimate_cpu_time {
                                "CPU time will be estimated from sampled work on this CPU."
                            } else {
                                "Exact CPU timing can take a very long time at this size."
                            },
                        );
                    }
                    if size >= 8192 {
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            "Large GPU runs are split into smaller submissions in Safe mode to reduce driver timeout risk.",
                        );
                    }
                    if size == 16384 {
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            "16K uses about 3 GB for A/B/C alone before readback and driver overhead.",
                        );
                    }
                }
                            });
                    },
                );
                ui.separator();

                ui.add_enabled_ui(!self.running && !self.adapters.is_empty(), |ui| {
                    if ui.button("Run benchmark").clicked() {
                        self.start_single();
                    }
                });
                ui.add_enabled_ui(self.running && !self.repeat_running, |ui| {
                    if ui.button("Cancel benchmark").clicked() {
                        self.cancel_single();
                    }
                });
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            let available_height = ui.available_height();
            let (results_height, log_height) =
                panel_content_log_heights(available_height, 0.18, 150.0);

            ui.heading("Results");
            ui.add_space(6.0);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), results_height),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            egui::Grid::new("results_grid")
                                .striped(true)
                                .num_columns(18)
                                .show(ui, |ui| {
                                    result_header(ui, "Size");
                                    result_header(ui, "CPU ms");
                                    result_header(ui, "GPU compute ms");
                                    result_header(ui, "GPU total ms");
                                    result_header(ui, "Transfer/sync ms");
                                    result_header(ui, "Speedup");
                                    result_header(ui, "CPU model");
                                    result_header(ui, "Adapter");
                                    result_header(ui, "GPU path");
                                    result_header(ui, "Tile");
                                    result_header(ui, "Dispatches");
                                    result_header(ui, "Last dispatch ms");
                                    result_header(ui, "Avg dispatch ms");
                                    result_header(ui, "Max dispatch ms");
                                    result_header(ui, "Backoffs");
                                    result_header(ui, "CPU temp");
                                    result_header(ui, "GPU temp");
                                    result_header(ui, "Validation");
                                    ui.end_row();

                                    for result in &self.results {
                                        result_cell(ui, format!("{}x{}", result.size, result.size));
                                        result_cell(ui, format_cpu_ms(result));
                                        result_cell(ui, format_ms(result.gpu_compute_ms));
                                        result_cell(ui, format_ms(Some(result.gpu_total_ms)));
                                        result_cell(ui, format_ms(result.transfer_sync_ms));
                                        result_cell(ui, format_speedup(result.speedup));
                                        result_cell(ui, result.cpu_model.as_str());
                                        result_cell(ui, result.adapter.as_str());
                                        result_cell(ui, result.gpu_path.label());
                                        result_cell(ui, result.tile_shape.as_str());
                                        result_cell(ui, result.dispatch_count.to_string());
                                        result_cell(ui, format_ms(result.last_dispatch_ms));
                                        result_cell(ui, format_ms(result.avg_dispatch_ms));
                                        result_cell(ui, format_ms(result.max_dispatch_ms));
                                        result_cell(ui, result.backoff_count.to_string());
                                        result_cell(
                                            ui,
                                            format_temperature_summary(&result.cpu_temperature),
                                        );
                                        result_cell(
                                            ui,
                                            format_temperature_summary(&result.gpu_temperature),
                                        );
                                        result_cell(ui, result.validation.as_str());
                                        ui.end_row();
                                    }
                                });
                        });
                },
            );

            ui.separator();
            ui.heading("Log");
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .max_height(log_height)
                .show(ui, |ui| {
                    for line in &self.log {
                        ui_log_line(ui, line);
                    }
                });
            ui.add_space(6.0);
            self.ui_sensor_panel(ui);
        });

        if let Some(warning) = self.pending_vram_warning.clone() {
            egui::Window::new("VRAM limit exceeded")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label(format!(
                        "{}x{} is estimated to need {} of GPU memory.",
                        warning.size,
                        warning.size,
                        format_bytes(warning.estimated_gpu_bytes)
                    ));
                    ui.label(format!(
                        "The selected adapter's {} is {}.",
                        warning.limit_label,
                        format_bytes(warning.limit_bytes)
                    ));
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "Running anyway may fail, trigger driver paging, or make the result misleading.",
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.pending_vram_warning = None;
                            self.status = "Run canceled before exceeding the VRAM estimate".to_owned();
                            self.log("Canceled run after VRAM warning");
                        }
                        if ui.button("Run anyway").clicked() {
                            self.continue_pending_vram_warning();
                        }
                    });
                });
        }

        if self.matrix_back_confirm {
            egui::Window::new("Return to main menu?")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label("A matrix benchmark is currently running.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Stay").clicked() {
                            self.matrix_back_confirm = false;
                        }
                        if ui.button("Cancel and return").clicked() {
                            if self.repeat_running {
                                self.cancel_repeat();
                            } else {
                                self.cancel_single();
                            }
                            self.matrix_back_confirm = false;
                            self.view = AppView::MainMenu;
                        }
                    });
                });
        }
    }
}

fn auto_drive_file_size(profile: DriveProfile) -> u64 {
    match profile {
        DriveProfile::Quick => 256 * 1024 * 1024,
        DriveProfile::Balanced => 512 * 1024 * 1024,
        DriveProfile::Thorough => 1024 * 1024 * 1024,
    }
}

fn detect_drives() -> Vec<DriveInfo> {
    #[cfg(windows)]
    {
        let device_names = windows_drive_device_names();
        let mut drives = Vec::new();
        for letter in b'A'..=b'Z' {
            let letter = letter as char;
            let root = PathBuf::from(format!("{letter}:\\"));
            if root.is_dir() {
                drives.push(DriveInfo::with_device_name(
                    root,
                    device_names.get(&letter).cloned(),
                ));
            }
        }
        drives
    }

    #[cfg(not(windows))]
    {
        vec![DriveInfo::with_device_name(PathBuf::from("/"), None)]
    }
}

#[cfg(windows)]
fn windows_drive_device_names() -> HashMap<char, String> {
    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
Get-Volume |
    Where-Object { $_.DriveLetter } |
    Sort-Object DriveLetter |
    ForEach-Object {
        $letter = $_.DriveLetter
        $partition = Get-Partition -DriveLetter $letter -ErrorAction SilentlyContinue | Select-Object -First 1
        $disk = $null
        if ($partition) {
            $disk = $partition | Get-Disk -ErrorAction SilentlyContinue
        }
        $name = ''
        if ($disk) {
            $name = $disk.FriendlyName
            if (-not $name) {
                $name = $disk.Model
            }
        }
        if (-not $name) {
            $name = $_.FileSystemLabel
        }
        "$letter`t$name"
    }
"#;
    run_powershell_sensor_script(script)
        .ok()
        .map(|output| parse_drive_device_names(&output))
        .unwrap_or_default()
}

#[cfg(windows)]
fn parse_drive_device_names(output: &str) -> HashMap<char, String> {
    output
        .lines()
        .filter_map(|line| {
            let (letter, name) = line.split_once('\t')?;
            let letter = letter.trim().chars().next()?.to_ascii_uppercase();
            if !letter.is_ascii_alphabetic() {
                return None;
            }
            let name = name.trim();
            (!name.is_empty()).then(|| (letter, name.to_owned()))
        })
        .collect()
}

fn selected_drive_for_path(drives: &[DriveInfo], path: &PathBuf) -> Option<usize> {
    let root = drive_root_for_path(path)?;
    drives
        .iter()
        .position(|drive| same_drive_root(&drive.root, &root))
}

fn same_drive_root(left: &PathBuf, right: &PathBuf) -> bool {
    let left = left.display().to_string();
    let right = right.display().to_string();
    let left = left.trim_end_matches(|ch| ch == '\\' || ch == '/');
    let right = right.trim_end_matches(|ch| ch == '\\' || ch == '/');
    left.eq_ignore_ascii_case(right)
}

fn drive_root_for_path(path: &PathBuf) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let text = path.display().to_string();
        let bytes = text.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            let letter = (bytes[0] as char).to_ascii_uppercase();
            return Some(PathBuf::from(format!("{letter}:\\")));
        }
        None
    }

    #[cfg(not(windows))]
    {
        let _ = path;
        Some(PathBuf::from("/"))
    }
}

fn drive_letter_for_path(path: &PathBuf) -> Option<char> {
    #[cfg(windows)]
    {
        let text = path.display().to_string();
        let bytes = text.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            return Some((bytes[0] as char).to_ascii_uppercase());
        }
        None
    }

    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

fn start_sensor_helper_reader() -> Option<Receiver<SensorSnapshot>> {
    let helper_path = sensor_helper_path()?;
    let mut command = Command::new(&helper_path);
    command
        .arg("--stream")
        .arg("--parent-pid")
        .arg(std::process::id().to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW_RAW);

    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();

    let _ = thread::Builder::new()
        .name("benchscope-sensor-helper".to_owned())
        .spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if let Some(snapshot) = parse_helper_snapshot(&line) {
                    let _ = tx.send(snapshot);
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        });

    Some(rx)
}

fn read_sensor_helper_snapshot_file(
    path: &PathBuf,
    last_modified: &mut Option<SystemTime>,
) -> Option<SensorSnapshot> {
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok();
    if let (Some(modified), Some(previous)) = (modified, *last_modified) {
        if modified <= previous {
            return None;
        }
    }

    let contents = fs::read_to_string(path).ok()?;
    let snapshot = parse_helper_snapshot(contents.trim())?;
    if let Some(modified) = modified {
        *last_modified = Some(modified);
    }
    Some(snapshot)
}

#[cfg(test)]
fn helper_snapshot_needs_elevation(snapshot: &SensorSnapshot) -> bool {
    snapshot.helper_elevated == Some(false)
        && [
            snapshot.cpu.as_ref(),
            snapshot.gpu.as_ref(),
            snapshot.drive.as_ref(),
        ]
        .into_iter()
        .flatten()
        .any(|reading| {
            reading.temperature_c.is_none()
                && reading
                    .provider
                    .eq_ignore_ascii_case("LibreHardwareMonitor")
        })
}

fn helper_snapshot_has_gaps(snapshot: &SensorSnapshot) -> bool {
    sensor_reading_has_gap(snapshot.cpu.as_ref())
        || sensor_reading_has_gap(snapshot.gpu.as_ref())
        || sensor_reading_has_gap(snapshot.drive.as_ref())
}

fn sensor_reading_has_gap(reading: Option<&SensorReading>) -> bool {
    match reading {
        Some(reading) => !reading.has_temperature() || !reading.has_utilization(),
        None => true,
    }
}

fn merge_sensor_snapshots(
    primary: Option<SensorSnapshot>,
    fallback: Option<SensorSnapshot>,
) -> Option<SensorSnapshot> {
    match (primary, fallback) {
        (Some(primary), Some(fallback)) => Some(SensorSnapshot {
            cpu: prefer_sensor_reading(primary.cpu, fallback.cpu),
            gpu: prefer_sensor_reading(primary.gpu, fallback.gpu),
            drive: prefer_sensor_reading(primary.drive, fallback.drive),
            helper_elevated: primary.helper_elevated,
        }),
        (Some(primary), None) => Some(primary),
        (None, Some(fallback)) => Some(fallback),
        (None, None) => None,
    }
}

fn prefer_sensor_reading(
    primary: Option<SensorReading>,
    fallback: Option<SensorReading>,
) -> Option<SensorReading> {
    match (primary, fallback) {
        (Some(primary), Some(fallback)) => Some(merge_sensor_reading(primary, fallback)),
        (Some(primary), None) => Some(primary),
        (None, fallback) => fallback,
    }
}

fn merge_sensor_reading(mut primary: SensorReading, fallback: SensorReading) -> SensorReading {
    if primary.temperature_c.is_none() && fallback.temperature_c.is_some() {
        primary.temperature_c = fallback.temperature_c;
        if primary.label.is_empty() || !primary.has_utilization() {
            primary.label = fallback.label.clone();
        }
    }
    if primary.utilization_percent.is_none() && fallback.utilization_percent.is_some() {
        primary.utilization_percent = fallback.utilization_percent;
    }
    if !primary.is_ok()
        && (primary.temperature_c.is_some() || primary.utilization_percent.is_some())
    {
        primary.status = SensorStatus::Ok;
    }
    if primary.provider != fallback.provider
        && (fallback.temperature_c.is_some() || fallback.utilization_percent.is_some())
    {
        primary.provider = format!("{} + {}", primary.provider, fallback.provider);
    }
    primary
}

fn apply_integrated_gpu_temperature_fallback(
    mut snapshot: SensorSnapshot,
    enabled: bool,
) -> SensorSnapshot {
    if !enabled {
        return snapshot;
    }

    let Some(cpu) = snapshot.cpu.as_ref() else {
        return snapshot;
    };
    let Some(cpu_temperature) = cpu.temperature_c else {
        return snapshot;
    };
    if snapshot
        .gpu
        .as_ref()
        .is_some_and(|reading| reading.temperature_c.is_some())
    {
        return snapshot;
    }

    let utilization_percent = snapshot
        .gpu
        .as_ref()
        .and_then(|reading| reading.utilization_percent);
    snapshot.gpu = Some(SensorReading {
        kind: SensorKind::Gpu,
        label: "iGPU shared CPU package".to_owned(),
        temperature_c: Some(cpu_temperature),
        utilization_percent,
        provider: format!("Shared CPU package ({})", cpu.provider),
        updated_at: Instant::now(),
        status: SensorStatus::Ok,
    });
    snapshot
}

#[cfg(windows)]
fn start_elevated_sensor_helper_file() -> Option<PathBuf> {
    let helper_path = sensor_helper_path()?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    let output_path = std::env::temp_dir().join(format!(
        "BenchScope.SensorHelper-{}-{timestamp}.json",
        std::process::id()
    ));
    let arguments = [
        windows_command_arg("--out-file"),
        windows_command_arg(&output_path.display().to_string()),
        windows_command_arg("--parent-pid"),
        windows_command_arg(&std::process::id().to_string()),
    ]
    .join(" ");
    let script = format!(
        "Start-Process -FilePath {} -ArgumentList {} -Verb RunAs -WindowStyle Hidden",
        powershell_quote(&helper_path.display().to_string()),
        powershell_quote(&arguments)
    );
    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .creation_flags(CREATE_NO_WINDOW_RAW)
        .status()
        .ok()?;
    status.success().then_some(output_path)
}

#[cfg(not(windows))]
fn start_elevated_sensor_helper_file() -> Option<PathBuf> {
    None
}

#[cfg(windows)]
fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(windows)]
fn windows_command_arg(value: &str) -> String {
    let mut quoted = String::from("\"");
    let mut backslashes = 0;
    for ch in value.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.push_str(&"\\".repeat(backslashes));
                backslashes = 0;
                quoted.push(ch);
            }
        }
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

fn sensor_helper_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(dir) = exe_path.parent() {
            candidates.push(dir.join("BenchScope.SensorHelper.exe"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("target/release/BenchScope.SensorHelper.exe"));
        candidates.push(cwd.join("target/debug/BenchScope.SensorHelper.exe"));
        candidates.push(
            cwd.join("sensor-helper/bin/Release/net10.0-windows/BenchScope.SensorHelper.exe"),
        );
        candidates
            .push(cwd.join("sensor-helper/bin/Debug/net10.0-windows/BenchScope.SensorHelper.exe"));
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn parse_helper_snapshot(line: &str) -> Option<SensorSnapshot> {
    if !line.contains("\"timestampUtc\"") && !line.contains("\"cpu\"") {
        return None;
    }

    Some(SensorSnapshot {
        cpu: parse_helper_reading(line, "cpu", SensorKind::Cpu, "CPU"),
        gpu: parse_helper_reading(line, "gpu", SensorKind::Gpu, "GPU"),
        drive: parse_helper_reading(line, "drive", SensorKind::Drive, "SSD"),
        helper_elevated: json_bool_for_key(line, "isElevated"),
    })
}

fn parse_helper_reading(
    line: &str,
    key: &str,
    kind: SensorKind,
    fallback_label: &str,
) -> Option<SensorReading> {
    let object = json_object_for_key(line, key)?;
    let label = json_string_for_key(object, "label").unwrap_or_else(|| fallback_label.to_owned());
    let provider = json_string_for_key(object, "provider")
        .unwrap_or_else(|| "LibreHardwareMonitor".to_owned());
    let status_text =
        json_string_for_key(object, "status").unwrap_or_else(|| "unsupported".to_owned());
    let message = json_string_for_key(object, "message");
    let temperature_c = json_number_for_key(object, "temperatureC");
    let utilization_percent = json_number_for_key(object, "utilizationPercent").map(clamp_percent);
    let status = helper_sensor_status(&status_text, message);

    Some(SensorReading {
        kind,
        label,
        temperature_c,
        utilization_percent,
        provider,
        updated_at: Instant::now(),
        status,
    })
}

fn helper_sensor_status(status: &str, message: Option<String>) -> SensorStatus {
    match status.to_ascii_lowercase().as_str() {
        "ok" => SensorStatus::Ok,
        "permissiondenied" | "permission_denied" | "permission" => SensorStatus::PermissionDenied,
        "unsupported" => message
            .map(SensorStatus::Error)
            .unwrap_or(SensorStatus::Unsupported),
        "stale" => SensorStatus::Stale,
        "error" => SensorStatus::Error(message.unwrap_or_else(|| "Sensor helper error".to_owned())),
        _ => SensorStatus::Error(
            message.unwrap_or_else(|| format!("Unknown helper status: {status}")),
        ),
    }
}

fn json_object_for_key<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pattern = format!("\"{key}\":");
    let start = line.find(&pattern)? + pattern.len();
    let bytes = line.as_bytes();
    let mut index = start;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if bytes.get(index).copied()? != b'{' {
        return None;
    }

    let object_start = index;
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in line[object_start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let end = object_start + offset + ch.len_utf8();
                    return Some(&line[object_start..end]);
                }
            }
            _ => {}
        }
    }
    None
}

fn json_string_for_key(object: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{key}\":");
    let start = object.find(&pattern)? + pattern.len();
    let mut index = start;
    let bytes = object.as_bytes();
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if bytes.get(index).copied()? != b'"' {
        return None;
    }
    index += 1;
    let mut value = String::new();
    let mut escaped = false;
    for ch in object[index..].chars() {
        if escaped {
            value.push(match ch {
                '"' => '"',
                '\\' => '\\',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(value);
        } else {
            value.push(ch);
        }
    }
    None
}

fn json_number_for_key(object: &str, key: &str) -> Option<f32> {
    let pattern = format!("\"{key}\":");
    let start = object.find(&pattern)? + pattern.len();
    let mut value = String::new();
    let mut started = false;
    for ch in object[start..].chars() {
        if ch.is_ascii_digit() || ch == '.' || ch == '-' {
            value.push(ch);
            started = true;
        } else if started {
            break;
        } else if ch == 'n' {
            return None;
        } else if !ch.is_ascii_whitespace() {
            return None;
        }
    }
    value.parse::<f32>().ok()
}

fn json_bool_for_key(object: &str, key: &str) -> Option<bool> {
    let pattern = format!("\"{key}\":");
    let start = object.find(&pattern)? + pattern.len();
    let value = object[start..].trim_start();
    if value.starts_with("true") {
        Some(true)
    } else if value.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn stale_checked_reading(
    reading: Option<SensorReading>,
    now: Instant,
    stale_after: Duration,
) -> Option<SensorReading> {
    reading.map(|reading| {
        if now.duration_since(reading.updated_at) > stale_after {
            reading.mark_stale()
        } else {
            reading
        }
    })
}

fn sensor_temperature(reading: Option<&SensorReading>) -> Option<f32> {
    reading
        .filter(|reading| reading.is_ok())
        .and_then(|reading| reading.temperature_c)
}

fn collect_sensor_snapshot(drive_letter: Option<char>) -> SensorSnapshot {
    let mut cpu = query_cpu_temperature();
    cpu.utilization_percent = query_cpu_utilization();
    let mut gpu = query_gpu_temperature();
    gpu.utilization_percent = query_gpu_utilization();
    let mut drive = query_drive_temperature(drive_letter);
    drive.utilization_percent = query_drive_utilization(drive_letter);

    SensorSnapshot {
        cpu: Some(cpu),
        gpu: Some(gpu),
        drive: Some(drive),
        helper_elevated: None,
    }
}

fn query_cpu_temperature() -> SensorReading {
    #[cfg(windows)]
    {
        let script = r#"
$zone = Get-CimInstance -Namespace root/wmi -ClassName MSAcpi_ThermalZoneTemperature -ErrorAction SilentlyContinue | Select-Object -First 1
if ($zone -and $zone.CurrentTemperature) {
    [math]::Round(($zone.CurrentTemperature / 10) - 273.15, 1)
}
"#;
        match run_powershell_sensor_script(script) {
            Ok(output) => parse_first_temperature(&output)
                .map(|temp| SensorReading::ok(SensorKind::Cpu, "CPU", temp, "ACPI thermal zone"))
                .unwrap_or_else(|| {
                    SensorReading::unavailable(
                        SensorKind::Cpu,
                        "CPU",
                        "ACPI thermal zone",
                        SensorStatus::Unsupported,
                    )
                }),
            Err(err) => SensorReading::unavailable(
                SensorKind::Cpu,
                "CPU",
                "ACPI thermal zone",
                sensor_error_status(err),
            ),
        }
    }

    #[cfg(not(windows))]
    {
        SensorReading::unavailable(
            SensorKind::Cpu,
            "CPU",
            "Windows sensors",
            SensorStatus::Unsupported,
        )
    }
}

fn query_gpu_temperature() -> SensorReading {
    #[cfg(windows)]
    {
        match run_nvidia_smi_temperature_query() {
            Ok(output) => parse_first_temperature(&output)
                .map(|temp| SensorReading::ok(SensorKind::Gpu, "GPU", temp, "NVML/nvidia-smi"))
                .unwrap_or_else(|| {
                    SensorReading::unavailable(
                        SensorKind::Gpu,
                        "GPU",
                        "NVML/nvidia-smi",
                        SensorStatus::Unsupported,
                    )
                }),
            Err(err) => SensorReading::unavailable(
                SensorKind::Gpu,
                "GPU",
                "NVML/nvidia-smi",
                sensor_error_status(err),
            ),
        }
    }

    #[cfg(not(windows))]
    {
        SensorReading::unavailable(
            SensorKind::Gpu,
            "GPU",
            "Windows sensors",
            SensorStatus::Unsupported,
        )
    }
}

fn query_drive_temperature(drive_letter: Option<char>) -> SensorReading {
    let Some(drive_letter) = drive_letter else {
        return SensorReading::unavailable(
            SensorKind::Drive,
            "SSD",
            "Windows Storage",
            SensorStatus::Unsupported,
        );
    };
    let label = format!("SSD {drive_letter}:");

    #[cfg(windows)]
    {
        let script = format!(
            r#"
$ErrorActionPreference = 'Stop'
$letter = '{drive_letter}'
$partition = Get-Partition -DriveLetter $letter | Select-Object -First 1
if ($partition) {{
    $disk = $partition | Get-Disk
    if ($disk) {{
        $physical = Get-PhysicalDisk |
            Where-Object {{ $_.DeviceId -eq "$($disk.Number)" -or $_.SerialNumber -eq $disk.SerialNumber }} |
            Select-Object -First 1
        if ($physical) {{
            $counter = $physical | Get-StorageReliabilityCounter
            if ($counter -and $counter.Temperature) {{
                [math]::Round($counter.Temperature, 1)
            }}
        }}
    }}
}}
"#
        );
        match run_powershell_sensor_script(&script) {
            Ok(output) => parse_first_temperature(&output)
                .map(|temp| {
                    SensorReading::ok(
                        SensorKind::Drive,
                        label.clone(),
                        temp,
                        "Windows Storage SMART",
                    )
                })
                .unwrap_or_else(|| {
                    SensorReading::unavailable(
                        SensorKind::Drive,
                        label.clone(),
                        "Windows Storage SMART",
                        SensorStatus::Unsupported,
                    )
                }),
            Err(err) => SensorReading::unavailable(
                SensorKind::Drive,
                label,
                "Windows Storage SMART",
                sensor_error_status(err),
            ),
        }
    }

    #[cfg(not(windows))]
    {
        let _ = drive_letter;
        SensorReading::unavailable(
            SensorKind::Drive,
            label,
            "Windows Storage SMART",
            SensorStatus::Unsupported,
        )
    }
}

fn query_cpu_utilization() -> Option<f32> {
    #[cfg(windows)]
    {
        let script = r#"
$counter = Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor -Filter "Name='_Total'" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($counter) {
    [math]::Round([math]::Min(100, [math]::Max(0, $counter.PercentProcessorTime)), 1)
}
"#;
        return run_powershell_sensor_script(script)
            .ok()
            .and_then(|output| parse_first_utilization(&output));
    }

    #[cfg(not(windows))]
    {
        None
    }
}

fn query_gpu_utilization() -> Option<f32> {
    #[cfg(windows)]
    {
        let script = r#"
$engines = Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine -ErrorAction SilentlyContinue
if ($engines) {
    $sum = ($engines | Measure-Object -Property UtilizationPercentage -Sum).Sum
    [math]::Round([math]::Min(100, [math]::Max(0, $sum)), 1)
}
"#;
        return run_powershell_sensor_script(script)
            .ok()
            .and_then(|output| parse_first_utilization(&output));
    }

    #[cfg(not(windows))]
    {
        None
    }
}

fn query_drive_utilization(drive_letter: Option<char>) -> Option<f32> {
    let drive_letter = drive_letter?;

    #[cfg(windows)]
    {
        let script = format!(
            r#"
$counter = Get-CimInstance Win32_PerfFormattedData_PerfDisk_LogicalDisk -Filter "Name='{drive_letter}:'" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($counter) {{
    [math]::Round([math]::Min(100, [math]::Max(0, $counter.PercentDiskTime)), 1)
}}
"#
        );
        return run_powershell_sensor_script(&script)
            .ok()
            .and_then(|output| parse_first_utilization(&output));
    }

    #[cfg(not(windows))]
    {
        let _ = drive_letter;
        None
    }
}

fn parse_first_temperature(output: &str) -> Option<f32> {
    output
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.' || ch == '-'))
        .filter(|token| !token.is_empty())
        .filter_map(|token| token.parse::<f32>().ok())
        .find(|value| (-40.0..=130.0).contains(value))
}

fn parse_first_utilization(output: &str) -> Option<f32> {
    output
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.' || ch == '-'))
        .filter(|token| !token.is_empty())
        .filter_map(|token| token.parse::<f32>().ok())
        .find(|value| (0.0..=10_000.0).contains(value))
        .map(clamp_percent)
}

fn clamp_percent(value: f32) -> f32 {
    (value.clamp(0.0, 100.0) * 10.0).round() / 10.0
}

fn sensor_error_status(err: anyhow::Error) -> SensorStatus {
    let message = err.to_string();
    let lower = message.to_ascii_lowercase();
    if lower.contains("access is denied") || lower.contains("permission") {
        SensorStatus::PermissionDenied
    } else if lower.contains("not found")
        || lower.contains("not recognized")
        || lower.contains("cannot find")
        || lower.contains("os error 2")
    {
        SensorStatus::Unsupported
    } else {
        SensorStatus::Error(message)
    }
}

#[cfg(windows)]
fn run_powershell_sensor_script(script: &str) -> Result<String> {
    run_command_no_window(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ],
    )
}

#[cfg(windows)]
fn run_nvidia_smi_temperature_query() -> Result<String> {
    const NVIDIA_SMI_ARGS: &[&str] = &[
        "--query-gpu=temperature.gpu",
        "--format=csv,noheader,nounits",
    ];

    match run_command_no_window("nvidia-smi", NVIDIA_SMI_ARGS) {
        Ok(output) => Ok(output),
        Err(path_err) => {
            let fallback = r"C:\Program Files\NVIDIA Corporation\NVSMI\nvidia-smi.exe";
            run_command_no_window(fallback, NVIDIA_SMI_ARGS).map_err(|fallback_err| {
                anyhow!(
                    "nvidia-smi unavailable on PATH ({path_err}) or at default install path ({fallback_err})"
                )
            })
        }
    }
}

#[cfg(windows)]
fn run_command_no_window(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW_RAW)
        .output()
        .with_context(|| format!("failed to start {program}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(anyhow!(
            "{} failed{}",
            program,
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {stderr}")
            }
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn open_drive_file_direct_preferred(
    path: &PathBuf,
    read: bool,
    write: bool,
    create: bool,
    truncate: bool,
    sequential: bool,
) -> Result<DriveOpenFile> {
    #[cfg(windows)]
    {
        let mut direct_options = OpenOptions::new();
        direct_options
            .read(read)
            .write(write)
            .create(create)
            .truncate(truncate);
        let access_hint = if sequential {
            FILE_FLAG_SEQUENTIAL_SCAN_RAW
        } else {
            FILE_FLAG_RANDOM_ACCESS_RAW
        };
        let write_hint = if write {
            FILE_FLAG_WRITE_THROUGH_RAW
        } else {
            0
        };
        direct_options.custom_flags(FILE_FLAG_NO_BUFFERING_RAW | write_hint | access_hint);
        match direct_options.open(path) {
            Ok(file) => {
                return Ok(DriveOpenFile {
                    file,
                    io_mode: DriveIoMode::Direct,
                    fallback_note: None,
                });
            }
            Err(err) => {
                let file = OpenOptions::new()
                    .read(read)
                    .write(write)
                    .create(create)
                    .truncate(truncate)
                    .open(path)
                    .with_context(|| {
                        format!(
                            "failed to open benchmark file {} after direct I/O failed",
                            path.display()
                        )
                    })?;
                return Ok(DriveOpenFile {
                    file,
                    io_mode: DriveIoMode::Cached,
                    fallback_note: Some(format!("Direct I/O unavailable: {err}")),
                });
            }
        }
    }

    #[cfg(not(windows))]
    {
        let file = OpenOptions::new()
            .read(read)
            .write(write)
            .create(create)
            .truncate(truncate)
            .open(path)
            .with_context(|| format!("failed to open benchmark file {}", path.display()))?;
        Ok(DriveOpenFile {
            file,
            io_mode: DriveIoMode::Cached,
            fallback_note: Some("Direct I/O is only implemented on Windows".to_owned()),
        })
    }
}

fn run_drive_benchmark(
    config: DriveBenchmarkConfig,
    cancel: Arc<AtomicBool>,
    tx: Sender<DriveWorkerEvent>,
) -> Result<Vec<DriveBenchmarkResult>> {
    let test_path = config.target_folder.join(DRIVE_BENCHMARK_FILE_NAME);
    let _ = tx.send(DriveWorkerEvent::Log(format!(
        "Using direct file I/O when available, with cached fallback."
    )));
    let _ = tx.send(DriveWorkerEvent::Log(format!(
        "Temporary benchmark file: {}",
        test_path.display()
    )));

    if config.selected_tests.iter().any(|test| test.is_read()) {
        prepare_drive_benchmark_file(&test_path, config.file_size_bytes, &cancel, &tx)?;
    }

    let suite_started = Instant::now();
    let mut results = Vec::new();
    let total_tests = config.selected_tests.len().max(1);
    for (index, test) in config.selected_tests.iter().copied().enumerate() {
        check_canceled_with(Some(&cancel), "Drive benchmark canceled")?;
        let result = run_drive_test(
            &test_path,
            config.file_size_bytes,
            config.profile,
            test,
            index,
            total_tests,
            suite_started,
            &cancel,
            &tx,
        )?;
        let _ = tx.send(DriveWorkerEvent::Log(format!(
            "{} complete: read {}, write {}, IOPS {}, duration {} ms",
            result.test,
            format_optional_rate(result.read_mbps),
            format_optional_rate(result.write_mbps),
            format_optional_iops(result.iops),
            format_ms(Some(result.duration_ms))
        )));
        results.push(result);
    }

    if let Err(err) = fs::remove_file(&test_path) {
        let _ = tx.send(DriveWorkerEvent::Log(format!(
            "Could not delete temporary benchmark file {}: {err}",
            test_path.display()
        )));
    }

    Ok(results)
}

fn prepare_drive_benchmark_file(
    path: &PathBuf,
    file_size_bytes: u64,
    cancel: &AtomicBool,
    tx: &Sender<DriveWorkerEvent>,
) -> Result<()> {
    check_canceled_with(
        Some(cancel),
        "Drive benchmark canceled during file preparation",
    )?;
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(true)
        .open(path)
        .with_context(|| format!("failed to create benchmark file {}", path.display()))?;
    file.set_len(file_size_bytes)
        .with_context(|| format!("failed to size benchmark file {}", path.display()))?;

    let mut buffer = vec![0_u8; DRIVE_SEQUENTIAL_BLOCK_BYTES];
    let mut written = 0_u64;
    let started = Instant::now();
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    let mut block_seed = 0xA5A5_5A5A_D15C_BEAD_u64;

    while written < file_size_bytes {
        check_canceled_with(
            Some(cancel),
            "Drive benchmark canceled during file preparation",
        )?;
        fill_drive_buffer(&mut buffer, block_seed);
        block_seed = splitmix64(&mut block_seed);
        let remaining = (file_size_bytes - written) as usize;
        let len = remaining.min(buffer.len());
        file.write_all(&buffer[..len])
            .context("failed to initialize benchmark file")?;
        written += len as u64;

        let now = Instant::now();
        if now.duration_since(last_emit) >= Duration::from_millis(PROGRESS_SAMPLE_MS) {
            last_emit = now;
            let progress = (written as f32 / file_size_bytes.max(1) as f32).clamp(0.0, 1.0);
            let elapsed_s = started.elapsed().as_secs_f64();
            let eta_s = if progress > 0.001 && progress < 1.0 {
                let total = elapsed_s / progress as f64;
                Some((total - elapsed_s).max(0.0))
            } else {
                None
            };
            let _ = tx.send(DriveWorkerEvent::Progress(DriveProgress {
                current_test: "Preparing read file".to_owned(),
                current_progress: progress,
                suite_progress: 0.0,
                elapsed_s,
                eta_s,
                bytes_processed: written,
                operations: 0,
            }));
        }
    }

    file.sync_all()
        .context("failed to flush prepared benchmark file")?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_drive_test(
    path: &PathBuf,
    file_size_bytes: u64,
    profile: DriveProfile,
    test: DriveTestKind,
    test_index: usize,
    total_tests: usize,
    suite_started: Instant,
    cancel: &AtomicBool,
    tx: &Sender<DriveWorkerEvent>,
) -> Result<DriveBenchmarkResult> {
    match test {
        DriveTestKind::SequentialRead => run_sequential_drive_read(
            path,
            file_size_bytes,
            profile,
            test,
            test_index,
            total_tests,
            suite_started,
            cancel,
            tx,
        ),
        DriveTestKind::SequentialWrite => run_sequential_drive_write(
            path,
            file_size_bytes,
            profile,
            test,
            test_index,
            total_tests,
            suite_started,
            cancel,
            tx,
        ),
        DriveTestKind::RandomRead4K => run_random_drive_test(
            path,
            file_size_bytes,
            profile,
            test,
            test_index,
            total_tests,
            suite_started,
            cancel,
            tx,
            false,
        ),
        DriveTestKind::RandomWrite4K => run_random_drive_test(
            path,
            file_size_bytes,
            profile,
            test,
            test_index,
            total_tests,
            suite_started,
            cancel,
            tx,
            true,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_sequential_drive_write(
    path: &PathBuf,
    file_size_bytes: u64,
    profile: DriveProfile,
    test: DriveTestKind,
    test_index: usize,
    total_tests: usize,
    suite_started: Instant,
    cancel: &AtomicBool,
    tx: &Sender<DriveWorkerEvent>,
) -> Result<DriveBenchmarkResult> {
    let opened = open_drive_file_direct_preferred(path, true, true, true, false, true)?;
    let mut file = opened.file;
    let mut notes = Vec::new();
    if let Some(note) = opened.fallback_note {
        notes.push(note);
    }
    file.set_len(file_size_bytes)
        .context("failed to size benchmark file for sequential write")?;
    file.seek(SeekFrom::Start(0))
        .context("failed to seek benchmark file")?;

    let target_duration = profile.target_duration();
    let mut buffer = AlignedBuffer::new(DRIVE_SEQUENTIAL_BLOCK_BYTES, DRIVE_RANDOM_BLOCK_BYTES);
    let mut offset = 0_u64;
    let mut bytes_processed = 0_u64;
    let mut operations = 0_u64;
    let mut seed = 0x5151_5151_BEEF_CAFE_u64;
    let started = Instant::now();
    let mut last_emit = Instant::now() - Duration::from_secs(1);

    while should_continue_drive_test(started, target_duration) {
        check_canceled_with(Some(cancel), "Drive benchmark canceled")?;
        if offset >= file_size_bytes {
            offset = 0;
            file.seek(SeekFrom::Start(0))
                .context("failed to rewind benchmark file")?;
        }
        fill_drive_buffer(buffer.as_mut_slice(), seed);
        seed = splitmix64(&mut seed);
        let len = ((file_size_bytes - offset) as usize).min(buffer.len());
        let op_started = Instant::now();
        file.write_all(&buffer.as_slice()[..len])
            .context("sequential write failed")?;
        let _op_elapsed = op_started.elapsed();
        offset += len as u64;
        bytes_processed += len as u64;
        operations += 1;
        emit_drive_progress(
            tx,
            test,
            test_index,
            total_tests,
            started,
            suite_started,
            target_duration,
            bytes_processed,
            operations,
            &mut last_emit,
            false,
        );
    }

    check_canceled_with(Some(cancel), "Drive benchmark canceled before flush")?;
    file.sync_all().context("sequential write flush failed")?;
    emit_drive_progress(
        tx,
        test,
        test_index,
        total_tests,
        started,
        suite_started,
        target_duration,
        bytes_processed,
        operations,
        &mut last_emit,
        true,
    );

    notes.push("Flush included".to_owned());
    Ok(make_drive_result(
        test,
        bytes_processed,
        operations,
        started.elapsed(),
        file_size_bytes,
        opened.io_mode,
        Vec::new(),
        notes,
    ))
}

#[allow(clippy::too_many_arguments)]
fn run_sequential_drive_read(
    path: &PathBuf,
    file_size_bytes: u64,
    profile: DriveProfile,
    test: DriveTestKind,
    test_index: usize,
    total_tests: usize,
    suite_started: Instant,
    cancel: &AtomicBool,
    tx: &Sender<DriveWorkerEvent>,
) -> Result<DriveBenchmarkResult> {
    let opened = open_drive_file_direct_preferred(path, true, false, false, false, true)?;
    let mut file = opened.file;
    let mut notes = Vec::new();
    if let Some(note) = opened.fallback_note {
        notes.push(note);
    }
    let target_duration = profile.target_duration();
    let mut buffer = AlignedBuffer::new(DRIVE_SEQUENTIAL_BLOCK_BYTES, DRIVE_RANDOM_BLOCK_BYTES);
    let mut offset = 0_u64;
    let mut bytes_processed = 0_u64;
    let mut operations = 0_u64;
    let mut checksum = 0_u64;
    let started = Instant::now();
    let mut last_emit = Instant::now() - Duration::from_secs(1);

    while should_continue_drive_test(started, target_duration) {
        check_canceled_with(Some(cancel), "Drive benchmark canceled")?;
        if offset >= file_size_bytes {
            offset = 0;
            file.seek(SeekFrom::Start(0))
                .context("failed to rewind benchmark file")?;
        }
        let len = ((file_size_bytes - offset) as usize).min(buffer.len());
        file.read_exact(&mut buffer.as_mut_slice()[..len])
            .context("sequential read failed")?;
        checksum = checksum
            .wrapping_add(buffer.as_slice()[0] as u64)
            .wrapping_add(buffer.as_slice()[len - 1] as u64);
        offset += len as u64;
        bytes_processed += len as u64;
        operations += 1;
        emit_drive_progress(
            tx,
            test,
            test_index,
            total_tests,
            started,
            suite_started,
            target_duration,
            bytes_processed,
            operations,
            &mut last_emit,
            false,
        );
    }

    emit_drive_progress(
        tx,
        test,
        test_index,
        total_tests,
        started,
        suite_started,
        target_duration,
        bytes_processed,
        operations,
        &mut last_emit,
        true,
    );

    notes.push(format!("Checksum {checksum:016X}"));
    Ok(make_drive_result(
        test,
        bytes_processed,
        operations,
        started.elapsed(),
        file_size_bytes,
        opened.io_mode,
        Vec::new(),
        notes,
    ))
}

#[allow(clippy::too_many_arguments)]
fn run_random_drive_test(
    path: &PathBuf,
    file_size_bytes: u64,
    profile: DriveProfile,
    test: DriveTestKind,
    test_index: usize,
    total_tests: usize,
    suite_started: Instant,
    cancel: &AtomicBool,
    tx: &Sender<DriveWorkerEvent>,
    write_mode: bool,
) -> Result<DriveBenchmarkResult> {
    let opened =
        open_drive_file_direct_preferred(path, true, write_mode, write_mode, false, false)?;
    let mut file = opened.file;
    let mut notes = Vec::new();
    if let Some(note) = opened.fallback_note {
        notes.push(note);
    }
    if write_mode {
        file.set_len(file_size_bytes)
            .context("failed to size benchmark file for random write")?;
    }

    let target_duration = profile.target_duration();
    let block_count = (file_size_bytes / DRIVE_RANDOM_BLOCK_BYTES as u64).max(1);
    let mut buffer = AlignedBuffer::new(DRIVE_RANDOM_BLOCK_BYTES, DRIVE_RANDOM_BLOCK_BYTES);
    fill_drive_buffer(buffer.as_mut_slice(), 0x4449_534B_524E_4434_u64);
    let mut rng = 0xC001_D00D_F00D_BAAD_u64;
    let mut bytes_processed = 0_u64;
    let mut operations = 0_u64;
    let mut latency_samples_ms = Vec::new();
    let mut latency_total_ms = 0.0_f64;
    let mut checksum = 0_u64;
    let started = Instant::now();
    let mut last_emit = Instant::now() - Duration::from_secs(1);

    while should_continue_drive_test(started, target_duration) {
        check_canceled_with(Some(cancel), "Drive benchmark canceled")?;
        let block_index = splitmix64(&mut rng) % block_count;
        let offset = block_index * DRIVE_RANDOM_BLOCK_BYTES as u64;
        file.seek(SeekFrom::Start(offset))
            .context("random seek failed")?;
        if write_mode {
            buffer.as_mut_slice()[..8].copy_from_slice(&operations.to_le_bytes());
        }

        let op_started = Instant::now();
        if write_mode {
            file.write_all(buffer.as_slice())
                .context("random write failed")?;
        } else {
            file.read_exact(buffer.as_mut_slice())
                .context("random read failed")?;
            checksum = checksum
                .wrapping_add(buffer.as_slice()[0] as u64)
                .wrapping_add(buffer.as_slice()[DRIVE_RANDOM_BLOCK_BYTES - 1] as u64);
        }
        let op_ms = op_started.elapsed().as_secs_f64() * 1000.0;
        latency_total_ms += op_ms;
        record_latency_sample(&mut latency_samples_ms, operations, op_ms);
        bytes_processed += DRIVE_RANDOM_BLOCK_BYTES as u64;
        operations += 1;

        emit_drive_progress(
            tx,
            test,
            test_index,
            total_tests,
            started,
            suite_started,
            target_duration,
            bytes_processed,
            operations,
            &mut last_emit,
            false,
        );
    }

    if write_mode {
        check_canceled_with(Some(cancel), "Drive benchmark canceled before flush")?;
        file.sync_all().context("random write flush failed")?;
    }

    emit_drive_progress(
        tx,
        test,
        test_index,
        total_tests,
        started,
        suite_started,
        target_duration,
        bytes_processed,
        operations,
        &mut last_emit,
        true,
    );

    if write_mode {
        notes.push("Flush included".to_owned());
    } else {
        notes.push(format!("Checksum {checksum:016X}"));
    }

    let mut result = make_drive_result(
        test,
        bytes_processed,
        operations,
        started.elapsed(),
        file_size_bytes,
        opened.io_mode,
        latency_samples_ms,
        notes,
    );
    result.avg_latency_ms = (operations > 0).then_some(latency_total_ms / operations as f64);
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn emit_drive_progress(
    tx: &Sender<DriveWorkerEvent>,
    test: DriveTestKind,
    test_index: usize,
    total_tests: usize,
    test_started: Instant,
    suite_started: Instant,
    target_duration: Duration,
    bytes_processed: u64,
    operations: u64,
    last_emit: &mut Instant,
    force: bool,
) {
    let now = Instant::now();
    if !force && now.duration_since(*last_emit) < Duration::from_millis(PROGRESS_SAMPLE_MS) {
        return;
    }
    *last_emit = now;
    let elapsed_s = test_started.elapsed().as_secs_f64();
    let target_s = target_duration.as_secs_f64().max(0.001);
    let current_progress = (elapsed_s / target_s).clamp(0.0, 1.0) as f32;
    let suite_progress =
        ((test_index as f32 + current_progress) / total_tests.max(1) as f32).clamp(0.0, 1.0);
    let suite_elapsed_s = suite_started.elapsed().as_secs_f64();
    let eta_s = if suite_progress > 0.001 && suite_progress < 1.0 {
        let total = suite_elapsed_s / suite_progress as f64;
        Some((total - suite_elapsed_s).max(0.0))
    } else {
        None
    };

    let _ = tx.send(DriveWorkerEvent::Progress(DriveProgress {
        current_test: test.label().to_owned(),
        current_progress,
        suite_progress,
        elapsed_s: suite_elapsed_s,
        eta_s,
        bytes_processed,
        operations,
    }));
}

fn should_continue_drive_test(started: Instant, target_duration: Duration) -> bool {
    let elapsed = started.elapsed();
    elapsed < target_duration && elapsed.as_secs_f64() < DRIVE_MAX_TEST_SECONDS
}

fn make_drive_result(
    test: DriveTestKind,
    bytes_processed: u64,
    operations: u64,
    elapsed: Duration,
    file_size_bytes: u64,
    io_mode: DriveIoMode,
    latency_samples_ms: Vec<f64>,
    mut notes: Vec<String>,
) -> DriveBenchmarkResult {
    let elapsed_s = elapsed.as_secs_f64().max(0.001);
    let mbps = bytes_processed as f64 / DECIMAL_MB / elapsed_s;
    if elapsed.as_secs_f64() >= DRIVE_MAX_TEST_SECONDS {
        notes.push("Capped at 30s".to_owned());
    }
    let p95_latency_ms = percentile_latency_ms(latency_samples_ms, 0.95);
    let iops = matches!(
        test,
        DriveTestKind::RandomRead4K | DriveTestKind::RandomWrite4K
    )
    .then_some(operations as f64 / elapsed_s);

    DriveBenchmarkResult {
        test,
        read_mbps: test.is_read().then_some(mbps),
        write_mbps: test.is_write().then_some(mbps),
        iops,
        avg_latency_ms: None,
        p95_latency_ms,
        duration_ms: elapsed.as_secs_f64() * 1000.0,
        file_size_bytes,
        io_mode,
        notes,
        ssd_temperature: TemperatureSummary::default(),
    }
}

fn record_latency_sample(samples: &mut Vec<f64>, operation: u64, latency_ms: f64) {
    if samples.len() < DRIVE_LATENCY_SAMPLE_LIMIT || operation % 64 == 0 {
        if samples.len() < DRIVE_LATENCY_SAMPLE_LIMIT {
            samples.push(latency_ms);
        }
    }
}

fn percentile_latency_ms(mut samples: Vec<f64>, percentile: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    let index = ((samples.len() - 1) as f64 * percentile.clamp(0.0, 1.0)).round() as usize;
    samples.get(index).copied()
}

fn fill_drive_buffer(buffer: &mut [u8], mut seed: u64) {
    for chunk in buffer.chunks_mut(8) {
        let value = splitmix64(&mut seed).to_le_bytes();
        let len = chunk.len();
        chunk.copy_from_slice(&value[..len]);
    }
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn matrix_buffers_bytes(size: usize, matrix_count: u64) -> Option<u64> {
    let size = size as u64;
    size.checked_mul(size)?
        .checked_mul(std::mem::size_of::<f32>() as u64)?
        .checked_mul(matrix_count)
}

fn gpu_working_set_bytes(size: usize) -> Option<u64> {
    matrix_buffers_bytes(size, 4)
}

fn estimate_gpu_seconds(size: usize, adapter: &AdapterInfo, gpu_intensity: GpuIntensity) -> f64 {
    let n = size as f64;
    let flops = 2.0 * n * n * n;
    let throughput_flops = match adapter.device_type {
        wgpu::DeviceType::DiscreteGpu => 8.0e12,
        wgpu::DeviceType::IntegratedGpu => 7.0e11,
        wgpu::DeviceType::VirtualGpu => 5.0e11,
        wgpu::DeviceType::Cpu => 1.0e11,
        wgpu::DeviceType::Other => 1.0e12,
    };
    let bandwidth_bytes = match adapter.device_type {
        wgpu::DeviceType::DiscreteGpu => 12.0e9,
        wgpu::DeviceType::IntegratedGpu => 25.0e9,
        wgpu::DeviceType::VirtualGpu => 10.0e9,
        wgpu::DeviceType::Cpu => 8.0e9,
        wgpu::DeviceType::Other => 12.0e9,
    };
    let transfer_s = gpu_working_set_bytes(size)
        .map(|bytes| bytes as f64 / bandwidth_bytes)
        .unwrap_or(0.0);
    let compute_s = flops / throughput_flops;
    let safety_factor = match gpu_intensity {
        GpuIntensity::Safe => 1.8,
        GpuIntensity::Balanced => 1.25,
        GpuIntensity::High => 1.0,
    };
    (compute_s * safety_factor + transfer_s).max(0.2)
}

fn adapter_memory_limit_bytes(adapter: &AdapterInfo) -> Option<(u64, &'static str)> {
    let dedicated = adapter.dedicated_vram_bytes.unwrap_or(0);
    let shared = adapter.shared_system_memory_bytes.unwrap_or(0);
    match adapter.device_type {
        wgpu::DeviceType::DiscreteGpu if dedicated > 0 => Some((dedicated, "dedicated VRAM")),
        wgpu::DeviceType::IntegratedGpu
        | wgpu::DeviceType::Cpu
        | wgpu::DeviceType::VirtualGpu
        | wgpu::DeviceType::Other
            if dedicated + shared > 0 =>
        {
            Some((dedicated + shared, "reported GPU/shared memory"))
        }
        _ if dedicated > 0 => Some((dedicated, "dedicated VRAM")),
        _ => None,
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.2} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

fn format_optional_bytes(bytes: Option<u64>) -> String {
    bytes
        .map(format_bytes)
        .unwrap_or_else(|| "unknown".to_owned())
}

fn format_eta(value: Option<f64>) -> String {
    match value {
        Some(seconds) if seconds <= 0.5 => "ETA: <1s".to_owned(),
        Some(seconds) => format!("ETA: {}", format_elapsed(seconds)),
        None => "ETA: estimating".to_owned(),
    }
}

fn format_elapsed(seconds: f64) -> String {
    if seconds >= 3600.0 {
        let hours = (seconds / 3600.0).floor();
        let minutes = ((seconds % 3600.0) / 60.0).floor();
        format!("{hours:.0}h {minutes:.0}m")
    } else if seconds >= 60.0 {
        let minutes = (seconds / 60.0).floor();
        let secs = seconds % 60.0;
        format!("{minutes:.0}m {secs:.0}s")
    } else if seconds >= 10.0 {
        format!("{seconds:.0}s")
    } else {
        format!("{seconds:.1}s")
    }
}

fn format_ms(value: Option<f64>) -> String {
    match value {
        Some(value) if value >= 1000.0 => format!("{value:.1}"),
        Some(value) => format!("{value:.3}"),
        None => "N/A".to_owned(),
    }
}

fn format_cpu_ms(result: &BenchmarkResult) -> String {
    let value = format_ms(Some(result.cpu_ms));
    if result.cpu_estimated {
        format!("Est. {value}")
    } else {
        value
    }
}

fn format_speedup(value: f64) -> String {
    if value.is_infinite() {
        "inf".to_owned()
    } else {
        format!("{value:.2}x")
    }
}

fn format_optional_rate(value: Option<f64>) -> String {
    match value {
        Some(value) if value >= 1000.0 => format!("{value:.0}"),
        Some(value) if value >= 100.0 => format!("{value:.1}"),
        Some(value) => format!("{value:.2}"),
        None => "N/A".to_owned(),
    }
}

fn format_drive_speed(result: &DriveBenchmarkResult) -> String {
    if result.test.is_read() {
        format_optional_rate(result.read_mbps)
    } else {
        format_optional_rate(result.write_mbps)
    }
}

fn format_optional_iops(value: Option<f64>) -> String {
    match value {
        Some(value) if value >= 1_000_000.0 => format!("{:.2}M", value / 1_000_000.0),
        Some(value) if value >= 10_000.0 => format!("{:.0}K", value / 1_000.0),
        Some(value) if value >= 1000.0 => format!("{:.1}K", value / 1_000.0),
        Some(value) => format!("{value:.0}"),
        None => "N/A".to_owned(),
    }
}

fn format_optional_latency(value: Option<f64>) -> String {
    match value {
        Some(value) if value < 1.0 => format!("{:.0} us", value * 1000.0),
        Some(value) if value < 100.0 => format!("{value:.2} ms"),
        Some(value) => format!("{value:.1} ms"),
        None => "N/A".to_owned(),
    }
}

fn format_temperature_value(value: Option<f32>) -> String {
    value
        .map(|value| format!("{value:.0} C"))
        .unwrap_or_else(|| "N/A".to_owned())
}

fn format_utilization_value(value: Option<f32>) -> String {
    value
        .map(|value| format!("{value:.0}%"))
        .unwrap_or_else(|| "N/A".to_owned())
}

fn format_temperature_summary(summary: &TemperatureSummary) -> String {
    if !summary.has_any_value() {
        return "N/A".to_owned();
    }
    format!(
        "{} -> {} (max {})",
        format_temperature_value(summary.start_c),
        format_temperature_value(summary.end_c),
        format_temperature_value(summary.max_c)
    )
}

fn format_temperature_run_report(report: &TemperatureRunReport) -> String {
    let mut parts = Vec::new();
    if report.scope == TemperatureScope::Matrix {
        parts.push(format!("CPU {}", format_temperature_summary(&report.cpu)));
        parts.push(format!("GPU {}", format_temperature_summary(&report.gpu)));
    } else {
        parts.push(format!("SSD {}", format_temperature_summary(&report.drive)));
    }
    parts.join(", ")
}

fn temperature_color(kind: SensorKind, value: Option<f32>, status: &SensorStatus) -> egui::Color32 {
    if !matches!(status, SensorStatus::Ok) {
        return egui::Color32::GRAY;
    }
    match value {
        Some(value) if value >= kind.critical_c() => egui::Color32::RED,
        Some(value) if value >= kind.warning_c() => egui::Color32::YELLOW,
        Some(_) => egui::Color32::WHITE,
        None => egui::Color32::GRAY,
    }
}

fn ui_sensor_row(ui: &mut egui::Ui, label: &str, reading: Option<&SensorReading>) {
    let (temperature, utilization, temperature_color, utilization_color, tooltip) =
        if let Some(reading) = reading {
            let (temperature, utilization) = if reading.status == SensorStatus::Stale {
                ("-- C".to_owned(), "--%".to_owned())
            } else {
                (
                    format_temperature_value(reading.temperature_c),
                    format_utilization_value(reading.utilization_percent),
                )
            };
            let tooltip = format!(
                "{}\nProvider: {}\nStatus: {}\nUtilization: {}",
                reading.label,
                reading.provider,
                reading.status.detail(),
                format_utilization_value(reading.utilization_percent)
            );
            (
                temperature,
                utilization,
                temperature_color(reading.kind, reading.temperature_c, &reading.status),
                if reading.utilization_percent.is_some() {
                    egui::Color32::WHITE
                } else {
                    egui::Color32::GRAY
                },
                tooltip,
            )
        } else {
            (
                "N/A".to_owned(),
                "N/A".to_owned(),
                egui::Color32::GRAY,
                egui::Color32::GRAY,
                "No sensor provider initialized".to_owned(),
            )
        };

    ui.horizontal(|ui| {
        ui.set_min_width(188.0);
        ui.label(egui::RichText::new(label).monospace());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(utilization)
                    .monospace()
                    .color(utilization_color),
            )
            .on_hover_text(tooltip.clone());
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(temperature)
                    .monospace()
                    .color(temperature_color),
            )
            .on_hover_text(tooltip);
        });
    });
}

fn ui_large_back_button(ui: &mut egui::Ui) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new("Back").size(20.0).strong())
            .min_size(egui::vec2(104.0, 42.0)),
    )
}

fn panel_content_log_heights(available_height: f32, log_fraction: f32, log_max: f32) -> (f32, f32) {
    let fixed_height = SENSOR_BOX_RESERVED_HEIGHT + PANEL_VERTICAL_CHROME_HEIGHT;
    let usable_height = (available_height - fixed_height).max(0.0);
    if usable_height <= MIN_CONTENT_HEIGHT + MIN_LOG_HEIGHT {
        let log_height = (usable_height * 0.34)
            .clamp(40.0, MIN_LOG_HEIGHT)
            .min(usable_height * 0.5);
        return ((usable_height - log_height).max(40.0), log_height.max(32.0));
    }

    let log_height = (available_height * log_fraction)
        .clamp(MIN_LOG_HEIGHT, log_max)
        .min(usable_height - MIN_CONTENT_HEIGHT);
    (usable_height - log_height, log_height)
}

fn ui_log_line(ui: &mut egui::Ui, line: &str) {
    ui.label(egui::RichText::new(line).monospace().size(LOG_TEXT_SIZE));
}

fn result_header(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .strong()
            .size(RESULT_HEADER_TEXT_SIZE),
    );
}

fn result_cell(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(egui::RichText::new(text.into()).size(RESULT_CELL_TEXT_SIZE));
}

fn device_type_label(value: wgpu::DeviceType) -> &'static str {
    match value {
        wgpu::DeviceType::IntegratedGpu => "Integrated GPU",
        wgpu::DeviceType::DiscreteGpu => "Discrete GPU",
        wgpu::DeviceType::VirtualGpu => "Virtual GPU",
        wgpu::DeviceType::Cpu => "CPU/Software",
        wgpu::DeviceType::Other => "Other GPU",
    }
}

fn adapter_uses_shared_cpu_temperature(adapter: &AdapterInfo) -> bool {
    if adapter.device_type == wgpu::DeviceType::IntegratedGpu {
        return true;
    }

    let name = adapter.name.to_ascii_lowercase();
    adapter.vendor == 0x8086
        && (name.contains("xe")
            || name.contains("iris")
            || name.contains("uhd")
            || name.contains("integrated")
            || name.contains("graphics"))
        || adapter.vendor == 0x1002
            && (name.contains("radeon graphics")
                || name.contains("vega")
                || name.contains("apu")
                || name.contains("integrated"))
}

fn configure_ui_style(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    style
        .text_styles
        .insert(egui::TextStyle::Heading, egui::FontId::proportional(26.0));
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(17.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(17.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, egui::FontId::monospace(16.0));
    style.spacing.button_padding = egui::vec2(14.0, 9.0);
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.interact_size = egui::vec2(44.0, 34.0);
    ctx.set_global_style(style);
}

fn empty_to_unknown(value: &str) -> &str {
    if value.is_empty() { "unknown" } else { value }
}

fn run_cli(args: &[String]) -> Result<bool> {
    if args.iter().any(|arg| arg == "--list-gpus") {
        let adapters = enumerate_adapters();
        if adapters.is_empty() {
            println!("No wgpu adapters found.");
        }
        for adapter in adapters {
            println!(
                "[{}] {} | vendor {:04X} device {:04X} | driver {} | timestamp {} | memory {}",
                adapter.index,
                adapter.label(),
                adapter.vendor,
                adapter.device,
                empty_to_unknown(&adapter.driver),
                if adapter.timestamp_query { "yes" } else { "no" },
                adapter_memory_limit_bytes(&adapter)
                    .map(|(bytes, label)| format!("{} {}", format_bytes(bytes), label))
                    .unwrap_or_else(|| "unknown".to_owned())
            );
        }
        return Ok(true);
    }

    if args.iter().any(|arg| arg == "--self-test") {
        let size = arg_value(args, "--size")
            .map(|value| value.parse::<usize>())
            .transpose()
            .context("--size must be an integer")?
            .unwrap_or(64);
        let adapter_index = arg_value(args, "--adapter")
            .map(|value| value.parse::<usize>())
            .transpose()
            .context("--adapter must be an integer")?;
        let estimate_cpu_time = args.iter().any(|arg| arg == "--estimate-cpu");
        let gpu_intensity = arg_value(args, "--gpu-intensity")
            .as_deref()
            .map(parse_gpu_intensity)
            .transpose()?
            .unwrap_or(GpuIntensity::Safe);
        let adapters = enumerate_adapters();
        let adapter = if let Some(index) = adapter_index {
            adapters
                .into_iter()
                .find(|adapter| adapter.index == index)
                .ok_or_else(|| anyhow!("adapter index {index} was not found"))?
        } else {
            adapters
                .into_iter()
                .find(|adapter| adapter.device_type != wgpu::DeviceType::Cpu)
                .ok_or_else(|| anyhow!("no hardware GPU adapter was found"))?
        };

        println!(
            "Running self-test on {} with {} GPU intensity",
            adapter.label(),
            gpu_intensity
        );
        let result = run_single(size, adapter, true, estimate_cpu_time, gpu_intensity)?;
        println!("Size: {}x{}", result.size, result.size);
        println!("CPU: {} ms ({})", format_cpu_ms(&result), result.cpu_model);
        println!("GPU compute: {} ms", format_ms(result.gpu_compute_ms));
        println!("GPU total: {} ms", format_ms(Some(result.gpu_total_ms)));
        println!("Transfer/sync: {} ms", format_ms(result.transfer_sync_ms));
        println!("GPU path: {}", result.gpu_path);
        println!("GPU intensity: {}", result.gpu_intensity);
        println!("Dispatches: {}", result.dispatch_count);
        println!("Tile/panel: {}", result.tile_shape);
        println!("Last dispatch: {} ms", format_ms(result.last_dispatch_ms));
        println!("Avg dispatch: {} ms", format_ms(result.avg_dispatch_ms));
        println!("Max dispatch: {} ms", format_ms(result.max_dispatch_ms));
        println!("Backoffs: {}", result.backoff_count);
        println!("Speedup: {}", format_speedup(result.speedup));
        println!("Validation: {}", result.validation);
        return Ok(true);
    }

    Ok(false)
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn parse_gpu_intensity(value: &str) -> Result<GpuIntensity> {
    GpuIntensity::parse(value).ok_or_else(|| {
        anyhow!("--gpu-intensity must be one of safe, balanced, or high (got {value})")
    })
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run_cli(&args) {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(err) => {
            eprintln!("{err:#}");
            std::process::exit(1);
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1220.0, 760.0]),
        ..Default::default()
    };
    eframe::run_native(
        "BenchScope",
        options,
        Box::new(|cc| Ok(Box::new(BenchScopeApp::new(cc)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_matrices_are_deterministic() {
        let (a1, b1) = generate_matrices(4).unwrap();
        let (a2, b2) = generate_matrices(4).unwrap();
        assert_eq!(a1, a2);
        assert_eq!(b1, b2);
    }

    #[test]
    fn cpu_multiply_known_values() {
        let a = vec![1.0, 2.0, 3.0, 4.0];
        let b = vec![5.0, 6.0, 7.0, 8.0];
        let (c, _) = cpu_multiply(2, &a, &b);
        assert_eq!(c, vec![19.0, 22.0, 43.0, 50.0]);
    }

    #[test]
    fn validation_accepts_close_values() {
        let cpu = vec![1.0, 2.0, 3.0, 4.0];
        let gpu = vec![1.0, 2.00001, 3.0, 4.0];
        assert!(validate(&cpu, &gpu, 2).starts_with("Passed"));
    }

    #[test]
    fn sampled_validation_accepts_exact_output() {
        let (a, b) = generate_matrices(4).unwrap();
        let (c, _) = cpu_multiply(4, &a, &b);

        assert!(
            validate_sampled(&a, &b, &c, 4, None)
                .unwrap()
                .starts_with("Sampled pass")
        );
    }

    #[test]
    fn cpu_estimate_honors_cancellation() {
        let (a, b) = generate_matrices(4).unwrap();
        let cancel = AtomicBool::new(true);
        let cpu_info = CpuInfo {
            model: "Test CPU".to_owned(),
            logical_processors: 8,
        };

        let err = estimate_cpu_multiply_ms(4, &a, &b, &cpu_info, Some(&cancel), None).unwrap_err();

        assert!(err.to_string().contains("canceled"));
    }

    #[test]
    fn cpu_estimate_sample_size_uses_cpu_class() {
        let high_end = CpuInfo {
            model: "AMD Ryzen 9 7950X".to_owned(),
            logical_processors: 32,
        };
        let mid = CpuInfo {
            model: "13th Gen Intel(R) Core(TM) i7-1360P".to_owned(),
            logical_processors: 16,
        };
        let base = CpuInfo {
            model: "Unknown CPU".to_owned(),
            logical_processors: 8,
        };

        assert_eq!(cpu_estimate_sample_size(4096, &high_end), 1024);
        assert_eq!(cpu_estimate_sample_size(4096, &mid), 768);
        assert_eq!(cpu_estimate_sample_size(4096, &base), 512);
        assert_eq!(cpu_estimate_sample_size(64, &high_end), 64);
        assert_eq!(cpu_estimate_sample_size(4, &high_end), 4);
    }

    #[test]
    fn cpu_estimate_row_sample_uses_full_width_rows() {
        let high_end = CpuInfo {
            model: "AMD Ryzen 9 7950X".to_owned(),
            logical_processors: 32,
        };
        let mid = CpuInfo {
            model: "13th Gen Intel(R) Core(TM) i7-1360P".to_owned(),
            logical_processors: 16,
        };
        let base = CpuInfo {
            model: "Unknown CPU".to_owned(),
            logical_processors: 8,
        };

        assert_eq!(cpu_estimate_row_sample_count(512, &mid), 512);
        assert!(cpu_estimate_row_sample_count(4096, &high_end) >= 32);
        assert!(cpu_estimate_row_sample_count(4096, &mid) >= 16);
        assert!(cpu_estimate_row_sample_count(4096, &base) >= 8);
        assert!(cpu_estimate_row_sample_count(16_384, &mid) < 64);
    }

    #[test]
    fn cpu_row_sample_honors_cancellation() {
        let (a, b) = generate_matrices(4).unwrap();
        let cancel = AtomicBool::new(true);

        let err = cpu_multiply_row_sample_cancelable(4, &a, &b, 0, 2, Some(&cancel)).unwrap_err();

        assert!(err.to_string().contains("canceled"));
    }

    #[test]
    fn top_left_submatrix_copy_keeps_layout() {
        let source = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0,
        ];

        assert_eq!(
            copy_top_left_submatrix(&source, 4, 2, None).unwrap(),
            vec![1.0, 2.0, 5.0, 6.0]
        );
    }

    #[test]
    fn estimated_cpu_format_is_marked() {
        let result = BenchmarkResult {
            size: 4096,
            adapter: "Test GPU".to_owned(),
            cpu_model: "Test CPU (8 logical processors)".to_owned(),
            cpu_ms: 1234.0,
            cpu_estimated: true,
            gpu_compute_ms: Some(10.0),
            gpu_total_ms: 12.0,
            transfer_sync_ms: Some(2.0),
            gpu_path: GpuPath::DirectFullBuffer,
            gpu_intensity: GpuIntensity::Safe,
            dispatch_count: 1,
            tile_shape: "4x4".to_owned(),
            last_dispatch_ms: Some(10.0),
            avg_dispatch_ms: Some(10.0),
            max_dispatch_ms: Some(10.0),
            backoff_count: 0,
            speedup: 102.83,
            validation: "Skipped".to_owned(),
            cpu_temperature: TemperatureSummary::default(),
            gpu_temperature: TemperatureSummary::default(),
        };

        assert_eq!(format_cpu_ms(&result), "Est. 1234.0");
    }

    #[test]
    fn cpu_info_has_model_and_parallelism() {
        let cpu_info = detect_cpu_info();

        assert!(!cpu_info.model.is_empty());
        assert!(cpu_info.logical_processors >= 1);
    }

    #[test]
    fn gpu_working_set_counts_four_matrices() {
        assert_eq!(gpu_working_set_bytes(16_384), Some(4 * 1024 * 1024 * 1024));
    }

    #[test]
    fn gpu_chunking_is_adaptive() {
        assert_eq!(gpu_dispatch_chunk_rows(128, GpuIntensity::Safe), 128);
        assert_eq!(
            gpu_dispatch_chunk_rows(2048, GpuIntensity::Safe),
            GPU_SAFE_CHUNK_ROWS
        );
        assert_eq!(
            gpu_dispatch_chunk_rows(4096, GpuIntensity::Balanced),
            GPU_BALANCED_CHUNK_ROWS
        );
        assert!(
            gpu_dispatch_chunk_rows(4096, GpuIntensity::High)
                > gpu_dispatch_chunk_rows(4096, GpuIntensity::Safe)
        );
    }

    #[test]
    fn gpu_intensity_changes_block_targets() {
        let safe = gpu_block_targets(GpuIntensity::Safe);
        let balanced = gpu_block_targets(GpuIntensity::Balanced);
        let high = gpu_block_targets(GpuIntensity::High);

        assert!(safe.0 < balanced.0);
        assert!(balanced.0 < high.0);
        assert!(safe.1 < balanced.1);
        assert!(balanced.1 < high.1);
        assert!(
            gpu_submission_pause(GpuIntensity::Safe) > gpu_submission_pause(GpuIntensity::High)
        );
    }

    #[test]
    fn gpu_intensity_parser_accepts_aliases() {
        assert_eq!(parse_gpu_intensity("safe").unwrap(), GpuIntensity::Safe);
        assert_eq!(parse_gpu_intensity("maximum").unwrap(), GpuIntensity::High);
        assert!(parse_gpu_intensity("danger").is_err());
    }

    #[test]
    fn cpu_worker_count_leaves_room_for_system() {
        assert_eq!(cpu_worker_count(64), 1);
        let available = thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1);
        assert!(cpu_worker_count(4096) <= available);
        if available > 1 {
            assert!(cpu_worker_count(4096) < available);
        }
    }

    #[test]
    fn gpu_eta_uses_real_progress_after_gpu_starts() {
        let adapter = AdapterInfo {
            index: 0,
            name: "Test GPU".to_owned(),
            backend: wgpu::Backend::Dx12,
            device_type: wgpu::DeviceType::DiscreteGpu,
            vendor: 0,
            device: 0,
            driver: String::new(),
            timestamp_query: true,
            dedicated_vram_bytes: None,
            dedicated_system_memory_bytes: None,
            shared_system_memory_bytes: None,
        };
        let mut tracker = SingleProgressTracker::new(8192, &adapter, GpuIntensity::Safe, None);
        tracker.cpu_progress = 1.0;
        tracker.gpu_progress = 0.25;
        tracker.gpu_estimate_s = 0.1;
        tracker.gpu_started = Some(Instant::now() - Duration::from_secs(4));

        let eta = tracker.eta_s().unwrap();

        assert!((11.5..=12.5).contains(&eta));
    }

    #[test]
    fn blocked_packers_keep_expected_layout() {
        let source = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0,
        ];

        assert_eq!(
            pack_row_block(&source, 4, 1, 2, None).unwrap(),
            vec![5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0]
        );
        assert_eq!(
            pack_column_block(&source, 4, 1, 2, None).unwrap(),
            vec![2.0, 3.0, 6.0, 7.0, 10.0, 11.0, 14.0, 15.0]
        );
    }

    #[test]
    fn column_panel_pack_unpack_round_trips() {
        let source = (0..16).map(|value| value as f32).collect::<Vec<_>>();

        let (packed, panels) = pack_column_panels(&source, 4, 2, None).unwrap();

        assert_eq!(panels.len(), 2);
        assert_eq!(
            packed,
            vec![
                0.0, 1.0, 4.0, 5.0, 8.0, 9.0, 12.0, 13.0, 2.0, 3.0, 6.0, 7.0, 10.0, 11.0, 14.0,
                15.0
            ]
        );
        assert_eq!(
            unpack_column_panels(&packed, 4, &panels, None).unwrap(),
            source
        );
    }

    #[test]
    fn dispatch_stats_uses_available_dispatch_timings() {
        let stats =
            GpuDispatchStats::new(GpuPath::PersistentPanelized, "32x512", &[2.0, 4.0, 6.0], 1);

        assert_eq!(stats.dispatch_count, 3);
        assert_eq!(stats.avg_dispatch_ms, Some(4.0));
        assert_eq!(stats.max_dispatch_ms, Some(6.0));
        assert_eq!(stats.last_dispatch_ms, Some(6.0));
        assert_eq!(stats.backoff_count, 1);
    }

    #[test]
    fn timestamp_query_plan_respects_wgpu_limit() {
        assert_eq!(timestamp_query_plan(2048), Some((4096, 32_768)));
        assert_eq!(timestamp_query_plan(2049), None);
    }

    #[test]
    fn block_extent_alignment_keeps_nonzero_small_values() {
        assert_eq!(align_block_extent(1), 1);
        assert_eq!(align_block_extent(15), 15);
        assert_eq!(align_block_extent(16), 16);
        assert_eq!(align_block_extent(31), 16);
        assert_eq!(align_block_extent(1025), 1024);
    }

    #[test]
    fn integrated_memory_limit_includes_shared_memory() {
        let adapter = AdapterInfo {
            index: 0,
            name: "Integrated Test GPU".to_owned(),
            backend: wgpu::Backend::Dx12,
            device_type: wgpu::DeviceType::IntegratedGpu,
            vendor: 0,
            device: 0,
            driver: String::new(),
            timestamp_query: true,
            dedicated_vram_bytes: Some(128 * 1024 * 1024),
            dedicated_system_memory_bytes: Some(0),
            shared_system_memory_bytes: Some(8 * 1024 * 1024 * 1024),
        };

        assert_eq!(
            adapter_memory_limit_bytes(&adapter),
            Some((8_724_152_320, "reported GPU/shared memory"))
        );
    }

    #[test]
    fn drive_auto_file_size_tracks_profile() {
        assert_eq!(auto_drive_file_size(DriveProfile::Quick), 256 * 1024 * 1024);
        assert_eq!(
            auto_drive_file_size(DriveProfile::Balanced),
            512 * 1024 * 1024
        );
        assert_eq!(
            auto_drive_file_size(DriveProfile::Thorough),
            1024 * 1024 * 1024
        );
    }

    #[test]
    fn drive_profiles_stay_under_hard_cap() {
        for profile in DriveProfile::ALL {
            assert!(profile.target_duration().as_secs_f64() < DRIVE_MAX_TEST_SECONDS);
        }
    }

    #[test]
    fn drive_latency_percentile_uses_sorted_samples() {
        let samples = vec![5.0, 1.0, 3.0, 2.0, 4.0];

        assert_eq!(percentile_latency_ms(samples, 0.95), Some(5.0));
    }

    #[test]
    fn drive_result_calculates_random_iops_and_rate() {
        let result = make_drive_result(
            DriveTestKind::RandomRead4K,
            4096 * 1000,
            1000,
            Duration::from_secs(2),
            256 * 1024 * 1024,
            DriveIoMode::Direct,
            vec![0.1, 0.2, 0.3],
            vec!["test note".to_owned()],
        );

        assert_eq!(result.read_mbps, Some(2.048));
        assert_eq!(result.write_mbps, None);
        assert_eq!(result.iops, Some(500.0));
        assert_eq!(result.p95_latency_ms, Some(0.3));
        assert_eq!(result.io_mode, DriveIoMode::Direct);
        assert_eq!(format_drive_speed(&result), "2.05");
        assert_eq!(result.ssd_temperature, TemperatureSummary::default());
    }

    #[test]
    fn drive_buffer_fill_is_deterministic_and_nonzero() {
        let mut a = vec![0_u8; 64];
        let mut b = vec![0_u8; 64];

        fill_drive_buffer(&mut a, 42);
        fill_drive_buffer(&mut b, 42);

        assert_eq!(a, b);
        assert!(a.iter().any(|byte| *byte != 0));
    }

    #[cfg(windows)]
    #[test]
    fn drive_root_parser_finds_windows_drive() {
        let root = drive_root_for_path(&PathBuf::from("c:\\Users\\test")).unwrap();

        assert_eq!(root.display().to_string(), "C:\\");
        assert!(same_drive_root(
            &PathBuf::from("c:\\"),
            &PathBuf::from("C:\\")
        ));
    }

    #[test]
    fn aligned_drive_buffer_meets_direct_io_alignment() {
        let mut buffer = AlignedBuffer::new(DRIVE_RANDOM_BLOCK_BYTES, DRIVE_RANDOM_BLOCK_BYTES);
        let ptr = buffer.as_mut_slice().as_ptr() as usize;

        assert_eq!(ptr % DRIVE_RANDOM_BLOCK_BYTES, 0);
        assert_eq!(buffer.len(), DRIVE_RANDOM_BLOCK_BYTES);
    }

    #[test]
    fn temperature_summary_tracks_start_end_and_max() {
        let mut summary = TemperatureSummary::begin(Some(42.0));

        summary.observe(Some(55.0));
        summary.observe(Some(51.0));
        summary.finish(Some(49.0));

        assert_eq!(summary.start_c, Some(42.0));
        assert_eq!(summary.end_c, Some(49.0));
        assert_eq!(summary.max_c, Some(55.0));
        assert_eq!(
            format_temperature_summary(&summary),
            "42 C -> 49 C (max 55 C)"
        );
    }

    #[test]
    fn temperature_parser_finds_first_plausible_value() {
        assert_eq!(parse_first_temperature("Temperature\n64 C\n"), Some(64.0));
        assert_eq!(parse_first_temperature("GPU 72, CPU 66"), Some(72.0));
        assert_eq!(parse_first_temperature("999"), None);
    }

    #[test]
    fn temperature_color_uses_thresholds() {
        assert_eq!(
            temperature_color(SensorKind::Gpu, Some(91.0), &SensorStatus::Ok),
            egui::Color32::RED
        );
        assert_eq!(
            temperature_color(SensorKind::Drive, Some(62.0), &SensorStatus::Ok),
            egui::Color32::YELLOW
        );
        assert_eq!(
            temperature_color(SensorKind::Cpu, None, &SensorStatus::Unsupported),
            egui::Color32::GRAY
        );
    }

    #[test]
    fn helper_snapshot_parser_reads_temperatures() {
        let line = r#"{"timestampUtc":"2026-05-16T03:36:28Z","isElevated":false,"cpu":{"label":"CPU Package","temperatureC":63.5,"provider":"LibreHardwareMonitor","status":"ok","utilizationPercent":12.5},"gpu":{"label":"GPU Core","temperatureC":57.0,"provider":"LibreHardwareMonitor","status":"ok","utilizationPercent":84.0},"drive":{"label":"NVMe SSD","temperatureC":41.0,"provider":"LibreHardwareMonitor","status":"ok","utilizationPercent":6.0},"diagnostics":[]}"#;

        let snapshot = parse_helper_snapshot(line).unwrap();

        assert_eq!(sensor_temperature(snapshot.cpu.as_ref()), Some(63.5));
        assert_eq!(sensor_temperature(snapshot.gpu.as_ref()), Some(57.0));
        assert_eq!(sensor_temperature(snapshot.drive.as_ref()), Some(41.0));
        assert_eq!(
            snapshot
                .gpu
                .as_ref()
                .and_then(|reading| reading.utilization_percent),
            Some(84.0)
        );
        assert_eq!(snapshot.helper_elevated, Some(false));
        assert_eq!(snapshot.cpu.unwrap().label, "CPU Package");
    }

    #[test]
    fn helper_snapshot_parser_preserves_unsupported_status() {
        let line = r#"{"timestampUtc":"2026-05-16T03:36:28Z","cpu":{"label":"CPU","provider":"LibreHardwareMonitor","status":"unsupported","message":"No CPU temperature sensor found"}}"#;

        let snapshot = parse_helper_snapshot(line).unwrap();
        let cpu = snapshot.cpu.unwrap();

        assert_eq!(cpu.temperature_c, None);
        assert_eq!(
            cpu.status,
            SensorStatus::Error("No CPU temperature sensor found".to_owned())
        );
    }

    #[test]
    fn helper_snapshot_requests_elevation_when_non_elevated_sensor_is_hidden() {
        let line = r#"{"timestampUtc":"2026-05-16T03:36:28Z","isElevated":false,"cpu":{"label":"CPU","provider":"LibreHardwareMonitor","status":"unsupported","message":"No CPU temperature sensor found"}}"#;
        let snapshot = parse_helper_snapshot(line).unwrap();

        assert!(helper_snapshot_needs_elevation(&snapshot));
    }

    #[test]
    fn merge_sensor_snapshots_uses_fallback_for_missing_helper_reading() {
        let helper = parse_helper_snapshot(
            r#"{"timestampUtc":"2026-05-16T03:36:28Z","isElevated":true,"gpu":{"label":"GPU","provider":"LibreHardwareMonitor","status":"unsupported","message":"No GPU temperature sensor found"}}"#,
        )
        .unwrap();
        let fallback = SensorSnapshot {
            cpu: None,
            gpu: Some(SensorReading::ok(
                SensorKind::Gpu,
                "GPU",
                61.0,
                "NVML/nvidia-smi",
            )),
            drive: None,
            helper_elevated: None,
        };

        let merged = merge_sensor_snapshots(Some(helper), Some(fallback)).unwrap();

        assert_eq!(sensor_temperature(merged.gpu.as_ref()), Some(61.0));
        assert!(merged.gpu.unwrap().provider.contains("NVML/nvidia-smi"));
        assert_eq!(merged.helper_elevated, Some(true));
    }

    #[test]
    fn integrated_gpu_fallback_uses_cpu_package_temperature() {
        let snapshot = SensorSnapshot {
            cpu: Some(SensorReading::ok(
                SensorKind::Cpu,
                "CPU Package",
                54.0,
                "LibreHardwareMonitor",
            )),
            gpu: Some(SensorReading {
                kind: SensorKind::Gpu,
                label: "Intel Xe Graphics".to_owned(),
                temperature_c: None,
                utilization_percent: Some(33.0),
                provider: "LibreHardwareMonitor".to_owned(),
                updated_at: Instant::now(),
                status: SensorStatus::Ok,
            }),
            drive: None,
            helper_elevated: Some(true),
        };

        let snapshot = apply_integrated_gpu_temperature_fallback(snapshot, true);
        let gpu = snapshot.gpu.unwrap();

        assert_eq!(gpu.temperature_c, Some(54.0));
        assert_eq!(gpu.utilization_percent, Some(33.0));
        assert_eq!(gpu.label, "iGPU shared CPU package");
    }

    #[test]
    fn panel_height_split_keeps_sensor_space_visible() {
        for available in [320.0, 480.0, 760.0] {
            let (content, log) = panel_content_log_heights(available, 0.18, 150.0);
            assert!(content >= 40.0);
            assert!(log >= 32.0);
            assert!(
                content + log + SENSOR_BOX_RESERVED_HEIGHT + PANEL_VERTICAL_CHROME_HEIGHT
                    <= available + 0.1
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn drive_device_name_parser_reads_tab_separated_names() {
        let names =
            parse_drive_device_names("C\tSamsung SSD 990 PRO 2TB\r\nD\tWD_BLACK SN850X\r\n");

        assert_eq!(
            names.get(&'C').map(String::as_str),
            Some("Samsung SSD 990 PRO 2TB")
        );
        assert_eq!(names.get(&'D').map(String::as_str), Some("WD_BLACK SN850X"));
    }
}
