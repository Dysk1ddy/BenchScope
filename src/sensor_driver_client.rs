#![allow(dead_code)]

#[cfg(windows)]
use std::{
    ffi::{OsStr, c_void},
    mem::{size_of, zeroed},
    os::windows::ffi::OsStrExt,
    ptr::null_mut,
};

#[cfg(windows)]
type Handle = *mut c_void;

#[cfg(windows)]
const INVALID_HANDLE_VALUE: Handle = (-1isize) as Handle;
#[cfg(windows)]
const GENERIC_READ: u32 = 0x8000_0000;
#[cfg(windows)]
const FILE_SHARE_READ: u32 = 0x0000_0001;
#[cfg(windows)]
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
#[cfg(windows)]
const OPEN_EXISTING: u32 = 3;
#[cfg(windows)]
const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;

const METHOD_BUFFERED: u32 = 0;
const FILE_READ_DATA: u32 = 0x0001;
const FILE_DEVICE_BENCHSCOPE_SENSOR: u32 = 0x8337;

pub const IOCTL_BENCHSCOPE_SENSOR_GET_VERSION: u32 = ctl_code(0x801);
pub const IOCTL_BENCHSCOPE_SENSOR_GET_CAPABILITIES: u32 = ctl_code(0x802);
pub const IOCTL_BENCHSCOPE_SENSOR_GET_SNAPSHOT: u32 = ctl_code(0x803);
pub const IOCTL_BENCHSCOPE_SENSOR_GET_ADVANCED_TELEMETRY: u32 = ctl_code(0x804);

pub const BENCHSCOPE_SENSOR_READING_HAS_TEMPERATURE: u32 = 0x0000_0001;
pub const BENCHSCOPE_SENSOR_READING_HAS_UTILIZATION: u32 = 0x0000_0002;
pub const BENCHSCOPE_SENSOR_ADVANCED_HAS_TEMPERATURE: u32 = 0x0000_0001;
pub const BENCHSCOPE_SENSOR_ADVANCED_HAS_THERMAL_LIMIT: u32 = 0x0000_0002;
pub const BENCHSCOPE_SENSOR_ADVANCED_THERMAL_THROTTLED: u32 = 0x0000_0004;
pub const BENCHSCOPE_SENSOR_ADVANCED_HAS_ENERGY: u32 = 0x0000_0008;
pub const BENCHSCOPE_SENSOR_ADVANCED_USER_MODE_PROVIDER: u32 = 0x8000_0000;

const fn ctl_code(function: u32) -> u32 {
    (FILE_DEVICE_BENCHSCOPE_SENSOR << 16)
        | (FILE_READ_DATA << 14)
        | (function << 2)
        | METHOD_BUFFERED
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BenchScopeSensorVersion {
    pub protocol_version: u32,
    pub driver_major: u32,
    pub driver_minor: u32,
    pub driver_patch: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BenchScopeSensorCapabilities {
    pub protocol_version: u32,
    pub supports_cpu_temperature: u32,
    pub supports_gpu_temperature: u32,
    pub supports_drive_temperature: u32,
    pub supports_utilization: u32,
    pub reserved: [u32; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BenchScopeSensorReading {
    pub kind: u32,
    pub status: u32,
    pub flags: u32,
    pub temperature_milli_c: i32,
    pub utilization_milli_percent: i32,
    pub label: [u16; 64],
    pub provider: [u16; 64],
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BenchScopeSensorSnapshot {
    pub protocol_version: u32,
    pub reading_count: u32,
    pub sequence: u64,
    pub timestamp_qpc: i64,
    pub readings: [BenchScopeSensorReading; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BenchScopeSensorAdvancedReading {
    pub kind: u32,
    pub status: u32,
    pub flags: u32,
    pub temperature_milli_c: i32,
    pub thermal_limit_milli_c: i32,
    pub utilization_milli_percent: i32,
    pub power_milli_watts: i32,
    pub energy_milli_joules: u64,
    pub fan_rpm: u32,
    pub voltage_milli_v: i32,
    pub label: [u16; 64],
    pub provider: [u16; 64],
    pub detail: [u16; 128],
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BenchScopeSensorAdvancedTelemetry {
    pub protocol_version: u32,
    pub provider_mask: u32,
    pub reading_count: u32,
    pub reserved: u32,
    pub sequence: u64,
    pub timestamp_qpc: i64,
    pub readings: [BenchScopeSensorAdvancedReading; 8],
}

#[derive(Clone, Copy, Debug)]
pub enum SensorBridgeKind {
    Cpu,
    Gpu,
    Drive,
    Memory,
    Motherboard,
    Fan,
    Voltage,
    StorageHealth,
    Unknown,
}

#[derive(Clone, Copy, Debug)]
pub enum SensorBridgeStatus {
    Ok,
    Unsupported,
    PermissionDenied,
    Unavailable,
    Error,
    Unknown,
}

#[cfg(windows)]
pub struct DeviceHandle(Handle);

#[cfg(windows)]
impl DeviceHandle {
    pub fn open_default() -> Result<Self, String> {
        Self::open(r"\\.\BenchScopeSensor")
    }

    pub fn open(path: &str) -> Result<Self, String> {
        let path_wide: Vec<u16> = OsStr::new(path).encode_wide().chain(Some(0)).collect();
        let handle = unsafe {
            CreateFileW(
                path_wide.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null_mut(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        };

        if handle == INVALID_HANDLE_VALUE {
            return Err(format!(
                "failed to open {path}: {}",
                std::io::Error::last_os_error()
            ));
        }

        Ok(Self(handle))
    }

    pub fn version(&self) -> Result<BenchScopeSensorVersion, String> {
        self.ioctl_out(IOCTL_BENCHSCOPE_SENSOR_GET_VERSION)
    }

    pub fn capabilities(&self) -> Result<BenchScopeSensorCapabilities, String> {
        self.ioctl_out(IOCTL_BENCHSCOPE_SENSOR_GET_CAPABILITIES)
    }

    pub fn snapshot(&self) -> Result<BenchScopeSensorSnapshot, String> {
        self.ioctl_out(IOCTL_BENCHSCOPE_SENSOR_GET_SNAPSHOT)
    }

    pub fn advanced_telemetry(&self) -> Result<BenchScopeSensorAdvancedTelemetry, String> {
        self.ioctl_out(IOCTL_BENCHSCOPE_SENSOR_GET_ADVANCED_TELEMETRY)
    }

    pub fn ioctl_out<T: Copy>(&self, code: u32) -> Result<T, String> {
        let mut output: T = unsafe { zeroed() };
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                self.0,
                code,
                null_mut(),
                0,
                (&mut output as *mut T).cast::<c_void>(),
                size_of::<T>() as u32,
                &mut returned,
                null_mut(),
            )
        };

        if ok == 0 {
            return Err(format!(
                "DeviceIoControl 0x{code:08x} failed: {}",
                std::io::Error::last_os_error()
            ));
        }

        if returned as usize != size_of::<T>() {
            return Err(format!(
                "DeviceIoControl 0x{code:08x} returned {returned} bytes, expected {}",
                size_of::<T>()
            ));
        }

        Ok(output)
    }
}

#[cfg(windows)]
impl Drop for DeviceHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(not(windows))]
pub struct DeviceHandle;

#[cfg(not(windows))]
impl DeviceHandle {
    pub fn open_default() -> Result<Self, String> {
        Err("BenchScope sensor driver is only available on Windows".to_owned())
    }
}

pub fn kind_from_driver(value: u32) -> SensorBridgeKind {
    match value {
        1 => SensorBridgeKind::Cpu,
        2 => SensorBridgeKind::Gpu,
        3 => SensorBridgeKind::Drive,
        4 => SensorBridgeKind::Memory,
        5 => SensorBridgeKind::Motherboard,
        6 => SensorBridgeKind::Fan,
        7 => SensorBridgeKind::Voltage,
        8 => SensorBridgeKind::StorageHealth,
        _ => SensorBridgeKind::Unknown,
    }
}

pub fn status_from_driver(value: u32) -> SensorBridgeStatus {
    match value {
        0 => SensorBridgeStatus::Ok,
        1 => SensorBridgeStatus::Unsupported,
        2 => SensorBridgeStatus::PermissionDenied,
        3 => SensorBridgeStatus::Unavailable,
        4 => SensorBridgeStatus::Error,
        _ => SensorBridgeStatus::Unknown,
    }
}

#[allow(dead_code)]
pub fn kind_json_key(kind: SensorBridgeKind) -> &'static str {
    match kind {
        SensorBridgeKind::Cpu => "cpu",
        SensorBridgeKind::Gpu => "gpu",
        SensorBridgeKind::Drive => "drive",
        SensorBridgeKind::Memory => "memory",
        SensorBridgeKind::Motherboard => "motherboard",
        SensorBridgeKind::Fan => "fan",
        SensorBridgeKind::Voltage => "voltage",
        SensorBridgeKind::StorageHealth => "storageHealth",
        SensorBridgeKind::Unknown => "unknown",
    }
}

pub fn status_json_value(status: SensorBridgeStatus) -> &'static str {
    match status {
        SensorBridgeStatus::Ok => "ok",
        SensorBridgeStatus::Unsupported => "unsupported",
        SensorBridgeStatus::PermissionDenied => "permissionDenied",
        SensorBridgeStatus::Unavailable => "unavailable",
        SensorBridgeStatus::Error => "error",
        SensorBridgeStatus::Unknown => "error",
    }
}

pub fn wide_to_string(value: &[u16]) -> String {
    let end = value
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(value.len());
    String::from_utf16_lossy(&value[..end])
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        lp_file_name: *const u16,
        dw_desired_access: u32,
        dw_share_mode: u32,
        lp_security_attributes: *mut c_void,
        dw_creation_disposition: u32,
        dw_flags_and_attributes: u32,
        h_template_file: Handle,
    ) -> Handle;

    fn DeviceIoControl(
        h_device: Handle,
        dw_io_control_code: u32,
        lp_in_buffer: *mut c_void,
        n_in_buffer_size: u32,
        lp_out_buffer: *mut c_void,
        n_out_buffer_size: u32,
        lp_bytes_returned: *mut u32,
        lp_overlapped: *mut c_void,
    ) -> i32;

    fn CloseHandle(h_object: Handle) -> i32;
}
