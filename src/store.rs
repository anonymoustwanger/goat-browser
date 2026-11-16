//! Browser data (settings, bookmarks, history, downloads), saved as JSON in `data\` next to Goat.exe.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const MAX_HISTORY: usize = 5000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub search_engine: String,
    /// "newtab" or a URL.
    pub home_page: String,
    pub show_bookmarks_bar: bool,
    pub save_history: bool,
    /// How long history is kept: 0 = don't save, -1 = forever, else days. None only in data
    /// from before this setting (filled from `save_history` on load).
    #[serde(default)]
    pub history_days: Option<i64>,
    pub theme: String,
    pub animations: bool,
    pub adblock: bool,
    /// Sites where the ad blocker is turned off.
    pub adblock_allow: Vec<String>,
    /// "ads" | "standard" | "strict"
    pub adblock_level: String,
    /// Extra rules: `||domain^` or `##selector`, one per line.
    pub adblock_custom: String,
    /// Requests blocked all-time.
    pub adblock_total: u64,
    pub gradients: bool,
    // ---- privacy (same switches as Goat)
    pub https_only: bool,
    /// quad9 | mullvad | mullvadAdblock | cloudflare | google | adguard | nextdns | custom | off
    pub dns_provider: String,
    pub dns_nextdns_id: String,
    pub dns_custom: String,
    /// off | standard | strict
    pub fingerprint: String,
    /// ask | allow | block
    pub webauthn: String,
    pub strip_tracking: bool,
    pub trim_referrers: bool,
    pub block_prefetch: bool,
    pub block_third_party_fonts: bool,
    pub block_third_party_cookies: bool,
    pub clear_cookies_on_exit: bool,
    /// Themes you made (Settings → Appearance → Create theme).
    pub custom_themes: Vec<CustomTheme>,
    // ---- performance
    /// 0 = all CPU cores, otherwise how many cores Goat may use.
    pub cpu_cores: u32,
    /// normal | above | high
    pub cpu_priority: String,
    /// Background tabs use less memory after this many minutes (0 = off).
    pub sleep_after_min: u32,
    /// Use the graphics card for pages (off helps with buggy drivers).
    pub hardware_accel: bool,
    /// Extensions (folder names) with a button in the toolbar.
    pub pinned_extensions: Vec<String>,
    /// Requests made by extensions themselves skip the ad blocker.
    pub adblock_skip_extensions: bool,
    /// "site|scheme" pairs allowed to open apps without asking (e.g. "roblox.com|roblox-player").
    pub app_links_allowed: Vec<String>,
    /// Zoom remembered per site (host → factor), like Chrome.
    pub site_zoom: std::collections::BTreeMap<String, f64>,
    /// Experimental: play page sound from Goat.exe so Discord can stream it.
    pub discord_audio: bool,
    /// Extensions (folder names) that also run in private windows.
    pub private_extensions: Vec<String>,
    /// Extensions (folder names) you turned off.
    pub disabled_extensions: Vec<String>,
    /// When a site asks for an image and one is in the clipboard, offer to paste it.
    pub paste_uploads: bool,
    /// Download new versions in the background and install them when Goat closes.
    pub auto_update: bool,
    /// Site icons on tabs and bookmarks (off: the first letter instead).
    pub site_icons: bool,
    /// Keep a hidden window ready so tabs dragged out of the strip open instantly.
    pub tear_standby: bool,
    /// Bring back the main window's tabs when Goat starts.
    pub restore_tabs: bool,
    /// A "Pop out" (picture in picture) button on videos.
    pub pip_button: bool,
    /// Ask where to save each download (Save As window).
    pub ask_download: bool,
    // ---- proxy
    /// system | none | manual
    pub proxy_mode: String,
    /// socks5 | http
    pub proxy_type: String,
    pub proxy_host: String,
    pub proxy_port: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct CustomTheme {
    pub id: String,
    pub name: String,
    pub bg: String,
    pub surface: String,
    pub text: String,
    pub accent: String,
    pub accent2: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            search_engine: "duckduckgo".into(),
            home_page: "newtab".into(),
            show_bookmarks_bar: true,
            save_history: true,
            history_days: Some(90),
            theme: "system".into(),
            animations: true,
            adblock: true,
            adblock_allow: Vec::new(),
            adblock_level: "standard".into(),
            adblock_custom: String::new(),
            adblock_total: 0,
            gradients: true,
            https_only: true,
            dns_provider: "quad9".into(),
            dns_nextdns_id: String::new(),
            dns_custom: String::new(),
            fingerprint: "standard".into(),
            webauthn: "ask".into(),
            strip_tracking: true,
            trim_referrers: true,
            block_prefetch: true,
            block_third_party_fonts: false,
            block_third_party_cookies: true,
            clear_cookies_on_exit: false,
            custom_themes: Vec::new(),
            cpu_cores: 0,
            cpu_priority: "normal".into(),
            sleep_after_min: 30,
            hardware_accel: true,
            pinned_extensions: Vec::new(),
            adblock_skip_extensions: true,
            app_links_allowed: Vec::new(),
            site_zoom: Default::default(),
            discord_audio: false,
            private_extensions: Vec::new(),
            disabled_extensions: Vec::new(),
            paste_uploads: true,
            auto_update: true,
            site_icons: true,
            tear_standby: true,
            restore_tabs: true,
            pip_button: true,
            ask_download: true,
            proxy_mode: "system".into(),
            proxy_type: "socks5".into(),
            proxy_host: String::new(),
            proxy_port: String::new(),
        }
    }
}

pub const ENGINES: &[(&str, &str, &str)] = &[
    ("duckduckgo", "DuckDuckGo", "https://duckduckgo.com/?q="),
    ("google", "Google", "https://www.google.com/search?q="),
    ("bing", "Bing", "https://www.bing.com/search?q="),
    ("brave", "Brave Search", "https://search.brave.com/search?q="),
    ("startpage", "Startpage", "https://www.startpage.com/do/search?q="),
];

impl Settings {
    pub fn search_prefix(&self) -> &'static str {
        ENGINES
            .iter()
            .find(|e| e.0 == self.search_engine)
            .unwrap_or(&ENGINES[0])
            .2
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: u64,
    pub title: String,
    pub url: String,
    pub added: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Visit {
    pub url: String,
    pub title: String,
    pub time: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    pub id: u64,
    pub url: String,
    pub path: String,
    /// "in_progress" | "done" | "failed"
    pub state: String,
    pub time: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Data {
    pub settings: Settings,
    pub bookmarks: Vec<Bookmark>,
    pub history: Vec<Visit>,
    pub downloads: Vec<Download>,
    pub next_id: u64,
    /// Open tabs of the main window (for "Reopen tabs on startup"), and which one was showing.
    pub session: Vec<SavedTab>,
    pub session_active: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedTab {
    pub url: String,
    pub title: String,
}

pub struct Store {
    pub data: Data,
    path: PathBuf,
}

/// The folder Goat.exe is in.
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Where Goat keeps everything (history, settings, browsing data, extensions, Tor):
/// %APPDATA%\Goat, like Goat Browser did. A file named `portable` next to Goat.exe
/// keeps it all beside the .exe instead.
pub fn app_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let exe = exe_dir();
        if exe.join("portable").exists() {
            return exe;
        }
        let Some(dir) = user_data_dir() else { return exe };
        let _ = std::fs::create_dir_all(&dir);
        migrate_from(&exe, &dir);
        dir
    })
    .clone()
}

fn user_data_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let (base, new, old) = (std::env::var_os("APPDATA").map(PathBuf::from), "Goat", "Goatr");
    #[cfg(not(windows))]
    let (base, new, old) = (
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))),
        "goat",
        "goatr",
    );
    let base = base?;
    // Data from when the browser was called Goatr.
    let (dir, prev) = (base.join(new), base.join(old));
    if !dir.exists() && prev.is_dir() {
        let _ = std::fs::rename(&prev, &dir);
    }
    Some(dir)
}

/// Older Goat builds kept their data next to Goat.exe: move it to the new place once.
fn migrate_from(old: &std::path::Path, new: &std::path::Path) {
    if old == new || new.join("data").join("goatr-data.json").exists() {
        return;
    }
    // Private/Tor windows' folders are thrown away anyway.
    for name in ["data", "profile", "extensions", "tor", "profiles"] {
        let from = old.join(name);
        let to = new.join(name);
        if !from.is_dir() || to.exists() {
            continue;
        }
        if std::fs::rename(&from, &to).is_err() {
            // Different drive: copy, then remove the old copy only if everything arrived.
            if copy_dir(&from, &to).is_ok() {
                let _ = std::fs::remove_dir_all(&from);
            } else {
                let _ = std::fs::remove_dir_all(&to);
            }
        }
    }
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let t = e.file_type()?;
        let dest = to.join(e.file_name());
        if t.is_dir() {
            copy_dir(&e.path(), &dest)?;
        } else if t.is_file() {
            // Chromium lock files can be skipped.
            if let Err(err) = std::fs::copy(e.path(), &dest) {
                let n = e.file_name().to_string_lossy().to_lowercase();
                if !(n == "lockfile" || n.ends_with(".lock") || n == "lock") {
                    return Err(err);
                }
            }
        }
    }
    Ok(())
}

/// How this Goat window was started: `--window private|tor`, `--profile NAME`, and a URL.
#[derive(Debug, Clone, Default)]
pub struct Launch {
    pub window: String,
    pub profile: String,
    pub url: Option<String>,
}

static LAUNCH: std::sync::OnceLock<Launch> = std::sync::OnceLock::new();

pub fn launch() -> &'static Launch {
    LAUNCH.get_or_init(|| {
        let mut l = Launch::default();
        let mut args = std::env::args().skip(1);
        while let Some(a) = args.next() {
            match a.as_str() {
                "--window" => l.window = args.next().unwrap_or_default(),
                "--profile" => l.profile = args.next().unwrap_or_default(),
                _ if l.url.is_none() && !a.starts_with("--") => l.url = Some(a),
                _ => {}
            }
        }
        l.profile = clean_profile(&l.profile);
        l
    })
}

/// Safe folder name for a profile ("" = the default profile).
pub fn clean_profile(name: &str) -> String {
    name.trim().chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_')).take(40).collect::<String>().trim().to_string()
}

/// Where this profile keeps its data, cookies and extensions.
pub fn profile_dir() -> PathBuf {
    let p = &launch().profile;
    if p.is_empty() { app_dir() } else { app_dir().join("profiles").join(p) }
}

/// A profile's logo: an emoji (empty = the name's first letter) on a coloured circle.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileLogo {
    #[serde(default)]
    pub emoji: String,
    /// "#rrggbb"; empty = picked from the name.
    #[serde(default)]
    pub color: String,
}

/// The colours offered for profile logos.
pub const LOGO_COLORS: [&str; 10] = ["#5fd3b3", "#6ea8ff", "#b57cf5", "#ff6fae", "#ff7a66", "#ffb347", "#ffd166", "#7bd88f", "#8b939c", "#e7e9ec"];

fn logos_file() -> PathBuf {
    app_dir().join("profiles.json")
}

/// Every profile's logo (shared by all profiles; "" = Default).
pub fn profile_logos() -> std::collections::HashMap<String, ProfileLogo> {
    std::fs::read_to_string(logos_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn set_profile_logo(name: &str, logo: ProfileLogo) {
    let mut all = profile_logos();
    all.insert(name.to_string(), logo);
    if let Ok(t) = serde_json::to_string_pretty(&all) {
        let _ = std::fs::write(logos_file(), t);
    }
}

/// The logo for `name`, with defaults filled in: (emoji or letter, colour).
pub fn profile_logo(name: &str) -> (String, String) {
    let l = profile_logos().remove(name).unwrap_or_default();
    let label = if name.is_empty() { "Default" } else { name };
    let glyph = label.chars().next().unwrap_or('?').to_uppercase().to_string();
    let color = if l.color.is_empty() {
        let h = label.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
        LOGO_COLORS[(h as usize) % 8].to_string()
    } else {
        l.color
    };
    (glyph, color)
}

/// Which profile opens when Goat starts (with more than one profile): None = ask,
/// Some("") = Default, Some(name) = that profile. Kept in `startup-profile.txt`.
pub fn startup_profile() -> Option<String> {
    let t = std::fs::read_to_string(app_dir().join("startup-profile.txt")).ok()?;
    let t = t.trim();
    match t {
        "" => None,
        "<default>" => Some(String::new()),
        name => Some(clean_profile(name)).filter(|n| profiles().contains(n)),
    }
}

pub fn set_startup_profile(p: Option<&str>) {
    let v = match p {
        None => String::new(),
        Some("") => "<default>".to_string(),
        Some(n) => clean_profile(n),
    };
    let _ = std::fs::write(app_dir().join("startup-profile.txt"), v);
}

/// Profiles that exist: "" (default) first, then the named ones.
pub fn profiles() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(app_dir().join("profiles"))
        .map(|rd| rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    v.sort_by_key(|n| n.to_lowercase());
    v.insert(0, String::new());
    v
}

/// Private and Tor windows never write history, settings or bookmarks back.
/// A window started with `--window nogpu`: hardware acceleration off. Windows' web engine can't
/// mix that with the normal windows' engine, so it has its own browsing data ("profile-nogpu").
pub fn no_gpu_window() -> bool {
    launch().window == "nogpu"
}

/// Where a site's icon is cached (saved from the tab the first time you visit it).
pub fn favicon_path(host: &str) -> PathBuf {
    let safe: String = host.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-').collect();
    profile_dir().join("data").join("favicons").join(format!("{safe}.png"))
}

/// The web engine's data folder for normal tabs in this window.
pub fn engine_dir() -> PathBuf {
    profile_dir().join(if no_gpu_window() { "profile-nogpu" } else { "profile" })
}

/// This private window's own, throw-away browsing data: a fresh folder per private window
/// process, so nothing carries over between private sessions (and extensions start clean).
pub fn private_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let root = profile_dir().join("private-sessions");
        // Leftovers from private windows that have closed (and the old shared folder).
        let _ = std::fs::remove_dir_all(profile_dir().join("profile-private"));
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                let pid = e.file_name().to_string_lossy().parse::<u32>().unwrap_or(0);
                if pid != std::process::id() && !process_alive(pid) {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        root.join(std::process::id().to_string())
    })
    .clone()
}

pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut code = 0u32;
        let alive = GetExitCodeProcess(h, &mut code).is_ok() && code == 259; // STILL_ACTIVE
        let _ = windows::Win32::Foundation::CloseHandle(h);
        alive
    }
    #[cfg(not(windows))]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
}

/// Block until the process ends (returns at once if it's already gone).
pub fn wait_for_exit(pid: u32) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE};
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            WaitForSingleObject(h, INFINITE);
            let _ = windows::Win32::Foundation::CloseHandle(h);
        }
    }
    #[cfg(not(windows))]
    while process_alive(pid) {
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

pub static READ_ONLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn now() -> i64 {
    chrono::Local::now().timestamp()
}

impl Store {
    pub fn load() -> Self {
        let dir = profile_dir().join("data");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("goatr-data.json");
        // Data from when the browser was called Ferrite.
        let old = dir.join("ferrite-data.json");
        if !path.exists() && old.exists() {
            let _ = std::fs::rename(&old, &path);
        }
        let data: Data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        crate::themes::set_custom(&data.settings.custom_themes);
        let mut st = Self { data, path };
        // Older data: history on means keep it forever, off means don't save.
        if st.data.settings.history_days.is_none() {
            st.data.settings.history_days = Some(if st.data.settings.save_history { -1 } else { 0 });
        }
        if st.prune_history() {
            st.save();
        }
        st
    }

    /// Days of history to keep: 0 = none, -1 = forever.
    pub fn history_days(&self) -> i64 {
        self.data.settings.history_days.unwrap_or(if self.data.settings.save_history { -1 } else { 0 })
    }

    /// Drop visits older than the history setting allows. True if anything was removed.
    pub fn prune_history(&mut self) -> bool {
        let days = self.history_days();
        self.data.settings.save_history = days != 0;
        let before = self.data.history.len();
        match days {
            -1 => {}
            0 => self.data.history.clear(),
            d => {
                let cutoff = now() - d * 86_400;
                if self.data.history.first().is_some_and(|v| v.time < cutoff) {
                    self.data.history.retain(|v| v.time >= cutoff);
                }
            }
        }
        self.data.history.len() != before
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self { data: Data::default(), path: std::env::temp_dir().join("goatr-test-data.json") }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn save(&self) {
        crate::themes::set_custom(&self.data.settings.custom_themes);
        if READ_ONLY.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        if let Ok(s) = serde_json::to_string(&self.data) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, s).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }

    fn id(&mut self) -> u64 {
        self.data.next_id += 1;
        self.data.next_id
    }

    pub fn is_bookmarked(&self, url: &str) -> bool {
        self.data.bookmarks.iter().any(|b| b.url == url)
    }

    /// Adds the bookmark, or removes it if it already exists. Returns true if now bookmarked.
    pub fn toggle_bookmark(&mut self, title: &str, url: &str) -> bool {
        if let Some(i) = self.data.bookmarks.iter().position(|b| b.url == url) {
            self.data.bookmarks.remove(i);
            self.save();
            return false;
        }
        let id = self.id();
        let title = if title.trim().is_empty() { url.to_string() } else { title.trim().to_string() };
        self.data.bookmarks.push(Bookmark { id, title, url: url.into(), added: now() });
        self.save();
        true
    }

    pub fn delete_bookmark(&mut self, id: u64) {
        self.data.bookmarks.retain(|b| b.id != id);
        self.save();
    }

    pub fn edit_bookmark(&mut self, id: u64, title: &str, url: &str) {
        if let Some(b) = self.data.bookmarks.iter_mut().find(|b| b.id == id) {
            if !title.trim().is_empty() {
                b.title = title.trim().into();
            }
            if !url.trim().is_empty() {
                b.url = url.trim().into();
            }
        }
        self.save();
    }

    pub fn record_visit(&mut self, url: &str, title: &str) {
        if self.history_days() == 0 {
            return;
        }
        self.prune_history();
        // Collapse reloads / redirects of the same URL.
        if let Some(last) = self.data.history.last_mut() {
            if last.url == url {
                last.time = now();
                if !title.is_empty() {
                    last.title = title.into();
                }
                self.save();
                return;
            }
        }
        self.data.history.push(Visit { url: url.into(), title: title.into(), time: now() });
        if self.data.history.len() > MAX_HISTORY {
            let extra = self.data.history.len() - MAX_HISTORY;
            self.data.history.drain(..extra);
        }
        self.save();
    }

    pub fn update_title(&mut self, url: &str, title: &str) {
        if let Some(v) = self.data.history.iter_mut().rev().take(20).find(|v| v.url == url) {
            if v.title != title {
                v.title = title.into();
                self.save();
            }
        }
    }

    pub fn delete_visit(&mut self, url: &str, time: i64) {
        self.data.history.retain(|v| !(v.url == url && v.time == time));
        self.save();
    }

    pub fn clear_history(&mut self) {
        self.data.history.clear();
        self.save();
    }

    pub fn download_started(&mut self, url: &str, path: &str) -> u64 {
        let id = self.id();
        self.data.downloads.push(Download {
            id,
            url: url.into(),
            path: path.into(),
            state: "in_progress".into(),
            time: now(),
        });
        if self.data.downloads.len() > 300 {
            self.data.downloads.remove(0);
        }
        self.save();
        id
    }

    pub fn download_finished(&mut self, url: &str, path: Option<&str>, ok: bool) {
        if let Some(d) = self
            .data
            .downloads
            .iter_mut()
            .rev()
            .find(|d| d.url == url && d.state == "in_progress")
        {
            d.state = if ok { "done" } else { "failed" }.into();
            if let Some(p) = path {
                d.path = p.into();
            }
        }
        self.save();
    }

    pub fn active_downloads(&self) -> usize {
        self.data.downloads.iter().filter(|d| d.state == "in_progress").count()
    }

    /// Address-bar suggestions: bookmarks first, then history, de-duplicated.
    pub fn suggest(&self, q: &str, limit: usize) -> Vec<(String, String)> {
        let q = q.trim().to_lowercase();
        if q.is_empty() {
            return vec![];
        }
        let mut out: Vec<(String, String)> = Vec::new();
        let matches = |title: &str, url: &str| title.to_lowercase().contains(&q) || url.to_lowercase().contains(&q);
        for b in &self.data.bookmarks {
            if matches(&b.title, &b.url) && !out.iter().any(|o| o.0 == b.url) {
                out.push((b.url.clone(), format!("★ {}", b.title)));
            }
        }
        for v in self.data.history.iter().rev() {
            if out.len() >= limit {
                break;
            }
            if matches(&v.title, &v.url) && !out.iter().any(|o| o.0 == v.url) {
                out.push((v.url.clone(), v.title.clone()));
            }
        }
        out.truncate(limit);
        out
    }

    /// Most-visited sites for the new tab page: (title, url).
    pub fn top_sites(&self, n: usize) -> Vec<(String, String)> {
        let mut counts: Vec<(String, String, usize)> = Vec::new();
        for v in self.data.history.iter().rev().take(2000) {
            let host = host_of(&v.url);
            if host.is_empty() {
                continue;
            }
            if let Some(e) = counts.iter_mut().find(|e| host_of(&e.1) == host) {
                e.2 += 1;
            } else {
                let origin = v.url.split('/').take(3).collect::<Vec<_>>().join("/") + "/";
                counts.push((host.trim_start_matches("www.").to_string(), origin, 1));
            }
        }
        counts.sort_by_key(|e| std::cmp::Reverse(e.2));
        counts.into_iter().take(n).map(|(t, u, _)| (t, u)).collect()
    }
}

pub fn host_of(u: &str) -> String {
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return String::new();
    }
    u.split("://").nth(1).unwrap_or("").split(['/', '?', '#']).next().unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_days() {
        let mut s = Store::in_memory();
        s.data.history.push(Visit { url: "https://old.com/".into(), title: "".into(), time: now() - 10 * 86_400 });
        s.data.history.push(Visit { url: "https://new.com/".into(), title: "".into(), time: now() - 3600 });
        s.data.settings.history_days = Some(-1);
        assert!(!s.prune_history());
        s.data.settings.history_days = Some(7);
        assert!(s.prune_history());
        assert_eq!(s.data.history.len(), 1);
        s.data.settings.history_days = Some(0);
        s.prune_history();
        assert!(s.data.history.is_empty() && !s.data.settings.save_history);
    }

    #[test]
    fn bookmarks_and_history() {
        let mut s = Store::in_memory();
        assert!(s.toggle_bookmark("Rust", "https://rust-lang.org/"));
        assert!(s.is_bookmarked("https://rust-lang.org/"));
        assert!(!s.toggle_bookmark("Rust", "https://rust-lang.org/"));
        s.record_visit("https://a.com/x", "A page");
        s.record_visit("https://a.com/x", "A page");
        s.record_visit("https://b.com/", "B");
        assert_eq!(s.data.history.len(), 2);
        assert_eq!(s.suggest("page", 5)[0].0, "https://a.com/x");
        assert_eq!(s.top_sites(5).len(), 2);
    }
}

#[cfg(test)]
mod theme_json_test {
    #[test]
    fn custom_theme_merge() {
        let mut cur = serde_json::to_value(super::Settings::default()).unwrap();
        let new: serde_json::Value = serde_json::from_str(r##"{"custom_themes":[{"id":"custom-abc","name":"T","bg":"#101214","surface":"#181b1f","text":"#e7e9ec","accent":"#ff9e6b","accent2":"#ff5e8a"}],"theme":"custom-abc"}"##).unwrap();
        for (k, v) in new.as_object().unwrap() { cur.as_object_mut().unwrap().insert(k.clone(), v.clone()); }
        let s: super::Settings = serde_json::from_value(cur).unwrap();
        assert_eq!(s.custom_themes.len(), 1);
    }
}
