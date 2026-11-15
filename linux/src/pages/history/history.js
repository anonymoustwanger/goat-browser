'use strict';
const api = window.goat;
const $ = (id) => document.getElementById(id);
let items = [];
let more = false;
const selected = new Set();

function el(tag, props = {}, ...kids) {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === 'class') n.className = v;
    else if (k.startsWith('on')) n.addEventListener(k.slice(2), v);
    else n.setAttribute(k, v);
  }
  for (const c of kids) if (c != null) n.append(c);
  return n;
}
const host = (u) => { try { return new URL(u).hostname.replace(/^www\./, ''); } catch { return u; } };
const hue = (str) => { let h = 0; for (const c of str) h = (h * 31 + c.charCodeAt(0)) % 360; return h; };
function dayLabel(t) {
  const d = new Date(t);
  const today = new Date(); today.setHours(0, 0, 0, 0);
  const diff = Math.round((today - new Date(d.getFullYear(), d.getMonth(), d.getDate())) / 86400000);
  const date = d.toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric', year: d.getFullYear() === today.getFullYear() ? undefined : 'numeric' });
  return diff === 0 ? `Today · ${date}` : diff === 1 ? `Yesterday · ${date}` : date;
}

function render() {
  const list = $('list');
  list.textContent = '';
  let currentDay = null;
  let group = null;
  for (const h of items) {
    const day = new Date(h.t).toDateString();
    if (day !== currentDay) {
      currentDay = day;
      group = el('ul', { class: 'hday' });
      list.append(el('section', {}, el('h2', {}, dayLabel(h.t)), group));
    }
    const hn = host(h.url);
    const check = el('input', { type: 'checkbox', 'aria-label': `Select ${h.title || hn}` });
    check.checked = selected.has(h.id);
    check.addEventListener('change', () => { if (check.checked) selected.add(h.id); else selected.delete(h.id); updateSel(); });
    const ico = el('span', { class: 'hico' }, (hn[0] || '•').toUpperCase());
    ico.style.setProperty('--h', String(hue(hn)));
    const link = el('a', { href: h.url, class: 'hlink', title: h.url },
      ico, el('span', { class: 'htitle' }, h.title || hn), el('span', { class: 'hhost' }, hn));
    link.addEventListener('click', (e) => { e.preventDefault(); api.open(h.url, e.ctrlKey || e.metaKey || e.button === 1 ? 'background' : 'current'); });
    link.addEventListener('auxclick', (e) => { if (e.button === 1) { e.preventDefault(); api.open(h.url, 'background'); } });
    const time = el('time', { class: 'htime' }, new Date(h.t).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' }));
    const del = el('button', { class: 'x', title: 'Remove from history', 'aria-label': 'Remove from history', onclick: async () => {
      await api.remove([h.id]);
      items = items.filter((x) => x.id !== h.id);
      selected.delete(h.id);
      render();
    } }, '×');
    group.append(el('li', {}, check, time, link, del));
  }
  $('more').hidden = !more;
  updateSel();
}
function updateSel() {
  $('delsel').hidden = selected.size === 0;
  $('delsel').textContent = `Delete selected (${selected.size})`;
}

async function load(reset = true) {
  const q = $('q').value.trim();
  const before = reset || !items.length ? Infinity : items[items.length - 1].t;
  const r = await api.query({ q, before, limit: 200 });
  items = reset ? r.items : items.concat(r.items);
  more = r.more;
  const empty = $('empty');
  empty.hidden = items.length > 0;
  empty.textContent = q ? 'No matches.' : 'No history yet. Pages you visit in normal tabs show up here (private tabs never do).';
  $('count').textContent = q ? `${items.length}${more ? '+' : ''} matches` : `${r.total.toLocaleString()} visits saved`;
  render();
}

let t = 0;
$('q').addEventListener('input', () => { clearTimeout(t); t = setTimeout(() => load(true), 150); });
$('more').addEventListener('click', () => load(false));
$('delsel').addEventListener('click', async () => {
  await api.remove([...selected]);
  selected.clear();
  load(true);
});
$('range').addEventListener('change', async (e) => {
  const v = e.target.value;
  e.target.value = '';
  if (!v) return;
  const label = e.target.querySelector(`option[value="${v}"]`).textContent.toLowerCase();
  if (!confirm(`Delete history from ${label}?`)) return;
  await api.clear(v);
  selected.clear();
  load(true);
});
$('keep').addEventListener('click', () => api.settings());
document.addEventListener('keydown', (e) => { if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'f') { e.preventDefault(); $('q').focus(); } });
load(true);
$('q').focus();
