impl BenchScopeApp {
    fn ui_device_info(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("device_info_top_panel").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui_large_back_button(ui).clicked() {
                    self.view = AppView::MainMenu;
                }
                self.ui_fullscreen_button(ui);
                ui.separator();
                ui.heading("Device Information");
                ui.separator();
                if self.device_info.running {
                    ui.spinner();
                }
                ui.label(&self.device_info.status);
            });
        });

        egui::Panel::left("device_info_controls")
            .resizable(false)
            .min_size(330.0)
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("device_info_controls_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.heading("Controls");
                        ui.add_space(8.0);
                        ui.add_enabled_ui(!self.device_info.running, |ui| {
                            if ui.button("Refresh hardware inventory").clicked() {
                                self.device_info.start_refresh(self.adapters.clone());
                            }
                            if ui.button("Export report").clicked() {
                                self.device_info.export_report();
                            }
                        });
                        if self.device_info.running {
                            ui.small("Refreshing Windows hardware, firmware, and driver inventory.");
                        }
                        if let Some(path) = &self.device_info.last_report_path {
                            ui.small(format!("Last report: {}", path.display()));
                        }

                        ui.separator();
                        ui.label(egui::RichText::new("Sections").strong());
                        for page in DeviceInfoPage::ALL {
                            ui.selectable_value(
                                &mut self.device_info.selected_page,
                                page,
                                page.label(),
                            );
                        }

                        if let Some(snapshot) = &self.device_info.snapshot {
                            ui.separator();
                            ui.label(egui::RichText::new("Summary").strong());
                            ui.small(format!(
                                "CPU: {} core(s), {} logical",
                                device_info_option_u32(snapshot.cpu_core_count()),
                                device_info_option_u32(snapshot.cpu_logical_processor_count())
                            ));
                            ui.small(format!(
                                "RAM: {} across {} module(s)",
                                format_optional_bytes(snapshot.total_ram_bytes()),
                                snapshot.memory_modules.len()
                            ));
                            ui.small(format!(
                                "Storage: {} across {} disk(s)",
                                format_optional_bytes(snapshot.total_storage_bytes()),
                                snapshot.disks.len()
                            ));
                            ui.small(format!(
                                "GPU adapters: Windows {}, wgpu {}",
                                snapshot.gpus.len(),
                                snapshot.wgpu_adapters.len()
                            ));
                            ui.small(format!("Drivers: {}", snapshot.drivers.len()));
                        }
                    });
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            let available_height = ui.available_height();
            let log_height = (available_height * 0.20).clamp(110.0, 180.0);
            let content_height = (available_height - log_height - 18.0).max(140.0);

            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), content_height),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if let Some(snapshot) = &self.device_info.snapshot {
                                match self.device_info.selected_page {
                                    DeviceInfoPage::Overview => ui_device_info_overview(ui, snapshot),
                                    DeviceInfoPage::CpuMemory => {
                                        ui_device_info_cpu_memory(ui, snapshot)
                                    }
                                    DeviceInfoPage::Storage => ui_device_info_storage(ui, snapshot),
                                    DeviceInfoPage::Graphics => ui_device_info_graphics(ui, snapshot),
                                    DeviceInfoPage::Drivers => ui_device_info_drivers(
                                        ui,
                                        snapshot,
                                        &mut self.device_info.show_all_driver_classes,
                                    ),
                                    DeviceInfoPage::Firmware => {
                                        ui_device_info_firmware(ui, snapshot)
                                    }
                                    DeviceInfoPage::ProviderPlan => {
                                        ui_device_info_provider_plan(ui)
                                    }
                                }
                            } else {
                                ui.heading("Hardware Inventory");
                                ui.label("Refresh hardware inventory to collect system, firmware, device, and driver details.");
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
                    for line in &self.device_info.log {
                        ui_log_line(ui, line);
                    }
                });
        });
    }
}

fn ui_device_info_overview(ui: &mut egui::Ui, snapshot: &DeviceInfoSnapshot) {
    ui.heading("Overview");
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        device_info_metric(
            ui,
            "CPU",
            format!(
                "{} / {}",
                device_info_option_u32(snapshot.cpu_core_count()),
                device_info_option_u32(snapshot.cpu_logical_processor_count())
            ),
            "cores / logical",
        );
        device_info_metric(
            ui,
            "RAM",
            format_optional_bytes(snapshot.total_ram_bytes()),
            "installed",
        );
        device_info_metric(
            ui,
            "Storage",
            format_optional_bytes(snapshot.total_storage_bytes()),
            "raw disk capacity",
        );
        device_info_metric(
            ui,
            "Drivers",
            snapshot.drivers.len().to_string(),
            "signed-driver records",
        );
    });

    ui.separator();
    ui.label(egui::RichText::new("System").strong());
    egui::Grid::new("device_info_system_grid")
        .striped(true)
        .num_columns(2)
        .show(ui, |ui| {
            if let Some(system) = &snapshot.system {
                device_info_row(ui, "Manufacturer", system.manufacturer.as_deref());
                device_info_row(ui, "Model", system.model.as_deref());
                device_info_row(ui, "Family", system.family.as_deref());
                device_info_row(ui, "SKU", system.sku.as_deref());
                device_info_row(ui, "System type", system.system_type.as_deref());
                device_info_row_value(ui, "Chassis", device_info_vec_label(&system.chassis));
                device_info_row_value(
                    ui,
                    "Physical memory",
                    format_optional_bytes(system.total_physical_memory_bytes),
                );
                device_info_row_value(
                    ui,
                    "Processors",
                    format!(
                        "{} package(s), {} logical processor(s)",
                        device_info_option_u32(system.physical_processor_count),
                        device_info_option_u32(system.logical_processor_count)
                    ),
                );
                device_info_row_value(
                    ui,
                    "Hypervisor present",
                    device_info_bool_label(system.hypervisor_present),
                );
                device_info_row(ui, "Domain", system.domain.as_deref());
                device_info_row(ui, "Workgroup", system.workgroup.as_deref());
                device_info_row(ui, "User", system.user_name.as_deref());
            }
            if let Some(os) = &snapshot.os {
                device_info_row(ui, "OS", os.caption.as_deref());
                device_info_row(ui, "OS version", os.version.as_deref());
                device_info_row(ui, "Build", os.build_number.as_deref());
                device_info_row(ui, "Architecture", os.architecture.as_deref());
                device_info_row(ui, "Install date", os.install_date.as_deref());
                device_info_row(ui, "Last boot", os.last_boot_time.as_deref());
            }
        });

    ui.separator();
    ui.label(egui::RichText::new("Provider Notes").strong());
    if snapshot.provider_notes.is_empty() {
        ui.label("No provider warnings were reported.");
    } else {
        for note in &snapshot.provider_notes {
            ui.small(note);
        }
    }
}

fn ui_device_info_cpu_memory(ui: &mut egui::Ui, snapshot: &DeviceInfoSnapshot) {
    ui.heading("CPU / RAM");
    ui.add_space(6.0);

    ui.label(egui::RichText::new("Processors").strong());
    egui::Grid::new("device_info_cpu_grid")
        .striped(true)
        .num_columns(13)
        .show(ui, |ui| {
            result_header(ui, "Name");
            result_header(ui, "Manufacturer");
            result_header(ui, "Description");
            result_header(ui, "Socket");
            result_header(ui, "Processor ID");
            result_header(ui, "Cores");
            result_header(ui, "Logical");
            result_header(ui, "Clock");
            result_header(ui, "L2");
            result_header(ui, "L3");
            result_header(ui, "Arch");
            result_header(ui, "Family / Model / Stepping");
            result_header(ui, "Virtualization");
            ui.end_row();
            for cpu in &snapshot.cpus {
                result_cell(ui, &cpu.name);
                result_cell(ui, device_info_option_label(cpu.manufacturer.as_deref()));
                result_cell(ui, device_info_option_label(cpu.description.as_deref()));
                result_cell(ui, device_info_option_label(cpu.socket.as_deref()));
                result_cell(ui, device_info_option_label(cpu.processor_id.as_deref()));
                result_cell(ui, device_info_option_u32(cpu.cores));
                result_cell(ui, device_info_option_u32(cpu.logical_processors));
                result_cell(
                    ui,
                    format!(
                        "{} / {}",
                        device_info_mhz_label(cpu.current_clock_mhz),
                        device_info_mhz_label(cpu.max_clock_mhz)
                    ),
                );
                result_cell(ui, device_info_kb_label(cpu.l2_cache_kb));
                result_cell(ui, device_info_kb_label(cpu.l3_cache_kb));
                result_cell(ui, device_info_option_label(cpu.architecture.as_deref()));
                result_cell(
                    ui,
                    format!(
                        "{} / {} / {}",
                        device_info_option_label(cpu.family.as_deref()),
                        device_info_option_label(cpu.model.as_deref()),
                        device_info_option_label(cpu.stepping.as_deref())
                    ),
                );
                result_cell(
                    ui,
                    format!(
                        "FW {}, SLAT {}, VMX {}",
                        device_info_bool_label(cpu.virtualization_firmware_enabled),
                        device_info_bool_label(cpu.second_level_address_translation),
                        device_info_bool_label(cpu.vm_monitor_extensions)
                    ),
                );
                ui.end_row();
            }
        });

    ui.separator();
    ui.label(egui::RichText::new("Memory Modules").strong());
    egui::Grid::new("device_info_memory_grid")
        .striped(true)
        .num_columns(12)
        .show(ui, |ui| {
            result_header(ui, "Slot");
            result_header(ui, "Bank");
            result_header(ui, "Capacity");
            result_header(ui, "Type");
            result_header(ui, "Detail");
            result_header(ui, "Form");
            result_header(ui, "Speed");
            result_header(ui, "Configured");
            result_header(ui, "Width");
            result_header(ui, "Manufacturer");
            result_header(ui, "Part");
            result_header(ui, "Serial");
            ui.end_row();
            for module in &snapshot.memory_modules {
                result_cell(ui, device_info_option_label(module.device_locator.as_deref()));
                result_cell(ui, device_info_option_label(module.bank_label.as_deref()));
                result_cell(ui, format_optional_bytes(module.capacity_bytes));
                result_cell(
                    ui,
                    device_info_option_label(
                        module
                            .smbios_memory_type
                            .as_deref()
                            .or(module.memory_type.as_deref()),
                        ),
                );
                result_cell(ui, device_info_option_label(module.type_detail.as_deref()));
                result_cell(ui, device_info_option_label(module.form_factor.as_deref()));
                result_cell(ui, device_info_mhz_label(module.speed_mhz));
                result_cell(ui, device_info_mhz_label(module.configured_clock_speed_mhz));
                result_cell(
                    ui,
                    format!(
                        "{} / {}",
                        device_info_bits_label(module.data_width_bits),
                        device_info_bits_label(module.total_width_bits)
                    ),
                );
                result_cell(ui, device_info_option_label(module.manufacturer.as_deref()));
                result_cell(ui, device_info_option_label(module.part_number.as_deref()));
                result_cell(ui, device_info_option_label(module.serial_number.as_deref()));
                ui.end_row();
            }
        });
}

fn ui_device_info_storage(ui: &mut egui::Ui, snapshot: &DeviceInfoSnapshot) {
    ui.heading("Storage");
    ui.add_space(6.0);

    ui.label(egui::RichText::new("Physical Disks").strong());
    egui::Grid::new("device_info_disk_grid")
        .striped(true)
        .num_columns(12)
        .show(ui, |ui| {
            result_header(ui, "#");
            result_header(ui, "Model");
            result_header(ui, "Device ID");
            result_header(ui, "Size");
            result_header(ui, "Bus");
            result_header(ui, "Media");
            result_header(ui, "Interface");
            result_header(ui, "Partitions");
            result_header(ui, "Firmware");
            result_header(ui, "Serial");
            result_header(ui, "Health");
            result_header(ui, "Status");
            ui.end_row();
            for disk in &snapshot.disks {
                result_cell(ui, device_info_option_u32(disk.index));
                result_cell(ui, &disk.model);
                result_cell(ui, device_info_option_label(disk.device_id.as_deref()));
                result_cell(ui, format_optional_bytes(disk.size_bytes));
                result_cell(ui, device_info_option_label(disk.bus_type.as_deref()));
                result_cell(ui, device_info_option_label(disk.media_type.as_deref()));
                result_cell(ui, device_info_option_label(disk.interface_type.as_deref()));
                result_cell(ui, device_info_option_u32(disk.partitions));
                result_cell(ui, device_info_option_label(disk.firmware.as_deref()));
                result_cell(ui, device_info_option_label(disk.serial_number.as_deref()));
                result_cell(ui, device_info_option_label(disk.health_status.as_deref()));
                result_cell(
                    ui,
                    device_info_option_label(
                        disk.operational_status.as_deref().or(disk.status.as_deref()),
                    ),
                );
                ui.end_row();
            }
        });

    ui.separator();
    ui.label(egui::RichText::new("Volumes").strong());
    egui::Grid::new("device_info_volume_grid")
        .striped(true)
        .num_columns(8)
        .show(ui, |ui| {
            result_header(ui, "Drive");
            result_header(ui, "Label");
            result_header(ui, "File system");
            result_header(ui, "Type");
            result_header(ui, "Size");
            result_header(ui, "Free");
            result_header(ui, "Health");
            result_header(ui, "Status");
            ui.end_row();
            for volume in &snapshot.volumes {
                result_cell(ui, device_info_option_label(volume.drive_letter.as_deref()));
                result_cell(ui, device_info_option_label(volume.label.as_deref()));
                result_cell(ui, device_info_option_label(volume.file_system.as_deref()));
                result_cell(ui, device_info_option_label(volume.drive_type.as_deref()));
                result_cell(ui, format_optional_bytes(volume.size_bytes));
                result_cell(ui, format_optional_bytes(volume.free_bytes));
                result_cell(ui, device_info_option_label(volume.health_status.as_deref()));
                result_cell(ui, device_info_option_label(volume.operational_status.as_deref()));
                ui.end_row();
            }
        });
}

fn ui_device_info_graphics(ui: &mut egui::Ui, snapshot: &DeviceInfoSnapshot) {
    ui.heading("Graphics");
    ui.add_space(6.0);

    ui.label(egui::RichText::new("Windows Display Controllers").strong());
    egui::Grid::new("device_info_gpu_grid")
        .striped(true)
        .num_columns(11)
        .show(ui, |ui| {
            result_header(ui, "Name");
            result_header(ui, "PNP ID");
            result_header(ui, "Processor");
            result_header(ui, "Vendor");
            result_header(ui, "RAM");
            result_header(ui, "Driver provider");
            result_header(ui, "Version");
            result_header(ui, "Date");
            result_header(ui, "INF");
            result_header(ui, "Display");
            result_header(ui, "Status");
            ui.end_row();
            for gpu in &snapshot.gpus {
                result_cell(ui, &gpu.name);
                result_cell(ui, device_info_option_label(gpu.pnp_device_id.as_deref()));
                result_cell(ui, device_info_option_label(gpu.video_processor.as_deref()));
                result_cell(ui, device_info_option_label(gpu.adapter_compatibility.as_deref()));
                result_cell(ui, format_optional_bytes(gpu.adapter_ram_bytes));
                result_cell(ui, device_info_option_label(gpu.driver_provider.as_deref()));
                result_cell(ui, device_info_option_label(gpu.driver_version.as_deref()));
                result_cell(ui, device_info_option_label(gpu.driver_date.as_deref()));
                result_cell(ui, device_info_option_label(gpu.inf_name.as_deref()));
                result_cell(
                    ui,
                    format!(
                        "{} @ {}",
                        device_info_option_label(gpu.resolution.as_deref()),
                        device_info_hz_label(gpu.refresh_rate_hz)
                    ),
                );
                result_cell(ui, device_info_option_label(gpu.status.as_deref()));
                ui.end_row();
            }
        });

    ui.separator();
    ui.label(egui::RichText::new("BenchScope wgpu / DXGI Adapters").strong());
    egui::Grid::new("device_info_wgpu_grid")
        .striped(true)
        .num_columns(9)
        .show(ui, |ui| {
            result_header(ui, "#");
            result_header(ui, "Name");
            result_header(ui, "Backend");
            result_header(ui, "Type");
            result_header(ui, "Vendor");
            result_header(ui, "Device");
            result_header(ui, "Driver");
            result_header(ui, "VRAM");
            result_header(ui, "Timestamp");
            ui.end_row();
            for adapter in &snapshot.wgpu_adapters {
                result_cell(ui, adapter.index.to_string());
                result_cell(ui, &adapter.name);
                result_cell(ui, format!("{:?}", adapter.backend));
                result_cell(ui, device_type_label(adapter.device_type));
                result_cell(ui, format!("{:04X}", adapter.vendor));
                result_cell(ui, format!("{:04X}", adapter.device));
                result_cell(ui, empty_to_unknown(&adapter.driver));
                result_cell(ui, format_optional_bytes(adapter.dedicated_vram_bytes));
                result_cell(ui, if adapter.timestamp_query { "yes" } else { "no" });
                ui.end_row();
            }
        });
}

fn ui_device_info_drivers(
    ui: &mut egui::Ui,
    snapshot: &DeviceInfoSnapshot,
    show_all_driver_classes: &mut bool,
) {
    ui.heading("Driver Inventory");
    ui.add_space(6.0);
    ui.checkbox(show_all_driver_classes, "Show system, HID, and software-component classes");
    let visible = snapshot
        .drivers
        .iter()
        .filter(|driver| *show_all_driver_classes || device_info_is_primary_driver(driver))
        .collect::<Vec<_>>();
    ui.small(format!(
        "Showing {} of {} collected signed-driver records.",
        visible.len(),
        snapshot.drivers.len()
    ));

    egui::Grid::new("device_info_driver_grid")
        .striped(true)
        .num_columns(10)
        .show(ui, |ui| {
            result_header(ui, "Class");
            result_header(ui, "Device");
            result_header(ui, "Manufacturer");
            result_header(ui, "Provider");
            result_header(ui, "Version");
            result_header(ui, "Date");
            result_header(ui, "Signed");
            result_header(ui, "Signer");
            result_header(ui, "INF");
            result_header(ui, "Device ID");
            ui.end_row();
            for driver in visible {
                result_cell(ui, device_info_option_label(driver.device_class.as_deref()));
                result_cell(ui, &driver.device_name);
                result_cell(ui, device_info_option_label(driver.manufacturer.as_deref()));
                result_cell(ui, device_info_option_label(driver.provider.as_deref()));
                result_cell(ui, device_info_option_label(driver.version.as_deref()));
                result_cell(ui, device_info_option_label(driver.date.as_deref()));
                result_cell(ui, device_info_bool_label(driver.is_signed));
                result_cell(ui, device_info_option_label(driver.signer.as_deref()));
                result_cell(ui, device_info_option_label(driver.inf_name.as_deref()));
                result_cell(ui, device_info_option_label(driver.device_id.as_deref()));
                ui.end_row();
            }
        });
}

fn ui_device_info_firmware(ui: &mut egui::Ui, snapshot: &DeviceInfoSnapshot) {
    ui.heading("Firmware / Board / Displays");
    ui.add_space(6.0);

    ui.label(egui::RichText::new("BIOS").strong());
    egui::Grid::new("device_info_bios_grid")
        .striped(true)
        .num_columns(2)
        .show(ui, |ui| {
            if let Some(bios) = &snapshot.bios {
                device_info_row(ui, "Manufacturer", bios.manufacturer.as_deref());
                device_info_row(ui, "SMBIOS version", bios.smbios_version.as_deref());
                device_info_row(ui, "Version", bios.version.as_deref());
                device_info_row(ui, "Release date", bios.release_date.as_deref());
                device_info_row(ui, "Serial", bios.serial_number.as_deref());
            }
            if let Some(board) = &snapshot.baseboard {
                device_info_row(ui, "Board manufacturer", board.manufacturer.as_deref());
                device_info_row(ui, "Board product", board.product.as_deref());
                device_info_row(ui, "Board version", board.version.as_deref());
                device_info_row(ui, "Board serial", board.serial_number.as_deref());
            }
        });

    ui.separator();
    ui.label(egui::RichText::new("Monitors").strong());
    egui::Grid::new("device_info_monitor_grid")
        .striped(true)
        .num_columns(6)
        .show(ui, |ui| {
            result_header(ui, "Name");
            result_header(ui, "Manufacturer");
            result_header(ui, "Type");
            result_header(ui, "Resolution");
            result_header(ui, "PNP ID");
            result_header(ui, "Status");
            ui.end_row();
            for monitor in &snapshot.monitors {
                result_cell(ui, &monitor.name);
                result_cell(ui, device_info_option_label(monitor.manufacturer.as_deref()));
                result_cell(ui, device_info_option_label(monitor.monitor_type.as_deref()));
                result_cell(ui, device_info_option_label(monitor.resolution.as_deref()));
                result_cell(ui, device_info_option_label(monitor.pnp_device_id.as_deref()));
                result_cell(ui, device_info_option_label(monitor.status.as_deref()));
                ui.end_row();
            }
        });

    ui.separator();
    ui.label(egui::RichText::new("Network Adapters").strong());
    egui::Grid::new("device_info_network_grid")
        .striped(true)
        .num_columns(8)
        .show(ui, |ui| {
            result_header(ui, "Name");
            result_header(ui, "Description");
            result_header(ui, "Physical");
            result_header(ui, "MAC");
            result_header(ui, "Speed");
            result_header(ui, "Driver");
            result_header(ui, "Date");
            result_header(ui, "Status");
            ui.end_row();
            for adapter in &snapshot.network_adapters {
                result_cell(ui, &adapter.name);
                result_cell(ui, device_info_option_label(adapter.description.as_deref()));
                result_cell(ui, device_info_bool_label(adapter.physical));
                result_cell(ui, device_info_option_label(adapter.mac_address.as_deref()));
                result_cell(ui, format_link_speed(adapter.speed_bps));
                result_cell(
                    ui,
                    format!(
                        "{} {}",
                        device_info_option_label(adapter.driver_provider.as_deref()),
                        device_info_option_label(adapter.driver_version.as_deref())
                    ),
                );
                result_cell(ui, device_info_option_label(adapter.driver_date.as_deref()));
                result_cell(
                    ui,
                    format!(
                        "{} / enabled {}",
                        device_info_option_label(adapter.status.as_deref()),
                        device_info_bool_label(adapter.net_enabled)
                    ),
                );
                ui.end_row();
            }
        });
}

fn ui_device_info_provider_plan(ui: &mut egui::Ui) {
    ui.heading("Provider Coverage");
    ui.add_space(6.0);
    egui::Grid::new("device_info_provider_plan_grid")
        .striped(true)
        .num_columns(5)
        .show(ui, |ui| {
            result_header(ui, "Area");
            result_header(ui, "Current provider");
            result_header(ui, "Status");
            result_header(ui, "Extra driver");
            result_header(ui, "Notes");
            ui.end_row();
            for plan in device_info_provider_plan() {
                result_cell(ui, plan.area);
                result_cell(ui, plan.current_provider);
                result_cell(ui, plan.status);
                result_cell(ui, plan.extra_driver_needed);
                result_cell(ui, plan.notes);
                ui.end_row();
            }
        });
}

fn device_info_metric(ui: &mut egui::Ui, label: &str, value: String, detail: &str) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_min_width(150.0);
            ui.small(label);
            ui.label(egui::RichText::new(value).strong());
            ui.small(detail);
        });
}

fn device_info_row(ui: &mut egui::Ui, label: &str, value: Option<&str>) {
    device_info_row_value(ui, label, device_info_option_label(value));
}

fn device_info_row_value(ui: &mut egui::Ui, label: &str, value: impl Into<String>) {
    ui.label(egui::RichText::new(label).strong());
    ui.label(value.into());
    ui.end_row();
}

fn device_info_kb_label(value: Option<u32>) -> String {
    value
        .map(|value| {
            if value >= 1024 {
                format!("{:.1} MiB", value as f32 / 1024.0)
            } else {
                format!("{value} KiB")
            }
        })
        .unwrap_or_else(|| "N/A".to_owned())
}

fn device_info_bits_label(value: Option<u32>) -> String {
    value
        .map(|value| format!("{value} bit"))
        .unwrap_or_else(|| "N/A".to_owned())
}

fn device_info_hz_label(value: Option<u32>) -> String {
    value
        .map(|value| format!("{value} Hz"))
        .unwrap_or_else(|| "N/A".to_owned())
}

fn device_info_is_primary_driver(driver: &DeviceDriverRecord) -> bool {
    let Some(class) = driver.device_class.as_deref() else {
        return true;
    };
    !matches!(
        class.to_ascii_uppercase().as_str(),
        "SYSTEM" | "HIDCLASS" | "KEYBOARD" | "MOUSE" | "SOFTWARECOMPONENT"
    )
}
