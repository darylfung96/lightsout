//! The dashboard page: one self-contained HTML file, no server, no assets.
//!
//! The page is a view over the same `JobState` the table and `--json` print, so
//! it cannot disagree with `status`: it adds hierarchy (what needs a person),
//! not facts. Everything dynamic is escaped; the CSS and script are inline so
//! the file opens from disk with no network.

use std::path::Path;

use crate::clock;
use crate::repo;
use crate::state;
use crate::status::JobState;

fn esc(value: &str) -> String {
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

/// The row's CSS class. A date never becomes a selector: "ran 2026-10-21"
/// changes every day, so every past run is simply `ran`.
fn row_class(st: &str) -> &'static str {
    match st {
        "blocked" => "blocked",
        "running" => "running",
        "failed" => "failed",
        "done today" => "done-today",
        "never run" => "never-run",
        _ if st.starts_with("ran ") => "ran",
        _ => "never-run",
    }
}

fn pill_class(st: &str) -> &'static str {
    match st {
        "blocked" => "p-blocked",
        "running" => "p-running",
        "failed" => "p-failed",
        "done today" => "p-done",
        "never run" => "p-never",
        _ if st.starts_with("ran ") => "p-ran",
        _ => "p-never",
    }
}

/// `7s`, `12m`, `1h 20m`, `2d 3h` — short enough for a table cell.
fn elapsed(seconds: i64) -> String {
    let seconds = seconds.max(0);
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 24 {
        let rest = minutes % 60;
        return if rest == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {rest:02}m")
        };
    }
    let days = hours / 24;
    let rest = hours % 24;
    if rest == 0 {
        format!("{days}d")
    } else {
        format!("{days}d {rest}h")
    }
}

fn ago(seconds: i64) -> String {
    if seconds < 60 {
        "just now".into()
    } else {
        format!("{} ago", elapsed(seconds))
    }
}

fn money(cents: i64) -> String {
    format!("${}.{:02}", cents / 100, (cents % 100).abs())
}

fn hhmm(iso: &str) -> String {
    let text: String = iso.chars().skip(11).take(5).collect();
    if text.is_empty() {
        iso.to_string()
    } else {
        text
    }
}

/// `08:12`, or `11-02 08:12` when the run was not today.
fn day_hhmm(iso: &str, today: &str) -> String {
    if iso.starts_with(today) {
        hhmm(iso)
    } else {
        let text: String = iso.chars().skip(5).take(11).collect();
        text.replace('T', " ")
    }
}

/// `stage 2: apply the plan` → `(2, "apply the plan")`.
fn stage_parts(stage: &str) -> Option<(String, String)> {
    let rest = stage.strip_prefix("stage ")?;
    let (number, what) = rest.split_once(": ")?;
    Some((number.to_string(), what.to_string()))
}

fn tag(label: &str, tone: &str, title: &str) -> String {
    format!(
        "<span class=\"tag {tone}\" title=\"{}\">{}</span>",
        esc(title),
        esc(label)
    )
}

/// A receipt is stale when it predates the last work run: a hold writes none,
/// and a run after the receipt skipped the pass.
fn memory_is_stale(pass_age: i64, work_age: i64) -> bool {
    pass_age > work_age + 60
}

/// Only problems earn a tag; a healthy job stays a name and a summary.
fn flags(rec: &JobState) -> String {
    let mut tags = Vec::new();
    if !rec.has_spec {
        tags.push(tag("no spec", "warn", "no spec.md describes this job"));
    }
    if let Some(error) = &rec.agent_error {
        tags.push(tag("agent !", "warn", error));
    }
    if let (Some(pass), Some(last)) = (&rec.memory, rec.last_work_epoch) {
        let work_age = (clock::now_epoch() - last).max(0);
        if memory_is_stale(pass.age, work_age) {
            tags.push(tag(
                &format!("memory {}", elapsed(pass.age)),
                "warn",
                "the memory pass receipt is older than the last work run",
            ));
        }
    }
    if tags.is_empty() {
        String::new()
    } else {
        format!("<span class=\"flags\">{}</span>", tags.join(""))
    }
}

fn duration_html(rec: &JobState) -> String {
    let text = state::duration_text(rec.last_duration);
    match rec.last_work_duration {
        Some(work) if Some(work) != rec.last_duration => format!(
            "{text} <span class=\"work\">(work {})</span>",
            state::duration_text(Some(work))
        ),
        _ => text,
    }
}

/// The last ten runs, oldest first, one square each: green exit 0, amber any
/// other non-2 code, red blocked. The title carries the exact run.
fn dots(rec: &JobState) -> String {
    if rec.runs.is_empty() {
        return "<span class=\"muted\">no runs</span>".into();
    }
    let mut out = String::new();
    for run in &rec.runs {
        let tone = match run.rc {
            0 => "r0",
            2 => "r2",
            _ => "r1",
        };
        out.push_str(&format!(
            "<span class=\"rd {tone}\" title=\"{} {} · rc {} · {}\"></span>",
            esc(&day_hhmm(&run.end, &clock::today())),
            esc(&run.verb),
            run.rc,
            state::duration_text(Some(run.seconds))
        ));
    }
    out
}

fn stage_html(rec: &JobState) -> String {
    match stage_parts(&rec.stage) {
        Some((number, what)) => {
            let total = if rec.stages_total > 0 {
                format!("/{}", rec.stages_total)
            } else {
                String::new()
            };
            format!(
                "<b>{number}</b>{total} <span class=\"muted\">{}</span>",
                esc(&what)
            )
        }
        None => "—".into(),
    }
}

/// The due time plus why it matters now: met, paused, held, running or late.
fn due_html(rec: &JobState, now: i64) -> String {
    let Some(minutes) = rec.expect_minutes else {
        return "—".into();
    };
    let time = esc(rec.expect.as_deref().unwrap_or(""));
    if rec.stale {
        let late = elapsed(rec.overdue_minutes.unwrap_or(0).max(0) * 60);
        return format!("{time} <span class=\"late\">late {late}</span>");
    }
    let note = match rec.state.as_str() {
        "running" => "running".to_string(),
        "blocked" => "paused".to_string(),
        "done today" => "<span class=\"ok\">met</span>".to_string(),
        _ if rec.held => "held".to_string(),
        _ if now < minutes => format!("in {}", elapsed((minutes - now) * 60)),
        _ => "due".to_string(),
    };
    format!("{time} <span class=\"muted\">{note}</span>")
}

/// The why cell: the block reason, or the failing exit, or nothing.
fn why_html(rec: &JobState) -> String {
    if !rec.detail.is_empty() {
        return esc(&rec.detail);
    }
    match rec.last_exit {
        Some(rc) if rc != 0 => format!("exit {rc}"),
        _ => "—".into(),
    }
}

fn memory_text(rec: &JobState) -> String {
    match &rec.memory {
        Some(pass) => format!(
            "{ago} · {} file{}",
            pass.files,
            if pass.files == 1 { "" } else { "s" },
            ago = ago(pass.age)
        ),
        None => "no receipt".into(),
    }
}

fn deliverable_text(rec: &JobState) -> String {
    match (&rec.deliverable, rec.deliverable_ok) {
        (Some(pattern), Some(true)) => format!("{} · met today", esc(pattern)),
        (Some(pattern), Some(false)) => format!("{} · missing today", esc(pattern)),
        (Some(pattern), None) => esc(pattern),
        (None, _) => "not named (state/expect line 2)".into(),
    }
}

/// A per-job disclosure: the facts a table row should not carry, but a person
/// diagnosing the job wants one click away.
fn details(rec: &JobState) -> String {
    let summary = if rec.summary.is_empty() {
        "no spec.md in the job directory".to_string()
    } else {
        esc(&rec.summary)
    };
    let cadence = rec
        .cadence
        .as_deref()
        .map(esc)
        .unwrap_or_else(|| "not written".into());
    let unit = rec
        .runs
        .last()
        .map(|run| esc(&run.unit))
        .unwrap_or_else(|| "not run yet".into());
    let agent = match (&rec.agent, &rec.agent_error) {
        (Some(agent), _) => esc(agent),
        (None, Some(error)) => format!("profile error — {}", esc(error)),
        (None, None) => "runner's own default".into(),
    };
    let heal = match &rec.heal_last {
        Some(heal) => format!(
            "{} {} · {}",
            esc(&heal.action),
            esc(&heal.verdict),
            heal.age.map(ago).unwrap_or_else(|| "unknown".into())
        ),
        None => "none".into(),
    };
    let mut runs = String::new();
    if rec.runs.is_empty() {
        runs.push_str("<span class=\"muted\">none recorded</span>");
    } else {
        for run in rec.runs.iter().rev().take(5) {
            runs.push_str(&format!(
                "<li><span class=\"num\">{}</span> {} · rc {} · {}</li>",
                esc(&day_hhmm(&run.end, &clock::today())),
                esc(&run.verb),
                run.rc,
                state::duration_text(Some(run.seconds))
            ));
        }
    }
    format!(
        "<details class=\"more\"><summary>details</summary>\
         <dl class=\"dl\">\
         <dt>spec</dt><dd>{summary}</dd>\
         <dt>cadence</dt><dd>{cadence}</dd>\
         <dt>unit</dt><dd>{unit}</dd>\
         <dt>agent</dt><dd>{agent}</dd>\
         <dt>memory</dt><dd>{memory}</dd>\
         <dt>deliverable</dt><dd>{deliverable}</dd>\
         <dt>heal</dt><dd>{heal}</dd>\
         <dt>spend</dt><dd>{} today · {} all time</dd>\
         <dt>outputs</dt><dd>{} file(s) in out/</dd>\
         <dt>path</dt><dd>{}</dd>\
         </dl><ol class=\"runslist\">{runs}</ol></details>",
        money(rec.spend_today),
        money(rec.spend_total),
        rec.out_files,
        esc(&rec.job_dir),
        memory = memory_text(rec),
        deliverable = deliverable_text(rec),
    )
}

fn jobs_table(data: &[JobState]) -> String {
    if data.is_empty() {
        return "<tr><td colspan=\"10\" class=\"empty\">no jobs yet — \
                <code>lightsout init</code> writes the noop fixture</td></tr>"
            .into();
    }
    let now = clock::now_minutes();
    let today = clock::today();
    let now_epoch = clock::now_epoch();
    let mut rows = Vec::new();
    for rec in data {
        let class = row_class(&rec.state);
        let last = rec
            .last_run
            .as_deref()
            .map(|iso| {
                let age = rec
                    .last_run_epoch
                    .map(|at| ago((now_epoch - at).max(0)))
                    .unwrap_or_default();
                format!(
                    "<span title=\"{}\">{}</span> <span class=\"muted\">{age}</span>",
                    esc(iso),
                    esc(&day_hhmm(iso, &today))
                )
            })
            .unwrap_or_else(|| "—".into());
        let heal = if rec.heal_attempts_today > 0 {
            let title = rec
                .heal_last
                .as_ref()
                .map(|heal| format!("last heal: {} {}", heal.action, heal.verdict))
                .unwrap_or_default();
            format!(
                " <span class=\"tag\" title=\"{}\">heal {}</span>",
                esc(&title),
                rec.heal_attempts_today
            )
        } else {
            String::new()
        };
        let spend = if rec.spend_today == 0 {
            "—".to_string()
        } else {
            format!(
                "<span title=\"total {}\">{}</span>",
                money(rec.spend_total),
                money(rec.spend_today)
            )
        };
        let why = why_html(rec);
        let whyline = if why == "—" {
            String::new()
        } else {
            format!("<span class=\"whyline\">{why}</span>")
        };
        rows.push(format!(
            "<tr class='{class}'>\
             <td class=\"dot\"><span class=\"b\" title=\"{}\"></span></td>\
             <td class=\"job\"><span class=\"name\">{}</span>{flags}<span class=\"summary\" title=\"{}\">{}</span>{whyline}{details}</td>\
             <td class=\"state\"><span class=\"pill {}\">{}</span></td>\
             <td class=\"stage\">{}</td>\
             <td class=\"due\">{}</td>\
             <td class=\"last\">{last}</td>\
             <td class=\"dur\">{duration}</td>\
             <td class=\"hist col-hist\">{dots}</td>\
             <td class=\"churn col-churn\" title=\"{} attempts today, {} all time\"><span class=\"num\">{}</span><span class=\"muted\"> / {}</span>{heal}</td>\
             <td class=\"spend col-spend\">{spend}</td>\
             </tr>",
            esc(&rec.state),
            esc(&rec.name),
            esc(&rec.summary),
            esc(&rec.summary),
            pill_class(&rec.state),
            esc(&rec.state),
            stage_html(rec),
            due_html(rec, now),
            rec.attempts_today,
            rec.attempts_total,
            rec.attempts_today,
            rec.attempts_total,
            duration = duration_html(rec),
            dots = dots(rec),
            flags = flags(rec),
            details = details(rec),
            whyline = whyline,
        ));
    }
    rows.join("\n")
}

fn since_text(rec: &JobState) -> String {
    rec.since
        .as_deref()
        .and_then(clock::epoch_of)
        .map(|at| ago((clock::now_epoch() - at).max(0)))
        .unwrap_or_else(|| "unknown".into())
}

fn class_chip(rec: &JobState) -> String {
    match rec.class.as_deref() {
        Some(class) => format!("<span class=\"cls c-{}\">{}</span>", esc(class), esc(class)),
        None => String::new(),
    }
}

fn command_for(rec: &JobState) -> (&'static str, String) {
    match rec.class.as_deref() {
        Some("retryable") => (
            "retry the bounded repair, then the job again",
            format!("lightsout retry {}", rec.name),
        ),
        Some("judgment") => (
            "read the stop before deciding",
            format!("lightsout diagnose {}", rec.name),
        ),
        _ => (
            "read the stop, then clear it",
            format!("lightsout diagnose {}", rec.name),
        ),
    }
}

fn attention_item(rec: &JobState) -> Option<String> {
    match rec.state.as_str() {
        "blocked" => {
            let (hint, command) = command_for(rec);
            Some(format!(
                "<li><div class=\"attn-top\"><span class=\"pill p-blocked\">blocked</span>\
                 <span class=\"name\">{}</span>{class}\
                 <span class=\"age\">since {}</span></div>\
                 <p class=\"reason\">{}</p>\
                 <div class=\"fix\"><code>{command}</code><span>{hint}</span></div></li>",
                esc(&rec.name),
                since_text(rec),
                esc(&rec.detail),
                class = class_chip(rec),
                command = esc(&command),
                hint = hint,
            ))
        }
        "failed" => Some(format!(
            "<li><div class=\"attn-top\"><span class=\"pill p-failed\">failed</span>\
             <span class=\"name\">{}</span><span class=\"age\">exit {} · {}</span></div>\
             <p class=\"reason\">the last run failed and wrote no done marker</p>\
             <div class=\"fix\"><code>lightsout run {}</code><span>take one attempt, then read the log</span></div></li>",
            esc(&rec.name),
            rec.last_exit.unwrap_or(1),
            rec.last_run.as_deref().map(|iso| esc(&day_hhmm(iso, &clock::today()))).unwrap_or_else(|| "unknown".into()),
            esc(&rec.name),
        )),
        _ if rec.stale => {
            let reason = match (&rec.deliverable, rec.deliverable_ok) {
                (Some(pattern), Some(false)) => {
                    format!("no {} from today", esc(pattern))
                }
                _ => "no run by its expect time".to_string(),
            };
            Some(format!(
                "<li><div class=\"attn-top\"><span class=\"pill p-late\">overdue</span>\
                 <span class=\"name\">{}</span>\
                 <span class=\"age\">late {}</span></div>\
                 <p class=\"reason\">{reason} (expect {})</p>\
                 <div class=\"fix\"><code>lightsout check {}</code><span>alarm once, or run it now</span></div></li>",
                esc(&rec.name),
                elapsed(rec.overdue_minutes.unwrap_or(0).max(0) * 60),
                esc(rec.expect.as_deref().unwrap_or("—")),
                esc(&rec.name),
            ))
        }
        _ => None,
    }
}

fn attention(data: &[JobState]) -> String {
    let items: Vec<String> = data.iter().filter_map(attention_item).collect();
    if items.is_empty() {
        return "<section class=\"panel allclear\"><span class=\"dot-ok\"></span>\
                <span><b>All quiet</b> — nothing blocked, failed or overdue.</span></section>"
            .into();
    }
    format!(
        "<section class=\"panel attention\">\
         <div class=\"panel-head\"><h2>Needs attention</h2>\
         <span class=\"count-pill\">{}</span>\
         <span class=\"hint\">blocked, failed or overdue</span></div>\
         <ul class=\"attn\">{}</ul></section>",
        items.len(),
        items.join("")
    )
}

fn kpi(label: &str, value: &str, note: &str, tone: &str) -> String {
    format!(
        "<div class=\"kpi {tone}\"><span class=\"label\">{label}</span>\
         <span class=\"value\">{value}</span><span class=\"note\">{note}</span></div>"
    )
}

/// A job that is past its expect time with no done marker and no block: the
/// one state the attention panel calls "overdue". A failed job is counted once,
/// as failed, so the panel count and the KPI count cannot drift.
fn overdue_only(rec: &JobState) -> bool {
    rec.stale && rec.state != "failed" && rec.state != "blocked"
}

fn needs_person(rec: &JobState) -> bool {
    matches!(rec.state.as_str(), "blocked" | "failed") || overdue_only(rec)
}

fn kpis(data: &[JobState]) -> String {
    let blocked = data.iter().filter(|rec| rec.state == "blocked").count();
    let failed = data.iter().filter(|rec| rec.state == "failed").count();
    let running = data.iter().filter(|rec| rec.state == "running").count();
    let done = data.iter().filter(|rec| rec.state == "done today").count();
    let late = data.iter().filter(|rec| overdue_only(rec)).count();
    let never = data.iter().filter(|rec| rec.state == "never run").count();
    let spend: i64 = data.iter().map(|rec| rec.spend_today).sum();
    let needs = data.iter().filter(|rec| needs_person(rec)).count();

    let mut note = format!("{blocked} blocked · {failed} failed");
    if late > 0 {
        note.push_str(&format!(" · {late} late"));
    }

    let mut out = String::new();
    out.push_str(&kpi(
        "needs you",
        &needs.to_string(),
        &note,
        if needs > 0 { "danger" } else { "good" },
    ));
    out.push_str(&kpi(
        "running",
        &running.to_string(),
        if running > 0 {
            "with a live lock"
        } else {
            "nothing in flight"
        },
        if running > 0 { "warn" } else { "quiet" },
    ));
    out.push_str(&kpi(
        "done today",
        &done.to_string(),
        &format!("of {} job(s)", data.len()),
        if done > 0 { "good" } else { "quiet" },
    ));
    out.push_str(&kpi(
        "overdue",
        &late.to_string(),
        if late > 0 {
            "past expect, no run"
        } else {
            "all on time"
        },
        if late > 0 { "danger" } else { "quiet" },
    ));
    out.push_str(&kpi(
        "never run",
        &never.to_string(),
        if never > 0 {
            "no run recorded"
        } else {
            "every job has run"
        },
        "quiet",
    ));
    out.push_str(&kpi(
        "spend today",
        &money(spend),
        "across all jobs",
        "quiet",
    ));
    out
}

const MARK: &str = r##"<svg viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d="M12 2.8a6.2 6.2 0 0 0-3.7 11.18V17h7.4v-3.02A6.2 6.2 0 0 0 12 2.8Z" stroke="#f0c060" stroke-width="1.5" stroke-linejoin="round"/><path d="M9.6 20h4.8" stroke="#f0c060" stroke-width="1.5" stroke-linecap="round"/></svg>"##;

pub fn render(data: &[JobState], root: &Path) -> String {
    let generated = clock::format_epoch(clock::now_epoch(), "%Y-%m-%d %H:%M");
    let needs = data.iter().filter(|rec| needs_person(rec)).count();
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string());
    let title = format!(
        "LightsOut — {} job{}{}",
        data.len(),
        if data.len() == 1 { "" } else { "s" },
        if needs > 0 {
            format!(
                " · {needs} need{} a look",
                if needs == 1 { "s" } else { "" }
            )
        } else {
            " · all quiet".into()
        }
    );

    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"color-scheme\" content=\"light dark\">\n\
         <meta name=\"generator\" content=\"lightsout {version}\">\n\
         <title>{title}</title>\n\
         <link rel=\"icon\" href=\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E%3Crect width='32' height='32' rx='8' fill='%2319182e'/%3E%3Cpath d='M16 7a7 7 0 0 0-4.2 12.6V23h8.4v-3.4A7 7 0 0 0 16 7Z' fill='none' stroke='%23f0c060' stroke-width='1.8'/%3E%3Cpath d='M13.4 26h5.2' stroke='%23f0c060' stroke-width='1.8' stroke-linecap='round'/%3E%3C/svg%3E\">\n\
         <script>try{{var t=localStorage.getItem('lightsout-theme');if(t&&t!=='auto')document.documentElement.dataset.theme=t}}catch(e){{}}</script>\n\
         <style>{css}</style>\n</head>\n<body>\n<div class=\"wrap\">\n\
         <header>\n\
           <div class=\"brand\"><span class=\"mark\">{mark}</span>\
             <div><h1>LightsOut</h1>\
             <p class=\"tagline\">Unattended jobs, read from state files</p></div></div>\n\
           <div class=\"meta\">\
             <span class=\"chip\">{count} job{s}</span>\
             <span class=\"chip path\" title=\"{path}\">{name}</span>\
             <span class=\"chip\">updated {generated}</span>\
             <span class=\"chip\">v{version}</span>\
             <button class=\"theme\" id=\"theme\" type=\"button\" aria-label=\"Change theme\">Auto</button>\
           </div>\n\
         </header>\n\
         <section class=\"kpis\">{kpis}</section>\n\
         {attention}\n\
         <section class=\"panel jobs\">\n\
           <div class=\"toolbar\">\
             <div class=\"search\"><svg viewBox=\"0 0 16 16\" aria-hidden=\"true\"><circle cx=\"7\" cy=\"7\" r=\"4.6\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.4\"/><path d=\"m10.5 10.5 3 3\" stroke=\"currentColor\" stroke-width=\"1.4\" stroke-linecap=\"round\"/></svg>\
               <input id=\"q\" type=\"search\" placeholder=\"Filter by name, state or reason\" aria-label=\"Filter jobs\" autocomplete=\"off\"></div>\
             <div class=\"segmented\" role=\"group\" aria-label=\"Filter by state\">\
               <button type=\"button\" data-filter=\"all\" aria-pressed=\"true\">All</button>\
               <button type=\"button\" data-filter=\"attention\" aria-pressed=\"false\">Needs attention</button>\
               <button type=\"button\" data-filter=\"running\" aria-pressed=\"false\">Running</button>\
               <button type=\"button\" data-filter=\"late\" aria-pressed=\"false\">Late</button>\
               <button type=\"button\" data-filter=\"done\" aria-pressed=\"false\">Done today</button>\
             </div>\
             <span class=\"showing\"><span id=\"shown\">{count}</span> of {count}</span>\
           </div>\n\
           <div class=\"tablewrap\"><table>\n\
             <thead><tr>\
               <th class=\"col-dot\" scope=\"col\"><span class=\"sr\">state</span></th>\
               <th scope=\"col\">job</th>\
               <th scope=\"col\">state</th>\
               <th scope=\"col\">stage</th>\
               <th scope=\"col\">due</th>\
               <th scope=\"col\">last run</th>\
               <th scope=\"col\">duration</th>\
               <th class=\"col-hist\" scope=\"col\" title=\"the last ten runs, oldest first\">last 10</th>\
               <th class=\"col-churn\" scope=\"col\" title=\"attempts today / all time\">attempts</th>\
               <th class=\"col-spend\" scope=\"col\" title=\"spent today / all time\">spend</th>\
             </tr></thead>\n\
             <tbody>\n{rows}\n</tbody>\n</table></div>\n\
         </section>\n\
         <footer><span>LightsOut v{version} · generated {generated} · static page, no server</span>\
         <span>refresh with <code>bin/report --html out/status.html --open</code></span></footer>\n\
         </div>\n<script>{script}</script>\n</body>\n</html>\n",
        version = repo::VERSION,
        title = esc(&title),
        mark = MARK,
        count = data.len(),
        s = if data.len() == 1 { "" } else { "s" },
        path = esc(&root.display().to_string()),
        name = esc(&name),
        generated = generated,
        kpis = kpis(data),
        attention = attention(data),
        rows = jobs_table(data),
        css = CSS,
        script = SCRIPT,
    )
}

const CSS: &str = r##"
*,*::before,*::after{box-sizing:border-box}
:root{
  color-scheme:light;
  --bg:#f2f0ea; --panel:#fffdf8; --panel-2:#f7f4ed; --ink:#1d1b17; --muted:#726c61;
  --line:#e4dfd3; --line-2:#d2cbbe; --accent:#4338ca; --accent-soft:#ecebfe;
  --red:#ad3027; --red-soft:#fbe7e4; --amber:#8d5a08; --amber-soft:#fbeed7;
  --green:#11673c; --green-soft:#e3f2e8; --gray:#6d6961; --gray-soft:#edeae3;
  --violet:#5b3fc4; --violet-soft:#efeafd;
  --radius:16px; --mono:ui-monospace,SFMono-Regular,"SF Mono",Menlo,monospace;
  --sans:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;
  --shadow:0 1px 1px rgba(30,25,15,.04),0 18px 40px -30px rgba(30,25,15,.45);
}
@media (prefers-color-scheme:dark){
  :root:not([data-theme="light"]){
    color-scheme:dark;
    --bg:#101116; --panel:#181a20; --panel-2:#1e2128; --ink:#eceae4; --muted:#a19e97;
    --line:#292c34; --line-2:#3a3e48; --accent:#a9a3ff; --accent-soft:#272343;
    --red:#ff9d90; --red-soft:#3a201d; --amber:#efb96a; --amber-soft:#372a16;
    --green:#78d5a1; --green-soft:#12301f; --gray:#a29e98; --gray-soft:#252930;
    --violet:#b9a6ff; --violet-soft:#271f40;
    --shadow:0 1px 1px rgba(0,0,0,.4),0 20px 44px -30px rgba(0,0,0,.9);
  }
}
:root[data-theme="dark"]{
  color-scheme:dark;
  --bg:#101116; --panel:#181a20; --panel-2:#1e2128; --ink:#eceae4; --muted:#a19e97;
  --line:#292c34; --line-2:#3a3e48; --accent:#a9a3ff; --accent-soft:#272343;
  --red:#ff9d90; --red-soft:#3a201d; --amber:#efb96a; --amber-soft:#372a16;
  --green:#78d5a1; --green-soft:#12301f; --gray:#a29e98; --gray-soft:#252930;
  --violet:#b9a6ff; --violet-soft:#271f40;
  --shadow:0 1px 1px rgba(0,0,0,.4),0 20px 44px -30px rgba(0,0,0,.9);
}
html{-webkit-text-size-adjust:100%}
body{
  margin:0;padding:34px 24px 52px;color:var(--ink);
  font:14.5px/1.55 var(--sans);
  background:
    radial-gradient(900px 420px at 6% -14%,rgba(67,56,202,.10),transparent 62%),
    radial-gradient(760px 400px at 98% -10%,rgba(180,120,40,.10),transparent 58%),
    var(--bg);
}
.wrap{max-width:1360px;margin:0 auto;display:flex;flex-direction:column;gap:20px}
header{display:flex;justify-content:space-between;align-items:flex-start;gap:18px;flex-wrap:wrap}
.brand{display:flex;align-items:center;gap:14px}
.mark{width:44px;height:44px;border-radius:13px;display:grid;place-items:center;
  background:linear-gradient(160deg,#2b2652,#141227);
  box-shadow:inset 0 0 0 1px rgba(255,255,255,.08),var(--shadow)}
.mark svg{width:23px;height:23px;display:block}
h1{margin:0;font-size:23px;font-weight:650;letter-spacing:-.022em;line-height:1.15}
.tagline{margin:2px 0 0;color:var(--muted);font-size:12.5px}
.meta{display:flex;align-items:center;gap:7px;flex-wrap:wrap;justify-content:flex-end}
.chip{display:inline-flex;align-items:center;gap:6px;padding:4px 10px;border:1px solid var(--line);
  border-radius:999px;background:var(--panel);font-size:11.5px;color:var(--muted);white-space:nowrap}
.chip.path{font-family:var(--mono);font-size:11px;max-width:320px;overflow:hidden;text-overflow:ellipsis}
button{font:inherit;color:inherit}
.theme{border:1px solid var(--line);background:var(--panel);border-radius:999px;padding:4px 10px;
  font-size:11.5px;color:var(--muted);cursor:pointer}
.theme:hover{color:var(--ink);border-color:var(--line-2)}
.kpis{display:grid;grid-template-columns:repeat(6,minmax(0,1fr));gap:11px}
.kpi{position:relative;overflow:hidden;background:var(--panel);border:1px solid var(--line);
  border-radius:14px;padding:13px 15px;box-shadow:var(--shadow)}
.kpi::before{content:"";position:absolute;inset:0 auto 0 0;width:3px;background:var(--tone,var(--line-2))}
.kpi.danger{--tone:var(--red)} .kpi.warn{--tone:var(--amber)}
.kpi.good{--tone:var(--green)} .kpi.quiet{--tone:var(--gray)}
.kpi .label{display:block;font-size:10.5px;font-weight:600;letter-spacing:.085em;
  text-transform:uppercase;color:var(--muted)}
.kpi .value{display:block;font-size:27px;font-weight:640;letter-spacing:-.022em;line-height:1.25;
  font-variant-numeric:tabular-nums}
.kpi .note{display:block;font-size:11.5px;color:var(--muted)}
.panel{background:var(--panel);border:1px solid var(--line);border-radius:var(--radius);
  box-shadow:var(--shadow);overflow:hidden}
.panel-head{display:flex;align-items:center;gap:10px;padding:14px 18px;border-bottom:1px solid var(--line);flex-wrap:wrap}
h2{margin:0;font-size:14px;font-weight:640;letter-spacing:-.01em}
.count-pill{min-width:22px;text-align:center;padding:1px 7px;border-radius:999px;
  background:var(--red-soft);color:var(--red);font-size:11.5px;font-weight:650;font-variant-numeric:tabular-nums}
.hint{color:var(--muted);font-size:11.5px}
.attention{border-color:var(--line-2)}
.allclear{display:flex;align-items:center;gap:10px;padding:14px 18px;color:var(--muted);font-size:13px}
.allclear b{color:var(--ink)}
.dot-ok{width:8px;height:8px;border-radius:50%;background:var(--green);flex:none;
  box-shadow:0 0 0 3px var(--green-soft)}
.attn{list-style:none;margin:0;padding:0}
.attn li{padding:13px 18px;border-top:1px solid var(--line);display:grid;gap:5px}
.attn li:first-child{border-top:0}
.attn-top{display:flex;align-items:center;gap:8px;flex-wrap:wrap}
.attn .name{font-weight:650;font-size:13.5px}
.attn .age{color:var(--muted);font-size:11.5px}
.reason{margin:0;font-size:13px;color:var(--ink);overflow-wrap:anywhere}
.fix{display:flex;align-items:center;gap:9px;flex-wrap:wrap;color:var(--muted);font-size:11.5px}
code{font-family:var(--mono);font-size:11.5px;background:var(--panel-2);
  border:1px solid var(--line);border-radius:7px;padding:2px 7px;color:var(--ink)}
.pill{display:inline-flex;align-items:center;gap:5px;padding:1px 9px;border-radius:999px;
  font-size:11px;font-weight:650;line-height:1.8;white-space:nowrap;background:var(--gray-soft);color:var(--gray)}
.pill::before{content:"";width:6px;height:6px;border-radius:50%;background:currentColor;flex:none}
.p-blocked{background:var(--red-soft);color:var(--red)}
.p-failed,.p-late{background:var(--amber-soft);color:var(--amber)}
.p-running{background:var(--amber-soft);color:var(--amber)}
.p-done{background:var(--green-soft);color:var(--green)}
.p-ran{background:var(--gray-soft);color:var(--gray)}
.p-never{background:var(--gray-soft);color:var(--gray)}
.cls{font-size:10.5px;font-weight:600;padding:1px 7px;border-radius:5px;letter-spacing:.01em}
.c-retryable{background:var(--amber-soft);color:var(--amber)}
.c-judgment{background:var(--violet-soft);color:var(--violet)}
.c-human{background:var(--red-soft);color:var(--red)}
.toolbar{display:flex;align-items:center;gap:10px;flex-wrap:wrap;padding:13px 16px;
  border-bottom:1px solid var(--line)}
.search{flex:1 1 220px;display:flex;align-items:center;gap:8px;min-width:180px;
  background:var(--panel-2);border:1px solid var(--line);border-radius:10px;padding:6px 10px;color:var(--muted)}
.search:focus-within{border-color:var(--accent);box-shadow:0 0 0 3px var(--accent-soft)}
.search svg{width:14px;height:14px;flex:none}
.search input{flex:1;min-width:0;border:0;background:transparent;outline:0;color:var(--ink);
  font:13px var(--sans)}
.search input::placeholder{color:var(--muted)}
.segmented{display:flex;gap:2px;background:var(--panel-2);border:1px solid var(--line);
  border-radius:10px;padding:3px}
.segmented button{border:0;background:transparent;color:var(--muted);font-size:12px;
  padding:4px 9px;border-radius:7px;cursor:pointer;white-space:nowrap}
.segmented button:hover{color:var(--ink)}
.segmented button[aria-pressed="true"]{background:var(--panel);color:var(--ink);font-weight:600;
  box-shadow:0 1px 2px rgba(0,0,0,.10)}
.showing{color:var(--muted);font-size:11.5px;margin-left:auto;font-variant-numeric:tabular-nums}
.tablewrap{overflow-x:auto}
table{width:100%;border-collapse:separate;border-spacing:0;font-size:12.8px}
thead th{position:sticky;top:0;z-index:1;background:var(--panel-2);color:var(--muted);
  font-size:10.5px;font-weight:600;letter-spacing:.08em;text-transform:uppercase;text-align:left;
  padding:9px 10px;border-bottom:1px solid var(--line);white-space:nowrap}
th.col-dot{width:1%}
tbody td{padding:10px 9px;border-bottom:1px solid var(--line);vertical-align:top}
tbody tr:last-child td{border-bottom:0}
tbody tr[hidden]{display:none}
tbody tr:hover td{background:var(--panel-2)}
td.dot{width:1%;padding-right:0;padding-top:16px}
.b{display:block;width:7px;height:7px;border-radius:50%;background:var(--tone,var(--gray))}
tr.blocked{--tone:var(--red)} tr.running{--tone:var(--amber)} tr.failed{--tone:var(--amber)}
tr.done-today{--tone:var(--green)} tr.ran{--tone:var(--line-2)} tr.never-run{--tone:var(--gray)}
td.job{min-width:190px;max-width:300px}
.name{font-weight:650;font-size:13.2px;letter-spacing:-.005em}
.flags{display:inline-flex;gap:4px;margin-left:6px;vertical-align:1px}
.tag{display:inline-block;font-size:10px;line-height:1.6;padding:0 5px;border-radius:5px;
  background:var(--panel-2);border:1px solid var(--line);color:var(--muted);white-space:nowrap}
.tag.warn{background:var(--amber-soft);border-color:transparent;color:var(--amber)}
.summary{display:block;margin-top:1px;color:var(--muted);font-size:11.5px;max-width:34ch;
  white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
td.state{white-space:nowrap}
.whyline{display:block;margin-top:1px;color:var(--muted);font-size:11.5px;max-width:44ch;
  overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
tr.blocked .whyline,tr.failed .whyline{color:var(--red)}
td.stage{white-space:nowrap;color:var(--muted)}
td.stage b{color:var(--ink);font-variant-numeric:tabular-nums}
tr.blocked td.stage b{color:var(--ink)}
td.due,td.last,td.dur,td.churn,td.spend{white-space:nowrap;font-variant-numeric:tabular-nums}
td.hist{white-space:nowrap;width:1%}
td.due{width:1%}
td.spend{text-align:right;width:1%}
td.dur .work,td .muted,span.muted{color:var(--muted)}
td.dur .work{font-size:11px}
.late{color:var(--red);font-weight:600}
.ok{color:var(--green);font-weight:600}
.dots{white-space:nowrap;line-height:1}
.rd{display:inline-block;width:8px;height:8px;border-radius:2px;margin-right:2px;
  background:var(--green);opacity:.92;vertical-align:middle}
.rd:last-child{margin-right:0}
.rd.r0{background:var(--green)} .rd.r1{background:var(--amber)} .rd.r2{background:var(--red)}
td.spend{color:var(--muted)}
tr.blocked td.spend,tr.failed td.spend{color:var(--ink)}
.more{margin-top:4px}
.more summary{display:inline-flex;align-items:center;gap:4px;cursor:pointer;list-style:none;
  color:var(--accent);font-size:11px;font-weight:600}
.more summary::-webkit-details-marker{display:none}
.more summary::before{content:"+";font-family:var(--mono);width:8px}
.more[open] summary::before{content:"–"}
.dl{display:grid;grid-template-columns:auto minmax(0,1fr);gap:5px 12px;margin:8px 0 0;
  font-size:11.8px;max-width:560px}
.dl dt{color:var(--muted);white-space:nowrap}
.dl dd{margin:0;overflow-wrap:anywhere}
.dl code{font-size:11px}
.runslist{list-style:none;margin:7px 0 0;padding:0;display:grid;gap:2px;
  font-size:11.5px;color:var(--muted);font-family:var(--mono)}
.runslist .num{color:var(--ink)}
.empty{color:var(--muted);padding:22px 18px}
footer{display:flex;justify-content:space-between;gap:14px;flex-wrap:wrap;color:var(--muted);font-size:11.5px}
footer code{background:transparent;border:0;padding:0;color:var(--muted)}
.sr{position:absolute;width:1px;height:1px;overflow:hidden;clip:rect(0 0 0 0);white-space:nowrap}
:focus-visible{outline:2px solid var(--accent);outline-offset:2px}
@media (max-width:1120px){.kpis{grid-template-columns:repeat(3,minmax(0,1fr))}
  .summary{max-width:26ch}}
@media (max-width:980px){.col-spend{display:none}.col-churn .tag{display:none}}
@media (max-width:820px){.col-hist{display:none}.kpis{grid-template-columns:repeat(2,minmax(0,1fr))}
  .summary{display:none}.showing{display:none}}
@media (max-width:560px){
  body{padding:20px 12px 36px}
  .meta{justify-content:flex-start}
  .segmented{width:100%;overflow-x:auto}
  td.why{max-width:180px}
}
@media print{
  body{background:#fff;padding:0}
  .theme,.toolbar,.more{display:none}
  .panel,.kpi{box-shadow:none;border-color:#ccc}
  header{margin-bottom:12px}
}
"##;

const SCRIPT: &str = r##"
(function(){
  var root=document.documentElement;
  var btn=document.getElementById('theme');
  var labels={auto:'Auto',light:'Light',dark:'Dark'};
  var mode='auto';
  try{ mode=localStorage.getItem('lightsout-theme')||'auto'; }catch(e){}
  function apply(next){
    mode=next;
    if(mode==='auto'){ root.removeAttribute('data-theme'); } else { root.dataset.theme=mode; }
    if(btn){ btn.textContent=labels[mode]; btn.setAttribute('aria-label','Theme: '+labels[mode]); }
    try{ localStorage.setItem('lightsout-theme',mode); }catch(e){}
  }
  apply(labels[mode]?mode:'auto');
  if(btn){ btn.addEventListener('click',function(){
    apply(mode==='auto'?'light':mode==='light'?'dark':'auto');
  }); }

  var rows=[].slice.call(document.querySelectorAll('tbody tr')).filter(function(row){
    return !row.querySelector('.empty');
  });
  if(!rows.length){ return; }
  var chips=[].slice.call(document.querySelectorAll('[data-filter]'));
  var search=document.getElementById('q');
  var shown=document.getElementById('shown');
  var filter='all';

  function matches(row){
    if(filter==='attention'){
      return row.classList.contains('blocked')||row.classList.contains('failed')||!!row.querySelector('.late');
    }
    if(filter==='running'){ return row.classList.contains('running'); }
    if(filter==='done'){ return row.classList.contains('done-today'); }
    if(filter==='late'){ return !!row.querySelector('.late'); }
    return true;
  }
  rows.forEach(function(row){
    var copy=row.cloneNode(true);
    copy.querySelectorAll('details').forEach(function(d){ d.remove(); });
    row.dataset.search=copy.textContent.toLowerCase();
  });
  function draw(){
    var q=(search&&search.value||'').trim().toLowerCase();
    var visible=0;
    rows.forEach(function(row){
      var keep=matches(row)&&(!q||row.dataset.search.indexOf(q)!==-1);
      row.hidden=!keep;
      if(keep){ visible++; }
    });
    if(shown){ shown.textContent=visible; }
  }
  chips.forEach(function(chip){
    chip.addEventListener('click',function(){
      filter=chip.dataset.filter||'all';
      chips.forEach(function(other){ other.setAttribute('aria-pressed',String(other===chip)); });
      draw();
    });
  });
  if(search){ search.addEventListener('input',draw); }
  document.addEventListener('keydown',function(event){
    if(event.key==='/'&&document.activeElement!==search){ event.preventDefault(); if(search){ search.focus(); } }
  });
  draw();
})();
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_stays_short() {
        assert_eq!(elapsed(7), "7s");
        assert_eq!(elapsed(63), "1m");
        assert_eq!(elapsed(3600), "1h");
        assert_eq!(elapsed(4800), "1h 20m");
        assert_eq!(elapsed(90_000), "1d 1h");
        assert_eq!(elapsed(172_800), "2d");
        assert_eq!(elapsed(180_000), "2d 2h");
        assert_eq!(ago(20), "just now");
        assert_eq!(ago(7200), "2h ago");
    }

    #[test]
    fn money_reads_in_dollars() {
        assert_eq!(money(0), "$0.00");
        assert_eq!(money(42), "$0.42");
        assert_eq!(money(1234), "$12.34");
    }

    #[test]
    fn a_stage_line_becomes_a_counter() {
        assert_eq!(
            stage_parts("stage 2: apply the plan"),
            Some(("2".into(), "apply the plan".into()))
        );
        assert_eq!(stage_parts("garbage"), None);
    }

    #[test]
    fn dynamic_text_is_escaped() {
        assert_eq!(esc("<script>x</script>"), "&lt;script&gt;x&lt;/script&gt;");
        assert_eq!(esc("a & b \"c\""), "a &amp; b &quot;c&quot;");
    }

    #[test]
    fn the_filter_ignores_hidden_details() {
        assert!(SCRIPT.contains("copy.querySelectorAll('details')"));
        assert!(!SCRIPT.contains("row.textContent"));
    }

    #[test]
    fn a_memory_receipt_before_the_work_run_is_stale() {
        assert!(memory_is_stale(25 * 3600, 2 * 3600));
        assert!(!memory_is_stale(2 * 3600, 25 * 3600));
        assert!(!memory_is_stale(25 * 3600, 25 * 3600));
    }

    #[test]
    fn the_due_cell_names_the_day() {
        assert_eq!(day_hhmm("2026-11-03T08:00:07-0600", "2026-11-03"), "08:00");
        assert_eq!(
            day_hhmm("2026-11-02T23:30:00-0600", "2026-11-03"),
            "11-02 23:30"
        );
    }
}
