//! Universal mode: a registered job directory outside any one workspace, a
//! generated command job, and the schedule printer that leaves the trigger to
//! cron or launchd.

mod common;

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use common::*;

fn registry_home(s: &Sandbox) -> PathBuf {
    s.path("home/.config/lightsout")
}

fn job_dir(s: &Sandbox, name: &str) -> PathBuf {
    registry_home(s).join("jobs").join(name)
}

/// A copy of the binary outside the sandbox tree, so `repo::root()` cannot
/// walk up into the sandbox workspace. This is what "run from anywhere" means.
fn isolated_binary(label: &str) -> (PathBuf, PathBuf) {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = env::temp_dir().join(format!(
        "lightsout-universal-{}-{unique}-{label}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let binary = dir.join("lightsout");
    fs::copy(env!("CARGO_BIN_EXE_lightsout"), &binary).unwrap();
    make_executable(&binary);
    (dir, binary)
}

fn isolated_run(
    s: &Sandbox,
    dir: &PathBuf,
    binary: &PathBuf,
    args: &[&str],
) -> std::process::Output {
    let mut cmd = Command::new(binary);
    cmd.current_dir(dir)
        .env("HOME", s.path("home"))
        .env("TMPDIR", s.path("tmp"))
        .env("U_NOTIFY", "0")
        .env("U_MEMORY", "0")
        .env("PATH", env::var_os("PATH").unwrap_or_default())
        .env_remove("LIGHTSOUT_REPO")
        .env_remove("LIGHTSOUT_HOME");
    cmd.args(args);
    cmd.output().expect("run isolated lightsout")
}

#[test]
fn a_command_job_runs_from_outside_any_workspace() {
    let s = Sandbox::new("um-command");
    let out = s.run(&[
        "job",
        "add",
        "backup",
        "--command",
        "printf 'ran\\n' > ran.txt",
    ]);
    assert_eq!(code(&out), 0, "job add: {}", combined(&out));
    assert!(
        registry_home(&s).join("lib/guards.sh").is_file(),
        "the registry home is initialized so job.sh can source guards"
    );
    assert!(
        registry_home(&s).join("lightsout.toml").is_file(),
        "the home is a workspace root"
    );

    let (dir, binary) = isolated_binary("command");
    let out = isolated_run(&s, &dir, &binary, &["run", "backup"]);
    assert_eq!(
        code(&out),
        0,
        "a registered job runs without a workspace: {}",
        combined(&out)
    );
    assert!(
        job_dir(&s, "backup").join("ran.txt").is_file(),
        "the command ran in the job's workdir"
    );
    assert!(
        job_dir(&s, "backup")
            .join("state/done")
            .join(s.today())
            .is_file(),
        "the exit code wrote the done marker"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_registered_job_is_visible_in_list_status_and_check() {
    let s = Sandbox::new("um-status");
    assert_eq!(
        code(&s.run(&["job", "add", "backup", "--command", "true"])),
        0
    );
    assert!(
        stdout(&s.run(&["list"]))
            .lines()
            .any(|line| line == "backup"),
        "list shows the registered job"
    );
    let row = s.row("backup");
    assert_eq!(row.get("name").and_then(|v| v.as_str()), Some("backup"));
    assert_eq!(row.get("has_job").and_then(|v| v.as_bool()), Some(true));

    assert_eq!(code(&s.run(&["run", "backup"])), 0);
    let row = s.row("backup");
    assert_eq!(
        row.get("state").and_then(|v| v.as_str()),
        Some("done today"),
        "the registry row reads the same state files: {row}"
    );

    // A missed expect is still an alarm for a job outside the workspace.
    let state = job_dir(&s, "backup").join("state");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("expect"), "00:00\n").unwrap();
    let _ = fs::remove_file(state.join("done").join(s.today()));
    // The rc-0 run above would otherwise read as a deliberate hold and
    // suppress the alarm; a job that never ran today has no `last` row.
    let _ = fs::remove_file(state.join("last"));
    let out = s.run(&["check", "backup"]);
    assert_eq!(code(&out), 0, "check: {}", combined(&out));
    assert!(
        state
            .join("alerts")
            .join(format!("{}.stale", s.today()))
            .is_file(),
        "the stale alarm lands under the registered job's state"
    );
}

#[test]
fn a_command_job_exit_code_is_the_gate() {
    let s = Sandbox::new("um-gate");
    assert_eq!(
        code(&s.run(&["job", "add", "bad", "--command", "exit 3"])),
        0
    );
    let out = s.run(&["run", "bad"]);
    assert_eq!(code(&out), 3, "the command's code is the job's code");
    let row = s.row("bad");
    assert_eq!(
        row.get("state").and_then(|v| v.as_str()),
        Some("failed"),
        "a non-zero command is a failed run, not never-run: {row}"
    );
    assert_eq!(row.get("last_exit").and_then(|v| v.as_i64()), Some(3));
    assert!(
        !job_dir(&s, "bad")
            .join("state/done")
            .join(s.today())
            .exists(),
        "a failed command writes no done marker"
    );
}

#[test]
fn a_job_dir_can_live_outside_the_workspace() {
    let s = Sandbox::new("um-jobdir");
    s.write(
        "elsewhere/proj/job.sh",
        "#!/usr/bin/env bash\nset -euo pipefail\n. \"$REPO/lib/guards.sh\"\nu_done\n",
    );
    let proj = s.path("elsewhere/proj").to_string_lossy().into_owned();
    let workspace = s.root.to_string_lossy().into_owned();
    let out = s.run(&[
        "job",
        "add",
        "proj",
        "--job-dir",
        &proj,
        "--workspace",
        &workspace,
    ]);
    assert_eq!(code(&out), 0, "job add: {}", combined(&out));
    let out = s.run(&["run", "proj"]);
    assert_eq!(code(&out), 0, "registered job: {}", combined(&out));
    assert!(
        s.is_file(&format!("elsewhere/proj/state/done/{}", s.today())),
        "state stays in the job directory"
    );
    let out = s.run(&["status", "--all", "--json"]);
    assert_eq!(code(&out), 0);
    let rows: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|row| row.get("name").and_then(|v| v.as_str()) == Some("proj")),
        "status shows a job registered from another directory"
    );
}

#[test]
fn job_remove_unregisters_without_deleting_files() {
    let s = Sandbox::new("um-remove");
    assert_eq!(
        code(&s.run(&["job", "add", "backup", "--command", "true"])),
        0
    );
    let dir = job_dir(&s, "backup");
    assert!(dir.join("job.sh").is_file());
    let out = s.run(&["job", "remove", "backup"]);
    assert_eq!(code(&out), 0, "remove: {}", combined(&out));
    assert!(dir.join("job.sh").is_file(), "the job files stay");
    assert!(
        !stdout(&s.run(&["list"]))
            .lines()
            .any(|line| line == "backup"),
        "the name is gone from the list"
    );
    assert_eq!(
        code(&s.run(&["run", "backup"])),
        64,
        "a removed name does not resolve"
    );
}

#[test]
fn schedule_prints_the_trigger_and_installs_nothing() {
    let s = Sandbox::new("um-schedule");
    assert_eq!(
        code(&s.run(&["job", "add", "backup", "--command", "true"])),
        0
    );
    let state = job_dir(&s, "backup").join("state");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("expect"), "07:15\n").unwrap();

    let out = s.run(&["schedule", "backup"]);
    assert_eq!(code(&out), 0, "schedule: {}", combined(&out));
    let text = stdout(&out);
    assert!(
        text.contains("15 7 * * *"),
        "the cron time comes from expect: {text}"
    );
    assert!(
        text.contains("run backup"),
        "the line names the job: {text}"
    );
    assert!(
        !text.contains("--workspace"),
        "a registered job needs no workspace"
    );
    assert!(
        !text.contains("--auto-repair"),
        "auto-repair is opt-in: {text}"
    );
    assert!(
        !registry_home(&s).join("logs/backup.log").exists(),
        "schedule writes no log"
    );

    let out = s.run(&["schedule", "backup", "--auto-repair"]);
    assert!(
        stdout(&out).contains("run backup --auto-repair"),
        "the flag reaches the cron command: {}",
        stdout(&out)
    );

    let out = s.run(&["schedule", "backup", "--at", "23:59"]);
    assert!(
        stdout(&out).contains("59 23 * * *"),
        "--at overrides expect"
    );

    let out = s.run(&["schedule", "backup", "--launchd"]);
    assert_eq!(code(&out), 0, "launchd: {}", combined(&out));
    let text = stdout(&out);
    assert!(
        text.contains("<string>com.lightsout.backup</string>"),
        "{text}"
    );
    assert!(text.contains("<integer>7</integer>"), "{text}");
    assert!(text.contains("<integer>15</integer>"), "{text}");
    assert!(
        !text.contains("--auto-repair"),
        "the plist is opt-in too: {text}"
    );

    let out = s.run(&["schedule", "backup", "--launchd", "--auto-repair"]);
    assert!(
        stdout(&out).contains("<string>--auto-repair</string>"),
        "the flag reaches the plist: {}",
        stdout(&out)
    );

    assert_eq!(code(&s.run(&["schedule", "ghost"])), 64);
}

#[test]
fn a_malformed_registry_fails_closed() {
    let s = Sandbox::new("um-bad-registry");
    s.write("home/.config/lightsout/jobs.json", "{not json\n");
    let out = s.run(&["list"]);
    assert_eq!(
        code(&out),
        2,
        "a bad registry is a hard error: {}",
        combined(&out)
    );
    assert!(
        stderr(&out).contains("registry"),
        "the error names the registry"
    );
    let out = s.run(&["job", "add", "backup", "--command", "true"]);
    assert_eq!(code(&out), 2, "job add refuses to overwrite it");
}

#[test]
fn init_home_writes_the_libraries_a_registered_job_sources() {
    let s = Sandbox::new("um-init-home");
    let out = s.run(&["init", "--home"]);
    assert_eq!(code(&out), 0, "init --home: {}", combined(&out));
    assert!(registry_home(&s).join("lib/guards.sh").is_file());
    assert!(registry_home(&s).join("lib/agent.sh").is_file());
    assert!(registry_home(&s).join("lightsout.toml").is_file());
    let out = s.run(&["init", "--home"]);
    assert_eq!(code(&out), 0);
    assert!(
        stdout(&out).contains("already initialized"),
        "a second init is idempotent: {}",
        combined(&out)
    );
}

#[test]
fn doctor_checks_the_registry_from_outside_a_workspace() {
    let s = Sandbox::new("um-doctor");
    assert_eq!(
        code(&s.run(&["job", "add", "backup", "--command", "true"])),
        0
    );
    let (dir, binary) = isolated_binary("doctor");
    let out = isolated_run(&s, &dir, &binary, &["doctor"]);
    assert_eq!(code(&out), 0, "doctor: {}", combined(&out));
    let text = combined(&out);
    assert!(text.contains("registry job 'backup'"), "{text}");
    let _ = fs::remove_dir_all(&dir);
}
