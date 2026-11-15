//! Tor support. On first use Goatr downloads the official Tor Expert Bundle from torproject.org
//! into `tor\`, runs tor.exe in the background, and routes Tor tabs through it (SOCKS5).

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::net;
use crate::store::app_dir;

pub const SOCKS_PORT: u16 = 9155;
/// Tor's control port (local only, cookie-authenticated): used to show a site's circuit.
pub const CONTROL_PORT: u16 = 9156;

/// One relay in a circuit, as shown in the Tor circuit panel.
#[derive(Debug, Clone, Default)]
pub struct Hop {
    pub nick: String,
    pub ip: String,
    /// Two-letter country code ("de"), or "" if unknown.
    pub country: String,
}

/// Extra Chromium switches for Tor tabs: everything over the SOCKS proxy, no local DNS, no WebRTC IP leaks.
pub fn browser_args() -> String {
    format!(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
         --proxy-server=socks5://127.0.0.1:{SOCKS_PORT} \
         --host-resolver-rules=\"MAP * ~NOTFOUND , EXCLUDE 127.0.0.1\" \
         --force-webrtc-ip-handling-policy=disable_non_proxied_udp \
         --webrtc-ip-handling-policy=disable_non_proxied_udp \
         --disable-quic"
    )
}

#[derive(Debug, Clone, Serialize)]
pub struct TorState {
    /// "off" | "downloading" | "starting" | "ready" | "error"
    pub phase: String,
    pub progress: u8,
    pub message: String,
}

pub struct Tor {
    state: Mutex<TorState>,
    child: Mutex<Option<Child>>,
    /// Site host → the circuit its last connection used (from Tor's stream events).
    streams: Arc<Mutex<std::collections::HashMap<String, String>>>,
}

fn tor_dir() -> PathBuf {
    app_dir().join("tor")
}

fn tor_exe() -> PathBuf {
    tor_dir().join("tor").join(if cfg!(windows) { "tor.exe" } else { "tor" })
}

impl Tor {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(TorState { phase: "off".into(), progress: 0, message: "Not running".into() }),
            child: Mutex::new(None),
            streams: Default::default(),
        })
    }

    pub fn state(&self) -> TorState {
        self.state.lock().unwrap().clone()
    }

    pub fn ready(&self) -> bool {
        self.state.lock().unwrap().phase == "ready"
    }

    fn set(&self, phase: &str, progress: u8, message: impl Into<String>, notify: &(dyn Fn() + Send + Sync)) {
        *self.state.lock().unwrap() = TorState { phase: phase.into(), progress, message: message.into() };
        notify();
    }

    /// Start Tor if it isn't already running or starting. `notify` is called on every state change.
    pub fn start(self: &Arc<Self>, notify: Arc<dyn Fn() + Send + Sync>) {
        {
            let mut s = self.state.lock().unwrap();
            if matches!(s.phase.as_str(), "downloading" | "starting" | "ready") {
                return;
            }
            *s = TorState { phase: "starting".into(), progress: 0, message: "Starting Tor…".into() };
        }
        notify();
        let me = self.clone();
        std::thread::spawn(move || {
            if let Err(e) = me.run(notify.as_ref()) {
                me.set("error", 0, e, notify.as_ref());
            }
        });
    }

    pub fn stop(&self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
        }
        *self.state.lock().unwrap() = TorState { phase: "off".into(), progress: 0, message: "Not running".into() };
    }

    fn run(&self, notify: &(dyn Fn() + Send + Sync)) -> Result<(), String> {
        if !tor_exe().exists() {
            self.download(notify)?;
        }
        for attempt in 0..8 {
            // Tor already running for another Goatr window: share it.
            if Control::open().is_ok() {
                self.set("ready", 100, "Connected to Tor", notify);
                crate::crashlog::debug("tor: sharing the Tor already running");
                watch_loop(self.streams.clone(), true);
                self.set("off", 0, "Tor stopped", notify);
                return Ok(());
            }
            match self.spawn_tor(notify) {
                // A Tor from a Goatr window that just closed still holds its folder: it exits
                // within a few seconds, so wait for it.
                Err(e) if e.contains("another Tor process") && attempt < 7 => {
                    self.set("starting", 0, "Waiting for the previous Tor to close…", notify);
                    std::thread::sleep(std::time::Duration::from_secs(3));
                }
                r => return r,
            }
        }
        Err("Tor is still in use by something else".into())
    }

    fn spawn_tor(&self, notify: &(dyn Fn() + Send + Sync)) -> Result<(), String> {
        self.set("starting", 0, "Connecting to the Tor network…", notify);
        let data = tor_dir().join("state");
        let _ = std::fs::create_dir_all(&data);
        let mut cmd = Command::new(tor_exe());
        cmd.arg("--SocksPort")
            .arg(format!("127.0.0.1:{SOCKS_PORT}"))
            .arg("--DataDirectory")
            .arg(&data)
            .arg("--ClientOnly")
            .arg("1")
            // Tor exits by itself if Goatr closes or crashes.
            .arg("--__OwningControllerProcess")
            .arg(std::process::id().to_string())
            .arg("--ControlPort")
            .arg(format!("127.0.0.1:{CONTROL_PORT}"))
            .arg("--CookieAuthentication")
            .arg("1")
            .arg("--Log")
            .arg("notice stdout");
        let geoip = tor_dir().join("data").join("geoip");
        if geoip.exists() {
            cmd.arg("--GeoIPFile").arg(&geoip).arg("--GeoIPv6File").arg(tor_dir().join("data").join("geoip6"));
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn().map_err(|e| format!("Couldn't start tor.exe: {e}"))?;
        let stdout = child.stdout.take().ok_or("no tor output")?;
        *self.child.lock().unwrap() = Some(child);

        let mut last = String::new();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(i) = line.find("Bootstrapped ") {
                let rest = &line[i + 13..];
                let pct: u8 = rest.split('%').next().and_then(|p| p.trim().parse().ok()).unwrap_or(0);
                let what = rest.split_once('(').and_then(|(_, r)| r.split_once("): ")).map(|(_, m)| m).unwrap_or(rest);
                if pct >= 100 {
                    self.set("ready", 100, "Connected to Tor", notify);
                    self.watch_streams();
                } else {
                    self.set("starting", pct, format!("Connecting to Tor — {what}"), notify);
                }
            }
            if line.contains("[err]") || line.contains("[warn]") {
                crate::crashlog::debug(format!("tor: {line:.200}"));
                // Keep the warning that explains an error ("see warnings above").
                if !(line.contains("[err]") && line.contains("see warnings above") && !last.is_empty()) {
                    last = line.clone();
                }
            }
        }
        // stdout closed: tor exited.
        let was_ready = self.ready();
        self.child.lock().unwrap().take();
        if was_ready {
            self.set("off", 0, "Tor stopped", notify);
            Ok(())
        } else {
            Err(if last.is_empty() { "Tor exited unexpectedly".into() } else { format!("Tor exited: {last}") })
        }
    }

    fn download(&self, notify: &(dyn Fn() + Send + Sync)) -> Result<(), String> {
        if !cfg!(windows) {
            return Err("Automatic Tor download is only set up for Windows".into());
        }
        self.set("downloading", 0, "Finding the latest Tor release…", notify);
        let info = net::get_text("https://aus1.torproject.org/torbrowser/update_3/release/downloads.json")?;
        let v: serde_json::Value = serde_json::from_str(&info).map_err(|e| e.to_string())?;
        let ver = v["version"].as_str().ok_or("Couldn't read the Tor version")?.to_string();
        let file = format!("tor-expert-bundle-windows-x86_64-{ver}.tar.gz");
        let urls = [
            format!("https://dist.torproject.org/torbrowser/{ver}/{file}"),
            format!("https://archive.torproject.org/tor-package-archive/torbrowser/{ver}/{file}"),
        ];
        let mut bytes = None;
        let mut err = String::new();
        for u in &urls {
            match net::get_bytes(u, 200 << 20, |got, total| {
                if let Some(t) = total.filter(|t| *t > 0) {
                    let pct = (got * 100 / t).min(100) as u8;
                    // Throttle UI updates to whole-percent changes.
                    let cur = self.state.lock().unwrap().progress;
                    if pct != cur {
                        self.set("downloading", pct, format!("Downloading Tor {ver}… {pct}%"), notify);
                    }
                }
            }) {
                Ok(b) => {
                    bytes = Some(b);
                    break;
                }
                Err(e) => err = e,
            }
        }
        let bytes = bytes.ok_or_else(|| format!("Couldn't download Tor: {err}"))?;
        self.set("downloading", 100, "Unpacking Tor…", notify);
        let tmp = tor_dir().with_extension("unpack");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        tar::Archive::new(flate2::read::GzDecoder::new(&bytes[..]))
            .unpack(&tmp)
            .map_err(|e| format!("Couldn't unpack Tor: {e}"))?;
        std::fs::create_dir_all(tor_dir()).map_err(|e| e.to_string())?;
        for sub in ["tor", "data"] {
            let from = tmp.join(sub);
            if from.exists() {
                let to = tor_dir().join(sub);
                let _ = std::fs::remove_dir_all(&to);
                std::fs::rename(&from, &to).map_err(|e| e.to_string())?;
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
        if !tor_exe().exists() {
            return Err("The Tor download didn't contain tor.exe".into());
        }
        Ok(())
    }
}

impl Drop for Tor {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
        }
    }
}

// ------------------------------------------------------------------ control port

/// Follow Tor's STREAM events into `streams` until the connection ends. With `own`, this
/// connection also keeps the (shared) Tor alive for this window and ends it when Goatr exits.
fn watch_loop(streams: Arc<Mutex<std::collections::HashMap<String, String>>>, own: bool) {
    let mut c = match Control::open() {
        Ok(c) => c,
        Err(e) => {
            crate::crashlog::debug(format!("tor streams: can't open control port: {e}"));
            return;
        }
    };
    let _ = c.r.get_ref().set_read_timeout(None);
    if own {
        let _ = c.cmd("TAKEOWNERSHIP");
        let _ = c.cmd("RESETCONF __OwningControllerProcess");
    }
    if let Err(e) = c.cmd("SETEVENTS STREAM") {
        crate::crashlog::debug(format!("tor streams: SETEVENTS failed: {e}"));
        return;
    }
    crate::crashlog::debug("tor streams: watching");
    // Tor reports the site's name when a connection starts (NEW) but only the IP once it's up
    // (SUCCEEDED), so remember each connection's site by its id.
    let mut names: std::collections::HashMap<String, String> = Default::default();
    loop {
        let mut line = String::new();
        if c.r.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        // 650 STREAM <id> <status> <circuit> <host>:<port> …
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 6 || f[0] != "650" || f[1] != "STREAM" {
            continue;
        }
        let target = f[5].rsplit_once(':').map(|(h, _)| h).unwrap_or(f[5]).to_lowercase();
        match f[3] {
            "NEW" | "NEWRESOLVE" => {
                names.insert(f[2].to_string(), target);
            }
            "SUCCEEDED" if f[4] != "0" => {
                let host = names.get(f[2]).cloned().unwrap_or(target);
                streams.lock().unwrap().insert(host, f[4].to_string());
            }
            "CLOSED" | "FAILED" => {
                names.remove(f[2]);
            }
            _ => {}
        }
        if names.len() > 4000 {
            names.clear();
        }
    }
}

/// A logged-in connection to Tor's control port.
struct Control {
    r: BufReader<std::net::TcpStream>,
    w: std::net::TcpStream,
}

impl Control {
    fn open() -> Result<Control, String> {
        use std::io::Write;
        let s = std::net::TcpStream::connect(("127.0.0.1", CONTROL_PORT)).map_err(|e| format!("Tor isn't answering ({e})"))?;
        s.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok();
        let cookie = std::fs::read(tor_dir().join("state").join("control_auth_cookie")).map_err(|_| "Tor's login cookie is missing")?;
        let hex: String = cookie.iter().map(|b| format!("{b:02X}")).collect();
        let mut c = Control { r: BufReader::new(s.try_clone().map_err(|e| e.to_string())?), w: s };
        c.w.write_all(format!("AUTHENTICATE {hex}\r\n").as_bytes()).map_err(|e| e.to_string())?;
        c.reply()?;
        Ok(c)
    }

    /// Lines of one reply (without the status codes), or the error.
    fn reply(&mut self) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        loop {
            let mut line = String::new();
            if self.r.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                return Err("Tor closed the connection".into());
            }
            let line = line.trim_end_matches(['\r', '\n']).to_string();
            if line.len() < 4 {
                continue;
            }
            let (code, sep, rest) = (&line[..3], &line[3..4], line[4..].to_string());
            if !code.starts_with('2') && !code.starts_with('6') {
                return Err(format!("Tor said: {rest}"));
            }
            match sep {
                " " => {
                    out.push(rest);
                    return Ok(out);
                }
                "+" => {
                    // Multi-line data until a lone ".".
                    out.push(rest);
                    loop {
                        let mut l = String::new();
                        if self.r.read_line(&mut l).map_err(|e| e.to_string())? == 0 {
                            return Ok(out);
                        }
                        let l = l.trim_end_matches(['\r', '\n']);
                        if l == "." {
                            break;
                        }
                        out.push(l.to_string());
                    }
                }
                _ => out.push(rest),
            }
        }
    }

    fn cmd(&mut self, c: &str) -> Result<Vec<String>, String> {
        use std::io::Write;
        self.w.write_all(format!("{c}\r\n").as_bytes()).map_err(|e| e.to_string())?;
        self.reply()
    }
}

impl Tor {
    /// Keep track of which circuit each site's connections use (Tor's STREAM events).
    fn watch_streams(&self) {
        let streams = self.streams.clone();
        let _ = std::thread::Builder::new().name("tor-streams".into()).spawn(move || watch_loop(streams, false));
    }

    /// The relays (entry → middle → exit) Tor uses for `host`.
    pub fn circuit_for(&self, host: &str) -> Result<Vec<Hop>, String> {
        if !self.ready() {
            return Err("Tor isn't connected yet".into());
        }
        let host = host.to_lowercase();
        let mut c = Control::open()?;
        let mut circ = self.streams.lock().unwrap().get(&host).cloned();
        if circ.is_none() {
            // Not seen yet: look at the connections that are open right now.
            for l in c.cmd("GETINFO stream-status")? {
                let f: Vec<&str> = l.trim_start_matches("stream-status=").split_whitespace().collect();
                if f.len() >= 4 && f[1] == "SUCCEEDED" && f[3].to_lowercase().starts_with(&format!("{host}:")) {
                    circ = Some(f[2].to_string());
                }
            }
        }
        if circ.is_none() {
            let known: Vec<String> = self.streams.lock().unwrap().keys().take(12).cloned().collect();
            crate::crashlog::debug(format!("tor circuit: nothing for {host}; known sites {known:?}"));
        }
        let circ = circ.ok_or("No Tor connection to this site yet. Reload the page and try again.")?;
        let status = c.cmd("GETINFO circuit-status")?;
        let line = status
            .iter()
            .map(|l| l.trim_start_matches("circuit-status="))
            .find(|l| l.split_whitespace().next() == Some(circ.as_str()))
            .ok_or("That connection's circuit has closed. Reload the page.")?
            .to_string();
        let path = line.split_whitespace().nth(2).unwrap_or("");
        let mut hops = Vec::new();
        for relay in path.split(',').filter(|r| !r.is_empty()) {
            let fp = relay.trim_start_matches('$').split(['~', '=']).next().unwrap_or("").to_string();
            let nick = relay.split(['~', '=']).nth(1).unwrap_or("").to_string();
            let ip = c
                .cmd(&format!("GETINFO ns/id/{fp}"))
                .ok()
                .and_then(|ls| ls.iter().find(|l| l.starts_with("r ")).and_then(|l| l.split_whitespace().nth(6).map(String::from)))
                .unwrap_or_default();
            let country = if ip.is_empty() {
                String::new()
            } else {
                c.cmd(&format!("GETINFO ip-to-country/{ip}"))
                    .ok()
                    .and_then(|ls| ls.first().and_then(|l| l.split('=').nth(1).map(|v| v.trim().to_string())))
                    .filter(|v| v != "??")
                    .unwrap_or_default()
            };
            hops.push(Hop { nick, ip, country });
        }
        Ok(hops)
    }

    /// New circuits for everything from now on (Tor's "new identity" signal).
    pub fn new_circuits(&self) -> Result<(), String> {
        let mut c = Control::open()?;
        c.cmd("SIGNAL NEWNYM").map(|_| ())?;
        self.streams.lock().unwrap().clear();
        Ok(())
    }
}

/// Country name for a two-letter code (the common ones), or the code itself.
pub fn country_name(code: &str) -> String {
    let n = match code.to_lowercase().as_str() {
        "us" => "United States", "de" => "Germany", "nl" => "Netherlands", "fr" => "France", "gb" => "United Kingdom",
        "ca" => "Canada", "se" => "Sweden", "ch" => "Switzerland", "fi" => "Finland", "at" => "Austria",
        "ro" => "Romania", "pl" => "Poland", "cz" => "Czechia", "lu" => "Luxembourg", "no" => "Norway",
        "dk" => "Denmark", "is" => "Iceland", "es" => "Spain", "it" => "Italy", "be" => "Belgium",
        "ie" => "Ireland", "pt" => "Portugal", "lt" => "Lithuania", "lv" => "Latvia", "ee" => "Estonia",
        "ua" => "Ukraine", "bg" => "Bulgaria", "hu" => "Hungary", "gr" => "Greece", "sk" => "Slovakia",
        "si" => "Slovenia", "hr" => "Croatia", "rs" => "Serbia", "md" => "Moldova", "ru" => "Russia",
        "jp" => "Japan", "sg" => "Singapore", "hk" => "Hong Kong", "au" => "Australia", "nz" => "New Zealand",
        "br" => "Brazil", "in" => "India", "kr" => "South Korea", "tw" => "Taiwan", "il" => "Israel",
        "za" => "South Africa", "ar" => "Argentina", "mx" => "Mexico", "cl" => "Chile", "tr" => "Turkey",
        "sc" => "Seychelles", "pa" => "Panama", "cy" => "Cyprus", "mt" => "Malta", "li" => "Liechtenstein",
        "" => "Unknown location",
        _ => return code.to_uppercase(),
    };
    n.to_string()
}
