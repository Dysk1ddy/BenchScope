# SSD / HDD Health Checker Plan

## Goal

Add a separate storage diagnosis tool to BenchScope focused on drive health, failure warning signs, and exportable reports.

This should be a distinct main-menu option, separate from the existing drive read/write benchmark. The benchmark answers "how fast is this drive?" while the health checker answers "is this drive healthy, overheating, wearing out, or showing early failure signs?"

Main menu tools should include:

- Matrix CPU/GPU Benchmark
- Matrix Stress Test
- Drive Benchmark
- SSD / HDD Health Checker

## User Experience

The health checker should feel like a practical diagnostic dashboard.

Top controls:

- Back button
- Drive selector
- Refresh button
- Run quick health scan
- Export report

Main sections:

- Overall health summary
- SMART / NVMe attribute table
- Temperature and utilization
- Remaining life estimate
- Sector and media error warnings
- Read-only surface scan
- Optional read/write benchmark shortcut
- Health report preview and export status

The first screen should show useful information immediately after selecting a drive. The user should not need to run a destructive or long benchmark to get basic health warnings.

## Main Menu Integration

Add a new app view:

```rust
enum AppView {
    MainMenu,
    MatrixBenchmark,
    MatrixStress,
    DriveBenchmark,
    StorageHealth,
}
```

The main menu should show a new button:

```text
SSD / HDD Health Checker
```

Selecting it switches to the storage health view.

Back behavior:

- If no scan is running, Back returns to the main menu.
- If a scan is running, Back asks whether to cancel the scan and return.
- No scan worker should continue running after leaving the tool.

## Supported Drive Types

Initial support should focus on Windows local physical drives:

- SATA SSD
- SATA HDD
- NVMe SSD
- USB external drives where SMART data is exposed

The UI should handle unsupported drives gracefully. Some USB bridges and RAID controllers hide SMART data. In those cases the app should still show basic drive identity, capacity, filesystem, and benchmark/surface-scan options where safe.

## Health Summary

The health checker should compute and display a simple status:

- Good
- Caution
- Critical
- Unknown

Summary fields:

- Drive model
- Serial number, if available
- Firmware version, if available
- Bus type: NVMe, SATA, USB, RAID, Unknown
- Capacity
- Selected volume or drive letter
- Current temperature
- Estimated remaining life
- Power-on hours
- Power cycle count
- Reallocated sector count
- Pending sector count
- Uncorrectable sector count
- NVMe media/data integrity errors
- SMART overall pass/fail status

Recommended visual rules:

- Good: no known warnings
- Caution: early warning values, high temperature, or wear indicators approaching limits
- Critical: SMART failure predicted, nonzero serious media errors, extreme temperature, or rapidly growing bad-sector indicators
- Unknown: health data unavailable or blocked

## SMART and NVMe Data Collection

Use multiple providers because Windows exposes storage health unevenly across drive types.

Recommended provider order:

1. Windows storage reliability counters
2. WMI / CIM disk information
3. Native Windows storage IOCTL calls
4. Existing sensor-helper integration when it exposes storage sensors
5. Command fallback only if needed

Possible Windows sources:

- `MSFT_PhysicalDisk`
- `MSFT_StorageReliabilityCounter`
- `Win32_DiskDrive`
- `Win32_DiskPartition`
- `Win32_LogicalDisk`
- `MSStorageDriver_FailurePredictStatus`
- `MSStorageDriver_FailurePredictData`
- `IOCTL_STORAGE_QUERY_PROPERTY`
- `IOCTL_STORAGE_PREDICT_FAILURE`

Provider results should be normalized into one internal model so the UI does not care where the data came from.

## SMART Attributes to Track

Important SATA SMART attributes:

- `5` Reallocated Sector Count
- `9` Power-On Hours
- `12` Power Cycle Count
- `184` End-to-End Error
- `187` Reported Uncorrectable Errors
- `188` Command Timeout
- `190` Airflow Temperature
- `194` Temperature
- `196` Reallocation Event Count
- `197` Current Pending Sector Count
- `198` Offline Uncorrectable Sector Count
- `199` UDMA CRC Error Count
- `202` Percent Lifetime Used, vendor-specific
- `231` SSD Life Left, vendor-specific
- `233` Media Wearout Indicator, vendor-specific
- `241` Total LBAs Written
- `242` Total LBAs Read

Important NVMe health fields:

- Critical warning flags
- Composite temperature
- Available spare
- Available spare threshold
- Percentage used
- Data units read
- Data units written
- Host read commands
- Host write commands
- Controller busy time
- Power cycles
- Power-on hours
- Unsafe shutdowns
- Media and data integrity errors
- Number of error information log entries

The raw table should show:

- ID or field name
- Display name
- Current value
- Worst value, if available
- Threshold, if available
- Raw value
- Interpretation
- Severity

## Temperature Rules

Temperature should be prominent because heat is a common cause of storage failures.

Suggested thresholds:

- Under 50 C: normal
- 50-59 C: warm
- 60-69 C: hot
- 70 C or higher: critical

HDDs may deserve a lower warning threshold than NVMe SSDs. The first version can use one shared rule, then add drive-type-specific rules later.

Temperature display:

- Current temperature
- Maximum observed during this session
- Start and end temperature for scans or benchmarks
- Warning if temperature data is unavailable

## Remaining Life Estimate

Remaining life is an estimate, not a promise. The UI should label it as estimated.

For NVMe:

- Use `percentage_used`.
- Remaining life can be approximated as `max(0, 100 - percentage_used)`.
- If `percentage_used` exceeds 100, show `0% or beyond rated endurance`.

For SATA SSD:

- Prefer vendor wear attributes when available:
  - `231` SSD Life Left
  - `233` Media Wearout Indicator
  - `202` Percent Lifetime Used
- Interpret conservatively because vendors encode these differently.

For HDD:

- Do not show a percentage life estimate unless the drive exposes a meaningful vendor value.
- Show `Unknown` and focus on SMART error indicators, power-on hours, temperature, and scan results.

The report should include a note that remaining-life estimates are based on vendor-reported SMART/NVMe data and may not predict sudden electronic failure.

## Bad Sector and Media Error Detection

The health checker should look for bad-sector indicators in two ways:

1. SMART attributes and NVMe media error counters
2. A read-only surface scan

SMART warning rules:

- Reallocated sector count greater than 0: Caution
- Reallocated sector count increasing between scans: Critical
- Current pending sector count greater than 0: Critical for HDDs, Caution or Critical for SSDs depending on count
- Offline uncorrectable greater than 0: Critical
- Reported uncorrectable errors greater than 0: Caution or Critical
- NVMe media/data integrity errors greater than 0: Critical
- UDMA CRC errors greater than 0: Caution, with note that this can indicate cable/controller issues

## Read-Only Surface Scan

Add a scan that reads regions of the selected volume or physical drive without writing data.

Modes:

- Quick scan: sample regions across the drive
- Balanced scan: read more samples and all filesystem metadata-friendly ranges if practical
- Full scan: read the whole selected volume or physical drive when supported

First release recommendation:

- Implement Quick and Balanced first.
- Add Full scan only after cancellation, permissions, and progress reporting are solid.

Metrics:

- Bytes scanned
- Percent complete
- Read errors
- Slow blocks
- Average read latency
- Worst read latency
- Scan duration
- Temperature at start/end/max

Safety rules:

- Default to read-only.
- Never write to user data during health scans.
- If raw physical-drive access requires administrator permission, show a clear message and fall back to file-based read sampling where possible.
- Cancel should be checked between every read batch.

Slow-block detection:

- Track read latency per block or sample.
- Flag blocks that are much slower than the median.
- Do not call a slow block a bad sector unless the read fails or the OS reports an I/O error.

## Read / Write Benchmark Integration

The health checker can include a small benchmark section, but it should not duplicate the full Drive Benchmark tool.

Recommended behavior:

- Show a compact "Run quick read/write benchmark" action.
- Reuse the existing drive benchmark backend where possible.
- Link or switch to the full Drive Benchmark tool for detailed performance testing.
- Include benchmark results in exported health reports only when the user runs them from this screen.

Write benchmark warning:

```text
Write tests create temporary data on the selected drive and may add SSD write wear.
```

The health checker should remain useful even if the user never runs a write benchmark.

## Export Health Report

Support exporting a report for troubleshooting or repair-shop use.

Formats:

- Markdown first
- JSON later
- CSV later for attribute tables

Default filename:

```text
benchscope-storage-health-YYYYMMDD-HHMMSS.md
```

Report contents:

- App name and version
- Report timestamp
- Drive identity
- Volume mapping
- Capacity and free space
- Bus type and media type
- Overall status
- SMART pass/fail status
- Temperature summary
- Remaining life estimate
- Key warnings
- SMART attribute table
- NVMe health fields
- Surface scan results, if run
- Benchmark results, if run
- Provider/source notes
- Permission or unsupported-data warnings

The export should avoid including sensitive user file paths except for the selected drive or volume root.

## UI Layout

Suggested layout:

```text
+------------------------------------------------------------+
| Back  SSD / HDD Health Checker       Drive: [dropdown] [Refresh] |
+------------------------------------------------------------+
| Overall: Good/Caution/Critical/Unknown                     |
| Model, serial, firmware, bus, capacity                     |
| Temperature, remaining life, power-on hours                |
+------------------------------------------------------------+
| Warnings                                                   |
| - Reallocated sectors: 0                                   |
| - Pending sectors: 0                                       |
| - NVMe media errors: 0                                     |
+------------------------------------------------------------+
| [Run Quick Scan] [Run Balanced Scan] [Cancel] [Export]     |
| Progress bar                                               |
+------------------------------------------------------------+
| SMART / NVMe Attribute Table                               |
+------------------------------------------------------------+
| Report Preview / Log                                       |
+------------------------------------------------------------+
```

The warnings section should be visible without scrolling.

## Data Model

Suggested internal types:

```rust
enum StorageHealthStatus {
    Good,
    Caution,
    Critical,
    Unknown,
}

enum StorageMediaType {
    Ssd,
    Hdd,
    Unknown,
}

enum StorageBusType {
    Nvme,
    Sata,
    Usb,
    Raid,
    Unknown,
}

enum HealthSeverity {
    Info,
    Warning,
    Critical,
}

struct StorageDriveIdentity {
    physical_drive_id: String,
    model: String,
    serial: Option<String>,
    firmware: Option<String>,
    bus_type: StorageBusType,
    media_type: StorageMediaType,
    capacity_bytes: u64,
    volumes: Vec<String>,
}

struct SmartAttribute {
    id: Option<u16>,
    name: String,
    current: Option<u64>,
    worst: Option<u64>,
    threshold: Option<u64>,
    raw: Option<u64>,
    display_value: String,
    interpretation: String,
    severity: HealthSeverity,
}

struct NvmeHealthInfo {
    critical_warnings: Vec<String>,
    temperature_c: Option<f32>,
    available_spare_percent: Option<u8>,
    spare_threshold_percent: Option<u8>,
    percentage_used: Option<u16>,
    data_units_read: Option<u128>,
    data_units_written: Option<u128>,
    power_on_hours: Option<u64>,
    unsafe_shutdowns: Option<u64>,
    media_errors: Option<u64>,
    error_log_entries: Option<u64>,
}

struct StorageHealthSnapshot {
    identity: StorageDriveIdentity,
    status: StorageHealthStatus,
    temperature_c: Option<f32>,
    remaining_life_percent: Option<f32>,
    smart_overall_passed: Option<bool>,
    smart_attributes: Vec<SmartAttribute>,
    nvme_health: Option<NvmeHealthInfo>,
    warnings: Vec<HealthWarning>,
    provider_notes: Vec<String>,
}

struct HealthWarning {
    severity: HealthSeverity,
    title: String,
    detail: String,
}
```

## Worker Events

Storage health checks should run off the UI thread.

Suggested event model:

```rust
enum StorageHealthEvent {
    DriveListUpdated(Vec<StorageDriveIdentity>),
    SnapshotUpdated(StorageHealthSnapshot),
    ScanProgress(StorageScanProgress),
    ScanCompleted(StorageScanResult),
    BenchmarkCompleted(Vec<DriveBenchmarkResult>),
    ReportExported(PathBuf),
    Log(String),
    Failed(String),
}
```

Scan progress:

```rust
struct StorageScanProgress {
    mode: StorageScanMode,
    bytes_scanned: u64,
    total_bytes_planned: u64,
    read_errors: u64,
    slow_regions: u64,
    current_region_label: String,
    elapsed_ms: u64,
}
```

## Implementation Phases

### Phase 1: Planning and Menu Shell

Tasks:

- Add this plan file.
- Add `StorageHealth` to the main app view enum.
- Add `SSD / HDD Health Checker` to the main menu.
- Add a placeholder health checker screen with Back navigation.

Acceptance criteria:

- App opens to main menu.
- New health checker option appears.
- Selecting it opens the new screen.
- Back returns to the main menu.

### Phase 2: Drive Inventory

Tasks:

- Enumerate physical drives.
- Map physical drives to volumes where possible.
- Display model, capacity, media type, bus type, and drive letters.
- Show unsupported or incomplete fields as `N/A`.

Acceptance criteria:

- User can select a drive.
- The UI shows stable identity information.
- Missing data does not crash the app.

### Phase 3: SMART / NVMe Snapshot

Tasks:

- Query Windows storage reliability counters.
- Add SMART failure-prediction status where available.
- Add NVMe health fields where available.
- Normalize data into `StorageHealthSnapshot`.
- Add warnings for serious attributes.

Acceptance criteria:

- Temperature appears when exposed by the OS/provider.
- Reallocated, pending, and uncorrectable sector warnings are shown.
- NVMe percentage used and media errors are shown when available.
- Unsupported SMART data produces an `Unknown` status with provider notes.

### Phase 4: Health Scoring

Tasks:

- Implement Good/Caution/Critical/Unknown status calculation.
- Add remaining life estimate.
- Add temperature severity rules.
- Add warning list with concise explanations.

Acceptance criteria:

- A healthy drive with no warning counters shows Good.
- Nonzero pending or uncorrectable sectors affect status.
- High temperature affects status.
- Unknown data is clearly differentiated from Good.

### Phase 5: Read-Only Surface Scan

Tasks:

- Add Quick and Balanced scan modes.
- Read sampled regions without writing.
- Track read failures and slow regions.
- Add progress, cancellation, and summary results.

Acceptance criteria:

- Scans run on a background worker.
- Cancel stops scanning promptly.
- Read failures are reported without crashing.
- The UI distinguishes slow reads from confirmed bad sectors.

### Phase 6: Benchmark Hook

Tasks:

- Add a compact quick benchmark action.
- Reuse existing drive benchmark code where possible.
- Include results in the health report when run.
- Add a shortcut to the full Drive Benchmark tool.

Acceptance criteria:

- The health checker does not duplicate the full benchmark UI.
- Benchmark results are clearly optional.
- Write-test wear warning is shown before write benchmarks.

### Phase 7: Report Export

Tasks:

- Export Markdown health reports.
- Include snapshot, warnings, attributes, scan results, and optional benchmark results.
- Add report export success/failure messages.

Acceptance criteria:

- User can export a readable `.md` report.
- Report includes all visible health data.
- Report handles missing SMART data honestly.

## Testing Plan

Automated tests:

- Health status calculation.
- Temperature threshold logic.
- Remaining-life estimation.
- SMART attribute interpretation.
- NVMe warning interpretation.
- Markdown report generation.
- Scan progress math.
- Cancellation flag behavior.

Manual tests:

- Internal NVMe SSD.
- Internal SATA SSD.
- Internal HDD, if available.
- USB external drive with SMART exposed.
- USB external drive with SMART hidden.
- Drive with no temperature data.
- Run Quick scan and cancel it.
- Export report before and after a scan.
- Compare key SMART values with a known tool such as CrystalDiskInfo for sanity.

## Risks

### SMART Data Availability

Risk:

- Some drives, USB bridges, or RAID controllers do not expose SMART data.

Mitigation:

- Use multiple providers.
- Show `Unknown` instead of pretending the drive is healthy.
- Include provider notes in the report.

### Vendor-Specific Attributes

Risk:

- SSD life attributes are not standardized across all SATA SSDs.

Mitigation:

- Prefer NVMe standard fields when available.
- Interpret SATA SSD life attributes conservatively.
- Label remaining life as an estimate.

### Permissions

Risk:

- Raw disk access and some health providers may require administrator permissions.

Mitigation:

- Show clear permission messages.
- Use non-admin provider paths where possible.
- Keep basic identity and volume information available without elevation.

### False Confidence

Risk:

- A drive can fail suddenly even when SMART data looks normal.

Mitigation:

- Use cautious report language.
- Explain that SMART is an early-warning tool, not a guarantee.
- Highlight backup recommendations when warnings appear.

### Write Wear

Risk:

- Optional write benchmarks add SSD wear.

Mitigation:

- Keep health scans read-only by default.
- Warn before write benchmarks.
- Reuse existing benchmark profile limits.

## Recommended Defaults

- Initial selected drive: system drive, if mapping is reliable
- Initial action: automatic health snapshot only
- Scan mode: Quick
- Temperature warning: 50 C
- Temperature critical: 70 C
- Report format: Markdown
- SMART refresh: manual Refresh button first, optional auto-refresh later
- Benchmark from health checker: optional and off by default

## Definition of Done

The SSD / HDD Health Checker is ready when:

- It appears as its own main-menu option.
- It has working Back navigation.
- It lists available local drives.
- It shows drive identity and capacity.
- It reads SMART or NVMe health data when available.
- It shows drive temperature when available.
- It estimates remaining SSD life when reliable data exists.
- It warns about high reallocated, pending, or uncorrectable sector counts.
- It reports NVMe media/data integrity errors.
- It can run a cancelable read-only surface scan.
- It can optionally run or link to a quick read/write benchmark.
- It exports a Markdown health report.
- It handles unsupported drives honestly with `Unknown` status and provider notes.
