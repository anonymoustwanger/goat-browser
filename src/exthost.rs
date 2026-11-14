//! Tab support for extensions. WebView2 has no tab strip, so when an extension calls
//! chrome.tabs.create (or windows.create) it would open the system's default browser. Goatr adds
//! a small script to each extension's background that sends those requests to Goatr instead,
//! over a local connection only Goatr's extensions know the key for.

use crate::extinstall;
use crate::shared::{Notice, Shared};
use std::io::{Read, Write};
use std::sync::Arc;

const SHIM: &str = r#"// Added by Goatr: opens extension tabs in Goatr instead of the default browser.
(() => {
  const g = typeof globalThis !== 'undefined' ? globalThis : self;
  const c = g.chrome;
  if (!c || !c.tabs || c.tabs.__goatr) return;
  let cfg = null;
  const config = async () => cfg || (cfg = await (await fetch(c.runtime.getURL('goatr.json'))).json());
  const abs = (u) => { try { return new URL(u || '', c.runtime.getURL('/')).href; } catch (_) { return u || ''; } };
  const open = async (url, active, popup) => {
    try {
      const k = await config();
      await fetch('http://127.0.0.1:' + k.port + '/open?t=' + k.token + '&a=' + (active === false ? 0 : 1) + '&p=' + (popup ? 1 : 0) + '&u=' + encodeURIComponent(url || ''), { mode: 'no-cors' });
    } catch (_) {}
  };
  const log = async (m) => { try { const k = await config(); await fetch('http://127.0.0.1:' + k.port + '/log?t=' + k.token + '&m=' + encodeURIComponent(c.runtime.id + ': ' + m), { mode: 'no-cors' }); } catch (_) {} };
  const fake = (url, active) => ({ id: -1, index: 0, windowId: -1, active: active !== false, url, pendingUrl: url, status: 'loading', incognito: false, highlighted: true, pinned: false, selected: true, discarded: false, autoDiscardable: true, groupId: -1 });
  // The real tab, once Goatr has opened it (so the extension can update or message it).
  const origQuery = c.tabs.query.bind(c.tabs);
  const find = async (url, active) => {
    const list = await origQuery({}).catch(() => []);
    const before = new Set(list.map(t => t.id));
    open(url, active);
    if (!list.length) return fake(url, active); // this extension can't see Goatr's tabs
    for (let i = 0; i < 30; i++) {
      await new Promise(r => setTimeout(r, 100));
      const all = await origQuery({}).catch(() => []);
      const t = all.find(t => !before.has(t.id) && (t.url === url || t.pendingUrl === url)) || all.find(t => !before.has(t.id));
      if (t) return t;
    }
    return fake(url, active);
  };
  const reply = (p, cb) => { if (typeof cb === 'function') { p.then(r => { try { cb(r); } catch (_) {} }); return undefined; } return p; };
  c.tabs.create = function (props, cb) {
    props = props || {};
    log('tabs.create ' + (props.url || ''));
    return reply(find(abs(props.url), props.active), cb);
  };
  if (c.windows && typeof c.windows.create === 'function') {
    c.windows.create = function (props, cb) {
      props = props || {};
      const url = abs(Array.isArray(props.url) ? props.url[0] : props.url);
      const popup = props.type === 'popup' || props.type === 'panel';
      const p = (popup ? (open(url, props.focused, true), Promise.resolve(fake(url))) : find(url, props.focused))
        .then(t => ({ id: t.windowId, focused: props.focused !== false, tabs: [t], type: popup ? 'popup' : 'normal', state: 'normal', incognito: false, alwaysOnTop: false }));
      return reply(p, cb);
    };
  }
  if (c.runtime && typeof c.runtime.openOptionsPage === 'function') {
    c.runtime.openOptionsPage = function (cb) {
      const m = c.runtime.getManifest();
      const page = (m.options_ui && m.options_ui.page) || m.options_page;
      if (page) open(abs(page), true);
      return reply(Promise.resolve(), cb);
    };
  }
  log(c.tabs.create.toString().includes('find(') ? 'background tab support on' : 'background tab support FAILED (tabs.create not replaceable)');
  try { Object.defineProperty(c.tabs, '__goatr', { value: true }); } catch (_) {}
})();
"#;

/// Start the local tab opener and prepare every installed extension. Call before any webview
/// with extensions is created.
pub fn start(sh: Arc<Shared>) {
    let token: String = {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        let a = h.finish();
        let mut h2 = std::collections::hash_map::RandomState::new().build_hasher();
        h2.write_u64(a);
        format!("{:016x}{:016x}", a, h2.finish())
    };
    let Ok(listener) = std::net::TcpListener::bind("127.0.0.1:0") else { return };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    *CONFIG.lock().unwrap() = Some((port, token.clone()));
    prepare_all();
    let _ = std::thread::Builder::new().name("ext-tabs".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
            handle(stream, &token, &sh);
        }
    });
}

static CONFIG: std::sync::Mutex<Option<(u16, String)>> = std::sync::Mutex::new(None);

fn handle(mut s: std::net::TcpStream, token: &str, sh: &Arc<Shared>) {
    let mut buf = [0u8; 8192];
    let n = s.read(&mut buf).unwrap_or(0);
    let req = String::from_utf8_lossy(&buf[..n]);
    let first = req.lines().next().unwrap_or("");
    let method = first.split_whitespace().next().unwrap_or("");
    let path = first.split_whitespace().nth(1).unwrap_or("");
    let mut ok = method == "OPTIONS"; // preflight (local network access)
    if let Some(q) = path.strip_prefix("/log?").filter(|_| method == "GET") {
        let ok_token = q.split('&').any(|kv| kv == format!("t={token}"));
        if let Some(m) = q.split('&').find_map(|kv| kv.strip_prefix("m=")).filter(|_| ok_token) {
            crate::crashlog::debug(format!("extension background: {:.300}", urlencoding::decode(m).map(|c| c.into_owned()).unwrap_or_default()));
            ok = true;
        }
    }
    if let Some(q) = path.strip_prefix("/open?").filter(|_| method == "GET") {
        let mut t = "";
        let mut u = String::new();
        let (mut active, mut popup) = (true, false);
        for kv in q.split('&') {
            match kv.split_once('=') {
                Some(("t", v)) => t = v,
                Some(("a", v)) => active = v != "0",
                Some(("p", v)) => popup = v == "1",
                Some(("u", v)) => u = urlencoding::decode(v).map(|c| c.into_owned()).unwrap_or_default(),
                _ => {}
            }
        }
        if t == token {
            ok = true;
            crate::crashlog::debug(format!("extension opened a tab: {u:.200}"));
            (sh.notify)(Notice::OpenTab(u, active, popup));
        }
    }
    let status = if ok { "204 No Content" } else { "403 Forbidden" };
    let _ = s.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Private-Network: true\r\nAccess-Control-Allow-Methods: GET\r\nConnection: close\r\n\r\n").as_bytes());
}

/// Add the shim to every installed extension and write this run's key.
pub fn prepare_all() {
    for e in extinstall::list() {
        prepare(&extinstall::ext_dir().join(&e.folder));
    }
}

/// Whether the background scripts (following their imports) open tabs or windows.
fn background_opens_tabs(dir: &std::path::Path, start: &[String]) -> bool {
    let mut todo: Vec<String> = start.to_vec();
    let mut seen = std::collections::HashSet::new();
    while let Some(f) = todo.pop() {
        if !seen.insert(f.clone()) || seen.len() > 300 {
            continue;
        }
        let Ok(code) = std::fs::read_to_string(dir.join(&f)) else { continue };
        if ["tabs.create", "windows.create", "openOptionsPage"].iter().any(|k| code.contains(k)) {
            return true;
        }
        // Follow import '…', from '…', import('…') and importScripts('…', '…').
        let base = std::path::Path::new(&f).parent().map(|p| p.to_path_buf()).unwrap_or_default();
        for (i, _) in code.match_indices(".js") {
            let before = &code[..i + 3];
            let Some(q) = before.rfind(['\'', '"', '`']) else { continue };
            let spec = &before[q + 1..];
            if spec.is_empty() || spec.contains(char::is_whitespace) || spec.contains("://") || spec.len() > 200 {
                continue;
            }
            let path = if let Some(abs) = spec.strip_prefix('/') { std::path::PathBuf::from(abs) } else { base.join(spec) };
            // Normalise "./" and "../".
            let mut parts: Vec<String> = Vec::new();
            for c in path.components() {
                match c {
                    std::path::Component::ParentDir => {
                        parts.pop();
                    }
                    std::path::Component::Normal(p) => parts.push(p.to_string_lossy().into_owned()),
                    _ => {}
                }
            }
            todo.push(parts.join("/"));
        }
    }
    false
}

/// Make one extension use Goatr's tabs: shim file, key file, and its background loads the shim.
pub fn prepare(dir: &std::path::Path) {
    // Chromium refuses to load an unpacked extension containing files that start with "_"
    // (reported as "Access is denied"), so Goatr's files are named goatr-*.
    let Some((port, token)) = CONFIG.lock().unwrap().clone() else { return };
    let mpath = dir.join("manifest.json");
    let Ok(text) = std::fs::read_to_string(&mpath) else { return };
    let Ok(mut m) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')) else { return };
    let bg = &mut m["background"];
    let mut changed = false;
    // The original worker, even if Goatr already wrapped it.
    let current_sw = bg["service_worker"].as_str().map(|s| s.trim_start_matches('/').to_string());
    let original_sw = match current_sw.as_deref() {
        Some(w @ ("_goatr_sw.js" | "goatr-sw.js")) => std::fs::read_to_string(dir.join(w)).ok().and_then(|w| {
            w.split(['\'', '"']).filter(|p| !p.is_empty() && p.ends_with(".js") && !p.contains("goatr_shim") && !p.contains("goatr-shim")).last().map(|p| p.trim_start_matches("./").trim_start_matches('/').to_string())
        }),
        other => other.map(|s| s.to_string()),
    };
    // Only extensions whose background opens tabs need the shim; others stay untouched.
    let opens_tabs = |files: &[String]| background_opens_tabs(dir, files);
    if let Some(sw) = original_sw {
        let want = opens_tabs(std::slice::from_ref(&sw));
        if want && current_sw.as_deref() != Some("goatr-sw.js") {
            // Load shim + original from a small wrapper.
            let module = bg["type"].as_str() == Some("module");
            let wrapper = if module {
                format!("import './goatr-shim.js';\nimport './{sw}';\n")
            } else {
                format!("importScripts('/goatr-shim.js', '/{sw}');\n")
            };
            if std::fs::write(dir.join("goatr-sw.js"), wrapper).is_ok() {
                bg["service_worker"] = serde_json::Value::String("goatr-sw.js".into());
                changed = true;
            }
        } else if !want && matches!(current_sw.as_deref(), Some("_goatr_sw.js" | "goatr-sw.js")) {
            // Undo: this extension doesn't open tabs itself.
            bg["service_worker"] = serde_json::Value::String(sw);
            changed = true;
        }
    } else if let Some(scripts) = bg["scripts"].as_array_mut() {
        scripts.retain(|v| v.as_str() != Some("_goatr_shim.js"));
        let files: Vec<String> = scripts.iter().filter_map(|v| v.as_str()).filter(|f| *f != "goatr-shim.js").map(|f| f.trim_start_matches('/').to_string()).collect();
        let has = scripts.first().and_then(|v| v.as_str()) == Some("goatr-shim.js");
        let want = opens_tabs(&files);
        if want && !has {
            scripts.insert(0, serde_json::Value::String("goatr-shim.js".into()));
            changed = true;
        } else if !want && has {
            scripts.remove(0);
            changed = true;
        }
    }
    if changed {
        if let Ok(out) = serde_json::to_string_pretty(&m) {
            let _ = std::fs::write(&mpath, out);
        }
    }
    for f in ["_goatr.json", "_goatr_shim.js", "_goatr_sw.js"] {
        let _ = std::fs::remove_file(dir.join(f));
    }
    // Shim files only for extensions that use them.
    let uses = m["background"]["service_worker"].as_str() == Some("goatr-sw.js")
        || m["background"]["scripts"].as_array().is_some_and(|a| a.first().and_then(|v| v.as_str()) == Some("goatr-shim.js"));
    if uses {
        let _ = std::fs::write(dir.join("goatr-shim.js"), SHIM);
        let _ = std::fs::write(dir.join("goatr.json"), format!("{{\"port\":{port},\"token\":\"{token}\"}}"));
    } else {
        for f in ["goatr.json", "goatr-shim.js", "goatr-sw.js"] {
            let _ = std::fs::remove_file(dir.join(f));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn prepares_mv3_and_mv2() {
        *super::CONFIG.lock().unwrap() = Some((1234, "abc".into()));
        let d = std::env::temp_dir().join("goatr-ext-test");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("a")).unwrap();
        std::fs::create_dir_all(d.join("b")).unwrap();
        std::fs::create_dir_all(d.join("a/bg")).unwrap();
        std::fs::write(d.join("a/bg/worker.js"), "import './lib/open.js';").unwrap();
        std::fs::create_dir_all(d.join("a/bg/lib")).unwrap();
        std::fs::write(d.join("a/bg/lib/open.js"), "chrome.tabs.create({})").unwrap();
        std::fs::write(d.join("b/x.js"), "chrome.windows.create({})").unwrap();
        std::fs::write(d.join("a/manifest.json"), r#"{"manifest_version":3,"background":{"service_worker":"bg/worker.js","type":"module"}}"#).unwrap();
        std::fs::write(d.join("b/manifest.json"), r#"{"manifest_version":2,"background":{"scripts":["x.js"]}}"#).unwrap();
        super::prepare(&d.join("a"));
        super::prepare(&d.join("a")); // idempotent
        super::prepare(&d.join("b"));
        let a = std::fs::read_to_string(d.join("a/manifest.json")).unwrap();
        assert!(a.contains("goatr-sw.js"));
        assert_eq!(std::fs::read_to_string(d.join("a/goatr-sw.js")).unwrap(), "import './goatr-shim.js';\nimport './bg/worker.js';\n");
        let b = std::fs::read_to_string(d.join("b/manifest.json")).unwrap();
        assert!(b.find("goatr-shim.js").unwrap() < b.find("x.js").unwrap());
        // Nothing starting with "_" (Chromium won't load the extension).
        assert!(std::fs::read_dir(d.join("a")).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with('_')));
        assert!(std::fs::read_to_string(d.join("a/goatr.json")).unwrap().contains("1234"));
        // An extension that doesn't open tabs is restored.
        std::fs::write(d.join("a/bg/lib/open.js"), "console.log(1)").unwrap();
        super::prepare(&d.join("a"));
        assert!(std::fs::read_to_string(d.join("a/manifest.json")).unwrap().contains("bg/worker.js"));
        assert!(!std::fs::read_to_string(d.join("a/manifest.json")).unwrap().contains("goatr-sw"));
        assert!(!d.join("a/goatr-shim.js").exists());
    }
}
