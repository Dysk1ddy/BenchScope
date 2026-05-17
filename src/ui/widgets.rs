fn ui_large_back_button(ui: &mut egui::Ui) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new("Back").size(20.0).strong())
            .min_size(egui::vec2(104.0, 42.0)),
    )
}

fn panel_content_log_heights(available_height: f32, log_fraction: f32, log_max: f32) -> (f32, f32) {
    let fixed_height = PANEL_VERTICAL_CHROME_HEIGHT;
    let usable_height = (available_height - fixed_height).max(0.0);
    if usable_height <= MIN_CONTENT_HEIGHT + MIN_LOG_HEIGHT {
        let log_height = (usable_height * 0.34)
            .clamp(40.0, MIN_LOG_HEIGHT)
            .min(usable_height * 0.5);
        return ((usable_height - log_height).max(40.0), log_height.max(32.0));
    }

    let log_height = (available_height * log_fraction)
        .clamp(MIN_LOG_HEIGHT, log_max)
        .min(usable_height - MIN_CONTENT_HEIGHT);
    (usable_height - log_height, log_height)
}

fn ui_log_line(ui: &mut egui::Ui, line: &str) {
    ui.label(egui::RichText::new(line).monospace().size(LOG_TEXT_SIZE));
}

fn result_header(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .strong()
            .size(RESULT_HEADER_TEXT_SIZE),
    );
}

fn result_cell(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(egui::RichText::new(text.into()).size(RESULT_CELL_TEXT_SIZE));
}

fn device_type_label(value: wgpu::DeviceType) -> &'static str {
    match value {
        wgpu::DeviceType::IntegratedGpu => "Integrated GPU",
        wgpu::DeviceType::DiscreteGpu => "Discrete GPU",
        wgpu::DeviceType::VirtualGpu => "Virtual GPU",
        wgpu::DeviceType::Cpu => "CPU/Software",
        wgpu::DeviceType::Other => "Other GPU",
    }
}

fn adapter_uses_shared_cpu_temperature(adapter: &AdapterInfo) -> bool {
    if adapter.device_type == wgpu::DeviceType::IntegratedGpu {
        return true;
    }

    let name = adapter.name.to_ascii_lowercase();
    adapter.vendor == 0x8086
        && (name.contains("xe")
            || name.contains("iris")
            || name.contains("uhd")
            || name.contains("integrated")
            || name.contains("graphics"))
        || adapter.vendor == 0x1002
            && (name.contains("radeon graphics")
                || name.contains("vega")
                || name.contains("apu")
                || name.contains("integrated"))
}

fn configure_ui_style(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    style
        .text_styles
        .insert(egui::TextStyle::Heading, egui::FontId::proportional(26.0));
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(17.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(17.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, egui::FontId::monospace(16.0));
    style.spacing.button_padding = egui::vec2(14.0, 9.0);
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.interact_size = egui::vec2(44.0, 34.0);
    ctx.set_global_style(style);
}

fn empty_to_unknown(value: &str) -> &str {
    if value.is_empty() { "unknown" } else { value }
}
