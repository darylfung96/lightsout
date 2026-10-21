//! `diagnose` — explain why a job is stopped, writing only `state/diagnosis.md`.
//!
//! The block is input, never something this report clears or edits. The file
//! is the only write; a human reads it without disturbing the stop.

use std::fs;
use std::path::{Path, PathBuf};

use crate::clock;
use crate::repo::Paths;
use crate::state;

const CLASS_HELP: [(&str, &str); 3] = [
    (
        "retryable",
        "transient: an allowlisted repair may clear the cause. The repair changes the world, never the verdict — the gate still decides.",
    ),
    (
        "judgment",
        "the artifact did not meet its gate. A human edits the artifact, then the same gate decides again.",
    ),
    (
        "human",
        "a human must change the environment first: credentials, disk space, or the decision the reason names.",
    ),
];

fn class_help(class: &str) -> &'static str {
    CLASS_HELP
        .iter()
        .find(|(name, _)| *name == class)
        .map(|(_, help)| *help)
        .unwrap_or(CLASS_HELP[2].1)
}

fn human_age(seconds: i64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h{:02}m", seconds / 3600, seconds % 3600 / 60)
    } else {
        format!("{}d{:02}h", seconds / 86400, seconds % 86400 / 3600)
    }
}

fn human_size(n: u64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    let mut value = n as f64;
    for unit in ["KB", "MB", "GB"] {
        value /= 1024.0;
        if value < 1024.0 || unit == "GB" {
            return format!("{value:.1} {unit}");
        }
    }
    format!("{value:.1} GB")
}

fn rel(repo: &Path, path: &Path) -> String {
    path.strip_prefix(repo)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn out_files(out_dir: &Path) -> Vec<(String, u64)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, u64)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                walk(base, &path, out);
            } else {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                let name = path.strip_prefix(base).unwrap_or(&path);
                out.push((name.to_string_lossy().into_owned(), size));
            }
        }
    }
    let mut out = Vec::new();
    walk(out_dir, out_dir, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn line_stamp(line: &str) -> Option<i64> {
    if !line.starts_with('[') {
        return None;
    }
    let close = line.find(']')?;
    let stamp = &line[1..close];
    let civil = clock::parse_civil(stamp)?;
    (stamp.len() >= 10).then(|| civil.naive_seconds())
}

fn log_tail(lines: &[String], block_at: Option<i64>) -> Vec<String> {
    let chosen: Vec<String> = match block_at {
        Some(when) => {
            let cutoff = when + 1;
            lines
                .iter()
                .filter(|line| match line_stamp(line) {
                    Some(stamp) => stamp <= cutoff,
                    None => true,
                })
                .cloned()
                .collect()
        }
        None => {
            let mut cut = lines.len();
            for (i, line) in lines.iter().enumerate().rev() {
                if line.contains("BLOCKED") {
                    cut = i + 1;
                    break;
                }
            }
            lines[..cut].to_vec()
        }
    };
    let start = chosen.len().saturating_sub(40);
    chosen[start..].to_vec()
}

fn gate_failure(reason: &str) -> Option<(String, String)> {
    let at = reason.find("gate failed:")?;
    let rest = reason[at + "gate failed:".len()..].trim_start();
    let mut parts = rest.splitn(2, char::is_whitespace);
    let gate = parts.next()?.to_string();
    if gate.is_empty() {
        return None;
    }
    let args = parts.next().unwrap_or("").trim().to_string();
    Some((gate, args))
}

struct ReasonBullets {
    body: String,
}

fn reason_bullets(block: &state::Blocked) -> ReasonBullets {
    let low = block.reason.to_lowercase();
    let mut out = Vec::new();
    let gate = gate_failure(&block.reason);
    if let Some((name, rest)) = &gate {
        let target = rest.split_whitespace().collect::<Vec<_>>();
        let mut where_ = String::new();
        if let Some(i) = target.iter().position(|a| *a == "--file") {
            if let Some(path) = target.get(i + 1) {
                where_ = format!(" on `{path}`");
            }
        }
        out.push(format!(
            "- gate `{name}` rejected the artifact{where_}; the artifact is the suspect, not the runner."
        ));
    }
    if low.contains("attempt cap") {
        out.push(
            "- the day's attempt cap is spent: the same stage has failed repeatedly, so another tick would only repeat it."
                .into(),
        );
    }
    if low.contains("low disk") {
        out.push(
            "- free space is under the job's threshold; find what the job has been writing before rerunning."
                .into(),
        );
    }
    if low.contains("memory pass") {
        out.push(
            "- the memory pass (the last stage) failed; check the notes it saved and the lint/receipt output."
                .into(),
        );
    }
    if gate.is_none()
        && !low.contains("attempt cap")
        && !low.contains("low disk")
        && !low.contains("memory pass")
    {
        out.push(
            "- no gate line explains it; the reason above is the only verdict on record.".into(),
        );
    }
    if !block.source.is_empty() && block.source != "unknown" {
        out.push(format!("- raised by `{}`.", block.source));
    }
    ReasonBullets {
        body: out.join("\n"),
    }
}

fn registered_repairs(repo: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let body = state::read_lossy(&repo.join("lib").join("repairs.sh"));
    for line in body.lines() {
        let Some(rest) = line.strip_prefix("repair_") else {
            continue;
        };
        let Some(open) = rest.find('(') else {
            continue;
        };
        let name = &rest[..open];
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            continue;
        }
        if rest[open..].trim_start().starts_with("()") {
            names.push(name.to_string());
        }
    }
    names
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".into();
    }
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_-./:=@".contains(c))
    {
        return value.into();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn gate_command(reason: &str) -> String {
    let Some((gate, rest)) = gate_failure(reason) else {
        return String::new();
    };
    let mut command = format!("bin/lightsout gate {}", shell_quote(&gate));
    for arg in rest.split_whitespace() {
        command.push(' ');
        command.push_str(&shell_quote(arg));
    }
    command
}

pub fn build(repo: &Path, paths: &Paths) -> String {
    let name = &paths.name;
    let block = state::read_blocked(&paths.state.join("blocked"));
    let log_path = paths.log.clone();
    let log_lines: Vec<String> = state::lines(&log_path)
        .into_iter()
        .filter(|l| !l.trim().is_empty())
        .collect();
    let block_seconds = block
        .as_ref()
        .and_then(|b| clock::parse_civil(&b.iso))
        .map(|c| c.naive_seconds());
    let tail = log_tail(&log_lines, block_seconds);
    let last_fail = log_lines
        .iter()
        .rfind(|line| line.contains("FAIL "))
        .cloned()
        .unwrap_or_default();
    let files = out_files(&paths.out);
    let age = block.as_ref().and_then(|block| {
        if let Some(epoch) = clock::parse_civil(&block.iso).and_then(|c| c.epoch()) {
            Some((clock::now_epoch() - epoch).max(0))
        } else {
            block_seconds.map(|secs| (clock::now_naive_seconds() - secs).max(0))
        }
    });

    let mut lines: Vec<String> = vec![
        format!("# Diagnosis — {name}"),
        String::new(),
        format!(
            "_Read-only report from `bin/job diagnose {name}`. It explains the current stop from files; it does not change `state/blocked`, `state/last`, or any artifact._"
        ),
        String::new(),
    ];
    if let Some(block) = &block {
        let mut since = format!("blocked since `{}`", block.iso);
        if let Some(age) = age {
            since.push_str(&format!(" ({} ago)", human_age(age)));
        }
        lines.push(format!(
            "**Status:** {since} — class `{}`, source `{}`",
            block.class, block.source
        ));
        lines.push(format!("**Reason:** {}", block.reason));
    } else {
        lines.push("**Status:** not blocked (no `state/blocked` marker)".into());
    }

    lines.push(String::new());
    lines.push("## Cause candidates".into());
    lines.push(String::new());
    if let Some(block) = &block {
        lines.push(format!(
            "- class `{}` — {}",
            block.class,
            class_help(&block.class)
        ));
        let bullets = reason_bullets(block);
        if !bullets.body.is_empty() {
            lines.push(bullets.body);
        }
        if !last_fail.is_empty() {
            lines.push("- the last explicit `FAIL` line in today's log is quoted below.".into());
        }
        if let Some(age) = age {
            if age >= 3600 {
                lines.push(format!(
                    "- the block has stood for {}; nothing has run since.",
                    human_age(age)
                ));
            }
        }
    } else {
        lines.push(
            "- no block is recorded. If the job stopped, the log tail below is the only evidence on disk."
                .into(),
        );
    }

    lines.push(String::new());
    lines.push("## Evidence".into());
    lines.push(String::new());
    if let Some(block) = &block {
        lines.push(format!(
            "- `{}`, byte for byte:",
            rel(repo, &paths.state.join("blocked"))
        ));
        lines.push(String::new());
        lines.push("```tsv".into());
        lines.push(block.raw.clone());
        lines.push("```".into());
        lines.push(String::new());
    }
    if !last_fail.is_empty() {
        lines.push("- last failing gate line in today's log:".into());
        lines.push(String::new());
        lines.push("```log".into());
        lines.push(last_fail);
        lines.push("```".into());
        lines.push(String::new());
    } else {
        lines.push(
            "- no `FAIL` line in today's log (the block was raised directly, or nothing failed)."
                .into(),
        );
    }
    if !tail.is_empty() {
        lines.push(format!(
            "- last {} log line(s) up to the block (`{}`):",
            tail.len(),
            rel(repo, &log_path)
        ));
        lines.push(String::new());
        lines.push("```log".into());
        lines.extend(tail);
        lines.push("```".into());
        lines.push(String::new());
    } else {
        lines.push(format!(
            "- log `{}` is empty or missing.",
            rel(repo, &log_path)
        ));
    }
    if !files.is_empty() {
        let total: u64 = files.iter().map(|(_, size)| *size).sum();
        lines.push(format!(
            "- `out/` holds {} file(s), {}:",
            files.len(),
            human_size(total)
        ));
        lines.push(String::new());
        lines.push("```out".into());
        for (path, size) in files.iter().take(25) {
            lines.push(format!("{path}\t{}", human_size(*size)));
        }
        lines.push("```".into());
        if files.len() > 25 {
            lines.push(format!("- {} more file(s) not shown.", files.len() - 25));
        }
        lines.push(String::new());
    } else {
        lines.push("- `out/` is empty: no artifact was produced.".into());
    }

    lines.push("## Next commands".into());
    lines.push(String::new());
    match block.as_ref().map(|b| b.class.as_str()) {
        None => {
            lines.push("```sh".into());
            lines.push("# nothing to clear; see what would run now:".into());
            lines.push(format!("bin/job plan {name}"));
            lines.push(format!("bin/job run {name}"));
            lines.push("```".into());
        }
        Some("retryable") => {
            lines.push("```sh".into());
            lines.push("# run the allowlisted repair; the gate still decides".into());
            lines.push(format!("bin/job retry {name}"));
            lines.push(format!("bin/job status {name}"));
            lines.push("```".into());
            let repairs = registered_repairs(repo);
            if !repairs.is_empty() {
                lines.push(String::new());
                lines.push(format!(
                    "Registered in `lib/repairs.sh`: {}.",
                    repairs
                        .iter()
                        .map(|r| format!("`{r}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        Some("judgment") => {
            lines.push("```sh".into());
            lines.push("# 1. edit the artifact the gate rejected".into());
            lines.push("# 2. clear the block, then rerun so the same gate decides".into());
            lines.push(format!("bin/job unblock {name}"));
            lines.push(format!("bin/job run {name}"));
            lines.push("```".into());
            let command = block
                .as_ref()
                .map(|b| gate_command(&b.reason))
                .unwrap_or_default();
            if !command.is_empty() {
                lines.push(String::new());
                lines.push("Check the artifact by hand first:".into());
                lines.push(String::new());
                lines.push("```sh".into());
                lines.push(command);
                lines.push("```".into());
            }
        }
        Some(_) => {
            lines.push("```sh".into());
            lines.push(
                "# 1. fix what the reason names: credentials, disk space, or the decision".into(),
            );
            lines.push("# 2. clear the marker only after a human looks".into());
            lines.push(format!("bin/job unblock {name}"));
            lines.push(format!("bin/job run {name}"));
            lines.push("```".into());
        }
    }
    lines.push(String::new());
    lines.push(format!(
        "Re-run `bin/job diagnose {name}` after a change to refresh this report."
    ));
    lines.push(String::new());
    lines.join("\n")
}

pub fn run(repo: &Path, name: &str) -> i32 {
    let paths = Paths::new(repo, name);
    if !paths.has_job() {
        eprintln!("no such job: {name}");
        return 64;
    }
    // diagnose_env creates state/ only: the report is the one thing it writes.
    let _ = fs::create_dir_all(&paths.state);
    let report = build(repo, &paths);
    let path: PathBuf = paths.state.join("diagnosis.md");
    if let Err(error) = fs::write(&path, &report) {
        eprintln!("cannot write {}: {error}", path.display());
        return 1;
    }
    print!("{report}");
    if !report.ends_with('\n') {
        println!();
    }
    0
}
