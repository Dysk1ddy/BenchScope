# Drive Benchmark Tool Plan

## Goal

Add a second benchmark tool to the existing BenchScope application: a drive read/write speed tester similar in spirit to CrystalDiskMark.

The app should gain a new main menu that lets the user choose between:

- Matrix CPU/GPU Benchmark
- Drive Benchmark

Each tool view must include a clear Back button that returns to the main menu without closing the app.

## Language and Framework Choice

Keep the implementation in Rust using the current `eframe`/`egui` desktop UI.

Reasons:

- The app already uses Rust and `egui`.
- Rust is a good fit for low-level file I/O timing, aligned buffers, worker threads, cancellation, and Windows API access.
- The existing app already has progress reporting, background workers, cancel controls, and result tables that can be reused.
- The existing `windows` crate dependency can be expanded to use native Windows file APIs for accurate disk testing.

## User Experience

### Main Menu

On launch, show a compact main menu instead of opening directly into the matrix benchmark.

Menu layout:

- App title: `BenchScope`
- Tool buttons:
  - `Matrix CPU/GPU Benchmark`
  - `Drive Benchmark`
- Small status area:
  - App version
  - Last selected tool, if useful later

Selecting a tool switches the app into that tool view.

### Tool Navigation

Every tool screen must include:

- A Back button in the top bar.
- Current tool title.
- Running-test guard:
  - If no benchmark is running, Back returns immediately.
  - If a benchmark is running, Back should either cancel the benchmark or ask for confirmation before returning.

Recommended behavior:

- If a test is running, Back opens a small confirmation dialog:
  - `Cancel benchmark and return to main menu`
  - `Stay`
- Returning to the main menu should not leave worker threads running.

### Matrix Tool View

The current matrix benchmark UI becomes the `Matrix CPU/GPU Benchmark` view.

No major behavior changes are required beyond moving it behind the new menu and adding the Back control.

### Drive Benchmark View

The drive benchmark view should feel dense and practical, not like a landing page.

Controls:

- Drive/folder selection
  - Default to a safe user-writable folder.
  - Allow choosing a target folder later with a file/folder picker if added.
  - Show detected drive root, available free space, and filesystem if available.
- Test profile
  - Quick
  - Balanced
  - Thorough
- Test file size
  - Auto
  - 256 MiB
  - 512 MiB
  - 1 GiB
  - 4 GiB
  - 8 GiB
- Test selection checkboxes
  - Sequential read
  - Sequential write
  - Random 4 KiB read
  - Random 4 KiB write
- Queue/thread preset
  - Simple: `Q1T1`
  - Heavy: `Q8T1`
  - Optional later: `Q32T16`
- Run button
- Cancel button while running
- Progress bar for current test
- Overall progress bar for the full suite
- Log area for warnings and lifecycle events
- Results table taking most of the space

Results columns:

- Test
- Read MB/s
- Write MB/s
- IOPS
- Latency avg
- Latency p95
- Duration
- Test file size
- Mode
- Notes

## Benchmark Scope

The first implementation should support these tests:

1. Sequential write
2. Sequential read
3. Random 4 KiB write
4. Random 4 KiB read

Later expansion can add:

- Mixed read/write tests
- More queue-depth/thread-count combinations
- Latency histograms
- CSV export
- Drive health metadata
- Comparison history

## 30 Second Test Limit

Each individual test must have a hard maximum duration of 30 seconds.

Target duration should be shorter:

- Quick profile: 3-5 seconds per test
- Balanced profile: 8-12 seconds per test
- Thorough profile: 15-25 seconds per test

Hard rule:

- No single read or write subtest may run longer than 30 seconds.
- Cancellation must be checked frequently, ideally between every block batch.
- If the test hits the 30 second cap, stop cleanly and mark the result as capped.

## Accuracy Strategy

Disk benchmarks can accidentally measure the operating-system file cache instead of the real drive. The plan should avoid misleading results by using explicit test modes.

### Preferred Mode: Direct I/O

On Windows, use native file APIs through the `windows` crate:

- `CreateFileW`
- `ReadFile`
- `WriteFile`
- `SetFilePointerEx`
- `FlushFileBuffers`
- `GetDiskFreeSpaceW`
- `GetDiskFreeSpaceExW`

Open the benchmark file with flags similar to:

- `FILE_FLAG_NO_BUFFERING`
- `FILE_FLAG_WRITE_THROUGH`
- `FILE_FLAG_RANDOM_ACCESS` for random tests
- `FILE_FLAG_SEQUENTIAL_SCAN` for sequential tests where appropriate

Direct I/O requirements:

- Buffer pointer alignment must match the sector or allocation granularity requirement.
- Read/write sizes must be sector aligned.
- File offsets must be sector aligned.

Implementation detail:

- Add an aligned buffer helper for direct I/O.
- Query sector size for the selected target path.
- Use block sizes that are always aligned.

### Fallback Mode: Cached I/O

If direct I/O fails for a path or filesystem, fall back to standard cached file I/O.

The UI must label this clearly as cached mode.

Cached mode is still useful, but it may include OS cache effects. The result should show a note:

`Cached I/O: results may include RAM cache effects`

## Test File Handling

The benchmark should create a temporary file inside the selected target folder.

Recommended filename:

`benchscope_drive_benchmark.tmp`

Safety rules:

- Never overwrite an unrelated existing user file.
- If the benchmark file already exists, verify it looks like a previous benchmark file before reuse or deletion.
- Delete the file after the benchmark completes unless the user enables a future `keep test file` debug option.
- On cancellation or failure, attempt cleanup.
- If cleanup fails, show the path in the log.

Preallocation:

- Preallocate the test file to the chosen size before read tests.
- For write tests, write the whole target region with deterministic pseudo-random data.
- For read tests, ensure the file exists and has enough initialized data.

## Adaptive Sizing and Timing

The benchmark should adapt to fast and slow drives while staying below the 30 second cap.

### Auto File Size

For `Auto`, choose size based on free space:

- If free space < 2 GiB: use 256 MiB or warn.
- If free space is 2-8 GiB: use 512 MiB.
- If free space is 8-64 GiB: use 1 GiB.
- If free space > 64 GiB: use 4 GiB.

Never consume more than 10% of free space by default.

### Probe Pass

Before a full test, run a short probe:

- 256 MiB max for sequential tests
- 64 MiB max for random tests
- 500 ms to 1500 ms target probe duration

Use the probe to estimate how much data or how many operations can fit into the selected profile duration.

### Measured Pass

Run the measured pass using the probe estimate, but stop on whichever comes first:

- Target bytes or operations complete
- Profile duration reached
- 30 second hard cap reached
- User cancellation requested

This gives short tests on slow drives and enough work on fast drives without running forever.

## Sequential Tests

### Sequential Write

Default block size:

- 8 MiB blocks for direct I/O
- 8 MiB or larger for cached I/O

Process:

1. Open benchmark file in write mode.
2. Preallocate or set file length.
3. Write aligned blocks sequentially.
4. Flush at the end.
5. Report MB/s from bytes written divided by measured elapsed time.

Timing note:

- The write result should include final flush time by default.
- Later, an advanced option can split `write submit time` and `flush time`.

### Sequential Read

Default block size:

- 8 MiB blocks

Process:

1. Ensure benchmark file exists and is initialized.
2. Open in read mode.
3. Read aligned blocks sequentially.
4. Accumulate a checksum so the compiler cannot optimize away reads.
5. Report MB/s.

## Random 4 KiB Tests

Random tests should mimic CrystalDiskMark-style small-block behavior.

Default block size:

- 4 KiB

Access pattern:

- Generate a deterministic shuffled list of aligned offsets.
- Keep the same seed for read and write tests in one run.
- Wrap around if the test needs more operations than there are unique offsets.

Metrics:

- MB/s
- IOPS
- Average latency
- p95 latency

Latency collection:

- Store per-operation latency for shorter tests.
- For heavier queue/thread settings, store sampled latencies to avoid excessive memory use.

## Queue Depth and Threads

Initial implementation can be synchronous per worker thread.

Profiles:

- `Q1T1`: one worker, one operation at a time.
- `Q8T1`: one worker with overlapped I/O queue depth 8, if implemented.

Simpler first version:

- Implement `Q1T1` only for direct correctness.
- Add `Q8T1` once overlapped I/O infrastructure is ready.

Better first release if time allows:

- Use Windows overlapped I/O for queued random and sequential tests.
- Keep the UI profile simple even if the backend is more capable.

The plan should not fake queue depth. If async/overlapped I/O is not implemented yet, the UI should only expose modes that are actually measured.

## Cancellation

Drive tests must be cancelable.

Cancellation checks:

- Before opening files
- Before preallocation
- Between every large sequential block
- Between random operation batches
- Before flush
- After flush

For synchronous file I/O, cancellation may wait until the current OS call returns. To keep this responsive:

- Use moderate block sizes.
- Avoid single enormous reads/writes.
- Split random work into small batches.

If overlapped I/O is implemented later:

- Keep handles needed to call `CancelIoEx`.
- Cancel outstanding operations when the user presses Cancel or Back.

## Data Patterns

Use deterministic pseudo-random data instead of all-zero buffers.

Reasons:

- Avoids storage compression or sparse-file behavior skewing results.
- Keeps runs repeatable.
- Allows lightweight validation/checksum.

Implementation:

- Use a small fast PRNG implemented locally, such as xorshift or splitmix64.
- Fill aligned buffers with deterministic data based on seed, block index, and test type.
- Avoid adding a heavyweight random dependency unless needed.

## Safety and Wear

Write tests can stress SSDs and consume write endurance.

UI should show a concise warning near the Run button:

`Write tests create temporary data on the selected drive and may add SSD write wear.`

For very large file sizes or Thorough profile:

- Show a stronger confirmation if planned writes exceed a threshold, such as 16 GiB total.
- Show estimated total write amount before starting.

## Result Interpretation

The results area should clearly separate:

- Sequential throughput
- Random throughput
- Random IOPS
- Latency

Units:

- MB/s using decimal megabytes, matching common disk benchmark tools.
- IOPS as operations per second.
- Latency in microseconds or milliseconds depending on value.

Each result should include notes when relevant:

- Direct I/O
- Cached I/O
- Capped at 30s
- Canceled
- Flush included
- Low free space
- Could not delete temp file

## App Architecture Changes

### New App View Enum

Add a top-level app view enum:

```rust
enum AppView {
    MainMenu,
    MatrixBenchmark,
    DriveBenchmark,
}
```

`BenchScopeApp` should hold:

- Current `AppView`
- Existing matrix state
- New drive benchmark state

Recommended refactor:

- Move matrix-specific state into a `MatrixBenchmarkState`.
- Add `DriveBenchmarkState`.
- Keep shared app shell state at the top level.

This avoids the current app struct growing into an unmaintainable single-state object.

### New Drive Types

Suggested structs/enums:

```rust
enum DriveTestKind {
    SequentialRead,
    SequentialWrite,
    RandomRead4K,
    RandomWrite4K,
}

enum DriveIoMode {
    Direct,
    Cached,
}

enum DriveProfile {
    Quick,
    Balanced,
    Thorough,
}

struct DriveBenchmarkConfig {
    target_folder: PathBuf,
    file_size_bytes: u64,
    auto_file_size: bool,
    profile: DriveProfile,
    selected_tests: Vec<DriveTestKind>,
    io_mode_preference: DriveIoMode,
}

struct DriveBenchmarkResult {
    test: DriveTestKind,
    read_mbps: Option<f64>,
    write_mbps: Option<f64>,
    iops: Option<f64>,
    avg_latency_ms: Option<f64>,
    p95_latency_ms: Option<f64>,
    duration_ms: f64,
    file_size_bytes: u64,
    io_mode: DriveIoMode,
    notes: Vec<String>,
}
```

### Worker Events

Extend the existing worker event pattern or add a drive-specific channel.

Suggested events:

```rust
enum DriveWorkerEvent {
    Progress(DriveProgress),
    TestDone(DriveBenchmarkResult),
    SuiteDone(Result<Vec<DriveBenchmarkResult>, String>),
    Log(String),
}
```

Progress should include:

- Current test name
- Current test progress 0.0-1.0
- Suite progress 0.0-1.0
- Elapsed time
- Estimated remaining time
- Bytes processed or operations completed

## Implementation Phases

### Phase 1: Main Menu Refactor

Tasks:

- Add `AppView`.
- Make launch screen show the main menu.
- Move existing matrix UI behind `MatrixBenchmark`.
- Add Back button to matrix tool.
- Ensure matrix cancellation still works when leaving the tool.

Acceptance criteria:

- App opens to main menu.
- Matrix benchmark still works.
- Back returns to main menu.
- Back during a matrix benchmark does not leave work running.

### Phase 2: Drive Benchmark UI Skeleton

Tasks:

- Add `DriveBenchmarkState`.
- Add Drive Benchmark main panel.
- Add controls for target folder text field, profile, file size, test checkboxes, Run, Cancel.
- Add results grid and log area.
- Add Back button.

Acceptance criteria:

- User can enter or edit a target folder.
- User can select tests and profile.
- UI handles invalid folder paths gracefully.
- Back returns to main menu.

### Phase 3: Cached I/O Backend

Tasks:

- Implement safe benchmark file creation and cleanup.
- Implement sequential write/read using Rust standard file APIs.
- Implement random 4 KiB write/read using standard file APIs.
- Add timing, progress, cancellation, and result reporting.
- Clearly label results as cached I/O.

Acceptance criteria:

- All four tests run.
- Each test stops before 30 seconds.
- Cancel works during every test.
- Temp file is cleaned up.
- Results are clearly marked cached.

### Phase 4: Direct I/O Backend

Tasks:

- Add Windows direct I/O file wrapper.
- Query sector size.
- Add aligned buffer allocation.
- Implement direct sequential read/write.
- Implement direct random 4 KiB read/write.
- Fallback to cached mode when direct mode is unavailable.

Acceptance criteria:

- Direct I/O works on common NTFS drives.
- Misaligned paths or unsupported filesystems fall back cleanly.
- Results clearly show direct or cached mode.
- No misleading cache-only results are labeled as drive results.

### Phase 5: Adaptive Test Planner

Tasks:

- Add probe pass.
- Add auto file size selection.
- Add target duration per profile.
- Add 30 second hard cap.
- Add estimated write amount warning.

Acceptance criteria:

- Fast drives get enough work for stable results.
- Slow drives do not run too long.
- No individual test exceeds 30 seconds.
- UI shows capped tests in notes.

### Phase 6: Accuracy and Polish

Tasks:

- Add latency tracking for random tests.
- Add p95 latency.
- Add checksum/validation for reads.
- Improve result formatting.
- Add tests for planner logic and unit conversion.
- Add README documentation.

Acceptance criteria:

- Results are stable across repeated runs.
- Units are clear.
- Known cache/direct fallback cases are explained.
- Internal tests cover timing caps, file-size selection, and result calculations.

## Testing Plan

Automated tests:

- Auto file size selection.
- Profile duration planning.
- 30 second cap logic.
- MB/s and IOPS calculations.
- Latency percentile calculation.
- Random offset generation alignment.
- Benchmark temp filename safety.
- Cancellation flag propagation.

Manual tests:

- Run Quick profile on system drive.
- Run read-only tests.
- Run write tests and verify temp file cleanup.
- Cancel during sequential write.
- Cancel during random read.
- Press Back during an active run.
- Try invalid target folder.
- Try low free-space target.
- Compare broad results with CrystalDiskMark for sanity.

## Risks

### OS Cache Effects

Risk:

- Cached I/O can inflate read results by measuring RAM instead of disk.

Mitigation:

- Prefer direct I/O.
- Clearly label cached fallback.
- Use large enough files in Auto mode.

### SSD Write Wear

Risk:

- Repeated write benchmarks can add real writes to SSDs.

Mitigation:

- Default to Quick/Balanced.
- Show planned write amount.
- Warn for larger write totals.

### Direct I/O Complexity

Risk:

- Direct I/O requires strict alignment and can fail in surprising ways.

Mitigation:

- Encapsulate direct I/O in a small module.
- Add careful error messages.
- Fall back to cached mode with a visible note.

### Cancellation During OS Calls

Risk:

- A blocking read/write call may not stop instantly.

Mitigation:

- Use moderate block sizes.
- Check cancellation between batches.
- Add `CancelIoEx` later if overlapped I/O is implemented.

## Initial Default Settings

Recommended first release defaults:

- Profile: Quick
- File size: Auto
- Tests enabled:
  - Sequential read
  - Sequential write
  - Random 4 KiB read
  - Random 4 KiB write
- I/O mode: Direct preferred with cached fallback
- Sequential block size: 8 MiB
- Random block size: 4 KiB
- Queue/thread mode: Q1T1
- Delete temp file after run: enabled

## Definition of Done

The drive benchmark tool is ready when:

- App opens to a main menu.
- User can enter Matrix Benchmark and Drive Benchmark from the menu.
- Each tool has a working Back button to the main menu.
- Drive Benchmark can run sequential read/write and random 4 KiB read/write.
- Every individual drive test is capped at 30 seconds.
- Cancel works during drive tests.
- Results show MB/s and IOPS where applicable.
- UI clearly labels direct versus cached I/O.
- Temporary benchmark files are cleaned up.
- The existing matrix benchmark still works after the menu refactor.
