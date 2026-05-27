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
}

#[derive(Clone, Debug)]
struct SetupAssistantItem {
    title: &'static str,
    requirement: &'static str,
    state: SetupItemState,
    detail: String,
    action: Option<SetupAction>,
}

impl SetupAssistantState {
    fn new() -> Self {
        Self {
            visible: false,
            dismissed_this_session: false,
            dismissed_persisted: setup_dismissed_marker_exists(),
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
    fn setup_should_open(&self) -> bool {
        !self.setup_assistant.dismissed_this_session
            && !self.setup_assistant.dismissed_persisted
            && self
                .setup_items()
                .iter()
                .any(|item| item.state == SetupItemState::Attention)
    }

    fn refresh_setup_detection(&mut self) {
        self.setup_detection = detect_setup_environment(&self.adapters);
    }

    fn setup_items(&self) -> Vec<SetupAssistantItem> {
        let has_nvidia = self
            .adapters
            .iter()
            .any(|adapter| adapter_vendor(adapter) == GpuVendor::Nvidia);
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

        let pytorch_state = if self.pytorch_install_running || self.ai_training.pytorch_install_running {
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
            } else {
                SetupItemState::Attention
            },
            detail: if self.setup_detection.sensor_service_available {
                "benchscope_sensor_service is available beside the app or in the build tree."
                    .to_owned()
            } else {
                "Packaged builds should ship benchscope_sensor_service beside BenchScope. Source builds can produce it with cargo build.".to_owned()
            },
            action: None,
        });
        items.push(SetupAssistantItem {
            title: "Libre/OpenHardwareMonitor WMI",
            requirement: "Optional sensors",
            state: if self.setup_detection.hardware_monitor_wmi_available {
                SetupItemState::Ready
            } else {
                SetupItemState::Attention
            },
            detail: if self.setup_detection.hardware_monitor_wmi_available {
                "LibreHardwareMonitor or OpenHardwareMonitor WMI sensors are visible.".to_owned()
            } else {
                "For broader board, CPU, and GPU temperature coverage, run LibreHardwareMonitor or OpenHardwareMonitor with WMI enabled.".to_owned()
            },
            action: None,
        });

        items
    }

    fn ui_setup_assistant(&mut self, ctx: &egui::Context) {
        if !self.setup_assistant.visible {
            return;
        }

        let mut open = true;
        egui::Window::new("BenchScope setup")
            .collapsible(false)
            .resizable(true)
            .default_width(760.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Review required runtime pieces and optional accelerators for this device.");
                ui.add_space(8.0);
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        egui::Grid::new("setup_assistant_grid")
                            .num_columns(5)
                            .striped(true)
                            .spacing([12.0, 8.0])
                            .show(ui, |ui| {
                                ui.strong("Status");
                                ui.strong("Item");
                                ui.strong("Type");
                                ui.strong("Details");
                                ui.strong("Action");
                                ui.end_row();

                                for item in self.setup_items() {
                                    let (label, color) = setup_state_label_color(item.state);
                                    ui.colored_label(color, label);
                                    ui.label(item.title);
                                    ui.label(item.requirement);
                                    ui.label(item.detail);
                                    match item.action {
                                        Some(SetupAction::InstallManagedPytorchCuda) => {
                                            let enabled = !self.running
                                                && !self.pytorch_probe_running
                                                && !self.pytorch_install_running
                                                && !self.ai_training.pytorch_probe_running
                                                && !self.ai_training.pytorch_install_running;
                                            ui.add_enabled_ui(enabled, |ui| {
                                                if ui.button("Install").clicked() {
                                                    self.start_managed_pytorch_cuda_install();
                                                }
                                            });
                                        }
                                        Some(SetupAction::ProbeManagedPytorchCuda) => {
                                            let enabled = !self.running
                                                && !self.pytorch_probe_running
                                                && !self.pytorch_install_running
                                                && !self.ai_training.pytorch_probe_running
                                                && !self.ai_training.pytorch_install_running;
                                            ui.add_enabled_ui(enabled, |ui| {
                                                if ui.button("Probe").clicked() {
                                                    self.start_managed_pytorch_cuda_probe();
                                                }
                                            });
                                        }
                                        None => {
                                            ui.label("");
                                        }
                                    }
                                    ui.end_row();
                                }
                            });
                    });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Refresh").clicked() {
                        self.refresh_setup_detection();
                    }
                    if ui.button("Remind me later").clicked() {
                        self.setup_assistant.visible = false;
                        self.setup_assistant.dismissed_this_session = true;
                    }
                    if ui.button("Don't show again").clicked() {
                        if let Err(err) = persist_setup_dismissed_marker() {
                            self.log(format!("Could not persist setup dismissal: {err:#}"));
                        }
                        self.setup_assistant.visible = false;
                        self.setup_assistant.dismissed_this_session = true;
                        self.setup_assistant.dismissed_persisted = true;
                    }
                });
            });

        if !open {
            self.setup_assistant.visible = false;
            self.setup_assistant.dismissed_this_session = true;
        }
    }

    fn start_managed_pytorch_cuda_probe(&mut self) {
        if self.running
            || self.pytorch_probe_running
            || self.pytorch_install_running
            || self.ai_training.pytorch_probe_running
            || self.ai_training.pytorch_install_running
        {
            return;
        }

        if let Some(python) = self.setup_detection.managed_pytorch_python.clone() {
            self.pytorch_python = python.clone();
            self.ai_training.pytorch_python = python;
            self.start_pytorch_cuda_probe();
        }
    }

    fn start_managed_pytorch_cuda_install(&mut self) {
        if self.running
            || self.pytorch_probe_running
            || self.pytorch_install_running
            || self.ai_training.pytorch_probe_running
            || self.ai_training.pytorch_install_running
        {
            return;
        }

        let tx = self.tx.clone();
        self.pending_pytorch_install = false;
        self.pytorch_install_running = true;
        self.pytorch_probe_running = true;
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
}

fn setup_state_label_color(state: SetupItemState) -> (&'static str, egui::Color32) {
    match state {
        SetupItemState::Ready => ("Ready", egui::Color32::from_rgb(110, 210, 130)),
        SetupItemState::Attention => ("Needs setup", egui::Color32::YELLOW),
        SetupItemState::Installing => ("Working", egui::Color32::LIGHT_BLUE),
    }
}

fn setup_dismissed_marker_exists() -> bool {
    setup_dismissed_marker_path().is_some_and(|path| path.is_file())
}

fn persist_setup_dismissed_marker() -> Result<()> {
    let path = setup_dismissed_marker_path()
        .ok_or_else(|| anyhow!("LOCALAPPDATA is not available"))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create setup marker directory {}", parent.display())
        })?;
    }
    fs::write(&path, "dismissed\n")
        .with_context(|| format!("failed to write setup marker {}", path.display()))
}

fn setup_dismissed_marker_path() -> Option<PathBuf> {
    std::env::var("LOCALAPPDATA")
        .ok()
        .map(|local| PathBuf::from(local).join("BenchScope").join("setup-dismissed.txt"))
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
