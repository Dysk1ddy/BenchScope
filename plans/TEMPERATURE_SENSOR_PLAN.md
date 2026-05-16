# Temperature Sensor Plan

## Goal

Add HWMonitor-style temperature readouts to BenchScope without changing the benchmark behavior or making tests depend on sensors being available.

Sensor readouts should appear in the bottom-right corner of the UI window:

- CPU temperature for matrix benchmark and matrix stress/repeat tests.
- GPU temperature for matrix benchmark and matrix stress/repeat tests.
- SSD temperature for drive benchmark tests.

The panel should keep updating while a benchmark is running and should degrade gracefully when a sensor is unsupported, blocked by permissions, or temporarily unavailable.

## User Experience

### Bottom-Right Sensor Panel

Use an `egui::Area` anchored to `Align2::RIGHT_BOTTOM` so the panel remains fixed at the bottom right regardless of the active tool view.

Recommended layout:

```text
Sensors
CPU   68 C
GPU   72 C
SSD   41 C
```

Behavior:

- Matrix view shows CPU and GPU rows.
- Drive view shows SSD row for the selected target drive, and may also show CPU/GPU if global sensors are available.
- Main menu can show all available sensors in a compact idle state.
- Unavailable readings show `N/A`, not `0 C`.
- Stale readings show `-- C` or a muted `stale` indicator.
- Hover text should explain the provider, for example `NVML`, `LibreHardwareMonitor`, or `NVMe SMART`.

Visual states:

- Normal: default text color.
- Warm: yellow/orange when approaching a configurable warning threshold.
- Hot: red when over the configured warning threshold.
- Missing: muted text with a short reason in the tooltip.

Initial thresholds:

- CPU warning: 85 C, critical: 95 C.
- GPU warning: 80 C, critical: 90 C.
- SSD warning: 60 C, critical: 70 C.

These thresholds should be constants first, with settings later if needed.

## Product Scope

In scope:

- Live sensor display in the bottom-right UI.
- Background polling that does not block rendering or benchmark workers.
- Sensor snapshots captured at benchmark start and end.
- Max temperature observed during each benchmark run.
- Result/log entries that include temperature deltas when readings are available.
- Graceful fallback when sensors are unsupported.

Out of scope for the first implementation:

- Fan speed, voltage, power, and clock telemetry.
- Historical charts.
- Automatic throttling or benchmark cancellation based on temperature.
- Requiring administrator privileges just to launch BenchScope.
- Bundling kernel drivers unless the user explicitly opts in later.

## Architecture

Add a provider-based telemetry layer:

```text
ui
  SensorPanel
    reads latest SensorSnapshot

telemetry
  SensorManager
    owns polling thread
    merges provider readings
    exposes latest snapshot to UI

providers
  CpuTemperatureProvider
  GpuTemperatureProvider
  DriveTemperatureProvider
```

Recommended files after refactor:

```text
src/main.rs
src/telemetry/mod.rs
src/telemetry/sensors.rs
src/telemetry/providers/mod.rs
src/telemetry/providers/windows.rs
```

Keep the first patch small if desired by implementing the module in `main.rs`, then moving it out once behavior is proven.

## Data Model

Suggested structs:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SensorKind {
    Cpu,
    Gpu,
    Drive,
}

#[derive(Clone, Debug)]
struct SensorReading {
    kind: SensorKind,
    label: String,
    temperature_c: Option<f32>,
    provider: String,
    updated_at: Instant,
    status: SensorStatus,
}

#[derive(Clone, Debug)]
enum SensorStatus {
    Ok,
    Unsupported,
    PermissionDenied,
    Stale,
    Error(String),
}

#[derive(Clone, Debug, Default)]
struct SensorSnapshot {
    cpu: Option<SensorReading>,
    gpu: Option<SensorReading>,
    drive: Option<SensorReading>,
}
```

Benchmark result extensions:

```rust
#[derive(Clone, Debug, Default)]
struct TemperatureSummary {
    start_c: Option<f32>,
    end_c: Option<f32>,
    max_c: Option<f32>,
}
```

Matrix benchmark results should include CPU and GPU summaries.

Drive benchmark results should include SSD summary for the target physical drive.

## Polling Model

Use one background polling thread owned by `SensorManager`.

Polling interval:

- Poll continuously at 1 Hz whether a benchmark is running or idle.
- Back off to every 5 seconds after repeated provider errors if this becomes necessary later.

The UI should never query hardware directly. It should only read the latest snapshot from an `Arc<RwLock<SensorSnapshot>>` or receive updates through an `mpsc` channel.

Cancellation:

- `SensorManager` owns an `Arc<AtomicBool>` shutdown flag.
- App shutdown sets the flag and lets the thread exit.
- Provider errors should not panic the thread.

## Sensor Providers

### Provider Priority

Use layered providers because Windows temperature support is fragmented.

Priority order:

1. Native vendor or device API when available.
2. Windows storage protocol APIs for SSD temperature.
3. Optional LibreHardwareMonitor/OpenHardwareMonitor bridge for broader CPU/GPU coverage.
4. WMI thermal zone fallback only as a last resort, because it is often motherboard or ACPI zone temperature rather than CPU package temperature.

### CPU Temperature

Windows does not expose a reliable universal CPU package temperature API.

Recommended first practical implementation:

- Add a provider interface first.
- Implement CPU temperature through an optional LibreHardwareMonitor-compatible provider.
- If unavailable, show `CPU N/A` with tooltip `CPU temperature provider unavailable`.

Possible implementation options:

- Run a small helper process that uses LibreHardwareMonitorLib and emits JSON.
- Later, embed a native sensor-chip reader if the project accepts driver requirements.
- Use WMI `MSAcpi_ThermalZoneTemperature` only as a fallback and label it clearly as `ACPI thermal zone`, not CPU package.

Important:

- Do not pretend ACPI thermal zone readings are exact CPU temperatures.
- Do not require administrator mode in the first pass.
- Keep sensor availability separate from benchmark correctness.

### GPU Temperature

Use vendor APIs where possible:

- NVIDIA: NVML through `nvml.dll`.
- AMD: ADLX or ADL when available.
- Intel: Level Zero or other Intel telemetry APIs if practical later.

First implementation can start with NVIDIA NVML because it is well-known and available on many systems.

Provider behavior:

- Match the selected `wgpu` adapter to a vendor provider when possible.
- If exact adapter matching is not available, show the primary GPU reading with a tooltip that says the match is best-effort.
- If no provider is available, show `GPU N/A`.

Future refinements:

- Map by PCI bus/device ID where APIs expose it.
- Support multiple GPU rows if the selected adapter cannot be matched confidently.

### SSD Temperature

Drive temperature should be tied to the drive benchmark target folder.

Plan:

1. Resolve the selected target folder to a volume path.
2. Resolve the volume to a physical disk number with `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`.
3. Open the matching `\\.\PhysicalDriveN`.
4. Query temperature:
   - NVMe: `IOCTL_STORAGE_QUERY_PROPERTY` with protocol-specific NVMe SMART/Health data.
   - SATA/SAS: SMART or storage protocol fallback where available.
5. Cache the target drive identity so polling does not repeat expensive mapping work every frame.

Drive provider should update when the target folder changes.

If the selected target spans multiple extents:

- Prefer the first extent for display.
- Add a tooltip saying the volume spans multiple physical disks.

If temperature cannot be read:

- Show `SSD N/A`.
- Include the reason in the tooltip, such as `SMART temperature unavailable`.

## UI Integration

Add a method on the app:

```rust
fn ui_sensor_panel(&mut self, ctx: &egui::Context) {
    egui::Area::new("sensor_panel")
        .anchor(egui::Align2::RIGHT_BOTTOM, [-12.0, -12.0])
        .show(ctx, |ui| {
            // compact sensor rows
        });
}
```

Call it once per frame after the active tool view is rendered.

Layout details:

- Keep panel compact, roughly 150-190 px wide.
- Use a subtle frame so it reads as utility UI, not a modal.
- Avoid covering buttons or result tables by giving bottom panels enough margin if needed.
- Do not let the panel resize based on long error text; put details in tooltips.

Recommended row helper:

```rust
fn ui_temperature_row(ui: &mut egui::Ui, label: &str, reading: Option<&SensorReading>) {
    // fixed label column, fixed value column, tooltip for provider/status
}
```

## Benchmark Integration

### Matrix Single Benchmark

At start:

- Capture current CPU/GPU readings.
- Reset per-run max tracking.

During run:

- Sensor manager continues polling independently.
- Main UI tracks max observed CPU/GPU temperatures while the matrix benchmark is running.

At completion:

- Capture end CPU/GPU readings.
- Store temperature summaries in `BenchmarkResult`.
- Add log line:

```text
Temperature: CPU 64 -> 78 C (max 80 C), GPU 52 -> 74 C (max 75 C)
```

Only include parts with available readings.

### Matrix Stress/Repeat Test

At start:

- Capture CPU/GPU start readings.

During repeat:

- Track max CPU/GPU temperatures across the whole stress window.

At cancellation or completion:

- Log start/end/max values.
- If repeat result rows are added later, include the summaries there too.

### Drive Benchmark

At start:

- Resolve selected target folder to a physical drive.
- Capture SSD start temperature.

During run:

- Reuse the continuous 1 Hz SSD polling stream while the drive benchmark is active.
- Track max SSD temperature.

At completion:

- Capture SSD end temperature.
- Include SSD temp summary in drive result/log:

```text
SSD temperature: 39 -> 48 C (max 49 C)
```

If the drive target changes before a run, refresh the drive provider mapping before the run starts.

## Error Handling

Sensor failures should be non-fatal.

Rules:

- Never fail a benchmark because temperature is unavailable.
- Never show fake readings.
- Log provider initialization failures once, not every poll.
- Mark stale readings after 5 seconds without a successful update.
- Keep the last known reading visible only if it is clearly marked stale.

Example UI states:

```text
CPU   N/A
GPU   71 C
SSD   stale
```

## Permissions and Packaging

Initial implementation should avoid mandatory administrator privileges.

If a provider needs elevated access:

- Detect the permission failure.
- Show `Permission denied` in the tooltip.
- Continue running benchmarks.

Packaging notes:

- If using NVML dynamically, load `nvml.dll` at runtime and handle missing DLL cleanly.
- If adding an external helper, keep it optional and document where it is expected.
- Avoid shipping unsigned kernel drivers in the first implementation.

## Testing Plan

Unit tests:

- Temperature formatting.
- Warning/critical color classification.
- Stale-reading detection.
- Max temperature aggregation.
- Drive target remapping logic with mocked disk extents.
- Provider error handling.

Integration tests with mocked providers:

- Matrix run captures CPU/GPU start/end/max summaries.
- Repeat/stress run captures CPU/GPU max across multiple updates.
- Drive run captures SSD start/end/max summary.
- UI panel renders `N/A` when providers are unavailable.

Manual tests:

- Launch on a system with no supported providers.
- Launch on NVIDIA GPU system and verify GPU temperature.
- Run matrix benchmark and confirm CPU/GPU panel updates.
- Run matrix repeat/stress test and confirm max temperature appears in logs.
- Run drive benchmark on NVMe SSD and confirm SSD temperature.
- Change drive target folder and confirm SSD provider remaps.
- Sleep/resume or unplug external drive and verify graceful stale/error state.

## Implementation Phases

### Phase 1: UI and Mock Sensor Layer

- Add `SensorReading`, `SensorSnapshot`, and `SensorManager` skeleton.
- Add fixed bottom-right sensor panel.
- Add mock provider for tests and local UI development.
- Show `N/A` when no real provider is configured.

Acceptance criteria:

- Panel appears at bottom right in main menu, matrix view, and drive view.
- No benchmark behavior changes.
- Tests cover formatting, stale state, and max aggregation.

### Phase 2: Benchmark Temperature Summaries

- Track start/end/max readings for matrix single benchmark.
- Track start/end/max readings for matrix repeat/stress tests.
- Track start/end/max readings for drive tests.
- Add concise log output after each run.

Acceptance criteria:

- Runs complete even when all sensors are unavailable.
- Available mock readings appear in result summaries/logs.

### Phase 3: GPU Provider

- Add dynamic NVIDIA NVML provider first.
- Add best-effort adapter matching.
- Add provider status tooltip.

Acceptance criteria:

- NVIDIA GPU temperature appears without blocking UI.
- Missing NVML shows `GPU N/A`, not an error dialog.

### Phase 4: SSD Provider

- Resolve target folder to physical disk.
- Query NVMe SMART/Health temperature via Windows storage APIs.
- Add SATA SMART fallback if practical.

Acceptance criteria:

- NVMe SSD temperature appears for drive benchmark target.
- Drive target changes refresh the mapped sensor.
- Unsupported drives show a clear `N/A` reason.

### Phase 5: CPU Provider

- Add optional LibreHardwareMonitor-compatible provider or helper.
- Label provider source clearly.
- Add ACPI thermal zone fallback only if it is explicitly labeled as such.

Acceptance criteria:

- CPU temperature appears when a supported provider is available.
- Unsupported or permission-blocked CPU temp does not affect benchmarks.

## Risks

- CPU temperature is not standardized on Windows.
- Some providers require elevated privileges or drivers.
- Vendor APIs may not map cleanly to `wgpu` adapters.
- SSD temperature APIs differ between NVMe, SATA, USB bridges, and RAID controllers.
- Polling too aggressively can add noise to benchmark timing.

Mitigations:

- Keep providers optional and isolated.
- Poll on a background thread.
- Use clear `N/A`, `stale`, and provider labels.
- Store temperature telemetry as supplemental metadata, not benchmark-critical data.

## Done Definition

The feature is ready when:

- A compact sensor panel is anchored to the bottom-right UI.
- Matrix and stress/repeat tests show CPU/GPU live readings when available.
- Drive tests show SSD live readings for the target drive when available.
- Each benchmark records start/end/max temperature summaries.
- Sensor failures never prevent a benchmark from running.
- Unit tests cover formatting, stale readings, max aggregation, and mocked benchmark summaries.
