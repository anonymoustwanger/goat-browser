'use strict';
const $ = (id) => document.getElementById(id);
const api = window.goat;
let S = null;

function get(obj, key) { return key.split('.').reduce((o, k) => (o ? o[k] : undefined), obj); }
function patchFor(key, value) {
  const parts = key.split('.');
  const out = {};
  let cur = out;
  parts.forEach((p, i) => { cur[p] = i === parts.length - 1 ? value : {}; cur = cur[p]; });
  return out;
}
function flash(el, text = 'Saved') {
  el.textContent = text;
  clearTimeout(el._t);
  el._t = setTimeout(() => { el.textContent = ''; }, 2500);
}
function el(tag, props = {}, ...kids) {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === 'class') n.className = v;
    else if (k.startsWith('on')) n.addEventListener(k.slice(2), v);
    else if (k in n) n[k] = v;
    else n.setAttribute(k, v);
  }
  for (const c of kids) if (c != null) n.append(c);
  return n;
}

// ------------------------------------------------------------ simple fields
function fillFields() {
  for (const input of document.querySelectorAll('[data-key]')) {
    const v = get(S, input.dataset.key);
    if (input.type === 'checkbox') input.checked = !!v;
    else if (input.type === 'radio') input.checked = input.value === v;
    else input.value = v ?? '';
  }
  for (const input of document.querySelectorAll('[data-proxy]')) {
    const v = S.proxy[input.dataset.proxy];
    if (input.type === 'radio') input.checked = input.value === v;
    else input.value = v ?? '';
  }
  $('rules').value = S.adblock.customRules;
  renderAllowlist();
  renderForget();
  renderProfiles();
  syncProxyFields();
  $('librefields').hidden = S.languages.engine !== 'libre';
  markRestart();
}

// ------------------------------------------------------------- user profiles
let PINFO = null;
function renderProfileList() {
  const box = $('profilelist');
  box.textContent = '';
  for (const p of PINFO.list) {
    const isCur = p.id === PINFO.current;
    const dot = el('button', { type: 'button', class: 'pdot', title: 'Change color', style: `--pc:${p.color}`,
      onclick: async () => {
        const i = PINFO.colors.indexOf(p.color);
        PINFO = await api.profiles.update(p.id, { color: PINFO.colors[(i + 1) % PINFO.colors.length] });
        renderProfileList();
      } }, (p.name || '?').charAt(0).toUpperCase());
    const name = el('input', { class: 'pname', value: p.name, maxLength: 40, spellcheck: false, 'aria-label': 'Profile name',
      onchange: async () => { PINFO = await api.profiles.update(p.id, { name: name.value }); renderProfileList(); } });
    const btns = el('span', { class: 'pbtns' });
    if (isCur) btns.append(el('span', { class: 'pill' }, 'This window'));
    else btns.append(el('button', { type: 'button', onclick: () => { api.profiles.open(p.id); flash($('profilemsg'), `Opening ${p.name}…`); } }, 'Open'));
    if (PINFO.canShortcut) btns.append(el('button', { type: 'button', title: 'Put a shortcut that opens this profile on your desktop', onclick: async () => { flash($('profilemsg'), (await api.profiles.shortcut(p.id)) ? 'Desktop shortcut added' : "Couldn't add shortcut"); } }, 'Shortcut'));
    if (p.id !== 'default' && !isCur) btns.append(el('button', { type: 'button', class: 'danger', onclick: async () => {
      if (!confirmDelete(p)) return;
      const r = await api.profiles.remove(p.id);
      PINFO = r.info; renderProfileList();
      flash($('profilemsg'), r.ok ? `${p.name} deleted` : r.error);
    } }, 'Delete'));
    box.append(el('div', { class: 'prow' + (isCur ? ' cur' : '') }, dot, name, btns));
  }
}
function confirmDelete(p) {
  const b = document.querySelector(`[data-confirm="${p.id}"]`);
  if (b) return true;
  // Two-click delete: first click arms it for 4 s.
  const msg = $('profilemsg');
  if (msg.dataset.arm === p.id) { delete msg.dataset.arm; return true; }
  msg.dataset.arm = p.id;
  flash(msg, `Click Delete again to erase ${p.name} and all its data`);
  setTimeout(() => { if (msg.dataset.arm === p.id) delete msg.dataset.arm; }, 4000);
  return false;
}
$('profileform').addEventListener('submit', async (e) => {
  e.preventDefault();
  const n = $('profilename').value.trim();
  if (!n) return;
  const r = await api.profiles.create(n);
  PINFO = r.info; $('profilename').value = ''; renderProfileList();
  flash($('profilemsg'), `${n} created · click Open to start it`);
});

// -------------------------------------------------------------------- updates
function renderUpdate(u) {
  const st = $('updstate');
  const txt = {
    idle: `You have Goat ${u.current || ''}.`,
    checking: 'Checking for updates…',
    latest: `You're up to date (Goat ${u.current || u.version}).`,
    available: `Goat ${u.version} is available (you have ${u.current}).`,
    downloading: `Downloading Goat ${u.version}… ${u.progress || 0}%`,
    ready: 'Starting the installer… click Update in its window.',
    error: `Couldn't check: ${u.error}`,
  }[u.state] || '';
  st.textContent = txt;
  st.className = u.state === 'available' ? 'good' : u.state === 'error' ? 'bad' : 'muted';
  $('updinstall').hidden = !(u.state === 'available' || (u.state === 'error' && u.version && u.asset));
  $('updinstall').textContent = u.platform === 'win32' && u.asset ? 'Download and install' : 'Open download page';
  $('updcheck').disabled = u.state === 'checking' || u.state === 'downloading';
  $('updbar').hidden = u.state !== 'downloading';
  $('updfill').style.width = (u.progress || 0) + '%';
}
let UCUR = '';
let UPLAT = '';
if (api) api.updates.onStatus((u) => renderUpdate({ ...u, current: UCUR, platform: UPLAT }));
$('updcheck').addEventListener('click', () => api.updates.check());
$('updinstall').addEventListener('click', () => api.updates.install());

// ----------------------------------------------------------------- profiles
let PROFILES = {};
function profileMatches(patch, cur) {
  return Object.entries(patch).every(([k, v]) => (v && typeof v === 'object' ? profileMatches(v, (cur || {})[k]) : (cur || {})[k] === v));
}
function currentProfile() {
  for (const [id, p] of Object.entries(PROFILES)) if (profileMatches(p.patch, S)) return id;
  return 'custom';
}
function renderProfiles() {
  const box = $('privprofiles');
  if (!box || !S) return;
  const cur = currentProfile();
  box.textContent = '';
  for (const [id, p] of Object.entries(PROFILES)) {
    box.append(el('button', {
      type: 'button', class: 'profile' + (cur === id ? ' on' : ''), role: 'radio', 'aria-checked': String(cur === id),
      onclick: async () => {
        S = await api.setSettings(p.patch);
        fillFields();
        pollBlocker();
        setTimeout(refreshDns, 300);
        $('profrestart').hidden = !document.querySelector('[data-restart^="privacy."]:not([hidden])');
      },
    }, el('b', {}, p.name), el('small', {}, p.desc)));
  }
  if (cur === 'custom') box.append(el('p', { class: 'hint custom' }, 'Custom: your own mix of the switches below.'));
}

// Settings read at startup: show "restart" when changed this session.
const START = {};
function markRestart() {
  for (const em of document.querySelectorAll('[data-restart]')) {
    const k = em.dataset.restart;
    if (!(k in START)) START[k] = get(S, k);
    em.hidden = get(S, k) === START[k];
  }
}

function renderForget() {
  const ul = $('forgetlist');
  ul.textContent = '';
  const list = S.privacy.forgetSites || [];
  if (!list.length) ul.append(el('li', { class: 'empty' }, 'No sites yet.'));
  for (const site of list) {
    ul.append(el('li', {}, site, el('button', {
      class: 'x', title: `Remove ${site}`, 'aria-label': `Remove ${site}`,
      onclick: async () => { S = await api.setSettings({ privacy: { forgetSites: S.privacy.forgetSites.filter((x) => x !== site) } }); renderForget(); },
    }, '×')));
  }
}
$('forgetform').addEventListener('submit', async (e) => {
  e.preventDefault();
  const v = $('forgetinput').value.trim();
  if (!v) return;
  S = await api.setSettings({ privacy: { forgetSites: [...(S.privacy.forgetSites || []), v] } });
  $('forgetinput').value = '';
  renderForget();
});

// ---------------------------------------------------------------------- tor
function renderTor(st) {
  const on = st.enabled !== false && S && S.tor.enabled;
  const txt = !on ? 'Off'
    : st.state === 'downloading' ? `Downloading Tor… ${st.progress || 0}%`
    : st.state === 'starting' ? `Connecting to the Tor network… ${st.progress || 0}%`
    : st.state === 'on' ? `Connected${st.version ? ` · Tor ${st.version}` : ''}`
    : st.state === 'error' ? `Not working: ${st.error}`
    : 'Starting…';
  const box = $('torstate');
  box.textContent = txt;
  box.className = on && st.state === 'error' ? 'bad' : on && st.state === 'on' ? 'good' : 'muted';
  $('tornew').disabled = !(on && st.state === 'on');
  $('torcheck').disabled = !(on && st.state === 'on');
}
async function refreshTor() { renderTor(await api.tor.status()); }
if (api) api.tor.onStatus((st) => renderTor(st));
$('tornew').addEventListener('click', async () => {
  $('tornew').disabled = true;
  const ok = await api.tor.newCircuit();
  $('torstate').textContent = ok ? 'New circuit ready · reload a page to use it' : "Couldn't change circuit";
  setTimeout(refreshTor, 4000);
});
$('torcheck').addEventListener('click', () => api.tor.check());

document.querySelectorAll('[data-key]').forEach((input) => {
  input.addEventListener('change', async () => {
    const v = input.type === 'checkbox' ? input.checked : input.dataset.number ? Number(input.value) : input.value;
    if (input.type === 'radio' && !input.checked) return;
    S = await api.setSettings(patchFor(input.dataset.key, v));
    if (input.dataset.key.startsWith('adblock')) pollBlocker();
    if (input.dataset.key.startsWith('tor')) refreshTor();
    if (input.dataset.key === 'languages.engine') $('librefields').hidden = S.languages.engine !== 'libre';
    markRestart();
    renderProfiles();
  });
});

// --------------------------------------------------------------- ad blocker
const LEVEL_NAME = { ads: 'Ads only', standard: 'Standard', strict: 'Strict' };
async function pollBlocker() {
  const st = await api.blockerStatus();
  const text = {
    on: `On · ${LEVEL_NAME[st.level]} lists loaded`,
    loading: 'Loading blocklists…',
    error: "Couldn't download blocklists. Check your connection or proxy.",
    disabled: 'Off',
  }[st.state] || st.state;
  $('abstatus').textContent = text;
  if (st.state === 'loading') setTimeout(pollBlocker, 800);
}
$('abrefresh').addEventListener('click', async () => {
  $('abrefresh').disabled = true;
  $('abstatus').textContent = 'Downloading fresh lists…';
  await api.refreshBlocklists();
  $('abrefresh').disabled = false;
  pollBlocker();
});

function renderAllowlist() {
  const ul = $('allowlist');
  ul.textContent = '';
  if (!S.adblock.allowlist.length) ul.append(el('li', { class: 'empty' }, 'No sites yet.'));
  for (const site of S.adblock.allowlist) {
    ul.append(el('li', {}, site, el('button', {
      class: 'x', title: `Remove ${site}`, 'aria-label': `Remove ${site}`,
      onclick: async () => { S = await api.setSettings({ adblock: { allowlist: S.adblock.allowlist.filter((s) => s !== site) } }); renderAllowlist(); },
    }, '×')));
  }
}
$('allowform').addEventListener('submit', async (e) => {
  e.preventDefault();
  const v = $('allowinput').value.trim();
  if (!v) return;
  S = await api.setSettings({ adblock: { allowlist: [...S.adblock.allowlist, v] } });
  $('allowinput').value = '';
  renderAllowlist();
});
$('rulesave').addEventListener('click', async () => {
  S = await api.setSettings({ adblock: { customRules: $('rules').value } });
  flash($('rulesaved'), 'Saved · reloading blocker');
  pollBlocker();
});

// -------------------------------------------------------------------- proxy
function syncProxyFields() {
  const mode = document.querySelector('[data-proxy="mode"]:checked')?.value;
  $('manualfields').hidden = mode !== 'manual';
  $('pacfields').hidden = mode !== 'pac';
}
document.querySelectorAll('[data-proxy="mode"]').forEach((r) => r.addEventListener('change', syncProxyFields));
$('proxysave').addEventListener('click', async () => {
  const proxy = {};
  for (const input of document.querySelectorAll('[data-proxy]')) {
    if (input.type === 'radio') { if (input.checked) proxy.mode = input.value; }
    else proxy[input.dataset.proxy] = input.value.trim();
  }
  if (proxy.mode === 'manual' && (!proxy.host || !/^\d{1,5}$/.test(proxy.port))) {
    flash($('proxysaved'), 'Enter a host and a numeric port.');
    return;
  }
  S = await api.setSettings({ proxy });
  fillFields();
  flash($('proxysaved'), 'Proxy applied');
});

// ----------------------------------------------------------- cookie manager
let cookies = [];
const open = new Set();
const host = (c) => c.domain.replace(/^\./, '');

function fmtExpiry(c) {
  if (c.session || !c.expirationDate) return 'Session';
  const d = new Date(c.expirationDate * 1000);
  return d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

async function loadCookies() {
  cookies = await api.cookies.list();
  renderCookies();
}

function cookieRow(c) {
  const value = el('input', { class: 'val', value: c.value, spellcheck: false, title: 'Edit value, then press Enter' });
  const save = async () => {
    if (value.value === c.value) return;
    await api.cookies.set({ ...c, value: value.value }, c);
    c.value = value.value;
    value.classList.add('ok');
    setTimeout(() => value.classList.remove('ok'), 900);
  };
  value.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); save(); value.blur(); } });
  value.addEventListener('change', save);
  const flags = [c.secure && 'Secure', c.httpOnly && 'HttpOnly', c.sameSite && c.sameSite !== 'unspecified' && `SameSite=${c.sameSite}`]
    .filter(Boolean).join(' · ');
  return el('tr', {},
    el('td', { class: 'name', title: c.name }, c.name),
    el('td', {}, value),
    el('td', { class: 'muted' }, fmtExpiry(c)),
    el('td', { class: 'muted small' }, (c.path !== '/' ? c.path + ' · ' : '') + flags),
    el('td', {}, el('button', {
      class: 'x', title: 'Delete cookie', 'aria-label': `Delete cookie ${c.name}`,
      onclick: async () => { await api.cookies.remove([c]); cookies = cookies.filter((x) => x !== c); renderCookies(); },
    }, '×')),
  );
}

function renderCookies() {
  const q = $('cookiesearch').value.trim().toLowerCase();
  const byHost = new Map();
  for (const c of cookies) {
    if (q && !host(c).includes(q) && !c.name.toLowerCase().includes(q)) continue;
    if (!byHost.has(host(c))) byHost.set(host(c), []);
    byHost.get(host(c)).push(c);
  }
  const hosts = [...byHost.keys()].sort((a, b) => a.replace(/^www\./, '').localeCompare(b.replace(/^www\./, '')));
  $('cookiecount').textContent = `${cookies.length} cookie${cookies.length === 1 ? "" : "s"} · ${new Set(cookies.map(host)).size} site${new Set(cookies.map(host)).size === 1 ? "" : "s"}`;
  const list = $('cookielist');
  list.textContent = '';
  if (!hosts.length) { list.append(el('p', { class: 'empty' }, q ? 'No matches.' : 'No cookies stored.')); return; }
  for (const h of hosts) {
    const items = byHost.get(h);
    const isOpen = open.has(h) || (q && hosts.length <= 3);
    const head = el('div', { class: 'site' + (isOpen ? ' open' : '') },
      el('button', {
        class: 'sitebtn', 'aria-expanded': String(!!isOpen),
        onclick: () => { open.has(h) ? open.delete(h) : open.add(h); renderCookies(); },
      }, el('span', { class: 'caret' }, '›'), el('span', { class: 'host' }, h), el('span', { class: 'muted' }, `${items.length}`)),
      el('button', {
        class: 'linkdanger', onclick: async () => { await api.cookies.remove(items); cookies = cookies.filter((c) => !items.includes(c)); renderCookies(); },
      }, 'Delete site'),
    );
    list.append(head);
    if (isOpen) {
      const table = el('table', {},
        el('thead', {}, el('tr', {}, el('th', {}, 'Name'), el('th', {}, 'Value'), el('th', {}, 'Expires'), el('th', {}, 'Details'), el('th', {}))),
        el('tbody', {}, ...items.sort((a, b) => a.name.localeCompare(b.name)).map(cookieRow)),
      );
      list.append(el('div', { class: 'tablewrap' }, table));
    }
  }
}
$('cookiesearch').addEventListener('input', renderCookies);
$('cookiereload').addEventListener('click', loadCookies);
$('cookieclear').addEventListener('click', async () => {
  if (!confirm('Delete all cookies? You will be signed out of every site.')) return;
  await api.cookies.clear();
  loadCookies();
});

// --------------------------------------------------------------------- DNS
async function refreshDns() {
  if (!S) return;
  const p = S.dns.provider;
  $('dnsnext').hidden = p !== 'nextdns';
  $('dnscustom').hidden = p !== 'custom';
  const st = $('dnsstate');
  st.className = 'muted';
  st.textContent = 'Testing…';
  const r = await api.dns.status();
  const viaProxy = r.proxy && !/^DIRECT/i.test(r.proxy);
  if (!r.server) { st.textContent = 'Off: lookups use your OS DNS and are not encrypted.'; st.className = 'warn'; }
  else if (!r.ok) { st.textContent = `Can't reach ${r.server}. Your network may block it: pick another provider.`; st.className = 'bad'; }
  else st.textContent = `Working: encrypted lookups via ${r.server} (${r.ms} ms).`, st.className = 'good';
  if (viaProxy && r.server) {
    st.textContent += ' A proxy or VPN is set, so websites are looked up by the proxy, not by this DNS.';
    st.className = 'warn';
  }
  $('dnscheck').hidden = !r.check;
  $('dnscheck').onclick = () => api.dns.openCheck(r.check);
}
$('dnstest').addEventListener('click', refreshDns);
document.querySelectorAll('[data-key^="dns."]').forEach((i) => i.addEventListener('change', () => setTimeout(refreshDns, 300)));

// ------------------------------------------------------------ task manager
async function renderTasks() {
  const box = $('tasklist');
  if (!box || document.hidden) return;
  const r = await api.tasks.list();
  $('memtotal').textContent = `Goat is using ${r.totalMb.toLocaleString()} MB: tabs ${(r.totalMb - r.browserMb).toLocaleString()} MB, browser engine ${r.browserMb.toLocaleString()} MB.`;
  box.textContent = '';
  for (const t of r.rows.sort((a, b) => b.mb - a.mb)) {
    const status = t.asleep ? 'sleeping' : t.active ? 'open now' : '';
    box.append(el('tr', {},
      el('td', { class: 'name', title: t.url }, t.title, status ? el('small', { class: 'muted' }, ` · ${status}`) : null),
      el('td', { class: 'num' }, t.asleep && !t.mb ? '–' : `${t.mb.toLocaleString()} MB${t.shared ? '*' : ''}`),
      el('td', { class: 'acts' },
        !t.active && !t.asleep && /^https?:/.test(t.url) ? el('button', { onclick: async () => { await api.tasks.sleep(t.id); setTimeout(renderTasks, 600); } }, 'Sleep') : null,
        t.active ? null : el('button', { class: 'x', title: 'Close tab', 'aria-label': 'Close tab', onclick: async () => { await api.tasks.close(t.id); renderTasks(); } }, '×')),
    ));
  }
}
setInterval(renderTasks, 3000);
document.addEventListener('visibilitychange', renderTasks);

// --------------------------------------------------------------- clear data
$('cleardata').addEventListener('click', async () => {
  if (!confirm('Clear all cookies, cache and site data, and close all tabs?')) return;
  await api.clearBrowsingData();
});

// --------------------------------------------------------------------- init
// Sidebar follows scrolling: the highlighted link is the section you're reading.
function setNav(id) {
  document.querySelectorAll('.side a').forEach((a) => {
    const on = a.getAttribute('href') === '#' + id;
    a.classList.toggle('on', on);
    if (on) a.setAttribute('aria-current', 'true'); else a.removeAttribute('aria-current');
  });
}
let navLock = 0; // ignore scroll updates while a clicked link scrolls into place
let navClicked = null; // short last sections can't reach the top; keep the clicked one
function markNav() {
  if (Date.now() < navLock) return;
  const sections = [...document.querySelectorAll('.content > section')];
  if (!sections.length) return;
  const atBottom = window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - 4;
  let current = sections[0];
  const clicked = navClicked && document.getElementById(navClicked);
  if (atBottom && clicked && clicked.getBoundingClientRect().top < window.innerHeight - 80) current = clicked;
  else if (atBottom) current = sections[sections.length - 1];
  else for (const sec of sections) if (sec.getBoundingClientRect().top <= 140) current = sec;
  setNav(current.id);
}
let navFrame = 0;
window.addEventListener('wheel', () => { navClicked = null; }, { passive: true });
window.addEventListener('scroll', () => {
  if (navFrame) return;
  navFrame = requestAnimationFrame(() => { navFrame = 0; markNav(); });
}, { passive: true });
window.addEventListener('resize', markNav);
document.querySelectorAll('.side a').forEach((a) => a.addEventListener('click', (e) => {
  const id = a.getAttribute('href').slice(1);
  const target = document.getElementById(id);
  if (!target) return;
  e.preventDefault();
  setNav(id);
  navClicked = id;
  navLock = Date.now() + 700;
  target.scrollIntoView({ behavior: document.documentElement.dataset.motion === 'off' ? 'auto' : 'smooth', block: 'start' });
  history.replaceState(null, '', '#' + id);
}));
window.addEventListener('hashchange', () => { const t = document.querySelector(location.hash); if (t) { t.scrollIntoView(); markNav(); } });

// ------------------------------------------------------------------ themes
// Base colors of the built-in themes, used to prefill the editor.
const BUILTIN = {
  midnight: { bg: '#101214', surface: '#181b1f', text: '#e7e9ec', accent: '#5fd3b3', accent2: '#6ea8ff' },
  daylight: { bg: '#eef0f3', surface: '#ffffff', text: '#16191d', accent: '#0d8a6c', accent2: '#4a6cf7' },
  forest: { bg: '#0f1511', surface: '#161e18', text: '#e4ece5', accent: '#8bd17c', accent2: '#d8c35b' },
  ocean: { bg: '#0b1220', surface: '#111a2c', text: '#e3eaf5', accent: '#5ab0ff', accent2: '#8a7dff' },
  sunset: { bg: '#1a1012', surface: '#24161a', text: '#f3e6e3', accent: '#ff9e6b', accent2: '#ff5e8a' },
  grape: { bg: '#140f1c', surface: '#1c1527', text: '#ece6f5', accent: '#c39bff', accent2: '#ff7ac6' },
  mono: { bg: '#0c0c0c', surface: '#161616', text: '#f2f2f2', accent: '#f2f2f2', accent2: '#9a9a9a' },
  paper: { bg: '#f4efe6', surface: '#fffaf2', text: '#2a241c', accent: '#b85a27', accent2: '#d4a03c' },
};
const hexRgb = (h) => { const n = parseInt(h.slice(1), 16); return [n >> 16, (n >> 8) & 255, n & 255]; };
const rgbHex = (c) => '#' + c.map((v) => Math.round(Math.max(0, Math.min(255, v))).toString(16).padStart(2, '0')).join('');
const mixHex = (a, b, t) => { const x = hexRgb(a); const y = hexRgb(b); return rgbHex(x.map((v, i) => v + (y[i] - v) * t)); };
const lum = (h) => {
  const [r, g, b] = hexRgb(h).map((v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
const contrast = (a, b) => { const x = lum(a); const y = lum(b); return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); };
// Same derivation as the main process, for live previews.
function deriveVars(c) {
  const dark = lum(c.bg) < 0.4;
  return {
    '--bg': c.bg, '--bg-2': c.surface, '--bg-3': mixHex(c.surface, c.text, 0.08), '--line': mixHex(c.surface, c.text, 0.15),
    '--text': c.text, '--muted': mixHex(c.text, c.bg, 0.42), '--accent': c.accent, '--accent-2': c.accent2,
    '--accent-ink': lum(c.accent) > 0.4 ? '#0b0d10' : '#ffffff', 'color-scheme': dark ? 'dark' : 'light',
  };
}
function applyVars(node, vars) {
  for (const [k, v] of Object.entries(vars)) node.style.setProperty(k, v);
}

let THEME_LIST = [];
let editing = null; // id of custom theme being edited, or 'new'

function themeCard(t, opts = {}) {
  const input = el('input', { type: 'radio', name: 'theme', value: t.id });
  input.checked = S.appearance.theme === t.id;
  input.addEventListener('change', async () => { if (input.checked) S = await api.setSettings({ appearance: { theme: t.id } }); });
  const swatch = el('span', { class: 'swatch' },
    el('span', { class: 'bar' }, el('i'), el('i')),
    el('span', { class: 'nav' }, el('b')),
    el('span', { class: 'page' }, el('em'), el('u')));
  if (opts.vars) { swatch.dataset.theme = 'custom'; applyVars(swatch, opts.vars); }
  else if (t.id !== 'system') swatch.dataset.theme = t.id;
  const label = el('span', { class: 'label' }, t.name, el('small', {}, t.scheme === 'auto' ? 'match system' : t.scheme));
  const card = el('label', { class: 'theme-card' + (t.id === 'system' ? ' system' : '') }, input, swatch, label);
  if (opts.editable) {
    card.append(el('button', {
      class: 'edit', title: `Edit ${t.name}`, 'aria-label': `Edit ${t.name}`,
      onclick: (e) => { e.preventDefault(); openEditor(t.id); },
    }, 'Edit'));
  }
  return card;
}

function renderThemes(themes, customVars) {
  if (themes) THEME_LIST = themes;
  const box = $('themes');
  box.textContent = '';
  box.append(themeCard({ id: 'system', name: 'System', scheme: 'auto' }));
  for (const t of THEME_LIST) box.append(themeCard(t));
  for (const t of S.appearance.customThemes) {
    const vars = (customVars && customVars[t.id]) || deriveVars(t.colors);
    box.append(themeCard({ id: t.id, name: t.name, scheme: lum(t.colors.bg) < 0.4 ? 'dark' : 'light' }, { vars, editable: true }));
  }
  box.append(el('button', { class: 'theme-new', onclick: () => openEditor('new') },
    el('span', { class: 'plus', 'aria-hidden': 'true' }, '+'), el('span', {}, 'Create theme')));
}

const ED_KEYS = ['bg', 'surface', 'text', 'accent', 'accent2'];
function editorColors() { return Object.fromEntries(ED_KEYS.map((k) => [k, $('ed-' + k).value])); }
function updatePreview() {
  const c = editorColors();
  applyVars($('ed-preview'), deriveVars(c));
  const ratio = contrast(c.text, c.bg);
  const warn = $('ed-warn');
  warn.hidden = ratio >= 4.5;
  warn.textContent = `Text contrast is ${ratio.toFixed(1)}:1. Aim for at least 4.5:1 so text stays readable.`;
}
function fillEditor(c) { for (const k of ED_KEYS) $('ed-' + k).value = c[k]; updatePreview(); }

function openEditor(id) {
  editing = id;
  const existing = S.appearance.customThemes.find((t) => t.id === id);
  $('editor').hidden = false;
  $('editortitle').textContent = existing ? `Edit “${existing.name}”` : 'New theme';
  $('ed-name').value = existing ? existing.name : '';
  $('ed-delete').hidden = !existing;
  const sel = $('ed-base');
  sel.textContent = '';
  sel.append(el('option', { value: '' }, existing ? 'Keep current colors' : 'Choose a theme…'));
  for (const [k] of Object.entries(BUILTIN)) {
    const t = THEME_LIST.find((x) => x.id === k);
    sel.append(el('option', { value: k }, t ? t.name : k));
  }
  const cur = S.appearance.theme;
  const curCustom = S.appearance.customThemes.find((t) => t.id === cur);
  fillEditor(existing ? existing.colors : curCustom ? curCustom.colors : BUILTIN[cur] || BUILTIN.midnight);
  $('editor').scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  $('ed-name').focus();
}
function closeEditor() { editing = null; $('editor').hidden = true; }

$('ed-base').addEventListener('change', (e) => { if (BUILTIN[e.target.value]) fillEditor(BUILTIN[e.target.value]); });
for (const k of ED_KEYS) $('ed-' + k).addEventListener('input', updatePreview);
$('ed-cancel').addEventListener('click', closeEditor);
$('ed-save').addEventListener('click', async () => {
  const name = $('ed-name').value.trim() || 'My theme';
  const colors = editorColors();
  const list = [...S.appearance.customThemes];
  let id = editing;
  if (id === 'new' || !list.some((t) => t.id === id)) {
    id = 'custom-' + Math.random().toString(36).slice(2, 10);
    list.push({ id, name, colors });
  } else {
    const i = list.findIndex((t) => t.id === id);
    list[i] = { id, name, colors };
  }
  S = await api.setSettings({ appearance: { customThemes: list, theme: id } });
  closeEditor();
  const info = await api.getSettings();
  renderThemes(null, info.customVars);
});
$('ed-delete').addEventListener('click', async () => {
  const t = S.appearance.customThemes.find((x) => x.id === editing);
  if (!t || !confirm(`Delete the theme “${t.name}”?`)) return;
  const list = S.appearance.customThemes.filter((x) => x.id !== editing);
  const patch = { appearance: { customThemes: list } };
  if (S.appearance.theme === editing) patch.appearance.theme = 'system';
  S = await api.setSettings(patch);
  closeEditor();
  renderThemes();
});

(async () => {
  if (!api) { document.body.textContent = 'Settings are only available inside Goat.'; return; }
  const info = await api.getSettings();
  S = info.settings;
  PROFILES = info.profiles || {};
  for (const sel of document.querySelectorAll('.langselect')) {
    for (const [code, name] of Object.entries(info.languages || {})) sel.append(el('option', { value: code }, name));
  }
  if (info.windowMode && info.windowMode !== 'normal') {
    $('winnote').hidden = false;
    $('winnote').textContent = `This is a ${info.windowMode === 'tor' ? 'Tor' : 'private'} window. Settings you change here only last until it closes. Change them in a normal window to keep them.`;
  }
  fillFields();
  renderThemes(info.themes, info.customVars);
  pollBlocker();
  loadCookies();
  renderTasks();
  refreshDns();
  refreshTor();
  markNav();
  $('about-text').textContent = `Goat ${info.version} for Linux · Chromium ${info.chrome}. Data folder: ~/.config/Goat.`;
  $('uninstallrow').hidden = !info.installed;
  api.profiles.list().then((p) => { PINFO = p; renderProfileList(); });
  api.updates.status().then((u) => { UCUR = u.current; UPLAT = u.platform; renderUpdate(u); if (u.state === 'idle') api.updates.check(); });
  $('uninstall').addEventListener('click', () => api.uninstall());
  if (location.hash) document.querySelector(location.hash)?.scrollIntoView();
  markNav();
})();
