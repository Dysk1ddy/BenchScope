param(
    [string]$BundlePath = "",
    [string]$OutputPath = "",
    [ValidateSet("Markdown", "Json")]
    [string]$Format = "Markdown",
    [switch]$Quiet,
    [switch]$FailOnMissingRequired
)

$ErrorActionPreference = "Stop"

$checks = New-Object System.Collections.Generic.List[object]

function Add-Check {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet("Pass", "Warning", "Fail", "Info")]
        [string]$Status,
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [Parameter(Mandatory = $true)]
        [string]$Detail,
        [string[]]$Links = @()
    )

    $checks.Add([pscustomobject]@{
        Status = $Status
        Name = $Name
        Detail = $Detail
        Links = @($Links)
    }) | Out-Null
}

function Test-CommandPresent {
    param([Parameter(Mandatory = $true)][string]$Name)
    return $null -ne (Get-Command $Name -ErrorAction SilentlyContinue)
}

function Test-RootWmiNamespace {
    param([Parameter(Mandatory = $true)][string]$Name)

    try {
        $namespaces = @(Get-CimInstance -Namespace root -ClassName __Namespace -ErrorAction Stop)
        return $null -ne ($namespaces | Where-Object { $_.Name -eq $Name } | Select-Object -First 1)
    } catch {
        return $false
    }
}

function Get-VcRuntimeState {
    $keys = @(
        "HKLM:\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64",
        "HKLM:\SOFTWARE\WOW6432Node\Microsoft\VisualStudio\14.0\VC\Runtimes\x64"
    )

    foreach ($key in $keys) {
        if (Test-Path -LiteralPath $key) {
            $props = Get-ItemProperty -LiteralPath $key
            if ($props.Installed -eq 1) {
                $version = $props.Version
                if (-not $version) {
                    $version = "$($props.Major).$($props.Minor).$($props.Bld).$($props.Rbld)"
                }
                return [pscustomobject]@{
                    Found = $true
                    Detail = "Installed runtime registry key found at $key; version $version."
                }
            }
        }
    }

    $dllPath = Join-Path $env:WINDIR "System32\VCRUNTIME140.dll"
    if (Test-Path -LiteralPath $dllPath) {
        return [pscustomobject]@{
            Found = $true
            Detail = "Registry key was not confirmed, but $dllPath exists."
        }
    }

    return [pscustomobject]@{
        Found = $false
        Detail = "Microsoft Visual C++ 2015-2022 x64 runtime was not detected."
    }
}

function Convert-Cell {
    param([AllowNull()][object]$Value)

    if ($null -eq $Value) {
        return ""
    }

    return ([string]$Value).Replace("|", "\|").Replace("`r`n", "<br>").Replace("`n", "<br>")
}

function Convert-ChecksToMarkdown {
    param([Parameter(Mandatory = $true)][object[]]$Items)

    $lines = New-Object System.Collections.Generic.List[string]
    $lines.Add("# BenchScope First-Boot Prerequisite Report") | Out-Null
    $lines.Add("") | Out-Null
    $lines.Add("Generated: $((Get-Date).ToString("s"))") | Out-Null
    $lines.Add("") | Out-Null
    $lines.Add("| Status | Check | Detail | Links |") | Out-Null
    $lines.Add("| --- | --- | --- | --- |") | Out-Null

    foreach ($item in $Items) {
        $links = @($item.Links)
        $linkText = ""
        if ($links.Count -gt 0) {
            $linkText = ($links -join "<br>")
        }

        $lines.Add("| $(Convert-Cell $item.Status) | $(Convert-Cell $item.Name) | $(Convert-Cell $item.Detail) | $(Convert-Cell $linkText) |") | Out-Null
    }

    return ($lines -join [Environment]::NewLine)
}

function Resolve-InputPath {
    param([Parameter(Mandatory = $true)][string]$Path)

    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }

    return [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Path))
}

try {
    $os = Get-CimInstance Win32_OperatingSystem -ErrorAction Stop
    if ([Environment]::Is64BitOperatingSystem) {
        Add-Check -Status Pass -Name "Windows x64" -Detail "$($os.Caption) $($os.Version), 64-bit."
    } else {
        Add-Check -Status Fail -Name "Windows x64" -Detail "$($os.Caption) $($os.Version), but the operating system is not 64-bit." -Links @("https://www.microsoft.com/software-download/windows11")
    }
} catch {
    if ([Environment]::OSVersion.Platform -eq [PlatformID]::Win32NT -and [Environment]::Is64BitOperatingSystem) {
        Add-Check -Status Pass -Name "Windows x64" -Detail "Windows 64-bit detected through .NET environment fallback; Win32_OperatingSystem query failed: $($_.Exception.Message)"
    } elseif ([Environment]::OSVersion.Platform -eq [PlatformID]::Win32NT) {
        Add-Check -Status Fail -Name "Windows x64" -Detail "Windows was detected, but the operating system is not 64-bit. Win32_OperatingSystem query also failed: $($_.Exception.Message)" -Links @("https://www.microsoft.com/software-download/windows11")
    } else {
        Add-Check -Status Fail -Name "Windows x64" -Detail "This does not appear to be Windows. Win32_OperatingSystem query failed: $($_.Exception.Message)"
    }
}

try {
    $principal = [Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
    if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        Add-Check -Status Pass -Name "Administrator permission" -Detail "Current process is elevated."
    } else {
        Add-Check -Status Warning -Name "Administrator permission" -Detail "Current process is not elevated. BenchScope can launch, but full hardware and storage diagnostics may request elevation."
    }
} catch {
    Add-Check -Status Warning -Name "Administrator permission" -Detail "Unable to determine elevation state: $($_.Exception.Message)"
}

$vcRuntime = Get-VcRuntimeState
if ($vcRuntime.Found) {
    Add-Check -Status Pass -Name "VC++ x64 runtime" -Detail $vcRuntime.Detail
} else {
    Add-Check -Status Fail -Name "VC++ x64 runtime" -Detail $vcRuntime.Detail -Links @("https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist?view=msvc-170", "https://aka.ms/vs/17/release/vc_redist.x64.exe")
}

$requiredCommands = @("powershell.exe", "powercfg.exe", "ping.exe", "netsh.exe")
$missingCommands = @($requiredCommands | Where-Object { -not (Test-CommandPresent $_) })
if ($missingCommands.Count -eq 0) {
    Add-Check -Status Pass -Name "Windows command tools" -Detail "Found $($requiredCommands -join ", ")."
} else {
    Add-Check -Status Fail -Name "Windows command tools" -Detail "Missing $($missingCommands -join ", ")."
}

$requiredCmdlets = @("Get-CimInstance", "Get-PhysicalDisk", "Get-StorageReliabilityCounter", "Get-NetAdapter", "Get-NetIPConfiguration")
$missingCmdlets = @($requiredCmdlets | Where-Object { -not (Test-CommandPresent $_) })
if ($missingCmdlets.Count -eq 0) {
    Add-Check -Status Pass -Name "Windows PowerShell providers" -Detail "Found CIM, storage, and network cmdlets used by BenchScope diagnostics."
} else {
    Add-Check -Status Warning -Name "Windows PowerShell providers" -Detail "Missing cmdlets: $($missingCmdlets -join ", "). Some diagnostics may degrade."
}

try {
    $videoControllers = @(Get-CimInstance Win32_VideoController -ErrorAction Stop)
    if ($videoControllers.Count -eq 0) {
        Add-Check -Status Warning -Name "GPU driver" -Detail "No Win32_VideoController entries were returned."
    } else {
        $gpuNames = @($videoControllers | ForEach-Object { $_.Name }) -join "; "
        $basicAdapters = @($videoControllers | Where-Object { $_.Name -match "Microsoft Basic Display|Software Adapter" })
        if ($basicAdapters.Count -gt 0) {
            Add-Check -Status Warning -Name "GPU driver" -Detail "Detected possible fallback/software adapter: $gpuNames." -Links @("https://www.nvidia.com/en-us/drivers/", "https://www.amd.com/en/support/download/drivers.html", "https://www.intel.com/content/www/us/en/support/detect.html")
        } else {
            Add-Check -Status Pass -Name "GPU driver" -Detail "Detected video controllers: $gpuNames."
        }
    }
} catch {
    Add-Check -Status Warning -Name "GPU driver" -Detail "Unable to query Win32_VideoController: $($_.Exception.Message)"
}

$nvidiaGpus = @()
try {
    $nvidiaGpus = @(Get-CimInstance Win32_VideoController -ErrorAction Stop | Where-Object { $_.Name -match "NVIDIA" })
} catch {
    $nvidiaGpus = @()
}

$nvidiaSmiCandidates = @()
$pathCommand = Get-Command "nvidia-smi.exe" -ErrorAction SilentlyContinue
if ($pathCommand) {
    $nvidiaSmiCandidates += $pathCommand.Source
}

$knownNvidiaSmiPaths = @(
    (Join-Path $env:ProgramFiles "NVIDIA Corporation\NVSMI\nvidia-smi.exe"),
    (Join-Path ${env:ProgramFiles(x86)} "NVIDIA Corporation\NVSMI\nvidia-smi.exe")
) | Where-Object { $_ }

foreach ($candidate in $knownNvidiaSmiPaths) {
    if (Test-Path -LiteralPath $candidate) {
        $nvidiaSmiCandidates += $candidate
    }
}

if ($nvidiaSmiCandidates.Count -gt 0) {
    Add-Check -Status Pass -Name "NVIDIA telemetry" -Detail "Found nvidia-smi at $($nvidiaSmiCandidates[0])."
} elseif ($nvidiaGpus.Count -gt 0) {
    Add-Check -Status Warning -Name "NVIDIA telemetry" -Detail "NVIDIA GPU detected, but nvidia-smi.exe was not found. GPU benchmarks can still use wgpu, but NVIDIA temperature telemetry may be unavailable." -Links @("https://www.nvidia.com/en-us/drivers/")
} else {
    Add-Check -Status Info -Name "NVIDIA telemetry" -Detail "No NVIDIA GPU detected and nvidia-smi.exe was not required."
}

if (Test-RootWmiNamespace -Name "LibreHardwareMonitor") {
    Add-Check -Status Pass -Name "LibreHardwareMonitor WMI" -Detail "root\LibreHardwareMonitor namespace is available."
} else {
    Add-Check -Status Info -Name "LibreHardwareMonitor WMI" -Detail "root\LibreHardwareMonitor namespace is not available. This optional provider is not bundled." -Links @("https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/releases")
}

if (Test-RootWmiNamespace -Name "OpenHardwareMonitor") {
    Add-Check -Status Pass -Name "OpenHardwareMonitor WMI" -Detail "root\OpenHardwareMonitor namespace is available."
} else {
    Add-Check -Status Info -Name "OpenHardwareMonitor WMI" -Detail "root\OpenHardwareMonitor namespace is not available. This optional provider is not bundled." -Links @("https://openhardwaremonitor.org/downloads/")
}

try {
    $driverOutput = & sc.exe query BenchScopeSensorDriver 2>&1
    if ($LASTEXITCODE -eq 0) {
        $driverText = ($driverOutput | Out-String).Trim()
        Add-Check -Status Info -Name "BenchScope sensor driver" -Detail "Optional driver service is present. $driverText"
    } else {
        Add-Check -Status Info -Name "BenchScope sensor driver" -Detail "Optional sensor driver is not installed. Standard BenchScope release should still run without it."
    }
} catch {
    Add-Check -Status Info -Name "BenchScope sensor driver" -Detail "Unable to query optional sensor driver: $($_.Exception.Message)"
}

if ($BundlePath) {
    $resolvedBundlePath = Resolve-InputPath $BundlePath
    if (-not (Test-Path -LiteralPath $resolvedBundlePath)) {
        Add-Check -Status Fail -Name "Bundle path" -Detail "Bundle path does not exist: $resolvedBundlePath"
    } else {
        Add-Check -Status Pass -Name "Bundle path" -Detail "Bundle path exists: $resolvedBundlePath"

        $mainExe = Join-Path $resolvedBundlePath "BenchScope.exe"
        if (Test-Path -LiteralPath $mainExe) {
            Add-Check -Status Pass -Name "BenchScope.exe" -Detail "Main executable found."
        } else {
            Add-Check -Status Fail -Name "BenchScope.exe" -Detail "Main executable is missing from the bundle root."
        }

        foreach ($companion in @("benchscope_sensor_service.exe", "benchscope_sensor_probe.exe")) {
            $path = Join-Path $resolvedBundlePath $companion
            if (Test-Path -LiteralPath $path) {
                Add-Check -Status Pass -Name $companion -Detail "Companion binary found."
            } else {
                Add-Check -Status Warning -Name $companion -Detail "Companion binary is missing. This should be intentional if the related optional path is disabled."
            }
        }

        $linksPath = Join-Path $resolvedBundlePath "docs\FIRST_BOOT_LINKS.md"
        if (Test-Path -LiteralPath $linksPath) {
            Add-Check -Status Pass -Name "First-boot links" -Detail "docs\FIRST_BOOT_LINKS.md is included."
        } else {
            Add-Check -Status Warning -Name "First-boot links" -Detail "docs\FIRST_BOOT_LINKS.md is missing."
        }
    }
}

$items = $checks.ToArray()

if ($OutputPath) {
    $resolvedOutputPath = Resolve-InputPath $OutputPath
    $outputDir = Split-Path -Parent $resolvedOutputPath
    if ($outputDir -and -not (Test-Path -LiteralPath $outputDir)) {
        New-Item -ItemType Directory -Path $outputDir -Force | Out-Null
    }

    if ($Format -eq "Json") {
        $items | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $resolvedOutputPath -Encoding UTF8
    } else {
        Convert-ChecksToMarkdown -Items $items | Set-Content -LiteralPath $resolvedOutputPath -Encoding UTF8
    }
}

if (-not $Quiet) {
    $items | Format-Table -AutoSize Status, Name, Detail
}

if ($FailOnMissingRequired -and (@($items | Where-Object { $_.Status -eq "Fail" }).Count -gt 0)) {
    exit 1
}
