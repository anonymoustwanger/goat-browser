//! One Goat per profile: a link opened from another app while Goat is running becomes a tab in
//! the open window instead of a new window. The running window listens on a named pipe; a new
//! Goat started with an address hands it over and exits.
//!
//! The same pipe lets a no-hardware-acceleration window share cookies with the normal window
//! (the two can't use one cookie store): it fetches the normal window's cookies when it opens,
//! and sends its own back while it runs and when it closes.

/// Pipe name for this Windows user and profile.
fn pipe_name(profile: &str) -> String {
    let user: String = std::env::var("USERNAME").unwrap_or_default().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let prof: String = if profile.is_empty() { "default".into() } else { profile.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect() };
    format!(r"\\.\pipe\Goat-{user}-{prof}")
}

/// What a client asked the running window for.
pub enum Request {
    Open(String),
    /// Send back all cookies (JSON list of CookieInfo) on the channel.
    CookiesGet(std::sync::mpsc::Sender<String>),
    CookiesPut(Vec<crate::shared::CookieInfo>),
}

#[cfg(windows)]
fn connect(profile: &str) -> Option<std::fs::File> {
    let name = pipe_name(profile);
    for _ in 0..10 {
        match std::fs::OpenOptions::new().read(true).write(true).open(&name) {
            Ok(f) => return Some(f),
            // All pipe instances busy for a moment: try again.
            Err(e) if e.raw_os_error() == Some(231) => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(_) => return None,
        }
    }
    None
}

/// The normal window's cookies (None when it isn't running).
#[cfg(windows)]
pub fn cookies_get(profile: &str) -> Option<Vec<crate::shared::CookieInfo>> {
    use std::io::{Read, Write};
    let mut f = connect(profile)?;
    f.write_all(b"cookies-get\n").ok()?;
    let mut out = Vec::new();
    f.read_to_end(&mut out).ok()?;
    serde_json::from_slice(&out).ok()
}

/// Give the normal window this window's cookies.
#[cfg(windows)]
pub fn cookies_put(profile: &str, list: &[crate::shared::CookieInfo]) -> bool {
    use std::io::Write;
    let Some(mut f) = connect(profile) else { return false };
    let body = serde_json::to_vec(list).unwrap_or_default();
    f.write_all(b"cookies-put\n").is_ok() && f.write_all(&body).is_ok()
}

/// Give `url` to a Goat already running this profile. True if one took it.
#[cfg(windows)]
pub fn forward(profile: &str, url: &str) -> bool {
    use std::io::Write;
    let Some(mut f) = connect(profile) else { return false };
    // Let the running window come to the front.
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(windows::Win32::UI::WindowsAndMessaging::ASFW_ANY);
    }
    f.write_all(format!("open\n{url}").as_bytes()).is_ok()
}

/// Listen for addresses from new Goat processes (on a background thread).
#[cfg(windows)]
pub fn listen(profile: &str, on_req: impl Fn(Request) + Send + 'static) {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{FlushFileBuffers, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
    use windows::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT};
    let name = HSTRING::from(pipe_name(profile));
    std::thread::Builder::new()
        .name("goat-instance".into())
        .spawn(move || loop {
            let pipe = unsafe {
                CreateNamedPipeW(
                    &name,
                    PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    1,
                    1 << 20,
                    1 << 20,
                    0,
                    None,
                )
            };
            if pipe == INVALID_HANDLE_VALUE || pipe.is_invalid() {
                // Another Goat window of this profile is listening: take over if it closes.
                std::thread::sleep(std::time::Duration::from_secs(3));
                continue;
            }
            let connected = unsafe { ConnectNamedPipe(pipe, None) }.is_ok() || std::io::Error::last_os_error().raw_os_error() == Some(535);
            if connected {
                let mut data = Vec::new();
                let mut buf = [0u8; 64 * 1024];
                let mut answered = false;
                loop {
                    let mut n = 0u32;
                    if unsafe { ReadFile(pipe, Some(&mut buf), Some(&mut n), None) }.is_err() || n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n as usize]);
                    // "cookies-get" waits for an answer, so it can't wait for the client to finish.
                    if data.starts_with(b"cookies-get\n") {
                        let (tx, rx) = std::sync::mpsc::channel();
                        on_req(Request::CookiesGet(tx));
                        let json = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap_or_else(|_| "[]".into());
                        let mut written = 0u32;
                        unsafe {
                            let _ = WriteFile(pipe, Some(json.as_bytes()), Some(&mut written), None);
                            let _ = FlushFileBuffers(pipe);
                        }
                        answered = true;
                        break;
                    }
                    if data.len() > 32 << 20 {
                        break;
                    }
                }
                if !answered {
                    if let Some(url) = data.strip_prefix(b"open\n") {
                        let url = String::from_utf8_lossy(url).trim().to_string();
                        if !url.is_empty() {
                            on_req(Request::Open(url));
                        }
                    } else if let Some(body) = data.strip_prefix(b"cookies-put\n") {
                        if let Ok(list) = serde_json::from_slice(body) {
                            on_req(Request::CookiesPut(list));
                        }
                    }
                }
                unsafe {
                    let _ = DisconnectNamedPipe(pipe);
                }
            }
            unsafe {
                let _ = CloseHandle(pipe);
            }
        })
        .ok();
}

/// Each window's own pipe (by process id), for tabs dragged in from another Goat window.
fn pid_pipe(pid: u32) -> String {
    let user: String = std::env::var("USERNAME").unwrap_or_default().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    format!(r"\\.\pipe\Goat-{user}-window-{pid}")
}

/// Hand a tab to the Goat window in process `pid`. True if it took it (same profile and kind).
#[cfg(windows)]
pub fn send_tab(pid: u32, kind: &str, profile: &str, url: &str) -> bool {
    use std::io::{Read, Write};
    let Ok(mut f) = std::fs::OpenOptions::new().read(true).write(true).open(pid_pipe(pid)) else { return false };
    if f.write_all(format!("tab\n{kind}\n{profile}\n{url}\n").as_bytes()).is_err() {
        return false;
    }
    let mut out = String::new();
    let _ = f.read_to_string(&mut out);
    out.trim() == "ok"
}

/// Accept tabs dragged from other Goat windows: `accept(kind, profile, url)` says yes or no.
#[cfg(windows)]
pub fn listen_window(accept: impl Fn(String, String, String) -> bool + Send + 'static) {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{FlushFileBuffers, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
    use windows::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT};
    let name = HSTRING::from(pid_pipe(std::process::id()));
    std::thread::Builder::new()
        .name("goat-window-pipe".into())
        .spawn(move || loop {
            let pipe = unsafe {
                CreateNamedPipeW(&name, PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, 1, 4096, 64 * 1024, 0, None)
            };
            if pipe == INVALID_HANDLE_VALUE || pipe.is_invalid() {
                return;
            }
            let connected = unsafe { ConnectNamedPipe(pipe, None) }.is_ok() || std::io::Error::last_os_error().raw_os_error() == Some(535);
            if connected {
                let mut data = Vec::new();
                let mut buf = [0u8; 8192];
                while data.iter().filter(|b| **b == b'\n').count() < 4 && data.len() < 64 * 1024 {
                    let mut n = 0u32;
                    if unsafe { ReadFile(pipe, Some(&mut buf), Some(&mut n), None) }.is_err() || n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n as usize]);
                }
                let text = String::from_utf8_lossy(&data).into_owned();
                let mut lines = text.split('\n');
                let ok = match (lines.next(), lines.next(), lines.next(), lines.next()) {
                    (Some("tab"), Some(kind), Some(profile), Some(url)) if !url.is_empty() => accept(kind.into(), profile.into(), url.into()),
                    _ => false,
                };
                let reply: &[u8] = if ok { b"ok" } else { b"no" };
                let mut w = 0u32;
                unsafe {
                    let _ = WriteFile(pipe, Some(reply), Some(&mut w), None);
                    let _ = FlushFileBuffers(pipe);
                    let _ = DisconnectNamedPipe(pipe);
                }
            }
            unsafe {
                let _ = CloseHandle(pipe);
            }
        })
        .ok();
}

/// The Goat window process under a screen point, if it's another Goat window.
#[cfg(windows)]
pub fn window_pid_at(x: f32, y: f32) -> Option<u32> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GetWindowThreadProcessId, WindowFromPoint, GA_ROOT};
    unsafe {
        let h = WindowFromPoint(POINT { x: x as i32, y: y as i32 });
        if h.is_invalid() {
            return None;
        }
        let root = GetAncestor(h, GA_ROOT);
        let mut pid = 0u32;
        GetWindowThreadProcessId(if root.is_invalid() { h } else { root }, Some(&mut pid));
        (pid != 0).then_some(pid)
    }
}
