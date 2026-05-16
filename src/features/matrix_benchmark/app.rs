impl BenchScopeApp {
    fn selected_size(&self) -> Result<usize> {
        let size = self
            .size_text
            .trim()
            .parse::<usize>()
            .context("matrix size must be an integer")?;
        if size == 0 {
            return Err(anyhow!("matrix size must be positive"));
        }
        if size > 16384 {
            return Err(anyhow!("matrix size is capped at 16384 for this version"));
        }
        Ok(size)
    }

    fn selected_adapter(&self) -> Result<AdapterInfo> {
        self.adapters
            .get(self.selected_adapter)
            .cloned()
            .ok_or_else(|| anyhow!("no GPU adapter selected"))
    }

    fn start_single(&mut self) {
        self.start_single_checked(false);
    }

    fn start_single_checked(&mut self, ignore_vram_warning: bool) {
        let size = match self.selected_size() {
            Ok(size) => size,
            Err(err) => {
                self.status = err.to_string();
                self.log(err.to_string());
                return;
            }
        };
        let adapter = match self.selected_adapter() {
            Ok(adapter) => adapter,
            Err(err) => {
                self.status = err.to_string();
                self.log(err.to_string());
                return;
            }
        };

        if !ignore_vram_warning {
            if let Some(warning) = self.vram_warning_for(
                RunAction::Single,
                size,
                adapter.clone(),
                self.gpu_intensity,
                self.validate_output,
                self.estimate_cpu_time,
                self.repeat_mode,
                self.repeat_duration,
            ) {
                self.status = "VRAM warning: confirm before running this benchmark".to_owned();
                self.pending_vram_warning = Some(warning);
                return;
            }
        }

        self.launch_single(
            size,
            adapter,
            self.gpu_intensity,
            self.validate_output,
            self.estimate_cpu_time,
        );
    }

    fn launch_single(
        &mut self,
        size: usize,
        adapter: AdapterInfo,
        gpu_intensity: GpuIntensity,
        validate: bool,
        estimate_cpu_time: bool,
    ) {
        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.begin_temperature_run(TemperatureScope::Matrix);
        self.cancel = Some(cancel);
        self.running = true;
        self.progress = 0.0;
        self.cpu_progress = 0.0;
        self.gpu_progress = 0.0;
        self.eta_text = "ETA: estimating".to_owned();
        self.status = format!("Running {size}x{size} benchmark...");
        self.log(format!(
            "Starting benchmark on {} with {} GPU intensity and {} CPU timing",
            adapter.label(),
            gpu_intensity,
            if estimate_cpu_time {
                "estimated"
            } else {
                "exact"
            }
        ));
        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                run_single_cancelable(
                    size,
                    adapter,
                    validate,
                    estimate_cpu_time,
                    gpu_intensity,
                    &worker_cancel,
                    Some(tx.clone()),
                )
            }))
            .map_err(|panic| format!("Benchmark panicked: {}", panic_message(&*panic)))
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(WorkerEvent::SingleDone(result));
        });
    }

    fn start_repeat(&mut self) {
        self.start_repeat_checked(false);
    }

    fn start_repeat_checked(&mut self, ignore_vram_warning: bool) {
        let size = match self.selected_size() {
            Ok(size) => size,
            Err(err) => {
                self.status = err.to_string();
                self.log(err.to_string());
                return;
            }
        };
        let adapter = match self.selected_adapter() {
            Ok(adapter) => adapter,
            Err(err) => {
                self.status = err.to_string();
                self.log(err.to_string());
                return;
            }
        };

        if !ignore_vram_warning {
            if let Some(warning) = self.vram_warning_for(
                RunAction::Repeat,
                size,
                adapter.clone(),
                self.gpu_intensity,
                self.validate_output,
                self.estimate_cpu_time,
                self.repeat_mode,
                self.repeat_duration,
            ) {
                self.status = "VRAM warning: confirm before starting the stress test".to_owned();
                self.pending_vram_warning = Some(warning);
                return;
            }
        }

        self.launch_repeat(
            size,
            adapter,
            self.gpu_intensity,
            self.repeat_mode,
            self.repeat_duration,
        );
    }

    fn launch_repeat(
        &mut self,
        size: usize,
        adapter: AdapterInfo,
        gpu_intensity: GpuIntensity,
        mode: RepeatMode,
        duration: RepeatDuration,
    ) {
        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.begin_temperature_run(TemperatureScope::Matrix);
        self.cancel = Some(cancel);
        self.running = true;
        self.repeat_running = true;
        self.progress = 0.0;
        self.status = format!("Running {mode} stress test for {duration}...");
        self.log(format!(
            "Starting {mode} {duration} stress test at {size}x{size} on {} with {} GPU intensity",
            adapter.label(),
            gpu_intensity
        ));
        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                run_repeat(
                    size,
                    adapter,
                    mode,
                    gpu_intensity,
                    worker_cancel,
                    tx.clone(),
                    duration.duration(),
                )
            }))
            .map_err(|panic| format!("Repeat test panicked: {}", panic_message(&*panic)))
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(WorkerEvent::RepeatDone(result));
        });
    }

    fn vram_warning_for(
        &self,
        action: RunAction,
        size: usize,
        adapter: AdapterInfo,
        gpu_intensity: GpuIntensity,
        validate_output: bool,
        estimate_cpu_time: bool,
        repeat_mode: RepeatMode,
        repeat_duration: RepeatDuration,
    ) -> Option<PendingVramWarning> {
        if action == RunAction::Repeat && repeat_mode == RepeatMode::Cpu {
            return None;
        }
        let estimated_gpu_bytes = gpu_working_set_bytes(size)?;
        let (limit_bytes, limit_label) = adapter_memory_limit_bytes(&adapter)?;
        (estimated_gpu_bytes > limit_bytes).then(|| PendingVramWarning {
            action,
            size,
            adapter,
            gpu_intensity,
            validate_output,
            estimate_cpu_time,
            repeat_mode,
            repeat_duration,
            estimated_gpu_bytes,
            limit_bytes,
            limit_label: limit_label.to_owned(),
        })
    }

    fn continue_pending_vram_warning(&mut self) {
        let Some(warning) = self.pending_vram_warning.take() else {
            return;
        };
        self.log(format!(
            "User chose to run {}x{} despite estimated GPU memory {} exceeding {} ({})",
            warning.size,
            warning.size,
            format_bytes(warning.estimated_gpu_bytes),
            warning.limit_label,
            format_bytes(warning.limit_bytes)
        ));
        match warning.action {
            RunAction::Single => self.launch_single(
                warning.size,
                warning.adapter,
                warning.gpu_intensity,
                warning.validate_output,
                warning.estimate_cpu_time,
            ),
            RunAction::Repeat => self.launch_repeat(
                warning.size,
                warning.adapter,
                warning.gpu_intensity,
                warning.repeat_mode,
                warning.repeat_duration,
            ),
        }
    }

    fn cancel_single(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
            self.status = "Cancel requested; stopping benchmark...".to_owned();
            self.log("Cancel requested for single benchmark");
        }
    }

    fn cancel_repeat(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
            self.status = "Cancel requested; stopping stress test...".to_owned();
            self.log("Cancel requested");
        }
    }

    fn poll_worker_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                WorkerEvent::SingleProgress(progress) => {
                    self.cpu_progress = progress.cpu_progress;
                    self.gpu_progress = progress.gpu_progress;
                    self.progress =
                        ((progress.cpu_progress + progress.gpu_progress) / 2.0).clamp(0.0, 1.0);
                    self.eta_text = format_eta(progress.eta_s);
                    self.status = format!(
                        "{} - elapsed {}",
                        progress.phase,
                        format_elapsed(progress.elapsed_s)
                    );
                }
                WorkerEvent::SingleDone(result) => {
                    self.running = false;
                    self.repeat_running = false;
                    self.cancel = None;
                    match result {
                        Ok(mut result) => {
                            self.progress = 1.0;
                            self.cpu_progress = 1.0;
                            self.gpu_progress = 1.0;
                            self.eta_text = "ETA: complete".to_owned();
                            self.status = "Benchmark complete".to_owned();
                            if let Some(report) = self.finish_and_log_temperature_run() {
                                result.cpu_temperature = report.cpu;
                                result.gpu_temperature = report.gpu;
                            }
                            self.log(format!(
                                "Benchmark complete: CPU {} ms ({}, {}), GPU total {} ms, GPU compute {} ms, path {}, dispatches {}, max dispatch {} ms",
                                format_cpu_ms(&result),
                                if result.cpu_estimated {
                                    "estimated"
                                } else {
                                    "exact"
                                },
                                result.cpu_model,
                                format_ms(Some(result.gpu_total_ms)),
                                format_ms(result.gpu_compute_ms),
                                result.gpu_path,
                                result.dispatch_count,
                                format_ms(result.max_dispatch_ms)
                            ));
                            self.results.push(result);
                        }
                        Err(err) => {
                            let _ = self.finish_and_log_temperature_run();
                            if err.to_ascii_lowercase().contains("canceled") {
                                self.progress = 0.0;
                                self.eta_text = "ETA: canceled".to_owned();
                            } else {
                                self.progress = 1.0;
                                self.eta_text.clear();
                            }
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
                WorkerEvent::RepeatProgress(progress) => {
                    self.progress =
                        (progress.elapsed_s / progress.duration_s).clamp(0.0, 1.0) as f32;
                    self.eta_text =
                        format_eta(Some((progress.duration_s - progress.elapsed_s).max(0.0)));
                    self.status = format!(
                        "{} stress: {:.1}s, {} iteration(s), latest {} ms, avg {} ms, compute avg {} ms",
                        progress.mode,
                        progress.elapsed_s,
                        progress.iterations,
                        format_ms(Some(progress.latest_ms)),
                        format_ms(Some(progress.average_total_ms)),
                        format_ms(progress.average_compute_ms)
                    );
                }
                WorkerEvent::RepeatDone(result) => {
                    self.running = false;
                    self.repeat_running = false;
                    self.cancel = None;
                    self.eta_text.clear();
                    match result {
                        Ok(progress) => {
                            let _ = self.finish_and_log_temperature_run();
                            if !progress.canceled {
                                self.progress = 1.0;
                            }
                            let state = if progress.canceled {
                                "canceled"
                            } else {
                                "complete"
                            };
                            self.status = format!(
                                "Stress test {state}: {} iteration(s), avg {} ms",
                                progress.iterations,
                                format_ms(Some(progress.average_total_ms))
                            );
                            self.log(format!(
                                "Stress test {state}: mode {}, size {}, iterations {}, avg {} ms, compute avg {} ms",
                                progress.mode,
                                progress.size,
                                progress.iterations,
                                format_ms(Some(progress.average_total_ms)),
                                format_ms(progress.average_compute_ms)
                            ));
                        }
                        Err(err) => {
                            let _ = self.finish_and_log_temperature_run();
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
            }
        }
    }
    fn request_matrix_back_to_menu(&mut self) {
        if self.running {
            self.matrix_back_confirm = true;
        } else {
            self.view = AppView::MainMenu;
        }
    }
    fn request_stress_back_to_menu(&mut self) {
        if self.repeat_running {
            self.stress_back_confirm = true;
        } else {
            self.view = AppView::MainMenu;
        }
    }
}
