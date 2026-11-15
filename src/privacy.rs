//! Privacy features ported from Goat Browser: encrypted DNS, HTTPS-only, tracking-link cleaning,
//! referrer trimming, third-party cookie blocking, no preloading, and the fingerprint shield.

use crate::store::Settings;

/// (id, name, DNS-over-HTTPS address, "am I using you?" check page)
pub const DNS_PROVIDERS: &[(&str, &str, &str, &str)] = &[
    ("quad9", "Quad9 (recommended)", "https://dns.quad9.net/dns-query", "https://on.quad9.net/"),
    ("mullvad", "Mullvad", "https://dns.mullvad.net/dns-query", "https://mullvad.net/en/check"),
    ("mullvadAdblock", "Mullvad, blocks ads", "https://adblock.dns.mullvad.net/dns-query", "https://mullvad.net/en/check"),
    ("cloudflare", "Cloudflare", "https://cloudflare-dns.com/dns-query", "https://one.one.one.one/help/"),
    ("google", "Google", "https://dns.google/dns-query", ""),
    ("adguard", "AdGuard, blocks ads", "https://dns.adguard-dns.com/dns-query", "https://adguard.com/en/test.html"),
    ("nextdns", "NextDNS", "", "https://test.nextdns.io/"),
    ("custom", "Custom…", "", ""),
    ("off", "Off (use OS DNS)", "", ""),
];

/// Goat's settings profiles: (id, name, description).
pub const PROFILES: &[(&str, &str, &str)] = &[
    ("fastest", "Fastest", "Like Chrome with an ad blocker. Pages preload. Cloudflare DNS."),
    ("balanced", "Balanced", "No background preloading, standard fingerprint protection, Quad9 DNS. The default."),
    ("private", "Most private", "No preloading, fonts from other sites blocked, strict fingerprint and ad blocking, Mullvad DNS. A few sites may break."),
];

pub fn apply_profile(s: &mut Settings, id: &str) {
    match id {
        "fastest" => {
            s.block_prefetch = false;
            s.block_third_party_fonts = false;
            s.fingerprint = "standard".into();
            s.adblock_level = "standard".into();
            s.dns_provider = "cloudflare".into();
        }
        "private" => {
            s.block_prefetch = true;
            s.block_third_party_fonts = true;
            s.fingerprint = "strict".into();
            s.adblock_level = "strict".into();
            s.dns_provider = "mullvad".into();
        }
        _ => {
            s.block_prefetch = true;
            s.block_third_party_fonts = false;
            s.fingerprint = "standard".into();
            s.adblock_level = "standard".into();
            s.dns_provider = "quad9".into();
        }
    }
    s.trim_referrers = true;
}

/// Which profile the current switches match, if any.
pub fn current_profile(s: &Settings) -> &'static str {
    for (id, _, _) in PROFILES {
        let mut t = s.clone();
        apply_profile(&mut t, id);
        if t.block_prefetch == s.block_prefetch
            && t.block_third_party_fonts == s.block_third_party_fonts
            && t.fingerprint == s.fingerprint
            && t.adblock_level == s.adblock_level
            && t.dns_provider == s.dns_provider
            && s.trim_referrers
        {
            return id;
        }
    }
    ""
}

pub fn doh_server(s: &Settings) -> Option<String> {
    match s.dns_provider.as_str() {
        "off" => None,
        "nextdns" => Some(if s.dns_nextdns_id.is_empty() {
            "https://dns.nextdns.io/".into()
        } else {
            format!("https://dns.nextdns.io/{}", s.dns_nextdns_id)
        }),
        "custom" => Some(s.dns_custom.trim().to_string()).filter(|u| u.starts_with("https://")),
        id => DNS_PROVIDERS.iter().find(|p| p.0 == id).map(|p| p.2.to_string()).filter(|u| !u.is_empty()),
    }
}

pub fn dns_check_page(s: &Settings) -> &'static str {
    DNS_PROVIDERS.iter().find(|p| p.0 == s.dns_provider).map(|p| p.3).unwrap_or("")
}

/// Chromium switches for normal and private tabs. Changing these needs a restart (WebView2 reads
/// them once when its browser process starts).
pub fn browser_args(s: &Settings) -> String {
    let mut disable = vec!["msWebOOUI", "msPdfOOUI", "msSmartScreenProtection"];
    let mut enable: Vec<String> = Vec::new();
    let mut args = vec![
        "--force-webrtc-ip-handling-policy=disable_non_proxied_udp".to_string(),
        "--webrtc-ip-handling-policy=disable_non_proxied_udp".to_string(),
        "--disable-breakpad".to_string(),
        "--disable-domain-reliability".to_string(),
        "--no-pings".to_string(),
        "--enable-parallel-downloading".to_string(),
    ];
    if s.block_prefetch {
        disable.extend(["Prerender2", "NoStatePrefetch", "SpeculationRulesPrefetchFuture", "PreconnectToSearch", "NetworkPrediction"]);
        args.push("--dns-prefetch-disable".into());
    }
    // Third-party cookies are handled per request (and by Edge's tracking prevention), not with
    // Chromium's switch: that switch also cut extensions like RoPro off from their own sites.
    if !s.hardware_accel || crate::store::no_gpu_window() {
        args.push("--disable-gpu".into());
        args.push("--disable-gpu-compositing".into());
    }
    match s.proxy_mode.as_str() {
        "none" => args.push("--no-proxy-server".into()),
        "manual" if !s.proxy_host.trim().is_empty() => {
            let scheme = if s.proxy_type == "http" { "http" } else { "socks5" };
            let port = s.proxy_port.trim();
            let port = if port.is_empty() { if scheme == "http" { "8080" } else { "1080" } } else { port };
            args.push(format!("--proxy-server={scheme}://{}:{port}", s.proxy_host.trim()));
        }
        _ => {}
    }
    if let Some(server) = doh_server(s) {
        enable.push("DnsOverHttps<DoHTrial".into());
        args.push("--force-fieldtrials=DoHTrial/Group1".into());
        args.push(format!(
            "--force-fieldtrial-params=DoHTrial.Group1:Fallback/false/Templates/{}",
            urlencoding::encode(&server).replace('.', "%2E")
        ));
    }
    let mut out = format!("--disable-features={}", disable.join(","));
    if !enable.is_empty() {
        out.push_str(&format!(" --enable-features={}", enable.join(",")));
    }
    for a in args {
        out.push(' ');
        out.push_str(&a);
    }
    out
}

/// Encrypted DNS the way Edge stores it: in the browser's "Local State" (read when its engine
/// starts). The command-line switches alone don't turn it on in Edge's engine.
pub fn write_dns_prefs(user_data: &std::path::Path, s: &Settings) {
    let dir = user_data.join("EBWebView");
    let path = dir.join("Local State");
    let _ = std::fs::create_dir_all(&dir);
    let mut v: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    let doh = match doh_server(s) {
        Some(url) => serde_json::json!({ "mode": "secure", "templates": url }),
        None => serde_json::json!({ "mode": "off", "templates": "" }),
    };
    if v["dns_over_https"] == doh {
        return;
    }
    v["dns_over_https"] = doh;
    if let Ok(out) = serde_json::to_string(&v) {
        let tmp = path.with_extension("goatr-tmp");
        if std::fs::write(&tmp, out).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

// ------------------------------------------------------------------ links

const TRACKING_PARAMS: &[&str] = &[
    "utm_source", "utm_medium", "utm_campaign", "utm_term", "utm_content", "utm_id", "utm_name", "utm_reader",
    "utm_brand", "utm_social", "utm_social-type", "fbclid", "gclid", "gclsrc", "dclid", "gbraid", "wbraid",
    "msclkid", "mc_cid", "mc_eid", "igshid", "igsh", "yclid", "_hsenc", "_hsmi", "mkt_tok", "oly_anon_id",
    "oly_enc_id", "vero_id", "vero_conv", "rb_clickid", "s_cid", "ttclid", "twclid", "wickedid", "srsltid",
    "ref_src", "ref_url", "_openstat", "trk", "trkcampaign", "sc_cid", "epik", "li_fat_id", "cmpid",
];

fn site_params(site: &str) -> &'static [&'static str] {
    match site {
        "youtube.com" => &["si", "pp"],
        "youtu.be" | "spotify.com" => &["si"],
        "twitter.com" | "x.com" => &["s", "t"],
        "amazon.com" => &["pd_rd_r", "pd_rd_w", "pd_rd_wg", "pf_rd_p", "pf_rd_r", "pd_rd_i", "ref_", "content-id"],
        _ => &[],
    }
}

/// Registrable domain ("site"), e.g. news.bbc.co.uk → bbc.co.uk. Simple rules, no full list.
pub fn site_of(host: &str) -> String {
    let h = host.split(':').next().unwrap_or("").trim_start_matches("www.").to_lowercase();
    let parts: Vec<&str> = h.split('.').collect();
    if parts.len() <= 2 || h.parse::<std::net::IpAddr>().is_ok() {
        return h;
    }
    let n = parts.len();
    let second = parts[n - 2];
    let take = if parts[n - 1].len() == 2 && ["co", "com", "org", "net", "ac", "gov", "edu", "ne", "or"].contains(&second) { 3 } else { 2 };
    parts[n - take..].join(".")
}

/// The URL without tracking parameters, if it had any.
pub fn clean_tracking(url: &str) -> Option<String> {
    let (base, rest) = url.split_once('?')?;
    let (query, frag) = match rest.split_once('#') {
        Some((q, f)) => (q, Some(f)),
        None => (rest, None),
    };
    let host = crate::store::host_of(url);
    let extra = site_params(&site_of(&host));
    let mut kept = Vec::new();
    let mut changed = false;
    for pair in query.split('&') {
        let key = pair.split('=').next().unwrap_or("");
        let k = key.to_lowercase();
        if !key.is_empty() && (TRACKING_PARAMS.contains(&k.as_str()) || extra.contains(&key)) {
            changed = true;
        } else if !pair.is_empty() {
            kept.push(pair);
        }
    }
    if !changed {
        return None;
    }
    let mut out = base.to_string();
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    if let Some(f) = frag {
        out.push('#');
        out.push_str(f);
    }
    Some(out)
}

fn is_local(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".lan")
        || host.ends_with(".test")
        || host.starts_with("127.")
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || host.starts_with("[")
        || (host.starts_with("172.") && host.split('.').nth(1).and_then(|n| n.parse::<u8>().ok()).is_some_and(|n| (16..32).contains(&n)))
}

/// http://… → https://… for HTTPS-only mode (not for local addresses or hosts you opted out).
pub fn https_upgrade(url: &str, allow_http: &std::collections::HashSet<String>) -> Option<String> {
    let rest = url.strip_prefix("http://")?;
    let authority = crate::store::host_of(url);
    let host = authority.split(':').next().unwrap_or("").to_string();
    if host.is_empty() || is_local(&host) || allow_http.contains(&authority) || allow_http.contains(&host) {
        return None;
    }
    let rest = if authority == format!("{host}:80") { rest.replacen(&authority, &host, 1) } else { rest.to_string() };
    Some(format!("https://{rest}"))
}

// ------------------------------------------------------------------ in-page shields

/// Runs before page scripts in every frame: Goat's fingerprint shield, passkey prompt control,
/// and Global Privacy Control.
pub fn page_script(s: &Settings) -> String {
    let secret: u32 = rand_seed();
    let cfg = serde_json::json!({ "level": s.fingerprint, "webauthn": s.webauthn, "secret": secret });
    FP_JS.replace("__CFG__", &cfg.to_string())
}

fn rand_seed() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.finish() as u32
}

const FP_JS: &str = r#"(() => {
  if (!/^https?:$/.test(location.protocol)) return;
  const cfg = __CFG__;
  try { Object.defineProperty(Navigator.prototype, 'globalPrivacyControl', { get: () => true, configurable: true }); } catch (e) {}
  if (cfg.level === 'off' && cfg.webauthn === 'allow') return;
  const origToString = Function.prototype.toString;
  const native = new WeakMap();
  const toString = { toString() { return native.has(this) ? native.get(this) : origToString.call(this); } }.toString;
  native.set(toString, origToString.call(origToString));
  try { Object.defineProperty(Function.prototype, 'toString', { value: toString, writable: true, configurable: true }); } catch (e) {}
  const wrap = (obj, name, impl) => {
    if (!obj) return;
    const orig = obj[name];
    if (typeof orig !== 'function') return;
    const f = { [name](...a) { return impl.call(this, orig, a); } }[name];
    native.set(f, origToString.call(orig));
    try { Object.defineProperty(obj, name, { value: f, writable: true, configurable: true }); } catch (e) {}
  };
  const getter = (proto, prop, fn) => {
    if (!proto) return;
    const d = Object.getOwnPropertyDescriptor(proto, prop);
    if (!d || !d.get) return;
    const g = Object.getOwnPropertyDescriptor({ get [prop]() { return fn.call(this, d.get); } }, prop).get;
    native.set(g, origToString.call(d.get));
    try { Object.defineProperty(proto, prop, { ...d, get: g }); } catch (e) {}
  };

  // ---- passkeys / security keys
  if (cfg.webauthn !== 'allow' && self.CredentialsContainer) {
    const notAllowed = () => new DOMException('The operation either timed out or was not allowed.', 'NotAllowedError');
    const hold = (signal) => new Promise((_, reject) => {
      if (!signal) return;
      const stop = () => reject(signal.reason || new DOMException('The operation was aborted.', 'AbortError'));
      if (signal.aborted) stop(); else signal.addEventListener('abort', stop, { once: true });
    });
    const ask = () => { try { return window.top === window && confirm(location.hostname + ' wants to use a passkey or security key. Allow?'); } catch (e) { return false; } };
    for (const name of ['get', 'create']) {
      wrap(CredentialsContainer.prototype, name, function (orig, args) {
        const opts = args[0];
        if (!opts || !opts.publicKey) return orig.apply(this, args);
        if (name === 'get' && opts.mediation === 'conditional') return hold(opts.signal);
        if (cfg.webauthn === 'block') return Promise.reject(notAllowed());
        return ask() ? orig.apply(this, args) : Promise.reject(notAllowed());
      });
    }
    if (self.PublicKeyCredential) {
      wrap(PublicKeyCredential, 'isConditionalMediationAvailable', () => Promise.resolve(false));
      if (cfg.webauthn === 'block') wrap(PublicKeyCredential, 'isUserVerifyingPlatformAuthenticatorAvailable', () => Promise.resolve(false));
    }
  }
  if (cfg.level === 'off') return;

  // Hardware: report a common machine.
  getter(Navigator.prototype, 'hardwareConcurrency', () => 8);
  getter(Navigator.prototype, 'deviceMemory', () => 8);
  try { delete Navigator.prototype.getBattery; } catch (e) {}

  // Client hints: no exact OS build, CPU or full version.
  const m = navigator.userAgent.match(/Chrome\/(\d+)/);
  const major = m ? m[1] : '131';
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
        if (Array.isArray(out.fullVersionList)) out.fullVersionList = out.fullVersionList.map((b) => ({ brand: b.brand, version: /Not.?A.?Brand/.test(b.brand) ? b.version : major + '.0.0.0' }));
        return out;
      });
    });
  }

  // WebGL: a very common Windows GPU instead of yours.
  const GL_VENDOR = 'Google Inc. (Intel)';
  const GL_RENDERER = 'ANGLE (Intel, Intel(R) UHD Graphics (0x00009BC4) Direct3D11 vs_5_0 ps_5_0, D3D11)';
  for (const C of [self.WebGLRenderingContext, self.WebGL2RenderingContext]) {
    if (!C) continue;
    wrap(C.prototype, 'getParameter', function (orig, args) {
      const p = args[0];
      if (p === 0x9245) return GL_VENDOR;
      if (p === 0x9246) return GL_RENDERER;
      return orig.apply(this, args);
    });
  }
  if (cfg.level !== 'strict') return;

  // Per-site seed: noise is stable on a site this session, different across sites.
  let h = cfg.secret >>> 0;
  for (const ch of (window.top === window ? location.hostname : (document.referrer ? new URL(document.referrer).hostname : location.hostname))) h = Math.imul(h ^ ch.charCodeAt(0), 0x01000193) >>> 0;
  let s = h;
  const rand = () => { s = (s + 0x6D2B79F5) >>> 0; let t = s; t = Math.imul(t ^ (t >>> 15), t | 1); t ^= t + Math.imul(t ^ (t >>> 7), t | 61); return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
  const noisePixels = (data) => {
    let x = h;
    const n = Math.max(8, Math.min(64, data.length >> 12));
    for (let i = 0; i < n; i++) {
      x = (Math.imul(x ^ (x >>> 13), 0x5bd1e995) + i) >>> 0;
      const idx = (x % (data.length >> 2)) * 4 + (x % 3);
      data[idx] ^= 1;
    }
  };
  const origGetImageData = CanvasRenderingContext2D.prototype.getImageData;
  const noisyCopy = (canvas) => {
    try {
      const w = canvas.width, hh = canvas.height;
      if (!w || !hh || w * hh > 16e6) return null;
      const c = document.createElement('canvas');
      c.width = w; c.height = hh;
      const ctx = c.getContext('2d');
      ctx.drawImage(canvas, 0, 0);
      const img = origGetImageData.call(ctx, 0, 0, w, hh);
      noisePixels(img.data);
      ctx.putImageData(img, 0, 0);
      return c;
    } catch (e) { return null; }
  };
  wrap(CanvasRenderingContext2D.prototype, 'getImageData', function (orig, args) { const img = orig.apply(this, args); noisePixels(img.data); return img; });
  wrap(HTMLCanvasElement.prototype, 'toDataURL', function (orig, args) { return orig.apply(noisyCopy(this) || this, args); });
  wrap(HTMLCanvasElement.prototype, 'toBlob', function (orig, args) { return orig.apply(noisyCopy(this) || this, args); });
  const touched = new WeakSet();
  wrap(self.AudioBuffer && AudioBuffer.prototype, 'getChannelData', function (orig, args) {
    const arr = orig.apply(this, args);
    if (!touched.has(arr)) { touched.add(arr); for (let i = 0; i < arr.length; i += 97) arr[i] += (rand() - 0.5) * 1e-7; }
    return arr;
  });
  wrap(self.AnalyserNode && AnalyserNode.prototype, 'getFloatFrequencyData', function (orig, args) {
    orig.apply(this, args);
    const a = args[0];
    if (a) for (let i = 0; i < a.length; i += 7) a[i] += (rand() - 0.5) * 1e-4;
  });
})();"#;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn links() {
        assert_eq!(clean_tracking("https://a.com/x?utm_source=t&id=5#top").as_deref(), Some("https://a.com/x?id=5#top"));
        assert_eq!(clean_tracking("https://a.com/x?fbclid=1").as_deref(), Some("https://a.com/x"));
        assert_eq!(clean_tracking("https://a.com/x?id=5"), None);
        assert_eq!(clean_tracking("https://www.youtube.com/watch?v=1&si=abc").as_deref(), Some("https://www.youtube.com/watch?v=1"));
        let none = Default::default();
        assert_eq!(https_upgrade("http://example.com:80/a", &none).as_deref(), Some("https://example.com/a"));
        assert_eq!(https_upgrade("http://localhost:3000/", &none), None);
        assert_eq!(site_of("news.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(site_of("a.b.google.com"), "google.com");
        let a = browser_args(&Settings::default());
        assert!(a.contains("Templates/https%3A%2F%2Fdns%2Equad9%2Enet%2Fdns-query"), "{a}");
    }
}
