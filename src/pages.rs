//! Built-in pages (goatr://newtab, history, bookmarks, downloads, settings) and their small JSON API.

use std::borrow::Cow;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use chrono::{Local, TimeZone};
use serde::Deserialize;
use wry::http::{Request, Response};

use crate::extinstall;
use crate::privacy;
use crate::shared::{Notice, Shared};
use crate::store::{host_of, Settings, Store, ENGINES};
use crate::themes::{self, THEMES};
use crate::INTERNAL_BASE;

type Resp = Response<Cow<'static, [u8]>>;


pub fn respond(status: u16, mime: &str, body: Vec<u8>) -> Resp {
    Response::builder()
        .status(status)
        .header("Content-Type", mime)
        .header("Cache-Control", "no-store")
        .body(Cow::Owned(body))
        .unwrap()
}

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (k == key).then(|| urlencoding::decode(&v.replace('+', " ")).map(|c| c.into_owned()).unwrap_or_default())
    })
}

/// Turn address-bar text into a URL: full URL, bare domain, built-in page, or a web search.
pub fn resolve_input(settings: &Settings, input: &str) -> String {
    let s = input.trim();
    if s.is_empty() {
        return format!("{INTERNAL_BASE}/newtab");
    }
    let lower = s.to_lowercase();
    if let Some(rest) = s.strip_prefix("goat-extension://").or_else(|| s.strip_prefix("Goat-extension://")) {
        return format!("chrome-extension://{rest}");
    }
    // goat://settings (and goatr://, edge://, chrome:// for Goat's own pages).
    if let Some(u) = browser_page(&lower) {
        return u;
    }
    let has_scheme = ["http://", "https://", "file://", "about:", "data:", "view-source:", "edge:"]
        .iter()
        .any(|p| lower.starts_with(p));
    if has_scheme || lower.starts_with(INTERNAL_BASE) {
        return s.to_string();
    }
    // Windows paths like C:\file.pdf
    if s.len() > 2 && s.as_bytes()[1] == b':' && (s.as_bytes()[2] == b'\\' || s.as_bytes()[2] == b'/') {
        return format!("file:///{}", s.replace('\\', "/"));
    }
    let looks_like_host = !s.contains(' ')
        && (s.contains('.') || lower.starts_with("localhost"))
        && !s.starts_with('.')
        && !s.ends_with('.');
    if looks_like_host {
        let local = lower.starts_with("localhost") || lower.starts_with("127.") || lower.starts_with("192.168.");
        return format!("{}://{s}", if local { "http" } else { "https" });
    }
    format!("{}{}", settings.search_prefix(), urlencoding::encode(s))
}

/// goat://… (also goatr://, edge://, chrome://, about:) → Goat's own page for it, if it has one.
pub fn browser_page(url: &str) -> Option<String> {
    let l = url.trim().to_lowercase();
    let rest = ["goat://", "goatr://", "edge://", "chrome://", "about:"].iter().find_map(|p| l.strip_prefix(p))?;
    let rest = rest.trim_start_matches("localhost/");
    let (name, tail) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let page = match name {
        "" | "newtab" | "new-tab-page" | "home" => "newtab",
        "settings" | "preferences" => "settings",
        "history" => "history",
        "bookmarks" | "favorites" | "favourites" => "bookmarks",
        "downloads" => "downloads",
        "extensions" | "apps" => "extensions",
        "tor" => "tor",
        _ if l.starts_with("goat://") || l.starts_with("goatr://") => name,
        _ => return None,
    };
    // Keep ?id=… for extensions; "/settings/privacy" → settings#privacy.
    let tail = tail.trim_start_matches('/');
    Some(match page {
        "extensions" => format!("{INTERNAL_BASE}/extensions{}", if tail.starts_with('?') { tail.to_string() } else { String::new() }),
        "settings" if !tail.is_empty() && !tail.starts_with('?') && !tail.starts_with('#') => format!("{INTERNAL_BASE}/settings#{}", tail.split('/').next().unwrap_or("")),
        _ => format!("{INTERNAL_BASE}/{page}{}", if tail.starts_with('?') || tail.starts_with('#') { tail } else { "" }),
    })
}

/// How a URL is shown in the address bar.
pub fn display_url(url: &str) -> String {
    // The error page shows the address that failed.
    if let Some(q) = url.strip_prefix(INTERNAL_BASE).and_then(|p| p.strip_prefix("/neterror?")) {
        return query_param(q, "u").unwrap_or_default();
    }
    match url.strip_prefix(INTERNAL_BASE) {
        Some(p) if p.starts_with("/newtab") || p == "/tor" || p.starts_with("/go") => String::new(),
        Some(rest) => format!("goat:/{}", if rest.is_empty() { "/newtab" } else { rest }),
        // Extension pages: goat-extension://<id>/… (typing that opens the real page).
        None => match url.strip_prefix("chrome-extension://") {
            Some(rest) => format!("goat-extension://{rest}"),
            None => url.to_string(),
        },
    }
}

/// Theme + effects for page rendering.
pub struct Look {
    vars: String,
    anim: bool,
}

pub fn handle(sh: &Arc<Shared>, req: Request<Vec<u8>>) -> Resp {
    let h = req.headers();
    let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();

    if let Some(api) = path.strip_prefix("/api/") {
        // Only our own pages may call the API: custom header (forces CORS preflight) + same origin.
        let origin = get("origin");
        let same_origin = origin.is_empty() || origin == "null" || origin.starts_with(INTERNAL_BASE);
        if req.method() != "POST" || get("x-goatr") != "1" || !same_origin {
            return respond(403, "text/plain", b"forbidden".to_vec());
        }
        return api_call(sh, api, req.body());
    }

    // Other websites can't embed or fetch these pages.
    let cross = get("sec-fetch-site") == "cross-site" && get("sec-fetch-mode") != "navigate";
    if cross {
        return respond(403, "text/plain", b"forbidden".to_vec());
    }

    let s = sh.store.lock().unwrap();
    let look = &Look {
        vars: themes::get(&s.data.settings.theme, sh.system_dark.load(Ordering::Relaxed)).css_vars(s.data.settings.gradients),
        anim: s.data.settings.animations,
    };
    match path.trim_end_matches('/') {
        "" | "/newtab" => newtab(&s, look, query_param(&query, "private").is_some()),
        "/go" => {
            let target = resolve_input(&s.data.settings, &query_param(&query, "q").unwrap_or_default());
            let js = serde_json::to_string(&target).unwrap_or_default();
            page(look, "Redirecting…", "", &format!("<script>location.replace({js})</script>"))
        }
        "/history" => history(&s, look, &query_param(&query, "q").unwrap_or_default()),
        "/bookmarks" => bookmarks(&s, look),
        "/downloads" => downloads(&s, look),
        "/extensions" => {
            // edge://extensions/?id=… from an extension: jump to that extension's row.
            let id = query.split('&').find_map(|kv| kv.strip_prefix("id=")).unwrap_or("");
            let id: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            let target = if id.is_empty() { "extensions".to_string() } else { format!("ext-{id}") };
            page(look, "Extensions", "", &format!("<script>location.replace('{INTERNAL_BASE}/settings#{target}')</script>"))
        }
        "/settings" => settings(&s, look, sh),
        "/tor" => tor_page(look),
        "/neterror" => net_error(look, &query_param(&query, "u").unwrap_or_default(), query_param(&query, "e").and_then(|e| e.parse().ok()).unwrap_or(0)),
        _ => page(look, "Not found", "", "<main class=\"solo\"><h1>Page not found</h1></main>"),
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ApiBody {
    id: u64,
    title: String,
    url: String,
    time: i64,
    items: Vec<(String, i64)>,
    range: String,
}

fn api_call(sh: &Arc<Shared>, api: &str, body: &[u8]) -> Resp {
    let ok = || respond(200, "application/json", b"{\"ok\":true}".to_vec());
    let json = |v: serde_json::Value| respond(200, "application/json", v.to_string().into_bytes());
    let notify = |n: Notice| (sh.notify)(n);
    let b: ApiBody = serde_json::from_slice(body).unwrap_or_default();

    // Calls that don't touch saved data.
    match api {
        "tor/status" => return json(serde_json::to_value(sh.tor.state()).unwrap_or_default()),
        "tor/start" => {
            sh.tor.start(sh.tor_notifier());
            return ok();
        }
        "updates/check" => {
            notify(Notice::CheckUpdate);
            return ok();
        }
        "tor/open" => {
            notify(Notice::OpenTorTab);
            return ok();
        }
        // Cookie editor: the engine answers later, so the page asks for the result by id.
        "cookies/op" => {
            let op: crate::shared::CookieOp = match serde_json::from_slice(body) {
                Ok(op) => op,
                Err(e) => return respond(400, "text/plain", e.to_string().into_bytes()),
            };
            let id = sh.next_cookie_req.fetch_add(1, Ordering::SeqCst) + 1;
            notify(Notice::Cookies(id, op));
            return json(serde_json::json!({ "id": id }));
        }
        "cookies/result" => {
            return match sh.cookie_results.lock().unwrap().remove(&b.id) {
                Some(v) => json(v),
                None => json(serde_json::json!({ "pending": true })),
            };
        }
        "adblock/update" => {
            let me = sh.clone();
            sh.adblock.refresh(true, move |n| (me.notify)(Notice::AdblockUpdated(n)));
            return ok();
        }
        "adblock/status" => {
            return json(serde_json::json!({
                "domains": sh.adblock.domain_count(),
                "updating": sh.adblock.is_updating(),
                "updated": sh.adblock.last_updated().map(|t| fmt_ago(t)).unwrap_or_else(|| "never".into()),
            }))
        }
        "extensions/install" => {
            match extinstall::parse_store_url(&b.url) {
                Some((src, id)) => {
                    let me = sh.clone();
                    std::thread::spawn(move || (me.notify)(Notice::ExtensionInstalled(extinstall::install(src, &id))));
                    return ok();
                }
                None => return respond(400, "text/plain", b"That isn't a Chrome Web Store or Edge Add-ons link".to_vec()),
            }
        }
        "extensions/remove" => {
            return match extinstall::remove(&b.title) {
                Ok(()) => ok(),
                Err(e) => respond(400, "text/plain", e.into_bytes()),
            }
        }
        // Turn an extension on or off (title = folder, url = "on"/"off").
        "extensions/enable" => {
            let on = b.url == "on";
            {
                let mut s = sh.store.lock().unwrap();
                let list = &mut s.data.settings.disabled_extensions;
                list.retain(|f| *f != b.title);
                if !on {
                    list.push(b.title.clone());
                }
                s.save();
            }
            notify(Notice::ExtensionEnabled(b.title.clone(), on));
            notify(Notice::StoreChanged);
            return ok();
        }
        // Allow or stop an extension in private windows (title = folder, url = "on"/"off").
        "extensions/private" => {
            {
                let mut s = sh.store.lock().unwrap();
                let list = &mut s.data.settings.private_extensions;
                list.retain(|f| *f != b.title);
                if b.url == "on" {
                    list.push(b.title.clone());
                }
                s.save();
            }
            notify(Notice::StoreChanged);
            return ok();
        }
        "extensions/open_folder" => {
            let dir = extinstall::ext_dir();
            let _ = std::fs::create_dir_all(&dir);
            let _ = open::that_detached(dir);
            return ok();
        }
        "restart" => {
            notify(Notice::Restart);
            return ok();
        }
        _ => {}
    }

    let mut s = sh.store.lock().unwrap();
    if let Some(op) = api.strip_prefix("profiles/") {
        let name = crate::store::clean_profile(&b.title);
        drop(s);
        match op {
            "open" => notify(Notice::OpenProfile(name)),
            "add" => {
                if name.is_empty() {
                    return respond(400, "text/plain", b"Give the profile a name".to_vec());
                }
                let _ = std::fs::create_dir_all(crate::store::app_dir().join("profiles").join(&name));
                notify(Notice::OpenProfile(name));
            }
            // Start-up profile: url = "ask", or title = the profile to always open.
            "startup" => {
                if b.url == "ask" {
                    crate::store::set_startup_profile(None);
                } else {
                    crate::store::set_startup_profile(Some(&name));
                }
            }
            // Logo: url = {"emoji": "…", "color": "#rrggbb"}.
            "logo" => {
                let mut logo: crate::store::ProfileLogo = serde_json::from_str(&b.url).unwrap_or_default();
                logo.emoji.clear();
                let ok_color = logo.color.is_empty() || crate::store::LOGO_COLORS.contains(&logo.color.as_str());
                if !ok_color {
                    return respond(400, "text/plain", b"Unknown logo".to_vec());
                }
                crate::store::set_profile_logo(&name, logo);
                notify(Notice::StoreChanged);
            }
            "delete" => {
                if name.is_empty() || name == crate::store::launch().profile {
                    return respond(400, "text/plain", b"Can't delete this profile".to_vec());
                }
                let dir = crate::store::app_dir().join("profiles").join(&name);
                // Fails while that profile's window is open (its files are in use).
                if std::fs::remove_dir_all(&dir).is_err() {
                    return respond(409, "text/plain", b"Close that profile's window first".to_vec());
                }
            }
            _ => return respond(404, "text/plain", Vec::new()),
        }
        return ok();
    }
    if api == "privacy/profile" {
        privacy::apply_profile(&mut s.data.settings, &b.title);
        let lvl = crate::adblock::Level::parse(&s.data.settings.adblock_level);
        s.save();
        drop(s);
        sh.adblock.set_level(lvl);
        notify(Notice::StoreChanged);
        return ok();
    }
    if api == "settings" {
        // Merge, so fields the page doesn't know about (e.g. the ad-block allow-list) are kept.
        let mut cur = serde_json::to_value(&s.data.settings).unwrap_or_default();
        if let (Some(c), Ok(serde_json::Value::Object(new))) = (cur.as_object_mut(), serde_json::from_slice::<serde_json::Value>(body)) {
            for (k, v) in new {
                c.insert(k, v);
            }
        }
        match serde_json::from_value::<Settings>(cur) {
            Ok(new) => {
                sh.adblock.set_enabled(new.adblock);
                s.data.settings = new;
                s.save();
                drop(s);
                notify(Notice::StoreChanged);
                return ok();
            }
            Err(e) => return respond(400, "text/plain", e.to_string().into_bytes()),
        }
    }
    match api {
        "adblock/allow_add" => {
            let h = host_of(if b.url.contains("://") { b.url.trim().to_string() } else { format!("https://{}", b.url.trim()) }.as_str());
            let h = h.trim_start_matches("www.").to_string();
            if !h.is_empty() && !s.data.settings.adblock_allow.contains(&h) {
                s.data.settings.adblock_allow.push(h);
                sh.adblock.set_allowlist(&s.data.settings.adblock_allow);
                s.save();
            }
        }
        "history/delete_many" => {
            let set: std::collections::HashSet<(String, i64)> = b.items.into_iter().collect();
            s.data.history.retain(|v| !set.contains(&(v.url.clone(), v.time)));
            s.save();
        }
        "history/clear_range" => {
            let secs = match b.range.as_str() { "hour" => 3600, "day" => 86_400, "week" => 7 * 86_400, "month" => 30 * 86_400, _ => i64::MAX };
            if secs == i64::MAX {
                s.clear_history();
            } else {
                let since = Local::now().timestamp() - secs;
                s.data.history.retain(|v| v.time < since);
                s.save();
            }
        }
        "adblock/allow_remove" => {
            s.data.settings.adblock_allow.retain(|h| h != &b.url);
            sh.adblock.set_allowlist(&s.data.settings.adblock_allow);
            s.save();
        }
        "bookmark/add" => {
            if !s.is_bookmarked(&b.url) && !b.url.trim().is_empty() {
                s.toggle_bookmark(&b.title, &b.url);
            }
        }
        "bookmark/delete" => s.delete_bookmark(b.id),
        "bookmark/edit" => s.edit_bookmark(b.id, &b.title, &b.url),
        "history/delete" => s.delete_visit(&b.url, b.time),
        "history/clear" => s.clear_history(),
        "downloads/clear" => {
            s.data.downloads.retain(|d| d.state == "in_progress");
            s.save();
        }
        "downloads/open" | "downloads/show" => {
            if let Some(d) = s.data.downloads.iter().find(|d| d.id == b.id) {
                let p = std::path::PathBuf::from(&d.path);
                if api == "downloads/open" {
                    let _ = open::that_detached(&p);
                } else {
                    show_in_folder(&p);
                }
            }
            return ok();
        }
        "clear_all" => {
            s.clear_history();
            drop(s);
            notify(Notice::ClearBrowsingData);
            notify(Notice::StoreChanged);
            return ok();
        }
        _ => return respond(404, "text/plain", b"unknown".to_vec()),
    }
    drop(s);
    notify(Notice::StoreChanged);
    ok()
}

fn show_in_folder(p: &std::path::Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(format!("/select,{}", p.display())).spawn();
    }
    #[cfg(not(windows))]
    {
        if let Some(dir) = p.parent() {
            let _ = open::that_detached(dir);
        }
    }
}

// ------------------------------------------------------------------ page shell (Goat layout)

const SHARED_CSS: &str = include_str!("web/shared.css");
const SETTINGS_CSS: &str = include_str!("web/settings.css");
const HISTORY_CSS: &str = include_str!("web/history.css");

/// Goat additions on top of Goat's stylesheets.
const EXTRA_CSS: &str = r#"
:root { --card: var(--bg-2); --dur-1: 120ms; --dur-2: 200ms; --dur-3: 320ms;
  --ease-out: cubic-bezier(.16, 1, .3, 1); --ease-in: cubic-bezier(.5, 0, .75, 0);
  /* no bouncing anywhere */ --ease-spring: cubic-bezier(.16, 1, .3, 1); }
:root[data-motion="off"] { --dur-1: 0ms; --dur-2: 0ms; --dur-3: 0ms; }
:root[data-motion="off"] *, :root[data-motion="off"] *::before, :root[data-motion="off"] *::after { animation: none !important; }
@keyframes rise { from { opacity: 0; transform: translateY(10px); } }
html { overflow-x: hidden; }
/* icons and site letters follow the theme */
.ticon { color: var(--accent); background: color-mix(in srgb, var(--accent) 14%, var(--bg-2)); border: 1px solid var(--line); box-shadow: none; }
:root[data-gradients="off"] .ticon { background: var(--bg-2); }
.hico { color: var(--accent); background: color-mix(in srgb, var(--accent) 16%, var(--bg-2)); }
.exticon { color: var(--accent); }
.plogo { width: 34px; height: 34px; border-radius: 50%; border: none; flex: none; display: grid; place-items: center; font-size: 16px; font-weight: 700; color: #101214; cursor: pointer; padding: 0; }
.plogo:hover { filter: brightness(1.1); }
.logopick { margin: 4px 0 10px 46px; padding: 10px; border-radius: 12px; background: var(--bg3, rgba(127,127,127,.12)); display: grid; gap: 10px; }
.lp-colors { display: flex; flex-wrap: wrap; gap: 6px; }
.lp-colors button { width: 24px; height: 24px; border-radius: 50%; border: 2px solid rgba(255,255,255,.25); padding: 0; }
.ic { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; stroke-linejoin: round; vertical-align: -3px; }
/* list pages (history, bookmarks, downloads) */
.hday li .sub { color: var(--muted); font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.hday li .grow { flex: 1; min-width: 0; display: flex; flex-direction: column; }
.hday li .grow b { font-weight: 500; font-size: 14px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.hday .rowbtn { font: inherit; font-size: 12.5px; color: var(--text); background: var(--bg); border: 1px solid var(--line); border-radius: 8px; padding: 4px 10px; cursor: pointer; }
.hday .rowbtn:hover { border-color: var(--muted); }
.navlinks { display: flex; gap: 6px; margin: -6px 0 14px; }
.navlinks a { color: var(--muted); text-decoration: none; font-size: 13px; padding: 5px 10px; border-radius: 8px; }
.navlinks a:hover { background: var(--card); color: var(--text); }
.navlinks a.on { background: var(--card); color: var(--text); font-weight: 600; }
/* settings extras */
.side a.on { box-shadow: inset 2px 0 0 var(--accent); }
.theme-card:not(.system) .swatch { background: var(--bg); }
.ext .x { font-size: 18px; }
.ext:target { outline: 2px solid var(--accent); outline-offset: 2px; border-radius: 10px; }
.extpriv { display: flex; align-items: center; gap: 8px; font-size: 12px; color: var(--muted); white-space: nowrap; margin-right: 6px; }
.storeform { display: flex; gap: 8px; padding: 0 0 14px; }
.storeform input { flex: 1; }
/* private / Tor new tab */
.privnote { margin: -8px auto 20px; max-width: 560px; text-align: center; font-size: 13px; color: var(--muted); }
.privnote b { color: #c4b5fd; }
body.private { background: radial-gradient(900px 520px at 50% -10%, rgba(167, 139, 250, .18), transparent 70%), var(--bg); }
body.private .search input:focus { border-color: #a78bfa; box-shadow: 0 0 0 4px rgba(167, 139, 250, .2); }
.torcard { background: var(--card); border: 1px solid var(--line); border-radius: 12px; padding: 16px 18px; margin-bottom: 20px; }
.torcard .bar { height: 6px; border-radius: 6px; background: var(--bg-3); overflow: hidden; margin: 12px 0; }
.torcard .bar i { display: block; height: 100%; width: 0; background: #a78bfa; transition: width .4s var(--ease-out); }
.torcard button { font: inherit; font-size: 14px; color: var(--text); background: var(--bg); border: 1px solid var(--line); border-radius: 8px; padding: 7px 14px; cursor: pointer; }
.torlinks { text-align: center; font-size: 13px; margin-top: 14px; }
.torlinks a { color: var(--accent); }
"#;

/// Line icons (same SVG paths as the toolbar), drawn in the theme's colours.
fn svg(path: &str) -> String {
    format!("<svg viewBox=\"0 0 16 16\" class=\"ic\">{path}</svg>")
}
const IC_TRASH: &str = r#"<path d="M2.5 4.5h11M6.5 4.5V3h3v1.5M4 4.5l.7 9h6.6l.7-9"/>"#;

const API_JS: &str = r#"<script>
async function api(p, body) {
  const r = await fetch('INTERNAL/api/' + p, {method: 'POST', headers: {'X-Goatr': '1', 'Content-Type': 'application/json'}, body: JSON.stringify(body || {})});
  return r.ok;
}
const $ = (id) => document.getElementById(id);
</script>"#;

fn page_doc(look: &Look, title: &str, body_class: &str, extra_css: &str, body: &str) -> Resp {
    let html = format!(
        "<!doctype html><html lang=\"en\" data-motion=\"{}\" data-gradients=\"{}\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\">\
         <title>{}</title><style>:root{{{}}}\n{SHARED_CSS}\n{extra_css}\n{EXTRA_CSS}</style>{}</head><body class=\"{body_class}\">{body}</body></html>",
        if look.anim { "on" } else { "off" },
        if look.vars.contains("--grad-page:var(--bg)") { "off" } else { "on" },
        esc(title),
        look.vars,
        API_JS.replace("INTERNAL", INTERNAL_BASE),
    );
    respond(200, "text/html; charset=utf-8", html.into_bytes())
}

/// Plain page for small messages.
fn page(look: &Look, title: &str, _active: &str, main: &str) -> Resp {
    page_doc(look, title, "", "", &format!("<main>{main}</main>"))
}

fn site_letter(title: &str, url: &str) -> String {
    let host = host_of(url);
    host.trim_start_matches("www.").chars().next().or_else(|| title.trim().chars().next()).unwrap_or('•').to_uppercase().to_string()
}

fn short_host(url: &str) -> String {
    host_of(url).trim_start_matches("www.").to_string()
}

/// Links between Goat's own list pages.
fn navlinks(active: &str) -> String {
    let l = |id: &str, label: &str| format!("<a href=\"{INTERNAL_BASE}/{id}\" class=\"{}\">{label}</a>", if id == active { "on" } else { "" });
    format!(
        "<nav class=\"navlinks\">{}{}{}{}</nav>",
        l("history", "History"),
        l("bookmarks", "Bookmarks"),
        l("downloads", "Downloads"),
        l("settings", "Settings")
    )
}

// ------------------------------------------------------------------ new tab

fn newtab(s: &Store, look: &Look, private: bool) -> Resp {
    if private {
        let body = r#"<main>
<p class="privnote"><b>Private window.</b> Cookies, site data and history from this window are kept in memory only and deleted when you close it. Extensions are off. Your internet provider and websites can still see your IP address.</p>
<form class="search" action="INTERNAL/go" method="get"><input name="q" placeholder="Search privately or enter address" autocomplete="off" spellcheck="false" autofocus aria-label="Search"></form>
</main>"#
            .replace("INTERNAL", INTERNAL_BASE);
        return page_doc(look, "New private tab", "private", "", &body);
    }
    let mut tiles = String::new();
    for b in s.data.bookmarks.iter().take(24) {
        tiles.push_str(&format!(
            "<div class=\"tilewrap\"><a class=\"tile\" href=\"{}\" title=\"{}\"><span class=\"ticon\">{}</span><span class=\"tlabel\">{}</span></a>\
             <button class=\"tremove\" title=\"Remove\" aria-label=\"Remove\" onclick=\"api('bookmark/delete',{{id:{}}}).then(()=>this.parentNode.remove())\">×</button></div>",
            esc(&b.url),
            esc(&b.url),
            esc(&site_letter(&b.title, &b.url)),
            esc(&b.title),
            b.id
        ));
    }
    let lvl = match s.data.settings.adblock_level.as_str() { "ads" => "Ads only", "strict" => "Strict", _ => "Standard" };
    let body = r#"<main>
<form class="search" action="INTERNAL/go" method="get"><input name="q" placeholder="Search or enter address" autocomplete="off" spellcheck="false" autofocus aria-label="Search"></form>
<section class="speed" aria-label="Bookmarks">
  <div class="tiles" id="tiles">TILES<button class="tile add" type="button" onclick="$('addbox').hidden=false;$('addurl').focus()"><span class="ticon">+</span><span class="tlabel">Add</span></button></div>
  <form class="addbox" id="addbox" hidden autocomplete="off">
    <div class="folderhead"><b>Add bookmark</b><button type="button" onclick="$('addbox').hidden=true" aria-label="Cancel">×</button></div>
    <div class="addfields">
      <label>Name <input id="addtitle" maxlength="120" placeholder="Optional" spellcheck="false"></label>
      <label>Address <input id="addurl" placeholder="youtube.com" spellcheck="false" required></label>
      <button type="submit" class="addgo">Add</button>
    </div>
  </form>
</section>
<ul class="facts" FACTSHIDE>
  <li><b>Ads & trackers blocked.</b> Shield level: LEVEL. Click the shield to change it or pause it on a site.</li>
  <li><b>Private and Tor tabs.</b> Ctrl+Shift+N for a private tab, Alt+Shift+N for a Tor tab.</li>
  <li><b>Extensions.</b> Install from the Chrome Web Store with “Add to Goat” in the address bar.</li>
  <li><b>Your rules.</b> Themes, blocker level and more in <a href="INTERNAL/settings">Settings</a>.</li>
</ul>
</main>
<script>
$('addbox').addEventListener('submit', async (e) => {
  e.preventDefault();
  let u = $('addurl').value.trim(); if (!u) return;
  if (!/^[a-z]+:/i.test(u)) u = 'https://' + u;
  let t = $('addtitle').value.trim(); if (!t) { try { t = new URL(u).hostname.replace(/^www\./, ''); } catch (_) { t = u; } }
  if (await api('bookmark/add', {title: t, url: u})) location.reload();
});
</script>"#
        .replace("INTERNAL", INTERNAL_BASE)
        .replace("TILES", &tiles)
        .replace("LEVEL", lvl)
        .replace("FACTSHIDE", if s.data.bookmarks.is_empty() { "" } else { "hidden" });
    page_doc(look, "New tab", if s.data.bookmarks.is_empty() { "" } else { "has-bookmarks" }, "", &body)
}

fn tor_page(look: &Look) -> Resp {
    let body = r#"<main>
<p class="privnote"><b>Tor window.</b> Pages in this window go through the Tor network, so sites can't see your IP address. Like a private tab, nothing is saved and extensions are off.</p>
<div class="torcard" id="connect">
  <div id="st">Starting…</div>
  <div class="bar"><i id="fill"></i></div>
  <button id="retry" hidden onclick="api('tor/start').then(poll)">Try again</button>
</div>
<form id="search" class="search" action="INTERNAL/go" method="get" hidden><input name="q" placeholder="Search through Tor or enter address" autocomplete="off" spellcheck="false" aria-label="Search"></form>
<p class="torlinks" id="links" hidden><a href="https://check.torproject.org/">Check my Tor connection</a></p>
<p class="privnote" style="margin-top:24px">Goat hides your IP through Tor with no DNS or WebRTC leaks, but it isn't Tor Browser: for strong anonymity against fingerprinting, use Tor Browser.</p>
</main>
<script>
async function poll() {
  const r = await fetch('INTERNAL/api/tor/status', {method: 'POST', headers: {'X-Goatr': '1'}, body: '{}'});
  const s = await r.json();
  const ready = s.phase === 'ready';
  $('st').textContent = s.message;
  $('fill').style.width = (ready ? 100 : s.progress) + '%';
  $('retry').hidden = !(s.phase === 'error' || s.phase === 'off');
  $('connect').hidden = ready; $('search').hidden = !ready; $('links').hidden = !ready;
  if (ready) document.querySelector('#search input').focus(); else setTimeout(poll, 600);
}
api('tor/start').then(poll);
</script>"#
        .replace("INTERNAL", INTERNAL_BASE);
    page_doc(look, "Tor tab", "private", "", &body)
}

/// Goat's "can't reach this page" (replaces the engine's Edge-branded one).
fn net_error(look: &Look, url: &str, code: i32) -> Resp {
    let host = short_host(url);
    let host = if host.is_empty() { url.to_string() } else { host };
    let (head, why) = match code {
        13 => ("Can't find this site", "The address may be mistyped, or your DNS can't look it up."),
        12 | 6 => ("Can't connect to this site", "The site refused the connection or isn't reachable right now."),
        7 => ("This site took too long to answer", "It may be down or very slow."),
        9 | 10 | 11 => ("The connection was interrupted", "Your network may have dropped, or the site closed the connection."),
        1 | 2 | 3 | 4 | 5 => ("This site's connection isn't secure", "Its certificate is invalid, expired or revoked, so Goat stopped before sending anything."),
        15 => ("This page redirects in a loop", "Try clearing this site's cookies."),
        17 | 18 => ("This page needs you to sign in", "The site or a proxy asked for a password Goat couldn't give."),
        _ => ("Can't reach this page", "Something went wrong loading it."),
    };
    let js = serde_json::to_string(url).unwrap_or_default();
    let body = format!(r#"<main class="neterr">
<h1>{head}</h1>
<p class="host">{h}</p>
<p class="why">{why}</p>
<ul class="tips"><li>Check the address and your internet connection</li><li>Turn off a VPN or proxy if you use one</li><li>Try again in a moment</li></ul>
<div class="acts"><button class="pri" onclick="location.replace(U)">Try again</button></div>
</main>
<script>const U = {js};</script>"#, h = esc(&host));
    let css = r#"
.neterr { max-width: 520px; margin: 14vh auto 0; text-align: center; }
.neterr h1 { margin: 0 0 6px; animation: rise .5s cubic-bezier(.34,1.56,.64,1) both; font-size: 26px; }
.neterr .host { margin: 0 0 14px; font-weight: 600; color: var(--accent); word-break: break-all; }
.neterr .why { color: var(--muted); margin: 0 0 16px; }
.neterr .tips { text-align: left; display: inline-block; color: var(--muted); margin: 0 0 22px; padding-left: 20px; line-height: 1.7; }
@keyframes rise { from { transform: scale(.9); opacity: 0 } }
.neterr .acts button { font: inherit; font-size: 14px; font-weight: 600; color: #fff; background: var(--accent); border: 0; border-radius: 10px; padding: 9px 24px; cursor: pointer; transition: transform .25s cubic-bezier(.34,1.56,.64,1), filter .15s; }
.neterr .acts button:hover { transform: scale(1.06); filter: brightness(1.1); }
.neterr .acts button:active { transform: scale(.96); }
"#;
    page_doc(look, &format!("Can't reach {host}"), "", css, &body)
}

// ------------------------------------------------------------------ history / bookmarks / downloads

fn fmt_ago(t: std::time::SystemTime) -> String {
    let secs = t.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        0..=119 => "just now".into(),
        120..=7199 => format!("{} minutes ago", secs / 60),
        7200..=172_799 => format!("{} hours ago", secs / 3600),
        _ => format!("{} days ago", secs / 86400),
    }
}

fn fmt_day(ts: i64) -> String {
    let d = Local.timestamp_opt(ts, 0).single().unwrap_or_else(Local::now);
    let today = Local::now().date_naive();
    let day = d.date_naive();
    let date = if d.format("%Y").to_string() == Local::now().format("%Y").to_string() {
        d.format("%A, %B %-d").to_string()
    } else {
        d.format("%A, %B %-d, %Y").to_string()
    };
    if day == today {
        format!("Today · {date}")
    } else if Some(day) == today.pred_opt() {
        format!("Yesterday · {date}")
    } else {
        date
    }
}

fn fmt_time(ts: i64) -> String {
    Local.timestamp_opt(ts, 0).single().map(|d| d.format("%-I:%M %p").to_string()).unwrap_or_default()
}

const LIST_JS: &str = r#"<script>
// Live search: hide rows (and empty day groups) that don't match.
$('q') && $('q').addEventListener('input', () => {
  const q = $('q').value.toLowerCase();
  let n = 0;
  document.querySelectorAll('.hday li').forEach(li => { const m = li.dataset.f.includes(q); li.hidden = !m; if (m) n++; });
  document.querySelectorAll('.hlist section').forEach(s => s.hidden = !s.querySelector('li:not([hidden])'));
  const c = $('count'); if (c && q) c.textContent = n + ' matches'; else if (c) c.textContent = c.dataset.total;
});
</script>"#;

fn history(s: &Store, look: &Look, q: &str) -> Resp {
    let mut list = String::new();
    let mut last_day = String::new();
    let mut n = 0;
    for v in s.data.history.iter().rev().take(3000) {
        n += 1;
        let day = fmt_day(v.time);
        if day != last_day {
            if !last_day.is_empty() {
                list.push_str("</ul></section>");
            }
            list.push_str(&format!("<section><h2>{}</h2><ul class=\"hday\">", esc(&day)));
            last_day = day;
        }
        let host = short_host(&v.url);
        let title = if v.title.is_empty() { host.clone() } else { v.title.clone() };
        let key = serde_json::to_string(&(v.url.as_str(), v.time)).unwrap_or_default();
        list.push_str(&format!(
            "<li data-f=\"{}\"><input type=\"checkbox\" data-k=\"{}\" aria-label=\"Select\"><time class=\"htime\">{}</time>\
             <a href=\"{}\" class=\"hlink\" title=\"{}\"><span class=\"hico\">{}</span><span class=\"htitle\">{}</span><span class=\"hhost\">{}</span></a>\
             <button class=\"x\" title=\"Remove from history\" onclick=\"api('history/delete_many',{{items:[{}]}}).then(()=>{{const s=this.closest('section');this.parentNode.remove();if(!s.querySelector('li'))s.remove();}})\">×</button></li>",
            esc(&format!("{} {}", title, v.url).to_lowercase()),
            esc(&key),
            fmt_time(v.time),
            esc(&v.url),
            esc(&v.url),
            esc(&site_letter(&title, &v.url)),
            esc(&title),
            esc(&host),
            esc(&key),
        ));
    }
    if !last_day.is_empty() {
        list.push_str("</ul></section>");
    }
    let total = format!("{} visits saved", s.data.history.len());
    let empty = if n == 0 {
        if s.data.settings.save_history {
            "<p class=\"hempty\">No history yet. Pages you visit in normal tabs show up here (private and Tor tabs never do).</p>"
        } else {
            "<p class=\"hempty\">History is turned off in Settings.</p>"
        }
    } else {
        ""
    };
    let body = format!(
        r#"<div class="wrap">
  <header class="hhead"><h1>History</h1><input id="q" type="search" placeholder="Search history" spellcheck="false" autocomplete="off" value="{}" aria-label="Search history"></header>
  {}
  <div class="hbar">
    <span class="muted" id="count" data-total="{total}">{total}</span>
    <span class="hacts">
      <button id="delsel" hidden>Delete selected</button>
      <select id="range" aria-label="Clear history from">
        <option value="">Clear history…</option>
        <option value="hour">Last hour</option>
        <option value="day">Last 24 hours</option>
        <option value="week">Last 7 days</option>
        <option value="month">Last 30 days</option>
        <option value="all">All time</option>
      </select>
      <button onclick="location.href='{INTERNAL_BASE}/settings#privacy'" title="Choose whether history is kept">Settings</button>
    </span>
  </div>
  <main id="list" class="hlist">{list}</main>
  {empty}
</div>
{LIST_JS}
<script>
const sel = () => [...document.querySelectorAll('.hday input:checked')];
document.addEventListener('change', (e) => {{
  if (e.target.matches('.hday input')) {{ const n = sel().length; $('delsel').hidden = !n; $('delsel').textContent = 'Delete selected (' + n + ')'; }}
}});
$('delsel').onclick = async () => {{
  const items = sel().map(c => JSON.parse(c.dataset.k));
  if (await api('history/delete_many', {{items}})) location.reload();
}};
$('range').onchange = async () => {{
  const r = $('range').value; if (!r) return;
  if (r !== 'all' || confirm('Clear all browsing history?')) {{ await api('history/clear_range', {{range: r}}); location.reload(); }}
  else $('range').value = '';
}};
if ($('q').value) $('q').dispatchEvent(new Event('input'));
</script>"#,
        esc(q),
        navlinks("history"),
    );
    page_doc(look, "History", "historypage", HISTORY_CSS, &body)
}

fn bookmarks(s: &Store, look: &Look) -> Resp {
    let mut rows = String::new();
    for b in &s.data.bookmarks {
        rows.push_str(&format!(
            "<li data-f=\"{}\"><a href=\"{}\" class=\"hlink\" title=\"{}\"><span class=\"hico\">{}</span><span class=\"htitle\">{}</span><span class=\"hhost\">{}</span></a>\
             <button class=\"x\" title=\"Edit\" onclick=\"edit({},{},{})\">✎</button>\
             <button class=\"x\" title=\"Delete\" onclick=\"api('bookmark/delete',{{id:{}}}).then(()=>this.parentNode.remove())\">×</button></li>",
            esc(&format!("{} {}", b.title, b.url).to_lowercase()),
            esc(&b.url),
            esc(&b.url),
            esc(&site_letter(&b.title, &b.url)),
            esc(&b.title),
            esc(&short_host(&b.url)),
            b.id,
            esc(&serde_json::to_string(&b.title).unwrap_or_default()),
            esc(&serde_json::to_string(&b.url).unwrap_or_default()),
            b.id
        ));
    }
    let total = format!("{} bookmarks", s.data.bookmarks.len());
    let list = if rows.is_empty() {
        "<p class=\"hempty\">No bookmarks yet. Click the ☆ in the address bar or press Ctrl+D.</p>".to_string()
    } else {
        format!("<section><ul class=\"hday\">{rows}</ul></section>")
    };
    let body = format!(
        r#"<div class="wrap">
  <header class="hhead"><h1>Bookmarks</h1><input id="q" type="search" placeholder="Search bookmarks" spellcheck="false" autocomplete="off" aria-label="Search bookmarks"></header>
  {}
  <div class="hbar"><span class="muted" id="count" data-total="{total}">{total}</span><span class="hacts"></span></div>
  <main class="hlist">{list}</main>
</div>
{LIST_JS}
<script>
function edit(id, title, url) {{
  const t = prompt('Name', title); if (t === null) return;
  const u = prompt('Address', url); if (u === null) return;
  api('bookmark/edit', {{id, title: t, url: u}}).then(() => location.reload());
}}
</script>"#,
        navlinks("bookmarks"),
    );
    page_doc(look, "Bookmarks", "historypage", HISTORY_CSS, &body)
}

fn downloads(s: &Store, look: &Look) -> Resp {
    let mut rows = String::new();
    for d in s.data.downloads.iter().rev() {
        let name = std::path::Path::new(&d.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| d.url.rsplit('/').next().unwrap_or("download").to_string());
        let (status, actions) = match d.state.as_str() {
            "done" => (
                "Completed",
                format!(
                    "<button class=\"rowbtn\" onclick=\"api('downloads/open',{{id:{0}}})\">Open</button><button class=\"rowbtn\" onclick=\"api('downloads/show',{{id:{0}}})\">Show in folder</button>",
                    d.id
                ),
            ),
            "failed" => ("Failed or cancelled", String::new()),
            _ => ("Downloading…", String::new()),
        };
        rows.push_str(&format!(
            "<li data-f=\"{}\"><span class=\"hico\">{}</span><span class=\"grow\"><b>{}</b><span class=\"sub\">{} · {} · {}</span></span>{actions}</li>",
            esc(&name.to_lowercase()),
            esc(&name.chars().next().unwrap_or('•').to_uppercase().to_string()),
            esc(&name),
            status,
            esc(&short_host(&d.url)),
            esc(&fmt_day(d.time)),
        ));
    }
    let active = s.active_downloads() > 0;
    let total = format!("{} downloads", s.data.downloads.len());
    let list = if rows.is_empty() {
        "<p class=\"hempty\">Files you download show up here.</p>".to_string()
    } else {
        format!("<section><ul class=\"hday\">{rows}</ul></section>")
    };
    let body = format!(
        r#"<div class="wrap">
  <header class="hhead"><h1>Downloads</h1><input id="q" type="search" placeholder="Search downloads" spellcheck="false" autocomplete="off" aria-label="Search downloads"></header>
  {}
  <div class="hbar"><span class="muted" id="count" data-total="{total}">{total}</span>
    <span class="hacts"><button onclick="api('downloads/clear').then(()=>location.reload())">Clear list</button></span></div>
  <main class="hlist">{list}</main>
</div>
{LIST_JS}
{}"#,
        navlinks("downloads"),
        if active { "<script>setTimeout(()=>location.reload(),1500)</script>" } else { "" }
    );
    page_doc(look, "Downloads", "historypage", HISTORY_CSS, &body)
}

// ------------------------------------------------------------------ settings (Goat layout)

fn toggle(key: &str, on: bool) -> String {
    format!("<input type=\"checkbox\" class=\"toggle\" data-key=\"{key}\" {}>", if on { "checked" } else { "" })
}

fn radio(name: &str, key: &str, value: &str, cur: &str, title: &str, sub: &str) -> String {
    format!(
        "<label><input type=\"radio\" name=\"{name}\" value=\"{value}\" data-key=\"{key}\" {}> <span><b>{title}</b>{}</span></label>",
        if value == cur { "checked" } else { "" },
        if sub.is_empty() { String::new() } else { format!("<small>{sub}</small>") }
    )
}

fn theme_card(id: &str, name: &str, sub: &str, cur: &str, vars: &str) -> String {
    let system = id == "system";
    format!(
        "<label class=\"theme-card{}\" style=\"{vars}\"><input type=\"radio\" name=\"theme\" value=\"{id}\" data-key=\"theme\" data-reload=\"1\" {}>\
         <span class=\"swatch\"><span class=\"bar\"><i></i><i></i></span><span class=\"nav\"><b></b></span><span class=\"page\"><em></em><u></u></span></span>\
         <span class=\"label\">{name}<small>{sub}</small></span></label>",
        if system { " system" } else { "" },
        if cur == id { "checked" } else { "" },
    )
}

fn settings(s: &Store, look: &Look, sh: &Shared) -> Resp {
    let st = &s.data.settings;
    let engines: String = ENGINES
        .iter()
        .map(|(id, name, _)| format!("<option value=\"{id}\"{}>{name}</option>", if *id == st.search_engine { " selected" } else { "" }))
        .collect();
    let custom_home = st.home_page != "newtab";

    let mut themes_html = theme_card("system", "System", "Auto", &st.theme, "");
    for t in themes::all() {
        let vars = format!("--bg:{};--bg-2:{};--bg-3:{};--line:{};--muted:{};--accent:{};--accent-2:{}", t.bg, t.bg2, t.bg3, t.line, t.muted, t.accent, t.accent2);
        let mut card = theme_card(t.id, &esc(t.name), if t.dark { "Dark" } else { "Light" }, &st.theme, &vars);
        if t.id.starts_with("custom-") {
            card = card.replace("</label>", &format!("<button type=\"button\" class=\"edit\" onclick=\"event.preventDefault();openEditor('{}')\">Edit</button></label>", t.id));
        }
        themes_html.push_str(&card);
    }
    themes_html.push_str("<button type=\"button\" class=\"theme-new\" onclick=\"openEditor('new')\"><span class=\"plus\" aria-hidden=\"true\">+</span><span>Create theme</span></button>");
    let builtin_json = serde_json::to_string(&THEMES.iter().map(|t| (t.id, serde_json::json!({"name": t.name, "bg": t.bg, "surface": t.bg2, "text": t.text, "accent": t.accent, "accent2": t.accent2}))).collect::<std::collections::BTreeMap<_, _>>()).unwrap_or_default();
    let custom_json = serde_json::to_string(&st.custom_themes).unwrap_or_default();

    let ab = &sh.adblock;
    let abstatus = if !ab.ready() {
        "Loading blocklists…".to_string()
    } else {
        format!(
            "Engine: adblock-rust · {} rules at this level · {} blocked all-time · lists updated {}",
            ab.domain_count(),
            st.adblock_total,
            ab.last_updated().map(fmt_ago).unwrap_or_else(|| "— downloading".into())
        )
    };
    let chips: String = if st.adblock_allow.is_empty() {
        "<li class=\"empty\">No sites yet.</li>".into()
    } else {
        st.adblock_allow
            .iter()
            .map(|h| {
                format!(
                    "<li>{}<button class=\"x\" title=\"Remove\" onclick=\"api('adblock/allow_remove',{{url:{}}}).then(()=>this.parentNode.remove())\">×</button></li>",
                    esc(h),
                    esc(&serde_json::to_string(h).unwrap_or_default())
                )
            })
            .collect()
    };

    let mut exts = String::new();
    for e in extinstall::list() {
        let folder_js = esc(&serde_json::to_string(&e.folder).unwrap_or_default());
        let in_private = st.private_extensions.contains(&e.folder);
        let enabled = !st.disabled_extensions.contains(&e.folder);
        exts.push_str(&format!(
            "<div class=\"ext{}\" id=\"ext-{}\"><span class=\"exticon\">{}</span><span class=\"extinfo\"><b>{}</b><small>{}{}</small></span>\
             <label class=\"extpriv\" title=\"Turn this extension on or off\"><span>On</span>\
             <input type=\"checkbox\" class=\"toggle\" {} onchange=\"extenable({}, this.checked, this)\"></label>\
             <label class=\"extpriv\" title=\"Also run this extension in private windows\"><span>Allow in private</span>\
             <input type=\"checkbox\" class=\"toggle\" {} onchange=\"privext({}, this.checked)\"></label>\
             <button class=\"x\" title=\"Remove\" onclick=\"rmext({})\">{}</button></div>",
            if enabled { "" } else { " off" },
            esc(&e.folder),
            esc(&e.name.chars().next().unwrap_or('?').to_uppercase().to_string()),
            esc(&e.name),
            if e.version.is_empty() { String::new() } else { format!("v{} · ", esc(&e.version)) },
            esc(if !e.valid { "Broken: no manifest.json" } else if e.description.is_empty() { &e.folder } else { &e.description }),
            if enabled { "checked" } else { "" },
            folder_js,
            if in_private { "checked" } else { "" },
            folder_js,
            folder_js,
            svg(IC_TRASH),
        ));
    }
    if exts.is_empty() {
        exts = "<p class=\"empty\">No extensions installed.</p>".into();
    }

    let ts = sh.tor.state();
    let torstate = match ts.phase.as_str() {
        "ready" => "<small id=\"torstate\" class=\"good\">Connected</small>".to_string(),
        "off" => "<small id=\"torstate\" class=\"muted\">Off. Opening a Tor tab downloads Tor (about 20 MB) from torproject.org the first time.</small>".to_string(),
        "error" => format!("<small id=\"torstate\" class=\"bad\">{}</small>", esc(&ts.message)),
        _ => format!("<small id=\"torstate\" class=\"muted\">{}</small>", esc(&ts.message)),
    };

    let body = r##"<nav class="side">
  <div class="brand">goat<span>.</span> settings</div>
  <a href="#general">General</a>
  <a href="#profiles">Profiles</a>
  <a href="#appearance">Appearance</a>
  <a href="#privacy">Privacy</a>
  <a href="#adblock">Ad blocker</a>
  <a href="#cookies">Cookies</a>
  <a href="#extensions">Extensions</a>
  <a href="#performance">Performance</a>
  <a href="#proxy">Proxy</a>
  <a href="#tor">Tor</a>
  <a href="#data">Clear data</a>
  <a href="#about">About</a>
</nav>
<div class="content">
  <section id="general">
    <h2>General</h2>
    <div class="card">
      <label class="row"><span><b>Search engine</b><small>Used by the address bar and new tab page.</small></span>
        <select data-key="search_engine">ENGINES</select>
      </label>
      <div class="row col">
        <span><b>Home page</b><small>Opened by the home shortcut (Alt+Home) and when Goat starts.</small></span>
        <div class="radios">
          <label><input type="radio" name="home" value="newtab" HOME_NEWTAB> <span><b>New tab page</b></span></label>
          <label><input type="radio" name="home" value="custom" HOME_CUSTOM> <span><b>A specific page</b></span></label>
        </div>
        <div class="grid"><label class="wide">Address <input id="homeurl" placeholder="https://example.com" spellcheck="false" value="HOMEURL"></label></div>
      </div>
      <label class="row"><span><b>Paste images when uploading</b><small>When a site asks for an image and there's one in your clipboard, Goat offers to paste it instead of picking a file. Applies to pages opened from now on.</small></span>
        TOGGLE_PASTE
      </label>
      <label class="row"><span><b>HTTPS-only mode</b><small>Upgrades http:// to https://. To use http:// on a site anyway, type its http:// address in the address bar.</small></span>
        TOGGLE_HTTPS
      </label>
      <div class="row col" id="dnsrow">
        <label class="dnshead"><span><b>Encrypted DNS</b><small>Site lookups are sent encrypted (DNS over HTTPS), so your internet provider can't see or change them. <em class="restart" data-restart="dns" hidden>Restart Goat to apply.</em></small></span>
          <select data-key="dns_provider" id="dnsprovider" data-restartkey="dns">DNSOPTS</select>
        </label>
        <div class="grid" id="dnsnext" hidden>
          <label class="wide">NextDNS configuration ID <input data-key="dns_nextdns_id" data-restartkey="dns" placeholder="abc123" spellcheck="false" value="NEXTID"></label>
        </div>
        <div class="grid" id="dnscustom" hidden>
          <label class="wide">DNS-over-HTTPS address <input data-key="dns_custom" data-restartkey="dns" placeholder="https://dns.example.com/dns-query" spellcheck="false" value="DNSCUSTOM"></label>
        </div>
        <div class="dnsstatus"><span id="dnsstate" class="muted">DNSSTATE</span>
          <span class="dnsbtns"><button type="button" id="dnscheck" DNSCHECKHIDE onclick="window.open(this.dataset.u)" data-u="DNSCHECK">Open check page</button></span>
        </div>
      </div>
    </div>
  </section>

  <section id="profiles">
    <h2>Profiles</h2>
    <div class="card">
      <p class="hint">Each profile has its own cookies, history, bookmarks, extensions and settings, in its own window. You're in: <b>CURPROFILE</b>.</p>
      <div class="extlist">PROFILELIST</div>
      <label class="row"><span><b>When Goat starts</b><small>With more than one profile, Goat can ask which one to use, or always open the same one.</small></span>
        <select id="startprof" onchange="setStartProfile(this.value)">STARTPROFOPTS</select>
      </label>
      <form class="inline" id="profform">
        <input id="profname" placeholder="New profile name, e.g. Work" maxlength="40" spellcheck="false">
        <button>Add and open</button>
      </form>
    </div>
  </section>

  <section id="appearance">
    <h2>Appearance</h2>
    <div class="card">
      <div class="row col">
        <span><b>Theme</b><small>Applies to the toolbar, Goat's own pages, and tells websites to use dark or light mode.</small></span>
        <div class="themes" role="radiogroup" aria-label="Theme">THEMES</div>
      </div>
      <div class="row col" id="editor" hidden>
        <span><b id="editortitle">New theme</b><small>Pick five colors. The rest are worked out for you.</small></span>
        <div class="editor">
          <div class="fields">
            <label class="wide">Name <input id="ed-name" maxlength="30" placeholder="My theme" spellcheck="false"></label>
            <label class="wide">Start from <select id="ed-base"></select></label>
            <label class="color">Background <input type="color" id="ed-bg"></label>
            <label class="color">Surface <input type="color" id="ed-surface"></label>
            <label class="color">Text <input type="color" id="ed-text"></label>
            <label class="color">Accent <input type="color" id="ed-accent"></label>
            <label class="color">Accent 2 <input type="color" id="ed-accent2"></label>
            <p class="warnmsg" id="ed-warn" hidden></p>
          </div>
          <div class="preview" id="ed-preview" aria-label="Preview">
            <div class="pv-strip"><i class="on"></i><i></i><i></i></div>
            <div class="pv-nav"><b></b><u></u></div>
            <div class="pv-page"><div class="pv-title">Aa</div><div class="pv-text"></div><div class="pv-text short"></div><div class="pv-btn">Button</div></div>
          </div>
        </div>
        <div class="actions">
          <button class="primary" id="ed-save">Save and use</button>
          <button id="ed-cancel">Cancel</button>
          <button class="danger" id="ed-delete" hidden>Delete theme</button>
        </div>
      </div>
      <label class="row"><span><b>Gradients</b><small>Soft colour gradients on the toolbar, pages and buttons.</small></span>
        TOGGLE_GRAD
      </label>
      <div class="row col">
        <span><b>Animations</b><small>Tab, panel and page motion.</small></span>
        <div class="radios">
          ANIM_ON
          ANIM_OFF
        </div>
      </div>
      <label class="row"><span><b>Bookmarks bar</b><small>Shows your bookmarks under the address bar (Ctrl+Shift+B).</small></span>
        TOGGLE_BAR
      </label>
    </div>
  </section>

  <section id="privacy">
    <h2>Privacy</h2>
    <div class="card">
      <div class="row col">
        <span><b>Profile</b><small>Sets the speed-vs-privacy switches in one click: preloading, font blocking, fingerprint protection, ad blocking level and DNS. You can still change any of them below. <em class="restart" data-restart="profile" hidden>Restart Goat to apply fully.</em></small></span>
        <div class="profiles" role="radiogroup" aria-label="Settings profile">PROFILES</div>
      </div>
    </div>
    <div class="card">
      <div class="row col">
        <span><b>Fingerprint protection</b><small>Stops sites from identifying your device by its hardware, GPU and browser details. Applies to tabs you open after changing it.</small></span>
        <div class="radios">
          FP_OFF
          FP_STD
          FP_STRICT
        </div>
      </div>
      <div class="row col">
        <span><b>Passkeys and security keys</b><small>Some sites start a passkey sign-in on their own, which makes Windows pop up "use your security key".</small></span>
        <div class="radios">
          WA_ASK
          WA_ALLOW
          WA_BLOCK
        </div>
      </div>
      <label class="row"><span><b>History</b><small>Pages you visit in normal tabs. Private and Tor tabs are never saved. <a href="INTERNAL/history" class="inlink">Open history</a></small></span>
        <select data-key="save_history" data-bool="1">
          <option value="1" HIST_ON>Save history</option>
          <option value="0" HIST_OFF>Don't save history</option>
        </select>
      </label>
      <label class="row"><span><b>Remove tracking from links</b><small>Strips tags like utm_source, fbclid and gclid from addresses you open.</small></span>
        TOGGLE_STRIP
      </label>
      <label class="row"><span><b>Trim referrers</b><small>When you open another site, it isn't told which page you came from. Things loaded from other servers only see the site name, not the page.</small></span>
        TOGGLE_REF
      </label>
      <label class="row"><span><b>No background preloading</b><small>Sites can't load pages you haven't clicked yet, and Goat doesn't open early connections. Pages may open a little slower. <em class="restart" data-restart="prefetch" hidden>Restart Goat to apply fully.</em></small></span>
        TOGGLE_PREFETCH
      </label>
      <label class="row"><span><b>Block fonts from other sites</b><small>Stops font services (like Google Fonts) from seeing which sites you visit. Some icons may show as boxes; pause the ad blocker on a site to allow them there.</small></span>
        TOGGLE_FONTS
      </label>
      <div class="row"><span><b>Private tabs</b><small>Cookies and site data stay in memory and are deleted when Goat closes. Extensions are off. Ctrl+Shift+N.</small></span></div>
      <div class="row"><span><b>Always on</b><small>Global Privacy Control and Do Not Track are sent to every site, WebRTC can't leak your IP address, and Edge's tracking prevention is set to Balanced.</small></span></div>
    </div>
  </section>

  <section id="adblock">
    <h2>Ad blocker</h2>
    <div class="card">
      <label class="row"><span><b>Block ads and trackers</b><small id="abstatus">ABSTATUS</small></span>
        TOGGLE_AB
      </label>
      <div class="row col">
        <span><b>Blocking level</b></span>
        <div class="radios">
          LVL_ADS
          LVL_STD
          LVL_STRICT
        </div>
      </div>
      <label class="row"><span><b>Don't block extensions</b><small>Extensions' own requests (password managers, Roblox tools, sync) skip the ad blocker, so it can't break them. Ads on websites are still blocked.</small></span>
        TOGGLE_SKIPEXT
      </label>
      <div class="row">
        <span><b>Update blocklists</b><small>EasyList, Peter Lowe's list, EasyPrivacy, EasyList Cookie and Fanboy's Annoyances. They refresh every 4 days on their own.</small></span>
        <button id="abrefresh" onclick="api('adblock/update').then(()=>{this.textContent='Updating…'})">Update now</button>
      </div>
    </div>
    <h3>Allowed sites</h3>
    <div class="card">
      <p class="hint">The ad blocker is paused on these sites. You can also click the shield in the toolbar.</p>
      <form class="inline" id="allowform">
        <input id="allowinput" placeholder="example.com" spellcheck="false">
        <button>Add</button>
      </form>
      <ul class="chips" id="allowlist">CHIPS</ul>
    </div>
    <h3>Custom filters</h3>
    <div class="card">
      <p class="hint">One rule per line, Adblock Plus / uBlock syntax. Examples: <code>||ads.example.com^</code> blocks a domain, <code>example.com##.banner</code> hides an element, <code>@@||site.com^</code> allows it. Lines starting with <code>!</code> are comments.</p>
      <textarea id="rules" rows="7" spellcheck="false">RULES</textarea>
      <div class="actions"><button class="primary" id="rulesave">Save filters</button><span class="saved" id="rulesaved"></span></div>
    </div>
  </section>

  <section id="cookies">
    <h2>Cookies</h2>
    <div class="card">
      <label class="row"><span><b>Block third-party cookies</b><small>Stops trackers that follow you across sites. Some embedded logins may need this off. <em class="restart" data-restart="cookies" hidden>Restart Goat to apply fully.</em></small></span>
        TOGGLE_3PC
      </label>
      <label class="row"><span><b>Delete cookies when Goat closes</b><small>Signs you out of sites on every exit.</small></span>
        TOGGLE_CLEAR
      </label>
    </div>

    <h3>Cookie editor</h3>
    <div class="card">
      <div class="toolbar">
        <input id="cookiesearch" placeholder="Search sites or cookie names" spellcheck="false" aria-label="Search cookies">
        <span class="muted" id="cookiecount">Loading…</span>
        <button id="cookieadd">Add cookie</button>
        <button id="cookiereload">Refresh</button>
        <button class="danger" id="cookieclear">Delete all</button>
      </div>
      <form id="cookieform" class="cookieform" hidden>
        <b id="cookieformtitle">Add cookie</b>
        <div class="grid">
          <label>Name<input id="cf-name" spellcheck="false" required></label>
          <label>Site (domain)<input id="cf-domain" spellcheck="false" placeholder="example.com or .example.com" required></label>
          <label>Path<input id="cf-path" spellcheck="false" placeholder="/"></label>
          <label class="wide">Value<textarea id="cf-value" spellcheck="false" rows="3"></textarea></label>
          <label>Expires<input id="cf-expires" type="datetime-local"></label>
          <label>Same-site<select id="cf-samesite"><option value="lax">Lax</option><option value="strict">Strict</option><option value="none">None</option></select></label>
          <label class="checks"><span><input type="checkbox" id="cf-session"> Session (deleted when Goat closes)</span><span><input type="checkbox" id="cf-secure"> Secure (HTTPS only)</span><span><input type="checkbox" id="cf-httponly"> HttpOnly (hidden from page scripts)</span></label>
        </div>
        <div class="actions"><button type="submit" class="primary">Save cookie</button><button type="button" id="cf-cancel">Cancel</button><span class="muted" id="cf-msg"></span></div>
      </form>
      <div id="cookielist" class="cookielist"></div>
    </div>
  </section>

  <section id="extensions">
    <h2>Extensions</h2>
    <div class="card">
      <div class="toolbar">
        <button class="primary" onclick="location.href='https://chromewebstore.google.com/category/extensions'">Get extensions from Chrome Web Store</button>
        <button onclick="api('extensions/open_folder')">Open extensions folder</button>
        <button id="restart" hidden onclick="api('restart')">Restart Goat</button>
        <span class="saved" id="extmsg"></span>
      </div>
      <form class="storeform" id="storeform"><input id="storeurl" placeholder="Paste a Chrome Web Store or Edge Add-ons link" spellcheck="false"><button>Add</button></form>
      <div class="extlist">EXTS</div>
      <p class="hint">On an extension's store page, click “Add to Goat” in the address bar. New extensions turn on after a restart. Ad blockers, password managers, dark mode and other page extensions work; extension toolbar buttons and pop-ups aren't available.</p>
    </div>
  </section>

  <section id="performance">
    <h2>Performance</h2>
    <div class="card">
      <label class="row"><span><b>CPU cores</b><small>How many of your THREADS processor threads Goat may use. Fewer leaves more for games and other apps; all is fastest for Goat. <em class="restart" data-restart="cpu" hidden>Applies now to Goat; pages pick it up after a restart.</em></small></span>
        <select data-key="cpu_cores" data-number="1" data-restartkey="cpu">COREOPTS</select>
      </label>
      <label class="row"><span><b>Priority</b><small>Higher priority makes Windows run Goat first when the PC is busy.</small></span>
        <select data-key="cpu_priority" data-restartkey="cpu">
          <option value="normal" PRIO_NORMAL>Normal</option>
          <option value="above" PRIO_ABOVE>Above normal</option>
          <option value="high" PRIO_HIGH>High</option>
        </select>
      </label>
      <label class="row"><span><b>Hardware acceleration</b><small>Pages, video and scrolling use your graphics card. Turn off if pages flicker, go black or crash. <em class="restart" data-restart="gpu" hidden>Restart Goat to apply.</em></small></span>
        TOGGLE_GPU
      </label>
      <label class="row"><span><b>Audio for streaming</b><small>Plays page sound from Goat itself, so Discord and other streaming apps pick it up when you share the Goat window. Experimental.</small></span>
        TOGGLE_STREAMAUDIO
      </label>
      <label class="row"><span><b>Memory saver</b><small>Background tabs you haven't used for a while free most of their memory. They come back when you open them.</small></span>
        <select data-key="sleep_after_min" data-number="1">SLEEPOPTS</select>
      </label>
    </div>
  </section>

  <section id="proxy">
    <h2>Proxy</h2>
    <div class="card">
      <div class="row col">
        <span><b>Connection</b><small>Normal and private tabs. Tor tabs always use Tor. <em class="restart" data-restart="proxy" hidden>Restart Goat to apply.</em></small></span>
        <div class="radios">
          PX_SYSTEM
          PX_NONE
          PX_MANUAL
        </div>
        <div class="grid" id="pxfields">
          <label>Type <select data-key="proxy_type" data-restartkey="proxy"><option value="socks5" PXT_SOCKS>SOCKS5</option><option value="http" PXT_HTTP>HTTP</option></select></label>
          <label>Host <input data-key="proxy_host" data-restartkey="proxy" placeholder="127.0.0.1" spellcheck="false" value="PXHOST"></label>
          <label>Port <input data-key="proxy_port" data-restartkey="proxy" placeholder="1080" spellcheck="false" value="PXPORT"></label>
        </div>
      </div>
    </div>
  </section>

  <section id="tor">
    <h2>Tor</h2>
    <div class="card">
      <div class="row">
        <span><b>Status</b>TORSTATE</span>
        <span class="dnsbtns"><button type="button" onclick="api('tor/open')">New Tor tab</button></span>
      </div>
      <p class="hint">Tor tabs (Alt+Shift+N) go through the Tor network and hide your IP. The first time, Goat downloads the official Tor Expert Bundle from torproject.org. Tor hides your IP, but it isn't Tor Browser: logging in to sites still identifies you.</p>
    </div>
  </section>

  <section id="data">
    <h2>Clear data</h2>
    <div class="card">
      <div class="row"><span><b>Browsing history</b><small>Every page in your history.</small></span>
        <button class="danger" onclick="if(confirm('Clear all browsing history?'))api('history/clear').then(()=>{this.textContent='Cleared'})">Clear history</button></div>
      <div class="row"><span><b>Cookies, cache and site data</b><small>Signs you out of websites.</small></span>
        <button class="danger" onclick="if(confirm('Clear cookies, cache, site data and history? You will be signed out of websites.'))api('clear_all').then(()=>{this.textContent='Cleared'})">Clear everything</button></div>
    </div>
  </section>

  <section id="about">
    <h2>About</h2>
    <div class="card">
      <label class="row"><span><b>Update automatically</b><small>Goat checks GitHub for new versions, downloads them in the background, and installs them when you close it (or when you click Update in the toolbar).</small></span>
        TOGGLE_AUTOUPDATE
      </label>
      <div class="row"><span><b>Check for updates</b><small>You're on Goat VERSION.</small></span>
        <button onclick="api('updates/check');this.textContent='Checking…';setTimeout(()=>this.textContent='Check now',4000)">Check now</button></div>
      <div class="row"><span><b>Goat VERSION</b><small>A browser written in Rust: the interface is native Rust (egui), pages render with Microsoft Edge WebView2, ads are blocked by Brave's adblock-rust engine.</small></span></div>
      <div class="row"><span><b>Your data</b><small>Settings, bookmarks and history are in <code>data</code>, cookies and logins in <code>profile</code>, next to Goat.exe. Crash reports go to <code>data\crash.log</code>.</small></span>
        <button onclick="api('restart')">Restart Goat</button></div>
    </div>
  </section>
</div>
<script>
// Settings that only take effect after a restart show a note next to them.
// ---- profiles
const LOGO_COLORS = LOGOCOLORS_JSON;
// Profile logo picker: the first letter on a colour.
function pickLogo(btn, name) {
  const open = btn.closest('.ext').nextElementSibling;
  document.querySelectorAll('.logopick').forEach((e) => e.remove());
  if (open && open.classList.contains('logopick')) return; // second click closes it
  const box = document.createElement('div');
  box.className = 'logopick';
  const cur = { emoji: '', color: btn.dataset.color || '' };
  const save = async () => {
    await api('profiles/logo', { title: name, url: JSON.stringify(cur) });
    btn.dataset.emoji = cur.emoji; btn.dataset.color = cur.color;
    if (cur.color) btn.style.background = cur.color;
    btn.textContent = ((name || 'Default')[0] || '?').toUpperCase();
  };
  const co = document.createElement('div'); co.className = 'lp-colors';
  LOGO_COLORS.forEach((c) => {
    const b = document.createElement('button');
    b.style.background = c; b.title = c;
    b.onclick = (ev) => { ev.preventDefault(); cur.color = c; save(); };
    co.appendChild(b);
  });
  box.append(co);
  btn.closest('.ext').after(box);
}
async function setStartProfile(v) {
  if (v === '*ask') await api('profiles/startup', { title: '', url: 'ask' });
  else await api('profiles/startup', { title: v, url: '' });
}
async function openProfile(name) { await api('profiles/open', {title: name}); }
async function deleteProfile(name) {
  if (!confirm('Delete the profile “' + name + '”? Its cookies, history, bookmarks and extensions are deleted. This can\'t be undone.')) return;
  if (await api('profiles/delete', {title: name})) location.reload(); else alert('Close that profile\'s window first.');
}
$('profform').addEventListener('submit', async (e) => {
  e.preventDefault();
  const n = $('profname').value.trim(); if (!n) return;
  if (await api('profiles/add', {title: n})) location.reload();
});
// ---- theme editor (Goat's)
const BUILTIN = BUILTIN_JSON;
let CUSTOM = CUSTOM_JSON;
let editing = null;
const ED_KEYS = ['bg', 'surface', 'text', 'accent', 'accent2'];
const hexRgb = (h) => [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16));
const mixHex = (a, b, t) => '#' + hexRgb(a).map((x, i) => Math.round(x + (hexRgb(b)[i] - x) * t).toString(16).padStart(2, '0')).join('');
const lum = (h) => { const f = (c) => { c /= 255; return c <= .03928 ? c / 12.92 : ((c + .055) / 1.055) ** 2.4; }; const [r, g, b] = hexRgb(h); return .2126 * f(r) + .7152 * f(g) + .0722 * f(b); };
const contrast = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + .05) / (y + .05); };
const edColors = () => Object.fromEntries(ED_KEYS.map(k => [k, $('ed-' + k).value]));
function preview() {
  const c = edColors(), el = $('ed-preview').style;
  const vars = {'--bg': c.bg, '--bg-2': c.surface, '--bg-3': mixHex(c.surface, c.text, .08), '--line': mixHex(c.surface, c.text, .15), '--text': c.text, '--muted': mixHex(c.text, c.bg, .42), '--accent': c.accent, '--accent-2': c.accent2, '--accent-ink': lum(c.accent) > .4 ? '#0b0d10' : '#ffffff'};
  for (const [k, v] of Object.entries(vars)) el.setProperty(k, v);
  const r = contrast(c.text, c.bg);
  $('ed-warn').hidden = r >= 4.5;
  $('ed-warn').textContent = 'Text contrast is ' + r.toFixed(1) + ':1. Aim for at least 4.5:1 so text stays readable.';
}
const fill = (c) => { for (const k of ED_KEYS) $('ed-' + k).value = c[k]; preview(); };
function openEditor(id) {
  editing = id;
  const ex = CUSTOM.find(t => t.id === id);
  $('editor').hidden = false;
  $('editortitle').textContent = ex ? 'Edit “' + ex.name + '”' : 'New theme';
  $('ed-name').value = ex ? ex.name : '';
  $('ed-delete').hidden = !ex;
  $('ed-base').innerHTML = '<option value="">' + (ex ? 'Keep current colors' : 'Choose a theme…') + '</option>' + Object.entries(BUILTIN).map(([k, t]) => '<option value="' + k + '">' + t.name + '</option>').join('');
  fill(ex || BUILTIN[CURTHEME] || BUILTIN.midnight);
  $('editor').scrollIntoView({block: 'nearest', behavior: 'smooth'});
  $('ed-name').focus();
}
$('ed-base').addEventListener('change', (e) => { if (BUILTIN[e.target.value]) fill(BUILTIN[e.target.value]); });
ED_KEYS.forEach(k => $('ed-' + k).addEventListener('input', preview));
$('ed-cancel').onclick = () => { editing = null; $('editor').hidden = true; };
$('ed-save').onclick = async () => {
  const t = {id: editing, name: $('ed-name').value.trim() || 'My theme', ...edColors()};
  let list = [...CUSTOM];
  if (!list.some(x => x.id === editing)) { t.id = 'custom-' + Math.random().toString(36).slice(2, 10); list.push(t); }
  else list = list.map(x => x.id === editing ? t : x);
  if (await api('settings', {custom_themes: list, theme: t.id})) location.reload();
};
$('ed-delete').onclick = async () => {
  const t = CUSTOM.find(x => x.id === editing);
  if (!t || !confirm('Delete the theme “' + t.name + '”?')) return;
  const patch = {custom_themes: CUSTOM.filter(x => x.id !== editing)};
  if (CURTHEME === editing) patch.theme = 'system';
  if (await api('settings', patch)) location.reload();
};
document.querySelectorAll('input[name=proxy]').forEach(r => r.addEventListener('change', () => { $('pxfields').hidden = r.value !== 'manual'; restartNote('proxy'); }));
$('pxfields').hidden = PXMANUAL_HIDDEN;
const restartNote = (k) => document.querySelectorAll('[data-restart="' + k + '"]').forEach(e => e.hidden = false);
document.querySelectorAll('[data-restartkey]').forEach(el => el.addEventListener('change', () => restartNote(el.dataset.restartkey)));
const dnsFields = () => { const v = $('dnsprovider').value; $('dnsnext').hidden = v !== 'nextdns'; $('dnscustom').hidden = v !== 'custom'; };
$('dnsprovider').addEventListener('change', dnsFields); dnsFields();
document.querySelectorAll('.profile').forEach(b => b.addEventListener('click', async () => {
  if (await api('privacy/profile', {title: b.dataset.id})) { restartNote('profile'); setTimeout(() => location.reload(), 300); }
}));
// Save a single setting; theme/gradients/animations reload the page to restyle it.
async function save(obj, reload) { const ok = await api('settings', obj); if (ok && reload) location.reload(); return ok; }
document.querySelectorAll('[data-key]').forEach(el => el.addEventListener('change', () => {
  let v = el.type === 'checkbox' ? el.checked : el.value;
  if (el.dataset.bool) v = v === '1' || v === true || v === 'on';
  if (el.dataset.number) v = Number(v);
  save({[el.dataset.key]: v}, !!el.dataset.reload);
}));
const home = () => document.querySelector('input[name=home]:checked').value === 'custom' && $('homeurl').value.trim() ? $('homeurl').value.trim() : 'newtab';
document.querySelectorAll('input[name=home]').forEach(r => r.addEventListener('change', () => save({home_page: home()})));
$('homeurl').addEventListener('change', () => { document.querySelector('input[name=home][value=custom]').checked = true; save({home_page: home()}); });
$('allowform').addEventListener('submit', async (e) => {
  e.preventDefault(); const h = $('allowinput').value.trim(); if (!h) return;
  if (await api('adblock/allow_add', {url: h})) location.reload();
});
$('rulesave').onclick = async () => { if (await save({adblock_custom: $('rules').value})) { $('rulesaved').textContent = 'Saved'; setTimeout(() => $('rulesaved').textContent = '', 1800); } };
$('storeform').addEventListener('submit', async (e) => {
  e.preventDefault();
  const r = await fetch('INTERNAL/api/extensions/install', {method: 'POST', headers: {'X-Goatr': '1', 'Content-Type': 'application/json'}, body: JSON.stringify({url: $('storeurl').value})});
  $('extmsg').textContent = r.ok ? 'Downloading… you’ll get a message when it’s ready.' : await r.text();
});
// edge://extensions/?id=… for an extension that isn't installed here: show the list.
if (location.hash.startsWith('#ext-') && !document.getElementById(location.hash.slice(1))) location.hash = '#extensions';
async function extenable(folder, on, box) {
  await api('extensions/enable', {title: folder, url: on ? 'on' : 'off'});
  box.closest('.ext').classList.toggle('off', !on);
  $('extmsg').textContent = on ? 'Turned on.' : 'Turned off. It stays installed; switch it back on any time.';
}
async function privext(folder, on) {
  await api('extensions/private', {title: folder, url: on ? 'on' : 'off'});
  $('extmsg').textContent = on ? 'Runs in private windows opened from now on.' : 'Off in private windows opened from now on.';
}
async function rmext(folder) {
  if (!confirm('Remove this extension?')) return;
  await api('extensions/remove', {title: folder});
  $('extmsg').textContent = 'Removed. Restart Goat to finish.'; $('restart').hidden = false;
  setTimeout(() => location.reload(), 900);
}
// ---- cookie editor (the engine answers later: ask, then collect the answer by id)
async function cookieApi(body) {
  const r = await fetch('INTERNAL/api/cookies/op', {method: 'POST', headers: {'X-Goatr': '1', 'Content-Type': 'application/json'}, body: JSON.stringify(body)});
  if (!r.ok) throw new Error(await r.text());
  const {id} = await r.json();
  for (let i = 0; i < 150; i++) {
    await new Promise((res) => setTimeout(res, i < 10 ? 30 : 100));
    const a = await (await fetch('INTERNAL/api/cookies/result', {method: 'POST', headers: {'X-Goatr': '1', 'Content-Type': 'application/json'}, body: JSON.stringify({id})})).json();
    if (a.pending) continue;
    if (a.error) throw new Error(a.error);
    return a;
  }
  throw new Error('The cookie store didn\'t answer.');
}
function el(tag, props = {}, ...kids) {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === 'class') n.className = v;
    else if (k.startsWith('on')) n.addEventListener(k.slice(2), v);
    else if (k in n) n[k] = v;
    else n.setAttribute(k, v);
  }
  for (const c of kids) if (c != null) n.append(c);
  return n;
}
let cookies = [], cookieEditing = null;
const openSites = new Set();
const chost = (c) => c.domain.replace(/^\./, '');
const fmtExpiry = (c) => c.session || !(c.expires > 0) ? 'Session' : new Date(c.expires * 1000).toLocaleDateString(undefined, {year: 'numeric', month: 'short', day: 'numeric'});
async function loadCookies() {
  try { cookies = await cookieApi({op: 'list'}); renderCookies(); }
  catch (e) { $('cookiecount').textContent = ''; $('cookielist').textContent = ''; $('cookielist').append(el('p', {class: 'empty'}, "Couldn't load cookies: " + e.message)); }
}
function cookieRow(c) {
  const value = el('input', {class: 'val', value: c.value, spellcheck: false, title: 'Edit the value, then press Enter'});
  const save = async () => {
    if (value.value === c.value) return;
    try {
      await cookieApi({op: 'set', cookie: {...c, value: value.value}, original: c});
      c.value = value.value; value.classList.add('ok'); setTimeout(() => value.classList.remove('ok'), 900);
    } catch (e) { alert(e.message); value.value = c.value; }
  };
  value.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); value.blur(); } });
  value.addEventListener('change', save);
  const flags = [c.secure && 'Secure', c.http_only && 'HttpOnly', 'SameSite=' + c.same_site[0].toUpperCase() + c.same_site.slice(1)].filter(Boolean).join(' · ');
  return el('tr', {},
    el('td', {class: 'name', title: c.name}, c.name),
    el('td', {}, value),
    el('td', {class: 'muted'}, fmtExpiry(c)),
    el('td', {class: 'muted small'}, (c.path !== '/' ? c.path + ' · ' : '') + flags),
    el('td', {class: 'rowbtns'},
      el('button', {class: 'x', title: 'Edit everything', 'aria-label': 'Edit cookie ' + c.name, onclick: () => openCookieForm(c)}, '✎'),
      el('button', {class: 'x', title: 'Delete cookie', 'aria-label': 'Delete cookie ' + c.name, onclick: async () => {
        try { await cookieApi({op: 'delete', cookie: c}); cookies = cookies.filter((x) => x !== c); renderCookies(); } catch (e) { alert(e.message); }
      }}, '×')),
  );
}
function renderCookies() {
  const q = $('cookiesearch').value.trim().toLowerCase();
  const byHost = new Map();
  for (const c of cookies) {
    if (q && !chost(c).toLowerCase().includes(q) && !c.name.toLowerCase().includes(q)) continue;
    if (!byHost.has(chost(c))) byHost.set(chost(c), []);
    byHost.get(chost(c)).push(c);
  }
  const hosts = [...byHost.keys()].sort((a, b) => a.replace(/^www\./, '').localeCompare(b.replace(/^www\./, '')));
  const sites = new Set(cookies.map(chost)).size;
  $('cookiecount').textContent = `${cookies.length} cookie${cookies.length === 1 ? '' : 's'} · ${sites} site${sites === 1 ? '' : 's'}`;
  const list = $('cookielist');
  list.textContent = '';
  if (!hosts.length) { list.append(el('p', {class: 'empty'}, q ? 'No matches.' : 'No cookies stored.')); return; }
  for (const h of hosts) {
    const items = byHost.get(h);
    const isOpen = openSites.has(h) || (q && hosts.length <= 3);
    list.append(el('div', {class: 'site' + (isOpen ? ' open' : '')},
      el('button', {class: 'sitebtn', 'aria-expanded': String(!!isOpen), onclick: () => { openSites.has(h) ? openSites.delete(h) : openSites.add(h); renderCookies(); }},
        el('span', {class: 'caret'}, '›'), el('span', {class: 'host'}, h), el('span', {class: 'muted'}, String(items.length))),
      el('button', {class: 'linkdanger', onclick: async () => {
        try { await cookieApi({op: 'delete_site', domain: h}); cookies = cookies.filter((c) => chost(c) !== h); renderCookies(); } catch (e) { alert(e.message); }
      }}, 'Delete site')));
    if (isOpen) {
      list.append(el('div', {class: 'tablewrap'}, el('table', {},
        el('thead', {}, el('tr', {}, el('th', {}, 'Name'), el('th', {}, 'Value'), el('th', {}, 'Expires'), el('th', {}, 'Details'), el('th', {}))),
        el('tbody', {}, ...items.sort((a, b) => a.name.localeCompare(b.name)).map(cookieRow)))));
    }
  }
}
const pad = (n) => String(n).padStart(2, '0');
const toLocalInput = (secs) => { const d = new Date(secs * 1000); return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`; };
function openCookieForm(c) {
  cookieEditing = c || null;
  $('cookieformtitle').textContent = c ? 'Edit cookie' : 'Add cookie';
  $('cf-name').value = c ? c.name : '';
  $('cf-domain').value = c ? c.domain : '';
  $('cf-path').value = c ? c.path : '/';
  $('cf-value').value = c ? c.value : '';
  $('cf-session').checked = c ? c.session : false;
  $('cf-expires').value = toLocalInput(c && c.expires > 0 ? c.expires : Date.now() / 1000 + 30 * 86400);
  $('cf-expires').disabled = $('cf-session').checked;
  $('cf-secure').checked = c ? c.secure : true;
  $('cf-httponly').checked = c ? c.http_only : false;
  $('cf-samesite').value = c ? c.same_site : 'lax';
  $('cf-msg').textContent = '';
  $('cookieform').hidden = false;
  $('cf-name').focus();
  $('cookieform').scrollIntoView({block: 'nearest', behavior: 'smooth'});
}
$('cf-session').onchange = () => { $('cf-expires').disabled = $('cf-session').checked; };
$('cf-cancel').onclick = () => { $('cookieform').hidden = true; };
$('cookieadd').onclick = () => openCookieForm(null);
$('cookieform').addEventListener('submit', async (e) => {
  e.preventDefault();
  const exp = $('cf-expires').value ? new Date($('cf-expires').value).getTime() / 1000 : 0;
  const cookie = {name: $('cf-name').value.trim(), value: $('cf-value').value, domain: $('cf-domain').value.trim().replace(/^https?:\/\//, '').replace(/\/.*$/, ''),
    path: $('cf-path').value.trim() || '/', session: $('cf-session').checked, expires: $('cf-session').checked ? 0 : exp,
    secure: $('cf-secure').checked, http_only: $('cf-httponly').checked, same_site: $('cf-samesite').value};
  try {
    await cookieApi({op: 'set', cookie, original: cookieEditing});
    $('cookieform').hidden = true;
    openSites.add(cookie.domain.replace(/^\./, ''));
    await loadCookies();
  } catch (err) { $('cf-msg').textContent = err.message; }
});
$('cookiesearch').addEventListener('input', renderCookies);
$('cookiereload').onclick = loadCookies;
$('cookieclear').onclick = async () => {
  if (!confirm('Delete all cookies? You will be signed out of every site.')) return;
  try { await cookieApi({op: 'delete_all'}); } catch (e) { alert(e.message); }
  loadCookies();
};
loadCookies();
// Highlight the section you're looking at in the side menu.
const links = [...document.querySelectorAll('.side a')];
const mark = () => {
  let cur = links[0];
  for (const a of links) { const s = document.querySelector(a.getAttribute('href')); if (s && s.getBoundingClientRect().top < 120) cur = a; }
  links.forEach(a => a.classList.toggle('on', a === cur));
};
addEventListener('scroll', mark, {passive: true}); mark();
</script>"##;

    let lvl = |id: &str, title: &str, sub: &str| radio("level", "adblock_level", id, &st.adblock_level, title, sub);
    let body = body
        .replace("INTERNAL", INTERNAL_BASE)
        .replace("ENGINES", &engines)
        .replace("HOME_NEWTAB", if custom_home { "" } else { "checked" })
        .replace("HOME_CUSTOM", if custom_home { "checked" } else { "" })
        .replace("HOMEURL", &esc(if custom_home { &st.home_page } else { "" }))
        .replace("THEMES", &themes_html)
        .replace("TOGGLE_GRAD", &toggle("gradients", st.gradients).replace(">", " data-reload=\"1\">"))
        .replace("ANIM_ON", &radio("motion", "animations", "1", if st.animations { "1" } else { "0" }, "On", "").replace("data-key", "data-bool=\"1\" data-reload=\"1\" data-key"))
        .replace("ANIM_OFF", &radio("motion", "animations", "0", if st.animations { "1" } else { "0" }, "Off", "Everything changes instantly.").replace("data-key", "data-bool=\"1\" data-reload=\"1\" data-key"))
        .replace("TOGGLE_BAR", &toggle("show_bookmarks_bar", st.show_bookmarks_bar))
        .replace("CURPROFILE", &esc(&{
            let p = crate::store::launch().profile.clone();
            if p.is_empty() { "Default".to_string() } else { p }
        }))
        .replace("PROFILELIST", &{
            let cur = crate::store::launch().profile.clone();
            crate::store::profiles()
                .iter()
                .map(|p| {
                    let label = if p.is_empty() { "Default".to_string() } else { p.clone() };
                    let js = esc(&serde_json::to_string(p).unwrap_or_default());
                    let this = *p == cur;
                    let (glyph, color) = crate::store::profile_logo(p);
                    let stored = crate::store::profile_logos().remove(p).unwrap_or_default();
                    format!(
                        "<div class=\"ext\"><button class=\"plogo\" style=\"background:{}\" data-emoji=\"{}\" data-color=\"{}\" title=\"Change logo\" onclick=\"pickLogo(this, {js})\">{}</button><span class=\"extinfo\"><b>{}</b><small>{}</small></span>{}{}</div>",
                        esc(&color),
                        esc(&stored.emoji),
                        esc(&stored.color),
                        esc(&glyph),
                        esc(&label),
                        if this { "This window" } else if p.is_empty() { "The main profile" } else { "Separate cookies, history and extensions" },
                        if this { String::new() } else { format!("<button onclick=\"openProfile({js})\">Open</button>") },
                        if this || p.is_empty() { String::new() } else { format!("<button class=\"x\" title=\"Delete profile\" onclick=\"deleteProfile({js})\">×</button>") },
                    )
                })
                .collect::<String>()
        })
        .replace("STARTPROFOPTS", &{
            let cur = crate::store::startup_profile();
            let mut o = format!("<option value=\"*ask\"{}>Ask which profile</option>", if cur.is_none() { " selected" } else { "" });
            for p in crate::store::profiles() {
                let label = if p.is_empty() { "Default".to_string() } else { p.clone() };
                let sel = cur.as_deref() == Some(p.as_str());
                o.push_str(&format!("<option value=\"{}\"{}>Always open {}</option>", esc(&p), if sel { " selected" } else { "" }, esc(&label)));
            }
            o
        })
        .replace("LOGOCOLORS_JSON", &serde_json::to_string(&crate::store::LOGO_COLORS).unwrap_or_default())
        .replace("BUILTIN_JSON", &builtin_json)
        .replace("CUSTOM_JSON", &custom_json)
        .replace("CURTHEME", &serde_json::to_string(&st.theme).unwrap_or_default())
        .replace("THREADS", &std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).to_string())
        .replace("COREOPTS", &{
            let n = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1);
            let mut o = format!("<option value=\"0\"{}>All ({n})</option>", if st.cpu_cores == 0 || st.cpu_cores >= n { " selected" } else { "" });
            for k in [1u32, 2, 4, 6, 8, 12, 16, 24, 32].into_iter().filter(|k| *k < n) {
                o.push_str(&format!("<option value=\"{k}\"{}>{k}</option>", if st.cpu_cores == k { " selected" } else { "" }));
            }
            o
        })
        .replace("PRIO_NORMAL", if st.cpu_priority == "normal" { "selected" } else { "" })
        .replace("PRIO_ABOVE", if st.cpu_priority == "above" { "selected" } else { "" })
        .replace("PRIO_HIGH", if st.cpu_priority == "high" { "selected" } else { "" })
        .replace("SLEEPOPTS", &[(0, "Off"), (5, "After 5 minutes"), (15, "After 15 minutes"), (30, "After 30 minutes"), (60, "After 1 hour"), (120, "After 2 hours")].iter().map(|(v, l)| format!("<option value=\"{v}\"{}>{l}</option>", if st.sleep_after_min == *v { " selected" } else { "" })).collect::<String>())
        .replace("PX_SYSTEM", &radio("proxy", "proxy_mode", "system", &st.proxy_mode, "Use Windows settings", ""))
        .replace("PX_NONE", &radio("proxy", "proxy_mode", "none", &st.proxy_mode, "No proxy", "Connect directly."))
        .replace("PX_MANUAL", &radio("proxy", "proxy_mode", "manual", &st.proxy_mode, "Manual", "Your own SOCKS5 or HTTP proxy."))
        .replace("PXT_SOCKS", if st.proxy_type != "http" { "selected" } else { "" })
        .replace("PXT_HTTP", if st.proxy_type == "http" { "selected" } else { "" })
        .replace("PXHOST", &esc(&st.proxy_host))
        .replace("PXPORT", &esc(&st.proxy_port))
        .replace("PXMANUAL_HIDDEN", if st.proxy_mode == "manual" { "false" } else { "true" })
        .replace("TOGGLE_SKIPEXT", &toggle("adblock_skip_extensions", st.adblock_skip_extensions))
        .replace("TOGGLE_STREAMAUDIO", &toggle("discord_audio", st.discord_audio))
        .replace("TOGGLE_GPU", &toggle("hardware_accel", st.hardware_accel).replace(">", " data-restartkey=\"gpu\">"))
        .replace("TOGGLE_HTTPS", &toggle("https_only", st.https_only))
        .replace("DNSOPTS", &privacy::DNS_PROVIDERS.iter().map(|(id, name, _, _)| format!("<option value=\"{id}\"{}>{name}</option>", if *id == st.dns_provider { " selected" } else { "" })).collect::<String>())
        .replace("NEXTID", &esc(&st.dns_nextdns_id))
        .replace("DNSCUSTOM", &esc(&st.dns_custom))
        .replace("DNSSTATE", &esc(&match privacy::doh_server(st) {
            Some(u) => format!("Lookups go encrypted to {u}"),
            None if st.dns_provider == "off" => "Off: lookups use Windows' DNS settings.".into(),
            None => "Enter an address above.".into(),
        }))
        .replace("DNSCHECKHIDE", if privacy::dns_check_page(st).is_empty() || privacy::doh_server(st).is_none() { "hidden" } else { "" })
        .replace("DNSCHECK", privacy::dns_check_page(st))
        .replace("PROFILES", &{
            let cur = privacy::current_profile(st);
            privacy::PROFILES.iter().map(|(id, name, desc)| format!("<button type=\"button\" class=\"profile{}\" data-id=\"{id}\" role=\"radio\" aria-checked=\"{}\"><b>{name}</b><small>{desc}</small></button>", if *id == cur { " on" } else { "" }, *id == cur)).collect::<String>()
        })
        .replace("FP_OFF", &radio("fp", "fingerprint", "off", &st.fingerprint, "Off", ""))
        .replace("FP_STD", &radio("fp", "fingerprint", "standard", &st.fingerprint, "Standard", "Reports common hardware and GPU values, hides exact OS and CPU details. Recommended."))
        .replace("FP_STRICT", &radio("fp", "fingerprint", "strict", &st.fingerprint, "Strict", "Also adds tiny per-site noise to canvas and audio readouts. Rarely, image editors or games may misbehave."))
        .replace("WA_ASK", &radio("wa", "webauthn", "ask", &st.webauthn, "Ask first", "Goat asks before a site can open the Windows passkey dialog, and stops sites from opening it by themselves. Recommended."))
        .replace("WA_ALLOW", &radio("wa", "webauthn", "allow", &st.webauthn, "Allow", "Sites use passkeys and security keys like in Chrome."))
        .replace("WA_BLOCK", &radio("wa", "webauthn", "block", &st.webauthn, "Block", "Never use passkeys or security keys. Sites fall back to passwords or codes."))
        .replace("TOGGLE_STRIP", &toggle("strip_tracking", st.strip_tracking))
        .replace("TOGGLE_REF", &toggle("trim_referrers", st.trim_referrers))
        .replace("TOGGLE_PREFETCH", &toggle("block_prefetch", st.block_prefetch).replace(">", " data-restartkey=\"prefetch\">"))
        .replace("TOGGLE_FONTS", &toggle("block_third_party_fonts", st.block_third_party_fonts))
        .replace("TOGGLE_3PC", &toggle("block_third_party_cookies", st.block_third_party_cookies).replace(">", " data-restartkey=\"cookies\">"))
        .replace("TOGGLE_CLEAR", &toggle("clear_cookies_on_exit", st.clear_cookies_on_exit))
        .replace("TOGGLE_PASTE", &toggle("paste_uploads", st.paste_uploads))
        .replace("TOGGLE_AUTOUPDATE", &toggle("auto_update", st.auto_update))
        .replace("HIST_ON", if st.save_history { "selected" } else { "" })
        .replace("HIST_OFF", if st.save_history { "" } else { "selected" })
        .replace("ABSTATUS", &esc(&abstatus))
        .replace("TOGGLE_AB", &toggle("adblock", st.adblock))
        .replace("LVL_ADS", &lvl("ads", "Ads only", "Lightest. Ads blocked, trackers allowed."))
        .replace("LVL_STD", &lvl("standard", "Standard", "Ads and trackers. Recommended."))
        .replace("LVL_STRICT", &lvl("strict", "Strict", "Also cookie banners, popups and other annoyances. May break some sites."))
        .replace("CHIPS", &chips)
        .replace("RULES", &esc(&st.adblock_custom))
        .replace("EXTS", &exts)
        .replace("TORSTATE", &torstate)
        .replace("VERSION", env!("CARGO_PKG_VERSION"));
    page_doc(look, "Settings", "settings", SETTINGS_CSS, &body)
}


#[cfg(test)]
mod dump {
    #[test]
    #[ignore]
    fn dump_settings() {
        let sh = crate::shared::test_shared();
        let req = wry::http::Request::builder().uri(format!("{}/settings", super::INTERNAL_BASE)).body(Vec::new()).unwrap();
        let r = super::handle(&sh, req);
        std::fs::write("/tmp/settings.html", r.body()).unwrap();
    }
}

#[cfg(test)]
mod api_theme {
    #[test]
    fn save_custom_theme() {
        let sh = crate::shared::test_shared();
        let body = br##"{"custom_themes":[{"id":"custom-abc","name":"T","bg":"#101214","surface":"#181b1f","text":"#e7e9ec","accent":"#ff9e6b","accent2":"#ff5e8a"}],"theme":"custom-abc"}"##.to_vec();
        let req = wry::http::Request::builder().method("POST").uri(format!("{}/api/settings", super::INTERNAL_BASE)).header("X-Goatr", "1").header("Content-Type", "application/json").body(body).unwrap();
        let r = super::handle(&sh, req);
        assert_eq!(r.status(), 200, "{}", String::from_utf8_lossy(r.body()));
        assert_eq!(sh.store.lock().unwrap().data.settings.custom_themes.len(), 1);
    }
}
