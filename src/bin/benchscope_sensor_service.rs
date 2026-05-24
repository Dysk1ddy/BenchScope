#[path = "../sensor_driver_client.rs"]
mod sensor_driver_client;

use std::{
    io::{self, Write},
    path::PathBuf,
    process::{Child, Command},
    sync::mpsc::{self, TryRecvError},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use windows::Win32::{
    Foundation::FILETIME,
    System::{
        SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
        Threading::GetSystemTimes,
    },
};

use sensor_driver_client::{
    BENCHSCOPE_SENSOR_ADVANCED_HAS_ENERGY, BENCHSCOPE_SENSOR_ADVANCED_HAS_TEMPERATURE,
    BENCHSCOPE_SENSOR_ADVANCED_HAS_THERMAL_LIMIT, BENCHSCOPE_SENSOR_ADVANCED_THERMAL_THROTTLED,
    BENCHSCOPE_SENSOR_ADVANCED_USER_MODE_PROVIDER, BENCHSCOPE_SENSOR_READING_HAS_TEMPERATURE,
    BENCHSCOPE_SENSOR_READING_HAS_UTILIZATION, BenchScopeSensorAdvancedTelemetry,
    BenchScopeSensorReading, DeviceHandle, kind_from_driver, kind_json_key, status_from_driver,
    status_json_value, wide_to_string,
};

#[cfg(windows)]
const CREATE_NO_WINDOW_RAW: u32 = 0x0800_0000;
const DEFAULT_INTERVAL_MS: u64 = 1_000;
const FAST_PROVIDER_TTL: Duration = Duration::from_millis(450);
const GPU_TEMP_TTL: Duration = Duration::from_secs(5);
const DRIVE_TEMP_TTL: Duration = Duration::from_secs(15);
const STALE_PROVIDER_AFTER: Duration = Duration::from_secs(45);
const FAST_COMMAND_TIMEOUT: Duration = Duration::from_millis(1_800);
const SLOW_COMMAND_TIMEOUT: Duration = Duration::from_millis(4_500);
const ASYNC_PROVIDER_ABANDON_AFTER: Duration = Duration::from_secs(6);
const CPU_INITIAL_SAMPLE_WINDOW: Duration = Duration::from_millis(120);
const BENCHSCOPE_SENSOR_ADVANCED_HAS_POWER: u32 = 0x0000_0010;
const BENCHSCOPE_SENSOR_ADVANCED_HAS_VOLTAGE: u32 = 0x0000_0040;
#[cfg(windows)]
const DRIVER_SERVICE_NAME: &str = "BenchScopeSensorDriver";
#[cfg(windows)]
const DRIVER_START_RETRY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BridgeMetricKind {
    Temperature,
    Utilization,
    MemoryUsage,
    Voltage,
    Power,
    Clock,
}

impl BridgeMetricKind {
    fn json_key(self) -> &'static str {
        match self {
            BridgeMetricKind::Temperature => "temperature",
            BridgeMetricKind::Utilization => "utilization",
            BridgeMetricKind::MemoryUsage => "memoryUsage",
            BridgeMetricKind::Voltage => "voltage",
            BridgeMetricKind::Power => "power",
            BridgeMetricKind::Clock => "clock",
        }
    }

    fn default_label(self) -> &'static str {
        match self {
            BridgeMetricKind::Temperature => "Temperature",
            BridgeMetricKind::Utilization => "Utilization",
            BridgeMetricKind::MemoryUsage => "VRAM Used",
            BridgeMetricKind::Voltage => "Voltage",
            BridgeMetricKind::Power => "Power",
            BridgeMetricKind::Clock => "Clock",
        }
    }
}

#[derive(Clone, Debug)]
struct BridgeMetric {
    kind: BridgeMetricKind,
    label: String,
    value: Option<f32>,
    min: Option<f32>,
    max: Option<f32>,
}

impl BridgeMetric {
    fn new(kind: BridgeMetricKind, label: &str, value: Option<f32>) -> Self {
        Self {
            kind,
            label: label.to_owned(),
            value,
            min: None,
            max: None,
        }
    }

    fn with_range(mut self, min: Option<f32>, max: Option<f32>) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    fn has_value(&self) -> bool {
        self.value.is_some()
    }
}

#[derive(Clone, Debug)]
struct BridgeReading {
    label: String,
    temperature_c: Option<f32>,
    utilization_percent: Option<f32>,
    metrics: Vec<BridgeMetric>,
    provider: String,
    status: String,
    message: Option<String>,
}

impl BridgeReading {
    fn unavailable(label: &str, provider: &str, status: &str, message: Option<String>) -> Self {
        Self {
            label: label.to_owned(),
            temperature_c: None,
            utilization_percent: None,
            metrics: Vec::new(),
            provider: provider.to_owned(),
            status: status.to_owned(),
            message,
        }
    }

    fn from_driver(reading: &BenchScopeSensorReading) -> Self {
        let temperature_c = (reading.flags & BENCHSCOPE_SENSOR_READING_HAS_TEMPERATURE != 0)
            .then_some(reading.temperature_milli_c as f32 / 1000.0);
        let utilization_percent = (reading.flags & BENCHSCOPE_SENSOR_READING_HAS_UTILIZATION != 0)
            .then_some(reading.utilization_milli_percent as f32 / 1000.0);
        let label = wide_to_string(&reading.label);
        let mut bridge = Self {
            label: label.clone(),
            temperature_c,
            utilization_percent,
            metrics: Vec::new(),
            provider: wide_to_string(&reading.provider),
            status: status_json_value(status_from_driver(reading.status)).to_owned(),
            message: None,
        };
        if let Some(value) = temperature_c {
            bridge.upsert_metric(BridgeMetric::new(
                BridgeMetricKind::Temperature,
                &label,
                Some(value),
            ));
        }
        if let Some(value) = utilization_percent {
            bridge.upsert_metric(BridgeMetric::new(
                BridgeMetricKind::Utilization,
                BridgeMetricKind::Utilization.default_label(),
                Some(value),
            ));
        }
        bridge
    }

    fn from_metrics(label: &str, provider: &str, mut metrics: Vec<BridgeMetric>) -> Self {
        metrics.retain(BridgeMetric::has_value);
        let temperature_c = first_bridge_metric_value(&metrics, BridgeMetricKind::Temperature);
        let utilization_percent =
            first_bridge_metric_value(&metrics, BridgeMetricKind::Utilization).map(clamp_percent);
        Self {
            label: label.to_owned(),
            temperature_c,
            utilization_percent,
            metrics,
            provider: provider.to_owned(),
            status: "ok".to_owned(),
            message: None,
        }
    }

    fn from_temperature(label: &str, value: f32, provider: &str) -> Self {
        let mut reading = Self {
            label: label.to_owned(),
            temperature_c: Some(round_tenth(value)),
            utilization_percent: None,
            metrics: Vec::new(),
            provider: provider.to_owned(),
            status: "ok".to_owned(),
            message: None,
        };
        reading.upsert_metric(BridgeMetric::new(
            BridgeMetricKind::Temperature,
            label,
            reading.temperature_c,
        ));
        reading
    }

    fn from_utilization(label: &str, value: f32, provider: &str, partial_message: &str) -> Self {
        let mut reading = Self {
            label: label.to_owned(),
            temperature_c: None,
            utilization_percent: Some(clamp_percent(value)),
            metrics: Vec::new(),
            provider: provider.to_owned(),
            status: "partial".to_owned(),
            message: Some(partial_message.to_owned()),
        };
        reading.upsert_metric(BridgeMetric::new(
            BridgeMetricKind::Utilization,
            BridgeMetricKind::Utilization.default_label(),
            reading.utilization_percent,
        ));
        reading
    }

    fn from_memory_utilization(value: f32) -> Self {
        let mut reading = Self {
            label: "System RAM".to_owned(),
            temperature_c: None,
            utilization_percent: Some(clamp_percent(value)),
            metrics: Vec::new(),
            provider: "Windows memory status".to_owned(),
            status: "ok".to_owned(),
            message: None,
        };
        reading.upsert_metric(BridgeMetric::new(
            BridgeMetricKind::Utilization,
            BridgeMetricKind::Utilization.default_label(),
            reading.utilization_percent,
        ));
        reading
    }

    fn merge_safe_provider(&mut self, safe: BridgeReading) {
        let mut changed = false;
        let safe_has_metrics = safe.metrics.iter().any(BridgeMetric::has_value);
        if self.temperature_c.is_none() && safe.temperature_c.is_some() {
            self.temperature_c = safe.temperature_c;
            if self.label.is_empty()
                || self.label == "CPU"
                || self.label == "GPU"
                || self.label == "SSD"
            {
                self.label = safe.label.clone();
            }
            changed = true;
        }
        if self.utilization_percent.is_none() && safe.utilization_percent.is_some() {
            self.utilization_percent = safe.utilization_percent;
            changed = true;
        }
        for metric in &safe.metrics {
            self.upsert_metric(metric.clone());
        }
        changed |= safe_has_metrics;
        if changed {
            self.merge_provider(&safe.provider);
            self.status = if safe.status == "ok" || self.temperature_c.is_some() {
                "ok".to_owned()
            } else {
                "partial".to_owned()
            };
            if self.status == "partial" {
                self.message = safe.message;
            } else {
                self.message = None;
            }
        }
    }

    fn merge_provider(&mut self, provider: &str) {
        if provider.is_empty() || self.provider.contains(provider) {
            return;
        }
        if self.provider.is_empty() || self.provider == "BenchScope sensor driver prototype" {
            self.provider = provider.to_owned();
        } else {
            self.provider = format!("{} + {}", self.provider, provider);
        }
    }

    fn upsert_metric(&mut self, metric: BridgeMetric) {
        if !metric.has_value() {
            return;
        }
        if let Some(existing) = self
            .metrics
            .iter_mut()
            .find(|existing| bridge_metric_slots_match(existing, &metric))
        {
            if existing.value.is_none() {
                existing.value = metric.value;
            }
            if existing.min.is_none() {
                existing.min = metric.min;
            }
            if existing.max.is_none() {
                existing.max = metric.max;
            }
            if bridge_metric_label_is_generic(&existing.label)
                && !bridge_metric_label_is_generic(&metric.label)
            {
                existing.label = metric.label;
            }
        } else {
            self.metrics.push(metric);
        }
    }
}

fn first_bridge_metric_value(metrics: &[BridgeMetric], kind: BridgeMetricKind) -> Option<f32> {
    metrics
        .iter()
        .find(|metric| metric.kind == kind)
        .and_then(|metric| metric.value)
}

fn bridge_metric_slots_match(left: &BridgeMetric, right: &BridgeMetric) -> bool {
    if left.kind != right.kind {
        return false;
    }
    left.label.eq_ignore_ascii_case(&right.label)
        || (matches!(
            left.kind,
            BridgeMetricKind::Temperature
                | BridgeMetricKind::Utilization
                | BridgeMetricKind::MemoryUsage
        ) && (bridge_metric_label_is_generic(&left.label)
            || bridge_metric_label_is_generic(&right.label)))
}

fn bridge_metric_label_is_generic(label: &str) -> bool {
    let label = label.trim().to_ascii_lowercase();
    matches!(
        label.as_str(),
        "" | "cpu"
            | "gpu"
            | "vram"
            | "gpu memory"
            | "ssd"
            | "ram"
            | "temperature"
            | "temperatures"
            | "utilization"
            | "memory usage"
            | "vram used"
            | "used"
            | "load"
            | "loads"
            | "voltage"
            | "voltages"
            | "power"
            | "powers"
            | "clock"
            | "clocks"
    )
}

#[derive(Clone, Debug)]
struct CachedReading {
    value: Option<BridgeReading>,
    updated_at: Option<Instant>,
    ttl: Duration,
}

impl CachedReading {
    fn new(ttl: Duration) -> Self {
        Self {
            value: None,
            updated_at: None,
            ttl,
        }
    }

    fn resolve_query(
        &mut self,
        now: Instant,
        next: Option<Option<BridgeReading>>,
    ) -> Option<BridgeReading> {
        if self.is_fresh(now) {
            return self.value.clone();
        }

        if let Some(Some(value)) = next {
            self.value = Some(value);
            self.updated_at = Some(now);
            return self.value.clone();
        }

        if self.is_usable_stale(now) {
            self.value.clone()
        } else {
            None
        }
    }

    fn is_fresh(&self, now: Instant) -> bool {
        self.updated_at
            .is_some_and(|updated_at| now.duration_since(updated_at) <= self.ttl)
    }

    fn is_usable_stale(&self, now: Instant) -> bool {
        self.value.is_some()
            && self
                .updated_at
                .is_some_and(|updated_at| now.duration_since(updated_at) <= STALE_PROVIDER_AFTER)
    }
}

#[derive(Debug)]
struct AsyncCachedReading {
    value: Option<BridgeReading>,
    updated_at: Option<Instant>,
    ttl: Duration,
    receiver: Option<mpsc::Receiver<Option<BridgeReading>>>,
    query_started_at: Option<Instant>,
}

impl AsyncCachedReading {
    fn new(ttl: Duration) -> Self {
        Self {
            value: None,
            updated_at: None,
            ttl,
            receiver: None,
            query_started_at: None,
        }
    }

    fn resolve_query(
        &mut self,
        now: Instant,
        query: fn() -> Option<BridgeReading>,
    ) -> Option<BridgeReading> {
        self.collect_finished(now);

        if !self.is_fresh(now) && self.receiver.is_none() {
            let (sender, receiver) = mpsc::channel();
            self.receiver = Some(receiver);
            self.query_started_at = Some(now);
            thread::spawn(move || {
                let _ = sender.send(query());
            });
        }

        if self.is_fresh(now) || self.is_usable_stale(now) {
            self.value.clone()
        } else {
            None
        }
    }

    fn collect_finished(&mut self, now: Instant) {
        let result = self.receiver.as_ref().map(|receiver| receiver.try_recv());

        match result {
            Some(Ok(Some(value))) => {
                self.value = Some(value);
                self.updated_at = Some(now);
                self.receiver = None;
                self.query_started_at = None;
            }
            Some(Ok(None)) | Some(Err(TryRecvError::Disconnected)) => {
                self.receiver = None;
                self.query_started_at = None;
            }
            Some(Err(TryRecvError::Empty)) => {
                if self.query_started_at.is_some_and(|started_at| {
                    now.duration_since(started_at) > ASYNC_PROVIDER_ABANDON_AFTER
                }) {
                    self.receiver = None;
                    self.query_started_at = None;
                }
            }
            None => {}
        }
    }

    fn is_fresh(&self, now: Instant) -> bool {
        self.updated_at
            .is_some_and(|updated_at| now.duration_since(updated_at) <= self.ttl)
    }

    fn is_usable_stale(&self, now: Instant) -> bool {
        self.value.is_some()
            && self
                .updated_at
                .is_some_and(|updated_at| now.duration_since(updated_at) <= STALE_PROVIDER_AFTER)
    }

    fn is_pending(&self) -> bool {
        self.receiver.is_some()
    }
}

#[cfg(windows)]
#[derive(Clone, Debug, Default)]
struct MotherboardIdentification {
    baseboard_manufacturer: Option<String>,
    baseboard_product: Option<String>,
    baseboard_version: Option<String>,
    baseboard_serial: Option<String>,
    bios_vendor: Option<String>,
    bios_version: Option<String>,
    bios_date: Option<String>,
    system_manufacturer: Option<String>,
    system_model: Option<String>,
    controller_hints: Vec<String>,
}

#[cfg(windows)]
impl MotherboardIdentification {
    fn has_any_value(&self) -> bool {
        self.baseboard_manufacturer.is_some()
            || self.baseboard_product.is_some()
            || self.baseboard_version.is_some()
            || self.bios_vendor.is_some()
            || self.bios_version.is_some()
            || self.system_manufacturer.is_some()
            || self.system_model.is_some()
            || !self.controller_hints.is_empty()
    }

    fn board_label(&self) -> String {
        join_nonempty(
            [
                self.baseboard_manufacturer.as_deref(),
                self.baseboard_product.as_deref(),
                self.baseboard_version.as_deref(),
            ],
            " ",
        )
        .unwrap_or_else(|| "Unknown motherboard".to_owned())
    }

    fn detail(&self) -> String {
        let mut parts = Vec::new();
        parts.push(format!("Board: {}", self.board_label()));
        if let Some(system) = join_nonempty(
            [
                self.system_manufacturer.as_deref(),
                self.system_model.as_deref(),
            ],
            " ",
        ) {
            parts.push(format!("System: {system}"));
        }
        if let Some(bios) = join_nonempty(
            [
                self.bios_vendor.as_deref(),
                self.bios_version.as_deref(),
                self.bios_date.as_deref(),
            ],
            " ",
        ) {
            parts.push(format!("BIOS: {bios}"));
        }
        if !self.controller_hints.is_empty() {
            parts.push(format!(
                "Controller hints: {}",
                self.controller_hints.join("; ")
            ));
        }
        parts.push(
            "Super I/O chip ID is not exposed by standard Windows APIs; BenchScope will not probe I/O ports until a board/chip allowlist is added."
                .to_owned(),
        );
        parts.join(" | ")
    }
}

#[cfg(windows)]
struct BridgeState {
    device: Option<DeviceHandle>,
    version: Option<sensor_driver_client::BenchScopeSensorVersion>,
    capabilities: Option<sensor_driver_client::BenchScopeSensorCapabilities>,
    driver_start_attempted: bool,
    cpu_sampler: CpuUtilizationSampler,
    cpu_temperature: AsyncCachedReading,
    cpu_utilization: CachedReading,
    gpu_temperature: AsyncCachedReading,
    gpu_utilization: AsyncCachedReading,
    gpu_memory: AsyncCachedReading,
    drive_temperature: AsyncCachedReading,
    memory_utilization: CachedReading,
    cpu_energy_sample: Option<(u64, Instant)>,
    motherboard_identification: Option<MotherboardIdentification>,
    motherboard_identification_attempted: bool,
}

#[cfg(windows)]
impl BridgeState {
    fn new() -> Self {
        Self {
            device: None,
            version: None,
            capabilities: None,
            driver_start_attempted: false,
            cpu_sampler: CpuUtilizationSampler::new(),
            cpu_temperature: AsyncCachedReading::new(GPU_TEMP_TTL),
            cpu_utilization: CachedReading::new(FAST_PROVIDER_TTL),
            gpu_temperature: AsyncCachedReading::new(GPU_TEMP_TTL),
            gpu_utilization: AsyncCachedReading::new(FAST_PROVIDER_TTL),
            gpu_memory: AsyncCachedReading::new(FAST_PROVIDER_TTL),
            drive_temperature: AsyncCachedReading::new(DRIVE_TEMP_TTL),
            memory_utilization: CachedReading::new(FAST_PROVIDER_TTL),
            cpu_energy_sample: None,
            motherboard_identification: None,
            motherboard_identification_attempted: false,
        }
    }

    fn snapshot_json(&mut self) -> String {
        match self.driver_snapshot_json() {
            Ok(json) => json,
            Err(error) => {
                self.device = None;
                self.version = None;
                self.capabilities = None;
                self.safe_snapshot_json(&error)
            }
        }
    }

    fn warm_up_slow_providers(&mut self, max_wait: Duration) {
        let started = Instant::now();
        if let Err(error) = self.driver_snapshot_json() {
            let _ = self.safe_snapshot_json(&error);
        }

        while started.elapsed() < max_wait {
            let now = Instant::now();
            self.cpu_temperature.collect_finished(now);
            self.gpu_temperature.collect_finished(now);
            self.gpu_utilization.collect_finished(now);
            self.gpu_memory.collect_finished(now);
            self.drive_temperature.collect_finished(now);

            if !self.cpu_temperature.is_pending()
                && !self.gpu_temperature.is_pending()
                && !self.gpu_utilization.is_pending()
                && !self.gpu_memory.is_pending()
                && !self.drive_temperature.is_pending()
            {
                break;
            }

            thread::sleep(Duration::from_millis(50));
        }
    }

    fn driver_snapshot_json(&mut self) -> Result<String, String> {
        if self.device.is_none() {
            self.device = Some(self.open_driver_with_auto_start()?);
        }
        let device = self.device.as_ref().expect("device was just opened");

        if self.version.is_none() {
            self.version = Some(device.version()?);
        }
        if self.capabilities.is_none() {
            self.capabilities = Some(device.capabilities()?);
        }

        let version = self.version.expect("version was just queried");
        let capabilities = self.capabilities.expect("capabilities were just queried");
        let snapshot = device.snapshot()?;
        let advanced = device.advanced_telemetry().ok();
        if !self.motherboard_identification_attempted {
            self.motherboard_identification = query_motherboard_identification();
            self.motherboard_identification_attempted = true;
        }
        let mut readings = driver_readings_from_snapshot(&snapshot);
        let cpu_power_w = self.derive_cpu_package_power_w(advanced.as_ref());
        if let Some(advanced) = &advanced {
            apply_advanced_telemetry_to_readings(&mut readings, advanced, cpu_power_w);
        }
        self.merge_user_mode_providers(&mut readings);

        Ok(snapshot_json_from_parts(
            version,
            capabilities,
            advanced.as_ref(),
            self.motherboard_identification.as_ref(),
            readings,
        ))
    }

    fn safe_snapshot_json(&mut self, driver_error: &str) -> String {
        let provider = "BenchScope sensor driver bridge";
        let mut readings = BridgeReadings {
            cpu: BridgeReading::unavailable(
                "CPU",
                provider,
                "unavailable",
                Some(driver_error.to_owned()),
            ),
            gpu: BridgeReading::unavailable(
                "GPU",
                provider,
                "unavailable",
                Some(driver_error.to_owned()),
            ),
            gpu_memory: BridgeReading::unavailable(
                "VRAM",
                provider,
                "unavailable",
                Some(driver_error.to_owned()),
            ),
            drive: BridgeReading::unavailable(
                "SSD",
                provider,
                "unavailable",
                Some(driver_error.to_owned()),
            ),
            memory: BridgeReading::unavailable(
                "System RAM",
                provider,
                "unavailable",
                Some(driver_error.to_owned()),
            ),
        };
        self.merge_user_mode_providers(&mut readings);
        fallback_snapshot_json(driver_error, readings)
    }

    fn merge_user_mode_providers(&mut self, readings: &mut BridgeReadings) {
        let now = Instant::now();
        let query_cpu_utilization_now = !self.cpu_utilization.is_fresh(now);
        let query_memory_utilization_now = !self.memory_utilization.is_fresh(now);

        let next_cpu_utilization =
            query_cpu_utilization_now.then(|| self.cpu_sampler.query_utilization());
        let next_memory_utilization = query_memory_utilization_now.then(query_memory_utilization);

        let cpu_temperature = self
            .cpu_temperature
            .resolve_query(now, query_cpu_temperature);
        let cpu_utilization = self
            .cpu_utilization
            .resolve_query(now, next_cpu_utilization);
        let gpu_temperature = self
            .gpu_temperature
            .resolve_query(now, query_gpu_temperature);
        let gpu_utilization = self
            .gpu_utilization
            .resolve_query(now, query_gpu_utilization);
        let gpu_memory = self.gpu_memory.resolve_query(now, query_gpu_memory_sensor);
        let drive_temperature = self
            .drive_temperature
            .resolve_query(now, query_drive_temperature);
        let memory_utilization = self
            .memory_utilization
            .resolve_query(now, next_memory_utilization);

        if let Some(reading) = cpu_temperature {
            readings.cpu.merge_safe_provider(reading);
        }
        if let Some(reading) = cpu_utilization {
            readings.cpu.merge_safe_provider(reading);
        }
        if let Some(reading) = gpu_temperature {
            readings.gpu.merge_safe_provider(reading);
        }
        if let Some(reading) = gpu_utilization {
            readings.gpu.merge_safe_provider(reading);
        }
        if let Some(reading) = gpu_memory {
            readings.gpu_memory.merge_safe_provider(reading);
        }
        if let Some(reading) = drive_temperature {
            readings.drive.merge_safe_provider(reading);
        }
        if let Some(reading) = memory_utilization {
            readings.memory.merge_safe_provider(reading);
        }
    }

    fn derive_cpu_package_power_w(
        &mut self,
        advanced: Option<&BenchScopeSensorAdvancedTelemetry>,
    ) -> Option<f32> {
        let advanced = advanced?;
        let reading = advanced
            .readings
            .iter()
            .take(advanced.reading_count.min(advanced.readings.len() as u32) as usize)
            .find(|reading| {
                kind_json_key(kind_from_driver(reading.kind)) == "cpu"
                    && reading.flags & BENCHSCOPE_SENSOR_ADVANCED_HAS_ENERGY != 0
            })?;
        let now = Instant::now();
        let energy_mj = reading.energy_milli_joules;
        let previous = self.cpu_energy_sample.replace((energy_mj, now));
        let (previous_energy_mj, previous_at) = previous?;
        let elapsed_s = now.duration_since(previous_at).as_secs_f32();
        if elapsed_s <= 0.0 || energy_mj < previous_energy_mj {
            return None;
        }
        let delta_j = (energy_mj - previous_energy_mj) as f32 / 1000.0;
        let watts = delta_j / elapsed_s;
        (0.0..=1000.0)
            .contains(&watts)
            .then_some(round_tenth(watts))
    }

    fn open_driver_with_auto_start(&mut self) -> Result<DeviceHandle, String> {
        match DeviceHandle::open_default() {
            Ok(device) => Ok(device),
            Err(first_error) => {
                if self.driver_start_attempted || !driver_open_error_can_auto_start(&first_error) {
                    return Err(first_error);
                }

                self.driver_start_attempted = true;
                let start_result = start_driver_service();
                let started = Instant::now();
                let mut last_error = first_error.clone();
                while started.elapsed() <= DRIVER_START_RETRY_TIMEOUT {
                    match DeviceHandle::open_default() {
                        Ok(device) => return Ok(device),
                        Err(error) => {
                            last_error = error;
                            thread::sleep(Duration::from_millis(100));
                        }
                    }
                }

                match start_result {
                    Ok(message) => Err(format!(
                        "{last_error}; automatic driver start was requested ({message}) but the device did not open before timeout"
                    )),
                    Err(start_error) => Err(format!(
                        "{first_error}; automatic driver start failed: {start_error}"
                    )),
                }
            }
        }
    }
}

#[cfg(not(windows))]
struct BridgeState;

#[cfg(not(windows))]
impl BridgeState {
    fn new() -> Self {
        Self
    }
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug)]
struct SystemTimesSample {
    idle: u64,
    kernel: u64,
    user: u64,
}

#[cfg(windows)]
impl SystemTimesSample {
    fn read() -> Option<Self> {
        let mut idle = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        unsafe {
            GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).ok()?;
        }
        Some(Self {
            idle: filetime_ticks(idle),
            kernel: filetime_ticks(kernel),
            user: filetime_ticks(user),
        })
    }

    fn utilization_since(self, previous: Self) -> Option<f32> {
        let idle_delta = self.idle.checked_sub(previous.idle)?;
        let kernel_delta = self.kernel.checked_sub(previous.kernel)?;
        let user_delta = self.user.checked_sub(previous.user)?;
        let total_delta = kernel_delta.checked_add(user_delta)?;
        if total_delta == 0 {
            return None;
        }
        let busy_delta = total_delta.saturating_sub(idle_delta);
        Some((busy_delta as f32 / total_delta as f32) * 100.0)
    }
}

#[cfg(windows)]
#[derive(Debug)]
struct CpuUtilizationSampler {
    previous: Option<SystemTimesSample>,
}

#[cfg(windows)]
impl CpuUtilizationSampler {
    fn new() -> Self {
        Self { previous: None }
    }

    fn query_utilization(&mut self) -> Option<BridgeReading> {
        let current = SystemTimesSample::read()?;
        if let Some(previous) = self.previous.replace(current) {
            return current
                .utilization_since(previous)
                .map(cpu_utilization_reading);
        }

        thread::sleep(CPU_INITIAL_SAMPLE_WINDOW);
        let next = SystemTimesSample::read()?;
        self.previous = Some(next);
        next.utilization_since(current).map(cpu_utilization_reading)
    }
}

#[cfg(windows)]
fn filetime_ticks(value: FILETIME) -> u64 {
    ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64
}

#[cfg(windows)]
fn cpu_utilization_reading(value: f32) -> BridgeReading {
    BridgeReading::from_utilization(
        "CPU",
        value,
        "Windows system times",
        "CPU temperature unavailable; utilization is live",
    )
}

#[cfg(windows)]
struct BridgeReadings {
    cpu: BridgeReading,
    gpu: BridgeReading,
    gpu_memory: BridgeReading,
    drive: BridgeReading,
    memory: BridgeReading,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stream = args.iter().any(|arg| arg == "--stream");
    let interval_ms = argument_value(&args, "--interval-ms")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (100..=60_000).contains(value))
        .unwrap_or(DEFAULT_INTERVAL_MS);
    let mut state = BridgeState::new();

    if stream {
        loop {
            let started = Instant::now();
            emit_snapshot_line(&mut state);
            let elapsed = started.elapsed();
            let interval = Duration::from_millis(interval_ms);
            if elapsed < interval {
                thread::sleep(interval - elapsed);
            }
        }
    } else {
        #[cfg(windows)]
        state.warm_up_slow_providers(SLOW_COMMAND_TIMEOUT + Duration::from_millis(250));
        emit_snapshot_line(&mut state);
    }
}

fn argument_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|items| items[0] == name)
        .map(|items| items[1].as_str())
}

fn emit_snapshot_line(state: &mut BridgeState) {
    println!("{}", snapshot_json(state));
    let _ = io::stdout().flush();
}

fn snapshot_json(state: &mut BridgeState) -> String {
    #[cfg(windows)]
    {
        state.snapshot_json()
    }

    #[cfg(not(windows))]
    {
        error_snapshot_json("BenchScope sensor driver bridge is only available on Windows")
    }
}

#[cfg(windows)]
fn driver_readings_from_snapshot(
    snapshot: &sensor_driver_client::BenchScopeSensorSnapshot,
) -> BridgeReadings {
    let mut readings = BridgeReadings {
        cpu: BridgeReading::unavailable(
            "CPU",
            "BenchScope sensor driver bridge",
            "unsupported",
            None,
        ),
        gpu: BridgeReading::unavailable(
            "GPU",
            "BenchScope sensor driver bridge",
            "unsupported",
            None,
        ),
        gpu_memory: BridgeReading::unavailable(
            "VRAM",
            "BenchScope sensor driver bridge",
            "unsupported",
            None,
        ),
        drive: BridgeReading::unavailable(
            "SSD",
            "BenchScope sensor driver bridge",
            "unsupported",
            None,
        ),
        memory: BridgeReading::unavailable(
            "System RAM",
            "BenchScope sensor driver bridge",
            "unsupported",
            None,
        ),
    };

    for reading in snapshot
        .readings
        .iter()
        .take(snapshot.reading_count.min(snapshot.readings.len() as u32) as usize)
    {
        match kind_json_key(kind_from_driver(reading.kind)) {
            "cpu" => readings.cpu = BridgeReading::from_driver(reading),
            "gpu" => readings.gpu = BridgeReading::from_driver(reading),
            "drive" => readings.drive = BridgeReading::from_driver(reading),
            "memory" => readings.memory = BridgeReading::from_driver(reading),
            _ => {}
        }
    }

    readings
}

#[cfg(windows)]
fn apply_advanced_telemetry_to_readings(
    readings: &mut BridgeReadings,
    advanced: &BenchScopeSensorAdvancedTelemetry,
    cpu_power_w: Option<f32>,
) {
    for reading in advanced
        .readings
        .iter()
        .take(advanced.reading_count.min(advanced.readings.len() as u32) as usize)
    {
        if kind_json_key(kind_from_driver(reading.kind)) != "cpu" {
            continue;
        }
        if reading.flags & BENCHSCOPE_SENSOR_ADVANCED_HAS_TEMPERATURE != 0
            && readings.cpu.temperature_c.is_none()
        {
            readings.cpu.temperature_c = Some(reading.temperature_milli_c as f32 / 1000.0);
            readings.cpu.label = wide_to_string(&reading.label);
            readings.cpu.provider = wide_to_string(&reading.provider);
            readings.cpu.status = status_json_value(status_from_driver(reading.status)).to_owned();
            readings.cpu.upsert_metric(BridgeMetric::new(
                BridgeMetricKind::Temperature,
                &readings.cpu.label.clone(),
                readings.cpu.temperature_c,
            ));
        }
        if reading.flags & BENCHSCOPE_SENSOR_ADVANCED_HAS_POWER != 0 {
            readings.cpu.upsert_metric(BridgeMetric::new(
                BridgeMetricKind::Power,
                "CPU Package",
                Some(reading.power_milli_watts as f32 / 1000.0),
            ));
        }
        if reading.flags & BENCHSCOPE_SENSOR_ADVANCED_HAS_VOLTAGE != 0 {
            readings.cpu.upsert_metric(BridgeMetric::new(
                BridgeMetricKind::Voltage,
                "CPU Voltage",
                Some(reading.voltage_milli_v as f32 / 1000.0),
            ));
        }

        let mut details = Vec::new();
        if reading.flags & BENCHSCOPE_SENSOR_ADVANCED_HAS_THERMAL_LIMIT != 0 {
            details.push(format!(
                "thermal limit {:.1} C",
                reading.thermal_limit_milli_c as f32 / 1000.0
            ));
        }
        if reading.flags & BENCHSCOPE_SENSOR_ADVANCED_HAS_ENERGY != 0 {
            details.push(format!(
                "package energy {:.3} J",
                reading.energy_milli_joules as f64 / 1000.0
            ));
        }
        if reading.flags & BENCHSCOPE_SENSOR_ADVANCED_THERMAL_THROTTLED != 0 {
            details.push("thermal limit active".to_owned());
        }
        let driver_detail = wide_to_string(&reading.detail);
        if !driver_detail.is_empty() {
            details.push(driver_detail);
        }
        if !details.is_empty() {
            readings.cpu.message = Some(details.join("; "));
        }
    }
    if let Some(cpu_power_w) = cpu_power_w {
        readings.cpu.upsert_metric(BridgeMetric::new(
            BridgeMetricKind::Power,
            "CPU Package",
            Some(cpu_power_w),
        ));
    }
}

#[cfg(windows)]
fn snapshot_json_from_parts(
    version: sensor_driver_client::BenchScopeSensorVersion,
    capabilities: sensor_driver_client::BenchScopeSensorCapabilities,
    advanced: Option<&BenchScopeSensorAdvancedTelemetry>,
    motherboard: Option<&MotherboardIdentification>,
    readings: BridgeReadings,
) -> String {
    let fields = [
        format!("\"timestampUtc\":\"{}\"", json_escape(&timestamp_label())),
        "\"isElevated\":true".to_owned(),
        "\"source\":\"BenchScopeSensorService\"".to_owned(),
        format!(
            "\"driver\":{{\"protocol\":{},\"version\":\"{}.{}.{}\",\"cpuTemp\":{},\"gpuTemp\":{},\"driveTemp\":{},\"utilization\":{}}}",
            version.protocol_version,
            version.driver_major,
            version.driver_minor,
            version.driver_patch,
            json_bool(capabilities.supports_cpu_temperature),
            json_bool(capabilities.supports_gpu_temperature),
            json_bool(capabilities.supports_drive_temperature),
            json_bool(capabilities.supports_utilization),
        ),
        format!(
            "\"advancedTelemetry\":{}",
            advanced_telemetry_json(advanced, motherboard)
        ),
        format!("\"cpu\":{}", bridge_reading_json(&readings.cpu)),
        format!("\"gpu\":{}", bridge_reading_json(&readings.gpu)),
        format!(
            "\"gpuMemory\":{}",
            bridge_reading_json(&readings.gpu_memory)
        ),
        format!("\"drive\":{}", bridge_reading_json(&readings.drive)),
        format!("\"memory\":{}", bridge_reading_json(&readings.memory)),
    ];

    format!("{{{}}}", fields.join(","))
}

#[cfg(windows)]
fn advanced_telemetry_json(
    advanced: Option<&BenchScopeSensorAdvancedTelemetry>,
    motherboard: Option<&MotherboardIdentification>,
) -> String {
    let Some(advanced) = advanced else {
        return "null".to_owned();
    };
    let readings = advanced
        .readings
        .iter()
        .take(advanced.reading_count.min(advanced.readings.len() as u32) as usize)
        .map(|reading| {
            let temperature = optional_milli_value_json(
                reading.flags & BENCHSCOPE_SENSOR_ADVANCED_HAS_TEMPERATURE,
                reading.temperature_milli_c,
            );
            let thermal_limit = optional_milli_value_json(
                reading.flags & BENCHSCOPE_SENSOR_ADVANCED_HAS_THERMAL_LIMIT,
                reading.thermal_limit_milli_c,
            );
            let energy = if reading.flags & BENCHSCOPE_SENSOR_ADVANCED_HAS_ENERGY == 0 {
                "null".to_owned()
            } else {
                format!("{:.3}", reading.energy_milli_joules as f64 / 1000.0)
            };
            let kind = kind_json_key(kind_from_driver(reading.kind));
            let mut detail = wide_to_string(&reading.detail);
            if kind == "motherboard" {
                if let Some(motherboard) = motherboard {
                    detail = if detail.is_empty() {
                        motherboard.detail()
                    } else {
                        format!("{} {}", detail, motherboard.detail())
                    };
                }
            }
            format!(
                "{{\"kind\":\"{}\",\"label\":\"{}\",\"status\":\"{}\",\"flags\":{},\"temperatureC\":{},\"thermalLimitC\":{},\"energyJ\":{},\"thermalThrottled\":{},\"userModeProvider\":{},\"provider\":\"{}\",\"detail\":\"{}\"}}",
                kind,
                json_escape(&wide_to_string(&reading.label)),
                status_json_value(status_from_driver(reading.status)),
                reading.flags,
                temperature,
                thermal_limit,
                energy,
                json_bool_from_bool(reading.flags & BENCHSCOPE_SENSOR_ADVANCED_THERMAL_THROTTLED != 0),
                json_bool_from_bool(reading.flags & BENCHSCOPE_SENSOR_ADVANCED_USER_MODE_PROVIDER != 0),
                json_escape(&wide_to_string(&reading.provider)),
                json_escape(&detail),
            )
        })
        .collect::<Vec<_>>();
    format!(
        "{{\"providerMask\":{},\"sequence\":{},\"motherboardIdentification\":{},\"readings\":[{}]}}",
        advanced.provider_mask,
        advanced.sequence,
        motherboard_identification_json(motherboard),
        readings.join(",")
    )
}

#[cfg(windows)]
fn motherboard_identification_json(motherboard: Option<&MotherboardIdentification>) -> String {
    let Some(motherboard) = motherboard else {
        return "null".to_owned();
    };
    let controllers = motherboard
        .controller_hints
        .iter()
        .map(|value| format!("\"{}\"", json_escape(value)))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"board\":\"{}\",\"baseboardManufacturer\":{},\"baseboardProduct\":{},\"baseboardVersion\":{},\"baseboardSerial\":{},\"biosVendor\":{},\"biosVersion\":{},\"biosDate\":{},\"systemManufacturer\":{},\"systemModel\":{},\"controllerHints\":[{}],\"superIoChip\":\"not exposed by standard Windows APIs\"}}",
        json_escape(&motherboard.board_label()),
        json_optional_string(motherboard.baseboard_manufacturer.as_deref()),
        json_optional_string(motherboard.baseboard_product.as_deref()),
        json_optional_string(motherboard.baseboard_version.as_deref()),
        json_optional_string(motherboard.baseboard_serial.as_deref()),
        json_optional_string(motherboard.bios_vendor.as_deref()),
        json_optional_string(motherboard.bios_version.as_deref()),
        json_optional_string(motherboard.bios_date.as_deref()),
        json_optional_string(motherboard.system_manufacturer.as_deref()),
        json_optional_string(motherboard.system_model.as_deref()),
        controllers,
    )
}

#[cfg(windows)]
fn optional_milli_value_json(has_value: u32, milli_value: i32) -> String {
    if has_value == 0 {
        "null".to_owned()
    } else {
        format!("{:.1}", milli_value as f32 / 1000.0)
    }
}

#[cfg(windows)]
fn driver_open_error_can_auto_start(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    !lower.contains("access is denied") && !lower.contains("permission")
}

#[cfg(windows)]
fn start_driver_service() -> Result<String, String> {
    let driver_sys = sensor_driver_sys_path()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let driver_sys = powershell_single_quote(&driver_sys);
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
$service = Get-Service -Name '{DRIVER_SERVICE_NAME}' -ErrorAction Stop
$driverSys = {driver_sys}
$serviceKey = 'HKLM:\SYSTEM\CurrentControlSet\Services\{DRIVER_SERVICE_NAME}'
$imagePath = ''
try {{
    $imagePath = [string](Get-ItemProperty -Path $serviceKey -Name ImagePath -ErrorAction Stop).ImagePath
}} catch {{
    $imagePath = ''
}}
$normalizedImagePath = [Environment]::ExpandEnvironmentVariables($imagePath)
if ($normalizedImagePath.StartsWith('\??\')) {{
    $normalizedImagePath = $normalizedImagePath.Substring(4)
}}
if ($driverSys -and (Test-Path -LiteralPath $driverSys) -and
    ($normalizedImagePath -and -not (Test-Path -LiteralPath $normalizedImagePath))) {{
    & sc.exe config '{DRIVER_SERVICE_NAME}' type= kernel start= demand binPath= $driverSys | Out-Null
    if ($LASTEXITCODE -ne 0) {{
        throw "sc.exe config failed while repairing stale driver path $imagePath"
    }}
}}
if ($service.Status -ne 'Running') {{
    Start-Service -Name '{DRIVER_SERVICE_NAME}' -ErrorAction Stop
    $service.WaitForStatus('Running', [TimeSpan]::FromSeconds(5))
}}
$service = Get-Service -Name '{DRIVER_SERVICE_NAME}' -ErrorAction Stop
"$($service.Status)"
"#
    );
    run_command_timeout(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ],
        DRIVER_START_RETRY_TIMEOUT + Duration::from_secs(2),
    )
    .map(|output| {
        if output.trim().is_empty() {
            "Start-Service completed".to_owned()
        } else {
            output
        }
    })
}

#[cfg(windows)]
fn query_motherboard_identification() -> Option<MotherboardIdentification> {
    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
function Clean($value) {
    if ($null -eq $value) { return '' }
    (($value -join ' ' -replace "`t", " ") -replace "`r?`n", " ").Trim()
}
function Emit($values) {
    [Console]::Out.WriteLine([string]::Join("`t", $values))
}
$baseboard = Get-CimInstance Win32_BaseBoard -ErrorAction SilentlyContinue | Select-Object -First 1
$bios = Get-CimInstance Win32_BIOS -ErrorAction SilentlyContinue | Select-Object -First 1
$system = Get-CimInstance Win32_ComputerSystem -ErrorAction SilentlyContinue | Select-Object -First 1
Emit @(
    'BOARD',
    (Clean $baseboard.Manufacturer),
    (Clean $baseboard.Product),
    (Clean $baseboard.Version),
    (Clean $baseboard.SerialNumber),
    (Clean $bios.Manufacturer),
    (Clean $bios.SMBIOSBIOSVersion),
    (Clean $bios.ReleaseDate),
    (Clean $system.Manufacturer),
    (Clean $system.Model)
)
$controllers = Get-CimInstance Win32_PnPEntity -ErrorAction SilentlyContinue |
    Where-Object {
        $_.Name -match '(?i)(LPC|eSPI|SMBus|System Management Bus|ISA Bridge|Super I/O|SuperIO|Embedded Controller)' -or
        $_.PNPClass -match '(?i)(System|Processor|HDC)'
    } |
    Where-Object {
        "$($_.Name) $($_.DeviceID)" -match '(?i)(LPC|eSPI|SMBus|System Management Bus|ISA Bridge|Super I/O|SuperIO|Embedded Controller)'
    } |
    Select-Object -First 12
foreach ($controller in $controllers) {
    Emit @('CTRL', (Clean $controller.Name), (Clean $controller.Manufacturer), (Clean $controller.DeviceID))
}
"#;
    run_slow_powershell(script)
        .ok()
        .and_then(|output| parse_motherboard_identification(&output))
}

#[cfg(windows)]
fn parse_motherboard_identification(output: &str) -> Option<MotherboardIdentification> {
    let mut identification = MotherboardIdentification::default();
    for line in output.lines() {
        let columns = line.split('\t').collect::<Vec<_>>();
        match columns.first().copied() {
            Some("BOARD") => {
                identification.baseboard_manufacturer = clean_identification_field(columns.get(1));
                identification.baseboard_product = clean_identification_field(columns.get(2));
                identification.baseboard_version = clean_identification_field(columns.get(3));
                identification.baseboard_serial = clean_identification_field(columns.get(4));
                identification.bios_vendor = clean_identification_field(columns.get(5));
                identification.bios_version = clean_identification_field(columns.get(6));
                identification.bios_date = clean_identification_field(columns.get(7));
                identification.system_manufacturer = clean_identification_field(columns.get(8));
                identification.system_model = clean_identification_field(columns.get(9));
            }
            Some("CTRL") => {
                let label = join_nonempty(
                    [
                        columns.get(1).copied(),
                        columns.get(2).copied(),
                        columns.get(3).copied(),
                    ],
                    " | ",
                );
                if let Some(label) = label {
                    if !identification.controller_hints.contains(&label) {
                        identification.controller_hints.push(label);
                    }
                }
            }
            _ => {}
        }
    }

    identification.has_any_value().then_some(identification)
}

#[cfg(windows)]
fn clean_identification_field(value: Option<&&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty()
        || value.eq_ignore_ascii_case("none")
        || value.eq_ignore_ascii_case("null")
        || value.eq_ignore_ascii_case("to be filled by o.e.m.")
        || value.eq_ignore_ascii_case("default string")
        || value.eq_ignore_ascii_case("system product name")
        || value.eq_ignore_ascii_case("base board product name")
    {
        None
    } else {
        Some(value.to_owned())
    }
}

#[cfg(windows)]
fn query_cpu_temperature() -> Option<BridgeReading> {
    query_external_hardware_metrics("CPU", external_cpu_metrics_script())
        .or_else(|| query_external_hardware_temperature("CPU", external_cpu_temperature_script()))
}

#[cfg(windows)]
fn query_gpu_temperature() -> Option<BridgeReading> {
    const NVIDIA_SMI_ARGS: &[&str] = &[
        "--query-gpu=temperature.gpu,name,utilization.gpu,power.draw,clocks.gr,clocks.mem",
        "--format=csv,noheader,nounits",
    ];
    run_nvidia_smi_query(NVIDIA_SMI_ARGS)
        .and_then(|output| nvidia_smi_gpu_reading(&output))
        .or_else(|| query_external_hardware_metrics("GPU", external_gpu_metrics_script()))
}

#[cfg(windows)]
fn run_nvidia_smi_query(args: &[&str]) -> Option<String> {
    run_command("nvidia-smi", args).ok().or_else(|| {
        nvidia_smi_fallback_paths()
            .into_iter()
            .find(|path| path.is_file())
            .and_then(|path| run_command(&path.display().to_string(), args).ok())
    })
}

#[cfg(windows)]
fn nvidia_smi_gpu_reading(output: &str) -> Option<BridgeReading> {
    let line = output.lines().find(|line| !line.trim().is_empty())?;
    let columns = line.split(',').map(str::trim).collect::<Vec<_>>();
    let label = columns
        .get(1)
        .copied()
        .filter(|name| !name.is_empty())
        .unwrap_or("GPU");
    let mut metrics = Vec::new();
    if let Some(value) = columns
        .first()
        .and_then(|value| parse_metric_number(value))
        .filter(|value| (-40.0..=130.0).contains(value))
    {
        metrics.push(BridgeMetric::new(
            BridgeMetricKind::Temperature,
            "GPU Core",
            Some(round_tenth(value)),
        ));
    }
    if let Some(value) = columns
        .get(2)
        .and_then(|value| parse_metric_number(value))
        .map(clamp_percent)
    {
        metrics.push(BridgeMetric::new(
            BridgeMetricKind::Utilization,
            "GPU Core",
            Some(value),
        ));
    }
    if let Some(value) = columns
        .get(3)
        .and_then(|value| parse_metric_number(value))
        .filter(|value| (0.0..=2000.0).contains(value))
    {
        metrics.push(BridgeMetric::new(
            BridgeMetricKind::Power,
            "GPU Board",
            Some(round_tenth(value)),
        ));
    }
    if let Some(value) = columns
        .get(4)
        .and_then(|value| parse_metric_number(value))
        .filter(|value| (0.0..=20_000.0).contains(value))
    {
        metrics.push(BridgeMetric::new(
            BridgeMetricKind::Clock,
            "Core Clock",
            Some(round_tenth(value)),
        ));
    }
    if let Some(value) = columns
        .get(5)
        .and_then(|value| parse_metric_number(value))
        .filter(|value| (0.0..=30_000.0).contains(value))
    {
        metrics.push(BridgeMetric::new(
            BridgeMetricKind::Clock,
            "Memory Clock",
            Some(round_tenth(value)),
        ));
    }

    (!metrics.is_empty()).then(|| BridgeReading::from_metrics(label, "NVML/nvidia-smi", metrics))
}

#[cfg(windows)]
fn query_gpu_memory_sensor() -> Option<BridgeReading> {
    const NVIDIA_SMI_MEMORY_ARGS: &[&str] = &[
        "--query-gpu=name,temperature.memory,memory.used,memory.total",
        "--format=csv,noheader,nounits",
    ];
    const NVIDIA_SMI_MEMORY_FALLBACK_ARGS: &[&str] = &[
        "--query-gpu=name,memory.used,memory.total",
        "--format=csv,noheader,nounits",
    ];

    run_nvidia_smi_query(NVIDIA_SMI_MEMORY_ARGS)
        .or_else(|| run_nvidia_smi_query(NVIDIA_SMI_MEMORY_FALLBACK_ARGS))
        .and_then(|output| nvidia_smi_gpu_memory_reading(&output))
}

#[cfg(windows)]
fn nvidia_smi_gpu_memory_reading(output: &str) -> Option<BridgeReading> {
    let line = output.lines().find(|line| !line.trim().is_empty())?;
    let columns = line.split(',').map(str::trim).collect::<Vec<_>>();
    let has_temperature_column = columns.len() >= 4;
    let temperature_c = has_temperature_column
        .then(|| columns.get(1).and_then(|value| parse_metric_number(value)))
        .flatten()
        .filter(|value| (-40.0..=130.0).contains(value))
        .map(round_tenth);
    let used_mib_column = if has_temperature_column { 2 } else { 1 };
    let total_mib_column = if has_temperature_column { 3 } else { 2 };
    let used_gb = columns
        .get(used_mib_column)
        .and_then(|value| parse_metric_number(value))
        .map(mib_to_gb);
    let total_gb = columns
        .get(total_mib_column)
        .and_then(|value| parse_metric_number(value))
        .map(mib_to_gb)
        .filter(|value| *value > 0.0);

    let mut metrics = Vec::new();
    if let Some(value) = temperature_c {
        metrics.push(BridgeMetric::new(
            BridgeMetricKind::Temperature,
            "VRAM",
            Some(value),
        ));
    }
    if let Some(value) = used_gb {
        metrics.push(
            BridgeMetric::new(
                BridgeMetricKind::MemoryUsage,
                BridgeMetricKind::MemoryUsage.default_label(),
                Some(value),
            )
            .with_range(None, total_gb),
        );
    }

    (!metrics.is_empty()).then(|| BridgeReading::from_metrics("VRAM", "NVML/nvidia-smi", metrics))
}

#[cfg(windows)]
fn query_external_hardware_temperature(
    fallback_label: &str,
    script: &str,
) -> Option<BridgeReading> {
    run_slow_powershell(script).ok().and_then(|output| {
        let mut parts = output.trim().split('\t');
        let temperature = parts.next()?.trim().parse::<f32>().ok()?;
        if !(-40.0..=130.0).contains(&temperature) {
            return None;
        }
        let label = parts
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(fallback_label);
        let namespace = parts
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("external hardware WMI");
        Some(BridgeReading::from_temperature(
            label,
            temperature,
            &format!("External hardware WMI ({namespace})"),
        ))
    })
}

#[cfg(windows)]
fn query_external_hardware_metrics(fallback_label: &str, script: &str) -> Option<BridgeReading> {
    let output = run_slow_powershell(script).ok()?;
    let (metrics, provider) = parse_external_hardware_metrics(&output, fallback_label);
    (!metrics.is_empty()).then(|| BridgeReading::from_metrics(fallback_label, &provider, metrics))
}

#[cfg(windows)]
#[derive(Clone, Debug)]
struct ExternalHardwareMetric {
    kind: BridgeMetricKind,
    label: String,
    value: f32,
    min: Option<f32>,
    max: Option<f32>,
    namespace: String,
}

#[cfg(windows)]
fn parse_external_hardware_metrics(
    output: &str,
    fallback_label: &str,
) -> (Vec<BridgeMetric>, String) {
    let raw = output
        .lines()
        .filter_map(parse_external_hardware_metric_line)
        .collect::<Vec<_>>();
    let provider = raw
        .first()
        .map(|metric| format!("External hardware WMI ({})", metric.namespace))
        .unwrap_or_else(|| "External hardware WMI".to_owned());
    (
        normalize_external_hardware_metrics(fallback_label, &raw),
        provider,
    )
}

#[cfg(windows)]
fn parse_external_hardware_metric_line(line: &str) -> Option<ExternalHardwareMetric> {
    let columns = line.split('\t').collect::<Vec<_>>();
    let sensor_type = columns.first()?.trim();
    let kind = external_sensor_type_kind(sensor_type)?;
    let label = columns
        .get(1)
        .map(|value| clean_metric_label(value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| kind.default_label().to_owned());
    let value = columns
        .get(2)
        .and_then(|value| parse_metric_number(value))?;
    if !metric_value_is_plausible(kind, value) {
        return None;
    }
    let min = columns
        .get(3)
        .and_then(|value| parse_metric_number(value))
        .filter(|value| metric_value_is_plausible(kind, *value));
    let max = columns
        .get(4)
        .and_then(|value| parse_metric_number(value))
        .filter(|value| metric_value_is_plausible(kind, *value));
    let namespace = columns
        .get(5)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "hardware WMI".to_owned());

    Some(ExternalHardwareMetric {
        kind,
        label,
        value: metric_value_for_kind(kind, value),
        min: min.map(|value| metric_value_for_kind(kind, value)),
        max: max.map(|value| metric_value_for_kind(kind, value)),
        namespace,
    })
}

#[cfg(windows)]
fn external_sensor_type_kind(value: &str) -> Option<BridgeMetricKind> {
    match value.to_ascii_lowercase().as_str() {
        "temperature" => Some(BridgeMetricKind::Temperature),
        "load" => Some(BridgeMetricKind::Utilization),
        "voltage" => Some(BridgeMetricKind::Voltage),
        "power" => Some(BridgeMetricKind::Power),
        "clock" => Some(BridgeMetricKind::Clock),
        _ => None,
    }
}

#[cfg(windows)]
fn normalize_external_hardware_metrics(
    fallback_label: &str,
    raw: &[ExternalHardwareMetric],
) -> Vec<BridgeMetric> {
    let mut metrics = Vec::new();
    let is_cpu = fallback_label.eq_ignore_ascii_case("CPU");

    if let Some(metric) = pick_preferred_external_metric(
        raw,
        BridgeMetricKind::Temperature,
        if is_cpu {
            &["package", "tctl", "tdie", "ccd", "core max", "core"]
        } else {
            &["hot spot", "hotspot", "junction", "core", "gpu"]
        },
    ) {
        metrics.push(bridge_metric_from_external(metric, None));
    }

    if let Some(metric) = pick_preferred_external_metric(
        raw,
        BridgeMetricKind::Utilization,
        if is_cpu {
            &["cpu total", "total", "processor"]
        } else {
            &["gpu core", "gpu total", "3d", "graphics", "gfx", "compute"]
        },
    ) {
        metrics.push(bridge_metric_from_external(metric, Some("Utilization")));
    }

    metrics.extend(select_external_metric_rows(
        raw,
        BridgeMetricKind::Voltage,
        if is_cpu {
            &["vcore", "core", "cpu", "soc", "vid"]
        } else {
            &["core", "gpu", "vid", "voltage"]
        },
        if is_cpu { 3 } else { 2 },
    ));
    metrics.extend(select_external_metric_rows(
        raw,
        BridgeMetricKind::Power,
        if is_cpu {
            &["package", "cpu package", "ppt", "cores", "cpu"]
        } else {
            &["board", "total", "chip", "gpu", "power"]
        },
        if is_cpu { 3 } else { 2 },
    ));
    metrics.extend(normalize_external_clock_metrics(fallback_label, raw));

    metrics
}

#[cfg(windows)]
fn pick_preferred_external_metric<'a>(
    raw: &'a [ExternalHardwareMetric],
    kind: BridgeMetricKind,
    preferred: &[&str],
) -> Option<&'a ExternalHardwareMetric> {
    let candidates = raw.iter().filter(|metric| metric.kind == kind);
    preferred
        .iter()
        .find_map(|needle| {
            candidates
                .clone()
                .filter(|metric| metric.label.to_ascii_lowercase().contains(needle))
                .max_by(|left, right| left.value.total_cmp(&right.value))
        })
        .or_else(|| candidates.max_by(|left, right| left.value.total_cmp(&right.value)))
}

#[cfg(windows)]
fn select_external_metric_rows(
    raw: &[ExternalHardwareMetric],
    kind: BridgeMetricKind,
    preferred: &[&str],
    limit: usize,
) -> Vec<BridgeMetric> {
    let mut candidates = raw
        .iter()
        .filter(|metric| metric.kind == kind)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        metric_preference_rank(&left.label, preferred)
            .cmp(&metric_preference_rank(&right.label, preferred))
            .then_with(|| left.label.cmp(&right.label))
    });

    let mut seen = Vec::<String>::new();
    let mut selected = Vec::new();
    for metric in candidates {
        let key = normalized_metric_label(&metric.label);
        if seen.iter().any(|value| value == &key) {
            continue;
        }
        seen.push(key);
        selected.push(bridge_metric_from_external(metric, None));
        if selected.len() >= limit {
            break;
        }
    }
    selected
}

#[cfg(windows)]
fn normalize_external_clock_metrics(
    fallback_label: &str,
    raw: &[ExternalHardwareMetric],
) -> Vec<BridgeMetric> {
    let clocks = raw
        .iter()
        .filter(|metric| metric.kind == BridgeMetricKind::Clock)
        .collect::<Vec<_>>();
    if clocks.is_empty() {
        return Vec::new();
    }

    if fallback_label.eq_ignore_ascii_case("CPU") {
        let intel = cpu_model_name_for_bridge()
            .map(|model| model.to_ascii_lowercase().contains("intel"))
            .unwrap_or(false);
        if intel {
            let p_cores = clocks
                .iter()
                .copied()
                .filter(|metric| {
                    let label = metric.label.to_ascii_lowercase();
                    label.contains("p-core")
                        || label.contains("p core")
                        || label.contains("performance")
                })
                .collect::<Vec<_>>();
            let e_cores = clocks
                .iter()
                .copied()
                .filter(|metric| {
                    let label = metric.label.to_ascii_lowercase();
                    label.contains("e-core")
                        || label.contains("e core")
                        || label.contains("efficient")
                })
                .collect::<Vec<_>>();
            let mut grouped = Vec::new();
            if let Some(metric) = aggregate_external_metrics("P-core Clock", &p_cores) {
                grouped.push(metric);
            }
            if let Some(metric) = aggregate_external_metrics("E-core Clock", &e_cores) {
                grouped.push(metric);
            }
            if !grouped.is_empty() {
                return grouped;
            }
        }

        let core_clocks = clocks
            .iter()
            .copied()
            .filter(|metric| {
                let label = metric.label.to_ascii_lowercase();
                label.contains("core") && !label.contains("bus")
            })
            .collect::<Vec<_>>();
        return aggregate_external_metrics(
            if intel {
                "Core Clock"
            } else {
                "CPU Core Clock"
            },
            if core_clocks.is_empty() {
                &clocks
            } else {
                &core_clocks
            },
        )
        .into_iter()
        .collect();
    }

    let memory_clocks = clocks
        .iter()
        .copied()
        .filter(|metric| metric.label.to_ascii_lowercase().contains("mem"))
        .collect::<Vec<_>>();
    let core_clocks = clocks
        .iter()
        .copied()
        .filter(|metric| !metric.label.to_ascii_lowercase().contains("mem"))
        .collect::<Vec<_>>();
    [
        aggregate_external_metrics("Core Clock", &core_clocks),
        aggregate_external_metrics("Memory Clock", &memory_clocks),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(windows)]
fn aggregate_external_metrics(
    label: &str,
    metrics: &[&ExternalHardwareMetric],
) -> Option<BridgeMetric> {
    if metrics.is_empty() {
        return None;
    }
    let value = metrics
        .iter()
        .map(|metric| metric.value)
        .max_by(f32::total_cmp)?;
    let min = metrics
        .iter()
        .filter_map(|metric| metric.min.or(Some(metric.value)))
        .min_by(f32::total_cmp);
    let max = metrics
        .iter()
        .filter_map(|metric| metric.max.or(Some(metric.value)))
        .max_by(f32::total_cmp);
    Some(BridgeMetric::new(BridgeMetricKind::Clock, label, Some(value)).with_range(min, max))
}

#[cfg(windows)]
fn bridge_metric_from_external(
    metric: &ExternalHardwareMetric,
    label_override: Option<&str>,
) -> BridgeMetric {
    BridgeMetric::new(
        metric.kind,
        label_override.unwrap_or(&metric.label),
        Some(metric.value),
    )
    .with_range(metric.min, metric.max)
}

#[cfg(windows)]
fn metric_preference_rank(label: &str, preferred: &[&str]) -> usize {
    let label = label.to_ascii_lowercase();
    preferred
        .iter()
        .position(|needle| label.contains(needle))
        .unwrap_or(preferred.len())
}

#[cfg(windows)]
fn clean_metric_label(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|ch: char| ch == ':' || ch == '-' || ch.is_whitespace())
        .to_owned()
}

#[cfg(windows)]
fn normalized_metric_label(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(windows)]
fn external_cpu_metrics_script() -> &'static str {
    r#"
$ErrorActionPreference = 'SilentlyContinue'
function Clean($value) {
    if ($null -eq $value) { return '' }
    (($value -join ' ' -replace "`t", " ") -replace "`r?`n", " ").Trim()
}
function Num($value) {
    if ($null -eq $value) { return '' }
    try {
        return [string]::Format([System.Globalization.CultureInfo]::InvariantCulture, '{0:F3}', [double]$value)
    } catch {
        return ''
    }
}
function Emit($sensor, $namespace) {
    [Console]::Out.WriteLine([string]::Join("`t", @(
        (Clean $sensor.SensorType),
        (Clean $sensor.Name),
        (Num $sensor.Value),
        (Num $sensor.Min),
        (Num $sensor.Max),
        $namespace,
        (Clean $sensor.Identifier),
        (Clean $sensor.HardwareType),
        (Clean $sensor.Parent)
    )))
}
$namespaces = @('root\LibreHardwareMonitor', 'root\OpenHardwareMonitor')
foreach ($namespace in $namespaces) {
    $sensors = Get-CimInstance -Namespace $namespace -ClassName Sensor -ErrorAction SilentlyContinue |
        Where-Object {
            $null -ne $_.Value -and
            @('Temperature', 'Load', 'Voltage', 'Power', 'Clock') -contains [string]$_.SensorType
        }
    if (-not $sensors) { continue }
    $candidates = $sensors | Where-Object {
        "$($_.Identifier) $($_.HardwareType) $($_.Parent) $($_.Name)" -match '(?i)(/cpu|intelcpu|amdcpu|cpu)'
    }
    if ($candidates) {
        foreach ($sensor in $candidates) { Emit $sensor $namespace }
        break
    }
}
exit 0
"#
}

#[cfg(windows)]
fn external_gpu_metrics_script() -> &'static str {
    r#"
$ErrorActionPreference = 'SilentlyContinue'
function Clean($value) {
    if ($null -eq $value) { return '' }
    (($value -join ' ' -replace "`t", " ") -replace "`r?`n", " ").Trim()
}
function Num($value) {
    if ($null -eq $value) { return '' }
    try {
        return [string]::Format([System.Globalization.CultureInfo]::InvariantCulture, '{0:F3}', [double]$value)
    } catch {
        return ''
    }
}
function Emit($sensor, $namespace) {
    [Console]::Out.WriteLine([string]::Join("`t", @(
        (Clean $sensor.SensorType),
        (Clean $sensor.Name),
        (Num $sensor.Value),
        (Num $sensor.Min),
        (Num $sensor.Max),
        $namespace,
        (Clean $sensor.Identifier),
        (Clean $sensor.HardwareType),
        (Clean $sensor.Parent)
    )))
}
$namespaces = @('root\LibreHardwareMonitor', 'root\OpenHardwareMonitor')
foreach ($namespace in $namespaces) {
    $sensors = Get-CimInstance -Namespace $namespace -ClassName Sensor -ErrorAction SilentlyContinue |
        Where-Object {
            $null -ne $_.Value -and
            @('Temperature', 'Load', 'Voltage', 'Power', 'Clock') -contains [string]$_.SensorType
        }
    if (-not $sensors) { continue }
    $candidates = $sensors | Where-Object {
        "$($_.Identifier) $($_.HardwareType) $($_.Parent) $($_.Name)" -match '(?i)(/gpu|nvidia|radeon|amd|graphics|intel.*gpu|intel.*graphics)'
    }
    if ($candidates) {
        foreach ($sensor in $candidates) { Emit $sensor $namespace }
        break
    }
}
exit 0
"#
}

#[cfg(windows)]
fn external_cpu_temperature_script() -> &'static str {
    r#"
$ErrorActionPreference = 'SilentlyContinue'
$namespaces = @('root\LibreHardwareMonitor', 'root\OpenHardwareMonitor')
foreach ($namespace in $namespaces) {
    $sensors = Get-CimInstance -Namespace $namespace -ClassName Sensor -ErrorAction SilentlyContinue |
        Where-Object { $_.SensorType -eq 'Temperature' -and $null -ne $_.Value }
    if (-not $sensors) { continue }
    $candidates = $sensors | Where-Object {
        "$($_.Identifier) $($_.HardwareType) $($_.Parent) $($_.Name)" -match '(?i)(/cpu|intelcpu|amdcpu|cpu)'
    }
    $preferred = $candidates |
        Sort-Object @{ Expression = { if ($_.Name -match '(?i)(package|tctl|tdie|ccd|core max)') { 0 } else { 1 } } },
                    @{ Expression = { [double]$_.Value }; Descending = $true } |
        Select-Object -First 1
    if ($preferred) {
        "$([math]::Round([double]$preferred.Value, 1))`t$($preferred.Name)`t$namespace"
        break
    }
}
exit 0
"#
}

#[cfg(windows)]
fn query_gpu_utilization() -> Option<BridgeReading> {
    let script = r#"
$sample = Get-Counter '\GPU Engine(*)\Utilization Percentage' -ErrorAction Stop
if ($sample) {
    $sum = ($sample.CounterSamples | Measure-Object -Property CookedValue -Sum).Sum
    [math]::Round([math]::Min(100, [math]::Max(0, $sum)), 1)
}
"#;
    run_powershell(script)
        .ok()
        .and_then(|output| parse_first_utilization(&output))
        .map(|value| {
            BridgeReading::from_utilization(
                "GPU",
                value,
                "Windows GPU Engine counter",
                "GPU temperature unavailable; utilization is live",
            )
        })
}

#[cfg(windows)]
fn query_drive_temperature() -> Option<BridgeReading> {
    let script = r#"
$physical = Get-PhysicalDisk -ErrorAction Stop |
    Where-Object { $_.MediaType -ne 'Unspecified' } |
    Select-Object -First 1
if ($physical) {
    $counter = $physical | Get-StorageReliabilityCounter -ErrorAction Stop
    if ($counter -and $null -ne $counter.Temperature) {
        "$([math]::Round([double]$counter.Temperature, 1))`t$($physical.FriendlyName)"
    }
}
"#;
    run_slow_powershell(script).ok().and_then(|output| {
        let mut parts = output.trim().split('\t');
        let temperature = parts.next()?.trim().parse::<f32>().ok()?;
        let label = parts
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("SSD");
        Some(BridgeReading::from_temperature(
            label,
            temperature,
            "Windows Storage SMART",
        ))
    })
}

#[cfg(windows)]
fn query_memory_utilization() -> Option<BridgeReading> {
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe {
        GlobalMemoryStatusEx(&mut status).ok()?;
    }
    Some(BridgeReading::from_memory_utilization(
        status.dwMemoryLoad as f32,
    ))
}

#[cfg(windows)]
fn run_powershell(script: &str) -> Result<String, String> {
    run_command_timeout(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ],
        FAST_COMMAND_TIMEOUT,
    )
}

#[cfg(windows)]
fn sensor_driver_sys_path() -> Option<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(parent) = exe_path.parent() {
            roots.extend(parent.ancestors().map(PathBuf::from));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.extend(cwd.ancestors().map(PathBuf::from));
    }

    let relative_paths = [
        PathBuf::from("BenchScopeSensorDriver.sys"),
        PathBuf::from("sensor-driver/x64/Release/BenchScopeSensorDriver.sys"),
        PathBuf::from("sensor-driver/x64/Debug/BenchScopeSensorDriver.sys"),
        PathBuf::from(
            "sensor-driver/x64/Release/BenchScopeSensorDriver/BenchScopeSensorDriver.sys",
        ),
        PathBuf::from("sensor-driver/x64/Debug/BenchScopeSensorDriver/BenchScopeSensorDriver.sys"),
    ];

    roots
        .into_iter()
        .flat_map(|root| {
            relative_paths
                .iter()
                .map(move |relative| root.join(relative))
        })
        .find(|path| path.is_file())
}

#[cfg(windows)]
fn powershell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(windows)]
fn run_command(program: &str, args: &[&str]) -> Result<String, String> {
    run_command_timeout(program, args, FAST_COMMAND_TIMEOUT)
}

#[cfg(windows)]
fn run_slow_powershell(script: &str) -> Result<String, String> {
    run_command_timeout(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ],
        SLOW_COMMAND_TIMEOUT,
    )
}

#[cfg(windows)]
fn run_command_timeout(program: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut command = Command::new(program);
    command.args(args).creation_flags(CREATE_NO_WINDOW_RAW);
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start {program}: {error}"))?;

    if !wait_for_child(&mut child, timeout)? {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!(
            "{program} timed out after {} ms",
            timeout.as_millis()
        ));
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("failed to collect {program} output: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if stderr.is_empty() {
            format!("{program} failed")
        } else {
            format!("{program} failed: {stderr}")
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(windows)]
fn wait_for_child(child: &mut Child, timeout: Duration) -> Result<bool, String> {
    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|error| format!("failed to poll child process: {error}"))?
            .is_some()
        {
            return Ok(true);
        }
        if started.elapsed() >= timeout {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(windows)]
fn nvidia_smi_fallback_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(system_root) = std::env::var("SystemRoot") {
        paths.push(PathBuf::from(system_root).join("System32/nvidia-smi.exe"));
    }
    for key in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(program_files) = std::env::var(key) {
            paths
                .push(PathBuf::from(program_files).join("NVIDIA Corporation/NVSMI/nvidia-smi.exe"));
        }
    }
    paths
}

fn parse_metric_number(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.' || ch == '-'))
        .filter(|token| !token.is_empty())
        .filter_map(|token| token.parse::<f32>().ok())
        .find(|value| value.is_finite())
}

fn mib_to_gb(value: f32) -> f32 {
    round_tenth(value / 1024.0)
}

fn metric_value_is_plausible(kind: BridgeMetricKind, value: f32) -> bool {
    match kind {
        BridgeMetricKind::Temperature => (-40.0..=130.0).contains(&value),
        BridgeMetricKind::Utilization => (0.0..=10_000.0).contains(&value),
        BridgeMetricKind::MemoryUsage => (0.0..=1_000_000.0).contains(&value),
        BridgeMetricKind::Voltage => (0.0..=20.0).contains(&value),
        BridgeMetricKind::Power => (0.0..=2000.0).contains(&value),
        BridgeMetricKind::Clock => (0.0..=30_000.0).contains(&value),
    }
}

fn metric_value_for_kind(kind: BridgeMetricKind, value: f32) -> f32 {
    match kind {
        BridgeMetricKind::Utilization => clamp_percent(value),
        _ => round_tenth(value),
    }
}

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
fn cpu_model_name_for_bridge() -> Option<String> {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::__cpuid;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::__cpuid;

    let max_extended = __cpuid(0x8000_0000).eax;
    if max_extended < 0x8000_0004 {
        return None;
    }

    let mut bytes = Vec::with_capacity(48);
    for leaf in 0x8000_0002..=0x8000_0004 {
        let result = __cpuid(leaf);
        for register in [result.eax, result.ebx, result.ecx, result.edx] {
            bytes.extend_from_slice(&register.to_le_bytes());
        }
    }

    String::from_utf8(bytes)
        .ok()
        .map(|value| value.trim_matches('\0').trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(all(windows, not(any(target_arch = "x86", target_arch = "x86_64"))))]
fn cpu_model_name_for_bridge() -> Option<String> {
    None
}

fn parse_first_utilization(output: &str) -> Option<f32> {
    output
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.' || ch == '-'))
        .filter(|token| !token.is_empty())
        .filter_map(|token| token.parse::<f32>().ok())
        .find(|value| (0.0..=10_000.0).contains(value))
        .map(clamp_percent)
}

fn clamp_percent(value: f32) -> f32 {
    round_tenth(value.clamp(0.0, 100.0))
}

fn round_tenth(value: f32) -> f32 {
    (value * 10.0).round() / 10.0
}

#[allow(dead_code)]
fn reading_json(reading: &BenchScopeSensorReading) -> String {
    bridge_reading_json(&BridgeReading::from_driver(reading))
}

fn bridge_reading_json(reading: &BridgeReading) -> String {
    let temperature = reading
        .temperature_c
        .map(|value| format!("{value:.1}"))
        .unwrap_or_else(|| "null".to_owned());
    let utilization = reading
        .utilization_percent
        .map(|value| format!("{value:.1}"))
        .unwrap_or_else(|| "null".to_owned());
    let metrics = reading
        .metrics
        .iter()
        .filter(|metric| metric.has_value())
        .map(bridge_metric_json)
        .collect::<Vec<_>>()
        .join(",");
    let mut fields = vec![
        format!("\"label\":\"{}\"", json_escape(&reading.label)),
        format!("\"temperatureC\":{temperature}"),
        format!("\"utilizationPercent\":{utilization}"),
        format!("\"metrics\":[{metrics}]"),
        format!("\"provider\":\"{}\"", json_escape(&reading.provider)),
        format!("\"status\":\"{}\"", json_escape(&reading.status)),
    ];
    if let Some(message) = &reading.message {
        fields.push(format!("\"message\":\"{}\"", json_escape(message)));
    }
    format!("{{{}}}", fields.join(","))
}

fn bridge_metric_json(metric: &BridgeMetric) -> String {
    format!(
        "{{\"kind\":\"{}\",\"label\":\"{}\",\"value\":{},\"min\":{},\"max\":{}}}",
        metric.kind.json_key(),
        json_escape(&metric.label),
        json_optional_f32(metric.value),
        json_optional_f32(metric.min),
        json_optional_f32(metric.max),
    )
}

#[cfg(windows)]
fn fallback_snapshot_json(driver_error: &str, readings: BridgeReadings) -> String {
    let message = json_escape(driver_error);
    let fields = [
        format!("\"timestampUtc\":\"{}\"", json_escape(&timestamp_label())),
        "\"isElevated\":false".to_owned(),
        "\"source\":\"BenchScopeSensorService\"".to_owned(),
        format!(
            "\"driver\":{{\"available\":false,\"error\":\"{}\"}}",
            message
        ),
        format!("\"cpu\":{}", bridge_reading_json(&readings.cpu)),
        format!("\"gpu\":{}", bridge_reading_json(&readings.gpu)),
        format!(
            "\"gpuMemory\":{}",
            bridge_reading_json(&readings.gpu_memory)
        ),
        format!("\"drive\":{}", bridge_reading_json(&readings.drive)),
        format!("\"memory\":{}", bridge_reading_json(&readings.memory)),
        format!("\"diagnostics\":[\"{}\"]", message),
    ];
    format!("{{{}}}", fields.join(","))
}

#[cfg(not(windows))]
fn error_snapshot_json(error: &str) -> String {
    let status = if error.to_ascii_lowercase().contains("access is denied") {
        "permissionDenied"
    } else {
        "unavailable"
    };
    let provider = "BenchScope sensor driver bridge";
    let message = json_escape(error);
    format!(
        "{{\"timestampUtc\":\"{}\",\"isElevated\":false,\"source\":\"BenchScopeSensorService\",\"cpu\":{},\"gpu\":{},\"gpuMemory\":{},\"drive\":{},\"memory\":{},\"diagnostics\":[\"{}\"]}}",
        json_escape(&timestamp_label()),
        unavailable_reading_json("CPU", provider, status, &message),
        unavailable_reading_json("GPU", provider, status, &message),
        unavailable_reading_json("VRAM", provider, status, &message),
        unavailable_reading_json("SSD", provider, status, &message),
        unavailable_reading_json("RAM", provider, status, &message),
        message,
    )
}

#[cfg(not(windows))]
fn unavailable_reading_json(label: &str, provider: &str, status: &str, message: &str) -> String {
    format!(
        "{{\"label\":\"{}\",\"temperatureC\":null,\"utilizationPercent\":null,\"provider\":\"{}\",\"status\":\"{}\",\"message\":\"{}\"}}",
        json_escape(label),
        json_escape(provider),
        status,
        message,
    )
}

fn json_bool(value: u32) -> &'static str {
    if value == 0 { "false" } else { "true" }
}

fn json_bool_from_bool(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn json_optional_string(value: Option<&str>) -> String {
    value
        .map(|value| format!("\"{}\"", json_escape(value)))
        .unwrap_or_else(|| "null".to_owned())
}

fn json_optional_f32(value: Option<f32>) -> String {
    value
        .filter(|value| value.is_finite())
        .map(|value| format!("{value:.3}"))
        .unwrap_or_else(|| "null".to_owned())
}

fn join_nonempty<'a, const N: usize>(
    values: [Option<&'a str>; N],
    separator: &str,
) -> Option<String> {
    let parts = values
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join(separator))
}

fn timestamp_label() -> String {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => format!("unix-ms:{}", duration.as_millis()),
        Err(_) => "unix-ms:0".to_owned(),
    }
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => escaped.push(' '),
            ch => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn driver_auto_start_skips_permission_errors() {
        assert!(!driver_open_error_can_auto_start(
            "failed to open \\\\.\\BenchScopeSensor: Access is denied. (os error 5)"
        ));
        assert!(driver_open_error_can_auto_start(
            "failed to open \\\\.\\BenchScopeSensor: The system cannot find the file specified. (os error 2)"
        ));
    }

    #[test]
    fn motherboard_identification_parser_reads_board_and_controller_hints() {
        let output = concat!(
            "BOARD\tASUSTeK COMPUTER INC.\tROG TEST BOARD\tRev 1.xx\tDefault string\tAmerican Megatrends\t1801\t20260501000000.000000+000\tASUS\tSystem Product Name\n",
            "CTRL\tIntel(R) SMBus - 7AA3\tIntel\tPCI\\VEN_8086&DEV_7AA3\n",
            "CTRL\tIntel(R) LPC/eSPI Controller\tIntel\tPCI\\VEN_8086&DEV_7A86\n"
        );

        let parsed = parse_motherboard_identification(output).unwrap();

        assert_eq!(
            parsed.baseboard_manufacturer.as_deref(),
            Some("ASUSTeK COMPUTER INC.")
        );
        assert_eq!(parsed.baseboard_product.as_deref(), Some("ROG TEST BOARD"));
        assert_eq!(parsed.baseboard_serial, None);
        assert_eq!(parsed.controller_hints.len(), 2);
        assert!(parsed.detail().contains("Super I/O chip ID is not exposed"));
    }

    #[test]
    fn fallback_snapshot_can_include_safe_temperature_readings() {
        let readings = BridgeReadings {
            cpu: BridgeReading::from_temperature("CPU Package", 63.5, "External hardware WMI"),
            gpu: BridgeReading::from_temperature("GPU Core", 57.0, "NVML/nvidia-smi"),
            gpu_memory: nvidia_smi_gpu_memory_reading("GeForce RTX Test, 86, 6144, 12288").unwrap(),
            drive: BridgeReading::unavailable(
                "SSD",
                "BenchScope sensor driver bridge",
                "unavailable",
                None,
            ),
            memory: BridgeReading::from_memory_utilization(42.0),
        };

        let json = fallback_snapshot_json("driver unavailable", readings);

        assert!(json.contains("\"driver\":{\"available\":false"));
        assert!(json.contains("\"cpu\":{\"label\":\"CPU Package\",\"temperatureC\":63.5"));
        assert!(json.contains("\"gpu\":{\"label\":\"GPU Core\",\"temperatureC\":57.0"));
        assert!(json.contains("\"gpuMemory\":{\"label\":\"VRAM\",\"temperatureC\":86.0"));
        assert!(json.contains("\"kind\":\"memoryUsage\",\"label\":\"VRAM Used\",\"value\":6.0"));
        assert!(json.contains("\"memory\":{\"label\":\"System RAM\""));
    }
}
