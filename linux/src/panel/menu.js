'use strict';
const api = window.goatMenu;
const ICONS = {
  plus: '<path d="M8 3v10M3 8h10"/>',
  user: '<circle cx="8" cy="5.5" r="2.7"/><path d="M2.8 14c.6-2.8 2.7-4.3 5.2-4.3s4.6 1.5 5.2 4.3"/>',
  window: '<rect x="2" y="3" width="12" height="10" rx="1.5"/><path d="M2 6h12"/>',
  onion: '<circle cx="8" cy="9" r="5"/><circle cx="8" cy="9" r="3"/><path d="M8 4c0-1.2.6-2 1.6-2.4"/>',
  mask: '<path d="M1.5 6.5c2-1.4 4.2-1.6 6.5-.4 2.3-1.2 4.5-1 6.5.4-.2 2.6-1.6 4.4-3.6 4.4-1.3 0-2.2-.8-2.9-1.9-.7 1.1-1.6 1.9-2.9 1.9-2 0-3.4-1.8-3.6-4.4z"/>',
  clock: '<circle cx="8" cy="8" r="6"/><path d="M8 4.8V8l2.2 1.6"/>',
  star: '<path d="m8 1.8 1.9 3.9 4.3.6-3.1 3 .7 4.3L8 11.6l-3.8 2 .7-4.3-3.1-3 4.3-.6z"/>',
  bar: '<rect x="2" y="3" width="12" height="10" rx="2"/><path d="M2 6.5h12"/>',
  download: '<path d="M8 2.5v8M4.8 7.5 8 10.7l3.2-3.2M3 13.5h10"/>',
  zoom: '<circle cx="7" cy="7" r="4.5"/><path d="m10.4 10.4 3.3 3.3"/>',
  expand: '<path d="M2.5 6V2.5H6M10 2.5h3.5V6M13.5 10v3.5H10M6 13.5H2.5V10"/>',
  search: '<circle cx="7" cy="7" r="4.5"/><path d="m10.4 10.4 3.3 3.3"/>',
  print: '<path d="M4.5 6V2.5h7V6M4.5 11.5h-2v-5h11v5h-2"/><rect x="4.5" y="9.5" width="7" height="4"/>',
  shield: '<path d="M8 1.8 2.8 3.7v4c0 3.2 2.2 5.4 5.2 6.5 3-1.1 5.2-3.3 5.2-6.5v-4z"/>',
  puzzle: '<path d="M6.2 2.5h1.6a1.3 1.3 0 0 1 2.6 0H12c.6 0 1 .4 1 1v1.9a1.3 1.3 0 0 1 0 2.6v2.5c0 .6-.4 1-1 1H9.6a1.3 1.3 0 0 0-2.6 0H4.5c-.6 0-1-.4-1-1V9.1a1.3 1.3 0 0 0 0-2.6V3.5c0-.6.4-1 1-1z"/>',
  trash: '<path d="M2.5 4.5h11M6.5 4.5V3h3v1.5M4 4.5l.7 9h6.6l.7-9"/>',
  cog: '<circle cx="8" cy="8" r="2"/><path d="M8.1 1.3h-.3a1.3 1.3 0 0 0-1.3 1.3v.1a1.3 1.3 0 0 1-.7 1.2l-.3.2a1.3 1.3 0 0 1-1.3 0l-.1-.1a1.3 1.3 0 0 0-1.8.5l-.1.3a1.3 1.3 0 0 0 .5 1.8l.1.1a1.3 1.3 0 0 1 .7 1.1v.3a1.3 1.3 0 0 1-.7 1.2l-.1.1a1.3 1.3 0 0 0-.5 1.8l.1.3a1.3 1.3 0 0 0 1.8.5l.1-.1a1.3 1.3 0 0 1 1.3 0l.3.2a1.3 1.3 0 0 1 .7 1.2v.1a1.3 1.3 0 0 0 1.3 1.3h.3a1.3 1.3 0 0 0 1.3-1.3v-.1a1.3 1.3 0 0 1 .7-1.2l.3-.2a1.3 1.3 0 0 1 1.3 0l.1.1a1.3 1.3 0 0 0 1.8-.5l.1-.3a1.3 1.3 0 0 0-.5-1.8l-.1-.1a1.3 1.3 0 0 1-.7-1.2v-.3a1.3 1.3 0 0 1 .7-1.2l.1-.1a1.3 1.3 0 0 0 .5-1.8l-.1-.3a1.3 1.3 0 0 0-1.8-.5l-.1.1a1.3 1.3 0 0 1-1.3 0l-.3-.2a1.3 1.3 0 0 1-.7-1.2v-.1a1.3 1.3 0 0 0-1.3-1.3z"/>',
  info: '<circle cx="8" cy="8" r="6"/><path d="M8 7.2v3.6M8 5.2v.2"/>',
  power: '<path d="M8 2v5.5M4.6 4.2a5 5 0 1 0 6.8 0"/>',
};
for (const i of document.querySelectorAll('i[data-icon]')) {
  i.innerHTML = `<svg viewBox="0 0 16 16" aria-hidden="true">${ICONS[i.dataset.icon] || ''}</svg>`;
}
for (const b of document.querySelectorAll('[data-do]')) {
  b.addEventListener('click', () => api.run(b.dataset.do));
}
const items = () => [...document.querySelectorAll('.menu button:not(:disabled)')];
document.addEventListener('keydown', (e) => {
  const list = items();
  const i = list.indexOf(document.activeElement);
  if (e.key === 'ArrowDown') { e.preventDefault(); list[(i + 1) % list.length].focus(); }
  if (e.key === 'ArrowUp') { e.preventDefault(); list[(i - 1 + list.length) % list.length].focus(); }
});

api.onState((s) => {
  document.getElementById('zoomval').textContent = `${s.zoom}%`;
  const bm = document.getElementById('bmbar');
  bm.classList.toggle('checked', s.bookmarksBar);
  bm.setAttribute('aria-checked', String(s.bookmarksBar));
  for (const b of document.querySelectorAll('[data-web]')) b.disabled = !s.isWeb;
  document.getElementById('ver').textContent = s.version ? `v${s.version}` : '';
  document.getElementById('profname').textContent = s.profile || '';
  document.getElementById('updateitem').hidden = !s.update;
  document.getElementById('updatelabel').textContent = s.update ? `Update to Goat ${s.update}` : 'Update Goat';
  report();
});
function report() { api.size(document.getElementById('card').getBoundingClientRect().height + 24); }
new ResizeObserver(report).observe(document.getElementById('card'));
api.onOpen(() => {
  const card = document.getElementById('card');
  card.classList.remove('opening');
  void card.offsetWidth;
  card.classList.add('opening');
  items()[0].focus({ preventScroll: true });
});
