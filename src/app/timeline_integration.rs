impl BenchScopeApp {
    fn start_timeline(&mut self, scope: TimelineScope, title: impl Into<String>) {
        self.timeline.start(scope, title);
        self.observe_timeline_run(true);
    }

    fn observe_timeline_run(&mut self, force: bool) {
        let Some(scope) = self.timeline.active_scope() else {
            return;
        };
        let snapshot = self.sensors.latest();
        let throughput = self.current_timeline_throughput(scope);
        let phase = self.current_timeline_phase(scope);
        self.timeline.observe(&snapshot, throughput, phase, force);
    }

    fn finish_timeline_run(
        &mut self,
        scope: TimelineScope,
        final_throughput: Option<TimelineThroughputSample>,
        final_phase: impl Into<String>,
    ) -> Option<TimelineSummary> {
        if self.timeline.active_scope() != Some(scope) {
            return None;
        }
        let phase = final_phase.into();
        let snapshot = self.sensors.latest();
        self.timeline
            .observe(&snapshot, final_throughput, phase.clone(), true);
        let summary = self.timeline.finish(&phase)?;
        self.log(format!(
            "Thermal timeline: {} confidence, {} sample(s), {}",
            summary.confidence,
            summary.sample_count,
            summary
                .throughput_drop_percent
                .map(|drop| format!("{drop:.1}% throughput drop"))
                .unwrap_or_else(|| "no throughput drop estimate".to_owned())
        ));
        self.history
            .append_event(history_event_from_timeline_summary(&summary));
        Some(summary)
    }

    fn current_timeline_phase(&self, scope: TimelineScope) -> String {
        match scope {
            TimelineScope::MatrixStress => self.status.clone(),
        }
    }

    fn current_timeline_throughput(
        &self,
        scope: TimelineScope,
    ) -> Option<TimelineThroughputSample> {
        match scope {
            TimelineScope::MatrixStress => {
                let progress = self.repeat_progress.as_ref()?;
                progress
                    .throughput_tflops()
                    .map(|value| timeline_throughput("Compute throughput", value, "TFLOP/s"))
                    .or_else(|| {
                        progress.iterations_per_second().map(|value| {
                            timeline_throughput("Iterations", value, "iterations/s")
                        })
                    })
            }
        }
    }

    fn ui_timeline_panel(&mut self, ui: &mut egui::Ui, scope: TimelineScope) {
        ui.heading("Thermal Timeline");
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.timeline.show_temperatures, "Temp");
            ui.checkbox(&mut self.timeline.show_utilization, "Util");
            ui.separator();
            ui.checkbox(&mut self.timeline.show_cpu, "CPU");
            ui.checkbox(&mut self.timeline.show_gpu, "GPU");
            ui.checkbox(&mut self.timeline.show_vram, "VRAM");
            ui.checkbox(&mut self.timeline.show_drive, "SSD");
            ui.checkbox(&mut self.timeline.show_memory, "RAM");
        });

        let Some(timeline) = self.timeline.timeline_for_scope(scope).cloned() else {
            ui.label("Run a supported benchmark or stress test to collect thermal timeline data.");
            return;
        };
        ui.small(format!(
            "{} samples over {}",
            timeline.samples.len(),
            timeline
                .samples
                .last()
                .map(|sample| format_elapsed(sample.elapsed_ms as f64 / 1000.0))
                .unwrap_or_else(|| "0s".to_owned())
        ));
        ui_timeline_graph(ui, &timeline, &self.timeline);
        if let Some(summary) = self.timeline.summary_for_scope(scope) {
            ui_timeline_findings(ui, &summary);
        }
    }
}

#[derive(Clone)]
struct TimelineGraphSeries {
    label: String,
    color: egui::Color32,
    unit: String,
    values: Vec<(f64, f64)>,
}

fn ui_timeline_graph(ui: &mut egui::Ui, timeline: &RunTimeline, state: &TimelineState) {
    if timeline.samples.is_empty() {
        ui.label("No timeline samples yet.");
        return;
    }
    let series = timeline_graph_series(timeline, state);
    if series.is_empty() {
        ui.label("No enabled timeline series has data.");
        return;
    }

    let chart_width = ui.available_width().max(1.0);
    let compact_legend = chart_width < 520.0;
    let legend_labels = series
        .iter()
        .map(|graph_series| timeline_legend_label(graph_series, compact_legend))
        .collect::<Vec<_>>();
    let legend_font = egui::FontId::proportional(12.0);
    let axis_font = egui::FontId::proportional(12.0);
    let legend_widths = legend_labels
        .iter()
        .map(|label| {
            ui.painter()
                .layout_no_wrap(
                    label.clone(),
                    legend_font.clone(),
                    egui::Color32::from_rgb(190, 198, 210),
                )
                .size()
                .x
                + TIMELINE_LEGEND_SWATCH_WIDTH
                + TIMELINE_LEGEND_TEXT_GAP
        })
        .collect::<Vec<_>>();
    let legend_rows = timeline_legend_row_count(
        &legend_widths,
        (chart_width - TIMELINE_CHART_INNER_PADDING * 2.0).max(1.0),
    );
    let desired_size = egui::vec2(
        chart_width,
        TIMELINE_CHART_BASE_HEIGHT
            + legend_rows.saturating_sub(1) as f32 * TIMELINE_LEGEND_ROW_HEIGHT,
    );
    let (rect, _) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, egui::CornerRadius::same(6), egui::Color32::from_rgb(18, 21, 26));
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(6),
        egui::Stroke::new(1.0, egui::Color32::from_rgb(54, 61, 72)),
        egui::StrokeKind::Inside,
    );

    let left_margin = timeline_chart_left_margin(rect.width());
    let right_margin = if rect.width() < 320.0 { 8.0 } else { 14.0 };
    let legend_left = rect.left() + TIMELINE_CHART_INNER_PADDING;
    let legend_right = rect.right() - TIMELINE_CHART_INNER_PADDING;
    let legend_top = rect.top() + 8.0;
    let legend_items =
        timeline_legend_layout(&legend_widths, legend_left, legend_right, legend_top);
    let plot_top =
        legend_top + legend_rows as f32 * TIMELINE_LEGEND_ROW_HEIGHT + TIMELINE_PLOT_TOP_GAP;
    let plot = egui::Rect::from_min_max(
        egui::pos2(rect.left() + left_margin, plot_top),
        egui::pos2(rect.right() - right_margin, rect.bottom() - 42.0),
    );
    let max_x = timeline
        .samples
        .last()
        .map(|sample| sample.elapsed_ms as f64 / 1000.0)
        .unwrap_or(1.0)
        .max(1.0);
    for i in 0..=4 {
        let x = plot.left() + plot.width() * i as f32 / 4.0;
        painter.line_segment(
            [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
            egui::Stroke::new(0.7, egui::Color32::from_rgb(35, 40, 48)),
        );
    }
    for i in 0..=4 {
        let y = plot.bottom() - plot.height() * i as f32 / 4.0;
        painter.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            egui::Stroke::new(0.7, egui::Color32::from_rgb(35, 40, 48)),
        );
        painter.text(
            egui::pos2(plot.left() - 10.0, y),
            egui::Align2::RIGHT_CENTER,
            format!("{}", i * 25),
            axis_font.clone(),
            egui::Color32::from_rgb(170, 178, 190),
        );
    }

    for graph_series in &series {
        draw_timeline_series(&painter, plot, max_x, graph_series);
    }
    for ((item, graph_series), label) in legend_items.iter().zip(&series).zip(&legend_labels) {
        let swatch_y = item.y + TIMELINE_LEGEND_ROW_HEIGHT * 0.48;
        let swatch_width = TIMELINE_LEGEND_SWATCH_WIDTH.min(item.width);
        painter.line_segment(
            [
                egui::pos2(item.x, swatch_y),
                egui::pos2(item.x + swatch_width, swatch_y),
            ],
            egui::Stroke::new(2.0, graph_series.color),
        );
        painter.text(
            egui::pos2(
                item.x + TIMELINE_LEGEND_SWATCH_WIDTH + TIMELINE_LEGEND_TEXT_GAP,
                item.y,
            ),
            egui::Align2::LEFT_TOP,
            label,
            legend_font.clone(),
            graph_series.color,
        );
    }

    painter.line_segment(
        [plot.left_bottom(), plot.right_bottom()],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(76, 84, 96)),
    );
    painter.line_segment(
        [plot.left_bottom(), plot.left_top()],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(76, 84, 96)),
    );
    painter.text(
        egui::pos2(plot.left(), plot.bottom() + 6.0),
        egui::Align2::LEFT_TOP,
        "0s",
        axis_font.clone(),
        egui::Color32::from_rgb(170, 178, 190),
    );
    painter.text(
        egui::pos2(plot.right(), plot.bottom() + 6.0),
        egui::Align2::RIGHT_TOP,
        format_elapsed(max_x),
        axis_font.clone(),
        egui::Color32::from_rgb(170, 178, 190),
    );
    painter.text(
        egui::pos2(plot.center().x, rect.bottom() - 16.0),
        egui::Align2::CENTER_CENTER,
        "Elapsed time",
        axis_font.clone(),
        egui::Color32::from_rgb(190, 198, 210),
    );
    draw_rotated_timeline_label(
        &painter,
        egui::pos2(
            rect.left() + (left_margin * 0.24).clamp(14.0, 22.0),
            plot.center().y,
        ),
        "Temp C / Util %",
        axis_font,
        egui::Color32::from_rgb(190, 198, 210),
    );
}

#[derive(Clone, Copy, Debug)]
struct TimelineLegendItemLayout {
    x: f32,
    y: f32,
    width: f32,
}

const TIMELINE_CHART_BASE_HEIGHT: f32 = 288.0;
const TIMELINE_CHART_INNER_PADDING: f32 = 12.0;
const TIMELINE_LEGEND_ROW_HEIGHT: f32 = 17.0;
const TIMELINE_LEGEND_COLUMN_GAP: f32 = 14.0;
const TIMELINE_LEGEND_SWATCH_WIDTH: f32 = 14.0;
const TIMELINE_LEGEND_TEXT_GAP: f32 = 5.0;
const TIMELINE_PLOT_TOP_GAP: f32 = 8.0;

fn timeline_chart_left_margin(width: f32) -> f32 {
    if width < 300.0 {
        68.0
    } else if width < 420.0 {
        78.0
    } else {
        88.0
    }
}

fn timeline_legend_label(series: &TimelineGraphSeries, compact: bool) -> String {
    if !compact {
        return format!("{} ({})", series.label, series.unit);
    }

    if let Some(device) = series.label.strip_suffix(" temp") {
        return format!("{device} {}", series.unit);
    }
    if let Some(device) = series.label.strip_suffix(" util") {
        return format!("{device} {}", series.unit);
    }
    format!("{} {}", series.label, series.unit)
}

fn timeline_legend_row_count(item_widths: &[f32], available_width: f32) -> usize {
    if item_widths.is_empty() {
        return 0;
    }
    timeline_legend_layout(item_widths, 0.0, available_width.max(1.0), 0.0)
        .iter()
        .map(|item| ((item.y / TIMELINE_LEGEND_ROW_HEIGHT).round() as usize).saturating_add(1))
        .max()
        .unwrap_or(1)
}

fn timeline_legend_layout(
    item_widths: &[f32],
    left: f32,
    right: f32,
    top: f32,
) -> Vec<TimelineLegendItemLayout> {
    let available_width = (right - left).max(1.0);
    let mut items = Vec::with_capacity(item_widths.len());
    let mut x = left;
    let mut y = top;

    for width in item_widths {
        let item_width = width.min(available_width);
        if x > left && x + item_width > right {
            x = left;
            y += TIMELINE_LEGEND_ROW_HEIGHT;
        }
        items.push(TimelineLegendItemLayout {
            x,
            y,
            width: item_width,
        });
        x += item_width + TIMELINE_LEGEND_COLUMN_GAP;
    }

    items
}

fn draw_rotated_timeline_label(
    painter: &egui::Painter,
    center: egui::Pos2,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
) {
    let galley = painter.layout_no_wrap(text.to_owned(), font, color);
    let pos = center - galley.size() * 0.5;
    painter.add(egui::Shape::Text(
        egui::epaint::TextShape::new(pos, galley, color)
            .with_angle_and_anchor(-std::f32::consts::FRAC_PI_2, egui::Align2::CENTER_CENTER),
    ));
}

fn draw_timeline_series(
    painter: &egui::Painter,
    plot: egui::Rect,
    max_x: f64,
    series: &TimelineGraphSeries,
) {
    if series.values.len() < 2 {
        return;
    }
    let points = series
        .values
        .iter()
        .map(|(x, y)| {
            let x = plot.left() + ((*x / max_x).clamp(0.0, 1.0) as f32) * plot.width();
            let normalized_y = (*y / 100.0).clamp(0.0, 1.0) as f32;
            let y = plot.bottom() - normalized_y * plot.height();
            egui::pos2(x, y)
        })
        .collect::<Vec<_>>();
    painter.add(egui::Shape::line(
        points,
        egui::Stroke::new(1.8, series.color),
    ));
}

fn timeline_graph_series(timeline: &RunTimeline, state: &TimelineState) -> Vec<TimelineGraphSeries> {
    let mut series = Vec::new();
    if state.show_temperatures {
        push_timeline_series(
            &mut series,
            "CPU temp",
            "C",
            egui::Color32::from_rgb(243, 117, 94),
            timeline,
            state.show_cpu,
            |sample| sample.sensor.cpu_temp_c.map(f64::from),
        );
        push_timeline_series(
            &mut series,
            "GPU temp",
            "C",
            egui::Color32::from_rgb(255, 178, 89),
            timeline,
            state.show_gpu,
            |sample| sample.sensor.gpu_temp_c.map(f64::from),
        );
        push_timeline_series(
            &mut series,
            "VRAM temp",
            "C",
            egui::Color32::from_rgb(255, 214, 102),
            timeline,
            state.show_vram,
            |sample| sample.sensor.gpu_memory_temp_c.map(f64::from),
        );
    }
    if state.show_utilization {
        push_timeline_series(
            &mut series,
            "CPU util",
            "%",
            egui::Color32::from_rgb(114, 177, 240),
            timeline,
            state.show_cpu,
            |sample| sample.sensor.cpu_util_percent.map(f64::from),
        );
        push_timeline_series(
            &mut series,
            "GPU util",
            "%",
            egui::Color32::from_rgb(161, 136, 228),
            timeline,
            state.show_gpu,
            |sample| sample.sensor.gpu_util_percent.map(f64::from),
        );
        push_timeline_series(
            &mut series,
            "SSD util",
            "%",
            egui::Color32::from_rgb(89, 206, 224),
            timeline,
            state.show_drive,
            |sample| sample.sensor.drive_util_percent.map(f64::from),
        );
        push_timeline_series(
            &mut series,
            "RAM util",
            "%",
            egui::Color32::from_rgb(189, 145, 226),
            timeline,
            state.show_memory,
            |sample| sample.sensor.memory_util_percent.map(f64::from),
        );
    }
    series
}

fn push_timeline_series(
    series: &mut Vec<TimelineGraphSeries>,
    label: &str,
    unit: &str,
    color: egui::Color32,
    timeline: &RunTimeline,
    enabled: bool,
    value: impl Fn(&TimelineSample) -> Option<f64>,
) {
    if !enabled {
        return;
    }
    let values = timeline
        .samples
        .iter()
        .filter_map(|sample| {
            value(sample).map(|value| (sample.elapsed_ms as f64 / 1000.0, value))
        })
        .collect::<Vec<_>>();
    if values.len() >= 2 {
        series.push(TimelineGraphSeries {
            label: label.to_owned(),
            color,
            unit: unit.to_owned(),
            values,
        });
    }
}

fn ui_timeline_findings(ui: &mut egui::Ui, summary: &TimelineSummary) {
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("Confidence: {}", summary.confidence));
        ui.separator();
        ui.label(format!(
            "Peak CPU/GPU/VRAM/SSD: {} / {} / {} / {}",
            format_temperature_value(summary.peak_cpu_temp_c),
            format_temperature_value(summary.peak_gpu_temp_c),
            format_temperature_value(summary.peak_gpu_memory_temp_c),
            format_temperature_value(summary.peak_drive_temp_c)
        ));
        if let Some(drop) = summary.throughput_drop_percent {
            ui.separator();
            ui.label(format!("Throughput drop: {drop:.1}%"));
        }
    });
    for finding in &summary.findings {
        let color = match finding.severity.as_str() {
            "warning" => egui::Color32::YELLOW,
            "caution" => egui::Color32::from_rgb(255, 190, 100),
            _ => egui::Color32::from_rgb(180, 188, 200),
        };
        ui.colored_label(color, &finding.message);
    }
}
