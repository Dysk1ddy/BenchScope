# BenchScope Windows First-Boot Bundle Plan

Source baseline: [`WINDOWS_INSTALL_REQUIREMENTS.md`](../WINDOWS_INSTALL_REQUIREMENTS.md)

This plan starts the packaging path for a Windows BenchScope install that works on first launch without requiring a user to install developer tooling by hand. The key split is:

- Bundle project-owned binaries and redistributable runtime files when licenses allow it.
- Detect platform, driver, and optional telemetry providers at first boot.
- Link users to anything that should not be bundled, especially OS updates, GPU/OEM drivers, developer tools, and unsigned/test-signed driver flows.

## Target Install Profiles

| Profile | Intended user | Installer behavior |
| --- | --- | --- |
| Standard release | Normal BenchScope user | Installs BenchScope binaries, companion binaries, VC++ runtime, shortcuts, and first-run checks. Does not install Rust, Visual Studio, WDK, Python, or vendor drivers. |
| Optional telemetry | User who wants broader sensor coverage | Shows detected gaps and opt-in links for vendor GPU drivers or external monitor tools. Does not silently install low-level sensor tools. |
| Developer bootstrap | Contributor building from source | Separate script or doc flow that can install Rust, MSVC Build Tools, Windows SDK, .NET SDK, and optionally WDK. This is not part of the end-user installer. |
| Driver development | Contributor working on `sensor-driver/` | Manual/elevated flow only. Requires Visual Studio/WDK, signing decisions, and potentially rebooting for test-signing mode. |

## Default End-User Bundle

These should be inside the release installer or installed by the installer chain.

| File/tool | Bundle action | First-boot validation | Notes |
| --- | --- | --- | --- |
| `BenchScope.exe` | Bundle from `cargo build --release` output. | Launch and run `--self-test --size 64` in release validation. | Primary app binary. |
| `benchscope_sensor_service.exe` | Bundle beside `BenchScope.exe` if the Rust sensor bridge remains enabled. | Check process/service startup path does not block the GUI. | Optional runtime companion from this repo. |
| `benchscope_sensor_probe.exe` | Bundle beside `BenchScope.exe`. | Confirm it can run as an admin diagnostic helper. | Useful for support and sensor-driver troubleshooting. |
| Microsoft Visual C++ 2015-2022 x64 runtime | Bundle the official redistributable installer, or ship allowed app-local runtime DLLs. | Detect `VCRUNTIME140.dll` availability before launching BenchScope. | Use the official [VC++ redistributable download page](https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist?view=msvc-170) and x64 permalink: [`vc_redist.x64.exe`](https://aka.ms/vs/17/release/vc_redist.x64.exe). Confirm redistribution terms before embedding. |
| License/readme notices | Bundle with installer payload. | Ensure installed folder includes third-party notices for bundled runtime pieces. | Needed if app-local VC++ runtime DLLs or helper DLLs are shipped. |
| Shortcuts | Bundle/start-menu and optional desktop shortcut. | Shortcut launches elevated relaunch path correctly. | Existing `Open BenchScope.lnk` can guide shortcut naming, but installer should create its own shortcut. |

## Items To Detect And Link Instead Of Bundling

| Requirement | Why not bundle | First-boot behavior | Link target |
| --- | --- | --- | --- |
| Windows 10/11 x64 | Operating system requirement. | Block unsupported OS/architecture with a clear message. | [Windows 11 download/support](https://www.microsoft.com/software-download/windows11) or OEM support page. |
| GPU/OEM display drivers | Vendor-specific, hardware-specific, and frequently updated. | If BenchScope sees only software adapters, Microsoft Basic Display Adapter, missing `nvidia-smi`, or weak `wgpu` backend coverage, show driver guidance. | [NVIDIA drivers](https://www.nvidia.com/en-us/drivers/), [AMD drivers](https://www.amd.com/en/support/download/drivers.html), [Intel Driver & Support Assistant](https://www.intel.com/content/www/us/en/support/detect.html), plus OEM support pages when available. |
| Administrator permission | Security boundary, not software. | App already relaunches elevated; installer should document why elevation is used. | No file link. Explain in first-run UI and docs. |
| LibreHardwareMonitor/OpenHardwareMonitor | Optional low-level sensor providers can rely on drivers that security tools may flag. | Detect WMI namespaces `root\LibreHardwareMonitor` and `root\OpenHardwareMonitor`; if absent, show opt-in links and security note. | [LibreHardwareMonitor releases](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/releases), [OpenHardwareMonitor downloads](https://openhardwaremonitor.org/downloads/). |
| NVIDIA `nvidia-smi.exe` | Ships with NVIDIA driver packages, not BenchScope. | Search PATH and common NVIDIA install paths; if missing on NVIDIA hardware, link driver page. | [NVIDIA drivers](https://www.nvidia.com/en-us/drivers/). |
| Production-signed kernel sensor driver package | Not ready until signing, uninstall, rollback, and update flows are production-grade. | Hide normal install path until signed package exists. Driver dev remains manual. | [Microsoft driver signing](https://learn.microsoft.com/windows-hardware/drivers/install/driver-signing) and [driver code signing requirements](https://learn.microsoft.com/windows-hardware/drivers/dashboard/code-signing-reqs). |
| Test-signing mode | Persistent boot setting requiring admin rights and reboot; inappropriate for end users. | Never enable from standard installer. Keep in driver-dev docs/scripts only. | [Microsoft test signing docs](https://learn.microsoft.com/windows-hardware/drivers/install/test-signing). |

## External Download Inventory

These are the concrete external installers or download pages the project should either bundle with permission, cache for developer/offline setup, or link from first-run diagnostics.

| External file/page | Bundle policy | Used by | Link |
| --- | --- | --- | --- |
| `vc_redist.x64.exe` | Bundle candidate for the standard installer, subject to Microsoft redistribution terms. | End-user runtime. | [Latest supported VC++ Redistributable](https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist?view=msvc-170), [x64 permalink](https://aka.ms/vs/17/release/vc_redist.x64.exe). |
| `rustup-init.exe` | Do not bundle in app installer; link or cache only for developer bootstrap. | Source builds. | [Install Rust](https://www.rust-lang.org/tools/install). |
| `vs_BuildTools.exe` | Do not bundle in app installer; link or cache only for developer bootstrap. | Rust/MSVC source builds. | [Visual Studio downloads](https://visualstudio.microsoft.com/downloads/), [Build Tools permalink](https://aka.ms/vs/17/release/vs_BuildTools.exe). |
| Windows SDK installer/components | Do not bundle in app installer. Install through Visual Studio Build Tools or link for developers. | Rust/MSVC linking and Windows headers/libs. | [Windows SDK](https://developer.microsoft.com/windows/downloads/windows-sdk/). |
| WDK installer or EWDK ISO | Do not bundle in app installer. Link for driver developers only. | `sensor-driver/` builds. | [Download the WDK](https://learn.microsoft.com/windows-hardware/drivers/download-the-wdk). |
| `.NET SDK 10` installer | Do not bundle in app installer. Link/cache only if the C# helper build is requested. | `sensor-helper/` builds. | [.NET 10 downloads](https://dotnet.microsoft.com/en-us/download/dotnet/10.0). |
| NuGet package restore from `nuget.org` | Do not bundle for normal users. CI can cache packages for offline developer builds. | `LibreHardwareMonitorLib` restore for `sensor-helper/`. | [nuget.org](https://www.nuget.org/). |
| NVIDIA, AMD, Intel, or OEM GPU driver installer | Never bundle. Drivers are hardware-specific and should come from vendors/OEMs. | GPU performance and optional telemetry. | [NVIDIA](https://www.nvidia.com/en-us/drivers/), [AMD](https://www.amd.com/en/support/download/drivers.html), [Intel](https://www.intel.com/content/www/us/en/support/detect.html). |
| LibreHardwareMonitor/OpenHardwareMonitor zip/release | Link only unless BenchScope later adds an explicit, opt-in helper package with security language. | Optional broader sensor telemetry. | [LibreHardwareMonitor releases](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/releases), [OpenHardwareMonitor downloads](https://openhardwaremonitor.org/downloads/). |

## Developer Bootstrap Bundle/Link Plan

The source-tree workflow should be separate from the app installer. A future `scripts\Bootstrap-Developer.ps1` can offer an online install path and an offline/cache path.

| Tool | Needed for | Bundle or link | Proposed handling |
| --- | --- | --- | --- |
| Rust toolchain with Cargo | Building the Rust app and binaries. | Link/install, not app bundle. | Prefer `winget` or [`rustup`](https://www.rust-lang.org/tools/install). Validate `cargo --version` and `rustc --version`. |
| MSVC C++ Build Tools and Windows SDK | Rust MSVC linking on Windows. | Link/install for contributors. | Prefer [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/) with C++ workload and Windows SDK. Validate `cl.exe`, `link.exe`, and SDK paths. |
| Windows Package Manager `winget` | Optional bootstrap automation. | Preinstalled on many Windows 10/11 systems, link if missing. | Use Microsoft docs for [`winget`](https://learn.microsoft.com/windows/package-manager/winget/). Scripts should degrade to manual links if `winget` is unavailable. |
| .NET SDK 10 | Rebuilding `sensor-helper/`. | Link/install only if helper build is requested. | Use [.NET 10 downloads](https://dotnet.microsoft.com/en-us/download/dotnet/10.0). Validate `dotnet --list-sdks`. |
| NuGet access and `LibreHardwareMonitorLib` | Restoring `sensor-helper/` dependencies. | Link/source restore. | Keep using `config\NuGet.Config` and [nuget.org](https://www.nuget.org/). For offline builds, cache package artifacts in CI rather than requiring user restore. |
| Visual Studio, WDK, Spectre-mitigated x64 libs | Building `sensor-driver/`. | Link/install for driver developers only. | Follow Microsoft [WDK download guidance](https://learn.microsoft.com/windows-hardware/drivers/download-the-wdk). Validate WDK version expected by `scripts\BUILD_SENSOR_DRIVER.ps1`. |
| Enterprise WDK | Alternative driver build environment. | Link only. | Useful for reproducible command-line driver builds, but too large/specialized for the normal installer. |
| Python/NumPy | Archived prototype only. | Exclude. | Keep out of all first-boot flows unless the archived prototype is intentionally revived. |

## Proposed Installer Flow

1. Build release artifacts in CI:
   - `BenchScope.exe`
   - `benchscope_sensor_service.exe`
   - `benchscope_sensor_probe.exe`
   - third-party notices
   - optional `vc_redist.x64.exe` cache, or app-local VC++ runtime DLLs if redistribution terms are satisfied
2. Create a signed `BenchScopeSetup.exe` or MSI bootstrapper:
   - Checks Windows version and x64 architecture.
   - Installs or repairs VC++ runtime before placing app binaries.
   - Installs files under `%ProgramFiles%\BenchScope`.
   - Creates start menu and optional desktop shortcut.
   - Does not install developer tools or vendor drivers.
3. First app launch performs non-blocking checks:
   - Confirms administrator elevation or requests relaunch.
   - Verifies Windows command providers used by diagnostics: PowerShell, CIM/WMI, `powercfg.exe`, `ping.exe`, `netsh.exe`, Storage cmdlets, and Network cmdlets.
   - Enumerates `wgpu` adapters and marks software/limited adapters as degraded.
   - Checks optional providers: `nvidia-smi`, LibreHardwareMonitor/OpenHardwareMonitor WMI namespaces, BenchScope sensor service, BenchScope sensor driver.
   - Shows `N/A` or degraded telemetry instead of failing benchmarks.
4. Optional provider screen links missing pieces:
   - GPU driver links by detected vendor where possible.
   - LibreHardwareMonitor/OpenHardwareMonitor links with a low-level-driver warning.
   - Sensor driver development links only in developer/debug builds.
5. Support diagnostics:
   - Add an "Export install diagnostics" report that records installed app version, VC++ runtime status, GPU adapters, optional provider status, OS build, elevation state, and links shown.

## Proposed Developer Bootstrap Flow

Create a separate `scripts\Bootstrap-Developer.ps1` later with these modes:

| Mode | Behavior |
| --- | --- |
| `-CheckOnly` | Reports missing Rust, Cargo, MSVC tools, Windows SDK, .NET SDK, and WDK without installing anything. |
| `-InstallCore` | Installs Rust and MSVC Build Tools/Windows SDK using `winget` or opens official links when automation is unavailable. |
| `-InstallHelperTools` | Adds .NET SDK 10 for `sensor-helper/`. |
| `-InstallDriverTools` | Opens Visual Studio/WDK guidance and validates driver build prerequisites. Avoid fully automating test-signing. |
| `-OfflineCache <path>` | Uses predownloaded installers and package caches created by CI. |

## First-Boot Validation Matrix

| Scenario | Expected result |
| --- | --- |
| Clean Windows 11 x64 VM without Rust/.NET/Visual Studio | BenchScope launches from installer. No developer tools required. |
| Clean Windows machine missing VC++ runtime | Installer installs or repairs VC++ runtime before first launch. |
| Machine with Microsoft Basic Display Adapter only | App launches, CPU/storage/RAM/network tools work, GPU benchmark shows driver guidance. |
| NVIDIA machine without `nvidia-smi` on PATH | GPU benchmark can still run through `wgpu`; telemetry links NVIDIA driver guidance. |
| No LibreHardwareMonitor/OpenHardwareMonitor | Sensor fields show `N/A` or Windows/NVIDIA fallback values; app does not prompt to silently install anything. |
| Non-admin launch | App relaunches elevated or explains limited functionality. |
| Driver-dev machine with test-signing disabled | Standard app still works; driver install script blocks with existing test-signing guidance. |

## Open Decisions

- Choose installer technology: WiX/MSI Burn bootstrapper, Inno Setup, NSIS, or MSIX plus an external runtime bootstrapper.
- Decide whether VC++ runtime is installed globally through `vc_redist.x64.exe` or shipped app-local.
- Decide whether `benchscope_sensor_service.exe` runs on demand, as a Windows service, or only when the app requests it.
- Decide whether the C# `sensor-helper/` path remains reference-only or becomes a shipped optional helper.
- Define production driver signing, rollback, and uninstall requirements before any kernel driver is included in a user installer.
- Add code-signing requirements for BenchScope binaries and installer to reduce SmartScreen friction.

## Near-Term Next Steps

1. Wire `scripts\Build-WindowsBundle.ps1` into CI so every release produces a staging folder matching the future installer layout.
2. Reuse `scripts\Test-WindowsRuntimePrereqs.ps1` from the installer and, later, from BenchScope first launch.
3. Prototype VC++ runtime repair by having the installer run bundled `installers\vc_redist.x64.exe` when the checker reports it missing.
4. Add an in-app optional provider report/export path so support can see exactly what was linked or skipped.
5. Draft `scripts\Bootstrap-Developer.ps1` as a separate developer-only bootstrapper.

## Initial Implementation

- `scripts\Build-WindowsBundle.ps1` stages the three Rust release binaries, bundle docs, first-boot links, the prerequisite checker, optional `vc_redist.x64.exe`, a prerequisite report, and a SHA-256 manifest under `dist\BenchScope-<version>-windows-x64`.
- `scripts\Test-WindowsRuntimePrereqs.ps1` checks Windows x64, elevation, VC++ runtime presence, Windows diagnostic command/cmdlet availability, GPU driver state, NVIDIA telemetry, optional hardware-monitor WMI namespaces, optional BenchScope sensor driver state, and expected bundle files.
- `scripts\Test-WindowsBundleLifecycle.ps1` installs a staged bundle into a temporary per-user target, verifies update stale-file cleanup, report preservation, default uninstall, and `-RemoveReports` cleanup.
- `scripts\Install-WindowsBundle.ps1` is copied into staged bundles as `Install-BenchScope.ps1`; it verifies the SHA-256 manifest, optionally runs bundled `vc_redist.x64.exe`, copies files to Program Files or LocalAppData, removes stale managed files during updates, creates shortcuts, and can run the installed prerequisite report.
- `scripts\Uninstall-WindowsBundle.ps1` is copied into staged bundles as `Uninstall-BenchScope.ps1`; it removes managed bundle files and shortcuts, blocks on running BenchScope processes unless asked to stop them, preserves reports by default, and can remove reports with `-RemoveReports`.
- `scripts\Bootstrap-Developer.ps1` is the separate developer-only lane for checking or installing Rust, MSVC Build Tools, Windows SDK, .NET SDK 10, and WDK through `winget`.
- `packaging\windows\README-BUNDLE.md`, `FIRST_BOOT_LINKS.md`, `CLEAN_VM_VALIDATION.md`, and `THIRD_PARTY_NOTICES.md` are copied into staged bundles.
- `.github\workflows\windows-bundle.yml` builds release binaries on Windows, stages/zips the bundle, runs the lifecycle smoke test, and uploads the bundle plus manifest/report artifacts. Manual runs can opt into downloading `vc_redist.x64.exe`; regular PR/tag runs link it instead of bundling it.
