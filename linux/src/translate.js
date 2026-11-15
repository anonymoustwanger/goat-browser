'use strict';
// Page translation. Text is collected from the page in an isolated JS world (the
// page's own scripts can't see or interfere), sent to the chosen service through
// the tab's own session (so a Tor window translates over Tor), and written back.
// "Show original" restores the exact original text.

const WORLD = 1729;

// Languages offered for websites and translation (BCP-47 code -> name).
const LANGUAGES = {
  en: 'English', 'en-GB': 'English (UK)', es: 'Spanish', fr: 'French', de: 'German', it: 'Italian',
  pt: 'Portuguese', 'pt-BR': 'Portuguese (Brazil)', nl: 'Dutch', sv: 'Swedish', no: 'Norwegian', da: 'Danish',
  fi: 'Finnish', pl: 'Polish', cs: 'Czech', sk: 'Slovak', hu: 'Hungarian', ro: 'Romanian', bg: 'Bulgarian',
  el: 'Greek', tr: 'Turkish', ru: 'Russian', uk: 'Ukrainian', hr: 'Croatian', sr: 'Serbian', sl: 'Slovenian',
  lt: 'Lithuanian', lv: 'Latvian', et: 'Estonian', ar: 'Arabic', he: 'Hebrew', fa: 'Persian', hi: 'Hindi',
  bn: 'Bengali', ur: 'Urdu', th: 'Thai', vi: 'Vietnamese', id: 'Indonesian', ms: 'Malay', fil: 'Filipino',
  zh: 'Chinese (Simplified)', 'zh-TW': 'Chinese (Traditional)', ja: 'Japanese', ko: 'Korean', sw: 'Swahili',
  af: 'Afrikaans', ca: 'Catalan', ga: 'Irish', cy: 'Welsh', is: 'Icelandic',
};
const base = (code) => String(code || '').toLowerCase().split(/[-_]/)[0];
const sameLang = (a, b) => {
  const x = String(a || '').toLowerCase(); const y = String(b || '').toLowerCase();
  if (base(x) === 'zh' && base(y) === 'zh') return (x.includes('tw') || x.includes('hant')) === (y.includes('tw') || y.includes('hant'));
  return base(x) === base(y);
};
function acceptLanguage(code) {
  const c = LANGUAGES[code] ? code : 'en';
  const b = base(c);
  // Plain list: Chromium adds the q= weights itself.
  const parts = c.includes('-') ? [c, b] : [c];
  if (b !== 'en') parts.push('en');
  return parts.join(',');
}

// Runs in the isolated world. Returns new text pieces as [id, text].
const COLLECT = `(() => {
  const S = window.__goatTr || (window.__goatTr = { nodes: [], orig: new Map(), attrs: [], done: new WeakSet() });
  const SKIP = new Set(['SCRIPT','STYLE','NOSCRIPT','CODE','PRE','TEXTAREA','KBD','SAMP','VAR','SVG','MATH','TEMPLATE','IFRAME']);
  const out = [];
  const ok = (el) => { for (let e = el; e; e = e.parentElement) { if (SKIP.has(e.tagName) || e.isContentEditable || e.getAttribute('translate') === 'no' || (e.classList && e.classList.contains('notranslate'))) return false; } return true; };
  const walker = document.createTreeWalker(document.body || document.documentElement, NodeFilter.SHOW_TEXT);
  for (let n = walker.nextNode(); n && out.length < 600; n = walker.nextNode()) {
    if (S.done.has(n)) continue;
    const t = n.nodeValue;
    if (!t || !/[^\\s\\d.,:;!?()\\[\\]{}'"\\-–—·•|/\\\\+*=%$€£#@&<>]/.test(t) || t.trim().length < 2) continue;
    if (!ok(n.parentElement)) continue;
    S.done.add(n);
    const id = S.nodes.push(n) - 1;
    S.orig.set(id, t);
    out.push([id, t.trim()]);
  }
  for (const el of document.querySelectorAll('[placeholder],[title],[aria-label],input[type=submit][value],input[type=button][value]')) {
    if (out.length >= 650) break;
    for (const a of ['placeholder', 'title', 'aria-label', 'value']) {
      const v = el.getAttribute(a);
      if (!v || (a === 'value' && !/^(submit|button)$/i.test(el.type)) || el.__goatTrA && el.__goatTrA[a] || !/[A-Za-z\\u00C0-\\uFFFF]/.test(v) || !ok(el)) continue;
      (el.__goatTrA = el.__goatTrA || {})[a] = v;
      const id = S.attrs.push([el, a, v]) - 1;
      out.push(['a' + id, v.trim()]);
    }
  }
  return out;
})()`;

const APPLY = (pairs) => `(() => {
  const S = window.__goatTr; if (!S) return 0;
  let n = 0;
  for (const [id, t] of ${JSON.stringify(pairs)}) {
    if (typeof id === 'string' && id[0] === 'a') { const r = S.attrs[Number(id.slice(1))]; if (r) { r[0].setAttribute(r[1], t); n++; } continue; }
    const node = S.nodes[id]; const o = S.orig.get(id);
    if (!node || o == null) continue;
    const lead = o.match(/^\\s*/)[0]; const trail = o.match(/\\s*$/)[0];
    node.nodeValue = lead + t + trail; n++;
  }
  return n;
})()`;

const RESTORE = `(() => {
  const S = window.__goatTr; if (!S) return 0;
  S.nodes.forEach((node, id) => { if (node && S.orig.has(id)) node.nodeValue = S.orig.get(id); });
  for (const [el, a, v] of S.attrs) { el.setAttribute(a, v); if (el.__goatTrA) delete el.__goatTrA[a]; }
  delete window.__goatTr;
  return 1;
})()`;

const PAGE_LANG = `(() => {
  const h = document.documentElement;
  return (h.getAttribute('lang') || h.getAttribute('xml:lang') || (document.querySelector('meta[http-equiv="content-language" i]') || {}).content || '').trim();
})()`;

class Translator {
  // getConfig() -> { engine: 'google' | 'libre', libreUrl, libreKey }
  constructor(getConfig) { this.getConfig = getConfig; }

  async pageLang(wc) {
    try { return await wc.executeJavaScriptInIsolatedWorld(WORLD, [{ code: PAGE_LANG }]); } catch { return ''; }
  }

  // Translates an array of strings. Returns { out: [...], src }.
  async translateTexts(ses, texts, to) {
    const cfg = this.getConfig();
    if (cfg.engine === 'libre') return this.libre(ses, texts, to, cfg);
    return this.google(ses, texts, to);
  }

  async google(ses, texts, to) {
    // One request per batch: segments joined by newlines, which the service keeps.
    const tl = to === 'zh' ? 'zh-CN' : to;
    const body = new URLSearchParams({ q: texts.map((t) => t.replace(/\s*\n\s*/g, ' ')).join('\n') }).toString();
    const res = await ses.fetch(`https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl=${encodeURIComponent(tl)}&dt=t`, {
      method: 'POST', body, headers: { 'Content-Type': 'application/x-www-form-urlencoded;charset=UTF-8' }, credentials: 'omit',
    });
    if (!res.ok) throw new Error(res.status === 429 ? 'the translation service is rate-limiting this IP (common over Tor); try again or use New circuit' : `translation service answered ${res.status}`);
    const data = await res.json();
    const joined = (data[0] || []).map((c) => (c && c[0]) || '').join('');
    let out = joined.split('\n');
    if (out.length !== texts.length) {
      if (texts.length === 1) out = [joined.trim()];
      else {
        // Lines got merged or split: fall back to one request per segment.
        out = [];
        for (const t of texts) out.push((await this.google(ses, [t], to)).out[0]);
      }
    }
    return { out: out.map((s) => s.trim()), src: data[2] || '' };
  }

  async libre(ses, texts, to, cfg) {
    const url = String(cfg.libreUrl || '').replace(/\/+$/, '');
    if (!/^https?:\/\//.test(url)) throw new Error('set your LibreTranslate address in Settings → Languages');
    const res = await ses.fetch(`${url}/translate`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, credentials: 'omit',
      body: JSON.stringify({ q: texts, source: 'auto', target: base(to) === 'zh' ? (to === 'zh-TW' ? 'zt' : 'zh') : base(to), format: 'text', api_key: cfg.libreKey || undefined }),
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) {
      const msg = data.error || `LibreTranslate answered ${res.status}`;
      if ((res.status === 400 || res.status === 403) && /key/i.test(msg)) throw new Error(`${msg}. Add an API key or switch service in Settings → Languages`);
      throw new Error(msg);
    }
    const arr = Array.isArray(data.translatedText) ? data.translatedText : [data.translatedText];
    const det = Array.isArray(data.detectedLanguage) ? data.detectedLanguage[0] : data.detectedLanguage;
    return { out: arr.map((s) => String(s || '')), src: (det && det.language) || '' };
  }

  // Translate whatever text on the page hasn't been translated yet.
  async translatePage(wc, to) {
    const pieces = await wc.executeJavaScriptInIsolatedWorld(WORLD, [{ code: COLLECT }]);
    if (!pieces || !pieces.length) return { count: 0, src: '' };
    let src = '';
    let count = 0;
    // Batches of ~3500 characters (service limit is ~5000).
    let batch = [];
    let size = 0;
    const flush = async () => {
      if (!batch.length) return;
      const r = await this.translateTexts(wc.session, batch.map((p) => p[1]), to);
      src = src || r.src;
      const pairs = batch.map((p, i) => [p[0], r.out[i] || p[1]]);
      if (!wc.isDestroyed()) count += await wc.executeJavaScriptInIsolatedWorld(WORLD, [{ code: APPLY(pairs) }]);
      batch = []; size = 0;
    };
    for (const p of pieces) {
      const t = p[1].slice(0, 3000);
      if (size + t.length > 3500) await flush();
      batch.push([p[0], t]); size += t.length + 1;
    }
    await flush();
    return { count, src };
  }

  async restore(wc) {
    try { await wc.executeJavaScriptInIsolatedWorld(WORLD, [{ code: RESTORE }]); } catch { /* page gone */ }
  }
}

module.exports = { Translator, LANGUAGES, acceptLanguage, sameLang, base };
