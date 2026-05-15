@echo off
cd /d "%~dp0"
if exist "%~dp0target\release\benchscope.exe" (
  "%~dp0target\release\benchscope.exe"
) else (
  "%USERPROFILE%\.cargo\bin\cargo.exe" run --release
)
