'use strict';
const $ = (id) => document.getElementById(id);
const api = window.goatPanel;
const LEVELS = {
  ads: { desc: 'Lightest. Removes ads, lets trackers through.', has: ['Ads'], not: ['Trackers', 'Cookie banners & popups'] },
  standard: { desc: 'Removes ads and trackers. Recommended.', has: ['Ads', 'Trackers'], not: ['Cookie banners & popups'] },
  strict: { desc: 'Also hides cookie banners and annoyances. May break some sites.', has: ['Ads', 'Trackers', 'Cookie banners & popups'], not: [] },
};
let state = null;
let busy = false;
let shownBlocked = 0;

const fmt = (n) => Number(n || 0).toLocaleString();

function animateCount(el, from, to) {
  const reduce = document.documentElement.dataset.motion === 'off' ||
    (document.documentElement.dataset.motion === 'system' && matchMedia('(prefers-reduced-motion: reduce)').matches);
  if (reduce || to <= from || to - from > 500) { el.textContent = fmt(to); return; }
  const start = performance.now();
  const step = (t) => {
    const k = Math.min(1, (t - start) / 260);
    el.textContent = fmt(Math.round(from + (to - from) * (1 - (1 - k) ** 3)));
    if (k < 1) requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
}

function render(s) {
  state = s;
  const on = s.enabled;
  $('master').checked = on;
  document.body.classList.toggle('off', !on);
  document.body.classList.toggle('paused', on && s.allowed);
  const status = !on ? 'Off everywhere'
    : s.state === 'loading' ? 'Loading blocklists…'
      : s.state === 'error' ? "Couldn't download blocklists"
        : s.allowed ? `Paused on ${s.site}` : 'Protecting you';
  $('status').textContent = status;

  // site
  $('sitebox').classList.toggle('nosite', !s.site);
  $('sitename').textContent = s.site || 'Not a website';
  const blocked = on && !s.allowed ? s.blocked : 0;
  animateCount($('blocked'), shownBlocked, blocked);
  shownBlocked = blocked;
  $('siterow').hidden = !s.site;
  $('sitetoggle').checked = !s.allowed;
  $('sitetoggle').disabled = !on || busy;
  $('sitelabel').textContent = s.site ? `Block on ${s.site}` : 'Block on this site';

  // level
  const seg = document.querySelector('.segments');
  const idx = ['ads', 'standard', 'strict'].indexOf(s.level);
  seg.style.setProperty('--i', String(Math.max(0, idx)));
  for (const b of seg.querySelectorAll('button')) {
    const sel = b.dataset.level === s.level;
    b.classList.toggle('on', sel);
    b.setAttribute('aria-checked', String(sel));
    b.disabled = !on || busy;
  }
  const L = LEVELS[s.level] || LEVELS.standard;
  $('leveldesc').textContent = L.desc;
  const ul = $('levelinfo');
  ul.textContent = '';
  for (const x of L.has) { const li = document.createElement('li'); li.className = 'yes'; li.textContent = x; ul.append(li); }
  for (const x of L.not) { const li = document.createElement('li'); li.className = 'no'; li.textContent = x; ul.append(li); }

  $('total').textContent = fmt(s.total);
  $('allowlink').textContent = `Allowed sites (${s.allowlistCount})`;
  $('filterslink').textContent = s.rulesCount ? `Custom filters (${s.rulesCount})` : 'Custom filters';
  reportSize();
}

async function run(fn) {
  if (busy) return;
  busy = true;
  document.body.classList.add('busy');
  if (state) render(state);
  try { const s = await fn(); if (s) state = s; } finally {
    busy = false;
    document.body.classList.remove('busy');
    if (state) render(state);
  }
}

$('master').addEventListener('change', () => run(() => api.set({ enabled: $('master').checked })));
$('sitetoggle').addEventListener('change', () => run(() => api.toggleSite()));
document.querySelectorAll('.segments button').forEach((b) => b.addEventListener('click', () => {
  if (!state || state.level === b.dataset.level) return;
  state = { ...state, level: b.dataset.level };
  render(state);
  run(() => api.set({ level: b.dataset.level }));
}));
$('allowlink').addEventListener('click', () => api.openSettings('allow'));
$('filterslink').addEventListener('click', () => api.openSettings('filters'));

function reportSize() { api.size($('card').getBoundingClientRect().height + 24); }
new ResizeObserver(reportSize).observe($('card'));

api.onState(render);
api.onOpen(() => {
  const card = $('card');
  card.classList.remove('opening');
  void card.offsetWidth;
  card.classList.add('opening');
  $('master').focus({ preventScroll: true });
});
