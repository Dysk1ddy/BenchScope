impl BenchScopeApp {
    fn selected_size(&self) -> Result<usize> {
        parse_matrix_size(&self.size_text)
    }

    fn selected_stress_size(&self) -> Result<usize> {
        parse_matrix_size(&self.stress_size_text)
    }

}

fn parse_matrix_size(size_text: &str) -> Result<usize> {
    let size = size_text
        .trim()
        .parse::<usize>()
        .context("matrix size must be an integer")?;
    if size == 0 {
        return Err(anyhow!("matrix size must be positive"));
    }
    let max_size = DEFAULT_SIZES.last().copied().unwrap_or(32_768);
    if size > max_size {
        return Err(anyhow!(
            "matrix size is capped at {max_size} for this version"
        ));
    }
    Ok(size)
}

impl BenchScopeApp {
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
                self.stress_gpu_backend,
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
        let _ = crashlog_write_operation(
            "Matrix benchmark started",
            &format!(
                "Size: {size}x{size}\nAdapter: {}\nVendor: {:04X}\nDevice: {:04X}\nBackend: {:?}\nDeviceType: {:?}\nIntensity: {}\nValidateOutput: {}\nEstimateCpuTime: {}",
                adapter.name,
                adapter.vendor,
                adapter.device,
                adapter.backend,
                adapter.device_type,
                gpu_intensity,
                validate,
                estimate_cpu_time
            ),
        );
        let pytorch_python = self.pytorch_python.trim().to_owned();
        if let Err(err) = thread::Builder::new()
            .name("benchscope-matrix-single".to_owned())
            .spawn(move || {
                let result = panic::catch_unwind(AssertUnwindSafe(|| {
                    run_single_cancelable(
                        size,
                        adapter,
                        validate,
                        estimate_cpu_time,
                        gpu_intensity,
                        pytorch_python,
                        &worker_cancel,
                        Some(tx.clone()),
                    )
                }))
                .map_err(|panic| {
                    format!("Benchmark panicked: {}. {}", panic_message(&*panic), crashlog_hint())
                })
                .and_then(|result| result.map_err(|err| format!("{err:#}")));
                let _ = tx.send(WorkerEvent::SingleDone(result));
            })
        {
            self.running = false;
            self.cancel = None;
            self.status = format!("Could not start benchmark worker: {err}");
            self.log(self.status.clone());
        }
    }

    fn start_repeat(&mut self) {
        self.start_repeat_checked(false);
    }

    fn start_repeat_checked(&mut self, ignore_vram_warning: bool) {
        let size = match self.selected_stress_size() {
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
                self.stress_gpu_backend,
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
            self.stress_gpu_backend,
            self.repeat_mode,
            self.repeat_duration,
        );
    }

    fn launch_repeat(
        &mut self,
        size: usize,
        adapter: AdapterInfo,
        gpu_intensity: GpuIntensity,
        stress_gpu_backend: StressGpuBackend,
        mode: RepeatMode,
        duration: RepeatDuration,
    ) {
        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.begin_temperature_run(TemperatureScope::Matrix);
        self.start_timeline(
            TimelineScope::MatrixStress,
            format!("{mode} stress {size}x{size} on {}", adapter.label()),
        );
        self.cancel = Some(cancel);
        self.running = true;
        self.repeat_running = true;
        self.progress = 0.0;
        self.repeat_progress = Some(RepeatProgress {
            mode,
            size,
            duration_s: duration.seconds(),
            elapsed_s: 0.0,
            iterations: 0,
            latest_ms: 0.0,
            average_total_ms: 0.0,
            average_compute_ms: None,
            theoretical_fp16_tc_fp32_accum_tflops:
                theoretical_fp16_tc_fp32_accum_tflops_for_adapter(&adapter.name),
            canceled: false,
        });
        self.eta_text = duration
            .seconds()
            .map(|seconds| format_eta(Some(seconds)))
            .unwrap_or_else(|| "Runs until canceled".to_owned());
        self.status = format!("Running {mode} stress test {}...", duration.run_label());
        self.log(format!(
            "Starting {mode} stress test {} at {size}x{size} on {} with {} GPU intensity and {} backend",
            duration.run_label(),
            adapter.label(),
            gpu_intensity,
            stress_gpu_backend
        ));
        let _ = crashlog_write_operation(
            "Matrix stress started",
            &format!(
                "Mode: {mode}\nDuration: {}\nSize: {size}x{size}\nAdapter: {}\nVendor: {:04X}\nDevice: {:04X}\nBackend: {:?}\nDeviceType: {:?}\nIntensity: {}\nStressBackend: {}",
                duration.run_label(),
                adapter.name,
                adapter.vendor,
                adapter.device,
                adapter.backend,
                adapter.device_type,
                gpu_intensity,
                stress_gpu_backend
            ),
        );
        let pytorch_python = self.pytorch_python.trim().to_owned();
        if let Err(err) = thread::Builder::new()
            .name("benchscope-matrix-repeat".to_owned())
            .spawn(move || {
                let result = panic::catch_unwind(AssertUnwindSafe(|| {
                    run_repeat(
                        size,
                        adapter,
                        mode,
                        gpu_intensity,
                        stress_gpu_backend,
                        pytorch_python,
                        worker_cancel,
                        tx.clone(),
                        duration,
                    )
                }))
                .map_err(|panic| {
                    format!(
                        "Repeat test panicked: {}. {}",
                        panic_message(&*panic),
                        crashlog_hint()
                    )
                })
                .and_then(|result| result.map_err(|err| format!("{err:#}")));
                let _ = tx.send(WorkerEvent::RepeatDone(result));
            })
        {
            self.running = false;
            self.repeat_running = false;
            self.cancel = None;
            self.status = format!("Could not start stress worker: {err}");
            self.log(self.status.clone());
        }
    }

    fn vram_warning_for(
        &self,
        action: RunAction,
        size: usize,
        adapter: AdapterInfo,
        gpu_intensity: GpuIntensity,
        stress_gpu_backend: StressGpuBackend,
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
            stress_gpu_backend,
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
                warning.stress_gpu_backend,
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
                            if !err.to_ascii_lowercase().contains("canceled") {
                                let _ = crashlog_write_error_report("Matrix benchmark error", &err);
                            }
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
                    self.repeat_progress = Some(progress.clone());
                    if let Some(duration_s) = progress.duration_s {
                        self.progress = (progress.elapsed_s / duration_s).clamp(0.0, 1.0) as f32;
                        self.eta_text =
                            format_eta(Some((duration_s - progress.elapsed_s).max(0.0)));
                    } else {
                        self.progress = 0.0;
                        self.eta_text = "Runs until canceled".to_owned();
                    }
                    let rate = format_stress_rate_per_min(progress.iterations, progress.elapsed_s);
                    self.status = format!(
                        "{} stress: {:.1}s, {} iteration(s), {}, latest {} ms, avg {} ms, compute avg {} ms",
                        progress.mode,
                        progress.elapsed_s,
                        progress.iterations,
                        rate,
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
                            self.repeat_progress = Some(progress.clone());
                            let _ = self.finish_and_log_temperature_run();
                            self.finish_timeline_run(
                                TimelineScope::MatrixStress,
                                self.current_timeline_throughput(TimelineScope::MatrixStress),
                                self.status.clone(),
                            );
                            if !progress.canceled && progress.duration_s.is_some() {
                                self.progress = 1.0;
                            }
                            let state = if progress.canceled {
                                "canceled"
                            } else {
                                "complete"
                            };
                            self.status = format!(
                                "Stress test {state}: {} iteration(s), {}, avg {} ms",
                                progress.iterations,
                                format_stress_rate_per_min(progress.iterations, progress.elapsed_s),
                                format_ms(Some(progress.average_total_ms))
                            );
                            self.log(format!(
                                "Stress test {state}: mode {}, size {}, iterations {}, rate {}, avg {} ms, compute avg {} ms",
                                progress.mode,
                                progress.size,
                                progress.iterations,
                                format_stress_rate_per_min(progress.iterations, progress.elapsed_s),
                                format_ms(Some(progress.average_total_ms)),
                                format_ms(progress.average_compute_ms)
                            ));
                        }
                        Err(err) => {
                            let _ = self.finish_and_log_temperature_run();
                            self.finish_timeline_run(TimelineScope::MatrixStress, None, err.clone());
                            if !err.to_ascii_lowercase().contains("canceled") {
                                let _ = crashlog_write_error_report("Matrix stress error", &err);
                            }
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
                WorkerEvent::PyTorchProbeDone(result) => {
                    self.pytorch_probe_running = false;
                    self.setup_task_running = false;
                    self.setup_task_progress = None;
                    self.eta_text.clear();
                    match result {
                        Ok(environment) => {
                            self.status = if environment.cuda_available {
                                format!(
                                    "PyTorch CUDA ready: {} CUDA device(s)",
                                    environment.device_count
                                )
                            } else if let Some(error) = &environment.error {
                                format!("PyTorch CUDA unavailable: {error}")
                            } else {
                                "PyTorch imported, but CUDA is unavailable".to_owned()
                            };
                            if environment.cuda_available {
                                self.pytorch_python = environment.python_executable.clone();
                                self.ai_training.pytorch_python =
                                    environment.python_executable.clone();
                                self.ai_training.pytorch_probe = Some(environment.clone());
                            }
                            self.log(self.status.clone());
                            for line in environment.summary_lines() {
                                self.log(line);
                            }
                            self.pytorch_probe = Some(environment);
                        }
                        Err(err) => {
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                    self.refresh_setup_detection();
                }
                WorkerEvent::PyTorchInstallDone(result) => {
                    self.pytorch_install_running = false;
                    self.pytorch_probe_running = false;
                    self.setup_task_running = false;
                    self.setup_task_progress = None;
                    self.pending_pytorch_install = false;
                    self.eta_text.clear();
                    match result {
                        Ok(environment) => {
                            self.pytorch_python = environment.python_executable.clone();
                            self.ai_training.pytorch_python = environment.python_executable.clone();
                            self.ai_training.pytorch_probe = Some(environment.clone());
                            self.status = format!(
                                "PyTorch CUDA installed and ready: {} CUDA device(s)",
                                environment.device_count
                            );
                            self.log(self.status.clone());
                            for line in environment.summary_lines() {
                                self.log(line);
                            }
                            self.pytorch_probe = Some(environment);
                        }
                        Err(err) => {
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                    self.refresh_setup_detection();
                }
                WorkerEvent::SetupTaskProgress(progress) => {
                    self.eta_text = progress.detail.clone();
                    self.setup_task_progress = Some(progress);
                }
                WorkerEvent::SetupTaskDone(result) => {
                    self.setup_task_running = false;
                    self.setup_task_progress = None;
                    self.eta_text.clear();
                    match result {
                        Ok(outcome) => {
                            self.status = outcome.message.clone();
                            self.log(outcome.message);
                            if let Some(environment) = outcome.pytorch_environment {
                                self.pytorch_python = environment.python_executable.clone();
                                self.ai_training.pytorch_python =
                                    environment.python_executable.clone();
                                self.ai_training.pytorch_probe = Some(environment.clone());
                                for line in environment.summary_lines() {
                                    self.log(line);
                                }
                                self.pytorch_probe = Some(environment);
                            }
                        }
                        Err(err) => {
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                    self.refresh_setup_detection();
                }
                WorkerEvent::Log(message) => self.log(message),
            }
        }
    }

    fn start_pytorch_cuda_probe(&mut self) {
        if self.pytorch_probe_running || self.pytorch_install_running || self.running {
            return;
        }

        let preferred_python = self.pytorch_python.trim().to_owned();
        let tx = self.tx.clone();
        self.pytorch_probe_running = true;
        self.status = "Probing PyTorch CUDA environment...".to_owned();
        self.eta_text = "ETA: estimating".to_owned();
        if preferred_python.is_empty() {
            self.log("Probing PyTorch CUDA with auto-discovered Python candidates");
        } else {
            self.log(format!(
                "Probing PyTorch CUDA with {preferred_python} and auto-discovered fallback candidates"
            ));
        }

        thread::spawn(move || {
            let result =
                panic::catch_unwind(AssertUnwindSafe(|| probe_first_pytorch_cuda(&preferred_python)))
                    .map_err(|panic| {
                        format!("PyTorch CUDA probe panicked: {}", panic_message(&*panic))
                    })
                    .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(WorkerEvent::PyTorchProbeDone(result));
        });
    }

    fn request_pytorch_cuda_install(&mut self) {
        if self.running || self.pytorch_probe_running || self.pytorch_install_running {
            return;
        }
        self.pending_pytorch_install = true;
    }

    fn start_pytorch_cuda_install(&mut self) {
        if self.running || self.pytorch_probe_running || self.pytorch_install_running {
            return;
        }
        let python = self.pytorch_python.trim().to_owned();
        if python.is_empty() {
            self.status = "Python executable is required before installing PyTorch CUDA".to_owned();
            self.log(self.status.clone());
            return;
        }

        let tx = self.tx.clone();
        self.pending_pytorch_install = false;
        self.pytorch_install_running = true;
        self.pytorch_probe_running = true;
        self.status = "Installing PyTorch CUDA...".to_owned();
        self.eta_text = "Large download in progress".to_owned();
        self.log(format!(
            "User approved PyTorch CUDA install via {}",
            pytorch_cuda_install_command_preview(&python)
        ));

        thread::spawn(move || {
            let log_tx = tx.clone();
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                install_pytorch_cuda(&python, |line| {
                    let _ = log_tx.send(WorkerEvent::Log(format!("PyTorch install: {line}")));
                })
            }))
            .map_err(|panic| {
                format!(
                    "PyTorch CUDA install panicked: {}",
                    panic_message(&*panic)
                )
            })
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(WorkerEvent::PyTorchInstallDone(result));
        });
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
