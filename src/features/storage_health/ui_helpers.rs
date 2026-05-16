fn storage_status_color(status: StorageHealthStatus) -> egui::Color32 {
    match status {
        StorageHealthStatus::Good => egui::Color32::GREEN,
        StorageHealthStatus::Caution => egui::Color32::YELLOW,
        StorageHealthStatus::Critical => egui::Color32::RED,
        StorageHealthStatus::Unknown => egui::Color32::GRAY,
    }
}

fn health_severity_color(severity: HealthSeverity) -> egui::Color32 {
    match severity {
        HealthSeverity::Info => egui::Color32::LIGHT_BLUE,
        HealthSeverity::Warning => egui::Color32::YELLOW,
        HealthSeverity::Critical => egui::Color32::RED,
    }
}

fn storage_metric(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(egui::RichText::new(label).strong());
    ui.label(value);
}

fn option_text(value: Option<&str>) -> &str {
    value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("N/A")
}
