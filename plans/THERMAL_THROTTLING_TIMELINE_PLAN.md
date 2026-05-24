# Thermal Throttling Timeline Plan

## Goal

Add a thermal throttling timeline to BenchScope that records and graphs temperature, utilization, clock, power, and throughput during stress and benchmark runs.

The feature should help answer:

- Did performance drop during the run?
- Did the drop correlate with CPU/GPU/SSD temperature?
- Was utilization stable, or did the workload stop keeping the hardware busy?
- Were clocks or power readings available, and did they fall when temperature rose?
- Is this likely thermal throttling, power limiting, storage thermal protection, driver scheduling, or an unrelated workload issue?

## Current App Baseline

BenchScope already has the foundation:

- `SensorManager` continuously samples `SensorSnapshot` at about 1 Hz.
- `SensorReading` supports temperature, utilization, memory usage, voltage, power, and clock metrics.
- Sensor UI already formats and displays `SensorMetricKind::Temperature`, `Utilization`, `Power`, and `Clock`.
- `TemperatureRunTracker` records start/end/max summaries for matrix, drive, and GPU-memory runs.
- Matrix stress emits `RepeatProgress` with iteration count, latest time, average time, compute average, and derived TFLOP/s.
- Single matrix benchmark emits progress, then final CPU/GPU timing and temperature summaries.
- Drive and GPU-memory benchmarks attach temperature summaries after completion.
- Run history and support bundles can persist and export structured records.

Missing pieces:

- No per-run time-series storage.
- No timeline UI graph.
- No correlation between performance and sensor changes.
- No throttling classification or confidence score.
- No timeline report export.

## Product Principles

- Keep the existing compact sensor panel and temperature summaries.
- Add timeline collection only while a benchmark/stress/diagnostic run is active.
- Timeline collection must not block benchmark worker threads.
- Treat missing clocks, power, or temperatures as normal provider gaps.
- Avoid overclaiming. Say "performance drop correlated with rising temperature" unless there is direct throttle-status evidence.
- Keep sample memory bounded.
- Persist summarized timeline evidence in history/support bundles, not every long-run sample by default.

## Initial Scope

Implement timeline support for:

- Matrix stress test.
- Matrix single benchmark.
- GPU memory benchmark.
- Drive benchmark.
- AI training benchmark where progress/throughput is available.

Later expansion:

- Network continuous monitor can use the same charting pattern, but it is not thermal throttling focused.
- RAM tester can show CPU/RAM temperature and memory utilization, but result throughput is less direct.

## Data Model

Add a run timeline model:

```rust
struct RunTimeline {
    run_id: String,
    scope: TimelineScope,
    started_at: SystemTime,
    samples: Vec<TimelineSample>,
    max_samples: usize,
}

enum TimelineScope {
    MatrixBenchmark,
    MatrixStress,
    GpuMemory,
    DriveBenchmark,
    AiTraining,
}

struct TimelineSample {
    elapsed_ms: u64,
    sensor: TimelineSensorSample,
    throughput: Option<TimelineThroughputSample>,
    phase: String,
}
```

Sensor sample:

```rust
struct TimelineSensorSample {
    cpu_temp_c: Option<f32>,
    gpu_temp_c: Option<f32>,
    gpu_memory_temp_c: Option<f32>,
    drive_temp_c: Option<f32>,
    memory_temp_c: Option<f32>,
    cpu_util_percent: Option<f32>,
    gpu_util_percent: Option<f32>,
    drive_util_percent: Option<f32>,
    memory_util_percent: Option<f32>,
    cpu_clock_mhz: Option<f32>,
    gpu_clock_mhz: Option<f32>,
    cpu_power_w: Option<f32>,
    gpu_power_w: Option<f32>,
}
```

Throughput sample:

```rust
struct TimelineThroughputSample {
    label: String,
    value: f64,
    unit: String,
}
```

Examples:

- Matrix stress: `TFLOP/s`, `iterations/min`, average ms.
- Matrix single: phase progress and eventual CPU/GPU timing markers.
- GPU memory: `GB/s` where available.
- Drive benchmark: `MB/s`, IOPS, latency per active subtest.
- AI training: samples/s, tokens/s, parameters/s, step latency.

## Sampling Strategy

Sampling loop:

- Reuse the UI thread's existing `observe_temperature_run` cadence.
- When a run timeline is active, append one `TimelineSample` every 1 second.
- Also append an immediate sample at run start and run finish.
- Attach the latest available progress/throughput from each running tool.

Retention per active run:

- Default max samples: 3,600 samples, enough for one hour at 1 Hz.
- For infinite stress tests, downsample older samples:
  - Keep first 5 minutes at 1 Hz.
  - Keep last 15 minutes at 1 Hz.
  - Keep middle section at 10-second buckets.

Do not make sensor sampling faster than the provider layer can safely support.

## Throughput Capture

Add helper methods on `BenchScopeApp`:

```rust
fn current_timeline_throughput(&self, scope: TimelineScope) -> Option<TimelineThroughputSample>;
fn current_timeline_phase(&self, scope: TimelineScope) -> String;
```

Source mapping:

- Matrix stress: use `RepeatProgress::throughput_tflops`, `iterations_per_second`, latest/average ms.
- Matrix single: use progress phase; final marker from `BenchmarkResult`.
- GPU memory: use current progress while running; final per-test GB/s markers after completion.
- Drive benchmark: use current test, bytes processed, operations, suite progress; final per-test MB/s/IOPS.
- AI training: use progress completed steps and final throughput when result is pushed.

## UI Design

Add timeline panels to the affected tool views:

- Matrix Stress Test: primary timeline panel below live progress.
- Matrix Benchmark: collapsed timeline panel during a run, expanded after completion.
- GPU Memory Benchmark: timeline panel near results.
- Drive Benchmark: timeline panel near results.
- AI Training Benchmark: timeline panel near progress/results.

Graph layout:

- One unframed timeline area, not nested cards.
- Shared x-axis: elapsed time.
- Toggle series:
  - Temperature.
  - Utilization.
  - Clock.
  - Power.
  - Throughput.
- Device toggles:
  - CPU.
  - GPU.
  - VRAM.
  - SSD.
  - RAM.
- Markers:
  - Run start.
  - Run finish.
  - Benchmark subtest changes.
  - Detected performance-drop window.
  - Temperature warning/critical threshold crossings.

Implementation note:

- Avoid adding a plotting dependency initially.
- Use `egui::Painter` to draw lightweight line graphs, similar to the existing battery capacity graph.
- Add a reusable `ui_timeline_graph` helper after the data model is stable.

## Throttling Analysis

Add a summary analyzer:

```rust
struct TimelineAnalysis {
    findings: Vec<TimelineFinding>,
    peak_temperatures: Vec<TimelinePeak>,
    throughput_drop_percent: Option<f64>,
    correlated_temperature_rise: bool,
    confidence: TimelineConfidence,
}
```

Finding examples:

- `GPU throughput dropped 28% while GPU temperature rose from 73 C to 87 C.`
- `CPU utilization stayed near 100%, but CPU clock samples were unavailable.`
- `Drive throughput dropped after SSD temperature crossed 60 C.`
- `Performance varied, but no matching thermal rise was observed.`
- `Sensor provider did not expose temperature; throttling cannot be assessed.`

Confidence levels:

- `High`: throughput drop, temperature rise, and clock/power drop or direct throttle flag if later available.
- `Medium`: throughput drop and temperature rise with steady utilization.
- `Low`: throughput drop but missing temperature/clock/power data.
- `None`: no meaningful performance drop or no timeline data.

Initial heuristics:

- Throughput drop threshold: 10% caution, 20% warning.
- CPU temp threshold: 85 C caution, 95 C critical.
- GPU temp threshold: 80 C caution, 90 C critical.
- VRAM temp threshold: 90 C caution, 100 C critical.
- SSD temp threshold: 60 C caution, 70 C critical.
- Require at least 5 samples for trend analysis.
- Use rolling averages over 3-5 samples to avoid reacting to single-sample noise.

## History and Support Bundle Integration

Extend history records with timeline summaries:

- Peak CPU/GPU/VRAM/SSD/RAM temperatures.
- Lowest/average/highest throughput.
- Largest throughput drop percent.
- Correlation confidence.
- Top timeline findings.

Do not store full sample arrays in normal history by default.

Support bundle should include:

```text
reports/
  thermal-timeline-summary.md
history/
  recent-timeline-summaries.redacted.json
```

Optional advanced export:

- Include full redacted timeline samples as CSV when user enables `Include detailed timelines`.

## File Layout

Proposed source layout:

```text
src/
  timeline/
    mod.rs
    model.rs
    capture.rs
    analysis.rs
    ui.rs
    report.rs
```

Wire into `src/main.rs`:

```rust
include!("timeline/mod.rs");
```

Add to `BenchScopeApp`:

```rust
timeline: TimelineState,
```

`TimelineState` owns:

- Current active timeline.
- Last completed timeline summaries.
- UI series toggles.
- Last analysis result.

## Implementation Phases

### Phase 1: Timeline Model and Capture

- Add `timeline` module and DTOs.
- Add `TimelineState`.
- Start timeline when supported runs begin.
- Append samples during `ui_app` while runs are active.
- Finish timeline when runs complete or cancel.
- Preserve existing `TemperatureRunTracker` for start/end/max summaries.

Acceptance:

- Matrix stress creates a timeline with sensor samples and throughput samples.
- Matrix single creates a timeline and final summary.
- No timeline capture occurs while idle.

### Phase 2: Timeline UI Graph

- Add reusable `ui_timeline_graph`.
- Render temperature and throughput on separate y-axis bands first.
- Add toggles for device and metric families.
- Add threshold lines for thermal caution/critical values.
- Add run phase labels/markers.

Acceptance:

- User can watch temperature and throughput move during matrix stress.
- Missing series do not break graph rendering.
- Text and graph labels fit on small and large windows.

### Phase 3: Throttling Analysis

- Add rolling-average trend helpers.
- Detect throughput drops.
- Detect temperature rises near drops.
- Add confidence labels and cautious findings.
- Show findings under the graph.

Acceptance:

- Synthetic samples detect a clear heat-correlated performance drop.
- Synthetic samples avoid false thermal finding when throughput drops without heat rise.
- Provider gaps produce clear "cannot assess" notes.

### Phase 4: Tool Coverage

- Add GPU memory timeline throughput.
- Add drive benchmark timeline throughput and SSD temperature analysis.
- Add AI training throughput/step-latency timeline.
- Add timeline marker support for subtests and phases.

Acceptance:

- Each target tool shows a timeline during and after a run.
- Final results link to the timeline summary.

### Phase 5: Export and History

- Add timeline summary to history events.
- Add Markdown timeline report renderer.
- Add support bundle timeline summary.
- Optional: add CSV export for full samples.

Acceptance:

- Support bundle includes thermal timeline findings.
- History comparison can show thermal regression summaries.

## Tests

Unit tests:

- Timeline sample append and max-sample retention.
- Downsampling keeps start and recent samples.
- Throughput drop calculation.
- Temperature threshold crossing.
- Correlation confidence for high/medium/low cases.
- Missing sensor data produces provider-gap notes.
- Report renderer includes peak temps, throughput drop, and findings.

Manual tests:

- Matrix stress for 1 minute with sensors available.
- Matrix stress on a system with missing temperature provider.
- GPU memory benchmark with NVIDIA GPU temperature available.
- Drive benchmark with SSD temperature available.
- Long/infinite stress run downsampling behavior.
- Support bundle includes timeline summary and redacts paths/IDs.

## Risks and Mitigations

- Sensor availability varies widely.
  - Show provider status and confidence instead of hard conclusions.

- Clock and power readings may not exist in safe Windows providers.
  - Use them opportunistically and mark unavailable series clearly.

- Graph complexity can overwhelm the tool views.
  - Start with temperature plus throughput, then add utilization/clocks/power toggles.

- Infinite stress tests can grow sample memory.
  - Enforce max samples and downsampling.

- Thermal correlation can be misleading if another process interferes.
  - Include utilization stability and confidence notes.

## Recommended First Milestone

Start with matrix stress:

1. Add `timeline` module and `TimelineState`.
2. Capture CPU/GPU/VRAM temperature, CPU/GPU utilization, and stress throughput at 1 Hz.
3. Draw a simple temperature-plus-throughput graph in the Matrix Stress Test view.
4. Add basic analysis: peak temperatures and largest throughput drop.
5. Add timeline summary to support bundles.

This proves the core correlation workflow before expanding to drive, GPU memory, matrix single-run, and AI training paths.
