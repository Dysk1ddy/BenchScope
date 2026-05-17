param(
    [switch]$DebugOnly,
    [switch]$ReleaseOnly,
    [switch]$All
)

$ErrorActionPreference = "Stop"

$selectedModes = @($DebugOnly, $ReleaseOnly, $All) | Where-Object { $_ }
if ($selectedModes.Count -gt 1) {
    throw "Choose only one cleanup mode: -DebugOnly, -ReleaseOnly, or -All."
}

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$targetDir = [System.IO.Path]::GetFullPath((Join-Path $repoRoot "..\.cargo-target\BenchScope"))

function Assert-ChildPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Parent,

        [Parameter(Mandatory = $true)]
        [string]$Child
    )

    $parentFull = [System.IO.Path]::GetFullPath($Parent).TrimEnd('\', '/')
    $childFull = [System.IO.Path]::GetFullPath($Child)
    $comparison = [System.StringComparison]::OrdinalIgnoreCase

    if ($childFull -ne $parentFull -and -not $childFull.StartsWith("$parentFull\", $comparison)) {
        throw "Refusing to clean path outside the Cargo target cache: $childFull"
    }
}

function Remove-TargetChild {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    $childPath = [System.IO.Path]::GetFullPath((Join-Path $targetDir $Name))
    Assert-ChildPath -Parent $targetDir -Child $childPath

    if (Test-Path -LiteralPath $childPath) {
        Remove-Item -LiteralPath $childPath -Recurse -Force
        Write-Host "Removed $childPath"
    } else {
        Write-Host "Nothing to clean at $childPath"
    }
}

if ($DebugOnly) {
    Remove-TargetChild -Name "debug"
    exit
}

if ($ReleaseOnly) {
    Push-Location $repoRoot
    try {
        cargo clean --release --target-dir $targetDir
        exit $LASTEXITCODE
    } finally {
        Pop-Location
    }
}

Push-Location $repoRoot
try {
    cargo clean --target-dir $targetDir
    exit $LASTEXITCODE
} finally {
    Pop-Location
}
