use std::thread::sleep;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    E_ACCESSDENIED, ERROR_INVALID_PARAMETER, FILETIME, HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetForegroundWindow, GetWindow, GetWindowThreadProcessId, IsHungAppWindow, IsIconic, IsWindowVisible,
    PostMessageW, WM_CLOSE,
};
use windows::core::{BOOL, Owned};

use crate::error::{AppError, Result};

const CLIENT_EXE: &str = "RobloxPlayerBeta.exe";
const INSTALLER_EXE: &str = "RobloxPlayerInstaller.exe";
const UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    pub start_time: u64,
}

pub fn roblox_processes() -> Vec<ProcInfo> {
    named(CLIENT_EXE)
}

pub fn installer_running() -> bool {
    !named(INSTALLER_EXE).is_empty()
}

fn named(exe: &str) -> Vec<ProcInfo> {
    let mut out = Vec::new();
    // -- SAFETY: the snapshot handle is owned and closed on drop; `e` is a zeroed PROCESSENTRY32W with dwSize set, as …
    unsafe {
        let snap = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(snap) => snap,
            Err(e) => {
                static ONCE: std::sync::Once = std::sync::Once::new();
                ONCE.call_once(|| log::warn!("could not list processes: {e}"));
                return out;
            }
        };
        let snap = Owned::new(snap);
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(*snap, &mut e).is_ok();
        while more {
            if is_named(&e.szExeFile, exe) {
                match start_time(e.th32ProcessID) {
                    Some(start_time) => out.push(ProcInfo { pid: e.th32ProcessID, start_time }),
                    None => log::debug!("{exe} process {} can't be opened; left out", e.th32ProcessID),
                }
            }
            more = Process32NextW(*snap, &mut e).is_ok();
        }
    }
    out
}

pub fn start_time(pid: u32) -> Option<u64> {
    started(*open(pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE)?)
}

pub fn unreadable_client(pid: u32) -> bool {
    // -- SAFETY: `h` is a live handle with PROCESS_SYNCHRONIZE
    if open(pid, PROCESS_SYNCHRONIZE).is_some_and(|h| unsafe { WaitForSingleObject(*h, 0) } == WAIT_OBJECT_0) {
        return false;
    }
    listed_as(pid, CLIENT_EXE)
}

fn listed_as(pid: u32, exe: &str) -> bool {
    // -- SAFETY: as in `named`
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return false };
        let snap = Owned::new(snap);
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(*snap, &mut e).is_ok();
        while more {
            if e.th32ProcessID == pid {
                return is_named(&e.szExeFile, exe);
            }
            more = Process32NextW(*snap, &mut e).is_ok();
        }
    }
    false
}

pub fn close(pid: u32, expected: Option<u64>, force: bool, grace: Duration) -> Result<()> {
    let seen = start_time(pid);
    if seen.is_none() && unreadable_client(pid) {
        post_close(pid);
        return Err(AppError::Internal(tr!("Could not close Roblox (process {pid}): {why}", pid = pid, why = tr!("access denied"))));
    }
    let Some(t0) = seen.filter(|t| expected.is_none_or(|e| e == *t)) else { return Ok(()) };
    let alive = || start_time(pid) == Some(t0);
    if !force {
        let deadline = Instant::now() + grace;
        post_close(pid);
        sleep(Duration::from_millis(400));
        if alive() {
            post_close(pid);
        }
        while alive() && Instant::now() < deadline {
            sleep(Duration::from_millis(100));
        }
        if !alive() {
            return Ok(());
        }
    }
    terminate(pid, t0)
}

fn terminate(pid: u32, t0: u64) -> Result<()> {
    let failed = |e: String| AppError::Internal(tr!("Could not close Roblox (process {pid}): {why}", pid = pid, why = e));
    let Some(h) = open(pid, PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE) else {
        return if start_time(pid) == Some(t0) { Err(failed(tr!("access denied"))) } else { Ok(()) };
    };
    if started(*h) != Some(t0) {
        return Ok(());
    }
    // -- SAFETY: `h` is a live handle with PROCESS_TERMINATE | SYNCHRONIZE, closed on drop
    unsafe {
        if let Err(e) = TerminateProcess(*h, 1)
            && started(*h).is_some()
            && WaitForSingleObject(*h, 3_000) == WAIT_TIMEOUT
        {
            return Err(failed(crate::error::windows_error(&e)));
        }
        if WaitForSingleObject(*h, 3_000) == WAIT_TIMEOUT {
            log::debug!("process {pid} still not gone 3 s after TerminateProcess");
        }
    }
    Ok(())
}

fn post_close(pid: u32) {
    unsafe extern "system" fn each(hwnd: HWND, pid: LPARAM) -> BOOL {
        let mut owner = 0u32;
        // -- SAFETY: `hwnd` comes from EnumWindows; PostMessageW only queues a message
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&raw mut owner));
            if owner as isize == pid.0 {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        true.into()
    }
    // -- SAFETY: `each` has the WNDENUMPROC signature and only uses its arguments
    let _ = unsafe { EnumWindows(Some(each), LPARAM(pid as isize)) };
}

pub fn hung(pid: u32) -> bool {
    struct Find {
        pid: u32,
        hung: bool,
    }
    unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> BOOL {
        // -- SAFETY: `data` is the `&mut Find` passed to EnumWindows below, alive for the whole call
        let f = unsafe { &mut *(data.0 as *mut Find) };
        let mut owner = 0u32;
        // -- SAFETY: `hwnd` comes from EnumWindows; these calls only read the window's state
        let hung = unsafe {
            GetWindowThreadProcessId(hwnd, Some(&raw mut owner));
            owner == f.pid && IsWindowVisible(hwnd).as_bool() && IsHungAppWindow(hwnd).as_bool()
        };
        f.hung |= hung;
        (!hung).into()
    }
    let mut f = Find { pid, hung: false };
    // -- SAFETY: `each` has the WNDENUMPROC signature; `f` outlives the call
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&raw mut f as isize)) };
    f.hung
}

pub fn main_window(pid: u32) -> Option<(HWND, bool)> {
    struct Find {
        pid: u32,
        found: Option<HWND>,
    }
    unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> BOOL {
        // -- SAFETY: `data` is the `&mut Find` passed to EnumWindows below, alive for the whole call
        let f = unsafe { &mut *(data.0 as *mut Find) };
        let mut owner = 0u32;
        // -- SAFETY: `hwnd` comes from EnumWindows; these calls only read the window's state
        let main = unsafe {
            GetWindowThreadProcessId(hwnd, Some(&raw mut owner));
            owner == f.pid && IsWindowVisible(hwnd).as_bool() && GetWindow(hwnd, GW_OWNER).is_err()
        };
        if main {
            f.found = Some(hwnd);
        }
        (!main).into()
    }
    let mut f = Find { pid, found: None };
    // -- SAFETY: `each` has the WNDENUMPROC signature; `f` outlives the call
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&raw mut f as isize)) };
    // -- SAFETY: reads one window's state
    f.found.map(|h| (h, unsafe { IsIconic(h) }.as_bool()))
}

pub fn has_any_window(pid: u32) -> bool {
    struct Find {
        pid: u32,
        found: bool,
    }
    unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> BOOL {
        // -- SAFETY: `data` is the `&mut Find` passed to EnumWindows below, alive for the whole call
        let f = unsafe { &mut *(data.0 as *mut Find) };
        let mut owner = 0u32;
        // -- SAFETY: `hwnd` comes from EnumWindows; this call only reads which process owns it
        let _ = unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut owner)) };
        f.found = owner == f.pid;
        (!f.found).into()
    }
    let mut f = Find { pid, found: false };
    // -- SAFETY: `each` has the WNDENUMPROC signature; `f` outlives the call
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&raw mut f as isize)) };
    f.found
}

pub fn foreground_pid() -> Option<u32> {
    let mut pid = 0u32;
    // -- SAFETY: reads which window is in front and its owner into a local; no handle is opened
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        GetWindowThreadProcessId(hwnd, Some(&raw mut pid));
    }
    (pid != 0).then_some(pid)
}

// -- Main func
pub(super) fn open(pid: u32, rights: PROCESS_ACCESS_RIGHTS) -> Option<Owned<HANDLE>> {
    // -- SAFETY: on success the handle is ours; Owned closes it on drop
    match unsafe { OpenProcess(rights, false, pid) } {
        // -- SAFETY: as above
        Ok(h) => Some(unsafe { Owned::new(h) }),
        Err(e) => {
            if e.code() == ERROR_INVALID_PARAMETER.to_hresult() {
                return None;
            }
            if e.code() == E_ACCESSDENIED {
                static ONCE: std::sync::Once = std::sync::Once::new();
                ONCE.call_once(|| log::warn!("process {pid}: access denied (is Roblox running as administrator?)"));
            } else {
                log::debug!("could not open process {pid}: {}", crate::error::windows_error(&e));
            }
            None
        }
    }
}

pub(super) fn started(h: HANDLE) -> Option<u64> {
    let (mut created, mut exited, mut kernel, mut user) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // -- SAFETY: `h` is a live handle with PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE; the out pointers are locals
    unsafe {
        let wait = WaitForSingleObject(h, 0);
        if wait != WAIT_TIMEOUT {
            if wait != WAIT_OBJECT_0 {
                log::debug!("process wait failed: {wait:?}");
            }
            return None;
        }
        GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user)
            .inspect_err(|e| log::debug!("process times unreadable: {}", crate::error::windows_error(e)))
            .ok()?;
    }
    let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    Some(ticks.saturating_sub(UNIX_EPOCH_TICKS) / 10_000_000)
}

fn is_named(exe: &[u16], name: &str) -> bool {
    let len = exe.iter().position(|&c| c == 0).unwrap_or(exe.len());
    String::from_utf16_lossy(&exe[..len]).eq_ignore_ascii_case(name)
}

#[cfg(test)]
mod tests {
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn spawn(exe: &str, args: &[&str]) -> Child {
        let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| r"C:\Windows".into());
        Command::new(root.join("System32").join(exe))
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .unwrap()
    }

    fn sleeper() -> Child {
        spawn("PING.EXE", &["-n", "30", "127.0.0.1"])
    }

    #[test]
    fn start_time_of_live_and_dead_processes() {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let own = start_time(std::process::id()).unwrap();
        assert!(own <= now && own > now - 3600, "{own} vs {now}");
        assert_eq!(start_time(u32::MAX), None);
        let mut done = spawn("cmd.exe", &["/c", "exit", "0"]);
        done.wait().unwrap();
        assert_eq!(start_time(done.id()), None);
    }

    #[test]
    fn force_close_terminates() {
        let mut child = sleeper();
        let pid = child.id();
        let t0 = start_time(pid);
        assert!(t0.is_some());
        close(pid, t0, true, Duration::from_secs(5)).unwrap();
        assert_eq!(child.try_wait().unwrap().and_then(|s| s.code()), Some(1));
        close(pid, t0, true, Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn a_process_started_at_another_time_is_left_alone() {
        let mut child = sleeper();
        let pid = child.id();
        let other = start_time(pid).map(|t| t - 10);
        close(pid, other, true, Duration::from_secs(5)).unwrap();
        assert!(child.try_wait().unwrap().is_none(), "a reused pid must not be closed");
        close(pid, None, true, Duration::from_secs(5)).unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn graceful_close_without_windows_terminates_after_grace() {
        let mut child = sleeper();
        let begun = Instant::now();
        close(child.id(), None, false, Duration::from_millis(200)).unwrap();
        assert!(begun.elapsed() >= Duration::from_millis(400));
        assert_eq!(child.try_wait().unwrap().and_then(|s| s.code()), Some(1));
    }

    #[test]
    fn windowless_and_missing_processes_are_not_hung() {
        let mut child = sleeper();
        assert!(!hung(child.id()));
        assert!(!hung(std::process::id()));
        assert!(!hung(u32::MAX));
        assert_eq!(main_window(child.id()), None);
        assert_eq!(main_window(u32::MAX), None);
        close(child.id(), None, true, Duration::from_secs(5)).unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn roblox_processes_does_not_panic() {
        let _ = roblox_processes();
        let utf16 = |s: &str| s.encode_utf16().collect::<Vec<_>>();
        assert!(is_named(&utf16("robloxplayerbeta.EXE\0junk"), CLIENT_EXE));
        assert!(!is_named(&utf16("RobloxPlayerBeta.exe.bak"), CLIENT_EXE));
        assert!(is_named(&utf16("RobloxPlayerInstaller.exe\0"), INSTALLER_EXE));
        let _ = installer_running();
    }
}

/// Read-only ownership query for a Roblox log already matched to a launch tracker ID.
/// Never closes handles, shuts down applications or reads Roblox process memory.
pub fn log_owners(path: &std::path::Path) -> Vec<ProcInfo> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows::Win32::System::RestartManager::{RmStartSession, RmRegisterResources, RmGetList, RmEndSession, RM_PROCESS_INFO};
    use windows::core::{PCWSTR, PWSTR};
    struct Session(u32);
    impl Drop for Session {
        fn drop(&mut self) {
            // SAFETY: this session was successfully opened below; no shutdown/restart API is used.
            unsafe { let _ = RmEndSession(self.0); }
        }
    }
    let mut handle = 0;
    let mut key = [0u16; 33];
    // SAFETY: the session key buffer has CCH_RM_SESSION_KEY + 1 UTF-16 code units.
    if unsafe { RmStartSession(&mut handle, None, PWSTR(key.as_mut_ptr())) } != ERROR_SUCCESS { return Vec::new(); }
    let session = Session(handle);
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let files = [PCWSTR(name.as_ptr())];
    // SAFETY: the file name remains alive through registration; all outputs are local allocations.
    unsafe {
        if RmRegisterResources(session.0, Some(&files), None, None) != ERROR_SUCCESS { return Vec::new(); }
        let (mut needed, mut count, mut reasons) = (0, 0, 0);
        if RmGetList(session.0, &mut needed, &mut count, None, &mut reasons) != ERROR_MORE_DATA || needed > 256 { return Vec::new(); }
        let mut apps = vec![RM_PROCESS_INFO::default(); needed as usize];
        count = needed;
        if RmGetList(session.0, &mut needed, &mut count, Some(apps.as_mut_ptr()), &mut reasons) != ERROR_SUCCESS { return Vec::new(); }
        apps.truncate((count as usize).min(apps.len()));
        apps.into_iter().map(|a| {
            let time = a.Process.ProcessStartTime;
            let ticks = (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
            ProcInfo { pid: a.Process.dwProcessId, start_time: ticks.saturating_sub(UNIX_EPOCH_TICKS) / 10_000_000 }
        }).collect()
    }
}
