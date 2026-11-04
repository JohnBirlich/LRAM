use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use super::Join;
use super::target::is_uuid;
use crate::App;
use crate::error::{AppError, Result};
use crate::instances::rejoin::backoff;
use crate::instances::{auth_failed, challenged, wrong_account};
use crate::roblox_api::types::UserPresence;
use crate::model::{
    AccountStatus, AccountSummary, BulkItem, BulkProgress, BulkRequest, BulkStage, GameServer, Instance,
    InstanceState, LaunchTarget, SameServer, ServerOrder,
};

const EVENT: &str = "bulk://progress";
const MIN_DELAY: u32 = 2_000;
const MAX_DELAY: u32 = 60_000;
fn cancelled() -> String {
    tr!("Cancelled")
}
fn rejoining() -> String {
    tr!("A rejoin is in progress, try again in a moment.")
}
fn launching() -> String {
    tr!("Another launch is in progress, try again in a moment.")
}
const TICK: Duration = Duration::from_millis(500);
const BIND_WAIT: Duration = Duration::from_secs(30);
const FOLLOW_WAIT: Duration = Duration::from_secs(90);
const SETTLE_WAIT: Duration = Duration::from_secs(120);
const CLOSE_WAIT: Duration = Duration::from_secs(10);
const REFUSAL_LASTS: Duration = Duration::from_secs(10 * 60);

pub struct Bulk {
    inner: Mutex<Option<Batch>>,
    rejoining: AtomicUsize,
    launches: AtomicUsize,
    last_launch: Mutex<Option<Instant>>,
    kills: AtomicU64,
    refusals: Mutex<(u32, Option<Instant>)>,
}

pub struct Slot {
    at: Instant,
    prev: Option<Instant>,
}

pub struct Busy<'a>(&'a AtomicUsize);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn busy(n: &AtomicUsize) -> Busy<'_> {
    n.fetch_add(1, Ordering::SeqCst);
    Busy(n)
}

struct Batch {
    progress: BulkProgress,
    cancel: Arc<AtomicBool>,
}

impl Bulk {
    pub fn new() -> Bulk {
        Bulk {
            inner: Mutex::new(None),
            rejoining: AtomicUsize::new(0),
            launches: AtomicUsize::new(0),
            last_launch: Mutex::new(None),
            kills: AtomicU64::new(0),
            refusals: Mutex::new((0, None)),
        }
    }

    pub fn refused(&self) {
        let mut r = self.refusals.lock().unwrap_or_else(|e| e.into_inner());
        *r = (r.0.saturating_add(1), Some(Instant::now()));
    }

    pub fn got_through(&self) {
        *self.refusals.lock().unwrap_or_else(|e| e.into_inner()) = (0, None);
    }

    pub fn launch_gap(&self, delay_ms: u32, now: Instant) -> Duration {
        let (n, at) = *self.refusals.lock().unwrap_or_else(|e| e.into_inner());
        let n = if at.is_some_and(|t| now.duration_since(t) < REFUSAL_LASTS) { n } else { 0 };
        let wanted = if n > 0 { backoff(delay_ms, n).min(MAX_DELAY) } else { delay_ms };
        Duration::from_millis(u64::from(wanted.saturating_sub(1_000)))
    }

    pub fn cooldown(&self, gap: Duration) -> (Duration, Slot) {
        self.reserve(Instant::now(), gap)
    }

    fn reserve(&self, now: Instant, gap: Duration) -> (Duration, Slot) {
        let mut last = self.last_launch.lock().unwrap_or_else(|e| e.into_inner());
        let at = last.map_or(now, |l| (l + gap).max(now));
        let prev = last.replace(at);
        (at - now, Slot { at, prev })
    }

    pub fn release(&self, slot: Slot) {
        let mut last = self.last_launch.lock().unwrap_or_else(|e| e.into_inner());
        if *last == Some(slot.at) {
            *last = slot.prev;
        }
    }

    pub fn kill(&self) {
        self.kills.fetch_add(1, Ordering::SeqCst);
        if let Some(b) = self.batch().as_ref() {
            b.cancel.store(true, Ordering::SeqCst);
        }
    }

    pub fn kills(&self) -> u64 {
        self.kills.load(Ordering::SeqCst)
    }

    pub fn current(&self) -> Option<BulkProgress> {
        self.batch().as_ref().map(|b| b.progress.clone())
    }

    pub fn running(&self) -> bool {
        self.batch().as_ref().is_some_and(|b| !b.progress.done)
    }

    pub fn begin_rejoin(&self) -> Option<Busy<'_>> {
        let batch = self.batch();
        (!batch.as_ref().is_some_and(|b| !b.progress.done)).then(|| busy(&self.rejoining))
    }

    pub fn begin_launch(&self) -> Busy<'_> {
        busy(&self.launches)
    }

    pub fn check_single_launch(&self) -> Result<()> {
        if self.running() {
            Err(AppError::Invalid(tr!("A batch is still running.")))
        } else if self.rejoining.load(Ordering::SeqCst) > 0 {
            Err(AppError::Invalid(rejoining()))
        } else {
            Ok(())
        }
    }

    pub fn begin_single_launch(&self) -> Result<Busy<'_>> {
        self.check_single_launch()?;
        self.launches.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst).map_err(|_| AppError::Invalid(launching()))?;
        Ok(Busy(&self.launches))
    }

    pub fn any_busy(&self) -> bool {
        self.running() || self.rejoining.load(Ordering::SeqCst) > 0 || self.launches.load(Ordering::SeqCst) > 0
    }

    pub fn cancel(&self, batch_id: &str) -> Result<()> {
        match self.batch().as_ref() {
            Some(b) if b.progress.batch_id == batch_id => {
                b.cancel.store(true, Ordering::SeqCst);
                Ok(())
            }
            _ => Err(AppError::NotFound),
        }
    }

    fn batch(&self) -> MutexGuard<'_, Option<Batch>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Default for Bulk {
    fn default() -> Self {
        Self::new()
    }
}

// -- Main func
pub fn start(app: AppHandle, mut req: BulkRequest) -> Result<String> {
    req.fps_cap = crate::commands::launch_fps(req.fps_cap)?;
    let s = app.state::<App>();
    let ids = &req.account_ids;
    if ids.is_empty() {
        return Err(AppError::Invalid(tr!("Pick at least one account.")));
    }
    if ids.iter().enumerate().any(|(i, id)| ids[..i].contains(id)) {
        return Err(AppError::Invalid(tr!("An account is in the list twice.")));
    }
    if s.vault.is_locked() {
        return Err(AppError::Locked);
    }
    if s.update.installing() {
        return Err(AppError::Invalid(crate::commands::installing()));
    }
    let mut accounts = Vec::with_capacity(ids.len());
    for &id in ids {
        let a = s.db.account(id)?;
        let name = name_of(&a);
        if let Some(why) = cant_launch(a.status) {
            return Err(AppError::Invalid(tr!("{name} can't launch: {why}", name = name, why = why)));
        }
        if s.tracker.has_live_client(id) {
            return Err(AppError::Invalid(tr!("{name} is already running. Untick it or close its client first.", name = name)));
        }
        accounts.push(Account { id, user_id: a.user_id, name, allow_disconnect: a.allow_disconnect });
    }
    if ids.len() > 1 && !s.multi.is_active() {
        let v = s.db.settings()?;
        if !v.multi_instance {
            return Err(AppError::Invalid(tr!("Turn on multi-instance to launch several accounts.")));
        }
        s.multi.enable()?;
    }

    let progress = new_progress(Uuid::new_v4().to_string(), &req, &accounts);
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut batch = s.bulk.batch();
        if batch.as_ref().is_some_and(|b| !b.progress.done) {
            return Err(AppError::Invalid(tr!("A batch is already running.")));
        }
        if s.bulk.rejoining.load(Ordering::SeqCst) > 0 {
            return Err(AppError::Invalid(rejoining()));
        }
        let last = batch.replace(Batch { progress: progress.clone(), cancel: cancel.clone() });
        if s.update.installing() {
            *batch = last;
            return Err(AppError::Invalid(crate::commands::installing()));
        }
    }
    let _ = app.emit(EVENT, &progress);
    log::info!("bulk batch {}: {} account(s)", progress.batch_id, accounts.len());
    let run = Run { app: app.clone(), batch_id: progress.batch_id.clone(), cancel, accounts };
    tauri::async_runtime::spawn(run.run(req));
    Ok(progress.batch_id)
}

pub fn choose_server(servers: &[GameServer], n: usize) -> Option<String> {
    let n = i64::try_from(n).ok()?;
    let fullest = |spare: i64| {
        servers
            .iter()
            .filter(|s| is_uuid(&s.id) && s.playing >= 0)
            .filter(|s| i64::from(s.playing) + n + spare <= i64::from(s.max_players))
            .min_by_key(|s| std::cmp::Reverse(s.playing))
    };
    fullest(1).or_else(|| fullest(0)).map(|s| s.id.clone())
}

pub fn jittered(delay_ms: u32) -> Duration {
    let jitter = (Uuid::new_v4().as_u128() % 2_001) as i64 - 1_000;
    let ms = i64::from(delay_ms.clamp(MIN_DELAY, MAX_DELAY)) + jitter;
    Duration::from_millis(ms.max(1_000) as u64)
}

pub fn stage_of(instance: &Instance, allow_disconnect: bool) -> BulkStage {
    if instance.checking_process { return BulkStage::Joining; }
    let failed = |default: fn() -> String| BulkStage::Failed { message: instance.note.clone().unwrap_or_else(default) };
    match instance.state {
        InstanceState::Starting => BulkStage::Starting,
        InstanceState::Joining => BulkStage::Joining,
        InstanceState::InGame => BulkStage::InGame,
        InstanceState::Disconnected if allow_disconnect => BulkStage::Done,
        InstanceState::Disconnected => failed(|| tr!("Disconnected")),
        InstanceState::Exited => failed(crate::instances::closed),
    }
}

#[derive(Default)]
struct Pace {
    refusals: u32,
    in_a_row: u32,
}

impl Pace {
    fn delay(&self, delay_ms: u32) -> u32 {
        backoff(delay_ms, self.refusals)
    }

    fn refused(&mut self) -> bool {
        self.refusals += 1;
        self.in_a_row += 1;
        self.in_a_row >= 2
    }

    fn got_through(&mut self) {
        self.in_a_row = 0;
    }
}

struct Account {
    id: i64,
    user_id: i64,
    name: String,
    allow_disconnect: bool,
}

struct Run {
    app: AppHandle,
    batch_id: String,
    cancel: Arc<AtomicBool>,
    accounts: Vec<Account>,
}

impl Run {
    async fn run(self, req: BulkRequest) {
        let stop = match self.launch_all(&req).await {
            Some(why) => Some(why),
            None => self.wait(SETTLE_WAIT, || self.progress().is_none_or(|p| settled(&p))).await,
        };
        if let Some(why) = &stop {
            log::warn!("bulk batch {} stopped early: {why}", self.batch_id);
        }
        self.update(|p| {
            if let Some(why) = stop {
                halt(p, why);
            }
            p.done = true;
        });
        if let Some(p) = self.progress().filter(|p| p.batch_id == self.batch_id) {
            crate::discord::batch_done(&self.app, &p);
        }
    }

    async fn launch_all(&self, req: &BulkRequest) -> Option<String> {
        let (mut server, mut follow) = (None, None);
        if req.same_server != SameServer::Off {
            self.set_stage(0, BulkStage::Ticket);
            (server, follow) = self.same_server(req).await;
            if let Some(found) = &server {
                self.update(|p| p.job_id = Some(found.1.clone()));
            }
        }
        let (mut started, mut pace) = (false, Pace::default());
        for (i, a) in self.accounts.iter().enumerate() {
            if i > 0
                && let Some(why) = self.wait(jittered(pace.delay(req.delay_ms)), || false).await
            {
                return Some(why);
            }
            if self.cancel.load(Ordering::SeqCst) {
                return Some(cancelled());
            }
            let s = self.app.state::<App>();
            if i > 0 && !s.multi.is_active() {
                return Some(tr!("Multi-instance was turned off. Turn it on."));
            }
            match s.db.account(a.id) {
                Ok(x) if x.user_id == a.user_id => {
                    if let Some(why) = cant_launch(x.status) {
                        log::debug!("bulk batch {}: account {} can't launch: {why}", self.batch_id, a.id);
                        self.set_stage(i, BulkStage::Failed { message: why });
                        if x.status == AccountStatus::Challenged {
                            return stop_reason(&AppError::Challenge, &a.name);
                        }
                        continue;
                    }
                }
                r => {
                    if let Some(e) = r.as_ref().err().filter(|e| !matches!(e, AppError::NotFound)) {
                        log::warn!("bulk batch {}: account {} could not be read: {e}", self.batch_id, a.id);
                    }
                    self.set_stage(i, BulkStage::Failed { message: tr!("Removed during the batch") });
                    continue;
                }
            }

            loop {
                self.set_stage(i, BulkStage::Ticket);
                let target = target_for(&req.target, server.as_ref());
                let favourite = if started { None } else { req.favourite_id };
                let refused = match crate::commands::launch_tracked(&self.app, &s, a.id, target, favourite, Some(req.delay_ms), req.fps_cap, true).await {
                    Ok((info, inst)) => {
                        started = true;
                        self.update(|p| {
                            p.items[i].instance_id = Some(inst.id.clone());
                            p.items[i].stage = BulkStage::Starting;
                            if p.note.is_none() {
                                p.note = info.warning;
                            }
                        });
                        let refused = |x: &Instance| x.note.as_deref().is_some_and(|n| n == auth_failed());
                        let through = || self.instance(&inst.id).is_none_or(|x| refused(&x) || !matches!(x.state, InstanceState::Starting | InstanceState::Joining));
                        if let Some(why) = self.wait(BIND_WAIT, through).await {
                            return Some(why);
                        }
                        if self.instance(&inst.id).is_some_and(|x| refused(&x)) {
                            if let Some(why) = self.close_refused(i, &inst.id).await {
                                return Some(why);
                            }
                            true
                        } else {
                            if let Some(place_id) = follow.take() {
                                match self.followed(a, &inst.id, place_id).await {
                                    Err(why) => return Some(why),
                                    Ok(Some(found)) => {
                                        self.update(|p| p.job_id = Some(found.1.clone()));
                                        server = Some(found);
                                    }
                                    Ok(None) => self.note(tr!(
                                        "{name}'s server couldn't be read, so the rest were matched by Roblox on their own.",
                                        name = a.name
                                    )),
                                }
                            }
                            false
                        }
                    }
                    Err(e) => {
                        log::warn!("bulk batch {}: account {} failed: {e}", self.batch_id, a.id);
                        if !e.is_forbidden() {
                            self.set_stage(i, BulkStage::Failed { message: e.message() });
                            if let Some(why) = stop_reason(&e, &a.name) {
                                return Some(why);
                            }
                        }
                        e.is_forbidden()
                    }
                };
                if !refused {
                    pace.got_through();
                    break;
                }
                if pace.refused() {
                    self.set_stage(i, BulkStage::Failed { message: auth_failed() });
                    return Some(tr!("Roblox keeps refusing sign-ins (error 403). Wait a few minutes, or verify the account on roblox.com."));
                }
                self.set_stage(i, BulkStage::Queued);
                self.note(tr!("Roblox refused {name} (error 403). Trying it once more, with more time between launches.", name = a.name));
                if let Some(why) = self.wait(jittered(pace.delay(req.delay_ms)), || false).await {
                    return Some(why);
                }
            }
        }
        None
    }

    async fn close_refused(&self, i: usize, id: &str) -> Option<String> {
        self.update(|p| p.items[i].instance_id = None);
        let s = self.app.state::<App>();
        if let Some((pid, started)) = s.tracker.mark_closing(id) {
            match tauri::async_runtime::spawn_blocking(move || crate::instances::process::close(pid, started, true, Duration::ZERO)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => log::warn!("could not close refused client {pid}: {e}"),
                Err(e) => log::warn!("close of refused client {pid} failed: {e}"),
            }
        }
        self.wait(CLOSE_WAIT, || self.instance(id).is_none_or(|x| x.state == InstanceState::Exited)).await
    }

    async fn same_server(&self, req: &BulkRequest) -> (Option<(i64, String)>, Option<i64>) {
        let s = self.app.state::<App>();
        let first = &self.accounts[0];
        let resolved = match super::account_cookie(&s, first.id) {
            Ok(cookie) => super::resolve(&s.api, first.id, &cookie, &req.target).await.map(|(join, _)| (join, cookie)),
            Err(e) => Err(e),
        };
        let (place_id, cookie) = match resolved {
            Ok((Join::Game { place_id, .. }, cookie)) => (place_id, cookie),
            Ok(_) => return (None, None),
            Err(e) => {
                self.note(tr!("The target couldn't be checked ({why}), so each account joins on its own.", why = e.message()));
                return (None, None);
            }
        };
        if req.same_server == SameServer::FollowFirst {
            return (None, Some(place_id));
        }
        let n = self.accounts.len();
        let why = match pick_server(&s.api, first.id, &cookie, place_id, n).await {
            Ok(Some(job_id)) => return (Some((place_id, job_id)), None),
            Ok(None) => trn!(n, "No server had room for all {n} accounts", "No server had room for all {n} accounts"),
            Err(e) => tr!("The server list couldn't be read ({why})", why = e.message()),
        };
        self.note(tr!("{why}, so the rest follow the first account's server.", why = why));
        (None, Some(place_id))
    }

    async fn followed(
        &self,
        a: &Account,
        instance_id: &str,
        place_id: i64,
    ) -> std::result::Result<Option<(i64, String)>, String> {
        let known = |i: Instance| i.job_id.is_some() || matches!(i.state, InstanceState::Disconnected | InstanceState::Exited);
        if let Some(why) = self.wait(FOLLOW_WAIT, || self.instance(instance_id).is_none_or(known)).await {
            return Err(why);
        }
        let inst = self.instance(instance_id);
        if let Some(found) = inst.as_ref().and_then(|i| server_of(i.place_id, i.job_id.as_deref())) {
            return Ok(Some(found));
        }
        let s = self.app.state::<App>();
        let cookie = match super::account_cookie(&s, a.id) {
            Ok(cookie) => cookie,
            Err(e) => {
                log::warn!("bulk batch {}: cookie of account {} could not be read: {e}", self.batch_id, a.id);
                return Ok(None);
            }
        };
        let rows = s.api.presence(a.id, &cookie, &[a.user_id]).await.unwrap_or_default();
        Ok(rows.iter().find(|p| p.user_id == a.user_id).and_then(|p| presence_server(p, place_id)))
    }

    async fn wait(&self, max: Duration, done: impl Fn() -> bool) -> Option<String> {
        let end = Instant::now() + max;
        loop {
            if let Some(why) = self.mirror() {
                return Some(why);
            }
            if self.cancel.load(Ordering::SeqCst) {
                return Some(cancelled());
            }
            let left = end.saturating_duration_since(Instant::now());
            if done() || left.is_zero() {
                return None;
            }
            tokio::time::sleep(left.min(TICK)).await;
        }
    }

    fn mirror(&self) -> Option<String> {
        let s = self.app.state::<App>();
        let mut stop = None;
        self.update(|p| {
            for (item, a) in p.items.iter_mut().zip(&self.accounts) {
                match item.instance_id.as_deref().map(|id| s.tracker.get(id)) {
                    Some(Some(inst)) => {
                        let why = mirror_item(item, &inst, a);
                        stop = stop.take().or(why);
                    }
                    Some(None) => gone(item),
                    None => {}
                }
            }
        });
        stop
    }

    fn instance(&self, id: &str) -> Option<Instance> {
        self.app.state::<App>().tracker.get(id)
    }

    fn progress(&self) -> Option<BulkProgress> {
        self.app.state::<App>().bulk.current()
    }

    fn set_stage(&self, i: usize, stage: BulkStage) {
        self.update(|p| p.items[i].stage = stage);
    }

    fn note(&self, note: String) {
        log::info!("bulk batch {}: {note}", self.batch_id);
        self.update(|p| {
            p.note.get_or_insert(note);
        });
    }

    fn update(&self, f: impl FnOnce(&mut BulkProgress)) {
        let s = self.app.state::<App>();
        let changed = {
            let mut batch = s.bulk.batch();
            let Some(b) = batch.as_mut().filter(|b| b.progress.batch_id == self.batch_id) else { return };
            let before = b.progress.clone();
            f(&mut b.progress);
            (b.progress != before).then(|| b.progress.clone())
        };
        if let Some(p) = changed {
            let _ = self.app.emit(EVENT, p);
        }
    }
}

async fn pick_server(
    api: &crate::roblox_api::Client,
    account_id: i64,
    cookie: &crate::vault::Secret,
    place_id: i64,
    n: usize,
) -> Result<Option<String>> {
    let list = |order| api.servers_public(account_id, Some(cookie), place_id, order, true, 100, None);
    let page = match list(ServerOrder::Recommended).await {
        Err(AppError::Invalid(_)) => list(ServerOrder::OccupancyDesc).await?,
        r => r?,
    };
    Ok(choose_server(&page.servers, n))
}

fn new_progress(batch_id: String, req: &BulkRequest, accounts: &[Account]) -> BulkProgress {
    let queued =
        |a: &Account| BulkItem { account_id: a.id, user_id: a.user_id, stage: BulkStage::Queued, instance_id: None };
    BulkProgress {
        batch_id,
        target: req.target.clone(),
        same_server: req.same_server,
        delay_ms: req.delay_ms.clamp(MIN_DELAY, MAX_DELAY),
        job_id: None,
        items: accounts.iter().map(queued).collect(),
        stopped: None,
        done: false,
        note: None,
    }
}

pub(crate) fn name_of(a: &AccountSummary) -> String {
    [&a.alias, &a.display_name, &a.username].into_iter().find(|n| !n.trim().is_empty()).cloned().unwrap_or_default()
}

fn cant_launch(status: AccountStatus) -> Option<String> {
    match status {
        AccountStatus::Valid => None,
        AccountStatus::Invalid => Some(tr!("its cookie is no longer valid. Sign in to it again.")),
        AccountStatus::Challenged => Some(tr!("Roblox wants a human check. Sign in to it again.")),
        AccountStatus::Unknown => Some(tr!("its sign-in hasn't been checked yet. Check it first.")),
    }
}

fn stop_reason(e: &AppError, name: &str) -> Option<String> {
    match e {
        AppError::Challenge => {
            Some(tr!("Roblox wants a human check for {name}. Sign in to it again with Add account > Sign in with Roblox.", name = name))
        }
        AppError::Locked => Some(tr!("The vault locked. Unlock it.")),
        _ => None,
    }
}

fn mirror_item(item: &mut BulkItem, inst: &Instance, a: &Account) -> Option<String> {
    let name = &a.name;
    let why = match inst.note.as_deref() {
        Some(n) if n == auth_failed() => {
            item.stage = BulkStage::Failed { message: n.to_owned() };
            return None;
        }
        Some(n) if n == wrong_account() => tr!(
            "{name}'s client signed in as another account. Close that client and turn on Settings > Protect \
             each client's sign-in.",
            name = name
        ),
        Some(n) if n == challenged() => {
            tr!("Roblox wants a human check for {name}. Finish it in that client or close it.", name = name)
        }
        _ => {
            item.stage = stage_of(inst, a.allow_disconnect);
            return None;
        }
    };
    item.stage = BulkStage::Failed { message: inst.note.clone().unwrap_or_default() };
    Some(why)
}

fn gone(item: &mut BulkItem) {
    if !matches!(item.stage, BulkStage::Failed { .. }) {
        item.stage = BulkStage::Failed { message: crate::instances::closed() };
    }
}

fn presence_server(p: &UserPresence, place_id: i64) -> Option<(i64, String)> {
    const IN_GAME: i64 = 2;
    let same_game = p.place_id == Some(place_id) || p.root_place_id == Some(place_id);
    if p.user_presence_type == IN_GAME && same_game { server_of(p.place_id, p.game_id.as_deref()) } else { None }
}

fn server_of(place_id: Option<i64>, job_id: Option<&str>) -> Option<(i64, String)> {
    let job_id = job_id.filter(|j| is_uuid(j))?;
    Some((place_id.filter(|&p| p > 0)?, job_id.to_owned()))
}

fn target_for(given: &LaunchTarget, server: Option<&(i64, String)>) -> LaunchTarget {
    match server {
        Some((place_id, job_id)) => LaunchTarget::Server { place_id: *place_id, job_id: job_id.clone() },
        None => given.clone(),
    }
}

fn halt(p: &mut BulkProgress, why: String) {
    let mut cancelled = false;
    for item in &mut p.items {
        if matches!(item.stage, BulkStage::Queued | BulkStage::Ticket) {
            item.stage = BulkStage::Cancelled;
            cancelled = true;
        }
    }
    p.stopped = Some(match why.strip_suffix('.') {
        Some(s) if cancelled && why != self::cancelled() => tr!("{why}, then launch the accounts that were cancelled.", why = s),
        _ => why,
    });
}

fn settled(p: &BulkProgress) -> bool {
    p.items
        .iter()
        .filter(|i| i.instance_id.is_some())
        .all(|i| matches!(i.stage, BulkStage::InGame | BulkStage::Done | BulkStage::Failed { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WRONG_ACCOUNT: &str = "Signed in as another account";
    const CHALLENGED: &str = "Roblox wants a human check";

    fn guid(n: u8) -> String {
        format!("aaaaaaaa-0000-4000-8000-0000000000{n:02}")
    }

    fn server(id: &str, playing: i32, max_players: i32) -> GameServer {
        GameServer { id: id.into(), playing, max_players, fps: None, ping: None, friend_count: None }
    }

    fn instance(state: InstanceState, note: Option<&str>) -> Instance {
        Instance {
            id: "i1".into(),
            account_id: 1,
            pid: 100,
            state,
            target: LaunchTarget::Place { place_id: 1818, launch_data: None },
            place_id: Some(1818),
            job_id: None,
            started_at: 0,
            ended_at: None,
            via_bootstrapper: false,
            note: note.map(Into::into),
            rejoins: 0,
            limits: Default::default(),
            anti_afk: true,
            muted: false,
            afk_at: None,
            throttled: false,
            micro: false,
            fps_cap: None,
            relaunching: false,
            relaunch_problem: None,
            sign_in_problem: false,
            applied_limits: Default::default(),
            checking_process: false,
            cpu_mode: Default::default(),
            cpu_warmup_remaining_s: 0,
        }
    }

    fn accounts(ids: &[i64]) -> Vec<Account> {
        ids.iter().map(|&id| Account { id, user_id: id * 1_000, name: format!("A{id}"), allow_disconnect: false }).collect()
    }

    fn item(stage: BulkStage, started: bool) -> BulkItem {
        BulkItem { account_id: 1, user_id: 1, stage, instance_id: started.then(|| "i1".into()) }
    }

    fn failed(message: &str) -> BulkStage {
        BulkStage::Failed { message: message.into() }
    }

    #[test]
    fn choose_server_prefers_the_fullest_with_a_spare_seat() {
        let (a, b, c, d) = (guid(1), guid(2), guid(3), guid(4));
        let list = [server(&c, 20, 35), server(&b, 33, 35), server(&d, 32, 35), server(&a, 30, 35)];
        assert_eq!(choose_server(&list, 3), Some(a.clone()));
        let exact = [server(&b, 33, 35), server(&d, 32, 35), server(&c, 31, 34)];
        assert_eq!(choose_server(&exact, 3), Some(d.clone()));
        assert_eq!(choose_server(&[server(&b, 33, 35), server(&a, 35, 35)], 3), None);
        assert_eq!(choose_server(&[], 1), None);
        assert_eq!(choose_server(&[server(&a, 30, 35), server(&c, 20, 35), server(&a, 30, 35)], 3), Some(a.clone()));
    }

    #[test]
    fn choose_server_skips_ids_that_are_not_guids() {
        let bad = ["not-a-guid", "", "aaaaaaaa-0000-4000-8000-00000000001", "aaaaaaaa_0000_4000_8000_000000000001"];
        let mut list: Vec<GameServer> = bad.iter().map(|id| server(id, 30, 35)).collect();
        assert_eq!(choose_server(&list, 2), None);
        list.push(server(&guid(9), 5, 35));
        assert_eq!(choose_server(&list, 2), Some(guid(9)));
        assert_eq!(choose_server(&[server(&guid(1), -5, 10)], 2), None);
    }

    #[test]
    fn jitter_stays_within_a_second_of_the_clamped_delay() {
        for _ in 0..500 {
            let ms = |d| jittered(d).as_millis();
            assert!((5_000..=7_000).contains(&ms(6_000)));
            assert!((1_000..=3_000).contains(&ms(0)));
            assert!((1_000..=3_000).contains(&ms(2_000)));
            assert!((59_000..=61_000).contains(&ms(u32::MAX)));
        }
        let spread: std::collections::HashSet<u128> = (0..50).map(|_| jittered(6_000).as_millis()).collect();
        assert!(spread.len() > 1, "jitter is random");
    }

    #[test]
    fn a_refusal_slows_every_launch_for_a_while() {
        let b = Bulk::new();
        let now = Instant::now();
        assert_eq!(b.launch_gap(6_000, now), Duration::from_secs(5), "as ever, less the jitter");
        assert_eq!(b.launch_gap(0, now), Duration::ZERO);
        b.refused();
        let now = Instant::now();
        assert_eq!(b.launch_gap(6_000, now), Duration::from_secs(11));
        b.refused();
        assert_eq!(b.launch_gap(6_000, now), Duration::from_secs(23));
        for _ in 0..30 {
            b.refused();
        }
        assert_eq!(b.launch_gap(6_000, now), Duration::from_secs(59), "never past the longest delay");
        assert_eq!(b.launch_gap(6_000, now + REFUSAL_LASTS + Duration::from_secs(1)), Duration::from_secs(5), "forgotten in time");
        b.got_through();
        assert_eq!(b.launch_gap(6_000, Instant::now()), Duration::from_secs(5), "over once a client gets through");
    }

    #[test]
    fn refusals_slow_the_batch_and_two_in_a_row_stop_it() {
        let mut pace = Pace::default();
        assert_eq!(pace.delay(6_000), 6_000);
        assert!(!pace.refused(), "tried once more");
        assert_eq!(pace.delay(6_000), 12_000);
        pace.got_through();
        assert!(!pace.refused(), "another account, after one got through");
        assert_eq!(pace.delay(6_000), 24_000, "still slower");
        assert!(pace.refused(), "twice in a row");
    }

    #[test]
    fn a_refused_client_fails_its_item_without_stopping_the_batch() {
        let a = accounts(&[1]).remove(0);
        let mut it = item(BulkStage::Joining, true);
        assert_eq!(mirror_item(&mut it, &instance(InstanceState::Joining, Some("Roblox refused the sign-in (403): verify this account on roblox.com")), &a), None);
        assert_eq!(it.stage, failed("Roblox refused the sign-in (403): verify this account on roblox.com"));
    }

    #[test]
    fn stages_mirror_the_instance() {
        use InstanceState::*;
        assert_eq!(stage_of(&instance(Starting, None), false), BulkStage::Starting);
        assert_eq!(stage_of(&instance(Joining, None), false), BulkStage::Joining);
        assert_eq!(stage_of(&instance(InGame, None), false), BulkStage::InGame);
        assert_eq!(stage_of(&instance(Disconnected, None), false), failed("Disconnected"));
        assert_eq!(stage_of(&instance(Exited, None), false), failed("Closed"));
        assert_eq!(stage_of(&instance(Exited, Some("Closed unexpectedly")), false), failed("Closed unexpectedly"));
        assert_eq!(stage_of(&instance(Disconnected, Some("Roblox wants a human check")), false), failed("Roblox wants a human check"));
    }

    #[test]
    fn a_disconnect_is_done_when_the_account_allows_it() {
        use InstanceState::*;
        assert_eq!(stage_of(&instance(Disconnected, None), true), BulkStage::Done);
        assert_eq!(stage_of(&instance(Disconnected, Some("Not responding")), true), BulkStage::Done);
        assert_eq!(stage_of(&instance(InGame, None), true), BulkStage::InGame);
        assert_eq!(stage_of(&instance(Exited, Some("Closed unexpectedly")), true), failed("Closed unexpectedly"));
        let mut a = accounts(&[1]).remove(0);
        a.allow_disconnect = true;
        let mut it = item(BulkStage::InGame, true);
        assert_eq!(mirror_item(&mut it, &instance(Disconnected, None), &a), None);
        assert_eq!(it.stage, BulkStage::Done);
        let req = BulkRequest { account_ids: vec![1], target: LaunchTarget::FollowUser { user_id: 1 }, delay_ms: 0, same_server: SameServer::Off, favourite_id: None, fps_cap: None };
        let mut p = new_progress("b".into(), &req, &[a]);
        p.items[0] = it;
        assert!(settled(&p));
    }

    #[test]
    fn launches_are_spaced_out_by_the_cooldown() {
        let b = Bulk::new();
        let (t0, gap) = (Instant::now(), Duration::from_secs(5));
        assert_eq!(b.reserve(t0, gap).0, Duration::ZERO, "the first goes at once");
        assert_eq!(b.reserve(t0 + Duration::from_secs(1), gap).0, Duration::from_secs(4));
        let (wait, slot) = b.reserve(t0 + Duration::from_secs(1), gap);
        assert_eq!(wait, Duration::from_secs(9));
        b.release(slot);
        assert_eq!(b.reserve(t0 + Duration::from_secs(1), gap).0, Duration::from_secs(9));
        let (_, slot) = b.reserve(t0 + Duration::from_secs(1), gap);
        b.reserve(t0 + Duration::from_secs(1), gap);
        b.release(slot);
        assert_eq!(b.reserve(t0 + Duration::from_secs(1), gap).0, Duration::from_secs(24));
        assert_eq!(b.reserve(t0 + Duration::from_secs(60), gap).0, Duration::ZERO);
        assert_eq!(Bulk::new().cooldown(gap).0, Duration::ZERO);
    }

    #[test]
    fn a_rejoin_holds_off_single_launches_and_waits_for_a_batch() {
        let b = Bulk::new();
        assert!(b.check_single_launch().is_ok() && !b.any_busy());
        let launch = b.begin_launch();
        assert!(b.any_busy() && b.check_single_launch().is_ok());
        assert!(matches!(b.begin_single_launch(), Err(AppError::Invalid(m)) if m == launching()));
        drop(launch);
        let single = b.begin_single_launch().unwrap();
        assert!(b.begin_single_launch().is_err());
        drop(single);
        let rejoin = b.begin_rejoin().unwrap();
        assert!(matches!(b.check_single_launch(), Err(AppError::Invalid(m)) if m == rejoining()));
        assert!(b.any_busy());
        drop(rejoin);
        assert!(b.check_single_launch().is_ok() && !b.any_busy());

        let req = BulkRequest { account_ids: vec![1], target: LaunchTarget::Place { place_id: 1, launch_data: None }, delay_ms: 0, same_server: SameServer::PickFirst, favourite_id: None, fps_cap: None };
        let progress = new_progress("b".into(), &req, &accounts(&[1]));
        *b.batch() = Some(Batch { progress, cancel: Arc::new(AtomicBool::new(false)) });
        assert!(b.begin_rejoin().is_none() && b.any_busy());
    }

    #[test]
    fn only_a_challenge_a_locked_vault_or_a_wrong_account_stops_the_batch() {
        let why = stop_reason(&AppError::Challenge, "Alt 1").unwrap();
        assert!(why.contains("Alt 1") && why.contains("Sign in"), "{why}");
        assert!(stop_reason(&AppError::Locked, "Alt 1").unwrap().contains("Unlock"));
        for e in [AppError::CookieInvalid, AppError::Network("x".into()), AppError::Roblox("HTTP 500".into())] {
            assert_eq!(stop_reason(&e, "Alt 1"), None, "{e}");
        }

        let alt = Account { id: 1, user_id: 1, name: "Alt 1".into(), allow_disconnect: true };
        let mut it = item(BulkStage::Starting, true);
        assert_eq!(mirror_item(&mut it, &instance(InstanceState::InGame, None), &alt), None);
        assert_eq!(it.stage, BulkStage::InGame);
        let why = mirror_item(&mut it, &instance(InstanceState::Joining, Some(WRONG_ACCOUNT)), &alt).unwrap();
        assert!(why.starts_with("Alt 1's client signed in as another account"), "{why}");
        assert_eq!(it.stage, failed(WRONG_ACCOUNT));
        let why = mirror_item(&mut it, &instance(InstanceState::Joining, Some(CHALLENGED)), &alt).unwrap();
        assert!(why.starts_with("Roblox wants a human check for Alt 1"), "{why}");
        assert_eq!(it.stage, failed(CHALLENGED));
        let replaced = "Replaced by another Roblox client";
        assert_eq!(mirror_item(&mut it, &instance(InstanceState::Joining, Some(replaced)), &alt), None);
        assert_eq!(it.stage, BulkStage::Joining);

        let mut it = item(BulkStage::Joining, true);
        gone(&mut it);
        assert_eq!(it.stage, failed("Closed"));
        let mut it = item(failed("Kicked"), true);
        gone(&mut it);
        assert_eq!(it.stage, failed("Kicked"));
    }

    #[test]
    fn presence_counts_only_in_game_in_the_same_game() {
        let row = |kind: i64, place: i64, root: i64| UserPresence {
            user_id: 1,
            user_presence_type: kind,
            last_location: None,
            place_id: Some(place),
            root_place_id: Some(root),
            game_id: Some(guid(1)),
            universe_id: None,
        };
        assert_eq!(presence_server(&row(2, 1818, 1818), 1818), Some((1818, guid(1))));
        assert_eq!(presence_server(&row(2, 77, 1818), 1818), Some((77, guid(1))));
        assert_eq!(presence_server(&row(2, 5, 5), 1818), None, "another game");
        assert_eq!(presence_server(&row(1, 1818, 1818), 1818), None, "online, not in game");
        assert_eq!(presence_server(&row(3, 1818, 1818), 1818), None, "in Studio");
    }

    #[test]
    fn halting_cancels_only_what_has_not_launched() {
        let req = BulkRequest {
            account_ids: vec![1, 2, 3, 4, 5],
            target: LaunchTarget::Place { place_id: 1818, launch_data: None },
            delay_ms: 6_000,
            same_server: SameServer::Off,
            favourite_id: None,
            fps_cap: None,
        };
        let mut p = new_progress("b".into(), &req, &accounts(&req.account_ids));
        assert_eq!(p.items[4].user_id, 5_000);
        p.items[0] = item(BulkStage::InGame, true);
        p.items[1] = item(failed("HTTP 500"), false);
        p.items[2] = item(BulkStage::Ticket, false);
        let fresh = p.clone();
        halt(&mut p, cancelled());
        let stages: Vec<_> = p.items.iter().map(|i| i.stage.clone()).collect();
        let cancelled = BulkStage::Cancelled;
        assert_eq!(stages, [BulkStage::InGame, failed("HTTP 500"), cancelled.clone(), cancelled.clone(), cancelled]);
        assert_eq!((p.stopped.as_deref(), p.done), (Some("Cancelled"), false));

        let mut stopped = fresh.clone();
        halt(&mut stopped, "The vault locked. Unlock it.".into());
        assert_eq!(stopped.stopped.as_deref(), Some("The vault locked. Unlock it, then launch the accounts that were cancelled."));
        let mut settled = fresh;
        settled.items.iter_mut().for_each(|i| i.stage = BulkStage::InGame);
        halt(&mut settled, "The vault locked. Unlock it.".into());
        assert_eq!(settled.stopped.as_deref(), Some("The vault locked. Unlock it."));
    }

    #[test]
    fn settled_once_every_started_client_is_in_game_or_ended() {
        let req = BulkRequest {
            account_ids: vec![1, 2],
            target: LaunchTarget::FollowUser { user_id: 1 },
            delay_ms: 500,
            same_server: SameServer::PickFirst,
            favourite_id: Some(3),
            fps_cap: None,
        };
        let accounts = accounts(&req.account_ids);
        let mut p = new_progress("b".into(), &req, &accounts);
        assert_eq!(p.delay_ms, 2_000);
        assert_eq!(new_progress("b".into(), &BulkRequest { delay_ms: 90_000, ..req.clone() }, &accounts).delay_ms, 60_000);
        assert!(p.items.iter().all(|i| i.stage == BulkStage::Queued && i.instance_id.is_none()));
        assert!(settled(&p), "nothing started");
        p.items = vec![item(BulkStage::InGame, true), item(BulkStage::Joining, true)];
        assert!(!settled(&p));
        p.items[1] = item(failed("Closed"), true);
        assert!(settled(&p));
        p.items[1] = item(failed("HTTP 500"), false);
        assert!(settled(&p));
    }

    #[test]
    fn the_rest_go_to_the_known_server_else_the_given_target() {
        let job = guid(7);
        assert_eq!(server_of(Some(1818), Some(&job)), Some((1818, job.clone())));
        assert_eq!(server_of(None, Some(&job)), None);
        assert_eq!(server_of(Some(0), Some(&job)), None);
        assert_eq!(server_of(Some(1818), None), None);
        assert_eq!(server_of(Some(1818), Some("1818' place 1")), None);

        let given = LaunchTarget::Place { place_id: 1818, launch_data: Some("x".into()) };
        assert_eq!(target_for(&given, None), given);
        let server = target_for(&given, Some(&(4483381587, job.clone())));
        assert_eq!(server, LaunchTarget::Server { place_id: 4483381587, job_id: job });
    }

    #[test]
    fn accounts_are_named_and_checked_like_the_ui() {
        let mut a = AccountSummary {
            id: 1,
            user_id: 2,
            username: "nova_alt01".into(),
            display_name: "Nova Alt".into(),
            alias: "Alt 01".into(),
            notes: String::new(),
            color: None,
            favourite: false,
            sort_order: 0,
            group_ids: vec![],
            status: AccountStatus::Valid,
            status_checked_at: None,
            created_at: 0,
            allow_disconnect: false,
            launch_args: None,
            discord_muted: false,
            full_quality: false,
            fps_cap: None,

        };
        assert_eq!(name_of(&a), "Alt 01");
        a.alias.clear();
        assert_eq!(name_of(&a), "Nova Alt");
        a.display_name = " ".into();
        assert_eq!(name_of(&a), "nova_alt01");
        assert_eq!(cant_launch(AccountStatus::Valid), None);
        for s in [AccountStatus::Invalid, AccountStatus::Challenged, AccountStatus::Unknown] {
            assert!(cant_launch(s).is_some(), "{s:?}");
        }
    }
}
