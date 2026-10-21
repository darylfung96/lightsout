//! The global job registry: a name for a job directory that lives outside any
//! one workspace.
//!
//! A workspace still owns `lib/`, `triage.conf` and the shared alert log, but a
//! job does not have to live under it. `lightsout job add` writes
//! `$LIGHTSOUT_HOME/jobs.json`, and `run`, `status`, `check`, `triage` and
//! `heal` resolve a name through it when the current workspace has no job of
//! that name. The registry is a locator, not a second state store: delete it
//! and every job's history is still under its own `state/`.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::workspace;

pub const SCHEMA: u32 = 1;
pub const FILE: &str = "jobs.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub job_dir: PathBuf,
    pub workspace: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
    pub workdir: Option<PathBuf>,
}

/// `LIGHTSOUT_HOME` wins so a test or a container can isolate the registry;
/// otherwise the XDG config directory, otherwise `~/.config/lightsout`.
pub fn home() -> PathBuf {
    if let Ok(explicit) = env::var("LIGHTSOUT_HOME") {
        if !explicit.is_empty() {
            return absolute(Path::new(&explicit));
        }
    }
    if let Ok(config) = env::var("XDG_CONFIG_HOME") {
        if !config.is_empty() {
            return absolute(&Path::new(&config).join("lightsout"));
        }
    }
    match env::var("HOME") {
        Ok(home) if !home.is_empty() => {
            absolute(&Path::new(&home).join(".config").join("lightsout"))
        }
        _ => PathBuf::from(".lightsout"),
    }
}

pub fn path() -> PathBuf {
    home().join(FILE)
}

fn absolute(path: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    fs::canonicalize(&path).unwrap_or(path)
}

/// A registered name is a launchd label segment and a CLI word, so it is
/// stricter than a workspace directory name: lower-case, digits and dash.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn load() -> Result<BTreeMap<String, Entry>, String> {
    let path = path();
    let body = match fs::read_to_string(&path) {
        Ok(body) => body,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    parse(&path, &body)
}

fn parse(path: &Path, body: &str) -> Result<BTreeMap<String, Entry>, String> {
    let value: Value = serde_json::from_str(body)
        .map_err(|error| format!("{}: not valid JSON: {error}", path.display()))?;
    let object = value
        .as_object()
        .ok_or_else(|| format!("{}: the top level must be an object", path.display()))?;
    let schema = object
        .get("schema")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{}: schema is missing", path.display()))?;
    if schema != u64::from(SCHEMA) {
        return Err(format!(
            "{}: schema {schema} is not supported (this binary writes {SCHEMA})",
            path.display()
        ));
    }
    let jobs = object
        .get("jobs")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{}: jobs is missing", path.display()))?;
    let mut out = BTreeMap::new();
    for (name, value) in jobs {
        if !valid_name(name) {
            return Err(format!(
                "{}: job '{name}' must be lower-case letters, digits or dash",
                path.display()
            ));
        }
        let entry = value
            .as_object()
            .ok_or_else(|| format!("{}: job '{name}' must be an object", path.display()))?;
        for key in entry.keys() {
            if !matches!(
                key.as_str(),
                "job_dir" | "workspace" | "state_dir" | "workdir"
            ) {
                return Err(format!(
                    "{}: job '{name}' has an unknown key '{key}'",
                    path.display()
                ));
            }
        }
        let job_dir = entry_path(path, name, entry, "job_dir")?
            .ok_or_else(|| format!("{}: job '{name}' has no job_dir", path.display()))?;
        out.insert(
            name.clone(),
            Entry {
                job_dir,
                workspace: entry_path(path, name, entry, "workspace")?,
                state_dir: entry_path(path, name, entry, "state_dir")?,
                workdir: entry_path(path, name, entry, "workdir")?,
            },
        );
    }
    Ok(out)
}

fn entry_path(
    path: &Path,
    name: &str,
    entry: &Map<String, Value>,
    key: &str,
) -> Result<Option<PathBuf>, String> {
    let Some(value) = entry.get(key) else {
        return Ok(None);
    };
    let text = value
        .as_str()
        .ok_or_else(|| format!("{}: job '{name}' has a non-string {key}", path.display()))?;
    let candidate = PathBuf::from(text);
    if !candidate.is_absolute() {
        return Err(format!(
            "{}: job '{name}' has a relative {key} '{text}'; the registry needs an absolute path",
            path.display()
        ));
    }
    Ok(Some(candidate))
}

pub fn save(jobs: &BTreeMap<String, Entry>) -> Result<(), String> {
    let home = home();
    fs::create_dir_all(&home)
        .map_err(|error| format!("cannot create {}: {error}", home.display()))?;
    let mut object = Map::new();
    for (name, entry) in jobs {
        let mut value = Map::new();
        value.insert(
            "job_dir".into(),
            json!(entry.job_dir.to_string_lossy().into_owned()),
        );
        if let Some(workspace) = &entry.workspace {
            value.insert(
                "workspace".into(),
                json!(workspace.to_string_lossy().into_owned()),
            );
        }
        if let Some(state_dir) = &entry.state_dir {
            value.insert(
                "state_dir".into(),
                json!(state_dir.to_string_lossy().into_owned()),
            );
        }
        if let Some(workdir) = &entry.workdir {
            value.insert(
                "workdir".into(),
                json!(workdir.to_string_lossy().into_owned()),
            );
        }
        object.insert(name.clone(), Value::Object(value));
    }
    let body = serde_json::to_string_pretty(&json!({
        "schema": SCHEMA,
        "jobs": Value::Object(object),
    }))
    .map_err(|error| format!("cannot encode the registry: {error}"))?;
    let path = path();
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, format!("{body}\n"))
        .map_err(|error| format!("cannot write {}: {error}", tmp.display()))?;
    fs::rename(&tmp, &path)
        .map_err(|error| format!("cannot replace {}: {error}", path.display()))?;
    Ok(())
}

/// Validate the file without acting on it. `main` calls this before every verb
/// so a malformed registry is a hard error instead of a silent fallback to the
/// workspace job of the same name.
pub fn check() -> Result<(), String> {
    load().map(|_| ())
}

pub fn lookup(name: &str) -> Option<Entry> {
    load().ok()?.get(name).cloned()
}

pub fn names() -> Vec<String> {
    load()
        .map(|jobs| jobs.keys().cloned().collect())
        .unwrap_or_default()
}

/// True when a non-empty registry exists, so a verb may work without a
/// workspace marker in the current directory.
pub fn is_configured() -> bool {
    load().map(|jobs| !jobs.is_empty()).unwrap_or(false)
}

pub fn cli(argv: &[String]) -> i32 {
    let Some(action) = argv.first().map(String::as_str) else {
        println!("{}", USAGE);
        return 64;
    };
    match action {
        "-h" | "--help" | "help" => {
            println!("{}", USAGE);
            0
        }
        "list" => list(),
        "remove" | "rm" => match argv.get(1) {
            Some(name) => remove(name),
            None => {
                eprintln!("job remove needs a name");
                64
            }
        },
        "add" => add(&argv[1..]),
        other => {
            eprintln!("unknown job action: {other}");
            println!("{}", USAGE);
            64
        }
    }
}

pub const USAGE: &str = "usage: lightsout job <action> [options]

  add <name> [--job-dir DIR] [--workspace DIR] [--workdir DIR]
             [--state-dir DIR] [--command CMD] [--force]
                       register a job; with --command, generate its job.sh
  list                 show the registry
  remove <name>        unregister a job; its files stay";

fn list() -> i32 {
    let jobs = match load() {
        Ok(jobs) => jobs,
        Err(error) => {
            eprintln!("registry: {error}");
            return 2;
        }
    };
    if jobs.is_empty() {
        println!("no registered jobs ({})", path().display());
        return 0;
    }
    for (name, entry) in jobs {
        println!("{name}\t{}", entry.job_dir.display());
    }
    0
}

fn remove(name: &str) -> i32 {
    let mut jobs = match load() {
        Ok(jobs) => jobs,
        Err(error) => {
            eprintln!("registry: {error}");
            return 2;
        }
    };
    let Some(entry) = jobs.remove(name) else {
        eprintln!("no such registered job: {name}");
        return 64;
    };
    if let Err(error) = save(&jobs) {
        eprintln!("registry: {error}");
        return 1;
    }
    println!(
        "removed {name} from the registry; files stay at {}",
        entry.job_dir.display()
    );
    0
}

struct AddOptions {
    name: String,
    job_dir: Option<PathBuf>,
    workspace: Option<PathBuf>,
    workdir: Option<PathBuf>,
    state_dir: Option<PathBuf>,
    command: Option<String>,
    force: bool,
}

fn add(argv: &[String]) -> i32 {
    let Some(name) = argv.first() else {
        println!("{}", USAGE);
        return 64;
    };
    if !valid_name(name) {
        eprintln!("job name '{name}' must be lower-case letters, digits or dash");
        return 64;
    }
    let mut options = AddOptions {
        name: name.clone(),
        job_dir: None,
        workspace: None,
        workdir: None,
        state_dir: None,
        command: None,
        force: false,
    };
    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];
        let mut take = |what: &str| -> Option<String> {
            match argv.get(i + 1) {
                Some(value) if !value.starts_with("--") => {
                    i += 1;
                    Some(value.clone())
                }
                _ => {
                    eprintln!("{what} needs a value");
                    None
                }
            }
        };
        match arg.as_str() {
            "--force" => options.force = true,
            "--job-dir" => match take("--job-dir") {
                Some(value) => options.job_dir = Some(absolute(Path::new(&value))),
                None => return 64,
            },
            "--workspace" => match take("--workspace") {
                Some(value) => options.workspace = Some(absolute(Path::new(&value))),
                None => return 64,
            },
            "--workdir" => match take("--workdir") {
                Some(value) => options.workdir = Some(absolute(Path::new(&value))),
                None => return 64,
            },
            "--state-dir" => match take("--state-dir") {
                Some(value) => options.state_dir = Some(absolute(Path::new(&value))),
                None => return 64,
            },
            "--command" => match take("--command") {
                Some(value) => options.command = Some(value),
                None => return 64,
            },
            other => {
                eprintln!("job add: unknown option: {other}");
                println!("{}", USAGE);
                return 64;
            }
        }
        i += 1;
    }

    let mut jobs = match load() {
        Ok(jobs) => jobs,
        Err(error) => {
            eprintln!("registry: {error}");
            return 2;
        }
    };
    if jobs.contains_key(&options.name) && !options.force {
        eprintln!(
            "job '{}' is already registered; pass --force to replace it",
            options.name
        );
        return 64;
    }

    let (job_dir, command) = match (&options.job_dir, &options.command) {
        (Some(dir), None) => {
            if !dir.join("job.sh").is_file() {
                eprintln!("job add: no job.sh in {}", dir.display());
                return 64;
            }
            (dir.clone(), None)
        }
        (None, Some(command)) => match generate(&options.name, command, options.force) {
            Ok(dir) => (dir, Some(command.clone())),
            Err(error) => {
                eprintln!("job add: {error}");
                return 1;
            }
        },
        (Some(dir), Some(_command)) => {
            eprintln!(
                "job add: --job-dir and --command cannot be combined; {} already has a job.sh",
                dir.display()
            );
            return 64;
        }
        (None, None) => {
            eprintln!("job add: give --job-dir DIR or --command CMD");
            return 64;
        }
    };

    let home = home();
    if options.workspace.is_none() && !job_dir.starts_with(&home) {
        // A job directory inside a workspace finds that workspace by walking
        // up; anything else falls back to LIGHTSOUT_HOME, which `job add`
        // initializes so `lib/guards.sh` exists.
        if let Err(error) = workspace::init_missing(&home) {
            eprintln!("job add: {error}");
            return 1;
        }
    }
    let workspace = options.workspace.or_else(|| {
        let parent = job_dir.parent()?;
        let grandparent = parent.parent()?;
        if parent.file_name()? == "jobs" && crate::repo::is_workspace(grandparent) {
            Some(grandparent.to_path_buf())
        } else {
            Some(home.clone())
        }
    });
    if let Some(workdir) = &options.workdir {
        if !workdir.is_dir() {
            eprintln!(
                "job add: --workdir is not a directory: {}",
                workdir.display()
            );
            return 64;
        }
    }
    let workdir = options.workdir.or_else(|| Some(job_dir.clone()));
    if let Some(workspace) = &workspace {
        if !crate::repo::is_workspace(workspace) {
            eprintln!(
                "job add: workspace is not a LightsOut workspace: {}",
                workspace.display()
            );
            return 64;
        }
        if !workspace.join("lib").join("guards.sh").is_file() {
            eprintln!(
                "job add: workspace has no lib/guards.sh: {}",
                workspace.display()
            );
            return 64;
        }
    }
    jobs.insert(
        options.name.clone(),
        Entry {
            job_dir: job_dir.clone(),
            workspace,
            state_dir: options.state_dir,
            workdir,
        },
    );
    if let Err(error) = save(&jobs) {
        eprintln!("registry: {error}");
        return 1;
    }
    println!(
        "registered {} -> {} (workspace {})",
        options.name,
        job_dir.display(),
        jobs.get(&options.name)
            .and_then(|entry| entry.workspace.as_ref())
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".into())
    );
    if let Some(command) = command {
        println!("command: {command}");
    }
    println!("next: lightsout run {}", options.name);
    0
}

fn generate(name: &str, command: &str, force: bool) -> Result<PathBuf, String> {
    if command.trim().is_empty() {
        return Err("--command is empty".into());
    }
    if command.chars().any(char::is_control) {
        return Err("--command must be one line without control characters".into());
    }
    let home = home();
    workspace::init_missing(&home)?;
    let dir = home.join("jobs").join(name);
    let job = dir.join("job.sh");
    if job.exists() && !force {
        return Err(format!(
            "{} already exists; pass --force to replace it",
            job.display()
        ));
    }
    fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    let body = format!(
        "#!/usr/bin/env bash\n\
         # {name} — a command job registered by `lightsout job add`.\n\
         #\n\
         # The command's exit code is the gate: 0 writes the done marker, any\n\
         # other code fails the run. Gates, memory and auto-repair stay opt-in;\n\
         # add them here when the command needs an artifact check.\n\
         set -euo pipefail\n\
         . \"${{LIGHTSOUT_LIB:-$REPO/lib}}/guards.sh\"\n\
         \n\
         u_say \"command job: {name}\"\n\
         {command}\n\
         u_done\n"
    );
    fs::write(&job, body).map_err(|error| format!("cannot write {}: {error}", job.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&job, fs::Permissions::from_mode(0o755));
    }
    let spec = dir.join("spec.md");
    if !spec.exists() || force {
        let body = format!(
            "# {name}\n\n\
             A command job registered with `lightsout job add`.\n\n\
             ## Done\n\n\
             The command exits 0.\n\n\
             ## Gates\n\n\
             - The command's own exit code.\n"
        );
        fs::write(&spec, body)
            .map_err(|error| format!("cannot write {}: {error}", spec.display()))?;
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registry_entry_round_trips() {
        let root = std::env::temp_dir().join(format!(
            "lightsout-registry-unit-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("jobs/backup")).unwrap();
        fs::write(root.join("jobs/backup/job.sh"), "#!/bin/sh\nexit 0\n").unwrap();
        let body = format!(
            "{{\"schema\":1,\"jobs\":{{\"backup\":{{\"job_dir\":\"{}\",\"workdir\":\"{}\"}}}}}}",
            root.join("jobs/backup").display(),
            root.display()
        );
        let jobs = parse(&root.join(FILE), &body).expect("parse");
        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs.get("backup").map(|entry| entry.job_dir.clone()),
            Some(root.join("jobs/backup"))
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_relative_path_and_an_unknown_key_are_refused() {
        let path = Path::new("/tmp/jobs.json");
        let relative = r#"{"schema":1,"jobs":{"backup":{"job_dir":"jobs/backup"}}}"#;
        assert!(parse(path, relative).unwrap_err().contains("absolute"));
        let unknown = r#"{"schema":1,"jobs":{"backup":{"job_dir":"/tmp/b","work_dir":"/tmp"}}}"#;
        assert!(parse(path, unknown).unwrap_err().contains("unknown key"));
    }

    #[test]
    fn a_name_must_be_a_launchd_safe_word() {
        assert!(valid_name("backup"));
        assert!(valid_name("nightly-2"));
        assert!(!valid_name("Backup"));
        assert!(!valid_name("a/b"));
        assert!(!valid_name("-x"));
    }
}
