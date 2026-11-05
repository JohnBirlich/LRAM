use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::logwatch::{self, LogFacts};
use super::metrics::{MEM_STRIKES, mem_strikes, mem_throttled};
use super::process::{self, ProcInfo};
use crate::model::{CrashKind, Instance, InstanceState, LaunchInfo, LaunchTarget, Limits, Settings, CpuMode};

const REBIND_MS: i64 = 120_000;
const DEAD_POLLS: u32 = 2;
const CHECK_PROCESS_MS: i64 = 15_000;
const RECOVER_ENDED_MS: i64 = 180_000;
const INSTALL_WAIT_MS: i64 = 900_000;
const SLOW_START_MS: i64 = 600_000;
const STUCK_MS: i64 = 15 * 60 * 1000;
const STUCK_TRIES: u32 = 3;
const MAX_ENDED: usize = 30;
const TELEPORT_MS: i64 = 60_000;
pub fn wrong_account() -> String {
    tr!("Signed in as another account")
}
pub fn challenged() -> String {
    tr!("Roblox wants a human check")
}
pub fn auth_failed() -> String {
    tr!("Roblox refused the sign-in (403): verify this account on roblox.com")
}
pub fn unexpected() -> String {
    tr!("Closed unexpectedly")
}
pub fn hung() -> String {
    tr!("Not responding")
}
pub fn stuck() -> String {
    tr!("Stuck before reaching a game")
}
pub fn never_loaded() -> String {
    tr!("Gave up: never reached a game")
}
pub fn closed() -> String {
    tr!("Closed")
}
const HANG_POLLS: u32 = 30;
const HANG_FREE_POLLS: u32 = 5;
const WRITER_AFTER_MS: i64 = 2_000;
const WRITER_PROOF_MS: i64 = 150_000;

pub struct Tracker {
    inner: Mutex<Vec<Entry>>,
    claimed: Mutex<Vec<i64>>,
    crashes: Mutex<Vec<Ended>>,
    persisted: Mutex<Vec<Saved>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Saved {
    inst: Instance,
    btid: i64,
    start_time: u64,
    log: Option<PathBuf>,
    offset: u64,
    facts: LogFacts,
    in_game_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ended {
    pub instance_id: String,
    pub account_id: i64,
    pub kind: CrashKind,
    pub at: i64,
    pub uptime_ms: i64,
    pub reason: Option<String>,
    pub place_id: Option<i64>,
    pub log: Option<PathBuf>,
    pub pid: u32,
    pub started_at: i64,
    pub died_after_ms: Option<i64>,
    pub clues: String,
}

pub struct Claim<'a> {
    tracker: &'a Tracker,
    account_id: i64,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.tracker.claimed.lock().unwrap_or_else(|e| e.into_inner()).retain(|&a| a != self.account_id);
    }
}

#[derive(Clone)]
struct Entry {
    inst: Instance,
    btid: i64,
    start_time: Option<u64>,
    adopted: bool,
    alive: bool,
    log: Option<PathBuf>,
    offset: u64,
    facts: LogFacts,
    expected_user: Option<i64>,
    user_checked: bool,
    closing: bool,
    in_game_at: Option<i64>,
    hung_polls: u32,
    gave_up: Option<String>,
    relaunching: Option<i64>,
    gone_at: Option<i64>,
    died_at: Option<i64>,
    writer_at: Option<i64>,
    unreadable_said: bool,
    dead_polls: u32,
    rejoin_seen: bool,
    teleport_at: Option<i64>,
    loading_since: Option<i64>,
    server_seen: Option<String>,
    mem_strikes: u32,
    mem_mb: u64,
    mem_restart: bool,
    window_seen: bool,
    log_warned: bool,
    installer: bool,
    slow_start: bool,
    cpu_ready_at: Option<i64>,
    last_join_generation: u64,
    owner_checked_at: Option<i64>,
}

impl Entry {
    fn new(inst: Instance, btid: i64, start_time: Option<u64>, adopted: bool) -> Entry {
        Entry {
            inst,
            btid,
            start_time,
            adopted,
            alive: true,
            log: None,
            offset: 0,
            facts: LogFacts::default(),
            expected_user: None,
            user_checked: false,
            closing: false,
            in_game_at: None,
            hung_polls: 0,
            gave_up: None,
            relaunching: None,
            gone_at: None,
            died_at: None,
            writer_at: None,
            unreadable_said: false,
            dead_polls: 0,
            rejoin_seen: false,
            teleport_at: None,
            loading_since: None,
            server_seen: None,
            mem_strikes: 0,
            mem_mb: 0,
            mem_restart: false,
            window_seen: false,
            log_warned: false,
            installer: false,
            slow_start: false,
            cpu_ready_at: None,
            last_join_generation: 0,
            owner_checked_at: None,
        }
    }

    fn hung(&self) -> bool {
        self.hung_polls >= HANG_POLLS
    }

    fn stuck(&self) -> bool {
        self.alive
            && self.in_game_at.is_none()
            && self.inst.state != InstanceState::Exited
            && self.loading_since.is_some_and(|t| crate::commands::now_ms() - t >= STUCK_MS)
    }

    fn runs_free(&self, now: i64) -> bool {
        let may_take = if self.in_game_at.is_none() { REBIND_MS } else { TELEPORT_MS };
        self.hung_polls >= HANG_FREE_POLLS || self.loading_since.is_some_and(|t| now - t < may_take)
    }

    fn mem_high(&self) -> bool {
        self.mem_strikes >= MEM_STRIKES
    }

    fn stop_relaunch(&mut self) {
        self.relaunching = None;
        self.inst.relaunching = false;
        self.inst.relaunch_problem = None;
    }

    fn cause(&self) -> Option<CrashKind> {
        if self.inst.checking_process { return None; }
        wants_rejoin(&self.inst, &self.facts, self.hung(), self.stuck()).or_else(|| {
            (self.mem_high() && self.inst.state != InstanceState::Exited && self.inst.note.is_none()).then_some(CrashKind::Mem)
        })
    }
}

impl Tracker {
    pub fn new() -> Tracker {
        Tracker {
            inner: Mutex::new(Vec::new()),
            claimed: Mutex::new(Vec::new()),
            crashes: Mutex::new(Vec::new()),
            persisted: Mutex::new(Vec::new()),
        }
    }

    fn entries(&self) -> MutexGuard<'_, Vec<Entry>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn register(&self, info: &LaunchInfo, target: LaunchTarget) -> Instance {
        let inst = Instance {
            id: uuid::Uuid::new_v4().to_string(),
            account_id: info.account_id,
            pid: info.pid,
            state: InstanceState::Starting,
            target,
            place_id: info.place_id,
            job_id: None,
            started_at: info.started_at,
            ended_at: None,
            via_bootstrapper: info.via_bootstrapper,
            note: None,
            rejoins: 0,
            limits: Default::default(),
            anti_afk: true,
            muted: false,
            afk_at: None,
            throttled: false,
            micro: false,
            fps_cap: info.fps_cap,
            relaunching: false,
            relaunch_problem: None,
            sign_in_problem: false,
            applied_limits: Limits::default(),
            checking_process: false,
            cpu_mode: Default::default(),
            cpu_warmup_remaining_s: 0,
            performance: None,
            performance_pending: false,
        };
        let mut entries = self.entries();
        for e in entries.iter_mut().filter(|e| e.inst.account_id == info.account_id) {
            e.inst.relaunch_problem = None;
        }
        entries.push(Entry::new(inst.clone(), info.browser_tracker_id, info.pid_start, !info.via_bootstrapper));
        inst
    }

    pub fn saved(&self) -> Vec<Saved> {
        self.entries()
            .iter()
            .filter(|e| e.inst.state != InstanceState::Exited)
            .filter_map(|e| {
                let (_, start_time) = client_of(e)?;
                let (log, facts) = (e.log.clone(), e.facts.clone());
                Some(Saved { inst: e.inst.clone(), btid: e.btid, start_time, log, offset: e.offset, facts, in_game_at: e.in_game_at })
            })
            .collect()
    }

    pub fn persist(&self, db: &crate::storage::Db, due: bool) {
        let now = self.saved();
        let mut last = self.persisted.lock().unwrap_or_else(|e| e.into_inner());
        let moved = now.len() != last.len() || now.iter().zip(last.iter()).any(|(a, b)| a.inst != b.inst);
        if *last == now || !(moved || due) {
            return;
        }
        let rows: Vec<(String, String)> =
            now.iter()
                .filter_map(|c| match serde_json::to_string(c) {
                    Ok(j) => Some((c.inst.id.clone(), j)),
                    Err(e) => {
                        log::warn!("tracked instance {} not saved: {:?} error", c.inst.id, e.classify());
                        None
                    }
                })
                .collect();
        match db.save_tracked(&rows) {
            Ok(()) => *last = now,
            Err(e) => log::warn!("could not save the tracked clients: {e}"),
        }
    }

    pub fn readopt(&self, saved: Vec<Saved>, roblox: &[ProcInfo]) -> Vec<(u32, u64)> {
        let mut entries = self.entries();
        let mut out = Vec::new();
        for s in saved {
            if !readoptable(&s, roblox, &entries) {
                continue;
            }
            out.push((s.inst.pid, s.start_time));
            let mut e = Entry::new(s.inst, s.btid, Some(s.start_time), true);
            (e.log, e.offset, e.facts, e.in_game_at) = (s.log, s.offset, s.facts, s.in_game_at);
            e.window_seen = true;
            entries.push(e);
        }
        out
    }

    pub fn list(&self) -> Vec<Instance> {
        let mut v: Vec<Instance> = self.entries().iter().map(|e| e.inst.clone()).collect();
        v.sort_by_key(|i| match i.state {
            InstanceState::Exited => (1, -i.ended_at.unwrap_or(0)),
            _ => (0, i.started_at),
        });
        v
    }

    pub fn get(&self, id: &str) -> Option<Instance> {
        self.entries().iter().find(|e| e.inst.id == id).map(|e| e.inst.clone())
    }

    pub fn mark_closing(&self, id: &str) -> Option<(u32, Option<u64>)> {
        self.mark_restart(id, None)
    }

    pub fn mark_restart(&self, id: &str, cause: Option<CrashKind>) -> Option<(u32, Option<u64>)> {
        let mut entries = self.entries();
        let e = entries.iter_mut().find(|e| e.inst.id == id && e.inst.state != InstanceState::Exited)?;
        e.closing = true;
        e.mem_restart = cause == Some(CrashKind::Mem);
        e.alive.then_some((e.inst.pid, e.start_time))
    }

    pub fn bootstrappers(&self) -> Vec<(u32, Option<u64>)> {
        self.entries().iter().filter(|e| !e.adopted && e.alive && e.inst.state != InstanceState::Exited).map(|e| (e.inst.pid, e.start_time)).collect()
    }

    pub fn mark_all_closing(&self) {
        self.entries().iter_mut().for_each(|e| e.closing = true);
    }

    pub fn give_up(&self, id: &str, note: String) {
        if let Some(e) = self.entries().iter_mut().find(|e| e.inst.id == id) {
            if !e.closing {
                e.inst.note = Some(note.clone());
            }
            e.gave_up = Some(note);
        }
    }

    pub fn take_servers(&self) -> Vec<(i64, crate::model::ServerVisit)> {
        let mut out = Vec::new();
        for e in self.entries().iter_mut().filter(|e| e.facts.in_game && !e.facts.teleporting) {
            if let (Some(job), Some(place)) = (&e.facts.job_id, e.facts.place_id)
                && e.server_seen.as_ref() != Some(job)
            {
                let private = e.server_seen.is_some() || matches!(e.inst.target, LaunchTarget::PrivateLink { .. } | LaunchTarget::PrivateAccess { .. });
                out.push((e.inst.account_id, crate::model::ServerVisit { place_id: place, job_id: job.clone(), private, joined_at: crate::commands::now_ms() }));
                e.server_seen = Some(job.clone());
            }
        }
        out
    }

    pub fn take_crashes(&self) -> Vec<Ended> {
        std::mem::take(&mut *self.crashes.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn clear_closing(&self, id: &str) {
        if let Some(e) = self.entries().iter_mut().find(|e| e.inst.id == id) {
            e.closing = false;
            e.mem_restart = false;
        }
    }

    pub fn has_live_client(&self, account_id: i64) -> bool {
        self.entries().iter().any(|e| {
            e.inst.account_id == account_id
                && (e.inst.checking_process || matches!(e.inst.state, InstanceState::Starting | InstanceState::Joining | InstanceState::InGame))
        })
    }

    pub fn claim(&self, account_id: i64) -> Option<Claim<'_>> {
        let mut claimed = self.claimed.lock().unwrap_or_else(|e| e.into_inner());
        if claimed.contains(&account_id) || self.has_live_client(account_id) {
            return None;
        }
        claimed.push(account_id);
        Some(Claim { tracker: self, account_id })
    }

    pub fn live_logs(&self) -> Vec<PathBuf> {
        self.entries().iter().filter(|e| e.inst.state != InstanceState::Exited).filter_map(|e| e.log.clone()).collect()
    }

    pub fn any_running(&self) -> bool {
        self.entries().iter().any(|e| e.inst.state != InstanceState::Exited)
    }

    pub fn set_rejoins(&self, id: &str, rejoins: u32) {
        if let Some(e) = self.entries().iter_mut().find(|e| e.inst.id == id) {
            e.inst.rejoins = rejoins;
        }
    }

    pub fn set_limits(&self, id: &str, limits: Limits) -> Option<(Limits, Option<(u32, u64)>)> {
        let mut entries = self.entries();
        let e = entries.iter_mut().find(|e| e.inst.id == id && e.inst.state != InstanceState::Exited)?;
        let mut limits = limits;
        if let Some(p) = e.inst.performance.as_mut() {
            if p.strict_cpu { limits.cpu_cap = Some(limits.cpu_cap.unwrap_or(1).clamp(1, 2)); }
            p.limits = limits;
        }
        let prev = std::mem::replace(&mut e.inst.limits, limits);
        Some((prev, client_of(e)))
    }

    pub fn set_anti_afk(&self, id: &str, on: bool) -> bool {
        let mut entries = self.entries();
        let Some(e) = entries.iter_mut().find(|e| e.inst.id == id && e.inst.state != InstanceState::Exited) else { return false };
        e.inst.anti_afk = on;
        true
    }

    pub fn set_muted(&self, id: &str, on: bool) -> bool {
        let mut entries = self.entries();
        let Some(e) = entries.iter_mut().find(|e| e.inst.id == id && e.inst.state != InstanceState::Exited) else {
            return false;
        };
        e.inst.muted = on;
        true
    }

    pub fn set_all_muted(&self, on: bool) -> usize {
        let mut entries = self.entries();
        let mut n = 0;
        for e in entries.iter_mut().filter(|e| e.inst.state != InstanceState::Exited) {
            e.inst.muted = on;
            n += 1;
        }
        n
    }

    pub fn mute_targets(&self) -> Vec<super::audio::MuteTarget> {
        self.entries()
            .iter()
            .filter(|e| e.inst.state != InstanceState::Exited && !e.closing)
            .filter_map(|e| {
                let (pid, _) = client_of(e)?;
                Some(super::audio::MuteTarget { pid, muted: e.inst.muted })
            })
            .collect()
    }

    pub fn afk_clients(&self) -> Vec<super::afk::AfkClient> {
        self.entries()
            .iter()
            .filter(|e| e.inst.state == InstanceState::InGame && e.inst.anti_afk && !e.closing)
            .filter_map(|e| {
                let (pid, _) = client_of(e)?;
                Some(super::afk::AfkClient { id: e.inst.id.clone(), pid, since: e.inst.afk_at.or(e.in_game_at)? })
            })
            .collect()
    }

    pub fn set_micro(&self, id: &str, on: bool) -> bool {
        let mut entries = self.entries();
        let Some(e) = entries.iter_mut().find(|e| e.inst.id == id && e.inst.state != InstanceState::Exited) else { return false };
        e.inst.micro = on;
        true
    }

    pub fn first_in_game(&self, has_window: &dyn Fn(u32) -> bool) -> Vec<u32> {
        let waiting: Vec<(String, u32)> = self
            .entries()
            .iter()
            .filter(|e| !e.window_seen && e.inst.state == InstanceState::InGame)
            .filter_map(|e| client_of(e).map(|(pid, _)| (e.inst.id.clone(), pid)))
            .collect();
        let seen: Vec<(String, u32)> = waiting.into_iter().filter(|&(_, pid)| has_window(pid)).collect();
        let mut entries = self.entries();
        seen.into_iter()
            .filter(|(id, pid)| {
                let Some(e) = entries.iter_mut().find(|e| e.inst.id == *id && e.inst.pid == *pid && !e.window_seen) else { return false };
                e.window_seen = true;
                true
            })
            .map(|(_, pid)| pid)
            .collect()
    }

    pub fn mark_afk(&self, id: &str, at: i64) {
        if let Some(e) = self.entries().iter_mut().find(|e| e.inst.id == id) {
            e.inst.afk_at = Some(at);
        }
    }

    pub fn clients(&self) -> Vec<(String, u32, u64, Limits)> {
        self.entries()
            .iter()
            .filter(|e| e.inst.state != InstanceState::Exited)
            .filter_map(|e| client_of(e).map(|(pid, t0)| (e.inst.id.clone(), pid, t0, e.inst.limits)))
            .collect()
    }

    pub fn free(&self, full_quality: &[i64], now: i64) -> Vec<String> {
        self.entries()
            .iter()
            .filter(|e| e.inst.state != InstanceState::Exited && (e.runs_free(now) || full_quality.contains(&e.inst.account_id)))
            .map(|e| e.inst.id.clone())
            .collect()
    }

    pub fn set_performance(&self, id: &str, p: crate::model::ClientPerformance, pending: bool) {
        if let Some(e) = self.entries().iter_mut().find(|e| e.inst.id == id && e.inst.state != InstanceState::Exited) {
            let changed = e.inst.performance.as_ref().is_none_or(|old| old.fps_cap != p.fps_cap || old.low_graphics != p.low_graphics || old.lighter_rendering != p.lighter_rendering || old.render_flags != p.render_flags);
            e.inst.performance_pending = pending && (changed || e.inst.performance_pending);
            e.inst.limits = p.limits;
            e.inst.performance = Some(p);
        }
    }

    pub fn cpu_policy(&self, v: &Settings, _foreground: Option<u32>, _full_quality: &[i64], _now: i64) -> (Vec<String>, bool) {
        let mut free = Vec::new();
        let mut changed = false;
        for e in self.entries().iter_mut().filter(|e| e.inst.state != InstanceState::Exited) {
            let own = e.inst.performance.as_ref();
            let strict = own.map_or(v.ultra_cpu.enabled, |p| p.strict_cpu);
            let (mode, remaining, released) = if e.inst.checking_process {
                (CpuMode::Checking, 0, false)
            } else if strict {
                (CpuMode::Background, 0, false)
            } else {
                (CpuMode::Standard, 0, false)
            };
            if released { free.push(e.inst.id.clone()); }
            if e.inst.cpu_mode != mode {
                let cap = own.and_then(|p| p.limits.cpu_cap).unwrap_or(v.ultra_cpu.cap);
                log::info!("instance {} PID {}: CPU mode {:?} -> {:?}; selected ceiling {}%, strict {}", e.inst.id, e.inst.pid, e.inst.cpu_mode, mode, cap, strict);
            }
            changed |= e.inst.cpu_mode != mode || e.inst.cpu_warmup_remaining_s != remaining;
            e.inst.cpu_mode = mode;
            e.inst.cpu_warmup_remaining_s = remaining;
        }
        (free, changed)
    }

    pub fn still_wants_rejoin(&self, id: &str) -> bool {
        self.entries().iter().any(|e| e.inst.id == id && !e.closing && e.cause().is_some())
    }

    pub fn relaunch_wanted(&self, id: &str, closed: bool) -> bool {
        let entries = self.entries();
        let Some(me) = entries.iter().find(|e| e.inst.id == id) else { return false };
        let Some(began) = me.relaunching else { return false };
        !me.inst.checking_process && (closed || (!me.closing && me.cause().is_some()))
            && !entries.iter().any(|e| e.inst.id != id && e.inst.account_id == me.inst.account_id && e.inst.started_at > began)
    }

    pub fn begin_relaunch(&self, id: &str, now: i64) -> bool {
        self.entries().iter_mut().find(|e| e.inst.id == id && !e.inst.checking_process).is_some_and(|e| {
            e.inst.relaunching = true;
            e.relaunching.replace(e.relaunching.unwrap_or(now)).is_none()
        })
    }

    pub fn set_relaunch_problem(&self, id: &str, why: Option<String>) {
        if let Some(e) = self.entries().iter_mut().find(|e| e.inst.id == id && e.relaunching.is_some()) {
            e.inst.relaunch_problem = why;
        }
    }

    pub fn is_relaunching(&self, id: &str) -> bool {
        self.entries().iter().any(|e| e.inst.id == id && e.relaunching.is_some())
    }

    pub fn end_relaunch(&self, id: &str) {
        if let Some(e) = self.entries().iter_mut().find(|e| e.inst.id == id) {
            e.stop_relaunch();
        }
    }

    pub fn cause_of(&self, id: &str) -> Option<CrashKind> {
        self.entries().iter().find(|e| e.inst.id == id).and_then(Entry::cause)
    }

    pub fn cancel_relaunches(&self) {
        for e in self.entries().iter_mut() {
            e.stop_relaunch();
            e.closing |= e.inst.state == InstanceState::Exited;
        }
    }

    pub fn relaunches_open(&self) -> usize {
        self.entries().iter().filter(|e| e.relaunching.is_some() && !e.closing && e.inst.state != InstanceState::Exited && e.cause().is_some()).count()
    }

    pub fn take_rejoins(&self) -> Vec<Rejoin> {
        let mut out = Vec::new();
        for e in self.entries().iter_mut() {
            if e.inst.checking_process { continue; }
            let trouble = e.hung() || e.stuck() || e.mem_high() || matches!(e.inst.state, InstanceState::Disconnected | InstanceState::Exited);
            if !trouble {
                e.rejoin_seen = false;
                e.gave_up = None;
                e.inst.relaunch_problem = None;
            } else if !e.rejoin_seen {
                e.rejoin_seen = true;
                if let Some(cause) = e.cause() {
                    if e.stuck() && e.inst.rejoins >= STUCK_TRIES {
                        log::warn!("instance {}: still not in a game after {} restarts: giving up", e.inst.id, e.inst.rejoins);
                        e.inst.note = Some(never_loaded());
                        e.gave_up = Some(never_loaded());
                    } else {
                        if e.stuck() {
                            log::warn!("instance {}: alive but out of a game for {} min: restarting it", e.inst.id, STUCK_MS / 60_000);
                        }
                        let i = &e.inst;
                        out.push(Rejoin {
                            from: i.id.clone(),
                            account_id: i.account_id,
                            target: i.target.clone(),
                            rejoins: i.rejoins + 1,
                            cause,
                        });
                    }
                }
            }
        }
        out
    }

    pub fn set_in_force(&self, in_force: &[(String, Limits)]) -> bool {
        let mut changed = false;
        for e in self.entries().iter_mut() {
            let now = in_force.iter().find(|(id, _)| *id == e.inst.id).map_or_else(Limits::default, |(_, l)| *l);
            changed |= now != e.inst.applied_limits;
            e.inst.applied_limits = now;
        }
        changed
    }

    pub fn check_memory(&self, limit_mb: u32, count: bool, private_mb: &dyn Fn(u32, u64) -> Option<u64>) {
        let read: Vec<(String, u32, u64)> = self
            .clients()
            .into_iter()
            .filter_map(|(id, pid, t0, _)| Some((id, pid, private_mb(pid, t0)?)))
            .collect();
        for e in self.entries().iter_mut().filter(|e| !e.closing) {
            match read.iter().find(|(id, pid, _)| *id == e.inst.id && *pid == e.inst.pid) {
                Some(&(_, _, mb)) => {
                    if count {
                        e.mem_strikes = mem_strikes(e.mem_strikes, mb, limit_mb);
                    }
                    e.mem_mb = mb;
                }
                None if count && e.inst.state != InstanceState::Exited => e.mem_strikes = 0,
                None => {}
            }
        }
    }

    pub fn throttle(&self, limit_mb: u32) -> (Vec<String>, bool) {
        let (mut held, mut changed) = (Vec::new(), false);
        for e in self.entries().iter_mut().filter(|e| e.inst.state != InstanceState::Exited) {
            let t = mem_throttled(e.inst.throttled, e.mem_mb, limit_mb);
            changed |= t != e.inst.throttled;
            e.inst.throttled = t;
            if t {
                held.push(e.inst.id.clone());
            }
        }
        (held, changed)
    }

    pub fn clear_ended(&self) {
        self.entries().retain(|e| e.inst.state != InstanceState::Exited);
    }

    pub fn spawn(app: AppHandle) {
        restore(&app);
        super::resize::start();
        let followed: Vec<u32> = app.state::<crate::App>().tracker.clients().into_iter().map(|c| c.1).collect();
        std::thread::spawn(move || super::window::recover(&followed));
        tauri::async_runtime::spawn(async move {
            let mut tick: u64 = 0;
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                tick += 1;
                let handle = app.clone();
                let (moved, rejoins, throttle_moved) = tauri::async_runtime::spawn_blocking(move || {
                    let s = handle.state::<crate::App>();
                    static SETTINGS_BAD: AtomicBool = AtomicBool::new(false);
                    let v = match s.db.settings() {
                        Ok(v) => {
                            SETTINGS_BAD.store(false, Ordering::Relaxed);
                            v
                        }
                        Err(e) => {
                            if !SETTINGS_BAD.swap(true, Ordering::Relaxed) {
                                log::warn!("tracker: could not read settings, using defaults: {e}");
                            }
                            crate::model::Settings::default()
                        }
                    };
                    let user_of = |account_id| {
                        s.db.account(account_id)
                            .inspect_err(|e| {
                                if !matches!(e, crate::error::AppError::NotFound) {
                                    log::debug!("tracker: could not read account {account_id}, wrong-account check off: {e}");
                                }
                            })
                            .ok()
                            .map(|a| a.user_id)
                    };
                    let dir = logwatch::logs_dir();
                    if dir.is_none() {
                        static ONCE: std::sync::Once = std::sync::Once::new();
                        ONCE.call_once(|| log::warn!("LOCALAPPDATA not set: Roblox client logs can't be followed"));
                    }
                    let moved = s.tracker.poll(dir.as_deref(), &Live, &user_of, crate::commands::now_ms());
                    s.multi.release_if(|| !s.tracker.any_running());
                    let clients = s.tracker.clients();
                    super::resize::set_clients(clients.iter().map(|c| c.1).collect());
                    let (held, throttle_moved) = s.tracker.throttle(v.mem_throttle.mb);
                    let full_quality = s.db.full_quality_accounts().unwrap_or_default();
                    let (_, cpu_changed) = s.tracker.cpu_policy(&v, super::process::foreground_pid(), &full_quality, crate::commands::now_ms());
                    if !clients.is_empty() {
                        s.limiter.sync(&super::limits::plan_now(&s, &v, &clients, &held));
                    }
                    let throttle_moved = s.tracker.set_in_force(&s.limiter.in_force()) || throttle_moved || cpu_changed;
                    super::window::set_live_hidden_previews(v.live_hidden_previews);
                    let mut arrange = false;
                    let mut tiny: Vec<(String, u32)> = Vec::new();
                    let mut hide: Vec<u32> = Vec::new();
                    for pid in s.tracker.first_in_game(&|pid| process::main_window(pid).is_some()) {
                        let id = clients.iter().find(|c| c.1 == pid).map(|c| c.0.clone());
                        let instance = id.as_deref().and_then(|id| s.tracker.get(id));
                        let full_quality = instance.as_ref().and_then(|i| s.db.account(i.account_id).ok()).is_some_and(|a| a.full_quality);
                        let launched = instance.is_none_or(|i| i.rejoins == 0);
                        let make_tiny = id.filter(|id| v.tiny_on_launch && !full_quality && s.tracker.set_micro(id, true));
                        let minimize = v.minimize_on_launch && !full_quality;
                        log::info!("client {pid}: in a game; making it tiny: {}, hiding it: {minimize}", make_tiny.is_some());
                        arrange |= arranges(v.auto_tile, launched);
                        if let Some(id) = make_tiny {
                            tiny.push((id, pid));
                        }
                        if minimize {
                            hide.push(pid);
                        }
                    }
                    if arrange || !tiny.is_empty() || !hide.is_empty() {
                        let list: Vec<(String, u32)> = clients.iter().map(|c| (c.0.clone(), c.1)).collect();
                        let win = handle.get_webview_window("main").and_then(|w| w.hwnd().ok()).map_or(0, |h| h.0 as isize);
                        let size = v.tiny_on_launch.then_some((v.micro_width, v.micro_height));
                        let micro = (v.micro_width, v.micro_height);
                        let arranged = handle.clone();
                        tauri::async_runtime::spawn_blocking(move || {
                            for (id, pid) in &tiny {
                                if let Err(e) = super::window::micro(id, *pid, true, micro) {
                                    log::debug!("instance {id}: not made tiny at launch: {e}");
                                }
                            }
                            if arrange {
                                let tiled = super::window::tile(&list, windows::Win32::Foundation::HWND(win as *mut _), size);
                                let s = arranged.state::<crate::App>();
                                for id in &tiled {
                                    s.tracker.set_micro(id, size.is_some());
                                }
                                log::info!("arranged {} client window(s) in a grid", tiled.len());
                                crate::commands::instances_changed(&arranged);
                            }
                            for pid in hide {
                                super::window::hide_on_launch(pid);
                            }
                        });
                    }
                    static SWEEPING: AtomicBool = AtomicBool::new(false);
                    if !clients.is_empty() && !SWEEPING.swap(true, Ordering::Relaxed) {
                        let size = (v.micro_width, v.micro_height);
                        let list: Vec<_> = clients.iter().map(|c| (c.1, s.tracker.get(&c.0).is_some_and(|i| i.micro).then_some(size))).collect();
                        tauri::async_runtime::spawn_blocking(move || {
                            super::window::sweep(&list);
                            SWEEPING.store(false, Ordering::Relaxed);
                        });
                    }
                    if tick.is_multiple_of(10) && !clients.is_empty() {
                        let mb = |pid, t0| super::metrics::read(pid, t0).map(|r| r.private >> 20);
                        s.tracker.check_memory(v.mem_restart_mb, !s.bulk.any_busy(), &mb);
                    }
                    if v.clean_logs
                        && tick % 3600 == 30
                        && let Some(dir) = logwatch::logs_dir()
                    {
                        let done = logwatch::clean(&dir, &s.tracker.live_logs());
                        log::info!("cleaned {} Roblox logs ({} MB)", done.files, done.bytes >> 20);
                    }
                    for (account, visit) in s.tracker.take_servers() {
                        if let Err(e) = s.db.add_server(account, &visit) {
                            log::warn!("could not record a server of account {account}: {e}");
                        }
                    }
                    for c in s.tracker.take_crashes() {
                        log::warn!(
                            "instance {} of account {} ended: {:?}, given up on after {} s (gone after {:?} ms), place {:?}, reason {:?}",
                            c.instance_id,
                            c.account_id,
                            c.kind,
                            c.uptime_ms / 1000,
                            c.died_after_ms,
                            c.place_id,
                            c.reason
                        );
                        let row = crate::storage::NewCrash {
                            instance_id: &c.instance_id,
                            account_id: c.account_id,
                            kind: c.kind,
                            at: c.at,
                            uptime_ms: c.uptime_ms,
                            reason: c.reason.as_deref(),
                            place_id: c.place_id,
                            log_path: c.log.as_ref().and_then(|p| p.to_str()),
                        };
                        if let Err(e) = s.db.add_crash(&row) {
                            log::warn!("could not record the end of instance {}: {e}", c.instance_id);
                        }
                        if c.kind != CrashKind::Mem {
                            std::thread::spawn(move || super::autopsy::report(&c));
                        }
                    }
                    s.tracker.persist(&s.db, tick.is_multiple_of(10));
                    (moved, s.tracker.take_rejoins(), throttle_moved)
                })
                .await
                .unwrap_or_else(|e| {
                    log::error!("tracker poll failed: {e}");
                    Default::default()
                });
                for t in &moved {
                    let (b, a) = (&t.before, &t.after);
                    if b.state != a.state || b.note != a.note {
                        let problem = a.state == InstanceState::Disconnected || a.note.as_ref().is_some_and(|n| *n != closed());
                        let level = if problem { log::Level::Warn } else { log::Level::Info };
                        let note = a.note.as_deref().unwrap_or("");
                        log::log!(level, "instance {} of account {}: {:?} -> {:?} {note}", a.id, a.account_id, b.state, a.state);
                        let bulk = &app.state::<crate::App>().bulk;
                        if a.note != b.note && a.note.as_ref().is_some_and(|n| *n == auth_failed()) {
                            bulk.refused();
                        } else if a.state == InstanceState::InGame && b.state != InstanceState::InGame {
                            bulk.got_through();
                        }
                    }
                }
                if !moved.is_empty() || throttle_moved {
                    crate::commands::instances_changed(&app);
                    crate::discord::transitions(&app, &moved);
                }
                super::rejoin::handle(&app, rejoins);
            }
        });
    }

    fn poll(&self, dir: Option<&Path>, procs: &dyn Procs, user_of: &dyn Fn(i64) -> Option<i64>, now: i64) -> Vec<Transition> {
        let snapshot = self.entries().clone();
        let active_accounts: Vec<i64> = snapshot.iter().filter(|e| e.inst.state != InstanceState::Exited).map(|e| e.inst.account_id).collect();
        let mut taken: Vec<u32> = snapshot.iter().filter(|e| e.inst.state != InstanceState::Exited).map(|e| e.inst.pid).collect();
        let mut work = Vec::new();
        for mut e in snapshot {
            if e.inst.state == InstanceState::Exited {
                if e.closing || e.relaunching.is_some() || active_accounts.contains(&e.inst.account_id)
                    || e.inst.note.as_deref() != Some(&*unexpected())
                    || !e.inst.ended_at.is_some_and(|at| now - at <= RECOVER_ENDED_MS) {
                    continue;
                }
                if !recover_ended(&mut e, procs, &mut taken, now) {
                    if let Some(old) = self.entries().iter_mut().find(|old| old.inst.id == e.inst.id) {
                        old.owner_checked_at = e.owner_checked_at;
                    }
                    continue;
                }
            }
            work.push(e);
        }
        if work.is_empty() {
            return Vec::new();
        }
        let mut roblox = None;
        let mut installer = None;
        for e in &mut work {
            if !e.user_checked {
                e.expected_user = user_of(e.inst.account_id);
                e.user_checked = true;
            }
            observe(e, dir, procs, &mut roblox, &mut installer, &mut taken, now);
        }
        // Match claim() lock order, so an in-flight/new launch cannot race a late recovery.
        let launching = self.claimed.lock().unwrap_or_else(|e| e.into_inner());
        let mut entries = self.entries();
        let mut moved = Vec::new();
        for w in work {
            let recovering = entries.iter().any(|e| e.inst.id == w.inst.id && e.inst.state == InstanceState::Exited);
            if recovering && (entries.iter().any(|e| e.inst.id == w.inst.id && (e.closing || e.relaunching.is_some()))
                || launching.contains(&w.inst.account_id)
                || entries.iter().any(|e| e.inst.id != w.inst.id && e.inst.account_id == w.inst.account_id && e.inst.state != InstanceState::Exited)) {
                continue;
            }
            let Some(e) = entries.iter_mut().find(|e| e.inst.id == w.inst.id) else { continue };
            let closing = e.closing && (e.adopted || !w.adopted);
            let old = std::mem::replace(e, w);
            e.closing = closing;
            e.inst.rejoins = old.inst.rejoins;
            (e.inst.limits, e.inst.applied_limits) = (old.inst.limits, old.inst.applied_limits);
            (e.inst.performance, e.inst.performance_pending) = (old.inst.performance.clone(), old.inst.performance_pending);
            (e.inst.cpu_mode, e.inst.cpu_warmup_remaining_s) = (old.inst.cpu_mode, old.inst.cpu_warmup_remaining_s);
            (e.inst.anti_afk, e.inst.afk_at) = (old.inst.anti_afk, old.inst.afk_at);
            e.inst.muted = old.inst.muted;
            let reattached = old.inst.state == InstanceState::Exited || old.inst.pid != e.inst.pid;
            (e.inst.throttled, e.inst.micro, e.window_seen) = (old.inst.throttled, old.inst.micro && !reattached, old.window_seen && !reattached);
            (e.gave_up, e.relaunching) = (old.gave_up, old.relaunching);
            (e.inst.relaunching, e.inst.relaunch_problem) = (old.inst.relaunching, old.inst.relaunch_problem.clone());
            let had_game = old.in_game_at.is_some();
            (e.mem_strikes, e.mem_mb, e.mem_restart) = (old.mem_strikes, old.mem_mb, old.mem_restart);
            if let Some(kind) = settle(e, now) {
                let record = ended(e, kind, now);
                log::warn!("instance {} ended as {kind:?} after {} ms: {}", record.instance_id, record.uptime_ms, record.clues);
                self.crashes.lock().unwrap_or_else(|e| e.into_inner()).push(record);
            }
            if e.inst != old.inst {
                moved.push(Transition { before: old.inst, after: e.inst.clone(), first_game: !had_game && e.in_game_at.is_some() });
            }
        }
        prune(&mut entries);
        moved
    }
}

fn restore(app: &AppHandle) {
    let s = app.state::<crate::App>();
    let saved: Vec<Saved> = match s.db.tracked() {
        Ok(rows) => rows
            .iter()
            .filter_map(|j| {
                serde_json::from_str(j)
                    .inspect_err(|e| {
                        log::warn!("a saved tracked client was dropped: {:?} error at {}:{}", e.classify(), e.line(), e.column())
                    })
                    .ok()
            })
            .collect(),
        Err(e) => return log::warn!("could not read the tracked clients: {e}"),
    };
    if saved.is_empty() {
        return;
    }
    *s.tracker.persisted.lock().unwrap_or_else(|e| e.into_inner()) = saved.clone();
    let total = saved.len();
    let roblox = with_unreadable(&saved, process::roblox_processes(), process::unreadable_client);
    let found = s.tracker.readopt(saved, &roblox);
    for &(pid, t0) in &found {
        super::limits::reset_followed(pid, t0);
    }
    s.multi.adopt(&found);
    log::info!("following {} client(s) from before LRAM restarted", found.len());
    if total > found.len() {
        log::debug!("{} saved client(s) no longer running, dropped", total - found.len());
    }
    s.tracker.persist(&s.db, true);
}

fn with_unreadable(saved: &[Saved], mut roblox: Vec<ProcInfo>, is_unreadable: impl Fn(u32) -> bool) -> Vec<ProcInfo> {
    for s in saved {
        if !roblox.iter().any(|p| p.pid == s.inst.pid) && is_unreadable(s.inst.pid) {
            log::warn!("instance {}: process {} can't be read but is still running: following it again", s.inst.id, s.inst.pid);
            roblox.push(ProcInfo { pid: s.inst.pid, start_time: s.start_time });
        }
    }
    roblox
}

fn readoptable(s: &Saved, roblox: &[ProcInfo], entries: &[Entry]) -> bool {
    roblox.iter().any(|p| p.pid == s.inst.pid && p.start_time == s.start_time)
        && !entries.iter().any(|e| e.inst.id == s.inst.id || (e.inst.pid == s.inst.pid && e.inst.state != InstanceState::Exited))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    pub before: Instance,
    pub after: Instance,
    pub first_game: bool,
}

fn client_of(e: &Entry) -> Option<(u32, u64)> {
    (e.adopted && e.alive).then_some((e.inst.pid, e.start_time?)).filter(|&(_, t0)| t0 != 0)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rejoin {
    pub from: String,
    pub account_id: i64,
    pub target: LaunchTarget,
    pub rejoins: u32,
    pub cause: CrashKind,
}

pub fn wants_rejoin(inst: &Instance, facts: &LogFacts, hung: bool, stuck: bool) -> Option<CrashKind> {
    if inst.checking_process || facts.left || facts.replaced {
        return None;
    }
    match inst.state {
        InstanceState::Exited if inst.note.as_deref() == Some(&*unexpected()) => {
            Some(if facts.disconnected { CrashKind::Disconnect } else { CrashKind::Crash })
        }
        InstanceState::Exited => None,
        _ if hung || stuck => {
            let ours = inst.note.as_deref().is_none_or(|n| n == self::hung() || n == self::stuck());
            ours.then_some(CrashKind::Hang)
        }
        InstanceState::Disconnected if inst.note.is_none() => Some(CrashKind::Disconnect),
        _ => None,
    }
}

fn crash_kind(facts: &LogFacts, hung: bool, note: Option<&str>) -> Option<CrashKind> {
    if hung {
        Some(CrashKind::Hang)
    } else if facts.disconnected && !facts.left {
        Some(CrashKind::Disconnect)
    } else if note == Some(&*unexpected()) && !facts.left {
        Some(CrashKind::Crash)
    } else {
        None
    }
}

fn ended(e: &Entry, kind: CrashKind, now: i64) -> Ended {
    Ended {
        instance_id: e.inst.id.clone(),
        account_id: e.inst.account_id,
        kind,
        at: now,
        uptime_ms: (now - e.inst.started_at).max(0),
        reason: match kind {
            CrashKind::Hang => Some(hung()),
            CrashKind::Mem => Some(tr!("Commit size reached {mb} MB", mb = e.mem_mb)),
            _ => e.facts.reason.clone(),
        },
        place_id: e.inst.place_id,
        log: e.log.clone(),
        pid: e.inst.pid,
        started_at: e.inst.started_at,
        died_after_ms: e.died_at.map(|d| (d - e.inst.started_at).max(0)),
        clues: format!(
            "pid {}, reached a game: {}, window work done: {}, polls without it: {}, hung polls {}, followed directly: {}, last job known: {}, disconnected: {}, left: {}, human check: {}, refused sign-in: {}, replaced: {}, teleporting: {}, closing: {}, relaunches: {}",
            e.inst.pid,
            e.in_game_at.is_some(),
            e.window_seen,
            e.dead_polls,
            e.hung_polls,
            e.adopted,
            e.facts.job_id.is_some(),
            e.facts.disconnected,
            e.facts.left,
            e.facts.challenged,
            e.facts.auth_failed,
            e.facts.replaced,
            e.facts.teleporting,
            e.closing,
            e.inst.rejoins,
        ),
    }
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

trait Procs {
    fn start_time(&self, pid: u32) -> Option<u64>;
    fn roblox(&self) -> Vec<ProcInfo>;
    fn hung(&self, pid: u32) -> bool;
    fn unreadable_client(&self, pid: u32) -> bool;
    fn has_window(&self, pid: u32) -> bool;
    fn installer_running(&self) -> bool;
    fn log_owners(&self, _path: &Path) -> Vec<ProcInfo> { Vec::new() }
}

struct Live;

impl Procs for Live {
    fn log_owners(&self, path: &Path) -> Vec<ProcInfo> { process::log_owners(path) }
    fn start_time(&self, pid: u32) -> Option<u64> {
        process::start_time(pid)
    }

    fn roblox(&self) -> Vec<ProcInfo> {
        process::roblox_processes()
    }

    fn hung(&self, pid: u32) -> bool {
        process::hung(pid)
    }

    fn unreadable_client(&self, pid: u32) -> bool {
        process::unreadable_client(pid)
    }

    fn has_window(&self, pid: u32) -> bool {
        process::has_any_window(pid)
    }

    fn installer_running(&self) -> bool {
        process::installer_running()
    }
}

// -- Main func
fn observe(
    e: &mut Entry,
    dir: Option<&Path>,
    procs: &dyn Procs,
    roblox: &mut Option<Vec<ProcInfo>>,
    installer: &mut Option<bool>,
    taken: &mut Vec<u32>,
    now: i64,
) {
    let seen = procs.start_time(e.inst.pid);
    if e.start_time == Some(0) {
        e.start_time = seen;
    }
    let first = *e.start_time.get_or_insert(seen.unwrap_or(0));
    let unreadable = seen.is_none() && procs.unreadable_client(e.inst.pid);
    let windowed = seen.is_none() && !unreadable && procs.has_window(e.inst.pid);
    e.alive = seen == Some(first) || unreadable || windowed;
    if (unreadable || windowed) && !e.unreadable_said {
        e.unreadable_said = true;
        log::warn!("instance {}: process {} can't be read but is still running: following it", e.inst.id, e.inst.pid);
    }
    if e.alive {
        e.died_at = None;
        e.dead_polls = 0;
    } else {
        e.died_at.get_or_insert(now);
        e.dead_polls = e.dead_polls.saturating_add(1);
    }
    if !e.adopted && !e.alive {
        e.gone_at.get_or_insert(now);
    }
    if !e.alive && e.in_game_at.is_none() {
        e.installer = *installer.get_or_insert_with(|| procs.installer_running());
    }
    if e.alive && e.adopted {
        e.hung_polls = if procs.hung(e.inst.pid) { e.hung_polls.saturating_add(1) } else { 0 };
    }
    let Some(dir) = dir else { return };
    let launched = UNIX_EPOCH.checked_add(Duration::from_millis(e.inst.started_at.max(0) as u64)).unwrap_or(UNIX_EPOCH);
    let after = first.max(u64::try_from(e.inst.started_at / 1000 - 1).unwrap_or(0));
    if !e.alive && !e.adopted && e.in_game_at.is_none() && !e.slow_start {
        e.slow_start = logwatch::started_log(dir, after);
    }

    if (e.log.is_none() || (!e.alive && e.owner_checked_at.is_none_or(|at| now - at >= 5_000)))
        && let Some(path) = logwatch::find_log(dir, e.btid, launched)
        && e.log.as_ref() != Some(&path)
    {
        e.log = Some(path);
        e.offset = 0;
        e.facts = LogFacts::default();
    }
    if let Some(path) = &e.log {
        let (account, id) = (e.inst.account_id, &e.inst.id);
        let on_problem = |p: &str, n: usize| {
            let times = if n > 1 { format!(" (x{n})") } else { String::new() };
            log::info!(target: "roblox", "account {account} instance {id}: {p}{times}");
        };
        match logwatch::read_new(path, e.offset, &mut e.facts, on_problem) {
            Ok(offset) => {
                if offset > e.offset && e.died_at.is_some_and(|d| now - d >= WRITER_AFTER_MS) {
                    if e.writer_at.is_none() {
                        log::warn!("instance {}: process {} is gone but its log still grows: a client is running", e.inst.id, e.inst.pid);
                    }
                    e.writer_at = Some(now);
                }
                e.offset = offset;
            }
            Err(err) if e.log_warned => log::debug!("instance {}: client log not readable: {err}", e.inst.id),
            Err(err) => {
                e.log_warned = true;
                log::warn!("instance {}: client log not readable: {err}", e.inst.id);
            }
        }
    }
    if (!e.alive || !e.adopted) && (!e.closing || (e.inst.via_bootstrapper && !e.adopted))
        && e.expected_user.zip(e.facts.user_id).is_none_or(|(expected, got)| expected == got)
        && let Some(p) = owned_client(e, procs, roblox, taken, now)
    {
        log::info!("instance {}: recovered PID {} as {} using tracker-matched log ownership", e.inst.id, e.inst.pid, p.pid);
        adopt(e, p, taken);
    }

}

fn owned_client(e: &mut Entry, procs: &dyn Procs, roblox: &mut Option<Vec<ProcInfo>>, taken: &[u32], now: i64) -> Option<ProcInfo> {
    if e.expected_user.zip(e.facts.user_id).is_some_and(|(expected, got)| expected != got) { return None; }
    let path = e.log.as_deref()?;
    if e.owner_checked_at.is_some_and(|at| now - at < 5_000) { return None; }
    e.owner_checked_at = Some(now);
    let owners = procs.log_owners(path);
    let candidates: Vec<_> = roblox.get_or_insert_with(|| procs.roblox()).iter()
        .filter(|p| ((p.pid == e.inst.pid && e.inst.state != InstanceState::Exited) || !taken.contains(&p.pid))
            && p.start_time.saturating_add(1) >= (e.inst.started_at.max(0) / 1000) as u64
            && owners.iter().any(|o| o == *p)).cloned().collect();
    if candidates.len() == 1 { candidates.into_iter().next() } else { None }
}

fn recover_ended(e: &mut Entry, procs: &dyn Procs, taken: &mut Vec<u32>, now: i64) -> bool {
    let mut roblox = Some(procs.roblox());
    let same = roblox.as_ref().and_then(|ps| ps.iter().find(|p| p.pid == e.inst.pid && Some(p.start_time) == e.start_time && !taken.contains(&p.pid))).cloned();
    let found = same.or_else(|| owned_client(e, procs, &mut roblox, taken, now));
    let Some(p) = found else { return false };
    log::info!("instance {}: restored recently lost PID {} as {}; identity confirmed", e.inst.id, e.inst.pid, p.pid);
    adopt(e, p, taken);
    e.gave_up = None;
    true
}

fn arranges(auto_tile: bool, launched: bool) -> bool {
    auto_tile && launched
}

fn adopt(e: &mut Entry, p: ProcInfo, taken: &mut Vec<u32>) {
    e.inst.pid = p.pid;
    e.start_time = Some(p.start_time);
    e.adopted = true;
    e.alive = true;
    e.died_at = None;
    e.writer_at = None;
    e.loading_since = None;
    e.inst.checking_process = false;
    e.inst.ended_at = None;
    e.inst.note = None;
    e.inst.state = InstanceState::Joining;
    e.cpu_ready_at = None;
    e.window_seen = false;
    e.dead_polls = 0;
    taken.push(p.pid);
}

#[cfg(test)]
fn created_secs(path: &Path) -> Option<u64> {
    let created = std::fs::metadata(path)
        .and_then(|m| m.created())
        .inspect_err(|e| log::debug!("log creation time unreadable: {e}"))
        .ok()?;
    Some(created.duration_since(UNIX_EPOCH).ok()?.as_secs())
}

fn may_appear(e: &Entry, now: i64) -> bool {
    let closed = e.closing && (e.adopted || e.gone_at.is_some_and(|g| now - g >= BOOTSTRAPPER_GRACE_MS));
    let waiting = if e.installer {
        INSTALL_WAIT_MS
    } else if e.slow_start && !e.adopted {
        SLOW_START_MS
    } else {
        REBIND_MS
    };
    e.in_game_at.is_none() && !closed && !e.facts.replaced && now - e.inst.started_at < waiting
}

const BOOTSTRAPPER_GRACE_MS: i64 = 5_000;

fn settle(e: &mut Entry, now: i64) -> Option<CrashKind> {
    if e.facts.in_game && e.in_game_at.is_none() {
        e.in_game_at = Some(now);
    }
    if !e.facts.teleporting {
        e.teleport_at = None;
    } else if now - *e.teleport_at.get_or_insert(now) >= TELEPORT_MS {
        log::info!("instance {}: its teleport didn't reach a game in {} s", e.inst.id, TELEPORT_MS / 1000);
        e.facts.teleporting = false;
        e.teleport_at = None;
    }
    let written = !e.closing && e.writer_at.is_some_and(|w| now - w < WRITER_PROOF_MS);
    let silent = !e.window_seen || e.dead_polls >= DEAD_POLLS;
    let missing = !e.alive && !e.closing && !e.facts.replaced;
    let checking = missing && e.died_at.is_some_and(|at| now - at < CHECK_PROCESS_MS);
    if checking != e.inst.checking_process {
        log::info!("instance {} PID {}: {}", e.inst.id, e.inst.pid, if checking { "checking missing process before declaring exit" } else if e.alive { "process tracking confirmed" } else { "process check completed" });
    }
    e.inst.checking_process = checking;
    let ended = !e.alive && silent && !may_appear(e, now) && !written && !checking;
    let (state, mut note) = state_of(&e.facts, e.log.is_some(), ended, e.closing, e.expected_user);
    let crash = match ended {
        true if e.closing && e.mem_restart => Some(CrashKind::Mem),
        true => crash_kind(&e.facts, e.hung(), note.as_deref()),
        false => None,
    };
    e.inst.sign_in_problem = note.as_ref().is_some_and(|n| *n == wrong_account() || *n == challenged() || *n == auth_failed());
    if !ended && note.is_none() && (e.hung() || e.stuck()) {
        let said = if e.stuck() { stuck() } else { hung() };
        if e.inst.note.as_deref() != Some(&*said) {
            log::warn!("instance {}: {said}: the watcher is on it", e.inst.id);
        }
        note = Some(said);
    }
    if let Some(g) = e.gave_up.as_ref().filter(|_| !e.closing) {
        note = Some(g.clone());
    }
    if e.last_join_generation != e.facts.join_generation {
        e.cpu_ready_at = None;
        e.last_join_generation = e.facts.join_generation;
    }
    if state == InstanceState::InGame && !checking && e.alive && !e.facts.teleporting && !e.facts.disconnected {
        e.cpu_ready_at.get_or_insert(now);
    } else {
        e.cpu_ready_at = None;
    }
    e.inst.state = state;
    e.inst.note = note;
    if matches!(state, InstanceState::Starting | InstanceState::Joining) {
        e.loading_since.get_or_insert(now);
    } else {
        e.loading_since = None;
    }
    if e.facts.place_id.is_some() {
        e.inst.place_id = e.facts.place_id;
    }
    if e.facts.job_id.is_some() {
        e.inst.job_id.clone_from(&e.facts.job_id);
    }
    if ended && e.inst.ended_at.is_none() {
        let why = if e.window_seen || e.adopted { "no process and no window" } else { "never showed a window" };
        log::info!("instance {}: gone: {why}, {} s after its launch", e.inst.id, (now - e.inst.started_at) / 1000);
    }
    if ended {
        e.inst.ended_at = Some(now);
    }
    crash
}

fn state_of(facts: &LogFacts, bound: bool, ended: bool, closing: bool, expected_user: Option<i64>) -> (InstanceState, Option<String>) {
    let problem = if facts.replaced {
        Some(tr!("Replaced by another Roblox client"))
    } else if matches!((facts.user_id, expected_user), (Some(got), Some(want)) if got != want) {
        Some(wrong_account())
    } else if facts.challenged {
        Some(challenged())
    } else if facts.auth_failed {
        Some(auth_failed())
    } else {
        None
    };
    if ended {
        let note = if closing { closed() } else { problem.unwrap_or_else(unexpected) };
        return (InstanceState::Exited, Some(note));
    }
    let state = if facts.teleporting {
        InstanceState::Joining
    } else if facts.disconnected {
        InstanceState::Disconnected
    } else if facts.in_game {
        InstanceState::InGame
    } else if bound {
        InstanceState::Joining
    } else {
        InstanceState::Starting
    };
    (state, problem)
}

fn prune(entries: &mut Vec<Entry>) {
    loop {
        let ended = entries.iter().enumerate().filter(|(_, e)| e.inst.state == InstanceState::Exited && e.relaunching.is_none());
        if ended.clone().count() <= MAX_ENDED {
            return;
        }
        let Some((i, _)) = ended.min_by_key(|(_, e)| e.inst.ended_at) else { return };
        entries.remove(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::InstanceState::*;

    const WRONG_ACCOUNT: &str = "Signed in as another account";
    const CHALLENGED: &str = "Roblox wants a human check";
    const UNEXPECTED: &str = "Closed unexpectedly";
    const HUNG: &str = "Not responding";
    const BTID: i64 = 5_550_001;
    const USER: i64 = 261;

    struct Fake(Mutex<Vec<ProcInfo>>, Mutex<Vec<u32>>, Mutex<Vec<u32>>, Mutex<Vec<u32>>, Mutex<bool>, Mutex<Vec<(PathBuf, Vec<ProcInfo>)>>);

    impl Fake {
        fn new(procs: &[(u32, u64)]) -> Fake {
            let f = Fake(Mutex::new(Vec::new()), Mutex::new(Vec::new()), Mutex::new(Vec::new()), Mutex::new(Vec::new()), Mutex::new(false), Mutex::new(Vec::new()));
            f.set(procs);
            f
        }

        fn set_windows(&self, pids: &[u32]) {
            *self.3.lock().unwrap() = pids.to_vec();
        }

        fn set_installer(&self, running: bool) {
            *self.4.lock().unwrap() = running;
        }

        fn set(&self, procs: &[(u32, u64)]) {
            *self.0.lock().unwrap() = procs.iter().map(|&(pid, start_time)| ProcInfo { pid, start_time }).collect();
        }

        fn owners(&self, path: &Path, pairs: &[(u32, u64)]) {
            self.5.lock().unwrap().push((path.into(), pairs.iter().map(|&(pid, start_time)| ProcInfo { pid, start_time }).collect()));
        }

        fn set_hung(&self, pids: &[u32]) {
            *self.1.lock().unwrap() = pids.to_vec();
        }

        fn set_unreadable(&self, pids: &[u32]) {
            *self.2.lock().unwrap() = pids.to_vec();
        }
    }

    impl Procs for Fake {
        fn log_owners(&self, path: &Path) -> Vec<ProcInfo> {
            self.5.lock().unwrap().iter().rev().find(|(p, _)| p == path).map(|(_, ps)| ps.clone()).unwrap_or_default()
        }
        fn start_time(&self, pid: u32) -> Option<u64> {
            self.0.lock().unwrap().iter().find(|p| p.pid == pid).map(|p| p.start_time)
        }

        fn roblox(&self) -> Vec<ProcInfo> {
            self.0.lock().unwrap().clone()
        }

        fn unreadable_client(&self, pid: u32) -> bool {
            self.2.lock().unwrap().contains(&pid)
        }

        fn hung(&self, pid: u32) -> bool {
            self.1.lock().unwrap().contains(&pid)
        }

        fn has_window(&self, pid: u32) -> bool {
            self.3.lock().unwrap().contains(&pid)
        }

        fn installer_running(&self) -> bool {
            *self.4.lock().unwrap()
        }
    }

    fn now_ms() -> i64 {
        crate::commands::now_ms()
    }

    fn info(pid: u32, started_at: i64, via_bootstrapper: bool) -> LaunchInfo {
        LaunchInfo { account_id: 1, pid, browser_tracker_id: BTID, place_id: Some(1818), started_at, via_bootstrapper, warning: None, fps_cap: None, pid_start: None }
    }

    fn target() -> LaunchTarget {
        LaunchTarget::Place { place_id: 1818, launch_data: None }
    }

    #[test]
    fn live_client_per_account() {
        let t = Tracker::new();
        let id = t.register(&info(900, now_ms(), false), target()).id;
        assert!(t.has_live_client(1) && !t.has_live_client(2));
        assert!(t.claim(1).is_none(), "a running account can't be claimed");
        let claim = t.claim(2).unwrap();
        assert!(t.claim(2).is_none(), "nor one being launched");
        drop(claim);
        assert!(t.claim(2).is_some());
        t.entries().iter_mut().find(|e| e.inst.id == id).unwrap().inst.state = Disconnected;
        assert!(!t.has_live_client(1) && t.claim(1).is_some());
    }

    #[test]
    fn a_client_taken_over_is_timed_from_then() {
        let t = Tracker::new();
        let start = now_ms();
        t.register(&LaunchInfo { pid_start: Some(50), ..info(900, start, false) }, target());
        let mut entries = t.entries();
        let e = &mut entries[0];
        settle(e, start);
        let later = start + 100_000;
        adopt(e, ProcInfo { pid: 901, start_time: 60 }, &mut Vec::new());
        settle(e, later);
        assert!(e.runs_free(later + REBIND_MS - 1), "its whole load is free");
        assert!(!e.runs_free(later + REBIND_MS));
    }

    #[test]
    fn loading_hung_and_full_quality_clients_run_free() {
        let t = Tracker::new();
        let start = now_ms();
        let a = t.register(&LaunchInfo { pid_start: Some(50), ..info(900, start, false) }, target()).id;
        let free = |now| t.free(&[], now);
        let set = |f: &dyn Fn(&mut Entry)| f(t.entries().iter_mut().find(|e| e.inst.id == a).unwrap());
        let settled = |now| set(&|e| _ = settle(e, now));
        assert!(free(start).is_empty(), "not seen by a poll yet");

        settled(start);
        assert_eq!(free(start + REBIND_MS - 1), std::slice::from_ref(&a));
        assert!(free(start + REBIND_MS).is_empty(), "stuck loading: limited again");

        set(&|e| e.facts.in_game = true);
        settled(start + 200_000);
        assert_eq!(t.get(&a).unwrap().state, InGame);
        assert!(free(start + 200_000).is_empty());

        let tp = start + 3_600_000;
        set(&|e| e.facts.teleporting = true);
        settled(tp);
        assert_eq!(t.get(&a).unwrap().state, Joining);
        assert_eq!(free(tp + TELEPORT_MS - 1), std::slice::from_ref(&a));
        assert!(free(tp + TELEPORT_MS).is_empty());
        settled(tp + TELEPORT_MS);
        assert_eq!(t.get(&a).unwrap().state, InGame, "the teleport gave up");
        assert!(free(tp + TELEPORT_MS).is_empty());

        let later = tp + 100_000;
        set(&|e| e.hung_polls = HANG_FREE_POLLS - 1);
        assert!(free(later).is_empty());
        set(&|e| e.hung_polls = HANG_FREE_POLLS);
        assert_eq!(free(later), std::slice::from_ref(&a));
        set(&|e| e.hung_polls = 0);

        assert_eq!(t.free(&[1], later), std::slice::from_ref(&a), "Full quality");
        assert!(t.free(&[2], later).is_empty(), "another account");
    }

    #[test]
    fn each_client_is_handed_out_once_when_it_reaches_a_game() {
        let t = Tracker::new();
        let a = t.register(&LaunchInfo { pid_start: Some(50), ..info(900, now_ms(), false) }, target()).id;
        let b = t.register(&LaunchInfo { pid_start: Some(60), ..info(901, now_ms(), true) }, target()).id;
        assert_eq!(t.first_in_game(&|_| true), Vec::<u32>::new(), "still loading: left alone");
        t.entries().iter_mut().for_each(|e| e.inst.state = InGame);
        assert_eq!(t.first_in_game(&|_| false), Vec::<u32>::new(), "no window yet");
        assert_eq!(t.first_in_game(&|_| true), [900], "a bootstrapper isn't the client");
        assert_eq!(t.first_in_game(&|_| true), Vec::<u32>::new(), "once");
        t.entries().iter_mut().find(|e| e.inst.id == b).unwrap().adopted = true;
        assert_eq!(t.first_in_game(&|_| true), [901]);
        assert!(t.set_micro(&a, true) && t.get(&a).unwrap().micro);
        assert!(!t.set_micro("nope", true));
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lram-tracker-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn append(path: &Path, msgs: &[String]) {
        use std::io::Write;
        let mut f = std::fs::File::options().create(true).append(true).open(path).unwrap();
        for m in msgs {
            writeln!(f, "2026-10-19T09:08:03.437Z,0.437181,0a50,6,Info {m}").unwrap();
        }
    }

    fn btid_line(btid: i64) -> String {
        format!("[FLog::LogWin32BTId] LogWin32BTId, App, web launch, websiteBTId is {btid}")
    }

    fn join_lines(user: i64) -> Vec<String> {
        vec![
            "[FLog::Output] ! Joining game '0d6c3f4e-5b2a-4c1d-9e8f-7a6b5c4d3e2f' place 1818 at 10.9.2.90".into(),
            format!("[FLog::GameJoinLoadTime] Report game_join_loadtime: placeid:1818, join_time:0.68, userid:{user},"),
        ]
    }

    fn replicator() -> String {
        "[FLog::Network] Replicator created: 000000007A26CF00".into()
    }

    fn user_of(_: i64) -> Option<i64> {
        Some(USER)
    }

    fn state(t: &Tracker, id: &str) -> (InstanceState, Option<String>, u32) {
        let i = t.get(id).unwrap();
        (i.state, i.note, i.pid)
    }

    fn note(s: &str) -> Option<String> {
        Some(s.into())
    }

    #[test]
    fn states_from_facts() {
        let f = |set: fn(&mut LogFacts)| {
            let mut f = LogFacts::default();
            set(&mut f);
            f
        };
        let none = LogFacts::default();
        let in_game = f(|f| f.in_game = true);
        assert_eq!(state_of(&none, false, false, false, Some(USER)), (Starting, None));
        assert_eq!(state_of(&none, true, false, false, Some(USER)), (Joining, None));
        assert_eq!(state_of(&in_game, true, false, false, Some(USER)), (InGame, None));
        let dropped = f(|f| {
            f.in_game = true;
            f.disconnected = true;
        });
        assert_eq!(state_of(&dropped, true, false, false, Some(USER)), (Disconnected, None));

        let other = f(|f| f.user_id = Some(USER + 1));
        assert_eq!(state_of(&other, true, false, false, Some(USER)), (Joining, note("Signed in as another account")));
        assert_eq!(state_of(&other, true, false, false, None), (Joining, None));
        assert_eq!(state_of(&f(|f| f.user_id = Some(USER)), true, false, false, Some(USER)), (Joining, None));
        let challenged = f(|f| f.challenged = true);
        assert_eq!(state_of(&challenged, true, false, false, Some(USER)), (Joining, note("Roblox wants a human check")));
        let mut refused = LogFacts::default();
        refused.auth_failed = true;
        assert_eq!(state_of(&refused, true, false, false, Some(USER)), (Joining, note("Roblox refused the sign-in (403): verify this account on roblox.com")));
        assert_eq!(state_of(&refused, true, true, false, Some(USER)), (Exited, note("Roblox refused the sign-in (403): verify this account on roblox.com")));
        assert_eq!(state_of(&refused, true, true, true, Some(USER)), (Exited, note("Closed")));

        let teleport = f(|f| (f.in_game, f.teleporting, f.disconnected) = (true, true, true));
        assert_eq!(state_of(&teleport, true, false, false, Some(USER)), (Joining, None), "not a disconnect");
        let failed = f(|f| (f.in_game, f.disconnected) = (true, true));
        assert_eq!(state_of(&failed, true, false, false, Some(USER)), (Disconnected, None));

        let replaced = f(|f| f.replaced = true);
        assert_eq!(state_of(&replaced, true, true, false, Some(USER)), (Exited, note("Replaced by another Roblox client")));
        assert_eq!(state_of(&replaced, true, true, true, Some(USER)), (Exited, note("Closed")));
        assert_eq!(state_of(&in_game, true, true, false, Some(USER)), (Exited, note("Closed unexpectedly")));
        assert_eq!(state_of(&in_game, true, true, true, Some(USER)), (Exited, note("Closed")));
        assert_eq!(state_of(&other, true, true, false, Some(USER)), (Exited, note("Signed in as another account")));
        assert_eq!(state_of(&challenged, false, true, false, Some(USER)), (Exited, note("Roblox wants a human check")));
    }

    #[test]
    fn lists_running_first_and_keeps_30_ended() {
        let t = Tracker::new();
        let ids: Vec<String> = (0..40).map(|n| t.register(&info(100 + n, 1_000 + n as i64, false), target()).id).collect();
        {
            let mut v = t.entries();
            for (n, e) in v.iter_mut().enumerate().take(35) {
                e.inst.state = Exited;
                e.inst.ended_at = Some(if n == 0 { 99_999 } else { 5_000 + n as i64 });
            }
            prune(&mut v);
        }
        let list = t.list();
        assert_eq!(list.len(), 35);
        let running: Vec<&str> = list[..5].iter().map(|i| i.id.as_str()).collect();
        assert_eq!(running, ids[35..].iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(list[5].id, ids[0]);
        assert_eq!(list[6].id, ids[34]);
        assert_eq!(list.last().unwrap().id, ids[6]);

        t.mark_closing(&ids[0]);
        assert!(!t.entries().iter().any(|e| e.closing));
        t.clear_ended();
        assert_eq!(t.list().len(), 5);
    }

    #[test]
    fn the_mute_toggle_belongs_to_one_client_and_the_switch_to_all() {
        let t = Tracker::new();
        let a = t.register(&LaunchInfo { pid_start: Some(50), ..info(900, 1_000, false) }, target()).id;
        let b = t.register(&LaunchInfo { pid_start: Some(60), ..info(901, 1_000, true) }, target()).id;
        let muted = |id: &str| t.get(id).map(|i| i.muted);
        assert_eq!((muted(&a), muted(&b)), (Some(false), Some(false)));
        assert!(!t.set_muted("no such client", true));
        assert!(t.set_muted(&a, true));
        assert_eq!((muted(&a), muted(&b)), (Some(true), Some(false)));
        assert_eq!(t.set_all_muted(true), 2);
        assert_eq!((muted(&a), muted(&b)), (Some(true), Some(true)));
        assert_eq!(t.set_all_muted(false), 2);
        assert_eq!((muted(&a), muted(&b)), (Some(false), Some(false)));

        {
            let mut v = t.entries();
            for e in v.iter_mut() {
                e.adopted = true;
                e.alive = true;
                e.start_time = Some(50);
            }
        }
        t.set_all_muted(true);
        assert_eq!(t.mute_targets(), [(900, true), (901, true)].map(|(pid, muted)| super::super::audio::MuteTarget { pid, muted }));
        t.entries().iter_mut().find(|e| e.inst.id == b).unwrap().inst.state = Exited;
        assert_eq!(t.mute_targets(), [super::super::audio::MuteTarget { pid: 900, muted: true }]);
    }

    #[test]
    fn follows_a_client_from_start_to_exit() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let secs = (start / 1000) as u64;
        let id = t.register(&info(100, start, false), target()).id;
        let procs = Fake::new(&[(100, secs)]);
        let poll = |now: i64| t.poll(Some(&dir), &procs, &user_of, now);

        assert!(poll(start + 1_000).is_empty());
        assert_eq!(state(&t, &id), (Starting, None, 100));

        let log = dir.join("0.744.0.7441080_20261024T120000Z_Player_ABCDE_last.log");
        append(&dir.join("0.744.0.7441080_20261024T120000Z_Player_ABCDE_CrashHandler_last.log"), &[btid_line(BTID)]);
        append(&log, &[btid_line(BTID)]);
        assert!(!poll(start + 2_000).is_empty());
        assert_eq!(state(&t, &id), (Joining, None, 100));
        assert!(poll(start + 3_000).is_empty());

        append(&log, &join_lines(USER));
        append(&log, &[replicator()]);
        assert!(!poll(start + 4_000).is_empty());
        let i = t.get(&id).unwrap();
        assert_eq!((i.state, i.job_id.as_deref(), i.place_id), (InGame, Some("0d6c3f4e-5b2a-4c1d-9e8f-7a6b5c4d3e2f"), Some(1818)));
        assert_eq!(t.afk_clients().iter().map(|c| (c.pid, c.since)).collect::<Vec<_>>(), [(100, start + 4_000)]);
        t.mark_afk(&id, start + 4_500);
        assert_eq!(t.afk_clients()[0].since, start + 4_500);
        assert!(t.set_anti_afk(&id, false) && t.afk_clients().is_empty());

        append(&log, &["[FLog::Network] Disconnection Notification. Reason: 267".into()]);
        let moved = poll(start + 5_000);
        assert_eq!(moved.iter().map(|m| (m.before.state, m.after.state)).collect::<Vec<_>>(), [(InGame, Disconnected)]);
        assert_eq!(state(&t, &id).0, Disconnected);
        assert_eq!(t.get(&id).map(|i| (i.anti_afk, i.afk_at)), Some((false, Some(start + 4_500))));

        procs.set(&[]);
        assert!(!poll(start + 6_000).is_empty());
        assert!(t.get(&id).unwrap().checking_process);
        assert!(t.take_rejoins().is_empty());
        poll(start + 21_000);
        assert_eq!(state(&t, &id), (Exited, note("Closed unexpectedly"), 100));
        assert_eq!(t.get(&id).unwrap().ended_at, Some(start + 21_000));
        assert!(poll(start + 22_000).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_client_that_showed_its_window_is_not_ended_by_one_bad_poll() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let id = t.register(&info(700, start, false), target()).id;
        let procs = Fake::new(&[(700, (start / 1000) as u64)]);
        let log = dir.join("0.744.0.7441080_20261024T120000Z_Player_WINDOW_last.log");
        append(&log, &[btid_line(BTID)]);
        append(&log, &join_lines(USER));
        append(&log, &[replicator()]);
        t.poll(Some(&dir), &procs, &user_of, start + 1_000);
        assert_eq!(state(&t, &id).0, InGame);
        assert_eq!(t.first_in_game(&|pid| pid == 700), [700]);

        procs.set(&[]);
        t.poll(Some(&dir), &procs, &user_of, start + 2_000);
        assert_eq!(state(&t, &id).0, InGame, "one bad read is not proof");
        t.poll(Some(&dir), &procs, &user_of, start + 3_000);
        assert!(t.get(&id).unwrap().checking_process);
        t.poll(Some(&dir), &procs, &user_of, start + 17_000);
        assert_eq!(state(&t, &id), (Exited, note("Closed unexpectedly"), 700));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn notes_the_wrong_account_and_a_requested_close() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let id = t.register(&info(100, start, false), target()).id;
        let procs = Fake::new(&[(100, (start / 1000) as u64)]);
        let log = dir.join("0.744.0.7441080_20261024T120000Z_Player_ABCDE_last.log");
        append(&log, &[btid_line(BTID)]);
        append(&log, &join_lines(USER + 1));
        t.poll(Some(&dir), &procs, &user_of, start + 1_000);
        assert_eq!(state(&t, &id), (Joining, note("Signed in as another account"), 100));

        t.mark_closing(&id);
        procs.set(&[]);
        t.poll(Some(&dir), &procs, &user_of, start + 2_000);
        assert_eq!(state(&t, &id), (Exited, note("Closed"), 100));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn readopts_only_the_same_running_client() {
        let old = Tracker::new();
        let start = now_ms();
        let a = old.register(&LaunchInfo { pid_start: Some(50), ..info(900, start, false) }, target()).id;
        old.register(&LaunchInfo { pid_start: Some(60), ..info(901, start, true) }, target());
        old.entries()[0].offset = 4_096;
        old.entries()[0].facts.in_game = true;
        old.entries()[0].in_game_at = Some(start + 9);
        assert!(old.set_limits(&a, Limits { cpu_cap: Some(20), ..Limits::default() }).is_some() && old.set_anti_afk(&a, false));
        let saved = old.saved();
        assert_eq!(saved.len(), 1);
        let json = serde_json::to_string(&saved).unwrap();
        let saved: Vec<Saved> = serde_json::from_str(&json).unwrap();
        assert_eq!(saved, old.saved(), "JSON round trip");

        let p = |pid, start_time| ProcInfo { pid, start_time };
        let fresh = Tracker::new();
        assert!(readoptable(&saved[0], &[p(900, 50)], &fresh.entries()));
        assert!(!readoptable(&saved[0], &[], &fresh.entries()), "gone");
        assert!(!readoptable(&saved[0], &[p(900, 51)], &fresh.entries()), "pid reused");
        assert!(!readoptable(&saved[0], &[p(900, 50)], &old.entries()), "already followed");

        assert_eq!(fresh.readopt(saved.clone(), &[p(900, 50)]), [(900, 50)]);
        assert_eq!(fresh.readopt(saved, &[p(900, 50)]), [], "not twice");
        let e = fresh.entries()[0].clone();
        assert_eq!((e.offset, e.facts.in_game, e.in_game_at, e.window_seen), (4_096, true, Some(start + 9), true));
        assert_eq!(fresh.get(&a), old.get(&a), "the UI sees it as before");
        assert_eq!(fresh.clients().len(), 1, "limits apply to it again");
    }

    #[test]
    fn a_client_that_cant_be_read_is_followed_again_after_a_restart() {
        let old = Tracker::new();
        old.register(&LaunchInfo { pid_start: Some(50), ..info(900, now_ms(), false) }, target());
        let saved = old.saved();
        let p = |pid, start_time| ProcInfo { pid, start_time };
        assert!(with_unreadable(&saved, vec![], |_| false).is_empty(), "gone");
        assert_eq!(with_unreadable(&saved, vec![], |pid| pid == 900), [p(900, 50)], "at its saved start time");
        assert_eq!(with_unreadable(&saved, vec![p(900, 51)], |_| true), [p(900, 51)], "readable: another process took the pid");
        let fresh = Tracker::new();
        assert_eq!(fresh.readopt(saved.clone(), &with_unreadable(&saved, vec![], |_| true)), [(900, 50)]);
    }

    #[test]
    fn a_reused_pid_is_not_the_client() {
        let t = Tracker::new();
        let start = now_ms();
        let id = t.register(&info(100, start, false), target()).id;
        let procs = Fake::new(&[(100, 10)]);
        t.poll(None, &procs, &user_of, start + 1_000);
        procs.set(&[(100, 20)]);
        t.poll(None, &procs, &user_of, start + 121_000);
        assert!(t.get(&id).unwrap().checking_process);
        t.poll(None, &procs, &user_of, start + 136_000);
        assert_eq!(state(&t, &id).0, Exited);
    }

    fn old_log(path: &Path, secs: u64) {
        use std::os::windows::fs::FileTimesExt;
        let times = std::fs::FileTimes::new()
            .set_created(UNIX_EPOCH + Duration::from_secs(secs))
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(2));
        std::fs::File::options().write(true).open(path).unwrap().set_times(times).unwrap();
    }

    #[test]
    fn rebinds_after_an_updater_relaunch() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms() - 10_000;
        let secs = (start / 1000) as u64;
        let id = t.register(&info(100, start, false), target()).id;
        let other = t.register(&LaunchInfo { browser_tracker_id: BTID + 1, ..info(300, start, false) }, target()).id;
        let procs = Fake::new(&[(100, secs), (300, secs)]);
        let poll = |now: i64| t.poll(Some(&dir), &procs, &user_of, now);

        let old = dir.join("0.743.0.7431134_20261024T120000Z_Player_AAAAA_last.log");
        append(&old, &[btid_line(BTID), "[FLog::UpdateController] Update check thread: updateRequired TRUE".into()]);
        old_log(&old, secs);
        poll(start + 1_000);
        assert_eq!(state(&t, &id), (Joining, None, 100));

        procs.set(&[(300, secs)]);
        poll(start + 2_000);
        assert_eq!(state(&t, &id), (Joining, None, 100));

        let new = dir.join("0.744.0.7441080_20261024T120010Z_Player_BBBBB_last.log");
        append(&new, &[btid_line(BTID)]);
        append(&new, &join_lines(USER));
        procs.set(&[(300, secs), (200, secs + 10)]);
        procs.owners(&new, &[(200, secs + 10)]);
        poll(start + 12_000);
        assert_eq!(state(&t, &id), (Joining, None, 200));
        append(&new, &[replicator()]);
        poll(start + 13_000);
        assert_eq!(state(&t, &id), (InGame, None, 200));
        assert_eq!(state(&t, &other), (Starting, None, 300));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_dead_clients_own_log_is_not_a_relaunch() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms() - 10_000;
        let secs = (start / 1000) as u64;
        let id = t.register(&info(100, start, false), target()).id;
        let procs = Fake::new(&[(100, secs)]);
        t.poll(Some(&dir), &procs, &user_of, start + 1_000);

        let own = dir.join("0.743.0.7431134_20261024T120000Z_Player_AAAAA_last.log");
        append(&own, &[btid_line(BTID)]);
        old_log(&own, secs);
        procs.set(&[(400, secs + 1)]);
        t.poll(Some(&dir), &procs, &user_of, start + 2_000);
        assert_eq!(state(&t, &id), (Joining, None, 100));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn in_game_client(dir: &Path, start: i64) -> (Tracker, String, Fake, PathBuf) {
        let t = Tracker::new();
        let id = t.register(&info(100, start, false), target()).id;
        let procs = Fake::new(&[(100, (start / 1000) as u64)]);
        let log = dir.join("0.744.0.7441080_20261024T120000Z_Player_ABCDE_last.log");
        append(&log, &[btid_line(BTID)]);
        append(&log, &join_lines(USER));
        append(&log, &["[FLog::Network] Replicator created".to_owned()]);
        t.poll(Some(dir), &procs, &user_of, start + 1_000);
        assert_eq!(state(&t, &id), (InGame, None, 100));
        (t, id, procs, log)
    }

    #[test]
    fn a_client_that_cant_be_read_but_runs_isnt_closed() {
        let dir = temp_dir();
        let start = now_ms() - 600_000;
        let (t, id, procs, _) = in_game_client(&dir, start);
        procs.set(&[]);
        procs.set_unreadable(&[100]);
        t.poll(Some(&dir), &procs, &user_of, start + 2_000);
        t.poll(Some(&dir), &procs, &user_of, start + 300_000);
        assert_eq!(state(&t, &id), (InGame, None, 100));
        assert!(t.take_crashes().is_empty());
        procs.set_unreadable(&[]);
        t.poll(Some(&dir), &procs, &user_of, start + 301_000);
        assert!(t.get(&id).unwrap().checking_process);
        t.poll(Some(&dir), &procs, &user_of, start + 316_000);
        assert_eq!(state(&t, &id), (Exited, note("Closed unexpectedly"), 100));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_start_time_unread_at_launch_is_read_later() {
        let t = Tracker::new();
        let start = now_ms() - 600_000;
        let launched = LaunchInfo { pid_start: Some(0), ..info(100, start, false) };
        let id = t.register(&launched, target()).id;
        let procs = Fake::new(&[(100, 555)]);
        t.poll(None, &procs, &user_of, start + 300_000);
        assert_eq!(state(&t, &id).0, Starting);
        procs.set(&[(100, 556)]);
        t.poll(None, &procs, &user_of, start + 301_000);
        assert!(t.get(&id).unwrap().checking_process);
        t.poll(None, &procs, &user_of, start + 316_000);
        assert_eq!(state(&t, &id).0, Exited);
    }

    #[test]
    fn a_client_without_a_readable_process_but_with_its_window_isnt_closed() {
        let dir = temp_dir();
        let start = now_ms() - 600_000;
        let (t, id, procs, _) = in_game_client(&dir, start);
        procs.set(&[]);
        procs.set_windows(&[100]);
        t.poll(Some(&dir), &procs, &user_of, start + 2_000);
        t.poll(Some(&dir), &procs, &user_of, start + 40_000);
        assert_eq!(state(&t, &id), (InGame, None, 100));
        assert!(t.take_crashes().is_empty());
        procs.set_windows(&[]);
        t.poll(Some(&dir), &procs, &user_of, start + 41_000);
        assert!(t.get(&id).unwrap().checking_process);
        t.poll(Some(&dir), &procs, &user_of, start + 56_000);
        assert_eq!(state(&t, &id), (Exited, note("Closed unexpectedly"), 100));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_client_the_installer_took_away_keeps_waiting() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let secs = (start / 1000) as u64;
        let id = t.register(&info(100, start, false), target()).id;
        let log = dir.join("0.744.0.7441080_20261024T120000Z_Player_ABCDE_last.log");
        append(&log, &[btid_line(BTID)]);
        let procs = Fake::new(&[(100, secs)]);
        t.poll(Some(&dir), &procs, &user_of, start + 1_000);
        procs.set(&[]);
        procs.set_installer(true);
        let late = start + REBIND_MS + 60_000;
        t.poll(Some(&dir), &procs, &user_of, start + 2_000);
        t.poll(Some(&dir), &procs, &user_of, late);
        assert_ne!(state(&t, &id).0, Exited, "the installer is still at it");
        assert!(t.take_crashes().is_empty());
        procs.set_installer(false);
        t.poll(Some(&dir), &procs, &user_of, late + 2_000);
        assert_eq!(state(&t, &id).0, Exited);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_log_that_grows_after_its_process_went_means_a_running_client() {
        let dir = temp_dir();
        let start = now_ms() - 600_000;
        let (t, id, procs, log) = in_game_client(&dir, start);
        procs.set(&[]);
        let gone = start + 2_000;
        t.entries().iter_mut().for_each(|e| e.died_at = Some(gone));
        append(&log, &["[FLog::Network] still here".to_owned()]);
        t.poll(Some(&dir), &procs, &user_of, gone + WRITER_AFTER_MS);
        assert_eq!(state(&t, &id), (InGame, None, 100), "the log grew after it went");
        assert!(t.take_crashes().is_empty());

        procs.set(&[(200, created_secs(&log).unwrap())]);
        procs.owners(&log, &[(200, created_secs(&log).unwrap())]);
        t.poll(Some(&dir), &procs, &user_of, gone + WRITER_AFTER_MS + 6_000);
        assert_eq!(state(&t, &id), (InGame, None, 200));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_crashing_clients_last_words_prove_nothing() {
        let dir = temp_dir();
        let start = now_ms() - 600_000;
        let (t, id, procs, log) = in_game_client(&dir, start);
        procs.set(&[]);
        append(&log, &["[FLog::Network] last words".to_owned()]);
        t.poll(Some(&dir), &procs, &user_of, start + 2_000);
        assert!(t.get(&id).unwrap().checking_process);
        t.poll(Some(&dir), &procs, &user_of, start + 17_000);
        assert_eq!(state(&t, &id), (Exited, note("Closed unexpectedly"), 100));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_log_that_stops_growing_ends_the_client_after_all() {
        let dir = temp_dir();
        let start = now_ms() - 600_000;
        let (t, id, procs, log) = in_game_client(&dir, start);
        procs.set(&[]);
        let gone = start + 2_000;
        t.entries().iter_mut().for_each(|e| e.died_at = Some(gone));
        append(&log, &["[FLog::Network] still here".to_owned()]);
        t.poll(Some(&dir), &procs, &user_of, gone + WRITER_AFTER_MS);
        assert_eq!(state(&t, &id).0, InGame);
        t.poll(Some(&dir), &procs, &user_of, gone + WRITER_AFTER_MS + WRITER_PROOF_MS);
        assert_eq!(state(&t, &id), (Exited, note("Closed unexpectedly"), 100));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_client_gone_before_the_game_ends_after_120_s() {
        let t = Tracker::new();
        let start = now_ms();
        let id = t.register(&info(100, start, false), target()).id;
        let procs = Fake::new(&[]);
        t.poll(None, &procs, &user_of, start + 1_000);
        assert_eq!(state(&t, &id).0, Starting);
        t.poll(None, &procs, &user_of, start + 119_000);
        assert_eq!(state(&t, &id).0, Starting);
        t.poll(None, &procs, &user_of, start + 120_000);
        assert_eq!(state(&t, &id), (Exited, note("Closed unexpectedly"), 100));
        let ended = t.take_crashes().remove(0);
        assert_eq!((ended.uptime_ms, ended.died_after_ms), (120_000, Some(1_000)), "when it really went, not when it was given up on");
    }

    #[test]
    fn adopts_the_client_behind_a_bootstrapper() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let secs = (start / 1000) as u64;
        let id = t.register(&info(900, start, true), target()).id;
        let procs = Fake::new(&[(900, secs), (50, secs - 600)]);
        let poll = |now: i64| t.poll(Some(&dir), &procs, &user_of, now);
        poll(start + 1_000);
        assert_eq!(state(&t, &id), (Starting, None, 900));

        procs.set(&[(50, secs - 600), (901, secs + 1)]);
        poll(start + 3_000);
        assert_eq!(state(&t, &id), (Starting, None, 900));
        append(&dir.join("0.744.0.7441080_20261024T120002Z_Player_CCCCC_last.log"), &[btid_line(BTID)]);
        procs.owners(&dir.join("0.744.0.7441080_20261024T120002Z_Player_CCCCC_last.log"), &[(901, secs + 1)]);
        poll(start + 4_000);
        assert_eq!(state(&t, &id), (Joining, None, 901));
        procs.set(&[(50, secs - 600)]);
        poll(start + 5_000);
        assert_eq!(state(&t, &id).0, Joining);
        assert!(t.get(&id).unwrap().via_bootstrapper);
        poll(start + 121_000);
        assert_eq!(state(&t, &id), (Exited, note("Closed unexpectedly"), 901));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_slow_client_is_not_given_up_on_before_it_joins() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let secs = (start / 1000) as u64;
        let id = t.register(&info(900, start, true), target()).id;
        let procs = Fake::new(&[]);
        let poll = |now: i64| t.poll(Some(&dir), &procs, &user_of, now);
        let log = dir.join("0.745.0.7451137_20261103T220008Z_Player_AAAAA_last.log");
        append(&log, &["[FLog::Output] Roblox stage ReadyForFlagFetch".into()]);
        poll(start + 1_000);
        poll(start + 121_000);
        assert_eq!(state(&t, &id).0, Starting, "its log says a client of this launch started");
        procs.set(&[(901, secs + 1)]);
        append(&log, &[btid_line(BTID)]);
        procs.owners(&log, &[(901, secs + 1)]);
        poll(start + 300_000);
        assert_eq!(state(&t, &id), (Joining, None, 901));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn closing_a_bootstrapper_leaves_its_client_running() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let secs = (start / 1000) as u64;
        let id = t.register(&info(900, start, true), target()).id;
        let procs = Fake::new(&[(900, secs)]);
        let poll = |now: i64| t.poll(Some(&dir), &procs, &user_of, now);
        poll(start + 1_000);
        assert_eq!(t.mark_closing(&id), Some((900, Some(secs))));

        procs.set(&[(901, secs + 1)]);
        poll(start + 3_000);
        assert_eq!(state(&t, &id), (Starting, None, 900));
        append(&dir.join("0.744.0.7441080_20261024T120002Z_Player_DDDDD_last.log"), &[btid_line(BTID)]);
        procs.owners(&dir.join("0.744.0.7441080_20261024T120002Z_Player_DDDDD_last.log"), &[(901, secs + 1)]);
        poll(start + 4_000);
        assert_eq!(state(&t, &id), (Joining, None, 901));
        assert_eq!(t.mark_closing(&id), Some((901, Some(secs + 1))));
        procs.set(&[]);
        poll(start + 5_000);
        assert_eq!(state(&t, &id), (Exited, note("Closed"), 901));

        let empty = temp_dir();
        let poll = |now: i64| t.poll(Some(&empty), &procs, &user_of, now);
        let id = t.register(&info(910, start, true), target()).id;
        procs.set(&[(910, secs)]);
        poll(start + 1_000);
        t.mark_closing(&id);
        procs.set(&[]);
        poll(start + 2_000);
        assert_eq!(state(&t, &id).0, Starting);
        poll(start + 8_000);
        assert_eq!(state(&t, &id), (Exited, note("Closed"), 910));
        assert_eq!(t.mark_closing(&id), None);
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&empty).unwrap();
    }

    #[test]
    fn rejoins_disconnects_and_restarts_crashes_and_hangs() {
        use CrashKind::*;
        let base = Tracker::new().register(&info(100, 0, false), target());
        let inst = |state, note: Option<&str>| Instance { state, note: note.map(Into::into), ..base.clone() };
        let facts = |disconnected, left| {
            let mut f = LogFacts::default();
            (f.disconnected, f.left) = (disconnected, left);
            f
        };
        let (kicked, none) = (facts(true, false), facts(false, false));
        assert_eq!(wants_rejoin(&inst(Disconnected, None), &kicked, false, false), Some(Disconnect));
        assert_eq!(wants_rejoin(&inst(Exited, Some(UNEXPECTED)), &kicked, false, false), Some(Disconnect), "closed by itself after the kick");
        assert_eq!(wants_rejoin(&inst(Disconnected, None), &facts(true, true), false, false), None, "the player left");
        assert_eq!(wants_rejoin(&inst(Exited, Some(UNEXPECTED)), &facts(true, true), false, false), None, "left, then closed");
        assert_eq!(wants_rejoin(&inst(Exited, Some(UNEXPECTED)), &none, false, false), Some(Crash));
        let mut replaced = none.clone();
        replaced.replaced = true;
        assert_eq!(wants_rejoin(&inst(Exited, Some(UNEXPECTED)), &replaced, false, false), None);
        for n in ["Closed", WRONG_ACCOUNT, CHALLENGED, "Gave up after 3 restarts"] {
            assert_eq!(wants_rejoin(&inst(Exited, Some(n)), &kicked, false, false), None, "{n}");
            assert_eq!(wants_rejoin(&inst(InGame, Some(n)), &none, true, false), None, "hung, {n}");
        }
        assert_eq!(wants_rejoin(&inst(Disconnected, Some(CHALLENGED)), &kicked, false, false), None);
        for state in [Starting, Joining, InGame] {
            assert_eq!(wants_rejoin(&inst(state, None), &none, false, false), None, "{state:?}");
            assert_eq!(wants_rejoin(&inst(state, Some(HUNG)), &none, true, false), Some(Hang), "{state:?}");
        }
        assert_eq!(wants_rejoin(&inst(Disconnected, Some(HUNG)), &kicked, true, false), Some(Hang));
        for state in [Starting, Joining, InGame] {
            assert_eq!(wants_rejoin(&inst(state, None), &none, false, true), Some(Hang), "stuck, {state:?}");
        }
        assert_eq!(wants_rejoin(&inst(Exited, None), &none, false, true), None, "gone, not stuck");
        let stuck_note = stuck();
        assert_eq!(wants_rejoin(&inst(Starting, Some(&stuck_note)), &none, false, true), Some(Hang), "said once, still restarted");
        assert_eq!(wants_rejoin(&inst(Starting, Some(&stuck_note)), &none, false, false), None, "and only while it is stuck");
        assert_eq!(wants_rejoin(&inst(Exited, Some("Closed")), &none, true, false), None, "a hung client LRAM closed");
    }

    #[test]
    fn what_ended_badly() {
        let facts = |disconnected, left| {
            let mut f = LogFacts::default();
            (f.disconnected, f.left) = (disconnected, left);
            f
        };
        assert_eq!(crash_kind(&facts(false, false), false, Some(UNEXPECTED)), Some(CrashKind::Crash));
        assert_eq!(crash_kind(&facts(true, false), false, Some("Closed")), Some(CrashKind::Disconnect));
        assert_eq!(crash_kind(&facts(false, false), true, Some("Closed")), Some(CrashKind::Hang));
        for note in ["Closed", "Replaced by another Roblox client", WRONG_ACCOUNT, CHALLENGED] {
            assert_eq!(crash_kind(&facts(false, false), false, Some(note)), None, "{note}");
        }
        assert_eq!(crash_kind(&facts(true, true), false, Some(UNEXPECTED)), None, "the player left");
    }

    #[test]
    fn hands_out_each_disconnect_once() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let secs = (start / 1000) as u64;
        let launch = |pid: u32, account_id: i64| LaunchInfo { account_id, browser_tracker_id: BTID + i64::from(pid), ..info(pid, start, false) };
        let follow = LaunchTarget::FollowUser { user_id: 42 };
        let kicked = t.register(&launch(100, 7), follow.clone()).id;
        let left = t.register(&launch(200, 1), target()).id;
        let closed = t.register(&launch(300, 1), target()).id;
        t.set_rejoins(&kicked, 1);
        let log = |pid: u32| dir.join(format!("0.744.0.7441080_20261024T1200{pid}Z_Player_R{pid}_last.log"));
        for pid in [100, 200, 300] {
            append(&log(pid), &[btid_line(BTID + i64::from(pid))]);
            append(&log(pid), &join_lines(USER));
            append(&log(pid), &[replicator()]);
        }
        let procs = Fake::new(&[(100, secs), (200, secs), (300, secs)]);
        let poll = |now: i64| t.poll(Some(&dir), &procs, &user_of, now);
        poll(start + 1_000);
        assert_eq!(state(&t, &kicked).0, InGame);
        assert!(t.take_rejoins().is_empty());

        append(&log(100), &["[FLog::Network] Sending disconnect with reason: 267".into()]);
        append(&log(200), &["[FLog::Network] Disconnection Notification. Reason: 285".into()]);
        procs.set(&[(100, secs), (200, secs)]);
        t.entries().iter_mut().find(|e| e.inst.id == closed).unwrap().died_at = Some(start - CHECK_PROCESS_MS);
        poll(start + 2_000);
        assert_eq!(state(&t, &kicked).0, Disconnected);
        assert_eq!(state(&t, &closed), (Exited, note("Closed unexpectedly"), 300));
        let crash = Rejoin { from: closed.clone(), account_id: 1, target: target(), rejoins: 1, cause: CrashKind::Crash };
        let kick = Rejoin { from: kicked.clone(), account_id: 7, target: follow.clone(), rejoins: 2, cause: CrashKind::Disconnect };
        assert_eq!(t.take_rejoins(), vec![kick, crash]);
        assert!(t.take_rejoins().is_empty());
        let ended = t.take_crashes();
        assert_eq!(ended.iter().map(|c| (c.instance_id.as_str(), c.kind)).collect::<Vec<_>>(), [(closed.as_str(), CrashKind::Crash)]);
        assert_eq!((ended[0].reason.as_ref(), ended[0].uptime_ms, ended[0].log.as_ref()), (None, 2_000, Some(&log(300))));
        poll(start + 3_000);
        assert!(t.take_rejoins().is_empty());

        append(&log(100), &join_lines(USER));
        append(&log(100), &[replicator()]);
        poll(start + 4_000);
        assert!(t.take_rejoins().is_empty());
        append(&log(100), &["[FLog::Network] Lost connection with reason : You have been kicked from the game".into()]);
        poll(start + 5_000);
        assert_eq!(t.take_rejoins().len(), 1);
        assert_eq!(state(&t, &left).0, Disconnected);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_waiting_relaunch_ends_with_close_all_a_clear_or_a_newer_launch() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let secs = (start / 1000) as u64;
        let id = t.register(&info(100, start, false), target()).id;
        let procs = Fake::new(&[(100, secs)]);
        let poll = |now: i64| t.poll(Some(&dir), &procs, &user_of, now);
        poll(start + 1_000);
        procs.set(&[]);
        poll(start + 200_000);
        assert!(t.get(&id).unwrap().checking_process && t.take_rejoins().is_empty());
        poll(start + 215_000);
        assert_eq!(t.take_rejoins().len(), 1);
        assert!(!t.relaunch_wanted(&id, false), "no task at work");

        assert!(t.begin_relaunch(&id, start));
        assert!(!t.begin_relaunch(&id, start) && t.is_relaunching(&id));
        poll(start + 216_000);
        assert!(t.relaunch_wanted(&id, false));
        t.entries()[0].closing = true;
        assert!(!t.relaunch_wanted(&id, false));
        assert!(t.relaunch_wanted(&id, true));
        t.entries()[0].closing = false;

        for pid in 0..(MAX_ENDED as u32 + 2) {
            let other = t.register(&LaunchInfo { account_id: 50 + i64::from(pid), browser_tracker_id: BTID + 10 + i64::from(pid), ..info(1_000 + pid, start, false) }, target()).id;
            let mut entries = t.entries();
            let e = entries.iter_mut().find(|e| e.inst.id == other).unwrap();
            (e.inst.state, e.inst.ended_at) = (Exited, Some(start + 300_000 + i64::from(pid)));
        }
        prune(&mut t.entries());
        assert_eq!(t.entries().iter().filter(|e| e.inst.state == Exited).count(), MAX_ENDED + 1);
        assert!(t.get(&id).is_some());
        t.end_relaunch(&id);
        prune(&mut t.entries());
        assert!(t.get(&id).is_none());

        let second = t.register(&LaunchInfo { account_id: 2, browser_tracker_id: BTID + 1, ..info(200, start + 1, false) }, target()).id;
        procs.set(&[(200, secs)]);
        poll(start + 400_000);
        procs.set(&[]);
        poll(start + 600_000);
        assert!(t.get(&second).unwrap().checking_process);
        poll(start + 615_000);
        assert!(t.begin_relaunch(&second, start) && t.relaunch_wanted(&second, false));
        assert_eq!(t.relaunches_open(), 0, "ended: nothing to close");
        t.cancel_relaunches();
        assert!(!t.relaunch_wanted(&second, false) && !t.relaunch_wanted(&second, true) && !t.is_relaunching(&second));
        assert!(t.begin_relaunch(&second, start + 1));
        assert!(!t.relaunch_wanted(&second, false), "handed out after Close all: counts as closed by the user");
        t.entries().iter_mut().find(|e| e.inst.id == second).unwrap().closing = false;

        let older = t.register(&LaunchInfo { account_id: 2, browser_tracker_id: BTID + 3, ..info(202, start, false) }, target()).id;
        t.entries().iter_mut().find(|e| e.inst.id == older).unwrap().inst.state = Exited;
        assert!(t.relaunch_wanted(&second, false));
        t.set_relaunch_problem(&second, Some("no network".into()));
        let newer = t.register(&LaunchInfo { account_id: 2, browser_tracker_id: BTID + 2, ..info(201, start + 2, false) }, target()).id;
        assert!(!t.relaunch_wanted(&second, true));
        assert_eq!(t.get(&second).unwrap().relaunch_problem, None, "the new client ended the wait");
        t.entries().iter_mut().find(|e| e.inst.id == newer).unwrap().inst.state = Exited;
        assert!(!t.relaunch_wanted(&second, true));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_recovered_client_loses_its_gave_up_note() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let secs = (start / 1000) as u64;
        let id = t.register(&info(100, start, false), target()).id;
        let log = dir.join("0.744.0.7441080_20261024T120100Z_Player_R100_last.log");
        append(&log, &[btid_line(BTID)]);
        append(&log, &join_lines(USER));
        append(&log, &[replicator()]);
        let procs = Fake::new(&[(100, secs)]);
        let poll = |now: i64| t.poll(Some(&dir), &procs, &user_of, now);
        poll(start + 1_000);
        append(&log, &["[FLog::Network] Sending disconnect with reason: 267".into()]);
        poll(start + 2_000);
        assert_eq!(t.take_rejoins().len(), 1);
        t.begin_relaunch(&id, start);
        t.set_relaunch_problem(&id, Some("no network".into()));
        t.give_up(&id, "Gave up".into());
        poll(start + 3_000);
        assert_eq!(state(&t, &id).1, note("Gave up"));
        assert!(t.take_rejoins().is_empty(), "still the same trouble");

        append(&log, &join_lines(USER));
        append(&log, &[replicator()]);
        poll(start + 4_000);
        assert!(t.take_rejoins().is_empty());
        assert_eq!(t.get(&id).unwrap().relaunch_problem, None, "back in a game: nothing to wait for");
        poll(start + 5_000);
        assert_eq!(state(&t, &id), (InGame, None, 100));
        append(&log, &["[FLog::Network] Sending disconnect with reason: 267".into()]);
        poll(start + 6_000);
        assert_eq!(t.take_rejoins().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_closed_client_is_not_rejoined() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let secs = (start / 1000) as u64;
        let id = t.register(&info(100, start, false), target()).id;
        let log = dir.join("0.744.0.7441080_20261024T120100Z_Player_R100_last.log");
        append(&log, &[btid_line(BTID)]);
        append(&log, &join_lines(USER));
        append(&log, &[replicator()]);
        let procs = Fake::new(&[(100, secs)]);
        let poll = |now: i64| t.poll(Some(&dir), &procs, &user_of, now);
        poll(start + 1_000);
        t.set_rejoins(&id, 5);
        append(&log, &["[FLog::Network] Sending disconnect with reason: 267".into()]);
        poll(start + 2_000);
        assert_eq!(t.take_rejoins(), vec![Rejoin { from: id.clone(), account_id: 1, target: target(), rejoins: 6, cause: CrashKind::Disconnect }]);
        assert!(t.still_wants_rejoin(&id));

        t.mark_closing(&id);
        assert!(!t.still_wants_rejoin(&id));
        procs.set(&[]);
        poll(start + 3_000);
        assert_eq!(state(&t, &id).1, note("Closed"));
        assert!(!t.still_wants_rejoin(&id));
        let ended = t.take_crashes();
        assert_eq!((ended.len(), ended[0].kind), (1, CrashKind::Disconnect));
        assert_eq!(ended[0].reason.as_deref(), Some("Sending disconnect with reason: 267"));
        t.clear_ended();
        assert!(!t.still_wants_rejoin(&id));

        let crashed = t.register(&LaunchInfo { browser_tracker_id: BTID + 1, ..info(200, start, false) }, target()).id;
        procs.set(&[(200, secs)]);
        poll(start + 4_000);
        procs.set(&[]);
        poll(start + 200_000);
        assert!(t.get(&crashed).unwrap().checking_process && !t.still_wants_rejoin(&crashed));
        poll(start + 215_000);
        assert!(t.still_wants_rejoin(&crashed));
        t.mark_all_closing();
        assert!(!t.still_wants_rejoin(&crashed));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_hung_client_is_restarted_once_and_given_up_on() {
        let t = Tracker::new();
        let start = now_ms();
        let secs = (start / 1000) as u64;
        let id = t.register(&info(100, start, false), target()).id;
        let procs = Fake::new(&[(100, secs)]);
        let poll = |n: i64| t.poll(None, &procs, &user_of, start + n * 1_000);
        poll(1);
        procs.set_hung(&[100]);
        for n in 2..=i64::from(HANG_POLLS) {
            poll(n);
        }
        assert_eq!(state(&t, &id).1, None, "not yet");
        assert!(t.take_rejoins().is_empty());
        poll(i64::from(HANG_POLLS) + 1);
        assert_eq!(state(&t, &id), (Starting, note(HUNG), 100));
        assert_eq!(t.take_rejoins().iter().map(|r| r.cause).collect::<Vec<_>>(), [CrashKind::Hang]);
        assert!(t.take_rejoins().is_empty() && t.still_wants_rejoin(&id));

        procs.set_hung(&[]);
        poll(40);
        assert_eq!(state(&t, &id).1, None);
        assert!(!t.still_wants_rejoin(&id) && t.take_rejoins().is_empty());

        procs.set_hung(&[100]);
        for n in 41..=41 + i64::from(HANG_POLLS) {
            poll(n);
        }
        assert_eq!(t.take_rejoins().len(), 1);
        t.give_up(&id, "Gave up after 3 restarts".into());
        poll(80);
        assert_eq!(state(&t, &id).1, note("Gave up after 3 restarts"));
        assert!(!t.still_wants_rejoin(&id));

        procs.set(&[]);
        poll(200);
        assert!(t.get(&id).unwrap().checking_process);
        poll(215);
        assert_eq!(state(&t, &id), (Exited, note("Gave up after 3 restarts"), 100));
        let ended = t.take_crashes();
        assert_eq!((ended.len(), ended[0].kind, ended[0].reason.as_deref()), (1, CrashKind::Hang, Some(HUNG)));
    }

    #[test]
    fn samples_during_a_batch_reach_the_throttle_but_not_the_restart() {
        let t = Tracker::new();
        let start = now_ms();
        let id = t.register(&info(100, start, false), target()).id;
        let procs = Fake::new(&[(100, (start / 1000) as u64)]);
        t.poll(None, &procs, &user_of, start + 1_000);
        for _ in 0..MEM_STRIKES + 1 {
            t.check_memory(4_096, false, &|_, _| Some(5_000));
        }
        assert!(t.take_rejoins().is_empty(), "restarts wait for the batch");
        assert_eq!(t.throttle(4_096).0, [id], "the throttle doesn't");
    }

    #[test]
    fn what_is_in_force_reaches_the_instance() {
        let t = Tracker::new();
        let id = t.register(&info(100, now_ms(), false), target()).id;
        let on = Limits { eco: true, ..Limits::default() };
        assert!(t.set_in_force(&[(id.clone(), on)]));
        assert_eq!(t.get(&id).unwrap().applied_limits, on);
        assert!(!t.set_in_force(&[(id.clone(), on)]), "no change");
        assert!(t.set_in_force(&[]), "nothing in force any more");
        assert_eq!(t.get(&id).unwrap().applied_limits, Limits::default());
    }

    #[test]
    fn a_client_over_its_memory_limit_is_restarted_once() {
        let t = Tracker::new();
        let start = now_ms();
        let secs = (start / 1000) as u64;
        let id = t.register(&info(100, start, false), target()).id;
        let procs = Fake::new(&[(100, secs)]);
        t.poll(None, &procs, &user_of, start + 1_000);
        let mb = Mutex::new(5_000u64);
        let read = |_: u32, _: u64| Some(*mb.lock().unwrap());
        t.check_memory(4_096, true, &read);
        t.check_memory(4_096, true, &read);
        assert!(t.take_rejoins().is_empty(), "two samples aren't enough");
        *mb.lock().unwrap() = 1_000;
        t.check_memory(4_096, true, &read);
        *mb.lock().unwrap() = 5_000;
        for _ in 0..MEM_STRIKES {
            t.check_memory(4_096, true, &read);
        }
        t.poll(None, &procs, &user_of, start + 2_000);
        assert_eq!(t.take_rejoins().iter().map(|r| r.cause).collect::<Vec<_>>(), [CrashKind::Mem]);
        assert!(t.take_rejoins().is_empty() && t.still_wants_rejoin(&id));
        t.check_memory(0, true, &read);
        assert!(!t.still_wants_rejoin(&id), "turned off");
        for _ in 0..MEM_STRIKES {
            t.check_memory(4_096, true, &read);
        }

        t.mark_restart(&id, Some(CrashKind::Mem));
        procs.set(&[]);
        t.poll(None, &procs, &user_of, start + 3_000);
        assert_eq!(state(&t, &id).1, note("Closed"));
        let ended = t.take_crashes();
        assert_eq!((ended.len(), ended[0].kind, ended[0].reason.as_deref()), (1, CrashKind::Mem, Some("Commit size reached 5000 MB")));
    }

    #[test]
    fn a_user_close_over_the_memory_limit_is_not_a_memory_restart() {
        let t = Tracker::new();
        let start = now_ms();
        let id = t.register(&info(100, start, false), target()).id;
        let procs = Fake::new(&[(100, (start / 1000) as u64)]);
        t.poll(None, &procs, &user_of, start + 1_000);
        for _ in 0..MEM_STRIKES {
            t.check_memory(4_096, true, &|_, _| Some(5_000));
        }
        t.mark_closing(&id);
        procs.set(&[]);
        t.poll(None, &procs, &user_of, start + 2_000);
        assert!(t.take_crashes().is_empty());
    }

    #[test]
    fn a_replaced_client_ends_at_once() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let id = t.register(&info(100, start, false), target()).id;
        let procs = Fake::new(&[(100, (start / 1000) as u64)]);
        let log = dir.join("0.744.0.7441080_20261024T120000Z_Player_ABCDE_last.log");
        append(&log, &[btid_line(BTID)]);
        t.poll(Some(&dir), &procs, &user_of, start + 1_000);
        append(
            &log,
            &[
                "[FLog::WndProcessCheck] WindowsAppReloadTelemetry EventWaitResultUpdate".into(),
                "[FLog::Systray] Main thread: Received close main window request".into(),
            ],
        );
        procs.set(&[]);
        t.poll(Some(&dir), &procs, &user_of, start + 2_000);
        assert_eq!(state(&t, &id), (Exited, note("Replaced by another Roblox client"), 100));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn arranging_is_for_fresh_launches_that_stay_visible() {
        assert!(arranges(true, true));
        assert!(!arranges(false, true), "the setting is off");
        assert!(!arranges(true, false), "a rejoin keeps its place");
    }
    #[test]
    fn temporary_process_loss_is_visible_and_blocks_duplicate_launches() {
        let dir = temp_dir();
        let start = now_ms();
        let (t, id, procs, _) = in_game_client(&dir, start);
        procs.set(&[]);
        t.poll(Some(&dir), &procs, &user_of, start + 2_000);
        t.poll(Some(&dir), &procs, &user_of, start + 8_000);
        assert!(t.get(&id).unwrap().checking_process);
        assert!(t.claim(1).is_none() && t.take_rejoins().is_empty() && t.take_crashes().is_empty());
        procs.set(&[(100, (start / 1000) as u64)]);
        t.poll(Some(&dir), &procs, &user_of, start + 9_000);
        let i = t.get(&id).unwrap();
        assert_eq!((i.state, i.checking_process, i.ended_at), (InGame, false, None));
        assert_eq!(t.clients().len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recently_ended_exact_client_recovers_but_not_while_a_launch_is_claimed() {
        let dir = temp_dir();
        let start = now_ms();
        let (t, id, procs, _) = in_game_client(&dir, start);
        assert_eq!(t.first_in_game(&|_| true), [100]);
        procs.set(&[]);
        t.poll(Some(&dir), &procs, &user_of, start + 2_000);
        t.poll(Some(&dir), &procs, &user_of, start + 17_000);
        assert_eq!(t.get(&id).unwrap().state, Exited);
        let claim = t.claim(1).unwrap();
        procs.set(&[(100, (start / 1000) as u64)]);
        t.poll(Some(&dir), &procs, &user_of, start + 18_000);
        assert_eq!(t.get(&id).unwrap().state, Exited);
        drop(claim);
        t.poll(Some(&dir), &procs, &user_of, start + 19_000);
        assert_eq!(t.get(&id).unwrap().state, InGame);
        assert!(!t.get(&id).unwrap().checking_process);
        assert_eq!(t.first_in_game(&|_| true), [100], "window actions can be restored");
        assert!(t.claim(1).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ownership_recovery_is_unique_identity_checked_and_never_nearest_window() {
        let procs = Fake::new(&[(101, 100), (102, 100)]);
        let path = PathBuf::from("test-log");
        let tracker = Tracker::new();
        tracker.register(&info(900, 100_000, true), target());
        let mut e = tracker.entries()[0].clone();
        e.log = Some(path.clone());
        e.expected_user = Some(USER);
        let find = |e: &mut Entry, taken: &[u32]| { e.owner_checked_at = None; owned_client(e, &procs, &mut None, taken, 101_000) };
        assert!(find(&mut e, &[]).is_none(), "no ownership proof, no guess");
        procs.owners(&path, &[(101, 100), (102, 100)]);
        assert!(find(&mut e, &[]).is_none(), "ambiguous owners");
        procs.owners(&path, &[(102, 99)]);
        assert!(find(&mut e, &[]).is_none(), "reused PID/start mismatch");
        procs.owners(&path, &[(102, 100)]);
        assert!(find(&mut e, &[102]).is_none(), "belongs to another tracker entry");
        e.facts.user_id = Some(USER + 1);
        assert!(find(&mut e, &[]).is_none(), "wrong account");
        e.facts.user_id = Some(USER);
        assert_eq!(find(&mut e, &[]).unwrap().pid, 102);
    }

    #[test]
    fn strict_ultra_has_no_loading_focus_or_teleport_exemptions() {
        use crate::model::UltraCpu;
        let dir = temp_dir();
        let start = now_ms();
        let (t, id, procs, log) = in_game_client(&dir, start);
        let v = Settings { ultra_cpu: UltraCpu { enabled: true, cap: 1, warmup_s: 90 }, ..Settings::default() };
        for (now, foreground) in [(start + 2_000, None), (start + 3_000, Some(100)), (start + 999_000, None)] {
            assert!(t.cpu_policy(&v, foreground, &[1], now).0.is_empty());
            assert_eq!(t.get(&id).unwrap().cpu_mode, CpuMode::Background);
            assert_eq!(t.get(&id).unwrap().cpu_warmup_remaining_s, 0);
        }
        append(&log, &["[FLog::SingleSurfaceApp] UgcExperienceController: doTeleport".into()]);
        t.poll(Some(&dir), &procs, &user_of, start + 4_000);
        assert!(t.cpu_policy(&v, Some(100), &[1], start + 4_000).0.is_empty());
        assert_eq!(t.get(&id).unwrap().cpu_mode, CpuMode::Background);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn checking_disconnect_does_not_consume_the_later_confirmed_rejoin() {
        let dir = temp_dir();
        let start = now_ms();
        let (t, id, procs, log) = in_game_client(&dir, start);
        procs.set(&[]);
        append(&log, &["[FLog::Network] Disconnection Notification. Reason: 267".into()]);
        t.poll(Some(&dir), &procs, &user_of, start + 2_000);
        assert!(t.get(&id).unwrap().checking_process);
        assert!(t.take_rejoins().is_empty());
        assert!(!t.begin_relaunch(&id, start + 3_000));
        t.poll(Some(&dir), &procs, &user_of, start + 17_000);
        assert_eq!(t.get(&id).unwrap().state, Exited);
        assert_eq!(t.take_rejoins().iter().map(|r| r.cause).collect::<Vec<_>>(), [CrashKind::Disconnect]);
        assert!(t.claim(1).is_some(), "normal launch permitted after confirmed exit");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn per_client_apply_does_not_claim_fps_is_already_changed_or_touch_another_client() {
        let t = Tracker::new();
        let a = t.register(&LaunchInfo { fps_cap: Some(30), ..info(100, 1000, false) }, target()).id;
        let b = t.register(&LaunchInfo { account_id: 2, ..info(200, 1000, false) }, target()).id;
        let v = Settings::default();
        let p = crate::model::ClientPerformance { fps_cap: 5, low_graphics: true, lighter_rendering: true, render_flags: v.render_flags, limits: Limits { cpu_cap: Some(1), ..Default::default() }, strict_cpu: true };
        t.set_performance(&a, p.clone(), true);
        let i = t.get(&a).unwrap();
        assert_eq!(i.fps_cap, Some(30), "current process still has its launch FPS");
        assert!(i.performance_pending);
        assert_eq!(i.limits.cpu_cap, Some(1));
        assert_eq!(t.get(&b).unwrap().performance, None);
        t.set_limits(&a, Limits { cpu_cap: Some(80), ..Default::default() });
        assert_eq!(t.get(&a).unwrap().limits.cpu_cap, Some(2), "manual edit cannot bypass strict ULTRA");
        t.set_performance(&a, p, false);
        assert!(!t.get(&a).unwrap().performance_pending, "launch path can acknowledge launch-time settings");
    }

}
