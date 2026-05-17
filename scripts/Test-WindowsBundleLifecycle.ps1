[CmdletBinding()]
param(
    [string]$BundlePath = "",
    [string]$WorkRoot = "",
    [string]$ReportPath = "",
    [switch]$InstallVcRedist,
    [switch]$KeepWorkRoot
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $BundlePath) {
    $BundlePath = Join-Path $repoRoot "dist\BenchScope-0.1.0-windows-x64"
}
if (-not $WorkRoot) {
    $WorkRoot = Join-Path $repoRoot "dist\bundle-lifecycle-smoke"
}
if (-not $ReportPath) {
    $ReportPath = Join-Path $repoRoot "dist\bundle-lifecycle-smoke.md"
}

$results = New-Object System.Collections.Generic.List[object]

function Resolve-InputPath {
    param([Parameter(Mandatory = $true)][string]$Path)

    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }

    return [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Path))
}

function Resolve-ChildPath {
    param(
        [Parameter(Mandatory = $true)][string]$BasePath,
        [Parameter(Mandatory = $true)][string]$RelativePath
    )

    if ([System.IO.Path]::IsPathRooted($RelativePath)) {
        throw "Refusing rooted relative path: $RelativePath"
    }

    $baseFull = [System.IO.Path]::GetFullPath($BasePath).TrimEnd("\")
    $childFull = [System.IO.Path]::GetFullPath((Join-Path $baseFull $RelativePath))
    $baseWithSeparator = $baseFull + "\"
    if (-not $childFull.StartsWith($baseWithSeparator, [System.StringComparison]::OrdinalIgnoreCase) -and
        -not $childFull.Equals($baseFull, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing path outside base directory: $RelativePath"
    }

    return $childFull
}

function Add-Result {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet("Pass", "Fail", "Info")]
        [string]$Status,
        [Parameter(Mandatory = $true)][string]$Step,
        [Parameter(Mandatory = $true)][string]$Detail
    )

    $results.Add([pscustomobject]@{
        Status = $Status
        Step = $Step
        Detail = $Detail
    }) | Out-Null
}

function Assert-Condition {
    param(
        [Parameter(Mandatory = $true)][bool]$Condition,
        [Parameter(Mandatory = $true)][string]$Step,
        [Parameter(Mandatory = $true)][string]$PassDetail,
        [Parameter(Mandatory = $true)][string]$FailDetail
    )

    if ($Condition) {
        Add-Result -Status Pass -Step $Step -Detail $PassDetail
        return
    }

    Add-Result -Status Fail -Step $Step -Detail $FailDetail
    throw $FailDetail
}

function Convert-Cell {
    param([AllowNull()][object]$Value)
    if ($null -eq $Value) {
        return ""
    }

    return ([string]$Value).Replace("|", "\|").Replace("`r`n", "<br>").Replace("`n", "<br>")
}

function Write-LifecycleReport {
    param([Parameter(Mandatory = $true)][string]$OutputPath)

    $resolved = Resolve-InputPath $OutputPath
    $dir = Split-Path -Parent $resolved
    if ($dir -and -not (Test-Path -LiteralPath $dir)) {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
    }

    $lines = New-Object System.Collections.Generic.List[string]
    $lines.Add("# BenchScope Bundle Lifecycle Smoke Report") | Out-Null
    $lines.Add("") | Out-Null
    $lines.Add("Generated: $((Get-Date).ToString("s"))") | Out-Null
    $lines.Add("") | Out-Null
    $lines.Add("| Status | Step | Detail |") | Out-Null
    $lines.Add("| --- | --- | --- |") | Out-Null

    foreach ($result in $results) {
        $lines.Add("| $(Convert-Cell $result.Status) | $(Convert-Cell $result.Step) | $(Convert-Cell $result.Detail) |") | Out-Null
    }

    $lines -join [Environment]::NewLine | Set-Content -LiteralPath $resolved -Encoding UTF8
}

function Invoke-Install {
    param(
        [Parameter(Mandatory = $true)][string]$BundleDirectory,
        [Parameter(Mandatory = $true)][string]$InstallDirectory
    )

    $installer = Join-Path $BundleDirectory "Install-BenchScope.ps1"
    if ($InstallVcRedist) {
        & $installer -Scope CurrentUser -InstallDir $InstallDirectory -NoStartMenuShortcut -RunPrereqCheck -InstallVcRedist
    } else {
        & $installer -Scope CurrentUser -InstallDir $InstallDirectory -NoStartMenuShortcut -RunPrereqCheck
    }
    if ($LASTEXITCODE -ne 0) {
        throw "Install-BenchScope.ps1 failed with exit code $LASTEXITCODE."
    }
}

function Invoke-Uninstall {
    param(
        [Parameter(Mandatory = $true)][string]$InstallDirectory,
        [switch]$RemoveReports
    )

    $uninstaller = Join-Path $InstallDirectory "Uninstall-BenchScope.ps1"
    if ($RemoveReports) {
        & $uninstaller -RemoveReports
    } else {
        & $uninstaller
    }
    if ($LASTEXITCODE -ne 0) {
        throw "Uninstall-BenchScope.ps1 failed with exit code $LASTEXITCODE."
    }
}

function Add-StaleManagedFile {
    param([Parameter(Mandatory = $true)][string]$InstallDirectory)

    $manifestPath = Join-Path $InstallDirectory "bundle-manifest.json"
    $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
    $stalePath = Resolve-ChildPath -BasePath $InstallDirectory -RelativePath "docs\OLD_STALE.md"
    Set-Content -LiteralPath $stalePath -Value "old managed file" -Encoding UTF8
    $stale = [pscustomobject]@{
        path = "docs\OLD_STALE.md"
        bytes = (Get-Item -LiteralPath $stalePath).Length
        sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $stalePath).Hash
    }

    $manifest | Add-Member -NotePropertyName files -NotePropertyValue (@($manifest.files) + $stale) -Force
    $manifest | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
}

$bundleRoot = Resolve-InputPath $BundlePath
$workRootFull = Resolve-InputPath $WorkRoot
$installRoot = Join-Path $workRootFull "install-target"

try {
    Assert-Condition -Condition (Test-Path -LiteralPath (Join-Path $bundleRoot "bundle-manifest.json")) -Step "Bundle manifest" -PassDetail "bundle-manifest.json exists in $bundleRoot." -FailDetail "Bundle manifest missing from $bundleRoot."

    $distRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot "dist")).TrimEnd("\")
    $workRootNormalized = $workRootFull.TrimEnd("\")
    $safeDistChild = $workRootNormalized.StartsWith($distRoot + "\", [System.StringComparison]::OrdinalIgnoreCase)
    $safeTempChild = $workRootNormalized.StartsWith([System.IO.Path]::GetFullPath($env:TEMP).TrimEnd("\") + "\", [System.StringComparison]::OrdinalIgnoreCase)
    if (-not $safeDistChild -and -not $safeTempChild) {
        throw "Refusing lifecycle test work root outside repo dist or TEMP: $workRootFull"
    }

    if (Test-Path -LiteralPath $workRootFull) {
        Remove-Item -LiteralPath $workRootFull -Recurse -Force
    }
    New-Item -ItemType Directory -Path $workRootFull -Force | Out-Null

    Invoke-Install -BundleDirectory $bundleRoot -InstallDirectory $installRoot
    Assert-Condition -Condition (Test-Path -LiteralPath (Join-Path $installRoot "BenchScope.exe")) -Step "Install" -PassDetail "BenchScope.exe installed." -FailDetail "BenchScope.exe was not installed."
    Assert-Condition -Condition (Test-Path -LiteralPath (Join-Path $installRoot "reports\install-prereqs.md")) -Step "Install prereq report" -PassDetail "Installed prerequisite report was generated." -FailDetail "Installed prerequisite report was not generated."

    New-Item -ItemType Directory -Path (Join-Path $installRoot "reports") -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $installRoot "reports\user-report-preserve-check.md") -Value "preserve me" -Encoding UTF8
    Add-StaleManagedFile -InstallDirectory $installRoot
    Invoke-Install -BundleDirectory $bundleRoot -InstallDirectory $installRoot
    Assert-Condition -Condition (-not (Test-Path -LiteralPath (Join-Path $installRoot "docs\OLD_STALE.md"))) -Step "Update stale cleanup" -PassDetail "Stale managed file was removed during update." -FailDetail "Stale managed file survived update."
    Assert-Condition -Condition (Test-Path -LiteralPath (Join-Path $installRoot "reports\user-report-preserve-check.md")) -Step "Update report preservation" -PassDetail "User report survived update." -FailDetail "User report was removed during update."

    Invoke-Uninstall -InstallDirectory $installRoot
    Assert-Condition -Condition (-not (Test-Path -LiteralPath (Join-Path $installRoot "BenchScope.exe"))) -Step "Uninstall preserve reports" -PassDetail "Managed binaries removed." -FailDetail "BenchScope.exe survived uninstall."
    Assert-Condition -Condition (Test-Path -LiteralPath (Join-Path $installRoot "reports\user-report-preserve-check.md")) -Step "Uninstall report preservation" -PassDetail "Reports were preserved by default." -FailDetail "Reports were not preserved by default."

    Invoke-Install -BundleDirectory $bundleRoot -InstallDirectory $installRoot
    Invoke-Uninstall -InstallDirectory $installRoot -RemoveReports
    Assert-Condition -Condition (-not (Test-Path -LiteralPath $installRoot)) -Step "Uninstall remove reports" -PassDetail "Install directory removed with -RemoveReports." -FailDetail "Install directory survived -RemoveReports uninstall."

    Add-Result -Status Pass -Step "Lifecycle smoke" -Detail "Install, update, preserve-report uninstall, and remove-report uninstall all passed."
} catch {
    Add-Result -Status Fail -Step "Lifecycle smoke" -Detail $_.Exception.Message
    throw
} finally {
    Write-LifecycleReport -OutputPath $ReportPath
    if (-not $KeepWorkRoot -and (Test-Path -LiteralPath $workRootFull)) {
        Remove-Item -LiteralPath $workRootFull -Recurse -Force
    }
}
