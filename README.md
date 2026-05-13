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

Use the sampled CPU estimate path:

```powershell
.\target\release\hardware_accel_tester.exe --self-test --size 512 --estimate-cpu
```

## Current Features

- CPU-only matrix multiplication timing by default, with an opt-in sampled estimate mode.
- CPU exact timing uses parallel worker threads for larger matrices while leaving one logical processor free for system responsiveness.
- GPU matrix multiplication using a tiled WGSL compute shader through `wgpu`.
- GPU adapter selection when multiple backends or devices are available.
- Timing columns for CPU, GPU compute-only, GPU total with transfer, and transfer/sync overhead.
- Timestamp-query based GPU compute timing when supported by the selected adapter.
- Pre-run warning when the estimated GPU working set exceeds the selected adapter's reported VRAM/shared-memory limit, with an explicit run-anyway override.
- Correctness validation against CPU output, using sampled validation when the CPU estimate mode is enabled.
- Cancelable single benchmark runs.
- Cancelable 1-minute or 5-minute repeat tests for CPU or GPU mode.
- Separate CPU and GPU progress bars with roughly 5Hz progress sampling and an estimated time remaining during single benchmark runs.
- Large GPU runs report real chunk/block progress so the progress bar keeps moving through long matrix computations.
- GPU matrices that exceed an adapter's storage-buffer binding limit use larger legal row/column blocks to keep the GPU busier instead of binding the whole matrix at once.
- CPU estimate results are labeled `Est.` and include the detected CPU model/logical processor count used for the calibrated estimate.

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

Exact CPU multiplication is the default for every supported matrix size. For very large matrices, the optional CPU estimate mode runs the same CPU multiply implementation on a representative submatrix, chooses the calibration size from the detected CPU class, and extrapolates from that measured throughput; estimated timings are labeled `Est.`.

For very large GPU runs, some adapters expose less storage-buffer binding space than a full matrix requires. The app automatically switches to a blocked GPU path in that case. When timestamp queries are supported, the blocked path sums compute-only timing across completed row/column blocks and derives transfer/sync time from total minus compute. The blocked path also reports progress per completed block.
