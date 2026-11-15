//! Installing extensions from the Chrome Web Store / Edge Add-ons.
//!
//! WebView2 can only load *unpacked* extensions, so Goat downloads the store's .crx package,
//! strips the CRX header, and unzips it into `extensions\<id>`. A restart loads it.

use std::io::Read;
use std::path::PathBuf;

use crate::net;

pub fn ext_dir() -> PathBuf {
    crate::store::profile_dir().join("extensions")
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    Chrome,
    Edge,
}

fn is_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

/// Recognise a store page URL (or a bare 32-letter extension id).
pub fn parse_store_url(url: &str) -> Option<(Source, String)> {
    let u = url.trim();
    if is_id(u) {
        return Some((Source::Chrome, u.to_string()));
    }
    let lower = u.to_lowercase();
    let src = if lower.contains("chromewebstore.google.com/detail/") || lower.contains("chrome.google.com/webstore/detail/") {
        Source::Chrome
    } else if lower.contains("microsoftedge.microsoft.com/addons/detail/") {
        Source::Edge
    } else {
        return None;
    };
    let path = u.split(['?', '#']).next().unwrap_or(u);
    path.split('/').find(|seg| is_id(seg)).map(|id| (src, id.to_string()))
}

fn crx_url(src: Source, id: &str) -> String {
    match src {
        Source::Chrome => format!(
            "https://clients2.google.com/service/update2/crx?response=redirect&prodversion=131.0.0.0&acceptformat=crx2,crx3&x=id%3D{id}%26installsource%3Dondemand%26uc"
        ),
        Source::Edge => format!(
            "https://edge.microsoft.com/extensionwebstorebase/v1/crx?response=redirect&prod=chromiumcrx&prodchannel=&x=id%3D{id}%26installsource%3Dondemand%26uc"
        ),
    }
}

/// Return the zip payload inside a .crx file (CRX2 or CRX3).
fn crx_zip(data: &[u8]) -> Result<&[u8], String> {
    if data.len() < 16 || &data[..4] != b"Cr24" {
        // Some mirrors serve the plain zip.
        if data.starts_with(b"PK") {
            return Ok(data);
        }
        return Err("Not an extension package (the store may have refused the download)".into());
    }
    let u32_at = |i: usize| u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
    let start = match u32_at(4) {
        3 => 12 + u32_at(8),
        2 => 16 + u32_at(8) + u32_at(12),
        v => return Err(format!("Unsupported CRX version {v}")),
    };
    data.get(start..).ok_or_else(|| "Corrupt extension package".to_string())
}

/// Download and unpack. Returns the extension's display name.
/// The Web Store's own "Add to Chrome" button makes WebView2 download a .crx from Google's
/// update service; pull the extension id out of that request so Goat can install it.
pub fn id_from_crx_download(url: &str, path: &str) -> Option<String> {
    let lower = url.to_lowercase();
    if !(lower.contains("prod=chromiumcrx") || lower.contains("clients2.google.com/service/update2/crx") || path.to_lowercase().ends_with(".crx")) {
        return None;
    }
    let dec = urlencoding::decode(url).map(|c| c.into_owned()).unwrap_or_else(|_| url.to_string());
    if let Some(i) = dec.find("id=") {
        let id: String = dec[i + 3..].chars().take(32).collect();
        if is_id(&id) {
            return Some(id);
        }
    }
    // Fall back to the file name: <id>_<n>.crx
    let name = std::path::Path::new(path).file_name()?.to_string_lossy().to_string();
    let id: String = name.chars().take(32).collect();
    is_id(&id).then_some(id)
}

static INSTALLING: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// A downloaded extension waiting for you to say yes (Goat asks first, like Chrome).
#[derive(Debug, Clone)]
pub struct Pending {
    pub id: String,
    pub name: String,
    pub version: String,
    /// What it will be able to do, in plain words.
    pub can: Vec<String>,
    tmp: std::path::PathBuf,
}

/// Plain-language list of what an extension's manifest asks for.
pub fn describe(m: &serde_json::Value) -> Vec<String> {
    let mut perms: Vec<String> = Vec::new();
    for k in ["permissions", "host_permissions", "optional_host_permissions"] {
        if k == "optional_host_permissions" {
            continue; // asked for later, not now
        }
        if let Some(a) = m[k].as_array() {
            perms.extend(a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())));
        }
    }
    // Content scripts also need access to their sites.
    if let Some(cs) = m["content_scripts"].as_array() {
        for c in cs {
            if let Some(a) = c["matches"].as_array() {
                perms.extend(a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())));
            }
        }
    }
    let has = |p: &str| perms.iter().any(|x| x == p);
    let broad = perms.iter().any(|p| p == "<all_urls>" || p.starts_with("*://*/") || p == "http://*/*" || p == "https://*/*" || p == "*://*/*");
    let mut out = Vec::new();
    if broad {
        out.push("Read and change all your data on all websites".to_string());
    } else {
        let mut sites: Vec<String> = perms
            .iter()
            .filter(|p| p.contains("://"))
            .filter_map(|p| p.split("://").nth(1).map(|r| r.split('/').next().unwrap_or("").trim_start_matches("*.").to_string()))
            .filter(|h| !h.is_empty() && h != "*")
            .collect();
        sites.sort();
        sites.dedup();
        match sites.len() {
            0 => {}
            1 => out.push(format!("Read and change your data on {}", sites[0])),
            2 => out.push(format!("Read and change your data on {} and {}", sites[0], sites[1])),
            n => out.push(format!("Read and change your data on {}, {} and {} other site{}", sites[0], sites[1], n - 2, if n - 2 == 1 { "" } else { "s" })),
        }
    }
    let table: &[(&str, &str)] = &[
        ("tabs", "Read your browsing history"),
        ("history", "Read and change your browsing history"),
        ("bookmarks", "Read and change your bookmarks"),
        ("cookies", "Read and change cookies"),
        ("downloads", "Manage your downloads"),
        ("clipboardRead", "Read data you copy and paste"),
        ("clipboardWrite", "Change data you copy and paste"),
        ("notifications", "Display notifications"),
        ("nativeMessaging", "Communicate with apps on your computer"),
        ("geolocation", "Detect your physical location"),
        ("management", "Manage your apps, extensions, and themes"),
        ("privacy", "Change your privacy-related settings"),
        ("proxy", "Read and change proxy settings"),
        ("topSites", "Read a list of your most frequently visited websites"),
        ("debugger", "Access the page debugger"),
        ("declarativeNetRequest", "Block content on any page"),
        ("webRequest", "Block or change network requests"),
        ("desktopCapture", "Capture content of your screen"),
        ("pageCapture", "Save pages you visit"),
        ("tts", "Read all text spoken using synthesized speech"),
    ];
    for (p, text) in table {
        if has(p) && !out.iter().any(|o| o == text) {
            out.push(text.to_string());
        }
    }
    out
}

/// Sites installed extensions have their own access to (from host_permissions, not "all sites").
/// Requests to these keep their cookies and skip the ad blocker (when "Don't block extensions").
pub fn extension_sites() -> Vec<String> {
    let mut out = Vec::new();
    for e in list() {
        let Ok(text) = std::fs::read_to_string(ext_dir().join(&e.folder).join("manifest.json")) else { continue };
        let Ok(m) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')) else { continue };
        for k in ["host_permissions", "permissions"] {
            for p in m[k].as_array().into_iter().flatten().filter_map(|v| v.as_str()) {
                let Some(rest) = p.split("://").nth(1) else { continue };
                let host = rest.split('/').next().unwrap_or("").trim_start_matches("*.").to_lowercase();
                if !host.is_empty() && host != "*" && !out.contains(&host) {
                    out.push(host);
                }
            }
        }
    }
    out
}

/// Accept: move the downloaded extension into place. Returns its name.
pub fn confirm(p: &Pending) -> Result<String, String> {
    let dest = ext_dir().join(&p.id);
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&p.tmp, &dest).map_err(|e| e.to_string())?;
    Ok(info(&dest).map(|i| i.name).unwrap_or_else(|| p.id.clone()))
}

/// Decline: throw the download away.
pub fn discard(p: &Pending) {
    let _ = std::fs::remove_dir_all(&p.tmp);
}

pub fn install(src: Source, id: &str) -> Result<Pending, String> {
    // One install per extension at a time (the store button may be clicked repeatedly).
    {
        let mut busy = INSTALLING.lock().unwrap();
        if busy.iter().any(|b| b == id) {
            return Err("Already being added…".into());
        }
        busy.push(id.to_string());
    }
    let r = install_inner(src, id);
    INSTALLING.lock().unwrap().retain(|b| b != id);
    r
}

fn install_inner(src: Source, id: &str) -> Result<Pending, String> {
    let data = net::get_bytes(&crx_url(src, id), 300 << 20, |_, _| {})?;
    let zip_bytes = crx_zip(&data)?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|e| format!("Couldn't open package: {e}"))?;

    let root = ext_dir();
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let tmp = root.join(format!(".{id}.tmp"));
    let _ = std::fs::remove_dir_all(&tmp);
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = f.enclosed_name() else { continue };
        // Chrome's signature folder; WebView2 refuses to load unpacked extensions that contain it.
        if rel.components().next().is_some_and(|c| c.as_os_str() == "_metadata") {
            continue;
        }
        let out = tmp.join(rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        } else {
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            let mut buf = Vec::with_capacity(f.size() as usize);
            f.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            std::fs::write(&out, buf).map_err(|e| e.to_string())?;
        }
    }
    if !tmp.join("manifest.json").exists() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("Package has no manifest.json".into());
    }
    let i = info(&tmp);
    let m: serde_json::Value = std::fs::read_to_string(tmp.join("manifest.json"))
        .ok()
        .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default();
    Ok(Pending {
        id: id.to_string(),
        name: i.as_ref().map(|i| i.name.clone()).filter(|n| !n.starts_with('.')).unwrap_or_else(|| id.to_string()),
        version: i.map(|i| i.version).unwrap_or_default(),
        can: describe(&m),
        tmp,
    })
}

pub fn remove(folder: &str) -> Result<(), String> {
    if folder.is_empty() || folder.contains(['/', '\\']) || folder.starts_with('.') {
        return Err("bad folder".into());
    }
    std::fs::remove_dir_all(ext_dir().join(folder)).map_err(|e| e.to_string())
}

#[derive(Clone, Debug)]
pub struct ExtInfo {
    pub folder: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub valid: bool,
    /// Toolbar pop-up page (manifest action.default_popup), relative to the extension.
    pub popup: String,
    /// Options page, relative to the extension.
    pub options: String,
    /// Icon file (largest up to 48 px), full path.
    pub icon: Option<std::path::PathBuf>,
}

/// Read name/version, resolving "__MSG_name__" placeholders from _locales.
fn info(dir: &std::path::Path) -> Option<ExtInfo> {
    let folder = dir.file_name()?.to_string_lossy().into_owned();
    let Some(m) = std::fs::read_to_string(dir.join("manifest.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s.trim_start_matches('\u{feff}')).ok())
    else {
        return Some(ExtInfo { name: folder.clone(), folder, version: String::new(), description: String::new(), valid: false, popup: String::new(), options: String::new(), icon: None });
    };
    let locale = m["default_locale"].as_str().unwrap_or("en").to_string();
    let msgs: serde_json::Value = ["en", "en_US", &locale]
        .iter()
        .find_map(|l| std::fs::read_to_string(dir.join("_locales").join(l).join("messages.json")).ok())
        .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default();
    let resolve = |v: &serde_json::Value| -> String {
        let s = v.as_str().unwrap_or("");
        match s.strip_prefix("__MSG_").and_then(|r| r.strip_suffix("__")) {
            Some(key) => msgs
                .as_object()
                .and_then(|o| o.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)))
                .and_then(|(_, v)| v["message"].as_str())
                .unwrap_or(key)
                .to_string(),
            None => s.to_string(),
        }
    };
    let name = resolve(&m["name"]);
    let action = if m["action"].is_object() { &m["action"] } else { &m["browser_action"] };
    let popup = action["default_popup"].as_str().unwrap_or("").trim_start_matches('/').to_string();
    let options = m["options_ui"]["page"].as_str().or(m["options_page"].as_str()).unwrap_or("").trim_start_matches('/').to_string();
    // Icon: prefer the toolbar icon, then the extension icon; largest size up to 48.
    let pick = |v: &serde_json::Value| -> Option<String> {
        if let Some(s) = v.as_str() {
            return Some(s.to_string());
        }
        let o = v.as_object()?;
        let mut sizes: Vec<(u32, String)> = o.iter().filter_map(|(k, v)| Some((k.parse().ok()?, v.as_str()?.to_string()))).collect();
        sizes.sort();
        sizes.iter().rev().find(|(n, _)| *n <= 48).or(sizes.first()).map(|(_, p)| p.clone())
    };
    let icon = pick(&action["default_icon"])
        .or_else(|| pick(&m["icons"]))
        .map(|p| dir.join(p.trim_start_matches('/')))
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")));
    Some(ExtInfo {
        name: if name.is_empty() { folder.clone() } else { name },
        version: m["version"].as_str().unwrap_or("").to_string(),
        description: resolve(&m["description"]),
        folder,
        valid: true,
        popup,
        options,
        icon,
    })
}

pub fn list() -> Vec<ExtInfo> {
    let mut out: Vec<ExtInfo> = std::fs::read_dir(ext_dir())
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'))
                .filter_map(|e| info(&e.path()))
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|e| e.name.to_lowercase());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn store_urls() {
        let (s, id) = parse_store_url("https://chromewebstore.google.com/detail/ublock-origin-lite/ddkjiahejlhfcafbddmgiahcphecmpfh?hl=en").unwrap();
        assert_eq!((s, id.as_str()), (Source::Chrome, "ddkjiahejlhfcafbddmgiahcphecmpfh"));
        let (s, _) = parse_store_url("https://microsoftedge.microsoft.com/addons/detail/ublock-origin/odfafepnkmbhccpbejgmiehpchacaeak").unwrap();
        assert_eq!(s, Source::Edge);
        assert!(parse_store_url("https://example.com/").is_none());
    }
    #[test]
    fn crx_headers() {
        let mut v = b"Cr24".to_vec();
        v.extend(3u32.to_le_bytes());
        v.extend(4u32.to_le_bytes());
        v.extend([0, 0, 0, 0]);
        v.extend(b"PK\x03\x04rest");
        assert!(crx_zip(&v).unwrap().starts_with(b"PK"));
    }
}


#[cfg(test)]
mod crx_tests {
    #[test]
    fn crx_ids() {
        let u = "https://permanently-removed.invalid/?response=redirect&os=win&arch=x64&prod=chromiumcrx&prodchannel=&prodversion=157.0.4341.46&lang=en-GB&acceptformat=crx3,puff&x=id%3Dpejdijmoenmkgeppbflobdenhhabjlaj%26installsource%3Dondemand%26uc";
        assert_eq!(super::id_from_crx_download(u, "x").as_deref(), Some("pejdijmoenmkgeppbflobdenhhabjlaj"));
        assert_eq!(super::id_from_crx_download("https://a.com/f.zip", "C:/f.zip"), None);
    }
}

#[cfg(test)]
mod describe_tests {
    #[test]
    fn perms() {
        let m: serde_json::Value = serde_json::from_str(r#"{"permissions":["tabs","notifications","storage","nativeMessaging"],"host_permissions":["https://*.roblox.com/*","https://ropro.io/*","https://*.rbxcdn.com/*"]}"#).unwrap();
        let d = super::describe(&m);
        assert_eq!(d[0], "Read and change your data on rbxcdn.com, roblox.com and 1 other site");
        assert!(d.contains(&"Display notifications".to_string()));
        let all: serde_json::Value = serde_json::from_str(r#"{"host_permissions":["<all_urls>"]}"#).unwrap();
        assert_eq!(super::describe(&all), vec!["Read and change all your data on all websites"]);
    }
}

