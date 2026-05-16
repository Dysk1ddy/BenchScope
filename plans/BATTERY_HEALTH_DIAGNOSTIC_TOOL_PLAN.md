# Battery Health Diagnostic Tool Plan

## Goal

Add a laptop-focused Battery Health Diagnostic Tool to BenchScope as another option on the main menu.

The tool should summarize battery condition, current charging behavior, historical capacity loss, and runtime estimate quality using Windows battery data. It should be useful for quick health checks without pretending software can diagnose every physical battery failure.

Main menu entries should become:

- Matrix CPU/GPU Benchmark
- Matrix Stress Test
- Drive Benchmark
- RAM Tester
- Battery Health Diagnostic

If the system has no battery, the tool should open to a clear unsupported state instead of failing.

## Primary Features

The first useful version should show:

- Battery cycle count.
- Design capacity.
- Full charge capacity.
- Battery health percentage.
- Battery wear percentage.
- Current charge level.
- Current AC/battery state.
- Live charge or discharge rate when Windows exposes it.
- Estimated runtime remaining.
- Estimated runtime accuracy compared with recent observed drain.
- Charging behavior graph.
- Capacity history graph.
- Warnings for failed or unhealthy battery indicators.
- A manual symptom checklist for swollen battery risk.

## Important Safety Note

Software can report degraded capacity, abnormal charge behavior, and Windows battery failure flags. It cannot reliably detect swelling directly.

The UI should phrase swelling warnings as symptom guidance, not as a sensor diagnosis:

`BenchScope cannot directly detect battery swelling. If the laptop case is bulging, the touchpad is lifting, the keyboard deck is separating, or the device smells hot or chemical, stop using it and seek service.`

This belongs near the warning panel, not buried in documentation.

## Data Sources

Use layered Windows data sources so the tool still works when one provider is incomplete.

### Source 1: `powercfg /batteryreport`

Preferred historical source:

```powershell
powercfg /batteryreport /output "<temp>\benchscope_batteryreport.xml" /xml /duration 14
```

Reasons:

- Windows can emit XML directly.
- XML is safer and more stable to parse than the generated HTML report.
- The report contains installed battery metadata, recent usage, battery usage history, capacity history, and battery life estimates.

Use `/duration 14` by default for recent behavior. Offer later options for 3, 7, 14, 30, and full report.

Expected report sections to mine:

- Installed batteries.
- Recent usage.
- Battery usage.
- Usage history.
- Battery capacity history.
- Battery life estimates.

Parser guidance:

- Use an XML parser, not string scraping.
- Treat field names case-insensitively when possible.
- Normalize capacities to mWh.
- Preserve unknown or missing fields as `N/A`.
- Keep raw parse warnings in the diagnostic log.

### Source 2: Windows Battery WMI

Use WMI as a live telemetry source where available.

Likely useful classes under `root\wmi`:

- `BatteryStaticData`
- `BatteryFullChargedCapacity`
- `BatteryStatus`
- `BatteryCycleCount`

Useful fields, when exposed by firmware:

- Designed capacity.
- Full charged capacity.
- Remaining capacity.
- Cycle count.
- Voltage.
- Charge rate.
- Discharge rate.
- Charging/discharging state.

WMI support varies by vendor. Missing cycle count or rate data should not fail the tool.

### Source 3: System Power Status

Use the Windows `GetSystemPowerStatus` API for broad live state:

- AC line status.
- Battery flag.
- Battery life percent.
- Estimated lifetime.
- Estimated full lifetime.

This is a good fallback for current state, but it is not enough for full health scoring.

## Main Menu and Navigation

Add `Battery Health Diagnostic` as a top-level tool button.

The view should include:

- Back button in the top bar.
- Current tool title.
- Refresh button.
- Export report button later.
- Non-laptop unsupported state.

Back behavior:

- If no scan is running, return immediately.
- If a scan or live sampling session is running, cancel it before leaving.
- Do not leave background `powercfg` or WMI sampling work running after returning to the main menu.

## User Experience

The tool should feel like a compact diagnostic dashboard, not a long static report.

Recommended layout:

- Top status strip:
  - Health grade.
  - Wear percentage.
  - Full charge capacity.
  - Cycle count.
  - Current state.
- Left panel:
  - Battery identity and raw capacity values.
  - Current telemetry.
  - Warnings and symptoms checklist.
- Main panel:
  - Capacity history graph.
  - Charge/discharge behavior graph.
  - Runtime estimate accuracy summary.
- Bottom panel:
  - Recent events and parse/provider notes.

## Health Metrics

### Capacity Health

Formula:

```text
health_percent = full_charge_capacity_mwh / design_capacity_mwh * 100
```

Show as:

- `Full charge capacity: 47,200 mWh`
- `Design capacity: 56,000 mWh`
- `Health: 84.3%`

### Wear Percentage

Formula:

```text
wear_percent = max(0, 1 - full_charge_capacity_mwh / design_capacity_mwh) * 100
```

If full charge capacity is greater than design capacity, clamp wear to `0%` and add a note:

`Full charge capacity is above design capacity. This can happen on new packs or with vendor calibration differences.`

### Cycle Count

Show cycle count when available.

If unavailable:

`Cycle count: N/A - not exposed by this battery firmware`

Suggested severity:

- 0 to 300 cycles: normal for most packs.
- 301 to 600 cycles: aging.
- 601+ cycles: high cycle count.

Avoid treating cycle count alone as failure. Capacity and symptoms matter more.

### Live Charge and Discharge Rate

Use WMI `ChargeRate` or `DischargeRate` when available.

Display:

- Direction: charging, discharging, idle, AC connected.
- Rate in watts:

```text
watts = milliwatts / 1000
```

If only capacity samples are available, estimate rate from recent samples:

```text
estimated_watts = capacity_delta_mwh / elapsed_hours / 1000
```

Label estimated values clearly:

`Estimated from capacity samples`

### Estimated Runtime

Use Windows reported estimate when available.

Also compute observed runtime from recent drain:

```text
observed_hours_remaining = remaining_capacity_mwh / observed_discharge_mw
```

Only compute this while discharging and after enough sample time has elapsed, such as at least 3 minutes.

### Runtime Accuracy

Compare Windows estimated runtime against observed drain-based runtime.

```text
accuracy_error_percent =
    abs(windows_estimate_minutes - observed_estimate_minutes)
    / observed_estimate_minutes
    * 100
```

Suggested labels:

- Good: within 20%.
- Fair: 20% to 40%.
- Poor: over 40%.
- Unknown: insufficient live discharge samples.

Do not mark runtime accuracy as poor while the laptop is charging, idling at full charge, or switching between AC and battery repeatedly.

## Graphs

Use `egui_plot` if already available or add it only if the project accepts the dependency. Otherwise draw simple line graphs with `egui::Painter`.

### Capacity History Graph

Data:

- Date.
- Full charge capacity.
- Design capacity reference line.

Purpose:

- Shows long-term battery wear.
- Makes sudden capacity drops visible.

### Charging Behavior Graph

Data:

- Timestamp.
- Battery percentage.
- Capacity in mWh when available.
- Charge/discharge rate in watts when available.
- AC connected state.

Purpose:

- Shows whether charging is smooth, stuck, oscillating, or unexpectedly slow.

Sampling:

- Sample live state every 5 seconds while the tool is open.
- Keep an in-memory ring buffer for the current session.
- Do not write live samples to disk in the first version.

## Warnings and Alerts

Warnings should be specific and evidence-based.

### Capacity Warnings

- Health below 80%:
  - `Battery health is below 80%. Runtime may be noticeably reduced.`
- Health below 60%:
  - `Battery health is severely degraded. Replacement should be considered.`
- Sudden capacity drop:
  - `Full charge capacity dropped sharply in the recent history. Recalibration or replacement may be needed.`

### Cycle Count Warnings

- High cycle count:
  - `Cycle count is high. Reduced capacity is expected on older packs.`

### Failure State Warnings

Use Windows battery flags and WMI status when exposed:

- Failed.
- Critical.
- Unknown state with no capacity response.
- Battery present but no capacity data.
- Not charging while AC is connected and charge is low.

### Swollen Battery Symptom Alert

This should be a manual checklist, not an automatic diagnosis.

Checklist items:

- Laptop case or bottom cover is bulging.
- Touchpad is lifting or hard to click.
- Keyboard deck is lifting or separating.
- Device rocks on a flat surface when it used to sit flat.
- Battery area is unusually hot while idle.
- Sweet, metallic, chemical, or solvent-like smell.

If any are selected, show a strong warning:

`Stop using the device, unplug it if safe, avoid charging it, and contact the laptop manufacturer or a repair professional. Do not puncture or press on the battery.`

## Data Model

Suggested Rust types:

```rust
struct BatteryDiagnosticState {
    scan_status: BatteryScanStatus,
    latest_report: Option<BatteryReport>,
    live_samples: VecDeque<BatteryLiveSample>,
    warnings: Vec<BatteryWarning>,
    log: Vec<String>,
}

enum BatteryScanStatus {
    Idle,
    Running,
    CancelRequested,
    Completed,
    Failed(String),
    Unsupported(String),
}

struct BatteryReport {
    generated_at: SystemTime,
    batteries: Vec<BatteryInfo>,
    capacity_history: Vec<BatteryCapacityPoint>,
    usage_history: Vec<BatteryUsagePoint>,
    life_estimates: Vec<BatteryLifeEstimate>,
    provider_notes: Vec<String>,
}

struct BatteryInfo {
    name: Option<String>,
    manufacturer: Option<String>,
    serial_number: Option<String>,
    chemistry: Option<String>,
    design_capacity_mwh: Option<f64>,
    full_charge_capacity_mwh: Option<f64>,
    cycle_count: Option<u32>,
}

struct BatteryHealthSummary {
    health_percent: Option<f64>,
    wear_percent: Option<f64>,
    cycle_count: Option<u32>,
    grade: BatteryHealthGrade,
    notes: Vec<String>,
}

enum BatteryHealthGrade {
    Excellent,
    Good,
    Fair,
    Poor,
    Failed,
    Unknown,
}

struct BatteryLiveSample {
    timestamp: Instant,
    ac_connected: Option<bool>,
    percent: Option<f32>,
    remaining_capacity_mwh: Option<f64>,
    charge_rate_watts: Option<f64>,
    discharge_rate_watts: Option<f64>,
    estimated_runtime_minutes: Option<f64>,
}

struct BatteryWarning {
    severity: BatteryWarningSeverity,
    title: String,
    detail: String,
    source: BatteryWarningSource,
}
```

## Worker Flow

Use a background worker for report generation and parsing.

Flow:

1. Create a temporary output path under the system temp directory.
2. Run `powercfg /batteryreport /output <path> /xml /duration 14`.
3. Parse the XML report.
4. Query WMI for live battery fields.
5. Query `GetSystemPowerStatus`.
6. Merge fields by battery identity when possible.
7. Compute health, wear, warnings, and runtime accuracy.
8. Emit a completed event to the UI.
9. Remove the temporary XML file unless debug retention is enabled later.

Events:

```rust
enum BatteryWorkerEvent {
    Log(String),
    ReportReady(BatteryReport),
    LiveSample(BatteryLiveSample),
    Warning(BatteryWarning),
    Failed(String),
    Unsupported(String),
}
```

Cancellation:

- Check before launching `powercfg`.
- If cancel is requested while `powercfg` is running, terminate the child process.
- Stop live sampling immediately when leaving the tool.
- Clean up temp files on cancellation.

## XML Parsing Strategy

The `powercfg` XML schema may vary slightly between Windows versions and battery firmware providers.

Parser rules:

- Parse structurally.
- Match likely section names and fields defensively.
- Support capacity units with commas, spaces, and `mWh` suffixes.
- Convert all capacities to `f64` mWh.
- Skip rows that cannot be parsed and log the row label.
- Keep all missing fields as `None`.

Capacity parser examples:

- `56,000 mWh` -> `56000.0`
- `56000` -> `56000.0`
- `-` -> `None`

Date/time parser:

- Prefer Windows report timestamps when present.
- If parsing fails, keep the point with an unknown timestamp only if the value is still useful.

## App Architecture Changes

Add a new top-level view:

```rust
enum AppView {
    MainMenu,
    MatrixBenchmark,
    MatrixStressTest,
    DriveBenchmark,
    RamTester,
    BatteryHealthDiagnostic,
}
```

Add battery-specific modules once implementation starts:

```text
src/
  battery/
    mod.rs
    powercfg.rs
    wmi.rs
    system_power.rs
    metrics.rs
    warnings.rs
  ui/
    battery.rs
```

Keep battery parsing and health calculations separate from `egui` so they can be unit tested.

## UI Controls

Initial controls:

- Refresh scan.
- Start live sampling.
- Stop live sampling.
- Report duration:
  - 3 days
  - 7 days
  - 14 days
  - 30 days
- Manual symptom checklist.

Future controls:

- Export diagnostic summary.
- Open generated battery report.
- Save live sampling session.
- Calibration helper notes.

## Result Formatting

Use consistent units:

- Capacity: mWh and Wh.
- Rate: W.
- Runtime: hours and minutes.
- Percentages: one decimal place.
- Cycle count: integer.

Example summary:

```text
Battery health: 84.3% Good
Wear: 15.7%
Design capacity: 56,000 mWh
Full charge capacity: 47,200 mWh
Cycle count: 412
Current state: Discharging at 8.4 W
Windows estimate: 4h 10m
Observed estimate: 3h 45m
Runtime estimate accuracy: Good, 11.1% error
```

## Implementation Phases

### Phase 1: Markdown Plan and Menu Placeholder

Tasks:

- Add this plan.
- Add a main-menu placeholder entry later if the UI already supports placeholder tools.

Acceptance criteria:

- The project has a written plan for the feature.
- The intended main-menu placement is clear.

### Phase 2: Battery View Skeleton

Tasks:

- Add `BatteryHealthDiagnostic` to `AppView`.
- Add the main-menu button.
- Add the battery diagnostic screen with Back and Refresh controls.
- Show unsupported state on systems without a battery.

Acceptance criteria:

- The new option appears on the main menu.
- Back returns to the main menu.
- The view can show placeholder health fields without running diagnostics yet.

### Phase 3: `powercfg` XML Report Parser

Tasks:

- Run `powercfg /batteryreport /xml`.
- Parse installed batteries.
- Parse design capacity, full charge capacity, and cycle count.
- Parse capacity history.
- Compute health and wear.

Acceptance criteria:

- The tool shows health, wear, and cycle count when present in the report.
- Missing cycle count is displayed as `N/A`.
- Parser tests cover capacity string normalization.

### Phase 4: Live Telemetry

Tasks:

- Query `GetSystemPowerStatus`.
- Query WMI battery classes when available.
- Add live samples every 5 seconds while the view is active.
- Compute current charge/discharge rate from direct WMI rate data or sample deltas.

Acceptance criteria:

- Current AC/battery state updates live.
- Charge/discharge rate is shown when available.
- Estimated rate is clearly labeled when inferred.

### Phase 5: Graphs

Tasks:

- Add capacity history graph.
- Add charging behavior graph.
- Handle missing data gracefully.

Acceptance criteria:

- Graphs render without layout overlap.
- Missing data produces an explanatory empty state.
- The battery view remains responsive while live samples update.

### Phase 6: Warnings and Runtime Accuracy

Tasks:

- Add health thresholds.
- Add cycle count notes.
- Add failure status warnings.
- Add runtime estimate accuracy calculation.
- Add swollen battery symptom checklist and warning copy.

Acceptance criteria:

- Warnings are evidence-based and specific.
- Swelling text is presented as manual symptom guidance.
- Runtime accuracy is only shown when enough discharge data exists.

### Phase 7: Tests and Documentation

Tasks:

- Add unit tests for capacity parsing.
- Add unit tests for wear calculation.
- Add unit tests for runtime accuracy labels.
- Add README feature note.

Acceptance criteria:

- Battery metric calculations are tested.
- Unsupported desktops do not fail the app.
- README explains that the tool is Windows/laptop-focused.

## Testing Plan

Automated tests:

- Capacity value parsing.
- Health and wear formulas.
- Full charge above design capacity clamping.
- Runtime accuracy labels.
- Warning threshold generation.
- Missing field handling.
- XML parser behavior for representative report snippets.

Manual tests:

- Laptop on AC power.
- Laptop on battery power.
- Laptop charging below 80%.
- Laptop fully charged on AC.
- Desktop with no battery.
- Battery where cycle count is missing.
- Battery where WMI rate data is unavailable.
- Cancel refresh while `powercfg` is running.
- Leave the tool while live sampling is active.

## Risks

### Vendor Data Gaps

Risk:

- Some laptops do not expose cycle count, charge rate, or full WMI battery fields.

Mitigation:

- Show `N/A` rather than errors.
- Merge `powercfg`, WMI, and `GetSystemPowerStatus`.
- Explain missing firmware fields in the notes area.

### Misleading Failure Claims

Risk:

- Software-only health checks can overstate certainty.

Mitigation:

- Use careful language.
- Separate measured battery health from physical symptom guidance.
- Never claim swelling was automatically detected.

### `powercfg` Runtime

Risk:

- `powercfg /batteryreport` may take a noticeable amount of time or fail on unsupported systems.

Mitigation:

- Run it in a background worker.
- Show progress/log messages.
- Support cancellation.
- Keep a clear unsupported state.

### XML Variability

Risk:

- Battery report XML can differ across Windows versions.

Mitigation:

- Use tolerant parsing.
- Unit test with saved snippets.
- Keep provider notes visible.

## Initial Default Settings

Recommended defaults:

- Report duration: 14 days.
- Live sampling: on while the battery view is open.
- Live sample interval: 5 seconds.
- Health warning threshold: below 80%.
- Severe health threshold: below 60%.
- Runtime accuracy minimum sample window: 3 minutes of discharging.
- Temporary XML cleanup: enabled.

## Definition of Done

The Battery Health Diagnostic Tool is ready when:

- The main menu includes `Battery Health Diagnostic`.
- The view works on laptops and shows an unsupported state on desktops.
- Design capacity and full charge capacity are displayed.
- Battery health and wear are calculated.
- Cycle count is shown when available.
- Current AC/battery state is shown.
- Charge/discharge rate is shown or explicitly marked unavailable.
- Runtime estimate accuracy is calculated when enough data exists.
- Capacity history and charging behavior graphs render cleanly.
- Battery warnings are clear and evidence-based.
- Swollen battery guidance is presented as manual symptom guidance.
- Diagnostics run on a background worker and can be canceled.
- Existing BenchScope tools still work after the menu addition.
