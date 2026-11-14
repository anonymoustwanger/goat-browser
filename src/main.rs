// Goatr — a web browser written in Rust.
//
// The browser UI (tab strip, toolbar, address bar, bookmarks bar, menus, ad-blocker panel…) is drawn
// natively in Rust with egui. Web pages are shown by Microsoft Edge WebView2 (via wry), which runs
// their JavaScript. Pop-up panels are separate native windows so they can float over web pages.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(deprecated)]

mod adblock;
mod anim;
mod chrome;
mod crashlog;
mod extinstall;
mod icons;
mod net;
mod pages;
mod paint;
mod panels;
mod exthost;
mod privacy;
mod shared;
mod store;
mod themes;
mod tor;
#[cfg(windows)]
mod winhooks;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use egui::{Pos2, Rect, Vec2};
use serde::Deserialize;
use wry::{
    dpi::{LogicalPosition, LogicalSize},
    http::Request,
    NewWindowResponse, PageLoadEvent, WebContext, WebView, WebViewBuilder,
};

use adblock::{AdBlock, Level};
use pages::{display_url, resolve_input};
use shared::{Notice, Shared};
use store::Store;

const INJECT_JS: &str = include_str!("inject.js");

/// Where the built-in goatr:// pages live inside the webview.
#[cfg(windows)]
pub const INTERNAL_BASE: &str = "http://goatr.localhost";
#[cfg(not(windows))]
pub const INTERNAL_BASE: &str = "goatr://localhost";

/// Events coming from webviews and background jobs.
#[derive(Debug)]
pub enum Ev {
    PageFailed(u32, &'static str),
    Redirect(u32, String),
    Eval(u32, String),
    ExtIds(Vec<(String, String)>),
    /// A page wants to open an app: (link, page it came from).
    AppLink(String, String),
    /// Result of loading an extension that wasn't running: (folder, Ok(name, id) / Err).
    ExtLoaded(String, Result<(String, String), String>),
    Content(u32, ContentMsg),
    NewTab(String, TabKind),
    Title(u32, String),
    Load(u32, bool, String),
    DownloadStarted(String, String),
    DownloadDone(String, Option<String>, bool),
    Blocked(u32),
    Notice(Notice),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum ContentMsg {
    Key { action: String },
    JsError { msg: String },
    /// A web store's install button was clicked on this page.
    InstallExt { url: String },
    /// An extension page asked for a tab.
    OpenTab {
        url: String,
        #[serde(default = "yes")]
        active: bool,
    },
    /// Mouse pressed inside a page (closes pop-ups that egui can't see the click for).
    Clicked,
    /// An extension pop-up page reports its size.
    Size { w: f32, h: f32 },
    Favicon { href: String },
    FindResult { count: u32 },
    /// Classes/ids seen on the page, for generic element-hiding rules.
    Cosmetic {
        url: String,
        #[serde(default)]
        classes: Vec<String>,
        #[serde(default)]
        ids: Vec<String>,
        #[serde(default)]
        first: bool,
    },
}

/// Everything the UI can ask for. Drawing code pushes these; `App::run` carries them out.
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    /// Open a window for another profile ("" = default).
    OpenProfile(String),
    /// Answer to "Add extension?": yes or no.
    ExtConfirm(bool),
    /// Answer to "Open this app?": (open, always allow for this site).
    AppLinkAnswer(bool, bool),
    /// A tab an extension asked for: (address, switch to it).
    ExtTab(String, bool),
    InstallExtensionFrom(String),
    /// Open an extension's toolbar pop-up (folder, anchor rect in window points).
    ExtOpen(String, egui::Rect),
    ExtOptions(String),
    ExtPin(String),
    CloseExtPopup,
    Navigate(String),
    Back,
    Forward,
    Reload,
    Stop,
    Home,
    NewTab,
    NewPrivateTab,
    NewTorTab,
    CloseTab(u32),
    SelectTab(u32),
    SelectIndex(usize),
    NextTab,
    PrevTab,
    ReopenTab,
    ToggleBookmark,
    OpenUrl(String, bool),
    OpenPage(&'static str),
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Fullscreen,
    Devtools,
    Print,
    FindOpen(bool),
    Find(bool),
    FocusAddress,
    FocusContent,
    ToggleBar,
    ToggleSiteAdblock,
    SetAdblock(bool),
    SetLevel(Level),
    InstallExtension,
    Restart,
    Popup(Popup, Pos2),
    ClosePopup,
    TabReload(u32),
    TabDup(u32),
    TabCloseOthers(u32),
    TabCloseRight(u32),
    BmDelete(u64),
    SetTheme(String),
    WinDrag,
    WinMinimize,
    WinToggleMax,
    WinClose,
    WinResize(egui::ResizeDirection),
    Toast(String),
}

/// Normal tabs use the saved profile. Private tabs share an in-memory session that's wiped when
/// Goatr closes; Tor tabs do too, and route everything through Tor. Neither keeps history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabKind {
    Normal,
    Private,
    Tor,
}

impl TabKind {
    pub fn is_private_like(self) -> bool {
        self != TabKind::Normal
    }
    fn slot(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Popup {
    Menu,
    Shield,
    Extensions,
    TabMenu(u32),
    BmMenu(u64),
    Suggest,
    /// "Add extension?" with its permissions.
    ExtConfirm,
    /// "Open Roblox?" for app links.
    AppLink,
}

pub struct Tab {
    pub id: u32,
    pub webview: WebView,
    pub title: String,
    pub url: String,
    pub loading: bool,
    pub zoom: f64,
    pub kind: TabKind,
    pub blocked: Arc<AtomicU32>,
    /// The page the tab is on (read by the ad blocker on every request).
    pub page_url: Arc<Mutex<String>>,
    /// Cosmetic-filter exceptions for the current page (from the filter lists).
    /// Element-hiding state for the current page: (exceptions, generic rules allowed).
    pub cos: Arc<Mutex<(std::collections::HashSet<String>, bool)>>,
    /// egui time when the tab was opened (for its slide-in animation).
    pub born: f64,
    /// Memory saver: when this tab went to the background, and whether it's been put to rest.
    pub hidden_since: Option<f64>,
    pub low_mem: bool,
}

/// Browser profiles: the normal one (saved) and an in-memory one for Tor tabs.
struct Contexts {
    normal: WebContext,
    private: Option<WebContext>,
    tor: Option<WebContext>,
    /// Linux registers custom protocols once per context (Windows: once per webview).
    proto: [bool; 3],
    /// Browser switches each context started with (WebView2 needs the same ones every time).
    args: [Option<String>; 3],
}

/// A tab that was just closed, drawn fading out for a moment.
pub struct Ghost {
    pub rect: Rect,
    pub title: String,
    pub url: String,
    pub kind: TabKind,
    pub born: f64,
}

pub struct Toast {
    pub text: String,
    pub action: Option<(String, Cmd)>,
    pub until: f64,
    pub born: f64,
}

pub struct App {
    pub sh: Arc<Shared>,
    tx: Sender<Ev>,
    rx: Receiver<Ev>,
    last_housekeeping: f64,
    /// Extension ids WebView2 assigned (by extension name).
    ext_ids: std::collections::HashMap<String, String>,
    last_ext_query: f64,
    ext_tex: std::collections::HashMap<String, Option<egui::TextureHandle>>,
    /// The open extension pop-up: its webview, folder, and anchor.
    pub ext_popup: Option<(wry::WebView, String, egui::Rect)>,
    ext_closed: Option<(String, f64)>,
    /// Extension pop-up to open once that extension has loaded.
    ext_retry: Option<(String, egui::Rect)>,
    ext_folder_ids: std::collections::HashMap<String, String>,
    ext_ids_known: bool,
    /// Downloaded extension waiting for "Add extension?".
    pub pending_ext: Option<extinstall::Pending>,
    /// A page asked to open an app; waiting for "Open …?".
    pub pending_app: Option<PendingApp>,
    last_cpu: f64,
    cpu_dirty: bool,
    /// Element-hiding work runs on its own thread so pages never wait on the window.
    cos_tx: Sender<Box<dyn FnOnce() + Send>>,
    pub ctx: egui::Context,
    ctxs: Contexts,
    pub tabs: Vec<Tab>,
    pub active: usize,
    next_id: u32,
    closed: Vec<(String, TabKind)>,
    pub ghosts: Vec<Ghost>,
    pub tab_rects: std::collections::HashMap<u32, Rect>,
    /// A pop-up that is animating closed: (popup, anchor, start time).
    pub popup_closing: Option<(Popup, Pos2, f64)>,
    /// Which pop-up last closed by itself (focus moved away) and when.
    pub popup_closed_at: Option<(Popup, f64)>,
    /// Which pop-up the window is ready (hidden setup done) for.
    pub popup_ready_for: Option<Popup>,
    pub popup_ready: bool,
    start_url: Option<String>,
    /// Normal, private or Tor window (each is its own Goatr process).
    pub window_kind: TabKind,
    // address bar
    pub addr: String,
    pub addr_editing: bool,
    pub addr_focus_req: bool,
    pub addr_rect: Rect,
    pub sugg: Vec<(String, String)>,
    pub sugg_sel: Option<usize>,
    pub sugg_for: String,
    // find bar
    pub find_open: bool,
    pub find_q: String,
    pub find_count: Option<u32>,
    pub find_focus_req: bool,
    // popups
    pub popup: Option<(Popup, Pos2)>,
    pub popup_had_focus: bool,
    pub popup_born: f64,
    pub toast: Option<Toast>,
    pub fullscreen: bool,
    pub content_rect: Rect,
    last_bounds: Option<[i32; 4]>,
    save_due: Option<f64>,
    pub fonts_ok: bool,
}

pub fn now(ctx: &egui::Context) -> f64 {
    ctx.input(|i| i.time)
}

fn main() -> eframe::Result<()> {
    crashlog::install();
    crashlog::note("start");
    // Linux only (for development): WebKitGTK needs GTK running.
    #[cfg(target_os = "linux")]
    let _ = gtk::init();
    let launch = store::launch().clone();
    let window_kind = match launch.window.as_str() {
        "private" => TabKind::Private,
        "tor" => TabKind::Tor,
        _ => TabKind::Normal,
    };
    // Private and Tor windows don't save anything back.
    store::READ_ONLY.store(window_kind != TabKind::Normal, Ordering::Relaxed);
    let store = Store::load();
    let settings_now = store.data.settings.clone();
    let st = store.data.settings.clone();
    let adblock = AdBlock::new(st.adblock, Level::parse(&st.adblock_level), &st.adblock_allow, &st.adblock_custom);

    let viewport = egui::ViewportBuilder::default()
        .with_title(match (window_kind, launch.profile.as_str()) {
            (TabKind::Private, _) => "Goatr — Private window".to_string(),
            (TabKind::Tor, _) => "Goatr — Tor window".to_string(),
            (_, "") => "Goatr".to_string(),
            (_, p) => format!("Goatr — {p}"),
        })
        .with_app_id("goatr")
        .with_decorations(false)
        .with_inner_size([1300.0, 860.0])
        .with_min_inner_size([520.0, 320.0])
        .with_icon(app_icon());
    let options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Glow, ..Default::default() };

    let result = eframe::run_native(
        "Goatr",
        options,
        Box::new(move |cc| {
            let (tx, rx) = channel::<Ev>();
            let ctx = cc.egui_ctx.clone();
            let fonts_ok = chrome::install_fonts(&ctx);
            let sh = {
                let tx = Mutex::new(tx.clone());
                let c = ctx.clone();
                Arc::new(Shared {
                    store: Mutex::new(store),
                    system_dark: AtomicBool::new(matches!(cc.egui_ctx.system_theme(), Some(egui::Theme::Dark) | None)),
                    adblock,
                    tor: tor::Tor::new(),
                    notify: Box::new(move |n| {
                        let _ = tx.lock().unwrap().send(Ev::Notice(n));
                        c.request_repaint();
                    }),
                    allow_http: Default::default(),
                    net_privacy: std::sync::RwLock::new(shared::NetPrivacy::from(&settings_now)),
                    ext_sites: std::sync::RwLock::new(extinstall::extension_sites()),
                })
            };
            {
                let c = ctx.clone();
                crashlog::set_probe(move || c.request_repaint());
            }
            let _ = std::fs::create_dir_all(extinstall::ext_dir());
            // Extension tabs open in Goatr (must run before extensions load).
            if window_kind == TabKind::Normal {
                exthost::start(sh.clone());
            }
            // Refresh ad-block filter lists in the background when they're a few days old.
            {
                let me = sh.clone();
                // Only the normal window updates the shared lists (private/Tor windows just read them).
                if window_kind == TabKind::Normal {
                    sh.adblock.refresh(false, move |n| (me.notify)(Notice::AdblockUpdated(n)));
                } else {
                    let _ = me; // AdBlock::new already builds from the saved lists
                }
            }
            let start = launch.url.clone();
            let mut app = App {
                sh,
                tx,
                rx,
                last_housekeeping: 0.0,
                ext_ids: Default::default(),
                last_ext_query: 0.0,
                ext_tex: Default::default(),
                ext_popup: None,
                ext_closed: None,
                ext_retry: None,
                ext_folder_ids: Default::default(),
                ext_ids_known: false,
                pending_ext: None,
                pending_app: None,
                last_cpu: 0.0,
                cpu_dirty: false,
                cos_tx: {
                    let (jt, jr) = channel::<Box<dyn FnOnce() + Send>>();
                    let _ = std::thread::Builder::new().name("cosmetic".into()).spawn(move || {
                        while let Ok(job) = jr.recv() {
                            job();
                        }
                    });
                    jt
                },
                ctx,
                ctxs: Contexts { normal: WebContext::new(Some(store::profile_dir().join("profile"))), private: None, tor: None, proto: [false; 3], args: [None, None, None] },
                tabs: Vec::new(),
                active: 0,
                next_id: 1,
                closed: Vec::new(),
                ghosts: Vec::new(),
                tab_rects: Default::default(),
                popup_closing: None,
                popup_closed_at: None,
                popup_ready_for: None,
                popup_ready: false,
                start_url: None,
                window_kind: TabKind::Normal,
                addr: String::new(),
                addr_editing: false,
                addr_focus_req: false,
                addr_rect: Rect::NOTHING,
                sugg: Vec::new(),
                sugg_sel: None,
                sugg_for: String::new(),
                find_open: false,
                find_q: String::new(),
                find_count: None,
                find_focus_req: false,
                popup: None,
                popup_had_focus: false,
                popup_born: 0.0,
                toast: None,
                fullscreen: false,
                content_rect: Rect::from_min_size(Pos2::new(0.0, 114.0), Vec2::new(1300.0, 746.0)),
                last_bounds: None,
                save_due: None,
                fonts_ok,
            };
            app.window_kind = window_kind;
            app.start_url = Some(match (start, window_kind) {
                (Some(a), _) => resolve_input(&app.settings(), &a),
                (None, TabKind::Private) => format!("{INTERNAL_BASE}/newtab?private=1"),
                (None, TabKind::Tor) => format!("{INTERNAL_BASE}/tor"),
                (None, TabKind::Normal) => app.home_url(),
            });
            // First tab: the window handle is available now.
            let url = app.start_url.take().unwrap();
            if let Err(e) = app.open_tab(cc, &url, window_kind) {
                eprintln!("couldn't create the first tab: {e}");
            }
            Ok(Box::new(app))
        }),
    );
    if let Err(e) = &result {
        crashlog::write("startup error", &e.to_string());
    }
    result
}

/// The Goat browser logo.
fn app_icon() -> egui::IconData {
    let dec = png::Decoder::new(std::io::Cursor::new(&include_bytes!("../assets/icon.png")[..]));
    let mut r = dec.read_info().expect("icon");
    let mut buf = vec![0; r.output_buffer_size().unwrap_or(256 * 256 * 4)];
    let info = r.next_frame(&mut buf).expect("icon");
    buf.truncate(info.buffer_size());
    egui::IconData { rgba: buf, width: info.width, height: info.height }
}

/// Settings shared by every webview. On Windows, webviews sharing a profile must register the
/// same custom protocols, so the goatr:// handler is on all of them.
fn common<'a>(b: WebViewBuilder<'a>, sh: &Arc<Shared>, kind: TabKind, register_proto: bool, args: &str) -> WebViewBuilder<'a> {
    let s = sh.clone();
    let tor = kind == TabKind::Tor;
    #[cfg(windows)]
    let b = {
        use wry::WebViewBuilderExtWindows;
        let b = b.with_additional_browser_args(args);
        if tor || kind == TabKind::Private {
            // Private and Tor profiles: no extensions (like Goat).
            b
        } else {
            // Extensions are loaded once, with the first window's webview. Passing the folder again
            // makes WebView2 reload every extension, which kills their pop-ups and the scripts
            // they run in other tabs (RoPro's missing text, "Extension context invalidated").
            static LOADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            let b = b.with_browser_extensions_enabled(true);
            if LOADED.swap(true, Ordering::SeqCst) { b } else { b.with_extensions_path(extinstall::ext_dir()) }
        }
    };
    #[cfg(not(windows))]
    let _ = args;
    #[cfg(not(windows))]
    let b = if tor {
        b.with_proxy_config(wry::ProxyConfig::Socks5(wry::ProxyEndpoint {
            host: "127.0.0.1".into(),
            port: tor::SOCKS_PORT.to_string(),
        }))
    } else {
        b
    };
    let b = if kind.is_private_like() { b.with_incognito(true) } else { b };
    let b = b.with_devtools(true).with_hotkeys_zoom(true).with_clipboard(true);
    if register_proto {
        b.with_custom_protocol("goatr".into(), move |_id, req| { let _g = crashlog::Scope::new("internal page request"); pages::handle(&s, req) })
    } else {
        b
    }
}

pub const EXT_POPUP_ID: u32 = u32::MAX;

#[derive(Clone, Debug)]
pub struct PendingApp {
    pub url: String,
    pub scheme: String,
    pub site: String,
    pub app: String,
    pub always: bool,
}

/// Where an extension pop-up goes: under its button, right-aligned, inside the page area.
pub fn ext_popup_rect(anchor: egui::Rect, w: f32, h: f32, content: egui::Rect) -> egui::Rect {
    let w = w.clamp(120.0, 800.0).min(content.width() - 16.0);
    let h = h.clamp(40.0, 600.0).min(content.height() - 16.0);
    let x = (anchor.right() - w).clamp(content.left() + 8.0, content.right() - w - 8.0);
    egui::Rect::from_min_size(egui::pos2(x, content.top() + 4.0), egui::vec2(w, h))
}

/// In Goatr an extension's pop-up is its own little browser, so "the active tab" would be the
/// pop-up itself. This makes tabs.query({active: true, currentWindow: true}) answer with the page
/// you're looking at, and window.close() close the pop-up.
fn ext_popup_shim(active_url: &str) -> String {
    let url = serde_json::to_string(active_url).unwrap_or_else(|_| "\"\"".into());
    r#"(() => {
  if (location.protocol !== 'chrome-extension:') return;
  const pageUrl = URL_JSON;
  window.close = () => {
    try {
      window.ipc.postMessage(JSON.stringify({ cmd: 'js_error', msg: 'extension pop-up closed itself: ' + String(new Error().stack).slice(0, 600) }));
      window.ipc.postMessage(JSON.stringify({ cmd: 'key', action: 'escape' }));
    } catch (_) {}
  };
  const install = () => {
    const c = window.chrome;
    if (!c || !c.tabs || !c.tabs.query) return false;
    if (c.tabs.__goatrQuery) return true;
    const orig = c.tabs.query.bind(c.tabs);
    const pageTab = () => ({ id: -1, index: 0, windowId: -1, active: true, url: pageUrl, status: 'complete', incognito: false, highlighted: true, pinned: false, selected: true, discarded: false, groupId: -1 });
    const pick = (all) => {
      const mine = location.href;
      const hits = all.filter(t => t.url === pageUrl || t.pendingUrl === pageUrl);
      if (hits.length) return [hits[0]];
      const others = all.filter(t => t.url !== mine && !String(t.url || '').startsWith('chrome-extension:'));
      return others.length ? others.slice(0, 1) : [pageTab()];
    };
    c.tabs.query = function (info, cb) {
      const wantsActive = info && info.active && !info.url && info.windowId == null;
      if (!wantsActive || !pageUrl) return orig(info, cb);
      const p = orig({}).then(pick, () => [pageTab()]);
      if (typeof cb === 'function') { p.then(cb); return undefined; }
      return p;
    };
    try { Object.defineProperty(c.tabs, '__goatrQuery', { value: true }); } catch (_) {}
    return true;
  };
  // Chrome's extension APIs can appear after this script runs: keep trying.
  if (!install()) {
    const iv = setInterval(() => { if (install()) clearInterval(iv); }, 25);
    setTimeout(() => clearInterval(iv), 10000);
    document.addEventListener('readystatechange', install);
  }
})();"#
        .replace("URL_JSON", &url)
}

fn yes() -> bool {
    true
}

fn load_png(path: &std::path::Path) -> Option<egui::ColorImage> {
    let file = std::fs::File::open(path).ok()?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let px = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        _ => return None,
    };
    (rgba.len() == w * h * 4).then(|| egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba))
}

fn webview_handles(url: &str) -> bool {
    let l = url.to_lowercase();
    ["http:", "https:", "about:", "data:", "blob:", "file:", "javascript:", "goatr:", "view-source:", "edge:", "chrome-extension:"]
        .iter()
        .any(|p| l.starts_with(p))
}

/// A link meant for an app on the computer (roblox-player:, steam:, mailto:, …).
fn is_app_link(url: &str) -> bool {
    let l = url.to_lowercase();
    let Some((scheme, _)) = l.split_once(':') else { return false };
    !scheme.is_empty()
        && scheme.len() < 40
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !["http", "https", "about", "data", "blob", "file", "javascript", "goatr", "view-source", "edge", "chrome", "chrome-extension", "chrome-search", "ws", "wss", "ms-settings", "ms-msdt", "search-ms", "shell"].contains(&scheme)
}

fn app_link_scheme(url: &str) -> String {
    url.split(':').next().unwrap_or("").to_lowercase()
}

fn wry_rect(r: Rect) -> wry::Rect {
    wry::Rect {
        position: LogicalPosition::new(r.min.x as f64, r.min.y as f64).into(),
        size: LogicalSize::new(r.width().max(0.0) as f64, r.height().max(0.0) as f64).into(),
    }
}

impl App {
    pub fn settings(&self) -> store::Settings {
        self.sh.store.lock().unwrap().data.settings.clone()
    }

    pub fn palette(&self) -> themes::Palette {
        let target = {
            let s = self.sh.store.lock().unwrap();
            themes::get(&s.data.settings.theme, self.sh.system_dark.load(Ordering::Relaxed)).palette(s.data.settings.gradients)
        };
        // Cross-fade when the theme changes.
        let ctx = &self.ctx;
        let key = egui::Id::new("palette-fade");
        let t_now = now(ctx);
        let (from, to, start): (themes::Palette, themes::Palette, f64) = ctx.data(|d| d.get_temp(key)).unwrap_or((target, target, t_now - 10.0));
        let k = |start: f64| anim::out_cubic(((t_now - start) / 0.4) as f32);
        if to != target {
            let cur = from.lerp(&to, k(start));
            ctx.data_mut(|d| d.insert_temp(key, (cur, target, t_now)));
            return cur;
        }
        let e = if anim::enabled(ctx) { k(start) } else { 1.0 };
        if e < 1.0 {
            ctx.request_repaint();
        }
        from.lerp(&to, e)
    }

    pub fn animations(&self) -> bool {
        self.sh.store.lock().unwrap().data.settings.animations
    }

    fn home_url(&self) -> String {
        let s = self.settings();
        if s.home_page.trim().is_empty() || s.home_page == "newtab" {
            format!("{INTERNAL_BASE}/newtab")
        } else {
            resolve_input(&s, &s.home_page)
        }
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    fn tab_index(&self, id: u32) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    fn open_tab(&mut self, window: &impl raw_window_handle::HasWindowHandle, url: &str, kind: TabKind) -> wry::Result<()> {
        let id = self.next_id;
        self.next_id += 1;
        let blocked = Arc::new(AtomicU32::new(0));
        let page_url = Arc::new(Mutex::new(url.to_string()));
        let (nav_url, nav_blocked) = (page_url.clone(), blocked.clone());
        let mk = |tx: &Sender<Ev>, ctx: &egui::Context| (tx.clone(), ctx.clone());
        let (t_ipc, c_ipc) = mk(&self.tx, &self.ctx);
        let (t_title, c_title) = mk(&self.tx, &self.ctx);
        let (t_load, c_load) = mk(&self.tx, &self.ctx);
        let (t_new, c_new) = mk(&self.tx, &self.ctx);
        let (t_dl, c_dl) = mk(&self.tx, &self.ctx);
        let sh_dl = self.sh.clone();
        let (t_dl2, c_dl2) = mk(&self.tx, &self.ctx);

        let slot = kind.slot();
        let register = cfg!(windows) || !self.ctxs.proto[slot];
        self.ctxs.proto[slot] = true;
        let settings = self.settings();
        if self.ctxs.args[slot].is_none() && kind != TabKind::Tor {
            // Before this profile's engine starts: encrypted DNS setting.
            let dir = store::profile_dir().join(if kind == TabKind::Private { "profile-private" } else { "profile" });
            privacy::write_dns_prefs(&dir, &settings);
        }
        let args = self.ctxs.args[slot]
            .get_or_insert_with(|| if kind == TabKind::Tor { tor::browser_args() } else { privacy::browser_args(&settings) })
            .clone();
        let web_ctx = match kind {
            TabKind::Tor => {
                self.sh.tor.start(self.sh.tor_notifier());
                self.ctxs.tor.get_or_insert_with(|| WebContext::new(Some(store::profile_dir().join("profile-tor"))))
            }
            TabKind::Private => self.ctxs.private.get_or_insert_with(|| WebContext::new(Some(store::profile_dir().join("profile-private")))),
            TabKind::Normal => &mut self.ctxs.normal,
        };

        let (t_nav, c_nav, sh_nav) = (self.tx.clone(), self.ctx.clone(), self.sh.clone());
        let (t_app, c_app) = (self.tx.clone(), self.ctx.clone());
        let webview = common(WebViewBuilder::new_with_web_context(web_ctx), &self.sh, kind, register, &args)
            .with_bounds(wry_rect(self.content_rect))
            .with_url(url)
            .with_initialization_script(INJECT_JS)
            .with_initialization_script(&privacy::page_script(&settings))
            .with_ipc_handler(move |req: Request<String>| {
                let _g = crashlog::Scope::new("page message");
                if let Ok(m) = serde_json::from_str::<ContentMsg>(req.body()) {
                    let _ = t_ipc.send(Ev::Content(id, m));
                    c_ipc.request_repaint();
                }
            })
            .with_document_title_changed_handler(move |t| {
                let _ = t_title.send(Ev::Title(id, t));
                c_title.request_repaint();
            })
            .with_on_page_load_handler(move |ev, url| {
                let _g = crashlog::Scope::new("page load event");
                let _ = t_load.send(Ev::Load(id, matches!(ev, PageLoadEvent::Finished), url));
                c_load.request_repaint();
            })
            .with_new_window_req_handler(move |url, features| {
                let _g = crashlog::Scope::new("new window request");
                // Real popups (sign-in windows etc.) get their own window so window.opener works.
                if features.size.is_some() {
                    NewWindowResponse::Allow
                } else {
                    let _ = t_new.send(Ev::NewTab(url, kind));
                    c_new.request_repaint();
                    NewWindowResponse::Deny
                }
            })
            .with_navigation_handler(move |url| {
                let _g = crashlog::Scope::new("navigation check");
                if webview_handles(&url) {
                    // HTTPS-only and tracking-link cleaning: load the fixed address instead.
                    if url.starts_with("http") && !url.starts_with(INTERNAL_BASE) {
                        let (https_only, strip) = sh_nav.store.try_lock().map(|s| (s.data.settings.https_only, s.data.settings.strip_tracking)).unwrap_or((true, true));
                        let mut fixed = None;
                        if https_only {
                            if let Ok(allow) = sh_nav.allow_http.try_lock() {
                                fixed = privacy::https_upgrade(&url, &allow);
                            }
                        }
                        if strip {
                            if let Some(c) = privacy::clean_tracking(fixed.as_deref().unwrap_or(&url)) {
                                fixed = Some(c);
                            }
                        }
                        if let Some(f) = fixed {
                            let _ = t_nav.send(Ev::Redirect(id, f));
                            c_nav.request_repaint();
                            return false;
                        }
                    }
                    *nav_url.lock().unwrap() = url.clone();
                    nav_blocked.store(0, Ordering::Relaxed);
                    return true;
                }
                if is_app_link(&url) {
                    let page = nav_url.lock().map(|u| u.clone()).unwrap_or_default();
                    let _ = t_app.send(Ev::AppLink(url, page));
                    c_app.request_repaint();
                }
                false
            })
            .with_download_started_handler(move |url, path| {
                if let Some(ext) = extinstall::id_from_crx_download(&url, &path.display().to_string()) {
                    let sh = sh_dl.clone();
                    std::thread::spawn(move || (sh.notify)(Notice::ExtensionInstalled(extinstall::install(extinstall::Source::Chrome, &ext))));
                    return false;
                }
                let _ = t_dl.send(Ev::DownloadStarted(url, path.display().to_string()));
                c_dl.request_repaint();
                true
            })
            .with_download_completed_handler(move |url, path, ok| {
                let _ = t_dl2.send(Ev::DownloadDone(url, path.map(|p| p.display().to_string()), ok));
                c_dl2.request_repaint();
            })
            .build_as_child(window)?;

        crashlog::debug(format!("open tab {id} {kind:?} adblock enabled={} ready={} rules={}", self.sh.adblock.enabled(), self.sh.adblock.ready(), self.sh.adblock.domain_count()));
        #[cfg(windows)]
        {
            let (tx, c) = (self.tx.clone(), self.ctx.clone());
            {
                let (tx, c) = (self.tx.clone(), self.ctx.clone());
                winhooks::watch_process(&webview, move |kind| {
                    let _ = tx.send(Ev::PageFailed(id, kind));
                    c.request_repaint();
                });
            }
            winhooks::set_tracking_prevention(&webview, settings.block_third_party_cookies);
            {
                let (tx, c) = (self.tx.clone(), self.ctx.clone());
                winhooks::watch_external_links(&webview, move |uri, origin| {
                    let _ = tx.send(Ev::AppLink(uri, origin));
                    c.request_repaint();
                });
            }
            winhooks::install_adblock(&webview, self.sh.clone(), page_url.clone(), blocked.clone(), move |n| {
                if n <= 5 || n % 5 == 0 {
                    let _ = tx.send(Ev::Blocked(id));
                    c.request_repaint();
                }
            });
        }

        for t in &self.tabs {
            let _ = t.webview.set_visible(false);
        }
        self.tabs.push(Tab {
            id,
            webview,
            title: match kind {
                TabKind::Tor => "Tor tab".into(),
                TabKind::Private => "New private tab".into(),
                TabKind::Normal => "New Tab".into(),
            },
            url: url.into(),
            loading: true,
            zoom: 1.0,
            kind,
            blocked,
            page_url,
            cos: Default::default(),
            hidden_since: None,
            low_mem: false,
            born: now(&self.ctx),
        });
        self.active = self.tabs.len() - 1;
        self.addr_editing = false;
        if url.starts_with(INTERNAL_BASE) && (url.contains("/newtab") || url.ends_with("/tor")) {
            self.addr_focus_req = true;
        }
        Ok(())
    }

    pub fn select(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        self.active = idx;
        for (i, t) in self.tabs.iter().enumerate() {
            let _ = t.webview.set_visible(i == idx);
        }
        let _ = self.tabs[idx].webview.focus();
        self.addr_editing = false;
        self.find_count = None;
    }

    fn close(&mut self, id: u32) {
        if let Some(i) = self.tab_index(id) {
            let t = self.tabs.remove(i);
            if let Some(r) = self.tab_rects.get(&id) {
                self.ghosts.push(Ghost { rect: *r, title: t.title.clone(), url: t.url.clone(), kind: t.kind, born: now(&self.ctx) });
            }
            if !t.url.contains("/newtab") {
                self.closed.push((t.url, t.kind));
            }
            if self.tabs.is_empty() {
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            let next = if i < self.active || self.active >= self.tabs.len() {
                self.active.saturating_sub(1).min(self.tabs.len() - 1)
            } else {
                self.active.min(self.tabs.len() - 1)
            };
            self.select(next);
        }
    }

    fn eval_active(&self, js: &str) {
        if let Some(t) = self.active_tab() {
            let _ = t.webview.evaluate_script(js);
        }
    }

    fn load_active(&mut self, url: &str) {
        if let Some(t) = self.tabs.get_mut(self.active) {
            let _ = t.webview.load_url(url);
            let _ = t.webview.focus();
            t.url = url.to_string();
        }
        self.addr_editing = false;
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        let n = now(&self.ctx);
        self.toast = Some(Toast { text: text.into(), action: None, until: n + 2.2, born: n });
    }

    fn toast_action(&mut self, text: impl Into<String>, label: &str, cmd: Cmd) {
        let n = now(&self.ctx);
        self.toast = Some(Toast { text: text.into(), action: Some((label.into(), cmd)), until: n + 9.0, born: n });
    }

    fn set_zoom(&mut self, z: Option<f64>, delta: f64) {
        let steps = [0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0];
        if let Some(t) = self.tabs.get_mut(self.active) {
            t.zoom = match z {
                Some(z) => z,
                None if delta > 0.0 => *steps.iter().find(|s| **s > t.zoom + 0.001).unwrap_or(&5.0),
                None => *steps.iter().rev().find(|s| **s < t.zoom - 0.001).unwrap_or(&0.25),
            };
            let _ = t.webview.zoom(t.zoom);
        }
    }

    fn mutate_settings(&mut self, f: impl FnOnce(&mut store::Settings)) {
        let mut s = self.sh.store.lock().unwrap();
        f(&mut s.data.settings);
        s.save();
    }

    fn reload_internal(&self, path: &str) {
        for t in &self.tabs {
            if t.url.starts_with(INTERNAL_BASE) && t.url.contains(path) {
                let _ = t.webview.reload();
            }
        }
    }

    /// Start a fresh copy of Goatr (after this one has closed) and exit.
    /// Start another Goatr window (a separate process): private/Tor and/or another profile.
    pub fn spawn_window(&mut self, window: Option<&str>, profile: Option<&str>, url: Option<&str>) {
        let Ok(exe) = std::env::current_exe() else { return };
        let mut cmd = std::process::Command::new(exe);
        if let Some(w) = window {
            cmd.args(["--window", w]);
        }
        let prof = profile.map(|p| p.to_string()).unwrap_or_else(|| store::launch().profile.clone());
        if !prof.is_empty() {
            cmd.args(["--profile", &prof]);
        }
        if let Some(u) = url {
            cmd.arg(u);
        }
        match cmd.spawn() {
            Ok(_) => self.toast(match window {
                Some("private") => "Opening a private window…".to_string(),
                Some("tor") => "Opening a Tor window…".to_string(),
                _ => format!("Opening {}…", if prof.is_empty() { "the default profile".to_string() } else { format!("profile “{prof}”") }),
            }),
            Err(e) => self.toast(format!("Couldn't open window: {e}")),
        }
    }

    fn restart(&mut self) {
        if let Ok(exe) = std::env::current_exe() {
            let l = store::launch();
            let mut extra = String::new();
            if !l.window.is_empty() {
                extra.push_str(&format!(" --window {}", l.window));
            }
            if !l.profile.is_empty() {
                extra.push_str(&format!(" --profile \"{}\"", l.profile));
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let _ = std::process::Command::new("cmd")
                    .raw_arg(format!("/C ping -n 3 127.0.0.1 >nul & start \"\" \"{}\"{extra}", exe.display()))
                    .creation_flags(0x0800_0000)
                    .spawn();
            }
            #[cfg(not(windows))]
            {
                let _ = extra;
                let _ = std::process::Command::new(exe).args(std::env::args().skip(1)).spawn();
            }
        }
        self.sh.tor.stop();
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    // ------------------------------------------------------------------ events from webviews

    fn handle_ev(&mut self, ev: Ev, frame: &eframe::Frame, cmds: &mut Vec<Cmd>) {
        match &ev {
            Ev::Blocked(_) | Ev::Eval(..) | Ev::ExtIds(_) | Ev::AppLink(..) | Ev::ExtLoaded(..) | Ev::Content(_, ContentMsg::Cosmetic { .. }) => {}
            e => crashlog::note(format!("{:.160}", format!("{e:?}"))),
        }
        match ev {
            Ev::NewTab(url, kind) => {
                // Extensions open "new tab" as chrome-search://local-ntp or chrome://newtab: use Goatr's.
                let l = url.to_lowercase();
                let url = if url.is_empty() || l.starts_with("chrome-search:") || l.starts_with("chrome://newtab") || l.starts_with("edge://newtab") || l == "about:newtab" {
                    format!("{INTERNAL_BASE}/newtab")
                } else {
                    url
                };
                if let Err(e) = self.open_tab(frame, &url, kind) { eprintln!("open_tab failed: {e}"); self.toast(format!("Couldn't open tab: {e}")); }
            }
            Ev::Blocked(id) => {
                let mut s = self.sh.store.lock().unwrap();
                s.data.settings.adblock_total += if self.tab_index(id).is_some() { 1 } else { 0 };
                drop(s);
                self.save_due.get_or_insert(now(&self.ctx) + 20.0);
            }
            Ev::ExtLoaded(folder, r) => match r {
                Ok((name, id)) => {
                    crashlog::debug(format!("loaded extension {folder}: {name} ({id})"));
                    self.ext_ids.insert(name, id.clone());
                    self.ext_folder_ids.insert(folder.clone(), id);
                    if let Some(anchor) = self.ext_retry.take().filter(|(f, _)| *f == folder).map(|(_, a)| a) {
                        cmds.push(Cmd::ExtOpen(folder, anchor));
                    }
                }
                Err(e) => {
                    crashlog::debug(format!("couldn't load extension {folder}: {e}"));
                    self.toast(format!("This extension couldn't start: {e}"));
                }
            },
            Ev::AppLink(url, page) => {
                let scheme = app_link_scheme(&url);
                let site = privacy::site_of(&store::host_of(&page));
                #[cfg(windows)]
                let app = winhooks::scheme_app_name(&scheme);
                #[cfg(not(windows))]
                let app = Some(scheme.clone());
                crashlog::debug(format!("app link {scheme}: from {site:.80}, app {app:?}"));
                match app {
                    None => self.toast(format!("No app on this PC opens “{scheme}:” links")),
                    Some(_) if !site.is_empty() && self.settings().app_links_allowed.contains(&format!("{site}|{scheme}")) => {
                        let _ = open::that_detached(&url);
                    }
                    Some(name) => {
                        self.pending_app = Some(PendingApp { url, scheme, site, app: name, always: false });
                        let at = chrome::to_screen(&self.ctx, egui::pos2(self.content_rect.center().x, self.content_rect.top() + 2.0));
                        self.popup = Some((Popup::AppLink, at));
                        self.popup_had_focus = false;
                        self.popup_born = now(&self.ctx);
                    }
                }
            }
            Ev::ExtIds(list) => {
                let names: Vec<&str> = list.iter().map(|(n, _)| n.as_str()).collect();
                if self.ext_ids.len() != list.len() {
                    crashlog::debug(format!("extensions loaded by WebView2: {names:?}"));
                }
                self.ext_ids = list.into_iter().collect();
                self.ext_ids_known = true;
                if let Some((folder, anchor)) = self.ext_retry.take() {
                    cmds.push(Cmd::ExtOpen(folder, anchor));
                }
            }
            Ev::Eval(id, js) => {
                if let Some(i) = self.tab_index(id) {
                    let _ = self.tabs[i].webview.evaluate_script(&js);
                }
            }
            Ev::Redirect(id, url) => {
                if let Some(i) = self.tab_index(id) {
                    let _ = self.tabs[i].webview.load_url(&url);
                }
            }
            Ev::PageFailed(id, kind) => {
                let url = self.tab_index(id).map(|i| self.tabs[i].url.clone()).unwrap_or_default();
                crashlog::write("page problem", &format!("{kind}: {url}\nad blocker: {}", if self.sh.adblock.enabled() { "on" } else { "off" }));
                if let Some(i) = self.tab_index(id) {
                    if kind != "browser process exited" {
                        let _ = self.tabs[i].webview.reload();
                    }
                }
                self.toast(match kind {
                    "unresponsive" => "This page stopped responding, so Goatr reloaded it".to_string(),
                    k => format!("Page problem ({k}), reloading"),
                });
            }
            Ev::Title(id, title) => {
                if let Some(i) = self.tab_index(id) {
                    let t = &mut self.tabs[i];
                    t.title = if title.trim().is_empty() { display_url(&t.url) } else { title };
                    let (u, ti, tor) = (t.url.clone(), t.title.clone(), t.kind.is_private_like());
                    if i == self.active {
                        self.ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{ti} - Goatr")));
                    }
                    if !tor && u.starts_with("http") && !u.starts_with(INTERNAL_BASE) {
                        self.sh.store.lock().unwrap().update_title(&u, &ti);
                    }
                }
            }
            Ev::Load(id, done, url) => {
                if let Some(i) = self.tab_index(id) {
                    let t = &mut self.tabs[i];
                    t.loading = !done;
                    *t.page_url.lock().unwrap() = url.clone();
                    t.url = url;
                    if i == self.active {
                        self.addr_editing = self.addr_editing && !done;
                    }
                    if done {
                        let web = t.url.starts_with("http") && !t.url.starts_with(INTERNAL_BASE);
                        if web && t.kind == TabKind::Normal {
                            let (u, ti) = (t.url.clone(), t.title.clone());
                            self.sh.store.lock().unwrap().record_visit(&u, &ti);
                        }
                    }
                }
            }
            Ev::Content(id, m) => {
                let is_active = self.active_tab().map(|t| t.id) == Some(id);
                match m {
                    ContentMsg::Favicon { .. } => {}
                    ContentMsg::Cosmetic { url, classes, ids, first } => self.cosmetic(id, &url, classes, ids, first),
                    ContentMsg::JsError { msg } => crashlog::debug(format!("page script error (tab {id}): {msg:.400}")),
                    ContentMsg::InstallExt { url } => cmds.push(Cmd::InstallExtensionFrom(url)),
                    ContentMsg::OpenTab { url, active } => {
                        if id == EXT_POPUP_ID && active {
                            cmds.push(Cmd::CloseExtPopup);
                        }
                        cmds.push(Cmd::ExtTab(url, active));
                    }
                    ContentMsg::Clicked if id != EXT_POPUP_ID => {
                        // A click in a page: close suggestions, menus and extension pop-ups.
                        if self.popup.is_some() {
                            cmds.push(Cmd::ClosePopup);
                        }
                        if self.ext_popup.is_some() {
                            cmds.push(Cmd::CloseExtPopup);
                        }
                        self.addr_editing = false;
                        self.ctx.memory_mut(|m| m.stop_text_input());
                    }
                    ContentMsg::Size { w, h } if id == EXT_POPUP_ID => {
                        if let Some((wv, _, anchor)) = &self.ext_popup {
                            let _ = wv.set_bounds(wry_rect(ext_popup_rect(*anchor, w, h, self.content_rect)));
                        }
                    }
                    ContentMsg::Key { action } if id == EXT_POPUP_ID => {
                        if action == "escape" {
                            cmds.push(Cmd::CloseExtPopup);
                        }
                    }
                    ContentMsg::FindResult { count } if is_active => self.find_count = Some(count),
                    ContentMsg::Key { action } if is_active => {
                        if let Some(c) = key_action(&action, id) {
                            cmds.push(c);
                        }
                    }
                    _ => {}
                }
            }
            Ev::DownloadStarted(url, path) => {
                self.sh.store.lock().unwrap().download_started(&url, &path);
                let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.toast(format!("Downloading {name}"));
            }
            Ev::DownloadDone(url, path, ok) => {
                self.sh.store.lock().unwrap().download_finished(&url, path.as_deref(), ok);
                if ok {
                    self.toast_action("Download complete", "Show", Cmd::OpenPage("downloads"));
                } else {
                    self.toast("Download failed");
                }
            }
            Ev::Notice(n) => match n {
                Notice::StoreChanged => {
                    let s = self.settings();
                    *self.sh.net_privacy.write().unwrap() = shared::NetPrivacy::from(&s);
                    self.cpu_dirty = true;
                    self.sh.adblock.set_enabled(s.adblock);
                    self.sh.adblock.set_level(Level::parse(&s.adblock_level));
                    self.sh.adblock.set_custom(&s.adblock_custom);
                    self.sh.adblock.set_allowlist(&s.adblock_allow);
                }
                Notice::ClearBrowsingData => {
                    if let Some(t) = self.tabs.iter().find(|t| t.kind == TabKind::Normal) {
                        let _ = t.webview.clear_all_browsing_data();
                    }
                }
                Notice::OpenTorTab => cmds.push(Cmd::NewTorTab),
                Notice::OpenProfile(name) => cmds.push(Cmd::OpenProfile(name)),
                Notice::OpenPrivateTab => cmds.push(Cmd::NewPrivateTab),
                Notice::TorChanged => {
                    let st = self.sh.tor.state();
                    if st.phase == "ready" {
                        self.toast("Connected to Tor");
                    } else if st.phase == "error" {
                        self.toast(format!("Tor: {}", st.message));
                    }
                }
                Notice::AdblockUpdated(_) => self.reload_internal("/settings"),
                Notice::ExtensionInstalled(r) => match r {
                    Ok(p) => {
                        if let Some(old) = self.pending_ext.take() {
                            extinstall::discard(&old);
                        }
                        self.pending_ext = Some(p);
                        // Centred under the toolbar, like Chrome's dialog.
                        let at = chrome::to_screen(&self.ctx, egui::pos2(self.content_rect.center().x, self.content_rect.top() + 2.0));
                        self.popup = Some((Popup::ExtConfirm, at));
                        self.popup_had_focus = false;
                        self.popup_born = now(&self.ctx);
                    }
                    Err(e) => self.toast(format!("Couldn't add extension: {e}")),
                },
                Notice::Restart => cmds.push(Cmd::Restart),
                Notice::OpenTab(url, active, popup) => {
                    if active {
                        self.ext_popup = None;
                    }
                    let _ = popup; // Goatr shows extension windows as tabs.
                    cmds.push(Cmd::ExtTab(url, active));
                }
            },
        }
    }

    // ------------------------------------------------------------------ commands

    pub fn run(&mut self, cmd: Cmd, frame: &eframe::Frame) {
        if std::env::var("GOATR_DEBUG").is_ok() {
            eprintln!("cmd: {cmd:?}");
        }
        crashlog::note(format!("{cmd:?}"));
        let kind_active = self.active_tab().map(|t| t.kind).unwrap_or(TabKind::Normal);
        match cmd {
            Cmd::Navigate(text) => {
                let target = resolve_input(&self.settings(), &text);
                // Typing an http:// address on purpose opts that site out of HTTPS-only mode.
                if text.trim().to_lowercase().starts_with("http://") {
                    self.sh.allow_http.lock().unwrap().insert(store::host_of(&target));
                }
                self.load_active(&target);
            }
            Cmd::Back => self.eval_active("history.back()"),
            Cmd::Forward => self.eval_active("history.forward()"),
            Cmd::Reload => {
                if let Some(t) = self.active_tab() {
                    let _ = t.webview.reload();
                }
            }
            Cmd::Stop => self.eval_active("window.stop()"),
            Cmd::Home => {
                let h = self.home_url();
                self.load_active(&h);
            }
            Cmd::NewTab => {
                let url = match self.window_kind {
                    TabKind::Private => format!("{INTERNAL_BASE}/newtab?private=1"),
                    TabKind::Tor => format!("{INTERNAL_BASE}/tor"),
                    TabKind::Normal => format!("{INTERNAL_BASE}/newtab"),
                };
                if let Err(e) = self.open_tab(frame, &url, self.window_kind) { self.toast(format!("Couldn't open tab: {e}")); }
            }
            // Private and Tor browsing get their own window (a new tab if you're already in one).
            Cmd::NewPrivateTab => {
                if self.window_kind == TabKind::Private {
                    if let Err(e) = self.open_tab(frame, &format!("{INTERNAL_BASE}/newtab?private=1"), TabKind::Private) { self.toast(format!("Couldn't open tab: {e}")); }
                } else {
                    self.spawn_window(Some("private"), None, None);
                }
            }
            Cmd::NewTorTab => {
                if self.window_kind == TabKind::Tor {
                    if let Err(e) = self.open_tab(frame, &format!("{INTERNAL_BASE}/tor"), TabKind::Tor) { self.toast(format!("Couldn't open tab: {e}")); }
                } else {
                    self.spawn_window(Some("tor"), None, None);
                }
            }
            Cmd::OpenProfile(name) => self.spawn_window(None, Some(&name), None),
            Cmd::CloseTab(id) => self.close(id),
            Cmd::SelectTab(id) => {
                if let Some(i) = self.tab_index(id) {
                    self.select(i);
                }
            }
            Cmd::SelectIndex(n) => {
                let i = if n >= 9 { self.tabs.len().saturating_sub(1) } else { n.saturating_sub(1) };
                self.select(i);
            }
            Cmd::NextTab => {
                if !self.tabs.is_empty() {
                    self.select((self.active + 1) % self.tabs.len());
                }
            }
            Cmd::PrevTab => {
                if !self.tabs.is_empty() {
                    self.select((self.active + self.tabs.len() - 1) % self.tabs.len());
                }
            }
            Cmd::ReopenTab => {
                if let Some((url, kind)) = self.closed.pop() {
                    if let Err(e) = self.open_tab(frame, &url, kind) { eprintln!("open_tab failed: {e}"); self.toast(format!("Couldn't open tab: {e}")); }
                }
            }
            Cmd::ToggleBookmark => {
                if let Some(t) = self.active_tab() {
                    let (title, url) = (t.title.clone(), t.url.clone());
                    if url.starts_with("http") && !url.starts_with(INTERNAL_BASE) {
                        let added = self.sh.store.lock().unwrap().toggle_bookmark(&title, &url);
                        self.toast(if added { "Bookmark added" } else { "Bookmark removed" });
                    }
                }
            }
            Cmd::AppLinkAnswer(open, always) => {
                self.popup = None;
                if let Some(p) = self.pending_app.take() {
                    if open {
                        if always && !p.site.is_empty() {
                            let mut s = self.sh.store.lock().unwrap();
                            let key = format!("{}|{}", p.site, p.scheme);
                            if !s.data.settings.app_links_allowed.contains(&key) {
                                s.data.settings.app_links_allowed.push(key);
                                s.save();
                            }
                        }
                        let _ = open::that_detached(&p.url);
                    }
                }
            }
            Cmd::ExtConfirm(yes) => {
                self.popup = None;
                if let Some(p) = self.pending_ext.take() {
                    if yes {
                        match extinstall::confirm(&p) {
                            Ok(name) => {
                                exthost::prepare_all();
                                *self.sh.ext_sites.write().unwrap() = extinstall::extension_sites();
                                self.toast_action(format!("Added “{name}” — restart to turn it on"), "Restart now", Cmd::Restart);
                                self.reload_internal("/extensions");
                            }
                            Err(e) => self.toast(format!("Couldn't add extension: {e}")),
                        }
                    } else {
                        extinstall::discard(&p);
                    }
                }
            }
            Cmd::ExtTab(url, active) => {
                let url = if url.is_empty() { format!("{INTERNAL_BASE}/newtab") } else { url };
                let prev = self.active;
                match self.open_tab(frame, &url, TabKind::Normal) {
                    // In the background if the extension said active: false (like Chrome).
                    Ok(()) if !active && prev < self.tabs.len() => self.select(prev),
                    Ok(()) => {}
                    Err(e) => self.toast(format!("Couldn't open tab: {e}")),
                }
            }
            Cmd::OpenUrl(url, new_tab) => {
                if new_tab {
                    if let Err(e) = self.open_tab(frame, &url, kind_active) { eprintln!("open_tab failed: {e}"); self.toast(format!("Couldn't open tab: {e}")); }
                } else {
                    self.load_active(&url);
                }
            }
            Cmd::OpenPage(page) => {
                let url = format!("{INTERNAL_BASE}/{page}");
                if let Some(i) = self.tabs.iter().position(|t| t.kind == self.window_kind && t.url.starts_with(&url)) {
                    let _ = self.tabs[i].webview.reload();
                    self.select(i);
                } else if self.active_tab().is_some_and(|t| t.kind == self.window_kind && t.url.ends_with("/newtab")) {
                    self.load_active(&url);
                } else {
                    if let Err(e) = self.open_tab(frame, &url, self.window_kind) { eprintln!("open_tab failed: {e}"); self.toast(format!("Couldn't open tab: {e}")); }
                }
            }
            Cmd::ZoomIn => self.set_zoom(None, 1.0),
            Cmd::ZoomOut => self.set_zoom(None, -1.0),
            Cmd::ZoomReset => self.set_zoom(Some(1.0), 0.0),
            Cmd::Fullscreen => {
                self.fullscreen = !self.fullscreen;
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
            }
            Cmd::Devtools => {
                if let Some(t) = self.active_tab() {
                    t.webview.open_devtools();
                }
            }
            Cmd::Print => {
                if let Some(t) = self.active_tab() {
                    let _ = t.webview.print();
                }
            }
            Cmd::FindOpen(open) => {
                self.find_open = open;
                if open {
                    self.find_focus_req = true;
                } else {
                    self.find_count = None;
                    self.eval_active("window.__goatr && window.__goatr.findClear()");
                    if let Some(t) = self.active_tab() {
                        let _ = t.webview.focus();
                    }
                }
            }
            Cmd::Find(back) => {
                let q = serde_json::to_string(&self.find_q).unwrap_or_default();
                self.eval_active(&format!("window.__goatr && window.__goatr.find({q}, {back})"));
            }
            Cmd::FocusAddress => {
                if let Some(t) = self.active_tab() {
                    let _ = t.webview.focus_parent();
                }
                self.addr_focus_req = true;
            }
            Cmd::FocusContent => {
                self.addr_editing = false;
                if let Some(t) = self.active_tab() {
                    let _ = t.webview.focus();
                }
            }
            Cmd::ToggleBar => self.mutate_settings(|s| s.show_bookmarks_bar = !s.show_bookmarks_bar),
            Cmd::ToggleSiteAdblock => {
                if let Some(t) = self.active_tab() {
                    let host = adblock::host_of(&t.page_url.lock().unwrap()).trim_start_matches("www.").to_string();
                    if !host.is_empty() {
                        let mut now_blocking = false;
                        self.mutate_settings(|s| {
                            if let Some(i) = s.adblock_allow.iter().position(|h| h == &host) {
                                s.adblock_allow.remove(i);
                                now_blocking = true;
                            } else {
                                s.adblock_allow.push(host.clone());
                            }
                        });
                        let allow = self.settings().adblock_allow;
                        self.sh.adblock.set_allowlist(&allow);
                        if let Some(t) = self.active_tab() {
                            let _ = t.webview.reload();
                        }
                        self.toast(if now_blocking { format!("Blocking on {host}") } else { format!("Ad blocker paused on {host}") });
                    }
                }
            }
            Cmd::SetAdblock(on) => {
                self.mutate_settings(|s| s.adblock = on);
                self.sh.adblock.set_enabled(on);
                self.reload_internal("/settings");
            }
            Cmd::SetLevel(l) => {
                self.mutate_settings(|s| s.adblock_level = l.id().into());
                self.sh.adblock.set_level(l);
                self.reload_internal("/settings");
            }
            Cmd::InstallExtension | Cmd::InstallExtensionFrom(_) => {
                // Store pages change address without reloading, so prefer the address the page sent.
                let url = match &cmd {
                    Cmd::InstallExtensionFrom(u) => u.clone(),
                    _ => self.active_tab().map(|t| t.page_url.lock().unwrap().clone()).unwrap_or_default(),
                };
                let found = extinstall::parse_store_url(&url).or_else(|| self.active_tab().and_then(|t| extinstall::parse_store_url(&t.url)));
                if found.is_none() {
                    self.toast("Open an extension's page in the store first");
                }
                if let Some((src, id)) = found {
                    self.toast("Adding extension…");
                    let sh = self.sh.clone();
                    std::thread::spawn(move || (sh.notify)(Notice::ExtensionInstalled(extinstall::install(src, &id))));
                }
            }
            Cmd::Restart => self.restart(),
            Cmd::Popup(p, at) => {
                // Opened from the menu (no button position): put it at the top right of the page.
                let at = if at == Pos2::ZERO { chrome::to_screen(&self.ctx, egui::pos2(self.content_rect.right() - 8.0, self.content_rect.top() + 2.0)) } else { at };
                // Its button was clicked while it was open: the press already closed it (focus moved
                // to the main window), so this click shouldn't open it again.
                if let Some(old) = self.pending_ext.take() {
                    extinstall::discard(&old); // another panel replaced "Add extension?"
                }
                let just_closed = self.popup.is_none() && self.popup_closed_at.is_some_and(|(cp, t0)| cp == p && now(&self.ctx) - t0 < 0.5);
                if just_closed {
                } else if self.popup.map(|x| x.0) == Some(p) {
                    if let Some((p, a)) = self.popup.take() {
                        self.popup_closing = Some((p, a, now(&self.ctx)));
                    }
                } else {
                    self.popup = Some((p, at));
                    self.popup_had_focus = false;
                    self.popup_born = now(&self.ctx);
                }
            }
            Cmd::ExtOpen(folder, anchor) => {
                // Clicking the button of the pop-up that's open (it closed on the press) just closes it.
                let just_closed = self.ext_closed.as_ref().is_some_and(|(f, t)| *f == folder && now(&self.ctx) - t < 0.5);
                let same = just_closed || self.ext_popup.as_ref().is_some_and(|p| p.1 == folder);
                self.ext_popup = None;
                if !same {
                    if self.ext_url(&folder, false).is_none() && self.ext_url(&folder, true).is_some() {
                        // No pop-up page: open its options instead.
                        self.run(Cmd::ExtOptions(folder), frame);
                    } else {
                        self.open_ext_popup(frame, &folder, anchor);
                    }
                }
            }
            Cmd::CloseExtPopup => self.ext_popup = None,
            Cmd::ExtOptions(folder) => {
                if let Some(url) = self.ext_url(&folder, true) {
                    self.ext_popup = None;
                    if let Err(e) = self.open_tab(frame, &url, TabKind::Normal) {
                        self.toast(format!("Couldn't open options: {e}"));
                    }
                } else {
                    self.toast("This extension isn't ready yet. Restart Goatr after installing.");
                }
            }
            Cmd::ExtPin(folder) => {
                let mut s = self.sh.store.lock().unwrap();
                let list = &mut s.data.settings.pinned_extensions;
                if let Some(i) = list.iter().position(|f| *f == folder) {
                    list.remove(i);
                } else {
                    list.push(folder);
                }
                s.save();
            }
            Cmd::ClosePopup => {
                if let Some((p, a)) = self.popup.take() {
                    self.popup_closing = Some((p, a, now(&self.ctx)));
                }
            }
            Cmd::TabReload(id) => {
                if let Some(i) = self.tab_index(id) {
                    let _ = self.tabs[i].webview.reload();
                }
            }
            Cmd::TabDup(id) => {
                if let Some(i) = self.tab_index(id) {
                    let (u, kind) = (self.tabs[i].url.clone(), self.tabs[i].kind);
                    if let Err(e) = self.open_tab(frame, &u, kind) { eprintln!("open_tab failed: {e}"); self.toast(format!("Couldn't open tab: {e}")); }
                }
            }
            Cmd::TabCloseOthers(id) => {
                let others: Vec<u32> = self.tabs.iter().map(|t| t.id).filter(|t| *t != id).collect();
                for o in others {
                    self.close(o);
                }
            }
            Cmd::TabCloseRight(id) => {
                if let Some(i) = self.tab_index(id) {
                    let right: Vec<u32> = self.tabs[i + 1..].iter().map(|t| t.id).collect();
                    for o in right {
                        self.close(o);
                    }
                }
            }
            Cmd::BmDelete(id) => self.sh.store.lock().unwrap().delete_bookmark(id),
            Cmd::SetTheme(t) => {
                self.mutate_settings(|s| s.theme = t);
                self.reload_internal("");
            }
            Cmd::WinDrag => self.ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag),
            Cmd::WinMinimize => self.ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            Cmd::WinToggleMax => {
                let m = self.ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!m));
            }
            Cmd::WinClose => {
                self.sh.tor.stop();
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Cmd::WinResize(d) => self.ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(d)),
            Cmd::Toast(t) => self.toast(t),
        }
    }

    /// Element hiding: site-specific rules on the first report from a page, then generic rules for
    /// the classes and ids the page uses (the page reports new ones as it changes).
    /// chrome-extension:// address of an extension's pop-up (or options) page.
    pub fn ext_url(&self, folder: &str, options: bool) -> Option<String> {
        let e = extinstall::list().into_iter().find(|e| e.folder == folder)?;
        // By folder (store id), by what WebView2 told us after loading it, or by name.
        let id = self.ext_folder_ids.get(folder).or_else(|| self.ext_ids.get(&e.name)).or_else(|| self.ext_ids.values().find(|v| v.as_str() == folder))?;
        let page = if options { e.options } else { e.popup };
        (!page.is_empty()).then(|| format!("chrome-extension://{id}/{page}"))
    }

    /// The extension's icon as a texture (loaded once).
    pub fn ext_icon(&mut self, ctx: &egui::Context, e: &extinstall::ExtInfo) -> Option<egui::TextureHandle> {
        if let Some(t) = self.ext_tex.get(&e.folder) {
            return t.clone();
        }
        let tex = e.icon.as_ref().and_then(|p| load_png(p)).map(|img| ctx.load_texture(format!("ext-{}", e.folder), img, egui::TextureOptions::LINEAR));
        self.ext_tex.insert(e.folder.clone(), tex.clone());
        tex
    }

    fn open_ext_popup(&mut self, window: &impl raw_window_handle::HasWindowHandle, folder: &str, anchor: egui::Rect) {
        let Some(url) = self.ext_url(folder, false) else {
            let info = extinstall::list().into_iter().find(|e| e.folder == folder);
            let loaded = info.as_ref().is_some_and(|e| self.ext_ids.contains_key(&e.name)) || self.ext_folder_ids.contains_key(folder);
            if info.as_ref().is_some_and(|e| e.popup.is_empty()) && loaded {
                self.toast("This extension has no toolbar pop-up.");
                return;
            }
            // Goatr hasn't asked WebView2 for its extension list yet: ask, then open.
            if !self.ext_ids_known {
                self.ext_retry = Some((folder.to_string(), anchor));
                self.last_ext_query = -100.0;
                self.last_housekeeping = -100.0;
                self.ctx.request_repaint();
                return;
            }
            // Really not running (e.g. added this session): load just this one, then open.
            #[cfg(windows)]
            if let Some(tab) = self.tabs.iter().find(|t| t.kind == TabKind::Normal).filter(|_| !loaded) {
                let (tx, c, f) = (self.tx.clone(), self.ctx.clone(), folder.to_string());
                self.ext_retry = Some((folder.to_string(), anchor));
                winhooks::add_extension(&tab.webview, &extinstall::ext_dir().join(folder), move |r| {
                    let _ = tx.send(Ev::ExtLoaded(f, r));
                    c.request_repaint();
                });
                self.toast("Starting the extension…");
                return;
            }
            self.toast("Extensions are still loading. Try again in a moment.");
            return;
        };
        let (tx, c) = (self.tx.clone(), self.ctx.clone());
        let active_url = self.active_tab().map(|t| t.page_url.lock().unwrap().clone()).unwrap_or_default();
        let slot = TabKind::Normal.slot();
        let settings = self.settings();
        let args = self.ctxs.args[slot].get_or_insert_with(|| privacy::browser_args(&settings)).clone();
        let b = common(WebViewBuilder::new_with_web_context(&mut self.ctxs.normal), &self.sh, TabKind::Normal, cfg!(windows), &args)
            .with_bounds(wry_rect(ext_popup_rect(anchor, 360.0, 480.0, self.content_rect)))
            .with_url(&url)
            .with_initialization_script(INJECT_JS)
            .with_initialization_script(&ext_popup_shim(&active_url))
            .with_new_window_req_handler({
                let (tx, c) = (self.tx.clone(), self.ctx.clone());
                move |url, features| {
                    crashlog::debug(format!("extension pop-up opened a window: {url:.200}"));
                    if features.size.is_some() {
                        // window.open with a size: a real pop-up window, like Chrome.
                        return NewWindowResponse::Allow;
                    }
                    // Links an extension pop-up opens go to a normal tab, and the pop-up closes.
                    let _ = tx.send(Ev::NewTab(url, TabKind::Normal));
                    let _ = tx.send(Ev::Content(EXT_POPUP_ID, ContentMsg::Key { action: "escape".into() }));
                    c.request_repaint();
                    NewWindowResponse::Deny
                }
            })
            .with_navigation_handler({
                let (tx, c) = (self.tx.clone(), self.ctx.clone());
                move |url| {
                    let l = url.to_lowercase();
                    if l.starts_with("chrome-extension:") || l.starts_with("about:") || l.starts_with("data:") || l.starts_with("blob:") {
                        return true;
                    }
                    crashlog::debug(format!("extension pop-up navigated to {url:.200}"));
                    // WebView2 swaps an extension page for Edge's new-tab page shortly after it
                    // loads; ignore that and keep the extension showing.
                    if l.starts_with("chrome-search:") || l.starts_with("chrome://newtab") || l.starts_with("edge://newtab") {
                        return false;
                    }
                    // A website opens as a tab instead; the browser's own new-tab page just closes the pop-up.
                    if l.starts_with("http") {
                        let _ = tx.send(Ev::NewTab(url, TabKind::Normal));
                    }
                    let _ = tx.send(Ev::Content(EXT_POPUP_ID, ContentMsg::Key { action: "escape".into() }));
                    c.request_repaint();
                    false
                }
            })
            .with_ipc_handler(move |req: Request<String>| {
                if let Ok(m) = serde_json::from_str::<ContentMsg>(req.body()) {
                    let _ = tx.send(Ev::Content(EXT_POPUP_ID, m));
                    c.request_repaint();
                }
            });
        match b.build_as_child(window) {
            Ok(wv) => {
                let _ = wv.focus();
                self.ext_popup = Some((wv, folder.to_string(), anchor));
            }
            Err(e) => self.toast(format!("Couldn't open the extension: {e}")),
        }
    }

    /// Every few seconds: memory saver for background tabs, CPU cores/priority for new page processes.
    fn housekeeping(&mut self, ctx: &egui::Context) {
        let t = now(ctx);
        if t - self.last_housekeeping < 5.0 {
            return;
        }
        let first = self.last_housekeeping == 0.0;
        self.last_housekeeping = t;
        let st = self.settings();
        #[cfg(windows)]
        if t - self.last_ext_query > 10.0 || (self.ext_ids.is_empty() && t - self.last_ext_query > 2.0) {
            self.last_ext_query = t;
            if let Some(tab) = self.tabs.iter().find(|x| x.kind == TabKind::Normal) {
                let (tx, c) = (self.tx.clone(), self.ctx.clone());
                winhooks::query_extensions(&tab.webview, move |list| {
                    let _ = tx.send(Ev::ExtIds(list));
                    c.request_repaint();
                });
            }
        }
        let active = self.active;
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            if i == active {
                tab.hidden_since = None;
                if tab.low_mem {
                    tab.low_mem = false;
                    #[cfg(windows)]
                    winhooks::set_memory_low(&tab.webview, false);
                }
            } else {
                let since = *tab.hidden_since.get_or_insert(t);
                if st.sleep_after_min > 0 && !tab.low_mem && t - since > st.sleep_after_min as f64 * 60.0 {
                    tab.low_mem = true;
                    crashlog::debug(format!("memory saver: tab {} resting", tab.id));
                    #[cfg(windows)]
                    winhooks::set_memory_low(&tab.webview, true);
                }
            }
        }
        #[cfg(windows)]
        if first || (t - self.last_cpu > 15.0 && (st.cpu_cores != 0 || st.cpu_priority != "normal")) || self.cpu_dirty {
            self.last_cpu = t;
            self.cpu_dirty = false;
            for kind in [TabKind::Normal, TabKind::Private, TabKind::Tor] {
                let wv = self.tabs.iter().find(|x| x.kind == kind).map(|x| &x.webview);
                if wv.is_some() || kind == TabKind::Normal {
                    winhooks::apply_cpu(wv, st.cpu_cores, &st.cpu_priority);
                }
            }
        }
        let _ = first;
    }

    fn cosmetic(&mut self, id: u32, url: &str, classes: Vec<String>, ids: Vec<String>, first: bool) {
        let ab = self.sh.adblock.clone();
        let Some(i) = self.tab_index(id) else { return };
        if !url.starts_with("http") || !ab.active_on(&adblock::host_of(url)) {
            return;
        }
        let cos = self.tabs[i].cos.clone();
        let (tx, c, url) = (self.tx.clone(), self.ctx.clone(), url.to_string());
        let _ = self.cos_tx.send(Box::new(move || {
            let t0 = std::time::Instant::now();
            let mut js = String::new();
            let mut st = cos.lock().unwrap();
            if first {
                let (css, exc, generic) = ab.page_css(&url);
                *st = (exc, generic);
                js.push_str(&format!("window.__goatr&&__goatr.css('__goatr_ab',{},false);", serde_json::to_string(&css).unwrap_or_default()));
            }
            if st.1 && (!classes.is_empty() || !ids.is_empty()) {
                let css = ab.generic_css(&classes, &ids, &st.0);
                if !css.is_empty() {
                    js.push_str(&format!("window.__goatr&&__goatr.css('__goatr_ab2',{},true);", serde_json::to_string(&css).unwrap_or_default()));
                }
            }
            drop(st);
            crashlog::debug(format!("cosmetic {url:.120} first={first} classes={} ids={} css={}B in {:?}", classes.len(), ids.len(), js.len(), t0.elapsed()));
            if !js.is_empty() {
                let _ = tx.send(Ev::Eval(id, js));
                c.request_repaint();
            }
        }));
    }

    /// Put every webview at the content rect (only when it changed).
    fn place_webviews(&mut self) {
        let r = self.content_rect;
        let key = [r.min.x.round() as i32, r.min.y.round() as i32, r.width().round() as i32, r.height().round() as i32];
        if self.last_bounds == Some(key) {
            return;
        }
        self.last_bounds = Some(key);
        for t in &self.tabs {
            let _ = t.webview.set_bounds(wry_rect(r));
        }
    }
}

/// Shortcut names sent by inject.js from inside web pages.
fn key_action(a: &str, tab: u32) -> Option<Cmd> {
    if let Some(n) = a.strip_prefix("select:") {
        return n.parse().ok().map(Cmd::SelectIndex);
    }
    Some(match a {
        "new_tab" => Cmd::NewTab,
        "new_tor_tab" => Cmd::NewTorTab,
        "new_private_tab" => Cmd::NewPrivateTab,
        "close_tab" => Cmd::CloseTab(tab),
        "reopen" => Cmd::ReopenTab,
        "next_tab" => Cmd::NextTab,
        "prev_tab" => Cmd::PrevTab,
        "focus_address" => Cmd::FocusAddress,
        "bookmark" => Cmd::ToggleBookmark,
        "find" => Cmd::FindOpen(true),
        "history" => Cmd::OpenPage("history"),
        "downloads" => Cmd::OpenPage("downloads"),
        "bookmarks" => Cmd::OpenPage("bookmarks"),
        "settings" => Cmd::OpenPage("settings"),
        "toggle_bar" => Cmd::ToggleBar,
        "reload" => Cmd::Reload,
        "fullscreen" => Cmd::Fullscreen,
        "devtools" => Cmd::Devtools,
        "print" => Cmd::Print,
        "back" => Cmd::Back,
        "forward" => Cmd::Forward,
        "home" => Cmd::Home,
        "escape" => Cmd::ClosePopup,
        "install_extension" => Cmd::InstallExtension,
        _ => return None,
    })
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        #[cfg(target_os = "linux")]
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        let frame_t0 = std::time::Instant::now();
        crashlog::phase("frame: webview events");
        crashlog::set_hidden(ui.ctx().input(|i| i.viewport().minimized.unwrap_or(false)));
        let mut cmds = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            self.handle_ev(ev, frame, &mut cmds);
        }
        // Escape from a page also leaves full screen / closes the find bar.
        if cmds.contains(&Cmd::ClosePopup) {
            if self.fullscreen {
                cmds.push(Cmd::Fullscreen);
            } else if self.find_open {
                cmds.push(Cmd::FindOpen(false));
            }
        }
        if let Some(t) = ui.ctx().system_theme() {
            self.sh.system_dark.store(t == egui::Theme::Dark, Ordering::Relaxed);
        }

        // A press anywhere in Goatr's own UI closes an open extension pop-up.
        if self.ext_popup.is_some() && ui.ctx().input(|i| i.pointer.any_pressed()) {
            let folder = self.ext_popup.take().map(|p| p.1).unwrap_or_default();
            self.ext_closed = Some((folder, now(ui.ctx())));
        }
        // The window lost focus to another app: close address suggestions.
        if matches!(self.popup, Some((Popup::Suggest, _))) && ui.ctx().input(|i| i.viewport().focused == Some(false)) && !self.popup_had_focus {
            let t = now(ui.ctx());
            let since: f64 = ui.ctx().data(|d| d.get_temp(egui::Id::new("unfocused-at")).unwrap_or(t));
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("unfocused-at"), since));
            if t - since > 0.3 {
                self.popup = None;
                self.addr_editing = false;
            }
        } else {
            ui.ctx().data_mut(|d| d.remove::<f64>(egui::Id::new("unfocused-at")));
        }
        crashlog::phase("frame: toolbar");
        self.draw_chrome(ui, &mut cmds);
        crashlog::phase("frame: pop-up windows");
        self.draw_popups(ui.ctx(), &mut cmds);

        crashlog::phase("frame: commands");
        for c in cmds {
            self.run(c, frame);
        }
        crashlog::phase("frame: housekeeping");
        self.housekeeping(ui.ctx());
        crashlog::phase("frame: place webviews");
        self.place_webviews();

        if let Some(due) = self.save_due {
            if now(&self.ctx) > due {
                self.sh.store.lock().unwrap().save();
                self.save_due = None;
            }
        }
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            if self.settings().clear_cookies_on_exit {
                if let Some(t) = self.tabs.iter().find(|t| t.kind == TabKind::Normal) {
                    let _ = t.webview.clear_all_browsing_data();
                }
            }
            self.sh.store.lock().unwrap().save();
            self.sh.tor.stop();
        }
        let ft = frame_t0.elapsed();
        if ft.as_millis() > 150 {
            crashlog::debug(format!("slow frame {ft:?}"));
        }
        crashlog::phase("between frames (egui painting / window events)");
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.sh.store.lock().unwrap().save();
        self.sh.tor.stop();
    }
}
