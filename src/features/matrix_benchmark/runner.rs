#[derive(Debug)]
enum WorkerEvent {
    SingleProgress(SingleProgress),
    SingleDone(Result<BenchmarkResult, String>),
    RepeatProgress(RepeatProgress),
    RepeatDone(Result<RepeatProgress, String>),
    Log(String),
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
    stress_gpu_backend: StressGpuBackend,
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

struct DirectDispatch {
    bind_group: wgpu::BindGroup,
    rows: usize,
    _params_buffer: wgpu::Buffer,
}

struct PanelDispatch {
    bind_group: wgpu::BindGroup,
    rows: usize,
    cols: usize,
    _params_buffer: wgpu::Buffer,
}

#[derive(Default)]
struct GpuRepeatCounters {
    iterations: u64,
    total_ms: f64,
    total_compute_ms: f64,
    compute_count: u64,
    latest_ms: f64,
    current_iteration_ms: f64,
}

impl GpuRepeatCounters {
    fn record_batch(&mut self, completed_iterations: u64, batch_ms: f64) {
        self.current_iteration_ms += batch_ms;
        if completed_iterations == 0 {
            self.latest_ms = self.current_iteration_ms;
            return;
        }

        self.iterations += completed_iterations;
        self.latest_ms = self.current_iteration_ms / completed_iterations as f64;
        self.total_ms += self.current_iteration_ms;
        self.total_compute_ms += self.current_iteration_ms;
        self.compute_count += completed_iterations;
        self.current_iteration_ms = 0.0;
    }
}

#[derive(Default)]
struct PyTorchMatrixStressState {
    cuda_available: Option<bool>,
    unavailable_reason: Option<String>,
    error: Option<String>,
    gpu_name: Option<String>,
    effective_size: Option<usize>,
    iterations: u64,
    latest_ms: f64,
    total_ms: f64,
    total_compute_ms: f64,
    compute_count: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PyTorchMatrixStressProgressSample {
    iterations: u64,
    latest_ms: f64,
    total_ms: f64,
    total_compute_ms: f64,
    compute_count: u64,
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
    max_row_extent: usize,
    target_low_ms: f64,
    soft_backoff_ms: f64,
    hard_backoff_ms: f64,
    hard_batch_backoff_ms: f64,
    backoff_count: usize,
    stable_low_count: usize,
}

impl GpuWorkGovernor {
    fn new(
        row_extent: usize,
        min_row_extent: usize,
        max_row_extent: usize,
        gpu_intensity: GpuIntensity,
    ) -> Self {
        let min_row_extent = min_row_extent.max(1);
        let max_row_extent = align_block_extent(max_row_extent.max(min_row_extent))
            .max(min_row_extent)
            .max(1);
        Self {
            row_extent: align_block_extent(row_extent.clamp(min_row_extent, max_row_extent))
                .clamp(min_row_extent, max_row_extent),
            min_row_extent,
            max_row_extent,
            target_low_ms: gpu_target_low_ms(gpu_intensity),
            soft_backoff_ms: gpu_soft_backoff_ms(gpu_intensity),
            hard_backoff_ms: gpu_hard_backoff_ms(gpu_intensity),
            hard_batch_backoff_ms: gpu_hard_batch_backoff_ms(gpu_intensity),
            backoff_count: 0,
            stable_low_count: 0,
        }
    }

    fn row_extent(&self, remaining: usize) -> usize {
        self.row_extent.min(remaining).max(1)
    }

    fn record_dispatch(&mut self, observed_ms: f64) {
        if observed_ms > self.hard_backoff_ms && self.row_extent > self.min_row_extent {
            self.row_extent = self.shrink_row_extent(2);
            self.backoff_count += 1;
            self.stable_low_count = 0;
        } else if observed_ms > self.soft_backoff_ms && self.row_extent > self.min_row_extent {
            self.row_extent = self.shrink_row_extent(4);
            self.backoff_count += 1;
            self.stable_low_count = 0;
        } else if observed_ms < self.target_low_ms && self.row_extent < self.max_row_extent {
            self.stable_low_count += 1;
            if self.stable_low_count >= GPU_STABLE_DISPATCHES_BEFORE_GROW {
                self.row_extent =
                    align_block_extent((self.row_extent * 2).min(self.max_row_extent));
                self.stable_low_count = 0;
            }
        } else {
            self.stable_low_count = 0;
        }
    }

    fn record_batch(&mut self, observed_ms: f64) {
        if observed_ms > self.hard_batch_backoff_ms && self.row_extent > self.min_row_extent {
            self.row_extent = self.shrink_row_extent(2);
            self.backoff_count += 1;
            self.stable_low_count = 0;
        }
    }

    fn shrink_row_extent(&self, divisor: usize) -> usize {
        align_block_extent((self.row_extent / divisor.max(1)).max(self.min_row_extent))
            .clamp(self.min_row_extent, self.max_row_extent)
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
    tiny_stress_pipeline: wgpu::ComputePipeline,
    register_tiny_stress_pipeline: wgpu::ComputePipeline,
    panel_stress_pipeline: wgpu::ComputePipeline,
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
        let tiny_stress_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Tiny matrix stress shader"),
            source: wgpu::ShaderSource::Wgsl(TINY_STRESS_MATMUL_SHADER.into()),
        });
        let register_tiny_stress_shader =
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Register tiny matrix stress shader"),
                source: wgpu::ShaderSource::Wgsl(REGISTER_TINY_STRESS_MATMUL_SHADER.into()),
            });
        let panel_stress_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Panel matrix stress shader"),
            source: wgpu::ShaderSource::Wgsl(PANEL_STRESS_MATMUL_SHADER.into()),
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
        let tiny_stress_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Tiny matrix stress compute pipeline"),
                layout: Some(&pipeline_layout),
                module: &tiny_stress_shader,
                entry_point: Some("main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
        let register_tiny_stress_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Register tiny matrix stress compute pipeline"),
                layout: Some(&pipeline_layout),
                module: &register_tiny_stress_shader,
                entry_point: Some("main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
        let panel_stress_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Panel matrix stress compute pipeline"),
                layout: Some(&pipeline_layout),
                module: &panel_stress_shader,
                entry_point: Some("main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });

        let runner = Self {
            device,
            queue,
            pipeline,
            blocked_pipeline,
            tiny_stress_pipeline,
            register_tiny_stress_pipeline,
            panel_stress_pipeline,
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

    fn create_direct_dispatch(
        &self,
        a_buffer: &wgpu::Buffer,
        b_buffer: &wgpu::Buffer,
        c_buffer: &wgpu::Buffer,
        params: Params,
        rows: usize,
    ) -> DirectDispatch {
        let params_buffer =
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Matrix dispatch params"),
                    contents: bytemuck::bytes_of(&params),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Matrix multiplication dispatch bind group"),
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

        DirectDispatch {
            bind_group,
            rows,
            _params_buffer: params_buffer,
        }
    }

    fn create_panel_dispatch(
        &self,
        a_buffer: &wgpu::Buffer,
        b_buffer: &wgpu::Buffer,
        c_buffer: &wgpu::Buffer,
        n: u32,
        size: usize,
        panel: &ColumnPanel,
        row_offset: usize,
        rows: usize,
    ) -> Result<PanelDispatch> {
        let a_elements = rows
            .checked_mul(size)
            .ok_or_else(|| anyhow!("A panel row size overflow"))?;
        let c_elements = rows
            .checked_mul(panel.cols)
            .ok_or_else(|| anyhow!("C panel row size overflow"))?;
        let a_offset = buffer_len_bytes(
            row_offset
                .checked_mul(size)
                .ok_or_else(|| anyhow!("A panel offset overflow"))?,
        )?;
        let b_offset = buffer_len_bytes(panel.element_offset)?;
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
        let b_bytes = buffer_len_bytes(
            size.checked_mul(panel.cols)
                .ok_or_else(|| anyhow!("B panel size overflow"))?,
        )?;
        let c_bytes = buffer_len_bytes(c_elements)?;
        let a_binding_size =
            wgpu::BufferSize::new(a_bytes).ok_or_else(|| anyhow!("empty A panel"))?;
        let b_binding_size =
            wgpu::BufferSize::new(b_bytes).ok_or_else(|| anyhow!("empty B panel"))?;
        let c_binding_size =
            wgpu::BufferSize::new(c_bytes).ok_or_else(|| anyhow!("empty C panel"))?;
        let params = BlockParams {
            n,
            rows: u32::try_from(rows).context("panel row block exceeds shader limits")?,
            cols: u32::try_from(panel.cols).context("panel column block exceeds shader limits")?,
            _pad: 0,
        };
        let params_buffer =
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Persistent panel dispatch params"),
                    contents: bytemuck::bytes_of(&params),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Persistent panel matrix dispatch bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: a_buffer,
                        offset: a_offset,
                        size: Some(a_binding_size),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: b_buffer,
                        offset: b_offset,
                        size: Some(b_binding_size),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: c_buffer,
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

        Ok(PanelDispatch {
            bind_group,
            rows,
            cols: panel.cols,
            _params_buffer: params_buffer,
        })
    }

    fn create_panel_stress_dispatch(
        &self,
        a_buffer: &wgpu::Buffer,
        b_buffer: &wgpu::Buffer,
        scratch_buffer: &wgpu::Buffer,
        n: u32,
        size: usize,
        panel: &ColumnPanel,
        row_offset: usize,
        rows: usize,
        rounds: u32,
    ) -> Result<PanelDispatch> {
        let a_elements = rows
            .checked_mul(size)
            .ok_or_else(|| anyhow!("A stress panel row size overflow"))?;
        let a_offset = buffer_len_bytes(
            row_offset
                .checked_mul(size)
                .ok_or_else(|| anyhow!("A stress panel offset overflow"))?,
        )?;
        let b_offset = buffer_len_bytes(panel.element_offset)?;
        let a_bytes = buffer_len_bytes(a_elements)?;
        let b_bytes = buffer_len_bytes(
            size.checked_mul(panel.cols)
                .ok_or_else(|| anyhow!("B stress panel size overflow"))?,
        )?;
        let a_binding_size =
            wgpu::BufferSize::new(a_bytes).ok_or_else(|| anyhow!("empty stress A panel"))?;
        let b_binding_size =
            wgpu::BufferSize::new(b_bytes).ok_or_else(|| anyhow!("empty stress B panel"))?;
        let params = BlockParams {
            n,
            rows: u32::try_from(rows).context("stress panel row block exceeds shader limits")?,
            cols: u32::try_from(panel.cols)
                .context("stress panel column block exceeds shader limits")?,
            _pad: rounds.max(1),
        };
        let params_buffer =
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Persistent panel stress dispatch params"),
                    contents: bytemuck::bytes_of(&params),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Persistent panel stress bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: a_buffer,
                        offset: a_offset,
                        size: Some(a_binding_size),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: b_buffer,
                        offset: b_offset,
                        size: Some(b_binding_size),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: scratch_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        Ok(PanelDispatch {
            bind_group,
            rows,
            cols: panel.cols,
            _params_buffer: params_buffer,
        })
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
        let chunk_rows = gpu_dispatch_chunk_rows(n, gpu_intensity);
        let min_chunk_rows = gpu_min_dispatch_rows(gpu_intensity).min(chunk_rows).max(1);
        let max_chunk_count = n.div_ceil(min_chunk_rows);
        let mut governor = GpuWorkGovernor::new(chunk_rows, min_chunk_rows, n, gpu_intensity);
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
            let mut dispatches = Vec::new();
            let batch_limit = gpu_dispatch_batch_limit(gpu_intensity);
            while row_offset < n && dispatches.len() < batch_limit {
                let rows_this_chunk = governor.row_extent(n - row_offset);
                let params = Params {
                    n: n_u32,
                    row_offset: row_offset as u32,
                    row_count: rows_this_chunk as u32,
                    _pad2: 0,
                };
                dispatches.push(self.create_direct_dispatch(
                    &a_buffer,
                    &b_buffer,
                    &c_buffer,
                    params,
                    rows_this_chunk,
                ));
                row_offset += rows_this_chunk;
            }

            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Matrix multiplication batch encoder"),
                });

            if let Some(query_set) = query_set.as_ref() {
                for (batch_index, dispatch) in dispatches.iter().enumerate() {
                    let timestamp_index = chunk_index + batch_index;
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("Matrix multiplication timed batch pass"),
                        timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                            query_set,
                            beginning_of_pass_write_index: Some((timestamp_index * 2) as u32),
                            end_of_pass_write_index: Some((timestamp_index * 2 + 1) as u32),
                        }),
                    });
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(0, &dispatch.bind_group, &[]);
                    let groups_x = gpu_column_workgroups(n_u32);
                    let groups_y = (dispatch.rows as u32).div_ceil(TILE_SIZE);
                    pass.dispatch_workgroups(groups_x, groups_y, 1);
                }
            } else {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Matrix multiplication batched pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.pipeline);
                for dispatch in &dispatches {
                    pass.set_bind_group(0, &dispatch.bind_group, &[]);
                    let groups_x = gpu_column_workgroups(n_u32);
                    let groups_y = (dispatch.rows as u32).div_ceil(TILE_SIZE);
                    pass.dispatch_workgroups(groups_x, groups_y, 1);
                }
            }

            let dispatch_count = dispatches.len().max(1);
            let dispatch_start = Instant::now();
            let submission = self.queue.submit([encoder.finish()]);
            self.wait_for_submission(submission, cancel, "waiting for GPU matrix batch to finish")?;
            let observed_batch_ms = dispatch_start.elapsed().as_secs_f64() * 1000.0;
            let observed_ms = observed_batch_ms / dispatch_count as f64;
            for _ in 0..dispatch_count {
                observed_dispatch_ms.push(observed_ms);
            }
            governor.record_dispatch(observed_ms);
            governor.record_batch(observed_batch_ms);
            chunk_index += dispatch_count;
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
        let initial_row_block = gpu_dispatch_chunk_rows(n, gpu_intensity)
            .min(row_block)
            .max(min_row_block);
        let mut governor =
            GpuWorkGovernor::new(initial_row_block, min_row_block, row_block, gpu_intensity);

        if let Some(progress) = progress.as_deref_mut() {
            progress.gpu_started = Some(Instant::now());
            progress.set_phase(
                format!("GPU persistent panel compute ({initial_row_block}-{row_block}x{col_block})"),
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
        let mut panel_index = 0usize;
        let mut row_offset = 0usize;
        while panel_index < panels.len() {
            self.check_gpu_canceled(cancel)?;
            let mut dispatches = Vec::new();
            let batch_limit = gpu_dispatch_batch_limit(gpu_intensity);
            while panel_index < panels.len() && dispatches.len() < batch_limit {
                let panel = &panels[panel_index];
                let rows = governor.row_extent(n - row_offset);
                dispatches.push(self.create_panel_dispatch(
                    &a_buffer,
                    &b_buffer,
                    &c_buffer,
                    n_u32,
                    n,
                    panel,
                    row_offset,
                    rows,
                )?);
                row_offset += rows;
                if row_offset >= n {
                    row_offset = 0;
                    panel_index += 1;
                }
            }

            let mut encoder =
                self.device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("Persistent panel matrix batch encoder"),
                    });
            if let Some(query_set) = query_set.as_ref() {
                for (batch_index, dispatch) in dispatches.iter().enumerate() {
                    let timestamp_index = query_pair_index + batch_index;
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("Persistent panel matrix timed batch pass"),
                        timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                            query_set,
                            beginning_of_pass_write_index: Some((timestamp_index * 2) as u32),
                            end_of_pass_write_index: Some((timestamp_index * 2 + 1) as u32),
                        }),
                    });
                    pass.set_pipeline(&self.blocked_pipeline);
                    pass.set_bind_group(0, &dispatch.bind_group, &[]);
                    pass.dispatch_workgroups(
                        gpu_column_workgroups(dispatch.cols as u32),
                        (dispatch.rows as u32).div_ceil(TILE_SIZE),
                        1,
                    );
                }
            } else {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Persistent panel matrix batched pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.blocked_pipeline);
                for dispatch in &dispatches {
                    pass.set_bind_group(0, &dispatch.bind_group, &[]);
                    pass.dispatch_workgroups(
                        gpu_column_workgroups(dispatch.cols as u32),
                        (dispatch.rows as u32).div_ceil(TILE_SIZE),
                        1,
                    );
                }
            }

            let dispatch_count = dispatches.len().max(1);
            let completed_in_batch = dispatches
                .iter()
                .map(|dispatch| dispatch.rows * dispatch.cols)
                .sum::<usize>();
            let dispatch_start = Instant::now();
            let submission = self.queue.submit([encoder.finish()]);
            self.wait_for_submission(
                submission,
                cancel,
                "waiting for persistent panel matrix batch",
            )?;
            let observed_batch_ms = dispatch_start.elapsed().as_secs_f64() * 1000.0;
            let observed_ms = observed_batch_ms / dispatch_count as f64;
            for _ in 0..dispatch_count {
                observed_dispatch_ms.push(observed_ms);
            }
            governor.record_dispatch(observed_ms);
            governor.record_batch(observed_batch_ms);
            query_pair_index += dispatch_count;
            completed_cells += completed_in_batch;

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
            format!("{initial_row_block}-{row_block}x{col_block}"),
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

    fn repeat_gpu_compute<F>(
        &self,
        n: usize,
        a: &[f32],
        b: &[f32],
        gpu_intensity: GpuIntensity,
        stress_gpu_backend: StressGpuBackend,
        cancel: &AtomicBool,
        deadline: &Option<Instant>,
        emit: &mut F,
    ) -> Result<RepeatProgress>
    where
        F: FnMut(u64, f64, f64, f64, u64, bool, bool) -> RepeatProgress,
    {
        let elements = n
            .checked_mul(n)
            .ok_or_else(|| anyhow!("matrix size overflow"))?;
        if a.len() != elements || b.len() != elements {
            return Err(anyhow!("matrix data length does not match {n}x{n}"));
        }
        let n_u32 = u32::try_from(n).context("matrix size exceeds GPU shader limits")?;
        let byte_len = buffer_len_bytes(elements)?;

        if n == 4 && stress_gpu_backend == StressGpuBackend::Optimized {
            return self.repeat_register_tiny_gpu_compute(
                n_u32,
                a,
                b,
                gpu_intensity,
                cancel,
                deadline,
                emit,
            );
        }

        if n <= GPU_TINY_STRESS_MAX_SIZE {
            return self.repeat_tiny_gpu_compute(
                n,
                n_u32,
                a,
                b,
                gpu_intensity,
                cancel,
                deadline,
                emit,
            );
        }

        if self.needs_blocked_path(byte_len) {
            if self.can_use_panelized_path(n, byte_len, gpu_intensity)? {
                return self.repeat_panelized_gpu_compute(
                    n,
                    n_u32,
                    a,
                    b,
                    byte_len,
                    gpu_intensity,
                    cancel,
                    deadline,
                    emit,
                );
            }
            return self.repeat_streaming_gpu_compute(
                n,
                a,
                b,
                gpu_intensity,
                cancel,
                deadline,
                emit,
            );
        }

        self.repeat_direct_gpu_compute(
            n,
            n_u32,
            a,
            b,
            byte_len,
            gpu_intensity,
            cancel,
            deadline,
            emit,
        )
    }

    fn repeat_register_tiny_gpu_compute<F>(
        &self,
        n_u32: u32,
        a: &[f32],
        b: &[f32],
        gpu_intensity: GpuIntensity,
        cancel: &AtomicBool,
        deadline: &Option<Instant>,
        emit: &mut F,
    ) -> Result<RepeatProgress>
    where
        F: FnMut(u64, f64, f64, f64, u64, bool, bool) -> RepeatProgress,
    {
        let a_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Register tiny stress matrix A"),
                contents: bytemuck::cast_slice(a),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let b_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Register tiny stress matrix B"),
                contents: bytemuck::cast_slice(b),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let workgroups = gpu_register_tiny_stress_workgroups(gpu_intensity);
        let scratch_elements = (workgroups as usize)
            .checked_mul(GPU_TINY_STRESS_LANES_PER_WORKGROUP)
            .ok_or_else(|| anyhow!("register tiny stress scratch buffer size overflow"))?;
        let scratch_bytes = buffer_len_bytes(scratch_elements)?;
        self.ensure_block_buffer_fits("Register tiny stress scratch output", scratch_bytes)?;
        let scratch_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Register tiny stress scratch output"),
            size: scratch_bytes,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let params = Params {
            n: n_u32,
            row_offset: 0,
            row_count: gpu_register_tiny_stress_rounds(gpu_intensity),
            _pad2: 0,
        };
        let dispatch = self.create_direct_dispatch(
            &a_buffer,
            &b_buffer,
            &scratch_buffer,
            params,
            scratch_elements,
        );
        let equivalent_iterations =
            register_tiny_stress_equivalent_iterations(scratch_elements, params.row_count);
        let max_batch_limit = gpu_register_tiny_stress_batch_limit(gpu_intensity).max(1);
        let mut batch_limit = max_batch_limit;
        let mut counters = GpuRepeatCounters::default();

        while repeat_should_continue(deadline, cancel) {
            if let Err(err) = self.check_gpu_canceled(Some(cancel)) {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                return Err(err);
            }

            let mut encoder =
                self.device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("Register tiny matrix stress batch encoder"),
                    });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Register tiny matrix stress batch pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.register_tiny_stress_pipeline);
                pass.set_bind_group(0, &dispatch.bind_group, &[]);
                for _ in 0..batch_limit {
                    pass.dispatch_workgroups(workgroups, 1, 1);
                }
            }

            let batch_start = Instant::now();
            let submission = self.queue.submit([encoder.finish()]);
            if let Err(err) = self.wait_for_submission(
                submission,
                Some(cancel),
                "waiting for register tiny matrix GPU stress batch",
            ) {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                return Err(err);
            }

            let batch_ms = batch_start.elapsed().as_secs_f64() * 1000.0;
            let completed_iterations =
                (batch_limit as u64).saturating_mul(equivalent_iterations);
            counters.record_batch(completed_iterations, batch_ms);
            if batch_ms > gpu_hard_batch_backoff_ms(gpu_intensity) {
                batch_limit = (batch_limit / 2).max(1);
            } else if batch_ms < gpu_target_low_ms(gpu_intensity) && batch_limit < max_batch_limit {
                batch_limit = (batch_limit * 2).min(max_batch_limit);
            }
            emit(
                counters.iterations,
                counters.latest_ms,
                counters.total_ms,
                counters.total_compute_ms,
                counters.compute_count,
                false,
                false,
            );
            if repeat_should_continue(deadline, cancel)
                && let Err(err) = pause_between_gpu_submissions(gpu_intensity, Some(cancel))
            {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                return Err(err);
            }
        }

        Ok(emit(
            counters.iterations,
            counters.latest_ms,
            counters.total_ms,
            counters.total_compute_ms,
            counters.compute_count,
            cancel.load(Ordering::Relaxed),
            true,
        ))
    }

    fn repeat_tiny_gpu_compute<F>(
        &self,
        n: usize,
        n_u32: u32,
        a: &[f32],
        b: &[f32],
        gpu_intensity: GpuIntensity,
        cancel: &AtomicBool,
        deadline: &Option<Instant>,
        emit: &mut F,
    ) -> Result<RepeatProgress>
    where
        F: FnMut(u64, f64, f64, f64, u64, bool, bool) -> RepeatProgress,
    {
        let a_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Tiny stress matrix A"),
                contents: bytemuck::cast_slice(a),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let b_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Tiny stress matrix B"),
                contents: bytemuck::cast_slice(b),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let workgroups = gpu_tiny_stress_workgroups(gpu_intensity);
        let scratch_elements = (workgroups as usize)
            .checked_mul(GPU_TINY_STRESS_LANES_PER_WORKGROUP)
            .ok_or_else(|| anyhow!("tiny stress scratch buffer size overflow"))?;
        let scratch_bytes = buffer_len_bytes(scratch_elements)?;
        self.ensure_block_buffer_fits("Tiny stress scratch output", scratch_bytes)?;
        let scratch_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tiny stress scratch output"),
            size: scratch_bytes,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let params = Params {
            n: n_u32,
            row_offset: 0,
            row_count: gpu_tiny_stress_rounds(n, gpu_intensity),
            _pad2: 0,
        };
        let dispatch = self.create_direct_dispatch(
            &a_buffer,
            &b_buffer,
            &scratch_buffer,
            params,
            scratch_elements,
        );
        let equivalent_iterations =
            tiny_stress_equivalent_iterations(scratch_elements, n, params.row_count);
        let max_batch_limit = gpu_tiny_stress_batch_limit(gpu_intensity).max(1);
        let mut batch_limit = max_batch_limit;
        let mut counters = GpuRepeatCounters::default();

        while repeat_should_continue(deadline, cancel) {
            if let Err(err) = self.check_gpu_canceled(Some(cancel)) {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                return Err(err);
            }

            let mut encoder =
                self.device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("Tiny matrix stress batch encoder"),
                    });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Tiny matrix stress batch pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.tiny_stress_pipeline);
                pass.set_bind_group(0, &dispatch.bind_group, &[]);
                for _ in 0..batch_limit {
                    pass.dispatch_workgroups(workgroups, 1, 1);
                }
            }

            let batch_start = Instant::now();
            let submission = self.queue.submit([encoder.finish()]);
            if let Err(err) = self.wait_for_submission(
                submission,
                Some(cancel),
                "waiting for tiny matrix GPU stress batch",
            ) {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                return Err(err);
            }

            let batch_ms = batch_start.elapsed().as_secs_f64() * 1000.0;
            let completed_iterations =
                (batch_limit as u64).saturating_mul(equivalent_iterations);
            counters.record_batch(completed_iterations, batch_ms);
            if batch_ms > gpu_hard_batch_backoff_ms(gpu_intensity) {
                batch_limit = (batch_limit / 2).max(1);
            } else if batch_ms < gpu_target_low_ms(gpu_intensity) && batch_limit < max_batch_limit {
                batch_limit = (batch_limit * 2).min(max_batch_limit);
            }
            emit(
                counters.iterations,
                counters.latest_ms,
                counters.total_ms,
                counters.total_compute_ms,
                counters.compute_count,
                false,
                false,
            );
            if repeat_should_continue(deadline, cancel)
                && let Err(err) = pause_between_gpu_submissions(gpu_intensity, Some(cancel))
            {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                return Err(err);
            }
        }

        Ok(emit(
            counters.iterations,
            counters.latest_ms,
            counters.total_ms,
            counters.total_compute_ms,
            counters.compute_count,
            cancel.load(Ordering::Relaxed),
            true,
        ))
    }

    fn repeat_direct_gpu_compute<F>(
        &self,
        n: usize,
        n_u32: u32,
        a: &[f32],
        b: &[f32],
        byte_len: u64,
        gpu_intensity: GpuIntensity,
        cancel: &AtomicBool,
        deadline: &Option<Instant>,
        emit: &mut F,
    ) -> Result<RepeatProgress>
    where
        F: FnMut(u64, f64, f64, f64, u64, bool, bool) -> RepeatProgress,
    {
        let a_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Stress matrix A"),
                contents: bytemuck::cast_slice(a),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let b_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Stress matrix B"),
                contents: bytemuck::cast_slice(b),
                usage: wgpu::BufferUsages::STORAGE,
            });

        if n > GPU_TINY_STRESS_MAX_SIZE {
            return self.repeat_dense_direct_gpu_compute(
                n,
                n_u32,
                &a_buffer,
                &b_buffer,
                gpu_intensity,
                cancel,
                deadline,
                emit,
            );
        }

        let c_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Stress matrix C output"),
            size: byte_len,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        self.repeat_direct_full_gpu_compute(
            n,
            n_u32,
            &a_buffer,
            &b_buffer,
            &c_buffer,
            gpu_intensity,
            cancel,
            deadline,
            emit,
        )
    }

    fn repeat_dense_direct_gpu_compute<F>(
        &self,
        n: usize,
        n_u32: u32,
        a_buffer: &wgpu::Buffer,
        b_buffer: &wgpu::Buffer,
        gpu_intensity: GpuIntensity,
        cancel: &AtomicBool,
        deadline: &Option<Instant>,
        emit: &mut F,
    ) -> Result<RepeatProgress>
    where
        F: FnMut(u64, f64, f64, f64, u64, bool, bool) -> RepeatProgress,
    {
        let workgroups = gpu_dense_stress_workgroups(gpu_intensity);
        let scratch_elements = (workgroups as usize)
            .checked_mul(GPU_TINY_STRESS_LANES_PER_WORKGROUP)
            .ok_or_else(|| anyhow!("dense stress scratch buffer size overflow"))?;
        let scratch_bytes = buffer_len_bytes(scratch_elements)?;
        self.ensure_block_buffer_fits("Dense stress scratch output", scratch_bytes)?;
        let scratch_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Dense stress scratch output"),
            size: scratch_bytes,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let params = Params {
            n: n_u32,
            row_offset: 0,
            row_count: gpu_dense_stress_rounds(n, gpu_intensity),
            _pad2: 0,
        };
        let dispatch = self.create_direct_dispatch(
            a_buffer,
            b_buffer,
            &scratch_buffer,
            params,
            scratch_elements,
        );
        let equivalent_iterations =
            dense_stress_equivalent_iterations(scratch_elements, n, params.row_count);
        let max_batch_limit = gpu_dense_stress_batch_limit(gpu_intensity).max(1);
        let mut batch_limit = max_batch_limit;
        let mut counters = GpuRepeatCounters::default();

        while repeat_should_continue(deadline, cancel) {
            if let Err(err) = self.check_gpu_canceled(Some(cancel)) {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                return Err(err);
            }

            let mut encoder =
                self.device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("Dense matrix stress batch encoder"),
                    });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Dense matrix stress batch pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.tiny_stress_pipeline);
                pass.set_bind_group(0, &dispatch.bind_group, &[]);
                for _ in 0..batch_limit {
                    pass.dispatch_workgroups(workgroups, 1, 1);
                }
            }

            let batch_start = Instant::now();
            let submission = self.queue.submit([encoder.finish()]);
            if let Err(err) = self.wait_for_submission(
                submission,
                Some(cancel),
                "waiting for dense matrix GPU stress batch",
            ) {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                return Err(err);
            }

            let batch_ms = batch_start.elapsed().as_secs_f64() * 1000.0;
            let completed_iterations =
                (batch_limit as u64).saturating_mul(equivalent_iterations);
            counters.record_batch(completed_iterations, batch_ms);
            if batch_ms > gpu_hard_batch_backoff_ms(gpu_intensity) {
                batch_limit = (batch_limit / 2).max(1);
            } else if batch_ms < gpu_target_low_ms(gpu_intensity) && batch_limit < max_batch_limit {
                batch_limit = (batch_limit * 2).min(max_batch_limit);
            }
            emit(
                counters.iterations,
                counters.latest_ms,
                counters.total_ms,
                counters.total_compute_ms,
                counters.compute_count,
                false,
                false,
            );
            if repeat_should_continue(deadline, cancel)
                && let Err(err) = pause_between_gpu_submissions(gpu_intensity, Some(cancel))
            {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                return Err(err);
            }
        }

        Ok(emit(
            counters.iterations,
            counters.latest_ms,
            counters.total_ms,
            counters.total_compute_ms,
            counters.compute_count,
            cancel.load(Ordering::Relaxed),
            true,
        ))
    }

    fn repeat_direct_full_gpu_compute<F>(
        &self,
        n: usize,
        n_u32: u32,
        a_buffer: &wgpu::Buffer,
        b_buffer: &wgpu::Buffer,
        c_buffer: &wgpu::Buffer,
        gpu_intensity: GpuIntensity,
        cancel: &AtomicBool,
        deadline: &Option<Instant>,
        emit: &mut F,
    ) -> Result<RepeatProgress>
    where
        F: FnMut(u64, f64, f64, f64, u64, bool, bool) -> RepeatProgress,
    {
        let params = Params {
            n: n_u32,
            row_offset: 0,
            row_count: n_u32,
            _pad2: 0,
        };
        let dispatch = self.create_direct_dispatch(a_buffer, b_buffer, c_buffer, params, n);
        let groups_x = gpu_column_workgroups(n_u32);
        let groups_y = n_u32.div_ceil(TILE_SIZE);
        let max_batch_limit = gpu_stress_repeat_batch_limit(n, gpu_intensity).max(1);
        let mut batch_limit = max_batch_limit;
        let mut counters = GpuRepeatCounters::default();

        while repeat_should_continue(deadline, cancel) {
            if let Err(err) = self.check_gpu_canceled(Some(cancel)) {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                return Err(err);
            }

            let mut encoder =
                self.device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("Matrix stress full-direct batch encoder"),
                    });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Matrix stress full-direct batch pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &dispatch.bind_group, &[]);
                for _ in 0..batch_limit {
                    pass.dispatch_workgroups(groups_x, groups_y, 1);
                }
            }

            let batch_start = Instant::now();
            let submission = self.queue.submit([encoder.finish()]);
            if let Err(err) = self.wait_for_submission(
                submission,
                Some(cancel),
                "waiting for repeated full-matrix GPU stress batch",
            ) {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                return Err(err);
            }

            let batch_ms = batch_start.elapsed().as_secs_f64() * 1000.0;
            counters.record_batch(batch_limit as u64, batch_ms);
            if batch_ms > gpu_hard_batch_backoff_ms(gpu_intensity) {
                batch_limit = (batch_limit / 2).max(1);
            } else if batch_ms < gpu_target_low_ms(gpu_intensity) && batch_limit < max_batch_limit {
                batch_limit = (batch_limit * 2).min(max_batch_limit);
            }
            emit(
                counters.iterations,
                counters.latest_ms,
                counters.total_ms,
                counters.total_compute_ms,
                counters.compute_count,
                false,
                false,
            );
            if repeat_should_continue(deadline, cancel)
                && let Err(err) = pause_between_gpu_submissions(gpu_intensity, Some(cancel))
            {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                return Err(err);
            }
        }

        Ok(emit(
            counters.iterations,
            counters.latest_ms,
            counters.total_ms,
            counters.total_compute_ms,
            counters.compute_count,
            cancel.load(Ordering::Relaxed),
            true,
        ))
    }

    fn repeat_panelized_gpu_compute<F>(
        &self,
        n: usize,
        n_u32: u32,
        a: &[f32],
        b: &[f32],
        _byte_len: u64,
        gpu_intensity: GpuIntensity,
        cancel: &AtomicBool,
        deadline: &Option<Instant>,
        emit: &mut F,
    ) -> Result<RepeatProgress>
    where
        F: FnMut(u64, f64, f64, f64, u64, bool, bool) -> RepeatProgress,
    {
        let (row_block, col_block) = self.stress_block_dimensions(n, gpu_intensity)?;
        let min_row_block =
            align_block_extent(gpu_min_dispatch_rows(gpu_intensity).min(row_block).max(1));
        let initial_row_block = row_block.max(min_row_block);
        let mut governor =
            GpuWorkGovernor::new(initial_row_block, min_row_block, row_block, gpu_intensity);
        let (b_packed, panels) = pack_column_panels(b, n, col_block, Some(cancel))?;
        self.ensure_panelized_offsets_aligned(n, min_row_block, &panels)?;
        self.ensure_panelized_offsets_aligned(n, row_block, &panels)?;

        let a_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Stress persistent matrix A"),
                contents: bytemuck::cast_slice(a),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let b_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Stress persistent packed matrix B panels"),
                contents: bytemuck::cast_slice(&b_packed),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let workgroups = gpu_dense_stress_workgroups(gpu_intensity);
        let scratch_elements = (workgroups as usize)
            .checked_mul(GPU_TINY_STRESS_LANES_PER_WORKGROUP)
            .ok_or_else(|| anyhow!("panel stress scratch buffer size overflow"))?;
        let scratch_bytes = buffer_len_bytes(scratch_elements)?;
        self.ensure_block_buffer_fits("Panel stress scratch output", scratch_bytes)?;
        let scratch_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Panel stress scratch output"),
            size: scratch_bytes,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let rounds = gpu_dense_stress_rounds(n, gpu_intensity);
        let work_cells_per_dispatch =
            (scratch_elements as u128).saturating_mul(rounds as u128);

        let elements = n
            .checked_mul(n)
            .ok_or_else(|| anyhow!("matrix size overflow"))?;
        let elements_u128 = elements as u128;
        let mut counters = GpuRepeatCounters::default();
        let mut completed_work_cells = 0u128;
        let mut panel_index = 0usize;
        let mut row_offset = 0usize;

        while repeat_should_continue(deadline, cancel) {
            if let Err(err) = self.check_gpu_canceled(Some(cancel)) {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                return Err(err);
            }
            let mut dispatches = Vec::new();
            let batch_limit = gpu_dense_stress_batch_limit(gpu_intensity);
            while repeat_should_continue(deadline, cancel) && dispatches.len() < batch_limit {
                let panel = &panels[panel_index];
                let rows = governor.row_extent(n - row_offset);
                let dispatch = self.create_panel_stress_dispatch(
                    &a_buffer,
                    &b_buffer,
                    &scratch_buffer,
                    n_u32,
                    n,
                    panel,
                    row_offset,
                    rows,
                    rounds,
                )?;
                dispatches.push(dispatch);
                row_offset += rows;
                if row_offset >= n {
                    row_offset = 0;
                    panel_index += 1;
                }
                if panel_index >= panels.len() {
                    panel_index = 0;
                }
            }
            if dispatches.is_empty() {
                break;
            }

            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Matrix stress panel batch encoder"),
                });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Matrix stress panel batch pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.panel_stress_pipeline);
                for dispatch in &dispatches {
                    pass.set_bind_group(0, &dispatch.bind_group, &[]);
                    pass.dispatch_workgroups(workgroups, 1, 1);
                }
            }

            let dispatch_count = dispatches.len().max(1);
            let batch_start = Instant::now();
            let submission = self.queue.submit([encoder.finish()]);
            if let Err(err) = self.wait_for_submission(
                submission,
                Some(cancel),
                "waiting for persistent GPU stress batch",
            ) {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                return Err(err);
            }
            let batch_ms = batch_start.elapsed().as_secs_f64() * 1000.0;
            let observed_ms = batch_ms / dispatch_count as f64;
            governor.record_dispatch(observed_ms);
            governor.record_batch(batch_ms);
            completed_work_cells = completed_work_cells
                .saturating_add(work_cells_per_dispatch.saturating_mul(dispatch_count as u128));
            let completed_iterations =
                (completed_work_cells / elements_u128).min(u64::MAX as u128) as u64;
            completed_work_cells %= elements_u128;
            counters.record_batch(completed_iterations, batch_ms);
            emit(
                counters.iterations,
                counters.latest_ms,
                counters.total_ms,
                counters.total_compute_ms,
                counters.compute_count,
                false,
                false,
            );
            if repeat_should_continue(deadline, cancel)
                && let Err(err) = pause_between_gpu_submissions(gpu_intensity, Some(cancel))
            {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                return Err(err);
            }
        }

        Ok(emit(
            counters.iterations,
            counters.latest_ms,
            counters.total_ms,
            counters.total_compute_ms,
            counters.compute_count,
            cancel.load(Ordering::Relaxed),
            true,
        ))
    }

    fn repeat_streaming_gpu_compute<F>(
        &self,
        n: usize,
        a: &[f32],
        b: &[f32],
        gpu_intensity: GpuIntensity,
        cancel: &AtomicBool,
        deadline: &Option<Instant>,
        emit: &mut F,
    ) -> Result<RepeatProgress>
    where
        F: FnMut(u64, f64, f64, f64, u64, bool, bool) -> RepeatProgress,
    {
        let mut iterations = 0_u64;
        let mut total_ms = 0.0;
        let mut total_compute_ms = 0.0;
        let mut compute_count = 0_u64;
        let mut latest_ms = 0.0;

        while repeat_should_continue(deadline, cancel) {
            let timing = match self.multiply_cancelable(
                n,
                a,
                b,
                true,
                gpu_intensity,
                Some(cancel),
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
                gpu_column_workgroups(cols_u32),
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

    fn stress_block_dimensions(
        &self,
        n: usize,
        gpu_intensity: GpuIntensity,
    ) -> Result<(usize, usize)> {
        self.block_dimensions_with_targets(n, gpu_stress_block_targets(gpu_intensity))
    }

    fn block_dimensions(&self, n: usize, gpu_intensity: GpuIntensity) -> Result<(usize, usize)> {
        self.block_dimensions_with_targets(n, gpu_block_targets(gpu_intensity))
    }

    fn block_dimensions_with_targets(
        &self,
        n: usize,
        targets: (usize, usize),
    ) -> Result<(usize, usize)> {
        let limit_bytes = self
            .max_storage_buffer_binding_size
            .min(self.max_buffer_size)
            .max(std::mem::size_of::<f32>() as u64);
        let limit_floats = (limit_bytes / std::mem::size_of::<f32>() as u64) as usize;
        let max_rows_or_cols = (limit_floats / n).max(1);
        let (target_rows, target_cols) = targets;
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

fn gpu_stress_block_targets(gpu_intensity: GpuIntensity) -> (usize, usize) {
    match gpu_intensity {
        GpuIntensity::Safe => (GPU_HIGH_BLOCK_ROWS, GPU_HIGH_BLOCK_COLS),
        GpuIntensity::Balanced => (GPU_HIGH_BLOCK_ROWS * 2, GPU_HIGH_BLOCK_COLS),
        GpuIntensity::High => (GPU_HIGH_BLOCK_ROWS * 2, GPU_HIGH_BLOCK_COLS * 2),
    }
}

fn gpu_column_workgroups(cols: u32) -> u32 {
    cols.div_ceil(TILE_SIZE * GPU_SHADER_COLS_PER_THREAD)
}

fn gpu_dispatch_batch_limit(gpu_intensity: GpuIntensity) -> usize {
    match gpu_intensity {
        GpuIntensity::Safe => GPU_SAFE_BATCH_DISPATCHES,
        GpuIntensity::Balanced => GPU_BALANCED_BATCH_DISPATCHES,
        GpuIntensity::High => GPU_HIGH_BATCH_DISPATCHES,
    }
}

fn gpu_repeat_batch_limit(size: usize, gpu_intensity: GpuIntensity) -> usize {
    let base = match gpu_intensity {
        GpuIntensity::Safe => GPU_SAFE_REPEAT_BATCH_DISPATCHES,
        GpuIntensity::Balanced => GPU_BALANCED_REPEAT_BATCH_DISPATCHES,
        GpuIntensity::High => GPU_HIGH_REPEAT_BATCH_DISPATCHES,
    };

    if size <= 256 {
        base
    } else if size <= 512 {
        (base / 2).max(gpu_dispatch_batch_limit(gpu_intensity))
    } else if size <= 1024 {
        (base / 4).max(gpu_dispatch_batch_limit(gpu_intensity))
    } else {
        gpu_dispatch_batch_limit(gpu_intensity)
    }
}

fn gpu_stress_repeat_batch_limit(size: usize, gpu_intensity: GpuIntensity) -> usize {
    if size <= 1024 {
        return gpu_repeat_batch_limit(size, gpu_intensity);
    }

    let base = if size <= 2048 {
        match gpu_intensity {
            GpuIntensity::Safe => 32,
            GpuIntensity::Balanced => 128,
            GpuIntensity::High => 256,
        }
    } else if size <= 4096 {
        match gpu_intensity {
            GpuIntensity::Safe => 8,
            GpuIntensity::Balanced => 32,
            GpuIntensity::High => 64,
        }
    } else if size <= 8192 {
        match gpu_intensity {
            GpuIntensity::Safe => 2,
            GpuIntensity::Balanced => 8,
            GpuIntensity::High => 16,
        }
    } else {
        match gpu_intensity {
            GpuIntensity::Safe => 1,
            GpuIntensity::Balanced => 4,
            GpuIntensity::High => 8,
        }
    };

    if size <= 8192 {
        base.max(gpu_dispatch_batch_limit(gpu_intensity))
    } else {
        base.max(1)
    }
}

fn gpu_tiny_stress_workgroups(gpu_intensity: GpuIntensity) -> u32 {
    match gpu_intensity {
        GpuIntensity::Safe => GPU_SAFE_TINY_STRESS_WORKGROUPS,
        GpuIntensity::Balanced => GPU_BALANCED_TINY_STRESS_WORKGROUPS,
        GpuIntensity::High => GPU_HIGH_TINY_STRESS_WORKGROUPS,
    }
}

fn gpu_tiny_stress_rounds(size: usize, gpu_intensity: GpuIntensity) -> u32 {
    let base = match gpu_intensity {
        GpuIntensity::Safe => GPU_SAFE_TINY_STRESS_ROUNDS,
        GpuIntensity::Balanced => GPU_BALANCED_TINY_STRESS_ROUNDS,
        GpuIntensity::High => GPU_HIGH_TINY_STRESS_ROUNDS,
    };
    let scaled = (u64::from(base) * u64::from(TILE_SIZE)).div_ceil(size.max(1) as u64);
    scaled.clamp(1, u64::from(u32::MAX)) as u32
}

fn gpu_register_tiny_stress_workgroups(gpu_intensity: GpuIntensity) -> u32 {
    gpu_tiny_stress_workgroups(gpu_intensity)
}

fn gpu_register_tiny_stress_rounds(gpu_intensity: GpuIntensity) -> u32 {
    gpu_tiny_stress_rounds(4, gpu_intensity)
}

fn gpu_dense_stress_workgroups(gpu_intensity: GpuIntensity) -> u32 {
    match gpu_intensity {
        GpuIntensity::Safe => GPU_SAFE_DENSE_STRESS_WORKGROUPS,
        GpuIntensity::Balanced => GPU_BALANCED_DENSE_STRESS_WORKGROUPS,
        GpuIntensity::High => GPU_HIGH_DENSE_STRESS_WORKGROUPS,
    }
}

fn gpu_dense_stress_rounds(size: usize, gpu_intensity: GpuIntensity) -> u32 {
    let target_k = match gpu_intensity {
        GpuIntensity::Safe => GPU_SAFE_DENSE_STRESS_TARGET_K,
        GpuIntensity::Balanced => GPU_BALANCED_DENSE_STRESS_TARGET_K,
        GpuIntensity::High => GPU_HIGH_DENSE_STRESS_TARGET_K,
    };
    u64::from(target_k)
        .div_ceil(size.max(1) as u64)
        .clamp(1, u64::from(u32::MAX)) as u32
}

fn dense_stress_equivalent_iterations(
    scratch_elements: usize,
    size: usize,
    rounds: u32,
) -> u64 {
    let cells_per_matrix = size
        .checked_mul(size)
        .map(|cells| cells.max(1) as u128)
        .unwrap_or(u128::MAX);
    let repeated_cells = (scratch_elements as u128).saturating_mul(rounds as u128);
    let equivalent_iterations = (repeated_cells / cells_per_matrix).max(1);
    equivalent_iterations.min(u64::MAX as u128) as u64
}

fn gpu_dense_stress_batch_limit(gpu_intensity: GpuIntensity) -> usize {
    match gpu_intensity {
        GpuIntensity::Safe => GPU_SAFE_DENSE_STRESS_BATCH_DISPATCHES,
        GpuIntensity::Balanced => GPU_BALANCED_DENSE_STRESS_BATCH_DISPATCHES,
        GpuIntensity::High => GPU_HIGH_DENSE_STRESS_BATCH_DISPATCHES,
    }
}

fn tiny_stress_equivalent_iterations(
    scratch_elements: usize,
    size: usize,
    rounds: u32,
) -> u64 {
    let cells_per_matrix = size
        .checked_mul(size)
        .map(|cells| cells.max(1) as u128)
        .unwrap_or(u128::MAX);
    let repeated_cells = (scratch_elements as u128).saturating_mul(rounds as u128);
    let equivalent_iterations = (repeated_cells / cells_per_matrix).max(1);
    equivalent_iterations.min(u64::MAX as u128) as u64
}

fn register_tiny_stress_equivalent_iterations(
    scratch_elements: usize,
    rounds: u32,
) -> u64 {
    let equivalent_iterations = (scratch_elements as u128).saturating_mul(rounds as u128);
    equivalent_iterations.min(u64::MAX as u128) as u64
}

fn gpu_tiny_stress_batch_limit(gpu_intensity: GpuIntensity) -> usize {
    match gpu_intensity {
        GpuIntensity::Safe => GPU_SAFE_TINY_STRESS_BATCH_DISPATCHES,
        GpuIntensity::Balanced => GPU_BALANCED_TINY_STRESS_BATCH_DISPATCHES,
        GpuIntensity::High => GPU_HIGH_TINY_STRESS_BATCH_DISPATCHES,
    }
}

fn gpu_register_tiny_stress_batch_limit(_gpu_intensity: GpuIntensity) -> usize {
    1
}

fn gpu_submission_pause(gpu_intensity: GpuIntensity) -> Duration {
    match gpu_intensity {
        GpuIntensity::Safe => Duration::from_millis(1),
        GpuIntensity::Balanced => Duration::from_millis(0),
        GpuIntensity::High => Duration::from_millis(0),
    }
}

fn gpu_target_low_ms(gpu_intensity: GpuIntensity) -> f64 {
    match gpu_intensity {
        GpuIntensity::Safe => 35.0,
        GpuIntensity::Balanced => 80.0,
        GpuIntensity::High => 140.0,
    }
}

fn gpu_soft_backoff_ms(gpu_intensity: GpuIntensity) -> f64 {
    match gpu_intensity {
        GpuIntensity::Safe => 250.0,
        GpuIntensity::Balanced => 500.0,
        GpuIntensity::High => 750.0,
    }
}

fn gpu_hard_backoff_ms(gpu_intensity: GpuIntensity) -> f64 {
    match gpu_intensity {
        GpuIntensity::Safe => 500.0,
        GpuIntensity::Balanced => 750.0,
        GpuIntensity::High => 1000.0,
    }
}

fn gpu_hard_batch_backoff_ms(gpu_intensity: GpuIntensity) -> f64 {
    match gpu_intensity {
        GpuIntensity::Safe => 700.0,
        GpuIntensity::Balanced => 1100.0,
        GpuIntensity::High => 1600.0,
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
    ensure_matrix_host_memory_available(
        size,
        if estimate_cpu_time { 3 } else { 4 },
        "matrix benchmark",
    )?;
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
    stress_gpu_backend: StressGpuBackend,
    cancel: Arc<AtomicBool>,
    tx: Sender<WorkerEvent>,
    duration: RepeatDuration,
) -> Result<RepeatProgress> {
    let started = Instant::now();
    let deadline = duration.duration().map(|duration| started + duration);
    let duration_s = duration.seconds();
    let mut iterations = 0_u64;
    let mut total_ms = 0.0;
    let total_compute_ms = 0.0;
    let compute_count = 0_u64;
    let mut latest_ms = 0.0;
    let mut last_emit = Instant::now() - Duration::from_secs(1);

    let progress_tx = tx.clone();
    let mut emit = move |iterations: u64,
                         latest_ms: f64,
                         total_ms: f64,
                         total_compute_ms: f64,
                         compute_count: u64,
                         canceled: bool,
                         force: bool| {
        let now = Instant::now();
        let elapsed_s = match duration_s {
            Some(duration_s) => (now - started).as_secs_f64().min(duration_s),
            None => (now - started).as_secs_f64(),
        };
        let progress = RepeatProgress {
            mode,
            size,
            duration_s,
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
        if force || now.duration_since(last_emit) >= Duration::from_millis(PROGRESS_SAMPLE_MS) {
            let _ = progress_tx.send(WorkerEvent::RepeatProgress(progress.clone()));
            last_emit = now;
        }
        progress
    };

    match mode {
        RepeatMode::Cpu => {
            ensure_matrix_host_memory_available(size, 3, "CPU matrix stress test")?;
            let (a, b) = generate_matrices_cancelable(size, Some(&cancel))?;
            while repeat_should_continue(&deadline, &cancel) {
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
            if size == 4 && stress_gpu_backend == StressGpuBackend::Optimized {
                let _ = tx.send(WorkerEvent::Log(
                    "Trying PyTorch CUDA tensor-core equivalent stress backend for 4x4"
                        .to_owned(),
                ));
                match repeat_pytorch_cuda_tiny_matrix_stress(
                    size,
                    gpu_intensity,
                    &cancel,
                    &deadline,
                    &mut emit,
                ) {
                    Ok(Some(progress)) => {
                        let _ = tx.send(WorkerEvent::Log(
                            "4x4 stress used the PyTorch CUDA tensor-core equivalent backend"
                                .to_owned(),
                        ));
                        return Ok(progress);
                    }
                    Ok(None) => {
                        let _ = tx.send(WorkerEvent::Log(
                            "PyTorch CUDA 4x4 stress backend unavailable; falling back to WGPU register microkernel"
                                .to_owned(),
                        ));
                    }
                    Err(err) => return Err(err),
                }
            }
            if size > GPU_TINY_STRESS_MAX_SIZE {
                let _ = tx.send(WorkerEvent::Log(format!(
                    "Trying PyTorch CUDA/cuBLAS stress backend for {size}x{size}"
                )));
                match repeat_pytorch_cuda_matrix_stress(
                    size,
                    gpu_intensity,
                    &cancel,
                    &deadline,
                    &mut emit,
                ) {
                    Ok(Some(progress)) => {
                        let _ = tx.send(WorkerEvent::Log(
                            "Large matrix stress used the PyTorch CUDA/cuBLAS backend".to_owned(),
                        ));
                        return Ok(progress);
                    }
                    Ok(None) => {
                        let _ = tx.send(WorkerEvent::Log(
                            "PyTorch CUDA stress backend unavailable; falling back to WGPU"
                                .to_owned(),
                        ));
                    }
                    Err(err) => return Err(err),
                }
            }
            ensure_matrix_host_memory_available(size, 3, "WGPU GPU matrix stress fallback")?;
            let (a, b) = generate_matrices_cancelable(size, Some(&cancel))?;
            let runner = GpuRunner::new(adapter.index)?;
            return runner.repeat_gpu_compute(
                size,
                &a,
                &b,
                gpu_intensity,
                stress_gpu_backend,
                &cancel,
                &deadline,
                &mut emit,
            );
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

const PYTORCH_CUDA_MATRIX_STRESS_SCRIPT: &str = r#"
import argparse
import sys
import time
import traceback

def clean(value):
    return str(value).replace("\t", " ").replace("\n", " ")

def emit(key, *values):
    if values:
        print(key + "\t" + "\t".join(clean(value) for value in values), flush=True)
    else:
        print(key, flush=True)

parser = argparse.ArgumentParser()
parser.add_argument("--device", type=int, default=0)
parser.add_argument("--size", type=int, required=True)
parser.add_argument("--time-limit", type=float, required=True)
parser.add_argument("--intensity", choices=("safe", "balanced", "high"), default="balanced")
args = parser.parse_args()

try:
    import torch
except Exception as exc:
    emit("PYTORCH_MATRIX_UNAVAILABLE", type(exc).__name__ + ": " + str(exc))
    sys.exit(0)

try:
    emit("TORCH", getattr(torch, "__version__", ""))
    emit("CUDA", getattr(torch.version, "cuda", "") or "")
    available = bool(torch.cuda.is_available())
    emit("CUDA_AVAILABLE", available)
    count = int(torch.cuda.device_count()) if available else 0
    emit("DEVICE_COUNT", count)
    if not available:
        emit("PYTORCH_MATRIX_UNAVAILABLE", "torch.cuda.is_available() is false")
        sys.exit(0)
    if args.device < 0 or args.device >= count:
        emit("PYTORCH_MATRIX_UNAVAILABLE", "CUDA device {} is not available".format(args.device))
        sys.exit(0)
    if args.size <= 0:
        emit("ERROR", "matrix size must be positive")
        sys.exit(0)

    torch.cuda.set_device(args.device)
    device = torch.device("cuda:{}".format(args.device))
    props = torch.cuda.get_device_properties(args.device)
    emit("RESULT_GPU_NAME", props.name)
    emit("RESULT_DEVICE_INDEX", args.device)
    emit("PYTORCH_MATRIX_BACKEND", "torch.mm fp16 CUDA")

    try:
        torch.backends.cuda.matmul.allow_tf32 = True
    except Exception:
        pass
    try:
        torch.set_float32_matmul_precision("high")
    except Exception:
        pass

    dtype = torch.float16
    bytes_per_value = 2
    memory_fraction = {"safe": 0.58, "balanced": 0.72, "high": 0.84}[args.intensity]
    min_size = min(args.size, 2048)

    def align_down(value, step=256):
        return max(step, (int(value) // step) * step)

    candidates = []
    target = align_down(args.size)
    while target >= min_size:
        required = target * target * bytes_per_value * 3
        if required <= int(props.total_memory * memory_fraction):
            candidates.append(target)
        next_target = align_down(target * 3 // 4)
        if next_target >= target:
            next_target = target - 256
        target = next_target
    if not candidates:
        candidates.append(align_down(min_size))

    a = b = c = None
    last_error = None
    effective_size = None
    for candidate in candidates:
        try:
            torch.cuda.empty_cache()
            a = torch.randn((candidate, candidate), device=device, dtype=dtype)
            b = torch.randn((candidate, candidate), device=device, dtype=dtype)
            c = torch.empty((candidate, candidate), device=device, dtype=dtype)
            torch.mm(a, b, out=c)
            torch.cuda.synchronize()
            effective_size = candidate
            break
        except Exception as exc:
            last_error = exc
            message = str(exc).lower()
            if "out of memory" in message or "cuda error" in message:
                a = b = c = None
                try:
                    torch.cuda.empty_cache()
                except Exception:
                    pass
                continue
            raise

    if a is None or b is None or c is None or effective_size is None:
        emit("ERROR", "could not allocate CUDA stress matrices: " + str(last_error))
        sys.exit(0)

    emit("RESULT_EFFECTIVE_SIZE", effective_size)
    emit("RESULT_DTYPE", "float16")
    if effective_size != args.size:
        emit("NOTE", "reduced CUDA stress matrix to {}x{} to stay within memory limits".format(effective_size, effective_size))

    warmups = {"safe": 1, "balanced": 2, "high": 3}[args.intensity]
    for _ in range(warmups):
        torch.mm(a, b, out=c)
    torch.cuda.synchronize()
    try:
        torch.cuda.reset_peak_memory_stats(device)
    except Exception:
        pass
    if effective_size < 4096:
        inner_batch = {"safe": 8, "balanced": 16, "high": 32}[args.intensity]
    elif effective_size < 8192:
        inner_batch = {"safe": 4, "balanced": 8, "high": 16}[args.intensity]
    else:
        inner_batch = 1
    emit("RESULT_INNER_BATCH", inner_batch)

    iterations = 0
    total_wall_ms = 0.0
    total_gpu_ms = 0.0
    end_at = time.perf_counter() + max(args.time_limit, 0.1)

    while True:
        if iterations > 0 and time.perf_counter() >= end_at:
            break
        start_event = torch.cuda.Event(enable_timing=True)
        end_event = torch.cuda.Event(enable_timing=True)
        wall_start = time.perf_counter()
        start_event.record()
        for _ in range(inner_batch):
            torch.mm(a, b, out=c)
        end_event.record()
        torch.cuda.synchronize()
        wall_ms = (time.perf_counter() - wall_start) * 1000.0
        gpu_ms = float(start_event.elapsed_time(end_event))
        iterations += inner_batch
        total_wall_ms += wall_ms
        total_gpu_ms += gpu_ms
        emit(
            "PROGRESS",
            iterations,
            "{:.6f}".format(wall_ms / max(inner_batch, 1)),
            "{:.6f}".format(total_wall_ms),
            "{:.6f}".format(total_gpu_ms),
            iterations,
        )

    try:
        peak_allocated = int(torch.cuda.max_memory_allocated(device))
        peak_reserved = int(torch.cuda.max_memory_reserved(device))
        emit("RESULT_PEAK_ALLOCATED_BYTES", peak_allocated)
        emit("RESULT_PEAK_RESERVED_BYTES", peak_reserved)
    except Exception:
        pass
    emit(
        "DONE",
        iterations,
        "{:.6f}".format(total_wall_ms / max(iterations, 1)),
        "{:.6f}".format(total_wall_ms),
        "{:.6f}".format(total_gpu_ms),
        iterations,
    )
except Exception as exc:
    emit("ERROR", type(exc).__name__ + ": " + str(exc))
    emit("NOTE", traceback.format_exc(limit=6))
"#;

const PYTORCH_CUDA_TINY_MATRIX_STRESS_SCRIPT: &str = r#"
import argparse
import math
import sys
import time
import traceback

def clean(value):
    return str(value).replace("\t", " ").replace("\n", " ")

def emit(key, *values):
    if values:
        print(key + "\t" + "\t".join(clean(value) for value in values), flush=True)
    else:
        print(key, flush=True)

parser = argparse.ArgumentParser()
parser.add_argument("--device", type=int, default=0)
parser.add_argument("--size", type=int, required=True)
parser.add_argument("--time-limit", type=float, required=True)
parser.add_argument("--intensity", choices=("safe", "balanced", "high"), default="balanced")
args = parser.parse_args()

try:
    import torch
except Exception as exc:
    emit("PYTORCH_MATRIX_UNAVAILABLE", type(exc).__name__ + ": " + str(exc))
    sys.exit(0)

try:
    emit("TORCH", getattr(torch, "__version__", ""))
    emit("CUDA", getattr(torch.version, "cuda", "") or "")
    available = bool(torch.cuda.is_available())
    emit("CUDA_AVAILABLE", available)
    count = int(torch.cuda.device_count()) if available else 0
    emit("DEVICE_COUNT", count)
    if not available:
        emit("PYTORCH_MATRIX_UNAVAILABLE", "torch.cuda.is_available() is false")
        sys.exit(0)
    if args.device < 0 or args.device >= count:
        emit("PYTORCH_MATRIX_UNAVAILABLE", "CUDA device {} is not available".format(args.device))
        sys.exit(0)
    if args.size != 4:
        emit("PYTORCH_MATRIX_UNAVAILABLE", "tensor-core equivalent stress currently targets 4x4")
        sys.exit(0)

    torch.cuda.set_device(args.device)
    device = torch.device("cuda:{}".format(args.device))
    props = torch.cuda.get_device_properties(args.device)
    emit("RESULT_GPU_NAME", props.name)
    emit("RESULT_DEVICE_INDEX", args.device)
    emit("PYTORCH_MATRIX_BACKEND", "torch.mm fp16 tensor-core equivalent 4x4")

    try:
        torch.backends.cuda.matmul.allow_tf32 = True
    except Exception:
        pass
    try:
        torch.set_float32_matmul_precision("high")
    except Exception:
        pass

    dtype = torch.float16
    bytes_per_value = 2
    memory_fraction = {"safe": 0.12, "balanced": 0.22, "high": 0.36}[args.intensity]
    desired_size = {"safe": 8192, "balanced": 16384, "high": 32768}[args.intensity]
    min_size = 4096

    def align_down(value, step=1024):
        return max(step, (int(value) // step) * step)

    memory_limited = align_down(math.sqrt(max(1, int(props.total_memory * memory_fraction)) / (3 * bytes_per_value)))
    target = align_down(min(desired_size, memory_limited))
    candidates = []
    while target >= min_size:
        candidates.append(target)
        next_target = align_down(target * 3 // 4)
        if next_target >= target:
            next_target = target - 1024
        target = next_target
    if not candidates:
        candidates.append(min_size)

    a = b = c = None
    last_error = None
    effective_size = None
    for candidate in candidates:
        try:
            torch.cuda.empty_cache()
            a = torch.randn((candidate, candidate), device=device, dtype=dtype)
            b = torch.randn((candidate, candidate), device=device, dtype=dtype)
            c = torch.empty((candidate, candidate), device=device, dtype=dtype)
            torch.mm(a, b, out=c)
            torch.cuda.synchronize()
            effective_size = candidate
            break
        except Exception as exc:
            last_error = exc
            message = str(exc).lower()
            if "out of memory" in message or "cuda error" in message:
                a = b = c = None
                try:
                    torch.cuda.empty_cache()
                except Exception:
                    pass
                continue
            raise

    if a is None or b is None or c is None or effective_size is None:
        emit("ERROR", "could not allocate CUDA tensor-core stress matrices: " + str(last_error))
        sys.exit(0)

    equivalent_per_mm = max(1, (effective_size ** 3) // (args.size ** 3))
    emit("RESULT_EFFECTIVE_SIZE", effective_size)
    emit("RESULT_DTYPE", "float16")
    emit("RESULT_EQUIVALENT_4X4_PER_MM", equivalent_per_mm)
    if effective_size != desired_size:
        emit("NOTE", "using {}x{} tensor-core GEMM for equivalent 4x4 accounting".format(effective_size, effective_size))

    warmups = {"safe": 1, "balanced": 2, "high": 3}[args.intensity]
    for _ in range(warmups):
        torch.mm(a, b, out=c)
    torch.cuda.synchronize()
    try:
        torch.cuda.reset_peak_memory_stats(device)
    except Exception:
        pass

    inner_batch = 1
    if effective_size <= 8192:
        inner_batch = {"safe": 2, "balanced": 4, "high": 8}[args.intensity]
    elif effective_size <= 16384:
        inner_batch = {"safe": 1, "balanced": 2, "high": 4}[args.intensity]
    emit("RESULT_INNER_BATCH", inner_batch)

    iterations = 0
    total_wall_ms = 0.0
    total_gpu_ms = 0.0
    end_at = time.perf_counter() + max(args.time_limit, 0.1)

    while True:
        if iterations > 0 and time.perf_counter() >= end_at:
            break
        start_event = torch.cuda.Event(enable_timing=True)
        end_event = torch.cuda.Event(enable_timing=True)
        wall_start = time.perf_counter()
        start_event.record()
        for _ in range(inner_batch):
            torch.mm(a, b, out=c)
        end_event.record()
        torch.cuda.synchronize()
        wall_ms = (time.perf_counter() - wall_start) * 1000.0
        gpu_ms = float(start_event.elapsed_time(end_event))
        completed = equivalent_per_mm * inner_batch
        iterations += completed
        total_wall_ms += wall_ms
        total_gpu_ms += gpu_ms
        emit(
            "PROGRESS",
            iterations,
            "{:.12f}".format(wall_ms / max(completed, 1)),
            "{:.6f}".format(total_wall_ms),
            "{:.6f}".format(total_gpu_ms),
            iterations,
        )

    try:
        peak_allocated = int(torch.cuda.max_memory_allocated(device))
        peak_reserved = int(torch.cuda.max_memory_reserved(device))
        emit("RESULT_PEAK_ALLOCATED_BYTES", peak_allocated)
        emit("RESULT_PEAK_RESERVED_BYTES", peak_reserved)
    except Exception:
        pass
    emit(
        "DONE",
        iterations,
        "{:.12f}".format(total_wall_ms / max(iterations, 1)),
        "{:.6f}".format(total_wall_ms),
        "{:.6f}".format(total_gpu_ms),
        iterations,
    )
except Exception as exc:
    emit("ERROR", type(exc).__name__ + ": " + str(exc))
    emit("NOTE", traceback.format_exc(limit=6))
"#;

fn repeat_pytorch_cuda_matrix_stress<F>(
    size: usize,
    gpu_intensity: GpuIntensity,
    cancel: &AtomicBool,
    deadline: &Option<Instant>,
    emit: &mut F,
) -> Result<Option<RepeatProgress>>
where
    F: FnMut(u64, f64, f64, f64, u64, bool, bool) -> RepeatProgress,
{
    repeat_pytorch_cuda_matrix_stress_script(
        PYTORCH_CUDA_MATRIX_STRESS_SCRIPT,
        size,
        gpu_intensity,
        cancel,
        deadline,
        emit,
    )
}

fn repeat_pytorch_cuda_tiny_matrix_stress<F>(
    size: usize,
    gpu_intensity: GpuIntensity,
    cancel: &AtomicBool,
    deadline: &Option<Instant>,
    emit: &mut F,
) -> Result<Option<RepeatProgress>>
where
    F: FnMut(u64, f64, f64, f64, u64, bool, bool) -> RepeatProgress,
{
    repeat_pytorch_cuda_matrix_stress_script(
        PYTORCH_CUDA_TINY_MATRIX_STRESS_SCRIPT,
        size,
        gpu_intensity,
        cancel,
        deadline,
        emit,
    )
}

fn repeat_pytorch_cuda_matrix_stress_script<F>(
    script: &str,
    size: usize,
    gpu_intensity: GpuIntensity,
    cancel: &AtomicBool,
    deadline: &Option<Instant>,
    emit: &mut F,
) -> Result<Option<RepeatProgress>>
where
    F: FnMut(u64, f64, f64, f64, u64, bool, bool) -> RepeatProgress,
{
    let python = default_pytorch_python_executable();
    let python = python.trim();
    if python.is_empty() {
        return Ok(None);
    }

    let time_limit_s = deadline
        .map(|deadline| deadline.saturating_duration_since(Instant::now()).as_secs_f64())
        .unwrap_or(86_400.0)
        .max(0.1);
    let mut command = Command::new(python);
    command
        .arg("-c")
        .arg(script)
        .arg("--device")
        .arg("0")
        .arg("--size")
        .arg(size.to_string())
        .arg("--time-limit")
        .arg(format!("{time_limit_s:.3}"))
        .arg("--intensity")
        .arg(pytorch_matrix_stress_intensity_arg(gpu_intensity))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW_RAW);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return Ok(None),
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Ok(None);
    };
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Ok(None);
    };

    let (line_tx, line_rx) = mpsc::channel::<String>();
    let stdout_thread = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else {
                break;
            };
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });
    let stderr_thread = thread::spawn(move || {
        let mut text = String::new();
        let mut reader = BufReader::new(stderr);
        let _ = reader.read_to_string(&mut text);
        text
    });

    let mut state = PyTorchMatrixStressState::default();
    let startup_timeout_at = Instant::now() + Duration::from_secs(60);
    let deadline_timeout_at = deadline.map(|deadline| deadline + Duration::from_secs(180));
    let mut canceled = false;
    let mut timed_out = false;
    let status = loop {
        drain_pytorch_matrix_stress_lines(&line_rx, &mut state, emit);
        if cancel.load(Ordering::Relaxed) {
            canceled = true;
            let _ = child.kill();
            break child
                .wait()
                .with_context(|| format!("failed to wait for PyTorch CUDA worker {python}"))?;
        }
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("failed to query PyTorch CUDA worker {python}"))?
        {
            break status;
        }

        let now = Instant::now();
        if state.cuda_available.is_none() && state.iterations == 0 && now >= startup_timeout_at {
            timed_out = true;
            let _ = child.kill();
            break child
                .wait()
                .with_context(|| format!("failed to wait for PyTorch CUDA worker {python}"))?;
        }
        if let Some(timeout_at) = deadline_timeout_at
            && now >= timeout_at
        {
            timed_out = true;
            let _ = child.kill();
            break child
                .wait()
                .with_context(|| format!("failed to wait for PyTorch CUDA worker {python}"))?;
        }

        thread::sleep(Duration::from_millis(25));
    };
    let _ = stdout_thread.join();
    drain_pytorch_matrix_stress_lines(&line_rx, &mut state, emit);
    let stderr = stderr_thread.join().unwrap_or_default();

    if canceled {
        return Ok(Some(emit(
            state.iterations,
            state.latest_ms,
            state.total_ms,
            state.total_compute_ms,
            state.compute_count,
            true,
            true,
        )));
    }
    if timed_out && state.iterations == 0 {
        return Ok(None);
    }
    if !status.success() {
        if state.iterations == 0 {
            return Ok(None);
        }
        return Err(anyhow!(
            "PyTorch CUDA matrix stress worker failed{}",
            if stderr.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", stderr.trim())
            }
        ));
    }
    if let Some(error) = state.error.as_deref() {
        if state.iterations == 0 {
            return Ok(None);
        }
        return Err(anyhow!("PyTorch CUDA matrix stress failed: {error}"));
    }
    if state.cuda_available == Some(false) || state.unavailable_reason.is_some() {
        return Ok(None);
    }
    if state.iterations == 0 && state.cuda_available != Some(true) {
        return Ok(None);
    }

    Ok(Some(emit(
        state.iterations,
        state.latest_ms,
        state.total_ms,
        state.total_compute_ms,
        state.compute_count,
        false,
        true,
    )))
}

fn drain_pytorch_matrix_stress_lines<F>(
    line_rx: &Receiver<String>,
    state: &mut PyTorchMatrixStressState,
    emit: &mut F,
) where
    F: FnMut(u64, f64, f64, f64, u64, bool, bool) -> RepeatProgress,
{
    while let Ok(line) = line_rx.try_recv() {
        record_pytorch_matrix_stress_line(&line, state, emit);
    }
}

fn record_pytorch_matrix_stress_line<F>(
    line: &str,
    state: &mut PyTorchMatrixStressState,
    emit: &mut F,
) where
    F: FnMut(u64, f64, f64, f64, u64, bool, bool) -> RepeatProgress,
{
    let mut parts = line.split('\t');
    let key = parts.next().unwrap_or_default();
    let values = parts.collect::<Vec<_>>();
    match key {
        "CUDA_AVAILABLE" => {
            state.cuda_available = values.first().copied().map(parse_probe_bool);
        }
        "PYTORCH_MATRIX_UNAVAILABLE" => {
            let reason = values.join(" ");
            if !reason.trim().is_empty() {
                state.unavailable_reason = Some(reason);
            }
        }
        "ERROR" => {
            let error = values.join(" ");
            if !error.trim().is_empty() {
                state.error = Some(error);
            }
        }
        "RESULT_GPU_NAME" => {
            let name = values.join(" ");
            if !name.trim().is_empty() {
                state.gpu_name = Some(name);
            }
        }
        "RESULT_EFFECTIVE_SIZE" => {
            state.effective_size = values.first().and_then(|value| value.parse().ok());
        }
        "PROGRESS" => {
            if let Some(sample) = parse_pytorch_matrix_stress_progress_line(line) {
                state.iterations = sample.iterations;
                state.latest_ms = sample.latest_ms;
                state.total_ms = sample.total_ms;
                state.total_compute_ms = sample.total_compute_ms;
                state.compute_count = sample.compute_count;
                emit(
                    state.iterations,
                    state.latest_ms,
                    state.total_ms,
                    state.total_compute_ms,
                    state.compute_count,
                    false,
                    false,
                );
            }
        }
        "DONE" => {
            if let Some(sample) = parse_pytorch_matrix_stress_progress_line(line) {
                state.iterations = sample.iterations;
                state.latest_ms = sample.latest_ms;
                state.total_ms = sample.total_ms;
                state.total_compute_ms = sample.total_compute_ms;
                state.compute_count = sample.compute_count;
            }
        }
        _ => {}
    }
}

fn parse_pytorch_matrix_stress_progress_line(
    line: &str,
) -> Option<PyTorchMatrixStressProgressSample> {
    let mut parts = line.split('\t');
    let key = parts.next()?;
    if key != "PROGRESS" && key != "DONE" {
        return None;
    }
    Some(PyTorchMatrixStressProgressSample {
        iterations: parts.next()?.parse().ok()?,
        latest_ms: parts.next()?.parse().ok()?,
        total_ms: parts.next()?.parse().ok()?,
        total_compute_ms: parts.next()?.parse().ok()?,
        compute_count: parts.next()?.parse().ok()?,
    })
}

fn pytorch_matrix_stress_intensity_arg(gpu_intensity: GpuIntensity) -> &'static str {
    match gpu_intensity {
        GpuIntensity::Safe => "safe",
        GpuIntensity::Balanced => "balanced",
        GpuIntensity::High => "high",
    }
}

fn ensure_matrix_host_memory_available(
    size: usize,
    matrix_count: u64,
    workload: &str,
) -> Result<()> {
    let required = matrix_buffers_bytes(size, matrix_count)
        .ok_or_else(|| anyhow!("matrix memory estimate overflow"))?;
    let Ok(info) = detect_ram_memory_info() else {
        return Ok(());
    };
    let safe_available = info
        .available_physical_bytes
        .saturating_sub(RAM_OS_HEADROOM_BYTES);
    if safe_available > 0 && required > safe_available {
        return Err(anyhow!(
            "{workload} needs about {} of system memory for {size}x{size}; safely available memory is {}",
            format_bytes(required),
            format_bytes(safe_available)
        ));
    }
    Ok(())
}

fn repeat_should_continue(deadline: &Option<Instant>, cancel: &AtomicBool) -> bool {
    if cancel.load(Ordering::Relaxed) {
        return false;
    }
    match deadline {
        Some(deadline) => Instant::now() < *deadline,
        None => true,
    }
}
