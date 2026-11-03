use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, Instant};

use tauri::{AppHandle, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};
use tauri_plugin_window_state::{StateFlags, WindowExt};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos};

const SPLASH: (f64, f64) = (440.0, 300.0);
pub const DEFAULT: (f64, f64) = (1280.0, 800.0);
pub const MIN: (f64, f64) = (960.0, 600.0);
const GROW: Duration = Duration::from_millis(550);
const FRAME: Duration = Duration::from_millis(16);
const FALLBACK: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

struct Target {
    rect: Rect,
    maximized: bool,
}

#[derive(Default)]
pub struct Pending(Mutex<Option<Target>>);

// -- Main func
pub fn start(app: &AppHandle, w: &WebviewWindow, skip: bool) -> tauri::Result<()> {
    w.restore_state(StateFlags::all() - StateFlags::VISIBLE)?;
    let maximized = w.is_maximized()?;
    if maximized {
        w.unmaximize()?;
    }
    let scale = w.scale_factor()?;
    let (pos, size) = (w.outer_position()?, w.outer_size()?);
    let mut rect = Rect { x: pos.x, y: pos.y, w: size.width as i32, h: size.height as i32 };
    let monitor = w.current_monitor()?.or(w.primary_monitor()?);
    if f64::from(rect.w) < MIN.0 * scale || f64::from(rect.h) < MIN.1 * scale {
        let (w, h) = ((DEFAULT.0 * scale) as i32, (DEFAULT.1 * scale) as i32);
        rect = match &monitor {
            Some(m) => centered(area(m), w, h),
            None => Rect { w, h, ..rect },
        };
    }
    if maximized && let Some(m) = &monitor {
        rect = area(m);
    }
    if skip {
        w.set_size(PhysicalSize::new(rect.w as u32, rect.h as u32))?;
        w.set_position(PhysicalPosition::new(rect.x, rect.y))?;
        settle(w, maximized);
        w.show()?;
        let _ = w.set_focus();
        return Ok(());
    }
    let s = centered(rect, (SPLASH.0 * scale) as i32, (SPLASH.1 * scale) as i32);
    w.set_size(PhysicalSize::new(s.w as u32, s.h as u32))?;
    w.set_position(PhysicalPosition::new(s.x, s.y))?;
    *app.state::<Pending>().0.lock().unwrap() = Some(Target { rect, maximized });
    w.show()?;
    let _ = w.set_focus();
    let handle = app.clone();
    std::thread::spawn(move || {
        sleep(FALLBACK);
        grow(&handle, true);
    });
    Ok(())
}

fn centered(on: Rect, w: i32, h: i32) -> Rect {
    Rect { x: on.x + (on.w - w) / 2, y: on.y + (on.h - h) / 2, w, h }
}

fn area(m: &tauri::Monitor) -> Rect {
    let a = m.work_area();
    Rect { x: a.position.x, y: a.position.y, w: a.size.width as i32, h: a.size.height as i32 }
}

fn lerp(a: Rect, b: Rect, t: f64) -> Rect {
    let e = if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 };
    let m = |p: i32, q: i32| p + (f64::from(q - p) * e).round() as i32;
    Rect { x: m(a.x, b.x), y: m(a.y, b.y), w: m(a.w, b.w), h: m(a.h, b.h) }
}

fn grow(app: &AppHandle, fallback: bool) {
    let Some(target) = app.state::<Pending>().0.lock().unwrap().take() else { return };
    if fallback {
        log::warn!("splash: the UI never called splash_done; growing the window anyway");
    }
    let Some(w) = app.get_webview_window("main") else {
        log::warn!("splash: main window missing at grow");
        return;
    };
    let from = match (w.outer_position(), w.outer_size()) {
        (Ok(p), Ok(s)) => Rect { x: p.x, y: p.y, w: s.width as i32, h: s.height as i32 },
        (p, s) => {
            log::debug!("splash: window rect unreadable ({:?}, {:?})", p.err(), s.err());
            target.rect
        }
    };
    let hwnd = w.hwnd().map(|h| HWND(h.0 as _)).inspect_err(|e| log::debug!("splash: no hwnd: {e}"));
    if let Ok(hwnd) = hwnd {
        let start = Instant::now();
        loop {
            let t = (start.elapsed().as_secs_f64() / GROW.as_secs_f64()).min(1.0);
            let r = lerp(from, target.rect, t);
            // -- SAFETY: moves and sizes our own top-level window; one call per frame so position and size change together
            let _ = unsafe { SetWindowPos(hwnd, None, r.x, r.y, r.w, r.h, SWP_NOZORDER | SWP_NOACTIVATE) };
            if t >= 1.0 {
                break;
            }
            sleep(FRAME);
        }
    } else {
        let r = target.rect;
        if let Err(e) = w.set_size(PhysicalSize::new(r.w as u32, r.h as u32)) {
            log::warn!("splash: could not size the window: {e}");
        }
        if let Err(e) = w.set_position(PhysicalPosition::new(r.x, r.y)) {
            log::warn!("splash: could not place the window: {e}");
        }
    }
    settle(&w, target.maximized);
}

fn settle(w: &WebviewWindow, maximized: bool) {
    if let Err(e) = w.set_min_size(Some(LogicalSize::new(MIN.0, MIN.1))) {
        log::warn!("splash: could not set the minimum size: {e}");
    }
    if maximized
        && let Err(e) = w.maximize()
    {
        log::warn!("splash: could not maximize the window: {e}");
    }
}

#[tauri::command]
pub async fn splash_done(app: AppHandle) {
    if let Err(e) = tauri::async_runtime::spawn_blocking(move || grow(&app, false)).await {
        log::warn!("splash: grow failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_from_the_splash_to_the_target() {
        let a = Rect { x: 740, y: 390, w: 440, h: 300 };
        let b = Rect { x: 320, y: 140, w: 1280, h: 800 };
        assert_eq!(lerp(a, b, 0.0), a);
        assert_eq!(lerp(a, b, 1.0), b);
        assert_eq!(lerp(a, b, 0.5), Rect { x: 530, y: 265, w: 860, h: 550 });
        assert_eq!(centered(b, 440, 300), a);
    }
}
