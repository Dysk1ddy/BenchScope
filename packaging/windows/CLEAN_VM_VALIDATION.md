# BenchScope Clean Windows VM Validation

Use this checklist on a fresh Windows 10/11 x64 VM before publishing a user-facing bundle.

## VM Baseline

- Fresh Windows 10/11 x64 install.
- No Rust toolchain.
- No Visual Studio, Visual Studio Build Tools, Windows Driver Kit, or .NET SDK.
- No LibreHardwareMonitor/OpenHardwareMonitor.
- Use a normal user account first; elevate only when a command explicitly asks.

## Bundle Without VC++ Redist

From an extracted `BenchScope-<version>-windows-x64` folder:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tools\Test-WindowsRuntimePrereqs.ps1 -BundlePath . -OutputPath .\reports\clean-vm-prereqs.md
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\Install-BenchScope.ps1 -Scope CurrentUser -RunPrereqCheck
```

Expected:

- Rust, Visual Studio, WDK, and .NET SDK are not required.
- Missing optional providers are warnings/info, not hard launch blockers.
- If VC++ runtime is missing and not bundled, the prerequisite report links the official redistributable.

## Bundle With VC++ Redist

Build the bundle with:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\Build-WindowsBundle.ps1 -Clean -Zip -DownloadVcRedist
```

On the clean VM, from an elevated PowerShell prompt:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\Install-BenchScope.ps1 -Scope Machine -InstallVcRedist -RunPrereqCheck
```

Expected:

- `installers\vc_redist.x64.exe` is present in the bundle.
- `Install-BenchScope.ps1 -InstallVcRedist` installs or repairs the Microsoft Visual C++ runtime before copying BenchScope files.
- Exit code `3010` from `vc_redist.x64.exe` is treated as success with reboot requested.

## Lifecycle Smoke

Run:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\Test-WindowsBundleLifecycle.ps1 -BundlePath .\dist\BenchScope-0.1.0-windows-x64 -ReportPath .\dist\bundle-lifecycle-smoke.md
```

Expected:

- Install succeeds.
- Reinstall/update removes stale managed files.
- Reports under `reports\` survive update.
- Default uninstall removes managed files and preserves reports.
- `-RemoveReports` uninstall removes the install directory when no non-managed files remain.

## App Smoke

From the installed directory:

```powershell
.\BenchScope.exe --list-gpus
.\BenchScope.exe --self-test --size 64
```

Expected:

- The app starts without source-tree developer tools.
- CPU, storage, RAM, battery, network, and device-info features are not blocked by missing optional sensor providers.
- GPU benchmark may show degraded guidance if the VM exposes only a software/basic adapter.
