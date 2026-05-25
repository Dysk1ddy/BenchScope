impl BenchScopeApp {
    fn request_ai_training_back_to_menu(&mut self) {
        if self.ai_training.running {
            self.ai_training_back_confirm = true;
        } else {
            self.view = AppView::MainMenu;
        }
    }

    fn ui_ai_training_benchmark(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        egui::Panel::top("ai_training_top_panel").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui_large_back_button(ui).clicked() {
                    self.request_ai_training_back_to_menu();
                }
                self.ui_fullscreen_button(ui);
                ui.separator();
                ui.heading("AI Training GPU Benchmark");
                ui.separator();
                ui.label(&self.ai_training.status);
                if !self.ai_training.eta_text.is_empty() {
                    ui.separator();
                    ui.label(&self.ai_training.eta_text);
                }
            });
            ui.add(
                egui::ProgressBar::new(self.ai_training.progress)
                    .show_percentage()
                    .text(self.ai_training.phase.as_str()),
            );
        });

        egui::Panel::left("ai_training_controls")
            .resizable(false)
            .min_size(360.0)
            .show_inside(ui, |ui| {
                let settings_height = (ui.available_height() - CONTROLS_ACTION_HEIGHT).max(160.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), settings_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("ai_training_controls_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.heading("Controls");
                                ui.add_space(8.0);

                                ui.label("Backend");
                                let mut backend = self.ai_training.backend;
                                egui::ComboBox::from_id_salt("ai_training_backend_combo")
                                    .selected_text(backend.label())
                                    .show_ui(ui, |ui| {
                                        for item in AiTrainingBackend::ALL {
                                            ui.selectable_value(&mut backend, item, item.label());
                                        }
                                    });
                                self.ai_training.set_backend(backend);
                                ui.small(self.ai_training.backend.description());

                                if self.ai_training.backend == AiTrainingBackend::PyTorchCuda {
                                    ui.add_space(6.0);
                                    ui.label("Python executable");
                                    ui.text_edit_singleline(&mut self.ai_training.pytorch_python);
                                    ui.horizontal(|ui| {
                                        ui.add_enabled_ui(
                                            !self.ai_training.pytorch_probe_running
                                                && !self.ai_training.pytorch_install_running
                                                && !self.ai_training.running,
                                            |ui| {
                                                if ui.button("Probe PyTorch CUDA").clicked() {
                                                    self.ai_training.start_pytorch_probe();
                                                }
                                                if ui.button("Install CUDA PyTorch").clicked() {
                                                    self.ai_training.request_pytorch_install();
                                                }
                                            },
                                        );
                                        if self.ai_training.pytorch_install_running {
                                            ui.label("Installing...");
                                        } else if self.ai_training.pytorch_probe_running {
                                            ui.label("Probing...");
                                        }
                                    });
                                    ui_pytorch_cuda_probe_summary(ui, &self.ai_training);
                                    ui_pytorch_cuda_device_selector(ui, &mut self.ai_training);
                                }

                                if self.ai_training.backend == AiTrainingBackend::PortableWgpu {
                                    ui.add_space(8.0);
                                    ui.label("Portable WGPU adapter");
                                    egui::ComboBox::from_id_salt("ai_training_adapter_combo")
                                        .selected_text(
                                            self.adapters
                                                .get(self.selected_adapter)
                                                .map(AdapterInfo::label)
                                                .unwrap_or_else(|| "No adapters found".to_owned()),
                                        )
                                        .show_ui(ui, |ui| {
                                            for (index, adapter) in self.adapters.iter().enumerate() {
                                                ui.selectable_value(
                                                    &mut self.selected_adapter,
                                                    index,
                                                    adapter.label(),
                                                );
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
                                            ui.small(format!(
                                                "Memory limit estimate: {} ({label})",
                                                format_bytes(limit)
                                            ));
                                        } else {
                                            ui.small("Memory limit estimate: unavailable for this adapter/backend");
                                        }
                                    }
                                }

                                ui.add_space(8.0);
                                ui.label("Workload");
                                let mut workload = self.ai_training.workload;
                                egui::ComboBox::from_id_salt("ai_training_workload_combo")
                                    .selected_text(workload.label())
                                    .show_ui(ui, |ui| {
                                        for item in AiTrainingWorkload::ALL {
                                            let enabled = self.ai_training.backend
                                                != AiTrainingBackend::PyTorchCuda
                                                || item != AiTrainingWorkload::OptimizerStress;
                                            ui.add_enabled_ui(enabled, |ui| {
                                                ui.selectable_value(&mut workload, item, item.label());
                                            });
                                        }
                                    });
                                self.ai_training.set_workload(workload);
                                ui.small(
                                    self.ai_training
                                        .workload
                                        .description_for_backend(self.ai_training.backend),
                                );

                                ui.add_space(8.0);
                                ui.label("Preset");
                                let mut preset = self.ai_training.preset;
                                egui::ComboBox::from_id_salt("ai_training_preset_combo")
                                    .selected_text(preset.label())
                                    .show_ui(ui, |ui| {
                                        for item in AiTrainingPreset::ALL {
                                            ui.selectable_value(&mut preset, item, item.label());
                                        }
                                    });
                                self.ai_training.set_preset(preset);

                                ui.add_space(8.0);
                                ui.label("Profile");
                                let mut profile = self.ai_training.profile;
                                egui::ComboBox::from_id_salt("ai_training_profile_combo")
                                    .selected_text(profile.label())
                                    .show_ui(ui, |ui| {
                                        for item in AiTrainingProfile::ALL {
                                            ui.selectable_value(&mut profile, item, item.label());
                                        }
                                    });
                                self.ai_training.set_profile(profile);

                                ui.add_space(8.0);
                                ui.label("Precision");
                                ui.horizontal(|ui| {
                                    for precision in AiTrainingPrecision::ALL {
                                        let enabled = self.ai_training.backend
                                            == AiTrainingBackend::PyTorchCuda
                                            || precision == AiTrainingPrecision::F32;
                                        ui.add_enabled_ui(enabled, |ui| {
                                            ui.selectable_value(
                                                &mut self.ai_training.precision,
                                                precision,
                                                precision.label(),
                                            );
                                        });
                                    }
                                });
                                if self.ai_training.backend != AiTrainingBackend::PyTorchCuda
                                    && self.ai_training.precision != AiTrainingPrecision::F32
                                {
                                    self.ai_training.precision = AiTrainingPrecision::F32;
                                }
                                if self.ai_training.backend == AiTrainingBackend::PyTorchCuda {
                                    ui.small("PyTorch CUDA uses f32, bf16, or f16 tensors with CUDA event timing.");
                                } else {
                                    ui.small("Portable wgpu currently supports f32 precision.");
                                }

                                ui.add_space(8.0);
                                ui.label("GPU intensity");
                                ui.horizontal(|ui| {
                                    for intensity in GpuIntensity::ALL {
                                        ui.selectable_value(
                                            &mut self.gpu_intensity,
                                            intensity,
                                            intensity.label(),
                                        );
                                    }
                                });
                                ui.small(self.gpu_intensity.description());

                                ui.separator();
                                ui.label(egui::RichText::new("Workload shape").strong());
                                ui_ai_training_dimension_controls(ui, &mut self.ai_training);

                                ui.separator();
                                ui.label(egui::RichText::new("Run policy").strong());
                                ui.horizontal(|ui| {
                                    ui.label("Warmup");
                                    ui.add(
                                        egui::DragValue::new(&mut self.ai_training.warmup_steps)
                                            .range(0..=10_000),
                                    );
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Measured");
                                    ui.add(
                                        egui::DragValue::new(&mut self.ai_training.measured_steps)
                                            .range(1..=100_000),
                                    );
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Limit");
                                    ui.add(
                                        egui::DragValue::new(&mut self.ai_training.time_limit_s)
                                            .range(1.0..=3600.0)
                                            .suffix(" s"),
                                    );
                                });
                            });
                    },
                );

                ui.separator();
                let can_run = self.ai_training.can_run() && !self.adapters.is_empty();
                ui.add_enabled_ui(can_run, |ui| {
                    if ui_start_action_button(ui, "Run training benchmark").clicked() {
                        if let Some(adapter) = self.adapters.get(self.selected_adapter).cloned() {
                            let was_running = self.ai_training.running;
                            self.ai_training.start(adapter, self.gpu_intensity);
                            if !was_running && self.ai_training.running {
                                self.start_timeline(
                                    TimelineScope::AiTraining,
                                    format!(
                                        "AI training {} {}",
                                        self.ai_training.backend, self.ai_training.workload
                                    ),
                                );
                            }
                        }
                    }
                });
                let can_sweep = self.ai_training.backend == AiTrainingBackend::PyTorchCuda
                    && self.ai_training.can_run()
                    && !self.adapters.is_empty();
                ui.add_enabled_ui(can_sweep, |ui| {
                    if ui_start_action_button(ui, "Run precision sweep").clicked() {
                        if let Some(adapter) = self.adapters.get(self.selected_adapter).cloned() {
                            let was_running = self.ai_training.running;
                            self.ai_training
                                .start_precision_sweep(adapter, self.gpu_intensity);
                            if !was_running && self.ai_training.running {
                                self.start_timeline(
                                    TimelineScope::AiTraining,
                                    format!("AI precision sweep {}", self.ai_training.workload),
                                );
                            }
                        }
                    }
                });
                let can_smoke = !self.ai_training.running
                    && !self.adapters.is_empty()
                    && !self.ai_training.pytorch_install_running
                    && !self.ai_training.pytorch_probe_running
                    && (self.ai_training.backend != AiTrainingBackend::PyTorchCuda
                        || (self.ai_training.pytorch_cuda_can_run_selection()
                            && !self.ai_training.pytorch_python.trim().is_empty()));
                ui.add_enabled_ui(can_smoke, |ui| {
                    if ui_start_action_button(ui, "Run smoke test").clicked() {
                        if let Some(adapter) = self.adapters.get(self.selected_adapter).cloned() {
                            let was_running = self.ai_training.running;
                            self.ai_training
                                .start_smoke_test(adapter, self.gpu_intensity);
                            if !was_running && self.ai_training.running {
                                self.start_timeline(
                                    TimelineScope::AiTraining,
                                    format!("AI smoke test {}", self.ai_training.workload),
                                );
                            }
                        }
                    }
                });
                ui.add_enabled_ui(self.ai_training.running, |ui| {
                    if ui_cancel_action_button(ui, "Cancel training benchmark").clicked() {
                        self.ai_training.cancel();
                    }
                });
                if self.ai_training.backend == AiTrainingBackend::PyTorchCuda {
                    if !self.ai_training.pytorch_cuda_can_run_selection() {
                        ui.small("PyTorch CUDA currently runs linear, MLP, and transformer training.");
                    } else if !self.ai_training.pytorch_cuda_ready() {
                        ui.small("Probe PyTorch CUDA before a full run. Smoke tests can still report setup errors directly.");
                    } else {
                        ui.small(format!(
                            "PyTorch CUDA will run single-process {} training on CUDA device {}.",
                            self.ai_training.workload.label(),
                            self.ai_training.pytorch_cuda_device
                        ));
                    }
                } else if self.ai_training.precision != AiTrainingPrecision::F32 {
                    ui.small("This milestone can run f32. f16 is staged for the next shader path.");
                } else if self.adapters.is_empty() {
                    ui.small("No GPU adapters are available.");
                }
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui_resizable_log_panel(
                ui,
                "ai_training_log",
                "ai_training_log_scroll",
                0.24,
                210.0,
                |ui| {
                    for line in &self.ai_training.log {
                        ui_log_line(ui, line);
                    }
                },
            );

            egui::CentralPanel::default().show_inside(ui, |ui| {
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                            ui.heading("Benchmark Summary");
                            ui.add_space(8.0);
                            ui_ai_training_summary(ui, &self.ai_training);

                            ui.separator();
                            ui.heading("Results");
                            ui.add_space(6.0);
                            ui_ai_training_results_table(ui, &self.ai_training.results);
                            ui.separator();
                            self.ui_timeline_panel(ui, TimelineScope::AiTraining);
                    });
            });
        });

        self.ui_sensor_window(&ctx);

        if self.ai_training.pending_pytorch_install {
            egui::Window::new("Install PyTorch CUDA?")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label("BenchScope can install the CUDA 12.8 PyTorch packages into this Python:");
                    ui.monospace(self.ai_training.pytorch_python.trim());
                    ui.add_space(6.0);
                    ui.label(format!(
                        "This downloads {} and may take several minutes.",
                        PYTORCH_CUDA_INSTALL_DOWNLOAD_NOTE
                    ));
                    ui.monospace(pytorch_cuda_install_command_preview(
                        self.ai_training.pytorch_python.trim(),
                    ));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.ai_training.pending_pytorch_install = false;
                            self.ai_training.log("Canceled PyTorch CUDA install prompt");
                        }
                        ui.add_enabled_ui(
                            !self.ai_training.pytorch_python.trim().is_empty()
                                && !self.ai_training.running
                                && !self.ai_training.pytorch_probe_running
                                && !self.ai_training.pytorch_install_running,
                            |ui| {
                                if ui.button("Install").clicked() {
                                    self.ai_training.start_pytorch_install();
                                }
                            },
                        );
                    });
                });
        }

        if self.ai_training_back_confirm {
            egui::Window::new("Return to main menu?")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label("An AI training benchmark is currently running.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Stay").clicked() {
                            self.ai_training_back_confirm = false;
                        }
                        if ui.button("Cancel and return").clicked() {
                            self.ai_training.cancel();
                            self.ai_training_back_confirm = false;
                            self.view = AppView::MainMenu;
                        }
                    });
                });
        }
    }
}

fn ui_ai_training_dimension_controls(ui: &mut egui::Ui, state: &mut AiTrainingBenchmarkState) {
    let mut changed = false;
    match state.workload {
        AiTrainingWorkload::LinearLayer => {
            changed |=
                ui_ai_training_usize_control(ui, "Batch", &mut state.dimensions.batch_size, 1..=65_536);
            changed |= ui_ai_training_usize_control(
                ui,
                "Input dim",
                &mut state.dimensions.input_dim,
                1..=65_536,
            );
            changed |= ui_ai_training_usize_control(
                ui,
                "Output dim",
                &mut state.dimensions.output_dim,
                1..=65_536,
            );
            state.dimensions.parameter_count = state
                .dimensions
                .input_dim
                .saturating_mul(state.dimensions.output_dim);
        }
        AiTrainingWorkload::Mlp => {
            changed |=
                ui_ai_training_usize_control(ui, "Batch", &mut state.dimensions.batch_size, 1..=65_536);
            changed |= ui_ai_training_usize_control(
                ui,
                "Hidden",
                &mut state.dimensions.hidden_size,
                1..=65_536,
            );
            changed |= ui_ai_training_usize_control(
                ui,
                "Expansion",
                &mut state.dimensions.output_dim,
                1..=131_072,
            );
            state.dimensions.input_dim = state.dimensions.hidden_size;
            state.dimensions.parameter_count = state
                .dimensions
                .hidden_size
                .saturating_mul(state.dimensions.output_dim)
                .saturating_mul(2);
        }
        AiTrainingWorkload::TransformerBlock => {
            changed |=
                ui_ai_training_usize_control(ui, "Batch", &mut state.dimensions.batch_size, 1..=4096);
            changed |= ui_ai_training_usize_control(
                ui,
                "Sequence",
                &mut state.dimensions.sequence_len,
                1..=16_384,
            );
            changed |= ui_ai_training_usize_control(
                ui,
                "Hidden",
                &mut state.dimensions.hidden_size,
                1..=65_536,
            );
            changed |= ui_ai_training_usize_control(
                ui,
                "Heads",
                &mut state.dimensions.attention_heads,
                1..=512,
            );
            state.dimensions.input_dim = state.dimensions.hidden_size;
            state.dimensions.output_dim = state.dimensions.hidden_size.saturating_mul(4);
            let attention_params = state
                .dimensions
                .hidden_size
                .saturating_mul(state.dimensions.hidden_size)
                .saturating_mul(4);
            let mlp_params = state
                .dimensions
                .hidden_size
                .saturating_mul(state.dimensions.output_dim)
                .saturating_mul(2);
            state.dimensions.parameter_count = attention_params.saturating_add(mlp_params);
        }
        AiTrainingWorkload::OptimizerStress => {
            changed |= ui_ai_training_usize_control(
                ui,
                "Parameters",
                &mut state.dimensions.parameter_count,
                1..=2_000_000_000,
            );
        }
    }
    if changed {
        state.mark_custom();
    }
}

fn ui_ai_training_usize_control(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut usize,
    range: std::ops::RangeInclusive<usize>,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        changed = ui.add(egui::DragValue::new(value).range(range)).changed();
    });
    changed
}

fn ui_pytorch_cuda_probe_summary(ui: &mut egui::Ui, state: &AiTrainingBenchmarkState) {
    let Some(environment) = &state.pytorch_probe else {
        ui_pytorch_cuda_status(
            ui,
            None,
            "Probe PyTorch CUDA to detect torch, CUDA runtime, cuDNN, NCCL, and CUDA devices.",
        );
        return;
    };

    ui_pytorch_cuda_status(ui, Some(environment), "");
    if environment.cuda_available {
        ui.small(format!(
            "torch {}, CUDA {}",
            environment.torch_version.as_deref().unwrap_or("unknown"),
            environment.torch_cuda_version.as_deref().unwrap_or("unknown")
        ));
    }

    for device in &environment.devices {
        ui.small(format!(
            "[{}] {} sm_{}{} {}",
            device.index,
            device.name,
            device.capability_major,
            device.capability_minor,
            format_bytes(device.total_memory_bytes)
        ));
    }
}

fn ui_pytorch_cuda_device_selector(ui: &mut egui::Ui, state: &mut AiTrainingBenchmarkState) {
    let Some(environment) = &state.pytorch_probe else {
        return;
    };
    if !environment.cuda_available || environment.device_count == 0 {
        return;
    }

    let devices = environment.devices.clone();
    let device_count = environment.device_count;
    if !pytorch_cuda_environment_has_device(environment, state.pytorch_cuda_device) {
        state.pytorch_cuda_device = devices.first().map(|device| device.index).unwrap_or(0);
    }

    ui.add_space(6.0);
    ui.label("CUDA device");
    let selected_text = devices
        .iter()
        .find(|device| device.index == state.pytorch_cuda_device)
        .map(pytorch_cuda_device_option_label)
        .unwrap_or_else(|| format!("CUDA device {}", state.pytorch_cuda_device));
    egui::ComboBox::from_id_salt("ai_training_pytorch_cuda_device_combo")
        .selected_text(selected_text)
        .show_ui(ui, |ui| {
            if devices.is_empty() {
                for index in 0..device_count {
                    ui.selectable_value(
                        &mut state.pytorch_cuda_device,
                        index,
                        format!("CUDA device {index}"),
                    );
                }
            } else {
                for device in &devices {
                    ui.selectable_value(
                        &mut state.pytorch_cuda_device,
                        device.index,
                        pytorch_cuda_device_option_label(device),
                    );
                }
            }
        });
}

fn pytorch_cuda_device_option_label(device: &PyTorchCudaDevice) -> String {
    format!(
        "[{}] {} sm_{}{} {}",
        device.index,
        device.name,
        device.capability_major,
        device.capability_minor,
        format_bytes(device.total_memory_bytes)
    )
}

fn ui_ai_training_summary(ui: &mut egui::Ui, state: &AiTrainingBenchmarkState) {
    let flops = state.estimated_flops_per_step();
    let memory = state.estimated_memory_bytes();
    egui::Grid::new("ai_training_summary_grid")
        .striped(true)
        .num_columns(2)
        .show(ui, |ui| {
            result_header(ui, "Setting");
            result_header(ui, "Value");
            ui.end_row();
            result_cell(ui, "Backend");
            result_cell(ui, state.backend.label());
            ui.end_row();
            result_cell(ui, "Workload");
            result_cell(ui, state.workload.label());
            ui.end_row();
            result_cell(ui, "Preset");
            result_cell(ui, state.preset.label());
            ui.end_row();
            result_cell(ui, "Precision");
            result_cell(ui, state.precision.label());
            ui.end_row();
            result_cell(ui, "FLOPs per step");
            result_cell(ui, format_flops_per_step(flops));
            ui.end_row();
            result_cell(ui, "Estimated tensor memory");
            result_cell(ui, format_bytes(memory));
            ui.end_row();
            result_cell(ui, "Warmup / measured steps");
            result_cell(ui, format!("{} / {}", state.warmup_steps, state.measured_steps));
            ui.end_row();
            result_cell(ui, "Time limit");
            result_cell(ui, format_elapsed(state.time_limit_s));
            ui.end_row();
            result_cell(ui, "Throughput metric");
            result_cell(ui, state.workload.throughput_label());
            ui.end_row();
        });
}

fn ui_ai_training_results_table(ui: &mut egui::Ui, results: &[AiTrainingResult]) {
    if results.is_empty() {
        ui.label("No AI training benchmark results yet.");
        return;
    }

    egui::Grid::new("ai_training_results_grid")
        .striped(true)
        .num_columns(17)
        .show(ui, |ui| {
            result_header_info(ui, "Workload", AI_RESULT_HELP_WORKLOAD);
            result_header_info(ui, "Backend", AI_RESULT_HELP_BACKEND);
            result_header_info(ui, "Precision", AI_RESULT_HELP_PRECISION);
            result_header_info(ui, "Throughput", AI_RESULT_HELP_THROUGHPUT);
            result_header_info(ui, "Avg step", AI_RESULT_HELP_AVG_STEP);
            result_header_info(ui, "p95 step", AI_RESULT_HELP_P95_STEP);
            result_header_info(ui, "Step split", AI_RESULT_HELP_STEP_SPLIT);
            result_header_info(ui, "Memory", AI_RESULT_HELP_MEMORY);
            result_header_info(ui, "Steps", AI_RESULT_HELP_STEPS);
            result_header_info(ui, "Shape", AI_RESULT_HELP_SHAPE);
            result_header_info(ui, "GPU(s)", AI_RESULT_HELP_GPU);
            result_header_info(ui, "E2E TFLOP/s", AI_RESULT_HELP_E2E_TFLOPS);
            result_header_info(ui, "Compute TFLOP/s", AI_RESULT_HELP_COMPUTE_TFLOPS);
            result_header_info(ui, "FLOPs/step", AI_RESULT_HELP_FLOPS_STEP);
            result_header_info(ui, "Preset", AI_RESULT_HELP_PRESET);
            result_header_info(ui, "Validation", AI_RESULT_HELP_VALIDATION);
            result_header_info(ui, "Notes", AI_RESULT_HELP_NOTES);
            ui.end_row();

            for result in results {
                result_cell(ui, result.workload.label());
                result_cell(ui, result.backend.label());
                result_cell(ui, result.precision.label());
                result_cell(
                    ui,
                    result
                        .throughput_value
                        .map(|value| format!("{value:.1} {}", result.throughput_label))
                        .unwrap_or_else(|| "N/A".to_owned()),
                );
                result_cell(ui, format_ms(result.avg_step_ms));
                result_cell(ui, format_ms(result.p95_step_ms));
                let step_timings = format_ai_step_timings(result.step_timings.as_ref());
                result_cell_hover(ui, step_timings.clone(), step_timings);
                result_cell(ui, format_bytes(result.memory_bytes));
                result_cell(ui, result.measured_steps.to_string());
                result_cell_hover(ui, result.shape.clone(), result.shape.clone());
                let gpu_names = result.gpu_names.join(", ");
                result_cell_hover(ui, gpu_names.clone(), gpu_names);
                result_cell(ui, format_optional_tflops(result.end_to_end_tflops));
                result_cell(ui, format_optional_tflops(result.compute_tflops));
                result_cell(ui, format_flops_per_step(result.flops_per_step));
                result_cell(ui, result.preset.label());
                result_cell_hover(ui, result.validation.clone(), result.validation.clone());
                result_cell_hover(ui, result.notes.clone(), result.notes.clone());
                ui.end_row();
            }
        });
}

const AI_RESULT_HELP_WORKLOAD: &str =
    "The training workload shape. PyTorch CUDA workloads run real model steps; Portable WGPU workloads are synthetic proxies.";
const AI_RESULT_HELP_BACKEND: &str =
    "PyTorch CUDA training is the primary path for real training behavior. Portable WGPU proxy is a cross-vendor fallback for synthetic GEMM/update work.";
const AI_RESULT_HELP_PRECISION: &str =
    "Tensor data type used by the backend. PyTorch CUDA supports f32, bf16, and f16 when the CUDA device supports them. Portable WGPU currently reports f32 proxy runs.";
const AI_RESULT_HELP_THROUGHPUT: &str =
    "Training-oriented throughput: samples/s for linear and MLP, tokens/s for transformer, parameters/s for optimizer pressure. This is usually the first number to compare for AI workloads.";
const AI_RESULT_HELP_AVG_STEP: &str =
    "Average wall-clock time for one measured training step after warmup. Lower is better.";
const AI_RESULT_HELP_P95_STEP: &str =
    "95th percentile measured step latency. This helps reveal stutter or occasional slow training steps that the average can hide.";
const AI_RESULT_HELP_STEP_SPLIT: &str =
    "PyTorch CUDA timing split for an average measured step: forward/loss, backward, and optimizer update. N/A means the backend does not expose a component split yet.";
const AI_RESULT_HELP_MEMORY: &str =
    "Estimated tensor memory for portable runs, or the larger of PyTorch peak allocated/reserved CUDA memory and the static estimate for PyTorch runs.";
const AI_RESULT_HELP_STEPS: &str =
    "Number of measured steps included in the result. Warmup steps are run first and excluded from the metrics.";
const AI_RESULT_HELP_SHAPE: &str =
    "The workload dimensions: batch/input/output for linear, batch/hidden/expansion for MLP, batch/sequence/hidden/heads for transformer, or parameter count for optimizer pressure.";
const AI_RESULT_HELP_GPU: &str =
    "The adapter or CUDA device that produced the result.";
const AI_RESULT_HELP_E2E_TFLOPS: &str =
    "Effective FLOP/s based on full measured step wall time. This includes launch, synchronization, framework overhead, and optimizer work.";
const AI_RESULT_HELP_COMPUTE_TFLOPS: &str =
    "Effective FLOP/s from GPU-side timing where available: CUDA events for PyTorch, timestamp queries for WGPU. This excludes more host-side overhead than E2E TFLOP/s.";
const AI_RESULT_HELP_FLOPS_STEP: &str =
    "Estimated floating-point operations per training step for the configured workload. For proxies, some model operations may be accounted rather than explicitly executed.";
const AI_RESULT_HELP_PRESET: &str =
    "The size preset used before any automatic memory-limit sizing. Custom means at least one dimension was edited.";
const AI_RESULT_HELP_VALIDATION: &str =
    "Sanity check result. PyTorch validates that training completed with finite loss. Small WGPU linear runs can compare against a CPU reference.";
const AI_RESULT_HELP_NOTES: &str =
    "Important context such as proxy status, CUDA versions, memory resizing, time limits, timestamp availability, and backend caveats.";

fn format_ai_step_timings(timings: Option<&AiTrainingStepTimings>) -> String {
    timings
        .map(|timings| {
            format!(
                "F/L {}, B {}, O {}",
                format_ms(Some(timings.forward_loss_ms)),
                format_ms(Some(timings.backward_ms)),
                format_ms(Some(timings.optimizer_ms))
            )
        })
        .unwrap_or_else(|| "N/A".to_owned())
}
