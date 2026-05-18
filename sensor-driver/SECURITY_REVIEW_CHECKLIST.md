# BenchScope Sensor Driver Security Review Checklist

Use this checklist before any attestation-signing submission and again before each signed driver update.

## Scope Decision

- [ ] Confirm a kernel driver is still required for the included readings.
- [ ] Confirm every available user-mode or vendor-supported provider has been preferred first.
- [ ] Confirm the release candidate contains only read-only telemetry operations.
- [ ] Confirm unsupported hardware returns a non-fatal unsupported status.
- [ ] Confirm the GUI and benchmarks continue without the driver.

## IOCTL Surface

- [ ] List every public IOCTL from `include/BenchScopeSensorIoctl.h`.
- [ ] Confirm each IOCTL uses `METHOD_BUFFERED`.
- [ ] Confirm each IOCTL uses `FILE_READ_DATA`, not `FILE_ANY_ACCESS`.
- [ ] Confirm no IOCTL accepts caller-controlled hardware addresses, register IDs, port IDs, MSR IDs, physical addresses, or buffer lengths that influence hardware access.
- [ ] Confirm output buffers are retrieved with exact minimum sizes.
- [ ] Confirm every returned structure is zeroed before fields are written.
- [ ] Confirm unknown IOCTLs complete with `STATUS_INVALID_DEVICE_REQUEST`.
- [ ] Confirm per-IOCTL access checks are explicit or documented.

Current intended IOCTLs:

- `IOCTL_BENCHSCOPE_SENSOR_GET_VERSION`
- `IOCTL_BENCHSCOPE_SENSOR_GET_CAPABILITIES`
- `IOCTL_BENCHSCOPE_SENSOR_GET_SNAPSHOT`
- `IOCTL_BENCHSCOPE_SENSOR_GET_ADVANCED_TELEMETRY`

## Device Access Control

- [ ] Confirm the control device uses a restrictive SDDL.
- [ ] Confirm access is limited to LocalSystem and built-in administrators unless a service-only SID is introduced.
- [ ] Confirm `FILE_DEVICE_SECURE_OPEN` is enabled.
- [ ] Confirm the service, not the GUI, owns normal driver access.
- [ ] Confirm non-admin users cannot open `\\.\BenchScopeSensor` directly.

## Hardware Access

- [ ] Confirm CPU telemetry is allowlisted by vendor/family/model/stepping as needed.
- [ ] Confirm each MSR read is fixed by the driver, not selected by user mode.
- [ ] Confirm MSR reads are wrapped with structured exception handling.
- [ ] Confirm temperatures, thermal limits, energy units, and wrap behavior are range-checked.
- [ ] Confirm no write MSRs or hardware control paths exist.
- [ ] Confirm no I/O port, SMBus, EC, MMIO, or physical-memory access is included unless separately reviewed and allowlisted.
- [ ] Confirm motherboard / Super I/O support remains disabled until a board/chip support matrix exists.

## Static Review

- [ ] Run `scripts\Test-SensorDriverSecuritySurface.ps1`.
- [ ] Search for forbidden primitives: `__writemsr`, `WRITE_PORT`, `READ_PORT`, `MmMapIoSpace`, `ZwMapViewOfSection`, `\\Device\\PhysicalMemory`, `METHOD_NEITHER`, `FILE_ANY_ACCESS`.
- [ ] Run Visual Studio code analysis for the Release x64 configuration.
- [ ] Run SDV or applicable WDK static checks when available.
- [ ] Run BinSkim or equivalent binary security analysis when available.
- [ ] Review compiler and linker warnings; fix every actionable warning.

## Runtime Validation

- [ ] Run Driver Verifier against `BenchScopeSensorDriver.sys`.
- [ ] Run `benchscope_sensor_probe.exe` against version, capabilities, snapshot, and advanced telemetry IOCTLs.
- [ ] Run `benchscope_sensor_service.exe --stream --interval-ms 1000` for at least two hours.
- [ ] Run a benchmark while streaming telemetry.
- [ ] Test sleep/resume while the service is running.
- [ ] Test service stop/start and driver stop/start behavior.
- [ ] Test unsupported CPU behavior.
- [ ] Test non-admin GUI behavior.
- [ ] Test clean uninstall and reinstall.

## Secure Boot And HVCI

- [ ] Validate the Microsoft-signed package on clean Windows 11 with Secure Boot enabled.
- [ ] Validate with Memory Integrity / HVCI enabled.
- [ ] Confirm no Code Integrity event reports signature error 577.
- [ ] Confirm the driver is not blocked by Microsoft vulnerable driver block rules.
- [ ] Confirm Defender or Smart App Control does not flag the package.

## Release Records

- [ ] Record source commit.
- [ ] Record `DriverVer`.
- [ ] Record driver protocol version.
- [ ] Record staged file SHA-256 hashes.
- [ ] Record CAB SHA-256 hash.
- [ ] Record EV signing certificate subject and thumbprint in private release notes.
- [ ] Record Microsoft Partner Center product ID and submission ID.
- [ ] Archive the Microsoft-signed package and validation logs.

## User-Facing Claims

- [ ] Do not claim WHQL, HLK, Windows Certification, or Windows Update distribution for attestation-signed packages.
- [ ] Describe the driver as Microsoft attestation-signed if that is the completed route.
- [ ] Document that sensor telemetry is optional and benchmarks remain valid without it.
- [ ] Provide a rollback and uninstall path.
