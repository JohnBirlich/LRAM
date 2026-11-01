//! Roblox login / session windows on throw-away WebView2 profiles.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tauri::webview::{Cookie, PlatformWebview};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use url::Url;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_COOKIE_SAME_SITE_KIND_LAX, ICoreWebView2_2, ICoreWebView2Settings4,
};
use windows::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
use windows::core::{Interface, PCWSTR, w};
use zeroize::Zeroizing;

use crate::error::{AppError, Result};
use crate::roblox_api::endpoints;
use crate::vault::Secret;

const TMP_DIR: &str = "tmp-webview";
const POLL: Duration = Duration::from_millis(700);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const HOME_PAGE: &str = "https://www.roblox.com/home";

/// Variables WebView2 reads before (and instead of) the options Tauri passes it.
const WEBVIEW2_VARS: [&str; 6] = [
    "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
    "WEBVIEW2_USER_DATA_FOLDER",
    "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
    "WEBVIEW2_RELEASE_CHANNELS",
    "WEBVIEW2_CHANNEL_SEARCH_KIND",
    "WEBVIEW2_RELEASE_CHANNEL_PREFERENCE",
];

static LOGIN_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Opens `https://www.roblox.com/login` in a `login-<uuid>` window on a fresh profile under
/// `<data_dir>/tmp-webview/<uuid>` (incognito, no devtools, no autofill, no context menu,
/// no IPC capability). A worker thread polls `cookies()` every ~700 ms until a non-empty
/// `.ROBLOSECURITY` appears, then closes the window, waits for its browser process to exit
/// and deletes the profile. Closing the window first → `AppError::Cancelled`.
pub async fn capture_login(app: &AppHandle, data_dir: &Path) -> Result<Secret> {
    let slot = LoginSlot::take(app)?;
    let id = uuid::Uuid::new_v4();
    let label = format!("login-{id}");
    let profile = data_dir.join(TMP_DIR).join(id.to_string());
    let built = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(parse_url(endpoints::LOGIN_PAGE)?))
        .title(tr!("Sign in to Roblox — LRAM"))
        .inner_size(480.0, 760.0)
        .center()
        .data_directory(profile.clone())
        .incognito(true)
        .devtools(cfg!(debug_assertions))
        .general_autofill_enabled(false)
        .on_navigation(allowed)
        .build();
    let window = match built {
        Ok(w) => w,
        Err(e) => {
            cleanup(None, profile).await;
            return Err(e.into());
        }
    };
    let (pid, cookie) = match harden(&window, None).await {
        Ok(pid) => (Some(pid), wait_for_cookie(app.clone(), label).await),
        Err(e) => (None, Err(e)),
    };
    if let Err(e) = window.destroy() {
        log::debug!("could not destroy login window login-{id}: {e}");
    }
    drop(slot);
    cleanup(pid, profile).await;
    cookie
}

/// Opens `https://www.roblox.com/home` signed in as a stored account: a `session-<uuid>`
/// window titled `title` on a fresh profile under `<data_dir>/tmp-webview/<uuid>` (not
/// incognito-shared, no devtools in release, no IPC capability, same hardening as login).
/// The cookie is injected through WebView2's own `ICoreWebView2CookieManager` with domain
/// `.roblox.com`, path `/`, HttpOnly, Secure, SameSite=Lax (never Tauri's `set_cookie`, which
/// drops the leading dot), before the first navigation. Returns once the window shows.
/// When the user closes it, `on_close` gets the profile's last non-empty `.ROBLOSECURITY`
/// (Roblox may have rotated it, or the user may have signed out / switched accounts), read
/// before the window is destroyed; then the profile is deleted as in `capture_login`.
pub async fn open_session(
    app: &AppHandle,
    data_dir: &Path,
    title: &str,
    cookie: Secret,
    on_close: impl FnOnce(Option<Secret>) + Send + 'static,
) -> Result<()> {
    let home = parse_url(HOME_PAGE)?;
    let id = uuid::Uuid::new_v4();
    let label = format!("session-{id}");
    let profile = data_dir.join(TMP_DIR).join(id.to_string());
    // Blank first, so nothing loads from Roblox before the cookie is in place.
    let built = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(parse_url("about:blank")?))
        .title(title)
        .inner_size(1180.0, 800.0)
        .center()
        .data_directory(profile.clone())
        .incognito(true)
        .devtools(cfg!(debug_assertions))
        .general_autofill_enabled(false)
        .on_navigation(allowed)
        .build();
    let window = match built {
        Ok(w) => w,
        Err(e) => {
            cleanup(None, profile).await;
            return Err(e.into());
        }
    };
    let pid = match harden(&window, Some(cookie)).await {
        Ok(pid) => pid,
        Err(e) => {
            if let Err(e) = window.destroy() {
                log::debug!("could not destroy session window {label}: {e}");
            }
            cleanup(None, profile).await;
            return Err(e);
        }
    };
    // ponytail: a close in the instant before this handler exists skips on_close; the start-up
    // sweep deletes the profile, as after an app exit.
    let (app, title) = (app.clone(), title.to_owned());
    let pending = Mutex::new(Some(move || end_session(&app, &label, &title, pid, &profile, on_close)));
    window.on_window_event(move |event| {
        // `cookies()` deadlocks in here: keep the window and finish on another thread.
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            if let Some(end) = take_once(&pending) {
                tauri::async_runtime::spawn_blocking(end);
            }
        }
    });
    window.navigate(home)?;
    Ok(())
}

/// Deletes leftover `tmp-webview/*` profiles from earlier runs.
pub fn sweep_temp_profiles(data_dir: &Path) {
    let entries = match std::fs::read_dir(data_dir.join(TMP_DIR)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return,
        Err(e) => {
            log::warn!("could not list old browser profiles: {e}");
            return;
        }
    };
    let entries = entries.filter_map(|e| e.inspect_err(|e| log::debug!("could not read an old browser profile entry: {e}")).ok());
    for path in entries.map(|e| e.path()) {
        let removed = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        if let Err(e) = removed {
            log::warn!("could not delete old browser profile {}: {e}", path.display());
        }
    }
}

/// Removes `WEBVIEW2_*` override variables so they cannot redirect or merge profiles.
/// Called first thing in `main`, before any thread starts.
pub fn clear_webview2_env() {
    for name in WEBVIEW2_VARS {
        // SAFETY: runs first thing in `main`, before any other thread exists, so nothing can
        // read the environment at the same time.
        unsafe { std::env::remove_var(name) };
    }
}

/// One login window at a time (a double click must not open two).
struct LoginSlot;

impl LoginSlot {
    fn take(app: &AppHandle) -> Result<Self> {
        if !LOGIN_ACTIVE.swap(true, Ordering::AcqRel) {
            return Ok(Self);
        }
        if let Some(w) = app.webview_windows().into_iter().find_map(|(l, w)| l.starts_with("login-").then_some(w)) {
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
        Err(AppError::Invalid(tr!("Finish the Roblox sign-in window that's already open.")))
    }
}

impl Drop for LoginSlot {
    fn drop(&mut self) {
        LOGIN_ACTIVE.store(false, Ordering::Release);
    }
}

/// Turns off what Tauri does not expose, adds `cookie` as `.ROBLOSECURITY` if given, and
/// returns the profile's browser process id.
async fn harden(window: &WebviewWindow, cookie: Option<Secret>) -> Result<u32> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    window.with_webview(move |pv| {
        let r = harden_webview(&pv, cookie.as_ref()).map_err(|e| AppError::Internal(tr!("WebView2 setup failed: {why}", why = e)));
        let _ = tx.send(r);
    })?;
    // The closure is dropped unrun if the window is already gone.
    rx.await.map_err(|_| {
        log::debug!("webview setup skipped: window {} already gone", window.label());
        AppError::Cancelled
    })?
}

fn harden_webview(pv: &PlatformWebview, cookie: Option<&Secret>) -> windows::core::Result<u32> {
    // SAFETY: COM calls on the main thread (with_webview), on live interfaces from WebView2;
    // `value` outlives the call that reads it.
    unsafe {
        let wv = pv.controller().CoreWebView2()?;
        let settings = wv.Settings()?;
        settings.SetAreDefaultContextMenusEnabled(false)?;
        let settings = settings.cast::<ICoreWebView2Settings4>()?;
        settings.SetAreBrowserAcceleratorKeysEnabled(false)?;
        settings.SetIsPasswordAutosaveEnabled(false)?;
        // As well as on_navigation: Tauri lets through any address it can't parse, and a page on
        // such a host panics Tauri's IPC, which ends the app.
        let guard = webview2_com::NavigationStartingEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args {
                let mut uri = windows::core::PWSTR::null();
                // A live args interface; `take_pwstr` frees the string it hands out.
                args.Uri(&mut uri)?;
                if !allowed_uri(&webview2_com::take_pwstr(uri)) {
                    args.SetCancel(true)?;
                }
            }
            Ok(())
        }));
        let mut token = Default::default();
        wv.add_NavigationStarting(&guard, &mut token)?;
        if let Some(cookie) = cookie {
            // WebView2's own manager: `.roblox.com` keeps its dot (Tauri's `set_cookie` drops it).
            let value = wide(cookie.expose());
            let mgr = wv.cast::<ICoreWebView2_2>()?.CookieManager()?;
            let c = mgr.CreateCookie(w!(".ROBLOSECURITY"), PCWSTR(value.as_ptr()), w!(".roblox.com"), w!("/"))?;
            c.SetIsHttpOnly(true)?;
            c.SetIsSecure(true)?;
            c.SetSameSite(COREWEBVIEW2_COOKIE_SAME_SITE_KIND_LAX)?;
            mgr.AddOrUpdateCookie(&c)?;
        }
        let mut pid = 0;
        wv.BrowserProcessId(&mut pid)?;
        Ok(pid)
    }
}

/// Logs WebView2 renderer/GPU/browser process failures of `window` (a white screen otherwise
/// leaves no trace).
pub fn log_process_failures(window: &WebviewWindow) {
    let label = window.label().to_string();
    let r = window.with_webview(move |pv| {
        use webview2_com::ProcessFailedEventHandler;
        let label = label.clone();
        let handler = ProcessFailedEventHandler::create(Box::new(move |_, args| {
            let mut kind = Default::default();
            if let Some(args) = args {
                // SAFETY: a live args interface handed to the event handler.
                unsafe { args.ProcessFailedKind(&mut kind)? };
            }
            log::error!("WebView2 process failed in window \"{label}\" (kind {})", kind.0);
            Ok(())
        }));
        let mut token = Default::default();
        // SAFETY: COM call on the main thread (with_webview) on a live controller.
        let added = unsafe { pv.controller().CoreWebView2().and_then(|wv| wv.add_ProcessFailed(&handler, &mut token)) };
        if let Err(e) = added {
            log::warn!("could not watch WebView2 process failures: {e}");
        }
    });
    if let Err(e) = r {
        log::warn!("could not watch WebView2 process failures: {e}");
    }
}

/// NUL-terminated UTF-16 for a `PCWSTR`, zeroed on drop.
fn wide(s: &str) -> Zeroizing<Vec<u16>> {
    Zeroizing::new(s.encode_utf16().chain([0]).collect())
}

fn parse_url(s: &str) -> Result<Url> {
    s.parse().map_err(|e: url::ParseError| AppError::Internal(e.to_string()))
}

/// The first close request gets the close path; clicks while it runs get nothing.
fn take_once<T>(slot: &Mutex<Option<T>>) -> Option<T> {
    slot.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Blocking (see `poll_cookie`): reads the profile's cookie while the window still exists,
/// hands it to `on_close`, then destroys the window and deletes the profile.
fn end_session(app: &AppHandle, label: &str, title: &str, pid: u32, profile: &Path, on_close: impl FnOnce(Option<Secret>)) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.hide();
        let last = match window.cookies().map(pick_roblosecurity) {
            Ok(Some(c)) => Some(c),
            Ok(None) => {
                log::warn!("session window \"{title}\" closed signed out of Roblox; stored cookie not changed");
                None
            }
            Err(e) => {
                log::warn!("could not read the cookies of session window \"{title}\": {e}");
                None
            }
        };
        on_close(last);
        if let Err(e) = window.destroy() {
            log::debug!("could not destroy session window {label}: {e}");
        }
    }
    wait_for_exit(pid);
    remove_profile(profile);
}

async fn wait_for_cookie(app: AppHandle, label: String) -> Result<Secret> {
    tauri::async_runtime::spawn_blocking(move || poll_cookie(&app, &label)).await?
}

/// Blocking: `cookies()` deadlocks on the main thread and in event handlers.
fn poll_cookie(app: &AppHandle, label: &str) -> Result<Secret> {
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    let mut failures = 0;
    while Instant::now() < deadline {
        // Also how we notice the user closing the window.
        let Some(window) = app.get_webview_window(label) else {
            return Err(AppError::Cancelled);
        };
        // Signing in (2-step codes, captchas) is activity: don't auto-lock under it, or the
        // captured cookie could not be encrypted.
        app.state::<crate::App>().vault.touch();
        // Not `cookies_for_url`: it panics if the window goes away mid-call.
        match window.cookies() {
            Ok(all) => {
                failures = 0;
                if let Some(cookie) = pick_roblosecurity(all) {
                    return Ok(cookie);
                }
            }
            Err(e) => {
                failures += 1;
                log::debug!("login window {label}: reading cookies failed ({failures}/5): {e}");
                if failures == 5 {
                    return Err(e.into());
                }
            }
        }
        std::thread::sleep(POLL);
    }
    Err(AppError::Invalid(tr!("The Roblox sign-in window timed out after 15 minutes. Try again.")))
}

/// The first non-empty `.ROBLOSECURITY` from roblox.com (Tauri strips the leading dot).
fn pick_roblosecurity(cookies: Vec<Cookie<'static>>) -> Option<Secret> {
    cookies
        .into_iter()
        .find(|c| c.name() == ".ROBLOSECURITY" && !c.value().is_empty() && c.domain().is_some_and(is_roblox))
        .map(|c| Secret::new(c.value()))
}

/// Where a Roblox window may go: Roblox over https, and the sign-in providers it hands off to.
/// A Tauri window's page can crash the app through its IPC with a crafted host name, so it stays
/// off anything else.
fn allowed(url: &Url) -> bool {
    let ok = url.as_str() == "about:blank"
        || (url.scheme() == "https"
            && url.username().is_empty()
            && url.port().is_none()
            && url.host_str().is_some_and(|h| is_roblox(h) || ["apple.com", "google.com"].iter().any(|d| h == *d || h.ends_with(&format!(".{d}")))));
    if !ok {
        log::debug!("Roblox window: navigation to another site refused");
    }
    ok
}

fn allowed_uri(uri: &str) -> bool {
    uri.parse().is_ok_and(|u| allowed(&u))
}

fn is_roblox(domain: &str) -> bool {
    let d = domain.trim_start_matches('.').to_ascii_lowercase();
    d == "roblox.com" || d.ends_with(".roblox.com")
}

async fn cleanup(pid: Option<u32>, profile: PathBuf) {
    let _ = tauri::async_runtime::spawn_blocking(move || {
        if let Some(pid) = pid {
            wait_for_exit(pid);
        }
        remove_profile(&profile);
    })
    .await;
}

/// A profile folder stays locked until its browser process has exited (≤ 5 s here).
fn wait_for_exit(pid: u32) {
    // SAFETY: the handle is used only here and closed before returning.
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            if WaitForSingleObject(h, 5_000) == WAIT_TIMEOUT {
                log::debug!("browser process {pid} still running after 5 s");
            }
            let _ = CloseHandle(h);
        }
    }
}

/// Child processes can hold files a moment longer, so retry; the start-up sweep catches the rest.
fn remove_profile(dir: &Path) {
    for attempt in 1..=5 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return,
            Err(e) if e.kind() == ErrorKind::NotFound => return,
            Err(e) if attempt == 5 => log::warn!("could not delete browser profile {}: {e}", dir.display()),
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(name: &str, value: &str, domain: &str) -> Cookie<'static> {
        Cookie::build((name.to_owned(), value.to_owned())).domain(domain.to_owned()).build()
    }

    fn picked(cookies: Vec<Cookie<'static>>) -> Option<String> {
        pick_roblosecurity(cookies).map(|s| s.expose().to_owned())
    }

    #[test]
    fn picks_only_a_non_empty_roblox_cookie() {
        let jar = vec![
            cookie(".ROBLOSECURITY", "fake", "evil.com"),
            cookie(".ROBLOSECURITY", "", ".roblox.com"),
            cookie("GuestData", "guest", ".roblox.com"),
            cookie(".ROBLOSECURITY", "good", ".roblox.com"),
        ];
        assert_eq!(picked(jar), Some("good".into()));
        assert_eq!(picked(vec![cookie(".ROBLOSECURITY", "w", "www.roblox.com")]), Some("w".into()));
        assert_eq!(picked(vec![cookie(".ROBLOSECURITY", "x", "notroblox.com")]), None);
        assert_eq!(picked(vec![cookie(".ROBLOSECURITY", "x", "roblox.com.evil.com")]), None);
        assert_eq!(picked(vec![Cookie::new(".ROBLOSECURITY", "no-domain")]), None);
        assert_eq!(picked(vec![cookie(".roblosecurity", "x", "roblox.com")]), None);
        assert_eq!(picked(vec![]), None);
    }

    #[test]
    fn roblox_windows_stay_on_roblox() {
        let go = allowed_uri;
        for u in ["about:blank", "https://www.roblox.com/home", "https://roblox.com/", "https://appleid.apple.com/x", "https://accounts.google.com/"] {
            assert!(go(u), "{u}");
        }
        for u in [
            "http://www.roblox.com/",
            "http://tauri.localhost/",
            "https://tauri.localhost/",
            "http://x:y@tauri.localhost:1234/",
            "https://evil.com/",
            "https://roblox.com.evil.com/",
            "https://x@www.roblox.com/",
            "https://www.roblox.com:8443/",
            "file:///C:/x",
            "http://xn--a.localhost:1/",
        ] {
            assert!(!go(u), "{u}");
        }
    }

    #[test]
    fn session_helpers() {
        assert_eq!(*wide("aZ_9"), [97, 90, 95, 57, 0]);
        assert_eq!(*wide(""), [0]);
        assert_eq!(parse_url(HOME_PAGE).unwrap().host_str(), Some("www.roblox.com"));
        assert!(parse_url("about:blank").is_ok());
    }

    #[test]
    fn close_path_runs_once() {
        let pending = Mutex::new(Some(7));
        assert_eq!(take_once(&pending), Some(7));
        assert_eq!(take_once(&pending), None);
    }

    #[test]
    fn sweep_removes_leftover_profiles() {
        let root = std::env::temp_dir().join(format!("roalt-sweep-{}", uuid::Uuid::new_v4()));
        let tmp = root.join(TMP_DIR);
        std::fs::create_dir_all(tmp.join("a").join("EBWebView")).unwrap();
        std::fs::write(tmp.join("a").join("EBWebView").join("Cookies"), b"x").unwrap();
        std::fs::create_dir_all(tmp.join("b")).unwrap();
        std::fs::write(tmp.join("stray.tmp"), b"x").unwrap();
        sweep_temp_profiles(&root);
        assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0);
        sweep_temp_profiles(&root.join("missing"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn clears_webview2_overrides() {
        for name in WEBVIEW2_VARS {
            // SAFETY: nothing else in the test binary reads these variables, and std serialises
            // its own environment access on Windows.
            unsafe { std::env::set_var(name, "x") };
        }
        clear_webview2_env();
        assert!(WEBVIEW2_VARS.iter().all(|n| std::env::var_os(n).is_none()));
    }
}
