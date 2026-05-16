fn battery_runtime_accuracy(
    samples: &VecDeque<BatteryLiveSample>,
) -> Option<BatteryRuntimeAccuracy> {
    let first = samples.front()?;
    let last = samples.back()?;
    let elapsed_minutes = last
        .captured_at
        .checked_duration_since(first.captured_at)?
        .as_secs_f64()
        / 60.0;
    if elapsed_minutes * 60.0 < BATTERY_RUNTIME_MIN_SAMPLE_SECONDS {
        return None;
    }
    let start_percent = first.percent? as f64;
    let end_percent = last.percent? as f64;
    let consumed = start_percent - end_percent;
    if consumed <= 0.5 {
        return None;
    }
    let observed_minutes = elapsed_minutes * (end_percent / consumed);
    let windows_minutes = last.windows_runtime_minutes?;
    if observed_minutes <= 0.0 || windows_minutes <= 0.0 {
        return None;
    }
    let error_percent = ((windows_minutes - observed_minutes).abs() / observed_minutes) * 100.0;
    let label = if error_percent <= 20.0 {
        "Good".to_owned()
    } else if error_percent <= 40.0 {
        "Fair".to_owned()
    } else {
        "Poor".to_owned()
    };
    Some(BatteryRuntimeAccuracy {
        label,
        error_percent,
        observed_minutes,
        windows_minutes,
    })
}

fn battery_health_percent(battery: Option<&BatteryInfo>) -> Option<f32> {
    let battery = battery?;
    let design = battery.design_capacity_mwh?;
    let full = battery.full_charge_capacity_mwh?;
    (design > 0.0).then(|| ((full / design) * 100.0).clamp(0.0, 150.0) as f32)
}

fn battery_wear_percent(battery: Option<&BatteryInfo>) -> Option<f32> {
    battery_health_percent(battery).map(|health| (100.0 - health).max(0.0))
}

fn battery_health_grade(health_percent: Option<f32>) -> BatteryHealthGrade {
    match health_percent {
        Some(value) if value >= 90.0 => BatteryHealthGrade::Excellent,
        Some(value) if value >= 75.0 => BatteryHealthGrade::Good,
        Some(value) if value >= 60.0 => BatteryHealthGrade::Fair,
        Some(value) if value >= 40.0 => BatteryHealthGrade::Poor,
        Some(_) => BatteryHealthGrade::Failed,
        None => BatteryHealthGrade::Unknown,
    }
}

fn battery_metric(ui: &mut egui::Ui, label: &str, value: impl Into<String>, color: egui::Color32) {
    ui.vertical(|ui| {
        ui.small(label);
        ui.label(egui::RichText::new(value.into()).strong().color(color));
    });
}

fn format_battery_percent(value: Option<f32>) -> String {
    format_optional_percent(value)
}

fn format_capacity_mwh(value: Option<f64>) -> String {
    format_optional_energy_mwh(value)
}

fn format_optional_watts(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.1} W"))
        .unwrap_or_else(|| "N/A".to_owned())
}

fn format_optional_minutes(value: Option<f64>) -> String {
    value
        .map(format_minutes)
        .unwrap_or_else(|| "N/A".to_owned())
}

fn format_minutes(value: f64) -> String {
    if value >= 120.0 {
        format!("{:.1} h", value / 60.0)
    } else {
        format!("{value:.0} min")
    }
}

fn draw_battery_capacity_graph(ui: &mut egui::Ui, points: &[BatteryCapacityPoint]) {
    if points.is_empty() {
        ui.label("No capacity history available.");
        return;
    }
    draw_line_graph(
        ui,
        "battery_capacity_graph",
        points
            .iter()
            .filter_map(|point| point.full_charge_capacity_mwh),
        points
            .iter()
            .filter_map(|point| point.design_capacity_mwh)
            .next(),
        "Full charge capacity history",
        "mWh",
    );
    egui::Grid::new("battery_capacity_history_grid")
        .striped(true)
        .num_columns(4)
        .show(ui, |ui| {
            result_header(ui, "Time");
            result_header(ui, "Design");
            result_header(ui, "Full charge");
            result_header(ui, "Cycles");
            ui.end_row();
            for point in points.iter().rev().take(8).rev() {
                result_cell(ui, point.label.as_str());
                result_cell(ui, format_capacity_mwh(point.design_capacity_mwh));
                result_cell(ui, format_capacity_mwh(point.full_charge_capacity_mwh));
                result_cell(
                    ui,
                    point
                        .cycle_count
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "N/A".to_owned()),
                );
                ui.end_row();
            }
        });
}

fn draw_battery_live_graph(ui: &mut egui::Ui, samples: &VecDeque<BatteryLiveSample>) {
    if samples.is_empty() {
        ui.label("No live battery samples collected.");
        return;
    }
    draw_line_graph(
        ui,
        "battery_live_charge_graph",
        samples
            .iter()
            .filter_map(|sample| sample.percent.map(f64::from)),
        None,
        "Live charge samples",
        "%",
    );
    let latest = samples.back();
    ui.label(format!(
        "Samples: {}; latest charge: {}",
        samples.len(),
        latest
            .map(|sample| format_optional_percent(sample.percent))
            .unwrap_or_else(|| "N/A".to_owned())
    ));
}

fn run_battery_report_scan(
    duration: BatteryReportDuration,
    cancel: Arc<AtomicBool>,
) -> Result<BatteryReport> {
    check_canceled_with(Some(&cancel), "Battery scan canceled")?;

    #[cfg(windows)]
    {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let report_path = std::env::temp_dir().join(format!(
            "benchscope_batteryreport-{}-{timestamp}.xml",
            std::process::id()
        ));
        let report_path_text = report_path.display().to_string();
        let duration_text = duration.days().to_string();
        let mut command = Command::new("powercfg");
        command.args([
            "/batteryreport",
            "/output",
            &report_path_text,
            "/xml",
            "/duration",
            &duration_text,
        ]);
        command.creation_flags(CREATE_NO_WINDOW_RAW);
        let output = command.output().context("failed to start powercfg")?;
        check_canceled_with(Some(&cancel), "Battery scan canceled")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(anyhow!(
                "powercfg /batteryreport failed{}",
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            ));
        }
        let xml = fs::read_to_string(&report_path)
            .with_context(|| format!("could not read {}", report_path.display()))?;
        let _ = fs::remove_file(&report_path);
        let mut report = parse_battery_report_xml(&xml);
        report
            .notes
            .push(format!("Parsed powercfg battery report for {}.", duration));
        let live_sample = collect_battery_live_sample(report.full_charge_capacity_mwh()).ok();
        if let Some(sample) = live_sample {
            report.live_sample = Some(sample);
        } else {
            report
                .notes
                .push("Live Windows battery sample was unavailable.".to_owned());
        }
        report
            .notes
            .push("Software cannot directly detect physical battery swelling.".to_owned());
        report.warnings = battery_report_warnings(&report);
        Ok(report)
    }

    #[cfg(not(windows))]
    {
        let _ = duration;
        Err(anyhow!(
            "battery report generation is currently implemented for Windows"
        ))
    }
}

fn collect_battery_live_sample(full_charge_capacity_mwh: Option<f64>) -> Result<BatteryLiveSample> {
    #[cfg(windows)]
    {
        let script = r#"
$b = Get-CimInstance Win32_Battery -ErrorAction SilentlyContinue | Select-Object -First 1
$s = Get-CimInstance -Namespace root/wmi -ClassName BatteryStatus -ErrorAction SilentlyContinue | Select-Object -First 1
if ($null -eq $b -and $null -eq $s) { exit 2 }
$fields = @(
    $b.EstimatedChargeRemaining,
    $b.EstimatedRunTime,
    $b.BatteryStatus,
    $s.RemainingCapacity,
    $s.ChargeRate,
    $s.DischargeRate,
    $s.PowerOnline,
    $s.Charging,
    $s.Discharging
)
($fields | ForEach-Object { if ($null -eq $_) { '' } else { $_ } }) -join "`t"
"#;
        let output =
            run_powershell_sensor_script(script).context("failed to query battery state")?;
        let fields = output.split('\t').map(str::trim).collect::<Vec<_>>();
        if fields.is_empty() || fields.iter().all(|field| field.is_empty()) {
            return Err(anyhow!("No battery detected"));
        }
        let percent = fields.first().and_then(|value| parse_text_f32(value));
        let runtime_minutes = fields
            .get(1)
            .and_then(|value| parse_optional_f64(value))
            .filter(|minutes| *minutes > 0.0 && *minutes < 10_080.0);
        let battery_status = fields.get(2).and_then(|value| parse_optional_u32(value));
        let wmi_remaining = fields.get(3).and_then(|value| parse_optional_f64(value));
        let charge_rate_watts = fields
            .get(4)
            .and_then(|value| parse_optional_f64(value))
            .and_then(milliwatts_to_watts);
        let discharge_rate_watts = fields
            .get(5)
            .and_then(|value| parse_optional_f64(value))
            .and_then(milliwatts_to_watts);
        let power_online = fields.get(6).and_then(|value| parse_text_bool(value));
        let charging = fields.get(7).and_then(|value| parse_text_bool(value));
        let discharging = fields.get(8).and_then(|value| parse_text_bool(value));

        let status_code = battery_status.unwrap_or(0);
        let mut status = battery_status_label(status_code).to_owned();
        if charging == Some(true) {
            status = "Charging".to_owned();
        } else if discharging == Some(true) {
            status = "Discharging".to_owned();
        }
        let ac_connected = power_online.or(match status_code {
            1 | 4 | 5 => Some(false),
            2 | 3 | 6 | 7 | 8 | 9 | 11 => Some(true),
            _ => None,
        });
        let remaining_capacity_mwh = wmi_remaining.or_else(|| {
            percent.and_then(|percent| {
                full_charge_capacity_mwh.map(|full| full * percent as f64 / 100.0)
            })
        });
        Ok(BatteryLiveSample {
            captured_at: Instant::now(),
            ac_connected,
            status,
            percent,
            remaining_capacity_mwh,
            charge_rate_watts,
            discharge_rate_watts,
            windows_runtime_minutes: runtime_minutes,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = full_charge_capacity_mwh;
        Err(anyhow!(
            "battery live sampling is currently implemented for Windows"
        ))
    }
}

fn parse_battery_report_xml(xml: &str) -> BatteryReport {
    let lines = xml.lines().map(str::trim).collect::<Vec<_>>();
    let mut report = BatteryReport {
        generated_at: None,
        batteries: Vec::new(),
        capacity_history: Vec::new(),
        recent_usage: Vec::new(),
        live_sample: None,
        warnings: Vec::new(),
        notes: Vec::new(),
    };
    let mut in_battery = false;
    let mut current_battery = BatteryInfo::default();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        if report.generated_at.is_none() {
            if let Some(value) =
                xml_tag_text(line, "LocalScanTime").or_else(|| xml_tag_text(line, "ScanTime"))
            {
                report.generated_at = Some(value);
            }
        }

        if line == "<Battery>" {
            in_battery = true;
            current_battery = BatteryInfo::default();
            index += 1;
            continue;
        }
        if line == "</Battery>" {
            in_battery = false;
            report.batteries.push(current_battery.clone());
            index += 1;
            continue;
        }

        if in_battery {
            if let Some(value) = xml_tag_text(line, "Id") {
                current_battery.id = non_empty_string(value);
            } else if let Some(value) = xml_tag_text(line, "Manufacturer") {
                current_battery.manufacturer = non_empty_string(value);
            } else if let Some(value) = xml_tag_text(line, "SerialNumber") {
                current_battery.serial_number = non_empty_string(value);
            } else if let Some(value) = xml_tag_text(line, "Chemistry") {
                current_battery.chemistry = non_empty_string(value);
            } else if let Some(value) = xml_tag_text(line, "DesignCapacity") {
                current_battery.design_capacity_mwh = parse_capacity_mwh(&value);
            } else if let Some(value) = xml_tag_text(line, "FullChargeCapacity") {
                current_battery.full_charge_capacity_mwh = parse_capacity_mwh(&value);
            } else if let Some(value) = xml_tag_text(line, "CycleCount") {
                current_battery.cycle_count = parse_optional_u32(&value);
            }
        }

        if line.starts_with("<HistoryEntry") {
            let (element, next_index) = collect_xml_element(&lines, index);
            let attrs = xml_attributes(&element);
            report.capacity_history.push(BatteryCapacityPoint {
                label: attrs
                    .get("LocalStartDate")
                    .or_else(|| attrs.get("StartDate"))
                    .cloned()
                    .unwrap_or_else(|| "Unknown".to_owned()),
                design_capacity_mwh: attrs
                    .get("DesignCapacity")
                    .and_then(|value| parse_capacity_mwh(value)),
                full_charge_capacity_mwh: attrs
                    .get("FullChargeCapacity")
                    .and_then(|value| parse_capacity_mwh(value)),
                cycle_count: attrs
                    .get("CycleCount")
                    .and_then(|value| parse_optional_u32(value)),
            });
            index = next_index;
            continue;
        }

        if line.starts_with("<UsageEntry") {
            let (element, next_index) = collect_xml_element(&lines, index);
            let attrs = xml_attributes(&element);
            report.recent_usage.push(BatteryUsagePoint {
                label: attrs
                    .get("LocalTimestamp")
                    .or_else(|| attrs.get("Timestamp"))
                    .cloned()
                    .unwrap_or_else(|| "Unknown".to_owned()),
                ac_connected: attrs.get("Ac").and_then(|value| parse_xml_bool(value)),
                charge_capacity_mwh: attrs
                    .get("ChargeCapacity")
                    .and_then(|value| parse_capacity_mwh(value)),
                discharge_mwh: attrs
                    .get("Discharge")
                    .and_then(|value| parse_capacity_mwh(value)),
                full_charge_capacity_mwh: attrs
                    .get("FullChargeCapacity")
                    .and_then(|value| parse_capacity_mwh(value)),
            });
            index = next_index;
            continue;
        }

        index += 1;
    }

    if report.batteries.is_empty() {
        report
            .notes
            .push("No installed batteries were present in the powercfg report.".to_owned());
    }
    if report.capacity_history.is_empty() {
        report
            .notes
            .push("No capacity history was present in the powercfg report.".to_owned());
    }
    report
}

fn battery_report_warnings(report: &BatteryReport) -> Vec<BatteryWarning> {
    let mut warnings = Vec::new();
    if report.batteries.is_empty() {
        warnings.push(BatteryWarning {
            severity: BatteryWarningSeverity::Info,
            title: "No battery detected".to_owned(),
            detail: "This diagnostic is intended for laptops with an installed battery.".to_owned(),
        });
        return warnings;
    }

    let battery = report.primary_battery();
    match battery_health_percent(battery) {
        Some(health) if health < 60.0 => warnings.push(BatteryWarning {
            severity: BatteryWarningSeverity::Critical,
            title: "Severe capacity wear".to_owned(),
            detail: format!("Full charge capacity is about {health:.1}% of design."),
        }),
        Some(health) if health < 80.0 => warnings.push(BatteryWarning {
            severity: BatteryWarningSeverity::Warning,
            title: "Reduced battery health".to_owned(),
            detail: format!("Full charge capacity is about {health:.1}% of design."),
        }),
        Some(health) if health > 105.0 => warnings.push(BatteryWarning {
            severity: BatteryWarningSeverity::Info,
            title: "Capacity above design".to_owned(),
            detail:
                "Full charge capacity is above design capacity; this can be normal on some packs."
                    .to_owned(),
        }),
        None => warnings.push(BatteryWarning {
            severity: BatteryWarningSeverity::Info,
            title: "Capacity health unavailable".to_owned(),
            detail: "Design or full charge capacity was not exposed by the report.".to_owned(),
        }),
        _ => {}
    }

    if let Some(cycles) = battery.and_then(|battery| battery.cycle_count) {
        if cycles >= 600 {
            warnings.push(BatteryWarning {
                severity: BatteryWarningSeverity::Warning,
                title: "High cycle count".to_owned(),
                detail: format!("{cycles} cycles reported by battery firmware."),
            });
        }
    }

    if let (Some(previous), Some(latest)) = (
        report
            .capacity_history
            .iter()
            .rev()
            .nth(1)
            .and_then(|point| point.full_charge_capacity_mwh),
        report
            .capacity_history
            .iter()
            .rev()
            .next()
            .and_then(|point| point.full_charge_capacity_mwh),
    ) {
        if previous > 0.0 && latest < previous * 0.9 {
            warnings.push(BatteryWarning {
                severity: BatteryWarningSeverity::Warning,
                title: "Sharp capacity drop".to_owned(),
                detail: format!(
                    "Recent full charge capacity dropped from {:.0} mWh to {:.0} mWh.",
                    previous, latest
                ),
            });
        }
    }

    warnings
}

fn draw_line_graph<I>(
    ui: &mut egui::Ui,
    id: &str,
    values: I,
    reference_value: Option<f64>,
    label: &str,
    unit: &str,
) where
    I: IntoIterator<Item = f64>,
{
    let values = values
        .into_iter()
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    if values.is_empty() {
        ui.label(format!("{label}: no plottable values."));
        return;
    }
    let desired_size = egui::vec2(ui.available_width().max(280.0), 150.0);
    let (rect, _) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let stroke = egui::Stroke::new(1.5, egui::Color32::LIGHT_BLUE);
    let axis_stroke = egui::Stroke::new(1.0, egui::Color32::DARK_GRAY);

    let min_value = values
        .iter()
        .copied()
        .chain(reference_value)
        .fold(f64::INFINITY, f64::min);
    let max_value = values
        .iter()
        .copied()
        .chain(reference_value)
        .fold(f64::NEG_INFINITY, f64::max);
    let span = (max_value - min_value).max(1.0);
    let plot_rect = rect.shrink2(egui::vec2(8.0, 18.0));
    painter.line_segment(
        [plot_rect.left_bottom(), plot_rect.right_bottom()],
        axis_stroke,
    );
    painter.line_segment([plot_rect.left_top(), plot_rect.left_bottom()], axis_stroke);

    if let Some(reference) = reference_value {
        let y = plot_rect.bottom()
            - (((reference - min_value) / span) as f32).clamp(0.0, 1.0) * plot_rect.height();
        painter.line_segment(
            [
                egui::pos2(plot_rect.left(), y),
                egui::pos2(plot_rect.right(), y),
            ],
            egui::Stroke::new(1.0, egui::Color32::GRAY),
        );
    }

    if values.len() == 1 {
        let y = plot_rect.bottom()
            - (((values[0] - min_value) / span) as f32).clamp(0.0, 1.0) * plot_rect.height();
        painter.circle_filled(
            egui::pos2(plot_rect.center().x, y),
            3.0,
            egui::Color32::LIGHT_BLUE,
        );
    } else {
        for index in 1..values.len() {
            let x0 = plot_rect.left()
                + ((index - 1) as f32 / (values.len() - 1) as f32) * plot_rect.width();
            let x1 =
                plot_rect.left() + (index as f32 / (values.len() - 1) as f32) * plot_rect.width();
            let y0 = plot_rect.bottom()
                - (((values[index - 1] - min_value) / span) as f32).clamp(0.0, 1.0)
                    * plot_rect.height();
            let y1 = plot_rect.bottom()
                - (((values[index] - min_value) / span) as f32).clamp(0.0, 1.0)
                    * plot_rect.height();
            painter.line_segment([egui::pos2(x0, y0), egui::pos2(x1, y1)], stroke);
        }
    }
    painter.text(
        rect.left_top() + egui::vec2(4.0, 2.0),
        egui::Align2::LEFT_TOP,
        format!("{label}: {:.0}-{:.0} {unit}", min_value, max_value),
        egui::FontId::proportional(12.0),
        egui::Color32::GRAY,
    );
    ui.memory_mut(|memory| memory.data.insert_temp(egui::Id::new(id), values.len()));
}

fn xml_tag_text(line: &str, tag: &str) -> Option<String> {
    let start_tag = format!("<{tag}>");
    let end_tag = format!("</{tag}>");
    let start = line.find(&start_tag)? + start_tag.len();
    let end = line[start..].find(&end_tag)? + start;
    Some(decode_xml_entities(line[start..end].trim()))
}

fn collect_xml_element(lines: &[&str], start_index: usize) -> (String, usize) {
    let mut text = String::new();
    let mut index = start_index;
    while index < lines.len() {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(lines[index]);
        index += 1;
        if text.contains("/>") || text.contains("</") {
            break;
        }
    }
    (text, index)
}

fn xml_attributes(element: &str) -> HashMap<String, String> {
    let bytes = element.as_bytes();
    let mut attrs = HashMap::new();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && !bytes[index].is_ascii_alphabetic() {
            index += 1;
        }
        let key_start = index;
        while index < bytes.len()
            && (bytes[index].is_ascii_alphanumeric()
                || bytes[index] == b'_'
                || bytes[index] == b'-')
        {
            index += 1;
        }
        if key_start == index {
            break;
        }
        let key = &element[key_start..index];
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] != b'=' {
            continue;
        }
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() || (bytes[index] != b'"' && bytes[index] != b'\'') {
            continue;
        }
        let quote = bytes[index];
        index += 1;
        let value_start = index;
        while index < bytes.len() && bytes[index] != quote {
            index += 1;
        }
        if index <= bytes.len() {
            attrs.insert(
                key.to_owned(),
                decode_xml_entities(element[value_start..index].trim()),
            );
        }
        index += 1;
    }
    attrs
}

fn decode_xml_entities(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn non_empty_string(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn parse_capacity_mwh(value: &str) -> Option<f64> {
    let cleaned = value
        .chars()
        .filter(|ch| ch.is_ascii_digit() || *ch == '.' || *ch == '-')
        .collect::<String>();
    parse_optional_f64(&cleaned).filter(|value| *value >= 0.0)
}

fn parse_optional_f64(value: &str) -> Option<f64> {
    let value = value.trim();
    if value.is_empty() || value == "-" {
        None
    } else {
        value.parse::<f64>().ok()
    }
}

fn parse_text_f32(value: &str) -> Option<f32> {
    parse_optional_f64(value).map(|value| value as f32)
}

fn parse_optional_u32(value: &str) -> Option<u32> {
    let value = value.trim();
    if value.is_empty() || value == "-" {
        None
    } else {
        value.parse::<u32>().ok()
    }
}

fn parse_text_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

fn parse_xml_bool(value: &str) -> Option<bool> {
    parse_text_bool(value)
}

fn milliwatts_to_watts(value: f64) -> Option<f64> {
    (value > 0.0 && value < 1_000_000.0).then_some(value / 1000.0)
}

fn battery_status_label(status: u32) -> &'static str {
    match status {
        1 => "Discharging",
        2 => "AC connected",
        3 => "Fully charged",
        4 => "Low",
        5 => "Critical",
        6 => "Charging",
        7 => "Charging high",
        8 => "Charging low",
        9 => "Charging critical",
        10 => "Undefined",
        11 => "Partially charged",
        _ => "Unknown",
    }
}

