//! `triage.conf` — the human-edited policy that routes a blocked job.
//!
//! The file is a small TSV grammar with five actions. The manager reads it and
//! never writes it; `triage` prints the decision, `heal` and
//! `run --auto-repair` consume it. A missing file is not an error: every block
//! falls back to `human`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::clock;
use crate::commands;
use crate::repo::Paths;
use crate::state::{self, Blocked};

pub const POLICY: &str = "triage.conf";
pub const DEFAULT_MAX_HEALS: usize = 2;
pub const DEFAULT_MAX_HEALS_TOTAL: usize = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Human,
    Retry,
    Repair(String),
    AgentPropose,
    AgentApply,
}

impl Action {
    pub fn label(&self) -> String {
        match self {
            Action::Human => "human".into(),
            Action::Retry => "retry".into(),
            Action::Repair(name) => format!("repair:{name}"),
            Action::AgentPropose => "agent:propose".into(),
            Action::AgentApply => "agent:apply".into(),
        }
    }

    pub fn mode(&self) -> &'static str {
        match self {
            Action::Human => "human",
            Action::Retry => "retry",
            Action::Repair(_) => "repair",
            Action::AgentPropose => "propose",
            Action::AgentApply => "apply",
        }
    }

    pub fn is_agent(&self) -> bool {
        matches!(self, Action::AgentPropose | Action::AgentApply)
    }
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub job: String,
    pub source: String,
    pub action: Action,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct Policy {
    rules: Vec<Rule>,
    present: bool,
    origin: String,
}

#[derive(Clone, Debug)]
pub struct PolicyError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line == 0 {
            write!(f, "{}", self.message)
        } else {
            write!(f, "line {}: {}", self.line, self.message)
        }
    }
}

#[derive(Clone, Debug)]
pub struct Resolution {
    pub action: Action,
    pub matched: Option<usize>,
    pub note: Option<String>,
}

impl Policy {
    pub fn load(path: &Path) -> Result<Policy, PolicyError> {
        let body = match fs::read_to_string(path) {
            Ok(body) => body,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Policy {
                    rules: Vec::new(),
                    present: false,
                    origin: path.display().to_string(),
                });
            }
            Err(error) => {
                return Err(PolicyError {
                    line: 0,
                    message: format!("cannot read {}: {error}", path.display()),
                });
            }
        };
        Policy::parse(&path.display().to_string(), &body)
    }

    pub fn parse(origin: &str, body: &str) -> Result<Policy, PolicyError> {
        let mut rules: Vec<Rule> = Vec::new();
        let mut seen: Vec<(String, String)> = Vec::new();
        for (index, raw) in body.lines().enumerate() {
            let line = index + 1;
            let text = raw.split('#').next().unwrap_or("").trim();
            if text.is_empty() {
                continue;
            }
            let fields: Vec<&str> = text.split_whitespace().collect();
            if fields.len() != 3 {
                return Err(PolicyError {
                    line,
                    message: format!(
                        "expected 3 fields (job, source, action), found {}",
                        fields.len()
                    ),
                });
            }
            let job = fields[0];
            let source = fields[1];
            validate_job(job).map_err(|message| PolicyError { line, message })?;
            if source.is_empty() || source.chars().any(|c| c.is_control()) {
                return Err(PolicyError {
                    line,
                    message: "source must be a non-empty token or *".into(),
                });
            }
            let action =
                parse_action(fields[2]).map_err(|message| PolicyError { line, message })?;
            if job != "*" && source != "*" && seen.iter().any(|(j, s)| j == job && s == source) {
                return Err(PolicyError {
                    line,
                    message: format!("duplicate rule for ({job}, {source})"),
                });
            }
            seen.push((job.to_string(), source.to_string()));
            rules.push(Rule {
                job: job.to_string(),
                source: source.to_string(),
                action,
                line,
            });
        }
        Ok(Policy {
            rules,
            present: true,
            origin: origin.to_string(),
        })
    }

    pub fn is_present(&self) -> bool {
        self.present
    }

    pub fn has_agent(&self) -> bool {
        self.rules.iter().any(|rule| rule.action.is_agent())
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// `None` means no rule matched, which is the same as `human`.
    pub fn resolve(&self, job: &str, source: &str) -> Resolution {
        let mut best: Option<(u8, &Rule)> = None;
        for rule in &self.rules {
            let score = match (
                rule.job == job,
                rule.job == "*",
                rule.source == source,
                rule.source == "*",
            ) {
                (true, _, true, _) => 4,
                (_, true, true, _) => 2,
                (true, _, _, true) => 3,
                (_, true, _, true) => 1,
                _ => 0,
            };
            if score > 0 && best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                best = Some((score, rule));
            }
        }
        match best {
            Some((_, rule)) => Resolution {
                action: rule.action.clone(),
                matched: Some(rule.line),
                note: None,
            },
            None => Resolution {
                action: Action::Human,
                matched: None,
                note: Some(if self.present {
                    "no rule matched; default human".into()
                } else {
                    "no triage.conf; default human".into()
                }),
            },
        }
    }

    pub fn check_repairs(&self, repo: &Path) -> Result<(), PolicyError> {
        let known = commands::registered_repairs(repo);
        for rule in &self.rules {
            if let Action::Repair(name) = &rule.action {
                if !known.iter().any(|candidate| candidate == name) {
                    return Err(PolicyError {
                        line: rule.line,
                        message: format!(
                            "unknown repair '{name}' (registered: {})",
                            if known.is_empty() {
                                "none".to_string()
                            } else {
                                known.join(", ")
                            }
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    pub fn warnings(&self, repo: &Path) -> Vec<String> {
        let mut out = Vec::new();
        for rule in &self.rules {
            if rule.job != "*" && !Paths::new(repo, &rule.job).has_job() {
                out.push(format!(
                    "triage.conf:{}: rule names a missing job '{}'",
                    rule.line, rule.job
                ));
            }
        }
        out
    }

    pub fn matched_text(&self, resolution: &Resolution) -> String {
        match resolution.matched {
            Some(line) => self
                .rules
                .iter()
                .find(|rule| rule.line == line)
                .map(|rule| format!("{}\t{}\t{}", rule.job, rule.source, rule.action.label()))
                .unwrap_or_else(|| "none".into()),
            None => "none".into(),
        }
    }
}

fn validate_job(job: &str) -> Result<(), String> {
    if job == "*" {
        return Ok(());
    }
    if job.is_empty()
        || job == "."
        || job == ".."
        || job
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
    {
        return Err(format!(
            "job '{job}' must be a directory name, letters/digits/._- or *"
        ));
    }
    Ok(())
}

fn parse_action(text: &str) -> Result<Action, String> {
    match text {
        "human" => Ok(Action::Human),
        "retry" => Ok(Action::Retry),
        "agent:apply" => Ok(Action::AgentApply),
        "agent:propose" => Ok(Action::AgentPropose),
        _ => match text.strip_prefix("repair:") {
            Some(name) if valid_repair_name(name) => Ok(Action::Repair(name.to_string())),
            Some(name) => Err(format!(
                "repair name '{name}' must be lower-case letters, digits or dash"
            )),
            None => Err(format!(
                "unknown action '{text}' (use human, retry, agent:apply, agent:propose or repair:<name>)"
            )),
        },
    }
}

fn valid_repair_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The stable identity used to decide whether a new failure is the same one.
/// A free-form source is taken at its word; an `unknown` source falls back to
/// the gate name in the reason, because `u_gate` writes `gate failed: <name>`.
pub fn failure_key(blocked: &Blocked) -> String {
    if !blocked.source.is_empty() && blocked.source != "unknown" {
        return blocked.source.clone();
    }
    if let Some(rest) = blocked.reason.strip_prefix("gate failed: ") {
        if let Some(name) = rest.split_whitespace().next() {
            if !name.is_empty() {
                return format!("gate:{name}");
            }
        }
    }
    "unknown".into()
}

pub fn decide(policy: &Policy, job: &str, blocked: &Blocked) -> Resolution {
    let mut resolution = policy.resolve(job, &blocked.source);
    let refused = match &resolution.action {
        Action::Retry => Some("retry"),
        Action::Repair(_) => Some("repair"),
        _ => None,
    };
    if let Some(what) = refused {
        if blocked.class != "retryable" {
            resolution.note = Some(format!(
                "class {} is not retryable; {} refused and human is used",
                blocked.class, what
            ));
            resolution.action = Action::Human;
        }
    }
    if blocked.class == "retryable" && resolution.action.is_agent() {
        resolution.note = Some(
            "the block is retryable and a deterministic retry or repair may be cheaper; agent action kept"
                .into(),
        );
    }
    resolution
}

pub fn heal_count(state: &Path) -> usize {
    let history = state.join("heal").join("history.tsv");
    let today = clock::today();
    state::lines(&history)
        .into_iter()
        .filter(|line| {
            let mut fields = line.split('\t');
            let iso = fields.next().unwrap_or("");
            let action = fields.nth(2).unwrap_or("");
            iso.starts_with(&today) && action.starts_with("agent")
        })
        .count()
}

pub fn workspace_heal_count(repo: &Path) -> usize {
    let path = repo
        .join("logs")
        .join("heal")
        .join(format!("{}.tsv", clock::today()));
    state::lines(&path)
        .into_iter()
        .filter(|line| !line.trim().is_empty())
        .count()
}

pub fn max_heals() -> usize {
    env_usize("MAX_HEALS", DEFAULT_MAX_HEALS)
}

pub fn max_heals_total() -> usize {
    env_usize("MAX_HEALS_TOTAL", DEFAULT_MAX_HEALS_TOTAL)
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
}

pub fn policy_path(repo: &Path) -> PathBuf {
    match env::var("LIGHTSOUT_TRIAGE") {
        Ok(path) if !path.is_empty() => PathBuf::from(path),
        _ => repo.join(POLICY),
    }
}

pub fn run(repo: &Path, argv: &[String]) -> i32 {
    let mut name: Option<String> = None;
    let mut json_output = false;
    for arg in argv {
        match arg.as_str() {
            "--json" => json_output = true,
            "-h" | "--help" => {
                println!("usage: lightsout triage <job> [--json]");
                return 0;
            }
            other if other.starts_with('-') => {
                eprintln!("triage: unknown option: {other}");
                return 64;
            }
            other => {
                if name.is_some() {
                    eprintln!("triage: one job at a time");
                    return 64;
                }
                name = Some(other.to_string());
            }
        }
    }
    let Some(name) = name else {
        eprintln!("usage: lightsout triage <job> [--json]");
        return 64;
    };
    let paths = Paths::new(repo, &name);
    if !paths.has_job() {
        eprintln!("no such job: {name}");
        return 64;
    }
    let Some(blocked) = state::blocked_marker(&paths.state.join("blocked")) else {
        println!("{name} is not blocked; nothing to triage");
        return 0;
    };

    let path = policy_path(&paths.repo);
    let policy = match Policy::load(&path) {
        Ok(policy) => policy,
        Err(error) => {
            eprintln!("{}: {error}", path.display());
            return 64;
        }
    };
    if let Err(error) = policy.check_repairs(&paths.repo) {
        eprintln!("{}: {error}", path.display());
        return 64;
    }
    let resolution = decide(&policy, &name, &blocked);
    let profile = match crate::agent::resolve(repo, &name, true) {
        Ok(profile) => Some(profile),
        Err(error) => {
            if resolution.action.is_agent() {
                eprintln!("agent profile: {error}");
                return 64;
            }
            eprintln!("agent profile: {error}");
            None
        }
    };
    let key = failure_key(&blocked);
    let agent_json = profile
        .as_ref()
        .filter(|profile| profile.configured || resolution.action.is_agent())
        .map(|profile| profile.to_json());
    let heals = heal_count(&paths.state);
    let workspace = workspace_heal_count(&paths.repo);
    let max = max_heals();
    let max_total = max_heals_total();
    let matched = policy.matched_text(&resolution);

    if json_output {
        let body = json!({
            "job": name,
            "blocked": {
                "iso": blocked.iso,
                "class": blocked.class,
                "source": blocked.source,
                "reason": blocked.reason,
            },
            "action": resolution.action.label(),
            "mode": resolution.action.mode(),
            "matched_rule": matched,
            "policy_present": policy.is_present(),
            "heals_today": heals,
            "max_heals": max,
            "workspace_heals_today": workspace,
            "max_heals_total": max_total,
            "key": key,
            "agent": agent_json,
            "note": resolution.note,
        });
        println!("{}", serde_json::to_string(&body).expect("triage json"));
        return 0;
    }

    println!(
        "job {name} is blocked: [{}] {} - {}",
        blocked.class, key, blocked.reason
    );
    match resolution.matched {
        Some(_) => println!(
            "policy: {} (matched {})",
            resolution.action.label(),
            matched
        ),
        None => println!(
            "policy: {} ({}; {})",
            resolution.action.label(),
            resolution
                .note
                .clone()
                .unwrap_or_else(|| "no rule matched".into()),
            policy.origin()
        ),
    }
    if let Some(note) = &resolution.note {
        if resolution.matched.is_some() {
            println!("note: {note}");
        }
    }
    println!("heals today: {heals} of {max}; workspace: {workspace} of {max_total}");
    if let Some(profile) = &profile {
        if profile.configured || resolution.action.is_agent() {
            println!("agent: {}", profile.describe());
        }
    }
    println!("next: {}", next_command(&name, &resolution.action));
    0
}

fn next_command(name: &str, action: &Action) -> String {
    match action {
        Action::Human => format!("fix the cause, then: lightsout unblock {name}"),
        Action::Retry => format!("lightsout run {name} --auto-repair"),
        Action::Repair(repair) => format!("lightsout retry {name} --repair {repair}"),
        Action::AgentPropose => {
            format!("review the proposal, then: lightsout heal {name} --apply")
        }
        Action::AgentApply => {
            format!("lightsout heal {name} (or: lightsout run {name} --auto-repair)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked(source: &str, reason: &str) -> Blocked {
        Blocked {
            raw: String::new(),
            iso: "2026-10-21T09:00:00-05:00".into(),
            class: "judgment".into(),
            source: source.into(),
            reason: reason.into(),
        }
    }

    #[test]
    fn failure_key_prefers_the_recorded_source() {
        assert_eq!(
            failure_key(&blocked("gate:exists", "anything")),
            "gate:exists"
        );
    }

    #[test]
    fn failure_key_derives_a_gate_from_the_reason() {
        assert_eq!(
            failure_key(&blocked(
                "unknown",
                "gate failed: duration --min 59 --max 61"
            )),
            "gate:duration"
        );
        assert_eq!(
            failure_key(&blocked("unknown", "voice was 48 s")),
            "unknown"
        );
    }

    #[test]
    fn duplicate_exact_rules_are_rejected() {
        let error = Policy::parse(
            "triage.conf",
            "noop gate:exists human\nnoop gate:exists agent:apply\n",
        )
        .expect_err("duplicate");
        assert_eq!(error.line, 2);
        assert!(error.message.contains("duplicate"), "{}", error.message);
    }

    #[test]
    fn precedence_is_exact_job_source_default() {
        let policy = Policy::parse(
            "triage.conf",
            "\
* * human
* gate:exists agent:propose
noop * agent:apply
noop gate:exists human
",
        )
        .expect("policy");
        assert_eq!(policy.resolve("noop", "gate:exists").action, Action::Human);
        assert_eq!(policy.resolve("noop", "other").action, Action::AgentApply);
        assert_eq!(
            policy.resolve("other", "gate:exists").action,
            Action::AgentPropose
        );
        assert_eq!(policy.resolve("other", "other").action, Action::Human);
    }

    #[test]
    fn a_repair_rule_on_a_judgment_block_becomes_human() {
        let policy = Policy::parse("triage.conf", "noop * repair:touch-missing\n").expect("policy");
        let resolution = decide(&policy, "noop", &blocked("gate:exists", "bad"));
        assert_eq!(resolution.action, Action::Human);
        assert!(resolution.note.unwrap().contains("not retryable"));
    }

    #[test]
    fn retry_is_parsed_and_refused_for_a_judgment_block() {
        let policy = Policy::parse("triage.conf", "noop * retry\n").expect("policy");
        assert_eq!(policy.resolve("noop", "sweep").action, Action::Retry);

        let resolution = decide(&policy, "noop", &blocked("gate:exists", "bad"));
        assert_eq!(resolution.action, Action::Human);
        assert!(resolution.note.unwrap().contains("retry refused"));
    }
}
