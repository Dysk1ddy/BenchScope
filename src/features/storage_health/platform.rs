fn query_storage_health_snapshot(drive: &DriveInfo) -> Result<StorageHealthSnapshot> {
    #[cfg(windows)]
    {
        let Some(letter) = drive_letter_for_path(&drive.root) else {
            return Ok(StorageHealthSnapshot::unknown(
                drive,
                "Could not map the selected target to a Windows drive letter",
            ));
        };
        let script = windows_storage_health_script(letter);
        match run_powershell_sensor_script(&script) {
            Ok(output) => {
                let snapshot = parse_windows_storage_health_output(drive, &output);
                Ok(merge_direct_nvme_health_log(drive, snapshot, &output))
            }
            Err(err) => {
                let mut snapshot = StorageHealthSnapshot::unknown(
                    drive,
                    format!("Windows storage health query failed: {err:#}"),
                );
                let drive_sensor = query_drive_temperature(Some(letter));
                snapshot.temperature_c = sensor_temperature(Some(&drive_sensor));
                snapshot.utilization_percent = query_drive_utilization(Some(letter));
                finalize_storage_health_snapshot(snapshot)
            }
        }
    }

    #[cfg(not(windows))]
    {
        Ok(StorageHealthSnapshot::unknown(
            drive,
            "Storage health collection is currently implemented for Windows providers",
        ))
    }
}

#[cfg(windows)]
fn windows_storage_health_script(drive_letter: char) -> String {
    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
$notes = New-Object System.Collections.Generic.List[string]
function Add-Note($message) {
    if ($message) {
        $script:notes.Add((($message -replace "`t", " ") -replace "`r?`n", " "))
    }
}
function Safe($name, [scriptblock]$block) {
    try {
        & $block
    } catch {
        Add-Note "$name failed: $($_.Exception.Message)"
        $null
    }
}
function Value($object, $name) {
    if ($null -eq $object) { return '' }
    $property = $object.PSObject.Properties[$name]
    if ($null -eq $property -or $null -eq $property.Value) { return '' }
    (($property.Value -join ', ') -replace "`t", " ") -replace "`r?`n", " "
}
function First-Value($values) {
    foreach ($value in $values) {
        if ($null -ne $value -and "$value".Trim().Length -gt 0) {
            return "$value"
        }
    }
    ''
}
function Emit($values) {
    [Console]::Out.WriteLine([string]::Join("`t", $values))
}

$letter = '__DRIVE_LETTER__'
$logical = Safe 'Win32_LogicalDisk' { Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='$($letter):'" | Select-Object -First 1 }
$partition = Safe 'Get-Partition' { Get-Partition -DriveLetter $letter -ErrorAction Stop | Select-Object -First 1 }
$disk = $null
if ($partition) {
    $disk = Safe 'Get-Disk' { $partition | Get-Disk -ErrorAction Stop | Select-Object -First 1 }
}
$physical = $null
if ($disk) {
    $physical = Safe 'Get-PhysicalDisk' {
        Get-PhysicalDisk -ErrorAction Stop |
            Where-Object {
                $_.DeviceId -eq "$($disk.Number)" -or
                ($disk.SerialNumber -and $_.SerialNumber -eq $disk.SerialNumber) -or
                ($disk.FriendlyName -and $_.FriendlyName -eq $disk.FriendlyName)
            } |
            Select-Object -First 1
    }
}
$counter = $null
if ($physical) {
    $counter = Safe 'Get-StorageReliabilityCounter' { $physical | Get-StorageReliabilityCounter -ErrorAction Stop }
}
$drive = $null
if ($disk) {
    $drive = Safe 'Win32_DiskDrive' {
        Get-CimInstance Win32_DiskDrive |
            Where-Object { $_.Index -eq $disk.Number } |
            Select-Object -First 1
    }
}

$physicalDriveNumber = First-Value @((Value $disk 'Number'), (Value $drive 'Index'))
if ($physicalDriveNumber) {
    Emit @('DISK', $physicalDriveNumber)
}

$model = First-Value @((Value $physical 'FriendlyName'), (Value $disk 'FriendlyName'), (Value $drive 'Model'))
$serial = First-Value @((Value $physical 'SerialNumber'), (Value $disk 'SerialNumber'), (Value $drive 'SerialNumber'))
$firmware = First-Value @((Value $physical 'FirmwareVersion'), (Value $drive 'FirmwareRevision'))
$busType = First-Value @((Value $disk 'BusType'), (Value $physical 'BusType'), (Value $drive 'InterfaceType'))
$mediaType = First-Value @((Value $physical 'MediaType'), (Value $drive 'MediaType'))
$capacity = First-Value @((Value $disk 'Size'), (Value $physical 'Size'), (Value $logical 'Size'))
$free = Value $logical 'FreeSpace'
$filesystem = Value $logical 'FileSystem'
$health = First-Value @((Value $physical 'HealthStatus'), (Value $disk 'HealthStatus'), (Value $drive 'Status'))
$operational = First-Value @((Value $physical 'OperationalStatus'), (Value $disk 'OperationalStatus'))

Emit @(
    'HEALTH',
    $letter,
    $model,
    $serial,
    $firmware,
    $busType,
    $mediaType,
    $capacity,
    $free,
    $filesystem,
    $health,
    $operational,
    '',
    (Value $counter 'Temperature'),
    (Value $counter 'Wear'),
    (Value $counter 'PowerOnHours'),
    (First-Value @((Value $counter 'PowerCycleCount'), (Value $counter 'StartStopCycleCount'))),
    (Value $counter 'ReadErrorsTotal'),
    (Value $counter 'WriteErrorsTotal'),
    (Value $counter 'ReadErrorsUncorrected'),
    (Value $counter 'WriteErrorsUncorrected'),
    (First-Value @((Value $counter 'MediaErrors'), (Value $counter 'MediaAndDataIntegrityErrors'))),
    (First-Value @((Value $counter 'DataUnitsRead'), (Value $counter 'BytesRead'))),
    (First-Value @((Value $counter 'DataUnitsWritten'), (Value $counter 'BytesWritten'))),
    ($notes -join ' | ')
)

if ($counter) {
    Emit @(
        'NVME',
        (First-Value @((Value $counter 'AvailableSpare'), (Value $counter 'AvailableSparePercent'))),
        (First-Value @((Value $counter 'AvailableSpareThreshold'), (Value $counter 'AvailableSpareThresholdPercent'))),
        (First-Value @((Value $counter 'CriticalWarning'), (Value $counter 'CriticalWarnings'))),
        (First-Value @((Value $counter 'UnsafeShutdowns'), (Value $counter 'UnsafeShutdownCount'))),
        (First-Value @((Value $counter 'ControllerBusyTime'), (Value $counter 'ControllerBusyTimeMinutes'))),
        (Value $counter 'HostReadCommands'),
        (Value $counter 'HostWriteCommands'),
        (First-Value @((Value $counter 'WarningCompositeTemperatureTime'), (Value $counter 'WarningTemperatureTime'))),
        (First-Value @((Value $counter 'CriticalCompositeTemperatureTime'), (Value $counter 'CriticalTemperatureTime'))),
        (Value $counter 'ThermalManagementTemperature1TransitionCount'),
        (Value $counter 'ThermalManagementTemperature2TransitionCount'),
        (Value $counter 'TemperatureSensor1'),
        (Value $counter 'TemperatureSensor2'),
        (Value $counter 'TemperatureSensor3'),
        (Value $counter 'TemperatureSensor4'),
        (Value $counter 'TemperatureSensor5'),
        (Value $counter 'TemperatureSensor6'),
        (Value $counter 'TemperatureSensor7'),
        (Value $counter 'TemperatureSensor8')
    )
}

$statusItems = Safe 'MSStorageDriver_FailurePredictStatus' {
    Get-CimInstance -Namespace root\wmi -ClassName MSStorageDriver_FailurePredictStatus
}
if ($statusItems) {
    foreach ($item in $statusItems) {
        Emit @('SMARTSTATUS', (Value $item 'InstanceName'), (Value $item 'PredictFailure'), (Value $item 'Reason'))
    }
}

$thresholdItems = Safe 'MSStorageDriver_FailurePredictThresholds' {
    Get-CimInstance -Namespace root\wmi -ClassName MSStorageDriver_FailurePredictThresholds
}
$dataItems = Safe 'MSStorageDriver_FailurePredictData' {
    Get-CimInstance -Namespace root\wmi -ClassName MSStorageDriver_FailurePredictData
}
if ($dataItems) {
    foreach ($item in $dataItems) {
        $bytes = $item.VendorSpecific
        if (-not $bytes) { continue }
        $thresholdBytes = $null
        if ($thresholdItems) {
            $threshold = $thresholdItems |
                Where-Object { $_.InstanceName -eq $item.InstanceName } |
                Select-Object -First 1
            if ($threshold) { $thresholdBytes = $threshold.VendorSpecific }
        }
        for ($i = 2; $i -lt 362 -and ($i + 11) -lt $bytes.Length; $i += 12) {
            $id = [int]$bytes[$i]
            if ($id -eq 0) { continue }
            $current = [int]$bytes[$i + 3]
            $worst = [int]$bytes[$i + 4]
            $raw = [uint64]0
            for ($b = 0; $b -lt 6; $b++) {
                $raw = $raw + (([uint64]$bytes[$i + 5 + $b]) -shl (8 * $b))
            }
            $thresholdValue = ''
            if ($thresholdBytes -and ($i + 1) -lt $thresholdBytes.Length -and [int]$thresholdBytes[$i] -eq $id) {
                $thresholdValue = [int]$thresholdBytes[$i + 1]
            }
            Emit @('SMART', (Value $item 'InstanceName'), $id, $current, $worst, $thresholdValue, $raw)
        }
    }
}
"#;
    script.replace("__DRIVE_LETTER__", &drive_letter.to_string())
}

#[derive(Clone, Debug)]
struct RawSmartAttribute {
    instance: String,
    id: u16,
    current: Option<u64>,
    worst: Option<u64>,
    threshold: Option<u64>,
    raw: Option<u64>,
}

#[derive(Clone, Debug)]
struct NvmeHealthLog {
    critical_warning_flags: u64,
    temperature_c: Option<f32>,
    available_spare_percent: u64,
    available_spare_threshold_percent: u64,
    percentage_used: u64,
    data_read_bytes: u64,
    data_written_bytes: u64,
    host_read_commands: u64,
    host_write_commands: u64,
    controller_busy_time_minutes: u64,
    power_cycle_count: u64,
    power_on_hours: u64,
    unsafe_shutdowns: u64,
    media_errors: u64,
    error_info_log_entries: u64,
    warning_temperature_time_minutes: u64,
    critical_temperature_time_minutes: u64,
    temperature_sensors_c: [Option<f32>; 8],
}

#[cfg(windows)]
fn parse_windows_storage_health_output(drive: &DriveInfo, output: &str) -> StorageHealthSnapshot {
    let mut snapshot =
        StorageHealthSnapshot::unknown(drive, "Windows storage providers returned limited data");
    let mut raw_smart = Vec::new();
    let mut smart_statuses: Vec<(String, bool)> = Vec::new();

    for line in output.lines() {
        let columns = line.split('\t').collect::<Vec<_>>();
        match columns.first().copied() {
            Some("HEALTH") => {
                snapshot.model = clean_storage_text(columns.get(2).copied())
                    .or_else(|| drive.device_name.clone())
                    .unwrap_or_else(|| "Unknown drive".to_owned());
                snapshot.serial = clean_storage_text(columns.get(3).copied());
                snapshot.firmware = clean_storage_text(columns.get(4).copied());
                snapshot.bus_type = clean_storage_text(columns.get(5).copied())
                    .unwrap_or_else(|| "Unknown".to_owned());
                snapshot.media_type = clean_storage_text(columns.get(6).copied())
                    .unwrap_or_else(|| "Unknown".to_owned());
                snapshot.capacity_bytes = parse_optional_u64(columns.get(7).copied());
                snapshot.free_bytes = parse_optional_u64(columns.get(8).copied());
                snapshot.file_system = clean_storage_text(columns.get(9).copied());
                snapshot.health_status_text = clean_storage_text(columns.get(10).copied());
                snapshot.operational_status = clean_storage_text(columns.get(11).copied());
                snapshot.temperature_c = parse_optional_f32_field(columns.get(13).copied());
                let wear_percent = parse_optional_u64(columns.get(14).copied());
                if let Some(wear) = wear_percent {
                    snapshot.remaining_life_percent = Some((100.0 - wear as f32).clamp(0.0, 100.0));
                    snapshot.attributes.push(storage_counter_attribute(
                        "Wear / percentage used",
                        Some(wear),
                        "%",
                        wear_severity(wear),
                    ));
                }
                snapshot.power_on_hours = parse_optional_u64(columns.get(15).copied());
                snapshot.power_cycle_count = parse_optional_u64(columns.get(16).copied());
                snapshot.read_errors_total = parse_optional_u64(columns.get(17).copied());
                snapshot.write_errors_total = parse_optional_u64(columns.get(18).copied());
                let read_uncorrected = parse_optional_u64(columns.get(19).copied());
                let write_uncorrected = parse_optional_u64(columns.get(20).copied());
                snapshot.uncorrectable_sectors =
                    max_optional_u64(read_uncorrected, write_uncorrected);
                snapshot.media_errors = parse_optional_u64(columns.get(21).copied());
                snapshot.data_read_bytes = parse_storage_data_units(columns.get(22).copied());
                snapshot.data_written_bytes = parse_storage_data_units(columns.get(23).copied());
                if let Some(note) = clean_storage_text(columns.get(24).copied()) {
                    if !note.is_empty() {
                        snapshot.provider_notes.push(note);
                    }
                }
                snapshot.provider_notes.retain(|note| {
                    note != "Windows storage providers returned limited data"
                        || snapshot.temperature_c.is_none()
                            && snapshot.health_status_text.is_none()
                            && snapshot.capacity_bytes.is_none()
                });

                push_optional_counter_attribute(
                    &mut snapshot.attributes,
                    "Temperature",
                    snapshot.temperature_c.map(|value| value.round() as u64),
                    " C",
                    snapshot
                        .temperature_c
                        .map(temperature_severity)
                        .unwrap_or(HealthSeverity::Info),
                );
                push_optional_counter_attribute(
                    &mut snapshot.attributes,
                    "Power-on hours",
                    snapshot.power_on_hours,
                    " h",
                    HealthSeverity::Info,
                );
                push_optional_counter_attribute(
                    &mut snapshot.attributes,
                    "Power cycles",
                    snapshot.power_cycle_count,
                    "",
                    HealthSeverity::Info,
                );
                push_optional_counter_attribute(
                    &mut snapshot.attributes,
                    "Read errors total",
                    snapshot.read_errors_total,
                    "",
                    error_count_severity(snapshot.read_errors_total),
                );
                push_optional_counter_attribute(
                    &mut snapshot.attributes,
                    "Write errors total",
                    snapshot.write_errors_total,
                    "",
                    error_count_severity(snapshot.write_errors_total),
                );
                push_optional_counter_attribute(
                    &mut snapshot.attributes,
                    "Uncorrected read/write errors",
                    snapshot.uncorrectable_sectors,
                    "",
                    error_count_severity(snapshot.uncorrectable_sectors),
                );
                push_optional_counter_attribute(
                    &mut snapshot.attributes,
                    "NVMe media/data integrity errors",
                    snapshot.media_errors,
                    "",
                    error_count_severity(snapshot.media_errors),
                );
            }
            Some("NVME") => {
                snapshot.available_spare_percent = parse_optional_u64(columns.get(1).copied());
                snapshot.available_spare_threshold_percent =
                    parse_optional_u64(columns.get(2).copied());
                snapshot.critical_warning_flags = parse_optional_u64(columns.get(3).copied());
                snapshot.unsafe_shutdowns = parse_optional_u64(columns.get(4).copied());
                snapshot.controller_busy_time_minutes =
                    parse_optional_u64(columns.get(5).copied());
                snapshot.host_read_commands = parse_optional_u64(columns.get(6).copied());
                snapshot.host_write_commands = parse_optional_u64(columns.get(7).copied());
                snapshot.warning_temperature_time_minutes =
                    parse_optional_u64(columns.get(8).copied());
                snapshot.critical_temperature_time_minutes =
                    parse_optional_u64(columns.get(9).copied());
                snapshot.thermal_management_temp1_transition_count =
                    parse_optional_u64(columns.get(10).copied());
                snapshot.thermal_management_temp2_transition_count =
                    parse_optional_u64(columns.get(11).copied());
                for sensor_index in 0..snapshot.nvme_temperature_sensors_c.len() {
                    snapshot.nvme_temperature_sensors_c[sensor_index] =
                        parse_optional_f32_field(columns.get(12 + sensor_index).copied());
                }
                push_nvme_health_attributes(&mut snapshot);
            }
            Some("SMARTSTATUS") => {
                if let (Some(instance), Some(predict_failure)) =
                    (columns.get(1), parse_optional_bool(columns.get(2).copied()))
                {
                    smart_statuses.push(((*instance).to_owned(), predict_failure));
                }
            }
            Some("SMART") => {
                if let (Some(instance), Some(id)) = (
                    columns.get(1),
                    columns.get(2).and_then(|value| value.parse::<u16>().ok()),
                ) {
                    raw_smart.push(RawSmartAttribute {
                        instance: (*instance).to_owned(),
                        id,
                        current: parse_optional_u64(columns.get(3).copied()),
                        worst: parse_optional_u64(columns.get(4).copied()),
                        threshold: parse_optional_u64(columns.get(5).copied()),
                        raw: parse_optional_u64(columns.get(6).copied()),
                    });
                }
            }
            _ => {}
        }
    }

    let selected_instance = select_smart_instance(&snapshot, &raw_smart);
    if let Some(instance) = selected_instance.as_deref() {
        for attribute in raw_smart
            .iter()
            .filter(|attribute| attribute.instance == instance)
        {
            apply_smart_attribute_to_snapshot(&mut snapshot, attribute);
        }
        if let Some((_, predict_failure)) = smart_statuses
            .iter()
            .find(|(status_instance, _)| status_instance == instance)
        {
            snapshot.smart_passed = Some(!*predict_failure);
        }
    } else if raw_smart.is_empty() {
        snapshot
            .provider_notes
            .push("Raw ATA SMART attributes were not exposed by the current provider".to_owned());
    } else {
        snapshot.provider_notes.push(
            "Raw ATA SMART attributes were available but could not be mapped safely to the selected volume"
                .to_owned(),
        );
    }

    if snapshot.smart_passed.is_none() && smart_statuses.len() == 1 {
        snapshot.smart_passed = smart_statuses.first().map(|(_, failed)| !*failed);
    }

    if snapshot.temperature_c.is_none() {
        let drive_sensor =
            drive_letter_for_path(&drive.root).map(|letter| query_drive_temperature(Some(letter)));
        if let Some(reading) = drive_sensor.as_ref() {
            snapshot.temperature_c = sensor_temperature(Some(reading));
        }
    }
    if snapshot.utilization_percent.is_none() {
        snapshot.utilization_percent = drive_letter_for_path(&drive.root)
            .and_then(|letter| query_drive_utilization(Some(letter)));
    }

    finalize_storage_health_snapshot(snapshot)
        .unwrap_or_else(|err| StorageHealthSnapshot::unknown(drive, format!("{err:#}")))
}

#[cfg(windows)]
fn merge_direct_nvme_health_log(
    drive: &DriveInfo,
    mut snapshot: StorageHealthSnapshot,
    provider_output: &str,
) -> StorageHealthSnapshot {
    if !is_nvme_storage_snapshot(&snapshot) {
        return snapshot;
    }

    let Some(physical_drive_number) = parse_windows_physical_drive_number(provider_output) else {
        snapshot.provider_notes.push(
            "Direct NVMe SMART health log was skipped because Windows did not report the physical drive number"
                .to_owned(),
        );
        return snapshot;
    };

    match query_direct_nvme_health_log(physical_drive_number) {
        Ok(health_log) => {
            apply_nvme_health_log_to_snapshot(&mut snapshot, &health_log);
            snapshot.provider_notes.push(format!(
                "Direct NVMe SMART health log read from PhysicalDrive{physical_drive_number}"
            ));
            finalize_storage_health_snapshot(snapshot)
                .unwrap_or_else(|err| StorageHealthSnapshot::unknown(drive, format!("{err:#}")))
        }
        Err(err) => {
            snapshot.provider_notes.push(format!(
                "Direct NVMe SMART health log unavailable for PhysicalDrive{physical_drive_number}: {err:#}"
            ));
            snapshot
        }
    }
}

#[cfg(windows)]
fn is_nvme_storage_snapshot(snapshot: &StorageHealthSnapshot) -> bool {
    snapshot.bus_type.to_ascii_lowercase().contains("nvme")
        || snapshot.model.to_ascii_lowercase().contains("nvme")
}

#[cfg(windows)]
fn parse_windows_physical_drive_number(output: &str) -> Option<u32> {
    output.lines().find_map(|line| {
        let columns = line.split('\t').collect::<Vec<_>>();
        (columns.first().copied() == Some("DISK"))
            .then(|| parse_optional_u64(columns.get(1).copied()))
            .flatten()
            .and_then(|value| u32::try_from(value).ok())
    })
}

#[cfg(windows)]
struct StorageDeviceHandle(*mut std::ffi::c_void);

#[cfg(windows)]
impl StorageDeviceHandle {
    fn open(path: &str) -> Result<Self> {
        const GENERIC_READ: u32 = 0x8000_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        const OPEN_EXISTING: u32 = 3;
        const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;

        use std::os::windows::ffi::OsStrExt;

        let path_wide = std::ffi::OsStr::new(path)
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let handle = unsafe {
            storage_create_file_w(
                path_wide.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };

        if handle as isize == -1 {
            return Err(anyhow!("opening {path}: {}", std::io::Error::last_os_error()));
        }

        Ok(Self(handle))
    }
}

#[cfg(windows)]
impl Drop for StorageDeviceHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = storage_close_handle(self.0);
        }
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    #[link_name = "CreateFileW"]
    fn storage_create_file_w(
        lp_file_name: *const u16,
        dw_desired_access: u32,
        dw_share_mode: u32,
        lp_security_attributes: *mut std::ffi::c_void,
        dw_creation_disposition: u32,
        dw_flags_and_attributes: u32,
        h_template_file: *mut std::ffi::c_void,
    ) -> *mut std::ffi::c_void;

    #[link_name = "DeviceIoControl"]
    fn storage_device_io_control(
        h_device: *mut std::ffi::c_void,
        dw_io_control_code: u32,
        lp_in_buffer: *mut std::ffi::c_void,
        n_in_buffer_size: u32,
        lp_out_buffer: *mut std::ffi::c_void,
        n_out_buffer_size: u32,
        lp_bytes_returned: *mut u32,
        lp_overlapped: *mut std::ffi::c_void,
    ) -> i32;

    #[link_name = "CloseHandle"]
    fn storage_close_handle(h_object: *mut std::ffi::c_void) -> i32;
}

#[cfg(windows)]
fn query_direct_nvme_health_log(physical_drive_number: u32) -> Result<NvmeHealthLog> {
    const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
    const STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY: u32 = 50;
    const PROPERTY_STANDARD_QUERY: u32 = 0;
    const PROTOCOL_TYPE_NVME: u32 = 3;
    const NVME_DATA_TYPE_LOG_PAGE: u32 = 2;
    const NVME_LOG_PAGE_HEALTH_INFO: u32 = 0x02;
    const STORAGE_PROPERTY_QUERY_SIZE: usize = 8;
    const STORAGE_PROTOCOL_SPECIFIC_DATA_SIZE: usize = 40;
    const STORAGE_PROTOCOL_DATA_DESCRIPTOR_HEADER_SIZE: usize = 8;
    const NVME_HEALTH_INFO_LOG_SIZE: usize = 512;

    let device_path = format!(r"\\.\PhysicalDrive{physical_drive_number}");
    let handle = StorageDeviceHandle::open(&device_path)?;
    let mut buffer = vec![
        0_u8;
        STORAGE_PROTOCOL_DATA_DESCRIPTOR_HEADER_SIZE
            + STORAGE_PROTOCOL_SPECIFIC_DATA_SIZE
            + NVME_HEALTH_INFO_LOG_SIZE
    ];

    write_u32_le(&mut buffer, 0, STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY);
    write_u32_le(&mut buffer, 4, PROPERTY_STANDARD_QUERY);
    let protocol = STORAGE_PROPERTY_QUERY_SIZE;
    write_u32_le(&mut buffer, protocol, PROTOCOL_TYPE_NVME);
    write_u32_le(&mut buffer, protocol + 4, NVME_DATA_TYPE_LOG_PAGE);
    write_u32_le(&mut buffer, protocol + 8, NVME_LOG_PAGE_HEALTH_INFO);
    write_u32_le(
        &mut buffer,
        protocol + 16,
        STORAGE_PROTOCOL_SPECIFIC_DATA_SIZE as u32,
    );
    write_u32_le(&mut buffer, protocol + 20, NVME_HEALTH_INFO_LOG_SIZE as u32);

    let mut returned = 0_u32;
    let ok = unsafe {
        storage_device_io_control(
            handle.0,
            IOCTL_STORAGE_QUERY_PROPERTY,
            buffer.as_mut_ptr().cast::<std::ffi::c_void>(),
            buffer.len() as u32,
            buffer.as_mut_ptr().cast::<std::ffi::c_void>(),
            buffer.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(anyhow!(
            "DeviceIoControl(IOCTL_STORAGE_QUERY_PROPERTY) failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    let returned_protocol = STORAGE_PROTOCOL_DATA_DESCRIPTOR_HEADER_SIZE;
    let data_offset = read_u32_le(&buffer, returned_protocol + 16)
        .map(|offset| returned_protocol + offset as usize)
        .unwrap_or(returned_protocol + STORAGE_PROTOCOL_SPECIFIC_DATA_SIZE);
    let data_length = read_u32_le(&buffer, returned_protocol + 20)
        .map(|length| length as usize)
        .unwrap_or(NVME_HEALTH_INFO_LOG_SIZE);
    if data_offset >= buffer.len() || data_length == 0 {
        return Err(anyhow!("NVMe health log returned an empty protocol data buffer"));
    }
    let data_end = data_offset.saturating_add(data_length).min(buffer.len());
    parse_nvme_health_log_bytes(&buffer[data_offset..data_end])
        .ok_or_else(|| anyhow!("NVMe health log was shorter than expected"))
}

fn finalize_storage_health_snapshot(
    mut snapshot: StorageHealthSnapshot,
) -> Result<StorageHealthSnapshot> {
    snapshot.warnings.clear();

    if snapshot.smart_passed == Some(false) {
        snapshot.warnings.push(HealthWarning {
            severity: HealthSeverity::Critical,
            title: "SMART failure predicted".to_owned(),
            detail: "The drive or controller reported a SMART failure prediction.".to_owned(),
        });
    }

    if let Some(health) = &snapshot.health_status_text {
        let lower = health.to_ascii_lowercase();
        if lower.contains("unhealthy")
            || lower.contains("failed")
            || lower.contains("lost")
            || lower.contains("critical")
        {
            snapshot.warnings.push(HealthWarning {
                severity: HealthSeverity::Critical,
                title: "Windows storage health is not healthy".to_owned(),
                detail: health.clone(),
            });
        } else if !lower.is_empty()
            && !lower.contains("healthy")
            && !lower.contains("ok")
            && !lower.contains("0")
        {
            snapshot.warnings.push(HealthWarning {
                severity: HealthSeverity::Warning,
                title: "Windows storage health needs attention".to_owned(),
                detail: health.clone(),
            });
        }
    }

    if let Some(temp) = snapshot.temperature_c {
        if temp >= STORAGE_HEALTH_TEMP_CRITICAL_C {
            snapshot.warnings.push(HealthWarning {
                severity: HealthSeverity::Critical,
                title: "Drive temperature is critical".to_owned(),
                detail: format!("{temp:.0} C is at or above the critical threshold."),
            });
        } else if temp >= STORAGE_HEALTH_TEMP_WARNING_C {
            snapshot.warnings.push(HealthWarning {
                severity: HealthSeverity::Warning,
                title: "Drive temperature is warm".to_owned(),
                detail: format!("{temp:.0} C is above the recommended warning threshold."),
            });
        }
    }

    add_counter_warning(
        &mut snapshot.warnings,
        "Reallocated sectors",
        snapshot.reallocated_sectors,
        HealthSeverity::Warning,
        HealthSeverity::Critical,
    );
    add_counter_warning(
        &mut snapshot.warnings,
        "Current pending sectors",
        snapshot.pending_sectors,
        HealthSeverity::Critical,
        HealthSeverity::Critical,
    );
    add_counter_warning(
        &mut snapshot.warnings,
        "Uncorrectable sectors or errors",
        snapshot.uncorrectable_sectors,
        HealthSeverity::Critical,
        HealthSeverity::Critical,
    );
    add_counter_warning(
        &mut snapshot.warnings,
        "NVMe media/data integrity errors",
        snapshot.media_errors,
        HealthSeverity::Critical,
        HealthSeverity::Critical,
    );
    add_counter_warning(
        &mut snapshot.warnings,
        "Read errors",
        snapshot.read_errors_total,
        HealthSeverity::Warning,
        HealthSeverity::Critical,
    );
    add_counter_warning(
        &mut snapshot.warnings,
        "Write errors",
        snapshot.write_errors_total,
        HealthSeverity::Warning,
        HealthSeverity::Critical,
    );
    if let (Some(spare), Some(threshold)) = (
        snapshot.available_spare_percent,
        snapshot.available_spare_threshold_percent,
    ) {
        if spare <= threshold {
            snapshot.warnings.push(HealthWarning {
                severity: HealthSeverity::Critical,
                title: "NVMe available spare is below threshold".to_owned(),
                detail: format!(
                    "Available spare is {spare}% and the device threshold is {threshold}%."
                ),
            });
        } else if spare <= threshold.saturating_add(10) {
            snapshot.warnings.push(HealthWarning {
                severity: HealthSeverity::Warning,
                title: "NVMe available spare is getting low".to_owned(),
                detail: format!(
                    "Available spare is {spare}% and the device threshold is {threshold}%."
                ),
            });
        }
    }
    if let Some(flags) = snapshot.critical_warning_flags.filter(|flags| *flags != 0) {
        snapshot.warnings.push(HealthWarning {
            severity: HealthSeverity::Critical,
            title: "NVMe critical warning flags are set".to_owned(),
            detail: format!("Critical warning bitfield is 0x{flags:02x}."),
        });
    }
    if let Some(minutes) = snapshot
        .critical_temperature_time_minutes
        .filter(|minutes| *minutes != 0)
    {
        snapshot.warnings.push(HealthWarning {
            severity: HealthSeverity::Critical,
            title: "NVMe critical temperature time recorded".to_owned(),
            detail: format!("The drive reports {minutes} minute(s) at critical temperature."),
        });
    }
    if let Some(minutes) = snapshot
        .warning_temperature_time_minutes
        .filter(|minutes| *minutes != 0)
    {
        snapshot.warnings.push(HealthWarning {
            severity: HealthSeverity::Warning,
            title: "NVMe warning temperature time recorded".to_owned(),
            detail: format!("The drive reports {minutes} minute(s) at warning temperature."),
        });
    }

    if let Some(life) = snapshot.remaining_life_percent {
        if life <= 5.0 {
            snapshot.warnings.push(HealthWarning {
                severity: HealthSeverity::Critical,
                title: "SSD life estimate is nearly exhausted".to_owned(),
                detail: format!("Estimated remaining life is {life:.0}%."),
            });
        } else if life <= 15.0 {
            snapshot.warnings.push(HealthWarning {
                severity: HealthSeverity::Warning,
                title: "SSD life estimate is low".to_owned(),
                detail: format!("Estimated remaining life is {life:.0}%."),
            });
        }
    }

    let has_health_data = snapshot.smart_passed.is_some()
        || snapshot.temperature_c.is_some()
        || snapshot.health_status_text.is_some()
        || !snapshot.attributes.is_empty()
        || snapshot.remaining_life_percent.is_some()
        || snapshot.reallocated_sectors.is_some()
        || snapshot.pending_sectors.is_some()
        || snapshot.uncorrectable_sectors.is_some()
        || snapshot.media_errors.is_some()
        || snapshot.nvme_error_info_log_entries.is_some()
        || snapshot.read_errors_total.is_some()
        || snapshot.write_errors_total.is_some()
        || snapshot.available_spare_percent.is_some()
        || snapshot.critical_warning_flags.is_some()
        || snapshot.unsafe_shutdowns.is_some()
        || snapshot.controller_busy_time_minutes.is_some()
        || snapshot
            .nvme_temperature_sensors_c
            .iter()
            .any(Option::is_some);
    snapshot.status = if !has_health_data {
        StorageHealthStatus::Unknown
    } else if snapshot
        .warnings
        .iter()
        .any(|warning| warning.severity == HealthSeverity::Critical)
    {
        StorageHealthStatus::Critical
    } else if snapshot
        .warnings
        .iter()
        .any(|warning| warning.severity == HealthSeverity::Warning)
    {
        StorageHealthStatus::Caution
    } else {
        StorageHealthStatus::Good
    };
    snapshot.health_percent = estimate_storage_health_percent(&snapshot);

    Ok(snapshot)
}

fn apply_nvme_health_log_to_snapshot(snapshot: &mut StorageHealthSnapshot, health: &NvmeHealthLog) {
    snapshot.temperature_c = health.temperature_c.or(snapshot.temperature_c);
    snapshot.available_spare_percent = Some(health.available_spare_percent);
    snapshot.available_spare_threshold_percent = Some(health.available_spare_threshold_percent);
    snapshot.critical_warning_flags = Some(health.critical_warning_flags);
    snapshot.unsafe_shutdowns = Some(health.unsafe_shutdowns);
    snapshot.controller_busy_time_minutes = Some(health.controller_busy_time_minutes);
    snapshot.host_read_commands = Some(health.host_read_commands);
    snapshot.host_write_commands = Some(health.host_write_commands);
    snapshot.warning_temperature_time_minutes = Some(health.warning_temperature_time_minutes);
    snapshot.critical_temperature_time_minutes = Some(health.critical_temperature_time_minutes);
    snapshot.power_cycle_count = Some(health.power_cycle_count);
    snapshot.power_on_hours = Some(health.power_on_hours);
    snapshot.media_errors = Some(health.media_errors);
    snapshot.nvme_error_info_log_entries = Some(health.error_info_log_entries);
    snapshot.data_read_bytes = Some(health.data_read_bytes);
    snapshot.data_written_bytes = Some(health.data_written_bytes);
    snapshot.remaining_life_percent =
        Some((100.0 - health.percentage_used as f32).clamp(0.0, 100.0));
    snapshot.nvme_temperature_sensors_c = health.temperature_sensors_c;

    snapshot
        .attributes
        .retain(|attribute| attribute.name != "Wear / percentage used");
    snapshot.attributes.push(storage_counter_attribute(
        "Wear / percentage used",
        Some(health.percentage_used),
        "%",
        wear_severity(health.percentage_used),
    ));
    push_nvme_health_attributes(snapshot);
}

fn push_nvme_health_attributes(snapshot: &mut StorageHealthSnapshot) {
    remove_nvme_health_attributes(snapshot);

    let spare_severity = nvme_spare_severity(
        snapshot.available_spare_percent,
        snapshot.available_spare_threshold_percent,
    );
    push_optional_counter_attribute(
        &mut snapshot.attributes,
        "NVMe available spare",
        snapshot.available_spare_percent,
        "%",
        spare_severity,
    );
    push_optional_counter_attribute(
        &mut snapshot.attributes,
        "NVMe spare threshold",
        snapshot.available_spare_threshold_percent,
        "%",
        HealthSeverity::Info,
    );
    push_optional_counter_attribute(
        &mut snapshot.attributes,
        "NVMe critical warning flags",
        snapshot.critical_warning_flags,
        "",
        nonzero_severity(snapshot.critical_warning_flags, HealthSeverity::Critical),
    );
    push_optional_counter_attribute(
        &mut snapshot.attributes,
        "NVMe unsafe shutdowns",
        snapshot.unsafe_shutdowns,
        "",
        nonzero_severity(snapshot.unsafe_shutdowns, HealthSeverity::Warning),
    );
    push_optional_counter_attribute(
        &mut snapshot.attributes,
        "NVMe controller busy time",
        snapshot.controller_busy_time_minutes,
        " min",
        HealthSeverity::Info,
    );
    push_optional_counter_attribute(
        &mut snapshot.attributes,
        "NVMe host read commands",
        snapshot.host_read_commands,
        "",
        HealthSeverity::Info,
    );
    push_optional_counter_attribute(
        &mut snapshot.attributes,
        "NVMe host write commands",
        snapshot.host_write_commands,
        "",
        HealthSeverity::Info,
    );
    push_optional_counter_attribute(
        &mut snapshot.attributes,
        "NVMe data read",
        snapshot.data_read_bytes,
        " bytes",
        HealthSeverity::Info,
    );
    push_optional_counter_attribute(
        &mut snapshot.attributes,
        "NVMe data written",
        snapshot.data_written_bytes,
        " bytes",
        HealthSeverity::Info,
    );
    push_optional_counter_attribute(
        &mut snapshot.attributes,
        "NVMe media/data integrity errors",
        snapshot.media_errors,
        "",
        error_count_severity(snapshot.media_errors),
    );
    push_optional_counter_attribute(
        &mut snapshot.attributes,
        "NVMe error information log entries",
        snapshot.nvme_error_info_log_entries,
        "",
        error_count_severity(snapshot.nvme_error_info_log_entries),
    );
    push_optional_counter_attribute(
        &mut snapshot.attributes,
        "NVMe warning temperature time",
        snapshot.warning_temperature_time_minutes,
        " min",
        nonzero_severity(
            snapshot.warning_temperature_time_minutes,
            HealthSeverity::Warning,
        ),
    );
    push_optional_counter_attribute(
        &mut snapshot.attributes,
        "NVMe critical temperature time",
        snapshot.critical_temperature_time_minutes,
        " min",
        nonzero_severity(
            snapshot.critical_temperature_time_minutes,
            HealthSeverity::Critical,
        ),
    );
    push_optional_counter_attribute(
        &mut snapshot.attributes,
        "NVMe thermal management temp 1 transitions",
        snapshot.thermal_management_temp1_transition_count,
        "",
        nonzero_severity(
            snapshot.thermal_management_temp1_transition_count,
            HealthSeverity::Warning,
        ),
    );
    push_optional_counter_attribute(
        &mut snapshot.attributes,
        "NVMe thermal management temp 2 transitions",
        snapshot.thermal_management_temp2_transition_count,
        "",
        nonzero_severity(
            snapshot.thermal_management_temp2_transition_count,
            HealthSeverity::Warning,
        ),
    );
    for (index, value) in snapshot.nvme_temperature_sensors_c.iter().enumerate() {
        if let Some(value) = value {
            snapshot.attributes.push(StorageAttribute {
                id: None,
                name: format!("NVMe temperature sensor {}", index + 1),
                current: Some(value.round() as u64),
                worst: None,
                threshold: None,
                raw: Some(value.round() as u64),
                display_value: format!("{value:.0} C"),
                interpretation: "Additional NVMe temperature sensor.".to_owned(),
                severity: temperature_severity(*value),
            });
        }
    }
}

fn remove_nvme_health_attributes(snapshot: &mut StorageHealthSnapshot) {
    snapshot
        .attributes
        .retain(|attribute| !attribute.name.starts_with("NVMe "));
}

fn nvme_spare_severity(spare: Option<u64>, threshold: Option<u64>) -> HealthSeverity {
    match (spare, threshold) {
        (Some(spare), Some(threshold)) if spare <= threshold => HealthSeverity::Critical,
        (Some(spare), Some(threshold)) if spare <= threshold.saturating_add(10) => {
            HealthSeverity::Warning
        }
        _ => HealthSeverity::Info,
    }
}

fn nonzero_severity(value: Option<u64>, severity: HealthSeverity) -> HealthSeverity {
    if value.unwrap_or(0) == 0 {
        HealthSeverity::Info
    } else {
        severity
    }
}

fn estimate_storage_health_percent(snapshot: &StorageHealthSnapshot) -> Option<f32> {
    if snapshot.status == StorageHealthStatus::Unknown {
        return None;
    }

    let mut score = snapshot
        .remaining_life_percent
        .unwrap_or(100.0)
        .clamp(0.0, 100.0);

    if snapshot.smart_passed == Some(false) {
        score = score.min(5.0);
    }

    if let Some(temp) = snapshot.temperature_c {
        if temp >= STORAGE_HEALTH_TEMP_CRITICAL_C {
            score = score.min(30.0);
        } else if temp >= STORAGE_HEALTH_TEMP_WARNING_C {
            score = score.min(82.0);
        }
    }

    score -= storage_counter_penalty(snapshot.reallocated_sectors, 3.0, 35.0);
    score -= storage_counter_penalty(snapshot.read_errors_total, 2.0, 30.0);
    score -= storage_counter_penalty(snapshot.write_errors_total, 2.0, 30.0);

    if snapshot.pending_sectors.unwrap_or(0) > 0 {
        score = score.min(40.0) - storage_counter_penalty(snapshot.pending_sectors, 8.0, 25.0);
    }
    if snapshot.uncorrectable_sectors.unwrap_or(0) > 0 {
        score =
            score.min(35.0) - storage_counter_penalty(snapshot.uncorrectable_sectors, 8.0, 25.0);
    }
    if snapshot.media_errors.unwrap_or(0) > 0 {
        score = score.min(35.0) - storage_counter_penalty(snapshot.media_errors, 6.0, 30.0);
    }
    if let (Some(spare), Some(threshold)) = (
        snapshot.available_spare_percent,
        snapshot.available_spare_threshold_percent,
    ) {
        if spare <= threshold {
            score = score.min(35.0);
        } else if spare <= threshold.saturating_add(10) {
            score = score.min(82.0);
        }
    }
    if snapshot.critical_warning_flags.unwrap_or(0) != 0 {
        score = score.min(25.0);
    }
    if snapshot.critical_temperature_time_minutes.unwrap_or(0) != 0 {
        score = score.min(40.0);
    } else if snapshot.warning_temperature_time_minutes.unwrap_or(0) != 0 {
        score = score.min(85.0);
    }

    for warning in &snapshot.warnings {
        match warning.severity {
            HealthSeverity::Info => {}
            HealthSeverity::Warning => {
                score = score.min(85.0);
            }
            HealthSeverity::Critical => {
                score = score.min(45.0);
            }
        }
    }

    Some(score.clamp(0.0, 100.0))
}

fn storage_counter_penalty(value: Option<u64>, per_count: f32, max_penalty: f32) -> f32 {
    value
        .map(|value| (value as f32 * per_count).min(max_penalty))
        .unwrap_or(0.0)
}

fn add_counter_warning(
    warnings: &mut Vec<HealthWarning>,
    title: &str,
    value: Option<u64>,
    low_severity: HealthSeverity,
    high_severity: HealthSeverity,
) {
    let Some(value) = value else {
        return;
    };
    if value == 0 {
        return;
    }
    let severity = if value >= 10 {
        high_severity
    } else {
        low_severity
    };
    warnings.push(HealthWarning {
        severity,
        title: title.to_owned(),
        detail: format!(
            "Reported value is {value}. Back up important data and monitor this drive."
        ),
    });
}

fn apply_smart_attribute_to_snapshot(
    snapshot: &mut StorageHealthSnapshot,
    raw: &RawSmartAttribute,
) {
    let attribute = smart_attribute_from_raw(raw);
    match raw.id {
        5 => snapshot.reallocated_sectors = raw.raw,
        9 => snapshot.power_on_hours = snapshot.power_on_hours.or(raw.raw),
        12 => snapshot.power_cycle_count = snapshot.power_cycle_count.or(raw.raw),
        187 | 198 => {
            snapshot.uncorrectable_sectors =
                max_optional_u64(snapshot.uncorrectable_sectors, raw.raw)
        }
        194 | 190 => {
            if snapshot.temperature_c.is_none() {
                snapshot.temperature_c = raw.raw.map(|value| (value & 0xFF) as f32);
            }
        }
        197 => snapshot.pending_sectors = raw.raw,
        199 => {}
        202 => {
            if snapshot.remaining_life_percent.is_none() {
                if let Some(value) = raw.raw.filter(|value| *value <= 100) {
                    snapshot.remaining_life_percent =
                        Some((100.0 - value as f32).clamp(0.0, 100.0));
                }
            }
        }
        231 | 233 => {
            if snapshot.remaining_life_percent.is_none() {
                if let Some(current) = raw.current.filter(|value| *value <= 100) {
                    snapshot.remaining_life_percent = Some(current as f32);
                } else if let Some(value) = raw.raw.filter(|value| *value <= 100) {
                    snapshot.remaining_life_percent = Some(value as f32);
                }
            }
        }
        241 => snapshot.data_written_bytes = raw.raw.map(|value| value.saturating_mul(512)),
        242 => snapshot.data_read_bytes = raw.raw.map(|value| value.saturating_mul(512)),
        _ => {}
    }
    snapshot.attributes.push(attribute);
}

fn smart_attribute_from_raw(raw: &RawSmartAttribute) -> StorageAttribute {
    let name = smart_attribute_name(raw.id).to_owned();
    let severity = smart_attribute_severity(raw);
    let raw_value = raw.raw;
    let interpretation = smart_attribute_interpretation(raw.id, raw_value, severity);
    StorageAttribute {
        id: Some(raw.id),
        name,
        current: raw.current,
        worst: raw.worst,
        threshold: raw.threshold,
        raw: raw.raw,
        display_value: raw
            .raw
            .map(|value| value.to_string())
            .unwrap_or_else(|| "N/A".to_owned()),
        interpretation,
        severity,
    }
}

fn smart_attribute_name(id: u16) -> &'static str {
    match id {
        5 => "Reallocated Sector Count",
        9 => "Power-On Hours",
        12 => "Power Cycle Count",
        184 => "End-to-End Error",
        187 => "Reported Uncorrectable Errors",
        188 => "Command Timeout",
        190 => "Airflow Temperature",
        194 => "Temperature",
        196 => "Reallocation Event Count",
        197 => "Current Pending Sector Count",
        198 => "Offline Uncorrectable Sector Count",
        199 => "UDMA CRC Error Count",
        202 => "Percent Lifetime Used",
        231 => "SSD Life Left",
        233 => "Media Wearout Indicator",
        241 => "Total LBAs Written",
        242 => "Total LBAs Read",
        _ => "SMART Attribute",
    }
}

fn smart_attribute_severity(raw: &RawSmartAttribute) -> HealthSeverity {
    if let (Some(current), Some(threshold)) = (raw.current, raw.threshold) {
        if threshold > 0 && current <= threshold {
            return HealthSeverity::Critical;
        }
    }
    let value = raw.raw.unwrap_or(0);
    match raw.id {
        5 | 196 if value >= 10 => HealthSeverity::Critical,
        5 | 196 if value > 0 => HealthSeverity::Warning,
        184 | 187 | 188 | 197 | 198 if value > 0 => HealthSeverity::Critical,
        199 if value > 0 => HealthSeverity::Warning,
        190 | 194 if (value & 0xFF) >= STORAGE_HEALTH_TEMP_CRITICAL_C as u64 => {
            HealthSeverity::Critical
        }
        190 | 194 if (value & 0xFF) >= STORAGE_HEALTH_TEMP_WARNING_C as u64 => {
            HealthSeverity::Warning
        }
        202 if value >= 95 => HealthSeverity::Critical,
        202 if value >= 80 => HealthSeverity::Warning,
        231 | 233 if raw.current.is_some_and(|value| value <= 5) => HealthSeverity::Critical,
        231 | 233 if raw.current.is_some_and(|value| value <= 15) => HealthSeverity::Warning,
        _ => HealthSeverity::Info,
    }
}

fn smart_attribute_interpretation(
    id: u16,
    raw_value: Option<u64>,
    severity: HealthSeverity,
) -> String {
    let value = raw_value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_owned());
    match (id, severity) {
        (5, HealthSeverity::Warning | HealthSeverity::Critical) => {
            format!("{value} sectors have been remapped.")
        }
        (197, HealthSeverity::Critical) => {
            format!("{value} sectors are pending re-test or remap.")
        }
        (198, HealthSeverity::Critical) | (187, HealthSeverity::Critical) => {
            format!("{value} uncorrectable errors were reported.")
        }
        (199, HealthSeverity::Warning) => {
            format!("{value} interface CRC errors; check cable, port, enclosure, or controller.")
        }
        (190 | 194, HealthSeverity::Warning | HealthSeverity::Critical) => {
            format!("Temperature-related SMART value is {value}.")
        }
        (202, HealthSeverity::Warning | HealthSeverity::Critical) => {
            format!("{value}% of rated lifetime appears to be used.")
        }
        _ => {
            if severity == HealthSeverity::Info {
                "No warning interpretation for this value.".to_owned()
            } else {
                format!("Reported value {value} needs attention.")
            }
        }
    }
}

fn select_smart_instance(
    snapshot: &StorageHealthSnapshot,
    attributes: &[RawSmartAttribute],
) -> Option<String> {
    let mut instances = attributes
        .iter()
        .map(|attribute| attribute.instance.clone())
        .collect::<Vec<_>>();
    instances.sort();
    instances.dedup();
    if instances.len() == 1 {
        return instances.pop();
    }

    let serial = snapshot
        .serial
        .as_deref()
        .map(normalize_storage_match_text)
        .filter(|value| !value.is_empty());
    let model = normalize_storage_match_text(&snapshot.model);
    instances.into_iter().find(|instance| {
        let normalized = normalize_storage_match_text(instance);
        serial
            .as_ref()
            .is_some_and(|serial| normalized.contains(serial))
            || (!model.is_empty()
                && model
                    .split_whitespace()
                    .filter(|part| part.len() >= 4)
                    .any(|part| normalized.contains(part)))
    })
}

fn normalize_storage_match_text(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn storage_counter_attribute(
    name: &str,
    value: Option<u64>,
    suffix: &str,
    severity: HealthSeverity,
) -> StorageAttribute {
    let display_value = value
        .map(|value| format!("{value}{suffix}"))
        .unwrap_or_else(|| "N/A".to_owned());
    StorageAttribute {
        id: None,
        name: name.to_owned(),
        current: value,
        worst: None,
        threshold: None,
        raw: value,
        display_value,
        interpretation: if severity == HealthSeverity::Info {
            "Provider counter.".to_owned()
        } else {
            "Provider counter needs attention.".to_owned()
        },
        severity,
    }
}

fn push_optional_counter_attribute(
    attributes: &mut Vec<StorageAttribute>,
    name: &str,
    value: Option<u64>,
    suffix: &str,
    severity: HealthSeverity,
) {
    if value.is_some() {
        attributes.push(storage_counter_attribute(name, value, suffix, severity));
    }
}

fn parse_nvme_health_log_bytes(bytes: &[u8]) -> Option<NvmeHealthLog> {
    if bytes.len() < 216 {
        return None;
    }

    let mut temperature_sensors_c = [None; 8];
    for (index, sensor) in temperature_sensors_c.iter_mut().enumerate() {
        *sensor = read_u16_le(bytes, 200 + index * 2).and_then(nvme_kelvin_to_celsius);
    }

    Some(NvmeHealthLog {
        critical_warning_flags: bytes.first().copied().unwrap_or(0) as u64,
        temperature_c: read_u16_le(bytes, 1).and_then(nvme_kelvin_to_celsius),
        available_spare_percent: bytes.get(3).copied().unwrap_or(0) as u64,
        available_spare_threshold_percent: bytes.get(4).copied().unwrap_or(0) as u64,
        percentage_used: bytes.get(5).copied().unwrap_or(0) as u64,
        data_read_bytes: nvme_data_units_to_bytes(read_u128_le(bytes, 32)?),
        data_written_bytes: nvme_data_units_to_bytes(read_u128_le(bytes, 48)?),
        host_read_commands: u128_to_u64_saturating(read_u128_le(bytes, 64)?),
        host_write_commands: u128_to_u64_saturating(read_u128_le(bytes, 80)?),
        controller_busy_time_minutes: u128_to_u64_saturating(read_u128_le(bytes, 96)?),
        power_cycle_count: u128_to_u64_saturating(read_u128_le(bytes, 112)?),
        power_on_hours: u128_to_u64_saturating(read_u128_le(bytes, 128)?),
        unsafe_shutdowns: u128_to_u64_saturating(read_u128_le(bytes, 144)?),
        media_errors: u128_to_u64_saturating(read_u128_le(bytes, 160)?),
        error_info_log_entries: u128_to_u64_saturating(read_u128_le(bytes, 176)?),
        warning_temperature_time_minutes: read_u32_le(bytes, 192)? as u64,
        critical_temperature_time_minutes: read_u32_le(bytes, 196)? as u64,
        temperature_sensors_c,
    })
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    let value = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    let value = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn read_u128_le(bytes: &[u8], offset: usize) -> Option<u128> {
    let value = bytes.get(offset..offset + 16)?;
    let mut wide = [0_u8; 16];
    wide.copy_from_slice(value);
    Some(u128::from_le_bytes(wide))
}

fn write_u32_le(bytes: &mut [u8], offset: usize, value: u32) {
    if let Some(target) = bytes.get_mut(offset..offset + 4) {
        target.copy_from_slice(&value.to_le_bytes());
    }
}

fn nvme_kelvin_to_celsius(kelvin: u16) -> Option<f32> {
    if kelvin == 0 {
        return None;
    }
    let celsius = kelvin as f32 - 273.15;
    (-60.0..=150.0).contains(&celsius).then_some(celsius)
}

fn nvme_data_units_to_bytes(units: u128) -> u64 {
    u128_to_u64_saturating(units.saturating_mul(512_000))
}

fn u128_to_u64_saturating(value: u128) -> u64 {
    value.min(u64::MAX as u128) as u64
}

fn parse_storage_data_units(value: Option<&str>) -> Option<u64> {
    parse_optional_u64(value).map(|units| units.saturating_mul(512_000))
}

fn parse_optional_bool(value: Option<&str>) -> Option<bool> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

fn parse_optional_u64(value: Option<&str>) -> Option<u64> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        return u64::from_str_radix(
            &hex.chars()
                .take_while(|ch| ch.is_ascii_hexdigit())
                .collect::<String>(),
            16,
        )
        .ok();
    }
    value
        .split(|ch: char| !(ch.is_ascii_digit()))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse::<u64>().ok())
}

fn parse_optional_f32_field(value: Option<&str>) -> Option<f32> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.' || ch == '-'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse::<f32>().ok())
}

fn clean_storage_text(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

fn max_optional_u64(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

fn temperature_severity(value: f32) -> HealthSeverity {
    if value >= STORAGE_HEALTH_TEMP_CRITICAL_C {
        HealthSeverity::Critical
    } else if value >= STORAGE_HEALTH_TEMP_WARNING_C {
        HealthSeverity::Warning
    } else {
        HealthSeverity::Info
    }
}

fn wear_severity(value: u64) -> HealthSeverity {
    if value >= 95 {
        HealthSeverity::Critical
    } else if value >= 80 {
        HealthSeverity::Warning
    } else {
        HealthSeverity::Info
    }
}

fn error_count_severity(value: Option<u64>) -> HealthSeverity {
    match value {
        Some(value) if value >= 10 => HealthSeverity::Critical,
        Some(value) if value > 0 => HealthSeverity::Warning,
        _ => HealthSeverity::Info,
    }
}

fn run_storage_surface_scan(
    drive: &DriveInfo,
    capacity_bytes: Option<u64>,
    mode: StorageScanMode,
    cancel: Arc<AtomicBool>,
    tx: Sender<StorageHealthEvent>,
) -> Result<StorageScanResult> {
    let capacity_bytes = capacity_bytes.unwrap_or(512 * 1024 * 1024);
    let mut file = open_volume_for_read(&drive.root).with_context(|| {
        format!(
            "could not open {} for a read-only surface scan; try running as administrator",
            drive.root.display()
        )
    })?;
    let sample_count = mode.sample_count();
    let block_bytes = STORAGE_HEALTH_SCAN_BLOCK_BYTES as u64;
    let scan_span = capacity_bytes.saturating_sub(block_bytes).max(block_bytes);
    let mut buffer = vec![0_u8; STORAGE_HEALTH_SCAN_BLOCK_BYTES];
    let mut latencies = Vec::new();
    let mut read_errors = 0_u64;
    let mut slow_regions = 0_u64;
    let mut bytes_scanned = 0_u64;
    let mut notes = Vec::new();
    let started = Instant::now();

    for index in 0..sample_count {
        check_canceled_with(Some(&cancel), "Storage surface scan canceled")?;
        let offset = if sample_count <= 1 {
            0
        } else {
            scan_span.saturating_mul(index as u64) / (sample_count as u64 - 1)
        };
        let offset = (offset / DRIVE_RANDOM_BLOCK_BYTES as u64) * DRIVE_RANDOM_BLOCK_BYTES as u64;
        let read_started = Instant::now();
        let result = file
            .seek(SeekFrom::Start(offset))
            .and_then(|_| file.read(&mut buffer));
        let elapsed_ms = read_started.elapsed().as_secs_f64() * 1000.0;
        match result {
            Ok(bytes_read) => {
                bytes_scanned = bytes_scanned.saturating_add(bytes_read as u64);
                latencies.push(elapsed_ms);
                if elapsed_ms >= 250.0 {
                    slow_regions += 1;
                }
            }
            Err(err) => {
                read_errors += 1;
                notes.push(format!("Read failed near byte offset {offset}: {err}"));
            }
        }
        let regions_done = index + 1;
        let elapsed_s = started.elapsed().as_secs_f64();
        let eta_s = if regions_done > 0 && regions_done < sample_count {
            let total = elapsed_s / regions_done as f64 * sample_count as f64;
            Some((total - elapsed_s).max(0.0))
        } else {
            None
        };
        let _ = tx.send(StorageHealthEvent::ScanProgress(StorageScanProgress {
            mode,
            regions_done,
            regions_total: sample_count,
            bytes_scanned,
            read_errors,
            slow_regions,
            elapsed_s,
            eta_s,
        }));
    }

    if read_errors == 0 {
        notes.push("No read errors were observed in sampled regions.".to_owned());
    }
    if slow_regions > 0 {
        notes.push("Slow regions are latency warnings, not confirmed bad sectors.".to_owned());
    }

    Ok(StorageScanResult {
        mode,
        bytes_scanned,
        regions_scanned: sample_count,
        read_errors,
        slow_regions,
        avg_latency_ms: average_f64(&latencies),
        worst_latency_ms: latencies.iter().copied().reduce(f64::max),
        duration_ms: started.elapsed().as_secs_f64() * 1000.0,
        notes,
    })
}

fn open_volume_for_read(root: &PathBuf) -> Result<File> {
    #[cfg(windows)]
    {
        let letter = drive_letter_for_path(root)
            .ok_or_else(|| anyhow!("selected drive does not have a drive letter"))?;
        let volume_path = format!("\\\\.\\{letter}:");
        let file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ_RAW | FILE_SHARE_WRITE_RAW | FILE_SHARE_DELETE_RAW)
            .custom_flags(FILE_FLAG_SEQUENTIAL_SCAN_RAW)
            .open(&volume_path)
            .with_context(|| format!("opening raw volume {volume_path}"))?;
        Ok(file)
    }

    #[cfg(not(windows))]
    {
        let _ = root;
        Err(anyhow!(
            "read-only raw surface scans are currently implemented for Windows volumes"
        ))
    }
}

fn average_f64(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }
}

fn export_storage_health_report(
    snapshot: &StorageHealthSnapshot,
    scan_result: Option<&StorageScanResult>,
    benchmark_results: &[DriveBenchmarkResult],
) -> Result<PathBuf> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let path = std::env::current_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join(format!("benchscope-storage-health-{timestamp}.md"));
    fs::write(
        &path,
        render_storage_health_report(snapshot, scan_result, benchmark_results),
    )
    .with_context(|| format!("writing report {}", path.display()))?;
    Ok(path)
}

fn render_storage_health_report(
    snapshot: &StorageHealthSnapshot,
    scan_result: Option<&StorageScanResult>,
    benchmark_results: &[DriveBenchmarkResult],
) -> String {
    let mut report = String::new();
    report.push_str("# BenchScope Storage Health Report\n\n");
    report.push_str(&format!("Generated: {:?}\n\n", snapshot.refreshed_at));
    report.push_str("## Summary\n\n");
    report.push_str(&format!(
        "- Drive: {}\n",
        markdown_escape(&snapshot.drive_label)
    ));
    report.push_str(&format!("- Root: {}\n", snapshot.root.display()));
    report.push_str(&format!("- Model: {}\n", markdown_escape(&snapshot.model)));
    report.push_str(&format!(
        "- Serial: {}\n",
        markdown_escape(option_text(snapshot.serial.as_deref()))
    ));
    report.push_str(&format!(
        "- Firmware: {}\n",
        markdown_escape(option_text(snapshot.firmware.as_deref()))
    ));
    report.push_str(&format!(
        "- Bus type: {}\n",
        markdown_escape(&snapshot.bus_type)
    ));
    report.push_str(&format!(
        "- Media type: {}\n",
        markdown_escape(&snapshot.media_type)
    ));
    report.push_str(&format!(
        "- Capacity: {}\n",
        format_optional_bytes(snapshot.capacity_bytes)
    ));
    report.push_str(&format!(
        "- Free space: {}\n",
        format_optional_bytes(snapshot.free_bytes)
    ));
    report.push_str(&format!(
        "- Health: {}\n",
        format_storage_health_percent(snapshot.health_percent)
    ));
    report.push_str(&format!("- Overall status: {}\n", snapshot.status));
    report.push_str(&format!(
        "- SMART passed: {}\n",
        snapshot
            .smart_passed
            .map(|passed| if passed { "yes" } else { "no" })
            .unwrap_or("unknown")
    ));
    report.push_str(&format!(
        "- Temperature: {}\n",
        format_temperature_value(snapshot.temperature_c)
    ));
    report.push_str(&format!(
        "- Remaining life estimate: {}\n",
        format_percent_value(snapshot.remaining_life_percent)
    ));
    report.push_str(&format!(
        "- NVMe available spare: {}\n",
        format_percent_u64(snapshot.available_spare_percent)
    ));
    report.push_str(&format!(
        "- NVMe spare threshold: {}\n",
        format_percent_u64(snapshot.available_spare_threshold_percent)
    ));
    report.push_str(&format!(
        "- NVMe critical warning flags: {}\n",
        format_hex_u64(snapshot.critical_warning_flags)
    ));
    report.push_str(&format!(
        "- Unsafe shutdowns: {}\n",
        format_optional_u64(snapshot.unsafe_shutdowns)
    ));
    report.push_str(&format!(
        "- Controller busy time: {}\n",
        format_optional_u64_minutes(snapshot.controller_busy_time_minutes)
    ));
    report.push_str(&format!(
        "- Thermal warning time: {}\n",
        format_optional_u64_minutes(snapshot.warning_temperature_time_minutes)
    ));
    report.push_str(&format!(
        "- Thermal critical time: {}\n\n",
        format_optional_u64_minutes(snapshot.critical_temperature_time_minutes)
    ));

    report.push_str("## Warnings\n\n");
    if snapshot.warnings.is_empty() {
        report.push_str("No warning counters were reported by the available providers.\n\n");
    } else {
        for warning in &snapshot.warnings {
            report.push_str(&format!(
                "- **{}**: {} - {}\n",
                warning.severity.label(),
                markdown_escape(&warning.title),
                markdown_escape(&warning.detail)
            ));
        }
        report.push('\n');
    }

    report.push_str("## SMART / NVMe Attributes\n\n");
    report.push_str("| ID | Attribute | Current | Worst | Threshold | Raw / value | Severity | Interpretation |\n");
    report.push_str("| --- | --- | ---: | ---: | ---: | --- | --- | --- |\n");
    for attribute in &snapshot.attributes {
        report.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            attribute
                .id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".to_owned()),
            markdown_escape(&attribute.name),
            format_optional_u64(attribute.current),
            format_optional_u64(attribute.worst),
            format_optional_u64(attribute.threshold),
            markdown_escape(&attribute.display_value),
            attribute.severity.label(),
            markdown_escape(&attribute.interpretation)
        ));
    }
    report.push('\n');

    report.push_str("## Read-Only Scan\n\n");
    if let Some(result) = scan_result {
        report.push_str(&format!("- Mode: {}\n", result.mode));
        report.push_str(&format!(
            "- Bytes scanned: {}\n",
            format_bytes(result.bytes_scanned)
        ));
        report.push_str(&format!("- Regions scanned: {}\n", result.regions_scanned));
        report.push_str(&format!("- Read errors: {}\n", result.read_errors));
        report.push_str(&format!("- Slow regions: {}\n", result.slow_regions));
        report.push_str(&format!(
            "- Average latency: {}\n",
            format_optional_latency(result.avg_latency_ms)
        ));
        report.push_str(&format!(
            "- Worst latency: {}\n",
            format_optional_latency(result.worst_latency_ms)
        ));
        report.push_str(&format!(
            "- Duration: {} ms\n",
            format_ms(Some(result.duration_ms))
        ));
        for note in &result.notes {
            report.push_str(&format!("- Note: {}\n", markdown_escape(note)));
        }
        report.push('\n');
    } else {
        report.push_str("No read-only scan was run.\n\n");
    }

    report.push_str("## Quick Benchmark\n\n");
    if benchmark_results.is_empty() {
        report.push_str("No quick benchmark was run from the health checker.\n\n");
    } else {
        report.push_str("| Test | MB/s | IOPS | Avg latency | Mode | Notes |\n");
        report.push_str("| --- | ---: | ---: | ---: | --- | --- |\n");
        for result in benchmark_results {
            report.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} |\n",
                result.test.label(),
                format_drive_speed(result),
                format_optional_iops(result.iops),
                format_optional_latency(result.avg_latency_ms),
                result.io_mode.label(),
                markdown_escape(&result.notes.join(", "))
            ));
        }
        report.push('\n');
    }

    report.push_str("## Provider Notes\n\n");
    if snapshot.provider_notes.is_empty() {
        report.push_str("No provider notes.\n");
    } else {
        for note in &snapshot.provider_notes {
            report.push_str(&format!("- {}\n", markdown_escape(note)));
        }
    }
    report.push_str("\nSMART and remaining-life values are early-warning signals, not a guarantee. Keep backups of important data.\n");
    report
}

fn markdown_escape(value: &str) -> String {
    value.replace('|', "\\|")
}
