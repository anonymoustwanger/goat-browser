//! Small HTTP helper used for filter lists, extension downloads and the Tor download.
//! Uses the operating system's TLS (Schannel on Windows), so no extra crypto libraries are bundled.

use std::io::Read;
use std::time::Duration;

fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::NativeTls)
        // Trust the Windows certificate store (the default is a bundled list, which we don't ship).
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(Duration::from_secs(600)))
        .max_redirects(10)
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Goatr/0.2")
        .build()
        .into()
}

/// Download a URL into memory, reporting (bytes_so_far, total_if_known).
pub fn get_bytes(url: &str, max: usize, mut progress: impl FnMut(u64, Option<u64>)) -> Result<Vec<u8>, String> {
    let resp = agent().get(url).call().map_err(|e| format!("{e}"))?;
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let mut reader = resp.into_body().into_reader();
    let mut out = Vec::with_capacity(total.unwrap_or(1 << 16).min(max as u64) as usize);
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if out.len() > max {
            return Err("download too large".into());
        }
        progress(out.len() as u64, total);
    }
    Ok(out)
}

pub fn get_text(url: &str) -> Result<String, String> {
    let b = get_bytes(url, 64 << 20, |_, _| {})?;
    Ok(String::from_utf8_lossy(&b).into_owned())
}

#[cfg(test)]
mod tls_test {
    #[test]
    #[ignore]
    fn https_works() {
        let b = super::get_bytes("https://pypi.org/simple/", 50 << 20, |_, _| {}).expect("https");
        assert!(b.len() > 1000);
    }
}
