impl BenchScopeApp {
    fn ui_matrix_benchmark(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::top("top_panel").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui_large_back_button(ui).clicked() {
                    self.request_matrix_back_to_menu();
                }
                self.ui_fullscreen_button(ui);
                ui.separator();
                ui.heading("BenchScope");
                ui.separator();
                ui.label(&self.status);
                if !self.eta_text.is_empty() {
                    ui.separator();
                    ui.label(&self.eta_text);
                }
            });
            ui.horizontal(|ui| {
                ui.label("CPU");
                ui.add(
                    egui::ProgressBar::new(self.cpu_progress)
                        .show_percentage()
                        .desired_width(260.0),
                );
                ui.label("GPU");
                ui.add(
                    egui::ProgressBar::new(self.gpu_progress)
                        .show_percentage()
                        .desired_width(260.0),
                );
            });
        });

        egui::Panel::left("controls")
            .resizable(false)
            .min_size(330.0)
            .show_inside(ui, |ui| {
                let settings_height = (ui.available_height() - CONTROLS_ACTION_HEIGHT).max(120.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), settings_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("matrix_controls_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.heading("Controls");
                                ui.add_space(8.0);

                ui.small(format!("CPU: {}", self.cpu_info.label()));
                ui.add_space(4.0);

                ui.label("GPU adapter");
                egui::ComboBox::from_id_salt("adapter_combo")
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
                    ui.small(format!(
                        "Reported memory: VRAM {}, dedicated system {}, shared {}",
                        format_optional_bytes(adapter.dedicated_vram_bytes),
                        format_optional_bytes(adapter.dedicated_system_memory_bytes),
                        format_optional_bytes(adapter.shared_system_memory_bytes)
                    ));
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

                ui.add_space(6.0);
                if let Some(adapter) = self.adapters.get(self.selected_adapter) {
                    match adapter_vendor(adapter) {
                        GpuVendor::Nvidia => {
                            ui.label("PyTorch CUDA");
                            ui.text_edit_singleline(&mut self.pytorch_python);
                            ui.horizontal(|ui| {
                                ui.add_enabled_ui(
                                    !self.running
                                        && !self.pytorch_probe_running
                                        && !self.pytorch_install_running,
                                    |ui| {
                                        if ui.button("Probe").clicked() {
                                            self.start_pytorch_cuda_probe();
                                        }
                                        if ui.button("Install CUDA PyTorch").clicked() {
                                            self.request_pytorch_cuda_install();
                                        }
                                    },
                                );
                                if self.pytorch_install_running {
                                    ui.label("Installing...");
                                } else if self.pytorch_probe_running {
                                    ui.label("Probing...");
                                }
                            });
                            ui_pytorch_cuda_status(
                                ui,
                                self.pytorch_probe.as_ref(),
                                "NVIDIA adapters try auto-detected PyTorch CUDA before WGPU fallback.",
                            );
                        }
                        GpuVendor::Amd => {
                            ui.label("PyTorch ROCm");
                            ui.text_edit_singleline(&mut self.pytorch_python);
                            ui.small(
                                "AMD adapters try PyTorch ROCm from this Python before optimized WGPU fallback.",
                            );
                        }
                        GpuVendor::Intel => {
                            ui.label("PyTorch XPU");
                            ui.text_edit_singleline(&mut self.pytorch_python);
                            ui.small(
                                "Intel adapters try PyTorch XPU from this Python before optimized WGPU fallback.",
                            );
                        }
                        GpuVendor::Other => {
                            ui.label("Optimized WGPU");
                            ui.small("This adapter uses the cross-vendor WGPU path.");
                        }
                    }
                }

                if ui.button("Refresh GPUs").clicked() && !self.running {
                    self.adapters = enumerate_adapters();
                    self.selected_adapter = 0;
                    self.status = format!("Found {} adapter(s)", self.adapters.len());
                    self.log(self.status.clone());
                }

                ui.separator();
                ui.label("Matrix size");
                egui::ComboBox::from_id_salt("size_combo")
                    .selected_text(self.size_text.clone())
                    .show_ui(ui, |ui| {
                        for size in DEFAULT_SIZES {
                            ui.selectable_value(&mut self.size_text, size.to_string(), size.to_string());
                        }
                });
                ui.text_edit_singleline(&mut self.size_text);
                ui.checkbox(&mut self.validate_output, "Validate GPU output");
                ui.checkbox(&mut self.estimate_cpu_time, "Estimate CPU time");

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
                    if size >= 4096 {
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            if self.estimate_cpu_time {
                                "CPU time will be estimated from sampled work on this CPU."
                            } else {
                                "Exact CPU timing can take a very long time at this size."
                            },
                        );
                    }
                    if size >= 8192 {
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            "Large GPU runs are split into smaller submissions in Safe mode to reduce driver timeout risk.",
                        );
                    }
                    if size == 16384 {
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            "16K uses about 3 GB for A/B/C alone before readback and driver overhead.",
                        );
                    }
                }
                            });
                    },
                );
                ui.separator();

                ui.add_enabled_ui(!self.running && !self.adapters.is_empty(), |ui| {
                    if ui_start_action_button(ui, "Run benchmark").clicked() {
                        self.start_single();
                    }
                });
                ui.add_enabled_ui(self.running && !self.repeat_running, |ui| {
                    if ui_cancel_action_button(ui, "Cancel benchmark").clicked() {
                        self.cancel_single();
                    }
                });
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui_resizable_log_panel(
                ui,
                "matrix_benchmark_log",
                "matrix_benchmark_log_scroll",
                0.18,
                150.0,
                |ui| {
                    for line in &self.log {
                        ui_log_line(ui, line);
                    }
                },
            );

            egui::CentralPanel::default().show_inside(ui, |ui| {
                ui.heading("Results");
                ui.add_space(6.0);
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                            egui::Grid::new("results_grid")
                                .striped(true)
                                .num_columns(18)
                                .show(ui, |ui| {
                                    result_header(ui, "Size");
                                    result_header(ui, "CPU ms");
                                    result_header(ui, "GPU compute ms");
                                    result_header(ui, "GPU total ms");
                                    result_header(ui, "Transfer/sync ms");
                                    result_header(ui, "Speedup");
                                    result_header(ui, "CPU model");
                                    result_header(ui, "Adapter");
                                    result_header(ui, "GPU path");
                                    result_header(ui, "Tile");
                                    result_header(ui, "Dispatches");
                                    result_header(ui, "Last dispatch ms");
                                    result_header(ui, "Avg dispatch ms");
                                    result_header(ui, "Max dispatch ms");
                                    result_header(ui, "Backoffs");
                                    result_header(ui, "CPU temp");
                                    result_header(ui, "GPU temp");
                                    result_header(ui, "Validation");
                                    ui.end_row();

                                    for result in &self.results {
                                        result_cell(ui, format!("{}x{}", result.size, result.size));
                                        result_cell(ui, format_cpu_ms(result));
                                        result_cell(ui, format_ms(result.gpu_compute_ms));
                                        result_cell(ui, format_ms(Some(result.gpu_total_ms)));
                                        result_cell(ui, format_ms(result.transfer_sync_ms));
                                        result_cell(ui, format_speedup(result.speedup));
                                        result_cell(ui, result.cpu_model.as_str());
                                        result_cell(ui, result.adapter.as_str());
                                        result_cell(ui, result.gpu_path.label());
                                        result_cell(ui, result.tile_shape.as_str());
                                        result_cell(ui, result.dispatch_count.to_string());
                                        result_cell(ui, format_ms(result.last_dispatch_ms));
                                        result_cell(ui, format_ms(result.avg_dispatch_ms));
                                        result_cell(ui, format_ms(result.max_dispatch_ms));
                                        result_cell(ui, result.backoff_count.to_string());
                                        result_cell(
                                            ui,
                                            format_temperature_summary(&result.cpu_temperature),
                                        );
                                        result_cell(
                                            ui,
                                            format_temperature_summary(&result.gpu_temperature),
                                        );
                                        result_cell(ui, result.validation.as_str());
                                        ui.end_row();
                                    }
                                });
                            ui.separator();
                            self.ui_timeline_panel(ui, TimelineScope::MatrixBenchmark);
                    });
            });
        });

        self.ui_sensor_window(ctx);

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

        if self.pending_pytorch_install {
            egui::Window::new("Install PyTorch CUDA?")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label("BenchScope can install the CUDA 12.8 PyTorch packages into this Python:");
                    ui.monospace(self.pytorch_python.trim());
                    ui.add_space(6.0);
                    ui.label(format!(
                        "This downloads {} and may take several minutes.",
                        PYTORCH_CUDA_INSTALL_DOWNLOAD_NOTE
                    ));
                    ui.monospace(pytorch_cuda_install_command_preview(
                        self.pytorch_python.trim(),
                    ));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.pending_pytorch_install = false;
                            self.log("Canceled PyTorch CUDA install prompt");
                        }
                        ui.add_enabled_ui(
                            !self.pytorch_python.trim().is_empty()
                                && !self.running
                                && !self.pytorch_probe_running
                                && !self.pytorch_install_running,
                            |ui| {
                                if ui.button("Install").clicked() {
                                    self.start_pytorch_cuda_install();
                                }
                            },
                        );
                    });
                });
        }

        if self.matrix_back_confirm {
            egui::Window::new("Return to main menu?")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label("A matrix benchmark is currently running.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Stay").clicked() {
                            self.matrix_back_confirm = false;
                        }
                        if ui.button("Cancel and return").clicked() {
                            if self.repeat_running {
                                self.cancel_repeat();
                            } else {
                                self.cancel_single();
                            }
                            self.matrix_back_confirm = false;
                            self.view = AppView::MainMenu;
                        }
                    });
                });
        }
    }
}
