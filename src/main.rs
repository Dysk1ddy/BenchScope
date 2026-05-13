use std::{
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use bytemuck::{Pod, Zeroable};
use eframe::egui;
use wgpu::util::DeviceExt;

const DEFAULT_SIZES: &[usize] = &[128, 256, 512, 1024, 2048];
const TILE_SIZE: u32 = 16;
const REPEAT_SECONDS: f64 = 60.0;

const MATMUL_SHADER: &str = r#"
const TILE: u32 = 16u;

struct Params {
    n: u32,
    _pad0: u32,
    _pad1: u32,
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

        if (row < params.n && a_col < params.n) {
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

    if (row < params.n && col < params.n) {
        c[row * params.n + col] = sum;
    }
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    n: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
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
struct BenchmarkResult {
    size: usize,
    adapter: String,
    cpu_ms: f64,
    gpu_compute_ms: Option<f64>,
    gpu_total_ms: f64,
    transfer_sync_ms: Option<f64>,
    speedup: f64,
    validation: String,
}

#[derive(Clone, Debug)]
struct RepeatProgress {
    mode: RepeatMode,
    size: usize,
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

#[derive(Debug)]
enum WorkerEvent {
    SingleDone(Result<BenchmarkResult, String>),
    RepeatProgress(RepeatProgress),
    RepeatDone(Result<RepeatProgress, String>),
}

#[derive(Debug)]
struct GpuTiming {
    compute_ms: Option<f64>,
    total_ms: f64,
    transfer_sync_ms: Option<f64>,
    output: Vec<f32>,
}

struct GpuRunner {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    timestamp_query: bool,
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

        let mut descriptor = wgpu::DeviceDescriptor::default();
        descriptor.label = Some("Hardware Acceleration Tester device");
        descriptor.required_features = required_features;
        descriptor.required_limits = wgpu::Limits::downlevel_defaults()
            .using_resolution(adapter.limits());

        let (device, queue) = pollster::block_on(adapter.request_device(&descriptor))
            .context("requesting wgpu device")?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Tiled matrix multiplication shader"),
            source: wgpu::ShaderSource::Wgsl(MATMUL_SHADER.into()),
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

        let runner = Self {
            device,
            queue,
            pipeline,
            bind_group_layout,
            timestamp_query,
        };
        runner.warm_up()?;
        Ok(runner)
    }

    fn warm_up(&self) -> Result<()> {
        let a = vec![1.0_f32];
        let b = vec![1.0_f32];
        self.multiply(1, &a, &b, false).map(|_| ())
    }

    fn multiply(
        &self,
        n: usize,
        a: &[f32],
        b: &[f32],
        use_timestamps: bool,
    ) -> Result<GpuTiming> {
        let elements = n
            .checked_mul(n)
            .ok_or_else(|| anyhow!("matrix size overflow"))?;
        if a.len() != elements || b.len() != elements {
            return Err(anyhow!("matrix data length does not match {n}x{n}"));
        }

        let byte_len = (elements * std::mem::size_of::<f32>()) as wgpu::BufferAddress;
        let total_start = Instant::now();

        let a_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Matrix A"),
            contents: bytemuck::cast_slice(a),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let b_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
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
            n: n as u32,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
        };
        let params_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Matrix params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
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

        let timestamp_enabled = self.timestamp_query && use_timestamps;
        let query_set = timestamp_enabled.then(|| {
            self.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("GPU compute timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            })
        });
        let timestamp_resolve = timestamp_enabled.then(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Timestamp resolve"),
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        });
        let timestamp_readback = timestamp_enabled.then(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Timestamp readback"),
                size: 16,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Matrix multiplication encoder"),
            });

        {
            let timestamp_writes = query_set.as_ref().map(|query_set| {
                wgpu::ComputePassTimestampWrites {
                    query_set,
                    beginning_of_pass_write_index: Some(0),
                    end_of_pass_write_index: Some(1),
                }
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Matrix multiplication pass"),
                timestamp_writes,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let groups = (n as u32).div_ceil(TILE_SIZE);
            pass.dispatch_workgroups(groups, groups, 1);
        }

        if let (Some(query_set), Some(resolve), Some(readback)) =
            (&query_set, &timestamp_resolve, &timestamp_readback)
        {
            encoder.resolve_query_set(query_set, 0..2, resolve, 0);
            encoder.copy_buffer_to_buffer(resolve, 0, readback, 0, 16);
        }
        encoder.copy_buffer_to_buffer(&c_buffer, 0, &readback_buffer, 0, byte_len);

        let submission = self.queue.submit([encoder.finish()]);
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
            .context("waiting for GPU submission")?;

        let output = read_f32_buffer(&self.device, &readback_buffer, elements)
            .context("reading GPU result buffer")?;
        let compute_ms = if let Some(readback) = &timestamp_readback {
            read_timestamps(&self.device, readback)
                .ok()
                .map(|timestamps| {
                    let delta = timestamps[1].saturating_sub(timestamps[0]);
                    (delta as f64 * self.queue.get_timestamp_period() as f64) / 1_000_000.0
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

fn read_f32_buffer(device: &wgpu::Device, buffer: &wgpu::Buffer, elements: usize) -> Result<Vec<f32>> {
    let slice = buffer.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result.map_err(|err| err.to_string()));
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .context("polling mapped result buffer")?;
    rx.recv()
        .context("waiting for result buffer map callback")?
        .map_err(|err| anyhow!(err))?;
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

fn read_timestamps(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Result<[u64; 2]> {
    let slice = buffer.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result.map_err(|err| err.to_string()));
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .context("polling mapped timestamp buffer")?;
    rx.recv()
        .context("waiting for timestamp map callback")?
        .map_err(|err| anyhow!(err))?;
    let data = slice.get_mapped_range();
    let timestamps = bytemuck::cast_slice::<u8, u64>(&data);
    let result = [timestamps[0], timestamps[1]];
    drop(data);
    buffer.unmap();
    Ok(result)
}

fn enumerate_adapters() -> Vec<AdapterInfo> {
    let instance = wgpu::Instance::default();
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    adapters
        .into_iter()
        .enumerate()
        .map(|(index, adapter)| {
            let info = adapter.get_info();
            let features = adapter.features();
            AdapterInfo {
                index,
                name: info.name,
                backend: info.backend,
                device_type: info.device_type,
                vendor: info.vendor,
                device: info.device,
                driver: info.driver,
                timestamp_query: features.contains(wgpu::Features::TIMESTAMP_QUERY),
            }
        })
        .collect()
}

fn generate_matrices(size: usize) -> Result<(Vec<f32>, Vec<f32>)> {
    let elements = size
        .checked_mul(size)
        .ok_or_else(|| anyhow!("matrix size overflow"))?;
    let mut a = Vec::with_capacity(elements);
    let mut b = Vec::with_capacity(elements);
    for i in 0..elements {
        a.push((i % 97) as f32 / 97.0);
        b.push(((i * 3 + 1) % 89) as f32 / 89.0);
    }
    Ok((a, b))
}

fn cpu_multiply(size: usize, a: &[f32], b: &[f32]) -> (Vec<f32>, f64) {
    let mut c = vec![0.0_f32; size * size];
    let tile = 32usize;
    let start = Instant::now();

    for ii in (0..size).step_by(tile) {
        let i_end = (ii + tile).min(size);
        for kk in (0..size).step_by(tile) {
            let k_end = (kk + tile).min(size);
            for jj in (0..size).step_by(tile) {
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
            }
        }
    }

    (c, start.elapsed().as_secs_f64() * 1000.0)
}

fn validate(cpu: &[f32], gpu: &[f32], size: usize) -> String {
    if cpu.len() != gpu.len() {
        return format!("Failed: CPU len {}, GPU len {}", cpu.len(), gpu.len());
    }

    let mut max_abs = 0.0_f32;
    let mut max_rel = 0.0_f32;
    for (&cpu_value, &gpu_value) in cpu.iter().zip(gpu.iter()) {
        let diff = (cpu_value - gpu_value).abs();
        max_abs = max_abs.max(diff);
        max_rel = max_rel.max(diff / cpu_value.abs().max(1.0));
    }

    let abs_tol = 0.02_f32.max(size as f32 * 0.00005);
    let rel_tol = 0.0025_f32;
    if max_abs <= abs_tol || max_rel <= rel_tol {
        format!("Passed (max abs {max_abs:.5}, max rel {max_rel:.5})")
    } else {
        format!("Failed (max abs {max_abs:.5}, max rel {max_rel:.5})")
    }
}

fn run_single(size: usize, adapter: AdapterInfo, validate_output: bool) -> Result<BenchmarkResult> {
    let (a, b) = generate_matrices(size)?;
    let (cpu_output, cpu_ms) = cpu_multiply(size, &a, &b);
    let runner = GpuRunner::new(adapter.index)?;
    let gpu = runner.multiply(size, &a, &b, true)?;
    let validation = if validate_output {
        validate(&cpu_output, &gpu.output, size)
    } else {
        "Skipped".to_owned()
    };
    let speedup = if gpu.total_ms > 0.0 {
        cpu_ms / gpu.total_ms
    } else {
        f64::INFINITY
    };
    Ok(BenchmarkResult {
        size,
        adapter: adapter.label(),
        cpu_ms,
        gpu_compute_ms: gpu.compute_ms,
        gpu_total_ms: gpu.total_ms,
        transfer_sync_ms: gpu.transfer_sync_ms,
        speedup,
        validation,
    })
}

fn run_repeat(
    size: usize,
    adapter: AdapterInfo,
    mode: RepeatMode,
    cancel: Arc<AtomicBool>,
    tx: Sender<WorkerEvent>,
    duration: Duration,
) -> Result<RepeatProgress> {
    let (a, b) = generate_matrices(size)?;
    let deadline = Instant::now() + duration;
    let started = Instant::now();
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
            elapsed_s,
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
        if force || now.duration_since(last_emit) >= Duration::from_millis(100) {
            let _ = tx.send(WorkerEvent::RepeatProgress(progress.clone()));
            last_emit = now;
        }
        progress
    };

    match mode {
        RepeatMode::Cpu => {
            while Instant::now() < deadline && !cancel.load(Ordering::Relaxed) {
                let (_, elapsed_ms) = cpu_multiply(size, &a, &b);
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
                let timing = runner.multiply(size, &a, &b, true)?;
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
    selected_adapter: usize,
    size_text: String,
    validate_output: bool,
    repeat_mode: RepeatMode,
    results: Vec<BenchmarkResult>,
    log: Vec<String>,
    status: String,
    progress: f32,
    rx: Receiver<WorkerEvent>,
    tx: Sender<WorkerEvent>,
    cancel: Option<Arc<AtomicBool>>,
    running: bool,
    repeat_running: bool,
}

impl HardwareAccelApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = mpsc::channel();
        let adapters = enumerate_adapters();
        let selected_adapter = adapters
            .iter()
            .position(|adapter| adapter.device_type != wgpu::DeviceType::Cpu)
            .unwrap_or(0);
        let mut app = Self {
            adapters,
            selected_adapter,
            size_text: DEFAULT_SIZES[1].to_string(),
            validate_output: true,
            repeat_mode: RepeatMode::Gpu,
            results: Vec::new(),
            log: Vec::new(),
            status: "Ready".to_owned(),
            progress: 0.0,
            rx,
            tx,
            cancel: None,
            running: false,
            repeat_running: false,
        };
        app.log("Application started");
        if app.adapters.is_empty() {
            app.status = "No wgpu adapters found".to_owned();
            app.log("No wgpu adapters found");
        } else {
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
        if size > 8192 {
            return Err(anyhow!("matrix size is too large for this first version"));
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

        let tx = self.tx.clone();
        let validate = self.validate_output;
        self.running = true;
        self.progress = 0.1;
        self.status = format!("Running {size}x{size} benchmark...");
        self.log(format!("Starting benchmark on {}", adapter.label()));
        thread::spawn(move || {
            let result = run_single(size, adapter, validate).map_err(|err| format!("{err:#}"));
            let _ = tx.send(WorkerEvent::SingleDone(result));
        });
    }

    fn start_repeat(&mut self) {
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

        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let mode = self.repeat_mode;
        self.cancel = Some(cancel);
        self.running = true;
        self.repeat_running = true;
        self.progress = 0.0;
        self.status = format!("Running {mode} repeat test for 60 seconds...");
        self.log(format!(
            "Starting {mode} repeat test at {size}x{size} on {}",
            adapter.label()
        ));
        thread::spawn(move || {
            let result = run_repeat(
                size,
                adapter,
                mode,
                worker_cancel,
                tx.clone(),
                Duration::from_secs_f64(REPEAT_SECONDS),
            )
            .map_err(|err| format!("{err:#}"));
            let _ = tx.send(WorkerEvent::RepeatDone(result));
        });
    }

    fn cancel_repeat(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
            self.status = "Cancel requested; waiting for current iteration to finish".to_owned();
            self.log("Cancel requested");
        }
    }

    fn poll_worker_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                WorkerEvent::SingleDone(result) => {
                    self.running = false;
                    self.repeat_running = false;
                    self.cancel = None;
                    self.progress = 1.0;
                    match result {
                        Ok(result) => {
                            self.status = "Benchmark complete".to_owned();
                            self.log(format!(
                                "Benchmark complete: CPU {} ms, GPU total {} ms, GPU compute {} ms",
                                format_ms(Some(result.cpu_ms)),
                                format_ms(Some(result.gpu_total_ms)),
                                format_ms(result.gpu_compute_ms)
                            ));
                            self.results.push(result);
                        }
                        Err(err) => {
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
                WorkerEvent::RepeatProgress(progress) => {
                    self.progress = (progress.elapsed_s / REPEAT_SECONDS).clamp(0.0, 1.0) as f32;
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
                    match result {
                        Ok(progress) => {
                            if !progress.canceled {
                                self.progress = 1.0;
                            }
                            let state = if progress.canceled { "canceled" } else { "complete" };
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
            });
            ui.add(egui::ProgressBar::new(self.progress).show_percentage());
        });

        egui::Panel::left("controls")
            .resizable(false)
            .min_size(330.0)
            .show_inside(ui, |ui| {
                ui.heading("Controls");
                ui.add_space(8.0);

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

                let memory_mb = self
                    .selected_size()
                    .ok()
                    .map(memory_estimate_mb)
                    .unwrap_or(0.0);
                ui.small(format!(
                    "Approx. matrix buffers: {memory_mb:.1} MB for A, B, and C"
                ));

                ui.add_enabled_ui(!self.running && !self.adapters.is_empty(), |ui| {
                    if ui.button("Run benchmark").clicked() {
                        self.start_single();
                    }
                });

                ui.separator();
                ui.label("1-minute repeat test");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.repeat_mode, RepeatMode::Gpu, "GPU");
                    ui.selectable_value(&mut self.repeat_mode, RepeatMode::Cpu, "CPU");
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
            ui.heading("Results");
            ui.add_space(6.0);
            egui::ScrollArea::both().show(ui, |ui| {
                egui::Grid::new("results_grid")
                    .striped(true)
                    .num_columns(8)
                    .show(ui, |ui| {
                        ui.strong("Size");
                        ui.strong("CPU ms");
                        ui.strong("GPU compute ms");
                        ui.strong("GPU total ms");
                        ui.strong("Transfer/sync ms");
                        ui.strong("Speedup");
                        ui.strong("Adapter");
                        ui.strong("Validation");
                        ui.end_row();

                        for result in &self.results {
                            ui.label(format!("{}x{}", result.size, result.size));
                            ui.label(format_ms(Some(result.cpu_ms)));
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

            ui.separator();
            ui.heading("Log");
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .max_height(220.0)
                .show(ui, |ui| {
                    for line in &self.log {
                        ui.monospace(line);
                    }
                });
        });
    }
}

fn memory_estimate_mb(size: usize) -> f64 {
    let bytes = size as f64 * size as f64 * 4.0 * 3.0;
    bytes / (1024.0 * 1024.0)
}

fn format_ms(value: Option<f64>) -> String {
    match value {
        Some(value) if value >= 1000.0 => format!("{value:.1}"),
        Some(value) => format!("{value:.3}"),
        None => "N/A".to_owned(),
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
    if value.is_empty() {
        "unknown"
    } else {
        value
    }
}

fn run_cli(args: &[String]) -> Result<bool> {
    if args.iter().any(|arg| arg == "--list-gpus") {
        let adapters = enumerate_adapters();
        if adapters.is_empty() {
            println!("No wgpu adapters found.");
        }
        for adapter in adapters {
            println!(
                "[{}] {} | vendor {:04X} device {:04X} | driver {} | timestamp {}",
                adapter.index,
                adapter.label(),
                adapter.vendor,
                adapter.device,
                empty_to_unknown(&adapter.driver),
                if adapter.timestamp_query { "yes" } else { "no" }
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
        let result = run_single(size, adapter, true)?;
        println!("Size: {}x{}", result.size, result.size);
        println!("CPU: {} ms", format_ms(Some(result.cpu_ms)));
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
}
