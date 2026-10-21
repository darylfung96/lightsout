//! Agent profiles: runner/model selection shared by a job and its repair.
//!
//! These are additive to the 261 contract checks. They exercise the profile
//! file, the dsh patch check, the repair path, and the read-only report.

mod common;

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

fn profile_patch(model: &str, effort: &str) -> String {
    format!(
        "- id: agent-default-model\n  config:\n    provider: opencode\n    model: {model}\n    reasoningEffort: {effort}\n"
    )
}

fn triage_json(s: &Sandbox) -> Value {
    let out = s.run(&["triage", "noop", "--json"]);
    assert_eq!(code(&out), 0, "triage --json: {}", combined(&out));
    serde_json::from_slice(&out.stdout).expect("triage --json parses")
}

#[test]
fn a_job_profile_sets_the_runner_and_model_for_the_job() {
    let s = Sandbox::new("agent-profile-run");
    s.write(
        "jobs/noop/agent.conf",
        "runner = fake\nmodel = test-model\n",
    );
    let log = s.path("agent.log");
    let log = log.to_string_lossy().into_owned();

    let out = s.run_env(
        &["run", "noop"],
        &[("NOOP_AGENT", "1"), ("FAKE_AGENT_LOG", &log)],
    );
    assert_eq!(code(&out), 0, "profile run: {}", combined(&out));
    let calls = s.read("agent.log");
    assert!(
        calls.contains("test-model"),
        "model reached the runner: {calls}"
    );
    assert!(
        calls.contains("\tfake\t"),
        "runner reached the adapter: {calls}"
    );

    let plan = s.run_env(&["plan", "noop"], &[("NOOP_AGENT", "1")]);
    assert_eq!(code(&plan), 0, "plan: {}", combined(&plan));
    assert!(
        combined(&plan).contains("agent: fake test-model"),
        "plan reports the resolved profile: {}",
        combined(&plan)
    );
}

#[test]
fn a_job_can_select_a_named_workspace_profile() {
    let s = Sandbox::new("agent-profile-named");
    s.write(
        "agents.conf",
        "[lesson] # the lesson agent\nrunner = fake # test double\nmodel = named-model # pinned\n",
    );
    s.write("jobs/noop/agent.conf", "profile = lesson\n");
    let log = s.path("agent.log");
    let log = log.to_string_lossy().into_owned();

    let out = s.run_env(
        &["run", "noop"],
        &[("NOOP_AGENT", "1"), ("FAKE_AGENT_LOG", &log)],
    );
    assert_eq!(code(&out), 0, "named profile run: {}", combined(&out));
    let calls = s.read("agent.log");
    assert!(calls.contains("named-model"), "named model: {calls}");
    assert!(calls.contains("profile=lesson"), "profile name: {calls}");
}

#[test]
fn a_dsh_patch_fills_and_checks_the_model() {
    let s = Sandbox::new("agent-profile-patch");
    s.write("pinned.yml", &profile_patch("pinned", "high"));
    s.write("jobs/noop/agent.conf", "runner = dsh\npatch = pinned.yml\n");
    block_noop(&s, "judgment", "gate:exists");
    s.write("triage.conf", "noop gate:exists agent:propose\n");

    let row = triage_json(&s);
    assert_eq!(
        row.pointer("/agent/runner").and_then(Value::as_str),
        Some("dsh")
    );
    assert_eq!(
        row.pointer("/agent/model_label").and_then(Value::as_str),
        Some("opencode/pinned")
    );
    assert_eq!(
        row.pointer("/agent/effort").and_then(Value::as_str),
        Some("high")
    );
    assert!(
        row.pointer("/agent/patch")
            .and_then(Value::as_str)
            .unwrap_or("")
            .ends_with("pinned.yml"),
        "patch is resolved: {row}"
    );

    let text = combined(&s.run(&["triage", "noop"]));
    assert!(
        text.contains("agent: dsh opencode/pinned effort=high"),
        "triage prints the profile: {text}"
    );
}

#[test]
fn a_profile_model_that_disagrees_with_the_patch_is_refused() {
    let s = Sandbox::new("agent-profile-mismatch");
    s.write("pinned.yml", &profile_patch("pinned", "high"));
    s.write(
        "jobs/noop/agent.conf",
        "runner = dsh\npatch = pinned.yml\nmodel = other\n",
    );
    let out = s.run(&["plan", "noop"]);
    assert_eq!(
        code(&out),
        64,
        "mismatch is a config error: {}",
        combined(&out)
    );
    assert!(
        combined(&out).contains("does not match"),
        "error names the mismatch: {}",
        combined(&out)
    );
}

#[test]
fn a_repair_uses_the_job_profile_and_records_it() {
    let s = Sandbox::new("agent-profile-heal");
    s.write(
        "jobs/noop/agent.conf",
        "runner = fake\nmodel = repair-model\n",
    );
    block_noop(&s, "judgment", "gate:exists");
    s.write("triage.conf", "noop gate:exists agent:apply\n");
    s.write(
        "proposal.json",
        r#"{"fixable":true,"summary":"write a marker","edits":[{"path":"out/fix.txt","content":"fixed\n"}]}"#,
    );
    let log = s.path("agent.log");
    let log = log.to_string_lossy().into_owned();
    let proposal = s.path("proposal.json");
    let proposal = proposal.to_string_lossy().into_owned();

    let out = s.run_env(
        &["heal", "noop"],
        &[("FAKE_AGENT_LOG", &log), ("FAKE_AGENT_PROPOSAL", &proposal)],
    );
    assert_eq!(code(&out), 0, "heal rerun: {}", combined(&out));
    let calls = s.read("agent.log");
    assert!(calls.contains("repair-model"), "repair model: {calls}");

    let history = s.read("jobs/noop/state/heal/history.tsv");
    assert!(
        history.contains("fake repair-model"),
        "history records the profile: {history}"
    );
}

#[test]
fn a_missing_dsh_patch_is_a_doctor_warning_but_a_run_refusal() {
    let s = Sandbox::new("agent-profile-missing");
    s.write(
        "jobs/noop/agent.conf",
        "runner = dsh\npatch = missing.yml\n",
    );
    let doctor = s.run(&["doctor"]);
    assert_eq!(code(&doctor), 0, "doctor: {}", combined(&doctor));
    assert!(
        combined(&doctor).contains("dsh patch does not exist"),
        "doctor warns: {}",
        combined(&doctor)
    );

    // The profile is deliberately broken, so this writes the block directly:
    // a normal run refuses before it can reach the job.
    s.write(
        "jobs/noop/state/blocked",
        &format!(
            "{}Z\tjudgment\tgate:exists\tsimulated gate failure\n",
            s.today()
        ),
    );
    s.write("triage.conf", "noop gate:exists agent:apply\n");
    let out = s.run(&["heal", "noop"]);
    assert_eq!(code(&out), 64, "run refuses: {}", combined(&out));
    assert!(
        combined(&out).contains("dsh patch does not exist"),
        "refusal names the patch: {}",
        combined(&out)
    );
    assert!(
        !s.exists(&format!("jobs/noop/state/heal/{}/attempt-1", s.today())),
        "no attempt is spent on a bad profile"
    );
}

#[test]
fn an_unknown_profile_key_is_a_config_error_with_a_line() {
    let s = Sandbox::new("agent-profile-key");
    s.write(
        "jobs/noop/agent.conf",
        "runner = fake\nmodel = x\ncolour = blue\n",
    );
    let out = s.run(&["plan", "noop"]);
    assert_eq!(code(&out), 64, "unknown key: {}", combined(&out));
    assert!(
        combined(&out).contains("agent.conf:3") && combined(&out).contains("colour"),
        "error names the line and key: {}",
        combined(&out)
    );
}
