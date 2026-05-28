#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SetupItemState {
    Ready,
    Attention,
    Installing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SetupAction {
    InstallManagedPytorchCuda,
    ProbeManagedPytorchCuda,
    BuildSensorServiceCompanion,
    InstallHardwareMonitorWmi,
}

#[derive(Clone, Debug)]
struct SetupAssistantItem {
    title: &'static str,
    requirement: &'static str,
    state: SetupItemState,
    detail: String,
    action: Option<SetupAction>,
}

impl SetupAction {
    fn label(self) -> &'static str {
        match self {
            Self::InstallManagedPytorchCuda => "Install",
            Self::ProbeManagedPytorchCuda => "Probe",
            Self::BuildSensorServiceCompanion => "Install",
            Self::InstallHardwareMonitorWmi => "Install",
        }
    }
}

fn detect_setup_environment(adapters: &[AdapterInfo]) -> SetupDetection {
    let has_nvidia = adapters
        .iter()
        .any(|adapter| adapter_vendor(adapter) == GpuVendor::Nvidia);

    SetupDetection {
        elevated: is_process_elevated(),
        vcruntime_available: vcruntime_available(),
        nvidia_smi_available: has_nvidia.then(nvidia_smi_available),
        hardware_monitor_wmi_available: hardware_monitor_wmi_available(),
        sensor_service_available: sensor_service_path().is_some(),
        managed_pytorch_python: managed_pytorch_cuda_python_executable()
            .filter(|python| PathBuf::from(python).is_file()),
        managed_pytorch_install_base_available: find_python_for_managed_pytorch_cuda().is_ok(),
    }
}

impl BenchScopeApp {
    fn refresh_setup_detection(&mut self) {
        self.setup_detection = detect_setup_environment(&self.adapters);
    }

    fn setup_items(&self) -> Vec<SetupAssistantItem> {
        let has_nvidia = self
            .adapters
            .iter()
            .any(|adapter| adapter_vendor(adapter) == GpuVendor::Nvidia);
        let setup_progress_title = self
            .setup_task_progress
            .as_ref()
            .map(|progress| progress.title.as_str())
            .unwrap_or_default();
        let setup_pytorch_running =
            self.setup_task_running && setup_progress_title.contains("PyTorch");
        let setup_sensor_service_running =
            self.setup_task_running && setup_progress_title.contains("Sensor service");
        let setup_hardware_monitor_running =
            self.setup_task_running && setup_progress_title.contains("LibreHardwareMonitor");
        let pytorch_ready = self.pytorch_probe.as_ref().is_some_and(|environment| {
            environment.cuda_available
                && self
                    .setup_detection
                    .managed_pytorch_python
                    .as_deref()
                    .is_some_and(|python| environment.python_executable.eq_ignore_ascii_case(python))
        }) || self.ai_training.pytorch_probe.as_ref().is_some_and(|environment| {
            environment.cuda_available
                && self
                    .setup_detection
                    .managed_pytorch_python
                    .as_deref()
                    .is_some_and(|python| environment.python_executable.eq_ignore_ascii_case(python))
        });

        let mut items = Vec::new();
        items.push(SetupAssistantItem {
            title: "Administrator access",
            requirement: "Required",
            state: if self.setup_detection.elevated {
                SetupItemState::Ready
            } else {
                SetupItemState::Attention
            },
            detail: if self.setup_detection.elevated {
                "BenchScope is running elevated.".to_owned()
            } else {
                "Restart BenchScope as administrator for raw storage and sensor paths.".to_owned()
            },
            action: None,
        });
        items.push(SetupAssistantItem {
            title: "GPU compute backend",
            requirement: "Required for GPU tests",
            state: if self
                .adapters
                .iter()
                .any(|adapter| adapter.device_type != wgpu::DeviceType::Cpu)
            {
                SetupItemState::Ready
            } else {
                SetupItemState::Attention
            },
            detail: if self.adapters.is_empty() {
                "No WGPU adapters were detected. Install the OEM or vendor graphics driver."
                    .to_owned()
            } else {
                format!("Detected {} GPU adapter(s).", self.adapters.len())
            },
            action: None,
        });
        items.push(SetupAssistantItem {
            title: "Visual C++ runtime",
            requirement: "Required for packaged builds",
            state: if self.setup_detection.vcruntime_available {
                SetupItemState::Ready
            } else {
                SetupItemState::Attention
            },
            detail: if self.setup_detection.vcruntime_available {
                "VCRUNTIME140 is available.".to_owned()
            } else {
                "Install the Microsoft Visual C++ 2015-2022 x64 Redistributable, or use a packaged BenchScope build that ships app-local runtime DLLs.".to_owned()
            },
            action: None,
        });

        let pytorch_state = if self.pytorch_probe_running
            || self.pytorch_install_running
            || self.ai_training.pytorch_probe_running
            || self.ai_training.pytorch_install_running
            || setup_pytorch_running
        {
            SetupItemState::Installing
        } else if !has_nvidia || pytorch_ready {
            SetupItemState::Ready
        } else {
            SetupItemState::Attention
        };
        let pytorch_action = if has_nvidia && pytorch_state == SetupItemState::Attention {
            if self.setup_detection.managed_pytorch_python.is_some() {
                Some(SetupAction::ProbeManagedPytorchCuda)
            } else if self.setup_detection.managed_pytorch_install_base_available {
                Some(SetupAction::InstallManagedPytorchCuda)
            } else {
                None
            }
        } else {
            None
        };
        items.push(SetupAssistantItem {
            title: "Managed PyTorch CUDA",
            requirement: "Optional accelerator",
            state: pytorch_state,
            detail: if !has_nvidia {
                "No NVIDIA adapter was detected, so CUDA PyTorch is not needed.".to_owned()
            } else if pytorch_ready {
                "CUDA PyTorch is verified for the managed BenchScope environment.".to_owned()
            } else if let Some(python) = &self.setup_detection.managed_pytorch_python {
                format!("Managed environment exists at {python}. Probe it to verify CUDA.")
            } else if self.setup_detection.managed_pytorch_install_base_available {
                format!(
                    "Install CUDA PyTorch into {}.",
                    managed_pytorch_cuda_python_executable()
                        .unwrap_or_else(|| "the BenchScope managed environment".to_owned())
                )
            } else {
                "Install Python 3.10+ first, then BenchScope can create its managed CUDA PyTorch environment under LocalAppData.".to_owned()
            },
            action: pytorch_action,
        });

        if has_nvidia {
            let nvidia_smi_ready = self.setup_detection.nvidia_smi_available.unwrap_or(false);
            items.push(SetupAssistantItem {
                title: "NVIDIA telemetry",
                requirement: "Optional sensors",
                state: if nvidia_smi_ready {
                    SetupItemState::Ready
                } else {
                    SetupItemState::Attention
                },
                detail: if nvidia_smi_ready {
                    "nvidia-smi is available for NVIDIA telemetry.".to_owned()
                } else {
                    "Install or repair the NVIDIA display driver so nvidia-smi is available for temperature, utilization, and VRAM telemetry.".to_owned()
                },
                action: None,
            });
        }

        items.push(SetupAssistantItem {
            title: "Sensor service companion",
            requirement: "Optional sensors",
            state: if self.setup_detection.sensor_service_available {
                SetupItemState::Ready
            } else if setup_sensor_service_running {
                SetupItemState::Installing
            } else {
                SetupItemState::Attention
            },
            detail: if self.setup_detection.sensor_service_available {
                "benchscope_sensor_service is available beside the app or in the build tree."
                    .to_owned()
            } else if setup_sensor_service_running {
                "Installing benchscope_sensor_service from this source checkout.".to_owned()
            } else {
                "Build benchscope_sensor_service into the local target directory so BenchScope can launch the optional sensor bridge.".to_owned()
            },
            action: (!self.setup_detection.sensor_service_available)
                .then_some(SetupAction::BuildSensorServiceCompanion),
        });
        items.push(SetupAssistantItem {
            title: "Libre/OpenHardwareMonitor WMI",
            requirement: "Optional sensors",
            state: if self.setup_detection.hardware_monitor_wmi_available {
                SetupItemState::Ready
            } else if setup_hardware_monitor_running {
                SetupItemState::Installing
            } else {
                SetupItemState::Attention
            },
            detail: if self.setup_detection.hardware_monitor_wmi_available {
                "LibreHardwareMonitor or OpenHardwareMonitor WMI sensors are visible.".to_owned()
            } else if setup_hardware_monitor_running {
                "Installing LibreHardwareMonitor and waiting for elevated WMI sensors.".to_owned()
            } else {
                "Install LibreHardwareMonitor under LocalAppData and launch it as administrator so BenchScope can read its WMI sensors.".to_owned()
            },
            action: (!self.setup_detection.hardware_monitor_wmi_available)
                .then_some(SetupAction::InstallHardwareMonitorWmi),
        });

        items
    }

    fn ui_setup_panel(&mut self, ui: &mut egui::Ui) {
        let items = self
            .setup_items()
            .into_iter()
            .filter(|item| item.state != SetupItemState::Ready)
            .collect::<Vec<_>>();
        if items.is_empty() {
            return;
        }
        let setup_progress = self.setup_task_progress.clone();
        let install_actions = setup_install_actions(&items);

        ui.add_space(12.0);
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(22, 25, 31))
            .stroke(egui::Stroke::new(
                1.0,
                egui::Color32::from_rgb(68, 76, 90),
            ))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(16, 14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("Setup & dependencies")
                            .strong()
                            .size(17.0)
                            .color(egui::Color32::from_rgb(236, 240, 246)),
                    );
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("{} item(s) need attention", items.len()))
                            .size(14.5)
                            .color(egui::Color32::from_rgb(176, 185, 198)),
                    );
                    if ui.button("Refresh").clicked() {
                        self.refresh_setup_detection();
                    }
                    if install_actions.len() > 1 {
                        let enabled = self.setup_action_enabled();
                        ui.add_enabled_ui(enabled, |ui| {
                            if ui.button("Install all").clicked() {
                                self.start_setup_action_sequence(install_actions.clone());
                            }
                        });
                    }
                });

                ui.add_space(8.0);
                if let Some(progress) = &setup_progress {
                    ui.add(
                        egui::ProgressBar::new(progress.progress.clamp(0.0, 1.0))
                            .desired_width(ui.available_width())
                            .text(progress.title.as_str()),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&progress.detail)
                                .size(13.0)
                                .color(egui::Color32::from_rgb(174, 188, 206)),
                        )
                        .wrap(),
                    );
                    ui.add_space(8.0);
                }
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        ui.separator();
                    }
                    self.ui_setup_panel_item(ui, item);
                }
            });
    }

    fn ui_setup_panel_item(&mut self, ui: &mut egui::Ui, item: &SetupAssistantItem) {
        let (status, color) = setup_state_label_color(item.state);
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(color, status);
            ui.label(
                egui::RichText::new(item.title)
                    .strong()
                    .color(egui::Color32::from_rgb(234, 238, 244)),
            );
            ui.label(
                egui::RichText::new(item.requirement)
                    .size(13.0)
                    .color(egui::Color32::from_rgb(151, 162, 178)),
            );
            if let Some(action) = item.action {
                let enabled = self.setup_action_enabled();
                ui.add_enabled_ui(enabled, |ui| {
                    if ui
                        .add_sized([92.0, 30.0], egui::Button::new(action.label()))
                        .clicked()
                    {
                        self.run_setup_action(action);
                    }
                });
            }
        });
        ui.add(
            egui::Label::new(
                egui::RichText::new(&item.detail)
                    .size(14.0)
                    .color(egui::Color32::from_rgb(181, 191, 204)),
            )
            .wrap(),
        );
    }

    fn setup_action_enabled(&self) -> bool {
        !self.running
            && !self.pytorch_probe_running
            && !self.pytorch_install_running
            && !self.ai_training.pytorch_probe_running
            && !self.ai_training.pytorch_install_running
            && !self.setup_task_running
    }

    fn run_setup_action(&mut self, action: SetupAction) {
        match action {
            SetupAction::InstallManagedPytorchCuda => self.start_managed_pytorch_cuda_install(),
            SetupAction::ProbeManagedPytorchCuda => self.start_managed_pytorch_cuda_probe(),
            SetupAction::BuildSensorServiceCompanion
            | SetupAction::InstallHardwareMonitorWmi => self.start_setup_action_sequence(vec![action]),
        }
    }

    fn start_managed_pytorch_cuda_probe(&mut self) {
        if self.running
            || self.pytorch_probe_running
            || self.pytorch_install_running
            || self.ai_training.pytorch_probe_running
            || self.ai_training.pytorch_install_running
            || self.setup_task_running
        {
            return;
        }

        if let Some(python) = self.setup_detection.managed_pytorch_python.clone() {
            self.pytorch_python = python.clone();
            self.ai_training.pytorch_python = python;
            self.start_pytorch_cuda_probe();
            self.setup_task_running = true;
            self.setup_task_progress = Some(setup_task_progress(
                "Managed PyTorch CUDA",
                "Probing the managed CUDA PyTorch environment",
                0.35,
            ));
        }
    }

    fn start_managed_pytorch_cuda_install(&mut self) {
        if self.running
            || self.pytorch_probe_running
            || self.pytorch_install_running
            || self.ai_training.pytorch_probe_running
            || self.ai_training.pytorch_install_running
            || self.setup_task_running
        {
            return;
        }

        let tx = self.tx.clone();
        self.pending_pytorch_install = false;
        self.pytorch_install_running = true;
        self.pytorch_probe_running = true;
        self.setup_task_running = true;
        self.setup_task_progress = Some(setup_task_progress(
            "Managed PyTorch CUDA",
            format!(
                "Downloading CUDA PyTorch wheels ({PYTORCH_CUDA_INSTALL_DOWNLOAD_NOTE})"
            ),
            0.08,
        ));
        self.status = "Installing managed PyTorch CUDA...".to_owned();
        self.eta_text = "Large download in progress".to_owned();
        self.log("User approved managed PyTorch CUDA install");

        thread::spawn(move || {
            let log_tx = tx.clone();
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                install_managed_pytorch_cuda(|line| {
                    let _ = log_tx.send(WorkerEvent::Log(format!(
                        "Managed PyTorch install: {line}"
                    )));
                })
            }))
            .map_err(|panic| {
                format!(
                    "Managed PyTorch CUDA install panicked: {}",
                    panic_message(&*panic)
                )
            })
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(WorkerEvent::PyTorchInstallDone(result));
        });
    }

    fn start_setup_action_sequence(&mut self, actions: Vec<SetupAction>) {
        if !self.setup_action_enabled() {
            return;
        }
        let actions = deduplicate_setup_actions(actions);
        if actions.is_empty() {
            return;
        }

        let tx = self.tx.clone();
        let plural = actions.len() > 1;
        self.setup_task_running = true;
        self.setup_task_progress = Some(setup_task_progress(
            if plural {
                "Installing dependencies"
            } else {
                setup_action_title(actions[0])
            },
            "Starting setup task",
            0.0,
        ));
        self.status = if plural {
            "Installing BenchScope dependencies...".to_owned()
        } else {
            format!("Installing {}...", setup_action_title(actions[0]))
        };
        self.eta_text = "Starting setup task".to_owned();
        self.log(if plural {
            format!("User requested install all for {} dependency setup items", actions.len())
        } else {
            format!("User requested {}", setup_action_log_name(actions[0]))
        });

        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                run_setup_action_sequence(actions, tx.clone())
            }))
            .map_err(|panic| {
                format!(
                    "BenchScope dependency setup panicked: {}",
                    panic_message(&*panic)
                )
            })
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(WorkerEvent::SetupTaskDone(result));
        });
    }
}

fn setup_state_label_color(state: SetupItemState) -> (&'static str, egui::Color32) {
    match state {
        SetupItemState::Ready => ("Ready", egui::Color32::from_rgb(110, 210, 130)),
        SetupItemState::Attention => ("Needs setup", egui::Color32::YELLOW),
        SetupItemState::Installing => ("Working", egui::Color32::LIGHT_BLUE),
    }
}

fn setup_install_actions(items: &[SetupAssistantItem]) -> Vec<SetupAction> {
    deduplicate_setup_actions(items.iter().filter_map(|item| item.action).collect())
}

fn deduplicate_setup_actions(actions: Vec<SetupAction>) -> Vec<SetupAction> {
    let mut unique = Vec::new();
    for action in actions {
        if !unique.contains(&action) {
            unique.push(action);
        }
    }
    unique
}

fn setup_action_title(action: SetupAction) -> &'static str {
    match action {
        SetupAction::InstallManagedPytorchCuda | SetupAction::ProbeManagedPytorchCuda => {
            "Managed PyTorch CUDA"
        }
        SetupAction::BuildSensorServiceCompanion => "Sensor service companion",
        SetupAction::InstallHardwareMonitorWmi => "LibreHardwareMonitor WMI",
    }
}

fn setup_action_log_name(action: SetupAction) -> &'static str {
    match action {
        SetupAction::InstallManagedPytorchCuda => "Managed PyTorch install",
        SetupAction::ProbeManagedPytorchCuda => "Managed PyTorch probe",
        SetupAction::BuildSensorServiceCompanion => "Sensor service install",
        SetupAction::InstallHardwareMonitorWmi => "LibreHardwareMonitor install",
    }
}

fn setup_action_start_detail(action: SetupAction) -> &'static str {
    match action {
        SetupAction::InstallManagedPytorchCuda => "Creating the managed environment and downloading CUDA wheels",
        SetupAction::ProbeManagedPytorchCuda => "Verifying the managed CUDA PyTorch environment",
        SetupAction::BuildSensorServiceCompanion => "Compiling benchscope_sensor_service into the local target directory",
        SetupAction::InstallHardwareMonitorWmi => "Downloading and installing LibreHardwareMonitor",
    }
}

fn setup_task_progress(
    title: impl Into<String>,
    detail: impl Into<String>,
    progress: f32,
) -> SetupTaskProgress {
    SetupTaskProgress {
        title: title.into(),
        detail: detail.into(),
        progress: progress.clamp(0.0, 1.0),
    }
}

fn setup_task_outcome(
    message: impl Into<String>,
    pytorch_environment: Option<PyTorchCudaEnvironment>,
) -> SetupTaskOutcome {
    SetupTaskOutcome {
        message: message.into(),
        pytorch_environment,
    }
}

fn run_setup_action_sequence(
    actions: Vec<SetupAction>,
    tx: Sender<WorkerEvent>,
) -> Result<SetupTaskOutcome> {
    let actions = deduplicate_setup_actions(actions);
    let total = actions.len().max(1);
    let mut messages = Vec::new();
    let mut pytorch_environment = None;

    for (index, action) in actions.into_iter().enumerate() {
        let base = index as f32 / total as f32;
        let span = 1.0 / total as f32;
        let step_prefix = (total > 1).then(|| format!("Step {}/{}: ", index + 1, total));

        let progress_tx = tx.clone();
        let mut progress = |progress: SetupTaskProgress| {
            let detail = match &step_prefix {
                Some(prefix) => format!("{prefix}{}", progress.detail),
                None => progress.detail.clone(),
            };
            let title = if total > 1 {
                format!("Installing dependencies: {}", progress.title)
            } else {
                progress.title.clone()
            };
            let _ = progress_tx.send(WorkerEvent::SetupTaskProgress(setup_task_progress(
                title,
                detail,
                base + progress.progress * span,
            )));
        };

        let log_tx = tx.clone();
        let mut log = |line: String| {
            let _ = log_tx.send(WorkerEvent::Log(format!(
                "{}: {line}",
                setup_action_log_name(action)
            )));
        };

        progress(setup_task_progress(
            setup_action_title(action),
            setup_action_start_detail(action),
            0.02,
        ));

        match action {
            SetupAction::InstallManagedPytorchCuda => {
                progress(setup_task_progress(
                    setup_action_title(action),
                    format!(
                        "Downloading CUDA PyTorch wheels ({PYTORCH_CUDA_INSTALL_DOWNLOAD_NOTE})"
                    ),
                    0.10,
                ));
                let environment = install_managed_pytorch_cuda(&mut log)?;
                let device_count = environment.device_count;
                pytorch_environment = Some(environment);
                messages.push(format!(
                    "Managed PyTorch CUDA installed and ready: {device_count} CUDA device(s)"
                ));
            }
            SetupAction::ProbeManagedPytorchCuda => {
                let python = managed_pytorch_cuda_python_executable()
                    .ok_or_else(|| anyhow!("Managed PyTorch CUDA environment was not found"))?;
                let environment = probe_pytorch_cuda(&python)?;
                if !environment.cuda_available {
                    let detail = environment
                        .error
                        .clone()
                        .unwrap_or_else(|| "CUDA is unavailable".to_owned());
                    return Err(anyhow!("Managed PyTorch CUDA probe failed: {detail}"));
                }
                let device_count = environment.device_count;
                pytorch_environment = Some(environment);
                messages.push(format!(
                    "Managed PyTorch CUDA verified: {device_count} CUDA device(s)"
                ));
            }
            SetupAction::BuildSensorServiceCompanion => {
                let message = build_sensor_service_companion(&mut log)?;
                messages.push(message);
            }
            SetupAction::InstallHardwareMonitorWmi => {
                let message = install_hardware_monitor_wmi(&mut progress, &mut log)?;
                messages.push(message);
            }
        }

        progress(setup_task_progress(
            setup_action_title(action),
            "Completed",
            1.0,
        ));
    }

    let message = match messages.len() {
        0 => "BenchScope dependency setup completed".to_owned(),
        1 => messages.remove(0),
        _ => format!("BenchScope dependency setup completed: {}", messages.join("; ")),
    };
    Ok(setup_task_outcome(message, pytorch_environment))
}

fn build_sensor_service_companion<F>(mut log: F) -> Result<String>
where
    F: FnMut(String),
{
    let root = benchscope_source_root().ok_or_else(|| {
        anyhow!(
            "Could not find a BenchScope source checkout with Cargo.toml and src/bin/benchscope_sensor_service.rs"
        )
    })?;
    log(format!("Using source tree {}", root.display()));

    let mut command = Command::new("cargo");
    command
        .current_dir(&root)
        .args([
            "build",
            "--bin",
            "benchscope_sensor_service",
            "--target-dir",
            "target",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW_RAW);

    let output = command
        .output()
        .with_context(|| "failed to start cargo build for benchscope_sensor_service")?;
    log_setup_command_output(&output.stdout, &mut log);
    log_setup_command_output(&output.stderr, &mut log);
    if !output.status.success() {
        return Err(anyhow!(
            "benchscope_sensor_service build failed with status {}",
            output.status
        ));
    }

    let service_path = root
        .join("target")
        .join("debug")
        .join(sensor_service_executable_name());
    if service_path.is_file() {
        Ok(format!(
            "Sensor service companion built at {}",
            service_path.display()
        ))
    } else {
        Ok("Sensor service companion build completed".to_owned())
    }
}

fn benchscope_source_root() -> Option<PathBuf> {
    tool_search_roots().into_iter().find(|root| {
        root.join("Cargo.toml").is_file()
            && root
                .join("src")
                .join("bin")
                .join("benchscope_sensor_service.rs")
                .is_file()
    })
}

fn sensor_service_executable_name() -> &'static str {
    #[cfg(windows)]
    {
        "benchscope_sensor_service.exe"
    }
    #[cfg(not(windows))]
    {
        "benchscope_sensor_service"
    }
}

fn log_setup_command_output<F>(bytes: &[u8], log: &mut F)
where
    F: FnMut(String),
{
    for line in String::from_utf8_lossy(bytes).lines() {
        let line = line.trim();
        if !line.is_empty() {
            log(line.to_owned());
        }
    }
}

const SETUP_PROGRESS_PREFIX: &str = "BENCHSCOPE_PROGRESS\t";
const SETUP_RESULT_PREFIX: &str = "BENCHSCOPE_RESULT\t";

#[cfg(windows)]
fn install_hardware_monitor_wmi<P, L>(mut progress: P, mut log: L) -> Result<String>
where
    P: FnMut(SetupTaskProgress),
    L: FnMut(String),
{
    progress(setup_task_progress(
        "LibreHardwareMonitor WMI",
        "Preparing LibreHardwareMonitor install",
        0.03,
    ));

    let script = r#"
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

function EmitProgress([double]$Value, [string]$Message) {
    $valueText = $Value.ToString('0.00', [Globalization.CultureInfo]::InvariantCulture)
    Write-Output "BENCHSCOPE_PROGRESS`t$valueText`t$Message"
}

EmitProgress 0.08 'Preparing install folder'
$localAppData = [Environment]::GetFolderPath('LocalApplicationData')
if ([string]::IsNullOrWhiteSpace($localAppData)) {
    throw 'LOCALAPPDATA is not available'
}
$installRoot = Join-Path $localAppData 'BenchScope\tools\LibreHardwareMonitor'
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('benchscope-lhm-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null

try {
    $headers = @{ 'User-Agent' = 'BenchScope' }
    EmitProgress 0.16 'Querying latest LibreHardwareMonitor release'
    $release = Invoke-RestMethod -Uri 'https://api.github.com/repos/LibreHardwareMonitor/LibreHardwareMonitor/releases/latest' -Headers $headers
    $asset = $release.assets | Where-Object { $_.name -match '\.zip$' } | Select-Object -First 1
    if (-not $asset) {
        throw 'No LibreHardwareMonitor zip asset was found on the latest release'
    }

    $zipPath = Join-Path $tempRoot $asset.name
    EmitProgress 0.28 ("Downloading " + $asset.name)
    Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $zipPath -Headers $headers

    EmitProgress 0.66 'Extracting LibreHardwareMonitor'
    if (Test-Path -LiteralPath $installRoot) {
        Remove-Item -Recurse -Force -LiteralPath $installRoot
    }
    New-Item -ItemType Directory -Force -Path $installRoot | Out-Null
    Expand-Archive -LiteralPath $zipPath -DestinationPath $installRoot -Force

    $exe = Get-ChildItem -LiteralPath $installRoot -Recurse -Filter 'LibreHardwareMonitor.exe' | Select-Object -First 1
    if (-not $exe) {
        throw 'LibreHardwareMonitor.exe was not found after extraction'
    }

    EmitProgress 0.84 'Launching LibreHardwareMonitor as administrator'
    Start-Process -FilePath $exe.FullName -WorkingDirectory $exe.DirectoryName -Verb RunAs
    Write-Output ("BENCHSCOPE_RESULT`t" + $exe.FullName)
    EmitProgress 0.92 'Waiting for WMI sensors'
} finally {
    Remove-Item -Recurse -Force -LiteralPath $tempRoot -ErrorAction SilentlyContinue
}
"#;

    let installed_exe = run_setup_powershell_script(
        script,
        Duration::from_secs(15 * 60),
        &mut progress,
        &mut log,
    )?
    .unwrap_or_else(|| "LibreHardwareMonitor.exe".to_owned());

    for attempt in 0..40 {
        if hardware_monitor_wmi_available() {
            progress(setup_task_progress(
                "LibreHardwareMonitor WMI",
                "WMI sensors are visible",
                1.0,
            ));
            return Ok(format!(
                "LibreHardwareMonitor installed and WMI sensors are visible ({installed_exe})"
            ));
        }
        if attempt % 4 == 0 {
            progress(setup_task_progress(
                "LibreHardwareMonitor WMI",
                "Waiting for LibreHardwareMonitor WMI sensors",
                0.92 + (attempt as f32 / 40.0) * 0.07,
            ));
        }
        thread::sleep(Duration::from_millis(500));
    }

    Ok(format!(
        "LibreHardwareMonitor installed and launched as administrator from {installed_exe}. Approve the Windows prompt, then refresh setup if WMI is still starting."
    ))
}

#[cfg(not(windows))]
fn install_hardware_monitor_wmi<P, L>(_progress: P, _log: L) -> Result<String>
where
    P: FnMut(SetupTaskProgress),
    L: FnMut(String),
{
    Err(anyhow!(
        "LibreHardwareMonitor WMI setup is only available on Windows"
    ))
}

#[cfg(windows)]
fn run_setup_powershell_script<P, L>(
    script: &str,
    timeout: Duration,
    progress: &mut P,
    log: &mut L,
) -> Result<Option<String>>
where
    P: FnMut(SetupTaskProgress),
    L: FnMut(String),
{
    log("Running PowerShell setup installer".to_owned());
    let mut command = Command::new("powershell");
    command
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.creation_flags(CREATE_NO_WINDOW_RAW);

    let mut child = command
        .spawn()
        .with_context(|| "failed to start PowerShell setup installer")?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("failed to capture PowerShell setup stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("failed to capture PowerShell setup stderr"))?;

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
    let mut installed_exe = None;
    let status = loop {
        while let Ok(line) = line_rx.try_recv() {
            handle_setup_installer_line(&line, &mut installed_exe, progress, log);
        }
        if let Some(status) = child
            .try_wait()
            .with_context(|| "failed to query PowerShell setup installer")?
        {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(
                "PowerShell setup installer timed out after {} seconds",
                timeout.as_secs()
            ));
        }
        if last_heartbeat.elapsed() >= Duration::from_secs(30) {
            log(format!(
                "Still running PowerShell setup installer after {}",
                format_elapsed(started.elapsed().as_secs_f64())
            ));
            last_heartbeat = Instant::now();
        }
        thread::sleep(Duration::from_millis(100));
    };

    let _ = stdout_thread.join();
    let _ = stderr_thread.join();
    while let Ok(line) = line_rx.try_recv() {
        handle_setup_installer_line(&line, &mut installed_exe, progress, log);
    }

    if status.success() {
        Ok(installed_exe)
    } else {
        Err(anyhow!(
            "PowerShell setup installer failed with status {status}"
        ))
    }
}

fn handle_setup_installer_line<P, L>(
    line: &str,
    installed_exe: &mut Option<String>,
    progress: &mut P,
    log: &mut L,
) where
    P: FnMut(SetupTaskProgress),
    L: FnMut(String),
{
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    if let Some(progress_update) =
        parse_setup_progress_line(line, "LibreHardwareMonitor WMI")
    {
        progress(progress_update);
        return;
    }
    if let Some(path) = line.strip_prefix(SETUP_RESULT_PREFIX) {
        let path = path.trim().to_owned();
        log(format!("Installed executable: {path}"));
        *installed_exe = Some(path);
        return;
    }
    log(line.to_owned());
}

fn parse_setup_progress_line(line: &str, title: &str) -> Option<SetupTaskProgress> {
    let rest = line.strip_prefix(SETUP_PROGRESS_PREFIX)?;
    let mut parts = rest.splitn(2, '\t');
    let progress = parts.next()?.trim().parse::<f32>().ok()?;
    let detail = parts.next().unwrap_or("").trim();
    Some(setup_task_progress(title, detail, progress))
}

fn vcruntime_available() -> bool {
    dll_available("vcruntime140.dll")
}

fn dll_available(name: &str) -> bool {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.to_path_buf());
        }
    }
    if let Ok(system_root) = std::env::var("SystemRoot") {
        dirs.push(PathBuf::from(system_root).join("System32"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.into_iter().any(|dir| dir.join(name).is_file())
}

fn nvidia_smi_available() -> bool {
    #[cfg(windows)]
    {
        run_nvidia_smi_temperature_query().is_ok()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn hardware_monitor_wmi_available() -> bool {
    #[cfg(windows)]
    {
        let script = r#"
$namespaces = @('root\LibreHardwareMonitor', 'root\OpenHardwareMonitor')
foreach ($namespace in $namespaces) {
    try {
        $items = Get-CimInstance -Namespace $namespace -ClassName Sensor -ErrorAction Stop | Select-Object -First 1
        if ($items) { 'true'; exit 0 }
    } catch {}
}
'false'
"#;
        run_powershell_sensor_script(script)
            .map(|output| output.trim().eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        false
    }
}
