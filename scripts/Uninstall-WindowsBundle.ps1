[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [ValidateSet("Machine", "CurrentUser")]
    [string]$Scope = "Machine",
    [string]$InstallDir = "",
    [switch]$RemoveReports,
    [switch]$RemoveDesktopShortcut,
    [switch]$StopRunningBenchScope
)

$ErrorActionPreference = "Stop"

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

function Test-IsAdministrator {
    $principal = [Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Get-DefaultInstallDir {
    param([Parameter(Mandatory = $true)][string]$RequestedScope)

    if ($RequestedScope -eq "Machine") {
        return Join-Path $env:ProgramFiles "BenchScope"
    }

    return Join-Path $env:LOCALAPPDATA "BenchScope"
}

function Get-ShortcutPaths {
    param(
        [Parameter(Mandatory = $true)][string]$RequestedScope,
        [object]$InstallState
    )

    $paths = New-Object System.Collections.Generic.List[string]

    if ($InstallState) {
        foreach ($propertyName in @("startMenuShortcut", "desktopShortcut")) {
            if ($InstallState.PSObject.Properties.Name -contains $propertyName) {
                $value = [string]$InstallState.$propertyName
                if ($value) {
                    $paths.Add($value) | Out-Null
                }
            }
        }
    }

    if ($RequestedScope -eq "Machine") {
        $paths.Add((Join-Path $env:ProgramData "Microsoft\Windows\Start Menu\Programs\BenchScope\BenchScope.lnk")) | Out-Null
        if ($RemoveDesktopShortcut) {
            $paths.Add((Join-Path $env:PUBLIC "Desktop\BenchScope.lnk")) | Out-Null
        }
    } else {
        $paths.Add((Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\BenchScope\BenchScope.lnk")) | Out-Null
        if ($RemoveDesktopShortcut) {
            $paths.Add((Join-Path $env:USERPROFILE "Desktop\BenchScope.lnk")) | Out-Null
        }
    }

    return @($paths | Select-Object -Unique)
}

function Remove-EmptyDirectories {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [string[]]$PreserveRelativeRoots = @()
    )

    if (-not (Test-Path -LiteralPath $Root)) {
        return
    }

    $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd("\")
    $preserve = @($PreserveRelativeRoots | ForEach-Object {
        [System.IO.Path]::GetFullPath((Join-Path $rootFull $_)).TrimEnd("\")
    })

    $dirs = @(Get-ChildItem -LiteralPath $rootFull -Directory -Recurse -Force | Sort-Object FullName -Descending)
    foreach ($dir in $dirs) {
        $dirFull = $dir.FullName.TrimEnd("\")
        if (@($preserve | Where-Object {
            $dirFull.Equals($_, [System.StringComparison]::OrdinalIgnoreCase) -or
            $dirFull.StartsWith($_ + "\", [System.StringComparison]::OrdinalIgnoreCase)
        }).Count -gt 0) {
            continue
        }

        if (@(Get-ChildItem -LiteralPath $dir.FullName -Force).Count -eq 0) {
            Remove-Item -LiteralPath $dir.FullName -Force
        }
    }

    if ($RemoveReports) {
        if (@(Get-ChildItem -LiteralPath $rootFull -Force).Count -eq 0) {
            Remove-Item -LiteralPath $rootFull -Force
        }
    }
}

function Get-RunningBenchScopeProcesses {
    param([Parameter(Mandatory = $true)][string]$InstallRoot)

    $installRootFull = [System.IO.Path]::GetFullPath($InstallRoot).TrimEnd("\")
    $names = @("BenchScope", "benchscope_sensor_service", "benchscope_sensor_probe")
    $running = New-Object System.Collections.Generic.List[object]

    foreach ($name in $names) {
        foreach ($process in @(Get-Process -Name $name -ErrorAction SilentlyContinue)) {
            $path = ""
            try {
                $path = [string]$process.MainModule.FileName
            } catch {
                $path = ""
            }

            if ($path -and [System.IO.Path]::GetFullPath($path).StartsWith($installRootFull, [System.StringComparison]::OrdinalIgnoreCase)) {
                $running.Add($process) | Out-Null
            }
        }
    }

    return $running.ToArray()
}

function Stop-OrBlockRunningBenchScope {
    param([Parameter(Mandatory = $true)][string]$InstallRoot)

    $running = @(Get-RunningBenchScopeProcesses -InstallRoot $InstallRoot)
    if ($running.Count -eq 0) {
        return
    }

    if (-not $StopRunningBenchScope) {
        $details = ($running | ForEach-Object { "$($_.ProcessName)($($_.Id))" }) -join ", "
        throw "BenchScope processes are running from this install and must be closed before uninstall: $details. Rerun with -StopRunningBenchScope to stop them."
    }

    foreach ($process in $running) {
        Stop-Process -Id $process.Id -Force
    }
}

if (-not $InstallDir) {
    $localState = Join-Path $PSScriptRoot "install-state.json"
    $localExe = Join-Path $PSScriptRoot "BenchScope.exe"
    if ((Test-Path -LiteralPath $localState) -or (Test-Path -LiteralPath $localExe)) {
        $InstallDir = $PSScriptRoot
    } else {
        $InstallDir = Get-DefaultInstallDir -RequestedScope $Scope
    }
}

$installRoot = Resolve-InputPath $InstallDir
if (-not (Test-Path -LiteralPath $installRoot)) {
    Write-Host "BenchScope install path does not exist: $installRoot"
    return
}

$statePath = Join-Path $installRoot "install-state.json"
$manifestPath = Join-Path $installRoot "bundle-manifest.json"
$mainExe = Join-Path $installRoot "BenchScope.exe"
if (-not (Test-Path -LiteralPath $statePath) -and -not (Test-Path -LiteralPath $manifestPath) -and -not (Test-Path -LiteralPath $mainExe)) {
    throw "Refusing to uninstall because $installRoot does not look like a BenchScope install."
}

$installState = $null
if (Test-Path -LiteralPath $statePath) {
    $installState = Get-Content -Raw -LiteralPath $statePath | ConvertFrom-Json
    if ($installState.scope) {
        $Scope = [string]$installState.scope
    }
}

if ($Scope -eq "Machine" -and -not (Test-IsAdministrator)) {
    throw "Machine-scope uninstall requires an elevated PowerShell session. Use -Scope CurrentUser for a per-user install."
}

Stop-OrBlockRunningBenchScope -InstallRoot $installRoot

$manifestFiles = @()
if (Test-Path -LiteralPath $manifestPath) {
    $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
    $manifestFiles = @($manifest.files)
}

$managedPaths = New-Object System.Collections.Generic.List[string]
foreach ($file in $manifestFiles) {
    if (-not $RemoveReports -and $file.path -like "reports\*") {
        continue
    }

    $managedPaths.Add([string]$file.path) | Out-Null
}

foreach ($extra in @("bundle-manifest.json", "install-state.json", "Install-BenchScope.ps1", "Uninstall-BenchScope.ps1")) {
    $managedPaths.Add($extra) | Out-Null
}

if ($RemoveReports) {
    $reportsPath = Join-Path $installRoot "reports"
    if (Test-Path -LiteralPath $reportsPath) {
        foreach ($reportFile in @(Get-ChildItem -LiteralPath $reportsPath -File -Recurse -Force)) {
            $relative = $reportFile.FullName.Substring($installRoot.TrimEnd("\").Length).TrimStart("\")
            $managedPaths.Add($relative) | Out-Null
        }
    }
}

if ($PSCmdlet.ShouldProcess($installRoot, "Uninstall BenchScope")) {
    foreach ($shortcutPath in Get-ShortcutPaths -RequestedScope $Scope -InstallState $installState) {
        if (Test-Path -LiteralPath $shortcutPath) {
            Remove-Item -LiteralPath $shortcutPath -Force
        }

        $shortcutDir = Split-Path -Parent $shortcutPath
        if ($shortcutDir -and (Split-Path -Leaf $shortcutDir) -eq "BenchScope" -and (Test-Path -LiteralPath $shortcutDir)) {
            if (@(Get-ChildItem -LiteralPath $shortcutDir -Force).Count -eq 0) {
                Remove-Item -LiteralPath $shortcutDir -Force
            }
        }
    }

    foreach ($relativePath in @($managedPaths | Select-Object -Unique)) {
        $target = Resolve-ChildPath -BasePath $installRoot -RelativePath $relativePath
        if (Test-Path -LiteralPath $target -PathType Leaf) {
            Remove-Item -LiteralPath $target -Force
        }
    }

    if ($RemoveReports) {
        $reportsPath = Join-Path $installRoot "reports"
        if (Test-Path -LiteralPath $reportsPath) {
            Remove-Item -LiteralPath $reportsPath -Recurse -Force
        }
    }

    $preserve = @()
    if (-not $RemoveReports) {
        $preserve = @("reports")
    }

    Remove-EmptyDirectories -Root $installRoot -PreserveRelativeRoots $preserve
}

if (Test-Path -LiteralPath $installRoot) {
    if ($RemoveReports) {
        Write-Host "BenchScope uninstall completed. Install directory remains because it still contains non-managed files: $installRoot"
    } else {
        Write-Host "BenchScope uninstall completed. Reports were preserved under: $installRoot"
    }
} else {
    Write-Host "BenchScope uninstall completed and install directory was removed."
}
