struct MainMenuItem {
    title: &'static str,
    description: &'static str,
    view: AppView,
    accent: egui::Color32,
}

impl BenchScopeApp {
    fn ui_main_menu(&mut self, ui: &mut egui::Ui) {
        let menu_items = [
            MainMenuItem {
                title: "Matrix CPU/GPU Benchmark",
                description: "Compare CPU throughput against the selected GPU compute adapter.",
                view: AppView::MatrixBenchmark,
                accent: egui::Color32::from_rgb(68, 184, 151),
            },
            MainMenuItem {
                title: "Matrix Stress Test",
                description: "Run repeat CPU or GPU matrix workloads for stability checks.",
                view: AppView::MatrixStressTest,
                accent: egui::Color32::from_rgb(214, 169, 74),
            },
            MainMenuItem {
                title: "Drive Benchmark",
                description: "Measure sequential and random disk performance on a target folder.",
                view: AppView::DriveBenchmark,
                accent: egui::Color32::from_rgb(91, 151, 224),
            },
            MainMenuItem {
                title: "SSD / HDD Health Checker",
                description: "Inspect SMART/NVMe health, warnings, temperature, and life estimates.",
                view: AppView::StorageHealth,
                accent: egui::Color32::from_rgb(224, 114, 91),
            },
            MainMenuItem {
                title: "RAM Tester",
                description: "Exercise memory with user-mode patterns and verification passes.",
                view: AppView::RamTester,
                accent: egui::Color32::from_rgb(166, 132, 224),
            },
            MainMenuItem {
                title: "Battery Health Diagnostic",
                description: "Review capacity, charge behavior, health estimates, and live readings.",
                view: AppView::BatteryHealthDiagnostic,
                accent: egui::Color32::from_rgb(112, 190, 105),
            },
            MainMenuItem {
                title: "Network Hardware Diagnostic",
                description: "Check adapter state, link quality, gateway, DNS, jitter, and packet loss.",
                view: AppView::NetworkDiagnostic,
                accent: egui::Color32::from_rgb(87, 188, 211),
            },
            MainMenuItem {
                title: "Device Information Viewer",
                description: "Browse system, BIOS, board, CPU, memory, disk, GPU, and driver details.",
                view: AppView::DeviceInfo,
                accent: egui::Color32::from_rgb(202, 129, 190),
            },
        ];
        let mut selected_view = None;

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::from_rgb(17, 19, 23)))
            .show_inside(ui, |ui| {
                let outer_margin = if ui.available_width() < 720.0 { 14 } else { 24 };
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(outer_margin, 18))
                    .show(ui, |ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            self.ui_fullscreen_button(ui);
                        });

                        egui::ScrollArea::vertical()
                            .id_salt("main_menu_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                let content_width = ui.available_width().min(1080.0).max(320.0);
                                let side_space =
                                    ((ui.available_width() - content_width) * 0.5).max(0.0);

                                ui.horizontal(|ui| {
                                    ui.add_space(side_space);
                                    ui.vertical(|ui| {
                                        ui.set_width(content_width);
                                        ui.add_space(if ui.available_height() < 620.0 {
                                            4.0
                                        } else {
                                            14.0
                                        });
                                        ui.vertical_centered(|ui| {
                                            ui.label(
                                                egui::RichText::new("BenchScope")
                                                    .size(38.0)
                                                    .strong()
                                                    .color(egui::Color32::from_rgb(
                                                        239, 242, 246,
                                                    )),
                                            );
                                            ui.add_space(4.0);
                                            ui.label(
                                                egui::RichText::new(
                                                    "Hardware benchmarks and diagnostics",
                                                )
                                                .size(17.0)
                                                .color(egui::Color32::from_rgb(167, 176, 190)),
                                            );
                                        });
                                        ui.add_space(22.0);

                                        let two_columns = content_width >= 760.0;
                                        let gap = if two_columns { 14.0 } else { 10.0 };
                                        if two_columns {
                                            let card_width = (content_width - gap) * 0.5;
                                            egui::Grid::new("main_menu_tools_grid")
                                                .num_columns(2)
                                                .spacing(egui::vec2(gap, gap))
                                                .show(ui, |ui| {
                                                    for row in menu_items.chunks(2) {
                                                        for item in row {
                                                            if ui_main_menu_card(
                                                                ui,
                                                                item,
                                                                egui::vec2(card_width, 94.0),
                                                            )
                                                            .clicked()
                                                            {
                                                                selected_view = Some(item.view);
                                                            }
                                                        }
                                                        if row.len() == 1 {
                                                            ui.allocate_space(egui::vec2(
                                                                card_width, 94.0,
                                                            ));
                                                        }
                                                        ui.end_row();
                                                    }
                                                });
                                        } else {
                                            for item in &menu_items {
                                                if ui_main_menu_card(
                                                    ui,
                                                    item,
                                                    egui::vec2(content_width, 90.0),
                                                )
                                                .clicked()
                                                {
                                                    selected_view = Some(item.view);
                                                }
                                                ui.add_space(gap);
                                            }
                                        }

                                        ui.add_space(16.0);
                                        ui_main_menu_system_footer(
                                            ui,
                                            &self.cpu_info,
                                            self.adapters.len(),
                                        );
                                        ui.add_space(10.0);
                                    });
                                });
                            });
                    });
            });

        if let Some(view) = selected_view {
            self.open_main_menu_view(view);
        }
    }

    fn open_main_menu_view(&mut self, view: AppView) {
        self.view = view;
        match view {
            AppView::StorageHealth => {
                if self.storage_health.snapshot.is_none() && !self.storage_health.running {
                    self.storage_health.start_refresh();
                }
            }
            AppView::BatteryHealthDiagnostic => {
                if self.battery.latest_report.is_none() && !self.battery.scanning {
                    self.battery.start_scan();
                }
            }
            AppView::DeviceInfo => {
                if self.device_info.snapshot.is_none() && !self.device_info.running {
                    self.device_info.start_refresh(self.adapters.clone());
                }
            }
            _ => {}
        }
    }
}

fn ui_main_menu_card(
    ui: &mut egui::Ui,
    item: &MainMenuItem,
    size: egui::Vec2,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.hovered() {
        ui.output_mut(|output| output.cursor_icon = egui::CursorIcon::PointingHand);
    }

    if ui.is_rect_visible(rect) {
        let fill = if response.hovered() {
            egui::Color32::from_rgb(33, 37, 44)
        } else {
            egui::Color32::from_rgb(25, 28, 34)
        };
        let stroke = if response.hovered() {
            egui::Stroke::new(1.4, item.accent)
        } else {
            egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 54, 64))
        };
        ui.painter().rect(
            rect,
            egui::CornerRadius::same(8),
            fill,
            stroke,
            egui::StrokeKind::Inside,
        );

        let accent_rect = egui::Rect::from_min_max(
            rect.left_top(),
            egui::pos2(rect.left() + 5.0, rect.bottom()),
        );
        ui.painter().rect_filled(
            accent_rect,
            egui::CornerRadius {
                nw: 8,
                ne: 0,
                sw: 8,
                se: 0,
            },
            item.accent,
        );

        let text_rect = rect.shrink2(egui::vec2(20.0, 14.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(text_rect), |ui| {
            ui.set_width(text_rect.width());
            ui.add(
                egui::Label::new(
                    egui::RichText::new(item.title)
                        .size(18.0)
                        .strong()
                        .color(egui::Color32::from_rgb(241, 244, 248)),
                )
                .wrap(),
            );
            ui.add_space(5.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(item.description)
                        .size(14.5)
                        .color(egui::Color32::from_rgb(166, 176, 190)),
                )
                .wrap(),
            );
        });
    }

    response.on_hover_text(item.description)
}

fn ui_main_menu_system_footer(ui: &mut egui::Ui, cpu_info: &CpuInfo, adapter_count: usize) {
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(23, 26, 31))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(47, 53, 62),
        ))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new("System")
                        .strong()
                        .color(egui::Color32::from_rgb(224, 228, 234)),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("CPU: {}", cpu_info.label()))
                        .size(14.5)
                        .color(egui::Color32::from_rgb(176, 185, 198)),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("GPU adapters detected: {adapter_count}"))
                        .size(14.5)
                        .color(egui::Color32::from_rgb(176, 185, 198)),
                );
            });
        });
}
