@echo off
setlocal
cd /d "%~dp0\.."

set "CARGO=%USERPROFILE%\.cargo\bin\cargo.exe"
if not exist "%CARGO%" set "CARGO=cargo"

set "MAKENSIS=C:\Program Files (x86)\NSIS\makensis.exe"
if not exist "%MAKENSIS%" set "MAKENSIS=C:\Program Files\NSIS\makensis.exe"
if not exist "%MAKENSIS%" set "MAKENSIS=makensis"

echo Building TraceBit release...
"%CARGO%" build --release
if errorlevel 1 exit /b 1
if not exist dist mkdir dist

echo Building NSIS installer...
"%MAKENSIS%" installer.nsi
if errorlevel 1 exit /b 1

echo Installer created in dist\
