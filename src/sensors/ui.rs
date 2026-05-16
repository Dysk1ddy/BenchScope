fn temperature_color(kind: SensorKind, value: Option<f32>, status: &SensorStatus) -> egui::Color32 {
    if !matches!(status, SensorStatus::Ok) {
        return egui::Color32::GRAY;
    }
    match value {
        Some(value) if value >= kind.critical_c() => egui::Color32::RED,
        Some(value) if value >= kind.warning_c() => egui::Color32::YELLOW,
        Some(_) => egui::Color32::WHITE,
        None => egui::Color32::GRAY,
    }
}

fn ui_sensor_row(ui: &mut egui::Ui, label: &str, reading: Option<&SensorReading>) {
    let (temperature, utilization, temperature_color, utilization_color, tooltip) =
        if let Some(reading) = reading {
            let (temperature, utilization) = if reading.status == SensorStatus::Stale {
                ("-- C".to_owned(), "--%".to_owned())
            } else {
                (
                    format_temperature_value(reading.temperature_c),
                    format_utilization_value(reading.utilization_percent),
                )
            };
            let tooltip = format!(
                "{}\nProvider: {}\nStatus: {}\nUtilization: {}",
                reading.label,
                reading.provider,
                reading.status.detail(),
                format_utilization_value(reading.utilization_percent)
            );
            (
                temperature,
                utilization,
                temperature_color(reading.kind, reading.temperature_c, &reading.status),
                if reading.utilization_percent.is_some() {
                    egui::Color32::WHITE
                } else {
                    egui::Color32::GRAY
                },
                tooltip,
            )
        } else {
            (
                "N/A".to_owned(),
                "N/A".to_owned(),
                egui::Color32::GRAY,
                egui::Color32::GRAY,
                "No sensor provider initialized".to_owned(),
            )
        };

    ui.horizontal(|ui| {
        ui.set_min_width(188.0);
        ui.label(egui::RichText::new(label).monospace());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(utilization)
                    .monospace()
                    .color(utilization_color),
            )
            .on_hover_text(tooltip.clone());
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(temperature)
                    .monospace()
                    .color(temperature_color),
            )
            .on_hover_text(tooltip);
        });
    });
}
