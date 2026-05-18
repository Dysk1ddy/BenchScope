param(
    [switch]$SkipBuild,
    [ValidateSet("Release")]
    [string]$Configuration = "Release",
    [ValidateSet("x64")]
    [string]$Platform = "x64",
    [string]$WindowsTargetPlatformVersion = "10.0.28000.0",
    [string]$OutputRoot
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot

if (-not $OutputRoot) {
    $OutputRoot = Join-Path $repoRoot "artifacts\attestation"
}

function Get-FullPathAllowMissing {
    param([Parameter(Mandatory = $true)][string]$Path)
    return [System.IO.Path]::GetFullPath($Path)
}

function Assert-NotUncPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    $fullPath = Get-FullPathAllowMissing -Path $Path
    if ($fullPath.StartsWith("\\") -and -not $fullPath.StartsWith("\\?\")) {
        throw "UNC paths are not valid for attestation CAB input/output: $fullPath"
    }
}

function Assert-ChildPath {
    param(
        [Parameter(Mandatory = $true)][string]$Parent,
        [Parameter(Mandatory = $true)][string]$Child
    )

    $parentFull = (Get-FullPathAllowMissing -Path $Parent).TrimEnd('\')
    $childFull = (Get-FullPathAllowMissing -Path $Child).TrimEnd('\')
    if ($childFull -ne $parentFull -and -not $childFull.StartsWith("$parentFull\", [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to use path outside expected root. Parent: $parentFull Child: $childFull"
    }
}

function Remove-GeneratedPath {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Path
    )

    Assert-ChildPath -Parent $Root -Child $Path
    if (Test-Path -LiteralPath $Path) {
        Remove-Item -LiteralPath $Path -Recurse -Force
    }
}

function Quote-DdfPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    if ($Path.Contains('"')) {
        throw "DDF paths cannot contain double quotes: $Path"
    }
    return '"' + $Path + '"'
}

$repoRootFull = Get-FullPathAllowMissing -Path $repoRoot
$outputRootFull = Get-FullPathAllowMissing -Path $OutputRoot
Assert-NotUncPath -Path $repoRootFull
Assert-NotUncPath -Path $outputRootFull
Assert-ChildPath -Parent $repoRootFull -Child $outputRootFull

if (-not $SkipBuild) {
    $buildScript = Join-Path $repoRootFull "scripts\BUILD_SENSOR_DRIVER.ps1"
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $buildScript `
        -Configuration $Configuration `
        -Platform $Platform `
        -WindowsTargetPlatformVersion $WindowsTargetPlatformVersion `
        -SignMode Off
    if ($LASTEXITCODE -ne 0) {
        throw "Release driver build failed."
    }
}

$driverOutputRoot = Join-Path $repoRootFull "sensor-driver\$Platform\$Configuration"
$driverPackageRoot = Join-Path $driverOutputRoot "BenchScopeSensorDriver"
$expectedSources = @(
    @{
        Source = Join-Path $driverPackageRoot "BenchScopeSensorDriver.inf"
        Name = "BenchScopeSensorDriver.inf"
    },
    @{
        Source = Join-Path $driverPackageRoot "BenchScopeSensorDriver.sys"
        Name = "BenchScopeSensorDriver.sys"
    },
    @{
        Source = Join-Path $driverPackageRoot "benchscopesensordriver.cat"
        Name = "BenchScopeSensorDriver.cat"
    },
    @{
        Source = Join-Path $driverOutputRoot "BenchScopeSensorDriver.pdb"
        Name = "BenchScopeSensorDriver.pdb"
    }
)

foreach ($entry in $expectedSources) {
    $sourceFull = Get-FullPathAllowMissing -Path $entry.Source
    Assert-NotUncPath -Path $sourceFull
    Assert-ChildPath -Parent $repoRootFull -Child $sourceFull
    if (-not (Test-Path -LiteralPath $sourceFull -PathType Leaf)) {
        throw "Required driver package input is missing: $sourceFull"
    }
}

$stageRoot = Join-Path $outputRootFull "stage"
$packageStageRoot = Join-Path $stageRoot "BenchScopeSensorDriver"
$cabPath = Join-Path $outputRootFull "BenchScopeSensorDriver-attestation.cab"
$ddfPath = Join-Path $outputRootFull "BenchScopeSensorDriver-attestation.ddf"
$hashPath = Join-Path $outputRootFull "BenchScopeSensorDriver-attestation.hashes.txt"
$cabInfPath = Join-Path $outputRootFull "BenchScopeSensorDriver-attestation.inf"
$cabReportPath = Join-Path $outputRootFull "BenchScopeSensorDriver-attestation.rpt"

Remove-GeneratedPath -Root $repoRootFull -Path $stageRoot
foreach ($generatedFile in @($cabPath, $ddfPath, $hashPath, $cabInfPath, $cabReportPath)) {
    Assert-ChildPath -Parent $outputRootFull -Child $generatedFile
    if (Test-Path -LiteralPath $generatedFile -PathType Leaf) {
        Remove-Item -LiteralPath $generatedFile -Force
    }
}

New-Item -ItemType Directory -Path $packageStageRoot -Force | Out-Null

$stagedFiles = @()
foreach ($entry in $expectedSources) {
    $destination = Join-Path $packageStageRoot $entry.Name
    Copy-Item -LiteralPath $entry.Source -Destination $destination -Force
    $stagedFiles += $destination
}

$ddfLines = @(
    ".OPTION EXPLICIT",
    ".Set CabinetFileCountThreshold=0",
    ".Set FolderFileCountThreshold=0",
    ".Set FolderSizeThreshold=0",
    ".Set MaxCabinetSize=0",
    ".Set MaxDiskFileCount=0",
    ".Set MaxDiskSize=0",
    ".Set CompressionType=MSZIP",
    ".Set Cabinet=on",
    ".Set Compress=on",
    ".Set CabinetNameTemplate=BenchScopeSensorDriver-attestation.cab",
    ".Set InfFileName=$(Quote-DdfPath -Path $cabInfPath)",
    ".Set RptFileName=$(Quote-DdfPath -Path $cabReportPath)",
    ".Set DiskDirectory1=$(Quote-DdfPath -Path $outputRootFull)",
    ".Set DestinationDir=BenchScopeSensorDriver"
)

foreach ($file in $stagedFiles) {
    $ddfLines += Quote-DdfPath -Path (Get-FullPathAllowMissing -Path $file)
}

Set-Content -LiteralPath $ddfPath -Value $ddfLines -Encoding ASCII

& makecab.exe /F $ddfPath
if ($LASTEXITCODE -ne 0) {
    throw "makecab failed while creating $cabPath."
}

if (-not (Test-Path -LiteralPath $cabPath -PathType Leaf)) {
    throw "makecab completed but the CAB was not found at $cabPath."
}

$hashInputs = @($stagedFiles + $cabPath)
$hashLines = foreach ($path in $hashInputs) {
    $hash = Get-FileHash -Algorithm SHA256 -LiteralPath $path
    "{0}  {1}" -f $hash.Hash, $hash.Path
}
Set-Content -LiteralPath $hashPath -Value $hashLines -Encoding ASCII

Write-Host "Created attestation CAB:"
Write-Host "  $cabPath"
Write-Host ""
Write-Host "Created hashes:"
Write-Host "  $hashPath"
Write-Host ""
Write-Host "Next release-only step: sign the CAB with the organization's Partner Center-associated certificate, then upload the signed CAB to the Microsoft Hardware Dashboard."
