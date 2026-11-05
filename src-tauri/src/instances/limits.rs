use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static RELEASED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOB_OBJECT_CPU_RATE_CONTROL, JOB_OBJECT_CPU_RATE_CONTROL_ENABLE,
    JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0,
    JobObjectCpuRateControlInformation, SetInformationJobObject,
};
use windows::Win32::System::ProcessStatus::K32EmptyWorkingSet;
use windows::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, GetProcessAffinityMask, MEMORY_PRIORITY_INFORMATION, MEMORY_PRIORITY_NORMAL, MEMORY_PRIORITY_VERY_LOW, SetProcessAffinityMask, NORMAL_PRIORITY_CLASS,
    PROCESS_ACCESS_RIGHTS, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
    PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, PROCESS_SET_QUOTA, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    ProcessMemoryPriority, ProcessPowerThrottling, SetPriorityClass, SetProcessInformation,
};
use windows::core::{BOOL, Owned, PCWSTR};

use super::process::{open, started};
use crate::error::{AppError, Result};
use crate::model::{Limits, Settings};

const FIRST_TRIM: Duration = Duration::from_secs(120);
const REBOUND: Duration = Duration::from_secs(60);
const LOOK: Duration = Duration::from_secs(10);
const SETTLE: Duration = Duration::from_secs(60);
const STEADY_MB: u64 = 25;
const TRIM_GAP: Duration = Duration::from_secs(120);
const GROWTH_MB: u64 = 150;
const BUDGET: u32 = 75;
const FLOOR: u32 = 25;
const TRIM_RETRY: Duration = Duration::from_secs(60);
const TRIM_FOLLOW_SECS: [u64; 3] = [10, 60, 300];

struct TrimFollow {
    why: &'static str,
    pid: u32,
    t0: u64,
    at: Instant,
    done: usize,
}

fn memory_line(r: &super::metrics::Reading) -> String {
    format!("working set {} MB, private working set {} MB, commit {} MB", r.working_set >> 20, r.memory >> 20, r.private >> 20)
}

struct Job(Owned<HANDLE>);
// -- SAFETY: a job handle may be used from any thread; it is only used under the Limiter's lock
unsafe impl Send for Job {}

struct Applied {
    pid: u32,
    t0: u64,
    job: Option<Job>,
    limits: Limits,
    next_trim: Instant,
    last_trim: Option<Instant>,
    baseline: Option<u64>,
    steady: Option<(Instant, u64)>,
    look_at: Instant,
    gap: Duration,
    regrew: bool,
    failed: Option<Limits>,
    cap_failed: bool,
}

impl Applied {
    fn new(pid: u32, t0: u64) -> Applied {
        let now = Instant::now();
        Applied { pid, t0, job: None, limits: Limits::default(), next_trim: now, last_trim: None, baseline: None, steady: None, look_at: now, gap: TRIM_GAP, regrew: false, failed: None, cap_failed: false }
    }

    fn trimmed(&mut self, now: Instant, every: u32) {
        self.next_trim = now + Duration::from_secs(u64::from(every) * 60);
        self.last_trim = Some(now);
        self.look_at = now + REBOUND;
        self.forget_memory();
        self.regrew = false;
    }

    fn forget_memory(&mut self) {
        self.baseline = None;
        self.steady = None;
    }

    fn set_job_cap(&mut self, cap: Option<u8>) -> Result<()> {
        if let Some(job) = &self.job {
            return set_cap(job, cap);
        }
        let (pid, t0) = (self.pid, self.t0);
        let h = client(pid, t0, PROCESS_SET_QUOTA | PROCESS_TERMINATE)?;
        let job = new_job()?;
        set_cap(&job, cap)?;
        // -- SAFETY: both handles are live; the process handle closes on drop
        unsafe { AssignProcessToJobObject(*job.0, *h) }.map_err(|e| failed(pid, e))?;
        self.job = Some(job);
        Ok(())
    }
}

pub struct Limiter {
    applied: Mutex<HashMap<String, Applied>>,
    free: Mutex<(Vec<String>, Option<u32>)>,
    follows: Mutex<Vec<TrimFollow>>,
}

impl Limiter {
    pub fn new() -> Limiter {
        Limiter { applied: Mutex::new(HashMap::new()), free: Mutex::new((Vec::new(), None)), follows: Mutex::new(Vec::new()) }
    }

    pub fn logged_trim(&self, why: &'static str, pid: u32, t0: u64) -> Result<(u64, u64)> {
        let read = || super::metrics::read(pid, t0);
        let before = read();
        trim(pid, t0).inspect_err(|e| log::warn!("trim of client {pid} ({why}) failed: {e}"))?;
        let after = read();
        let line = |r: &Option<super::metrics::Reading>| r.as_ref().map_or_else(|| "unreadable".to_owned(), memory_line);
        log::info!("trim of client {pid} ({why}): before: {}; right after: {}", line(&before), line(&after));
        self.follows.lock().unwrap_or_else(|e| e.into_inner()).push(TrimFollow { why, pid, t0, at: Instant::now(), done: 0 });
        let mb = |r: Option<super::metrics::Reading>| r.map_or(0, |r| r.memory >> 20);
        Ok((mb(before), mb(after)))
    }

    fn follow_trims(&self, now: Instant, read: impl Fn(u32, u64) -> Option<super::metrics::Reading>) {
        self.follows.lock().unwrap_or_else(|e| e.into_inner()).retain_mut(|f| {
            let secs = TRIM_FOLLOW_SECS[f.done];
            if now < f.at + Duration::from_secs(secs) {
                return true;
            }
            let Some(r) = read(f.pid, f.t0) else { return false };
            log::info!("trim of client {} ({}): {secs} s later: {}", f.pid, f.why, memory_line(&r));
            f.done += 1;
            f.done < TRIM_FOLLOW_SECS.len()
        });
    }

    fn trim_failed(&self, pid: u32, t0: u64, now: Instant) {
        let mut map = self.applied.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(a) = map.values_mut().find(|a| (a.pid, a.t0) == (pid, t0)) {
            a.next_trim = now + TRIM_RETRY;
        }
    }

    pub fn in_force(&self) -> Vec<(String, Limits)> {
        self.applied.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(id, a)| (id.clone(), a.limits)).collect()
    }

    pub fn apply(&self, id: &str, pid: u32, t0: u64, limits: Limits) -> Result<()> {
        let mut map = self.applied.lock().unwrap_or_else(|e| e.into_inner());
        let a = map.entry(id.to_owned()).or_insert_with(|| Applied::new(pid, t0));
        if (a.pid, a.t0) != (pid, t0) {
            *a = Applied::new(pid, t0);
        }
        let mut r = Ok(());
        if limits.cpu_cap.is_none() {
            a.cap_failed = false;
        }
        if limits.cpu_cap != a.limits.cpu_cap {
            let done = a.set_job_cap(limits.cpu_cap);
            a.cap_failed = done.is_err();
            if done.is_ok() {
                a.limits.cpu_cap = limits.cpu_cap;
            }
            r = r.and(done);
        }
        if limits.eco != a.limits.eco {
            let done = set_eco(pid, t0, limits.eco);
            if done.is_ok() {
                a.limits.eco = limits.eco;
            }
            r = r.and(done);
        }
        if limits.affinity != a.limits.affinity {
            let done = set_affinity(pid, t0, limits.affinity);
            if done.is_ok() {
                a.limits.affinity = limits.affinity;
            }
            r = r.and(done);
        }
        if limits.trim_every_min != a.limits.trim_every_min {
            let now = Instant::now();
            a.limits.trim_every_min = limits.trim_every_min;
            a.next_trim = next_trim(now, a.last_trim, limits.trim_every_min);
        }
        r
    }

    pub fn uncapped(&self) -> Vec<String> {
        self.applied.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|(_, a)| a.cap_failed).map(|(id, _)| id.clone()).collect()
    }

    fn due_trim(&self, now: Instant) -> Option<(u32, u64)> {
        let mut map = self.applied.lock().unwrap_or_else(|e| e.into_inner());
        map.values_mut().find_map(|a| {
            let m = a.limits.trim_every_min.filter(|&m| m > 0 && now >= a.next_trim)?;
            if !a.regrew {
                a.gap = TRIM_GAP;
            }
            a.trimmed(now, m);
            Some((a.pid, a.t0))
        })
    }

    fn grown_trim(&self, now: Instant, free: impl Fn(&str, u32) -> bool, read: impl Fn(u32, u64) -> Option<u64>) -> Option<(u32, u64)> {
        let mut map = self.applied.lock().unwrap_or_else(|e| e.into_inner());
        for (id, a) in map.iter_mut() {
            let (Some(m), Some(last)) = (a.limits.trim_every_min.filter(|&m| m > 0), a.last_trim) else { continue };
            if free(id, a.pid) {
                a.forget_memory();
                a.look_at = now + FIRST_TRIM;
                continue;
            }
            if now < a.look_at {
                continue;
            }
            a.look_at = now + LOOK;
            let Some(memory) = read(a.pid, a.t0) else { continue };
            let steady = |base: u64| memory.abs_diff(base) <= (base / 20).max(STEADY_MB * 1024 * 1024);
            match a.baseline {
                Some(base) if grew(memory, base) => {
                    if now >= last + a.gap {
                        a.gap = (a.gap * 2).min(Duration::from_secs(u64::from(m) * 60));
                        a.trimmed(now, m);
                        return Some((a.pid, a.t0));
                    }
                    a.regrew = true;
                }
                Some(base) if memory < base && !steady(base) => a.forget_memory(),
                Some(_) => {}
                None => match a.steady {
                    Some((since, from)) if steady(from) => {
                        if now >= since + SETTLE {
                            a.baseline = Some(memory);
                        }
                    }
                    _ => a.steady = Some((now, memory)),
                },
            }
        }
        None
    }

    fn note_free(&self, ids: &[String], front: Option<u32>) {
        *self.free.lock().unwrap_or_else(|e| e.into_inner()) = (ids.to_vec(), front);
    }

    pub fn release_all(&self) {
        RELEASED.store(true, std::sync::atomic::Ordering::Relaxed);
        let applied = std::mem::take(&mut *self.applied.lock().unwrap_or_else(|e| e.into_inner()));
        for (id, a) in applied {
            let mut r = Ok(());
            if let Some(job) = a.job.as_ref().filter(|_| a.limits.cpu_cap.is_some()) {
                r = r.and(set_cap(job, None).inspect_err(|e| log::debug!("instance {id}: CPU cap not taken off: {e}")));
            }
            if a.limits.eco {
                r = r.and(set_eco(a.pid, a.t0, false).inspect_err(|e| log::debug!("instance {id}: efficiency mode not taken off: {e}")));
            }
            if a.limits.affinity.is_some() {
                r = r.and(set_affinity(a.pid, a.t0, None).inspect_err(|e| log::debug!("instance {id}: cores not given back: {e}")));
            }
            if let Err(e) = r {
                log::warn!("could not take the limits off instance {id}: {e}");
            }
        }
    }

    pub fn sync(&self, clients: &[(String, u32, u64, Limits)]) {
        if RELEASED.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        self.applied.lock().unwrap_or_else(|e| e.into_inner()).retain(|id, _| clients.iter().any(|c| c.0 == *id));
        for (id, pid, t0, limits) in clients {
            let failed = {
                let map = self.applied.lock().unwrap_or_else(|e| e.into_inner());
                map.get(id).is_some_and(|a| a.pid == *pid && a.failed == Some(*limits))
            };
            if failed {
                continue;
            }
            let r = self.apply(id, *pid, *t0, *limits);
            if let Err(e) = &r {
                log::warn!("could not apply limits to instance {id}: {e}");
            }
            if let Some(a) = self.applied.lock().unwrap_or_else(|e| e.into_inner()).get_mut(id) {
                a.failed = r.is_err().then_some(*limits);
            }
        }
        let now = Instant::now();
        let free = self.free.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let is_free = |id: &str, pid: u32| free.1 == Some(pid) || free.0.iter().any(|f| f == id);
        if let Some((pid, t0)) = self.due_trim(now).or_else(|| self.grown_trim(now, is_free, |pid, t0| super::metrics::read(pid, t0).map(|r| r.memory)))
            && self.logged_trim("scheduled", pid, t0).is_err()
        {
            self.trim_failed(pid, t0, now);
        }
        self.follow_trims(now, super::metrics::read);
    }
}

pub fn reset_followed(pid: u32, t0: u64) {
    if let Err(e) = set_eco(pid, t0, false).and_then(|()| set_affinity(pid, t0, None)) {
        log::warn!("could not reset the limits of client {pid}: {e}");
    }
    let Ok(h) = client(pid, t0, PROCESS_QUERY_LIMITED_INFORMATION) else { return };
    let mut inside = BOOL(0);
    // -- SAFETY: `h` is live; `inside` is a local
    if unsafe { IsProcessInJob(*h, None, &mut inside) }.is_ok() && inside.as_bool() {
        log::info!("client {pid} is already in a job: a CPU cap from before LRAM restarted may stay on until it closes");
    }
}

pub fn plan_now(s: &crate::App, v: &Settings, clients: &[(String, u32, u64, Limits)], throttled: &[String]) -> Vec<(String, u32, u64, Limits)> {
    let full_quality = s.db.full_quality_accounts().unwrap_or_else(|e| {
        log::debug!("could not read the Full quality accounts: {e}");
        Vec::new()
    });
    let now = crate::commands::now_ms();
    let mut free = s.tracker.free(&full_quality, now);
    let front = super::process::foreground_pid();
    let (ultra_free, _) = s.tracker.cpu_policy(v, front, &full_quality, now);
    free.extend(ultra_free.iter().cloned());
    s.limiter.note_free(&free, front);
    let mut result = plan(clients, v, front, crate::commands::cores(), throttled, &free, &s.limiter.uncapped());
    for (id, _, _, limits) in &mut result {
        if let Some(own) = s.tracker.get(id).and_then(|i| i.performance) {
            *limits = own.limits;
            if own.strict_cpu { limits.cpu_cap = Some(limits.cpu_cap.unwrap_or(1).clamp(1, 2)); }
        }
        release_startup_cpu(limits, ultra_free.contains(id));
    }
    result
}

// Last policy layer: no profile, manual cap, fair cap or memory throttle may
// reintroduce a CPU rate limit/Eco while loading or during the universal grace.
fn release_startup_cpu(limits: &mut Limits, released: bool) {
    if released { limits.cpu_cap = None; limits.eco = false; }
}

// -- Main func
pub fn plan(
    clients: &[(String, u32, u64, Limits)],
    v: &Settings,
    foreground: Option<u32>,
    cores: u32,
    throttled: &[String],
    free: &[String],
    uncapped: &[String],
) -> Vec<(String, u32, u64, Limits)> {
    let mut policy = v.clone();
    if v.ultra_cpu.enabled {
        policy.fair_cap = false;
        policy.adaptive.enabled = true;
        policy.adaptive.background.cpu_cap = Some(v.ultra_cpu.cap);
    }
    let v = &policy;
    let n = clients.len() as u32;
    let fair = v.fair_cap.then(|| fair_cap(n, cores));
    clients
        .iter()
        .enumerate()
        .map(|(i, (id, pid, t0, manual))| {
            let i = i as u32;
            let share = match v.auto_spread && n > 1 {
                true if uncapped.contains(id) => Some(pin(i, n, cores)),
                true => spread(i, n, cores),
                false => None,
            };
            let mut l = effective(*manual, foreground == Some(*pid) || free.contains(id), v, share, fair);
            if v.mem_throttle.mb > 0 && throttled.contains(id) {
                l.cpu_cap = Some(throttle_cap(l.cpu_cap, v.mem_throttle.cpu_cap));
            }
            if v.ultra_cpu.enabled {
                l.cpu_cap = Some(l.cpu_cap.unwrap_or(v.ultra_cpu.cap).min(v.ultra_cpu.cap.clamp(1, 2)));
                l.eco = true;
            }
            (id.clone(), *pid, *t0, l)
        })
        .collect()
}

pub fn effective(manual: Limits, free: bool, v: &Settings, share: Option<u64>, fair: Option<u8>) -> Limits {
    if free {
        return manual;
    }
    let bg = if v.adaptive.enabled { v.adaptive.background } else { Limits::default() };
    let mut l = Limits {
        cpu_cap: manual.cpu_cap.or(bg.cpu_cap),
        eco: manual.eco || bg.eco,
        affinity: manual.affinity.or(bg.affinity),
        trim_every_min: manual.trim_every_min.or(bg.trim_every_min),
    };
    if l.trim_every_min.is_none() && v.trim_every_min > 0 {
        l.trim_every_min = Some(v.trim_every_min);
    }
    if l.affinity.is_none() {
        l.affinity = share;
    }
    if let Some(f) = fair.filter(|_| manual.cpu_cap.is_none()) {
        l.cpu_cap = Some(throttle_cap(l.cpu_cap, f));
    }
    l
}

pub fn throttle_cap(own: Option<u8>, throttle: u8) -> u8 {
    own.map_or(throttle, |c| c.min(throttle))
}

pub fn fair_cap(n: u32, cores: u32) -> u8 {
    (BUDGET / n.max(1)).max(FLOOR.div_ceil(cores.max(1))).clamp(1, 100) as u8
}

fn next_trim(now: Instant, last: Option<Instant>, every: Option<u32>) -> Instant {
    let settled = now + FIRST_TRIM;
    match (last, every) {
        (Some(t), Some(m)) => settled.max(t + Duration::from_secs(u64::from(m) * 60)),
        _ => settled,
    }
}

fn grew(memory: u64, base: u64) -> bool {
    memory > base + (base / 4).max(GROWTH_MB * 1024 * 1024)
}

pub fn spread(i: u32, n: u32, cores: u32) -> Option<u64> {
    (n.saturating_mul(2) <= cores.clamp(1, 64)).then(|| pin(i, n, cores))
}

fn pin(i: u32, n: u32, cores: u32) -> u64 {
    let cores = cores.clamp(1, 64);
    if n >= cores {
        return 1 << (i % cores);
    }
    (0..cores).filter(|c| c % n == i % n).fold(0, |m, c| m | 1 << c)
}

pub fn clamp_affinity(mask: u64, system: u64) -> Option<u64> {
    Some(mask & system).filter(|&m| m != 0)
}

pub fn trim(pid: u32, t0: u64) -> Result<()> {
    let h = client(pid, t0, PROCESS_SET_QUOTA)?;
    // -- SAFETY: `h` is live with PROCESS_SET_QUOTA | PROCESS_QUERY_LIMITED_INFORMATION
    unsafe { K32EmptyWorkingSet(*h) }
        .ok()
        .map_err(|e| AppError::Internal(tr!("Could not trim Roblox (process {pid}): {why}", pid = pid, why = crate::error::windows_error(&e))))
}

fn set_affinity(pid: u32, t0: u64, mask: Option<u64>) -> Result<()> {
    let h = client(pid, t0, PROCESS_SET_INFORMATION)?;
    let (mut own, mut system) = (0usize, 0usize);
    // -- SAFETY: `h` is live with PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION; the out pointers are locals
    unsafe { GetProcessAffinityMask(*h, &mut own, &mut system) }.map_err(|e| failed(pid, e))?;
    let mask = match mask {
        Some(m) => clamp_affinity(m, system as u64).ok_or_else(|| AppError::Invalid(tr!("None of those CPU cores exist here.")))?,
        None => system as u64,
    };
    // -- SAFETY: as above; `mask` is a subset of the system mask
    unsafe { SetProcessAffinityMask(*h, mask as usize) }.map_err(|e| failed(pid, e))
}

fn new_job() -> Result<Job> {
    // -- SAFETY: default security, no name; Owned closes the handle on drop
    unsafe { CreateJobObjectW(None, PCWSTR::null()) }.map(|h| Job(unsafe { Owned::new(h) })).map_err(|e| AppError::Internal(crate::error::windows_error(&e)))
}

fn set_cap(job: &Job, cap: Option<u8>) -> Result<()> {
    let info = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
        ControlFlags: match cap {
            Some(_) => JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
            None => JOB_OBJECT_CPU_RATE_CONTROL(0),
        },
        Anonymous: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0 { CpuRate: u32::from(cap.unwrap_or(100)) * 100 },
    };
    // -- SAFETY: `info` is a local of the size passed
    unsafe {
        SetInformationJobObject(
            *job.0,
            JobObjectCpuRateControlInformation,
            (&raw const info).cast(),
            size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as u32,
        )
    }
    .map_err(|e| AppError::Internal(tr!("Could not set the CPU limit: {why}", why = crate::error::windows_error(&e))))
}

fn set_eco(pid: u32, t0: u64, on: bool) -> Result<()> {
    let h = client(pid, t0, PROCESS_SET_INFORMATION)?;
    let throttling = |mask: u32| {
        let state = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ControlMask: mask, StateMask: mask };
        // -- SAFETY: `h` is live with PROCESS_SET_INFORMATION; `state` is a local of the size passed
        unsafe { SetProcessInformation(*h, ProcessPowerThrottling, (&raw const state).cast(), size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32) }
    };
    let memory = MEMORY_PRIORITY_INFORMATION { MemoryPriority: if on { MEMORY_PRIORITY_VERY_LOW } else { MEMORY_PRIORITY_NORMAL } };
    // -- SAFETY: as above, for `memory`
    let memory = unsafe { SetProcessInformation(*h, ProcessMemoryPriority, (&raw const memory).cast(), size_of::<MEMORY_PRIORITY_INFORMATION>() as u32) };
    match memory {
        Err(e) if on => return Err(failed(pid, e)),
        Err(e) => log::warn!("process {pid}: memory priority not put back: {e}"),
        Ok(()) => {}
    }
    let done = match on {
        true => throttling(PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION).or_else(|e| {
            log::debug!("process {pid}: timer resolution requests not ignored: {e}");
            throttling(PROCESS_POWER_THROTTLING_EXECUTION_SPEED)
        }),
        false => throttling(0),
    };
    done.map_err(|e| failed(pid, e))?;
    // -- SAFETY: as above
    unsafe { SetPriorityClass(*h, if on { BELOW_NORMAL_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS }) }.map_err(|e| failed(pid, e))
}

fn client(pid: u32, t0: u64, rights: PROCESS_ACCESS_RIGHTS) -> Result<Owned<HANDLE>> {
    let h = open(pid, rights | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE)
        .ok_or_else(|| AppError::Internal(tr!("Could not reach Roblox (process {pid}).", pid = pid)))?;
    if started(*h) != Some(t0) {
        return Err(AppError::Invalid(tr!("That client has closed.")));
    }
    Ok(h)
}

fn failed(pid: u32, e: windows::core::Error) -> AppError {
    AppError::Internal(tr!("Could not limit Roblox (process {pid}): {why}", pid = pid, why = crate::error::windows_error(&e)))
}

#[cfg(test)]
mod tests {
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command, Stdio};

    use windows::Win32::System::Threading::GetProcessInformation;

    use super::*;
    use crate::instances::process::start_time;

    fn ping() -> (Child, u32, u64) {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let child = Command::new(std::path::Path::new(&root).join(r"System32\PING.EXE"))
            .args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .unwrap();
        let (pid, t0) = (child.id(), start_time(child.id()).unwrap());
        (child, pid, t0)
    }

    #[test]
    fn universal_grace_wins_over_manual_profile_fair_and_memory_caps() {
        for cap in [1, 2, 25, 100] {
            let mut limits = Limits { cpu_cap: Some(cap), eco: true, affinity: Some(3), trim_every_min: Some(5) };
            release_startup_cpu(&mut limits, true);
            assert_eq!((limits.cpu_cap, limits.eco), (None, false));
            assert_eq!((limits.affinity, limits.trim_every_min), (Some(3), Some(5)));
            release_startup_cpu(&mut limits, false);
            assert_eq!(limits.cpu_cap, None);
        }
    }

    #[test]
    fn caps_and_frees_a_process() {
        let (mut child, pid, t0) = ping();
        let l = Limiter::new();
        let capped = Limits { cpu_cap: Some(15), eco: true, ..Limits::default() };
        l.apply("i", pid, t0, capped).unwrap();
        l.apply("i", pid, t0, Limits { cpu_cap: Some(50), ..Limits::default() }).unwrap();
        l.apply("i", pid, t0, Limits { affinity: Some(1), trim_every_min: Some(5), ..Limits::default() }).unwrap();
        assert_eq!(affinity(pid), 1);
        trim(pid, t0).unwrap();
        assert!(trim(pid, t0 - 10).is_err(), "a reused pid must not be trimmed");
        l.apply("i", pid, t0, Limits::default()).unwrap();
        assert_eq!(affinity(pid), system(), "back to every core");
        if system().count_ones() < 64 {
            assert!(set_affinity(pid, t0, Some(1 << 63)).is_err(), "only cores that don't exist");
        }
        assert!(l.apply("j", pid, t0 - 10, capped).is_err(), "a reused pid must not be limited");
        l.sync(&[]);
        assert!(l.applied.lock().unwrap().is_empty());
        l.apply("i", pid, t0, Limits { cpu_cap: Some(15), eco: true, affinity: Some(1), ..Limits::default() }).unwrap();
        l.release_all();
        assert_eq!(affinity(pid), system(), "every core again at exit");
        assert!(l.applied.lock().unwrap().is_empty());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    fn system() -> u64 {
        let (mut own, mut system) = (0usize, 0usize);
        // -- SAFETY: the pseudo handle of this process; the out pointers are locals
        unsafe { GetProcessAffinityMask(windows::Win32::System::Threading::GetCurrentProcess(), &mut own, &mut system) }.unwrap();
        system as u64
    }

    fn affinity(pid: u32) -> u64 {
        let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION).unwrap();
        let (mut own, mut system) = (0usize, 0usize);
        // -- SAFETY: `h` is live with PROCESS_QUERY_LIMITED_INFORMATION; the out pointers are locals
        unsafe { GetProcessAffinityMask(*h, &mut own, &mut system) }.unwrap();
        own as u64
    }

    fn throttling(pid: u32) -> (u32, u32) {
        let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION).unwrap();
        let mut s = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ControlMask: 0, StateMask: 0 };
        // -- SAFETY: `h` is live with that right; `s` is a local of the size passed
        unsafe { GetProcessInformation(*h, ProcessPowerThrottling, (&raw mut s).cast(), size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32) }.unwrap();
        (s.ControlMask, s.StateMask)
    }

    #[test]
    fn efficiency_mode_holds_and_goes_back_to_the_system() {
        let (mut child, pid, t0) = ping();
        set_eco(pid, t0, true).unwrap();
        let both = PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION;
        assert_eq!(throttling(pid), (both, both));
        set_eco(pid, t0, false).unwrap();
        assert_eq!(throttling(pid), (0, 0));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn a_followed_client_gets_its_efficiency_mode_and_cores_back() {
        let (mut child, pid, t0) = ping();
        Limiter::new().apply("i", pid, t0, Limits { eco: true, affinity: Some(1), ..Limits::default() }).unwrap();
        reset_followed(pid, t0);
        assert_eq!(throttling(pid), (0, 0));
        assert_eq!(affinity(pid), system());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn a_client_back_from_the_front_keeps_its_trim_schedule() {
        let now = Instant::now();
        assert_eq!(next_trim(now, None, Some(15)), now + FIRST_TRIM, "never trimmed");
        let an_hour_ago = now.checked_sub(Duration::from_secs(3600)).unwrap();
        assert_eq!(next_trim(now, Some(an_hour_ago), Some(15)), now + FIRST_TRIM, "overdue, but it settles first");
        assert_eq!(next_trim(now, Some(now), Some(15)), now + Duration::from_secs(15 * 60), "trimmed just now");
        assert_eq!(next_trim(now, Some(now), None), now + FIRST_TRIM);
    }

    #[test]
    fn one_trim_per_sync_and_the_first_after_settling() {
        let l = Limiter::new();
        let every = |m| Limits { trim_every_min: Some(m), ..Limits::default() };
        let now = Instant::now();
        {
            let mut map = l.applied.lock().unwrap();
            for (id, pid, limits) in [("a", 1, every(15)), ("b", 2, every(15)), ("off", 3, every(0)), ("none", 4, Limits::default())] {
                map.insert(id.to_owned(), Applied { limits, next_trim: now, ..Applied::new(pid, 1) });
            }
        }
        let first = l.due_trim(now).unwrap().0;
        let second = l.due_trim(now).unwrap().0;
        assert!(first != second && first <= 2 && second <= 2, "one each, one a call");
        assert_eq!(l.due_trim(now), None, "not again until its interval has passed");
        assert!(l.due_trim(now + Duration::from_secs(15 * 60)).is_some());

        let (mut child, pid, t0) = ping();
        let l = Limiter::new();
        l.apply("i", pid, t0, every(1)).unwrap();
        let now = Instant::now();
        assert_eq!(l.due_trim(now), None);
        assert_eq!(l.due_trim(now + FIRST_TRIM), Some((pid, t0)));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn memory_growth_trims_between_the_scheduled_ones() {
        const MB: u64 = 1024 * 1024;
        assert!(!grew(300 * MB, 200 * MB), "under 150 MB");
        assert!(grew(351 * MB, 200 * MB));
        assert!(!grew(1200 * MB, 1000 * MB), "under a quarter");
        assert!(grew(1251 * MB, 1000 * MB));

        let l = Limiter::new();
        let start = Instant::now();
        let every = Limits { trim_every_min: Some(15), ..Limits::default() };
        l.applied.lock().unwrap().insert("a".to_owned(), Applied { limits: every, next_trim: start + Duration::from_secs(3600), ..Applied::new(1, 1) });
        let at = |s: u64| start + Duration::from_secs(s);
        let busy = |_: &str, _: u32| false;
        let grown = |s: u64, mb: u64| l.grown_trim(at(s), busy, |_, _| Some(mb * MB));
        let settle = |s: u64, mb: u64| {
            assert_eq!(grown(s, mb), None);
            assert_eq!(grown(s + 60, mb), None);
            assert_eq!(l.applied.lock().unwrap()["a"].baseline, Some(mb * MB));
        };

        assert_eq!(grown(0, 900), None, "never trimmed yet: the schedule goes first");
        l.applied.lock().unwrap().get_mut("a").unwrap().trimmed(at(0), 15);
        assert_eq!(l.grown_trim(at(30), busy, |_, _| panic!("read while it gets its memory back")), None);

        for (s, mb) in [(60, 100), (70, 200), (80, 300), (90, 400), (100, 500), (110, 600), (120, 700)] {
            assert_eq!(grown(s, mb), None);
        }
        assert_eq!(l.applied.lock().unwrap()["a"].baseline, None);

        settle(130, 700);
        assert_eq!(l.grown_trim(at(195), busy, |_, _| panic!("read before the next look")), None);
        assert_eq!(grown(200, 850), None, "under a quarter");
        assert_eq!(grown(210, 850), None);
        assert_eq!(grown(220, 50), None);
        assert_eq!(l.applied.lock().unwrap()["a"].baseline, None);
        assert_eq!(grown(230, 900), None, "not growth");

        settle(240, 700);
        assert_eq!(grown(310, 1000), Some((1, 1)), "grown, 2 min after the last");
        assert_eq!(l.applied.lock().unwrap()["a"].gap, Duration::from_secs(240));

        settle(370, 700);
        assert_eq!(grown(440, 1000), None, "4 min, not 2");
        assert!(l.applied.lock().unwrap()["a"].regrew);
        assert_eq!(grown(550, 1000), Some((1, 1)));
        assert_eq!(l.applied.lock().unwrap()["a"].gap, Duration::from_secs(480));
        assert_eq!(l.applied.lock().unwrap()["a"].next_trim, at(550 + 15 * 60), "the schedule starts over");
        assert!(!l.applied.lock().unwrap()["a"].regrew);

        settle(610, 700);
        assert_eq!(grown(680, 1000), None);
        assert_eq!(l.due_trim(at(1450)), Some((1, 1)));
        assert_eq!(l.applied.lock().unwrap()["a"].gap, Duration::from_secs(480));
        assert_eq!(l.due_trim(at(2350)), Some((1, 1)));
        assert_eq!(l.applied.lock().unwrap()["a"].gap, TRIM_GAP);

        settle(2410, 700);
        assert_eq!(l.grown_trim(at(2480), |_, _| true, |_, _| panic!("free")), None);
        assert_eq!(l.applied.lock().unwrap()["a"].baseline, None);
        assert_eq!(l.grown_trim(at(2490), busy, |_, _| panic!("settling after the front")), None);

        l.applied.lock().unwrap().get_mut("a").unwrap().limits.trim_every_min = None;
        assert_eq!(l.grown_trim(at(9000), busy, |_, _| panic!("no trim set")), None);
    }

    #[test]
    fn a_refused_cap_keeps_the_other_limits_and_the_pin() {
        let (mut child, pid, t0) = ping();
        let l = Limiter::new();
        let bad = Limits { cpu_cap: Some(0), affinity: Some(1), trim_every_min: Some(5), eco: true };
        assert!(l.apply("i", pid, t0, bad).is_err());
        assert_eq!(l.uncapped(), ["i"]);
        assert_eq!(affinity(pid), 1, "cores set all the same");
        {
            let map = l.applied.lock().unwrap();
            let a = &map["i"];
            assert!(a.limits.eco && a.limits.cpu_cap.is_none() && a.limits.trim_every_min == Some(5));
        }
        l.apply("i", pid, t0, Limits { cpu_cap: None, ..bad }).unwrap();
        assert!(l.uncapped().is_empty());
        assert!(l.apply("i", pid, t0, bad).is_err());
        l.apply("i", pid, t0, Limits { cpu_cap: Some(50), ..bad }).unwrap();
        assert!(l.uncapped().is_empty());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn affinity_masks_are_clamped() {
        assert_eq!(clamp_affinity(0b1111, 0b0011), Some(0b0011));
        assert_eq!(clamp_affinity(0b1100, 0b0011), None, "no such core");
        assert_eq!(clamp_affinity(0, u64::MAX), None);
        assert_eq!(clamp_affinity(u64::MAX, u64::MAX), Some(u64::MAX));
    }

    #[test]
    fn auto_spread_shares_cores_round_robin() {
        assert_eq!(spread(0, 2, 8), Some(0b0101_0101));
        assert_eq!(spread(1, 2, 8), Some(0b1010_1010));
        assert_eq!(spread(0, 3, 6), Some(0b00_1001));
        assert_eq!((spread(2, 3, 4), spread(0, 3, 5), spread(4, 6, 4), spread(0, u32::MAX, 8)), (None, None, None, None));
        assert_eq!(spread(1, 32, 200), Some(0b10 | 1 << 33), "at most 64 cores");
        assert_eq!(spread(1, 33, 200), None);
        let all: Vec<u64> = (0..3).map(|i| spread(i, 3, 12).unwrap()).collect();
        assert_eq!(all.iter().fold(0, |m, s| m | s), 0xFFF);
        assert_eq!(all.iter().map(|s| s.count_ones()).sum::<u32>(), 12);
        assert_eq!(pin(2, 3, 4), 0b0100);
        assert_eq!(pin(0, 3, 4), 0b1001);
        assert_eq!((pin(0, 6, 4), pin(4, 6, 4), pin(5, 6, 4)), (1, 1, 2));
        assert_eq!(pin(63, 100, 200), 1 << 63, "at most 64 cores");
    }

    #[test]
    fn a_failed_trim_comes_again_soon() {
        let limiter = Limiter::new();
        let mut a = Applied::new(7, 9);
        a.limits.trim_every_min = Some(60);
        let now = Instant::now();
        a.next_trim = now;
        limiter.applied.lock().unwrap().insert("i".into(), a);
        assert_eq!(limiter.due_trim(now), Some((7, 9)));
        assert_eq!(limiter.due_trim(now + TRIM_RETRY), None, "the next is an interval away");
        limiter.trim_failed(7, 9, now);
        assert_eq!(limiter.due_trim(now + TRIM_RETRY), Some((7, 9)), "sooner after a failure");
    }

    #[test]
    fn a_trim_is_followed_until_its_last_measurement() {
        let limiter = Limiter::new();
        let at = Instant::now();
        limiter.follows.lock().unwrap().push(TrimFollow { why: "test", pid: 1, t0: 2, at, done: 0 });
        let r = super::super::metrics::Reading { cpu: 0, memory: 2 << 20, working_set: 3 << 20, private: 5 << 20, read: 0, write: 0 };
        assert_eq!(memory_line(&r), "working set 3 MB, private working set 2 MB, commit 5 MB");
        let left = || limiter.follows.lock().unwrap().first().map(|f| f.done);
        limiter.follow_trims(at + Duration::from_secs(9), |_, _| Some(r));
        assert_eq!(left(), Some(0), "not yet");
        for (secs, done) in [(10, Some(1)), (60, Some(2)), (300, None)] {
            limiter.follow_trims(at + Duration::from_secs(secs), |_, _| Some(r));
            assert_eq!(left(), done, "at {secs} s");
        }
        limiter.follows.lock().unwrap().push(TrimFollow { why: "test", pid: 1, t0: 2, at, done: 0 });
        limiter.follow_trims(at + Duration::from_secs(10), |_, _| None);
        assert_eq!(left(), None);
    }

    #[test]
    fn the_fair_cap_divides_the_budget_above_a_floor() {
        assert_eq!(fair_cap(1, 8), 75);
        assert_eq!(fair_cap(3, 8), 25);
        assert_eq!(fair_cap(10, 8), 7);
        assert_eq!(fair_cap(50, 8), 4, "at least a quarter of a core: 3 % of 8 would be less");
        assert_eq!(fair_cap(50, 16), 2);
        assert_eq!(fair_cap(500, 64), 1, "never 0: Windows has no such rate");
        assert_eq!((fair_cap(0, 1), fair_cap(200, 1)), (75, 25));
    }

    #[test]
    fn adaptive_limits_background_clients_and_manual_ones_win() {
        let bg = Limits { cpu_cap: Some(25), eco: true, ..Limits::default() };
        let mut v = Settings { adaptive: crate::model::Adaptive { enabled: true, background: bg }, ..Settings::default() };
        let none = Limits::default();
        let mine = Limits { cpu_cap: Some(50), ..Limits::default() };
        assert_eq!(effective(none, false, &v, None, None), bg, "in the background");
        assert_eq!(effective(none, true, &v, None, None), none, "in front: lifted");
        assert_eq!(effective(mine, false, &v, None, None), Limits { cpu_cap: Some(50), eco: true, ..Limits::default() });
        let cores = Limits { affinity: Some(0b11), ..Limits::default() };
        assert_eq!(effective(cores, false, &v, None, None), Limits { cpu_cap: Some(25), eco: true, affinity: Some(0b11), ..Limits::default() });
        v.adaptive.enabled = false;
        assert_eq!(effective(mine, false, &v, None, None), mine, "adaptive off");
        assert_eq!(effective(none, false, &v, None, None), none, "off");

        v.trim_every_min = 15;
        let filled = effective(none, false, &v, Some(0b11), None);
        assert_eq!((filled.trim_every_min, filled.affinity), (Some(15), Some(0b11)));
        let own = Limits { trim_every_min: Some(0), affinity: Some(0b100), ..Limits::default() };
        assert_eq!(effective(own, false, &v, Some(0b11), None), own, "its own trim (off) and cores");

        assert_eq!(effective(none, true, &v, Some(0b11), Some(5)), none);
        assert_eq!(effective(mine, true, &v, Some(0b11), Some(5)), mine);

        assert_eq!(effective(none, false, &v, None, Some(5)).cpu_cap, Some(5));
        assert_eq!(effective(mine, false, &v, None, Some(5)).cpu_cap, Some(50));
        v.adaptive.enabled = true;
        assert_eq!(effective(none, false, &v, None, Some(5)).cpu_cap, Some(5));
        assert_eq!(effective(none, false, &v, None, Some(60)).cpu_cap, Some(25));

        v.auto_spread = true;
        let c = |id: &str, pid| (id.to_owned(), pid, 1, none);
        let p = plan(&[c("a", 10), c("b", 20)], &v, Some(20), 4, &[], &[], &[]);
        assert_eq!(p[0].3, Limits { trim_every_min: Some(15), affinity: Some(0b0101), ..bg });
        assert_eq!(p[1].3, none);
        assert_eq!(plan(&[c("a", 10)], &v, None, 4, &[], &[], &[])[0].3.affinity, None);

        v.fair_cap = true;
        let p = plan(&[c("a", 10), c("b", 20), c("c", 30)], &v, None, 6, &[], &["b".to_owned()], &[]);
        assert_eq!(p[0].3.cpu_cap, Some(25));
        assert_eq!(p[1].3, none);
        v.fair_cap = false;

        let three = [c("a", 10), c("b", 20), c("c", 30)];
        assert_eq!(plan(&three, &v, None, 4, &[], &[], &[])[0].3.affinity, None);
        assert_eq!(plan(&three, &v, None, 4, &[], &[], &["a".to_owned()])[0].3.affinity, Some(0b1001));

        v.mem_throttle = crate::model::MemThrottle { mb: 4_096, cpu_cap: 10 };
        let held = ["b".to_owned()];
        let p = plan(&[c("a", 10), c("b", 20)], &v, Some(20), 4, &held, &[], &[]);
        assert_eq!((p[0].3.cpu_cap, p[1].3.cpu_cap), (Some(25), Some(10)));
        let tight = Limits { cpu_cap: Some(5), ..none };
        assert_eq!(plan(&[("b".to_owned(), 20, 1, tight)], &v, None, 4, &held, &[], &[])[0].3.cpu_cap, Some(5), "its own tighter cap");
        v.mem_throttle.mb = 0;
        assert_eq!(plan(&[c("b", 20)], &v, Some(20), 4, &held, &[], &[])[0].3.cpu_cap, None, "off");
    }

    #[test]
    fn throttle_cap_only_tightens() {
        assert_eq!((throttle_cap(None, 10), throttle_cap(Some(50), 10), throttle_cap(Some(5), 10)), (10, 10, 5));
    }
    #[test]
    fn ultra_selected_ceiling_is_not_divided_by_client_count_and_manual_limits_survive() {
        use crate::model::UltraCpu;
        for cap in 1..=2 {
            let v = Settings { fair_cap: true, ultra_cpu: UltraCpu { enabled: true, cap, warmup_s: 90 }, ..Settings::default() };
            let clients: Vec<_> = (1..=20).map(|pid| (pid.to_string(), pid, 1, Limits::default())).collect();
            let limits = plan(&clients, &v, None, 12, &[], &[], &[]);
            assert!(limits.iter().all(|p| p.3.cpu_cap == Some(cap)));
            assert_eq!(plan(&clients, &v, Some(1), 12, &[], &[], &[])[0].3.cpu_cap, Some(cap));
            assert_eq!(plan(&clients, &v, None, 12, &[], &["1".into()], &[])[0].3.cpu_cap, Some(cap));
            let manual = [("1".into(), 1, 1, Limits { cpu_cap: Some(8), ..Limits::default() })];
            assert_eq!(plan(&manual, &v, Some(1), 12, &[], &["1".into()], &[])[0].3.cpu_cap, Some(cap));
        }
    }

}
