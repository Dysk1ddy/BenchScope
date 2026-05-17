param(
    [switch]$Restart
)

$ErrorActionPreference = "Stop"

$principal = [Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Run this script from an elevated PowerShell window."
}

$current = & bcdedit.exe /enum
if ($LASTEXITCODE -ne 0) {
    throw "bcdedit failed while reading the current boot entry."
}
$currentText = $current | Out-String

if ($currentText -match "(?im)^\s*testsigning\s+Yes\s*$") {
    Write-Host "Windows test-signing is already enabled."
} else {
    $setOutput = & bcdedit.exe /set testsigning on 2>&1
    if ($LASTEXITCODE -ne 0) {
        $message = ($setOutput | Out-String).Trim()
        if ($message -match "Secure Boot") {
            throw "Failed to enable Windows test-signing because Secure Boot policy blocked the BCD change. Disable Secure Boot in UEFI firmware, boot Windows again, then rerun this script."
        }
        throw "Failed to enable Windows test-signing. $message"
    }
    Write-Host "Windows test-signing has been enabled."
    Write-Host "A reboot is required before a test-signed kernel driver can load."
}

if ($Restart) {
    Restart-Computer
}
