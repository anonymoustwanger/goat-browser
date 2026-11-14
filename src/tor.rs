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
                } else {
                    self.set("starting", pct, format!("Connecting to Tor — {what}"), notify);
                }
            }
            if line.contains("[err]") || line.contains("[warn]") {
                last = line.clone();
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
