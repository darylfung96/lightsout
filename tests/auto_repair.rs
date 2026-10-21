//! Auto-repair tests: triage policy, the bounded loop, and the agent repair.
//!
//! These tests are additive to the 261-check contract suite; they do not touch
//! `tests/check_manifest.rs` or the counts in `tests/run.sh`.

mod common;

use std::fs;
use std::path::Path;

use common::*;
use serde_json::Value;

fn block_noop(s: &Sandbox, class: &str, source: &str) {
    s.reset_noop();
    let out = s.run_env(
        &["run", "noop", "--class", class, "--source", source],
        &[("NOOP_BLOCK", "1")],
    );
    assert_eq!(code(&out), 2, "block noop: {}", combined(&out));
}

fn triage_json(s: &Sandbox) -> Value {
    let out = s.run(&["triage", "noop", "--json"]);
    assert_eq!(code(&out), 0, "triage --json: {}", combined(&out));
    serde_json::from_slice(&out.stdout).expect("triage --json parses")
}

fn action(row: &Value) -> String {
    row.get("action")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn fingerprint(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => walk(&path, base, out),
                Ok(_) => out.push((rel, fs::read(&path).unwrap_or_default())),
                Err(_) => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn policy_precedence_exact_then_job_then_source_then_default() {
    let s = Sandbox::new("ar-precedence");
    block_noop(&s, "judgment", "gate:exists");

    s.write(
        "triage.conf",
        "\
*       *             human
*       gate:exists   agent:propose
noop    *             agent:apply
noop    gate:exists   agent:apply
",
    );
    assert_eq!(action(&triage_json(&s)), "agent:apply", "exact rule wins");

    s.write(
        "triage.conf",
        "\
*       *             human
*       gate:exists   agent:propose
noop    *             agent:apply
",
    );
    let row = triage_json(&s);
    assert_eq!(action(&row), "agent:apply", "job wildcard beats source");
    assert!(
        row.get("matched_rule")
            .and_then(Value::as_str)
            .unwrap_or("")
            .contains("noop"),
        "matched rule names the job: {row}"
    );

    s.write(
        "triage.conf",
        "\
*       *             human
*       gate:exists   agent:propose
",
    );
    assert_eq!(
        action(&triage_json(&s)),
        "agent:propose",
        "source rule wins"
    );

    s.write("triage.conf", "* * human\n");
    assert_eq!(action(&triage_json(&s)), "human", "default rule wins");
}

#[test]
fn policy_duplicate_exact_is_an_error_with_a_line_number() {
    let s = Sandbox::new("ar-duplicate");
    block_noop(&s, "judgment", "gate:exists");
    s.write(
        "triage.conf",
        "\
* * human
noop gate:exists human
noop gate:exists agent:apply
",
    );
    let out = s.run(&["triage", "noop", "--json"]);
    assert_eq!(code(&out), 64, "duplicate exact rule is rejected");
    let text = combined(&out);
    assert!(text.contains("3"), "line number is reported: {text}");
    assert!(
        text.contains("duplicate") || text.contains("already"),
        "error names the duplicate: {text}"
    );
}

#[test]
fn policy_unknown_action_and_repair_are_errors() {
    let s = Sandbox::new("ar-unknown");
    block_noop(&s, "judgment", "gate:exists");

    s.write("triage.conf", "noop * robot:fix\n");
    let out = s.run(&["triage", "noop"]);
    assert_eq!(code(&out), 64, "unknown action is rejected");
    assert!(
        combined(&out).contains("robot:fix"),
        "error names the action: {}",
        combined(&out)
    );

    s.write("triage.conf", "noop * repair:not-registered\n");
    let out = s.run(&["triage", "noop"]);
    assert_eq!(code(&out), 64, "unknown repair is rejected");
    assert!(
        combined(&out).contains("not-registered"),
        "error names the repair: {}",
        combined(&out)
    );
}

#[test]
fn triage_read_only_and_reports_the_decision() {
    let s = Sandbox::new("ar-readonly");
    block_noop(&s, "judgment", "gate:exists");
    s.write("triage.conf", "noop gate:exists agent:apply\n");

    let before = fingerprint(&s.root);
    let out = s.run(&["triage", "noop"]);
    let after = fingerprint(&s.root);

    assert_eq!(code(&out), 0, "triage exits 0: {}", combined(&out));
    assert_eq!(before, after, "triage writes nothing");
    let text = combined(&out);
    assert!(text.contains("agent:apply"), "decision: {text}");
    assert!(text.contains("judgment"), "block class: {text}");
    assert!(text.contains("gate:exists"), "failure key: {text}");

    let row = triage_json(&s);
    assert_eq!(
        row.pointer("/blocked/class").and_then(Value::as_str),
        Some("judgment")
    );
    assert_eq!(
        row.pointer("/blocked/source").and_then(Value::as_str),
        Some("gate:exists")
    );
    assert_eq!(row.get("key").and_then(Value::as_str), Some("gate:exists"));
    assert_eq!(row.get("mode").and_then(Value::as_str), Some("apply"));
    assert_eq!(row.get("max_heals").and_then(Value::as_u64), Some(2));
    assert_eq!(row.get("max_heals_total").and_then(Value::as_u64), Some(5));
}

#[test]
fn triage_without_a_policy_is_human() {
    let s = Sandbox::new("ar-nopolicy");
    block_noop(&s, "judgment", "gate:exists");
    let row = triage_json(&s);
    assert_eq!(action(&row), "human");
    assert_eq!(row.get("mode").and_then(Value::as_str), Some("human"));
}

#[test]
fn doctor_validates_the_policy() {
    let s = Sandbox::new("ar-doctor");
    s.write("triage.conf", "noop gate:exists robot:fix\n");
    let out = s.run(&["doctor"]);
    assert_eq!(code(&out), 1, "malformed policy is an error");
    let text = combined(&out);
    assert!(
        text.contains("triage.conf") && text.contains("robot:fix"),
        "doctor names the bad rule: {text}"
    );

    s.write("triage.conf", "ghost-job gate:exists human\nnoop * human\n");
    let out = s.run(&["doctor"]);
    assert_eq!(code(&out), 0, "a rule for a missing job is a warning");
    assert!(
        combined(&out).contains("ghost-job"),
        "doctor names the missing job: {}",
        combined(&out)
    );
}

#[test]
fn triage_failure_key_derives_a_gate_from_the_reason() {
    let s = Sandbox::new("ar-key");
    s.reset_noop();
    s.mkdir("jobs/noop/state");
    s.write(
        "jobs/noop/state/blocked",
        &format!(
            "{}\tjudgment\tunknown\tgate failed: duration --min 59 --max 61\n",
            s.now_log_stamp()
        ),
    );
    let row = triage_json(&s);
    assert_eq!(
        row.get("key").and_then(Value::as_str),
        Some("gate:duration")
    );
}

fn ready_job(s: &Sandbox, always_block: bool) {
    let body = if always_block {
        r#"#!/usr/bin/env bash
set -euo pipefail
. "$REPO/lib/guards.sh"
u_block --class retryable --source gate:ready "ready marker missing"
"#
    } else {
        r#"#!/usr/bin/env bash
set -euo pipefail
. "$REPO/lib/guards.sh"
if [ ! -f "$OUT/ready" ]; then
  u_block --class retryable --source gate:ready "ready marker missing"
fi
u_done
"#
    };
    s.write("jobs/ready/job.sh", body);
}

fn add_repair(s: &Sandbox, name: &str, body: &str) {
    s.append(
        "lib/repairs.sh",
        &format!("\nrepair_{name}() {{\n{body}\n}}\n"),
    );
}

fn attempt_dir(s: &Sandbox) -> String {
    format!("jobs/ready/state/heal/{}/attempt-1", s.today())
}

#[test]
fn auto_repair_repairs_and_reruns_to_done() {
    let s = Sandbox::new("ar-auto-pass");
    ready_job(&s, false);
    add_repair(
        &s,
        "make-ready",
        "  mkdir -p \"$OUT\"\n  : > \"$OUT/ready\"",
    );
    s.write("triage.conf", "ready gate:ready repair:make-ready\n");

    let out = s.run(&["run", "ready", "--auto-repair"]);
    assert_eq!(code(&out), 0, "auto repair passes: {}", combined(&out));
    assert!(!s.exists("jobs/ready/state/blocked"), "block cleared");
    assert!(s.is_file("jobs/ready/out/ready"), "repair artifact written");

    let history = s.read("jobs/ready/state/heal/history.tsv");
    assert_eq!(
        history.lines().count(),
        1,
        "one attempt recorded: {history}"
    );
    assert!(history.contains("passed"), "attempt verdict: {history}");
    assert_eq!(
        s.read(&format!("jobs/ready/state/repairs/{}.count", s.today()))
            .trim(),
        "1",
        "one repair counted"
    );
    assert!(
        s.read(&format!("{}/verdict", attempt_dir(&s)))
            .contains("passed"),
        "attempt verdict file"
    );
}

#[test]
fn auto_repair_stops_when_the_same_failure_returns() {
    let s = Sandbox::new("ar-auto-noprogress");
    ready_job(&s, true);
    add_repair(&s, "do-nothing", "  :");
    s.write("triage.conf", "ready gate:ready repair:do-nothing\n");

    let out = s.run(&["run", "ready", "--auto-repair"]);
    assert_eq!(code(&out), 2, "same failure stops: {}", combined(&out));
    assert!(s.is_file("jobs/ready/state/blocked"), "block remains");
    let history = s.read("jobs/ready/state/heal/history.tsv");
    assert_eq!(history.lines().count(), 1, "one attempt only: {history}");
    assert!(
        history.contains("no-progress"),
        "verdict is no-progress: {history}"
    );
    assert_eq!(
        s.read(&format!("jobs/ready/state/repairs/{}.count", s.today()))
            .trim(),
        "1",
        "the repair ran once"
    );
}

#[test]
fn auto_repair_leaves_a_human_block_alone() {
    let s = Sandbox::new("ar-auto-human");
    ready_job(&s, true);
    let out = s.run(&["run", "ready", "--auto-repair"]);
    assert_eq!(code(&out), 2, "human block exits 2");
    assert!(s.is_file("jobs/ready/state/blocked"));
    assert!(
        !s.exists("jobs/ready/state/heal/history.tsv"),
        "no attempt for a human block"
    );
}

#[test]
fn auto_repair_is_held_by_a_live_lock() {
    let s = Sandbox::new("ar-auto-lock");
    ready_job(&s, false);
    s.mkdir("jobs/ready/state/heal/lock");
    s.write("jobs/ready/state/heal/lock/pid", "999\n");
    s.write(
        "jobs/ready/state/heal/lock/expiry",
        &format!("{}\n", s.now_epoch() + 60),
    );

    let out = s.run(&["run", "ready", "--auto-repair"]);
    assert_eq!(code(&out), 0, "held lock exits 0");
    assert!(combined(&out).contains("held"), "held is reported");
    assert!(!s.exists("jobs/ready/state/heal/history.tsv"));
}

#[test]
fn auto_repair_environment_enables_the_loop() {
    let s = Sandbox::new("ar-auto-env");
    ready_job(&s, false);
    add_repair(
        &s,
        "make-ready",
        "  mkdir -p \"$OUT\"\n  : > \"$OUT/ready\"",
    );
    s.write("triage.conf", "ready gate:ready repair:make-ready\n");

    let out = s.run_env(&["run", "ready"], &[("LIGHTSOUT_AUTO_REPAIR", "1")]);
    assert_eq!(code(&out), 0, "env enables auto repair: {}", combined(&out));
    assert!(s.is_file("jobs/ready/out/ready"));
}

#[test]
fn auto_repair_reports_the_repair_cap() {
    let s = Sandbox::new("ar-auto-repair-cap");
    ready_job(&s, true);
    add_repair(&s, "do-nothing", "  :");
    s.write("triage.conf", "ready gate:ready repair:do-nothing\n");

    let out = s.run_env(&["run", "ready", "--auto-repair"], &[("MAX_REPAIRS", "0")]);
    assert_eq!(code(&out), 2, "cap exits 2");
    assert!(combined(&out).contains("repair cap"), "cap is reported");
    let history = s.read("jobs/ready/state/heal/history.tsv");
    assert!(history.contains("refused"), "refused attempt recorded");
}

#[test]
fn plain_run_does_not_repair() {
    let s = Sandbox::new("ar-plain-run");
    ready_job(&s, false);
    add_repair(
        &s,
        "make-ready",
        "  mkdir -p \"$OUT\"\n  : > \"$OUT/ready\"",
    );
    s.write("triage.conf", "ready gate:ready repair:make-ready\n");

    let out = s.run(&["run", "ready"]);
    assert_eq!(code(&out), 2, "plain run stops at the block");
    assert!(
        !s.is_file("jobs/ready/out/ready"),
        "no repair without opt-in"
    );
    assert!(!s.exists("jobs/ready/state/heal/history.tsv"));
}

fn propose_policy(s: &Sandbox) {
    s.write("triage.conf", "ready gate:ready agent:propose\n");
}

fn block_ready(s: &Sandbox) {
    let out = s.run(&["run", "ready"]);
    assert_eq!(code(&out), 2, "block ready: {}", combined(&out));
}

fn fake_proposal(s: &Sandbox, body: &str) -> String {
    s.write("proposal-input.json", body);
    s.path("proposal-input.json").to_string_lossy().into_owned()
}

fn run_heal(s: &Sandbox, proposal: &str) -> std::process::Output {
    s.run_env(
        &["heal", "ready"],
        &[("U_RUNNER", "fake"), ("FAKE_AGENT_PROPOSAL", proposal)],
    )
}

#[test]
fn heal_propose_writes_a_proposal_and_changes_nothing() {
    let s = Sandbox::new("ar-heal-propose");
    ready_job(&s, true);
    propose_policy(&s);
    block_ready(&s);
    let proposal = fake_proposal(
        &s,
        r#"{"fixable":true,"summary":"write the ready marker","edits":[{"path":"out/ready","content":"ok"}]}"#,
    );

    let out = run_heal(&s, &proposal);
    assert_eq!(code(&out), 2, "propose exits 2: {}", combined(&out));
    assert!(
        combined(&out).contains("proposal:"),
        "proposal path printed"
    );
    assert!(
        !s.exists("jobs/ready/out/ready"),
        "propose changes no artifact"
    );

    let dir = format!("jobs/ready/state/heal/{}/attempt-1", s.today());
    assert!(
        s.is_file(&format!("{dir}/proposal.json")),
        "proposal stored"
    );
    assert!(s.is_file(&format!("{dir}/prompt.md")), "prompt stored");
    assert!(s.is_file(&format!("{dir}/runner.log")), "runner log stored");
    let prompt = s.read(&format!("{dir}/prompt.md"));
    assert!(prompt.contains("out/"), "allowed prefix is in the prompt");
    assert!(prompt.contains("\"edits\""), "schema is in the prompt");
    assert!(
        s.read("jobs/ready/state/heal/history.tsv")
            .contains("proposed"),
        "history records the proposal"
    );
}

#[test]
fn heal_fixable_false_leaves_the_block() {
    let s = Sandbox::new("ar-heal-unfixable");
    ready_job(&s, true);
    propose_policy(&s);
    block_ready(&s);
    let proposal = fake_proposal(
        &s,
        r#"{"fixable":false,"summary":"not safe to guess","edits":[]}"#,
    );

    let out = run_heal(&s, &proposal);
    assert_eq!(code(&out), 2, "unfixable exits 2");
    assert!(combined(&out).contains("unfixable"), "reason is printed");
    assert!(s.is_file("jobs/ready/state/blocked"), "block remains");
    assert!(
        s.read("jobs/ready/state/heal/history.tsv")
            .contains("unfixable"),
        "verdict is unfixable"
    );
}

#[test]
fn heal_timeout_is_recorded() {
    let s = Sandbox::new("ar-heal-timeout");
    ready_job(&s, true);
    propose_policy(&s);
    block_ready(&s);
    let proposal = fake_proposal(
        &s,
        r#"{"fixable":true,"summary":"late","edits":[{"path":"out/ready","content":"ok"}]}"#,
    );

    let out = s.run_env(
        &["heal", "ready"],
        &[
            ("U_RUNNER", "fake"),
            ("FAKE_AGENT_PROPOSAL", &proposal),
            ("FAKE_AGENT_SLEEP", "5"),
            ("U_AGENT_TIMEOUT", "1"),
        ],
    );
    assert_eq!(code(&out), 2, "timeout exits 2: {}", combined(&out));
    assert!(
        combined(&out).contains("timed out"),
        "timeout is reported: {}",
        combined(&out)
    );
    assert!(
        s.read("jobs/ready/state/heal/history.tsv")
            .contains("timeout"),
        "verdict is timeout"
    );
}

#[test]
fn heal_prompt_redacts_a_token() {
    let s = Sandbox::new("ar-heal-redact");
    ready_job(&s, true);
    propose_policy(&s);
    s.run(&["run", "ready"]);
    s.append(
        &format!("jobs/ready/logs/{}.log", s.today()),
        "token sk-abcdef1234567890abcdef\n",
    );
    let proposal = fake_proposal(
        &s,
        r#"{"fixable":true,"summary":"ok","edits":[{"path":"out/ready","content":"ok"}]}"#,
    );

    let out = run_heal(&s, &proposal);
    assert_eq!(code(&out), 2, "redaction run exits 2");
    let prompt = s.read(&format!(
        "jobs/ready/state/heal/{}/attempt-1/prompt.md",
        s.today()
    ));
    assert!(
        !prompt.contains("sk-abcdef1234567890abcdef"),
        "token is absent from the prompt"
    );
    assert!(prompt.contains("[redacted]"), "redaction marker present");
    let count: usize = s
        .read(&format!(
            "jobs/ready/state/heal/{}/attempt-1/redactions",
            s.today()
        ))
        .trim()
        .parse()
        .unwrap_or(0);
    assert!(count >= 1, "redaction count is recorded");
}

#[test]
fn heal_respects_the_job_heal_cap() {
    let s = Sandbox::new("ar-heal-cap");
    ready_job(&s, true);
    propose_policy(&s);
    block_ready(&s);
    let proposal = fake_proposal(
        &s,
        r#"{"fixable":true,"summary":"ok","edits":[{"path":"out/ready","content":"ok"}]}"#,
    );

    let out = s.run_env(
        &["heal", "ready"],
        &[
            ("U_RUNNER", "fake"),
            ("FAKE_AGENT_PROPOSAL", &proposal),
            ("MAX_HEALS", "0"),
        ],
    );
    assert_eq!(code(&out), 2, "cap exits 2");
    assert!(combined(&out).contains("heal cap"), "cap is reported");
    assert!(
        !s.exists(&format!("jobs/ready/state/heal/{}/attempt-1", s.today())),
        "no attempt dir"
    );
    assert!(!s.exists(&format!("logs/heal/{}.tsv", s.today())));
}

#[test]
fn heal_respects_the_workspace_heal_cap() {
    let s = Sandbox::new("ar-heal-ws-cap");
    ready_job(&s, true);
    propose_policy(&s);
    block_ready(&s);
    s.write(
        &format!("logs/heal/{}.tsv", s.today()),
        "2026-10-21T09:00:00-05:00\tother\tagent:apply\n",
    );
    let proposal = fake_proposal(
        &s,
        r#"{"fixable":true,"summary":"ok","edits":[{"path":"out/ready","content":"ok"}]}"#,
    );

    let out = s.run_env(
        &["heal", "ready"],
        &[
            ("U_RUNNER", "fake"),
            ("FAKE_AGENT_PROPOSAL", &proposal),
            ("MAX_HEALS_TOTAL", "1"),
        ],
    );
    assert_eq!(code(&out), 2, "workspace cap exits 2");
    assert!(
        combined(&out).contains("workspace heal cap"),
        "workspace cap is reported: {}",
        combined(&out)
    );
    assert!(
        !s.exists(&format!("jobs/ready/state/heal/{}/attempt-1", s.today())),
        "no attempt dir"
    );
}

#[test]
fn heal_without_a_block_is_a_noop() {
    let s = Sandbox::new("ar-heal-clean");
    ready_job(&s, false);
    let out = s.run(&["heal", "ready"]);
    assert_eq!(code(&out), 0, "clean heal exits 0");
    assert!(combined(&out).contains("not blocked"));
}

#[test]
fn auto_repair_propose_stops_with_a_proposal() {
    let s = Sandbox::new("ar-auto-propose");
    ready_job(&s, true);
    propose_policy(&s);
    let proposal = fake_proposal(
        &s,
        r#"{"fixable":true,"summary":"ok","edits":[{"path":"out/ready","content":"ok"}]}"#,
    );

    let out = s.run_env(
        &["run", "ready", "--auto-repair"],
        &[("U_RUNNER", "fake"), ("FAKE_AGENT_PROPOSAL", &proposal)],
    );
    assert_eq!(code(&out), 2, "propose in the loop exits 2");
    assert!(
        s.is_file(&format!(
            "jobs/ready/state/heal/{}/attempt-1/proposal.json",
            s.today()
        )),
        "loop stores the proposal"
    );
    assert!(!s.is_file("jobs/ready/out/ready"), "loop applies nothing");
}

fn fresh_blocked_ready(s: &Sandbox) {
    s.reset("ready");
    let _ = std::fs::remove_dir_all(s.path("logs/heal"));
    ready_job(s, true);
    block_ready(s);
}

#[test]
fn invalid_proposals_are_refused_without_changes() {
    let s = Sandbox::new("ar-validate");
    fresh_blocked_ready(&s);
    propose_policy(&s);

    let cases: &[(&str, &str)] = &[
        (
            "escape",
            r#"{"fixable":true,"summary":"bad","edits":[{"path":"../escape","content":"x"}]}"#,
        ),
        (
            "path",
            r#"{"fixable":true,"summary":"bad","edits":[{"path":"/tmp/escape","content":"x"}]}"#,
        ),
        (
            "protected",
            r#"{"fixable":true,"summary":"bad","edits":[{"path":"job.sh","content":"x"}]}"#,
        ),
        (
            "protected",
            r#"{"fixable":true,"summary":"bad","edits":[{"path":"state/blocked","content":"x"}]}"#,
        ),
        (
            "duplicate",
            r#"{"fixable":true,"summary":"bad","edits":[{"path":"out/x","content":"a"},{"path":"out/x","content":"b"}]}"#,
        ),
        (
            "schema",
            r#"{"fixable":true,"summary":"bad","commands":["rm"],"edits":[{"path":"out/x","content":"a"}]}"#,
        ),
        (
            "fixable-false-with-edits",
            r#"{"fixable":false,"summary":"bad","edits":[{"path":"out/x","content":"a"}]}"#,
        ),
        (
            "allow",
            r#"{"fixable":true,"summary":"bad","edits":[{"path":"assets/x","content":"a"}]}"#,
        ),
    ];

    for (rule, body) in cases {
        fresh_blocked_ready(&s);
        let proposal = fake_proposal(&s, body);
        let out = run_heal(&s, &proposal);
        assert_eq!(code(&out), 2, "{rule} exits 2: {}", combined(&out));
        let history = s.read("jobs/ready/state/heal/history.tsv");
        assert!(
            history.contains(&format!("invalid:{rule}")),
            "{rule} is recorded: {history}"
        );
        assert!(!s.exists("jobs/ready/out/x"), "{rule} writes nothing");
        assert!(
            s.is_file("jobs/ready/state/blocked"),
            "{rule} keeps the block"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_symlinked_allow_prefix_is_refused() {
    let s = Sandbox::new("ar-validate-symlink");
    fresh_blocked_ready(&s);
    propose_policy(&s);
    let outside = s.path("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::remove_dir_all(s.path("jobs/ready/out")).unwrap();
    std::os::unix::fs::symlink(&outside, s.path("jobs/ready/out")).unwrap();

    let proposal = fake_proposal(
        &s,
        r#"{"fixable":true,"summary":"bad","edits":[{"path":"out/x","content":"a"}]}"#,
    );
    let out = run_heal(&s, &proposal);
    assert_eq!(code(&out), 2, "symlink exits 2");
    let history = s.read("jobs/ready/state/heal/history.tsv");
    assert!(
        history.contains("invalid:symlink"),
        "symlink is recorded: {history}"
    );
    assert!(
        !s.exists("outside/x"),
        "nothing is written through the link"
    );
}
