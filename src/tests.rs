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
    fn ram_time_budget_scales_to_installed_memory() {
        assert_eq!(ram_time_budget_seconds(8 * RAM_GIB_BYTES), 120.0);
        assert_eq!(ram_time_budget_seconds(16 * RAM_GIB_BYTES), 240.0);
        assert_eq!(ram_time_budget_seconds(32 * RAM_GIB_BYTES), 480.0);
    }

    #[test]
    fn ram_planned_allocation_leaves_headroom() {
        let info = RamMemoryInfo {
            total_physical_bytes: 16 * RAM_GIB_BYTES,
            available_physical_bytes: 10 * RAM_GIB_BYTES,
            memory_load_percent: 38,
        };

        assert_eq!(
            planned_ram_test_bytes(info, RamAllocation::Auto),
            7 * RAM_GIB_BYTES
        );
        assert_eq!(
            planned_ram_test_bytes(info, RamAllocation::Gib4),
            4 * RAM_GIB_BYTES
        );
        assert_eq!(
            planned_ram_test_bytes(info, RamAllocation::Gib8),
            7 * RAM_GIB_BYTES
        );
    }

    #[test]
    fn ram_random_pattern_is_deterministic_and_index_sensitive() {
        let seed = 0x1234_5678_9ABC_DEF0;

        assert_eq!(ram_random_pattern(42, seed), ram_random_pattern(42, seed));
        assert_ne!(ram_random_pattern(42, seed), ram_random_pattern(43, seed));
    }

    #[test]
    fn ram_allocation_parser_accepts_common_sizes() {
        assert_eq!(RamAllocation::parse("auto"), Some(RamAllocation::Auto));
        assert_eq!(RamAllocation::parse("1GiB"), Some(RamAllocation::Gib1));
        assert_eq!(RamAllocation::parse("512 mb"), Some(RamAllocation::Mib512));
        assert_eq!(RamAllocation::parse("bad"), None);
    }

    #[test]
    fn gpu_working_set_counts_four_matrices() {
        assert_eq!(gpu_working_set_bytes(16_384), Some(4 * 1024 * 1024 * 1024));
        assert_eq!(gpu_working_set_bytes(32_768), Some(16 * 1024 * 1024 * 1024));
    }

    #[test]
    fn default_matrix_sizes_include_32768() {
        assert!(DEFAULT_SIZES.contains(&32_768));
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
        assert!(gpu_dispatch_batch_limit(GpuIntensity::High) > gpu_dispatch_batch_limit(GpuIntensity::Safe));
        assert!(
            gpu_repeat_batch_limit(128, GpuIntensity::High)
                > gpu_dispatch_batch_limit(GpuIntensity::High)
        );
        assert_eq!(
            gpu_repeat_batch_limit(2048, GpuIntensity::Safe),
            gpu_dispatch_batch_limit(GpuIntensity::Safe)
        );
    }

    #[test]
    fn gpu_repeat_counter_accumulates_partial_batches() {
        let mut counters = GpuRepeatCounters::default();

        counters.record_batch(0, 10.0);
        assert_eq!(counters.iterations, 0);
        assert_eq!(counters.latest_ms, 10.0);

        counters.record_batch(2, 30.0);
        assert_eq!(counters.iterations, 2);
        assert_eq!(counters.compute_count, 2);
        assert_eq!(counters.total_ms, 40.0);
        assert_eq!(counters.total_compute_ms, 40.0);
        assert_eq!(counters.latest_ms, 20.0);
    }

    #[test]
    fn stress_gpu_backend_keeps_archived_option() {
        assert_eq!(StressGpuBackend::AutoOptimized.label(), "Auto optimized");
        assert_eq!(StressGpuBackend::OptimizedWgpu.label(), "Optimized WGPU");
        assert_eq!(StressGpuBackend::ArchivedWgpu.label(), "Archived WGPU");
        assert!(StressGpuBackend::ALL.contains(&StressGpuBackend::AutoOptimized));
        assert!(StressGpuBackend::ALL.contains(&StressGpuBackend::OptimizedWgpu));
        assert!(StressGpuBackend::ALL.contains(&StressGpuBackend::ArchivedWgpu));
        assert!(StressGpuBackend::AutoOptimized.can_try_native_pytorch());
        assert!(StressGpuBackend::OptimizedWgpu.uses_optimized_wgpu());
        assert!(!StressGpuBackend::ArchivedWgpu.uses_optimized_wgpu());
        assert_eq!(GpuPath::PyTorchRocm.label(), "PyTorch ROCm");
        assert_eq!(GpuPath::PyTorchXpu.label(), "PyTorch XPU");
        assert_eq!(GpuPath::PersistentPanelized.label(), "Panelized");
        assert_eq!(GpuPath::StreamingBlocked.label(), "Streaming");
    }

    #[test]
    fn register_tiny_stress_counts_one_full_4x4_per_lane_round() {
        assert_eq!(register_tiny_stress_equivalent_iterations(256, 4, 8), 2048);
        assert_eq!(register_tiny_stress_equivalent_iterations(256, 8, 8), 512);
        assert_eq!(register_tiny_stress_equivalent_iterations(256, 16, 8), 128);
        assert_eq!(register_tiny_stress_equivalent_iterations(256, 32, 8), 32);
    }

    #[test]
    fn conservative_tiny_stress_profile_limits_single_dispatch_size() {
        assert!(
            gpu_tiny_stress_workgroups(GpuIntensity::Safe, true)
                < gpu_tiny_stress_workgroups(GpuIntensity::Safe, false)
        );
        assert!(
            gpu_register_tiny_stress_rounds(4, GpuIntensity::Safe, true)
                < gpu_register_tiny_stress_rounds(4, GpuIntensity::Safe, false)
        );
        assert_eq!(
            gpu_register_tiny_stress_batch_limit(GpuIntensity::High, true),
            1
        );
    }

    #[test]
    fn stress_rate_formatter_uses_trillion_units() {
        assert_eq!(format_stress_rate_per_min(1_700_000_000_000, 60.0), "1.70T/min");
        assert_eq!(
            format_stress_iterations_per_second(Some(1_700_000_000_000.0)),
            "1.70T/s"
        );
    }

    #[test]
    fn stress_progress_reports_tflops_and_tensor_core_efficiency() {
        let progress = RepeatProgress {
            mode: RepeatMode::Gpu,
            size: 1000,
            duration_s: Some(60.0),
            elapsed_s: 10.0,
            iterations: 120,
            latest_ms: 2.0,
            average_total_ms: 3.0,
            average_compute_ms: Some(2.0),
            theoretical_fp16_tc_fp32_accum_tflops: Some(MetricRange::single(100.0)),
            canceled: false,
        };

        assert_eq!(progress.iterations_per_second(), Some(12.0));
        assert!((progress.throughput_tflops().unwrap() - 1.0).abs() < 0.0001);
        assert!(
            (progress.fp16_tensor_core_efficiency_percent().unwrap()
                .max
                - 1.0)
                .abs()
                < 0.0001
        );
    }

    #[test]
    fn gpu_theoretical_database_matches_desktop_laptop_and_regional_models() {
        assert!(GPU_THEORETICAL_SPECS.len() >= 62);

        let rtx_5090 =
            theoretical_fp16_tc_fp32_accum_tflops_for_adapter("NVIDIA GeForce RTX 5090").unwrap();
        assert!(rtx_5090.is_single());
        assert!((rtx_5090.max - 209.8).abs() < 0.1);

        let rtx_5090_d =
            theoretical_fp16_tc_fp32_accum_tflops_for_adapter("NVIDIA GeForce RTX 5090 D v2")
                .unwrap();
        assert!(rtx_5090_d.is_single());
        assert!((rtx_5090_d.max - 148.4).abs() < 0.1);

        let laptop =
            theoretical_fp16_tc_fp32_accum_tflops_for_adapter("NVIDIA GeForce RTX 4090 Laptop GPU")
                .unwrap();
        assert!(!laptop.is_single());
        assert!((laptop.min - 56.6).abs() < 0.1);
        assert!((laptop.max - 79.4).abs() < 0.1);

        assert_eq!(
            theoretical_gpu_model_name_for_adapter("NVIDIA GeForce RTX 4070 Ti SUPER"),
            Some("GeForce RTX 4070 Ti Super")
        );
        assert_eq!(
            theoretical_gpu_model_name_for_adapter("NVIDIA GeForce RTX 3050 Laptop GPU"),
            Some("GeForce RTX 3050 Laptop GPU")
        );
        assert_eq!(theoretical_gpu_model_name_for_adapter("AMD Radeon"), None);
    }

    #[test]
    fn adapter_vendor_routes_native_pytorch_backends() {
        let mut adapter = AdapterInfo {
            index: 0,
            name: "NVIDIA GeForce RTX 5090".to_owned(),
            backend: wgpu::Backend::Dx12,
            device_type: wgpu::DeviceType::DiscreteGpu,
            vendor: 0x10DE,
            device: 0,
            driver: String::new(),
            timestamp_query: true,
            dedicated_vram_bytes: None,
            dedicated_system_memory_bytes: None,
            shared_system_memory_bytes: None,
        };

        assert_eq!(adapter_vendor(&adapter), GpuVendor::Nvidia);
        assert_eq!(
            native_pytorch_backend_for_adapter(&adapter),
            Some(PyTorchMatrixBackend::Cuda)
        );

        adapter.name = "AMD Radeon RX 7900 XTX".to_owned();
        adapter.vendor = 0x1002;
        assert_eq!(adapter_vendor(&adapter), GpuVendor::Amd);
        assert_eq!(
            native_pytorch_backend_for_adapter(&adapter),
            Some(PyTorchMatrixBackend::Rocm)
        );
        assert!(!adapter_prefers_pytorch_cuda(&adapter));

        adapter.name = "Intel(R) Arc(TM) A770 Graphics".to_owned();
        adapter.vendor = 0x8086;
        assert_eq!(adapter_vendor(&adapter), GpuVendor::Intel);
        assert_eq!(
            native_pytorch_backend_for_adapter(&adapter),
            Some(PyTorchMatrixBackend::Xpu)
        );
        assert!(!adapter_prefers_pytorch_cuda(&adapter));
        assert!(adapter_uses_conservative_tiny_stress(&adapter));

        adapter.name = "Microsoft Basic Render Driver".to_owned();
        adapter.vendor = 0;
        assert_eq!(adapter_vendor(&adapter), GpuVendor::Other);
        assert_eq!(native_pytorch_backend_for_adapter(&adapter), None);

        adapter.device_type = wgpu::DeviceType::IntegratedGpu;
        assert!(adapter_uses_conservative_tiny_stress(&adapter));
    }

    #[test]
    fn small_tile_path_covers_small_power_of_two_matrices() {
        for size in [4, 8, 16, 32] {
            assert!(uses_small_tile_path(size));
            assert!(gpu_small_tile_workgroups(size).unwrap() >= 1);
        }
        assert!(!uses_small_tile_path(64));
    }

    #[test]
    fn gpu_intensity_parser_accepts_aliases() {
        assert_eq!(parse_gpu_intensity("safe").unwrap(), GpuIntensity::Safe);
        assert_eq!(parse_gpu_intensity("maximum").unwrap(), GpuIntensity::High);
        assert!(parse_gpu_intensity("danger").is_err());
    }

    #[test]
    fn ai_linear_flop_accounting_matches_training_step_formula() {
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
        let config = AiTrainingConfig {
            backend: AiTrainingBackend::PortableWgpu,
            pytorch_python: "python".to_owned(),
            pytorch_cuda_device: 0,
            adapter,
            workload: AiTrainingWorkload::LinearLayer,
            profile: AiTrainingProfile::Quick,
            precision: AiTrainingPrecision::F32,
            preset: AiTrainingPreset::Custom,
            dimensions: AiTrainingDimensions::linear(2, 3, 5),
            warmup_steps: 1,
            measured_steps: 2,
            time_limit_s: 10.0,
            gpu_intensity: GpuIntensity::Safe,
            validation_enabled: true,
            smoke_test: false,
        };

        assert_eq!(config_flops_per_step(&config), 210.0);
    }

    #[test]
    fn ai_training_autosizer_reduces_linear_shape_for_memory_cap() {
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
        let mut config = AiTrainingConfig {
            backend: AiTrainingBackend::PortableWgpu,
            pytorch_python: "python".to_owned(),
            pytorch_cuda_device: 0,
            adapter,
            workload: AiTrainingWorkload::LinearLayer,
            profile: AiTrainingProfile::Quick,
            precision: AiTrainingPrecision::F32,
            preset: AiTrainingPreset::Custom,
            dimensions: AiTrainingDimensions::linear(1024, 4096, 4096),
            warmup_steps: 1,
            measured_steps: 2,
            time_limit_s: 10.0,
            gpu_intensity: GpuIntensity::Safe,
            validation_enabled: true,
            smoke_test: false,
        };

        let note = auto_size_ai_training_config_for_limits(
            &mut config,
            Some(64 * 1024 * 1024),
            Some(16 * 1024 * 1024),
        );

        assert!(note.is_some());
        assert!(config_memory_bytes(&config) <= 64 * 1024 * 1024 * 9 / 10);
    }

    #[test]
    fn ai_training_transpose_keeps_row_major_layout() {
        let source = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];

        assert_eq!(
            transpose_row_major(&source, 2, 3),
            vec![1.0, 4.0, 2.0, 5.0, 3.0, 6.0]
        );
    }

    #[test]
    fn ai_training_percentile_uses_sorted_values() {
        let values = vec![8.0, 2.0, 4.0, 10.0, 6.0];

        assert_eq!(percentile_sorted_copy(&values, 0.95), 10.0);
        assert_eq!(percentile_sorted_copy(&values, 0.50), 6.0);
    }

    #[test]
    fn ai_sgd_chunking_handles_large_linear_boundary() {
        let max_chunk = ai_sgd_chunk_elements_for_workgroup_limit(65_535);
        let large_linear_parameters = 4096 * 4096;

        assert_eq!(max_chunk, 16_776_960);
        assert_eq!(ai_sgd_chunk_count(large_linear_parameters, max_chunk), 2);
        assert!(max_chunk.div_ceil(256) <= 65_535);
    }

    #[test]
    fn ai_mlp_flop_accounting_includes_two_training_blocks() {
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
        let config = AiTrainingConfig {
            backend: AiTrainingBackend::PortableWgpu,
            pytorch_python: "python".to_owned(),
            pytorch_cuda_device: 0,
            adapter,
            workload: AiTrainingWorkload::Mlp,
            profile: AiTrainingProfile::Quick,
            precision: AiTrainingPrecision::F32,
            preset: AiTrainingPreset::Custom,
            dimensions: AiTrainingDimensions::mlp(2, 3, 5),
            warmup_steps: 1,
            measured_steps: 2,
            time_limit_s: 10.0,
            gpu_intensity: GpuIntensity::Safe,
            validation_enabled: true,
            smoke_test: false,
        };

        assert_eq!(config_flops_per_step(&config), 460.0);
    }

    #[test]
    fn ai_transformer_proxy_builds_expected_block_sequence() {
        let dims = AiTrainingDimensions::transformer(2, 4, 8, 2);
        let specs = transformer_linear_block_specs(&dims).unwrap();

        assert_eq!(specs.len(), 8);
        assert_eq!(specs[0].batch, 8);
        assert_eq!(specs[0].input, 8);
        assert_eq!(specs[0].output, 8);
        assert_eq!(specs[4].batch, 16);
        assert_eq!(specs[4].input, 4);
        assert_eq!(specs[4].output, 4);
        assert_eq!(specs[6].input, 8);
        assert_eq!(specs[6].output, 32);
    }

    #[test]
    fn ai_optimizer_autosizer_respects_single_buffer_cap() {
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
        let mut config = AiTrainingConfig {
            backend: AiTrainingBackend::PortableWgpu,
            pytorch_python: "python".to_owned(),
            pytorch_cuda_device: 0,
            adapter,
            workload: AiTrainingWorkload::OptimizerStress,
            profile: AiTrainingProfile::Quick,
            precision: AiTrainingPrecision::F32,
            preset: AiTrainingPreset::Custom,
            dimensions: AiTrainingDimensions::optimizer(64_000_000),
            warmup_steps: 1,
            measured_steps: 2,
            time_limit_s: 10.0,
            gpu_intensity: GpuIntensity::Safe,
            validation_enabled: true,
            smoke_test: false,
        };

        let note = auto_size_ai_training_config_for_limits(
            &mut config,
            Some(128 * 1024 * 1024),
            Some(16 * 1024 * 1024),
        );

        assert!(note.is_some());
        assert!(config.dimensions.parameter_count <= 4 * 1024 * 1024);
        assert!(config_memory_bytes(&config) <= 128 * 1024 * 1024 * 9 / 10);
    }

    #[test]
    fn pytorch_cuda_probe_parser_reads_environment_and_devices() {
        let output = concat!(
            "PYTHON\t3.13.1\n",
            "TORCH\t2.11.0\n",
            "CUDA\t12.8\n",
            "CUDNN\t9000\n",
            "DISTRIBUTED_AVAILABLE\tTrue\n",
            "NCCL_AVAILABLE\tTrue\n",
            "CUDA_AVAILABLE\tTrue\n",
            "DEVICE_COUNT\t1\n",
            "DEVICE\t0\tNVIDIA Test\t8\t9\t17179869184\n",
        );

        let environment = parse_pytorch_cuda_probe_output(output, r#"C:\Python\python.exe"#);

        assert!(environment.cuda_available);
        assert_eq!(environment.python, r#"C:\Python\python.exe"#);
        assert_eq!(environment.python_version.as_deref(), Some("3.13.1"));
        assert_eq!(environment.torch_version.as_deref(), Some("2.11.0"));
        assert_eq!(environment.torch_cuda_version.as_deref(), Some("12.8"));
        assert_eq!(environment.cudnn_version.as_deref(), Some("9000"));
        assert!(environment.distributed_available);
        assert!(environment.nccl_available);
        assert_eq!(environment.device_count, 1);
        assert_eq!(environment.devices[0].name, "NVIDIA Test");
        assert_eq!(environment.devices[0].capability_major, 8);
        assert_eq!(environment.devices[0].total_memory_bytes, 17_179_869_184);
    }

    #[test]
    fn py_launcher_parser_reads_registered_python_paths() {
        let output = concat!(
            " -V:3.14 *        C:\\Users\\epicm\\AppData\\Local\\Python\\pythoncore-3.14-64\\python.exe\n",
            " -V:3.11          C:\\Users\\epicm\\AppData\\Local\\Programs\\Python\\Python311\\python.exe\n",
        );

        let paths = parse_py_launcher_python_paths(output);

        assert_eq!(
            paths,
            vec![
                r"C:\Users\epicm\AppData\Local\Python\pythoncore-3.14-64\python.exe",
                r"C:\Users\epicm\AppData\Local\Programs\Python\Python311\python.exe",
            ]
        );
    }

    #[test]
    fn preferred_pytorch_python_candidates_are_deduplicated() {
        let candidates = pytorch_python_candidates_with_preferred("python");

        assert_eq!(candidates.iter().filter(|candidate| *candidate == "python").count(), 1);
        assert_eq!(candidates.first().map(String::as_str), Some("python"));
    }

    #[test]
    fn pytorch_cuda_benchmark_parser_reads_timings_and_memory() {
        let output = concat!(
            "PYTHON\t3.13.1\n",
            "TORCH\t2.11.0\n",
            "CUDA\t12.8\n",
            "CUDNN\t9000\n",
            "DISTRIBUTED_AVAILABLE\tTrue\n",
            "NCCL_AVAILABLE\tTrue\n",
            "CUDA_AVAILABLE\tTrue\n",
            "DEVICE_COUNT\t1\n",
            "DEVICE\t0\tNVIDIA Test\t8\t9\t17179869184\n",
            "RESULT_DEVICE_INDEX\t0\n",
            "RESULT_GPU_NAME\tNVIDIA Test\n",
            "RESULT_MEASURED_STEPS\t2\n",
            "RESULT_GPU_STEP_MS\t1.25\t1.50\n",
            "RESULT_WALL_STEP_MS\t1.75\t2.25\n",
            "RESULT_FORWARD_LOSS_MS\t0.25\t0.35\n",
            "RESULT_BACKWARD_MS\t0.75\t0.85\n",
            "RESULT_OPTIMIZER_MS\t0.10\t0.12\n",
            "RESULT_PEAK_ALLOCATED_BYTES\t1024\n",
            "RESULT_PEAK_RESERVED_BYTES\t2048\n",
            "RESULT_VALIDATION\tPassed: finite loss 0.5\n",
            "RESULT_TIME_LIMITED\tFalse\n",
            "NOTE\tbenchmark note\n",
        );

        let benchmark = parse_pytorch_cuda_benchmark_output(output, "python");

        assert_eq!(benchmark.device_index, Some(0));
        assert_eq!(benchmark.gpu_name.as_deref(), Some("NVIDIA Test"));
        assert_eq!(benchmark.measured_steps, 2);
        assert_eq!(benchmark.gpu_step_ms, vec![1.25, 1.50]);
        assert_eq!(benchmark.wall_step_ms, vec![1.75, 2.25]);
        assert_eq!(benchmark.forward_loss_ms, vec![0.25, 0.35]);
        assert_eq!(benchmark.backward_ms, vec![0.75, 0.85]);
        assert_eq!(benchmark.optimizer_ms, vec![0.10, 0.12]);
        assert_eq!(benchmark.peak_allocated_bytes, 1024);
        assert_eq!(benchmark.peak_reserved_bytes, 2048);
        assert_eq!(
            benchmark.validation.as_deref(),
            Some("Passed: finite loss 0.5")
        );
        assert!(!benchmark.time_limited);
        assert_eq!(benchmark.environment.notes, vec!["benchmark note".to_owned()]);
    }

    #[test]
    fn pytorch_matrix_stress_progress_parser_reads_progress_lines() {
        let progress = parse_pytorch_matrix_stress_progress_line(
            "PROGRESS\t3\t12.500000\t40.000000\t38.000000\t3",
        )
        .unwrap();

        assert_eq!(progress.iterations, 3);
        assert_eq!(progress.latest_ms, 12.5);
        assert_eq!(progress.total_ms, 40.0);
        assert_eq!(progress.total_compute_ms, 38.0);
        assert_eq!(progress.compute_count, 3);
        assert!(parse_pytorch_matrix_stress_progress_line("NOTE\tignored").is_none());
    }

    #[test]
    fn pytorch_single_matrix_sample_parser_reads_sample_lines() {
        let sample = parse_pytorch_single_matrix_sample(&["7", "11", "1.25"]).unwrap();

        assert_eq!(sample.row, 7);
        assert_eq!(sample.col, 11);
        assert_eq!(sample.value, 1.25);
    }

    #[test]
    fn pytorch_single_matrix_samples_validate_against_generated_inputs() {
        let (a, b) = generate_matrices(4).unwrap();
        let samples = vec![ValidationSample {
            row: 2,
            col: 3,
            value: (0..4).map(|k| a[2 * 4 + k] * b[k * 4 + 3]).sum(),
        }];

        let validation = validate_samples(&a, &b, &samples, 4, None).unwrap();

        assert!(validation.starts_with("Sampled mixed-precision pass"));
    }

    #[test]
    fn ai_training_backend_parser_accepts_aliases() {
        assert_eq!(
            parse_ai_training_backend("portable-wgpu").unwrap(),
            AiTrainingBackend::PortableWgpu
        );
        assert_eq!(
            parse_ai_training_backend("pytorch_cuda").unwrap(),
            AiTrainingBackend::PyTorchCuda
        );
        assert!(parse_ai_training_backend("rocm").is_err());
    }

    #[test]
    fn ai_training_precision_parser_accepts_mixed_precision_aliases() {
        assert_eq!(
            parse_ai_training_precision("fp32").unwrap(),
            AiTrainingPrecision::F32
        );
        assert_eq!(
            parse_ai_training_precision("bfloat16").unwrap(),
            AiTrainingPrecision::Bf16
        );
        assert_eq!(
            parse_ai_training_precision("half").unwrap(),
            AiTrainingPrecision::F16
        );
        assert!(parse_ai_training_precision("int8").is_err());
    }

    #[test]
    fn pytorch_cuda_validation_allows_requested_training_workloads() {
        let adapter = AdapterInfo {
            index: 0,
            name: "test".to_owned(),
            vendor: 0,
            device: 0,
            device_type: wgpu::DeviceType::DiscreteGpu,
            backend: wgpu::Backend::Dx12,
            driver: String::new(),
            timestamp_query: true,
            dedicated_vram_bytes: Some(32 * 1024 * 1024 * 1024),
            dedicated_system_memory_bytes: Some(0),
            shared_system_memory_bytes: Some(16 * 1024 * 1024 * 1024),
        };
        let mut config = ai_training_smoke_config_for_workload(
            adapter,
            GpuIntensity::Safe,
            AiTrainingWorkload::Mlp,
        );
        config.backend = AiTrainingBackend::PyTorchCuda;
        config.precision = AiTrainingPrecision::Bf16;
        assert!(validate_pytorch_cuda_training_config(&config).is_ok());

        config.workload = AiTrainingWorkload::TransformerBlock;
        config.precision = AiTrainingPrecision::F16;
        config.dimensions = AiTrainingDimensions::transformer(1, 16, 64, 4);
        assert!(validate_pytorch_cuda_training_config(&config).is_ok());

        config.workload = AiTrainingWorkload::OptimizerStress;
        assert!(validate_pytorch_cuda_training_config(&config).is_err());
    }

    #[test]
    fn rtx5090_ai_training_preset_uses_large_shapes() {
        let linear =
            AiTrainingDimensions::for_preset(AiTrainingWorkload::LinearLayer, AiTrainingPreset::Rtx5090);
        assert_eq!(linear.batch_size, 8192);
        assert_eq!(linear.input_dim, 8192);
        assert_eq!(linear.output_dim, 8192);

        let transformer = AiTrainingDimensions::for_preset(
            AiTrainingWorkload::TransformerBlock,
            AiTrainingPreset::Rtx5090,
        );
        assert_eq!(transformer.sequence_len, 4096);
        assert_eq!(transformer.hidden_size, 4096);
        assert_eq!(transformer.attention_heads, 32);
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
    fn infinite_repeat_runs_until_canceled() {
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
        let (tx, _rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = Arc::clone(&cancel);
        let cancel_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            cancel.store(true, Ordering::Relaxed);
        });

        let progress = run_repeat(
            1,
            adapter,
            RepeatMode::Cpu,
            GpuIntensity::Safe,
            StressGpuBackend::OptimizedWgpu,
            String::new(),
            cancel_worker,
            tx,
            RepeatDuration::Infinite,
        )
        .unwrap();
        cancel_thread.join().unwrap();

        assert!(progress.canceled);
        assert_eq!(progress.duration_s, None);
        assert!(progress.elapsed_s > 0.0);
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

    #[test]
    fn drive_temp_file_uses_unique_owned_path_and_preserves_fixed_name() {
        let timestamp_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp_dir = std::env::current_dir()
            .unwrap()
            .join(format!(
                "benchscope-drive-temp-test-{}-{timestamp_ns}",
                std::process::id()
            ));
        fs::create_dir(&temp_dir).unwrap();
        let fixed_name_path = temp_dir.join("benchscope_drive_benchmark.tmp");
        fs::write(&fixed_name_path, b"user data").unwrap();

        let temp_file = DriveBenchmarkTempFile::create(&temp_dir).unwrap();
        let reserved_path = temp_file.path().clone();

        assert_ne!(reserved_path, fixed_name_path);
        assert!(reserved_path.exists());
        drop(temp_file);
        assert!(!reserved_path.exists());
        assert_eq!(fs::read(&fixed_name_path).unwrap(), b"user data");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn storage_health_pending_sector_is_critical() {
        let drive = DriveInfo::with_device_name(PathBuf::from("C:\\"), Some("Test SSD".to_owned()));
        let mut snapshot = StorageHealthSnapshot::unknown(&drive, "test provider");
        snapshot.provider_notes.clear();
        snapshot.pending_sectors = Some(1);

        let snapshot = finalize_storage_health_snapshot(snapshot).unwrap();

        assert_eq!(snapshot.status, StorageHealthStatus::Critical);
        assert!(snapshot.health_percent.is_some_and(|value| value <= 40.0));
        assert!(
            snapshot
                .warnings
                .iter()
                .any(|warning| warning.title == "Current pending sectors")
        );
    }

    #[cfg(windows)]
    #[test]
    fn storage_health_parser_reads_nvme_extended_counters() {
        let drive =
            DriveInfo::with_device_name(PathBuf::from("C:\\"), Some("Test NVMe".to_owned()));
        let output = concat!(
            "HEALTH\tC\tTest NVMe\tSN123\t1.0\tNVMe\tSSD\t1000000\t500000\tNTFS\tHealthy\tOK\t\t42\t12\t100\t10\t0\t0\t0\t0\t0\t100\t200\t\n",
            "NVME\t4\t10\t2\t3\t40\t12345\t67890\t5\t2\t1\t0\t43\t44\t\t\t\t\t\t\n"
        );

        let snapshot = parse_windows_storage_health_output(&drive, output);

        assert_eq!(snapshot.available_spare_percent, Some(4));
        assert_eq!(snapshot.available_spare_threshold_percent, Some(10));
        assert_eq!(snapshot.critical_warning_flags, Some(2));
        assert_eq!(snapshot.unsafe_shutdowns, Some(3));
        assert_eq!(snapshot.controller_busy_time_minutes, Some(40));
        assert_eq!(snapshot.nvme_temperature_sensors_c[0], Some(43.0));
        assert_eq!(snapshot.status, StorageHealthStatus::Critical);
        assert!(
            snapshot
                .warnings
                .iter()
                .any(|warning| warning.title.contains("available spare"))
        );
        assert!(
            snapshot
                .attributes
                .iter()
                .any(|attribute| attribute.name == "NVMe critical warning flags")
        );
    }

    #[test]
    fn storage_health_direct_nvme_log_populates_deep_attributes() {
        fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
            bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
        fn write_u128(bytes: &mut [u8], offset: usize, value: u128) {
            bytes[offset..offset + 16].copy_from_slice(&value.to_le_bytes());
        }

        let mut bytes = vec![0_u8; 512];
        bytes[0] = 0x02;
        write_u16(&mut bytes, 1, 315);
        bytes[3] = 95;
        bytes[4] = 10;
        bytes[5] = 7;
        write_u128(&mut bytes, 32, 2);
        write_u128(&mut bytes, 48, 3);
        write_u128(&mut bytes, 64, 123);
        write_u128(&mut bytes, 80, 456);
        write_u128(&mut bytes, 96, 40);
        write_u128(&mut bytes, 112, 12);
        write_u128(&mut bytes, 128, 3456);
        write_u128(&mut bytes, 144, 1);
        write_u128(&mut bytes, 160, 0);
        write_u128(&mut bytes, 176, 2);
        write_u32_le(&mut bytes, 192, 5);
        write_u32_le(&mut bytes, 196, 0);
        write_u16(&mut bytes, 200, 316);

        let health = parse_nvme_health_log_bytes(&bytes).unwrap();
        let drive =
            DriveInfo::with_device_name(PathBuf::from("C:\\"), Some("Test NVMe".to_owned()));
        let mut snapshot = StorageHealthSnapshot::unknown(&drive, "test provider");
        snapshot.bus_type = "NVMe".to_owned();

        apply_nvme_health_log_to_snapshot(&mut snapshot, &health);
        let snapshot = finalize_storage_health_snapshot(snapshot).unwrap();

        assert_eq!(snapshot.available_spare_percent, Some(95));
        assert_eq!(snapshot.available_spare_threshold_percent, Some(10));
        assert_eq!(snapshot.critical_warning_flags, Some(2));
        assert_eq!(snapshot.unsafe_shutdowns, Some(1));
        assert_eq!(snapshot.power_on_hours, Some(3456));
        assert_eq!(snapshot.power_cycle_count, Some(12));
        assert_eq!(snapshot.nvme_error_info_log_entries, Some(2));
        assert_eq!(snapshot.data_read_bytes, Some(1_024_000));
        assert_eq!(snapshot.data_written_bytes, Some(1_536_000));
        assert!(snapshot.remaining_life_percent.is_some_and(|value| value == 93.0));
        assert!(snapshot.nvme_temperature_sensors_c[0].is_some());
        assert!(
            snapshot
                .attributes
                .iter()
                .any(|attribute| attribute.name == "NVMe unsafe shutdowns"
                    && attribute.display_value == "1")
        );
        assert!(
            snapshot
                .attributes
                .iter()
                .any(|attribute| attribute.name == "NVMe error information log entries"
                    && attribute.display_value == "2")
        );
    }

    #[test]
    fn storage_health_report_mentions_benchmark_results() {
        let drive = DriveInfo::with_device_name(PathBuf::from("C:\\"), Some("Test SSD".to_owned()));
        let mut snapshot = StorageHealthSnapshot::unknown(&drive, "test provider");
        snapshot.status = StorageHealthStatus::Good;
        snapshot.health_percent = Some(98.0);
        let result = make_drive_result(
            DriveTestKind::SequentialRead,
            10_000_000,
            1,
            Duration::from_secs(1),
            256 * 1024 * 1024,
            DriveIoMode::Cached,
            Vec::new(),
            vec!["cached".to_owned()],
        );

        let report = render_storage_health_report(&snapshot, None, &[result]);

        assert!(report.contains("BenchScope Storage Health Report"));
        assert!(report.contains("98% health"));
        assert!(report.contains("Sequential read"));
        assert!(report.contains("cached"));
    }

    #[test]
    fn battery_report_parser_reads_powercfg_xml() {
        let xml = r#"
<BatteryReport>
  <ReportInformation>
    <LocalScanTime>2026-05-16T15:50:52</LocalScanTime>
  </ReportInformation>
  <Batteries>
    <Battery>
      <Id>L21C4PH0</Id>
      <Manufacturer>Celxpert</Manufacturer>
      <SerialNumber> 1377</SerialNumber>
      <Chemistry>LiP</Chemistry>
      <DesignCapacity>75000</DesignCapacity>
      <FullChargeCapacity>63510</FullChargeCapacity>
      <CycleCount>205</CycleCount>
    </Battery>
  </Batteries>
  <RecentUsage>
    <UsageEntry
      LocalTimestamp="2026-05-16T15:39:23"
      Ac="0"
      ChargeCapacity="62350"
      Discharge="4250"
      FullChargeCapacity="63510"
      />
  </RecentUsage>
  <History>
    <HistoryEntry
      LocalStartDate="2026-05-15T00:00:00"
      DesignCapacity="75000"
      FullChargeCapacity="63510"
      CycleCount="205"
      />
  </History>
</BatteryReport>
"#;

        let report = parse_battery_report_xml(xml);
        let battery = report.primary_battery().unwrap();

        assert_eq!(report.generated_at.as_deref(), Some("2026-05-16T15:50:52"));
        assert_eq!(battery.manufacturer.as_deref(), Some("Celxpert"));
        assert_eq!(battery.design_capacity_mwh, Some(75_000.0));
        assert_eq!(battery.full_charge_capacity_mwh, Some(63_510.0));
        assert_eq!(battery.cycle_count, Some(205));
        assert_eq!(report.capacity_history.len(), 1);
        assert_eq!(report.recent_usage.len(), 1);
    }

    #[test]
    fn battery_wear_clamps_capacity_above_design() {
        let battery = BatteryInfo {
            design_capacity_mwh: Some(50_000.0),
            full_charge_capacity_mwh: Some(52_000.0),
            ..Default::default()
        };

        assert_eq!(battery_wear_percent(Some(&battery)), Some(0.0));
    }

    #[test]
    fn battery_runtime_accuracy_uses_discharge_samples() {
        let start = Instant::now();
        let end = start + Duration::from_secs(600);
        let mut samples = VecDeque::new();
        samples.push_back(BatteryLiveSample {
            captured_at: start,
            ac_connected: Some(false),
            status: "Discharging".to_owned(),
            percent: Some(80.0),
            remaining_capacity_mwh: Some(40_000.0),
            charge_rate_watts: None,
            discharge_rate_watts: None,
            windows_runtime_minutes: Some(70.0),
        });
        samples.push_back(BatteryLiveSample {
            captured_at: end,
            ac_connected: Some(false),
            status: "Discharging".to_owned(),
            percent: Some(70.0),
            remaining_capacity_mwh: Some(35_000.0),
            charge_rate_watts: None,
            discharge_rate_watts: None,
            windows_runtime_minutes: Some(70.0),
        });

        let accuracy = battery_runtime_accuracy(&samples).unwrap();

        assert_eq!(accuracy.label, "Good");
        assert!((accuracy.observed_minutes - 70.0).abs() < 0.1);
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
    fn partial_sensor_status_keeps_utilization_live() {
        let mut reading = SensorReading::unavailable(
            SensorKind::Cpu,
            "CPU",
            "Windows safe sensors",
            SensorStatus::Unsupported,
        );

        attach_utilization(
            &mut reading,
            Some(42.0),
            "Windows performance counter",
            "CPU temperature unavailable; utilization is live",
        );

        assert_eq!(reading.temperature_c, None);
        assert_eq!(reading.utilization_percent, Some(42.0));
        assert!(reading.has_utilization());
        assert!(!reading.has_temperature());
        assert_eq!(sensor_temperature(Some(&reading)), None);
        assert!(matches!(reading.status, SensorStatus::Partial(_)));
    }

    #[test]
    fn partial_cpu_temperature_label_explains_missing_safe_provider() {
        let mut reading = SensorReading::unavailable(
            SensorKind::Cpu,
            "CPU",
            "Windows safe sensors",
            SensorStatus::Unsupported,
        );
        attach_utilization(
            &mut reading,
            Some(12.0),
            "Windows performance counter",
            "CPU temperature unavailable; utilization is live",
        );

        assert_eq!(format_sensor_temperature(&reading), "No safe temp");
        assert!(
            format_sensor_temperature_detail(&reading)
                .contains("No safe CPU/GPU temperature provider")
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
    fn helper_snapshot_parser_reads_hwmonitor_metrics() {
        let line = r#"{"timestampUtc":"2026-05-16T03:36:28Z","isElevated":true,"cpu":{"label":"CPU Package","temperatureC":63.5,"provider":"BenchScopeSensorService","status":"ok","metrics":[{"kind":"temperature","label":"CPU Package","value":63.5,"min":41.0,"max":72.0},{"kind":"utilization","label":"Utilization","value":16.0,"min":4.0,"max":91.0},{"kind":"voltage","label":"Vcore","value":1.214,"min":0.711,"max":1.302},{"kind":"power","label":"CPU Package","value":45.5,"min":12.0,"max":88.2},{"kind":"clock","label":"P-core Clock","value":5200.0,"min":800.0,"max":5500.0}]}}"#;

        let snapshot = parse_helper_snapshot(line).unwrap();
        let cpu = snapshot.cpu.unwrap();

        assert_eq!(cpu.temperature_c, Some(63.5));
        assert_eq!(cpu.metrics.len(), 5);
        let power = cpu
            .metrics
            .iter()
            .find(|metric| metric.kind == SensorMetricKind::Power)
            .unwrap();
        assert_eq!(power.label, "CPU Package");
        assert_eq!(power.value, Some(45.5));
        assert_eq!(power.max, Some(88.2));
    }

    #[test]
    fn sensor_snapshot_reset_metric_ranges_rebases_current_values() {
        let mut cpu = SensorReading::ok(SensorKind::Cpu, "CPU", 63.0, "test");
        let temperature = cpu
            .metrics
            .iter_mut()
            .find(|metric| metric.kind == SensorMetricKind::Temperature)
            .unwrap();
        temperature.min = Some(41.0);
        temperature.max = Some(82.0);
        cpu.upsert_metric(
            SensorMetric::new(
                SensorMetricKind::Utilization,
                SensorMetricKind::Utilization.default_label(),
                Some(42.0),
            )
            .with_range(Some(5.0), Some(96.0)),
        );
        let vram = SensorReading {
            kind: SensorKind::GpuMemory,
            label: "VRAM".to_owned(),
            temperature_c: None,
            utilization_percent: None,
            metrics: vec![
                SensorMetric::new(
                    SensorMetricKind::MemoryUsage,
                    SensorMetricKind::MemoryUsage.default_label(),
                    Some(4.0),
                )
                .with_range(Some(1.0), Some(16.0)),
            ],
            provider: "test".to_owned(),
            updated_at: Instant::now(),
            status: SensorStatus::Ok,
        };
        let mut snapshot = SensorSnapshot {
            cpu: Some(cpu),
            gpu_memory: Some(vram),
            ..SensorSnapshot::default()
        };

        snapshot.reset_metric_ranges();

        let cpu = snapshot.cpu.unwrap();
        let temperature = cpu
            .metrics
            .iter()
            .find(|metric| metric.kind == SensorMetricKind::Temperature)
            .unwrap();
        let utilization = cpu
            .metrics
            .iter()
            .find(|metric| metric.kind == SensorMetricKind::Utilization)
            .unwrap();
        let vram = snapshot.gpu_memory.unwrap();
        let memory_usage = vram
            .metrics
            .iter()
            .find(|metric| metric.kind == SensorMetricKind::MemoryUsage)
            .unwrap();

        assert_eq!(temperature.min, Some(63.0));
        assert_eq!(temperature.max, Some(63.0));
        assert_eq!(utilization.min, Some(42.0));
        assert_eq!(utilization.max, Some(42.0));
        assert_eq!(memory_usage.min, Some(4.0));
        assert_eq!(memory_usage.max, Some(16.0));
    }

    #[test]
    fn tracked_metric_ranges_continue_from_reset_values() {
        let mut previous_cpu = SensorReading::ok(SensorKind::Cpu, "CPU", 60.0, "test");
        let previous_temperature = previous_cpu
            .metrics
            .iter_mut()
            .find(|metric| metric.kind == SensorMetricKind::Temperature)
            .unwrap();
        previous_temperature.min = Some(60.0);
        previous_temperature.max = Some(60.0);
        let previous = SensorSnapshot {
            cpu: Some(previous_cpu),
            ..SensorSnapshot::default()
        };
        let mut next_cpu = SensorReading::ok(SensorKind::Cpu, "CPU", 62.0, "test");
        let next_temperature = next_cpu
            .metrics
            .iter_mut()
            .find(|metric| metric.kind == SensorMetricKind::Temperature)
            .unwrap();
        next_temperature.min = Some(40.0);
        next_temperature.max = Some(90.0);

        let next = SensorSnapshot {
            cpu: Some(next_cpu),
            ..SensorSnapshot::default()
        }
        .with_tracked_metric_ranges(Some(&previous));
        let temperature = next
            .cpu
            .unwrap()
            .metrics
            .into_iter()
            .find(|metric| metric.kind == SensorMetricKind::Temperature)
            .unwrap();

        assert_eq!(temperature.min, Some(60.0));
        assert_eq!(temperature.max, Some(62.0));
    }

    #[test]
    fn nvidia_smi_gpu_memory_parser_reads_vram_usage() {
        let reading =
            parse_nvidia_smi_gpu_memory_reading("GeForce RTX Test, 84, 4096, 12288").unwrap();
        let usage = reading
            .metrics
            .iter()
            .find(|metric| metric.kind == SensorMetricKind::MemoryUsage)
            .unwrap();

        assert_eq!(reading.kind, SensorKind::GpuMemory);
        assert_eq!(reading.temperature_c, Some(84.0));
        assert_eq!(usage.value, Some(4.0));
        assert_eq!(usage.max, Some(12.0));
        assert_eq!(
            format_sensor_metric_current_value(usage, &SensorStatus::Ok),
            "4.0/12.0 GB"
        );
    }

    #[test]
    fn helper_snapshot_parser_reads_gpu_memory_metrics() {
        let line = r#"{"timestampUtc":"unix-ms:1770000000000","isElevated":true,"gpuMemory":{"label":"VRAM","temperatureC":86.0,"utilizationPercent":null,"provider":"NVML/nvidia-smi","status":"ok","metrics":[{"kind":"temperature","label":"VRAM","value":86.0,"min":82.0,"max":91.0},{"kind":"memoryUsage","label":"VRAM Used","value":5.5,"min":null,"max":16.0}]}}"#;

        let snapshot = parse_helper_snapshot(line).unwrap();
        let vram = snapshot.gpu_memory.unwrap();
        let usage = vram
            .metrics
            .iter()
            .find(|metric| metric.kind == SensorMetricKind::MemoryUsage)
            .unwrap();

        assert_eq!(vram.kind, SensorKind::GpuMemory);
        assert_eq!(vram.temperature_c, Some(86.0));
        assert_eq!(usage.value, Some(5.5));
        assert_eq!(usage.max, Some(16.0));
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
    fn sensor_service_snapshot_parser_reads_driver_bridge_status() {
        let line = r#"{"timestampUtc":"unix-ms:1770000000000","isElevated":true,"source":"BenchScopeSensorService","driver":{"protocol":1,"version":"0.1.0","cpuTemp":false,"gpuTemp":false,"driveTemp":false,"utilization":false},"cpu":{"label":"CPU","temperatureC":null,"utilizationPercent":null,"provider":"BenchScope sensor driver prototype","status":"unsupported"},"gpu":{"label":"GPU","temperatureC":null,"utilizationPercent":null,"provider":"BenchScope sensor driver prototype","status":"unsupported"},"drive":{"label":"SSD","temperatureC":null,"utilizationPercent":null,"provider":"BenchScope sensor driver prototype","status":"unsupported"},"memory":{"label":"RAM","temperatureC":null,"utilizationPercent":null,"provider":"BenchScope sensor driver prototype","status":"unsupported"}}"#;

        let snapshot = parse_helper_snapshot(line).unwrap();

        assert_eq!(snapshot.helper_elevated, Some(true));
        let cpu = snapshot.cpu.unwrap();
        assert_eq!(cpu.label, "CPU");
        assert_eq!(cpu.provider, "BenchScope sensor driver prototype");
        assert_eq!(cpu.temperature_c, None);
        assert!(matches!(cpu.status, SensorStatus::Unsupported));
    }

    #[test]
    fn sensor_service_snapshot_parser_reads_partial_utilization() {
        let line = r#"{"timestampUtc":"unix-ms:1770000000000","isElevated":true,"source":"BenchScopeSensorService","cpu":{"label":"CPU","temperatureC":null,"utilizationPercent":16.4,"provider":"Windows performance counter","status":"partial","message":"CPU temperature unavailable; utilization is live"}}"#;

        let snapshot = parse_helper_snapshot(line).unwrap();
        let cpu = snapshot.cpu.unwrap();

        assert_eq!(cpu.utilization_percent, Some(16.4));
        assert!(matches!(cpu.status, SensorStatus::Partial(_)));
        assert!(cpu.has_utilization());
        assert!(!cpu.has_temperature());
    }

    #[test]
    fn sensor_service_snapshot_parser_reads_ram_utilization() {
        let line = r#"{"timestampUtc":"unix-ms:1770000000000","isElevated":true,"source":"BenchScopeSensorService","memory":{"label":"System RAM","temperatureC":null,"utilizationPercent":47.2,"provider":"Windows memory status","status":"ok","metrics":[{"kind":"utilization","label":"Utilization","value":47.2,"min":null,"max":null}]}}"#;

        let snapshot = parse_helper_snapshot(line).unwrap();
        let memory = snapshot.memory.unwrap();
        let utilization = memory
            .metrics
            .iter()
            .find(|metric| metric.kind == SensorMetricKind::Utilization)
            .unwrap();

        assert_eq!(memory.label, "System RAM");
        assert_eq!(memory.utilization_percent, Some(47.2));
        assert!(memory.has_utilization());
        assert_eq!(format_sensor_metric_value(utilization.kind, utilization.value), "47%");
    }

    #[test]
    fn sensor_rows_for_feature_views_include_all_devices() {
        let snapshot = SensorSnapshot::default();
        let expected = vec!["CPU", "GPU", "VRAM", "SSD", "RAM"];

        for view in [
            AppView::MatrixBenchmark,
            AppView::MatrixStressTest,
            AppView::GpuMemoryBenchmark,
            AppView::AiTrainingBenchmark,
            AppView::DriveBenchmark,
            AppView::StorageHealth,
            AppView::RamTester,
            AppView::BatteryHealthDiagnostic,
            AppView::NetworkDiagnostic,
            AppView::DeviceInfo,
        ] {
            let labels = sensor_rows_for_view(view, &snapshot)
                .unwrap()
                .into_iter()
                .map(|(label, _)| label)
                .collect::<Vec<_>>();
            assert_eq!(labels, expected);
        }

        assert!(sensor_rows_for_view(AppView::MainMenu, &snapshot).is_none());
    }

    #[test]
    fn helper_snapshot_requests_elevation_when_non_elevated_sensor_is_hidden() {
        let line = r#"{"timestampUtc":"2026-05-16T03:36:28Z","isElevated":false,"cpu":{"label":"CPU","provider":"LibreHardwareMonitor","status":"unsupported","message":"No CPU temperature sensor found"}}"#;
        let snapshot = parse_helper_snapshot(line).unwrap();

        assert!(helper_snapshot_needs_elevation(&snapshot));
    }

    fn sensor_reading_with_temperature_and_utilization(
        kind: SensorKind,
        label: &str,
        temperature_c: f32,
        utilization_percent: f32,
        provider: &str,
    ) -> SensorReading {
        let mut reading = SensorReading::ok(kind, label, temperature_c, provider);
        reading.utilization_percent = Some(utilization_percent);
        reading.sync_legacy_metrics();
        reading
    }

    #[test]
    fn memory_utilization_without_temperature_does_not_request_fallback() {
        let mut memory = SensorReading::unavailable(
            SensorKind::Memory,
            "System RAM",
            "Windows memory status",
            SensorStatus::Ok,
        );
        memory.utilization_percent = Some(44.0);
        memory.sync_legacy_metrics();
        let snapshot = SensorSnapshot {
            cpu: Some(sensor_reading_with_temperature_and_utilization(
                SensorKind::Cpu,
                "CPU",
                55.0,
                12.0,
                "service",
            )),
            gpu: Some(sensor_reading_with_temperature_and_utilization(
                SensorKind::Gpu,
                "GPU",
                50.0,
                18.0,
                "service",
            )),
            gpu_memory: None,
            drive: Some(sensor_reading_with_temperature_and_utilization(
                SensorKind::Drive,
                "SSD",
                41.0,
                3.0,
                "service",
            )),
            memory: Some(memory),
            helper_elevated: Some(true),
        };

        assert!(!sensor_snapshot_needs_fallback(&snapshot, Instant::now()));
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
            gpu_memory: None,
            drive: None,
            memory: None,
            helper_elevated: None,
        };

        let merged = merge_sensor_snapshots(Some(helper), Some(fallback)).unwrap();

        assert_eq!(sensor_temperature(merged.gpu.as_ref()), Some(61.0));
        assert!(merged.gpu.unwrap().provider.contains("NVML/nvidia-smi"));
        assert_eq!(merged.helper_elevated, Some(true));
    }

    #[test]
    fn merge_sensor_snapshots_uses_helper_for_missing_service_temperatures() {
        let service = parse_helper_snapshot(
            r#"{"timestampUtc":"unix-ms:1770000000000","isElevated":true,"source":"BenchScopeSensorService","cpu":{"label":"CPU","temperatureC":null,"utilizationPercent":14.0,"provider":"BenchScope sensor driver bridge","status":"partial","message":"CPU temperature unavailable; utilization is live"},"gpu":{"label":"GPU","temperatureC":null,"utilizationPercent":22.0,"provider":"BenchScope sensor driver bridge","status":"partial","message":"GPU temperature unavailable; utilization is live"}}"#,
        )
        .unwrap();
        let helper = parse_helper_snapshot(
            r#"{"timestampUtc":"2026-05-16T03:36:28Z","isElevated":true,"cpu":{"label":"CPU Package","temperatureC":63.5,"provider":"LibreHardwareMonitor","status":"ok"},"gpu":{"label":"GPU Core","temperatureC":57.0,"provider":"LibreHardwareMonitor","status":"ok"}}"#,
        )
        .unwrap();

        let merged = merge_sensor_snapshots(Some(service), Some(helper)).unwrap();

        let cpu = merged.cpu.unwrap();
        let gpu = merged.gpu.unwrap();
        assert_eq!(cpu.temperature_c, Some(63.5));
        assert_eq!(cpu.utilization_percent, Some(14.0));
        assert_eq!(cpu.status, SensorStatus::Ok);
        assert!(cpu.provider.contains("LibreHardwareMonitor"));
        assert_eq!(gpu.temperature_c, Some(57.0));
        assert_eq!(gpu.utilization_percent, Some(22.0));
        assert_eq!(gpu.status, SensorStatus::Ok);
        assert!(gpu.provider.contains("LibreHardwareMonitor"));
    }

    #[test]
    fn stale_complete_sensor_snapshot_requests_fallback() {
        let now = Instant::now();
        let mut cpu = sensor_reading_with_temperature_and_utilization(
            SensorKind::Cpu,
            "CPU",
            55.0,
            12.0,
            "stale service",
        );
        cpu.updated_at = now - Duration::from_millis(SENSOR_STALE_AFTER_MS + 1);
        let snapshot = SensorSnapshot {
            cpu: Some(cpu),
            gpu: Some(sensor_reading_with_temperature_and_utilization(
                SensorKind::Gpu,
                "GPU",
                50.0,
                18.0,
                "service",
            )),
            gpu_memory: None,
            drive: Some(sensor_reading_with_temperature_and_utilization(
                SensorKind::Drive,
                "SSD",
                41.0,
                3.0,
                "service",
            )),
            memory: Some(sensor_reading_with_temperature_and_utilization(
                SensorKind::Memory,
                "RAM",
                35.0,
                44.0,
                "service",
            )),
            helper_elevated: Some(true),
        };

        assert!(sensor_snapshot_needs_fallback(&snapshot, now));
    }

    #[test]
    fn fresh_fallback_replaces_stale_primary_reading() {
        let now = Instant::now();
        let mut primary_cpu = sensor_reading_with_temperature_and_utilization(
            SensorKind::Cpu,
            "CPU",
            75.0,
            90.0,
            "stale service",
        );
        primary_cpu.updated_at = now - Duration::from_millis(SENSOR_STALE_AFTER_MS + 1);
        let primary = SensorSnapshot {
            cpu: Some(primary_cpu),
            gpu: None,
            gpu_memory: None,
            drive: None,
            memory: None,
            helper_elevated: Some(true),
        };
        let fallback = SensorSnapshot {
            cpu: Some(sensor_reading_with_temperature_and_utilization(
                SensorKind::Cpu,
                "CPU",
                56.0,
                13.0,
                "fresh fallback",
            )),
            gpu: None,
            gpu_memory: None,
            drive: None,
            memory: None,
            helper_elevated: None,
        };

        let merged = merge_sensor_snapshots_prefer_fresh(Some(primary), Some(fallback), now)
            .unwrap()
            .cpu
            .unwrap();

        assert_eq!(merged.temperature_c, Some(56.0));
        assert_eq!(merged.utilization_percent, Some(13.0));
        assert_eq!(merged.provider, "fresh fallback");
    }

    #[test]
    fn disconnected_sensor_bridge_clears_snapshot_for_fallback() {
        let (tx, rx) = mpsc::channel();
        let mut rx = Some(rx);
        let mut snapshot = Some(SensorSnapshot {
            cpu: Some(sensor_reading_with_temperature_and_utilization(
                SensorKind::Cpu,
                "CPU",
                61.0,
                25.0,
                "service",
            )),
            gpu: None,
            gpu_memory: None,
            drive: None,
            memory: None,
            helper_elevated: Some(true),
        });
        drop(tx);

        assert!(drain_sensor_bridge_receiver(&mut rx, &mut snapshot));
        assert!(rx.is_none());
        assert!(snapshot.is_none());
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
                metrics: Vec::new(),
                provider: "LibreHardwareMonitor".to_owned(),
                updated_at: Instant::now(),
                status: SensorStatus::Ok,
            }),
            gpu_memory: None,
            drive: None,
            memory: None,
            helper_elevated: Some(true),
        };

        let snapshot = apply_integrated_gpu_temperature_fallback(snapshot, true);
        let gpu = snapshot.gpu.unwrap();

        assert_eq!(gpu.temperature_c, Some(54.0));
        assert_eq!(gpu.utilization_percent, Some(33.0));
        assert_eq!(gpu.label, "iGPU shared CPU package");
    }

    #[test]
    fn integrated_gpu_fallback_ignores_acpi_thermal_zone_temperature() {
        let snapshot = SensorSnapshot {
            cpu: Some(SensorReading::ok(
                SensorKind::Cpu,
                "CPU",
                28.0,
                "ACPI thermal zone",
            )),
            gpu: Some(SensorReading {
                kind: SensorKind::Gpu,
                label: "Intel Xe Graphics".to_owned(),
                temperature_c: None,
                utilization_percent: Some(33.0),
                metrics: Vec::new(),
                provider: "Windows performance counters".to_owned(),
                updated_at: Instant::now(),
                status: SensorStatus::Ok,
            }),
            gpu_memory: None,
            drive: None,
            memory: None,
            helper_elevated: None,
        };

        let snapshot = apply_integrated_gpu_temperature_fallback(snapshot, true);
        let gpu = snapshot.gpu.unwrap();

        assert_eq!(gpu.temperature_c, None);
        assert_eq!(gpu.utilization_percent, Some(33.0));
        assert_eq!(gpu.label, "Intel Xe Graphics");
    }

    #[test]
    fn panel_height_split_leaves_content_and_log_visible() {
        for available in [320.0, 480.0, 760.0] {
            let (content, log) = panel_content_log_heights(available, 0.18, 150.0);
            assert!(content >= 40.0);
            assert!(log >= 32.0);
            assert!(
                content + log + PANEL_VERTICAL_CHROME_HEIGHT <= available + 0.1
            );
        }
    }

    #[test]
    fn panel_log_resize_bounds_stay_ordered() {
        for usable_height in [80.0, 180.0, 640.0] {
            let (min_log, max_log) = panel_log_resize_bounds(usable_height, 150.0);
            assert!(min_log <= max_log);
            assert!(min_log >= 32.0);
            assert!(max_log <= usable_height.max(32.0));
        }
    }

    #[test]
    fn panel_log_resize_can_grow_past_default_max() {
        let (_min_log, max_log) = panel_log_resize_bounds(640.0, 150.0);

        assert!(max_log > 150.0);
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

    #[cfg(windows)]
    #[test]
    fn device_info_parser_reads_core_inventory_and_driver_dates() {
        let output = concat!(
            "SYSTEM\tContoso\tModel X\tFamily\tSKU1\tx64-based PC\t3; 10\t34359738368\t1\t16\tWORKGROUP\t\tTrue\tuser\r\n",
            "OS\tWindows 11 Pro\t10.0.26100\t26100\tx64\t2025-01-01\t2026-05-17\r\n",
            "BIOS\tContoso\t1.2.3\tCONTOSO - 1\t2026-04-01\tSN-BIOS\r\n",
            "BOARD\tContoso\tBoard X\tRev 1\tSN-BOARD\r\n",
            "CPU\tAMD Ryzen Test\tAuthenticAMD\tAMD64 Family\tAM5\tABC123\t9\t25\t97\t2\t8\t16\t5200\t4200\t8192\t32768\tTrue\tTrue\tTrue\r\n",
            "MEMORY\tG.Skill\tF5-6000\tRAMSN\tBANK 0\tDIMM A1\t17179869184\t6000\t5600\t8\t0\t34\t8192\t64\t64\r\n",
            "DISK\t0\t\\\\.\\PHYSICALDRIVE0\tTest NVMe\tDISKSN\t9B2Q\tSCSI\tSSD\tNVMe\t2000000000000\t3\tOK\tHealthy\tOnline\r\n",
            "VOLUME\tC\tSystem\tNTFS\tFixed\t1000000000\t500000000\tHealthy\tOK\r\n",
            "GPU\tNVIDIA Test GPU\tPCI\\VEN_10DE\tNVIDIA\tNVIDIA Processor\tNVIDIA\t32.0.15\t2026-03-22\tnv_disp.inf\t8589934592\t2560\t1440\t165\tOK\r\n",
            "NETWORK\tEthernet\tIntel Ethernet\tTrue\tAA:BB:CC:DD:EE:FF\t1000000000\tTrue\tIntel\t2.1.0\t2026-02-01\tOK\r\n",
            "MONITOR\tGeneric PnP Monitor\tContoso\tOLED\t2560x1440\tMONITOR\\ABC\tOK\r\n",
            "DRIVER\tDISPLAY\tNVIDIA Test GPU\tNVIDIA\tNVIDIA\t32.0.15\t2026-03-22\tMicrosoft Windows Hardware Compatibility Publisher\tnv_disp.inf\tPCI\\VEN_10DE\tTrue\r\n",
            "NOTE\tProvider note example\r\n",
        );

        let snapshot = parse_windows_device_info_output(output);

        assert_eq!(
            snapshot.system.as_ref().and_then(|system| system.manufacturer.as_deref()),
            Some("Contoso")
        );
        assert_eq!(snapshot.total_ram_bytes(), Some(34_359_738_368));
        assert_eq!(snapshot.cpu_core_count(), Some(8));
        assert_eq!(snapshot.cpu_logical_processor_count(), Some(16));
        assert_eq!(snapshot.cpus[0].architecture.as_deref(), Some("x64"));
        assert_eq!(
            snapshot.memory_modules[0].smbios_memory_type.as_deref(),
            Some("DDR5")
        );
        assert_eq!(snapshot.disks[0].bus_type.as_deref(), Some("NVMe"));
        assert_eq!(snapshot.gpus[0].driver_date.as_deref(), Some("2026-03-22"));
        assert_eq!(snapshot.gpus[0].resolution.as_deref(), Some("2560x1440"));
        assert_eq!(snapshot.network_adapters[0].driver_version.as_deref(), Some("2.1.0"));
        assert_eq!(snapshot.drivers[0].date.as_deref(), Some("2026-03-22"));
        assert_eq!(snapshot.drivers[0].is_signed, Some(true));
        assert_eq!(snapshot.provider_notes, vec!["Provider note example"]);
    }

    #[test]
    fn network_adapter_parser_reads_wifi_details() {
        let row = "12\tWi-Fi\tIntel(R) Wi-Fi 6 AX201\tUp\t866.7 Mbps\tAA-BB-CC-DD-EE-FF\tTrue\tIntel\t1.2.3\t2026-01-01\t2\t192.168.1.10\tfe80::1\t192.168.1.1\t1.1.1.1; 8.8.8.8\tHomeNet\t72\t802.11ax\t36\t866.7\t720.0\t100\t200\t10\t20\t0\t0\t0\t0";
        let adapters = parse_network_adapter_rows(row);

        assert_eq!(adapters.len(), 1);
        let adapter = &adapters[0];
        assert_eq!(adapter.kind, NetworkAdapterKind::Wifi);
        assert!(adapter.connected);
        assert_eq!(adapter.link_speed_bps, Some(866_700_000));
        assert_eq!(
            adapter
                .wifi
                .as_ref()
                .and_then(|wifi| wifi.signal_quality_percent),
            Some(72)
        );
        assert_eq!(adapter.gateways, vec!["192.168.1.1"]);
    }

    #[test]
    fn ping_latency_parser_handles_standard_windows_tokens() {
        let output = "Reply from 1.1.1.1: bytes=32 time=14ms TTL=57\r\nReply from 1.1.1.1: bytes=32 time<1ms TTL=57";

        let latencies = parse_ping_latencies_ms(output);

        assert_eq!(latencies, vec![14.0, 0.5]);
    }

    #[test]
    fn network_speed_sample_parser_reads_result_row() {
        let output = "noise\r\nBENCHSCOPE_SPEED\tdownload\t25000000\t2000.0\r\n";

        let sample = parse_network_speed_sample_output(output).unwrap();

        assert_eq!(sample.direction, NetworkSpeedDirection::Download);
        assert_eq!(sample.bytes, 25_000_000);
        assert_eq!(sample.elapsed_ms, 2000.0);
        assert!((sample.mbps - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn network_speed_summary_uses_best_sample_per_direction() {
        let samples = vec![
            NetworkSpeedSample {
                direction: NetworkSpeedDirection::Download,
                bytes: 5_000_000,
                elapsed_ms: 1000.0,
                mbps: 40.0,
            },
            NetworkSpeedSample {
                direction: NetworkSpeedDirection::Download,
                bytes: 25_000_000,
                elapsed_ms: 2000.0,
                mbps: 100.0,
            },
            NetworkSpeedSample {
                direction: NetworkSpeedDirection::Upload,
                bytes: 10_000_000,
                elapsed_ms: 4000.0,
                mbps: 20.0,
            },
        ];

        assert_eq!(
            summarize_network_speed(&samples, NetworkSpeedDirection::Download),
            Some(100.0)
        );
        assert_eq!(
            summarize_network_speed(&samples, NetworkSpeedDirection::Upload),
            Some(20.0)
        );
    }

    fn test_network_adapter_snapshot(
        id: &str,
        name: &str,
        status: NetworkHealthStatus,
    ) -> NetworkAdapterSnapshot {
        NetworkAdapterSnapshot {
            id: id.to_owned(),
            name: name.to_owned(),
            description: format!("{name} adapter"),
            kind: NetworkAdapterKind::Ethernet,
            status,
            connected: true,
            is_physical: true,
            link_speed_bps: Some(1_000_000_000),
            mac_address: None,
            ipv4_addresses: Vec::new(),
            ipv6_addresses: Vec::new(),
            gateways: Vec::new(),
            dns_servers: Vec::new(),
            driver: None,
            wifi: None,
            counters: None,
            provider_notes: Vec::new(),
        }
    }

    #[test]
    fn network_adapter_snapshot_update_preserves_other_adapters() {
        let mut adapters = vec![
            test_network_adapter_snapshot("wifi", "Wi-Fi", NetworkHealthStatus::Good),
            test_network_adapter_snapshot("ethernet", "Ethernet", NetworkHealthStatus::Good),
        ];
        let mut updated =
            test_network_adapter_snapshot("ethernet", "Ethernet", NetworkHealthStatus::Critical);
        updated.link_speed_bps = Some(100_000_000);

        let selected_index = upsert_network_adapter_snapshot(&mut adapters, updated);

        assert_eq!(selected_index, 1);
        assert_eq!(adapters.len(), 2);
        assert_eq!(adapters[0].id, "wifi");
        assert_eq!(adapters[0].status, NetworkHealthStatus::Good);
        assert_eq!(adapters[1].id, "ethernet");
        assert_eq!(adapters[1].status, NetworkHealthStatus::Critical);
        assert_eq!(adapters[1].link_speed_bps, Some(100_000_000));
    }

    #[test]
    fn network_diagnosis_flags_dns_specific_failure() {
        let snapshot = NetworkAdapterSnapshot {
            id: "1".to_owned(),
            name: "Ethernet".to_owned(),
            description: "Intel Ethernet".to_owned(),
            kind: NetworkAdapterKind::Ethernet,
            status: NetworkHealthStatus::Good,
            connected: true,
            is_physical: true,
            link_speed_bps: Some(1_000_000_000),
            mac_address: None,
            ipv4_addresses: vec!["192.168.1.10".to_owned()],
            ipv6_addresses: Vec::new(),
            gateways: vec!["192.168.1.1".to_owned()],
            dns_servers: vec!["1.1.1.1".to_owned()],
            driver: None,
            wifi: None,
            counters: None,
            provider_notes: Vec::new(),
        };
        let probes = vec![
            network_probe_from_latencies(
                "Gateway",
                "192.168.1.1",
                NetworkProbeKind::Icmp,
                10,
                &[1.0, 1.2, 1.1],
                Vec::new(),
            ),
            network_probe_from_latencies(
                "Public IP",
                "1.1.1.1",
                NetworkProbeKind::Icmp,
                10,
                &[10.0, 11.0, 10.5],
                Vec::new(),
            ),
            NetworkProbeResult {
                target_label: "Hostname DNS".to_owned(),
                target: "example.com".to_owned(),
                probe_kind: NetworkProbeKind::DnsLookup,
                sent: 1,
                received: 0,
                loss_percent: 100.0,
                min_latency_ms: None,
                avg_latency_ms: None,
                max_latency_ms: None,
                jitter_ms: None,
                notes: Vec::new(),
            },
        ];

        let (status, findings) = evaluate_network_diagnosis(&snapshot, &probes);

        assert_eq!(status, NetworkHealthStatus::Critical);
        assert!(findings.iter().any(|finding| finding.title.contains("DNS")));
    }

    #[test]
    fn network_quick_probe_honors_pre_canceled_flag() {
        let cancel = AtomicBool::new(true);
        let (tx, _rx) = mpsc::channel();

        let err = run_icmp_probe_cancelable(
            "Gateway",
            "127.0.0.1",
            1,
            50,
            &cancel,
            &tx,
            Instant::now(),
            0,
            1,
        )
        .unwrap_err();

        assert!(err.to_string().contains("canceled"));
    }

    #[test]
    fn gpu_memory_buffer_size_parser_accepts_common_sizes() {
        assert_eq!(
            GpuMemoryBufferSize::parse("auto"),
            Some(GpuMemoryBufferSize::Auto)
        );
        assert_eq!(
            GpuMemoryBufferSize::parse("64MiB"),
            Some(GpuMemoryBufferSize::Mib64)
        );
        assert_eq!(
            GpuMemoryBufferSize::parse("256 mb"),
            Some(GpuMemoryBufferSize::Mib256)
        );
        assert_eq!(
            GpuMemoryBufferSize::parse("1g"),
            Some(GpuMemoryBufferSize::Gib1)
        );
        assert_eq!(GpuMemoryBufferSize::parse("bad"), None);
    }

    #[test]
    fn gpu_memory_internal_byte_accounting_counts_two_reads_and_one_write() {
        assert_eq!(
            GpuMemoryTestKind::InternalReadWrite.bytes_per_iteration(1024),
            3 * 1024
        );
        assert_eq!(
            GpuMemoryTestKind::DeviceCopy.bytes_per_iteration(1024),
            1024
        );
        assert_eq!(
            GpuMemoryTestKind::RoundTrip.bytes_per_iteration(1024),
            2048
        );
    }

    #[test]
    fn gpu_memory_bandwidth_uses_decimal_gbps() {
        assert_eq!(gpu_memory_bandwidth_gbps(1_000_000_000, 1000.0), 1.0);
        assert_eq!(format_gpu_memory_bandwidth(123.4), "123");
        assert_eq!(format_gpu_memory_bandwidth(12.34), "12.3");
    }

    #[test]
    fn gpu_memory_validation_catches_sample_mismatch() {
        let mut sample = make_gpu_memory_pattern_bytes(64, GPU_MEMORY_PATTERN_A_SEED).unwrap();
        assert!(validate_gpu_memory_pattern_sample(&sample, GPU_MEMORY_PATTERN_A_SEED)
            .starts_with("Passed"));

        sample[4] ^= 0xFF;
        assert!(validate_gpu_memory_pattern_sample(&sample, GPU_MEMORY_PATTERN_A_SEED)
            .starts_with("Failed"));
    }

    #[test]
    fn main_menu_categories_cover_all_tools() {
        let tools = main_menu_tool_items();

        assert_eq!(main_menu_category_items().len(), MenuCategory::ALL.len());
        for tool in tools {
            assert!(
                !tool.categories.is_empty(),
                "{} should appear in at least one category",
                tool.title
            );
        }
        for category in MenuCategory::ALL {
            assert!(
                !main_menu_items_for_category(category).is_empty(),
                "{category:?} should expose at least one tool"
            );
        }
    }

    #[test]
    fn matrix_tools_are_shared_between_cpu_and_gpu_menus() {
        for category in [MenuCategory::Cpu, MenuCategory::Gpu] {
            let items = main_menu_items_for_category(category);

            assert!(
                items
                    .iter()
                    .any(|item| item.view == AppView::MatrixBenchmark)
            );
            assert!(
                items
                    .iter()
                    .any(|item| item.view == AppView::MatrixStressTest)
            );
        }
    }

    #[test]
    fn main_menu_category_membership_matches_current_tools() {
        assert_eq!(
            main_menu_views_for_category(MenuCategory::Cpu),
            vec![
                AppView::MatrixBenchmark,
                AppView::MatrixStressTest,
                AppView::DeviceInfo,
            ]
        );
        assert_eq!(
            main_menu_views_for_category(MenuCategory::Gpu),
            vec![
                AppView::MatrixBenchmark,
                AppView::MatrixStressTest,
                AppView::GpuMemoryBenchmark,
                AppView::AiTrainingBenchmark,
                AppView::DeviceInfo,
            ]
        );
        assert_eq!(
            main_menu_views_for_category(MenuCategory::Ram),
            vec![AppView::RamTester, AppView::DeviceInfo]
        );
        assert_eq!(
            main_menu_views_for_category(MenuCategory::Storage),
            vec![
                AppView::DriveBenchmark,
                AppView::StorageHealth,
                AppView::DeviceInfo,
            ]
        );
        assert_eq!(
            main_menu_views_for_category(MenuCategory::Drivers),
            vec![
                AppView::NetworkDiagnostic,
                AppView::DeviceInfo,
                AppView::HistoryReports,
            ]
        );
        assert_eq!(
            main_menu_views_for_category(MenuCategory::Io),
            vec![
                AppView::DriveBenchmark,
                AppView::StorageHealth,
                AppView::NetworkDiagnostic,
                AppView::DeviceInfo,
                AppView::HistoryReports,
            ]
        );
        assert_eq!(
            main_menu_views_for_category(MenuCategory::Misc),
            vec![
                AppView::BatteryHealthDiagnostic,
                AppView::DeviceInfo,
                AppView::HistoryReports,
            ]
        );
    }

    fn make_test_timeline(scope: TimelineScope, samples: Vec<TimelineSample>) -> RunTimeline {
        RunTimeline {
            run_id: "test-timeline".to_owned(),
            title: "Test timeline".to_owned(),
            scope,
            started_at: SystemTime::UNIX_EPOCH,
            started_instant: Instant::now(),
            last_sample_at: None,
            samples,
            max_samples: TIMELINE_MAX_SAMPLES,
        }
    }

    fn timeline_test_sample(second: u64, gpu_temp_c: f32, throughput: f64) -> TimelineSample {
        TimelineSample {
            elapsed_ms: second * 1_000,
            sensor: TimelineSensorSample {
                gpu_temp_c: Some(gpu_temp_c),
                gpu_util_percent: Some(98.0),
                gpu_clock_mhz: Some(2_100.0),
                ..TimelineSensorSample::default()
            },
            throughput: Some(timeline_throughput(
                "Compute throughput",
                throughput,
                "TFLOP/s",
            )),
            phase: format!("sample {second}"),
        }
    }

    #[test]
    fn timeline_analysis_detects_heat_correlated_drop() {
        let samples = (0..12)
            .map(|index| {
                let temp = 58.0 + index as f32 * 2.0;
                let throughput = if index < 5 { 10.0 } else { 6.8 };
                timeline_test_sample(index, temp, throughput)
            })
            .collect();
        let timeline = make_test_timeline(TimelineScope::MatrixStress, samples);

        let summary = analyze_timeline(&timeline);

        assert!(summary.throughput_drop_percent.unwrap_or_default() >= 30.0);
        assert_eq!(summary.confidence, "High");
        assert!(summary.findings.iter().any(|finding| {
            finding.message.contains("Throughput dropped")
                || finding.message.contains("Temperature rose")
        }));
    }

    #[test]
    fn timeline_analysis_marks_performance_drop_low_without_heat() {
        let samples = (0..12)
            .map(|index| {
                let throughput = if index < 5 { 10.0 } else { 8.5 };
                timeline_test_sample(index, 58.0, throughput)
            })
            .collect();
        let timeline = make_test_timeline(TimelineScope::MatrixStress, samples);

        let summary = analyze_timeline(&timeline);

        assert!(summary.throughput_drop_percent.unwrap_or_default() >= 10.0);
        assert_eq!(summary.confidence, "Low");
        assert!(!summary
            .findings
            .iter()
            .any(|finding| finding.message.contains("Temperature rose")));
    }

    #[test]
    fn timeline_downsampling_respects_sample_limit() {
        let mut timeline = make_test_timeline(
            TimelineScope::MatrixStress,
            (0..25)
                .map(|index| timeline_test_sample(index, 60.0 + index as f32, 10.0))
                .collect(),
        );
        timeline.max_samples = 12;

        downsample_timeline_samples(&mut timeline);

        assert!(timeline.samples.len() <= timeline.max_samples);
        assert_eq!(timeline.samples.first().unwrap().elapsed_ms, 0);
        assert_eq!(timeline.samples.last().unwrap().elapsed_ms, 24_000);
    }

    #[test]
    fn timeline_graph_excludes_ssd_temperature_and_throughput() {
        let mut samples = Vec::new();
        for second in 0..3 {
            samples.push(TimelineSample {
                elapsed_ms: second * 1_000,
                sensor: TimelineSensorSample {
                    cpu_temp_c: Some(50.0 + second as f32),
                    gpu_temp_c: Some(60.0 + second as f32),
                    gpu_memory_temp_c: Some(70.0 + second as f32),
                    drive_temp_c: Some(40.0 + second as f32),
                    drive_util_percent: Some(20.0 + second as f32),
                    ..TimelineSensorSample::default()
                },
                throughput: Some(timeline_throughput("Compute throughput", 10.0, "TFLOP/s")),
                phase: "test".to_owned(),
            });
        }
        let timeline = make_test_timeline(TimelineScope::MatrixStress, samples);
        let state = TimelineState::new();
        let labels = timeline_graph_series(&timeline, &state)
            .into_iter()
            .map(|series| series.label)
            .collect::<Vec<_>>();

        assert!(labels.contains(&"CPU temp".to_owned()));
        assert!(labels.contains(&"SSD util".to_owned()));
        assert!(!labels.contains(&"SSD temp".to_owned()));
        assert!(!labels.contains(&"Throughput".to_owned()));
    }

    #[test]
    fn timeline_legend_wraps_inside_chart_bounds() {
        let items = timeline_legend_layout(&[90.0, 95.0, 85.0, 80.0], 12.0, 190.0, 8.0);

        assert!(items.iter().all(|item| item.x >= 12.0));
        assert!(items.iter().all(|item| item.x + item.width <= 190.1));
        assert!(items
            .iter()
            .any(|item| item.y >= 8.0 + TIMELINE_LEGEND_ROW_HEIGHT));
    }

    #[test]
    fn timeline_chart_left_margin_keeps_axis_label_clear() {
        assert!(timeline_chart_left_margin(520.0) >= 88.0);
        assert!(timeline_chart_left_margin(360.0) >= 78.0);
        assert!(timeline_chart_left_margin(260.0) >= 68.0);
    }

    #[test]
    fn timeline_legend_uses_compact_labels_on_small_charts() {
        let series = TimelineGraphSeries {
            label: "RAM util".to_owned(),
            color: egui::Color32::WHITE,
            unit: "%".to_owned(),
            values: Vec::new(),
        };

        assert_eq!(timeline_legend_label(&series, true), "RAM %");
        assert_eq!(timeline_legend_label(&series, false), "RAM util (%)");
    }
}
