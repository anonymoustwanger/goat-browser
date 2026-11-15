//! "Audio for Discord streaming" (Windows only, experimental).
//!
//! Page sound comes from the web engine's own processes (msedgewebview2.exe), not Goat.exe, so
//! Discord, which records the app whose window you stream, hears nothing. While this is on,
//! Goat records the web engine's sound (Windows' per-app loopback), plays it from Goat.exe,
//! and turns the engine's own sound down to inaudible so you don't hear it twice.
//!
//! The recording runs in a helper process (`Goat.exe --audio-relay <engine pid>`), so a
//! problem in Windows' audio code can never take the browser down. The helper stops (and gives
//! the engine its sound back) when Goat closes its input or exits.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::{implement, Interface, Ref, Result as WinResult};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, BLOB, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;

const RATE: u32 = 48_000;
/// Windows records an app *after* its volume, so muting the engine also silenced the copy.
/// Instead the engine's own sound is turned down to 0.01% (inaudible) and the copy is turned
/// back up by the same amount. The mixing is in floating point, so nothing is lost.
const ENGINE_VOLUME: f32 = 0.0001;
const GAIN: f32 = 1.0 / ENGINE_VOLUME;
const CHANNELS: u16 = 2;

/// The helper process; dropping it (or `stop`) ends it and unmutes the engine.
pub struct Relay {
    child: std::process::Child,
    browser_pid: u32,
}

impl Relay {
    pub fn start(browser_pid: u32) -> Option<Relay> {
        use std::os::windows::process::CommandExt;
        let exe = std::env::current_exe().ok()?;
        let child = std::process::Command::new(exe)
            .args(["--audio-relay", &browser_pid.to_string()])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(0x0800_0000) // no console window
            .spawn()
            .ok()?;
        Some(Relay { child, browser_pid })
    }

    /// Still running? If it died, the engine's sound is given back.
    pub fn alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            _ => {
                unmute_tree_of(self.browser_pid);
                false
            }
        }
    }

    pub fn stop(mut self) {
        self.end();
    }

    fn end(&mut self) {
        // Closing its input asks it to stop (it unmutes on the way out).
        drop(self.child.stdin.take());
        for _ in 0..20 {
            if !matches!(self.child.try_wait(), Ok(None)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        unmute_tree_of(self.browser_pid);
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.end();
    }
}

/// The helper's own log: data\\audio-relay.log (Goat's debug.log belongs to the browser).
fn log(line: &str) {
    use std::io::Write;
    let dir = crate::store::app_dir().join("data");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("audio-relay.log")) {
        let _ = writeln!(f, "{} {line}", chrono::Local::now().format("%H:%M:%S%.3f"));
    }
}

/// `Goat.exe --audio-relay <pid>`: the helper process. Returns when stdin closes.
pub fn helper_main(browser_pid: u32) {
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = [0u8; 64];
        let mut input = std::io::stdin();
        while matches!(input.read(&mut buf), Ok(n) if n > 0) {}
        s.store(true, Ordering::SeqCst);
    });
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    if let Err(e) = run(browser_pid, &stop) {
        log(&format!("stopped: {e}"));
    }
    unmute_tree_of(browser_pid);
}

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Activated(isize);

impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
    fn ActivateCompleted(&self, _op: Ref<IActivateAudioInterfaceAsyncOperation>) -> WinResult<()> {
        unsafe {
            let _ = SetEvent(HANDLE(self.0 as _));
        }
        Ok(())
    }
}

fn float_format() -> WAVEFORMATEX {
    WAVEFORMATEX {
        wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
        nChannels: CHANNELS,
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * CHANNELS as u32 * 4,
        nBlockAlign: CHANNELS * 4,
        wBitsPerSample: 32,
        cbSize: 0,
    }
}

/// An audio client that records `pid` and every process it started.
fn loopback_client(pid: u32) -> Result<IAudioClient, String> {
    unsafe {
        // Kept on the heap (stable address) and alive until the end of this function.
        let mut params = Box::new(AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        });
        let mut pv = std::mem::ManuallyDrop::new(PROPVARIANT::default());
        {
            let inner = &mut *pv.Anonymous.Anonymous;
            inner.vt = VT_BLOB;
            inner.Anonymous.blob = BLOB {
                cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                pBlobData: &mut *params as *mut AUDIOCLIENT_ACTIVATION_PARAMS as *mut u8,
            };
        }
        let event = CreateEventW(None, false, false, None).map_err(|e| e.to_string())?;
        let handler: IActivateAudioInterfaceCompletionHandler = Activated(event.0 as isize).into();
        let op = ActivateAudioInterfaceAsync(VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, &IAudioClient::IID, Some(&*pv as *const PROPVARIANT), &handler)
            .map_err(|e| format!("can't start recording ({e})"))?;
        let waited = WaitForSingleObject(event, 5000);
        let _ = CloseHandle(event);
        if waited != WAIT_OBJECT_0 {
            return Err("recording didn't start in time".into());
        }
        let mut hr = windows::core::HRESULT(0);
        let mut unk = None;
        op.GetActivateResult(&mut hr, &mut unk).map_err(|e| e.to_string())?;
        hr.ok().map_err(|e| format!("Windows refused recording ({e})"))?;
        let client = unk.ok_or("no audio client")?.cast::<IAudioClient>().map_err(|e| e.to_string());
        drop(params);
        client
    }
}

fn run(browser_pid: u32, stop: &AtomicBool) -> Result<(), String> {
    unsafe {
        let fmt = float_format();
        // Record the web engine.
        let cap = loopback_client(browser_pid)?;
        cap.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            200_000,
            0,
            &fmt,
            None,
        )
        .map_err(|e| format!("recording setup failed ({e})"))?;
        let cap_event = CreateEventW(None, false, false, None).map_err(|e| e.to_string())?;
        cap.SetEventHandle(cap_event).map_err(|e| e.to_string())?;
        let capture: IAudioCaptureClient = cap.GetService().map_err(|e| e.to_string())?;

        // Play it from Goat.exe on the default speakers.
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole).map_err(|e| e.to_string())?;
        let out: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(|e| e.to_string())?;
        out.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            1_000_000,
            0,
            &fmt,
            None,
        )
        .map_err(|e| format!("playback setup failed ({e})"))?;
        let render: IAudioRenderClient = out.GetService().map_err(|e| e.to_string())?;
        let out_frames = out.GetBufferSize().map_err(|e| e.to_string())?;
        let sessions: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None).map_err(|e| e.to_string())?;

        cap.Start().map_err(|e| e.to_string())?;
        out.Start().map_err(|e| e.to_string())?;
        log(&format!("recording web engine {browser_pid}"));

        let mut last_mute = Instant::now() - Duration::from_secs(10);
        let block = fmt.nBlockAlign as usize;
        while !stop.load(Ordering::SeqCst) {
            // Keep the engine's own sound muted (its sound processes can come and go).
            if last_mute.elapsed() > Duration::from_millis(800) {
                last_mute = Instant::now();
                mute_tree(&sessions, browser_pid);
            }
            let _ = WaitForSingleObject(cap_event, 100);
            loop {
                let n = capture.GetNextPacketSize().unwrap_or(0);
                if n == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                if capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None).is_err() {
                    break;
                }
                let pad = out.GetCurrentPadding().unwrap_or(out_frames);
                let room = out_frames.saturating_sub(pad).min(frames);
                if room > 0 {
                    if let Ok(dst) = render.GetBuffer(room) {
                        let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
                        if silent || data.is_null() {
                            std::ptr::write_bytes(dst, 0, room as usize * block);
                        } else {
                            let n = room as usize * CHANNELS as usize;
                            let src = std::slice::from_raw_parts(data as *const f32, n);
                            let out = std::slice::from_raw_parts_mut(dst as *mut f32, n);
                            for (o, s) in out.iter_mut().zip(src) {
                                *o = (s * GAIN).clamp(-1.0, 1.0);
                            }
                        }
                        let _ = render.ReleaseBuffer(room, 0);
                    }
                }
                let _ = capture.ReleaseBuffer(frames);
            }
        }
        let _ = cap.Stop();
        let _ = out.Stop();
        let _ = CloseHandle(cap_event);
        Ok(())
    }
}


/// `root` and every process started from it.
fn process_tree(root: u32) -> HashSet<u32> {
    let mut parent: Vec<(u32, u32)> = Vec::new();
    unsafe {
        if let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            if Process32FirstW(snap, &mut e).is_ok() {
                loop {
                    parent.push((e.th32ProcessID, e.th32ParentProcessID));
                    if Process32NextW(snap, &mut e).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
    }
    let mut tree: HashSet<u32> = HashSet::from([root]);
    loop {
        let before = tree.len();
        for (pid, ppid) in &parent {
            if tree.contains(ppid) {
                tree.insert(*pid);
            }
        }
        if tree.len() == before {
            break;
        }
    }
    tree
}

fn mute_tree(sessions: &IAudioSessionManager2, root: u32) {
    let tree = process_tree(root);
    let me = std::process::id();
    unsafe {
        let Ok(list) = sessions.GetSessionEnumerator() else { return };
        let n = list.GetCount().unwrap_or(0);
        for i in 0..n {
            let Ok(s) = list.GetSession(i) else { continue };
            let Ok(s2) = s.cast::<IAudioSessionControl2>() else { continue };
            let pid = s2.GetProcessId().unwrap_or(0);
            if pid == 0 || pid == me || !tree.contains(&pid) {
                continue;
            }
            if let Ok(v) = s.cast::<ISimpleAudioVolume>() {
                if v.GetMute().map(|m| m.as_bool()).unwrap_or(false) {
                    let _ = v.SetMute(false, std::ptr::null());
                }
                if v.GetMasterVolume().map(|x| (x - ENGINE_VOLUME).abs() > ENGINE_VOLUME * 0.1).unwrap_or(false) {
                    let _ = v.SetMasterVolume(ENGINE_VOLUME, std::ptr::null());
                }
            }
        }
    }
}

/// Give the web engine (and everything it started) its sound back.
pub fn unmute_tree_of(root: u32) {
    let tree = process_tree(root);
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(devices) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return };
        let Ok(device) = devices.GetDefaultAudioEndpoint(eRender, eConsole) else { return };
        let Ok(sessions) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { return };
        let Ok(list) = sessions.GetSessionEnumerator() else { return };
        for i in 0..list.GetCount().unwrap_or(0) {
            let Ok(s) = list.GetSession(i) else { continue };
            let pid = s.cast::<IAudioSessionControl2>().and_then(|s2| s2.GetProcessId()).unwrap_or(0);
            if pid != 0 && tree.contains(&pid) {
                if let Ok(v) = s.cast::<ISimpleAudioVolume>() {
                    let _ = v.SetMute(false, std::ptr::null());
                    let _ = v.SetMasterVolume(1.0, std::ptr::null());
                }
            }
        }
    }
}
