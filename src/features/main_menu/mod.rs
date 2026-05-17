#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuCategory {
    Cpu,
    Gpu,
    Ram,
    Storage,
    Drivers,
    Io,
    Misc,
}

impl MenuCategory {
    const ALL: [Self; 7] = [
        Self::Cpu,
        Self::Gpu,
        Self::Ram,
        Self::Storage,
        Self::Drivers,
        Self::Io,
        Self::Misc,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Gpu => "GPU",
            Self::Ram => "RAM",
            Self::Storage => "Storage",
            Self::Drivers => "Drivers",
            Self::Io => "I/O",
            Self::Misc => "Misc",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Cpu => "Processor benchmarks, stress runs, and CPU inventory.",
            Self::Gpu => {
                "GPU compute, VRAM bandwidth, training workloads, and adapter inventory."
            }
            Self::Ram => "Memory integrity tests and installed module details.",
            Self::Storage => "Drive speed, SMART/NVMe health, and disk inventory.",
            Self::Drivers => "Signed driver records, provider metadata, and device coverage.",
            Self::Io => "Disk and network diagnostics for hardware I/O paths.",
            Self::Misc => "Battery health and whole-system information tools.",
        }
    }

    fn accent(self) -> egui::Color32 {
        match self {
            Self::Cpu => egui::Color32::from_rgb(68, 184, 151),
            Self::Gpu => egui::Color32::from_rgb(118, 203, 168),
            Self::Ram => egui::Color32::from_rgb(166, 132, 224),
            Self::Storage => egui::Color32::from_rgb(91, 151, 224),
            Self::Drivers => egui::Color32::from_rgb(202, 129, 190),
            Self::Io => egui::Color32::from_rgb(87, 188, 211),
            Self::Misc => egui::Color32::from_rgb(112, 190, 105),
        }
    }

    fn scroll_id(self) -> &'static str {
        match self {
            Self::Cpu => "main_menu_scroll_cpu",
            Self::Gpu => "main_menu_scroll_gpu",
            Self::Ram => "main_menu_scroll_ram",
            Self::Storage => "main_menu_scroll_storage",
            Self::Drivers => "main_menu_scroll_drivers",
            Self::Io => "main_menu_scroll_io",
            Self::Misc => "main_menu_scroll_misc",
        }
    }
}

#[derive(Clone, Copy)]
struct MainMenuCategoryItem {
    category: MenuCategory,
    description: &'static str,
    accent: egui::Color32,
}

#[derive(Clone, Copy)]
struct MainMenuItem {
    title: &'static str,
    description: &'static str,
    view: AppView,
    accent: egui::Color32,
    categories: &'static [MenuCategory],
}

#[derive(Clone, Copy)]
struct MainMenuCard {
    title: &'static str,
    description: &'static str,
    accent: egui::Color32,
}

impl MainMenuCategoryItem {
    fn card(self) -> MainMenuCard {
        MainMenuCard {
            title: self.category.label(),
            description: self.description,
            accent: self.accent,
        }
    }
}

impl MainMenuItem {
    fn card(self) -> MainMenuCard {
        MainMenuCard {
            title: self.title,
            description: self.description,
            accent: self.accent,
        }
    }

    fn belongs_to(self, category: MenuCategory) -> bool {
        self.categories.contains(&category)
    }
}

impl BenchScopeApp {
    fn ui_main_menu(&mut self, ui: &mut egui::Ui) {
        let selected_category = self.main_menu_category;
        let mut next_category = selected_category;
        let mut selected_view = None;
        if selected_category.is_some()
            && ui
                .ctx()
                .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            next_category = None;
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::from_rgb(17, 19, 23)))
            .show_inside(ui, |ui| {
                let outer_margin = if ui.available_width() < 720.0 { 14 } else { 24 };
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(outer_margin, 18))
                    .show(ui, |ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            self.ui_fullscreen_button(ui);
                            if selected_category.is_some() {
                                if ui
                                    .add_sized(
                                        [156.0, 38.0],
                                        egui::Button::new(
                                            egui::RichText::new("Categories").size(17.0),
                                        ),
                                    )
                                    .on_hover_text("Return to the category list")
                                    .clicked()
                                {
                                    next_category = None;
                                }
                            }
                        });

                        egui::ScrollArea::both()
                            .id_salt(main_menu_scroll_id(selected_category))
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let content_width = ui.available_width().max(1.0);
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
                                            .color(egui::Color32::from_rgb(239, 242, 246)),
                                    );
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(main_menu_subtitle(selected_category))
                                        .size(17.0)
                                        .color(egui::Color32::from_rgb(167, 176, 190)),
                                    );
                                });
                                ui.add_space(22.0);

                                if let Some(category) = selected_category {
                                    let menu_items = main_menu_items_for_category(category);
                                    let cards = menu_items
                                        .iter()
                                        .map(|item| item.card())
                                        .collect::<Vec<_>>();
                                    if let Some(index) =
                                        ui_main_menu_card_grid(ui, content_width, &cards)
                                    {
                                        selected_view = Some(menu_items[index].view);
                                    }
                                } else {
                                    let category_items = main_menu_category_items();
                                    let cards = category_items
                                        .iter()
                                        .map(|item| item.card())
                                        .collect::<Vec<_>>();
                                    if let Some(index) =
                                        ui_main_menu_card_grid(ui, content_width, &cards)
                                    {
                                        next_category = Some(category_items[index].category);
                                    }
                                }

                                ui.add_space(6.0);
                                ui_main_menu_system_footer(
                                    ui,
                                    &self.cpu_info,
                                    self.adapters.len(),
                                );
                                ui.add_space(10.0);
                            });
                    });
            });

        self.main_menu_category = next_category;
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

fn main_menu_category_items() -> [MainMenuCategoryItem; 7] {
    MenuCategory::ALL.map(|category| MainMenuCategoryItem {
        category,
        description: category.description(),
        accent: category.accent(),
    })
}

fn main_menu_tool_items() -> [MainMenuItem; 10] {
    [
        MainMenuItem {
            title: "Matrix CPU/GPU Benchmark",
            description: "Compare CPU throughput against the selected GPU compute adapter.",
            view: AppView::MatrixBenchmark,
            accent: egui::Color32::from_rgb(68, 184, 151),
            categories: &[MenuCategory::Cpu, MenuCategory::Gpu],
        },
        MainMenuItem {
            title: "Matrix Stress Test",
            description: "Run repeat CPU or GPU matrix workloads for stability checks.",
            view: AppView::MatrixStressTest,
            accent: egui::Color32::from_rgb(214, 169, 74),
            categories: &[MenuCategory::Cpu, MenuCategory::Gpu],
        },
        MainMenuItem {
            title: "GPU Memory Bandwidth",
            description: "Measure GPU internal memory, copy, upload, and readback throughput.",
            view: AppView::GpuMemoryBenchmark,
            accent: egui::Color32::from_rgb(118, 203, 168),
            categories: &[MenuCategory::Gpu],
        },
        MainMenuItem {
            title: "AI Training GPU Benchmark",
            description:
                "Plan training-style GPU workloads with FLOPs, throughput, and latency metrics.",
            view: AppView::AiTrainingBenchmark,
            accent: egui::Color32::from_rgb(109, 180, 238),
            categories: &[MenuCategory::Gpu],
        },
        MainMenuItem {
            title: "Drive Benchmark",
            description: "Measure sequential and random disk performance on a target folder.",
            view: AppView::DriveBenchmark,
            accent: egui::Color32::from_rgb(91, 151, 224),
            categories: &[MenuCategory::Storage, MenuCategory::Io],
        },
        MainMenuItem {
            title: "SSD / HDD Health Checker",
            description: "Inspect SMART/NVMe health, warnings, temperature, and life estimates.",
            view: AppView::StorageHealth,
            accent: egui::Color32::from_rgb(224, 114, 91),
            categories: &[MenuCategory::Storage, MenuCategory::Io],
        },
        MainMenuItem {
            title: "RAM Tester",
            description: "Exercise memory with user-mode patterns and verification passes.",
            view: AppView::RamTester,
            accent: egui::Color32::from_rgb(166, 132, 224),
            categories: &[MenuCategory::Ram],
        },
        MainMenuItem {
            title: "Battery Health Diagnostic",
            description: "Review capacity, charge behavior, health estimates, and live readings.",
            view: AppView::BatteryHealthDiagnostic,
            accent: egui::Color32::from_rgb(112, 190, 105),
            categories: &[MenuCategory::Misc],
        },
        MainMenuItem {
            title: "Network Hardware Diagnostic",
            description: "Check adapter state, link quality, gateway, DNS, jitter, and packet loss.",
            view: AppView::NetworkDiagnostic,
            accent: egui::Color32::from_rgb(87, 188, 211),
            categories: &[MenuCategory::Drivers, MenuCategory::Io],
        },
        MainMenuItem {
            title: "Device Information Viewer",
            description: "Browse system, BIOS, board, CPU, memory, disk, GPU, and driver details.",
            view: AppView::DeviceInfo,
            accent: egui::Color32::from_rgb(202, 129, 190),
            categories: &[
                MenuCategory::Cpu,
                MenuCategory::Gpu,
                MenuCategory::Ram,
                MenuCategory::Storage,
                MenuCategory::Drivers,
                MenuCategory::Io,
                MenuCategory::Misc,
            ],
        },
    ]
}

fn main_menu_items_for_category(category: MenuCategory) -> Vec<MainMenuItem> {
    main_menu_tool_items()
        .into_iter()
        .filter(|item| item.belongs_to(category))
        .collect()
}

#[cfg(test)]
fn main_menu_views_for_category(category: MenuCategory) -> Vec<AppView> {
    main_menu_items_for_category(category)
        .into_iter()
        .map(|item| item.view)
        .collect()
}

fn main_menu_subtitle(category: Option<MenuCategory>) -> &'static str {
    category
        .map(MenuCategory::label)
        .unwrap_or("Hardware benchmarks and diagnostics")
}

fn main_menu_scroll_id(category: Option<MenuCategory>) -> &'static str {
    category
        .map(MenuCategory::scroll_id)
        .unwrap_or("main_menu_scroll_categories")
}

fn ui_main_menu_card_grid(
    ui: &mut egui::Ui,
    content_width: f32,
    cards: &[MainMenuCard],
) -> Option<usize> {
    let mut clicked_index = None;
    let two_columns = content_width >= 760.0;
    let gap = if two_columns { 14.0 } else { 10.0 };
    if two_columns {
        for (row_index, row) in cards.chunks(2).enumerate() {
            ui.columns(2, |columns| {
                for (column, card) in row.iter().enumerate() {
                    let card_width = columns[column].available_width().max(1.0);
                    if ui_main_menu_card(
                        &mut columns[column],
                        card,
                        row_index * 2 + column,
                        egui::vec2(card_width, 94.0),
                    )
                    .clicked()
                    {
                        clicked_index = Some(row_index * 2 + column);
                    }
                }
            });
            ui.add_space(gap);
        }
    } else {
        let card_height = if content_width < 420.0 { 122.0 } else { 90.0 };
        for (index, card) in cards.iter().enumerate() {
            let card_width = ui.available_width().max(1.0);
            if ui_main_menu_card(ui, card, index, egui::vec2(card_width, card_height)).clicked() {
                clicked_index = Some(index);
            }
            ui.add_space(gap);
        }
    }
    clicked_index
}

fn ui_main_menu_card(
    ui: &mut egui::Ui,
    card: &MainMenuCard,
    index: usize,
    size: egui::Vec2,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let response = ui.interact(
        rect,
        egui::Id::new(("main_menu_card", card.title, index)),
        egui::Sense::click(),
    );
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
            egui::Stroke::new(1.4, card.accent)
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
            card.accent,
        );

        let text_rect = rect.shrink2(egui::vec2(20.0, 14.0));
        let text_painter = ui.painter().with_clip_rect(text_rect);
        let title_color = egui::Color32::from_rgb(241, 244, 248);
        let description_color = egui::Color32::from_rgb(166, 176, 190);
        let wrap_width = text_rect.width().max(1.0);
        let title_galley = text_painter.layout(
            card.title.to_owned(),
            egui::FontId::proportional(18.0),
            title_color,
            wrap_width,
        );
        let title_height = title_galley.size().y;
        text_painter.galley(text_rect.min, title_galley, title_color);

        let description_pos = egui::pos2(text_rect.left(), text_rect.top() + title_height + 5.0);
        let description_galley = text_painter.layout(
            card.description.to_owned(),
            egui::FontId::proportional(14.5),
            description_color,
            wrap_width,
        );
        text_painter.galley(description_pos, description_galley, description_color);
    }

    response.on_hover_text(card.description)
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
