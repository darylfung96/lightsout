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
    pub last_exit: Option<i64>,
    pub last_run: Option<String>,
    pub last_duration: Option<i64>,
    pub last_work_duration: Option<i64>,
    pub runs: Vec<state::Run>,
    pub out_files: i64,
    pub expect: Option<String>,
    pub stale: bool,
}

fn job_state(repo: &Path, name: &str) -> JobState {
    let paths = repo::Paths::new(repo, name);
    let dir = paths.dir.clone();
    let st = paths.state.clone();
    let out = paths.out.clone();
    let today = clock::today();
    let log = paths.log.clone();

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
        last_heal: state::lines(&st.join("heal").join("history.tsv"))
            .last()
            .cloned(),
        spend_today: state::spend_today(&st, &today),
        last_exit: None,
        last_run: None,
        last_duration: None,
        last_work_duration: None,
        runs: Vec::new(),
        out_files: 0,
        expect: None,
        stale: false,
    };

    if let Some((when, rc)) = state::last_run_row(&st) {
        rec.last_run = Some(when);
        rec.last_exit = rc;
    }

    rec.runs = state::run_history(&st, 10);
    if let Some(last) = rec.runs.last() {
        rec.last_duration = Some(last.seconds);
        // A damaged `state/last` should not hide the run the history records.
        if rec.last_run.is_none() {
            rec.last_run = Some(last.end.clone());
        }
    }
    rec.last_work_duration = state::last_work_run(&st, &rec.runs).map(|run| run.seconds);

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
        let now = clock::now_hm();
        let now_minutes = now
            .split_once(':')
            .and_then(|(h, m)| Some(h.parse::<i64>().ok()? * 60 + m.parse::<i64>().ok()?))
            .unwrap_or(0);
        rec.stale = now_minutes >= minutes
            && !done_today
            && !locked
            && !blocked_now
            && !state::held_today(&st, &today);
    }

    rec
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
        "last_exit": rec.last_exit,
        "last_run": rec.last_run,
        "last_duration": rec.last_duration,
        "last_work_duration": rec.last_work_duration,
        "runs": runs,
        "out_files": rec.out_files,
        "expect": rec.expect,
        "stale": rec.stale,
    })
}

fn html_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn runs_html(history: &[state::Run]) -> String {
    let start = history.len().saturating_sub(5);
    let recent = &history[start..];
    if recent.is_empty() {
        return "—".into();
    }
    let mut spans = Vec::new();
    for run in recent.iter().rev() {
        let stamp = slice_chars(&run.end, 5, 16).replace('T', " ");
        spans.push(format!(
            "<span class='run'>{} {} rc {} {}</span>",
            html_escape(&stamp),
            html_escape(&run.verb),
            run.rc,
            state::duration_text(Some(run.seconds))
        ));
    }
    spans.join("<br>")
}

fn slice_chars(value: &str, start: usize, end: usize) -> String {
    value
        .chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

/// The row's CSS class: "ran 2026-10-21" must not become a selector with a
/// date in it, because the date changes every day.
fn state_class(state: &str) -> &'static str {
    match state {
        "blocked" => "blocked",
        "running" => "running",
        "failed" => "failed",
        "done today" => "done-today",
        "never run" => "never-run",
        _ if state.starts_with("ran ") => "ran",
        _ => "never-run",
    }
}

/// The newest run's duration, with the newest run that actually did work in
/// parentheses when the last tick was a hold. A hold exits immediately, so its
/// `0s` alone would read as a job that never got going.
fn duration_cell(last: Option<i64>, work: Option<i64>) -> String {
    let text = state::duration_text(last);
    match work {
        Some(work) if Some(work) != last => format!(
            "{} <span class='work'>(work {})</span>",
            text,
            state::duration_text(Some(work))
        ),
        _ => text,
    }
}

fn html_table(data: &[JobState]) -> String {
    let mut rows = Vec::new();
    for rec in data {
        let class = state_class(&rec.state);
        let stage = if rec.stage.is_empty() {
            "—".to_string()
        } else {
            html_escape(&rec.stage)
        };
        rows.push(format!(
            "<tr class='{class}'><td><b>{}</b></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{} / {}</td><td>{}</td></tr>",
            html_escape(&rec.name),
            html_escape(&rec.state),
            html_escape(&rec.detail),
            stage,
            html_escape(rec.last_run.as_deref().unwrap_or("—")),
            duration_cell(rec.last_duration, rec.last_work_duration),
            runs_html(&rec.runs),
            rec.attempts_today,
            rec.attempts_total,
            rec.heal_attempts_today
        ));
    }
    if rows.is_empty() {
        "<tr><td colspan=9>no jobs yet</td></tr>".into()
    } else {
        rows.join("\n")
    }
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
        let page = format!(
            "<!doctype html><meta charset=utf-8>\n\
             <title>LightsOut</title>\n\
             <style>\n\
             body{{font:15px/1.5 -apple-system,system-ui,sans-serif;margin:2rem;color:#1b1b1b}}\n\
             table{{border-collapse:collapse;width:100%}}\n\
             th,td{{text-align:left;padding:.5rem .75rem;border-bottom:1px solid #e5e5e5;vertical-align:top}}\n\
             th{{font-size:.8rem;text-transform:uppercase;letter-spacing:.05em;color:#666}}\n\
             tr.blocked{{background:#fdeceb}} tr.blocked td:nth-child(2){{color:#b42318;font-weight:600}}\n\
             tr.running{{background:#fff8e6}} tr.failed{{background:#fff3cd}} tr.failed td:nth-child(2){{color:#b54708}}\n\
             tr.done-today td:nth-child(2){{color:#087443}}\n\
             tr.ran td:nth-child(2){{color:#555}}\n\
             span.run, span.work{{font-size:.85rem;color:#555}}\n\
             caption{{text-align:left;font-weight:600;margin-bottom:.75rem}}\n\
             </style><table><caption>LightsOut — {}</caption>\n\
             <tr><th>job</th><th>state</th><th>why</th><th>stage</th><th>last run</th><th>duration</th><th>last 5 runs</th><th>attempts today / total</th><th>heals</th></tr>\n\
             {}</table>",
            clock::format_epoch(clock::now_epoch(), "%Y-%m-%d %H:%M"),
            html_table(&data)
        );
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
