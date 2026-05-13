use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

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
const CPU_ESTIMATE_MID_SAMPLE_SIZE: usize = 768;
const CPU_ESTIMATE_MAX_SAMPLE_SIZE: usize = 1024;
const VALIDATION_SAMPLE_POINTS: usize = 256;
const GPU_CANCELABLE_CHUNK_ROWS: usize = 16;
const GPU_BLOCKED_ROW_TARGET: usize = 16;
const GPU_BLOCKED_COL_TARGET: usize = 1024;
const GPU_WAIT_POLL_MS: u64 = 1;
const FORCE_BLOCKED_GPU_ENV: &str = "HARDWARE_ACCEL_TEST_FORCE_BLOCKED_GPU";

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
    speedup: f64,
    validation: String,
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
    output: Vec<f32>,
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
    fn new(size: usize, adapter: &AdapterInfo, tx: Option<Sender<WorkerEvent>>) -> Self {
        Self {
            tx,
            started: Instant::now(),
            last_emit: Instant::now() - Duration::from_secs(1),
            cpu_progress: 0.0,
            gpu_progress: 0.0,
            phase: "Preparing benchmark".to_owned(),
            gpu_estimate_s: estimate_gpu_seconds(size, adapter),
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

    fn start_gpu_ticker(&mut self) -> Option<ProgressTicker> {
        self.gpu_started = Some(Instant::now());
        self.set_phase("GPU computing and readback", true);
        let tx = self.tx.clone()?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let started = self.started;
        let gpu_started = self.gpu_started?;
        let cpu_progress = self.cpu_progress;
        let gpu_estimate_s = self.gpu_estimate_s;
        let handle = thread::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(PROGRESS_SAMPLE_MS));
                if worker_stop.load(Ordering::Relaxed) {
                    break;
                }
                let gpu_elapsed_s = gpu_started.elapsed().as_secs_f64();
                let gpu_progress = (gpu_elapsed_s / gpu_estimate_s).clamp(0.0, 0.95) as f32;
                let eta_s = Some((gpu_estimate_s - gpu_elapsed_s).max(0.0));
                let _ = tx.send(WorkerEvent::SingleProgress(SingleProgress {
                    cpu_progress,
                    gpu_progress,
                    elapsed_s: started.elapsed().as_secs_f64(),
                    eta_s,
                    phase: "GPU computing and readback".to_owned(),
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
            self.gpu_started
                .map(|started| (self.gpu_estimate_s - started.elapsed().as_secs_f64()).max(0.0))
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
        descriptor.label = Some("Hardware Acceleration Tester device");
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
        self.multiply_cancelable(n, a, b, use_timestamps, None)
    }

    fn multiply_cancelable(
        &self,
        n: usize,
        a: &[f32],
        b: &[f32],
        use_timestamps: bool,
        cancel: Option<&AtomicBool>,
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
            return self.multiply_blocked(n, n_u32, a, b, cancel);
        }

        let total_start = Instant::now();

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

        let chunk_rows = gpu_dispatch_chunk_rows(n);
        let chunk_count = n.div_ceil(chunk_rows);
        let timestamp_enabled = self.timestamp_query && use_timestamps;
        let timestamp_query_count = (chunk_count * 2) as u32;
        let timestamp_buffer_size = (timestamp_query_count as u64) * 8;
        let query_set = timestamp_enabled.then(|| {
            self.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("GPU compute timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: timestamp_query_count,
            })
        });
        let timestamp_resolve = timestamp_enabled.then(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Timestamp resolve"),
                size: timestamp_buffer_size,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        });
        let timestamp_readback = timestamp_enabled.then(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Timestamp readback"),
                size: timestamp_buffer_size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });

        for (chunk_index, row_offset) in (0..n).step_by(chunk_rows).enumerate() {
            self.check_gpu_canceled(cancel)?;
            let rows_this_chunk = (n - row_offset).min(chunk_rows);
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

            let submission = self.queue.submit([encoder.finish()]);
            self.wait_for_submission(submission, cancel, "waiting for GPU matrix chunk to finish")?;
        }

        self.check_gpu_canceled(cancel)?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Matrix readback encoder"),
            });
        if let (Some(query_set), Some(resolve), Some(readback)) =
            (&query_set, &timestamp_resolve, &timestamp_readback)
        {
            encoder.resolve_query_set(query_set, 0..timestamp_query_count, resolve, 0);
            encoder.copy_buffer_to_buffer(resolve, 0, readback, 0, timestamp_buffer_size);
        }
        encoder.copy_buffer_to_buffer(&c_buffer, 0, &readback_buffer, 0, byte_len);

        let submission = self.queue.submit([encoder.finish()]);
        self.wait_for_submission(submission, cancel, "waiting for GPU readback copy")?;

        let output = read_f32_buffer_cancelable(&self.device, &readback_buffer, elements, cancel)
            .context("reading GPU result buffer")?;
        let compute_ms = if let Some(readback) = &timestamp_readback {
            read_timestamps(&self.device, readback, chunk_count, cancel)
                .ok()
                .map(|timestamps| {
                    let timestamp_period = self.queue.get_timestamp_period() as f64;
                    timestamps
                        .into_iter()
                        .map(|[start, end]| {
                            let delta = end.saturating_sub(start);
                            (delta as f64 * timestamp_period) / 1_000_000.0
                        })
                        .sum::<f64>()
                })
        } else {
            None
        };

        let total_ms = total_start.elapsed().as_secs_f64() * 1000.0;
        let transfer_sync_ms = compute_ms.map(|ms| (total_ms - ms).max(0.0));

        Ok(GpuTiming {
            compute_ms,
            total_ms,
            transfer_sync_ms,
            output,
        })
    }

    fn multiply_blocked(
        &self,
        n: usize,
        n_u32: u32,
        a: &[f32],
        b: &[f32],
        cancel: Option<&AtomicBool>,
    ) -> Result<GpuTiming> {
        let elements = n
            .checked_mul(n)
            .ok_or_else(|| anyhow!("matrix size overflow"))?;
        let total_start = Instant::now();
        let (row_block, col_block) = self.block_dimensions(n)?;
        let mut output = vec![0.0_f32; elements];

        for col_offset in (0..n).step_by(col_block) {
            self.check_gpu_canceled(cancel)?;
            let cols = (n - col_offset).min(col_block);
            let b_block = pack_column_block(b, n, col_offset, cols, cancel)?;

            for row_offset in (0..n).step_by(row_block) {
                self.check_gpu_canceled(cancel)?;
                let rows = (n - row_offset).min(row_block);
                let a_block = pack_row_block(a, n, row_offset, rows, cancel)?;
                let c_block = self.multiply_block(n_u32, rows, cols, &a_block, &b_block, cancel)?;

                for row in 0..rows {
                    if row % 8 == 0 {
                        check_canceled(cancel)?;
                    }
                    let output_start = (row_offset + row) * n + col_offset;
                    let block_start = row * cols;
                    output[output_start..output_start + cols]
                        .copy_from_slice(&c_block[block_start..block_start + cols]);
                }
            }
        }

        Ok(GpuTiming {
            compute_ms: None,
            total_ms: total_start.elapsed().as_secs_f64() * 1000.0,
            transfer_sync_ms: None,
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
        cancel: Option<&AtomicBool>,
    ) -> Result<Vec<f32>> {
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

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Blocked matrix multiplication encoder"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Blocked matrix multiplication pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.blocked_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(
                cols_u32.div_ceil(TILE_SIZE),
                rows_u32.div_ceil(TILE_SIZE),
                1,
            );
        }
        encoder.copy_buffer_to_buffer(&c_buffer, 0, &readback_buffer, 0, c_bytes);

        let submission = self.queue.submit([encoder.finish()]);
        self.wait_for_submission(submission, cancel, "waiting for blocked GPU matrix chunk")?;
        read_f32_buffer_cancelable(&self.device, &readback_buffer, c_elements, cancel)
            .context("reading blocked GPU result buffer")
    }

    fn needs_blocked_path(&self, matrix_byte_len: u64) -> bool {
        std::env::var_os(FORCE_BLOCKED_GPU_ENV).is_some()
            || matrix_byte_len > self.max_storage_buffer_binding_size
            || matrix_byte_len > self.max_buffer_size
    }

    fn block_dimensions(&self, n: usize) -> Result<(usize, usize)> {
        let limit_bytes = self
            .max_storage_buffer_binding_size
            .min(self.max_buffer_size)
            .max(std::mem::size_of::<f32>() as u64);
        let limit_floats = (limit_bytes / std::mem::size_of::<f32>() as u64) as usize;
        let max_rows_or_cols = (limit_floats / n).max(1);
        let rows = align_block_extent(GPU_BLOCKED_ROW_TARGET.min(max_rows_or_cols));
        let cols = align_block_extent(GPU_BLOCKED_COL_TARGET.min(max_rows_or_cols));

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

fn gpu_dispatch_chunk_rows(size: usize) -> usize {
    if size <= 1024 {
        size.max(1)
    } else if size <= 2048 {
        64
    } else {
        GPU_CANCELABLE_CHUNK_ROWS.min(size).max(1)
    }
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
    let total_blocks = (blocks_per_dim * blocks_per_dim * blocks_per_dim).max(1) as f32;
    let mut completed_blocks = 0usize;
    let start = Instant::now();

    if let Some(progress) = progress.as_deref_mut() {
        progress.set_phase("CPU computing", true);
        progress.set_cpu_progress(0.0, true);
    }

    for ii in (0..size).step_by(tile) {
        check_canceled(cancel)?;
        let i_end = (ii + tile).min(size);
        for kk in (0..size).step_by(tile) {
            check_canceled(cancel)?;
            let k_end = (kk + tile).min(size);
            for jj in (0..size).step_by(tile) {
                check_canceled(cancel)?;
                let j_end = (jj + tile).min(size);
                for i in ii..i_end {
                    let c_row = i * size;
                    let a_row = i * size;
                    for k in kk..k_end {
                        let a_val = a[a_row + k];
                        let b_row = k * size;
                        for j in jj..j_end {
                            c[c_row + j] += a_val * b[b_row + j];
                        }
                    }
                }
                completed_blocks += 1;
                if let Some(progress) = progress.as_deref_mut() {
                    progress.set_cpu_progress(completed_blocks as f32 / total_blocks, false);
                }
            }
        }
    }

    if let Some(progress) = progress.as_deref_mut() {
        progress.set_cpu_progress(1.0, true);
    }

    Ok((c, start.elapsed().as_secs_f64() * 1000.0))
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

    let sample_size = cpu_estimate_sample_size(size, cpu_info);
    let sample_a = copy_top_left_submatrix(a, size, sample_size, cancel)?;
    let sample_b = copy_top_left_submatrix(b, size, sample_size, cancel)?;

    if sample_size > CPU_ESTIMATE_MIN_SAMPLE_SIZE {
        let warm_size = CPU_ESTIMATE_MIN_SAMPLE_SIZE.min(sample_size);
        let warm_a = copy_top_left_submatrix(&sample_a, sample_size, warm_size, cancel)?;
        let warm_b = copy_top_left_submatrix(&sample_b, sample_size, warm_size, cancel)?;
        let _ = cpu_multiply_cancelable(warm_size, &warm_a, &warm_b, cancel, None)?;
    }

    check_canceled(cancel)?;
    let (_, elapsed_ms) = cpu_multiply_cancelable(sample_size, &sample_a, &sample_b, cancel, None)?;
    let scale = (size as f64 / sample_size as f64).powi(3);

    if let Some(progress) = progress.as_deref_mut() {
        progress.set_cpu_progress(1.0, true);
    }

    Ok(elapsed_ms * scale)
}

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
) -> Result<BenchmarkResult> {
    let cancel = AtomicBool::new(false);
    run_single_cancelable(
        size,
        adapter,
        validate_output,
        estimate_cpu_time,
        &cancel,
        None,
    )
}

fn run_single_cancelable(
    size: usize,
    adapter: AdapterInfo,
    validate_output: bool,
    estimate_cpu_time: bool,
    cancel: &AtomicBool,
    progress_tx: Option<Sender<WorkerEvent>>,
) -> Result<BenchmarkResult> {
    let mut progress = SingleProgressTracker::new(size, &adapter, progress_tx);
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
    let gpu_ticker = progress.start_gpu_ticker();
    let gpu = runner.multiply_cancelable(size, &a, &b, true, Some(cancel))?;
    if let Some(ticker) = gpu_ticker {
        ticker.stop();
    }
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
        speedup,
        validation,
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

fn run_repeat(
    size: usize,
    adapter: AdapterInfo,
    mode: RepeatMode,
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
                let timing = match runner.multiply_cancelable(size, &a, &b, true, Some(&cancel)) {
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

struct HardwareAccelApp {
    adapters: Vec<AdapterInfo>,
    cpu_info: CpuInfo,
    selected_adapter: usize,
    size_text: String,
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
}

impl HardwareAccelApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = mpsc::channel();
        let adapters = enumerate_adapters();
        let cpu_info = detect_cpu_info();
        let selected_adapter = adapters
            .iter()
            .position(|adapter| adapter.device_type != wgpu::DeviceType::Cpu)
            .unwrap_or(0);
        let mut app = Self {
            adapters,
            cpu_info,
            selected_adapter,
            size_text: DEFAULT_SIZES[6].to_string(),
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

        self.launch_single(size, adapter, self.validate_output, self.estimate_cpu_time);
    }

    fn launch_single(
        &mut self,
        size: usize,
        adapter: AdapterInfo,
        validate: bool,
        estimate_cpu_time: bool,
    ) {
        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.cancel = Some(cancel);
        self.running = true;
        self.progress = 0.0;
        self.cpu_progress = 0.0;
        self.gpu_progress = 0.0;
        self.eta_text = "ETA: estimating".to_owned();
        self.status = format!("Running {size}x{size} benchmark...");
        self.log(format!(
            "Starting benchmark on {} with {} CPU timing",
            adapter.label(),
            if estimate_cpu_time {
                "estimated"
            } else {
                "exact"
            }
        ));
        thread::spawn(move || {
            let result = run_single_cancelable(
                size,
                adapter,
                validate,
                estimate_cpu_time,
                &worker_cancel,
                Some(tx.clone()),
            )
            .map_err(|err| format!("{err:#}"));
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
                self.validate_output,
                self.estimate_cpu_time,
                self.repeat_mode,
                self.repeat_duration,
            ) {
                self.status = "VRAM warning: confirm before starting the repeat test".to_owned();
                self.pending_vram_warning = Some(warning);
                return;
            }
        }

        self.launch_repeat(size, adapter, self.repeat_mode, self.repeat_duration);
    }

    fn launch_repeat(
        &mut self,
        size: usize,
        adapter: AdapterInfo,
        mode: RepeatMode,
        duration: RepeatDuration,
    ) {
        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.cancel = Some(cancel);
        self.running = true;
        self.repeat_running = true;
        self.progress = 0.0;
        self.status = format!("Running {mode} repeat test for {duration}...");
        self.log(format!(
            "Starting {mode} {duration} repeat test at {size}x{size} on {}",
            adapter.label()
        ));
        thread::spawn(move || {
            let result = run_repeat(
                size,
                adapter,
                mode,
                worker_cancel,
                tx.clone(),
                duration.duration(),
            )
            .map_err(|err| format!("{err:#}"));
            let _ = tx.send(WorkerEvent::RepeatDone(result));
        });
    }

    fn vram_warning_for(
        &self,
        action: RunAction,
        size: usize,
        adapter: AdapterInfo,
        validate_output: bool,
        estimate_cpu_time: bool,
        repeat_mode: RepeatMode,
        repeat_duration: RepeatDuration,
    ) -> Option<PendingVramWarning> {
        let estimated_gpu_bytes = gpu_working_set_bytes(size)?;
        let (limit_bytes, limit_label) = adapter_memory_limit_bytes(&adapter)?;
        (estimated_gpu_bytes > limit_bytes).then(|| PendingVramWarning {
            action,
            size,
            adapter,
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
                warning.validate_output,
                warning.estimate_cpu_time,
            ),
            RunAction::Repeat => self.launch_repeat(
                warning.size,
                warning.adapter,
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
            self.status = "Cancel requested; stopping repeat test...".to_owned();
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
                        Ok(result) => {
                            self.progress = 1.0;
                            self.cpu_progress = 1.0;
                            self.gpu_progress = 1.0;
                            self.eta_text = "ETA: complete".to_owned();
                            self.status = "Benchmark complete".to_owned();
                            self.log(format!(
                                "Benchmark complete: CPU {} ms ({}, {}), GPU total {} ms, GPU compute {} ms",
                                format_cpu_ms(&result),
                                if result.cpu_estimated {
                                    "estimated"
                                } else {
                                    "exact"
                                },
                                result.cpu_model,
                                format_ms(Some(result.gpu_total_ms)),
                                format_ms(result.gpu_compute_ms)
                            ));
                            self.results.push(result);
                        }
                        Err(err) => {
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
                        "{} repeat: {:.1}s, {} iteration(s), latest {} ms, avg {} ms, compute avg {} ms",
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
                            if !progress.canceled {
                                self.progress = 1.0;
                            }
                            let state = if progress.canceled {
                                "canceled"
                            } else {
                                "complete"
                            };
                            self.status = format!(
                                "Repeat test {state}: {} iteration(s), avg {} ms",
                                progress.iterations,
                                format_ms(Some(progress.average_total_ms))
                            );
                            self.log(format!(
                                "Repeat test {state}: mode {}, size {}, iterations {}, avg {} ms, compute avg {} ms",
                                progress.mode,
                                progress.size,
                                progress.iterations,
                                format_ms(Some(progress.average_total_ms)),
                                format_ms(progress.average_compute_ms)
                            ));
                        }
                        Err(err) => {
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
            }
        }
    }
}

impl eframe::App for HardwareAccelApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_worker_events();
        if self.running {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        egui::Panel::top("top_panel").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Hardware Acceleration Tester");
                ui.separator();
                ui.label(&self.status);
                if !self.eta_text.is_empty() {
                    ui.separator();
                    ui.label(&self.eta_text);
                }
            });
            if self.repeat_running {
                ui.add(
                    egui::ProgressBar::new(self.progress)
                        .show_percentage()
                        .text("Repeat elapsed"),
                );
            } else {
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
            }
        });

        egui::Panel::left("controls")
            .resizable(false)
            .min_size(330.0)
            .show_inside(ui, |ui| {
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
                    if size == 16384 {
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            "16K uses about 3 GB for A/B/C alone before readback and driver overhead.",
                        );
                    }
                }

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

                ui.separator();
                ui.label("Repeat test");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.repeat_mode, RepeatMode::Gpu, "GPU");
                    ui.selectable_value(&mut self.repeat_mode, RepeatMode::Cpu, "CPU");
                });
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
                ui.add_enabled_ui(!self.running && !self.adapters.is_empty(), |ui| {
                    if ui.button("Start repeat").clicked() {
                        self.start_repeat();
                    }
                });
                ui.add_enabled_ui(self.repeat_running, |ui| {
                    if ui.button("Cancel repeat").clicked() {
                        self.cancel_repeat();
                    }
                });
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            let available_height = ui.available_height();
            let log_height = (available_height * 0.18).clamp(110.0, 150.0);
            let results_height = (available_height - log_height - 56.0).max(260.0);

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
                                .num_columns(9)
                                .show(ui, |ui| {
                                    ui.strong("Size");
                                    ui.strong("CPU ms");
                                    ui.strong("CPU model");
                                    ui.strong("GPU compute ms");
                                    ui.strong("GPU total ms");
                                    ui.strong("Transfer/sync ms");
                                    ui.strong("Speedup");
                                    ui.strong("Adapter");
                                    ui.strong("Validation");
                                    ui.end_row();

                                    for result in &self.results {
                                        ui.label(format!("{}x{}", result.size, result.size));
                                        ui.label(format_cpu_ms(result));
                                        ui.label(&result.cpu_model);
                                        ui.label(format_ms(result.gpu_compute_ms));
                                        ui.label(format_ms(Some(result.gpu_total_ms)));
                                        ui.label(format_ms(result.transfer_sync_ms));
                                        ui.label(format_speedup(result.speedup));
                                        ui.label(&result.adapter);
                                        ui.label(&result.validation);
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
                        ui.monospace(line);
                    }
                });
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
    }
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

fn estimate_gpu_seconds(size: usize, adapter: &AdapterInfo) -> f64 {
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
    (compute_s + transfer_s).max(0.2)
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

fn device_type_label(value: wgpu::DeviceType) -> &'static str {
    match value {
        wgpu::DeviceType::IntegratedGpu => "Integrated GPU",
        wgpu::DeviceType::DiscreteGpu => "Discrete GPU",
        wgpu::DeviceType::VirtualGpu => "Virtual GPU",
        wgpu::DeviceType::Cpu => "CPU/Software",
        wgpu::DeviceType::Other => "Other GPU",
    }
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

        println!("Running self-test on {}", adapter.label());
        let result = run_single(size, adapter, true, estimate_cpu_time)?;
        println!("Size: {}x{}", result.size, result.size);
        println!("CPU: {} ms ({})", format_cpu_ms(&result), result.cpu_model);
        println!("GPU compute: {} ms", format_ms(result.gpu_compute_ms));
        println!("GPU total: {} ms", format_ms(Some(result.gpu_total_ms)));
        println!("Transfer/sync: {} ms", format_ms(result.transfer_sync_ms));
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
        "Hardware Acceleration Tester",
        options,
        Box::new(|cc| Ok(Box::new(HardwareAccelApp::new(cc)))),
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
            speedup: 102.83,
            validation: "Skipped".to_owned(),
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
        assert_eq!(gpu_dispatch_chunk_rows(128), 128);
        assert_eq!(gpu_dispatch_chunk_rows(2048), 64);
        assert_eq!(gpu_dispatch_chunk_rows(4096), GPU_CANCELABLE_CHUNK_ROWS);
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
}
