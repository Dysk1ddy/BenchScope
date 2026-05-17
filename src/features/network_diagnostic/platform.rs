fn detect_network_adapters() -> Result<Vec<NetworkAdapterSnapshot>> {
    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
function Clean($value) {
    if ($null -eq $value) { return '' }
    return ([string]$value) -replace "`t", ' ' -replace "`r|`n", ' '
}
function JoinValues($values) {
    if ($null -eq $values) { return '' }
    return (@($values) | Where-Object { $_ }) -join '; '
}
function Prop($obj, $name) {
    if ($null -eq $obj) { return '' }
    $prop = $obj.PSObject.Properties[$name]
    if ($null -eq $prop) { return '' }
    return $prop.Value
}

$driverByName = @{}
Get-CimInstance Win32_PnPSignedDriver -Filter "DeviceClass='NET'" | ForEach-Object {
    if ($_.DeviceName) { $driverByName[$_.DeviceName] = $_ }
}

$wifi = @{}
$netsh = netsh wlan show interfaces 2>$null
if ($netsh) {
    $current = @{}
    foreach ($line in $netsh) {
        if ($line -match '^\s*Name\s*:\s*(.+)$') {
            if ($current.Name) { $wifi[$current.Name] = $current }
            $current = @{ Name = $matches[1].Trim() }
        } elseif ($line -match '^\s*SSID\s*:\s*(.+)$' -and $line -notmatch 'BSSID') {
            $current.SSID = $matches[1].Trim()
        } elseif ($line -match '^\s*Signal\s*:\s*(\d+)%') {
            $current.Signal = $matches[1]
        } elseif ($line -match '^\s*Radio type\s*:\s*(.+)$') {
            $current.Radio = $matches[1].Trim()
        } elseif ($line -match '^\s*Channel\s*:\s*(\d+)') {
            $current.Channel = $matches[1]
        } elseif ($line -match '^\s*Receive rate.*:\s*([0-9.]+)') {
            $current.RxRate = $matches[1]
        } elseif ($line -match '^\s*Transmit rate.*:\s*([0-9.]+)') {
            $current.TxRate = $matches[1]
        }
    }
    if ($current.Name) { $wifi[$current.Name] = $current }
}

Get-NetAdapter | Sort-Object ifIndex | ForEach-Object {
    $adapter = $_
    $ip = Get-NetIPConfiguration -InterfaceIndex $adapter.ifIndex -ErrorAction SilentlyContinue
    $dns = Get-DnsClientServerAddress -InterfaceIndex $adapter.ifIndex -ErrorAction SilentlyContinue
    $stats = Get-NetAdapterStatistics -Name $adapter.Name -ErrorAction SilentlyContinue
    $driver = $driverByName[$adapter.InterfaceDescription]
    $wifiInfo = $wifi[$adapter.Name]
    $deviceStatus = ''
    $device = Get-CimInstance Win32_NetworkAdapter -Filter "InterfaceIndex=$($adapter.ifIndex)" -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($device) { $deviceStatus = $device.NetConnectionStatus }
    $ipv4 = @()
    $ipv6 = @()
    $gateway = @()
    if ($ip) {
        $ipv4 = @($ip.IPv4Address | ForEach-Object { $_.IPv4Address })
        $ipv6 = @($ip.IPv6Address | ForEach-Object { $_.IPv6Address })
        $gateway = @($ip.IPv4DefaultGateway | ForEach-Object { $_.NextHop }) + @($ip.IPv6DefaultGateway | ForEach-Object { $_.NextHop })
    }
    $dnsServers = @($dns | ForEach-Object { $_.ServerAddresses } | Where-Object { $_ })
    @(
        Clean $adapter.ifIndex
        Clean $adapter.Name
        Clean $adapter.InterfaceDescription
        Clean $adapter.Status
        Clean $adapter.LinkSpeed
        Clean $adapter.MacAddress
        Clean $adapter.HardwareInterface
        Clean $driver.DriverProviderName
        Clean $driver.DriverVersion
        Clean $driver.DriverDate
        Clean $deviceStatus
        Clean (JoinValues $ipv4)
        Clean (JoinValues $ipv6)
        Clean (JoinValues $gateway)
        Clean (JoinValues $dnsServers)
        Clean $wifiInfo.SSID
        Clean $wifiInfo.Signal
        Clean $wifiInfo.Radio
        Clean $wifiInfo.Channel
        Clean $wifiInfo.RxRate
        Clean $wifiInfo.TxRate
        Clean (Prop $stats 'ReceivedBytes')
        Clean (Prop $stats 'SentBytes')
        Clean (Prop $stats 'ReceivedUnicastPackets')
        Clean (Prop $stats 'SentUnicastPackets')
        Clean (Prop $stats 'ReceivedPacketErrors')
        Clean (Prop $stats 'OutboundPacketErrors')
        Clean (Prop $stats 'ReceivedDiscardedPackets')
        Clean (Prop $stats 'OutboundDiscardedPackets')
    ) -join "`t"
}
"#;
    let output = run_powershell_sensor_script(script)?;
    Ok(parse_network_adapter_rows(&output))
}

#[cfg(not(windows))]
fn detect_network_adapters() -> Result<Vec<NetworkAdapterSnapshot>> {
    Err(anyhow!(
        "network adapter detection is currently implemented for Windows"
    ))
}

fn parse_network_adapter_rows(output: &str) -> Vec<NetworkAdapterSnapshot> {
    output
        .lines()
        .filter_map(parse_network_adapter_row)
        .collect()
}

fn parse_network_adapter_row(line: &str) -> Option<NetworkAdapterSnapshot> {
    if line.trim().is_empty() {
        return None;
    }
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() < 7 {
        return None;
    }

    let id = network_field(&fields, 0);
    let name = network_field(&fields, 1);
    let description = network_field(&fields, 2);
    let status_text = network_field(&fields, 3);
    let link_speed_bps = parse_link_speed_bps(&network_field(&fields, 4));
    let mac_address = empty_to_option(network_field(&fields, 5));
    let is_physical = parse_network_bool(&network_field(&fields, 6)).unwrap_or(false);
    let driver_provider = empty_to_option(network_field(&fields, 7));
    let driver_version = empty_to_option(network_field(&fields, 8));
    let driver_date = empty_to_option(network_field(&fields, 9));
    let device_status = empty_to_option(network_field(&fields, 10));
    let ipv4_addresses = split_network_list(&network_field(&fields, 11));
    let ipv6_addresses = split_network_list(&network_field(&fields, 12));
    let gateways = split_network_list(&network_field(&fields, 13));
    let dns_servers = split_network_list(&network_field(&fields, 14));
    let ssid = empty_to_option(network_field(&fields, 15));
    let signal_quality_percent = parse_percent_u8(&network_field(&fields, 16));
    let phy_type = empty_to_option(network_field(&fields, 17));
    let channel = parse_u32_maybe(&network_field(&fields, 18));
    let rx_link_speed_bps = parse_mbps_to_bps(&network_field(&fields, 19)).or(link_speed_bps);
    let tx_link_speed_bps = parse_mbps_to_bps(&network_field(&fields, 20)).or(link_speed_bps);
    let connected = status_text.eq_ignore_ascii_case("up")
        || status_text.eq_ignore_ascii_case("connected")
        || !ipv4_addresses.is_empty()
        || !ipv6_addresses.is_empty();
    let kind = classify_network_adapter(&name, &description, ssid.as_deref(), is_physical);

    let driver = (driver_provider.is_some()
        || driver_version.is_some()
        || driver_date.is_some()
        || device_status.is_some())
    .then_some(NetworkDriverInfo {
        provider: driver_provider,
        version: driver_version,
        date: driver_date,
        device_status,
    });
    let wifi =
        (kind == NetworkAdapterKind::Wifi || ssid.is_some() || signal_quality_percent.is_some())
            .then_some(WifiSnapshot {
                ssid,
                signal_quality_percent,
                phy_type,
                channel,
                rx_link_speed_bps,
                tx_link_speed_bps,
            });
    let counters = Some(NetworkCounters {
        bytes_received: parse_u64_maybe(&network_field(&fields, 21)),
        bytes_sent: parse_u64_maybe(&network_field(&fields, 22)),
        packets_received: parse_u64_maybe(&network_field(&fields, 23)),
        packets_sent: parse_u64_maybe(&network_field(&fields, 24)),
        inbound_errors: parse_u64_maybe(&network_field(&fields, 25)),
        outbound_errors: parse_u64_maybe(&network_field(&fields, 26)),
        inbound_discards: parse_u64_maybe(&network_field(&fields, 27)),
        outbound_discards: parse_u64_maybe(&network_field(&fields, 28)),
    });

    let mut snapshot = NetworkAdapterSnapshot {
        id: if id.is_empty() { name.clone() } else { id },
        name,
        description,
        kind,
        status: NetworkHealthStatus::Unknown,
        connected,
        is_physical,
        link_speed_bps,
        mac_address,
        ipv4_addresses,
        ipv6_addresses,
        gateways,
        dns_servers,
        driver,
        wifi,
        counters,
        provider_notes: Vec::new(),
    };
    snapshot.status = initial_network_status(&snapshot);
    Some(snapshot)
}

fn network_field(fields: &[&str], index: usize) -> String {
    fields
        .get(index)
        .map(|value| value.trim().to_owned())
        .unwrap_or_default()
}

fn empty_to_option(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn split_network_list(value: &str) -> Vec<String> {
    value
        .split([';', ','])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_network_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" => Some(true),
        "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

fn parse_u64_maybe(value: &str) -> Option<u64> {
    let normalized = value.trim().replace(',', "");
    if normalized.is_empty() {
        None
    } else {
        normalized.parse::<u64>().ok()
    }
}

fn parse_u32_maybe(value: &str) -> Option<u32> {
    parse_u64_maybe(value).and_then(|value| u32::try_from(value).ok())
}

fn parse_percent_u8(value: &str) -> Option<u8> {
    let normalized = value.trim().trim_end_matches('%').replace(',', "");
    normalized.parse::<u8>().ok().map(|value| value.min(100))
}

fn parse_mbps_to_bps(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    value
        .replace(',', "")
        .parse::<f64>()
        .ok()
        .map(|mbps| (mbps * 1_000_000.0).round() as u64)
}

fn parse_link_speed_bps(value: &str) -> Option<u64> {
    let normalized = value.trim().replace(',', "");
    if normalized.is_empty() {
        return None;
    }
    let lower = normalized.to_ascii_lowercase();
    let number = lower
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<f64>().ok())?;
    let multiplier = if lower.contains("gbps") || lower.contains("gbit") {
        1_000_000_000.0
    } else if lower.contains("mbps") || lower.contains("mbit") {
        1_000_000.0
    } else if lower.contains("kbps") || lower.contains("kbit") {
        1_000.0
    } else if number >= 10_000.0 {
        1.0
    } else {
        1_000_000.0
    };
    Some((number * multiplier).round() as u64)
}

fn classify_network_adapter(
    name: &str,
    description: &str,
    ssid: Option<&str>,
    is_physical: bool,
) -> NetworkAdapterKind {
    let text = format!("{name} {description}").to_ascii_lowercase();
    if ssid.is_some()
        || text.contains("wi-fi")
        || text.contains("wifi")
        || text.contains("wireless")
        || text.contains("802.11")
        || text.contains("wlan")
    {
        return NetworkAdapterKind::Wifi;
    }
    if !is_physical
        || text.contains("virtual")
        || text.contains("vpn")
        || text.contains("tap")
        || text.contains("tun ")
        || text.contains("wintun")
        || text.contains("wireguard")
        || text.contains("hyper-v")
        || text.contains("loopback")
        || text.contains("bluetooth")
        || text.contains("wsl")
    {
        return NetworkAdapterKind::Virtual;
    }
    if text.contains("ethernet")
        || text.contains("gbe")
        || text.contains("2.5g")
        || text.contains("lan")
        || text.contains("realtek")
        || text.contains("intel")
    {
        return NetworkAdapterKind::Ethernet;
    }
    if is_physical {
        NetworkAdapterKind::Other
    } else {
        NetworkAdapterKind::Unknown
    }
}

fn initial_network_status(snapshot: &NetworkAdapterSnapshot) -> NetworkHealthStatus {
    if snapshot.kind == NetworkAdapterKind::Virtual {
        return NetworkHealthStatus::Unknown;
    }
    if !snapshot.connected {
        return NetworkHealthStatus::Critical;
    }
    if snapshot.gateways.is_empty() {
        return NetworkHealthStatus::Caution;
    }
    if let Some(wifi) = &snapshot.wifi {
        if wifi
            .signal_quality_percent
            .is_some_and(|signal| signal < 40)
        {
            return NetworkHealthStatus::Caution;
        }
    }
    NetworkHealthStatus::Good
}

fn preferred_network_adapter_index(adapters: &[NetworkAdapterSnapshot]) -> Option<usize> {
    adapters
        .iter()
        .position(|adapter| {
            adapter.connected
                && adapter.is_physical
                && adapter.kind == NetworkAdapterKind::Ethernet
                && !adapter.gateways.is_empty()
        })
        .or_else(|| {
            adapters.iter().position(|adapter| {
                adapter.connected
                    && adapter.is_physical
                    && adapter.kind == NetworkAdapterKind::Wifi
                    && !adapter.gateways.is_empty()
            })
        })
        .or_else(|| {
            adapters
                .iter()
                .position(|adapter| adapter.connected && !adapter.gateways.is_empty())
        })
        .or_else(|| adapters.iter().position(|adapter| adapter.connected))
}

fn find_network_adapter_snapshot(
    adapters: Vec<NetworkAdapterSnapshot>,
    adapter_id: &str,
) -> Option<NetworkAdapterSnapshot> {
    adapters
        .iter()
        .find(|adapter| adapter.id == adapter_id)
        .cloned()
        .or_else(|| {
            preferred_network_adapter_index(&adapters)
                .and_then(|index| adapters.get(index).cloned())
        })
}

fn run_network_quick_diagnosis(
    adapter_id: String,
    cancel: Arc<AtomicBool>,
    tx: Sender<NetworkWorkerEvent>,
) -> Result<NetworkDiagnosisResult> {
    let start = Instant::now();
    let adapters = detect_network_adapters()?;
    let mut snapshot = find_network_adapter_snapshot(adapters, &adapter_id)
        .ok_or_else(|| anyhow!("selected network adapter was not found"))?;
    let mut probes = Vec::new();

    let mut steps = Vec::new();
    if let Some(gateway) = snapshot.gateways.first().cloned() {
        steps.push(("Gateway", gateway, NETWORK_QUICK_PROBE_COUNT));
    }
    if let Some(dns) = snapshot.dns_servers.first().cloned() {
        steps.push(("DNS server", dns, NETWORK_QUICK_PROBE_COUNT));
    }
    steps.push(("Public IP", "1.1.1.1".to_owned(), NETWORK_QUICK_PROBE_COUNT));
    steps.push((
        "Public hostname",
        "example.com".to_owned(),
        NETWORK_HOSTNAME_PROBE_COUNT,
    ));
    let total_steps = steps.len() + 1;

    let _ = tx.send(NetworkWorkerEvent::Log(format!(
        "Running network diagnosis on {}",
        snapshot.menu_label()
    )));

    for (index, (label, target, count)) in steps.iter().enumerate() {
        check_canceled_with(Some(&cancel), "Network diagnosis canceled")?;
        let _ = tx.send(NetworkWorkerEvent::Progress(NetworkProgress {
            step: format!("Testing {label}"),
            progress: index as f32 / total_steps as f32,
            elapsed_s: start.elapsed().as_secs_f64(),
        }));
        let result = run_icmp_probe_cancelable(
            label,
            target,
            *count,
            NETWORK_PROBE_TIMEOUT_MS,
            &cancel,
            &tx,
            start,
            index,
            total_steps,
        )?;
        probes.push(result.clone());
        let _ = tx.send(NetworkWorkerEvent::ProbeCompleted(result));
    }

    check_canceled_with(Some(&cancel), "Network diagnosis canceled")?;
    let _ = tx.send(NetworkWorkerEvent::Progress(NetworkProgress {
        step: "Resolving DNS".to_owned(),
        progress: (total_steps - 1) as f32 / total_steps as f32,
        elapsed_s: start.elapsed().as_secs_f64(),
    }));
    let dns_result = run_dns_lookup_probe("Hostname DNS", "example.com");
    probes.push(dns_result.clone());
    let _ = tx.send(NetworkWorkerEvent::ProbeCompleted(dns_result));

    let (status, findings) = evaluate_network_diagnosis(&snapshot, &probes);
    snapshot.status = status;
    Ok(NetworkDiagnosisResult {
        snapshot,
        probes,
        findings,
        status,
    })
}

fn run_network_speed_test(
    cancel: Arc<AtomicBool>,
    tx: Sender<NetworkWorkerEvent>,
) -> Result<NetworkSpeedTestResult> {
    let start = Instant::now();
    let total_samples = NETWORK_SPEED_DOWNLOAD_BYTES
        .len()
        .saturating_add(NETWORK_SPEED_UPLOAD_BYTES.len())
        .max(1);
    let mut completed_samples = 0usize;
    let mut samples = Vec::new();
    let mut notes = vec![
        "Single-stream payload test against Cloudflare Speed Test endpoints; results are useful for diagnostics but may differ from a saturation-grade ISP benchmark."
            .to_owned(),
    ];

    let _ = tx.send(NetworkWorkerEvent::Log(
        "Running internet speed test against Cloudflare Speed Test endpoints".to_owned(),
    ));

    for (direction, byte_sizes) in [
        (
            NetworkSpeedDirection::Download,
            NETWORK_SPEED_DOWNLOAD_BYTES,
        ),
        (NetworkSpeedDirection::Upload, NETWORK_SPEED_UPLOAD_BYTES),
    ] {
        for bytes in byte_sizes {
            check_canceled_with(Some(&cancel), "Internet speed test canceled")?;
            let _ = tx.send(NetworkWorkerEvent::Progress(NetworkProgress {
                step: format!(
                    "Testing {} {}",
                    direction.label().to_ascii_lowercase(),
                    format_network_payload_size(*bytes)
                ),
                progress: completed_samples as f32 / total_samples as f32,
                elapsed_s: start.elapsed().as_secs_f64(),
            }));

            match run_network_speed_http_sample(direction, *bytes) {
                Ok(sample) => {
                    completed_samples += 1;
                    let _ = tx.send(NetworkWorkerEvent::SpeedSampleCompleted(sample.clone()));
                    samples.push(sample);
                }
                Err(err) => {
                    completed_samples += 1;
                    let note = format!(
                        "{} sample {} failed: {err:#}",
                        direction.label(),
                        format_network_payload_size(*bytes)
                    );
                    let _ = tx.send(NetworkWorkerEvent::Log(note.clone()));
                    notes.push(note);
                }
            }
        }
    }

    check_canceled_with(Some(&cancel), "Internet speed test canceled")?;
    if samples.is_empty() {
        return Err(anyhow!(
            "internet speed test failed; no download or upload samples completed"
        ));
    }

    let download_mbps = summarize_network_speed(&samples, NetworkSpeedDirection::Download);
    let upload_mbps = summarize_network_speed(&samples, NetworkSpeedDirection::Upload);
    let _ = tx.send(NetworkWorkerEvent::Progress(NetworkProgress {
        step: "Internet speed test complete".to_owned(),
        progress: 1.0,
        elapsed_s: start.elapsed().as_secs_f64(),
    }));

    Ok(NetworkSpeedTestResult {
        download_mbps,
        upload_mbps,
        samples,
        notes,
        elapsed_s: start.elapsed().as_secs_f64(),
    })
}

#[cfg(windows)]
fn run_network_speed_http_sample(
    direction: NetworkSpeedDirection,
    bytes: u64,
) -> Result<NetworkSpeedSample> {
    let script = r#"
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
Add-Type -AssemblyName System.Net.Http
[System.Net.ServicePointManager]::SecurityProtocol = [System.Net.SecurityProtocolType]::Tls12

$direction = '__DIRECTION__'
$bytes = [Int64]__BYTES__
$timeoutMs = [Int32]__TIMEOUT_MS__
$client = [System.Net.Http.HttpClient]::new()
$client.Timeout = [TimeSpan]::FromMilliseconds($timeoutMs)
$client.DefaultRequestHeaders.UserAgent.ParseAdd('Mozilla/5.0 (Windows NT 10.0; Win64; x64) BenchScope/0.1')
$client.DefaultRequestHeaders.Accept.ParseAdd('*/*')
$client.DefaultRequestHeaders.Referrer = [Uri]'https://speed.cloudflare.com/'
$client.DefaultRequestHeaders.CacheControl = [System.Net.Http.Headers.CacheControlHeaderValue]::new()
$client.DefaultRequestHeaders.CacheControl.NoCache = $true
$client.DefaultRequestHeaders.Pragma.ParseAdd('no-cache')
$client.DefaultRequestHeaders.TryAddWithoutValidation('Origin', 'https://speed.cloudflare.com') | Out-Null
$response = $null
$content = $null
$stream = $null
$actualBytes = [Int64]0

try {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    if ($direction -eq 'download') {
        $cacheBust = [Guid]::NewGuid().ToString('N')
        $uri = "https://speed.cloudflare.com/__down?bytes=$bytes&during=download&cacheBust=$cacheBust"
        $response = $client.GetAsync($uri, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).GetAwaiter().GetResult()
        $response.EnsureSuccessStatusCode() | Out-Null
        $stream = $response.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
        $buffer = [byte[]]::new(65536)
        while (($read = $stream.Read($buffer, 0, $buffer.Length)) -gt 0) {
            $actualBytes += $read
        }
    } else {
        $payload = [byte[]]::new($bytes)
        $content = [System.Net.Http.ByteArrayContent]::new($payload)
        $content.Headers.ContentType = [System.Net.Http.Headers.MediaTypeHeaderValue]::Parse('application/octet-stream')
        $cacheBust = [Guid]::NewGuid().ToString('N')
        $response = $client.PostAsync("https://speed.cloudflare.com/__up?during=upload&cacheBust=$cacheBust", $content).GetAwaiter().GetResult()
        $response.EnsureSuccessStatusCode() | Out-Null
        $actualBytes = $bytes
    }
    $sw.Stop()
    $elapsed = $sw.Elapsed.TotalMilliseconds.ToString('0.###', [System.Globalization.CultureInfo]::InvariantCulture)
    "BENCHSCOPE_SPEED`t$direction`t$actualBytes`t$elapsed"
} finally {
    if ($stream -ne $null) { $stream.Dispose() }
    if ($response -ne $null) { $response.Dispose() }
    if ($content -ne $null) { $content.Dispose() }
    $client.Dispose()
}
"#
    .replace("__DIRECTION__", direction.script_value())
    .replace("__BYTES__", &bytes.to_string())
    .replace(
        "__TIMEOUT_MS__",
        &NETWORK_SPEED_REQUEST_TIMEOUT_MS.to_string(),
    );
    let output = run_powershell_sensor_script(&script)?;
    parse_network_speed_sample_output(&output)
}

#[cfg(not(windows))]
fn run_network_speed_http_sample(
    direction: NetworkSpeedDirection,
    bytes: u64,
) -> Result<NetworkSpeedSample> {
    let _ = (direction, bytes);
    Err(anyhow!(
        "internet speed test is currently implemented for Windows"
    ))
}

fn parse_network_speed_sample_output(output: &str) -> Result<NetworkSpeedSample> {
    for line in output.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.first().copied() != Some("BENCHSCOPE_SPEED") {
            continue;
        }
        if fields.len() < 4 {
            return Err(anyhow!("speed test output was incomplete"));
        }
        let direction = parse_network_speed_direction(fields[1])
            .ok_or_else(|| anyhow!("unknown speed test direction '{}'", fields[1]))?;
        let bytes = parse_u64_maybe(fields[2])
            .ok_or_else(|| anyhow!("invalid speed test byte count '{}'", fields[2]))?;
        let elapsed_ms = fields[3]
            .trim()
            .parse::<f64>()
            .with_context(|| format!("invalid speed test elapsed time '{}'", fields[3]))?;
        if elapsed_ms <= 0.0 {
            return Err(anyhow!("speed test elapsed time was not positive"));
        }
        return Ok(NetworkSpeedSample {
            direction,
            bytes,
            elapsed_ms,
            mbps: network_speed_mbps(bytes, elapsed_ms),
        });
    }
    Err(anyhow!("speed test did not return a result row"))
}

fn parse_network_speed_direction(value: &str) -> Option<NetworkSpeedDirection> {
    match value.trim().to_ascii_lowercase().as_str() {
        "download" => Some(NetworkSpeedDirection::Download),
        "upload" => Some(NetworkSpeedDirection::Upload),
        _ => None,
    }
}

fn network_speed_mbps(bytes: u64, elapsed_ms: f64) -> f64 {
    (bytes as f64 * 8.0) / (elapsed_ms / 1000.0) / 1_000_000.0
}

fn summarize_network_speed(
    samples: &[NetworkSpeedSample],
    direction: NetworkSpeedDirection,
) -> Option<f64> {
    samples
        .iter()
        .filter(|sample| sample.direction == direction)
        .map(|sample| sample.mbps)
        .reduce(f64::max)
}

fn run_network_monitor(
    adapter_id: String,
    cancel: Arc<AtomicBool>,
    tx: Sender<NetworkWorkerEvent>,
) -> Result<()> {
    let mut last_gateway_probe = Instant::now()
        .checked_sub(Duration::from_millis(NETWORK_MONITOR_GATEWAY_PROBE_MS))
        .unwrap_or_else(Instant::now);
    while !cancel.load(Ordering::Relaxed) {
        let adapters = detect_network_adapters()?;
        let mut snapshot = find_network_adapter_snapshot(adapters, &adapter_id)
            .ok_or_else(|| anyhow!("selected network adapter was not found"))?;
        let gateway_probe = if last_gateway_probe.elapsed()
            >= Duration::from_millis(NETWORK_MONITOR_GATEWAY_PROBE_MS)
        {
            last_gateway_probe = Instant::now();
            snapshot.gateways.first().map(|gateway| {
                run_icmp_probe("Gateway monitor", gateway, 1, NETWORK_PROBE_TIMEOUT_MS)
            })
        } else {
            None
        };
        let probe_slice = gateway_probe
            .as_ref()
            .map(std::slice::from_ref)
            .unwrap_or(&[]);
        let (status, findings) = evaluate_network_diagnosis(&snapshot, probe_slice);
        snapshot.status = status;
        let signal = WifiSignalSample {
            timestamp_s: unix_timestamp_seconds(),
            signal_percent: snapshot
                .wifi
                .as_ref()
                .and_then(|wifi| wifi.signal_quality_percent),
            link_speed_bps: snapshot.link_speed_bps,
            gateway_latency_ms: gateway_probe
                .as_ref()
                .and_then(|probe| probe.avg_latency_ms),
        };
        let _ = tx.send(NetworkWorkerEvent::MonitorSample(NetworkMonitorSample {
            snapshot,
            signal,
            gateway_probe,
            findings,
        }));

        let sleep_until = Instant::now() + Duration::from_millis(NETWORK_MONITOR_SAMPLE_MS);
        while Instant::now() < sleep_until {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    Ok(())
}

fn run_dns_lookup_probe(label: &str, hostname: &str) -> NetworkProbeResult {
    let start = Instant::now();
    let lookup = (hostname, 443).to_socket_addrs();
    match lookup {
        Ok(addresses) => {
            let count = addresses.count();
            let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
            NetworkProbeResult {
                target_label: label.to_owned(),
                target: hostname.to_owned(),
                probe_kind: NetworkProbeKind::DnsLookup,
                sent: 1,
                received: 1,
                loss_percent: 0.0,
                min_latency_ms: Some(elapsed_ms),
                avg_latency_ms: Some(elapsed_ms),
                max_latency_ms: Some(elapsed_ms),
                jitter_ms: Some(0.0),
                notes: vec![format!("Resolved {count} address(es)")],
            }
        }
        Err(err) => NetworkProbeResult {
            target_label: label.to_owned(),
            target: hostname.to_owned(),
            probe_kind: NetworkProbeKind::DnsLookup,
            sent: 1,
            received: 0,
            loss_percent: 100.0,
            min_latency_ms: None,
            avg_latency_ms: None,
            max_latency_ms: None,
            jitter_ms: None,
            notes: vec![format!("DNS lookup failed: {err}")],
        },
    }
}

fn run_icmp_probe(label: &str, target: &str, count: u32, timeout_ms: u64) -> NetworkProbeResult {
    match run_icmp_probe_impl(label, target, count, timeout_ms) {
        Ok(result) => result,
        Err(err) => NetworkProbeResult {
            target_label: label.to_owned(),
            target: target.to_owned(),
            probe_kind: NetworkProbeKind::Icmp,
            sent: count,
            received: 0,
            loss_percent: 100.0,
            min_latency_ms: None,
            avg_latency_ms: None,
            max_latency_ms: None,
            jitter_ms: None,
            notes: vec![format!("ICMP probe unavailable: {err:#}")],
        },
    }
}

fn run_icmp_probe_cancelable(
    label: &str,
    target: &str,
    count: u32,
    timeout_ms: u64,
    cancel: &AtomicBool,
    tx: &Sender<NetworkWorkerEvent>,
    start: Instant,
    step_index: usize,
    total_steps: usize,
) -> Result<NetworkProbeResult> {
    check_canceled_with(Some(cancel), "Network diagnosis canceled")?;
    match run_icmp_probe_cancelable_impl(
        label,
        target,
        count,
        timeout_ms,
        cancel,
        tx,
        start,
        step_index,
        total_steps,
    ) {
        Ok(result) => Ok(result),
        Err(err) => {
            check_canceled_with(Some(cancel), "Network diagnosis canceled")?;
            Ok(NetworkProbeResult {
                target_label: label.to_owned(),
                target: target.to_owned(),
                probe_kind: NetworkProbeKind::Icmp,
                sent: count,
                received: 0,
                loss_percent: 100.0,
                min_latency_ms: None,
                avg_latency_ms: None,
                max_latency_ms: None,
                jitter_ms: None,
                notes: vec![format!("ICMP probe unavailable: {err:#}")],
            })
        }
    }
}

#[cfg(windows)]
fn run_icmp_probe_impl(
    label: &str,
    target: &str,
    count: u32,
    timeout_ms: u64,
) -> Result<NetworkProbeResult> {
    let count_text = count.to_string();
    let timeout_text = timeout_ms.to_string();
    let output = Command::new("ping")
        .args(["-n", &count_text, "-w", &timeout_text, target])
        .creation_flags(CREATE_NO_WINDOW_RAW)
        .output()
        .with_context(|| format!("failed to start ping for {target}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut rtts = parse_ping_latencies_ms(&stdout);
    if rtts.is_empty() && !output.status.success() && !stderr.trim().is_empty() {
        return Err(anyhow!(stderr.trim().to_owned()));
    }
    if rtts.len() > count as usize {
        rtts.truncate(count as usize);
    }
    Ok(network_probe_from_latencies(
        label,
        target,
        NetworkProbeKind::Icmp,
        count,
        &rtts,
        Vec::new(),
    ))
}

#[cfg(windows)]
fn run_icmp_probe_cancelable_impl(
    label: &str,
    target: &str,
    count: u32,
    timeout_ms: u64,
    cancel: &AtomicBool,
    tx: &Sender<NetworkWorkerEvent>,
    start: Instant,
    step_index: usize,
    total_steps: usize,
) -> Result<NetworkProbeResult> {
    let timeout_text = timeout_ms.to_string();
    let mut rtts = Vec::new();
    let mut notes = Vec::new();
    let safe_count = count.max(1);

    for attempt in 0..safe_count {
        check_canceled_with(Some(cancel), "Network diagnosis canceled")?;
        let completed_fraction = attempt as f32 / safe_count as f32;
        let progress = (step_index as f32 + completed_fraction) / total_steps.max(1) as f32;
        let _ = tx.send(NetworkWorkerEvent::Progress(NetworkProgress {
            step: format!("Testing {label} ({}/{safe_count})", attempt + 1),
            progress,
            elapsed_s: start.elapsed().as_secs_f64(),
        }));

        let output = Command::new("ping")
            .args(["-n", "1", "-w", &timeout_text, target])
            .creation_flags(CREATE_NO_WINDOW_RAW)
            .output()
            .with_context(|| format!("failed to start ping for {target}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        rtts.extend(parse_ping_latencies_ms(&stdout).into_iter().take(1));
        if !output.status.success() && !stderr.trim().is_empty() && notes.is_empty() {
            notes.push(stderr.trim().to_owned());
        }
    }

    check_canceled_with(Some(cancel), "Network diagnosis canceled")?;
    Ok(network_probe_from_latencies(
        label,
        target,
        NetworkProbeKind::Icmp,
        safe_count,
        &rtts,
        notes,
    ))
}

#[cfg(not(windows))]
fn run_icmp_probe_impl(
    label: &str,
    target: &str,
    count: u32,
    _timeout_ms: u64,
) -> Result<NetworkProbeResult> {
    let _ = (label, target, count);
    Err(anyhow!("ICMP probe is currently implemented for Windows"))
}

#[cfg(not(windows))]
fn run_icmp_probe_cancelable_impl(
    label: &str,
    target: &str,
    count: u32,
    timeout_ms: u64,
    cancel: &AtomicBool,
    _tx: &Sender<NetworkWorkerEvent>,
    _start: Instant,
    _step_index: usize,
    _total_steps: usize,
) -> Result<NetworkProbeResult> {
    check_canceled_with(Some(cancel), "Network diagnosis canceled")?;
    run_icmp_probe_impl(label, target, count, timeout_ms)
}

fn parse_ping_latencies_ms(output: &str) -> Vec<f64> {
    output
        .lines()
        .filter_map(parse_ping_line_latency_ms)
        .collect()
}

fn parse_ping_line_latency_ms(line: &str) -> Option<f64> {
    let lower = line.to_ascii_lowercase();
    let index = lower.find("time")?;
    let after = &lower[index + 4..];
    let after = after.trim_start();
    if let Some(rest) = after.strip_prefix('<') {
        let digits = rest
            .chars()
            .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
            .collect::<String>();
        return digits.parse::<f64>().ok().map(|value| value.min(0.5));
    }
    let rest = after.strip_prefix('=')?.trim_start();
    let digits = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>();
    digits.parse::<f64>().ok()
}

fn network_probe_from_latencies(
    label: &str,
    target: &str,
    probe_kind: NetworkProbeKind,
    sent: u32,
    latencies: &[f64],
    notes: Vec<String>,
) -> NetworkProbeResult {
    let received = latencies.len() as u32;
    let loss_percent = if sent == 0 {
        0.0
    } else {
        ((sent.saturating_sub(received)) as f32 / sent as f32) * 100.0
    };
    let (min_latency_ms, avg_latency_ms, max_latency_ms, jitter_ms) = latency_stats(latencies);
    NetworkProbeResult {
        target_label: label.to_owned(),
        target: target.to_owned(),
        probe_kind,
        sent,
        received,
        loss_percent,
        min_latency_ms,
        avg_latency_ms,
        max_latency_ms,
        jitter_ms,
        notes,
    }
}

fn latency_stats(latencies: &[f64]) -> (Option<f64>, Option<f64>, Option<f64>, Option<f64>) {
    if latencies.is_empty() {
        return (None, None, None, None);
    }
    let min = latencies
        .iter()
        .copied()
        .fold(f64::INFINITY, |left, right| left.min(right));
    let max = latencies
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, |left, right| left.max(right));
    let avg = latencies.iter().sum::<f64>() / latencies.len() as f64;
    let jitter = if latencies.len() >= 2 {
        let total_delta = latencies
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .sum::<f64>();
        Some(total_delta / (latencies.len() - 1) as f64)
    } else {
        Some(0.0)
    };
    (Some(min), Some(avg), Some(max), jitter)
}

fn evaluate_network_diagnosis(
    snapshot: &NetworkAdapterSnapshot,
    probes: &[NetworkProbeResult],
) -> (NetworkHealthStatus, Vec<NetworkFinding>) {
    let mut status = initial_network_status(snapshot);
    let mut findings = Vec::new();

    if snapshot.kind == NetworkAdapterKind::Virtual {
        findings.push(NetworkFinding {
            severity: NetworkFindingSeverity::Info,
            title: "Virtual adapter selected".to_owned(),
            detail: "Hardware diagnosis is limited for virtual, VPN, tunnel, or loopback adapters."
                .to_owned(),
            recommended_action: Some(
                "Select the physical Wi-Fi or Ethernet adapter when available.".to_owned(),
            ),
        });
    }

    if !snapshot.connected {
        push_network_finding(
            &mut findings,
            &mut status,
            NetworkFindingSeverity::Critical,
            "Adapter disconnected",
            "The selected adapter is not currently connected.",
            "Check that the adapter is enabled and connected to a network.",
        );
    }

    if snapshot.connected && snapshot.gateways.is_empty() {
        push_network_finding(
            &mut findings,
            &mut status,
            NetworkFindingSeverity::Critical,
            "No default gateway",
            "The adapter has no default gateway, so normal internet routing is unlikely to work.",
            "Reconnect to the network or check DHCP/static IP settings.",
        );
    }

    if snapshot.connected && snapshot.dns_servers.is_empty() {
        push_network_finding(
            &mut findings,
            &mut status,
            NetworkFindingSeverity::Warning,
            "No DNS servers",
            "No DNS servers are configured for this adapter.",
            "Check DHCP, VPN, or manual DNS settings.",
        );
    }

    if snapshot.kind == NetworkAdapterKind::Ethernet && snapshot.connected {
        if let Some(speed) = snapshot.link_speed_bps {
            if speed <= 10_000_000 {
                push_network_finding(
                    &mut findings,
                    &mut status,
                    NetworkFindingSeverity::Critical,
                    "Very low Ethernet link speed",
                    "The Ethernet link negotiated at 10 Mbps.",
                    "Try a different cable and router or switch port.",
                );
            } else if speed < 1_000_000_000 {
                push_network_finding(
                    &mut findings,
                    &mut status,
                    NetworkFindingSeverity::Warning,
                    "Ethernet below gigabit",
                    "The Ethernet link is below 1 Gbps. This can indicate a cable, port, dock, or negotiation issue when gigabit is expected.",
                    "Try a known-good cable and another router or switch port.",
                );
            }
        }
    }

    if let Some(wifi) = &snapshot.wifi {
        if let Some(signal) = wifi.signal_quality_percent {
            if signal < 25 {
                push_network_finding(
                    &mut findings,
                    &mut status,
                    NetworkFindingSeverity::Critical,
                    "Very weak Wi-Fi signal",
                    "Wi-Fi signal quality is below 25%.",
                    "Move closer to the access point or switch to a less congested band.",
                );
            } else if signal < 40 {
                push_network_finding(
                    &mut findings,
                    &mut status,
                    NetworkFindingSeverity::Warning,
                    "Weak Wi-Fi signal",
                    "Wi-Fi signal quality is below 40%.",
                    "Move closer to the access point, reduce interference, or try another band.",
                );
            }
        }
    }

    if let Some(driver) = &snapshot.driver {
        if let Some(device_status) = &driver.device_status {
            if !device_status.is_empty() && device_status != "2" {
                push_network_finding(
                    &mut findings,
                    &mut status,
                    NetworkFindingSeverity::Warning,
                    "Adapter device status is not normal",
                    "Windows reported a nonstandard network device status.",
                    "Check Device Manager for driver or device errors.",
                );
            }
        }
        if driver.version.is_none() {
            push_network_finding(
                &mut findings,
                &mut status,
                NetworkFindingSeverity::Warning,
                "Driver version unavailable",
                "Windows did not expose driver version metadata for this adapter.",
                "Check the adapter driver in Device Manager if symptoms continue.",
            );
        }
    }

    if let Some(counters) = &snapshot.counters {
        let errors = counters
            .inbound_errors
            .unwrap_or(0)
            .saturating_add(counters.outbound_errors.unwrap_or(0));
        let discards = counters
            .inbound_discards
            .unwrap_or(0)
            .saturating_add(counters.outbound_discards.unwrap_or(0));
        if errors > 0 {
            push_network_finding(
                &mut findings,
                &mut status,
                NetworkFindingSeverity::Warning,
                "Interface error counters present",
                "Windows reports packet errors on this adapter.",
                "For Ethernet, try another cable or port. For Wi-Fi, check signal and interference.",
            );
        } else if discards > 0 {
            push_network_finding(
                &mut findings,
                &mut status,
                NetworkFindingSeverity::Info,
                "Interface discard counters present",
                "Windows reports discarded packets on this adapter.",
                "Watch whether discard counters increase during continuous monitoring.",
            );
        }
    }

    for probe in probes {
        if probe.loss_percent >= 5.0 {
            push_network_finding(
                &mut findings,
                &mut status,
                NetworkFindingSeverity::Critical,
                &format!("Packet loss to {}", probe.target_label),
                &format!(
                    "{} lost {} of {} probe(s).",
                    probe.target,
                    probe.sent.saturating_sub(probe.received),
                    probe.sent
                ),
                "If this is the gateway, focus on local Wi-Fi/cable/router. If only public targets fail, check upstream internet.",
            );
        } else if probe.loss_percent >= 1.0 {
            push_network_finding(
                &mut findings,
                &mut status,
                NetworkFindingSeverity::Warning,
                &format!("Minor packet loss to {}", probe.target_label),
                &format!(
                    "{} packet loss detected.",
                    format_loss_percent(probe.loss_percent)
                ),
                "Run continuous monitor to see whether the loss is intermittent.",
            );
        }
        if probe
            .jitter_ms
            .is_some_and(|jitter| jitter >= 100.0 && probe.received > 1)
        {
            push_network_finding(
                &mut findings,
                &mut status,
                NetworkFindingSeverity::Critical,
                &format!("Severe jitter to {}", probe.target_label),
                "Latency varies sharply between probes.",
                "Check Wi-Fi stability, congestion, and router load.",
            );
        } else if probe
            .jitter_ms
            .is_some_and(|jitter| jitter >= 30.0 && probe.received > 1)
        {
            push_network_finding(
                &mut findings,
                &mut status,
                NetworkFindingSeverity::Warning,
                &format!("High jitter to {}", probe.target_label),
                "Latency variation is high enough to affect calls, games, or remote desktop.",
                "Check for congestion, weak Wi-Fi, or bufferbloat.",
            );
        }
    }

    let gateway_ok = probes
        .iter()
        .find(|probe| probe.target_label.contains("Gateway"))
        .is_some_and(|probe| probe.loss_percent < 1.0);
    let public_ip_ok = probes
        .iter()
        .find(|probe| probe.target_label == "Public IP")
        .is_some_and(|probe| probe.loss_percent < 1.0);
    let dns_failed = probes
        .iter()
        .find(|probe| probe.probe_kind == NetworkProbeKind::DnsLookup)
        .is_some_and(|probe| probe.received == 0);
    if gateway_ok && public_ip_ok && dns_failed {
        push_network_finding(
            &mut findings,
            &mut status,
            NetworkFindingSeverity::Critical,
            "Likely DNS issue",
            "Gateway and public IP probes work, but hostname resolution failed.",
            "Try another DNS server or renew DHCP settings.",
        );
    }

    if findings.is_empty() {
        findings.push(NetworkFinding {
            severity: NetworkFindingSeverity::Info,
            title: "No obvious network hardware symptoms".to_owned(),
            detail: "The selected adapter, gateway, DNS, packet loss, and latency checks did not expose a clear hardware symptom.".to_owned(),
            recommended_action: Some("Use continuous monitor if the problem is intermittent.".to_owned()),
        });
    }

    (status, findings)
}

fn push_network_finding(
    findings: &mut Vec<NetworkFinding>,
    status: &mut NetworkHealthStatus,
    severity: NetworkFindingSeverity,
    title: &str,
    detail: &str,
    recommended_action: &str,
) {
    *status = combine_network_status(*status, severity);
    findings.push(NetworkFinding {
        severity,
        title: title.to_owned(),
        detail: detail.to_owned(),
        recommended_action: Some(recommended_action.to_owned()),
    });
}

fn combine_network_status(
    current: NetworkHealthStatus,
    severity: NetworkFindingSeverity,
) -> NetworkHealthStatus {
    match severity {
        NetworkFindingSeverity::Critical => NetworkHealthStatus::Critical,
        NetworkFindingSeverity::Warning => {
            if current == NetworkHealthStatus::Critical {
                current
            } else {
                NetworkHealthStatus::Caution
            }
        }
        NetworkFindingSeverity::Info => current,
    }
}

fn write_network_diagnostic_report(state: &NetworkDiagnosticState) -> Result<PathBuf> {
    let dir = std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir());
    let path = dir.join(format!(
        "benchscope-network-diagnostic-{}.md",
        unix_timestamp_seconds()
    ));
    fs::write(&path, network_diagnostic_report_markdown(state))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

fn network_diagnostic_report_markdown(state: &NetworkDiagnosticState) -> String {
    let mut report = String::new();
    report.push_str("# BenchScope Network Diagnostic Report\n\n");
    report.push_str(&format!("- Timestamp: {}\n", unix_timestamp_seconds()));
    report.push_str(&format!("- Status: {}\n", state.status));
    if let Some(adapter) = state.selected_adapter() {
        report.push_str("\n## Adapter\n\n");
        report.push_str(&format!("- Name: {}\n", adapter.name));
        report.push_str(&format!("- Description: {}\n", adapter.description));
        report.push_str(&format!("- Type: {}\n", adapter.kind.label()));
        report.push_str(&format!("- Connected: {}\n", adapter.connected));
        report.push_str(&format!(
            "- Link speed: {}\n",
            format_link_speed(adapter.link_speed_bps)
        ));
        report.push_str(&format!(
            "- IPv4: {}\n",
            empty_list_label(&adapter.ipv4_addresses)
        ));
        report.push_str(&format!(
            "- IPv6: {}\n",
            empty_list_label(&adapter.ipv6_addresses)
        ));
        report.push_str(&format!(
            "- Gateway: {}\n",
            empty_list_label(&adapter.gateways)
        ));
        report.push_str(&format!(
            "- DNS: {}\n",
            empty_list_label(&adapter.dns_servers)
        ));
        if let Some(wifi) = &adapter.wifi {
            report.push_str(&format!(
                "- Wi-Fi SSID: {}\n",
                wifi.ssid.as_deref().unwrap_or("N/A")
            ));
            report.push_str(&format!(
                "- Wi-Fi signal: {}\n",
                wifi.signal_quality_percent
                    .map(|value| format!("{value}%"))
                    .unwrap_or_else(|| "N/A".to_owned())
            ));
            report.push_str(&format!(
                "- Wi-Fi PHY/channel: {} / {}\n",
                wifi.phy_type.as_deref().unwrap_or("N/A"),
                wifi.channel
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "N/A".to_owned())
            ));
        }
        if let Some(driver) = &adapter.driver {
            report.push_str(&format!(
                "- Driver: {} {} ({})\n",
                driver.provider.as_deref().unwrap_or("N/A"),
                driver.version.as_deref().unwrap_or("N/A"),
                driver.date.as_deref().unwrap_or("N/A")
            ));
        }
        if let Some(counters) = &adapter.counters {
            report.push_str(&format!(
                "- Bytes received/sent: {} / {}\n",
                format_optional_bytes(counters.bytes_received),
                format_optional_bytes(counters.bytes_sent)
            ));
            report.push_str(&format!(
                "- Packets received/sent: {} / {}\n",
                format_optional_count(counters.packets_received),
                format_optional_count(counters.packets_sent)
            ));
            report.push_str(&format!(
                "- Errors in/out: {} / {}; discards in/out: {} / {}\n",
                format_optional_count(counters.inbound_errors),
                format_optional_count(counters.outbound_errors),
                format_optional_count(counters.inbound_discards),
                format_optional_count(counters.outbound_discards)
            ));
        }
        for note in &adapter.provider_notes {
            report.push_str(&format!("- Provider note: {note}\n"));
        }
    }

    if let Some(speed) = &state.speed_result {
        report.push_str("\n## Internet Speed Test\n\n");
        report.push_str(&format!(
            "- Download: {}\n",
            format_network_speed_mbps(speed.download_mbps)
        ));
        report.push_str(&format!(
            "- Upload: {}\n",
            format_network_speed_mbps(speed.upload_mbps)
        ));
        report.push_str(&format!("- Elapsed: {}\n", format_elapsed(speed.elapsed_s)));
        for note in &speed.notes {
            report.push_str(&format!("- Note: {note}\n"));
        }
        report.push_str("\n| Direction | Payload | Elapsed | Throughput |\n");
        report.push_str("| --- | ---: | ---: | ---: |\n");
        for sample in &speed.samples {
            report.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                sample.direction.label(),
                format_network_payload_size(sample.bytes),
                format_elapsed(sample.elapsed_ms / 1000.0),
                format_network_speed_mbps(Some(sample.mbps))
            ));
        }
    }

    report.push_str("\n## Findings\n\n");
    for finding in &state.findings {
        report.push_str(&format!(
            "- [{}] {}: {}",
            finding.severity.label(),
            finding.title,
            finding.detail
        ));
        if let Some(action) = &finding.recommended_action {
            report.push_str(&format!(" Recommended action: {action}"));
        }
        report.push('\n');
    }

    report.push_str("\n## Probe Results\n\n");
    report.push_str("| Target | Probe | Sent | Received | Loss | Avg | Jitter | Notes |\n");
    report.push_str("| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |\n");
    for probe in &state.probe_results {
        report.push_str(&format!(
            "| {} ({}) | {} | {} | {} | {} | {} | {} | {} |\n",
            probe.target_label,
            probe.target,
            probe.probe_kind.label(),
            probe.sent,
            probe.received,
            format_loss_percent(probe.loss_percent),
            format_optional_latency(probe.avg_latency_ms),
            format_optional_latency(probe.jitter_ms),
            probe.notes.join(", ")
        ));
    }

    if !state.signal_history.is_empty() {
        report.push_str("\n## Signal History\n\n");
        report.push_str("| Time | Signal | Link speed | Gateway latency |\n");
        report.push_str("| ---: | ---: | ---: | ---: |\n");
        for sample in &state.signal_history {
            report.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                sample.timestamp_s,
                sample
                    .signal_percent
                    .map(|value| format!("{value}%"))
                    .unwrap_or_else(|| "N/A".to_owned()),
                format_link_speed(sample.link_speed_bps),
                format_optional_latency(sample.gateway_latency_ms)
            ));
        }
    }

    report
}

fn unix_timestamp_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn empty_list_label(values: &[String]) -> String {
    if values.is_empty() {
        "N/A".to_owned()
    } else {
        values.join(", ")
    }
}

fn format_optional_count(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "N/A".to_owned())
}

fn format_link_speed(value: Option<u64>) -> String {
    match value {
        Some(value) if value >= 1_000_000_000 => {
            format!("{:.1} Gbps", value as f64 / 1_000_000_000.0)
        }
        Some(value) if value >= 1_000_000 => format!("{:.0} Mbps", value as f64 / 1_000_000.0),
        Some(value) if value >= 1_000 => format!("{:.0} Kbps", value as f64 / 1_000.0),
        Some(value) => format!("{value} bps"),
        None => "N/A".to_owned(),
    }
}

fn format_network_payload_size(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.0} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes} B")
    }
}

fn format_network_speed_mbps(value: Option<f64>) -> String {
    match value {
        Some(value) if value >= 1000.0 => format!("{:.2} Gbps", value / 1000.0),
        Some(value) if value >= 100.0 => format!("{value:.0} Mbps"),
        Some(value) if value >= 10.0 => format!("{value:.1} Mbps"),
        Some(value) => format!("{value:.2} Mbps"),
        None => "N/A".to_owned(),
    }
}

fn format_loss_percent(value: f32) -> String {
    if value >= 10.0 {
        format!("{value:.0}%")
    } else {
        format!("{value:.1}%")
    }
}
