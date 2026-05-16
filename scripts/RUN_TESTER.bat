@echo off
set "REPO_ROOT=%~dp0.."
cd /d "%REPO_ROOT%"
if exist "%REPO_ROOT%\target\release\benchscope.exe" (
  "%REPO_ROOT%\target\release\benchscope.exe"
) else (
  "%USERPROFILE%\.cargo\bin\cargo.exe" run --release
)
