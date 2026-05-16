impl BenchScopeApp {
    fn ui_main_menu(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                self.ui_fullscreen_button(ui);
            });
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.heading(egui::RichText::new("BenchScope").size(34.0));
                ui.add_space(24.0);
                if ui
                    .add_sized(
                        [360.0, 58.0],
                        egui::Button::new(
                            egui::RichText::new("Matrix CPU/GPU Benchmark").size(19.0),
                        ),
                    )
                    .clicked()
                {
                    self.view = AppView::MatrixBenchmark;
                }
                ui.add_space(10.0);
                if ui
                    .add_sized(
                        [360.0, 58.0],
                        egui::Button::new(egui::RichText::new("Matrix Stress Test").size(19.0)),
                    )
                    .clicked()
                {
                    self.view = AppView::MatrixStressTest;
                }
                ui.add_space(10.0);
                if ui
                    .add_sized(
                        [360.0, 58.0],
                        egui::Button::new(egui::RichText::new("Drive Benchmark").size(19.0)),
                    )
                    .clicked()
                {
                    self.view = AppView::DriveBenchmark;
                }
                ui.add_space(10.0);
                if ui
                    .add_sized(
                        [360.0, 58.0],
                        egui::Button::new(
                            egui::RichText::new("SSD / HDD Health Checker").size(19.0),
                        ),
                    )
                    .clicked()
                {
                    self.view = AppView::StorageHealth;
                    if self.storage_health.snapshot.is_none() && !self.storage_health.running {
                        self.storage_health.start_refresh();
                    }
                }
                ui.add_space(10.0);
                if ui
                    .add_sized(
                        [360.0, 58.0],
                        egui::Button::new(egui::RichText::new("RAM Tester").size(19.0)),
                    )
                    .clicked()
                {
                    self.view = AppView::RamTester;
                }
                ui.add_space(10.0);
                if ui
                    .add_sized(
                        [360.0, 58.0],
                        egui::Button::new(
                            egui::RichText::new("Battery Health Diagnostic").size(19.0),
                        ),
                    )
                    .clicked()
                {
                    self.view = AppView::BatteryHealthDiagnostic;
                    if self.battery.latest_report.is_none() && !self.battery.scanning {
                        self.battery.start_scan();
                    }
                }
                ui.add_space(10.0);
                if ui
                    .add_sized(
                        [360.0, 58.0],
                        egui::Button::new(
                            egui::RichText::new("Network Hardware Diagnostic").size(19.0),
                        ),
                    )
                    .clicked()
                {
                    self.view = AppView::NetworkDiagnostic;
                }
                ui.add_space(22.0);
                ui.small(format!("CPU: {}", self.cpu_info.label()));
                ui.small(format!("GPU adapters detected: {}", self.adapters.len()));
            });
        });
    }
}
