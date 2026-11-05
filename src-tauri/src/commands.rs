use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::error::{AppError, Result};
use crate::model::*;
use crate::storage::{AccountMeta, NewAccount};
use crate::vault::backup::{self, BackupAccount, BackupGroup, BackupPayload};
use crate::vault::{Secret, cookie_aad};
use crate::roblox_api::types::{AuthenticatedUser, GameDetails, UserPresence};
use crate::{App, webview};

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn accounts_changed(app: &AppHandle) {
    let _ = app.emit("accounts://changed", ());
}

#[tauri::command]
pub fn vault_status(s: State<'_, App>) -> Result<VaultStatus> {
    Ok(VaultStatus {
        mode: s.vault.mode(),
        locked: s.vault.is_locked(),
        accounts: s.db.accounts()?.len() as u32,
        auto_lock_minutes: s.db.settings()?.auto_lock_minutes,
        has_recovery: s.db.recovery_record()?.is_some(),
        retry_in: s.vault.retry_in(),
    })
}

#[tauri::command]
pub async fn vault_unlock(app: AppHandle, s: State<'_, App>, password: String) -> Result<()> {
    let rec = s.db.vault_record()?.ok_or(AppError::Internal("vault record missing".into()))?;
    s.vault.unlock(&rec, &password)?;
    after_unlock(&app, &s);
    Ok(())
}

fn after_unlock(app: &AppHandle, s: &App) {
    let later = std::mem::take(&mut *s.rotated_later.lock().unwrap_or_else(|e| e.into_inner()));
    later.into_iter().for_each(|(id, cookie)| s.store_rotated(id, cookie));
    crate::tray::refresh(app);
}

#[tauri::command]
pub async fn vault_recover(app: AppHandle, s: State<'_, App>, code: String, new_password: String) -> Result<String> {
    let recovery = s.db.recovery_record()?.ok_or(AppError::WrongPassword)?;
    let (rec, new_code, new_recovery) = crate::vault::recover(&recovery, &code, &new_password)?;
    s.db.save_vault_and_recovery(&rec, &new_recovery)?;
    s.vault.unlock_after_recovery(&rec, &new_password)?;
    after_unlock(&app, &s);
    Ok(new_code.expose().to_owned())
}

#[tauri::command]
pub async fn vault_recovery_create(s: State<'_, App>, current_password: String) -> Result<String> {
    let rec = s.db.vault_record()?.ok_or(AppError::Internal("vault record missing".into()))?;
    if rec.mode != VaultMode::Master {
        return Err(AppError::Invalid(tr!("No master password is set.")));
    }
    s.vault.check_password(&rec, &current_password)?;
    let (code, recovery) = s.vault.seal_recovery()?;
    s.db.save_recovery_record(&recovery)?;
    Ok(code.expose().to_owned())
}

fn recovery_file_text(code: &str) -> Result<String> {
    let code = crate::vault::master::normalize_code(code).ok_or_else(not_a_code)?;
    Ok(tr!(
        "LRAM recovery code: {code}\n\nIf you forget your master password, press Forgot password? on the unlock screen and enter this code. \
         It works once, for this Windows user on this PC.\n\nAnyone who finds this file on this PC can open your vault. Keep it on a USB stick, \
         a phone or paper.\n",
        code = code.expose()
    ))
}

fn not_a_code() -> AppError {
    AppError::Invalid(tr!("That isn't a recovery code."))
}

#[tauri::command]
pub async fn vault_recovery_save(path: String, code: String) -> Result<()> {
    crate::vault::backup::write_atomic(Path::new(&path), recovery_file_text(&code)?.as_bytes())
}

#[tauri::command]
pub async fn vault_recovery_copy(app: AppHandle, s: State<'_, App>, code: String) -> Result<()> {
    let code = crate::vault::master::normalize_code(&code).ok_or_else(not_a_code)?;
    copy_secret(&app, &s, code)
}

#[tauri::command]
pub fn vault_lock(app: AppHandle, s: State<'_, App>) {
    if s.vault.mode() == VaultMode::Master {
        lock_vault(&app);
    }
}

#[tauri::command]
pub async fn vault_set_mode(
    app: AppHandle,
    s: State<'_, App>,
    mode: VaultMode,
    current_password: Option<String>,
    new_password: Option<String>,
) -> Result<()> {
    let rec = s.db.vault_record()?.ok_or(AppError::Internal("vault record missing".into()))?;
    if rec.mode == VaultMode::Dpapi && mode == VaultMode::Master && crate::vault::hello_available().await {
        reauth(&app, &s, None, &tr!("Confirm it's you to set a master password.")).await?;
    }
    let new_rec = s.vault.rewrap(&rec, mode, current_password.as_deref(), new_password.as_deref())?;
    s.db.save_vault_record(&new_rec)?;
    s.vault.set_mode(mode);
    crate::tray::refresh(&app);
    Ok(())
}

#[tauri::command]
pub fn app_activity(s: State<'_, App>) {
    s.vault.touch();
}

#[tauri::command]
pub fn accounts_list(s: State<'_, App>) -> Result<Vec<AccountSummary>> {
    s.db.accounts()
}

fn normalize_cookie(raw: &str) -> Result<Secret> {
    let v = raw.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    let v = v.strip_prefix(".ROBLOSECURITY=").unwrap_or(v).trim_end_matches(';').trim();
    if v.is_empty() || v.len() > 8192 || !v.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(AppError::Invalid(tr!("That doesn't look like a .ROBLOSECURITY cookie.")));
    }
    Ok(Secret::new(v))
}

async fn add_account(app: &AppHandle, s: &App, cookie: Secret) -> Result<AccountSummary> {
    let (user, rotated): (AuthenticatedUser, _) = s.api.authenticated_user(0, &cookie).await?;
    let cookie = rotated.unwrap_or(cookie);
    let blob = s.vault.encrypt(&cookie_aad(user.id), cookie.expose().as_bytes())?;
    let id = s.db.upsert_account(&NewAccount {
        user_id: user.id,
        username: &user.name,
        display_name: &user.display_name,
        cookie: &blob,
        status: AccountStatus::Valid,
        checked_at: Some(now_ms()),
    })?;
    accounts_changed(app);
    s.db.account(id)
}

#[tauri::command]
pub async fn account_add_cookie(app: AppHandle, s: State<'_, App>, cookie: String) -> Result<AccountSummary> {
    if s.vault.is_locked() {
        return Err(AppError::Locked);
    }
    if !cookie.contains(COOKIE_MARK) && looks_like_credentials(&cookie) {
        return Err(AppError::Invalid(not_a_cookie()));
    }
    add_account(&app, &s, normalize_cookie(&cookie)?).await
}

#[tauri::command]
pub async fn account_add_login(app: AppHandle, s: State<'_, App>) -> Result<AccountSummary> {
    if s.vault.is_locked() {
        return Err(AppError::Locked);
    }
    let cookie = webview::capture_login(&app, &s.data_dir).await?;
    add_account(&app, &s, cookie).await
}

const COOKIE_MARK: &str = "_|WARNING:-DO-NOT-SHARE-THIS.";

fn not_a_cookie() -> String {
    tr!("That is a username and password, not a cookie: Roblox no longer signs in from those. Use \"Sign in with Roblox\", or paste the .ROBLOSECURITY cookie of a browser where the account is signed in.")
}

fn looks_like_credentials(text: &str) -> bool {
    text.lines().filter(|line| !line.contains(COOKIE_MARK)).any(|line| {
        let Some((user, rest)) = line.trim().split_once(':') else { return false };
        let (user, pass) = (user.trim(), rest.trim());
        !user.is_empty()
            && !pass.is_empty()
            && user.len() <= 64
            && !user.contains(char::is_whitespace)
            && !pass.contains(char::is_whitespace)
    })
}

fn find_cookies(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let mut rest = line;
        while let Some(at) = rest.find(COOKIE_MARK) {
            let tail = &rest[at..];
            let body = tail.find("|_").map_or(COOKIE_MARK.len(), |i| i + 2);
            let end = tail[body..].find(|c: char| !c.is_ascii_graphic() || "\"';,:".contains(c)).map_or(tail.len(), |i| body + i);
            let c = &tail[..end];
            if end > body && !out.iter().any(|o| o == c) {
                out.push(c.to_owned());
            }
            rest = &tail[end..];
        }
    }
    out
}

async fn import_cookies(app: &AppHandle, s: &App, cookies: Vec<(Secret, ImportExtras)>) -> Result<ImportReport> {
    let mut report = ImportReport::default();
    let before: Vec<i64> = s.db.accounts()?.iter().map(|a| a.id).collect();
    for (i, (cookie, extras)) in cookies.into_iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        match add_account(app, s, cookie).await {
            Ok(a) => {
                if before.contains(&a.id) { report.updated += 1 } else { report.added += 1 }
                let patch = AccountPatch {
                    alias: Some(extras.alias.chars().take(64).collect()).filter(|v: &String| !v.trim().is_empty()),
                    notes: Some(extras.notes.chars().take(2000).collect()).filter(|v: &String| !v.trim().is_empty()),
                    ..Default::default()
                };
                if (patch.alias.is_some() || patch.notes.is_some()) && let Err(e) = s.db.update_account(a.id, &patch) {
                    log::warn!("import: details of account {} not kept: {e}", a.id);
                }
            }
            Err(AppError::Locked) => return Err(AppError::Locked),
            Err(e) => {
                log::info!("import: a cookie was refused: {e}");
                report.failed += 1;
            }
        }
    }
    accounts_changed(app);
    Ok(report)
}

#[derive(Default)]
struct ImportExtras {
    alias: String,
    notes: String,
}

#[tauri::command]
pub async fn accounts_import_cookies(app: AppHandle, s: State<'_, App>, text: String) -> Result<ImportReport> {
    if s.vault.is_locked() {
        return Err(AppError::Locked);
    }
    let found = find_cookies(&text);
    if found.is_empty() {
        return Err(AppError::Invalid(if looks_like_credentials(&text) {
            not_a_cookie()
        } else {
            tr!("No .ROBLOSECURITY cookies found. Each starts with _|WARNING:-DO-NOT-SHARE-THIS.")
        }));
    }
    let total = text.matches(COOKIE_MARK).count();
    let cookies = found.iter().map(|c| normalize_cookie(c).map(|c| (c, ImportExtras::default()))).collect::<Result<Vec<_>>>()?;
    let mut report = import_cookies(&app, &s, cookies).await?;
    report.skipped = total.saturating_sub(found.len()) as u32;
    Ok(report)
}

const RAM_ENTROPY: &[u8] = b"ROBLOX ACCOUNT MANAGER | :) | BROUGHT TO YOU BUY ic3w0lf";
const RAM_HEADER: &[u8] = b"Roblox Account Manager created by ic3w0lf22 @ github.com .......";

#[derive(serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RamAccount {
    #[serde(default)]
    security_token: Option<String>,
    #[serde(default)]
    alias: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

fn ram_accounts(json: &[u8]) -> Result<Vec<(Option<String>, ImportExtras)>> {
    let list: Vec<RamAccount> = serde_json::from_slice(json.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(json))
        .map_err(|e| {
            log::debug!("RAM AccountData.json unreadable: {:?} at {}:{}", e.classify(), e.line(), e.column());
            AppError::Invalid(tr!("That isn't a Roblox Account Manager AccountData.json."))
        })?;
    Ok(list
        .into_iter()
        .map(|a| {
            let cookie = a.security_token.filter(|t| !t.trim().is_empty());
            (cookie, ImportExtras { alias: a.alias.unwrap_or_default(), notes: a.description.unwrap_or_default() })
        })
        .collect())
}

#[tauri::command]
pub async fn accounts_import_ram(app: AppHandle, s: State<'_, App>, path: String) -> Result<ImportReport> {
    if s.vault.is_locked() {
        return Err(AppError::Locked);
    }
    let meta = std::fs::metadata(&path).map_err(|e| {
        log::warn!("could not read RAM file metadata: {e}");
        AppError::Invalid(tr!("LRAM couldn't read that file."))
    })?;
    if meta.len() > 32 * 1024 * 1024 {
        return Err(AppError::Invalid(tr!("That file is too big to be an AccountData.json.")));
    }
    let data = zeroize::Zeroizing::new(std::fs::read(&path).map_err(|e| {
        log::warn!("could not read RAM file: {e}");
        AppError::Invalid(tr!("LRAM couldn't read that file."))
    })?);
    if data.starts_with(RAM_HEADER) {
        return Err(AppError::Invalid(tr!(
            "This file is locked with a Roblox Account Manager password. Remove the password in Roblox Account Manager, then import again."
        )));
    }
    let entries = match crate::vault::unprotect_with(&data, RAM_ENTROPY).inspect_err(|e| log::debug!("RAM file not DPAPI-readable: {e}")) {
        Ok(plain) => ram_accounts(&plain)?,
        Err(_) if data.trim_ascii_start().starts_with(b"[") || data.starts_with(b"\xEF\xBB\xBF") => ram_accounts(&data)?,
        Err(_) => {
            return Err(AppError::Invalid(tr!(
                "LRAM couldn't decrypt this file. It must come from Roblox Account Manager on this PC, signed in as the same Windows user."
            )));
        }
    };
    let mut cookies = Vec::new();
    let mut failed = 0;
    for (cookie, extras) in entries {
        match cookie.as_deref().map(normalize_cookie) {
            Some(Ok(c)) => cookies.push((c, extras)),
            _ => failed += 1,
        }
    }
    if failed > 0 {
        log::debug!("RAM import: {failed} entries with a blank or malformed cookie");
    }
    let mut report = import_cookies(&app, &s, cookies).await?;
    report.failed += failed;
    Ok(report)
}

fn unlocked(s: &App) -> Result<()> {
    if s.vault.is_locked() { Err(AppError::Locked) } else { Ok(()) }
}

fn not_too_many(ids: &[i64]) -> Result<()> {
    if ids.len() > 20_000 { Err(AppError::Internal("too many ids".into())) } else { Ok(()) }
}

#[tauri::command]
pub fn account_update(app: AppHandle, s: State<'_, App>, id: i64, patch: AccountPatch) -> Result<AccountSummary> {
    unlocked(&s)?;
    if let Some(a) = &patch.launch_args {
        crate::launcher::launch_args(a)?;
    }
    if let Some(f) = patch.fps_cap {
        check_fps(f)?;
    }
    s.db.update_account(id, &patch)?;
    accounts_changed(&app);
    s.db.account(id)
}

#[tauri::command]
pub fn accounts_reorder(app: AppHandle, s: State<'_, App>, ids: Vec<i64>) -> Result<()> {
    unlocked(&s)?;
    not_too_many(&ids)?;
    s.db.reorder_accounts(&ids)?;
    accounts_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn accounts_remove(app: AppHandle, s: State<'_, App>, ids: Vec<i64>) -> Result<()> {
    unlocked(&s)?;
    not_too_many(&ids)?;
    s.db.remove_accounts(&ids)?;
    accounts_changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn accounts_validate(app: AppHandle, s: State<'_, App>, ids: Option<Vec<i64>>) -> Result<Vec<AccountSummary>> {
    let ids = match ids {
        Some(ids) => ids,
        None => s.db.accounts()?.iter().map(|a| a.id).collect(),
    };
    let (mut answered, mut last_err) = (0, None);
    for id in ids {
        let (user_id, blob) = match s.db.account_cookie(id) {
            Err(AppError::NotFound) => continue,
            r => r?,
        };
        let cookie = s
            .vault
            .decrypt_secret(&cookie_aad(user_id), &blob)
            .inspect_err(|e| log::warn!("could not decrypt the cookie of account {id}: {e}"))?;
        let now = now_ms();
        let stored = match s.api.authenticated_user(id, &cookie).await {
            Ok((u, _)) if u.id == user_id => {
                s.db.set_status(id, AccountStatus::Valid, now, Some(&u.name), Some(&u.display_name))
            }
            Ok(_) => s.db.set_status(id, AccountStatus::Invalid, now, None, None),
            Err(AppError::CookieInvalid | AppError::BoundSessionRequired) => {
                s.db.set_status(id, AccountStatus::Invalid, now, None, None)
            }
            Err(AppError::Challenge) => s.db.set_status(id, AccountStatus::Challenged, now, None, None),
            Err(e) => {
                log::warn!("could not validate account {id}: {e}");
                let limited = matches!(e, AppError::RateLimited(_));
                last_err = Some(e);
                if limited {
                    break;
                }
                continue;
            }
        };
        match stored {
            Err(AppError::NotFound) => continue,
            r => r?,
        }
        answered += 1;
        accounts_changed(&app);
    }
    accounts_changed(&app);
    match last_err {
        Some(e) if answered == 0 => Err(e),
        _ => s.db.accounts(),
    }
}

async fn reauth(app: &AppHandle, s: &App, password: Option<String>, prompt: &str) -> Result<()> {
    match s.vault.mode() {
        VaultMode::Master => {
            let rec = s.db.vault_record()?.ok_or(AppError::Internal("vault record missing".into()))?;
            s.vault.check_password(&rec, password.as_deref().ok_or(AppError::WrongPassword)?)
        }
        VaultMode::Dpapi => {
            let window = app.get_webview_window("main").ok_or(AppError::Internal("no main window".into()))?;
            crate::vault::windows_hello(&window, prompt).await
        }
    }
}

fn reveal_prompt() -> String {
    tr!("Confirm it's you to reveal a Roblox cookie.")
}

pub(crate) fn record_cookie_error(app: &AppHandle, s: &App, id: i64, e: &AppError) -> Result<bool> {
    let status = match e {
        AppError::CookieInvalid | AppError::BoundSessionRequired => AccountStatus::Invalid,
        AppError::Challenge => AccountStatus::Challenged,
        _ => return Ok(false),
    };
    log::info!("account {id} marked {status:?}: {e}");
    s.db.set_status(id, status, now_ms(), None, None)?;
    accounts_changed(app);
    Ok(true)
}

fn stored_cookie(s: &App, id: i64) -> Result<Secret> {
    let (user_id, blob) = s.db.account_cookie(id)?;
    s.vault.decrypt_secret(&cookie_aad(user_id), &blob)
}

#[tauri::command]
pub async fn account_reveal_cookie(app: AppHandle, s: State<'_, App>, id: i64, password: Option<String>) -> Result<String> {
    reauth(&app, &s, password, &reveal_prompt()).await?;
    Ok(stored_cookie(&s, id)?.expose().to_owned())
}

#[tauri::command]
pub async fn account_copy_cookie(app: AppHandle, s: State<'_, App>, id: i64, password: Option<String>) -> Result<()> {
    reauth(&app, &s, password, &reveal_prompt()).await?;
    copy_secret(&app, &s, stored_cookie(&s, id)?)
}

fn copy_secret(app: &AppHandle, s: &App, secret: Secret) -> Result<()> {
    let secs = s.db.settings()?.clipboard_clear_seconds.max(5);
    app.clipboard().write_secret(secret.expose().to_owned()).map_err(|e| AppError::Internal(e.to_string()))?;
    *s.copied.lock().unwrap_or_else(|e| e.into_inner()) = Some(secret);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(secs as u64)).await;
        clear_copied_cookie(&app);
    });
    Ok(())
}

pub fn clear_copied_cookie(app: &AppHandle) {
    let s = app.state::<App>();
    let Some(cookie) = s.copied.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    match app.clipboard().read_text() {
        Ok(t) if t == cookie.expose() => {
            if let Err(e) = app.clipboard().clear() {
                log::warn!("could not clear the copied cookie from the clipboard: {e}");
            }
        }
        Ok(_) => {}
        Err(e) => log::warn!("could not read the clipboard to clear the copied cookie: {e}"),
    }
}

const HEADSHOT_TTL_MS: i64 = 6 * 60 * 60 * 1000;

fn presence_of(p: UserPresence) -> Presence {
    Presence {
        kind: PresenceKind::from_code(p.user_presence_type),
        last_location: p.last_location,
        place_id: p.place_id,
        root_place_id: p.root_place_id,
        universe_id: p.universe_id,
        game_id: p.game_id,
    }
}

#[tauri::command]
pub async fn account_info(app: AppHandle, s: State<'_, App>, id: i64, refresh: bool) -> Result<AccountInfo> {
    let a = s.db.account(id)?;
    let key = format!("info:{}", a.user_id);
    let ttl = s.db.settings()?.cache_minutes as i64 * 60_000;
    if !refresh
        && let Some((json, _)) = s.db.cache_get(&key, ttl, now_ms())?
        && let Ok(mut info) = serde_json::from_str::<AccountInfo>(&json).inspect_err(|e| {
            log::debug!("cached info of account {id} unreadable: {:?} at {}:{}", e.classify(), e.line(), e.column())
        })
    {
        if now_ms() - info.fetched_at > 60_000
            && let Ok(rows) = s.api.presence(id, &stored_cookie(&s, id)?, &[a.user_id]).await
        {
            info.presence = rows.into_iter().find(|p| p.user_id == a.user_id).map(presence_of);
        }
        return Ok(info);
    }
    let cookie = stored_cookie(&s, id)?;
    let uid = a.user_id;
    let one = [uid];
    let (details, heads, robux, premium, plus, counts, presence, email, two_step) = tokio::join!(
        s.api.user_details(id, Some(&cookie), uid),
        s.api.headshots(&one),
        s.api.robux(id, &cookie),
        s.api.premium(id, &cookie, uid),
        s.api.plus(&one),
        s.api.social_counts(uid),
        s.api.presence(id, &cookie, &one),
        s.api.email_verified(id, &cookie),
        s.api.two_step_methods(id, &cookie, uid),
    );
    let mut unavailable = Vec::new();
    let mut part = |name: &str, ok: bool| {
        if !ok {
            unavailable.push(name.to_owned());
        }
    };
    part("profile", details.is_ok());
    part("avatar", heads.as_ref().is_ok_and(|h| h.contains_key(&uid)));
    part("robux", robux.is_ok());
    part("friends", counts.is_ok());
    part("presence", presence.as_ref().is_ok_and(|p| !p.is_empty()));
    part("email", email.is_ok());
    part("2-step", two_step.is_ok());
    let subscription = match (plus.as_ref().ok().and_then(|p| p.get(&uid).copied()), premium.as_ref().ok().copied()) {
        (Some(true), _) => Some(Subscription::Plus),
        (_, Some(true)) => Some(Subscription::Premium),
        (Some(false), _) => Some(Subscription::None),
        (None, _) => None,
    };
    part("subscription", subscription.is_some());

    let robux = match robux {
        Ok(r) => Some(r),
        Err(e) => {
            if record_cookie_error(&app, &s, id, &e)? || unavailable.len() == 8 {
                return Err(e);
            }
            None
        }
    };
    let details = details.ok();
    let counts = counts.ok();
    let info = AccountInfo {
        account_id: id,
        user_id: uid,
        username: details.as_ref().map_or(a.username.clone(), |d| d.name.clone()),
        display_name: details.as_ref().map_or(a.display_name.clone(), |d| d.display_name.clone()),
        description: details.as_ref().map(|d| d.description.clone()).unwrap_or_default(),
        created: details.as_ref().and_then(|d| d.created.clone()),
        is_banned: details.as_ref().is_some_and(|d| d.is_banned),
        verified_badge: details.as_ref().is_some_and(|d| d.has_verified_badge),
        headshot_url: heads.ok().and_then(|mut h| h.remove(&uid)),
        robux,
        subscription,
        friends: counts.map(|c| c.friends),
        followers: counts.map(|c| c.followers),
        following: counts.map(|c| c.following),
        email_verified: email.ok(),
        two_step_methods: two_step.ok(),
        presence: presence.ok().and_then(|p| p.into_iter().find(|p| p.user_id == uid)).map(presence_of),
        fetched_at: now_ms(),
        unavailable,
    };
    let json = serde_json::to_string(&info).map_err(|e| AppError::Internal(e.to_string()))?;
    s.db.cache_put(&key, &json, info.fetched_at)?;
    if let Some(url) = &info.headshot_url {
        s.db.cache_put(&format!("headshot:{uid}"), url, info.fetched_at)?;
    }
    Ok(info)
}

#[tauri::command]
pub async fn accounts_headshots(s: State<'_, App>, refresh: bool) -> Result<Vec<Headshot>> {
    let now = now_ms();
    let accounts = s.db.accounts()?;
    let mut out = Vec::new();
    let mut missing = Vec::new();
    for a in &accounts {
        match s.db.cache_get(&format!("headshot:{}", a.user_id), HEADSHOT_TTL_MS, now)? {
            Some((url, _)) if !refresh => out.push(Headshot { account_id: a.id, url }),
            _ => missing.push(a.user_id),
        }
    }
    if !missing.is_empty() {
        match s.api.headshots(&missing).await {
            Ok(urls) => {
                for a in accounts.iter().filter(|a| missing.contains(&a.user_id)) {
                    if let Some(url) = urls.get(&a.user_id) {
                        s.db.cache_put(&format!("headshot:{}", a.user_id), url, now)?;
                        out.push(Headshot { account_id: a.id, url: url.clone() });
                    }
                }
            }
            Err(e) => log::warn!("could not fetch avatars: {e}"),
        }
    }
    Ok(out)
}

#[tauri::command]
pub async fn accounts_presence(s: State<'_, App>, ids: Vec<i64>) -> Result<Vec<AccountPresence>> {
    let mut out = Vec::new();
    for a in s.db.accounts()?.into_iter().filter(|a| ids.contains(&a.id) && a.status == AccountStatus::Valid) {
        let cookie = match stored_cookie(&s, a.id) {
            Ok(c) => c,
            Err(AppError::Locked) => continue,
            Err(e) => {
                log::debug!("presence: cookie of account {} unusable: {e}", a.id);
                continue;
            }
        };
        match s.api.presence(a.id, &cookie, &[a.user_id]).await {
            Ok(rows) => out.extend(
                rows.into_iter()
                    .find(|p| p.user_id == a.user_id)
                    .map(|p| AccountPresence { account_id: a.id, presence: presence_of(p) }),
            ),
            Err(e) => log::info!("presence of account {} unavailable: {e}", a.id),
        }
    }
    Ok(out)
}

#[tauri::command]
pub async fn account_open_session(app: AppHandle, s: State<'_, App>, id: i64) -> Result<()> {
    let a = s.db.account(id)?;
    let cookie = stored_cookie(&s, id)?;
    let name = if a.alias.is_empty() { a.display_name.clone() } else { a.alias.clone() };
    let title = format!("{name} (@{}) — Roblox — LRAM", a.username);
    let (user_id, handle) = (a.user_id, app.clone());
    webview::open_session(&app, &s.data_dir, &title, cookie, move |last| {
        let Some(last) = last else { return };
        tauri::async_runtime::block_on(async move {
            let s = handle.state::<App>();
            match s.db.account(id) {
                Ok(a) if a.user_id == user_id => {}
                Ok(_) | Err(AppError::NotFound) => return,
                Err(e) => {
                    log::debug!("session window for account {id}: could not read the account: {e}");
                    return;
                }
            }
            match s.api.authenticated_user(0, &last).await {
                Ok((u, rotated)) if u.id == user_id => {
                    s.store_rotated(id, rotated.unwrap_or(last));
                    if let Err(e) = s.db.set_status(id, AccountStatus::Valid, now_ms(), Some(&u.name), Some(&u.display_name)) {
                        log::warn!("could not mark account {id} valid after its session window: {e}");
                    }
                }
                Ok(_) => log::warn!("session window for account {id} ended signed in as another user; cookie not changed"),
                Err(e) => log::warn!("session window for account {id} ended with an unusable cookie ({e}); cookie not changed"),
            }
            accounts_changed(&handle);
        });
    })
    .await
}

#[tauri::command]
pub fn target_parse(input: String) -> ParsedInput {
    crate::launcher::target::parse(&input)
}

#[tauri::command]
pub async fn user_lookup(s: State<'_, App>, name: String) -> Result<UserRef> {
    let name = name.trim().trim_start_matches('@').to_owned();
    if name.is_empty() {
        return Err(AppError::Invalid(tr!("Type a Roblox username.")));
    }
    let found = s.api.users_by_name(std::slice::from_ref(&name)).await?;
    let u = found.into_iter().next().ok_or_else(|| AppError::Invalid(tr!("No Roblox user is called \"{name}\".", name = name)))?;
    Ok(UserRef { user_id: u.id, username: u.name, display_name: u.display_name })
}

#[tauri::command]
pub async fn launch(
    app: AppHandle,
    s: State<'_, App>,
    account_id: i64,
    target: LaunchTarget,
    favourite_id: Option<i64>,
    fps_cap: Option<u32>,
) -> Result<LaunchInfo> {
    let fps_cap = launch_fps(fps_cap)?;
    let _single = s.bulk.begin_single_launch()?;
    Ok(launch_tracked(&app, &s, account_id, target, favourite_id, None, fps_cap, true).await?.0)
}

pub(crate) fn installing() -> String {
    tr!("Roblox is being installed. Launch again once it is done.")
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn launch_tracked(
    app: &AppHandle,
    s: &App,
    account_id: i64,
    target: LaunchTarget,
    favourite_id: Option<i64>,
    delay_ms: Option<u32>,
    fps_cap: Option<u32>,
    hold: bool,
) -> Result<(LaunchInfo, Instance)> {
    launch_tracked_guarded(app, s, account_id, target, favourite_id, delay_ms, fps_cap, hold, None).await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn launch_tracked_guarded(
    app: &AppHandle, s: &App, account_id: i64, target: LaunchTarget,
    favourite_id: Option<i64>, delay_ms: Option<u32>, fps_cap: Option<u32>, hold: bool,
    expected_kills: Option<u64>,
) -> Result<(LaunchInfo, Instance)> {
    let kills = s.bulk.kills();
    let killed = || AppError::Invalid(tr!("Launch cancelled by Stop all."));
    if expected_kills.is_some_and(|expected| expected != kills) { return Err(killed()); }
    let Some(_claim) = s.tracker.claim(account_id) else {
        let name = s.db.account(account_id).map(|a| crate::launcher::bulk::name_of(&a)).ok().filter(|n| !n.is_empty());
        let name = name.unwrap_or_else(|| tr!("This account"));
        return Err(AppError::Wait(tr!("{name} is already running. Close its client first.", name = name)));
    };
    if let Some(why) = crate::network::blocked(s) {
        return Err(AppError::Wait(why));
    }
    let _busy = s.bulk.begin_launch();
    if s.update.installing() {
        return Err(AppError::Wait(installing()));
    }
    if hold && let Some(why) = crate::launcher::update::update_hold(s, 0).await {
        return Err(AppError::Wait(why));
    }
    let v = s.db.settings()?;
    let gap = s.bulk.launch_gap(delay_ms.unwrap_or(v.launch_delay_ms), Instant::now());
    let (wait, slot) = s.bulk.cooldown(gap);
    if !wait.is_zero() {
        log::debug!("launch of account {account_id} waits {wait:?} (launch gap {gap:?})");
    }
    tokio::time::sleep(wait).await;
    if s.bulk.kills() != kills {
        s.bulk.release(slot);
        return Err(killed());
    }
    retake_multi(s, &v);
    let launching = s.multi.launching(v.multi_instance && v.protect_shared_login);
    let info = match crate::launcher::launch(s, account_id, &target, fps_cap).await {
        Ok(info) => info,
        Err(e) => {
            log::warn!("launch failed for account {account_id}: {e}");
            s.bulk.release(slot);
            if e.is_forbidden() {
                s.bulk.refused();
            }
            record_cookie_error(app, s, account_id, &e)?;
            return Err(e);
        }
    };
    let mut instance = s.tracker.register(&info, target);
    if let Some(p) = s.db.client_performance(account_id)? {
        s.tracker.set_performance(&instance.id, p, false);
        instance = s.tracker.get(&instance.id).ok_or(AppError::NotFound)?;
    }
    if v.mute_clients {
        s.tracker.set_muted(&instance.id, true);
        crate::instances::audio::wake();
    }
    drop(launching);
    instances_changed(app);
    if s.bulk.kills() != kills {
        if let Some((pid, started)) = s.tracker.mark_closing(&instance.id) {
            match tauri::async_runtime::spawn_blocking(move || {
                crate::instances::process::close(pid, started, true, Duration::ZERO)
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => log::warn!("could not close client {pid} started during kill all: {e}"),
                Err(e) => log::warn!("close of client {pid} started during kill all failed: {e}"),
            }
        }
        return Err(killed());
    }
    if let Some(f) = favourite_id
        && let Err(e) = s.db.touch_favourite(f, now_ms())
    {
        log::debug!("could not touch favourite {f}: {e}");
    }
    Ok((info, instance))
}

pub fn notice(app: &AppHandle, error: bool, message: impl Into<String>) {
    let message = message.into();
    if error {
        log::warn!("notice: {message}");
    } else {
        log::info!("notice: {message}");
    }
    let _ = app.emit("app://notice", Notice { error, message });
}

#[tauri::command]
pub fn roblox_client(app: AppHandle) -> RobloxClient {
    crate::launcher::update::publish(&app)
}

#[tauri::command]
pub async fn roblox_install(app: AppHandle) -> Result<RobloxClient> {
    crate::launcher::update::install(&app).await
}

#[tauri::command]
pub fn self_update_status(s: State<'_, App>) -> AppUpdate {
    crate::self_update::status(&s)
}

#[tauri::command]
pub async fn self_update_check(app: AppHandle) -> Result<AppUpdate> {
    crate::self_update::check(&app).await
}

#[tauri::command]
pub async fn self_update_install(app: AppHandle) -> Result<()> {
    crate::self_update::install(&app).await
}

#[tauri::command]
pub fn data_dir_info(s: State<'_, App>) -> DataDir {
    DataDir {
        path: s.data_dir.display().to_string(),
        default: crate::same_dir(&s.data_dir, &crate::default_dir()),
        overridden: crate::data_dir_overridden(),
    }
}

#[tauri::command]
pub fn data_dir_open(app: AppHandle, s: State<'_, App>) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_path(s.data_dir.display().to_string(), None::<&str>).map_err(|e| AppError::Internal(e.to_string()))
}

#[tauri::command]
pub fn logs_open(app: AppHandle, s: State<'_, App>) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    let dir = s.data_dir.join("logs");
    std::fs::create_dir_all(&dir)?;
    app.opener().open_path(dir.display().to_string(), None::<&str>).map_err(|e| AppError::Internal(e.to_string()))
}

#[tauri::command]
pub async fn debug_tail(s: State<'_, App>, lines: u32) -> Result<Vec<String>> {
    let path = s.data_dir.join("logs").join("lram.log");
    blocking(move || tail(&path, lines.min(10_000) as usize)).await?
}

fn tail(path: &Path, n: usize) -> Result<Vec<String>> {
    use std::io::{Read, Seek, SeekFrom};
    const WINDOW: u64 = 4 << 20;
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let len = f.metadata()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(WINDOW)))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    let all: Vec<&str> = text.lines().collect();
    let skip = usize::from(len > WINDOW && !all.is_empty());
    let all = &all[skip..];
    Ok(all[all.len().saturating_sub(n)..].iter().map(|l| l.to_string()).collect())
}

#[tauri::command]
pub fn debug_log(level: String, message: String) {
    let level = match level.as_str() {
        "error" => log::Level::Error,
        "warn" => log::Level::Warn,
        "info" => log::Level::Info,
        _ => log::Level::Debug,
    };
    log::log!(target: "ui", level, "{}", ui_log_text(message));
}

fn ui_log_text(message: String) -> String {
    let mut message = message.replace(['\r', '\n'], " ");
    if message.len() > 4096 {
        let mut end = 4096;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
    }
    message
}

#[tauri::command]
pub async fn logs_clean(s: State<'_, App>) -> Result<LogsCleaned> {
    let Some(dir) = crate::instances::logwatch::logs_dir() else {
        log::debug!("logs clean: no Roblox logs folder");
        return Ok(LogsCleaned::default());
    };
    let keep = s.tracker.live_logs();
    blocking(move || crate::instances::logwatch::clean(&dir, &keep)).await
}

const BACKGROUND_KEY: &str = "background";
const BACKGROUND_MAX: u64 = 8 * 1024 * 1024;

fn is_image(b: &[u8]) -> bool {
    b.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
        || b.starts_with(&[0xff, 0xd8, 0xff])
        || b.starts_with(b"GIF8")
        || (b.len() >= 12 && b.starts_with(b"RIFF") && &b[8..12] == b"WEBP")
}

#[tauri::command]
pub async fn background_set(s: State<'_, App>, path: String) -> Result<()> {
    let bytes = blocking(move || -> Result<Vec<u8>> {
        if std::fs::metadata(&path)?.len() > BACKGROUND_MAX {
            return Err(AppError::Invalid(tr!("Pick an image of 8 MB or less.")));
        }
        Ok(std::fs::read(&path)?)
    })
    .await??;
    if !is_image(&bytes) {
        return Err(AppError::Invalid(tr!("Pick a PNG, JPEG, WebP or GIF image.")));
    }
    s.db.set_setting_value(BACKGROUND_KEY, Some(&crate::discord::hex(&bytes)))
}

#[tauri::command]
pub fn background_clear(s: State<'_, App>) -> Result<()> {
    s.db.set_setting_value(BACKGROUND_KEY, None)
}

#[tauri::command]
pub async fn background_get(s: State<'_, App>) -> Result<tauri::ipc::Response> {
    let bytes = s
        .db
        .setting_value(BACKGROUND_KEY)?
        .and_then(|h| {
            let b = crate::discord::unhex(&h);
            if b.is_none() {
                log::warn!("stored background isn't valid hex; showing none");
            }
            b
        })
        .unwrap_or_default();
    Ok(tauri::ipc::Response::new(bytes))
}

fn shutdown_secs(at_ms: Option<i64>, in_secs: Option<u32>, now_ms: i64) -> Result<u32> {
    let secs = match (in_secs, at_ms) {
        (Some(s), _) => i64::from(s),
        (None, Some(at)) => (at - now_ms + 999).div_euclid(1000),
        (None, None) => return Err(AppError::Invalid(tr!("Pick when to shut down."))),
    };
    if !(60..=7 * 24 * 3600).contains(&secs) {
        return Err(AppError::Invalid(tr!("The shutdown must be 1 minute to 7 days from now.")));
    }
    Ok(secs as u32)
}

fn shutdown_exe(args: &[&str]) -> Result<(i32, String)> {
    use std::os::windows::process::CommandExt;
    let root = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| r"C:\Windows".into());
    let out = std::process::Command::new(root.join(r"System32\shutdown.exe")).args(args).creation_flags(0x0800_0000).output()?;
    let text = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    let code = out.status.code().unwrap_or(-1);
    if code != 0 {
        log::warn!("shutdown.exe {} exited {code}: {}", args.join(" "), text.chars().take(300).collect::<String>());
    }
    Ok((code, text))
}

const SHUTDOWN_KEY: &str = "shutdown_at";

pub fn pending_shutdown(db: &crate::storage::Db) -> Option<i64> {
    let v = db.setting_value(SHUTDOWN_KEY).inspect_err(|e| log::warn!("could not read the scheduled shutdown: {e}")).ok().flatten()?;
    v.parse::<i64>().inspect_err(|e| log::warn!("scheduled shutdown deadline {v:?} unreadable: {e}")).ok().filter(|&at| at > now_ms())
}

#[tauri::command]
pub fn shutdown_schedule(app: AppHandle, s: State<'_, App>, at_ms: Option<i64>, in_secs: Option<u32>, close_clients: bool) -> Result<Option<i64>> {
    let secs = shutdown_secs(at_ms, in_secs, now_ms())?;
    let mut at = s.shutdown.lock().unwrap_or_else(|e| e.into_inner());
    if at.take().is_some() {
        shutdown_exe(&["/a"])?;
    }
    let (code, text) = shutdown_exe(&["/s", "/t", &secs.to_string()])?;
    if code != 0 {
        return Err(AppError::Invalid(tr!("Windows wouldn't schedule the shutdown: {text}", text = text)));
    }
    let deadline = now_ms() + i64::from(secs) * 1000;
    *at = Some(deadline);
    if let Err(e) = s.db.set_setting_value(SHUTDOWN_KEY, Some(&deadline.to_string())) {
        log::warn!("the shutdown's deadline wasn't stored: {e}");
    }
    log::info!("shutdown scheduled in {secs} s");
    if close_clients {
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(u64::from(secs.saturating_sub(30)))).await;
            let s = app.state::<App>();
            if *s.shutdown.lock().unwrap_or_else(|e| e.into_inner()) == Some(deadline)
                && let Err(e) = close_running(&s, false).await
            {
                log::warn!("could not close the clients before the shutdown: {e}");
            }
        });
    }
    Ok(Some(deadline))
}

#[tauri::command]
pub fn shutdown_cancel(s: State<'_, App>) -> Result<Option<i64>> {
    let mut at = s.shutdown.lock().unwrap_or_else(|e| e.into_inner());
    if at.is_some() {
        let (code, text) = shutdown_exe(&["/a"])?;
        if code != 0 && code != 1116 {
            return Err(AppError::Invalid(tr!("Windows wouldn't call off the shutdown: {text}", text = text)));
        }
        log::info!("shutdown called off");
    }
    *at = None;
    s.db.set_setting_value(SHUTDOWN_KEY, None)?;
    Ok(None)
}

#[tauri::command]
pub fn shutdown_status(s: State<'_, App>) -> Option<i64> {
    *s.shutdown.lock().unwrap_or_else(|e| e.into_inner())
}

#[tauri::command]
pub async fn data_dir_move(app: AppHandle, s: State<'_, App>, path: String) -> Result<()> {
    if crate::data_dir_overridden() {
        return Err(AppError::Invalid(tr!("A test run's data folder (LRAM_DATA_DIR) can't be moved.")));
    }
    if s.bulk.any_busy() {
        return Err(AppError::Invalid(tr!("Wait for the running launch, batch or auto-rejoin to finish, then move the data folder.")));
    }
    let target = crate::move_target(&s.data_dir, &path)?;
    let empty = std::fs::read_dir(&target).map_or(true, |mut d| d.next().is_none());
    std::fs::create_dir_all(&target)?;
    if empty {
        crate::restrict_to_user(&target);
    }
    s.tracker.persist(&s.db, true);
    let location = crate::Location { data_dir: target.clone(), moved_from: Some(s.data_dir.clone()) };
    let failed = s.db.copy_to(&target.join(crate::DB_FILE), || crate::write_location(&location).map_err(AppError::from));
    if failed.is_ok() {
        log::info!("database copied to {}; restarting to use it", target.display());
        app.request_restart();
        return Ok(());
    }
    for f in crate::DB_FILES {
        if let Err(e) = std::fs::remove_file(target.join(f))
            && e.kind() != std::io::ErrorKind::NotFound
        {
            log::debug!("could not delete the half-written {f}: {e}");
        }
    }
    failed
}

#[tauri::command]
pub fn bulk_launch(app: AppHandle, request: BulkRequest) -> Result<String> {
    crate::launcher::bulk::start(app, request)
}

#[tauri::command]
pub fn bulk_cancel(s: State<'_, App>, batch_id: String) -> Result<()> {
    s.bulk.cancel(&batch_id)
}

#[tauri::command]
pub fn bulk_current(s: State<'_, App>) -> Option<BulkProgress> {
    s.bulk.current()
}

#[tauri::command]
pub async fn game_info(s: State<'_, App>, place_id: i64) -> Result<GameInfo> {
    let key = format!("game:{place_id}");
    let ttl = s.db.settings()?.cache_minutes as i64 * 60_000;
    if let Some((json, _)) = s.db.cache_get(&key, ttl, now_ms())?
        && let Ok(info) = serde_json::from_str::<GameInfo>(&json).inspect_err(|e| log::debug!("cached game {place_id} unreadable: {e}"))
    {
        return Ok(info);
    }
    let (universe, icons) = tokio::join!(s.api.place_universe(place_id), s.api.place_icons(std::slice::from_ref(&place_id)));
    let universe = universe?.ok_or(AppError::NotFound)?;
    let game = s.api.games(&[universe]).await?.into_iter().find(|g| g.universe_id == universe).ok_or(AppError::NotFound)?;
    let info = game_info_of(place_id, game, icons.ok().and_then(|mut i| i.remove(&place_id)));
    if info.icon_url.is_some() {
        let json = serde_json::to_string(&info).map_err(|e| AppError::Internal(e.to_string()))?;
        s.db.cache_put(&key, &json, now_ms())?;
    }
    Ok(info)
}

fn game_info_of(place_id: i64, g: GameDetails, icon_url: Option<String>) -> GameInfo {
    GameInfo {
        place_id,
        universe_id: g.universe_id,
        root_place_id: g.root_place_id,
        name: g.name,
        creator: g.creator,
        playing: g.playing,
        visits: g.visits,
        max_players: g.max_players,
        icon_url,
        private_servers: g.private_servers,
    }
}

#[tauri::command]
pub async fn target_place(app: AppHandle, s: State<'_, App>, target: LaunchTarget, account_id: Option<i64>) -> Result<i64> {
    let no_game = || AppError::Invalid(tr!("That link doesn't lead to a game."));
    match &target {
        LaunchTarget::Place { place_id, .. }
        | LaunchTarget::Server { place_id, .. }
        | LaunchTarget::PrivateLink { place_id, .. }
        | LaunchTarget::PrivateAccess { place_id, .. } => Ok(*place_id),
        LaunchTarget::Universe { universe_id } => crate::launcher::root_place(&s.api, *universe_id).await,
        LaunchTarget::ShareLink { .. } | LaunchTarget::ShareLinkV2 { .. } => {
            let id = account_id.ok_or_else(|| AppError::Invalid(tr!("Pick an account first: share links open as an account.")))?;
            let cookie = stored_cookie(&s, id)?;
            match crate::launcher::resolve(&s.api, id, &cookie, &target).await {
                Ok((join, _)) => join.place_id().ok_or_else(no_game),
                Err(e) => {
                    record_cookie_error(&app, &s, id, &e)?;
                    Err(e)
                }
            }
        }
        LaunchTarget::FollowUser { .. } => Err(no_game()),
    }
}

#[tauri::command]
pub async fn private_servers_list(
    app: AppHandle,
    s: State<'_, App>,
    place_id: i64,
    account_id: i64,
    cursor: Option<String>,
) -> Result<PrivateServerPage> {
    let cookie = stored_cookie(&s, account_id)?;
    let page = s.api.private_servers(account_id, &cookie, place_id, cursor.as_deref()).await;
    if let Err(e) = &page {
        record_cookie_error(&app, &s, account_id, e)?;
    }
    page
}

#[tauri::command]
pub async fn servers_list(
    s: State<'_, App>,
    place_id: i64,
    order: ServerOrder,
    exclude_full: bool,
    limit: u8,
    cursor: Option<String>,
    account_id: Option<i64>,
) -> Result<ServerPage> {
    let cookie = account_id.map(|id| stored_cookie(&s, id)).transpose()?;
    let id = account_id.unwrap_or(0);
    s.api.servers_public(id, cookie.as_ref(), place_id, order, exclude_full, limit, cursor.as_deref()).await
}

pub fn instances_changed(app: &AppHandle) {
    let _ = app.emit("instances://changed", ());
    crate::tray::refresh(app);
}

#[tauri::command]
pub fn instances_list(s: State<'_, App>) -> Vec<Instance> {
    s.tracker.list()
}

#[tauri::command]
pub async fn instance_close(s: State<'_, App>, id: String, force: bool) -> Result<()> {
    let instance = s.tracker.get(&id).ok_or(AppError::NotFound)?;
    if instance.state != InstanceState::Exited && crate::instances::process::start_time(instance.pid).is_none()
        && !crate::instances::process::unreadable_client(instance.pid)
    {
        log::warn!("instance {id}: close requested but tracked PID {} is gone; waiting for identity-safe recovery", instance.pid);
        return Err(AppError::Internal("The tracked Roblox process has changed or exited. LRAM is checking the replacement process; no unrelated client was closed.".into()));
    }
    s.tracker.end_relaunch(&id);
    let Some(target) = s.tracker.mark_closing(&id) else { return Ok(()) };
    let r = close_all(vec![target], force, grace(&s)).await?.pop().unwrap_or(Ok(()));
    if r.is_err() {
        s.tracker.clear_closing(&id);
    }
    r
}

fn grace(s: &App) -> Duration {
    Duration::from_secs(u64::from(
        s.db.settings().inspect_err(|e| log::debug!("could not read settings, close grace 5 s: {e}")).map_or(5, |v| v.close_grace_s),
    ))
}

async fn close_all(targets: Vec<(u32, Option<u64>)>, force: bool, grace: Duration) -> Result<Vec<Result<()>>> {
    tauri::async_runtime::spawn_blocking(move || {
        std::thread::scope(|sc| {
            let jobs: Vec<_> = targets
                .into_iter()
                .map(|(pid, t0)| sc.spawn(move || crate::instances::process::close(pid, t0, force, grace)))
                .collect();
            jobs.into_iter()
                .map(|j| j.join().unwrap_or_else(|_| Err(AppError::Internal("close panicked".into()))))
                .collect()
        })
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))
}

#[tauri::command]
pub async fn instances_close_all(s: State<'_, App>, force: bool) -> Result<()> {
    close_running(&s, force).await
}

pub(crate) async fn close_running(s: &App, force: bool) -> Result<()> {
    s.tracker.cancel_relaunches();
    let running: Vec<Instance> = s.tracker.list().into_iter().filter(|i| i.state != InstanceState::Exited).collect();
    let (ids, targets): (Vec<String>, Vec<_>) =
        running.into_iter().filter_map(|i| Some((i.id.clone(), s.tracker.mark_closing(&i.id)?))).unzip();
    for (id, r) in ids.iter().zip(close_all(targets, force, grace(s)).await?) {
        if let Err(e) = r {
            log::warn!("could not close Roblox client of instance {id}: {e}");
            s.tracker.clear_closing(id);
        }
    }
    Ok(())
}

pub fn cores() -> u32 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as u32)
}

pub(crate) fn launch_fps(f: Option<u32>) -> Result<Option<u32>> {
    let f = f.filter(|&f| f != 0);
    if let Some(f) = f {
        check_fps(f)?;
    }
    Ok(f)
}

fn check_fps(f: u32) -> Result<()> {
    if f != 0 && !crate::launcher::client_settings::RANGE.contains(&f) {
        return Err(AppError::Invalid(tr!("The FPS cap must be 0 (off) or 5 to 360.")));
    }
    Ok(())
}

fn check_limits(l: &Limits) -> Result<()> {
    if l.cpu_cap.is_some_and(|c| !(1..=100).contains(&c)) {
        return Err(AppError::Invalid(tr!("The CPU limit must be between 1 and 100 %.")));
    }
    if l.trim_every_min.is_some_and(|m| m > 24 * 60) {
        return Err(AppError::Invalid(tr!("Trimming can be at most once a day apart.")));
    }
    if l.affinity == Some(0) {
        return Err(AppError::Invalid(tr!("Pick at least one CPU core.")));
    }
    Ok(())
}

#[tauri::command]
pub async fn instance_set_limits(app: AppHandle, s: State<'_, App>, id: String, limits: Limits) -> Result<()> {
    check_limits(&limits)?;
    let (prev, client) = s.tracker.set_limits(&id, limits).ok_or(AppError::NotFound)?;
    instances_changed(&app);
    if client.is_none() {
        return Ok(());
    }
    let app2 = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || {
        let s = app2.state::<App>();
        let v = s.db.settings().unwrap_or_else(|e| {
            log::warn!("could not read settings for the limits of {id}; using defaults: {e}");
            Default::default()
        });
        let held = s.tracker.throttle(v.mem_throttle.mb).0;
        let plan = crate::instances::limits::plan_now(&s, &v, &s.tracker.clients(), &held);
        match plan.into_iter().find(|c| c.0 == id) {
            Some((_, pid, t0, l)) => s.limiter.apply(&id, pid, t0, l).map_err(|e| (id, e)),
            None => Ok(()),
        }
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;
    if let Err((id, e)) = r {
        s.tracker.set_limits(&id, prev);
        instances_changed(&app);
        return Err(e);
    }
    Ok(())
}

#[tauri::command]
pub async fn instance_performance(app: AppHandle, s: State<'_, App>, id: String, performance: crate::model::ClientPerformance, all_running: bool) -> Result<()> {
    unlocked(&s)?;
    launch_fps(Some(performance.fps_cap))?;
    check_limits(&performance.limits)?;
    if performance.strict_cpu && !performance.limits.cpu_cap.is_some_and(|c| (1..=2).contains(&c)) {
        return Err(AppError::Invalid("Strict ULTRA requires a 1% or 2% CPU ceiling.".into()));
    }
    let selected = s.tracker.get(&id).ok_or(AppError::NotFound)?;
    let snapshot: std::collections::HashMap<_, _> = s.tracker.clients().into_iter().map(|(id, pid, t0, _)| (id, (pid, t0))).collect();
    if selected.state == InstanceState::Exited || selected.checking_process || !snapshot.contains_key(&id) {
        return Err(AppError::Wait("Wait for a verified running process before applying a profile.".into()));
    }
    let targets = if all_running { s.tracker.list().into_iter().filter(|i| !i.checking_process && snapshot.contains_key(&i.id)).collect() } else { vec![selected] };
    let ids: std::collections::HashSet<_> = targets.iter().map(|i| i.id.clone()).collect();
    let apply = || -> Result<()> {
    for i in targets {
        // Resolve snapshot IDs again: never apply to a replacement/newly launched instance.
        if !s.tracker.clients().iter().any(|(id, pid, t0, _)| *id == i.id && snapshot.get(id) == Some(&(*pid, *t0))) { continue; }
        s.db.save_client_performance(i.account_id, &performance)?;
        s.tracker.set_performance(&i.id, performance.clone(), true);
    }
    let v = s.db.settings()?;
    let held = s.tracker.throttle(v.mem_throttle.mb).0;
    for (id, pid, t0, limits) in crate::instances::limits::plan_now(&s, &v, &s.tracker.clients(), &held) {
        if ids.contains(&id) && snapshot.get(&id) == Some(&(pid, t0)) { s.limiter.apply(&id, pid, t0, limits)?; }
    }
    Ok(())
    };
    let result = apply();
    // Always expose truthful applied/pending state, including partial OS/DB failures.
    s.tracker.set_in_force(&s.limiter.in_force());
    instances_changed(&app);
    result.map_err(|e| AppError::Invalid(format!("Some profile changes may already be saved/applied. Check each client's applied limits before retrying. {e}")))
}

#[tauri::command]
pub async fn instance_restart_performance(app: AppHandle, s: State<'_, App>, id: String) -> Result<()> {
    unlocked(&s)?;
    let i = s.tracker.get(&id).ok_or(AppError::NotFound)?;
    if i.checking_process || i.state == InstanceState::Exited {
        return Err(AppError::Wait("This client cannot be restarted while its process is being checked or has ended.".into()));
    }
    let kills = s.bulk.kills();
    let Some(target) = s.tracker.mark_closing(&id) else { return Err(AppError::NotFound) };
    s.tracker.end_relaunch(&id);
    let closed = match close_all(vec![target], false, grace(&s)).await {
        Ok(mut results) => results.pop().unwrap_or_else(|| Err(AppError::Internal("Close returned no result.".into()))),
        Err(error) => Err(error),
    };
    if let Err(error) = closed {
        s.tracker.clear_closing(&id);
        return Err(error);
    }
    for _ in 0..40 {
        if s.bulk.kills() != kills { return Err(AppError::Wait("Restart cancelled by Stop all.".into())); }
        if s.tracker.get(&id).is_none_or(|i| i.state == InstanceState::Exited) {
            launch_tracked_guarded(&app, &s, i.account_id, i.target, None, None, None, true, Some(kills)).await?;
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(AppError::Wait("The old client has not finished closing; no duplicate was launched.".into()))
}

#[tauri::command]
pub fn instance_set_anti_afk(app: AppHandle, s: State<'_, App>, id: String, enabled: bool) -> Result<()> {
    if !s.tracker.set_anti_afk(&id, enabled) {
        return Err(AppError::NotFound);
    }
    instances_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn instance_set_muted(app: AppHandle, s: State<'_, App>, id: String, muted: bool) -> Result<()> {
    if !s.tracker.set_muted(&id, muted) {
        return Err(AppError::NotFound);
    }
    crate::instances::audio::wake();
    instances_changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn instance_trim(app: AppHandle, s: State<'_, App>, id: String) -> Result<(u64, u64)> {
    s.tracker.get(&id).ok_or(AppError::NotFound)?;
    let (_, pid, t0, _) = s
        .tracker
        .clients()
        .into_iter()
        .find(|c| c.0 == id)
        .ok_or_else(|| AppError::Invalid(tr!("That client hasn't started yet, or has closed.")))?;
    tauri::async_runtime::spawn_blocking(move || app.state::<App>().limiter.logged_trim("by hand", pid, t0))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
}

fn client_pid(s: &App, id: &str) -> Result<Option<u32>> {
    s.tracker.get(id).ok_or(AppError::NotFound)?;
    Ok(s.tracker.clients().into_iter().find(|c| c.0 == id).map(|c| c.1))
}

fn not_started() -> AppError {
    AppError::Invalid(tr!("That client hasn't started yet, or has closed."))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| AppError::Internal(e.to_string()))
}

#[tauri::command]
pub async fn instance_thumbnail(s: State<'_, App>, id: String, max_w: u32, known: u32) -> Result<tauri::ipc::Response> {
    let pid = client_pid(&s, &id)?;
    let preview = blocking(move || crate::instances::window::preview(pid, max_w.clamp(16, 1280))).await?;
    Ok(tauri::ipc::Response::new(preview.to_bytes(known)))
}

#[tauri::command]
pub async fn instance_show(s: State<'_, App>, id: String, what: crate::instances::window::Show) -> Result<()> {
    let pid = client_pid(&s, &id)?.ok_or_else(not_started)?;
    blocking(move || crate::instances::window::show(pid, what)).await?
}

#[tauri::command]
pub async fn instances_show_all(s: State<'_, App>, what: crate::instances::window::Show) -> Result<()> {
    if what == crate::instances::window::Show::Focus {
        return Err(AppError::Invalid(tr!("Only one client can be in front.")));
    }
    let pids: Vec<u32> = s.tracker.clients().into_iter().map(|c| c.1).collect();
    blocking(move || crate::instances::window::show_all(&pids, what)).await
}

#[tauri::command]
pub async fn instance_micro(app: AppHandle, s: State<'_, App>, id: String, on: bool, size: Option<(u32, u32)>) -> Result<(u32, u32)> {
    let pid = client_pid(&s, &id)?.ok_or_else(not_started)?;
    let id2 = id.clone();
    let v = s.db.settings()?;
    let wanted = size.unwrap_or((v.micro_width, v.micro_height));
    check_window_size(wanted)?;
    let size = blocking(move || crate::instances::window::micro(&id2, pid, on, wanted)).await??;
    s.tracker.set_micro(&id, on);
    instances_changed(&app);
    Ok(size)
}

fn check_window_size((w, h): (u32, u32)) -> Result<()> {
    if !(16..=7680).contains(&w) || !(16..=4320).contains(&h) {
        return Err(AppError::Invalid(tr!("A window can be 16 to 7680 pixels wide and 16 to 4320 high.")));
    }
    Ok(())
}

#[tauri::command]
pub async fn instances_tile(app: AppHandle, s: State<'_, App>, micro: bool, columns: Option<u32>) -> Result<usize> {
    if columns.is_some_and(|c| !(1..=20).contains(&c)) {
        return Err(AppError::Invalid(tr!("Grid columns must be between 1 and 20.")));
    }
    let clients: Vec<(String, u32)> = s.tracker.clients().into_iter().map(|c| (c.0, c.1)).collect();
    let near = app.get_webview_window("main").and_then(|w| w.hwnd().ok()).map_or(0, |h| h.0 as isize);
    let v = s.db.settings()?;
    let size = micro.then_some((v.micro_width, v.micro_height));
    let tiled = blocking(move || {
        crate::instances::window::tile_columns(&clients, windows::Win32::Foundation::HWND(near as *mut _), size, columns)
    })
    .await?;
    for id in &tiled {
        s.tracker.set_micro(id, micro);
    }
    instances_changed(&app);
    Ok(tiled.len())
}

#[tauri::command]
pub async fn instances_metrics(app: AppHandle) -> Result<Vec<ClientMetrics>> {
    tauri::async_runtime::spawn_blocking(move || {
        let s = app.state::<App>();
        s.metrics.sample(&s.tracker.clients())
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))
}

const QOS_KEY: &str = "qos:applied";

pub(crate) fn bandwidth(s: &App) -> Result<BandwidthStatus> {
    let kbps = s.db.settings()?.bandwidth_kbps;
    let now = now_ms();
    let boot = crate::instances::qos::boot_ms(now);
    let applied = s.db.cache_get(QOS_KEY, i64::MAX / 2, now)?.is_some_and(|(_, at)| crate::instances::qos::applied_this_boot(at, boot));
    Ok(BandwidthStatus { kbps, active: kbps > 0 && applied })
}

#[tauri::command]
pub fn bandwidth_status(s: State<'_, App>) -> Result<BandwidthStatus> {
    bandwidth(&s)
}

#[tauri::command]
pub async fn bandwidth_apply(s: State<'_, App>, kbps: u32) -> Result<BandwidthStatus> {
    crate::instances::qos::script(kbps)?;
    tauri::async_runtime::spawn_blocking(move || crate::instances::qos::apply(kbps))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))??;
    let mut v = s.db.settings()?;
    v.bandwidth_kbps = kbps;
    s.db.save_settings(&v)?;
    s.db.cache_put(QOS_KEY, "", crate::instances::qos::boot_ms(now_ms()))?;
    bandwidth(&s)
}

#[tauri::command]
pub async fn system_usage(app: AppHandle) -> Result<Option<crate::model::SystemUsage>> {
    tauri::async_runtime::spawn_blocking(move || app.state::<App>().metrics.system())
        .await
        .map_err(|e| AppError::Internal(e.to_string()))
}

#[tauri::command]
pub fn instance_stop_relaunch(app: AppHandle, s: State<'_, App>, id: String) {
    s.tracker.end_relaunch(&id);
    crate::instances::rejoin::retry_now();
    instances_changed(&app);
}

#[tauri::command]
pub fn instances_retry_relaunch() {
    crate::instances::rejoin::retry_now();
}

#[tauri::command]
pub fn instances_clear_ended(app: AppHandle, s: State<'_, App>) {
    s.tracker.clear_ended();
    instances_changed(&app);
}

#[tauri::command]
pub fn multi_instance_status(s: State<'_, App>) -> Result<MultiInstanceStatus> {
    let v = s.db.settings()?;
    retake_multi(&s, &v);
    Ok(s.multi.status(v.multi_instance, v.protect_shared_login))
}

pub(crate) fn retake_multi(s: &App, v: &Settings) {
    if v.multi_instance && !s.multi.is_active() && s.multi.enable().is_ok() {
        protect_running(s, v);
    }
}

fn protect_running(s: &App, v: &Settings) {
    if v.protect_shared_login && (s.multi.in_flight() || s.tracker.any_running() || s.multi.has_survivors()) {
        s.multi.lock_shared();
    }
}

#[tauri::command]
pub fn multi_instance_set(s: State<'_, App>, enabled: bool) -> Result<MultiInstanceStatus> {
    let mut v = s.db.settings()?;
    v.multi_instance = enabled;
    s.db.save_settings(&v)?;
    if enabled {
        match s.multi.enable() {
            Ok(()) => protect_running(&s, &v),
            Err(e) => log::info!("multi-instance not active yet: {e}"),
        }
    } else {
        s.multi.disable();
    }
    Ok(s.multi.status(v.multi_instance, v.protect_shared_login))
}

#[tauri::command]
pub async fn multi_instance_take_over(app: AppHandle) -> Result<MultiInstanceStatus> {
    tauri::async_runtime::spawn_blocking(move || {
        let s = app.state::<App>();
        let v = s.db.settings()?;
        if !v.multi_instance {
            return Err(AppError::Invalid(tr!("Turn on multi-instance first.")));
        }
        s.multi.take_over()?;
        protect_running(&s, &v);
        Ok(s.multi.status(v.multi_instance, v.protect_shared_login))
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
}

#[tauri::command]
pub async fn roblox_close_all(s: State<'_, App>, force: bool) -> Result<MultiInstanceStatus> {
    close_roblox(&s, force, false).await
}

#[tauri::command]
pub async fn panic_kill(s: State<'_, App>) -> Result<MultiInstanceStatus> {
    kill_all(&s).await
}

pub fn set_panic_hotkey(app: &AppHandle, old: &str, key: &str) -> Result<()> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
    let parse = |k: &str| {
        k.parse::<Shortcut>()
            .map_err(|_| AppError::Invalid(tr!("\"{k}\" isn't a key combination, like Ctrl+Shift+F12.", k = k.chars().take(32).collect::<String>())))
    };
    let new = if key.is_empty() { None } else { Some(parse(key)?) };
    let gs = app.global_shortcut();
    let old = parse(old).ok().filter(|&o| gs.is_registered(o));
    if new == old {
        return Ok(());
    }
    if let Some(o) = old
        && let Err(e) = gs.unregister(o)
    {
        log::warn!("could not unregister the old panic hotkey: {e}");
    }
    if let Some(n) = new
        && let Err(e) = gs.register(n)
    {
        if let Some(o) = old
            && let Err(e) = gs.register(o)
        {
            log::warn!("could not re-register the old panic hotkey; none is bound: {e}");
        }
        log::warn!("could not register the panic hotkey {key}: {e}");
        return Err(AppError::Invalid(tr!("Windows wouldn't give LRAM {key}: another app probably uses it. Pick another.", key = key)));
    }
    Ok(())
}

pub(crate) async fn kill_all(s: &App) -> Result<MultiInstanceStatus> {
    log::info!("kill all Roblox");
    s.bulk.kill();
    s.tracker.mark_all_closing();
    for r in close_all(s.tracker.bootstrappers(), true, Duration::ZERO).await? {
        if let Err(e) = r {
            log::warn!("could not close a bootstrapper: {e}");
        }
    }
    close_roblox(s, true, true).await
}

async fn close_roblox(s: &App, force: bool, keep_closing: bool) -> Result<MultiInstanceStatus> {
    s.tracker.cancel_relaunches();
    for i in s.tracker.list() {
        s.tracker.mark_closing(&i.id);
    }
    let procs = crate::instances::process::roblox_processes();
    let targets = procs.iter().map(|p| (p.pid, Some(p.start_time))).collect();
    for (p, r) in procs.iter().zip(close_all(targets, force, grace(s)).await?) {
        if let Err(e) = r {
            log::warn!("could not close Roblox process {}: {e}", p.pid);
            for i in s.tracker.list().iter().filter(|i| !keep_closing && i.pid == p.pid && i.state != InstanceState::Exited) {
                s.tracker.clear_closing(&i.id);
            }
        }
    }
    let v = s.db.settings()?;
    retake_multi(s, &v);
    Ok(s.multi.status(v.multi_instance, v.protect_shared_login))
}

fn diagnostics_sources(s: &App) -> Result<crate::diagnostics::Sources> {
    use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    let reg = |value| crate::launcher::client::reg_string(HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", Some(value)).unwrap_or_default();
    let v = s.db.settings()?;
    let head = format!(
        "LRAM {} on {} {} (build {})\nMulti-instance: {}\nPerformance: {}\n",
        env!("CARGO_PKG_VERSION"),
        reg("ProductName"),
        reg("DisplayVersion"),
        reg("CurrentBuildNumber"),
        v.multi_instance,
        crate::performance::of(&v, None, now_ms()),
    );
    let now = now_ms();
    let list = s.tracker.list();
    let running: Vec<&Instance> = list.iter().filter(|i| i.state != InstanceState::Exited).collect();
    let mut clients = format!("Clients: {} running\n", running.len());
    for i in running {
        let up = (now - i.started_at).max(0) / 1000;
        let window = crate::instances::window::report_state(i.pid).unwrap_or_else(|| "no window".to_owned());
        let cell = crate::instances::window::cell_of(i.pid).map_or_else(|| "none".to_owned(), |(x, y, w, h)| format!("{w}x{h} at {x},{y}"));
        clients.push_str(&format!(
            "  instance {} account {}: {:?}{}, pid {}, up {}s, rejoins {}, cell {cell}, window {window}, limits {}\n",
            i.id,
            i.account_id,
            i.state,
            i.note.as_ref().map_or_else(String::new, |n| format!(" ({n})")),
            i.pid,
            up,
            i.rejoins,
            crate::performance::limits_line(&i.applied_limits),
        ));
    }
    let mut names: Vec<String> = s.db.accounts()?.into_iter().flat_map(|a| [a.username, a.display_name, a.alias]).collect();
    names.extend(s.db.favourites()?.into_iter().map(|f| f.name));
    let mut roblox_logs = s.tracker.live_logs();
    for crash in s.db.crashes()?.iter().filter(|c| c.has_log).take(5) {
        if let Ok(path) = s.db.crash_log(crash.id) {
            let path = std::path::PathBuf::from(path);
            if !roblox_logs.contains(&path) {
                roblox_logs.push(path);
            }
        }
    }
    roblox_logs.truncate(5);
    Ok(crate::diagnostics::Sources { head, clients, lram_log: s.data_dir.join("logs").join("lram.log"), roblox_logs, names })
}

#[tauri::command]
pub async fn diagnostics_text(s: State<'_, App>) -> Result<String> {
    let src = diagnostics_sources(&s)?;
    blocking(move || crate::diagnostics::report(&src, crate::diagnostics::FOR_CLIPBOARD)).await
}

#[tauri::command]
pub async fn diagnostics_save(s: State<'_, App>, path: String) -> Result<()> {
    let src = diagnostics_sources(&s)?;
    let text = blocking(move || crate::diagnostics::report(&src, crate::diagnostics::FOR_FILE)).await?;
    crate::vault::backup::write_atomic(Path::new(&path), text.as_bytes())
}

#[tauri::command]
pub fn server_history(s: State<'_, App>, account_id: i64) -> Result<Vec<crate::model::ServerVisit>> {
    s.db.servers(account_id)
}

#[tauri::command]
pub fn crashes_list(s: State<'_, App>) -> Result<Vec<Crash>> {
    s.db.crashes()
}

#[tauri::command]
pub fn crash_open_log(app: AppHandle, s: State<'_, App>, id: i64) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    let path = s.db.crash_log(id)?;
    if !Path::new(&path).is_file() {
        return Err(AppError::Invalid(tr!("That client log is gone; Roblox deletes old logs.")));
    }
    app.opener().open_path(path, None::<&str>).map_err(|e| AppError::Internal(e.to_string()))
}

const FAVOURITE_AAD: &[u8] = b"roalt.favourite";

pub(crate) fn favourite_of(s: &App, row: crate::storage::FavouriteRow) -> Result<Favourite> {
    let json = s.vault.decrypt(FAVOURITE_AAD, &row.target)?;
    let target = serde_json::from_slice(&json).map_err(|e| {
        log::warn!("favourite {} target unreadable: {:?} at {}:{}", row.id, e.classify(), e.line(), e.column());
        AppError::Crypto
    })?;
    Ok(Favourite {
        id: row.id,
        name: row.name,
        target,
        account_ids: row.account_ids,
        in_tray: row.in_tray,
        sort_order: row.sort_order,
        last_used_at: row.last_used_at,
    })
}

#[tauri::command]
pub fn favourites_list(s: State<'_, App>) -> Result<Vec<Favourite>> {
    s.db.favourites()?.into_iter().map(|r| favourite_of(&s, r)).collect()
}

#[tauri::command]
pub fn favourite_save(app: AppHandle, s: State<'_, App>, input: FavouriteInput) -> Result<Favourite> {
    not_too_many(&input.account_ids)?;
    let json = zeroize::Zeroizing::new(serde_json::to_vec(&input.target).map_err(|e| AppError::Internal(e.to_string()))?);
    let blob = s.vault.encrypt(FAVOURITE_AAD, &json)?;
    let id = s.db.save_favourite(input.id, &input.name, &blob, &input.account_ids, input.in_tray)?;
    crate::tray::refresh(&app);
    let row = s.db.favourites()?.into_iter().find(|f| f.id == id).ok_or(AppError::NotFound)?;
    favourite_of(&s, row)
}

#[tauri::command]
pub fn favourite_remove(app: AppHandle, s: State<'_, App>, id: i64) -> Result<()> {
    unlocked(&s)?;
    s.db.remove_favourite(id)?;
    crate::tray::refresh(&app);
    Ok(())
}

#[tauri::command]
pub fn favourites_reorder(app: AppHandle, s: State<'_, App>, ids: Vec<i64>) -> Result<()> {
    unlocked(&s)?;
    not_too_many(&ids)?;
    s.db.reorder_favourites(&ids)?;
    crate::tray::refresh(&app);
    Ok(())
}

fn presence_kind(s: Option<&str>) -> PresenceKind {
    match s {
        Some("InGame") => PresenceKind::InGame,
        Some("Online") => PresenceKind::Online,
        Some("InStudio") => PresenceKind::Studio,
        Some("Offline") => PresenceKind::Offline,
        None => PresenceKind::Unknown,
        Some(other) => {
            log::debug!("unrecognised friend presence {other:?}");
            PresenceKind::Unknown
        }
    }
}

#[tauri::command]
pub async fn join_candidates(app: AppHandle, s: State<'_, App>, account_id: i64) -> Result<Vec<JoinCandidate>> {
    let me = s.db.account(account_id)?;
    let cookie = stored_cookie(&s, account_id)?;
    let friends = match s.api.friends_online(account_id, &cookie, me.user_id).await {
        Ok(f) => f,
        Err(e) if record_cookie_error(&app, &s, account_id, &e)? => return Err(e),
        Err(e) => {
            log::info!("friends online for account {account_id} failed: {e}");
            Vec::new()
        }
    };
    let mut out: Vec<JoinCandidate> = friends
        .into_iter()
        .map(|f| {
            let p = f.presence.unwrap_or_default();
            JoinCandidate {
                user_id: f.id,
                username: f.name,
                display_name: f.display_name,
                account_id: None,
                presence: Presence {
                    kind: presence_kind(p.user_presence_type.as_deref()),
                    last_location: p.last_location,
                    place_id: p.place_id,
                    root_place_id: p.root_place_id,
                    universe_id: p.universe_id,
                    game_id: p.game_instance_id,
                },
            }
        })
        .collect();
    for a in s.db.accounts()?.into_iter().filter(|a| a.id != account_id && a.status == AccountStatus::Valid) {
        let c = match stored_cookie(&s, a.id) {
            Ok(c) => c,
            Err(e) => {
                log::debug!("join candidates: cookie of account {} unusable: {e}", a.id);
                continue;
            }
        };
        let Ok(rows) = s.api.presence(a.id, &c, &[a.user_id]).await else { continue };
        let Some(p) = rows.into_iter().find(|p| p.user_id == a.user_id) else { continue };
        out.retain(|f| f.user_id != a.user_id);
        out.push(JoinCandidate {
            user_id: a.user_id,
            username: a.username,
            display_name: a.display_name,
            account_id: Some(a.id),
            presence: presence_of(p),
        });
    }
    Ok(out)
}

fn failed(app: &AppHandle, s: &App, account_id: i64, e: AppError) -> AppError {
    if let Err(db) = record_cookie_error(app, s, account_id, &e) {
        log::warn!("account {account_id}: could not record its cookie error: {db}");
    }
    e
}

#[tauri::command]
pub async fn friends_requests(app: AppHandle, s: State<'_, App>, account_id: i64) -> Result<Vec<UserRef>> {
    let me = s.db.account(account_id)?;
    let cookie = stored_cookie(&s, account_id)?;
    let rows = s.api.friend_requests(account_id, &cookie, me.user_id).await.map_err(|e| failed(&app, &s, account_id, e))?;
    let mut out: Vec<UserRef> = Vec::new();
    for row in rows {
        let Some(id) = row.user_id().filter(|&id| id > 0) else { continue };
        if out.iter().any(|u| u.user_id == id) {
            continue;
        }
        let name = row.name.clone().unwrap_or_default();
        out.push(UserRef { user_id: id, username: name.clone(), display_name: row.display_name.clone().unwrap_or(name) });
    }
    let unnamed: Vec<i64> = out.iter().filter(|u| u.username.is_empty()).map(|u| u.user_id).collect();
    if !unnamed.is_empty() {
        match s.api.users_by_ids(&unnamed).await {
            Ok(users) => {
                for u in users {
                    if let Some(row) = out.iter_mut().find(|r| r.user_id == u.id) {
                        *row = UserRef { user_id: u.id, username: u.name.clone(), display_name: u.display_name };
                    }
                }
            }
            Err(e) => log::info!("names for account {account_id}'s friend requests: {e}"),
        }
    }
    Ok(out)
}

#[tauri::command]
pub async fn friend_request_send(app: AppHandle, s: State<'_, App>, account_id: i64, name: String) -> Result<UserRef> {
    let me = s.db.account(account_id)?;
    let cookie = stored_cookie(&s, account_id)?;
    let name = name.trim().trim_start_matches('@').to_owned();
    if name.is_empty() {
        return Err(AppError::Invalid(tr!("Type a Roblox username.")));
    }
    let target = s
        .api
        .users_by_name(std::slice::from_ref(&name))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Invalid(tr!("No Roblox user is called \"{name}\".", name = name)))?;
    if target.id == me.user_id {
        return Err(AppError::Invalid(tr!("That is this account itself.")));
    }
    s.api.friend_request_send(account_id, &cookie, target.id).await.map_err(|e| failed(&app, &s, account_id, e))?;
    Ok(UserRef { user_id: target.id, username: target.name, display_name: target.display_name })
}

#[tauri::command]
pub async fn friend_request_answer(
    app: AppHandle,
    s: State<'_, App>,
    account_id: i64,
    requester_id: i64,
    accept: bool,
) -> Result<()> {
    if requester_id <= 0 {
        return Err(AppError::Invalid(tr!("That friend request is gone.")));
    }
    s.db.account(account_id)?;
    let cookie = stored_cookie(&s, account_id)?;
    s.api.friend_request_answer(account_id, &cookie, requester_id, accept).await.map_err(|e| failed(&app, &s, account_id, e))
}

pub fn lock_vault(app: &AppHandle) {
    for (label, w) in app.webview_windows() {
        if label.starts_with("session-")
            && let Err(e) = w.close()
        {
            log::warn!("could not close session window {label} on lock: {e}");
        }
    }
    let s = app.state::<App>();
    s.vault.lock();
    let _ = app.emit("vault://locked", ());
    crate::tray::refresh(app);
}

#[tauri::command]
pub fn discord_get(s: State<'_, App>) -> Result<DiscordStatus> {
    crate::discord::status(&s)
}

#[tauri::command]
pub fn discord_set(s: State<'_, App>, url: Option<String>) -> Result<DiscordStatus> {
    crate::discord::set(&s, url.as_deref())
}

#[tauri::command]
pub async fn discord_test(s: State<'_, App>) -> Result<()> {
    crate::discord::test(&s).await
}

#[tauri::command]
pub fn groups_list(s: State<'_, App>) -> Result<Vec<Group>> {
    s.db.groups()
}

#[tauri::command]
pub fn group_save(app: AppHandle, s: State<'_, App>, input: GroupInput) -> Result<Group> {
    unlocked(&s)?;
    let g = s.db.save_group(&input)?;
    accounts_changed(&app);
    Ok(g)
}

#[tauri::command]
pub fn group_remove(app: AppHandle, s: State<'_, App>, id: i64) -> Result<()> {
    unlocked(&s)?;
    s.db.remove_group(id)?;
    accounts_changed(&app);
    Ok(())
}

const MIN_BACKUP_PASSWORD: usize = 8;

#[tauri::command]
pub async fn backup_export(
    app: AppHandle,
    s: State<'_, App>,
    path: String,
    password: String,
    current_password: Option<String>,
    account_ids: Option<Vec<i64>>,
) -> Result<u32> {
    if password.chars().count() < MIN_BACKUP_PASSWORD {
        return Err(AppError::Invalid(tr!("Use at least {min} characters for the backup password.", min = MIN_BACKUP_PASSWORD)));
    }
    reauth(&app, &s, current_password, &tr!("Confirm it's you to export your Roblox accounts.")).await?;
    let groups = s.db.groups()?;
    let name_of = |gid: i64| groups.iter().find(|g| g.id == gid).map(|g| g.name.clone());
    let mut accounts = Vec::new();
    for a in s.db.accounts()? {
        if account_ids.as_ref().is_some_and(|ids| !ids.contains(&a.id)) {
            continue;
        }
        let cookie = stored_cookie(&s, a.id)?;
        accounts.push(BackupAccount {
            user_id: a.user_id,
            username: a.username,
            display_name: a.display_name,
            alias: a.alias,
            notes: a.notes,
            color: a.color,
            favourite: a.favourite,
            sort_order: a.sort_order,
            groups: a.group_ids.iter().filter_map(|g| name_of(*g)).collect(),
            cookie: cookie.expose().to_owned().into(),
            allow_disconnect: Some(a.allow_disconnect),
            launch_args: a.launch_args,
            discord_muted: a.discord_muted,
            fps_cap: a.fps_cap,
            full_quality: a.full_quality,
        });
    }
    let user_of = |id: i64| accounts.iter().find(|a| s.db.account_id_for_user(a.user_id).ok().flatten() == Some(id)).map(|a| a.user_id);
    let mut favourites = Vec::new();
    for row in s.db.favourites()? {
        let f = favourite_of(&s, row)?;
        favourites.push(backup::BackupFavourite { name: f.name, target: f.target, user_ids: f.account_ids.iter().filter_map(|id| user_of(*id)).collect(), in_tray: f.in_tray });
    }
    let count = accounts.len() as u32;
    let payload = BackupPayload {
        version: 1,
        exported_at: now_ms(),
        accounts,
        groups: groups.iter().map(|g| BackupGroup { name: g.name.clone(), color: g.color.clone(), sort_order: g.sort_order }).collect(),
        favourites,
    };
    backup::write(Path::new(&path), &password, &payload)?;
    Ok(count)
}

fn check_backup(p: &backup::BackupPayload) -> Result<()> {
    let bad = |t: &str| t.chars().count() > 64;
    let wrong = p.version != 1
        || p.accounts.iter().any(|a| a.user_id <= 0 || bad(&a.username) || bad(&a.display_name))
        || p.groups.iter().any(|g| bad(&g.name))
        || p.favourites.iter().any(|f| f.name.is_empty() || bad(&f.name));
    if wrong {
        return Err(AppError::Invalid(tr!("This isn't a LRAM backup.")));
    }
    for a in &p.accounts {
        if let Some(args) = &a.launch_args {
            crate::launcher::launch_args(args)?;
        }
        if let Some(fps) = a.fps_cap {
            check_fps(fps)?;
        }
    }
    p.accounts.iter().try_for_each(|a| crate::storage::check_meta(&AccountMeta { alias: a.alias.clone(), notes: a.notes.clone(), color: a.color.clone(), favourite: a.favourite }))
}

#[tauri::command]
pub async fn backup_import(app: AppHandle, s: State<'_, App>, path: String, password: String, on_conflict: ConflictPolicy) -> Result<ImportReport> {
    if s.vault.is_locked() {
        return Err(AppError::Locked);
    }
    let payload = backup::read(Path::new(&path), &password)?;
    check_backup(&payload)?;
    for g in &payload.groups {
        s.db.group_by_name_or_create(&g.name, g.color.as_deref())?;
    }
    let mut report = ImportReport::default();
    for a in &payload.accounts {
        let existing = s.db.account_id_for_user(a.user_id)?;
        if existing.is_some() && on_conflict == ConflictPolicy::Skip {
            report.skipped += 1;
            continue;
        }
        let Ok(mut cookie) = normalize_cookie(&a.cookie) else {
            report.failed += 1;
            continue;
        };
        if existing.is_some() {
            match s.api.authenticated_user(0, &cookie).await {
                Ok((u, rotated)) if u.id == a.user_id => cookie = rotated.unwrap_or(cookie),
                _ => {
                    report.failed += 1;
                    continue;
                }
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        let blob = s.vault.encrypt(&cookie_aad(a.user_id), cookie.expose().as_bytes())?;
        let id = s.db.upsert_account(&NewAccount {
            user_id: a.user_id,
            username: &a.username,
            display_name: &a.display_name,
            cookie: &blob,
            status: AccountStatus::Unknown,
            checked_at: None,
        })?;
        s.db.set_meta(id, &AccountMeta { alias: a.alias.clone(), notes: a.notes.clone(), color: a.color.clone(), favourite: a.favourite })?;
        let gids = a.groups.iter().map(|n| s.db.group_by_name_or_create(n, None)).collect::<Result<Vec<_>>>()?;
        s.db.set_account_groups(id, &gids)?;
        if a.allow_disconnect.is_some() {
            s.db.update_account(id, &AccountPatch {
                allow_disconnect: a.allow_disconnect,
                launch_args: Some(a.launch_args.clone().unwrap_or_default()),
                discord_muted: Some(a.discord_muted),
                fps_cap: Some(a.fps_cap.unwrap_or(0)),
                full_quality: Some(a.full_quality),
                ..Default::default()
            })?;
        }
        if existing.is_some() { report.updated += 1 } else { report.added += 1 }
    }
    let have: Vec<String> = s.db.favourites()?.into_iter().map(|f| f.name).collect();
    for f in payload.favourites.iter().filter(|f| !have.contains(&f.name)) {
        let ids: Vec<i64> = f.user_ids.iter().filter_map(|u| s.db.account_id_for_user(*u).ok().flatten()).collect();
        let json = zeroize::Zeroizing::new(serde_json::to_vec(&f.target).map_err(|e| AppError::Internal(e.to_string()))?);
        let blob = s.vault.encrypt(FAVOURITE_AAD, &json)?;
        s.db.save_favourite(None, &f.name, &blob, &ids, f.in_tray)?;
    }
    crate::tray::refresh(&app);
    accounts_changed(&app);
    Ok(report)
}

#[tauri::command]
pub fn settings_get(s: State<'_, App>) -> Result<Settings> {
    s.db.settings()
}

#[tauri::command]
pub fn performance_profile(s: State<'_, App>, preview_fps: Option<u32>) -> Result<String> {
    Ok(crate::performance::of(&s.db.settings()?, preview_fps, now_ms()))
}

#[tauri::command]
pub async fn performance_export(s: State<'_, App>, path: String, preview_fps: Option<u32>) -> Result<()> {
    let text = crate::performance::of(&s.db.settings()?, preview_fps, now_ms());
    crate::vault::backup::write_atomic(Path::new(&path), text.as_bytes())
}

#[tauri::command]
pub async fn performance_read(s: State<'_, App>, path: String) -> Result<crate::performance::Profile> {
    let refused = || AppError::Invalid(tr!("This isn't a LRAM performance profile."));
    if std::fs::metadata(&path)?.len() > crate::performance::MAX_BYTES as u64 {
        return Err(refused());
    }
    let text = std::fs::read_to_string(&path).map_err(|_| refused())?;
    crate::performance::parse(&text, &s.db.settings()?)
}

#[tauri::command]
pub fn performance_parse(s: State<'_, App>, text: String) -> Result<crate::performance::Profile> {
    crate::performance::parse(&text, &s.db.settings()?)
}

pub(crate) fn apply_performance(v: &mut Settings, patch: &SettingsPatch) -> Result<()> {
    if let Some(f) = patch.fps_cap {
        check_fps(f)?;
        v.fps_cap = f;
    }
    if let Some(l) = patch.low_graphics { v.low_graphics = l }
    if let Some(l) = patch.lighter_rendering { v.lighter_rendering = l }
    if let Some(r) = patch.render_flags { v.render_flags = r }
    if let Some(m) = patch.minimize_on_launch { v.minimize_on_launch = m }
    if let Some(t) = patch.tiny_on_launch { v.tiny_on_launch = t }
    if let Some(a) = patch.auto_tile { v.auto_tile = a }
    if let Some(l) = patch.live_hidden_previews { v.live_hidden_previews = l }
    if patch.micro_width.is_some() || patch.micro_height.is_some() {
        let size = (patch.micro_width.unwrap_or(v.micro_width), patch.micro_height.unwrap_or(v.micro_height));
        check_window_size(size)?;
        (v.micro_width, v.micro_height) = size;
    }
    if let Some(m) = patch.mem_restart_mb {
        if m != 0 && !(512..=65_536).contains(&m) {
            return Err(AppError::Invalid(tr!("The memory limit must be 0 (off) or 512 to 65 536 MB.")));
        }
        v.mem_restart_mb = m;
    }
    if let Some(t) = patch.mem_throttle {
        if t.mb != 0 && !(512..=65_536).contains(&t.mb) {
            return Err(AppError::Invalid(tr!("The memory throttle must be 0 (off) or 512 to 65 536 MB.")));
        }
        if !(1..=100).contains(&t.cpu_cap) {
            return Err(AppError::Invalid(tr!("The throttle's CPU limit must be 1 to 100 %.")));
        }
        v.mem_throttle = t;
    }
    if let Some(m) = patch.trim_every_min {
        if m > 24 * 60 {
            return Err(AppError::Invalid(tr!("Trimming can be at most once a day apart.")));
        }
        v.trim_every_min = m;
    }
    if let Some(a) = patch.auto_spread { v.auto_spread = a }
    if let Some(f) = patch.fair_cap { v.fair_cap = f }
    if let Some(u) = patch.ultra_cpu {
        if !(1..=2).contains(&u.cap) {
            return Err(AppError::Invalid(tr!("ULTRA CPU must be 1% or 2%.")));
        }
        v.ultra_cpu = u;
    }
    if let Some(a) = patch.adaptive {
        check_limits(&a.background)?;
        v.adaptive = a;
    }
    Ok(())
}

#[tauri::command]
// -- Main func
pub fn settings_set(app: AppHandle, s: State<'_, App>, patch: SettingsPatch) -> Result<Settings> {
    if patch.auto_lock_minutes.is_some() {
        unlocked(&s)?;
    }
    let mut v = s.db.settings()?;
    if let Some(private) = patch.privacy_mode { v.privacy_mode = private; }
    apply_performance(&mut v, &patch)?;
    if let Some(t) = patch.theme { v.theme = t }
    if let Some(a) = patch.accent {
        if !(a.is_empty() || (a.len() == 7 && a.starts_with('#') && a[1..].bytes().all(|c| c.is_ascii_hexdigit()))) {
            return Err(AppError::Invalid(tr!("The accent must be a #rrggbb colour.")));
        }
        v.accent = a.to_ascii_lowercase();
    }
    if let Some(d) = patch.background_dim {
        if d > 95 {
            return Err(AppError::Invalid(tr!("The background dim must be 0 to 95 %.")));
        }
        v.background_dim = d;
    }
    if let Some(d) = patch.density { v.density = d }
    if let Some(m) = patch.auto_lock_minutes {
        if m > 24 * 60 {
            return Err(AppError::Invalid(tr!("Auto-lock can be at most 24 hours.")));
        }
        v.auto_lock_minutes = m;
    }
    if let Some(c) = patch.clipboard_clear_seconds {
        if !(5..=300).contains(&c) {
            return Err(AppError::Invalid(tr!("Clipboard clearing must be between 5 and 300 seconds.")));
        }
        v.clipboard_clear_seconds = c;
    }
    if let Some(c) = patch.cache_minutes {
        if !(1..=24 * 60).contains(&c) {
            return Err(AppError::Invalid(tr!("Cache time must be between 1 minute and 24 hours.")));
        }
        v.cache_minutes = c;
    }
    if let Some(r) = patch.auto_rejoin { v.auto_rejoin = r }
    if let Some(r) = patch.auto_restart_on_crash { v.auto_restart_on_crash = r }
    if let Some(n) = patch.breaker_max {
        if !(1..=20).contains(&n) {
            return Err(AppError::Invalid(tr!("The restart limit must be between 1 and 20.")));
        }
        v.breaker_max = n;
    }
    if let Some(m) = patch.breaker_window_min {
        if !(1..=240).contains(&m) {
            return Err(AppError::Invalid(tr!("The restart window must be between 1 minute and 4 hours.")));
        }
        v.breaker_window_min = m;
    }
    if let Some(g) = patch.close_grace_s {
        if !(1..=60).contains(&g) {
            return Err(AppError::Invalid(tr!("The close grace must be between 1 and 60 seconds.")));
        }
        v.close_grace_s = g;
    }
    if let Some(a) = patch.launch_args {
        crate::launcher::launch_args(&a)?;
        v.launch_args = a.trim().to_owned();
    }
    if let Some(l) = patch.tab_layout { v.tab_layout = l }
    if let Some(c) = patch.close_to_tray { v.close_to_tray = c }
    if let Some(s) = patch.skip_splash { v.skip_splash = s }
    if let Some(a) = patch.anti_afk {
        if !(1..=19).contains(&a.interval_min) {
            return Err(AppError::Invalid(tr!("Anti-AFK must press a key every 1 to 19 minutes.")));
        }
        v.anti_afk = a;
    }
    if let Some(m) = patch.mute_clients {
        v.mute_clients = m;
        s.tracker.set_all_muted(m);
        crate::instances::audio::wake();
    }
    if let Some(d) = patch.discord {
        if d.status_every_min != 0 && !(5..=24 * 60).contains(&d.status_every_min) {
            return Err(AppError::Invalid(tr!("The status summary must be off (0) or every 5 minutes to 24 hours.")));
        }
        v.discord = d;
    }
    if let Some(a) = patch.auto_update { v.auto_update = a }
    let self_update = patch.self_update.filter(|&u| u != v.self_update);
    if let Some(u) = self_update { v.self_update = u }
    if let Some(c) = patch.clean_logs { v.clean_logs = c }
    if let Some(d) = patch.launch_delay_ms {
        if !(2_000..=60_000).contains(&d) {
            return Err(AppError::Invalid(tr!("The launch delay must be between 2 and 60 seconds.")));
        }
        v.launch_delay_ms = d;
    }
    if let Some(p) = patch.protect_shared_login
        && p != v.protect_shared_login
    {
        v.protect_shared_login = p;
        if !p {
            s.multi.release_shared();
        } else if v.multi_instance {
            protect_running(&s, &v);
        }
    }
    let had = v.panic_hotkey.clone();
    if let Some(k) = patch.panic_hotkey {
        let k = k.trim();
        set_panic_hotkey(&app, &had, k)?;
        v.panic_hotkey = k.to_owned();
    }
    let rest = {
        if let Some(a) = patch.allow_public_networks {
            v.allow_public_networks = a;
        }
        s.db.save_settings(&v)
    };
    if rest.is_err()
        && v.panic_hotkey != had
        && let Err(e) = set_panic_hotkey(&app, &v.panic_hotkey, &had)
    {
        log::warn!("could not roll back the panic hotkey after a failed save: {e}");
    }
    rest?;
    if let Some(u) = self_update {
        crate::self_update::switched(&app, u);
    }
    Ok(v)
}

#[tauri::command]
pub fn network_status(s: State<'_, App>) -> NetworkStatus {
    s.network.get()
}

#[cfg(test)]
mod tests {
    #[test]
    fn tail_keeps_the_last_lines() {
        let path = std::env::temp_dir().join(format!("lram-tail-{}.log", uuid::Uuid::new_v4()));
        assert!(crate::commands::tail(&path, 5).unwrap().is_empty());
        std::fs::write(&path, "a\nb\nc\nd\n").unwrap();
        assert_eq!(crate::commands::tail(&path, 2).unwrap(), ["c", "d"]);
        assert_eq!(crate::commands::tail(&path, 10).unwrap(), ["a", "b", "c", "d"]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_ui_message_cannot_forge_a_log_line() {
        assert_eq!(super::ui_log_text("two\nlines\r\nmore".into()), "two lines  more");
        let long = super::ui_log_text("ü".repeat(3000));
        assert_eq!(long.chars().count(), 2048);
    }

    #[test]
    fn recovery_file_holds_only_a_code() {
        assert!(super::recovery_file_text("abc def 234").unwrap().contains("ABC-DEF-234"));
        for other in ["", "ABC-DEF-23", "ABC-DEF-230", "cookie=_|WARNING:-DO-NOT-SHARE", "ABC-DEF-234\nand more"] {
            assert!(matches!(super::recovery_file_text(other), Err(crate::error::AppError::Invalid(_))), "{other:?}");
        }
    }

    use super::{GameDetails, GameInfo, find_cookies, game_info_of, is_image, looks_like_credentials, normalize_cookie, ram_accounts, shutdown_secs};

    #[test]
    fn knows_images_by_their_bytes() {
        assert!(is_image(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0]));
        assert!(is_image(&[0xff, 0xd8, 0xff, 0xe0]));
        assert!(is_image(b"GIF89a"));
        assert!(is_image(b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(!is_image(b"RIFF\0\0\0\0WAVE"));
        assert!(!is_image(b"<svg></svg>"));
        assert!(!is_image(b""));
    }

    const W: &str = "_|WARNING:-DO-NOT-SHARE-THIS.--Sharing-this-will-allow-someone-to-log-in-as-you-and-to-steal-your-ROBUX-and-items.|_";

    #[test]
    fn finds_cookies_in_pasted_lists() {
        let (a, b) = (format!("{W}ABC123"), format!("{W}CAEaAh+/x="));
        let text = format!("user:pass:{a}
  {b}
.ROBLOSECURITY={a};
\"{b}\", other
nothing here
{W}
{a} {b}");
        assert_eq!(find_cookies(&text), [a.clone(), b.clone()]);
        assert_eq!(find_cookies(&format!("{a}:user:pass")), std::slice::from_ref(&a));
        assert_eq!(find_cookies(&format!("{a}\u{201d}\u{200b}")), [a], "a smart quote or zero-width space ends it");
        assert!(find_cookies("no cookies").is_empty());
    }

    #[test]
    fn tells_a_login_and_password_from_a_cookie() {
        assert!(looks_like_credentials("coolguy:hunter2"));
        assert!(looks_like_credentials("  farm3 : s3cret!  \n"));
        assert!(looks_like_credentials("user:pass:cookie-value"), "the shared list format is recognised too");
        assert!(!looks_like_credentials(&format!("{W}CAEaAh+/x=")), "a cookie has no colon");
        assert!(!looks_like_credentials("nothing here"));
        assert!(!looks_like_credentials("user:"));
        assert!(!looks_like_credentials(":pass"));
        assert!(!looks_like_credentials("two words:pass"), "a space before the colon is prose, not a login");
    }

    #[test]
    fn reads_roblox_account_manager_json() {
        let json = br#"[{"SecurityToken":"tok","Username":"u","Alias":"farm","Description":"note","Group":"Default"},{"SecurityToken":"","Alias":null}]"#;
        let list = ram_accounts(json).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].0.as_deref(), list[0].1.alias.as_str(), list[0].1.notes.as_str()), (Some("tok"), "farm", "note"));
        assert!(list[1].0.is_none() && list[1].1.alias.is_empty());
        let bom = [b"\xEF\xBB\xBF".as_slice(), b"[]"].concat();
        assert!(ram_accounts(&bom).unwrap().is_empty());
        assert!(ram_accounts(b"{}").is_err());
    }

    #[test]
    fn shutdown_delays() {
        let now = 1_000_000_000_000;
        assert_eq!(shutdown_secs(None, Some(3600), now).unwrap(), 3600);
        assert_eq!(shutdown_secs(Some(now + 5_000_000), Some(60), now).unwrap(), 60);
        assert_eq!(shutdown_secs(Some(now + 90_001), None, now).unwrap(), 91);
        for bad in [(None, None), (None, Some(59)), (Some(now - 1000), None), (Some(now + 59_000), None), (None, Some(7 * 24 * 3600 + 1))] {
            assert!(shutdown_secs(bad.0, bad.1, now).is_err(), "{bad:?}");
        }
        assert_eq!(shutdown_secs(None, Some(7 * 24 * 3600), now).unwrap(), 7 * 24 * 3600);
    }

    #[test]
    fn the_suggested_panic_hotkey_parses() {
        assert!(crate::model::Settings::default().panic_hotkey.is_empty(), "off until the user picks one");
        let k = "Ctrl+Shift+F12";
        assert!(k.parse::<tauri_plugin_global_shortcut::Shortcut>().is_ok(), "{k}");
        assert!("Ctrl+Shift+".parse::<tauri_plugin_global_shortcut::Shortcut>().is_err());
    }

    #[test]
    fn game_info_keeps_the_asked_place() {
        let g = GameDetails {
            universe_id: 994732206,
            root_place_id: 2753915549,
            name: "Blox Fruits".into(),
            creator: "Gamer Robot Inc".into(),
            playing: 193096,
            visits: 65079604670,
            max_players: 12,
            private_servers: true,
        };
        let info = game_info_of(7449423635, g, Some("https://tr.rbxcdn.com/i".into()));
        let want = GameInfo {
            place_id: 7449423635,
            universe_id: 994732206,
            root_place_id: 2753915549,
            name: "Blox Fruits".into(),
            creator: "Gamer Robot Inc".into(),
            playing: 193096,
            visits: 65079604670,
            max_players: 12,
            icon_url: Some("https://tr.rbxcdn.com/i".into()),
            private_servers: true,
        };
        assert_eq!(info, want);
        assert_eq!(serde_json::from_str::<GameInfo>(&serde_json::to_string(&info).unwrap()).unwrap(), want);
    }

    #[test]
    fn cookie_input_forms() {
        for raw in ["abc123", " abc123 ", "\"abc123\"", ".ROBLOSECURITY=abc123;", "'.ROBLOSECURITY=abc123'"] {
            assert_eq!(normalize_cookie(raw).unwrap().expose(), "abc123", "{raw}");
        }
        assert!(normalize_cookie("").is_err());
        assert!(normalize_cookie("has space").is_err());
        assert!(normalize_cookie(".ROBLOSECURITY=").is_err());
    }
}


#[tauri::command]
pub async fn hardware_info() -> Result<crate::hardware::HardwareInfo> {
    tauri::async_runtime::spawn_blocking(crate::hardware::read)
        .await.map_err(|e| AppError::Internal(e.to_string()))
}

#[tauri::command]
pub async fn public_location() -> Result<crate::hardware::PublicLocation> {
    crate::hardware::public_location().await
}
