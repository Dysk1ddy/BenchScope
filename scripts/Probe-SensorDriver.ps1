param()

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot

Push-Location $repoRoot
try {
    cargo run --bin benchscope_sensor_probe
} finally {
    Pop-Location
}
