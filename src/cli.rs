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

    if args.iter().any(|arg| arg == "--ram-test") {
        let allocation = arg_value(args, "--ram-size")
            .as_deref()
            .map(parse_ram_allocation)
            .transpose()?
            .unwrap_or(RamAllocation::Auto);
        let memory_info = detect_ram_memory_info()?;
        let planned_bytes = planned_ram_test_bytes(memory_info, allocation);
        println!(
            "Running RAM test with {} allocation (planned {}, installed {}, available {}, budget {})",
            allocation,
            format_bytes(planned_bytes),
            format_bytes(memory_info.total_physical_bytes),
            format_bytes(memory_info.available_physical_bytes),
            format_elapsed(ram_time_budget_seconds(memory_info.total_physical_bytes))
        );
        let (tx, rx) = mpsc::channel();
        drop(rx);
        let cancel = Arc::new(AtomicBool::new(false));
        let result = run_ram_test(
            RamTestConfig {
                allocation,
                memory_info,
            },
            cancel,
            tx,
        )?;
        println!("Status: {}", result.status.label());
        println!("Tested: {}", format_bytes(result.tested_bytes));
        println!("Elapsed: {} ms", format_ms(Some(result.elapsed_ms)));
        println!("Budget: {}", format_elapsed(result.budget_seconds));
        println!(
            "Phases: {}/{}",
            result.completed_phases, result.total_phases
        );
        println!("Checks: {}", result.checks);
        println!("Errors: {}", result.error_count);
        println!("First failure: {}", format_ram_first_failure(&result));
        if !result.notes.is_empty() {
            println!("Notes: {}", result.notes.join("; "));
        }
        return Ok(true);
    }

    if args.iter().any(|arg| arg == "--probe-pytorch-cuda") {
        let python = arg_value(args, "--python")
            .unwrap_or_else(default_pytorch_python_executable);
        let environment = probe_pytorch_cuda(&python)?;
        print_pytorch_cuda_environment(&environment);
        return Ok(true);
    }

    if args.iter().any(|arg| arg == "--ai-training-smoke-test") {
        let backend = arg_value(args, "--ai-training-backend")
            .or_else(|| arg_value(args, "--backend"))
            .as_deref()
            .map(parse_ai_training_backend)
            .transpose()?
            .unwrap_or(AiTrainingBackend::PortableWgpu);
        let adapter_index = arg_value(args, "--adapter")
            .map(|value| value.parse::<usize>())
            .transpose()
            .context("--adapter must be an integer")?;
        let workload = arg_value(args, "--ai-workload")
            .or_else(|| arg_value(args, "--workload"))
            .as_deref()
            .map(parse_ai_training_workload)
            .transpose()?
            .unwrap_or(AiTrainingWorkload::LinearLayer);
        let gpu_intensity = arg_value(args, "--gpu-intensity")
            .as_deref()
            .map(parse_gpu_intensity)
            .transpose()?
            .unwrap_or(GpuIntensity::Safe);
        let pytorch_python = arg_value(args, "--python")
            .unwrap_or_else(default_pytorch_python_executable);
        let pytorch_cuda_device = arg_value(args, "--cuda-device")
            .or_else(|| arg_value(args, "--pytorch-cuda-device"))
            .map(|value| value.parse::<usize>())
            .transpose()
            .context("--cuda-device must be an integer")?
            .unwrap_or(0);
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
            "Running {} {} smoke test on {} with {} GPU intensity",
            backend,
            workload,
            adapter.label(),
            gpu_intensity
        );
        let (tx, rx) = mpsc::channel();
        drop(rx);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut config = ai_training_smoke_config_for_workload(adapter, gpu_intensity, workload);
        config.backend = backend;
        config.pytorch_python = pytorch_python;
        config.pytorch_cuda_device = pytorch_cuda_device;
        let result = run_ai_training_benchmark(config, cancel, tx)?;
        println!("Backend: {}", result.backend);
        println!("Workload: {}", result.workload);
        println!("Shape: {}", result.shape);
        println!("Precision: {}", result.precision);
        println!("Steps: {}", result.measured_steps);
        println!("Compute TFLOP/s: {}", format_optional_tflops(result.compute_tflops));
        println!(
            "End-to-end TFLOP/s: {}",
            format_optional_tflops(result.end_to_end_tflops)
        );
        println!(
            "Throughput: {}",
            result
                .throughput_value
                .map(|value| format!("{value:.1} {}", result.throughput_label))
                .unwrap_or_else(|| "N/A".to_owned())
        );
        println!("Avg step: {} ms", format_ms(result.avg_step_ms));
        println!("p95 step: {} ms", format_ms(result.p95_step_ms));
        println!("Memory: {}", format_bytes(result.memory_bytes));
        println!("Validation: {}", result.validation);
        println!("Notes: {}", result.notes);
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

fn parse_ai_training_workload(value: &str) -> Result<AiTrainingWorkload> {
    match value.to_ascii_lowercase().replace(['_', '-'], "").as_str() {
        "linear" | "linearlayer" => Ok(AiTrainingWorkload::LinearLayer),
        "mlp" => Ok(AiTrainingWorkload::Mlp),
        "transformer" | "transformerblock" | "transformerproxy" => {
            Ok(AiTrainingWorkload::TransformerBlock)
        }
        "optimizer" | "optimizerstress" => Ok(AiTrainingWorkload::OptimizerStress),
        other => Err(anyhow!(
            "--ai-workload must be one of linear, mlp, transformer, or optimizer (got {other})"
        )),
    }
}

fn parse_ai_training_backend(value: &str) -> Result<AiTrainingBackend> {
    match value.to_ascii_lowercase().replace(['_', '-'], "").as_str() {
        "wgpu" | "portable" | "portablewgpu" => Ok(AiTrainingBackend::PortableWgpu),
        "pytorch" | "pytorchcuda" | "cuda" => Ok(AiTrainingBackend::PyTorchCuda),
        other => Err(anyhow!(
            "--ai-training-backend must be one of wgpu or pytorch-cuda (got {other})"
        )),
    }
}

fn print_pytorch_cuda_environment(environment: &PyTorchCudaEnvironment) {
    println!("Python: {}", environment.python);
    println!(
        "Python version: {}",
        environment.python_version.as_deref().unwrap_or("unavailable")
    );
    println!(
        "PyTorch: {}",
        environment.torch_version.as_deref().unwrap_or("unavailable")
    );
    println!(
        "PyTorch CUDA runtime: {}",
        environment
            .torch_cuda_version
            .as_deref()
            .unwrap_or("unavailable")
    );
    println!(
        "cuDNN: {}",
        environment.cudnn_version.as_deref().unwrap_or("unavailable")
    );
    println!(
        "CUDA available: {}",
        if environment.cuda_available { "yes" } else { "no" }
    );
    println!("CUDA device count: {}", environment.device_count);
    for device in &environment.devices {
        println!(
            "[{}] {} | sm_{}{} | memory {}",
            device.index,
            device.name,
            device.capability_major,
            device.capability_minor,
            format_bytes(device.total_memory_bytes)
        );
    }
    println!(
        "Distributed: {}; NCCL: {}",
        if environment.distributed_available {
            "yes"
        } else {
            "no"
        },
        if environment.nccl_available { "yes" } else { "no" }
    );
    if let Some(error) = &environment.error {
        println!("Error: {error}");
    }
}
