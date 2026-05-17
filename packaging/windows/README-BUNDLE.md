# BenchScope Windows Bundle

This folder is a staged BenchScope Windows x64 release payload. It is intentionally not a full installer yet; it is the input a future MSI/EXE bootstrapper can consume.

## Contents

- `BenchScope.exe` - main desktop application.
- `benchscope_sensor_service.exe` - optional sensor bridge companion.
- `benchscope_sensor_probe.exe` - optional sensor-driver diagnostic companion.
- `docs/` - install requirements, first-boot links, and redistribution notes.
- `tools/Test-WindowsRuntimePrereqs.ps1` - first-boot prerequisite and optional-provider checker.
- `tools/Test-WindowsBundleLifecycle.ps1` - install/update/uninstall smoke test helper.
- `Install-BenchScope.ps1` - PowerShell installer for this staged bundle.
- `Uninstall-BenchScope.ps1` - PowerShell uninstaller for installed/staged bundle payloads.
- `installers/` - optional bundled prerequisite installers, such as `vc_redist.x64.exe`, when explicitly included.
- `reports/` - reports generated while staging this bundle.

## First Run

Run `BenchScope.exe`. On Windows, BenchScope may relaunch elevated so hardware and storage diagnostics can use richer Windows providers.

If GPU benchmarking or telemetry is degraded, use `docs/FIRST_BOOT_LINKS.md` for vendor driver and optional sensor-provider links. Developer tools such as Rust, Visual Studio Build Tools, the Windows Driver Kit, and .NET are not required for this bundled app.

## Prerequisite Check

From an elevated PowerShell prompt:

```powershell
.\tools\Test-WindowsRuntimePrereqs.ps1 -BundlePath . -OutputPath .\reports\first-boot-prereqs.md
```

The report is informational by default. Missing optional providers should degrade BenchScope telemetry instead of blocking the app.

## Lifecycle Smoke

From the repo root after staging a bundle:

```powershell
.\scripts\Test-WindowsBundleLifecycle.ps1 -BundlePath .\dist\BenchScope-0.1.0-windows-x64 -ReportPath .\dist\bundle-lifecycle-smoke.md
```

For clean-VM validation, follow `docs\CLEAN_VM_VALIDATION.md`.

## Install From This Bundle

Machine-scope install, from an elevated PowerShell prompt:

```powershell
.\Install-BenchScope.ps1 -Scope Machine -RunPrereqCheck
```

Per-user install without writing to Program Files:

```powershell
.\Install-BenchScope.ps1 -Scope CurrentUser -RunPrereqCheck
```

If `installers\vc_redist.x64.exe` is included in the bundle, add `-InstallVcRedist` to repair the VC++ runtime before copying BenchScope files.

Rerun `Install-BenchScope.ps1` with the same install directory to update an existing install. The installer removes old managed bundle files that are no longer present in the new manifest and preserves generated reports.

## Uninstall

From the installed BenchScope directory:

```powershell
.\Uninstall-BenchScope.ps1
```

Reports are preserved by default. To remove generated reports and clean up the install directory when no non-managed files remain:

```powershell
.\Uninstall-BenchScope.ps1 -RemoveReports
```
