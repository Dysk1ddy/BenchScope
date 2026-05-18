param()

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$driverRoot = Join-Path $repoRoot "sensor-driver"
$sourceFiles = @(
    (Join-Path $driverRoot "BenchScopeSensorDriver.c"),
    (Join-Path $driverRoot "BenchScopeSensorDriver.h"),
    (Join-Path $driverRoot "include\BenchScopeSensorIoctl.h")
)

$failures = [System.Collections.Generic.List[string]]::new()

function Add-Failure {
    param([Parameter(Mandatory = $true)][string]$Message)
    $failures.Add($Message)
}

foreach ($path in $sourceFiles) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        Add-Failure "Missing expected driver source file: $path"
    }
}

if ($failures.Count -eq 0) {
    $headerPath = Join-Path $driverRoot "include\BenchScopeSensorIoctl.h"
    $driverSourcePath = Join-Path $driverRoot "BenchScopeSensorDriver.c"
    $allSource = ($sourceFiles | ForEach-Object { Get-Content -LiteralPath $_ -Raw }) -join "`n"
    $driverSource = Get-Content -LiteralPath $driverSourcePath -Raw

    $forbiddenPatterns = @(
        "__writemsr",
        "WRITE_PORT",
        "READ_PORT",
        "MmMapIoSpace",
        "ZwMapViewOfSection",
        "\\Device\\PhysicalMemory",
        "METHOD_NEITHER",
        "FILE_ANY_ACCESS"
    )

    foreach ($pattern in $forbiddenPatterns) {
        if ($allSource -match [regex]::Escape($pattern)) {
            Add-Failure "Forbidden kernel-access pattern found: $pattern"
        }
    }

    $ioctlDefinitions = Select-String `
        -LiteralPath $headerPath `
        -Pattern "CTL_CODE\(FILE_DEVICE_BENCHSCOPE_SENSOR" `
        -AllMatches

    if ($ioctlDefinitions.Count -eq 0) {
        Add-Failure "No BenchScope sensor IOCTL definitions were found."
    }

    foreach ($definition in $ioctlDefinitions) {
        $line = $definition.Line
        if ($line -notmatch "METHOD_BUFFERED" -or $line -notmatch "FILE_READ_DATA") {
            Add-Failure "IOCTL definition must use METHOD_BUFFERED and FILE_READ_DATA: $line"
        }
    }

    if ($driverSource -notmatch 'D:P\(A;;GA;;;SY\)\(A;;GA;;;BA\)') {
        Add-Failure "Expected restrictive LocalSystem/Built-in Administrators SDDL was not found."
    }

    if ($driverSource -notmatch "FILE_DEVICE_SECURE_OPEN") {
        Add-Failure "FILE_DEVICE_SECURE_OPEN was not found."
    }

    if ($driverSource -notmatch "WdfControlDeviceInitAllocate") {
        Add-Failure "KMDF control-device initialization was not found."
    }

    if ($driverSource -notmatch "WdfRequestRetrieveOutputBuffer") {
        Add-Failure "Output-buffer retrieval was not found."
    }

    if ($driverSource -notmatch "RtlZeroMemory") {
        Add-Failure "Output zeroing pattern was not found."
    }
}

if ($failures.Count -gt 0) {
    Write-Host "Sensor driver security surface check failed:"
    foreach ($failure in $failures) {
        Write-Host "  - $failure"
    }
    exit 1
}

Write-Host "Sensor driver security surface check passed."
