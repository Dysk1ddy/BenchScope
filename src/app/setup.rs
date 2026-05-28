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

impl SetupAction {
    fn label(self) -> &'static str {
        match self {
            Self::InstallManagedPytorchCuda => "Install",
            Self::ProbeManagedPytorchCuda => "Probe",
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

    fn ui_setup_panel(&mut self, ui: &mut egui::Ui) {
        let items = self
            .setup_items()
            .into_iter()
            .filter(|item| item.state != SetupItemState::Ready)
            .collect::<Vec<_>>();
        if items.is_empty() {
            return;
        }

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
                });

                ui.add_space(8.0);
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
    }

    fn run_setup_action(&mut self, action: SetupAction) {
        match action {
            SetupAction::InstallManagedPytorchCuda => self.start_managed_pytorch_cuda_install(),
            SetupAction::ProbeManagedPytorchCuda => self.start_managed_pytorch_cuda_probe(),
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
