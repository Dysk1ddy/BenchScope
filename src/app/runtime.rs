impl BenchScopeApp {
    fn ui_app(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_global_shortcuts(&ctx);
        self.sync_sensor_state();
        let drive_was_running = self.drive.running;
        let drive_result_count_before = self.drive.results.len();
        self.poll_worker_events();
        self.drive.poll_worker_events();
        self.storage_health.poll_worker_events();
        self.ram.poll_worker_events();
        self.battery.poll_worker_events();
        self.network.poll_worker_events();
        self.observe_temperature_run();
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
        self.sync_sensor_state();
        if self.running
            || self.drive.running
            || self.storage_health.running
            || self.ram.running
            || self.battery.scanning
            || self.battery.live_running
            || self.network.running
            || self.network.monitoring
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
            AppView::MatrixStressTest => self.ui_matrix_stress_test(ui),
            AppView::MatrixBenchmark => self.ui_matrix_benchmark(ui, &ctx),
        }
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
