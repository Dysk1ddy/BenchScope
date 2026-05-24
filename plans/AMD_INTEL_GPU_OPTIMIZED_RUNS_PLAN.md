# AMD And Intel Optimized GPU Runs Plan

## Implementation Status

Implemented in the matrix benchmark and matrix stress test:

- Adapter vendor detection for NVIDIA, AMD, Intel, and unknown adapters.
- `Auto optimized`, `Optimized WGPU`, and `Archived WGPU` stress backend choices.
- Adapter-correct auto routing:
  - NVIDIA tries PyTorch CUDA, then optimized WGPU.
  - AMD tries PyTorch ROCm, then optimized WGPU.
  - Intel tries PyTorch XPU, then optimized WGPU.
  - Unknown adapters use optimized WGPU.
- Normal matrix benchmark native probes for AMD ROCm and Intel XPU before WGPU fallback.
- Matrix stress native probes for AMD ROCm and Intel XPU before WGPU fallback.
- UI copy that names CUDA, ROCm, XPU, and optimized WGPU accurately.
- Unit coverage for vendor detection and backend routing.

Still future work:

- Persistent on-disk shader autotune cache.
- AMD/Intel theoretical baseline tables.
- Native oneAPI/oneMKL bridge beyond PyTorch XPU.
- Explicit ROCm/XPU install helpers inside the app.

## Purpose

Add optimized AMD and Intel GPU runs for the matrix benchmark and matrix stress test while keeping BenchScope honest about which backend is being measured.

The current cross-vendor path uses `wgpu` WGSL compute shaders. That is the correct compatibility baseline for NVIDIA, AMD, Intel, integrated GPUs, and software adapters. NVIDIA also has a native PyTorch CUDA path for some matrix runs. AMD and Intel need a similar plan, but the implementation should avoid pretending that one vendor's acceleration stack is available everywhere.

## Current State

Relevant files:

- `src/features/matrix_benchmark/model.rs`
- `src/features/matrix_benchmark/runner.rs`
- `src/features/matrix_benchmark/hardware.rs`
- `src/features/matrix_benchmark/stress_ui.rs`
- `src/features/ai_training_benchmark/runner.rs`
- `src/features/ai_training_benchmark/pytorch_cuda.rs`

Current backend behavior:

- `wgpu` enumerates all adapters through `wgpu::Backends::all()`.
- The normal matrix benchmark uses PyTorch CUDA only when the selected adapter looks like NVIDIA.
- AMD and Intel matrix benchmark runs use the portable WGPU path.
- The matrix stress test `Optimized` mode currently tries PyTorch CUDA before WGPU fallback.
- The new RTX theoretical TFLOPS table only affects NVIDIA theoretical baseline and efficiency display; it does not block AMD or Intel WGPU runs.

Immediate correctness issue:

- On hybrid machines with AMD or Intel plus NVIDIA, stress `Optimized` can run CUDA on the NVIDIA GPU even when the user selected an AMD or Intel adapter. Fix this before adding new native backends.

## Backend Naming

Use names that describe the actual execution stack:

```rust
enum MatrixGpuBackend {
    AutoOptimized,
    PortableWgpu,
    ArchivedWgpu,
}

enum GpuPath {
    PyTorchCuda,
    PyTorchRocm,
    PyTorchXpu,
    OptimizedWgpu,
    ArchivedWgpu,
    DirectFullBuffer,
    SmallTile,
    PersistentPanelized,
    StreamingBlocked,
}
```

User-facing labels:

- `Auto optimized`
- `Optimized WGPU`
- `Archived WGPU`
- `PyTorch CUDA`
- `PyTorch ROCm`
- `PyTorch XPU`

Do not label ROCm or XPU results as CUDA. Do not label WGPU shader results as native matrix-core results.

## Phase 1: Make AutoOptimized Adapter-Correct

Goal: ensure the selected adapter controls the backend.

Tasks:

- Add vendor helper predicates:
  - NVIDIA: vendor `0x10DE` or name contains `nvidia`.
  - AMD: vendor `0x1002` or name contains `amd` / `radeon`.
  - Intel: vendor `0x8086` or name contains `intel` / `arc` / `iris`.
- Change stress `Optimized` so CUDA is considered only for NVIDIA adapters.
- For AMD and Intel, route `Optimized` to the best WGPU path until native backend probes exist.
- Add logs that name the selected adapter and chosen backend.
- Add unit tests for backend routing using fake `AdapterInfo` values.

Acceptance:

- Selecting an AMD adapter never launches CUDA.
- Selecting an Intel adapter never launches CUDA.
- Selecting an NVIDIA adapter can still use CUDA when PyTorch CUDA is available.
- WGPU fallback remains available for every vendor.

## Phase 2: Split Optimized WGPU From Archived WGPU

Goal: make WGPU itself faster and tunable for AMD and Intel.

Tasks:

- Rename the current preserved comparison path to `ArchivedWgpu`.
- Add `OptimizedWgpu` as the main cross-vendor shader backend.
- Keep all current safety controls:
  - adapter memory preflight,
  - storage-buffer binding limit checks,
  - timestamp query fallback,
  - GPU intensity,
  - short dispatch batches,
  - cancellation between submissions.
- Keep `ArchivedWgpu` available for regression comparisons.

Acceptance:

- UI clearly distinguishes `Optimized WGPU` from `Archived WGPU`.
- Existing matrix stress tests still run on NVIDIA, AMD, and Intel without Python.
- Logs show when optimized WGPU was chosen.

## Phase 3: WGPU Shader Autotuning

Goal: choose a strong shader variant for the selected adapter instead of using one fixed kernel everywhere.

Candidate WGSL variants:

- Baseline tiled f32 kernel.
- Current small-tile register microkernel.
- AMD-oriented register-heavy f32 microkernel.
- Intel-oriented lower-register f32 microkernel.
- Large-dispatch panelized stress kernel.
- Optional f16 storage/compute variant when adapter features and WGPU support allow it.

Autotune strategy:

- On first run per adapter, compile candidate pipelines.
- Run a tiny benchmark for each candidate at the selected or nearest preset size.
- Measure GPU timestamp time when available; otherwise use CPU-observed completion time.
- Cache the winning variant by adapter identity:
  - vendor ID,
  - device ID,
  - adapter name,
  - backend,
  - driver string.
- Invalidate cache when driver or BenchScope version changes.

Likely tuning knobs:

- Workgroup size: `16x16`, `8x32`, `32x8`, `256x1`.
- Output columns per lane/thread.
- Microtile shape: `2x4`, `4x4`, `4x8`.
- Number of repeated rounds per stress dispatch.
- Batch submission size per GPU intensity.
- Panel row/column block size.

Acceptance:

- Autotune never blocks app launch.
- Autotune can be canceled.
- Failed candidate shaders are logged and skipped.
- A conservative default is used if no candidate wins.

## Phase 4: AMD Native Path With PyTorch ROCm

Goal: add an optional high-fidelity AMD backend when a supported ROCm PyTorch environment exists.

Preferred first integration:

- Reuse the existing out-of-process Python worker pattern.
- Add a ROCm probe:
  - import `torch`,
  - check `torch.version.hip`,
  - check `torch.cuda.is_available()`,
  - list devices through PyTorch's HIP-backed CUDA-compatible APIs,
  - match the selected adapter by name where possible.
- Run matrix stress through `torch.matmul` on the AMD device.
- Time with events or explicit synchronization.
- Emit the same line/event protocol as the CUDA helper where practical.

Important caveat:

- PyTorch ROCm exposes many HIP devices through the `torch.cuda` namespace. The UI and logs must still call the backend `PyTorch ROCm`.

Windows support caution:

- AMD's Windows PyTorch ROCm support is device-limited and the full ROCm stack may not be available on every Radeon model. Do not offer a one-click install until support detection and compatibility messaging are reliable.

Acceptance:

- If ROCm PyTorch is missing, the app falls back to optimized WGPU with a useful log message.
- If ROCm PyTorch is present but no matching AMD GPU is visible, fallback is clean.
- Results are labeled `PyTorch ROCm`.
- The backend reports PyTorch version, HIP/ROCm version, Python path, and device name.

## Phase 5: Intel Native Path With PyTorch XPU Or oneAPI

Goal: add an optional Intel-optimized backend where Intel's GPU software stack is available.

Preferred first integration:

- Add a Python probe for PyTorch XPU:
  - import `torch`,
  - check `hasattr(torch, "xpu")`,
  - check `torch.xpu.is_available()`,
  - list XPU device names if APIs are available,
  - match the selected adapter by name where possible.
- Run matrix stress with tensors on `xpu:0` or the matched device.
- Use XPU events if available; otherwise synchronize around timed regions.
- Label results `PyTorch XPU`.

Alternative later path:

- Native Rust/C++ bridge to oneAPI oneMKL/SYCL GEMM.
- This is more packaging-heavy than a Python probe and should wait until PyTorch XPU feasibility is known.

Acceptance:

- Missing XPU dependencies do not block WGPU runs.
- Results are labeled `PyTorch XPU`.
- Logs explain whether the app used XPU, optimized WGPU, or archived WGPU.

## Phase 6: AMD And Intel Theoretical Baselines

Goal: add honest theoretical baseline data for non-NVIDIA adapters.

Do this after backend routing is correct.

Separate baseline types:

- FP32 shader/ALU TFLOPS.
- FP16/bfloat16 matrix or AI accelerator TFLOPS when model documentation supports it.
- Memory bandwidth baseline for memory-bound modes.

Important rule:

- Do not compare a WGPU f32 shader result against an FP16 matrix-core headline number unless the result is clearly labeled as a proxy and the efficiency text explains the mismatch.

Implementation:

- Add a vendor-neutral theoretical spec table beside `gpu_theoretical.rs`, or rename it to a broader `gpu_specs.rs`.
- Store the metric type with each baseline.
- Display `N/A` when the selected backend and theoretical metric are not comparable.

Acceptance:

- AMD and Intel cards can show a theoretical baseline only when the metric matches the measured workload.
- NVIDIA RTX FP16 tensor-core baseline remains intact.
- Efficiency text never implies that a WGPU f32 run used tensor, XMX, or matrix cores.

## Phase 7: UI And Reporting

Add report fields:

- Selected adapter.
- Selected backend mode.
- Actual execution path.
- Native runtime status:
  - CUDA available,
  - ROCm available,
  - XPU available.
- Precision and math mode:
  - f32 WGPU,
  - f16 WGPU,
  - PyTorch fp32,
  - PyTorch fp16/bf16.
- Timing source:
  - GPU timestamp query,
  - CUDA/ROCm/XPU event,
  - CPU-observed synchronization.
- Theoretical baseline type.
- Efficiency baseline label.

UI behavior:

- `Auto optimized` should show the resolved path after a run.
- Installation prompts should remain explicit and vendor-specific.
- For AMD/Intel native backends, prefer "Probe environment" before "Install".
- If native stack is unavailable, show "Using Optimized WGPU" without alarming the user.

## Phase 8: Tests And Hardware Matrix

Unit tests:

- Vendor detection for NVIDIA, AMD, Intel, and unknown adapters.
- Backend route selection.
- Fallback route messages.
- Theoretical baseline comparability checks.
- Result labels for CUDA, ROCm, XPU, optimized WGPU, and archived WGPU.

Integration tests:

- WGPU smoke on at least one real adapter.
- CUDA unavailable path.
- ROCm unavailable path.
- XPU unavailable path.
- Hybrid adapter routing with fake or injectable adapter lists.

Manual hardware matrix:

- NVIDIA RTX discrete GPU.
- AMD Radeon RX 7000 or newer where ROCm PyTorch is supported.
- AMD integrated GPU using WGPU fallback.
- Intel Arc discrete GPU.
- Intel Iris/Xe/Core Ultra integrated GPU.
- Hybrid Intel plus NVIDIA laptop.
- Hybrid AMD plus NVIDIA desktop or laptop.

## Open Questions

- Should native AMD/Intel backends live under the matrix benchmark only, or should they also power AI training benchmark runs?
- Should WGPU f16 be exposed as a separate precision mode or only used internally when autotune proves it is faster?
- Is oneAPI oneMKL worth a native dependency, or is PyTorch XPU enough for the first Intel optimized path?
- Should ROCm/XPU install helpers be allowed at all, or should BenchScope only document external setup?
- How should device matching work when PyTorch reports a generic device name that does not exactly match WGPU/DXGI?

## First Implementation Slice

Start here:

1. Add vendor helper predicates in `runner.rs` or a small matrix backend module.
2. Fix stress `Optimized` so CUDA is gated to NVIDIA adapters.
3. Add `OptimizedWgpu` as the resolved path for AMD/Intel auto mode.
4. Add log lines and tests for route decisions.
5. Keep the existing WGPU shaders unchanged for this slice.

This slice is small, low-risk, and removes the current hybrid-system ambiguity before deeper optimization work begins.
