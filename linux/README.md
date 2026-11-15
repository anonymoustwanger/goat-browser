# Goat

A private, fast browser built on Chromium 156 via Electron 45.

**What it's built on:** Electron bundles Chromium (Chrome's open-source engine: Blink for rendering, V8 for JavaScript) plus Node.js. Goat's own code is the toolbar, tabs, folders, settings, ad blocker wiring and privacy rules. Google's services (sync, Safe Browsing, telemetry) are not included.

**app.asar:** in `dist\Goat-win32-x64\resources\`. It's an archive of Goat's code (`src\` plus `node_modules`) that Electron reads at startup. `GoatBrowser.exe` and the DLLs are the Chromium/Electron engine. Updating Goat usually only means replacing `app.asar`.

## Run

- **Installer:** `dist\GoatBrowserSetup-<version>.exe`. Opens Goat's own setup screen: desktop shortcut, default-browser and location options, progress, then "Open Goat". Installs for your user only (no admin) into `%LOCALAPPDATA%\Programs\Goat`, adds a Start menu shortcut, an entry in Windows "Apps", and lists Goat in Default apps. Running a newer installer updates Goat and keeps your profile.
- **Uninstall:** Windows Settings → Apps → Goat → Uninstall, or Goat Settings → About → Uninstall. Goat's uninstall screen asks whether to also delete your profile. Build it with `npm run installer` (needs [NSIS](https://nsis.sourceforge.io/) installed).
- **Linux:** `dist/GoatBrowser-<version>-x86_64.AppImage`. Make it executable (`chmod +x`, or right-click → Properties → allow executing) and double-click. Build with `npm run appimage` on Linux (needs [appimagetool](https://github.com/AppImage/appimagetool) on PATH or in `$APPIMAGETOOL`). Profile: `~/.config/Goat`.
  <a id="linux"></a>Chromium's sandbox needs unprivileged user namespaces. On Ubuntu 24.04+ they're blocked by AppArmor, so the AppImage starts without the sandbox and some extensions may not fully work. To keep the sandbox, run `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` (add it to `/etc/sysctl.d/` to keep it after reboot).
- **Ready-made:** `dist\Goat-win32-x64\GoatBrowser.exe`. Windows SmartScreen may warn because the exe is unsigned: choose "More info", then "Run anyway".
- **From source:** `npm install`, then `npm start`. Repackage with `npm run package:win`.

Your profile (cookies, settings, open tabs, blocklists) lives in `%APPDATA%\Goat`.

## Features

- **Ad blocker.** Ghostery engine with network and cosmetic filtering. Three levels: Ads only, Standard (ads + trackers), Strict (adds cookie banners and annoyances). Supports custom filter rules and a per-site allowlist. Extensions' own requests skip the ad blocker by default (Settings → Ad blocker → "Don't block extensions"), so it can't break them. Click the shield in the toolbar for the Goat Shield panel: blocked count, per-site switch, level picker (Ads / Standard / Strict), and links to allowed sites and custom filters.
- **Extensions menu.** New extensions start unpinned, like Chrome. The puzzle button in the toolbar lists your extensions: click one to open it, pin or unpin it from the toolbar (also: right-click a toolbar button, or the Toolbar switch in Settings → Extensions), open its options, or manage extensions.
- **Extension compatibility.** Native messaging works with desktop apps like iCloud Passwords, 1Password and Bitwarden (hosts registered for Chrome, Edge, Chromium or Brave are found). VPN extensions like Proton VPN work: `chrome.proxy` (fixed servers and PAC scripts), proxy logins via `webRequest.onAuthRequired`, sign-in pages that message their extension (`externally_connectable` / `runtime.onMessageExternal`), and Chrome-style host-permission matching in `chrome.permissions`. `chrome.privacy`, `chrome.idle`, `chrome.declarativeContent` and `chrome.sidePanel` have safe stand-ins. A VPN extension only affects normal tabs; Tor overrides it. Patches live in `scripts/patch-extensions.js` and are applied on `npm install`.
- **Extensions.** Install from the Chrome Web Store (Settings → Extensions → "Get extensions"), or load an unpacked folder. Extension buttons sit next to the shield. Most extensions work; a few Chrome APIs (like `chrome.commands`) are missing. Powered by electron-chrome-extensions.
- **Cookies persist.** Logins survive restarts. Third-party cookies are blocked (toggle in Settings). Optional "delete cookies on exit".
- **Cookie manager.** Settings → Cookies. Search, view, edit values, delete a cookie, a whole site, or everything.
- **Proxy.** System, none, manual (HTTP / HTTPS / SOCKS4 / SOCKS5, with bypass list and HTTP proxy login), or PAC URL.
- **Folders (tab groups).** Drag a tab onto the middle of another tab to make a folder. Drop more tabs on a folder or its tabs to add them. Drop on a tab's edge to reorder instead. Click a folder to open or close it, double-click to rename, right-click for color, ungroup, or close. Drag a folder to move it with all its tabs. Folders are restored on restart.
- **Themes.** Settings → Appearance: System (default), Midnight, Daylight, Forest, Ocean, Sunset, Grape, Mono, Paper, plus your own. "Create theme" lets you pick five colors with a live preview and a contrast check. Gradients can be turned off. Websites follow the theme's dark or light mode.
- **Home page bookmarks.** Your bookmarks show as tiles on the new tab page. Click **+ Add** to add one, hover a tile and click × to remove it, click a folder tile to open it there.
- **Bookmarks bar.** Click the ☆ in the address bar (or `Ctrl+D`) to bookmark a page. Drag a tab onto the bar to bookmark it. Drag bookmarks to reorder, or onto a folder to file them. Right-click for rename, delete, new folder, or to hide the bar (`Ctrl+Shift+B`). Middle-click opens in a new tab.
- **Title bar.** No separate title bar: minimize, maximize and close sit at the right end of the tab strip and follow the theme. Drag empty tab-strip space to move the window.
- **Animations.** Tabs, folders, toasts, loading bar and pages animate. Settings → Appearance → Animations: Follow system, On, or Off.
- **History.** `Ctrl+H` or menu → History. Search, delete single visits or selections, clear the last hour/day/week/month/all. Private tabs are never recorded. Settings → Privacy → History chooses how long it's kept (off, 7/30/90 days, 1 year, forever; default 90 days).
- **Main menu.** The cog opens a menu: new tab, new private tab, history, bookmarks, downloads folder, zoom, full screen, find, print, ad blocker, extensions, clear data, settings, about, exit.
- **Private tabs.** `Ctrl+Shift+N`, right-click the + button, or right-click a link. Private tabs share an in-memory session (cookies, storage, cache) that is wiped when the last private tab closes; they're never saved or restored, and extensions don't run in them. Marked with a mask icon and a "Private" badge.
- **Passkeys and security keys.** Settings → Privacy. "Ask first" (default) stops sites from popping up the Windows security-key dialog by themselves and asks before a site can open it. Also "Allow" and "Block".
- **Fingerprint protection.** Standard: common hardware/GPU values, exact OS/CPU client hints stripped, no Battery API. Strict: plus per-site canvas and audio noise. Settings → Privacy.
- **Private and Tor windows.** Menu → New private window (`Ctrl+Shift+P`) or New Tor window (`Alt+Shift+N`). Each opens as its own window with a throwaway profile: nothing is saved (no history, cookies, tabs or settings), extensions don't run, and a Tor window always goes through Tor (separate from the Tor switch in your normal window). Your bookmarks, settings and ad blocker lists are read from your normal profile.
- **Translation.** When a page is in another language, a translate button appears in the address bar: click to translate, click again for the original, right-click to pick a language or turn on "Always translate". Also on the page's right-click menu. Uses LibreTranslate by default (libretranslate.com needs an API key; or set your own server), or Google Translate (Settings → Languages). Text added later (infinite scroll, menus) is translated too.
- **Website language.** Settings → Languages → Website language sets the language sites are asked for (Accept-Language and `navigator.language`), so sites stay in your language over Tor. First run uses Windows' language.
- **Profiles.** Settings → Profiles: add profiles (e.g. Work, Personal), each with its own cookies, logins, history, bookmarks, extensions and settings, stored only on this PC (`%APPDATA%\Goat\Profiles\<name>`). Each opens in its own window with its own taskbar button (pin it to reopen that profile) and a colored dot on its taskbar icon; a colored initial next to the menu button shows which one you're in (click it to switch). "Shortcut" puts a desktop shortcut that opens that profile.
- **Updates.** Goat checks GitHub Releases once a day (Settings → About; can be turned off). When there's a new version a dot appears on the menu button and the menu shows "Update to Goat x.y". On Windows, "Download and install" fetches the installer and runs it; it updates in place and keeps your profiles. On Linux it opens the download page.
- **Settings profiles.** Settings → Privacy → Profile: **Fastest** (preloading on, sites share connections, Cloudflare DNS), **Balanced** (default) or **Most private** (no preloading, fonts from other sites blocked, strict fingerprint and ad blocking, Mullvad DNS). One click sets those switches; change any of them afterwards and it shows as Custom. Restart Goat after switching for everything to apply.
- **Tor, built in.** Settings → Tor → "Route everything through Tor". No Tor Browser needed: the first time, Goat downloads the official Tor Expert Bundle from torproject.org, checks it against the Tor Browser team's signed checksums (OpenPGP, pinned key), and runs it in the background. Hides your IP from sites. A **Tor** button (hidden by default: Settings → Tor → "Tor button in the toolbar") turns Tor on and off with one click and shows the status; right-click it for New circuit. It always shows while Tor is on; while Tor is connecting or broken, pages don't load (no leaks). "New circuit" gets a new Tor IP. Tor stops when Goat closes.
- **Keep sites separate.** Storage, cache and connections are partitioned per site, and the Storage Access API is off (Settings → Privacy; restart to apply).
- **Trim referrers.** Other sites aren't told which page you came from; embedded content only sees the site name, even if the page asks for more.
- **No background preloading.** Blocks prefetch/prerender requests and Goat's own early connections.
- **Block fonts from other sites** (off by default). Stops font services seeing where you browse; may break icon fonts.
- **Forget when closed.** Per-site list (Settings → Privacy, or right-click a tab): cookies and storage are deleted when the site's last tab closes.
- **Google sign-in.** Google blocks sign-in from browsers built on Electron. On Google's sign-in pages only (`accounts.google.com`, `accounts.youtube.com`), Goat presents itself as current Firefox (user agent, no Chrome client hints, Firefox-like `navigator` values); everywhere else it stays Chrome.
- **Link cleaning.** Tracking tags (utm_*, fbclid, gclid, msclkid, YouTube/Spotify `si`, and more) are removed from addresses you open.
- **Privacy Sandbox off.** Topics, Protected Audience (FLEDGE), Attribution Reporting and Shared Storage are disabled.
- **Speed.** Background new-tab pages give their memory back after 5 seconds (each costs ~100 MB). Memory saver ends the process of idle background tabs, like Chrome's tab discarding, and reloads them when opened (Settings → Privacy → Performance). Restored tabs load only when opened, so startup is fast. When Windows is low on memory, idle tabs sleep after 5 minutes. Settings → Privacy → Performance shows memory per tab, with Sleep and Close buttons. Connections are warmed up when you focus the address bar or hover a bookmark.
- **Encrypted DNS.** Settings → General: Quad9 (default), Mullvad, Mullvad/AdGuard with ad blocking, Cloudflare, Google, NextDNS, a custom DoH address, or off. Shows whether it's working and warns when a proxy or VPN is doing the lookups instead.
- **Privacy.** DNS over HTTPS, HTTPS-only mode, WebRTC IP-leak protection, generic Chrome user agent, `Sec-GPC` and `DNT` headers, no telemetry, sync, or Google services. Camera, mic, location and notifications always ask.
- **Search.** DuckDuckGo by default, switchable to Brave, Startpage, Google, or Bing.

## Shortcuts

`Ctrl+T` new tab · `Ctrl+Shift+N` private tab · `Ctrl+Shift+P` private window · `Alt+Shift+N` Tor window · `Ctrl+H` history · `Ctrl+P` print · `Ctrl+W` close (closing the last tab closes Goat) · `Ctrl+L` address bar · `Ctrl+Tab` / `Ctrl+Shift+Tab` switch · `Ctrl+1…9` jump · `Ctrl+R` / `F5` reload · `Alt+←/→` back/forward · `Ctrl+F` find · `Ctrl+ +/−/0` zoom · `Ctrl+,` settings · `Ctrl+D` bookmark · `Ctrl+Shift+B` bookmarks bar · `Ctrl+Shift+Del` clear data · `F11` fullscreen · `F12` devtools.

## Limits

- Without Tor your ISP still sees which IPs and hostnames (TLS SNI) you connect to. Tor in Goat hides your IP, but Goat is not Tor Browser: its fingerprint differs from other Tor users.
- No saved passwords yet (use an extension).
- Chromium can't log in to SOCKS5 proxies that need a password. Use an HTTP(S) proxy for those.

## Testing speed

Goat uses the same engine as Chrome of the same version, so JavaScript and rendering scores should match Chrome. Compare on [browserbench.org](https://browserbench.org/):

- [Speedometer 3.1](https://browserbench.org/Speedometer3.1/): how fast web apps respond. The most useful single number.
- [JetStream 3.0](https://browserbench.org/JetStream3.0/): JavaScript and WebAssembly speed.
- [MotionMark 1.3.2](https://browserbench.org/MotionMark1.3.2/): graphics and animation.

For a fair test: plug in your laptop, close other apps, use one tab, pause the ad blocker (shield → off) and disable extensions, then run each test 3 times in Goat and in Chrome or Edge and compare the middle score. Goat's real-world advantage is on ad-heavy sites, where blocking makes pages load faster and use less memory; benchmarks don't measure that.
