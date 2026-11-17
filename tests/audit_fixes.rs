//! Regression tests for the audit of 2026-11-17: a charge that overflowed the
//! spend total, exit codes that were not the contract's three, a lock that two
//! ticks could both take and a held tick could delete, names that escaped
//! `jobs/`, damaged state files that failed open or read as "never run", a
//! dashboard that disagreed with `check`, and gates that passed on an empty
//! requirement.

mod common;

use common::*;
use serde_json::Value;

fn write_job(s: &Sandbox, name: &str, body: &str) {
    s.write(&format!("jobs/{name}/job.sh"), body);
}

fn job_json(s: &Sandbox, name: &str) -> Value {
    let out = s.run(&["status", "--json"]);
    let rows: Value = serde_json::from_slice(&out.stdout).expect("status --json parses");
    rows.as_array()
        .and_then(|rows| rows.iter().find(|row| row["name"] == name).cloned())
        .unwrap_or_else(|| panic!("no row for {name}: {}", combined(&out)))
}

fn arg(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn a_charge_near_i64_max_does_not_take_the_dashboard_down() {
    let s = Sandbox::new("af-spend");
    write_job(
        &s,
        "noop",
        r#"#!/usr/bin/env bash
set -euo pipefail
. "$REPO/lib/guards.sh"
u_spend 9223372036854775807 first
u_spend 1 second
u_done
"#,
    );
    let out = s.run(&["run", "noop"]);
    assert_eq!(code(&out), 0, "run: {}", combined(&out));
    let out = s.run(&["status"]);
    assert_eq!(code(&out), 0, "status must not panic: {}", combined(&out));
    let page = arg(&s.path("page.html"));
    let out = s.run(&["report", "--html", &page]);
    assert_eq!(code(&out), 0, "report: {}", combined(&out));
}

#[test]
fn run_reports_the_contract_code_not_the_child_code() {
    let s = Sandbox::new("af-codes");
    write_job(&s, "three", "#!/usr/bin/env bash\nexit 3\n");
    assert_eq!(
        code(&s.run(&["run", "three"])),
        1,
        "3 is not one of the three codes, so the run failed"
    );
    write_job(&s, "two", "#!/usr/bin/env bash\nexit 2\n");
    assert_eq!(
        code(&s.run(&["run", "two"])),
        1,
        "2 without state/blocked is a failure, not a block nothing recorded"
    );
    write_job(
        &s,
        "stopped",
        r#"#!/usr/bin/env bash
set -euo pipefail
. "$REPO/lib/guards.sh"
u_block --class human "nope"
"#,
    );
    assert_eq!(
        code(&s.run(&["run", "stopped"])),
        2,
        "a recorded block is 2"
    );
}

#[test]
fn a_held_tick_leaves_the_running_lock_alone() {
    let s = Sandbox::new("af-lock-held");
    write_job(
        &s,
        "slow",
        r#"#!/usr/bin/env bash
set -euo pipefail
. "$REPO/lib/guards.sh"
u_guard_lock
sleep 3
u_done
"#,
    );
    let mut winner = s.spawn_bash("bin/lightsout run slow", &[]);
    for _ in 0..80 {
        if s.is_dir("jobs/slow/state/lock") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        s.is_dir("jobs/slow/state/lock"),
        "the first run took the lock"
    );

    let out = s.run(&["run", "slow"]);
    assert_eq!(code(&out), 0, "a held tick exits 0: {}", combined(&out));
    assert!(
        stdout(&out).contains("held"),
        "the second tick says it was held: {}",
        combined(&out)
    );
    assert!(
        s.is_dir("jobs/slow/state/lock"),
        "the held tick must not delete the lock its owner is holding"
    );
    let _ = winner.wait();
    assert!(
        !s.is_dir("jobs/slow/state/lock"),
        "the owner releases the lock when it finishes"
    );
}

#[test]
fn two_simultaneous_runs_do_not_both_do_the_work() {
    let s = Sandbox::new("af-lock-race");
    write_job(
        &s,
        "race",
        r#"#!/usr/bin/env bash
set -euo pipefail
. "$REPO/lib/guards.sh"
u_guard_lock
mkdir -p "$JOB_DIR/out"
printf '%s\n' "$$" >> "$JOB_DIR/out/pids.txt"
sleep 2
u_done
"#,
    );
    let mut first = s.spawn_bash("bin/lightsout run race", &[]);
    let mut second = s.spawn_bash("bin/lightsout run race", &[]);
    let _ = first.wait();
    let _ = second.wait();
    let pids = s.read("jobs/race/out/pids.txt");
    assert_eq!(
        pids.lines().count(),
        1,
        "only one run may execute the body: {pids}"
    );
}

#[test]
fn a_job_name_with_a_slash_is_refused() {
    let s = Sandbox::new("af-name");
    let outside = s.path("../af-outside");
    let _ = std::fs::create_dir_all(&outside);
    std::fs::write(outside.join("job.sh"), "#!/usr/bin/env bash\necho RAN\n").unwrap();
    let out = s.run(&["run", "../../af-outside"]);
    assert_eq!(
        code(&out),
        64,
        "a name is one directory: {}",
        combined(&out)
    );
    assert!(
        !s.exists("../af-outside/state"),
        "nothing ran outside jobs/"
    );
    assert_eq!(code(&s.run(&["status", "../../af-outside"])), 64);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn a_damaged_expect_marker_blocks_the_check() {
    let s = Sandbox::new("af-expect");
    s.mkdir("jobs/noop/state/expect");
    let out = s.run(&["check", "noop"]);
    assert_eq!(
        code(&out),
        2,
        "an unreadable schedule must fail the check: {}",
        combined(&out)
    );
}

#[test]
fn a_truncated_last_line_still_names_the_run() {
    let s = Sandbox::new("af-last");
    s.write("jobs/noop/state/last", "2026-11-17T08:00:00\t0\n");
    let out = s.run(&["status", "noop"]);
    let said = stdout(&out);
    assert!(
        !said.contains("never run"),
        "two fields are a run without a verb: {said}"
    );
    assert!(
        said.contains("ran 2026-11-17"),
        "the row keeps its date: {said}"
    );
}

#[test]
fn a_blocked_directory_is_still_a_stop() {
    let s = Sandbox::new("af-blocked-dir");
    s.mkdir("jobs/noop/state/blocked");
    let out = s.run(&["status", "noop"]);
    assert!(stdout(&out).contains("blocked"), "{}", stdout(&out));

    let out = s.run(&["run", "noop"]);
    assert_eq!(
        code(&out),
        2,
        "a stopped job does not run: {}",
        combined(&out)
    );
    assert!(
        !s.exists("jobs/noop/out/hello.txt"),
        "the body never started"
    );

    let out = s.run(&["unblock", "noop"]);
    assert_eq!(code(&out), 0, "unblock: {}", combined(&out));
    assert!(!s.exists("jobs/noop/state/blocked"), "the marker is gone");
}

#[test]
fn a_missing_deliverable_is_stale_in_status_and_the_page() {
    let s = Sandbox::new("af-deliverable");
    write_job(
        &s,
        "holds",
        r#"#!/usr/bin/env bash
set -euo pipefail
. "$REPO/lib/guards.sh"
u_held "nothing to do"
"#,
    );
    s.mkdir("jobs/holds/state");
    s.write("jobs/holds/state/expect", "00:00\noutbox/*-master.mp4\n");
    s.run(&["run", "holds"]);

    let row = job_json(&s, "holds");
    assert_eq!(row["deliverable_ok"], Value::Bool(false), "row: {row}");
    assert_eq!(
        row["stale"],
        Value::Bool(true),
        "a hold cannot silence a missing deliverable: {row}"
    );

    let page = arg(&s.path("page.html"));
    s.run(&["report", "--html", &page]);
    let html = s.read("page.html");
    assert!(
        !html.contains("All quiet"),
        "the page must not call the day quiet while the deliverable is missing"
    );
}

#[test]
fn a_failure_after_done_today_still_reads_failed() {
    let s = Sandbox::new("af-failed-after-done");
    assert_eq!(code(&s.run(&["run", "noop"])), 0, "the first run is done");
    write_job(&s, "noop", "#!/usr/bin/env bash\nexit 1\n");
    assert_eq!(code(&s.run(&["run", "noop"])), 1, "the second run fails");
    let row = job_json(&s, "noop");
    assert_eq!(
        row["state"], "failed",
        "done today describes the unit, not the last run: {row}"
    );
}

#[test]
fn a_gate_with_an_empty_requirement_fails_closed() {
    let s = Sandbox::new("af-gates");
    s.write("artifact.txt", "hello\n");
    let file = arg(&s.path("artifact.txt"));
    assert_eq!(
        code(&s.run(&["gate", "contains", "--file", &file])),
        1,
        "an empty needle matches every file"
    );
    assert_eq!(
        code(&s.run(&["gate", "sections", "--file", &file])),
        1,
        "no heading named means nothing was checked"
    );
    assert_eq!(
        code(&s.run(&["gate", "contains", "--file", &file, "--text", "hello"])),
        0,
        "the gate still passes on a real needle"
    );
}

#[test]
fn a_cron_line_enters_the_workspace_first() {
    let s = Sandbox::new("af-cron");
    s.mkdir("jobs/noop/state");
    s.write("jobs/noop/state/expect", "02:30\n");
    let out = s.run(&["schedule", "noop"]);
    assert_eq!(code(&out), 0, "schedule: {}", combined(&out));
    let said = stdout(&out);
    assert!(
        said.contains("cd "),
        "cron starts in $HOME, so the line must cd: {said}"
    );
    assert!(said.contains("&& "), "the cd comes before the run: {said}");
}

#[test]
fn a_launchd_label_escapes_the_job_name() {
    let s = Sandbox::new("af-launchd");
    s.mkdir("jobs/a&b");
    write_job(&s, "a&b", "#!/usr/bin/env bash\nexit 0\n");
    let out = s.run(&["schedule", "a&b", "--launchd", "--at", "03:00"]);
    assert_eq!(code(&out), 0, "schedule: {}", combined(&out));
    let said = stdout(&out);
    assert!(
        said.contains("a&amp;b"),
        "the label is XML, not raw text: {said}"
    );
    assert!(
        !said.contains("com.lightsout.a&b"),
        "an unescaped name makes the plist unreadable: {said}"
    );
}

#[test]
fn a_new_unit_pays_for_its_own_artifact() {
    let s = Sandbox::new("af-unit");
    s.mkdir("jobs/noop/out");
    s.write(
        "jobs/noop/out/hello.txt",
        "hello 2026-11-16 2026-11-16T08:00:00-06:00\n",
    );
    let out = s.run(&["run", "noop"]);
    assert_eq!(code(&out), 0, "run: {}", combined(&out));
    let ledger = s.read("jobs/noop/state/ledger.tsv");
    assert!(
        ledger.contains("\t25\t"),
        "yesterday's artifact is not this unit's: {ledger}"
    );
}

#[test]
fn a_repair_scope_cannot_name_the_workspaces_lib() {
    let s = Sandbox::new("af-scope");
    let outside = s.path("../af-scope-job");
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(outside.join("state")).unwrap();
    std::fs::write(outside.join("job.sh"), "#!/usr/bin/env bash\nexit 0\n").unwrap();
    std::fs::write(
        outside.join("repair.allow"),
        format!("manager={}/lib\n", s.root.display()),
    )
    .unwrap();
    std::fs::write(
        outside.join("state/blocked"),
        "2026-11-17T00:00:00-0600\thuman\tgate\tboom\n",
    )
    .unwrap();
    s.write("triage.conf", "books * agent:propose\n");

    let jd = arg(&outside);
    let ws = arg(&s.root);
    let out = s.run(&["job", "add", "books", "--job-dir", &jd, "--workspace", &ws]);
    assert_eq!(code(&out), 0, "job add: {}", combined(&out));

    let proposal = s.path("proposal.json");
    std::fs::write(
        &proposal,
        r##"{"fixable":true,"summary":"clobber","edits":[{"path":"manager/guards.sh","content":"# clobbered\n"}]}"##,
    )
    .unwrap();
    let out = s.run_env(
        &["heal", "books"],
        &[
            ("U_RUNNER", "fake"),
            ("FAKE_AGENT_PROPOSAL", &proposal.to_string_lossy()),
        ],
    );
    assert_eq!(
        code(&out),
        2,
        "a scope inside the workspace is refused: {}",
        combined(&out)
    );
    assert!(
        combined(&out).contains("inside the workspace"),
        "the refusal names the reason: {}",
        combined(&out)
    );
    // The guard file a repair must never reach, whatever the allow list says.
    let guards = s.read("lib/guards.sh");
    assert!(
        guards.contains("u_done()"),
        "the manager's own lib/ is untouched: {guards}"
    );
    let _ = std::fs::remove_dir_all(&outside);
}
