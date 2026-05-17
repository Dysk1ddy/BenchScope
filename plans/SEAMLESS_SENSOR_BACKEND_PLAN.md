# Seamless Sensor Backend Plan

## Goal

Make BenchScope temperature sensors work by launching the app only. The user should not need to install HWMonitor, manually run a helper, open PowerShell, add `nvidia-smi` to PATH, or start BenchScope as Administrator by hand.

Target experience:

- User double-clicks `scripts\RUN_TESTER.bat` or `BenchScope.exe`.
- BenchScope initializes a bundled sensor backend automatically.
- CPU, GPU, and SSD temperature and utilization rows populate when the hardware exposes sensors.
- BenchScope requests elevated sensor access through a standard Windows UAC prompt when the app opens.
- If a sensor is still unavailable, the UI explains why in the sensor tooltip.

## Recommended Approach

Bundle a small Windows sensor helper executable with BenchScope.

Use LibreHardwareMonitorLib inside the helper because it can read the same broad class of sensors as HWMonitor-style tools:

- CPU package and core temperatures.
- NVIDIA, AMD, and Intel GPU temperatures where supported, including Intel/AMD iGPU readings exposed as CPU/APU `GT`, `Graphics`, `GFX`, or `iGPU` temperature sensors.
- CPU, GPU, and storage utilization where LibreHardwareMonitor or Windows performance counters expose load/activity data.
- NVMe/SATA drive SMART temperatures where supported.

BenchScope remains the Rust/egui app. The helper is a separate .NET executable that BenchScope starts and talks to over local JSON messages.

Why a helper instead of trying to do everything in Rust:

- LibreHardwareMonitor is a mature .NET library.
- It already handles many motherboard, CPU, GPU, and storage sensor paths.
- It avoids writing or bundling custom kernel drivers.
- It isolates Windows-specific hardware probing from the benchmark code.
- If the helper crashes or lacks permission, BenchScope can keep running.

## Project Layout

Add a helper project:

```text
sensor-helper/
  BenchScope.SensorHelper.csproj
  Program.cs
  SensorSnapshot.cs
  SensorMapper.cs
  app.manifest
```

Ship output beside the release executable:

```text
target/release/BenchScope.exe
target/release/BenchScope.SensorHelper.exe
target/release/LibreHardwareMonitorLib.dll
```

The release script or build instructions should copy the helper artifacts into `target/release`.

## Helper Responsibilities

The helper should:

- Initialize LibreHardwareMonitor.
- Enable CPU, GPU, motherboard, memory, controller, and storage scanning.
- Update sensors every 1 second.
- Emit compact JSON snapshots.
- Exit when BenchScope exits or closes the helper process.
- Avoid any UI of its own.

Helper output format, one JSON object per line:

```json
{
  "timestampUtc": "2026-05-15T21:30:00Z",
  "cpu": {
    "label": "CPU Package",
    "temperatureC": 63.5,
    "provider": "LibreHardwareMonitor",
    "status": "ok"
  },
  "gpu": {
    "label": "GPU Core",
    "temperatureC": 57.0,
    "provider": "LibreHardwareMonitor",
    "status": "ok"
  },
  "drive": {
    "label": "NVMe SSD",
    "temperatureC": 41.0,
    "provider": "LibreHardwareMonitor",
    "status": "ok"
  },
  "diagnostics": []
}
```

If a sensor is missing:

```json
{
  "cpu": {
    "label": "CPU",
    "temperatureC": null,
    "provider": "LibreHardwareMonitor",
    "status": "unsupported",
    "message": "No CPU temperature sensor found"
  }
}
```

## Elevation Flow

Some hardware sensors need administrator rights. The app should make this automatic and understandable.

Startup sequence:

1. BenchScope starts normally.
2. BenchScope launches the helper without elevation.
3. Helper reports whether sensor access is complete, partial, or blocked.
4. If all important sensors are blocked by permission, BenchScope shows a small prompt:

```text
Temperature sensors need permission
[Enable sensors] [Not now]
```

5. Clicking `Enable sensors` launches the helper elevated with `runas`, which triggers UAC.
6. After the elevated helper starts, BenchScope connects automatically.
7. If the user chooses `Not now`, BenchScope keeps running and shows `Permission needed` in the sensor box.

Important:

- BenchScope itself does not need to restart elevated.
- The benchmark app can remain unelevated.
- Only the helper is elevated.
- If UAC is canceled, the app should fall back to non-elevated readings.

## Helper Communication

Use stdin/stdout JSON lines first because it is simple and avoids firewall prompts.

Non-elevated helper:

- BenchScope starts helper with piped stdout.
- Helper writes one JSON snapshot per second.
- BenchScope reads snapshots on a background thread.

Elevated helper complication:

- `runas` does not preserve normal redirected stdout cleanly.
- For the elevated helper, publish snapshots to a temp JSON file that BenchScope polls.

Recommended IPC:

- Non-elevated mode can use stdout for simplicity.
- Elevated mode can use a per-process temp snapshot file:

```text
%TEMP%\BenchScope.SensorHelper-{pid}-{nonce}.json
```

BenchScope:

- Generates a random nonce.
- Starts helper with the temp snapshot path and parent process id.
- Reads the file when its modified time changes.

Helper:

- Writes snapshots every second through an atomic temp-file replace.
- Exits when the parent BenchScope process exits.

Security:

- Snapshot path includes BenchScope PID and nonce.
- Data is local telemetry only; no command input is accepted from the file.

## Rust Integration

Replace the current command-based sensor polling with a provider chain:

```text
SensorManager
  HelperProvider
  NvidiaSmiFallbackProvider
  WindowsStorageFallbackProvider
  AcpiFallbackProvider
```

Priority:

1. HelperProvider.
2. Existing lightweight fallbacks.

Behavior:

- Start helper provider immediately when the app opens.
- Keep polling at 1 Hz.
- Store latest helper snapshot in `SensorSnapshot`.
- Use fallback providers per sensor when the helper reading for that sensor is missing or unsupported.
- Keep all sensor errors non-fatal.

Suggested Rust additions:

```rust
struct HelperProvider {
    mode: HelperMode,
    child: Option<Child>,
    rx: Receiver<HelperSnapshot>,
    status: HelperStatus,
}

enum HelperMode {
    Stdout,
    NamedPipe,
}

enum HelperStatus {
    Starting,
    Running,
    NeedsElevation,
    Unavailable(String),
}
```

## Sensor Matching

CPU:

- Prefer sensors named:
  - `CPU Package`
  - `Core Max`
  - average/max of CPU core temperature sensors
- Avoid motherboard `Temperature #1` as CPU unless clearly associated with CPU.

GPU:

- Prefer the selected matrix benchmark adapter if matchable.
- Match by:
  - Vendor name.
  - Device name substring.
  - PCI bus/device ID if available later.
- If exact matching fails, use the first GPU temperature and label it `GPU (best effort)`.

Drive:

- Match the drive benchmark target.
- Use drive letter/root to map to disk model/serial when possible.
- Prefer storage sensors whose name or serial matches the selected drive.
- If matching fails, show the first storage temperature as `SSD (best effort)` only with a tooltip.

## UI Changes

Keep the sensor box as a separate in-layout UI box, not an overlay.

Add helper status in the tooltip:

```text
Provider: LibreHardwareMonitor
Helper: running elevated
Status: OK
```

If elevation is needed:

```text
Sensors
CPU  Permission
GPU  Permission
SSD  Permission
[Enable sensors]
```

Do not show sensors on the main menu.

Do not block benchmark controls while the helper starts.

## Packaging

For local development:

```powershell
dotnet publish sensor-helper -c Release -r win-x64 --self-contained false
cargo build --release
Copy-Item sensor-helper/bin/Release/net8.0-windows/win-x64/publish/* target/release/
```

For release:

- Include the helper executable and LibreHardwareMonitor DLL beside `BenchScope.exe`.
- Keep helper version in sync with BenchScope.
- Add a startup diagnostic if helper files are missing.

Optional later:

- Build helper self-contained for machines without .NET runtime.
- Add a small installer that places helper files next to BenchScope.

## Security and Trust

Because the helper may request elevation:

- Keep helper source in the repo.
- Keep helper behavior narrow: read sensors, emit JSON, exit.
- Do not accept arbitrary commands from BenchScope.
- Do not expose network ports.
- Use local named pipe only.
- Sign helper later if distributing outside local use.

## Failure Modes

Helper missing:

- Show fallback provider readings if available.
- Tooltip: `LibreHardwareMonitor helper not found`.

UAC canceled:

- Show `Permission needed`.
- Keep app running.

Helper crashes:

- Restart once after a short delay.
- If it crashes repeatedly, disable helper and show `Sensor helper crashed`.

No sensor found:

- Show `N/A`.
- Tooltip says which provider ran and what sensor type was missing.

## Testing Plan

Unit tests:

- Parse helper JSON snapshots.
- Convert helper sensor status into `SensorReading`.
- Pick CPU package over generic motherboard sensors.
- Pick selected drive over unrelated drive sensors.
- Preserve fallback providers when helper is unavailable.

Manual tests:

- Launch app normally.
- Confirm helper starts automatically.
- Confirm no sensor appears on the main menu.
- Open matrix benchmark view and confirm CPU/GPU readings update at 1 Hz.
- Open drive view and confirm SSD reading updates at 1 Hz.
- Deny UAC and confirm app keeps working.
- Accept UAC and confirm elevated helper readings replace fallback `N/A`.
- Run matrix benchmark and confirm start/end/max temperatures are recorded.
- Run drive benchmark and confirm SSD start/end/max temperatures are recorded.

## Implementation Phases

### Phase 1: Helper Prototype

- Add .NET helper project.
- Reference LibreHardwareMonitorLib.
- Print JSON snapshots once per second.
- Test helper standalone from PowerShell.

Acceptance:

- Helper prints CPU/GPU/SSD temps on the target machine when run as Administrator.

### Phase 2: Non-Elevated Helper Integration

- Launch helper automatically from BenchScope.
- Read stdout JSON snapshots.
- Feed `SensorSnapshot`.
- Keep existing fallback providers.

Acceptance:

- Opening BenchScope starts helper automatically.
- Sensor box uses helper data when available.

### Phase 3: UAC Helper Flow

- Detect permission-blocked helper status.
- Relaunch helper elevated automatically through a Windows UAC prompt.
- Read elevated helper snapshots from the temp snapshot file.

Acceptance:

- User only opens BenchScope and approves the standard Windows UAC prompt.
- No manual terminal or setup required.

### Phase 4: Sensor Matching

- Improve CPU, GPU, and drive selection.
- Match drive temperature to selected drive benchmark target.
- Add best-effort labels and tooltips.

Acceptance:

- Readings correspond to the hardware being benchmarked, with Intel/AMD iGPU rows using shared CPU package temperature when no separate iGPU temperature sensor exists.

### Phase 5: Packaging

- Update build scripts/docs to publish helper and copy artifacts.
- Add missing-helper diagnostics.
- Rebuild release package.

Acceptance:

- A fresh release folder works by opening `BenchScope.exe`.

## Done Definition

The seamless sensor fix is complete when:

- BenchScope starts a bundled helper automatically.
- The user does not need to install or launch another app.
- CPU/GPU/SSD temperatures populate on supported hardware.
- The app asks for elevated sensor access on launch through one Windows UAC flow.
- Sensor failure states are clear and non-fatal.
- Benchmarks still run when sensors are unavailable.
- Temperature summaries continue to appear in logs/result tables when readings exist.
