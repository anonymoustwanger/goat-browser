//! State shared between the main window and the built-in pages (which are served on another thread).

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::adblock::AdBlock;
use crate::store::Store;
use crate::tor::Tor;

/// Things a built-in page (or a background job) needs the main window to do.
#[derive(Debug)]
pub enum Notice {
    StoreChanged,
    ClearBrowsingData,
    OpenTorTab,
    OpenPrivateTab,
    TorChanged,
    AdblockUpdated(usize),
    /// An extension finished downloading and waits for your OK (or failed).
    ExtensionInstalled(Result<crate::extinstall::Pending, String>),
    Restart,
    /// An extension asked for a tab (chrome.tabs.create).
    /// (address, switch to it, extension asked for a pop-up window)
    OpenTab(String, bool, bool),
    /// Open a window for a profile ("" = default).
    OpenProfile(String),
    /// Settings' cookie editor: run this on the engine's cookie store; the answer goes into
    /// `Shared::cookie_results` under the id.
    Cookies(u64, CookieOp),
    /// Turn an extension (folder name) on or off.
    ExtensionEnabled(String, bool),
    /// Settings → About → Check for updates.
    CheckUpdate,
    /// Another app opened a link while Goat was running: a tab here, and come to the front.
    External(String),
    /// The no-GPU window wants this window's cookies (answer: JSON list).
    CookiesGet(std::sync::mpsc::Sender<String>),
    /// Cookies from the other window to add here.
    CookiesPut(Vec<CookieInfo>),
    /// A tab dragged here from another Goat window.
    TabIn(String),
    /// The window opened for a dragged-out tab is showing (the preview can go).
    TearReady,
    /// The standby window's answer for a dragged-out tab (tab id, took it).
    TearDone(u32, bool),
    /// This hidden standby window takes a dragged-out tab.
    StandbyAdopt(String),
    /// Close the standby window (the setting was turned off).
    StandbyQuit,
    /// An update check finished (true = you asked for it).
    UpdateChecked(Result<crate::updater::Found, String>, bool),
}

/// One cookie, as the cookie editor shows it.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CookieInfo {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    /// Seconds since 1970; ignored for session cookies.
    pub expires: f64,
    pub session: bool,
    pub secure: bool,
    pub http_only: bool,
    /// "lax" | "strict" | "none"
    pub same_site: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CookieOp {
    List,
    /// Save a cookie; `original` is the one being edited (removed first if its name, site or path changed).
    Set { cookie: CookieInfo, original: Option<CookieInfo> },
    Delete { cookie: CookieInfo },
    DeleteSite { domain: String },
    DeleteAll,
}

pub struct Shared {
    pub store: Mutex<Store>,
    pub system_dark: AtomicBool,
    pub adblock: Arc<AdBlock>,
    pub tor: Arc<Tor>,
    pub notify: Box<dyn Fn(Notice) + Send + Sync>,
    /// Hosts you chose to open over plain http:// (HTTPS-only mode exceptions, this session).
    pub allow_http: Mutex<std::collections::HashSet<String>>,
    /// Request-time privacy switches, read by the network hook.
    pub net_privacy: std::sync::RwLock<NetPrivacy>,
    /// Sites extensions talk to (their own servers): never stripped of cookies or blocked.
    pub ext_sites: std::sync::RwLock<Vec<String>>,
    /// Finished cookie-editor requests (id → answer), collected by the settings page.
    pub cookie_results: Mutex<std::collections::HashMap<u64, serde_json::Value>>,
    pub next_cookie_req: std::sync::atomic::AtomicU64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NetPrivacy {
    pub trim_referrers: bool,
    pub block_fonts: bool,
    pub strip_hints: bool,
    pub block_prefetch: bool,
    pub skip_extensions: bool,
    /// "Block third-party cookies" (cookies aren't sent to other sites a page loads from).
    pub block_cookies: bool,
}

impl NetPrivacy {
    pub fn from(s: &crate::store::Settings) -> Self {
        NetPrivacy { trim_referrers: s.trim_referrers, block_fonts: s.block_third_party_fonts, strip_hints: s.fingerprint != "off", block_prefetch: s.block_prefetch, skip_extensions: s.adblock_skip_extensions, block_cookies: s.block_third_party_cookies }
    }
}

impl Shared {
    pub fn tor_notifier(self: &Arc<Self>) -> Arc<dyn Fn() + Send + Sync> {
        let me = Arc::downgrade(self);
        Arc::new(move || {
            if let Some(s) = me.upgrade() {
                (s.notify)(Notice::TorChanged);
            }
        })
    }
}

#[cfg(test)]
pub fn test_shared() -> Arc<Shared> {
    Arc::new(Shared {
        store: Mutex::new(Store::in_memory()),
        system_dark: AtomicBool::new(true),
        adblock: AdBlock::new(true, crate::adblock::Level::Standard, &[], ""),
        tor: Tor::new(),
        notify: Box::new(|_| {}),
        allow_http: Default::default(),
        net_privacy: Default::default(),
        ext_sites: Default::default(),
    })
}

/// What was right-clicked in a page, for Goat's own context menu.
#[derive(Debug, Clone, Default)]
pub struct PageMenu {
    pub link: String,
    pub src: String,
    pub selection: String,
    pub editable: bool,
    /// The engine's own commands: (name, label, command id, enabled).
    pub native: Vec<(String, String, i32, bool)>,
}

impl PageMenu {
    /// The engine's command with this name, if it offered one (and whether it's enabled).
    pub fn native(&self, name: &str) -> Option<(i32, bool)> {
        self.native.iter().find(|n| n.0 == name).map(|n| (n.2, n.3))
    }
}
