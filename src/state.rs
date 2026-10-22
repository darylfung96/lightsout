//! Parsers for the files under `jobs/<name>/state/`.
//!
//! Every fact the dashboard, `check` and `diagnose` need lives here, so the
//! manager cannot disagree with itself about one line. The formats are the
//! run contract's; an older or damaged line degrades field by field instead of
//! taking the whole view down.

use std::fs;
use std::io;
use std::path::Path;

use crate::clock;

pub const CLASSES: [&str; 3] = ["retryable", "judgment", "human"];

#[derive(Clone, Debug)]
pub struct Blocked {
    pub raw: String,
    pub iso: String,
    pub class: String,
    pub source: String,
    pub reason: String,
}

/// `state/blocked` is `<iso>TAB<class>TAB<source>TAB<reason>`.
///
/// A two-field line written by an older run reads as `human` with field 2 as
/// the reason; an unreadable class becomes `human`, and missing source/reason
/// fall back to `unknown` / a placeholder. This is the one parser.
pub fn blocked_fields(raw: &str) -> Blocked {
    let raw = raw.trim().to_string();
    let parts: Vec<&str> = raw.splitn(4, '\t').collect();
    if parts.len() == 2 {
        return Blocked {
            iso: nonempty(parts[0], "unknown"),
            class: "human".into(),
            source: "unknown".into(),
            reason: nonempty(parts[1], "no reason recorded"),
            raw,
        };
    }
    let class = parts
        .get(1)
        .filter(|c| CLASSES.contains(c))
        .copied()
        .unwrap_or("human")
        .to_string();
    let source = parts
        .get(2)
        .filter(|s| !s.is_empty())
        .copied()
        .unwrap_or("unknown")
        .to_string();
    let reason = parts
        .get(3)
        .filter(|r| !r.is_empty())
        .copied()
        .unwrap_or("no reason recorded")
        .to_string();
    let iso = parts
        .first()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "unknown".into());
    Blocked {
        iso,
        class,
        source,
        reason,
        raw,
    }
}

fn nonempty(value: &str, fallback: &str) -> String {
    if value.is_empty() {
        fallback.into()
    } else {
        value.into()
    }
}

pub fn read_lossy(path: &Path) -> String {
    match fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => String::new(),
    }
}

/// A block for `run`, `retry` and `status`: the marker's existence is the
/// stop, even if the line was damaged to empty. `diagnose` deliberately uses
/// `read_blocked` instead: its Python predecessor treated an empty marker as
/// "not blocked", and the report must stay a read-only explanation.
pub fn blocked_marker(path: &Path) -> Option<Blocked> {
    if path.is_file() {
        Some(blocked_fields(&read_lossy(path)))
    } else {
        None
    }
}

pub fn read_blocked(path: &Path) -> Option<Blocked> {
    let raw = read_lossy(path);
    if raw.trim().is_empty() {
        None
    } else {
        Some(blocked_fields(&raw))
    }
}

pub fn lines(path: &Path) -> Vec<String> {
    read_lossy(path)
        .lines()
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect()
}

/// `state/lock` is live only while its pid names a running process. The
/// directory alone is not evidence: SIGKILL, a reboot, or a failed pid write
/// leaves the directory behind, and treating it as a live run would suppress
/// the stale alarm forever.
pub fn lock_live(state: &Path) -> bool {
    let lock = state.join("lock");
    if !lock.is_dir() {
        return false;
    }
    let pid = read_lossy(&lock.join("pid"));
    let pid = pid.trim();
    if pid.is_empty() || !pid.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    #[cfg(unix)]
    {
        let kill = if Path::new("/bin/kill").is_file() {
            "/bin/kill"
        } else {
            "kill"
        };
        std::process::Command::new(kill)
            .arg("-0")
            .arg(pid)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// A run held today is file evidence that the day guard, the lock or a busy
/// resource decided not to work; it must not read as a missed run.
pub fn held_today(state: &Path, today: &str) -> bool {
    let prefix = format!("{today}T");
    for line in lines(&state.join("last")) {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 && parts[0].starts_with(&prefix) && parts[1] == "0" {
            return true;
        }
    }
    false
}

#[derive(Clone, Debug)]
pub struct Run {
    pub start: String,
    pub end: String,
    pub rc: i64,
    pub verb: String,
    pub unit: String,
    pub seconds: i64,
}

/// `state/runs.tsv` is
/// `<start-iso>TAB<end-iso>TAB<rc>TAB<verb>TAB<unit>TAB<seconds>`.
/// A short or malformed line is skipped, not allowed to hide the rest.
pub fn run_history(state: &Path, limit: usize) -> Vec<Run> {
    let mut out = Vec::new();
    for line in lines(&state.join("runs.tsv")) {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 6 {
            continue;
        }
        let (Ok(rc), Ok(seconds)) = (parts[2].parse::<i64>(), parts[5].parse::<i64>()) else {
            continue;
        };
        out.push(Run {
            start: parts[0].into(),
            end: parts[1].into(),
            rc,
            verb: parts[3].into(),
            unit: parts[4].into(),
            seconds,
        });
    }
    if out.len() > limit {
        out.split_off(out.len() - limit)
    } else {
        out
    }
}

/// The newest run that was not a deliberate hold. A hold exits 0 without
/// writing `done/<unit>`, so a run did work when its unit's marker was written
/// at or after the run began, or when it failed at all. The scan stops at the
/// newest row that passes, because a later work run rewrote the marker and an
/// older row's timestamp would then read the newer write.
pub fn last_work_run<'a>(state: &Path, runs: &'a [Run]) -> Option<&'a Run> {
    runs.iter()
        .rev()
        .find(|run| run.rc != 0 || wrote_done(state, run))
}

fn wrote_done(state: &Path, run: &Run) -> bool {
    // The unit is a filename from a TSV line: refuse anything that would walk
    // out of `done/`, because the test below would then describe another file.
    if run.unit.is_empty() || run.unit == "." || run.unit == ".." || run.unit.contains(['/', '\\'])
    {
        return false;
    }
    let Some(start) = clock::parse_civil(&run.start).and_then(|civil| civil.epoch()) else {
        return false;
    };
    let Ok(modified) = fs::metadata(state.join("done").join(&run.unit)).and_then(|m| m.modified())
    else {
        return false;
    };
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .is_ok_and(|at| at >= start)
}

pub fn last_run_row(state: &Path) -> Option<(String, Option<i64>)> {
    let mut rows: Vec<String> = lines(&state.join("last"))
        .into_iter()
        .filter(|l| !l.trim().is_empty())
        .collect();
    let row = rows.pop()?;
    let parts: Vec<&str> = row.split('\t').collect();
    if parts.len() < 3 {
        return None;
    }
    Some((parts[0].to_string(), parts[1].parse::<i64>().ok()))
}

/// Cents booked today in `state/ledger.tsv`; only today's date prefix counts.
pub fn spend_today(state: &Path, today: &str) -> i64 {
    let mut total = 0i64;
    for line in lines(&state.join("ledger.tsv")) {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 2 || !parts[0].starts_with(today) {
            continue;
        }
        if let Ok(cents) = parts[1].parse::<i64>() {
            total += cents;
        }
    }
    total
}

/// `state/expect` is one line, `HH:MM` local. Returns
/// `(minutes since midnight, "HH:MM")`.
pub fn expect_time(state: &Path) -> Option<(i64, String)> {
    let raw = fs::read(state.join("expect")).ok()?;
    let body = String::from_utf8_lossy(&raw);
    let body = body.trim();
    let line = body.lines().next().unwrap_or("").trim();
    let bytes = line.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return None;
    }
    if !bytes[..2].iter().all(|c| c.is_ascii_digit())
        || !bytes[3..].iter().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let hour: i64 = line[..2].parse().ok()?;
    let minute: i64 = line[3..].parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((hour * 60 + minute, line.to_string()))
}

/// `%Y-%m-%dT%H:%M:%S%z` for a file's mtime, or None when it is gone.
pub fn mtime_iso(path: &Path) -> Option<String> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let epoch = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    Some(clock::iso(epoch))
}

/// Number of entries in `state/attempts` beginning `<today>.attempt-`.
pub fn attempts_today(state: &Path, today: &str) -> i64 {
    count_matching(&state.join("attempts"), &format!("{today}.attempt-"))
}

/// Every job attempt on disk: `state/attempts/<date>.attempt-N`. A stage's
/// file is `<date>.<id>.attempt-N`, a different counter, so a second dot before
/// `attempt-` disqualifies the name.
pub fn attempts_total(state: &Path) -> i64 {
    let Ok(entries) = fs::read_dir(state.join("attempts")) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| is_day_attempt(&entry.file_name().to_string_lossy()))
        .count() as i64
}

fn is_day_attempt(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() > 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4].iter().all(|c| c.is_ascii_digit())
        && bytes[5..7].iter().all(|c| c.is_ascii_digit())
        && bytes[8..10].iter().all(|c| c.is_ascii_digit())
        && name[10..].starts_with(".attempt-")
}

fn count_matching(dir: &Path, prefix: &str) -> i64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .count() as i64
}

/// Recursive file count under `out/`, the same number `os.walk` produced.
pub fn out_files(dir: &Path) -> i64 {
    fn walk(dir: &Path, n: &mut i64) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(t) if t.is_dir() => walk(&entry.path(), n),
                Ok(_) => *n += 1,
                Err(_) => {}
            }
        }
    }
    let mut n = 0;
    walk(dir, &mut n);
    n
}

/// Keep the newest `limit` lines, the way `tail -50` did.
pub fn write_tail(path: &Path, lines: &[String], limit: usize) -> io::Result<()> {
    let start = lines.len().saturating_sub(limit);
    let mut body = String::new();
    for line in &lines[start..] {
        body.push_str(line);
        body.push('\n');
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&tmp, body)?;
    fs::rename(&tmp, path)
}

pub fn append(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(text.as_bytes())
}

/// A tab or newline in one field would corrupt a TSV line.
pub fn one_line(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}

/// The page's duration format: `7s`, `1m 03s`, or an em dash.
pub fn duration_text(seconds: Option<i64>) -> String {
    let Some(seconds) = seconds else {
        return "—".into();
    };
    let seconds = seconds.max(0);
    let minutes = seconds / 60;
    let secs = seconds % 60;
    if minutes > 0 {
        format!("{minutes}m {secs:02}s")
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_field_and_legacy_blocks() {
        let b = blocked_fields("2026-01-01T00:00:00-0600\tretryable\tsrc\treason");
        assert_eq!((b.class.as_str(), b.source.as_str()), ("retryable", "src"));
        let b = blocked_fields("2026-01-01T00:00:00-0600\treason");
        assert_eq!((b.class.as_str(), b.reason.as_str()), ("human", "reason"));
    }

    #[test]
    fn duration_reads_like_the_page() {
        assert_eq!(duration_text(None), "—");
        assert_eq!(duration_text(Some(7)), "7s");
        assert_eq!(duration_text(Some(63)), "1m 03s");
    }
}
