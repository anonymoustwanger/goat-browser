'use strict';
const $ = (id) => document.getElementById(id);
const root = document.documentElement;
const tabsEl = $('tabs');
const address = $('address');
const sec = $('sec');
const findbar = $('findbar');
const findinput = $('findinput');

let state = { tabs: [], groups: [], activeId: null, shield: 'loading' };
let editing = false; // user is typing in the address bar
let renaming = null; // folder id being renamed
let drag = null; // { kind: 'tab' | 'group', id }
let firstRender = true;
let lastBlocked = 0;
const els = new Map(); // key -> element (keyed so hover/animations survive updates)

const LOCK = '<svg viewBox="0 0 16 16"><rect x="3.5" y="7" width="9" height="6.5" rx="1.5"/><path d="M5.5 7V5.5a2.5 2.5 0 0 1 5 0V7"/></svg>';
const WARN = '<svg viewBox="0 0 16 16"><path d="M8 2 1.8 13h12.4z"/><path d="M8 6.5v3M8 11.3v.2"/></svg>';
const CLOSE = '<svg viewBox="0 0 16 16"><path d="M4 4l8 8M12 4l-8 8"/></svg>';
const SPEAKER = '<svg viewBox="0 0 16 16"><path d="M3 6h2.5L9 3v10L5.5 10H3z"/><path d="M11.5 5.5a3.5 3.5 0 0 1 0 5"/></svg>';
const MUTED = '<svg viewBox="0 0 16 16"><path d="M3 6h2.5L9 3v10L5.5 10H3z"/><path d="m11 6 3 4M14 6l-3 4"/></svg>';
const MASK = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M1.5 6.5c2-1.4 4.2-1.6 6.5-.4 2.3-1.2 4.5-1 6.5.4-.2 2.6-1.6 4.4-3.6 4.4-1.3 0-2.2-.8-2.9-1.9-.7 1.1-1.6 1.9-2.9 1.9-2 0-3.4-1.8-3.6-4.4z"/><circle cx="5" cy="7.8" r=".9"/><circle cx="11" cy="7.8" r=".9"/></svg>';
const FOLDER_CLOSED = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M1.8 4.2c0-.6.4-1 1-1h3.4l1.5 1.6h5.5c.6 0 1 .4 1 1v6.6c0 .6-.4 1-1 1H2.8c-.6 0-1-.4-1-1z" fill="currentColor" fill-opacity=".3"/></svg>';
const FOLDER_OPEN = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M1.8 12V4.2c0-.6.4-1 1-1h3.4l1.5 1.6h4.5c.6 0 1 .4 1 1v1.4"/><path d="M1.8 12.4 3.6 7.6c.1-.4.5-.6.9-.6h9.3c.7 0 1.1.6.9 1.2l-1.5 4.2c-.1.4-.5.6-.9.6H2.3" fill="currentColor" fill-opacity=".3"/></svg>';

const active = () => state.tabs.find((t) => t.id === state.activeId);
const groupOf = (gid) => state.groups.find((g) => g.id === gid);
const motionOn = () => {
  const m = root.dataset.motion;
  if (m === 'off') return false;
  if (m === 'system') return !matchMedia('(prefers-reduced-motion: reduce)').matches;
  return true;
};
function hostLetter(t) {
  try { return new URL(t.url).hostname.replace(/^www\./, '')[0] || '•'; } catch { return '•'; }
}
function afterAnim(el, fn) {
  if (!motionOn()) { fn(); return; }
  let done = false;
  const go = () => { if (!done) { done = true; fn(); } };
  el.addEventListener('animationend', go, { once: true });
  setTimeout(go, 400);
}

// ------------------------------------------------------------- drag & drop
const ZONES = ['drop-before', 'drop-after', 'merge-target'];
function setZone(el, zone) { for (const z of ZONES) el.classList.toggle(z, z === zone); }
function clearZones() { document.querySelectorAll('.drop-before,.drop-after,.merge-target').forEach((e) => setZone(e, null)); tabsEl.classList.remove('drop-end'); }

function nextTabAfter(id, skipGroup) {
  const list = state.tabs;
  let i = list.findIndex((t) => t.id === id);
  if (i < 0) return null;
  i++;
  while (i < list.length && ((drag && drag.kind === 'tab' && list[i].id === drag.id) || (skipGroup && list[i].groupId === skipGroup))) i++;
  return i < list.length ? list[i].id : null;
}

function tabZone(el, t, e) {
  if (!drag) return null;
  const r = el.getBoundingClientRect();
  const x = (e.clientX - r.left) / r.width;
  if (drag.kind === 'tab') {
    if (drag.id === t.id) return null;
    return x < 0.28 ? 'drop-before' : x > 0.72 ? 'drop-after' : 'merge-target';
  }
  if (t.groupId === drag.id) return null;
  return x < 0.5 ? 'drop-before' : 'drop-after';
}

function wireDrag(el) {
  el.addEventListener('dragstart', (e) => {
    // Never start a drag from the close button: it swallowed the click.
    if (e.target.closest && e.target.closest('.close, input')) { e.preventDefault(); return; }
    const d = el._data;
    drag = el._kind === 'tab' ? { kind: 'tab', id: d.id } : { kind: 'group', id: d.id };
    e.dataTransfer.effectAllowed = 'move';
    e.dataTransfer.setData('text/plain', '');
    requestAnimationFrame(() => el.classList.add('dragging'));
  });
  el.addEventListener('dragend', () => { drag = null; el.classList.remove('dragging'); clearZones(); });
  el.addEventListener('dragover', (e) => {
    if (!drag || drag.kind === 'bm') return;
    let zone;
    if (el._kind === 'tab') zone = tabZone(el, el._data, e);
    else if (drag.kind === 'tab') zone = 'merge-target';
    else zone = drag.id === el._data.id ? null : 'drop-before';
    if (!zone) return;
    e.preventDefault();
    e.stopPropagation();
    tabsEl.classList.remove('drop-end');
    setZone(el, zone);
  });
  el.addEventListener('dragleave', (e) => { if (!el.contains(e.relatedTarget)) setZone(el, null); });
  el.addEventListener('drop', (e) => {
    if (!drag) return;
    e.preventDefault();
    e.stopPropagation();
    const zone = ZONES.find((z) => el.classList.contains(z));
    const d = el._data;
    clearZones();
    if (el._kind === 'tab') {
      if (drag.kind === 'tab') {
        if (zone === 'merge-target') goat.send('merge-tabs', drag.id, d.id);
        else if (zone === 'drop-before') goat.send('move-tab', drag.id, d.id, d.groupId);
        else if (zone === 'drop-after') goat.send('move-tab', drag.id, nextTabAfter(d.id), d.groupId);
      } else if (zone === 'drop-before') {
        goat.send('move-group', drag.id, d.groupId ? state.tabs.find((t) => t.groupId === d.groupId).id : d.id);
      } else if (zone === 'drop-after') {
        goat.send('move-group', drag.id, nextTabAfter(d.id, d.groupId));
      }
    } else if (drag.kind === 'tab') {
      goat.send('add-to-group', drag.id, d.id);
    } else if (drag.id !== d.id) {
      const first = state.tabs.find((t) => t.groupId === d.id);
      goat.send('move-group', drag.id, first ? first.id : null);
    }
    drag = null;
  });
}

tabsEl.addEventListener('dragover', (e) => {
  if (!drag || drag.kind === 'bm') return;
  e.preventDefault();
  clearZones();
  tabsEl.classList.add('drop-end');
});
tabsEl.addEventListener('dragleave', (e) => { if (!tabsEl.contains(e.relatedTarget)) tabsEl.classList.remove('drop-end'); });
tabsEl.addEventListener('drop', (e) => {
  if (!drag) return;
  e.preventDefault();
  clearZones();
  if (drag.kind === 'tab') goat.send('move-tab', drag.id, null, null);
  else goat.send('move-group', drag.id, null);
  drag = null;
});

// ------------------------------------------------------------- tab elements
function createTabEl() {
  const el = document.createElement('div');
  el._kind = 'tab';
  el.className = 'tab';
  el.setAttribute('role', 'tab');
  el.draggable = true;
  el.innerHTML = '<span class="dot"></span><span class="title"></span><span class="audio" hidden></span>' +
    `<button class="close" aria-label="Close tab" tabindex="-1">${CLOSE}</button>`;
  const close = el.querySelector('.close');
  const doClose = () => {
    if (el.classList.contains('leaving')) return;
    el.classList.add('leaving');
    afterAnim(el, () => goat.send('close-tab', el._data.id));
  };
  el._close = doClose;
  close.addEventListener('click', (e) => { e.stopPropagation(); doClose(); });
  close.draggable = false;
  close.addEventListener('dragstart', (e) => e.preventDefault());
  close.addEventListener('mousedown', (e) => e.stopPropagation());
  el.addEventListener('mousedown', (e) => {
    if (e.button === 1) { e.preventDefault(); doClose(); }
    else if (e.button === 0) goat.send('activate', el._data.id);
  });
  el.addEventListener('contextmenu', (e) => { e.preventDefault(); goat.send('tab-menu', el._data.id); });
  wireDrag(el);
  return el;
}

function updateTabEl(el, t) {
  el._data = t;
  const g = t.groupId ? groupOf(t.groupId) : null;
  el.classList.toggle('active', t.id === state.activeId);
  el.classList.toggle('loading', !!t.loading);
  el.classList.toggle('asleep', !!t.asleep);
  el.classList.toggle('private', !!t.private);
  if (t.asleep) el.title = `${t.title}\nSleeping to save memory`;
  el.classList.toggle('in-group', !!g);
  for (const c of [...el.classList]) if (c.startsWith('c-')) el.classList.remove(c);
  if (g) el.classList.add(`c-${g.color}`);
  el.setAttribute('aria-selected', String(t.id === state.activeId));
  el.title = t.title + (t.url ? '\n' + t.url : '');
  const dot = el.querySelector('.dot');
  if (t.private) { if (!dot.querySelector('svg')) dot.innerHTML = MASK; }
  else if (t.icon) {
    if (dot._icon !== t.icon) { dot.textContent = ''; const im = document.createElement('img'); im.src = t.icon; im.alt = ''; dot.append(im); dot._icon = t.icon; }
    dot.classList.add('hasimg');
  } else { dot._icon = null; dot.classList.remove('hasimg'); if (dot.querySelector('svg,img')) dot.textContent = ''; dot.textContent = t.url ? hostLetter(t) : 'g'; }
  el.querySelector('.title').textContent = t.title;
  const audio = el.querySelector('.audio');
  audio.hidden = !(t.audible || t.muted);
  const icon = t.muted ? MUTED : SPEAKER;
  if (!audio.hidden && audio._icon !== icon) { audio.innerHTML = icon; audio._icon = icon; }
}

// ---------------------------------------------------------- folder elements
function createFolderEl(g) {
  const el = document.createElement('div');
  el._kind = 'group';
  el._renaming = renaming === g.id;
  el.className = 'folder';
  el.setAttribute('role', 'button');
  if (el._renaming) {
    el.innerHTML = FOLDER_OPEN;
    const input = document.createElement('input');
    input.value = g.name;
    input.placeholder = 'Folder name';
    input.maxLength = 40;
    const done = (save) => {
      if (renaming !== g.id) return;
      renaming = null;
      if (save) goat.send('group-rename', g.id, input.value.trim());
      render();
    };
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') done(true);
      if (e.key === 'Escape') done(false);
      e.stopPropagation();
    });
    input.addEventListener('blur', () => done(true));
    el.append(input);
    setTimeout(() => { input.focus(); input.select(); }, 0);
  } else {
    el.draggable = true;
    el.tabIndex = 0;
    el.innerHTML = '<span class="ico"></span><span class="name"></span><span class="count" hidden></span>';
    el.addEventListener('click', () => goat.send('group-toggle', el._data.id));
    el.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); goat.send('group-toggle', el._data.id); } });
    el.addEventListener('dblclick', (e) => { e.stopPropagation(); renaming = el._data.id; render(); });
    el.addEventListener('contextmenu', (e) => { e.preventDefault(); goat.send('group-menu', el._data.id); });
  }
  wireDrag(el);
  return el;
}

function updateFolderEl(el, g) {
  el._data = g;
  for (const c of [...el.classList]) if (c.startsWith('c-')) el.classList.remove(c);
  el.classList.add(`c-${g.color}`);
  el.classList.toggle('collapsed', !!g.collapsed);
  if (el._renaming) return;
  const n = state.tabs.filter((t) => t.groupId === g.id).length;
  const name = g.name || 'Folder';
  el.setAttribute('aria-expanded', String(!g.collapsed));
  el.setAttribute('aria-label', `${name} folder, ${n} tabs`);
  el.title = `${name} · ${n} tab${n === 1 ? '' : 's'}\nClick to ${g.collapsed ? 'open' : 'close'}, double-click to rename, right-click for options`;
  const ico = el.querySelector('.ico');
  const want = g.collapsed ? FOLDER_CLOSED : FOLDER_OPEN;
  if (ico._icon !== want) { ico.innerHTML = want; ico._icon = want; }
  el.querySelector('.name').textContent = name;
  const count = el.querySelector('.count');
  count.hidden = !g.collapsed;
  count.textContent = String(n);
}

// ------------------------------------------------------------------- render
function renderTabs() {
  const motion = motionOn() && !firstRender;
  const before = new Map();
  if (motion) for (const [k, e] of els) if (e.isConnected) before.set(k, e.getBoundingClientRect().left);

  const items = [];
  let prevGroup = null;
  for (const t of state.tabs) {
    if (t.groupId && t.groupId !== prevGroup) {
      const g = groupOf(t.groupId);
      if (g) items.push({ key: `g${g.id}`, kind: 'group', data: g });
    }
    prevGroup = t.groupId;
    const g = t.groupId ? groupOf(t.groupId) : null;
    if (!(g && g.collapsed)) items.push({ key: `t${t.id}`, kind: 'tab', data: t });
  }

  const keep = new Set(items.map((i) => i.key));
  for (const [k, e] of els) {
    if (!keep.has(k)) { els.delete(k); e.remove(); }
  }

  let cursor = tabsEl.firstChild;
  for (const item of items) {
    let el = els.get(item.key);
    if (el && item.kind === 'group' && el._renaming !== (renaming === item.data.id)) { el.remove(); el = null; }
    if (!el) {
      el = item.kind === 'tab' ? createTabEl() : createFolderEl(item.data);
      if (motion) {
        el.classList.add('enter');
        el.addEventListener('animationend', () => el.classList.remove('enter'), { once: true });
      }
      els.set(item.key, el);
    }
    if (item.kind === 'tab') updateTabEl(el, item.data); else updateFolderEl(el, item.data);
    if (el !== cursor) tabsEl.insertBefore(el, cursor);
    else cursor = cursor.nextSibling;
  }

  // FLIP: slide moved items from their old spot to the new one.
  if (motion) {
    for (const item of items) {
      const el = els.get(item.key);
      const old = before.get(item.key);
      if (old == null || el.classList.contains('dragging')) continue;
      const dx = old - el.getBoundingClientRect().left;
      if (Math.abs(dx) < 1) continue;
      el.style.transition = 'none';
      el.style.transform = `translateX(${dx}px)`;
      requestAnimationFrame(() => requestAnimationFrame(() => { el.style.transition = ''; el.style.transform = ''; }));
    }
  }
  firstRender = false;
}

function renderNav() {
  const t = active();
  $('back').disabled = !t || !t.canBack;
  $('forward').disabled = !t || !t.canForward;
  $('reload').classList.toggle('loading', !!(t && t.loading));
  $('reload').title = t && t.loading ? 'Stop' : 'Reload (Ctrl+R)';
  $('progress').classList.toggle('on', !!(t && t.loading));
  if (!editing) address.value = t ? t.url : '';
  document.documentElement.classList.toggle('in-private', !!(t && t.private));

  const s = t ? t.security : 'internal';
  if (sec.className !== s) {
    sec.className = s;
    if (s === 'secure') { sec.innerHTML = LOCK; sec.title = 'Encrypted connection (HTTPS)'; }
    else if (s === 'insecure') { sec.innerHTML = WARN + 'Not secure'; sec.title = 'This page is not encrypted.'; }
    else { sec.innerHTML = ''; sec.title = ''; }
  }

  document.documentElement.dataset.window = state.windowMode || 'normal';
  const pb = $('profilebtn');
  const prof = state.profile;
  pb.hidden = !(prof && prof.many);
  if (!pb.hidden) {
    pb.style.setProperty('--pc', prof.color);
    $('profileinitial').textContent = (prof.name || '?').trim().charAt(0).toUpperCase();
    pb.title = `Profile: ${prof.name}. Click to switch profiles.`;
    pb.setAttribute('aria-label', pb.title);
  }
  $('settings').classList.toggle('has-update', !!state.updateReady);
  $('settings').title = state.updateReady ? 'Menu · an update is available' : 'Menu';
  const tr = $('trbtn');
  const tri = t && t.translate;
  tr.hidden = !tri;
  if (tri) {
    tr.className = 'trbtn ' + tri.state;
    tr.title = {
      offer: `This page is in ${tri.fromName || 'another language'}. Click to translate to ${tri.to}. Right-click for options.`,
      working: 'Translating… Click to show the original.',
      done: `Translated${tri.fromName ? ' from ' + tri.fromName : ''} to ${tri.to}. Click to show the original.`,
      error: `Couldn't translate: ${tri.error}. Click to try again.`,
    }[tri.state] || 'Translate';
    tr.setAttribute('aria-pressed', String(tri.state === 'done'));
  }
  const torBtn = $('torbtn');
  torBtn.hidden = !state.torButton;
  if (!torBtn.hidden) {
    const ts = state.tor || 'off';
    torBtn.className = ts;
    torBtn.setAttribute('aria-pressed', String(ts !== 'off'));
    $('torlabel').textContent = ts === 'on' ? 'Tor' : ts === 'off' ? 'Tor' : ts === 'error' ? 'Tor !' : 'Tor…';
    torBtn.title = {
      off: 'Tor is off. Click to route everything through Tor. Right-click for options.',
      on: 'Connected through Tor: sites see a Tor IP, not yours. Click to turn Tor off. Right-click for options.',
      downloading: 'Downloading Tor… Pages wait until Tor is connected. Click to cancel.',
      starting: 'Connecting to Tor… Pages wait until Tor is connected. Click to cancel.',
      error: "Tor isn't working, so pages won't load (nothing leaks). Click to turn Tor off; right-click for settings.",
    }[ts] || 'Tor';
  }

  const shield = $('shield');
  const isWeb = t && /^https?:/.test(t.url);
  let cls = state.shield;
  let label;
  let tip;
  if (state.shield === 'on' && t && t.allowlisted) {
    cls = 'paused'; label = 'off'; tip = 'Ad blocker paused on this site. Click for options.';
  } else if (state.shield === 'on') {
    label = String(t ? t.blocked : 0);
    tip = `${label} ads and trackers blocked on this page.` + (isWeb ? ' Click to change blocking.' : ' Click for ad blocker options.');
  } else if (state.shield === 'loading') { label = '…'; tip = 'Loading blocklists…'; }
  else if (state.shield === 'disabled') { label = 'off'; tip = 'Ad blocker is turned off in Settings.'; }
  else { label = '!'; tip = 'Ad blocker could not load its lists.'; }
  shield.className = cls;
  const blocked = $('blocked');
  const n = t ? t.blocked : 0;
  if (cls === 'on' && n > lastBlocked && motionOn()) {
    blocked.classList.remove('bump');
    void blocked.offsetWidth;
    blocked.classList.add('bump');
  }
  lastBlocked = n;
  blocked.textContent = label;
  shield.title = tip;
}

function render() { renderTabs(); renderNav(); }

goat.on('state', (s) => { state = s; render(); });
goat.on('rename-group', (gid) => { renaming = gid; render(); });
root.dataset.platform = goat.platform;
goat.on('appearance', (a) => {
  root.dataset.theme = a.theme;
  root.dataset.motion = a.motion;
  root.dataset.gradients = a.gradients ? 'on' : 'off';
  root.removeAttribute('style');
  if (a.vars) for (const [k, v] of Object.entries(a.vars)) root.style.setProperty(k, v);
});

// --------------------------------------------------------------- tab strip
const strip = $('tabstrip');
tabsEl.addEventListener('dblclick', (e) => { if (e.target === tabsEl) goat.send('new-tab'); });
tabsEl.addEventListener('wheel', (e) => { tabsEl.scrollLeft += e.deltaY + e.deltaX; }, { passive: true });

// ----------------------------------------------------------------- address
address.addEventListener('focus', () => { address.select(); goat.send('preconnect'); });
address.addEventListener('input', () => { editing = true; });
address.addEventListener('blur', () => { editing = false; renderNav(); });
address.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') { editing = false; renderNav(); goat.send('focus-page'); }
});
$('addressform').addEventListener('submit', (e) => {
  e.preventDefault();
  const v = address.value.trim();
  if (!v) return;
  editing = false;
  address.blur();
  goat.send('navigate', v);
});
goat.on('focus-address', () => { address.focus(); address.select(); });

// ----------------------------------------------------------------- buttons
$('newtab').addEventListener('click', () => goat.send('new-tab'));
$('newtab').addEventListener('contextmenu', (e) => { e.preventDefault(); goat.send('new-tab-menu', e.clientX, e.clientY); });
$('newtab').title = 'New tab (Ctrl+T) · right-click for a private tab';
$('back').addEventListener('click', () => goat.send('back'));
$('forward').addEventListener('click', () => goat.send('forward'));
$('reload').addEventListener('click', () => {
  const t = active();
  goat.send(t && t.loading ? 'stop' : 'reload');
});
$('torbtn').addEventListener('click', () => goat.send('tor-toggle'));
$('profilebtn').addEventListener('click', () => goat.send('profile-menu'));
$('torbtn').addEventListener('contextmenu', (e) => { e.preventDefault(); goat.send('tor-menu'); });
$('trbtn').addEventListener('click', () => goat.send('translate'));
$('trbtn').addEventListener('contextmenu', (e) => { e.preventDefault(); goat.send('translate-menu'); });
$('shield').addEventListener('click', () => {
  const r = $('shield').getBoundingClientRect();
  goat.send('shield', r.right);
});
$('settings').addEventListener('click', () => {
  const r = $('settings').getBoundingClientRect();
  goat.send('app-menu', { x: r.left, y: r.top, width: r.width, height: r.height });
});

// -------------------------------------------------------------------- find
function openFind() {
  findbar.hidden = false;
  findinput.focus();
  findinput.select();
  if (findinput.value) goat.send('find', findinput.value, true, false);
}
function closeFind() {
  if (findbar.hidden) return;
  findbar.hidden = true;
  $('findcount').textContent = '';
  goat.send('find-stop');
}
goat.on('find-open', openFind);
goat.on('find-close', () => { if (!findbar.hidden) { findbar.hidden = true; $('findcount').textContent = ''; } });
goat.on('find-result', ({ active: a, total }) => {
  $('findcount').textContent = findinput.value ? `${a}/${total}` : '';
});
findinput.addEventListener('input', () => goat.send('find', findinput.value, true, false));
findinput.addEventListener('keydown', (e) => {
  if (e.key === 'Enter') { e.preventDefault(); goat.send('find', findinput.value, !e.shiftKey, true); }
  if (e.key === 'Escape') closeFind();
});
$('findnext').addEventListener('click', () => goat.send('find', findinput.value, true, true));
$('findprev').addEventListener('click', () => goat.send('find', findinput.value, false, true));
$('findclose').addEventListener('click', closeFind);

// ------------------------------------------------------------------- toast
let toastTimer = null;
goat.on('toast', (text) => {
  const el = $('toast');
  el.hidden = true;
  void el.offsetWidth; // restart the entrance animation
  el.textContent = text;
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { el.hidden = true; }, 4000);
});

// --------------------------------------------------------------- bookmarks
const bmBar = $('bookmarks');
const bmItems = $('bmitems');
const bmMore = $('bmmore');
let bmRenaming = null;
let bmKnown = null; // ids seen last render, for entrance animation
const FOLDER_ICON = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M1.8 4.2c0-.6.4-1 1-1h3.4l1.5 1.6h5.5c.6 0 1 .4 1 1v6.6c0 .6-.4 1-1 1H2.8c-.6 0-1-.4-1-1z" fill="currentColor" fill-opacity=".25"/></svg>';

// First letter of the bookmark's name (its site if the name has no letters).
function bmLetter(url, title) {
  const m = /[\p{L}\p{N}]/u.exec(title || '');
  if (m) return m[0].toUpperCase();
  try { return new URL(url).hostname.replace(/^www\./, '')[0] || '•'; } catch { return '•'; }
}

function bmZone(el, b, e) {
  if (!drag) return null;
  if (drag.kind === 'bm' && drag.id === b.id) return null;
  const r = el.getBoundingClientRect();
  const x = (e.clientX - r.left) / r.width;
  if (b.type === 'folder' && !(drag.kind === 'bm' && drag.isFolder)) {
    if (x > 0.25 && x < 0.75) return 'drop-into';
  }
  return x < 0.5 ? 'drop-before' : 'drop-after';
}
const BM_ZONES = ['drop-before', 'drop-after', 'drop-into'];
function clearBmZones() { bmBar.querySelectorAll('.drop-before,.drop-after,.drop-into').forEach((x) => x.classList.remove(...BM_ZONES)); bmBar.classList.remove('drop-end'); }

function bmEl(b, list) {
  const el = document.createElement('button');
  el.className = 'bm' + (b.type === 'folder' ? ' folder' : '');
  el.dataset.id = b.id;
  el.title = b.type === 'folder' ? `${b.title} (${b.children.length})` : `${b.title}\n${b.url}`;
  el.draggable = bmRenaming !== b.id;
  const ic = document.createElement('span');
  ic.className = 'ic';
  if (b.type === 'folder') ic.innerHTML = FOLDER_ICON;
  else if (b.icon) { const im = document.createElement('img'); im.src = b.icon; im.alt = ''; ic.append(im); ic.classList.add('hasimg'); }
  else ic.textContent = bmLetter(b.url, b.title);
  el.append(ic);
  if (bmRenaming === b.id) {
    const input = document.createElement('input');
    input.value = b.title;
    input.maxLength = 120;
    const done = (save) => {
      if (bmRenaming !== b.id) return;
      bmRenaming = null;
      if (save && input.value.trim()) goat.send('bm-rename', b.id, input.value.trim());
      renderBookmarks();
    };
    input.addEventListener('keydown', (e) => { e.stopPropagation(); if (e.key === 'Enter') done(true); if (e.key === 'Escape') done(false); });
    input.addEventListener('blur', () => done(true));
    input.addEventListener('click', (e) => e.stopPropagation());
    el.append(input);
    setTimeout(() => { input.focus(); input.select(); }, 0);
  } else {
    const t = document.createElement('span');
    t.className = 't';
    t.textContent = b.title || (b.type === 'folder' ? 'Folder' : b.url);
    el.append(t);
  }
  const anchor = () => { const r = el.getBoundingClientRect(); return [r.left, r.bottom + 2]; };
  el.addEventListener('click', (e) => {
    if (bmRenaming === b.id) return;
    if (b.type === 'folder') goat.send('bm-folder', b.id, ...anchor());
    else goat.send('bm-open', b.id, e.ctrlKey || e.metaKey ? 'background' : e.shiftKey ? 'new' : 'current');
  });
  if (b.type === 'link') el.addEventListener('mouseenter', () => goat.send('preconnect', b.url), { once: true });
  el.addEventListener('auxclick', (e) => { if (e.button === 1 && b.type === 'link') { e.preventDefault(); goat.send('bm-open', b.id, 'background'); } });
  el.addEventListener('contextmenu', (e) => { e.preventDefault(); e.stopPropagation(); goat.send('bm-menu', b.id, e.clientX, e.clientY); });
  el.addEventListener('dragstart', (e) => {
    drag = { kind: 'bm', id: b.id, isFolder: b.type === 'folder' };
    e.dataTransfer.effectAllowed = 'move';
    e.dataTransfer.setData('text/plain', b.url || b.title);
    requestAnimationFrame(() => el.classList.add('dragging'));
  });
  el.addEventListener('dragend', () => { drag = null; el.classList.remove('dragging'); clearBmZones(); });
  el.addEventListener('dragover', (e) => {
    if (!drag || drag.kind === 'group') return;
    const zone = bmZone(el, b, e);
    if (!zone) return;
    e.preventDefault();
    e.stopPropagation();
    bmBar.classList.remove('drop-end');
    el.classList.remove(...BM_ZONES);
    el.classList.add(zone);
  });
  el.addEventListener('dragleave', (e) => { if (!el.contains(e.relatedTarget)) el.classList.remove(...BM_ZONES); });
  el.addEventListener('drop', (e) => {
    if (!drag) return;
    e.preventDefault();
    e.stopPropagation();
    const zone = BM_ZONES.find((z) => el.classList.contains(z));
    clearBmZones();
    const i = list.indexOf(b);
    const next = list[i + 1];
    const beforeId = zone === 'drop-before' ? b.id : zone === 'drop-after' ? (next ? next.id : null) : null;
    const folderId = zone === 'drop-into' ? b.id : null;
    if (drag.kind === 'bm') goat.send('bm-move', drag.id, beforeId, folderId);
    else if (drag.kind === 'tab') goat.send('bm-add-tab', drag.id, beforeId, folderId);
    drag = null;
  });
  if (bmKnown && !bmKnown.has(b.id) && motionOn()) el.classList.add('enter');
  return el;
}

bmBar.addEventListener('dragover', (e) => {
  if (!drag || drag.kind === 'group') return;
  e.preventDefault();
  clearBmZones();
  bmBar.classList.add('drop-end');
});
bmBar.addEventListener('dragleave', (e) => { if (!bmBar.contains(e.relatedTarget)) bmBar.classList.remove('drop-end'); });
bmBar.addEventListener('drop', (e) => {
  if (!drag) return;
  e.preventDefault();
  clearBmZones();
  if (drag.kind === 'bm') goat.send('bm-move', drag.id, null, null);
  else if (drag.kind === 'tab') goat.send('bm-add-tab', drag.id, null, null);
  drag = null;
});
bmBar.addEventListener('contextmenu', (e) => { e.preventDefault(); goat.send('bm-bar-menu', e.clientX, e.clientY); });

function renderBookmarks() {
  const on = !!state.bookmarksBar;
  root.classList.toggle('bm-on', on);
  const star = $('star');
  const wasOn = star.classList.contains('on');
  star.classList.toggle('on', !!state.bookmarked);
  star.setAttribute('aria-pressed', String(!!state.bookmarked));
  star.title = state.bookmarked ? 'Remove bookmark (Ctrl+D)' : 'Bookmark this page (Ctrl+D)';
  if (!wasOn && state.bookmarked && motionOn()) { star.classList.remove('pop'); void star.offsetWidth; star.classList.add('pop'); }
  if (!on) return;
  const list = state.bookmarks || [];
  bmItems.textContent = '';
  for (const b of list) bmItems.append(bmEl(b, list));
  bmKnown = new Set(list.map((b) => b.id));
  $('bmempty').hidden = list.length > 0;
  // Overflow: hide what doesn't fit and list it under ».
  bmMore.hidden = true;
  const hidden = [];
  const max = bmBar.clientWidth - 16;
  const kids = [...bmItems.children];
  let used = 0;
  kids.forEach((k) => { k.style.display = ''; });
  for (let i = 0; i < kids.length; i++) {
    used += kids[i].offsetWidth + 2;
    const reserve = i < kids.length - 1 ? 40 : 0;
    if (used + reserve > max) {
      for (let j = i; j < kids.length; j++) { kids[j].style.display = 'none'; hidden.push(kids[j].dataset.id); }
      break;
    }
  }
  bmMore.hidden = hidden.length === 0;
  bmMore._ids = hidden;
}
bmMore.addEventListener('click', () => {
  const r = bmMore.getBoundingClientRect();
  goat.send('bm-overflow', bmMore._ids || [], r.left, r.bottom + 2);
});
window.addEventListener('resize', () => renderBookmarks());
goat.on('bm-rename-start', (id) => { bmRenaming = id; renderBookmarks(); });
goat.on('state', () => renderBookmarks());

$('star').addEventListener('click', (e) => { e.preventDefault(); goat.send('bm-star'); });
$('star').addEventListener('mousedown', (e) => e.preventDefault()); // keep the address bar as-is

// Clicks in the strip above a tab (e.g. mouse pinned to the top screen edge) still hit that tab.
tabsEl.addEventListener('mousedown', (e) => {
  if (e.target !== tabsEl || e.button !== 0) return;
  const hit = [...tabsEl.children].find((c) => { const r = c.getBoundingClientRect(); return e.clientX >= r.left && e.clientX <= r.right; });
  if (!hit) return;
  if (hit._kind === 'tab') goat.send('activate', hit._data.id);
  else if (hit._kind === 'group') goat.send('group-toggle', hit._data.id);
});


