impl BenchScopeApp {
    fn request_gpu_memory_back_to_menu(&mut self) {
        if self.gpu_memory.running {
            self.gpu_memory_back_confirm = true;
        } else {
            self.view = AppView::MainMenu;
        }
    }

    fn start_gpu_memory_benchmark(&mut self) {
        let adapter = match self.selected_adapter() {
            Ok(adapter) => adapter,
            Err(err) => {
                self.gpu_memory.status = err.to_string();
                self.gpu_memory.log(err.to_string());
                return;
            }
        };
        let was_running = self.gpu_memory.running;
        self.gpu_memory.start(adapter);
        if !was_running && self.gpu_memory.running {
            self.begin_temperature_run(TemperatureScope::Matrix);
        }
    }

    fn ui_gpu_memory_benchmark(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        egui::Panel::top("gpu_memory_top_panel").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui_large_back_button(ui).clicked() {
                    self.request_gpu_memory_back_to_menu();
                }
                self.ui_fullscreen_button(ui);
                ui.separator();
                ui.heading("GPU Memory Bandwidth");
                ui.separator();
                ui.label(&self.gpu_memory.status);
                if !self.gpu_memory.eta_text.is_empty() {
                    ui.separator();
                    ui.label(&self.gpu_memory.eta_text);
                }
            });
            ui.horizontal(|ui| {
                ui.label("Current");
                ui.add(
                    egui::ProgressBar::new(self.gpu_memory.current_progress)
                        .show_percentage()
                        .desired_width(260.0),
                );
                ui.label("Suite");
                ui.add(
                    egui::ProgressBar::new(self.gpu_memory.suite_progress)
                        .show_percentage()
                        .desired_width(260.0),
                );
            });
        });

        egui::Panel::left("gpu_memory_controls")
            .resizable(false)
            .min_size(360.0)
            .show_inside(ui, |ui| {
                let settings_height = (ui.available_height() - CONTROLS_ACTION_HEIGHT).max(120.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), settings_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("gpu_memory_controls_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.heading("Controls");
                                ui.add_space(8.0);

                                ui.label("GPU adapter");
                                ui.add_enabled_ui(!self.gpu_memory.running, |ui| {
                                    egui::ComboBox::from_id_salt("gpu_memory_adapter_combo")
                                        .selected_text(
                                            self.adapters
                                                .get(self.selected_adapter)
                                                .map(AdapterInfo::label)
                                                .unwrap_or_else(|| "No adapters found".to_owned()),
                                        )
                                        .width(320.0)
                                        .show_ui(ui, |ui| {
                                            for (index, adapter) in self.adapters.iter().enumerate()
                                            {
                                                ui.selectable_value(
                                                    &mut self.selected_adapter,
                                                    index,
                                                    adapter.label(),
                                                );
                                            }
                                        });
                                    if ui.button("Refresh GPUs").clicked() {
                                        self.adapters = enumerate_adapters();
                                        self.selected_adapter = 0;
                                        self.gpu_memory.status =
                                            format!("Found {} adapter(s)", self.adapters.len());
                                        self.gpu_memory.log(self.gpu_memory.status.clone());
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
                                    if let Some((limit, label)) = adapter_memory_limit_bytes(adapter)
                                    {
                                        ui.small(format!(
                                            "Memory estimate: {} ({label})",
                                            format_bytes(limit)
                                        ));
                                    } else {
                                        ui.small("Memory estimate: unavailable for this adapter/backend");
                                    }
                                    ui.small(format!(
                                        "Reported memory: VRAM {}, dedicated system {}, shared {}",
                                        format_optional_bytes(adapter.dedicated_vram_bytes),
                                        format_optional_bytes(adapter.dedicated_system_memory_bytes),
                                        format_optional_bytes(adapter.shared_system_memory_bytes)
                                    ));
                                }

                                ui.separator();
                                ui.label("Buffer size");
                                ui.add_enabled_ui(!self.gpu_memory.running, |ui| {
                                    egui::ComboBox::from_id_salt("gpu_memory_size_combo")
                                        .selected_text(self.gpu_memory.buffer_size.label())
                                        .show_ui(ui, |ui| {
                                            for size in GpuMemoryBufferSize::ALL {
                                                ui.selectable_value(
                                                    &mut self.gpu_memory.buffer_size,
                                                    size,
                                                    size.label(),
                                                );
                                            }
                                        });
                                });
                                let planned_size = self
                                    .gpu_memory
                                    .planned_buffer_size(self.adapters.get(self.selected_adapter));
                                ui.small(format!("Requested buffer: {}", format_bytes(planned_size)));
                                ui.small("Internal shader tests may clamp this to the adapter storage-buffer binding limit.");

                                ui.add_space(8.0);
                                ui.label("Iterations");
                                ui.add_enabled_ui(!self.gpu_memory.running, |ui| {
                                    ui.horizontal(|ui| {
                                        for iterations in [3_u32, 5, 10, 20] {
                                            ui.selectable_value(
                                                &mut self.gpu_memory.iterations,
                                                iterations,
                                                iterations.to_string(),
                                            );
                                        }
                                    });
                                });
                                self.gpu_memory.iterations =
                                    self.gpu_memory.iterations.clamp(1, GPU_MEMORY_MAX_ITERATIONS);

                                ui.separator();
                                ui.label("Tests");
                                ui.add_enabled_ui(!self.gpu_memory.running, |ui| {
                                    ui.checkbox(
                                        &mut self.gpu_memory.run_internal_read_write,
                                        GpuMemoryTestKind::InternalReadWrite.label(),
                                    )
                                    .on_hover_text(
                                        GpuMemoryTestKind::InternalReadWrite.description(),
                                    );
                                    ui.checkbox(
                                        &mut self.gpu_memory.run_device_copy,
                                        GpuMemoryTestKind::DeviceCopy.label(),
                                    )
                                    .on_hover_text(GpuMemoryTestKind::DeviceCopy.description());
                                    ui.checkbox(
                                        &mut self.gpu_memory.run_upload,
                                        GpuMemoryTestKind::Upload.label(),
                                    )
                                    .on_hover_text(GpuMemoryTestKind::Upload.description());
                                    ui.checkbox(
                                        &mut self.gpu_memory.run_readback,
                                        GpuMemoryTestKind::Readback.label(),
                                    )
                                    .on_hover_text(GpuMemoryTestKind::Readback.description());
                                    ui.checkbox(
                                        &mut self.gpu_memory.run_round_trip,
                                        GpuMemoryTestKind::RoundTrip.label(),
                                    )
                                    .on_hover_text(GpuMemoryTestKind::RoundTrip.description());
                                });
                                ui.colored_label(
                                    egui::Color32::YELLOW,
                                    "Upload/readback numbers include driver, staging, and synchronization overhead.",
                                );
                                ui.small("Internal read/write is the closest result to GPU memory or VRAM bandwidth.");
                            });
                    },
                );

                ui.separator();
                ui.add_enabled_ui(!self.gpu_memory.running && !self.adapters.is_empty(), |ui| {
                    if ui_start_action_button(ui, "Run GPU memory benchmark").clicked() {
                        self.start_gpu_memory_benchmark();
                    }
                });
                ui.add_enabled_ui(self.gpu_memory.running, |ui| {
                    if ui_cancel_action_button(ui, "Cancel GPU memory benchmark").clicked() {
                        self.gpu_memory.cancel();
                    }
                });
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            let available_height = ui.available_height();
            let (results_height, log_height) =
                panel_content_log_heights(available_height, 0.18, 150.0);

            ui.heading("GPU Memory Results");
            ui.add_space(6.0);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), results_height),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            egui::Grid::new("gpu_memory_results_grid")
                                .striped(true)
                                .num_columns(11)
                                .show(ui, |ui| {
                                    result_header(ui, "Test");
                                    result_header(ui, "Buffer");
                                    result_header(ui, "Iterations");
                                    result_header(ui, "Bytes");
                                    result_header(ui, "Time ms");
                                    result_header(ui, "Avg GB/s");
                                    result_header(ui, "Best GB/s");
                                    result_header(ui, "Timing");
                                    result_header(ui, "Adapter");
                                    result_header(ui, "GPU temp");
                                    result_header(ui, "Validation / Notes");
                                    ui.end_row();

                                    for result in &self.gpu_memory.results {
                                        result_cell(ui, result.test.label());
                                        result_cell(ui, format_bytes(result.buffer_size_bytes));
                                        result_cell(ui, result.iterations.to_string());
                                        result_cell(ui, format_bytes(result.bytes_processed));
                                        result_cell(ui, format_ms(Some(result.elapsed_ms)));
                                        result_cell(
                                            ui,
                                            format_gpu_memory_bandwidth(
                                                result.average_bandwidth_gbps,
                                            ),
                                        );
                                        result_cell(
                                            ui,
                                            format_gpu_memory_bandwidth(
                                                result.best_bandwidth_gbps,
                                            ),
                                        );
                                        result_cell(ui, result.timing_source.label());
                                        result_cell(ui, result.adapter.as_str());
                                        result_cell(
                                            ui,
                                            format_temperature_summary(&result.gpu_temperature),
                                        );
                                        let mut detail = result.validation.clone();
                                        if !result.notes.is_empty() {
                                            detail.push_str("; ");
                                            detail.push_str(&result.notes.join("; "));
                                        }
                                        result_cell(ui, detail);
                                        ui.end_row();
                                    }
                                });
                        });
                },
            );

            ui.separator();
            ui.heading("Log");
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .max_height(log_height)
                .show(ui, |ui| {
                    for line in &self.gpu_memory.log {
                        ui_log_line(ui, line);
                    }
                });
        });

        self.ui_sensor_window(&ctx);

        if self.gpu_memory_back_confirm {
            egui::Window::new("Return to main menu?")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label("A GPU memory benchmark is currently running.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Stay").clicked() {
                            self.gpu_memory_back_confirm = false;
                        }
                        if ui.button("Cancel and return").clicked() {
                            self.gpu_memory.cancel();
                            self.gpu_memory_back_confirm = false;
                            self.view = AppView::MainMenu;
                        }
                    });
                });
        }
    }
}
