# Target Directory Size Optimization Plan

## Implementation Status

Implemented:

- Added project-level Cargo configuration at `.cargo/config.toml`.
- Redirected Cargo build output to `../.cargo-target/BenchScope`.
- Added `[profile.dev]` settings in `Cargo.toml` to keep incremental builds enabled while reducing debug symbol size.
- Added `scripts/Clean-CargoCache.ps1` for explicit cleanup of the redirected Cargo cache.
- Removed the old in-repo `target` directory after confirming Cargo wrote new artifacts to the redirected cache.

Validation note:

- `cargo build` used the redirected cache path, but the full app build is currently blocked by existing `src/features/ai_training_benchmark` compile errors unrelated to this target-directory change.

## Objective

Keep the BenchScope repository directory small while preserving fast local Rust build times.

Current disk usage is dominated by generated build output:

- `target`: about 5.04 GiB
- `.nuget`: about 33 MiB
- tracked source files: about 1.23 MiB total

The goal is not to delete build caches on every run. That would save disk space but make normal development slower. The goal is to move and tune the generated artifacts so the repo folder stays clean while incremental builds remain fast.

## Key Diagnosis

Cargo writes compiled artifacts, debug symbols, dependency libraries, metadata, object files, and incremental compilation caches into `target` by default.

BenchScope has large graphics and Windows dependencies, including `wgpu`, `naga`, `egui`, and `windows`. These dependencies produce large debug artifacts on Windows, especially `.pdb`, `.rlib`, and `.rmeta` files.

The largest files are debug symbol databases:

```text
target/debug/BenchScope.pdb
target/debug/deps/BenchScope.pdb
target/debug/deps/*.pdb
```

This is expected for active Rust development on Windows, but it should not have to live inside the repo directory.

## Strategy

Use three complementary changes:

1. Redirect Cargo build output to a sibling cache folder outside the repo.
2. Reduce dev debug symbol size while keeping useful source-line debugging.
3. Add explicit cleanup commands for stale artifacts instead of cleaning automatically on every build.

This keeps the build cache warm while preventing `BenchScope/target` from becoming the largest folder in the project.

## Proposed Repository Changes

### 1. Add Project Cargo Configuration

Create:

```text
.cargo/config.toml
```

with:

```toml
[build]
target-dir = "../.cargo-target/BenchScope"
```

Expected result:

```text
C:\Users\thato\Documents\Projects\.cargo-target\BenchScope
```

instead of:

```text
C:\Users\thato\Documents\Projects\BenchScope\target
```

Benefits:

- Keeps generated build output outside the repo folder.
- Works automatically for normal `cargo build`, `cargo run`, `cargo test`, and editor-driven Cargo commands.
- Preserves incremental compilation caches for fast rebuilds.
- Avoids needing every shell session to set `CARGO_TARGET_DIR`.

Tradeoff:

- The build cache still exists on disk, just outside the repo.
- Anyone who clones the repo will also use this relative target directory unless they override it.

### 2. Add Dev Profile Settings

Update `Cargo.toml`:

```toml
[profile.dev]
debug = 1
incremental = true
```

Benefits:

- Keeps incremental development builds fast.
- Reduces debug symbol size compared with full debug info.
- Preserves basic line-level debugging and useful stack traces.

Optional stronger setting:

```toml
[profile.dev]
debug = "line-tables-only"
incremental = true
```

Use this only if `debug = 1` does not reduce `.pdb` size enough. It can make variable inspection in the debugger less useful.

### 3. Add a Cleanup Script

Create:

```text
scripts/Clean-CargoCache.ps1
```

Suggested behavior:

```powershell
cargo clean --target-dir ..\.cargo-target\BenchScope
```

Optional switches:

- `-ReleaseOnly`: clean only release artifacts.
- `-DebugOnly`: clean only debug artifacts.
- `-All`: clean the redirected target directory completely.

Benefits:

- Gives a one-command cleanup path.
- Avoids automatic cleanup during normal builds.
- Makes the policy obvious for future development.

### 4. Keep `.gitignore` Coverage

Current `.gitignore` already ignores:

```text
/target
.nuget/
.dotnet_home/
sensor-helper/bin/
sensor-helper/obj/
sensor-driver/BenchSco.*/
sensor-driver/x64/
```

Add an ignore entry only if the redirected cache is ever placed inside the repo:

```text
.cargo-target/
```

If the redirected cache remains at `../.cargo-target/BenchScope`, no `.gitignore` entry is required for that folder because it is outside the repository.

## Implementation Order

1. Create `.cargo/config.toml` with the redirected `target-dir`.
2. Add `[profile.dev]` settings to `Cargo.toml`.
3. Run `cargo build` and confirm Cargo writes to `..\.cargo-target\BenchScope` rather than `target`.
4. Run the app or test command normally to confirm behavior is unchanged.
5. Add `scripts/Clean-CargoCache.ps1`.
6. Run the cleanup script once only after confirming the redirected build works.
7. Remove the old in-repo `target` directory manually or with `cargo clean` after verifying there is no needed artifact inside it.

## Validation Checklist

- `cargo build` succeeds.
- `cargo run` launches BenchScope.
- `cargo test` succeeds or has the same result as before the change.
- A new `target` folder is not recreated inside the repo.
- The redirected cache folder contains the new build artifacts.
- Incremental rebuild after a small source edit remains fast.
- Debug stack traces still include useful file and line information.

## Rollback Plan

To restore default Cargo behavior:

1. Delete `.cargo/config.toml`, or remove the `[build] target-dir` entry.
2. Remove `[profile.dev]` changes from `Cargo.toml` if full debug symbols are needed.
3. Run `cargo build`.

Cargo will recreate the default in-repo `target` directory.

## Notes

Do not automatically run `cargo clean` before or after every build. That would remove the exact cache Cargo needs for fast rebuilds.

The recommended default is:

- Always redirect the target directory.
- Keep incremental builds enabled.
- Reduce debug symbols moderately with `debug = 1`.
- Clean manually after large dependency changes, release builds, or when disk space matters.
