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
    icon.textContent = (h[0] || '•').toUpperCase();
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
  if (b.type !== 'link') return a;
  // Wrapper so the remove button isn't nested inside the tile button.
  const wrap = document.createElement('div');
  wrap.className = 'tilewrap';
  const rm = document.createElement('button');
  rm.type = 'button';
  rm.className = 'tremove';
  rm.title = `Remove ${b.title || host(b.url)}`;
  rm.setAttribute('aria-label', rm.title);
  rm.textContent = '×';
  rm.addEventListener('click', (e) => { e.stopPropagation(); api.remove(b.id); });
  wrap.append(a, rm);
  return wrap;
}

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
function showAdd(on) {
  addBox.hidden = !on;
  document.getElementById('adderr').hidden = true;
  if (on) {
    openFolder = null;
    render(lastItems);
    addBox.hidden = false;
    document.getElementById('addurl').value = '';
    document.getElementById('addtitle').value = '';
    document.getElementById('addurl').focus();
  }
}
document.getElementById('addclose').addEventListener('click', () => showAdd(false));
addBox.addEventListener('keydown', (e) => { if (e.key === 'Escape') showAdd(false); });
addBox.addEventListener('submit', async (e) => {
  e.preventDefault();
  const r = await api.add(document.getElementById('addurl').value, document.getElementById('addtitle').value);
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
