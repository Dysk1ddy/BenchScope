@echo off
setlocal EnableExtensions
set "REPO_ROOT=%~dp0.."
cd /d "%REPO_ROOT%"
for %%I in ("%REPO_ROOT%\..\.cargo-target\BenchScope") do set "TARGET_ROOT=%%~fI"
set "BENCHSCOPE_EXE=%TARGET_ROOT%\release\BenchScope.exe"
set "LEGACY_BENCHSCOPE_EXE=%REPO_ROOT%\target\release\BenchScope.exe"
set "LAUNCH_DIR=%REPO_ROOT%\target\run"
set "LAUNCH_EXE=%LAUNCH_DIR%\BenchScope.exe"
set "CARGO=%USERPROFILE%\.cargo\bin\cargo.exe"
if exist "%CARGO%" goto Build
where cargo.exe >nul 2>nul
if not errorlevel 1 (
    set "CARGO=cargo.exe"
    goto Build
)
echo Rust/Cargo was not found. Installing Rust toolchain...
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%REPO_ROOT%\scripts\Bootstrap-Developer.ps1" -InstallRust
if errorlevel 1 exit /b %errorlevel%
set "CARGO=%USERPROFILE%\.cargo\bin\cargo.exe"
if exist "%CARGO%" goto Build
where cargo.exe >nul 2>nul
if not errorlevel 1 (
    set "CARGO=cargo.exe"
    goto Build
)
echo cargo.exe is still unavailable after the Rust install. Open a new terminal and rerun this launcher.
exit /b 1

:Build
"%CARGO%" build --release --bin BenchScope
if not errorlevel 1 goto PrepareLaunch
set "BUILD_EXIT=%ERRORLEVEL%"
if exist "%LAUNCH_EXE%" (
    echo Build failed. Launching the last successful BenchScope run copy.
    goto Launch
)
if exist "%BENCHSCOPE_EXE%" (
    echo Build failed. Launching the last successful BenchScope release build.
    set "LAUNCH_EXE=%BENCHSCOPE_EXE%"
    goto Launch
)
exit /b %BUILD_EXIT%

:PrepareLaunch
if not exist "%BENCHSCOPE_EXE%" exit /b 1
if not exist "%REPO_ROOT%\target\release" mkdir "%REPO_ROOT%\target\release"
copy /Y "%BENCHSCOPE_EXE%" "%LEGACY_BENCHSCOPE_EXE%" >nul 2>nul
if not exist "%LAUNCH_DIR%" mkdir "%LAUNCH_DIR%"
set "LAUNCH_EXE=%LAUNCH_DIR%\BenchScope.exe"
copy /Y "%BENCHSCOPE_EXE%" "%LAUNCH_EXE%" >nul 2>nul
if not errorlevel 1 goto Launch
set "LAUNCH_EXE=%LAUNCH_DIR%\BenchScope-%RANDOM%-%RANDOM%.exe"
copy /Y "%BENCHSCOPE_EXE%" "%LAUNCH_EXE%" >nul
if errorlevel 1 exit /b %ERRORLEVEL%

:Launch
if "%BENCHSCOPE_BUILD_ONLY%"=="1" (
    echo Built "%LAUNCH_EXE%"
    exit /b 0
)
"%LAUNCH_EXE%"
