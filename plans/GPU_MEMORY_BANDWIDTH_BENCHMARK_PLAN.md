# GPU Memory Bandwidth Benchmark Plan

## Goal

Add a separate BenchScope tool for measuring GPU memory bandwidth across the main data paths a user cares about:

- GPU-internal buffer reads and writes.
- GPU buffer-to-buffer copies.
- CPU-to-GPU uploads.
- GPU-to-CPU readbacks.
- Optional end-to-end round trips.

The feature should be labeled as **GPU Memory Bandwidth**, not only **VRAM bandwidth**, because integrated GPUs often use shared system memory instead of dedicated VRAM.

## Product Fit

BenchScope already has the pieces this feature needs:

- `wgpu` adapter enumeration and adapter metadata.
- Timestamp-query awareness.
- Progress, cancellation, and background-worker patterns.
- Sensor summaries for GPU temperature/utilization.
- Result tables and diagnostic logs.
- VRAM/shared-memory reporting through DXGI/wgpu metadata.

This benchmark should be a new main-menu tool, separate from the matrix benchmark, so matrix results stay focused on compute throughput while memory results describe data movement.

## User-Facing Tool

Suggested menu item:

```text
GPU Memory Bandwidth
Measure GPU internal memory, copy, upload, and readback throughput.
```

Suggested controls:

- GPU adapter picker.
- Buffer size: `Auto`, `64 MiB`, `256 MiB`, `512 MiB`, `1 GiB`, `2 GiB`.
- Iterations or duration target.
- Test checkboxes:
  - Internal read/write.
  - GPU buffer copy.
  - CPU to GPU upload.
  - GPU to CPU readback.
  - Round trip.
- Run / Cancel.

Suggested result columns:

| Column | Meaning |
| --- | --- |
| Test | Which memory path was measured. |
| Buffer | Buffer size used for each iteration. |
| Iterations | Completed measured passes. |
| Bytes processed | Total bytes counted for bandwidth. |
| Time | Measured elapsed time. |
| Bandwidth | GB/s or GiB/s. |
| Timing source | GPU timestamp or CPU-observed wall time. |
| Adapter | Selected GPU/backend. |
| GPU temp | Start/end/max when available. |
| Validation | Sample/checksum validation status. |
| Notes | Fallbacks, chunking, timing caveats. |

## Benchmark Types

### 1. Internal Read/Write Kernel

Purpose:

Measure the closest practical approximation of GPU memory or VRAM bandwidth.

Implementation:

- Allocate two read-only storage buffers and one writable storage buffer.
- Run a WGSL compute shader that streams through the buffers.
- Use a low-arithmetic operation so memory movement dominates:

```wgsl
dst[i] = src_a[i] ^ rotate_or_mix(src_b[i]);
```

Recommended data shape:

- Use `u32` or `vec4<u32>`.
- Prefer `vec4<u32>` to encourage wider loads/stores.

Byte accounting:

- For scalar `u32`: two reads plus one write = `12 bytes` per element.
- For `vec4<u32>`: two 16-byte reads plus one 16-byte write = `48 bytes` per element.

Timing:

- Use GPU timestamp queries when supported.
- Fall back to CPU-observed submit/wait time and label it clearly.

Validation:

- Avoid full readback after every measured pass.
- Read back a tiny sample buffer or sampled slice after the run.
- Validate deterministic first/middle/last samples.

### 2. GPU Buffer Copy

Purpose:

Measure GPU-side copy throughput using `copy_buffer_to_buffer`.

Implementation:

- Allocate source and destination buffers with `COPY_SRC` / `COPY_DST`.
- Encode repeated `copy_buffer_to_buffer` commands.
- Submit and wait for completion.

Timing:

- Use CPU-observed submit/wait timing in the first version.
- Label as copy-path timing because it includes driver scheduling and synchronization.

Validation:

- Copy a small sampled region to a readback buffer and verify the expected pattern.

### 3. CPU To GPU Upload

Purpose:

Measure host-to-GPU transfer throughput.

Implementation options:

- First version: use `queue.write_buffer`.
- Later version: compare staging-buffer upload paths.

Timing:

- CPU-observed time from upload start through GPU-visible completion.
- Label honestly because this measures API/staging/driver behavior, not just PCIe bandwidth.

Validation:

- Launch a tiny shader or copy a sample back to confirm the uploaded data became visible on the GPU.

### 4. GPU To CPU Readback

Purpose:

Measure readback throughput from GPU memory to CPU-visible memory.

Implementation:

- Fill or compute a GPU buffer.
- Copy to a `MAP_READ` buffer.
- Submit, wait, map, and read.

Timing:

- CPU-observed total readback time.
- Include map time because that is what users experience.

Validation:

- Validate sampled values while the mapped range is available.

### 5. Round Trip

Purpose:

Measure end-to-end data movement:

```text
CPU memory -> GPU buffer -> GPU touch/copy -> CPU readback
```

This is useful for workloads that transfer data every frame or every compute task.

Timing:

- CPU-observed total time.
- Report separately from upload/readback so users do not mistake it for raw VRAM speed.

## Sizing Rules

The benchmark should avoid accidentally exhausting VRAM or shared memory.

Recommended presets:

| Preset | Bytes |
| --- | ---: |
| 64 MiB | 67,108,864 |
| 256 MiB | 268,435,456 |
| 512 MiB | 536,870,912 |
| 1 GiB | 1,073,741,824 |
| 2 GiB | 2,147,483,648 |

Recommended `Auto` behavior:

```text
min(512 MiB, 10-15% of reported adapter memory)
```

Clamp by:

- `wgpu::Limits::max_buffer_size`.
- `wgpu::Limits::max_storage_buffer_binding_size` for shader tests.
- Available adapter memory estimate when BenchScope has one.

If a selected buffer is too large:

- Warn before running.
- Offer a smaller suggested size.
- Use chunking only when the result can still be labeled clearly.

## Timing Rules

Each test should:

1. Warm up once.
2. Run enough measured iterations to avoid tiny one-shot timing noise.
3. Stop when it reaches either:
   - a configured iteration count, or
   - a target duration around 1-2 seconds.

Report:

- Best bandwidth.
- Average bandwidth.
- Total elapsed time.
- Timing source.

Use decimal GB/s for hardware-style bandwidth reporting:

```text
GB/s = bytes_processed / elapsed_seconds / 1_000_000_000
```

Optional: show GiB/s in a tooltip or secondary field later.

## WGSL Kernel Sketch

```wgsl
struct Params {
    element_count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(0) @binding(0) var<storage, read> src_a: array<vec4<u32>>;
@group(0) @binding(1) var<storage, read> src_b: array<vec4<u32>>;
@group(0) @binding(2) var<storage, read_write> dst: array<vec4<u32>>;
@group(0) @binding(3) var<uniform> params: Params;

@compute @workgroup_size(256, 1, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= params.element_count) {
        return;
    }

    let a = src_a[i];
    let b = src_b[i];
    dst[i] = (a ^ b) + vec4<u32>(0x9E3779B9u);
}
```

Notes:

- This is intentionally simple.
- Too much arithmetic would turn the test into a compute benchmark.
- A later version can add variants for read-only, write-only, read/write, and copy-like kernels.

## Rust Module Shape

Suggested new feature directory:

```text
src/features/gpu_memory_benchmark/
  mod.rs
  model.rs
  runner.rs
  ui.rs
```

Suggested includes:

```rust
include!("features/gpu_memory_benchmark/mod.rs");
```

Suggested app view:

```rust
enum AppView {
    MainMenu,
    MatrixBenchmark,
    MatrixStressTest,
    GpuMemoryBenchmark,
    DriveBenchmark,
    StorageHealth,
    RamTester,
    BatteryHealthDiagnostic,
    NetworkDiagnostic,
    DeviceInfo,
}
```

Suggested model types:

```rust
enum GpuMemoryTestKind {
    InternalReadWrite,
    DeviceCopy,
    Upload,
    Readback,
    RoundTrip,
}

enum GpuMemoryBufferSize {
    Auto,
    Mib64,
    Mib256,
    Mib512,
    Gib1,
    Gib2,
}

struct GpuMemoryBenchmarkConfig {
    adapter: AdapterInfo,
    buffer_size_bytes: u64,
    target_duration_s: f64,
    selected_tests: Vec<GpuMemoryTestKind>,
}

struct GpuMemoryBenchmarkResult {
    test: GpuMemoryTestKind,
    adapter: String,
    buffer_size_bytes: u64,
    iterations: u32,
    bytes_processed: u64,
    elapsed_ms: f64,
    bandwidth_gbps: f64,
    timing_source: GpuMemoryTimingSource,
    validation: String,
    notes: Vec<String>,
    gpu_temperature: TemperatureSummary,
}
```

## App Integration

Add to `BenchScopeApp`:

```rust
gpu_memory: GpuMemoryBenchmarkState,
gpu_memory_back_confirm: bool,
```

Startup:

- Initialize `GpuMemoryBenchmarkState::new()`.
- Reuse the existing adapter list from startup.

Runtime polling:

- Poll GPU memory benchmark worker events.
- Request repaint while running.
- Finish GPU temperature summary when the run ends.

Sensors:

- Show GPU sensor rows for this view.
- Reuse integrated-GPU CPU-package fallback behavior.

Back behavior:

- If a run is active, ask before returning to the main menu.

## Worker Events

Suggested events:

```rust
enum GpuMemoryWorkerEvent {
    Progress(GpuMemoryProgress),
    TestDone(GpuMemoryBenchmarkResult),
    Done(Result<Vec<GpuMemoryBenchmarkResult>, String>),
    Log(String),
}

struct GpuMemoryProgress {
    current_test: String,
    current_progress: f32,
    suite_progress: f32,
    elapsed_s: f64,
    eta_s: Option<f64>,
    bytes_processed: u64,
}
```

## Validation Strategy

Internal read/write:

- Initialize buffers with deterministic patterns.
- Read back a small sampled slice after measured passes.
- Validate expected transformed values.

Copy:

- Validate sampled copied values.

Upload:

- Validate sampled uploaded values by copying a tiny range back.

Readback:

- Validate mapped values directly.

Round trip:

- Validate final mapped values.

Validation should never dominate benchmark time. Keep full-buffer validation out of measured timing unless the test itself is readback.

## Safety And Accuracy Notes

The UI should explain:

- Internal read/write is the closest result to GPU memory or VRAM bandwidth.
- Upload/readback include API, staging, bus, synchronization, and driver overhead.
- Integrated GPU results are shared-memory results.
- Results can change with power mode, thermals, driver version, display activity, and active GPU load.

Avoid saying:

- "This is exact VRAM bandwidth."
- "This proves PCIe bandwidth."
- "This is comparable to vendor memory clock marketing numbers."

## Implementation Phases

### Phase 1: Shell And UI

- Add `GpuMemoryBenchmarkState`.
- Add main menu entry and `AppView`.
- Add controls, progress bars, result grid, and log.
- No real GPU tests yet; wire state and cancellation shape.

### Phase 2: GPU Copy And Readback

- Implement buffer pattern generation.
- Implement GPU buffer copy test.
- Implement GPU-to-CPU readback test.
- Add validation and unit tests for byte accounting.

### Phase 3: Internal Read/Write Kernel

- Add WGSL shader.
- Add compute pipeline and bind group layout.
- Add timestamp-query timing when supported.
- Add CPU-observed fallback.

### Phase 4: Upload And Round Trip

- Add `queue.write_buffer` upload test.
- Add round-trip test.
- Label timing caveats in result notes.

### Phase 5: Polish

- Add auto sizing.
- Add VRAM/shared-memory warning.
- Attach GPU temperature summaries.
- Add CLI entry later:

```powershell
.\target\release\BenchScope.exe --gpu-memory-test --memory-size 512m --adapter 0
```

## Tests

Recommended unit tests:

- Buffer-size parser accepts `64m`, `256MiB`, `1g`, `auto`.
- Auto size clamps to adapter memory and buffer limits.
- Internal read/write byte accounting is correct.
- Copy byte accounting is correct.
- Bandwidth calculation uses decimal GB/s.
- Validation catches a sampled mismatch.
- Canceled worker stops before starting the next test.

Recommended manual tests:

- Discrete NVIDIA/AMD GPU.
- Integrated Intel/AMD GPU.
- Software adapter behavior.
- Timestamp-query supported and unsupported adapters.
- Small buffers where overhead dominates.
- Large buffers near the warning threshold.

## Definition Of Done

- GPU Memory Bandwidth appears as a separate main-menu tool.
- The tool can run at least internal read/write, copy, upload, and readback tests.
- Results clearly distinguish GPU-internal, upload, readback, and round-trip paths.
- The UI remains responsive and cancelable.
- Unsupported timestamp queries do not break the benchmark.
- Large buffer choices warn before misleading or risky runs.
- Results include adapter name, timing source, validation, notes, and GPU temperature when available.
