// Goat: a picture-in-picture button on videos (hover a playing video, top right).
(() => {
  if (window.__goatPip || !document.pictureInPictureEnabled) return;
  window.__goatPip = true;
  let btn = null, target = null, hideT = 0;
  const make = () => {
    const host = document.createElement('div');
    host.style.cssText = 'position:fixed;z-index:2147483647;top:0;left:0;display:none;pointer-events:auto';
    const sh = host.attachShadow({ mode: 'closed' });
    sh.innerHTML = '<style>button{all:initial;cursor:pointer;display:flex;align-items:center;gap:6px;padding:6px 10px;border-radius:9px;background:rgba(20,20,24,.82);color:#fff;font:600 12px system-ui,sans-serif;backdrop-filter:blur(8px);box-shadow:0 2px 10px rgba(0,0,0,.35);transition:background .15s}button:hover{background:rgba(40,40,48,.95)}svg{width:15px;height:15px}</style>' +
      '<button title="Picture in picture"><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="1.5" y="2.5" width="13" height="10" rx="1.5"/><rect x="8" y="7.5" width="5" height="3.5" rx=".6" fill="currentColor"/></svg>Pop out</button>';
    sh.querySelector('button').addEventListener('click', async (e) => {
      e.preventDefault(); e.stopPropagation();
      const v = target;
      if (!v) return;
      try {
        if (document.pictureInPictureElement === v) await document.exitPictureInPicture();
        else { v.disablePictureInPicture = false; await v.requestPictureInPicture(); }
      } catch (_) {}
      hide();
    }, true);
    host.addEventListener('mouseenter', () => clearTimeout(hideT));
    host.addEventListener('mouseleave', () => { hideT = setTimeout(hide, 600); });
    (document.body || document.documentElement).appendChild(host);
    return host;
  };
  const hide = () => { if (btn) btn.style.display = 'none'; target = null; };
  const show = (v) => {
    const r = v.getBoundingClientRect();
    if (r.width < 200 || r.height < 120 || v.readyState < 1) return;
    if (!btn || !btn.isConnected) btn = make();
    target = v;
    btn.style.display = 'block';
    btn.style.top = Math.max(4, r.top + 10) + 'px';
    btn.style.left = Math.max(4, r.right - 104) + 'px';
    clearTimeout(hideT);
    hideT = setTimeout(hide, 2500);
  };
  const videoAt = (x, y) => {
    for (const v of document.querySelectorAll('video')) {
      const r = v.getBoundingClientRect();
      if (x >= r.left && x <= r.right && y >= r.top && y <= r.bottom) return v;
    }
    return null;
  };
  let last = 0;
  document.addEventListener('mousemove', (e) => {
    const t = performance.now();
    if (t - last < 120) return;
    last = t;
    const v = videoAt(e.clientX, e.clientY);
    if (v) show(v);
  }, { passive: true, capture: true });
  window.addEventListener('scroll', hide, { passive: true, capture: true });
})();
