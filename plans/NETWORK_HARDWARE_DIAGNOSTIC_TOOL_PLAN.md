# Network Hardware Diagnostic Tool Plan

## Goal

Add a network-focused hardware diagnostic tool to BenchScope for troubleshooting Wi-Fi and Ethernet adapter problems.

This should be a separate main-menu option from performance benchmarks and storage tools. The tool should answer:

- Is my network adapter healthy and active?
- Is the link speed what it should be?
- Is the connection losing packets?
- Is latency stable?
- Is Wi-Fi signal weak or unstable?
- Is DNS or gateway connectivity broken?
- Does the evidence suggest a bad cable, bad port, weak Wi-Fi, driver issue, or router/upstream problem?

Main menu tools should include:

- Matrix CPU/GPU Benchmark
- Matrix Stress Test
- Drive Benchmark
- SSD / HDD Health Checker
- RAM Tester
- Network Hardware Diagnostic Tool

## User Experience

The network diagnostic should feel like a practical troubleshooting console.

Top controls:

- Back button
- Adapter selector
- Refresh adapters button
- Run quick diagnosis
- Start continuous monitor
- Stop monitor
- Export report

Main sections:

- Adapter health summary
- Link and driver details
- Gateway, DNS, and internet reachability tests
- Packet loss and latency results
- Wi-Fi signal strength history
- Ethernet cable and link-quality symptoms
- Findings and recommended actions
- Diagnostic log

The first screen should show useful adapter information immediately after selecting an adapter. The user should not need to run a long test to see link speed, connection state, IP addresses, driver version, or Wi-Fi signal strength.

## Main Menu Integration

Add a new app view:

```rust
enum AppView {
    MainMenu,
    MatrixBenchmark,
    MatrixStress,
    DriveBenchmark,
    StorageHealth,
    RamTester,
    NetworkDiagnostic,
}
```

The main menu should show a new button:

```text
Network Hardware Diagnostic Tool
```

Selecting it switches to the network diagnostic view.

Back behavior:

- If no diagnostic is running, Back returns to the main menu.
- If a test or continuous monitor is running, Back asks whether to stop diagnostics and return.
- No worker thread should continue collecting network data after leaving the tool.

## Supported Adapter Types

Initial support should focus on Windows network adapters:

- Wi-Fi adapters
- Ethernet adapters
- USB Ethernet adapters
- Docking-station Ethernet adapters
- Virtual adapters, shown but clearly labeled
- VPN adapters, shown but not used for hardware diagnosis by default

The UI should prioritize physical, connected adapters first. Virtual, loopback, Bluetooth PAN, Hyper-V, WSL, VPN, and disabled adapters should be grouped lower or hidden behind an "include virtual adapters" toggle.

## Adapter Health Summary

The tool should compute and display a simple status:

- Good
- Caution
- Critical
- Unknown

Summary fields:

- Adapter name
- Interface description
- Adapter type: Wi-Fi, Ethernet, Virtual, Unknown
- Connection state
- Link speed
- MAC address
- IPv4 address
- IPv6 address
- Default gateway
- DNS servers
- DHCP enabled
- Driver provider
- Driver version
- Driver date
- Wi-Fi SSID, if connected
- Wi-Fi signal strength, if available
- Wi-Fi band and channel, if available
- Bytes sent and received
- Packet/error counters, where available

Recommended visual rules:

- Good: connected, expected link speed, no packet loss, stable latency, DNS and gateway pass.
- Caution: weak Wi-Fi, lower-than-expected link speed, intermittent loss, high jitter, old or missing driver metadata, DNS fallback failure, or minor interface errors.
- Critical: disconnected adapter, no gateway, severe packet loss, DNS failure, gateway unreachable, likely bad Ethernet cable, or driver/device error state.
- Unknown: adapter details or test results are unavailable.

## Data Collection Sources

Use multiple sources because Windows exposes adapter information unevenly across hardware.

Recommended provider order:

1. Windows IP Helper API for adapters, addresses, gateways, DNS servers, and counters
2. Windows WLAN API for Wi-Fi SSID, signal quality, channel, PHY type, and radio state
3. WMI / CIM for driver version, driver date, PNP device state, and adapter metadata
4. PowerShell command fallback only if direct APIs are insufficient
5. Standard network probes for latency, packet loss, DNS, gateway, and internet tests

Possible Windows sources:

- `GetAdaptersAddresses`
- `GetIfEntry2`
- `GetIfTable2`
- `WlanOpenHandle`
- `WlanEnumInterfaces`
- `WlanQueryInterface`
- `Win32_NetworkAdapter`
- `Win32_NetworkAdapterConfiguration`
- `MSFT_NetAdapter`
- `MSFT_NetAdapterStatisticsSettingData`
- `MSFT_NetAdapterRdmaSettingData`, later if useful

Provider results should be normalized into one internal model so the UI does not care where the data came from.

## Adapter Inventory

The adapter list should show:

- Friendly name
- Connection state
- Adapter type
- Link speed
- Current IP address
- Wi-Fi signal strength or Ethernet label
- Hardware/virtual badge

Selection behavior:

- Default to the active physical adapter with a default gateway.
- If multiple adapters are active, prefer Ethernet over Wi-Fi only when both have a gateway and the Ethernet link is physical.
- If only virtual adapters are active, show them but mark hardware diagnosis as limited.
- Disabled adapters should be visible in a collapsed section or behind a toggle.

## Link Speed Check

The link panel should show:

- Current link speed
- Receive link speed, if separately available
- Transmit link speed, if separately available
- Connection state
- Adapter media type
- Duplex mode, if available
- MTU
- Interface metric

Ethernet interpretation:

- 10 Mbps: Critical for modern wired networks unless expected.
- 100 Mbps: Caution when gigabit hardware is expected.
- 1 Gbps or higher: Usually Good.
- Frequent link down/up events: Caution or Critical depending on frequency.

Wi-Fi interpretation:

- Low link speed plus weak signal suggests range or interference.
- Good signal but poor throughput/latency suggests congestion, router issue, driver problem, or ISP/upstream issue.
- Rapid link-speed swings suggest weak signal, roaming, interference, or power saving.

The first version should avoid claiming a link speed is wrong unless the evidence is strong. Use wording such as "may indicate" and include context.

## Packet Loss Test

The quick diagnosis should run a packet loss test against multiple targets:

1. Default gateway
2. Primary DNS server
3. Public internet target by IP, such as `1.1.1.1` or `8.8.8.8`
4. Public hostname, such as `example.com`, for DNS plus internet path validation

Recommended quick profile:

- Gateway: 10 probes
- DNS server: 10 probes
- Public IP: 10 probes
- Public hostname: 5 probes
- Per-probe timeout: 1000 ms

Recommended thorough profile:

- Gateway: 50 probes
- DNS server: 50 probes
- Public IP: 50 probes
- Public hostname: 20 probes
- Per-probe timeout: 1500 ms

Metrics:

- Sent probes
- Received responses
- Packet loss percentage
- Minimum latency
- Average latency
- Maximum latency
- Jitter
- Timeout count

Implementation note:

- ICMP requires platform-specific behavior and may be blocked by firewalls.
- If ICMP is blocked, fall back to TCP connect timing to ports such as DNS `53`, HTTPS `443`, or gateway HTTP/HTTPS when appropriate.
- Clearly label the probe type used.

## Latency Test

Latency results should separate local network latency from wider internet latency.

Targets:

- Gateway latency: local router/access point path
- DNS server latency: resolver path
- Public IP latency: internet path without DNS lookup
- Public hostname latency: internet path plus DNS dependency

Interpretation examples:

- High gateway latency: local Wi-Fi, cable, switch, router, adapter, or driver issue.
- Low gateway latency but high public latency: ISP/upstream or remote network issue.
- Low IP latency but hostname failure: DNS issue.
- High jitter: unstable Wi-Fi, bufferbloat, congestion, or adapter power-saving behavior.

Suggested thresholds:

- Gateway average under 5 ms: Good for wired, usually Good for strong Wi-Fi.
- Gateway average 5-30 ms: Caution for wired, acceptable-to-caution for Wi-Fi.
- Gateway average over 30 ms: Caution or Critical depending on loss and jitter.
- Packet loss over 1%: Caution.
- Packet loss over 5%: Critical.
- Jitter over 30 ms: Caution.
- Jitter over 100 ms: Critical.

Thresholds should be configurable later because network expectations differ.

## Wi-Fi Signal Strength History

For Wi-Fi adapters, show a small rolling history chart.

Fields:

- Current signal quality percentage
- Estimated RSSI dBm, if available
- SSID
- BSSID, optional and hidden by default
- PHY type, such as 802.11n/ac/ax/be
- Band, such as 2.4 GHz, 5 GHz, or 6 GHz
- Channel, if available
- Link speed
- Samples over time

Sampling:

- Quick diagnosis: collect during the test.
- Continuous monitor: sample every 1 second by default.
- Keep the last 5 minutes in memory for the UI.
- Export summarized history, not an unbounded raw stream.

Wi-Fi symptom detection:

- Signal under 40%: weak Wi-Fi.
- Signal swings greater than 25 percentage points: unstable Wi-Fi or roaming.
- Strong signal plus high loss: interference, congestion, router issue, or driver issue.
- Low link speed with decent signal: adapter/router capability mismatch, bad band selection, power saving, or driver problem.

## Driver Version Check

The driver panel should show:

- Driver provider
- Driver version
- Driver date
- PNP device ID
- Device status
- Hardware IDs, optional advanced detail

Diagnostic rules:

- Missing driver metadata: Unknown.
- Device reports error status: Critical.
- Very old driver date: Caution.
- Microsoft generic driver on vendor hardware: Caution when known vendor details are present.
- Recent driver but unstable symptoms: do not blame the driver automatically; list it as one possible cause.

The app should not require internet access to determine whether a driver is "latest" in the first release. A future version can optionally compare vendor versions, but offline diagnostics should remain useful.

## DNS and Gateway Test

Gateway tests:

- Confirm a default gateway exists.
- Ping or TCP-probe the gateway.
- Check gateway latency and packet loss.
- Check ARP/neighbor reachability if available.

DNS tests:

- Confirm DNS servers exist.
- Resolve a known stable hostname.
- Time the DNS lookup.
- Try each configured DNS server when possible.
- Compare direct public IP reachability with hostname reachability.

Interpretation:

- No gateway: Critical for normal internet access.
- Gateway unreachable: local network issue.
- Gateway reachable but DNS fails: DNS configuration or resolver issue.
- DNS succeeds but public IP fails: upstream internet path issue.
- Public IP succeeds but hostname fails: DNS-specific problem.

## Bad Cable and Weak Wi-Fi Symptom Detection

The tool should infer symptoms from multiple signals instead of using one metric alone.

Possible bad Ethernet cable or port symptoms:

- Ethernet link negotiates at 10 or 100 Mbps when gigabit is expected.
- Link repeatedly disconnects and reconnects.
- Interface error counters increase during a test.
- Gateway packet loss appears on Ethernet while Wi-Fi or another adapter is stable.
- Duplex mismatch indicators, if available.

Recommended message:

```text
Symptoms are consistent with a bad cable, damaged port, or negotiation issue. Try a different Ethernet cable and router/switch port.
```

Possible weak Wi-Fi symptoms:

- Signal strength below 40%.
- Signal fluctuates sharply during a short monitor window.
- Gateway latency and jitter are high.
- Packet loss occurs to the gateway.
- Link speed drops repeatedly.

Recommended message:

```text
Symptoms are consistent with weak or unstable Wi-Fi. Move closer to the access point, reduce interference, or try a different band.
```

Possible DNS issue symptoms:

- Gateway and public IP probes succeed.
- Hostname resolution fails or is slow.
- One DNS server fails while another works.

Possible router/upstream issue symptoms:

- Gateway is stable.
- DNS works.
- Public IP or hostname tests lose packets or have high latency.

Possible adapter/driver issue symptoms:

- Device status reports errors.
- Interface resets during test.
- Link state changes without user action.
- All local network targets fail while other adapters work.
- Driver metadata is missing or unusually old.

## Continuous Monitor

Add a monitor mode for intermittent issues.

Controls:

- Start monitor
- Stop monitor
- Sample interval selector later
- Clear history

Default behavior:

- Sample adapter state every 1 second.
- Probe gateway every 5 seconds.
- Probe DNS and public IP every 15 seconds.
- Track Wi-Fi signal each second when available.
- Track link speed changes.
- Track packet/error counter deltas.

The monitor should show:

- Current status
- Last status change
- Link up/down events
- Packet loss trend
- Latency trend
- Wi-Fi signal trend
- New findings detected during the session

Continuous monitor should be cancelable immediately from the UI.

## Export Diagnostic Report

Support exporting a Markdown report first.

Default filename:

```text
benchscope-network-diagnostic-YYYYMMDD-HHMMSS.md
```

Report contents:

- App name and version
- Report timestamp
- Selected adapter identity
- Connection state
- Link speed and adapter type
- IP, gateway, DNS, and DHCP summary
- Driver version and date
- Wi-Fi SSID, signal summary, and channel details when available
- Packet loss results
- Latency and jitter results
- DNS and gateway test results
- Cable/Wi-Fi symptom analysis
- Findings and recommended actions
- Provider/source notes
- Probe limitations, such as ICMP blocked

The report should avoid exposing sensitive details unnecessarily. MAC address, BSSID, and full hardware IDs can be included only in an advanced section or behind an option.

## UI Layout

Suggested layout:

```text
+------------------------------------------------------------------+
| Back  Network Hardware Diagnostic Tool  Adapter: [dropdown] [Refresh] |
+------------------------------------------------------------------+
| Overall: Good/Caution/Critical/Unknown                           |
| Adapter name, type, state, link speed, IP, gateway, DNS           |
| Driver provider, version, date                                    |
+------------------------------------------------------------------+
| [Run Quick Diagnosis] [Start Monitor] [Stop] [Export Report]      |
| Progress bar / current step                                       |
+------------------------------------------------------------------+
| Findings                                                         |
| - Gateway reachable: yes                                          |
| - DNS healthy: yes                                                |
| - Packet loss: 0%                                                 |
| - Wi-Fi signal: 72%, stable                                       |
+------------------------------------------------------------------+
| Latency and packet loss table                                     |
+------------------------------------------------------------------+
| Wi-Fi signal history / Ethernet link events                       |
+------------------------------------------------------------------+
| Diagnostic log                                                    |
+------------------------------------------------------------------+
```

The findings section should be visible without scrolling.

## Data Model

Suggested internal types:

```rust
enum NetworkHealthStatus {
    Good,
    Caution,
    Critical,
    Unknown,
}

enum NetworkAdapterKind {
    Wifi,
    Ethernet,
    Virtual,
    Loopback,
    Other,
    Unknown,
}

enum NetworkFindingSeverity {
    Info,
    Warning,
    Critical,
}

enum NetworkProbeKind {
    Icmp,
    TcpConnect,
    DnsLookup,
}

struct NetworkAdapterIdentity {
    id: String,
    name: String,
    description: String,
    kind: NetworkAdapterKind,
    is_physical: bool,
    mac_address: Option<String>,
    mtu: Option<u32>,
}

struct NetworkAdapterSnapshot {
    identity: NetworkAdapterIdentity,
    status: NetworkHealthStatus,
    connected: bool,
    link_speed_bps: Option<u64>,
    ipv4_addresses: Vec<String>,
    ipv6_addresses: Vec<String>,
    gateways: Vec<String>,
    dns_servers: Vec<String>,
    dhcp_enabled: Option<bool>,
    driver: Option<NetworkDriverInfo>,
    wifi: Option<WifiSnapshot>,
    counters: Option<NetworkCounters>,
    findings: Vec<NetworkFinding>,
    provider_notes: Vec<String>,
}

struct NetworkDriverInfo {
    provider: Option<String>,
    version: Option<String>,
    date: Option<String>,
    device_status: Option<String>,
}

struct WifiSnapshot {
    ssid: Option<String>,
    signal_quality_percent: Option<u8>,
    rssi_dbm: Option<i32>,
    phy_type: Option<String>,
    band: Option<String>,
    channel: Option<u32>,
    rx_link_speed_bps: Option<u64>,
    tx_link_speed_bps: Option<u64>,
}

struct NetworkCounters {
    bytes_sent: u64,
    bytes_received: u64,
    packets_sent: Option<u64>,
    packets_received: Option<u64>,
    inbound_errors: Option<u64>,
    outbound_errors: Option<u64>,
    inbound_discards: Option<u64>,
    outbound_discards: Option<u64>,
}

struct NetworkProbeResult {
    target_label: String,
    target: String,
    probe_kind: NetworkProbeKind,
    sent: u32,
    received: u32,
    loss_percent: f32,
    min_latency_ms: Option<f32>,
    avg_latency_ms: Option<f32>,
    max_latency_ms: Option<f32>,
    jitter_ms: Option<f32>,
    notes: Vec<String>,
}

struct NetworkFinding {
    severity: NetworkFindingSeverity,
    title: String,
    detail: String,
    recommended_action: Option<String>,
}
```

## Worker Events

Network diagnostics should run off the UI thread.

Suggested event model:

```rust
enum NetworkDiagnosticEvent {
    AdapterListUpdated(Vec<NetworkAdapterIdentity>),
    SnapshotUpdated(NetworkAdapterSnapshot),
    ProbeProgress(NetworkProbeProgress),
    ProbeCompleted(NetworkProbeResult),
    DiagnosisCompleted(NetworkDiagnosisResult),
    MonitorSample(NetworkMonitorSample),
    ReportExported(PathBuf),
    Log(String),
    Failed(String),
}
```

Progress should include:

- Current diagnostic step
- Current target
- Probe count
- Overall progress
- Elapsed time
- Cancellation status

Monitor samples should include:

- Timestamp
- Link state
- Link speed
- Wi-Fi signal quality
- Gateway latency
- Packet loss summary
- Counter deltas
- Newly detected findings

## Implementation Phases

### Phase 1: Planning and Menu Shell

Tasks:

- Add this plan file.
- Add `NetworkDiagnostic` to the main app view enum.
- Add `Network Hardware Diagnostic Tool` to the main menu.
- Add a placeholder diagnostic screen with Back navigation.

Acceptance criteria:

- App opens to main menu.
- New network diagnostic option appears.
- Selecting it opens the network diagnostic screen.
- Back returns to the main menu.

### Phase 2: Adapter Inventory

Tasks:

- Enumerate network adapters.
- Identify physical, virtual, Wi-Fi, and Ethernet adapters.
- Display connection state, link speed, IP addresses, gateway, DNS, and MAC address.
- Select the active physical adapter by default.

Acceptance criteria:

- User can select an adapter.
- Active adapters are easy to find.
- Virtual adapters are clearly labeled.
- Missing fields show `N/A` instead of crashing.

### Phase 3: Driver and Wi-Fi Details

Tasks:

- Query driver provider, version, date, and device status.
- Query Wi-Fi SSID, signal quality, PHY type, channel, and link speeds.
- Add provider notes when data cannot be read.

Acceptance criteria:

- Wi-Fi adapters show signal strength when available.
- Ethernet adapters do not show irrelevant Wi-Fi controls.
- Driver details are visible for hardware adapters.
- Unsupported details are reported honestly.

### Phase 4: Gateway, DNS, Packet Loss, and Latency Tests

Tasks:

- Implement gateway probe.
- Implement DNS server probe.
- Implement public IP probe.
- Implement hostname resolution test.
- Calculate loss, latency, and jitter.
- Add ICMP blocked fallback behavior where practical.

Acceptance criteria:

- Quick diagnosis runs without blocking the UI.
- Results distinguish gateway, DNS, public IP, and hostname tests.
- Loss and latency metrics are shown clearly.
- Tests can be canceled.

### Phase 5: Health Scoring and Symptom Detection

Tasks:

- Implement Good/Caution/Critical/Unknown scoring.
- Detect weak Wi-Fi symptoms.
- Detect possible bad Ethernet cable or port symptoms.
- Detect likely DNS issue symptoms.
- Detect likely upstream/router symptoms.
- Detect adapter/driver warning symptoms.

Acceptance criteria:

- Findings are based on multiple signals where possible.
- The UI explains evidence behind each finding.
- Recommendations are actionable and cautious.
- Healthy networks are not over-warned.

### Phase 6: Continuous Monitor

Tasks:

- Add monitor start/stop controls.
- Sample adapter state and Wi-Fi signal.
- Probe gateway periodically.
- Track link changes, signal history, and packet loss trend.
- Keep bounded in-memory history.

Acceptance criteria:

- Monitor can run and stop cleanly.
- Signal history updates over time for Wi-Fi.
- Link speed changes are logged.
- Leaving the tool stops the monitor.

### Phase 7: Report Export

Tasks:

- Export Markdown reports.
- Include adapter snapshot, tests, findings, and provider notes.
- Include signal history summary when available.
- Add export success/failure messages.

Acceptance criteria:

- User can export a readable `.md` report.
- Report includes the key visible diagnostic results.
- Report handles missing data honestly.
- Sensitive identifiers are minimized by default.

## Testing Plan

Automated tests:

- Adapter health status calculation.
- Packet loss percentage calculation.
- Latency min/avg/max/jitter calculation.
- DNS/gateway result interpretation.
- Weak Wi-Fi symptom detection.
- Bad cable symptom detection.
- Driver warning interpretation.
- Markdown report generation.
- Monitor history bounds.
- Cancellation flag behavior.

Manual tests:

- Connected Ethernet adapter.
- Connected Wi-Fi adapter with strong signal.
- Connected Wi-Fi adapter with weak signal.
- Disconnected Ethernet adapter.
- Adapter with no default gateway.
- DNS misconfiguration, if safely reproducible.
- ICMP blocked environment.
- VPN active while physical adapter is active.
- USB Ethernet adapter or dock.
- Start monitor, wait for samples, then stop.
- Press Back during a running quick diagnosis.
- Export report before and after running tests.

## Risks

### ICMP May Be Blocked

Risk:

- Ping-style packet loss tests may fail because ICMP is blocked, not because the network is broken.

Mitigation:

- Fall back to TCP connect probes where practical.
- Label the probe type.
- Show a note when ICMP appears blocked.

### False Cable Diagnosis

Risk:

- Low Ethernet link speed can be caused by adapter settings, switch limits, or router ports, not only a bad cable.

Mitigation:

- Phrase findings as symptoms.
- Recommend trying a different cable and port instead of claiming certainty.
- Combine link speed, link flap, and error counters before raising severity.

### Wi-Fi Environment Variability

Risk:

- Wi-Fi signal and latency can change quickly due to interference, roaming, or congestion.

Mitigation:

- Show history instead of one sample.
- Use cautious interpretation.
- Let users run continuous monitor for intermittent problems.

### Privacy

Risk:

- Reports may include MAC addresses, SSIDs, BSSIDs, IP addresses, or hardware IDs.

Mitigation:

- Keep advanced identifiers optional where possible.
- Avoid full hardware IDs in default reports.
- Clearly label report contents before export.

### Provider Availability

Risk:

- Some adapter details are unavailable depending on driver, permissions, or Windows version.

Mitigation:

- Use multiple providers.
- Show `Unknown` or `N/A` honestly.
- Include provider notes in the UI and report.

## Recommended Defaults

- Initial selected adapter: active physical adapter with default gateway
- Quick diagnosis: gateway, DNS, public IP, and hostname tests
- Continuous monitor interval: 1 second adapter sampling
- Gateway monitor probe interval: 5 seconds
- DNS/public probe interval: 15 seconds
- Wi-Fi history window: last 5 minutes
- Packet loss warning: over 1%
- Packet loss critical: over 5%
- Weak Wi-Fi warning: under 40% signal quality
- Report format: Markdown

## Definition of Done

The Network Hardware Diagnostic Tool is ready when:

- It appears as its own main-menu option.
- It has working Back navigation.
- It lists network adapters and clearly labels physical versus virtual adapters.
- It shows adapter health, connection state, link speed, IP, gateway, DNS, and driver details.
- It shows Wi-Fi signal strength and signal history for Wi-Fi adapters.
- It runs cancelable gateway, DNS, packet loss, and latency tests.
- It distinguishes local gateway issues from DNS and upstream internet issues.
- It detects symptoms consistent with bad Ethernet cables or weak Wi-Fi.
- It provides cautious, actionable findings.
- It exports a Markdown diagnostic report.
- It handles missing provider data honestly with `Unknown`, `N/A`, and provider notes.
