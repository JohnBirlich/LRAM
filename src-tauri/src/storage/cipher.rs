use std::path::{Path, PathBuf};

use rusqlite::Connection;
use zeroize::Zeroizing;

use crate::discord::hex;
use crate::error::{AppError, Result};
use crate::vault::master::random_bytes;
use crate::vault::{protect_with, unprotect_with};

const ENTROPY: &[u8] = b"RoALT/db/v1";

pub fn beside(db: &Path, suffix: &str) -> PathBuf {
    let mut name = db.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    db.with_file_name(name)
}

fn key_literal(key: &[u8]) -> Zeroizing<String> {
    Zeroizing::new(format!("\"x'{}'\"", hex(key)))
}

pub fn unlock(conn: &Connection, key: &[u8]) -> Result<()> {
    conn.execute_batch(&format!("PRAGMA key = {};", key_literal(key).as_str()))
        .map_err(|_| AppError::Storage("setting the database key failed".into()))?;
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
        .map_err(|e| AppError::Storage(format!("the database key doesn't open it ({e})")))
}

pub fn read_key(db: &Path) -> Result<Zeroizing<Vec<u8>>> {
    let blob = std::fs::read(beside(db, ".key"))?;
    let key = unprotect_with(&blob, ENTROPY)?;
    if key.len() != 32 {
        log::error!("database key file holds {} bytes, not 32", key.len());
        return Err(AppError::Crypto);
    }
    Ok(key)
}

pub fn new_key(path: &Path) -> Result<Zeroizing<Vec<u8>>> {
    let key = Zeroizing::new(random_bytes::<32>().to_vec());
    write_key(path, &key)?;
    Ok(key)
}

pub fn write_key(path: &Path, key: &[u8]) -> Result<()> {
    let tmp = beside(path, ".tmp");
    std::fs::write(&tmp, protect_with(key, ENTROPY)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn export(conn: &Connection, to: &Path, key: &[u8]) -> Result<()> {
    let to = to.to_str().ok_or_else(|| AppError::Invalid(tr!("That folder's name can't be used.")))?;
    conn.execute(&format!("ATTACH DATABASE ?1 AS enc KEY {}", key_literal(key).as_str()), [to])
        .map_err(|_| AppError::Storage("attaching the encrypted copy failed".into()))?;
    let done = (|| {
        conn.query_row("SELECT sqlcipher_export('enc')", [], |_| Ok(()))?;
        let version: i64 = conn.query_row("PRAGMA main.user_version", [], |r| r.get(0))?;
        conn.execute_batch(&format!("PRAGMA enc.user_version = {version};"))
    })()
    .map_err(AppError::from);
    if let Err(e) = conn.execute_batch("DETACH DATABASE enc;") {
        if let Err(d) = &done {
            log::error!("database export failed: {d}");
        }
        return Err(e.into());
    }
    done
}

fn fingerprint(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut out = vec![("user_version".to_owned(), conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)];
    for t in tables.drain(..) {
        let n = conn.query_row(&format!("SELECT count(*) FROM \"{}\"", t.replace('"', "\"\"")), [], |r| r.get(0))?;
        out.push((t, n));
    }
    Ok(out)
}

fn remove(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

fn remove_journal(db: &Path) -> Result<()> {
    remove(&beside(db, "-wal"))?;
    remove(&beside(db, "-shm"))
}

pub fn resume(db: &Path) -> Result<()> {
    let (enc, key_new, plain) = (beside(db, ".enc"), beside(db, ".key.new"), beside(db, ".plain"));
    if !plain.exists() {
        return Ok(());
    }
    if !db.exists() && !enc.exists() {
        std::fs::rename(&plain, db)?;
        return Ok(());
    }
    (|| -> Result<()> {
        if !db.exists() && enc.exists() {
            std::fs::rename(&enc, db)?;
        }
        if key_new.exists() {
            std::fs::rename(&key_new, beside(db, ".key"))?;
        }
        if !beside(db, ".key").exists() {
            return Err(AppError::Storage("the database's key is missing".into()));
        }
        remove(&plain)?;
        remove_journal(&plain)
    })()
    .inspect_err(|e| log::error!("could not finish an interrupted database encryption: {e}"))?;
    log::info!("finished encrypting the database after an interrupted start");
    Ok(())
}

// -- Main func
pub fn migrate(db: &Path) -> Result<Zeroizing<Vec<u8>>> {
    let (enc, key_new, plain) = (beside(db, ".enc"), beside(db, ".key.new"), beside(db, ".plain"));
    remove(&enc)?;
    remove(&key_new)?;
    let conn = Connection::open(db)?;
    let before = fingerprint(&conn)?;
    let key = Zeroizing::new(random_bytes::<32>().to_vec());
    export(&conn, &enc, &key)?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    drop(conn);
    {
        let copy = Connection::open(&enc)?;
        unlock(&copy, &key)?;
        let after = fingerprint(&copy)?;
        if after != before {
            log::error!("encrypted database copy doesn't match: before {before:?}, after {after:?}");
            drop(copy);
            remove(&enc)?;
            return Err(AppError::Storage("the encrypted copy of the database didn't match it".into()));
        }
    }
    write_key(&key_new, &key)?;
    remove_journal(db)?;
    std::fs::rename(db, &plain)?;
    std::fs::rename(&enc, db)?;
    std::fs::rename(&key_new, beside(db, ".key"))?;
    remove(&plain)?;
    log::info!("database encrypted ({} tables)", before.len() - 1);
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;

    fn tmp() -> PathBuf {
        std::env::temp_dir().join(format!("lram-cipher-{}", uuid::Uuid::new_v4()))
    }

    fn plain_has_rows(path: &Path) -> rusqlite::Result<i64> {
        Connection::open(path)?.query_row("SELECT count(*) FROM accounts", [], |r| r.get(0))
    }

    fn put(db: &Path, user_id: i64) {
        let c = Connection::open(db).unwrap();
        c.execute(
            "INSERT INTO accounts (user_id, username, display_name, sort_order, cookie, status, created_at, updated_at) VALUES (?1, 'n', 'n', 0, x'00', 'valid', 0, 0)",
            [user_id],
        )
        .unwrap();
    }

    #[test]
    fn a_new_database_is_unreadable_without_its_key() {
        let dir = tmp();
        let path = dir.join("lram.db");
        drop(Db::open(&path).unwrap());
        assert!(beside(&path, ".key").is_file());
        assert!(plain_has_rows(&path).is_err(), "opens as plain SQLite");
        let raw = std::fs::read(&path).unwrap();
        assert!(!raw.starts_with(b"SQLite format 3"));
        let c = Connection::open(&path).unwrap();
        assert!(unlock(&c, &[7; 32]).is_err());
        drop(c);
        assert!(Db::open(&path).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plain_database_is_encrypted_with_its_data() {
        let dir = tmp();
        let path = dir.join("lram.db");
        std::fs::create_dir_all(&dir).unwrap();
        Db::open_in_memory().unwrap().copy_to(&path, || Ok(())).unwrap();
        put(&path, 42);
        assert_eq!(plain_has_rows(&path).unwrap(), 1);
        let db = Db::open(&path).unwrap();
        assert_eq!(db.accounts().unwrap().iter().map(|a| a.user_id).collect::<Vec<_>>(), [42]);
        drop(db);
        assert!(plain_has_rows(&path).is_err());
        for left in [".enc", ".key.new", ".plain"] {
            assert!(!beside(&path, left).exists(), "{left} left behind");
        }
        assert_eq!(Db::open(&path).unwrap().accounts().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_interrupted_commit_is_finished() {
        let dir = tmp();
        let path = dir.join("lram.db");
        std::fs::create_dir_all(&dir).unwrap();
        Db::open_in_memory().unwrap().copy_to(&path, || Ok(())).unwrap();
        put(&path, 7);
        let key = Zeroizing::new(random_bytes::<32>().to_vec());
        export(&Connection::open(&path).unwrap(), &beside(&path, ".enc"), &key).unwrap();
        write_key(&beside(&path, ".key.new"), &key).unwrap();
        std::fs::rename(&path, beside(&path, ".plain")).unwrap();
        let db = Db::open(&path).unwrap();
        assert_eq!(db.accounts().unwrap()[0].user_id, 7);
        drop(db);
        assert!(!beside(&path, ".plain").exists() && !beside(&path, ".key.new").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_copy_is_encrypted_and_carries_its_key() {
        let dir = tmp();
        let db = Db::open(&dir.join("lram.db")).unwrap();
        let copy = dir.join("moved").join("lram.db");
        std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
        db.copy_to(&copy, || Ok(())).unwrap();
        assert!(plain_has_rows(&copy).is_err());
        assert!(beside(&copy, ".key").is_file());
        assert!(Db::open(&copy).is_ok());
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
