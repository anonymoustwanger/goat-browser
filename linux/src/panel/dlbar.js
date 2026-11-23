'use strict';
// Goat's download bar (Linux): one card per download, newest first.
const ICON = {
  pause: '<path d="M5.5 3.5v9M10.5 3.5v9"/>',
  resume: '<path d="M5 3.2v9.6L12.6 8z"/>',
  retry: '<path d="M13 8a5 5 0 1 1-1.5-3.6M13 2.5v3h-3"/>',
  cancel: '<path d="M4 4l8 8M12 4l-8 8"/>',
  remove: '<path d="M4 4l8 8M12 4l-8 8"/>',
  folder: '<path d="M2 4.5a1 1 0 0 1 1-1h3l1.5 1.5H13a1 1 0 0 1 1 1V12a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z"/>',
};
const TIP = { pause: 'Pause', resume: 'Resume', retry: 'Try again', cancel: 'Cancel', remove: 'Remove from bar', folder: 'Show in folder' };
const GLYPH = {
  progressing: '<path d="M8 4v6M5.8 8 8 10.2 10.2 8" transform="translate(3 3)"/>',
  paused: '<path d="M6 5v6M10 5v6" transform="translate(3 3)"/>',
  completed: '<path d="m3.5 8.5 3 3 6-7" transform="translate(3 3)"/>',
  bad: '<path d="M8 3 14 13H2zM8 7v3M8 11.5v.1" transform="translate(3 3)"/>',
};
const size = (b) => (b >= 1e9 ? (b / 1e9).toFixed(2) + ' GB' : b >= 1e6 ? (b / 1e6).toFixed(1) + ' MB' : Math.max(1, Math.round(b / 1e3)) + ' KB');
function left(s) {
  if (s >= 3600) return `${Math.floor(s / 3600)} h ${Math.floor((s % 3600) / 60)} min left`;
  if (s >= 60) return `${Math.round(s / 60)} min left`;
  return `${Math.round(s)} s left`;
}
function status(d) {
  if (d.state === 'progressing' && !d.paused) {
    let t = d.total > 0 ? `${size(d.received)} of ${size(d.total)}` : size(d.received);
    if (d.speed > 1) {
      t += ` · ${size(d.speed)}/s`;
      if (d.total > 0) t += ` · ${left((d.total - d.received) / d.speed)}`;
    }
    return t;
  }
  if (d.paused) return `Paused · ${size(d.received)}${d.total > 0 ? ' of ' + size(d.total) : ''}`;
  if (d.state === 'completed') return `${size(d.total || d.received)} · Done`;
  if (d.state === 'cancelled') return 'Cancelled';
  return 'Download failed — connection lost';
}
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
const box = document.getElementById('items');
const seen = new Set();
window.goatDl.onItems((items) => {
  box.innerHTML = '';
  for (const d of items) {
    const running = d.state === 'progressing';
    const cls = d.paused ? 'paused' : running ? 'running' : d.state === 'completed' ? 'done' : d.state === 'cancelled' ? 'cancelled' : 'failed';
    const btns = running ? [d.paused ? 'resume' : 'pause', 'cancel']
      : d.state === 'completed' ? ['folder', 'remove']
      : d.state === 'interrupted' && d.canResume ? ['retry', 'remove'] : ['remove'];
    const frac = d.total > 0 ? Math.min(1, d.received / d.total) : 0.25;
    const C = 2 * Math.PI * 10;
    const ring = running ? `<circle class="ring-bg" cx="11" cy="11" r="10"/><circle class="ring" cx="11" cy="11" r="10" stroke-dasharray="${C}" stroke-dashoffset="${C * (1 - frac)}"/>` : '';
    const glyph = d.paused ? GLYPH.paused : running ? GLYPH.progressing : d.state === 'completed' ? GLYPH.completed : GLYPH.bad;
    const el = document.createElement('div');
    el.className = `card ${cls}`;
    if (seen.has(d.id)) el.style.animation = 'none';
    seen.add(d.id);
    el.innerHTML = `<div class="ic"><svg viewBox="0 0 22 22">${ring}<g class="glyph">${glyph}</g></svg></div>` +
      `<div class="txt"><div class="name">${esc(d.name)}</div><div class="status">${esc(status(d))}</div></div>` +
      `<div class="btns">${btns.map((b) => `<button data-id="${d.id}" data-act="${b}" title="${TIP[b]}"><svg viewBox="0 0 16 16">${ICON[b]}</svg></button>`).join('')}</div>`;
    if (d.state === 'completed') el.dataset.open = d.id;
    box.appendChild(el);
  }
});
document.addEventListener('click', (e) => {
  const b = e.target.closest('button');
  if (b) { window.goatDl.run(b.dataset.id || 0, b.dataset.act); return; }
  const c = e.target.closest('[data-open]');
  if (c) window.goatDl.run(c.dataset.open, 'open');
});
