//! The dashboard page: hierarchy, the facts a person needs, and the escaping
//! rules that keep a block reason from becoming markup.

mod common;

use std::process::Output;

use common::*;

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

#[test]
fn the_page_says_what_needs_a_person() {
    let s = Sandbox::new("report-attention");
    s.write(
        "jobs/ready/job.sh",
        "#!/usr/bin/env bash\nset -euo pipefail\n. \"$REPO/lib/guards.sh\"\nu_say \"stage 1: build\"\nu_done\n",
    );
    s.write(
        "jobs/ready/spec.md",
        "# ready — spec\n\nBuild the day's master and gate it.\n\ncadence\n: Daily at 06:00.\n\nstages\n: 1. build\n  2. gate\n",
    );
    let today = s.today();
    s.write("jobs/ready/state/expect", "06:00\n");
    s.write(
        "jobs/ready/state/blocked",
        &format!("{today}T08:00:00-0600\tjudgment\trender\tthe gate saw <b>empty</b> master\n"),
    );
    s.write(
        "jobs/ready/state/runs.tsv",
        &format!("{today}T08:00:00-0600\t{today}T08:01:00-0600\t2\trun\tx<b>unit</b>\t60\n"),
    );
    s.write(
        "jobs/ready/state/last",
        &format!("{today}T08:01:00-0600\t2\trun\n"),
    );
    s.write(
        "jobs/ready/state/ledger.tsv",
        &format!("{today}\t123\tready\ttest-model\n"),
    );

    let out = s.run(&["report", "--html", "out/page.html"]);
    assert_eq!(code(&out), 0, "report exits 0: {}", combined(&out));
    let page = s.read("out/page.html");

    assert!(
        page.contains("<section class=\"panel attention\">"),
        "blocked jobs get the attention panel: {page}"
    );
    assert!(page.contains("Needs attention"), "the panel is named");
    assert!(
        page.contains("class=\"cls c-judgment\""),
        "the triage class is shown"
    );
    assert!(
        page.contains("lightsout diagnose ready"),
        "the page names the next command"
    );
    assert!(
        page.contains("&lt;b&gt;empty&lt;/b&gt;"),
        "the reason is escaped"
    );
    assert!(
        !page.contains("<b>empty</b>"),
        "raw markup never reaches the page"
    );
    assert!(
        page.contains("x&lt;b&gt;unit&lt;/b&gt;"),
        "the run unit is escaped"
    );
    assert!(
        !page.contains("x<b>unit</b>"),
        "a run unit cannot inject markup"
    );
    assert!(
        page.contains("$1.23"),
        "ledger cents read as dollars: {page}"
    );
    assert!(
        page.contains("<span class=\"label\">needs you</span><span class=\"value\">1</span>"),
        "the KPI counts the blocked job"
    );
    assert!(
        page.contains("id=\"q\"") && page.contains("data-filter=\"attention\""),
        "the page can be filtered without a server"
    );
    assert!(
        page.contains("<tr class='blocked'>"),
        "the row keeps its state class"
    );
}

#[test]
fn a_quiet_workspace_says_so() {
    let s = Sandbox::new("report-quiet");
    s.reset_noop();
    assert_eq!(code(&s.run(&["run", "noop"])), 0, "the fixture runs");

    let out = s.run(&["report", "--html", "out/page.html"]);
    assert_eq!(code(&out), 0, "report exits 0: {}", combined(&out));
    let page = s.read("out/page.html");

    assert!(
        page.contains("All quiet"),
        "no attention panel when nothing needs a person"
    );
    assert!(
        !page.contains("<section class=\"panel attention\">"),
        "the attention panel is absent, not empty"
    );
    assert!(
        page.contains("<span class=\"pill p-done\">done today</span>"),
        "the done job is a pill"
    );
}

#[test]
fn the_page_carries_runs_and_attempts() {
    let s = Sandbox::new("report-runs");
    s.reset_noop();
    assert_eq!(code(&s.run(&["run", "noop"])), 0, "the fixture runs");

    let out = s.run(&["report", "--html", "out/page.html"]);
    assert_eq!(code(&out), 0, "report exits 0: {}", combined(&out));
    let page = s.read("out/page.html");

    assert!(
        page.contains("class=\"rd r0\""),
        "a successful run is a green square"
    );
    assert!(
        page.contains("attempts today / all time"),
        "the attempts column explains its two numbers"
    );
}

#[test]
fn a_late_job_leads_the_page() {
    let s = Sandbox::new("report-late");
    s.reset_noop();
    s.write("jobs/noop/state/expect", "00:00\n");

    let out = s.run(&["report", "--html", "out/page.html"]);
    assert_eq!(code(&out), 0, "report exits 0: {}", combined(&out));
    let page = s.read("out/page.html");

    assert!(
        page.contains("class=\"pill p-late\">overdue</span>"),
        "a missed expect time is an overdue item: {page}"
    );
    assert!(
        page.contains("<span class=\"label\">overdue</span><span class=\"value\">1</span>"),
        "the KPI counts the late job"
    );
}

#[test]
fn a_failed_last_run_is_not_a_hold() {
    let s = Sandbox::new("report-failed-after-hold");
    s.reset_noop();
    let stamp = s.now_log_stamp();
    s.write("jobs/noop/state/expect", "00:00\n");
    s.write(
        "jobs/noop/state/last",
        &format!("{stamp}\t0\trun\n{stamp}\t1\trun\n"),
    );

    let out = s.run(&["report", "--html", "out/page.html"]);
    assert_eq!(code(&out), 0, "report exits 0: {}", combined(&out));
    let page = s.read("out/page.html");

    assert!(
        page.contains("class=\"pill p-failed\">failed</span>"),
        "the latest run failed: {page}"
    );
    assert!(
        page.contains("<span class=\"late\">late"),
        "the due time is late, not held: {page}"
    );
    assert!(
        !page.contains(">held</span>"),
        "an earlier hold does not excuse a later failure"
    );
}
