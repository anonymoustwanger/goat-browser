'use strict';
// Installer, uninstaller and the "please close" helper live in setup.js.
if (process.argv.some((a) => a === '--goat-setup' || a === '--goat-uninstall' || a === '--goat-quit')) {
  require('./setup');
  return; // CommonJS module: stop here
}
// Goat — a private, fast browser built on Chromium (via Electron).
// Cookies and site data persist in the profile; trackers, ads and third-party
// cookies are blocked; DNS goes over HTTPS; http:// is upgraded; sites must ask
// for devices. Settings: goat://settings (ad blocker, cookies, proxy, general).

const {
  app, BaseWindow, WebContentsView, session, ipcMain, Menu, dialog,
  clipboard, protocol, net, safeStorage, nativeTheme, webContents: allWebContents, shell,
} = require('electron');
const path = require('path');
const fs = require('fs');
const { pathToFileURL } = require('url');
const { parse: parseDomain } = require('tldts');
const { ElectronBlocker } = require('@ghostery/adblocker-electron');
// Linux edition: no Chrome extensions (Electron's support for them is too limited).
const uninstallExtension = async () => {};
const { Tor } = require('./tor');
const { Translator, LANGUAGES, acceptLanguage, sameLang } = require('./translate');

// ------------------------------------------------------------------- profiles
// Local profiles: each has its own cookies, history, bookmarks, extensions and settings,
// in %APPDATA%\Goat\Profiles\<id>, and runs as its own Goat process
// (--goat-profile=<id>). The default profile is %APPDATA%\Goat itself.
// Data from when the browser was called Goatr moves to the new name.
try {
  const appData = app.getPath('appData');
  const prev = path.join(appData, 'Goatr');
  if (!fs.existsSync(path.join(appData, 'Goat')) && fs.existsSync(prev)) fs.renameSync(prev, path.join(appData, 'Goat'));
} catch { /* keep going with a fresh profile */ }
const ROOT_DIR = app.getPath('userData');
const PROFILE = (() => {
  const a = process.argv.find((x) => x.startsWith('--goat-profile='));
  const id = a ? a.split('=')[1] : '';
  return /^[a-z0-9-]{1,40}$/.test(id) && id !== 'default' ? id : 'default';
})();
const profilesFile = () => path.join(ROOT_DIR, 'profiles.json');
const profileDir = (id) => (id === 'default' ? ROOT_DIR : path.join(ROOT_DIR, 'Profiles', id));
if (PROFILE !== 'default') {
  fs.mkdirSync(profileDir(PROFILE), { recursive: true });
  app.setPath('userData', profileDir(PROFILE));
}
// Each profile gets its own taskbar button on Windows (different AppUserModelID).
const APP_ID = PROFILE === 'default' ? 'com.goat.browser' : `com.goat.browser.profile.${PROFILE}`;
if (process.platform === 'win32') app.setAppUserModelId(APP_ID);
const PROFILE_COLORS = ['#5fd3b3', '#6ea8ff', '#ff9e6b', '#c39bff', '#ff7aa8', '#e8c15a', '#8fa394'];
let profilesCache = null; let profilesCacheAt = 0;
function loadProfiles() {
  if (profilesCache && Date.now() - profilesCacheAt < 3000) return profilesCache.map((p) => ({ ...p }));
  let list = [];
  try { list = JSON.parse(fs.readFileSync(profilesFile(), 'utf8')).profiles || []; } catch { /* none yet */ }
  list = list.filter((p) => p && /^[a-z0-9-]{1,40}$/.test(p.id) && p.id !== 'default')
    .map((p) => ({ id: p.id, name: String(p.name || 'Profile').slice(0, 40), color: PROFILE_COLORS.includes(p.color) ? p.color : PROFILE_COLORS[1] }));
  let def = { id: 'default', name: 'Personal', color: PROFILE_COLORS[0] };
  try { const d = JSON.parse(fs.readFileSync(profilesFile(), 'utf8')).default; if (d) def = { ...def, name: String(d.name || def.name).slice(0, 40), color: PROFILE_COLORS.includes(d.color) ? d.color : def.color }; } catch { /* default */ }
  profilesCache = [def, ...list]; profilesCacheAt = Date.now();
  return profilesCache.map((p) => ({ ...p }));
}
function saveProfiles(list) {
  const def = list.find((p) => p.id === 'default');
  fs.mkdirSync(ROOT_DIR, { recursive: true });
  fs.writeFileSync(profilesFile(), JSON.stringify({ default: { name: def.name, color: def.color }, profiles: list.filter((p) => p.id !== 'default') }, null, 2));
  profilesCache = null;
}
const currentProfile = () => loadProfiles().find((p) => p.id === PROFILE) || { id: PROFILE, name: PROFILE, color: PROFILE_COLORS[1] };
const profileArgs = (id = PROFILE) => (id === 'default' ? [] : [`--goat-profile=${id}`]);
const devArgs = () => [...(process.defaultApp ? [app.getAppPath()] : []), ...(process.argv.includes('--no-sandbox') ? ['--no-sandbox'] : [])];
function openProfile(id) {
  if (id === PROFILE) { if (win) win.focus(); return; }
  try {
    const child = require('child_process').spawn(process.execPath, [...devArgs(), ...profileArgs(id)], { detached: true, stdio: 'ignore' });
    windowProcs.add(child);
    child.on('exit', () => windowProcs.delete(child));
    child.unref();
  } catch (err) { toast(`Couldn't open profile: ${err.message}`); }
}

// ---------------------------------------------------------------- window mode
// Private and Tor windows run as their own Goat process with a throwaway Chromium
// profile in %TEMP%. They read your settings, bookmarks and blocklists from the
// main profile but never write history, tabs, cookies or settings back.
const MODE = (() => {
  const a = process.argv.find((x) => x.startsWith('--goat-window='));
  const m = a ? a.split('=')[1] : 'normal';
  return ['private', 'tor'].includes(m) ? m : 'normal';
})();
const EPHEMERAL = MODE !== 'normal';
const MAIN_DIR = app.getPath('userData');
const TEMP_PREFIX = 'goat-window-';
if (EPHEMERAL) {
  const tmp = path.join(app.getPath('temp'), `${TEMP_PREFIX}${MODE}-${process.pid}`);
  fs.mkdirSync(tmp, { recursive: true });
  app.setPath('userData', tmp);
  app.setPath('sessionData', tmp);
}
// Remove temp profiles left by private/Tor windows that are no longer running.
function cleanStaleWindowProfiles() {
  let names = [];
  try { names = fs.readdirSync(app.getPath('temp')); } catch { return; }
  for (const n of names) {
    const m = new RegExp(`^${TEMP_PREFIX}(private|tor)-(\\d+)$`).exec(n);
    if (!m || Number(m[2]) === process.pid) continue;
    let alive = false;
    try { process.kill(Number(m[2]), 0); alive = true; } catch { /* not running */ }
    if (!alive) fs.rm(path.join(app.getPath('temp'), n), { recursive: true, force: true }, () => {});
  }
}
var windowProcs = new Set(); // private/Tor/profile windows we started (closed with us for updates)
function openWindow(mode) {
  const args = [...devArgs(), ...profileArgs(), `--goat-window=${mode}`];
  try {
    const child = require('child_process').spawn(process.execPath, args, { detached: true, stdio: 'ignore' });
    windowProcs.add(child);
    child.on('exit', () => windowProcs.delete(child));
    child.unref();
  } catch (err) { toast(`Couldn't open window: ${err.message}`); }
}

// ------------------------------------------------------------------ constants
const SCHEME = 'goat';
const NEWTAB_URL = `${SCHEME}://newtab/`;
const SETTINGS_URL = `${SCHEME}://settings/`;
const HISTORY_URL = `${SCHEME}://history/`;
const PAGES_DIR = path.join(__dirname, 'pages');
const TOOLBAR_HEIGHT = 82;
const BOOKMARKS_BAR_HEIGHT = 32;
const uiHeight = () => TOOLBAR_HEIGHT + (settings && settings.showBookmarksBar ? BOOKMARKS_BAR_HEIGHT : 0);
const BLOCKLIST_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;
// Encrypted DNS providers (DNS over HTTPS). One provider at a time, so the
// provider's own "am I using you?" check page gives a clear answer.
const DNS_PROVIDERS = {
  quad9: { name: 'Quad9', url: 'https://dns.quad9.net/dns-query', check: 'https://on.quad9.net/' },
  mullvad: { name: 'Mullvad', url: 'https://dns.mullvad.net/dns-query', check: 'https://mullvad.net/en/check' },
  mullvadAdblock: { name: 'Mullvad (blocks ads)', url: 'https://adblock.dns.mullvad.net/dns-query', check: 'https://mullvad.net/en/check' },
  cloudflare: { name: 'Cloudflare', url: 'https://cloudflare-dns.com/dns-query', check: 'https://one.one.one.one/help/' },
  google: { name: 'Google', url: 'https://dns.google/dns-query', check: null },
  adguard: { name: 'AdGuard (blocks ads)', url: 'https://dns.adguard-dns.com/dns-query', check: 'https://adguard.com/en/test.html' },
  nextdns: { name: 'NextDNS', url: null, check: 'https://test.nextdns.io/' },
  custom: { name: 'Custom', url: null, check: null },
};
const SEARCH_ENGINES = {
  duckduckgo: 'https://duckduckgo.com/?q=',
  brave: 'https://search.brave.com/search?q=',
  startpage: 'https://www.startpage.com/do/search?q=',
  google: 'https://www.google.com/search?q=',
  bing: 'https://www.bing.com/search?q=',
};
const BLOCK_LEVELS = {
  ads: 'fromPrebuiltAdsOnly',
  standard: 'fromPrebuiltAdsAndTracking',
  strict: 'fromPrebuiltFull',
};
const THEMES = {
  midnight: { scheme: 'dark', bg: '#101214', text: '#e7e9ec', name: 'Midnight' },
  daylight: { scheme: 'light', bg: '#eef0f3', text: '#16191d', name: 'Daylight' },
  forest: { scheme: 'dark', bg: '#0f1511', text: '#e4ece5', name: 'Forest' },
  ocean: { scheme: 'dark', bg: '#0b1220', text: '#e3eaf5', name: 'Ocean' },
  sunset: { scheme: 'dark', bg: '#1a1012', text: '#f3e6e3', name: 'Sunset' },
  grape: { scheme: 'dark', bg: '#140f1c', text: '#ece6f5', name: 'Grape' },
  mono: { scheme: 'dark', bg: '#0c0c0c', text: '#f2f2f2', name: 'Mono' },
  paper: { scheme: 'light', bg: '#f4efe6', text: '#2a241c', name: 'Paper' },
};
// Settings profiles (Settings → Privacy). Each sets only the speed-vs-privacy switches below.
const PROFILES = {
  fastest: {
    name: 'Fastest', desc: 'Like Chrome with an ad blocker. Pages preload and connections are reused across sites. Cloudflare DNS.',
    patch: { privacy: { blockPrefetch: false, isolateSites: false, blockThirdPartyFonts: false, fingerprint: 'standard', trimReferrers: true }, adblock: { level: 'standard' }, dns: { provider: 'cloudflare' } },
  },
  balanced: {
    name: 'Balanced', desc: 'Sites kept separate, no background preloading, Quad9 DNS. The default.',
    patch: { privacy: { blockPrefetch: true, isolateSites: true, blockThirdPartyFonts: false, fingerprint: 'standard', trimReferrers: true }, adblock: { level: 'standard' }, dns: { provider: 'quad9' } },
  },
  private: {
    name: 'Most private', desc: 'No preloading, sites kept separate, fonts from other sites blocked, strict fingerprint and ad blocking, Mullvad DNS. Slower, and a few sites may break.',
    patch: { privacy: { blockPrefetch: true, isolateSites: true, blockThirdPartyFonts: true, fingerprint: 'strict', trimReferrers: true }, adblock: { level: 'strict' }, dns: { provider: 'mullvad' } },
  },
};
const CONTROLS_HEIGHT = 38; // native min/max/close sit in the tab strip
const GROUP_COLORS = ['blue', 'red', 'yellow', 'green', 'pink', 'purple', 'cyan', 'orange', 'grey'];

const DEFAULT_SETTINGS = {
  searchEngine: 'duckduckgo',
  restoreTabs: true,
  history: { keepDays: 90 }, // 0 = don't save, -1 = forever
  privacy: {
    fingerprint: 'standard', stripTracking: true, webauthn: 'ask',
    isolateSites: true, // partition storage, cache and connections per site (restart)
    trimReferrers: true, // other sites only see the site you came from, or nothing
    blockPrefetch: true, // no background loading of pages you haven't opened
    blockThirdPartyFonts: false, // fonts from other servers (can break icons)
    forgetSites: [], // cookies + storage deleted when the site's last tab closes
  },
  tor: { enabled: false, showButton: false },
  updates: { auto: true, lastCheck: 0, skip: '' }, // auto: check GitHub for a new Goat once a day // showButton: Tor on/off button in the toolbar
  // sites: language websites are asked for (Accept-Language, navigator.language).
  // translateTo: language pages are translated into. engine: google | libre.
  languages: { sites: 'en', translateTo: 'en', offer: true, auto: false, engine: 'libre', libreUrl: 'https://libretranslate.com', libreKey: '' },
  performance: { sleepAfterMin: 30 },
  showBookmarksBar: true,
  appearance: { theme: 'system', motion: 'system', gradients: true, customThemes: [], siteIcons: true }, // siteIcons: off shows first letters
  stats: { blocked: 0 },
  httpsOnly: true,
  secureDns: true,
  dns: { provider: 'quad9', nextdnsId: '', custom: '' },
  adblock: { enabled: true, level: 'standard', allowlist: [], customRules: '', skipExtensions: true }, // skipExtensions: never filter extensions' own requests
  cookies: { blockThirdParty: true, clearOnExit: false },
  uploads: { paste: true }, // paste: offer the clipboard's image when a site asks for an image file
  extensions: { unpacked: [], hidden: [], pinned: null }, // pinned: ids shown in the toolbar
  proxy: { mode: 'system', type: 'socks5', host: '', port: '', bypass: '<local>', pacUrl: '', username: '', password: '' },
};

// ------------------------------------------------------------ engine switches
app.commandLine.appendSwitch('force-webrtc-ip-handling-policy', 'disable_non_proxied_udp');
app.commandLine.appendSwitch('disable-breakpad');
app.commandLine.appendSwitch('disable-domain-reliability');
app.commandLine.appendSwitch('no-pings');
app.commandLine.appendSwitch('enable-parallel-downloading');
app.commandLine.appendSwitch('enable-gpu-rasterization');
app.commandLine.appendSwitch('enable-zero-copy');
// Settings that need to be known before Chromium starts (restart to change).
const EARLY = (() => {
  try { return JSON.parse(fs.readFileSync(path.join(MAIN_DIR, 'settings.json'), 'utf8')); } catch { return {}; }
})();
{ const l = EARLY.languages && EARLY.languages.sites; if (l && LANGUAGES[l]) app.commandLine.appendSwitch('lang', l); }
const EARLY_ISOLATE = !(EARLY.privacy && EARLY.privacy.isolateSites === false);
const EARLY_PREFETCH_OFF = !(EARLY.privacy && EARLY.privacy.blockPrefetch === false);
if (EARLY_ISOLATE) {
  app.commandLine.appendSwitch('enable-features', [
    'ThirdPartyStoragePartitioning', 'SplitCacheByNetworkIsolationKey', 'SplitCodeCacheByNetworkIsolationKey',
    'PartitionConnectionsByNetworkIsolationKey', 'PartitionedCookies', 'PartitionNelAndReportingByNetworkIsolationKey',
    'PartitionDomainReliabilityByNetworkIsolationKey', 'StrictOriginIsolation',
  ].join(','));
}
app.commandLine.appendSwitch('disable-features', [
  ...(EARLY_ISOLATE ? ['StorageAccessAPI', 'StorageAccessHeaders'] : []),
  ...(EARLY_PREFETCH_OFF ? ['Prerender2', 'SpeculationRulesPrefetchFuture', 'NavigationPredictor', 'PrefetchPrivacyChanges'] : []),
  'AutofillServerCommunication', 'MediaRouter', 'OptimizationHints', 'Translate',
  // Privacy Sandbox ad APIs: no interest groups, topics or attribution reports.
  'BrowsingTopics', 'InterestGroupStorage', 'Fledge', 'AdInterestGroupAPI', 'PrivacySandboxAdsAPIs',
  'AttributionReportingCrossAppWeb', 'ConversionMeasurement', 'SharedStorageAPI', 'FencedFrames',
].join(','));

const CHROME_MAJOR = process.versions.chrome.split('.')[0];
const UA_PLATFORM = {
  win32: 'Windows NT 10.0; Win64; x64',
  darwin: 'Macintosh; Intel Mac OS X 10_15_7',
}[process.platform] || 'X11; Linux x86_64';
const USER_AGENT = `Mozilla/5.0 (${UA_PLATFORM}) AppleWebKit/537.36 ` +
  `(KHTML, like Gecko) Chrome/${CHROME_MAJOR}.0.0.0 Safari/537.36`;
app.userAgentFallback = USER_AGENT;

protocol.registerSchemesAsPrivileged([
  { scheme: SCHEME, privileges: { standard: true, secure: true } },
]);

// ---------------------------------------------------------------------- state
let tor = null; // created once userData is known (see lifecycle)
let win = null;
let ui = null;
let ses = null;
let privSes = null; // private tabs: in-memory only, wiped when the last one closes
let blocker = null;
let blockerPromise = Promise.resolve();
let extensions = null;
let statsDirty = false;
let panel = null;
let panelOpen = false;
let panelHiddenAt = 0;
setInterval(() => { if (statsDirty) { statsDirty = false; saveSettings(); } }, 15000).unref();
let blockerGen = 0;
let shieldState = 'loading'; // loading | on | error | disabled
let settings = null;
const tabs = new Map(); // id -> { id, view, title, blocked, groupId }
const groups = new Map(); // id -> { id, name, color, collapsed }
let order = [];
let activeId = null;
let nextId = 1;
let nextGroupId = 1;
let fullscreen = false;
let quitting = false;
const allowHttp = new Set();
const upgradedHosts = new Set();
const permDecisions = new Map();

const userDir = () => MAIN_DIR; // your profile files (read-only in private/Tor windows)
const settingsFile = () => path.join(userDir(), 'settings.json');
const sessionFile = () => path.join(userDir(), 'tabs.json');

// ------------------------------------------------------------------- settings
function merge(base, patch) {
  const out = Array.isArray(base) ? [...base] : { ...base };
  for (const [k, v] of Object.entries(patch || {})) {
    out[k] = v && typeof v === 'object' && !Array.isArray(v) && base && typeof base[k] === 'object' && !Array.isArray(base[k])
      ? merge(base[k], v) : v;
  }
  return out;
}

function loadSettings() {
  let raw = {};
  try { raw = JSON.parse(fs.readFileSync(settingsFile(), 'utf8')); } catch { /* first run */ }
  settings = merge(DEFAULT_SETTINGS, raw);
  // 4.4: LibreTranslate became the default translation service.
  if (!raw.v44) {
    settings.v44 = true;
    settings.languages.engine = 'libre';
    if (!settings.languages.libreUrl) settings.languages.libreUrl = DEFAULT_SETTINGS.languages.libreUrl;
  }
  if (!raw.languages) {
    // First run: use the system language, so sites (and translation) match you.
    const loc = app.getLocale();
    const code = LANGUAGES[loc] ? loc : LANGUAGES[loc.split('-')[0]] ? loc.split('-')[0] : 'en';
    settings.languages.sites = code;
    settings.languages.translateTo = code === 'en-GB' ? 'en' : code;
  }
  // Older versions had a single on/off switch.
  if (!raw.dns && raw.secureDns === false) settings.dns.provider = 'off';
  // Themes that no longer exist (e.g. the removed "Goat" theme) fall back to System.
  const th = settings.appearance.theme;
  if (th !== 'system' && !THEMES[th] && !(settings.appearance.customThemes || []).some((c) => c && c.id === th)) settings.appearance.theme = 'system';
}

let saveTimer = null;
function saveSettings() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    if (EPHEMERAL) return;
    fs.mkdirSync(userDir(), { recursive: true });
    fs.writeFileSync(settingsFile(), JSON.stringify(settings, null, 2));
  }, 200);
}

function encryptSecret(s) {
  if (!s) return '';
  try { return safeStorage.isEncryptionAvailable() ? 'enc:' + safeStorage.encryptString(s).toString('base64') : s; }
  catch { return s; }
}
function decryptSecret(s) {
  if (!s || !s.startsWith('enc:')) return s || '';
  try { return safeStorage.decryptString(Buffer.from(s.slice(4), 'base64')); } catch { return ''; }
}

function publicSettings() {
  const s = JSON.parse(JSON.stringify(settings));
  s.proxy.password = s.proxy.password ? '••••••••' : '';
  return s;
}

async function applySettings(patch) {
  const before = JSON.stringify(settings);
  const p = JSON.parse(JSON.stringify(patch || {}));
  if (p.proxy && 'password' in p.proxy) {
    if (p.proxy.password === '••••••••') delete p.proxy.password;
    else p.proxy.password = encryptSecret(p.proxy.password);
  }
  if (p.adblock && Array.isArray(p.adblock.allowlist)) {
    p.adblock.allowlist = [...new Set(p.adblock.allowlist.map(normalizeSite).filter(Boolean))];
  }
  const old = settings;
  settings = merge(settings, p);
  if (!SEARCH_ENGINES[settings.searchEngine]) settings.searchEngine = 'duckduckgo';
  if (!BLOCK_LEVELS[settings.adblock.level]) settings.adblock.level = 'standard';
  settings.appearance.customThemes = sanitizeCustomThemes(settings.appearance.customThemes);
  if (settings.appearance.theme !== 'system' && !themeInfo(settings.appearance.theme)) settings.appearance.theme = 'system';
  settings.appearance.gradients = settings.appearance.gradients !== false;
  if (!['off', 'standard', 'strict'].includes(settings.privacy.fingerprint)) settings.privacy.fingerprint = 'standard';
  settings.privacy.stripTracking = settings.privacy.stripTracking !== false;
  settings.adblock.skipExtensions = settings.adblock.skipExtensions !== false;
  if (![0, 7, 30, 90, 365, -1].includes(Number(settings.history.keepDays))) settings.history.keepDays = 90;
  settings.history.keepDays = Number(settings.history.keepDays);
  if (!['ask', 'allow', 'block'].includes(settings.privacy.webauthn)) settings.privacy.webauthn = 'ask';
  for (const k of ['isolateSites', 'trimReferrers', 'blockPrefetch', 'blockThirdPartyFonts']) settings.privacy[k] = !!settings.privacy[k];
  settings.privacy.forgetSites = [...new Set((Array.isArray(settings.privacy.forgetSites) ? settings.privacy.forgetSites : []).map(normalizeSite).filter(Boolean))];
  settings.tor.enabled = MODE === 'tor' || !!settings.tor.enabled;
  settings.tor.showButton = !!settings.tor.showButton;
  settings.updates.auto = settings.updates.auto !== false;
  settings.updates.skip = String(settings.updates.skip || '').slice(0, 30);
  { const L = settings.languages;
    if (!LANGUAGES[L.sites]) L.sites = 'en';
    if (!LANGUAGES[L.translateTo]) L.translateTo = 'en';
    L.offer = L.offer !== false; L.auto = !!L.auto;
    if (!['google', 'libre'].includes(L.engine)) L.engine = 'google';
    L.libreUrl = String(L.libreUrl || '').trim().slice(0, 300);
    if (L.libreUrl && !/^https?:\/\//.test(L.libreUrl)) L.libreUrl = 'https://' + L.libreUrl;
    L.libreKey = String(L.libreKey || '').trim().slice(0, 200); }
  if (settings.dns.provider !== 'off' && !DNS_PROVIDERS[settings.dns.provider]) settings.dns.provider = 'quad9';
  settings.dns.nextdnsId = String(settings.dns.nextdnsId || '').trim().replace(/[^a-z0-9]/gi, '').slice(0, 20);
  settings.dns.custom = String(settings.dns.custom || '').trim().slice(0, 300);
  settings.performance.sleepAfterMin = [0, 5, 15, 30, 60, 120].includes(Number(settings.performance.sleepAfterMin)) ? Number(settings.performance.sleepAfterMin) : 30;
  if (!['system', 'on', 'off'].includes(settings.appearance.motion)) settings.appearance.motion = 'system';
  if (JSON.stringify(settings) === before) return publicSettings();
  saveSettings();

  if (old.tor.enabled !== settings.tor.enabled) {
    if (settings.tor.enabled) tor.start(); else tor.stop();
    await applyProxy();
  } else if (JSON.stringify(old.proxy) !== JSON.stringify(settings.proxy)) await applyProxy();
  if (JSON.stringify(old.dns) !== JSON.stringify(settings.dns)) applyDns();
  if (old.languages.sites !== settings.languages.sites) for (const x of allSessions()) x.setUserAgent(USER_AGENT, acceptLanguage(settings.languages.sites));
  if (old.languages.translateTo !== settings.languages.translateTo || old.languages.offer !== settings.languages.offer) {
    for (const t of tabs.values()) if (!t.tr || t.tr.state === 'offer' || t.tr.state === 'none') checkTranslate(t);
  }
  if (JSON.stringify(old.appearance) !== JSON.stringify(settings.appearance)) applyAppearance();
  if (old.showBookmarksBar !== settings.showBookmarksBar) { layout(); scheduleState(); }
  if (old.history.keepDays !== settings.history.keepDays) pruneHistory(true);
  const ab = (o) => JSON.stringify({ e: o.adblock.enabled, l: o.adblock.level, r: o.adblock.customRules });
  if (ab(old) !== ab(settings)) blockerPromise = reloadBlocker();
  if (JSON.stringify(old.adblock.allowlist) !== JSON.stringify(settings.adblock.allowlist)) scheduleState();
  if (old.tor.showButton !== settings.tor.showButton) scheduleState();
  return publicSettings();
}

function searchUrl(q) { return SEARCH_ENGINES[settings.searchEngine] + encodeURIComponent(q); }

// -------------------------------------------------------------------- helpers
// Site (eTLD+1) of a URL, cached by host: called several times per network request.
const siteCache = new Map();
function siteOf(url) {
  let host;
  try { host = new URL(url).host; } catch { host = null; }
  if (host && siteCache.has(host)) return siteCache.get(host);
  let site = null;
  try { const p = parseDomain(url); site = p.domain || p.hostname || null; } catch { site = null; }
  if (host) {
    if (siteCache.size > 4000) siteCache.clear();
    siteCache.set(host, site);
  }
  return site;
}
function normalizeSite(s) {
  const t = String(s || '').trim().toLowerCase();
  if (!t) return null;
  return siteOf(/^[a-z]+:\/\//.test(t) ? t : 'https://' + t);
}
function isLocalHost(host) {
  return /^(localhost|127\.\d+\.\d+\.\d+|\[::1\]|::1|10\.\d+\.\d+\.\d+|192\.168\.\d+\.\d+|172\.(1[6-9]|2\d|3[01])\.\d+\.\d+)$/i.test(host) ||
    /\.(localhost|local|lan|home\.arpa|internal)$/i.test(host);
}
function deleteHeader(headers, name) {
  for (const k of Object.keys(headers)) if (k.toLowerCase() === name) delete headers[k];
}
function topUrlFor(details) {
  const wc = details.webContents;
  try { return wc && !wc.isDestroyed() ? wc.getURL() : null; } catch { return null; }
}
// Sites run by the same company that sign you in across each other (iCloud Mail signs in
// through apple.com, YouTube through google.com, Outlook through live.com…): their cookies
// aren't treated as third-party between them, or you keep getting signed out.
const RELATED_SITES = [
  ["apple.com", "icloud.com", "icloud-content.com", "cdn-apple.com", "apple-cloudkit.com", "mzstatic.com", "me.com", "icloud.com.cn"],
  ["google.com", "youtube.com", "googleapis.com", "gstatic.com", "googleusercontent.com", "ggpht.com", "ytimg.com", "googlevideo.com", "gmail.com", "google.co.uk", "youtube-nocookie.com", "withgoogle.com"],
  ["microsoft.com", "live.com", "microsoftonline.com", "office.com", "office365.com", "outlook.com", "sharepoint.com", "onedrive.com", "msn.com", "bing.com", "xbox.com", "skype.com", "msftauth.net", "msauth.net", "azure.com", "microsoft365.com", "live.net"],
  ["roblox.com", "rbxcdn.com", "rbx.com", "robloxlabs.com"],
  ["discord.com", "discordapp.com", "discord.gg", "discordapp.net", "discord.media"],
  ["github.com", "githubusercontent.com", "githubassets.com"],
  ["reddit.com", "redditmedia.com", "redditstatic.com", "redd.it"],
  ["x.com", "twitter.com", "twimg.com"],
  ["facebook.com", "fbcdn.net", "instagram.com", "cdninstagram.com", "messenger.com", "whatsapp.com", "threads.net"],
  ["twitch.tv", "ttvnw.net", "jtvnw.net"],
  ["spotify.com", "scdn.co", "spotifycdn.com"],
  ["paypal.com", "paypalobjects.com"],
  ["steampowered.com", "steamcommunity.com", "steamstatic.com"],
  ["proton.me", "protonmail.com"],
  ["amazon.com", "amazon.co.uk", "media-amazon.com", "ssl-images-amazon.com"],
];
const relatedSites = (a, b) => a === b || RELATED_SITES.some((g) => g.includes(a) && g.includes(b));
function isThirdParty(details) {
  if (details.resourceType === 'mainFrame') return false;
  const top = topUrlFor(details);
  if (!top || !/^https?:/.test(top)) return false;
  const a = siteOf(details.url);
  const b = siteOf(top);
  return !!(a && b && !relatedSites(a, b));
}
// Requests made by an extension itself (its background worker, popup or options page).
function fromExtension(details) {
  if (/^chrome-extension:/.test(details.initiatorOrigin || '')) return true;
  if (/^chrome-extension:/.test(details.referrer || '')) return true;
  try { if (details.frame && /^chrome-extension:/.test(details.frame.url)) return true; } catch { /* frame gone */ }
  const top = topUrlFor(details);
  return !!(top && top.startsWith('chrome-extension:'));
}
const skipForExtension = (details) => settings.adblock.skipExtensions && fromExtension(details);
function isAllowlisted(url) {
  const s = siteOf(url);
  return !!s && settings.adblock.allowlist.includes(s);
}
function originOf(url) { try { return new URL(url).origin; } catch { return String(url); } }
function tabByWebContentsId(wcId) {
  for (const t of tabs.values()) {
    if (!t.view.webContents.isDestroyed() && t.view.webContents.id === wcId) return t;
  }
  return null;
}
function toUrl(text) {
  const s = String(text || '').trim();
  if (!s) return null;
  if (/^(https?|goat|file|view-source|chrome-extension):/i.test(s) || /^about:blank$/i.test(s)) return s;
  if (/\s/.test(s)) return searchUrl(s);
  const host = s.split(/[/?#]/)[0].replace(/:\d+$/, '');
  if (isLocalHost(host) || /^\d{1,3}(\.\d{1,3}){3}$/.test(host)) return 'http://' + s;
  if (/^[^.]+(\.[^.]+)*\.[a-z][a-z0-9-]{1,62}$/i.test(host)) return 'https://' + s;
  return searchUrl(s);
}
const pendingUrls = new WeakMap(); // webContents -> URL not loaded yet (lazy restore)
function displayUrl(wc) {
  if (pendingUrls.has(wc)) return pendingUrls.get(wc);
  const u = wc.getURL();
  if (!u || u.startsWith(NEWTAB_URL)) return '';
  if (u.startsWith(`${SCHEME}://error/`) || u.startsWith(`${SCHEME}://sleep/`)) {
    try { return new URL(u).searchParams.get('u') || u; } catch { return u; }
  }
  return u;
}
function securityOf(url) {
  if (!url) return 'internal';
  if (url.startsWith('https:')) return 'secure';
  if (url.startsWith('http:')) return 'insecure';
  return 'internal';
}

// --------------------------------------------------------------- web requests
// Tracking parameters removed from links you open (when enabled).
const TRACKING_PARAMS = new Set([
  'utm_source', 'utm_medium', 'utm_campaign', 'utm_term', 'utm_content', 'utm_id', 'utm_name', 'utm_reader',
  'utm_brand', 'utm_social', 'utm_social-type', 'fbclid', 'gclid', 'gclsrc', 'dclid', 'gbraid', 'wbraid',
  'msclkid', 'mc_cid', 'mc_eid', 'igshid', 'igsh', 'yclid', '_hsenc', '_hsmi', 'mkt_tok', 'oly_anon_id',
  'oly_enc_id', 'vero_id', 'vero_conv', 'rb_clickid', 's_cid', 'ttclid', 'twclid', 'wickedid', 'srsltid',
  'ref_src', 'ref_url', '_openstat', 'trk', 'trkCampaign', 'sc_cid', 'epik', 'li_fat_id', 'cmpid',
]);
const SITE_TRACKING_PARAMS = { 'youtube.com': ['si', 'pp'], 'youtu.be': ['si'], 'spotify.com': ['si'], 'twitter.com': ['s', 't'], 'x.com': ['s', 't'], 'amazon.com': ['pd_rd_r', 'pd_rd_w', 'pd_rd_wg', 'pf_rd_p', 'pf_rd_r', 'pd_rd_i', 'ref_', 'content-id'] };
function cleanTrackingUrl(raw) {
  let u;
  try { u = new URL(raw); } catch { return null; }
  if (!u.search) return null;
  const extra = SITE_TRACKING_PARAMS[siteOf(raw)] || [];
  let changed = false;
  for (const key of [...u.searchParams.keys()]) {
    if (TRACKING_PARAMS.has(key) || TRACKING_PARAMS.has(key.toLowerCase()) || extra.includes(key)) { u.searchParams.delete(key); changed = true; }
  }
  return changed ? u.toString() : null;
}

function onBeforeRequest(details, callback) {
  const { url } = details;
  if (settings.privacy.stripTracking && details.resourceType === 'mainFrame' && /^https?:/.test(url)) {
    const clean = cleanTrackingUrl(url);
    if (clean) { callback({ redirectURL: clean }); return; }
  }
  if (settings.httpsOnly && details.resourceType === 'mainFrame' && url.startsWith('http://')) {
    try {
      const u = new URL(url);
      if (!isLocalHost(u.hostname) && !allowHttp.has(u.hostname)) {
        u.protocol = 'https:';
        if (u.port === '80') u.port = '';
        upgradedHosts.add(u.hostname);
        callback({ redirectURL: u.toString() });
        return;
      }
    } catch { /* fall through */ }
  }
  if (settings.privacy.blockThirdPartyFonts && details.resourceType === 'font' && /^https?:/.test(url) && isThirdParty(details) && !skipForExtension(details)) {
    const t = topUrlFor(details);
    if (!(t && isAllowlisted(t))) { callback({ cancel: true }); return; }
  }
  if (!blocker || !/^(https?|wss?):/.test(url) || skipForExtension(details)) { callback({}); return; }
  const top = topUrlFor(details);
  if (top && isAllowlisted(top)) { callback({}); return; }
  blocker.onBeforeRequest(details, (res) => {
    if (res && (res.cancel || res.redirectURL)) {
      const t = tabByWebContentsId(details.webContentsId);
      if (t) { t.blocked += 1; scheduleState(); }
      settings.stats.blocked = (settings.stats.blocked || 0) + 1;
      statsDirty = true;
    }
    callback(res || {});
  });
}

// Google refuses sign-in from browsers built on Electron ("This browser or app may not be
// secure"). On its sign-in pages only, Goat looks like current Firefox: Firefox's user agent,
// no Chrome client hints, and Firefox-like navigator values (set in fp-preload).
const GOOGLE_SIGNIN_HOSTS = new Set(['accounts.google.com', 'accounts.youtube.com']);
function firefoxUA() {
  // Firefox 160 shipped 2026-11-10; a new version every 4 weeks keeps this current.
  const v = 160 + Math.max(0, Math.floor((Date.now() - Date.UTC(2026, 10, 10)) / (28 * 864e5)));
  const plat = process.platform === 'win32' ? 'Windows NT 10.0; Win64; x64' : process.platform === 'darwin' ? 'Macintosh; Intel Mac OS X 10.15' : 'X11; Linux x86_64';
  return `Mozilla/5.0 (${plat}; rv:${v}.0) Gecko/20100101 Firefox/${v}.0`;
}
const isGoogleSignIn = (url) => { try { return GOOGLE_SIGNIN_HOSTS.has(new URL(url).hostname); } catch { return false; } };

function onBeforeSendHeaders(details, callback) {
  const h = details.requestHeaders;
  if (isGoogleSignIn(details.url)) {
    for (const k of Object.keys(h)) {
      const lk = k.toLowerCase();
      if (lk === 'user-agent') delete h[k];
      if (lk.startsWith('sec-ch-')) delete h[k];
    }
    h['User-Agent'] = firefoxUA();
  }
  h['Sec-GPC'] = '1';
  h['DNT'] = '1';
  if (settings.privacy.fingerprint !== 'off') {
    // Only low-entropy client hints leave the browser.
    for (const k of Object.keys(h)) {
      const lk = k.toLowerCase();
      if (lk.startsWith('sec-ch-ua-') && !['sec-ch-ua-mobile', 'sec-ch-ua-platform'].includes(lk)) delete h[k];
      if (lk === 'sec-ch-device-memory' || lk === 'device-memory' || lk === 'sec-ch-dpr' || lk === 'dpr' || lk === 'sec-ch-viewport-width' || lk === 'viewport-width') delete h[k];
    }
  }
  if (settings.cookies.blockThirdParty && isThirdParty(details)) deleteHeader(h, 'cookie');
  if (settings.privacy.blockPrefetch) {
    const purpose = Object.keys(h).find((k) => /^(sec-)?purpose$/i.test(k));
    if (purpose && /prefetch|prerender/i.test(h[purpose])) { callback({ cancel: true }); return; }
  }
  if (settings.privacy.trimReferrers) {
    const ref = Object.keys(h).find((k) => k.toLowerCase() === 'referer');
    if (ref && siteOf(h[ref]) !== siteOf(details.url)) {
      // Opening another site: it doesn't learn where you came from.
      // Loading things from other sites: they only see the site, not the page.
      if (details.resourceType === 'mainFrame') delete h[ref];
      else { const o = originOf(h[ref]); if (/^https?:/.test(o)) h[ref] = o + '/'; else delete h[ref]; }
    }
  }
  callback({ requestHeaders: h });
}

function onHeadersReceived(details, callback) {
  const finish = (res) => {
    let headers = res && res.responseHeaders;
    let changed = !!headers;
    if (!headers) headers = details.responseHeaders;
    if (headers && settings.cookies.blockThirdParty && isThirdParty(details)) {
      for (const k of Object.keys(headers)) {
        if (k.toLowerCase() === 'set-cookie') { delete headers[k]; changed = true; }
      }
    }
    callback(changed ? { responseHeaders: headers } : {});
  };
  const top = topUrlFor(details);
  if (blocker && /^https?:/.test(details.url) && !(top && isAllowlisted(top)) && !skipForExtension(details)) {
    blocker.onHeadersReceived(details, finish);
  } else finish({});
}

function attachRequestHandlers() {
  // Only web traffic goes through Goat's filters; goat://, extension, data: and blob: URLs skip the round trip.
  const filter = { urls: ['http://*/*', 'https://*/*', 'ws://*/*', 'wss://*/*'] };
  for (const s of allSessions()) {
    s.webRequest.onBeforeRequest(filter, onBeforeRequest);
    s.webRequest.onBeforeSendHeaders(filter, onBeforeSendHeaders);
    s.webRequest.onHeadersReceived(filter, onHeadersReceived);
  }
}

// ---------------------------------------------------------------- ad blocker
async function loadBlocker(level) {
  const cacheDir = path.join(userDir(), 'blocklists');
  fs.mkdirSync(cacheDir, { recursive: true });
  const cachePath = path.join(cacheDir, `${level}.bin`);
  try {
    if (Date.now() - fs.statSync(cachePath).mtimeMs > BLOCKLIST_MAX_AGE_MS) fs.rmSync(cachePath, { force: true });
  } catch { /* no cache yet */ }
  const load = ElectronBlocker[BLOCK_LEVELS[level]](
    (url, opts) => net.fetch(url, opts),
    { path: cachePath, read: fs.promises.readFile, write: fs.promises.writeFile },
  );
  const timeout = new Promise((_, rej) => setTimeout(() => rej(new Error('timeout')), 25000));
  return Promise.race([load, timeout]);
}

function disableBlocker() {
  if (!blocker) return;
  for (const s of allSessions()) { try { blocker.disableBlockingInSession(s); } catch { /* already off */ } }
  blocker = null;
  attachRequestHandlers();
}

let retryTimer = null;
async function reloadBlocker() {
  const gen = ++blockerGen;
  clearTimeout(retryTimer);
  disableBlocker();
  if (!settings.adblock.enabled) { shieldState = 'disabled'; scheduleState(); return; }
  shieldState = 'loading';
  scheduleState();
  try {
    const b = await loadBlocker(settings.adblock.level);
    if (gen !== blockerGen) return;
    const rules = settings.adblock.customRules.split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('!'));
    if (rules.length) b.updateFromDiff({ added: rules });
    blocker = b;
    b.enableBlockingInSession(ses);
    if (privSes) {
      // The blocker registers global IPC handlers per session; clear them before the second one.
      ipcMain.removeHandler('@ghostery/adblocker/inject-cosmetic-filters');
      ipcMain.removeHandler('@ghostery/adblocker/is-mutation-observer-enabled');
      b.enableBlockingInSession(privSes);
    }
    // Respect the allowlist for cosmetic filtering too.
    ipcMain.removeHandler('@ghostery/adblocker/inject-cosmetic-filters');
    ipcMain.handle('@ghostery/adblocker/inject-cosmetic-filters', (event, url, msg) => {
      if (isAllowlisted(url)) return undefined;
      return b.onInjectCosmeticFilters(event, url, msg);
    });
    attachRequestHandlers();
    shieldState = 'on';
  } catch (err) {
    if (gen !== blockerGen) return;
    shieldState = 'error';
    toast('Ad blocker could not download its lists. Retrying in 60 s.');
    retryTimer = setTimeout(reloadBlocker, 60000);
  }
  scheduleState();
}

function blockerStatus() {
  return { state: shieldState, level: settings.adblock.level };
}

// ----------------------------------------------------------------- appearance
const HEX = /^#[0-9a-f]{6}$/i;
function hexToRgb(h) { const n = parseInt(h.slice(1), 16); return [n >> 16, (n >> 8) & 255, n & 255]; }
function rgbToHex(c) { return '#' + c.map((v) => Math.round(Math.max(0, Math.min(255, v))).toString(16).padStart(2, '0')).join(''); }
function mix(a, b, t) { const x = hexToRgb(a); const y = hexToRgb(b); return rgbToHex(x.map((v, i) => v + (y[i] - v) * t)); }
function luminance(h) {
  const [r, g, b] = hexToRgb(h).map((v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function sanitizeCustomThemes(list) {
  if (!Array.isArray(list)) return [];
  const out = [];
  for (const t of list.slice(0, 30)) {
    if (!t || typeof t !== 'object' || !/^custom-[a-z0-9]{1,16}$/.test(t.id)) continue;
    const c = t.colors || {};
    const keys = ['bg', 'surface', 'text', 'accent', 'accent2'];
    if (!keys.every((k) => HEX.test(c[k] || ''))) continue;
    out.push({
      id: t.id,
      name: String(t.name || 'My theme').slice(0, 30),
      colors: Object.fromEntries(keys.map((k) => [k, c[k].toLowerCase()])),
    });
  }
  return out;
}

// Full set of CSS variables for a custom theme, derived from five colors.
function customVars(c) {
  const dark = luminance(c.bg) < 0.4;
  return {
    '--bg': c.bg, '--bg-2': c.surface, '--bg-3': mix(c.surface, c.text, 0.08), '--line': mix(c.surface, c.text, 0.15),
    '--text': c.text, '--muted': mix(c.text, c.bg, 0.42), '--accent': c.accent, '--accent-2': c.accent2,
    '--accent-ink': luminance(c.accent) > 0.4 ? '#0b0d10' : '#ffffff',
    '--warn': dark ? '#f0b35a' : '#a35f00', '--danger': dark ? '#ff7a66' : '#c93a2b',
    'color-scheme': dark ? 'dark' : 'light',
  };
}

function themeInfo(id) {
  if (THEMES[id]) return THEMES[id];
  const t = settings && settings.appearance.customThemes.find((x) => x.id === id);
  if (!t) return null;
  return { scheme: luminance(t.colors.bg) < 0.4 ? 'dark' : 'light', bg: t.colors.bg, text: t.colors.text, name: t.name, custom: t };
}

function appearance() {
  const t = settings.appearance.theme;
  const id = t === 'system' || !themeInfo(t) ? (nativeTheme.shouldUseDarkColors ? 'midnight' : 'daylight') : t;
  const info = themeInfo(id);
  return {
    theme: info.custom ? 'custom' : id,
    motion: settings.appearance.motion,
    gradients: settings.appearance.gradients,
    scheme: info.scheme,
    vars: info.custom ? customVars(info.custom.colors) : null,
    bg: info.bg,
    text: info.text,
  };
}

function applyAppearance() {
  const t = settings.appearance.theme;
  const info = themeInfo(t);
  nativeTheme.themeSource = t === 'system' || !info ? 'system' : info.scheme;
  const a = appearance();
  if (win) {
    win.setBackgroundColor(a.bg);
    try { win.setTitleBarOverlay({ color: '#00000000', symbolColor: a.text, height: CONTROLS_HEIGHT }); } catch { /* macOS */ }
  }
  if (ui && !ui.webContents.isDestroyed()) { ui.setBackgroundColor(a.bg); ui.webContents.send('appearance', a); }
  if (panel && !panel.webContents.isDestroyed()) panel.webContents.send('appearance', a);
  for (const tab of tabs.values()) {
    const wc = tab.view.webContents;
    if (!wc.isDestroyed() && wc.getURL().startsWith(`${SCHEME}://`)) { wc.send('goat:appearance', a); tab.view.setBackgroundColor(a.bg); }
  }
}

nativeTheme.on('updated', () => { if (settings && settings.appearance.theme === 'system') applyAppearance(); });

// ---------------------------------------------------------------- DNS & proxy
function dohServer() {
  const d = settings.dns;
  if (d.provider === 'off') return null;
  if (d.provider === 'nextdns') return d.nextdnsId ? `https://dns.nextdns.io/${d.nextdnsId}` : 'https://dns.nextdns.io/';
  if (d.provider === 'custom') return /^https:\/\/[^\s/]+\.[^\s]+/.test(d.custom) ? d.custom : DNS_PROVIDERS.quad9.url;
  return DNS_PROVIDERS[d.provider].url;
}

async function dnsStatus() {
  const server = dohServer();
  const out = { server, provider: settings.dns.provider, check: (DNS_PROVIDERS[settings.dns.provider] || {}).check || null };
  try { out.proxy = await ses.resolveProxy('https://example.com/'); } catch { out.proxy = 'DIRECT'; }
  const t0 = Date.now();
  try {
    const r = await ses.resolveHost('example.com', { cacheUsage: 'disallowed', source: 'dns' });
    out.ok = !!(r && r.endpoints && r.endpoints.length);
    out.ms = Date.now() - t0;
  } catch (err) { out.ok = false; out.error = String(err.message || err); }
  return out;
}
function applyDns() {
  const server = dohServer();
  // The built-in resolver is off by default in Electron on Windows; Chrome uses
  // it, and secure DNS is only reliable with it on.
  app.configureHostResolver(server
    ? { enableBuiltInResolver: true, secureDnsMode: 'secure', secureDnsServers: [server] }
    : { enableBuiltInResolver: true, secureDnsMode: 'off' });
  if (ses) { ses.clearHostResolverCache().catch(() => {}); ses.closeAllConnections().catch(() => {}); }
}

function proxyConfig() {
  // Tor on: everything goes through Tor, or nowhere while it connects (no leaks).
  if (settings.tor.enabled) return { mode: 'fixed_servers', proxyRules: `socks5://127.0.0.1:${tor.port || 9155}`, proxyBypassRules: '' };
  const p = settings.proxy;
  if (p.mode === 'direct') return { mode: 'direct' };
  if (p.mode === 'manual' && p.host && p.port) {
    const scheme = ['http', 'https', 'socks4', 'socks5'].includes(p.type) ? p.type : 'http';
    return { mode: 'fixed_servers', proxyRules: `${scheme}://${p.host}:${p.port}`, proxyBypassRules: p.bypass || '<local>' };
  }
  if (p.mode === 'pac' && p.pacUrl) return { mode: 'pac_script', pacScript: p.pacUrl };
  return { mode: 'system' };
}

async function applyProxy() {
  const cfg = proxyConfig();
  const list = [...allSessions(), session.defaultSession];
  // A VPN extension's proxy (chrome.proxy) applies to normal tabs only, like Chrome; Tor always wins.
  const own = extProxy && !settings.tor.enabled ? extProxy.config : null;
  await Promise.allSettled(list.map((s) => s.setProxy(own && s === ses ? own : cfg)));
  await Promise.allSettled(list.map((s) => s.closeAllConnections()));
}

// ------------------------------------------------------ chrome.proxy for extensions
let extProxy = null; // { owner: extensionId, value: chrome.proxy value, config: Electron proxy config }
let authSeq = 0;
const pendingAuth = new Map(); // requestId -> callback(username, password)
function chromeProxyToElectron(v) {
  const mode = v && v.mode;
  if (mode === 'direct') return { mode: 'direct' };
  if (mode === 'system') return { mode: 'system' };
  if (mode === 'auto_detect') return { mode: 'auto_detect' };
  if (mode === 'pac_script' && v.pacScript) {
    if (v.pacScript.data) return { mode: 'pac_script', pacScript: 'data:application/x-ns-proxy-autoconfig;base64,' + Buffer.from(String(v.pacScript.data), 'utf8').toString('base64') };
    if (v.pacScript.url) return { mode: 'pac_script', pacScript: String(v.pacScript.url) };
  }
  if (mode === 'fixed_servers' && v.rules) {
    const r = v.rules;
    const one = (p) => (p && p.host ? `${p.scheme || 'http'}://${p.host}${p.port ? ':' + p.port : ''}` : '');
    let rules = '';
    if (r.singleProxy) rules = one(r.singleProxy);
    else rules = [['http', r.proxyForHttp], ['https', r.proxyForHttps], ['ftp', r.proxyForFtp]].filter(([, x]) => x).map(([k, x]) => `${k}=${one(x)}`).join(';') + (r.fallbackProxy ? `;${one(r.fallbackProxy)}` : '');
    return { mode: 'fixed_servers', proxyRules: rules, proxyBypassRules: (r.bypassList || []).join(',') };
  }
  return null;
}
function wireExtensionProxy() {
  const handle = extensions.ctx.router.apiHandler();
  const level = (extId) => (settings.tor.enabled ? 'not_controllable'
    : !extProxy ? 'controllable_by_this_extension'
      : extProxy.owner === extId ? 'controlled_by_this_extension' : 'controlled_by_other_extensions');
  const current = (extId) => ({
    levelOfControl: level(extId),
    value: extProxy ? extProxy.value : (settings.proxy.mode === 'direct' ? { mode: 'direct' } : { mode: 'system' }),
    incognitoSpecific: false,
  });
  const changed = () => { for (const e of extApi().getAllExtensions()) extensions.ctx.router.sendEvent(e.id, 'goat.proxy.onChange', current(e.id)); };
  handle('goat.proxy.get', ({ extension }) => current(extension.id));
  handle('goat.proxy.set', async ({ extension }, details) => {
    if (settings.tor.enabled) return undefined; // Tor is in charge
    const config = chromeProxyToElectron(details && details.value);
    if (!config) return undefined;
    extProxy = { owner: extension.id, value: details.value, config };
    await applyProxy();
    changed();
    toast(`${extText(extension, extension.name)} is now handling your connection`);
    return undefined;
  });
  handle('goat.proxy.clear', async ({ extension }) => {
    if (extProxy && extProxy.owner === extension.id) { extProxy = null; await applyProxy(); changed(); }
    return undefined;
  });
  handle('goat.proxy.authReply', ({ extension }, requestId, resp) => {
    const cb = pendingAuth.get(requestId);
    if (!cb || !extProxy || extProxy.owner !== extension.id) return undefined;
    pendingAuth.delete(requestId);
    const c = resp && resp.authCredentials;
    if (c && c.username != null) cb(String(c.username), String(c.password || ''));
    else cb();
    return undefined;
  });
  extApi().on('extension-unloaded', (_e, ext) => { if (extProxy && extProxy.owner === ext.id) { extProxy = null; applyProxy(); } });
  // Chrome keeps an extension's proxy across restarts; so does Goat (only while that extension is installed).
  handle('goat.runtime.externalReply', (_ctx, requestId, resp) => {
    const done = pendingExtMsgs.get(requestId);
    if (done) { pendingExtMsgs.delete(requestId); done(resp); }
    return undefined;
  });
  ipcMain.handle('goat:ext-external', async (event, extId, message) => {
    const url = (event.senderFrame && event.senderFrame.url) || '';
    if (event.sender.session !== ses) return { error: 'Extensions are off in private windows.' };
    if (!externallyConnectableIds(url).includes(String(extId))) return { error: 'This site may not talk to that extension.' };
    const tab = tabByWebContentsId(event.sender.id);
    let msg; try { msg = JSON.parse(JSON.stringify(message)); } catch { return { error: 'Message could not be sent.' }; }
    return deliverExternalMessage(String(extId), msg, { url, origin: originOf(url), frameId: 0, tab: tab ? { id: event.sender.id, url, active: tab.id === activeId } : undefined });
  });
}
// ------------------------------------- website -> extension messages (externally_connectable)
// e.g. Proton VPN: you sign in on account.proton.me and the page hands the login to the extension.
function matchUrlPattern(pattern, url) {
  if (pattern === '<all_urls>') return /^(https?|wss?|ftp):/.test(url);
  const m = /^(\*|https?|wss?|ftp):\/\/([^/]+)(\/.*)?$/.exec(pattern);
  let u; try { u = new URL(url); } catch { return false; }
  if (!m) return false;
  const scheme = u.protocol.slice(0, -1);
  if (m[1] === '*' ? !/^https?$/.test(scheme) : m[1] !== scheme) return false;
  const host = m[2].toLowerCase(); const h = u.hostname.toLowerCase();
  if (host !== '*' && host !== h && !(host.startsWith('*.') && (h === host.slice(2) || h.endsWith(host.slice(1))))) return false;
  const pathRe = new RegExp('^' + (m[3] || '/*').split('*').map((x) => x.replace(/[.+?^${}()|[\]\\]/g, '\\$&')).join('.*') + '$');
  return pathRe.test(u.pathname + u.search);
}
function externallyConnectableIds(url) {
  if (!extensions || !/^https?:/.test(url)) return [];
  return extApi().getAllExtensions().filter((e) => {
    const ec = e.manifest && e.manifest.externally_connectable;
    return ec && Array.isArray(ec.matches) && ec.matches.some((p) => matchUrlPattern(p, url));
  }).map((e) => e.id);
}
let extMsgSeq = 0;
const pendingExtMsgs = new Map(); // requestId -> resolve
async function deliverExternalMessage(extId, message, sender) {
  const router = extensions.ctx.router;
  const listenersOf = () => {
    const obs = router.sessionMap && router.sessionMap.get(ses);
    const map = (obs && obs.listeners) || router.listeners;
    return (map && map.get('goat.runtime.messageExternal')) || [];
  };
  const hasListener = () => listenersOf().some((l) => l.extensionId === extId);
  if (!hasListener()) {
    // Wake the extension's background worker, then give it a moment to register.
    try { await ses.serviceWorkers.startWorkerForScope(`chrome-extension://${extId}/`); } catch { /* not a worker */ }
    for (let i = 0; i < 30 && !hasListener(); i++) await new Promise((r) => setTimeout(r, 100));
    if (!hasListener()) return { error: 'The extension is not listening.' };
  }
  const requestId = `goat-ext-${++extMsgSeq}`;
  return new Promise((resolve) => {
    const timer = setTimeout(() => { if (pendingExtMsgs.delete(requestId)) resolve({ error: 'The extension did not answer.' }); }, 30000);
    pendingExtMsgs.set(requestId, (resp) => { clearTimeout(timer); resolve({ response: resp }); });
    router.sendEvent(extId, 'goat.runtime.messageExternal', message, sender, requestId);
  });
}

// ------------------------------------------------- extension debug log
// Writes what happens in extension popups and background workers to goat-debug.log in the
// profile folder: popup position/size/focus, every click and key that reaches the popup
// page, and errors. Used to find out why extension buttons don't respond on a real PC.
function debugLog(line) {
  const file = path.join(userDir(), 'goat-debug.log');
  try {
    try { if (fs.statSync(file).size > 1024 * 1024) fs.renameSync(file, file + '.old'); } catch { /* new */ }
    fs.appendFileSync(file, `${new Date().toISOString()} ${line}\n`);
  } catch { /* ignore */ }
}
function wireExtensionDiagnostics() {
  const { screen } = require('electron');
  const nameOf = (url) => {
    const m = /^chrome-extension:\/\/([a-p]{32})/.exec(url || '');
    if (!m) return null;
    const e = extApi().getExtension(m[1]);
    return e ? extText(e, e.name) : m[1];
  };
  const LEVEL = { 0: 'log', 1: 'info', 2: 'warn', 3: 'error', verbose: 'log', info: 'info', warning: 'warn', error: 'error' };
  extensions.on('browser-action-popup-created', (popup) => {
    const pw = popup.browserWindow;
    if (!pw) return;
    const who = nameOf(pw.webContents.getURL()) || popup.extensionId;
    const disp = screen.getDisplayMatching(win.getBounds());
    debugLog(`[popup ${who}] created; main window content ${JSON.stringify(win.getContentBounds())}, display scale ${disp.scaleFactor}, anchor ${JSON.stringify(popup.anchorRect)}`);
    pw.on('show', () => debugLog(`[popup ${who}] shown at ${JSON.stringify(pw.getBounds())}, focused=${pw.isFocused()}, visible=${pw.isVisible()}`));
    pw.on('focus', () => debugLog(`[popup ${who}] focus`));
    pw.on('blur', () => debugLog(`[popup ${who}] blur (focused window now: ${(() => { const f = require('electron').BrowserWindow.getFocusedWindow(); return f ? f.id : 'none'; })()})`));
    pw.on('resize', () => debugLog(`[popup ${who}] bounds ${JSON.stringify(pw.getBounds())}`));
    pw.on('closed', () => debugLog(`[popup ${who}] closed`));
    const wc = pw.webContents;
    wc.on('console-message', (e) => debugLog(`[popup ${who} ${LEVEL[e.level] || e.level}] ${e.message} @ ${e.sourceId}:${e.lineNumber}`));
    wc.on('did-fail-load', (_e, code, desc, url) => debugLog(`[popup ${who}] failed to load ${url}: ${desc} (${code})`));
    wc.on('render-process-gone', (_e, d) => debugLog(`[popup ${who}] page crashed: ${d.reason}`));
    wc.on('before-input-event', (_e, input) => { if (input.type === 'keyDown') debugLog(`[popup ${who}] key ${input.key}`); });
    wc.on('dom-ready', () => {
      // Report every mouse press/click that reaches the page, and what it hit.
      wc.executeJavaScriptInIsolatedWorld(999, [{ code: `
        const d = (t, e) => { const el = e.target; console.debug('[goat-diag] ' + t + ' at ' + e.clientX + ',' + e.clientY + ' on <' + (el.tagName || '?').toLowerCase() + (el.id ? '#' + el.id : '') + (el.className && typeof el.className === 'string' ? '.' + el.className.trim().split(/\\s+/).join('.') : '') + '> "' + String(el.textContent || '').trim().slice(0, 40) + '"' + (e.defaultPrevented ? ' (handled)' : '')); };
        document.addEventListener('pointerdown', (e) => d('press', e), true);
        document.addEventListener('click', (e) => setTimeout(() => d('click', e), 0), true);
        window.addEventListener('error', (e) => console.debug('[goat-diag] page error: ' + e.message));
        window.addEventListener('unhandledrejection', (e) => console.debug('[goat-diag] unhandled promise: ' + (e.reason && e.reason.message || e.reason)));
        'ok';` }]).catch((err) => debugLog(`[popup ${who}] couldn't watch clicks: ${err.message}`));
      debugLog(`[popup ${who}] page ready: ${wc.getURL()}`);
    });
  });
  // Extension background workers: errors and warnings.
  ses.serviceWorkers.on('console-message', (_e, d) => {
    const who = nameOf(d.sourceUrl || d.scope || '');
    const lvl = LEVEL[d.level] || d.level;
    if (who && (lvl === 'error' || lvl === 'warn')) debugLog(`[background ${who} ${lvl}] ${d.message} @ ${d.sourceUrl}:${d.lineNumber}`);
  });
  // Native messaging (e.g. iCloud Passwords <-> iCloud for Windows): connection steps and errors only,
  // never message contents (they can contain passwords).
  process.on('goat-debug', (line) => { if (!/\] (receive|send|sending)\b/.test(line)) debugLog(line); });
  debugLog(`--- Goat ${require(path.join(__dirname, '..', 'package.json')).version} started; extensions: ${extApi().getAllExtensions().map((e) => extText(e, e.name)).join(', ') || 'none yet'}`);
}

// The VPN extension's servers ask for a login: let the extension answer, as Chrome does.
function askExtensionForProxyLogin(authInfo, url, callback) {
  if (!extProxy || !extensions) return false;
  const requestId = `goat-${++authSeq}`;
  const timer = setTimeout(() => { if (pendingAuth.delete(requestId)) callback(); }, 25000);
  pendingAuth.set(requestId, (u, p) => { clearTimeout(timer); if (u == null) callback(); else callback(u, p); });
  extensions.ctx.router.sendEvent(extProxy.owner, 'goat.proxyAuth', {
    requestId, url: url || '', method: 'GET', frameId: 0, parentFrameId: -1, tabId: -1, type: 'other', timeStamp: Date.now(),
    isProxy: true, scheme: authInfo.scheme, realm: authInfo.realm, statusCode: 407, statusLine: 'HTTP/1.1 407 Proxy Authentication Required',
    challenger: { host: authInfo.host, port: authInfo.port },
  });
  return true;
}

app.on('login', (event, _wc, details, authInfo, callback) => {
  if (authInfo.isProxy && extProxy && !settings.tor.enabled) {
    event.preventDefault();
    if (!askExtensionForProxyLogin(authInfo, details && details.url, callback)) callback();
    return;
  }
  const p = settings && settings.proxy;
  if (authInfo.isProxy && p && p.mode === 'manual' && p.username) {
    event.preventDefault();
    callback(p.username, decryptSecret(p.password));
  }
});

// ---------------------------------------------------------------- permissions
const ALWAYS_ALLOW = new Set(['fullscreen', 'clipboard-sanitized-write', 'pointerLock']);
const ASKABLE = new Set(['media', 'geolocation', 'notifications', 'display-capture',
  'midi', 'midiSysex', 'idle-detection', 'window-management']);
const PERM_LABEL = {
  geolocation: 'location', notifications: 'notifications', 'display-capture': 'screen',
  midi: 'MIDI devices', midiSysex: 'MIDI devices', 'idle-detection': 'idle detection',
  'window-management': 'window placement',
};

function onPermissionRequest(wc, permission, callback, details) {
  if (ALWAYS_ALLOW.has(permission)) { callback(true); return; }
  if (!ASKABLE.has(permission)) { callback(false); return; }
  const origin = originOf(details.requestingUrl || wc.getURL());
  const key = `${origin}|${permission}|${(details.mediaTypes || []).join(',')}`;
  if (permDecisions.has(key)) { callback(permDecisions.get(key)); return; }
  let what = PERM_LABEL[permission] || permission;
  if (permission === 'media') {
    const m = details.mediaTypes || [];
    what = m.includes('video') && m.includes('audio') ? 'camera and microphone'
      : m.includes('video') ? 'camera' : 'microphone';
  }
  dialog.showMessageBox(win, {
    type: 'question', buttons: ['Block', 'Allow'], defaultId: 0, cancelId: 0,
    message: `${origin} wants to use your ${what}.`,
    detail: 'This choice lasts until you close Goat.',
  }).then(({ response }) => {
    const ok = response === 1;
    permDecisions.set(key, ok);
    callback(ok);
  }).catch(() => callback(false));
}

function onPermissionCheck(_wc, permission, requestingOrigin) {
  if (ALWAYS_ALLOW.has(permission)) return true;
  const prefix = `${originOf(requestingOrigin)}|${permission}|`;
  for (const [k, v] of permDecisions) if (k.startsWith(prefix) && v) return true;
  return false;
}

// -------------------------------------------------------------------- session
const allSessions = () => [ses, privSes].filter(Boolean);

function configureSession(s) {
  s.setUserAgent(USER_AGENT, acceptLanguage(settings.languages.sites));
  s.setSpellCheckerEnabled(false);
  s.setPermissionRequestHandler(onPermissionRequest);
  s.setPermissionCheckHandler(onPermissionCheck);
  s.setDevicePermissionHandler(() => false);
  s.registerPreloadScript({ type: 'frame', filePath: path.join(__dirname, 'pages-preload.js') });
  s.registerPreloadScript({ type: 'frame', filePath: path.join(__dirname, 'fp-preload.js') });
  s.protocol.handle(SCHEME, serveInternal);
  s.on('will-download', (_e, item) => {
    item.once('done', (_ev, state) => {
      const name = item.getFilename();
      toast(state === 'completed' ? `Downloaded ${name}` : `Download ${state}: ${name}`);
    });
  });
}

// ------------------------------------------------------------ site icons
// Each site's icon, saved (as a small PNG) when a normal tab shows it, for tabs and bookmarks.
const iconCache = new Map(); // host -> data: URL, or null when there's none
const iconsDir = () => path.join(userDir(), 'favicons');
const iconFile = (host) => path.join(iconsDir(), host.replace(/[^a-z0-9.-]/gi, '') + '.png');
function hostIcon(url) {
  if (settings.appearance.siteIcons === false || !/^https?:/i.test(url || '')) return null;
  let host;
  try { host = new URL(url).hostname; } catch { return null; }
  if (!iconCache.has(host)) {
    let v = null;
    try { v = 'data:image/png;base64,' + fs.readFileSync(iconFile(host)).toString('base64'); } catch { /* none yet */ }
    iconCache.set(host, v);
  }
  return iconCache.get(host);
}
// Bookmarks (and folders) with their site icons, for the bookmarks bar and the new tab page.
function withIcons(items) {
  return items.map((b) => (b.type === 'folder' ? { ...b, children: withIcons(b.children || []) } : { ...b, icon: hostIcon(b.url) }));
}
async function loadTabIcon(tab, wc, iconUrl) {
  try {
    const r = await wc.session.fetch(iconUrl);
    if (!r.ok) return;
    const { nativeImage } = require('electron');
    let img = nativeImage.createFromBuffer(Buffer.from(await r.arrayBuffer()));
    if (img.isEmpty()) return;
    img = img.resize({ width: 32, height: 32, quality: 'best' });
    const png = img.toPNG();
    const host = new URL(displayUrl(wc)).hostname;
    tab.icon = 'data:image/png;base64,' + png.toString('base64');
    tab.iconHost = host;
    iconCache.set(host, tab.icon);
    if (!tab.private && !EPHEMERAL) {
      fs.mkdirSync(iconsDir(), { recursive: true });
      fs.writeFileSync(iconFile(host), png);
    }
    scheduleState();
  } catch { /* no icon */ }
}

function serveInternal(req) {
  const u = new URL(req.url);
  const rel = path.normalize(path.join(u.hostname, decodeURIComponent(u.pathname)));
  let file = path.join(PAGES_DIR, rel);
  if (!file.startsWith(PAGES_DIR + path.sep)) return new Response('Not found', { status: 404 });
  if (u.pathname === '/' || u.pathname === '') file = path.join(file, 'index.html');
  return net.fetch(pathToFileURL(file).toString());
}

function setupSession() {
  ses = session.fromPartition(EPHEMERAL ? 'goat-window' : 'persist:goat'); // normal: cookies & site data survive restarts
  privSes = session.fromPartition('goat-private'); // no "persist:" = memory only
  configureSession(ses);
  configureSession(privSes);
  attachRequestHandlers();
}

// "Forget when closed": once no normal tab shows the site, its cookies and storage go.
async function forgetSiteIfClosed(site) {
  if (!site || !settings.privacy.forgetSites.includes(site)) return;
  for (const t of tabs.values()) if (!t.private && t.site === site) return;
  const all = await ses.cookies.get({}).catch(() => []);
  const mine = all.filter((c) => siteOf('https://' + c.domain.replace(/^\./, '')) === site);
  await Promise.allSettled(mine.map((c) => ses.cookies.remove(cookieUrl(c), c.name)));
  const hosts = new Set([site, 'www.' + site, ...mine.map((c) => c.domain.replace(/^\./, ''))]);
  const storages = ['filesystem', 'indexdb', 'localstorage', 'shadercache', 'websql', 'serviceworkers', 'cachestorage'];
  await Promise.allSettled([...hosts].flatMap((h) => ['https://', 'http://'].map((pre) => ses.clearStorageData({ origin: pre + h, storages }))));
}

// Private tabs share one in-memory session; when the last one closes it is wiped.
async function wipePrivateIfUnused() {
  if (!privSes || [...tabs.values()].some((t) => t.private)) return;
  await Promise.allSettled([
    privSes.clearStorageData(), privSes.clearCache(), privSes.clearAuthCache(),
    privSes.clearHostResolverCache(), privSes.clearCodeCaches({}),
  ]);
}

// ----------------------------------------------------------------------- tabs
function createTab(url, { activate = true, groupId = null, position = 'afterActive', lazy = false, private: isPrivate = false } = {}) {
  if (EPHEMERAL) isPrivate = true; // private/Tor windows: every tab is private
  const id = nextId++;
  const view = new WebContentsView({
    webPreferences: {
      session: isPrivate ? privSes : ses, contextIsolation: true, sandbox: true, nodeIntegration: false,
      spellcheck: false, safeDialogs: true, navigateOnDragDrop: true,
    },
  });
  // Theme color for Goat's own pages avoids a white flash; web pages get white.
  view.setBackgroundColor(url && /^https?:/i.test(url) ? '#ffffff' : appearance().bg);
  const tab = { id, view, title: '', blocked: 0, groupId: groups.has(groupId) ? groupId : null, lastActive: Date.now(), private: !!isPrivate };
  tabs.set(id, tab);

  if (tab.groupId) {
    insertInGroup(id, tab.groupId);
    groups.get(tab.groupId).collapsed = false;
  } else if (position === 'end') {
    order.push(id);
  } else {
    // After the active tab, but never inside a group the tab doesn't belong to.
    let i = order.indexOf(activeId);
    const g = i >= 0 ? tabs.get(order[i]).groupId : null;
    if (g) while (i + 1 < order.length && tabs.get(order[i + 1]).groupId === g) i++;
    order.splice(i >= 0 ? i + 1 : order.length, 0, id);
  }

  view.setVisible(false);
  win.contentView.addChildView(view);
  const wc = view.webContents;
  wc.setWebRTCIPHandlingPolicy('disable_non_proxied_udp');
  wireTab(tab);
  if (extensions && !isPrivate) extensions.addTab(wc, win); // extensions stay out of private tabs
  // Restored background tabs load when first opened, so startup stays fast.
  if (lazy && url && !activate) pendingUrls.set(wc, url);
  else wc.loadURL(url || (isPrivate ? NEWTAB_URL + '?private=1' : NEWTAB_URL));
  if (activate) activateTab(id, !url);
  else scheduleState();
  saveTabsSoon();
  return tab;
}

function insertInGroup(tabId, groupId) {
  order = order.filter((x) => x !== tabId);
  let last = -1;
  order.forEach((x, i) => { if (tabs.get(x).groupId === groupId) last = i; });
  if (last >= 0) order.splice(last + 1, 0, tabId);
  else order.push(tabId);
}

function openInternal(url) {
  for (const id of order) {
    const wc = tabs.get(id).view.webContents;
    if (wc.getURL().startsWith(url)) { activateTab(id); return; }
  }
  createTab(url);
}

function wireTab(tab) {
  const wc = tab.view.webContents;

  wc.setWindowOpenHandler(({ url, disposition }) => {
    if (disposition === 'new-window') {
      return {
        action: 'allow',
        overrideBrowserWindowOptions: {
          autoHideMenuBar: true, width: 520, height: 680,
          webPreferences: { contextIsolation: true, sandbox: true, nodeIntegration: false },
        },
      };
    }
    if (/^(https?|goat):/.test(url)) {
      createTab(url, { activate: disposition !== 'background-tab', groupId: tab.groupId, private: tab.private });
    }
    return { action: 'deny' };
  });
  wc.on('did-create-window', (child) => {
    child.webContents.setWebRTCIPHandlingPolicy('disable_non_proxied_udp');
  });

  wc.on('page-title-updated', (_e, title) => { tab.title = title; updateHistoryTitle(tab, title); scheduleState(); });
  wc.on('did-start-loading', scheduleState);
  wc.on('did-stop-loading', () => { scheduleState(); saveTabsSoon(); checkTranslate(tab); });
  wc.on('did-navigate', (_e, navUrl, httpCode) => {
    tab.title = '';
    stopTranslateWatch(tab);
    tab.tr = null;
    if (!tab.private && /^https?:/.test(navUrl)) {
      const prevSite = tab.site;
      tab.site = siteOf(navUrl);
      if (prevSite && prevSite !== tab.site) setImmediate(() => forgetSiteIfClosed(prevSite));
    }
    if (!(httpCode >= 400)) recordVisit(tab, navUrl);
    tab.view.setBackgroundColor(/^https?:/i.test(navUrl) ? '#ffffff' : appearance().bg);
    scheduleState();
  });
  wc.on('did-navigate-in-page', (_e, navUrl, isMainFrame) => { if (isMainFrame) recordVisit(tab, navUrl, true); scheduleState(); });
  wc.on('did-start-navigation', (d) => {
    if (d.isMainFrame && !d.isSameDocument) { tab.blocked = 0; scheduleState(); }
  });
  wc.on('page-favicon-updated', (_e, list) => { if (list && list[0]) loadTabIcon(tab, wc, list[0]); });

  wc.on('will-navigate', (e) => {
    const target = e.url;
    if (!target.startsWith(`${SCHEME}:`)) return;
    const current = wc.getURL();
    if (target.startsWith(`${SCHEME}://allow-http/`) && current.startsWith(`${SCHEME}://error/`)) {
      e.preventDefault();
      try {
        const insecure = new URL(new URL(target).searchParams.get('u'));
        insecure.protocol = 'http:';
        allowHttp.add(insecure.hostname);
        wc.loadURL(insecure.toString());
      } catch { /* bad input */ }
    } else if (!current.startsWith(`${SCHEME}://`)) {
      e.preventDefault(); // websites may not open internal pages
    }
  });

  wc.on('did-fail-load', (_e, code, desc, url, isMainFrame) => {
    if (!isMainFrame || code === -3 || !url || url.startsWith(`${SCHEME}:`)) return;
    showError(wc, url, code, desc);
  });
  wc.on('render-process-gone', (_e, details) => {
    if (details.reason === 'clean-exit' || tab.discarded) return;
    const url = displayUrl(wc) || NEWTAB_URL;
    setImmediate(() => { if (!wc.isDestroyed()) showError(wc, url, 'crashed', details.reason); });
  });

  wc.on('before-input-event', handleShortcut);
  wc.on('context-menu', (_e, p) => showContextMenu(wc, p));
  wc.on('found-in-page', (_e, r) => {
    if (tab.id === activeId) ui.webContents.send('find-result', { active: r.activeMatchOrdinal, total: r.matches });
  });
  wc.on('enter-html-full-screen', () => setFullscreen(true));
  wc.on('leave-html-full-screen', () => setFullscreen(false));
  wc.on('zoom-changed', (_e, dir) => zoom(dir === 'in' ? 0.5 : -0.5));
}

function showError(wc, url, code, desc) {
  let host = '';
  try { host = new URL(url).hostname; } catch { /* ignore */ }
  const canHttp = url.startsWith('https:') && upgradedHosts.has(host) && !allowHttp.has(host);
  const q = new URLSearchParams({ u: url, code: String(code), d: desc || '', http: canHttp ? '1' : '0' });
  wc.loadURL(`${SCHEME}://error/?${q}`);
}

function activateTab(id, focusAddress = false) {
  const t = tabs.get(id);
  if (!t) return;
  if (t.groupId && groups.get(t.groupId)?.collapsed) groups.get(t.groupId).collapsed = false;
  const wc = t.view.webContents;
  if (t.discarded) { t.discarded = false; wc.reload(); }
  else if (pendingUrls.has(wc)) { const u = pendingUrls.get(wc); pendingUrls.delete(wc); wc.loadURL(u); }
  else if (wc.getURL().startsWith(`${SCHEME}://sleep/`)) wakeTab(t);
  const prev = tabs.get(activeId);
  if (prev && prev.id !== id) {
    prev.lastActive = Date.now();
    // Background new-tab pages cost ~100 MB each and reload instantly, so drop them fast.
    const prevWc = prev.view.webContents;
    if (!prevWc.isDestroyed() && prevWc.getURL().startsWith(NEWTAB_URL)) {
      setTimeout(() => { if (tabs.has(prev.id) && activeId !== prev.id) discardTab(prev); }, 5000);
    }
  }
  t.lastActive = Date.now();
  for (const other of tabs.values()) other.view.setVisible(other.id === id);
  if (extensions && !t.private) extensions.selectTab(t.view.webContents);
  activeId = id;
  layout();
  if (focusAddress) { ui.webContents.focus(); ui.webContents.send('focus-address'); }
  else t.view.webContents.focus();
  ui.webContents.send('find-close');
  pushPanel(true);
  scheduleState();
}

function destroyTab(id) {
  const t = tabs.get(id);
  if (!t) return;
  tabs.delete(id);
  order = order.filter((x) => x !== id);
  win.contentView.removeChildView(t.view);
  if (!t.private) { try { extensions?.removeTab(t.view.webContents); } catch { /* not tracked */ } }
  stopTranslateWatch(t);
  if (t.private) setImmediate(wipePrivateIfUnused);
  else if (t.site) setImmediate(() => forgetSiteIfClosed(t.site));
  if (!t.view.webContents.isDestroyed()) t.view.webContents.close();
}

function cleanupGroups() {
  for (const gid of [...groups.keys()]) {
    if (!order.some((id) => tabs.get(id).groupId === gid)) groups.delete(gid);
  }
}

function visibleTabs() {
  return order.filter((id) => {
    const g = tabs.get(id).groupId;
    return !(g && groups.get(g)?.collapsed);
  });
}

function closeTabs(ids) {
  const wasActive = ids.includes(activeId);
  const idx = order.indexOf(activeId);
  for (const id of ids) destroyTab(id);
  cleanupGroups();
  if (order.length === 0) { saveTabsNow(); win?.close(); return; } // last tab closed: close the browser
  if (wasActive) {
    const vis = visibleTabs();
    const pick = vis.length ? vis[Math.min(idx, vis.length - 1)] : order[Math.min(idx, order.length - 1)];
    activateTab(pick);
  } else scheduleState();
  saveTabsSoon();
}
const closeTab = (id) => { if (tabs.has(id)) closeTabs([id]); };

function activeWc() {
  const t = tabs.get(activeId);
  return t ? t.view.webContents : null;
}

function navigate(text) {
  const url = toUrl(text);
  const wc = activeWc();
  if (!url || !wc) return;
  wc.loadURL(url);
  wc.focus();
}

function reload(hard) {
  const wc = activeWc();
  if (!wc) return;
  const cur = wc.getURL();
  if (cur.startsWith(`${SCHEME}://error/`)) {
    const u = displayUrl(wc);
    if (u && u !== cur) { wc.loadURL(u); return; }
  }
  if (hard) wc.reloadIgnoringCache(); else wc.reload();
}

function zoom(delta) {
  const wc = activeWc();
  if (!wc) return;
  wc.setZoomLevel(delta === 0 ? 0 : Math.max(-5, Math.min(5, wc.getZoomLevel() + delta)));
}

function cycleTab(step) {
  const vis = visibleTabs();
  if (vis.length < 2) return;
  const i = vis.indexOf(activeId);
  activateTab(vis[(i + step + vis.length) % vis.length]);
}

function setFullscreen(on) {
  fullscreen = on;
  win.setFullScreen(on);
  layout();
}

async function clearBrowsingData() {
  for (const id of [...order]) destroyTab(id);
  groups.clear();
  activeId = null;
  allowHttp.clear();
  upgradedHosts.clear();
  permDecisions.clear();
  await Promise.allSettled([
    ses.clearStorageData(), ses.clearCache(), ses.clearAuthCache(),
    ses.clearHostResolverCache(), ses.clearCodeCaches({}),
  ]);
  clearHistory('all');
  createTab(null);
  saveTabsSoon();
  toast('Browsing data cleared: history, cookies, cache, storage and tabs.');
}

// ----------------------------------------------------------------- tab groups
function newGroupWith(tabIds) {
  const id = nextGroupId++;
  const used = new Set([...groups.values()].map((g) => g.color));
  const color = GROUP_COLORS.find((c) => !used.has(c)) || GROUP_COLORS[id % GROUP_COLORS.length];
  groups.set(id, { id, name: '', color, collapsed: false });
  const first = Math.min(...tabIds.map((t) => order.indexOf(t)));
  order = order.filter((x) => !tabIds.includes(x));
  order.splice(Math.min(first, order.length), 0, ...tabIds);
  for (const t of tabIds) tabs.get(t).groupId = id;
  cleanupGroups();
  scheduleState();
  saveTabsSoon();
  setTimeout(() => ui.webContents.send('rename-group', id), 60);
  return id;
}

function addToGroup(tabId, groupId) {
  const t = tabs.get(tabId);
  if (!t || !groups.has(groupId)) return;
  t.groupId = groupId;
  insertInGroup(tabId, groupId);
  groups.get(groupId).collapsed = false;
  cleanupGroups();
  scheduleState();
  saveTabsSoon();
}

function removeFromGroup(tabId) {
  const t = tabs.get(tabId);
  if (!t || !t.groupId) return;
  const gid = t.groupId;
  let last = -1;
  order.forEach((x, i) => { if (tabs.get(x).groupId === gid) last = i; });
  order = order.filter((x) => x !== tabId);
  order.splice(Math.min(last, order.length), 0, tabId);
  t.groupId = null;
  cleanupGroups();
  scheduleState();
  saveTabsSoon();
}

function setGroupCollapsed(gid, collapsed) {
  const g = groups.get(gid);
  if (!g) return;
  g.collapsed = collapsed;
  if (collapsed && tabs.get(activeId)?.groupId === gid) {
    const outside = order.filter((id) => tabs.get(id).groupId !== gid && !groups.get(tabs.get(id).groupId)?.collapsed);
    if (outside.length) activateTab(outside[0]);
    else createTab(null, { position: 'end' });
  }
  scheduleState();
  saveTabsSoon();
}

function moveTab(id, beforeId, groupId) {
  const t = tabs.get(id);
  if (!t || id === beforeId) return;
  order = order.filter((x) => x !== id);
  const i = beforeId != null ? order.indexOf(beforeId) : -1;
  order.splice(i >= 0 ? i : order.length, 0, id);
  t.groupId = groups.has(groupId) ? groupId : null;
  // Keep groups contiguous: pull stray members next to each other.
  for (const gid of groups.keys()) {
    const members = order.filter((x) => tabs.get(x).groupId === gid);
    if (!members.length) continue;
    const at = order.indexOf(members[0]);
    order = order.filter((x) => tabs.get(x).groupId !== gid);
    order.splice(Math.min(at, order.length), 0, ...members);
  }
  cleanupGroups();
  scheduleState();
  saveTabsSoon();
}

// Drop one tab on another: join its folder, or make a new folder from both.
function mergeTabs(dragId, targetId) {
  const a = tabs.get(dragId);
  const b = tabs.get(targetId);
  if (!a || !b || dragId === targetId) return;
  if (b.groupId) { addToGroup(dragId, b.groupId); return; }
  order = order.filter((x) => x !== dragId);
  order.splice(order.indexOf(targetId) + 1, 0, dragId);
  a.groupId = null;
  newGroupWith([targetId, dragId]);
}

function moveGroup(gid, beforeId) {
  if (!groups.has(gid)) return;
  const members = order.filter((x) => tabs.get(x).groupId === gid);
  if (beforeId != null && members.includes(beforeId)) return;
  order = order.filter((x) => tabs.get(x).groupId !== gid);
  let i = beforeId != null ? order.indexOf(beforeId) : -1;
  if (i >= 0) {
    // Never split another folder: snap to its start.
    const g = tabs.get(order[i]).groupId;
    if (g) while (i > 0 && tabs.get(order[i - 1]).groupId === g) i--;
  }
  order.splice(i >= 0 ? i : order.length, 0, ...members);
  scheduleState();
  saveTabsSoon();
}

function groupMenu(gid) {
  const g = groups.get(gid);
  if (!g) return;
  const ids = order.filter((id) => tabs.get(id).groupId === gid);
  Menu.buildFromTemplate([
    { label: 'New tab in group', click: () => createTab(null, { groupId: gid }) },
    { label: 'Rename group', click: () => ui.webContents.send('rename-group', gid) },
    {
      label: 'Color',
      submenu: GROUP_COLORS.map((c) => ({
        label: c[0].toUpperCase() + c.slice(1), type: 'radio', checked: g.color === c,
        click: () => { g.color = c; scheduleState(); saveTabsSoon(); },
      })),
    },
    { label: g.collapsed ? 'Expand group' : 'Collapse group', click: () => setGroupCollapsed(gid, !g.collapsed) },
    { type: 'separator' },
    { label: 'Ungroup', click: () => { for (const id of ids) tabs.get(id).groupId = null; cleanupGroups(); scheduleState(); saveTabsSoon(); } },
    { label: 'Close group', click: () => closeTabs(ids) },
  ]).popup({ window: win });
}

function tabMenu(id) {
  const t = tabs.get(id);
  if (!t) return;
  const wc = t.view.webContents;
  const others = [...groups.values()].filter((g) => g.id !== t.groupId);
  const items = [
    { label: 'New tab to the right', click: () => { activateTab(id); createTab(null, { groupId: t.groupId, private: t.private }); } },
    { label: 'New private tab', accelerator: 'CmdOrCtrl+Shift+N', click: () => createTab(null, { position: 'end', private: true }) },
    { type: 'separator' },
    { label: 'Add to new group', click: () => newGroupWith([id]) },
  ];
  if (others.length) {
    items.push({
      label: 'Add to group',
      submenu: others.map((g) => ({ label: g.name || `${g.color} group`, click: () => addToGroup(id, g.id) })),
    });
  }
  if (t.groupId) items.push({ label: 'Remove from group', click: () => removeFromGroup(id) });
  items.push(
    { type: 'separator' },
    { label: 'Reload', click: () => wc.reload() },
    { label: 'Duplicate', click: () => { const u = wc.getURL(); activateTab(id); createTab(u, { groupId: t.groupId, private: t.private }); } },
    { label: wc.isAudioMuted() ? 'Unmute site' : 'Mute site', click: () => { wc.setAudioMuted(!wc.isAudioMuted()); scheduleState(); } },
  );
  const site = /^https?:/.test(displayUrl(wc)) ? siteOf(displayUrl(wc)) : null;
  if (site && !t.private) {
    const on = settings.privacy.forgetSites.includes(site);
    items.push({
      label: `Forget ${site} when closed`, type: 'checkbox', checked: on,
      click: () => {
        applySettings({ privacy: { forgetSites: on ? settings.privacy.forgetSites.filter((x) => x !== site) : [...settings.privacy.forgetSites, site] } });
        toast(on ? `${site} will be remembered` : `${site}'s cookies and data will be deleted when you close it`);
      },
    });
  }
  items.push(
    { type: 'separator' },
    { label: 'Close tab', click: () => closeTab(id) },
    { label: 'Close other tabs', enabled: order.length > 1, click: () => closeTabs(order.filter((x) => x !== id)) },
    { label: 'Close tabs to the right', click: () => closeTabs(order.slice(order.indexOf(id) + 1)) },
  );
  Menu.buildFromTemplate(items).popup({ window: win });
}

// ------------------------------------------------------------- tabs on disk
let tabsTimer = null;
function saveTabsSoon() {
  clearTimeout(tabsTimer);
  tabsTimer = setTimeout(saveTabsNow, 500);
}
function saveTabsNow() {
  if (!settings || EPHEMERAL) return;
  const data = {
    groups: [...groups.values()],
    tabs: order.filter((id) => !tabs.get(id).private).map((id) => {
      const t = tabs.get(id);
      const url = t.view.webContents.isDestroyed() ? '' : displayUrl(t.view.webContents);
      return { url, title: t.title, groupId: t.groupId, active: id === activeId };
    }),
  };
  try { fs.writeFileSync(sessionFile(), JSON.stringify(data)); } catch { /* ignore */ }
}
function restoreTabs() {
  if (!settings.restoreTabs || EPHEMERAL) return false;
  let data;
  try { data = JSON.parse(fs.readFileSync(sessionFile(), 'utf8')); } catch { return false; }
  if (!data.tabs || !data.tabs.length) return false;
  const idMap = new Map();
  for (const g of data.groups || []) {
    const id = nextGroupId++;
    idMap.set(g.id, id);
    groups.set(id, { id, name: String(g.name || ''), color: GROUP_COLORS.includes(g.color) ? g.color : 'grey', collapsed: !!g.collapsed });
  }
  let activate = null;
  for (const t of data.tabs) {
    const tab = createTab(t.url || null, { activate: false, groupId: idMap.get(t.groupId) || null, position: 'end', lazy: !t.active });
    if (t.groupId && idMap.has(t.groupId) && groups.get(idMap.get(t.groupId))) {
      groups.get(idMap.get(t.groupId)).collapsed = !!(data.groups.find((g) => g.id === t.groupId) || {}).collapsed;
    }
    tab.title = t.title || '';
    if (t.active) activate = tab.id;
  }
  cleanupGroups();
  activateTab(activate || order[0]);
  return true;
}

// ---------------------------------------------------------------------- input
function handleShortcut(event, input) {
  if (input.type !== 'keyDown') return;
  const mod = process.platform === 'darwin' ? input.meta : input.control;
  const k = input.key.toLowerCase();
  let handled = true;
  if (mod && !input.shift && k === 't') createTab(null, { position: 'end' });
  else if (mod && input.shift && k === 'n') createTab(null, { position: 'end', private: true });
  else if (mod && input.shift && k === 'p') openWindow('private');
  else if (input.alt && input.shift && k === 'n') openWindow('tor');
  else if (mod && k === 'w') closeTab(activeId);
  else if ((mod && k === 'l') || (input.alt && k === 'd') || k === 'f6') {
    ui.webContents.focus(); ui.webContents.send('focus-address');
  } else if ((mod && k === 'r') || k === 'f5') reload(input.shift || (mod && k === 'f5'));
  else if (mod && k === 'tab') cycleTab(input.shift ? -1 : 1);
  else if (mod && /^[1-8]$/.test(k)) { const id = visibleTabs()[Number(k) - 1]; if (id) activateTab(id); }
  else if (mod && k === '9') { const v = visibleTabs(); activateTab(v[v.length - 1]); }
  else if (input.alt && k === 'arrowleft') activeWc()?.navigationHistory.goBack();
  else if (input.alt && k === 'arrowright') activeWc()?.navigationHistory.goForward();
  else if (mod && (k === '=' || k === '+')) zoom(0.5);
  else if (mod && k === '-') zoom(-0.5);
  else if (mod && k === '0') zoom(0);
  else if (mod && k === 'f') { ui.webContents.focus(); ui.webContents.send('find-open'); }
  else if (mod && k === ',') openInternal(SETTINGS_URL);
  else if (mod && !input.shift && k === 'h') openInternal(HISTORY_URL);
  else if (mod && !input.shift && !input.alt && k === 'p') activeWc()?.print({});
  else if (mod && !input.shift && k === 'd') toggleBookmarkForActive();
  else if (mod && input.shift && k === 'b') setBookmarksBar(!settings.showBookmarksBar);
  else if (mod && input.shift && (k === 'delete' || k === 'backspace')) openInternal(SETTINGS_URL + '#data');
  else if (k === 'f12' || (mod && input.shift && k === 'i')) activeWc()?.toggleDevTools();
  else if (k === 'f11') setFullscreen(!fullscreen);
  else handled = false;
  if (handled) event.preventDefault();
}

function showContextMenu(wc, p) {
  const items = [];
  const sep = { type: 'separator' };
  const groupId = tabByWebContentsId(wc.id)?.groupId || null;
  const isPriv = !!tabByWebContentsId(wc.id)?.private;
  if (p.linkURL) {
    items.push(
      { label: isPriv ? 'Open link in new private tab' : 'Open link in new tab', click: () => createTab(p.linkURL, { activate: false, groupId, private: isPriv }) },
      ...(isPriv ? [] : [{ label: 'Open link in private tab', click: () => createTab(p.linkURL, { position: 'end', private: true }) }]),
      { label: 'Copy link', click: () => clipboard.writeText(p.linkURL) },
      sep,
    );
  }
  if (p.mediaType === 'image' && p.srcURL) {
    items.push(
      { label: 'Open image in new tab', click: () => createTab(p.srcURL, { activate: false, groupId }) },
      { label: 'Copy image', click: () => wc.copyImageAt(p.x, p.y) },
      { label: 'Save image as…', click: () => wc.downloadURL(p.srcURL) },
      sep,
    );
  }
  if (p.isEditable) {
    items.push({ role: 'undo' }, { role: 'redo' }, sep, { role: 'cut' }, { role: 'copy' },
      { role: 'paste' }, { role: 'selectAll' }, sep);
  } else if (p.selectionText) {
    const s = p.selectionText.trim();
    const short = s.length > 24 ? s.slice(0, 24) + '…' : s;
    items.push({ role: 'copy' }, { label: `Search for “${short}”`, click: () => createTab(searchUrl(s), { groupId }) }, sep);
  }
  const ctab = tabByWebContentsId(wc.id);
  if (ctab && /^https?:/.test(wc.getURL())) {
    const done = ctab.tr && (ctab.tr.state === 'done' || ctab.tr.state === 'working');
    items.push(done
      ? { label: 'Show original', click: () => showOriginal(ctab) }
      : { label: `Translate page to ${LANGUAGES[settings.languages.translateTo]}`, click: () => translateTab(ctab) }, sep);
  }
  items.push(
    ...extensionMenuItems(wc, p),
    { label: 'Back', enabled: wc.navigationHistory.canGoBack(), click: () => wc.navigationHistory.goBack() },
    { label: 'Forward', enabled: wc.navigationHistory.canGoForward(), click: () => wc.navigationHistory.goForward() },
    { label: 'Reload', click: () => wc.reload() },
    sep,
    { label: 'Inspect', click: () => wc.inspectElement(p.x, p.y) },
  );
  Menu.buildFromTemplate(items).popup({ window: win });
}

// ------------------------------------------------------------------ UI bridge
let stateTimer = null;
function scheduleState() {
  if (stateTimer) return;
  stateTimer = setTimeout(() => { stateTimer = null; pushState(); }, 40);
}

function pushState() {
  pushPanel();
  if (!ui || ui.webContents.isDestroyed()) return;
  ui.webContents.send('state', {
    activeId,
    shield: shieldState,
    tor: settings.tor.enabled ? tor.state : 'off',
    torButton: settings.tor.showButton || settings.tor.enabled || MODE === 'tor',
    windowMode: MODE,
    profile: { ...currentProfile(), many: loadProfiles().length > 1 },
    updateReady: update.state === 'available' && settings.updates.skip !== update.version,
    bookmarksBar: !!settings.showBookmarksBar,
    pinnedExtensions: settings.extensions.pinned || [],
    bookmarks: withIcons(bookmarks.items),
    bookmarked: !!(activeWc() && findBookmarkByUrl(displayUrl(activeWc()))),
    groups: [...groups.values()],
    tabs: order.map((id) => {
      const t = tabs.get(id);
      const wc = t.view.webContents;
      const url = displayUrl(wc);
      return {
        id,
        groupId: t.groupId,
        title: t.title || url || 'New tab',
        url,
        icon: settings.appearance.siteIcons !== false && t.icon && (() => { try { return new URL(url).hostname === t.iconHost; } catch { return false; } })() ? t.icon : null,
        loading: wc.isLoading(),
        canBack: wc.navigationHistory.canGoBack(),
        canForward: wc.navigationHistory.canGoForward(),
        blocked: t.blocked,
        allowlisted: /^https?:/.test(url) && isAllowlisted(url),
        private: !!t.private,
        asleep: !!t.discarded || pendingUrls.has(wc) || wc.getURL().startsWith(`${SCHEME}://sleep/`),
        muted: wc.isAudioMuted(),
        audible: wc.isCurrentlyAudible(),
        security: securityOf(url),
        translate: t.tr ? { state: t.tr.state, from: t.tr.from || '', fromName: LANGUAGES[t.tr.from] || LANGUAGES[(t.tr.from || '').split('-')[0]] || t.tr.from || '', to: LANGUAGES[settings.languages.translateTo], error: t.tr.error || '' } : null,
      };
    }),
  });
}

function toast(text) {
  if (ui && !ui.webContents.isDestroyed()) ui.webContents.send('toast', text);
}

function fromUi(handler) {
  return (event, ...args) => { if (ui && event.sender === ui.webContents) handler(...args); };
}

// --------------------------------------------------------------- memory saver
// Background tabs idle longer than the limit swap to a tiny placeholder page,
// freeing their memory. Opening the tab goes back to the page (from cache).
// Discarding ends the tab's renderer process (like Chrome's tab discarding).
// The tab keeps its title and history; opening it reloads the page.
function discardTab(t) {
  const wc = t.view.webContents;
  if (wc.isDestroyed() || t.discarded || t.id === activeId || pendingUrls.has(wc)) return false;
  if (wc.isLoading() || wc.isCurrentlyAudible() || wc.isDevToolsOpened()) return false;
  const pid = wc.getOSProcessId();
  if (!pid) return false;
  // Never kill a process that another page, popup or extension also uses.
  const shared = allWebContents.getAllWebContents().some((o) => o !== wc && !o.isDestroyed() && o.getOSProcessId() === pid);
  if (shared) return false;
  t.discarded = true;
  wc.forcefullyCrashRenderer();
  scheduleState();
  return true;
}

function sleepTab(t) {
  const wc = t.view.webContents;
  const url = wc.getURL();
  if (discardTab(t)) return;
  if (!/^https?:/.test(url) || wc.isLoading() || wc.isCurrentlyAudible() || wc.isDevToolsOpened()) return;
  const q = new URLSearchParams({ u: url, t: t.title || url });
  t.sleptAt = Date.now();
  wc.loadURL(`${SCHEME}://sleep/?${q}`);
}
function wakeTab(t) {
  const wc = t.view.webContents;
  const h = wc.navigationHistory;
  if (h.canGoBack()) {
    const sleepIndex = h.getActiveIndex();
    wc.once('did-finish-load', () => { try { h.removeEntryAtIndex(sleepIndex); } catch { /* older Electron */ } });
    h.goBack();
  } else {
    try { wc.loadURL(new URL(wc.getURL()).searchParams.get('u')); } catch { /* ignore */ }
  }
}
setInterval(() => {
  const mins = settings && settings.performance.sleepAfterMin;
  if (!mins) return;
  // When the PC is low on memory, sleep idle tabs much sooner.
  const os = require('os');
  const lowMemory = os.freemem() / os.totalmem() < 0.15;
  const cutoff = Date.now() - (lowMemory ? Math.min(mins, 5) : mins) * 60000;
  for (const t of tabs.values()) {
    if (t.id === activeId || t.lastActive > cutoff) continue;
    const wc = t.view.webContents;
    if (wc.isDestroyed() || t.discarded || pendingUrls.has(wc) || wc.getURL().startsWith(`${SCHEME}://sleep/`)) continue;
    if (wc.getURL().startsWith(`${SCHEME}://`)) discardTab(t); else sleepTab(t);
  }
  scheduleState();
}, 60000).unref();

// Memory per tab for Settings → Performance (task manager).
function taskList() {
  const metrics = new Map(app.getAppMetrics().map((m) => [m.pid, m]));
  const usedPids = new Map();
  for (const t of tabs.values()) {
    const wc = t.view.webContents;
    if (wc.isDestroyed()) continue;
    const pid = wc.getOSProcessId();
    usedPids.set(pid, (usedPids.get(pid) || 0) + 1);
  }
  const kb = (m) => (m && m.memory ? m.memory.workingSetSize : 0);
  const rows = [];
  let tabsKb = 0;
  for (const id of order) {
    const t = tabs.get(id);
    const wc = t.view.webContents;
    if (wc.isDestroyed()) continue;
    const pid = wc.getOSProcessId();
    const asleep = !!t.discarded || pendingUrls.has(wc) || wc.getURL().startsWith(`${SCHEME}://sleep/`);
    const shared = (usedPids.get(pid) || 1) > 1;
    const mem = t.discarded || pendingUrls.has(wc) ? 0 : kb(metrics.get(pid)) / (usedPids.get(pid) || 1);
    tabsKb += mem;
    rows.push({ id, title: t.title || displayUrl(wc) || 'New tab', url: displayUrl(wc), mb: Math.round(mem / 1024), asleep, shared, active: id === activeId });
  }
  let totalKb = 0;
  for (const m of metrics.values()) totalKb += kb(m);
  return { rows, totalMb: Math.round(totalKb / 1024), browserMb: Math.round((totalKb - tabsKb) / 1024) };
}

// -------------------------------------------------------- fingerprint shield
// Per-session secret: fingerprint noise is stable on a site, different across
// sites and after every restart.
const fpSecret = require('crypto').randomBytes(16);
function fpSeed(url) {
  const site = siteOf(url) || 'none';
  return require('crypto').createHmac('sha256', fpSecret).update(site).digest().readUInt32LE(0);
}

// ------------------------------------------------------- profiles (manager)
function profilesInfo() {
  return { current: PROFILE, list: loadProfiles(), colors: PROFILE_COLORS, canShortcut: process.platform === 'win32' };
}
function createProfile(name, color) {
  const list = loadProfiles();
  const base = String(name || '').trim().slice(0, 40) || 'Profile';
  let id = base.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 30) || 'profile';
  if (id === 'default') id = 'profile';
  let n = 2; const taken = new Set(list.map((p) => p.id)); const stem = id;
  while (taken.has(id)) id = `${stem}-${n++}`;
  list.push({ id, name: base, color: PROFILE_COLORS.includes(color) ? color : PROFILE_COLORS[list.length % PROFILE_COLORS.length] });
  saveProfiles(list);
  fs.mkdirSync(profileDir(id), { recursive: true });
  // Start the new profile with this profile's look and privacy choices, but none of its data.
  try {
    const seed = JSON.parse(JSON.stringify(settings));
    delete seed.stats; seed.extensions = { unpacked: [], hidden: [], pinned: null }; seed.adblock.allowlist = []; seed.privacy.forgetSites = [];
    fs.writeFileSync(path.join(profileDir(id), 'settings.json'), JSON.stringify(seed, null, 2));
  } catch { /* defaults then */ }
  return id;
}
function updateProfile(id, patch) {
  const list = loadProfiles();
  const p = list.find((x) => x.id === id);
  if (!p) return false;
  if (patch.name != null) p.name = String(patch.name).trim().slice(0, 40) || p.name;
  if (PROFILE_COLORS.includes(patch.color)) p.color = patch.color;
  saveProfiles(list);
  if (id === PROFILE) { scheduleState(); if (win) win.setTitle(windowTitle()); applyTaskbarIdentity(); }
  return true;
}
async function deleteProfile(id) {
  if (id === 'default' || id === PROFILE) return { ok: false, error: "This profile is open in this window. Open another profile to delete it." };
  // Ask that profile to close if it's running (same single-instance trick as the installer).
  await new Promise((resolve) => {
    require('child_process').execFile(process.execPath, [...devArgs(), ...profileArgs(id), '--goat-quit'], { timeout: 15000, windowsHide: true }, () => resolve());
  });
  await new Promise((r) => setTimeout(r, 800));
  try { fs.rmSync(profileDir(id), { recursive: true, force: true }); } catch (err) { return { ok: false, error: `Couldn't delete its files: ${err.message}` }; }
  saveProfiles(loadProfiles().filter((p) => p.id !== id));
  if (process.platform === 'win32') { try { fs.rmSync(path.join(app.getPath('desktop'), `Goat - ${profileName(id)}.lnk`), { force: true }); } catch { /* none */ } }
  scheduleState();
  return { ok: true };
}
function profileName(id) { const p = loadProfiles().find((x) => x.id === id); return p ? p.name : id; }
function profileShortcut(id) {
  if (process.platform !== 'win32') return false;
  const p = loadProfiles().find((x) => x.id === id);
  if (!p) return false;
  return shell.writeShortcutLink(path.join(app.getPath('desktop'), `Goat - ${p.name.replace(/[\\/:*?"<>|]/g, '')}.lnk`), 'create', {
    target: process.execPath, args: profileArgs(id).join(' '), icon: process.execPath, iconIndex: 0, description: `Goat (${p.name})`,
    appUserModelId: id === 'default' ? 'com.goat.browser' : `com.goat.browser.profile.${id}`,
  });
}
function profileMenu() {
  const list = loadProfiles();
  Menu.buildFromTemplate([
    ...list.map((p) => ({ label: p.name + (p.id === PROFILE ? '  (this window)' : ''), type: 'radio', checked: p.id === PROFILE, click: () => openProfile(p.id) })),
    { type: 'separator' },
    { label: 'Add profile…', click: () => openInternal(SETTINGS_URL + '#profiles') },
    { label: 'Manage profiles…', click: () => openInternal(SETTINGS_URL + '#profiles') },
  ]).popup({ window: win });
}
// Taskbar: separate button per profile, pinning reopens the same profile, and a
// small colored dot on the icon shows which profile a window belongs to.
function applyTaskbarIdentity() {
  if (!win || process.platform !== 'win32') return;
  const p = currentProfile();
  const many = loadProfiles().length > 1;
  try {
    win.setAppDetails({
      appId: APP_ID, appIconPath: process.execPath, appIconIndex: 0,
      relaunchCommand: `"${process.execPath}"${PROFILE === 'default' ? '' : ` --goat-profile=${PROFILE}`}`,
      relaunchDisplayName: PROFILE === 'default' ? 'Goat' : `Goat (${p.name})`,
    });
  } catch { /* older Windows */ }
  try { win.setOverlayIcon(many ? profileDot(p.color) : null, many ? `Profile: ${p.name}` : ''); } catch { /* ignore */ }
}
// 16x16 colored circle with a dark ring, drawn pixel by pixel (no image files needed).
function profileDot(hex) {
  const { nativeImage } = require('electron');
  const n = parseInt(hex.slice(1), 16); const R = n >> 16; const G = (n >> 8) & 255; const B = n & 255;
  const S = 16; const buf = Buffer.alloc(S * S * 4);
  for (let y = 0; y < S; y++) {
    for (let x = 0; x < S; x++) {
      const d = Math.hypot(x + 0.5 - S / 2, y + 0.5 - S / 2);
      const i = (y * S + x) * 4;
      if (d <= 7.5) {
        const ring = d > 5.8;
        buf[i] = ring ? 16 : B; buf[i + 1] = ring ? 18 : G; buf[i + 2] = ring ? 20 : R; // BGRA
        buf[i + 3] = d > 7 ? Math.round(255 * (7.5 - d) * 2) : 255;
      }
    }
  }
  return nativeImage.createFromBitmap(buf, { width: S, height: S });
}
function windowTitle() {
  const base = { normal: 'Goat', private: 'Goat · Private window', tor: 'Goat · Tor window' }[MODE];
  return PROFILE === 'default' && loadProfiles().length === 1 ? base : `${base} · ${currentProfile().name}`;
}

// ------------------------------------------------------------------- updates
// Checks GitHub Releases for a newer Goat. Windows: downloads the installer and runs it
// (it updates in place and keeps your profile). Linux: opens the download page.
const RELEASES_API = process.env.GOAT_UPDATE_URL || 'https://api.github.com/repos/anonymoustwanger/goat-browser/releases/latest';
const RELEASES_PAGE = 'https://github.com/anonymoustwanger/goat-browser/releases/latest';
const APP_VERSION = require(path.join(__dirname, '..', 'package.json')).version;
let update = { state: 'idle', version: '', notes: '', url: '', asset: null, progress: 0, error: '', checkedAt: 0 };
const newerThan = (a, b) => {
  const x = String(a).replace(/^v/i, '').split(/[.-]/).map((n) => parseInt(n, 10) || 0);
  const y = String(b).replace(/^v/i, '').split(/[.-]/).map((n) => parseInt(n, 10) || 0);
  for (let i = 0; i < 3; i++) { if ((x[i] || 0) !== (y[i] || 0)) return (x[i] || 0) > (y[i] || 0); }
  return false;
};
function pushUpdate() {
  scheduleState();
  for (const wc of allWebContents.getAllWebContents()) {
    if (!wc.isDestroyed() && wc.getURL().startsWith(SETTINGS_URL)) wc.send('goat:update', update);
  }
}
async function checkForUpdates(manual = false) {
  if (update.state === 'checking' || update.state === 'downloading') return update;
  update = { ...update, state: 'checking', error: '' };
  pushUpdate();
  try {
    const res = await ses.fetch(RELEASES_API, { headers: { Accept: 'application/vnd.github+json' }, cache: 'no-store', credentials: 'omit' });
    if (res.status === 404) throw new Error('no releases published yet');
    if (!res.ok) throw new Error(`GitHub answered ${res.status}`);
    const rel = await res.json();
    const version = String(rel.tag_name || rel.name || '').replace(/^v/i, '');
    const want = process.platform === 'win32' ? /GoatBrowserSetup.*\.exe$/i : /\.AppImage$/i;
    const asset = (rel.assets || []).find((a) => want.test(a.name)) || null;
    update.checkedAt = Date.now();
    settings.updates.lastCheck = update.checkedAt;
    saveSettings();
    if (version && newerThan(version, APP_VERSION)) {
      const isNew = update.version !== version;
      update = { ...update, state: 'available', version, notes: String(rel.body || '').slice(0, 2000), url: rel.html_url || RELEASES_PAGE,
        asset: asset ? { name: asset.name, url: asset.browser_download_url, size: asset.size } : null };
      if (isNew && !manual && settings.updates.skip !== version) toast(`Goat ${version} is available. Menu → Update Goat.`);
    } else update = { ...update, state: 'latest', version: version || APP_VERSION };
  } catch (err) {
    update = { ...update, state: 'error', error: String(err.message || err) };
  }
  pushUpdate();
  return update;
}
async function installUpdate() {
  if (update.state !== 'available' && update.state !== 'error') return update;
  if (process.platform !== 'win32' || !update.asset) { createTab(update.url || RELEASES_PAGE); return update; }
  const file = path.join(app.getPath('temp'), update.asset.name.replace(/[^\w.-]/g, '_'));
  update = { ...update, state: 'downloading', progress: 0, error: '' };
  pushUpdate();
  try {
    const res = await ses.fetch(update.asset.url, { credentials: 'omit' });
    if (!res.ok || !res.body) throw new Error(`download failed (${res.status})`);
    const total = Number(res.headers.get('content-length')) || update.asset.size || 0;
    const out = fs.createWriteStream(file);
    const reader = res.body.getReader();
    let got = 0; let last = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (!out.write(Buffer.from(value))) await new Promise((r) => out.once('drain', r));
      got += value.length;
      if (total && Date.now() - last > 250) { last = Date.now(); update.progress = Math.round((got / total) * 100); pushUpdate(); }
    }
    await new Promise((resolve, reject) => out.end((err) => (err ? reject(err) : resolve())));
    if (update.asset.size && fs.statSync(file).size !== update.asset.size) throw new Error('download was incomplete');
    update = { ...update, state: 'ready', progress: 100 };
    pushUpdate();
    // The installer closes Goat itself when you click Update, then updates in place.
    require('child_process').spawn(file, [], { detached: true, stdio: 'ignore' }).unref();
  } catch (err) {
    update = { ...update, state: 'error', error: String(err.message || err) };
    pushUpdate();
  }
  return update;
}
function scheduleUpdateChecks() {
  if (EPHEMERAL || PROFILE !== 'default') return; // one window checks, not every profile
  const due = () => settings.updates.auto && Date.now() - (settings.updates.lastCheck || 0) > 20 * 60 * 60 * 1000;
  setTimeout(() => { if (due()) checkForUpdates(false); }, 15000);
  setInterval(() => { if (due()) checkForUpdates(false); }, 60 * 60 * 1000).unref();
}

// ---------------------------------------------------------------- translation
const translator = new Translator(() => settings.languages);
// tab.tr = { state: 'offer' | 'working' | 'done' | 'error', from, error }
async function checkTranslate(tab) {
  const wc = tab.view.webContents;
  if (wc.isDestroyed() || !/^https?:/.test(wc.getURL()) || wc.isLoading()) return;
  if (tab.tr && tab.tr.state !== 'offer') return;
  const lang = await translator.pageLang(wc);
  const to = settings.languages.translateTo;
  if (!lang || sameLang(lang, to) || !settings.languages.offer) { if (tab.tr) { tab.tr = null; scheduleState(); } return; }
  tab.tr = { state: 'offer', from: lang };
  if (settings.languages.auto) translateTab(tab);
  else scheduleState();
}
async function translateTab(tab) {
  const wc = tab.view.webContents;
  if (wc.isDestroyed()) return;
  const url = wc.getURL();
  tab.tr = { ...(tab.tr || {}), state: 'working', error: '' };
  scheduleState();
  try {
    const r = await translator.translatePage(wc, settings.languages.translateTo);
    if (wc.isDestroyed() || wc.getURL() !== url) return;
    tab.tr = { state: 'done', from: (tab.tr && tab.tr.from) || r.src };
    if (r.src && !tab.tr.from) tab.tr.from = r.src;
    // Keep translating text the page adds later (infinite scroll, menus, chat).
    stopTranslateWatch(tab);
    tab.trTimer = setInterval(async () => {
      if (wc.isDestroyed() || !tab.tr || tab.tr.state !== 'done' || tab.trBusy) return;
      if (tab.id !== activeId) return;
      tab.trBusy = true;
      try { await translator.translatePage(wc, settings.languages.translateTo); } catch { /* try next time */ }
      tab.trBusy = false;
    }, 2500);
  } catch (err) {
    if (wc.isDestroyed()) return;
    tab.tr = { ...(tab.tr || {}), state: 'error', error: String(err.message || err) };
    toast(`Couldn't translate: ${tab.tr.error}`);
  }
  scheduleState();
}
function stopTranslateWatch(tab) { if (tab.trTimer) { clearInterval(tab.trTimer); tab.trTimer = null; } }
async function showOriginal(tab) {
  stopTranslateWatch(tab);
  await translator.restore(tab.view.webContents);
  tab.tr = tab.tr ? { state: 'offer', from: tab.tr.from } : null;
  scheduleState();
}
function translateMenu(id) {
  const t = tabs.get(id) || tabs.get(activeId);
  if (!t) return;
  const L = settings.languages;
  const done = t.tr && (t.tr.state === 'done' || t.tr.state === 'working');
  const langItems = Object.entries(LANGUAGES).map(([code, name]) => ({
    label: name, type: 'radio', checked: code === L.translateTo,
    click: async () => { if (done) await showOriginal(t); await applySettings({ languages: { translateTo: code } }); translateTab(t); },
  }));
  Menu.buildFromTemplate([
    done ? { label: 'Show original', click: () => showOriginal(t) } : { label: `Translate to ${LANGUAGES[L.translateTo]}`, click: () => translateTab(t) },
    { label: 'Translate to', submenu: langItems },
    { type: 'separator' },
    { label: 'Always translate other languages', type: 'checkbox', checked: L.auto, click: () => applySettings({ languages: { auto: !L.auto } }) },
    { label: 'Language settings…', click: () => openInternal(SETTINGS_URL + '#languages') },
  ]).popup({ window: win });
}

// -------------------------------------------------------------------- history
// Visits from normal tabs (never private ones), newest last, in history.json.
let history = [];
let historySeq = 0;
let historyTimer = null;
const historyFile = () => path.join(userDir(), 'history.json');
const HISTORY_MAX = 50000;

function loadHistory() {
  try {
    const data = JSON.parse(fs.readFileSync(historyFile(), 'utf8'));
    if (Array.isArray(data)) history = data.filter((h) => h && typeof h.url === 'string' && Number.isFinite(h.t));
  } catch { /* none yet */ }
  historySeq = history.reduce((m, h) => Math.max(m, h.id || 0), 0);
  pruneHistory(false);
}
function saveHistorySoon() {
  clearTimeout(historyTimer);
  historyTimer = setTimeout(saveHistoryNow, 1500);
}
function saveHistoryNow() {
  clearTimeout(historyTimer);
  if (EPHEMERAL) return;
  try {
    if (!history.length) fs.rmSync(historyFile(), { force: true });
    else fs.writeFileSync(historyFile(), JSON.stringify(history));
  } catch { /* ignore */ }
}
function pruneHistory(save) {
  const days = settings ? settings.history.keepDays : 90;
  const before = history.length;
  if (days === 0) history = [];
  else if (days > 0) { const cutoff = Date.now() - days * 86400000; history = history.filter((h) => h.t >= cutoff); }
  if (history.length > HISTORY_MAX) history = history.slice(history.length - HISTORY_MAX);
  if (save || history.length !== before) saveHistorySoon();
}
setInterval(() => pruneHistory(false), 6 * 3600000).unref();

function recordVisit(tab, url, inPage = false) {
  if (tab.private || !settings || settings.history.keepDays === 0) return;
  if (!/^https?:\/\//.test(url)) return;
  const now = Date.now();
  const last = history[history.length - 1];
  // Same page again within a minute (reloads, in-page hash changes): just refresh the time.
  if (last && last.url === url && now - last.t < 60000) { last.t = now; tab.histId = last.id; saveHistorySoon(); return; }
  const entry = { id: ++historySeq, url, title: inPage ? (tab.title || '') : '', t: now };
  history.push(entry);
  tab.histId = entry.id;
  if (history.length > HISTORY_MAX + 500) pruneHistory(false);
  saveHistorySoon();
}
function updateHistoryTitle(tab, title) {
  if (!tab.histId || tab.private) return;
  for (let i = history.length - 1; i >= Math.max(0, history.length - 200); i--) {
    if (history[i].id === tab.histId) { history[i].title = String(title || '').slice(0, 300); saveHistorySoon(); return; }
  }
}
function clearHistory(range) {
  const spans = { hour: 3600000, day: 86400000, week: 7 * 86400000, month: 30 * 86400000 };
  if (range === 'all' || !spans[range]) history = [];
  else { const cutoff = Date.now() - spans[range]; history = history.filter((h) => h.t < cutoff); }
  saveHistoryNow();
}
function queryHistory({ q = '', before = Infinity, limit = 200 } = {}) {
  const words = String(q).toLowerCase().split(/\s+/).filter(Boolean);
  const out = [];
  for (let i = history.length - 1; i >= 0 && out.length < Math.min(1000, limit); i--) {
    const h = history[i];
    if (h.t >= before) continue;
    if (words.length) {
      const hay = (h.title + ' ' + h.url).toLowerCase();
      if (!words.every((w) => hay.includes(w))) continue;
    }
    out.push(h);
  }
  return { items: out, total: history.length, more: out.length >= limit };
}

// ------------------------------------------------------------------ bookmarks
// { items: [ { id, type: 'link', title, url } | { id, type: 'folder', title, children: [links] } ] }
let bookmarks = { items: [] };
const bookmarksFile = () => path.join(userDir(), 'bookmarks.json');
let bmTimer = null;

function loadBookmarks() {
  try {
    const data = JSON.parse(fs.readFileSync(bookmarksFile(), 'utf8'));
    if (Array.isArray(data.items)) bookmarks = { items: data.items.filter(validBookmark) };
  } catch { /* none yet */ }
}
function validBookmark(b) {
  if (!b || typeof b.id !== 'string') return false;
  if (b.type === 'folder') { b.children = (b.children || []).filter((c) => c && c.type === 'link' && validBookmark(c)); return true; }
  return b.type === 'link' && typeof b.url === 'string';
}
function saveBookmarks() {
  clearTimeout(bmTimer);
  bmTimer = setTimeout(() => {
    if (EPHEMERAL) return;
    try { fs.writeFileSync(bookmarksFile(), JSON.stringify(bookmarks, null, 2)); } catch { /* ignore */ }
  }, 200);
  scheduleState();
  // Refresh the bookmarks on any open new-tab pages.
  for (const t of tabs.values()) {
    const wc = t.view.webContents;
    if (!wc.isDestroyed() && wc.getURL().startsWith(NEWTAB_URL)) wc.send('goat:bookmarks', withIcons(bookmarks.items));
  }
}
const newBmId = () => 'b' + Date.now().toString(36) + Math.random().toString(36).slice(2, 6);

function walkBookmarks(fn) {
  for (const b of bookmarks.items) {
    if (fn(b, bookmarks.items) === false) return;
    if (b.type === 'folder') for (const c of b.children) if (fn(c, b.children) === false) return;
  }
}
function findBookmark(id) {
  let hit = null;
  walkBookmarks((b, list) => { if (b.id === id) { hit = { b, list }; return false; } return true; });
  return hit;
}
function findBookmarkByUrl(url) {
  if (!url) return null;
  let hit = null;
  walkBookmarks((b, list) => { if (b.type === 'link' && b.url === url) { hit = { b, list }; return false; } return true; });
  return hit;
}

function addBookmark(url, title, beforeId = null, folderId = null) {
  if (!/^(https?|goat|chrome-extension|file):/i.test(url || '')) return null;
  const item = { id: newBmId(), type: 'link', title: String(title || url).slice(0, 120), url };
  const folder = folderId ? findBookmark(folderId) : null;
  const list = folder && folder.b.type === 'folder' ? folder.b.children : bookmarks.items;
  const i = beforeId ? list.findIndex((x) => x.id === beforeId) : -1;
  list.splice(i >= 0 ? i : list.length, 0, item);
  saveBookmarks();
  return item;
}

function removeBookmark(id) {
  const hit = findBookmark(id);
  if (!hit) return;
  hit.list.splice(hit.list.indexOf(hit.b), 1);
  saveBookmarks();
}

function moveBookmark(id, beforeId, folderId) {
  const hit = findBookmark(id);
  if (!hit || id === beforeId || id === folderId) return;
  if (hit.b.type === 'folder' && folderId) return; // one level of folders
  hit.list.splice(hit.list.indexOf(hit.b), 1);
  const folder = folderId ? findBookmark(folderId) : null;
  const list = folder && folder.b.type === 'folder' ? folder.b.children : bookmarks.items;
  const i = beforeId ? list.findIndex((x) => x.id === beforeId) : -1;
  list.splice(i >= 0 ? i : list.length, 0, hit.b);
  saveBookmarks();
}

function toggleBookmarkForActive() {
  const t = tabs.get(activeId);
  if (!t) return;
  const url = displayUrl(t.view.webContents);
  const hit = findBookmarkByUrl(url);
  if (hit) { removeBookmark(hit.b.id); toast('Bookmark removed'); return; }
  const item = addBookmark(url, t.title || url);
  if (!item) { toast("This page can't be bookmarked"); return; }
  if (!settings.showBookmarksBar) setBookmarksBar(true);
  toast('Bookmarked. Right-click it on the bar to rename.');
}

function setBookmarksBar(show) {
  applySettings({ showBookmarksBar: !!show });
}

function openBookmarkUrl(url, how) {
  if (how === 'new' || how === 'background') {
    const g = tabs.get(activeId)?.groupId || null;
    createTab(url, { activate: how === 'new', groupId: null, position: 'end', private: !!tabs.get(activeId)?.private });
    return g;
  }
  activeWc()?.loadURL(url);
  activeWc()?.focus();
  return null;
}

function bookmarkFolderMenu(folderId, x, y) {
  const hit = findBookmark(folderId);
  if (!hit || hit.b.type !== 'folder') return;
  const kids = hit.b.children;
  const items = kids.length ? kids.map((c) => ({ label: c.title || c.url, click: () => openBookmarkUrl(c.url, 'current') })) : [{ label: '(empty)', enabled: false }];
  if (kids.length) {
    items.push({ type: 'separator' }, { label: `Open all (${kids.length}) in new tabs`, click: () => kids.forEach((c) => createTab(c.url, { activate: false, position: 'end' })) });
  }
  popupAt(Menu.buildFromTemplate(items), x, y);
}

function bookmarkOverflowMenu(ids, x, y) {
  const items = [];
  for (const id of ids) {
    const hit = findBookmark(id);
    if (!hit) continue;
    const b = hit.b;
    if (b.type === 'folder') {
      items.push({ label: b.title || 'Folder', submenu: b.children.length ? b.children.map((c) => ({ label: c.title || c.url, click: () => openBookmarkUrl(c.url, 'current') })) : [{ label: '(empty)', enabled: false }] });
    } else items.push({ label: b.title || b.url, click: () => openBookmarkUrl(b.url, 'current') });
  }
  if (items.length) popupAt(Menu.buildFromTemplate(items), x, y);
}

function bookmarkItemMenu(id, x, y) {
  const hit = findBookmark(id);
  if (!hit) return;
  const b = hit.b;
  const items = [];
  if (b.type === 'link') {
    items.push(
      { label: 'Open', click: () => openBookmarkUrl(b.url, 'current') },
      { label: 'Open in new tab', click: () => openBookmarkUrl(b.url, 'background') },
      { label: 'Copy link', click: () => clipboard.writeText(b.url) },
      { type: 'separator' },
    );
    const folders = bookmarks.items.filter((f) => f.type === 'folder' && !f.children.includes(b));
    if (folders.length) items.push({ label: 'Move to folder', submenu: folders.map((f) => ({ label: f.title || 'Folder', click: () => moveBookmark(b.id, null, f.id) })) });
    if (hit.list !== bookmarks.items) items.push({ label: 'Move to bar', click: () => moveBookmark(b.id, null, null) });
  } else {
    items.push({ label: `Open all (${b.children.length}) in new tabs`, enabled: b.children.length > 0, click: () => b.children.forEach((c) => createTab(c.url, { activate: false, position: 'end' })) }, { type: 'separator' });
  }
  items.push(
    { label: 'Rename…', click: () => ui.webContents.send('bm-rename-start', b.id) },
    { label: b.type === 'folder' ? 'Delete folder' : 'Delete', click: () => removeBookmark(b.id) },
    { type: 'separator' },
    ...barMenuItems(),
  );
  popupAt(Menu.buildFromTemplate(items), x, y);
}

function barMenuItems() {
  return [
    { label: 'Bookmark this page', accelerator: 'CmdOrCtrl+D', click: () => toggleBookmarkForActive() },
    { label: 'New folder', click: () => { const f = { id: newBmId(), type: 'folder', title: 'New folder', children: [] }; bookmarks.items.push(f); saveBookmarks(); setTimeout(() => ui.webContents.send('bm-rename-start', f.id), 80); } },
    { type: 'separator' },
    { label: 'Show bookmarks bar', type: 'checkbox', checked: !!settings.showBookmarksBar, accelerator: 'CmdOrCtrl+Shift+B', click: () => setBookmarksBar(!settings.showBookmarksBar) },
  ];
}

function popupAt(menu, x, y) {
  const opts = { window: win };
  if (Number.isFinite(x) && Number.isFinite(y)) { opts.x = Math.round(x); opts.y = Math.round(y); }
  menu.popup(opts);
}

function wireBookmarksIpc() {
  ipcMain.on('bm-open', fromUi((id, how) => {
    const hit = findBookmark(id);
    if (hit && hit.b.type === 'link') openBookmarkUrl(hit.b.url, how);
  }));
  ipcMain.on('bm-star', fromUi(() => toggleBookmarkForActive()));
  ipcMain.on('bm-menu', fromUi((id, x, y) => bookmarkItemMenu(id, x, y)));
  ipcMain.on('bm-bar-menu', fromUi((x, y) => popupAt(Menu.buildFromTemplate(barMenuItems()), x, y)));
  ipcMain.on('bm-folder', fromUi((id, x, y) => bookmarkFolderMenu(id, x, y)));
  ipcMain.on('bm-overflow', fromUi((ids, x, y) => bookmarkOverflowMenu(Array.isArray(ids) ? ids : [], x, y)));
  ipcMain.on('bm-move', fromUi((id, beforeId, folderId) => moveBookmark(id, beforeId, folderId)));
  ipcMain.on('bm-rename', fromUi((id, title) => {
    const hit = findBookmark(id);
    if (hit && String(title || '').trim()) { hit.b.title = String(title).trim().slice(0, 120); saveBookmarks(); }
  }));
  ipcMain.on('bm-add-tab', fromUi((tabId, beforeId, folderId) => {
    const t = tabs.get(tabId);
    if (!t) return;
    const url = displayUrl(t.view.webContents);
    if (!addBookmark(url, t.title || url, beforeId, folderId)) toast("This page can't be bookmarked");
  }));
}

// ------------------------------------------------------------ main menu (cog)
const APPMENU_W = 340;
let appMenu = null;
let appMenuOpen = false;
let appMenuHiddenAt = 0;
let appMenuHeight = 460;
let appMenuAnchor = null;

function ensureAppMenu() {
  if (appMenu && !appMenu.webContents.isDestroyed()) return appMenu;
  appMenu = new WebContentsView({
    webPreferences: {
      preload: path.join(__dirname, 'panel', 'menu-preload.js'),
      contextIsolation: true, sandbox: true, nodeIntegration: false, spellcheck: false,
    },
  });
  appMenu.setBackgroundColor('#00000000');
  appMenu.setVisible(false);
  const wc = appMenu.webContents;
  wc.setWindowOpenHandler(() => ({ action: 'deny' }));
  wc.on('will-navigate', (e) => e.preventDefault());
  wc.on('blur', () => hideAppMenu());
  wc.on('before-input-event', (e, input) => {
    if (input.type === 'keyDown' && input.key === 'Escape') { e.preventDefault(); hideAppMenu(); activeWc()?.focus(); }
  });
  wc.on('did-finish-load', () => { wc.send('appearance', appearance()); pushAppMenu(); });
  wc.loadFile(path.join(__dirname, 'panel', 'menu.html'));
  win.contentView.addChildView(appMenu);
  return appMenu;
}
function placeAppMenu() {
  if (!appMenu || !win || !appMenuAnchor) return;
  const { width, height } = win.getContentBounds();
  const right = appMenuAnchor.x + appMenuAnchor.width;
  const x = Math.max(8, Math.min(width - APPMENU_W - 4, Math.round(right - APPMENU_W + 12)));
  const y = uiHeight() - 8;
  appMenu.setBounds({ x, y, width: APPMENU_W, height: Math.min(appMenuHeight, Math.max(160, height - y - 4)) });
}
function toggleAppMenu(rect) {
  if (appMenuOpen) { hideAppMenu(); return; }
  if (Date.now() - appMenuHiddenAt < 300) return;
  hidePanel();
  hideExtMenu();
  appMenuAnchor = rect && Number.isFinite(rect.x) ? rect : { x: 0, y: 0, width: 0, height: 0 };
  const v = ensureAppMenu();
  win.contentView.addChildView(v);
  placeAppMenu();
  v.setVisible(true);
  appMenuOpen = true;
  pushAppMenu();
  v.webContents.focus();
  v.webContents.send('panel-open');
}
function hideAppMenu() {
  if (!appMenu || !appMenuOpen) return;
  appMenuOpen = false;
  appMenuHiddenAt = Date.now();
  appMenu.setVisible(false);
}
function pushAppMenu() {
  if (!appMenu || appMenu.webContents.isDestroyed()) return;
  const wc = activeWc();
  const url = wc ? displayUrl(wc) : '';
  const level = wc ? wc.getZoomLevel() : 0;
  appMenu.webContents.send('menu-state', {
    zoom: Math.round(Math.pow(1.2, level) * 100),
    bookmarksBar: !!settings.showBookmarksBar,
    isWeb: /^https?:/.test(url),
    fullscreen,
    version: require(path.join(__dirname, "..", "package.json")).version,
    profile: currentProfile().name,
    update: update.state === 'available' ? update.version : '',
  });
}
function wireAppMenuIpc() {
  const fromMenu = (fn) => (event, ...args) => { if (appMenu && event.sender === appMenu.webContents) return fn(...args); return undefined; };
  ipcMain.on('app-menu', fromUi((rect) => toggleAppMenu(rect)));
  ipcMain.on('appmenu:size', fromMenu((h) => { appMenuHeight = Math.max(160, Math.min(720, Math.ceil(Number(h) || 460))); placeAppMenu(); }));
  ipcMain.on('appmenu:close', fromMenu(() => { hideAppMenu(); activeWc()?.focus(); }));
  ipcMain.on('appmenu:do', fromMenu((action) => {
    const keepOpen = ['zoomin', 'zoomout', 'zoomreset'].includes(action);
    if (!keepOpen) hideAppMenu();
    const wc = activeWc();
    switch (action) {
      case 'newtab': createTab(null, { position: 'end' }); break;
      case 'private': createTab(null, { position: 'end', private: true }); break;
      case 'privatewindow': openWindow('private'); break;
      case 'profiles': profileMenu(); break;
      case 'update': openInternal(SETTINGS_URL + '#about'); break;
      case 'torwindow': openWindow('tor'); break;
      case 'history': openInternal(HISTORY_URL); break;
      case 'bookmarksbar': setBookmarksBar(!settings.showBookmarksBar); break;
      case 'bookmark': toggleBookmarkForActive(); break;
      case 'find': ui.webContents.focus(); ui.webContents.send('find-open'); break;
      case 'print': wc?.print({}); break;
      case 'zoomin': zoom(0.5); break;
      case 'zoomout': zoom(-0.5); break;
      case 'zoomreset': zoom(0); break;
      case 'fullscreen': setFullscreen(!fullscreen); break;
      case 'downloads': shell.openPath(app.getPath('downloads')); break;
      case 'extensions': openInternal(SETTINGS_URL + '#extensions'); break;
      case 'cleardata': openInternal(SETTINGS_URL + '#data'); break;
      case 'adblock': openInternal(SETTINGS_URL + '#adblock'); break;
      case 'settings': openInternal(SETTINGS_URL); break;
      case 'about': openInternal(SETTINGS_URL + '#about'); break;
      case 'devtools': wc?.toggleDevTools(); break;
      case 'quit': win?.close(); break;
      default: break;
    }
    if (keepOpen) setTimeout(pushAppMenu, 30);
  }));
}

// ---------------------------------------------------------- extensions menu
const EXTMENU_W = 320;
let extMenu = null;
let extMenuOpen = false;
let extMenuHiddenAt = 0;
let extMenuHeight = 300;
let extMenuAnchor = null; // puzzle button rect in window coordinates

function ensureExtMenu() {
  if (extMenu && !extMenu.webContents.isDestroyed()) return extMenu;
  extMenu = new WebContentsView({
    webPreferences: {
      preload: path.join(__dirname, 'panel', 'ext-preload.js'),
      contextIsolation: true, sandbox: true, nodeIntegration: false, spellcheck: false,
    },
  });
  extMenu.setBackgroundColor('#00000000');
  extMenu.setVisible(false);
  const wc = extMenu.webContents;
  wc.setWindowOpenHandler(() => ({ action: 'deny' }));
  wc.on('will-navigate', (e) => e.preventDefault());
  wc.on('blur', () => hideExtMenu());
  wc.on('before-input-event', (e, input) => {
    if (input.type === 'keyDown' && input.key === 'Escape') { e.preventDefault(); hideExtMenu(); activeWc()?.focus(); }
  });
  wc.on('did-finish-load', () => { wc.send('appearance', appearance()); pushExtMenu(); });
  wc.loadFile(path.join(__dirname, 'panel', 'extensions.html'));
  win.contentView.addChildView(extMenu);
  return extMenu;
}
function placeExtMenu() {
  if (!extMenu || !win || !extMenuAnchor) return;
  const { width } = win.getContentBounds();
  const right = extMenuAnchor.x + extMenuAnchor.width;
  const x = Math.max(8, Math.min(width - EXTMENU_W - 4, Math.round(right - EXTMENU_W + 12)));
  extMenu.setBounds({ x, y: uiHeight() - 8, width: EXTMENU_W, height: extMenuHeight });
}
function toggleExtMenu(rect) {
  if (extMenuOpen) { hideExtMenu(); return; }
  if (Date.now() - extMenuHiddenAt < 300) return;
  hidePanel();
  hideAppMenu();
  extMenuAnchor = rect && Number.isFinite(rect.x) ? rect : { x: 0, y: 0, width: 0, height: 0 };
  const v = ensureExtMenu();
  win.contentView.addChildView(v);
  placeExtMenu();
  v.setVisible(true);
  extMenuOpen = true;
  pushExtMenu();
  v.webContents.focus();
  v.webContents.send('panel-open');
}
function hideExtMenu() {
  if (!extMenu || !extMenuOpen) return;
  extMenuOpen = false;
  extMenuHiddenAt = Date.now();
  extMenu.setVisible(false);
}
function pushExtMenu() {
  if (!extMenu || extMenu.webContents.isDestroyed()) return;
  let list = [];
  try { list = extensions ? listExtensions() : []; } catch { /* not ready */ }
  extMenu.webContents.send('ext-state', { list, ready: !!extensions });
}
function setExtensionPinned(id, pinned) {
  const set = new Set(settings.extensions.pinned || []);
  if (pinned) set.add(String(id)); else set.delete(String(id));
  settings.extensions.pinned = [...set];
  saveSettings();
  scheduleState();
  pushExtMenu();
  for (const t of tabs.values()) {
    const wc = t.view.webContents;
    if (!wc.isDestroyed() && wc.getURL().startsWith(SETTINGS_URL)) wc.send('goat:extensions-changed');
  }
}

function wireExtMenuIpc() {
  const fromMenu = (fn) => (event, ...args) => { if (extMenu && event.sender === extMenu.webContents) return fn(...args); return undefined; };
  ipcMain.on('ext-menu', fromUi((rect) => toggleExtMenu(rect)));
  ipcMain.on('extmenu:size', fromMenu((h) => { extMenuHeight = Math.max(120, Math.min(560, Math.ceil(Number(h) || 300))); placeExtMenu(); }));
  ipcMain.on('extmenu:close', fromMenu(() => { hideExtMenu(); activeWc()?.focus(); }));
  ipcMain.on('extmenu:pin', fromMenu((id, pinned) => setExtensionPinned(id, pinned)));
  ipcMain.on('extmenu:open', fromMenu((id) => {
    hideExtMenu();
    const wc = activeWc();
    // The toolbar's preload asks the extension library to open the popup (or fire onClicked).
    ui.webContents.send('ext-activate', String(id), wc ? wc.id : -1, extMenuAnchor);
  }));
  ipcMain.on('extmenu:options', fromMenu((id) => {
    hideExtMenu();
    const e = extApi().getExtension(String(id));
    const m = (e && e.manifest) || {};
    const page = (m.options_ui && m.options_ui.page) || m.options_page;
    if (page) createTab(`chrome-extension://${e.id}/${String(page).replace(/^\//, '')}`);
  }));
  ipcMain.on('extmenu:manage', fromMenu(() => { hideExtMenu(); openInternal(SETTINGS_URL + '#extensions'); }));
  ipcMain.on('extmenu:store', fromMenu(() => { hideExtMenu(); createTab('https://chromewebstore.google.com/category/extensions'); }));
}

// ------------------------------------------------------- ad blocker panel
const PANEL_W = 344;
let panelHeight = 420;

function ensurePanel() {
  if (panel && !panel.webContents.isDestroyed()) return panel;
  panel = new WebContentsView({
    webPreferences: {
      preload: path.join(__dirname, 'panel', 'preload.js'),
      contextIsolation: true, sandbox: true, nodeIntegration: false, spellcheck: false,
    },
  });
  panel.setBackgroundColor('#00000000');
  panel.setVisible(false);
  const wc = panel.webContents;
  wc.setWindowOpenHandler(() => ({ action: 'deny' }));
  wc.on('will-navigate', (e) => e.preventDefault());
  wc.on('blur', () => hidePanel());
  wc.on('before-input-event', (e, input) => {
    if (input.type === 'keyDown' && input.key === 'Escape') { e.preventDefault(); hidePanel(); activeWc()?.focus(); }
  });
  wc.on('did-finish-load', () => { wc.send('appearance', appearance()); pushPanel(true); });
  wc.loadFile(path.join(__dirname, 'panel', 'adblock.html'));
  win.contentView.addChildView(panel);
  return panel;
}

let panelAnchor = 0;
function placePanel() {
  if (!panel || !win) return;
  const { width } = win.getContentBounds();
  const x = Math.max(8, Math.min(width - PANEL_W - 4, Math.round(panelAnchor - PANEL_W + 12)));
  panel.setBounds({ x, y: uiHeight() - 8, width: PANEL_W, height: panelHeight });
}

function togglePanel(anchorRight) {
  if (panelOpen) { hidePanel(); return; }
  if (Date.now() - panelHiddenAt < 300) return; // the click that just closed it
  panelAnchor = Number(anchorRight) || 0;
  const p = ensurePanel();
  win.contentView.addChildView(p); // bring to front
  placePanel();
  p.setVisible(true);
  panelOpen = true;
  pushPanel(true);
  p.webContents.focus();
  p.webContents.send('panel-open');
}

function hidePanel() {
  if (!panel || !panelOpen) return;
  panelOpen = false;
  panelHiddenAt = Date.now();
  panel.setVisible(false);
}

function panelState() {
  const t = tabs.get(activeId);
  const wc = t ? t.view.webContents : null;
  const url = wc && !wc.isDestroyed() ? wc.getURL() : '';
  const site = /^https?:/.test(url) ? siteOf(url) : null;
  return {
    site,
    blocked: t ? t.blocked : 0,
    total: settings.stats.blocked || 0,
    enabled: settings.adblock.enabled,
    allowed: !!site && settings.adblock.allowlist.includes(site),
    level: settings.adblock.level,
    state: shieldState,
    allowlistCount: settings.adblock.allowlist.length,
    rulesCount: settings.adblock.customRules.split(/\r?\n/).filter((l) => l.trim() && !l.trim().startsWith('!')).length,
  };
}

let panelTimer = null;
function pushPanel(now) {
  if (!panel || !panelOpen || panel.webContents.isDestroyed()) return;
  if (now) { panel.webContents.send('panel-state', panelState()); return; }
  if (panelTimer) return;
  panelTimer = setTimeout(() => { panelTimer = null; if (panelOpen) panel.webContents.send('panel-state', panelState()); }, 120);
}

function wirePanelIpc() {
  const fromPanel = (fn) => (event, ...args) => { if (panel && event.sender === panel.webContents) return fn(...args); return undefined; };
  const reloadActiveWeb = () => { const wc = activeWc(); if (wc && /^https?:/.test(wc.getURL())) wc.reload(); };
  ipcMain.on('panel:size', fromPanel((h) => { panelHeight = Math.max(120, Math.min(640, Math.ceil(Number(h) || 420))); placePanel(); }));
  ipcMain.on('panel:close', fromPanel(() => { hidePanel(); activeWc()?.focus(); }));
  ipcMain.on('panel:settings', fromPanel((hash) => { hidePanel(); openInternal(SETTINGS_URL + (hash === 'filters' ? '#adblock' : '#adblock')); }));
  ipcMain.handle('panel:set', fromPanel(async (patch) => {
    await applySettings({ adblock: patch });
    pushPanel(true);
    await blockerPromise;
    reloadActiveWeb();
    pushPanel(true);
    return panelState();
  }));
  ipcMain.handle('panel:toggle-site', fromPanel(async () => {
    const st = panelState();
    if (!st.site) return st;
    const list = settings.adblock.allowlist;
    const next = list.includes(st.site) ? list.filter((s) => s !== st.site) : [...list, st.site];
    await applySettings({ adblock: { allowlist: next } });
    reloadActiveWeb();
    pushPanel(true);
    return panelState();
  }));
}

function showShieldMenu(x, y) {
  const wc = activeWc();
  const t = tabs.get(activeId);
  const url = wc ? wc.getURL() : '';
  const site = /^https?:/.test(url) ? siteOf(url) : null;
  const on = settings.adblock.enabled;
  const allowed = !!site && settings.adblock.allowlist.includes(site);
  const level = settings.adblock.level;
  const reloadIfWeb = () => { if (site && wc && !wc.isDestroyed()) wc.reload(); };
  let header = 'Ad blocker';
  if (!on) header = 'Ad blocker is off';
  else if (shieldState === 'error') header = "Ad blocker couldn't load its lists";
  else if (allowed) header = `Paused on ${site}`;
  else if (site) header = `${t ? t.blocked : 0} ads and trackers blocked on this page`;
  const LEVELS = [
    ['ads', 'Ads only'],
    ['standard', 'Standard (ads + trackers)'],
    ['strict', 'Strict (+ cookie banners, popups)'],
  ];
  const items = [
    { label: header, enabled: false },
    { type: 'separator' },
    {
      label: 'Ad blocker on', type: 'checkbox', checked: on,
      click: async () => { await applySettings({ adblock: { enabled: !on } }); await blockerPromise; reloadIfWeb(); },
    },
  ];
  if (site) {
    items.push({ label: `Block on ${site}`, type: 'checkbox', checked: !allowed, enabled: on, click: () => toggleSiteShield() });
  }
  items.push(
    { type: 'separator' },
    { label: 'Blocking level', enabled: false },
    ...LEVELS.map(([value, label]) => ({
      label, type: 'radio', checked: level === value, enabled: on,
      click: async () => {
        if (settings.adblock.level === value) return;
        await applySettings({ adblock: { level: value } });
        toast(`Blocking level: ${label}`);
        await blockerPromise;
        reloadIfWeb();
      },
    })),
    { type: 'separator' },
    { label: 'Ad blocker settings…', click: () => openInternal(SETTINGS_URL + '#adblock') },
  );
  const menu = Menu.buildFromTemplate(items);
  const opts = { window: win };
  if (Number.isFinite(x) && Number.isFinite(y)) { opts.x = Math.round(x); opts.y = Math.round(y); }
  menu.popup(opts);
}

function toggleSiteShield() {
  const wc = activeWc();
  const site = wc && /^https?:/.test(wc.getURL()) ? siteOf(wc.getURL()) : null;
  if (!site) return;
  const list = settings.adblock.allowlist;
  const next = list.includes(site) ? list.filter((s) => s !== site) : [...list, site];
  applySettings({ adblock: { allowlist: next } }).then(() => {
    toast(next.includes(site) ? `Ad blocker paused on ${site}` : `Ad blocker on for ${site}`);
    wc.reload();
  });
}

function wireUiIpc() {
  ipcMain.on('navigate', fromUi((text) => navigate(text)));
  ipcMain.on('new-tab', fromUi(() => createTab(null, { position: 'end' })));
  ipcMain.on('new-tab-menu', fromUi((x, y) => popupAt(Menu.buildFromTemplate([
    { label: 'New tab', accelerator: 'CmdOrCtrl+T', click: () => createTab(null, { position: 'end' }) },
    { label: 'New private tab', accelerator: 'CmdOrCtrl+Shift+N', click: () => createTab(null, { position: 'end', private: true }) },
  ]), x, y)));
  ipcMain.on('close-tab', fromUi((id) => closeTab(id)));
  ipcMain.on('activate', fromUi((id) => activateTab(id)));
  ipcMain.on('back', fromUi(() => activeWc()?.navigationHistory.goBack()));
  ipcMain.on('forward', fromUi(() => activeWc()?.navigationHistory.goForward()));
  ipcMain.on('reload', fromUi(() => reload(false)));
  ipcMain.on('stop', fromUi(() => activeWc()?.stop()));
  ipcMain.on('settings', fromUi(() => openInternal(SETTINGS_URL)));
  ipcMain.on('tor-settings', fromUi(() => openInternal(SETTINGS_URL + '#tor')));
  ipcMain.on('profile-menu', fromUi(() => profileMenu()));
  ipcMain.on('update-open', fromUi(() => openInternal(SETTINGS_URL + '#about')));
  ipcMain.on('tor-toggle', fromUi(() => {
    if (MODE === 'tor') { toast('This is a Tor window: it always uses Tor.'); return; }
    const on = !settings.tor.enabled;
    applySettings({ tor: { enabled: on } });
    toast(on ? 'Tor on: connecting…' : 'Tor off: connecting directly');
  }));
  ipcMain.on('tor-menu', fromUi(() => {
    Menu.buildFromTemplate([
      { label: settings.tor.enabled ? 'Turn Tor off' : 'Turn Tor on', enabled: MODE !== 'tor', click: () => applySettings({ tor: { enabled: !settings.tor.enabled } }) },
      { label: 'New circuit', enabled: tor.state === 'on', click: async () => { if (await tor.newCircuit().catch(() => false)) { await Promise.allSettled(allSessions().map((x) => x.closeAllConnections())); toast('New Tor circuit'); } } },
      { type: 'separator' },
      { label: 'Show this button', type: 'checkbox', checked: settings.tor.showButton, click: () => applySettings({ tor: { showButton: !settings.tor.showButton } }) },
      { label: 'Tor settings…', click: () => openInternal(SETTINGS_URL + '#tor') },
    ]).popup({ window: win });
  }));
  ipcMain.on('translate', fromUi(() => {
    const t = tabs.get(activeId);
    if (!t) return;
    if (t.tr && (t.tr.state === 'done' || t.tr.state === 'working')) showOriginal(t); else translateTab(t);
  }));
  ipcMain.on('translate-menu', fromUi(() => translateMenu(activeId)));
  ipcMain.on('shield', fromUi((right) => { hideExtMenu(); hideAppMenu(); togglePanel(right); }));
  ipcMain.on('focus-page', fromUi(() => activeWc()?.focus()));
  // Warm up a connection before the user hits Enter or clicks.
  ipcMain.on('preconnect', fromUi((url) => {
    try {
      const target = url ? new URL(url) : new URL(SEARCH_ENGINES[settings.searchEngine]);
      if (/^https?:$/.test(target.protocol) && !settings.privacy.blockPrefetch) ses.preconnect({ url: target.origin, numSockets: 1 });
    } catch { /* ignore */ }
  }));
  ipcMain.on('tab-menu', fromUi((id) => tabMenu(id)));
  ipcMain.on('group-menu', fromUi((gid) => groupMenu(gid)));
  ipcMain.on('group-toggle', fromUi((gid) => { const g = groups.get(gid); if (g) setGroupCollapsed(gid, !g.collapsed); }));
  ipcMain.on('group-rename', fromUi((gid, name) => {
    const g = groups.get(gid);
    if (g) { g.name = String(name || '').slice(0, 40); scheduleState(); saveTabsSoon(); }
  }));
  ipcMain.on('move-tab', fromUi((id, beforeId, groupId) => moveTab(id, beforeId, groupId)));
  ipcMain.on('merge-tabs', fromUi((id, targetId) => mergeTabs(id, targetId)));
  ipcMain.on('move-group', fromUi((gid, beforeId) => moveGroup(gid, beforeId)));
  ipcMain.on('add-to-group', fromUi((id, gid) => addToGroup(id, gid)));
  ipcMain.on('find', fromUi((text, forward, next) => {
    const wc = activeWc();
    if (!wc) return;
    if (!text) { wc.stopFindInPage('clearSelection'); ui.webContents.send('find-result', { active: 0, total: 0 }); return; }
    wc.findInPage(String(text), { forward: forward !== false, findNext: !!next });
  }));
  ipcMain.on('find-stop', fromUi(() => { activeWc()?.stopFindInPage('clearSelection'); activeWc()?.focus(); }));
}

// ------------------------------------------------------- internal pages IPC
function fromPage(host, handler) {
  return (event, ...args) => {
    const url = event.senderFrame && event.senderFrame.url;
    if (!url || !url.startsWith(`${SCHEME}://${host}/`)) throw new Error('forbidden');
    return handler(...args);
  };
}

function cookieUrl(c) {
  const host = String(c.domain || '').replace(/^\./, '');
  return `${c.secure ? 'https' : 'http'}://${host}${c.path || '/'}`;
}

function wirePagesIpc() {
  ipcMain.on('goat:fp-config', (event) => {
    const url = event.senderFrame && event.senderFrame.url;
    if (!settings || !url || !/^https?:/.test(url)) { event.returnValue = { level: 'off', webauthn: 'allow' }; return; }
    const langs = acceptLanguage(settings.languages.sites).split(',').map((x) => x.split(';')[0]);
    let top = url;
    try { top = event.sender.getURL() || url; } catch { /* ignore */ }
    event.returnValue = { level: settings.privacy.fingerprint, webauthn: settings.privacy.webauthn, seed: fpSeed(top || url), chrome: CHROME_MAJOR, langs,
      googleCompat: isGoogleSignIn(url) ? firefoxUA() : '',
      paste: !!(settings.uploads && settings.uploads.paste),
      extIds: event.sender.session === ses && event.senderFrame && !event.senderFrame.parent ? externallyConnectableIds(url) : [] };
  });
  // Paste images when uploading: the clipboard's image (copied pixels, or a copied image file).
  ipcMain.handle('goat:clip-image', async () => {
    const { nativeImage } = require('electron');
    const types = { png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp', bmp: 'image/bmp' };
    let items = [];
    try { items = await clipboard.read(); } catch { return null; }
    let bytes = null; let mime = '';
    for (const it of items) {
      const t = it.types.find((x) => /^image\//.test(x));
      if (t) {
        try { bytes = Buffer.from(await (await it.getType(t)).arrayBuffer()); mime = t; } catch { /* next */ }
        if (bytes) break;
      }
      // A copied image file (file managers put file:// links on the clipboard).
      if (it.types.includes('text/uri-list')) {
        try {
          const blob = await it.getType('text/uri-list');
          const list = String(typeof blob === 'string' ? blob : await blob.text()).split(/\r?\n/).map((x) => x.trim()).filter((x) => x && !x.startsWith('#'));
          if (list.length === 1 && /^file:\/\//.test(list[0])) {
            const file = require('url').fileURLToPath(list[0]);
            const ext = path.extname(file).slice(1).toLowerCase();
            if (types[ext] && fs.statSync(file).size < 40 * 1024 * 1024) { bytes = fs.readFileSync(file); mime = types[ext]; break; }
          }
        } catch { /* not a file */ }
      }
    }
    if (!bytes || bytes.length > 40 * 1024 * 1024) return null;
    const size = nativeImage.createFromBuffer(bytes).getSize();
    return { b64: bytes.toString('base64'), mime, w: size.width, h: size.height };
  });
  // A site wants a passkey / security key: ask before Windows shows its dialog.
  const webauthnAllowed = new Map(); // origin -> true (this run only)
  const webauthnPending = new Map();
  ipcMain.handle('goat:webauthn-ask', async (event) => {
    const frame = event.senderFrame;
    const origin = frame ? frame.origin : '';
    if (!/^https:\/\//.test(origin) && !/^http:\/\/(localhost|127\.)/.test(origin)) return false;
    if (settings.privacy.webauthn === 'allow' || webauthnAllowed.get(origin)) return true;
    if (settings.privacy.webauthn === 'block') return false;
    if (webauthnPending.has(origin)) return webauthnPending.get(origin);
    const host = origin.replace(/^https?:\/\//, '');
    const p = dialog.showMessageBox(win, {
      type: 'question', buttons: ['Not now', 'Continue'], defaultId: 1, cancelId: 0,
      message: `${host} wants to sign you in with a passkey or security key.`,
      detail: 'Your system will ask for your phone or a USB security key. Choose "Not now" to sign in another way.',
      checkboxLabel: 'Allow for this site until Goat closes', checkboxChecked: false,
    }).then(({ response, checkboxChecked }) => {
      const ok = response === 1;
      if (ok && checkboxChecked) webauthnAllowed.set(origin, true);
      return ok;
    }).catch(() => false).finally(() => webauthnPending.delete(origin));
    webauthnPending.set(origin, p);
    return p;
  });
  ipcMain.on('goat:appearance-get', (event) => {
    const url = event.senderFrame && event.senderFrame.url;
    event.returnValue = url && url.startsWith(`${SCHEME}://`) && settings ? appearance() : null;
  });
  ipcMain.handle('goat:history-query', fromPage('history', (opts) => queryHistory(opts || {})));
  ipcMain.handle('goat:history-delete', fromPage('history', (ids) => {
    const set = new Set((ids || []).map(Number));
    history = history.filter((h) => !set.has(h.id));
    saveHistorySoon();
    return true;
  }));
  ipcMain.handle('goat:history-clear', fromPage('history', (range) => { clearHistory(String(range)); return true; }));
  ipcMain.handle('goat:history-open', fromPage('history', (url, how) => {
    if (!/^https?:\/\//.test(String(url))) return;
    if (how === 'background') createTab(String(url), { activate: false, position: 'end' });
    else activeWc()?.loadURL(String(url));
  }));
  ipcMain.handle('goat:history-settings', fromPage('history', () => { openInternal(SETTINGS_URL + '#privacy'); }));
  ipcMain.handle('goat:newtab-bookmarks', fromPage('newtab', () => withIcons(bookmarks.items)));
  ipcMain.handle('goat:newtab-add', fromPage('newtab', (rawUrl, rawTitle) => {
    const text = String(rawUrl || '').trim();
    const url = toUrl(text);
    if (!url || url === searchUrl(text) || !/^(https?|goat):/i.test(url)) return { error: 'Enter a web address, like youtube.com' };
    let title = String(rawTitle || '').trim();
    if (!title) {
      try {
        const h = new URL(url).hostname.replace(/^www\./, '').split('.')[0];
        title = h.charAt(0).toUpperCase() + h.slice(1);
      } catch { title = url; }
    }
    return addBookmark(url, title) ? { ok: true } : { error: "That address can't be bookmarked" };
  }));
  ipcMain.handle('goat:newtab-remove', fromPage('newtab', (id) => { removeBookmark(String(id)); return true; }));
  // Right-click → Edit name and link.
  ipcMain.handle('goat:newtab-edit', fromPage('newtab', (id, rawUrl, rawTitle) => {
    const hit = findBookmark(String(id));
    if (!hit || hit.b.type !== 'link') return { error: 'That bookmark is gone' };
    const text = String(rawUrl || '').trim();
    const url = toUrl(text);
    if (!url || url === searchUrl(text) || !/^(https?|goat):/i.test(url)) return { error: 'Enter a web address, like youtube.com' };
    hit.b.url = url;
    const title = String(rawTitle || '').trim();
    if (title) hit.b.title = title.slice(0, 120);
    saveBookmarks();
    return { ok: true };
  }));
  ipcMain.handle('goat:newtab-open', fromPage('newtab', (id, how) => {
    const hit = findBookmark(String(id));
    if (hit && hit.b.type === 'link') openBookmarkUrl(hit.b.url, how === 'background' ? 'background' : 'current');
  }));
  ipcMain.handle('goat:newtab-search', fromPage('newtab', (q) => {
    const u = toUrl(q);
    const t = activeWc();
    if (u && t) t.loadURL(u);
  }));
  ipcMain.handle('goat:settings-get', fromPage('settings', () => ({
    windowMode: MODE,
    languages: LANGUAGES,
    profiles: PROFILES,
    settings: publicSettings(),
    blocker: blockerStatus(),
    engines: Object.keys(SEARCH_ENGINES),
    themes: Object.entries(THEMES).map(([id, t]) => ({ id, name: t.name, scheme: t.scheme })),
    customVars: Object.fromEntries(settings.appearance.customThemes.map((t) => [t.id, customVars(t.colors)])),
    version: require(path.join(__dirname, "..", "package.json")).version,
    chrome: process.versions.chrome,
    installed: fs.existsSync(path.join(path.dirname(process.execPath), '.goat-installed')),
  })));
  ipcMain.handle('goat:settings-set', fromPage('settings', (patch) => applySettings(patch)));
  ipcMain.handle('goat:blocker-status', fromPage('settings', () => blockerStatus()));
  ipcMain.handle('goat:blocker-refresh', fromPage('settings', async () => {
    fs.rmSync(path.join(userDir(), 'blocklists'), { recursive: true, force: true });
    blockerPromise = reloadBlocker();
    await blockerPromise;
    return blockerStatus();
  }));
  ipcMain.handle('goat:cookies-list', fromPage('settings', async () => {
    const list = await ses.cookies.get({});
    return list.map((c) => ({
      domain: c.domain, name: c.name, value: c.value, path: c.path, secure: c.secure,
      httpOnly: c.httpOnly, session: c.session, expirationDate: c.expirationDate, sameSite: c.sameSite,
      hostOnly: c.hostOnly,
    }));
  }));
  ipcMain.handle('goat:cookies-remove', fromPage('settings', async (list) => {
    for (const c of list || []) await ses.cookies.remove(cookieUrl(c), c.name).catch(() => {});
    return true;
  }));
  ipcMain.handle('goat:cookies-set', fromPage('settings', async (c, original) => {
    if (original && (original.name !== c.name || original.domain !== c.domain || original.path !== c.path)) {
      await ses.cookies.remove(cookieUrl(original), original.name).catch(() => {});
    }
    const details = {
      url: cookieUrl(c), name: String(c.name), value: String(c.value ?? ''), path: c.path || '/',
      secure: !!c.secure, httpOnly: !!c.httpOnly,
      sameSite: ['unspecified', 'no_restriction', 'lax', 'strict'].includes(c.sameSite) ? c.sameSite : 'lax',
    };
    if (!c.hostOnly && c.domain) details.domain = c.domain;
    if (!c.session && c.expirationDate) details.expirationDate = Number(c.expirationDate);
    await ses.cookies.set(details);
    return true;
  }));
  ipcMain.handle('goat:cookies-clear', fromPage('settings', async () => {
    await ses.clearStorageData({ storages: ['cookies'] });
    return true;
  }));
  ipcMain.handle('goat:tasks', fromPage('settings', () => taskList()));
  ipcMain.handle('goat:profiles', fromPage('settings', () => profilesInfo()));
  ipcMain.handle('goat:profile-create', fromPage('settings', (name, color) => { const id = createProfile(name, color); scheduleState(); applyTaskbarIdentity(); return { id, info: profilesInfo() }; }));
  ipcMain.handle('goat:profile-update', fromPage('settings', (id, patch) => { updateProfile(String(id), patch || {}); return profilesInfo(); }));
  ipcMain.handle('goat:profile-delete', fromPage('settings', async (id) => { const r = await deleteProfile(String(id)); applyTaskbarIdentity(); return { ...r, info: profilesInfo() }; }));
  ipcMain.handle('goat:profile-open', fromPage('settings', (id) => { openProfile(String(id)); return true; }));
  ipcMain.handle('goat:profile-shortcut', fromPage('settings', (id) => profileShortcut(String(id))));
  ipcMain.handle('goat:update-status', fromPage('settings', () => ({ ...update, current: APP_VERSION, platform: process.platform })));
  ipcMain.handle('goat:update-check', fromPage('settings', () => checkForUpdates(true)));
  ipcMain.handle('goat:update-install', fromPage('settings', () => installUpdate()));
  ipcMain.handle('goat:uninstall', fromPage('settings', async () => {
    const r = await dialog.showMessageBox(win, {
      type: 'question', buttons: ['Cancel', 'Uninstall…'], defaultId: 0, cancelId: 0,
      message: 'Uninstall Goat?', detail: 'Goat will close and the uninstaller will open. You can choose there whether to keep your profile.',
    });
    if (r.response !== 1) return false;
    require('child_process').spawn(process.execPath, [...(process.defaultApp ? [process.argv[1]] : []), '--goat-uninstall'], { detached: true, stdio: 'ignore' }).unref();
    return true;
  }));
  ipcMain.handle('goat:tor-status', fromPage('settings', () => ({ ...tor.status(), enabled: settings.tor.enabled })));
  ipcMain.handle('goat:tor-newcircuit', fromPage('settings', async () => {
    const ok = await tor.newCircuit().catch(() => false);
    if (ok) await Promise.allSettled(allSessions().map((s) => s.closeAllConnections()));
    return ok;
  }));
  ipcMain.handle('goat:tor-check', fromPage('settings', () => { createTab('https://check.torproject.org/'); }));
  ipcMain.handle('goat:dns-status', fromPage('settings', () => dnsStatus()));
  ipcMain.handle('goat:dns-check', fromPage('settings', (url) => { if (/^https:\/\//.test(String(url))) createTab(String(url)); }));
  ipcMain.handle('goat:task-sleep', fromPage('settings', (id) => {
    const t = tabs.get(id);
    if (t && id !== activeId) sleepTab(t);
    return true;
  }));
  ipcMain.handle('goat:task-close', fromPage('settings', (id) => { closeTab(id); return true; }));
  ipcMain.handle('goat:clear-data', fromPage('settings', async () => {
    await clearBrowsingData();
    return true;
  }));
}

// ----------------------------------------------------------------- extensions
const extApi = () => ses.extensions || ses; // Electron 36+ moved these to session.extensions
const extensionsDir = () => path.join(userDir(), 'Extensions');

function describePermissions(manifest) {
  const perms = [...(manifest.permissions || []), ...(manifest.host_permissions || [])]
    .filter((p) => typeof p === 'string');
  const broad = perms.some((p) => p === '<all_urls>' || p.startsWith('*://*/') || p === 'http://*/*' || p === 'https://*/*');
  const lines = [];
  if (broad) lines.push('• Read and change data on all websites');
  if (perms.includes('tabs')) lines.push('• See your open tabs');
  if (perms.includes('cookies')) lines.push('• Read and change cookies');
  if (perms.includes('history')) lines.push('• Read browsing history');
  if (perms.includes('downloads')) lines.push('• Manage downloads');
  if (perms.includes('webRequest') || perms.includes('declarativeNetRequest')) lines.push('• Block or change network requests');
  return lines.length ? 'It will be able to:\n' + lines.join('\n') : 'It asks for no special permissions.';
}

function extensionMenuItems(wc, params) {
  try {
    const items = extensions ? extensions.getContextMenuItems(wc, params) : [];
    return items.length ? [...items, { type: 'separator' }] : [];
  } catch { return []; }
}

// "__MSG_extName__" style names and descriptions come from the extension's _locales.
const extMsgCache = new Map();
function extMessages(e) {
  if (extMsgCache.has(e.id + e.version)) return extMsgCache.get(e.id + e.version);
  const m = e.manifest || {};
  const lang = app.getLocale().replace('-', '_');
  let msgs = {};
  for (const loc of [lang, lang.split('_')[0], m.default_locale, 'en'].filter(Boolean)) {
    try { msgs = JSON.parse(fs.readFileSync(path.join(e.path, '_locales', loc, 'messages.json'), 'utf8')); break; } catch { /* try next */ }
  }
  const lower = Object.fromEntries(Object.entries(msgs).map(([k, v]) => [k.toLowerCase(), v && v.message]));
  extMsgCache.set(e.id + e.version, lower);
  return lower;
}
function extText(e, text) {
  const t = String(text || '');
  const mm = /^__MSG_(.+)__$/.exec(t);
  return mm ? (extMessages(e)[mm[1].toLowerCase()] || t) : t;
}
const extIconCache = new Map();
function extIcon(e) {
  if (extIconCache.has(e.id + e.version)) return extIconCache.get(e.id + e.version);
  const m = e.manifest || {};
  const sets = [m.icons, (m.action || m.browser_action || {}).default_icon].filter((x) => x && typeof x === 'object');
  let best = null;
  for (const set of sets) {
    for (const [size, file] of Object.entries(set)) {
      const n = Number(size);
      if (!best || (n >= 32 && (best.n < 32 || n < best.n)) || (best.n < 32 && n > best.n)) best = { n, file };
    }
  }
  let url = null;
  if (best) {
    try {
      const buf = fs.readFileSync(path.join(e.path, String(best.file).replace(/^\//, '')));
      const ext = path.extname(best.file).toLowerCase();
      const mime = ext === '.svg' ? 'image/svg+xml' : ext === '.jpg' || ext === '.jpeg' ? 'image/jpeg' : ext === '.webp' ? 'image/webp' : 'image/png';
      url = `data:${mime};base64,${buf.toString('base64')}`;
    } catch { /* no icon */ }
  }
  extIconCache.set(e.id + e.version, url);
  return url;
}

function listExtensions() {
  return extApi().getAllExtensions().map((e) => {
    const m = e.manifest || {};
    const options = (m.options_ui && m.options_ui.page) || m.options_page || null;
    return {
      id: e.id, name: extText(e, e.name), version: e.version, description: extText(e, m.description || ''),
      icon: extIcon(e), pinned: (settings.extensions.pinned || []).includes(e.id), hasAction: !!(m.action || m.browser_action),
      unpacked: settings.extensions.unpacked.includes(e.path), hasOptions: !!options,
    };
  });
}

function wireExtensionsIpc() {
  ipcMain.handle('goat:ext-list', fromPage('settings', () => listExtensions()));
  ipcMain.handle('goat:ext-pin', fromPage('settings', (id, pinned) => { setExtensionPinned(String(id), !!pinned); return listExtensions(); }));
  ipcMain.handle('goat:ext-webstore', fromPage('settings', () => { createTab('https://chromewebstore.google.com/category/extensions'); }));
  ipcMain.handle('goat:ext-options', fromPage('settings', (id) => {
    const e = extApi().getExtension(id);
    if (!e) return;
    const m = e.manifest || {};
    const page = (m.options_ui && m.options_ui.page) || m.options_page;
    if (page) createTab(`chrome-extension://${id}/${page.replace(/^\//, '')}`);
  }));
  ipcMain.handle('goat:ext-remove', fromPage('settings', async (id) => {
    const e = extApi().getExtension(id);
    if (!e) return listExtensions();
    if (settings.extensions.unpacked.includes(e.path)) {
      extApi().removeExtension(id);
      settings.extensions.unpacked = settings.extensions.unpacked.filter((p) => p !== e.path);
      saveSettings();
    } else {
      await uninstallExtension(id, { session: ses, extensionsPath: extensionsDir() });
    }
    return listExtensions();
  }));
  ipcMain.handle('goat:ext-load-unpacked', fromPage('settings', async () => {
    const r = await dialog.showOpenDialog(win, {
      title: 'Choose an unpacked extension folder (with manifest.json)', properties: ['openDirectory'],
    });
    if (r.canceled || !r.filePaths[0]) return { list: listExtensions() };
    const dir = r.filePaths[0];
    try {
      await extApi().loadExtension(dir, { allowFileAccess: true });
      if (!settings.extensions.unpacked.includes(dir)) {
        settings.extensions.unpacked = [...settings.extensions.unpacked, dir];
        saveSettings();
      }
      return { list: listExtensions() };
    } catch (err) {
      return { list: listExtensions(), error: err.message };
    }
  }));
}

// --------------------------------------------------------------------- window
function layout() {
  if (!win) return;
  const { width, height } = win.getContentBounds();
  const top = fullscreen ? 0 : uiHeight();
  ui.setVisible(!fullscreen);
  ui.setBounds({ x: 0, y: 0, width, height: uiHeight() });
  const t = tabs.get(activeId);
  if (t) t.view.setBounds({ x: 0, y: top, width, height: Math.max(0, height - top) });
}

function createWindow() {
  win = new BaseWindow({
    width: 1280, height: 840, minWidth: 520, minHeight: 320,
    title: windowTitle(), backgroundColor: appearance().bg, icon: path.join(__dirname, 'icon.png'),
    // Hide the OS title bar; min/max/close are drawn over the right end of the tab strip.
    titleBarStyle: 'hidden',
    titleBarOverlay: { color: '#00000000', symbolColor: appearance().text, height: CONTROLS_HEIGHT },
  });
  applyTaskbarIdentity();
  ui = new WebContentsView({
    webPreferences: {
      preload: path.join(__dirname, 'ui', 'preload.bundle.js'),
      contextIsolation: true, sandbox: true, nodeIntegration: false, spellcheck: false,
    },
  });
  ui.setBackgroundColor('#101214');
  win.contentView.addChildView(ui);
  ui.webContents.on('before-input-event', handleShortcut);
  ui.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  ui.webContents.on('will-navigate', (e) => e.preventDefault());
  ui.webContents.loadFile(path.join(__dirname, 'ui', 'index.html'));
  ui.webContents.on('did-finish-load', () => { applyAppearance(); pushState(); });
  win.on('resize', () => { hidePanel(); hideExtMenu(); hideAppMenu(); layout(); });
  win.on('blur', () => { hidePanel(); hideExtMenu(); hideAppMenu(); });
  win.on('leave-full-screen', () => { if (fullscreen) { fullscreen = false; layout(); } });
  win.on('close', () => { saveTabsNow(); });
  win.on('closed', () => { win = null; app.quit(); });
  layout();
}

// ------------------------------------------------------------------ lifecycle
// A web address or a local page Windows asked Goat to open (default browser, "Open with").
function launchUrl(argv) {
  for (const a of argv.slice(1)) {
    if (/^https?:\/\//i.test(a)) return a;
    if (/\.(html?|xhtml|svg|pdf)$/i.test(a)) {
      try { if (fs.statSync(a).isFile()) return pathToFileURL(path.resolve(a)).toString(); } catch { /* not a file */ }
    }
  }
  return null;
}
if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on('second-instance', (_e, argv) => {
    if (argv.includes('--goat-quit')) {
      // The installer or uninstaller needs Goat closed: save everything and exit.
      for (const c of windowProcs) { try { c.kill(); } catch { /* gone */ } }
      app.quit();
      return;
    }
    if (!win) return;
    if (win.isMinimized()) win.restore();
    win.focus();
    const url = launchUrl(argv);
    if (url) createTab(url, { position: 'end' });
  });

  app.whenReady().then(async () => {
    Menu.setApplicationMenu(null);
    loadSettings();
    tor = new Tor({
      dir: path.join(ROOT_DIR, 'tor'), // tor.exe is shared by all profiles
      stateDir: EPHEMERAL ? path.join(app.getPath('userData'), 'tor-state') : path.join(userDir(), 'tor', 'state'),
      onChange: (st) => {
        if (st.state === 'starting') applyProxy();
        if (st.state === 'on') {
          // The port may have changed: point the sessions at it, then reload tabs that failed while connecting.
          applyProxy().then(() => {
            for (const t of tabs.values()) {
              const wc = t.view.webContents;
              if (!wc.isDestroyed() && wc.getURL().startsWith(`${SCHEME}://error/`)) {
                try { wc.loadURL(new URL(wc.getURL()).searchParams.get('u')); } catch { /* ignore */ }
              }
            }
          });
          toast('Tor connected');
        } else if (st.state === 'error') toast('Tor: ' + st.error);
        scheduleState();
        for (const wc of allWebContents.getAllWebContents()) {
          if (!wc.isDestroyed() && wc.getURL().startsWith(SETTINGS_URL)) wc.send('goat:tor', st);
        }
      },
    });
    if (MODE === 'tor') settings.tor.enabled = true;
    if (settings.tor.enabled) tor.start();
    cleanStaleWindowProfiles();
    loadBookmarks();
    loadHistory();
    settings.appearance.customThemes = sanitizeCustomThemes(settings.appearance.customThemes);
    { const info = themeInfo(settings.appearance.theme); nativeTheme.themeSource = info && settings.appearance.theme !== 'system' ? info.scheme : 'system'; }
    applyDns();
    setupSession();
    await applyProxy();
    wireUiIpc();
    wirePagesIpc();
    wireExtensionsIpc();
    wirePanelIpc();
    wireExtMenuIpc();
    wireAppMenuIpc();
    wireBookmarksIpc();
    createWindow();
    scheduleUpdateChecks();

    // Load blocklists before the first page so it loads protected.
    blockerPromise = reloadBlocker();
    await blockerPromise;
    if (!win) return;
    const startUrl = launchUrl(process.argv);
    if (!restoreTabs()) createTab(startUrl || null);
    else if (startUrl) createTab(startUrl, { position: 'end' });
  });
}

app.on('window-all-closed', () => app.quit());
app.on('before-quit', (e) => {
  if (quitting || !settings || !ses) return;
  e.preventDefault();
  quitting = true;
  const first = settings.cookies.clearOnExit ? ses.clearStorageData({ storages: ['cookies'] }) : Promise.resolve();
  Promise.allSettled([first])
    .then(() => Promise.allSettled([ses.cookies.flushStore(), Promise.resolve(ses.flushStorageData())]))
    .finally(() => app.quit());
});
// electron-chrome-extensions can hang Electron's native shutdown (main process
// spins after will-quit). Cookies, storage, settings and tabs are already saved
// by this point, so end the process directly.
app.on('will-quit', () => {
  if (tor) tor.kill();
  if (!EPHEMERAL) {
  try { if (settings) fs.writeFileSync(settingsFile(), JSON.stringify(settings, null, 2)); } catch { /* ignore */ }
  if (historyTimer) saveHistoryNow();
  }
  if (bmTimer && !EPHEMERAL) { try { fs.writeFileSync(bookmarksFile(), JSON.stringify(bookmarks, null, 2)); } catch { /* ignore */ } }
  if (typeof process.reallyExit === 'function') process.reallyExit(0);
  else process.kill(process.pid, 'SIGKILL');
});

// Test hooks (only when GOAT_TEST is set).
if (process.env.GOAT_TEST) {
  global.__goat = {
    createTab, newGroupWith, setGroupCollapsed, addToGroup, removeFromGroup, applySettings, mergeTabs, moveGroup,
    togglePanel, hidePanel, panel: () => panel, addBookmark, bookmarks: () => bookmarks, toggleBookmarkForActive,
    sleepTab, discardTab, activateTab, pendingUrls, listExtensions, shield: () => shieldState, closeTabById: (id) => closeTab(id),
    history: () => history, toggleAppMenu, translateTab, showOriginal, ui: () => ui, skipForExtension, extRouter: () => extensions && extensions.ctx.router, extensions: () => extensions, win: () => win, extMenu: () => extMenu, googleHosts: GOOGLE_SIGNIN_HOSTS, createProfile, deleteProfile, openProfile, loadProfiles, checkForUpdates, update: () => update, appMenu: () => appMenu,
    groups, tabs, order: () => order, activeId: () => activeId, session: () => ses, pushState,
  };
}
