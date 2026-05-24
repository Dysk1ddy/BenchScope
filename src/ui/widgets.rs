fn ui_large_back_button(ui: &mut egui::Ui) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new("Back").size(20.0).strong())
            .min_size(egui::vec2(104.0, 42.0)),
    )
}

fn ui_start_action_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui_highlighted_action_button(
        ui,
        label,
        egui::Color32::from_rgb(31, 129, 91),
        egui::Color32::from_rgb(84, 224, 160),
        egui::Color32::from_rgb(47, 67, 61),
        egui::Color32::from_rgb(84, 111, 100),
    )
}

fn ui_cancel_action_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui_highlighted_action_button(
        ui,
        label,
        egui::Color32::from_rgb(166, 58, 67),
        egui::Color32::from_rgb(244, 127, 135),
        egui::Color32::from_rgb(74, 52, 55),
        egui::Color32::from_rgb(119, 82, 87),
    )
}

fn ui_highlighted_action_button(
    ui: &mut egui::Ui,
    label: &str,
    active_fill: egui::Color32,
    active_stroke: egui::Color32,
    disabled_fill: egui::Color32,
    disabled_stroke: egui::Color32,
) -> egui::Response {
    let enabled = ui.is_enabled();
    let fill = if enabled { active_fill } else { disabled_fill };
    let stroke = if enabled {
        egui::Stroke::new(1.6, active_stroke)
    } else {
        egui::Stroke::new(1.0, disabled_stroke)
    };
    let text_color = if enabled {
        egui::Color32::from_rgb(248, 250, 252)
    } else {
        egui::Color32::from_rgb(166, 176, 184)
    };
    let width = ui.available_width().clamp(220.0, 360.0);

    ui.add_sized(
        [width, 38.0],
        egui::Button::new(
            egui::RichText::new(label)
                .size(17.0)
                .strong()
                .color(text_color),
        )
        .fill(fill)
        .stroke(stroke)
        .corner_radius(egui::CornerRadius::same(6)),
    )
}

#[cfg(test)]
fn panel_content_log_heights(available_height: f32, log_fraction: f32, log_max: f32) -> (f32, f32) {
    panel_content_log_heights_with_chrome(
        available_height,
        log_fraction,
        log_max,
        PANEL_VERTICAL_CHROME_HEIGHT,
    )
}

fn panel_content_log_heights_with_chrome(
    available_height: f32,
    log_fraction: f32,
    log_max: f32,
    fixed_height: f32,
) -> (f32, f32) {
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

fn resizable_panel_content_log_heights(
    ui: &mut egui::Ui,
    log_id: &'static str,
    available_height: f32,
    log_fraction: f32,
    log_max: f32,
) -> (f32, f32) {
    let fixed_height = PANEL_VERTICAL_CHROME_HEIGHT + LOG_RESIZE_HANDLE_HEIGHT;
    let usable_height = (available_height - fixed_height).max(0.0);
    let (_, default_log_height) =
        panel_content_log_heights_with_chrome(available_height, log_fraction, log_max, fixed_height);
    let (min_log_height, max_log_height) = panel_log_resize_bounds(usable_height, log_max);
    let height_id = panel_log_height_id(log_id);
    let saved_log_height = ui
        .ctx()
        .data_mut(|data| data.get_persisted::<f32>(height_id));
    let log_height = saved_log_height
        .unwrap_or(default_log_height)
        .clamp(min_log_height, max_log_height);

    ((usable_height - log_height).max(40.0), log_height)
}

fn ui_log_resize_handle(
    ui: &mut egui::Ui,
    log_id: &'static str,
    available_height: f32,
    current_log_height: f32,
    log_max: f32,
) {
    let height_id = panel_log_height_id(log_id);
    let drag_start_id = panel_log_drag_start_id(log_id);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), LOG_RESIZE_HANDLE_HEIGHT),
        egui::Sense::drag(),
    );
    let response = response
        .on_hover_cursor(egui::CursorIcon::ResizeVertical)
        .on_hover_text("Drag to resize log");

    if response.drag_started() {
        ui.ctx()
            .data_mut(|data| data.insert_temp(drag_start_id, current_log_height));
    }

    if response.dragged() {
        let usable_height =
            (available_height - PANEL_VERTICAL_CHROME_HEIGHT - LOG_RESIZE_HANDLE_HEIGHT).max(0.0);
        let (min_log_height, max_log_height) = panel_log_resize_bounds(usable_height, log_max);
        let start_log_height = ui
            .ctx()
            .data_mut(|data| data.get_temp::<f32>(drag_start_id))
            .unwrap_or(current_log_height);
        let resized_log_height =
            (start_log_height - response.drag_delta().y).clamp(min_log_height, max_log_height);
        ui.ctx()
            .data_mut(|data| data.insert_persisted(height_id, resized_log_height));
        ui.ctx().request_repaint();
    }

    let visuals = ui.style().interact(&response);
    let color = if response.dragged() {
        visuals.fg_stroke.color
    } else if response.hovered() {
        visuals.bg_stroke.color
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke.color
    };
    let grip_width = (rect.width() * 0.18).clamp(48.0, 160.0);
    let grip_rect =
        egui::Rect::from_center_size(rect.center(), egui::vec2(grip_width, 3.0));
    ui.painter()
        .rect_filled(grip_rect, egui::CornerRadius::same(2), color);
}

fn panel_log_resize_bounds(usable_height: f32, log_max: f32) -> (f32, f32) {
    if usable_height <= MIN_CONTENT_HEIGHT + MIN_LOG_HEIGHT {
        let max_log_height = (usable_height * 0.5).max(32.0).min(log_max);
        return (32.0_f32.min(max_log_height), max_log_height);
    }

    let max_log_height = (usable_height - MIN_CONTENT_HEIGHT)
        .min(log_max)
        .max(MIN_LOG_HEIGHT);
    (MIN_LOG_HEIGHT.min(max_log_height), max_log_height)
}

fn panel_log_height_id(log_id: &'static str) -> egui::Id {
    egui::Id::new(("panel_log_height", log_id))
}

fn panel_log_drag_start_id(log_id: &'static str) -> egui::Id {
    egui::Id::new(("panel_log_drag_start", log_id))
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

fn result_header_info(ui: &mut egui::Ui, text: &str, help: &str) {
    ui.label(
        egui::RichText::new(format!("{text} ?"))
            .strong()
            .size(RESULT_HEADER_TEXT_SIZE),
    )
    .on_hover_ui(|ui| {
        ui.set_max_width(360.0);
        ui.label(help);
    });
}

fn result_cell(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(egui::RichText::new(text.into()).size(RESULT_CELL_TEXT_SIZE));
}

fn result_cell_hover(ui: &mut egui::Ui, text: impl Into<String>, help: impl Into<String>) {
    let text = text.into();
    let help = help.into();
    ui.label(egui::RichText::new(truncate_result_cell(&text)).size(RESULT_CELL_TEXT_SIZE))
        .on_hover_ui(|ui| {
            ui.set_max_width(460.0);
            ui.label(help);
        });
}

fn truncate_result_cell(text: &str) -> String {
    const MAX_CHARS: usize = 88;
    if text.chars().count() <= MAX_CHARS {
        return text.to_owned();
    }

    let prefix = text.chars().take(MAX_CHARS.saturating_sub(3)).collect::<String>();
    format!("{prefix}...")
}

fn ui_pytorch_cuda_status(
    ui: &mut egui::Ui,
    environment: Option<&PyTorchCudaEnvironment>,
    empty_text: &str,
) {
    if let Some(environment) = environment {
        if environment.cuda_available {
            ui.small(format!(
                "Ready via {} with {} CUDA device(s).",
                environment.python_executable, environment.device_count
            ));
        } else if let Some(error) = &environment.error {
            ui.colored_label(egui::Color32::YELLOW, format!("Unavailable: {error}"));
        } else {
            ui.colored_label(
                egui::Color32::YELLOW,
                "PyTorch imported, but CUDA is unavailable.",
            );
        }
    } else {
        ui.small(empty_text);
    }
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
