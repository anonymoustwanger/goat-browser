; Goatr setup bootstrapper (NSIS 3). Build: npm run installer
; It only unpacks Goatr to a temp folder and starts Goatr's own installer screen
; (GoatBrowser.exe --goat-setup, see src/setup.js), which copies Goatr into
; %LOCALAPPDATA%\Programs\Goatr, adds shortcuts, and registers it with Windows.
; Uninstalling (Windows "Apps" or Goatr Settings > About) runs GoatBrowser.exe --goat-uninstall.

Unicode true
!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!define SRC "..\dist\Goatr-win32-x64"

Name "Goatr"
OutFile "..\dist\GoatBrowserSetup-${VERSION}.exe"
Icon "..\build\icon.ico"
RequestExecutionLevel user
SilentInstall silent
SetCompressor /SOLID lzma
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "Goatr"
VIAddVersionKey "FileDescription" "Goatr setup"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "Goatr"

Section
  InitPluginsDir
  Banner::show /NOUNLOAD /set 76 "Goatr" "Getting setup ready…"
  SetOutPath "$PLUGINSDIR\goat"
  File /r "${SRC}\*.*"
  Banner::destroy
  ExecWait '"$PLUGINSDIR\goat\GoatBrowser.exe" --goat-setup' $0
  SetOutPath "$TEMP" ; leave the folder so it can be cleaned up
SectionEnd
