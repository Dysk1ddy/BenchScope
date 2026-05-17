# AI Training GPU Benchmark Plan

## Goal

Add a new BenchScope tool that benchmarks AI training-style GPU performance on one GPU or multiple GPUs in the same system.

The tool should report:

- Effective FLOPs and TFLOP/s for known training kernels.
- Training throughput such as samples/s, tokens/s, or steps/s.
- Latency for individual GPU dispatches, full training steps, and end-to-end benchmark runs.
- Per-GPU results and multi-GPU scaling efficiency.
- Temperature, utilization, memory pressure, and safety warnings where sensors are available.

The first version should be honest and repeatable: it should benchmark synthetic training workloads that resemble real AI training compute patterns, not claim to reproduce PyTorch, CUDA, cuDNN, NCCL, or a specific production training stack.

## Product Shape

Add a new main-menu tool:

```text
AI Training GPU Benchmark
```

The view should feel like the existing matrix, drive, RAM, and diagnostic tools:

- Dense, practical controls.
- Background worker execution.
- Cancel button while running.
- Progress and phase labels.
- Results table.
- Log/warning area.
- Back button with a running-test cancellation guard.

The benchmark should not replace the existing matrix benchmark. It should reuse its GPU adapter enumeration, timestamp-query strategy, intensity modes, sensor summaries, cancellation model, and result-table patterns where practical.

## Why This Is Different From The Current Matrix Benchmark

The existing matrix benchmark measures one core operation: matrix multiplication.

AI training performance needs a broader workload shape:

- Repeated forward/backward/update steps.
- Many GEMMs with different dimensions.
- Activation, normalization, softmax, and reduction kernels.
- Persistent tensors and optimizer state.
- More frequent kernel dispatches.
- Batch size and sequence length effects.
- Optional multi-GPU scaling.

The first milestone should still lean heavily on GEMM because dense matrix multiplication dominates many training workloads. Later milestones can add more model-shaped kernels.

## Technical Constraints

BenchScope is currently a Rust + `egui` + `wgpu` app.

`wgpu` is a good fit for a portable benchmark because it can run across DX12, Vulkan, Metal, OpenGL, and compatible adapters. However, it is not the same as vendor AI training stacks.

Important constraints:

- `wgpu` does not expose CUDA tensor cores, ROCm/MIOpen, cuDNN, NCCL, or framework-specific graph compilers directly.
- Multi-GPU peer-to-peer transfers and all-reduce collectives are not portable through `wgpu`.
- Adapter-local memory limits and storage-buffer binding limits can be smaller than the physical VRAM size.
- Timestamp query support varies by backend and adapter.
- Shader `f16` support must be detected before enabling half-precision workloads.
- Windows Timeout Detection and Recovery risk still applies to long compute dispatches.

Therefore, the initial benchmark should be called a portable AI training-style benchmark. A later native-provider layer can add CUDA, ROCm, oneAPI, or DirectML specific paths if the project wants closer framework parity.

## Implementation Phases

### Phase 1: Portable Single-GPU Training Core

Build a new single-GPU benchmark runner using `wgpu`.

Deliverables:

- New feature module: `src/features/ai_training_benchmark/`.
- Main-menu entry and view navigation.
- Adapter selection using existing GPU detection patterns.
- Workload presets for synthetic training loops.
- GPU timestamp query support where available.
- CPU-observed fallback timings where timestamp queries are unavailable.
- FLOP accounting for every implemented compute kernel.
- Step latency and throughput reporting.
- Cancellation between dispatches and benchmark steps.
- Safe default workload sizes.

This phase should focus on correctness of measurement plumbing before adding complex model kernels.

### Phase 2: Richer Training Workloads

Add model-shaped presets that approximate common training patterns.

Deliverables:

- MLP training step.
- Transformer block training step.
- Optional CNN-like convolution benchmark, if a practical direct convolution or im2col path is added.
- Mixed precision options where supported.
- Memory footprint estimates before running.
- Warmup and measurement controls.
- CSV or Markdown export.

The transformer workload can start with the most important dense operations, then add layer norm, softmax, and optimizer kernels as separate measured components.

### Phase 3: Multi-GPU Benchmarking

Add multi-adapter execution.

Deliverables:

- Multi-select GPU list.
- One worker per selected adapter.
- Independent per-GPU training loops.
- Aggregate throughput and latency summaries.
- Scaling efficiency calculation.
- Optional CPU-mediated gradient all-reduce simulation.

The first multi-GPU version should benchmark independent data-parallel replicas. This answers "how much total training-style compute can this system sustain across all GPUs?" without claiming to measure true distributed training communication.

### Phase 4: Native AI Backend Providers

Add optional backend-specific providers for closer real-world training parity.

Potential providers:

- CUDA provider for NVIDIA GPUs.
- ROCm or HIP provider for AMD GPUs on supported systems.
- oneAPI/SYCL provider for Intel GPUs.
- DirectML provider for Windows systems.

This can be a later plugin-like layer behind a common benchmark interface. The portable `wgpu` implementation remains the fallback.

## New Module Layout

Recommended module structure:

```text
src/features/ai_training_benchmark/
  mod.rs
  model.rs
  ui.rs
  runner.rs
  gpu.rs
  workloads.rs
  shaders.rs
  sizing.rs
  multi_gpu.rs
  metrics.rs
```

Responsibilities:

- `model.rs`: UI state, benchmark settings, workload enums, result structs.
- `ui.rs`: `egui` controls, result tables, warnings, progress display.
- `runner.rs`: background worker orchestration, cancellation, phase updates.
- `gpu.rs`: adapter/device/queue setup, feature detection, timestamp helpers.
- `workloads.rs`: workload definitions and FLOP formulas.
- `shaders.rs`: WGSL shader sources and pipeline creation.
- `sizing.rs`: memory estimates, preset scaling, safety caps.
- `multi_gpu.rs`: multi-adapter worker spawning and aggregation.
- `metrics.rs`: latency percentiles, throughput, TFLOP/s, scaling efficiency.

Keep the initial implementation separate from `matrix_benchmark` to avoid making that tool harder to reason about. Reuse shared helper patterns later only after the AI benchmark stabilizes.

## Workload Presets

### Preset 1: Linear Layer Training

This is the best first workload because it maps cleanly to GEMM operations and has exact FLOP accounting.

Training step:

```text
Y = X * W
dW = X^T * dY
dX = dY * W^T
W = W - lr * dW
```

Inputs:

- Batch size `B`.
- Input dimension `I`.
- Output dimension `O`.
- Precision mode.
- Step count.

FLOP accounting:

```text
forward FLOPs      = 2 * B * I * O
weight-grad FLOPs  = 2 * I * B * O
input-grad FLOPs   = 2 * B * O * I
optimizer FLOPs    = O(I * O)
total approximate  = 6 * B * I * O + optimizer work
```

Metrics:

- Effective TFLOP/s.
- Samples/s.
- Step latency.
- Forward/backward/update timing split.

Recommended presets:

```text
Small:  B=256,  I=1024, O=1024
Medium: B=512,  I=2048, O=2048
Large:  B=1024, I=4096, O=4096
```

### Preset 2: MLP Training

Use multiple linear layers with activation and backward passes.

Example shape:

```text
X -> Linear -> GELU/ReLU -> Linear -> Loss gradient
```

Training step:

- Forward GEMM for layer 1.
- Activation kernel.
- Forward GEMM for layer 2.
- Loss-gradient seed kernel.
- Backward GEMM for layer 2.
- Activation backward kernel.
- Backward GEMM for layer 1.
- Optimizer update kernels.

FLOP accounting should count GEMM exactly and activation approximately. The result table should label exact and approximate components clearly.

Metrics:

- Samples/s.
- End-to-end step latency.
- Compute-only TFLOP/s.
- Dispatch count per step.
- Activation/update overhead.

### Preset 3: Transformer Block Training

Add a transformer-like workload for LLM-style training behavior.

Config:

- Batch size `B`.
- Sequence length `S`.
- Hidden size `H`.
- Attention heads `A`.
- Head dimension `D = H / A`.
- MLP expansion ratio, default `4`.

Forward pass components:

- Q/K/V projections.
- Attention score GEMM: `Q * K^T`.
- Softmax.
- Attention value GEMM: `P * V`.
- Output projection.
- MLP up projection.
- Activation.
- MLP down projection.
- Layer norm kernels.

Backward pass components:

- Start with GEMM-heavy backward approximations.
- Add exact hand-written backward kernels incrementally.
- Keep the UI label clear if the first transformer version is "GEMM-dominant training proxy" rather than full mathematical transformer training.

Useful throughput metrics:

- Tokens/s: `B * S / step_seconds`.
- Sequences/s: `B / step_seconds`.
- Effective TFLOP/s.
- Full-step latency.
- Attention latency and MLP latency.

Recommended presets:

```text
Tiny:  B=4,  S=128, H=512,  A=8
Small: B=4,  S=256, H=768,  A=12
Base:  B=2,  S=512, H=1024, A=16
Large: B=1,  S=1024, H=2048, A=16
```

These should be adjusted at runtime based on detected adapter limits.

### Preset 4: Optimizer Stress

Measure optimizer update throughput over large parameter arrays.

Initial optimizers:

- SGD.
- AdamW.

AdamW update touches parameter, gradient, first moment, and second moment buffers. It is more memory-bandwidth heavy than pure GEMM, which makes it useful as a second performance axis.

Metrics:

- Parameters updated per second.
- Effective GB/s based on bytes read/written.
- Update latency.
- Dispatch count.

## Precision Modes

Start with:

- `f32`: always available.
- `f16`: only when `wgpu::Features::SHADER_F16` is supported.

Later:

- `bf16`: only if a native provider or shader path supports it.
- Mixed precision: store activations/weights in `f16`, accumulate in `f32` where feasible.

UI behavior:

- Disable unsupported precision modes.
- Show a short note explaining why a mode is unavailable.
- Include precision in every result row.

Important measurement rule:

If the portable `wgpu` shader does not use hardware tensor cores or equivalent matrix acceleration, do not label the result as tensor-core TFLOP/s. Label it as portable shader TFLOP/s.

## Metrics

### FLOPs

For every workload, compute:

- FLOPs per step.
- Total measured FLOPs.
- Compute-only TFLOP/s.
- End-to-end TFLOP/s.

Definitions:

```text
compute_tflops = total_flops / gpu_compute_seconds / 1e12
end_to_end_tflops = total_flops / wall_seconds / 1e12
```

Only count operations that the implemented kernels actually perform. If a workload uses estimated FLOPs for activation, softmax, or normalization, mark those rows as estimated.

### Throughput

Report workload-specific throughput:

- Linear/MLP: samples/s.
- Transformer: tokens/s and sequences/s.
- Optimizer: parameters/s and GB/s.
- Generic: steps/s.

Definitions:

```text
samples_per_second = measured_samples / wall_seconds
tokens_per_second = measured_tokens / wall_seconds
steps_per_second = measured_steps / wall_seconds
```

For multi-GPU:

```text
aggregate_throughput = sum(per_gpu_throughput)
scaling_efficiency = aggregate_throughput / (best_single_gpu_throughput * gpu_count)
```

If the selected GPUs are not identical, also show normalized scaling against the measured single-GPU baseline for each selected adapter when available.

### Latency

Track:

- Full run duration.
- Warmup duration.
- Average step latency.
- p50 step latency.
- p95 step latency.
- p99 step latency when enough samples exist.
- Minimum and maximum step latency.
- Average dispatch latency.
- Maximum dispatch latency.
- CPU submit/wait latency where measurable.

Use a bounded vector or streaming percentile estimator for step latencies. Step counts are small enough at first that a simple vector and sort is acceptable.

### Memory

Track:

- Estimated tensor memory.
- Allocated GPU buffer bytes.
- Peak planned working set.
- Per-GPU memory estimate.
- Whether the preset was reduced to fit adapter limits.

Memory should be reported in MiB/GiB.

### Telemetry

Reuse existing sensor summary style:

- GPU temperature start/end/max.
- GPU utilization start/end/max if available.
- CPU temperature summary if available.
- Warnings for missing telemetry.

Sensor absence should never block a benchmark.

## Benchmark Run Lifecycle

Recommended lifecycle:

```text
1. Validate settings.
2. Estimate memory and adapter limits.
3. Compile or reuse pipelines.
4. Allocate persistent tensors.
5. Initialize deterministic input data.
6. Warm up for N steps or T seconds.
7. Measure for N steps or T seconds.
8. Collect timestamps and CPU timings.
9. Summarize metrics.
10. Release GPU resources.
```

Default run policy:

- Warmup: 3 to 5 steps.
- Measurement: 10 to 30 steps, or 10 seconds, whichever comes first.
- Hard cap: 60 seconds per selected workload unless the user chooses Thorough mode.
- Cancel check: before and after each step, and between large dispatch groups.

Profiles:

```text
Quick:    short warmup, 5 measured steps, small presets
Balanced: default warmup, 10-30 measured steps
Thorough: longer warmup, 30-100 measured steps, more stable percentiles
```

## UI Plan

### Controls

Top controls:

- GPU selection:
  - Single GPU mode.
  - Multi-GPU mode with checkboxes.
- Workload:
  - Linear Layer Training.
  - MLP Training.
  - Transformer Training Proxy.
  - Optimizer Stress.
- Preset:
  - Tiny.
  - Small.
  - Medium.
  - Large.
  - Custom.
- Precision:
  - f32.
  - f16 when supported.
- Profile:
  - Quick.
  - Balanced.
  - Thorough.
- Intensity:
  - Safe.
  - Balanced.
  - High.
- Run.
- Cancel.

Custom controls:

- Batch size.
- Sequence length for transformer.
- Hidden size.
- Layer count, default one block for early versions.
- Step count or time limit.
- Warmup steps.

### Status Area

Show:

- Current phase.
- Current step.
- Progress bar.
- Estimated memory.
- Active adapter name.
- Selected precision.
- Warnings.

### Results Table

Columns:

- Time.
- Workload.
- Preset.
- GPU.
- Precision.
- GPUs used.
- FLOPs/step.
- Compute TFLOP/s.
- End-to-end TFLOP/s.
- Throughput.
- Avg latency.
- p95 latency.
- Dispatch avg/max.
- Memory.
- Temperature max.
- Notes.

For multi-GPU summary rows:

- Aggregate throughput.
- Aggregate end-to-end TFLOP/s.
- Slowest GPU.
- Fastest GPU.
- Scaling efficiency.
- Synchronization mode.

### Detail Panel

When a result is selected, show:

- Per-phase timing split.
- Per-GPU rows.
- FLOP accounting details.
- Memory accounting details.
- Feature flags used.
- Timestamp mode.
- Any fallback or warning.

## Data Structures

Suggested core enums:

```rust
enum AiTrainingWorkload {
    LinearLayer,
    Mlp,
    TransformerBlock,
    OptimizerStress,
}

enum AiPrecision {
    F32,
    F16,
}

enum AiBenchmarkProfile {
    Quick,
    Balanced,
    Thorough,
}

enum AiMultiGpuMode {
    Single,
    IndependentReplicas,
    CpuAllReduceSimulation,
}
```

Suggested settings struct:

```rust
struct AiTrainingSettings {
    workload: AiTrainingWorkload,
    profile: AiBenchmarkProfile,
    precision: AiPrecision,
    gpu_indices: Vec<usize>,
    multi_gpu_mode: AiMultiGpuMode,
    batch_size: usize,
    sequence_len: usize,
    hidden_size: usize,
    input_dim: usize,
    output_dim: usize,
    measured_steps: usize,
    warmup_steps: usize,
    time_limit_s: f64,
    intensity: GpuIntensity,
}
```

Suggested result struct:

```rust
struct AiTrainingResult {
    workload: AiTrainingWorkload,
    preset_name: String,
    precision: AiPrecision,
    gpu_names: Vec<String>,
    gpu_count: usize,
    flops_per_step: f64,
    measured_steps: usize,
    total_flops: f64,
    gpu_compute_ms: Option<f64>,
    wall_ms: f64,
    compute_tflops: Option<f64>,
    end_to_end_tflops: f64,
    throughput_label: String,
    throughput_value: f64,
    avg_step_ms: f64,
    p50_step_ms: f64,
    p95_step_ms: f64,
    p99_step_ms: Option<f64>,
    avg_dispatch_ms: Option<f64>,
    max_dispatch_ms: Option<f64>,
    memory_bytes: u64,
    scaling_efficiency: Option<f64>,
    notes: String,
}
```

Keep result structs UI-friendly, even if the runner uses lower-level internal structs.

## Shader Plan

### GEMM Kernel

Start with a tiled matrix multiplication shader similar in spirit to the existing matrix benchmark.

Needed variants:

- `C = A * B`.
- `C = A^T * B` for weight gradients.
- `C = A * B^T` for input gradients.
- Optional bias add.

The first implementation can use separate kernels for transpose cases. Later, add packed layouts or pre-transposed buffers if that is faster and easier to bind.

### Activation Kernels

Initial activation:

- ReLU forward.
- ReLU backward.

Later:

- GELU approximate forward/backward.

### Optimizer Kernels

Initial:

- SGD update.

Later:

- AdamW update:
  - update first moment.
  - update second moment.
  - bias correction if included.
  - weight decay.
  - parameter update.

### Transformer Kernels

Initial transformer proxy:

- Projection GEMMs.
- Attention score GEMM.
- Softmax forward.
- Attention value GEMM.
- Output projection GEMM.
- MLP GEMMs.

Later:

- Layer norm forward/backward.
- Full backward pass for attention and MLP.
- Fused kernels where useful.

## Multi-GPU Strategy

### Version 1: Independent Replica Mode

Run the same workload independently on each selected GPU.

This measures:

- Aggregate system training-style compute.
- Per-GPU sustained throughput.
- Thermal and power interaction between GPUs.
- Slowest-device behavior.

It does not measure:

- Gradient synchronization.
- Peer-to-peer bandwidth.
- NCCL-style collective efficiency.
- Framework scheduling overhead.

UI label:

```text
Multi-GPU: Independent replicas
```

Implementation:

- Spawn one benchmark worker per selected adapter.
- Each worker creates its own `wgpu::Device` and `wgpu::Queue`.
- Use a shared cancellation token.
- Collect per-GPU progress updates through channels.
- Aggregate results after all workers finish or cancellation occurs.

### Version 2: CPU All-Reduce Simulation

Add an optional synchronization step:

```text
for each training step:
  each GPU computes local gradients
  read gradient summary or gradient buffer to CPU
  CPU combines gradients
  upload combined gradient/update signal
  continue
```

This is intentionally a simulation and should be labeled that way. It can expose the cost of CPU-mediated synchronization but should not be presented as true high-performance multi-GPU training.

Metrics:

- Compute time.
- Readback time.
- CPU reduction time.
- Upload/broadcast time.
- Synchronization overhead percent.

### Version 3: Native Collective Providers

If native providers are added later, expose true multi-GPU modes:

- CUDA + NCCL for NVIDIA.
- ROCm + RCCL for AMD.
- oneAPI collectives for Intel where available.

These should live behind provider interfaces and should not complicate the portable `wgpu` path.

## Safety And Stability

Reuse GPU intensity ideas from the matrix benchmark.

Safe mode:

- Smaller tiles.
- Shorter dispatches.
- Lower queue depth.
- More frequent cancellation checks.
- Conservative memory caps.

Balanced mode:

- Larger tiles.
- Fewer pauses.
- Still backs off if dispatch latency spikes.

High mode:

- Largest tiles and longest runs.
- Clear warning for unstable systems, overclocks, high thermals, or laptops on battery.

Hard safety rules:

- Estimate memory before allocating.
- Refuse obviously impossible allocations unless the user explicitly overrides.
- Do not submit a single huge dispatch when a tiled loop can keep dispatches shorter.
- Cancel only between dispatches and steps.
- Mark incomplete/canceled runs clearly.
- Show warnings when timestamp queries are unavailable.

## Result Accuracy Rules

The benchmark should prioritize clear labels over inflated numbers.

Rules:

- Report both compute-only and end-to-end metrics.
- Include warmup steps separately from measured steps.
- Count FLOPs only for implemented kernels.
- Mark approximate FLOP components.
- Include precision mode.
- Include backend and adapter name.
- Include whether timestamp queries or CPU-observed timings were used.
- Include memory footprint and workload shape.
- Include dispatch count and dispatch latency summary.
- For multi-GPU, distinguish independent replica scaling from synchronized training.

This protects the benchmark from becoming a misleading "big number generator."

## Validation Strategy

### Kernel Correctness

For small tensor sizes:

- Run CPU reference implementations.
- Compare GPU outputs within tolerance.
- Test each GEMM variant.
- Test activation forward/backward.
- Test optimizer updates.

Use deterministic input generation so results are stable.

Tolerance:

- `f32`: tight tolerance for small sizes.
- `f16`: looser tolerance based on accumulated error.

### Benchmark Sanity

Add smoke tests:

- Tiny linear training step completes.
- Cancellation exits cleanly.
- Unsupported `f16` disables the option.
- FLOP calculations match expected formulas.
- Percentile calculations are correct.
- Multi-GPU aggregator handles one, two, and failed-worker cases.

### Manual Verification

Manual checks:

- Run Quick profile on the default adapter.
- Run f16 only on a supported adapter.
- Run with timestamp queries unavailable or forcibly disabled.
- Run multi-GPU if more than one real adapter is present.
- Confirm Back/Cancel behavior.
- Confirm no UI freeze during long runs.

## Export Plan

Add export after the main runner is stable.

Formats:

- Markdown report.
- CSV result table.

Markdown report should include:

- System summary.
- GPU adapter summary.
- Workload settings.
- Results table.
- Per-GPU details.
- Warnings and notes.
- Sensor summary.

CSV should include one row per result plus enough columns to compare across runs.

## Integration Steps

1. Add module folder and `mod.rs`.
2. Add feature state to the app model.
3. Add main-menu button.
4. Add empty UI view with settings controls.
5. Add adapter and feature detection.
6. Add `AiTrainingSettings`, workload presets, and memory estimates.
7. Add single-GPU linear-layer runner.
8. Add WGSL GEMM variants needed for forward/backward.
9. Add timestamp and CPU fallback timing.
10. Add results table and detail panel.
11. Add cancellation and progress updates.
12. Add tests for FLOP formulas and metric calculations.
13. Add MLP workload.
14. Add optimizer stress workload.
15. Add transformer training proxy.
16. Add multi-GPU independent replica mode.
17. Add CPU all-reduce simulation mode if desired.
18. Add export.
19. Document limitations in `README.md`.

## Suggested First Pull Request Scope

Keep the first PR narrow:

- Main-menu entry.
- AI Training GPU Benchmark UI skeleton.
- Single-GPU Linear Layer Training workload.
- f32 precision only.
- Quick/Balanced profiles.
- FLOP/s, samples/s, avg latency, p95 latency.
- Timestamp query support if easy to reuse from matrix benchmark.
- CPU timing fallback.
- Cancellation.
- FLOP formula tests.

Avoid in the first PR:

- Full transformer training.
- f16.
- Multi-GPU.
- Native CUDA/ROCm providers.
- Export.

This creates a usable foundation without forcing all AI-training complexity into the first implementation.

## Suggested Second Pull Request Scope

- Add f16 feature detection and f16 shader path.
- Add MLP workload.
- Add optimizer stress workload.
- Add richer result detail panel.
- Add Markdown/CSV export.

## Suggested Third Pull Request Scope

- Add transformer training proxy.
- Add phase-level timing split.
- Add memory scaling controls.
- Add more robust preset auto-sizing.

## Suggested Fourth Pull Request Scope

- Add multi-GPU independent replica mode.
- Add aggregate throughput and scaling efficiency.
- Add per-GPU progress and result rows.
- Add CPU all-reduce simulation only if the UX can label it clearly.

## Future Native Provider Interface

If the project later adds native AI backends, define a provider trait like:

```rust
trait AiTrainingBackend {
    fn name(&self) -> &'static str;
    fn enumerate_devices(&self) -> Vec<AiTrainingDevice>;
    fn supported_precisions(&self, device_id: &str) -> Vec<AiPrecision>;
    fn run_benchmark(
        &self,
        settings: &AiTrainingSettings,
        cancel: &CancellationToken,
        progress: ProgressSender,
    ) -> anyhow::Result<AiTrainingResult>;
}
```

Potential backends:

```text
PortableWgpuBackend
CudaBackend
RocmBackend
DirectMlBackend
OneApiBackend
```

The UI should show the selected provider so users understand whether they are seeing portable shader performance or native AI-stack performance.

## Open Questions

- Should the first user-facing name be `AI Training GPU Benchmark` or `Training Workload Benchmark`?
- Should f16 be postponed until after the f32 linear runner is stable?
- Should transformer results be labeled `Transformer proxy` until full backward kernels exist?
- Should multi-GPU synchronized mode wait for native providers instead of adding CPU all-reduce simulation?
- Should BenchScope store historical AI benchmark results for comparisons over time?

## Success Criteria

The feature is successful when:

- A user can select a GPU, run a training-style benchmark, and get repeatable FLOP/s, throughput, and latency metrics.
- The UI clearly explains the workload, precision, backend, and limitations through labels and notes.
- Results do not freeze the app and can be canceled.
- The benchmark works on systems without vendor AI SDKs.
- Multi-GPU mode reports per-GPU and aggregate results without pretending to be NCCL-style distributed training.
- The implementation can grow toward native AI backends without rewriting the UI.
