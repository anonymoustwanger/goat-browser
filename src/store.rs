//! Browser data (settings, bookmarks, history, downloads), saved as JSON in `data\` next to Goatr.exe.

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
    /// 0 = all CPU cores, otherwise how many cores Goatr may use.
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
}

pub struct Store {
    pub data: Data,
    path: PathBuf,
}

pub fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// How this Goatr window was started: `--window private|tor`, `--profile NAME`, and a URL.
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
        Self { data, path }
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
        if !self.data.settings.save_history {
            return;
        }
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
