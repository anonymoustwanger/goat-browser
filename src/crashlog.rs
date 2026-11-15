//! Crash reporting: Rust panics and Windows crashes are written to `data\crash.log` next to
//! Goat.exe (release builds have no console). A short trail of recent actions is included.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use crate::store::app_dir;

static BEAT: AtomicU64 = AtomicU64::new(0);
static BUSY: AtomicBool = AtomicBool::new(false);
static PHASE: Mutex<&'static str> = Mutex::new("starting");
#[cfg(windows)]
static MAIN_THREAD: AtomicU64 = AtomicU64::new(0);

static TRAIL: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

static DEBUG: std::sync::OnceLock<Mutex<std::sync::mpsc::Sender<String>>> = std::sync::OnceLock::new();

/// Append a line to data\debug.log (written by a background thread, never blocks the caller).
pub fn debug(s: impl Into<String>) {
    let line = format!("{} {}", chrono::Local::now().format("%H:%M:%S%.3f"), s.into());
    let tx = DEBUG.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let _ = std::thread::Builder::new().name("debug-log".into()).spawn(move || {
            let dir = app_dir().join("data");
            let _ = std::fs::create_dir_all(&dir);
            // Extra Goat processes (other windows, the standby window) get their own log, so they
            // don't wipe the main window's.
            let main = !std::env::args().any(|a| a == "--new-window" || a.starts_with("--window"));
            let (name, prev) = if main { ("debug.log", "debug-prev.log") } else { ("debug-window.log", "debug-window-prev.log") };
            let path = dir.join(name);
            // Start fresh each run (keep the previous run).
            let _ = std::fs::rename(&path, dir.join(prev));
            let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else { return };
            while let Ok(l) = rx.recv() {
                let _ = writeln!(f, "{l}");
                while let Ok(l) = rx.try_recv() {
                    let _ = writeln!(f, "{l}");
                }
                let _ = f.flush();
            }
        });
        Mutex::new(tx)
    });
    if let Ok(t) = tx.try_lock() {
        let _ = t.send(line);
    }
}

static BLOCKED: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// Remember a recent blocked request (last 25), to show next to freezes.
pub fn note_blocked(s: &str) {
    debug(format!("blocked {s:.300}"));
    if let Ok(mut t) = BLOCKED.try_lock() {
        if t.len() >= 25 {
            t.pop_front();
        }
        t.push_back(format!("{} {:.200}", chrono::Local::now().format("%H:%M:%S%.3f"), s));
    }
}

/// Remember a recent action (kept: last 40).
pub fn note(s: impl Into<String>) {
    let s = s.into();
    debug(format!("action: {s:.300}"));
    if let Ok(mut t) = TRAIL.lock() {
        if t.len() >= 40 {
            t.pop_front();
        }
        t.push_back(format!("{} {}", chrono::Local::now().format("%H:%M:%S%.3f"), s));
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Called by the UI thread: it is alive and about to do `what`.
pub fn phase(what: &'static str) {
    BEAT.store(now_ms(), Ordering::Relaxed);
    mark(what);
}

/// Record what the UI thread is doing without counting it as a drawn frame.
fn mark(what: &'static str) {
    if let Ok(mut p) = PHASE.try_lock() {
        *p = what;
    }
}

static PROBE: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

/// How the watchdog wakes the UI thread (egui request_repaint) to check it still responds.
pub fn set_probe(f: impl Fn() + Send + Sync + 'static) {
    let _ = PROBE.set(Box::new(f));
}

/// Minimized windows don't draw frames; don't treat that as a freeze.
pub fn set_hidden(h: bool) {
    BUSY.store(h, Ordering::Relaxed);
}

/// Marks a step done on the UI thread outside a frame (webview callbacks); restores on drop.
pub struct Scope(&'static str);
impl Scope {
    pub fn new(what: &'static str) -> Self {
        let prev = PHASE.try_lock().map(|p| *p).unwrap_or("?");
        mark(what);
        Scope(prev)
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        mark(self.0);
    }
}

/// Watchdog: if the UI thread is stuck in one step for 6 s, log where (with its call stack on
/// Windows) to data\crash.log, so freezes can be fixed like crashes.
fn watchdog() {
    let _ = std::thread::Builder::new().name("watchdog".into()).spawn(|| {
        let mut reported = 0u64;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            // Wake the UI thread from a separate short-lived thread: if the UI thread is stuck
            // holding egui's lock, the wake-up call would block the watchdog too.
            if PROBE.get().is_some() {
                let _ = std::thread::Builder::new().name("watchdog-probe".into()).spawn(|| {
                    if let Some(p) = PROBE.get() {
                        p();
                    }
                });
            }
            let beat = BEAT.load(Ordering::Relaxed);
            if BUSY.load(Ordering::Relaxed) || beat == 0 || beat == reported {
                continue;
            }
            let stuck = now_ms().saturating_sub(beat);
            if stuck > 6000 {
                reported = beat;
                let phase = PHASE.try_lock().map(|p| *p).unwrap_or("?");
                #[allow(unused_mut)]
                let mut detail = format!("No frame drawn for {}s (window not updating). UI thread last in: {phase}", stuck / 1000);
                #[cfg(windows)]
                detail.push_str(&format!("\n-- all threads --\n{}", unsafe { main_stack() }));
                debug(format!("FREEZE {detail}"));
                write("freeze", &detail);
            }
        }
    });
}

#[cfg(windows)]
unsafe fn module_of(addr: u64) -> String {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT};
    let mut m = HMODULE::default();
    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    if unsafe { GetModuleHandleExW(flags, windows::core::PCWSTR(addr as *const u16), &mut m) }.is_err() {
        return format!("0x{addr:x}");
    }
    let mut buf = [0u16; 260];
    let n = unsafe { GetModuleFileNameW(Some(m), &mut buf) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    let name = path.rsplit('\\').next().unwrap_or(&path).to_string();
    format!("{name}+0x{:x}", addr - m.0 as u64)
}

/// Raw return addresses of a thread's stack. Nothing is allocated while it's suspended (it
/// might hold the heap lock).
#[cfg(windows)]
unsafe fn walk(h: windows::Win32::Foundation::HANDLE) -> Option<Vec<u64>> {
    use windows::Win32::System::Diagnostics::Debug::{GetThreadContext, RtlLookupFunctionEntry, RtlVirtualUnwind, CONTEXT, CONTEXT_FLAGS, UNW_FLAG_NHANDLER};
    use windows::Win32::System::Threading::{ResumeThread, SuspendThread};
    let mut pcs: Vec<u64> = Vec::with_capacity(64);
    let mut ctx = CONTEXT { ContextFlags: CONTEXT_FLAGS(0x0010_000B), ..Default::default() }; // CONTEXT_FULL (AMD64)
    if unsafe { SuspendThread(h) } == u32::MAX {
        return None;
    }
    let got = unsafe { GetThreadContext(h, &mut ctx) };
    if got.is_ok() {
        while pcs.len() < 64 {
            let pc = ctx.Rip;
            if pc == 0 {
                break;
            }
            pcs.push(pc);
            let mut base = 0u64;
            let f = unsafe { RtlLookupFunctionEntry(pc, &mut base, None) };
            if f.is_null() {
                ctx.Rip = unsafe { *(ctx.Rsp as *const u64) };
                ctx.Rsp += 8;
            } else {
                let mut data = std::ptr::null_mut();
                let mut frame = 0u64;
                unsafe { RtlVirtualUnwind(UNW_FLAG_NHANDLER, base, pc, f, &mut ctx, &mut data, &mut frame, None) };
            }
        }
    }
    unsafe { ResumeThread(h) };
    got.ok().map(|_| pcs)
}

/// Every thread's stack (the UI thread first), so a freeze shows who holds what.
#[cfg(windows)]
unsafe fn main_stack() -> String {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32};
    use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId, GetThreadDescription, OpenThread, THREAD_GET_CONTEXT, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME};
    let fmt = |pcs: Vec<u64>| pcs.into_iter().map(|pc| format!("  {}", unsafe { module_of(pc) })).collect::<Vec<_>>().join("\n");
    let mut out = String::new();
    let main = HANDLE(MAIN_THREAD.load(Ordering::Relaxed) as *mut _);
    if !main.0.is_null() {
        out.push_str("[UI thread]\n");
        out.push_str(&unsafe { walk(main) }.map(fmt).unwrap_or_else(|| "  (couldn't read)".into()));
    }
    // Other threads of Goat (names where set).
    let me = unsafe { GetCurrentThreadId() };
    let pid = unsafe { GetCurrentProcessId() };
    let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }) else { return out };
    let mut te = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
    let mut ids = Vec::new();
    if unsafe { Thread32First(snap, &mut te) }.is_ok() {
        loop {
            if te.th32OwnerProcessID == pid && te.th32ThreadID != me {
                ids.push(te.th32ThreadID);
            }
            if unsafe { Thread32Next(snap, &mut te) }.is_err() {
                break;
            }
        }
    }
    let _ = unsafe { CloseHandle(snap) };
    for tid in ids.into_iter().take(80) {
        let Ok(h) = (unsafe { OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_LIMITED_INFORMATION, false, tid) }) else { continue };
        let name = unsafe { GetThreadDescription(h) }.ok().map(|p| unsafe { p.to_string() }.unwrap_or_default()).unwrap_or_default();
        let pcs = unsafe { walk(h) };
        let _ = unsafe { CloseHandle(h) };
        let Some(pcs) = pcs else { continue };
        // Skip threads that are just idle inside system DLLs (WebView2 / thread-pool workers).
        let text = fmt(pcs);
        if !text.contains("Goat.exe") && !text.contains("goat.exe") {
            continue;
        }
        out.push_str(&format!("\n[thread {tid} {name}]\n{text}"));
    }
    out
}

pub fn write(kind: &str, detail: &str) {
    let dir = app_dir().join("data");
    let _ = std::fs::create_dir_all(&dir);
    debug(format!("LOGGED {kind}: {detail:.500}"));
    let trail = TRAIL.try_lock().map(|t| t.iter().cloned().collect::<Vec<_>>().join("\n")).unwrap_or_default();
    let blocked = BLOCKED.try_lock().map(|t| t.iter().cloned().collect::<Vec<_>>().join("\n")).unwrap_or_default();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("crash.log")) {
        let _ = writeln!(
            f,
            "==== {} — Goat {} — {kind}\n{detail}\n-- recent actions --\n{trail}\n-- recently blocked by the ad blocker --\n{blocked}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            env!("CARGO_PKG_VERSION")
        );
    }
}

pub fn install() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::{DuplicateHandle, HANDLE, DUPLICATE_SAME_ACCESS};
        use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread};
        let mut h = HANDLE::default();
        if DuplicateHandle(GetCurrentProcess(), GetCurrentThread(), GetCurrentProcess(), &mut h, 0, false, DUPLICATE_SAME_ACCESS).is_ok() {
            MAIN_THREAD.store(h.0 as u64, Ordering::Relaxed);
        }
    }
    watchdog();
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current().name().unwrap_or("?").to_string();
        let bt = std::backtrace::Backtrace::force_capture();
        write("panic", &format!("thread '{thread}': {info}\n{bt}"));
        default(info);
    }));
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Diagnostics::Debug::{SetUnhandledExceptionFilter, EXCEPTION_POINTERS};
        unsafe extern "system" fn on_crash(info: *const EXCEPTION_POINTERS) -> i32 {
            let detail = unsafe {
                info.as_ref()
                    .and_then(|i| i.ExceptionRecord.as_ref())
                    .map(|r| format!("exception code 0x{:08X} at address {:?}", r.ExceptionCode.0 as u32, r.ExceptionAddress))
                    .unwrap_or_else(|| "unknown exception".into())
            };
            write("native crash", &detail);
            0 // EXCEPTION_CONTINUE_SEARCH
        }
        SetUnhandledExceptionFilter(Some(on_crash));
    }
}
