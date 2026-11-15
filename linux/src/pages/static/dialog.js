'use strict';
// Goat's own dialogs for its pages (instead of the system confirm / alert / prompt boxes).
// goatDialog({title, text, ok, cancel, danger, fields: [{label, value}]}) → true/false, or the
// field values (null when cancelled).
(() => {
  const link = document.createElement('link');
  link.rel = 'stylesheet';
  link.href = 'goat://static/dialog.css';
  document.head.append(link);
  window.goatDialog = (o) => new Promise((done) => {
    const bg = document.createElement('div');
    bg.className = 'gdlg-bg';
    const box = document.createElement('div');
    box.className = 'gdlg';
    box.setAttribute('role', 'dialog');
    box.setAttribute('aria-modal', 'true');
    if (o.title) { const h = document.createElement('h3'); h.textContent = o.title; box.append(h); }
    if (o.text) { const p = document.createElement('p'); p.textContent = o.text; box.append(p); }
    const inputs = (o.fields || []).map((f) => {
      const l = document.createElement('label'); l.textContent = f.label;
      const i = document.createElement('input'); i.value = f.value || ''; i.spellcheck = false;
      l.append(i); box.append(l); return i;
    });
    const row = document.createElement('div'); row.className = 'gdlg-btns';
    const no = document.createElement('button'); no.type = 'button'; no.textContent = o.cancel || 'Cancel';
    const yes = document.createElement('button'); yes.type = 'button'; yes.textContent = o.ok || 'OK';
    yes.className = o.danger ? 'gdlg-danger' : 'gdlg-ok';
    if (o.cancel !== false) row.append(no);
    row.append(yes); box.append(row); bg.append(box); document.body.append(bg);
    const close = (v) => {
      removeEventListener('keydown', key, true);
      bg.classList.add('out');
      setTimeout(() => bg.remove(), 160);
      done(v);
    };
    const key = (e) => {
      if (e.key === 'Escape') { e.preventDefault(); close(o.fields ? null : false); }
      else if (e.key === 'Enter') { e.preventDefault(); yes.click(); }
    };
    addEventListener('keydown', key, true);
    no.onclick = () => close(o.fields ? null : false);
    yes.onclick = () => close(o.fields ? inputs.map((i) => i.value) : true);
    bg.addEventListener('pointerdown', (e) => { if (e.target === bg) close(o.fields ? null : false); });
    (inputs[0] || yes).focus();
    if (inputs[0]) inputs[0].select();
  });
  window.goatAsk = (text, ok, danger) => window.goatDialog({ text, ok: ok || 'OK', danger });
})();
