# Advanced Telemetry Provider Plan

## Goal

Add richer BenchScope telemetry through three focused provider tracks:

- Read-only CPU telemetry driver.
- Motherboard / Super I/O sensor driver.
- NVMe / storage health provider.

These providers should improve benchmark context without making benchmarks depend on telemetry availability. Every provider must fail safely, report clear diagnostics, and keep BenchScope usable when hardware is unsupported.

## Product Principles

- Prefer user-mode providers before kernel drivers.
- Keep all first versions read-only.
- Do not expose arbitrary MSR, I/O port, SMBus, EC, MMIO, or physical-memory access to user mode.
- Add one hardware class at a time behind explicit capability flags.
- Require hardware allowlists for low-level motherboard access.
- Keep the BenchScope GUI out of privileged hardware access. The GUI should talk to the BenchScope Sensor Service.
- Keep provider failures non-fatal and visible through status text/tooltips.
- Do not ask users to disable Secure Boot, Memory Integrity, Defender protections, or vulnerable-driver block rules.

## Shared Architecture

```text
BenchScope.exe
  |
  | local read-only IPC
  v
BenchScope Sensor Service
  |
  | user-mode providers
  | - Windows performance counters
  | - Windows storage APIs
  | - NVML / vendor SDKs
  |
  | optional narrow IOCTLs
  v
BenchScope kernel drivers
  |
  | read-only, model-aware telemetry operations only
```

The service should merge provider results into the existing `SensorSnapshot` shape and include provider diagnostics. Kernel drivers should report capabilities, version, supported hardware IDs, and per-reading status.

## Phase Order

1. Build the Sensor Service bridge and app integration.
2. Implement the NVMe / storage health provider in user mode.
3. Add CPU telemetry research and a narrow proof-of-concept driver for one supported CPU family.
4. Add motherboard / Super I/O support only after a chip support matrix and timeout strategy exist.
5. Package, sign, and harden only the providers that pass reliability and security gates.

This order gives BenchScope useful improvements quickly while delaying the riskiest kernel work until the architecture is ready.

## Workstream 1: Read-Only CPU Telemetry Driver

### Goal

Expose CPU package temperature, thermal status, and possibly package power / energy for known CPU models using fixed read-only operations.

### Candidate Readings

- CPU package temperature.
- Core/package thermal throttling status.
- Critical temperature / PROCHOT style status when safely available.
- Package energy counter.
- Estimated package power derived from energy deltas in the service.
- CPU model, family, stepping, and provider capability metadata.

### Design

- Add CPU-specific capability flags to the driver/service contract.
- Detect CPU vendor, family, model, and stepping before enabling any read path.
- Use a strict allowlist for supported CPU models and register layouts.
- Return `Unsupported` for unknown CPUs.
- Return raw values only after unit conversion and range checks in kernel or service.
- Calculate power over time in the service rather than the driver when possible.
- Keep IOCTLs fixed-shape and output-only for initial versions.

### Non-Goals

- No arbitrary MSR read/write IOCTL.
- No voltage, multiplier, clock, overclock, undervolt, or power-limit writes.
- No kernel worker that continuously polls without service control.
- No attempt to support every CPU family in the first version.

### Implementation Steps

1. Research supported Intel and AMD telemetry paths and document register semantics, units, wrap behavior, and privilege requirements.
2. Add CPU telemetry fields to the service snapshot model.
3. Extend the kernel IOCTL contract with CPU capability metadata or a versioned extended snapshot.
4. Implement a no-hardware CPU provider stub that reports the detected CPU model and unsupported status.
5. Add one real read path for one target CPU family.
6. Add service-side smoothing, delta calculation for energy/power, stale detection, and diagnostics.
7. Integrate readings into the existing sensor panel and benchmark temperature summaries.

### Acceptance Criteria

- Unsupported CPUs return clear unsupported diagnostics.
- Supported CPU package temperature updates at approximately 1 Hz.
- Thermal status is reported without blocking the UI.
- Power/energy readings are monotonic or clearly reset on wrap/sleep.
- Benchmarks continue if the CPU driver is missing, stopped, or unsupported.

### Tests

- IOCTL buffer validation and fuzzing.
- Driver Verifier on the CPU driver.
- Supported CPU smoke test under idle and benchmark load.
- Sleep/resume test.
- Multi-hour polling stability test.
- Service downgrade path when the driver is unavailable.

## Workstream 2: Motherboard / Super I/O Sensor Driver

### Goal

Read fan RPM, motherboard temperatures, and voltages from specific whitelisted Super I/O or embedded controller chips.

This is high value because it can reveal cooling and board-level issues, but it is also the most fragile provider track.

### Candidate Readings

- CPU fan RPM.
- Case/system fan RPM.
- Motherboard / VRM / chipset temperatures.
- Basic voltage rails where labels are known.
- Chip identity, revision, and board/vendor metadata.

### Design

- Treat this as a separate provider from CPU telemetry.
- Require explicit chip allowlists and board-specific label maps.
- Add a provider circuit breaker for slow, hanging, or inconsistent firmware paths.
- Poll conservatively, usually 1 Hz or slower.
- Prefer known Super I/O chips with stable register maps before embedded controller access.
- Keep raw register details inside the driver/service, never exposed to the GUI.

### Non-Goals

- No fan speed control in the first version.
- No RGB, voltage control, EC writes, fan curve writes, or board tuning.
- No broad `read port` / `write port` IOCTLs.
- No blind scanning of I/O ports on production builds.
- No EC access without model-specific research and timeout handling.

### Implementation Steps

1. Create a motherboard sensor support matrix with chip, board, register map source, expected labels, and risk notes.
2. Add service schema support for fan RPM, voltage, and board temperature readings.
3. Implement a simulated provider with fixed sample data for UI and report work.
4. Add detection-only driver capability for a single Super I/O family.
5. Add read-only RPM/temperature/voltage reads for one whitelisted chip.
6. Add label mapping in the service so values are human-readable.
7. Add diagnostics for unsupported chip, unsupported board, stale value, and circuit-breaker disabled.

### Acceptance Criteria

- Unknown chips and boards are refused by default.
- Supported board readings update without UI stalls.
- Fan, board temperature, and voltage values have labels and sane ranges.
- Provider disables itself after repeated timeout/error conditions.
- No write IOCTLs exist in the production path.

### Tests

- Detection-only tests on unsupported systems.
- Driver Verifier and IOCTL fuzzing.
- Long polling test with benchmark load.
- Sleep/resume and shutdown tests.
- Negative tests for unsupported chip IDs.
- Manual validation against BIOS or a trusted monitoring tool on the target board.

## Workstream 3: NVMe / Storage Health Provider

### Goal

Add richer SMART/NVMe health telemetry: wear, media errors, unsafe shutdowns, temperature sensors, lifetime counters, and thermal history.

This should start as a user-mode service provider using Windows storage APIs. A kernel driver should not be needed for the first version.

### Candidate Readings

- NVMe percentage used / wear estimate.
- Available spare and spare threshold.
- Critical warning flags.
- Composite temperature.
- Additional NVMe temperature sensors when exposed.
- Data units read/written.
- Host reads/writes.
- Controller busy time.
- Power cycles and power-on hours.
- Unsafe shutdown count.
- Media/data integrity error count.
- Error information log entries.
- Thermal management event counts.
- Warning and critical composite temperature time.

### Design

- Resolve the selected BenchScope target path to volume and physical disk identity.
- Query storage health in the Sensor Service, not the GUI.
- Prefer documented Windows storage protocol query APIs.
- Normalize NVMe and SATA health into a common storage health model.
- Preserve vendor-specific values as diagnostics only when meaning is clear.
- Cache drive identity and refresh when target path changes.
- Continue using the existing storage health UI as the first consumer.

### Non-Goals

- No destructive drive tests.
- No firmware update, sanitize, secure erase, format, or vendor maintenance commands.
- No kernel storage filter driver in the initial version.
- No raw pass-through command UI.

### Implementation Steps

1. Extend the storage health model with NVMe log-page fields and source metadata.
2. Move richer drive probing into the Sensor Service or a service-owned provider module.
3. Resolve drive letter/path to physical disk and stable identity.
4. Query NVMe SMART / health information through Windows storage APIs.
5. Add SATA/SCSI fallbacks where Windows exposes equivalent data.
6. Add result severity mapping: ok, caution, warning, critical, unknown.
7. Show richer storage findings in the SSD / HDD Health Checker and Markdown export.

### Acceptance Criteria

- NVMe health data appears for supported NVMe drives without a kernel driver.
- Unsupported drives keep the existing health checker usable.
- Selected benchmark target maps to the correct physical disk when possible.
- Reports include wear, errors, unsafe shutdowns, thermal warnings, and temperature fields when available.
- Repeated scans do not require administrator prompts beyond service setup.

### Tests

- Unit tests for log parsing and severity mapping.
- Mocked storage API responses for healthy, worn, overheated, and failing drives.
- Multi-drive mapping tests.
- USB enclosure / unsupported bridge behavior test.
- Markdown export regression test.
- Manual comparison against Windows `Get-PhysicalDisk` / reliability counters and vendor tools.

## Service Contract Changes

Add a versioned extended telemetry snapshot rather than forcing every new reading into the current four-row temperature model.

Suggested categories:

- `thermal`: CPU/GPU/drive/board temperatures.
- `utilization`: CPU/GPU/drive/RAM activity.
- `fans`: fan RPM readings.
- `voltages`: board voltage rails.
- `power`: CPU package power/energy where available.
- `storageHealth`: SMART/NVMe/SATA health counters.
- `diagnostics`: provider status, unsupported reasons, stale flags, and permissions.

Every reading should include:

- Stable kind/category.
- Label.
- Numeric value and unit.
- Provider.
- Status.
- Timestamp.
- Optional source identity, such as CPU model, board chip ID, or drive serial.

## UI Integration

1. Keep the current compact sensor panel for CPU/GPU/SSD temperature and utilization.
2. Add richer details to tooltips first.
3. Expand the SSD / HDD Health Checker with NVMe fields and findings.
4. Add an optional "Telemetry Details" view later for fans, voltages, board temps, and power.
5. Add provider diagnostics so users can tell whether data came from Windows, vendor APIs, service, or driver.

## Security Gates

Before installing or shipping any kernel provider:

- IOCTL surface reviewed for arbitrary access risks.
- Device ACL limited to the service identity where practical.
- Driver Verifier run documented.
- Static analysis clean enough for release consideration.
- Unsupported hardware path tested.
- Sleep/resume path tested.
- Signing and uninstall plan documented.
- No write/control paths in v1.

## Recommended First Milestone

Build the NVMe / storage health provider first because it is likely user-mode, valuable to the existing app, and lower risk than kernel sensor access.

After that, implement the Sensor Service bridge and a CPU telemetry driver stub that proves versioning, capabilities, and unsupported diagnostics. Only then add one real CPU read path.

Motherboard / Super I/O support should wait until there is a specific target board and chip to support.
