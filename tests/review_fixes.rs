//! Regression tests for the review fixes: repair-path containment, block and
//! lock handling, per-day caps, status truthfulness, resource locks, and the
//! read-only plan contract.

mod common;

use std::fs;
use std::process::Output;

use common::*;

fn write_job(s: &Sandbox, name: &str, body: &str) {
    s.write(&format!("jobs/{name}/job.sh"), body);
}

fn block_ready(s: &Sandbox) {
    write_job(
        s,
        "ready",
        r#"#!/usr/bin/env bash
set -euo pipefail
. "$REPO/lib/guards.sh"
u_block --class judgment --source gate:ready "ready marker missing"
"#,
    );
    let out = s.run(&["run", "ready"]);
    assert_eq!(code(&out), 2, "block ready: {}", combined(&out));
}

fn fake_proposal(s: &Sandbox, body: &str) -> String {
    s.write("proposal-input.json", body);
    s.path("proposal-input.json").to_string_lossy().into_owned()
}

fn run_heal(s: &Sandbox, proposal: &str) -> Output {
    s.run_env(
        &["heal", "ready"],
        &[("U_RUNNER", "fake"), ("FAKE_AGENT_PROPOSAL", proposal)],
    )
}

#[test]
fn a_control_character_path_is_refused_before_apply() {
    let s = Sandbox::new("rf-control");
    block_ready(&s);
    s.write("triage.conf", "ready * agent:propose\n");
    s.write("jobs/ready/victim.txt", "keep me\n");
    let proposal = fake_proposal(
        &s,
        r#"{"fixable":true,"summary":"bad","edits":[{"path":"out/a\nvictim.txt","content":"x"}]}"#,
    );

    let out = run_heal(&s, &proposal);
    assert_eq!(code(&out), 2, "control path is refused: {}", combined(&out));
    let history = s.read("jobs/ready/state/heal/history.tsv");
    assert!(
        history.contains("invalid:path"),
        "refusal is recorded: {history}"
    );
    assert!(
        s.exists("jobs/ready/victim.txt"),
        "the injected path cannot name a real file"
    );
}

#[test]
fn revert_refuses_control_characters_and_paths_outside_the_allowlist() {
    let s = Sandbox::new("rf-revert-path");
    write_job(
        &s,
        "ready",
        "#!/usr/bin/env bash\n. \"$REPO/lib/guards.sh\"\nu_done\n",
    );
    s.write("jobs/ready/victim.txt", "keep me\n");
    let attempt = format!("jobs/ready/state/heal/{}/attempt-1", s.today());
    s.write(
        &format!("{attempt}/applied.tsv"),
        "\"out/a\\nvictim.txt\"\tcreated\n",
    );
    let out = s.run(&["heal", "ready", "--revert", "1"]);
    assert_eq!(code(&out), 2, "control path is refused: {}", combined(&out));
    assert!(s.exists("jobs/ready/victim.txt"), "victim survives");

    s.write(
        &format!("{attempt}/applied.tsv"),
        "\"victim.txt\"\tcreated\n",
    );
    let out = s.run(&["heal", "ready", "--revert", "1"]);
    assert_eq!(
        code(&out),
        2,
        "outside-allow path is refused: {}",
        combined(&out)
    );
    assert!(s.exists("jobs/ready/victim.txt"), "victim still survives");
}

#[test]
fn the_tripwire_sees_another_jobs_code() {
    let s = Sandbox::new("rf-tripwire");
    block_ready(&s);
    write_job(&s, "other", "#!/usr/bin/env bash\nexit 0\n");
    s.write("triage.conf", "ready * agent:apply\n");
    let proposal = fake_proposal(
        &s,
        r#"{"fixable":true,"summary":"fix","edits":[{"path":"out/ready","content":"ok"}]}"#,
    );
    let other = s.path("jobs/other/job.sh").to_string_lossy().into_owned();

    let out = s.run_env(
        &["heal", "ready"],
        &[
            ("U_RUNNER", "fake"),
            ("FAKE_AGENT_PROPOSAL", &proposal),
            ("FAKE_AGENT_TOUCH", &other),
        ],
    );
    assert_eq!(code(&out), 2, "tamper is refused: {}", combined(&out));
    let history = s.read("jobs/ready/state/heal/history.tsv");
    assert!(
        history.contains("violation"),
        "violation recorded: {history}"
    );
    assert!(!s.exists("jobs/ready/out/ready"), "nothing is applied");
    assert!(
        s.read("jobs/other/job.sh").contains("tampered"),
        "the other job was touched, so the detection is meaningful"
    );
}

#[test]
fn an_inherited_retrying_variable_does_not_bypass_a_block() {
    let s = Sandbox::new("rf-retrying");
    write_job(
        &s,
        "ready",
        "#!/usr/bin/env bash\nprintf ran > \"$JOB_DIR/ran\"\n",
    );
    s.write(
        "jobs/ready/state/blocked",
        &format!("{}\thuman\tmanual\tblocked\n", s.now_log_stamp()),
    );
    let out = s.run_env(&["run", "ready"], &[("U_RETRYING", "1")]);
    assert_eq!(code(&out), 2, "block still refuses: {}", combined(&out));
    assert!(!s.exists("jobs/ready/ran"), "the job did not run");
}

#[test]
fn everyday_laws_records_a_missing_video_repo_as_a_block() {
    let s = Sandbox::new("rf-video-config");
    let missing = s.path("no-such-video-repo");
    let missing = missing.to_string_lossy().into_owned();
    // The fixture's agent.conf points at the real video checkout; override the
    // runner so this test reaches the job's own config block first.
    let out = s.run_env(
        &["run", "everyday-laws"],
        &[("VIDEO_REPO", &missing), ("U_RUNNER", "fake")],
    );
    assert_eq!(code(&out), 2, "missing repo exits 2: {}", combined(&out));
    assert!(
        s.is_file("jobs/everyday-laws/state/blocked"),
        "the stop is recorded"
    );
    assert!(
        s.read("jobs/everyday-laws/state/blocked")
            .contains("VIDEO_REPO"),
        "the block names the cause"
    );
}

#[test]
fn a_dead_lock_does_not_hide_a_missed_run() {
    let s = Sandbox::new("rf-stale-lock");
    s.reset_noop();
    s.mkdir("jobs/noop/state/lock");
    s.write("jobs/noop/state/lock/pid", "999999\n");
    s.write("jobs/noop/state/expect", "00:00\n");

    let row = s.row("noop");
    assert_ne!(
        row.get("state").and_then(|v| v.as_str()),
        Some("running"),
        "a dead pid is not a running job: {row}"
    );
    assert_eq!(
        row.get("stale").and_then(|v| v.as_bool()),
        Some(true),
        "the missed run is visible: {row}"
    );

    let out = s.run(&["check"]);
    assert_eq!(code(&out), 0, "check exits 0 for a stale job");
    assert!(
        s.is_file(&format!("jobs/noop/state/alerts/{}.stale", s.today())),
        "the stale alarm is raised"
    );
}

#[test]
fn the_job_heal_cap_is_per_day() {
    let s = Sandbox::new("rf-heal-day");
    write_job(
        &s,
        "ready",
        "#!/usr/bin/env bash\n. \"$REPO/lib/guards.sh\"\nu_done\n",
    );
    let yesterday = "2026-10-20T09:00:00-0500";
    let today = format!("{}T09:00:00-0500", s.today());
    s.write(
        "jobs/ready/state/heal/history.tsv",
        &format!(
            "{yesterday}\tready\t1\tagent:apply\tgate:a\tgate:b\tpassed\t0\t1\t\tfake\n\
             {today}\tready\t2\tagent:apply\tgate:a\tgate:b\tpassed\t0\t1\t\tfake\n"
        ),
    );
    let row = s.row("ready");
    assert_eq!(
        row.get("heal_attempts_today").and_then(|v| v.as_u64()),
        Some(1),
        "only today's agent call counts: {row}"
    );
}

#[test]
fn an_invalid_attempt_cap_fails_closed() {
    let s = Sandbox::new("rf-attempt-cap");
    let out = s.run_env(&["run", "noop"], &[("MAX_ATTEMPTS", "two")]);
    assert_eq!(
        code(&out),
        1,
        "invalid cap is a failure: {}",
        combined(&out)
    );
    assert!(
        combined(&out).contains("MAX_ATTEMPTS"),
        "the bad setting is named"
    );
    assert_eq!(
        s.count_prefix(
            "jobs/noop/state/attempts",
            &format!("{}.attempt-", s.today())
        ),
        0,
        "no attempt was taken"
    );
}

#[cfg(unix)]
#[test]
fn unblock_does_not_claim_success_when_the_marker_stays() {
    use std::os::unix::fs::PermissionsExt;
    let s = Sandbox::new("rf-unblock");
    write_job(
        &s,
        "ready",
        "#!/usr/bin/env bash\n. \"$REPO/lib/guards.sh\"\nu_done\n",
    );
    s.write(
        "jobs/ready/state/blocked",
        &format!("{}\thuman\tmanual\tblocked\n", s.now_log_stamp()),
    );
    let state = s.path("jobs/ready/state");
    fs::set_permissions(&state, fs::Permissions::from_mode(0o500)).unwrap();
    let out = s.run(&["unblock", "ready"]);
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(code(&out), 2, "failed removal exits 2: {}", combined(&out));
    assert!(s.is_file("jobs/ready/state/blocked"), "block remains");
}

#[test]
fn a_malformed_expect_file_fails_the_check() {
    let s = Sandbox::new("rf-expect");
    s.reset_noop();
    s.write("jobs/noop/state/expect", "6am\n");
    let out = s.run(&["check"]);
    assert_eq!(code(&out), 2, "bad schedule exits 2: {}", combined(&out));
    assert!(
        stderr(&out).contains("not HH:MM"),
        "the bad schedule is named: {}",
        stderr(&out)
    );
    assert!(
        !s.exists(&format!("jobs/noop/state/alerts/{}.stale", s.today())),
        "no false stale marker"
    );
}

#[test]
fn status_calls_a_failed_run_failed_and_rejects_an_unknown_name() {
    let s = Sandbox::new("rf-status");
    write_job(&s, "bad", "#!/usr/bin/env bash\nexit 1\n");
    let out = s.run(&["run", "bad"]);
    assert_eq!(code(&out), 1);
    let row = s.row("bad");
    assert_eq!(
        row.get("state").and_then(|v| v.as_str()),
        Some("failed"),
        "a failed run is not never-run: {row}"
    );
    assert_eq!(row.get("last_exit").and_then(|v| v.as_i64()), Some(1));

    let out = s.run(&["status", "ghost"]);
    assert_eq!(code(&out), 64, "unknown name is a usage error");
    assert!(stderr(&out).contains("no such job"), "the typo is named");
}

#[test]
fn a_run_from_an_earlier_unit_is_not_never_run() {
    let s = Sandbox::new("rf-ran-state");
    s.reset_noop();
    assert_eq!(code(&s.run(&["run", "noop"])), 0);
    // Next midnight: `done` is keyed to today's unit, so yesterday's marker no
    // longer explains the row while the history still records the run.
    let _ = fs::remove_file(s.path(&format!("jobs/noop/state/done/{}", s.today())));

    let row = s.row("noop");
    let want = format!("ran {}", s.today());
    assert_eq!(
        row.get("state").and_then(|v| v.as_str()),
        Some(want.as_str()),
        "a completed run is not never-run: {row}"
    );
    assert_eq!(row.get("last_exit").and_then(|v| v.as_i64()), Some(0));
    assert_eq!(row.get("since"), row.get("last_run"), "since names the run");
    assert!(
        row.get("last_duration").and_then(|v| v.as_i64()).is_some(),
        "the run still has a duration: {row}"
    );

    let page = s.run(&["report", "--html", "out/state.html"]);
    assert_eq!(code(&page), 0, "report: {}", combined(&page));
    let body = s.read("out/state.html");
    assert!(
        body.contains(&format!("<td>{want}</td>")),
        "the page says the same thing: {body}"
    );
    assert!(
        body.contains("<tr class='ran'>"),
        "the ran row is still styled: {body}"
    );
}

#[test]
fn status_separates_todays_attempts_from_the_total() {
    let s = Sandbox::new("rf-attempt-total");
    s.reset_noop();
    s.write("jobs/noop/state/attempts/2026-10-01.attempt-1", "");
    s.write("jobs/noop/state/attempts/2026-10-01.attempt-2", "");
    s.write(
        &format!("jobs/noop/state/attempts/{}.attempt-1", s.today()),
        "",
    );
    // A stage's counter is its own and must not inflate the job's.
    s.write(
        &format!("jobs/noop/state/attempts/{}.write.attempt-1", s.today()),
        "",
    );

    let row = s.row("noop");
    assert_eq!(
        row.get("attempts_today").and_then(|v| v.as_i64()),
        Some(1),
        "only today's job attempt counts: {row}"
    );
    assert_eq!(
        row.get("attempts_total").and_then(|v| v.as_i64()),
        Some(3),
        "the total survives midnight: {row}"
    );
}

#[test]
fn a_hold_does_not_hide_the_last_work_duration() {
    let s = Sandbox::new("rf-work-duration");
    s.reset_noop();
    let today = s.today();
    // The work row's start is older than the done marker's mtime and the hold
    // row's is newer, so the newest row reads as a hold and the one before it
    // as the work it already did.
    s.write(
        "jobs/noop/state/runs.tsv",
        &format!(
            "2019-12-31T00:00:00+0000\t2019-12-31T00:00:05+0000\t0\trun\t{today}\t5\n\
             2020-01-03T00:00:00+0000\t2020-01-03T00:00:00+0000\t0\trun\t{today}\t0\n"
        ),
    );
    s.write(
        "jobs/noop/state/last",
        &format!("{today}T00:00:00+0000\t0\trun\n"),
    );
    s.touch(&format!("jobs/noop/state/done/{today}"));
    let touched = s
        .external("touch")
        .arg("-t")
        .arg("202001011200")
        .arg(s.path(&format!("jobs/noop/state/done/{today}")))
        .status()
        .expect("touch");
    assert!(touched.success(), "the done marker keeps its old mtime");

    let row = s.row("noop");
    assert_eq!(
        row.get("last_duration").and_then(|v| v.as_i64()),
        Some(0),
        "the newest run is the hold: {row}"
    );
    assert_eq!(
        row.get("last_work_duration").and_then(|v| v.as_i64()),
        Some(5),
        "the work that preceded it keeps its time: {row}"
    );

    let page = s.run(&["report", "--html", "out/duration.html"]);
    assert_eq!(code(&page), 0, "report: {}", combined(&page));
    assert!(
        s.read("out/duration.html").contains("(work 5s)"),
        "the page names the work time beside the hold"
    );
}

#[test]
fn status_detail_answers_what_a_job_is_and_when_it_runs() {
    let s = Sandbox::new("rf-status-detail");
    write_job(
        &s,
        "brief",
        "#!/usr/bin/env bash\nset -euo pipefail\n. \"$REPO/lib/guards.sh\"\n\
         u_say \"stage 1: gather\"\nu_done\n",
    );
    s.write(
        "jobs/brief/spec.md",
        r#"# brief — spec

deliverable
: A one-page brief every morning.

cadence
: Daily at 07:30.

stages
: 1. gather — read the queue
  2. write — render the page
"#,
    );
    s.write("jobs/brief/state/expect", "07:30\n");
    assert_eq!(code(&s.run(&["run", "brief"])), 0);

    let out = s.run(&["status", "brief"]);
    assert_eq!(code(&out), 0, "{}", combined(&out));
    let text = String::from_utf8_lossy(&out.stdout);
    for needle in [
        "brief — done today",
        "A one-page brief every morning.",
        "Daily at 07:30.",
        "07:30 (state/expect) — cron 30 7 * * *",
        "what it does",
        "1. gather",
        "2. write",
        "last runs",
        "lightsout plan brief",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }

    let all = s.run(&["status", "--detail"]);
    assert_eq!(code(&all), 0, "{}", combined(&all));
    assert!(
        String::from_utf8_lossy(&all.stdout).contains("brief — done today"),
        "every job gets a block"
    );
}

#[test]
fn plan_does_not_run_the_memory_pass() {
    let s = Sandbox::new("rf-plan-memory");
    let log = s.path("memory.log");
    let log = log.to_string_lossy().into_owned();
    let out = s.run_env(
        &["plan", "noop"],
        &[
            ("U_MEMORY", "1"),
            ("U_PI_MEMORY", "tests/fake-pi-memory"),
            ("FAKE_MEMORY_LOG", &log),
        ],
    );
    assert_eq!(code(&out), 0, "plan exits 0: {}", combined(&out));
    assert!(
        combined(&out).contains("would run the memory pass"),
        "plan says what it would do: {}",
        combined(&out)
    );
    assert!(!s.exists("memory.log"), "plan wrote no store receipt");
}

#[test]
fn heal_apply_refuses_a_proposal_for_a_different_block() {
    let s = Sandbox::new("rf-stale-proposal");
    block_ready(&s);
    s.write("triage.conf", "ready * agent:propose\n");
    let proposal = fake_proposal(
        &s,
        r#"{"fixable":true,"summary":"fix","edits":[{"path":"out/ready","content":"ok"}]}"#,
    );
    assert_eq!(code(&run_heal(&s, &proposal)), 2);

    s.write(
        "jobs/ready/state/blocked",
        &format!(
            "{}\tjudgment\tgate:other\tdifferent failure\n",
            s.now_log_stamp()
        ),
    );
    let out = s.run(&["heal", "ready", "--apply"]);
    assert_eq!(
        code(&out),
        2,
        "stale proposal is refused: {}",
        combined(&out)
    );
    assert!(
        combined(&out).contains("stale proposal"),
        "the refusal explains itself: {}",
        combined(&out)
    );
    assert!(!s.exists("jobs/ready/out/ready"), "nothing is applied");
}

#[test]
fn the_html_report_escapes_a_block_reason() {
    let s = Sandbox::new("rf-html");
    write_job(
        &s,
        "ready",
        "#!/usr/bin/env bash\n. \"$REPO/lib/guards.sh\"\nu_done\n",
    );
    s.write(
        "jobs/ready/state/blocked",
        &format!(
            "{}\tjudgment\tmanual\t<script>alert(1)</script>\n",
            s.now_log_stamp()
        ),
    );
    let out = s.run(&["report", "--html", "out/status.html"]);
    assert_eq!(code(&out), 0, "report exits 0: {}", combined(&out));
    let page = s.read("out/status.html");
    assert!(page.contains("&lt;script&gt;"), "reason is escaped");
    assert!(
        !page.contains("<script>alert(1)</script>"),
        "raw markup is absent"
    );
}

#[test]
fn both_jobs_contend_for_the_same_workspace_resource() {
    let s = Sandbox::new("rf-resource");
    for name in ["left", "right"] {
        write_job(
            &s,
            name,
            "#!/usr/bin/env bash\nset -euo pipefail\n. \"$REPO/lib/guards.sh\"\nu_resource shared\nu_done\n",
        );
    }
    s.mkdir("state/resources/shared.lock");
    s.write(
        "state/resources/shared.lock/pid",
        &format!("{}\n", std::process::id()),
    );
    s.write(
        "state/resources/shared.lock/expiry",
        &format!("{}\n", s.now_epoch() + 60),
    );
    for name in ["left", "right"] {
        let out = s.run(&["run", name]);
        assert_eq!(
            code(&out),
            0,
            "{name} is held, not failed: {}",
            combined(&out)
        );
        assert!(
            !s.exists(&format!("jobs/{name}/state/done/{}", s.today())),
            "{name} did no work while the resource was busy"
        );
    }
}

#[test]
fn an_agent_timeout_returns_124_and_writes_no_marker() {
    let s = Sandbox::new("rf-agent-timeout");
    let dir = s.path("agent-timeout");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("prompt.md"), "prompt for the timeout test\n").unwrap();
    let dir_str = dir.to_string_lossy().into_owned();
    let repo_str = s.root.to_string_lossy().into_owned();
    let log = dir.join("agent.log").to_string_lossy().into_owned();
    // The script redirects its own output: the runner's child would otherwise
    // hold the test's pipe open after the kill and hide whether the deadline
    // actually ended the call.
    let script = r#". "$REPO/lib/guards.sh"
exec >"$1/out.log" 2>&1
u_agent "$1/prompt.md" slow.done
printf "agent=%s\n" "$?"
u_bounded 1 "$1/bounded.marker" sleep 5
printf "bounded=%s marker=%s\n" "$?" "$([ -e "$1/bounded.marker" ] && echo present || echo gone)"
"#;
    let started = std::time::Instant::now();
    let out = s.bash(
        script,
        &[
            ("REPO", repo_str.as_str()),
            ("STATE", &dir_str),
            ("LOG", &log),
            ("DRY", "0"),
            ("U_RUNNER", "fake"),
            ("U_AGENT_TIMEOUT", "1"),
            ("FAKE_AGENT_SLEEP", "5"),
        ],
        &[&dir_str],
    );
    let elapsed = started.elapsed();
    assert_eq!(
        code(&out),
        0,
        "the harness script finishes: {}",
        combined(&out)
    );
    let text = fs::read_to_string(dir.join("out.log")).unwrap_or_default();
    assert!(
        text.lines().any(|line| line == "agent=124"),
        "the timed-out agent reports 124: {text}"
    );
    assert!(
        text.lines().any(|line| line == "bounded=124 marker=gone"),
        "u_bounded reports 124 and clears its marker: {text}"
    );
    assert!(
        !dir.join("slow.done").is_file(),
        "a timed-out agent writes no marker"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "the deadline ends the call, not the test: {elapsed:?}"
    );
}

#[test]
fn revert_refuses_an_older_attempt_after_a_newer_one_was_applied() {
    let s = Sandbox::new("rf-revert-order");
    write_job(
        &s,
        "ready",
        "#!/usr/bin/env bash\n. \"$REPO/lib/guards.sh\"\nu_done\n",
    );
    s.write("jobs/ready/out/artifact.txt", "current\n");
    let base = format!("jobs/ready/state/heal/{}", s.today());
    s.write(
        &format!("{base}/attempt-1/backup/out/artifact.txt"),
        "old\n",
    );
    s.write(
        &format!("{base}/attempt-1/applied.tsv"),
        "\"out/artifact.txt\"\tmodified\n",
    );
    s.write(
        &format!("{base}/attempt-2/applied.tsv"),
        "\"out/artifact.txt\"\tmodified\n",
    );

    let out = s.run(&["heal", "ready", "--revert", "1"]);
    assert_eq!(
        code(&out),
        2,
        "an older revert is refused: {}",
        combined(&out)
    );
    assert!(
        stderr(&out).contains("attempt-2"),
        "the newer attempt is named: {}",
        stderr(&out)
    );
    assert_eq!(
        s.read("jobs/ready/out/artifact.txt"),
        "current\n",
        "nothing is restored over the newer edit"
    );
}

#[test]
fn a_failed_init_removes_the_files_it_created() {
    let s = Sandbox::new("rf-init-fail");
    // A plain file where init needs the jobs directory makes the ninth template
    // fail, after the first eight files were already written.
    s.write("broken/jobs", "not a directory\n");
    let out = s.run(&["init", "broken"]);
    assert_eq!(code(&out), 1, "init fails: {}", combined(&out));
    assert!(
        stderr(&out).contains("cannot write"),
        "the failure is named: {}",
        stderr(&out)
    );
    assert!(
        !s.exists("broken/lib/guards.sh"),
        "the file written before the failure is gone"
    );
    assert!(
        !s.exists("broken/lightsout.toml"),
        "no marker claims the workspace was initialized"
    );
    assert_eq!(
        s.read("broken/jobs"),
        "not a directory\n",
        "a file that predated init is left untouched"
    );
}
