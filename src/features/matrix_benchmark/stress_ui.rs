impl BenchScopeApp {
    fn ui_matrix_stress_test(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        egui::Panel::top("stress_top_panel").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui_large_back_button(ui).clicked() {
                    self.request_stress_back_to_menu();
                }
                self.ui_fullscreen_button(ui);
                ui.separator();
                ui.heading("Matrix Stress Test");
                ui.separator();
                ui.label(&self.status);
                if !self.eta_text.is_empty() {
                    ui.separator();
                    ui.label(&self.eta_text);
                }
            });
            let progress_bar = if self.repeat_running && self.repeat_duration.is_infinite() {
                egui::ProgressBar::new(0.0)
                    .animate(true)
                    .text("Stress test running until canceled")
            } else {
                egui::ProgressBar::new(self.progress)
                    .show_percentage()
                    .text("Stress test elapsed")
            };
            ui.add(progress_bar);
        });

        egui::Panel::left("stress_controls")
            .resizable(false)
            .min_size(330.0)
            .show_inside(ui, |ui| {
                let settings_height = (ui.available_height() - CONTROLS_ACTION_HEIGHT).max(120.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), settings_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("stress_controls_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.heading("Stress Controls");
                                ui.add_space(8.0);

                ui.small(format!("CPU: {}", self.cpu_info.label()));
                ui.add_space(4.0);

                ui.label("GPU adapter");
                egui::ComboBox::from_id_salt("stress_adapter_combo")
                    .selected_text(
                        self.adapters
                            .get(self.selected_adapter)
                            .map(AdapterInfo::label)
                            .unwrap_or_else(|| "No adapters found".to_owned()),
                    )
                    .show_ui(ui, |ui| {
                        for (index, adapter) in self.adapters.iter().enumerate() {
                            ui.selectable_value(&mut self.selected_adapter, index, adapter.label());
                        }
                    });

                if let Some(adapter) = self.adapters.get(self.selected_adapter) {
                    ui.small(format!(
                        "Vendor {:04X}, device {:04X}, driver {}, timestamp queries {}",
                        adapter.vendor,
                        adapter.device,
                        empty_to_unknown(&adapter.driver),
                        if adapter.timestamp_query {
                            "supported"
                        } else {
                            "unavailable"
                        }
                    ));
                    if let Some((limit, label)) = adapter_memory_limit_bytes(adapter) {
                        ui.small(format!("Memory limit estimate: {} ({label})", format_bytes(limit)));
                    } else {
                        ui.small("Memory limit estimate: unavailable for this adapter/backend");
                    }
                }

                ui.add_space(6.0);
                ui.label("GPU intensity");
                ui.add_enabled_ui(!self.running, |ui| {
                    ui.horizontal(|ui| {
                        for intensity in GpuIntensity::ALL {
                            ui.selectable_value(&mut self.gpu_intensity, intensity, intensity.label());
                        }
                    });
                });
                ui.small(self.gpu_intensity.description());
                if self.gpu_intensity == GpuIntensity::High {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "High mode can stress the driver, PSU, and thermals during large matrices.",
                    );
                }

                if ui.button("Refresh GPUs").clicked() && !self.running {
                    self.adapters = enumerate_adapters();
                    self.selected_adapter = 0;
                    self.status = format!("Found {} adapter(s)", self.adapters.len());
                    self.log(self.status.clone());
                }

                ui.separator();
                ui.label("Matrix size");
                egui::ComboBox::from_id_salt("stress_size_combo")
                    .selected_text(self.size_text.clone())
                    .show_ui(ui, |ui| {
                        for size in DEFAULT_SIZES {
                            ui.selectable_value(&mut self.size_text, size.to_string(), size.to_string());
                        }
                    });
                ui.text_edit_singleline(&mut self.size_text);

                if let Ok(size) = self.selected_size() {
                    if let (Some(matrix_bytes), Some(gpu_bytes)) =
                        (matrix_buffers_bytes(size, 3), gpu_working_set_bytes(size))
                    {
                        ui.small(format!(
                            "A/B/C: {}; GPU run estimate: {}",
                            format_bytes(matrix_bytes),
                            format_bytes(gpu_bytes)
                        ));

                        if let Some(adapter) = self.adapters.get(self.selected_adapter) {
                            if let Some((limit, label)) = adapter_memory_limit_bytes(adapter) {
                                if gpu_bytes > limit {
                                    ui.colored_label(
                                        egui::Color32::RED,
                                        format!(
                                            "Estimated GPU memory exceeds {label}: {} > {}.",
                                            format_bytes(gpu_bytes),
                                            format_bytes(limit)
                                        ),
                                    );
                                }
                            }
                        }
                    }
                    if size >= 8192 {
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            "Large stress tests can heavily load the selected processor for the full duration.",
                        );
                    }
                }

                ui.separator();
                ui.label("Processor");
                ui.add_enabled_ui(!self.running, |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.repeat_mode, RepeatMode::Gpu, "GPU");
                        ui.selectable_value(&mut self.repeat_mode, RepeatMode::Cpu, "CPU");
                    });
                });
                ui.label("Duration");
                ui.add_enabled_ui(!self.running, |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut self.repeat_duration,
                            RepeatDuration::OneMinute,
                            "1 min",
                        );
                        ui.selectable_value(
                            &mut self.repeat_duration,
                            RepeatDuration::FiveMinutes,
                            "5 min",
                        );
                        ui.selectable_value(
                            &mut self.repeat_duration,
                            RepeatDuration::Infinite,
                            "Infinite",
                        );
                    });
                });
                            });
                    },
                );
                ui.separator();

                ui.add_enabled_ui(!self.running && !self.adapters.is_empty(), |ui| {
                    if ui_start_action_button(ui, "Start stress test").clicked() {
                        self.start_repeat();
                    }
                });
                ui.add_enabled_ui(self.repeat_running, |ui| {
                    if ui_cancel_action_button(ui, "Cancel stress test").clicked() {
                        self.cancel_repeat();
                    }
                });
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            let available_height = ui.available_height();
            let (summary_height, log_height) =
                panel_content_log_heights(available_height, 0.35, 220.0);

            ui.heading("Stress Test");
            ui.add_space(6.0);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), summary_height),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.label(format!("Mode: {}", self.repeat_mode));
                    ui.label(format!("Duration: {}", self.repeat_duration));
                    if self.repeat_duration.is_infinite() {
                        ui.label("Progress: runs until canceled");
                    } else {
                        ui.label(format!("Progress: {:.0}%", self.progress * 100.0));
                    }
                    if !self.eta_text.is_empty() {
                        ui.label(&self.eta_text);
                    }
                    ui.label(&self.status);
                },
            );

            ui.separator();
            ui.heading("Log");
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .max_height(log_height)
                .show(ui, |ui| {
                    for line in &self.log {
                        ui_log_line(ui, line);
                    }
                });
        });

        self.ui_sensor_window(&ctx);

        if let Some(warning) = self.pending_vram_warning.clone() {
            egui::Window::new("VRAM limit exceeded")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label(format!(
                        "{}x{} is estimated to need {} of GPU memory.",
                        warning.size,
                        warning.size,
                        format_bytes(warning.estimated_gpu_bytes)
                    ));
                    ui.label(format!(
                        "The selected adapter's {} is {}.",
                        warning.limit_label,
                        format_bytes(warning.limit_bytes)
                    ));
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "Running anyway may fail, trigger driver paging, or make the result misleading.",
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.pending_vram_warning = None;
                            self.status = "Run canceled before exceeding the VRAM estimate".to_owned();
                            self.log("Canceled run after VRAM warning");
                        }
                        if ui.button("Run anyway").clicked() {
                            self.continue_pending_vram_warning();
                        }
                    });
                });
        }

        if self.stress_back_confirm {
            egui::Window::new("Return to main menu?")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label("A matrix stress test is currently running.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Stay").clicked() {
                            self.stress_back_confirm = false;
                        }
                        if ui.button("Cancel and return").clicked() {
                            self.cancel_repeat();
                            self.stress_back_confirm = false;
                            self.view = AppView::MainMenu;
                        }
                    });
                });
        }
    }
}
