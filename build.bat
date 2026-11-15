@echo off
setlocal
cd /d "%~dp0"
where cargo >nul 2>nul
if errorlevel 1 (
  echo Rust is not installed. Get it from https://rustup.rs ^(choose the default MSVC option^), then run this again.
  pause
  exit /b 1
)
echo Building Goat ^(release^)... first build takes a few minutes.
cargo build --release
if errorlevel 1 (
  echo.
  echo Build failed. If it mentions link.exe, install "Visual Studio Build Tools" with the "Desktop development with C++" workload.
  pause
  exit /b 1
)
copy /y "target\release\goat.exe" "Goat.exe" >nul
echo.
echo Done: %~dp0Goat.exe
start "" "%~dp0Goat.exe"
