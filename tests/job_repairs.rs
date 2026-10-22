//! Job-local repairs: `jobs/<name>/repairs.sh` joins the allowlist and is
//! sourced after `lib/repairs.sh`, so a job can ship the repair for its own
//! failure mode without editing the workspace library.

mod common;

use common::*;

/// Writes the artifact noop's `exists` gate wants, plus a marker only this
/// definition writes, so a test can tell which repair actually ran.
const MARK_REPAIR: &str = "\
repair_mark() {
  mkdir -p \"$OUT\"
  : > \"$OUT/hello.txt\"
  printf 'job-local\\n' > \"$OUT/who\"
}
";

fn block_noop(s: &Sandbox) {
    let out = s.run_env(
        &["run", "noop"],
        &[("NOOP_BLOCK", "1"), ("NOOP_CLASS", "retryable")],
    );
    assert_eq!(code(&out), 2, "run: {}", combined(&out));
}

#[test]
fn a_job_local_repair_is_allowlisted_and_runs() {
    let s = Sandbox::new("job-repair-local");
    s.write("jobs/noop/repairs.sh", MARK_REPAIR);
    block_noop(&s);

    let out = s.run(&["retry", "noop", "--repair", "mark"]);
    assert_eq!(code(&out), 0, "retry: {}", combined(&out));
    assert_eq!(s.read("jobs/noop/out/who").trim(), "job-local");
    assert!(
        s.is_file(&format!("jobs/noop/state/done/{}", s.today())),
        "the re-run reached done"
    );
}

#[test]
fn a_job_local_repair_wins_over_the_library() {
    let s = Sandbox::new("job-repair-wins");
    s.write(
        "jobs/noop/repairs.sh",
        "repair_touch-missing() {\n  : > \"$OUT/hello.txt\"\n  printf 'job-local\\n' > \"$OUT/who\"\n}\n",
    );
    block_noop(&s);

    // No --repair: touch-missing is the default name, defined in both files.
    let out = s.run(&["retry", "noop"]);
    assert_eq!(code(&out), 0, "retry: {}", combined(&out));
    assert_eq!(
        s.read("jobs/noop/out/who").trim(),
        "job-local",
        "the job's definition must be sourced last"
    );
}

#[test]
fn triage_accepts_a_rule_naming_a_job_local_repair() {
    let s = Sandbox::new("job-repair-triage");
    s.write("jobs/noop/repairs.sh", MARK_REPAIR);
    s.write("triage.conf", "noop * repair:mark\n");
    block_noop(&s);

    let out = s.run(&["triage", "noop"]);
    assert_eq!(code(&out), 0, "triage: {}", combined(&out));
    assert!(
        combined(&out).contains("repair:mark"),
        "the rule resolves: {}",
        combined(&out)
    );
}
