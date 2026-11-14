//! Built-in ad blocker, powered by Brave's `adblock` engine (adblock-rust) — the same kind of
//! full filter-list engine Goat uses (Ghostery's): it understands the complete Adblock Plus /
//! uBlock Origin network-rule syntax (paths, wildcards, `$third-party`, `$script`, `$domain=`,
//! exceptions, `$important`…) plus cosmetic rules (site-specific and generic element hiding).
//!
//! Three levels, like Goat:
//! * **Ads** — EasyList + Peter Lowe's list.
//! * **Standard** — adds EasyPrivacy (trackers). Default.
//! * **Strict** — adds the EasyList Cookie List and Fanboy's Annoyance List.
//!
//! Lists are cached in `data\adblock\` and refreshed every 4 days. Custom rules from Settings are
//! added to the engine at every level.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

use adblock::lists::{FilterSet, ParseOptions};
use adblock::request::Request;
use adblock::Engine;

use crate::net;
use crate::store::app_dir;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ads = 0,
    Standard = 1,
    Strict = 2,
}

impl Level {
    pub fn parse(s: &str) -> Level {
        match s {
            "ads" => Level::Ads,
            "strict" => Level::Strict,
            _ => Level::Standard,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Level::Ads => "ads",
            Level::Standard => "standard",
            Level::Strict => "strict",
        }
    }
}

/// (file, download URLs in order of preference, level that turns it on)
const LISTS: &[(&str, &[&str], Level)] = &[
    ("easylist.txt", &["https://easylist.to/easylist/easylist.txt", "https://ublockorigin.github.io/uAssets/thirdparties/easylist.txt", "https://ublockorigin.pages.dev/thirdparties/easylist.txt"], Level::Ads),
    ("peterlowe.txt", &["https://pgl.yoyo.org/adservers/serverlist.php?hostformat=adblockplus&showintro=0&mimetype=plaintext"], Level::Ads),
    ("easyprivacy.txt", &["https://easylist.to/easylist/easyprivacy.txt", "https://ublockorigin.github.io/uAssets/thirdparties/easyprivacy.txt", "https://ublockorigin.pages.dev/thirdparties/easyprivacy.txt"], Level::Standard),
    ("cookies.txt", &["https://secure.fanboy.co.nz/fanboy-cookiemonster.txt", "https://ublockorigin.github.io/uAssets/thirdparties/easylist-cookies.txt", "https://ublockorigin.pages.dev/thirdparties/easylist-cookies.txt"], Level::Strict),
    ("annoyances.txt", &["https://secure.fanboy.co.nz/fanboy-annoyance.txt", "https://ublockorigin.github.io/uAssets/thirdparties/easylist-annoyances.txt", "https://ublockorigin.pages.dev/thirdparties/easylist-annoyances.txt"], Level::Strict),
];
const MAX_AGE: Duration = Duration::from_secs(4 * 24 * 3600);

/// Starter rules so blocking works before the first list download finishes.
const BUILTIN_ADS: &str = "||doubleclick.net^\n||googlesyndication.com^\n||googleadservices.com^\n||googletagservices.com^\n\
||adservice.google.com^\n||adnxs.com^\n||criteo.com^\n||criteo.net^\n||taboola.com^\n||outbrain.com^\n||amazon-adsystem.com^\n\
||pubmatic.com^\n||rubiconproject.com^\n||openx.net^\n||casalemedia.com^\n||adsrvr.org^\n||media.net^\n||teads.tv^\n\
||33across.com^\n||sharethrough.com^\n||indexww.com^\n||mgid.com^\n||revcontent.com^\n||propellerads.com^\n||popads.net^\n\
||exoclick.com^\n||adsterra.com^\n||trafficjunky.net^\n||triplelift.com^\n||3lift.com^\n||adroll.com^\n||smartadserver.com^\n\
##ins.adsbygoogle\n##[id^=\"google_ads_iframe\"]\n##[id^=\"div-gpt-ad\"]\n##.adsbygoogle\n##[data-ad-slot]\n";
const BUILTIN_TRACKERS: &str = "||google-analytics.com^\n||googletagmanager.com^\n||scorecardresearch.com^\n||quantserve.com^\n\
||hotjar.com^\n||chartbeat.com^\n||demdex.net^\n||krxd.net^\n||bluekai.com^\n||mixpanel.com^\n||segment.io^\n||fullstory.com^\n\
||clarity.ms^\n||nr-data.net^\n||amplitude.com^\n||omtrdc.net^\n||moatads.com^\n||doubleverify.com^\n||adsafeprotected.com^\n";
const BUILTIN_ANNOY: &str = "||cookielaw.org^$third-party\n||cookiebot.com^$third-party\n||usercentrics.eu^$third-party\n\
||didomi.io^$third-party\n||onesignal.com^$third-party\n##[id=\"onetrust-banner-sdk\"]\n###onetrust-consent-sdk\n\
###CybotCookiebotDialog\n##.cc-window\n###cookie-banner\n##.cookie-banner\n###cookie-notice\n##.cookie-notice\n##.qc-cmp2-container\n\
###didomi-host\n##.fc-consent-root\n###usercentrics-root\n";

pub struct AdBlock {
    engine: Mutex<Option<Engine>>,
    enabled: AtomicBool,
    level: AtomicU8,
    allow: RwLock<HashSet<String>>,
    custom: RwLock<String>,
    rules: AtomicUsize,
    updating: AtomicBool,
    building: AtomicBool,
    /// Bumped on every rebuild request so an older build can't overwrite a newer one.
    generation: AtomicUsize,
}

fn dir() -> PathBuf {
    app_dir().join("data").join("adblock")
}

fn norm_host(h: &str) -> String {
    h.trim().trim_start_matches("www.").to_lowercase()
}

pub fn host_of(url: &str) -> String {
    let rest = match url.split_once("://") {
        Some((_, r)) => r,
        None => return String::new(),
    };
    let hostport = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = hostport.rsplit('@').next().unwrap_or(hostport);
    let host = if host.starts_with('[') { host } else { host.split(':').next().unwrap_or(host) };
    host.to_lowercase()
}

/// Selectors that are always hidden while blocking is on (a safety net before lists load).
pub const BASE_CSS: &str = "ins.adsbygoogle,[id^='google_ads_iframe'],[id^='div-gpt-ad'],iframe[src*='doubleclick.net'],\
iframe[src*='googlesyndication.com'],.adsbygoogle{display:none!important}";

fn count_rules(text: &str) -> usize {
    text.lines().filter(|l| {
        let l = l.trim();
        !l.is_empty() && !l.starts_with('!') && !l.starts_with('[')
    }).count()
}

/// Rules for a level: cached lists (or the built-in starters) + custom rules.
fn assemble(level: Level, custom: &str) -> (FilterSet, usize) {
    let mut set = FilterSet::new(false);
    let mut n = 0;
    for (lvl, builtin) in [(Level::Ads, BUILTIN_ADS), (Level::Standard, BUILTIN_TRACKERS), (Level::Strict, BUILTIN_ANNOY)] {
        if lvl > level {
            continue;
        }
        let mut have_list = false;
        for (file, _, l) in LISTS {
            if *l == lvl {
                if let Ok(t) = std::fs::read_to_string(dir().join(file)) {
                    n += count_rules(&t);
                    set.add_filter_list(t, ParseOptions::default());
                    have_list = true;
                }
            }
        }
        if !have_list {
            n += count_rules(builtin);
            set.add_filter_list(builtin.to_string(), ParseOptions::default());
        }
    }
    if !custom.trim().is_empty() {
        n += count_rules(custom);
        set.add_filter_list(custom.to_string(), ParseOptions::default());
    }
    (set, n)
}

impl AdBlock {
    pub fn new(enabled: bool, level: Level, allow: &[String], custom: &str) -> Arc<Self> {
        let ab = Arc::new(Self {
            engine: Mutex::new(None),
            enabled: AtomicBool::new(enabled),
            level: AtomicU8::new(level as u8),
            allow: RwLock::new(allow.iter().map(|h| norm_host(h)).collect()),
            custom: RwLock::new(custom.to_string()),
            rules: AtomicUsize::new(0),
            updating: AtomicBool::new(false),
            building: AtomicBool::new(false),
            generation: AtomicUsize::new(0),
        });
        ab.rebuild(None::<fn()>);
        ab
    }

    /// Build a fresh engine on a background thread and swap it in.
    pub fn rebuild(self: &Arc<Self>, done: Option<impl FnOnce() + Send + 'static>) {
        let gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.building.store(true, Ordering::SeqCst);
        let me = self.clone();
        let _ = std::thread::Builder::new().name("adblock-build".into()).stack_size(32 << 20).spawn(move || {
            let (set, n) = assemble(me.level(), &me.custom.read().unwrap());
            let engine = Engine::new_with_filter_set(set);
            if me.generation.load(Ordering::SeqCst) == gen {
                *me.engine.lock().unwrap() = Some(engine);
                me.rules.store(n, Ordering::SeqCst);
                me.building.store(false, Ordering::SeqCst);
            }
            if let Some(f) = done {
                f();
            }
        });
    }

    /// Build synchronously (tests).
    #[cfg(test)]
    fn rebuild_now(&self) {
        let (set, n) = assemble(self.level(), &self.custom.read().unwrap());
        *self.engine.lock().unwrap() = Some(Engine::new_with_filter_set(set));
        self.rules.store(n, Ordering::SeqCst);
        self.building.store(false, Ordering::SeqCst);
    }

    pub fn set_custom(self: &Arc<Self>, text: &str) {
        if *self.custom.read().unwrap() != text {
            *self.custom.write().unwrap() = text.to_string();
            self.rebuild(None::<fn()>);
        }
    }

    pub fn custom_count(&self) -> usize {
        count_rules(&self.custom.read().unwrap())
    }

    pub fn domain_count(&self) -> usize {
        self.rules.load(Ordering::Relaxed)
    }

    pub fn ready(&self) -> bool {
        self.engine.lock().map(|e| e.is_some()).unwrap_or(false)
    }

    pub fn level(&self) -> Level {
        match self.level.load(Ordering::Relaxed) {
            0 => Level::Ads,
            2 => Level::Strict,
            _ => Level::Standard,
        }
    }

    pub fn set_level(self: &Arc<Self>, l: Level) {
        if self.level() != l {
            self.level.store(l as u8, Ordering::Relaxed);
            self.rebuild(None::<fn()>);
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    pub fn set_allowlist(&self, hosts: &[String]) {
        *self.allow.write().unwrap() = hosts.iter().map(|h| norm_host(h)).collect();
    }

    pub fn is_allowed(&self, page_host: &str) -> bool {
        self.allow.read().unwrap().contains(&norm_host(page_host))
    }

    /// Is blocking active on a page with this host?
    pub fn active_on(&self, page_host: &str) -> bool {
        self.enabled() && !self.is_allowed(page_host)
    }

    /// Network check. `page_url` is the tab's current page; `rtype` is the resource type
    /// ("script", "image", "sub_frame", "xmlhttprequest", …).
    pub fn should_block(&self, url: &str, page_url: &str, rtype: &str) -> bool {
        if !url.starts_with("http") {
            return false;
        }
        let page_host = host_of(page_url);
        if !self.active_on(&page_host) {
            return false;
        }
        // Never block the page itself.
        if rtype == "document" {
            return false;
        }
        let Ok(req) = Request::new(url, page_url, rtype, "GET") else { return false };
        // Never wait on the engine (it may be being swapped): let the request through instead.
        match self.engine.try_lock() {
            Ok(g) => g.as_ref().is_some_and(|e| e.check_network_request(&req).should_block()),
            Err(_) => false,
        }
    }

    /// CSS for a page: site-specific hiding rules from the lists, plus the base rules.
    /// Returns (css, exceptions for generic rules, generic rules allowed?).
    pub fn page_css(&self, url: &str) -> (String, HashSet<String>, bool) {
        let mut css = String::from(BASE_CSS);
        let guard = self.engine.lock();
        let Some(e) = guard.as_ref().ok().and_then(|g| g.as_ref()) else { return (css, HashSet::new(), false) };
        let r = e.url_cosmetic_resources(url);
        // `:has()` rules make Chromium re-check the whole page on every change; skip them.
        for s in r.hide_selectors.iter().filter(|s| !s.contains(":has(")).take(3000) {
            css.push('\n');
            css.push_str(s);
            css.push_str("{display:none!important}");
        }
        (css, r.exceptions, !r.generichide)
    }

    /// Generic hiding rules matching classes/ids seen on the page.
    pub fn generic_css(&self, classes: &[String], ids: &[String], exceptions: &HashSet<String>) -> String {
        let guard = self.engine.lock();
        let Some(e) = guard.as_ref().ok().and_then(|g| g.as_ref()) else { return String::new() };
        e.hidden_class_id_selectors(classes, ids, exceptions)
            .into_iter()
            .filter(|s| !s.contains(":has("))
            .map(|s| format!("{s}{{display:none!important}}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn last_updated(&self) -> Option<SystemTime> {
        LISTS
            .iter()
            .filter_map(|(f, _, _)| std::fs::metadata(dir().join(f)).and_then(|m| m.modified()).ok())
            .min()
    }

    pub fn is_updating(&self) -> bool {
        self.updating.load(Ordering::Relaxed) || self.building.load(Ordering::Relaxed)
    }

    /// Refresh lists in the background (only stale ones unless `force`), then rebuild.
    pub fn refresh(self: &Arc<Self>, force: bool, done: impl FnOnce(usize) + Send + 'static) {
        if self.updating.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            let _ = std::fs::create_dir_all(dir());
            let mut changed = false;
            let mut log = String::new();
            for (file, urls, _) in LISTS {
                let path = dir().join(file);
                let fresh = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age < MAX_AGE);
                if fresh && !force {
                    continue;
                }
                for url in urls.iter() {
                    match net::get_text(url) {
                        Ok(text) if text.len() > 1000 && (text.contains("||") || text.contains("##")) => {
                            if std::fs::write(&path, text).is_ok() {
                                changed = true;
                                log.push_str(&format!("ok   {file} <- {url}\n"));
                                break;
                            }
                        }
                        Ok(text) => log.push_str(&format!("bad  {file} <- {url} ({} bytes)\n", text.len())),
                        Err(e) => log.push_str(&format!("fail {file} <- {url}: {e}\n")),
                    }
                }
            }
            let _ = std::fs::write(dir().join("update.log"), format!("{}\n{log}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S")));
            me.updating.store(false, Ordering::SeqCst);
            if changed {
                let me2 = me.clone();
                me.rebuild(Some(move || done(me2.domain_count())));
            } else {
                done(me.domain_count());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn engine_levels() {
        let ab = AdBlock::new(true, Level::Standard, &["allowed.com".into()], "||custom-ads.net^\n##.my-ad\nexample.org##.site-ad");
        while ab.building.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let page = "https://news.com/article";
        assert!(ab.should_block("https://x.custom-ads.net/a.js", page, "script"));
        assert!(ab.should_block("https://securepubads.g.doubleclick.net/tag/js/gpt.js", page, "script"));
        assert!(ab.should_block("https://www.google-analytics.com/analytics.js", page, "script"));
        assert!(!ab.should_block("https://news.com/app.js", page, "script"));
        assert!(!ab.should_block("https://doubleclick.net/x", "https://allowed.com/", "script"));
        assert!(!ab.should_block("https://doubleclick.net.evil.org/x", page, "script"));
        let (css, _, generic) = ab.page_css("https://example.org/");
        assert!(css.contains(".site-ad{display:none!important}") && generic);
        assert!(ab.generic_css(&["my-ad".into()], &[], &HashSet::new()).contains(".my-ad"));
        ab.level.store(Level::Ads as u8, Ordering::Relaxed);
        ab.rebuild_now();
        assert!(!ab.should_block("https://www.google-analytics.com/analytics.js", page, "script"));
        assert!(!ab.should_block("https://cdn.cookielaw.org/x.js", page, "script"));
        ab.level.store(Level::Strict as u8, Ordering::Relaxed);
        ab.rebuild_now();
        assert!(ab.should_block("https://cdn.cookielaw.org/x.js", page, "script"));
        assert!(ab.page_css("https://site.com/").0.contains("onetrust"));
        assert_eq!(host_of("https://user@Sub.Example.com:8080/x?y"), "sub.example.com");
    }
}


#[cfg(test)]
mod bench {
    use super::*;
    #[test]
    #[ignore]
    fn timing() {
        let ab = AdBlock::new(true, Level::Standard, &[], "");
        let t = std::time::Instant::now();
        ab.rebuild_now();
        eprintln!("build {:?} rules {}", t.elapsed(), ab.domain_count());
        for url in ["https://www.google.com/search?q=test", "https://chromewebstore.google.com/", "https://www.youtube.com/"] {
            let t = std::time::Instant::now();
            let (css, exc, g) = ab.page_css(url);
            eprintln!("page_css {url} {:?} len {} exc {} generic {g}", t.elapsed(), css.len(), exc.len());
            let classes: Vec<String> = (0..3000).map(|i| format!("c{i}x")).chain(["ad", "ads", "banner", "sponsored"].iter().map(|s| s.to_string())).collect();
            let ids: Vec<String> = (0..500).map(|i| format!("i{i}")).collect();
            let t = std::time::Instant::now();
            let g = ab.generic_css(&classes, &ids, &exc);
            eprintln!("generic {:?} len {}", t.elapsed(), g.len());
            let t = std::time::Instant::now();
            for i in 0..2000 {
                ab.should_block(&format!("https://www.gstatic.com/x/{i}/a.js?foo=bar"), url, "script");
            }
            eprintln!("2000 checks {:?}", t.elapsed());
        }
    }
}
