# Run History and Support Bundle Plan

## Goal

Add two related features to BenchScope:

- Run history and baseline comparison for benchmark, battery, drive, RAM, network, storage-health, sensor, and device-information snapshots.
- One-click support report bundle export with Markdown reports, structured data, app logs, provider diagnostics, and privacy redaction.

These features should turn BenchScope from a collection of in-memory diagnostic tools into a local-first troubleshooting record. A user should be able to answer:

- What changed since the last run?
- Did performance, health, or connectivity get better or worse?
- Which hardware, driver, firmware, or provider details changed?
- Can I export a useful support package without exposing unnecessary personal details?

## Current App Baseline

BenchScope already has useful data sources:

- Matrix benchmark keeps `BenchmarkResult` rows in `BenchScopeApp.results`.
- Drive benchmark keeps `DriveBenchmarkResult` rows in `DriveBenchmarkState.results`.
- GPU memory benchmark keeps `GpuMemoryBenchmarkResult` rows in `GpuMemoryBenchmarkState.results`.
- AI training benchmark keeps `AiTrainingResult` rows in `AiTrainingBenchmarkState.results`.
- RAM tester keeps `RamTestResult` rows in `RamTestState.results`.
- Battery diagnostic keeps the latest parsed `BatteryReport` and live samples.
- Network diagnostic keeps adapter snapshots, probe results, bounded signal history, and Markdown export.
- Storage health keeps the latest `StorageHealthSnapshot`, scan result, benchmark-linked results, and Markdown export.
- Device information keeps `DeviceInfoSnapshot` and Markdown export.
- Sensors expose `SensorSnapshot`, `SensorReading`, provider labels, status, min/max values, and temperature-run summaries.
- The app keeps a session log in `BenchScopeApp.log`.

Existing Markdown exports:

- Device information report.
- Storage health report.
- Network diagnostic report.

Missing pieces:

- Most result rows are lost when the app closes.
- Existing report writers save near the current working directory instead of a stable app data directory.
- Matrix, drive, GPU memory, AI training, RAM, battery, and sensor summaries do not have a shared export contract.
- There is no canonical app/session snapshot that support export can reuse.
- There is no privacy-redaction layer.
- There is no schema versioning or migration story for persisted history.

## Product Principles

- Keep history local by default.
- Do not require internet access.
- Do not require administrator rights just to read local history or export a bundle.
- Store structured history in stable DTOs instead of serializing UI state directly.
- Treat missing provider data as normal. History should record unsupported, unavailable, blocked, and stale states honestly.
- Prefer append-only writes for run records so interrupted app sessions do not corrupt all history.
- Let users delete history from the UI.
- Never silently include sensitive identifiers in support bundles.
- Make redaction explicit, predictable, and testable.
- Use the same history records for comparison UI and support bundles.

## Storage Layout

Use a stable per-user app data root:

```text
%LOCALAPPDATA%\BenchScope\
  history\
    events-YYYY-MM.jsonl
    baselines.json
    index.json
  reports\
    device-info\
    storage-health\
    network\
    battery\
    benchmarks\
  bundles\
    benchscope-support-YYYYMMDD-HHMMSS.zip
  logs\
    session-YYYYMMDD-HHMMSS.log
```

Resolution order:

1. Use `LOCALAPPDATA\BenchScope` when available on Windows.
2. Fall back to `std::env::temp_dir().join("BenchScope")`.
3. Surface the active path in a future Settings or Diagnostics view.

Do not write persistent history beside the executable or source checkout unless the user explicitly chooses an export path.

## Dependencies

Add only when implementation begins:

- `serde` with `derive` for stable history DTOs.
- `serde_json` for JSONL history and index files.
- `zip` or equivalent pure-Rust zip writer for support bundles.
- Optional later: `sha2` for privacy-preserving hashes of serial numbers, machine IDs, and adapter IDs.

If dependency footprint matters, defer zip support and first implement an uncompressed timestamped support folder, then add zip packaging.

## Feature 1: Run History and Baseline Comparison

### User Experience

Add a new main-menu tool:

```text
History & Reports
```

Initial view:

- Latest run summary by category.
- Last run versus today comparison.
- Pinned baseline versus latest comparison.
- Hardware and driver changes since baseline.
- Buttons:
  - Save current snapshot.
  - Pin latest as baseline.
  - Export support bundle.
  - Open history folder.
  - Delete history.

Comparison states:

- No history yet.
- Latest only, no baseline.
- Comparable run found.
- Not comparable because hardware, adapter, benchmark config, drive target, or workload differs.
- Comparison available with warnings.

### History Categories

Persist records for these categories:

- `matrix_benchmark`
- `matrix_stress`
- `gpu_memory_benchmark`
- `ai_training_benchmark`
- `drive_benchmark`
- `storage_health`
- `ram_test`
- `battery_diagnostic`
- `network_diagnostic`
- `device_info`
- `sensor_snapshot`
- `session_log`
- `app_environment`

### Event Model

Use append-only JSONL events:

```rust
struct HistoryEvent {
    schema_version: u32,
    event_id: String,
    captured_at_unix_ms: u64,
    app_version: String,
    kind: HistoryEventKind,
    machine: MachineIdentitySnapshot,
    payload: HistoryPayload,
}
```

`HistoryPayload` should use feature-specific DTOs, for example:

```rust
enum HistoryPayload {
    MatrixBenchmark(MatrixBenchmarkHistoryRecord),
    DriveBenchmark(DriveBenchmarkHistoryRecord),
    RamTest(RamTestHistoryRecord),
    BatteryDiagnostic(BatteryHistoryRecord),
    NetworkDiagnostic(NetworkHistoryRecord),
    DeviceInfo(DeviceInfoHistoryRecord),
    SensorSnapshot(SensorHistoryRecord),
    AppEnvironment(AppEnvironmentRecord),
}
```

Avoid persisting internal UI structs directly. Create small DTOs containing:

- IDs and labels needed for matching.
- Numeric values needed for trend and delta calculations.
- Warnings/findings.
- Provider status.
- Report-relative references where applicable.

### App Environment Record

Capture once per app start and include in support bundles:

- BenchScope version.
- OS caption/version/build/architecture when available.
- Elevation state.
- Current backend mode.
- Sensor service status.
- Sensor driver/service versions when available.
- WGPU adapter list.
- CPU label and logical processor count.
- History root path.
- Feature availability notes.

### Machine and Hardware Identity

History comparison needs stable matching without exposing sensitive IDs by default.

Store locally:

- CPU model label.
- GPU adapter label, vendor ID, device ID, backend, device type, driver string.
- Drive model and root/volume label.
- Network adapter name, interface type, link kind, driver version.
- BIOS vendor/version/date.
- Board manufacturer/model.
- RAM installed bytes and module summary.

Sensitive values:

- Serial numbers.
- MAC addresses.
- BSSID.
- Full hardware IDs.
- User file paths.
- Hostname.
- Username.
- Local IP addresses.
- External/public IP address.

Local history may keep sensitive values only if they are necessary and behind a future setting. Support bundles should redact them by default.

### Baseline Model

Support two baseline types:

- Automatic baseline: previous successful comparable run.
- Pinned baseline: user-selected record per category/profile.

Example:

```rust
struct BaselineIndex {
    schema_version: u32,
    pinned: Vec<PinnedBaseline>,
}

struct PinnedBaseline {
    category: String,
    profile_key: String,
    event_id: String,
    label: String,
    pinned_at_unix_ms: u64,
}
```

Profile keys should be strict enough to avoid bad comparisons:

- Matrix benchmark: adapter label/vendor/device/backend, matrix size, CPU estimate flag, GPU intensity, validation mode.
- GPU memory benchmark: adapter label/vendor/device/backend, test kind, buffer size, iterations.
- AI training: backend, adapter/device, workload, preset, precision, batch/shape config, Python/CUDA metadata when applicable.
- Drive benchmark: drive model/root, filesystem mode label, profile, test kind, file size.
- RAM test: installed memory bucket, allocation mode, test pattern set.
- Battery: battery manufacturer/model when available, design capacity bucket.
- Network: adapter name/type, physical versus virtual, link kind.
- Device info: machine identity key.
- Storage health: drive model/root/serial hash where allowed.

### Delta Calculations

Use explicit comparison helpers per category. Do not rely on generic numeric diff over all fields.

Common delta fields:

- Current value.
- Baseline value.
- Absolute delta.
- Percent delta.
- Direction: better, worse, neutral, unknown.
- Severity: info, caution, warning, critical.
- Comparability notes.

Examples:

- Matrix/GPU/AI throughput: higher is better.
- Latency and elapsed time: lower is better.
- Drive MB/s and IOPS: higher is better.
- Drive p95 latency: lower is better.
- Battery full charge capacity: higher is better.
- Battery wear percent: lower is better.
- Network packet loss, latency, jitter: lower is better.
- RAM test failures: zero is expected; any new failure is critical.
- Storage media errors, unsafe shutdowns, critical warnings: lower/no growth is better.
- Temperature under similar load: lower is generally better, but mark low-confidence unless workload and ambient context match.

### Hardware and Driver Change Detection

Create a `HardwareDiff` view from two `DeviceInfoHistoryRecord` values:

- CPU model/logical processor count changed.
- GPU added/removed.
- GPU driver provider/version/date changed.
- Storage drive added/removed.
- Network adapter driver changed.
- BIOS version/date changed.
- RAM installed capacity/module count changed.
- OS build changed.
- Sensor provider coverage changed.

Each diff item should include:

- Component.
- Field.
- Previous value.
- Current value.
- Severity or note.

This is especially useful when benchmark performance changes after a driver, BIOS, Windows, or hardware update.

### Capture Triggers

Automatic capture:

- App start: app environment and initial sensor/provider status.
- Matrix benchmark result completed.
- GPU memory benchmark result completed.
- AI training result completed.
- Drive benchmark suite completed.
- Storage health refresh completed.
- Storage scan completed.
- RAM test completed.
- Battery scan completed.
- Network quick diagnosis completed.
- Network monitor stopped.
- Device info refresh completed.

Manual capture:

- Add a button in History & Reports: `Save current snapshot`.
- Optional later: add small `Save to history` buttons inside each tool.

### Retention

Initial policy:

- Keep all events for 90 days.
- Keep pinned baselines until user deletes them.
- Keep at most 1,000 events per category by default.
- Do not delete exported bundles automatically.

Retention should run opportunistically at app start or after successful writes.

### History Service Module

Proposed source layout:

```text
src/
  history/
    mod.rs
    paths.rs
    model.rs
    writer.rs
    reader.rs
    compare.rs
    redact.rs
    ui.rs
```

Wire into `src/main.rs` with:

```rust
include!("history/mod.rs");
```

Suggested state on `BenchScopeApp`:

```rust
history: HistoryState,
```

`HistoryState` should own:

- Root paths.
- Recent event index.
- Pinned baselines.
- Last write status.
- Last comparison summary.
- Bundle export status.

### Implementation Phases for History

#### Phase 1: Foundation

- Add `history` module.
- Add app data path resolution.
- Add event DTOs for app environment, matrix benchmark, drive benchmark, RAM test, battery, network, storage health, device info, GPU memory, AI training, and sensors.
- Add append-only JSONL writer.
- Add tolerant JSONL reader that skips malformed lines and reports warnings.
- Add unit tests for path resolution, JSONL round trip, and malformed-line handling.

#### Phase 2: Capture Current Results

- Capture app environment during startup completion.
- Capture each new result when a worker transitions from running to idle.
- Capture reportable snapshots after refresh/scan completion.
- Add session log flush to `logs/session-*.log`.
- Keep writes off the UI path where possible; small writes can be synchronous initially, but large support export should be backgrounded.

#### Phase 3: History UI

- Add `AppView::HistoryReports`.
- Add menu card under `Misc` or a new `Reports` category.
- Show recent events grouped by category.
- Show latest event details.
- Allow pinning a baseline.
- Allow deleting history with confirmation.

#### Phase 4: Comparison Engine

- Add profile-key matching.
- Add category-specific comparators.
- Add last-run-versus-latest deltas.
- Add pinned-baseline-versus-latest deltas.
- Add hardware/driver diff view.
- Add tests for directionality and non-comparable cases.

#### Phase 5: Polish

- Add filtering by category and date.
- Add baseline labels.
- Add CSV export for selected history records.
- Add retention settings.

## Feature 2: One-Click Support Report Bundle

### User Experience

Add `Export support bundle` in:

- History & Reports view.
- Device Information Viewer.
- Main menu footer or future Diagnostics page.

Flow:

1. User clicks `Export support bundle`.
2. App shows a compact privacy summary:
   - Default redactions enabled.
   - Full hardware IDs excluded.
   - Serial numbers excluded or hashed.
   - MAC/BSSID/IP/hostname/user paths redacted.
3. User confirms.
4. App creates a zip under `%LOCALAPPDATA%\BenchScope\bundles`.
5. UI shows the path and status.

Optional later:

- Include advanced identifiers toggle.
- Choose date range.
- Choose categories.
- Save bundle to custom path.

### Bundle Contents

Recommended initial zip layout:

```text
benchscope-support-YYYYMMDD-HHMMSS/
  README.md
  manifest.json
  summary.md
  reports/
    device-info.md
    storage-health.md
    network-diagnostic.md
    battery-diagnostic.md
    benchmark-results.md
    sensor-provider-status.md
  history/
    recent-events.redacted.jsonl
    baselines.redacted.json
    comparisons.json
  logs/
    session.redacted.log
  raw/
    app-environment.redacted.json
```

Do not include temporary drive benchmark files, battery XML reports, raw command output containing user paths, or unbounded monitor logs.

### Manifest

`manifest.json` should include:

- Bundle schema version.
- Generated timestamp.
- BenchScope version.
- OS build summary.
- Redaction mode.
- Included categories.
- Event count.
- Report filenames.
- Warnings about missing data.
- Bundle generator status.

### Summary Report

`summary.md` should be the first file a repair shop or support person reads.

Sections:

- System summary.
- Health findings.
- Recent benchmark deltas.
- Hardware/driver changes.
- Storage warnings.
- Battery warnings.
- Network warnings.
- RAM test status.
- Sensor/provider coverage.
- Notes on redaction and limitations.

Keep the language practical and cautious. Avoid claiming a component is failing unless the app has direct evidence.

### Report Sources

Reuse existing renderers where available:

- `render_device_info_report`.
- `render_storage_health_report`.
- `network_diagnostic_report_markdown`.

Add new renderers:

- `render_battery_diagnostic_report`.
- `render_matrix_benchmark_report`.
- `render_drive_benchmark_report`.
- `render_gpu_memory_benchmark_report`.
- `render_ai_training_benchmark_report`.
- `render_ram_test_report`.
- `render_sensor_provider_report`.
- `render_history_comparison_report`.

Where a full report is not ready, include a compact table in `benchmark-results.md` first.

### Privacy Redaction

Create a shared redaction module:

```rust
struct RedactionOptions {
    include_sensitive_ids: bool,
    hash_sensitive_ids: bool,
    include_local_paths: bool,
    include_network_addresses: bool,
}
```

Default options:

- `include_sensitive_ids = false`
- `hash_sensitive_ids = true`
- `include_local_paths = false`
- `include_network_addresses = false`

Redact or hash:

- Serial numbers.
- MAC addresses.
- BSSID.
- Full hardware IDs.
- Hostname.
- Username.
- Local profile paths.
- Public IP.
- Local IP.
- DNS suffix/search domain where it may reveal an organization.
- Wi-Fi SSID by default if privacy mode is strict. Consider including a redacted label like `Wi-Fi network 1`.

Keep non-sensitive troubleshooting details:

- Component model names.
- Driver provider/version/date.
- OS build.
- Adapter type.
- Link speed.
- Signal quality.
- SMART/NVMe health counters.
- Battery capacity and cycle count.
- Benchmark metrics.
- Provider availability and error categories.

### Bundle Writer

Proposed source layout:

```text
src/
  support_bundle/
    mod.rs
    model.rs
    collect.rs
    render.rs
    zip.rs
```

The bundle collector should build a `SupportBundleSnapshot` from:

- Current app state.
- Latest history records.
- Pinned baseline comparisons.
- Current sensor snapshot.
- Session log.
- Report renderers.

Run bundle export on a background worker because report rendering and zip writing can take noticeable time.

Suggested UI state:

```rust
struct SupportBundleState {
    running: bool,
    last_path: Option<PathBuf>,
    last_error: Option<String>,
    redaction_options: RedactionOptions,
}
```

### Support Bundle Snapshot

Use a single DTO to avoid each renderer reaching into app state directly:

```rust
struct SupportBundleSnapshot {
    generated_at_unix_ms: u64,
    app_environment: AppEnvironmentRecord,
    current_device_info: Option<DeviceInfoHistoryRecord>,
    latest_history: Vec<HistoryEvent>,
    baseline_comparisons: Vec<ComparisonSummary>,
    current_sensor_snapshot: Option<SensorHistoryRecord>,
    session_log: Vec<String>,
    redaction: RedactionOptions,
}
```

### Implementation Phases for Support Bundle

#### Phase 1: Report Inventory

- Add report renderers for result types missing Markdown export.
- Normalize report filenames.
- Make existing report renderers return strings independently from writing files.
- Add tests for each renderer's basic contents.

#### Phase 2: Redaction

- Add redaction helpers.
- Apply to device info, network, storage serials, app logs, and history events.
- Add tests for MAC, IP, serial, path, hostname, and hardware-ID redaction.

#### Phase 3: Bundle Folder Export

- First export to a timestamped folder.
- Write manifest, summary, reports, redacted history, and log files.
- Add UI button and status.
- Add tests for generated file list.

#### Phase 4: Zip Export

- Add zip writer dependency.
- Zip the timestamped bundle folder or write directly to zip entries.
- Keep folder export as a debug fallback.
- Add error handling for locked files, permission failures, and disk-full errors.

#### Phase 5: One-Click Polish

- Add a privacy confirmation panel.
- Add bundle-size estimate.
- Add open-folder action if appropriate for the platform.
- Add category/date filters later.

## UI Integration

### Main Menu

Add a menu item:

```text
History & Reports
Review saved runs, compare baselines, and export support bundles.
```

Suggested categories:

- `Misc`
- `Drivers`
- `I/O`

Consider adding a future `Reports` category if the menu grows.

### History & Reports View

Top band:

- Last capture timestamp.
- Event count.
- Pinned baseline count.
- Last support bundle path/status.

Tabs or sections:

- Recent runs.
- Baselines.
- Comparisons.
- Hardware changes.
- Support bundle.
- Privacy.

Keep the first implementation simple: one scrollable view with sections is enough.

## Data Migration and Compatibility

Start with `schema_version = 1`.

Reader behavior:

- Skip unknown event kinds.
- Preserve unknown JSON fields by ignoring them.
- Report malformed lines as warnings, not fatal errors.
- If `baselines.json` is invalid, rename it to `.broken-<timestamp>` and continue with no pinned baselines.

Writer behavior:

- Append one JSON object per line.
- Flush after each completed event.
- Use temporary file plus rename for index/baseline updates.

## Error Handling

Show clear errors for:

- History root cannot be created.
- JSONL write failed.
- Baseline index cannot be saved.
- Bundle export failed.
- Zip writer failed.
- Report renderer failed.

Do not block benchmarks because history capture failed. Log the failure and keep the benchmark result visible.

## Testing Plan

Unit tests:

- App data path fallback.
- JSONL event round trip.
- Malformed JSONL line handling.
- Baseline profile-key generation.
- Delta direction for higher-is-better and lower-is-better metrics.
- Non-comparable runs produce explicit notes.
- Hardware diff detects driver version changes.
- Redaction removes MAC, IP, serial, path, hostname, and hardware IDs.
- Bundle manifest contains expected files.
- Report renderers include key metrics.

Integration/manual tests:

- Run matrix benchmark, close app, reopen, history persists.
- Run drive benchmark twice and compare deltas.
- Pin baseline, run a slower/faster result, verify direction and percent delta.
- Refresh device info before and after simulated driver data change.
- Export support bundle with empty history.
- Export support bundle with all tools populated.
- Validate the zip opens in Windows Explorer.
- Confirm privacy-redacted bundle does not include local username, full profile path, MAC address, IP address, or drive serial.

## Acceptance Criteria

Run history:

- Completed tool results are saved locally and survive restart.
- The History & Reports view shows recent records grouped by category.
- The user can pin at least one baseline per category/profile.
- Latest versus previous and latest versus pinned baseline comparisons show absolute and percent deltas.
- Hardware and driver changes are called out when device-info snapshots differ.
- History failures do not break tool execution.

Support bundle:

- The user can export a support bundle with one click after privacy confirmation.
- The bundle includes a manifest, summary, Markdown reports, recent redacted history, comparison summaries, provider status, and session log.
- Existing device, storage, and network reports are reused.
- Sensitive identifiers are redacted by default.
- Bundle export errors are surfaced without crashing the app.

## Recommended First Milestone

Build the history foundation first:

1. Add `serde`, `serde_json`, and the `history` module.
2. Persist app environment, matrix benchmark, drive benchmark, RAM test, battery, network, storage health, device info, GPU memory, AI training, and sensor snapshots as JSONL events.
3. Add a minimal History & Reports view showing recent events.
4. Add baseline pinning for matrix and drive benchmarks.
5. Add comparison helpers for matrix and drive benchmarks.

Then build support bundle export:

1. Add missing Markdown renderers for battery, RAM, drive, matrix, GPU memory, AI training, sensors, and comparisons.
2. Add redaction helpers and tests.
3. Export a timestamped support folder.
4. Add zip packaging.

This order keeps the core data model useful before adding the more visible export workflow.
