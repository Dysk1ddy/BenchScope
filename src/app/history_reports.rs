impl BenchScopeApp {
    fn capture_app_environment_history(&mut self) {
        let snapshot = self.sensors.latest();
        let event = history_app_environment_event(
            &self.adapters,
            &self.cpu_info,
            &snapshot,
            &self.history.root_dir,
        );
        self.history.append_event(event);
        self.history.append_event(history_event_from_sensor_snapshot(&snapshot));
    }

    #[allow(clippy::too_many_arguments)]
    fn capture_history_after_poll(
        &mut self,
        matrix_result_count_before: usize,
        drive_result_count_before: usize,
        gpu_memory_result_count_before: usize,
        ram_result_count_before: usize,
        ai_training_result_count_before: usize,
        storage_health_was_running: bool,
        storage_health_snapshot_before: bool,
        storage_health_scan_before: bool,
        storage_health_benchmark_count_before: usize,
        battery_was_scanning: bool,
        network_was_active: bool,
        device_info_was_running: bool,
    ) {
        for result in self.results.iter().skip(matrix_result_count_before) {
            self.history
                .append_event(history_event_from_matrix_result(result));
        }

        let drive_label = self.drive.selected_drive_label();
        for result in self.drive.results.iter().skip(drive_result_count_before) {
            self.history
                .append_event(history_event_from_drive_result(result, &drive_label));
        }

        for result in self
            .gpu_memory
            .results
            .iter()
            .skip(gpu_memory_result_count_before)
        {
            self.history
                .append_event(history_event_from_gpu_memory_result(result));
        }

        for result in self.ram.results.iter().skip(ram_result_count_before) {
            self.history.append_event(history_event_from_ram_result(result));
        }

        for result in self
            .ai_training
            .results
            .iter()
            .skip(ai_training_result_count_before)
        {
            self.history
                .append_event(history_event_from_ai_training_result(result));
        }

        if storage_health_was_running && !self.storage_health.running {
            let snapshot_changed = !storage_health_snapshot_before && self.storage_health.snapshot.is_some();
            let scan_changed = !storage_health_scan_before && self.storage_health.scan_result.is_some();
            let benchmark_changed =
                self.storage_health.benchmark_results.len() > storage_health_benchmark_count_before;
            if snapshot_changed || scan_changed || benchmark_changed {
                if let Some(event) = history_event_from_storage_health_state(&self.storage_health) {
                    self.history.append_event(event);
                }
            }
            let health_drive_label = self.storage_health.selected_drive_label();
            for result in self
                .storage_health
                .benchmark_results
                .iter()
                .skip(storage_health_benchmark_count_before)
            {
                self.history
                    .append_event(history_event_from_drive_result(result, &health_drive_label));
            }
        }

        if battery_was_scanning && !self.battery.scanning {
            if let Some(report) = &self.battery.latest_report {
                self.history
                    .append_event(history_event_from_battery_report(report));
            }
        }

        let network_is_active =
            self.network.running || self.network.monitoring || self.network.adapter_refresh_running;
        if network_was_active && !network_is_active {
            self.history
                .append_event(history_event_from_network_state(&self.network));
        }

        if device_info_was_running && !self.device_info.running {
            if let Some(snapshot) = &self.device_info.snapshot {
                self.history
                    .append_event(history_event_from_device_info(snapshot));
            }
        }
    }

    fn capture_current_history_snapshot(&mut self) {
        self.capture_app_environment_history();
        if let Some(result) = self.results.last() {
            self.history
                .append_event(history_event_from_matrix_result(result));
        }
        if let Some(progress) = &self.repeat_progress {
            self.history
                .append_event(history_event_from_repeat_progress(progress));
        }
        let drive_label = self.drive.selected_drive_label();
        if let Some(result) = self.drive.results.last() {
            self.history
                .append_event(history_event_from_drive_result(result, &drive_label));
        }
        if let Some(result) = self.gpu_memory.results.last() {
            self.history
                .append_event(history_event_from_gpu_memory_result(result));
        }
        if let Some(result) = self.ai_training.results.last() {
            self.history
                .append_event(history_event_from_ai_training_result(result));
        }
        if let Some(result) = self.ram.results.last() {
            self.history.append_event(history_event_from_ram_result(result));
        }
        if let Some(report) = &self.battery.latest_report {
            self.history
                .append_event(history_event_from_battery_report(report));
        }
        self.history
            .append_event(history_event_from_network_state(&self.network));
        if let Some(event) = history_event_from_storage_health_state(&self.storage_health) {
            self.history.append_event(event);
        }
        if let Some(snapshot) = &self.device_info.snapshot {
            self.history
                .append_event(history_event_from_device_info(snapshot));
        }
        self.history
            .append_event(history_event_from_sensor_snapshot(&self.sensors.latest()));
        self.history.last_status = "Current BenchScope snapshot saved to history".to_owned();
    }

    fn export_support_bundle_now(&mut self) {
        let mut reports = Vec::new();
        reports.push((
            "matrix-benchmark.md".to_owned(),
            render_matrix_benchmark_report(&self.results),
        ));
        reports.push((
            "drive-benchmark.md".to_owned(),
            render_drive_benchmark_report(&self.drive.results, &self.drive.selected_drive_label()),
        ));
        reports.push((
            "gpu-memory-benchmark.md".to_owned(),
            render_gpu_memory_benchmark_report(&self.gpu_memory.results),
        ));
        reports.push((
            "ai-training-benchmark.md".to_owned(),
            render_ai_training_benchmark_report(&self.ai_training.results),
        ));
        reports.push(("ram-test.md".to_owned(), render_ram_test_report(&self.ram.results)));
        reports.push((
            "battery-diagnostic.md".to_owned(),
            render_battery_diagnostic_report(self.battery.latest_report.as_ref()),
        ));
        reports.push((
            "network-diagnostic.md".to_owned(),
            network_diagnostic_report_markdown(&self.network),
        ));
        let storage_report = self
            .storage_health
            .snapshot
            .as_ref()
            .map(|snapshot| {
                render_storage_health_report(
                    snapshot,
                    self.storage_health.scan_result.as_ref(),
                    &self.storage_health.benchmark_results,
                )
            })
            .unwrap_or_else(|| {
                "# BenchScope Storage Health Report\n\nNo storage health snapshot has completed in this session.\n".to_owned()
            });
        reports.push(("storage-health.md".to_owned(), storage_report));
        let device_report = self
            .device_info
            .snapshot
            .as_ref()
            .map(render_device_info_report)
            .unwrap_or_else(|| {
                "# BenchScope Device Information Report\n\nNo device inventory snapshot has completed in this session.\n".to_owned()
            });
        reports.push(("device-info.md".to_owned(), device_report));
        reports.push((
            "sensor-provider-status.md".to_owned(),
            render_sensor_provider_report(&self.sensors.latest()),
        ));

        let session_log = self.combined_session_log();
        match export_support_bundle(&mut self.history, reports, &session_log) {
            Ok(path) => {
                self.status = format!("Support bundle exported: {}", path.display());
                self.log(self.status.clone());
                self.history.confirm_bundle_export = false;
            }
            Err(err) => {
                self.status = format!("Support bundle export failed: {err:#}");
                self.history.last_error = Some(self.status.clone());
                self.log(self.status.clone());
            }
        }
    }

    fn combined_session_log(&self) -> Vec<String> {
        let mut lines = Vec::new();
        lines.push(format!("BenchScope {}", env!("CARGO_PKG_VERSION")));
        lines.push(format!("Status: {}", self.status));
        append_prefixed_lines(&mut lines, "App", &self.log);
        append_prefixed_lines(&mut lines, "Drive", &self.drive.log);
        append_prefixed_lines(&mut lines, "Storage", &self.storage_health.log);
        append_prefixed_lines(&mut lines, "RAM", &self.ram.log);
        append_prefixed_lines(&mut lines, "Battery", &self.battery.log);
        append_prefixed_lines(&mut lines, "Network", &self.network.log);
        append_prefixed_lines(&mut lines, "DeviceInfo", &self.device_info.log);
        append_prefixed_lines(&mut lines, "AITraining", &self.ai_training.log);
        append_prefixed_lines(&mut lines, "GPUMemory", &self.gpu_memory.log);
        lines
    }

    fn ui_history_reports(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("history_reports_top_panel").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui_large_back_button(ui).clicked() {
                    self.view = AppView::MainMenu;
                }
                self.ui_fullscreen_button(ui);
                ui.separator();
                ui.heading("History & Reports");
                ui.separator();
                ui.label(&self.history.last_status);
            });
        });

        egui::Panel::left("history_reports_controls")
            .resizable(false)
            .min_size(330.0)
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("history_reports_controls_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.heading("Controls");
                        ui.add_space(8.0);
                        if ui.button("Save current snapshot").clicked() {
                            self.capture_current_history_snapshot();
                        }
                        if ui.button("Pin latest as baseline").clicked() {
                            self.history.pin_latest_selected();
                        }
                        if ui.button("Export support bundle").clicked() {
                            self.history.confirm_bundle_export = true;
                        }
                        if self.history.confirm_bundle_export {
                            ui.separator();
                            ui.label(egui::RichText::new("Privacy").strong());
                            ui.small("Default export redacts serials, MAC/IP addresses, Wi-Fi names, host/user paths, and hardware IDs.");
                            ui.checkbox(
                                &mut self.history.redaction.include_sensitive_ids,
                                "Include sensitive hardware IDs",
                            );
                            ui.checkbox(
                                &mut self.history.redaction.include_network_addresses,
                                "Include IP/MAC/network addresses",
                            );
                            ui.checkbox(
                                &mut self.history.redaction.include_wifi_names,
                                "Include Wi-Fi network names",
                            );
                            ui.checkbox(
                                &mut self.history.redaction.include_local_paths,
                                "Include local user paths",
                            );
                            ui.horizontal(|ui| {
                                if ui.button("Create bundle").clicked() {
                                    self.export_support_bundle_now();
                                }
                                if ui.button("Cancel").clicked() {
                                    self.history.confirm_bundle_export = false;
                                }
                            });
                        }

                        ui.separator();
                        ui.label(egui::RichText::new("Categories").strong());
                        for category in history_categories() {
                            ui.selectable_value(
                                &mut self.history.selected_category,
                                category.id.to_owned(),
                                category.label,
                            );
                        }

                        ui.separator();
                        ui.label(egui::RichText::new("Storage").strong());
                        ui.small(format!("Root: {}", self.history.root_dir.display()));
                        ui.small(format!("Events: {}", self.history.events.len()));
                        ui.small(format!("Pinned baselines: {}", self.history.baselines.pinned.len()));
                        if let Some(path) = &self.history.last_bundle_path {
                            ui.small(format!("Last bundle: {}", path.display()));
                        }
                        if let Some(error) = &self.history.last_error {
                            ui.colored_label(egui::Color32::YELLOW, error);
                        }

                        ui.separator();
                        if self.history.confirm_delete {
                            ui.colored_label(egui::Color32::YELLOW, "Delete all saved history and baselines?");
                            ui.horizontal(|ui| {
                                if ui.button("Delete").clicked() {
                                    self.history.clear_history();
                                }
                                if ui.button("Cancel").clicked() {
                                    self.history.confirm_delete = false;
                                }
                            });
                        } else if ui.button("Delete history").clicked() {
                            self.history.confirm_delete = true;
                        }
                    });
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("history_reports_content_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    self.ui_history_overview(ui);
                    ui.separator();
                    self.ui_history_comparisons(ui);
                    ui.separator();
                    self.ui_history_hardware_changes(ui);
                    ui.separator();
                    self.ui_history_recent_events(ui);
                });
        });
    }

    fn ui_history_overview(&mut self, ui: &mut egui::Ui) {
        ui.heading("Overview");
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("History root: {}", self.history.root_dir.display()));
            ui.separator();
            ui.label(format!("Events: {}", self.history.events.len()));
            ui.separator();
            ui.label(format!("Pinned: {}", self.history.baselines.pinned.len()));
        });
        ui.add_space(8.0);
        egui::Grid::new("history_latest_grid")
            .striped(true)
            .num_columns(4)
            .show(ui, |ui| {
                result_header(ui, "Category");
                result_header(ui, "Latest");
                result_header(ui, "Summary");
                result_header(ui, "Captured");
                ui.end_row();
                for category in history_categories() {
                    if category.id == "all" {
                        continue;
                    }
                    if let Some(event) = self.history.latest_event_for_category(category.id) {
                        result_cell(ui, category.label);
                        result_cell_hover(ui, &event.title, &event.profile_key);
                        result_cell(ui, &event.summary);
                        result_cell(ui, format!("{}s", event.captured_at_unix_ms / 1000));
                        ui.end_row();
                    }
                }
            });
    }

    fn ui_history_comparisons(&self, ui: &mut egui::Ui) {
        ui.heading("Baseline Comparisons");
        let comparisons = self.history.comparisons_for_selected();
        if comparisons.is_empty() {
            ui.label("Run a tool at least twice with the same profile, or pin a baseline, to see deltas.");
            return;
        }
        for comparison in comparisons {
            ui.collapsing(
                format!("{}: {}", comparison.category, comparison.current_title),
                |ui| {
                    ui.small(format!("Baseline: {}", comparison.baseline_title));
                    egui::Grid::new(("history_comparison_grid", &comparison.profile_key))
                        .striped(true)
                        .num_columns(6)
                        .show(ui, |ui| {
                            result_header(ui, "Metric");
                            result_header(ui, "Baseline");
                            result_header(ui, "Current");
                            result_header(ui, "Delta");
                            result_header(ui, "Direction");
                            result_header(ui, "Severity");
                            ui.end_row();
                            for delta in &comparison.deltas {
                                result_cell(ui, &delta.metric);
                                result_cell(ui, &delta.baseline);
                                result_cell(ui, &delta.current);
                                result_cell(ui, &delta.delta);
                                result_cell(ui, &delta.direction);
                                result_cell(ui, &delta.severity);
                                ui.end_row();
                            }
                        });
                    for note in &comparison.notes {
                        ui.small(note);
                    }
                },
            );
        }
    }

    fn ui_history_hardware_changes(&self, ui: &mut egui::Ui) {
        ui.heading("Hardware & Driver Changes");
        let changes = self.history.hardware_changes();
        if changes.is_empty() {
            ui.label("No hardware or driver changes were detected between the last two device inventory snapshots.");
            return;
        }
        egui::Grid::new("history_hardware_changes_grid")
            .striped(true)
            .num_columns(3)
            .show(ui, |ui| {
                result_header(ui, "Field");
                result_header(ui, "Previous");
                result_header(ui, "Current");
                ui.end_row();
                for change in changes {
                    result_cell(ui, change.field);
                    result_cell(ui, change.previous);
                    result_cell(ui, change.current);
                    ui.end_row();
                }
            });
    }

    fn ui_history_recent_events(&self, ui: &mut egui::Ui) {
        ui.heading("Recent Events");
        let events = self.history.selected_events();
        if events.is_empty() {
            ui.label("No saved history events yet.");
            return;
        }
        egui::Grid::new("history_recent_events_grid")
            .striped(true)
            .num_columns(5)
            .show(ui, |ui| {
                result_header(ui, "Time");
                result_header(ui, "Category");
                result_header(ui, "Title");
                result_header(ui, "Summary");
                result_header(ui, "Warnings");
                ui.end_row();
                for event in events.iter().rev() {
                    result_cell(ui, format!("{}s", event.captured_at_unix_ms / 1000));
                    result_cell(ui, &event.category);
                    result_cell_hover(ui, &event.title, &event.profile_key);
                    result_cell(ui, &event.summary);
                    result_cell(ui, event.warnings.len().to_string());
                    ui.end_row();
                }
            });
    }
}

fn append_prefixed_lines(target: &mut Vec<String>, prefix: &str, lines: &[String]) {
    for line in lines {
        target.push(format!("[{prefix}] {line}"));
    }
}
