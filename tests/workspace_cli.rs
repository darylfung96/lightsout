//! Workspace lifecycle tests: `init`, `doctor`, the marker and `--workspace`.
//!
//! These are additions to the 261 contract checks; they exercise the
//! distribution surface a new user meets before the first job runs.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Temp {
    path: PathBuf,
}

impl Temp {
    fn new(label: &str) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = env::temp_dir().join(format!(
            "lightsout-workspace-{}-{nanos}-{unique}-{label}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Temp { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn join(&self, rel: &str) -> PathBuf {
        self.path.join(rel)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn run(cwd: &Path, args: &[&str]) -> Output {
    run_env(cwd, args, &[])
}

fn run_env(cwd: &Path, args: &[&str], vars: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lightsout"));
    command.current_dir(cwd);
    for (key, _) in env::vars_os() {
        let key = key.to_string_lossy();
        if key.starts_with("U_") || key.starts_with("NOOP_") || key.starts_with("FAKE_") {
            command.env_remove(key.as_ref());
        }
    }
    command.env("U_NOTIFY", "0");
    command.env("U_MEMORY", "0");
    command.env_remove("LIGHTSOUT_REPO");
    for (key, value) in vars {
        command.env(key, value);
    }
    command.args(args);
    command.output().expect("run lightsout")
}

fn code(output: &Output) -> i32 {
    if let Some(code) = output.status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = output.status.signal() {
            return 128 + signal;
        }
    }
    1
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn init_creates_a_runnable_workspace() {
    let temp = Temp::new("init");
    let office = temp.join("office");
    let output = run(temp.path(), &["init", office.to_str().unwrap()]);
    assert_eq!(code(&output), 0, "{}", text(&output));
    assert!(office.join("lightsout.toml").is_file());
    assert!(office.join("lib/guards.sh").is_file());
    assert!(office.join("jobs/noop/job.sh").is_file());
    assert!(office.join("bin/scaffold").is_file());

    let doctor = run(&office, &["doctor"]);
    assert_eq!(code(&doctor), 0, "{}", text(&doctor));
    assert!(text(&doctor).contains("marker schema 1"));

    let run_noop = run(&office, &["run", "noop"]);
    assert_eq!(code(&run_noop), 0, "{}", text(&run_noop));
    assert!(office
        .join("jobs/noop/state/done")
        .read_dir()
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false));
    assert!(office.join("jobs/noop/out/hello.txt").is_file());

    let list = run(&office, &["list"]);
    assert_eq!(code(&list), 0);
    assert_eq!(String::from_utf8_lossy(&list.stdout).trim(), "noop");
}

#[test]
fn init_dry_run_writes_nothing() {
    let temp = Temp::new("dry");
    let office = temp.join("office");
    let output = run(
        temp.path(),
        &["init", "--dry-run", office.to_str().unwrap()],
    );
    assert_eq!(code(&output), 0, "{}", text(&output));
    assert!(text(&output).contains("dry run: no files written"));
    assert!(!office.exists());
}

#[test]
fn init_refuses_to_overwrite_and_writes_nothing() {
    let temp = Temp::new("collision");
    let office = temp.join("office");
    fs::create_dir_all(office.join("lib")).unwrap();
    fs::write(office.join("lib/guards.sh"), "keep me\n").unwrap();
    let output = run(temp.path(), &["init", office.to_str().unwrap()]);
    assert_eq!(code(&output), 64, "{}", text(&output));
    assert_eq!(
        fs::read_to_string(office.join("lib/guards.sh")).unwrap(),
        "keep me\n"
    );
    assert!(!office.join("lightsout.toml").exists());
}

#[test]
fn init_is_idempotent_after_the_marker() {
    let temp = Temp::new("idempotent");
    let office = temp.join("office");
    assert_eq!(
        code(&run(temp.path(), &["init", office.to_str().unwrap()])),
        0
    );
    let marker_before = fs::read(office.join("lightsout.toml")).unwrap();
    let again = run(temp.path(), &["init", office.to_str().unwrap()]);
    assert_eq!(code(&again), 0, "{}", text(&again));
    assert!(text(&again).contains("already initialized"));
    assert_eq!(
        fs::read(office.join("lightsout.toml")).unwrap(),
        marker_before
    );
}

#[test]
fn doctor_reports_a_missing_library() {
    let temp = Temp::new("doctor");
    let office = temp.join("office");
    assert_eq!(
        code(&run(temp.path(), &["init", office.to_str().unwrap()])),
        0
    );
    fs::remove_file(office.join("lib/guards.sh")).unwrap();
    let doctor = run(&office, &["doctor"]);
    assert_eq!(code(&doctor), 1, "{}", text(&doctor));
    assert!(text(&doctor).contains("lib/guards.sh is missing"));
}

#[test]
fn explicit_workspace_flag_works_from_elsewhere() {
    let temp = Temp::new("flag");
    let office = temp.join("office");
    assert_eq!(
        code(&run(temp.path(), &["init", office.to_str().unwrap()])),
        0
    );
    let elsewhere = temp.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let list = run(
        &elsewhere,
        &["--workspace", office.to_str().unwrap(), "list"],
    );
    assert_eq!(code(&list), 0, "{}", text(&list));
    assert_eq!(String::from_utf8_lossy(&list.stdout).trim(), "noop");
}

#[test]
fn version_and_unknown_workspace() {
    let temp = Temp::new("version");
    let version = run(temp.path(), &["--version"]);
    assert_eq!(code(&version), 0);
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        format!("lightsout {}", env!("CARGO_PKG_VERSION"))
    );

    let status = run_env(
        temp.path(),
        &["status"],
        &[("LIGHTSOUT_REPO", temp.path().to_str().unwrap())],
    );
    assert_eq!(code(&status), 64, "{}", text(&status));
    assert!(text(&status).contains("not a LightsOut workspace"));
}
