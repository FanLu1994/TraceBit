@echo off
setlocal
cd /d "%~dp0\.."
set "CARGO=%USERPROFILE%\.cargo\bin\cargo.exe"
if not exist "%CARGO%" set "CARGO=cargo"

"%CARGO%" test
if errorlevel 1 exit /b 1
"%CARGO%" run -- --native-smoke
if errorlevel 1 exit /b 1
