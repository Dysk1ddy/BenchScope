#[derive(Clone, Debug)]
struct PyTorchCudaDevice {
    index: usize,
    name: String,
    capability_major: u32,
    capability_minor: u32,
    total_memory_bytes: u64,
}

#[derive(Clone, Debug)]
struct PyTorchCudaEnvironment {
    ok: bool,
    python_executable: String,
    python: String,
    python_version: Option<String>,
    torch_version: Option<String>,
    torch_cuda_version: Option<String>,
    cudnn_version: Option<String>,
    cuda_available: bool,
    device_count: usize,
    devices: Vec<PyTorchCudaDevice>,
    distributed_available: bool,
    nccl_available: bool,
    notes: Vec<String>,
    error: Option<String>,
}

#[derive(Clone, Debug)]
struct PyTorchCudaBenchmarkOutput {
    environment: PyTorchCudaEnvironment,
    device_index: Option<usize>,
    gpu_name: Option<String>,
    measured_steps: usize,
    gpu_step_ms: Vec<f64>,
    wall_step_ms: Vec<f64>,
    forward_loss_ms: Vec<f64>,
    backward_ms: Vec<f64>,
    optimizer_ms: Vec<f64>,
    peak_allocated_bytes: u64,
    peak_reserved_bytes: u64,
    validation: Option<String>,
    time_limited: bool,
}

const PYTORCH_CUDA_PIP_INDEX_URL: &str = "https://download.pytorch.org/whl/cu128";
const PYTORCH_CUDA_INSTALL_PACKAGES: [&str; 3] = ["torch", "torchvision", "torchaudio"];
const PYTORCH_CUDA_INSTALL_DOWNLOAD_NOTE: &str = "about 3 GB";

impl PyTorchCudaEnvironment {
    fn summary_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        lines.push(format!("Probe OK: {}", yes_no(self.ok)));
        lines.push(format!("Python executable: {}", self.python_executable));
        if let Some(version) = &self.python_version {
            lines.push(format!("Python version: {version}"));
        }
        if let Some(version) = &self.torch_version {
            lines.push(format!("PyTorch version: {version}"));
        }
        if let Some(version) = &self.torch_cuda_version {
            lines.push(format!("CUDA runtime: {version}"));
        }
        if let Some(version) = &self.cudnn_version {
            lines.push(format!("cuDNN version: {version}"));
        }
        lines.push(format!(
            "CUDA available: {}",
            if self.cuda_available { "yes" } else { "no" }
        ));
        lines.push(format!(
            "Distributed: {}; NCCL: {}",
            if self.distributed_available { "yes" } else { "no" },
            if self.nccl_available { "yes" } else { "no" }
        ));
        lines.push(format!("CUDA devices: {}", self.device_count));
        for device in &self.devices {
            lines.push(format!(
                "CUDA device {}: {} sm_{}{} {}",
                device.index,
                device.name,
                device.capability_major,
                device.capability_minor,
                format_bytes(device.total_memory_bytes)
            ));
        }
        if let Some(error) = &self.error {
            lines.push(format!("Probe note: {error}"));
        }
        for note in &self.notes {
            lines.push(format!("Probe note: {note}"));
        }
        lines
    }
}

fn default_pytorch_python_executable() -> String {
    pytorch_python_candidates()
        .into_iter()
        .next()
        .unwrap_or_else(|| "python".to_owned())
}

fn probe_pytorch_cuda(python: &str) -> Result<PyTorchCudaEnvironment> {
    let output = run_pytorch_cuda_probe_script(python, Duration::from_secs(15))?;
    Ok(parse_pytorch_cuda_probe_output(&output, python))
}

fn probe_first_pytorch_cuda(preferred_python: &str) -> Result<PyTorchCudaEnvironment> {
    let candidates = pytorch_python_candidates_with_preferred(preferred_python);
    let mut best_unavailable = None;
    let mut errors = Vec::new();

    for python in candidates {
        match probe_pytorch_cuda(&python) {
            Ok(environment) if environment.cuda_available => return Ok(environment),
            Ok(environment) => {
                if best_unavailable.is_none() {
                    best_unavailable = Some(environment);
                }
            }
            Err(err) => errors.push(format!("{python}: {err:#}")),
        }
    }

    if let Some(environment) = best_unavailable {
        Ok(environment)
    } else if errors.is_empty() {
        Err(anyhow!("no Python executable candidates were found"))
    } else {
        Err(anyhow!(
            "no usable Python executable was found for PyTorch CUDA: {}",
            errors.join("; ")
        ))
    }
}

fn install_pytorch_cuda<F>(python: &str, mut log: F) -> Result<PyTorchCudaEnvironment>
where
    F: FnMut(String),
{
    let python = python.trim();
    if python.is_empty() {
        return Err(anyhow!("Python executable is required to install PyTorch CUDA"));
    }

    log(format!(
        "Installing PyTorch CUDA 12.8 via {python}; download may be {PYTORCH_CUDA_INSTALL_DOWNLOAD_NOTE}"
    ));
    log("Ensuring pip is available".to_owned());
    if let Err(err) = run_pytorch_install_command(
        python,
        &["-m", "ensurepip", "--upgrade"],
        Duration::from_secs(10 * 60),
        &mut log,
    ) {
        log(format!("ensurepip did not complete: {err:#}"));
    }

    log("Upgrading pip".to_owned());
    run_pytorch_install_command(
        python,
        &["-m", "pip", "install", "--upgrade", "pip"],
        Duration::from_secs(20 * 60),
        &mut log,
    )?;

    let mut args = vec!["-m", "pip", "install", "--upgrade"];
    args.extend(PYTORCH_CUDA_INSTALL_PACKAGES);
    args.extend(["--index-url", PYTORCH_CUDA_PIP_INDEX_URL]);
    log(format!(
        "Installing {} from {PYTORCH_CUDA_PIP_INDEX_URL}",
        PYTORCH_CUDA_INSTALL_PACKAGES.join(", ")
    ));
    run_pytorch_install_command(
        python,
        &args,
        Duration::from_secs(2 * 60 * 60),
        &mut log,
    )?;

    log("Probing installed PyTorch CUDA environment".to_owned());
    let environment = probe_pytorch_cuda(python)?;
    if environment.cuda_available {
        Ok(environment)
    } else {
        Err(anyhow!(
            "PyTorch installed, but CUDA is still unavailable for {python}"
        ))
    }
}

fn pytorch_cuda_install_command_preview(python: &str) -> String {
    format!(
        "{} -m pip install --upgrade {} --index-url {}",
        python.trim(),
        PYTORCH_CUDA_INSTALL_PACKAGES.join(" "),
        PYTORCH_CUDA_PIP_INDEX_URL
    )
}

fn pytorch_python_candidates_with_preferred(preferred_python: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    push_python_candidate(&mut candidates, preferred_python);
    for candidate in pytorch_python_candidates() {
        push_python_candidate(&mut candidates, &candidate);
    }
    candidates
}

fn pytorch_python_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    if let Ok(python) = std::env::var("PYTHON") {
        push_python_candidate(&mut candidates, &python);
    }

    for candidate in discover_py_launcher_python_paths() {
        push_python_candidate(&mut candidates, &candidate);
    }

    #[cfg(windows)]
    {
        for version in ["314", "313", "312", "311", "310", "39"] {
            if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
                push_python_file_candidate(
                    &mut candidates,
                    format!(r"{local_app_data}\Programs\Python\Python{version}\python.exe"),
                );
            }
            push_python_file_candidate(
                &mut candidates,
                format!(r"C:\Python{version}\python.exe"),
            );
        }
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            push_python_file_candidate(
                &mut candidates,
                format!(r"{local_app_data}\Python\bin\python.exe"),
            );
        }
    }

    push_python_candidate(&mut candidates, "python");
    push_python_candidate(&mut candidates, "python3");
    candidates
}

fn discover_py_launcher_python_paths() -> Vec<String> {
    let mut command = Command::new("py");
    command.arg("-0p");
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW_RAW);
    let Ok(output) = command.output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_py_launcher_python_paths(&String::from_utf8_lossy(&output.stdout))
}

fn parse_py_launcher_python_paths(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            let drive_marker = line.find(r":\")?;
            let start = line[..drive_marker]
                .rfind(char::is_whitespace)
                .map(|index| index + 1)
                .unwrap_or(0);
            let path = line[start..].trim();
            (!path.is_empty()).then(|| path.to_owned())
        })
        .collect()
}

fn push_python_file_candidate(candidates: &mut Vec<String>, candidate: String) {
    if std::path::Path::new(&candidate).is_file() {
        push_python_candidate(candidates, &candidate);
    }
}

fn push_python_candidate(candidates: &mut Vec<String>, candidate: &str) {
    let candidate = candidate.trim().trim_matches('"');
    if candidate.is_empty() {
        return;
    }
    let normalized = normalize_python_candidate(candidate);
    if candidates
        .iter()
        .any(|existing| normalize_python_candidate(existing) == normalized)
    {
        return;
    }
    candidates.push(candidate.to_owned());
}

fn normalize_python_candidate(candidate: &str) -> String {
    #[cfg(windows)]
    {
        candidate.trim().trim_matches('"').to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        candidate.trim().trim_matches('"').to_owned()
    }
}

fn run_pytorch_install_command<F>(
    python: &str,
    args: &[&str],
    timeout: Duration,
    log: &mut F,
) -> Result<()>
where
    F: FnMut(String),
{
    let label = format_command_line(python, args);
    log(format!("Running {label}"));

    let mut command = Command::new(python);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW_RAW);

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to start {label}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("failed to capture stdout for {label}"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("failed to capture stderr for {label}"))?;

    let (line_tx, line_rx) = mpsc::channel::<String>();
    let stdout_tx = line_tx.clone();
    let stdout_thread = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = stdout_tx.send(line);
        }
    });
    let stderr_thread = thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = line_tx.send(line);
        }
    });

    let started = Instant::now();
    let mut last_heartbeat = Instant::now();
    let status = loop {
        while let Ok(line) = line_rx.try_recv() {
            if !line.trim().is_empty() {
                log(line);
            }
        }
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("failed to query {label}"))?
        {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(
                "{label} timed out after {} seconds",
                timeout.as_secs()
            ));
        }
        if last_heartbeat.elapsed() >= Duration::from_secs(30) {
            log(format!(
                "Still running {label} after {}",
                format_elapsed(started.elapsed().as_secs_f64())
            ));
            last_heartbeat = Instant::now();
        }
        thread::sleep(Duration::from_millis(100));
    };

    let _ = stdout_thread.join();
    let _ = stderr_thread.join();
    while let Ok(line) = line_rx.try_recv() {
        if !line.trim().is_empty() {
            log(line);
        }
    }

    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("{label} failed with status {status}"))
    }
}

fn format_command_line(program: &str, args: &[&str]) -> String {
    std::iter::once(program)
        .chain(args.iter().copied())
        .map(command_line_part)
        .collect::<Vec<_>>()
        .join(" ")
}

fn command_line_part(part: &str) -> String {
    if part.contains(char::is_whitespace) {
        format!("\"{}\"", part.replace('"', "\\\""))
    } else {
        part.to_owned()
    }
}

fn validate_pytorch_cuda_training_config(config: &AiTrainingConfig) -> Result<()> {
    match config.workload {
        AiTrainingWorkload::LinearLayer => validate_ai_linear_dimensions(
            config.dimensions.batch_size,
            config.dimensions.input_dim,
            config.dimensions.output_dim,
        ),
        AiTrainingWorkload::Mlp => validate_ai_linear_dimensions(
            config.dimensions.batch_size,
            config.dimensions.hidden_size,
            config.dimensions.output_dim,
        ),
        AiTrainingWorkload::TransformerBlock => {
            if config.dimensions.batch_size == 0
                || config.dimensions.sequence_len == 0
                || config.dimensions.hidden_size == 0
                || config.dimensions.attention_heads == 0
            {
                return Err(anyhow!("AI transformer dimensions must be non-zero"));
            }
            usize_to_u32(config.dimensions.batch_size, "batch size")?;
            usize_to_u32(config.dimensions.sequence_len, "sequence length")?;
            usize_to_u32(config.dimensions.hidden_size, "hidden size")?;
            usize_to_u32(config.dimensions.attention_heads, "attention heads")?;
            if !config
                .dimensions
                .hidden_size
                .is_multiple_of(config.dimensions.attention_heads)
            {
                return Err(anyhow!(
                    "hidden size must be divisible by attention heads for PyTorch transformer training"
                ));
            }
            Ok(())
        }
        AiTrainingWorkload::OptimizerStress => Err(anyhow!(
            "PyTorch CUDA currently supports linear, MLP, and transformer training workloads"
        )),
    }
}

fn run_pytorch_cuda_training_benchmark(
    config: AiTrainingConfig,
    cancel: Arc<AtomicBool>,
    tx: Sender<AiTrainingWorkerEvent>,
) -> Result<AiTrainingResult> {
    validate_pytorch_cuda_training_config(&config)?;
    check_canceled_with(Some(cancel.as_ref()), "PyTorch CUDA benchmark canceled")?;

    let total_steps = config.warmup_steps.saturating_add(config.measured_steps);
    let started = Instant::now();
    emit_ai_training_progress(
        &tx,
        "Launching PyTorch CUDA worker",
        0,
        total_steps,
        started,
        Some(config.time_limit_s),
        true,
    );
    let _ = tx.send(AiTrainingWorkerEvent::Log(format!(
        "Launching PyTorch CUDA {} {} training via {} on CUDA device {}",
        config.workload, config.precision, config.pytorch_python, config.pytorch_cuda_device
    )));

    let timeout = pytorch_cuda_benchmark_timeout(config.time_limit_s);
    let output = run_pytorch_cuda_benchmark_script(&config, cancel.as_ref(), timeout)?;
    let parsed = parse_pytorch_cuda_benchmark_output(&output, &config.pytorch_python);
    if let Some(error) = &parsed.environment.error {
        return Err(anyhow!("PyTorch CUDA benchmark failed: {error}"));
    }

    let measured_steps = parsed
        .measured_steps
        .min(parsed.gpu_step_ms.len())
        .min(parsed.wall_step_ms.len());
    let measured_steps = if measured_steps == 0 {
        parsed.gpu_step_ms.len().min(parsed.wall_step_ms.len())
    } else {
        measured_steps
    };
    if measured_steps == 0 {
        return Err(anyhow!(
            "PyTorch CUDA benchmark did not return measured step timings"
        ));
    }

    let gpu_step_ms = &parsed.gpu_step_ms[..measured_steps];
    let wall_step_ms = &parsed.wall_step_ms[..measured_steps];
    let gpu_elapsed_ms = gpu_step_ms.iter().sum::<f64>();
    let wall_elapsed_ms = wall_step_ms.iter().sum::<f64>();
    let gpu_elapsed_s = gpu_elapsed_ms / 1000.0;
    let wall_elapsed_s = (wall_elapsed_ms / 1000.0).max(f64::MIN_POSITIVE);
    let flops_per_step = config_flops_per_step(&config);
    let total_flops = flops_per_step * measured_steps as f64;
    let compute_tflops =
        (gpu_elapsed_s > 0.0).then_some(total_flops / gpu_elapsed_s / 1.0e12);
    let end_to_end_tflops = total_flops / wall_elapsed_s / 1.0e12;
    let throughput_value = ai_training_throughput(&config, measured_steps, wall_elapsed_s);
    let avg_step_ms = wall_elapsed_ms / measured_steps as f64;
    let p95_step_ms = percentile_sorted_copy(wall_step_ms, 0.95);
    let step_timings = py_torch_step_timing_summary(&parsed, measured_steps);
    let memory_bytes = parsed
        .peak_reserved_bytes
        .max(parsed.peak_allocated_bytes)
        .max(config_memory_bytes(&config));

    let gpu_name = parsed.gpu_name.unwrap_or_else(|| {
        parsed
            .environment
            .devices
            .first()
            .map(|device| device.name.clone())
            .unwrap_or_else(|| format!("CUDA device {}", config.pytorch_cuda_device))
    });
    let validation = if config.validation_enabled {
        parsed
            .validation
            .unwrap_or_else(|| "Passed: PyTorch completed measured training steps".to_owned())
    } else {
        "Skipped: validation disabled".to_owned()
    };
    let mut notes = parsed.environment.notes.clone();
    if let Some(version) = &parsed.environment.torch_version {
        notes.push(format!("PyTorch {version}."));
    }
    if let Some(version) = &parsed.environment.torch_cuda_version {
        notes.push(format!("CUDA runtime {version}."));
    }
    if let Some(device_index) = parsed.device_index {
        notes.push(format!("CUDA device {device_index} was benchmarked."));
    } else {
        notes.push(format!(
            "Requested CUDA device {}.",
            config.pytorch_cuda_device
        ));
    }
    notes.push(format!(
        "Peak CUDA allocated {}; reserved {}.",
        format_bytes(parsed.peak_allocated_bytes),
        format_bytes(parsed.peak_reserved_bytes)
    ));
    notes.push(
        "Real PyTorch training path: tensors stay resident while forward, loss, backward, and AdamW update run on CUDA."
            .to_owned(),
    );
    notes.push(
        "Compute timing uses torch.cuda.Event; end-to-end timing uses Python wall-clock step latency."
            .to_owned(),
    );
    if let Some(step_timings) = &step_timings {
        notes.push(format!(
            "Average step split: forward/loss {}, backward {}, optimizer {}.",
            format_ms(Some(step_timings.forward_loss_ms)),
            format_ms(Some(step_timings.backward_ms)),
            format_ms(Some(step_timings.optimizer_ms))
        ));
    }
    notes.push(
        "Single-process PyTorch CUDA path only; no distributed or cross-GPU communication is measured."
            .to_owned(),
    );
    if parsed.time_limited {
        notes.push(format!(
            "Stopped after {} measured step(s) at the {} time limit.",
            measured_steps,
            format_elapsed(config.time_limit_s)
        ));
    }
    if config.smoke_test {
        notes.push("Smoke test run.".to_owned());
    }

    emit_ai_training_progress(
        &tx,
        "PyTorch CUDA benchmark complete",
        total_steps,
        total_steps,
        started,
        Some(config.time_limit_s),
        true,
    );
    let _ = tx.send(AiTrainingWorkerEvent::Log(format!(
        "Completed {} PyTorch CUDA measured step(s): {:.2} end-to-end TFLOP/s, {:.1} {}, avg step {} ms",
        measured_steps,
        end_to_end_tflops,
        throughput_value,
        config.workload.throughput_label(),
        format_ms(Some(avg_step_ms))
    )));

    Ok(AiTrainingResult {
        backend: config.backend,
        workload: config.workload,
        preset: config.preset,
        precision: config.precision,
        gpu_names: vec![gpu_name],
        shape: ai_training_shape_label(config.workload, &config.dimensions),
        flops_per_step,
        measured_steps,
        compute_tflops,
        end_to_end_tflops: Some(end_to_end_tflops),
        throughput_value: Some(throughput_value),
        throughput_label: config.workload.throughput_label(),
        avg_step_ms: Some(avg_step_ms),
        p95_step_ms: Some(p95_step_ms),
        step_timings,
        memory_bytes,
        validation,
        notes: notes.join(" "),
    })
}

fn py_torch_step_timing_summary(
    parsed: &PyTorchCudaBenchmarkOutput,
    measured_steps: usize,
) -> Option<AiTrainingStepTimings> {
    let count = measured_steps
        .min(parsed.forward_loss_ms.len())
        .min(parsed.backward_ms.len())
        .min(parsed.optimizer_ms.len());
    if count == 0 {
        return None;
    }

    Some(AiTrainingStepTimings {
        forward_loss_ms: parsed.forward_loss_ms.iter().take(count).sum::<f64>() / count as f64,
        backward_ms: parsed.backward_ms.iter().take(count).sum::<f64>() / count as f64,
        optimizer_ms: parsed.optimizer_ms.iter().take(count).sum::<f64>() / count as f64,
    })
}

fn parse_pytorch_cuda_probe_output(output: &str, python: &str) -> PyTorchCudaEnvironment {
    let mut environment = PyTorchCudaEnvironment {
        ok: true,
        python_executable: python.to_owned(),
        python: python.to_owned(),
        python_version: None,
        torch_version: None,
        torch_cuda_version: None,
        cudnn_version: None,
        cuda_available: false,
        device_count: 0,
        devices: Vec::new(),
        distributed_available: false,
        nccl_available: false,
        notes: Vec::new(),
        error: None,
    };

    for line in output.lines() {
        let mut parts = line.splitn(2, '\t');
        let key = parts.next().unwrap_or_default();
        let value = parts.next().unwrap_or_default().trim();
        match key {
            "PYTHON" => environment.python_version = nonempty_probe_value(value),
            "TORCH" => environment.torch_version = nonempty_probe_value(value),
            "CUDA" => environment.torch_cuda_version = nonempty_probe_value(value),
            "CUDNN" => environment.cudnn_version = nonempty_probe_value(value),
            "CUDA_AVAILABLE" => {
                environment.cuda_available = parse_probe_bool(value);
            }
            "DISTRIBUTED_AVAILABLE" => {
                environment.distributed_available = parse_probe_bool(value);
            }
            "NCCL_AVAILABLE" => {
                environment.nccl_available = parse_probe_bool(value);
            }
            "DEVICE_COUNT" => {
                environment.device_count = value.parse::<usize>().unwrap_or(0);
            }
            "DEVICE" => {
                if let Some(device) = parse_pytorch_cuda_device(value) {
                    environment.devices.push(device);
                }
            }
            "ERROR" if !value.is_empty() => environment.error = Some(value.to_owned()),
            "NOTE" if !value.is_empty() => environment.notes.push(value.to_owned()),
            _ => {}
        }
    }

    if environment.device_count == 0 {
        environment.device_count = environment.devices.len();
    }
    environment.ok = environment.error.is_none();
    environment
}

fn parse_pytorch_cuda_benchmark_output(output: &str, python: &str) -> PyTorchCudaBenchmarkOutput {
    let environment = parse_pytorch_cuda_probe_output(output, python);
    let mut benchmark = PyTorchCudaBenchmarkOutput {
        environment,
        device_index: None,
        gpu_name: None,
        measured_steps: 0,
        gpu_step_ms: Vec::new(),
        wall_step_ms: Vec::new(),
        forward_loss_ms: Vec::new(),
        backward_ms: Vec::new(),
        optimizer_ms: Vec::new(),
        peak_allocated_bytes: 0,
        peak_reserved_bytes: 0,
        validation: None,
        time_limited: false,
    };

    for line in output.lines() {
        let mut parts = line.splitn(2, '\t');
        let key = parts.next().unwrap_or_default();
        let value = parts.next().unwrap_or_default().trim();
        match key {
            "RESULT_DEVICE_INDEX" => benchmark.device_index = value.parse().ok(),
            "RESULT_GPU_NAME" => benchmark.gpu_name = nonempty_probe_value(value),
            "RESULT_MEASURED_STEPS" => {
                benchmark.measured_steps = value.parse::<usize>().unwrap_or(0);
            }
            "RESULT_GPU_STEP_MS" => {
                benchmark.gpu_step_ms = parse_tabbed_f64_values(value);
            }
            "RESULT_WALL_STEP_MS" => {
                benchmark.wall_step_ms = parse_tabbed_f64_values(value);
            }
            "RESULT_FORWARD_LOSS_MS" => {
                benchmark.forward_loss_ms = parse_tabbed_f64_values(value);
            }
            "RESULT_BACKWARD_MS" => {
                benchmark.backward_ms = parse_tabbed_f64_values(value);
            }
            "RESULT_OPTIMIZER_MS" => {
                benchmark.optimizer_ms = parse_tabbed_f64_values(value);
            }
            "RESULT_PEAK_ALLOCATED_BYTES" => {
                benchmark.peak_allocated_bytes = value.parse::<u64>().unwrap_or(0);
            }
            "RESULT_PEAK_RESERVED_BYTES" => {
                benchmark.peak_reserved_bytes = value.parse::<u64>().unwrap_or(0);
            }
            "RESULT_VALIDATION" => benchmark.validation = nonempty_probe_value(value),
            "RESULT_TIME_LIMITED" => benchmark.time_limited = parse_probe_bool(value),
            _ => {}
        }
    }

    benchmark
}

fn parse_pytorch_cuda_device(value: &str) -> Option<PyTorchCudaDevice> {
    let columns = value.split('\t').collect::<Vec<_>>();
    Some(PyTorchCudaDevice {
        index: columns.first()?.parse().ok()?,
        name: columns.get(1)?.to_string(),
        capability_major: columns.get(2)?.parse().ok()?,
        capability_minor: columns.get(3)?.parse().ok()?,
        total_memory_bytes: columns.get(4)?.parse().ok()?,
    })
}

fn pytorch_cuda_environment_has_device(
    environment: &PyTorchCudaEnvironment,
    device_index: usize,
) -> bool {
    environment
        .devices
        .iter()
        .any(|device| device.index == device_index)
        || (environment.devices.is_empty() && device_index < environment.device_count)
}

fn nonempty_probe_value(value: &str) -> Option<String> {
    (!value.is_empty() && value != "None").then(|| value.to_owned())
}

fn parse_probe_bool(value: &str) -> bool {
    value.eq_ignore_ascii_case("true")
        || value == "1"
        || value.eq_ignore_ascii_case("yes")
}

fn parse_tabbed_f64_values(value: &str) -> Vec<f64> {
    value
        .split('\t')
        .filter_map(|part| part.trim().parse::<f64>().ok())
        .collect()
}

fn pytorch_cuda_benchmark_timeout(time_limit_s: f64) -> Duration {
    let benchmark_limit = if time_limit_s.is_finite() && time_limit_s > 0.0 {
        time_limit_s.ceil() as u64
    } else {
        10
    };
    Duration::from_secs(benchmark_limit.saturating_add(180).max(180))
}

fn run_pytorch_cuda_probe_script(python: &str, timeout: Duration) -> Result<String> {
    let script = r#"
import sys
print("PYTHON\t" + sys.version.split()[0])
try:
    import torch
    print("TORCH\t" + str(torch.__version__))
    print("CUDA\t" + str(torch.version.cuda or ""))
    try:
        cudnn_version = torch.backends.cudnn.version()
        print("CUDNN\t" + str(cudnn_version or ""))
    except Exception as exc:
        print("CUDNN\t")
        print("ERROR\tCould not read cuDNN version: " + str(exc))
    try:
        distributed = bool(hasattr(torch, "distributed") and torch.distributed.is_available())
        print("DISTRIBUTED_AVAILABLE\t" + str(distributed))
        if distributed and hasattr(torch.distributed, "is_nccl_available"):
            print("NCCL_AVAILABLE\t" + str(bool(torch.distributed.is_nccl_available())))
        else:
            print("NCCL_AVAILABLE\tFalse")
    except Exception as exc:
        print("DISTRIBUTED_AVAILABLE\tFalse")
        print("NCCL_AVAILABLE\tFalse")
        print("ERROR\tCould not read distributed backend availability: " + str(exc))
    available = bool(torch.cuda.is_available())
    print("CUDA_AVAILABLE\t" + str(available))
    count = int(torch.cuda.device_count()) if available else 0
    print("DEVICE_COUNT\t" + str(count))
    for index in range(count):
        props = torch.cuda.get_device_properties(index)
        major, minor = torch.cuda.get_device_capability(index)
        print("DEVICE\t{}\t{}\t{}\t{}\t{}".format(
            index,
            props.name,
            major,
            minor,
            int(props.total_memory),
        ))
except Exception as exc:
    print("ERROR\t" + type(exc).__name__ + ": " + str(exc))
"#;

    let mut command = Command::new(python);
    command
        .arg("-c")
        .arg(script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW_RAW);

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to start Python executable {python}"))?;
    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .with_context(|| format!("failed to query Python probe status for {python}"))?
            .is_some()
        {
            break;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(
                "PyTorch CUDA probe timed out after {} seconds",
                timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }

    let output = child
        .wait_with_output()
        .with_context(|| format!("failed to collect Python probe output from {python}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if output.status.success() {
        return Ok(stdout);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(anyhow!(
        "PyTorch CUDA probe failed{}",
        if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        }
    ))
}

fn run_pytorch_cuda_benchmark_script(
    config: &AiTrainingConfig,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<String> {
    let python = config.pytorch_python.trim();
    if python.is_empty() {
        return Err(anyhow!("Python executable is required for PyTorch CUDA benchmarking"));
    }
    let time_limit = if config.time_limit_s.is_finite() && config.time_limit_s > 0.0 {
        config.time_limit_s
    } else {
        10.0
    };
    let script = r#"
import argparse
import math
import sys
import time
import traceback

def clean(value):
    return str(value).replace("\t", " ").replace("\n", " ")

def emit(key, *values):
    if values:
        print(key + "\t" + "\t".join(clean(value) for value in values), flush=True)
    else:
        print(key, flush=True)

parser = argparse.ArgumentParser()
parser.add_argument("--device", type=int, default=0)
parser.add_argument("--workload", choices=("linear", "mlp", "transformer"), required=True)
parser.add_argument("--precision", choices=("f32", "bf16", "f16"), required=True)
parser.add_argument("--batch", type=int, required=True)
parser.add_argument("--input", type=int, required=True)
parser.add_argument("--output", type=int, required=True)
parser.add_argument("--sequence", type=int, required=True)
parser.add_argument("--hidden", type=int, required=True)
parser.add_argument("--heads", type=int, required=True)
parser.add_argument("--warmup", type=int, required=True)
parser.add_argument("--measured", type=int, required=True)
parser.add_argument("--time-limit", type=float, required=True)
parser.add_argument("--learning-rate", type=float, default=1.0e-4)
args = parser.parse_args()

emit("PYTHON", sys.version.split()[0])
try:
    import torch
    import torch.nn as nn
    import torch.nn.functional as F

    emit("TORCH", getattr(torch, "__version__", ""))
    emit("CUDA", getattr(torch.version, "cuda", "") or "")
    try:
        emit("CUDNN", torch.backends.cudnn.version() or "")
    except Exception as exc:
        emit("CUDNN", "")
        emit("NOTE", "Could not read cuDNN version: " + str(exc))
    try:
        distributed = bool(hasattr(torch, "distributed") and torch.distributed.is_available())
        emit("DISTRIBUTED_AVAILABLE", distributed)
        if distributed and hasattr(torch.distributed, "is_nccl_available"):
            emit("NCCL_AVAILABLE", bool(torch.distributed.is_nccl_available()))
        else:
            emit("NCCL_AVAILABLE", False)
    except Exception as exc:
        emit("DISTRIBUTED_AVAILABLE", False)
        emit("NCCL_AVAILABLE", False)
        emit("NOTE", "Could not read distributed backend availability: " + str(exc))

    available = bool(torch.cuda.is_available())
    emit("CUDA_AVAILABLE", available)
    count = int(torch.cuda.device_count()) if available else 0
    emit("DEVICE_COUNT", count)
    for index in range(count):
        props = torch.cuda.get_device_properties(index)
        major, minor = torch.cuda.get_device_capability(index)
        emit("DEVICE", index, props.name, major, minor, int(props.total_memory))

    if not available:
        emit("ERROR", "torch.cuda.is_available() is false")
        sys.exit(0)
    if args.device < 0 or args.device >= count:
        emit("ERROR", "CUDA device {} is not available".format(args.device))
        sys.exit(0)
    if args.batch <= 0 or args.input <= 0 or args.output <= 0 or args.measured <= 0:
        emit("ERROR", "batch, input, output, and measured steps must be positive")
        sys.exit(0)
    if args.sequence <= 0 or args.hidden <= 0 or args.heads <= 0:
        emit("ERROR", "sequence, hidden, and heads must be positive")
        sys.exit(0)

    torch.cuda.set_device(args.device)
    device = torch.device("cuda:{}".format(args.device))
    props = torch.cuda.get_device_properties(args.device)
    torch.manual_seed(1234)
    torch.cuda.manual_seed_all(1234)
    try:
        torch.backends.cuda.matmul.allow_tf32 = True
        torch.backends.cudnn.allow_tf32 = True
    except Exception:
        pass
    try:
        torch.set_float32_matmul_precision("high")
    except Exception:
        pass

    if args.precision == "f32":
        dtype = torch.float32
    elif args.precision == "bf16":
        if hasattr(torch.cuda, "is_bf16_supported") and not torch.cuda.is_bf16_supported():
            emit("ERROR", "bf16 is not supported on this CUDA device")
            sys.exit(0)
        dtype = torch.bfloat16
    else:
        dtype = torch.float16

    def make_optimizer(parameters):
        try:
            return torch.optim.AdamW(parameters, lr=args.learning_rate, fused=True)
        except Exception as exc:
            emit("NOTE", "Fused AdamW unavailable; using standard AdamW: " + str(exc))
            return torch.optim.AdamW(parameters, lr=args.learning_rate)

    class BenchMlp(nn.Module):
        def __init__(self, hidden, expansion):
            super().__init__()
            self.fc1 = nn.Linear(hidden, expansion, bias=False)
            self.fc2 = nn.Linear(expansion, hidden, bias=False)

        def forward(self, x):
            return self.fc2(F.gelu(self.fc1(x), approximate="tanh"))

    class BenchTransformerBlock(nn.Module):
        def __init__(self, hidden, heads):
            super().__init__()
            if hidden % heads != 0:
                raise ValueError("hidden size must be divisible by attention heads")
            self.hidden = hidden
            self.heads = heads
            self.head_dim = hidden // heads
            self.ln1 = nn.LayerNorm(hidden)
            self.qkv = nn.Linear(hidden, hidden * 3, bias=False)
            self.proj = nn.Linear(hidden, hidden, bias=False)
            self.ln2 = nn.LayerNorm(hidden)
            self.fc1 = nn.Linear(hidden, hidden * 4, bias=False)
            self.fc2 = nn.Linear(hidden * 4, hidden, bias=False)

        def forward(self, x):
            batch, seq, hidden = x.shape
            residual = x
            x_norm = self.ln1(x)
            qkv = self.qkv(x_norm)
            qkv = qkv.reshape(batch, seq, 3, self.heads, self.head_dim).permute(2, 0, 3, 1, 4)
            q, k, v = qkv[0], qkv[1], qkv[2]
            if hasattr(F, "scaled_dot_product_attention"):
                attn = F.scaled_dot_product_attention(q, k, v, dropout_p=0.0, is_causal=False)
            else:
                scores = torch.matmul(q, k.transpose(-2, -1)) / math.sqrt(float(self.head_dim))
                attn = torch.matmul(torch.softmax(scores, dim=-1), v)
            attn = attn.transpose(1, 2).contiguous().view(batch, seq, hidden)
            x = residual + self.proj(attn)
            residual = x
            x_norm = self.ln2(x)
            x = residual + self.fc2(F.gelu(self.fc1(x_norm), approximate="tanh"))
            return x

    if args.workload == "linear":
        model = nn.Linear(args.input, args.output, bias=False)
        x = torch.randn(args.batch, args.input, device=device, dtype=dtype)
        target = torch.randn(args.batch, args.output, device=device, dtype=dtype)
    elif args.workload == "mlp":
        model = BenchMlp(args.hidden, args.output)
        x = torch.randn(args.batch, args.hidden, device=device, dtype=dtype)
        target = torch.randn(args.batch, args.hidden, device=device, dtype=dtype)
    else:
        model = BenchTransformerBlock(args.hidden, args.heads)
        x = torch.randn(args.batch, args.sequence, args.hidden, device=device, dtype=dtype)
        target = torch.randn(args.batch, args.sequence, args.hidden, device=device, dtype=dtype)

    model = model.to(device=device, dtype=dtype)
    optimizer = make_optimizer(model.parameters())

    def train_step():
        optimizer.zero_grad(set_to_none=True)
        y = model(x)
        loss = F.mse_loss(y.float(), target.float())
        loss.backward()
        optimizer.step()
        return loss

    torch.cuda.synchronize()
    for _ in range(max(args.warmup, 0)):
        loss = train_step()
    torch.cuda.synchronize()
    try:
        torch.cuda.reset_peak_memory_stats(device)
    except Exception as exc:
        emit("NOTE", "Could not reset peak CUDA memory stats: " + str(exc))

    gpu_step_ms = []
    wall_step_ms = []
    forward_loss_ms = []
    backward_ms = []
    optimizer_ms = []
    measured_started = time.perf_counter()
    time_limited = False
    loss = None
    for step in range(args.measured):
        start_event = torch.cuda.Event(enable_timing=True)
        end_event = torch.cuda.Event(enable_timing=True)
        forward_start = torch.cuda.Event(enable_timing=True)
        forward_end = torch.cuda.Event(enable_timing=True)
        backward_start = torch.cuda.Event(enable_timing=True)
        backward_end = torch.cuda.Event(enable_timing=True)
        optimizer_start = torch.cuda.Event(enable_timing=True)
        optimizer_end = torch.cuda.Event(enable_timing=True)
        wall_started = time.perf_counter()
        start_event.record()
        optimizer.zero_grad(set_to_none=True)
        forward_start.record()
        y = model(x)
        loss = F.mse_loss(y.float(), target.float())
        forward_end.record()
        backward_start.record()
        loss.backward()
        backward_end.record()
        optimizer_start.record()
        optimizer.step()
        optimizer_end.record()
        end_event.record()
        torch.cuda.synchronize()
        wall_ms = (time.perf_counter() - wall_started) * 1000.0
        gpu_ms = float(start_event.elapsed_time(end_event))
        wall_step_ms.append(wall_ms)
        gpu_step_ms.append(gpu_ms)
        forward_loss_ms.append(float(forward_start.elapsed_time(forward_end)))
        backward_ms.append(float(backward_start.elapsed_time(backward_end)))
        optimizer_ms.append(float(optimizer_start.elapsed_time(optimizer_end)))
        if step + 1 < args.measured and (time.perf_counter() - measured_started) >= args.time_limit:
            time_limited = True
            break

    final_loss = float(loss.detach().cpu()) if loss is not None else float("nan")
    if not math.isfinite(final_loss):
        emit("ERROR", "final loss is not finite")
        sys.exit(0)

    try:
        peak_allocated = int(torch.cuda.max_memory_allocated(device))
        peak_reserved = int(torch.cuda.max_memory_reserved(device))
    except Exception as exc:
        peak_allocated = 0
        peak_reserved = 0
        emit("NOTE", "Could not read peak CUDA memory stats: " + str(exc))

    emit("RESULT_DEVICE_INDEX", args.device)
    emit("RESULT_GPU_NAME", props.name)
    emit("RESULT_WORKLOAD", args.workload)
    emit("RESULT_PRECISION", args.precision)
    emit("RESULT_MEASURED_STEPS", len(gpu_step_ms))
    emit("RESULT_GPU_STEP_MS", *["{:.6f}".format(value) for value in gpu_step_ms])
    emit("RESULT_WALL_STEP_MS", *["{:.6f}".format(value) for value in wall_step_ms])
    emit("RESULT_FORWARD_LOSS_MS", *["{:.6f}".format(value) for value in forward_loss_ms])
    emit("RESULT_BACKWARD_MS", *["{:.6f}".format(value) for value in backward_ms])
    emit("RESULT_OPTIMIZER_MS", *["{:.6f}".format(value) for value in optimizer_ms])
    emit("RESULT_PEAK_ALLOCATED_BYTES", peak_allocated)
    emit("RESULT_PEAK_RESERVED_BYTES", peak_reserved)
    emit("RESULT_VALIDATION", "Passed: finite loss {:.6g}".format(final_loss))
    emit("RESULT_TIME_LIMITED", time_limited)
    emit("NOTE", "PyTorch CUDA benchmark keeps tensors resident, runs forward/backward/AdamW steps, times with CUDA events, and reads back only final loss.")
except Exception as exc:
    emit("ERROR", type(exc).__name__ + ": " + str(exc))
    emit("NOTE", traceback.format_exc(limit=6))
"#;

    let mut command = Command::new(python);
    command
        .arg("-c")
        .arg(script)
        .arg("--device")
        .arg(config.pytorch_cuda_device.to_string())
        .arg("--workload")
        .arg(pytorch_cuda_workload_arg(config.workload))
        .arg("--precision")
        .arg(pytorch_cuda_precision_arg(config.precision))
        .arg("--batch")
        .arg(config.dimensions.batch_size.to_string())
        .arg("--input")
        .arg(config.dimensions.input_dim.to_string())
        .arg("--output")
        .arg(config.dimensions.output_dim.to_string())
        .arg("--sequence")
        .arg(config.dimensions.sequence_len.to_string())
        .arg("--hidden")
        .arg(config.dimensions.hidden_size.to_string())
        .arg("--heads")
        .arg(config.dimensions.attention_heads.to_string())
        .arg("--warmup")
        .arg(config.warmup_steps.to_string())
        .arg("--measured")
        .arg(config.measured_steps.to_string())
        .arg("--time-limit")
        .arg(format!("{time_limit:.3}"))
        .arg("--learning-rate")
        .arg("0.0001")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW_RAW);

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to start Python executable {python}"))?;
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!("PyTorch CUDA benchmark canceled"));
        }
        if child
            .try_wait()
            .with_context(|| format!("failed to query Python benchmark status for {python}"))?
            .is_some()
        {
            break;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(
                "PyTorch CUDA benchmark timed out after {} seconds",
                timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }

    let output = child
        .wait_with_output()
        .with_context(|| format!("failed to collect Python benchmark output from {python}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if output.status.success() {
        return Ok(stdout);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(anyhow!(
        "PyTorch CUDA benchmark process failed{}",
        if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        }
    ))
}

fn pytorch_cuda_workload_arg(workload: AiTrainingWorkload) -> &'static str {
    match workload {
        AiTrainingWorkload::LinearLayer => "linear",
        AiTrainingWorkload::Mlp => "mlp",
        AiTrainingWorkload::TransformerBlock => "transformer",
        AiTrainingWorkload::OptimizerStress => "optimizer",
    }
}

fn pytorch_cuda_precision_arg(precision: AiTrainingPrecision) -> &'static str {
    match precision {
        AiTrainingPrecision::F32 => "f32",
        AiTrainingPrecision::Bf16 => "bf16",
        AiTrainingPrecision::F16 => "f16",
    }
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}
