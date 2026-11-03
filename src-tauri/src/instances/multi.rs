use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use windows::Win32::Foundation::{
    CompareObjectHandles, DUPLICATE_CLOSE_SOURCE, DUPLICATE_HANDLE_OPTIONS, DuplicateHandle, ERROR_INVALID_HANDLE, GENERIC_READ, HANDLE,
    STATUS_INFO_LENGTH_MISMATCH,
};
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE};
use windows::core::{HSTRING, Owned};

use super::process;
use crate::error::{AppError, Result};
use crate::model::MultiInstanceStatus;

pub const SINGLETON_NAME: &str = "ROBLOX_singletonEvent";

fn running() -> String {
    tr!("Roblox is already running. Close it (including its tray icon) to run several clients.")
}

pub struct Multi {
    inner: Mutex<Option<Held>>,
    launches: AtomicUsize,
}

struct Held {
    _mutex: Singleton,
    files: Vec<File>,
    survivors: Vec<(u32, u64)>,
}

pub struct Launching<'a>(&'a Multi);

impl Drop for Launching<'_> {
    fn drop(&mut self) {
        self.0.launches.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Singleton {
    _handle: Owned<HANDLE>,
}

// -- SAFETY: a kernel handle is valid on any thread of the process; it is never used after creation except by CloseHandle …
unsafe impl Send for Singleton {}
// -- SAFETY: as above; `&Singleton` gives no access to the handle
unsafe impl Sync for Singleton {}

impl Multi {
    pub fn new() -> Multi {
        Multi { inner: Mutex::new(None), launches: AtomicUsize::new(0) }
    }

    pub fn enable(&self) -> Result<()> {
        self.take(SINGLETON_NAME)
    }

    pub fn take_over(&self) -> Result<()> {
        let pids: Vec<u32> = process::roblox_processes().iter().map(|p| p.pid).collect();
        self.take_over_at(SINGLETON_NAME, &pids)
    }

    pub fn disable(&self) {
        *self.held() = None;
    }

    pub fn is_active(&self) -> bool {
        self.held().is_some()
    }

    pub fn lock_shared(&self) {
        self.lock_shared_at(&shared_files());
    }

    pub fn release_shared(&self) {
        if let Some(h) = self.held().as_mut() {
            h.files.clear();
        }
    }

    pub fn launching(&self, protect: bool) -> Launching<'_> {
        self.launching_at(protect, &shared_files())
    }

    pub fn release_if(&self, idle: impl FnOnce() -> bool) {
        if let Some(h) = self.held().as_mut()
            && !h.files.is_empty()
            && !self.in_flight()
            && !survivors_alive(h)
            && idle()
        {
            h.files.clear();
        }
    }

    pub fn in_flight(&self) -> bool {
        self.launches.load(Ordering::SeqCst) > 0
    }

    pub fn adopt(&self, clients: &[(u32, u64)]) {
        if let Some(h) = self.held().as_mut() {
            h.survivors.retain(|s| !clients.contains(s));
        }
    }

    pub fn has_survivors(&self) -> bool {
        self.held().as_mut().is_some_and(survivors_alive)
    }

    fn launching_at(&self, protect: bool, paths: &[PathBuf]) -> Launching<'_> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        if protect && let Some(h) = self.held().as_mut() {
            relock(h, paths);
        }
        Launching(self)
    }

    fn lock_shared_at(&self, paths: &[PathBuf]) {
        if let Some(h) = self.held().as_mut() {
            relock(h, paths);
        }
    }

    pub fn status(&self, enabled: bool, protect_shared_login: bool) -> MultiInstanceStatus {
        let active = self.is_active();
        let blocked_by: Vec<u32> =
            if active { Vec::new() } else { process::roblox_processes().iter().map(|p| p.pid).collect() };
        let message = match (enabled && !active, blocked_by.is_empty()) {
            (false, _) => None,
            (true, false) => Some(running()),
            (true, true) => Some(tr!("Multi-instance isn't active yet.")),
        };
        MultiInstanceStatus { enabled, active, protect_shared_login, blocked_by, message }
    }

    fn take(&self, name: &str) -> Result<()> {
        let mut held = self.held();
        if held.is_none() {
            let survivors = process::roblox_processes().iter().map(|p| (p.pid, p.start_time)).collect();
            *held = Some(Held { _mutex: create_mutex(name)?, files: Vec::new(), survivors });
        }
        Ok(())
    }

    fn take_over_at(&self, name: &str, pids: &[u32]) -> Result<()> {
        if self.is_active() {
            return Ok(());
        }
        let suffix = format!("\\{name}");
        let table = handle_table(pids)?;
        let mut failed = None;
        for &pid in pids {
            match close_in(pid, &table, &suffix) {
                Ok(n) => log::info!("multi-instance take-over: closed {n} singleton handle(s) in process {pid}"),
                Err(e) => {
                    log::warn!("multi-instance take-over: process {pid}: {e}");
                    failed.get_or_insert(e);
                }
            }
        }
        self.take(name).map_err(|e| match (e, failed) {
            (e, Some(f)) => {
                log::debug!("multi-instance take-over: {name} still taken: {e}");
                f
            }
            (AppError::Invalid(_), None) => AppError::Invalid(still_running()),
            (e, None) => e,
        })
    }

    fn held(&self) -> MutexGuard<'_, Option<Held>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Default for Multi {
    fn default() -> Self {
        Self::new()
    }
}

fn relock(h: &mut Held, paths: &[PathBuf]) {
    if h.files.len() < paths.len() {
        h.files.clear();
        h.files = lock_files(paths);
    }
}

fn survivors_alive(h: &mut Held) -> bool {
    h.survivors.retain(|&(pid, t)| process::start_time(pid) == Some(t));
    !h.survivors.is_empty()
}

fn create_mutex(name: &str) -> Result<Singleton> {
    // -- SAFETY: CreateMutexW only reads the name; on success the handle is ours to close
    let h = unsafe { CreateMutexW(None, false, &HSTRING::from(name)) }.map_err(|e| {
        if e.code() == ERROR_INVALID_HANDLE.to_hresult() {
            AppError::Invalid(running())
        } else {
            AppError::Internal(tr!("Could not take the multi-instance lock: {why}", why = crate::error::windows_error(&e)))
        }
    })?;
    // -- SAFETY: as above
    Ok(Singleton { _handle: unsafe { Owned::new(h) } })
}

fn still_running() -> String {
    tr!("Roblox's lock is still there after the take-over (a client may have just started). Close Roblox instead.")
}

const SYSTEM_EXTENDED_HANDLE_INFORMATION: u32 = 64;
const OBJECT_NAME_INFORMATION: u32 = 1;
const OBJECT_TYPE_INFORMATION: u32 = 2;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut c_void, len: u32, ret_len: *mut u32) -> i32;
    fn NtQueryObject(handle: HANDLE, class: u32, info: *mut c_void, len: u32, ret_len: *mut u32) -> i32;
}

#[repr(C)]
#[allow(dead_code)]
struct HandleEntry {
    object: *mut c_void,
    pid: usize,
    handle: usize,
    access: u32,
    backtrace: u16,
    type_index: u16,
    attributes: u32,
    reserved: u32,
}

#[repr(C)]
#[allow(dead_code)]
struct UnicodeString {
    length: u16,
    max: u16,
    buffer: *const u16,
}

fn handle_table(pids: &[u32]) -> Result<Vec<(u32, usize)>> {
    let mut size = 1usize << 20;
    loop {
        let mut buf = vec![0u64; size / 8];
        let mut need = 0u32;
        // -- SAFETY: `buf` is writable for the length given; `need` is a local
        let st = unsafe {
            NtQuerySystemInformation(SYSTEM_EXTENDED_HANDLE_INFORMATION, buf.as_mut_ptr().cast(), size as u32, &mut need)
        };
        if st == STATUS_INFO_LENGTH_MISMATCH.0 {
            size = (need as usize + (1 << 16)).max(size * 2);
            if size > 1 << 30 {
                return Err(AppError::Internal(tr!("The system's handle table is too large to read.")));
            }
            continue;
        }
        if st < 0 {
            return Err(AppError::Internal(tr!("Could not read the system's handle table (NTSTATUS {status}).", status = format!("{st:#x}"))));
        }
        let count = buf[0] as usize;
        if count.saturating_mul(size_of::<HandleEntry>()).saturating_add(16) > size {
            return Err(AppError::Internal(tr!("The system's handle table came back truncated.")));
        }
        // -- SAFETY: the kernel wrote `count` entries right after the two-word header, inside `buf` (checked above); HandleEntry …
        let entries = unsafe { std::slice::from_raw_parts(buf.as_ptr().add(2).cast::<HandleEntry>(), count) };
        return Ok(entries
            .iter()
            .filter_map(|e| u32::try_from(e.pid).ok().filter(|p| pids.contains(p)).map(|p| (p, e.handle)))
            .collect());
    }
}

// -- Main func
fn close_in(pid: u32, table: &[(u32, usize)], suffix: &str) -> Result<usize> {
    // -- SAFETY: opens the process with the one right DuplicateHandle needs; closed on drop
    let proc = unsafe { OpenProcess(PROCESS_DUP_HANDLE, false, pid) }
        // -- SAFETY: as above
        .map(|h| unsafe { Owned::new(h) })
        .map_err(|e| AppError::Invalid(tr!("Windows wouldn't let LRAM into Roblox (process {pid}): {why}", pid = pid, why = crate::error::windows_error(&e))))?;
    let mut closed = 0;
    for &(_, value) in table.iter().filter(|(p, _)| *p == pid) {
        let source = HANDLE(value as *mut c_void);
        let mut copy = HANDLE::default();
        // -- SAFETY: `proc` has PROCESS_DUP_HANDLE; `source` is a handle value in that process (one closed since just fails); the …
        let ok = unsafe { DuplicateHandle(*proc, source, GetCurrentProcess(), &mut copy, 0, false, DUPLICATE_HANDLE_OPTIONS(0)) };
        if ok.is_err() {
            continue;
        }
        // -- SAFETY: as above
        let copy = unsafe { Owned::new(copy) };
        if !matches!(object_string(*copy, OBJECT_TYPE_INFORMATION).as_deref(), Some("Event" | "Mutant")) {
            continue;
        }
        if !object_string(*copy, OBJECT_NAME_INFORMATION).is_some_and(|n| n.ends_with(suffix)) {
            continue;
        }
        let mut again = HANDLE::default();
        // -- SAFETY: as the first copy
        if unsafe { DuplicateHandle(*proc, source, GetCurrentProcess(), &mut again, 0, false, DUPLICATE_HANDLE_OPTIONS(0)) }.is_err() {
            continue;
        }
        // -- SAFETY: as above
        let again = unsafe { Owned::new(again) };
        // -- SAFETY: both are our own handles
        if !unsafe { CompareObjectHandles(*copy, *again) }.as_bool() {
            continue;
        }
        // -- SAFETY: closes `source` inside `proc` (DUPLICATE_CLOSE_SOURCE, no copy is made); no memory of the process is touched
        let r = unsafe { DuplicateHandle(*proc, source, HANDLE::default(), std::ptr::null_mut(), 0, false, DUPLICATE_CLOSE_SOURCE) };
        match r {
            Ok(()) => closed += 1,
            Err(e) => log::warn!("multi-instance take-over: could not close a handle in process {pid}: {e}"),
        }
    }
    Ok(closed)
}

fn object_string(h: HANDLE, class: u32) -> Option<String> {
    let mut buf = vec![0u64; 512];
    let mut len = 0u32;
    // -- SAFETY: `h` is our own handle; `buf` is writable and pointer-aligned for the length given
    let st = unsafe { NtQueryObject(h, class, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32, &mut len) };
    if st < 0 {
        return None;
    }
    // -- SAFETY: on success both classes start with a UNICODE_STRING whose buffer (if any) points into `buf`, which is alive …
    unsafe {
        let us = &*buf.as_ptr().cast::<UnicodeString>();
        if us.buffer.is_null() {
            return Some(String::new());
        }
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(us.buffer, usize::from(us.length) / 2)))
    }
}

fn lock_files(paths: &[PathBuf]) -> Vec<File> {
    paths
        .iter()
        .filter_map(|p| {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match OpenOptions::new().write(true).create(true).access_mode(GENERIC_READ.0).share_mode(0).open(p) {
                Ok(f) => Some(f),
                Err(e) => {
                    log::warn!("could not lock {}: {e}", p.display());
                    None
                }
            }
        })
        .collect()
}

fn shared_files() -> Vec<PathBuf> {
    let Some(base) = std::env::var_os("LOCALAPPDATA") else {
        log::warn!("LOCALAPPDATA not set: shared Roblox login files not locked");
        return Vec::new();
    };
    let dir = PathBuf::from(base).join("Roblox").join("LocalStorage");
    vec![dir.join("RobloxCookies.dat"), dir.join("appStorage.json")]
}

#[cfg(test)]
mod tests {
    use windows::Win32::Foundation::ERROR_SHARING_VIOLATION;
    use windows::Win32::System::Threading::CreateEventW;

    use super::*;

    fn test_name() -> String {
        format!("LRAM_test_singleton_{}", uuid::Uuid::new_v4())
    }

    fn event(name: &str) -> windows::core::Result<Owned<HANDLE>> {
        // -- SAFETY: CreateEventW only reads the name; the handle is closed on drop
        unsafe { CreateEventW(None, false, false, &HSTRING::from(name)).map(|h| Owned::new(h)) }
    }

    #[test]
    fn holds_and_releases_the_name() {
        let name = test_name();
        let (a, b) = (Multi::new(), Multi::new());
        assert!(!a.status(false, false).active && a.status(false, false).message.is_none());
        a.take(&name).unwrap();
        assert!(a.is_active());
        let s = a.status(true, true);
        assert!(s.active && s.blocked_by.is_empty() && s.message.is_none() && s.protect_shared_login);
        assert_eq!(event(&name).unwrap_err().code(), ERROR_INVALID_HANDLE.to_hresult());
        b.take(&name).unwrap();
        assert!(b.is_active());
        a.disable();
        b.disable();
        assert!(!a.is_active() && !b.is_active());
        event(&name).unwrap();
    }

    #[test]
    fn an_existing_event_means_roblox_runs() {
        let name = test_name();
        let _roblox = event(&name).unwrap();
        let m = Multi::new();
        let err = m.take(&name).unwrap_err();
        assert!(matches!(err, AppError::Invalid(ref s) if *s == running()), "{err}");
        assert!(!m.is_active());
    }

    #[test]
    fn take_over_closes_the_event_in_its_owner_only() {
        let (name, other) = (test_name(), test_name());
        std::mem::forget(event(&name).unwrap());
        let _kept = event(&other).unwrap();
        let m = Multi::new();
        assert!(m.take(&name).is_err());
        m.take_over_at(&name, &[std::process::id()]).unwrap();
        assert!(m.is_active());
        assert!(matches!(create_mutex(&other), Err(AppError::Invalid(ref s)) if *s == running()));
        m.disable();
        let _roblox = event(&name).unwrap();
        let err = Multi::new().take_over_at(&name, &[]).unwrap_err();
        assert!(matches!(err, AppError::Invalid(ref s) if *s == still_running()), "{err}");
    }

    #[test]
    fn survivors_count_only_while_they_run() {
        let m = Multi::new();
        m.take(&test_name()).unwrap();
        let mut child = std::process::Command::new("PING.EXE")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let t = process::start_time(child.id()).unwrap();
        m.held().as_mut().unwrap().survivors = vec![(child.id(), t), (child.id(), t - 5)];
        assert!(m.has_survivors());
        assert_eq!(m.held().as_ref().unwrap().survivors.len(), 1);
        m.adopt(&[(child.id(), t)]);
        assert!(!m.has_survivors(), "followed again: LRAM's own");
        m.held().as_mut().unwrap().survivors = vec![(child.id(), t)];
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!m.has_survivors());
        m.disable();
    }

    #[test]
    fn locks_shared_files_while_held_and_creates_missing_ones() {
        let dir = std::env::temp_dir().join(format!("lram-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("RobloxCookies.dat");
        std::fs::write(&file, b"{}").unwrap();
        let missing = dir.join("LocalStorage").join("appStorage.json");
        let files = [file.clone(), missing.clone()];
        let locked = |p: &PathBuf| File::open(p).unwrap_err().raw_os_error() == Some(ERROR_SHARING_VIOLATION.0 as i32);

        let m = Multi::new();
        m.lock_shared_at(&files);
        File::open(&file).unwrap();
        let name = test_name();
        m.take(&name).unwrap();
        m.held().as_mut().unwrap().survivors.clear();
        m.lock_shared_at(&files);
        assert!(locked(&file) && locked(&missing));
        m.lock_shared_at(&files);
        assert!(locked(&file) && locked(&missing));
        let other = Multi::new();
        other.take(&test_name()).unwrap();
        other.lock_shared_at(&files);
        assert!(other.is_active());

        let launch = m.launching_at(true, &files);
        m.release_if(|| true);
        assert!(locked(&file));
        drop(launch);
        m.release_if(|| false);
        assert!(locked(&file));
        m.release_if(|| true);
        File::open(&file).unwrap();
        let launch = m.launching_at(false, &files);
        m.disable();
        m.take(&name).unwrap();
        m.held().as_mut().unwrap().survivors.clear();
        assert!(m.in_flight());
        drop(launch);
        assert!(!m.in_flight());
        m.lock_shared_at(&files);

        m.release_shared();
        assert_eq!(std::fs::read(&file).unwrap(), b"{}");
        assert!(m.is_active());
        m.lock_shared_at(&files);
        assert!(locked(&file));
        m.disable();
        File::open(&file).unwrap();
        other.disable();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
