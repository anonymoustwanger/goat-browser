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
}
