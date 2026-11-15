// Injected into every page Goat loads: keyboard shortcuts, tab icons and find-on-page.
(() => {
  if (window.top !== window || window.__goatr) return;
  const ipc = window.ipc;
  const post = m => { try { ipc.postMessage(JSON.stringify(m)); } catch (_) {} };
  // Tell Goat about clicks in the page, so its menus and suggestions close.
  addEventListener('mousedown', () => post({ cmd: 'clicked' }), true);

  // Extension pages: tabs they open go to Goat (not the system's default browser). They get the
  // real tab back (so they can update or message it), can open it in the background, and
  // pop-up windows open as real pop-up windows.
  const installTabs = () => {
    if (!(window.chrome && chrome.tabs && chrome.tabs.create) || chrome.tabs.__goatr) return !!(window.chrome && chrome.tabs && chrome.tabs.__goatr);
    const abs = u => { try { return new URL(u || '', location.href).href; } catch (_) { return u || ''; } };
    const fake = (url, active) => ({ id: -1, index: 0, windowId: -1, active: active !== false, url, pendingUrl: url, status: 'loading', incognito: false, highlighted: true, pinned: false, selected: true, discarded: false, groupId: -1 });
    const origQuery = chrome.tabs.query.bind(chrome.tabs);
    const find = async (url, active) => {
      const list = await origQuery({}).catch(() => []);
      const before = new Set(list.map(t => t.id));
      post({ cmd: 'open_tab', url, active: active !== false });
      if (!list.length) return fake(url, active); // this extension can't see Goat's tabs
      for (let i = 0; i < 30; i++) {
        await new Promise(r => setTimeout(r, 100));
        const all = await origQuery({}).catch(() => []);
        const t = all.find(t => !before.has(t.id) && (t.url === url || t.pendingUrl === url)) || all.find(t => !before.has(t.id));
        if (t) return t;
      }
      return fake(url, active);
    };
    const reply = (p, cb) => { if (typeof cb === 'function') { p.then(r => { try { cb(r); } catch (_) {} }); return undefined; } return p; };
    chrome.tabs.create = function (props, cb) { props = props || {}; return reply(find(abs(props.url), props.active), cb); };
    if (chrome.windows && chrome.windows.create) {
      chrome.windows.create = function (props, cb) {
        props = props || {};
        const url = abs(Array.isArray(props.url) ? props.url[0] : props.url);
        let p;
        if (props.type === 'popup' || props.type === 'panel') {
          window.open(url, '_blank', 'popup,width=' + (props.width || 500) + ',height=' + (props.height || 600) + (props.left != null ? ',left=' + props.left : '') + (props.top != null ? ',top=' + props.top : ''));
          p = Promise.resolve(fake(url));
        } else {
          p = find(url, props.focused);
        }
        return reply(p.then(t => ({ id: t.windowId, focused: props.focused !== false, tabs: [t], type: props.type || 'normal', state: 'normal', incognito: false })), cb);
      };
    }
    if (chrome.runtime && chrome.runtime.openOptionsPage) {
      chrome.runtime.openOptionsPage = function (cb) {
        const m = chrome.runtime.getManifest();
        const page = (m.options_ui && m.options_ui.page) || m.options_page;
        if (page) post({ cmd: 'open_tab', url: chrome.runtime.getURL(page), active: true });
        return reply(Promise.resolve(), cb);
      };
    }
    try { Object.defineProperty(chrome.tabs, '__goatr', { value: true }); } catch (_) {}
    return true;
  };
  if (location.protocol === 'chrome-extension:') {
    // Chrome's extension APIs can appear after this script runs: keep trying until they're there.
    const done = () => post({ cmd: 'js_error', msg: 'tab support on in ' + location.pathname });
    if (installTabs()) done();
    else {
      const iv = setInterval(() => { if (installTabs()) { clearInterval(iv); done(); } }, 25);
      setTimeout(() => clearInterval(iv), 10000);
      document.addEventListener('readystatechange', installTabs);
      document.addEventListener('DOMContentLoaded', installTabs);
    }
  }

  // Extension pages: Edge's page engine may never answer chrome.storage.sync (there's no browser
  // sync account), which leaves pages waiting forever, e.g. RoPro's settings with no text. If
  // sync doesn't answer quickly, use local storage instead, and keep both in step.
  const installStorage = () => {
    const st = window.chrome && chrome.storage;
    if (!st || !st.sync || !st.local || st.sync.__goatr) return !!(st && st.sync && st.sync.__goatr);
    const sync = st.sync, local = st.local;
    let syncDead = false;
    const wrapRead = (name) => {
      const orig = sync[name] && sync[name].bind(sync);
      if (!orig) return;
      sync[name] = function (keys, cb) {
        if (typeof keys === 'function') { cb = keys; keys = null; }
        const viaLocal = () => new Promise(res => local[name](keys, r => res(r)));
        const p = syncDead ? viaLocal() : new Promise(res => {
          let done = false;
          const t = setTimeout(() => { if (!done) { done = true; syncDead = true; post({ cmd: 'js_error', msg: 'chrome.storage.sync not answering; using local storage' }); viaLocal().then(res); } }, 1200);
          try {
            const r = orig(keys, v => { if (!done) { done = true; clearTimeout(t); res(v); } });
            if (r && typeof r.then === 'function') r.then(v => { if (!done) { done = true; clearTimeout(t); res(v); } }, () => {});
          } catch (_) { done = true; clearTimeout(t); syncDead = true; viaLocal().then(res); }
        });
        if (typeof cb === 'function') { p.then(v => { try { cb(v); } catch (_) {} }); return undefined; }
        return p;
      };
    };
    const wrapWrite = (name) => {
      const orig = sync[name] && sync[name].bind(sync);
      if (!orig) return;
      sync[name] = function (arg, cb) {
        if (typeof arg === 'function') { cb = arg; arg = undefined; }
        const both = Promise.all([
          new Promise(res => { try { const r = orig(...(arg === undefined ? [] : [arg]), () => res()); if (r && r.then) r.then(res, res); setTimeout(res, 1200); } catch (_) { res(); } }),
          new Promise(res => { try { local[name](...(arg === undefined ? [] : [arg]), () => res()); } catch (_) { res(); } }),
        ]).then(() => undefined);
        if (typeof cb === 'function') { both.then(() => { try { cb(); } catch (_) {} }); return undefined; }
        return both;
      };
    };
    ['get', 'getBytesInUse'].forEach(wrapRead);
    ['set', 'remove', 'clear'].forEach(wrapWrite);
    try { Object.defineProperty(sync, '__goatr', { value: true }); } catch (_) {}
    return true;
  };
  if (location.protocol === 'chrome-extension:') {
    if (!installStorage()) {
      const iv2 = setInterval(() => { if (installStorage()) clearInterval(iv2); }, 25);
      setTimeout(() => clearInterval(iv2), 10000);
    }
    // Report extension files or servers that fail to load (shows in data\debug.log).
    const of = window.fetch && window.fetch.bind(window);
    if (of) window.fetch = function (input, init) {
      const u = String(input && input.url || input);
      return of(input, init).then(r => { if (!r.ok) post({ cmd: 'js_error', msg: 'fetch ' + r.status + ' ' + u.slice(0, 200) }); return r; },
        e => { post({ cmd: 'js_error', msg: 'fetch failed ' + u.slice(0, 200) + ': ' + e }); throw e; });
    };
  }

  // Extension pop-up pages: report their size so the pop-up fits them.
  if (location.protocol === 'chrome-extension:') {
    const report = () => {
      const d = document.documentElement, b = document.body;
      if (!b) return;
      post({ cmd: 'size', w: Math.ceil(Math.max(d.scrollWidth, b.scrollWidth)), h: Math.ceil(Math.max(d.scrollHeight, b.scrollHeight)) });
    };
    addEventListener('load', report);
    document.addEventListener('DOMContentLoaded', () => { report(); try { new ResizeObserver(report).observe(document.body); } catch (_) {} });
  }

  // Web stores: "Add to Chrome" / "Get" installs into Goat instead of asking about Edge.
  if (/^(chromewebstore\.google\.com|microsoftedge\.microsoft\.com)$/.test(location.hostname)) {
    document.addEventListener('click', e => {
      const b = e.target.closest && e.target.closest('button, [role=button], a');
      if (!b || !/\/detail\//.test(location.pathname)) return;
      const t = (b.textContent || '').trim();
      if (/^(add to (chrome|microsoft edge|edge)|get)$/i.test(t)) {
        e.preventDefault();
        e.stopImmediatePropagation();
        post({ cmd: 'install_ext', url: location.href });
      }
    }, true);
  }

  // Script errors on Goat's own pages go to data\debug.log.
  if (location.hostname === 'goatr.localhost' || location.protocol === 'goatr:' || location.protocol === 'chrome-extension:') {
    addEventListener('error', e => post({ cmd: 'js_error', msg: String(e.message) + ' @' + (e.lineno || 0) + ':' + (e.colno || 0) }));
    addEventListener('unhandledrejection', e => post({ cmd: 'js_error', msg: 'promise: ' + String(e.reason && (e.reason.stack || e.reason)) }));
  }

  document.addEventListener('keydown', e => {
    const c = e.ctrlKey || e.metaKey, sh = e.shiftKey, k = (e.key || '').toLowerCase();
    let a = null;
    if (c && sh && k === 't') a = 'reopen';
    else if (c && sh && k === 'n') a = 'new_private_tab';
    else if (e.altKey && sh && k === 'n') a = 'new_tor_tab';
    else if (c && k === 't') a = 'new_tab';
    else if (c && k === 'w') a = 'close_tab';
    else if (c && k === 'tab') a = sh ? 'prev_tab' : 'next_tab';
    else if ((c && k === 'l') || (e.altKey && k === 'd') || k === 'f6') a = 'focus_address';
    else if (c && k === 'd') a = 'bookmark';
    else if (c && k === 'f') a = 'find';
    else if (c && k === 'h') a = 'history';
    else if (c && k === ',') a = 'settings';
    else if (c && k === 'j') a = 'downloads';
    else if (c && sh && k === 'o') a = 'bookmarks';
    else if (c && sh && k === 'b') a = 'toggle_bar';
    else if (k === 'f5' || (c && k === 'r')) a = 'reload';
    else if (k === 'f11') a = 'fullscreen';
    else if (k === 'f12') a = 'devtools';
    else if (c && k === 'p') a = 'print';
    else if (e.altKey && k === 'arrowleft') a = 'back';
    else if (e.altKey && k === 'arrowright') a = 'forward';
    else if (e.altKey && k === 'home') a = 'home';
    else if (c && !sh && /^[1-9]$/.test(k)) a = 'select:' + k;
    else if (c && (k === '=' || k === '+')) a = 'zoom_in';
    else if (c && (k === '-' || k === '_')) a = 'zoom_out';
    else if (c && k === '0') a = 'zoom_reset';
    else if (k === 'escape') { post({cmd: 'key', action: 'escape'}); return; } // let pages see Esc too
    if (a) { e.preventDefault(); e.stopPropagation(); post({cmd: 'key', action: a}); }
  }, true);

  // Ctrl + mouse wheel zooms (Goat shows the zoom bubble).
  let lastWheel = 0;
  window.addEventListener('wheel', e => {
    if (!(e.ctrlKey || e.metaKey) || !e.cancelable) return;
    e.preventDefault();
    const t = Date.now();
    if (t - lastWheel < 90) return;
    lastWheel = t;
    post({cmd: 'key', action: e.deltaY < 0 ? 'zoom_in' : 'zoom_out'});
  }, {capture: true, passive: false});

  // Mouse back/forward side buttons.
  window.addEventListener('mouseup', e => {
    if (e.button === 3) { e.preventDefault(); post({cmd: 'key', action: 'back'}); }
    if (e.button === 4) { e.preventDefault(); post({cmd: 'key', action: 'forward'}); }
  }, true);

  // Tab icon.
  let lastIcon = '';
  const reportIcon = () => {
    let href = '';
    const links = Array.from(document.querySelectorAll('link[rel~="icon"], link[rel="shortcut icon"], link[rel="apple-touch-icon"]'));
    const pick = links.find(l => /icon/i.test(l.rel) && !/apple/i.test(l.rel)) || links[0];
    if (pick && pick.href) href = pick.href;
    else if (location.protocol.startsWith('http')) href = location.origin + '/favicon.ico';
    if (href && href !== lastIcon) { lastIcon = href; post({cmd: 'favicon', href}); }
  };
  document.addEventListener('DOMContentLoaded', () => {
    reportIcon();
    if (document.head) new MutationObserver(reportIcon).observe(document.head, {childList: true, subtree: true, attributes: true, attributeFilter: ['href']});
  });
  window.addEventListener('load', reportIcon);

  // Find on page.
  const count = q => {
    if (!q || !document.body) return 0;
    const text = document.body.innerText.toLowerCase(), needle = q.toLowerCase();
    let n = 0, i = 0;
    while ((i = text.indexOf(needle, i)) !== -1 && n < 9999) { n++; i += needle.length; }
    return n;
  };
  let lastQ = '';
  // ---- Ad blocker element hiding: report the page's classes and ids so the filter engine can
  // send back the matching hiding rules (like Brave/uBlock's generic cosmetic filtering).
  const seenC = new Set(), seenI = new Set();
  let first = true, pending = false;
  const collect = (root) => {
    const classes = [], ids = [];
    const els = root.querySelectorAll ? root.querySelectorAll('[class],[id]') : [];
    for (let i = 0; i < els.length && i < 4000; i++) {
      const el = els[i];
      if (el.id && !seenI.has(el.id)) { seenI.add(el.id); ids.push(el.id); }
      const cl = el.classList;
      if (cl) for (const c of cl) if (!seenC.has(c)) { seenC.add(c); classes.push(c); }
    }
    return {classes, ids};
  };
  const report = () => {
    pending = false;
    if (!location.protocol.startsWith('http') || !document.documentElement) return;
    const {classes, ids} = collect(document);
    if (first || classes.length || ids.length) {
      post({cmd: 'cosmetic', url: location.href, classes: classes.slice(0, 3000), ids: ids.slice(0, 3000), first});
      first = false;
    }
  };
  const schedule = () => { if (!pending) { pending = true; setTimeout(report, 400); } };
  document.addEventListener('DOMContentLoaded', () => {
    report();
    new MutationObserver(schedule).observe(document.documentElement, {childList: true, subtree: true, attributes: true, attributeFilter: ['class', 'id']});
  });

  window.__goatr = {
    css(id, text, append) {
      let s = document.getElementById(id);
      if (!s) { s = document.createElement('style'); s.id = id; (document.head || document.documentElement).appendChild(s); }
      if (append) s.textContent += '\n' + text; else s.textContent = text;
    },
    find(q, back) {
      if (!q) { window.getSelection().removeAllRanges(); post({cmd: 'find_result', count: 0}); lastQ = ''; return; }
      if (q !== lastQ) { window.getSelection().removeAllRanges(); lastQ = q; }
      window.find(q, false, !!back, true, false, false, false);
      post({cmd: 'find_result', count: count(q)});
    },
    findClear() { lastQ = ''; try { window.getSelection().removeAllRanges(); } catch (_) {} },
  };
})();
