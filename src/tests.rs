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
        let now = Instant::now();
        let mut samples = VecDeque::new();
        samples.push_back(BatteryLiveSample {
            captured_at: now - Duration::from_secs(600),
            ac_connected: Some(false),
            status: "Discharging".to_owned(),
            percent: Some(80.0),
            remaining_capacity_mwh: Some(40_000.0),
            charge_rate_watts: None,
            discharge_rate_watts: None,
            windows_runtime_minutes: Some(70.0),
        });
        samples.push_back(BatteryLiveSample {
            captured_at: now,
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
            memory: None,
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
}
