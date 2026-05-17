const GPU_MEMORY_SHADER: &str = r#"
struct Params {
    element_count: u32,
    x_invocations: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<storage, read> src_a: array<vec4<u32>>;
@group(0) @binding(1) var<storage, read> src_b: array<vec4<u32>>;
@group(0) @binding(2) var<storage, read_write> dst: array<vec4<u32>>;
@group(0) @binding(3) var<uniform> params: Params;

@compute @workgroup_size(256, 1, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x + gid.y * params.x_invocations;
    if (i >= params.element_count) {
        return;
    }

    let a = src_a[i];
    let b = src_b[i];
    dst[i] = (a ^ b) + vec4<u32>(0x9E3779B9u);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuMemoryParams {
    element_count: u32,
    x_invocations: u32,
    _pad0: u32,
    _pad1: u32,
}

struct GpuMemoryRunner {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    timestamp_query: bool,
    max_buffer_size: u64,
    max_storage_buffer_binding_size: u64,
    max_compute_workgroups_per_dimension: u32,
}

impl GpuMemoryRunner {
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
        descriptor.label = Some("BenchScope GPU memory benchmark device");
        descriptor.required_features = required_features;
        descriptor.required_limits = requested_limits.clone();

        let (device, queue) = pollster::block_on(adapter.request_device(&descriptor))
            .context("requesting wgpu device for GPU memory benchmark")?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("GPU memory bandwidth shader"),
            source: wgpu::ShaderSource::Wgsl(GPU_MEMORY_SHADER.into()),
        });
        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("GPU memory benchmark bind group layout"),
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
            label: Some("GPU memory benchmark pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("GPU memory bandwidth compute pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        Ok(Self {
            device,
            queue,
            pipeline,
            bind_group_layout,
            timestamp_query,
            max_buffer_size: requested_limits.max_buffer_size,
            max_storage_buffer_binding_size: requested_limits.max_storage_buffer_binding_size,
            max_compute_workgroups_per_dimension: requested_limits
                .max_compute_workgroups_per_dimension,
        })
    }

    fn run_test(
        &self,
        config: &GpuMemoryBenchmarkConfig,
        test: GpuMemoryTestKind,
        test_index: usize,
        total_tests: usize,
        suite_started: Instant,
        cancel: &AtomicBool,
        tx: &Sender<GpuMemoryWorkerEvent>,
    ) -> Result<GpuMemoryBenchmarkResult> {
        match test {
            GpuMemoryTestKind::InternalReadWrite => {
                self.run_internal_read_write(config, test_index, total_tests, suite_started, cancel, tx)
            }
            GpuMemoryTestKind::DeviceCopy => {
                self.run_device_copy(config, test_index, total_tests, suite_started, cancel, tx)
            }
            GpuMemoryTestKind::Upload => {
                self.run_upload(config, test_index, total_tests, suite_started, cancel, tx)
            }
            GpuMemoryTestKind::Readback => {
                self.run_readback(config, test_index, total_tests, suite_started, cancel, tx)
            }
            GpuMemoryTestKind::RoundTrip => {
                self.run_round_trip(config, test_index, total_tests, suite_started, cancel, tx)
            }
        }
    }

    fn run_internal_read_write(
        &self,
        config: &GpuMemoryBenchmarkConfig,
        test_index: usize,
        total_tests: usize,
        suite_started: Instant,
        cancel: &AtomicBool,
        tx: &Sender<GpuMemoryWorkerEvent>,
    ) -> Result<GpuMemoryBenchmarkResult> {
        let mut notes = Vec::new();
        let buffer_size = self.buffer_size_for_test(
            config.requested_buffer_size_bytes,
            GpuMemoryTestKind::InternalReadWrite,
            &mut notes,
        )?;
        let vec4_count = gpu_memory_vec4_count(buffer_size);
        let element_count =
            u32::try_from(vec4_count).context("GPU memory buffer has too many vec4 elements")?;
        let (groups_x, groups_y, x_invocations) = self.dispatch_shape(element_count)?;

        let src_a = self.create_gpu_buffer(
            "GPU memory source A",
            buffer_size,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let src_b = self.create_gpu_buffer(
            "GPU memory source B",
            buffer_size,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let dst = self.create_gpu_buffer(
            "GPU memory shader destination",
            buffer_size,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        self.write_pattern_to_buffer(&src_a, buffer_size, GPU_MEMORY_PATTERN_A_SEED, cancel)?;
        self.write_pattern_to_buffer(&src_b, buffer_size, GPU_MEMORY_PATTERN_B_SEED, cancel)?;

        let params = GpuMemoryParams {
            element_count,
            x_invocations,
            _pad0: 0,
            _pad1: 0,
        };
        let params_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("GPU memory benchmark params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("GPU memory benchmark bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: src_a.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: src_b.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: dst.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        self.dispatch_internal_kernel(&bind_group, groups_x, groups_y, false, cancel)?;

        let mut iteration_ms = Vec::new();
        let mut source = GpuMemoryTimingSource::CpuObserved;
        for iteration in 0..config.iterations {
            self.check_canceled(cancel)?;
            let (elapsed_ms, timing_source) =
                self.dispatch_internal_kernel(&bind_group, groups_x, groups_y, true, cancel)?;
            source = timing_source;
            iteration_ms.push(elapsed_ms);
            self.emit_progress(
                tx,
                suite_started,
                GpuMemoryTestKind::InternalReadWrite,
                test_index,
                total_tests,
                iteration + 1,
                config.iterations,
                GpuMemoryTestKind::InternalReadWrite
                    .bytes_per_iteration(buffer_size)
                    .saturating_mul(u64::from(iteration + 1)),
            );
        }

        let sample = self.copy_buffer_sample(&dst, buffer_size, cancel)?;
        let validation = validate_gpu_memory_internal_sample(&sample);
        let result = make_gpu_memory_result(
            GpuMemoryTestKind::InternalReadWrite,
            &config.adapter,
            buffer_size,
            config.iterations,
            &iteration_ms,
            source,
            validation,
            notes,
        );
        Ok(result)
    }

    fn run_device_copy(
        &self,
        config: &GpuMemoryBenchmarkConfig,
        test_index: usize,
        total_tests: usize,
        suite_started: Instant,
        cancel: &AtomicBool,
        tx: &Sender<GpuMemoryWorkerEvent>,
    ) -> Result<GpuMemoryBenchmarkResult> {
        let mut notes = Vec::new();
        let buffer_size = self.buffer_size_for_test(
            config.requested_buffer_size_bytes,
            GpuMemoryTestKind::DeviceCopy,
            &mut notes,
        )?;
        let src = self.create_gpu_buffer(
            "GPU memory copy source",
            buffer_size,
            wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        );
        let dst = self.create_gpu_buffer(
            "GPU memory copy destination",
            buffer_size,
            wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        );
        self.write_pattern_to_buffer(&src, buffer_size, GPU_MEMORY_PATTERN_A_SEED, cancel)?;
        self.copy_buffer_once(&src, &dst, buffer_size, cancel)?;

        let mut iteration_ms = Vec::new();
        for iteration in 0..config.iterations {
            self.check_canceled(cancel)?;
            let elapsed_ms = self.copy_buffer_once(&src, &dst, buffer_size, cancel)?;
            iteration_ms.push(elapsed_ms);
            self.emit_progress(
                tx,
                suite_started,
                GpuMemoryTestKind::DeviceCopy,
                test_index,
                total_tests,
                iteration + 1,
                config.iterations,
                GpuMemoryTestKind::DeviceCopy
                    .bytes_per_iteration(buffer_size)
                    .saturating_mul(u64::from(iteration + 1)),
            );
        }

        let sample = self.copy_buffer_sample(&dst, buffer_size, cancel)?;
        let validation = validate_gpu_memory_pattern_sample(&sample, GPU_MEMORY_PATTERN_A_SEED);
        Ok(make_gpu_memory_result(
            GpuMemoryTestKind::DeviceCopy,
            &config.adapter,
            buffer_size,
            config.iterations,
            &iteration_ms,
            GpuMemoryTimingSource::CpuObserved,
            validation,
            notes,
        ))
    }

    fn run_upload(
        &self,
        config: &GpuMemoryBenchmarkConfig,
        test_index: usize,
        total_tests: usize,
        suite_started: Instant,
        cancel: &AtomicBool,
        tx: &Sender<GpuMemoryWorkerEvent>,
    ) -> Result<GpuMemoryBenchmarkResult> {
        let mut notes = vec![
            "CPU observed timing includes wgpu staging and driver synchronization.".to_owned(),
        ];
        let buffer_size = self.buffer_size_for_test(
            config.requested_buffer_size_bytes,
            GpuMemoryTestKind::Upload,
            &mut notes,
        )?;
        let host_data = make_gpu_memory_pattern_bytes(buffer_size, GPU_MEMORY_PATTERN_A_SEED)?;
        let gpu_buffer = self.create_gpu_buffer(
            "GPU memory upload destination",
            buffer_size,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        );

        self.write_host_data_to_buffer(&gpu_buffer, &host_data, cancel)?;
        let mut iteration_ms = Vec::new();
        for iteration in 0..config.iterations {
            self.check_canceled(cancel)?;
            let elapsed_ms = self.write_host_data_to_buffer(&gpu_buffer, &host_data, cancel)?;
            iteration_ms.push(elapsed_ms);
            self.emit_progress(
                tx,
                suite_started,
                GpuMemoryTestKind::Upload,
                test_index,
                total_tests,
                iteration + 1,
                config.iterations,
                GpuMemoryTestKind::Upload
                    .bytes_per_iteration(buffer_size)
                    .saturating_mul(u64::from(iteration + 1)),
            );
        }

        let sample = self.copy_buffer_sample(&gpu_buffer, buffer_size, cancel)?;
        let validation = validate_gpu_memory_pattern_sample(&sample, GPU_MEMORY_PATTERN_A_SEED);
        Ok(make_gpu_memory_result(
            GpuMemoryTestKind::Upload,
            &config.adapter,
            buffer_size,
            config.iterations,
            &iteration_ms,
            GpuMemoryTimingSource::CpuObserved,
            validation,
            notes,
        ))
    }

    fn run_readback(
        &self,
        config: &GpuMemoryBenchmarkConfig,
        test_index: usize,
        total_tests: usize,
        suite_started: Instant,
        cancel: &AtomicBool,
        tx: &Sender<GpuMemoryWorkerEvent>,
    ) -> Result<GpuMemoryBenchmarkResult> {
        let mut notes = vec!["CPU observed timing includes copy, wait, map, and unmap.".to_owned()];
        let buffer_size = self.buffer_size_for_test(
            config.requested_buffer_size_bytes,
            GpuMemoryTestKind::Readback,
            &mut notes,
        )?;
        let src = self.create_gpu_buffer(
            "GPU memory readback source",
            buffer_size,
            wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        );
        self.write_pattern_to_buffer(&src, buffer_size, GPU_MEMORY_PATTERN_A_SEED, cancel)?;
        let readback = self.create_gpu_buffer(
            "GPU memory readback staging",
            buffer_size,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );

        let _ = self.readback_once(&src, &readback, buffer_size, cancel)?;
        let mut iteration_ms = Vec::new();
        let mut sample = Vec::new();
        for iteration in 0..config.iterations {
            self.check_canceled(cancel)?;
            let (elapsed_ms, current_sample) =
                self.readback_once(&src, &readback, buffer_size, cancel)?;
            sample = current_sample;
            iteration_ms.push(elapsed_ms);
            self.emit_progress(
                tx,
                suite_started,
                GpuMemoryTestKind::Readback,
                test_index,
                total_tests,
                iteration + 1,
                config.iterations,
                GpuMemoryTestKind::Readback
                    .bytes_per_iteration(buffer_size)
                    .saturating_mul(u64::from(iteration + 1)),
            );
        }

        let validation = validate_gpu_memory_pattern_sample(&sample, GPU_MEMORY_PATTERN_A_SEED);
        Ok(make_gpu_memory_result(
            GpuMemoryTestKind::Readback,
            &config.adapter,
            buffer_size,
            config.iterations,
            &iteration_ms,
            GpuMemoryTimingSource::CpuObserved,
            validation,
            notes,
        ))
    }

    fn run_round_trip(
        &self,
        config: &GpuMemoryBenchmarkConfig,
        test_index: usize,
        total_tests: usize,
        suite_started: Instant,
        cancel: &AtomicBool,
        tx: &Sender<GpuMemoryWorkerEvent>,
    ) -> Result<GpuMemoryBenchmarkResult> {
        let mut notes = vec![
            "CPU observed timing includes upload staging, GPU copy, wait, map, and unmap."
                .to_owned(),
        ];
        let buffer_size = self.buffer_size_for_test(
            config.requested_buffer_size_bytes,
            GpuMemoryTestKind::RoundTrip,
            &mut notes,
        )?;
        let host_data = make_gpu_memory_pattern_bytes(buffer_size, GPU_MEMORY_PATTERN_A_SEED)?;
        let gpu_buffer = self.create_gpu_buffer(
            "GPU memory round trip buffer",
            buffer_size,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        );
        let readback = self.create_gpu_buffer(
            "GPU memory round trip readback",
            buffer_size,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );

        let _ = self.round_trip_once(&gpu_buffer, &readback, &host_data, cancel)?;
        let mut iteration_ms = Vec::new();
        let mut sample = Vec::new();
        for iteration in 0..config.iterations {
            self.check_canceled(cancel)?;
            let (elapsed_ms, current_sample) =
                self.round_trip_once(&gpu_buffer, &readback, &host_data, cancel)?;
            sample = current_sample;
            iteration_ms.push(elapsed_ms);
            self.emit_progress(
                tx,
                suite_started,
                GpuMemoryTestKind::RoundTrip,
                test_index,
                total_tests,
                iteration + 1,
                config.iterations,
                GpuMemoryTestKind::RoundTrip
                    .bytes_per_iteration(buffer_size)
                    .saturating_mul(u64::from(iteration + 1)),
            );
        }

        let validation = validate_gpu_memory_pattern_sample(&sample, GPU_MEMORY_PATTERN_A_SEED);
        Ok(make_gpu_memory_result(
            GpuMemoryTestKind::RoundTrip,
            &config.adapter,
            buffer_size,
            config.iterations,
            &iteration_ms,
            GpuMemoryTimingSource::CpuObserved,
            validation,
            notes,
        ))
    }

    fn dispatch_internal_kernel(
        &self,
        bind_group: &wgpu::BindGroup,
        groups_x: u32,
        groups_y: u32,
        use_timestamps: bool,
        cancel: &AtomicBool,
    ) -> Result<(f64, GpuMemoryTimingSource)> {
        let timestamp_enabled = self.timestamp_query && use_timestamps;
        let query_set = timestamp_enabled.then(|| {
            self.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("GPU memory benchmark timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            })
        });
        let timestamp_resolve = timestamp_enabled.then(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("GPU memory timestamp resolve"),
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        });
        let timestamp_readback = timestamp_enabled.then(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("GPU memory timestamp readback"),
                size: 16,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("GPU memory benchmark compute encoder"),
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
                label: Some("GPU memory bandwidth compute pass"),
                timestamp_writes,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.dispatch_workgroups(groups_x, groups_y, 1);
        }
        if let (Some(query_set), Some(resolve), Some(readback)) =
            (&query_set, &timestamp_resolve, &timestamp_readback)
        {
            encoder.resolve_query_set(query_set, 0..2, resolve, 0);
            encoder.copy_buffer_to_buffer(resolve, 0, readback, 0, 16);
        }

        let start = Instant::now();
        let submission = self.queue.submit([encoder.finish()]);
        self.wait_for_submission(submission, cancel, "waiting for GPU memory kernel")?;
        let observed_ms = start.elapsed().as_secs_f64() * 1000.0;
        if let Some(readback) = &timestamp_readback {
            if let Ok(timestamps) = read_timestamps(&self.device, readback, 1, Some(cancel)) {
                if let Some([begin, end]) = timestamps.into_iter().next() {
                    let delta = end.saturating_sub(begin);
                    let elapsed_ms =
                        (delta as f64 * self.queue.get_timestamp_period() as f64) / 1_000_000.0;
                    return Ok((elapsed_ms, GpuMemoryTimingSource::GpuTimestamp));
                }
            }
        }
        Ok((observed_ms, GpuMemoryTimingSource::CpuObserved))
    }

    fn copy_buffer_once(
        &self,
        src: &wgpu::Buffer,
        dst: &wgpu::Buffer,
        buffer_size: u64,
        cancel: &AtomicBool,
    ) -> Result<f64> {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("GPU memory copy encoder"),
            });
        encoder.copy_buffer_to_buffer(src, 0, dst, 0, buffer_size);
        let start = Instant::now();
        let submission = self.queue.submit([encoder.finish()]);
        self.wait_for_submission(submission, cancel, "waiting for GPU memory copy")?;
        Ok(start.elapsed().as_secs_f64() * 1000.0)
    }

    fn readback_once(
        &self,
        src: &wgpu::Buffer,
        readback: &wgpu::Buffer,
        buffer_size: u64,
        cancel: &AtomicBool,
    ) -> Result<(f64, Vec<u8>)> {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("GPU memory readback encoder"),
            });
        encoder.copy_buffer_to_buffer(src, 0, readback, 0, buffer_size);
        let start = Instant::now();
        let submission = self.queue.submit([encoder.finish()]);
        self.wait_for_submission(submission, cancel, "waiting for GPU memory readback copy")?;
        let sample = self.map_readback_sample(readback, buffer_size, cancel)?;
        Ok((start.elapsed().as_secs_f64() * 1000.0, sample))
    }

    fn round_trip_once(
        &self,
        gpu_buffer: &wgpu::Buffer,
        readback: &wgpu::Buffer,
        host_data: &[u8],
        cancel: &AtomicBool,
    ) -> Result<(f64, Vec<u8>)> {
        let start = Instant::now();
        self.write_host_data_to_buffer_no_wait(gpu_buffer, host_data, cancel)?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("GPU memory round trip encoder"),
            });
        encoder.copy_buffer_to_buffer(gpu_buffer, 0, readback, 0, host_data.len() as u64);
        let submission = self.queue.submit([encoder.finish()]);
        self.wait_for_submission(submission, cancel, "waiting for GPU memory round trip")?;
        let sample = self.map_readback_sample(readback, host_data.len() as u64, cancel)?;
        Ok((start.elapsed().as_secs_f64() * 1000.0, sample))
    }

    fn create_gpu_buffer(
        &self,
        label: &'static str,
        size: u64,
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage,
            mapped_at_creation: false,
        })
    }

    fn write_pattern_to_buffer(
        &self,
        buffer: &wgpu::Buffer,
        buffer_size: u64,
        seed: u32,
        cancel: &AtomicBool,
    ) -> Result<f64> {
        let mut offset = 0_u64;
        let chunk_len = GPU_MEMORY_PATTERN_CHUNK_BYTES.min(buffer_size as usize).max(16);
        let mut chunk = vec![0_u8; chunk_len];
        let start = Instant::now();
        while offset < buffer_size {
            self.check_canceled(cancel)?;
            let bytes_this_chunk = (buffer_size - offset).min(chunk.len() as u64) as usize;
            fill_gpu_memory_pattern_bytes(offset, &mut chunk[..bytes_this_chunk], seed);
            self.queue
                .write_buffer(buffer, offset, &chunk[..bytes_this_chunk]);
            offset += bytes_this_chunk as u64;
        }
        self.submit_empty(cancel, "waiting for GPU memory pattern upload")?;
        Ok(start.elapsed().as_secs_f64() * 1000.0)
    }

    fn write_host_data_to_buffer(
        &self,
        buffer: &wgpu::Buffer,
        host_data: &[u8],
        cancel: &AtomicBool,
    ) -> Result<f64> {
        let start = Instant::now();
        self.write_host_data_to_buffer_no_wait(buffer, host_data, cancel)?;
        self.submit_empty(cancel, "waiting for GPU memory upload")?;
        Ok(start.elapsed().as_secs_f64() * 1000.0)
    }

    fn write_host_data_to_buffer_no_wait(
        &self,
        buffer: &wgpu::Buffer,
        host_data: &[u8],
        cancel: &AtomicBool,
    ) -> Result<()> {
        let mut offset = 0_u64;
        for chunk in host_data.chunks(GPU_MEMORY_PATTERN_CHUNK_BYTES) {
            self.check_canceled(cancel)?;
            self.queue.write_buffer(buffer, offset, chunk);
            offset += chunk.len() as u64;
        }
        Ok(())
    }

    fn copy_buffer_sample(
        &self,
        src: &wgpu::Buffer,
        buffer_size: u64,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>> {
        let sample_size = gpu_memory_sample_size(buffer_size);
        let sample = self.create_gpu_buffer(
            "GPU memory validation sample",
            sample_size,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("GPU memory sample copy encoder"),
            });
        encoder.copy_buffer_to_buffer(src, 0, &sample, 0, sample_size);
        let submission = self.queue.submit([encoder.finish()]);
        self.wait_for_submission(submission, cancel, "waiting for GPU memory validation sample")?;
        self.map_readback_sample(&sample, sample_size, cancel)
    }

    fn map_readback_sample(
        &self,
        buffer: &wgpu::Buffer,
        buffer_size: u64,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>> {
        let sample_size = gpu_memory_sample_size(buffer_size) as usize;
        let slice = buffer.slice(..);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result.map_err(|err| err.to_string()));
        });
        wait_for_map_callback(&self.device, &rx, Some(cancel), "polling GPU memory readback")?;
        let data = slice.get_mapped_range();
        let mut sample = Vec::with_capacity(sample_size);
        sample.extend_from_slice(&data[..sample_size.min(data.len())]);
        drop(data);
        buffer.unmap();
        Ok(sample)
    }

    fn submit_empty(&self, cancel: &AtomicBool, context: &'static str) -> Result<()> {
        let encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("GPU memory empty synchronization encoder"),
            });
        let submission = self.queue.submit([encoder.finish()]);
        self.wait_for_submission(submission, cancel, context)
    }

    fn wait_for_submission(
        &self,
        _submission: wgpu::SubmissionIndex,
        cancel: &AtomicBool,
        context: &'static str,
    ) -> Result<()> {
        let (done_tx, done_rx) = mpsc::channel();
        self.queue.on_submitted_work_done(move || {
            let _ = done_tx.send(());
        });

        loop {
            self.check_canceled(cancel)?;
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

    fn buffer_size_for_test(
        &self,
        requested_size: u64,
        test: GpuMemoryTestKind,
        notes: &mut Vec<String>,
    ) -> Result<u64> {
        let mut limit = self.max_buffer_size;
        if test.needs_storage_binding() {
            limit = limit.min(self.max_storage_buffer_binding_size);
        }
        let mut size = align_gpu_memory_buffer_size(requested_size.min(limit));
        if size != requested_size {
            notes.push(format!(
                "Buffer clamped from {} to {} for this adapter/test limit.",
                format_bytes(requested_size),
                format_bytes(size)
            ));
        }
        if size < 16 {
            return Err(anyhow!("adapter buffer limits are too small for GPU memory benchmark"));
        }
        if test.needs_storage_binding() {
            size = size.max(16);
        }
        Ok(size)
    }

    fn dispatch_shape(&self, element_count: u32) -> Result<(u32, u32, u32)> {
        let groups_total = element_count.div_ceil(GPU_MEMORY_WORKGROUP_SIZE).max(1);
        let max_groups = self.max_compute_workgroups_per_dimension.max(1);
        let groups_x = groups_total.min(max_groups);
        let groups_y = groups_total.div_ceil(groups_x).max(1);
        if groups_y > max_groups {
            return Err(anyhow!(
                "GPU memory dispatch requires {}x{} groups, above this adapter's limit of {} per dimension",
                groups_x,
                groups_y,
                max_groups
            ));
        }
        let x_invocations = groups_x
            .checked_mul(GPU_MEMORY_WORKGROUP_SIZE)
            .ok_or_else(|| anyhow!("GPU memory dispatch shape overflow"))?;
        Ok((groups_x, groups_y, x_invocations))
    }

    fn emit_progress(
        &self,
        tx: &Sender<GpuMemoryWorkerEvent>,
        suite_started: Instant,
        test: GpuMemoryTestKind,
        test_index: usize,
        total_tests: usize,
        iteration: u32,
        iterations: u32,
        bytes_processed: u64,
    ) {
        let current_progress = (iteration as f32 / iterations.max(1) as f32).clamp(0.0, 1.0);
        let suite_progress = ((test_index as f32 + current_progress) / total_tests.max(1) as f32)
            .clamp(0.0, 1.0);
        let elapsed_s = suite_started.elapsed().as_secs_f64();
        let eta_s = (suite_progress > 0.01 && suite_progress < 1.0)
            .then(|| (elapsed_s / suite_progress as f64 - elapsed_s).max(0.0));
        let _ = tx.send(GpuMemoryWorkerEvent::Progress(GpuMemoryProgress {
            current_test: test.label().to_owned(),
            current_progress,
            suite_progress,
            elapsed_s,
            eta_s,
            bytes_processed,
        }));
    }

    fn check_canceled(&self, cancel: &AtomicBool) -> Result<()> {
        if cancel.load(Ordering::Relaxed) {
            self.device.destroy();
            Err(anyhow!("GPU memory benchmark canceled"))
        } else {
            Ok(())
        }
    }
}

fn run_gpu_memory_benchmark(
    config: GpuMemoryBenchmarkConfig,
    cancel: Arc<AtomicBool>,
    tx: Sender<GpuMemoryWorkerEvent>,
) -> Result<Vec<GpuMemoryBenchmarkResult>> {
    let runner = GpuMemoryRunner::new(config.adapter.index)?;
    let _ = tx.send(GpuMemoryWorkerEvent::Log(format!(
        "GPU memory timing: internal shader timestamps {}",
        if runner.timestamp_query {
            "supported"
        } else {
            "unavailable; using CPU-observed timing"
        }
    )));
    let suite_started = Instant::now();
    let total_tests = config.selected_tests.len();
    let mut results = Vec::new();
    for (index, test) in config.selected_tests.iter().copied().enumerate() {
        runner.check_canceled(&cancel)?;
        let _ = tx.send(GpuMemoryWorkerEvent::Log(format!(
            "Running {}: {}",
            test.label(),
            test.description()
        )));
        let result = runner.run_test(
            &config,
            test,
            index,
            total_tests,
            suite_started,
            &cancel,
            &tx,
        )?;
        let _ = tx.send(GpuMemoryWorkerEvent::Log(format!(
            "{} complete: avg {} GB/s, best {} GB/s",
            result.test.label(),
            format_gpu_memory_bandwidth(result.average_bandwidth_gbps),
            format_gpu_memory_bandwidth(result.best_bandwidth_gbps)
        )));
        results.push(result);
    }
    Ok(results)
}

fn make_gpu_memory_result(
    test: GpuMemoryTestKind,
    adapter: &AdapterInfo,
    buffer_size_bytes: u64,
    iterations: u32,
    iteration_ms: &[f64],
    timing_source: GpuMemoryTimingSource,
    validation: String,
    notes: Vec<String>,
) -> GpuMemoryBenchmarkResult {
    let bytes_per_iteration = test.bytes_per_iteration(buffer_size_bytes);
    let bytes_processed = bytes_per_iteration.saturating_mul(iterations as u64);
    let elapsed_ms = iteration_ms.iter().sum::<f64>();
    let average_bandwidth_gbps = gpu_memory_bandwidth_gbps(bytes_processed, elapsed_ms);
    let best_bandwidth_gbps = iteration_ms
        .iter()
        .copied()
        .map(|elapsed_ms| gpu_memory_bandwidth_gbps(bytes_per_iteration, elapsed_ms))
        .fold(0.0, f64::max);

    GpuMemoryBenchmarkResult {
        test,
        adapter: adapter.label(),
        buffer_size_bytes,
        iterations,
        bytes_processed,
        elapsed_ms,
        best_bandwidth_gbps,
        average_bandwidth_gbps,
        timing_source,
        validation,
        notes,
        gpu_temperature: TemperatureSummary::default(),
    }
}

fn gpu_memory_sample_size(buffer_size: u64) -> u64 {
    (GPU_MEMORY_SAMPLE_BYTES.min(buffer_size) / 4 * 4).max(4)
}

fn make_gpu_memory_pattern_bytes(size: u64, seed: u32) -> Result<Vec<u8>> {
    let len = usize::try_from(size).context("GPU memory buffer is too large for host allocation")?;
    let mut bytes = vec![0_u8; len];
    fill_gpu_memory_pattern_bytes(0, &mut bytes, seed);
    Ok(bytes)
}

fn fill_gpu_memory_pattern_bytes(offset_bytes: u64, bytes: &mut [u8], seed: u32) {
    let base_word = offset_bytes / 4;
    for (index, chunk) in bytes.chunks_exact_mut(4).enumerate() {
        let value = gpu_memory_pattern_word(base_word + index as u64, seed);
        chunk.copy_from_slice(&value.to_le_bytes());
    }
}

fn validate_gpu_memory_pattern_sample(sample: &[u8], seed: u32) -> String {
    validate_gpu_memory_sample_with(sample, |word_index| {
        gpu_memory_pattern_word(word_index, seed)
    })
}

fn validate_gpu_memory_internal_sample(sample: &[u8]) -> String {
    validate_gpu_memory_sample_with(sample, gpu_memory_internal_word)
}

fn validate_gpu_memory_sample_with(
    sample: &[u8],
    expected: impl Fn(u64) -> u32,
) -> String {
    if sample.len() < 4 {
        return "Skipped (sample unavailable)".to_owned();
    }
    let mut checked = 0_u64;
    for (index, chunk) in sample.chunks_exact(4).enumerate() {
        let actual = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let expected = expected(index as u64);
        if actual != expected {
            return format!(
                "Failed at word {index}: expected 0x{expected:08X}, got 0x{actual:08X}"
            );
        }
        checked += 1;
    }
    format!("Passed ({checked} sampled words)")
}
