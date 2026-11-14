# Goatr (formerly Ferrite) – where we left off (2026-11-14)

Project: Rust browser in `D:\claude d drive\new-goat-browser`.
Goal: replace the Electron "Goat Browser" (`D:\claude d drive\goat`) with a Rust version.
Working copy in the cloud session: /home/claude/ferrite (newer than the D drive copy).

## Done and deployed
- Goat-style settings/history/bookmarks/downloads/new tab pages (src/web/*.css copied from Goat).
- Pop-ups and tabs no longer animate out of bounds.
- Privacy from Goat: encrypted DNS providers, HTTPS-only, tracking-link cleaning, referrer trimming,
  third-party cookies, no preloading, font blocking, fingerprint shield, passkey control, profiles.
- Extension install fixed (Windows cert store; store "Add to Chrome" button caught).
- Freeze diagnostics: data\crash.log, data\debug.log, watchdog, page-unresponsive auto reload.

## Done 2026-11-14 (deployed to D:\claude d drive\goatr-browser\Goatr.exe)
- Renamed to Goatr (folder, exe, scheme goatr://, data file goatr-data.json with migration).
- Pop-ups shown only once placed, Windows open animation off; Goat fade+scale animations.
- Theme editor (create/edit/delete custom themes), Performance (CPU cores, priority, memory
  saver), Proxy (system/none/manual) sections.
- Page script errors on internal pages go to data\debug.log.

## Still to do
- Ad blocker freeze: unresolved; ask user to reproduce, then read data\debug.log / crash.log.
- Not ported from Goat (bigger features): local profiles, translate, cookie manager,
  "forget when closed", tab folders, languages.

## Build / deploy
- Windows exe: `RUSTC_BOOTSTRAP=1 cargo build --release -Zbuild-std=std,panic_abort --target x86_64-pc-windows-gnu`,
  strip with x86_64-w64-mingw32-strip, ship with WebView2Loader.dll.
- Check: `RUSTC_BOOTSTRAP=1 cargo check -Zbuild-std=std,panic_abort --target x86_64-pc-windows-msvc`.
- Linux test under Xvfb :99 + openbox.
