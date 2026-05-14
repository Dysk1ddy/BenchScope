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

Choose the GPU submission intensity used by self-test:

```powershell
.\target\release\hardware_accel_tester.exe --self-test --size 512 --gpu-intensity safe
.\target\release\hardware_accel_tester.exe --self-test --size 512 --gpu-intensity balanced
.\target\release\hardware_accel_tester.exe --self-test --size 512 --gpu-intensity high
```

## Current Features

- CPU-only matrix multiplication timing by default, with an opt-in sampled estimate mode.
- CPU exact timing uses parallel worker threads for larger matrices while leaving one logical processor free for system responsiveness.
- GPU matrix multiplication using a tiled WGSL compute shader through `wgpu`.
- GPU adapter selection when multiple backends or devices are available.
- GPU intensity selection defaults to Safe mode, which uses smaller GPU submissions and brief pauses between large chunks to reduce Windows driver-timeout and power-spike risk.
- GPU execution paths are reported as Direct, Panelized, or Streaming:
  - Direct keeps full A/B/C buffers on the GPU when binding limits allow it.
  - Panelized keeps persistent GPU buffers and uses packed column/output panels when full matrix binding is too large but full buffers still fit.
  - Streaming is the compatibility fallback that uploads/reads one block at a time.
- Timing columns for CPU, GPU compute-only, GPU total with transfer, and transfer/sync overhead.
- Timestamp-query based GPU compute timing when supported by the selected adapter.
- Dispatch telemetry reports tile/panel shape, dispatch count, last/average/max dispatch time, and safety backoff count. When a large run would exceed `wgpu`'s timestamp-query limit, the app disables exact compute-only timestamps for that run and uses CPU-observed dispatch timings for telemetry instead of panicking.
- Pre-run warning when the estimated GPU working set exceeds the selected adapter's reported VRAM/shared-memory limit, with an explicit run-anyway override.
- Correctness validation against CPU output, using sampled validation when the CPU estimate mode is enabled.
- Cancelable single benchmark runs.
- Cancelable 1-minute or 5-minute repeat tests for CPU or GPU mode.
- Separate CPU and GPU progress bars with roughly 5Hz progress sampling and an estimated time remaining during single benchmark runs.
- Large GPU runs report real chunk/block progress so the progress bar keeps moving through long matrix computations.
- GPU matrices that exceed an adapter's storage-buffer binding limit use intensity-controlled legal row/column blocks instead of binding the whole matrix at once.
- CPU estimate results are labeled `Est.` and include the detected CPU model/logical processor count. Large estimates spend about two seconds computing real full-width CPU rows against the full-size B matrix, then extrapolate from completed row throughput.

## Build and Test

```powershell
$env:CARGO_NET_OFFLINE='false'
& "$env:USERPROFILE\.cargo\bin\cargo.exe" check
& "$env:USERPROFILE\.cargo\bin\cargo.exe" test
& "$env:USERPROFILE\.cargo\bin\cargo.exe" build --release
```

## Notes

GPU compute-only time uses `wgpu` timestamp queries. If an adapter does not expose timestamp queries, or if a large split run would need more timestamp queries than `wgpu` allows in one query set, the app still reports GPU total time and dispatch telemetry, and marks compute-only timing as `N/A`.

The Rust app is cross-vendor and can enumerate multiple `wgpu` backends, such as Vulkan, DX12, OpenGL, and software adapters. For best hardware measurements, choose a real integrated or discrete GPU rather than a software adapter.

Exact CPU multiplication is the default for every supported matrix size. For very large matrices, the optional CPU estimate mode warms up on a small matrix, spends about two seconds computing real full-width rows of the target matrix against the full B matrix, and extrapolates from the measured row throughput. Estimated timings are labeled `Est.`.

For very large GPU runs, some adapters expose less storage-buffer binding space than a full matrix requires. The app first tries a persistent panelized path in that case, keeping full GPU buffers allocated and binding legal subranges. If that is not possible because of buffer limits or alignment, it falls back to the streaming blocked path. When timestamp queries are supported, both paths sum compute-only timing across completed dispatches and derive transfer/sync time from total minus compute.

On Windows, a large compute dispatch that keeps the GPU busy for too long can trigger Timeout Detection and Recovery (TDR), which may reset the graphics driver or reboot the system if recovery fails. Full GPU utilization by itself should be safe on stable hardware, but long non-preemptible compute work can combine with driver bugs, overclocks, power spikes, or thermals. Use Safe mode first for 8192+ matrices, avoid High mode until the system is stable, and check Windows Event Viewer for `nvlddmkm`, LiveKernelEvent 117/141, WHEA, or power events after any crash.
