# Hardware Acceleration Tester Plan

## Purpose

Create a desktop application that benchmarks matrix multiplication on CPU versus GPU, reports timing breakdowns, and supports repeatable stress-style testing. The first version should be small, accurate, and designed so future benchmark types can be added without rewriting the core.

## Recommended Stack

Use Rust for the application.

Primary libraries:

- `wgpu` for cross-vendor GPU compute through the system graphics backends.
- WGSL compute shaders for matrix multiplication kernels.
- `eframe` / `egui` for a native desktop UI.
- `rayon` optionally for a later multi-threaded CPU comparison mode, while the first required CPU baseline remains CPU-only and clearly labeled.
- `serde` + `serde_json` or `csv` for exporting benchmark results later.

Why Rust + `wgpu`:

- Native compiled performance without a heavy runtime.
- One GPU path can work across NVIDIA, AMD, Intel, and integrated GPUs where supported.
- `wgpu` exposes adapter information, which allows presenting selectable GPUs when more than one adapter is available.
- `wgpu` supports timestamp queries, which allows separating GPU compute time from CPU-observed transfer and dispatch overhead.
- The same benchmark engine can later be reused by a CLI, desktop GUI, or automated test runner.

Alternatives considered:

- C++ + CUDA: very fast, but NVIDIA-only and not suitable for iGPU versus dGPU comparisons across vendors.
- C++ + DirectX 12 compute: strong on Windows, but less portable and slower to build safely.
- Python + CuPy/PyTorch: fast to prototype, but dependency-heavy, vendor-biased, and less precise for a standalone hardware tester.
- C# + DirectCompute/ComputeSharp: good Windows ergonomics, but less portable and more constrained around low-level timing and adapter abstraction.

## Initial Product Scope

The first version should support:

- Matrix multiplication of square matrices at multiple selectable sizes.
- CPU-only matrix multiplication timing.
- GPU matrix multiplication timing.
- Timing report with:
  - CPU time.
  - GPU compute time without transfer time.
  - GPU total time with transfer time.
  - Transfer time.
- GPU selection when multiple compatible adapters are available.
- A separate 1-minute repeated matrix test.
- Cancel/interrupt support for the 1-minute repeated test.
- Basic correctness validation comparing GPU output against CPU output within a floating-point tolerance.

Out of scope for the first version:

- Tensor core or cooperative matrix optimization.
- Vendor-specific CUDA, ROCm, OneAPI, or DirectML paths.
- Non-square matrices.
- Benchmarking graphics rendering performance.
- Persistent historical database.
- Thermal, power, or clock telemetry.

## Benchmark Definitions

### Matrix Operation

Use dense square matrix multiplication:

```text
C = A x B
```

Use `f32` values for the first version.

Default matrix sizes:

```text
128, 256, 512, 1024, 2048
```

Optional advanced size:

```text
4096
```

The UI should warn that very large sizes may take significant memory and time, especially on integrated GPUs.

### CPU Test

First implementation:

- Single-threaded tiled matrix multiplication.
- Deterministic input data.
- Measure using a monotonic high-resolution timer.
- Report wall-clock elapsed time.

Later enhancement:

- Add an optional multi-threaded CPU mode using `rayon`.
- Keep single-threaded and multi-threaded CPU results separate so the comparison remains honest.

### GPU Test

First implementation:

- Upload matrices A and B to GPU buffers.
- Dispatch a WGSL compute shader.
- Write result matrix C to a GPU output buffer.
- Read result C back to CPU.
- Validate a sample or full output against CPU result depending on size.

Timing split:

- GPU compute time without transfer:
  - Measured using GPU timestamp queries around the compute pass.
  - This measures time spent executing the GPU workload.
- GPU total time with transfer:
  - CPU-observed elapsed time from upload start through readback completion.
- Transfer time:
  - `gpu_total_with_transfer - gpu_compute_without_transfer`.
  - Label this as transfer plus queue/readback/synchronization overhead.

Important timing note:

GPU work is asynchronous. CPU wall-clock timing around dispatch alone is not enough. The implementation must explicitly wait for completion before recording total GPU time, and must use timestamp queries for the compute-only number when the selected adapter supports them.

### 1-Minute Repeated Test

This is a separate mode from the single benchmark run.

Behavior:

- User selects matrix size and GPU adapter.
- Test runs repeated CPU and/or GPU matrix operations for 60 seconds.
- UI displays elapsed time, completed iterations, latest timing, average timing, and cancellation state.
- User can cancel mid-test.
- Cancel should stop scheduling new work promptly.
- If one GPU dispatch is already in flight, cancellation may complete after that dispatch/readback finishes.

Default repeated-test mode:

- GPU repeated test first, because it exercises the hardware acceleration path.
- Add CPU repeated mode as a selectable option if it is simple to include cleanly.

Cancellation design:

- Run benchmark work on a background worker thread.
- Use an atomic cancellation flag or async cancellation token.
- UI remains responsive.
- Worker checks cancellation between iterations and before starting expensive CPU work.

## Application Architecture

Proposed crate layout:

```text
src/
  main.rs
  app.rs
  benchmark/
    mod.rs
    cpu.rs
    gpu.rs
    matrix.rs
    results.rs
    repeat.rs
  gpu/
    adapter.rs
    device.rs
    shaders/
      matmul.wgsl
  ui/
    mod.rs
    controls.rs
    results_table.rs
```

### Core Modules

`benchmark::matrix`

- Owns matrix dimensions, allocation, deterministic generation, and tolerance-based comparison.
- Uses row-major layout.

`benchmark::cpu`

- Implements CPU matrix multiplication.
- Starts with single-threaded tiled multiplication.
- Returns timing and checksum/correctness data.

`benchmark::gpu`

- Creates GPU buffers.
- Builds compute pipeline.
- Dispatches matrix multiplication shader.
- Captures compute timestamp queries when available.
- Measures upload, dispatch, readback, and synchronization.

`gpu::adapter`

- Enumerates compatible GPU adapters.
- Captures adapter name, vendor, backend, device type, driver info, and feature support.
- Provides stable labels for the UI, such as:

```text
Intel(R) Iris Xe Graphics - Integrated GPU - DX12
NVIDIA GeForce RTX ... - Discrete GPU - DX12
```

`benchmark::repeat`

- Owns the 60-second loop.
- Tracks cancellation.
- Emits progress updates to the UI.

`benchmark::results`

- Defines serializable result structs.
- Keeps all timing units explicit, preferably milliseconds in UI and nanoseconds internally.

## Data Model

Suggested result types:

```rust
struct BenchmarkConfig {
    matrix_size: usize,
    adapter_id: Option<String>,
    validate_output: bool,
}

struct BenchmarkResult {
    matrix_size: usize,
    cpu_ms: f64,
    gpu_compute_ms: Option<f64>,
    gpu_total_ms: f64,
    transfer_and_sync_ms: Option<f64>,
    adapter_name: String,
    validation: ValidationResult,
}

struct RepeatTestResult {
    matrix_size: usize,
    adapter_name: String,
    duration_ms: f64,
    completed_iterations: u64,
    canceled: bool,
    average_gpu_total_ms: f64,
    average_gpu_compute_ms: Option<f64>,
}
```

Use `Option<f64>` for GPU compute-only timing because timestamp queries may be unavailable on some adapters.

## UI Plan

Main screen sections:

- GPU selector:
  - Dropdown listing all compatible adapters.
  - Show backend and device type.
  - Disable GPU run button if no compatible adapter exists.
- Matrix size selector:
  - Presets plus custom size input.
- Single-run benchmark controls:
  - Run benchmark.
  - Validation toggle.
- Results table:
  - Matrix size.
  - CPU time.
  - GPU compute time.
  - GPU total time.
  - Transfer/sync time.
  - Speedup versus CPU.
  - Validation result.
- 1-minute repeated test controls:
  - Start.
  - Cancel.
  - Progress bar.
  - Iteration count.
  - Running average.

UI states:

- Idle.
- Enumerating GPUs.
- Running single benchmark.
- Running repeated test.
- Cancel requested.
- Completed.
- Failed with readable error.

## GPU Shader Plan

Start with a straightforward tiled WGSL matrix multiplication shader.

Initial goals:

- Correctness first.
- Stable performance across vendors.
- Workgroup tiling to avoid the slowest naive global-memory-only implementation.

Possible first tile size:

```text
16 x 16
```

Follow-up optimization options:

- Tune tile size per adapter.
- Add shader variants for different matrix sizes.
- Add cooperative matrix support only if exposed and stable enough.
- Add half-precision tests later as a separate benchmark category.

## Timing Methodology

For each size:

1. Generate deterministic matrices A and B.
2. Warm up CPU once when size is small enough.
3. Warm up GPU once after pipeline creation.
4. Run measured CPU test.
5. Run measured GPU test.
6. Validate GPU result against CPU result.
7. Record timing breakdown.
8. Display result immediately.

Recommended first-run behavior:

- Run each selected size once.
- Add configurable repetitions later.
- Avoid averaging too early, because the tool should first expose raw timings clearly.

Correctness tolerance:

```text
absolute error <= 0.01 for normal sizes
relative error <= 0.001 for larger accumulated sums
```

The exact tolerance should be adjusted after the first implementation, because floating-point accumulation order differs between CPU and GPU.

## Error Handling

Handle and display:

- No compatible GPU adapter found.
- Selected adapter does not support required storage buffer limits.
- Timestamp queries unavailable.
- Matrix too large for adapter limits.
- Buffer allocation failure.
- Shader compilation failure.
- GPU device lost.
- Validation mismatch.
- Repeated test canceled by user.

When timestamp queries are unavailable:

- Still run the GPU benchmark.
- Report GPU total time with transfer.
- Show GPU compute-only time as unavailable.
- Explain that transfer/sync time cannot be separated reliably for that adapter.

## Implementation Phases

### Phase 1: Project Scaffold

- Create Rust project.
- Add `wgpu`, `pollster`, `bytemuck`, `eframe`, `egui`, and `serde`.
- Add a minimal app window.
- Add a benchmark engine interface independent from the UI.

Deliverable:

- App opens and shows placeholder controls.

### Phase 2: CPU Matrix Benchmark

- Implement deterministic matrix generation.
- Implement single-threaded tiled CPU multiplication.
- Add timing.
- Add result struct.
- Add basic unit tests for small matrix correctness.

Deliverable:

- CPU benchmark runs for selected matrix sizes and displays time.

### Phase 3: GPU Adapter Enumeration

- Enumerate adapters.
- Show name, backend, vendor, device type, and feature support.
- Allow selecting one adapter.

Deliverable:

- UI lists available GPUs and keeps selected adapter state.

### Phase 4: GPU Matrix Compute

- Add WGSL matrix multiplication shader.
- Add GPU buffer creation, upload, dispatch, readback.
- Validate GPU result against CPU output for small and medium sizes.

Deliverable:

- GPU benchmark runs and reports total time.

### Phase 5: GPU Timestamp Timing

- Add timestamp query support detection.
- Measure compute pass time when supported.
- Report:
  - GPU compute time.
  - GPU total time.
  - Transfer/sync time.

Deliverable:

- Results table contains the required timing breakdown.

### Phase 6: 1-Minute Repeated Test

- Add background worker.
- Add 60-second repeated loop.
- Add cancellation flag.
- Update UI progress during the run.
- Return summary on completion or cancellation.

Deliverable:

- User can start and cancel the 1-minute repeated test without freezing the UI.

### Phase 7: Polish and Export

- Add CSV or JSON export.
- Improve error messages.
- Add tooltips for timing definitions.
- Add memory estimate before running large matrices.
- Add app icon and release build settings.

Deliverable:

- Usable first release candidate.

## Testing Plan

Unit tests:

- Matrix generation is deterministic.
- CPU multiplication is correct for known small matrices.
- GPU result comparison tolerance works.
- Timing result calculations are correct.
- Cancellation flag stops repeat loop between iterations.

Integration tests:

- Run CPU and GPU tests at small sizes.
- Validate GPU output for `16`, `32`, and `64`.
- Ensure repeated test can be canceled.

Manual tests:

- System with one GPU.
- System with integrated plus discrete GPU.
- Timestamp-supported adapter.
- Timestamp-unavailable adapter fallback.
- Large matrix memory warning.

## Performance Notes

Memory required for three `f32` square matrices:

```text
bytes = matrix_size * matrix_size * 4 * 3
```

Examples:

```text
1024 x 1024: about 12 MB
2048 x 2048: about 48 MB
4096 x 4096: about 192 MB
```

GPU benchmarking should reuse buffers where possible during repeated tests to avoid measuring allocation overhead repeatedly.

Single-run GPU total time may include first-use overhead if pipeline creation is included. Pipeline creation should happen before measured timing.

## Future Expansion Ideas

- Multi-threaded CPU mode.
- Half-precision `f16` benchmark.
- Batched matrix multiplication.
- Vector addition and convolution benchmarks.
- Exportable benchmark reports.
- Historical comparison view.
- Command-line mode for automated testing.
- Vendor-specific backends as optional plugins:
  - CUDA for NVIDIA.
  - DirectML for Windows.
  - ROCm for AMD where available.
  - OneAPI for Intel where available.
- Temperature and clock telemetry if safe APIs are available.

## Reference Docs

- `wgpu` adapter API: https://docs.rs/wgpu/latest/wgpu/struct.Adapter.html
- `wgpu` adapter metadata: https://docs.rs/wgpu/latest/wgpu/struct.AdapterInfo.html
- `wgpu` timestamp queries: https://wgpu.rs/doc/wgpu/enum.QueryType.html
- `wgpu` timestamp query example notes: https://wgpu.rs/doc/wgpu_examples/timestamp_queries/index.html
- `egui` / `eframe`: https://github.com/emilk/egui
- `rayon` parallel iterators: https://docs.rs/rayon/latest/rayon/iter/trait.ParallelIterator.html
