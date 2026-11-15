//! One Goat per profile: a link opened from another app while Goat is running becomes a tab in
//! the open window instead of a new window. The running window listens on a named pipe; a new
//! Goat started with an address hands it over and exits.

/// Pipe name for this Windows user and profile.
fn pipe_name(profile: &str) -> String {
    let user: String = std::env::var("USERNAME").unwrap_or_default().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let prof: String = if profile.is_empty() { "default".into() } else { profile.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect() };
    format!(r"\\.\pipe\Goat-{user}-{prof}")
}

/// Give `url` to a Goat already running this profile. True if one took it.
#[cfg(windows)]
pub fn forward(profile: &str, url: &str) -> bool {
    use std::io::Write;
    let name = pipe_name(profile);
    for _ in 0..10 {
        match std::fs::OpenOptions::new().write(true).open(&name) {
            Ok(mut f) => {
                // Let the running window come to the front.
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(windows::Win32::UI::WindowsAndMessaging::ASFW_ANY);
                }
                return f.write_all(format!("open\n{url}").as_bytes()).is_ok();
            }
            // All pipe instances busy for a moment: try again.
            Err(e) if e.raw_os_error() == Some(231) => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(_) => return false,
        }
    }
    false
}

/// Listen for addresses from new Goat processes (on a background thread).
#[cfg(windows)]
pub fn listen(profile: &str, on_url: impl Fn(String) + Send + 'static) {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{ReadFile, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND};
    use windows::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT};
    let name = HSTRING::from(pipe_name(profile));
    std::thread::Builder::new()
        .name("goat-instance".into())
        .spawn(move || loop {
            let pipe = unsafe {
                CreateNamedPipeW(
                    &name,
                    PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    1,
                    0,
                    64 * 1024,
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
                let mut buf = [0u8; 8192];
                loop {
                    let mut n = 0u32;
                    if unsafe { ReadFile(pipe, Some(&mut buf), Some(&mut n), None) }.is_err() || n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n as usize]);
                    if data.len() > 64 * 1024 {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&data).into_owned();
                if let Some(url) = text.strip_prefix("open\n") {
                    if !url.trim().is_empty() {
                        on_url(url.trim().to_string());
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
