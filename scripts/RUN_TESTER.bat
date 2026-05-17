@echo off
set "REPO_ROOT=%~dp0.."
cd /d "%REPO_ROOT%"
for %%I in ("%REPO_ROOT%\..\.cargo-target\BenchScope") do set "TARGET_ROOT=%%~fI"
set "BENCHSCOPE_EXE=%TARGET_ROOT%\release\BenchScope.exe"
set "CARGO=%USERPROFILE%\.cargo\bin\cargo.exe"
if not exist "%CARGO%" set "CARGO=cargo"
"%CARGO%" build --release
if errorlevel 1 (
    if exist "%BENCHSCOPE_EXE%" (
        echo Build failed. Launching the last successful BenchScope release build.
    ) else (
        exit /b %errorlevel%
    )
)
"%BENCHSCOPE_EXE%"
