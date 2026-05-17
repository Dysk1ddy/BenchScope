param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Debug",
    [ValidateSet("x64")]
    [string]$Platform = "x64",
    [string]$WindowsTargetPlatformVersion = "10.0.28000.0",
    [ValidateSet("Off", "TestSign", "ProductionSign")]
    [string]$SignMode = "Off"
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$project = Join-Path $repoRoot "sensor-driver\BenchScopeSensorDriver.vcxproj"

$candidateRoots = @(
    "${env:ProgramFiles}\Microsoft Visual Studio\2022\Community",
    "${env:ProgramFiles}\Microsoft Visual Studio\2022\Professional",
    "${env:ProgramFiles}\Microsoft Visual Studio\2022\Enterprise",
    "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2022\BuildTools",
    "${env:ProgramFiles(x86)}\Microsoft Visual Studio\18\BuildTools"
) | Where-Object { $_ -and (Test-Path -LiteralPath $_) }

$vsRoot = $candidateRoots | Where-Object {
    (Test-Path -LiteralPath (Join-Path $_ "VC\Auxiliary\Build\vcvars64.bat")) -and
    (Test-Path -LiteralPath (Join-Path $_ "MSBuild\Current\Bin\amd64\MSBuild.exe"))
} | Select-Object -First 1

if (-not $vsRoot) {
    throw "No usable Visual Studio C++/MSBuild installation was found."
}

$vcvars = Join-Path $vsRoot "VC\Auxiliary\Build\vcvars64.bat"
$msbuild = Join-Path $vsRoot "MSBuild\Current\Bin\amd64\MSBuild.exe"
$wdkRoot = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10"
$wdkBuildRoot = Join-Path $wdkRoot "build\$WindowsTargetPlatformVersion"
$wdkBinX86 = Join-Path $wdkRoot "bin\$WindowsTargetPlatformVersion\x86"

if (-not (Test-Path -LiteralPath (Join-Path $wdkBuildRoot "WindowsDriver.common.targets"))) {
    throw "WDK build targets were not found for $WindowsTargetPlatformVersion."
}

$visualStudioVersion = "17.0"
if ((Test-Path -LiteralPath (Join-Path $wdkBuildRoot "bin\Microsoft.DriverKit.Build.Tasks.18.0.dll")) -and
    -not (Test-Path -LiteralPath (Join-Path $wdkBuildRoot "bin\Microsoft.DriverKit.Build.Tasks.17.0.dll"))) {
    $visualStudioVersion = "18.0"
}

$extraSigningProperties = ""
if ($SignMode -eq "Off") {
    $extraSigningProperties = " /p:GenerateTestCertificate=false"
}

$cmd = @(
    "call `"$vcvars`" $WindowsTargetPlatformVersion",
    "set `"Path=$wdkBinX86;!Path!`"",
    "`"$msbuild`" `"$project`" /m:1 /p:Configuration=$Configuration /p:Platform=$Platform /p:WindowsTargetPlatformVersion=$WindowsTargetPlatformVersion /p:VisualStudioVersion=$visualStudioVersion /p:SignMode=$SignMode$extraSigningProperties /v:minimal"
) -join " && "

$basePath = [System.Environment]::GetEnvironmentVariable("Path")
if (-not $basePath) {
    $basePath = [System.Environment]::GetEnvironmentVariable("PATH")
}

$psi = [System.Diagnostics.ProcessStartInfo]::new()
$psi.FileName = Join-Path $env:SystemRoot "System32\cmd.exe"
$psi.Arguments = "/v:on /d /s /c `"$cmd`""
$psi.WorkingDirectory = $repoRoot
$psi.UseShellExecute = $false
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.Environment.Clear()

foreach ($entry in [System.Environment]::GetEnvironmentVariables().GetEnumerator()) {
    if ($entry.Key -notmatch "^(?i:path)$") {
        $psi.Environment[$entry.Key] = [string]$entry.Value
    }
}

$psi.Environment["Path"] = $basePath
$psi.Environment["VSCMD_SKIP_SENDTELEMETRY"] = "1"

$process = [System.Diagnostics.Process]::Start($psi)
$stdout = $process.StandardOutput.ReadToEnd()
$stderr = $process.StandardError.ReadToEnd()
$process.WaitForExit()

Write-Host $stdout
if ($stderr) {
    Write-Error $stderr
}

exit $process.ExitCode
