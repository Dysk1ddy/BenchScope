fn temperature_color(kind: SensorKind, value: Option<f32>, status: &SensorStatus) -> egui::Color32 {
    if !matches!(status, SensorStatus::Ok | SensorStatus::Partial(_)) {
        return egui::Color32::GRAY;
    }
    match value {
        Some(value) if value >= kind.critical_c() => egui::Color32::RED,
        Some(value) if value >= kind.warning_c() => egui::Color32::YELLOW,
        Some(_) => egui::Color32::WHITE,
        None => egui::Color32::GRAY,
    }
}

const SENSOR_TABLE_ROW_HEIGHT: f32 = 16.0;
const SENSOR_TABLE_COLUMN_GAP: f32 = 6.0;
const SENSOR_TABLE_VALUE_WIDTH: f32 = 104.0;
const SENSOR_TABLE_RANGE_WIDTH: f32 = 76.0;
const SENSOR_TABLE_LABEL_MIN_WIDTH: f32 = 132.0;

#[derive(Clone, Copy)]
struct SensorTableColumns {
    label: f32,
    value: f32,
    min: f32,
    max: f32,
}

fn ui_sensor_table(ui: &mut egui::Ui, rows: &[(&str, Option<&SensorReading>)]) {
    let columns = sensor_table_columns(ui.available_width());
    egui::Grid::new("sensor_metrics_header_grid")
        .num_columns(4)
        .spacing(egui::vec2(SENSOR_TABLE_COLUMN_GAP, 1.0))
        .striped(false)
        .show(ui, |ui| {
            sensor_header_cell(ui, "", columns.label);
            sensor_header_cell(ui, "Value", columns.value);
            sensor_header_cell(ui, "Min", columns.min);
            sensor_header_cell(ui, "Max", columns.max);
            ui.end_row();
        });

    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt("sensor_metrics_body_scroll")
        .auto_shrink([false, false])
        .max_height(ui.available_height().max(80.0))
        .show(ui, |ui| {
            egui::Grid::new("sensor_metrics_grid")
                .num_columns(4)
                .spacing(egui::vec2(SENSOR_TABLE_COLUMN_GAP, 1.0))
                .striped(false)
                .show(ui, |ui| {
                    for (device_label, reading) in rows {
                        ui_sensor_device_rows(ui, columns, device_label, *reading);
                    }
                });
        });
}

fn sensor_table_columns(available_width: f32) -> SensorTableColumns {
    let fixed_width =
        SENSOR_TABLE_VALUE_WIDTH + (SENSOR_TABLE_RANGE_WIDTH * 2.0) + (SENSOR_TABLE_COLUMN_GAP * 3.0);
    SensorTableColumns {
        label: (available_width - fixed_width).max(SENSOR_TABLE_LABEL_MIN_WIDTH),
        value: SENSOR_TABLE_VALUE_WIDTH,
        min: SENSOR_TABLE_RANGE_WIDTH,
        max: SENSOR_TABLE_RANGE_WIDTH,
    }
}

fn sensor_header_cell(ui: &mut egui::Ui, label: &str, width: f32) {
    sensor_text_cell(
        ui,
        width,
        egui::RichText::new(label)
            .small()
            .strong()
            .monospace()
            .color(egui::Color32::LIGHT_GRAY),
        None,
    );
}

fn ui_sensor_device_rows(
    ui: &mut egui::Ui,
    columns: SensorTableColumns,
    label: &str,
    reading: Option<&SensorReading>,
) {
    let tooltip = reading
        .map(sensor_reading_tooltip)
        .unwrap_or_else(|| "No sensor provider initialized".to_owned());
    sensor_text_cell(
        ui,
        columns.label,
        egui::RichText::new(label).strong().small().monospace(),
        Some(&tooltip),
    );
    sensor_text_cell(
        ui,
        columns.value,
        egui::RichText::new(
            reading
                .map(sensor_compact_status_label)
                .unwrap_or("N/A"),
        )
        .small()
        .color(if reading.is_some_and(SensorReading::is_ok) {
            egui::Color32::LIGHT_GRAY
        } else {
            egui::Color32::GRAY
        }),
        Some(&tooltip),
    );
    sensor_text_cell(ui, columns.min, egui::RichText::new("").small(), Some(&tooltip));
    sensor_text_cell(ui, columns.max, egui::RichText::new("").small(), Some(&tooltip));
    ui.end_row();

    let Some(reading) = reading else {
        ui_sensor_metric_placeholder(ui, columns, "Provider", "N/A", &tooltip);
        return;
    };

    let mut rendered_any = false;
    for kind in [
        SensorMetricKind::Temperature,
        SensorMetricKind::MemoryUsage,
        SensorMetricKind::Utilization,
        SensorMetricKind::Voltage,
        SensorMetricKind::Power,
        SensorMetricKind::Clock,
    ] {
        let metrics = reading.metrics_for(kind).collect::<Vec<_>>();
        if metrics.is_empty() {
            if should_show_missing_metric_placeholder(reading, kind) {
                ui_sensor_metric_placeholder(
                    ui,
                    columns,
                    sensor_metric_kind_prefix(kind),
                    sensor_missing_metric_label(reading, kind),
                    &tooltip,
                );
                rendered_any = true;
            }
            continue;
        }

        for metric in metrics {
            ui_sensor_metric_row(ui, columns, reading, metric, &tooltip);
            rendered_any = true;
        }
    }

    if !rendered_any {
        ui_sensor_metric_placeholder(ui, columns, "Status", &reading.status.detail(), &tooltip);
    }
}

fn ui_sensor_metric_row(
    ui: &mut egui::Ui,
    columns: SensorTableColumns,
    reading: &SensorReading,
    metric: &SensorMetric,
    tooltip: &str,
) {
    let label = sensor_metric_compact_label(metric);
    sensor_text_cell(
        ui,
        columns.label,
        egui::RichText::new(label).small().monospace(),
        Some(tooltip),
    );
    sensor_text_cell(
        ui,
        columns.value,
        egui::RichText::new(format_sensor_metric_current_value(metric, &reading.status))
            .small()
            .monospace()
            .color(sensor_metric_value_color(reading, metric)),
        Some(tooltip),
    );
    sensor_text_cell(
        ui,
        columns.min,
        egui::RichText::new(format_sensor_metric_value(metric.kind, metric.min))
            .small()
            .monospace()
            .color(sensor_metric_range_color(metric.min)),
        Some(tooltip),
    );
    sensor_text_cell(
        ui,
        columns.max,
        egui::RichText::new(format_sensor_metric_value(metric.kind, metric.max))
            .small()
            .monospace()
            .color(sensor_metric_range_color(metric.max)),
        Some(tooltip),
    );
    ui.end_row();
}

fn should_show_missing_metric_placeholder(reading: &SensorReading, kind: SensorMetricKind) -> bool {
    match kind {
        SensorMetricKind::Temperature => true,
        SensorMetricKind::Utilization => reading.kind != SensorKind::GpuMemory,
        SensorMetricKind::MemoryUsage => {
            reading.kind == SensorKind::GpuMemory
                && reading.metrics_for(SensorMetricKind::Utilization).next().is_none()
        }
        SensorMetricKind::Voltage | SensorMetricKind::Power | SensorMetricKind::Clock => false,
    }
}

fn ui_sensor_metric_placeholder(
    ui: &mut egui::Ui,
    columns: SensorTableColumns,
    label: &str,
    value: &str,
    tooltip: &str,
) {
    sensor_text_cell(
        ui,
        columns.label,
        egui::RichText::new(label).small().monospace(),
        Some(tooltip),
    );
    sensor_text_cell(
        ui,
        columns.value,
        egui::RichText::new(value)
            .small()
            .monospace()
            .color(egui::Color32::GRAY),
        Some(tooltip),
    );
    sensor_text_cell(
        ui,
        columns.min,
        egui::RichText::new("N/A")
            .small()
            .monospace()
            .color(egui::Color32::GRAY),
        Some(tooltip),
    );
    sensor_text_cell(
        ui,
        columns.max,
        egui::RichText::new("N/A")
            .small()
            .monospace()
            .color(egui::Color32::GRAY),
        Some(tooltip),
    );
    ui.end_row();
}

fn sensor_text_cell(
    ui: &mut egui::Ui,
    width: f32,
    text: egui::RichText,
    tooltip: Option<&str>,
) -> egui::Response {
    let response = ui.add_sized(
        [width, SENSOR_TABLE_ROW_HEIGHT],
        egui::Label::new(text).truncate(),
    );
    if let Some(tooltip) = tooltip {
        response.on_hover_text(tooltip.to_owned())
    } else {
        response
    }
}

fn sensor_compact_status_label(reading: &SensorReading) -> &'static str {
    match reading.status {
        SensorStatus::Ok => "OK",
        SensorStatus::Partial(_) => "Partial",
        SensorStatus::Unsupported => "N/A",
        SensorStatus::PermissionDenied => "Denied",
        SensorStatus::Stale => "Stale",
        SensorStatus::Error(_) => "Error",
    }
}

fn sensor_metric_compact_label(metric: &SensorMetric) -> String {
    let prefix = sensor_metric_kind_prefix(metric.kind);
    let label = metric.label.trim();
    if label.is_empty()
        || label.eq_ignore_ascii_case(metric.kind.default_label())
        || sensor_metric_label_is_generic(label)
    {
        prefix.to_owned()
    } else {
        format!("{prefix} {label}")
    }
}

fn sensor_metric_kind_prefix(kind: SensorMetricKind) -> &'static str {
    match kind {
        SensorMetricKind::Temperature => "Temp",
        SensorMetricKind::Utilization => "Util",
        SensorMetricKind::MemoryUsage => "Used",
        SensorMetricKind::Voltage => "Volt",
        SensorMetricKind::Power => "Power",
        SensorMetricKind::Clock => "Clock",
    }
}

fn sensor_missing_metric_label(reading: &SensorReading, kind: SensorMetricKind) -> &str {
    if reading.status == SensorStatus::Stale {
        return "--";
    }
    match kind {
        SensorMetricKind::Temperature => {
            if matches!(reading.kind, SensorKind::Cpu | SensorKind::Gpu) {
                "No safe temp"
            } else if matches!(reading.kind, SensorKind::GpuMemory) {
                "No VRAM temp"
            } else {
                "N/A"
            }
        }
        SensorMetricKind::Utilization => "N/A",
        SensorMetricKind::MemoryUsage
        | SensorMetricKind::Voltage
        | SensorMetricKind::Power
        | SensorMetricKind::Clock => "N/A",
    }
}

fn sensor_reading_tooltip(reading: &SensorReading) -> String {
    format!(
        "{}\nProvider: {}\nStatus: {}\nTemperature: {}\nUtilization: {}",
        reading.label,
        reading.provider,
        reading.status.detail(),
        format_sensor_temperature_detail(reading),
        format_utilization_value(reading.utilization_percent)
    )
}

fn sensor_metric_value_color(reading: &SensorReading, metric: &SensorMetric) -> egui::Color32 {
    if metric.kind == SensorMetricKind::Temperature {
        return temperature_color(reading.kind, metric.value, &reading.status);
    }
    if matches!(reading.status, SensorStatus::Ok | SensorStatus::Partial(_))
        && metric.value.is_some()
    {
        egui::Color32::WHITE
    } else {
        egui::Color32::GRAY
    }
}

fn sensor_metric_range_color(value: Option<f32>) -> egui::Color32 {
    if value.is_some() {
        egui::Color32::LIGHT_GRAY
    } else {
        egui::Color32::GRAY
    }
}

fn format_sensor_metric_current_value(metric: &SensorMetric, status: &SensorStatus) -> String {
    if *status == SensorStatus::Stale {
        return "--".to_owned();
    }
    if metric.kind == SensorMetricKind::MemoryUsage {
        return format_memory_usage_metric_value(metric);
    }
    format_sensor_metric_value(metric.kind, metric.value)
}

fn format_sensor_metric_value(kind: SensorMetricKind, value: Option<f32>) -> String {
    let Some(value) = value else {
        return "N/A".to_owned();
    };
    match kind {
        SensorMetricKind::Temperature => format!("{value:.0} C"),
        SensorMetricKind::Utilization => format!("{value:.0}%"),
        SensorMetricKind::MemoryUsage => format!("{value:.1} GB"),
        SensorMetricKind::Voltage => {
            if value.abs() < 10.0 {
                format!("{value:.3} V")
            } else {
                format!("{value:.2} V")
            }
        }
        SensorMetricKind::Power => format!("{value:.1} W"),
        SensorMetricKind::Clock => format!("{value:.0} MHz"),
    }
}

fn format_memory_usage_metric_value(metric: &SensorMetric) -> String {
    let Some(used_gb) = metric.value else {
        return "N/A".to_owned();
    };
    if let Some(total_gb) = metric.max.filter(|total| *total > 0.0) {
        format!("{used_gb:.1}/{total_gb:.1} GB")
    } else {
        format!("{used_gb:.1} GB")
    }
}

#[cfg(test)]
fn format_sensor_temperature(reading: &SensorReading) -> String {
    if reading.temperature_c.is_some() {
        return format_temperature_value(reading.temperature_c);
    }
    match reading.status {
        SensorStatus::Partial(_) if matches!(reading.kind, SensorKind::Cpu | SensorKind::Gpu) => {
            "No safe temp".to_owned()
        }
        SensorStatus::Partial(_) if matches!(reading.kind, SensorKind::GpuMemory) => {
            "No VRAM temp".to_owned()
        }
        _ => "N/A".to_owned(),
    }
}

fn format_sensor_temperature_detail(reading: &SensorReading) -> String {
    if reading.temperature_c.is_some() {
        format_temperature_value(reading.temperature_c)
    } else if matches!(reading.kind, SensorKind::Cpu | SensorKind::Gpu) {
        "No safe CPU/GPU temperature provider was found".to_owned()
    } else if matches!(reading.kind, SensorKind::GpuMemory) {
        "No safe VRAM temperature provider was found".to_owned()
    } else {
        "N/A".to_owned()
    }
}
