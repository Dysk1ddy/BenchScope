# BenchScope

Native desktop tool for comparing CPU matrix multiplication against GPU compute, with storage, memory, battery, network, and device-information diagnostic tools built into the same app.

The primary implementation is now Rust + `wgpu` + `egui`. The earlier Python Direct3D prototype remains archived in `archive/benchscope.py` as a fallback/reference implementation.

## Project Layout

- `src/` - main Rust desktop application.
- `sensor-helper/` - C# LibreHardwareMonitor sensor helper.
- `plans/` - implementation and feature planning notes.
- `scripts/` - local launcher and utility scripts.
- `config/` - tool configuration files.
- `archive/` - older prototype/reference code.

## Run

Double-click `scripts\RUN_TESTER.bat`, or run the release binary directly:

```powershell
.\target\release\BenchScope.exe
```

The source-tree launcher checks for Cargo first. If Rust is missing, it runs `scripts\Bootstrap-Developer.ps1 -InstallRust` through `winget`, then retries the release build.

If the release binary has not been built yet:

```powershell
$env:CARGO_NET_OFFLINE='false'
& "$env:USERPROFILE\.cargo\bin\cargo.exe" run --release
```

## Command-Line Checks

List detected `wgpu` adapters:

```powershell
.\target\release\BenchScope.exe --list-gpus
```

Run a small CPU/GPU smoke test:

```powershell
.\target\release\BenchScope.exe --self-test --size 64
```

Run an AI training GPU smoke test:

```powershell
.\target\release\BenchScope.exe --ai-training-smoke-test
.\target\release\BenchScope.exe --ai-training-smoke-test --ai-workload mlp
.\target\release\BenchScope.exe --ai-training-smoke-test --ai-workload transformer
.\target\release\BenchScope.exe --ai-training-smoke-test --ai-workload optimizer
```

Probe an optional PyTorch CUDA backend:

```powershell
.\target\release\BenchScope.exe --probe-pytorch-cuda
.\target\release\BenchScope.exe --probe-pytorch-cuda --python C:\path\to\python.exe
```

Run the current PyTorch CUDA training smoke path:

```powershell
.\target\release\BenchScope.exe --ai-training-smoke-test --ai-training-backend pytorch-cuda --ai-workload linear
.\target\release\BenchScope.exe --ai-training-smoke-test --ai-training-backend pytorch-cuda --ai-workload linear --python C:\path\to\python.exe
.\target\release\BenchScope.exe --ai-training-smoke-test --ai-training-backend pytorch-cuda --ai-workload linear --cuda-device 1
```

Run a Memtest-style user-mode RAM test:

```powershell
.\target\release\BenchScope.exe --ram-test --ram-size auto
.\target\release\BenchScope.exe --ram-test --ram-size 1g
```

Select a specific adapter:

```powershell
.\target\release\BenchScope.exe --self-test --size 64 --adapter 1
```

Use the sampled CPU estimate path:

```powershell
.\target\release\BenchScope.exe --self-test --size 512 --estimate-cpu
```

Choose the GPU submission intensity used by self-test:

```powershell
.\target\release\BenchScope.exe --self-test --size 512 --gpu-intensity safe
.\target\release\BenchScope.exe --self-test --size 512 --gpu-intensity balanced
.\target\release\BenchScope.exe --self-test --size 512 --gpu-intensity high
```

## Current Features

- Main menu with separate tool views for the matrix benchmark, matrix stress test, drive benchmark, storage health checker, RAM tester, battery health diagnostic, network hardware diagnostic, and device information viewer.
- Device Information Viewer with HWiNFO-style system inventory for OS/system, BIOS version and date, baseboard, CPU details, RAM modules, disks/volumes, GPUs, monitors, network adapters, and signed-driver records with provider, version, date, signer, and INF metadata.
- Device Information Viewer includes a provider coverage plan showing which details are available through current Windows/CIM, DXGI/wgpu, Storage Health, Network Diagnostic, and BenchScope sensor-service paths, plus which HWiNFO-class gaps would need future signed driver/vendor-provider work.
- SSD / HDD Health Checker tool with SMART/NVMe health snapshots, temperature, life estimates, bad-sector warning counters, read-only sampled scans, quick benchmark hook, and Markdown report export.
- RAM tester tool with Memtest-style moving inversions, walking-bit samples, address-sensitive patterns, pseudo-random verification, modulo-stride checks, and block-move stress.
- RAM tester runtime is capped to 2 minutes per installed 8 GiB of system memory and reports tested bytes separately from installed RAM.
- Each tool view has a Back control that returns to the main menu; active tests ask for cancellation before leaving.
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
- Matrix stress test tool with cancelable 1-minute, 5-minute, or infinite CPU/GPU repeat runs.
- GPU Memory Bandwidth tool with separate internal read/write, GPU buffer copy, CPU-to-GPU upload, GPU-to-CPU readback, and optional round-trip transfer tests.
- AI Training GPU Benchmark tool focused on PyTorch CUDA training workloads with linear-layer, MLP, and transformer runs reporting training throughput, average/p95 step latency, precision, step timing split, FLOPs, and CUDA memory, plus a clearly labeled portable WGPU proxy path for cross-vendor synthetic fallback and optimizer/update pressure.
- Separate CPU and GPU progress bars with roughly 5Hz progress sampling and an estimated time remaining during single benchmark runs.
- Large GPU runs report real chunk/block progress so the progress bar keeps moving through long matrix computations.
- GPU matrices that exceed an adapter's storage-buffer binding limit use intensity-controlled legal row/column blocks instead of binding the whole matrix at once.
- CPU estimate results are labeled `Est.` and include the detected CPU model/logical processor count. Large estimates spend about two seconds computing real full-width CPU rows against the full-size B matrix, then extrapolate from completed row throughput.
- Startup shows a loading progress bar while hardware, drive, RAM, battery, network, and sensor-permission setup completes.
- Drive benchmark tool with sequential read, sequential write, random 4 KiB read, and random 4 KiB write tests.
- Drive benchmark includes a detected-drive picker and editable target folder.
- Drive tests report MB/s, IOPS for random tests, average latency, p95 latency, duration, file size, I/O mode, and notes.
- Bottom-right sensor panel shows CPU/GPU temperature and utilization for matrix benchmark and stress views, plus SSD temperature and utilization for the selected drive benchmark target when the operating system/provider exposes readings.
- Fullscreen can be toggled with the on-screen button or `F11`.
- Windows/NVIDIA sensor probes are used by default; BenchScope automatically relaunches the GUI as administrator for Windows hardware probes and does not launch the LibreHardwareMonitor helper or WinRing driver path.
- Benchmark logs and result tables include start/end/max temperature summaries when readings are available.
- Drive tests prefer Windows direct/no-buffering I/O and fall back to cached file I/O when direct mode is unavailable.
- Drive benchmark profiles keep measured subtests below a 30 second hard cap, with shorter Quick/Balanced/Thorough targets.
- Drive benchmark runs on a background worker, reports current-test and whole-suite progress, and can be canceled mid-run.
- Drive benchmark uses a temporary file in the selected target folder and attempts cleanup after completion or failure.
- Network hardware diagnostic tool for Wi-Fi and Ethernet adapter troubleshooting.
- Network tool lists adapters, labels physical versus virtual interfaces, and shows connection state, link speed, IP, gateway, DNS, Wi-Fi signal, and driver details.
- Network quick diagnosis runs gateway, DNS-server, public-IP, hostname, and DNS lookup checks with packet loss, latency, and jitter reporting.
- Network continuous monitor samples adapter state, Wi-Fi signal/link speed, and gateway latency over time for intermittent issues.
- Network findings call out likely weak Wi-Fi, low Ethernet negotiation/bad cable symptoms, DNS failures, gateway problems, packet loss, high jitter, and driver/device warnings.
- Network reports export to Markdown for troubleshooting notes.

## Build and Test

```powershell
$env:CARGO_NET_OFFLINE='false'
& "$env:USERPROFILE\.cargo\bin\cargo.exe" check
& "$env:USERPROFILE\.cargo\bin\cargo.exe" test
& "$env:USERPROFILE\.cargo\bin\cargo.exe" build --release
```

## Notes

GPU compute-only time uses `wgpu` timestamp queries. If an adapter does not expose timestamp queries, or if a large split run would need more timestamp queries than `wgpu` allows in one query set, the app still reports GPU total time and dispatch telemetry, and marks compute-only timing as `N/A`.

The Rust app is cross-vendor on Windows and uses the `wgpu` DX12 backend in release builds to keep startup and binary size lean. For best hardware measurements, choose a real integrated or discrete GPU rather than a software adapter.

Exact CPU multiplication is the default for every supported matrix size. For very large matrices, the optional CPU estimate mode warms up on a small matrix, spends about two seconds computing real full-width rows of the target matrix against the full B matrix, and extrapolates from the measured row throughput. Estimated timings are labeled `Est.`.

For very large GPU runs, some adapters expose less storage-buffer binding space than a full matrix requires. The app first tries a persistent panelized path in that case, keeping full GPU buffers allocated and binding legal subranges. If that is not possible because of buffer limits or alignment, it falls back to the streaming blocked path. When timestamp queries are supported, both paths sum compute-only timing across completed dispatches and derive transfer/sync time from total minus compute.

On Windows, a large compute dispatch that keeps the GPU busy for too long can trigger Timeout Detection and Recovery (TDR), which may reset the graphics driver or reboot the system if recovery fails. Full GPU utilization by itself should be safe on stable hardware, but long non-preemptible compute work can combine with driver bugs, overclocks, power spikes, or thermals. Use Safe mode first for 8192+ matrices, avoid High mode until the system is stable, and check Windows Event Viewer for `nvlddmkm`, LiveKernelEvent 117/141, WHEA, or power events after any crash.

The drive benchmark prefers direct/no-buffering I/O on Windows. If the selected path or filesystem rejects direct mode, the app falls back to cached I/O and labels the result. Cached mode is useful for quick comparisons, but it can include operating-system RAM cache effects, especially on repeated reads.

Temperature and utilization readings are supplemental telemetry and are sampled continuously at 1 Hz, even when no benchmark is running. BenchScope merges the BenchScope sensor service/driver and command-based Windows/NVIDIA probes. The safe provider layer uses `nvidia-smi`/NVML for GPU temperature when available, already-running OpenHardwareMonitor/LibreHardwareMonitor WMI namespaces for external CPU/GPU temperatures when present, Windows performance counters for utilization, and Windows storage reliability counters for the selected drive letter. BenchScope does not treat Windows ACPI thermal-zone values as CPU package/core temperatures because those readings are often static firmware zones. Unsupported or permission-blocked sensors show `N/A` and never prevent benchmarks from running. On Windows, BenchScope automatically relaunches the GUI as administrator before opening the main window.

The optional `sensor-helper/` project remains in the repository for reference, but the Rust app does not launch it. LibreHardwareMonitor can create or load a WinRing driver on Windows, and Microsoft Defender may identify that driver as `VulnerableDriver:WinNT/Winring0`; BenchScope avoids that path and uses the native BenchScope sensor driver plus safe Windows/NVIDIA probes instead.
