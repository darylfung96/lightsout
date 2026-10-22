//! lightsout — one binary for the manager: the run contract, artifact gates,
//! the dashboard and the workspace lifecycle. `jobs/<name>/job.sh` stays shell.

mod agent;
mod clock;
mod commands;
mod diagnose;
mod gates;
mod heal;
mod registry;
mod repo;
mod schedule;
mod spec;
mod state;
mod status;
mod triage;
mod workspace;

use std::env;
use std::path::{Path, PathBuf};
use std::process;

fn main() {
    restore_sigpipe();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    process::exit(run(&argv));
}

/// Rust ignores `SIGPIPE`, so a reader that closes early (`status | head`)
/// would make the next `println!` panic on a broken stdout. The manager prints
/// reports a person pages through, so it restores the default action and dies
/// quietly the way `cat` does instead of printing a panic into the pipeline.
#[cfg(unix)]
fn restore_sigpipe() {
    extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
    }
    const SIGPIPE: i32 = 13;
    const SIG_DFL: usize = 0;
    // SAFETY: only the default disposition is installed, and `signal` stores
    // it without dereferencing anything the process owns.
    unsafe {
        signal(SIGPIPE, SIG_DFL);
    }
}

#[cfg(not(unix))]
fn restore_sigpipe() {}

fn run(argv: &[String]) -> i32 {
    let mut index = 0;
    let mut requested_workspace: Option<String> = None;
    while index < argv.len() {
        match argv[index].as_str() {
            "--version" | "-V" => {
                println!("lightsout {}", repo::VERSION);
                return 0;
            }
            "--workspace" => {
                let Some(value) = argv.get(index + 1) else {
                    eprintln!("--workspace needs a directory");
                    return 64;
                };
                requested_workspace = Some(value.clone());
                index += 2;
            }
            _ if argv[index].starts_with("--workspace=") => {
                requested_workspace = Some(argv[index]["--workspace=".len()..].to_string());
                index += 1;
            }
            _ => break,
        }
    }

    if let Some(directory) = requested_workspace {
        let path = Path::new(&directory);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };
        if path.exists() && !path.is_dir() {
            eprintln!("--workspace is not a directory: {}", path.display());
            return 64;
        }
        env::set_var("LIGHTSOUT_REPO", path);
    }

    let argv = &argv[index..];
    let Some(verb) = argv.first().map(String::as_str) else {
        println!("{}", commands::USAGE);
        return 0;
    };
    let rest = &argv[1..];
    // A malformed registry is a hard error for any verb that resolves a job:
    // falling back to a workspace job of the same name would run the wrong
    // thing. `init`, `gate` and `job` handle their own files.
    if !matches!(
        verb,
        "help" | "-h" | "--help" | "version" | "init" | "gate" | "gates" | "job"
    ) {
        if let Err(error) = registry::check() {
            eprintln!("registry: {error}");
            return 2;
        }
    }
    match verb {
        "help" | "-h" | "--help" => {
            println!("{}", commands::USAGE);
            0
        }
        "version" => {
            println!("lightsout {}", repo::VERSION);
            0
        }
        "init" => workspace::init(rest),
        "doctor" => workspace::doctor(&repo::root()),
        "gate" | "gates" => gates::run(rest),
        "job" => registry::cli(rest),
        "list" => with_workspace(commands::list),
        "status" | "report" => with_workspace(|_| status::run(rest)),
        "check" => with_workspace(|root| commands::check(root, rest.first().map(String::as_str))),
        "schedule" => with_workspace(|root| schedule::run(root, rest)),
        "plan" => match rest.first() {
            Some(name) => with_workspace(|root| commands::plan(root, name)),
            None => {
                println!("{}", commands::USAGE);
                64
            }
        },
        "diagnose" => match rest.first() {
            Some(name) => with_workspace(|root| diagnose::run(root, name)),
            None => {
                println!("{}", commands::USAGE);
                64
            }
        },
        "unblock" => match rest.first() {
            Some(name) => with_workspace(|root| commands::unblock(root, name)),
            None => {
                println!("{}", commands::USAGE);
                64
            }
        },
        "retry" => with_workspace(|root| commands::retry(root, rest)),
        "heal" => with_workspace(|root| commands::heal(root, rest)),
        "triage" => with_workspace(|root| triage::run(root, rest)),
        "run" | "resume" => run_verb(verb, rest),
        _ => {
            println!("{}", commands::USAGE);
            64
        }
    }
}

fn with_workspace(body: impl FnOnce(&Path) -> i32) -> i32 {
    match workspace_root() {
        Ok(root) => body(&root),
        Err(code) => code,
    }
}

fn workspace_root() -> Result<PathBuf, i32> {
    let root = repo::root();
    // A registry entry names its own job directory and workspace, so the
    // current directory does not have to be a workspace for a registered job.
    if repo::is_workspace(&root) || registry::is_configured() {
        Ok(root)
    } else {
        eprintln!("not a LightsOut workspace: {}", root.display());
        eprintln!("  create one with: lightsout init <dir>");
        eprintln!("  or register a job with: lightsout job add <name> --job-dir <dir>");
        Err(64)
    }
}

fn run_verb(verb: &str, argv: &[String]) -> i32 {
    let Some(name) = argv.first() else {
        println!("{}", commands::USAGE);
        return 64;
    };
    let mut class: Option<String> = None;
    let mut source: Option<String> = None;
    let mut auto = std::env::var("LIGHTSOUT_AUTO_REPAIR").ok().as_deref() == Some("1");
    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];
        match arg.as_str() {
            "--auto-repair" => auto = true,
            "--class" => {
                let Some(value) = argv.get(i + 1) else {
                    eprintln!("--class needs a value");
                    return 64;
                };
                class = Some(value.clone());
                i += 1;
            }
            "--source" => {
                let Some(value) = argv.get(i + 1) else {
                    eprintln!("--source needs a value");
                    return 64;
                };
                source = Some(value.clone());
                i += 1;
            }
            _ if arg.starts_with("--class=") => {
                class = Some(arg["--class=".len()..].to_string());
            }
            _ if arg.starts_with("--source=") => {
                source = Some(arg["--source=".len()..].to_string());
            }
            other => {
                eprintln!("unknown option: {other}");
                println!("{}", commands::USAGE);
                return 64;
            }
        }
        i += 1;
    }
    let inherited = std::env::var("U_BLOCK_CLASS").ok();
    let effective = class
        .as_deref()
        .filter(|c| !c.is_empty())
        .or_else(|| inherited.as_deref().filter(|c| !c.is_empty()))
        .unwrap_or("human");
    if !matches!(effective, "retryable" | "judgment" | "human") {
        eprintln!("--class must be retryable, judgment or human");
        return 64;
    }
    with_workspace(|root| {
        commands::run(
            root,
            name,
            if verb == "resume" { "resume" } else { "run" },
            class.as_deref(),
            source.as_deref(),
            auto,
        )
    })
}
