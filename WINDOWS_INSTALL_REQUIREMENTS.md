# BenchScope Windows Install Requirements

This file tracks software or tools BenchScope may need beyond a default Windows installation.

## Short Answer

A packaged BenchScope release should not require Rust, Python, Visual Studio, the Windows Driver Kit, or .NET if the release includes the compiled companion binaries it needs. It should install or bundle the Microsoft Visual C++ runtime because the current release binaries import `VCRUNTIME140.dll`. The current source-tree launcher does require developer tooling because `scripts\RUN_TESTER.bat` builds the Rust app before running it, but it now bootstraps Rust through `scripts\Bootstrap-Developer.ps1 -InstallRust` when Cargo is missing.

BenchScope uses several Windows components that are normally preinstalled on Windows 10/11:

- `powershell.exe` / Windows PowerShell 5.1
- CIM/WMI providers and cmdlets such as `Get-CimInstance`
- Storage and network cmdlets such as `Get-PhysicalDisk`, `Get-StorageReliabilityCounter`, `Get-NetAdapter`, and `Get-NetIPConfiguration`
- `powercfg.exe`, `ping.exe`, `netsh.exe`
- DirectX/DXGI and the installed graphics stack
- UAC elevation and administrator access for hardware sensor/raw-volume paths

## Runtime Requirements For A Packaged App

| Requirement | Needed for | Required? | Autoinstaller/bundle candidate | Notes |
| --- | --- | --- | --- | --- |
| Windows 10/11 x64 | Main app and Windows diagnostics | Yes | No | This is the app platform, not something the installer should add. |
| Microsoft Visual C++ 2015-2022 x64 Redistributable runtime | `VCRUNTIME140.dll` and Universal CRT imports used by the current release binaries | Yes unless already present or bundled app-local | Yes | Clean Windows machines may not have `VCRUNTIME140.dll`. A future installer can install the official redistributable or ship allowed runtime DLLs app-local. |
| Working GPU driver with a `wgpu` backend such as DX12, Vulkan, OpenGL, or software fallback | GPU enumeration and GPU matrix benchmark | Yes for GPU benchmark | No | The app may see software adapters, but meaningful hardware results require the vendor/OEM GPU driver. Detect and link to vendor driver guidance instead of bundling drivers. |
| Administrator permission | GUI startup, storage raw-volume scans, and richer hardware sensor access | Yes for full functionality | No | The app already relaunches itself elevated on Windows. |
| `benchscope_sensor_service.exe` companion binary | Optional sensor bridge process | Optional | Yes | This is built from this repo and can be shipped next to `benchscope.exe`. The app falls back to safe Windows probes if the bridge is absent or incomplete. |
| `benchscope_sensor_probe.exe` companion binary | Optional driver/service diagnostics | Optional | Yes | Useful for troubleshooting the optional sensor driver path. |

## Optional Feature Enhancers

| Software/tool | Needed for | Required? | Autoinstaller/bundle candidate | Recommendation |
| --- | --- | --- | --- | --- |
| Current NVIDIA display driver with `nvidia-smi.exe` | NVIDIA GPU temperature through NVML/`nvidia-smi` | Optional | No | Do not bundle vendor GPU drivers. Detect presence through PATH and known NVIDIA install paths, then show guidance if missing. |
| LibreHardwareMonitor or OpenHardwareMonitor already running with WMI enabled | Broader CPU/GPU temperature readings through `root\LibreHardwareMonitor` or `root\OpenHardwareMonitor` | Optional | Conditional | Do not silently install. These tools can rely on low-level drivers such as WinRing0, which may be flagged by Microsoft Defender. Any future installer path should be opt-in and clearly explained. |
| BenchScope sensor driver package | Driver-backed CPU package temperature/telemetry on supported systems | Optional/prototype | Conditional | Package only after production signing is solved. Dev/test installs require test-signing mode and a reboot, so they are not appropriate for a normal end-user installer. |
| `BenchScope.SensorHelper.exe` plus LibreHardwareMonitor DLLs | Disabled/reference sensor helper path | Optional/not currently launched | Conditional | If this path is re-enabled, prefer publishing the helper self-contained and bundling it beside the app. Keep it opt-in because of the low-level sensor-driver concerns above. |

## Requirements To Run Or Build From This Source Tree

These are not needed by a normal packaged release, but they are needed for the current repo workflow.

| Software/tool | Needed for | Required? | Autoinstaller/bundle candidate | Notes |
| --- | --- | --- | --- | --- |
| Rust toolchain with Cargo | `cargo build`, `cargo run`, tests, and `scripts\RUN_TESTER.bat` | Yes for source checkout | Yes for a developer bootstrapper | `scripts\RUN_TESTER.bat` auto-runs `scripts\Bootstrap-Developer.ps1 -InstallRust` when Cargo is missing. This should remain a dev setup flow, not an end-user app installer requirement. |
| MSVC C++ build tools and Windows SDK | Rust MSVC linking on Windows | Usually yes for source checkout | Conditional | Can be automated with the Visual Studio Build Tools bootstrapper, but it is large and developer-focused. |
| Internet access to crates.io | First Rust dependency restore | Yes unless dependencies are cached/vendorized | No | For seamless offline installs, vendor/cache Rust dependencies in CI or release artifacts instead. |
| .NET SDK 10 | Building `sensor-helper/` | Optional | Conditional | Only needed if rebuilding the C# helper. A packaged helper can be self-contained to avoid a user-installed .NET runtime. |
| NuGet access | Restoring `LibreHardwareMonitorLib` for `sensor-helper/` | Optional | No | Only needed when rebuilding the C# helper from source. Release artifacts should include restored helper DLLs if this feature is enabled. |
| Visual Studio with C++ workload, Windows Driver Kit, and Spectre-mitigated x64 libraries | Building `sensor-driver/BenchScopeSensorDriver.vcxproj` | Optional driver development only | Conditional | Required by the prototype driver build scripts, not by the normal app. |
| Windows test-signing mode or production driver signing | Loading the prototype sensor driver | Optional driver development/release only | No for test-signing, conditional for production | Test-signing is a boot setting and requires reboot. A release installer should only install a properly signed production driver. |

## Archived Prototype Only

`archive\benchscope.py` is not the current app. If someone runs it manually, it needs:

| Software/tool | Needed for | Required? | Autoinstaller/bundle candidate | Notes |
| --- | --- | --- | --- | --- |
| Python 3 | Archived Direct3D prototype | No | No | Keep out of the main installer. |
| NumPy | Archived prototype matrix math | No | No | The script exits with an install hint if NumPy is missing. |

## Packaging Guidance

Bundle these in a future BenchScope installer:

- `benchscope.exe`
- `benchscope_sensor_service.exe` if the sensor bridge remains enabled
- `benchscope_sensor_probe.exe` for diagnostics
- Microsoft Visual C++ runtime through the official redistributable or app-local runtime DLLs
- Optional C# helper artifacts only if that path is intentionally re-enabled and shipped with clear opt-in language
- A production-signed BenchScope sensor driver package only after driver signing and uninstall/update flows are complete

Do not bundle these in the app installer:

- Rust/Cargo
- Visual Studio Build Tools
- Windows Driver Kit
- Python/NumPy
- GPU vendor drivers
- Unsigned or test-signed kernel drivers

For the smoothest user install, the app installer should detect missing optional providers, explain which feature is affected, and offer a link or opt-in action instead of treating optional telemetry as a hard failure.
