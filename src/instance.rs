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

/// What another Goat window sent to this window's own pipe.
pub enum WindowReq {
    /// A dragged-in tab: (kind, profile, address). Answer: take it?
    Tab(String, String, String),
    /// A window this one opened for a dragged-out tab is now on screen.
    Ready,
}

/// Accept tabs dragged from other Goat windows, and "ready" from torn-off windows.
#[cfg(windows)]
pub fn listen_window(on_req: impl Fn(WindowReq) -> bool + Send + 'static) {
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
                loop {
                    let lines = data.iter().filter(|b| **b == b'\n').count();
                    if (data.starts_with(b"ready\n")) || lines >= 4 || data.len() > 64 * 1024 {
                        break;
                    }
                    let mut n = 0u32;
                    if unsafe { ReadFile(pipe, Some(&mut buf), Some(&mut n), None) }.is_err() || n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n as usize]);
                }
                let text = String::from_utf8_lossy(&data).into_owned();
                let mut lines = text.split('\n');
                let ok = match (lines.next(), lines.next(), lines.next(), lines.next()) {
                    (Some("ready"), ..) => on_req(WindowReq::Ready),
                    (Some("tab"), Some(kind), Some(profile), Some(url)) if !url.is_empty() => on_req(WindowReq::Tab(kind.into(), profile.into(), url.into())),
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

/// Tell the window in process `pid` (which dragged out a tab) that this new window is showing.
#[cfg(windows)]
pub fn send_ready(pid: u32) {
    use std::io::{Read, Write};
    if let Ok(mut f) = std::fs::OpenOptions::new().read(true).write(true).open(pid_pipe(pid)) {
        let _ = f.write_all(b"ready\n");
        let mut s = String::new();
        let _ = f.read_to_string(&mut s);
    }
}

// ------------------------------------------------------------ standby window
// A hidden Goat window kept ready (per profile), so a tab dragged out of the strip becomes a
// window instantly instead of waiting for a new Goat to start.

fn standby_pipe(profile: &str) -> String {
    pipe_name(profile).replacen("Goat-", "Goat-standby-", 1)
}

/// Is a pipe there (busy or free)?
#[cfg(windows)]
fn pipe_exists(name: &str) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::System::Pipes::WaitNamedPipeW;
    unsafe {
        if WaitNamedPipeW(&HSTRING::from(name), 1).as_bool() {
            return true;
        }
    }
    // 2 = no such pipe; anything else (e.g. timed out because busy) means it's there.
    std::io::Error::last_os_error().raw_os_error() != Some(2)
}

#[cfg(windows)]
pub fn standby_alive(profile: &str) -> bool {
    pipe_exists(&standby_pipe(profile))
}

/// Hand a tab to the standby window: it shows itself at `rect` (x, y, w, h in screen points)
/// with the tab. True once it's on screen.
#[cfg(windows)]
pub fn standby_adopt(profile: &str, url: &str, rect: [f32; 4]) -> bool {
    use std::io::{Read, Write};
    let Ok(mut f) = std::fs::OpenOptions::new().read(true).write(true).open(standby_pipe(profile)) else { return false };
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(windows::Win32::UI::WindowsAndMessaging::ASFW_ANY);
    }
    if f.write_all(format!("adopt\n{url}\n{},{},{},{}\n", rect[0], rect[1], rect[2], rect[3]).as_bytes()).is_err() {
        return false;
    }
    let mut out = String::new();
    let _ = f.read_to_string(&mut out);
    out.trim() == "ok"
}

/// Close the standby window (the setting was turned off).
#[cfg(windows)]
pub fn standby_quit(profile: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(standby_pipe(profile)) {
        let _ = f.write_all(b"quit\n");
    }
}

pub enum StandbyReq {
    /// Show with this tab at (x, y, w, h); answer on the channel once showing.
    Adopt(String, [f32; 4], std::sync::mpsc::Sender<bool>),
    Quit,
}

/// Claim the standby pipe for this profile. None if another standby already has it.
#[cfg(windows)]
pub fn claim_standby(profile: &str) -> Option<windows::Win32::Foundation::HANDLE> {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
    use windows::Win32::System::Pipes::{CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT};
    let h = unsafe {
        CreateNamedPipeW(&HSTRING::from(standby_pipe(profile)), PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, 1, 4096, 64 * 1024, 0, None)
    };
    (!h.is_invalid() && h != windows::Win32::Foundation::INVALID_HANDLE_VALUE).then_some(h)
}

/// Serve the claimed standby pipe until a tab is adopted (or Quit).
#[cfg(windows)]
pub fn serve_standby(pipe: windows::Win32::Foundation::HANDLE, on_req: impl Fn(StandbyReq) + Send + 'static) {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Storage::FileSystem::{FlushFileBuffers, ReadFile, WriteFile};
    use windows::Win32::System::Pipes::{ConnectNamedPipe, DisconnectNamedPipe};
    let raw = pipe.0 as usize;
    std::thread::Builder::new()
        .name("goat-standby".into())
        .spawn(move || {
            let pipe = HANDLE(raw as *mut _);
            loop {
                let connected = unsafe { ConnectNamedPipe(pipe, None) }.is_ok() || std::io::Error::last_os_error().raw_os_error() == Some(535);
                if !connected {
                    break;
                }
                let mut data = Vec::new();
                let mut buf = [0u8; 8192];
                while !(data.starts_with(b"quit\n") || data.iter().filter(|b| **b == b'\n').count() >= 3) && data.len() < 64 * 1024 {
                    let mut n = 0u32;
                    if unsafe { ReadFile(pipe, Some(&mut buf), Some(&mut n), None) }.is_err() || n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n as usize]);
                }
                let text = String::from_utf8_lossy(&data).into_owned();
                let mut lines = text.split('\n');
                let mut done = false;
                let reply: &[u8] = match (lines.next(), lines.next(), lines.next()) {
                    (Some("quit"), ..) => {
                        on_req(StandbyReq::Quit);
                        done = true;
                        b"ok"
                    }
                    (Some("adopt"), Some(url), Some(r)) => {
                        let v: Vec<f32> = r.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                        if v.len() == 4 && !url.is_empty() {
                            let (tx, rx) = std::sync::mpsc::channel();
                            on_req(StandbyReq::Adopt(url.to_string(), [v[0], v[1], v[2], v[3]], tx));
                            done = rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap_or(false);
                            if done { b"ok" } else { b"no" }
                        } else {
                            b"no"
                        }
                    }
                    _ => b"no",
                };
                let mut w = 0u32;
                unsafe {
                    let _ = WriteFile(pipe, Some(reply), Some(&mut w), None);
                    let _ = FlushFileBuffers(pipe);
                    let _ = DisconnectNamedPipe(pipe);
                }
                if done {
                    break;
                }
            }
            unsafe {
                let _ = CloseHandle(pipe);
            }
        })
        .ok();
}

/// The process of the top window under a screen point (Goat's drag preview doesn't count).
#[cfg(windows)]
pub fn window_pid_at(x: f32, y: f32) -> Option<u32> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible};
    struct Find {
        x: i32,
        y: i32,
        pid: u32,
    }
    unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> BOOL {
        let f = &mut *(lp.0 as *mut Find);
        if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() {
            return true.into();
        }
        let mut r = RECT::default();
        if GetWindowRect(h, &mut r).is_err() || f.x < r.left || f.x >= r.right || f.y < r.top || f.y >= r.bottom {
            return true.into();
        }
        let mut buf = [0u16; 64];
        let n = GetWindowTextW(h, &mut buf) as usize;
        if String::from_utf16_lossy(&buf[..n]) == crate::DRAG_PREVIEW_TITLE {
            return true.into();
        }
        GetWindowThreadProcessId(h, Some(&mut f.pid));
        false.into() // topmost hit: stop
    }
    let mut f = Find { x: x as i32, y: y as i32, pid: 0 };
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut f as *mut Find as isize));
    }
    (f.pid != 0).then_some(f.pid)
}
