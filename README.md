# Hardware Acceleration Tester

Native desktop tool for comparing CPU matrix multiplication against GPU compute.

The primary implementation is now Rust + `wgpu` + `egui`. The earlier Python Direct3D prototype remains archived in `archive/hardware_accel_tester.py` as a fallback/reference implementation.

## Run

Double-click `RUN_TESTER.bat`, or run the release binary directly:

```powershell
.\target\release\hardware_accel_tester.exe
```

If the release binary has not been built yet:

```powershell
$env:CARGO_NET_OFFLINE='false'
& "$env:USERPROFILE\.cargo\bin\cargo.exe" run --release
```

## Command-Line Checks

List detected `wgpu` adapters:

```powershell
.\target\release\hardware_accel_tester.exe --list-gpus
```

Run a small CPU/GPU smoke test:

```powershell
.\target\release\hardware_accel_tester.exe --self-test --size 64
```

Select a specific adapter:

```powershell
.\target\release\hardware_accel_tester.exe --self-test --size 64 --adapter 1
```

## Current Features

- CPU-only matrix multiplication timing for manageable sizes, with a sampled CPU baseline estimate for very large matrices.
- GPU matrix multiplication using a tiled WGSL compute shader through `wgpu`.
- GPU adapter selection when multiple backends or devices are available.
- Timing columns for CPU, GPU compute-only, GPU total with transfer, and transfer/sync overhead.
- Timestamp-query based GPU compute timing when supported by the selected adapter.
- Pre-run warning when the estimated GPU working set exceeds the selected adapter's reported VRAM/shared-memory limit, with an explicit run-anyway override.
- Correctness validation against CPU output, using sampled validation for large matrices where keeping a full CPU output would make the run impractical.
- Cancelable single benchmark runs.
- Cancelable 1-minute or 5-minute repeat tests for CPU or GPU mode.
- Separate CPU and GPU progress bars with roughly 5Hz progress sampling and an estimated time remaining during single benchmark runs.
- Large GPU runs are split into smaller row chunks so cancellation can be observed during long matrix computations.

## Build and Test

```powershell
$env:CARGO_NET_OFFLINE='false'
& "$env:USERPROFILE\.cargo\bin\cargo.exe" check
& "$env:USERPROFILE\.cargo\bin\cargo.exe" test
& "$env:USERPROFILE\.cargo\bin\cargo.exe" build --release
```

## Notes

GPU compute-only time uses `wgpu` timestamp queries. If an adapter does not expose timestamp queries, the app still reports GPU total time and marks compute-only timing as `N/A`.

The Rust app is cross-vendor and can enumerate multiple `wgpu` backends, such as Vulkan, DX12, OpenGL, and software adapters. For best hardware measurements, choose a real integrated or discrete GPU rather than a software adapter.

Matrices larger than 2048x2048 use an estimated CPU baseline instead of a full exact CPU multiplication. Exact CPU multiplication at 8192x8192 is on the order of hundreds of billions of operations, so the app marks those CPU timings with `~` and avoids blocking the benchmark before the GPU test can run.
