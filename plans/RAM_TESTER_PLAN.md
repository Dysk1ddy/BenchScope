# RAM Tester Plan

## Goal

Add a Memtest-style RAM tester to BenchScope that stresses data integrity, address-sensitive behavior, and intermittent stability while staying inside a hard duration budget of no more than 2 minutes per installed 8 GiB of RAM.

This is a user-mode tester. It cannot own every physical address the way a bootable Memtest86-style tool can, so it should be explicit about tested bytes versus installed bytes. It should allocate a large committed buffer, leave headroom for the OS, and run deterministic destructive tests against that buffer.

## Time Budget

- Compute budget from installed physical RAM: `installed_bytes / 8 GiB * 120 seconds`.
- Keep the default full run under that budget by using a fixed sequence of high-value passes and checking a deadline throughout.
- If the deadline is reached, stop after the current safe checkpoint and report the run as time-budget limited, not as a pass over untested phases.
- Default allocation should target most available RAM while leaving OS headroom:
  - about 70% of currently available physical RAM
  - never more than about 85% of installed RAM
  - leave at least 1 GiB free

## Memtest-Style Test Order

1. Allocation and boundary touch
   - Reserve and commit the selected buffer.
   - Touch first, middle, last, and per-page offsets.

2. Data bus sample
   - At several representative offsets, write walking 1s and walking 0s across 64-bit words.
   - Report exact expected, actual, diff, failed bit, offset, and test name.

3. Address alias sample
   - Use power-of-two offsets inside the buffer.
   - Write a sentinel everywhere, toggle one offset, and ensure other tested offsets did not alias.

4. Fixed-pattern moving inversions
   - Use 64-bit forms of classic patterns:
     - `0x0000000000000000`
     - `0xFFFFFFFFFFFFFFFF`
     - `0xAAAAAAAAAAAAAAAA`
     - `0x5555555555555555`
     - `0xCCCCCCCCCCCCCCCC`
     - `0x3333333333333333`
   - Fill, verify, invert, then reverse-verify where useful.

5. Own-address pattern
   - Store an address-derived value per word and verify it.
   - This catches decoder-like and index-sensitive faults visible through the virtual buffer.

6. Pseudo-random sequence
   - Generate values from a deterministic SplitMix64-derived sequence keyed by word index and seed.
   - Verify by recomputing, then run an inverted pass.

7. Modulo-stride pattern
   - Memtest-style modulo pass with selected phases, such as stride 20 phases 0, 7, and 13.
   - Write one pattern at matching offsets and inverse elsewhere, then verify.

8. Block move stress
   - Fill with address-derived values.
   - Move/copy chunks within the buffer and verify expected moved regions.
   - Keep chunk sizes bounded to avoid runaway copy cost.

## Reporting

Each failure record should include:

- test name and pass number
- byte offset and word index
- expected value
- actual value
- XOR diff
- first failed bit
- repeatability status when the same check is immediately retried

Run summary should include:

- tested bytes
- installed and available memory snapshot
- elapsed time and budget
- pass/fail/limited/canceled status
- number of checks and errors
- first failure details

## UI and CLI

- Add a main-menu RAM Tester entry.
- Run tests on a background worker with cancellation and progress events.
- Show current phase, progress, ETA, tested allocation, error count, and a compact result table.
- Add a CLI entry such as `--ram-test` for smoke/manual use.

## Safety Notes

- Do not claim this replaces bootable Memtest86 for firmware-level or physical-address coverage.
- Warn that the test intentionally allocates and writes a large memory buffer and may make the system sluggish.
- Avoid using swap/pagefile as a success path; size allocation from available physical memory and report if the request is reduced.
