param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Debug",
    [ValidateSet("x64")]
    [string]$Platform = "x64",
    [string]$WindowsTargetPlatformVersion = "10.0.28000.0",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$principal = [Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Run this script from an elevated PowerShell window."
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$driverName = "BenchScopeSensorDriver"
$buildScript = Join-Path $PSScriptRoot "BUILD_SENSOR_DRIVER.ps1"
$driverSys = Join-Path $repoRoot "sensor-driver\$Platform\$Configuration\$driverName.sys"
$driverCert = Join-Path $repoRoot "sensor-driver\$Platform\$Configuration\$driverName.cer"
$packageInf = Join-Path $repoRoot "sensor-driver\$Platform\$Configuration\$driverName\$driverName.inf"

function Wait-ForServiceDeletion {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [int]$TimeoutSeconds = 20
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        $query = & sc.exe query $Name 2>&1
        $exit = $LASTEXITCODE
        $text = $query | Out-String
        if ($exit -ne 0 -and $text -match "1060") {
            return $true
        }
        if ($text -match "1072|marked for deletion") {
            Start-Sleep -Milliseconds 500
            continue
        }
        Start-Sleep -Milliseconds 500
    } while ((Get-Date) -lt $deadline)

    return $false
}

$current = & bcdedit.exe /enum
if ($LASTEXITCODE -ne 0) {
    throw "bcdedit failed while reading the current boot entry."
}
$currentText = $current | Out-String

if ($currentText -notmatch "(?im)^\s*testsigning\s+Yes\s*$") {
    throw "Windows test-signing is not enabled for this boot. Run scripts\Enable-SensorDriverTestSigning.ps1, reboot, then rerun this installer."
}

if (-not $SkipBuild) {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $buildScript `
        -Configuration $Configuration `
        -Platform $Platform `
        -WindowsTargetPlatformVersion $WindowsTargetPlatformVersion `
        -SignMode TestSign
    if ($LASTEXITCODE -ne 0) {
        throw "Driver build/sign step failed."
    }
}

if (-not (Test-Path -LiteralPath $driverSys)) {
    throw "Driver binary was not found at $driverSys."
}

if (Test-Path -LiteralPath $driverCert) {
    & certutil.exe -addstore -f Root $driverCert | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to trust the development test certificate in LocalMachine Root."
    }

    & certutil.exe -addstore -f TrustedPublisher $driverCert | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to trust the development test certificate in LocalMachine TrustedPublisher."
    }
} else {
    throw "Development test certificate was not found at $driverCert."
}

if (Test-Path -LiteralPath $packageInf) {
    & pnputil.exe /add-driver $packageInf
    if ($LASTEXITCODE -ne 0) {
        throw "pnputil failed to add the driver package."
    }
}

$existing = & sc.exe query $driverName 2>$null
if ($LASTEXITCODE -eq 0) {
    $stopOutput = & sc.exe stop $driverName 2>&1
    $stopExit = $LASTEXITCODE
    $stopText = $stopOutput | Out-String
    $stopText | Out-Host
    if ($stopExit -ne 0 -and $stopText -notmatch "1052|1062|not valid|not been started") {
        throw "Failed to stop the existing $driverName service."
    }
    Start-Sleep -Milliseconds 500
    $deleteOutput = & sc.exe delete $driverName 2>&1
    $deleteExit = $LASTEXITCODE
    $deleteText = $deleteOutput | Out-String
    $deleteText | Out-Host
    if ($deleteExit -ne 0 -and $deleteText -notmatch "1072|marked for deletion") {
        throw "Failed to delete the existing $driverName service."
    }
    if (-not (Wait-ForServiceDeletion -Name $driverName)) {
        throw "The existing $driverName service is still marked for deletion. Close any BenchScope/probe/service windows and reboot Windows, then rerun this installer."
    }
}

$createOutput = & sc.exe create $driverName type= kernel start= demand binPath= $driverSys DisplayName= "BenchScope Sensor Driver" 2>&1
$createExit = $LASTEXITCODE
$createText = $createOutput | Out-String
$createText | Out-Host
if ($createExit -ne 0) {
    if ($createText -match "1072|marked for deletion") {
        throw "The $driverName service is still marked for deletion by Windows. Reboot Windows, then rerun this installer."
    }
    throw "Failed to create the $driverName kernel service."
}

& sc.exe start $driverName | Out-Host
if ($LASTEXITCODE -ne 0) {
    throw "Failed to start the $driverName kernel service."
}

Write-Host "BenchScope sensor driver is installed and running."
