impl BenchScopeApp {
    fn request_drive_back_to_menu(&mut self) {
        if self.drive.running {
            self.drive_back_confirm = true;
        } else {
            self.view = AppView::MainMenu;
        }
    }
    fn ui_drive_benchmark(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        egui::Panel::top("drive_top_panel").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui_large_back_button(ui).clicked() {
                    self.request_drive_back_to_menu();
                }
                self.ui_fullscreen_button(ui);
                ui.separator();
                ui.heading("Drive Benchmark");
                ui.separator();
                ui.label(&self.drive.status);
                if !self.drive.eta_text.is_empty() {
                    ui.separator();
                    ui.label(&self.drive.eta_text);
                }
            });
            ui.horizontal(|ui| {
                ui.label("Current");
                ui.add(
                    egui::ProgressBar::new(self.drive.current_progress)
                        .show_percentage()
                        .desired_width(260.0),
                );
                ui.label("Suite");
                ui.add(
                    egui::ProgressBar::new(self.drive.suite_progress)
                        .show_percentage()
                        .desired_width(260.0),
                );
            });
        });

        egui::Panel::left("drive_controls")
            .resizable(false)
            .min_size(350.0)
            .show_inside(ui, |ui| {
                let settings_height = (ui.available_height() - CONTROLS_ACTION_HEIGHT).max(120.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), settings_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("drive_controls_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.heading("Controls");
                                ui.add_space(8.0);

                                self.drive.sync_selected_drive_to_target();
                                ui.label("Target drive");
                                ui.add_enabled_ui(!self.drive.running, |ui| {
                                    let mut selected_drive = self.drive.selected_drive;
                                    egui::ComboBox::from_id_salt("drive_picker_combo")
                                        .selected_text(self.drive.selected_drive_label())
                                        .width(320.0)
                                        .show_ui(ui, |ui| {
                                            for (index, drive) in self.drive.drives.iter().enumerate() {
                                                ui.selectable_value(&mut selected_drive, index, &drive.label);
                                            }
                                        });
                                    if selected_drive != self.drive.selected_drive {
                                        self.drive.select_drive(selected_drive);
                                    }
                                    if ui.button("Refresh drives").clicked() {
                                        self.drive.refresh_drives();
                                    }
                                });
                                if let Some(name) = self.drive.selected_drive_device_name() {
                                    ui.small(format!("Device: {name}"));
                                }
                                ui.small("Selecting a drive sets the target folder to that drive root.");

                ui.label("Target folder");
                ui.add_enabled_ui(!self.drive.running, |ui| {
                    ui.text_edit_singleline(&mut self.drive.target_folder_text);
                });
                let target_path = PathBuf::from(self.drive.target_folder_text.trim());
                if target_path.is_dir() {
                    ui.small(format!(
                        "Benchmark files use unique {}-*.{} names",
                        DRIVE_BENCHMARK_FILE_PREFIX, DRIVE_BENCHMARK_FILE_EXTENSION
                    ));
                } else {
                    ui.colored_label(egui::Color32::YELLOW, "Target folder is not valid.");
                }
                ui.small("If the drive root is protected, enter a writable folder on that drive.");

                ui.add_space(8.0);
                ui.label("Profile");
                ui.add_enabled_ui(!self.drive.running, |ui| {
                    ui.horizontal(|ui| {
                        for profile in DriveProfile::ALL {
                            ui.selectable_value(&mut self.drive.profile, profile, profile.label());
                        }
                    });
                });
                ui.small(format!(
                    "Measured target: {} per test, hard cap: {:.0}s",
                    format_elapsed(self.drive.profile.target_duration().as_secs_f64()),
                    DRIVE_MAX_TEST_SECONDS
                ));

                ui.add_space(8.0);
                ui.label("Test file size");
                ui.add_enabled_ui(!self.drive.running, |ui| {
                    egui::ComboBox::from_id_salt("drive_file_size_combo")
                        .selected_text(self.drive.file_size.label())
                        .show_ui(ui, |ui| {
                            for size in DriveFileSize::ALL {
                                ui.selectable_value(&mut self.drive.file_size, size, size.label());
                            }
                        });
                });
                ui.small(format!(
                    "Planned file size: {}",
                    format_bytes(self.drive.planned_file_size())
                ));

                ui.separator();
                ui.label("Tests");
                ui.add_enabled_ui(!self.drive.running, |ui| {
                    ui.checkbox(&mut self.drive.run_seq_read, "Sequential read");
                    ui.checkbox(&mut self.drive.run_seq_write, "Sequential write");
                    ui.checkbox(&mut self.drive.run_random_read, "Random 4 KiB read");
                    ui.checkbox(&mut self.drive.run_random_write, "Random 4 KiB write");
                });
                ui.small("Mode: direct I/O preferred; cached fallback is labeled in results.");
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Write tests create temporary data on the selected drive.",
                );
                let planned_writes = self.drive.planned_write_bytes();
                if planned_writes >= 4 * 1024 * 1024 * 1024 {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        format!(
                            "Selected write tests may write at least {}.",
                            format_bytes(planned_writes)
                        ),
                    );
                }

                            });
                    },
                );
                ui.separator();
                ui.add_enabled_ui(!self.drive.running, |ui| {
                    if ui_start_action_button(ui, "Run drive benchmark").clicked() {
                        let was_running = self.drive.running;
                        self.drive.start();
                        if !was_running && self.drive.running {
                            self.begin_temperature_run(TemperatureScope::Drive);
                        }
                    }
                });
                ui.add_enabled_ui(self.drive.running, |ui| {
                    if ui_cancel_action_button(ui, "Cancel drive benchmark").clicked() {
                        self.drive.cancel();
                    }
                });
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui_resizable_log_panel(
                ui,
                "drive_benchmark_log",
                "drive_benchmark_log_scroll",
                0.18,
                150.0,
                |ui| {
                    for line in &self.drive.log {
                        ui_log_line(ui, line);
                    }
                },
            );

            egui::CentralPanel::default().show_inside(ui, |ui| {
                ui.heading("Drive Results");
                ui.add_space(6.0);
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                            egui::Grid::new("drive_results_grid")
                                .striped(true)
                                .num_columns(10)
                                .show(ui, |ui| {
                                    result_header(ui, "Test");
                                    result_header(ui, "Speed MB/s");
                                    result_header(ui, "IOPS");
                                    result_header(ui, "Avg latency");
                                    result_header(ui, "P95 latency");
                                    result_header(ui, "Duration");
                                    result_header(ui, "File size");
                                    result_header(ui, "Mode");
                                    result_header(ui, "SSD temp");
                                    result_header(ui, "Notes");
                                    ui.end_row();

                                    for result in &self.drive.results {
                                        result_cell(ui, result.test.label());
                                        result_cell(ui, format_drive_speed(result));
                                        result_cell(ui, format_optional_iops(result.iops));
                                        result_cell(
                                            ui,
                                            format_optional_latency(result.avg_latency_ms),
                                        );
                                        result_cell(
                                            ui,
                                            format_optional_latency(result.p95_latency_ms),
                                        );
                                        result_cell(ui, format_ms(Some(result.duration_ms)));
                                        result_cell(ui, format_bytes(result.file_size_bytes));
                                        result_cell(ui, result.io_mode.label());
                                        result_cell(
                                            ui,
                                            format_temperature_summary(&result.ssd_temperature),
                                        );
                                        result_cell(
                                            ui,
                                            if result.notes.is_empty() {
                                                String::new()
                                            } else {
                                                result.notes.join(", ")
                                            },
                                        );
                                        ui.end_row();
                                    }
                                });
                    });
            });
        });

        self.ui_sensor_window(&ctx);

        if self.drive_back_confirm {
            egui::Window::new("Return to main menu?")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label("A drive benchmark is currently running.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Stay").clicked() {
                            self.drive_back_confirm = false;
                        }
                        if ui.button("Cancel and return").clicked() {
                            self.drive.cancel();
                            self.drive_back_confirm = false;
                            self.view = AppView::MainMenu;
                        }
                    });
                });
        }
    }
}
