//! Anti-AFK. Roblox disconnects a player after 20 minutes without input (reason 267). Every
//! `Settings.anti_afk.interval_min` minutes, each client in a game that hasn't opted out
//! (`Instance.anti_afk`) gets one key press, the way `Settings.anti_afk.mode` says:
//! - Focus: the client is brought to the front for a moment, gets the press through `SendInput`
//!   of Space (by scan code, as a keyboard sends it), and the window that was in front gets the focus
//!   back. One client at a time, about 100 ms each. Only while nobody has touched the keyboard or
//!   mouse for 5 s (`GetLastInputInfo`); otherwise it tries again 30 s later. Hidden clients
//!   (`window`) get it invisibly. A minimized one, asleep or minimized by hand, wakes as a hidden one
//!   first, up to about 2 s each, so Background suits many clients. A hidden one goes back to the
//!   bottom of the z-order after its press, and to sleep soon after.
//! - Background: WM_ACTIVATE, then WM_KEYDOWN/WM_KEYUP for Tab are posted to the client's
//!   window; the focus never moves, so it runs while the user is at the PC and in minimized
//!   clients too. Roblox may not count posted input as activity in every game.
//!
//! In focus mode what the press does is `Settings.anti_afk.press`: Space (the jump, which resets a
//! treadmill's counter and also knocks the character off it), a right-button camera drag of a few
//! pixels, or the cursor moved into the client and back with no button at all. Both gentle kinds
//! put the cursor and the focus back where they were.
//!
//! Nothing is injected into or read from the client; auto-rejoin on the 267 kick stays the
//! fallback.

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

/// No keyboard or mouse input for this long counts as the user being away.
const IDLE_MS: u32 = 5_000;
const TICK: Duration = Duration::from_secs(10);
/// After the user was active, or a client couldn't be brought to the front.
const RETRY: Duration = Duration::from_secs(30);
const BETWEEN: Duration = Duration::from_millis(100);
/// Space's set-1 scan code.
const SPACE: u16 = 0x39;
/// Tab, for background mode: posted Space counts as activity less reliably than Tab.
const TAB: u16 = 0x0F;
const VK_TAB: usize = 0x09;
/// Background mode, around its press: a capped or sleeping client may draw less than one frame in
/// a shorter time, and would see activate, key and deactivate all at once.
const POST_SETTLE: Duration = Duration::from_millis(200);
const POST_HOLD: Duration = Duration::from_millis(300);
/// After waking a client, before its press: it has to be taking input again.
// ponytail: a guess; tune from real clients if pressed clients still get kicked.
const AFTER_WAKE: Duration = Duration::from_millis(300);
/// How long Windows may take to make the window it was asked for the front one, and how often
/// that is looked at.
const FRONT_WAIT: Duration = Duration::from_millis(300);
const FRONT_STEP: Duration = Duration::from_millis(20);
/// A press is under way: a hidden client in front is this, not something to get away from (`resize`).
pub(super) static PRESSING: AtomicBool = AtomicBool::new(false);

/// A client anti-AFK may press keys in (`Tracker::afk_clients`).
#[derive(Clone, Debug, PartialEq)]
pub struct AfkClient {
    pub id: String,
    pub pid: u32,
    /// Unix ms of its last press, else of reaching the game.
    pub since: i64,
}

/// What one tick does.
#[derive(Debug, PartialEq)]
pub enum Plan {
    /// Nobody is due.
    Wait,
    /// Someone is due but the user is at the PC: again in 30 s.
    Retry,
    /// Press a key in these clients (ids).
    Press(Vec<String>),
}

/// Which clients are due (`interval_min` since their `since`); in focus mode gated on the user
/// having been idle for 5 s, in background mode not (pure).
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

/// Runs for the app's lifetime on its own thread (focus changes and `SendInput` block briefly).
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

/// One round; how long to wait before the next.
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
    // A hung client would block this one thread, and the others would get kicked for idling.
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
    // Our own SendInput moves GetLastInputInfo too, so "the user came back" is any input newer
    // than our last injection.
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
            // Waking took a while: the user may be back.
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

/// Tick count of the last keyboard or mouse input anywhere in the session (ours included);
/// None if it can't be read.
fn last_input() -> Option<u32> {
    let mut li = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    // SAFETY: `li` is a local with cbSize set, as GetLastInputInfo requires.
    unsafe { GetLastInputInfo(&mut li) }.as_bool().then_some(li.dwTime)
}

/// How long since the last keyboard or mouse input anywhere in the session.
fn idle_ms() -> u32 {
    let Some(t) = last_input() else {
        log::warn!("anti-AFK: GetLastInputInfo failed: {}", std::io::Error::last_os_error());
        return 0;
    };
    // SAFETY: no arguments; the tick count wraps the same way dwTime does.
    unsafe { GetTickCount() }.wrapping_sub(t)
}

enum Pressed {
    Yes,
    /// Couldn't get or keep the focus: no key was sent.
    No,
    /// The user (or the restore) needs the round to stop.
    Interrupted,
}

/// The press in `hwnd`, then the focus back where it was, and a hidden client back down.
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

/// The gentle press: the cursor goes into the middle of the client and a few pixels to the right
/// (with the right button held, which turns the camera a hair), then the cursor and the focus go
/// back where they were. The character neither jumps nor steps, so a treadmill keeps its counter.
fn mouse_press(hwnd: HWND, camera: bool) -> Pressed {
    // SAFETY: reads which window is in front.
    let prev = unsafe { GetForegroundWindow() };
    // Roblox takes input only in the window it has, and only under the cursor.
    let mut ok = prev == hwnd || to_front(hwnd);
    let back = cursor();
    if ok {
        match inside(hwnd) {
            Some((x, y)) => {
                // Into the client first: that move must not be a camera drag.
                ok = move_to(x, y);
                let held = camera && ok && button(true);
                ok = ok && move_to(x + 3, y) && move_to(x, y);
                // Let go even when a move failed, or the button would stay down.
                if held {
                    ok = button(false) && ok;
                }
            }
            None => ok = false,
        }
    }
    if let Some((x, y)) = back {
        // SAFETY: puts the cursor back; a failure only leaves it where the press left it.
        let _ = unsafe { SetCursorPos(x, y) };
    }
    if !prev.is_invalid() && prev != hwnd && !to_front(prev) {
        log::warn!("anti-AFK: couldn't give the focus back to the previous window; stopping this round");
        return Pressed::Interrupted;
    }
    if ok { Pressed::Yes } else { Pressed::No }
}

/// The middle of a window's client area, in screen coordinates.
fn inside(hwnd: HWND) -> Option<(i32, i32)> {
    let mut r = RECT::default();
    // SAFETY: reads one window's client rectangle into a local.
    unsafe { GetClientRect(hwnd, &mut r) }.ok()?;
    let mut at = POINT { x: (r.right - r.left) / 2, y: (r.bottom - r.top) / 2 };
    // SAFETY: converts that client point to a screen point in place; no pointer escapes.
    if !unsafe { ClientToScreen(hwnd, &mut at) }.as_bool() {
        return None;
    }
    Some((at.x, at.y))
}

/// Where the cursor is now, so the press can put it back.
fn cursor() -> Option<(i32, i32)> {
    let mut at = POINT::default();
    // SAFETY: reads the cursor position into a local.
    unsafe { GetCursorPos(&mut at) }.ok()?;
    Some((at.x, at.y))
}

/// Moves the cursor through `SendInput` (absolute, over all monitors), which the window under it
/// counts as input; `SetCursorPos` may move it without any input event.
fn move_to(x: i32, y: i32) -> bool {
    // SAFETY: no arguments; metrics of the whole desktop.
    let (w, h) = unsafe { (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN)) };
    // SAFETY: as above.
    let (x0, y0) = unsafe { (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN)) };
    if w < 2 || h < 2 {
        return false;
    }
    let nx = ((x - x0) as i64 * 65_535 / (w - 1) as i64).clamp(0, 65_535) as i32;
    let ny = ((y - y0) as i64 * 65_535 / (h - 1) as i64).clamp(0, 65_535) as i32;
    // SAFETY: one fully initialised mouse INPUT and its size, as SendInput requires.
    let sent = unsafe {
        let input = mouse_input(nx, ny, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK);
        SendInput(&[input], size_of::<INPUT>() as i32)
    };
    sent == 1
}

/// The right button down or up, where the cursor already is.
fn button(down: bool) -> bool {
    let flags = if down { MOUSEEVENTF_RIGHTDOWN } else { MOUSEEVENTF_RIGHTUP };
    // SAFETY: one fully initialised mouse INPUT and its size, as SendInput requires.
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
    // SAFETY: reads which window is in front.
    let prev = unsafe { GetForegroundWindow() };
    if !to_front(hwnd) {
        return Pressed::No;
    }
    // Never into another window: only while the client is still the one in front.
    // SAFETY: reads which window is in front.
    let sent = unsafe { GetForegroundWindow() } == hwnd;
    // Input other than ours between Space down and up: the user is back.
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

/// Tab posted to `hwnd` without touching the focus: WM_ACTIVATE(WA_ACTIVE) first, since a
/// client that thinks it is in the background may drop keys, and WA_INACTIVE after unless it
/// really is in front. False if Windows wouldn't queue the messages (window gone or hung).
fn post_press(hwnd: HWND) -> bool {
    // lParam: repeat count 1, scan code in bits 16-23; key up also sets the previous-state and
    // transition bits (30, 31).
    let down = LPARAM(1 | ((TAB as isize) << 16));
    let up = LPARAM(down.0 | (1 << 30) | (1 << 31));
    // SAFETY: PostMessageW only queues a message to a top-level window handle; one that is gone
    // just fails. None of these messages carries a pointer.
    let post = |msg, w: usize, l: LPARAM| unsafe { PostMessageW(Some(hwnd), msg, WPARAM(w), l) }.is_ok();
    // SAFETY: reads which window is in front.
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
        // The user may have brought it to the front meanwhile.
        // SAFETY: reads which window is in front.
        if unsafe { GetForegroundWindow() } != hwnd {
            post(WM_ACTIVATE, WA_INACTIVE as usize, LPARAM(0));
        }
    }
    ok
}

/// Brings `hwnd` to the front: plainly; else attached to the input of the window in front
/// (whose thread may hand the focus over); else after a zero-movement mouse input, which makes
/// this process the last input source and so lifts Windows' foreground lock without any window
/// reacting (an ALT tap would open menus).
pub(super) fn to_front(hwnd: HWND) -> bool {
    // SAFETY: a window-manager call on a top-level window handle; one that is gone just fails.
    let _ = unsafe { SetForegroundWindow(hwnd) };
    if front_soon(hwnd) {
        return true;
    }
    // SAFETY: attaches this thread's input to the foreground window's thread only for these
    // calls and detaches it again; both thread ids are live (ours, the window's owner).
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
    // SAFETY: one fully initialised mouse INPUT and its size, as SendInput requires.
    if unsafe { SendInput(&[nudge], size_of::<INPUT>() as i32) } == 0 {
        log::debug!("anti-AFK: mouse nudge not sent: {}", std::io::Error::last_os_error());
    }
    // SAFETY: as the first SetForegroundWindow.
    let _ = unsafe { SetForegroundWindow(hwnd) };
    front_soon(hwnd)
}

/// Whether `hwnd` is the front window, waiting a moment for it: Windows activates a window after
/// the call that asked for it returns, so asking straight away says no while it already worked.
fn front_soon(hwnd: HWND) -> bool {
    // SAFETY: reads which window is in front.
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

/// One key event through `SendInput`.
fn key(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    };
    // SAFETY: one fully initialised keyboard INPUT and its size, as SendInput requires.
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
        // Nobody due yet.
        assert_eq!(plan(&clients, 15 * min - 1, 15, 60_000, AfkMode::Focus), Plan::Wait);
        assert_eq!(plan(&[], 99 * min, 15, 60_000, AfkMode::Focus), Plan::Wait);
        // Due exactly at the interval.
        assert_eq!(plan(&clients, 15 * min, 15, 60_000, AfkMode::Focus), ids(&["a"]));
        assert_eq!(plan(&clients, 25 * min, 15, 5_000, AfkMode::Focus), ids(&["a", "b"]));
        // The user touched the keyboard or mouse within 5 s: later.
        assert_eq!(plan(&clients, 25 * min, 15, 4_999, AfkMode::Focus), Plan::Retry);
        assert_eq!(plan(&clients, 25 * min, 15, 0, AfkMode::Focus), Plan::Retry);
        // Nobody due wins over a busy user: nothing to retry.
        assert_eq!(plan(&clients, 5 * min, 15, 0, AfkMode::Focus), Plan::Wait);
        assert_eq!(plan(&clients, 31 * min, 1, 10_000, AfkMode::Focus), ids(&["a", "b", "c"]));
        // Background mode doesn't wait for the user to be away; nobody due is still nothing.
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
