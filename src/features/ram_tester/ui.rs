impl BenchScopeApp {
    fn request_ram_back_to_menu(&mut self) {
        if self.ram.running {
            self.ram_back_confirm = true;
        } else {
            self.view = AppView::MainMenu;
        }
    }
    fn ui_ram_tester(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        egui::Panel::top("ram_top_panel").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui_large_back_button(ui).clicked() {
                    self.request_ram_back_to_menu();
                }
                self.ui_fullscreen_button(ui);
                ui.separator();
                ui.heading("RAM Tester");
                ui.separator();
                ui.label(&self.ram.status);
                if !self.ram.eta_text.is_empty() {
                    ui.separator();
                    ui.label(&self.ram.eta_text);
                }
            });
            ui.add(
                egui::ProgressBar::new(self.ram.progress)
                    .show_percentage()
                    .text(if self.ram.phase.is_empty() {
                        "RAM test progress"
                    } else {
                        self.ram.phase.as_str()
                    }),
            );
        });

        egui::Panel::left("ram_controls")
            .resizable(false)
            .min_size(350.0)
            .show_inside(ui, |ui| {
                let settings_height = (ui.available_height() - CONTROLS_ACTION_HEIGHT).max(120.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), settings_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("ram_controls_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.heading("Controls");
                                ui.add_space(8.0);

                                ui.label("System memory");
                                ui.small(format!(
                                    "Installed: {}; available: {}; load: {}%",
                                    format_bytes(self.ram.memory_info.total_physical_bytes),
                                    format_bytes(self.ram.memory_info.available_physical_bytes),
                                    self.ram.memory_info.memory_load_percent
                                ));
                                if ui.button("Refresh memory").clicked() && !self.ram.running {
                                    self.ram.refresh_memory_info();
                                }

                                ui.add_space(8.0);
                                ui.label("Test allocation");
                                ui.add_enabled_ui(!self.ram.running, |ui| {
                                    egui::ComboBox::from_id_salt("ram_allocation_combo")
                                        .selected_text(self.ram.allocation.label())
                                        .show_ui(ui, |ui| {
                                            for allocation in RamAllocation::ALL {
                                                ui.selectable_value(
                                                    &mut self.ram.allocation,
                                                    allocation,
                                                    allocation.label(),
                                                );
                                            }
                                        });
                                });
                                let planned_bytes = self.ram.planned_bytes();
                                ui.small(format!("Planned test buffer: {}", format_bytes(planned_bytes)));
                                ui.small(format!(
                                    "Full-test time budget: {}",
                                    format_elapsed(ram_time_budget_seconds(
                                        self.ram.memory_info.total_physical_bytes
                                    ))
                                ));
                                if planned_bytes < self.ram.allocation.requested_bytes().unwrap_or(planned_bytes)
                                {
                                    ui.colored_label(
                                        egui::Color32::YELLOW,
                                        "Requested size is clamped to leave OS headroom.",
                                    );
                                }
                                ui.colored_label(
                                    egui::Color32::YELLOW,
                                    "This writes a large committed buffer and can make the system sluggish.",
                                );
                                ui.small("For full physical-address coverage, use a boot-time RAM tester.");
                            });
                    },
                );
                ui.separator();
                ui.add_enabled_ui(!self.ram.running, |ui| {
                    if ui_start_action_button(ui, "Run RAM test").clicked() {
                        self.ram.start();
                    }
                });
                ui.add_enabled_ui(self.ram.running, |ui| {
                    if ui_cancel_action_button(ui, "Cancel RAM test").clicked() {
                        self.ram.cancel();
                    }
                });
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui_resizable_log_panel(
                ui,
                "ram_tester_log",
                "ram_tester_log_scroll",
                0.22,
                190.0,
                |ui| {
                    for line in &self.ram.log {
                        ui_log_line(ui, line);
                    }
                },
            );

            egui::CentralPanel::default().show_inside(ui, |ui| {
                ui.heading("RAM Results");
                ui.add_space(6.0);
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                            egui::Grid::new("ram_results_grid")
                                .striped(true)
                                .num_columns(11)
                                .show(ui, |ui| {
                                    result_header(ui, "Status");
                                    result_header(ui, "Tested");
                                    result_header(ui, "Installed");
                                    result_header(ui, "Available start");
                                    result_header(ui, "Elapsed");
                                    result_header(ui, "Budget");
                                    result_header(ui, "Phases");
                                    result_header(ui, "Checks");
                                    result_header(ui, "Errors");
                                    result_header(ui, "First failure");
                                    result_header(ui, "Notes");
                                    ui.end_row();

                                    for result in &self.ram.results {
                                        result_cell(ui, result.status.label());
                                        result_cell(ui, format_bytes(result.tested_bytes));
                                        result_cell(ui, format_bytes(result.installed_bytes));
                                        result_cell(
                                            ui,
                                            format_bytes(result.available_at_start_bytes),
                                        );
                                        result_cell(ui, format_ms(Some(result.elapsed_ms)));
                                        result_cell(ui, format_elapsed(result.budget_seconds));
                                        result_cell(
                                            ui,
                                            format!(
                                                "{}/{}",
                                                result.completed_phases, result.total_phases
                                            ),
                                        );
                                        result_cell(ui, result.checks.to_string());
                                        result_cell(ui, result.error_count.to_string());
                                        result_cell(ui, format_ram_first_failure(result));
                                        result_cell(ui, result.notes.join(", "));
                                        ui.end_row();
                                    }
                                });
                    });
            });
        });

        self.ui_sensor_window(&ctx);

        if self.ram_back_confirm {
            egui::Window::new("Return to main menu?")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label("A RAM test is currently running.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Stay").clicked() {
                            self.ram_back_confirm = false;
                        }
                        if ui.button("Cancel and return").clicked() {
                            self.ram.cancel();
                            self.ram_back_confirm = false;
                            self.view = AppView::MainMenu;
                        }
                    });
                });
        }
    }
}
