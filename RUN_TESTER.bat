@echo off
cd /d "%~dp0"
if exist "%~dp0target\release\hardware_accel_tester.exe" (
  "%~dp0target\release\hardware_accel_tester.exe"
) else (
  "%USERPROFILE%\.cargo\bin\cargo.exe" run --release
)
