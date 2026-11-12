@echo off
setlocal
cd /d "%~dp0"
set "CARGO=%USERPROFILE%\.cargo\bin\cargo.exe"
if not exist "%CARGO%" set "CARGO=cargo"
"%CARGO%" run
if errorlevel 1 pause
