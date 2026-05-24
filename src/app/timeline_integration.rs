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
            TimelineScope::MatrixBenchmark => self.status.clone(),
            TimelineScope::MatrixStress => self.status.clone(),
            TimelineScope::GpuMemory => self.gpu_memory.status.clone(),
            TimelineScope::DriveBenchmark => self.drive.status.clone(),
            TimelineScope::AiTraining => {
                if self.ai_training.phase.trim().is_empty() {
                    self.ai_training.status.clone()
                } else {
                    self.ai_training.phase.clone()
                }
            }
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
            TimelineScope::GpuMemory => {
                (self.gpu_memory.running
                    && self.gpu_memory.timeline_elapsed_s > 0.0
                    && self.gpu_memory.timeline_bytes_processed > 0)
                    .then(|| {
                        timeline_throughput(
                            "GPU memory bandwidth",
                            self.gpu_memory.timeline_bytes_processed as f64
                                / self.gpu_memory.timeline_elapsed_s
                                / 1_000_000_000.0,
                            "GB/s",
                        )
                    })
            }
            TimelineScope::DriveBenchmark => {
                (self.drive.running
                    && self.drive.timeline_elapsed_s > 0.0
                    && self.drive.timeline_bytes_processed > 0)
                    .then(|| {
                        timeline_throughput(
                            "Drive throughput",
                            self.drive.timeline_bytes_processed as f64
                                / self.drive.timeline_elapsed_s
                                / 1_000_000.0,
                            "MB/s",
                        )
                    })
            }
            TimelineScope::AiTraining => {
                (self.ai_training.running
                    && self.ai_training.timeline_elapsed_s > 0.0
                    && self.ai_training.timeline_completed_steps > 0)
                    .then(|| {
                        timeline_throughput(
                            "Training progress",
                            self.ai_training.timeline_completed_steps as f64
                                / self.ai_training.timeline_elapsed_s,
                            "steps/s",
                        )
                    })
            }
            TimelineScope::MatrixBenchmark => None,
        }
    }

    fn ui_timeline_panel(&mut self, ui: &mut egui::Ui, scope: TimelineScope) {
        ui.heading("Thermal Timeline");
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.timeline.show_temperatures, "Temp");
            ui.checkbox(&mut self.timeline.show_utilization, "Util");
            ui.checkbox(&mut self.timeline.show_clocks, "Clock");
            ui.checkbox(&mut self.timeline.show_power, "Power");
            ui.checkbox(&mut self.timeline.show_throughput, "Throughput");
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

    let desired_size = egui::vec2(ui.available_width().max(360.0), 260.0);
    let (rect, _) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, egui::CornerRadius::same(6), egui::Color32::from_rgb(18, 21, 26));
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(6),
        egui::Stroke::new(1.0, egui::Color32::from_rgb(54, 61, 72)),
        egui::StrokeKind::Inside,
    );

    let plot = rect.shrink2(egui::vec2(12.0, 28.0));
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
    for i in 0..=3 {
        let y = plot.top() + plot.height() * i as f32 / 3.0;
        painter.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            egui::Stroke::new(0.7, egui::Color32::from_rgb(35, 40, 48)),
        );
    }

    let mut legend_x = plot.left();
    let legend_y = rect.top() + 8.0;
    for graph_series in &series {
        draw_timeline_series(&painter, plot, max_x, graph_series);
        let label = format!("{} ({})", graph_series.label, graph_series.unit);
        painter.text(
            egui::pos2(legend_x, legend_y),
            egui::Align2::LEFT_TOP,
            label,
            egui::FontId::proportional(12.0),
            graph_series.color,
        );
        legend_x += 118.0;
        if legend_x > plot.right() - 110.0 {
            legend_x = plot.left();
        }
    }

    painter.text(
        egui::pos2(plot.left(), plot.bottom() + 6.0),
        egui::Align2::LEFT_TOP,
        "0s",
        egui::FontId::proportional(12.0),
        egui::Color32::from_rgb(170, 178, 190),
    );
    painter.text(
        egui::pos2(plot.right(), plot.bottom() + 6.0),
        egui::Align2::RIGHT_TOP,
        format_elapsed(max_x),
        egui::FontId::proportional(12.0),
        egui::Color32::from_rgb(170, 178, 190),
    );
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
    let min_y = series
        .values
        .iter()
        .map(|(_, value)| *value)
        .fold(f64::INFINITY, f64::min);
    let max_y = series
        .values
        .iter()
        .map(|(_, value)| *value)
        .fold(f64::NEG_INFINITY, f64::max);
    let span = (max_y - min_y).max(1.0);
    let points = series
        .values
        .iter()
        .map(|(x, y)| {
            let x = plot.left() + ((*x / max_x).clamp(0.0, 1.0) as f32) * plot.width();
            let normalized_y = ((*y - min_y) / span).clamp(0.0, 1.0) as f32;
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
        push_timeline_series(
            &mut series,
            "SSD temp",
            "C",
            egui::Color32::from_rgb(111, 203, 166),
            timeline,
            state.show_drive,
            |sample| sample.sensor.drive_temp_c.map(f64::from),
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
    if state.show_clocks {
        push_timeline_series(
            &mut series,
            "CPU clock",
            "MHz",
            egui::Color32::from_rgb(96, 201, 177),
            timeline,
            state.show_cpu,
            |sample| sample.sensor.cpu_clock_mhz.map(f64::from),
        );
        push_timeline_series(
            &mut series,
            "GPU clock",
            "MHz",
            egui::Color32::from_rgb(222, 147, 216),
            timeline,
            state.show_gpu,
            |sample| sample.sensor.gpu_clock_mhz.map(f64::from),
        );
    }
    if state.show_power {
        push_timeline_series(
            &mut series,
            "CPU power",
            "W",
            egui::Color32::from_rgb(224, 118, 122),
            timeline,
            state.show_cpu,
            |sample| sample.sensor.cpu_power_w.map(f64::from),
        );
        push_timeline_series(
            &mut series,
            "GPU power",
            "W",
            egui::Color32::from_rgb(224, 176, 105),
            timeline,
            state.show_gpu,
            |sample| sample.sensor.gpu_power_w.map(f64::from),
        );
    }
    if state.show_throughput {
        push_timeline_series(
            &mut series,
            "Throughput",
            timeline
                .samples
                .iter()
                .find_map(|sample| sample.throughput.as_ref().map(|value| value.unit.as_str()))
                .unwrap_or("rate"),
            egui::Color32::from_rgb(145, 214, 111),
            timeline,
            true,
            |sample| sample.throughput.as_ref().map(|value| value.value),
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

fn drive_result_timeline_throughput(
    result: &DriveBenchmarkResult,
) -> Option<TimelineThroughputSample> {
    let value = if result.test.is_read() {
        result.read_mbps
    } else {
        result.write_mbps
    }?;
    Some(timeline_throughput(result.test.label(), value, "MB/s"))
}

fn gpu_memory_result_timeline_throughput(
    result: &GpuMemoryBenchmarkResult,
) -> TimelineThroughputSample {
    timeline_throughput(
        result.test.label(),
        result.average_bandwidth_gbps,
        "GB/s",
    )
}

fn ai_training_result_timeline_throughput(
    result: &AiTrainingResult,
) -> Option<TimelineThroughputSample> {
    result
        .throughput_value
        .map(|value| timeline_throughput(result.workload.label(), value, result.throughput_label))
}

fn matrix_result_timeline_throughput(result: &BenchmarkResult) -> TimelineThroughputSample {
    let n = result.size as f64;
    let flops = 2.0 * n * n * n;
    let value = if result.gpu_total_ms > 0.0 {
        flops / (result.gpu_total_ms / 1000.0) / 1.0e12
    } else {
        0.0
    };
    timeline_throughput("GPU total throughput", value, "TFLOP/s")
}
