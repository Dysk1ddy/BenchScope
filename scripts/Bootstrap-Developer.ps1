[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [switch]$InstallCore,
    [switch]$InstallHelperTools,
    [switch]$InstallDriverTools,
    [string]$ReportPath = "",
    [ValidateSet("Table", "Markdown", "Json")]
    [string]$Format = "Table"
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
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

function Resolve-InputPath {
    param([Parameter(Mandatory = $true)][string]$Path)

    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }

    return [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Path))
}

function Test-CommandPresent {
    param([Parameter(Mandatory = $true)][string]$Name)
    return $null -ne (Get-Command $Name -ErrorAction SilentlyContinue)
}

function Get-CommandText {
    param([Parameter(Mandatory = $true)][string]$Name)

    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    return ""
}

function Get-VsInstallations {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path -LiteralPath $vswhere)) {
        return @()
    }

    try {
        $json = & $vswhere -all -products * -format json
        if ($LASTEXITCODE -ne 0 -or -not $json) {
            return @()
        }

        return @($json | ConvertFrom-Json)
    } catch {
        return @()
    }
}

function Find-MsvcToolPath {
    param([Parameter(Mandatory = $true)][string]$FileName)

    $roots = New-Object System.Collections.Generic.List[string]
    foreach ($install in Get-VsInstallations) {
        if ($install.installationPath) {
            $roots.Add([string]$install.installationPath) | Out-Null
        }
    }

    $fallbackRoots = @(
        (Join-Path $env:ProgramFiles "Microsoft Visual Studio\2022\Community"),
        (Join-Path $env:ProgramFiles "Microsoft Visual Studio\2022\Professional"),
        (Join-Path $env:ProgramFiles "Microsoft Visual Studio\2022\Enterprise"),
        (Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\2022\BuildTools")
    ) | Where-Object { $_ }

    foreach ($root in $fallbackRoots) {
        if (Test-Path -LiteralPath $root) {
            $roots.Add($root) | Out-Null
        }
    }

    foreach ($root in @($roots | Select-Object -Unique)) {
        $matches = @(Get-ChildItem -LiteralPath (Join-Path $root "VC\Tools\MSVC") -Filter $FileName -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1)
        if ($matches.Count -gt 0) {
            return $matches[0].FullName
        }
    }

    return ""
}

function Find-MsvcSpectreLibPath {
    $roots = New-Object System.Collections.Generic.List[string]
    foreach ($install in Get-VsInstallations) {
        if ($install.installationPath) {
            $roots.Add([string]$install.installationPath) | Out-Null
        }
    }

    $fallbackRoots = @(
        (Join-Path $env:ProgramFiles "Microsoft Visual Studio\2022\Community"),
        (Join-Path $env:ProgramFiles "Microsoft Visual Studio\2022\Professional"),
        (Join-Path $env:ProgramFiles "Microsoft Visual Studio\2022\Enterprise"),
        (Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\2022\BuildTools")
    ) | Where-Object { $_ }

    foreach ($root in $fallbackRoots) {
        if (Test-Path -LiteralPath $root) {
            $roots.Add($root) | Out-Null
        }
    }

    foreach ($root in @($roots | Select-Object -Unique)) {
        $toolRoot = Join-Path $root "VC\Tools\MSVC"
        if (-not (Test-Path -LiteralPath $toolRoot)) {
            continue
        }

        $matches = @(Get-ChildItem -LiteralPath $toolRoot -Filter "vcruntime.lib" -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match "\\lib\\spectre\\x64\\" } |
            Select-Object -First 1)
        if ($matches.Count -gt 0) {
            return $matches[0].FullName
        }
    }

    return ""
}

function Get-WindowsSdkRoot {
    $keys = @(
        "HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots",
        "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots"
    )

    foreach ($key in $keys) {
        if (Test-Path -LiteralPath $key) {
            $props = Get-ItemProperty -LiteralPath $key
            if ($props.KitsRoot10 -and (Test-Path -LiteralPath $props.KitsRoot10)) {
                return $props.KitsRoot10
            }
        }
    }

    $fallback = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10"
    if (Test-Path -LiteralPath $fallback) {
        return $fallback
    }

    return ""
}

function Get-WindowsSdkVersions {
    $sdkRoot = Get-WindowsSdkRoot
    if (-not $sdkRoot) {
        return @()
    }

    $libRoot = Join-Path $sdkRoot "Lib"
    if (-not (Test-Path -LiteralPath $libRoot)) {
        return @()
    }

    return @(Get-ChildItem -LiteralPath $libRoot -Directory -ErrorAction SilentlyContinue | Where-Object {
        Test-Path -LiteralPath (Join-Path $_.FullName "um\x64\kernel32.lib")
    } | ForEach-Object { $_.Name })
}

function Get-WdkVersions {
    $sdkRoot = Get-WindowsSdkRoot
    if (-not $sdkRoot) {
        return @()
    }

    $buildRoot = Join-Path $sdkRoot "build"
    if (-not (Test-Path -LiteralPath $buildRoot)) {
        return @()
    }

    return @(Get-ChildItem -LiteralPath $buildRoot -Directory -ErrorAction SilentlyContinue | Where-Object {
        Test-Path -LiteralPath (Join-Path $_.FullName "WindowsDriver.common.targets")
    } | ForEach-Object { $_.Name })
}

function Test-DotNetSdk10 {
    $dotnet = Get-Command dotnet.exe -ErrorAction SilentlyContinue
    if (-not $dotnet) {
        return [pscustomobject]@{ Found = $false; Detail = "dotnet.exe was not found." }
    }

    $sdks = @(& $dotnet.Source --list-sdks 2>$null)
    $sdk10 = @($sdks | Where-Object { $_ -match "^10\." })
    if ($sdk10.Count -gt 0) {
        return [pscustomobject]@{ Found = $true; Detail = "Found .NET SDK: $($sdk10 -join "; ")" }
    }

    return [pscustomobject]@{ Found = $false; Detail = "dotnet.exe exists, but no .NET 10 SDK was listed." }
}

function Invoke-WingetInstall {
    param(
        [Parameter(Mandatory = $true)][string]$PackageId,
        [string]$Override = "",
        [switch]$Silent
    )

    $winget = Get-Command winget.exe -ErrorAction SilentlyContinue
    if (-not $winget) {
        throw "winget.exe is not available. See https://learn.microsoft.com/windows/package-manager/winget/"
    }

    $args = @("install", "--source", "winget", "--exact", "--id", $PackageId, "--accept-source-agreements", "--accept-package-agreements")
    if ($Silent) {
        $args += "--silent"
    }
    if ($Override) {
        $args += @("--override", $Override)
    }

    if ($PSCmdlet.ShouldProcess($PackageId, "winget install")) {
        & $winget.Source @args
        if ($LASTEXITCODE -ne 0) {
            throw "winget install failed for $PackageId with exit code $LASTEXITCODE."
        }
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
    $lines.Add("# BenchScope Developer Bootstrap Report") | Out-Null
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

if ($InstallCore) {
    Invoke-WingetInstall -PackageId "Rustlang.Rustup" -Silent
    Invoke-WingetInstall -PackageId "Microsoft.VisualStudio.2022.BuildTools" -Override "--passive --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --includeRecommended"
    Invoke-WingetInstall -PackageId "Microsoft.WindowsSDK.10.0.26100"
}

if ($InstallHelperTools) {
    Invoke-WingetInstall -PackageId "Microsoft.DotNet.SDK.10"
}

if ($InstallDriverTools) {
    Invoke-WingetInstall -PackageId "Microsoft.VisualStudio.2022.BuildTools" -Override "--passive --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --add Microsoft.VisualStudio.Component.VC.Runtimes.x86.x64.Spectre --includeRecommended"
    Invoke-WingetInstall -PackageId "Microsoft.WindowsSDK.10.0.26100"
    Invoke-WingetInstall -PackageId "Microsoft.WindowsWDK.10.0.26100"
}

$cargo = Get-CommandText "cargo.exe"
$rustc = Get-CommandText "rustc.exe"
if ($cargo -and $rustc) {
    $cargoVersion = (& $cargo --version 2>$null | Out-String).Trim()
    $rustcVersion = (& $rustc --version 2>$null | Out-String).Trim()
    Add-Check -Status Pass -Name "Rust toolchain" -Detail "$cargoVersion; $rustcVersion"
} else {
    Add-Check -Status Fail -Name "Rust toolchain" -Detail "cargo.exe or rustc.exe was not found." -Links @("https://www.rust-lang.org/tools/install")
}

$cl = Find-MsvcToolPath -FileName "cl.exe"
$link = Find-MsvcToolPath -FileName "link.exe"
if ($cl -and $link) {
    Add-Check -Status Pass -Name "MSVC C++ tools" -Detail "Found cl.exe at $cl and link.exe at $link."
} else {
    Add-Check -Status Fail -Name "MSVC C++ tools" -Detail "MSVC C++ compiler/linker were not found." -Links @("https://visualstudio.microsoft.com/downloads/")
}

$spectreLib = Find-MsvcSpectreLibPath
if ($spectreLib) {
    Add-Check -Status Pass -Name "MSVC Spectre x64 libs" -Detail "Found Spectre library at $spectreLib."
} else {
    Add-Check -Status Warning -Name "MSVC Spectre x64 libs" -Detail "Spectre-mitigated x64 libraries were not confirmed. They are needed for WDK driver builds." -Links @("https://learn.microsoft.com/visualstudio/install/workload-component-id-vs-build-tools?view=vs-2022")
}

$sdkVersions = Get-WindowsSdkVersions
if ($sdkVersions.Count -gt 0) {
    Add-Check -Status Pass -Name "Windows SDK" -Detail "Found SDK versions: $($sdkVersions -join ", ")."
} else {
    Add-Check -Status Fail -Name "Windows SDK" -Detail "No Windows 10/11 SDK libraries were found." -Links @("https://developer.microsoft.com/windows/downloads/windows-sdk/")
}

$dotnet10 = Test-DotNetSdk10
if ($dotnet10.Found) {
    Add-Check -Status Pass -Name ".NET SDK 10" -Detail $dotnet10.Detail
} else {
    Add-Check -Status Warning -Name ".NET SDK 10" -Detail "$($dotnet10.Detail) Only needed for sensor-helper builds." -Links @("https://dotnet.microsoft.com/en-us/download/dotnet/10.0")
}

$wdkVersions = Get-WdkVersions
if ($wdkVersions.Count -gt 0) {
    Add-Check -Status Pass -Name "Windows Driver Kit" -Detail "Found WDK build targets: $($wdkVersions -join ", ")."
} else {
    Add-Check -Status Warning -Name "Windows Driver Kit" -Detail "WDK build targets were not found. Only needed for sensor-driver builds." -Links @("https://learn.microsoft.com/windows-hardware/drivers/download-the-wdk")
}

if (Test-CommandPresent "winget.exe") {
    Add-Check -Status Pass -Name "winget" -Detail "Windows Package Manager is available."
} else {
    Add-Check -Status Warning -Name "winget" -Detail "winget.exe was not found; install switches cannot be used." -Links @("https://learn.microsoft.com/windows/package-manager/winget/")
}

$nugetConfig = Join-Path $repoRoot "config\NuGet.Config"
if (Test-Path -LiteralPath $nugetConfig) {
    Add-Check -Status Info -Name "NuGet config" -Detail "Repo NuGet config exists at config\NuGet.Config."
} else {
    Add-Check -Status Warning -Name "NuGet config" -Detail "config\NuGet.Config was not found."
}

$items = $checks.ToArray()

if ($ReportPath) {
    $resolvedReportPath = Resolve-InputPath $ReportPath
    $reportDir = Split-Path -Parent $resolvedReportPath
    if ($reportDir -and -not (Test-Path -LiteralPath $reportDir)) {
        New-Item -ItemType Directory -Path $reportDir -Force | Out-Null
    }

    if ($Format -eq "Json") {
        $items | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $resolvedReportPath -Encoding UTF8
    } else {
        Convert-ChecksToMarkdown -Items $items | Set-Content -LiteralPath $resolvedReportPath -Encoding UTF8
    }
}

if ($Format -eq "Json" -and -not $ReportPath) {
    $items | ConvertTo-Json -Depth 5
} elseif ($Format -eq "Markdown" -and -not $ReportPath) {
    Convert-ChecksToMarkdown -Items $items
} else {
    $items | Format-Table -AutoSize Status, Name, Detail
}
