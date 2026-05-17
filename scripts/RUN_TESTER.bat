@echo off
set "REPO_ROOT=%~dp0.."
cd /d "%REPO_ROOT%"
set "CARGO=%USERPROFILE%\.cargo\bin\cargo.exe"
if not exist "%CARGO%" set "CARGO=cargo"
"%CARGO%" build --release
if errorlevel 1 exit /b %errorlevel%
"%REPO_ROOT%\target\release\BenchScope.exe"
