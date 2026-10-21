//! The verbs that run or repair a job: `list`, `plan`, `run`, `resume`,
//! `check`, `retry`, `unblock`.
//!
//! The job itself remains shell (`jobs/<name>/job.sh` and `lib/*.sh`). This
//! module owns the wrapper: the environment the job receives, the blocked
//! marker, the history files, the stale alarm and the repair allowlist.

use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::agent;
use crate::clock;
use crate::heal;
use crate::repo::Paths;
use crate::state;
use crate::triage;

pub const USAGE: &str = "usage: lightsout [--workspace DIR] <verb> [name]

  init [dir] [--dry-run]
                       create a workspace (lightsout.toml, lib/, jobs/noop…)
  doctor               check the workspace and the tools its jobs need
  list                 jobs that exist
  status [name]        state from files; --json for machines
  report [--html PATH] the dashboard page (same data as status)
  check [name]         alarm when an expected run never happened: notifies once
                       per job per day and exits 2 if a missed run is blocked
  plan <name>          what would run now; touches nothing
  diagnose <name>      read-only report on the current stop; writes
                       state/diagnosis.md and exits 0 even when blocked
  run <name> [--class C] [--source S] [--auto-repair]
                       do the work, resuming from disk; C defaults every block
                       raised in this run (retryable|judgment|human);
                       --auto-repair routes a block through triage.conf and
                       makes bounded attempts (also LIGHTSOUT_AUTO_REPAIR=1)
  retry <name> [--repair R]
                       run the allowlisted repair R (default touch-missing)
                       for a retryable block, then the job again so the same
                       gate decides; capped by MAX_REPAIRS (default 2)
  triage <name> [--json]
                       show how the current block routes through triage.conf;
                       writes nothing
  heal <name> [--apply | --revert N]
                       run one agent repair attempt for the current block;
                       writes a proposal, changes no artifact; --apply applies
                       the latest validated proposal and reruns; --revert N
                       restores attempt N's backup
  resume <name>        same as run
  unblock <name>       clear a blocked marker after a human looks
  gate <gate> [opts]   artifact gates (see lightsout gate --help)
  version              print the binary version

global: --workspace DIR (or LIGHTSOUT_REPO)  --version  --help

exit: 0 done or held, 1 failed, 2 blocked";

/// `job_env`: the state directories every run may use.
fn job_env(repo: &Path, name: &str) -> Paths {
    let paths = Paths::new(repo, name);
    for dir in [
        paths.state.join("attempts"),
        paths.state.join("done"),
        paths.out.clone(),
        paths.dir.join("logs"),
        paths.repo.join("logs"),
    ] {
        let _ = fs::create_dir_all(dir);
    }
    paths
}

/// `retry_env`: paths only, because a refused retry must touch nothing.
fn retry_env(repo: &Path, name: &str) -> Paths {
    Paths::new(repo, name)
}

pub fn list(repo: &Path) -> i32 {
    let base = repo.join("jobs");
    let Ok(entries) = fs::read_dir(&base) else {
        return 0;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| {
            e.path().join("job.sh").is_file() && !e.file_name().to_string_lossy().starts_with('.')
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for name in names {
        println!("{name}");
    }
    0
}

fn apply_job_env(
    cmd: &mut Command,
    paths: &Paths,
    dry: bool,
    class: Option<&str>,
    source: Option<&str>,
    retrying: bool,
    agent: Option<&agent::Resolved>,
) {
    cmd.env("REPO", &paths.repo);
    cmd.env("LIGHTSOUT_REPO", &paths.repo);
    if let Ok(exe) = env::current_exe() {
        cmd.env("LIGHTSOUT_BIN", exe);
    }
    cmd.env("JOB", &paths.name);
    cmd.env("JOB_DIR", &paths.dir);
    cmd.env("STATE", &paths.state);
    cmd.env("OUT", &paths.out);
    cmd.env("LOG", &paths.log);
    cmd.env("UNIT", &paths.unit);
    cmd.env("U_ALERTS", paths.alerts());
    cmd.env("DRY", if dry { "1" } else { "0" });
    let class = class
        .map(str::to_string)
        .or_else(|| env::var("U_BLOCK_CLASS").ok())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "human".into());
    let source = source
        .map(str::to_string)
        .or_else(|| env::var("U_BLOCK_SOURCE").ok())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    cmd.env("U_BLOCK_CLASS", class);
    cmd.env("U_BLOCK_SOURCE", source);
    if retrying {
        cmd.env("U_RETRYING", "1");
    }
    if let Some(profile) = agent {
        cmd.env("U_RUNNER", &profile.runner);
        cmd.env("U_AGENT_PROFILE", profile.describe());
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
    }
}

fn child_rc(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}

fn run_job(
    repo: &Path,
    name: &str,
    verb: &str,
    dry: bool,
    class: Option<&str>,
    source: Option<&str>,
    retrying: bool,
) -> i32 {
    let profile = match agent::resolve(repo, name, false) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("agent profile: {error}");
            return 64;
        }
    };
    run_job_with(
        repo,
        name,
        verb,
        RunOptions {
            dry,
            class,
            source,
            retrying,
            agent: Some(&profile),
        },
    )
}

struct RunOptions<'a> {
    dry: bool,
    class: Option<&'a str>,
    source: Option<&'a str>,
    retrying: bool,
    agent: Option<&'a agent::Resolved>,
}

fn run_job_with(repo: &Path, name: &str, verb: &str, options: RunOptions<'_>) -> i32 {
    let RunOptions {
        dry,
        class,
        source,
        retrying,
        agent,
    } = options;
    if !repo.join("jobs").join(name).join("job.sh").is_file() {
        eprintln!("no such job: {name}");
        return 64;
    }
    let paths = job_env(repo, name);

    if !dry {
        if let Some(blocked) = state::blocked_marker(&paths.state.join("blocked")) {
            let bypass = env::var("U_RETRYING").ok().as_deref() == Some("1") || retrying;
            if !bypass {
                println!(
                    "blocked since {} [{}]: {}",
                    blocked.iso, blocked.class, blocked.reason
                );
                if blocked.class == "retryable" {
                    println!("  run the allowlisted repair: bin/job retry {}", paths.name);
                } else {
                    println!("  fix the cause, then: bin/job unblock {}", paths.name);
                }
                return 2;
            }
        }
        let now = clock::now_epoch();
        let _ = fs::write(
            paths.state.join("start"),
            format!("{}\t{}\n", clock::iso(now), now),
        );
    }

    let mut cmd = Command::new("bash");
    cmd.arg(paths.dir.join("job.sh"));
    apply_job_env(&mut cmd, &paths, dry, class, source, retrying, agent);
    let rc = match cmd.status() {
        Ok(status) => child_rc(status),
        Err(error) => {
            eprintln!("cannot run {}: {error}", paths.dir.join("job.sh").display());
            127
        }
    };

    if !dry {
        if let Err(error) = record(&paths, verb, rc) {
            eprintln!("job: cannot record the run: {error}");
        }
    }
    rc
}

/// One completed run: `state/last` keeps the last fifty lines, `state/runs.tsv`
/// keeps the same event with its duration. The duration starts at the job's
/// own `state/start` line, not at the wrapper's clock.
fn record(paths: &Paths, verb: &str, rc: i32) -> std::io::Result<()> {
    let end_epoch = clock::now_epoch();
    let end = clock::iso(end_epoch);
    let mut start = end.clone();
    let mut start_epoch = end_epoch;
    if let Ok(body) = fs::read_to_string(paths.state.join("start")) {
        if let Some(line) = body.lines().next() {
            let mut parts = line.split('\t');
            let recorded_start = parts.next().unwrap_or("");
            let recorded_epoch = parts.next().unwrap_or("");
            if !recorded_start.is_empty() {
                start = recorded_start.to_string();
            }
            if let Ok(parsed) = recorded_epoch.parse::<i64>() {
                start_epoch = parsed;
            }
        }
    }
    let seconds = (end_epoch - start_epoch).max(0);
    let unit = state::one_line(&paths.unit);

    let last_path = paths.state.join("last");
    let mut rows = state::lines(&last_path);
    rows.push(format!("{end}\t{rc}\t{verb}"));
    state::write_tail(&last_path, &rows, 50)?;

    let runs_path = paths.state.join("runs.tsv");
    state::append(
        &runs_path,
        &format!("{start}\t{end}\t{rc}\t{verb}\t{unit}\t{seconds}\n"),
    )
}

pub fn plan(repo: &Path, name: &str) -> i32 {
    match agent::resolve(repo, name, false) {
        Ok(profile) => {
            if profile.configured {
                println!("agent: {}", profile.describe());
            }
            run_job_with(
                repo,
                name,
                "plan",
                RunOptions {
                    dry: true,
                    class: None,
                    source: None,
                    retrying: false,
                    agent: Some(&profile),
                },
            )
        }
        Err(error) if error.is_unavailable() => {
            println!("warn: agent profile: {error}");
            run_job_with(
                repo,
                name,
                "plan",
                RunOptions {
                    dry: true,
                    class: None,
                    source: None,
                    retrying: false,
                    agent: None,
                },
            )
        }
        Err(error) => {
            eprintln!("agent profile: {error}");
            64
        }
    }
}

pub fn run(
    repo: &Path,
    name: &str,
    verb: &str,
    class: Option<&str>,
    source: Option<&str>,
    auto: bool,
) -> i32 {
    if auto {
        return run_auto(repo, name, class, source);
    }
    run_job(repo, name, verb, false, class, source, false)
}

enum Check {
    Quiet,
    Stale,
    Blocked,
}

/// `check [name]` — one notification per stale job per day. It never runs a
/// job; an alarm that repaired would be a second runner.
pub fn check(repo: &Path, want: Option<&str>) -> i32 {
    if let Some(name) = want {
        if !repo.join("jobs").join(name).join("job.sh").is_file() {
            eprintln!("no such job: {name}");
            return 64;
        }
    }
    let _ = fs::create_dir_all(repo.join("logs"));
    let alerts = repo.join("logs").join("alerts.log");
    let check_log = repo.join("logs").join("check.log");

    let Ok(entries) = fs::read_dir(repo.join("jobs")) else {
        return 0;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| {
            e.path().join("job.sh").is_file() && !e.file_name().to_string_lossy().starts_with('.')
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();

    let mut rc = 0;
    for name in names {
        if let Some(want) = want {
            if name != want {
                continue;
            }
        }
        if let Check::Blocked = check_job(repo, &name, &alerts, &check_log) {
            rc = 2;
        }
    }
    rc
}

fn check_job(repo: &Path, name: &str, alerts: &Path, check_log: &Path) -> Check {
    let state_dir = repo.join("jobs").join(name).join("state");
    let Some(expect) = state::expect_time_loose(&state_dir) else {
        return Check::Quiet;
    };
    if !state::is_clock(&expect) {
        eprintln!("check: {name}: state/expect is not HH:MM: {expect}");
        return Check::Quiet;
    }
    if clock::now_hm() < expect {
        return Check::Quiet;
    }
    let today = clock::today();
    if state_dir.join("done").join(&today).exists() {
        return Check::Quiet;
    }
    if state_dir.join("lock").is_dir() {
        return Check::Quiet;
    }
    if state_dir.join("blocked").is_file() {
        println!("blocked: {name} missed its {expect} run");
        return Check::Blocked;
    }
    if state::held_today(&state_dir, &today) {
        return Check::Quiet;
    }

    let marker = state_dir.join("alerts").join(format!("{today}.stale"));
    if !marker.exists() {
        let _ = fs::create_dir_all(state_dir.join("alerts"));
        let _ = fs::write(&marker, format!("{}\t{}\n", clock::iso_now(), expect));
        notify(
            repo,
            alerts,
            check_log,
            &format!("LightsOut: {name} stale"),
            &format!("no run by {expect} today"),
        );
    }
    println!("stale: {name} (expected {expect})");
    Check::Stale
}

/// The alarm uses the one channel list jobs use: `lib/notify.sh` through
/// `u_notify`. Keeping one implementation means `check` and a block reach the
/// same places with the same timeout.
fn notify(repo: &Path, alerts: &Path, check_log: &Path, title: &str, body: &str) {
    let script = r#". "$REPO/lib/guards.sh"; u_notify "$1" "$2""#;
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(script)
        .arg("lightsout")
        .arg(title)
        .arg(body);
    cmd.env("REPO", repo)
        .env("U_ALERTS", alerts)
        .env("LOG", check_log);
    let _ = cmd.status();
}

/// The allowlist is the set of `repair_<name>()` functions in `lib/repairs.sh`.
pub(crate) fn registered_repairs(repo: &Path) -> Vec<String> {
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
    names.sort();
    names
}

fn say(log: &Path, message: &str) {
    let line = format!("[{}] {message}", clock::log_stamp(clock::now_epoch()));
    println!("{line}");
    let _ = state::append(log, &format!("{line}\n"));
}

pub fn retry(repo: &Path, argv: &[String]) -> i32 {
    let Some(name) = argv.first() else {
        println!("{}", USAGE);
        return 64;
    };
    let mut repair = env::var("NOOP_REPAIR")
        .ok()
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| "touch-missing".into());
    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];
        match arg.as_str() {
            "--repair" => {
                let Some(value) = argv.get(i + 1) else {
                    eprintln!("--repair needs a value");
                    return 64;
                };
                repair = value.clone();
                i += 1;
            }
            _ if arg.starts_with("--repair=") => {
                repair = arg["--repair=".len()..].to_string();
            }
            other => {
                eprintln!("unknown option: {other}");
                println!("{}", USAGE);
                return 64;
            }
        }
        i += 1;
    }

    let paths = retry_env(repo, name);
    if !paths.has_job() {
        eprintln!("no such job: {name}");
        return 64;
    }
    if let Err(error) = agent::resolve(repo, name, false) {
        eprintln!("agent profile: {error}");
        return 64;
    }
    let blocked_path = paths.state.join("blocked");
    let Some(blocked) = state::blocked_marker(&blocked_path) else {
        println!("{} is not blocked; nothing to retry", paths.name);
        return 0;
    };
    if blocked.class != "retryable" {
        println!(
            "refusing to retry {}: class {} is not retryable",
            paths.name, blocked.class
        );
        println!(
            "  the block is the record; read: bin/job diagnose {}",
            paths.name
        );
        println!("  report: {}", paths.state.join("diagnosis.md").display());
        return 2;
    }

    if let Err(code) = run_repair(&paths, &repair) {
        return code;
    }

    run_job(repo, &paths.name, "retry", false, None, None, true)
}

fn run_repair(paths: &Paths, repair: &str) -> Result<(), i32> {
    let repairs_path = paths.repo.join("lib").join("repairs.sh");
    if !repairs_path.is_file() {
        eprintln!("refusing to retry: no repair registry at lib/repairs.sh");
        return Err(2);
    }
    let allowed = registered_repairs(&paths.repo);
    if !allowed.iter().any(|candidate| candidate.as_str() == repair) {
        println!("refusing unknown repair: {repair}");
        println!(
            "  allowlisted: {}",
            if allowed.is_empty() {
                "none".to_string()
            } else {
                allowed.join(" ")
            }
        );
        return Err(2);
    }

    let max = env::var("MAX_REPAIRS")
        .ok()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "2".into());
    if max.is_empty() || !max.chars().all(|c| c.is_ascii_digit()) {
        eprintln!("MAX_REPAIRS must be a whole number");
        return Err(64);
    }
    let max: i64 = max.parse().unwrap_or(2);
    let count_path = paths
        .state
        .join("repairs")
        .join(format!("{}.count", clock::today()));
    let count = fs::read_to_string(&count_path)
        .map(|body| {
            let digits: String = body.chars().filter(|c| c.is_ascii_digit()).collect();
            digits.parse::<i64>().unwrap_or(0)
        })
        .unwrap_or(0);
    if count >= max {
        println!(
            "repair cap reached ({count} of {max} today) for {}",
            paths.name
        );
        println!(
            "  the next repair needs a human: bin/job diagnose {}",
            paths.name
        );
        return Err(2);
    }

    let _ = fs::create_dir_all(paths.state.join("repairs"));
    let _ = fs::create_dir_all(paths.dir.join("logs"));
    let _ = fs::write(&count_path, format!("{}\n", count + 1));

    say(
        &paths.log,
        &format!("retry: repair {repair} ({} of {max} today)", count + 1),
    );
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(r#"set -euo pipefail; . "$REPO/lib/repairs.sh"; "repair_$1""#)
        .arg("lightsout")
        .arg(repair);
    cmd.env("REPO", &paths.repo)
        .env("JOB", &paths.name)
        .env("JOB_DIR", &paths.dir)
        .env("STATE", &paths.state)
        .env("OUT", &paths.out)
        .env("LOG", &paths.log)
        .env("UNIT", &paths.unit)
        .env("U_ALERTS", paths.alerts())
        .env("DRY", "0");
    let repaired = cmd.status().map(|s| s.success()).unwrap_or(false);
    if !repaired {
        say(
            &paths.log,
            &format!("retry: repair {repair} failed; the block stands"),
        );
        return Err(1);
    }
    say(
        &paths.log,
        &format!(
            "retry: repair {repair} done; rerunning {} so the gate decides",
            paths.name
        ),
    );
    Ok(())
}

pub fn run_auto(repo: &Path, name: &str, class: Option<&str>, source: Option<&str>) -> i32 {
    if !repo.join("jobs").join(name).join("job.sh").is_file() {
        eprintln!("no such job: {name}");
        return 64;
    }
    let paths = job_env(repo, name);

    let lock = match heal::Lock::acquire(&paths.state) {
        heal::LockResult::Acquired(lock) => lock,
        heal::LockResult::Held { pid, expiry } => {
            println!(
                "held: auto-repair lock is held by pid {pid} until {}",
                clock::iso(expiry)
            );
            return 0;
        }
        heal::LockResult::Failed(error) => {
            eprintln!("auto-repair: {error}");
            return 2;
        }
    };

    let max_attempts = match heal::max_auto_attempts() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            return 64;
        }
    };
    let policy_file = triage::policy_path(repo);
    let policy = match triage::Policy::load(&policy_file) {
        Ok(policy) => policy,
        Err(error) => {
            eprintln!("{}: {error}", policy_file.display());
            return 64;
        }
    };
    if let Err(error) = policy.check_repairs(repo) {
        eprintln!("{}: {error}", policy_file.display());
        return 64;
    }

    let blocked_path = paths.state.join("blocked");
    if let Some(blocked) = state::blocked_marker(&blocked_path) {
        let day = blocked.iso.get(..10).unwrap_or("");
        if day != clock::today() {
            println!("block dated {day}; use: lightsout heal {name}");
            return 2;
        }
        return heal_loop(repo, &paths, &policy, blocked, max_attempts, &lock);
    }

    let rc = run_job(repo, name, "run", false, class, source, false);
    if rc != 2 {
        return rc;
    }
    let Some(blocked) = state::blocked_marker(&blocked_path) else {
        return 2;
    };
    heal_loop(repo, &paths, &policy, blocked, max_attempts, &lock)
}

fn heal_loop(
    repo: &Path,
    paths: &Paths,
    policy: &triage::Policy,
    mut blocked: state::Blocked,
    max_attempts: usize,
    lock: &heal::Lock,
) -> i32 {
    let mut attempts = 0usize;
    loop {
        if attempts >= max_attempts {
            println!(
                "auto-repair: attempt cap reached ({attempts} of {max_attempts}) for {}",
                paths.name
            );
            return 2;
        }
        let decision = triage::decide(policy, &paths.name, &blocked);
        let before_key = triage::failure_key(&blocked);
        match decision.action {
            triage::Action::Human => {
                println!(
                    "auto-repair: {} is left for a human ({})",
                    paths.name,
                    decision
                        .note
                        .unwrap_or_else(|| "no rule matched".to_string())
                );
                return 2;
            }
            triage::Action::Repair(repair) => {
                let attempt = match heal::begin_attempt(paths) {
                    Ok(attempt) => attempt,
                    Err(error) => {
                        eprintln!("auto-repair: {error}");
                        return 2;
                    }
                };
                lock.refresh();
                let action = format!("repair:{repair}");
                if let Err(code) = run_repair(paths, &repair) {
                    heal::record_attempt(
                        paths,
                        &attempt,
                        &heal::Record {
                            action: &action,
                            before_key: &before_key,
                            after_key: &before_key,
                            verdict: "refused",
                            rc: code,
                            cost: None,
                            agent: None,
                        },
                    );
                    return code;
                }
                attempts += 1;
                let rc = run_job(repo, &paths.name, "run", false, None, None, true);
                let next = state::blocked_marker(&paths.state.join("blocked"));
                let after_key = next
                    .as_ref()
                    .map(triage::failure_key)
                    .unwrap_or_else(|| "done".to_string());
                let verdict = if rc == 2 && after_key == before_key {
                    "no-progress"
                } else if rc == 0 {
                    "passed"
                } else {
                    "repaired"
                };
                heal::record_attempt(
                    paths,
                    &attempt,
                    &heal::Record {
                        action: &action,
                        before_key: &before_key,
                        after_key: &after_key,
                        verdict,
                        rc,
                        cost: None,
                        agent: None,
                    },
                );
                if rc != 2 {
                    return rc;
                }
                if after_key == before_key {
                    println!(
                        "auto-repair: {} failed the same way after the repair; stopping",
                        paths.name
                    );
                    return 2;
                }
                blocked = match next {
                    Some(next) => next,
                    None => return 2,
                };
                continue;
            }
            triage::Action::AgentPropose => {
                return agent_attempt(repo, paths, &blocked, "agent:propose", lock, false);
            }
            triage::Action::AgentApply => {
                let before = triage::failure_key(&blocked);
                let rc = agent_attempt(repo, paths, &blocked, "agent:apply", lock, true);
                if rc != 2 {
                    return rc;
                }
                let next = state::blocked_marker(&paths.state.join("blocked"));
                let after = next
                    .as_ref()
                    .map(triage::failure_key)
                    .unwrap_or_else(|| "done".to_string());
                if after == before {
                    return 2;
                }
                attempts += 1;
                blocked = match next {
                    Some(next) => next,
                    None => return 2,
                };
                continue;
            }
        }
    }
}

/// Record one agent attempt. The action label is `agent:propose` or
/// `agent:apply`; the verdict is what the manager concluded from the runner.
fn record_heal(
    paths: &Paths,
    attempt: &heal::Attempt,
    action: &str,
    before: &str,
    after: &str,
    verdict: &str,
    rc: i32,
) {
    heal::record_attempt(
        paths,
        attempt,
        &heal::Record {
            action,
            before_key: before,
            after_key: after,
            verdict,
            rc,
            cost: None,
            agent: attempt.agent.as_deref(),
        },
    );
}

fn agent_attempt(
    repo: &Path,
    paths: &Paths,
    blocked: &state::Blocked,
    action: &str,
    lock: &heal::Lock,
    apply: bool,
) -> i32 {
    let profile = match agent::resolve(repo, &paths.name, true) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("agent profile: {error}");
            return 64;
        }
    };
    let agent_label = profile.describe();
    let used = triage::heal_count(&paths.state);
    let max = triage::max_heals();
    if used >= max {
        println!(
            "heal cap reached ({used} of {max} today) for {}",
            paths.name
        );
        return 2;
    }
    if let Err(error) = heal::reserve_workspace_attempt(repo, &paths.name, action) {
        eprintln!("heal: {error}");
        return 2;
    }
    let mut attempt = match heal::begin_attempt(paths) {
        Ok(attempt) => attempt,
        Err(error) => {
            eprintln!("heal: {error}");
            return 2;
        }
    };
    attempt.agent = Some(agent_label.clone());
    lock.refresh();
    println!("heal: agent {agent_label}");
    let before = triage::failure_key(blocked);
    let timeout = match heal::timeout_seconds() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            return 64;
        }
    };
    let prompt = match heal::build_prompt(paths, blocked, &attempt) {
        Ok(prompt) => prompt,
        Err(error) => {
            record_heal(
                paths,
                &attempt,
                action,
                &before,
                &before,
                "invalid:prompt",
                2,
            );
            eprintln!("heal: {error}");
            return 2;
        }
    };
    let prompt_path = attempt.dir.join("prompt.md");
    if let Err(error) = fs::write(&prompt_path, &prompt) {
        record_heal(
            paths,
            &attempt,
            action,
            &before,
            &before,
            "invalid:prompt",
            2,
        );
        eprintln!("heal: cannot write {}: {error}", prompt_path.display());
        return 2;
    }

    let before_files = heal::fingerprint(repo, paths);
    let outcome = heal::call_agent(paths, &attempt, &prompt_path, timeout, &profile);
    let after_files = heal::fingerprint(repo, paths);
    if let Some(path) = heal::first_change(&before_files, &after_files) {
        record_heal(paths, &attempt, action, &before, &before, "violation", 2);
        eprintln!(
            "heal: the runner changed '{path}' during the attempt; the proposal is not applied"
        );
        return 2;
    }
    match outcome {
        heal::AgentOutcome::Proposal(value) => {
            let allow: Vec<std::path::PathBuf> = heal::allowed_prefixes(&paths.dir)
                .into_iter()
                .map(std::path::PathBuf::from)
                .collect();
            let edits = match heal::validate(&paths.dir, &allow, &value) {
                Ok(edits) => edits,
                Err(refusal) => {
                    record_heal(
                        paths,
                        &attempt,
                        action,
                        &before,
                        &before,
                        &format!("invalid:{}", refusal.rule),
                        2,
                    );
                    eprintln!("heal: proposal refused ({refusal})");
                    return 2;
                }
            };
            if edits.is_empty() {
                record_heal(paths, &attempt, action, &before, &before, "unfixable", 0);
                let summary = value
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or("the agent declined to propose a fix");
                println!("unfixable: {summary}");
                return 2;
            }
            if apply {
                let applied = match heal::apply(&attempt.dir, &paths.dir, &edits) {
                    Ok(applied) => applied,
                    Err(error) => {
                        record_heal(paths, &attempt, action, &before, &before, "apply-failed", 2);
                        eprintln!("heal: apply failed ({error})");
                        return 2;
                    }
                };
                let rc = run_job(repo, &paths.name, "run", false, None, None, true);
                let next = state::blocked_marker(&paths.state.join("blocked"));
                let after = next
                    .as_ref()
                    .map(triage::failure_key)
                    .unwrap_or_else(|| "done".to_string());
                let verdict = if rc == 0 {
                    "passed"
                } else if rc == 2 && after == before {
                    "no-progress"
                } else if rc == 2 {
                    "blocked"
                } else {
                    "failed"
                };
                record_heal(paths, &attempt, action, &before, &after, verdict, rc);
                println!(
                    "heal: applied {} edit(s); rerun exit {rc}",
                    applied.edited.len()
                );
                if rc == 2 && after == before {
                    println!("heal: the same failure returned; stopping");
                }
                return rc;
            }
            record_heal(paths, &attempt, action, &before, &before, "proposed", 0);
            println!("proposal: {}", attempt.dir.join("proposal.json").display());
            println!("review it, then: lightsout heal {} --apply", paths.name);
            2
        }
        heal::AgentOutcome::NoProposal(reason) => {
            record_heal(
                paths,
                &attempt,
                action,
                &before,
                &before,
                "invalid:no-proposal",
                2,
            );
            eprintln!("heal: {reason}");
            2
        }
        heal::AgentOutcome::Timeout => {
            record_heal(paths, &attempt, action, &before, &before, "timeout", 124);
            println!("heal: the runner timed out; the block stands");
            2
        }
        heal::AgentOutcome::RunnerFailed(code) => {
            record_heal(
                paths,
                &attempt,
                action,
                &before,
                &before,
                "runner-failed",
                code,
            );
            println!("heal: the runner failed with exit {code}; the block stands");
            2
        }
    }
}

fn latest_proposal(paths: &Paths) -> Option<(usize, std::path::PathBuf)> {
    let base = paths.state.join("heal").join(clock::today());
    let entries = fs::read_dir(base).ok()?;
    let mut best: Option<(usize, std::path::PathBuf)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(number) = name.strip_prefix("attempt-") else {
            continue;
        };
        let Ok(number) = number.parse::<usize>() else {
            continue;
        };
        let proposal = entry.path().join("proposal.json");
        if !proposal.is_file() {
            continue;
        }
        if best
            .as_ref()
            .map(|(best, _)| number > *best)
            .unwrap_or(true)
        {
            best = Some((number, entry.path()));
        }
    }
    best
}

fn heal_apply(repo: &Path, name: &str) -> i32 {
    if !repo.join("jobs").join(name).join("job.sh").is_file() {
        eprintln!("no such job: {name}");
        return 64;
    }
    let profile = match agent::resolve(repo, name, false) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("agent profile: {error}");
            return 64;
        }
    };
    let agent_label = profile.describe();
    let paths = job_env(repo, name);
    let Some(blocked) = state::blocked_marker(&paths.state.join("blocked")) else {
        println!("{name} is not blocked; nothing to apply");
        return 0;
    };
    let lock = match heal::Lock::acquire(&paths.state) {
        heal::LockResult::Acquired(lock) => lock,
        heal::LockResult::Held { pid, expiry } => {
            println!(
                "held: auto-repair lock is held by pid {pid} until {}",
                clock::iso(expiry)
            );
            return 0;
        }
        heal::LockResult::Failed(error) => {
            eprintln!("heal: {error}");
            return 2;
        }
    };
    let Some((number, attempt_dir)) = latest_proposal(&paths) else {
        println!("heal: no stored proposal for {name} today");
        return 2;
    };
    // The proposal may have been written under an earlier profile; record the
    // agent that produced it, not whatever the job resolves to today.
    let proposal_agent = state::read_lossy(&attempt_dir.join("meta.env"))
        .lines()
        .find_map(|line| line.strip_prefix("agent="))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| agent_label.clone());
    let body = fs::read_to_string(attempt_dir.join("proposal.json")).unwrap_or_default();
    let proposal: Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("heal: cannot parse attempt-{number}/proposal.json: {error}");
            return 2;
        }
    };
    let allow: Vec<std::path::PathBuf> = heal::allowed_prefixes(&paths.dir)
        .into_iter()
        .map(std::path::PathBuf::from)
        .collect();
    let edits = match heal::validate(&paths.dir, &allow, &proposal) {
        Ok(edits) => edits,
        Err(refusal) => {
            eprintln!("heal: proposal refused ({refusal})");
            return 2;
        }
    };
    if edits.is_empty() {
        println!("heal: attempt-{number} is unfixable; nothing to apply");
        return 2;
    }
    let applied = match heal::apply(&attempt_dir, &paths.dir, &edits) {
        Ok(applied) => applied,
        Err(error) => {
            eprintln!("heal: apply failed ({error})");
            return 2;
        }
    };
    let _ = fs::write(attempt_dir.join("verdict"), "applied\n");
    lock.refresh();

    let before = triage::failure_key(&blocked);
    let rc = run_job(repo, name, "apply", false, None, None, true);
    let next = state::blocked_marker(&paths.state.join("blocked"));
    let after = next
        .as_ref()
        .map(triage::failure_key)
        .unwrap_or_else(|| "done".to_string());
    let verdict = if rc == 0 {
        "passed"
    } else if rc == 2 {
        "blocked"
    } else {
        "failed"
    };
    record_heal(
        &paths,
        &heal::Attempt {
            number,
            dir: attempt_dir.clone(),
            started: clock::now_epoch(),
            agent: Some(proposal_agent.clone()),
        },
        "apply",
        &before,
        &after,
        verdict,
        rc,
    );
    println!(
        "heal: applied {} edit(s) from attempt-{number}; rerun exit {rc}",
        applied.edited.len()
    );
    rc
}

fn heal_revert(repo: &Path, name: &str, number: usize) -> i32 {
    if !repo.join("jobs").join(name).join("job.sh").is_file() {
        eprintln!("no such job: {name}");
        return 64;
    }
    let paths = job_env(repo, name);
    let _lock = match heal::Lock::acquire(&paths.state) {
        heal::LockResult::Acquired(lock) => lock,
        heal::LockResult::Held { pid, expiry } => {
            println!(
                "held: auto-repair lock is held by pid {pid} until {}",
                clock::iso(expiry)
            );
            return 0;
        }
        heal::LockResult::Failed(error) => {
            eprintln!("heal: {error}");
            return 2;
        }
    };
    let attempt_dir = paths
        .state
        .join("heal")
        .join(clock::today())
        .join(format!("attempt-{number}"));
    if !attempt_dir.is_dir() {
        eprintln!("heal: no attempt-{number} today");
        return 2;
    }
    match heal::revert(&attempt_dir, &paths.dir) {
        Ok(restored) => {
            println!("heal: restored {restored} path(s) from attempt-{number}");
            0
        }
        Err(error) => {
            eprintln!("heal: revert failed ({error})");
            2
        }
    }
}

pub fn heal(repo: &Path, argv: &[String]) -> i32 {
    let Some(name) = argv.first() else {
        println!("{}", USAGE);
        return 64;
    };
    let mut apply = false;
    let mut revert: Option<usize> = None;
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--apply" => apply = true,
            "--revert" => {
                let Some(value) = argv.get(i + 1) else {
                    eprintln!("--revert needs an attempt number");
                    return 64;
                };
                match value.parse::<usize>() {
                    Ok(number) if number > 0 => revert = Some(number),
                    _ => {
                        eprintln!("--revert needs a positive attempt number");
                        return 64;
                    }
                }
                i += 1;
            }
            other if other.starts_with("--revert=") => {
                match other["--revert=".len()..].parse::<usize>() {
                    Ok(number) if number > 0 => revert = Some(number),
                    _ => {
                        eprintln!("--revert needs a positive attempt number");
                        return 64;
                    }
                }
            }
            other => {
                eprintln!("heal: unknown option: {other}");
                return 64;
            }
        }
        i += 1;
    }
    if apply && revert.is_some() {
        eprintln!("heal: choose --apply or --revert, not both");
        return 64;
    }
    if let Some(number) = revert {
        return heal_revert(repo, name, number);
    }
    if apply {
        return heal_apply(repo, name);
    }
    let paths = job_env(repo, name);
    if !paths.has_job() {
        eprintln!("no such job: {name}");
        return 64;
    }
    let Some(blocked) = state::blocked_marker(&paths.state.join("blocked")) else {
        println!("{name} is not blocked; nothing to heal");
        return 0;
    };
    let policy_file = triage::policy_path(repo);
    let policy = match triage::Policy::load(&policy_file) {
        Ok(policy) => policy,
        Err(error) => {
            eprintln!("{}: {error}", policy_file.display());
            return 64;
        }
    };
    if let Err(error) = policy.check_repairs(repo) {
        eprintln!("{}: {error}", policy_file.display());
        return 64;
    }
    let decision = triage::decide(&policy, name, &blocked);
    match decision.action {
        triage::Action::Human => {
            println!(
                "heal: the policy leaves {} for a human ({})",
                paths.name,
                decision
                    .note
                    .unwrap_or_else(|| "no rule matched".to_string())
            );
            2
        }
        triage::Action::Repair(repair) => {
            println!(
                "heal: the policy routes {} to repair:{repair}; use: lightsout retry {}",
                paths.name, paths.name
            );
            2
        }
        triage::Action::AgentApply => {
            let lock = match heal::Lock::acquire(&paths.state) {
                heal::LockResult::Acquired(lock) => lock,
                heal::LockResult::Held { pid, expiry } => {
                    println!(
                        "held: auto-repair lock is held by pid {pid} until {}",
                        clock::iso(expiry)
                    );
                    return 0;
                }
                heal::LockResult::Failed(error) => {
                    eprintln!("heal: {error}");
                    return 2;
                }
            };
            agent_attempt(repo, &paths, &blocked, "agent:apply", &lock, true)
        }
        triage::Action::AgentPropose => {
            let lock = match heal::Lock::acquire(&paths.state) {
                heal::LockResult::Acquired(lock) => lock,
                heal::LockResult::Held { pid, expiry } => {
                    println!(
                        "held: auto-repair lock is held by pid {pid} until {}",
                        clock::iso(expiry)
                    );
                    return 0;
                }
                heal::LockResult::Failed(error) => {
                    eprintln!("heal: {error}");
                    return 2;
                }
            };
            agent_attempt(repo, &paths, &blocked, "agent:propose", &lock, false)
        }
    }
}

pub fn unblock(repo: &Path, name: &str) -> i32 {
    if !repo.join("jobs").join(name).join("job.sh").is_file() {
        eprintln!("no such job: {name}");
        return 64;
    }
    let paths = job_env(repo, name);
    let blocked = paths.state.join("blocked");
    if blocked.is_file() {
        let raw = state::read_lossy(&blocked);
        let raw = raw.trim_end_matches(['\n', '\r']);
        let _ = state::append(
            &paths.alerts(),
            &format!("{}\tunblocked\t{}\n", clock::iso_now(), raw),
        );
        let _ = fs::remove_file(&blocked);
        println!("unblocked {}", paths.name);
    } else {
        println!("{} was not blocked", paths.name);
    }
    0
}
