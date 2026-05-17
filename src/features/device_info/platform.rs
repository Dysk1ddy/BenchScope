fn collect_device_info_snapshot(wgpu_adapters: Vec<AdapterInfo>) -> Result<DeviceInfoSnapshot> {
    #[cfg(windows)]
    {
        let output = run_powershell_sensor_script(windows_device_info_script())
            .context("failed to query Windows device inventory")?;
        let mut snapshot = parse_windows_device_info_output(&output);
        snapshot.wgpu_adapters = wgpu_adapters;
        if snapshot.cpus.is_empty() {
            let cpu = detect_cpu_info();
            snapshot.cpus.push(DeviceCpuDetails {
                name: cpu.model,
                manufacturer: None,
                description: None,
                socket: None,
                processor_id: None,
                architecture: None,
                family: None,
                model: None,
                stepping: None,
                cores: None,
                logical_processors: Some(cpu.logical_processors as u32),
                max_clock_mhz: None,
                current_clock_mhz: None,
                l2_cache_kb: None,
                l3_cache_kb: None,
                virtualization_firmware_enabled: None,
                second_level_address_translation: None,
                vm_monitor_extensions: None,
            });
            snapshot
                .provider_notes
                .push("CPU details fell back to CPUID/available parallelism.".to_owned());
        }
        if snapshot.gpus.is_empty() && !snapshot.wgpu_adapters.is_empty() {
            snapshot
                .provider_notes
                .push("Windows video-controller inventory was empty; wgpu adapters are still listed.".to_owned());
        }
        Ok(snapshot)
    }

    #[cfg(not(windows))]
    {
        let mut snapshot = DeviceInfoSnapshot::empty(
            "Detailed device inventory is currently implemented for Windows providers.",
        );
        let cpu = detect_cpu_info();
        snapshot.cpus.push(DeviceCpuDetails {
            name: cpu.model,
            manufacturer: None,
            description: None,
            socket: None,
            processor_id: None,
            architecture: None,
            family: None,
            model: None,
            stepping: None,
            cores: None,
            logical_processors: Some(cpu.logical_processors as u32),
            max_clock_mhz: None,
            current_clock_mhz: None,
            l2_cache_kb: None,
            l3_cache_kb: None,
            virtualization_firmware_enabled: None,
            second_level_address_translation: None,
            vm_monitor_extensions: None,
        });
        snapshot.wgpu_adapters = wgpu_adapters;
        Ok(snapshot)
    }
}

#[cfg(windows)]
fn windows_device_info_script() -> &'static str {
    r#"
$ErrorActionPreference = 'SilentlyContinue'
$notes = New-Object System.Collections.Generic.List[string]
function Add-Note($message) {
    if ($message) {
        $script:notes.Add((([string]$message -replace "`t", " ") -replace "`r?`n", " "))
    }
}
function Clean($value) {
    if ($null -eq $value) { return '' }
    if ($value -is [array]) { $value = ($value | Where-Object { $null -ne $_ }) -join '; ' }
    return (([string]$value -replace "`t", " ") -replace "`r?`n", " ").Trim()
}
function JoinValues($values) {
    if ($null -eq $values) { return '' }
    return (@($values) | Where-Object { $null -ne $_ -and "$_".Trim().Length -gt 0 }) -join '; '
}
function Prop($obj, $name) {
    if ($null -eq $obj) { return $null }
    $prop = $obj.PSObject.Properties[$name]
    if ($null -eq $prop) { return $null }
    return $prop.Value
}
function FirstValue($values) {
    foreach ($value in $values) {
        if ($null -ne $value -and "$(Clean $value)".Length -gt 0) {
            return $value
        }
    }
    return $null
}
function DateValue($value) {
    if ($null -eq $value) { return '' }
    try {
        if ($value -is [datetime]) { return $value.ToString('yyyy-MM-dd') }
        $text = [string]$value
        if ($text -match '^\d{14}\.') {
            return ([Management.ManagementDateTimeConverter]::ToDateTime($text)).ToString('yyyy-MM-dd')
        }
        return ([datetime]$value).ToString('yyyy-MM-dd')
    } catch {
        return Clean $value
    }
}
function Safe($name, [scriptblock]$block) {
    try {
        & $block
    } catch {
        Add-Note "$name failed: $($_.Exception.Message)"
        @()
    }
}
function Emit($values) {
    [Console]::Out.WriteLine([string]::Join("`t", ($values | ForEach-Object { Clean $_ })))
}

$cs = Safe 'Win32_ComputerSystem' { Get-CimInstance Win32_ComputerSystem | Select-Object -First 1 }
$enclosure = Safe 'Win32_SystemEnclosure' { Get-CimInstance Win32_SystemEnclosure | Select-Object -First 1 }
if ($cs) {
    Emit @(
        'SYSTEM',
        (Prop $cs 'Manufacturer'),
        (Prop $cs 'Model'),
        (Prop $cs 'SystemFamily'),
        (Prop $cs 'SystemSKUNumber'),
        (Prop $cs 'SystemType'),
        (JoinValues (Prop $enclosure 'ChassisTypes')),
        (Prop $cs 'TotalPhysicalMemory'),
        (Prop $cs 'NumberOfProcessors'),
        (Prop $cs 'NumberOfLogicalProcessors'),
        (Prop $cs 'Domain'),
        (Prop $cs 'Workgroup'),
        (Prop $cs 'HypervisorPresent'),
        (Prop $cs 'UserName')
    )
}

$os = Safe 'Win32_OperatingSystem' { Get-CimInstance Win32_OperatingSystem | Select-Object -First 1 }
if ($os) {
    Emit @(
        'OS',
        (Prop $os 'Caption'),
        (Prop $os 'Version'),
        (Prop $os 'BuildNumber'),
        (Prop $os 'OSArchitecture'),
        (DateValue (Prop $os 'InstallDate')),
        (DateValue (Prop $os 'LastBootUpTime'))
    )
}

$bios = Safe 'Win32_BIOS' { Get-CimInstance Win32_BIOS | Select-Object -First 1 }
if ($bios) {
    Emit @(
        'BIOS',
        (Prop $bios 'Manufacturer'),
        (Prop $bios 'SMBIOSBIOSVersion'),
        (Prop $bios 'Version'),
        (DateValue (Prop $bios 'ReleaseDate')),
        (Prop $bios 'SerialNumber')
    )
}

$board = Safe 'Win32_BaseBoard' { Get-CimInstance Win32_BaseBoard | Select-Object -First 1 }
if ($board) {
    Emit @(
        'BOARD',
        (Prop $board 'Manufacturer'),
        (Prop $board 'Product'),
        (Prop $board 'Version'),
        (Prop $board 'SerialNumber')
    )
}

Safe 'Win32_Processor' {
    Get-CimInstance Win32_Processor | Sort-Object DeviceID | ForEach-Object {
        Emit @(
            'CPU',
            (Prop $_ 'Name'),
            (Prop $_ 'Manufacturer'),
            (Prop $_ 'Description'),
            (Prop $_ 'SocketDesignation'),
            (Prop $_ 'ProcessorId'),
            (Prop $_ 'Architecture'),
            (Prop $_ 'Family'),
            (Prop $_ 'Model'),
            (Prop $_ 'Stepping'),
            (Prop $_ 'NumberOfCores'),
            (Prop $_ 'NumberOfLogicalProcessors'),
            (Prop $_ 'MaxClockSpeed'),
            (Prop $_ 'CurrentClockSpeed'),
            (Prop $_ 'L2CacheSize'),
            (Prop $_ 'L3CacheSize'),
            (Prop $_ 'VirtualizationFirmwareEnabled'),
            (Prop $_ 'SecondLevelAddressTranslationExtensions'),
            (Prop $_ 'VMMonitorModeExtensions')
        )
    }
}

Safe 'Win32_PhysicalMemory' {
    Get-CimInstance Win32_PhysicalMemory |
        Sort-Object BankLabel, DeviceLocator |
        ForEach-Object {
            Emit @(
                'MEMORY',
                (Prop $_ 'Manufacturer'),
                (Prop $_ 'PartNumber'),
                (Prop $_ 'SerialNumber'),
                (Prop $_ 'BankLabel'),
                (Prop $_ 'DeviceLocator'),
                (Prop $_ 'Capacity'),
                (Prop $_ 'Speed'),
                (Prop $_ 'ConfiguredClockSpeed'),
                (Prop $_ 'FormFactor'),
                (Prop $_ 'MemoryType'),
                (Prop $_ 'SMBIOSMemoryType'),
                (Prop $_ 'TypeDetail'),
                (Prop $_ 'DataWidth'),
                (Prop $_ 'TotalWidth')
            )
        }
}

$diskByNumber = @{}
Safe 'Get-Disk' {
    Get-Disk | ForEach-Object {
        $diskByNumber[[string]$_.Number] = $_
    }
}
Safe 'Win32_DiskDrive' {
    Get-CimInstance Win32_DiskDrive |
        Sort-Object Index |
        ForEach-Object {
            $storageDisk = $diskByNumber[[string](Prop $_ 'Index')]
            Emit @(
                'DISK',
                (Prop $_ 'Index'),
                (Prop $_ 'DeviceID'),
                (FirstValue @((Prop $storageDisk 'FriendlyName'), (Prop $_ 'Model'))),
                (FirstValue @((Prop $storageDisk 'SerialNumber'), (Prop $_ 'SerialNumber'))),
                (FirstValue @((Prop $storageDisk 'FirmwareVersion'), (Prop $_ 'FirmwareRevision'))),
                (Prop $_ 'InterfaceType'),
                (Prop $_ 'MediaType'),
                (Prop $storageDisk 'BusType'),
                (FirstValue @((Prop $storageDisk 'Size'), (Prop $_ 'Size'))),
                (Prop $_ 'Partitions'),
                (Prop $_ 'Status'),
                (Prop $storageDisk 'HealthStatus'),
                (JoinValues (Prop $storageDisk 'OperationalStatus'))
            )
        }
}

Safe 'Get-Volume' {
    Get-Volume |
        Where-Object { $_.DriveLetter } |
        Sort-Object DriveLetter |
        ForEach-Object {
            Emit @(
                'VOLUME',
                (Prop $_ 'DriveLetter'),
                (Prop $_ 'FileSystemLabel'),
                (Prop $_ 'FileSystem'),
                (Prop $_ 'DriveType'),
                (Prop $_ 'Size'),
                (Prop $_ 'SizeRemaining'),
                (Prop $_ 'HealthStatus'),
                (JoinValues (Prop $_ 'OperationalStatus'))
            )
        }
}

$displayDrivers = Safe 'Win32_PnPSignedDriver display' {
    @(Get-CimInstance Win32_PnPSignedDriver -Filter "DeviceClass='DISPLAY'")
}
Safe 'Win32_VideoController' {
    Get-CimInstance Win32_VideoController |
        Sort-Object Name |
        ForEach-Object {
            $gpu = $_
            $driver = $displayDrivers |
                Where-Object {
                    (Prop $_ 'DeviceID') -eq (Prop $gpu 'PNPDeviceID') -or
                    (Prop $_ 'DeviceName') -eq (Prop $gpu 'Name')
                } |
                Select-Object -First 1
            Emit @(
                'GPU',
                (Prop $gpu 'Name'),
                (Prop $gpu 'PNPDeviceID'),
                (Prop $gpu 'AdapterCompatibility'),
                (Prop $gpu 'VideoProcessor'),
                (FirstValue @((Prop $driver 'DriverProviderName'), (Prop $gpu 'AdapterCompatibility'))),
                (FirstValue @((Prop $driver 'DriverVersion'), (Prop $gpu 'DriverVersion'))),
                (FirstValue @((DateValue (Prop $driver 'DriverDate')), (DateValue (Prop $gpu 'DriverDate')))),
                (Prop $driver 'InfName'),
                (Prop $gpu 'AdapterRAM'),
                (Prop $gpu 'CurrentHorizontalResolution'),
                (Prop $gpu 'CurrentVerticalResolution'),
                (Prop $gpu 'CurrentRefreshRate'),
                (Prop $gpu 'Status')
            )
        }
}

$netDrivers = Safe 'Win32_PnPSignedDriver net' {
    @(Get-CimInstance Win32_PnPSignedDriver -Filter "DeviceClass='NET'")
}
Safe 'Win32_NetworkAdapter' {
    Get-CimInstance Win32_NetworkAdapter |
        Where-Object { $_.PhysicalAdapter -eq $true -or $_.NetEnabled -eq $true } |
        Sort-Object InterfaceIndex |
        ForEach-Object {
            $net = $_
            $driver = $netDrivers |
                Where-Object {
                    (Prop $_ 'DeviceID') -eq (Prop $net 'PNPDeviceID') -or
                    (Prop $_ 'DeviceName') -eq (Prop $net 'Name')
                } |
                Select-Object -First 1
            Emit @(
                'NETWORK',
                (Prop $net 'Name'),
                (Prop $net 'Description'),
                (Prop $net 'PhysicalAdapter'),
                (Prop $net 'MACAddress'),
                (Prop $net 'Speed'),
                (Prop $net 'NetEnabled'),
                (Prop $driver 'DriverProviderName'),
                (Prop $driver 'DriverVersion'),
                (DateValue (Prop $driver 'DriverDate')),
                (Prop $net 'Status')
            )
        }
}

Safe 'Win32_DesktopMonitor' {
    Get-CimInstance Win32_DesktopMonitor |
        Sort-Object Name |
        ForEach-Object {
            $resolution = ''
            if ($_.ScreenWidth -and $_.ScreenHeight) {
                $resolution = "$($_.ScreenWidth)x$($_.ScreenHeight)"
            }
            Emit @(
                'MONITOR',
                (Prop $_ 'Name'),
                (Prop $_ 'MonitorManufacturer'),
                (Prop $_ 'MonitorType'),
                $resolution,
                (Prop $_ 'PNPDeviceID'),
                (Prop $_ 'Status')
            )
        }
}

$driverClasses = @(
    'DISPLAY', 'NET', 'HDC', 'SCSIADAPTER', 'DISKDRIVE', 'MEDIA', 'USB',
    'BLUETOOTH', 'BATTERY', 'MONITOR', 'SYSTEM', 'PROCESSOR', 'HIDCLASS',
    'KEYBOARD', 'MOUSE', 'IMAGE', 'CAMERA', 'SOFTWARECOMPONENT', 'FIRMWARE'
)
Safe 'Win32_PnPSignedDriver inventory' {
    Get-CimInstance Win32_PnPSignedDriver |
        Where-Object { $driverClasses -contains "$($_.DeviceClass)".ToUpperInvariant() } |
        Sort-Object DeviceClass, DeviceName |
        ForEach-Object {
            Emit @(
                'DRIVER',
                (Prop $_ 'DeviceClass'),
                (Prop $_ 'DeviceName'),
                (Prop $_ 'Manufacturer'),
                (Prop $_ 'DriverProviderName'),
                (Prop $_ 'DriverVersion'),
                (DateValue (Prop $_ 'DriverDate')),
                (Prop $_ 'Signer'),
                (Prop $_ 'InfName'),
                (Prop $_ 'DeviceID'),
                (Prop $_ 'IsSigned')
            )
        }
}

foreach ($note in $notes) {
    Emit @('NOTE', $note)
}
"#
}

#[cfg(windows)]
fn parse_windows_device_info_output(output: &str) -> DeviceInfoSnapshot {
    let mut snapshot = DeviceInfoSnapshot::empty("Windows device inventory collected.");
    snapshot.provider_notes.clear();

    for line in output.lines() {
        let fields = line.split('\t').collect::<Vec<_>>();
        match fields.first().copied() {
            Some("SYSTEM") => {
                snapshot.system = Some(DeviceSystemInfo {
                    manufacturer: device_info_text_field(&fields, 1),
                    model: device_info_text_field(&fields, 2),
                    family: device_info_text_field(&fields, 3),
                    sku: device_info_text_field(&fields, 4),
                    system_type: device_info_text_field(&fields, 5),
                    chassis: device_info_list_field(&fields, 6),
                    total_physical_memory_bytes: device_info_u64_field(&fields, 7),
                    physical_processor_count: device_info_u32_field(&fields, 8),
                    logical_processor_count: device_info_u32_field(&fields, 9),
                    domain: device_info_text_field(&fields, 10),
                    workgroup: device_info_text_field(&fields, 11),
                    hypervisor_present: device_info_bool_field(&fields, 12),
                    user_name: device_info_text_field(&fields, 13),
                });
            }
            Some("OS") => {
                snapshot.os = Some(DeviceOsInfo {
                    caption: device_info_text_field(&fields, 1),
                    version: device_info_text_field(&fields, 2),
                    build_number: device_info_text_field(&fields, 3),
                    architecture: device_info_text_field(&fields, 4),
                    install_date: device_info_text_field(&fields, 5),
                    last_boot_time: device_info_text_field(&fields, 6),
                });
            }
            Some("BIOS") => {
                snapshot.bios = Some(DeviceBiosInfo {
                    manufacturer: device_info_text_field(&fields, 1),
                    smbios_version: device_info_text_field(&fields, 2),
                    version: device_info_text_field(&fields, 3),
                    release_date: device_info_text_field(&fields, 4),
                    serial_number: device_info_text_field(&fields, 5),
                });
            }
            Some("BOARD") => {
                snapshot.baseboard = Some(DeviceBaseboardInfo {
                    manufacturer: device_info_text_field(&fields, 1),
                    product: device_info_text_field(&fields, 2),
                    version: device_info_text_field(&fields, 3),
                    serial_number: device_info_text_field(&fields, 4),
                });
            }
            Some("CPU") => {
                snapshot.cpus.push(DeviceCpuDetails {
                    name: device_info_text_field(&fields, 1)
                        .unwrap_or_else(|| "Unknown CPU".to_owned()),
                    manufacturer: device_info_text_field(&fields, 2),
                    description: device_info_text_field(&fields, 3),
                    socket: device_info_text_field(&fields, 4),
                    processor_id: device_info_text_field(&fields, 5),
                    architecture: device_info_architecture_label(device_info_u32_field(&fields, 6))
                        .or_else(|| device_info_text_field(&fields, 6)),
                    family: device_info_text_field(&fields, 7),
                    model: device_info_text_field(&fields, 8),
                    stepping: device_info_text_field(&fields, 9),
                    cores: device_info_u32_field(&fields, 10),
                    logical_processors: device_info_u32_field(&fields, 11),
                    max_clock_mhz: device_info_u32_field(&fields, 12),
                    current_clock_mhz: device_info_u32_field(&fields, 13),
                    l2_cache_kb: device_info_u32_field(&fields, 14),
                    l3_cache_kb: device_info_u32_field(&fields, 15),
                    virtualization_firmware_enabled: device_info_bool_field(&fields, 16),
                    second_level_address_translation: device_info_bool_field(&fields, 17),
                    vm_monitor_extensions: device_info_bool_field(&fields, 18),
                });
            }
            Some("MEMORY") => {
                snapshot.memory_modules.push(DeviceMemoryModuleInfo {
                    manufacturer: device_info_text_field(&fields, 1),
                    part_number: device_info_text_field(&fields, 2),
                    serial_number: device_info_text_field(&fields, 3),
                    bank_label: device_info_text_field(&fields, 4),
                    device_locator: device_info_text_field(&fields, 5),
                    capacity_bytes: device_info_u64_field(&fields, 6),
                    speed_mhz: device_info_u32_field(&fields, 7),
                    configured_clock_speed_mhz: device_info_u32_field(&fields, 8),
                    form_factor: device_info_memory_form_factor_label(
                        device_info_u32_field(&fields, 9),
                    )
                    .or_else(|| device_info_text_field(&fields, 9)),
                    memory_type: device_info_memory_type_label(device_info_u32_field(&fields, 10))
                        .or_else(|| device_info_text_field(&fields, 10)),
                    smbios_memory_type: device_info_memory_type_label(device_info_u32_field(
                        &fields, 11,
                    ))
                    .or_else(|| device_info_text_field(&fields, 11)),
                    type_detail: device_info_memory_type_detail_label(
                        device_info_u32_field(&fields, 12),
                    )
                    .or_else(|| device_info_text_field(&fields, 12)),
                    data_width_bits: device_info_u32_field(&fields, 13),
                    total_width_bits: device_info_u32_field(&fields, 14),
                });
            }
            Some("DISK") => {
                snapshot.disks.push(DeviceDiskInfo {
                    index: device_info_u32_field(&fields, 1),
                    device_id: device_info_text_field(&fields, 2),
                    model: device_info_text_field(&fields, 3)
                        .unwrap_or_else(|| "Unknown disk".to_owned()),
                    serial_number: device_info_text_field(&fields, 4),
                    firmware: device_info_text_field(&fields, 5),
                    interface_type: device_info_text_field(&fields, 6),
                    media_type: device_info_text_field(&fields, 7),
                    bus_type: device_info_text_field(&fields, 8),
                    size_bytes: device_info_u64_field(&fields, 9),
                    partitions: device_info_u32_field(&fields, 10),
                    status: device_info_text_field(&fields, 11),
                    health_status: device_info_text_field(&fields, 12),
                    operational_status: device_info_text_field(&fields, 13),
                });
            }
            Some("VOLUME") => {
                snapshot.volumes.push(DeviceVolumeInfo {
                    drive_letter: device_info_text_field(&fields, 1).map(|letter| format!("{letter}:")),
                    label: device_info_text_field(&fields, 2),
                    file_system: device_info_text_field(&fields, 3),
                    drive_type: device_info_text_field(&fields, 4),
                    size_bytes: device_info_u64_field(&fields, 5),
                    free_bytes: device_info_u64_field(&fields, 6),
                    health_status: device_info_text_field(&fields, 7),
                    operational_status: device_info_text_field(&fields, 8),
                });
            }
            Some("GPU") => {
                let width = device_info_u32_field(&fields, 10);
                let height = device_info_u32_field(&fields, 11);
                snapshot.gpus.push(DeviceGpuDetails {
                    name: device_info_text_field(&fields, 1)
                        .unwrap_or_else(|| "Unknown GPU".to_owned()),
                    pnp_device_id: device_info_text_field(&fields, 2),
                    adapter_compatibility: device_info_text_field(&fields, 3),
                    video_processor: device_info_text_field(&fields, 4),
                    driver_provider: device_info_text_field(&fields, 5),
                    driver_version: device_info_text_field(&fields, 6),
                    driver_date: device_info_text_field(&fields, 7),
                    inf_name: device_info_text_field(&fields, 8),
                    adapter_ram_bytes: device_info_u64_field(&fields, 9),
                    resolution: match (width, height) {
                        (Some(width), Some(height)) if width > 0 && height > 0 => {
                            Some(format!("{width}x{height}"))
                        }
                        _ => None,
                    },
                    refresh_rate_hz: device_info_u32_field(&fields, 12),
                    status: device_info_text_field(&fields, 13),
                });
            }
            Some("NETWORK") => {
                snapshot.network_adapters.push(DeviceNetworkAdapterInfo {
                    name: device_info_text_field(&fields, 1)
                        .unwrap_or_else(|| "Unknown network adapter".to_owned()),
                    description: device_info_text_field(&fields, 2),
                    physical: device_info_bool_field(&fields, 3),
                    mac_address: device_info_text_field(&fields, 4),
                    speed_bps: device_info_u64_field(&fields, 5),
                    net_enabled: device_info_bool_field(&fields, 6),
                    driver_provider: device_info_text_field(&fields, 7),
                    driver_version: device_info_text_field(&fields, 8),
                    driver_date: device_info_text_field(&fields, 9),
                    status: device_info_text_field(&fields, 10),
                });
            }
            Some("MONITOR") => {
                snapshot.monitors.push(DeviceMonitorInfo {
                    name: device_info_text_field(&fields, 1)
                        .unwrap_or_else(|| "Unknown monitor".to_owned()),
                    manufacturer: device_info_text_field(&fields, 2),
                    monitor_type: device_info_text_field(&fields, 3),
                    resolution: device_info_text_field(&fields, 4),
                    pnp_device_id: device_info_text_field(&fields, 5),
                    status: device_info_text_field(&fields, 6),
                });
            }
            Some("DRIVER") => {
                snapshot.drivers.push(DeviceDriverRecord {
                    device_class: device_info_text_field(&fields, 1),
                    device_name: device_info_text_field(&fields, 2)
                        .unwrap_or_else(|| "Unknown device".to_owned()),
                    manufacturer: device_info_text_field(&fields, 3),
                    provider: device_info_text_field(&fields, 4),
                    version: device_info_text_field(&fields, 5),
                    date: device_info_text_field(&fields, 6),
                    signer: device_info_text_field(&fields, 7),
                    inf_name: device_info_text_field(&fields, 8),
                    device_id: device_info_text_field(&fields, 9),
                    is_signed: device_info_bool_field(&fields, 10),
                });
            }
            Some("NOTE") => {
                if let Some(note) = device_info_text_field(&fields, 1) {
                    snapshot.provider_notes.push(note);
                }
            }
            _ => {}
        }
    }

    snapshot
        .drivers
        .sort_by(|left, right| {
            left.device_class
                .cmp(&right.device_class)
                .then_with(|| left.device_name.cmp(&right.device_name))
        });
    snapshot.provider_notes.sort();
    snapshot.provider_notes.dedup();
    snapshot
}

fn device_info_field<'a>(fields: &'a [&str], index: usize) -> &'a str {
    fields.get(index).copied().unwrap_or_default().trim()
}

fn device_info_text_field(fields: &[&str], index: usize) -> Option<String> {
    let value = device_info_field(fields, index);
    if value.is_empty()
        || value.eq_ignore_ascii_case("unknown")
        || value.eq_ignore_ascii_case("not available")
        || value.eq_ignore_ascii_case("to be filled by o.e.m.")
        || value.eq_ignore_ascii_case("system serial number")
        || value == "-"
    {
        None
    } else {
        Some(value.to_owned())
    }
}

fn device_info_list_field(fields: &[&str], index: usize) -> Vec<String> {
    device_info_field(fields, index)
        .split([';', ','])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn device_info_u64_field(fields: &[&str], index: usize) -> Option<u64> {
    let value = device_info_field(fields, index).replace(',', "");
    if value.is_empty() {
        return None;
    }
    value
        .split(|ch: char| !(ch.is_ascii_digit()))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse::<u64>().ok())
}

fn device_info_u32_field(fields: &[&str], index: usize) -> Option<u32> {
    device_info_u64_field(fields, index).and_then(|value| u32::try_from(value).ok())
}

fn device_info_bool_field(fields: &[&str], index: usize) -> Option<bool> {
    match device_info_field(fields, index)
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

fn device_info_architecture_label(value: Option<u32>) -> Option<String> {
    value.map(|value| {
        match value {
            0 => "x86",
            1 => "MIPS",
            2 => "Alpha",
            3 => "PowerPC",
            5 => "ARM",
            6 => "Itanium",
            9 => "x64",
            12 => "ARM64",
            _ => return value.to_string(),
        }
        .to_owned()
    })
}

fn device_info_memory_form_factor_label(value: Option<u32>) -> Option<String> {
    value.map(|value| {
        match value {
            3 => "SIMM",
            8 => "DIMM",
            9 => "TSOP",
            12 => "SODIMM",
            13 => "RIMM",
            15 => "FB-DIMM",
            _ => return value.to_string(),
        }
        .to_owned()
    })
}

fn device_info_memory_type_label(value: Option<u32>) -> Option<String> {
    value.map(|value| {
        match value {
            20 => "DDR",
            21 => "DDR2",
            24 => "DDR3",
            26 => "DDR4",
            30 => "LPDDR4",
            34 => "DDR5",
            35 => "LPDDR5",
            _ => return value.to_string(),
        }
        .to_owned()
    })
}

fn device_info_memory_type_detail_label(value: Option<u32>) -> Option<String> {
    value.map(|value| {
        let mut details = Vec::new();
        if value & 0x0002 != 0 {
            details.push("Other");
        }
        if value & 0x0004 != 0 {
            details.push("Unknown");
        }
        if value & 0x0080 != 0 {
            details.push("Synchronous");
        }
        if value & 0x2000 != 0 {
            details.push("Unbuffered");
        }
        if value & 0x4000 != 0 {
            details.push("Registered");
        }
        if details.is_empty() {
            value.to_string()
        } else {
            details.join(", ")
        }
    })
}

fn device_info_provider_plan() -> Vec<DeviceInfoProviderPlan> {
    vec![
        DeviceInfoProviderPlan {
            area: "Core system inventory",
            current_provider: "Windows CIM/WMI classes",
            status: "Implemented",
            extra_driver_needed: "No",
            notes: "Computer system, OS, BIOS, baseboard, CPU, RAM modules, disks, volumes, monitors, and PnP driver inventory.",
        },
        DeviceInfoProviderPlan {
            area: "GPU identity and capabilities",
            current_provider: "Win32_VideoController, PnP signed drivers, wgpu, DXGI",
            status: "Implemented",
            extra_driver_needed: "No",
            notes: "Shows Windows display drivers plus BenchScope's existing adapter backend, VRAM/shared-memory, and timestamp-query support.",
        },
        DeviceInfoProviderPlan {
            area: "Network drivers",
            current_provider: "Win32_NetworkAdapter and Win32_PnPSignedDriver",
            status: "Implemented",
            extra_driver_needed: "No",
            notes: "Complements the existing Network Hardware Diagnostic adapter and driver view.",
        },
        DeviceInfoProviderPlan {
            area: "Storage SMART/NVMe health",
            current_provider: "Existing Storage Health Checker providers",
            status: "Implemented in storage tool",
            extra_driver_needed: "No for exposed Windows counters",
            notes: "This viewer lists firmware and driver metadata; the storage health tool owns deep SMART/NVMe counters and read-only scans.",
        },
        DeviceInfoProviderPlan {
            area: "Live sensors",
            current_provider: "BenchScope sensor service/driver bridge plus safe Windows/NVIDIA probes",
            status: "Partially implemented",
            extra_driver_needed: "Driver extensions planned",
            notes: "The current service path is used for temperature/utilization telemetry. HWiNFO-class fan, voltage, EC, and chipset sensors need signed low-level provider extensions or vendor SDKs.",
        },
        DeviceInfoProviderPlan {
            area: "RAM SPD timings",
            current_provider: "Win32_PhysicalMemory",
            status: "Partial",
            extra_driver_needed: "Yes",
            notes: "Windows exposes module capacity, speed, manufacturer, part, and serial. Full SPD/XMP/EXPO timings require SMBus/SPD access through a signed driver or vendor-specific API.",
        },
        DeviceInfoProviderPlan {
            area: "BIOS update metadata",
            current_provider: "Win32_BIOS",
            status: "Version/date implemented",
            extra_driver_needed: "No driver; vendor catalog needed",
            notes: "Installed BIOS version/date is local. Latest available BIOS and changelogs require vendor-specific online lookup and are outside the offline inventory path.",
        },
    ]
}

fn write_device_info_report(snapshot: &DeviceInfoSnapshot) -> Result<PathBuf> {
    let dir = std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir());
    let path = dir.join(format!(
        "benchscope-device-info-{}.md",
        device_info_unix_timestamp_seconds()
    ));
    fs::write(&path, render_device_info_report(snapshot))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

fn render_device_info_report(snapshot: &DeviceInfoSnapshot) -> String {
    let mut report = String::new();
    report.push_str("# BenchScope Device Information Report\n\n");
    report.push_str(&format!(
        "- Generated: {}\n",
        device_info_unix_timestamp_seconds()
    ));
    report.push_str(&format!(
        "- Captured: {:?}\n",
        snapshot.captured_at
    ));
    report.push_str(&format!(
        "- CPU cores/logical processors: {} / {}\n",
        device_info_option_u32(snapshot.cpu_core_count()),
        device_info_option_u32(snapshot.cpu_logical_processor_count())
    ));
    report.push_str(&format!(
        "- RAM: {}\n",
        format_optional_bytes(snapshot.total_ram_bytes())
    ));
    report.push_str(&format!(
        "- Storage: {}\n",
        format_optional_bytes(snapshot.total_storage_bytes())
    ));
    report.push_str(&format!("- GPUs: {}\n", snapshot.gpus.len().max(snapshot.wgpu_adapters.len())));
    report.push_str(&format!("- Driver records: {}\n\n", snapshot.drivers.len()));

    if let Some(system) = &snapshot.system {
        report.push_str("## System\n\n");
        report.push_str(&device_info_report_pair("Manufacturer", system.manufacturer.as_deref()));
        report.push_str(&device_info_report_pair("Model", system.model.as_deref()));
        report.push_str(&device_info_report_pair("Family", system.family.as_deref()));
        report.push_str(&device_info_report_pair("SKU", system.sku.as_deref()));
        report.push_str(&device_info_report_pair("Type", system.system_type.as_deref()));
        report.push_str(&format!("- Chassis: {}\n", device_info_vec_label(&system.chassis)));
        report.push('\n');
    }

    if let Some(os) = &snapshot.os {
        report.push_str("## Operating System\n\n");
        report.push_str(&device_info_report_pair("Caption", os.caption.as_deref()));
        report.push_str(&device_info_report_pair("Version", os.version.as_deref()));
        report.push_str(&device_info_report_pair("Build", os.build_number.as_deref()));
        report.push_str(&device_info_report_pair("Architecture", os.architecture.as_deref()));
        report.push_str(&device_info_report_pair("Install date", os.install_date.as_deref()));
        report.push_str(&device_info_report_pair("Last boot", os.last_boot_time.as_deref()));
        report.push('\n');
    }

    report.push_str("## BIOS / Board\n\n");
    if let Some(bios) = &snapshot.bios {
        report.push_str(&device_info_report_pair("BIOS manufacturer", bios.manufacturer.as_deref()));
        report.push_str(&device_info_report_pair("BIOS SMBIOS version", bios.smbios_version.as_deref()));
        report.push_str(&device_info_report_pair("BIOS version", bios.version.as_deref()));
        report.push_str(&device_info_report_pair("BIOS date", bios.release_date.as_deref()));
    }
    if let Some(board) = &snapshot.baseboard {
        report.push_str(&device_info_report_pair("Board manufacturer", board.manufacturer.as_deref()));
        report.push_str(&device_info_report_pair("Board product", board.product.as_deref()));
        report.push_str(&device_info_report_pair("Board version", board.version.as_deref()));
    }
    report.push('\n');

    report.push_str("## CPU\n\n");
    for cpu in &snapshot.cpus {
        report.push_str(&format!("- {}\n", markdown_escape(&cpu.name)));
        report.push_str(&format!(
            "  - Cores/logical: {} / {}\n",
            device_info_option_u32(cpu.cores),
            device_info_option_u32(cpu.logical_processors)
        ));
        report.push_str(&format!(
            "  - Clock: {} current, {} max\n",
            device_info_mhz_label(cpu.current_clock_mhz),
            device_info_mhz_label(cpu.max_clock_mhz)
        ));
        report.push_str(&format!(
            "  - Socket: {}; virtualization: {}\n",
            device_info_option_label(cpu.socket.as_deref()),
            device_info_bool_label(cpu.virtualization_firmware_enabled)
        ));
    }
    report.push('\n');

    report.push_str("## RAM Modules\n\n");
    report.push_str("| Slot | Capacity | Type | Speed | Manufacturer | Part | Serial |\n");
    report.push_str("| --- | ---: | --- | ---: | --- | --- | --- |\n");
    for module in &snapshot.memory_modules {
        report.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            markdown_escape(&device_info_option_label(module.device_locator.as_deref())),
            format_optional_bytes(module.capacity_bytes),
            markdown_escape(&device_info_option_label(module.smbios_memory_type.as_deref().or(module.memory_type.as_deref()))),
            device_info_mhz_label(module.configured_clock_speed_mhz.or(module.speed_mhz)),
            markdown_escape(&device_info_option_label(module.manufacturer.as_deref())),
            markdown_escape(&device_info_option_label(module.part_number.as_deref())),
            markdown_escape(&device_info_option_label(module.serial_number.as_deref()))
        ));
    }
    report.push('\n');

    report.push_str("## Storage\n\n");
    report.push_str("| Disk | Size | Bus | Media | Firmware | Serial | Health |\n");
    report.push_str("| --- | ---: | --- | --- | --- | --- | --- |\n");
    for disk in &snapshot.disks {
        report.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            markdown_escape(&disk.model),
            format_optional_bytes(disk.size_bytes),
            markdown_escape(&device_info_option_label(disk.bus_type.as_deref().or(disk.interface_type.as_deref()))),
            markdown_escape(&device_info_option_label(disk.media_type.as_deref())),
            markdown_escape(&device_info_option_label(disk.firmware.as_deref())),
            markdown_escape(&device_info_option_label(disk.serial_number.as_deref())),
            markdown_escape(&device_info_option_label(disk.health_status.as_deref().or(disk.status.as_deref())))
        ));
    }
    report.push('\n');

    report.push_str("## Graphics\n\n");
    for gpu in &snapshot.gpus {
        report.push_str(&format!("- {}\n", markdown_escape(&gpu.name)));
        report.push_str(&format!(
            "  - Driver: {} {} ({})\n",
            device_info_option_label(gpu.driver_provider.as_deref()),
            device_info_option_label(gpu.driver_version.as_deref()),
            device_info_option_label(gpu.driver_date.as_deref())
        ));
        report.push_str(&format!(
            "  - VRAM/adapter RAM: {}; resolution: {}\n",
            format_optional_bytes(gpu.adapter_ram_bytes),
            device_info_option_label(gpu.resolution.as_deref())
        ));
    }
    if !snapshot.wgpu_adapters.is_empty() {
        report.push_str("\n### wgpu Adapters\n\n");
        for adapter in &snapshot.wgpu_adapters {
            report.push_str(&format!(
                "- {} | vendor {:04X} device {:04X} | driver {} | VRAM {}\n",
                markdown_escape(&adapter.label()),
                adapter.vendor,
                adapter.device,
                markdown_escape(empty_to_unknown(&adapter.driver)),
                format_optional_bytes(adapter.dedicated_vram_bytes)
            ));
        }
    }
    report.push('\n');

    report.push_str("## Driver Inventory\n\n");
    report.push_str("| Class | Device | Provider | Version | Date | Signed | Signer | INF | Device ID |\n");
    report.push_str("| --- | --- | --- | --- | --- | --- | --- | --- | --- |\n");
    for driver in &snapshot.drivers {
        report.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            markdown_escape(&device_info_option_label(driver.device_class.as_deref())),
            markdown_escape(&driver.device_name),
            markdown_escape(&device_info_option_label(driver.provider.as_deref())),
            markdown_escape(&device_info_option_label(driver.version.as_deref())),
            markdown_escape(&device_info_option_label(driver.date.as_deref())),
            device_info_bool_label(driver.is_signed),
            markdown_escape(&device_info_option_label(driver.signer.as_deref())),
            markdown_escape(&device_info_option_label(driver.inf_name.as_deref())),
            markdown_escape(&device_info_option_label(driver.device_id.as_deref()))
        ));
    }
    report.push('\n');

    report.push_str("## Provider Coverage Plan\n\n");
    report.push_str("| Area | Current provider | Status | Extra driver needed | Notes |\n");
    report.push_str("| --- | --- | --- | --- | --- |\n");
    for plan in device_info_provider_plan() {
        report.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            markdown_escape(plan.area),
            markdown_escape(plan.current_provider),
            markdown_escape(plan.status),
            markdown_escape(plan.extra_driver_needed),
            markdown_escape(plan.notes)
        ));
    }

    if !snapshot.provider_notes.is_empty() {
        report.push_str("\n## Provider Notes\n\n");
        for note in &snapshot.provider_notes {
            report.push_str(&format!("- {}\n", markdown_escape(note)));
        }
    }

    report
}

fn device_info_unix_timestamp_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn device_info_report_pair(label: &str, value: Option<&str>) -> String {
    format!("- {label}: {}\n", device_info_option_label(value))
}

fn device_info_option_label(value: Option<&str>) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("N/A")
        .to_owned()
}

fn device_info_option_u32(value: Option<u32>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "N/A".to_owned())
}

fn device_info_bool_label(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "yes",
        Some(false) => "no",
        None => "N/A",
    }
}

fn device_info_mhz_label(value: Option<u32>) -> String {
    value
        .map(|value| format!("{value} MHz"))
        .unwrap_or_else(|| "N/A".to_owned())
}

fn device_info_vec_label(values: &[String]) -> String {
    if values.is_empty() {
        "N/A".to_owned()
    } else {
        values.join(", ")
    }
}
