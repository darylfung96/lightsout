//! Shared harness for the ported shell suite.
//!
//! Every test gets its own throwaway repository with the real shell jobs and
//! libraries, the real `tests/fake-*` doubles, and the just-built `lightsout`
//! binary copied to `bin/lightsout`. That keeps `cargo test` parallel-safe and
//! leaves the working tree untouched, while still exercising the same files
//! `tests/run.sh` exercised.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::env;
use std::fmt::Display;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The number of `ok` checks each task's section produced in tests/run.sh.
/// A test fails if it loses or duplicates one.
pub const EXPECTED: [usize; 23] = [
    3, 2, 5, 3, 2, 2, 1, 9, 3, 14, 25, 3, 11, 10, 13, 13, 13, 16, 16, 14, 29, 28, 26,
];

pub const TOTAL_CHECKS: usize = 261;

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct Sandbox {
    pub root: PathBuf,
}

impl Sandbox {
    pub fn new(label: &str) -> Self {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = env::temp_dir().join(format!(
            "lightsout-rs-{}-{}-{unique}-{label}",
            std::process::id(),
            nanos
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("sandbox");
        for dir in [
            "bin", "logs", "tmp", "home", "jobs", "lib", "notes", "tests", "spec",
        ] {
            fs::create_dir_all(root.join(dir)).expect("sandbox dir");
        }

        copy_tree(&source.join("lib"), &root.join("lib"));
        copy_tree(&source.join("notes"), &root.join("notes"));
        copy_tree(&source.join("spec"), &root.join("spec"));
        copy_tree(
            &source.join("tests/fake-pi-memory"),
            &root.join("tests/fake-pi-memory"),
        );
        copy_file(
            &source.join("tests/fake-agent.sh"),
            &root.join("tests/fake-agent.sh"),
        );
        copy_file(&source.join("bin/scaffold"), &root.join("bin/scaffold"));
        copy_file(
            &source.join("jobs/noop/job.sh"),
            &root.join("jobs/noop/job.sh"),
        );
        copy_file(
            &source.join("jobs/everyday-laws/job.sh"),
            &root.join("jobs/everyday-laws/job.sh"),
        );
        copy_file(
            &source.join("jobs/everyday-laws/spec.md"),
            &root.join("jobs/everyday-laws/spec.md"),
        );
        for rel in [
            "jobs/video-prune/job.sh",
            "jobs/video-prune/prune.py",
            "jobs/video-prune/spec.md",
        ] {
            copy_file(&source.join(rel), &root.join(rel));
        }
        for rel in ["jobs/everyday-laws/agent.conf", "agents.conf"] {
            let from = source.join(rel);
            if from.is_file() {
                copy_file(&from, &root.join(rel));
            }
        }
        fs::create_dir_all(root.join("jobs/noop")).unwrap();
        fs::create_dir_all(root.join("jobs/everyday-laws")).unwrap();

        let binary = PathBuf::from(env!("CARGO_BIN_EXE_lightsout"));
        copy_file(&binary, &root.join("bin/lightsout"));
        write_exec(
            &root.join("bin/job"),
            "#!/usr/bin/env bash\nexec \"$(dirname \"$0\")/lightsout\" \"$@\"\n",
        );
        write_exec(
            &root.join("bin/report"),
            "#!/usr/bin/env bash\nexec \"$(dirname \"$0\")/job\" report \"$@\"\n",
        );
        // The copied scripts keep their modes on Unix; assert the two that
        // matter for shell invocation.
        for path in [
            root.join("tests/fake-agent.sh"),
            root.join("tests/fake-pi-memory/qmd-mem.sh"),
            root.join("bin/scaffold"),
        ] {
            make_executable(&path);
        }
        for entry in fs::read_dir(root.join("tests/fake-pi-memory"))
            .unwrap()
            .flatten()
        {
            make_executable(&entry.path());
        }

        Sandbox { root }
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    pub fn reset(&self, job: &str) {
        for leaf in ["state", "out", "logs"] {
            let _ = fs::remove_dir_all(self.root.join("jobs").join(job).join(leaf));
        }
    }

    pub fn reset_noop(&self) {
        self.reset("noop");
    }

    pub fn command(&self) -> Command {
        let mut cmd = Command::new(self.root.join("bin/lightsout"));
        self.configure(&mut cmd);
        cmd
    }

    pub fn external(&self, program: &str) -> Command {
        let mut cmd = Command::new(program);
        self.configure(&mut cmd);
        cmd
    }

    fn configure(&self, cmd: &mut Command) {
        cmd.current_dir(&self.root);
        for (key, _) in env::vars_os() {
            let key = key.to_string_lossy();
            if key.starts_with("U_")
                || key.starts_with("NOOP_")
                || key.starts_with("FAKE_")
                || key.starts_with("MAX_")
                || key.starts_with("VIDEO_")
                || key.starts_with("NOTIFY_")
                || key.starts_with("AGENT_")
                || key.starts_with("LIGHTSOUT_")
                || matches!(
                    key.as_ref(),
                    "REPO" | "JOB" | "JOB_DIR" | "STATE" | "OUT" | "LOG" | "UNIT" | "DRY"
                )
            {
                cmd.env_remove(key.as_ref());
            }
        }
        cmd.env("U_NOTIFY", "0");
        cmd.env("U_MEMORY", "0");
        cmd.env("LIGHTSOUT_REPO", &self.root);
        cmd.env("TMPDIR", self.root.join("tmp"));
        cmd.env("HOME", self.root.join("home"));
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.run_env(args, &[])
    }

    pub fn run_env(&self, args: &[&str], vars: &[(&str, &str)]) -> Output {
        let mut cmd = self.command();
        cmd.args(args);
        for (key, value) in vars {
            cmd.env(key, value);
        }
        cmd.output().expect("run lightsout")
    }

    /// Start `bash -c <script> _ <args...>` in the sandbox.
    pub fn bash(&self, script: &str, vars: &[(&str, &str)], args: &[&str]) -> Output {
        let mut cmd = self.external("bash");
        cmd.arg("-c").arg(script).arg("_");
        cmd.args(args);
        for (key, value) in vars {
            cmd.env(key, value);
        }
        cmd.output().expect("run bash")
    }

    pub fn spawn_bash(&self, script: &str, args: &[&str]) -> std::process::Child {
        let mut cmd = self.external("bash");
        cmd.arg("-c").arg(script).arg("_");
        cmd.args(args);
        cmd.spawn().expect("spawn bash")
    }

    pub fn spawn_sleep(&self, seconds: &str) -> std::process::Child {
        let mut cmd = self.external("sleep");
        cmd.arg(seconds);
        cmd.spawn().expect("spawn sleep")
    }

    pub fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path(rel)).unwrap_or_default()
    }

    pub fn read_bytes(&self, rel: &str) -> Vec<u8> {
        fs::read(self.path(rel)).unwrap_or_default()
    }

    pub fn write(&self, rel: &str, body: &str) {
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    pub fn touch(&self, rel: &str) {
        self.write(rel, "");
    }

    pub fn append(&self, rel: &str, body: &str) {
        use std::io::Write;
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap()
            .write_all(body.as_bytes())
            .unwrap();
    }

    pub fn mkdir(&self, rel: &str) {
        fs::create_dir_all(self.path(rel)).unwrap();
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.path(rel).exists()
    }

    pub fn is_file(&self, rel: &str) -> bool {
        self.path(rel).is_file()
    }

    pub fn is_dir(&self, rel: &str) -> bool {
        self.path(rel).is_dir()
    }

    pub fn is_executable(&self, rel: &str) -> bool {
        executable(&self.path(rel))
    }

    pub fn status_json(&self) -> serde_json::Value {
        let output = self.run(&["status", "--all", "--json"]);
        assert_eq!(code(&output), 0, "status --json: {}", combined(&output));
        serde_json::from_slice(&output.stdout).expect("status --json parses")
    }

    pub fn row(&self, name: &str) -> serde_json::Value {
        self.status_json()
            .as_array()
            .expect("array")
            .iter()
            .find(|row| row.get("name").and_then(|v| v.as_str()) == Some(name))
            .unwrap_or_else(|| panic!("no status row for {name}"))
            .clone()
    }

    pub fn today(&self) -> String {
        date_arg("+%F")
    }

    pub fn now_log_stamp(&self) -> String {
        date_arg("+%F %T")
    }

    pub fn now_epoch(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    pub fn count_files(&self, rel: &str) -> usize {
        count_files(&self.path(rel))
    }

    pub fn count_prefix(&self, rel: &str, prefix: &str) -> usize {
        let Ok(entries) = fs::read_dir(self.path(rel)) else {
            return 0;
        };
        entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
            .count()
    }

    pub fn count_lines(&self, rel: &str) -> usize {
        self.read(rel).lines().count()
    }

    pub fn count_containing(&self, rel: &str, needle: &str) -> usize {
        self.read(rel)
            .lines()
            .filter(|line| line.contains(needle))
            .count()
    }

    pub fn has_line(&self, rel: &str, exact: &str) -> bool {
        self.read(rel).lines().any(|line| line == exact)
    }

    pub fn alerts(&self) -> String {
        self.read("logs/alerts.log")
    }

    pub fn last_line(&self, rel: &str) -> String {
        self.read(rel).lines().last().unwrap_or("").to_string()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if env::var_os("LIGHTSOUT_KEEP_TESTS").is_none() {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

pub struct Checks {
    pub count: usize,
    task: usize,
}

impl Checks {
    pub fn new(task: usize) -> Self {
        Checks { count: 0, task }
    }

    pub fn check<W: Display, G: Display>(&mut self, label: &str, want: W, got: G) {
        self.count += 1;
        let want = want.to_string();
        let got = got.to_string();
        if want == got {
            println!("ok   {label}");
        } else {
            panic!(
                "FAIL {label} (want {want}, got {got}) [task {} check {}]",
                self.task, self.count
            );
        }
    }

    pub fn is_true(&mut self, label: &str, value: bool) {
        self.check(label, 0, if value { 0 } else { 1 });
    }

    pub fn finish(self, task: usize) {
        let expected = EXPECTED[task - 1];
        assert_eq!(
            self.count, expected,
            "task {task}: expected {expected} ported checks, ran {}",
            self.count
        );
    }
}

pub fn code(output: &Output) -> i32 {
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

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn combined(output: &Output) -> String {
    format!("{}{}", stdout(output), stderr(output))
}

pub fn field(line: &str, n: usize) -> String {
    line.split('\t').nth(n).unwrap_or("").to_string()
}

pub fn line_n(rel: &str, sandbox: &Sandbox, n: usize) -> String {
    sandbox.read(rel).lines().nth(n).unwrap_or("").to_string()
}

pub fn count_files(root: &Path) -> usize {
    fn walk(dir: &Path, n: &mut usize) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(t) if t.is_dir() => walk(&entry.path(), n),
                Ok(_) => *n += 1,
                Err(_) => {}
            }
        }
    }
    let mut n = 0;
    walk(root, &mut n);
    n
}

pub fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                walk(base, &path, out);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                out.insert(rel, fs::read(&path).unwrap_or_default());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

pub fn date_arg(format: &str) -> String {
    let output = Command::new("date").arg(format).output().expect("date");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

pub fn write_exec(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    make_executable(path);
}

pub fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut mode = fs::metadata(path)
            .map(|m| m.permissions().mode())
            .unwrap_or(0o644);
        mode |= 0o755;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }
}

pub fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

pub fn copy_file(from: &Path, to: &Path) {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::copy(from, to)
        .unwrap_or_else(|e| panic!("copy {} -> {}: {e}", from.display(), to.display()));
    make_executable(to);
}

pub fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            copy_tree(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
            make_executable(&target);
        }
    }
}

pub fn io_text(result: io::Result<String>) -> String {
    result.unwrap_or_default()
}
