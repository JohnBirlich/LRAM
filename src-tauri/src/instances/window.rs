use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    GW_HWNDNEXT, GW_OWNER, GWL_EXSTYLE, GWL_STYLE, GetClientRect, GetForegroundWindow, GetLayeredWindowAttributes, GetShellWindow, GetWindow, GetWindowLongPtrW, GetWindowPlacement,
    GetWindowRect, HWND_BOTTOM, HWND_TOP, IsIconic, IsWindow, IsWindowVisible, IsZoomed, LAYERED_WINDOW_ATTRIBUTES_FLAGS, LWA_ALPHA, SWP_ASYNCWINDOWPOS, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_SHOWMAXIMIZED, SW_SHOWMINIMIZED, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, SetLayeredWindowAttributes,
    SetWindowLongPtrW, SetWindowPlacement, SetWindowPos, ShowWindow, ShowWindowAsync, WINDOWPLACEMENT, WPF_RESTORETOMAXIMIZED, WS_CAPTION, WS_EX_LAYERED, WS_EX_TRANSPARENT,
    WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
};

use super::{afk, process};
use crate::error::{AppError, Result};

pub struct Thumb {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Show {
    Focus,
    Minimize,
    Restore,
}

#[derive(Clone)]
pub struct Shot {
    pub thumb: Arc<Thumb>,
    pub seq: u32,
    pub at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Live = 0,
    Minimized = 1,
    NoWindow = 2,
    Pending = 3,
    Unsupported = 4,
}

pub fn status_of(minimized: Option<bool>, has_shot: bool, unsupported: bool) -> Status {
    match (minimized, has_shot, unsupported) {
        (None, ..) => Status::NoWindow,
        (Some(true), ..) => Status::Minimized,
        (Some(false), true, _) => Status::Live,
        (Some(false), false, true) => Status::Unsupported,
        (Some(false), false, false) => Status::Pending,
    }
}

pub struct Preview {
    pub status: Status,
    pub hidden: bool,
    pub shot: Option<Shot>,
}

impl Preview {
    pub fn to_bytes(&self, known: u32) -> Vec<u8> {
        let (w, h, seq, age) = self.shot.as_ref().map_or((0, 0, 0, 0), |s| {
            (s.thumb.width, s.thumb.height, s.seq, u32::try_from(s.at.elapsed().as_millis()).unwrap_or(u32::MAX))
        });
        let rgba = self.shot.as_ref().filter(|s| s.seq != known).map_or(&[][..], |s| &s.thumb.rgba);
        let mut b = Vec::with_capacity(24 + rgba.len());
        for v in [w, h, self.status as u32, u32::from(self.hidden), seq, age] {
            b.extend(v.to_le_bytes());
        }
        b.extend_from_slice(rgba);
        b
    }
}

pub fn preview(pid: Option<u32>, max_w: u32) -> Preview {
    let Some((pid, hwnd, iconic)) = pid.and_then(|pid| process::main_window(pid).map(|(hwnd, iconic)| (pid, hwnd, iconic))) else {
        return Preview { status: status_of(None, false, false), hidden: false, shot: None };
    };
    let unsupported = super::capture::unsupported();
    if !LIVE_HIDDEN.load(Ordering::Relaxed) && (iconic || is_hidden(hwnd)) {
        let shot = super::capture::last(hwnd);
        return Preview { status: status_of(Some(iconic), shot.is_some(), unsupported), hidden: is_hidden(hwnd), shot };
    }
    wanted().insert(hwnd.0 as isize, Instant::now());
    let waking = iconic && may_wake(pid);
    if waking {
        wake(pid, hwnd, false);
    } else if !iconic {
        super::capture::request(hwnd, max_w);
    }
    let shot = super::capture::last(hwnd);
    Preview { status: status_of(Some(iconic && !waking), shot.is_some(), unsupported), hidden: is_hidden(hwnd), shot }
}

const GHOST: u32 = WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0;
const GHOST_ALPHA: u8 = 1;
const OLD_PARK_LEFT: i32 = -15_000;
const OLD_PARK_BACK: (i32, i32) = (100, 100);
const WATCH: Duration = Duration::from_secs(5);
const RESTORE_TRIES: u32 = 3;
const RESTORE_GAP: Duration = Duration::from_millis(30);
const RESTORE_WAITS: u32 = 66;
const MOST_WAKING: usize = 4;
const SHOW_ALL_WAIT: Duration = Duration::from_secs(3);

static WANTED: LazyLock<Mutex<HashMap<isize, Instant>>> = LazyLock::new(Mutex::default);
static WAKING: Mutex<Vec<isize>> = Mutex::new(Vec::new());
static TINY: Mutex<Vec<(u32, (u32, u32))>> = Mutex::new(Vec::new());

static LIVE_HIDDEN: AtomicBool = AtomicBool::new(true);
static TINY_TRIED: LazyLock<Mutex<HashMap<u32, Instant>>> = LazyLock::new(Mutex::default);
const TINY_RETRY: Duration = Duration::from_secs(60);
static TINY_FIXING: Mutex<Vec<isize>> = Mutex::new(Vec::new());
static RESIZING: Mutex<Vec<isize>> = Mutex::new(Vec::new());
const RESIZE_WAITS: u32 = 100;
const RESIZE_GAP: Duration = Duration::from_millis(20);

fn size_set_now(pid: u32) {
    TINY_TRIED.lock().unwrap_or_else(|e| e.into_inner()).insert(pid, Instant::now());
}

pub fn set_live_hidden_previews(on: bool) {
    LIVE_HIDDEN.store(on, Ordering::Relaxed);
}

fn wanted() -> MutexGuard<'static, HashMap<isize, Instant>> {
    WANTED.lock().unwrap_or_else(|e| e.into_inner())
}

fn tiny() -> MutexGuard<'static, Vec<(u32, (u32, u32))>> {
    TINY.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_tiny(pid: u32, size: Option<(u32, u32)>) {
    size_set_now(pid);
    let mut t = tiny();
    t.retain(|e| e.0 != pid);
    if let Some(size) = size {
        t.push((pid, size));
    }
}

fn ghostly(ex: u32, alpha: u8) -> bool {
    ex & GHOST == GHOST && alpha == GHOST_ALPHA
}

fn has_ghost_bits(hwnd: HWND) -> bool {
    // -- SAFETY: reads one window's style; no message reaches the client
    unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & GHOST == GHOST }
}

pub fn is_hidden(hwnd: HWND) -> bool {
    let (mut alpha, mut flags) = (0u8, LAYERED_WINDOW_ATTRIBUTES_FLAGS(0));
    // -- SAFETY: reads one window's style and layering into locals; no message reaches the client
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        ex & GHOST == GHOST
            && GetLayeredWindowAttributes(hwnd, None, Some(&mut alpha), Some(&mut flags)).is_ok()
            && flags.0 & LWA_ALPHA.0 != 0
            && ghostly(ex, alpha)
    }
}

fn ghost(hwnd: HWND) -> bool {
    if is_hidden(hwnd) {
        return true;
    }
    let_go_of_focus(hwnd);
    // -- SAFETY: window-manager calls on a top-level window
    unsafe {
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 | GHOST) as isize);
        if let Err(e) = SetLayeredWindowAttributes(hwnd, COLORREF(0), GHOST_ALPHA, LWA_ALPHA) {
            log::debug!("ghost: alpha not set: {e}");
            clear_ghost(hwnd);
            let _ = ShowWindowAsync(hwnd, SW_SHOWMINNOACTIVE);
            return false;
        }
    }
    sink(hwnd);
    true
}

fn sink(hwnd: HWND) {
    // -- SAFETY: a window-manager call on a top-level window; queued
    let _ = unsafe { SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS) };
}

pub fn back_down(hwnd: HWND) {
    if is_hidden(hwnd) {
        let_go_of_focus(hwnd);
        sink(hwnd);
    }
}

fn opaque(hwnd: HWND) {
    // -- SAFETY: a window-manager call on a top-level layered window
    if let Err(e) = unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA) } {
        log::debug!("alpha not reset: {e}");
    }
}

fn clear_ghost(hwnd: HWND) {
    // -- SAFETY: changes one top-level window's extended style
    unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & !GHOST) as isize) };
}

fn let_go_of_focus(hwnd: HWND) {
    // -- SAFETY: reads windows' state only
    unsafe {
        if GetForegroundWindow() != hwnd {
            return;
        }
        let mut next = GetWindow(hwnd, GW_HWNDNEXT);
        while let Ok(w) = next {
            if IsWindowVisible(w).as_bool() && !IsIconic(w).as_bool() && GetWindow(w, GW_OWNER).is_err() && !is_hidden(w) {
                afk::to_front(w);
                return;
            }
            next = GetWindow(w, GW_HWNDNEXT);
        }
        let desktop = GetShellWindow();
        if !desktop.is_invalid() {
            afk::to_front(desktop);
        }
    }
}

fn hide(pid: u32, hwnd: HWND) {
    if process::hung(pid) {
        // -- SAFETY: a window-manager call on a top-level window; posted
        let _ = unsafe { ShowWindowAsync(hwnd, SW_SHOWMINNOACTIVE) };
        return;
    }
    ghost(hwnd);
}

fn may_wake(pid: u32) -> bool {
    !super::capture::unsupported() && !process::hung(pid)
}

fn un_iconify(hwnd: HWND, landing: bool) {
    for n in 0..RESTORE_TRIES + RESTORE_WAITS {
        let trying = n < RESTORE_TRIES;
        // -- SAFETY: reads one window's state, and a window-manager call on it; posted
        unsafe {
            let iconic = IsIconic(hwnd).as_bool();
            if !iconic && !(landing && trying) {
                return;
            }
            if iconic && trying {
                let _ = ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE);
            }
        }
        std::thread::sleep(RESTORE_GAP);
    }
}

pub fn user_sized(pid: u32, hwnd: HWND) {
    // -- SAFETY: reads one window's state
    if unsafe { IsIconic(hwnd).as_bool() || IsZoomed(hwnd).as_bool() } {
        return;
    }
    let Some(size) = client_size(hwnd) else { return };
    if let Some(e) = tiny().iter_mut().find(|e| e.0 == pid) {
        e.1 = size;
    }
    if let Some(e) = normal_size().iter_mut().find(|e| e.pid == pid) {
        e.size = size;
        e.full = false;
    }
}

fn back_to_tiny(pid: u32, hwnd: HWND) {
    let Some(size) = tiny().iter().find(|e| e.0 == pid).map(|e| e.1) else { return };
    // -- SAFETY: reads one window's state
    if unsafe { IsIconic(hwnd).as_bool() || IsZoomed(hwnd).as_bool() } || client_size(hwnd) == Some(size) {
        return;
    }
    size_set_now(pid);
    let (fw, fh) = frame(hwnd).unwrap_or((0, 0));
    free_resize(hwnd, (0, 0, size.0 as i32 + fw, size.1 as i32 + fh), true);
}

fn waking() -> MutexGuard<'static, Vec<isize>> {
    WAKING.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn wake(pid: u32, hwnd: HWND, landing: bool) {
    let h = hwnd.0 as isize;
    {
        let mut w = waking();
        if w.contains(&h) || w.len() >= MOST_WAKING {
            return;
        }
        w.push(h);
    }
    std::thread::spawn(move || {
        let hwnd = HWND(h as *mut _);
        if may_wake(pid) && ghost(hwnd) {
            let still = waking().contains(&h);
            if still {
                un_iconify(hwnd, landing);
                back_to_tiny(pid, hwnd);
            } else {
                clear_ghost(hwnd);
            }
        }
        waking().retain(|w| *w != h);
    });
}

pub fn wake_for_press(pid: u32, hwnd: HWND) -> bool {
    let h = hwnd.0 as isize;
    if process::hung(pid) {
        return false;
    }
    wanted().insert(h, Instant::now());
    {
        let mut w = waking();
        if !w.contains(&h) {
            w.push(h);
        }
    }
    let woke = ghost(hwnd) && waking().contains(&h);
    if woke {
        un_iconify(hwnd, false);
        back_to_tiny(pid, hwnd);
    }
    waking().retain(|w| *w != h);
    wanted().insert(h, Instant::now());
    // -- SAFETY: reads one window's state
    woke && !unsafe { IsIconic(hwnd) }.as_bool()
}

pub fn watched(hwnd: HWND) -> bool {
    asked_within(wanted().get(&(hwnd.0 as isize)).copied())
}

fn asked_within(at: Option<Instant>) -> bool {
    at.is_some_and(|t| t.elapsed() < WATCH)
}

fn keep_tiny(pid: u32, hwnd: HWND) {
    let Some(size) = tiny().iter().find(|e| e.0 == pid).map(|e| e.1) else { return };
    let h = hwnd.0 as isize;
    let recent = TINY_TRIED.lock().unwrap_or_else(|e| e.into_inner()).get(&pid).is_some_and(|t| t.elapsed() < TINY_RETRY);
    // -- SAFETY: reads which window is in front and one window's state
    if recent || client_size(hwnd) == Some(size) || unsafe { GetForegroundWindow() == hwnd || IsZoomed(hwnd).as_bool() } || waking().contains(&h) || process::hung(pid) {
        return;
    }
    {
        let mut fixing = TINY_FIXING.lock().unwrap_or_else(|e| e.into_inner());
        if fixing.contains(&h) || fixing.len() >= MOST_WAKING {
            return;
        }
        fixing.push(h);
    }
    log::info!("client {pid}: tiny but {:?}: putting it back to {size:?}", client_size(hwnd));
    std::thread::spawn(move || {
        let hwnd = HWND(h as *mut _);
        back_to_tiny(pid, hwnd);
        if let Some(now) = client_size(hwnd).filter(|now| *now != size)
            && let Some(e) = tiny().iter_mut().find(|e| e.0 == pid && e.1 == size)
        {
            log::info!("client {pid}: stays at {now:?}, not {size:?}");
            e.1 = now;
        }
        TINY_FIXING.lock().unwrap_or_else(|e| e.into_inner()).retain(|f| *f != h);
    });
}

pub fn sweep(clients: &[(u32, Option<(u32, u32)>)]) {
    {
        let mut t = tiny();
        t.retain(|e| clients.iter().any(|c| c.0 == e.0 && c.1.is_some()));
        for &(pid, size) in clients {
            if let Some(size) = size
                && !t.iter().any(|e| e.0 == pid)
            {
                t.push((pid, size));
            }
        }
    }
    // -- SAFETY: only asks whether a handle still is a window
    wanted().retain(|h, _| unsafe { IsWindow(Some(HWND(*h as *mut _))) }.as_bool());
    TINY_TRIED.lock().unwrap_or_else(|e| e.into_inner()).retain(|pid, _| clients.iter().any(|c| c.0 == *pid));
    normal_size().retain(|e| clients.iter().any(|c| c.0 == e.pid));
    for &(pid, _) in clients {
        if let Some((hwnd, false)) = process::main_window(pid) {
            keep_tiny(pid, hwnd);
            note_size(pid, hwnd);
        }
        if let Some((hwnd, iconic)) = process::main_window(pid)
            && !iconic
            && is_hidden(hwnd)
            && !watched(hwnd)
            // -- SAFETY: reads which window is in front
            && unsafe { GetForegroundWindow() } != hwnd
        {
            log::debug!("client {pid}: hidden and nobody watches its preview: going to sleep (anti-AFK's Focus mode skips it now)");
            // -- SAFETY: a window-manager call on a top-level window; posted
            let _ = unsafe { ShowWindowAsync(hwnd, SW_SHOWMINNOACTIVE) };
        }
    }
}

pub fn unhide(pid: u32, hwnd: HWND) {
    waking().retain(|w| *w != hwnd.0 as isize);
    // -- SAFETY: reads one window's state
    let iconic = unsafe { IsIconic(hwnd) }.as_bool();
    if !iconic && !has_ghost_bits(hwnd) {
        return;
    }
    if process::hung(pid) {
        if iconic {
            // -- SAFETY: a window-manager call on a top-level window; posted
            let _ = unsafe { ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE) };
        }
        let h = hwnd.0 as isize;
        std::thread::spawn(move || clear_ghost(HWND(h as *mut _)));
        return;
    }
    un_iconify(hwnd, false);
    back_to_tiny(pid, hwnd);
    back_to_cell(pid, hwnd);
    waking().retain(|w| *w != hwnd.0 as isize);
    clear_ghost(hwnd);
    // -- SAFETY: a window-manager call on a top-level window; queued
    let _ = unsafe { SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS) };
}

fn all_at_once<T: Send + 'static>(items: Vec<T>, limit: Duration, each: fn(T)) -> bool {
    let (done, finished) = std::sync::mpsc::channel();
    let left = items.len();
    for item in items {
        let done = done.clone();
        std::thread::spawn(move || {
            each(item);
            let _ = done.send(());
        });
    }
    let until = Instant::now() + limit;
    (0..left).all(|_| finished.recv_timeout(until.saturating_duration_since(Instant::now())).is_ok())
}

pub fn show_all(pids: &[u32], what: Show) {
    let items: Vec<(u32, Show)> = pids.iter().map(|&pid| (pid, what)).collect();
    all_at_once(items, SHOW_ALL_WAIT, |(pid, what)| {
        if let Err(e) = show(pid, what) {
            log::debug!("show all: client {pid}: {e}");
        }
    });
}

pub fn unhide_quickly(pids: &[u32], limit: Duration) {
    let ghosts: Vec<isize> = pids
        .iter()
        .filter_map(|&pid| process::main_window(pid))
        .filter(|(hwnd, _)| has_ghost_bits(*hwnd))
        .map(|(hwnd, _)| {
            opaque(hwnd);
            hwnd.0 as isize
        })
        .collect();
    if !all_at_once(ghosts, limit, |h| clear_ghost(HWND(h as *mut _))) {
        log::warn!("a hidden client didn't answer: it takes no clicks until LRAM starts again");
    }
}

pub fn recover(followed: &[u32]) {
    let clients: Vec<(u32, bool)> = process::roblox_processes().into_iter().map(|p| (p.pid, followed.contains(&p.pid))).collect();
    all_at_once(clients, Duration::ZERO, recover_one);
}

fn recover_one((pid, followed): (u32, bool)) {
    let Some((hwnd, iconic)) = process::main_window(pid) else { return };
    if has_ghost_bits(hwnd) && !(followed && is_hidden(hwnd)) {
        log::info!("client {pid} was left hidden: showing it");
        clear_ghost(hwnd);
    }
    if iconic {
        let mut wp = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        // -- SAFETY: reads and sets one window's placement, through a local with length set
        unsafe {
            if GetWindowPlacement(hwnd, &mut wp).is_ok() && wp.rcNormalPosition.left <= OLD_PARK_LEFT {
                log::info!("client {pid} would come back off-screen: moving it back");
                wp.rcNormalPosition = back_from_park(wp.rcNormalPosition);
                wp.showCmd = SW_SHOWMINNOACTIVE.0 as u32;
                let _ = SetWindowPlacement(hwnd, &wp);
            }
        }
        return;
    }
    let mut r = RECT::default();
    // -- SAFETY: reads one window's rectangle into a local
    if unsafe { GetWindowRect(hwnd, &mut r) }.is_ok() && r.left <= OLD_PARK_LEFT {
        log::info!("client {pid} was left off-screen: moving it back");
        // -- SAFETY: a window-manager call on a top-level window; queued
        let _ = unsafe { SetWindowPos(hwnd, None, OLD_PARK_BACK.0, OLD_PARK_BACK.1, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS) };
    }
}

fn back_from_park(r: RECT) -> RECT {
    let (x, y) = OLD_PARK_BACK;
    RECT { left: x, top: y, right: x + r.right - r.left, bottom: y + r.bottom - r.top }
}

// -- Main func
pub fn show(pid: u32, what: Show) -> Result<()> {
    let (hwnd, _) = process::main_window(pid).ok_or_else(|| AppError::Invalid(tr!("That client has no window yet.")))?;
    match what {
        Show::Minimize => hide(pid, hwnd),
        Show::Restore => unhide(pid, hwnd),
        Show::Focus => {
            let hwnd = window_to_change(pid)?;
            unhide(pid, hwnd);
            if !afk::to_front(hwnd) {
                return Err(AppError::Invalid(tr!("Windows didn't let LRAM bring that client to the front.")));
            }
        }
    }
    Ok(())
}

static SAVED: Mutex<Vec<(String, u32, WINDOWPLACEMENT)>> = Mutex::new(Vec::new());

fn saved() -> MutexGuard<'static, Vec<(String, u32, WINDOWPLACEMENT)>> {
    SAVED.lock().unwrap_or_else(|e| e.into_inner())
}

static NORMAL_SIZE: Mutex<Vec<Normal>> = Mutex::new(Vec::new());

struct Normal {
    pid: u32,
    size: (u32, u32),
    full: bool,
}

fn normal_size() -> MutexGuard<'static, Vec<Normal>> {
    NORMAL_SIZE.lock().unwrap_or_else(|e| e.into_inner())
}

static RESTORING: Mutex<Vec<isize>> = Mutex::new(Vec::new());

fn restoring() -> MutexGuard<'static, Vec<isize>> {
    RESTORING.lock().unwrap_or_else(|e| e.into_inner())
}

const SANE: u32 = 64;
const FULL: i32 = 2;

fn fullscreen(hwnd: HWND) -> bool {
    if (style_of(hwnd) as u32) & WS_CAPTION.0 != 0 {
        return false;
    }
    let Some((w, h)) = client_size(hwnd) else { return false };
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    // -- SAFETY: reads the monitor nearest to a window into a local with cbSize set; with MONITOR_DEFAULTTONEAREST there …
    if !unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi) }.as_bool() {
        return false;
    }
    let (mw, mh) = (mi.rcMonitor.right - mi.rcMonitor.left, mi.rcMonitor.bottom - mi.rcMonitor.top);
    (w as i32 - mw).abs() <= FULL && (h as i32 - mh).abs() <= FULL
}

fn note_size(pid: u32, hwnd: HWND) {
    // -- SAFETY: reads one window's state
    if unsafe { IsZoomed(hwnd) }.as_bool() || is_hidden(hwnd) || process::hung(pid) || waking().contains(&(hwnd.0 as isize)) {
        return;
    }
    let Some(size) = client_size(hwnd).filter(|s| s.0 >= SANE && s.1 >= SANE) else { return };
    let full = fullscreen(hwnd);
    let mut all = normal_size();
    let Some(at) = all.iter().position(|e| e.pid == pid) else {
        all.push(Normal { pid, size, full });
        return;
    };
    if full {
        all[at].full = true;
        return;
    }
    let (want, was_full) = (all[at].size, all[at].full);
    if !was_full {
        all[at].size = size;
        return;
    }
    all[at].full = false;
    if !needs_restore(was_full, want, size) {
        return;
    }
    log::info!("client {pid}: out of fullscreen at {size:?}, putting its {want:?} back");
    drop(all);
    restore_size(pid, hwnd, want);
}

fn needs_restore(was_full: bool, before: (u32, u32), now: (u32, u32)) -> bool {
    was_full && before != now
}

fn restore_size(pid: u32, hwnd: HWND, size: (u32, u32)) {
    let h = hwnd.0 as isize;
    {
        let mut r = restoring();
        if r.contains(&h) {
            return;
        }
        r.push(h);
    }
    std::thread::spawn(move || {
        let hwnd = HWND(h as *mut _);
        let (fw, fh) = frame(hwnd).unwrap_or((0, 0));
        // -- SAFETY: a window-manager call on a top-level window: only its size changes
        let _ = unsafe {
            SetWindowPos(hwnd, None, 0, 0, size.0 as i32 + fw, size.1 as i32 + fh, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE)
        };
        match client_size(hwnd) {
            Some(now) => log::info!("client {pid}: asked for {size:?}, it has {now:?}"),
            None => log::debug!("client {pid}: window went away before its size came back"),
        }
        restoring().retain(|x| *x != h);
    });
}

pub fn hide_on_launch(pid: u32) {
    if let Some((hwnd, false)) = process::main_window(pid) {
        hide(pid, hwnd);
    }
}

fn window_to_change(pid: u32) -> Result<HWND> {
    let (hwnd, _) = process::main_window(pid).ok_or_else(|| AppError::Invalid(tr!("That client has no window yet.")))?;
    if process::hung(pid) {
        return Err(AppError::Invalid(tr!("That client isn't responding.")));
    }
    Ok(hwnd)
}

fn borderless(style: u32) -> u32 {
    (style & !(WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX).0) | WS_POPUP.0
}

pub fn drag_style(style: u32) -> u32 {
    (style & !WS_THICKFRAME.0) | WS_POPUP.0
}

pub fn style_of(hwnd: HWND) -> isize {
    // -- SAFETY: reads one window's style
    unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) }
}

pub fn set_style(hwnd: HWND, style: isize) {
    // -- SAFETY: changes one top-level window's style; it answers with a message, as to a theme change
    unsafe { SetWindowLongPtrW(hwnd, GWL_STYLE, style) };
}

pub fn free_resize(hwnd: HWND, (x, y, w, h): (i32, i32, i32, i32), keep_place: bool) {
    super::resize::mark_small(hwnd);
    let id = hwnd.0 as isize;
    for _ in 0..RESIZE_WAITS {
        {
            let mut busy = RESIZING.lock().unwrap_or_else(|e| e.into_inner());
            if !busy.contains(&id) {
                busy.push(id);
                break;
            }
        }
        std::thread::sleep(RESIZE_GAP);
    }
    let style = style_of(hwnd);
    let place = if keep_place { SWP_NOMOVE } else { Default::default() };
    set_style(hwnd, borderless(style as u32) as isize);
    // -- SAFETY: window-manager calls on a top-level window
    if let Err(e) = unsafe { SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE | place) } {
        log::debug!("resize failed: {e}");
    }
    if ask_not_met(hwnd, w, h) {
        log::info!("client at {} px won't take {w}x{h}: restoring it and asking again", hwnd.0 as isize);
        normal(hwnd, false);
        // -- SAFETY: as above
        let _ = unsafe { SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE | place) };
        if ask_not_met(hwnd, w, h) {
            log::warn!("client at {} still won't take {w}x{h}: Roblox keeps its own size", hwnd.0 as isize);
        }
    }
    set_style(hwnd, style);
    // -- SAFETY: as above; only makes the frame follow the style again
    let _ = unsafe { SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED) };
    RESIZING.lock().unwrap_or_else(|e| e.into_inner()).retain(|b| *b != id);
}

pub fn micro(id: &str, pid: u32, on: bool, size: (u32, u32)) -> Result<(u32, u32)> {
    let hwnd = window_to_change(pid)?;
    size_set_now(pid);
    if on {
        remember(id, pid, hwnd);
        normal(hwnd, false);
        let (fw, fh) = frame(hwnd).unwrap_or((0, 0));
        free_resize(hwnd, (0, 0, size.0 as i32 + fw, size.1 as i32 + fh), true);
        set_tiny(pid, Some(size));
    } else {
        set_tiny(pid, None);
        let hidden = is_hidden(hwnd);
        let before = {
            let mut s = saved();
            let at = s.iter().position(|(i, p, ..)| i == id && *p == pid);
            at.map(|at| s.remove(at).2)
        };
        if let Some(mut wp) = before {
            wp.showCmd = restore_cmd(wp.showCmd, (wp.flags & WPF_RESTORETOMAXIMIZED).0 != 0);
            // -- SAFETY: `wp` was filled by GetWindowPlacement for this window, its length set
            unsafe { SetWindowPlacement(hwnd, &wp) }.map_err(|e| AppError::Internal(tr!("Could not resize that window: {why}", why = crate::error::windows_error(&e))))?;
            if hidden {
                let_go_of_focus(hwnd);
            }
        } else if hidden || (style_of(hwnd) as u32) & WS_POPUP.0 != 0 {
            if hidden {
                un_iconify(hwnd, false);
            }
            if (style_of(hwnd) as u32) & WS_POPUP.0 != 0 {
                set_style(hwnd, (((style_of(hwnd) as u32) & !WS_POPUP.0) | WS_OVERLAPPEDWINDOW.0) as isize);
            }
            // -- SAFETY: as above
            let _ = unsafe { SetWindowPos(hwnd, None, 0, 0, 1280, 720, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED) };
        }
    }
    client_size(hwnd).ok_or_else(|| AppError::Invalid(tr!("That client's window has closed.")))
}

pub fn tile(clients: &[(String, u32)], near: HWND, micro: Option<(u32, u32)>) -> Vec<String> {
    tile_columns(clients, near, micro, None)
}

pub fn tile_columns(clients: &[(String, u32)], near: HWND, micro: Option<(u32, u32)>, columns: Option<u32>) -> Vec<String> {
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    // -- SAFETY: reads the monitor nearest to a window into a local with cbSize set; with MONITOR_DEFAULTTONEAREST there …
    if !unsafe { GetMonitorInfoW(MonitorFromWindow(near, MONITOR_DEFAULTTONEAREST), &mut mi) }.as_bool() {
        log::warn!("tile: monitor info unreadable, nothing laid out: {}", std::io::Error::last_os_error());
        return Vec::new();
    }
    let available: Vec<(String, u32)> = clients.iter()
        .filter(|(_, pid)| resizable(*pid).is_some()).cloned().collect();
    let clients = available.as_slice();
    let w = mi.rcWork;
    let mut laid = Vec::new();
    let size = micro.and_then(|(mw, mh)| {
        clients
            .iter()
            .find_map(|(_, pid)| resizable(*pid).and_then(frame).map(|(fw, fh)| (mw as i32 + fw, mh as i32 + fh)))
    });
    let area = (w.left, w.top, w.right - w.left, w.bottom - w.top);
    let cells = match columns {
        Some(cols) => column_grid(clients.len(), area, cols),
        None => grid(clients.len(), area, size),
    };
    for ((id, pid), &slot) in clients.iter().zip(&cells) {
        let Some(hwnd) = resizable(*pid) else { continue };
        set_tiny(*pid, micro);
        if micro.is_some() {
            remember(id, *pid, hwnd);
        } else {
            saved().retain(|(i, p, _)| !(i == id && *p == *pid));
        }
        normal(hwnd, false);
        free_resize(hwnd, slot, false);
        laid.push(id.clone());
    }
    let mut plan = placed();
    plan.clear();
    plan.extend(clients.iter().zip(&cells).map(|((_, pid), slot)| (*pid, *slot)));
    laid
}

fn resizable(pid: u32) -> Option<HWND> {
    match process::main_window(pid) {
        Some((hwnd, false)) if !is_hidden(hwnd) && !process::hung(pid) => Some(hwnd),
        _ => None,
    }
}

type Cell = (i32, i32, i32, i32);

static PLACED: Mutex<Vec<(u32, Cell)>> = Mutex::new(Vec::new());

fn placed() -> MutexGuard<'static, Vec<(u32, Cell)>> {
    PLACED.lock().unwrap_or_else(|e| e.into_inner())
}

fn back_to_cell(pid: u32, hwnd: HWND) {
    let Some(cell) = placed().iter().find(|(p, _)| *p == pid).map(|(_, c)| *c) else { return };
    if !left_cell(hwnd, cell) {
        return;
    }
    log::debug!("client {pid}: back to its cell {cell:?}");
    free_resize(hwnd, cell, false);
}

pub fn cell_of(pid: u32) -> Option<Cell> {
    placed().iter().find(|(p, _)| *p == pid).map(|(_, c)| *c)
}

pub fn report_state(pid: u32) -> Option<String> {
    let (hwnd, iconic) = process::main_window(pid)?;
    let mut r = RECT::default();
    // -- SAFETY: reads one window's rectangle into a local
    let (w, h, x, y) = match unsafe { GetWindowRect(hwnd, &mut r) } {
        Ok(()) => (r.right - r.left, r.bottom - r.top, r.left, r.top),
        Err(_) => (0, 0, 0, 0),
    };
    let how = match (iconic, is_hidden(hwnd)) {
        (true, _) => "minimized",
        (_, true) => "ghost",
        _ if process::hung(pid) => "not responding",
        _ => "visible",
    };
    Some(format!("{w}x{h} at {x},{y} {how}"))
}

fn left_cell(hwnd: HWND, (x, y, w, h): Cell) -> bool {
    let mut r = RECT::default();
    // -- SAFETY: reads one window's rectangle into a local
    if unsafe { GetWindowRect(hwnd, &mut r) }.is_err() {
        return false;
    }
    (r.left - x).abs() > 2 || (r.top - y).abs() > 2 || ((r.right - r.left) - w).abs() > 2 || ((r.bottom - r.top) - h).abs() > 2
}

pub fn column_grid(n: usize, (ax, ay, aw, ah): Cell, columns: u32) -> Vec<Cell> {
    if n == 0 || aw <= 0 || ah <= 0 {
        return Vec::new();
    }
    let cols = (columns as usize).clamp(1, n);
    let rows = n.div_ceil(cols);
    (0..n).map(|i| {
        // Use boundaries rather than rounded cell sizes to cover the work area exactly.
        let x = (i % cols) as i64 * aw as i64 / cols as i64;
        let y = (i / cols) as i64 * ah as i64 / rows as i64;
        let right = ((i % cols) + 1) as i64 * aw as i64 / cols as i64;
        let bottom = ((i / cols) + 1) as i64 * ah as i64 / rows as i64;
        (ax + x as i32, ay + y as i32, (right - x) as i32, (bottom - y) as i32)
    }).collect()
}

pub fn grid(n: usize, area: Cell, cell: Option<(i32, i32)>) -> Vec<Cell> {
    let (ax, ay, aw, ah) = area;
    if n == 0 || aw <= 0 || ah <= 0 {
        return Vec::new();
    }
    let fit = || {
        let picture = |c: usize| (aw / c as i32).min(ah / n.div_ceil(c) as i32 * 4 / 3);
        let cols = (1..=n).fold(1, |best, c| if picture(c) > picture(best) { c } else { best });
        (cols, aw / cols as i32, ah / n.div_ceil(cols) as i32, n)
    };
    let (cols, cw, ch, per) = match cell {
        Some((cw, ch)) => {
            let (cw, ch) = (cw.max(1), ch.max(1));
            let cols = (aw / cw).max(1) as usize;
            let per = cols * (ah / ch).max(1) as usize;
            if per >= n { (cols, cw, ch, per) } else { fit() }
        }
        None => fit(),
    };
    (0..n)
        .map(|i| {
            let slot = i % per;
            (ax + (slot % cols) as i32 * cw, ay + (slot / cols) as i32 * ch, cw, ch)
        })
        .collect()
}

pub fn restore_cmd(show_cmd: u32, to_maximized: bool) -> u32 {
    let max = SW_SHOWMAXIMIZED.0 as u32;
    if show_cmd == max || (show_cmd == SW_SHOWMINIMIZED.0 as u32 && to_maximized) { max } else { SW_SHOWNOACTIVATE.0 as u32 }
}

fn remember(id: &str, pid: u32, hwnd: HWND) {
    let mut s = saved();
    if s.iter().any(|(i, p, _)| i == id && *p == pid) {
        return;
    }
    let mut wp = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
    // -- SAFETY: reads one window's placement into a local with length set
    match unsafe { GetWindowPlacement(hwnd, &mut wp) } {
        Ok(()) => s.push((id.to_owned(), pid, wp)),
        Err(e) => log::debug!("instance {id}: window placement not remembered: {e}"),
    }
}

fn normal(hwnd: HWND, queued: bool) {
    // -- SAFETY: reads and sets one top-level window's state
    unsafe {
        if IsIconic(hwnd).as_bool() || IsZoomed(hwnd).as_bool() {
            let _ = if queued { ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE) } else { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        }
    }
}

fn ask_not_met(hwnd: HWND, w: i32, h: i32) -> bool {
    let mut r = RECT::default();
    // -- SAFETY: reads one window's rectangle into a local
    if unsafe { GetWindowRect(hwnd, &mut r) }.is_err() {
        return false;
    }
    ((r.right - r.left) - w).abs() > 2 || ((r.bottom - r.top) - h).abs() > 2
}

fn frame(hwnd: HWND) -> Option<(i32, i32)> {
    let (mut w, mut c) = (RECT::default(), RECT::default());
    // -- SAFETY: reads one window's rectangles into locals
    unsafe {
        GetWindowRect(hwnd, &mut w).ok()?;
        GetClientRect(hwnd, &mut c).ok()?;
    }
    Some((w.right - w.left - c.right, w.bottom - w.top - c.bottom))
}

fn client_size(hwnd: HWND) -> Option<(u32, u32)> {
    let mut r = RECT::default();
    // -- SAFETY: reads one window's client size into a local
    unsafe { GetClientRect(hwnd, &mut r) }.ok()?;
    Some(((r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32))
}

pub fn to_rgba(bgra: &[u8]) -> Vec<u8> {
    bgra.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "system" fn insists(hwnd: HWND, msg: u32, w: windows::Win32::Foundation::WPARAM, l: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
        use windows::Win32::UI::WindowsAndMessaging::{DefWindowProcW, MINMAXINFO, WM_GETMINMAXINFO};
        if msg == WM_GETMINMAXINFO {
            // -- SAFETY: for this message `l` points at the MINMAXINFO to fill in
            unsafe { (*(l.0 as *mut MINMAXINFO)).ptMinTrackSize = windows::Win32::Foundation::POINT { x: 800, y: 600 } };
            return windows::Win32::Foundation::LRESULT(0);
        }
        // -- SAFETY: the default for everything else
        unsafe { DefWindowProcW(hwnd, msg, w, l) }
    }

    fn moved_size(toggle: bool, size_loop: bool, start: (i32, i32)) -> (i32, i32) {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, PostMessageW, RegisterClassW, SC_MOVE, SC_SIZE, SendMessageW, WINDOW_EX_STYLE, WM_KEYDOWN, WM_SYSCOMMAND, WNDCLASSW, WS_VISIBLE};
        use windows::core::w;
        // -- SAFETY: a private window of this thread, destroyed at the end
        unsafe {
            let class = WNDCLASSW { lpfnWndProc: Some(insists), lpszClassName: w!("lram-test-moveloop"), ..Default::default() };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), w!("lram-test-moveloop"), w!("t"), WS_OVERLAPPEDWINDOW | WS_VISIBLE, 100, 100, 1000, 800, None, None, None, None).unwrap();
            free_resize(hwnd, (100, 100, start.0, start.1), false);
            let h = hwnd.0 as isize;
            let keys = std::thread::spawn(move || {
                let hwnd = HWND(h as *mut _);
                std::thread::sleep(std::time::Duration::from_millis(400));
                if toggle {
                    set_style(hwnd, drag_style(style_of(hwnd) as u32) as isize);
                }
                let _ = PostMessageW(Some(hwnd), WM_KEYDOWN, WPARAM(0x27), LPARAM(0));
                for _ in 0..5 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    let _ = PostMessageW(Some(hwnd), WM_KEYDOWN, WPARAM(if size_loop { 0x25 } else { 0x27 }), LPARAM(0));
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
                let _ = PostMessageW(Some(hwnd), WM_KEYDOWN, WPARAM(0x0D), LPARAM(0));
            });
            SendMessageW(hwnd, WM_SYSCOMMAND, Some(WPARAM(if size_loop { SC_SIZE } else { SC_MOVE } as usize)), Some(LPARAM(0)));
            keys.join().unwrap();
            let mut r = RECT::default();
            GetWindowRect(hwnd, &mut r).unwrap();
            let size = (r.right - r.left, r.bottom - r.top);
            let moved = r.left != 100;
            DestroyWindow(hwnd).unwrap();
            println!("toggle {toggle} size_loop {size_loop}: size {size:?}, moved {moved}");
            size
        }
    }

    #[test]
    fn a_size_just_set_is_left_alone() {
        const PID: u32 = 4_000_000_001;
        set_tiny(PID, None);
        let recent = TINY_TRIED.lock().unwrap().get(&PID).is_some_and(|t| t.elapsed() < TINY_RETRY);
        assert!(recent, "normal size or a big tile isn't undone by a sweep with an older list");
        TINY_TRIED.lock().unwrap().remove(&PID);
    }

    #[test]
    #[ignore = "opens a window: run on a desktop of its own"]
    fn move_loop_keeps_the_size_with_the_drag_style() {
        println!("without: {:?}", moved_size(false, false, (200, 150)));
        assert_eq!(moved_size(true, false, (200, 150)), (200, 150));
    }

    #[test]
    #[ignore = "opens a window: run on a desktop of its own"]
    fn size_loop_goes_below_the_minimum_with_the_drag_style() {
        println!("small, plain: {:?}", moved_size(false, true, (500, 400)));
        println!("small, drag style: {:?}", moved_size(true, true, (500, 400)));
        println!("big, plain: {:?}", moved_size(false, true, (900, 700)));
        println!("big, drag style: {:?}", moved_size(true, true, (900, 700)));
    }

    #[test]
    fn goes_below_the_windows_minimum_and_keeps_its_frame() {
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, RegisterClassW, WINDOW_EX_STYLE, WNDCLASSW};
        use windows::core::w;
        // -- SAFETY: a private hidden window of this thread, destroyed at the end
        unsafe {
            let class = WNDCLASSW { lpfnWndProc: Some(insists), lpszClassName: w!("lram-test-minsize"), ..Default::default() };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), w!("lram-test-minsize"), w!("t"), WS_OVERLAPPEDWINDOW, 0, 0, 1000, 800, None, None, None, None).unwrap();
            let before = style_of(hwnd);

            SetWindowPos(hwnd, None, 0, 0, 200, 150, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE).unwrap();
            let mut r = RECT::default();
            GetWindowRect(hwnd, &mut r).unwrap();
            assert_eq!(r.right - r.left, 800, "with a frame, Windows holds it to the minimum");

            free_resize(hwnd, (0, 0, 200, 150), true);
            GetWindowRect(hwnd, &mut r).unwrap();
            assert_eq!((r.right - r.left, r.bottom - r.top), (200, 150));
            assert_eq!(style_of(hwnd), before, "title bar and edges are back");
            assert!(frame(hwnd).unwrap().0 > 0, "and drawn");

            SetWindowPos(hwnd, None, 50, 50, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE).unwrap();
            GetWindowRect(hwnd, &mut r).unwrap();
            assert_eq!((r.right - r.left, r.bottom - r.top), (200, 150));
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    fn a_ghost_goes_and_comes_back_with_the_rest_of_its_style() {
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WNDCLASSW, WS_EX_TOOLWINDOW};
        use windows::core::w;
        unsafe extern "system" fn plain(hwnd: HWND, msg: u32, w: windows::Win32::Foundation::WPARAM, l: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
            // -- SAFETY: the default for everything
            unsafe { DefWindowProcW(hwnd, msg, w, l) }
        }
        let pid = std::process::id();
        // -- SAFETY: a private hidden window of this thread, destroyed at the end
        unsafe {
            let class = WNDCLASSW { lpfnWndProc: Some(plain), lpszClassName: w!("lram-test-ghost"), ..Default::default() };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, w!("lram-test-ghost"), w!("t"), WS_OVERLAPPEDWINDOW, 0, 0, 400, 300, None, None, None, None).unwrap();
            let ex = || GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let before = ex();
            assert!(!is_hidden(hwnd));

            hide(pid, hwnd);
            assert!(is_hidden(hwnd), "read from the window itself");
            hide(pid, hwnd);
            assert_eq!(ex(), before | GHOST, "twice is once");

            unhide(pid, hwnd);
            assert!(!is_hidden(hwnd));
            assert_eq!(ex(), before, "the rest of its style stays");
            unhide(pid, hwnd);
            assert_eq!(ex(), before, "twice is once");

            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (before | GHOST) as isize);
            assert!(!is_hidden(hwnd));
            unhide(pid, hwnd);
            assert_eq!(ex(), before, "by the bits alone");

            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (before | GHOST) as isize);
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 200, LWA_ALPHA).unwrap();
            assert!(!is_hidden(hwnd));
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    fn the_alpha_changes_without_the_client_answering() {
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WINDOW_EX_STYLE, WNDCLASSW};
        use windows::core::w;
        unsafe extern "system" fn plain(hwnd: HWND, msg: u32, w: windows::Win32::Foundation::WPARAM, l: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
            // -- SAFETY: the default for everything
            unsafe { DefWindowProcW(hwnd, msg, w, l) }
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let stalled = std::thread::spawn(move || {
            // -- SAFETY: a private hidden window of this thread, destroyed at the end
            unsafe {
                let class = WNDCLASSW { lpfnWndProc: Some(plain), lpszClassName: w!("lram-test-stalled"), ..Default::default() };
                RegisterClassW(&class);
                let hwnd = CreateWindowExW(WINDOW_EX_STYLE(GHOST), w!("lram-test-stalled"), w!("t"), WS_OVERLAPPEDWINDOW, 0, 0, 400, 300, None, None, None, None).unwrap();
                SetLayeredWindowAttributes(hwnd, COLORREF(0), GHOST_ALPHA, LWA_ALPHA).unwrap();
                tx.send(hwnd.0 as isize).unwrap();
                std::thread::sleep(Duration::from_millis(1500));
                DestroyWindow(hwnd).unwrap();
            }
        });
        let hwnd = HWND(rx.recv().unwrap() as *mut _);
        assert!(is_hidden(hwnd));
        let at = Instant::now();
        opaque(hwnd);
        assert!(at.elapsed() < Duration::from_millis(750), "took {:?}", at.elapsed());
        assert!(!is_hidden(hwnd) && has_ghost_bits(hwnd));
        stalled.join().unwrap();
    }

    #[test]
    fn one_slow_item_holds_up_only_itself() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static DONE: AtomicU32 = AtomicU32::new(0);
        let slow = |ms: u64| {
            std::thread::sleep(Duration::from_millis(ms));
            DONE.fetch_add(1, Ordering::SeqCst);
        };
        let at = Instant::now();
        assert!(!all_at_once(vec![1500, 0, 0, 0], Duration::from_millis(400), slow), "not all in time");
        assert!(at.elapsed() < Duration::from_millis(1000), "the wait has a limit");
        assert_eq!(DONE.load(Ordering::SeqCst), 3, "the others are done");
        assert!(all_at_once(vec![0, 0], Duration::from_secs(5), slow));
        assert!(all_at_once(Vec::<u64>::new(), Duration::ZERO, slow), "nothing to do");
    }

    #[test]
    fn a_parked_window_comes_back_the_same_size() {
        let r = back_from_park(RECT { left: -30_000, top: 40, right: -29_200, bottom: 640 });
        assert_eq!((r.left, r.top, r.right, r.bottom), (100, 100, 900, 700));
    }

    #[test]
    fn tells_a_ghost_by_both_bits_and_its_alpha() {
        assert!(ghostly(GHOST, GHOST_ALPHA));
        assert!(ghostly(GHOST | 0x100, GHOST_ALPHA), "whatever else it has");
        assert!(!ghostly(WS_EX_LAYERED.0, GHOST_ALPHA));
        assert!(!ghostly(WS_EX_TRANSPARENT.0, GHOST_ALPHA));
        assert!(!ghostly(GHOST, 255));
    }

    #[test]
    fn a_client_is_watched_for_a_while_after_its_preview_was_asked_for() {
        assert!(!asked_within(None));
        assert!(asked_within(Some(Instant::now())));
        assert!(!asked_within(Instant::now().checked_sub(WATCH)));
    }

    #[test]
    fn the_sweep_keeps_a_tiny_size_of_its_own_and_forgets_the_rest() {
        set_tiny(4_000_001, Some((100, 80)));
        sweep(&[(4_000_001, Some((320, 240))), (4_000_002, Some((320, 240))), (4_000_003, None)]);
        let of = |pid| tiny().iter().find(|e| e.0 == pid).map(|e| e.1);
        assert_eq!(of(4_000_001), Some((100, 80)), "the size it was made tiny at");
        assert_eq!(of(4_000_002), Some((320, 240)), "from before LRAM restarted");
        tiny().iter_mut().find(|e| e.0 == 4_000_001).unwrap().1 = (500, 400);
        sweep(&[(4_000_001, Some((320, 240))), (4_000_002, Some((320, 240)))]);
        assert_eq!(of(4_000_001), Some((500, 400)), "a size the user dragged it to stays too");
        assert_eq!(of(4_000_003), None);
        sweep(&[(4_000_001, None)]);
        assert_eq!(of(4_000_001), None, "back to normal");
        assert_eq!(of(4_000_002), None, "gone");
    }

    #[test]
    fn monitor_columns_cover_odd_work_areas_without_overlap() {
        assert!(column_grid(0, (0, 0, 1920, 1080), 4).is_empty());
        assert!(column_grid(4, (0, 0, 0, 1080), 4).is_empty());
        assert_eq!(column_grid(1, (-1920, 40, 1920, 1040), 20), [(-1920, 40, 1920, 1040)]);
        for n in [1, 3, 7, 25, 100] {
            for cols in 1..=20 {
                let cells = column_grid(n, (-1919, 31, 1919, 1001), cols);
                assert_eq!(cells.len(), n);
                for (i, &(x, y, w, h)) in cells.iter().enumerate() {
                    assert!(w > 0 && h > 0);
                    assert!(x >= -1919 && y >= 31 && x + w <= 0 && y + h <= 1032);
                    for &(xx, yy, ww, hh) in &cells[..i] {
                        assert!(x + w <= xx || xx + ww <= x || y + h <= yy || yy + hh <= y);
                    }
                }
            }
        }
    }

    #[test]
    fn says_why_there_is_no_live_picture() {
        assert_eq!(status_of(None, false, true), Status::NoWindow);
        assert_eq!(status_of(Some(true), true, false), Status::Minimized, "only a real minimize says so, old picture or not");
        assert_eq!(status_of(Some(true), false, true), Status::Minimized);
        assert_eq!(status_of(Some(false), true, true), Status::Live);
        assert_eq!(status_of(Some(false), false, false), Status::Pending, "a first frame or a timeout isn't minimized");
        assert_eq!(status_of(Some(false), false, true), Status::Unsupported);
    }

    #[test]
    fn sends_the_pixels_only_of_a_new_picture() {
        let at = Instant::now().checked_sub(std::time::Duration::from_secs(2)).unwrap();
        let shot = Shot { thumb: Arc::new(Thumb { width: 1, height: 2, rgba: vec![9; 8] }), seq: 7, at };
        let p = Preview { status: Status::Minimized, hidden: true, shot: Some(shot) };
        let word = |b: &[u8], i: usize| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());

        let new = p.to_bytes(6);
        assert_eq!(new.len(), 24 + 8);
        assert_eq!((word(&new, 0), word(&new, 1), word(&new, 2), word(&new, 3), word(&new, 4)), (1, 2, 1, 1, 7));
        assert!(word(&new, 5) >= 2000, "the age");
        assert_eq!(&new[24..], [9; 8]);

        assert_eq!(p.to_bytes(7).len(), 24, "known: the header alone");
        let none = Preview { status: Status::NoWindow, hidden: false, shot: None }.to_bytes(0);
        assert_eq!(none.len(), 24);
        assert_eq!(word(&none, 2), 2);
    }

    #[test]
    fn pictures_turn_rgba() {
        assert_eq!(to_rgba(&[1, 2, 3, 0, 4, 5, 6, 9]), [3, 2, 1, 255, 6, 5, 4, 255]);
    }

    #[test]
    fn grids_fit_the_work_area() {
        let area = (0, 40, 1920, 1000);
        assert_eq!(grid(0, area, None), []);
        assert_eq!(grid(1, area, None), [(0, 40, 1920, 1000)]);
        assert_eq!(grid(2, area, None), [(0, 40, 960, 1000), (960, 40, 960, 1000)]);
        assert_eq!(grid(4, area, None)[3], (960, 540, 960, 500), "2 x 2");
        assert!(grid(12, area, None).iter().all(|&(x, y, w, h)| x >= 0 && x + w <= 1920 && y >= 40 && y + h <= 1040));
        let micro = grid(15, area, Some((336, 279)));
        assert_eq!(micro[4], (1344, 40, 336, 279));
        assert_eq!(micro[5], (0, 319, 336, 279));
        assert_eq!(micro[14], (1344, 598, 336, 279));
        let full = grid(16, area, Some((336, 279)));
        assert_eq!((full[0], full[15]), ((0, 40, 480, 250), (1440, 790, 480, 250)), "4 x 4 of the work area");
        assert_no_overlap(&full);
        assert_no_overlap(&grid(2, (0, 0, 100, 100), Some((300, 300))));
    }

    fn assert_no_overlap(cells: &[(i32, i32, i32, i32)]) {
        for (i, &(ax, ay, aw, ah)) in cells.iter().enumerate() {
            for &(bx, by, bw, bh) in &cells[i + 1..] {
                let apart = ax + aw <= bx || bx + bw <= ax || ay + ah <= by || by + bh <= ay;
                assert!(apart, "({ax},{ay},{aw},{ah}) covers ({bx},{by},{bw},{bh})");
            }
        }
    }

    #[test]
    fn every_window_gets_a_cell_of_its_own() {
        let areas = [(0, 40, 1920, 1000), (0, 0, 2560, 1040), (0, 0, 1024, 768), (-1280, 0, 1280, 720)];
        for area in areas {
            for n in 1..=40 {
                let cells = grid(n, area, None);
                assert_eq!(cells.len(), n);
                assert_no_overlap(&cells);
            }
            for size in [(320, 240), (336, 279), (800, 600)] {
                for n in 1..=40 {
                    let cells = grid(n, area, Some(size));
                    assert_eq!(cells.len(), n, "{area:?} {size:?} x {n}");
                    assert_no_overlap(&cells);
                }
            }
        }
    }

    #[test]
    fn placements_come_back_without_the_focus() {
        let (min, max, normal) = (SW_SHOWMINIMIZED.0 as u32, SW_SHOWMAXIMIZED.0 as u32, 1);
        let quiet = SW_SHOWNOACTIVATE.0 as u32;
        assert_eq!(restore_cmd(max, false), max);
        assert_eq!(restore_cmd(min, true), max);
        assert_eq!(restore_cmd(min, false), quiet);
        assert_eq!(restore_cmd(normal, false), quiet);
    }

    #[test]
    fn only_a_fullscreen_trip_puts_a_size_back() {
        assert!(needs_restore(true, (1280, 720), (1024, 768)));
        assert!(!needs_restore(true, (1280, 720), (1280, 720)));
        assert!(!needs_restore(false, (1280, 720), (1024, 768)));
    }
}
