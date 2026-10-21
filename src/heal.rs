//! The manager-level parts of bounded auto-repair: the loop lock, attempt
//! records and the workspace heal budget.
//!
//! The loop itself lives in `commands` because it has to call `run` and the
//! repair registry; this module owns the files those calls leave behind.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::clock;
use crate::repo::Paths;
use crate::state;

pub const DEFAULT_MAX_AUTO_ATTEMPTS: usize = 3;
pub const LOCK_TTL: i64 = 3600;

pub enum LockResult {
    Acquired(Lock),
    Held { pid: String, expiry: i64 },
    Failed(String),
}

pub struct Lock {
    path: PathBuf,
    token: String,
}

impl Lock {
    pub fn acquire(state: &Path) -> LockResult {
        let base = state.join("heal");
        if let Err(error) = fs::create_dir_all(&base) {
            return LockResult::Failed(format!("cannot create {}: {error}", base.display()));
        }
        let path = base.join("lock");
        for _ in 0..2 {
            match fs::create_dir(&path) {
                Ok(()) => {
                    let token = format!("{}-{}", std::process::id(), clock::now_epoch());
                    let _ = fs::write(path.join("pid"), format!("{}\n", std::process::id()));
                    let _ = fs::write(path.join("token"), format!("{token}\n"));
                    let _ = fs::write(
                        path.join("expiry"),
                        format!("{}\n", clock::now_epoch() + LOCK_TTL),
                    );
                    return LockResult::Acquired(Lock { path, token });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let expiry = read_expiry(&path);
                    if expiry < clock::now_epoch() {
                        let _ = fs::remove_dir_all(&path);
                        continue;
                    }
                    let pid = state::read_lossy(&path.join("pid")).trim().to_string();
                    return LockResult::Held {
                        pid: if pid.is_empty() {
                            "unknown".into()
                        } else {
                            pid
                        },
                        expiry,
                    };
                }
                Err(error) => {
                    return LockResult::Failed(format!("cannot lock {}: {error}", path.display()));
                }
            }
        }
        LockResult::Held {
            pid: "unknown".into(),
            expiry: clock::now_epoch() + LOCK_TTL,
        }
    }

    pub fn refresh(&self) {
        if state::read_lossy(&self.path.join("token")).trim() != self.token {
            return;
        }
        let _ = fs::write(
            self.path.join("expiry"),
            format!("{}\n", clock::now_epoch() + LOCK_TTL),
        );
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        if state::read_lossy(&self.path.join("token")).trim() == self.token {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn read_expiry(path: &Path) -> i64 {
    state::read_lossy(&path.join("expiry"))
        .trim()
        .parse::<i64>()
        .unwrap_or(0)
}

pub struct Attempt {
    pub number: usize,
    pub dir: PathBuf,
    pub started: i64,
}

pub fn begin_attempt(paths: &Paths) -> Result<Attempt, String> {
    let base = paths.state.join("heal").join(clock::today());
    fs::create_dir_all(&base)
        .map_err(|error| format!("cannot create {}: {error}", base.display()))?;
    for number in 1..10_000 {
        let dir = base.join(format!("attempt-{number}"));
        match fs::create_dir(&dir) {
            Ok(()) => {
                return Ok(Attempt {
                    number,
                    dir,
                    started: clock::now_epoch(),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!("cannot create {}: {error}", dir.display()));
            }
        }
    }
    Err(format!(
        "cannot find a free attempt number under {}",
        base.display()
    ))
}

pub struct Record<'a> {
    pub action: &'a str,
    pub before_key: &'a str,
    pub after_key: &'a str,
    pub verdict: &'a str,
    pub rc: i32,
    pub cost: Option<&'a str>,
}

pub fn record_attempt(paths: &Paths, attempt: &Attempt, record: &Record) {
    let action = state::one_line(record.action);
    let before = state::one_line(record.before_key);
    let after = state::one_line(record.after_key);
    let verdict = state::one_line(record.verdict);
    let cost = record.cost.unwrap_or("");

    let _ = fs::write(attempt.dir.join("before"), format!("{before}\n"));
    let _ = fs::write(attempt.dir.join("after"), format!("{after}\n"));
    let _ = fs::write(attempt.dir.join("verdict"), format!("{verdict}\n"));
    let _ = fs::write(
        attempt.dir.join("meta.env"),
        format!(
            "action={action}\nbefore={before}\nafter={after}\nverdict={verdict}\nrc={}\ncost={cost}\n",
            record.rc
        ),
    );

    let seconds = (clock::now_epoch() - attempt.started).max(0);
    let line = format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        clock::iso(attempt.started),
        paths.name,
        attempt.number,
        action,
        before,
        after,
        verdict,
        record.rc,
        seconds,
        cost
    );
    let _ = state::append(&paths.state.join("heal").join("history.tsv"), &line);
}

pub fn max_auto_attempts() -> Result<usize, String> {
    match env::var("MAX_AUTO_ATTEMPTS") {
        Ok(value) if !value.is_empty() => value
            .parse::<usize>()
            .map_err(|_| "MAX_AUTO_ATTEMPTS must be a whole number".to_string()),
        _ => Ok(DEFAULT_MAX_AUTO_ATTEMPTS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = env::temp_dir().join(format!(
            "lightsout-heal-{}-{nanos}-{label}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn lock_is_exclusive_and_released_on_drop() {
        let state = temp("lock");
        let first = Lock::acquire(&state);
        assert!(matches!(first, LockResult::Acquired(_)));
        let second = Lock::acquire(&state);
        assert!(matches!(second, LockResult::Held { .. }));
        drop(first);
        let third = Lock::acquire(&state);
        assert!(matches!(third, LockResult::Acquired(_)));
    }

    #[test]
    fn a_stale_lock_is_reclaimed() {
        let state = temp("stale");
        let lock = state.join("heal").join("lock");
        fs::create_dir_all(&lock).unwrap();
        fs::write(lock.join("expiry"), "0\n").unwrap();
        assert!(matches!(Lock::acquire(&state), LockResult::Acquired(_)));
    }

    #[test]
    fn attempt_numbers_do_not_reuse_a_directory() {
        let root = temp("attempt");
        let paths = Paths::new(&root, "ready");
        fs::create_dir_all(&paths.state).unwrap();
        let first = begin_attempt(&paths).unwrap();
        let second = begin_attempt(&paths).unwrap();
        assert_eq!(first.number, 1);
        assert_eq!(second.number, 2);
        assert!(first.dir.is_dir() && second.dir.is_dir());
    }
}
