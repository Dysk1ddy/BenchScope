param()

$ErrorActionPreference = "Stop"

$principal = [Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Run this script from an elevated PowerShell window."
}

$driverName = "BenchScopeSensorDriver"

& sc.exe query $driverName *> $null
if ($LASTEXITCODE -ne 0) {
    Write-Host "$driverName service is not installed."
    exit 0
}

& sc.exe stop $driverName | Out-Host
Start-Sleep -Milliseconds 500

& sc.exe delete $driverName | Out-Host
if ($LASTEXITCODE -ne 0) {
    throw "Failed to delete the $driverName service."
}

Write-Host "BenchScope sensor driver service was removed."
