'use strict';
const $ = (id) => document.getElementById(id);
const api = window.goatExt;
const PIN = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M9.6 1.9 14 6.3l-1.3.4-2.9 2.9-.4 3.1-1.1 1.1-2.2-2.2-3 3-.9-.1v-.9l3-3-2.2-2.2 1.1-1.1 3.1-.4 2.9-2.9z"/></svg>';
const GEAR = '<svg viewBox="0 0 24 24" aria-hidden="true" style="stroke-width:2"><path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/></svg>';

function el(tag, props = {}, ...kids) {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === 'class') n.className = v;
    else if (k === 'html') n.innerHTML = v;
    else if (k.startsWith('on')) n.addEventListener(k.slice(2), v);
    else n.setAttribute(k, v);
  }
  for (const c of kids) if (c != null) n.append(c);
  return n;
}

function render(s) {
  const list = (s.list || []).sort((a, b) => a.name.localeCompare(b.name));
  const ul = $('list');
  ul.textContent = '';
  $('empty').hidden = list.length > 0;
  $('hint').hidden = list.length === 0;
  $('sub').textContent = list.length ? `${list.length} installed` : '';
  for (const x of list) {
    const icon = x.icon ? el('img', { class: 'ico', src: x.icon, alt: '' }) : el('span', { class: 'ico letter' }, (x.name[0] || '?').toUpperCase());
    const main = el('button', {
      class: 'name', title: x.hasAction ? `Open ${x.name}` : x.name,
      onclick: () => api.open(x.id),
    }, icon, el('span', { class: 'label' }, x.name));
    if (!x.hasAction) main.disabled = true;
    const pin = el('button', {
      class: 'iconbtn pin' + (x.pinned ? ' on' : ''), html: PIN,
      title: x.pinned ? 'Unpin from toolbar' : 'Pin to toolbar',
      'aria-label': (x.pinned ? 'Unpin ' : 'Pin ') + x.name, 'aria-pressed': String(x.pinned),
      onclick: () => api.pin(x.id, !x.pinned),
    });
    const opts = x.hasOptions ? el('button', {
      class: 'iconbtn', html: GEAR, title: `${x.name} options`, 'aria-label': `${x.name} options`,
      onclick: () => api.options(x.id),
    }) : null;
    ul.append(el('li', {}, main, opts, pin));
  }
  reportSize();
}

$('store').addEventListener('click', () => api.store());
$('manage').addEventListener('click', () => api.manage());
function reportSize() { api.size($('card').getBoundingClientRect().height + 24); }
new ResizeObserver(reportSize).observe($('card'));
api.onState(render);
api.onOpen(() => {
  const card = $('card');
  card.classList.remove('opening');
  void card.offsetWidth;
  card.classList.add('opening');
  const first = document.querySelector('.exts button:not(:disabled)') || $('store');
  first.focus({ preventScroll: true });
});
