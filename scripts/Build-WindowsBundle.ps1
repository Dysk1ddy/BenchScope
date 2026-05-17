param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Release",
    [string]$OutputRoot = "",
    [string]$BundleName = "",
    [switch]$SkipBuild,
    [switch]$Clean,
    [switch]$Zip,
    [string]$VcRedistPath = "",
    [switch]$DownloadVcRedist,
    [switch]$NoPrereqReport
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $OutputRoot) {
    $OutputRoot = Join-Path $repoRoot "dist"
}

function Resolve-FullPath {
    param([Parameter(Mandatory = $true)][string]$Path)

    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }

    return [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Path))
}

function Get-ProjectVersion {
    $cargoToml = Join-Path $repoRoot "Cargo.toml"
    $versionLine = Select-String -Path $cargoToml -Pattern '^\s*version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if ($versionLine -and $versionLine.Matches.Count -gt 0) {
        return $versionLine.Matches[0].Groups[1].Value
    }

    return "0.0.0"
}

function Get-CargoTargetRoot {
    $configPath = Join-Path $repoRoot ".cargo\config.toml"
    if (Test-Path -LiteralPath $configPath) {
        $targetLine = Select-String -Path $configPath -Pattern '^\s*target-dir\s*=\s*"([^"]+)"' | Select-Object -First 1
        if ($targetLine -and $targetLine.Matches.Count -gt 0) {
            $targetDir = $targetLine.Matches[0].Groups[1].Value
            if ([System.IO.Path]::IsPathRooted($targetDir)) {
                return [System.IO.Path]::GetFullPath($targetDir)
            }

            return [System.IO.Path]::GetFullPath((Join-Path $repoRoot $targetDir))
        }
    }

    return Join-Path $repoRoot "target"
}

function Get-SourceCommit {
    $git = Get-Command git.exe -ErrorAction SilentlyContinue
    if (-not $git) {
        return ""
    }

    $commit = & $git.Source -C $repoRoot rev-parse --short HEAD 2>$null
    if ($LASTEXITCODE -ne 0) {
        return ""
    }

    return ($commit | Out-String).Trim()
}

function Copy-RequiredFile {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination
    )

    if (-not (Test-Path -LiteralPath $Source)) {
        throw "Required bundle file is missing: $Source"
    }

    Copy-Item -LiteralPath $Source -Destination $Destination -Force
}

$version = Get-ProjectVersion
if (-not $BundleName) {
    $BundleName = "BenchScope-$version-windows-x64"
}

$outputRootFull = Resolve-FullPath $OutputRoot
$bundleDir = Join-Path $outputRootFull $BundleName
$bundleDirFull = [System.IO.Path]::GetFullPath($bundleDir)

if ((Test-Path -LiteralPath $bundleDirFull) -and -not $Clean) {
    throw "Bundle directory already exists: $bundleDirFull. Rerun with -Clean or choose a different -BundleName."
}

if ($Clean -and (Test-Path -LiteralPath $bundleDirFull)) {
    $outputRootWithSeparator = $outputRootFull.TrimEnd("\") + "\"
    if (-not $bundleDirFull.StartsWith($outputRootWithSeparator, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to clean a bundle outside the output root: $bundleDirFull"
    }

    if ((Split-Path -Leaf $bundleDirFull) -notlike "BenchScope-*") {
        throw "Refusing to clean a directory that does not look like a BenchScope bundle: $bundleDirFull"
    }

    Remove-Item -LiteralPath $bundleDirFull -Recurse -Force
}

New-Item -ItemType Directory -Path $bundleDirFull -Force | Out-Null
$docsDir = Join-Path $bundleDirFull "docs"
$toolsDir = Join-Path $bundleDirFull "tools"
$reportsDir = Join-Path $bundleDirFull "reports"
$installersDir = Join-Path $bundleDirFull "installers"
foreach ($dir in @($docsDir, $toolsDir, $reportsDir, $installersDir)) {
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
}

if (-not $SkipBuild) {
    $cargo = Get-Command cargo.exe -ErrorAction SilentlyContinue
    if (-not $cargo) {
        $userCargo = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
        if (Test-Path -LiteralPath $userCargo) {
            $cargo = [pscustomobject]@{ Source = $userCargo }
        }
    }

    if (-not $cargo) {
        throw "cargo.exe was not found. Install Rust for source builds, or rerun with -SkipBuild if release binaries already exist."
    }

    $cargoArgs = @("build", "--bins")
    if ($Configuration -eq "Release") {
        $cargoArgs += "--release"
    }

    Push-Location $repoRoot
    try {
        & $cargo.Source @cargoArgs
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build failed with exit code $LASTEXITCODE."
        }
    } finally {
        Pop-Location
    }
}

$targetRoot = Get-CargoTargetRoot
$profileDirName = $Configuration.ToLowerInvariant()
$profileDir = Join-Path $targetRoot $profileDirName
$binaryNames = @("BenchScope.exe", "benchscope_sensor_service.exe", "benchscope_sensor_probe.exe")

foreach ($binaryName in $binaryNames) {
    Copy-RequiredFile -Source (Join-Path $profileDir $binaryName) -Destination (Join-Path $bundleDirFull $binaryName)
}

$packagingDir = Join-Path $repoRoot "packaging\windows"
Copy-RequiredFile -Source (Join-Path $packagingDir "README-BUNDLE.md") -Destination (Join-Path $docsDir "README-BUNDLE.md")
Copy-RequiredFile -Source (Join-Path $packagingDir "FIRST_BOOT_LINKS.md") -Destination (Join-Path $docsDir "FIRST_BOOT_LINKS.md")
Copy-RequiredFile -Source (Join-Path $packagingDir "CLEAN_VM_VALIDATION.md") -Destination (Join-Path $docsDir "CLEAN_VM_VALIDATION.md")
Copy-RequiredFile -Source (Join-Path $packagingDir "THIRD_PARTY_NOTICES.md") -Destination (Join-Path $docsDir "THIRD_PARTY_NOTICES.md")
Copy-RequiredFile -Source (Join-Path $repoRoot "WINDOWS_INSTALL_REQUIREMENTS.md") -Destination (Join-Path $docsDir "WINDOWS_INSTALL_REQUIREMENTS.md")
Copy-RequiredFile -Source (Join-Path $PSScriptRoot "Test-WindowsRuntimePrereqs.ps1") -Destination (Join-Path $toolsDir "Test-WindowsRuntimePrereqs.ps1")
Copy-RequiredFile -Source (Join-Path $PSScriptRoot "Test-WindowsBundleLifecycle.ps1") -Destination (Join-Path $toolsDir "Test-WindowsBundleLifecycle.ps1")
Copy-RequiredFile -Source (Join-Path $PSScriptRoot "Install-WindowsBundle.ps1") -Destination (Join-Path $bundleDirFull "Install-BenchScope.ps1")
Copy-RequiredFile -Source (Join-Path $PSScriptRoot "Uninstall-WindowsBundle.ps1") -Destination (Join-Path $bundleDirFull "Uninstall-BenchScope.ps1")

$vcRedistBundled = $false
if ($VcRedistPath) {
    $vcRedistFull = Resolve-FullPath $VcRedistPath
    Copy-RequiredFile -Source $vcRedistFull -Destination (Join-Path $installersDir "vc_redist.x64.exe")
    $vcRedistBundled = $true
} elseif ($DownloadVcRedist) {
    $vcRedistUrl = "https://aka.ms/vs/17/release/vc_redist.x64.exe"
    $destination = Join-Path $installersDir "vc_redist.x64.exe"
    Invoke-WebRequest -Uri $vcRedistUrl -OutFile $destination
    $vcRedistBundled = $true
}

if (-not $NoPrereqReport) {
    $checker = Join-Path $PSScriptRoot "Test-WindowsRuntimePrereqs.ps1"
    $reportPath = Join-Path $reportsDir "first-boot-prereqs.md"
    & $checker -BundlePath $bundleDirFull -OutputPath $reportPath -Quiet
    if ($LASTEXITCODE -ne 0) {
        throw "Prerequisite report failed with exit code $LASTEXITCODE."
    }
}

$manifestFiles = New-Object System.Collections.Generic.List[object]
foreach ($path in @(Get-ChildItem -LiteralPath $bundleDirFull -File -Recurse)) {
    $relativePath = $path.FullName.Substring($bundleDirFull.Length).TrimStart("\")
    $manifestFiles.Add([pscustomobject]@{
        path = $relativePath
        bytes = $path.Length
        sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $path.FullName).Hash
    }) | Out-Null
}

$manifest = [pscustomobject]@{
    name = "BenchScope"
    version = $version
    platform = "windows-x64"
    configuration = $Configuration
    generatedAtUtc = (Get-Date).ToUniversalTime().ToString("o")
    sourceCommit = Get-SourceCommit
    vcRedistBundled = $vcRedistBundled
    files = $manifestFiles.ToArray()
    linkedPrerequisites = @(
        "https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist?view=msvc-170",
        "https://www.nvidia.com/en-us/drivers/",
        "https://www.amd.com/en/support/download/drivers.html",
        "https://www.intel.com/content/www/us/en/support/detect.html",
        "https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/releases",
        "https://openhardwaremonitor.org/downloads/"
    )
}

$manifestPath = Join-Path $bundleDirFull "bundle-manifest.json"
$manifest | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $manifestPath -Encoding UTF8

$zipPath = ""
if ($Zip) {
    $zipPath = Join-Path $outputRootFull "$BundleName.zip"
    if (Test-Path -LiteralPath $zipPath) {
        Remove-Item -LiteralPath $zipPath -Force
    }
    Compress-Archive -LiteralPath $bundleDirFull -DestinationPath $zipPath -Force
}

Write-Host "BenchScope bundle staged at: $bundleDirFull"
if ($vcRedistBundled) {
    Write-Host "Bundled VC++ redistributable in installers\vc_redist.x64.exe"
} else {
    Write-Host "VC++ redistributable was not bundled; see docs\FIRST_BOOT_LINKS.md"
}
if ($zipPath) {
    Write-Host "Bundle zip created at: $zipPath"
}
