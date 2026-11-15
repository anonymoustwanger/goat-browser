//! Goatr's installer, updater and uninstaller (a port of Goat Browser's setup screen).
//!
//!   GoatrSetup.exe                  install the latest Goatr from GitHub Releases (or update it)
//!   "Goatr Setup.exe" --uninstall   remove Goatr (Windows "Apps > Uninstall")
//!   ... --uninstall --silent        remove without asking, keeping your data
//!
//! Installs per user (no admin) into %LOCALAPPDATA%\Programs\Goatr, adds Start menu and desktop
//! shortcuts and registers Goatr as a browser in HKCU. Your data in %APPDATA%\Goatr is only
//! deleted if you tick the box while uninstalling.
#![cfg_attr(windows, windows_subsystem = "windows")]
#![allow(deprecated)]

use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const APP: &str = "Goatr";
const EXE: &str = "Goatr.exe";
const SETUP_EXE: &str = "Goatr Setup.exe";
const REPO: &str = "anonymoustwanger/goat-browser";
const REGKEY: &str = "Goatr";
const PROGID: &str = "GoatrHTML";
const MARKER: &str = ".goatr-installed";

// Midnight, Goat's default theme.
const BG: Color32 = Color32::from_rgb(0x10, 0x12, 0x14);
const BG2: Color32 = Color32::from_rgb(0x18, 0x1b, 0x1f);
const BG3: Color32 = Color32::from_rgb(0x22, 0x26, 0x2b);
const LINE: Color32 = Color32::from_rgb(0x2b, 0x30, 0x36);
const TEXT: Color32 = Color32::from_rgb(0xe7, 0xe9, 0xec);
const MUTED: Color32 = Color32::from_rgb(0x8b, 0x93, 0x9c);
const ACCENT: Color32 = Color32::from_rgb(0x5f, 0xd3, 0xb3);
const ACCENT2: Color32 = Color32::from_rgb(0x6e, 0xa8, 0xff);
const INK: Color32 = Color32::from_rgb(0x06, 0x23, 0x1b);
const DANGER: Color32 = Color32::from_rgb(0xff, 0x7a, 0x66);

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Install,
    Uninstall,
}

#[derive(Clone, PartialEq)]
enum Screen {
    Welcome,
    Confirm,
    Work,
    Done,
    Error(String),
}

#[derive(Clone)]
struct Release {
    version: String,
    exe_url: String,
    exe_size: u64,
    dll_url: Option<String>,
}

#[derive(Default)]
struct Shared {
    release: Option<Result<Release, String>>,
    step: String,
    pct: f32,
    result: Option<Result<(), String>>,
}

struct SetupApp {
    mode: Mode,
    screen: Screen,
    dir: String,
    existing: Option<(PathBuf, String)>,
    desktop: bool,
    make_default: bool,
    remove_data: bool,
    shared: Arc<Mutex<Shared>>,
    logo: Option<egui::TextureHandle>,
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mode = if args.iter().any(|a| a == "--uninstall") { Mode::Uninstall } else { Mode::Install };
    if mode == Mode::Uninstall && args.iter().any(|a| a == "--silent") {
        let _ = uninstall(&exe_dir(), false, &|_, _| {});
        return Ok(());
    }
    let viewport = egui::ViewportBuilder::default()
        .with_title(if mode == Mode::Install { "Goatr setup" } else { "Uninstall Goatr" })
        .with_inner_size([640.0, 460.0])
        .with_resizable(false)
        .with_maximize_button(false)
        .with_decorations(false)
        .with_icon(icon());
    let options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Glow, centered: true, ..Default::default() };
    eframe::run_native(
        "Goatr setup",
        options,
        Box::new(move |cc| {
            fonts(&cc.egui_ctx);
            let existing = installed();
            let dir = existing.as_ref().map(|e| e.0.clone()).unwrap_or_else(default_dir);
            let shared = Arc::new(Mutex::new(Shared::default()));
            if mode == Mode::Install {
                fetch_release(shared.clone(), cc.egui_ctx.clone());
            }
            Ok(Box::new(SetupApp {
                mode,
                screen: if mode == Mode::Install { Screen::Welcome } else { Screen::Confirm },
                dir: dir.display().to_string(),
                existing,
                desktop: true,
                make_default: false,
                remove_data: false,
                shared,
                logo: None,
            }))
        }),
    )
}

// ---------------------------------------------------------------- paths

fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."))
}

fn default_dir() -> PathBuf {
    #[cfg(windows)]
    {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"));
        local.join("Programs").join(APP)
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local").join("share").join("goatr-app")
    }
}

fn data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join(APP)
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local").join("share").join("goatr")
    }
}

/// Where Goatr is installed now, and which version.
fn installed() -> Option<(PathBuf, String)> {
    let mut candidates = vec![];
    #[cfg(windows)]
    if let Some(loc) = reg_query(&uninst_key(), "InstallLocation") {
        candidates.push(PathBuf::from(loc));
    }
    candidates.push(exe_dir());
    candidates.push(default_dir());
    for d in candidates {
        if let Ok(m) = std::fs::read_to_string(d.join(MARKER)) {
            let v = serde_json::from_str::<serde_json::Value>(&m).ok().and_then(|j| j["version"].as_str().map(String::from)).unwrap_or_default();
            return Some((d, v));
        }
    }
    None
}

// ---------------------------------------------------------------- network

fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::NativeTls)
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(Duration::from_secs(900)))
        .max_redirects(10)
        .http_status_as_error(false)
        .user_agent("Goatr-Setup")
        .build()
        .into()
}

fn latest_release() -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut resp = agent()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("Couldn't reach GitHub: {e}"))?;
    match resp.status().as_u16() {
        200 => {}
        404 => return Err("No Goatr release found on GitHub yet (or the repository is private).".into()),
        403 | 429 => return Err("GitHub is limiting requests right now. Try again in a few minutes.".into()),
        s => return Err(format!("GitHub answered {s}.")),
    }
    let text = resp.body_mut().read_to_string().map_err(|e| format!("Unexpected answer from GitHub: {e}"))?;
    let j: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("Unexpected answer from GitHub: {e}"))?;
    let version = j["tag_name"].as_str().or(j["name"].as_str()).unwrap_or("").trim_start_matches(['v', 'V']).to_string();
    let assets = j["assets"].as_array().cloned().unwrap_or_default();
    let find = |want: &dyn Fn(&str) -> bool| {
        assets.iter().find(|a| want(&a["name"].as_str().unwrap_or("").to_lowercase())).cloned()
    };
    let exe = find(&|n| n == "goatr.exe").ok_or("The latest release has no Goatr.exe to download.")?;
    let dll = find(&|n| n == "webview2loader.dll");
    Ok(Release {
        version: if version.is_empty() { "latest".into() } else { version },
        exe_url: exe["browser_download_url"].as_str().unwrap_or("").to_string(),
        exe_size: exe["size"].as_u64().unwrap_or(0),
        dll_url: dll.and_then(|d| d["browser_download_url"].as_str().map(String::from)),
    })
}

fn fetch_release(shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
    shared.lock().unwrap().release = None;
    std::thread::spawn(move || {
        let r = latest_release();
        shared.lock().unwrap().release = Some(r);
        ctx.request_repaint();
    });
}

fn download(url: &str, size_hint: u64, mut progress: impl FnMut(f32)) -> Result<Vec<u8>, String> {
    let mut resp = agent().get(url).call().map_err(|e| format!("Download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Download failed (GitHub answered {}).", resp.status().as_u16()));
    }
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(size_hint);
    let mut reader = resp.body_mut().with_config().limit(1 << 30).reader();
    let mut out = Vec::with_capacity(total as usize);
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("Download failed: {e}"))?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if total > 0 {
            progress(out.len() as f32 / total as f32);
        }
    }
    if size_hint > 0 && out.len() as u64 != size_hint {
        return Err("The download was incomplete. Check your connection and try again.".into());
    }
    Ok(out)
}

// ---------------------------------------------------------------- install / uninstall

/// Copy of the WebView2 loader Goatr needs, in case a release doesn't include it.
#[cfg(windows)]
const WEBVIEW2_LOADER: Option<&[u8]> = Some(include_bytes!("../../assets/WebView2Loader.dll"));
#[cfg(not(windows))]
const WEBVIEW2_LOADER: Option<&[u8]> = None;

fn install(rel: &Release, dir: &Path, desktop: bool, make_default: bool, step: &dyn Fn(&str, f32)) -> Result<(), String> {
    step("Downloading Goatr…", 0.0);
    let exe_bytes = download(&rel.exe_url, rel.exe_size, |f| step("Downloading Goatr…", f * 0.8))?;
    let dll_bytes = match &rel.dll_url {
        Some(u) => Some(download(u, 0, |_| {})?),
        None => WEBVIEW2_LOADER.map(|b| b.to_vec()),
    };

    step("Checking Goatr isn't running…", 0.82);
    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let exe = dir.join(EXE);
    if exe.exists() {
        // A running .exe can't be replaced: moving it aside tells us whether it's in use.
        let old = dir.join("Goatr.old.exe");
        let _ = std::fs::remove_file(&old);
        if std::fs::rename(&exe, &old).is_err() {
            return Err("Goatr is open. Close all Goatr windows (including private and Tor windows), then click Try again.".into());
        }
        let _ = std::fs::remove_file(&old);
    }

    step("Copying files…", 0.86);
    std::fs::write(&exe, &exe_bytes).map_err(|e| format!("Couldn't write Goatr.exe: {e}"))?;
    if let Some(b) = dll_bytes {
        let p = dir.join("WebView2Loader.dll");
        if std::fs::write(&p, &b).is_err() && !p.exists() {
            return Err("Couldn't write WebView2Loader.dll.".into());
        }
    }
    // Keep a copy of this program as the uninstaller.
    if let Ok(me) = std::env::current_exe() {
        let to = dir.join(SETUP_EXE);
        if !same_file(&me, &to) {
            let _ = std::fs::copy(&me, &to);
        }
    }
    let _ = std::fs::write(dir.join("THIRD-PARTY-NOTICES.txt"), include_str!("../../THIRD-PARTY-NOTICES.txt"));
    let marker = serde_json::json!({ "version": rel.version, "installed": chrono::Local::now().to_rfc3339() });
    let _ = std::fs::write(dir.join(MARKER), marker.to_string());

    #[cfg(windows)]
    {
        step("Adding shortcuts…", 0.92);
        shortcut(&start_menu_link(), dir)?;
        if desktop {
            shortcut(&desktop_link(), dir)?;
        } else {
            let _ = std::fs::remove_file(desktop_link());
        }
        step("Registering with Windows…", 0.96);
        register(dir, &rel.version)?;
        if make_default {
            let _ = std::process::Command::new("cmd")
                .args(["/d", "/c", "start", "", &format!("ms-settings:defaultapps?registeredAppUser={REGKEY}")])
                .creation_flags_hidden()
                .spawn();
        }
    }
    let _ = (desktop, make_default);
    step("Done", 1.0);
    Ok(())
}

fn uninstall(dir: &Path, remove_data: bool, step: &dyn Fn(&str, f32)) -> Result<(), String> {
    step("Checking Goatr isn't running…", 0.1);
    let exe = dir.join(EXE);
    if exe.exists() {
        let probe = dir.join("Goatr.closing.exe");
        if std::fs::rename(&exe, &probe).is_err() {
            return Err("Goatr is open. Close all Goatr windows, then try again.".into());
        }
        let _ = std::fs::rename(&probe, &exe);
    }
    #[cfg(windows)]
    {
        step("Removing shortcuts…", 0.3);
        let _ = std::fs::remove_file(start_menu_link());
        let _ = std::fs::remove_file(desktop_link());
        step("Removing from Windows…", 0.5);
        unregister();
    }
    if remove_data {
        step("Deleting your data…", 0.7);
        let _ = std::fs::remove_dir_all(data_dir());
    }
    step("Done", 1.0);
    schedule_self_delete(dir);
    Ok(())
}

/// The install folder (with this uninstaller in it) can't be deleted while we run from it,
/// so a hidden helper deletes it a moment after we exit. Only ever a folder we installed.
fn schedule_self_delete(dir: &Path) {
    if !dir.join(MARKER).exists() {
        return;
    }
    #[cfg(windows)]
    {
        let cmd = format!("ping 127.0.0.1 -n 4 >nul & rmdir /s /q \"{}\"", dir.display());
        let _ = std::process::Command::new("cmd")
            .args(["/d", "/c", &cmd])
            .current_dir(std::env::temp_dir())
            .creation_flags_hidden()
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("sh").args(["-c", &format!("sleep 3; rm -rf '{}'", dir.display())]).spawn();
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

trait HiddenWindow {
    fn creation_flags_hidden(&mut self) -> &mut Self;
}
impl HiddenWindow for std::process::Command {
    fn creation_flags_hidden(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        self
    }
}

#[cfg(windows)]
fn uninst_key() -> String {
    format!(r"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Uninstall\{REGKEY}")
}

#[cfg(windows)]
fn reg_query(key: &str, value: &str) -> Option<String> {
    let out = std::process::Command::new("reg").args(["query", key, "/v", value]).creation_flags_hidden().output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.trim_start().starts_with(value))?;
    line.split("REG_SZ").nth(1).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

#[cfg(windows)]
fn start_menu_link() -> PathBuf {
    PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default())
        .join(r"Microsoft\Windows\Start Menu\Programs")
        .join(format!("{APP}.lnk"))
}

#[cfg(windows)]
fn desktop_link() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default());
    // OneDrive can move the desktop.
    let od = std::env::var_os("OneDrive").map(|o| PathBuf::from(o).join("Desktop"));
    let desk = match od {
        Some(d) if d.is_dir() && !home.join("Desktop").is_dir() => d,
        _ => home.join("Desktop"),
    };
    desk.join(format!("{APP}.lnk"))
}

#[cfg(windows)]
fn shortcut(link: &Path, dir: &Path) -> Result<(), String> {
    if let Some(p) = link.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let exe = dir.join(EXE);
    let q = |p: &Path| p.display().to_string().replace('\'', "''");
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.WorkingDirectory='{}';$s.IconLocation='{},0';$s.Description='Goatr browser';$s.Save()",
        q(link),
        q(&exe),
        q(dir),
        q(&exe)
    );
    let ok = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .creation_flags_hidden()
        .status()
        .is_ok_and(|s| s.success());
    if ok { Ok(()) } else { Err("Couldn't create the shortcut.".into()) }
}

/// A .reg file: one `reg import` instead of dozens of `reg add`.
#[cfg(windows)]
fn reg_file(entries: &[(String, Vec<(&str, RegVal)>)]) -> Vec<u8> {
    let esc = |v: &str| v.replace('\\', "\\\\").replace('"', "\\\"");
    let mut txt = String::from("Windows Registry Editor Version 5.00\r\n");
    for (key, values) in entries {
        txt.push_str(&format!("\r\n[{key}]\r\n"));
        for (name, val) in values {
            let n = if *name == "@" { "@".to_string() } else { format!("\"{}\"", esc(name)) };
            match val {
                RegVal::S(s) => txt.push_str(&format!("{n}=\"{}\"\r\n", esc(s))),
                RegVal::D(d) => txt.push_str(&format!("{n}=dword:{d:08x}\r\n")),
            }
        }
    }
    let mut out = vec![0xff, 0xfe];
    for u in txt.encode_utf16() {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}

#[cfg(windows)]
enum RegVal {
    S(String),
    D(u32),
}

#[cfg(windows)]
fn register(dir: &Path, version: &str) -> Result<(), String> {
    use RegVal::{D, S};
    let exe = dir.join(EXE).display().to_string();
    let setup = dir.join(SETUP_EXE).display().to_string();
    let icon = format!("{exe},0");
    let clients = format!(r"HKEY_CURRENT_USER\Software\Clients\StartMenuInternet\{REGKEY}");
    let classes = format!(r"HKEY_CURRENT_USER\Software\Classes\{PROGID}");
    let size_kb = std::fs::read_dir(dir).map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum::<u64>() / 1024).unwrap_or(0) as u32;
    let entries: Vec<(String, Vec<(&str, RegVal)>)> = vec![
        (uninst_key(), vec![
            ("DisplayName", S(APP.into())),
            ("DisplayVersion", S(version.into())),
            ("Publisher", S(APP.into())),
            ("DisplayIcon", S(icon.clone())),
            ("InstallLocation", S(dir.display().to_string())),
            ("UninstallString", S(format!("\"{setup}\" --uninstall"))),
            ("QuietUninstallString", S(format!("\"{setup}\" --uninstall --silent"))),
            ("URLInfoAbout", S(format!("https://github.com/{REPO}"))),
            ("NoModify", D(1)),
            ("NoRepair", D(1)),
            ("EstimatedSize", D(size_kb)),
        ]),
        (classes.clone(), vec![("@", S("Goatr HTML Document".into())), ("URL Protocol", S(String::new()))]),
        (format!(r"{classes}\DefaultIcon"), vec![("@", S(icon.clone()))]),
        (format!(r"{classes}\shell\open\command"), vec![("@", S(format!("\"{exe}\" \"%1\"")))]),
        (format!(r"{classes}\Application"), vec![("ApplicationName", S(APP.into())), ("ApplicationIcon", S(icon.clone()))]),
        (clients.clone(), vec![("@", S(APP.into()))]),
        (format!(r"{clients}\DefaultIcon"), vec![("@", S(icon.clone()))]),
        (format!(r"{clients}\shell\open\command"), vec![("@", S(format!("\"{exe}\"")))]),
        (format!(r"{clients}\Capabilities"), vec![
            ("ApplicationName", S(APP.into())),
            ("ApplicationDescription", S("A private, fast browser.".into())),
            ("ApplicationIcon", S(icon.clone())),
        ]),
        (format!(r"{clients}\Capabilities\StartMenu"), vec![("StartMenuInternet", S(REGKEY.into()))]),
        (format!(r"{clients}\Capabilities\URLAssociations"), vec![("http", S(PROGID.into())), ("https", S(PROGID.into()))]),
        (format!(r"{clients}\Capabilities\FileAssociations"), vec![
            (".htm", S(PROGID.into())),
            (".html", S(PROGID.into())),
            (".pdf", S(PROGID.into())),
        ]),
        (r"HKEY_CURRENT_USER\Software\RegisteredApplications".to_string(), vec![(REGKEY, S(format!(r"Software\Clients\StartMenuInternet\{REGKEY}\Capabilities")))]),
    ];
    let file = std::env::temp_dir().join(format!("goatr-reg-{}.reg", std::process::id()));
    std::fs::write(&file, reg_file(&entries)).map_err(|e| e.to_string())?;
    let ok = std::process::Command::new("reg").arg("import").arg(&file).creation_flags_hidden().status().is_ok_and(|s| s.success());
    let _ = std::fs::remove_file(&file);
    if ok { Ok(()) } else { Err("Couldn't register Goatr with Windows.".into()) }
}

#[cfg(windows)]
fn unregister() {
    for k in [
        uninst_key(),
        format!(r"HKEY_CURRENT_USER\Software\Clients\StartMenuInternet\{REGKEY}"),
        format!(r"HKEY_CURRENT_USER\Software\Classes\{PROGID}"),
    ] {
        let _ = std::process::Command::new("reg").args(["delete", &k, "/f"]).creation_flags_hidden().status();
    }
    let _ = std::process::Command::new("reg")
        .args(["delete", r"HKEY_CURRENT_USER\Software\RegisteredApplications", "/v", REGKEY, "/f"])
        .creation_flags_hidden()
        .status();
}

// ---------------------------------------------------------------- UI

fn icon() -> egui::IconData {
    let (rgba, w, h) = logo_rgba();
    egui::IconData { rgba, width: w, height: h }
}

fn logo_rgba() -> (Vec<u8>, u32, u32) {
    let dec = png::Decoder::new(std::io::Cursor::new(&include_bytes!("../../assets/icon.png")[..]));
    let mut r = dec.read_info().expect("icon");
    let mut buf = vec![0; r.output_buffer_size().unwrap_or(256 * 256 * 4)];
    let info = r.next_frame(&mut buf).expect("icon");
    buf.truncate(info.buffer_size());
    (buf, info.width, info.height)
}

fn fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let dir = Path::new(r"C:\Windows\Fonts");
    for (name, file, bold) in [("segoe", "segoeui.ttf", false), ("segoe-sb", "seguisb.ttf", true)] {
        if let Ok(b) = std::fs::read(dir.join(file)) {
            fonts.font_data.insert(name.into(), Arc::new(egui::FontData::from_owned(b)));
            let fam = if bold { egui::FontFamily::Name("bold".into()) } else { egui::FontFamily::Proportional };
            fonts.families.entry(fam).or_default().insert(0, name.into());
        }
    }
    let prop = fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();
    fonts.families.entry(egui::FontFamily::Name("bold".into())).or_default().extend(prop);
    ctx.set_fonts(fonts);
}

fn font(size: f32) -> FontId {
    FontId::proportional(size)
}
fn bold(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name("bold".into()))
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

impl SetupApp {
    fn start(&mut self, ctx: &egui::Context) {
        self.screen = Screen::Work;
        {
            let mut s = self.shared.lock().unwrap();
            s.result = None;
            s.pct = 0.0;
            s.step = "Getting ready…".into();
        }
        let shared = self.shared.clone();
        let c = ctx.clone();
        let mode = self.mode;
        let dir = PathBuf::from(self.dir.trim());
        let (desktop, make_default, remove_data) = (self.desktop, self.make_default, self.remove_data);
        let rel = self.shared.lock().unwrap().release.clone().and_then(Result::ok);
        std::thread::spawn(move || {
            let step = |t: &str, p: f32| {
                let mut s = shared.lock().unwrap();
                s.step = t.to_string();
                s.pct = p;
                c.request_repaint();
            };
            let r = match mode {
                Mode::Install => match rel {
                    Some(rel) => install(&rel, &dir, desktop, make_default, &step),
                    None => Err("No release to install.".into()),
                },
                Mode::Uninstall => uninstall(&dir, remove_data, &step),
            };
            shared.lock().unwrap().result = Some(r);
            c.request_repaint();
        });
    }

    fn button(&self, ui: &mut egui::Ui, r: Rect, label: &str, kind: u8, enabled: bool) -> bool {
        // kind: 0 ghost, 1 primary, 2 danger
        let resp = ui.interact(r, Id::new(("btn", label)), if enabled { Sense::click() } else { Sense::hover() });
        let h = ui.ctx().animate_bool_with_time(Id::new(("btn-h", label)), resp.hovered() && enabled, 0.12);
        let p = ui.painter();
        let fade = if enabled { 1.0 } else { 0.45 };
        match kind {
            1 => {
                grad_rect(p, r, 10.0, ACCENT.gamma_multiply(fade), ACCENT2.gamma_multiply(fade));
                if h > 0.0 {
                    p.rect_filled(r, 10, Color32::WHITE.gamma_multiply(0.10 * h));
                }
                p.text(r.center(), Align2::CENTER_CENTER, label, bold(13.5), INK);
            }
            2 => {
                p.rect_filled(r, 10, mix(DANGER, Color32::WHITE, 0.12 * h).gamma_multiply(fade));
                p.text(r.center(), Align2::CENTER_CENTER, label, bold(13.5), Color32::from_rgb(0x2a, 0x08, 0x04));
            }
            _ => {
                p.rect_filled(r, 10, mix(BG2, BG3, h));
                p.rect_stroke(r, 10, Stroke::new(1.0, LINE), StrokeKind::Inside);
                p.text(r.center(), Align2::CENTER_CENTER, label, font(13.5), TEXT);
            }
        }
        if resp.hovered() && enabled {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        resp.clicked() && enabled
    }

    fn checkbox(&self, ui: &mut egui::Ui, r: Rect, on: &mut bool, title: &str, sub: &str) {
        let resp = ui.interact(r, Id::new(("opt", title)), Sense::click());
        if resp.clicked() {
            *on = !*on;
        }
        let p = ui.painter();
        if resp.hovered() {
            p.rect_filled(r, 10, BG3.gamma_multiply(0.6));
        }
        let b = Rect::from_min_size(pos2(r.left() + 14.0, r.top() + 14.0), Vec2::splat(18.0));
        let t = ui.ctx().animate_bool_with_time(Id::new(("opt-t", title)), *on, 0.14);
        p.rect_filled(b, 5, mix(BG, ACCENT, t));
        p.rect_stroke(b, 5, Stroke::new(1.0, mix(LINE, ACCENT, t)), StrokeKind::Inside);
        if t > 0.05 {
            let c = b.center();
            let pts = vec![pos2(c.x - 4.5, c.y + 0.2), pos2(c.x - 1.3, c.y + 3.4), pos2(c.x + 4.6, c.y - 3.6)];
            p.add(egui::Shape::line(pts, Stroke::new(2.0, INK.gamma_multiply(t))));
        }
        p.text(pos2(b.right() + 12.0, r.top() + 12.0), Align2::LEFT_TOP, title, bold(13.5), TEXT);
        p.text(pos2(b.right() + 12.0, r.top() + 32.0), Align2::LEFT_TOP, sub, font(12.0), MUTED);
    }

    fn hero(&mut self, ui: &mut egui::Ui, at: Pos2, title: &str, sub: &str, sub_color: Color32) {
        let tex = self.logo_tex(ui.ctx());
        let logo = Rect::from_min_size(at, Vec2::splat(56.0));
        ui.painter().image(tex.id(), logo, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        ui.painter().text(pos2(logo.right() + 16.0, at.y + 6.0), Align2::LEFT_TOP, title, bold(22.0), TEXT);
        ui.painter().text(pos2(logo.right() + 16.0, at.y + 36.0), Align2::LEFT_TOP, sub, font(13.0), sub_color);
    }

    fn logo_tex(&mut self, ctx: &egui::Context) -> egui::TextureHandle {
        self.logo
            .get_or_insert_with(|| {
                let (rgba, w, h) = logo_rgba();
                ctx.load_texture("logo", egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba), egui::TextureOptions::LINEAR)
            })
            .clone()
    }
}

fn grad_rect(p: &egui::Painter, r: Rect, radius: f32, a: Color32, b: Color32) {
    // Rounded rect with a diagonal gradient, soft-edged.
    let pts = {
        let mut v = vec![];
        let corners = [
            (pos2(r.left() + radius, r.top() + radius), std::f32::consts::PI),
            (pos2(r.right() - radius, r.top() + radius), -std::f32::consts::FRAC_PI_2),
            (pos2(r.right() - radius, r.bottom() - radius), 0.0),
            (pos2(r.left() + radius, r.bottom() - radius), std::f32::consts::FRAC_PI_2),
        ];
        for (c, a0) in corners {
            for i in 0..=10 {
                v.push(c + Vec2::angled(a0 + i as f32 / 10.0 * std::f32::consts::FRAC_PI_2) * radius);
            }
        }
        v
    };
    let dir = vec2(1.0, 1.0).normalized();
    let len = r.width() * dir.x + r.height() * dir.y;
    let col = |q: Pos2| mix(a, b, (0.5 + (q - r.center()).dot(dir) / len).clamp(0.0, 1.0));
    let feather = 1.0 / p.ctx().pixels_per_point();
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(r.center(), col(r.center()));
    let n = pts.len();
    for i in 0..n {
        let out = (pts[i] - r.center()).normalized() * feather * 0.5;
        mesh.colored_vertex(pts[i] - out, col(pts[i]));
        mesh.colored_vertex(pts[i] + out, Color32::TRANSPARENT);
    }
    let n = n as u32;
    for i in 0..n {
        let j = (i + 1) % n;
        let (ii, io, ji, jo) = (1 + 2 * i, 2 + 2 * i, 1 + 2 * j, 2 + 2 * j);
        mesh.add_triangle(0, ii, ji);
        mesh.add_triangle(ii, io, ji);
        mesh.add_triangle(io, jo, ji);
    }
    p.add(egui::Shape::mesh(mesh));
}

impl eframe::App for SetupApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0, BG);
        // Soft accent glow like Goat's page background.
        for i in 0..12 {
            let k = i as f32 / 12.0;
            ui.painter().circle_filled(pos2(full.left() + 40.0, full.top() - 40.0), 420.0 * (1.0 - k), ACCENT.gamma_multiply(0.012));
        }

        // Title bar: drag to move, minimise, close.
        let bar = Rect::from_min_size(full.min, vec2(full.width(), 40.0));
        let drag = ui.interact(bar, Id::new("bar"), Sense::click_and_drag());
        if drag.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        let title = if self.mode == Mode::Install { "Goatr setup" } else { "Uninstall Goatr" };
        ui.painter().text(pos2(bar.left() + 16.0, bar.center().y), Align2::LEFT_CENTER, title, font(12.5), MUTED);
        let busy = self.screen == Screen::Work;
        for (i, kind) in [(0, "close"), (1, "min")] {
            let r = Rect::from_min_size(pos2(bar.right() - 46.0 * (i as f32 + 1.0), bar.top()), vec2(46.0, 40.0));
            let resp = ui.interact(r, Id::new(kind), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r, 0, if kind == "close" { Color32::from_rgb(0xc4, 0x2b, 0x1c) } else { BG3 });
            }
            let c = r.center();
            let s = Stroke::new(1.2, TEXT);
            if kind == "close" {
                ui.painter().line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], s);
                ui.painter().line_segment([c + vec2(5.0, -5.0), c + vec2(-5.0, 5.0)], s);
            } else {
                ui.painter().line_segment([c + vec2(-5.0, 0.0), c + vec2(5.0, 0.0)], s);
            }
            if resp.clicked() {
                if kind == "close" {
                    if !busy {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                } else {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
            }
        }

        // Finished work → next screen.
        if busy {
            if let Some(r) = self.shared.lock().unwrap().result.take() {
                self.screen = match r {
                    Ok(()) => Screen::Done,
                    Err(e) => Screen::Error(e),
                };
            }
        }

        let body = Rect::from_min_max(pos2(full.left() + 36.0, full.top() + 64.0), pos2(full.right() - 36.0, full.bottom() - 28.0));
        let actions_y = body.bottom() - 40.0;
        let primary_r = Rect::from_min_size(pos2(body.right() - 170.0, actions_y), vec2(170.0, 40.0));
        let ghost_r = Rect::from_min_size(pos2(primary_r.left() - 12.0 - 120.0, actions_y), vec2(120.0, 40.0));

        match self.screen.clone() {
            Screen::Welcome => {
                let (rel, sub, sub_col) = {
                    let s = self.shared.lock().unwrap();
                    match &s.release {
                        None => (None, "Checking GitHub for the latest Goatr…".to_string(), MUTED),
                        Some(Ok(r)) => {
                            let sub = match &self.existing {
                                Some((_, v)) if !v.is_empty() && v == &r.version => format!("Goatr {} is installed. Reinstall it to repair.", r.version),
                                Some((_, v)) if !v.is_empty() => format!("Update from {v} to Goatr {}.", r.version),
                                _ => format!("Private, fast, and yours. Goatr {}.", r.version),
                            };
                            (Some(r.clone()), sub, MUTED)
                        }
                        Some(Err(e)) => (None, e.clone(), DANGER),
                    }
                };
                let failed = matches!(self.shared.lock().unwrap().release, Some(Err(_)));
                let title = if self.existing.is_some() { "Update Goatr" } else { "Install Goatr" };
                self.hero(ui, body.min, title, &sub, sub_col);

                let card = Rect::from_min_size(pos2(body.left(), body.top() + 84.0), vec2(body.width(), 3.0 * 58.0 + 12.0));
                ui.painter().rect_filled(card, 14, BG2);
                ui.painter().rect_stroke(card, 14, Stroke::new(1.0, LINE), StrokeKind::Inside);
                let row = |i: f32| Rect::from_min_size(pos2(card.left() + 6.0, card.top() + 6.0 + i * 58.0), vec2(card.width() - 12.0, 58.0));
                let mut desktop = self.desktop;
                self.checkbox(ui, row(0.0), &mut desktop, "Desktop shortcut", "Put Goatr on your desktop.");
                self.desktop = desktop;
                let mut def = self.make_default;
                self.checkbox(ui, row(1.0), &mut def, "Make Goatr my default browser", "Opens Windows settings after installing, so links open in Goatr.");
                self.make_default = def;
                // Install location.
                let loc = row(2.0);
                ui.painter().text(pos2(loc.left() + 14.0, loc.top() + 12.0), Align2::LEFT_TOP, "Install location", bold(13.5), TEXT);
                let field = Rect::from_min_max(pos2(loc.left() + 14.0, loc.top() + 30.0), pos2(loc.right() - 14.0, loc.bottom() - 6.0));
                let editable = self.existing.is_none();
                ui.scope_builder(egui::UiBuilder::new().max_rect(field), |ui| {
                    ui.add_enabled(
                        editable,
                        egui::TextEdit::singleline(&mut self.dir).frame(egui::Frame::NONE).font(font(12.0)).text_color(MUTED).desired_width(field.width()),
                    );
                });

                ui.painter().text(
                    pos2(body.left(), card.bottom() + 16.0),
                    Align2::LEFT_TOP,
                    "Installs just for you, no admin needed. Your data in %APPDATA%\\Goatr is kept.",
                    font(12.0),
                    MUTED,
                );
                if self.button(ui, ghost_r, "Cancel", 0, true) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if failed {
                    if self.button(ui, primary_r, "Try again", 1, true) {
                        fetch_release(self.shared.clone(), ctx.clone());
                    }
                } else {
                    let label = if self.existing.is_some() { "Update" } else { "Install" };
                    if self.button(ui, primary_r, label, 1, rel.is_some() && !self.dir.trim().is_empty()) {
                        self.start(&ctx);
                    }
                }
            }
            Screen::Confirm => {
                self.hero(ui, body.min, "Uninstall Goatr", "Goatr will be removed from this computer.", MUTED);
                let card = Rect::from_min_size(pos2(body.left(), body.top() + 84.0), vec2(body.width(), 96.0));
                ui.painter().rect_filled(card, 14, BG2);
                ui.painter().rect_stroke(card, 14, Stroke::new(1.0, LINE), StrokeKind::Inside);
                let mut rm = self.remove_data;
                let r = Rect::from_min_size(pos2(card.left() + 6.0, card.top() + 6.0), vec2(card.width() - 12.0, 84.0));
                self.checkbox(ui, r, &mut rm, "Also delete my data", "History, bookmarks, settings, passwords saved in sites and extensions.");
                self.remove_data = rm;
                ui.painter().text(pos2(r.left() + 44.0, r.top() + 54.0), Align2::LEFT_TOP, data_dir().display().to_string(), font(11.5), MUTED.gamma_multiply(0.8));
                if self.button(ui, ghost_r, "Keep Goatr", 0, true) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if self.button(ui, primary_r, "Uninstall", 2, true) {
                    self.dir = self.existing.as_ref().map(|e| e.0.clone()).unwrap_or_else(exe_dir).display().to_string();
                    self.start(&ctx);
                }
            }
            Screen::Work => {
                let (step, pct) = {
                    let s = self.shared.lock().unwrap();
                    (s.step.clone(), s.pct)
                };
                let tex = self.logo_tex(&ctx);
                let c = pos2(full.center().x, body.top() + 70.0);
                // Gentle breathing logo.
                let t = ctx.input(|i| i.time) as f32;
                let s = 76.0 + (t * 2.4).sin() * 3.0;
                ui.painter().image(tex.id(), Rect::from_center_size(c, Vec2::splat(s)), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                let head = if self.mode == Mode::Install { if self.existing.is_some() { "Updating…" } else { "Installing…" } } else { "Uninstalling…" };
                ui.painter().text(pos2(c.x, c.y + 62.0), Align2::CENTER_TOP, head, bold(22.0), TEXT);
                ui.painter().text(pos2(c.x, c.y + 96.0), Align2::CENTER_TOP, &step, font(13.0), MUTED);
                let track = Rect::from_center_size(pos2(c.x, c.y + 140.0), vec2(360.0, 8.0));
                ui.painter().rect_filled(track, 4, BG3);
                let shown = ctx.animate_value_with_time(Id::new("pct"), pct.clamp(0.0, 1.0), 0.25);
                if shown > 0.005 {
                    let fill = Rect::from_min_size(track.min, vec2(track.width() * shown, track.height()));
                    grad_rect(ui.painter(), fill, 4.0_f32.min(fill.width() / 2.0), ACCENT, ACCENT2);
                }
                ui.painter().text(pos2(c.x, track.bottom() + 12.0), Align2::CENTER_TOP, format!("{}%", (pct * 100.0).round()), font(12.0), MUTED);
                ctx.request_repaint_after(Duration::from_millis(33));
            }
            Screen::Done => {
                let c = pos2(full.center().x, body.top() + 70.0);
                ui.painter().circle_filled(c, 34.0, ACCENT.gamma_multiply(0.16));
                ui.painter().add(egui::Shape::line(
                    vec![c + vec2(-13.0, 1.0), c + vec2(-4.0, 10.0), c + vec2(14.0, -9.0)],
                    Stroke::new(3.2, ACCENT),
                ));
                let (h, sub) = if self.mode == Mode::Install {
                    ("Goatr is ready", "Find it in the Start menu any time.")
                } else {
                    ("Goatr was removed", "Thanks for using Goatr.")
                };
                ui.painter().text(pos2(c.x, c.y + 54.0), Align2::CENTER_TOP, h, bold(22.0), TEXT);
                ui.painter().text(pos2(c.x, c.y + 88.0), Align2::CENTER_TOP, sub, font(13.0), MUTED);
                if self.mode == Mode::Install {
                    if self.button(ui, ghost_r, "Close", 0, true) {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if self.button(ui, primary_r, "Open Goatr", 1, true) {
                        let dir = PathBuf::from(self.dir.trim());
                        let _ = std::process::Command::new(dir.join(EXE)).current_dir(&dir).spawn();
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                } else if self.button(ui, primary_r, "Close", 1, true) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Screen::Error(msg) => {
                let c = pos2(full.center().x, body.top() + 70.0);
                ui.painter().circle_filled(c, 34.0, DANGER.gamma_multiply(0.16));
                ui.painter().line_segment([c + vec2(0.0, -12.0), c + vec2(0.0, 4.0)], Stroke::new(3.2, DANGER));
                ui.painter().circle_filled(c + vec2(0.0, 12.0), 2.0, DANGER);
                ui.painter().text(pos2(c.x, c.y + 54.0), Align2::CENTER_TOP, "Something got in the way", bold(22.0), TEXT);
                let galley = ui.painter().layout(msg.clone(), font(13.0), MUTED, body.width() - 40.0);
                let gpos = pos2(c.x - galley.size().x / 2.0, c.y + 90.0);
                ui.painter().galley(gpos, galley, MUTED);
                if self.button(ui, ghost_r, "Close", 0, true) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if self.button(ui, primary_r, "Try again", 1, true) {
                    self.screen = if self.mode == Mode::Install { Screen::Welcome } else { Screen::Confirm };
                    if self.mode == Mode::Install && !matches!(self.shared.lock().unwrap().release, Some(Ok(_))) {
                        fetch_release(self.shared.clone(), ctx.clone());
                    }
                }
            }
        }
        // Thin border around the frameless window.
        ui.painter().rect_stroke(full, CornerRadius::ZERO, Stroke::new(1.0, LINE), StrokeKind::Inside);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.063, 0.071, 0.078, 1.0]
    }
}
