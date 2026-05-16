impl BenchScopeApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_ui_style(&cc.egui_ctx);
        let (tx, rx) = mpsc::channel();
        let adapters = enumerate_adapters();
        let cpu_info = detect_cpu_info();
        let selected_adapter = adapters
            .iter()
            .position(|adapter| adapter.device_type != wgpu::DeviceType::Cpu)
            .unwrap_or(0);
        let drive = DriveBenchmarkState::new();
        let storage_health = StorageHealthState::new();
        let ram = RamTestState::new();
        let battery = BatteryDiagnosticState::new();
        let network = NetworkDiagnosticState::new();
        let sensors = SensorManager::new(drive_letter_for_path(&PathBuf::from(
            drive.target_folder_text.trim(),
        )));
        let mut app = Self {
            view: AppView::MainMenu,
            adapters,
            cpu_info,
            selected_adapter,
            size_text: DEFAULT_SIZES[6].to_string(),
            gpu_intensity: GpuIntensity::Safe,
            validate_output: true,
            estimate_cpu_time: false,
            repeat_mode: RepeatMode::Gpu,
            repeat_duration: RepeatDuration::OneMinute,
            results: Vec::new(),
            log: Vec::new(),
            status: "Ready".to_owned(),
            progress: 0.0,
            cpu_progress: 0.0,
            gpu_progress: 0.0,
            eta_text: String::new(),
            rx,
            tx,
            cancel: None,
            running: false,
            repeat_running: false,
            pending_vram_warning: None,
            matrix_back_confirm: false,
            stress_back_confirm: false,
            drive_back_confirm: false,
            ram_back_confirm: false,
            battery_back_confirm: false,
            network_back_confirm: false,
            drive,
            storage_health_back_confirm: false,
            storage_health,
            ram,
            battery,
            network,
            sensors,
            temperature_run: None,
            fullscreen: false,
            sensor_permission_prompt: sensor_permission_prompt_needed(),
        };
        app.log("Application started");
        if app.adapters.is_empty() {
            app.status = "No wgpu adapters found".to_owned();
            app.log("No wgpu adapters found");
        } else {
            app.log(format!("CPU: {}", app.cpu_info.label()));
            app.log(format!("Found {} adapter(s)", app.adapters.len()));
            for adapter in app.adapters.clone() {
                app.log(format!(
                    "{} | vendor {:04X} device {:04X} | driver {} | timestamp {}",
                    adapter.label(),
                    adapter.vendor,
                    adapter.device,
                    empty_to_unknown(&adapter.driver),
                    if adapter.timestamp_query { "yes" } else { "no" }
                ));
                if let Some((limit, label)) = adapter_memory_limit_bytes(&adapter) {
                    app.log(format!(
                        "  memory limit estimate: {} ({label})",
                        format_bytes(limit)
                    ));
                }
            }
        }
        app
    }

    fn log(&mut self, message: impl Into<String>) {
        self.log.push(message.into());
    }

    fn begin_temperature_run(&mut self, scope: TemperatureScope) {
        let snapshot = self.sensors.latest();
        self.temperature_run = Some(TemperatureRunTracker::start(scope, &snapshot));
    }

    fn observe_temperature_run(&mut self) {
        let snapshot = self.sensors.latest();
        if let Some(run) = &mut self.temperature_run {
            run.observe(&snapshot);
        }
    }

    fn finish_temperature_run(&mut self) -> Option<TemperatureRunReport> {
        let snapshot = self.sensors.latest();
        self.temperature_run.take().map(|run| run.finish(&snapshot))
    }

    fn finish_and_log_temperature_run(&mut self) -> Option<TemperatureRunReport> {
        let report = self.finish_temperature_run()?;
        let summary = format_temperature_run_report(&report);
        self.log(format!("Temperature: {summary}"));
        Some(report)
    }

    fn sync_sensor_state(&mut self) {
        let target_drive_letter = if self.view == AppView::StorageHealth {
            self.storage_health
                .selected_drive_root()
                .and_then(|root| drive_letter_for_path(&root))
        } else {
            drive_letter_for_path(&PathBuf::from(self.drive.target_folder_text.trim()))
        };
        self.sensors.set_target_drive_letter(target_drive_letter);
        let gpu_uses_shared_cpu_temperature = self
            .adapters
            .get(self.selected_adapter)
            .is_some_and(adapter_uses_shared_cpu_temperature);
        self.sensors
            .set_target_gpu_uses_shared_cpu_temperature(gpu_uses_shared_cpu_temperature);
    }

    fn ui_sensor_panel(&mut self, ui: &mut egui::Ui) {
        let snapshot = self.sensors.latest();
        let rows: Vec<(&str, Option<&SensorReading>)> = match self.view {
            AppView::MatrixBenchmark | AppView::MatrixStressTest => {
                vec![
                    ("CPU", snapshot.cpu.as_ref()),
                    ("GPU", snapshot.gpu.as_ref()),
                ]
            }
            AppView::DriveBenchmark | AppView::StorageHealth => {
                vec![("SSD", snapshot.drive.as_ref())]
            }
            AppView::RamTester => vec![
                ("CPU", snapshot.cpu.as_ref()),
                ("RAM", snapshot.memory.as_ref()),
            ],
            AppView::BatteryHealthDiagnostic | AppView::NetworkDiagnostic | AppView::MainMenu => {
                return;
            }
        };

        ui.with_layout(egui::Layout::right_to_left(egui::Align::BOTTOM), |ui| {
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_min_width(210.0);
                    ui.label(egui::RichText::new("Sensors").strong());
                    ui.horizontal(|ui| {
                        ui.set_min_width(188.0);
                        ui.label(egui::RichText::new("").monospace());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new("Util %").small());
                            ui.add_space(12.0);
                            ui.label(egui::RichText::new("Temp").small());
                        });
                    });
                    ui.add_space(3.0);
                    for (label, reading) in rows {
                        ui_sensor_row(ui, label, reading);
                    }
                });
        });
    }

    fn ui_sensor_permission_prompt(&mut self, ctx: &egui::Context) {
        if !self.sensor_permission_prompt {
            return;
        }

        egui::Window::new("Enable Extended Sensors?")
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.label("BenchScope can use safe Windows/NVIDIA probes without extra permission.");
                ui.add_space(6.0);
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "CPU, RAM, and some SSD temperatures may be unavailable without low-level driver access.",
                );
                ui.label("The optional LibreHardwareMonitor helper can create or load a WinRing driver. Microsoft Defender blocks that driver as VulnerableDriver:WinNT/Winring0 on this machine, so BenchScope will not launch the helper from the UI.");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Use safe sensors").clicked() {
                        self.sensor_permission_prompt = false;
                        self.log("Extended sensor helper skipped; using safe Windows/NVIDIA probes.");
                    }
                });
            });
    }
}
