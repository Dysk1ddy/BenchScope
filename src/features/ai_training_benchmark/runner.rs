#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct AiGemmParams {
    rows: u32,
    cols: u32,
    inner: u32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct AiSgdParams {
    element_count: u32,
    input_dim: u32,
    output_dim: u32,
    start_index: u32,
    learning_rate: f32,
    _pad1: [f32; 3],
}

const AI_LINEAR_VALIDATION_MAX_REFERENCE_OPS: usize = 20_000_000;
const AI_LINEAR_TIMESTAMP_PAIRS_PER_STEP: usize = 2;
const AI_OPTIMIZER_TIMESTAMP_PAIRS_PER_STEP: usize = 1;
const AI_MEMORY_HEADROOM_NUMERATOR: u64 = 9;
const AI_MEMORY_HEADROOM_DENOMINATOR: u64 = 10;
const AI_SGD_LEARNING_RATE: f32 = 0.000001;

struct AiGpuRunner {
    device: wgpu::Device,
    queue: wgpu::Queue,
    gemm_pipeline: wgpu::ComputePipeline,
    sgd_pipeline: wgpu::ComputePipeline,
    gemm_bind_group_layout: wgpu::BindGroupLayout,
    sgd_bind_group_layout: wgpu::BindGroupLayout,
    timestamp_query: bool,
    max_storage_buffer_binding_size: u64,
    max_buffer_size: u64,
    max_compute_workgroups_per_dimension: u32,
}

struct AiTrainingBuffers {
    x: wgpu::Buffer,
    x_t: wgpu::Buffer,
    weights: wgpu::Buffer,
    weights_t: wgpu::Buffer,
    dy: wgpu::Buffer,
    y: wgpu::Buffer,
    dw: wgpu::Buffer,
    dx: wgpu::Buffer,
    forward_params: wgpu::Buffer,
    weight_grad_params: wgpu::Buffer,
    input_grad_params: wgpu::Buffer,
    validation_inputs: Option<AiLinearValidationInputs>,
}

struct AiLinearValidationInputs {
    x: Vec<f32>,
    initial_weights: Vec<f32>,
    dy: Vec<f32>,
}

#[derive(Clone, Copy)]
struct AiLinearBlockSpec {
    batch: usize,
    input: usize,
    output: usize,
}

struct AiLinearTrainingBlock {
    batch: usize,
    input: usize,
    output: usize,
    _buffers: AiTrainingBuffers,
    forward_bind_group: wgpu::BindGroup,
    weight_grad_bind_group: wgpu::BindGroup,
    input_grad_bind_group: wgpu::BindGroup,
    sgd_chunks: Vec<AiOptimizerChunk>,
}

struct AiOptimizerChunk {
    element_count: usize,
    _params: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl AiGpuRunner {
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

        let requested_limits = wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());
        let mut descriptor = wgpu::DeviceDescriptor::default();
        descriptor.label = Some("BenchScope AI training device");
        descriptor.required_features = required_features;
        descriptor.required_limits = requested_limits.clone();

        let (device, queue) = pollster::block_on(adapter.request_device(&descriptor))
            .context("requesting wgpu device for AI training benchmark")?;

        let gemm_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("AI training GEMM shader"),
            source: wgpu::ShaderSource::Wgsl(AI_GEMM_SHADER.into()),
        });
        let sgd_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("AI training SGD update shader"),
            source: wgpu::ShaderSource::Wgsl(AI_SGD_UPDATE_SHADER.into()),
        });

        let gemm_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("AI training GEMM bind group layout"),
                entries: &[
                    storage_entry(0, true),
                    storage_entry(1, true),
                    storage_entry(2, false),
                    uniform_entry(3),
                ],
            });
        let sgd_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("AI training SGD bind group layout"),
                entries: &[
                    storage_entry(0, true),
                    storage_entry(1, false),
                    storage_entry(2, false),
                    uniform_entry(3),
                ],
            });

        let gemm_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("AI training GEMM pipeline layout"),
                bind_group_layouts: &[Some(&gemm_bind_group_layout)],
                immediate_size: 0,
            });
        let sgd_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("AI training SGD pipeline layout"),
            bind_group_layouts: &[Some(&sgd_bind_group_layout)],
            immediate_size: 0,
        });

        let gemm_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("AI training GEMM pipeline"),
            layout: Some(&gemm_pipeline_layout),
            module: &gemm_shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let sgd_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("AI training SGD update pipeline"),
            layout: Some(&sgd_pipeline_layout),
            module: &sgd_shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        Ok(Self {
            device,
            queue,
            gemm_pipeline,
            sgd_pipeline,
            gemm_bind_group_layout,
            sgd_bind_group_layout,
            timestamp_query,
            max_storage_buffer_binding_size: requested_limits.max_storage_buffer_binding_size,
            max_buffer_size: requested_limits.max_buffer_size,
            max_compute_workgroups_per_dimension: requested_limits.max_compute_workgroups_per_dimension,
        })
    }

    fn run_linear_training(
        &self,
        mut config: AiTrainingConfig,
        cancel: &AtomicBool,
        tx: &Sender<AiTrainingWorkerEvent>,
    ) -> Result<AiTrainingResult> {
        let mut run_notes = Vec::new();
        let adapter_memory_limit =
            adapter_memory_limit_bytes(&config.adapter).map(|(bytes, _)| bytes);
        if let Some(note) = auto_size_ai_training_config_for_limits(
            &mut config,
            adapter_memory_limit,
            Some(self.max_storage_buffer_binding_size.min(self.max_buffer_size)),
        ) {
            let _ = tx.send(AiTrainingWorkerEvent::Log(note.clone()));
            run_notes.push(note);
        }
        let dims = &config.dimensions;
        let batch = dims.batch_size;
        let input = dims.input_dim;
        let output = dims.output_dim;
        validate_ai_linear_dimensions(batch, input, output)?;
        self.ensure_linear_buffers_fit(batch, input, output)?;

        let total_steps = config.warmup_steps.saturating_add(config.measured_steps);
        let started = Instant::now();
        emit_ai_training_progress(
            tx,
            "Preparing tensors",
            0,
            total_steps,
            started,
            Some(config.time_limit_s),
            true,
        );

        let buffers = self.create_linear_buffers(batch, input, output, cancel)?;
        let forward_bind_group = self.create_gemm_bind_group(
            &buffers.x,
            &buffers.weights,
            &buffers.y,
            &buffers.forward_params,
            "AI forward GEMM bind group",
        );
        let weight_grad_bind_group = self.create_gemm_bind_group(
            &buffers.x_t,
            &buffers.dy,
            &buffers.dw,
            &buffers.weight_grad_params,
            "AI weight-gradient GEMM bind group",
        );
        let input_grad_bind_group = self.create_gemm_bind_group(
            &buffers.dy,
            &buffers.weights_t,
            &buffers.dx,
            &buffers.input_grad_params,
            "AI input-gradient GEMM bind group",
        );
        let parameter_count = input
            .checked_mul(output)
            .ok_or_else(|| anyhow!("parameter count overflow"))?;
        let sgd_chunks = self.create_sgd_chunks(
            parameter_count,
            input,
            output,
            self.optimizer_chunk_elements(),
            &buffers.dw,
            &buffers.weights,
            &buffers.weights_t,
        )?;
        if sgd_chunks.len() > 1 {
            run_notes.push(format!(
                "SGD update is chunked into {} dispatches for adapter workgroup limits.",
                sgd_chunks.len()
            ));
        }
        let timestamp_plan = if self.timestamp_query {
            timestamp_query_plan(
                config
                    .measured_steps
                    .saturating_mul(AI_LINEAR_TIMESTAMP_PAIRS_PER_STEP),
            )
        } else {
            None
        };
        let query_set = timestamp_plan.map(|(query_count, _)| {
            self.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("AI training compute timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: query_count,
            })
        });
        let timestamp_resolve = timestamp_plan.map(|(_, timestamp_buffer_size)| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("AI training timestamp resolve"),
                size: timestamp_buffer_size,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        });
        let timestamp_readback = timestamp_plan.map(|(_, timestamp_buffer_size)| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("AI training timestamp readback"),
                size: timestamp_buffer_size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });

        let warmup_steps = config.warmup_steps;
        let measured_steps = config.measured_steps;
        let mut step_latencies_ms = Vec::with_capacity(measured_steps);
        let mut measured_started = None;
        let mut measured_elapsed_s = 0.0;
        let mut capped = false;
        let mut last_emit = Instant::now() - Duration::from_secs(1);
        let mut completed_step_count = 0usize;

        for step_index in 0..total_steps {
            check_canceled_with(Some(cancel), "AI training benchmark canceled")?;
            let measured_index = step_index.saturating_sub(warmup_steps);
            if step_index == warmup_steps {
                measured_started = Some(Instant::now());
            }
            if let Some(measured_started) = measured_started {
                if measured_index > 0 && measured_started.elapsed().as_secs_f64() >= config.time_limit_s {
                    capped = true;
                    break;
                }
            }

            let phase = if step_index < warmup_steps {
                "Warmup training step"
            } else {
                "Measured training step"
            };
            let step_start = Instant::now();
            self.submit_linear_step(
                &forward_bind_group,
                &weight_grad_bind_group,
                &input_grad_bind_group,
                &sgd_chunks,
                batch,
                input,
                output,
                cancel,
                query_set.as_ref(),
                (step_index >= warmup_steps)
                    .then_some(measured_index.saturating_mul(AI_LINEAR_TIMESTAMP_PAIRS_PER_STEP)),
            )?;
            completed_step_count = step_index + 1;
            let step_ms = step_start.elapsed().as_secs_f64() * 1000.0;
            if step_index >= warmup_steps {
                step_latencies_ms.push(step_ms);
                measured_elapsed_s = measured_started
                    .map(|instant| instant.elapsed().as_secs_f64())
                    .unwrap_or_default();
            }

            let now = Instant::now();
            let force = step_index == 0 || step_index + 1 == total_steps;
            if force || now.duration_since(last_emit) >= Duration::from_millis(PROGRESS_SAMPLE_MS) {
                last_emit = now;
                emit_ai_training_progress(
                    tx,
                    phase,
                    step_index + 1,
                    total_steps,
                    started,
                    Some(config.time_limit_s),
                    force,
                );
            }

            pause_between_gpu_submissions(config.gpu_intensity, Some(cancel))?;
        }

        if step_latencies_ms.is_empty() {
            return Err(anyhow!("No measured AI training steps completed"));
        }
        let measured_steps = step_latencies_ms.len();
        if measured_elapsed_s <= 0.0 {
            measured_elapsed_s = step_latencies_ms.iter().sum::<f64>() / 1000.0;
        }

        let flops_per_step = config_flops_per_step(&config);
        let total_flops = flops_per_step * measured_steps as f64;
        let compute_ms = if let (Some(query_set), Some(resolve), Some(readback)) =
            (&query_set, &timestamp_resolve, &timestamp_readback)
        {
            self.resolve_compute_timestamps(
                query_set,
                resolve,
                readback,
                measured_steps.saturating_mul(AI_LINEAR_TIMESTAMP_PAIRS_PER_STEP),
                cancel,
            )
            .ok()
        } else {
            None
        };
        let compute_tflops =
            compute_ms.map(|compute_ms| total_flops / (compute_ms / 1000.0) / 1.0e12);
        let end_to_end_tflops = total_flops / measured_elapsed_s / 1.0e12;
        let throughput_value = ai_training_throughput(&config, measured_steps, measured_elapsed_s);
        let avg_step_ms = step_latencies_ms.iter().sum::<f64>() / measured_steps as f64;
        let p95_step_ms = percentile_sorted_copy(&step_latencies_ms, 0.95);
        let validation = validate_linear_training_result(
            self,
            &config,
            &buffers,
            completed_step_count,
            cancel,
        );
        if capped {
            run_notes.push(format!(
                "Stopped after {} measured step(s) at the {} time limit.",
                measured_steps,
                format_elapsed(config.time_limit_s)
            ));
        }
        run_notes.push(if compute_ms.is_some() {
            "GPU compute timing uses timestamp queries.".to_owned()
        } else if self.timestamp_query {
            "Compute-only timestamps unavailable for this run; using CPU-observed step latency.".to_owned()
        } else {
            "Adapter does not expose timestamp queries; using CPU-observed step latency.".to_owned()
        });
        if config.smoke_test {
            run_notes.push("Smoke test run.".to_owned());
        }
        let notes = run_notes.join(" ");

        let _ = tx.send(AiTrainingWorkerEvent::Log(format!(
            "Completed {} measured step(s): {:.2} end-to-end TFLOP/s, {:.1} {}, avg step {} ms",
            measured_steps,
            end_to_end_tflops,
            throughput_value,
            config.workload.throughput_label(),
            format_ms(Some(avg_step_ms))
        )));

        Ok(AiTrainingResult {
            backend: config.backend,
            workload: config.workload,
            preset: config.preset,
            precision: config.precision,
            gpu_names: vec![config.adapter.label()],
            shape: linear_shape_label(&config.dimensions),
            flops_per_step,
            measured_steps,
            compute_tflops,
            end_to_end_tflops: Some(end_to_end_tflops),
            throughput_value: Some(throughput_value),
            throughput_label: config.workload.throughput_label(),
            avg_step_ms: Some(avg_step_ms),
            p95_step_ms: Some(p95_step_ms),
            memory_bytes: config_memory_bytes(&config),
            validation,
            notes,
        })
    }

    fn run_mlp_training(
        &self,
        mut config: AiTrainingConfig,
        cancel: &AtomicBool,
        tx: &Sender<AiTrainingWorkerEvent>,
    ) -> Result<AiTrainingResult> {
        let mut run_notes = Vec::new();
        let adapter_memory_limit =
            adapter_memory_limit_bytes(&config.adapter).map(|(bytes, _)| bytes);
        if let Some(note) = auto_size_ai_training_config_for_limits(
            &mut config,
            adapter_memory_limit,
            Some(self.max_storage_buffer_binding_size.min(self.max_buffer_size)),
        ) {
            let _ = tx.send(AiTrainingWorkerEvent::Log(note.clone()));
            run_notes.push(note);
        }

        let dims = &config.dimensions;
        let hidden = dims.hidden_size;
        let expansion = dims.output_dim;
        let specs = [
            AiLinearBlockSpec {
                batch: dims.batch_size,
                input: hidden,
                output: expansion,
            },
            AiLinearBlockSpec {
                batch: dims.batch_size,
                input: expansion,
                output: hidden,
            },
        ];
        run_notes.push(
            "MLP benchmark runs two dense training-shaped GPU blocks with SGD-style updates."
                .to_owned(),
        );

        self.run_linear_block_sequence_training(
            config,
            &specs,
            cancel,
            tx,
            "Preparing MLP tensors",
            "Warmup MLP training step",
            "Measured MLP training step",
            "Skipped: MLP proxy validation is not implemented".to_owned(),
            run_notes,
        )
    }

    fn run_transformer_proxy_training(
        &self,
        mut config: AiTrainingConfig,
        cancel: &AtomicBool,
        tx: &Sender<AiTrainingWorkerEvent>,
    ) -> Result<AiTrainingResult> {
        let mut run_notes = Vec::new();
        let adapter_memory_limit =
            adapter_memory_limit_bytes(&config.adapter).map(|(bytes, _)| bytes);
        if let Some(note) = auto_size_ai_training_config_for_limits(
            &mut config,
            adapter_memory_limit,
            Some(self.max_storage_buffer_binding_size.min(self.max_buffer_size)),
        ) {
            let _ = tx.send(AiTrainingWorkerEvent::Log(note.clone()));
            run_notes.push(note);
        }

        let specs = transformer_linear_block_specs(&config.dimensions)?;
        run_notes.push(format!(
            "Transformer proxy runs {} projection, attention, and MLP GEMM/update block(s); softmax and normalization are included in FLOP accounting only.",
            specs.len()
        ));

        self.run_linear_block_sequence_training(
            config,
            &specs,
            cancel,
            tx,
            "Preparing transformer proxy tensors",
            "Warmup transformer proxy step",
            "Measured transformer proxy step",
            "Skipped: transformer proxy validation is not implemented".to_owned(),
            run_notes,
        )
    }

    fn run_linear_block_sequence_training(
        &self,
        config: AiTrainingConfig,
        block_specs: &[AiLinearBlockSpec],
        cancel: &AtomicBool,
        tx: &Sender<AiTrainingWorkerEvent>,
        prepare_phase: &'static str,
        warmup_phase: &'static str,
        measured_phase: &'static str,
        validation: String,
        mut run_notes: Vec<String>,
    ) -> Result<AiTrainingResult> {
        if block_specs.is_empty() {
            return Err(anyhow!("AI training workload has no GPU blocks to run"));
        }
        let total_steps = config.warmup_steps.saturating_add(config.measured_steps);
        let timestamp_pairs_per_step = block_specs
            .len()
            .saturating_mul(AI_LINEAR_TIMESTAMP_PAIRS_PER_STEP);
        let started = Instant::now();
        emit_ai_training_progress(
            tx,
            prepare_phase,
            0,
            total_steps,
            started,
            Some(config.time_limit_s),
            true,
        );

        let mut blocks = Vec::with_capacity(block_specs.len());
        for spec in block_specs {
            blocks.push(self.create_linear_training_block(*spec, cancel)?);
        }
        let chunked_blocks = blocks
            .iter()
            .filter(|block| block.sgd_chunks.len() > 1)
            .count();
        if chunked_blocks > 0 {
            run_notes.push(format!(
                "SGD update is chunked for {} block(s) to stay within adapter workgroup limits.",
                chunked_blocks
            ));
        }

        let timestamp_plan = if self.timestamp_query {
            timestamp_query_plan(config.measured_steps.saturating_mul(timestamp_pairs_per_step))
        } else {
            None
        };
        let query_set = timestamp_plan.map(|(query_count, _)| {
            self.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("AI training proxy compute timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: query_count,
            })
        });
        let timestamp_resolve = timestamp_plan.map(|(_, timestamp_buffer_size)| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("AI training proxy timestamp resolve"),
                size: timestamp_buffer_size,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        });
        let timestamp_readback = timestamp_plan.map(|(_, timestamp_buffer_size)| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("AI training proxy timestamp readback"),
                size: timestamp_buffer_size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });

        let warmup_steps = config.warmup_steps;
        let mut step_latencies_ms = Vec::with_capacity(config.measured_steps);
        let mut measured_started = None;
        let mut measured_elapsed_s = 0.0;
        let mut capped = false;
        let mut last_emit = Instant::now() - Duration::from_secs(1);

        for step_index in 0..total_steps {
            check_canceled_with(Some(cancel), "AI training benchmark canceled")?;
            let measured_index = step_index.saturating_sub(warmup_steps);
            if step_index == warmup_steps {
                measured_started = Some(Instant::now());
            }
            if let Some(measured_started) = measured_started {
                if measured_index > 0
                    && measured_started.elapsed().as_secs_f64() >= config.time_limit_s
                {
                    capped = true;
                    break;
                }
            }

            let phase = if step_index < warmup_steps {
                warmup_phase
            } else {
                measured_phase
            };
            let step_start = Instant::now();
            for (block_index, block) in blocks.iter().enumerate() {
                let timestamp_pair_base = (step_index >= warmup_steps).then_some(
                    measured_index
                        .saturating_mul(timestamp_pairs_per_step)
                        .saturating_add(
                            block_index.saturating_mul(AI_LINEAR_TIMESTAMP_PAIRS_PER_STEP),
                        ),
                );
                self.submit_linear_training_block(
                    block,
                    cancel,
                    query_set.as_ref(),
                    timestamp_pair_base,
                )?;
            }
            let step_ms = step_start.elapsed().as_secs_f64() * 1000.0;
            if step_index >= warmup_steps {
                step_latencies_ms.push(step_ms);
                measured_elapsed_s = measured_started
                    .map(|instant| instant.elapsed().as_secs_f64())
                    .unwrap_or_default();
            }

            let now = Instant::now();
            let force = step_index == 0 || step_index + 1 == total_steps;
            if force || now.duration_since(last_emit) >= Duration::from_millis(PROGRESS_SAMPLE_MS) {
                last_emit = now;
                emit_ai_training_progress(
                    tx,
                    phase,
                    step_index + 1,
                    total_steps,
                    started,
                    Some(config.time_limit_s),
                    force,
                );
            }

            pause_between_gpu_submissions(config.gpu_intensity, Some(cancel))?;
        }

        if step_latencies_ms.is_empty() {
            return Err(anyhow!("No measured AI training steps completed"));
        }
        let measured_steps = step_latencies_ms.len();
        if measured_elapsed_s <= 0.0 {
            measured_elapsed_s = step_latencies_ms.iter().sum::<f64>() / 1000.0;
        }

        let flops_per_step = config_flops_per_step(&config);
        let total_flops = flops_per_step * measured_steps as f64;
        let compute_ms = if let (Some(query_set), Some(resolve), Some(readback)) =
            (&query_set, &timestamp_resolve, &timestamp_readback)
        {
            self.resolve_compute_timestamps(
                query_set,
                resolve,
                readback,
                measured_steps.saturating_mul(timestamp_pairs_per_step),
                cancel,
            )
            .ok()
        } else {
            None
        };
        let compute_tflops =
            compute_ms.map(|compute_ms| total_flops / (compute_ms / 1000.0) / 1.0e12);
        let end_to_end_tflops = total_flops / measured_elapsed_s / 1.0e12;
        let throughput_value = ai_training_throughput(&config, measured_steps, measured_elapsed_s);
        let avg_step_ms = step_latencies_ms.iter().sum::<f64>() / measured_steps as f64;
        let p95_step_ms = percentile_sorted_copy(&step_latencies_ms, 0.95);

        if capped {
            run_notes.push(format!(
                "Stopped after {} measured step(s) at the {} time limit.",
                measured_steps,
                format_elapsed(config.time_limit_s)
            ));
        }
        run_notes.push(if compute_ms.is_some() {
            "GPU compute timing uses timestamp queries.".to_owned()
        } else if self.timestamp_query {
            "Compute-only timestamps unavailable for this run; using CPU-observed step latency."
                .to_owned()
        } else {
            "Adapter does not expose timestamp queries; using CPU-observed step latency.".to_owned()
        });

        let _ = tx.send(AiTrainingWorkerEvent::Log(format!(
            "Completed {} measured step(s): {:.2} end-to-end TFLOP/s, {:.1} {}, avg step {} ms",
            measured_steps,
            end_to_end_tflops,
            throughput_value,
            config.workload.throughput_label(),
            format_ms(Some(avg_step_ms))
        )));

        Ok(AiTrainingResult {
            backend: config.backend,
            workload: config.workload,
            preset: config.preset,
            precision: config.precision,
            gpu_names: vec![config.adapter.label()],
            shape: ai_training_shape_label(config.workload, &config.dimensions),
            flops_per_step,
            measured_steps,
            compute_tflops,
            end_to_end_tflops: Some(end_to_end_tflops),
            throughput_value: Some(throughput_value),
            throughput_label: config.workload.throughput_label(),
            avg_step_ms: Some(avg_step_ms),
            p95_step_ms: Some(p95_step_ms),
            memory_bytes: config_memory_bytes(&config),
            validation,
            notes: run_notes.join(" "),
        })
    }

    fn run_optimizer_stress(
        &self,
        mut config: AiTrainingConfig,
        cancel: &AtomicBool,
        tx: &Sender<AiTrainingWorkerEvent>,
    ) -> Result<AiTrainingResult> {
        let mut run_notes = Vec::new();
        let adapter_memory_limit =
            adapter_memory_limit_bytes(&config.adapter).map(|(bytes, _)| bytes);
        if let Some(note) = auto_size_ai_training_config_for_limits(
            &mut config,
            adapter_memory_limit,
            Some(self.max_storage_buffer_binding_size.min(self.max_buffer_size)),
        ) {
            let _ = tx.send(AiTrainingWorkerEvent::Log(note.clone()));
            run_notes.push(note);
        }

        let parameter_count = config.dimensions.parameter_count;
        if parameter_count == 0 {
            return Err(anyhow!("Optimizer stress parameter count must be non-zero"));
        }
        usize_to_u32(parameter_count, "parameter count")?;
        self.ensure_buffer_elements_fit("optimizer gradient", parameter_count)?;
        let max_chunk_elements = self.optimizer_chunk_elements();

        let total_steps = config.warmup_steps.saturating_add(config.measured_steps);
        let started = Instant::now();
        emit_ai_training_progress(
            tx,
            "Preparing optimizer stress tensors",
            0,
            total_steps,
            started,
            Some(config.time_limit_s),
            true,
        );

        let gradient = self.create_empty_storage_buffer("AI optimizer gradient", parameter_count)?;
        let weights = self.create_empty_storage_buffer("AI optimizer weights", parameter_count)?;
        let weights_t =
            self.create_empty_storage_buffer("AI optimizer transposed weights", parameter_count)?;
        let chunks = self.create_sgd_chunks(
            parameter_count,
            parameter_count,
            1,
            max_chunk_elements,
            &gradient,
            &weights,
            &weights_t,
        )?;

        let timestamp_plan = if self.timestamp_query {
            timestamp_query_plan(
                config
                    .measured_steps
                    .saturating_mul(AI_OPTIMIZER_TIMESTAMP_PAIRS_PER_STEP),
            )
        } else {
            None
        };
        let query_set = timestamp_plan.map(|(query_count, _)| {
            self.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("AI optimizer stress compute timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: query_count,
            })
        });
        let timestamp_resolve = timestamp_plan.map(|(_, timestamp_buffer_size)| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("AI optimizer stress timestamp resolve"),
                size: timestamp_buffer_size,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        });
        let timestamp_readback = timestamp_plan.map(|(_, timestamp_buffer_size)| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("AI optimizer stress timestamp readback"),
                size: timestamp_buffer_size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });

        let warmup_steps = config.warmup_steps;
        let mut step_latencies_ms = Vec::with_capacity(config.measured_steps);
        let mut measured_started = None;
        let mut measured_elapsed_s = 0.0;
        let mut capped = false;
        let mut last_emit = Instant::now() - Duration::from_secs(1);

        for step_index in 0..total_steps {
            check_canceled_with(Some(cancel), "AI optimizer stress canceled")?;
            let measured_index = step_index.saturating_sub(warmup_steps);
            if step_index == warmup_steps {
                measured_started = Some(Instant::now());
            }
            if let Some(measured_started) = measured_started {
                if measured_index > 0
                    && measured_started.elapsed().as_secs_f64() >= config.time_limit_s
                {
                    capped = true;
                    break;
                }
            }

            let phase = if step_index < warmup_steps {
                "Warmup optimizer stress step"
            } else {
                "Measured optimizer stress step"
            };
            let step_start = Instant::now();
            self.submit_optimizer_step(
                &chunks,
                cancel,
                query_set.as_ref(),
                (step_index >= warmup_steps).then_some(measured_index),
            )?;
            let step_ms = step_start.elapsed().as_secs_f64() * 1000.0;
            if step_index >= warmup_steps {
                step_latencies_ms.push(step_ms);
                measured_elapsed_s = measured_started
                    .map(|instant| instant.elapsed().as_secs_f64())
                    .unwrap_or_default();
            }

            let now = Instant::now();
            let force = step_index == 0 || step_index + 1 == total_steps;
            if force || now.duration_since(last_emit) >= Duration::from_millis(PROGRESS_SAMPLE_MS) {
                last_emit = now;
                emit_ai_training_progress(
                    tx,
                    phase,
                    step_index + 1,
                    total_steps,
                    started,
                    Some(config.time_limit_s),
                    force,
                );
            }

            pause_between_gpu_submissions(config.gpu_intensity, Some(cancel))?;
        }

        if step_latencies_ms.is_empty() {
            return Err(anyhow!("No measured AI optimizer stress steps completed"));
        }
        let measured_steps = step_latencies_ms.len();
        if measured_elapsed_s <= 0.0 {
            measured_elapsed_s = step_latencies_ms.iter().sum::<f64>() / 1000.0;
        }

        let flops_per_step = config_flops_per_step(&config);
        let total_flops = flops_per_step * measured_steps as f64;
        let compute_ms = if let (Some(query_set), Some(resolve), Some(readback)) =
            (&query_set, &timestamp_resolve, &timestamp_readback)
        {
            self.resolve_compute_timestamps(
                query_set,
                resolve,
                readback,
                measured_steps.saturating_mul(AI_OPTIMIZER_TIMESTAMP_PAIRS_PER_STEP),
                cancel,
            )
            .ok()
        } else {
            None
        };
        let compute_tflops =
            compute_ms.map(|compute_ms| total_flops / (compute_ms / 1000.0) / 1.0e12);
        let end_to_end_tflops = total_flops / measured_elapsed_s / 1.0e12;
        let throughput_value = ai_training_throughput(&config, measured_steps, measured_elapsed_s);
        let avg_step_ms = step_latencies_ms.iter().sum::<f64>() / measured_steps as f64;
        let p95_step_ms = percentile_sorted_copy(&step_latencies_ms, 0.95);

        if capped {
            run_notes.push(format!(
                "Stopped after {} measured step(s) at the {} time limit.",
                measured_steps,
                format_elapsed(config.time_limit_s)
            ));
        }
        if chunks.len() > 1 {
            run_notes.push(format!(
                "Optimizer pass is chunked into {} dispatches for adapter workgroup limits.",
                chunks.len()
            ));
        }
        run_notes.push(if compute_ms.is_some() {
            "GPU compute timing uses timestamp queries.".to_owned()
        } else if self.timestamp_query {
            "Compute-only timestamps unavailable for this run; using CPU-observed step latency."
                .to_owned()
        } else {
            "Adapter does not expose timestamp queries; using CPU-observed step latency.".to_owned()
        });

        let _ = tx.send(AiTrainingWorkerEvent::Log(format!(
            "Completed {} measured optimizer step(s): {:.2} end-to-end TFLOP/s, {:.1} {}, avg step {} ms",
            measured_steps,
            end_to_end_tflops,
            throughput_value,
            config.workload.throughput_label(),
            format_ms(Some(avg_step_ms))
        )));

        Ok(AiTrainingResult {
            backend: config.backend,
            workload: config.workload,
            preset: config.preset,
            precision: config.precision,
            gpu_names: vec![config.adapter.label()],
            shape: ai_training_shape_label(config.workload, &config.dimensions),
            flops_per_step,
            measured_steps,
            compute_tflops,
            end_to_end_tflops: Some(end_to_end_tflops),
            throughput_value: Some(throughput_value),
            throughput_label: config.workload.throughput_label(),
            avg_step_ms: Some(avg_step_ms),
            p95_step_ms: Some(p95_step_ms),
            memory_bytes: config_memory_bytes(&config),
            validation: "Skipped: optimizer stress validates dispatch completion only".to_owned(),
            notes: run_notes.join(" "),
        })
    }

    fn create_linear_buffers(
        &self,
        batch: usize,
        input: usize,
        output: usize,
        cancel: &AtomicBool,
    ) -> Result<AiTrainingBuffers> {
        check_canceled_with(Some(cancel), "AI training benchmark canceled during tensor setup")?;
        let x = generate_ai_training_values(
            batch
                .checked_mul(input)
                .ok_or_else(|| anyhow!("X tensor size overflow"))?,
            0xA17A_1001,
        );
        let x_t = transpose_row_major(&x, batch, input);
        let weights = generate_ai_training_values(
            input
                .checked_mul(output)
                .ok_or_else(|| anyhow!("weight tensor size overflow"))?,
            0xA17A_2002,
        );
        let weights_t = transpose_row_major(&weights, input, output);
        let dy = generate_ai_training_values(
            batch
                .checked_mul(output)
                .ok_or_else(|| anyhow!("dY tensor size overflow"))?,
            0xA17A_3003,
        );
        let validation_inputs = should_validate_linear_training(batch, input, output).then(|| {
            AiLinearValidationInputs {
                x: x.clone(),
                initial_weights: weights.clone(),
                dy: dy.clone(),
            }
        });

        let y_elements = batch
            .checked_mul(output)
            .ok_or_else(|| anyhow!("Y tensor size overflow"))?;
        let dw_elements = input
            .checked_mul(output)
            .ok_or_else(|| anyhow!("dW tensor size overflow"))?;
        let dx_elements = batch
            .checked_mul(input)
            .ok_or_else(|| anyhow!("dX tensor size overflow"))?;

        let forward_params = AiGemmParams {
            rows: usize_to_u32(batch, "batch size")?,
            cols: usize_to_u32(output, "output dimension")?,
            inner: usize_to_u32(input, "input dimension")?,
            _pad: 0,
        };
        let weight_grad_params = AiGemmParams {
            rows: usize_to_u32(input, "input dimension")?,
            cols: usize_to_u32(output, "output dimension")?,
            inner: usize_to_u32(batch, "batch size")?,
            _pad: 0,
        };
        let input_grad_params = AiGemmParams {
            rows: usize_to_u32(batch, "batch size")?,
            cols: usize_to_u32(input, "input dimension")?,
            inner: usize_to_u32(output, "output dimension")?,
            _pad: 0,
        };
        Ok(AiTrainingBuffers {
            x: self.create_init_buffer("AI X", &x, true),
            x_t: self.create_init_buffer("AI X transpose", &x_t, true),
            weights: self.create_init_buffer("AI weights", &weights, false),
            weights_t: self.create_init_buffer("AI weights transpose", &weights_t, false),
            dy: self.create_init_buffer("AI dY", &dy, true),
            y: self.create_empty_storage_buffer("AI Y", y_elements)?,
            dw: self.create_empty_storage_buffer("AI dW", dw_elements)?,
            dx: self.create_empty_storage_buffer("AI dX", dx_elements)?,
            forward_params: self.create_uniform_buffer("AI forward params", &forward_params),
            weight_grad_params: self.create_uniform_buffer("AI dW params", &weight_grad_params),
            input_grad_params: self.create_uniform_buffer("AI dX params", &input_grad_params),
            validation_inputs,
        })
    }

    fn create_linear_training_block(
        &self,
        spec: AiLinearBlockSpec,
        cancel: &AtomicBool,
    ) -> Result<AiLinearTrainingBlock> {
        validate_ai_linear_dimensions(spec.batch, spec.input, spec.output)?;
        self.ensure_linear_buffers_fit(spec.batch, spec.input, spec.output)?;
        let buffers = self.create_linear_buffers(spec.batch, spec.input, spec.output, cancel)?;
        let forward_bind_group = self.create_gemm_bind_group(
            &buffers.x,
            &buffers.weights,
            &buffers.y,
            &buffers.forward_params,
            "AI proxy forward GEMM bind group",
        );
        let weight_grad_bind_group = self.create_gemm_bind_group(
            &buffers.x_t,
            &buffers.dy,
            &buffers.dw,
            &buffers.weight_grad_params,
            "AI proxy weight-gradient GEMM bind group",
        );
        let input_grad_bind_group = self.create_gemm_bind_group(
            &buffers.dy,
            &buffers.weights_t,
            &buffers.dx,
            &buffers.input_grad_params,
            "AI proxy input-gradient GEMM bind group",
        );
        let parameter_count = spec
            .input
            .checked_mul(spec.output)
            .ok_or_else(|| anyhow!("parameter count overflow"))?;
        let sgd_chunks = self.create_sgd_chunks(
            parameter_count,
            spec.input,
            spec.output,
            self.optimizer_chunk_elements(),
            &buffers.dw,
            &buffers.weights,
            &buffers.weights_t,
        )?;

        Ok(AiLinearTrainingBlock {
            batch: spec.batch,
            input: spec.input,
            output: spec.output,
            _buffers: buffers,
            forward_bind_group,
            weight_grad_bind_group,
            input_grad_bind_group,
            sgd_chunks,
        })
    }

    fn submit_linear_training_block(
        &self,
        block: &AiLinearTrainingBlock,
        cancel: &AtomicBool,
        query_set: Option<&wgpu::QuerySet>,
        timestamp_pair_base: Option<usize>,
    ) -> Result<()> {
        self.submit_linear_step(
            &block.forward_bind_group,
            &block.weight_grad_bind_group,
            &block.input_grad_bind_group,
            &block.sgd_chunks,
            block.batch,
            block.input,
            block.output,
            cancel,
            query_set,
            timestamp_pair_base,
        )
    }

    fn optimizer_chunk_elements(&self) -> usize {
        ai_sgd_chunk_elements_for_workgroup_limit(self.max_compute_workgroups_per_dimension)
    }

    fn create_sgd_chunks(
        &self,
        element_count: usize,
        input_dim: usize,
        output_dim: usize,
        max_chunk_elements: usize,
        gradient: &wgpu::Buffer,
        weights: &wgpu::Buffer,
        weights_t: &wgpu::Buffer,
    ) -> Result<Vec<AiOptimizerChunk>> {
        let mut chunks = Vec::new();
        let input_dim = usize_to_u32(input_dim, "input dimension")?;
        let output_dim = usize_to_u32(output_dim, "output dimension")?;
        let mut start = 0usize;
        while start < element_count {
            let chunk_element_count = max_chunk_elements.min(element_count - start);
            let params = AiSgdParams {
                element_count: usize_to_u32(chunk_element_count, "SGD chunk element count")?,
                input_dim,
                output_dim,
                start_index: usize_to_u32(start, "SGD chunk start")?,
                learning_rate: AI_SGD_LEARNING_RATE,
                _pad1: [0.0; 3],
            };
            let params_buffer = self.create_uniform_buffer("AI SGD chunk params", &params);
            let bind_group =
                self.create_sgd_bind_group(gradient, weights, weights_t, &params_buffer);
            chunks.push(AiOptimizerChunk {
                element_count: chunk_element_count,
                _params: params_buffer,
                bind_group,
            });
            start = start.saturating_add(chunk_element_count);
        }
        debug_assert_eq!(
            chunks.len(),
            ai_sgd_chunk_count(element_count, max_chunk_elements)
        );
        Ok(chunks)
    }

    fn submit_linear_step(
        &self,
        forward_bind_group: &wgpu::BindGroup,
        weight_grad_bind_group: &wgpu::BindGroup,
        input_grad_bind_group: &wgpu::BindGroup,
        sgd_chunks: &[AiOptimizerChunk],
        batch: usize,
        input: usize,
        output: usize,
        cancel: &AtomicBool,
        query_set: Option<&wgpu::QuerySet>,
        timestamp_pair_base: Option<usize>,
    ) -> Result<()> {
        check_canceled_with(Some(cancel), "AI training benchmark canceled")?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("AI training step encoder"),
            });
        {
            let gemm_timestamp_writes =
                query_set
                    .zip(timestamp_pair_base)
                    .map(|(query_set, timestamp_pair_base)| {
                        let base = (timestamp_pair_base * 2) as u32;
                        wgpu::ComputePassTimestampWrites {
                            query_set,
                            beginning_of_pass_write_index: Some(base),
                            end_of_pass_write_index: Some(base + 1),
                        }
                    });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("AI training GEMM pass"),
                timestamp_writes: gemm_timestamp_writes,
            });
            pass.set_pipeline(&self.gemm_pipeline);
            pass.set_bind_group(0, forward_bind_group, &[]);
            pass.dispatch_workgroups(
                usize_to_u32(output, "output dimension")?.div_ceil(TILE_SIZE),
                usize_to_u32(batch, "batch size")?.div_ceil(TILE_SIZE),
                1,
            );
            pass.set_bind_group(0, weight_grad_bind_group, &[]);
            pass.dispatch_workgroups(
                usize_to_u32(output, "output dimension")?.div_ceil(TILE_SIZE),
                usize_to_u32(input, "input dimension")?.div_ceil(TILE_SIZE),
                1,
            );
            pass.set_bind_group(0, input_grad_bind_group, &[]);
            pass.dispatch_workgroups(
                usize_to_u32(input, "input dimension")?.div_ceil(TILE_SIZE),
                usize_to_u32(batch, "batch size")?.div_ceil(TILE_SIZE),
                1,
            );
        }
        {
            let sgd_timestamp_writes =
                query_set
                    .zip(timestamp_pair_base)
                    .map(|(query_set, timestamp_pair_base)| {
                        let base = ((timestamp_pair_base + 1) * 2) as u32;
                        wgpu::ComputePassTimestampWrites {
                            query_set,
                            beginning_of_pass_write_index: Some(base),
                            end_of_pass_write_index: Some(base + 1),
                        }
                    });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("AI training SGD update pass"),
                timestamp_writes: sgd_timestamp_writes,
            });
            pass.set_pipeline(&self.sgd_pipeline);
            for chunk in sgd_chunks {
                pass.set_bind_group(0, &chunk.bind_group, &[]);
                pass.dispatch_workgroups(
                    usize_to_u32(chunk.element_count, "SGD chunk element count")?.div_ceil(256),
                    1,
                    1,
                );
            }
        }
        let submission = self.queue.submit([encoder.finish()]);
        self.wait_for_submission(submission, Some(cancel), "waiting for AI training step")
    }

    fn submit_optimizer_step(
        &self,
        chunks: &[AiOptimizerChunk],
        cancel: &AtomicBool,
        query_set: Option<&wgpu::QuerySet>,
        measured_step_index: Option<usize>,
    ) -> Result<()> {
        check_canceled_with(Some(cancel), "AI optimizer stress canceled")?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("AI optimizer stress encoder"),
            });
        let timestamp_writes =
            query_set
                .zip(measured_step_index)
                .map(|(query_set, measured_step_index)| {
                    let base = (measured_step_index * 2) as u32;
                    wgpu::ComputePassTimestampWrites {
                        query_set,
                        beginning_of_pass_write_index: Some(base),
                        end_of_pass_write_index: Some(base + 1),
                    }
                });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("AI optimizer stress pass"),
                timestamp_writes,
            });
            pass.set_pipeline(&self.sgd_pipeline);
            for chunk in chunks {
                pass.set_bind_group(0, &chunk.bind_group, &[]);
                pass.dispatch_workgroups(
                    usize_to_u32(chunk.element_count, "optimizer chunk element count")?
                        .div_ceil(256),
                    1,
                    1,
                );
            }
        }
        let submission = self.queue.submit([encoder.finish()]);
        self.wait_for_submission(submission, Some(cancel), "waiting for AI optimizer stress step")
    }

    fn create_gemm_bind_group(
        &self,
        a: &wgpu::Buffer,
        b: &wgpu::Buffer,
        c: &wgpu::Buffer,
        params: &wgpu::Buffer,
        label: &'static str,
    ) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.gemm_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: a.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: b.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: c.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params.as_entire_binding(),
                },
            ],
        })
    }

    fn create_sgd_bind_group(
        &self,
        gradient: &wgpu::Buffer,
        weights: &wgpu::Buffer,
        weights_t: &wgpu::Buffer,
        params: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("AI SGD bind group"),
            layout: &self.sgd_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: gradient.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: weights.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: weights_t.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params.as_entire_binding(),
                },
            ],
        })
    }

    fn create_init_buffer(&self, label: &'static str, data: &[f32], read_only: bool) -> wgpu::Buffer {
        let mut usage = wgpu::BufferUsages::STORAGE;
        if !read_only {
            usage |= wgpu::BufferUsages::COPY_SRC;
        }
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(data),
                usage,
            })
    }

    fn create_empty_storage_buffer(&self, label: &'static str, elements: usize) -> Result<wgpu::Buffer> {
        let size = ai_buffer_len_bytes(elements)?;
        Ok(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }))
    }

    fn create_uniform_buffer<T: Pod>(&self, label: &'static str, value: &T) -> wgpu::Buffer {
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::bytes_of(value),
                usage: wgpu::BufferUsages::UNIFORM,
            })
    }

    fn ensure_linear_buffers_fit(&self, batch: usize, input: usize, output: usize) -> Result<()> {
        let buffers = [
            ("X", batch.checked_mul(input).ok_or_else(|| anyhow!("X tensor size overflow"))?),
            (
                "X transpose",
                batch
                    .checked_mul(input)
                    .ok_or_else(|| anyhow!("X transpose tensor size overflow"))?,
            ),
            (
                "weights",
                input
                    .checked_mul(output)
                    .ok_or_else(|| anyhow!("weight tensor size overflow"))?,
            ),
            (
                "weights transpose",
                input
                    .checked_mul(output)
                    .ok_or_else(|| anyhow!("weight transpose tensor size overflow"))?,
            ),
            (
                "dY",
                batch
                    .checked_mul(output)
                    .ok_or_else(|| anyhow!("dY tensor size overflow"))?,
            ),
            (
                "Y",
                batch
                    .checked_mul(output)
                    .ok_or_else(|| anyhow!("Y tensor size overflow"))?,
            ),
            (
                "dW",
                input
                    .checked_mul(output)
                    .ok_or_else(|| anyhow!("dW tensor size overflow"))?,
            ),
            (
                "dX",
                batch
                    .checked_mul(input)
                    .ok_or_else(|| anyhow!("dX tensor size overflow"))?,
            ),
        ];
        for (label, elements) in buffers {
            let bytes = ai_buffer_len_bytes(elements)?;
            if bytes > self.max_storage_buffer_binding_size {
                return Err(anyhow!(
                    "{label} tensor requires {}, above this adapter's storage binding limit of {}",
                    format_bytes(bytes),
                    format_bytes(self.max_storage_buffer_binding_size)
                ));
            }
            if bytes > self.max_buffer_size {
                return Err(anyhow!(
                    "{label} tensor requires {}, above this adapter's buffer size limit of {}",
                    format_bytes(bytes),
                    format_bytes(self.max_buffer_size)
                ));
            }
        }
        Ok(())
    }

    fn ensure_buffer_elements_fit(&self, label: &str, elements: usize) -> Result<()> {
        let bytes = ai_buffer_len_bytes(elements)?;
        if bytes > self.max_storage_buffer_binding_size {
            return Err(anyhow!(
                "{label} tensor requires {}, above this adapter's storage binding limit of {}",
                format_bytes(bytes),
                format_bytes(self.max_storage_buffer_binding_size)
            ));
        }
        if bytes > self.max_buffer_size {
            return Err(anyhow!(
                "{label} tensor requires {}, above this adapter's buffer size limit of {}",
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
            if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
                self.device.destroy();
                return Err(anyhow!("AI training benchmark canceled while GPU work was running"));
            }
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
            thread::sleep(Duration::from_millis(GPU_WAIT_POLL_MS));
        }
    }

    fn resolve_compute_timestamps(
        &self,
        query_set: &wgpu::QuerySet,
        resolve: &wgpu::Buffer,
        readback: &wgpu::Buffer,
        pair_count: usize,
        cancel: &AtomicBool,
    ) -> Result<f64> {
        if pair_count == 0 {
            return Err(anyhow!("no timestamp pairs were recorded"));
        }
        let used_query_count = u32::try_from(pair_count.saturating_mul(2))
            .context("timestamp query count overflow")?;
        let used_timestamp_buffer_size = u64::from(used_query_count) * 8;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("AI training timestamp resolve encoder"),
            });
        encoder.resolve_query_set(query_set, 0..used_query_count, resolve, 0);
        encoder.copy_buffer_to_buffer(resolve, 0, readback, 0, used_timestamp_buffer_size);
        let submission = self.queue.submit([encoder.finish()]);
        self.wait_for_submission(submission, Some(cancel), "waiting for AI timestamp resolve")?;
        let timestamp_pairs = read_timestamps(&self.device, readback, pair_count, Some(cancel))?;
        let (compute_ms, _) = dispatch_stats_from_timestamps(
            Some(timestamp_pairs),
            &[],
            self.queue.get_timestamp_period() as f64,
        );
        compute_ms.ok_or_else(|| anyhow!("timestamp query result was empty"))
    }

    fn read_storage_buffer_f32(
        &self,
        source: &wgpu::Buffer,
        elements: usize,
        label: &'static str,
        cancel: &AtomicBool,
    ) -> Result<Vec<f32>> {
        let size = ai_buffer_len_bytes(elements)?;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("AI training readback encoder"),
            });
        encoder.copy_buffer_to_buffer(source, 0, &readback, 0, size);
        let submission = self.queue.submit([encoder.finish()]);
        self.wait_for_submission(submission, Some(cancel), "waiting for AI validation readback")?;
        read_f32_buffer_cancelable(&self.device, &readback, elements, Some(cancel))
    }
}

fn run_ai_training_benchmark(
    config: AiTrainingConfig,
    cancel: Arc<AtomicBool>,
    tx: Sender<AiTrainingWorkerEvent>,
) -> Result<AiTrainingResult> {
    if config.backend == AiTrainingBackend::PyTorchCuda {
        return run_pytorch_cuda_training_benchmark(config, cancel, tx);
    }
    if config.precision != AiTrainingPrecision::F32 {
        return Err(anyhow!("Only f32 precision is implemented for the current AI training runner"));
    }

    let _ = tx.send(AiTrainingWorkerEvent::Log(format!(
        "Creating portable wgpu AI training GPU runner for {}",
        config.adapter.label()
    )));
    let runner = AiGpuRunner::new(config.adapter.index)?;
    match config.workload {
        AiTrainingWorkload::LinearLayer => runner.run_linear_training(config, &cancel, &tx),
        AiTrainingWorkload::Mlp => runner.run_mlp_training(config, &cancel, &tx),
        AiTrainingWorkload::TransformerBlock => {
            runner.run_transformer_proxy_training(config, &cancel, &tx)
        }
        AiTrainingWorkload::OptimizerStress => runner.run_optimizer_stress(config, &cancel, &tx),
    }
}

fn ai_training_smoke_config_for_workload(
    adapter: AdapterInfo,
    gpu_intensity: GpuIntensity,
    workload: AiTrainingWorkload,
) -> AiTrainingConfig {
    AiTrainingConfig {
        backend: AiTrainingBackend::PortableWgpu,
        pytorch_python: default_pytorch_python_executable(),
        pytorch_cuda_device: 0,
        adapter,
        workload,
        profile: AiTrainingProfile::Quick,
        precision: AiTrainingPrecision::F32,
        preset: AiTrainingPreset::Tiny,
        dimensions: ai_training_smoke_dimensions(workload),
        warmup_steps: 1,
        measured_steps: 2,
        time_limit_s: 10.0,
        gpu_intensity,
        validation_enabled: workload == AiTrainingWorkload::LinearLayer,
        smoke_test: true,
    }
}

fn ai_training_smoke_dimensions(workload: AiTrainingWorkload) -> AiTrainingDimensions {
    match workload {
        AiTrainingWorkload::LinearLayer => AiTrainingDimensions::linear(16, 64, 64),
        AiTrainingWorkload::Mlp => AiTrainingDimensions::mlp(16, 64, 128),
        AiTrainingWorkload::TransformerBlock => AiTrainingDimensions::transformer(1, 16, 64, 4),
        AiTrainingWorkload::OptimizerStress => AiTrainingDimensions::optimizer(262_144),
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn validate_ai_linear_dimensions(batch: usize, input: usize, output: usize) -> Result<()> {
    if batch == 0 || input == 0 || output == 0 {
        return Err(anyhow!("AI training dimensions must be non-zero"));
    }
    usize_to_u32(batch, "batch size")?;
    usize_to_u32(input, "input dimension")?;
    usize_to_u32(output, "output dimension")?;
    let parameter_count = input
        .checked_mul(output)
        .ok_or_else(|| anyhow!("parameter count overflow"))?;
    usize_to_u32(parameter_count, "parameter count")?;
    Ok(())
}

fn usize_to_u32(value: usize, label: &str) -> Result<u32> {
    u32::try_from(value).with_context(|| format!("{label} exceeds GPU shader limits"))
}

fn ai_buffer_len_bytes(elements: usize) -> Result<u64> {
    elements
        .checked_mul(std::mem::size_of::<f32>())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or_else(|| anyhow!("tensor byte length overflow"))
}

fn generate_ai_training_values(len: usize, seed: u64) -> Vec<f32> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            let word = splitmix64(&mut state);
            let normalized = ((word >> 40) as f32 / 16_777_216.0) - 0.5;
            normalized * 0.02
        })
        .collect()
}

fn transpose_row_major(values: &[f32], rows: usize, cols: usize) -> Vec<f32> {
    let mut transposed = vec![0.0; values.len()];
    for row in 0..rows {
        for col in 0..cols {
            transposed[col * rows + row] = values[row * cols + col];
        }
    }
    transposed
}

fn linear_shape_label(dimensions: &AiTrainingDimensions) -> String {
    format!(
        "B{} I{} O{}",
        dimensions.batch_size, dimensions.input_dim, dimensions.output_dim
    )
}

fn ai_training_shape_label(workload: AiTrainingWorkload, dimensions: &AiTrainingDimensions) -> String {
    match workload {
        AiTrainingWorkload::LinearLayer => linear_shape_label(dimensions),
        AiTrainingWorkload::Mlp => format!(
            "B{} H{} E{}",
            dimensions.batch_size, dimensions.hidden_size, dimensions.output_dim
        ),
        AiTrainingWorkload::TransformerBlock => format!(
            "B{} S{} H{} A{}",
            dimensions.batch_size,
            dimensions.sequence_len,
            dimensions.hidden_size,
            dimensions.attention_heads
        ),
        AiTrainingWorkload::OptimizerStress => {
            format!("P{}", dimensions.parameter_count)
        }
    }
}

fn transformer_linear_block_specs(
    dimensions: &AiTrainingDimensions,
) -> Result<Vec<AiLinearBlockSpec>> {
    let batch = dimensions.batch_size.max(1);
    let sequence = dimensions.sequence_len.max(1);
    let hidden = dimensions.hidden_size.max(1);
    let heads = dimensions.attention_heads.clamp(1, hidden);
    let tokens = batch
        .checked_mul(sequence)
        .ok_or_else(|| anyhow!("transformer token count overflow"))?;
    let mlp_dim = hidden
        .checked_mul(4)
        .ok_or_else(|| anyhow!("transformer MLP dimension overflow"))?;
    let head_dim = (hidden / heads).max(1);
    let attention_rows = batch
        .checked_mul(heads)
        .and_then(|value| value.checked_mul(sequence))
        .ok_or_else(|| anyhow!("transformer attention row count overflow"))?;

    let mut specs = Vec::with_capacity(8);
    for _ in 0..4 {
        specs.push(AiLinearBlockSpec {
            batch: tokens,
            input: hidden,
            output: hidden,
        });
    }
    specs.push(AiLinearBlockSpec {
        batch: attention_rows,
        input: head_dim,
        output: sequence,
    });
    specs.push(AiLinearBlockSpec {
        batch: attention_rows,
        input: sequence,
        output: head_dim,
    });
    specs.push(AiLinearBlockSpec {
        batch: tokens,
        input: hidden,
        output: mlp_dim,
    });
    specs.push(AiLinearBlockSpec {
        batch: tokens,
        input: mlp_dim,
        output: hidden,
    });
    Ok(specs)
}

fn auto_size_ai_training_config_for_limits(
    config: &mut AiTrainingConfig,
    memory_limit_bytes: Option<u64>,
    single_buffer_limit_bytes: Option<u64>,
) -> Option<String> {
    let original = config.dimensions.clone();
    let value_bytes = config.precision.bytes_per_value().max(1);
    let max_single_elements = single_buffer_limit_bytes
        .map(|bytes| (bytes / value_bytes).max(1) as usize)
        .unwrap_or(usize::MAX);
    let memory_cap = memory_limit_bytes.map(|bytes| {
        bytes
            .saturating_mul(AI_MEMORY_HEADROOM_NUMERATOR)
            / AI_MEMORY_HEADROOM_DENOMINATOR
    });

    for _ in 0..128 {
        normalize_ai_training_dimensions(config);
        if ai_training_dimensions_fit_limits(config, max_single_elements, memory_cap) {
            break;
        }

        if !reduce_ai_training_dimensions_for_limits(config, max_single_elements, memory_cap) {
            break;
        }
    }
    normalize_ai_training_dimensions(config);

    let original_label = ai_training_shape_label(config.workload, &original);
    let current_label = ai_training_shape_label(config.workload, &config.dimensions);
    if original_label == current_label {
        None
    } else {
        Some(format!(
            "Auto-sized {} from {} to {} for adapter limits.",
            config.workload.label(),
            original_label,
            current_label
        ))
    }
}

fn ai_training_dimensions_fit_limits(
    config: &AiTrainingConfig,
    max_single_elements: usize,
    memory_cap: Option<u64>,
) -> bool {
    let largest_buffer_elements = match config.workload {
        AiTrainingWorkload::LinearLayer => linear_largest_buffer_elements(&config.dimensions),
        AiTrainingWorkload::Mlp => mlp_largest_buffer_elements(&config.dimensions),
        AiTrainingWorkload::TransformerBlock => {
            transformer_largest_buffer_elements(&config.dimensions).unwrap_or(usize::MAX)
        }
        AiTrainingWorkload::OptimizerStress => config.dimensions.parameter_count,
    };
    largest_buffer_elements <= max_single_elements
        && memory_cap.is_none_or(|cap| config_memory_bytes(config) <= cap)
}

fn reduce_ai_training_dimensions_for_limits(
    config: &mut AiTrainingConfig,
    max_single_elements: usize,
    memory_cap: Option<u64>,
) -> bool {
    let memory_over_cap = memory_cap.is_some_and(|cap| config_memory_bytes(config) > cap);
    let dims = &mut config.dimensions;
    match config.workload {
        AiTrainingWorkload::LinearLayer => {
            let weight_elements = dims.input_dim.saturating_mul(dims.output_dim);
            let x_elements = dims.batch_size.saturating_mul(dims.input_dim);
            let y_elements = dims.batch_size.saturating_mul(dims.output_dim);
            if weight_elements > max_single_elements {
                reduce_larger_pair(&mut dims.input_dim, &mut dims.output_dim)
            } else if x_elements > max_single_elements || y_elements > max_single_elements {
                reduce_dimension(&mut dims.batch_size)
            } else if memory_over_cap {
                if dims.batch_size > 1 {
                    reduce_dimension(&mut dims.batch_size)
                } else {
                    reduce_larger_pair(&mut dims.input_dim, &mut dims.output_dim)
                }
            } else {
                false
            }
        }
        AiTrainingWorkload::Mlp => {
            let weight_elements = dims.hidden_size.saturating_mul(dims.output_dim);
            let hidden_activations = dims.batch_size.saturating_mul(dims.hidden_size);
            let expansion_activations = dims.batch_size.saturating_mul(dims.output_dim);
            if weight_elements > max_single_elements {
                reduce_larger_pair(&mut dims.hidden_size, &mut dims.output_dim)
            } else if hidden_activations > max_single_elements
                || expansion_activations > max_single_elements
            {
                reduce_dimension(&mut dims.batch_size)
            } else if memory_over_cap {
                if dims.batch_size > 1 {
                    reduce_dimension(&mut dims.batch_size)
                } else {
                    reduce_larger_pair(&mut dims.hidden_size, &mut dims.output_dim)
                }
            } else {
                false
            }
        }
        AiTrainingWorkload::TransformerBlock => {
            let largest = transformer_largest_buffer_elements(dims).unwrap_or(usize::MAX);
            if largest > max_single_elements || memory_over_cap {
                if dims.sequence_len > 1 {
                    reduce_dimension(&mut dims.sequence_len)
                } else if dims.hidden_size > 1 {
                    reduce_dimension(&mut dims.hidden_size)
                } else {
                    reduce_dimension(&mut dims.batch_size)
                }
            } else {
                false
            }
        }
        AiTrainingWorkload::OptimizerStress => {
            if dims.parameter_count > max_single_elements || memory_over_cap {
                reduce_dimension(&mut dims.parameter_count)
            } else {
                false
            }
        }
    }
}

fn normalize_ai_training_dimensions(config: &mut AiTrainingConfig) {
    let dims = &mut config.dimensions;
    match config.workload {
        AiTrainingWorkload::LinearLayer => {
            dims.batch_size = dims.batch_size.max(1);
            dims.input_dim = dims.input_dim.max(1);
            dims.output_dim = dims.output_dim.max(1);
            dims.sequence_len = 1;
            dims.hidden_size = dims.input_dim.max(dims.output_dim);
            dims.attention_heads = 1;
            dims.parameter_count = dims.input_dim.saturating_mul(dims.output_dim);
        }
        AiTrainingWorkload::Mlp => {
            dims.batch_size = dims.batch_size.max(1);
            dims.hidden_size = dims.hidden_size.max(1);
            dims.output_dim = dims.output_dim.max(1);
            dims.input_dim = dims.hidden_size;
            dims.sequence_len = 1;
            dims.attention_heads = 1;
            dims.parameter_count = dims
                .hidden_size
                .saturating_mul(dims.output_dim)
                .saturating_mul(2);
        }
        AiTrainingWorkload::TransformerBlock => {
            dims.batch_size = dims.batch_size.max(1);
            dims.sequence_len = dims.sequence_len.max(1);
            dims.hidden_size = dims.hidden_size.max(1);
            dims.attention_heads = dims.attention_heads.clamp(1, dims.hidden_size);
            dims.input_dim = dims.hidden_size;
            dims.output_dim = dims.hidden_size.saturating_mul(4).max(1);
            let attention_params = dims.hidden_size.saturating_mul(dims.hidden_size).saturating_mul(4);
            let mlp_params = dims
                .hidden_size
                .saturating_mul(dims.output_dim)
                .saturating_mul(2);
            dims.parameter_count = attention_params.saturating_add(mlp_params);
        }
        AiTrainingWorkload::OptimizerStress => {
            dims.batch_size = 1;
            dims.input_dim = 1;
            dims.output_dim = 1;
            dims.sequence_len = 1;
            dims.hidden_size = 1;
            dims.attention_heads = 1;
            dims.parameter_count = dims.parameter_count.max(1);
        }
    }
}

fn linear_largest_buffer_elements(dimensions: &AiTrainingDimensions) -> usize {
    [
        dimensions.batch_size.saturating_mul(dimensions.input_dim),
        dimensions.batch_size.saturating_mul(dimensions.output_dim),
        dimensions.input_dim.saturating_mul(dimensions.output_dim),
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
}

fn mlp_largest_buffer_elements(dimensions: &AiTrainingDimensions) -> usize {
    [
        dimensions.batch_size.saturating_mul(dimensions.hidden_size),
        dimensions.batch_size.saturating_mul(dimensions.output_dim),
        dimensions.hidden_size.saturating_mul(dimensions.output_dim),
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
}

fn transformer_largest_buffer_elements(dimensions: &AiTrainingDimensions) -> Result<usize> {
    let specs = transformer_linear_block_specs(dimensions)?;
    Ok(specs
        .iter()
        .map(|spec| linear_largest_buffer_elements_for_shape(spec.batch, spec.input, spec.output))
        .max()
        .unwrap_or(0))
}

fn linear_largest_buffer_elements_for_shape(batch: usize, input: usize, output: usize) -> usize {
    [
        batch.saturating_mul(input),
        batch.saturating_mul(output),
        input.saturating_mul(output),
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
}

fn reduce_larger_pair(left: &mut usize, right: &mut usize) -> bool {
    if *left >= *right {
        reduce_dimension(left)
    } else {
        reduce_dimension(right)
    }
}

fn reduce_dimension(value: &mut usize) -> bool {
    if *value <= 1 {
        return false;
    }
    let next = ((*value as f64) * 0.75).floor() as usize;
    *value = next.clamp(1, (*value).saturating_sub(1).max(1));
    true
}

fn linear_memory_bytes(dimensions: &AiTrainingDimensions, precision: AiTrainingPrecision) -> u64 {
    let value_bytes = precision.bytes_per_value();
    let activations = dimensions
        .batch_size
        .saturating_mul(dimensions.input_dim.saturating_add(dimensions.output_dim))
        as u64;
    let weights = dimensions
        .input_dim
        .saturating_mul(dimensions.output_dim) as u64;
    activations
        .saturating_mul(value_bytes)
        .saturating_add(weights.saturating_mul(value_bytes).saturating_mul(4))
}

fn should_validate_linear_training(batch: usize, input: usize, output: usize) -> bool {
    let Some(forward_ops) = batch.checked_mul(input).and_then(|value| value.checked_mul(output)) else {
        return false;
    };
    let reference_ops = forward_ops.saturating_mul(3);
    reference_ops <= AI_LINEAR_VALIDATION_MAX_REFERENCE_OPS
}

fn validate_linear_training_result(
    runner: &AiGpuRunner,
    config: &AiTrainingConfig,
    buffers: &AiTrainingBuffers,
    completed_steps: usize,
    cancel: &AtomicBool,
) -> String {
    if !config.validation_enabled {
        return "Skipped: disabled".to_owned();
    }
    let Some(inputs) = &buffers.validation_inputs else {
        return "Skipped: reference workload too large".to_owned();
    };
    if completed_steps == 0 {
        return "Skipped: no completed steps".to_owned();
    }

    let dims = &config.dimensions;
    let batch = dims.batch_size;
    let input = dims.input_dim;
    let output = dims.output_dim;
    let result = (|| -> Result<String> {
        let y = runner.read_storage_buffer_f32(
            &buffers.y,
            batch
                .checked_mul(output)
                .ok_or_else(|| anyhow!("Y validation size overflow"))?,
            "AI Y validation readback",
            cancel,
        )?;
        let dw = runner.read_storage_buffer_f32(
            &buffers.dw,
            input
                .checked_mul(output)
                .ok_or_else(|| anyhow!("dW validation size overflow"))?,
            "AI dW validation readback",
            cancel,
        )?;
        let dx = runner.read_storage_buffer_f32(
            &buffers.dx,
            batch
                .checked_mul(input)
                .ok_or_else(|| anyhow!("dX validation size overflow"))?,
            "AI dX validation readback",
            cancel,
        )?;
        let weights = runner.read_storage_buffer_f32(
            &buffers.weights,
            input
                .checked_mul(output)
                .ok_or_else(|| anyhow!("weight validation size overflow"))?,
            "AI weight validation readback",
            cancel,
        )?;
        let reference = cpu_linear_training_reference(inputs, batch, input, output, completed_steps);
        let checks = [
            ("Y", compare_f32_slices(&reference.y, &y, 1.0e-3, 1.0e-2)),
            ("dW", compare_f32_slices(&reference.dw, &dw, 1.0e-3, 1.0e-2)),
            ("dX", compare_f32_slices(&reference.dx, &dx, 1.0e-3, 1.0e-2)),
            (
                "W",
                compare_f32_slices(&reference.weights, &weights, 1.0e-3, 1.0e-2),
            ),
        ];
        if let Some((label, diff)) = checks.iter().find(|(_, diff)| !diff.passed) {
            Ok(format!(
                "Failed {label}: max abs {:.3e}, max rel {:.3e}",
                diff.max_abs, diff.max_rel
            ))
        } else {
            Ok(format!(
                "Passed tiny reference after {completed_steps} step(s)"
            ))
        }
    })();

    result.unwrap_or_else(|err| format!("Skipped: {err:#}"))
}

struct AiLinearReferenceOutputs {
    y: Vec<f32>,
    dw: Vec<f32>,
    dx: Vec<f32>,
    weights: Vec<f32>,
}

fn cpu_linear_training_reference(
    inputs: &AiLinearValidationInputs,
    batch: usize,
    input: usize,
    output: usize,
    steps: usize,
) -> AiLinearReferenceOutputs {
    let mut weights = inputs.initial_weights.clone();
    let x_t = transpose_row_major(&inputs.x, batch, input);
    let mut y = vec![0.0; batch * output];
    let mut dw = vec![0.0; input * output];
    let mut dx = vec![0.0; batch * input];

    for _ in 0..steps {
        y = cpu_gemm_row_major(&inputs.x, &weights, batch, output, input);
        dw = cpu_gemm_row_major(&x_t, &inputs.dy, input, output, batch);
        let weights_t = transpose_row_major(&weights, input, output);
        dx = cpu_gemm_row_major(&inputs.dy, &weights_t, batch, input, output);
        for (weight, gradient) in weights.iter_mut().zip(&dw) {
            *weight -= AI_SGD_LEARNING_RATE * *gradient;
        }
    }

    AiLinearReferenceOutputs { y, dw, dx, weights }
}

fn cpu_gemm_row_major(a: &[f32], b: &[f32], rows: usize, cols: usize, inner: usize) -> Vec<f32> {
    let mut c = vec![0.0; rows * cols];
    for row in 0..rows {
        for col in 0..cols {
            let mut sum = 0.0;
            for k in 0..inner {
                sum += a[row * inner + k] * b[k * cols + col];
            }
            c[row * cols + col] = sum;
        }
    }
    c
}

struct F32SliceDiff {
    passed: bool,
    max_abs: f32,
    max_rel: f32,
}

fn compare_f32_slices(expected: &[f32], actual: &[f32], abs_tol: f32, rel_tol: f32) -> F32SliceDiff {
    if expected.len() != actual.len() {
        return F32SliceDiff {
            passed: false,
            max_abs: f32::INFINITY,
            max_rel: f32::INFINITY,
        };
    }
    let mut max_abs = 0.0_f32;
    let mut max_rel = 0.0_f32;
    for (&expected, &actual) in expected.iter().zip(actual) {
        let abs = (expected - actual).abs();
        let rel = abs / expected.abs().max(1.0e-6);
        max_abs = max_abs.max(abs);
        max_rel = max_rel.max(rel);
    }
    F32SliceDiff {
        passed: max_abs <= abs_tol || max_rel <= rel_tol,
        max_abs,
        max_rel,
    }
}

fn emit_ai_training_progress(
    tx: &Sender<AiTrainingWorkerEvent>,
    phase: &str,
    completed_steps: usize,
    total_steps: usize,
    started: Instant,
    time_limit_s: Option<f64>,
    _force: bool,
) {
    let progress = (completed_steps as f32 / total_steps.max(1) as f32).clamp(0.0, 1.0);
    let elapsed_s = started.elapsed().as_secs_f64();
    let eta_s = if progress > 0.001 && progress < 1.0 {
        Some((elapsed_s / progress as f64 - elapsed_s).max(0.0))
    } else {
        time_limit_s.filter(|_| progress < 1.0)
    };
    let _ = tx.send(AiTrainingWorkerEvent::Progress(AiTrainingProgress {
        phase: phase.to_owned(),
        progress,
        elapsed_s,
        eta_s,
        completed_steps,
        total_steps,
    }));
}

fn config_flops_per_step(config: &AiTrainingConfig) -> f64 {
    let dims = &config.dimensions;
    match config.workload {
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

fn config_memory_bytes(config: &AiTrainingConfig) -> u64 {
    let dims = &config.dimensions;
    match config.workload {
        AiTrainingWorkload::LinearLayer => linear_memory_bytes(dims, config.precision),
        _ => {
            let mut shadow = AiTrainingBenchmarkState::new();
            shadow.workload = config.workload;
            shadow.precision = config.precision;
            shadow.dimensions = config.dimensions.clone();
            shadow.estimated_memory_bytes()
        }
    }
}

fn ai_training_throughput(config: &AiTrainingConfig, measured_steps: usize, elapsed_s: f64) -> f64 {
    let units_per_step = match config.workload {
        AiTrainingWorkload::LinearLayer | AiTrainingWorkload::Mlp => {
            config.dimensions.batch_size as f64
        }
        AiTrainingWorkload::TransformerBlock => {
            config.dimensions.batch_size as f64 * config.dimensions.sequence_len as f64
        }
        AiTrainingWorkload::OptimizerStress => config.dimensions.parameter_count as f64,
    };
    units_per_step * measured_steps as f64 / elapsed_s.max(f64::MIN_POSITIVE)
}

fn ai_sgd_chunk_elements_for_workgroup_limit(max_workgroups_per_dimension: u32) -> usize {
    (max_workgroups_per_dimension.max(1) as usize)
        .saturating_mul(256)
        .max(1)
}

fn ai_sgd_chunk_count(element_count: usize, max_chunk_elements: usize) -> usize {
    if element_count == 0 {
        0
    } else {
        element_count.div_ceil(max_chunk_elements.max(1))
    }
}

fn percentile_sorted_copy(values: &[f64], percentile: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = ((sorted.len() - 1) as f64 * percentile.clamp(0.0, 1.0)).ceil() as usize;
    sorted[rank.min(sorted.len() - 1)]
}
