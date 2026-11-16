'use strict';
// Fingerprint shield. Runs in every frame before page scripts and patches
// APIs that sites use to build a unique ID for your device.
//  standard: generic hardware, GPU and client-hint values; no Battery API.
//  strict:   also adds tiny, per-site noise to canvas and audio readouts.
// Passkeys / security keys (WebAuthn), separately configurable:
//  ask:   sites must get your OK before Windows shows its security-key dialog, and
//         background "autofill" passkey requests (which pop that dialog on their own) are held.
//  block: passkey and security-key sign-in is turned off.
//  allow: untouched.
const { contextBridge, ipcRenderer, webFrame } = require('electron');

if (/^https?:$/.test(location.protocol)) {
  let cfg = null;
  try { cfg = ipcRenderer.sendSync('goat:fp-config'); } catch { /* ignore */ }
  if (cfg && (cfg.level !== 'off' || cfg.webauthn !== 'allow')) {
    const run = (cfgArg, ask) => {
      const origToString = Function.prototype.toString;
      const native = new WeakMap();
      const toString = { toString() { return native.has(this) ? native.get(this) : origToString.call(this); } }.toString;
      native.set(toString, origToString.call(origToString));
      try { Object.defineProperty(Function.prototype, 'toString', { value: toString, writable: true, configurable: true }); } catch (e) { /* ignore */ }

      const wrap = (obj, name, impl) => {
        if (!obj) return;
        const orig = obj[name];
        if (typeof orig !== 'function') return;
        const f = { [name](...a) { return impl.call(this, orig, a); } }[name];
        native.set(f, origToString.call(orig));
        try { Object.defineProperty(obj, name, { value: f, writable: true, configurable: true }); } catch (e) { /* ignore */ }
      };
      const getter = (proto, prop, fn) => {
        if (!proto) return;
        const d = Object.getOwnPropertyDescriptor(proto, prop);
        if (!d || !d.get) return;
        const g = Object.getOwnPropertyDescriptor({ get [prop]() { return fn.call(this, d.get); } }, prop).get;
        native.set(g, origToString.call(d.get));
        try { Object.defineProperty(proto, prop, { ...d, get: g }); } catch (e) { /* ignore */ }
      };

      // ---- passkeys / security keys
      if (cfgArg.webauthn !== 'allow' && self.CredentialsContainer) {
        const notAllowed = () => new DOMException('The operation either timed out or was not allowed.', 'NotAllowedError');
        const hold = (signal) => new Promise((_, reject) => {
          if (!signal) return;
          const stop = () => reject(signal.reason || new DOMException('The operation was aborted.', 'AbortError'));
          if (signal.aborted) stop(); else signal.addEventListener('abort', stop, { once: true });
        });
        for (const name of ['get', 'create']) {
          wrap(CredentialsContainer.prototype, name, function (orig, args) {
            const opts = args[0];
            if (!opts || !opts.publicKey) return orig.apply(this, args);
            // Autofill-style requests wait silently instead of popping a Windows dialog.
            if (name === 'get' && opts.mediation === 'conditional') return hold(opts.signal);
            if (cfgArg.webauthn === 'block') return Promise.reject(notAllowed());
            return Promise.resolve(ask()).then((ok) => (ok ? orig.apply(this, args) : Promise.reject(notAllowed())));
          });
        }
        if (self.PublicKeyCredential) {
          wrap(PublicKeyCredential, 'isConditionalMediationAvailable', () => Promise.resolve(false));
          wrap(PublicKeyCredential, 'getClientCapabilities', function (orig, args) {
            return orig.apply(this, args).then((c) => ({ ...c, conditionalGet: false, conditionalCreate: false, passkeyPlatformAuthenticator: cfgArg.webauthn === 'block' ? false : c.passkeyPlatformAuthenticator }));
          });
          if (cfgArg.webauthn === 'block') wrap(PublicKeyCredential, 'isUserVerifyingPlatformAuthenticatorAvailable', () => Promise.resolve(false));
        }
      }

      if (cfgArg.level === 'off') return;

      // Hardware: report a common machine.
      getter(Navigator.prototype, 'hardwareConcurrency', () => 8);
      getter(Navigator.prototype, 'deviceMemory', () => 8);
      try { delete Navigator.prototype.getBattery; } catch (e) { /* ignore */ }

      // User-Agent Client Hints: no exact OS build, CPU or full version.
      const major = String(cfgArg.chrome);
      if (self.NavigatorUAData) {
        wrap(NavigatorUAData.prototype, 'getHighEntropyValues', function (orig, args) {
          return orig.apply(this, args).then((v) => {
            const out = { ...v };
            if ('platformVersion' in out) out.platformVersion = '10.0.0';
            if ('architecture' in out) out.architecture = 'x86';
            if ('bitness' in out) out.bitness = '64';
            if ('model' in out) out.model = '';
            if ('wow64' in out) out.wow64 = false;
            if ('uaFullVersion' in out) out.uaFullVersion = major + '.0.0.0';
            if (Array.isArray(out.fullVersionList)) out.fullVersionList = out.fullVersionList.map((b) => ({ brand: b.brand, version: /^\d+$/.test(b.version.split('.')[0]) && b.brand !== 'Not_A Brand' && !/Not.?A.?Brand/.test(b.brand) ? major + '.0.0.0' : b.version }));
            return out;
          });
        });
      }

      // WebGL: a very common Windows GPU string instead of your real one.
      const GL_VENDOR = 'Google Inc. (Intel)';
      const GL_RENDERER = 'ANGLE (Intel, Intel(R) UHD Graphics (0x00009BC4) Direct3D11 vs_5_0 ps_5_0, D3D11)';
      for (const C of [self.WebGLRenderingContext, self.WebGL2RenderingContext]) {
        if (!C) continue;
        wrap(C.prototype, 'getParameter', function (orig, args) {
          const p = args[0];
          if (p === 0x9245) return GL_VENDOR; // UNMASKED_VENDOR_WEBGL
          if (p === 0x9246) return GL_RENDERER; // UNMASKED_RENDERER_WEBGL
          return orig.apply(this, args);
        });
      }

      if (cfgArg.level !== 'strict') return;

      // Seeded PRNG so noise is stable for this site this session.
      let s = cfgArg.seed >>> 0;
      const rand = () => { s = (s + 0x6D2B79F5) >>> 0; let t = s; t = Math.imul(t ^ (t >>> 15), t | 1); t ^= t + Math.imul(t ^ (t >>> 7), t | 61); return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
      const seedBase = cfgArg.seed >>> 0;
      const noisePixels = (data) => {
        // Flip the lowest bit of a few channels at seed-chosen positions.
        let x = seedBase;
        const n = Math.max(8, Math.min(64, data.length >> 12));
        for (let i = 0; i < n; i++) {
          x = (Math.imul(x ^ (x >>> 13), 0x5bd1e995) + i) >>> 0;
          const idx = (x % (data.length >> 2)) * 4 + (x % 3);
          data[idx] ^= 1;
        }
      };
      const noisyCopy = (canvas) => {
        try {
          const w = canvas.width; const h = canvas.height;
          if (!w || !h || w * h > 16e6) return null;
          const c = document.createElement('canvas');
          c.width = w; c.height = h;
          const ctx = c.getContext('2d');
          ctx.drawImage(canvas, 0, 0);
          const img = origGetImageData.call(ctx, 0, 0, w, h);
          noisePixels(img.data);
          ctx.putImageData(img, 0, 0);
          return c;
        } catch (e) { return null; }
      };
      const origGetImageData = CanvasRenderingContext2D.prototype.getImageData;
      wrap(CanvasRenderingContext2D.prototype, 'getImageData', function (orig, args) {
        const img = orig.apply(this, args);
        noisePixels(img.data);
        return img;
      });
      wrap(HTMLCanvasElement.prototype, 'toDataURL', function (orig, args) {
        const c = noisyCopy(this);
        return orig.apply(c || this, args);
      });
      wrap(HTMLCanvasElement.prototype, 'toBlob', function (orig, args) {
        const c = noisyCopy(this);
        return orig.apply(c || this, args);
      });

      // Audio: imperceptible noise on read-back (used for audio fingerprinting).
      const touched = new WeakSet();
      wrap(self.AudioBuffer && AudioBuffer.prototype, 'getChannelData', function (orig, args) {
        const arr = orig.apply(this, args);
        if (!touched.has(arr)) {
          touched.add(arr);
          for (let i = 0; i < arr.length; i += 97) arr[i] += (rand() - 0.5) * 1e-7;
        }
        return arr;
      });
      wrap(self.AnalyserNode && AnalyserNode.prototype, 'getFloatFrequencyData', function (orig, args) {
        orig.apply(this, args);
        const a = args[0];
        if (a) for (let i = 0; i < a.length; i += 7) a[i] += (rand() - 0.5) * 1e-4;
      });
    };
    try {
      const ask = () => ipcRenderer.invoke('goat:webauthn-ask');
      if (contextBridge.executeInMainWorld) contextBridge.executeInMainWorld({ func: run, args: [cfg, ask] });
      else {
        contextBridge.exposeInMainWorld('__goatWebAuthnAsk', ask);
        webFrame.executeJavaScript(`(${run})(${JSON.stringify(cfg)}, window.__goatWebAuthnAsk)`);
      }
    } catch (e) { /* page still works without the shield */ }
  }
  // Website language (Settings → Languages): navigator.language(s) match the Accept-Language header.
  if (cfg && Array.isArray(cfg.langs) && cfg.langs.length) {
    const setLangs = (langs) => {
      const list = Object.freeze(langs.slice());
      const def = (prop, val) => {
        const d = Object.getOwnPropertyDescriptor(Navigator.prototype, prop);
        if (!d || !d.get) return;
        const g = Object.getOwnPropertyDescriptor({ get [prop]() { return val; } }, prop).get;
        try { Object.defineProperty(Navigator.prototype, prop, { ...d, get: g }); } catch (e) { /* ignore */ }
      };
      def('language', list[0]);
      def('languages', list);
    };
    try {
      if (contextBridge.executeInMainWorld) contextBridge.executeInMainWorld({ func: setLangs, args: [cfg.langs] });
      else webFrame.executeJavaScript(`(${setLangs})(${JSON.stringify(cfg.langs)})`);
    } catch (e) { /* ignore */ }
  }
  // Websites an extension lists in "externally_connectable" (e.g. Proton VPN's sign-in page)
  // can message it with chrome.runtime.sendMessage(extensionId, message), as in Chrome.
  if (cfg && Array.isArray(cfg.extIds) && cfg.extIds.length) {
    const send = (id, msg) => ipcRenderer.invoke('goat:ext-external', String(id), msg);
    const install = (ids, sendFn) => {
      const w = window;
      if (!w.chrome) Object.defineProperty(w, 'chrome', { value: {}, writable: true, configurable: true });
      const runtime = w.chrome.runtime || {};
      let lastError;
      Object.defineProperty(runtime, 'lastError', { get: () => lastError, configurable: true });
      runtime.sendMessage = function sendMessage(extensionId, message, options, callback) {
        if (typeof options === 'function') { callback = options; options = undefined; }
        const p = (!ids.includes(extensionId)
          ? Promise.resolve({ error: 'Could not establish connection. Receiving end does not exist.' })
          : sendFn(extensionId, message)).then((r) => {
          if (r && r.error) throw new Error(r.error);
          return r ? r.response : undefined;
        });
        if (typeof callback === 'function') {
          p.then((res) => { lastError = undefined; callback(res); }, (e) => { lastError = { message: e.message }; try { callback(); } finally { lastError = undefined; } });
          return undefined;
        }
        return p;
      };
      w.chrome.runtime = runtime;
    };
    try {
      if (contextBridge.executeInMainWorld) contextBridge.executeInMainWorld({ func: install, args: [cfg.extIds, send] });
    } catch (e) { /* ignore */ }
  }
  // Google sign-in: look like Firefox (Google blocks Electron-based browsers there).
  if (cfg && cfg.googleCompat) {
    const ff = (ua) => {
      const win = /Windows/.test(ua); const mac = /Macintosh/.test(ua);
      const vals = {
        userAgent: ua,
        appVersion: win ? '5.0 (Windows)' : mac ? '5.0 (Macintosh)' : '5.0 (X11)',
        platform: win ? 'Win32' : mac ? 'MacIntel' : 'Linux x86_64',
        vendor: '', productSub: '20100101',
        oscpu: win ? 'Windows NT 10.0; Win64; x64' : mac ? 'Intel Mac OS X 10.15' : 'Linux x86_64',
        buildID: '20181001000000',
        userAgentData: undefined,
      };
      for (const [k, v] of Object.entries(vals)) {
        try { Object.defineProperty(Navigator.prototype, k, { get: () => v, configurable: true, enumerable: true }); } catch (e) { /* ignore */ }
      }
      try { delete window.chrome; } catch (e) { /* ignore */ }
      try { Object.defineProperty(window, 'chrome', { get: () => undefined, configurable: true }); } catch (e) { /* ignore */ }
    };
    try {
      if (contextBridge.executeInMainWorld) contextBridge.executeInMainWorld({ func: ff, args: [cfg.googleCompat] });
      else webFrame.executeJavaScript(`(${ff})(${JSON.stringify(cfg.googleCompat)})`);
    } catch (e) { /* ignore */ }
  }
  // Paste images when uploading: offer the clipboard's image when a site asks for one.
  if (cfg && cfg.paste) {
    const paste = (getClip) => {
  if (window.__goatrPaste) return;
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
    getClip().then((r) => answer(id, r && r.b64, r && r.mime, r && r.w, r && r.h), () => answer(id, null));
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
  const answer = (id, b64, mime, w, h) => {
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
};
    try {
      const getClip = () => ipcRenderer.invoke('goat:clip-image');
      if (contextBridge.executeInMainWorld) contextBridge.executeInMainWorld({ func: paste, args: [getClip] });
    } catch (e) { /* ignore */ }
  }
  if (cfg && cfg.pip) {
    // A picture-in-picture "Pop out" button on videos (same as Windows Goat's pip.js).
    const pip = () => {
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
    };
    try {
      if (contextBridge.executeInMainWorld) contextBridge.executeInMainWorld({ func: pip, args: [] });
    } catch (e) { /* ignore */ }
  }
}
