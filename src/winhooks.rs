//! Windows-only: hooks the ad blocker into WebView2's network layer.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use webview2_com::{take_pwstr, Microsoft::Web::WebView2::Win32::*, ProcessFailedEventHandler, WebResourceRequestedEventHandler};
use windows::core::{w, Interface, HSTRING, PWSTR};
use wry::WebViewExtWindows;

use crate::adblock::{host_of, AdBlock};

/// WebView2 resource context → filter-list request type.
fn request_type(ctx: COREWEBVIEW2_WEB_RESOURCE_CONTEXT, url: &str, page_host: &str) -> &'static str {
    match ctx {
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT => {
            // The tab's own page vs. an embedded frame (ad iframes are documents too).
            if host_of(url) == page_host { "document" } else { "sub_frame" }
        }
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET => "stylesheet",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE => "image",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA => "media",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT => "font",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT => "script",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST | COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH | COREWEBVIEW2_WEB_RESOURCE_CONTEXT_EVENT_SOURCE => "xmlhttprequest",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET => "websocket",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING => "ping",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_CSP_VIOLATION_REPORT => "csp_report",
        _ => "other",
    }
}

/// Every request the page makes is checked against the filter engine; blocked ones get an
/// empty 403. `page_url` is the tab's current page (kept current by the navigation handler).
pub fn install_adblock(
    webview: &wry::WebView,
    sh: Arc<crate::shared::Shared>,
    page_url: Arc<Mutex<String>>,
    blocked: Arc<AtomicU32>,
    on_block: impl Fn(u32) + 'static,
) {
    let ab: Arc<AdBlock> = sh.adblock.clone();
    unsafe {
        let core = webview.webview();
        let env = webview.environment();
        let filter = HSTRING::from("*");
        let added = match core.cast::<ICoreWebView2_22>() {
            // Newer runtimes: also see requests from iframes and workers.
            Ok(c22) => c22.AddWebResourceRequestedFilterWithRequestSourceKinds(
                &filter,
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                // Pages and frames only. Intercepting service-worker / shared-worker requests can stall
                // WebView2 (Google search uses a service worker).
                COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_DOCUMENT,
            ),
            Err(_) => core.AddWebResourceRequestedFilter(&filter, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL),
        };
        crate::crashlog::debug(format!("adblock hook: filter {}", match &added { Ok(_) => "added".to_string(), Err(e) => format!("FAILED {e}") }));
        if added.is_err() {
            return;
        }
        let handler = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let _g = crate::crashlog::Scope::new("ad-block request check");
            let request = args.Request()?;
            let mut uri = PWSTR::null();
            request.Uri(&mut uri)?;
            let uri = take_pwstr(uri);
            if !uri.starts_with("http") {
                return Ok(());
            }
            let mut ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT::default();
            let _ = args.ResourceContext(&mut ctx);
            let page = page_url.lock().map(|h| h.clone()).unwrap_or_default();
            let rtype = request_type(ctx, &uri, &host_of(&page));
            let np = sh.net_privacy.try_read().map(|g| *g).unwrap_or_default();
            let page_site = crate::privacy::site_of(&host_of(&page));
            let req_site = crate::privacy::site_of(&host_of(&uri));
            let req_host = host_of(&uri);
            // An extension's own server (e.g. RoPro's ropro.io), even when called from a page.
            // (Only for other sites: a site's own trackers on its own pages are still blocked.)
            let ext_site = page_site != req_site && sh.ext_sites.try_read().is_ok_and(|l| l.iter().any(|h| req_host == *h || req_host.ends_with(&format!(".{h}"))));
            let third_party = rtype != "document" && !page_site.is_empty() && page_site != req_site && !ext_site;
            let allowed_here = !ab.active_on(&host_of(&page));
            // Privacy headers (Goat's onBeforeSendHeaders).
            if let Ok(h) = request.Headers() {
                let _ = h.SetHeader(w!("Sec-GPC"), w!("1"));
                let _ = h.SetHeader(w!("DNT"), w!("1"));
                if np.strip_hints {
                    for name in [w!("Sec-CH-UA-Full-Version-List"), w!("Sec-CH-UA-Platform-Version"), w!("Sec-CH-UA-Arch"), w!("Sec-CH-UA-Bitness"), w!("Sec-CH-UA-Model"), w!("Sec-CH-UA-WoW64"), w!("Sec-CH-UA-Full-Version"), w!("Sec-CH-Device-Memory"), w!("Device-Memory")] {
                        let _ = h.RemoveHeader(name);
                    }
                }
                if third_party {
                    let _ = h.RemoveHeader(w!("Cookie"));
                }
                if np.trim_referrers {
                    let mut v = PWSTR::null();
                    if h.GetHeader(w!("Referer"), &mut v).is_ok() && !v.is_null() {
                        let r = take_pwstr(v);
                        if crate::privacy::site_of(&host_of(&r)) != req_site {
                            if rtype == "document" {
                                let _ = h.RemoveHeader(w!("Referer"));
                            } else {
                                let origin = r.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
                                let _ = h.SetHeader(w!("Referer"), &HSTRING::from(format!("{origin}/")));
                            }
                        }
                    }
                }
                if np.block_prefetch {
                    let mut v = PWSTR::null();
                    let purpose = if h.GetHeader(w!("Sec-Purpose"), &mut v).is_ok() && !v.is_null() { take_pwstr(v) } else { String::new() };
                    if purpose.contains("prefetch") || purpose.contains("prerender") {
                        let response = env.CreateWebResourceResponse(None, 403, w!("Preloading off"), w!(""))?;
                        args.SetResponse(&response)?;
                        return Ok(());
                    }
                }
            }
            if np.block_fonts && rtype == "font" && third_party && !allowed_here {
                let response = env.CreateWebResourceResponse(None, 403, w!("Blocked font"), w!(""))?;
                args.SetResponse(&response)?;
                return Ok(());
            }
            // Requests an extension makes itself (its pages, frames and fetches) skip the blocker.
            if np.skip_extensions {
                let from_ext = uri.starts_with("chrome-extension:") || page.starts_with("chrome-extension:") || request.Headers().ok().is_some_and(|h| {
                    [w!("Origin"), w!("Referer")].iter().any(|n| {
                        let mut v = PWSTR::null();
                        h.GetHeader(*n, &mut v).is_ok() && !v.is_null() && take_pwstr(v).starts_with("chrome-extension:")
                    })
                });
                if from_ext || ext_site {
                    return Ok(());
                }
            }
            let t0 = std::time::Instant::now();
            let block = ab.should_block(&uri, &page, rtype);
            let took = t0.elapsed();
            if took.as_millis() > 20 {
                crate::crashlog::debug(format!("slow check {took:?} {uri:.200}"));
            }
            if block {
                let response = env.CreateWebResourceResponse(None, 403, w!("Blocked by Goatr"), w!(""))?;
                args.SetResponse(&response)?;
                crate::crashlog::note_blocked(&format!("[{rtype}] {uri}"));
                on_block(blocked.fetch_add(1, Ordering::Relaxed) + 1);
            }
            Ok(())
        }));
        let mut token = Default::default();
        let _ = core.add_WebResourceRequested(&handler, &mut token);
    }
}

/// Tell us when a tab's page stops responding or its process dies.
pub fn watch_process(webview: &wry::WebView, on_fail: impl Fn(&'static str) + 'static) {
    unsafe {
        let core = webview.webview();
        let handler = ProcessFailedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
            let _ = args.ProcessFailedKind(&mut kind);
            on_fail(match kind {
                COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE => "unresponsive",
                COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED => "page process exited",
                COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED => "frame process exited",
                COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED => "browser process exited",
                _ => "other process failure",
            });
            Ok(())
        }));
        let mut token = Default::default();
        let _ = core.add_ProcessFailed(&handler, &mut token);
    }
}

/// Edge's built-in tracking prevention (runs inside WebView2, no per-request cost here).
pub fn set_tracking_prevention(webview: &wry::WebView, block_third_party: bool) {
    unsafe {
        let Ok(c13) = webview.webview().cast::<ICoreWebView2_13>() else { return };
        let Ok(profile) = c13.Profile() else { return };
        if let Ok(p3) = profile.cast::<ICoreWebView2Profile3>() {
            let _ = p3.SetPreferredTrackingPreventionLevel(if block_third_party { COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_BALANCED } else { COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_BASIC });
        }
    }
}

/// Memory saver: a background tab frees most of its memory; it comes back when shown.
pub fn set_memory_low(webview: &wry::WebView, low: bool) {
    unsafe {
        if let Ok(c19) = webview.webview().cast::<ICoreWebView2_19>() {
            let _ = c19.SetMemoryUsageTargetLevel(if low { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW } else { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL });
        }
    }
}

/// CPU cores and priority for Goatr and all its page processes.
/// `cores` 0 = all. `priority`: normal | above | high.
pub fn apply_cpu(webview: Option<&wry::WebView>, cores: u32, priority: &str) {
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, SetPriorityClass, SetProcessAffinityMask, ABOVE_NORMAL_PRIORITY_CLASS, HIGH_PRIORITY_CLASS,
        NORMAL_PRIORITY_CLASS, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    };
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(64) as u32;
    let use_n = if cores == 0 || cores >= n { n } else { cores };
    let mask: usize = if use_n >= 64 { usize::MAX } else { (1usize << use_n) - 1 };
    let class = match priority {
        "high" => HIGH_PRIORITY_CLASS,
        "above" => ABOVE_NORMAL_PRIORITY_CLASS,
        _ => NORMAL_PRIORITY_CLASS,
    };
    unsafe {
        let me = GetCurrentProcess();
        let _ = SetProcessAffinityMask(me, mask);
        let _ = SetPriorityClass(me, class);
        let Some(wv) = webview else { return };
        let Ok(env8) = wv.environment().cast::<ICoreWebView2Environment8>() else { return };
        let Ok(list) = env8.GetProcessInfos() else { return };
        let mut count = 0u32;
        let _ = list.Count(&mut count);
        for i in 0..count {
            let Ok(info) = list.GetValueAtIndex(i) else { continue };
            let mut pid = 0i32;
            if info.ProcessId(&mut pid).is_err() || pid <= 0 {
                continue;
            }
            if let Ok(h) = OpenProcess(PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION, false, pid as u32) {
                let _ = SetProcessAffinityMask(h, mask);
                let _ = SetPriorityClass(h, class);
                let _ = windows::Win32::Foundation::CloseHandle(h);
            }
        }
    }
}

/// The ids WebView2 gave the loaded extensions: (name, id).
pub fn query_extensions(webview: &wry::WebView, done: impl FnOnce(Vec<(String, String)>) + 'static) {
    unsafe {
        let Ok(c13) = webview.webview().cast::<ICoreWebView2_13>() else { return };
        let Ok(profile) = c13.Profile() else { return };
        let Ok(p7) = profile.cast::<ICoreWebView2Profile7>() else { return };
        let handler = webview2_com::ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(move |_hr, list| {
            let mut out = Vec::new();
            if let Some(list) = list {
                let mut n = 0u32;
                let _ = list.Count(&mut n);
                for i in 0..n {
                    let Ok(ext) = list.GetValueAtIndex(i) else { continue };
                    let (mut id, mut name) = (PWSTR::null(), PWSTR::null());
                    if ext.Id(&mut id).is_ok() && ext.Name(&mut name).is_ok() {
                        out.push((take_pwstr(name), take_pwstr(id)));
                    }
                }
            }
            done(out);
            Ok(())
        }));
        let _ = p7.GetBrowserExtensions(&handler);
    }
}

/// Load (or reload) an unpacked extension now; reports its (name, id) or the error WebView2 gives.
pub fn add_extension(webview: &wry::WebView, folder: &std::path::Path, done: impl FnOnce(Result<(String, String), String>) + 'static) {
    unsafe {
        let Ok(c13) = webview.webview().cast::<ICoreWebView2_13>() else { return };
        let Ok(profile) = c13.Profile() else { return };
        let Ok(p7) = profile.cast::<ICoreWebView2Profile7>() else { return };
        let handler = webview2_com::ProfileAddBrowserExtensionCompletedHandler::create(Box::new(move |hr, ext| {
            match (hr, ext) {
                (Ok(()), Some(ext)) => {
                    let (mut id, mut name) = (PWSTR::null(), PWSTR::null());
                    let _ = ext.Id(&mut id);
                    let _ = ext.Name(&mut name);
                    done(Ok((take_pwstr(name), take_pwstr(id))));
                }
                (Err(e), _) => done(Err(e.message().to_string())),
                _ => done(Err("no extension returned".into())),
            }
            Ok(())
        }));
        let path = HSTRING::from(folder.as_os_str());
        if let Err(e) = p7.AddBrowserExtension(&path, &handler) {
            crate::crashlog::debug(format!("AddBrowserExtension call failed: {e}"));
        }
    }
}

/// Links to apps (roblox-player:, steam:, …) started from frames or scripts: Goatr asks first.
pub fn watch_external_links(webview: &wry::WebView, ask: impl Fn(String, String) + 'static) {
    unsafe {
        let Ok(c18) = webview.webview().cast::<ICoreWebView2_18>() else { return };
        let handler = webview2_com::LaunchingExternalUriSchemeEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let _ = args.SetCancel(true);
            let (mut u, mut o) = (PWSTR::null(), PWSTR::null());
            let uri = if args.Uri(&mut u).is_ok() { take_pwstr(u) } else { String::new() };
            let origin = if args.InitiatingOrigin(&mut o).is_ok() { take_pwstr(o) } else { String::new() };
            if !uri.is_empty() {
                ask(uri, origin);
            }
            Ok(())
        }));
        let mut token = Default::default();
        let _ = c18.add_LaunchingExternalUriScheme(&handler, &mut token);
    }
}

/// The app Windows opens a link type with (e.g. "Roblox"), or None if no app handles it.
pub fn scheme_app_name(scheme: &str) -> Option<String> {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CLASSES_ROOT, RRF_RT_REG_SZ};
    let read = |key: &str, value: Option<&str>| -> Option<String> {
        let mut buf = [0u16; 512];
        let mut len = (buf.len() * 2) as u32;
        let k = HSTRING::from(key);
        let v = value.map(HSTRING::from);
        let r = unsafe {
            RegGetValueW(
                HKEY_CLASSES_ROOT,
                &k,
                v.as_ref().map(|h| windows::core::PCWSTR(h.as_ptr())).unwrap_or(windows::core::PCWSTR::null()),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr() as *mut _),
                Some(&mut len),
            )
        };
        if r.is_err() {
            return None;
        }
        let n = (len as usize / 2).saturating_sub(1).min(buf.len());
        Some(String::from_utf16_lossy(&buf[..n]))
    };
    // Must be a registered link type with a command to run.
    read(&format!("{scheme}\\shell\\open\\command"), None)?;
    let name = read(scheme, Some("FriendlyTypeName")).or_else(|| read(scheme, None)).unwrap_or_default();
    let name = name.trim_start_matches("URL:").trim_start_matches("URL ").trim().trim_end_matches(" Protocol").trim().to_string();
    Some(if name.is_empty() || name.starts_with('@') { scheme.to_string() } else { name })
}
