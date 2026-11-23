@echo off
setlocal
cd /d "%~dp0\.."
title NEO build
echo Building NEO desktop...
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build.ps1" %*
set ERR=%ERRORLEVEL%
if %ERR% neq 0 (
  echo.
  echo Build failed with exit %ERR%.
  pause
  exit /b %ERR%
)
echo.
if /I "%~1"=="-Release" (
  echo Done. Output: out\neo-desktop\release\neo-desktop.exe
) else (
  echo Done. Output: out\neo-desktop\debug\neo-desktop.exe
)
echo Release: scripts\build.cmd -Release
echo Build and run: scripts\build.cmd -Run
pause
exit /b 0
