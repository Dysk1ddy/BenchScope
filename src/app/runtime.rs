impl BenchScopeApp {
    fn ui_app(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_global_shortcuts(&ctx);
        self.sync_sensor_state();
        let matrix_result_count_before = self.results.len();
        let ram_result_count_before = self.ram.results.len();
        let ai_training_result_count_before = self.ai_training.results.len();
        let drive_was_running = self.drive.running;
        let drive_result_count_before = self.drive.results.len();
        let storage_health_was_running = self.storage_health.running;
        let storage_health_snapshot_before = self.storage_health.snapshot.is_some();
        let storage_health_scan_before = self.storage_health.scan_result.is_some();
        let storage_health_benchmark_count_before = self.storage_health.benchmark_results.len();
        let battery_was_scanning = self.battery.scanning;
        let network_was_active =
            self.network.running || self.network.monitoring || self.network.adapter_refresh_running;
        let device_info_was_running = self.device_info.running;
        let gpu_memory_was_running = self.gpu_memory.running;
        let gpu_memory_result_count_before = self.gpu_memory.results.len();
        self.poll_worker_events();
        self.gpu_memory.poll_worker_events();
        self.drive.poll_worker_events();
        self.storage_health.poll_worker_events();
        self.ram.poll_worker_events();
        self.battery.poll_worker_events();
        self.network.poll_worker_events();
        self.device_info.poll_worker_events();
        self.ai_training.poll_worker_events();
        self.sync_pytorch_cuda_from_ai_training();
        self.observe_temperature_run();
        self.observe_timeline_run(false);
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
        if gpu_memory_was_running && !self.gpu_memory.running {
            if let Some(report) = self.finish_and_log_temperature_run() {
                for result in self
                    .gpu_memory
                    .results
                    .iter_mut()
                    .skip(gpu_memory_result_count_before)
                {
                    result.gpu_temperature = report.gpu;
                }
            }
        }
        self.capture_history_after_poll(
            matrix_result_count_before,
            drive_result_count_before,
            gpu_memory_result_count_before,
            ram_result_count_before,
            ai_training_result_count_before,
            storage_health_was_running,
            storage_health_snapshot_before,
            storage_health_scan_before,
            storage_health_benchmark_count_before,
            battery_was_scanning,
            network_was_active,
            device_info_was_running,
        );
        self.sync_sensor_state();
        if self.running
            || self.gpu_memory.running
            || self.drive.running
            || self.storage_health.running
            || self.ram.running
            || self.battery.scanning
            || self.battery.live_running
            || self.network.running
            || self.network.adapter_refresh_running
            || self.network.monitoring
            || self.device_info.running
            || self.ai_training.running
            || self.pytorch_probe_running
            || self.pytorch_install_running
            || self.ai_training.pytorch_probe_running
            || self.ai_training.pytorch_install_running
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else if self.view != AppView::MainMenu {
            ctx.request_repaint_after(Duration::from_millis(SENSOR_POLL_MS));
        }
        match self.view {
            AppView::MainMenu => self.ui_main_menu(ui),
            AppView::DriveBenchmark => self.ui_drive_benchmark(ui),
            AppView::StorageHealth => self.ui_storage_health(ui),
            AppView::RamTester => self.ui_ram_tester(ui),
            AppView::BatteryHealthDiagnostic => self.ui_battery_health_diagnostic(ui),
            AppView::NetworkDiagnostic => self.ui_network_diagnostic(ui),
            AppView::DeviceInfo => self.ui_device_info(ui),
            AppView::HistoryReports => self.ui_history_reports(ui),
            AppView::AiTrainingBenchmark => self.ui_ai_training_benchmark(ui),
            AppView::GpuMemoryBenchmark => self.ui_gpu_memory_benchmark(ui),
            AppView::MatrixStressTest => self.ui_matrix_stress_test(ui),
            AppView::MatrixBenchmark => self.ui_matrix_benchmark(ui, &ctx),
        }
    }

    fn sync_pytorch_cuda_from_ai_training(&mut self) {
        let Some(environment) = self.ai_training.pytorch_probe.as_ref() else {
            return;
        };
        if !environment.cuda_available {
            return;
        }
        let already_synced = self
            .pytorch_probe
            .as_ref()
            .is_some_and(|probe| {
                probe.cuda_available && probe.python_executable == environment.python_executable
            });
        if already_synced {
            return;
        }

        self.pytorch_python = environment.python_executable.clone();
        self.pytorch_probe = Some(environment.clone());
    }
}

impl eframe::App for BenchScopeRoot {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.poll_startup();
        if let Some(app) = &mut self.app {
            app.ui_app(ui, frame);
        } else {
            self.ui_startup(ui);
        }
    }
}
