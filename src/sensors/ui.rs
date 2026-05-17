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

fn ui_sensor_table(ui: &mut egui::Ui, rows: &[(&str, Option<&SensorReading>)]) {
    egui::Grid::new("sensor_metrics_grid")
        .num_columns(4)
        .spacing(egui::vec2(14.0, 3.0))
        .striped(false)
        .show(ui, |ui| {
            ui.label("");
            sensor_header_cell(ui, "Value");
            sensor_header_cell(ui, "Min");
            sensor_header_cell(ui, "Max");
            ui.end_row();

            for (device_label, reading) in rows {
                ui_sensor_device_rows(ui, device_label, *reading);
            }
        });
}

fn sensor_header_cell(ui: &mut egui::Ui, label: &str) {
    ui.label(
        egui::RichText::new(label)
            .small()
            .strong()
            .monospace()
            .color(egui::Color32::LIGHT_GRAY),
    );
}

fn ui_sensor_device_rows(ui: &mut egui::Ui, label: &str, reading: Option<&SensorReading>) {
    let tooltip = reading
        .map(sensor_reading_tooltip)
        .unwrap_or_else(|| "No sensor provider initialized".to_owned());
    ui.label(egui::RichText::new(label).strong().monospace())
        .on_hover_text(tooltip.clone());
    ui.label(
        egui::RichText::new(
            reading
                .map(|reading| reading.status.detail())
                .unwrap_or_else(|| "N/A".to_owned()),
        )
        .small()
        .color(if reading.is_some_and(SensorReading::is_ok) {
            egui::Color32::LIGHT_GRAY
        } else {
            egui::Color32::GRAY
        }),
    )
    .on_hover_text(tooltip.clone());
    ui.label("");
    ui.label("");
    ui.end_row();

    let Some(reading) = reading else {
        ui_sensor_metric_placeholder(ui, "Provider", "N/A", &tooltip);
        return;
    };

    let mut rendered_any = false;
    for kind in [
        SensorMetricKind::Temperature,
        SensorMetricKind::Utilization,
        SensorMetricKind::Voltage,
        SensorMetricKind::Power,
        SensorMetricKind::Clock,
    ] {
        let metrics = reading.metrics_for(kind).collect::<Vec<_>>();
        if metrics.is_empty() {
            if matches!(
                kind,
                SensorMetricKind::Temperature | SensorMetricKind::Utilization
            ) {
                ui_sensor_metric_placeholder(
                    ui,
                    kind.group_label(),
                    sensor_missing_metric_label(reading, kind),
                    &tooltip,
                );
                rendered_any = true;
            }
            continue;
        }

        ui.label(
            egui::RichText::new(kind.group_label())
                .small()
                .strong()
                .color(egui::Color32::LIGHT_GRAY),
        );
        ui.label("");
        ui.label("");
        ui.label("");
        ui.end_row();

        for metric in metrics {
            ui_sensor_metric_row(ui, reading, metric, &tooltip);
            rendered_any = true;
        }
    }

    if !rendered_any {
        ui_sensor_metric_placeholder(ui, "Status", &reading.status.detail(), &tooltip);
    }
}

fn ui_sensor_metric_row(
    ui: &mut egui::Ui,
    reading: &SensorReading,
    metric: &SensorMetric,
    tooltip: &str,
) {
    ui.label(egui::RichText::new(&metric.label).small().monospace())
        .on_hover_text(tooltip.to_owned());
    ui.label(
        egui::RichText::new(format_sensor_metric_value_for_status(
            metric.kind,
            metric.value,
            &reading.status,
            true,
        ))
        .small()
        .monospace()
        .color(sensor_metric_value_color(reading, metric)),
    )
    .on_hover_text(tooltip.to_owned());
    ui.label(
        egui::RichText::new(format_sensor_metric_value(metric.kind, metric.min))
            .small()
            .monospace()
            .color(sensor_metric_range_color(metric.min)),
    )
    .on_hover_text(tooltip.to_owned());
    ui.label(
        egui::RichText::new(format_sensor_metric_value(metric.kind, metric.max))
            .small()
            .monospace()
            .color(sensor_metric_range_color(metric.max)),
    )
    .on_hover_text(tooltip.to_owned());
    ui.end_row();
}

fn ui_sensor_metric_placeholder(ui: &mut egui::Ui, label: &str, value: &str, tooltip: &str) {
    ui.label(egui::RichText::new(label).small().monospace())
        .on_hover_text(tooltip.to_owned());
    ui.label(
        egui::RichText::new(value)
            .small()
            .monospace()
            .color(egui::Color32::GRAY),
    )
    .on_hover_text(tooltip.to_owned());
    ui.label(egui::RichText::new("N/A").small().monospace().color(egui::Color32::GRAY));
    ui.label(egui::RichText::new("N/A").small().monospace().color(egui::Color32::GRAY));
    ui.end_row();
}

fn sensor_missing_metric_label(reading: &SensorReading, kind: SensorMetricKind) -> &str {
    if reading.status == SensorStatus::Stale {
        return "--";
    }
    match kind {
        SensorMetricKind::Temperature => {
            if matches!(reading.kind, SensorKind::Cpu | SensorKind::Gpu) {
                "No safe temp"
            } else {
                "N/A"
            }
        }
        SensorMetricKind::Utilization => "N/A",
        SensorMetricKind::Voltage | SensorMetricKind::Power | SensorMetricKind::Clock => "N/A",
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

fn format_sensor_metric_value_for_status(
    kind: SensorMetricKind,
    value: Option<f32>,
    status: &SensorStatus,
    is_current_value: bool,
) -> String {
    if is_current_value && *status == SensorStatus::Stale {
        return "--".to_owned();
    }
    format_sensor_metric_value(kind, value)
}

fn format_sensor_metric_value(kind: SensorMetricKind, value: Option<f32>) -> String {
    let Some(value) = value else {
        return "N/A".to_owned();
    };
    match kind {
        SensorMetricKind::Temperature => format!("{value:.0} C"),
        SensorMetricKind::Utilization => format!("{value:.0}%"),
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

fn format_sensor_temperature(reading: &SensorReading) -> String {
    if reading.temperature_c.is_some() {
        return format_temperature_value(reading.temperature_c);
    }
    match reading.status {
        SensorStatus::Partial(_) if matches!(reading.kind, SensorKind::Cpu | SensorKind::Gpu) => {
            "No safe temp".to_owned()
        }
        _ => "N/A".to_owned(),
    }
}

fn format_sensor_temperature_detail(reading: &SensorReading) -> String {
    if reading.temperature_c.is_some() {
        format_temperature_value(reading.temperature_c)
    } else if matches!(reading.kind, SensorKind::Cpu | SensorKind::Gpu) {
        "No safe CPU/GPU temperature provider was found".to_owned()
    } else {
        "N/A".to_owned()
    }
}
