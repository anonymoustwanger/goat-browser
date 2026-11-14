@echo off
cd /d "%~dp0"
if not exist Goatr.exe call build.bat & exit /b
start "" "%~dp0Goatr.exe" %*
