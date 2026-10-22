//! The manager-level parts of bounded auto-repair: the loop lock, attempt
//! records and the workspace heal budget.
//!
//! The loop itself lives in `commands` because it has to call `run` and the
//! repair registry; this module owns the files those calls leave behind.

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::agent;
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
        Self::acquire_at(state.join("heal").join("lock"), LOCK_TTL)
    }

    pub(crate) fn acquire_at(path: PathBuf, ttl: i64) -> LockResult {
        if let Some(parent) = path.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                return LockResult::Failed(format!("cannot create {}: {error}", parent.display()));
            }
        }
        for _ in 0..2 {
            match fs::create_dir(&path) {
                Ok(()) => {
                    let token = format!("{}-{}", std::process::id(), clock::now_epoch());
                    let _ = fs::write(path.join("pid"), format!("{}\n", std::process::id()));
                    let _ = fs::write(path.join("token"), format!("{token}\n"));
                    let _ = fs::write(
                        path.join("expiry"),
                        format!("{}\n", clock::now_epoch() + ttl),
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
            expiry: clock::now_epoch() + ttl,
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
    /// The resolved profile, once the caller knows it. `record_attempt` copies
    /// it into `meta.env` and the history's agent column.
    pub agent: Option<String>,
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
                    agent: None,
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
    pub agent: Option<&'a str>,
}

pub fn record_attempt(paths: &Paths, attempt: &Attempt, record: &Record) {
    let action = state::one_line(record.action);
    let before = state::one_line(record.before_key);
    let after = state::one_line(record.after_key);
    let verdict = state::one_line(record.verdict);
    let cost = record.cost.unwrap_or("");
    let agent = record.agent.map(state::one_line).unwrap_or_default();

    let _ = fs::write(attempt.dir.join("before"), format!("{before}\n"));
    let _ = fs::write(attempt.dir.join("after"), format!("{after}\n"));
    let _ = fs::write(attempt.dir.join("verdict"), format!("{verdict}\n"));
    let _ = fs::write(
        attempt.dir.join("meta.env"),
        format!(
            "action={action}\nbefore={before}\nafter={after}\nverdict={verdict}\nrc={}\ncost={cost}\nagent={agent}\n",
            record.rc
        ),
    );

    let seconds = (clock::now_epoch() - attempt.started).max(0);
    let line = format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        clock::iso(attempt.started),
        paths.name,
        attempt.number,
        action,
        before,
        after,
        verdict,
        record.rc,
        seconds,
        cost,
        agent
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

pub const DEFAULT_RETRY_BACKOFF: u64 = 30;
pub const MAX_RETRY_BACKOFF: u64 = 600;

/// Seconds a policy `retry` action waits before rerunning the job.
///
/// A transient upstream error is often gone within seconds, but a scheduler
/// must not inherit an unbounded sleep from its environment, so the value is
/// capped and an invalid one is a configuration error rather than a default.
pub fn retry_backoff() -> Result<u64, String> {
    match env::var("LIGHTSOUT_RETRY_BACKOFF") {
        Ok(value) if !value.is_empty() => {
            let seconds = value
                .parse::<u64>()
                .map_err(|_| "LIGHTSOUT_RETRY_BACKOFF must be a whole number".to_string())?;
            if seconds > MAX_RETRY_BACKOFF {
                return Err(format!(
                    "LIGHTSOUT_RETRY_BACKOFF must be at most {MAX_RETRY_BACKOFF} seconds"
                ));
            }
            Ok(seconds)
        }
        _ => Ok(DEFAULT_RETRY_BACKOFF),
    }
}

pub const DEFAULT_AGENT_TIMEOUT: u64 = 600;

pub const SCHEMA_TEXT: &str =
    "The manager validates this exact schema; any unknown top-level field is rejected.";

pub enum AgentOutcome {
    Proposal(Value),
    NoProposal(String),
    Timeout,
    RunnerFailed(i32),
}

pub fn timeout_seconds() -> Result<u64, String> {
    match env::var("U_AGENT_TIMEOUT") {
        Ok(value) if !value.is_empty() => value
            .parse::<u64>()
            .map_err(|_| "U_AGENT_TIMEOUT must be a whole number".to_string()),
        _ => Ok(DEFAULT_AGENT_TIMEOUT),
    }
}

pub fn allowed_prefixes(job_dir: &Path) -> Vec<String> {
    let path = job_dir.join("repair.allow");
    let mut out = Vec::new();
    if let Ok(body) = fs::read_to_string(&path) {
        for line in body.lines() {
            let text = line.split('#').next().unwrap_or("").trim();
            if !text.is_empty() {
                out.push(text.to_string());
            }
        }
    }
    if out.is_empty() {
        out.push("out/".into());
    }
    out
}

/// Render `lib/prompts/repair.md` with the job's state, then redact it.
/// The caller writes `prompt.md`; this function returns the redacted text.
pub fn build_prompt(
    paths: &Paths,
    blocked: &state::Blocked,
    attempt: &Attempt,
) -> Result<String, String> {
    let template_path = paths.repo.join("lib").join("prompts").join("repair.md");
    let template = fs::read_to_string(&template_path)
        .map_err(|error| format!("cannot read {}: {error}", template_path.display()))?;

    let spec = capped(&state::read_lossy(&paths.dir.join("spec.md")), 8192);
    let diagnosis = capped(&state::read_lossy(&paths.state.join("diagnosis.md")), 16384);
    let diagnosis = if diagnosis.trim().is_empty() {
        "not generated; run lightsout diagnose for a report".to_string()
    } else {
        diagnosis
    };
    let log_tail = log_tail(&paths.log, 80, 32768);
    let attempts = history_tail(paths, 5);
    let allowed = allowed_prefixes(&paths.dir).join(", ");
    let proposal_path = attempt.dir.join("proposal.json");
    let blocked_text = if blocked.raw.trim().is_empty() {
        format!(
            "{}\t{}\t{}\t{}",
            blocked.iso, blocked.class, blocked.source, blocked.reason
        )
    } else {
        blocked.raw.clone()
    };

    let mut text = template;
    for (key, value) in [
        ("JOB", paths.name.clone()),
        ("UNIT", paths.unit.clone()),
        ("SPEC", spec),
        ("BLOCKED", blocked_text),
        ("DIAGNOSIS", diagnosis),
        ("LOG_TAIL", log_tail),
        ("ATTEMPTS", attempts),
        ("ALLOWED", allowed),
        ("PROPOSAL_PATH", proposal_path.display().to_string()),
        ("SCHEMA", SCHEMA_TEXT.to_string()),
    ] {
        text = text.replace(&format!("{{{{{key}}}}}"), &value);
    }

    let (text, redactions) = redact(&text);
    let _ = fs::write(attempt.dir.join("redactions"), format!("{redactions}\n"));
    Ok(text)
}

fn capped(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    text.chars().take(limit).collect()
}

fn log_tail(path: &Path, lines: usize, cap: usize) -> String {
    let body = state::read_lossy(path);
    let all: Vec<&str> = body.lines().collect();
    let start = all.len().saturating_sub(lines);
    capped(&all[start..].join("\n"), cap)
}

fn history_tail(paths: &Paths, lines: usize) -> String {
    let body = state::read_lossy(&paths.state.join("heal").join("history.tsv"));
    let all: Vec<&str> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if all.is_empty() {
        return "none".into();
    }
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

pub fn call_agent(
    paths: &Paths,
    attempt: &Attempt,
    prompt_path: &Path,
    timeout: u64,
    profile: &agent::Resolved,
) -> AgentOutcome {
    let proposal_path = attempt.dir.join("proposal.json");
    let log_path = attempt.dir.join("runner.log");
    let timeout_path = attempt.dir.join("timeout");
    let _ = fs::remove_file(&timeout_path);

    let Ok(log) = fs::File::create(&log_path) else {
        return AgentOutcome::NoProposal("cannot create runner.log".into());
    };
    let stderr = log.try_clone().ok();

    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(r#". "$1/lib/heal.sh"; u_heal_agent "$2""#)
        .arg("lightsout")
        .arg(&paths.repo)
        .arg(prompt_path)
        .current_dir(&paths.repo)
        .env("REPO", &paths.repo)
        .env("JOB", &paths.name)
        .env("JOB_DIR", &paths.dir)
        .env("STATE", &paths.state)
        .env("OUT", &paths.out)
        .env("LOG", &paths.log)
        .env("UNIT", &paths.unit)
        .env("DRY", "0")
        .env("U_AGENT_TIMEOUT", timeout.to_string())
        .env("U_RUNNER", &profile.runner)
        .env("U_HEAL_RUNNER", &profile.runner)
        .env("U_AGENT_PROFILE", profile.describe())
        .env("LIGHTSOUT_HEAL_OUT", &proposal_path)
        .env("LIGHTSOUT_HEAL_TIMEOUT", &timeout_path)
        .stdout(Stdio::from(log));
    match &profile.model {
        Some(model) => cmd.env("U_AGENT_MODEL", model),
        None => cmd.env_remove("U_AGENT_MODEL"),
    };
    match &profile.patch {
        Some(patch) => cmd.env("U_AGENT_PATCH", patch),
        None => cmd.env_remove("U_AGENT_PATCH"),
    };
    match &profile.effort {
        Some(effort) => cmd.env("U_AGENT_EFFORT", effort),
        None => cmd.env_remove("U_AGENT_EFFORT"),
    };
    if let Some(stderr) = stderr {
        cmd.stderr(Stdio::from(stderr));
    }

    let status = cmd.status();
    if timeout_path.is_file() {
        let _ = fs::remove_file(&timeout_path);
        return AgentOutcome::Timeout;
    }
    let code = match status {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            return AgentOutcome::NoProposal(format!("cannot start the runner: {error}"));
        }
    };
    if code == 124 {
        return AgentOutcome::Timeout;
    }
    if code != 0 {
        return AgentOutcome::RunnerFailed(code);
    }

    match read_proposal(&proposal_path) {
        Some(value) => AgentOutcome::Proposal(value),
        None => {
            let log = state::read_lossy(&log_path);
            if let Some(value) = proposal_from_log(&log) {
                let _ = fs::write(
                    &proposal_path,
                    serde_json::to_string_pretty(&value).unwrap_or_default(),
                );
                return AgentOutcome::Proposal(value);
            }
            AgentOutcome::NoProposal("the runner exited 0 but wrote no JSON proposal".into())
        }
    }
}

fn read_proposal(path: &Path) -> Option<Value> {
    if fs::metadata(path).ok()?.len() > 1024 * 1024 {
        return None;
    }
    let body = fs::read_to_string(path).ok()?;
    serde_json::from_str(&body).ok()
}

fn proposal_from_log(text: &str) -> Option<Value> {
    let mut in_json = false;
    let mut buf = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if in_json {
                if let Ok(value) = serde_json::from_str(&buf) {
                    return Some(value);
                }
                buf.clear();
                in_json = false;
            } else if trimmed == "```json" {
                in_json = true;
                buf.clear();
            }
            continue;
        }
        if in_json {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    None
}

pub fn reserve_workspace_attempt(repo: &Path, job: &str, action: &str) -> Result<(), String> {
    let base = repo.join("logs").join("heal");
    fs::create_dir_all(&base)
        .map_err(|error| format!("cannot create {}: {error}", base.display()))?;
    let lock = match Lock::acquire_at(base.join("lock"), 60) {
        LockResult::Acquired(lock) => lock,
        LockResult::Held { .. } => {
            return Err("the workspace heal lock is busy; try again".into());
        }
        LockResult::Failed(error) => return Err(error),
    };
    let file = base.join(format!("{}.tsv", clock::today()));
    let count = state::lines(&file)
        .into_iter()
        .filter(|line| !line.trim().is_empty())
        .count();
    let max = crate::triage::max_heals_total();
    if count >= max {
        return Err(format!(
            "workspace heal cap reached ({count} of {max} today)"
        ));
    }
    state::append(
        &file,
        &format!(
            "{}\t{}\t{}\n",
            clock::iso_now(),
            state::one_line(job),
            state::one_line(action)
        ),
    )
    .map_err(|error| format!("cannot append the workspace heal ledger: {error}"))?;
    drop(lock);
    Ok(())
}

fn redact(text: &str) -> (String, usize) {
    let mut out = text.to_string();
    let mut count = 0usize;
    for prefix in ["sk-", "ghp_", "github_pat_", "xoxb-", "xoxp-", "AKIA"] {
        let (next, n) = redact_prefix(&out, prefix);
        out = next;
        count += n;
    }
    let (next, n) = redact_bearer(&out);
    out = next;
    count += n;
    let (next, n) = redact_assignments(&out);
    out = next;
    count += n;

    for (name, value) in env::vars() {
        if value.len() < 8 {
            continue;
        }
        let upper = name.to_ascii_uppercase();
        if !(upper.contains("TOKEN")
            || upper.contains("SECRET")
            || upper.contains("PASSWORD")
            || upper.contains("API_KEY"))
        {
            continue;
        }
        let hits = out.matches(value.as_str()).count();
        if hits > 0 {
            out = out.replace(value.as_str(), "[redacted]");
            count += hits;
        }
    }
    (out, count)
}

fn redact_prefix(text: &str, prefix: &str) -> (String, usize) {
    let mut out = String::new();
    let mut rest = text;
    let mut count = 0;
    while let Some(index) = rest.find(prefix) {
        out.push_str(&rest[..index]);
        let tail = &rest[index..];
        let end = tail
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';' | ')' | ']' | '}')
            })
            .unwrap_or(tail.len());
        out.push_str("[redacted]");
        count += 1;
        rest = &tail[end..];
    }
    out.push_str(rest);
    (out, count)
}

fn redact_bearer(text: &str) -> (String, usize) {
    let mut out = String::new();
    let mut rest = text;
    let mut count = 0;
    while let Some(index) = rest.find("Bearer ") {
        out.push_str(&rest[..index]);
        out.push_str("Bearer ");
        let tail = &rest[index + "Bearer ".len()..];
        let end = tail
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';' | ')' | ']' | '}')
            })
            .unwrap_or(tail.len());
        if end == 0 {
            out.push_str(tail);
            rest = "";
            break;
        }
        out.push_str("[redacted]");
        count += 1;
        rest = &tail[end..];
    }
    out.push_str(rest);
    (out, count)
}

fn redact_assignments(text: &str) -> (String, usize) {
    let mut out = String::new();
    let mut count = 0;
    for line in text.split_inclusive('\n') {
        let mut line_out = line.to_string();
        for key in ["api_key", "apikey", "password", "secret", "token"] {
            let lower = line_out.to_ascii_lowercase();
            let Some(position) = lower.find(key) else {
                continue;
            };
            let after = &line_out[position + key.len()..];
            let Some(separator) = after.find(['=', ':']) else {
                continue;
            };
            let value_start = position + key.len() + separator + 1;
            let rest = &line_out[value_start..];
            let trimmed = rest.trim_start();
            let skip = rest.len() - trimmed.len();
            let end = trimmed
                .find(|c: char| {
                    c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';' | ')' | ']' | '}')
                })
                .unwrap_or(trimmed.len());
            if end == 0 {
                continue;
            }
            line_out.replace_range(value_start + skip..value_start + skip + end, "[redacted]");
            count += 1;
        }
        out.push_str(&line_out);
    }
    (out, count)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditKind {
    Content(String),
    Delete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub path: PathBuf,
    pub kind: EditKind,
}

#[derive(Clone, Debug)]
pub struct Refusal {
    pub rule: &'static str,
    pub message: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.rule, self.message)
    }
}

fn refusal(rule: &'static str, message: impl Into<String>) -> Refusal {
    Refusal {
        rule,
        message: message.into(),
    }
}

const EDIT_LIMIT: usize = 20;
const CONTENT_LIMIT: usize = 2 * 1024 * 1024;
const SUMMARY_LIMIT: usize = 2000;

pub fn validate(job_dir: &Path, allow: &[PathBuf], proposal: &Value) -> Result<Vec<Edit>, Refusal> {
    let object = proposal
        .as_object()
        .ok_or_else(|| refusal("schema", "the proposal must be a JSON object"))?;
    for key in object.keys() {
        if !matches!(key.as_str(), "fixable" | "summary" | "edits") {
            return Err(refusal("schema", format!("unknown field '{key}'")));
        }
    }
    let fixable = object
        .get("fixable")
        .and_then(Value::as_bool)
        .ok_or_else(|| refusal("schema", "fixable must be true or false"))?;
    let summary = object
        .get("summary")
        .and_then(Value::as_str)
        .ok_or_else(|| refusal("schema", "summary must be a string"))?;
    if summary.trim().is_empty() || summary.chars().count() > SUMMARY_LIMIT {
        return Err(refusal(
            "summary",
            format!("summary must be 1-{SUMMARY_LIMIT} characters"),
        ));
    }
    let edits = object
        .get("edits")
        .and_then(Value::as_array)
        .ok_or_else(|| refusal("schema", "edits must be an array"))?;
    if edits.len() > EDIT_LIMIT {
        return Err(refusal(
            "edits",
            format!("at most {EDIT_LIMIT} edits are allowed"),
        ));
    }
    if !fixable {
        if edits.is_empty() {
            return Ok(Vec::new());
        }
        return Err(refusal(
            "fixable-false-with-edits",
            "fixable:false requires an empty edits list",
        ));
    }
    if edits.is_empty() {
        return Err(refusal(
            "no-edits",
            "fixable:true requires at least one edit",
        ));
    }

    let prefixes = normalize_allow(allow);
    let mut out = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut total = 0usize;
    for (index, edit) in edits.iter().enumerate() {
        let fields = edit
            .as_object()
            .ok_or_else(|| refusal("schema", format!("edit {} is not an object", index + 1)))?;
        for key in fields.keys() {
            if !matches!(key.as_str(), "path" | "content" | "delete") {
                return Err(refusal(
                    "schema",
                    format!("edit {} has unknown field '{key}'", index + 1),
                ));
            }
        }
        let raw = fields
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| refusal("schema", format!("edit {} needs a path", index + 1)))?;
        let path = normalize_path(raw)?;
        if seen.iter().any(|seen| seen == &path) {
            return Err(refusal(
                "duplicate",
                format!("edit {} repeats path '{}'", index + 1, path.display()),
            ));
        }
        seen.push(path.clone());

        if is_protected(&path) {
            return Err(refusal(
                "protected",
                format!("path '{}' is reserved for the manager", path.display()),
            ));
        }
        if !prefixes.iter().any(|prefix| path.starts_with(prefix)) {
            return Err(refusal(
                "allow",
                format!(
                    "path '{}' is outside the allowlist ({})",
                    path.display(),
                    prefixes
                        .iter()
                        .map(|prefix| prefix.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
        let full = job_dir.join(&path);
        check_components(job_dir, &path)?;

        let has_content = fields.contains_key("content");
        let has_delete = fields.contains_key("delete");
        if has_content == has_delete {
            return Err(refusal(
                "schema",
                format!(
                    "edit {} must have exactly one of content or delete",
                    index + 1
                ),
            ));
        }
        if has_delete {
            match fields.get("delete").and_then(Value::as_bool) {
                Some(true) => {}
                _ => {
                    return Err(refusal(
                        "schema",
                        format!("edit {} has delete:false; omit it or use true", index + 1),
                    ));
                }
            }
            if !full.is_file() {
                return Err(refusal(
                    "delete-target",
                    format!("delete target '{}' is not a regular file", path.display()),
                ));
            }
            out.push(Edit {
                path,
                kind: EditKind::Delete,
            });
            continue;
        }

        let content = fields
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| refusal("schema", format!("edit {} content must be text", index + 1)))?;
        total = total.saturating_add(content.len());
        if total > CONTENT_LIMIT {
            return Err(refusal(
                "size",
                format!("total content exceeds {CONTENT_LIMIT} bytes"),
            ));
        }
        let parent = full
            .parent()
            .ok_or_else(|| refusal("parent", format!("'{}' has no parent", path.display())))?;
        if !parent.is_dir() {
            return Err(refusal(
                "parent",
                format!(
                    "parent '{}' is missing or not a directory",
                    parent.display()
                ),
            ));
        }
        if full.exists() && !full.is_file() {
            return Err(refusal(
                "target",
                format!("'{}' is not a regular file", path.display()),
            ));
        }
        out.push(Edit {
            path,
            kind: EditKind::Content(content.to_string()),
        });
    }
    Ok(out)
}

fn normalize_allow(allow: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for prefix in allow {
        if let Ok(path) = normalize_path(&prefix.to_string_lossy()) {
            if !path.as_os_str().is_empty() {
                out.push(path);
            }
        }
    }
    if out.is_empty() {
        out.push(PathBuf::from("out"));
    }
    out
}

fn normalize_path(raw: &str) -> Result<PathBuf, Refusal> {
    if raw.is_empty() {
        return Err(refusal("path", "path is empty"));
    }
    if raw.contains('\\') {
        return Err(refusal("path", "path must not contain a backslash"));
    }
    // `applied.tsv` is line-oriented: a newline or tab in a path would forge a
    // second record, and `revert` would then act on a path the proposal never
    // named. NUL and the other control characters are refused for the same
    // reason, and because no real artifact name needs them.
    if raw.chars().any(char::is_control) {
        return Err(refusal("path", "path must not contain control characters"));
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(refusal("path", "path must be relative"));
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(name) => out.push(name),
            std::path::Component::CurDir => {}
            _ => {
                return Err(refusal(
                    "escape",
                    format!("path '{raw}' must not leave the job directory"),
                ));
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(refusal("path", "path has no file name"));
    }
    Ok(out)
}

fn is_protected(path: &Path) -> bool {
    let components: Vec<&str> = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect();
    if components.is_empty() {
        return true;
    }
    if components.contains(&".git") {
        return true;
    }
    if components[0].starts_with('.') {
        return true;
    }
    if matches!(components[0], "state" | "logs" | "lib" | "spec") {
        return true;
    }
    if components.len() == 1
        && matches!(
            components[0],
            "job.sh" | "spec.md" | "repair.allow" | "repairs.sh"
        )
    {
        return true;
    }
    false
}

fn check_components(job_dir: &Path, path: &Path) -> Result<(), Refusal> {
    let mut current = job_dir.to_path_buf();
    let mut parts: Vec<&std::ffi::OsStr> = path
        .components()
        .map(|component| component.as_os_str())
        .collect();
    let last = parts.pop();
    for part in parts {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(refusal(
                    "symlink",
                    format!("'{}' is a symlink", current.display()),
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(refusal(
                    "parent",
                    format!("'{}' is not a directory", current.display()),
                ));
            }
            Ok(_) => {}
            Err(_) => {
                return Err(refusal(
                    "parent",
                    format!("'{}' is missing", current.display()),
                ));
            }
        }
    }
    if let Some(last) = last {
        current.push(last);
        if let Ok(metadata) = fs::symlink_metadata(&current) {
            if metadata.file_type().is_symlink() {
                return Err(refusal(
                    "symlink",
                    format!("'{}' is a symlink", current.display()),
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct ApplyError {
    pub message: String,
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

fn apply_error(message: impl Into<String>) -> ApplyError {
    ApplyError {
        message: message.into(),
    }
}

#[derive(Debug, Default)]
pub struct Applied {
    pub edited: Vec<PathBuf>,
    pub created: Vec<PathBuf>,
}

pub fn apply(attempt_dir: &Path, job_dir: &Path, edits: &[Edit]) -> Result<Applied, ApplyError> {
    let mut applied: Vec<(PathBuf, bool)> = Vec::new();
    let mut result: Result<(), ApplyError> = Ok(());

    for edit in edits {
        let rel = match normalize_path(&edit.path.to_string_lossy()) {
            Ok(rel) => rel,
            Err(refusal) => {
                result = Err(apply_error(refusal.to_string()));
                break;
            }
        };
        let full = job_dir.join(&rel);
        if !full.starts_with(job_dir) || is_protected(&rel) {
            result = Err(apply_error(
                "edit escapes the job directory or is protected",
            ));
            break;
        }
        if let Err(refusal) = check_components(job_dir, &rel) {
            result = Err(apply_error(refusal.to_string()));
            break;
        }
        match &edit.kind {
            EditKind::Content(content) => {
                let existed = full.exists();
                if existed {
                    if let Err(error) = backup(attempt_dir, job_dir, &rel) {
                        result = Err(error);
                        break;
                    }
                }
                if let Err(error) = write_atomic(&full, content) {
                    result = Err(error);
                    break;
                }
                applied.push((rel, existed));
            }
            EditKind::Delete => {
                if !full.is_file() {
                    result = Err(apply_error(format!(
                        "delete target '{}' is not a regular file",
                        rel.display()
                    )));
                    break;
                }
                if let Err(error) = backup(attempt_dir, job_dir, &rel) {
                    result = Err(error);
                    break;
                }
                if let Err(error) = fs::remove_file(&full) {
                    result = Err(apply_error(format!(
                        "cannot delete '{}': {error}",
                        rel.display()
                    )));
                    break;
                }
                applied.push((rel, true));
            }
        }
    }

    if result.is_ok() {
        if let Err(error) = write_applied(attempt_dir, &applied) {
            result = Err(error);
        }
    }
    if let Err(error) = result {
        rollback(attempt_dir, job_dir, &applied);
        return Err(error);
    }

    let mut out = Applied::default();
    for (rel, existed) in applied {
        out.edited.push(rel.clone());
        if !existed {
            out.created.push(rel);
        }
    }
    Ok(out)
}

fn backup(attempt_dir: &Path, job_dir: &Path, rel: &Path) -> Result<(), ApplyError> {
    let source = job_dir.join(rel);
    let target = attempt_dir.join("backup").join(rel);
    if target.is_file() {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            apply_error(format!(
                "cannot create backup dir {}: {error}",
                parent.display()
            ))
        })?;
    }
    fs::copy(&source, &target).map_err(|error| {
        apply_error(format!(
            "cannot back up '{}' to '{}': {error}",
            source.display(),
            target.display()
        ))
    })?;
    let permissions = fs::metadata(&source)
        .map_err(|error| apply_error(format!("cannot read {}: {error}", source.display())))?
        .permissions();
    let _ = fs::set_permissions(&target, permissions);
    Ok(())
}

fn write_atomic(path: &Path, content: &str) -> Result<(), ApplyError> {
    let parent = path
        .parent()
        .ok_or_else(|| apply_error(format!("'{}' has no parent", path.display())))?;
    let permissions = fs::metadata(path).ok().map(|meta| meta.permissions());
    let temp = parent.join(format!(
        ".lightsout-tmp-{}-{}",
        std::process::id(),
        clock::now_epoch()
    ));
    {
        let mut file = fs::File::create(&temp)
            .map_err(|error| apply_error(format!("cannot create {}: {error}", temp.display())))?;
        if let Err(error) = file.write_all(content.as_bytes()) {
            let _ = fs::remove_file(&temp);
            return Err(apply_error(format!(
                "cannot write {}: {error}",
                temp.display()
            )));
        }
        if let Err(error) = file.sync_all() {
            let _ = fs::remove_file(&temp);
            return Err(apply_error(format!(
                "cannot sync {}: {error}",
                temp.display()
            )));
        }
    }
    if let Some(permissions) = permissions {
        let _ = fs::set_permissions(&temp, permissions);
    }
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(apply_error(format!(
            "cannot replace '{}': {error}",
            path.display()
        )));
    }
    Ok(())
}

fn write_applied(attempt_dir: &Path, applied: &[(PathBuf, bool)]) -> Result<(), ApplyError> {
    let mut body = String::new();
    for (rel, existed) in applied {
        // The path is JSON-quoted so a tab or newline can never split one
        // record into two. Older attempts wrote the raw path; `revert` still
        // reads that form, but every new record is escaped.
        let encoded = serde_json::to_string(&rel.to_string_lossy())
            .map_err(|error| apply_error(format!("cannot encode an applied path: {error}")))?;
        body.push_str(&format!(
            "{encoded}\t{}\n",
            if *existed { "replaced" } else { "created" }
        ));
    }
    fs::write(attempt_dir.join("applied.tsv"), body)
        .map_err(|error| apply_error(format!("cannot record applied edits: {error}")))
}

fn rollback(attempt_dir: &Path, job_dir: &Path, applied: &[(PathBuf, bool)]) {
    for (rel, existed) in applied.iter().rev() {
        let full = job_dir.join(rel);
        if *existed {
            let backup = attempt_dir.join("backup").join(rel);
            let _ = fs::copy(&backup, &full);
        } else {
            let _ = fs::remove_file(&full);
        }
    }
}

pub fn revert(attempt_dir: &Path, job_dir: &Path) -> Result<usize, ApplyError> {
    let body = fs::read_to_string(attempt_dir.join("applied.tsv"))
        .map_err(|error| apply_error(format!("no applied record: {error}")))?;
    let allowed: Vec<PathBuf> = allowed_prefixes(job_dir)
        .into_iter()
        .map(PathBuf::from)
        .collect();
    let prefixes = normalize_allow(&allowed);
    let mut restored = 0usize;
    for line in body.lines() {
        let mut parts = line.split('\t');
        let Some(raw) = parts.next() else {
            continue;
        };
        let kind = parts.next().unwrap_or("replaced");
        // New records quote the path; older attempts wrote it raw. Accept both,
        // but never let a forged record name a path outside the allowlist.
        let raw = if raw.starts_with('"') {
            serde_json::from_str::<String>(raw).map_err(|error| {
                apply_error(format!("cannot decode the applied path {raw}: {error}"))
            })?
        } else {
            raw.to_string()
        };
        let rel = normalize_path(&raw).map_err(|refusal| apply_error(refusal.to_string()))?;
        if is_protected(&rel) {
            return Err(apply_error(format!(
                "'{}' is protected and cannot be reverted through",
                rel.display()
            )));
        }
        if !prefixes.iter().any(|prefix| rel.starts_with(prefix)) {
            return Err(apply_error(format!(
                "'{}' is outside the current repair allowlist",
                rel.display()
            )));
        }
        if let Err(refusal) = check_components(job_dir, &rel) {
            return Err(apply_error(refusal.to_string()));
        }
        let full = job_dir.join(&rel);
        if kind == "created" {
            if full.exists() {
                fs::remove_file(&full).map_err(|error| {
                    apply_error(format!("cannot remove '{}': {error}", full.display()))
                })?;
                restored += 1;
            }
        } else {
            let backup = attempt_dir.join("backup").join(&rel);
            if !backup.is_file() {
                return Err(apply_error(format!(
                    "backup '{}' is missing",
                    backup.display()
                )));
            }
            let permissions = fs::metadata(&backup).ok().map(|meta| meta.permissions());
            fs::copy(&backup, &full).map_err(|error| {
                apply_error(format!("cannot restore '{}': {error}", full.display()))
            })?;
            if let Some(permissions) = permissions {
                let _ = fs::set_permissions(&full, permissions);
            }
            restored += 1;
        }
    }
    let _ = fs::write(
        attempt_dir.join("reverted"),
        format!("{}\n", clock::iso_now()),
    );
    Ok(restored)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    entries: Vec<(PathBuf, u64, u64)>,
}

/// A metadata snapshot of everything an agent must not edit during its run.
/// The current job's `state/heal/` directory is excluded because the runner and
/// the manager write the prompt, proposal and log there on purpose; every other
/// job and code directory is in scope, because an agent that can edit a
/// neighbouring job's `job.sh` has escaped the job it was asked to repair.
pub fn fingerprint(paths: &Paths) -> Fingerprint {
    let mut entries = Vec::new();
    let skip = paths.state.join("heal");
    let workspace = &paths.repo;
    // A registered job may live outside its workspace, so its own directory is
    // walked too; a workspace job is already under jobs/ and dedups below.
    walk_fingerprint(
        workspace,
        &workspace.join("jobs"),
        Some(&skip),
        &mut entries,
    );
    walk_fingerprint(workspace, &paths.dir, Some(&skip), &mut entries);
    for dir in ["lib", "spec", "bin", "src", "tests", "packaging"] {
        walk_fingerprint(workspace, &workspace.join(dir), None, &mut entries);
    }
    for rel in [
        "triage.conf",
        "agents.conf",
        "lightsout.toml",
        "Cargo.toml",
        "Cargo.lock",
        "install.sh",
    ] {
        let path = workspace.join(rel);
        if let Ok(meta) = fs::metadata(&path) {
            entries.push((
                path.strip_prefix(workspace).unwrap_or(&path).to_path_buf(),
                meta.len(),
                modified(meta),
            ));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries.dedup_by(|a, b| a.0 == b.0);
    Fingerprint { entries }
}

fn walk_fingerprint(
    repo: &Path,
    dir: &Path,
    skip: Option<&Path>,
    out: &mut Vec<(PathBuf, u64, u64)>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if skip.map(|skip| path == skip).unwrap_or(false) {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            walk_fingerprint(repo, &path, skip, out);
        } else {
            out.push((
                path.strip_prefix(repo).unwrap_or(&path).to_path_buf(),
                meta.len(),
                modified(meta),
            ));
        }
    }
}

fn modified(meta: fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
}

pub fn first_change(before: &Fingerprint, after: &Fingerprint) -> Option<String> {
    let mut left = before.entries.iter().peekable();
    let mut right = after.entries.iter().peekable();
    loop {
        match (left.peek(), right.peek()) {
            (None, None) => return None,
            (Some(_), None) => return left.next().map(|(path, _, _)| path.display().to_string()),
            (None, Some(_)) => return right.next().map(|(path, _, _)| path.display().to_string()),
            (Some(a), Some(b)) => match a.0.cmp(&b.0) {
                std::cmp::Ordering::Less => {
                    return Some(a.0.display().to_string());
                }
                std::cmp::Ordering::Greater => {
                    return Some(b.0.display().to_string());
                }
                std::cmp::Ordering::Equal => {
                    if a.1 != b.1 || a.2 != b.2 {
                        return Some(a.0.display().to_string());
                    }
                    left.next();
                    right.next();
                }
            },
        }
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

    fn job_dir(label: &str) -> PathBuf {
        let root = temp(label);
        fs::create_dir_all(root.join("out")).unwrap();
        root
    }

    fn value(body: &str) -> Value {
        serde_json::from_str(body).unwrap()
    }

    fn allow_out() -> Vec<PathBuf> {
        vec![PathBuf::from("out")]
    }

    #[test]
    fn apply_backs_up_and_replaces() {
        let dir = job_dir("apply-ok");
        let attempt = dir.join("attempt");
        fs::create_dir_all(&attempt).unwrap();
        fs::write(dir.join("out/a.txt"), "before").unwrap();
        let edits = vec![Edit {
            path: PathBuf::from("out/a.txt"),
            kind: EditKind::Content("after".into()),
        }];
        let applied = apply(&attempt, &dir, &edits).expect("apply");
        assert_eq!(fs::read_to_string(dir.join("out/a.txt")).unwrap(), "after");
        assert_eq!(
            fs::read_to_string(attempt.join("backup/out/a.txt")).unwrap(),
            "before"
        );
        assert_eq!(applied.edited.len(), 1);
        assert!(applied.created.is_empty());
    }

    #[test]
    fn apply_rolls_back_when_a_later_edit_fails() {
        let dir = job_dir("apply-rollback");
        let attempt = dir.join("attempt");
        fs::create_dir_all(&attempt).unwrap();
        fs::write(dir.join("out/a.txt"), "before").unwrap();
        fs::create_dir_all(dir.join("out/b")).unwrap();
        let edits = vec![
            Edit {
                path: PathBuf::from("out/a.txt"),
                kind: EditKind::Content("after".into()),
            },
            Edit {
                path: PathBuf::from("out/b"),
                kind: EditKind::Content("nope".into()),
            },
        ];
        assert!(apply(&attempt, &dir, &edits).is_err());
        assert_eq!(fs::read_to_string(dir.join("out/a.txt")).unwrap(), "before");
        assert!(!attempt.join("applied.tsv").exists());
    }

    #[test]
    fn revert_restores_replaced_and_removes_created() {
        let dir = job_dir("revert");
        let attempt = dir.join("attempt");
        fs::create_dir_all(&attempt).unwrap();
        fs::write(dir.join("out/a.txt"), "before").unwrap();
        let edits = vec![
            Edit {
                path: PathBuf::from("out/a.txt"),
                kind: EditKind::Content("after".into()),
            },
            Edit {
                path: PathBuf::from("out/new.txt"),
                kind: EditKind::Content("new".into()),
            },
        ];
        apply(&attempt, &dir, &edits).expect("apply");
        assert_eq!(fs::read_to_string(dir.join("out/new.txt")).unwrap(), "new");
        let restored = revert(&attempt, &dir).expect("revert");
        assert_eq!(restored, 2);
        assert_eq!(fs::read_to_string(dir.join("out/a.txt")).unwrap(), "before");
        assert!(!dir.join("out/new.txt").exists());
    }

    #[test]
    fn validate_accepts_a_safe_edit() {
        let dir = job_dir("validate-ok");
        let edits = validate(
            &dir,
            &allow_out(),
            &value(r#"{"fixable":true,"summary":"ok","edits":[{"path":"out/new.txt","content":"hello"}]}"#),
        )
        .expect("safe edit");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].path, PathBuf::from("out/new.txt"));
    }

    #[test]
    fn validate_rejects_too_many_edits() {
        let dir = job_dir("validate-many");
        let mut edits = Vec::new();
        for i in 0..21 {
            edits.push(format!("{{\"path\":\"out/{i}.txt\",\"content\":\"x\"}}"));
        }
        let body = format!(
            "{{\"fixable\":true,\"summary\":\"many\",\"edits\":[{}]}}",
            edits.join(",")
        );
        let refused = validate(&dir, &allow_out(), &value(&body)).expect_err("21 edits");
        assert_eq!(refused.rule, "edits");
    }

    #[test]
    fn validate_rejects_oversized_content() {
        let dir = job_dir("validate-size");
        let content = "x".repeat(2 * 1024 * 1024 + 1);
        let body = value(&format!(
            "{{\"fixable\":true,\"summary\":\"big\",\"edits\":[{{\"path\":\"out/big.txt\",\"content\":\"{content}\"}}]}}"
        ));
        let refused = validate(&dir, &allow_out(), &body).expect_err("too big");
        assert_eq!(refused.rule, "size");
    }

    #[test]
    fn validate_rejects_an_unknown_field() {
        let dir = job_dir("validate-schema");
        let refused = validate(
            &dir,
            &allow_out(),
            &value(r#"{"fixable":true,"summary":"ok","commands":[],"edits":[{"path":"out/x","content":"x"}]}"#),
        )
        .expect_err("unknown field");
        assert_eq!(refused.rule, "schema");
    }

    #[test]
    fn validate_rejects_fixable_false_with_edits() {
        let dir = job_dir("validate-false");
        let refused = validate(
            &dir,
            &allow_out(),
            &value(r#"{"fixable":false,"summary":"no","edits":[{"path":"out/x","content":"x"}]}"#),
        )
        .expect_err("false with edits");
        assert_eq!(refused.rule, "fixable-false-with-edits");
    }

    #[test]
    fn validate_rejects_a_protected_path() {
        let dir = job_dir("validate-protected");
        let refused = validate(
            &dir,
            &[PathBuf::from("job.sh")],
            &value(r#"{"fixable":true,"summary":"bad","edits":[{"path":"job.sh","content":"x"}]}"#),
        )
        .expect_err("protected");
        assert_eq!(refused.rule, "protected");
    }

    #[test]
    fn validate_rejects_a_delete_of_a_missing_file() {
        let dir = job_dir("validate-delete");
        let refused = validate(
            &dir,
            &allow_out(),
            &value(r#"{"fixable":true,"summary":"bad","edits":[{"path":"out/missing.bin","delete":true}]}"#),
        )
        .expect_err("missing delete");
        assert_eq!(refused.rule, "delete-target");
    }

    #[cfg(unix)]
    #[test]
    fn validate_rejects_a_symlinked_component() {
        let dir = job_dir("validate-symlink");
        let outside = dir.join("outside");
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("out/link")).unwrap();
        let refused = validate(
            &dir,
            &[PathBuf::from("out")],
            &value(
                r#"{"fixable":true,"summary":"bad","edits":[{"path":"out/link/x","content":"x"}]}"#,
            ),
        )
        .expect_err("symlink");
        assert_eq!(refused.rule, "symlink");
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
