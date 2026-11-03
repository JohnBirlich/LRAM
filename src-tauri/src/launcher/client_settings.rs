use std::io::{ErrorKind, Read};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::App;
use crate::instances::logwatch;
use crate::storage::Db;

pub const RANGE: RangeInclusive<u32> = 5..=360;

const KEYS: [(&str, &str); 3] = [("int", "FramerateCap"), ("int", "GraphicsQualityLevel"), ("token", "SavedQualityLevel")];

const EMPTY: &str = "<roblox version=\"4\">\n\t<Item class=\"UserGameSettings\" referent=\"RBX0\">\n\t\t<Properties>\n\t\t</Properties>\n\t</Item>\n</roblox>\n";

const READ_MARKS: [&[u8]; 2] = [b"SceneManager: resizing main targets", b"[FLog::Network] Replicator created"];
const AFTER_READ: Duration = Duration::from_secs(1);
const LOG_TIMEOUT: Duration = Duration::from_secs(60);
const MARK_TIMEOUT: Duration = Duration::from_secs(15);
const UPDATE_TIMEOUT: Duration = Duration::from_secs(180);
const RELAUNCH_GRACE: Duration = Duration::from_secs(3);
const WATCH: Duration = Duration::from_secs(1);
const KEEP: Duration = Duration::from_millis(100);
const TRIES: u32 = 10;
const RETRY: Duration = Duration::from_millis(50);

const KEPT: &str = "roblox_settings";
const OURS: usize = 16;

pub static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

type Values = [Option<i64>; 3];

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
struct Kept {
    user: Option<Values>,
    ours: [Vec<i64>; 3],
}

fn file() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Roblox").join("GlobalBasicSettings_13.xml"))
}

fn get(text: &str, (tag, name): (&str, &str)) -> Option<i64> {
    let open = format!("<{tag} name=\"{name}\">");
    let start = text.find(&open)? + open.len();
    let end = start + text[start..].find('<')?;
    text[start..end].trim().parse().ok()
}

fn set(text: &str, (tag, name): (&str, &str), v: Option<i64>) -> String {
    let (open, close) = (format!("<{tag} name=\"{name}\">"), format!("</{tag}>"));
    let element = v.map(|v| format!("{open}{v}{close}"));
    match text.find(&open) {
        Some(i) => match text[i..].find(&close) {
            Some(len) => format!("{}{}{}", &text[..i], element.unwrap_or_default(), &text[i + len + close.len()..]),
            None => text.into(),
        },
        None => match (element, text.find("</Properties>")) {
            (Some(e), Some(i)) => format!("{}\t{e}\n\t\t{}", &text[..i], &text[i..]),
            _ => text.into(),
        },
    }
}

fn read(text: &str) -> Values {
    KEYS.map(|k| get(text, k))
}

fn write(text: &str, v: Values) -> String {
    KEYS.iter().zip(v).fold(text.to_owned(), |t, (k, v)| set(&t, *k, v))
}

fn for_launch([fps_now, quality, saved]: Values, fps: u32, low: bool) -> Values {
    [
        if fps == 0 { fps_now } else { Some(fps.into()) },
        if low { Some(1) } else { quality },
        if low { Some(1) } else { saved },
    ]
}

fn settle_values(kept: &mut Kept, now: Values) -> Values {
    let Some(mut user) = kept.user else {
        kept.user = Some(now);
        return now;
    };
    let mut want = now;
    for i in 0..KEYS.len() {
        if now[i] == user[i] {
            continue;
        }
        if now[i].is_some_and(|v| kept.ours[i].contains(&v)) {
            want[i] = user[i];
        } else {
            user[i] = now[i];
        }
    }
    kept.user = Some(user);
    want
}

fn load(db: &Db) -> Kept {
    let json = db.setting_value(KEPT).inspect_err(|e| log::warn!("Roblox settings: couldn't read the user's own values: {e}")).ok().flatten();
    json.and_then(|j| serde_json::from_str(&j).inspect_err(|e| log::warn!("Roblox settings: the user's own values are unreadable: {e}")).ok())
        .unwrap_or_default()
}

fn save(db: &Db, kept: &Kept) {
    let saved = serde_json::to_string(kept).map_err(|e| e.to_string()).and_then(|j| db.set_setting_value(KEPT, Some(&j)).map_err(|e| e.to_string()));
    if let Err(e) = saved {
        log::warn!("Roblox settings: couldn't keep the user's own values: {e}");
    }
}

pub(super) fn retry<T>(mut f: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut tries = 1;
    loop {
        match f() {
            Err(e) if tries < TRIES && (e.kind() == ErrorKind::PermissionDenied || e.raw_os_error() == Some(32)) => {
                tries += 1;
                std::thread::sleep(RETRY);
            }
            done => return done,
        }
    }
}

type Stamp = (SystemTime, u64);

fn stamp(file: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(file).ok()?;
    Some((m.modified().ok()?, m.len()))
}

fn read_file(file: &Path) -> std::io::Result<Option<(String, Stamp)>> {
    let seen = stamp(file);
    let text = retry(|| std::fs::read_to_string(file))?;
    Ok(seen.filter(|_| text.trim_end().ends_with("</roblox>")).map(|s| (text, s)))
}

#[allow(clippy::permissions_set_readonly_false)]
pub(super) fn set_readonly(file: &Path, on: bool) -> std::io::Result<()> {
    let mut p = std::fs::metadata(file)?.permissions();
    p.set_readonly(on);
    std::fs::set_permissions(file, p)
}

fn replace(file: &Path, text: &str, seen: Option<Stamp>) -> std::io::Result<()> {
    let tmp = file.with_extension("xml.lram");
    let readonly = std::fs::metadata(file).is_ok_and(|m| m.permissions().readonly());
    let unchanged = || match stamp(file) == seen {
        true => Ok(()),
        false => Err(std::io::Error::new(ErrorKind::Interrupted, "the file changed meanwhile")),
    };
    let done = retry(|| std::fs::write(&tmp, text)).and_then(|()| {
        unchanged()?;
        if readonly {
            set_readonly(file, false)?;
        }
        retry(|| unchanged().and_then(|()| std::fs::rename(&tmp, file)))
    });
    if readonly && let Err(e) = set_readonly(file, true) {
        log::warn!("Roblox settings: couldn't make {} read-only again: {e}", file.display());
    }
    if done.is_err()
        && let Err(e) = std::fs::remove_file(&tmp)
        && e.kind() != ErrorKind::NotFound
    {
        log::debug!("Roblox settings: couldn't remove {}: {e}", tmp.display());
    }
    done
}

pub struct Undo {
    file: PathBuf,
    want: Values,
    kept: Kept,
    fps: u32,
    low: bool,
}

impl Undo {
    pub fn keep_until(mut self, stop: &std::sync::atomic::AtomicBool) -> Undo {
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            self.keep();
            std::thread::sleep(KEEP);
        }
        self
    }

    fn keep(&mut self) {
        let Ok(Some((text, seen))) = read_file(&self.file) else { return };
        let now = read(&text);
        if now == self.want {
            return;
        }
        settle_values(&mut self.kept, now);
        self.want = for_launch(self.kept.user.unwrap_or(now), self.fps, self.low);
        if self.want == now {
            return;
        }
        match replace(&self.file, &write(&text, self.want), Some(seen)) {
            Ok(()) => log::info!("Roblox settings: another client saved its own while this one started; this launch's FPS cap and graphics are back"),
            Err(e) => log::debug!("Roblox settings: couldn't write this launch's values again yet: {e}"),
        }
    }

    pub fn put_back(mut self) {
        for _ in 0..TRIES {
            let (text, seen) = match read_file(&self.file) {
                Ok(Some(whole)) => whole,
                Ok(None) => {
                    std::thread::sleep(RETRY);
                    continue;
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    return log::debug!("Roblox settings: {} is gone; nothing to put back", self.file.display());
                }
                Err(e) => return log::warn!("Roblox settings: couldn't read {} to put the user's own back: {e}", self.file.display()),
            };
            let now = read(&text);
            if now != self.want {
                log::info!("Roblox settings: another client saved its own while this one started; it may not run at this launch's FPS cap and graphics");
            }
            let want = settle_values(&mut self.kept, now);
            if want == now {
                return;
            }
            match replace(&self.file, &write(&text, want), Some(seen)) {
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return log::warn!("Roblox settings: couldn't put {} back: {e}", self.file.display()),
                Ok(()) => return,
            }
        }
        log::warn!("Roblox settings: {} kept changing; the watcher puts the user's own back", self.file.display());
    }
}

// -- Main func
pub fn apply(db: &Db, fps: u32, low: bool) -> Option<Undo> {
    let Some(file) = file() else {
        log::warn!("FPS cap and graphics skipped: LOCALAPPDATA isn't set");
        return None;
    };
    apply_at(db, file, fps, low)
}

fn apply_at(db: &Db, file: PathBuf, fps: u32, low: bool) -> Option<Undo> {
    for _ in 0..TRIES {
        let (text, seen) = match read_file(&file) {
            Ok(Some((text, seen))) => (text, Some(seen)),
            Ok(None) => {
                std::thread::sleep(RETRY);
                continue;
            }
            Err(e) if e.kind() == ErrorKind::NotFound && fps == 0 && !low => return Some(Undo { file, want: [None; 3], kept: Kept::default(), fps, low }),
            Err(e) if e.kind() == ErrorKind::NotFound => (EMPTY.to_owned(), None),
            Err(e) => {
                log::warn!("FPS cap and graphics skipped: can't read {}: {e}", file.display());
                return None;
            }
        };
        let now = read(&text);
        let mut kept = load(db);
        settle_values(&mut kept, now);
        let user = kept.user.unwrap_or(now);
        let want = for_launch(user, fps, low);
        for (i, v) in want.iter().enumerate() {
            if let Some(v) = v.filter(|_| want[i] != user[i]) {
                kept.ours[i].retain(|o| *o != v);
                kept.ours[i].push(v);
                let extra = kept.ours[i].len().saturating_sub(OURS);
                kept.ours[i].drain(..extra);
            }
        }
        save(db, &kept);
        let undo = Undo { file: file.clone(), want, kept, fps, low };
        if want == now {
            return Some(undo);
        }
        match replace(&file, &write(&text, want), seen) {
            Ok(()) => {
                log::info!("Roblox settings for this launch: FPS cap {:?}, graphics {:?}", want[0], want[2]);
                return Some(undo);
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => {
                log::warn!("FPS cap and graphics not written to {}: {e}", file.display());
                return None;
            }
        }
    }
    log::warn!("FPS cap and graphics skipped: {} kept changing", file.display());
    None
}

fn shows_read(log: &[u8]) -> bool {
    READ_MARKS.iter().any(|m| log.windows(m.len()).any(|w| w == *m))
}

fn has_read(path: &Path) -> bool {
    let mut head = Vec::new();
    std::fs::File::open(path).and_then(|f| f.take(4 << 20).read_to_end(&mut head)).is_ok() && shows_read(&head)
}

pub async fn wait_until_read(browser_tracker_id: i64, since: SystemTime, client: Option<(u32, u64)>) {
    use crate::instances::process;
    use tokio::time::Instant;
    let Some(dir) = logwatch::logs_dir() else {
        log::warn!("Roblox settings: no Roblox logs folder; waiting {} s before putting the user's back", MARK_TIMEOUT.as_secs());
        return tokio::time::sleep(MARK_TIMEOUT).await;
    };
    let start = Instant::now();
    let mut client = client;
    let mut log: Option<(PathBuf, Instant)> = None;
    let mut gone: Option<(Instant, bool)> = None;
    loop {
        let d = dir.clone();
        let (found, read) = tokio::task::spawn_blocking(move || {
            let l = logwatch::find_log(&d, browser_tracker_id, since);
            let read = l.as_deref().is_some_and(has_read);
            (l, read)
        })
        .await
        .unwrap_or_else(|e| {
            log::warn!("Roblox settings: looking for the client's log failed: {e}");
            (None, false)
        });
        if read {
            return tokio::time::sleep(AFTER_READ).await;
        }
        if let Some(l) = found.filter(|l| log.as_ref().is_none_or(|(known, _)| known != l)) {
            if log.is_some() {
                (client, gone) = (None, None);
            }
            log = Some((l, Instant::now()));
        }
        if let Some((pid, t)) = client.filter(|&(pid, t)| process::start_time(pid) != Some(t)) {
            client = None;
            let installer = tokio::task::spawn_blocking(process::installer_running).await.unwrap_or_else(|e| {
                log::debug!("Roblox settings: installer check failed: {e}");
                false
            });
            let lived = since.elapsed().map_or(0, |d| d.as_secs());
            log::warn!("Roblox settings: the client (pid {pid}, started {t}) ended about {lived} s after its launch, before it read them (Roblox's installer running: {installer}, client log found: {})", log.is_some());
            gone = Some((Instant::now(), installer));
        }
        let deadline = match (gone, &log) {
            (Some((at, true)), _) => at + UPDATE_TIMEOUT,
            (Some((at, false)), _) => at + RELAUNCH_GRACE,
            (None, Some((_, at))) => *at + MARK_TIMEOUT,
            (None, None) => start + LOG_TIMEOUT,
        };
        if Instant::now() >= deadline {
            let what = if gone.is_some() { "come back" } else if log.is_some() { "show it had read them" } else { "start logging" };
            return log::warn!("Roblox settings: the client didn't {what} in time; putting the user's back. If it died, the \"autopsy\" lines of its instance say why, up to 2 minutes after its launch");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

pub fn restore(db: &Db) {
    if let Some(file) = file() {
        restore_at(db, &file);
    }
}

fn restore_at(db: &Db, file: &Path) {
    let (text, seen) = match read_file(file) {
        Ok(Some(whole)) => whole,
        Ok(None) => return,
        Err(e) => return log::debug!("Roblox settings: couldn't read {} to put the user's own back: {e}", file.display()),
    };
    let now = read(&text);
    let mut kept = load(db);
    let want = settle_values(&mut kept, now);
    save(db, &kept);
    if want != now
        && let Err(e) = replace(file, &write(&text, want), Some(seen))
    {
        log::warn!("Roblox settings: couldn't put {} back: {e}", file.display());
    }
}

fn settle(db: &Db, file: &Path) -> bool {
    let (text, seen) = match read_file(file) {
        Ok(Some(whole)) => whole,
        Ok(None) => return false,
        Err(e) if e.kind() == ErrorKind::NotFound => return true,
        Err(e) => {
            static LAST: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
            let why = e.to_string();
            let mut last = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if *last != why {
                log::warn!("Roblox settings: couldn't read {}: {why}", file.display());
                *last = why;
            }
            return false;
        }
    };
    let now = read(&text);
    let mut kept = load(db);
    let before = kept.clone();
    let want = settle_values(&mut kept, now);
    if kept != before {
        save(db, &kept);
    }
    if want == now {
        return true;
    }
    match replace(file, &write(&text, want), Some(seen)) {
        Ok(()) => {
            log::info!("Roblox settings: a client that had a launch's FPS cap and graphics saved them as it closed; the user's own are back");
            true
        }
        Err(e) if e.kind() == ErrorKind::Interrupted => false,
        Err(e) => {
            let again = e.kind() == ErrorKind::PermissionDenied || e.raw_os_error() == Some(32);
            if again {
                log::debug!("Roblox settings: couldn't put {} back yet: {e}", file.display());
            } else {
                log::warn!("Roblox settings: couldn't put {} back: {e}", file.display());
            }
            !again
        }
    }
}

pub fn spawn(app: AppHandle) {
    let watching = std::thread::Builder::new().name("client-settings".into()).spawn(move || {
        let mut seen = None;
        loop {
            std::thread::sleep(WATCH);
            let Ok(_idle) = LOCK.try_lock() else { continue };
            let Some(file) = file() else { continue };
            let Ok(modified) = std::fs::metadata(&file).and_then(|m| m.modified()) else { continue };
            if seen == Some(modified) {
                continue;
            }
            if settle(&app.state::<App>().db, &file) {
                seen = Some(modified);
            }
        }
    });
    if let Err(e) = watching {
        log::warn!("Roblox settings watcher not running: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = "<roblox>\n\t<Item class=\"UserGameSettings\">\n\t\t<Properties>\n\t\t\t<int name=\"FramerateCap\">240</int>\n\t\t\t<int name=\"GraphicsQualityLevel\">19</int>\n\t\t\t<token name=\"SavedQualityLevel\">9</token>\n\t\t</Properties>\n\t</Item>\n</roblox>";
    const MINE: Values = [Some(240), Some(19), Some(9)];
    const LOW: Values = [Some(15), Some(1), Some(1)];

    struct TestFile(PathBuf);

    impl TestFile {
        fn new(name: &str, text: Option<&str>) -> Self {
            let f = TestFile(std::env::temp_dir().join(format!("lram-test-{name}-{}.xml", std::process::id())));
            let _ = std::fs::remove_file(&f.0);
            if let Some(t) = text {
                std::fs::write(&f.0, t).unwrap();
            }
            f
        }
        fn text(&self) -> String {
            std::fs::read_to_string(&self.0).unwrap()
        }
        fn put(&self, text: &str) {
            std::fs::write(&self.0, text).unwrap();
        }
    }

    impl Drop for TestFile {
        fn drop(&mut self) {
            let _ = set_readonly(&self.0, false);
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn reads_and_writes_only_its_settings() {
        assert_eq!(read(XML), MINE);
        let t = write(XML, LOW);
        assert_eq!(read(&t), LOW);
        assert_eq!(write(&t, MINE), XML, "putting back restores the file exactly");
        assert_eq!(read(&set(XML, KEYS[0], None)), [None, Some(19), Some(9)]);
    }

    #[test]
    fn adds_a_missing_setting_in_the_files_layout() {
        let bare = XML.replace("\t\t\t<int name=\"FramerateCap\">240</int>\n", "");
        let t = set(&bare, KEYS[0], Some(30));
        assert!(t.contains("\t\t\t<int name=\"FramerateCap\">30</int>\n\t\t</Properties>"), "{t}");
        assert_eq!(set(&t, KEYS[0], None).replace("\t\t\t\n", ""), bare);
        assert_eq!(set("<roblox/>", KEYS[0], Some(30)), "<roblox/>", "no Properties: left alone");
        assert_eq!(read("not xml"), [None; 3]);
        assert_eq!(read(&write(EMPTY, LOW)), LOW, "a missing file starts empty");
    }

    #[test]
    fn a_launch_changes_only_what_it_asks_for() {
        assert_eq!(for_launch(MINE, 0, false), MINE);
        assert_eq!(for_launch(MINE, 30, false), [Some(30), Some(19), Some(9)]);
        assert_eq!(for_launch(MINE, 0, true), [Some(240), Some(1), Some(1)]);
    }

    #[test]
    fn settles_each_setting_on_its_own() {
        let kept = || Kept { user: Some(MINE), ours: [vec![15], vec![1], vec![1]] };
        let mut k = kept();
        assert_eq!(settle_values(&mut k, [Some(15), Some(3), Some(1)]), [Some(240), Some(3), Some(9)]);
        assert_eq!(k.user, Some([Some(240), Some(3), Some(9)]));
        let mut k = kept();
        assert_eq!(settle_values(&mut k, [Some(144), Some(19), Some(9)]), [Some(144), Some(19), Some(9)]);
        assert_eq!(k.user, Some([Some(144), Some(19), Some(9)]));
        let mut k = Kept::default();
        assert_eq!(settle_values(&mut k, LOW), LOW);
        assert_eq!(k.user, Some(LOW));
        let back: Kept = serde_json::from_str(&serde_json::to_string(&kept()).unwrap()).unwrap();
        assert_eq!(back, kept());
    }

    #[test]
    fn launches_and_closing_clients_leave_the_users_settings() {
        let db = Db::open_in_memory().unwrap();
        let f = TestFile::new("cycle", Some(XML));

        let undo = apply_at(&db, f.0.clone(), 15, true).expect("the launch's values are written");
        assert_eq!(read(&f.text()), LOW);
        undo.put_back();
        assert_eq!(f.text(), XML, "the user's own are back");

        f.put(&write(XML, LOW));
        assert!(settle(&db, &f.0));
        assert_eq!(f.text(), XML);
        f.put(&write(XML, LOW));
        apply_at(&db, f.0.clone(), 30, false).unwrap().put_back();
        assert_eq!(f.text(), XML);

        let mine = [Some(144), Some(19), Some(9)];
        f.put(&write(XML, mine));
        assert!(settle(&db, &f.0));
        assert_eq!(read(&f.text()), mine);
        assert_eq!(load(&db).user, Some(mine));
        let undo = apply_at(&db, f.0.clone(), 15, true).unwrap();
        f.put(&write(XML, [Some(60), Some(1), Some(1)]));
        undo.put_back();
        assert_eq!(read(&f.text()), [Some(60), Some(19), Some(9)]);

        let now = f.text();
        let undo = apply_at(&db, f.0.clone(), 0, false).expect("held until its client has read the file");
        assert_eq!(f.text(), now);
        undo.put_back();
        assert_eq!(f.text(), now);
        f.put(&write(XML, LOW));
        let undo = apply_at(&db, f.0.clone(), 0, false).unwrap();
        assert_eq!(read(&f.text()), [Some(60), Some(19), Some(9)]);
        undo.put_back();

        f.put(&write(XML, LOW));
        restore_at(&db, &f.0);
        assert_eq!(read(&f.text()), [Some(60), Some(19), Some(9)]);
    }

    #[test]
    fn a_client_closing_during_a_launch_doesnt_change_what_the_new_one_reads() {
        let db = Db::open_in_memory().unwrap();
        let f = TestFile::new("closing", Some(XML));
        apply_at(&db, f.0.clone(), 15, true).unwrap().put_back();
        let mut undo = apply_at(&db, f.0.clone(), 30, true).unwrap();
        let want = [Some(30), Some(1), Some(1)];
        assert_eq!(read(&f.text()), want);
        undo.keep();
        assert_eq!(read(&f.text()), want, "nothing to do");

        f.put(&write(XML, LOW));
        undo.keep();
        assert_eq!(read(&f.text()), want);
        undo.put_back();
        assert_eq!(f.text(), XML);

        let mut undo = apply_at(&db, f.0.clone(), 30, false).unwrap();
        f.put(&write(XML, [Some(144), Some(19), Some(5)]));
        undo.keep();
        assert_eq!(read(&f.text()), [Some(30), Some(19), Some(5)]);
        undo.put_back();
        assert_eq!(read(&f.text()), [Some(144), Some(19), Some(5)]);

        let mut undo = apply_at(&db, f.0.clone(), 30, false).unwrap();
        f.put("<roblox>");
        undo.keep();
        assert_eq!(f.text(), "<roblox>");
        f.put(&write(XML, [Some(144), Some(19), Some(5)]));
        undo.keep();
        assert_eq!(read(&f.text()), [Some(30), Some(19), Some(5)]);

        let stop = std::sync::atomic::AtomicBool::new(true);
        undo.keep_until(&stop).put_back();
        assert_eq!(read(&f.text()), [Some(144), Some(19), Some(5)]);
    }

    #[test]
    fn leaves_a_file_being_written_alone() {
        let db = Db::open_in_memory().unwrap();
        let f = TestFile::new("partial", Some(XML));
        assert!(settle(&db, &f.0));
        f.put("");
        assert!(!settle(&db, &f.0), "empty: look again");
        f.put(&write(XML, LOW)[..60]);
        assert!(!settle(&db, &f.0), "half written: look again");
        assert_eq!(load(&db).user, Some(MINE), "the user's own are untouched");
        assert!(apply_at(&db, f.0.clone(), 15, true).is_none(), "a launch doesn't write over it");
        assert_eq!(f.text(), write(XML, LOW)[..60]);
    }

    #[test]
    fn writes_a_read_only_file_and_keeps_it_read_only() {
        let db = Db::open_in_memory().unwrap();
        let f = TestFile::new("readonly", Some(XML));
        set_readonly(&f.0, true).unwrap();
        let undo = apply_at(&db, f.0.clone(), 15, true).expect("written despite read-only");
        assert_eq!(read(&f.text()), LOW);
        assert!(std::fs::metadata(&f.0).unwrap().permissions().readonly());
        undo.put_back();
        assert_eq!(f.text(), XML);
        assert!(std::fs::metadata(&f.0).unwrap().permissions().readonly());
        assert!(!f.0.with_extension("xml.lram").exists(), "no temporary file left");
    }

    #[test]
    fn starts_a_missing_file() {
        let db = Db::open_in_memory().unwrap();
        let f = TestFile::new("missing", None);
        let undo = apply_at(&db, f.0.clone(), 15, true).expect("written to a new file");
        assert_eq!(read(&f.text()), LOW);
        undo.put_back();
        assert_eq!(read(&f.text()), [None; 3], "the user had none of them");
        assert!(f.text().trim_end().ends_with("</roblox>"));
        let roblox = f.text();
        assert_eq!(replace(&f.0, EMPTY, None).unwrap_err().kind(), ErrorKind::Interrupted);
        assert_eq!(f.text(), roblox);

        let none = TestFile::new("missing-alone", None);
        apply_at(&db, none.0.clone(), 0, false).expect("held until read").put_back();
        assert!(!none.0.exists(), "leaving Roblox alone creates nothing");
    }

    #[test]
    fn knows_when_the_client_has_read_the_file() {
        assert!(!shows_read(b"[FLog::FlagFetchingStarterModule] Successfully loaded flags"));
        assert!(!shows_read(b"x ! Joining game 'abc' place 1818 at 1.2.3.4"), "a join comes before the read");
        assert!(shows_read(b"x [FLog::Graphics] SceneManager: resizing main targets to 1004x758"));
        assert!(shows_read(b"x [FLog::Network] Replicator created: 000000007A26CF00"), "in a game");
    }
}
