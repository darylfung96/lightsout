//! Workspace lifecycle: `init` writes a versioned, runnable office; `doctor`
//! checks the workspace and the tools its jobs actually need.
//!
//! The templates are embedded from this repository at compile time, so a
//! released binary and the workspace it creates cannot drift apart.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::agent;
use crate::repo;
use crate::state;

const USAGE: &str = "usage: lightsout init [--dry-run] [--home] [dir]
       lightsout doctor

init   write a new workspace (lightsout.toml, lib/, jobs/noop, spec/, bin/);
       --home initializes $LIGHTSOUT_HOME instead, for registered jobs
doctor check the workspace marker, shell libraries, jobs and dependencies

Global options: --workspace DIR  --version  --help
Exit: init 0 written or already initialized, 64 misuse or collision;
      doctor 0 healthy (warnings allowed), 1 a required check failed.";

const MARKER_HEADER: &str = "# LightsOut workspace marker. Written by `lightsout init`.\n\
# schema is the workspace contract; manager is the binary that wrote it.\n";

const GITIGNORE: &str = "logs/\n/state/\njobs/*/logs/\njobs/*/state/\njobs/*/out/\nout/\n*.tmp\n";

const JOB_LAUNCHER: &str = "#!/usr/bin/env bash\n\
# job — run the workspace manager through the installed lightsout binary.\n\
set -euo pipefail\n\
exec \"$(dirname \"$0\")/lightsout\" \"$@\"\n";

const LIGHTSOUT_LAUNCHER: &str = "#!/usr/bin/env bash\n\
# lightsout — find the installed manager; the binary owns the workspace root.\n\
set -euo pipefail\n\
if [ -n \"${LIGHTSOUT_BIN:-}\" ] && [ -x \"$LIGHTSOUT_BIN\" ]; then\n\
  exec \"$LIGHTSOUT_BIN\" \"$@\"\n\
fi\n\
if command -v lightsout >/dev/null 2>&1; then\n\
  exec lightsout \"$@\"\n\
fi\n\
repo=$(cd \"$(dirname \"$0\")/..\" && pwd)\n\
for candidate in \"$repo/target/release/lightsout\" \"$repo/target/debug/lightsout\"; do\n\
  if [ -x \"$candidate\" ]; then exec \"$candidate\" \"$@\"; fi\n\
done\n\
echo \"lightsout: not found; install it on PATH or set LIGHTSOUT_BIN\" >&2\n\
exit 1\n";

const REPORT_LAUNCHER: &str = "#!/usr/bin/env bash\n\
# report — render every job's state from its state files.\n\
set -euo pipefail\n\
exec \"$(dirname \"$0\")/job\" report \"$@\"\n";

const GUARDS: &str = include_str!("../lib/guards.sh");
const LOCKS: &str = include_str!("../lib/locks.sh");
const NOTIFY: &str = include_str!("../lib/notify.sh");
const AGENT: &str = include_str!("../lib/agent.sh");
const MEMORY: &str = include_str!("../lib/memory.sh");
const REPAIRS: &str = include_str!("../lib/repairs.sh");
const HEAL: &str = include_str!("../lib/heal.sh");
const REPAIR_PROMPT: &str = include_str!("../lib/prompts/repair.md");
const TRIAGE_CONF: &str = "\
# LightsOut triage policy. One rule per line: <job> <source> <action>.
# A missing file routes every block to a human; this sample is safe.
#
#   example-job        gate:duration  agent:apply
#   example-job        gate:sections  agent:propose
#   example-job        *              retry
#
# Actions: human, retry, repair:<name>, agent:propose, agent:apply.
# Precedence: exact, then (job, *), then (*, source), then (*, *).
* * human
";
const AGENTS_CONF: &str = "\
# Named agent profiles. A job opts in with jobs/<name>/agent.conf:
#   profile = default
#
# A profile names the runner and, for dsh, the patch that pins provider, model
# and effort. The manager records the resolved profile in plan, triage, doctor
# and the heal history. A missing job profile uses the runner's own default.
#
# [default]
# runner = dsh
# patch = ~/path/to/agent.patch.yml
";
const NOOP_JOB: &str = include_str!("../jobs/noop/job.sh");
const SCHEMA: &str = include_str!("../spec/SCHEMA.md");
const DIGEST_SPEC: &str = include_str!("../spec/examples/digest.md");
const SCAFFOLD: &str = include_str!("../bin/scaffold");

struct TemplateFile {
    rel: &'static str,
    body: String,
    mode: u32,
    optional_if_exists: bool,
}

fn templates() -> Vec<TemplateFile> {
    let marker = format!(
        "{MARKER_HEADER}schema = {}\nmanager = \"{}\"\n",
        repo::CURRENT_SCHEMA,
        repo::VERSION
    );
    vec![
        file("lib/guards.sh", GUARDS, 0o644),
        file("lib/locks.sh", LOCKS, 0o644),
        file("lib/notify.sh", NOTIFY, 0o644),
        file("lib/agent.sh", AGENT, 0o644),
        file("lib/memory.sh", MEMORY, 0o644),
        file("lib/repairs.sh", REPAIRS, 0o644),
        file("lib/heal.sh", HEAL, 0o644),
        file("lib/prompts/repair.md", REPAIR_PROMPT, 0o644),
        file("jobs/noop/job.sh", NOOP_JOB, 0o755),
        file("spec/SCHEMA.md", SCHEMA, 0o644),
        file("spec/examples/digest.md", DIGEST_SPEC, 0o644),
        file("bin/scaffold", SCAFFOLD, 0o755),
        file("bin/lightsout", LIGHTSOUT_LAUNCHER, 0o755),
        file("bin/job", JOB_LAUNCHER, 0o755),
        file("bin/report", REPORT_LAUNCHER, 0o755),
        file("triage.conf", TRIAGE_CONF, 0o644),
        file("agents.conf", AGENTS_CONF, 0o644),
        TemplateFile {
            rel: ".gitignore",
            body: GITIGNORE.to_string(),
            mode: 0o644,
            optional_if_exists: true,
        },
        TemplateFile {
            rel: repo::MARKER,
            body: marker,
            mode: 0o644,
            optional_if_exists: false,
        },
    ]
}

fn file(rel: &'static str, body: &str, mode: u32) -> TemplateFile {
    TemplateFile {
        rel,
        body: body.to_string(),
        mode,
        optional_if_exists: false,
    }
}

pub fn init(argv: &[String]) -> i32 {
    let mut target: Option<String> = None;
    let mut dry_run = false;
    let mut home = false;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--dry-run" => dry_run = true,
            "--home" => home = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return 0;
            }
            "--" => {
                if let Some(value) = argv.get(i + 1) {
                    if target.is_some() {
                        eprintln!("init: one directory at a time");
                        return 64;
                    }
                    target = Some(value.clone());
                    i += 1;
                }
            }
            other if other.starts_with('-') => {
                eprintln!("init: unknown option: {other}");
                println!("{USAGE}");
                return 64;
            }
            other => {
                if target.is_some() {
                    eprintln!("init: one directory at a time");
                    return 64;
                }
                target = Some(other.to_string());
            }
        }
        i += 1;
    }

    let target = if home {
        if target.is_some() {
            eprintln!("init: --home takes no directory");
            return 64;
        }
        crate::registry::home().display().to_string()
    } else {
        match target {
            Some(target) => target,
            None => env::var("LIGHTSOUT_REPO")
                .ok()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    env::current_dir()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|_| ".".into())
                }),
        }
    };
    let target = absolute(Path::new(&target));
    if target.exists() && !target.is_dir() {
        eprintln!("init: not a directory: {}", target.display());
        return 64;
    }

    if repo::has_marker(&target) {
        println!("already initialized: {}", target.display());
        println!("next: lightsout doctor");
        return 0;
    }

    let templates = templates();
    let mut collisions = Vec::new();
    for template in &templates {
        let path = target.join(template.rel);
        if path.exists() && !template.optional_if_exists {
            collisions.push(template.rel);
        }
    }
    if !collisions.is_empty() {
        eprintln!(
            "init: refusing to overwrite existing files in {}:",
            target.display()
        );
        for collision in collisions {
            eprintln!("  {collision}");
        }
        return 64;
    }

    if dry_run {
        println!("would initialize workspace at {}", target.display());
        for template in &templates {
            let path = target.join(template.rel);
            if path.exists() {
                println!("  keep {}", template.rel);
            } else {
                println!("  create {}", template.rel);
            }
        }
        println!("dry run: no files written");
        return 0;
    }

    if let Err(error) = fs::create_dir_all(&target) {
        eprintln!("init: cannot create {}: {error}", target.display());
        return 1;
    }
    let mut created: Vec<PathBuf> = Vec::new();
    for template in &templates {
        let path = target.join(template.rel);
        if path.exists() && template.optional_if_exists {
            println!("kept existing {}", template.rel);
            continue;
        }
        if let Err(error) = write_template(&path, &template.body, template.mode) {
            eprintln!("init: cannot write {}: {error}", path.display());
            // A partial office is worse than none: a retry would refuse the
            // files this attempt already wrote. Remove only what this run
            // created and leave anything that predated it untouched.
            let _ = fs::remove_file(&path);
            for path in created.iter().rev() {
                let _ = fs::remove_file(path);
            }
            return 1;
        }
        created.push(path);
        println!("created {}", template.rel);
    }

    println!("initialized workspace at {}", target.display());
    println!("next:");
    println!("  cd {}", shell_quote(&target.display().to_string()));
    println!("  lightsout doctor");
    println!("  lightsout run noop");
    0
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

/// Write every template that is missing and never overwrite one that exists.
/// `job add` uses this so a generated command job can source `lib/guards.sh`
/// from `LIGHTSOUT_HOME` without asking the operator to run `init` first.
pub fn init_missing(root: &Path) -> Result<(), String> {
    fs::create_dir_all(root)
        .map_err(|error| format!("cannot create {}: {error}", root.display()))?;
    for template in templates() {
        let path = root.join(template.rel);
        if path.exists() {
            continue;
        }
        write_template(&path, &template.body, template.mode)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_-./:@".contains(c))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn write_template(path: &Path, body: &str, mode: u32) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

pub fn doctor(root: &Path) -> i32 {
    let mut errors = 0usize;
    let mut warnings = 0usize;

    if !root.is_dir() {
        println!("workspace {}", root.display());
        fail(&mut errors, &format!("not a directory: {}", root.display()));
        return finish(errors, warnings);
    }

    // Outside a workspace the registry is the whole picture, so `doctor` can
    // still answer whether the registered jobs are runnable.
    if !repo::is_workspace(root) && crate::registry::is_configured() {
        println!("registry {}", crate::registry::path().display());
        check_registry(root, &mut errors);
        return finish(errors, warnings);
    }
    println!("workspace {}", root.display());

    if repo::has_marker(root) {
        let body = state::read_lossy(&root.join(repo::MARKER));
        match parse_marker(&body) {
            Some((schema, manager)) => {
                if schema == repo::CURRENT_SCHEMA {
                    ok(&format!(
                        "marker schema {schema}, manager {}",
                        manager.as_deref().unwrap_or("unknown")
                    ));
                } else if schema > repo::CURRENT_SCHEMA {
                    fail(
                        &mut errors,
                        &format!(
                            "workspace schema {schema} is newer than this binary supports ({})",
                            repo::CURRENT_SCHEMA
                        ),
                    );
                } else {
                    warning(
                        &mut warnings,
                        &format!(
                            "workspace schema {schema} is older than {}; re-run lightsout init or upgrade",
                            repo::CURRENT_SCHEMA
                        ),
                    );
                }
                if let Some(manager) = manager {
                    if manager != repo::VERSION {
                        warning(
                            &mut warnings,
                            &format!(
                                "workspace was written by lightsout {manager}; this binary is {}",
                                repo::VERSION
                            ),
                        );
                    }
                } else {
                    warning(&mut warnings, "marker has no manager version");
                }
            }
            None => fail(
                &mut errors,
                &format!("cannot parse {}: expected schema = 1", repo::MARKER),
            ),
        }
    } else if repo::is_workspace(root) {
        warning(
            &mut warnings,
            "legacy workspace has no lightsout.toml; run lightsout init to add the marker",
        );
    } else {
        fail(
            &mut errors,
            "not a LightsOut workspace; run `lightsout init <dir>` first",
        );
        return finish(errors, warnings);
    }

    for rel in [
        "lib/guards.sh",
        "lib/locks.sh",
        "lib/notify.sh",
        "lib/agent.sh",
        "lib/memory.sh",
        "lib/repairs.sh",
        "lib/heal.sh",
        "lib/prompts/repair.md",
    ] {
        require_file(root, rel, &mut errors);
    }

    let policy_path = root.join(crate::triage::POLICY);
    if policy_path.is_file() {
        match crate::triage::Policy::load(&policy_path) {
            Ok(policy) => match policy.check_repairs(root) {
                Ok(()) => {
                    ok("triage.conf");
                    for message in policy.warnings(root) {
                        warning(&mut warnings, &message);
                    }
                    if policy.has_agent() {
                        let runner = env::var("U_HEAL_RUNNER")
                            .ok()
                            .filter(|value| !value.is_empty())
                            .or_else(|| env::var("U_RUNNER").ok().filter(|value| !value.is_empty()))
                            .unwrap_or_else(|| "dsh".into());
                        if repo::which(&runner).is_some() {
                            ok(&format!("agent runner {runner}"));
                        } else {
                            warning(
                                &mut warnings,
                                &format!(
                                    "agent runner '{runner}' is not installed; heal and auto-repair will fail"
                                ),
                            );
                        }
                        warning(
                            &mut warnings,
                            &format!(
                                "containment: tripwire only for runner {runner}; adapter-level read-only is not implemented"
                            ),
                        );
                    }
                }
                Err(error) => fail(&mut errors, &format!("triage.conf: {error}")),
            },
            Err(error) => fail(&mut errors, &format!("triage.conf: {error}")),
        }
    } else {
        warning(
            &mut warnings,
            "triage.conf is missing; every block routes to a human",
        );
    }

    if root.join(agent::PROFILES).is_file() {
        match agent::workspace_profiles(root) {
            Ok(names) => ok(&format!(
                "agents.conf ({} named profile{})",
                names.len(),
                if names.len() == 1 { "" } else { "s" }
            )),
            Err(error) if error.is_unavailable() => {
                warning(&mut warnings, &format!("agents.conf: {error}"))
            }
            Err(error) => fail(&mut errors, &format!("agents.conf: {error}")),
        }
    }

    let jobs_dir = root.join("jobs");
    let mut job_bodies: Vec<(String, String)> = Vec::new();
    if !jobs_dir.is_dir() {
        fail(&mut errors, "jobs/ is missing");
    } else {
        let mut names: Vec<String> = fs::read_dir(&jobs_dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        if names.is_empty() {
            warning(
                &mut warnings,
                "jobs/ has no jobs; create one with bin/scaffold",
            );
        }
        for name in names {
            let job = jobs_dir.join(&name).join("job.sh");
            if job.is_file() {
                ok(&format!("jobs/{name}/job.sh"));
                check_repair_allow(root, &name, &mut errors);
                check_agent_profile(root, &name, &mut errors, &mut warnings);
                check_expect(root, &name, &mut errors);
                job_bodies.push((name, state::read_lossy(&job)));
            } else {
                warning(&mut warnings, &format!("jobs/{name}/ has no job.sh"));
            }
        }
    }

    if root.join("bin/scaffold").is_file() {
        ok("bin/scaffold");
    } else {
        warning(
            &mut warnings,
            "bin/scaffold is missing; creating new jobs is harder",
        );
    }
    if !root.join("spec/SCHEMA.md").is_file() {
        warning(&mut warnings, "spec/SCHEMA.md is missing");
    }

    if repo::which("bash").is_some() {
        ok("bash");
    } else {
        fail(&mut errors, "bash is required to run jobs");
    }

    let needs_python = job_bodies.iter().any(|(_, body)| {
        body.contains("python3") || body.contains("U_MEMORY=1") || body.contains("U_MEMORY=\"1\"")
    });
    match repo::which("python3") {
        Some(path) => ok(&format!("python3 ({path})")),
        None if needs_python => fail(
            &mut errors,
            "python3 is required by a job (python3 or a memory pass)",
        ),
        None => warning(
            &mut warnings,
            "python3 is missing; memory gates and some jobs will not run",
        ),
    }

    let needs_media = job_bodies.iter().any(|(_, body)| {
        ["duration", "resolution", "loudness", "ffprobe", "ffmpeg"]
            .iter()
            .any(|needle| body.contains(needle))
    });
    if needs_media {
        for tool in ["ffprobe", "ffmpeg"] {
            match repo::which(tool) {
                Some(path) => ok(&format!("{tool} ({path})")),
                None => warning(
                    &mut warnings,
                    &format!("{tool} is missing; duration/resolution/loudness gates will fail"),
                ),
            }
        }
    }

    #[cfg(target_os = "macos")]
    if repo::which("osascript").is_none() {
        warning(
            &mut warnings,
            "osascript is missing; desktop notifications will be skipped",
        );
    }

    check_registry(root, &mut errors);

    finish(errors, warnings)
}

/// Registered jobs are checked from any workspace, because a name that
/// resolves nowhere is worse than a missing local job.
fn check_registry(_root: &Path, errors: &mut usize) {
    let jobs = match crate::registry::load() {
        Ok(jobs) => jobs,
        Err(error) => {
            fail(errors, &format!("registry: {error}"));
            return;
        }
    };
    for (name, entry) in jobs {
        if !entry.job_dir.join("job.sh").is_file() {
            fail(
                errors,
                &format!(
                    "registry job '{name}' has no job.sh at {}",
                    entry.job_dir.display()
                ),
            );
            continue;
        }
        if let Some(workspace) = &entry.workspace {
            if !repo::is_workspace(workspace) {
                fail(
                    errors,
                    &format!(
                        "registry job '{name}' workspace is not a LightsOut workspace: {}",
                        workspace.display()
                    ),
                );
                continue;
            }
            if !workspace.join("lib").join("guards.sh").is_file() {
                fail(
                    errors,
                    &format!(
                        "registry job '{name}' workspace has no lib/guards.sh: {}",
                        workspace.display()
                    ),
                );
                continue;
            }
        }
        ok(&format!(
            "registry job '{name}' -> {}",
            entry.job_dir.display()
        ));
    }
}

fn check_agent_profile(root: &Path, job: &str, errors: &mut usize, warnings: &mut usize) {
    let path = root.join("jobs").join(job).join(agent::JOB_PROFILE);
    if !path.is_file() {
        return;
    }
    match agent::resolve_config(root, job) {
        Ok(profile) => {
            ok(&format!(
                "agent profile jobs/{job}/{}: {}",
                agent::JOB_PROFILE,
                profile.describe()
            ));
            if profile.runner == "fake" {
                if root.join("tests/fake-agent.sh").is_file() {
                    ok("agent runner fake (repository test double)");
                } else {
                    warning(
                        warnings,
                        "agent runner fake is configured but tests/fake-agent.sh is missing",
                    );
                }
            } else if repo::which(&profile.runner).is_none() {
                warning(
                    warnings,
                    &format!(
                        "agent runner '{}' is not installed; heal and auto-repair will fail",
                        profile.runner
                    ),
                );
            } else {
                ok(&format!("agent runner {}", profile.runner));
            }
            if profile.runner == "dsh" && profile.patch.is_none() {
                warning(
                    warnings,
                    "agent profile has no dsh patch; the runner's default profile and model are used",
                );
            }
            if profile.model.is_none() && profile.runner != "fake" {
                warning(
                    warnings,
                    "agent profile pins no model; the runner's ambient default is used",
                );
            }
        }
        Err(error) if error.is_unavailable() => {
            warning(warnings, &format!("agent profile: {error}"));
        }
        Err(error) => fail(errors, &format!("agent profile: {error}")),
    }
}

fn finish(errors: usize, warnings: usize) -> i32 {
    let noun = if errors == 1 { "error" } else { "errors" };
    let warning_noun = if warnings == 1 { "warning" } else { "warnings" };
    println!("doctor: {errors} {noun}, {warnings} {warning_noun}");
    if errors > 0 {
        1
    } else {
        0
    }
}

fn parse_marker(body: &str) -> Option<(u32, Option<String>)> {
    let mut schema = None;
    let mut manager = None;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once('=')?;
        let key = key.trim();
        let value = value.trim().trim_matches('"');
        match key {
            "schema" => schema = value.parse::<u32>().ok(),
            "manager" => manager = Some(value.to_string()),
            _ => {}
        }
    }
    schema.map(|schema| (schema, manager))
}

fn check_expect(root: &Path, job: &str, errors: &mut usize) {
    let state = root.join("jobs").join(job).join("state");
    let path = state.join("expect");
    if !path.is_file() {
        return;
    }
    if crate::state::expect_time(&state).is_some() {
        ok(&format!("jobs/{job}/state/expect"));
    } else {
        fail(
            errors,
            &format!("jobs/{job}/state/expect is not HH:MM; the stale check cannot run"),
        );
    }
}

fn check_repair_allow(root: &Path, job: &str, errors: &mut usize) {
    let path = root.join("jobs").join(job).join("repair.allow");
    if !path.is_file() {
        return;
    }
    let body = state::read_lossy(&path);
    let mut bad = 0usize;
    for (index, line) in body.lines().enumerate() {
        let text = line.split('#').next().unwrap_or("").trim();
        if text.is_empty() {
            continue;
        }
        let prefix = Path::new(text);
        let escapes = prefix.is_absolute()
            || text.contains('\\')
            || prefix.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                        | std::path::Component::CurDir
                )
            });
        if escapes {
            bad += 1;
            fail(
                errors,
                &format!(
                    "jobs/{job}/repair.allow:{}: prefix must stay inside jobs/{job}/",
                    index + 1
                ),
            );
        }
    }
    if bad == 0 {
        ok(&format!("jobs/{job}/repair.allow"));
    }
}

fn require_file(root: &Path, rel: &str, errors: &mut usize) {
    if root.join(rel).is_file() {
        ok(rel);
    } else {
        fail(errors, &format!("{rel} is missing"));
    }
}

fn ok(message: &str) {
    println!("ok   {message}");
}

fn warning(warnings: &mut usize, message: &str) {
    *warnings += 1;
    println!("warn {message}");
}

fn fail(errors: &mut usize, message: &str) {
    *errors += 1;
    println!("FAIL {message}");
}
