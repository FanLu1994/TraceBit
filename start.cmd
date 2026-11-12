@echo off
setlocal
cd /d "%~dp0"
rem Some agent terminals export this for Node debugging; it must not affect Electron.
set ELECTRON_RUN_AS_NODE=
if not exist node_modules\electron\dist\electron.exe (
  echo Installing TraceBit dependencies...
  call npm install
  if errorlevel 1 exit /b 1
)
call npm start
