impl BenchScopeApp {
    fn request_storage_health_back_to_menu(&mut self) {
        if self.storage_health.running {
            self.storage_health_back_confirm = true;
        } else {
            self.view = AppView::MainMenu;
        }
    }
    fn ui_storage_health(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        egui::Panel::top("storage_health_top_panel").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui_large_back_button(ui).clicked() {
                    self.request_storage_health_back_to_menu();
                }
                self.ui_fullscreen_button(ui);
                ui.separator();
                ui.heading("SSD / HDD Health Checker");
                ui.separator();
                ui.label(&self.storage_health.status);
                if !self.storage_health.eta_text.is_empty() {
                    ui.separator();
                    ui.label(&self.storage_health.eta_text);
                }
            });
            ui.add(
                egui::ProgressBar::new(self.storage_health.progress)
                    .show_percentage()
                    .text(match self.storage_health.active_task {
                        Some(StorageHealthTask::Snapshot) => "Reading health data",
                        Some(StorageHealthTask::Scan) => "Read-only scan",
                        Some(StorageHealthTask::Benchmark) => "Quick benchmark",
                        None => "Storage health",
                    }),
            );
        });

        egui::Panel::left("storage_health_controls")
            .resizable(false)
            .min_size(360.0)
            .show_inside(ui, |ui| {
                let settings_height = (ui.available_height() - CONTROLS_ACTION_HEIGHT).max(120.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), settings_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("storage_health_controls_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.heading("Controls");
                                ui.add_space(8.0);

                                ui.label("Target drive");
                                ui.add_enabled_ui(!self.storage_health.running, |ui| {
                                    let mut selected_drive = self.storage_health.selected_drive;
                                    egui::ComboBox::from_id_salt("storage_health_drive_combo")
                                        .selected_text(self.storage_health.selected_drive_label())
                                        .width(320.0)
                                        .show_ui(ui, |ui| {
                                            for (index, drive) in self
                                                .storage_health
                                                .drives
                                                .iter()
                                                .enumerate()
                                            {
                                                ui.selectable_value(
                                                    &mut selected_drive,
                                                    index,
                                                    &drive.label,
                                                );
                                            }
                                        });
                                    if selected_drive != self.storage_health.selected_drive {
                                        self.storage_health.select_drive(selected_drive);
                                    }
                                    if ui.button("Refresh drives").clicked() {
                                        self.storage_health.refresh_drives();
                                    }
                                });

                                if let Some(root) = self.storage_health.selected_drive_root() {
                                    ui.small(format!("Volume root: {}", root.display()));
                                }
                                ui.small("SMART/NVMe data depends on what Windows, the controller, and the drive expose.");

                                ui.separator();
                                ui.add_enabled_ui(!self.storage_health.running, |ui| {
                                    if ui.button("Refresh health snapshot").clicked() {
                                        self.storage_health.start_refresh();
                                    }
                                });

                                ui.separator();
                                ui.label("Read-only surface scan");
                                ui.add_enabled_ui(!self.storage_health.running, |ui| {
                                    for mode in StorageScanMode::ALL {
                                        if ui.button(mode.label()).clicked() {
                                            self.storage_health.start_scan(mode);
                                        }
                                    }
                                });
                                ui.small("Reads sampled regions from the volume. Raw access may require administrator permissions.");

                                ui.separator();
                                ui.label("Optional performance check");
                                ui.colored_label(
                                    egui::Color32::YELLOW,
                                    "Quick benchmark writes temporary data and may add SSD wear.",
                                );
                                ui.add_enabled_ui(!self.storage_health.running, |ui| {
                                    if ui.button("Run quick read/write benchmark").clicked() {
                                        self.storage_health.start_quick_benchmark();
                                    }
                                    if ui.button("Open full Drive Benchmark").clicked() {
                                        if let Some(root) = self.storage_health.selected_drive_root() {
                                            self.drive.target_folder_text = root.display().to_string();
                                            self.drive.sync_selected_drive_to_target();
                                        }
                                        self.view = AppView::DriveBenchmark;
                                    }
                                });
                            });
                    },
                );

                ui.separator();
                ui.add_enabled_ui(self.storage_health.running, |ui| {
                    if ui.button("Cancel storage task").clicked() {
                        self.storage_health.cancel();
                    }
                });
                ui.add_enabled_ui(!self.storage_health.running, |ui| {
                    if ui.button("Export health report").clicked() {
                        self.storage_health.export_report();
                    }
                });
                if let Some(path) = &self.storage_health.last_report_path {
                    ui.small(format!("Last report: {}", path.display()));
                }
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            let snapshot = self.storage_health.snapshot.clone();
            let available_height = ui.available_height();
            let (content_height, log_height) =
                panel_content_log_heights(available_height, 0.18, 150.0);

            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), content_height),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("storage_health_content_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if let Some(snapshot) = &snapshot {
                                ui.horizontal(|ui| {
                                    ui.heading("Overall Health");
                                    ui.label(
                                        egui::RichText::new(format_storage_health_percent(
                                            snapshot.health_percent,
                                        ))
                                        .strong()
                                        .color(storage_status_color(snapshot.status)),
                                    );
                                    ui.label(
                                        egui::RichText::new(snapshot.status.label())
                                            .strong()
                                            .color(storage_status_color(snapshot.status)),
                                    );
                                });
                                ui.add_space(6.0);
                                egui::Grid::new("storage_health_summary_grid")
                                    .striped(true)
                                    .num_columns(4)
                                    .show(ui, |ui| {
                                        storage_metric(
                                            ui,
                                            "Health",
                                            &format_storage_health_percent(
                                                snapshot.health_percent,
                                            ),
                                        );
                                        storage_metric(ui, "Status", snapshot.status.label());
                                        ui.end_row();
                                        storage_metric(ui, "Model", &snapshot.model);
                                        storage_metric(
                                            ui,
                                            "Serial",
                                            snapshot.serial.as_deref().unwrap_or("N/A"),
                                        );
                                        ui.end_row();
                                        storage_metric(ui, "Firmware", option_text(snapshot.firmware.as_deref()));
                                        storage_metric(ui, "Bus", &snapshot.bus_type);
                                        ui.end_row();
                                        storage_metric(ui, "Media", &snapshot.media_type);
                                        storage_metric(
                                            ui,
                                            "Capacity",
                                            &format_optional_bytes(snapshot.capacity_bytes),
                                        );
                                        ui.end_row();
                                        storage_metric(
                                            ui,
                                            "Free",
                                            &format_optional_bytes(snapshot.free_bytes),
                                        );
                                        storage_metric(
                                            ui,
                                            "Filesystem",
                                            option_text(snapshot.file_system.as_deref()),
                                        );
                                        ui.end_row();
                                        storage_metric(
                                            ui,
                                            "Temperature",
                                            &format_temperature_value(snapshot.temperature_c),
                                        );
                                        storage_metric(
                                            ui,
                                            "Utilization",
                                            &format_utilization_value(snapshot.utilization_percent),
                                        );
                                        ui.end_row();
                                        storage_metric(
                                            ui,
                                            "Remaining life",
                                            &format_percent_value(snapshot.remaining_life_percent),
                                        );
                                        storage_metric(
                                            ui,
                                            "Power-on hours",
                                            &format_optional_u64(snapshot.power_on_hours),
                                        );
                                        ui.end_row();
                                        storage_metric(
                                            ui,
                                            "NVMe spare",
                                            &format_percent_u64(snapshot.available_spare_percent),
                                        );
                                        storage_metric(
                                            ui,
                                            "NVMe warning flags",
                                            &format_hex_u64(snapshot.critical_warning_flags),
                                        );
                                        ui.end_row();
                                        storage_metric(
                                            ui,
                                            "Unsafe shutdowns",
                                            &format_optional_u64(snapshot.unsafe_shutdowns),
                                        );
                                        storage_metric(
                                            ui,
                                            "Controller busy",
                                            &format_optional_u64_minutes(
                                                snapshot.controller_busy_time_minutes,
                                            ),
                                        );
                                        ui.end_row();
                                        storage_metric(
                                            ui,
                                            "Thermal warning time",
                                            &format_optional_u64_minutes(
                                                snapshot.warning_temperature_time_minutes,
                                            ),
                                        );
                                        storage_metric(
                                            ui,
                                            "Thermal critical time",
                                            &format_optional_u64_minutes(
                                                snapshot.critical_temperature_time_minutes,
                                            ),
                                        );
                                        ui.end_row();
                                    });

                                ui.add_space(10.0);
                                ui.heading("Warnings");
                                if snapshot.warnings.is_empty() {
                                    ui.label("No warning counters were reported by the available providers.");
                                } else {
                                    for warning in &snapshot.warnings {
                                        ui.colored_label(
                                            health_severity_color(warning.severity),
                                            format!(
                                                "{}: {} - {}",
                                                warning.severity.label(),
                                                warning.title,
                                                warning.detail
                                            ),
                                        );
                                    }
                                }

                                ui.add_space(10.0);
                                ui.heading("SMART / NVMe Attributes");
                                egui::ScrollArea::both()
                                    .id_salt("storage_health_attribute_table")
                                    .max_height(220.0)
                                    .show(ui, |ui| {
                                        egui::Grid::new("storage_health_attributes_grid")
                                            .striped(true)
                                            .num_columns(8)
                                            .show(ui, |ui| {
                                                result_header(ui, "ID");
                                                result_header(ui, "Attribute");
                                                result_header(ui, "Current");
                                                result_header(ui, "Worst");
                                                result_header(ui, "Threshold");
                                                result_header(ui, "Raw / value");
                                                result_header(ui, "Severity");
                                                result_header(ui, "Interpretation");
                                                ui.end_row();
                                                for attribute in &snapshot.attributes {
                                                    result_cell(
                                                        ui,
                                                        attribute
                                                            .id
                                                            .map(|id| id.to_string())
                                                            .unwrap_or_else(|| "-".to_owned()),
                                                    );
                                                    result_cell(ui, attribute.name.as_str());
                                                    result_cell(
                                                        ui,
                                                        format_optional_u64(attribute.current),
                                                    );
                                                    result_cell(
                                                        ui,
                                                        format_optional_u64(attribute.worst),
                                                    );
                                                    result_cell(
                                                        ui,
                                                        format_optional_u64(attribute.threshold),
                                                    );
                                                    result_cell(
                                                        ui,
                                                        attribute
                                                            .raw
                                                            .map(|value| value.to_string())
                                                            .unwrap_or_else(|| {
                                                                attribute.display_value.clone()
                                                            }),
                                                    );
                                                    result_cell(ui, attribute.severity.label());
                                                    result_cell(ui, attribute.interpretation.as_str());
                                                    ui.end_row();
                                                }
                                            });
                                    });

                                ui.add_space(10.0);
                                ui.heading("Read-Only Scan");
                                if let Some(result) = &self.storage_health.scan_result {
                                    ui.label(format!(
                                        "{}: {}, {} region(s), {} read error(s), {} slow region(s), avg {}, worst {}, duration {} ms",
                                        result.mode,
                                        format_bytes(result.bytes_scanned),
                                        result.regions_scanned,
                                        result.read_errors,
                                        result.slow_regions,
                                        format_optional_latency(result.avg_latency_ms),
                                        format_optional_latency(result.worst_latency_ms),
                                        format_ms(Some(result.duration_ms))
                                    ));
                                    for note in &result.notes {
                                        ui.small(note);
                                    }
                                } else {
                                    ui.label("No read-only scan has been run yet.");
                                }

                                ui.add_space(10.0);
                                ui.heading("Quick Benchmark Results");
                                if self.storage_health.benchmark_results.is_empty() {
                                    ui.label("No quick benchmark has been run from this screen.");
                                } else {
                                    egui::Grid::new("storage_health_benchmark_grid")
                                        .striped(true)
                                        .num_columns(6)
                                        .show(ui, |ui| {
                                            result_header(ui, "Test");
                                            result_header(ui, "MB/s");
                                            result_header(ui, "IOPS");
                                            result_header(ui, "Avg latency");
                                            result_header(ui, "Mode");
                                            result_header(ui, "Notes");
                                            ui.end_row();
                                            for result in &self.storage_health.benchmark_results {
                                                result_cell(ui, result.test.label());
                                                result_cell(ui, format_drive_speed(result));
                                                result_cell(ui, format_optional_iops(result.iops));
                                                result_cell(
                                                    ui,
                                                    format_optional_latency(result.avg_latency_ms),
                                                );
                                                result_cell(ui, result.io_mode.label());
                                                result_cell(ui, result.notes.join(", "));
                                                ui.end_row();
                                            }
                                        });
                                }

                                if !snapshot.provider_notes.is_empty() {
                                    ui.add_space(10.0);
                                    ui.heading("Provider Notes");
                                    for note in &snapshot.provider_notes {
                                        ui.small(note);
                                    }
                                }
                            } else {
                                ui.heading("Overall Health");
                                ui.label("Choose a drive and refresh the health snapshot to read SMART/NVMe data.");
                            }
                        });
                },
            );

            ui.separator();
            ui.heading("Log");
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .max_height(log_height)
                .show(ui, |ui| {
                    for line in &self.storage_health.log {
                        ui_log_line(ui, line);
                    }
                });
        });

        self.ui_sensor_window(&ctx);

        if self.storage_health_back_confirm {
            egui::Window::new("Return to main menu?")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label("A storage health task is currently running.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Stay").clicked() {
                            self.storage_health_back_confirm = false;
                        }
                        if ui.button("Cancel and return").clicked() {
                            self.storage_health.cancel();
                            self.storage_health_back_confirm = false;
                            self.view = AppView::MainMenu;
                        }
                    });
                });
        }
    }
}
