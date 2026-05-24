impl BenchScopeApp {
    fn request_battery_back_to_menu(&mut self) {
        if self.battery.scanning || self.battery.live_running {
            self.battery_back_confirm = true;
        } else {
            self.view = AppView::MainMenu;
        }
    }
    fn ui_battery_health_diagnostic(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let report = self.battery.latest_report.clone();
        let latest_live = self.battery.latest_live_sample().cloned();
        let runtime_accuracy = battery_runtime_accuracy(&self.battery.live_samples);

        egui::Panel::top("battery_top_panel").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui_large_back_button(ui).clicked() {
                    self.request_battery_back_to_menu();
                }
                self.ui_fullscreen_button(ui);
                ui.separator();
                ui.heading("Battery Health Diagnostic");
                ui.separator();
                ui.label(&self.battery.status);
            });
        });

        egui::Panel::left("battery_controls")
            .resizable(false)
            .min_size(360.0)
            .show_inside(ui, |ui| {
                let settings_height = (ui.available_height() - CONTROLS_ACTION_HEIGHT).max(120.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), settings_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("battery_controls_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.heading("Controls");
                                ui.add_space(8.0);

                                ui.label("Battery report window");
                                ui.add_enabled_ui(!self.battery.scanning, |ui| {
                                    egui::ComboBox::from_id_salt("battery_duration_combo")
                                        .selected_text(self.battery.duration.label())
                                        .show_ui(ui, |ui| {
                                            for duration in BatteryReportDuration::ALL {
                                                ui.selectable_value(
                                                    &mut self.battery.duration,
                                                    duration,
                                                    duration.label(),
                                                );
                                            }
                                        });
                                });
                                ui.small("Uses powercfg /batteryreport /xml and merges live Windows battery state.");

                                ui.add_space(10.0);
                                ui.add_enabled_ui(!self.battery.scanning, |ui| {
                                    if ui_start_action_button(ui, "Refresh battery scan").clicked() {
                                        self.battery.start_scan();
                                    }
                                });
                                ui.add_enabled_ui(self.battery.scanning, |ui| {
                                    if ui_cancel_action_button(ui, "Cancel scan").clicked() {
                                        self.battery.cancel_scan();
                                    }
                                });

                                ui.add_space(10.0);
                                if self.battery.live_running {
                                    if ui_cancel_action_button(ui, "Stop live sampling").clicked() {
                                        self.battery.stop_live_sampling();
                                    }
                                } else if ui_start_action_button(ui, "Start live sampling").clicked() {
                                    self.battery.start_live_sampling();
                                }
                                ui.small(format!(
                                    "Live samples kept: {}",
                                    self.battery.live_samples.len()
                                ));
                            });
                    },
                );
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            egui::ScrollArea::both()
                .id_salt("battery_content_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if let Some(report) = &report {
                        self.ui_battery_summary(
                            ui,
                            report,
                            latest_live.as_ref(),
                            runtime_accuracy.as_ref(),
                        );
                    } else {
                        ui.heading("Battery Summary");
                        ui.label("Run a battery scan to generate the diagnostic summary.");
                    }
                });
        });

        self.ui_sensor_window(&ctx);

        if self.battery_back_confirm {
            egui::Window::new("Return to main menu?")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label("A battery scan or live sampling session is active.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Stay").clicked() {
                            self.battery_back_confirm = false;
                        }
                        if ui.button("Stop and return").clicked() {
                            self.battery.stop_all();
                            self.battery_back_confirm = false;
                            self.view = AppView::MainMenu;
                        }
                    });
                });
        }
    }

    fn ui_battery_summary(
        &self,
        ui: &mut egui::Ui,
        report: &BatteryReport,
        latest_live: Option<&BatteryLiveSample>,
        runtime_accuracy: Option<&BatteryRuntimeAccuracy>,
    ) {
        if report.batteries.is_empty() {
            ui.colored_label(
                egui::Color32::YELLOW,
                "No installed battery was found. This diagnostic is intended for laptops.",
            );
            return;
        }

        let health_percent = battery_health_percent(report.primary_battery());
        let wear_percent = battery_wear_percent(report.primary_battery());
        let grade = battery_health_grade(health_percent);
        let battery = report.primary_battery();

        ui.heading("Battery Summary");
        ui.add_space(6.0);
        egui::Grid::new("battery_summary_grid")
            .num_columns(4)
            .spacing([18.0, 8.0])
            .show(ui, |ui| {
                battery_metric(
                    ui,
                    "Health",
                    format_battery_percent(health_percent),
                    grade.color(),
                );
                battery_metric(
                    ui,
                    "Wear",
                    format_battery_percent(wear_percent),
                    egui::Color32::WHITE,
                );
                battery_metric(
                    ui,
                    "Full charge",
                    format_capacity_mwh(
                        battery.and_then(|battery| battery.full_charge_capacity_mwh),
                    ),
                    egui::Color32::WHITE,
                );
                battery_metric(
                    ui,
                    "Cycle count",
                    battery
                        .and_then(|battery| battery.cycle_count)
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "N/A".to_owned()),
                    egui::Color32::WHITE,
                );
                ui.end_row();
                battery_metric(ui, "Grade", grade.label(), grade.color());
                battery_metric(
                    ui,
                    "Design capacity",
                    format_capacity_mwh(battery.and_then(|battery| battery.design_capacity_mwh)),
                    egui::Color32::WHITE,
                );
                battery_metric(
                    ui,
                    "Charge",
                    latest_live
                        .map(|sample| format_optional_percent(sample.percent))
                        .unwrap_or_else(|| "N/A".to_owned()),
                    egui::Color32::WHITE,
                );
                battery_metric(
                    ui,
                    "State",
                    latest_live
                        .map(|sample| sample.status.as_str())
                        .unwrap_or("N/A"),
                    egui::Color32::WHITE,
                );
                ui.end_row();
            });

        ui.add_space(10.0);
        if let Some(battery) = battery {
            ui.small(format!(
                "Battery: {} | Manufacturer: {} | Chemistry: {} | Serial: {}",
                battery.id.as_deref().unwrap_or("Unknown"),
                battery.manufacturer.as_deref().unwrap_or("Unknown"),
                battery.chemistry.as_deref().unwrap_or("Unknown"),
                battery.serial_number.as_deref().unwrap_or("Unknown")
            ));
        }
        if let Some(generated_at) = &report.generated_at {
            ui.small(format!("Report generated: {generated_at}"));
        }

        ui.separator();
        ui.heading("Live Behavior");
        egui::Grid::new("battery_live_grid")
            .num_columns(5)
            .spacing([18.0, 8.0])
            .show(ui, |ui| {
                battery_metric(
                    ui,
                    "Power",
                    latest_live
                        .and_then(|sample| sample.ac_connected)
                        .map(|ac| if ac { "AC connected" } else { "On battery" })
                        .unwrap_or("N/A"),
                    egui::Color32::WHITE,
                );
                battery_metric(
                    ui,
                    "Charge rate",
                    format_optional_watts(latest_live.and_then(|sample| sample.charge_rate_watts)),
                    egui::Color32::WHITE,
                );
                battery_metric(
                    ui,
                    "Discharge rate",
                    format_optional_watts(
                        latest_live.and_then(|sample| sample.discharge_rate_watts),
                    ),
                    egui::Color32::WHITE,
                );
                battery_metric(
                    ui,
                    "Windows runtime",
                    format_optional_minutes(
                        latest_live.and_then(|sample| sample.windows_runtime_minutes),
                    ),
                    egui::Color32::WHITE,
                );
                battery_metric(
                    ui,
                    "Remaining",
                    format_capacity_mwh(
                        latest_live.and_then(|sample| sample.remaining_capacity_mwh),
                    ),
                    egui::Color32::WHITE,
                );
                ui.end_row();
            });

        if let Some(accuracy) = runtime_accuracy {
            ui.small(format!(
                "Runtime estimate accuracy: {} ({:.1}% error, Windows {}, observed {})",
                accuracy.label,
                accuracy.error_percent,
                format_minutes(accuracy.windows_minutes),
                format_minutes(accuracy.observed_minutes)
            ));
        } else {
            ui.small("Runtime estimate accuracy: N/A until at least 3 minutes of discharge samples are collected.");
        }

        ui.separator();
        ui.heading("Warnings");
        let warnings = battery_report_warnings(report);
        if warnings.is_empty() {
            ui.colored_label(egui::Color32::GREEN, "No battery health warnings found.");
        } else {
            for warning in warnings {
                ui.colored_label(
                    warning.severity.color(),
                    format!("{}: {}", warning.title, warning.detail),
                );
            }
        }

        ui.separator();
        ui.heading("Capacity History");
        draw_battery_capacity_graph(ui, &report.capacity_history);

        ui.separator();
        ui.heading("Charging Behavior");
        draw_battery_live_graph(ui, &self.battery.live_samples);

        if !report.recent_usage.is_empty() {
            ui.separator();
            ui.heading("Recent Usage");
            egui::Grid::new("battery_recent_usage_grid")
                .striped(true)
                .num_columns(5)
                .show(ui, |ui| {
                    result_header(ui, "Time");
                    result_header(ui, "Power");
                    result_header(ui, "Charge");
                    result_header(ui, "Delta");
                    result_header(ui, "Full charge");
                    ui.end_row();
                    for point in report.recent_usage.iter().rev().take(8).rev() {
                        result_cell(ui, point.label.as_str());
                        result_cell(
                            ui,
                            point
                                .ac_connected
                                .map(|ac| if ac { "AC" } else { "Battery" })
                                .unwrap_or("N/A"),
                        );
                        result_cell(ui, format_capacity_mwh(point.charge_capacity_mwh));
                        result_cell(ui, format_capacity_mwh(point.discharge_mwh));
                        result_cell(ui, format_capacity_mwh(point.full_charge_capacity_mwh));
                        ui.end_row();
                    }
                });
        }
    }
}
