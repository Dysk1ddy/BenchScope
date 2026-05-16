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
