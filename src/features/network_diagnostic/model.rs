#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NetworkHealthStatus {
    Good,
    Caution,
    Critical,
    Unknown,
}

impl NetworkHealthStatus {
    fn label(self) -> &'static str {
        match self {
            NetworkHealthStatus::Good => "Good",
            NetworkHealthStatus::Caution => "Caution",
            NetworkHealthStatus::Critical => "Critical",
            NetworkHealthStatus::Unknown => "Unknown",
        }
    }

    fn color(self) -> egui::Color32 {
        match self {
            NetworkHealthStatus::Good => egui::Color32::GREEN,
            NetworkHealthStatus::Caution => egui::Color32::YELLOW,
            NetworkHealthStatus::Critical => egui::Color32::RED,
            NetworkHealthStatus::Unknown => egui::Color32::GRAY,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NetworkAdapterKind {
    Wifi,
    Ethernet,
    Virtual,
    Other,
    Unknown,
}

impl NetworkAdapterKind {
    fn label(self) -> &'static str {
        match self {
            NetworkAdapterKind::Wifi => "Wi-Fi",
            NetworkAdapterKind::Ethernet => "Ethernet",
            NetworkAdapterKind::Virtual => "Virtual",
            NetworkAdapterKind::Other => "Other",
            NetworkAdapterKind::Unknown => "Unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NetworkFindingSeverity {
    Info,
    Warning,
    Critical,
}

impl NetworkFindingSeverity {
    fn label(self) -> &'static str {
        match self {
            NetworkFindingSeverity::Info => "Info",
            NetworkFindingSeverity::Warning => "Warning",
            NetworkFindingSeverity::Critical => "Critical",
        }
    }

    fn color(self) -> egui::Color32 {
        match self {
            NetworkFindingSeverity::Info => egui::Color32::LIGHT_BLUE,
            NetworkFindingSeverity::Warning => egui::Color32::YELLOW,
            NetworkFindingSeverity::Critical => egui::Color32::RED,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NetworkProbeKind {
    Icmp,
    DnsLookup,
}

impl NetworkProbeKind {
    fn label(self) -> &'static str {
        match self {
            NetworkProbeKind::Icmp => "ICMP",
            NetworkProbeKind::DnsLookup => "DNS",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NetworkRunKind {
    QuickDiagnosis,
    SpeedTest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NetworkSpeedDirection {
    Download,
    Upload,
}

impl NetworkSpeedDirection {
    fn label(self) -> &'static str {
        match self {
            NetworkSpeedDirection::Download => "Download",
            NetworkSpeedDirection::Upload => "Upload",
        }
    }

    fn script_value(self) -> &'static str {
        match self {
            NetworkSpeedDirection::Download => "download",
            NetworkSpeedDirection::Upload => "upload",
        }
    }
}

#[derive(Clone, Debug)]
struct NetworkDriverInfo {
    provider: Option<String>,
    version: Option<String>,
    date: Option<String>,
    device_status: Option<String>,
}

#[derive(Clone, Debug)]
struct WifiSnapshot {
    ssid: Option<String>,
    signal_quality_percent: Option<u8>,
    phy_type: Option<String>,
    channel: Option<u32>,
    rx_link_speed_bps: Option<u64>,
    tx_link_speed_bps: Option<u64>,
}

#[derive(Clone, Debug)]
struct NetworkCounters {
    bytes_sent: Option<u64>,
    bytes_received: Option<u64>,
    packets_sent: Option<u64>,
    packets_received: Option<u64>,
    inbound_errors: Option<u64>,
    outbound_errors: Option<u64>,
    inbound_discards: Option<u64>,
    outbound_discards: Option<u64>,
}

#[derive(Clone, Debug)]
struct NetworkAdapterSnapshot {
    id: String,
    name: String,
    description: String,
    kind: NetworkAdapterKind,
    status: NetworkHealthStatus,
    connected: bool,
    is_physical: bool,
    link_speed_bps: Option<u64>,
    mac_address: Option<String>,
    ipv4_addresses: Vec<String>,
    ipv6_addresses: Vec<String>,
    gateways: Vec<String>,
    dns_servers: Vec<String>,
    driver: Option<NetworkDriverInfo>,
    wifi: Option<WifiSnapshot>,
    counters: Option<NetworkCounters>,
    provider_notes: Vec<String>,
}

impl NetworkAdapterSnapshot {
    fn menu_label(&self) -> String {
        let state = if self.connected { "up" } else { "down" };
        format!(
            "{} - {} - {} - {}",
            self.name,
            self.kind.label(),
            state,
            format_link_speed(self.link_speed_bps)
        )
    }
}

#[derive(Clone, Debug)]
struct NetworkProbeResult {
    target_label: String,
    target: String,
    probe_kind: NetworkProbeKind,
    sent: u32,
    received: u32,
    loss_percent: f32,
    min_latency_ms: Option<f64>,
    avg_latency_ms: Option<f64>,
    max_latency_ms: Option<f64>,
    jitter_ms: Option<f64>,
    notes: Vec<String>,
}

#[derive(Clone, Debug)]
struct NetworkSpeedSample {
    direction: NetworkSpeedDirection,
    bytes: u64,
    elapsed_ms: f64,
    mbps: f64,
}

#[derive(Clone, Debug)]
struct NetworkSpeedTestResult {
    download_mbps: Option<f64>,
    upload_mbps: Option<f64>,
    samples: Vec<NetworkSpeedSample>,
    notes: Vec<String>,
    elapsed_s: f64,
}

#[derive(Clone, Debug)]
struct NetworkFinding {
    severity: NetworkFindingSeverity,
    title: String,
    detail: String,
    recommended_action: Option<String>,
}

#[derive(Clone, Debug)]
struct NetworkProgress {
    step: String,
    progress: f32,
    elapsed_s: f64,
}

#[derive(Clone, Debug)]
struct WifiSignalSample {
    timestamp_s: u64,
    signal_percent: Option<u8>,
    link_speed_bps: Option<u64>,
    gateway_latency_ms: Option<f64>,
}

#[derive(Clone, Debug)]
struct NetworkMonitorSample {
    snapshot: NetworkAdapterSnapshot,
    signal: WifiSignalSample,
    gateway_probe: Option<NetworkProbeResult>,
    findings: Vec<NetworkFinding>,
}

#[derive(Clone, Debug)]
struct NetworkDiagnosisResult {
    snapshot: NetworkAdapterSnapshot,
    probes: Vec<NetworkProbeResult>,
    findings: Vec<NetworkFinding>,
    status: NetworkHealthStatus,
}

enum NetworkWorkerEvent {
    Progress(NetworkProgress),
    ProbeCompleted(NetworkProbeResult),
    SpeedSampleCompleted(NetworkSpeedSample),
    DiagnosisDone(Result<NetworkDiagnosisResult, String>),
    SpeedTestDone(Result<NetworkSpeedTestResult, String>),
    MonitorSample(NetworkMonitorSample),
    MonitorStopped(Result<(), String>),
    Log(String),
}

struct NetworkDiagnosticState {
    adapters: Vec<NetworkAdapterSnapshot>,
    selected_adapter: usize,
    include_virtual: bool,
    probe_results: Vec<NetworkProbeResult>,
    speed_samples: Vec<NetworkSpeedSample>,
    speed_result: Option<NetworkSpeedTestResult>,
    findings: Vec<NetworkFinding>,
    signal_history: VecDeque<WifiSignalSample>,
    log: Vec<String>,
    status: String,
    current_step: String,
    progress: f32,
    rx: Receiver<NetworkWorkerEvent>,
    tx: Sender<NetworkWorkerEvent>,
    cancel: Option<Arc<AtomicBool>>,
    running: bool,
    running_kind: Option<NetworkRunKind>,
    monitoring: bool,
    last_report_path: Option<PathBuf>,
}

impl NetworkDiagnosticState {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let mut log = vec!["Network diagnostic ready".to_owned()];
        let adapters = match detect_network_adapters() {
            Ok(adapters) => adapters,
            Err(err) => {
                log.push(format!("Adapter detection unavailable: {err:#}"));
                Vec::new()
            }
        };
        let selected_adapter = preferred_network_adapter_index(&adapters).unwrap_or(0);
        let status = if adapters.is_empty() {
            "No network adapters detected".to_owned()
        } else {
            format!("Detected {} network adapter(s)", adapters.len())
        };
        Self {
            adapters,
            selected_adapter,
            include_virtual: false,
            probe_results: Vec::new(),
            speed_samples: Vec::new(),
            speed_result: None,
            findings: Vec::new(),
            signal_history: VecDeque::new(),
            log,
            status,
            current_step: "Ready".to_owned(),
            progress: 0.0,
            rx,
            tx,
            cancel: None,
            running: false,
            running_kind: None,
            monitoring: false,
            last_report_path: None,
        }
    }

    fn log(&mut self, message: impl Into<String>) {
        self.log.push(message.into());
    }

    fn selected_adapter(&self) -> Option<&NetworkAdapterSnapshot> {
        self.adapters.get(self.selected_adapter)
    }

    fn selected_adapter_id(&self) -> Option<String> {
        self.selected_adapter().map(|adapter| adapter.id.clone())
    }

    fn selected_adapter_label(&self) -> String {
        self.selected_adapter()
            .map(NetworkAdapterSnapshot::menu_label)
            .unwrap_or_else(|| "No adapters detected".to_owned())
    }

    fn visible_adapter_indices(&self) -> Vec<usize> {
        self.adapters
            .iter()
            .enumerate()
            .filter(|(_, adapter)| {
                self.include_virtual
                    || adapter.is_physical
                    || adapter.kind != NetworkAdapterKind::Virtual
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn replace_adapters(&mut self, adapters: Vec<NetworkAdapterSnapshot>) {
        let selected_id = self.selected_adapter_id();
        self.adapters = adapters;
        self.selected_adapter = selected_id
            .and_then(|id| self.adapters.iter().position(|adapter| adapter.id == id))
            .or_else(|| preferred_network_adapter_index(&self.adapters))
            .unwrap_or(0);
    }

    fn update_adapter_snapshot(&mut self, snapshot: NetworkAdapterSnapshot) {
        self.selected_adapter = upsert_network_adapter_snapshot(&mut self.adapters, snapshot);
    }

    fn refresh_adapters(&mut self) {
        match detect_network_adapters() {
            Ok(adapters) => {
                let count = adapters.len();
                self.replace_adapters(adapters);
                self.status = format!("Detected {count} network adapter(s)");
                self.log(self.status.clone());
            }
            Err(err) => {
                self.status = format!("Could not refresh adapters: {err:#}");
                self.log(self.status.clone());
            }
        }
    }

    fn start_quick_diagnosis(&mut self) {
        if self.running || self.monitoring {
            return;
        }
        let Some(adapter_id) = self.selected_adapter_id() else {
            self.status = "Select a network adapter first".to_owned();
            self.log(self.status.clone());
            return;
        };
        self.probe_results.clear();
        self.findings.clear();
        self.progress = 0.0;
        self.current_step = "Starting quick diagnosis".to_owned();
        self.status = "Running network diagnosis...".to_owned();
        self.log(self.status.clone());

        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.cancel = Some(cancel);
        self.running = true;
        self.running_kind = Some(NetworkRunKind::QuickDiagnosis);
        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                run_network_quick_diagnosis(adapter_id, worker_cancel, tx.clone())
            }))
            .map_err(|panic| format!("Network diagnosis panicked: {}", panic_message(&*panic)))
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(NetworkWorkerEvent::DiagnosisDone(result));
        });
    }

    fn start_speed_test(&mut self) {
        if self.running || self.monitoring {
            return;
        }
        self.speed_samples.clear();
        self.speed_result = None;
        self.progress = 0.0;
        self.current_step = "Starting internet speed test".to_owned();
        self.status = "Running internet speed test...".to_owned();
        self.log(self.status.clone());
        if let Some(adapter) = self.selected_adapter() {
            self.log(format!(
                "Selected adapter context: {}; speed test uses system internet routing",
                adapter.menu_label()
            ));
        }

        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.cancel = Some(cancel);
        self.running = true;
        self.running_kind = Some(NetworkRunKind::SpeedTest);
        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                run_network_speed_test(worker_cancel, tx.clone())
            }))
            .map_err(|panic| format!("Internet speed test panicked: {}", panic_message(&*panic)))
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(NetworkWorkerEvent::SpeedTestDone(result));
        });
    }

    fn start_monitor(&mut self) {
        if self.running || self.monitoring {
            return;
        }
        let Some(adapter_id) = self.selected_adapter_id() else {
            self.status = "Select a network adapter first".to_owned();
            self.log(self.status.clone());
            return;
        };
        self.progress = 0.0;
        self.current_step = "Monitoring adapter".to_owned();
        self.status = "Continuous network monitor started".to_owned();
        self.log(self.status.clone());

        let tx = self.tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.cancel = Some(cancel);
        self.monitoring = true;
        self.running_kind = None;
        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                run_network_monitor(adapter_id, worker_cancel, tx.clone())
            }))
            .map_err(|panic| format!("Network monitor panicked: {}", panic_message(&*panic)))
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(NetworkWorkerEvent::MonitorStopped(result));
        });
    }

    fn stop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
            self.status = match self.running_kind {
                Some(NetworkRunKind::SpeedTest) => "Cancel requested for internet speed test",
                Some(NetworkRunKind::QuickDiagnosis) => "Cancel requested for network diagnosis",
                None => "Stop requested for network monitor",
            }
            .to_owned();
            self.current_step = self.status.clone();
            self.log(self.status.clone());
        }
    }

    fn export_report(&mut self) {
        match write_network_diagnostic_report(self) {
            Ok(path) => {
                self.status = format!("Network report exported: {}", path.display());
                self.log(self.status.clone());
                self.last_report_path = Some(path);
            }
            Err(err) => {
                self.status = format!("Could not export network report: {err:#}");
                self.log(self.status.clone());
            }
        }
    }

    fn push_signal_sample(&mut self, sample: WifiSignalSample) {
        self.signal_history.push_back(sample);
        while self.signal_history.len() > NETWORK_SIGNAL_HISTORY_LIMIT {
            self.signal_history.pop_front();
        }
    }

    fn poll_worker_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                NetworkWorkerEvent::Progress(progress) => {
                    self.current_step = progress.step;
                    self.progress = progress.progress;
                    self.status = format!(
                        "{} - elapsed {}",
                        self.current_step,
                        format_elapsed(progress.elapsed_s)
                    );
                }
                NetworkWorkerEvent::ProbeCompleted(result) => {
                    self.log(format!(
                        "{} probe complete: {} loss, avg {}",
                        result.target_label,
                        format_loss_percent(result.loss_percent),
                        format_optional_latency(result.avg_latency_ms)
                    ));
                    self.probe_results.push(result);
                }
                NetworkWorkerEvent::SpeedSampleCompleted(sample) => {
                    self.log(format!(
                        "{} speed sample: {} in {}, {}",
                        sample.direction.label(),
                        format_network_payload_size(sample.bytes),
                        format_elapsed(sample.elapsed_ms / 1000.0),
                        format_network_speed_mbps(Some(sample.mbps))
                    ));
                    self.speed_samples.push(sample);
                }
                NetworkWorkerEvent::DiagnosisDone(result) => {
                    self.running = false;
                    self.running_kind = None;
                    self.cancel = None;
                    match result {
                        Ok(result) => {
                            self.update_adapter_snapshot(result.snapshot.clone());
                            self.probe_results = result.probes;
                            self.findings = result.findings;
                            self.progress = 1.0;
                            self.current_step = "Diagnosis complete".to_owned();
                            self.status =
                                format!("Network diagnosis complete: {}", result.status.label());
                            self.log(self.status.clone());
                        }
                        Err(err) => {
                            self.progress = 0.0;
                            self.current_step = "Diagnosis stopped".to_owned();
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
                NetworkWorkerEvent::SpeedTestDone(result) => {
                    self.running = false;
                    self.running_kind = None;
                    self.cancel = None;
                    match result {
                        Ok(result) => {
                            self.speed_samples = result.samples.clone();
                            self.progress = 1.0;
                            self.current_step = "Speed test complete".to_owned();
                            self.status = format!(
                                "Internet speed test complete: down {}, up {}",
                                format_network_speed_mbps(result.download_mbps),
                                format_network_speed_mbps(result.upload_mbps)
                            );
                            self.log(self.status.clone());
                            for note in &result.notes {
                                self.log(format!("Speed test note: {note}"));
                            }
                            self.speed_result = Some(result);
                        }
                        Err(err) => {
                            self.progress = 0.0;
                            self.current_step = "Speed test stopped".to_owned();
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
                NetworkWorkerEvent::MonitorSample(sample) => {
                    self.update_adapter_snapshot(sample.snapshot.clone());
                    self.push_signal_sample(sample.signal);
                    if let Some(probe) = sample.gateway_probe {
                        self.probe_results.push(probe);
                        while self.probe_results.len() > 20 {
                            self.probe_results.remove(0);
                        }
                    }
                    self.findings = sample.findings;
                    self.progress = 1.0;
                    self.status = "Continuous monitor sample updated".to_owned();
                }
                NetworkWorkerEvent::MonitorStopped(result) => {
                    self.monitoring = false;
                    self.cancel = None;
                    match result {
                        Ok(()) => {
                            self.status = "Continuous network monitor stopped".to_owned();
                            self.log(self.status.clone());
                        }
                        Err(err) => {
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
                NetworkWorkerEvent::Log(message) => self.log(message),
            }
        }
    }
}

fn upsert_network_adapter_snapshot(
    adapters: &mut Vec<NetworkAdapterSnapshot>,
    snapshot: NetworkAdapterSnapshot,
) -> usize {
    if let Some(index) = adapters
        .iter()
        .position(|adapter| adapter.id == snapshot.id)
    {
        adapters[index] = snapshot;
        index
    } else {
        adapters.push(snapshot);
        adapters.len() - 1
    }
}
