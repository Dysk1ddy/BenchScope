# BenchScope Sensor Driver Prototype

This directory contains the first safe implementation slice for the signed sensor driver/service path.

The driver is a KMDF control-device prototype. It exposes a narrow read-only IOCTL contract so the future BenchScope sensor service can verify driver installation, query capabilities, and request a normalized sensor snapshot.

The current prototype includes a gated Intel family 6 CPU telemetry path for package temperature, thermal-limit status, and package energy counter reads when the required MSRs are available. Unknown CPUs and unsupported MSRs return `Unsupported`. Motherboard / Super I/O support is intentionally reported as unsupported until a chip and board allowlist exists.

## Current Scope

- Creates one control device: `\\.\BenchScopeSensor`
- Restricts access to LocalSystem and built-in administrators.
- Supports read-only IOCTLs:
  - `IOCTL_BENCHSCOPE_SENSOR_GET_VERSION`
  - `IOCTL_BENCHSCOPE_SENSOR_GET_CAPABILITIES`
  - `IOCTL_BENCHSCOPE_SENSOR_GET_SNAPSHOT`
  - `IOCTL_BENCHSCOPE_SENSOR_GET_ADVANCED_TELEMETRY`
- Reports CPU package temperature on supported Intel family 6 systems.
- Reports CPU package energy counter support through the advanced telemetry IOCTL when RAPL MSRs are available.
- Reports motherboard / Super I/O and storage health provider status without exposing raw hardware access.

## Non-Goals For This Slice

- No WinRing0 or equivalent arbitrary register/port/memory access.
- No arbitrary MSR, SMBus, EC, fan, voltage, clock, RGB, or write IOCTLs.
- No motherboard / Super I/O port probing yet.
- No production signing package.
- No automatic installation from BenchScope yet.

## Build Requirements

- Visual Studio with C++ workload.
- Windows Driver Kit matching the installed Visual Studio version.
- MSVC Spectre-mitigated x64 libraries for the compiler used by the WDK build.

Build command:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\BUILD_SENSOR_DRIVER.ps1 -Configuration Debug -Platform x64 -SignMode Off
```

The script normalizes the `Path`/`PATH` environment block before invoking MSBuild because duplicate casing can break the Visual C++ task host. `-SignMode Off` is for local compile/signability checks only; loading the driver still requires test signing or production signing. Normal `cargo check` does not build this driver.

## Attestation Package Prep

The repo can stage a Microsoft attestation-signing CAB, but it cannot complete EV signing or Partner Center submission by itself.

Build Release x64 and create the unsigned attestation CAB:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\New-SensorDriverAttestationPackage.ps1
```

If the Release x64 driver package already exists, skip the build:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\New-SensorDriverAttestationPackage.ps1 -SkipBuild
```

The script stages files under `artifacts\attestation\stage\BenchScopeSensorDriver`, creates `artifacts\attestation\BenchScopeSensorDriver-attestation.cab`, and writes SHA-256 hashes beside it. The generated `artifacts` directory is intentionally ignored by Git.

Before submitting a CAB, complete [SECURITY_REVIEW_CHECKLIST.md](SECURITY_REVIEW_CHECKLIST.md), follow [ATTESTATION_SUBMISSION_RUNBOOK.md](ATTESTATION_SUBMISSION_RUNBOOK.md), and keep the milestone plan in [plans/ATTESTATION_SIGNING_PLAN.md](../plans/ATTESTATION_SIGNING_PLAN.md) up to date.

Run the lightweight source-surface check before packaging:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\Test-SensorDriverSecuritySurface.ps1
```

## Dev Install Flow

Test-signed kernel drivers require Windows test-signing mode. Enabling it is a persistent boot setting and requires a reboot:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\Enable-SensorDriverTestSigning.ps1
```

After reboot, run from an elevated PowerShell:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\Install-SensorDriver.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\Probe-SensorDriver.ps1
```

The dev installer imports the generated test certificate into LocalMachine Root and TrustedPublisher so Windows can trust the test-signed package. This is for local driver development only, not release distribution.

To remove the dev service:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\Uninstall-SensorDriver.ps1
```

## Next Steps

1. Add a user-mode sensor service that opens `\\.\BenchScopeSensor` and calls the version/capability/advanced telemetry IOCTLs.
2. Move package-power calculation into the service by sampling CPU package energy deltas over time.
3. Add an installer flow for development/test-signed builds.
4. Add motherboard / Super I/O support only after hardware/provider research and a chip/board allowlist.
5. Keep the IOCTL surface fixed and narrow.

See [SERVICE_INTEGRATION.md](SERVICE_INTEGRATION.md) for the expected service-side call pattern.
