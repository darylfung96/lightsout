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

use crate::clock;
use crate::repo::Paths;
use crate::state;

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
  run <name> [--class C] [--source S]
                       do the work, resuming from disk; C defaults every block
                       raised in this run (retryable|judgment|human)
  retry <name> [--repair R]
                       run the allowlisted repair R (default touch-missing)
                       for a retryable block, then the job again so the same
                       gate decides; capped by MAX_REPAIRS (default 2)
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
    apply_job_env(&mut cmd, &paths, dry, class, source, retrying);
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
    run_job(repo, name, "plan", true, None, None, false)
}

pub fn run(repo: &Path, name: &str, verb: &str, class: Option<&str>, source: Option<&str>) -> i32 {
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

    let repairs_path = paths.repo.join("lib").join("repairs.sh");
    if !repairs_path.is_file() {
        eprintln!("refusing to retry: no repair registry at lib/repairs.sh");
        return 2;
    }
    let allowed = registered_repairs(repo);
    if !allowed.iter().any(|candidate| candidate == &repair) {
        println!("refusing unknown repair: {repair}");
        println!(
            "  allowlisted: {}",
            if allowed.is_empty() {
                "none".to_string()
            } else {
                allowed.join(" ")
            }
        );
        return 2;
    }

    let max = env::var("MAX_REPAIRS")
        .ok()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "2".into());
    if max.is_empty() || !max.chars().all(|c| c.is_ascii_digit()) {
        eprintln!("MAX_REPAIRS must be a whole number");
        return 64;
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
        return 2;
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
        .arg(&repair);
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
        return 1;
    }
    say(
        &paths.log,
        &format!(
            "retry: repair {repair} done; rerunning {} so the gate decides",
            paths.name
        ),
    );

    run_job(repo, &paths.name, "retry", false, None, None, true)
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
