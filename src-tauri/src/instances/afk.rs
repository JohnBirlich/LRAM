use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetLastInputInfo, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, LASTINPUTINFO, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput, VIRTUAL_KEY,
};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetClientRect, GetCursorPos, GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId,
    PostMessageW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SetCursorPos,
    SetForegroundWindow, WA_ACTIVE, WA_INACTIVE, WM_ACTIVATE, WM_KEYDOWN, WM_KEYUP,
};

use super::{process, window};
use crate::App;
use crate::commands::{instances_changed, now_ms};
use crate::model::{AfkMode, AfkPress};

const IDLE_MS: u32 = 5_000;
const TICK: Duration = Duration::from_secs(10);
const RETRY: Duration = Duration::from_secs(30);
const BETWEEN: Duration = Duration::from_millis(100);
const SPACE: u16 = 0x39;
const TAB: u16 = 0x0F;
const VK_TAB: usize = 0x09;
const POST_SETTLE: Duration = Duration::from_millis(200);
const POST_HOLD: Duration = Duration::from_millis(300);
const AFTER_WAKE: Duration = Duration::from_millis(300);
const FRONT_WAIT: Duration = Duration::from_millis(300);
const FRONT_STEP: Duration = Duration::from_millis(20);
pub(super) static PRESSING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, PartialEq)]
pub struct AfkClient {
    pub id: String,
    pub pid: u32,
    pub since: i64,
}

#[derive(Debug, PartialEq)]
pub enum Plan {
    Wait,
    Retry,
    Press(Vec<String>),
}

// -- Main func
pub fn plan(clients: &[AfkClient], now: i64, interval_min: u32, idle_ms: u32, mode: AfkMode) -> Plan {
    let every = i64::from(interval_min) * 60_000;
    let due: Vec<String> = clients.iter().filter(|c| now - c.since >= every).map(|c| c.id.clone()).collect();
    if due.is_empty() {
        Plan::Wait
    } else if mode == AfkMode::Focus && idle_ms < IDLE_MS {
        Plan::Retry
    } else {
        Plan::Press(due)
    }
}

pub fn spawn(app: AppHandle) {
    let started = std::thread::Builder::new().name("anti-afk".into()).spawn(move || {
        loop {
            sleep(tick(&app));
        }
    });
    if let Err(e) = started {
        log::warn!("anti-AFK not running: {e}");
    }
}

fn tick(app: &AppHandle) -> Duration {
    let s = app.state::<App>();
    let v = match s.db.settings() {
        Ok(v) => v,
        Err(e) => {
            log::warn!("anti-AFK: could not read settings: {e}");
            return TICK;
        }
    };
    if !v.anti_afk.enabled {
        return TICK;
    }
    let clients: Vec<_> = s.tracker.afk_clients().into_iter().filter(|c| !process::hung(c.pid)).collect();
    let ids = match plan(&clients, now_ms(), v.anti_afk.interval_min, idle_ms(), v.anti_afk.mode) {
        Plan::Wait => return TICK,
        Plan::Retry => return RETRY,
        Plan::Press(ids) => ids,
    };
    if v.anti_afk.mode == AfkMode::Background {
        for c in clients.iter().filter(|c| ids.contains(&c.id)) {
            let Some((hwnd, minimized)) = process::main_window(c.pid) else {
                log::debug!("anti-AFK: instance {} has no main window", c.id);
                continue;
            };
            if post_press(hwnd) {
                log::info!("anti-AFK: posted Tab to instance {} (minimized: {minimized})", c.id);
                s.tracker.mark_afk(&c.id, now_ms());
            } else {
                log::info!("anti-AFK: couldn't post a key to instance {}", c.id);
            }
        }
        instances_changed(app);
        return TICK;
    }
    let what = v.anti_afk.press;
    let mut failed = false;
    let mut ours = last_input();
    for c in clients.iter().filter(|c| ids.contains(&c.id)) {
        if last_input() != ours {
            failed = true;
            break;
        }
        let Some((hwnd, minimized)) = process::main_window(c.pid) else {
            log::debug!("anti-AFK: instance {} has no window", c.id);
            continue;
        };
        if minimized {
            if !window::wake_for_press(c.pid, hwnd) {
                failed = true;
                log::info!("anti-AFK: instance {} couldn't be woken for its press", c.id);
                continue;
            }
            sleep(AFTER_WAKE);
            if last_input() != ours {
                failed = true;
                break;
            }
        }
        let r = press(hwnd, what);
        ours = last_input();
        match r {
            Pressed::Yes => s.tracker.mark_afk(&c.id, now_ms()),
            Pressed::No => {
                failed = true;
                log::info!("anti-AFK: instance {} couldn't be brought to the front", c.id);
            }
            Pressed::Interrupted => {
                failed = true;
                break;
            }
        }
        sleep(BETWEEN);
    }
    instances_changed(app);
    if failed { RETRY } else { TICK }
}

fn last_input() -> Option<u32> {
    let mut li = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    // -- SAFETY: `li` is a local with cbSize set, as GetLastInputInfo requires
    unsafe { GetLastInputInfo(&mut li) }.as_bool().then_some(li.dwTime)
}

fn idle_ms() -> u32 {
    let Some(t) = last_input() else {
        log::warn!("anti-AFK: GetLastInputInfo failed: {}", std::io::Error::last_os_error());
        return 0;
    };
    // -- SAFETY: no arguments; the tick count wraps the same way dwTime does
    unsafe { GetTickCount() }.wrapping_sub(t)
}

enum Pressed {
    Yes,
    No,
    Interrupted,
}

fn press(hwnd: HWND, what: AfkPress) -> Pressed {
    PRESSING.store(true, Ordering::Relaxed);
    let pressed = match what {
        AfkPress::Space => press_key(hwnd),
        AfkPress::Camera => mouse_press(hwnd, true),
        AfkPress::Mouse => mouse_press(hwnd, false),
    };
    window::back_down(hwnd);
    PRESSING.store(false, Ordering::Relaxed);
    pressed
}

fn mouse_press(hwnd: HWND, camera: bool) -> Pressed {
    // -- SAFETY: reads which window is in front
    let prev = unsafe { GetForegroundWindow() };
    let mut ok = prev == hwnd || to_front(hwnd);
    let back = cursor();
    if ok {
        match inside(hwnd) {
            Some((x, y)) => {
                ok = move_to(x, y);
                let held = camera && ok && button(true);
                ok = ok && move_to(x + 3, y) && move_to(x, y);
                if held {
                    ok = button(false) && ok;
                }
            }
            None => ok = false,
        }
    }
    if let Some((x, y)) = back {
        // -- SAFETY: puts the cursor back; a failure only leaves it where the press left it
        let _ = unsafe { SetCursorPos(x, y) };
    }
    if !prev.is_invalid() && prev != hwnd && !to_front(prev) {
        log::warn!("anti-AFK: couldn't give the focus back to the previous window; stopping this round");
        return Pressed::Interrupted;
    }
    if ok { Pressed::Yes } else { Pressed::No }
}

fn inside(hwnd: HWND) -> Option<(i32, i32)> {
    let mut r = RECT::default();
    // -- SAFETY: reads one window's client rectangle into a local
    unsafe { GetClientRect(hwnd, &mut r) }.ok()?;
    let mut at = POINT { x: (r.right - r.left) / 2, y: (r.bottom - r.top) / 2 };
    // -- SAFETY: converts that client point to a screen point in place; no pointer escapes
    if !unsafe { ClientToScreen(hwnd, &mut at) }.as_bool() {
        return None;
    }
    Some((at.x, at.y))
}

fn cursor() -> Option<(i32, i32)> {
    let mut at = POINT::default();
    // -- SAFETY: reads the cursor position into a local
    unsafe { GetCursorPos(&mut at) }.ok()?;
    Some((at.x, at.y))
}

fn move_to(x: i32, y: i32) -> bool {
    // -- SAFETY: no arguments; metrics of the whole desktop
    let (w, h) = unsafe { (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN)) };
    // -- SAFETY: as above
    let (x0, y0) = unsafe { (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN)) };
    if w < 2 || h < 2 {
        return false;
    }
    let nx = ((x - x0) as i64 * 65_535 / (w - 1) as i64).clamp(0, 65_535) as i32;
    let ny = ((y - y0) as i64 * 65_535 / (h - 1) as i64).clamp(0, 65_535) as i32;
    // -- SAFETY: one fully initialised mouse INPUT and its size, as SendInput requires
    let sent = unsafe {
        let input = mouse_input(nx, ny, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK);
        SendInput(&[input], size_of::<INPUT>() as i32)
    };
    sent == 1
}

fn button(down: bool) -> bool {
    let flags = if down { MOUSEEVENTF_RIGHTDOWN } else { MOUSEEVENTF_RIGHTUP };
    // -- SAFETY: one fully initialised mouse INPUT and its size, as SendInput requires
    let sent = unsafe {
        let input = mouse_input(0, 0, flags);
        SendInput(&[input], size_of::<INPUT>() as i32)
    };
    sent == 1
}

fn mouse_input(dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

fn press_key(hwnd: HWND) -> Pressed {
    // -- SAFETY: reads which window is in front
    let prev = unsafe { GetForegroundWindow() };
    if !to_front(hwnd) {
        return Pressed::No;
    }
    // -- SAFETY: reads which window is in front
    let sent = unsafe { GetForegroundWindow() } == hwnd;
    let mut user = false;
    if sent {
        key(VIRTUAL_KEY(0), SPACE, KEYEVENTF_SCANCODE);
        let after_down = last_input();
        sleep(Duration::from_millis(40));
        user = last_input() != after_down;
        key(VIRTUAL_KEY(0), SPACE, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP);
        sleep(Duration::from_millis(40));
    }
    if !prev.is_invalid() && prev != hwnd && !to_front(prev) {
        log::warn!("anti-AFK: couldn't give the focus back to the previous window; stopping this round");
        return Pressed::Interrupted;
    }
    match (sent, user) {
        (_, true) => Pressed::Interrupted,
        (true, false) => Pressed::Yes,
        (false, false) => Pressed::No,
    }
}

fn post_press(hwnd: HWND) -> bool {
    let down = LPARAM(1 | ((TAB as isize) << 16));
    let up = LPARAM(down.0 | (1 << 30) | (1 << 31));
    // -- SAFETY: PostMessageW only queues a message to a top-level window handle; one that is gone just fails
    let post = |msg, w: usize, l: LPARAM| unsafe { PostMessageW(Some(hwnd), msg, WPARAM(w), l) }.is_ok();
    // -- SAFETY: reads which window is in front
    let front = unsafe { GetForegroundWindow() } == hwnd;
    if !front {
        if !post(WM_ACTIVATE, WA_ACTIVE as usize, LPARAM(0)) {
            return false;
        }
        sleep(POST_SETTLE);
    }
    let ok = post(WM_KEYDOWN, VK_TAB, down);
    sleep(POST_HOLD);
    let ok = ok && post(WM_KEYUP, VK_TAB, up);
    if !front {
        sleep(POST_SETTLE);
        // -- SAFETY: reads which window is in front
        if unsafe { GetForegroundWindow() } != hwnd {
            post(WM_ACTIVATE, WA_INACTIVE as usize, LPARAM(0));
        }
    }
    ok
}

pub(super) fn to_front(hwnd: HWND) -> bool {
    // -- SAFETY: a window-manager call on a top-level window handle; one that is gone just fails
    let _ = unsafe { SetForegroundWindow(hwnd) };
    if front_soon(hwnd) {
        return true;
    }
    // -- SAFETY: attaches this thread's input to the foreground window's thread only for these calls and detaches it again; …
    unsafe {
        let fg_thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let me = GetCurrentThreadId();
        if fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool() {
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
            let _ = AttachThreadInput(me, fg_thread, false);
        }
    }
    if front_soon(hwnd) {
        return true;
    }
    let nudge = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 0, dy: 0, dwFlags: MOUSEEVENTF_MOVE, ..Default::default() } },
    };
    // -- SAFETY: one fully initialised mouse INPUT and its size, as SendInput requires
    if unsafe { SendInput(&[nudge], size_of::<INPUT>() as i32) } == 0 {
        log::debug!("anti-AFK: mouse nudge not sent: {}", std::io::Error::last_os_error());
    }
    // -- SAFETY: as the first SetForegroundWindow
    let _ = unsafe { SetForegroundWindow(hwnd) };
    front_soon(hwnd)
}

fn front_soon(hwnd: HWND) -> bool {
    // -- SAFETY: reads which window is in front
    let front = || unsafe { GetForegroundWindow() } == hwnd;
    let steps = FRONT_WAIT.as_millis() / FRONT_STEP.as_millis();
    for _ in 0..steps {
        if front() {
            return true;
        }
        sleep(FRONT_STEP);
    }
    front()
}

fn key(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    };
    // -- SAFETY: one fully initialised keyboard INPUT and its size, as SendInput requires
    if unsafe { SendInput(&[input], size_of::<INPUT>() as i32) } == 0 {
        log::warn!("anti-AFK: SendInput sent no key: {}", std::io::Error::last_os_error());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(id: &str, since: i64) -> AfkClient {
        AfkClient { id: id.into(), pid: 1, since }
    }

    #[test]
    fn presses_only_due_clients_while_the_user_is_away() {
        let min = 60_000;
        let clients = [client("a", 0), client("b", 10 * min), client("c", 15 * min + 1)];
        let ids = |v: &[&str]| Plan::Press(v.iter().map(|s| s.to_string()).collect());
        assert_eq!(plan(&clients, 15 * min - 1, 15, 60_000, AfkMode::Focus), Plan::Wait);
        assert_eq!(plan(&[], 99 * min, 15, 60_000, AfkMode::Focus), Plan::Wait);
        assert_eq!(plan(&clients, 15 * min, 15, 60_000, AfkMode::Focus), ids(&["a"]));
        assert_eq!(plan(&clients, 25 * min, 15, 5_000, AfkMode::Focus), ids(&["a", "b"]));
        assert_eq!(plan(&clients, 25 * min, 15, 4_999, AfkMode::Focus), Plan::Retry);
        assert_eq!(plan(&clients, 25 * min, 15, 0, AfkMode::Focus), Plan::Retry);
        assert_eq!(plan(&clients, 5 * min, 15, 0, AfkMode::Focus), Plan::Wait);
        assert_eq!(plan(&clients, 31 * min, 1, 10_000, AfkMode::Focus), ids(&["a", "b", "c"]));
        assert_eq!(plan(&clients, 25 * min, 15, 0, AfkMode::Background), ids(&["a", "b"]));
        assert_eq!(plan(&clients, 5 * min, 15, 0, AfkMode::Background), Plan::Wait);
    }

    #[test]
    fn settings_stored_before_the_press_setting_keep_the_jump() {
        let old: crate::model::AntiAfk =
            serde_json::from_str(r#"{"enabled":true,"intervalMin":15,"mode":"focus"}"#).unwrap();
        assert_eq!(old.press, AfkPress::Space);
        let camera: crate::model::AntiAfk =
            serde_json::from_str(r#"{"enabled":true,"intervalMin":10,"mode":"focus","press":"camera"}"#).unwrap();
        assert_eq!((camera.press, camera.interval_min), (AfkPress::Camera, 10));
    }
}
