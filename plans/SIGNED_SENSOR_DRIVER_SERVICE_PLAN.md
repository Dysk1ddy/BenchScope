# Signed Sensor Driver and Service Plan

## Goal

Design a truly seamless BenchScope sensor backend for Windows that can read CPU, GPU, motherboard, fan, and storage telemetry without asking the user to manually start another monitoring tool.

## Implementation Status

Initial driver scaffolding has begun:

- Added `sensor-driver/` with a KMDF control-device prototype.
- Added a shared user/kernel IOCTL contract in `sensor-driver/include/BenchScopeSensorIoctl.h`.
- Added read-only IOCTLs for version, capability, and normalized snapshot queries.
- Added an INF and WDK project file for x64 Debug/Release builds.
- Added `scripts/BUILD_SENSOR_DRIVER.ps1` as a convenience wrapper that finds Visual Studio/WDK, normalizes duplicate `Path`/`PATH`, and supports unsigned signability builds.
- Added development scripts for test-signing setup, install, uninstall, and probing:
  - `scripts/Enable-SensorDriverTestSigning.ps1`
  - `scripts/Install-SensorDriver.ps1`
  - `scripts/Uninstall-SensorDriver.ps1`
  - `scripts/Probe-SensorDriver.ps1`
- Added `benchscope_sensor_probe`, a small Rust CLI that opens `\\.\BenchScopeSensor` and calls the version, capability, and snapshot IOCTLs.
- Added `benchscope_sensor_service`, the first user-mode bridge process. It opens `\\.\BenchScopeSensor`, converts driver IOCTL responses into BenchScope snapshot JSON, and supports `--stream` for the app-side reader.
- Wired `SensorManager` to prefer the service bridge when the binary is available, then merge in existing safe Windows fallback probes for gaps.
- Added safe user-mode providers inside the bridge for CPU/GPU utilization, NVIDIA GPU temperature via `nvidia-smi`, drive temperature via Windows Storage reliability counters, and RAM utilization.
- Tuned the bridge for streaming: it keeps one driver handle open, caches version/capability IOCTLs, samples CPU utilization through native Windows system-time deltas, caches slower GPU/drive temperature providers, and times out external commands.
- Verified the unsigned Debug driver package builds, passes INF signability, and generates a CAT file.

The local development machine has passed the test-signing gate for prototype work: Secure Boot was disabled, Windows test-signing was enabled, and the test-signed BenchScope driver was installed. That is only a development state; release distribution still requires a Microsoft-compatible signing path.

Attestation signing is now tracked as a dedicated milestone in [ATTESTATION_SIGNING_PLAN.md](ATTESTATION_SIGNING_PLAN.md).

The prototype deliberately does not read hardware yet. It returns unsupported sensor readings until a specific provider is researched, reviewed, and added behind a narrow IOCTL/service path.

Target experience:

- User opens `BenchScope.exe`.
- BenchScope starts normally and shows the existing startup loading progress.
- If privileged sensor access is required, Windows shows one standard UAC/install prompt during first setup.
- A trusted local service collects hardware telemetry in the background.
- BenchScope reads CPU/GPU/SSD temperature and utilization from that service.
- No WinRing0, vulnerable-driver-blocklist workaround, Defender exception, or user security downgrade is required.

This is a real Windows driver/service project, not a small application patch.

## Why This Exists

The current safe paths are limited:

- Windows performance counters can provide CPU/GPU utilization.
- `nvidia-smi`/NVML can provide NVIDIA GPU temperature when the NVIDIA driver exposes it.
- Windows storage reliability counters can provide some drive temperatures.
- Windows ACPI thermal-zone values can be static firmware readings, so they should not be used as CPU package temperature.
- CPU package/core temperatures and many motherboard sensors usually require low-level hardware access.

The previous broad-temperature approach used LibreHardwareMonitor-style access. That can work, but it often depends on low-level drivers such as WinRing0. Microsoft Defender classifies WinRing0 as a vulnerable driver, so BenchScope should not silently install or launch it.

## Non-Goals

- Do not bypass Microsoft Defender or the Microsoft vulnerable driver blocklist.
- Do not ask users to disable Memory Integrity, Core Isolation, Secure Boot, or driver block rules.
- Do not ship WinRing0 under another name.
- Do not silently load any kernel driver.
- Do not expose sensor control or fan/RGB write APIs in the first version.
- Do not make benchmark execution depend on sensor availability.

## Product Decision

BenchScope should have three sensor tiers:

1. **Safe user-mode probes**
   - Windows performance counters.
   - Windows storage counters.
   - NVIDIA NVML / `nvidia-smi` if installed.
   - External WMI providers if the user intentionally runs them.

2. **BenchScope signed sensor service**
   - Installed once.
   - Runs with least required privilege.
   - Owns all privileged telemetry collection.
   - Exposes a narrow local IPC API to BenchScope.

3. **BenchScope signed kernel driver**
   - Only if the service cannot reach necessary sensors through supported user-mode APIs.
   - Microsoft-signed or WHQL/attestation-signed.
   - Read-only by default.
   - Narrow IOCTL surface.

The service should be useful without the driver where possible. The driver is the last resort for hardware classes that truly require kernel access.

## High-Level Architecture

```text
BenchScope.exe
  |
  | local named pipe / ALPC / localhost-disabled IPC
  v
BenchScope Sensor Service (Windows service)
  |
  | user-mode providers
  | - PDH/performance counters
  | - NVML / vendor SDKs
  | - Windows storage APIs
  | - ACPI/WMI only when trustworthy
  |
  | optional IOCTLs
  v
BenchScope Sensor Driver (KMDF)
  |
  | read-only hardware access
  | - model-specific registers where allowed
  | - chipset/SMBus/EC access where legally and technically supportable
  | - sensor controller access
```

## Components

### BenchScope App

Responsibilities:

- Detect whether the sensor service is installed and healthy.
- Start the service if installed but stopped.
- Request service installation through a clear setup flow if missing.
- Read snapshots from the service.
- Show service/driver status in sensor tooltips.
- Keep benchmarks running if service setup is skipped or unavailable.

The app should not:

- Load a kernel driver directly.
- Perform arbitrary privileged hardware reads.
- Accept untrusted data as commands.

### Sensor Service

Responsibilities:

- Run as a Windows service under a restricted account where possible.
- Poll sensors at a fixed interval, initially 1 Hz.
- Normalize telemetry into BenchScope's `SensorSnapshot` model.
- Own provider selection, caching, and diagnostics.
- Expose read-only snapshot IPC to the app.
- Handle driver installation state and version checks.
- Restart cleanly after sleep/resume.

Suggested service language:

- Rust if we want one systems language and strong Windows service integration.
- C#/.NET if we want easier reuse of existing LibreHardwareMonitor parsing logic for non-driver providers.
- C++ only if driver-adjacent shared headers and native Windows SDK ergonomics dominate.

Recommendation: service in Rust or C#; driver in C/KMDF.

### Sensor Driver

Responsibilities:

- Provide the minimal kernel access needed by the service.
- Expose read-only IOCTLs for specific safe telemetry operations.
- Validate every request.
- Refuse unsupported hardware clearly.
- Never expose arbitrary port, MSR, MMIO, or physical-memory access to user mode.
- Log errors through ETW or Windows Event Log.

Initial driver scope should be intentionally narrow:

- CPU package temperature for a small set of supported Intel/AMD families, if feasible.
- Optional SMBus/EC sensor reads only after a hardware support matrix exists.
- No fan control, voltage control, clock control, RGB, overclocking, or write paths.

## Driver Signing and Distribution

Shipping this safely requires Microsoft-compatible driver signing.

Known constraints:

- Modern 64-bit Windows requires kernel-mode drivers to be properly signed and trusted.
- Public distribution generally needs Microsoft signing through the Windows Hardware Dev Center path.
- WHQL or attestation signing may be viable depending on the driver type and target Windows versions.
- A vulnerable or overly broad driver can be blocked even if it is signed.

Plan:

1. Build a prototype driver only on development machines with test signing enabled.
2. Keep prototype builds out of normal BenchScope releases.
3. Once the IOCTL surface is narrow and reviewed, prepare a signing path:
   - Create/verify Microsoft Partner Center / Hardware Dev Center account.
   - Obtain required code-signing identity.
   - Package INF/CAT/SYS correctly.
   - Submit for attestation or WHQL as appropriate.
4. Publish only Microsoft-signed release drivers.
5. Add installer/uninstaller and rollback.

References:

- Microsoft kernel-mode signing requirements: https://learn.microsoft.com/en-us/windows-hardware/drivers/install/kernel-mode-code-signing-requirements--windows-vista-and-later-
- Microsoft driver signing requirements: https://learn.microsoft.com/en-gb/windows-hardware/drivers/dashboard/code-signing-reqs
- Microsoft vulnerable driver block rules: https://learn.microsoft.com/en-us/windows/security/application-security/application-control/windows-defender-application-control/design/microsoft-recommended-driver-block-rules
- Microsoft WinRing0 Defender alert: https://support.microsoft.com/en-gb/windows/microsoft-defender-antivirus-alert-vulnerabledriver-winnt-winring0-eb057830-d77b-41a2-9a34-015a5d203c42

## Security Model

Security posture:

- Read-only telemetry first.
- No arbitrary register/port/memory access.
- No network listener.
- Local IPC only.
- Authenticated local client access.
- Service accepts only fixed request types.
- Driver accepts only fixed IOCTLs with strict buffer validation.
- No kernel pointer disclosure.
- No privileged write operations in v1.
- Driver disabled/uninstalled cleanly if tampering or version mismatch is detected.

Threats to design against:

- Unprivileged local process abusing the driver to read/write privileged memory.
- Malicious app impersonating BenchScope over IPC.
- Service privilege escalation through malformed requests.
- Driver crash from unsupported motherboard/EC access.
- Sensor polling causing hangs on bad firmware.
- Defender or Windows Update blocking the driver after release.

Mitigations:

- Service ACL restricts IPC to the current interactive user and administrators.
- Driver device object ACL restricts access to the service SID, not arbitrary users.
- Fixed allowlist of operations.
- Hardware support allowlist.
- Timeouts and circuit breakers around slow/hanging providers.
- ETW logging and crash telemetry.
- Fuzz IOCTL request parsing.
- Static analysis and Driver Verifier in CI/manual gates.

## Hardware Support Strategy

Do not promise universal CPU/motherboard temperature coverage in v1.

Recommended order:

1. NVIDIA GPU temperature through NVML user-mode API.
2. AMD GPU temperature through official AMD ADLX/ADL SDK if licensing permits.
3. Intel GPU telemetry through official Intel APIs if available and redistributable.
4. Drive temperatures through Windows storage APIs.
5. CPU package temperatures through documented/vendor-supported paths where available.
6. Driver-backed motherboard/SMBus/EC sensors only after support matrix research.

For each supported sensor:

- Identify provider.
- Identify privilege requirement.
- Identify update interval.
- Identify expected units and valid range.
- Add diagnostics for unsupported hardware.

## IPC Snapshot Format

Use one compact read-only snapshot shape.

```json
{
  "version": 1,
  "timestampUtc": "2026-05-17T20:00:00Z",
  "service": {
    "version": "0.1.0",
    "elevated": true,
    "driverLoaded": true,
    "driverVersion": "0.1.0",
    "status": "ok"
  },
  "sensors": {
    "cpu": {
      "label": "CPU Package",
      "temperatureC": 61.4,
      "utilizationPercent": 18.2,
      "provider": "BenchScope Sensor Service",
      "status": "ok"
    },
    "gpu": {
      "label": "NVIDIA GPU",
      "temperatureC": 55.0,
      "utilizationPercent": 42.0,
      "provider": "NVML",
      "status": "ok"
    },
    "drive": {
      "label": "NVMe SSD",
      "temperatureC": 38.0,
      "utilizationPercent": 2.0,
      "provider": "Windows Storage",
      "status": "ok"
    }
  },
  "diagnostics": []
}
```

## Installer Flow

First-run flow:

1. BenchScope starts.
2. App detects missing service.
3. App shows a setup prompt:
   - `Install BenchScope sensor service`
   - `Continue without advanced sensors`
4. If accepted, launch signed installer with UAC.
5. Installer installs service and driver package.
6. Service starts.
7. App connects automatically.

Update flow:

- App detects service/driver version mismatch.
- Prompt for update.
- Stop service.
- Replace service binary.
- Update driver package if needed.
- Restart service.
- Roll back if startup fails.

Uninstall flow:

- Stop service.
- Delete service.
- Remove driver package.
- Remove local config/logs if requested.

## Development Phases

### Phase 0: Research and Feasibility

- Inventory safe vendor APIs:
  - NVML for NVIDIA.
  - AMD ADLX/ADL.
  - Intel GPU/Power Gadget alternatives.
  - Windows storage APIs.
- Determine which CPU package temperature paths require kernel access.
- Decide whether v1 can ship as service-only.
- Document hardware support matrix.

Acceptance:

- A table lists providers, supported hardware, license/redistribution status, privilege needs, and expected sensors.

### Phase 1: Service-Only Prototype

- Build a local sensor service.
- Expose snapshots over named pipe.
- Implement CPU/GPU utilization through Windows performance counters.
- Implement NVIDIA temperature through NVML if present.
- Implement storage temperature through Windows storage APIs.
- Integrate BenchScope app with service snapshots.

Acceptance:

- No driver required.
- Utilization works.
- NVIDIA GPU temperature works on NVIDIA systems with NVML.
- Service failure is non-fatal.

### Phase 2: Installer Prototype

- Create signed or development-signed service installer.
- Add install/update/uninstall commands.
- Add app first-run setup prompt.
- Persist service status diagnostics.

Acceptance:

- Fresh machine can install service through one UAC flow.
- BenchScope connects after install.

### Phase 3: Driver Feasibility Prototype

- Create a KMDF driver with no hardware access first.
- Implement device creation, service-only access control, ETW logging, and one harmless test IOCTL.
- Run Driver Verifier.
- Add CI/manual build instructions with WDK.

Acceptance:

- Driver installs only on test-signed development machine.
- Service can call test IOCTL.
- No release packaging yet.

### Phase 4: Narrow Hardware Driver Reads

- Add one read-only sensor class at a time.
- Start with the lowest-risk, best-documented path.
- Add hardware allowlist and timeout/circuit breaker.
- Refuse unsupported hardware.

Acceptance:

- Driver-backed temp reads work on one target test system.
- Unsupported systems fail safely.
- No arbitrary low-level access is exposed.

### Phase 5: Signing and Release Readiness

- Complete code review and threat model.
- Run static analysis, Driver Verifier, stress tests, sleep/resume tests.
- Submit driver for Microsoft signing path.
- Build release installer.
- Add release notes and uninstall instructions.

Acceptance:

- Driver is Microsoft-signed or otherwise accepted by target Windows versions.
- Defender does not flag the package.
- BenchScope installs/updates/uninstalls cleanly.

## Testing Plan

Unit tests:

- Snapshot parsing.
- IPC permission checks.
- Provider priority.
- Unsupported hardware diagnostics.
- Version mismatch handling.

Service tests:

- Start/stop/restart.
- Sleep/resume.
- Multiple BenchScope client connections.
- Service unavailable.
- Provider timeout.

Driver tests:

- Driver Verifier.
- IOCTL fuzzing.
- Buffer validation.
- Access control: normal user cannot open driver directly.
- Unsupported hardware returns clear error.
- Stress polling for several hours.

Manual hardware matrix:

- Intel CPU + NVIDIA GPU desktop.
- AMD CPU + NVIDIA GPU desktop.
- AMD APU/iGPU system.
- Intel laptop/iGPU system.
- NVMe and SATA storage.
- System with Secure Boot and Memory Integrity enabled.

## Risks

- Driver signing can take time and may require business/legal setup.
- CPU/motherboard sensors are vendor-specific and brittle.
- A bad kernel driver can crash or destabilize the machine.
- Windows security policy can block overly broad drivers.
- Some anti-cheat or EDR products may distrust hardware access drivers.
- Maintaining a driver is a long-term obligation.

## Safer Interim Path

Before committing to a driver, ship a service-only backend:

- Auto-elevated service.
- NVML/ADLX/vendor user-mode APIs.
- Windows performance counters.
- Windows storage APIs.
- Optional external WMI ingestion from user-run LibreHardwareMonitor/OpenHardwareMonitor.

This gives a seamless improvement for many users without taking on kernel-driver risk immediately.

## Open Questions

- Do we need CPU package temperature enough to justify a driver?
- Which hardware should v1 officially support?
- Are vendor SDK licenses compatible with redistribution?
- Is a service-only product acceptable for first release?
- Will this be distributed publicly or only used locally?
- Is Microsoft Partner Center / Hardware Dev Center access available?
- Should the driver be open-source, and if so, how do we handle security review?

## Done Definition

This project is complete when:

- BenchScope installs a trusted sensor service through one clear setup flow.
- The service provides CPU/GPU/SSD utilization and temperatures on the supported hardware matrix.
- Any kernel driver is Microsoft-signed, narrow, read-only, and not flagged by Defender.
- BenchScope never asks users to disable security features.
- Sensor failures are clear and benchmarks remain usable.
- The installer can update and uninstall the service/driver cleanly.
