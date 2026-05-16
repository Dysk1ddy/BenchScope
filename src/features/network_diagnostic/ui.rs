impl BenchScopeApp {
    fn request_network_back_to_menu(&mut self) {
        if self.network.running || self.network.monitoring {
            self.network_back_confirm = true;
        } else {
            self.view = AppView::MainMenu;
        }
    }
    fn ui_network_diagnostic(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        egui::Panel::top("network_top_panel").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui_large_back_button(ui).clicked() {
                    self.request_network_back_to_menu();
                }
                self.ui_fullscreen_button(ui);
                ui.separator();
                ui.heading("Network Hardware Diagnostic");
                ui.separator();
                ui.label(&self.network.status);
            });
            ui.add(
                egui::ProgressBar::new(self.network.progress)
                    .show_percentage()
                    .text(self.network.current_step.as_str()),
            );
        });

        egui::Panel::left("network_controls")
            .resizable(false)
            .min_size(370.0)
            .show_inside(ui, |ui| {
                let settings_height = (ui.available_height() - CONTROLS_ACTION_HEIGHT).max(120.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), settings_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("network_controls_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.heading("Controls");
                                ui.add_space(8.0);

                                ui.label("Adapter");
                                ui.add_enabled_ui(
                                    !self.network.running && !self.network.monitoring,
                                    |ui| {
                                        let visible_indices =
                                            self.network.visible_adapter_indices();
                                        let mut selected = self.network.selected_adapter;
                                        egui::ComboBox::from_id_salt("network_adapter_combo")
                                            .selected_text(self.network.selected_adapter_label())
                                            .width(330.0)
                                            .show_ui(ui, |ui| {
                                                for index in visible_indices {
                                                    if let Some(adapter) =
                                                        self.network.adapters.get(index)
                                                    {
                                                        ui.selectable_value(
                                                            &mut selected,
                                                            index,
                                                            adapter.menu_label(),
                                                        );
                                                    }
                                                }
                                            });
                                        if selected != self.network.selected_adapter {
                                            self.network.selected_adapter = selected;
                                        }
                                        ui.checkbox(
                                            &mut self.network.include_virtual,
                                            "Include virtual adapters",
                                        );
                                        if ui.button("Refresh adapters").clicked() {
                                            self.network.refresh_adapters();
                                        }
                                    },
                                );

                                if let Some(adapter) = self.network.selected_adapter().cloned() {
                                    ui.separator();
                                    ui.label(egui::RichText::new("Adapter Details").strong());
                                    ui.small(format!("Type: {}", adapter.kind.label()));
                                    ui.small(format!(
                                        "State: {}",
                                        if adapter.connected {
                                            "Connected"
                                        } else {
                                            "Disconnected"
                                        }
                                    ));
                                    ui.small(format!(
                                        "Link speed: {}",
                                        format_link_speed(adapter.link_speed_bps)
                                    ));
                                    ui.small(format!(
                                        "IPv4: {}",
                                        empty_list_label(&adapter.ipv4_addresses)
                                    ));
                                    ui.small(format!(
                                        "IPv6: {}",
                                        empty_list_label(&adapter.ipv6_addresses)
                                    ));
                                    ui.small(format!(
                                        "Gateway: {}",
                                        empty_list_label(&adapter.gateways)
                                    ));
                                    ui.small(format!(
                                        "DNS: {}",
                                        empty_list_label(&adapter.dns_servers)
                                    ));
                                    ui.small(format!(
                                        "MAC: {}",
                                        adapter.mac_address.as_deref().unwrap_or("N/A")
                                    ));
                                    if let Some(driver) = &adapter.driver {
                                        ui.small(format!(
                                            "Driver: {} {}",
                                            driver.provider.as_deref().unwrap_or("N/A"),
                                            driver.version.as_deref().unwrap_or("N/A")
                                        ));
                                        ui.small(format!(
                                            "Driver date: {}",
                                            driver.date.as_deref().unwrap_or("N/A")
                                        ));
                                    }
                                    if let Some(counters) = &adapter.counters {
                                        ui.separator();
                                        ui.label(egui::RichText::new("Counters").strong());
                                        ui.small(format!(
                                            "Bytes Rx/Tx: {} / {}",
                                            format_optional_bytes(counters.bytes_received),
                                            format_optional_bytes(counters.bytes_sent)
                                        ));
                                        ui.small(format!(
                                            "Packets Rx/Tx: {} / {}",
                                            format_optional_count(counters.packets_received),
                                            format_optional_count(counters.packets_sent)
                                        ));
                                        ui.small(format!(
                                            "Errors in/out: {} / {}; discards in/out: {} / {}",
                                            format_optional_count(counters.inbound_errors),
                                            format_optional_count(counters.outbound_errors),
                                            format_optional_count(counters.inbound_discards),
                                            format_optional_count(counters.outbound_discards)
                                        ));
                                    }
                                    for note in &adapter.provider_notes {
                                        ui.small(format!("Provider note: {note}"));
                                    }
                                    if let Some(wifi) = &adapter.wifi {
                                        ui.separator();
                                        ui.label(egui::RichText::new("Wi-Fi").strong());
                                        ui.small(format!(
                                            "SSID: {}",
                                            wifi.ssid.as_deref().unwrap_or("N/A")
                                        ));
                                        ui.small(format!(
                                            "Signal: {}",
                                            wifi.signal_quality_percent
                                                .map(|value| format!("{value}%"))
                                                .unwrap_or_else(|| "N/A".to_owned())
                                        ));
                                        ui.small(format!(
                                            "PHY: {}",
                                            wifi.phy_type.as_deref().unwrap_or("N/A")
                                        ));
                                        ui.small(format!(
                                            "Channel: {}",
                                            wifi.channel
                                                .map(|value| value.to_string())
                                                .unwrap_or_else(|| "N/A".to_owned())
                                        ));
                                        ui.small(format!(
                                            "Rx/Tx: {} / {}",
                                            format_link_speed(wifi.rx_link_speed_bps),
                                            format_link_speed(wifi.tx_link_speed_bps)
                                        ));
                                    }
                                } else {
                                    ui.colored_label(
                                        egui::Color32::YELLOW,
                                        "No adapter is selected.",
                                    );
                                }
                            });
                    },
                );

                ui.separator();
                ui.add_enabled_ui(!self.network.running && !self.network.monitoring, |ui| {
                    if ui.button("Run quick diagnosis").clicked() {
                        self.network.start_quick_diagnosis();
                    }
                    if ui.button("Start continuous monitor").clicked() {
                        self.network.start_monitor();
                    }
                    if ui.button("Export report").clicked() {
                        self.network.export_report();
                    }
                });
                ui.add_enabled_ui(self.network.running || self.network.monitoring, |ui| {
                    let label = if self.network.running {
                        "Cancel diagnosis"
                    } else {
                        "Stop monitor"
                    };
                    if ui.button(label).clicked() {
                        self.network.stop();
                    }
                });
                if let Some(path) = &self.network.last_report_path {
                    ui.small(format!("Last report: {}", path.display()));
                }
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            let available_height = ui.available_height();
            let (content_height, log_height) =
                panel_content_log_heights(available_height, 0.18, 150.0);

            ui.horizontal(|ui| {
                ui.heading("Network Findings");
                if let Some(adapter) = self.network.selected_adapter() {
                    ui.separator();
                    ui.colored_label(
                        adapter.status.color(),
                        egui::RichText::new(adapter.status.label()).strong(),
                    );
                }
            });
            ui.add_space(6.0);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), content_height),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("Findings").strong());
                            if self.network.findings.is_empty() {
                                ui.label("Run a quick diagnosis or start the monitor to populate findings.");
                            } else {
                                for finding in &self.network.findings {
                                    ui.colored_label(
                                        finding.severity.color(),
                                        format!("[{}] {}", finding.severity.label(), finding.title),
                                    );
                                    ui.label(&finding.detail);
                                    if let Some(action) = &finding.recommended_action {
                                        ui.small(format!("Action: {action}"));
                                    }
                                    ui.add_space(6.0);
                                }
                            }

                            ui.separator();
                            ui.label(egui::RichText::new("Packet Loss and Latency").strong());
                            egui::Grid::new("network_probe_grid")
                                .striped(true)
                                .num_columns(9)
                                .show(ui, |ui| {
                                    result_header(ui, "Target");
                                    result_header(ui, "Probe");
                                    result_header(ui, "Sent");
                                    result_header(ui, "Recv");
                                    result_header(ui, "Loss");
                                    result_header(ui, "Min");
                                    result_header(ui, "Avg");
                                    result_header(ui, "Max");
                                    result_header(ui, "Jitter");
                                    ui.end_row();
                                    for probe in &self.network.probe_results {
                                        result_cell(ui, format!("{} ({})", probe.target_label, probe.target));
                                        result_cell(ui, probe.probe_kind.label());
                                        result_cell(ui, probe.sent.to_string());
                                        result_cell(ui, probe.received.to_string());
                                        result_cell(ui, format_loss_percent(probe.loss_percent));
                                        result_cell(ui, format_optional_latency(probe.min_latency_ms));
                                        result_cell(ui, format_optional_latency(probe.avg_latency_ms));
                                        result_cell(ui, format_optional_latency(probe.max_latency_ms));
                                        result_cell(ui, format_optional_latency(probe.jitter_ms));
                                        ui.end_row();
                                    }
                                });

                            ui.separator();
                            ui.label(egui::RichText::new("Wi-Fi Signal / Link History").strong());
                            if self.network.signal_history.is_empty() {
                                ui.label("Start continuous monitor to collect signal and link samples.");
                            } else {
                                egui::Grid::new("network_signal_history_grid")
                                    .striped(true)
                                    .num_columns(4)
                                    .show(ui, |ui| {
                                        result_header(ui, "Time");
                                        result_header(ui, "Signal");
                                        result_header(ui, "Link speed");
                                        result_header(ui, "Gateway latency");
                                        ui.end_row();
                                        for sample in self.network.signal_history.iter().rev().take(20) {
                                            result_cell(ui, sample.timestamp_s.to_string());
                                            result_cell(
                                                ui,
                                                sample
                                                    .signal_percent
                                                    .map(|value| format!("{value}%"))
                                                    .unwrap_or_else(|| "N/A".to_owned()),
                                            );
                                            result_cell(ui, format_link_speed(sample.link_speed_bps));
                                            result_cell(
                                                ui,
                                                format_optional_latency(sample.gateway_latency_ms),
                                            );
                                            ui.end_row();
                                        }
                                    });
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
                    for line in &self.network.log {
                        ui_log_line(ui, line);
                    }
                });
        });

        if self.network_back_confirm {
            egui::Window::new("Return to main menu?")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label("A network diagnostic is currently running.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Stay").clicked() {
                            self.network_back_confirm = false;
                        }
                        if ui.button("Stop and return").clicked() {
                            self.network.stop();
                            self.network_back_confirm = false;
                            self.view = AppView::MainMenu;
                        }
                    });
                });
        }
    }
}
