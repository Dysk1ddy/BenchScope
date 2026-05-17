#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeviceInfoPage {
    Overview,
    CpuMemory,
    Storage,
    Graphics,
    Drivers,
    Firmware,
    ProviderPlan,
}

impl DeviceInfoPage {
    const ALL: [DeviceInfoPage; 7] = [
        DeviceInfoPage::Overview,
        DeviceInfoPage::CpuMemory,
        DeviceInfoPage::Storage,
        DeviceInfoPage::Graphics,
        DeviceInfoPage::Drivers,
        DeviceInfoPage::Firmware,
        DeviceInfoPage::ProviderPlan,
    ];

    fn label(self) -> &'static str {
        match self {
            DeviceInfoPage::Overview => "Overview",
            DeviceInfoPage::CpuMemory => "CPU / RAM",
            DeviceInfoPage::Storage => "Storage",
            DeviceInfoPage::Graphics => "Graphics",
            DeviceInfoPage::Drivers => "Drivers",
            DeviceInfoPage::Firmware => "Firmware",
            DeviceInfoPage::ProviderPlan => "Provider Coverage",
        }
    }
}

#[derive(Clone, Debug)]
struct DeviceSystemInfo {
    manufacturer: Option<String>,
    model: Option<String>,
    family: Option<String>,
    sku: Option<String>,
    system_type: Option<String>,
    chassis: Vec<String>,
    total_physical_memory_bytes: Option<u64>,
    physical_processor_count: Option<u32>,
    logical_processor_count: Option<u32>,
    domain: Option<String>,
    workgroup: Option<String>,
    hypervisor_present: Option<bool>,
    user_name: Option<String>,
}

#[derive(Clone, Debug)]
struct DeviceOsInfo {
    caption: Option<String>,
    version: Option<String>,
    build_number: Option<String>,
    architecture: Option<String>,
    install_date: Option<String>,
    last_boot_time: Option<String>,
}

#[derive(Clone, Debug)]
struct DeviceBiosInfo {
    manufacturer: Option<String>,
    smbios_version: Option<String>,
    version: Option<String>,
    release_date: Option<String>,
    serial_number: Option<String>,
}

#[derive(Clone, Debug)]
struct DeviceBaseboardInfo {
    manufacturer: Option<String>,
    product: Option<String>,
    version: Option<String>,
    serial_number: Option<String>,
}

#[derive(Clone, Debug)]
struct DeviceCpuDetails {
    name: String,
    manufacturer: Option<String>,
    description: Option<String>,
    socket: Option<String>,
    processor_id: Option<String>,
    architecture: Option<String>,
    family: Option<String>,
    model: Option<String>,
    stepping: Option<String>,
    cores: Option<u32>,
    logical_processors: Option<u32>,
    max_clock_mhz: Option<u32>,
    current_clock_mhz: Option<u32>,
    l2_cache_kb: Option<u32>,
    l3_cache_kb: Option<u32>,
    virtualization_firmware_enabled: Option<bool>,
    second_level_address_translation: Option<bool>,
    vm_monitor_extensions: Option<bool>,
}

#[derive(Clone, Debug)]
struct DeviceMemoryModuleInfo {
    manufacturer: Option<String>,
    part_number: Option<String>,
    serial_number: Option<String>,
    bank_label: Option<String>,
    device_locator: Option<String>,
    capacity_bytes: Option<u64>,
    speed_mhz: Option<u32>,
    configured_clock_speed_mhz: Option<u32>,
    form_factor: Option<String>,
    memory_type: Option<String>,
    smbios_memory_type: Option<String>,
    type_detail: Option<String>,
    data_width_bits: Option<u32>,
    total_width_bits: Option<u32>,
}

#[derive(Clone, Debug)]
struct DeviceDiskInfo {
    index: Option<u32>,
    device_id: Option<String>,
    model: String,
    serial_number: Option<String>,
    firmware: Option<String>,
    interface_type: Option<String>,
    media_type: Option<String>,
    bus_type: Option<String>,
    size_bytes: Option<u64>,
    partitions: Option<u32>,
    status: Option<String>,
    health_status: Option<String>,
    operational_status: Option<String>,
}

#[derive(Clone, Debug)]
struct DeviceVolumeInfo {
    drive_letter: Option<String>,
    label: Option<String>,
    file_system: Option<String>,
    drive_type: Option<String>,
    size_bytes: Option<u64>,
    free_bytes: Option<u64>,
    health_status: Option<String>,
    operational_status: Option<String>,
}

#[derive(Clone, Debug)]
struct DeviceGpuDetails {
    name: String,
    pnp_device_id: Option<String>,
    adapter_compatibility: Option<String>,
    video_processor: Option<String>,
    driver_provider: Option<String>,
    driver_version: Option<String>,
    driver_date: Option<String>,
    inf_name: Option<String>,
    adapter_ram_bytes: Option<u64>,
    resolution: Option<String>,
    refresh_rate_hz: Option<u32>,
    status: Option<String>,
}

#[derive(Clone, Debug)]
struct DeviceNetworkAdapterInfo {
    name: String,
    description: Option<String>,
    physical: Option<bool>,
    mac_address: Option<String>,
    speed_bps: Option<u64>,
    net_enabled: Option<bool>,
    driver_provider: Option<String>,
    driver_version: Option<String>,
    driver_date: Option<String>,
    status: Option<String>,
}

#[derive(Clone, Debug)]
struct DeviceMonitorInfo {
    name: String,
    manufacturer: Option<String>,
    monitor_type: Option<String>,
    resolution: Option<String>,
    pnp_device_id: Option<String>,
    status: Option<String>,
}

#[derive(Clone, Debug)]
struct DeviceDriverRecord {
    device_class: Option<String>,
    device_name: String,
    manufacturer: Option<String>,
    provider: Option<String>,
    version: Option<String>,
    date: Option<String>,
    signer: Option<String>,
    inf_name: Option<String>,
    device_id: Option<String>,
    is_signed: Option<bool>,
}

#[derive(Clone, Debug)]
struct DeviceInfoProviderPlan {
    area: &'static str,
    current_provider: &'static str,
    status: &'static str,
    extra_driver_needed: &'static str,
    notes: &'static str,
}

#[derive(Clone, Debug)]
struct DeviceInfoSnapshot {
    captured_at: SystemTime,
    system: Option<DeviceSystemInfo>,
    os: Option<DeviceOsInfo>,
    bios: Option<DeviceBiosInfo>,
    baseboard: Option<DeviceBaseboardInfo>,
    cpus: Vec<DeviceCpuDetails>,
    memory_modules: Vec<DeviceMemoryModuleInfo>,
    disks: Vec<DeviceDiskInfo>,
    volumes: Vec<DeviceVolumeInfo>,
    gpus: Vec<DeviceGpuDetails>,
    network_adapters: Vec<DeviceNetworkAdapterInfo>,
    monitors: Vec<DeviceMonitorInfo>,
    drivers: Vec<DeviceDriverRecord>,
    wgpu_adapters: Vec<AdapterInfo>,
    provider_notes: Vec<String>,
}

impl DeviceInfoSnapshot {
    fn empty(note: impl Into<String>) -> Self {
        Self {
            captured_at: SystemTime::now(),
            system: None,
            os: None,
            bios: None,
            baseboard: None,
            cpus: Vec::new(),
            memory_modules: Vec::new(),
            disks: Vec::new(),
            volumes: Vec::new(),
            gpus: Vec::new(),
            network_adapters: Vec::new(),
            monitors: Vec::new(),
            drivers: Vec::new(),
            wgpu_adapters: Vec::new(),
            provider_notes: vec![note.into()],
        }
    }

    fn total_ram_bytes(&self) -> Option<u64> {
        self.system
            .as_ref()
            .and_then(|system| system.total_physical_memory_bytes)
            .or_else(|| {
                let total = self
                    .memory_modules
                    .iter()
                    .filter_map(|module| module.capacity_bytes)
                    .sum::<u64>();
                (total > 0).then_some(total)
            })
    }

    fn cpu_core_count(&self) -> Option<u32> {
        let total = self.cpus.iter().filter_map(|cpu| cpu.cores).sum::<u32>();
        (total > 0).then_some(total)
    }

    fn cpu_logical_processor_count(&self) -> Option<u32> {
        let total = self
            .cpus
            .iter()
            .filter_map(|cpu| cpu.logical_processors)
            .sum::<u32>();
        (total > 0)
            .then_some(total)
            .or_else(|| {
                self.system
                    .as_ref()
                    .and_then(|system| system.logical_processor_count)
            })
    }

    fn total_storage_bytes(&self) -> Option<u64> {
        let total = self
            .disks
            .iter()
            .filter_map(|disk| disk.size_bytes)
            .sum::<u64>();
        (total > 0).then_some(total)
    }
}

#[derive(Debug)]
enum DeviceInfoEvent {
    Snapshot(Result<DeviceInfoSnapshot, String>),
}

struct DeviceInfoState {
    selected_page: DeviceInfoPage,
    snapshot: Option<DeviceInfoSnapshot>,
    log: Vec<String>,
    status: String,
    rx: Receiver<DeviceInfoEvent>,
    tx: Sender<DeviceInfoEvent>,
    running: bool,
    last_report_path: Option<PathBuf>,
    show_all_driver_classes: bool,
}

impl DeviceInfoState {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            selected_page: DeviceInfoPage::Overview,
            snapshot: None,
            log: vec!["Device information viewer ready".to_owned()],
            status: "Ready".to_owned(),
            rx,
            tx,
            running: false,
            last_report_path: None,
            show_all_driver_classes: false,
        }
    }

    fn log(&mut self, message: impl Into<String>) {
        self.log.push(message.into());
    }

    fn start_refresh(&mut self, wgpu_adapters: Vec<AdapterInfo>) {
        if self.running {
            return;
        }
        self.running = true;
        self.status = "Refreshing hardware inventory...".to_owned();
        self.log(self.status.clone());

        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                collect_device_info_snapshot(wgpu_adapters)
            }))
            .map_err(|panic| {
                format!(
                    "Device information refresh panicked: {}",
                    panic_message(&*panic)
                )
            })
            .and_then(|result| result.map_err(|err| format!("{err:#}")));
            let _ = tx.send(DeviceInfoEvent::Snapshot(result));
        });
    }

    fn export_report(&mut self) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            self.status = "Refresh device information before exporting a report".to_owned();
            self.log(self.status.clone());
            return;
        };
        match write_device_info_report(snapshot) {
            Ok(path) => {
                self.status = format!("Device information report exported: {}", path.display());
                self.last_report_path = Some(path.clone());
                self.log(self.status.clone());
            }
            Err(err) => {
                self.status = format!("Could not export device information report: {err:#}");
                self.log(self.status.clone());
            }
        }
    }

    fn poll_worker_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                DeviceInfoEvent::Snapshot(result) => {
                    self.running = false;
                    match result {
                        Ok(snapshot) => {
                            self.status = format!(
                                "Hardware inventory refreshed: {} driver record(s)",
                                snapshot.drivers.len()
                            );
                            self.log(self.status.clone());
                            for note in &snapshot.provider_notes {
                                self.log(format!("Provider note: {note}"));
                            }
                            self.snapshot = Some(snapshot);
                        }
                        Err(err) => {
                            self.status = err.clone();
                            self.log(err);
                        }
                    }
                }
            }
        }
    }
}
