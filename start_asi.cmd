@echo off
setlocal
cd /d "%~dp0"
powershell -NoLogo -ExecutionPolicy Bypass -File ".\start_asi.ps1"
exit /b %ERRORLEVEL%
