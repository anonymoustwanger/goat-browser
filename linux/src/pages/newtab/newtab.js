'use strict';
const isPrivate = new URLSearchParams(location.search).has('private');
if (isPrivate) { document.body.classList.add('private'); document.getElementById('privnote').hidden = false; document.title = 'New private tab'; }
document.getElementById('search').addEventListener('submit', (e) => {
  e.preventDefault();
  const q = document.getElementById('q').value.trim();
  if (q && window.goat) window.goat.search(q);
});

// ------------------------------------------------------- bookmarks on home
const api = window.goat;
const tilesEl = document.getElementById('tiles');
const folderBox = document.getElementById('folderbox');
let openFolder = null;
const FOLDER_SVG = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3 7.2c0-.9.7-1.6 1.6-1.6h4.8l2.1 2.3h7.9c.9 0 1.6.7 1.6 1.6v8.6c0 .9-.7 1.6-1.6 1.6H4.6c-.9 0-1.6-.7-1.6-1.6z"/></svg>';

function host(url) { try { return new URL(url).hostname.replace(/^www\./, ''); } catch { return url; } }
function hue(str) { let h = 0; for (const c of str) h = (h * 31 + c.charCodeAt(0)) % 360; return h; }

function tile(b, small) {
  const a = document.createElement('button');
  a.className = 'tile' + (b.type === 'folder' ? ' folder' : '');
  a.type = 'button';
  const icon = document.createElement('span');
  icon.className = 'ticon';
  if (b.type === 'folder') {
    icon.innerHTML = FOLDER_SVG;
  } else {
    const h = host(b.url);
    const m = /[\p{L}\p{N}]/u.exec(b.title || '');
    icon.textContent = (m ? m[0] : (h[0] || '•')).toUpperCase();
    icon.style.setProperty('--h', String(hue(h)));
  }
  const label = document.createElement('span');
  label.className = 'tlabel';
  label.textContent = b.title || (b.type === 'folder' ? 'Folder' : host(b.url));
  a.title = b.type === 'folder' ? `${b.title} · ${b.children.length} bookmarks` : `${b.title}\n${b.url}`;
  a.append(icon, label);
  if (b.type === 'folder') {
    const n = document.createElement('span');
    n.className = 'tcount';
    n.textContent = String(b.children.length);
    a.append(n);
    a.addEventListener('click', () => { openFolder = openFolder === b.id ? null : b.id; render(lastItems); });
    a.classList.toggle('open', openFolder === b.id);
  } else {
    a.addEventListener('click', (e) => api.open(b.id, e.ctrlKey || e.metaKey ? 'background' : 'current'));
    a.addEventListener('auxclick', (e) => { if (e.button === 1) { e.preventDefault(); api.open(b.id, 'background'); } });
  }
  if (small) a.classList.add('small');
  if (b.type === 'link') {
    a.title += '\nRight-click to edit or delete';
    a.addEventListener('contextmenu', (e) => { e.preventDefault(); showMenu(b, a, e.clientX, e.clientY); });
  }
  return a;
}

// Right-click a bookmark: edit its name and link, open it in a new tab, or delete it.
const menu = document.getElementById('tmenu');
document.body.append(menu); // fixed position must not be inside an animated (transformed) box
let menuFor = null;
function showMenu(b, el, x, y) {
  menuFor = { b, el };
  menu.hidden = false;
  menu.style.left = Math.min(x, innerWidth - menu.offsetWidth - 8) + 'px';
  menu.style.top = Math.min(y, innerHeight - menu.offsetHeight - 8) + 'px';
  menu.querySelector('button').focus({ preventScroll: true });
}
const hideMenu = () => { menu.hidden = true; menuFor = null; };
addEventListener('pointerdown', (e) => { if (!menu.hidden && !menu.contains(e.target)) hideMenu(); }, true);
addEventListener('keydown', (e) => { if (e.key === 'Escape') hideMenu(); });
document.addEventListener('contextmenu', (e) => { if (!e.defaultPrevented) hideMenu(); });
let editing = null; // bookmark id being edited, or null when adding
menu.addEventListener('click', (e) => {
  const btn = e.target.closest('button');
  if (!btn || !menuFor) return;
  const { b, el } = menuFor;
  hideMenu();
  if (btn.dataset.a === 'edit') {
    showAdd(true, b);
  } else if (btn.dataset.a === 'newtab') {
    api.open(b.id, 'background');
  } else if (btn.dataset.a === 'delete') {
    el.style.transition = 'opacity .18s ease, transform .18s ease';
    el.style.opacity = '0';
    el.style.transform = 'scale(.85)';
    setTimeout(() => api.remove(b.id), 170);
  }
});

function addTile() {
  const a = document.createElement('button');
  a.type = 'button';
  a.className = 'tile add';
  a.title = 'Add a bookmark';
  a.innerHTML = '<span class="ticon" aria-hidden="true">+</span><span class="tlabel">Add</span>';
  a.addEventListener('click', () => showAdd(true));
  return a;
}

const addBox = document.getElementById('addbox');
function showAdd(on, b) {
  addBox.hidden = !on;
  document.getElementById('adderr').hidden = true;
  editing = on && b ? b.id : null;
  document.getElementById('addhead').textContent = editing ? 'Edit bookmark' : 'Add bookmark';
  document.getElementById('addgo').textContent = editing ? 'Save' : 'Add';
  if (on) {
    openFolder = null;
    render(lastItems);
    addBox.hidden = false;
    document.getElementById('addurl').value = b ? b.url : '';
    document.getElementById('addtitle').value = b ? b.title : '';
    const f = document.getElementById(b ? 'addtitle' : 'addurl');
    f.focus();
    if (b) f.select();
  }
}
document.getElementById('addclose').addEventListener('click', () => showAdd(false));
addBox.addEventListener('keydown', (e) => { if (e.key === 'Escape') showAdd(false); });
addBox.addEventListener('submit', async (e) => {
  e.preventDefault();
  const url = document.getElementById('addurl').value, title = document.getElementById('addtitle').value;
  const r = editing ? await api.edit(editing, url, title) : await api.add(url, title);
  if (r && r.error) {
    const err = document.getElementById('adderr');
    err.textContent = r.error;
    err.hidden = false;
    document.getElementById('addurl').focus();
    return;
  }
  showAdd(false);
});

let lastItems = [];
function render(items) {
  lastItems = items || [];
  document.body.classList.toggle('has-bookmarks', lastItems.length > 0);
  tilesEl.textContent = '';
  for (const b of lastItems) tilesEl.append(tile(b));
  tilesEl.append(addTile());
  const f = lastItems.find((b) => b.id === openFolder && b.type === 'folder');
  folderBox.hidden = !f;
  if (f) {
    document.getElementById('foldername').textContent = f.title || 'Folder';
    const box = document.getElementById('foldertiles');
    box.textContent = '';
    if (!f.children.length) { const p = document.createElement('p'); p.className = 'empty'; p.textContent = 'This folder is empty.'; box.append(p); }
    for (const c of f.children) box.append(tile(c, true));
  }
}
document.getElementById('folderclose').addEventListener('click', () => { openFolder = null; render(lastItems); });

if (api && api.bookmarks) {
  api.bookmarks().then(render);
  api.onBookmarks(render);
}
