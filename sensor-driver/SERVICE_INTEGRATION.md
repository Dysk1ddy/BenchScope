# Sensor Service Integration Notes

The BenchScope GUI should not open the kernel driver directly in the final architecture. A local Windows service should own the driver handle and expose normalized snapshots to the app.

## Current Bridge

The first user-mode bridge is `benchscope_sensor_service`.

It is not a permanent Windows service yet. It is a small process that owns `\\.\BenchScopeSensor` access, calls the driver IOCTLs, and emits one JSON snapshot per line. BenchScope now looks for this executable next to the app or in `target/debug` / `target/release`, starts it with `--stream`, parses the snapshots, and falls back to safe Windows probes for values the driver does not provide yet.

The bridge also fills safe user-mode values before emitting each snapshot:

- CPU utilization through native Windows system-time deltas.
- GPU utilization through Windows GPU Engine counters.
- NVIDIA GPU temperature through `nvidia-smi` when present.
- Drive temperature through Windows Storage reliability counters.
- RAM utilization through Windows memory status.

Streaming is tuned for the GUI path:

- The bridge keeps the driver handle open instead of reopening `\\.\BenchScopeSensor` on every poll.
- Driver version and capability responses are cached for the process lifetime.
- Fast utilization providers are refreshed at roughly the UI polling rate.
- NVIDIA GPU temperature is cached for 5 seconds.
- Drive temperature is cached for 15 seconds because storage reliability queries can be slow.
- Provider commands have timeouts so a stuck WMI/PowerShell call cannot stall the sensor stream indefinitely.

Useful commands:

```powershell
cargo build --bin benchscope_sensor_service --bin benchscope_sensor_probe
target\debug\benchscope_sensor_service.exe
target\debug\benchscope_sensor_service.exe --stream --interval-ms 1000
target\debug\benchscope_sensor_probe.exe
```

Because the driver device ACL is restricted to LocalSystem and built-in administrators, run the bridge/probe elevated until the real Windows service is added.

## Device Path

The prototype driver creates:

```text
\\.\BenchScopeSensor
```

The shared IOCTL contract lives in:

```text
sensor-driver/include/BenchScopeSensorIoctl.h
```

## Initial Service Startup Flow

1. Service starts.
2. Service opens `\\.\BenchScopeSensor`.
3. Service calls `IOCTL_BENCHSCOPE_SENSOR_GET_VERSION`.
4. Service verifies `protocolVersion == BENCHSCOPE_SENSOR_PROTOCOL_VERSION`.
5. Service calls `IOCTL_BENCHSCOPE_SENSOR_GET_CAPABILITIES`.
6. Service calls `IOCTL_BENCHSCOPE_SENSOR_GET_ADVANCED_TELEMETRY` when the protocol version supports it.
7. Service derives CPU package power from package energy deltas when available.
8. Service falls back to user-mode providers for anything unsupported by the driver.
9. Service publishes merged snapshots to BenchScope over a local named pipe.

## Read-Only Driver Call Pattern

User-mode service pseudocode:

```c
HANDLE driver = CreateFileW(
    L"\\\\.\\BenchScopeSensor",
    GENERIC_READ,
    FILE_SHARE_READ,
    NULL,
    OPEN_EXISTING,
    FILE_ATTRIBUTE_NORMAL,
    NULL);

BENCHSCOPE_SENSOR_VERSION version = {0};
DWORD bytes = 0;
DeviceIoControl(
    driver,
    IOCTL_BENCHSCOPE_SENSOR_GET_VERSION,
    NULL,
    0,
    &version,
    sizeof(version),
    &bytes,
    NULL);
```

## Security Expectations

- The driver ACL currently allows LocalSystem and built-in administrators.
- The final service should run as LocalSystem or a dedicated service identity.
- The final driver ACL should be narrowed to the service SID where practical.
- BenchScope should communicate with the service, not the driver.
- No IOCTL should ever provide arbitrary raw hardware access.

## Snapshot Semantics

`IOCTL_BENCHSCOPE_SENSOR_GET_SNAPSHOT` returns a fixed-size `BENCHSCOPE_SENSOR_SNAPSHOT`.

Unsupported readings return `BenchScopeSensorStatusUnsupported`. Real sensor providers should be added one at a time behind reviewed capability flags.

The current driver can report Intel family 6 CPU package temperature and package energy on systems where the allowlisted MSRs are present. Motherboard / Super I/O support remains disabled until a chip and board allowlist exists. NVMe / storage health should remain a user-mode service provider unless a future storage-driver need is proven.

Unit conversion:

- `temperatureMilliC`: 61.25 C is `61250`.
- `utilizationMilliPercent`: 42.5% is `42500`.
- Flags determine whether each value is present.
