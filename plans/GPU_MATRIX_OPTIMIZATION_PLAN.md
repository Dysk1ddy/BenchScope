# GPU Matrix Optimization Plan

## Implementation Status

Core implementation has begun in the Rust app:

- Added GPU execution path telemetry: Direct, Panelized, and Streaming.
- Added dispatch count, tile/panel shape, last/average/max dispatch timing, and backoff count to benchmark results.
- Added an adaptive safety backoff that reduces row work after slow dispatches.
- Added a persistent panelized path for large matrices that exceed storage-buffer binding limits but still fit adapter buffer limits.
- Kept the streaming blocked path as the compatibility fallback.
- Added packing/unpacking tests for panelized column layouts.

Advanced shader variants and vendor-specific acceleration remain future optimization work.

## Objective

Optimize matrix multiplication so the tool can drive the GPU close to full compute utilization while reducing the chance of Windows driver timeouts, failed driver recovery, or full system instability.

The target is not simply "100% utilization at any cost." The target is:

- High sustained GPU compute occupancy.
- Short enough individual GPU submissions that Windows sees the GPU as responsive.
- Accurate timing breakdowns for compute, transfer, setup, and synchronization.
- Fast cancellation between dispatches.
- Safe defaults, with higher-risk modes clearly labeled.
- Correct full-matrix results, not approximate block-only work.

## Key Diagnosis

The crash risk comes mostly from long-running GPU compute dispatches, not from high utilization alone.

Rendering stress tests such as FurMark keep the GPU busy by submitting many frame-sized workloads. Each frame finishes quickly enough that the driver, scheduler, and Windows Timeout Detection and Recovery system can still observe forward progress.

Large matrix compute can be more dangerous if the app submits a single huge dispatch:

```text
one huge GPU dispatch -> GPU occupied for too long -> Windows may detect a hang -> driver reset / reboot
```

Cancellation has the same limitation. The app cannot stop a shader dispatch while it is already running on the GPU. It can only cancel before submitting work, while waiting for completion, or between dispatches. Therefore, safe cancellation requires every dispatch to be short.

## Design Principle

Use many short, high-arithmetic GPU dispatches instead of one massive dispatch.

The full matrix is still computed. The output matrix is divided into tiles or panels, and each dispatch computes one region of the final result.

For matrix multiplication:

```text
C[row, col] = sum over k of A[row, k] * B[k, col]
```

Each output tile still loops across the full `k` dimension. Blocking changes scheduling and memory layout; it does not reduce the mathematical work.

## Target GPU Paths

### Path 1: Direct Full-Buffer Tiled Path

Use this when the full matrix buffers fit both VRAM and the adapter's storage-buffer binding limits.

Pattern:

```text
upload A once
upload B once
allocate C once
compute C in short row/column dispatches
read C once
```

Advantages:

- Minimal transfer overhead.
- Clean timing model.
- High GPU occupancy when tiles are sized well.
- Best candidate for fast benchmark mode.

Limitations:

- Some adapters expose storage-buffer binding limits smaller than a full large matrix.
- For example, an `8192x8192` `f32` matrix is 256 MiB. If `max_storage_buffer_binding_size` is 128 MiB, the full matrix cannot be bound as one storage buffer range.

### Path 2: Persistent Panelized Path

Use this when full buffers fit in VRAM but cannot be bound as full storage ranges.

The goal is to avoid repeated CPU-to-GPU uploads and repeated per-block readbacks while still obeying binding-size limits.

Pattern:

```text
generate A on CPU
generate B in packed column-panel layout on CPU
upload A once
upload packed B once
allocate packed C once

for each column panel:
  for each row block:
    bind A row range
    bind B panel range
    bind C panel output range
    dispatch short compute work

read packed C once
validate sampled output against packed layout
```

Why packed B is needed:

- In normal row-major layout, a column slice of B is not one contiguous range.
- A packed column-panel layout stores `B[k, col..col+panel_width]` contiguously.
- This allows the shader to bind only the needed B panel without exceeding binding limits.

Why packed C may be needed:

- A rectangular sub-block of row-major C is not contiguous unless it spans the full row width.
- A packed C panel lets each output panel be stored contiguously and read back once at the end.

Advantages:

- Avoids repeated B block uploads.
- Avoids repeated C block readbacks.
- Can keep utilization high on large matrices.
- Works around `max_storage_buffer_binding_size`.

Tradeoffs:

- Requires a second layout for B and C.
- Validation must understand packed output layout.
- CPU setup may spend time packing B.
- More complex than the current streaming blocked path.

### Path 3: Streaming Blocked Fallback

Use this only when persistent buffers are too large for VRAM or adapter limits.

Pattern:

```text
pack/upload A block
pack/upload B block
compute C block
read C block
assemble CPU result
repeat
```

Advantages:

- Most compatible.
- Works when full persistent buffers are too large.
- Keeps memory use bounded.

Tradeoffs:

- Highest transfer and synchronization overhead.
- Lower maximum utilization.
- More CPU involvement.
- Still needs short dispatch limits for safety.

## GPU Intensity Modes

Keep user-facing intensity modes, but implement them through a runtime governor rather than fixed block sizes.

### Safe

Purpose:

- Default mode.
- Minimize TDR risk.
- Make cancellation feel responsive.

Targets:

```text
target dispatch time: 25-100 ms
soft maximum dispatch time: 250 ms
hard backoff threshold: 500 ms
queue depth: 1-2 dispatches
```

Behavior:

- Start with small tiles.
- Increase tile size only after several stable dispatches.
- Immediately reduce tile size if dispatch time spikes.
- Insert short CPU-side yields between batches when necessary.

### Balanced

Purpose:

- Higher performance while still avoiding known dangerous dispatch sizes.

Targets:

```text
target dispatch time: 100-250 ms
soft maximum dispatch time: 500 ms
hard backoff threshold: 750 ms
queue depth: 2-4 dispatches
```

Behavior:

- Larger tiles than Safe.
- Fewer pauses.
- Still backs off automatically when dispatch latency rises.

### High

Purpose:

- Maximize throughput on stable systems.
- User-confirmed for large matrices.

Targets:

```text
target dispatch time: 250-500 ms
soft maximum dispatch time: 750 ms
hard backoff threshold: 1000 ms
queue depth: 4-8 dispatches
```

Behavior:

- Largest adaptive tiles.
- Minimal pauses.
- Automatic backoff remains mandatory.
- No single dispatch should intentionally approach the Windows TDR danger zone.

### Max / Experimental

Do not make this the default.

Purpose:

- Optional expert mode for users who explicitly want maximum stress behavior.

Requirements:

- Separate confirmation dialog.
- Clear warning that unstable hardware, drivers, power delivery, or thermals may still crash the system.
- Show live last/average/max dispatch time.
- Show a visible "Back off automatically" toggle that defaults on.

## Adaptive GPU Governor

The governor decides tile sizes and queue depth based on actual measured dispatch times.

### Inputs

- Matrix size.
- Adapter type.
- Adapter limits:
  - `max_storage_buffer_binding_size`
  - `max_buffer_size`
  - timestamp query support
  - reported VRAM/shared memory
- Selected intensity mode.
- Last dispatch compute time.
- Moving average dispatch compute time.
- Max dispatch compute time.
- Cancellation state.

### Outputs

- Row block size.
- Column panel size.
- Dispatch batch size.
- Pause/yield duration.
- Backoff events.

### Basic Algorithm

Start conservative:

```text
row_block = safe initial row block
col_panel = safe initial column panel
queue_depth = selected mode initial queue depth
```

After each measured batch:

```text
if max_dispatch_ms > hard_backoff_threshold:
    reduce row_block and/or col_panel aggressively
    reduce queue_depth
    log backoff event

else if average_dispatch_ms > soft_maximum:
    reduce tile size one step

else if average_dispatch_ms < target_low for several stable batches:
    increase tile size one step

else:
    keep current tile size
```

Tile changes should be aligned to workgroup tile sizes:

```text
row_block multiple of 16
col_panel multiple of 16
```

### Dispatch Time Measurement

Use GPU timestamp queries when available.

Record:

- Last dispatch compute ms.
- Average dispatch compute ms.
- Max dispatch compute ms.
- Total GPU compute ms.
- Dispatch count.

If timestamp queries are unavailable:

- Use CPU-observed submit-to-complete time.
- Mark the timing as CPU-observed.
- Use more conservative thresholds because CPU-observed time includes scheduling and synchronization.

## Queueing Strategy

To keep utilization high, avoid a strict "submit one tiny dispatch, wait, submit next" loop when possible.

Preferred pattern:

```text
create command encoder
record several short dispatches
submit batch
poll completion
update telemetry
check cancellation
submit next batch
```

Safety rule:

- The total batch should still complete quickly enough for responsive cancellation.
- Safe mode should prefer smaller batches.
- High mode can use deeper batches, but must still cap dispatch and batch time.

Timing rule:

- Use timestamp queries per dispatch or per batch.
- If per-dispatch query count becomes too large, measure per batch and periodically sample individual dispatches.

## Shader Optimization Plan

The current WGSL tiled matmul shader is functional, but not benchmark-class. To approach real benchmark utilization, the shader needs more arithmetic per memory transaction and better occupancy.

### Phase 1 Shader Improvements

- Add row and column offsets to the direct shader.
- Add support for panelized B and panelized C layouts.
- Keep workgroup tile size aligned to 16x16 initially.
- Ensure each workgroup uses shared memory for A and B tiles.
- Avoid branches inside the hot loop where possible.

### Phase 2 Shader Tuning

Test variants:

- `16x16` workgroup, one output per thread.
- `16x16` workgroup, two outputs per thread.
- `8x16` or `16x8` workgroup variants.
- Wider column panels to improve B reuse.
- Larger row blocks only when dispatch time remains safe.

Measure:

- GPU compute ms.
- Dispatch ms.
- Throughput in GFLOP/s or TFLOP/s.
- Validation error.
- Stability across repeated runs.

### Phase 3 Advanced Paths

Possible future options:

- Vendor-specific CUDA path for NVIDIA.
- DirectML or cooperative matrix path.
- `f16` mode.
- Tensor-core-like acceleration where supported.

These should be separate benchmark modes because they change precision, portability, or vendor coverage.

## Memory Strategy

### VRAM Estimate

For `f32` square matrices:

```text
one matrix bytes = N * N * 4
basic A/B/C bytes = 3 * N * N * 4
with readback/staging = roughly 4 * N * N * 4
with packed layouts = A + packed B + packed C + staging + overhead
```

Examples:

```text
4096x4096 one matrix = 64 MiB
4096x4096 A/B/C = 192 MiB
4096x4096 with staging estimate = 256 MiB

8192x8192 one matrix = 256 MiB
8192x8192 A/B/C = 768 MiB
8192x8192 with staging estimate = 1 GiB

16384x16384 one matrix = 1 GiB
16384x16384 A/B/C = 3 GiB
16384x16384 with staging estimate = 4 GiB
```

### VRAM Warnings

Warn at multiple levels:

```text
>= 70% reported VRAM: caution
>= 85% reported VRAM: strong warning
>= 100% reported VRAM: block until user confirms
```

Also warn that reported VRAM is not a perfect limit because:

- Drivers reserve memory.
- Other apps use VRAM.
- Windows may page or migrate resources.
- Integrated GPUs share system RAM.

### Allocation Rules

- Prefer persistent full/panel buffers when memory allows.
- Reuse buffers across repeat tests.
- Avoid creating and destroying large buffers inside every block.
- Avoid readback until the end unless the user requests diagnostic mode.

## Timing Model

Report separate timing fields so users can tell compute performance from transfer overhead.

Required columns:

- CPU ms.
- GPU compute ms.
- GPU total ms.
- Transfer/sync ms.
- Speedup.

Add optimization-specific columns:

- GPU path:
  - Direct full-buffer.
  - Persistent panelized.
  - Streaming blocked fallback.
- Intensity mode.
- Dispatch count.
- Tile/panel size.
- Last dispatch ms.
- Average dispatch ms.
- Max dispatch ms.
- Governor backoff count.
- Upload/setup ms.
- Readback ms.
- Validation ms.

Derived values:

```text
effective TFLOP/s = (2 * N^3) / GPU compute seconds / 1e12
total TFLOP/s = (2 * N^3) / GPU total seconds / 1e12
```

## Stress Mode

Benchmark mode and stress mode should be separate.

Benchmark mode:

- Computes a requested matrix.
- Reads back output.
- Validates output.
- Reports accurate timings.

Stress mode:

- Repeats short GPU workloads continuously.
- Skips full readback by default.
- Validates occasionally or samples a small output region.
- Displays live dispatch timing and backoff events.
- Is designed to hold high utilization safely.

Stress mode loop:

```text
allocate persistent GPU buffers
warm up
ramp intensity over several seconds

while duration remains and not canceled:
    submit batch of short dispatches
    collect timing
    adapt tile size and queue depth
    occasionally validate sampled output
    update UI
```

Ramp schedule:

```text
0-5 seconds: low tile size
5-10 seconds: medium tile size
10+ seconds: target intensity
```

If dispatch time spikes:

```text
reduce tile size
reduce queue depth
show warning in log
continue at lower intensity
```

## Cancellation Plan

Cancellation cannot interrupt a currently executing GPU dispatch. Therefore, cancellation responsiveness is controlled by maximum dispatch and batch duration.

Acceptance targets:

```text
Safe mode cancellation: usually below 1 second
Balanced cancellation: usually below 2 seconds
High cancellation: usually below 3 seconds
```

Implementation:

- Check cancellation before every batch.
- Check cancellation after every batch.
- Keep dispatch/batch targets short.
- If canceled while waiting, destroy the `wgpu::Device` only as a last-resort recovery path.
- Report whether cancellation happened immediately or after the active dispatch completed.

## UI Changes

Add a GPU execution section:

- Execution path:
  - Auto.
  - Direct full-buffer.
  - Persistent panelized.
  - Streaming blocked fallback.
- Intensity:
  - Safe.
  - Balanced.
  - High.
  - Experimental Max.
- Backoff enabled:
  - Default on.
- Show advanced telemetry:
  - Off by default.

Live telemetry:

- Current path.
- Current tile/panel size.
- Dispatch count.
- Last dispatch ms.
- Average dispatch ms.
- Max dispatch ms.
- Backoff events.
- Estimated VRAM use.
- GPU compute/total/transfer timing.

Warnings:

- High/Max warning for large matrices.
- VRAM pressure warning.
- TDR risk warning when a dispatch exceeds safety thresholds.

## Implementation Phases

### Phase 1: Telemetry First

Goal:

Make the current implementation observable before changing the architecture.

Tasks:

- Add dispatch count to results.
- Add last/average/max dispatch timing.
- Add block/chunk size to results.
- Add GPU path label.
- Log when blocked path is used.
- Log when any dispatch exceeds 250 ms, 500 ms, or 1000 ms.

Validation:

- Run `64`, `128`, and `512` self-tests.
- Confirm timing columns still work.
- Confirm no behavior change yet except additional telemetry.

### Phase 2: Direct Full-Buffer Tiled Path

Goal:

Keep full matrices on the GPU and compute in short row/column dispatches when binding limits allow it.

Tasks:

- Extend shader params:
  - `row_offset`
  - `col_offset`
  - `row_count`
  - `col_count`
  - `n`
- Compute rectangular C regions.
- Keep A/B/C buffers persistent.
- Read C once at the end.
- Add timestamp queries per dispatch or batch.

Validation:

- Compare against CPU for small sizes.
- Sample-validate large sizes.
- Confirm transfer/sync is lower than streaming blocked mode.

### Phase 3: Persistent Panelized Path

Goal:

Handle large matrices that exceed storage-buffer binding range limits without repeated uploads/readbacks.

Tasks:

- Add packed B column-panel layout.
- Add packed C output-panel layout.
- Implement CPU-side B packing.
- Bind subranges of A, packed B, and packed C.
- Dispatch row block by column panel.
- Read packed C once.
- Add sampled validation against packed C layout.

Validation:

- Unit-test packed B indexing.
- Unit-test packed C indexing.
- Validate `64`, `128`, and `256` against exact CPU output.
- Sample-validate larger sizes.

### Phase 4: Adaptive Governor

Goal:

Automatically tune tile size and queue depth to keep utilization high without dangerous dispatch duration.

Tasks:

- Define per-intensity thresholds.
- Track moving average dispatch time.
- Increase tile sizes after stable low-latency batches.
- Decrease tile sizes after slow batches.
- Log backoff events.
- Expose live telemetry in UI.

Validation:

- Confirm Safe starts conservative.
- Confirm Balanced/High increase tile size when stable.
- Confirm artificial threshold breaches trigger backoff.

### Phase 5: Stress Mode

Goal:

Add a dedicated high-utilization mode that behaves more like a graphics benchmark.

Tasks:

- Add stress mode duration options.
- Reuse persistent buffers.
- Skip full readback by default.
- Validate sampled output periodically.
- Ramp intensity gradually.
- Show live utilization-oriented telemetry.

Validation:

- Confirm cancellation works quickly.
- Confirm telemetry updates while running.
- Confirm no full readback bottleneck unless validation requires it.

### Phase 6: Shader Tuning

Goal:

Improve GPU occupancy and arithmetic throughput.

Tasks:

- Benchmark multiple workgroup shapes.
- Benchmark multiple outputs per thread.
- Tune row block and column panel defaults.
- Track effective TFLOP/s.
- Keep correctness validation for every shader variant.

Validation:

- Compare variants on the same GPU and matrix size.
- Keep the fastest stable variant per path.
- Preserve fallback path for incompatible adapters.

## Testing Plan

### Unit Tests

- Packed B layout indexing.
- Packed C layout indexing.
- Tile coverage for full matrix.
- No duplicate output cells.
- No missing output cells.
- Governor tile-size growth.
- Governor backoff behavior.
- VRAM warning thresholds.
- CLI parsing for intensity and execution path.

### Correctness Tests

Exact CPU comparison:

- `4x4`
- `8x8`
- `16x16`
- `64x64`
- `128x128`

Sampled validation:

- `512x512`
- `1024x1024`
- `2048x2048`
- larger sizes only when safe to run.

### Performance Tests

Record:

- GPU path.
- Matrix size.
- Dispatch count.
- Last/average/max dispatch ms.
- GPU compute ms.
- GPU total ms.
- Transfer/sync ms.
- Effective TFLOP/s.
- Backoff count.

Run:

- Safe mode.
- Balanced mode.
- High mode only after Safe/Balanced are stable.

### Safety Tests

- Cancel during CPU phase.
- Cancel during GPU phase.
- Cancel during readback.
- Force low dispatch threshold and confirm backoff.
- Force blocked path and confirm it still completes.
- Attempt large matrix near VRAM warning threshold and confirm warning appears.

## Acceptance Criteria

The optimization is successful when:

- Large matrices compute the full result or sampled-validatable full result.
- Safe mode avoids intentionally long dispatches.
- Cancellation waits for at most the current short dispatch/batch.
- GPU compute timing is available when timestamp queries are supported.
- Transfer/sync timing clearly shows setup/readback overhead.
- Direct or persistent panelized path is faster than streaming blocked mode when memory allows.
- Stress mode can sustain high GPU compute load without using multi-second single dispatches.
- The UI explains when a mode is safer, faster, or riskier.

## Important Caveat

No application can guarantee that a PC will not crash under high GPU load. A stable GPU, driver, PSU, power cable, motherboard, and cooling setup should tolerate full utilization. However, high load can expose:

- Driver bugs.
- Unstable overclocks or undervolts.
- Power-supply transient issues.
- Loose or overloaded GPU power cables.
- Thermal problems.
- Faulty VRAM or system RAM.

The tool should reduce software-induced crash risk by avoiding long GPU hangs, but it cannot make unstable hardware stable.

## Recommended Next Step

Start with Phase 1 telemetry, then implement the Direct Full-Buffer Tiled Path. That gives immediate visibility into dispatch timing and improves performance for matrices that already fit binding limits. After that, implement the Persistent Panelized Path for `8192x8192` and larger matrices where binding limits currently force slower streaming behavior.
