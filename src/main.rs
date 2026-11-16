// Goat — a web browser written in Rust.
//
// The browser UI (tab strip, toolbar, address bar, bookmarks bar, menus, ad-blocker panel…) is drawn
// natively in Rust with egui. Web pages are shown by Microsoft Edge WebView2 (via wry), which runs
// their JavaScript. Pop-up panels are separate native windows so they can float over web pages.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(deprecated)]

mod adblock;
#[cfg(windows)]
mod audiorelay;
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
mod updater;
#[cfg(windows)]
mod instance;
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

/// YouTube ad removal (its ads come with the video data, so address blocking can't stop them).
const YOUTUBE_JS: &str = include_str!("youtube.js");
const INJECT_JS: &str = include_str!("inject.js");
/// Per-tab volume (Goat's speaker button).
const VOLUME_JS: &str = include_str!("volume.js");
const PASTE_JS: &str = include_str!("paste.js");
const PIP_JS: &str = include_str!("pip.js");

/// Title of the floating preview of a tab being dragged out (so it's skipped when looking for
/// the window under the mouse).
pub const DRAG_PREVIEW_TITLE: &str = "Goat tab drag";

/// The standby window has its window and engine up (it only takes tabs after that).
static STANDBY_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The standby window has taken a tab and is a normal window now.
static STANDBY_ADOPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Where the built-in goatr:// pages live inside the webview.
#[cfg(windows)]
pub const INTERNAL_BASE: &str = "http://goatr.localhost";
#[cfg(not(windows))]
pub const INTERNAL_BASE: &str = "goatr://localhost";

/// Events coming from webviews and background jobs.
#[derive(Debug)]
pub enum Ev {
    PageFailed(u32, &'static str),
    /// The page's icon changed (PNG).
    Favicon(u32, Vec<u8>),
    Redirect(u32, String),
    Eval(u32, String),
    ExtIds(Vec<(String, String)>),
    /// A page wants to open an app: (link, page it came from).
    AppLink(String, String),
    /// Right-click in a tab.
    PageMenu(u32, shared::PageMenu),
    /// A tab started (true) or stopped playing sound.
    Audio(u32, bool),
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
    /// A site asked for a file: is there an image in the clipboard? (answered with __goatrClip)
    ClipImage { id: u64 },
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
    /// Remove a site from history so it stops being suggested.
    ForgetSuggestion(String),
    /// Turn "Audio for Discord streaming" on or off.
    ToggleDiscordAudio,
    /// Show the Tor circuit for the current site (anchor: below the Tor button).
    ShowTorCircuit(egui::Pos2),
    /// Ask Tor for new circuits, then reload the page.
    NewTorCircuit,
    /// Install the downloaded update now (Goat closes and reopens).
    InstallUpdate,
    /// Look for a new version now.
    CheckUpdate,
    /// Drag a tab to a new place in the strip.
    MoveTab(u32, usize),
    /// A tab dropped outside the strip: into another Goat window, or a new window of its own.
    TearOff(u32),
    /// Set the active tab's volume (1.0 = 100%).
    SetVolume(f32),
    /// Right-click menu: run the engine's own command (copy, paste, save image…).
    PageNative(i32),
    CopyText(String),
    OpenPrivate(String),
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
    /// A window with hardware acceleration off (its own process and engine).
    NewNoGpuWindow,
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
    /// Open the zoom bubble without changing the zoom.
    ShowZoom,
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
/// Goat closes; Tor tabs do too, and route everything through Tor. Neither keeps history.
/// The floating preview of a tab pulled out of the strip.
#[derive(Clone)]
pub struct TearPreview {
    pub title: String,
    pub icon: Option<egui::TextureId>,
    pub born: f64,
    /// Let go: (when, where on screen, in points). It stays there until the window shows.
    pub drop_at: Option<(f64, egui::Pos2)>,
    /// Fading out since.
    pub fade_from: Option<f64>,
}

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
    /// Zoom bubble under the address bar (Ctrl +/−).
    Zoom,
    /// Tor window: the relays this site goes through (like Tor Browser's circuit display).
    TorCircuit,
    /// The tab's volume slider (speaker button in the address bar).
    Volume,
    /// Right-click menu for a page (links, images, text).
    PageMenu,
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
    /// This tab's volume (1.0 = 100%, up to 6.0) and whether it's playing sound right now.
    pub volume: f32,
    /// Its first page has drawn (until then the web view stays hidden, so no white flash).
    pub painted: bool,
    pub playing: bool,
    /// The page's icon: (site it's for, picture).
    pub favicon: Option<(String, egui::TextureHandle)>,
    /// A reopened tab that hasn't loaded yet (it loads when you first show it).
    pub pending: Option<String>,
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
    /// Put the cursor back in the address bar (no select-all), e.g. after removing a suggestion.
    pub addr_refocus: bool,
    /// The zoom bubble closes itself after this time unless the pointer is on it.
    pub zoom_bubble_until: f64,
    /// What was right-clicked, while Goat's page menu is open.
    pub page_menu: Option<shared::PageMenu>,
    /// This window's profile logo (emoji or letter, colour), shown only when there's more than
    /// one profile. Refreshed every few seconds and when settings change.
    pub profile_badge: Option<(String, egui::Color32)>,
    /// Start-up "Who's browsing?" screen is showing (no tabs yet).
    pub choosing_profile: bool,
    /// Frame timing (for the debug log) and the monitor's refresh rate.
    frame_stats: (std::time::Instant, u32, f32, f32),
    refresh_hz: u32,
    pub profile_always: bool,
    /// The Tor circuit panel's contents (filled in by a background lookup).
    pub tor_circuit: Arc<Mutex<TorCircuitView>>,
    /// A downloaded update waiting to be installed (Update button in the toolbar).
    pub update: Option<updater::Ready>,
    /// Newer version you have to download yourself (portable copy).
    pub update_manual: Option<String>,
    last_update_check: f64,
    update_checking: bool,
    update_started: bool,
    /// No-GPU window: when it last sent its cookies to the normal window, and the send that
    /// runs while it closes (started at, finished).
    last_cookie_share: f64,
    closing_share: Option<(f64, Arc<std::sync::atomic::AtomicBool>)>,
    /// Page sound played from Goat.exe for Discord (experimental).
    #[cfg(windows)]
    relay: Option<audiorelay::Relay>,
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
    /// The tab being dragged: (id, where in the tab it was grabbed, has it moved).
    pub tab_drag: Option<(u32, f32, bool)>,
    /// The floating preview of a tab pulled out of the strip: (title, icon, appeared at,
    /// dropped at).
    pub tear_preview: Option<TearPreview>,
    /// This is the hidden window kept ready for dragged-out tabs (until it takes one).
    pub standby: bool,
    /// A dragged-out tab waiting for the standby window's answer.
    tearing: Option<u32>,
    /// The standby window said no: open a new window for this tear-off instead.
    tear_fallback: bool,
    /// Tell this process (the window a tab was dragged out of) when this window shows.
    ready_pid: Option<u32>,
    shown_frames: u32,
    last_standby_check: f64,
    last_history_prune: f64,
    /// Site icons loaded for the bookmarks bar (host → picture, None = there isn't one).
    pub site_icons: std::cell::RefCell<std::collections::HashMap<String, Option<egui::TextureHandle>>>,
    /// A pop-up that is animating closed: (popup, anchor, start time).
    pub popup_closing: Option<(Popup, Pos2, f64)>,
    /// Which pop-up last closed by itself (focus moved away) and when.
    pub popup_closed_at: Option<(Popup, f64)>,
    /// Which pop-up the window is ready (hidden setup done) for.
    pub popup_ready_for: Option<Popup>,
    pub popup_ready: bool,
    start_url: Option<String>,
    /// Normal, private or Tor window (each is its own Goat process).
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
    /// Frames a pop-up has been drawn while visible (it stays transparent for the first two).
    pub popup_vis_frames: u32,
    /// Window background (theme colour), so nothing flashes black before Goat draws.
    pub clear: [f32; 4],
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
    // Helper process for "Audio for streaming" (see audiorelay.rs).
    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--audio-relay") {
            if let Some(pid) = args.get(i + 1).and_then(|p| p.parse::<u32>().ok()) {
                audiorelay::helper_main(pid);
            }
            return Ok(());
        }
    }
    crashlog::install();
    crashlog::note("start");
    // Linux only (for development): WebKitGTK needs GTK running.
    #[cfg(target_os = "linux")]
    let _ = gtk::init();
    let launch = store::launch().clone();
    // "Always open this profile": hand over to that profile's window straight away.
    let new_window = std::env::args().any(|a| a == "--new-window");
    if !new_window && launch.window.is_empty() && launch.profile.is_empty() && store::profiles().len() > 1 {
        if let Some(p) = store::startup_profile().filter(|p| !p.is_empty()) {
            if let Ok(exe) = std::env::current_exe() {
                let mut cmd = std::process::Command::new(exe);
                cmd.args(["--profile", &p]);
                if let Some(u) = &launch.url {
                    cmd.arg(u);
                }
                if cmd.spawn().is_ok() {
                    return Ok(());
                }
            }
        }
    }
    // Goat is already open (same profile): hand the link to it instead of opening a window.
    #[cfg(windows)]
    if launch.window.is_empty() && !std::env::args().any(|a| a == "--new-window") {
        if let Some(u) = &launch.url {
            if instance::forward(&launch.profile, u) {
                return Ok(());
            }
        }
    }
    // Private, Tor, no-GPU and profile windows get their own taskbar button instead of being
    // grouped with the main window (the main window keeps Windows' default, so pins still work).
    #[cfg(windows)]
    {
        let id = match (launch.window.as_str(), launch.profile.as_str()) {
            ("private", _) => Some("Goat.Private".to_string()),
            ("tor", _) => Some("Goat.Tor".to_string()),
            ("nogpu", _) => Some("Goat.NoGPU".to_string()),
            (_, "") => None,
            (_, p) => Some(format!("Goat.Profile.{}", p.replace(' ', "_"))),
        };
        if let Some(id) = id {
            unsafe {
                let _ = windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(&windows::core::HSTRING::from(id));
            }
        }
    }
    let window_kind = match launch.window.as_str() {
        "private" => TabKind::Private,
        "tor" => TabKind::Tor,
        _ => TabKind::Normal,
    };
    // Private and Tor windows don't save anything back.
    // (A no-hardware-acceleration window is a second Goat process: it reads your settings and
    // bookmarks but leaves saving them to the main window.)
    store::READ_ONLY.store(window_kind != TabKind::Normal || store::no_gpu_window(), Ordering::Relaxed);
    let store = Store::load();
    let settings_now = store.data.settings.clone();
    let st = store.data.settings.clone();
    let adblock = AdBlock::new(st.adblock, Level::parse(&st.adblock_level), &st.adblock_allow, &st.adblock_custom);

    let viewport = egui::ViewportBuilder::default()
        .with_title(match (window_kind, launch.profile.as_str()) {
            (TabKind::Private, _) => "Goat — Private window".to_string(),
            (TabKind::Tor, _) => "Goat — Tor window".to_string(),
            (_, "") if store::no_gpu_window() => "Goat — No hardware acceleration".to_string(),
            (_, "") => "Goat".to_string(),
            (_, p) => format!("Goat — {p}"),
        })
        .with_app_id("goat")
        .with_decorations(false)
        // The GPU surface format is picked from this window, and pop-ups need one with
        // transparency to be see-through. Goat paints its own background, so this looks the same.
        .with_transparent(true)
        .with_inner_size(launch_size().unwrap_or([1300.0, 860.0]))
        .with_min_inner_size([520.0, 320.0])
        .with_icon(app_icon());
    // The standby window: one per profile, hidden until a dragged-out tab needs it.
    let standby = std::env::args().any(|a| a == "--standby");
    #[cfg(windows)]
    let standby_pipe = if standby {
        match instance::claim_standby(&launch.profile) {
            Some(h) => Some(h),
            None => return Ok(()), // there's one already
        }
    } else {
        None
    };
    // Kept off-screen and off the taskbar too: the window system may still show a window on
    // its first frame, so it's also hidden again by hand (see ui).
    let viewport = if standby {
        viewport.with_visible(false).with_taskbar(false).with_position([-32000.0, -32000.0]).with_title(format!("Goat standby {}", std::process::id()))
    } else {
        viewport
    };
    let viewport = match launch_pos() {
        Some(p) => viewport.with_position(p),
        None => viewport,
    };
    // Windows: no per-window vsync. With it, every open pop-up window waited for its own screen
    // refresh, halving the frame rate while a menu animated. Frames are paced once instead, by
    // DwmFlush at the start of each frame (Windows composes windows, so there's no tearing).
    let mut options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Glow, ..Default::default() };
    options.glow_options.vsync = !cfg!(windows);

    let result = eframe::run_native(
        "Goat",
        options,
        Box::new(move |cc| {
            let (tx, rx) = channel::<Ev>();
            let ctx = cc.egui_ctx.clone();
            // Ctrl +/− zooms the page, not Goat's own UI.
            ctx.options_mut(|o| o.zoom_with_keyboard = false);
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
                    cookie_results: Default::default(),
                    next_cookie_req: Default::default(),
                })
            };
            {
                let c = ctx.clone();
                crashlog::set_probe(move || c.request_repaint());
            }
            let _ = std::fs::create_dir_all(extinstall::ext_dir());
            // Extension tabs open in Goat (must run before extensions load).
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
                addr_refocus: false,
                zoom_bubble_until: 0.0,
                page_menu: None,
                tor_circuit: Default::default(),
                update: None,
                update_manual: None,
                last_update_check: f64::NEG_INFINITY,
                update_checking: false,
                update_started: false,
                last_cookie_share: 0.0,
                closing_share: None,
                profile_badge: None,
                choosing_profile: false,
                frame_stats: (std::time::Instant::now(), 0, 0.0, 0.0),
                refresh_hz: 0,
                profile_always: false,
                #[cfg(windows)]
                relay: None,
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
                ctxs: Contexts { normal: WebContext::new(Some(store::engine_dir())), private: None, tor: None, proto: [false; 3], args: [None, None, None] },
                tabs: Vec::new(),
                active: 0,
                next_id: 1,
                closed: Vec::new(),
                ghosts: Vec::new(),
                tab_rects: Default::default(),
                tab_drag: None,
                tear_preview: None,
                standby,
                tearing: None,
                tear_fallback: false,
                ready_pid: std::env::args().find_map(|a| a.strip_prefix("--ready-pid=").and_then(|p| p.parse().ok())),
                shown_frames: 0,
                last_standby_check: 0.0,
                last_history_prune: 0.0,
                site_icons: Default::default(),
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
                popup_vis_frames: 0,
                clear: [0.06, 0.07, 0.08, 1.0],
                toast: None,
                fullscreen: false,
                content_rect: Rect::from_min_size(Pos2::new(0.0, 114.0), Vec2::new(1300.0, 746.0)),
                last_bounds: None,
                save_due: None,
                fonts_ok,
            };
            app.window_kind = window_kind;
            #[cfg(windows)]
            #[cfg(windows)]
            if let Some(h) = standby_pipe {
                let sh = app.sh.clone();
                let title = format!("Goat standby {}", std::process::id());
                instance::serve_standby(h, move |req| match req {
                    instance::StandbyReq::Adopt(url, r, tx) => {
                        // Still starting up, or already taken: say no (the other window opens a
                        // new one instead). Otherwise show right here (a hidden window isn't
                        // drawing), then load the tab.
                        let ok = STANDBY_READY.load(Ordering::SeqCst)
                            && !STANDBY_ADOPTED.swap(true, Ordering::SeqCst)
                            && winhooks::show_window_by_title(&title, r);
                        crashlog::debug(format!("standby: adopt {} → {ok}", url.chars().take(80).collect::<String>()));
                        if ok {
                            (sh.notify)(shared::Notice::StandbyAdopt(url));
                        }
                        let _ = tx.send(ok);
                    }
                    // Hidden, so it isn't drawing frames: just go.
                    instance::StandbyReq::Quit => std::process::exit(0),
                });
                // Gone with the window that started it (another window starts a new one).
                if let Some(parent) = std::env::args().find_map(|a| a.strip_prefix("--standby-for=").and_then(|p| p.parse::<u32>().ok())) {
                    std::thread::spawn(move || {
                        // Hold a handle to the parent and wait on it: a polled process id can be
                        // reused by another program, which kept old standbys alive for ever.
                        store::wait_for_exit(parent);
                        loop {
                            if !STANDBY_ADOPTED.load(Ordering::SeqCst) {
                                std::process::exit(0);
                            }
                            std::thread::sleep(std::time::Duration::from_secs(5));
                        }
                    });
                }
            }
            #[cfg(windows)]
            if window_kind == TabKind::Normal && store::launch().window.is_empty() && !standby {
                let sh = app.sh.clone();
                instance::listen(&store::launch().profile, move |req| {
                    (sh.notify)(match req {
                        instance::Request::Open(url) => shared::Notice::External(url),
                        instance::Request::CookiesGet(tx) => shared::Notice::CookiesGet(tx),
                        instance::Request::CookiesPut(list) => shared::Notice::CookiesPut(list),
                    })
                });
            }
            // Tabs dragged in from other Goat windows (same profile and kind only).
            #[cfg(windows)]
            {
                let sh = app.sh.clone();
                let my_kind = match window_kind {
                    TabKind::Normal => "normal",
                    TabKind::Private => "private",
                    TabKind::Tor => "tor",
                };
                instance::listen_window(move |req| match req {
                    instance::WindowReq::Tab(kind, profile, url) => {
                        let ok = kind == my_kind && profile == store::launch().profile;
                        if ok {
                            (sh.notify)(shared::Notice::TabIn(url));
                        }
                        ok
                    }
                    instance::WindowReq::Ready => {
                        (sh.notify)(shared::Notice::TearReady);
                        true
                    }
                });
            }
            // No-GPU window: start with the normal window's cookies (logins carry over).
            #[cfg(windows)]
            if store::no_gpu_window() {
                let sh = app.sh.clone();
                std::thread::spawn(move || {
                    if let Some(list) = instance::cookies_get(&store::launch().profile) {
                        (sh.notify)(shared::Notice::CookiesPut(list));
                    }
                });
            }
            app.start_url = Some(match (start, window_kind) {
                (Some(a), _) => resolve_input(&app.settings(), &a),
                (None, TabKind::Private) => format!("{INTERNAL_BASE}/newtab?private=1"),
                (None, TabKind::Tor) => format!("{INTERNAL_BASE}/tor"),
                (None, TabKind::Normal) => app.home_url(),
            });
            // More than one profile and no "always open": ask which one first.
            let ask = window_kind == TabKind::Normal
                && !std::env::args().any(|a| a == "--new-window")
                && store::launch().window.is_empty()
                && store::launch().profile.is_empty()
                && store::profiles().len() > 1
                && store::startup_profile().is_none();
            if ask {
                app.choosing_profile = true;
            } else {
                // First tab: the window handle is available now.
                let url = app.start_url.take().unwrap();
                // Reopen last time's tabs (asleep until shown), then any link Goat was opened with.
                let saved = if app.session_window() && app.settings().restore_tabs {
                    let s = app.sh.store.lock().unwrap();
                    (s.data.session.clone(), s.data.session_active)
                } else {
                    (Vec::new(), 0)
                };
                let mut restored = 0;
                for (k, t) in saved.0.iter().enumerate() {
                    let active = k == saved.1.min(saved.0.len().saturating_sub(1));
                    let first = if active { t.url.clone() } else { "about:blank".to_string() };
                    if app.open_tab(cc, &first, window_kind).is_ok() {
                        restored += 1;
                        if let Some(tab) = app.tabs.last_mut() {
                            tab.url = t.url.clone();
                            tab.title = t.title.clone();
                            if !active {
                                tab.pending = Some(t.url.clone());
                            }
                        }
                    }
                }
                if restored > 0 {
                    let a = saved.1.min(app.tabs.len() - 1);
                    app.select(a);
                }
                if restored == 0 || store::launch().url.is_some() {
                    if let Err(e) = app.open_tab(cc, &url, window_kind) {
                        eprintln!("couldn't create the first tab: {e}");
                    }
                }
                STANDBY_READY.store(true, Ordering::SeqCst);
            }
            Ok(Box::new(app))
        }),
    );
    if let Err(e) = &result {
        crashlog::write("startup error", &e.to_string());
        return result;
    }
    // Leftover threads (pipe listeners etc.) must not keep the process around.
    std::process::exit(0);
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
    // Before a page paints, show the theme's background instead of a white flash.
    let bg = {
        let st = sh.store.lock().unwrap();
        themes::get(&st.data.settings.theme, sh.system_dark.load(Ordering::Relaxed)).palette(false).bg
    };
    let b = b.with_background_color((bg.r(), bg.g(), bg.b(), 255));
    let s = sh.clone();
    let tor = kind == TabKind::Tor;
    #[cfg(windows)]
    let b = {
        use wry::WebViewBuilderExtWindows;
        let b = b.with_additional_browser_args(args);
        if tor {
            // Tor: never any extensions (they could reveal who you are).
            b
        } else if kind == TabKind::Private {
            // Private: only the extensions you allowed there (added after the first tab opens).
            let allowed = !s.store.lock().unwrap().data.settings.private_extensions.is_empty();
            b.with_browser_extensions_enabled(allowed)
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
    let b = b.with_devtools(true).with_hotkeys_zoom(false).with_clipboard(true);
    if register_proto {
        b.with_custom_protocol("goatr".into(), move |_id, req| { let _g = crashlog::Scope::new("internal page request"); pages::handle(&s, req) })
    } else {
        b
    }
}

pub const EXT_POPUP_ID: u32 = u32::MAX;

#[derive(Clone, Debug, Default)]
pub struct TorCircuitView {
    pub host: String,
    pub loading: bool,
    pub hops: Vec<tor::Hop>,
    pub error: String,
}

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

/// In Goat an extension's pop-up is its own little browser, so "the active tab" would be the
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
            let dir = if kind == TabKind::Private { store::private_dir() } else { store::engine_dir() };
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
            TabKind::Private => self.ctxs.private.get_or_insert_with(|| WebContext::new(Some(store::private_dir()))),
            TabKind::Normal => &mut self.ctxs.normal,
        };

        let (t_nav, c_nav, sh_nav) = (self.tx.clone(), self.ctx.clone(), self.sh.clone());
        let (t_app, c_app) = (self.tx.clone(), self.ctx.clone());
        let webview = common(WebViewBuilder::new_with_web_context(web_ctx), &self.sh, kind, register, &args)
            .with_bounds(wry_rect(self.content_rect))
            .with_url(url)
            .with_initialization_script(INJECT_JS)
            .with_initialization_script(&privacy::page_script(&settings))
            .with_initialization_script(VOLUME_JS)
            .with_initialization_script(if settings.paste_uploads { PASTE_JS } else { "" })
            .with_initialization_script(if settings.pip_button { PIP_JS } else { "" })
            .with_initialization_script(if settings.adblock { YOUTUBE_JS } else { "" })
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
                // edge://… and chrome://… pages (e.g. an extension's "Manage" link): Goat's own page.
                let lower = url.to_lowercase();
                if lower.starts_with("edge://") || lower.starts_with("chrome://") {
                    if let Some(u) = pages::browser_page(&url) {
                        let _ = t_nav.send(Ev::Redirect(id, u));
                        c_nav.request_repaint();
                        return false;
                    }
                }
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
            // Hidden until its first page has drawn: a brand-new web view flashes white first.
            .with_visible(false)
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
            winhooks::watch_load_errors(&webview);
            {
                let (tx, c) = (self.tx.clone(), self.ctx.clone());
                winhooks::watch_external_links(&webview, move |uri, origin| {
                    let _ = tx.send(Ev::AppLink(uri, origin));
                    c.request_repaint();
                });
            }
            // Private window: the extensions you allowed there (once, with its first tab).
            if kind == TabKind::Private {
                static PRIVATE_EXTS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                if !PRIVATE_EXTS.swap(true, Ordering::SeqCst) {
                    for folder in settings.private_extensions.iter().filter(|f| !settings.disabled_extensions.contains(f)).cloned().collect::<Vec<_>>() {
                        let dir = extinstall::ext_dir().join(&folder);
                        if dir.is_dir() {
                            winhooks::add_extension(&webview, &dir, move |r| crashlog::debug(format!("private window extension {folder}: {r:?}")));
                        }
                    }
                }
            }
            {
                let (tx, c) = (self.tx.clone(), self.ctx.clone());
                {
                    let (tx, c) = (tx.clone(), c.clone());
                    winhooks::watch_favicon(&webview, move |png| {
                        let _ = tx.send(Ev::Favicon(id, png));
                        c.request_repaint();
                    });
                }
                {
                    let (tx, c) = (tx.clone(), c.clone());
                    winhooks::watch_audio(&webview, move |playing| {
                        let _ = tx.send(Ev::Audio(id, playing));
                        c.request_repaint();
                    });
                }
                winhooks::watch_context_menu(&webview, move |m| {
                    let _ = tx.send(Ev::PageMenu(id, m));
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
            volume: 1.0,
            painted: false,
            playing: false,
            favicon: None,
            pending: None,
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
        // A reopened tab loads the first time you look at it.
        if let Some(u) = self.tabs[idx].pending.take() {
            let _ = self.tabs[idx].webview.load_url(&u);
        }
        for (i, t) in self.tabs.iter().enumerate() {
            let _ = t.webview.set_visible(i == idx && t.painted);
        }
        let _ = self.tabs[idx].webview.focus();
        self.addr_editing = false;
        self.find_count = None;
        // Switching tabs ends address-bar editing: drop its suggestions too.
        if matches!(self.popup, Some((Popup::Suggest, _))) {
            self.popup = None;
        }
        self.sugg.clear();
        self.sugg_sel = None;
        self.ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("addr")));
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

    /// The window-shaped preview that follows a tab pulled out of the strip, and settles where
    /// the new window opens when you let go.
    fn draw_tear_preview(&mut self, ctx: &egui::Context) {
        let Some(tp) = self.tear_preview.clone() else { return };
        let (title, icon, born) = (tp.title.clone(), tp.icon, tp.born);
        let t = now(ctx);
        // Waiting for the new window, but never forever.
        if tp.fade_from.is_none() && tp.drop_at.is_some_and(|(d, _)| t - d > 5.0) {
            if let Some(p) = self.tear_preview.as_mut() {
                p.fade_from = Some(t);
            }
        }
        let dropped = self.tear_preview.as_ref().and_then(|p| p.fade_from);
        if dropped.is_some_and(|d| t - d > 0.3) {
            self.tear_preview = None;
            return;
        }
        let ppp = ctx.pixels_per_point();
        #[cfg(windows)]
        let cur = winhooks::cursor_pos().map(|(x, y)| egui::pos2(x / ppp, y / ppp));
        #[cfg(not(windows))]
        let cur = ctx.input(|i| i.pointer.latest_pos());
        let Some(cur) = tp.drop_at.map(|d| d.1).or(cur) else { return };
        let (w, h) = (300.0, 200.0);
        // The window is see-through around the card (shadow room).
        let pad = 18.0;
        let pos = egui::pos2(cur.x - 40.0 - pad, cur.y - 14.0 - pad);
        let pal = self.palette();
        let builder = egui::ViewportBuilder::default()
            .with_title(DRAG_PREVIEW_TITLE)
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_taskbar(false)
            .with_active(false)
            .with_always_on_top()
            .with_mouse_passthrough(true)
            .with_position(pos)
            .with_inner_size([w + pad * 2.0, h + pad * 2.0]);
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("goat-tear-preview"), builder, |ui, _| {
            let c = ui.ctx().clone();
            // First frame: no Windows border or rounded frame around the see-through window.
            #[cfg(windows)]
            if dropped.is_none() && t - born < 0.05 {
                panels::round_corners(DRAG_PREVIEW_TITLE);
            }
            let full = ui.max_rect();
            let card = Rect::from_min_size(full.min + egui::vec2(pad, pad), egui::vec2(w, h));
            // Pops in like Goat's menus; on drop, swells a touch and fades as the window opens.
            let (scale, op) = match dropped {
                None => (anim::pop(((t - born) as f32).min(1.0), 0.8), ((t - born) as f32 / 0.08).clamp(0.0, 1.0)),
                Some(d) => {
                    let k = ((t - d) as f32 / 0.3).clamp(0.0, 1.0);
                    (1.0 + 0.04 * anim::out_cubic(k), 1.0 - anim::in_cubic(k))
                }
            };
            let center = card.center();
            let card = Rect::from_center_size(center, card.size() * scale);
            let mut p = ui.painter().clone();
            p.set_opacity(op * 0.96);
            paint::shadow(&p, card, 14.0, pal.dark);
            p.rect_filled(card, 12, pal.bg);
            // Mini tab strip with the tab, then a page.
            let strip = Rect::from_min_size(card.min, egui::vec2(card.width(), 34.0));
            p.rect_filled(strip, egui::CornerRadius { nw: 12, ne: 12, sw: 0, se: 0 }, pal.bg2);
            let tab = Rect::from_min_size(strip.min + egui::vec2(8.0, 6.0), egui::vec2((card.width() * 0.62).min(200.0), 28.0));
            p.rect_filled(tab, egui::CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }, pal.bg3);
            let dot = Rect::from_min_size(egui::pos2(tab.left() + 9.0, tab.center().y - 7.0), egui::Vec2::splat(14.0));
            match icon {
                Some(tex) => {
                    p.image(tex, dot, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                }
                None => {
                    p.rect_filled(dot, 4, pal.line);
                }
            }
            let galley = p.layout_no_wrap(title.clone(), egui::FontId::proportional(12.0), pal.text);
            let clip = Rect::from_min_max(egui::pos2(dot.right() + 7.0, tab.top()), egui::pos2(tab.right() - 8.0, tab.bottom()));
            p.with_clip_rect(clip).galley(egui::pos2(clip.left(), tab.center().y - galley.size().y / 2.0), galley, pal.text);
            let page = Rect::from_min_max(egui::pos2(card.left() + 8.0, strip.bottom() + 8.0), card.max - egui::vec2(8.0, 8.0));
            p.rect_filled(page, 8, pal.bg2);
            for k in 0..4 {
                let y = page.top() + 16.0 + k as f32 * 18.0;
                let wl = page.width() * [0.7, 0.9, 0.55, 0.8][k];
                p.rect_filled(Rect::from_min_size(egui::pos2(page.left() + 14.0, y), egui::vec2(wl - 28.0, 8.0)), 4, pal.bg3);
            }
            c.request_repaint();
        });
        ctx.request_repaint();
    }

    /// A tab dragged out of the strip and let go: into the Goat window under the mouse (same
    /// profile and kind), or, over anything else, a window of its own.
    fn tear_off(&mut self, id: u32) {
        let Some(i) = self.tab_index(id) else { return };
        let (url, kind) = (self.tabs[i].url.clone(), self.tabs[i].kind);
        let kind_s = match kind {
            TabKind::Normal => "normal",
            TabKind::Private => "private",
            TabKind::Tor => "tor",
        };
        #[cfg(windows)]
        {
            let me = std::process::id();
            if let Some((x, y)) = winhooks::cursor_pos() {
                match instance::window_pid_at(x, y) {
                    Some(pid) if pid == me => {
                        self.tear_preview = None;
                        return;
                    }
                    Some(pid) => {
                        if instance::send_tab(pid, kind_s, &store::launch().profile, &url) {
                            self.close(id);
                            self.tear_preview = None;
                            return;
                        }
                    }
                    None => {}
                }
            }
        }
        // Somewhere else: its own window (a lone tab just stays put). The hidden standby
        // window takes it at once; otherwise a new Goat starts and the preview waits for it.
        let fade_now = |me: &mut Self| {
            if let Some(p) = me.tear_preview.as_mut() {
                p.fade_from = Some(now(&me.ctx));
            }
        };
        let ppp = self.ctx.pixels_per_point();
        let size = self.ctx.input(|i| i.viewport().inner_rect).map(|r| r.size()).unwrap_or(egui::vec2(1300.0, 860.0));
        #[cfg(windows)]
        if self.tabs.len() > 1 && kind == TabKind::Normal && !store::no_gpu_window() && self.settings().tear_standby && !self.tear_fallback {
            if let Some((x, y)) = winhooks::cursor_pos() {
                if self.tearing.is_some() {
                    return;
                }
                // Ask the standby window off the UI thread; it answers TearDone.
                self.tearing = Some(id);
                let rect = [x - 60.0 * ppp, y - 16.0 * ppp, size.x * ppp, size.y * ppp];
                let (sh, u, profile) = (self.sh.clone(), url.clone(), store::launch().profile.clone());
                std::thread::spawn(move || {
                    let ok = instance::standby_adopt(&profile, &u, rect);
                    (sh.notify)(shared::Notice::TearDone(id, ok));
                });
                return;
            }
        }
        self.tear_fallback = false;
        if self.tabs.len() <= 1 {
            fade_now(self);
        }
        if self.tabs.len() > 1 {
            let window = match kind {
                TabKind::Normal if store::no_gpu_window() => Some("nogpu"),
                TabKind::Normal => None,
                TabKind::Private => Some("private"),
                TabKind::Tor => Some("tor"),
            };
            if let Ok(exe) = std::env::current_exe() {
                let mut cmd = std::process::Command::new(exe);
                cmd.arg("--new-window");
                if let Some(w) = window {
                    cmd.args(["--window", w]);
                }
                let prof = store::launch().profile.clone();
                if !prof.is_empty() {
                    cmd.args(["--profile", &prof]);
                }
                // Open where you let go, the size of this window, under the waiting preview.
                cmd.arg(format!("--ready-pid={}", std::process::id()));
                #[cfg(not(windows))]
                fade_now(self);
                #[cfg(windows)]
                if let Some((x, y)) = winhooks::cursor_pos() {
                    cmd.arg(format!("--pos={},{}", (x / ppp - 60.0).round(), (y / ppp - 16.0).round()));
                }
                cmd.arg(format!("--size={},{}", size.x.round(), size.y.round()));
                cmd.arg(&url);
                if cmd.spawn().is_ok() {
                    self.close(id);
                }
            }
        }
        let _ = kind_s;
    }

    /// A site's icon (from an open tab or the cache), when site icons are on.
    pub fn site_icon(&self, url: &str) -> Option<egui::TextureId> {
        if !self.settings().site_icons {
            return None;
        }
        let host = store::host_of(url);
        if host.is_empty() {
            return None;
        }
        let mut m = self.site_icons.borrow_mut();
        let e = m.entry(host.clone()).or_insert_with(|| std::fs::read(store::favicon_path(&host)).ok().and_then(|b| decode_icon(&self.ctx, &host, &b)));
        e.as_ref().map(|t| t.id())
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
            // Remember it for this site (not in private/Tor windows).
            let host = store::host_of(&t.url);
            let (z, kind) = (t.zoom, t.kind);
            if !host.is_empty() && !kind.is_private_like() && t.url.starts_with("http") {
                let mut s = self.sh.store.lock().unwrap();
                if (z - 1.0).abs() < 0.001 {
                    s.data.settings.site_zoom.remove(&host);
                } else {
                    s.data.settings.site_zoom.insert(host, z);
                }
                s.save();
            }
        }
        // Chrome-style bubble showing the new zoom (the ⋮ menu has its own zoom row).
        let t = now(&self.ctx);
        self.zoom_bubble_until = t + 2.0;
        if !matches!(self.popup, Some((Popup::Zoom | Popup::Menu, _))) {
            let at = chrome::to_screen(&self.ctx, egui::pos2(self.addr_rect.right(), self.addr_rect.bottom() + 4.0));
            self.popup = Some((Popup::Zoom, at));
            self.popup_had_focus = false;
            self.popup_born = t;
        }
        self.ctx.request_repaint();
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

    /// Start a fresh copy of Goat (after this one has closed) and exit.
    /// Start another Goat window (a separate process): private/Tor and/or another profile.
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
                Some("nogpu") => "Opening a window without hardware acceleration…".to_string(),
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
            Ev::Blocked(_) | Ev::Eval(..) | Ev::ExtIds(_) | Ev::AppLink(..) | Ev::PageMenu(..) | Ev::Audio(..) | Ev::ExtLoaded(..) | Ev::Content(_, ContentMsg::Cosmetic { .. }) => {}
            e => crashlog::note(format!("{:.160}", format!("{e:?}"))),
        }
        match ev {
            Ev::NewTab(url, kind) => {
                // Extensions open "new tab" as chrome-search://local-ntp or chrome://newtab: use Goat's.
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
            Ev::Favicon(id, png) => {
                if let Some(i) = self.tab_index(id) {
                    let url = self.tabs[i].page_url.lock().unwrap().clone();
                    let host = store::host_of(&url);
                    if host.is_empty() || url.starts_with(INTERNAL_BASE) {
                        return;
                    }
                    if let Some(tex) = decode_icon(&self.ctx, &host, &png) {
                        self.tabs[i].favicon = Some((host.clone(), tex.clone()));
                        self.site_icons.borrow_mut().insert(host.clone(), Some(tex));
                        // Kept for bookmarks (not from private or Tor tabs).
                        if self.tabs[i].kind == TabKind::Normal && !store::READ_ONLY.load(Ordering::Relaxed) {
                            let p = store::favicon_path(&host);
                            let _ = std::fs::create_dir_all(p.parent().unwrap_or(std::path::Path::new(".")));
                            let _ = std::fs::write(p, &png);
                        }
                    }
                }
            }
            Ev::Audio(id, playing) => {
                if let Some(i) = self.tab_index(id) {
                    self.tabs[i].playing = playing;
                    // Media that just started gets the tab's volume.
                    let v = self.tabs[i].volume;
                    if playing && (v - 1.0).abs() > 0.001 {
                        let _ = self.tabs[i].webview.evaluate_script(&format!("window.__goatrVolume&&__goatrVolume({v})"));
                    }
                }
            }
            Ev::PageMenu(_id, m) => {
                crashlog::debug(format!("page menu: link={} src={} sel={} edit={} items={:?}", !m.link.is_empty(), !m.src.is_empty(), !m.selection.is_empty(), m.editable, m.native.iter().map(|n| n.0.as_str()).collect::<Vec<_>>()));
                #[cfg(windows)]
                let at = winhooks::cursor_pos().map(|(x, y)| {
                    let ppp = self.ctx.pixels_per_point();
                    egui::pos2(x / ppp, y / ppp)
                });
                #[cfg(not(windows))]
                let at: Option<egui::Pos2> = None;
                let at = at.unwrap_or_else(|| chrome::to_screen(&self.ctx, self.content_rect.center()));
                self.page_menu = Some(m);
                self.popup = Some((Popup::PageMenu, at));
                self.popup_had_focus = false;
                self.popup_born = now(&self.ctx);
            }
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
                let first = !self.ext_ids_known;
                self.ext_ids_known = true;
                if first {
                    self.apply_extension_states();
                }
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
                    "unresponsive" => "This page stopped responding, so Goat reloaded it".to_string(),
                    k => format!("Page problem ({k}), reloading"),
                });
            }
            Ev::Title(id, title) => {
                self.show_painted(id);
                if self.tab_index(id).is_some_and(|i| self.tabs[i].pending.is_some()) {
                    return;
                }
                if let Some(i) = self.tab_index(id) {
                    let t = &mut self.tabs[i];
                    t.title = if title.trim().is_empty() { display_url(&t.url) } else { title };
                    let (u, ti, tor) = (t.url.clone(), t.title.clone(), t.kind.is_private_like());
                    if i == self.active {
                        self.ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{ti} - Goat")));
                    }
                    if !tor && u.starts_with("http") && !u.starts_with(INTERNAL_BASE) {
                        self.sh.store.lock().unwrap().update_title(&u, &ti);
                    }
                }
            }
            Ev::Load(id, done, url) => {
                if done {
                    self.show_painted(id);
                }
                // A reopened tab still asleep: it keeps its saved address and title.
                if let Some(i) = self.tab_index(id).filter(|i| self.tabs[*i].pending.is_some()) {
                    self.tabs[i].loading = false;
                    return;
                }
                // A different site: its own icon comes with FaviconChanged (none until then).
                if let Some(i) = self.tab_index(id) {
                    let host = store::host_of(&url);
                    if self.tabs[i].favicon.as_ref().is_some_and(|(h, _)| *h != host) {
                        self.tabs[i].favicon = None;
                    }
                }
                // Goat's own pages get the theme colour behind them (no white flash);
                // websites get white, like every browser, for pages with no background of their own.
                if done {
                    if let Some(i) = self.tab_index(id) {
                        let v = self.tabs[i].volume;
                        if (v - 1.0).abs() > 0.001 {
                            let _ = self.tabs[i].webview.evaluate_script(&format!("window.__goatrVolume&&__goatrVolume({v})"));
                        }
                    }
                }
                let internal = url.starts_with(INTERNAL_BASE) || url.is_empty() || url.starts_with("about:");
                let bg = if internal { self.palette().bg } else { egui::Color32::WHITE };
                if let Some(i) = self.tab_index(id) {
                    let _ = self.tabs[i].webview.set_background_color((bg.r(), bg.g(), bg.b(), 255));
                }
                if let Some(i) = self.tab_index(id) {
                    let t = &mut self.tabs[i];
                    t.loading = !done;
                    *t.page_url.lock().unwrap() = url.clone();
                    let new_host = store::host_of(&url) != store::host_of(&t.url);
                    t.url = url;
                    if new_host || done {
                        let host = store::host_of(&t.url);
                        let want = if t.kind.is_private_like() { None } else { self.sh.store.lock().unwrap().data.settings.site_zoom.get(&host).copied() };
                        let want = want.unwrap_or(1.0);
                        if (t.zoom - want).abs() > 0.001 {
                            t.zoom = want;
                            let _ = t.webview.zoom(want);
                        }
                    }
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
                    ContentMsg::ClipImage { id: req } => {
                        let js = match clipboard_image() {
                            Some((b64, mime, w, h)) => format!("window.__goatrClip&&window.__goatrClip({req},\"{b64}\",\"{mime}\",{w},{h})"),
                            None => format!("window.__goatrClip&&window.__goatrClip({req},null)"),
                        };
                        if let Some(i) = self.tab_index(id) {
                            let _ = self.tabs[i].webview.evaluate_script(&js);
                        }
                    }
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
                    #[cfg(windows)]
                    if !self.standby && !self.settings().tear_standby {
                        let p = store::launch().profile.clone();
                        std::thread::spawn(move || {
                            if instance::standby_alive(&p) {
                                instance::standby_quit(&p);
                            }
                        });
                    }
                    let s = self.settings();
                    *self.sh.net_privacy.write().unwrap() = shared::NetPrivacy::from(&s);
                    self.cpu_dirty = true;
                    self.sh.adblock.set_enabled(s.adblock);
                    self.sh.adblock.set_level(Level::parse(&s.adblock_level));
                    self.sh.adblock.set_custom(&s.adblock_custom);
                    // "Audio for streaming" switched in Settings.
                    self.apply_discord_audio();
                    self.refresh_profile_badge();
                    self.sh.adblock.set_allowlist(&s.adblock_allow);
                }
                Notice::ClearBrowsingData => {
                    if let Some(t) = self.tabs.iter().find(|t| t.kind == TabKind::Normal) {
                        let _ = t.webview.clear_all_browsing_data();
                    }
                }
                Notice::OpenTorTab => cmds.push(Cmd::NewTorTab),
                Notice::OpenProfile(name) => cmds.push(Cmd::OpenProfile(name)),
                Notice::CheckUpdate => self.check_update(true),
                Notice::CookiesGet(tx) => {
                    #[cfg(windows)]
                    match self.tabs.iter().find(|t| t.kind == TabKind::Normal) {
                        Some(t) => winhooks::cookie_op(&t.webview, shared::CookieOp::List, move |r| {
                            let _ = tx.send(r.map(|v| v.to_string()).unwrap_or_else(|_| "[]".into()));
                        }),
                        None => {
                            let _ = tx.send("[]".into());
                        }
                    }
                    #[cfg(not(windows))]
                    let _ = tx.send("[]".into());
                }
                Notice::CookiesPut(list) => {
                    #[cfg(windows)]
                    if let Some(t) = self.tabs.iter().find(|t| t.kind == TabKind::Normal) {
                        let n = list.len();
                        for c in list {
                            winhooks::cookie_op(&t.webview, shared::CookieOp::Set { cookie: c, original: None }, |_| {});
                        }
                        crashlog::debug(format!("cookies shared from the other window: {n}"));
                    }
                    #[cfg(not(windows))]
                    let _ = list;
                }
                Notice::TearDone(id, ok) => {
                    self.tearing = None;
                    crashlog::debug(format!("tear-off to standby: {ok}"));
                    if ok {
                        self.close(id);
                        if let Some(p) = self.tear_preview.as_mut() {
                            p.fade_from.get_or_insert(now(&self.ctx));
                        }
                    } else {
                        self.tear_fallback = true;
                        self.tear_off(id);
                    }
                }
                Notice::TearReady => {
                    if let Some(p) = self.tear_preview.as_mut() {
                        p.fade_from.get_or_insert(now(&self.ctx));
                    }
                }
                Notice::StandbyAdopt(url) => {
                    self.standby = false;
                    if self.tabs.is_empty() {
                        let _ = self.open_tab(frame, &url, TabKind::Normal);
                    } else {
                        // The new-tab page it kept ready stays hidden; the tab's page shows once drawn.
                        if let Some(t) = self.tabs.get_mut(self.active) {
                            t.painted = false;
                            let _ = t.webview.set_visible(false);
                            t.url = url.clone();
                            t.title = String::new();
                        }
                        self.load_active(&url);
                    }
                    let title = match store::launch().profile.as_str() {
                        "" => "Goat".to_string(),
                        p => format!("Goat — {p}"),
                    };
                    self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Title(title));
                    self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(true));
                    self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
                }
                Notice::StandbyQuit => {
                    if self.standby {
                        self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
                    }
                }
                Notice::TabIn(url) => {
                    if let Err(e) = self.open_tab(frame, &url, self.window_kind) {
                        self.toast(format!("Couldn't open tab: {e}"));
                    }
                    self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
                }
                Notice::External(raw) => {
                    let url = resolve_input(&self.settings(), &raw);
                    if let Err(e) = self.open_tab(frame, &url, TabKind::Normal) {
                        self.toast(format!("Couldn't open tab: {e}"));
                    }
                    self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Minimized(false));
                    self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
                    #[cfg(windows)]
                    {
                        let hwnd = panels::MAIN_HWND.load(Ordering::Relaxed);
                        if hwnd != 0 {
                            unsafe {
                                let h = windows::Win32::Foundation::HWND(hwnd as *mut _);
                                if windows::Win32::UI::WindowsAndMessaging::IsIconic(h).as_bool() {
                                    let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(h, windows::Win32::UI::WindowsAndMessaging::SW_RESTORE);
                                }
                                let _ = windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(h);
                            }
                        }
                    }
                }
                Notice::UpdateChecked(r, asked) => {
                    self.update_checking = false;
                    match r {
                        Ok(updater::Found::Ready(u)) => {
                            if asked || self.update.as_ref().map(|x| &x.version) != Some(&u.version) {
                                self.toast(format!("Goat {} is ready. Click Update in the toolbar, or it installs when you close Goat.", u.version));
                            }
                            self.update = Some(u);
                        }
                        Ok(updater::Found::Manual(v)) => {
                            if asked || self.update_manual.as_ref() != Some(&v) {
                                self.toast(format!("Goat {v} is out. Click Update in the toolbar to download it."));
                            }
                            self.update_manual = Some(v);
                        }
                        Ok(updater::Found::UpToDate) => {
                            if asked {
                                self.toast(format!("Goat {} is up to date", updater::VERSION));
                            }
                        }
                        Err(e) => {
                            crashlog::debug(format!("update check: {e}"));
                            if asked {
                                self.toast(e);
                            }
                        }
                    }
                }
                Notice::ExtensionEnabled(folder, on) => {
                    if !self.ext_ids_known {
                        self.toast("Applies once extensions have loaded");
                    }
                    self.apply_extension_states();
                    if !on {
                        self.toast("Extension turned off");
                    }
                    let _ = folder;
                }
                Notice::Cookies(id, op) => {
                    let sh = self.sh.clone();
                    let answer = move |r: Result<serde_json::Value, String>| {
                        let v = r.unwrap_or_else(|e| serde_json::json!({ "error": e }));
                        sh.cookie_results.lock().unwrap().insert(id, v);
                    };
                    #[cfg(windows)]
                    match self.tabs.first() {
                        Some(t) => winhooks::cookie_op(&t.webview, op, answer),
                        None => answer(Err("Open a tab first.".into())),
                    }
                    #[cfg(not(windows))]
                    {
                        let _ = op;
                        answer(Err("The cookie editor needs Windows.".into()));
                    }
                }
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
                    let _ = popup; // Goat shows extension windows as tabs.
                    cmds.push(Cmd::ExtTab(url, active));
                }
            },
        }
    }

    // ------------------------------------------------------------------ commands

    pub fn run(&mut self, cmd: Cmd, frame: &eframe::Frame) {
        if std::env::var("GOAT_DEBUG").is_ok() {
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
            Cmd::NewNoGpuWindow => self.spawn_window(Some("nogpu"), None, None),
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
            Cmd::MoveTab(id, to) => {
                if let Some(from) = self.tab_index(id) {
                    let to = to.min(self.tabs.len() - 1);
                    if from != to {
                        let active_id = self.tabs.get(self.active).map(|t| t.id);
                        let t = self.tabs.remove(from);
                        self.tabs.insert(to, t);
                        if let Some(a) = active_id.and_then(|a| self.tab_index(a)) {
                            self.active = a;
                        }
                    }
                }
            }
            Cmd::TearOff(id) => self.tear_off(id),
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
            Cmd::PageNative(cmd) => {
                self.page_menu = None;
                #[cfg(windows)]
                winhooks::finish_context_menu(Some(cmd));
                let _ = cmd;
            }
            Cmd::CopyText(t) => self.ctx.copy_text(t),
            Cmd::ShowTorCircuit(at) => {
                if matches!(self.popup, Some((Popup::TorCircuit, _))) {
                    self.popup = None;
                } else {
                    // Goat's own pages (like the Tor start page) don't go through Tor: no circuit.
                    let host = self
                        .active_tab()
                        .filter(|t| t.url.starts_with("http") && !t.url.starts_with(INTERNAL_BASE) && !t.url.contains("goatr.localhost"))
                        .map(|t| store::host_of(&t.url))
                        .unwrap_or_default();
                    *self.tor_circuit.lock().unwrap() = TorCircuitView { host: host.clone(), loading: true, ..Default::default() };
                    let (view, tor, c) = (self.tor_circuit.clone(), self.sh.tor.clone(), self.ctx.clone());
                    std::thread::spawn(move || {
                        let r = if host.is_empty() { Err("This is one of Goat's own pages, which don't go through Tor. Open a website in this tab to see the relays it uses.".to_string()) } else { tor.circuit_for(&host) };
                        let mut v = view.lock().unwrap();
                        v.loading = false;
                        match r {
                            Ok(h) => v.hops = h,
                            Err(e) => v.error = e,
                        }
                        drop(v);
                        c.request_repaint();
                    });
                    self.popup = Some((Popup::TorCircuit, at));
                    self.popup_had_focus = false;
                    self.popup_born = now(&self.ctx);
                }
            }
            Cmd::CheckUpdate => {
                self.popup = None;
                self.toast("Checking for updates…");
                self.check_update(true);
            }
            Cmd::InstallUpdate => {
                if let Some(u) = self.update.clone() {
                    #[cfg(windows)]
                    instance::standby_quit(&store::launch().profile);
                    if updater::run(&u, true) {
                        self.update_started = true;
                        self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
                    } else {
                        self.toast("Couldn't start the update");
                    }
                } else if self.update_manual.is_some() {
                    let _ = self.open_tab(frame, updater::RELEASES_PAGE, TabKind::Normal);
                }
            }
            Cmd::NewTorCircuit => {
                self.popup = None;
                match self.sh.tor.new_circuits() {
                    Ok(()) => {
                        self.toast("New Tor circuits: reloading through different relays");
                        if let Some(t) = self.active_tab() {
                            let _ = t.webview.reload();
                        }
                    }
                    Err(e) => self.toast(format!("Couldn't get new circuits: {e}")),
                }
            }
            Cmd::ToggleDiscordAudio => {
                let on = !self.settings().discord_audio;
                self.mutate_settings(|s| s.discord_audio = on);
                self.apply_discord_audio();
                self.toast(if on {
                    "Page sound now plays from Goat, so streaming apps can pick it up (experimental)"
                } else {
                    "Audio for streaming is off"
                });
            }
            Cmd::SetVolume(v) => {
                if let Some(t) = self.tabs.get_mut(self.active) {
                    t.volume = v.clamp(0.0, 6.0);
                    let _ = t.webview.evaluate_script(&format!("window.__goatrVolume&&__goatrVolume({})", t.volume));
                }
            }
            Cmd::OpenPrivate(url) => self.spawn_window(Some("private"), None, Some(&url)),
            Cmd::ForgetSuggestion(url) => {
                {
                    let mut s = self.sh.store.lock().unwrap();
                    s.data.history.retain(|v| v.url != url);
                    s.save();
                }
                self.sugg.retain(|(u, _)| *u != url);
                self.sugg_sel = self.sugg_sel.filter(|&i| i < self.sugg.len());
                if self.sugg.is_empty() && self.addr.trim().is_empty() && matches!(self.popup, Some((Popup::Suggest, _))) {
                    self.popup = None;
                }
                self.addr_refocus = true;
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
            // The magnifier toggles: a press that just closed the bubble doesn't reopen it.
            Cmd::ShowZoom if now(&self.ctx) - self.ctx.data(|d| d.get_temp::<f64>(egui::Id::new("zoom-closed-at")).unwrap_or(-9.0)) < 0.5 => {}
            Cmd::ShowZoom => {
                let z = self.active_tab().map(|t| t.zoom);
                self.set_zoom(z, 0.0);
                self.zoom_bubble_until = now(&self.ctx) + 4.0;
            }
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
                    self.toast("This extension isn't ready yet. Restart Goat after installing.");
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
    /// Make WebView2's extensions match Settings (the ones you turned off stay off).
    fn apply_extension_states(&self) {
        #[cfg(windows)]
        {
            let off = self.settings().disabled_extensions;
            let wanted: Vec<(String, bool)> = extinstall::list()
                .into_iter()
                .filter_map(|e| {
                    let id = self.ext_folder_ids.get(&e.folder).or_else(|| self.ext_ids.get(&e.name)).or_else(|| self.ext_ids.values().find(|v| **v == e.folder))?;
                    Some((id.clone(), !off.contains(&e.folder)))
                })
                .collect();
            if let Some(t) = self.tabs.iter().find(|t| t.kind == TabKind::Normal).filter(|_| !wanted.is_empty()) {
                winhooks::set_extensions_enabled(&t.webview, wanted);
            }
        }
    }

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
            // Goat hasn't asked WebView2 for its extension list yet: ask, then open.
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
                    if l.starts_with("edge://") || l.starts_with("chrome://") {
                        if let Some(u) = pages::browser_page(&url) {
                            let _ = tx.send(Ev::NewTab(u, TabKind::Normal));
                        }
                    } else if l.starts_with("http") {
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
    /// A tab's first page has drawn: show its web view (if it's the active tab).
    fn show_painted(&mut self, id: u32) {
        if let Some(i) = self.tab_index(id) {
            if !self.tabs[i].painted {
                self.tabs[i].painted = true;
                if i == self.active {
                    let _ = self.tabs[i].webview.set_visible(true);
                }
            }
        }
    }

    fn refresh_profile_badge(&mut self) {
        self.profile_badge = if store::profiles().len() > 1 {
            let (glyph, color) = store::profile_logo(&store::launch().profile);
            let c = egui::Color32::from_hex(&color).unwrap_or(egui::Color32::from_rgb(0x5f, 0xd3, 0xb3));
            Some((glyph, c))
        } else {
            None
        };
    }

    /// Start or stop the Discord audio relay to match the setting.
    fn apply_discord_audio(&mut self) {
        #[cfg(windows)]
        {
            let want = self.settings().discord_audio && !self.tabs.is_empty();
            // The helper stopped by itself (crashed): give up and say so, rather than restart it.
            if let Some(r) = self.relay.as_mut() {
                if !r.alive() {
                    self.relay = None;
                    self.mutate_settings(|s| s.discord_audio = false);
                    self.toast("Audio for streaming stopped working and was turned off");
                    return;
                }
            }
            if want && self.relay.is_none() {
                if let Some(pid) = self.tabs.first().and_then(|t| winhooks::browser_pid(&t.webview)) {
                    self.relay = audiorelay::Relay::start(pid);
                }
            } else if !want {
                if let Some(r) = self.relay.take() {
                    r.stop();
                }
            }
        }
    }

    /// Look for a newer Goat on a background thread (answer: Notice::UpdateChecked).
    fn check_update(&mut self, asked: bool) {
        if self.update_checking {
            return;
        }
        self.update_checking = true;
        self.last_update_check = now(&self.ctx);
        let sh = self.sh.clone();
        std::thread::spawn(move || {
            let r = updater::check();
            (sh.notify)(shared::Notice::UpdateChecked(r, asked));
        });
    }

    /// The window whose tabs are saved and reopened: the profile's first normal window (not
    /// private, Tor, no-GPU, the standby window or a window a tab was dragged into).
    fn session_window(&self) -> bool {
        self.window_kind == TabKind::Normal
            && !store::no_gpu_window()
            && !self.standby
            && store::launch().window.is_empty()
            && !std::env::args().any(|a| a == "--new-window")
    }

    /// Save the open tabs when they've changed (and when Goat closes).
    fn save_session(&mut self) {
        if !self.session_window() || store::READ_ONLY.load(Ordering::Relaxed) {
            return;
        }
        let on = self.settings().restore_tabs;
        let tabs: Vec<store::SavedTab> = if on {
            self.tabs
                .iter()
                .filter(|t| t.kind == TabKind::Normal && !t.url.is_empty() && !t.url.starts_with("about:"))
                .map(|t| store::SavedTab { url: t.pending.clone().unwrap_or_else(|| t.page_url.lock().unwrap().clone()), title: t.title.clone() })
                .filter(|t| !t.url.is_empty() && !t.url.starts_with("about:"))
                .collect()
        } else {
            Vec::new()
        };
        let mut s = self.sh.store.lock().unwrap();
        if s.data.session != tabs || s.data.session_active != self.active {
            s.data.session = tabs;
            s.data.session_active = self.active;
            s.save();
        }
    }

    /// No-GPU window: send this window's cookies to the normal window (logins made here carry
    /// over). `done` is set when it's finished.
    fn share_cookies_back(&mut self, done: Option<Arc<std::sync::atomic::AtomicBool>>) {
        self.last_cookie_share = now(&self.ctx);
        #[cfg(windows)]
        if let Some(t) = self.tabs.iter().find(|t| t.kind == TabKind::Normal) {
            let ctx = self.ctx.clone();
            winhooks::cookie_op(&t.webview, shared::CookieOp::List, move |r| {
                let list: Vec<shared::CookieInfo> = r.ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
                std::thread::spawn(move || {
                    if !list.is_empty() {
                        instance::cookies_put(&store::launch().profile, &list);
                    }
                    if let Some(d) = done {
                        d.store(true, Ordering::SeqCst);
                    }
                    ctx.request_repaint();
                });
            });
            return;
        }
        if let Some(d) = done {
            d.store(true, Ordering::SeqCst);
        }
    }

    fn housekeeping(&mut self, ctx: &egui::Context) {
        let t = now(ctx);
        // Old history goes even if Goat stays open for days.
        if t - self.last_history_prune > 3600.0 {
            self.last_history_prune = t;
            let mut s = self.sh.store.lock().unwrap();
            if s.prune_history() {
                s.save();
            }
        }
        // Standby window: keep one ready while a normal window of this profile is open.
        #[cfg(windows)]
        if t - self.last_standby_check > 2.0 && self.window_kind == TabKind::Normal && !store::no_gpu_window() && t > 3.0 {
            self.last_standby_check = t;
            let profile = store::launch().profile.clone();
            let want = self.settings().tear_standby;
            if !self.standby && want && !instance::standby_alive(&profile) {
                if let Ok(exe) = std::env::current_exe() {
                    let mut cmd = std::process::Command::new(exe);
                    cmd.args(["--standby", "--new-window"]);
                    cmd.arg(format!("--standby-for={}", std::process::id()));
                    if !profile.is_empty() {
                        cmd.args(["--profile", &profile]);
                    }
                    let _ = cmd.spawn();
                }
            }
        }
        if t - self.last_housekeeping < 5.0 {
            return;
        }
        self.save_session();
        if store::no_gpu_window() && t - self.last_cookie_share > 30.0 {
            self.share_cookies_back(None);
        }
        // Automatic updates: 30 s after starting, then every 6 hours (normal windows only).
        if self.window_kind == TabKind::Normal && !self.standby && self.settings().auto_update && self.update.is_none() {
            let due = if self.last_update_check == f64::NEG_INFINITY { t > 30.0 } else { t - self.last_update_check > 6.0 * 3600.0 };
            if due {
                self.check_update(false);
            }
        }
        self.apply_discord_audio();
        let first = self.last_housekeeping == 0.0;
        self.last_housekeeping = t;
        #[cfg(windows)]
        {
            let hwnd = panels::MAIN_HWND.load(Ordering::Relaxed);
            if hwnd != 0 {
                let hz = winhooks::monitor_refresh(hwnd).unwrap_or(0);
                if hz != self.refresh_hz {
                    crashlog::debug(format!("screen refresh rate: {hz} Hz"));
                    self.refresh_hz = hz;
                }
            }
        }
        self.refresh_profile_badge();
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
        "zoom_in" => Cmd::ZoomIn,
        "zoom_out" => Cmd::ZoomOut,
        "zoom_reset" => Cmd::ZoomReset,
        "install_extension" => Cmd::InstallExtension,
        _ => return None,
    })
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.clear
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // One wait for the next screen refresh per frame (see `vsync` above).
        #[cfg(windows)]
        if unsafe { windows::Win32::Graphics::Dwm::DwmFlush() }.is_err() {
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
        #[cfg(target_os = "linux")]
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        let frame_t0 = std::time::Instant::now();
        // Never leave a tab hidden for long if its page doesn't report drawing.
        if let Some(t) = self.tabs.get(self.active) {
            if !t.painted && now(ui.ctx()) - t.born > 1.2 {
                let id = t.id;
                self.show_painted(id);
            } else if !t.painted {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
            }
        }
        #[cfg(windows)]
        if panels::MAIN_HWND.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(h) = frame.window_handle() {
                if let RawWindowHandle::Win32(w) = h.as_raw() {
                    panels::MAIN_HWND.store(w.hwnd.get(), std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        crashlog::phase("frame: webview events");
        crashlog::set_hidden(ui.ctx().input(|i| i.viewport().minimized.unwrap_or(false)));
        if self.choosing_profile {
            if let Some(choice) = self.draw_profile_picker(ui) {
                self.choosing_profile = false;
                if self.profile_always {
                    store::set_startup_profile(Some(&choice));
                }
                let url = self.start_url.take().unwrap_or_else(|| self.home_url());
                if choice.is_empty() {
                    if let Err(e) = self.open_tab(frame, &url, self.window_kind) {
                        self.toast(format!("Couldn't open tab: {e}"));
                    }
                } else {
                    let start = store::launch().url.clone();
                    self.spawn_window(None, Some(&choice), start.as_deref());
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            return;
        }
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

        // The page menu closed: let the page carry on (nothing picked from the engine's commands).
        if self.page_menu.is_some() && !matches!(self.popup, Some((Popup::PageMenu, _))) {
            self.page_menu = None;
            #[cfg(windows)]
            winhooks::finish_context_menu(None);
        }
        // A press anywhere in Goat's own UI (address bar included) closes the zoom bubble.
        if matches!(self.popup, Some((Popup::Zoom, _))) && ui.ctx().input(|i| i.pointer.any_pressed()) {
            self.popup = None;
            let t = now(ui.ctx());
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("zoom-closed-at"), t));
        }
        // A press anywhere in Goat's own UI closes an open extension pop-up.
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
        // Opened for a tab dragged out of another window: tell it we're on screen.
        self.shown_frames = self.shown_frames.saturating_add(1);
        // The standby window must stay hidden until it takes a tab.
        #[cfg(windows)]
        if self.standby && !STANDBY_ADOPTED.load(Ordering::SeqCst) {
            winhooks::hide_window_by_title(&format!("Goat standby {}", std::process::id()));
        }
        #[cfg(windows)]
        if self.shown_frames >= 3 {
            if let Some(pid) = self.ready_pid.take() {
                std::thread::spawn(move || instance::send_ready(pid));
            }
        }
        crashlog::phase("frame: pop-up windows");
        self.draw_popups(ui.ctx(), &mut cmds);
        self.draw_tear_preview(ui.ctx());

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
        // No-GPU window closing: hand its cookies to the normal window first (a moment at most).
        if store::no_gpu_window() {
            if ui.ctx().input(|i| i.viewport().close_requested()) && self.closing_share.is_none() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
                let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
                self.closing_share = Some((now(ui.ctx()), flag.clone()));
                self.share_cookies_back(Some(flag));
            }
            if let Some((t0, flag)) = &self.closing_share {
                if (flag.load(Ordering::SeqCst) || now(ui.ctx()) - t0 > 3.0) && !ui.ctx().input(|i| i.viewport().close_requested()) {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
                }
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
        // Every 2 s of continuous drawing (an animation), log how fast frames came.
        {
            let ms = ft.as_secs_f32() * 1000.0;
            let (start, n, sum, max) = &mut self.frame_stats;
            *n += 1;
            *sum += ms;
            *max = max.max(ms);
            let el = start.elapsed().as_secs_f32();
            if el >= 2.0 {
                if *n as f32 / el > 20.0 {
                    crashlog::debug(format!("frames: {:.0} fps (screen {} Hz), Goat's work per frame avg {:.1} ms, max {:.1} ms", *n as f32 / el, self.refresh_hz, *sum / *n as f32, *max));
                }
                self.frame_stats = (std::time::Instant::now(), 0, 0.0, 0.0);
            }
        }
        crashlog::phase("between frames (egui painting / window events)");
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // The hidden standby window goes with us (another open window starts a new one).
        #[cfg(windows)]
        if !self.standby && self.window_kind == TabKind::Normal {
            instance::standby_quit(&store::launch().profile);
        }
        // Never linger after closing: if shutting down the web engine hangs, end anyway.
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(4));
            std::process::exit(0);
        });
        self.save_session();
        self.sh.store.lock().unwrap().save();
        self.sh.tor.stop();
        // A downloaded update installs once Goat has closed.
        if let Some(u) = &self.update {
            if self.settings().auto_update && !self.update_started {
                #[cfg(windows)]
                instance::standby_quit(&store::launch().profile);
                updater::run(u, false);
            }
        }
    }
}

/// The image in the clipboard as base64 (copied pixels as PNG, or a copied image file), with
/// its type and size; None when there's no image.
fn clipboard_image() -> Option<(String, &'static str, u32, u32)> {
    use base64::Engine;
    let mut cb = arboard::Clipboard::new().ok()?;
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    if let Ok(img) = cb.get_image() {
        let (w, h) = (img.width as u32, img.height as u32);
        if w == 0 || h == 0 || (w as u64) * (h as u64) > 50_000_000 {
            return None;
        }
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_compression(png::Compression::Fast);
            let mut wr = enc.write_header().ok()?;
            wr.write_image_data(&img.bytes).ok()?;
        }
        return Some((b64(&out), "image/png", w, h));
    }
    // A single image file copied in Explorer.
    let files = cb.get().file_list().ok()?;
    let [path] = files.as_slice() else { return None };
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => return None,
    };
    let bytes = std::fs::read(path).ok().filter(|b| b.len() < 40 << 20)?;
    Some((b64(&bytes), mime, 0, 0))
}

/// A PNG icon as a texture (None if it can't be read).
fn decode_icon(ctx: &egui::Context, host: &str, png: &[u8]) -> Option<egui::TextureHandle> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 || w > 1024 || h > 1024 {
        return None;
    }
    let ci = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
    Some(ctx.load_texture(format!("fav-{host}"), ci, egui::TextureOptions::LINEAR))
}

/// `--size=W,H` (a window torn off from another one opens at its size).
fn launch_size() -> Option<[f32; 2]> {
    let a = std::env::args().find_map(|a| a.strip_prefix("--size=").map(String::from))?;
    let (w, h) = a.split_once(',')?;
    Some([w.trim().parse::<f32>().ok()?.max(520.0), h.trim().parse::<f32>().ok()?.max(320.0)])
}

/// `--pos=X,Y`: where a torn-off window opens (screen points).
fn launch_pos() -> Option<[f32; 2]> {
    let a = std::env::args().find_map(|a| a.strip_prefix("--pos=").map(String::from))?;
    let (x, y) = a.split_once(',')?;
    Some([x.trim().parse().ok()?, y.trim().parse().ok()?])
}
