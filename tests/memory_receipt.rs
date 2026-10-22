//! Shell-job memory receipts: the gate records save, lint, scan and evidence.
//!
//! Additive to the 261-check contract suite; a shell job has no DSH session
//! transcript, so `lightsout gate memory --receipt` writes its own proof.

mod common;

use std::fs;

use common::*;
use serde_json::Value;

fn fake_store(s: &Sandbox) -> String {
    s.path("tests/fake-pi-memory")
        .to_string_lossy()
        .into_owned()
}

fn receipt_path(s: &Sandbox) -> std::path::PathBuf {
    s.path("state/memory-pass.json")
}

#[test]
fn shell_job_records_its_own_receipt() {
    let s = Sandbox::new("memory-receipt");
    let out = s.run_env(
        &[
            "gate",
            "memory",
            "--files",
            "notes/00-index.md",
            "--receipt",
            "state/memory-pass.json",
        ],
        &[
            ("U_PI_MEMORY", &fake_store(&s)),
            ("FAKE_MEMORY_RC", "0"),
            ("FAKE_MEMORY_DUPES", "0"),
            ("JOB", "noop"),
        ],
    );
    assert_eq!(code(&out), 0, "gate: {}", combined(&out));
    let path = receipt_path(&s);
    assert!(path.is_file(), "the gate wrote state/memory-pass.json");
    let receipt: Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).expect("receipt is JSON");
    assert_eq!(receipt["gate"], "memory");
    assert_eq!(receipt["job"], "noop");
    assert_eq!(receipt["exact_repeats"], 0);
    let page = &receipt["files"][0];
    assert_eq!(page["name"], "notes/00-index.md");
    assert!(page["hash"].as_str().unwrap_or("").starts_with("fnv1a64:"));
    assert!(page["mtime"].as_u64().unwrap_or(0) > 0);
    assert!(page["bytes"].as_u64().unwrap_or(0) > 0);
    let cwd = fs::canonicalize(&s.root).unwrap();
    assert_eq!(receipt["cwd"].as_str().unwrap_or(""), cwd.to_string_lossy());
}

#[test]
fn a_failed_scan_writes_no_receipt() {
    let s = Sandbox::new("memory-scan-fail");
    let out = s.run_env(
        &[
            "gate",
            "memory",
            "--files",
            "notes/00-index.md",
            "--receipt",
            "state/memory-pass.json",
        ],
        &[
            ("U_PI_MEMORY", &fake_store(&s)),
            ("FAKE_MEMORY_RC", "0"),
            ("FAKE_MEMORY_FAIL", "scan"),
        ],
    );
    assert_eq!(code(&out), 1, "a failing scan fails closed");
    assert!(!receipt_path(&s).exists(), "no receipt after a failed scan");
}

#[test]
fn duplicates_fail_the_gate_and_write_no_receipt() {
    let s = Sandbox::new("memory-dupes");
    let out = s.run_env(
        &[
            "gate",
            "memory",
            "--files",
            "notes/00-index.md",
            "--receipt",
            "state/memory-pass.json",
        ],
        &[
            ("U_PI_MEMORY", &fake_store(&s)),
            ("FAKE_MEMORY_RC", "0"),
            ("FAKE_MEMORY_DUPES", "2"),
        ],
    );
    assert_eq!(code(&out), 1, "duplicates fail the gate");
    assert!(
        combined(&out).contains("2 exact repeats"),
        "the gate names the count: {}",
        combined(&out)
    );
    assert!(!receipt_path(&s).exists(), "no receipt when repeats remain");
}

#[test]
fn a_job_run_writes_the_receipt_under_state() {
    let s = Sandbox::new("memory-job");
    let home = s.path("memory-home");
    fs::create_dir_all(&home).unwrap();
    s.reset_noop();
    let out = s.run_env(
        &["run", "noop"],
        &[
            ("HOME", &home.to_string_lossy()),
            ("U_MEMORY", "1"),
            ("U_PI_MEMORY", &fake_store(&s)),
            ("FAKE_MEMORY_RC", "0"),
        ],
    );
    assert_eq!(code(&out), 0, "run: {}", combined(&out));
    assert!(
        s.path("jobs/noop/state/memory-pass.json").is_file(),
        "lib/memory.sh records the job's pass under state/"
    );
}
