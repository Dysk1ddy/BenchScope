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
const FAST_PROVIDER_TTL: Duration = Duration::from_millis(900);
const GPU_TEMP_TTL: Duration = Duration::from_secs(5);
const DRIVE_TEMP_TTL: Duration = Duration::from_secs(15);
const STALE_PROVIDER_AFTER: Duration = Duration::from_secs(45);
const FAST_COMMAND_TIMEOUT: Duration = Duration::from_millis(1_800);
const SLOW_COMMAND_TIMEOUT: Duration = Duration::from_millis(4_500);
const ASYNC_PROVIDER_ABANDON_AFTER: Duration = Duration::from_secs(6);
const CPU_INITIAL_SAMPLE_WINDOW: Duration = Duration::from_millis(120);
#[cfg(windows)]
const DRIVER_SERVICE_NAME: &str = "BenchScopeSensorDriver";
#[cfg(windows)]
const DRIVER_START_RETRY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
struct BridgeReading {
    label: String,
    temperature_c: Option<f32>,
    utilization_percent: Option<f32>,
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
            provider: provider.to_owned(),
            status: status.to_owned(),
            message,
        }
    }

    fn from_driver(reading: &BenchScopeSensorReading) -> Self {
        Self {
            label: wide_to_string(&reading.label),
            temperature_c: (reading.flags & BENCHSCOPE_SENSOR_READING_HAS_TEMPERATURE != 0)
                .then_some(reading.temperature_milli_c as f32 / 1000.0),
            utilization_percent: (reading.flags & BENCHSCOPE_SENSOR_READING_HAS_UTILIZATION != 0)
                .then_some(reading.utilization_milli_percent as f32 / 1000.0),
            provider: wide_to_string(&reading.provider),
            status: status_json_value(status_from_driver(reading.status)).to_owned(),
            message: None,
        }
    }

    fn from_temperature(label: &str, value: f32, provider: &str) -> Self {
        Self {
            label: label.to_owned(),
            temperature_c: Some(round_tenth(value)),
            utilization_percent: None,
            provider: provider.to_owned(),
            status: "ok".to_owned(),
            message: None,
        }
    }

    fn from_utilization(label: &str, value: f32, provider: &str, partial_message: &str) -> Self {
        Self {
            label: label.to_owned(),
            temperature_c: None,
            utilization_percent: Some(clamp_percent(value)),
            provider: provider.to_owned(),
            status: "partial".to_owned(),
            message: Some(partial_message.to_owned()),
        }
    }

    fn from_memory_utilization(value: f32) -> Self {
        Self {
            label: "System RAM".to_owned(),
            temperature_c: None,
            utilization_percent: Some(clamp_percent(value)),
            provider: "Windows memory status".to_owned(),
            status: "ok".to_owned(),
            message: None,
        }
    }

    fn merge_safe_provider(&mut self, safe: BridgeReading) {
        let mut changed = false;
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
    cpu_utilization: CachedReading,
    gpu_temperature: AsyncCachedReading,
    gpu_utilization: AsyncCachedReading,
    drive_temperature: AsyncCachedReading,
    memory_utilization: CachedReading,
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
            cpu_utilization: CachedReading::new(FAST_PROVIDER_TTL),
            gpu_temperature: AsyncCachedReading::new(GPU_TEMP_TTL),
            gpu_utilization: AsyncCachedReading::new(FAST_PROVIDER_TTL),
            drive_temperature: AsyncCachedReading::new(DRIVE_TEMP_TTL),
            memory_utilization: CachedReading::new(FAST_PROVIDER_TTL),
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
                error_snapshot_json(&error)
            }
        }
    }

    fn warm_up_slow_providers(&mut self, max_wait: Duration) {
        let started = Instant::now();
        let _ = self.driver_snapshot_json();

        while started.elapsed() < max_wait {
            let now = Instant::now();
            self.gpu_temperature.collect_finished(now);
            self.gpu_utilization.collect_finished(now);
            self.drive_temperature.collect_finished(now);

            if !self.gpu_temperature.is_pending()
                && !self.gpu_utilization.is_pending()
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
        if let Some(advanced) = &advanced {
            apply_advanced_telemetry_to_readings(&mut readings, advanced);
        }

        let now = Instant::now();
        let query_cpu_utilization_now = !self.cpu_utilization.is_fresh(now);
        let query_memory_utilization_now = !self.memory_utilization.is_fresh(now);

        let next_cpu_utilization =
            query_cpu_utilization_now.then(|| self.cpu_sampler.query_utilization());
        let next_memory_utilization = query_memory_utilization_now.then(query_memory_utilization);

        let cpu_utilization = self
            .cpu_utilization
            .resolve_query(now, next_cpu_utilization);
        let gpu_temperature = self
            .gpu_temperature
            .resolve_query(now, query_gpu_temperature);
        let gpu_utilization = self
            .gpu_utilization
            .resolve_query(now, query_gpu_utilization);
        let drive_temperature = self
            .drive_temperature
            .resolve_query(now, query_drive_temperature);
        let memory_utilization = self
            .memory_utilization
            .resolve_query(now, next_memory_utilization);

        if let Some(reading) = cpu_utilization {
            readings.cpu.merge_safe_provider(reading);
        }
        if let Some(reading) = gpu_temperature {
            readings.gpu.merge_safe_provider(reading);
        }
        if let Some(reading) = gpu_utilization {
            readings.gpu.merge_safe_provider(reading);
        }
        if let Some(reading) = drive_temperature {
            readings.drive.merge_safe_provider(reading);
        }
        if let Some(reading) = memory_utilization {
            readings.memory.merge_safe_provider(reading);
        }

        Ok(snapshot_json_from_parts(
            version,
            capabilities,
            advanced.as_ref(),
            self.motherboard_identification.as_ref(),
            readings,
        ))
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
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
$service = Get-Service -Name '{DRIVER_SERVICE_NAME}' -ErrorAction Stop
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
fn query_gpu_temperature() -> Option<BridgeReading> {
    const NVIDIA_SMI_ARGS: &[&str] = &[
        "--query-gpu=temperature.gpu,name",
        "--format=csv,noheader,nounits",
    ];
    run_command("nvidia-smi", NVIDIA_SMI_ARGS)
        .ok()
        .or_else(|| {
            nvidia_smi_fallback_paths()
                .into_iter()
                .find(|path| path.is_file())
                .and_then(|path| run_command(&path.display().to_string(), NVIDIA_SMI_ARGS).ok())
        })
        .and_then(|output| {
            let temperature = parse_first_temperature(&output)?;
            let label = output
                .lines()
                .next()
                .and_then(|line| line.split_once(',').map(|(_, name)| name.trim()))
                .filter(|name| !name.is_empty())
                .unwrap_or("GPU");
            Some(BridgeReading::from_temperature(
                label,
                temperature,
                "NVML/nvidia-smi",
            ))
        })
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

fn parse_first_temperature(output: &str) -> Option<f32> {
    output
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.' || ch == '-'))
        .filter(|token| !token.is_empty())
        .filter_map(|token| token.parse::<f32>().ok())
        .find(|value| (-40.0..=130.0).contains(value))
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
    let mut fields = vec![
        format!("\"label\":\"{}\"", json_escape(&reading.label)),
        format!("\"temperatureC\":{temperature}"),
        format!("\"utilizationPercent\":{utilization}"),
        format!("\"provider\":\"{}\"", json_escape(&reading.provider)),
        format!("\"status\":\"{}\"", json_escape(&reading.status)),
    ];
    if let Some(message) = &reading.message {
        fields.push(format!("\"message\":\"{}\"", json_escape(message)));
    }
    format!("{{{}}}", fields.join(","))
}

fn error_snapshot_json(error: &str) -> String {
    let status = if error.to_ascii_lowercase().contains("access is denied") {
        "permissionDenied"
    } else {
        "unavailable"
    };
    let provider = "BenchScope sensor driver bridge";
    let message = json_escape(error);
    format!(
        "{{\"timestampUtc\":\"{}\",\"isElevated\":false,\"source\":\"BenchScopeSensorService\",\"cpu\":{},\"gpu\":{},\"drive\":{},\"memory\":{},\"diagnostics\":[\"{}\"]}}",
        json_escape(&timestamp_label()),
        unavailable_reading_json("CPU", provider, status, &message),
        unavailable_reading_json("GPU", provider, status, &message),
        unavailable_reading_json("SSD", provider, status, &message),
        unavailable_reading_json("RAM", provider, status, &message),
        message,
    )
}

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
}
