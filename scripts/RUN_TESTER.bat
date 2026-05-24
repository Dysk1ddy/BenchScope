@echo off
set "REPO_ROOT=%~dp0.."
cd /d "%REPO_ROOT%"
for %%I in ("%REPO_ROOT%\..\.cargo-target\BenchScope") do set "TARGET_ROOT=%%~fI"
set "BENCHSCOPE_EXE=%TARGET_ROOT%\release\BenchScope.exe"
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
"%CARGO%" build --release
if errorlevel 1 (
    if exist "%BENCHSCOPE_EXE%" (
        echo Build failed. Launching the last successful BenchScope release build.
    ) else (
        exit /b %errorlevel%
    )
)
"%BENCHSCOPE_EXE%"
