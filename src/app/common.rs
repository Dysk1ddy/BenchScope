impl BenchScopeRoot {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_ui_style(&cc.egui_ctx);
        let (tx, rx) = mpsc::channel();
        let worker_tx = tx.clone();
        if let Err(err) = thread::Builder::new()
            .name("benchscope-startup".to_owned())
            .spawn(move || {
                let result =
                    panic::catch_unwind(AssertUnwindSafe(|| load_startup_data(&worker_tx)));
                match result {
                    Ok(data) => {
                        let _ = worker_tx.send(StartupEvent::Ready(Box::new(data)));
                    }
                    Err(panic) => {
                        let _ = worker_tx.send(StartupEvent::Failed(format!(
                            "Startup failed: {}. {}",
                            panic_message(&*panic),
                            crashlog_hint()
                        )));
                    }
                }
            })
        {
            let _ = tx.send(StartupEvent::Failed(format!(
                "Could not start initialization worker: {err}"
            )));
        }

        Self {
            startup_rx: rx,
            startup_progress: StartupProgress {
                step: "Starting BenchScope".to_owned(),
                progress: 0.02,
            },
            app: None,
            startup_error: None,
        }
    }

    fn poll_startup(&mut self) {
        while let Ok(event) = self.startup_rx.try_recv() {
            match event {
                StartupEvent::Progress(progress) => self.startup_progress = progress,
                StartupEvent::Ready(data) => {
                    self.startup_progress = StartupProgress {
                        step: "Ready".to_owned(),
                        progress: 1.0,
                    };
                    self.app = Some(BenchScopeApp::from_startup(*data));
                    self.startup_error = None;
                }
                StartupEvent::Failed(message) => {
                    self.startup_error = Some(message);
                }
            }
        }
    }

    fn ui_startup(&self, ui: &mut egui::Ui) {
        ui.ctx().request_repaint_after(Duration::from_millis(50));
        egui::CentralPanel::default().show_inside(ui, |ui| {
            let top_space = (ui.available_height() * 0.32).clamp(64.0, 220.0);
            ui.add_space(top_space);
            ui.vertical_centered(|ui| {
                ui.heading(egui::RichText::new("BenchScope").size(34.0));
                ui.add_space(22.0);
                if let Some(error) = &self.startup_error {
                    ui.colored_label(egui::Color32::RED, error);
                } else {
                    ui.add(
                        egui::ProgressBar::new(self.startup_progress.progress.clamp(0.0, 1.0))
                            .desired_width(460.0)
                            .show_percentage(),
                    );
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(&self.startup_progress.step).size(16.0));
                }
            });
        });
    }
}

fn load_startup_data(tx: &Sender<StartupEvent>) -> StartupData {
    startup_progress(tx, 0.10, "Detecting GPU adapters");
    let adapters = enumerate_adapters();
    startup_progress(tx, 0.36, "Reading CPU information");
    let cpu_info = detect_cpu_info();
    startup_progress(tx, 0.48, "Detecting drives");
    let drives = detect_drives();
    startup_progress(tx, 0.56, "Preparing drive benchmark");
    let drive = DriveBenchmarkState::with_drives(drives.clone());
    startup_progress(tx, 0.62, "Preparing storage health");
    let storage_health = StorageHealthState::with_drives(drives);
    startup_progress(tx, 0.74, "Reading RAM status");
    let ram = RamTestState::new();
    startup_progress(tx, 0.84, "Preparing battery diagnostic");
    let battery = BatteryDiagnosticState::new();
    startup_progress(tx, 0.92, "Preparing network diagnostic");
    let network = NetworkDiagnosticState::new();
    startup_progress(tx, 0.95, "Preparing device information viewer");
    let device_info = DeviceInfoState::new();
    startup_progress(tx, 0.97, "Preparing AI training benchmark");
    let ai_training = AiTrainingBenchmarkState::new();
    startup_progress(tx, 0.975, "Preparing GPU memory benchmark");
    let gpu_memory = GpuMemoryBenchmarkState::new();
    startup_progress(tx, 0.98, "Checking setup requirements");
    let setup_detection = detect_setup_environment(&adapters);
    startup_progress(tx, 0.99, "Starting safe sensor sampler");

    StartupData {
        adapters,
        cpu_info,
        setup_detection,
        drive,
        storage_health,
        ram,
        battery,
        network,
        device_info,
        ai_training,
        gpu_memory,
    }
}

fn startup_progress(tx: &Sender<StartupEvent>, progress: f32, step: &str) {
    let _ = tx.send(StartupEvent::Progress(StartupProgress {
        step: step.to_owned(),
        progress,
    }));
}

impl BenchScopeApp {
    fn from_startup(data: StartupData) -> Self {
        let StartupData {
            adapters,
            cpu_info,
            setup_detection,
            drive,
            storage_health,
            ram,
            battery,
            network,
            device_info,
            ai_training,
            gpu_memory,
        } = data;
        let (tx, rx) = mpsc::channel();
        let history = HistoryState::new();
        let timeline = TimelineState::new();
        let selected_adapter = adapters
            .iter()
            .position(|adapter| adapter.device_type != wgpu::DeviceType::Cpu)
            .unwrap_or(0);
        let sensors = SensorManager::new(drive_letter_for_path(&PathBuf::from(
            drive.target_folder_text.trim(),
        )));
        let mut app = Self {
            view: AppView::MainMenu,
            main_menu_category: None,
            main_menu_search_text: String::new(),
            adapters,
            cpu_info,
            setup_detection,
            setup_task_running: false,
            setup_task_progress: None,
            selected_adapter,
            size_text: DEFAULT_SIZES[6].to_string(),
            stress_size_text: DEFAULT_SIZES[0].to_string(),
            gpu_intensity: GpuIntensity::Safe,
            stress_gpu_backend: StressGpuBackend::AutoOptimized,
            validate_output: true,
            estimate_cpu_time: false,
            repeat_mode: RepeatMode::Gpu,
            repeat_duration: RepeatDuration::OneMinute,
            pytorch_python: default_pytorch_python_executable(),
            pytorch_probe: None,
            pytorch_probe_running: false,
            pytorch_install_running: false,
            pending_pytorch_install: false,
            results: Vec::new(),
            repeat_progress: None,
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
            ai_training_back_confirm: false,
            gpu_memory_back_confirm: false,
            device_info,
            ai_training,
            gpu_memory,
            drive,
            storage_health_back_confirm: false,
            storage_health,
            ram,
            battery,
            network,
            history,
            timeline,
            sensors,
            temperature_run: None,
            sensor_window_minimized: false,
            fullscreen: false,
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
        app.capture_app_environment_history();
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

    fn current_sensor_rows<'a>(
        &self,
        snapshot: &'a SensorSnapshot,
    ) -> Option<Vec<(&'static str, Option<&'a SensorReading>)>> {
        sensor_rows_for_view(self.view, snapshot)
    }

    fn ui_sensor_window(&mut self, ctx: &egui::Context) {
        let snapshot = self.sensors.latest();
        let Some(rows) = self.current_sensor_rows(&snapshot) else {
            return;
        };

        if self.sensor_window_minimized {
            egui::Area::new(egui::Id::new("sensor_metrics_minimized"))
                .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-14.0, -14.0))
                .show(ctx, |ui| {
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            if ui.button(sensor_minimized_label(&rows)).clicked() {
                                self.sensor_window_minimized = false;
                            }
                        });
                });
            return;
        }

        egui::Window::new("Sensors")
            .id(egui::Id::new("sensor_metrics_window"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-14.0, -14.0))
            .default_size(egui::vec2(
                SENSOR_WINDOW_DEFAULT_WIDTH,
                SENSOR_WINDOW_DEFAULT_HEIGHT,
            ))
            .min_width(SENSOR_WINDOW_MIN_WIDTH)
            .min_height(SENSOR_WINDOW_MIN_HEIGHT)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(sensor_minimized_label(&rows))
                            .strong()
                            .monospace(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Minimize").clicked() {
                            self.sensor_window_minimized = true;
                        }
                        if ui
                            .button("Reset Min/Max")
                            .on_hover_text("Reset sensor minimum and maximum readings to the current values")
                            .clicked()
                        {
                            self.sensors.reset_metric_ranges();
                            self.status = "Sensor min/max values reset".to_owned();
                            self.log("Sensor min/max values reset");
                        }
                    });
                });
                ui.separator();
                ui_sensor_table(ui, &rows);
            });
    }
}

fn sensor_rows_for_view<'a>(
    view: AppView,
    snapshot: &'a SensorSnapshot,
) -> Option<Vec<(&'static str, Option<&'a SensorReading>)>> {
    if matches!(view, AppView::MainMenu | AppView::HistoryReports) {
        return None;
    }

    Some(vec![
        ("CPU", snapshot.cpu.as_ref()),
        ("GPU", snapshot.gpu.as_ref()),
        ("VRAM", snapshot.gpu_memory.as_ref()),
        ("SSD", snapshot.drive.as_ref()),
        ("RAM", snapshot.memory.as_ref()),
    ])
}

fn sensor_minimized_label(rows: &[(&str, Option<&SensorReading>)]) -> String {
    let labels = rows
        .iter()
        .map(|(label, _)| *label)
        .collect::<Vec<_>>()
        .join("/");
    if labels.is_empty() {
        "Sensors".to_owned()
    } else {
        format!("Sensors: {labels}")
    }
}
