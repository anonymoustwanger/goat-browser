'use strict';
// Goat's own installer and uninstaller, with Goat's look.
//   GoatBrowser.exe --goat-setup      (started by the small NSIS bootstrapper from a temp folder)
//   GoatBrowser.exe --goat-uninstall  (Windows "Apps > Uninstall", or Settings > About)
//   GoatBrowser.exe --goat-quit       (helper: asks a running Goat to close; exit 0 = none was running)
// Installs per user (no admin) into %LOCALAPPDATA%\Programs\Goat and registers Goat
// as a browser in HKCU. Your profile in %APPDATA%\Goat is never touched unless you
// choose to delete it while uninstalling.

const { app, BrowserWindow, ipcMain, dialog, shell, nativeTheme } = require('electron');
const path = require('path');
const os = require('os');
const ofs = require('original-fs'); // plain fs: sees app.asar as a file, not a folder
const { execFile, spawn } = require('child_process');

const MODE = process.argv.includes('--goat-uninstall') ? 'uninstall' : 'install';
const APP = 'Goat';
const EXE = process.platform === 'win32' ? 'GoatBrowser.exe' : path.basename(process.execPath);
const REGKEY = 'GoatBrowser';
const PROGID = 'GoatHTML';
const UNINST = `HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\${REGKEY}`;
const CLIENTS = `HKEY_CURRENT_USER\\Software\\Clients\\StartMenuInternet\\${REGKEY}`;
const CLASSES = `HKEY_CURRENT_USER\\Software\\Classes\\${PROGID}`;
const REGAPPS = 'HKEY_CURRENT_USER\\Software\\RegisteredApplications';
const MARKER = '.goat-installed';
const VERSION = require(path.join(__dirname, '..', 'package.json')).version;
const PROFILE_DIR = path.join(app.getPath('appData'), APP);
const SOURCE_DIR = path.dirname(process.execPath);
const DEFAULT_DIR = process.env.GOAT_SETUP_DIR || (process.platform === 'win32'
  ? path.join(process.env.LOCALAPPDATA || path.join(os.homedir(), 'AppData', 'Local'), 'Programs', APP)
  : path.join(os.homedir(), '.local', 'share', 'goat-browser'));

// --goat-quit helper: if Goat is running, the single-instance lock fails and the running
// Goat receives our argv (it saves and closes). Exit code tells the caller which happened.
if (process.argv.includes('--goat-quit')) {
  // A profile other than the default has its own lock in its own folder.
  const pa = process.argv.find((x) => x.startsWith('--goat-profile='));
  const pid = pa ? pa.split('=')[1] : '';
  if (/^[a-z0-9-]{1,40}$/.test(pid) && pid !== 'default') app.setPath('userData', path.join(app.getPath('userData'), 'Profiles', pid));
  const got = app.requestSingleInstanceLock();
  app.exit(got ? 0 : 3);
} else {
  // Setup/uninstall run with a throwaway Chromium profile, so the real one can be deleted.
  const tmp = path.join(app.getPath('temp'), `goat-${MODE}-${process.pid}`);
  app.setPath('userData', tmp);
  app.setPath('sessionData', tmp);
  run();
}

// ---------------------------------------------------------------- helpers
const isWin = process.platform === 'win32';
const exec = (file, args) => new Promise((resolve) => {
  execFile(file, args, { windowsHide: true, timeout: 20000 }, (err, stdout) => resolve({ ok: !err, code: err ? err.code : 0, out: String(stdout || '') }));
});

// Ask any running Goat to close; resolves true when none is left.
async function closeRunningGoat(timeoutMs = 10000) {
  const helperArgs = [...(process.defaultApp ? [app.getAppPath()] : []), ...(process.argv.includes('--no-sandbox') ? ['--no-sandbox'] : []), '--goat-quit'];
  const t0 = Date.now();
  for (;;) {
    const r = await exec(process.execPath, helperArgs);
    if (r.ok) return true; // exit 0: nothing running
    if (Date.now() - t0 > timeoutMs) return false;
    await new Promise((res) => setTimeout(res, 700));
  }
}

function walk(dir, base = dir, out = []) {
  for (const e of ofs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, e.name);
    if (e.isDirectory()) walk(full, base, out);
    else if (e.isFile()) out.push({ rel: path.relative(base, full), size: ofs.statSync(full).size });
  }
  return out;
}
function dirSizeKb(dir) {
  try { return Math.round(walk(dir).reduce((a, f) => a + f.size, 0) / 1024); } catch { return 0; }
}

// .reg file: one `reg import` call instead of dozens of `reg add`.
function regFile(entries) {
  const esc = (v) => String(v).replace(/\\/g, '\\\\').replace(/"/g, '\\"');
  let txt = 'Windows Registry Editor Version 5.00\r\n';
  for (const [key, values] of entries) {
    txt += `\r\n[${key}]\r\n`;
    for (const [name, val] of Object.entries(values)) {
      const n = name === '@' ? '@' : `"${esc(name)}"`;
      txt += typeof val === 'number' ? `${n}=dword:${val.toString(16).padStart(8, '0')}\r\n` : `${n}="${esc(val)}"\r\n`;
    }
  }
  return Buffer.concat([Buffer.from([0xff, 0xfe]), Buffer.from(txt, 'utf16le')]);
}

async function register(dir) {
  if (!isWin) return;
  const exe = path.join(dir, EXE);
  const icon = `${exe},0`;
  const entries = [
    [UNINST, {
      DisplayName: APP, DisplayVersion: VERSION, Publisher: APP, DisplayIcon: icon, InstallLocation: dir,
      UninstallString: `"${exe}" --goat-uninstall`, QuietUninstallString: `"${exe}" --goat-uninstall --silent`,
      URLInfoAbout: 'https://github.com/anonymoustwanger/goat-browser', NoModify: 1, NoRepair: 1, EstimatedSize: dirSizeKb(dir),
    }],
    [CLASSES, { '@': 'Goat HTML Document', 'URL Protocol': '' }],
    [`${CLASSES}\\DefaultIcon`, { '@': icon }],
    [`${CLASSES}\\shell\\open\\command`, { '@': `"${exe}" "%1"` }],
    [`${CLASSES}\\Application`, { ApplicationName: APP, ApplicationIcon: icon }],
    [CLIENTS, { '@': APP }],
    [`${CLIENTS}\\DefaultIcon`, { '@': icon }],
    [`${CLIENTS}\\shell\\open\\command`, { '@': `"${exe}"` }],
    [`${CLIENTS}\\Capabilities`, { ApplicationName: APP, ApplicationDescription: 'A private, fast browser built on Chromium.', ApplicationIcon: icon }],
    [`${CLIENTS}\\Capabilities\\StartMenu`, { StartMenuInternet: REGKEY }],
    [`${CLIENTS}\\Capabilities\\URLAssociations`, { http: PROGID, https: PROGID }],
    [`${CLIENTS}\\Capabilities\\FileAssociations`, { '.htm': PROGID, '.html': PROGID, '.pdf': PROGID }],
    [REGAPPS, { [REGKEY]: `Software\\Clients\\StartMenuInternet\\${REGKEY}\\Capabilities` }],
  ];
  const file = path.join(app.getPath('temp'), `goat-reg-${process.pid}.reg`);
  ofs.writeFileSync(file, regFile(entries));
  const r = await exec('reg', ['import', file]);
  try { ofs.rmSync(file, { force: true }); } catch { /* ignore */ }
  if (!r.ok) throw new Error("couldn't register Goat with Windows");
}
async function unregister() {
  if (!isWin) return;
  for (const k of [UNINST, CLIENTS, CLASSES]) await exec('reg', ['delete', k, '/f']);
  await exec('reg', ['delete', REGAPPS, '/v', REGKEY, '/f']);
}

const startMenuLink = () => path.join(app.getPath('appData'), 'Microsoft', 'Windows', 'Start Menu', 'Programs', `${APP}.lnk`);
const desktopLink = () => path.join(app.getPath('desktop'), `${APP}.lnk`);
function makeShortcut(file, dir) {
  if (!isWin) return;
  const exe = path.join(dir, EXE);
  shell.writeShortcutLink(file, 'create', { target: exe, cwd: dir, icon: exe, iconIndex: 0, description: 'Private, fast browser', appUserModelId: 'com.goat.browser' });
}

// Where Goat is installed now (for updates and the uninstaller).
async function installedDir() {
  if (MODE === 'uninstall') return SOURCE_DIR;
  if (isWin) {
    const r = await exec('reg', ['query', UNINST, '/v', 'InstallLocation']);
    const m = /InstallLocation\s+REG_SZ\s+(.+)/.exec(r.out);
    if (m && ofs.existsSync(path.join(m[1].trim(), EXE))) return m[1].trim();
  }
  return ofs.existsSync(path.join(DEFAULT_DIR, MARKER)) ? DEFAULT_DIR : null;
}
function installedVersion(dir) {
  try { return JSON.parse(ofs.readFileSync(path.join(dir, MARKER), 'utf8')).version || ''; } catch { return ''; }
}

// ---------------------------------------------------------------- the window
function run() {
  app.whenReady().then(async () => {
    const existing = await installedDir();
    const win = new BrowserWindow({
      width: 640, height: 460, resizable: false, maximizable: false, fullscreenable: false,
      frame: false, show: false, backgroundColor: nativeTheme.shouldUseDarkColors ? '#101214' : '#eef0f3',
      title: MODE === 'install' ? `${APP} setup` : `Uninstall ${APP}`,
      icon: path.join(__dirname, 'icon.png'),
      webPreferences: { preload: path.join(__dirname, 'setup', 'preload.js'), contextIsolation: true, sandbox: true, nodeIntegration: false },
    });
    win.setMenu(null);
    win.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
    win.webContents.on('will-navigate', (e) => e.preventDefault());
    win.once('ready-to-show', () => win.show());
    win.on('closed', () => app.quit());
    const send = (ch, data) => { if (!win.isDestroyed()) win.webContents.send(ch, data); };
    const own = (fn) => (event, ...a) => (event.sender === win.webContents ? fn(...a) : undefined);

    ipcMain.handle('setup:info', own(() => ({
      mode: MODE, version: VERSION, dir: existing || DEFAULT_DIR, existing: !!existing,
      existingVersion: existing ? installedVersion(existing) : '', platform: process.platform,
      profileDir: PROFILE_DIR, dark: nativeTheme.shouldUseDarkColors,
    })));
    ipcMain.handle('setup:choose-dir', own(async (current) => {
      const r = await dialog.showOpenDialog(win, { title: 'Install Goat to…', defaultPath: current, properties: ['openDirectory', 'createDirectory'] });
      if (r.canceled || !r.filePaths[0]) return null;
      const p = r.filePaths[0];
      return path.basename(p).toLowerCase() === APP.toLowerCase() ? p : path.join(p, APP);
    }));
    ipcMain.on('setup:close', own(() => win.close()));
    ipcMain.on('setup:minimize', own(() => win.minimize()));

    ipcMain.handle('setup:install', own(async (opts) => {
      const dir = String((opts && opts.dir) || DEFAULT_DIR);
      try {
        send('setup:progress', { step: 'Closing Goat…', pct: 0 });
        if (!(await closeRunningGoat())) return { ok: false, error: 'Goat is still open. Close all Goat windows, then click Install again.' };
        const files = walk(SOURCE_DIR);
        const total = files.reduce((a, f) => a + f.size, 0) || 1;
        ofs.mkdirSync(dir, { recursive: true });
        // Old engine files go first so nothing stale is left behind.
        for (const d of ['resources', 'locales']) ofs.rmSync(path.join(dir, d), { recursive: true, force: true });
        ofs.rmSync(path.join(dir, `Uninstall ${APP}.exe`), { force: true }); // left by the 4.6 installer
        let done = 0;
        let last = 0;
        for (const f of files) {
          const to = path.join(dir, f.rel);
          ofs.mkdirSync(path.dirname(to), { recursive: true });
          await ofs.promises.copyFile(path.join(SOURCE_DIR, f.rel), to);
          done += f.size;
          if (Date.now() - last > 60) { last = Date.now(); send('setup:progress', { step: 'Copying files…', pct: Math.round((done / total) * 92) }); }
        }
        ofs.writeFileSync(path.join(dir, MARKER), JSON.stringify({ version: VERSION, installed: new Date().toISOString() }));
        send('setup:progress', { step: 'Adding shortcuts…', pct: 94 });
        makeShortcut(startMenuLink(), dir);
        if (opts && opts.desktop) makeShortcut(desktopLink(), dir);
        else if (isWin) { try { ofs.rmSync(desktopLink(), { force: true }); } catch { /* none */ } }
        send('setup:progress', { step: 'Registering with Windows…', pct: 97 });
        await register(dir);
        send('setup:progress', { step: 'Done', pct: 100 });
        installedTo = dir;
        if (opts && opts.makeDefault && isWin) shell.openExternal(`ms-settings:defaultapps?registeredAppUser=${REGKEY}`).catch(() => {});
        return { ok: true, dir };
      } catch (err) {
        const busy = err && (err.code === 'EBUSY' || err.code === 'EPERM');
        return { ok: false, error: busy ? 'Some Goat files are in use. Close all Goat windows (including private and Tor windows), then click Install again.' : String(err.message || err) };
      }
    }));
    let installedTo = null;
    ipcMain.on('setup:launch', own(() => {
      const exe = path.join(installedTo || DEFAULT_DIR, EXE);
      try { spawn(exe, [], { detached: true, stdio: 'ignore', cwd: path.dirname(exe) }).unref(); } catch { /* ignore */ }
      win.close();
    }));

    ipcMain.handle('setup:uninstall', own(async (opts) => {
      const dir = SOURCE_DIR; // the uninstaller runs from the installed copy
      try {
        send('setup:progress', { step: 'Closing Goat…', pct: 5 });
        if (!(await closeRunningGoat())) return { ok: false, error: 'Goat is still open. Close all Goat windows, then try again.' };
        send('setup:progress', { step: 'Removing shortcuts…', pct: 30 });
        for (const f of [startMenuLink(), desktopLink()]) { try { ofs.rmSync(f, { force: true }); } catch { /* none */ } }
        send('setup:progress', { step: 'Removing from Windows…', pct: 55 });
        await unregister();
        if (opts && opts.removeProfile) {
          send('setup:progress', { step: 'Deleting your profile…', pct: 75 });
          ofs.rmSync(PROFILE_DIR, { recursive: true, force: true });
        }
        send('setup:progress', { step: 'Done', pct: 100 });
        scheduleSelfDelete(dir);
        return { ok: true };
      } catch (err) { return { ok: false, error: String(err.message || err) }; }
    }));

    if (process.argv.includes('--silent') && MODE === 'uninstall') {
      // Quiet uninstall (QuietUninstallString): keep the profile.
      await closeRunningGoat();
      for (const f of [startMenuLink(), desktopLink()]) { try { ofs.rmSync(f, { force: true }); } catch { /* none */ } }
      await unregister();
      scheduleSelfDelete(SOURCE_DIR);
      app.quit();
      return;
    }

    win.loadFile(path.join(__dirname, 'setup', 'index.html'));
  });
  app.on('window-all-closed', () => app.quit());
}

// The install folder can't be deleted while Goat runs from it, so a hidden helper
// deletes it a moment after this process exits.
function scheduleSelfDelete(dir) {
  if (!ofs.existsSync(path.join(dir, MARKER))) return; // only ever delete a folder Goat's installer created
  if (isWin) {
    const cmd = `ping 127.0.0.1 -n 4 >nul & rmdir /s /q "${dir}"`;
    spawn('cmd.exe', ['/d', '/c', cmd], { detached: true, stdio: 'ignore', windowsHide: true, cwd: os.tmpdir() }).unref();
  } else {
    spawn('sh', ['-c', `sleep 3; rm -rf "${dir}"`], { detached: true, stdio: 'ignore', cwd: os.tmpdir() }).unref();
  }
}
