# BenchScope First-Boot Links

BenchScope bundles project-owned binaries and redistributable runtime files when allowed. Hardware-specific drivers, developer tools, and test-signed driver flows are linked instead of silently installed.

## Runtime

- Microsoft Visual C++ 2015-2022 x64 Redistributable:
  - Download page: https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist?view=msvc-170
  - Latest x64 permalink: https://aka.ms/vs/17/release/vc_redist.x64.exe

## GPU And OEM Drivers

- NVIDIA drivers: https://www.nvidia.com/en-us/drivers/
- AMD drivers: https://www.amd.com/en/support/download/drivers.html
- Intel Driver & Support Assistant: https://www.intel.com/content/www/us/en/support/detect.html
- Prefer the PC or motherboard OEM support page when the system is a laptop, prebuilt desktop, or workstation with customized drivers.

## Optional Sensor Providers

- LibreHardwareMonitor releases: https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/releases
- OpenHardwareMonitor downloads: https://openhardwaremonitor.org/downloads/

These providers are optional and can rely on low-level drivers. BenchScope should never install them silently.

## Developer-Only Tools

- Rust installer: https://www.rust-lang.org/tools/install
- Visual Studio Build Tools: https://visualstudio.microsoft.com/downloads/
- Windows SDK: https://developer.microsoft.com/windows/downloads/windows-sdk/
- Windows Driver Kit: https://learn.microsoft.com/windows-hardware/drivers/download-the-wdk
- .NET 10 downloads: https://dotnet.microsoft.com/en-us/download/dotnet/10.0
- Windows Package Manager: https://learn.microsoft.com/windows/package-manager/winget/

These tools are for building BenchScope from source. They should not be required by a normal BenchScope installer.

## Driver Signing

- Driver signing overview: https://learn.microsoft.com/windows-hardware/drivers/install/driver-signing
- Driver code signing requirements: https://learn.microsoft.com/windows-hardware/drivers/dashboard/code-signing-reqs
- Test signing: https://learn.microsoft.com/windows-hardware/drivers/install/test-signing

BenchScope should not enable test-signing mode from the standard installer. Test-signing is a development flow that requires administrator rights and a reboot.
