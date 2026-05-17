[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$BundlePath = "",
    [ValidateSet("Machine", "CurrentUser")]
    [string]$Scope = "Machine",
    [string]$InstallDir = "",
    [switch]$InstallVcRedist,
    [switch]$CreateDesktopShortcut,
    [switch]$NoStartMenuShortcut,
    [switch]$RunPrereqCheck,
    [switch]$StopRunningBenchScope,
    [switch]$SkipManifestVerification
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

function Get-VcRuntimeState {
    $keys = @(
        "HKLM:\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64",
        "HKLM:\SOFTWARE\WOW6432Node\Microsoft\VisualStudio\14.0\VC\Runtimes\x64"
    )

    foreach ($key in $keys) {
        if (Test-Path -LiteralPath $key) {
            $props = Get-ItemProperty -LiteralPath $key
            if ($props.Installed -eq 1) {
                return [pscustomobject]@{
                    Found = $true
                    Detail = "Installed runtime registry key found at $key."
                }
            }
        }
    }

    $dllPath = Join-Path $env:WINDIR "System32\VCRUNTIME140.dll"
    if (Test-Path -LiteralPath $dllPath) {
        return [pscustomobject]@{
            Found = $true
            Detail = "Runtime DLL found at $dllPath."
        }
    }

    return [pscustomobject]@{
        Found = $false
        Detail = "Microsoft Visual C++ 2015-2022 x64 runtime was not detected."
    }
}

function Invoke-VcRedistInstall {
    param([Parameter(Mandatory = $true)][string]$BundleRoot)

    $runtime = Get-VcRuntimeState
    if ($runtime.Found) {
        Write-Host "VC++ runtime already present. $($runtime.Detail)"
        return
    }

    if (-not (Test-IsAdministrator)) {
        throw "Installing the VC++ runtime requires an elevated PowerShell session."
    }

    $redistPath = Join-Path $BundleRoot "installers\vc_redist.x64.exe"
    if (-not (Test-Path -LiteralPath $redistPath)) {
        throw "VC++ runtime is missing and $redistPath was not bundled. See docs\FIRST_BOOT_LINKS.md for the official download link."
    }

    Write-Host "Installing Microsoft Visual C++ 2015-2022 x64 Redistributable..."
    $process = Start-Process -FilePath $redistPath -ArgumentList @("/install", "/quiet", "/norestart") -Wait -PassThru -WindowStyle Hidden
    if ($process.ExitCode -notin @(0, 1638, 3010)) {
        throw "vc_redist.x64.exe failed with exit code $($process.ExitCode)."
    }

    if ($process.ExitCode -eq 3010) {
        Write-Warning "VC++ runtime installed and requested a reboot."
    }
}

function New-BenchScopeShortcut {
    param(
        [Parameter(Mandatory = $true)][string]$ShortcutPath,
        [Parameter(Mandatory = $true)][string]$TargetPath,
        [Parameter(Mandatory = $true)][string]$WorkingDirectory
    )

    $shortcutDir = Split-Path -Parent $ShortcutPath
    if ($shortcutDir -and -not (Test-Path -LiteralPath $shortcutDir)) {
        New-Item -ItemType Directory -Path $shortcutDir -Force | Out-Null
    }

    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($ShortcutPath)
    $shortcut.TargetPath = $TargetPath
    $shortcut.WorkingDirectory = $WorkingDirectory
    $shortcut.Description = "BenchScope hardware benchmark and diagnostic tool"
    $shortcut.IconLocation = "$TargetPath,0"
    $shortcut.Save()
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
        throw "BenchScope processes are running from this install and must be closed before update: $details. Rerun with -StopRunningBenchScope to stop them."
    }

    foreach ($process in $running) {
        Stop-Process -Id $process.Id -Force
    }
}

function Remove-StaleManagedFiles {
    param(
        [Parameter(Mandatory = $true)][string]$InstallRoot,
        [Parameter(Mandatory = $true)][object[]]$NewManifestFiles
    )

    $existingManifestPath = Join-Path $InstallRoot "bundle-manifest.json"
    if (-not (Test-Path -LiteralPath $existingManifestPath)) {
        return
    }

    $existingManifest = Get-Content -Raw -LiteralPath $existingManifestPath | ConvertFrom-Json
    $existingFiles = @($existingManifest.files)
    if ($existingFiles.Count -eq 0) {
        return
    }

    $newPaths = New-Object "System.Collections.Generic.HashSet[string]" ([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($file in $NewManifestFiles) {
        $newPaths.Add([string]$file.path) | Out-Null
    }

    foreach ($file in $existingFiles) {
        $relativePath = [string]$file.path
        if ($relativePath -like "reports\*") {
            continue
        }

        if (-not $newPaths.Contains($relativePath)) {
            $target = Resolve-ChildPath -BasePath $InstallRoot -RelativePath $relativePath
            if (Test-Path -LiteralPath $target -PathType Leaf) {
                Remove-Item -LiteralPath $target -Force
            }
        }
    }

    $dirs = @(Get-ChildItem -LiteralPath $InstallRoot -Directory -Recurse -Force | Sort-Object FullName -Descending)
    foreach ($dir in $dirs) {
        $relative = $dir.FullName.Substring($InstallRoot.TrimEnd("\").Length).TrimStart("\")
        if ($relative -eq "reports" -or $relative.StartsWith("reports\", [System.StringComparison]::OrdinalIgnoreCase)) {
            continue
        }

        if (@(Get-ChildItem -LiteralPath $dir.FullName -Force).Count -eq 0) {
            Remove-Item -LiteralPath $dir.FullName -Force
        }
    }
}

function Remove-StaleShortcut {
    param(
        [string]$ExistingPath,
        [string]$DesiredPath
    )

    if (-not $ExistingPath) {
        return
    }

    if ($DesiredPath -and $ExistingPath.Equals($DesiredPath, [System.StringComparison]::OrdinalIgnoreCase)) {
        return
    }

    if (Test-Path -LiteralPath $ExistingPath) {
        Remove-Item -LiteralPath $ExistingPath -Force
    }

    $shortcutDir = Split-Path -Parent $ExistingPath
    if ($shortcutDir -and (Split-Path -Leaf $shortcutDir) -eq "BenchScope" -and (Test-Path -LiteralPath $shortcutDir)) {
        if (@(Get-ChildItem -LiteralPath $shortcutDir -Force).Count -eq 0) {
            Remove-Item -LiteralPath $shortcutDir -Force
        }
    }
}

function Remove-StaleShortcuts {
    param(
        [Parameter(Mandatory = $true)][string]$InstallRoot,
        [string]$DesiredStartMenuShortcut,
        [string]$DesiredDesktopShortcut
    )

    $statePath = Join-Path $InstallRoot "install-state.json"
    if (-not (Test-Path -LiteralPath $statePath)) {
        return
    }

    try {
        $state = Get-Content -Raw -LiteralPath $statePath | ConvertFrom-Json
    } catch {
        return
    }

    if ($state.PSObject.Properties.Name -contains "startMenuShortcut") {
        Remove-StaleShortcut -ExistingPath ([string]$state.startMenuShortcut) -DesiredPath $DesiredStartMenuShortcut
    }

    if ($state.PSObject.Properties.Name -contains "desktopShortcut") {
        Remove-StaleShortcut -ExistingPath ([string]$state.desktopShortcut) -DesiredPath $DesiredDesktopShortcut
    }
}

if (-not $BundlePath) {
    $BundlePath = $PSScriptRoot
}

$bundleRoot = Resolve-InputPath $BundlePath
if (-not (Test-Path -LiteralPath $bundleRoot)) {
    throw "Bundle path does not exist: $bundleRoot"
}

$mainSourceExe = Join-Path $bundleRoot "BenchScope.exe"
if (-not (Test-Path -LiteralPath $mainSourceExe)) {
    throw "BenchScope.exe was not found in bundle root: $bundleRoot"
}

if (-not $InstallDir) {
    if ($Scope -eq "Machine") {
        $InstallDir = Join-Path $env:ProgramFiles "BenchScope"
    } else {
        $InstallDir = Join-Path $env:LOCALAPPDATA "BenchScope"
    }
}

$installRoot = Resolve-InputPath $InstallDir

if ($Scope -eq "Machine" -and -not (Test-IsAdministrator)) {
    throw "Machine-scope installation requires an elevated PowerShell session. Use -Scope CurrentUser for a per-user install."
}

$manifestPath = Join-Path $bundleRoot "bundle-manifest.json"
if (-not (Test-Path -LiteralPath $manifestPath)) {
    throw "Bundle manifest is missing: $manifestPath"
}

$manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
$manifestFiles = @($manifest.files)
if ($manifestFiles.Count -eq 0) {
    throw "Bundle manifest does not list any files."
}

if (-not $SkipManifestVerification) {
    Write-Host "Verifying bundle manifest..."
    $previousWhatIfPreference = $WhatIfPreference
    $WhatIfPreference = $false
    try {
        foreach ($file in $manifestFiles) {
            $sourcePath = Resolve-ChildPath -BasePath $bundleRoot -RelativePath $file.path
            if (-not (Test-Path -LiteralPath $sourcePath)) {
                throw "Manifest file is missing: $($file.path)"
            }

            $item = Get-Item -LiteralPath $sourcePath
            if ($item.Length -ne [int64]$file.bytes) {
                throw "Manifest byte count mismatch for $($file.path). Expected $($file.bytes), found $($item.Length)."
            }

            $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $sourcePath).Hash
            if ($hash -ne $file.sha256) {
                throw "Manifest SHA-256 mismatch for $($file.path)."
            }
        }
    } finally {
        $WhatIfPreference = $previousWhatIfPreference
    }
}

$installed = $false
if ($PSCmdlet.ShouldProcess($installRoot, "Install BenchScope bundle")) {
    if ($InstallVcRedist) {
        Invoke-VcRedistInstall -BundleRoot $bundleRoot
    }

    if (Test-Path -LiteralPath $installRoot) {
        Stop-OrBlockRunningBenchScope -InstallRoot $installRoot
    }

    New-Item -ItemType Directory -Path $installRoot -Force | Out-Null
    Remove-StaleManagedFiles -InstallRoot $installRoot -NewManifestFiles $manifestFiles

    $startMenuShortcut = ""
    if (-not $NoStartMenuShortcut) {
        if ($Scope -eq "Machine") {
            $startMenuShortcut = Join-Path $env:ProgramData "Microsoft\Windows\Start Menu\Programs\BenchScope\BenchScope.lnk"
        } else {
            $startMenuShortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\BenchScope\BenchScope.lnk"
        }
    }

    $desktopShortcut = ""
    if ($CreateDesktopShortcut) {
        if ($Scope -eq "Machine") {
            $desktopShortcut = Join-Path $env:PUBLIC "Desktop\BenchScope.lnk"
        } else {
            $desktopShortcut = Join-Path $env:USERPROFILE "Desktop\BenchScope.lnk"
        }
    }

    Remove-StaleShortcuts -InstallRoot $installRoot -DesiredStartMenuShortcut $startMenuShortcut -DesiredDesktopShortcut $desktopShortcut

    foreach ($file in $manifestFiles) {
        if ($file.path -like "reports\*") {
            continue
        }

        $sourcePath = Resolve-ChildPath -BasePath $bundleRoot -RelativePath $file.path
        $destinationPath = Resolve-ChildPath -BasePath $installRoot -RelativePath $file.path
        $destinationDir = Split-Path -Parent $destinationPath
        if ($destinationDir -and -not (Test-Path -LiteralPath $destinationDir)) {
            New-Item -ItemType Directory -Path $destinationDir -Force | Out-Null
        }

        Copy-Item -LiteralPath $sourcePath -Destination $destinationPath -Force
    }

    Copy-Item -LiteralPath $manifestPath -Destination (Join-Path $installRoot "bundle-manifest.json") -Force

    $localInstaller = Join-Path $bundleRoot "Install-BenchScope.ps1"
    if (Test-Path -LiteralPath $localInstaller) {
        Copy-Item -LiteralPath $localInstaller -Destination (Join-Path $installRoot "Install-BenchScope.ps1") -Force
    }

    if ($startMenuShortcut) {
        New-BenchScopeShortcut -ShortcutPath $startMenuShortcut -TargetPath (Join-Path $installRoot "BenchScope.exe") -WorkingDirectory $installRoot
    }

    if ($desktopShortcut) {
        New-BenchScopeShortcut -ShortcutPath $desktopShortcut -TargetPath (Join-Path $installRoot "BenchScope.exe") -WorkingDirectory $installRoot
    }

    $installState = [pscustomobject]@{
        name = "BenchScope"
        version = $manifest.version
        scope = $Scope
        installRoot = $installRoot
        installedAtUtc = (Get-Date).ToUniversalTime().ToString("o")
        sourceBundle = $bundleRoot
        startMenuShortcut = $startMenuShortcut
        desktopShortcut = $desktopShortcut
    }
    $installState | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $installRoot "install-state.json") -Encoding UTF8

    if ($RunPrereqCheck) {
        $checker = Join-Path $installRoot "tools\Test-WindowsRuntimePrereqs.ps1"
        if (Test-Path -LiteralPath $checker) {
            $reportPath = Join-Path $installRoot "reports\install-prereqs.md"
            & $checker -BundlePath $installRoot -OutputPath $reportPath -Quiet
            if ($LASTEXITCODE -ne 0) {
                throw "Installed prerequisite report failed with exit code $LASTEXITCODE."
            }
        } else {
            Write-Warning "Prerequisite checker was not installed at $checker."
        }
    }

    $installed = $true
}

if ($installed) {
    Write-Host "BenchScope installed to: $installRoot"
    if (-not $InstallVcRedist) {
        Write-Host "VC++ runtime repair was not requested. Use -InstallVcRedist with a bundle that includes installers\vc_redist.x64.exe if repair is needed."
    }
} else {
    Write-Host "BenchScope install was not applied."
}
