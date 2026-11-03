use std::collections::HashMap;
use std::sync::mpsc::{Sender, channel};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Manager};
use windows::Win32::Media::Audio::{
    IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, eConsole, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::core::{Interface, Result as WinResult};

use crate::App;
use crate::commands::instances_changed;

const TICK: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MuteTarget {
    pub pid: u32,
    pub muted: bool,
}

static WAKE: LazyLock<Mutex<Option<Sender<()>>>> = LazyLock::new(|| Mutex::new(None));

pub fn wake() {
    if let Some(tx) = WAKE.lock().ok().and_then(|w| w.clone()) {
        let _ = tx.send(());
    }
}

// -- Main func
pub fn spawn(app: AppHandle) {
    let (tx, rx) = channel();
    if let Ok(mut w) = WAKE.lock() {
        *w = Some(tx);
    }
    let started = std::thread::Builder::new().name("audio".into()).spawn(move || {
        // -- SAFETY: COM on this thread only (S_FALSE: already initialised, also fine); every interface below is released on drop
        if let Err(e) = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
            log::warn!("client mute: COM unavailable, muting is off: {e}");
            return;
        }
        let mut applied: HashMap<u32, bool> = HashMap::new();
        loop {
            let changed = sweep(&app, &mut applied);
            if changed {
                instances_changed(&app);
            }
            let _ = rx.recv_timeout(TICK);
        }
    });
    if let Err(e) = started {
        log::warn!("client mute not running: {e}");
    }
}

fn sweep(app: &AppHandle, applied: &mut HashMap<u32, bool>) -> bool {
    let s = app.state::<App>();
    let targets = s.tracker.mute_targets();
    applied.retain(|pid, _| targets.iter().any(|t| t.pid == *pid));
    let mut changed = false;
    for t in targets {
        if !t.muted && applied.get(&t.pid) != Some(&true) {
            continue;
        }
        match mute(t.pid, t.muted) {
            Ok(n) => {
                if applied.insert(t.pid, t.muted) != Some(t.muted) {
                    if t.muted {
                        log::info!("client {} muted ({} session(s))", t.pid, n);
                    } else {
                        log::info!("client {} unmuted ({} session(s))", t.pid, n);
                    }
                    changed = true;
                }
            }
            Err(e) => log::debug!("client {} mute={}: {e}", t.pid, t.muted),
        }
    }
    changed
}

fn mute(pid: u32, muted: bool) -> WinResult<usize> {
    // -- SAFETY: the enumerator, the device manager and the session list are COM objects released on drop; nothing here keeps …
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
        let sessions = manager.GetSessionEnumerator()?;
        let count = sessions.GetCount()?;
        let mut found = 0;
        for i in 0..count {
            let Ok(control) = sessions.GetSession(i) else { continue };
            let Ok(control) = control.cast::<IAudioSessionControl2>() else { continue };
            if control.GetProcessId()? != pid {
                continue;
            }
            let Ok(volume) = control.cast::<ISimpleAudioVolume>() else { continue };
            volume.SetMute(muted, std::ptr::null())?;
            found += 1;
        }
        Ok(found)
    }
}
