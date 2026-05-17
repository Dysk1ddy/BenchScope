# BenchScope Third-Party Notices

This file is a staging placeholder for release-bundle notices.

## Microsoft Visual C++ Runtime

If `vc_redist.x64.exe` or app-local Microsoft Visual C++ runtime DLLs are included in a release bundle, confirm the current Microsoft redistribution terms and add the required notice text here before publishing.

Official redistributable page: https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist?view=msvc-170

## Rust Dependencies

BenchScope is built from the Rust dependencies declared in `Cargo.toml` and locked in `Cargo.lock`. A release process should generate a dependency/license inventory from the lockfile before public distribution.

## Optional Sensor Providers

LibreHardwareMonitor and OpenHardwareMonitor are not bundled by default. If a future opt-in helper package includes either project or its libraries, add license and attribution details here before publishing.
