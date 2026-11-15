// Goat: when a site asks for an image file and there's an image in the clipboard, offer to paste it.
(() => {
  if (window.__goatrPaste || !window.ipc) return;
  window.__goatrPaste = true;
  let bypass = false, seq = 0, menu = null;
  const waiting = new Map();
  const wantsImage = (inp) => {
    const a = (inp.accept || '').toLowerCase();
    return !a || /image|\*|\.(png|jpe?g|gif|webp|bmp|avif|heic)\b/.test(a);
  };
  const oc = HTMLInputElement.prototype.click;
  const openPicker = (inp) => { bypass = true; try { oc.call(inp); } finally { bypass = false; } };
  const ask = (inp) => {
    const id = ++seq;
    waiting.set(id, inp);
    window.ipc.postMessage(JSON.stringify({ cmd: 'clip_image', id }));
    setTimeout(() => { if (waiting.delete(id)) openPicker(inp); }, 1500);
  };
  const intercept = (inp) => !bypass && !inp.disabled && wantsImage(inp);
  document.addEventListener('click', (e) => {
    const inp = e.target;
    if (!(inp instanceof HTMLInputElement) || inp.type !== 'file' || !intercept(inp)) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    ask(inp);
  }, true);
  // Inputs a script makes and clicks without adding them to the page.
  HTMLInputElement.prototype.click = function () {
    if (this.type === 'file' && !this.isConnected && intercept(this)) { ask(this); return; }
    return oc.call(this);
  };
  const sp = HTMLInputElement.prototype.showPicker;
  if (sp) HTMLInputElement.prototype.showPicker = function () {
    if (this.type === 'file' && intercept(this)) { ask(this); return; }
    return sp.call(this);
  };
  const close = () => { if (menu) { menu.remove(); menu = null; } };
  const put = (inp, b64, mime) => {
    const bin = atob(b64), buf = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) buf[i] = bin.charCodeAt(i);
    const stamp = new Date().toISOString().slice(0, 19).replace(/[:T]/g, '-');
    const ext = (mime.split('/')[1] || 'png').replace('jpeg', 'jpg');
    const file = new File([buf], `pasted-${stamp}.${ext}`, { type: mime, lastModified: Date.now() });
    const dt = new DataTransfer();
    dt.items.add(file);
    inp.files = dt.files;
    inp.dispatchEvent(new Event('input', { bubbles: true }));
    inp.dispatchEvent(new Event('change', { bubbles: true }));
  };
  // Rust answers: no image (b64 null) → normal file picker; image → Goat's little menu.
  window.__goatrClip = (id, b64, mime, w, h) => {
    const inp = waiting.get(id);
    if (!inp) return;
    waiting.delete(id);
    if (!b64) { openPicker(inp); return; }
    close();
    const host = document.createElement('goat-paste');
    const root = host.attachShadow({ mode: 'closed' });
    const r = inp.getBoundingClientRect();
    const visible = r.width > 4 && r.height > 4 && r.bottom > 0 && r.top < innerHeight;
    const x = visible ? Math.min(Math.max(8, r.left), innerWidth - 268) : innerWidth / 2 - 130;
    const y = visible ? (r.bottom + 230 < innerHeight ? r.bottom + 6 : Math.max(8, r.top - 230)) : innerHeight / 2 - 110;
    const dark = matchMedia('(prefers-color-scheme: dark)').matches;
    root.innerHTML = `<style>
      .m { position: fixed; z-index: 2147483647; left: ${x}px; top: ${y}px; width: 260px; padding: 8px; box-sizing: border-box;
        font: 13px "Segoe UI", system-ui, sans-serif; color: ${dark ? '#e7e9ec' : '#16191d'}; background: ${dark ? '#181b1f' : '#fff'};
        border: 1px solid ${dark ? '#2b3036' : '#d4d9e0'}; border-radius: 12px; box-shadow: 0 12px 32px rgba(0,0,0,.35);
        transform-origin: top left; animation: pop .32s cubic-bezier(.34,1.56,.64,1) both; }
      @keyframes pop { from { transform: scale(.88); opacity: 0 } }
      img { display: block; width: 100%; max-height: 120px; object-fit: contain; border-radius: 8px; background: ${dark ? '#101214' : '#eef0f3'}; margin-bottom: 6px; }
      small { display: block; opacity: .65; margin: 0 4px 6px; }
      button { all: unset; box-sizing: border-box; display: block; width: 100%; padding: 8px 10px; border-radius: 8px; cursor: pointer; transition: transform .25s cubic-bezier(.34,1.56,.64,1), background .12s; }
      button:hover { background: ${dark ? '#22262b' : '#eef0f3'}; transform: scale(1.03); }
      button.p { font-weight: 600; color: ${dark ? '#5fd3b3' : '#0d8a6c'}; }
    </style><div class="m" role="menu"><img alt=""><small>Image in your clipboard${w ? ` · ${w}×${h}` : ''}</small>
      <button class="p" role="menuitem" data-a="paste">Paste image from clipboard</button>
      <button role="menuitem" data-a="pick">Choose a file…</button></div>`;
    root.querySelector('img').src = `data:${mime};base64,${b64}`;
    root.addEventListener('click', (e) => {
      const a = e.target.closest && e.target.closest('button') && e.target.closest('button').dataset.a;
      if (!a) return;
      close();
      if (a === 'paste') put(inp, b64, mime); else openPicker(inp);
    });
    (document.body || document.documentElement).append(host);
    menu = host;
    root.querySelector('button.p').focus();
    setTimeout(() => {
      const off = (e) => { if (!e.composedPath().includes(host)) { close(); cleanup(); } };
      const esc = (e) => { if (e.key === 'Escape') { close(); cleanup(); } };
      const cleanup = () => { removeEventListener('pointerdown', off, true); removeEventListener('keydown', esc, true); };
      addEventListener('pointerdown', off, true);
      addEventListener('keydown', esc, true);
    }, 0);
  };
})();
