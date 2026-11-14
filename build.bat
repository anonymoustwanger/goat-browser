@echo off
setlocal
cd /d "%~dp0"
where cargo >nul 2>nul
if errorlevel 1 (
  echo Rust is not installed. Get it from https://rustup.rs ^(choose the default MSVC option^), then run this again.
  pause
  exit /b 1
)
echo Building Goatr ^(release^)... first build takes a few minutes.
cargo build --release
if errorlevel 1 (
  echo.
  echo Build failed. If it mentions link.exe, install "Visual Studio Build Tools" with the "Desktop development with C++" workload.
  pause
  exit /b 1
)
copy /y "target\release\goatr.exe" "Goatr.exe" >nul
echo.
echo Done: %~dp0Goatr.exe
start "" "%~dp0Goatr.exe"
