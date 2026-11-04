use std::sync::{Mutex, MutexGuard};

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::App;
use crate::commands::{self, notice};
use crate::error::{AppError, Result};
use crate::model::{AccountStatus, BulkRequest, SameServer, VaultMode};

const TRAY: &str = "main";

#[derive(PartialEq)]
struct View {
    favourites: Vec<(i64, String)>,
    locked: bool,
    master: bool,
    running: bool,
    lang: &'static str,
}

static SHOWN: Mutex<Option<View>> = Mutex::new(None);

fn shown() -> MutexGuard<'static, Option<View>> {
    SHOWN.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let view = view(app);
    let mut tray = TrayIconBuilder::with_id(TRAY)
        .tooltip("LRAM")
        .menu(&menu(app, &view)?)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| on_menu(app, e.id().as_ref()))
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    *shown() = Some(view);
    Ok(())
}

// -- Main func
pub fn refresh(app: &AppHandle) {
    let handle = app.clone();
    let queued = app.run_on_main_thread(move || {
        let view = view(&handle);
        if shown().as_ref() == Some(&view) {
            return;
        }
        let Some(tray) = handle.tray_by_id(TRAY) else {
            log::warn!("tray icon missing; menu not updated");
            return;
        };
        match menu(&handle, &view).and_then(|m| tray.set_menu(Some(m))) {
            Ok(()) => *shown() = Some(view),
            Err(e) => log::warn!("could not update the tray menu: {e}"),
        }
    });
    if let Err(e) = queued {
        log::warn!("could not update the tray menu: {e}");
    }
}

fn view(app: &AppHandle) -> View {
    let s = app.state::<App>();
    let favourites = match s.db.favourites() {
        Ok(rows) => rows.into_iter().filter(|f| f.in_tray).map(|f| (f.id, f.name)).collect(),
        Err(e) => {
            log::warn!("tray: could not read favourites: {e}");
            Vec::new()
        }
    };
    View {
        favourites,
        locked: s.vault.is_locked(),
        master: s.vault.mode() == VaultMode::Master,
        running: s.tracker.any_running(),
        lang: crate::i18n::current(),
    }
}

fn menu(app: &AppHandle, v: &View) -> tauri::Result<Menu<Wry>> {
    let item = |id: &str, text: &str, enabled: bool| MenuItem::with_id(app, id, text, enabled, None::<&str>);
    let favourites = Submenu::new(app, tr!("Favourites"), true)?;
    if v.favourites.is_empty() {
        favourites.append(&MenuItem::new(app, tr!("No favourites"), false, None::<&str>)?)?;
    } else if v.locked {
        favourites.append(&MenuItem::new(app, tr!("Unlock LRAM to launch"), false, None::<&str>)?)?;
    } else {
        for (id, name) in &v.favourites {
            favourites.append(&item(&format!("fav:{id}"), &name.replace('&', "&&"), true)?)?;
        }
    }
    let menu = Menu::with_items(
        app,
        &[
            &item("show", &tr!("Show LRAM"), true)?,
            &favourites,
            &item("close-all", &tr!("Close all clients"), v.running)?,
            &item("kill-all", &tr!("Stop all Roblox"), true)?,
        ],
    )?;
    if v.master {
        menu.append(&item("lock", &tr!("Lock vault"), !v.locked)?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&item("quit", &tr!("Quit LRAM"), true)?)?;
    Ok(menu)
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        for r in [w.show(), w.unminimize(), w.set_focus()] {
            if let Err(e) = r {
                log::debug!("tray: could not show the main window: {e}");
            }
        }
    }
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "show" => show_main(app),
        "close-all" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = commands::close_running(&app.state::<App>(), false).await {
                    notice(&app, true, tr!("Couldn't close the clients: {why}", why = e.message()));
                }
            });
        }
        "kill-all" => kill_all(app),
        "lock" if app.state::<App>().vault.mode() == VaultMode::Master => commands::lock_vault(app),
        "quit" => app.exit(0),
        _ => {
            if let Some(fav) = id.strip_prefix("fav:").and_then(|n| n.parse().ok()) {
                launch_favourite(app, fav);
            } else {
                log::debug!("tray: unknown menu id {id}");
            }
        }
    }
}

pub fn kill_all(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = commands::kill_all(&app.state::<App>()).await {
            notice(&app, true, tr!("Couldn't kill every Roblox client: {why}", why = e.message()));
        }
    });
}

fn launch_favourite(app: &AppHandle, id: i64) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = launch(&app, id).await {
            notice(&app, true, tr!("Couldn't launch that favourite: {why}", why = e.message()));
        }
    });
}

async fn launch(app: &AppHandle, id: i64) -> Result<()> {
    let s = app.state::<App>();
    let row = s.db.favourites()?.into_iter().find(|f| f.id == id).ok_or(AppError::NotFound)?;
    let fav = commands::favourite_of(&s, row)?;
    let ids: Vec<i64> = fav
        .account_ids
        .iter()
        .copied()
        .filter(|&a| match s.db.account(a) {
            Ok(a) => a.status == AccountStatus::Valid,
            Err(e) => {
                log::warn!("tray: could not read account {a} of favourite {id}: {e}");
                false
            }
        })
        .collect();
    match ids.len() {
        0 => {
            show_main(app);
            notice(app, false, tr!("No account of \"{name}\" can launch. Pick accounts for it in LRAM.", name = fav.name));
        }
        1 => {
            if !s.multi.is_active() && s.tracker.list().iter().any(|i| i.account_id != ids[0] && i.state != crate::model::InstanceState::Exited) {
                show_main(app);
                notice(app, false, tr!("\"{name}\" wasn't started: another client is running and multi-instance is off.", name = fav.name));
                return Ok(());
            }
            let _single = s.bulk.begin_single_launch()?;
            commands::launch_tracked(app, &s, ids[0], fav.target, Some(fav.id), None, None, true).await?;
        }
        _ => {
            let delay_ms = s.db.settings()?.launch_delay_ms;
            let req = BulkRequest {
                account_ids: ids,
                target: fav.target,
                delay_ms,
                same_server: SameServer::PickFirst,
                favourite_id: Some(fav.id),
                fps_cap: None,
            };
            crate::launcher::bulk::start(app.clone(), req)?;
        }
    }
    Ok(())
}
