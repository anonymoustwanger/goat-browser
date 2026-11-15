//! Automatic updates: checks GitHub Releases for a newer Goat, downloads its GoatSetup.exe in
//! the background, and runs it (silently) when you choose "Update" or close Goat.

use std::path::PathBuf;

const RELEASES_API: &str = "https://api.github.com/repos/anonymoustwanger/goat-browser/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/anonymoustwanger/goat-browser/releases/latest";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A downloaded update, ready to install.
#[derive(Debug, Clone)]
pub struct Ready {
    pub version: String,
    pub setup: PathBuf,
}

/// What a check found.
#[derive(Debug)]
pub enum Found {
    UpToDate,
    Ready(Ready),
    /// Newer version, but there's nothing Goat can install by itself (portable copy, or the
    /// release has no GoatSetup.exe): show the download page.
    Manual(String),
}

fn parts(v: &str) -> Vec<u64> {
    v.trim_start_matches(['v', 'V']).split(['.', '-']).map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)).collect()
}

pub fn newer(remote: &str, local: &str) -> bool {
    parts(remote) > parts(local)
}

/// Goat was installed by GoatSetup (not a portable copy or a dev build).
pub fn installed() -> bool {
    crate::store::exe_dir().join(".goatr-installed").exists()
}

/// Ask GitHub; if there's a newer version, download its installer to the temp folder.
/// Runs on a background thread.
pub fn check() -> Result<Found, String> {
    let text = crate::net::get_text(RELEASES_API).map_err(|e| format!("Couldn't reach GitHub: {e}"))?;
    let j: serde_json::Value = serde_json::from_str(&text).map_err(|_| "Unexpected answer from GitHub".to_string())?;
    let tag = j["tag_name"].as_str().unwrap_or("").trim_start_matches(['v', 'V']).to_string();
    if tag.is_empty() || !newer(&tag, VERSION) {
        return Ok(Found::UpToDate);
    }
    let asset = j["assets"].as_array().and_then(|a| a.iter().find(|x| x["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case("GoatSetup.exe")))).cloned();
    let (Some(asset), true) = (asset, installed()) else { return Ok(Found::Manual(tag)) };
    let url = asset["browser_download_url"].as_str().unwrap_or("").to_string();
    let size = asset["size"].as_u64().unwrap_or(0);
    let dest = std::env::temp_dir().join(format!("GoatSetup-{tag}.exe"));
    // Already downloaded by an earlier check.
    if size > 0 && std::fs::metadata(&dest).is_ok_and(|m| m.len() == size) {
        return Ok(Found::Ready(Ready { version: tag, setup: dest }));
    }
    let bytes = crate::net::get_bytes(&url, 200 << 20, |_, _| {})?;
    if size > 0 && bytes.len() as u64 != size {
        return Err("The update download was incomplete.".into());
    }
    if bytes.len() < 2 || &bytes[..2] != b"MZ" {
        return Err("The update download isn't a Windows program.".into());
    }
    std::fs::write(&dest, &bytes).map_err(|e| format!("Couldn't save the update: {e}"))?;
    Ok(Found::Ready(Ready { version: tag, setup: dest }))
}

/// Start the installer in update mode: it waits for Goat to close, installs, and (with
/// `relaunch`) opens Goat again.
pub fn run(r: &Ready, relaunch: bool) -> bool {
    let mut cmd = std::process::Command::new(&r.setup);
    cmd.arg("--update").arg(format!("--dir={}", crate::store::exe_dir().display()));
    if relaunch {
        cmd.arg("--relaunch");
    }
    cmd.spawn().is_ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn versions() {
        assert!(super::newer("1.0.1", "1.0.0"));
        assert!(super::newer("v1.2.0", "1.1.9"));
        assert!(!super::newer("1.0.0", "1.0.0"));
        assert!(!super::newer("0.9", "1.0.0"));
    }
}
