@echo off
cd /d "%~dp0"
if not exist Goat.exe call build.bat & exit /b
start "" "%~dp0Goat.exe" %*
