//! `status` and `report` — rebuild every job's picture from its state files.
//!
//! Read-only: no command in this module writes job state. The only file it
//! writes is the HTML page named by `--html`, exactly like the Python report did.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::agent;
use crate::clock;
use crate::repo;
use crate::schedule;
use crate::spec;
use crate::state;

#[derive(Clone, Debug)]
pub struct MemoryPass {
    pub age: i64,
    pub files: usize,
}

#[derive(Clone, Debug)]
pub struct HealLast {
    pub action: String,
    pub verdict: String,
    pub age: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct JobState {
    pub name: String,
    pub has_job: bool,
    pub has_spec: bool,
    pub state: String,
    pub detail: String,
    pub class: Option<String>,
    pub source: Option<String>,
    pub since: Option<String>,
    pub stage: String,
    pub attempts_today: i64,
    pub attempts_total: i64,
    pub heal_attempts_today: usize,
    pub last_heal: Option<String>,
    pub spend_today: i64,
    pub spend_total: i64,
    pub last_exit: Option<i64>,
    pub last_run: Option<String>,
    pub last_run_epoch: Option<i64>,
    pub last_work_epoch: Option<i64>,
    pub last_duration: Option<i64>,
    pub last_work_duration: Option<i64>,
    pub runs: Vec<state::Run>,
    pub out_files: i64,
    pub expect: Option<String>,
    pub expect_minutes: Option<i64>,
    pub overdue_minutes: Option<i64>,
    pub stale: bool,
    pub held: bool,
    pub summary: String,
    pub cadence: Option<String>,
    pub stages_total: usize,
    pub agent: Option<String>,
    pub agent_configured: bool,
    pub agent_error: Option<String>,
    pub deliverable: Option<String>,
    pub deliverable_ok: Option<bool>,
    pub memory: Option<MemoryPass>,
    pub heal_last: Option<HealLast>,
    pub registered: bool,
    pub job_dir: String,
}

fn job_state(repo: &Path, name: &str) -> JobState {
    let paths = repo::Paths::new(repo, name);
    let dir = paths.dir.clone();
    let st = paths.state.clone();
    let out = paths.out.clone();
    let today = clock::today();
    let log = paths.log.clone();

    let spec = spec::read(&dir.join("spec.md"));
    let artifact = state::expect_artifact(&st);
    let held = state::held_today(&st, &today);
    let last_heal_line = state::lines(&st.join("heal").join("history.tsv"))
        .into_iter()
        .rfind(|line| !line.trim().is_empty());
    let heal_last = last_heal_line.as_deref().map(parse_heal_line);

    let mut rec = JobState {
        name: name.to_string(),
        has_job: paths.has_job(),
        has_spec: dir.join("spec.md").is_file(),
        state: "never run".into(),
        detail: String::new(),
        class: None,
        source: None,
        since: None,
        stage: String::new(),
        attempts_today: state::attempts_today(&st, &today),
        attempts_total: state::attempts_total(&st),
        heal_attempts_today: crate::triage::heal_count(&st),
        last_heal: last_heal_line,
        spend_today: state::spend_today(&st, &today),
        spend_total: state::spend_total(&st),
        last_exit: None,
        last_run: None,
        last_run_epoch: None,
        last_work_epoch: None,
        last_duration: None,
        last_work_duration: None,
        runs: Vec::new(),
        out_files: 0,
        expect: None,
        expect_minutes: None,
        overdue_minutes: None,
        stale: false,
        held,
        summary: spec
            .as_ref()
            .map(|spec| spec.summary.clone())
            .unwrap_or_default(),
        cadence: spec
            .as_ref()
            .and_then(|spec| spec.field("cadence"))
            .map(str::to_string),
        stages_total: spec.as_ref().map(|spec| spec.stages.len()).unwrap_or(0),
        agent: None,
        agent_configured: false,
        agent_error: None,
        deliverable: artifact.as_ref().map(|path| path.display().to_string()),
        deliverable_ok: artifact
            .as_ref()
            .map(|path| crate::commands::artifact_today(path, &dir, &today)),
        memory: memory_pass(&st),
        heal_last,
        registered: paths.workdir.is_some(),
        job_dir: dir.display().to_string(),
    };

    match agent::resolve_config(repo, name) {
        Ok(profile) if profile.configured => {
            rec.agent = Some(profile.describe());
            rec.agent_configured = true;
        }
        Ok(_) => {}
        Err(error) => rec.agent_error = Some(error.to_string()),
    }

    if let Some((when, rc)) = state::last_run_row(&st) {
        rec.last_run = Some(when);
        rec.last_exit = rc;
    }
    // A hold is the latest run doing no work, not any zero exit today: a job
    // that held at 05:00 and failed at 07:32 is late, not held.
    rec.held = held && rec.last_exit == Some(0);

    rec.runs = state::run_history(&st, 10);
    if let Some(last) = rec.runs.last() {
        rec.last_duration = Some(last.seconds);
        // A damaged `state/last` should not hide the run the history records.
        if rec.last_run.is_none() {
            rec.last_run = Some(last.end.clone());
        }
    }
    rec.last_run_epoch = rec.last_run.as_deref().and_then(clock::epoch_of);
    let work = state::last_work_run(&st, &rec.runs);
    rec.last_work_duration = work.map(|run| run.seconds);
    rec.last_work_epoch = work.and_then(|run| clock::epoch_of(&run.end));

    let done_today = st.join("done").join(&today).exists();
    if done_today {
        rec.state = "done today".into();
        rec.since = state::mtime_iso(&st.join("done").join(&today));
    }

    let blocked_now = st.join("blocked").is_file();
    if blocked_now {
        if let Some(blocked) = state::blocked_marker(&st.join("blocked")) {
            rec.since = Some(blocked.iso);
            rec.class = Some(blocked.class);
            rec.source = Some(blocked.source);
            rec.detail = blocked.reason;
            rec.state = "blocked".into();
        }
    }

    let locked = state::lock_live(&st);
    if locked {
        rec.state = "running".into();
    }

    if log.is_file() {
        if let Some((when, number, what)) = last_stage_line(&state::read_lossy(&log)) {
            rec.stage = format!("stage {number}: {what}");
            if rec.last_run.is_none() {
                rec.last_run = Some(when);
            }
        }
    }

    // A run recorded by the history is not a job that never ran: `done/<unit>`
    // belongs to the unit, so at the next midnight no marker explains the row
    // while its `last run` still does. A non-zero exit is a failure; a zero
    // exit is a finished run whose date is the one the history knows.
    if rec.state == "never run" {
        match rec.last_exit {
            Some(rc) if rc != 0 => rec.state = "failed".into(),
            _ => {
                if let Some(when) = rec.last_run.clone() {
                    rec.since = Some(when.clone());
                    rec.state = format!("ran {}", slice_chars(&when, 0, 10));
                }
            }
        }
    }

    rec.out_files = state::out_files(&out);

    if let Some((minutes, label)) = state::expect_time(&st) {
        rec.expect = Some(label);
        rec.expect_minutes = Some(minutes);
        let now_minutes = clock::now_minutes();
        rec.stale = now_minutes >= minutes && !done_today && !locked && !blocked_now && !rec.held;
        if rec.stale {
            rec.overdue_minutes = Some(now_minutes - minutes);
        }
    }

    rec
}

/// `state/memory-pass.json` (the receipt a job writes at the end of its pass):
/// age and the number of pages it touched. A missing or damaged receipt is not
/// an error here — the page says "no receipt" instead.
fn memory_pass(state: &Path) -> Option<MemoryPass> {
    let body = state::read_lossy(&state.join("memory-pass.json"));
    let value: Value = serde_json::from_str(body.trim()).ok()?;
    let recorded = value.get("recorded_at")?.as_i64()?;
    let files = value
        .get("files")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    Some(MemoryPass {
        age: clock::now_epoch() - recorded,
        files,
    })
}

/// One line of `state/heal/history.tsv`: `<iso> <job> <attempt> <action>
/// <before> <after> <verdict> <rc> <seconds> <cost> <agent>`.
fn parse_heal_line(line: &str) -> HealLast {
    let fields: Vec<&str> = line.split('\t').collect();
    HealLast {
        action: fields.get(3).unwrap_or(&"?").to_string(),
        verdict: fields.get(6).unwrap_or(&"?").to_string(),
        age: fields
            .first()
            .and_then(|iso| clock::epoch_of(iso))
            .map(|at| clock::now_epoch() - at),
    }
}

/// The newest `[when] stage N: what` line in today's log.
fn last_stage_line(body: &str) -> Option<(String, String, String)> {
    let mut found = None;
    for line in body.lines() {
        let Some(close) = line.find("] stage ") else {
            continue;
        };
        if !line.starts_with('[') {
            continue;
        }
        let when = &line[1..close];
        let rest = &line[close + "] stage ".len()..];
        let Some(colon) = rest.find(": ") else {
            continue;
        };
        let number = &rest[..colon];
        if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        found = Some((
            when.to_string(),
            number.to_string(),
            rest[colon + 2..].to_string(),
        ));
    }
    found
}

fn jobs(repo: &Path) -> Vec<JobState> {
    repo::all_job_names(repo)
        .iter()
        .map(|name| job_state(repo, name))
        .collect()
}

fn order(state: &str) -> u8 {
    match state {
        "blocked" => 0,
        "running" => 1,
        "failed" => 2,
        "done today" => 3,
        "never run" => 5,
        _ if state.starts_with("ran ") => 4,
        _ => 9,
    }
}

fn to_json(rec: &JobState) -> Value {
    let runs: Vec<Value> = rec
        .runs
        .iter()
        .map(|run| {
            json!({
                "start": run.start,
                "end": run.end,
                "rc": run.rc,
                "verb": run.verb,
                "unit": run.unit,
                "seconds": run.seconds,
            })
        })
        .collect();
    json!({
        "name": rec.name,
        "has_job": rec.has_job,
        "has_spec": rec.has_spec,
        "state": rec.state,
        "detail": rec.detail,
        "class": rec.class,
        "source": rec.source,
        "since": rec.since,
        "stage": rec.stage,
        "attempts_today": rec.attempts_today,
        "attempts_total": rec.attempts_total,
        "heal_attempts_today": rec.heal_attempts_today,
        "last_heal": rec.last_heal,
        "spend_today": rec.spend_today,
        "spend_total": rec.spend_total,
        "last_exit": rec.last_exit,
        "last_run": rec.last_run,
        "last_duration": rec.last_duration,
        "last_work_duration": rec.last_work_duration,
        "runs": runs,
        "out_files": rec.out_files,
        "expect": rec.expect,
        "expect_minutes": rec.expect_minutes,
        "overdue_minutes": rec.overdue_minutes,
        "stale": rec.stale,
        "held": rec.held,
        "summary": rec.summary,
        "cadence": rec.cadence,
        "stages_total": rec.stages_total,
        "agent": rec.agent,
        "agent_configured": rec.agent_configured,
        "agent_error": rec.agent_error,
        "deliverable": rec.deliverable,
        "deliverable_ok": rec.deliverable_ok,
        "memory_pass": rec.memory.as_ref().map(|pass| json!({
            "age_seconds": pass.age,
            "files": pass.files,
        })),
        "heal_last": rec.heal_last.as_ref().map(|heal| json!({
            "action": heal.action,
            "verdict": heal.verdict,
            "age_seconds": heal.age,
        })),
        "registered": rec.registered,
        "job_dir": rec.job_dir,
    })
}

fn slice_chars(value: &str, start: usize, end: usize) -> String {
    value
        .chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

/// Greedy wrap at `width` characters; one long word is left long rather than
/// broken, because paths and commands must stay copy-pasteable.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let long = !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width;
        if long {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

const DETAIL_LABEL: usize = 13;
const DETAIL_WIDTH: usize = 76;

fn detail_row(label: &str, text: &str) {
    let pad = format!("  {label:<DETAIL_LABEL$} ");
    let indent = " ".repeat(pad.chars().count());
    for (index, line) in wrap(text, DETAIL_WIDTH - indent.len()).iter().enumerate() {
        if index == 0 {
            println!("{pad}{line}");
        } else {
            println!("{indent}{line}");
        }
    }
}

/// `status <name>` — what the job is, when it is due and what a run does, from
/// files only. Running `plan` here would execute `job.sh`, so the block names
/// that command instead of doing it.
fn print_detail(root: &Path, rec: &JobState) {
    println!("{} — {}", rec.name, rec.state);
    let paths = repo::Paths::new(root, &rec.name);
    let spec = spec::read(&paths.dir.join("spec.md"));

    if !rec.detail.is_empty() {
        let class = rec.class.as_deref().unwrap_or("human");
        let source = rec.source.as_deref().unwrap_or("unknown");
        detail_row("why", &format!("[{class}] {source}: {}", rec.detail));
        if let Some(since) = &rec.since {
            detail_row("since", since);
        }
    }

    match &spec {
        Some(spec) if !spec.summary.is_empty() => detail_row("description", &spec.summary),
        Some(_) => detail_row("description", "spec.md has no summary"),
        None => detail_row("description", "no spec.md in the job directory"),
    }
    if let Some(spec) = &spec {
        if let Some(cadence) = spec.field("cadence") {
            detail_row("cadence", cadence);
        }
    }
    let unit = match spec.as_ref().and_then(|spec| spec.field("unit")) {
        Some(unit) => unit.to_string(),
        None => format!("{} (the unit the next run uses)", paths.unit),
    };
    detail_row("unit", &unit);

    match state::expect_time(&paths.state) {
        Some((minutes, label)) => detail_row(
            "schedule",
            &format!(
                "{label} (state/expect) — cron {}",
                schedule::cron_time((minutes / 60) as u32, (minutes % 60) as u32)
            ),
        ),
        None => detail_row(
            "schedule",
            "no time recorded: write HH:MM to state/expect \
             (lightsout schedule <name> --at HH:MM prints the trigger)",
        ),
    }

    match agent::resolve_config(root, &rec.name) {
        Ok(profile) if profile.configured => detail_row("agent", &profile.describe()),
        Ok(_) => detail_row("agent", "no agent.conf: the runner's own default"),
        Err(error) => detail_row("agent", &format!("profile error: {error}")),
    }

    if let Some(spec) = &spec {
        if !spec.stages.is_empty() {
            println!("  what it does");
            for stage in &spec.stages {
                for (index, line) in wrap(stage, DETAIL_WIDTH - 4).iter().enumerate() {
                    if index == 0 {
                        println!("    {line}");
                    } else {
                        println!("      {line}");
                    }
                }
            }
        }
        if let Some(guards) = spec.field("guards") {
            detail_row("guards", guards);
        }
    }

    if !rec.stage.is_empty() {
        detail_row("log stage", &rec.stage);
    }
    if rec.runs.is_empty() {
        detail_row("last runs", "none recorded");
    } else {
        println!("  last runs");
        for run in rec.runs.iter().rev().take(5) {
            let when = slice_chars(&run.end, 0, 19).replace('T', " ");
            println!(
                "    {when}  {:<6} rc {}  {}  unit {}",
                run.verb,
                run.rc,
                state::duration_text(Some(run.seconds)),
                run.unit
            );
        }
    }
    detail_row(
        "attempts",
        &format!(
            "{} today / {} total",
            rec.attempts_today, rec.attempts_total
        ),
    );
    if rec.heal_attempts_today > 0 {
        detail_row("heals", &format!("{} today", rec.heal_attempts_today));
    }
    detail_row("outputs", &format!("out/ has {} file(s)", rec.out_files));

    let mut files = Vec::new();
    if paths.has_job() {
        files.push("job.sh");
    }
    if paths.dir.join("spec.md").is_file() {
        files.push("spec.md");
    }
    if paths.dir.join("agent.conf").is_file() {
        files.push("agent.conf");
    }
    let files = if files.is_empty() {
        "none".to_string()
    } else {
        files.join(", ")
    };
    detail_row("files", &files);

    detail_row(
        "plan",
        &format!(
            "lightsout plan {} — what would run now; touches nothing",
            rec.name
        ),
    );
    detail_row(
        "run",
        &format!("lightsout run {} — take an attempt", rec.name),
    );
    if rec.state == "blocked" {
        detail_row(
            "diagnose",
            &format!("lightsout diagnose {} — read the stop", rec.name),
        );
    }
}

#[derive(Default)]
struct Args {
    json: bool,
    html: Option<String>,
    open: bool,
    name: Option<String>,
    all: bool,
    detail: bool,
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut args = Args::default();
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        match arg.as_str() {
            "--json" => args.json = true,
            "--all" => args.all = true,
            "--open" => args.open = true,
            "--html" => {
                let Some(value) = argv.get(i + 1) else {
                    return Err("--html needs a value".into());
                };
                args.html = Some(value.clone());
                i += 1;
            }
            "--detail" | "-d" => args.detail = true,
            _ if arg.starts_with("--html=") => {
                args.html = Some(arg["--html=".len()..].to_string());
            }
            _ if arg.starts_with('-') => return Err(format!("unknown option: {arg}")),
            _ => {
                if args.name.is_some() {
                    return Err(format!("unexpected argument: {arg}"));
                }
                args.name = Some(arg.clone());
            }
        }
        i += 1;
    }
    Ok(args)
}

/// `--open` is a convenience, not a gate: the page is already on disk, so a
/// missing `open`/`xdg-open` warns and still exits 0.
fn open_page(path: &Path) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    match std::process::Command::new(program).arg(path).status() {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!("report: {program} exited {status}"),
        Err(error) => eprintln!("report: cannot run {program}: {error}"),
    }
}

pub fn run(argv: &[String]) -> i32 {
    let repo = repo::root();
    let args = match parse_args(argv) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("report: {message}");
            return 64;
        }
    };
    if let Some(name) = &args.name {
        if !repo::Paths::new(&repo, name).has_job() {
            eprintln!("no such job: {name}");
            return 64;
        }
    }
    let mut data: Vec<JobState> = jobs(&repo)
        .into_iter()
        .filter(|rec| args.name.as_deref().is_none_or(|name| name == rec.name))
        .collect();
    data.sort_by(|a, b| (order(&a.state), a.name.clone()).cmp(&(order(&b.state), b.name.clone())));

    if args.json {
        let values: Vec<Value> = data.iter().map(to_json).collect();
        match serde_json::to_string_pretty(&values) {
            Ok(body) => println!("{body}"),
            Err(error) => {
                eprintln!("status: {error}");
                return 1;
            }
        }
        return 0;
    }

    if let Some(path) = args.html {
        let page = crate::report::render(&data, &repo);
        let out = PathBuf::from(&path);
        if let Some(parent) = out.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(error) = fs::create_dir_all(parent) {
                    eprintln!("report: cannot create {}: {error}", parent.display());
                    return 1;
                }
            }
        }
        if let Err(error) = fs::write(&out, page) {
            eprintln!("report: cannot write {path}: {error}");
            return 1;
        }
        println!("{path}");
        if args.open {
            open_page(&out);
        }
        return 0;
    }

    if args.detail || args.name.is_some() {
        for (index, rec) in data.iter().enumerate() {
            if index > 0 {
                println!();
            }
            print_detail(&repo, rec);
        }
        return 0;
    }

    for rec in &data {
        let detail: String = rec.detail.chars().take(70).collect();
        println!("{:<14} {:<14} {}", rec.state, rec.name, detail);
    }
    0
}
