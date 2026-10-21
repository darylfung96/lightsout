//! Tests/run.sh sections 1–12, ported check for check.
//!
//! Each function is one task section; `Checks::finish` asserts that the
//! section produced exactly the number of assertions the shell suite printed,
//! so a check cannot be dropped or duplicated silently.

mod common;

use common::*;
use serde_json::Value;

fn text(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

#[test]
fn t01_fresh_run_writes_done() {
    let s = Sandbox::new("t01");
    let mut c = Checks::new(1);
    s.reset_noop();
    let out = s.run(&["run", "noop"]);
    c.check("fresh run exits 0", 0, code(&out));
    c.check(
        "done marker written",
        0,
        if s.is_file(&format!("jobs/noop/state/done/{}", s.today())) {
            0
        } else {
            1
        },
    );
    c.check(
        "artifact written",
        0,
        if s.is_file("jobs/noop/out/hello.txt") {
            0
        } else {
            1
        },
    );
    c.finish(1);
}

#[test]
fn t02_same_day_holds() {
    let s = Sandbox::new("t02");
    let mut c = Checks::new(2);
    s.reset_noop();
    s.run(&["run", "noop"]);
    let out = s.run(&["run", "noop"]);
    c.check("same-day run exits 0", 0, code(&out));
    c.is_true("same-day run held", combined(&out).contains("held:"));
    c.finish(2);
}

#[test]
fn t03_failed_gate_blocks_and_notifies_once() {
    let s = Sandbox::new("t03");
    let mut c = Checks::new(3);
    s.reset_noop();
    let first = s.run_env(&["run", "noop"], &[("NOOP_BLOCK", "1")]);
    c.check("gate failure exits 2", 2, code(&first));
    c.check(
        "blocked marker written",
        0,
        if s.is_file("jobs/noop/state/blocked") {
            0
        } else {
            1
        },
    );
    let before = s.count_containing("logs/alerts.log", "simulated");
    let repeat = s.run(&["run", "noop"]);
    c.check("blocked repeat exits 2", 2, code(&repeat));
    c.is_true(
        "repeat reports the block",
        combined(&repeat).contains("blocked since"),
    );
    let after = s.count_containing("logs/alerts.log", "simulated");
    c.check("no second notification", before, after);
    c.finish(3);
}

#[test]
fn t04_unblock_permits_a_run() {
    let s = Sandbox::new("t04");
    let mut c = Checks::new(4);
    s.reset_noop();
    s.run_env(&["run", "noop"], &[("NOOP_BLOCK", "1")]);
    let unblock = s.run(&["unblock", "noop"]);
    c.check("unblock exits 0", 0, code(&unblock));
    c.check(
        "blocked marker cleared",
        1,
        if s.is_file("jobs/noop/state/blocked") {
            0
        } else {
            1
        },
    );
    s.reset_noop();
    let run = s.run(&["run", "noop"]);
    c.check("run after unblock exits 0", 0, code(&run));
    c.finish(4);
}

#[test]
fn t05_plan_touches_nothing() {
    let s = Sandbox::new("t05");
    let mut c = Checks::new(5);
    s.reset_noop();
    let before = s.count_files("jobs/noop/state") + s.count_files("jobs/noop/out");
    let plan = s.run(&["plan", "noop"]);
    c.check("plan exits 0", 0, code(&plan));
    let after = s.count_files("jobs/noop/state") + s.count_files("jobs/noop/out");
    c.check("plan wrote no files", before, after);
    c.finish(5);
}

#[test]
fn t06_status_and_report() {
    let s = Sandbox::new("t06");
    let mut c = Checks::new(6);
    let data = s.status_json();
    c.is_true(
        "status --json parses",
        data.as_array()
            .map(|rows| !rows.is_empty())
            .unwrap_or(false),
    );
    let report = s.run(&["report", "--html", "out/status.html"]);
    c.check("report renders html", 0, code(&report));
    c.finish(6);
}

#[test]
fn t07_blocked_surfaces_on_dashboard() {
    let s = Sandbox::new("t07");
    let mut c = Checks::new(7);
    s.reset_noop();
    s.run_env(&["run", "noop"], &[("NOOP_BLOCK", "1")]);
    let row = s.row("noop");
    c.is_true(
        "blocked job surfaces on the dashboard",
        text(&row, "state") == "blocked" && text(&row, "detail").contains("simulated"),
    );
    c.finish(7);
}

#[test]
fn t08_class_and_source() {
    let s = Sandbox::new("t08");
    let mut c = Checks::new(8);
    s.reset_noop();
    s.run_env(
        &["run", "noop", "--class", "retryable"],
        &[("NOOP_BLOCK", "1")],
    );
    let row = s.row("noop");
    c.check("block class is recorded", "retryable", text(&row, "class"));
    c.check(
        "block source defaults to unknown",
        "unknown",
        text(&row, "source"),
    );
    let raw = s.read("jobs/noop/state/blocked");
    c.check(
        "blocked is a four-field line",
        4,
        raw.trim_end().split('\t').count(),
    );
    c.is_true(
        "blocked line is <iso> CLASS SOURCE REASON",
        raw.contains("\tretryable\tunknown\tsimulated gate failure"),
    );
    let repeat = s.run(&["run", "noop"]);
    c.check("retryable repeat exits 2", 2, code(&repeat));
    let out = combined(&repeat);
    c.is_true(
        "retryable block names retry",
        out.contains("blocked since")
            && out.contains("[retryable]")
            && out.contains("bin/job retry noop"),
    );

    s.reset_noop();
    s.run_env(
        &[
            "run",
            "noop",
            "--class",
            "judgment",
            "--source",
            "gate:exists",
        ],
        &[("NOOP_BLOCK", "1")],
    );
    let row = s.row("noop");
    c.check(
        "class and source are recorded",
        "judgment gate:exists",
        format!("{} {}", text(&row, "class"), text(&row, "source")),
    );
    let repeat = s.run(&["run", "noop"]);
    c.check("judgment repeat exits 2", 2, code(&repeat));
    let out = combined(&repeat);
    c.is_true(
        "judgment block does not name retry",
        !(out.contains("[judgment]") && out.contains("bin/job retry")),
    );
    c.finish(8);
}

#[test]
fn t09_legacy_two_field_block() {
    let s = Sandbox::new("t09");
    let mut c = Checks::new(9);
    s.reset_noop();
    s.mkdir("jobs/noop/state");
    s.write(
        "jobs/noop/state/blocked",
        "2026-10-20T00:00:00-0500\tsimulated gate failure (NOOP_BLOCK=1)\n",
    );
    let row = s.row("noop");
    c.check(
        "two-field block reads as human",
        "human unknown simulated gate failure (NOOP_BLOCK=1)",
        format!(
            "{} {} {}",
            text(&row, "class"),
            text(&row, "source"),
            text(&row, "detail")
        ),
    );
    let run = s.run(&["run", "noop"]);
    c.check("legacy block still exits 2", 2, code(&run));
    let out = combined(&run);
    c.is_true(
        "human block names unblock",
        out.contains("[human]") && out.contains("bin/job unblock noop"),
    );
    c.finish(9);
}

#[test]
fn t10_diagnose_is_read_only() {
    let s = Sandbox::new("t10");
    let mut c = Checks::new(10);
    s.reset_noop();
    s.run_env(&["run", "noop"], &[("NOOP_BLOCK", "1")]);
    let log = format!("jobs/noop/logs/{}.log", s.today());
    s.append(
        &log,
        &format!(
            "[{}] FAIL exists: simulated gate failure\n",
            s.now_log_stamp()
        ),
    );
    let before = s.read_bytes("jobs/noop/state/blocked");
    let diagnose = s.run(&["diagnose", "noop"]);
    c.check("diagnose exits 0", 0, code(&diagnose));
    c.check(
        "diagnose leaves the block untouched",
        String::from_utf8_lossy(&before).into_owned(),
        String::from_utf8_lossy(&s.read_bytes("jobs/noop/state/blocked")).into_owned(),
    );
    let report = s.read("jobs/noop/state/diagnosis.md");
    c.check(
        "diagnosis written",
        0,
        if report.is_empty() { 1 } else { 0 },
    );
    for section in ["Cause candidates", "Evidence", "Next commands"] {
        c.is_true(
            &format!("diagnosis has a {section} section"),
            report
                .lines()
                .any(|line| line.starts_with(&format!("## {section}"))),
        );
    }
    c.is_true(
        "diagnosis quotes the last failing gate line",
        report.contains("FAIL exists: simulated gate failure"),
    );
    c.is_true(
        "diagnosis carries the log tail",
        report.contains("stage 1: write artifact"),
    );
    c.is_true(
        "human block names unblock",
        report.contains("bin/job unblock noop"),
    );
    let echoed = s.run(&["diagnose", "noop"]);
    c.check("diagnose prints and exits 0", 0, code(&echoed));
    c.is_true(
        "diagnosis printed on stdout",
        stdout(&echoed).contains("# Diagnosis"),
    );
    let run = s.run(&["run", "noop"]);
    c.check("diagnose does not clear the block", 2, code(&run));

    s.reset_noop();
    let clean = s.run(&["diagnose", "noop"]);
    c.check("diagnose works when not blocked", 0, code(&clean));
    let report = s.read("jobs/noop/state/diagnosis.md").to_lowercase();
    c.is_true(
        "unblocked diagnosis says so",
        report.contains("not blocked"),
    );
    c.finish(10);
}

#[test]
fn t11_retry_allowlist() {
    let s = Sandbox::new("t11");
    let mut c = Checks::new(11);
    let count_path = |s: &Sandbox| format!("jobs/noop/state/repairs/{}.count", s.today());

    // A retryable block and the fixture's registered repair.
    s.reset_noop();
    s.run_env(
        &["run", "noop"],
        &[("NOOP_BLOCK", "1"), ("NOOP_CLASS", "retryable")],
    );
    let retry = s.run(&["retry", "noop"]);
    c.check("retry runs after a retryable block", 0, code(&retry));
    c.check(
        "retry re-runs the job to done",
        0,
        if s.is_file(&format!("jobs/noop/state/done/{}", s.today())) {
            0
        } else {
            1
        },
    );
    c.check(
        "retry cleared the block on success",
        1,
        if s.is_file("jobs/noop/state/blocked") {
            0
        } else {
            1
        },
    );
    c.check("one repair counted", "1", s.read(&count_path(&s)).trim());
    c.is_true(
        "the repair is logged by name",
        s.read(&format!("jobs/noop/logs/{}.log", s.today()))
            .contains("retry: repair touch-missing"),
    );
    let last = s.last_line("jobs/noop/state/last");
    c.is_true(
        "retry is recorded as its own verb",
        field(&last, 1) == "0" && field(&last, 2) == "retry",
    );

    // A human block is refused.
    s.reset_noop();
    s.run_env(
        &["run", "noop"],
        &[("NOOP_BLOCK", "1"), ("NOOP_CLASS", "human")],
    );
    c.check(
        "retry refuses a human block",
        2,
        code(&s.run(&["retry", "noop"])),
    );
    c.check(
        "refused retry keeps the block",
        0,
        if s.is_file("jobs/noop/state/blocked") {
            0
        } else {
            1
        },
    );
    c.check(
        "refused retry runs no repair",
        1,
        if s.is_dir("jobs/noop/state/repairs") {
            0
        } else {
            1
        },
    );
    let refusal = s.run(&["retry", "noop"]);
    c.is_true(
        "refusal points at the diagnosis",
        combined(&refusal).contains("diagnosis.md"),
    );

    // A judgment block is refused too.
    s.reset_noop();
    s.run_env(
        &["run", "noop"],
        &[("NOOP_BLOCK", "1"), ("NOOP_CLASS", "judgment")],
    );
    c.check(
        "retry refuses a judgment block",
        2,
        code(&s.run(&["retry", "noop"])),
    );

    // An unknown repair is refused before anything is touched.
    s.reset_noop();
    s.run_env(
        &["run", "noop"],
        &[("NOOP_BLOCK", "1"), ("NOOP_CLASS", "retryable")],
    );
    let before = s.count_files("jobs/noop/state");
    let unknown = s.run(&["retry", "noop", "--repair", "does-not-exist"]);
    c.check("retry refuses an unknown repair", 2, code(&unknown));
    c.check(
        "unknown repair touches no state",
        before,
        s.count_files("jobs/noop/state"),
    );
    c.is_true(
        "unknown repair lists the allowlist",
        combined(&unknown).contains("touch-missing"),
    );

    // Nothing blocked is a held no-op.
    s.reset_noop();
    c.check(
        "retry with nothing blocked is a held no-op",
        0,
        code(&s.run(&["retry", "noop"])),
    );
    c.check(
        "no repair ran without a block",
        1,
        if s.is_dir("jobs/noop/state/repairs") {
            0
        } else {
            1
        },
    );

    // A repair cannot launder a bad artifact.
    s.reset_noop();
    s.run_env(
        &["run", "noop"],
        &[("NOOP_BLOCK", "1"), ("NOOP_CLASS", "retryable")],
    );
    let failing = s.run_env(
        &["retry", "noop"],
        &[("NOOP_BLOCK", "1"), ("NOOP_CLASS", "retryable")],
    );
    c.check("a repair cannot launder a bad artifact", 2, code(&failing));
    c.check(
        "a failing retry is blocked again",
        0,
        if s.is_file("jobs/noop/state/blocked") {
            0
        } else {
            1
        },
    );

    // Diagnose explains a retryable block and lists the registry.
    s.reset_noop();
    s.run_env(
        &["run", "noop", "--class", "retryable"],
        &[("NOOP_BLOCK", "1")],
    );
    c.check(
        "diagnose on a retryable block exits 0",
        0,
        code(&s.run(&["diagnose", "noop"])),
    );
    c.is_true(
        "diagnose lists the registered repair",
        s.read("jobs/noop/state/diagnosis.md")
            .contains("touch-missing"),
    );

    // MAX_REPAIRS caps the loop; the stage/day caps are lifted so only the
    // repair cap can stop it.
    s.reset_noop();
    s.run_env(
        &["run", "noop"],
        &[("NOOP_BLOCK", "1"), ("NOOP_CLASS", "retryable")],
    );
    let loop_env = [
        ("NOOP_BLOCK", "1"),
        ("NOOP_CLASS", "retryable"),
        ("MAX_ATTEMPTS", "99"),
        ("MAX_STAGE_ATTEMPTS", "99"),
        ("MAX_REPAIRS", "2"),
    ];
    c.check(
        "first retry under the cap runs",
        2,
        code(&s.run_env(&["retry", "noop"], &loop_env)),
    );
    c.check(
        "second retry under the cap runs",
        2,
        code(&s.run_env(&["retry", "noop"], &loop_env)),
    );
    let third = s.run_env(&["retry", "noop"], &loop_env);
    c.check("repair cap blocks the third retry", 2, code(&third));
    c.is_true(
        "repair cap is reported",
        combined(&third).contains("repair cap"),
    );
    c.check(
        "two repairs recorded today",
        "2",
        s.read(&count_path(&s)).trim(),
    );
    c.finish(11);
}

#[test]
fn t12_stage_attempt_cap() {
    let s = Sandbox::new("t12");
    let mut c = Checks::new(12);
    s.reset_noop();
    for _ in 0..3 {
        s.run_env(&["run", "noop"], &[("NOOP_FAIL_STAGE", "1")]);
    }
    let row = s.row("noop");
    c.is_true(
        "a failing stage blocks with a stage detail",
        text(&row, "state") == "blocked" && text(&row, "detail").contains("stage"),
    );

    s.reset_noop();
    for i in 0..3 {
        s.run_env(
            &["run", "noop"],
            &[("NOOP_FAIL_STAGE", "1"), ("MAX_ATTEMPTS", "99")],
        );
        if i != 2 {
            s.run(&["unblock", "noop"]);
        }
    }
    let row = s.row("noop");
    c.check(
        "third stage attempt is a human block",
        "human|stage 1 attempt cap reached (2 of 2 today)",
        format!("{}|{}", text(&row, "class"), text(&row, "detail")),
    );
    c.check(
        "only the stage's own attempts are counted",
        2,
        s.count_prefix(
            "jobs/noop/state/attempts",
            &format!("{}.1.attempt-", s.today()),
        ),
    );
    c.finish(12);
}
