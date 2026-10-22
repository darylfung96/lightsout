//! Tests/run.sh sections 13–23, ported check for check.

mod common;

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use common::*;
use serde_json::Value;

fn text(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn number(row: &Value, key: &str) -> i64 {
    row.get(key).and_then(Value::as_i64).unwrap_or(-1)
}

fn boolean(row: &Value, key: &str) -> bool {
    row.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn spend_today(s: &Sandbox) -> i64 {
    number(&s.row("noop"), "spend_today")
}

fn stale_of_noop(s: &Sandbox) -> bool {
    boolean(&s.row("noop"), "stale")
}

fn agent_invocations(s: &Sandbox, calls: &str) -> usize {
    if s.is_file(calls) {
        s.count_lines(calls)
    } else {
        0
    }
}

fn write_resource_lock(s: &Sandbox, pid: u32, expiry: i64) {
    s.mkdir("state/resources");
    s.write(
        "state/resources/fixture.lock",
        &format!("{pid}\t{expiry}\n"),
    );
}

fn notify_stubs(s: &Sandbox) -> PathBuf {
    let dir = s.path("notify-stubs");
    fs::create_dir_all(&dir).unwrap();
    for program in ["osascript", "curl"] {
        write_exec(
            &dir.join(program),
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"${NOTIFY_LOG:-/dev/null}\"\n",
        );
    }
    dir
}

fn shots_command(s: &Sandbox, path: &str, min: Option<&str>) -> std::process::Output {
    let mut cmd = s.command();
    cmd.args(["gate", "shots", "--file", path]);
    if let Some(min) = min {
        cmd.args(["--min", min]);
    }
    cmd.output().expect("gate shots")
}

#[test]
fn t13_spend_ledger() {
    let s = Sandbox::new("t13");
    let mut c = Checks::new(13);
    s.reset_noop();
    c.check("the paid run completes", 0, code(&s.run(&["run", "noop"])));
    c.check("spend_today sums today's ledger", 25, spend_today(&s));
    let ledger = s.read("jobs/noop/state/ledger.tsv");
    let first = ledger.lines().next().unwrap_or("");
    c.check(
        "the ledger books 25c for stage 1",
        "25|stage 1",
        format!("{}|{}", field(first, 1), field(first, 2)),
    );
    c.check("the ledger line names the unit", s.today(), field(first, 3));
    s.run(&["run", "noop"]);
    c.check("a held re-run books nothing", 25, spend_today(&s));

    s.reset_noop();
    s.mkdir("jobs/noop/state");
    s.write(
        "jobs/noop/state/ledger.tsv",
        "2020-01-01T00:00:00-0600\t900\tstage 1\t2020-01-01\n",
    );
    let under = s.run_env(&["run", "noop"], &[("U_SPEND_CAP", "100")]);
    c.check(
        "a run under the cap is not blocked by old spend",
        0,
        code(&under),
    );
    c.check(
        "yesterday's spend is not counted today",
        25,
        spend_today(&s),
    );

    s.reset_noop();
    let over = s.run_env(&["run", "noop"], &[("U_SPEND_CAP", "10")]);
    c.check("spend cap blocks", 2, code(&over));
    let row = s.row("noop");
    c.check(
        "the spend block is a human stop",
        "human|spend cap reached (25 of 10 cents today)",
        format!("{}|{}", text(&row, "class"), text(&row, "detail")),
    );

    s.reset_noop();
    s.run_env(
        &["run", "noop"],
        &[("NOOP_BLOCK", "1"), ("NOOP_CLASS", "retryable")],
    );
    c.check(
        "the retried run finishes",
        0,
        code(&s.run(&["retry", "noop"])),
    );
    c.check("a retry does not double-charge", 25, spend_today(&s));
    c.finish(13);
}

#[test]
fn t14_stale_and_legitimate_holds() {
    let s = Sandbox::new("t14");
    let mut c = Checks::new(14);
    s.reset_noop();
    c.check(
        "a job with no expectation is never stale",
        false,
        stale_of_noop(&s),
    );

    s.reset_noop();
    s.mkdir("jobs/noop/state");
    s.write("jobs/noop/state/expect", "00:00\n");
    c.check(
        "a job past its time with nothing done is stale",
        true,
        stale_of_noop(&s),
    );
    s.run(&["run", "noop"]);
    c.check("a job done today is not stale", false, stale_of_noop(&s));

    let held = s.run(&["run", "noop"]);
    c.check("the second run is held", 0, code(&held));
    c.is_true(
        "the day guard hold is reported",
        combined(&held).contains("held:"),
    );
    c.check("a day-guard hold is not stale", false, stale_of_noop(&s));

    s.reset_noop();
    s.mkdir("jobs/noop/state/lock");
    s.write("jobs/noop/state/expect", "00:00\n");
    s.write(
        "jobs/noop/state/lock/pid",
        &format!("{}\n", std::process::id()),
    );
    c.check("a held lock is not stale", false, stale_of_noop(&s));
    let locked = s.run(&["run", "noop"]);
    c.check("a run under another run's lock exits 0", 0, code(&locked));
    c.is_true(
        "the lock hold is reported",
        combined(&locked).contains("held: another run"),
    );
    let _ = fs::remove_dir_all(s.path("jobs/noop/state/lock"));
    c.check(
        "a run that ended held is not a missed run",
        false,
        stale_of_noop(&s),
    );
    c.finish(14);
}

#[test]
fn t15_check_alarm() {
    let s = Sandbox::new("t15");
    let mut c = Checks::new(15);
    let today = s.today();
    let stale_alerts = |s: &Sandbox| s.count_containing("logs/alerts.log", "noop stale");

    s.reset_noop();
    s.mkdir("jobs/noop/state");
    s.write("jobs/noop/state/expect", "00:00\n");
    let before = stale_alerts(&s);
    let check = s.run(&["check"]);
    c.check("check exits 0 for a stale job", 0, code(&check));
    c.check(
        "check writes one stale marker",
        0,
        if s.is_file(&format!("jobs/noop/state/alerts/{today}.stale")) {
            0
        } else {
            1
        },
    );
    c.check("check notifies once", 1, stale_alerts(&s) - before);
    s.run(&["check"]);
    c.check(
        "a second check does not notify again",
        1,
        stale_alerts(&s) - before,
    );
    c.check(
        "check never runs the job",
        1,
        if s.is_file(&format!("jobs/noop/state/done/{today}")) {
            0
        } else {
            1
        },
    );
    c.check(
        "check writes no artifact",
        1,
        if s.is_file("jobs/noop/out/hello.txt") {
            0
        } else {
            1
        },
    );

    s.reset_noop();
    s.mkdir("jobs/noop/state");
    s.write("jobs/noop/state/expect", "00:00\n");
    s.run(&["run", "noop"]);
    let before = stale_alerts(&s);
    c.check(
        "check exits 0 when the job is done",
        0,
        code(&s.run(&["check"])),
    );
    c.check(
        "a done job raises no stale marker",
        1,
        if s.is_file(&format!("jobs/noop/state/alerts/{today}.stale")) {
            0
        } else {
            1
        },
    );
    c.check("a done job raises no stale alert", before, stale_alerts(&s));

    s.reset_noop();
    s.mkdir("jobs/noop/state");
    s.write("jobs/noop/state/expect", "00:00\n");
    s.run_env(&["run", "noop"], &[("NOOP_BLOCK", "1")]);
    let before = stale_alerts(&s);
    c.check(
        "a missed run that is blocked exits 2",
        2,
        code(&s.run(&["check"])),
    );
    c.check(
        "a blocked job does not raise the stale alert",
        before,
        stale_alerts(&s),
    );
    c.check(
        "a blocked job writes no stale marker",
        1,
        if s.is_file(&format!("jobs/noop/state/alerts/{today}.stale")) {
            0
        } else {
            1
        },
    );

    s.reset_noop();
    s.mkdir("jobs/noop/state/lock");
    s.write("jobs/noop/state/expect", "00:00\n");
    s.write(
        "jobs/noop/state/lock/pid",
        &format!("{}\n", std::process::id()),
    );
    s.run(&["run", "noop"]);
    let _ = fs::remove_dir_all(s.path("jobs/noop/state/lock"));
    s.run_env(&["run", "noop"], &[("NOOP_BLOCK", "1")]);
    c.check(
        "a hold does not mask a later block",
        2,
        code(&s.run(&["check"])),
    );
    c.finish(15);
}

#[test]
fn t16_notifications() {
    let s = Sandbox::new("t16");
    let mut c = Checks::new(16);
    let stub = notify_stubs(&s);
    let notify_log = s.path("notify.log");
    let notify_log_str = notify_log.to_string_lossy().into_owned();
    let path = format!(
        "{}:{}",
        stub.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let command = format!("printf \"%s|%s\\n\" >> {}", notify_log.display());
    let blocked_alerts = |s: &Sandbox| s.count_containing("logs/alerts.log", "noop blocked");

    s.reset_noop();
    let _ = fs::remove_file(&notify_log);
    let before = blocked_alerts(&s);
    let out = s.run_env(
        &["run", "noop"],
        &[
            ("NOOP_BLOCK", "1"),
            ("U_NOTIFY", "1"),
            ("U_NOTIFY_CMD", &command),
            ("U_NOTIFY_WEBHOOK", "http://example.invalid/hook"),
            ("PATH", &path),
            ("NOTIFY_LOG", &notify_log_str),
        ],
    );
    c.check("channels do not change the verdict", 2, code(&out));
    let log = fs::read_to_string(&notify_log).unwrap_or_default();
    c.is_true(
        "the command channel gets the title and the body",
        log.contains("LightsOut: noop blocked|simulated gate failure"),
    );
    c.is_true(
        "the desktop channel falls back through osascript",
        log.contains("display notification"),
    );
    c.is_true(
        "the webhook channel fires",
        log.contains("http://example.invalid/hook"),
    );
    c.is_true(
        "the webhook carries the block as JSON",
        log.contains("\"body\": \"simulated gate failure"),
    );
    c.check("the alert is recorded once", 1, blocked_alerts(&s) - before);

    s.reset_noop();
    let _ = fs::remove_file(&notify_log);
    let before = blocked_alerts(&s);
    let out = s.run_env(
        &["run", "noop"],
        &[
            ("NOOP_BLOCK", "1"),
            ("U_NOTIFY", "0"),
            ("U_NOTIFY_CMD", &command),
            ("U_NOTIFY_WEBHOOK", "http://example.invalid/hook"),
            ("PATH", &path),
            ("NOTIFY_LOG", &notify_log_str),
        ],
    );
    c.check("a silenced notification still blocks", 2, code(&out));
    c.check(
        "U_NOTIFY=0 silences every channel",
        1,
        if notify_log.exists() { 0 } else { 1 },
    );
    c.check(
        "a silenced notification is still recorded",
        1,
        blocked_alerts(&s) - before,
    );

    s.reset_noop();
    let out = s.run_env(
        &["run", "noop"],
        &[
            ("NOOP_BLOCK", "1"),
            ("U_NOTIFY", "1"),
            ("U_NOTIFY_CMD", "false"),
            ("PATH", &path),
        ],
    );
    c.check("a failing channel does not fail the job", 2, code(&out));
    c.check(
        "a failing channel keeps the block",
        0,
        if s.is_file("jobs/noop/state/blocked") {
            0
        } else {
            1
        },
    );

    s.reset_noop();
    let started = Instant::now();
    let out = s.run_env(
        &["run", "noop"],
        &[
            ("NOOP_BLOCK", "1"),
            ("U_NOTIFY", "1"),
            ("U_NOTIFY_CMD", "sleep 30; :"),
            ("PATH", &path),
        ],
    );
    c.check(
        "a hanging channel does not change the verdict",
        2,
        code(&out),
    );
    let elapsed = started.elapsed().as_secs();
    c.check(
        &format!("a hanging channel is cut off at 5s, not at its own pace ({elapsed}s)"),
        0,
        if (4..15).contains(&elapsed) { 0 } else { 1 },
    );
    c.finish(16);
}

#[test]
fn t17_run_history() {
    let s = Sandbox::new("t17");
    let mut c = Checks::new(17);
    s.reset_noop();
    let row = s.row("noop");
    c.is_true(
        "a job with no runs has an empty history",
        row.get("runs")
            .and_then(Value::as_array)
            .map(|runs| runs.is_empty())
            .unwrap_or(false)
            && row.get("last_duration") == Some(&Value::Null),
    );

    s.reset_noop();
    s.run(&["run", "noop"]);
    s.run(&["run", "noop"]);
    let runs = s.read("jobs/noop/state/runs.tsv");
    c.check("two runs recorded", 2, runs.lines().count());
    c.check(
        "a run line has six fields",
        6,
        runs.lines().next().unwrap_or("").split('\t').count(),
    );
    c.check(
        "the held run is in the history too",
        "0",
        field(runs.lines().nth(1).unwrap_or(""), 2),
    );
    let first = runs.lines().next().unwrap_or("");
    c.check(
        "a run records its verb and unit",
        format!("run|{}", s.today()),
        format!("{}|{}", field(first, 3), field(first, 4)),
    );
    let row = s.row("noop");
    let parsed = row
        .get("runs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let ok = parsed.len() == 2
        && parsed[0].get("verb").and_then(Value::as_str) == Some("run")
        && parsed[0].get("rc").and_then(Value::as_i64) == Some(0)
        && parsed[0]
            .get("start")
            .and_then(Value::as_str)
            .is_some_and(|s| s.starts_with("20"))
        && parsed[0]
            .get("end")
            .and_then(Value::as_str)
            .is_some_and(|s| s.starts_with("20"))
        && parsed[0]
            .get("seconds")
            .and_then(Value::as_i64)
            .is_some_and(|s| s >= 0)
        && parsed[1].get("seconds") == row.get("last_duration");
    c.is_true("status --json parses runs with durations", ok);

    s.reset_noop();
    s.run_env(
        &["run", "noop"],
        &[("NOOP_BLOCK", "1"), ("NOOP_CLASS", "retryable")],
    );
    s.run(&["retry", "noop"]);
    let runs = s.read("jobs/noop/state/runs.tsv");
    c.check(
        "a blocked run keeps its exit in the history",
        "2",
        field(runs.lines().next().unwrap_or(""), 2),
    );
    c.check(
        "a retry is recorded as its own verb",
        "retry",
        field(runs.lines().last().unwrap_or(""), 3),
    );

    s.reset_noop();
    for _ in 0..12 {
        s.run(&["run", "noop"]);
    }
    c.check(
        "runs.tsv keeps every completed run",
        12,
        s.count_lines("jobs/noop/state/runs.tsv"),
    );
    let rows: Vec<Vec<String>> = s
        .read("jobs/noop/state/runs.tsv")
        .lines()
        .map(|line| line.split('\t').map(str::to_string).collect())
        .collect();
    let row = s.row("noop");
    let parsed = row
        .get("runs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut window_ok = parsed.len() == 10;
    if window_ok {
        for (run, source) in parsed.iter().zip(rows[rows.len() - 10..].iter()) {
            window_ok &= run.get("start").and_then(Value::as_str) == Some(source[0].as_str())
                && run.get("end").and_then(Value::as_str) == Some(source[1].as_str())
                && run.get("rc").and_then(Value::as_i64) == source[2].parse::<i64>().ok()
                && run.get("verb").and_then(Value::as_str) == Some(source[3].as_str())
                && run.get("unit").and_then(Value::as_str) == Some(source[4].as_str())
                && run.get("seconds").and_then(Value::as_i64) == source[5].parse::<i64>().ok();
        }
        window_ok &= row.get("last_duration").and_then(Value::as_i64)
            == rows.last().and_then(|r| r[5].parse::<i64>().ok());
    }
    c.is_true("the last ten runs are parsed oldest first", window_ok);

    let page = s.run(&["report", "--html", "out/history.html"]);
    let _ = code(&page);
    let body = s.read("out/history.html");
    c.is_true(
        "the page names the duration",
        body.contains("<th>duration</th>"),
    );
    c.is_true(
        "the page names the last 5 runs",
        body.contains("<th>last 5 runs</th>"),
    );
    c.check(
        "the page renders five recent runs",
        5,
        body.matches("class='run'").count(),
    );
    c.finish(17);
}

#[test]
fn t18_resource_locks() {
    let s = Sandbox::new("t18");
    let mut c = Checks::new(18);

    s.reset_noop();
    s.mkdir("jobs/noop/state");
    s.write("jobs/noop/state/expect", "00:00\n");
    let mut holder = s.spawn_sleep("30");
    write_resource_lock(&s, holder.id(), s.now_epoch() + 60);
    let lock_before = s.read("state/resources/fixture.lock");
    let out = s.run(&["run", "noop"]);
    c.check("busy resource holds", 0, code(&out));
    c.check(
        "a resource hold does no work",
        1,
        if s.is_file(&format!("jobs/noop/state/done/{}", s.today())) {
            0
        } else {
            1
        },
    );
    c.check(
        "a held run leaves the other run's lock alone",
        lock_before,
        s.read("state/resources/fixture.lock"),
    );
    c.is_true(
        "the resource hold is reported",
        combined(&out).contains("held: resource fixture"),
    );
    c.check(
        "a resource hold is not a missed run",
        false,
        stale_of_noop(&s),
    );
    let _ = holder.kill();
    let _ = holder.wait();

    s.reset_noop();
    write_resource_lock(&s, 999999, s.now_epoch() - 60);
    let out = s.run(&["run", "noop"]);
    c.check("expired resource reclaimed", 0, code(&out));
    c.check(
        "a reclaimed resource runs the job",
        0,
        if s.is_file(&format!("jobs/noop/state/done/{}", s.today())) {
            0
        } else {
            1
        },
    );
    c.check(
        "a released resource leaves no lock",
        1,
        if s.exists("state/resources/fixture.lock") {
            0
        } else {
            1
        },
    );
    c.is_true(
        "the stale lock is named",
        combined(&out).contains("reclaimed"),
    );

    s.reset_noop();
    let mut killed = s.spawn_sleep("30");
    write_resource_lock(&s, killed.id(), s.now_epoch() + 3600);
    let _ = killed.kill();
    let _ = killed.wait();
    let out = s.run(&["run", "noop"]);
    c.check("a SIGKILLed holder is reclaimed", 0, code(&out));
    c.check(
        "a lock left by SIGKILL does not deadlock",
        0,
        if s.is_file(&format!("jobs/noop/state/done/{}", s.today())) {
            0
        } else {
            1
        },
    );
    c.check(
        "the dead holder's lock is removed",
        1,
        if s.exists("state/resources/fixture.lock") {
            0
        } else {
            1
        },
    );

    s.reset_noop();
    let mut waiter = s.spawn_sleep("30");
    write_resource_lock(&s, waiter.id(), s.now_epoch() + 60);
    let lock_path = s.path("state/resources/fixture.lock");
    let remover = thread::spawn(move || {
        thread::sleep(Duration::from_secs(2));
        let _ = fs::remove_file(lock_path);
    });
    let out = s.run_env(&["run", "noop"], &[("U_RESOURCE_WAIT", "10")]);
    c.check(
        "a waiting run takes the resource when it frees",
        0,
        code(&out),
    );
    c.check(
        "a waited-for resource runs the job",
        0,
        if s.is_file(&format!("jobs/noop/state/done/{}", s.today())) {
            0
        } else {
            1
        },
    );
    c.is_true(
        "the waiting run logs the acquisition",
        combined(&out).contains("resource fixture acquired"),
    );
    let _ = remover.join();
    let _ = waiter.kill();
    let _ = waiter.wait();

    s.reset_noop();
    s.run(&["plan", "noop"]);
    c.check(
        "plan takes no resource",
        1,
        if s.is_dir("state/resources") { 0 } else { 1 },
    );
    c.finish(18);
}

#[test]
fn t19_shots_gate() {
    let s = Sandbox::new("t19");
    let mut c = Checks::new(19);
    s.reset_noop();
    s.run(&["run", "noop"]);
    let hello = s.path("jobs/noop/out/hello.txt");
    let hello = hello.to_string_lossy().into_owned();
    let shots = s.path("shots");
    fs::create_dir_all(&shots).unwrap();
    fs::write(shots.join("empty.mp4"), "").unwrap();
    let shots = shots.to_string_lossy().into_owned();

    s.write(
        "shots.json",
        &format!(r#"[{{"shot":1,"query":"staff badge","file":"{hello}","approved":false}}]"#),
    );
    let out = shots_command(&s, "shots.json", Some("1"));
    c.check("an unapproved shot fails", 1, code(&out));
    let body = combined(&out);
    c.is_true(
        "the failure names the unapproved shot and its query",
        body.contains("shot 1") && body.contains("staff badge") && body.contains("not approved"),
    );

    s.write(
        "shots.json",
        &format!(r#"[{{"shot":1,"query":"staff badge","file":"{hello}","approved":true}}]"#),
    );
    c.check(
        "an approved shot with its file passes",
        0,
        code(&shots_command(&s, "shots.json", Some("1"))),
    );

    s.write(
        "shots.json",
        &format!(
            r#"[{{"shot":2,"query":"staff badge","file":"{shots}/nope.mp4","approved":true}}]"#
        ),
    );
    let out = shots_command(&s, "shots.json", Some("1"));
    c.check("an approved shot with no file fails", 1, code(&out));
    let body = combined(&out);
    c.is_true(
        "the failure names the missing file",
        body.contains("shot 2") && body.contains("missing"),
    );

    s.write(
        "shots.json",
        &format!(
            r#"[{{"shot":3,"query":"staff badge","file":"{shots}/empty.mp4","approved":true}}]"#
        ),
    );
    c.check(
        "an empty shot file fails",
        1,
        code(&shots_command(&s, "shots.json", Some("1"))),
    );

    s.write(
        "shots.json",
        &format!(
            r#"[{{"shot":1,"query":"staff badge","file":"{hello}","approved":true}},{{"shot":2,"query":"lobby door","file":"{hello}","approved":false}}]"#
        ),
    );
    let out = shots_command(&s, "shots.json", Some("2"));
    c.check("one unapproved shot fails a longer manifest", 1, code(&out));
    let body = combined(&out);
    c.is_true(
        "the message names the first unapproved shot",
        body.contains("shot 2"),
    );
    c.is_true(
        "the message leaves the approved shot out",
        !body.contains("shot 1"),
    );

    s.write(
        "shots.json",
        &format!(r#"[{{"shot":1,"query":"staff badge","file":"{hello}","approved":true}}]"#),
    );
    let out = shots_command(&s, "shots.json", Some("2"));
    c.check("a manifest short of --min fails", 1, code(&out));
    c.is_true(
        "the failure counts the shots",
        combined(&out).contains("1 shot, want at least 2"),
    );

    s.write(
        "shots.json",
        &format!(
            r#"[{{"shot":1,"query":"staff badge","file":"{hello}","approved":true}},{{"shot":2,"query":"lobby door","file":"{hello}","approved":true}}]"#
        ),
    );
    c.check(
        "two approved shots meet --min",
        0,
        code(&shots_command(&s, "shots.json", Some("2"))),
    );

    s.write("shots.json", "not json");
    c.check(
        "malformed manifest JSON fails",
        1,
        code(&shots_command(&s, "shots.json", Some("1"))),
    );
    c.check(
        "a missing manifest fails",
        1,
        code(&shots_command(&s, "shots/nope.json", Some("1"))),
    );
    s.write("shots.json", "[]");
    c.check(
        "an empty manifest fails",
        1,
        code(&shots_command(&s, "shots.json", None)),
    );
    s.write("shots.json", "{\"shot\":1}\n");
    c.check(
        "a manifest that is not a list fails",
        1,
        code(&shots_command(&s, "shots.json", None)),
    );
    c.finish(19);
}

#[test]
fn t20_memory_gate() {
    let s = Sandbox::new("t20");
    let mut c = Checks::new(20);
    let fake = s
        .path("tests/fake-pi-memory")
        .to_string_lossy()
        .into_owned();
    let home = s.path("memory-home").to_string_lossy().into_owned();
    fs::create_dir_all(&home).unwrap();
    let memory_log = s.path("memory.log");
    let memory_log_str = memory_log.to_string_lossy().into_owned();

    s.reset_noop();
    let failing = s.run_env(
        &["run", "noop"],
        &[
            ("HOME", &home),
            ("U_MEMORY", "1"),
            ("U_PI_MEMORY", &fake),
            ("FAKE_MEMORY_RC", "1"),
        ],
    );
    c.check("a failing memory pass blocks", 2, code(&failing));

    s.reset_noop();
    let passing = s.run_env(
        &["run", "noop"],
        &[
            ("HOME", &home),
            ("U_MEMORY", "1"),
            ("U_PI_MEMORY", &fake),
            ("FAKE_MEMORY_RC", "0"),
        ],
    );
    c.check("a passing memory pass completes", 0, code(&passing));

    s.reset_noop();
    let _ = fs::remove_file(&memory_log);
    let named = s.run_env(
        &["run", "noop"],
        &[
            ("HOME", &home),
            ("U_MEMORY", "1"),
            ("U_MEMORY_FILES", "notes/00-index.md"),
            ("U_PI_MEMORY", &fake),
            ("FAKE_MEMORY_RC", "0"),
            ("FAKE_MEMORY_LOG", &memory_log_str),
        ],
    );
    c.check(
        "a named note passes save, lint and receipt",
        0,
        code(&named),
    );
    let log = fs::read_to_string(&memory_log).unwrap_or_default();
    c.check(
        "the pass runs all three store scripts",
        3,
        log.lines().count(),
    );
    c.check(
        "save gets the named note",
        "qmd-mem save notes/00-index.md",
        log.lines().next().unwrap_or(""),
    );
    c.check(
        "lint runs deep",
        "lint --deep",
        log.lines().nth(1).unwrap_or(""),
    );
    c.check(
        "the duplicate scan runs after lint",
        "scan --from-index",
        log.lines().nth(2).unwrap_or(""),
    );

    let save = s.run_env(
        &["gate", "memory", "--files", "notes/00-index.md"],
        &[
            ("HOME", &home),
            ("U_PI_MEMORY", &fake),
            ("FAKE_MEMORY_RC", "0"),
            ("FAKE_MEMORY_FAIL", "save"),
        ],
    );
    c.check("a save failure fails the memory gate", 1, code(&save));
    c.is_true(
        "the gate names the failed save",
        combined(&save).contains("save notes/00-index.md failed"),
    );

    let lint = s.run_env(
        &["gate", "memory"],
        &[
            ("HOME", &home),
            ("U_PI_MEMORY", &fake),
            ("FAKE_MEMORY_RC", "0"),
            ("FAKE_MEMORY_FAIL", "lint"),
        ],
    );
    c.check("a lint failure fails the memory gate", 1, code(&lint));
    c.is_true(
        "the gate names the failed lint",
        combined(&lint).contains("lint failed"),
    );

    let receipt = s.run_env(
        &["gate", "memory"],
        &[
            ("HOME", &home),
            ("U_PI_MEMORY", &fake),
            ("FAKE_MEMORY_RC", "0"),
            ("FAKE_MEMORY_FAIL", "receipt"),
        ],
    );
    c.check("a receipt failure fails the memory gate", 1, code(&receipt));
    c.is_true(
        "the gate names the failed receipt",
        combined(&receipt).contains("receipt failed"),
    );

    let missing = s.run_env(
        &["gate", "memory"],
        &[
            ("HOME", &home),
            ("U_PI_MEMORY", &format!("{fake}/missing")),
            ("FAKE_MEMORY_RC", "0"),
        ],
    );
    c.check("a missing store fails closed", 1, code(&missing));
    c.finish(20);
}

#[test]
fn t21_agent_adapter() {
    let s = Sandbox::new("t21");
    let mut c = Checks::new(21);
    let calls = s.path("agent-calls.log");
    let calls_str = calls.to_string_lossy().into_owned();
    let prompt = format!("jobs/noop/state/prompt-{}.md", s.today());
    let marker = format!("{prompt}.done");
    let run_agent = |s: &Sandbox, extra: &[(&str, &str)]| {
        let mut vars = vec![
            ("NOOP_AGENT", "1"),
            ("U_RUNNER", "fake"),
            ("FAKE_AGENT_LOG", calls_str.as_str()),
        ];
        vars.extend_from_slice(extra);
        s.run_env(&["run", "noop"], &vars)
    };
    s.reset_noop();
    let _ = fs::remove_file(&calls);
    let out = run_agent(&s, &[]);
    c.check("a fake-runner agent step completes", 0, code(&out));
    c.check("agent invoked once", 1, agent_invocations(&s, &calls_str));
    c.check(
        "a passing runner writes the marker",
        0,
        if s.is_file(&marker) { 0 } else { 1 },
    );
    c.check(
        "the prompt is a rendered file",
        0,
        if s.path(&prompt)
            .metadata()
            .map(|m| m.len() > 0)
            .unwrap_or(false)
        {
            0
        } else {
            1
        },
    );
    c.check(
        "the agent's artifact reaches the gate",
        0,
        if s.is_file(&format!("{prompt}.out")) {
            0
        } else {
            1
        },
    );
    run_agent(&s, &[]);
    c.check(
        "a held rerun does not invoke the agent",
        1,
        agent_invocations(&s, &calls_str),
    );

    s.reset_noop();
    let _ = fs::remove_file(&calls);
    let blocked = run_agent(&s, &[("NOOP_BLOCK", "1")]);
    c.check("a later stage failing blocks the run", 2, code(&blocked));
    c.check(
        "the marker survives a later failure",
        0,
        if s.is_file(&marker) { 0 } else { 1 },
    );
    s.run(&["unblock", "noop"]);
    let rerun = run_agent(&s, &[]);
    c.check(
        "the rerun after a partial failure completes",
        0,
        code(&rerun),
    );
    c.check(
        "a rerun does not pay for the agent again",
        1,
        agent_invocations(&s, &calls_str),
    );

    s.reset_noop();
    let _ = fs::remove_file(&calls);
    let failed = run_agent(&s, &[("FAKE_AGENT_RC", "1")]);
    c.check("a failing runner fails the job", 1, code(&failed));
    c.check(
        "a failing runner was tried once",
        1,
        agent_invocations(&s, &calls_str),
    );
    c.check(
        "a failing runner writes no marker",
        1,
        if s.is_file(&marker) { 0 } else { 1 },
    );
    c.check(
        "a failing agent does not finish the unit",
        1,
        if s.is_file(&format!("jobs/noop/state/done/{}", s.today())) {
            0
        } else {
            1
        },
    );

    let direct = s.path("agent-direct");
    fs::create_dir_all(&direct).unwrap();
    fs::write(direct.join("prompt.md"), "prompt for the adapter\n").unwrap();
    let direct_str = direct.to_string_lossy().into_owned();
    let _ = fs::remove_file(&calls);
    let script = r#". "$REPO/lib/guards.sh"
u_agent "$1/prompt.md" adapter.done; a=$?
u_agent "$1/prompt.md" adapter.done; b=$?
printf "%s %s\n" "$a" "$b""#;
    let repo_str = s.root.to_string_lossy().into_owned();
    let direct_log = direct.join("agent.log").to_string_lossy().into_owned();
    let out = s.bash(
        script,
        &[
            ("REPO", repo_str.as_str()),
            ("STATE", &direct_str),
            ("LOG", direct_log.as_str()),
            ("DRY", "0"),
            ("U_RUNNER", "fake"),
            ("FAKE_AGENT_LOG", &calls_str),
        ],
        &[&direct_str],
    );
    c.check(
        "u_agent is once, then free",
        "0 0",
        stdout(&out).lines().last().unwrap_or(""),
    );
    c.check(
        "the second u_agent call does not invoke the runner",
        1,
        agent_invocations(&s, &calls_str),
    );
    c.check(
        "a bare marker is written under STATE",
        0,
        if direct.join("adapter.done").is_file() {
            0
        } else {
            1
        },
    );
    let script = r#". "$REPO/lib/guards.sh"
u_agent "$1/prompt.md" failed.done
printf "rc=%s\n" "$?""#;
    let out = s.bash(
        script,
        &[
            ("REPO", repo_str.as_str()),
            ("STATE", &direct_str),
            ("LOG", direct_log.as_str()),
            ("DRY", "0"),
            ("U_RUNNER", "fake"),
            ("FAKE_AGENT_RC", "1"),
        ],
        &[&direct_str],
    );
    c.check(
        "u_agent returns non-zero on runner failure",
        "rc=1",
        stdout(&out).lines().last().unwrap_or(""),
    );
    c.check(
        "a failed u_agent writes no marker",
        1,
        if direct.join("failed.done").is_file() {
            0
        } else {
            1
        },
    );

    let decoy = s.path("agent-decoy");
    fs::create_dir_all(&decoy).unwrap();
    for program in ["dsh", "claude", "codex", "fake-agent.sh"] {
        write_exec(
            &decoy.join(program),
            &format!(
                "#!/bin/sh\nprintf \"%s\\n\" \"$0\" >> \"{}/used\"\nexit 0\n",
                decoy.display()
            ),
        );
    }
    c.check(
        "the fake runner is a committed stub",
        0,
        if s.is_executable("tests/fake-agent.sh") {
            0
        } else {
            1
        },
    );
    s.reset_noop();
    let _ = fs::remove_file(&calls);
    let path = format!(
        "{}:{}",
        decoy.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = s.run_env(
        &["run", "noop"],
        &[
            ("NOOP_AGENT", "1"),
            ("U_RUNNER", "fake"),
            ("FAKE_AGENT_LOG", &calls_str),
            ("PATH", &path),
        ],
    );
    c.check("the fake runner does not change the verdict", 0, code(&out));
    c.check(
        "no runner reachable by PATH is called",
        1,
        if decoy.join("used").is_file() { 0 } else { 1 },
    );
    c.check(
        "the repository's stub is the one that ran",
        1,
        agent_invocations(&s, &calls_str),
    );

    s.reset_noop();
    let unknown = s.run_env(
        &["run", "noop"],
        &[("NOOP_AGENT", "1"), ("U_RUNNER", "not-a-runner")],
    );
    c.check("an unknown runner fails closed", 1, code(&unknown));
    c.check(
        "an unknown runner writes no marker",
        1,
        if s.is_file(&marker) { 0 } else { 1 },
    );

    s.reset_noop();
    let _ = fs::remove_file(&calls);
    let plan = s.run_env(
        &["plan", "noop"],
        &[
            ("NOOP_AGENT", "1"),
            ("U_RUNNER", "fake"),
            ("FAKE_AGENT_LOG", &calls_str),
        ],
    );
    c.check("plan with an agent stage exits 0", 0, code(&plan));
    c.check(
        "plan does not invoke the agent",
        0,
        agent_invocations(&s, &calls_str),
    );
    c.check(
        "plan renders no prompt",
        1,
        if s.is_file(&prompt) { 0 } else { 1 },
    );
    c.check(
        "plan writes no marker",
        1,
        if s.is_file(&marker) { 0 } else { 1 },
    );
    c.finish(21);
}

#[test]
fn t22_scaffolder() {
    let s = Sandbox::new("t22");
    let mut c = Checks::new(22);
    let spec = "spec/examples/digest.md";
    let job = "jobs/morning-digest";
    let bad = "scaffold-bad";
    s.mkdir(bad);
    for name in ["morning-digest", "no-budget", "no-gate-stage"] {
        let _ = fs::remove_dir_all(s.path(&format!("jobs/{name}")));
    }

    let out = s
        .external("bin/scaffold")
        .arg(spec)
        .output()
        .expect("scaffold");
    c.check("scaffold exits 0", 0, code(&out));
    c.check(
        "scaffold is executable",
        0,
        if s.is_executable("bin/scaffold") {
            0
        } else {
            1
        },
    );
    c.check(
        "job emitted",
        0,
        if s.is_file(&format!("{job}/job.sh")) {
            0
        } else {
            1
        },
    );
    c.check(
        "the spec is copied into the job",
        0,
        if s.is_file(&format!("{job}/spec.md")) {
            0
        } else {
            1
        },
    );
    c.check(
        "the scaffolded job is executable",
        0,
        if s.is_executable(&format!("{job}/job.sh")) {
            0
        } else {
            1
        },
    );
    c.check(
        "the copied spec matches the source",
        0,
        if s.read_bytes(spec) == s.read_bytes(&format!("{job}/spec.md")) {
            0
        } else {
            1
        },
    );
    c.check(
        "the job is named from the spec",
        "morning-digest",
        s.path(job)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    c.is_true(
        "the scaffolded job is listable",
        s.run(&["list"])
            .stdout
            .split(|b| *b == b'\n')
            .any(|line| String::from_utf8_lossy(line) == "morning-digest"),
    );

    let job_body = s.read(&format!("{job}/job.sh"));
    c.check(
        "one never gate per stage",
        4,
        job_body
            .lines()
            .filter(|line| *line == "u_gate never")
            .count(),
    );
    c.is_true(
        "the job calls a guard",
        job_body.lines().any(|line| line.starts_with("u_guard_")),
    );
    c.is_true(
        "the job ends with a memory pass",
        job_body
            .lines()
            .any(|line| line.starts_with("u_memory_pass")),
    );
    c.is_true(
        "the job marks the unit done",
        job_body.lines().any(|line| line == "u_done"),
    );

    let plan = s.run(&["plan", "morning-digest"]);
    c.check("plan exits 0 for a scaffolded job", 0, code(&plan));
    let plan_out = combined(&plan);
    for label in ["collect", "summarize", "write", "memory pass"] {
        c.is_true(
            &format!("plan reports the {label} gate as never"),
            plan_out.contains(label) && plan_out.contains("never"),
        );
    }

    let before = s.read_bytes(&format!("{job}/job.sh"));
    let again = s
        .external("bin/scaffold")
        .arg(spec)
        .output()
        .expect("scaffold");
    c.check("scaffold refuses to overwrite", 64, code(&again));
    c.check(
        "the existing job is untouched",
        String::from_utf8_lossy(&before).into_owned(),
        String::from_utf8_lossy(&s.read_bytes(&format!("{job}/job.sh"))).into_owned(),
    );
    c.check(
        "the existing spec copy is untouched too",
        0,
        if s.is_file(&format!("{job}/spec.md")) {
            0
        } else {
            1
        },
    );

    let digest = s.read(spec);
    let no_budget: String = digest
        .lines()
        .take_while(|line| *line != "budget")
        .map(|line| {
            if line == "# spec: morning-digest" {
                "# spec: no-budget".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    s.write(&format!("{bad}/no-budget.md"), &no_budget);
    let out = s
        .external("bin/scaffold")
        .arg(format!("{bad}/no-budget.md"))
        .output()
        .expect("scaffold");
    c.check("scaffold refuses a missing heading", 64, code(&out));
    c.check(
        "a spec with a missing heading writes no job",
        1,
        if s.exists("jobs/no-budget") { 0 } else { 1 },
    );
    c.is_true(
        "the refusal names the missing heading",
        combined(&out).contains("budget"),
    );

    let no_gate: String = digest
        .lines()
        .map(|line| {
            if line == "# spec: morning-digest" {
                "# spec: no-gate-stage".to_string()
            } else if let Some(at) = line.find("gate:") {
                line[..at].to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    s.write(&format!("{bad}/no-gate.md"), &no_gate);
    let out = s
        .external("bin/scaffold")
        .arg(format!("{bad}/no-gate.md"))
        .output()
        .expect("scaffold");
    c.check("scaffold refuses a stage without a gate", 64, code(&out));
    c.check(
        "a gate-less spec writes no job",
        1,
        if s.exists("jobs/no-gate-stage") { 0 } else { 1 },
    );
    c.is_true("the refusal names the missing gate", {
        let body = combined(&out);
        body.contains("no gate") || body.contains("gate:")
    });

    let missing = s
        .external("bin/scaffold")
        .arg(format!("{bad}/nope.md"))
        .output()
        .expect("scaffold");
    c.check(
        "scaffold refuses a spec that does not exist",
        64,
        code(&missing),
    );

    for name in ["morning-digest", "no-budget", "no-gate-stage"] {
        let _ = fs::remove_dir_all(s.path(&format!("jobs/{name}")));
    }
    c.is_true(
        "the test deletes the job it scaffolded",
        !s.run(&["list"])
            .stdout
            .split(|b| *b == b'\n')
            .any(|line| String::from_utf8_lossy(line) == "morning-digest"),
    );
    c.finish(22);
}

#[test]
fn t23_video_job() {
    let s = Sandbox::new("t23");
    let mut c = Checks::new(23);
    let video = s.path("video-repo");
    for dir in [
        "automation",
        "bin",
        "episodes",
        "lessons",
        "notes",
        "outbox",
        "pipeline",
    ] {
        fs::create_dir_all(video.join(dir)).unwrap();
    }
    let video_str = video.to_string_lossy().into_owned();
    fs::write(
        video.join("lessons/ledger.jsonl"),
        "{\"episode\": \"041\", \"title\": \"A Previous Episode\", \"principle\": \"old\", \"status\": \"built\", \"date\": \"2026-10-19\"}\n",
    )
    .unwrap();
    fs::write(video.join("outbox/041-master.mp4"), "").unwrap();
    fs::write(
        video.join("pipeline/topic-queue.txt"),
        "041|A Previous Episode|OBS. 041 — THE OLD ONE|calm|old\n042|Why Unfinished Things Stay With You|OBS. 042 — THE LOOSE THREAD|curious|the mind keeps what is unresolved in view\n043|A Later Episode|OBS. 043 — THE NEXT ONE|calm|next\n",
    )
    .unwrap();
    fs::write(
        video.join("automation/lesson-task.md"),
        "# Daily lesson — {{RUN_DATE}}\n\nEpisode **{{EP}}**. Queue line: `{{TOPIC_LINE}}`.\n",
    )
    .unwrap();
    fs::write(
        video.join("notes/60-log.md"),
        "# notes\n\n## [2026-10-19] build | № 041 \"A Previous Episode\" built\n",
    )
    .unwrap();
    write_exec(
        &video.join("pipeline/lesson-check.py"),
        "#!/usr/bin/env python3\nimport sys\nprint(\"LESSON-OK\")\nsys.exit(0)\n",
    );
    write_exec(
        &video.join("pipeline/lesson-split.py"),
        "#!/usr/bin/env python3\nimport pathlib, sys\nep = pathlib.Path(sys.argv[1])\n(ep / \"_scratch\").mkdir(parents=True, exist_ok=True)\n(ep / \"_scratch/meta.json\").write_text(\"{}\\n\")\n(ep / \"narration\").mkdir(parents=True, exist_ok=True)\n(ep / \"narration/tts.txt\").write_text(\"narration\\n\")\nprint(\"split ok\")\n",
    );
    write_exec(
        &video.join("pipeline/daily-build.sh"),
        "#!/bin/bash\nset -euo pipefail\nhere=$(cd \"$(dirname \"$0\")\" && pwd)\nprintf '%s\\n' \"$*\" >> \"$here/../calls.log\"\ncase \"${1:-}\" in\n  --preflight)\n    printf '[fixture] next episode: %s\\n' \"${FAKE_NEXT_EP:-042}\"\n    printf '[fixture] preflight OK (ep %s, beds 3)\\n' \"${FAKE_NEXT_EP:-042}\"\n    exit 0 ;;\n  *)\n    if [ \"${FAKE_DRIVER_HOLD:-0}\" = 1 ]; then\n      printf '[fixture] a master was built 3h ago - one per day, skipping\\n'\n      exit 0\n    fi\n    : > \"$here/../outbox/$1-master.mp4\"\n    printf '[fixture] built %s\\n' \"$1\"\n    exit 0 ;;\nesac\n",
    );
    write_exec(
        &video.join("bin/ffprobe"),
        "#!/bin/sh\nres=\"${FAKE_VIDEO_RES:-1080x1920}\"\nw=1080; h=1920\ncase \"$res\" in\n  *x*) w=${res%x*}; h=${res#*x} ;;\nesac\nprintf '{\"format\":{\"duration\":\"60.00\"},\"streams\":[{\"codec_type\":\"video\",\"width\":%s,\"height\":%s}]}\\n' \"$w\" \"$h\"\n",
    );
    write_exec(
        &video.join("bin/ffmpeg"),
        "#!/bin/sh\nprintf '[Parsed_ebur128_0] Summary:\\n  I: -14.0 LUFS\\n' >&2\nexit 0\n",
    );

    s.reset("everyday-laws");
    c.check(
        "the video spec is a copy of the example",
        0,
        if s.read_bytes("spec/examples/video.md") == s.read_bytes("jobs/everyday-laws/spec.md") {
            0
        } else {
            1
        },
    );
    c.is_true(
        "the job records that its first real run is supervised",
        s.read("jobs/everyday-laws/job.sh")
            .to_lowercase()
            .contains("first real run is supervised"),
    );

    let before = snapshot(&video);
    let plan = s.run_env(&["plan", "everyday-laws"], &[("VIDEO_REPO", &video_str)]);
    c.check("video plan exits 0", 0, code(&plan));
    c.is_true(
        "plan names the next episode",
        combined(&plan).contains("next episode: 042"),
    );
    let plan_out = combined(&plan);
    for want in [
        "lesson-check.py",
        "daily-build.sh 042",
        "duration",
        "resolution",
        "loudness",
        "--min 59",
        "--max 61",
        "--w 1080",
        "--h 1920",
        "--target -14",
        "--tol 1",
        "memory",
    ] {
        assert!(
            plan_out.contains(want),
            "plan omits the {want} gate: {plan_out}"
        );
    }
    c.check(
        "plan does not execute the driver",
        1,
        if video.join("calls.log").exists() {
            0
        } else {
            1
        },
    );
    c.check(
        "plan writes no state or output",
        0,
        s.count_files("jobs/everyday-laws/state") + s.count_files("jobs/everyday-laws/out"),
    );
    c.check(
        "plan writes no job log",
        0,
        s.count_files("jobs/everyday-laws/logs"),
    );
    c.check(
        "plan writes nothing in the driver's checkout",
        0,
        if before == snapshot(&video) { 0 } else { 1 },
    );

    fs::create_dir_all(video.join("episodes/043")).unwrap();
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(video.join("lessons/ledger.jsonl"))
        .unwrap()
        .write_all(b"{\"episode\": \"044\", \"title\": \"Posted Already\", \"principle\": \"x\", \"status\": \"posted\", \"date\": \"2026-10-19\"}\n")
        .unwrap();
    fs::write(video.join("outbox/044-master.mp4"), "").unwrap();
    let plan = s.run_env(&["plan", "everyday-laws"], &[("VIDEO_REPO", &video_str)]);
    c.is_true(
        "plan resumes the unfinished episode first",
        combined(&plan).contains("next episode: 043"),
    );

    let agent_log = s.path("video-agent.log");
    let agent_log_str = agent_log.to_string_lossy().into_owned();
    let fake_memory = s
        .path("tests/fake-pi-memory")
        .to_string_lossy()
        .into_owned();
    let agent_out = video
        .join("episodes/042/lesson-raw.md")
        .to_string_lossy()
        .into_owned();
    let video_path = format!(
        "{}:{}",
        video.join("bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let run_env = |s: &Sandbox, extra: &[(&str, &str)]| {
        let mut vars: Vec<(&str, &str)> = vec![
            ("PATH", video_path.as_str()),
            ("VIDEO_REPO", video_str.as_str()),
            ("U_RUNNER", "fake"),
            ("FAKE_AGENT_LOG", agent_log_str.as_str()),
            ("FAKE_AGENT_OUT", agent_out.as_str()),
            ("U_MEMORY", "1"),
            ("U_PI_MEMORY", fake_memory.as_str()),
        ];
        vars.extend_from_slice(extra);
        s.run_env(&["run", "everyday-laws"], &vars)
    };

    for leaf in ["state", "out", "logs"] {
        let _ = fs::remove_dir_all(s.path("jobs/everyday-laws").join(leaf));
    }
    let _ = fs::remove_file(video.join("calls.log"));
    for episode in ["041", "042", "044"] {
        let _ = fs::remove_file(video.join(format!("outbox/{episode}-master.mp4")));
    }
    let _ = fs::remove_file(&agent_log);
    let out = run_env(&s, &[]);
    c.check("the fixture video run exits 0", 0, code(&out));
    c.check(
        "the agent ran stage 1 once",
        1,
        s.count_lines(&agent_log_str),
    );
    let calls = fs::read_to_string(video.join("calls.log")).unwrap_or_default();
    c.check(
        "the driver was asked to preflight and build",
        2,
        calls.lines().count(),
    );
    c.check(
        "the preflight comes first",
        "--preflight",
        calls.lines().next().unwrap_or(""),
    );
    c.check(
        "the build gets the episode number",
        "042",
        calls.lines().nth(1).unwrap_or(""),
    );
    c.check(
        "the master is on disk",
        0,
        if video.join("outbox/042-master.mp4").is_file() {
            0
        } else {
            1
        },
    );
    c.check(
        "a passing master marks the day done",
        0,
        if s.is_file(&format!("jobs/everyday-laws/state/done/{}", s.today())) {
            0
        } else {
            1
        },
    );

    for leaf in ["state", "out", "logs"] {
        let _ = fs::remove_dir_all(s.path("jobs/everyday-laws").join(leaf));
    }
    let _ = fs::remove_file(video.join("calls.log"));
    let _ = fs::remove_file(video.join("outbox/042-master.mp4"));
    let out = run_env(&s, &[("FAKE_VIDEO_RES", "720x1280")]);
    c.check("a master that is not 1080x1920 blocks", 2, code(&out));
    c.check(
        "the bad master leaves a block",
        0,
        if s.is_file("jobs/everyday-laws/state/blocked") {
            0
        } else {
            1
        },
    );
    c.is_true(
        "the block names the resolution gate",
        s.read("jobs/everyday-laws/state/blocked")
            .contains("resolution"),
    );
    c.check(
        "a blocked master does not mark the day done",
        1,
        if s.is_file(&format!("jobs/everyday-laws/state/done/{}", s.today())) {
            0
        } else {
            1
        },
    );

    for leaf in ["state", "out", "logs"] {
        let _ = fs::remove_dir_all(s.path("jobs/everyday-laws").join(leaf));
    }
    let _ = fs::remove_file(video.join("calls.log"));
    let _ = fs::remove_file(video.join("outbox/042-master.mp4"));
    let out = run_env(&s, &[("FAKE_DRIVER_HOLD", "1")]);
    c.check("a driver hold exits 0", 0, code(&out));
    c.check(
        "a driver hold leaves no block",
        1,
        if s.is_file("jobs/everyday-laws/state/blocked") {
            0
        } else {
            1
        },
    );
    c.check(
        "a driver hold does not mark the day done",
        1,
        if s.is_file(&format!("jobs/everyday-laws/state/done/{}", s.today())) {
            0
        } else {
            1
        },
    );

    for leaf in ["state", "out", "logs"] {
        let _ = fs::remove_dir_all(s.path("jobs/everyday-laws").join(leaf));
    }
    let _ = fs::remove_file(video.join("calls.log"));
    fs::write(video.join("outbox/041-master.mp4"), "").unwrap();
    let out = run_env(&s, &[]);
    c.check("a same-day master holds", 0, code(&out));
    c.check(
        "the hold spends no attempt",
        0,
        s.count_files("jobs/everyday-laws/state/attempts"),
    );
    c.check(
        "the hold never starts the driver",
        1,
        if video.join("calls.log").exists() {
            0
        } else {
            1
        },
    );
    c.finish(23);
}
