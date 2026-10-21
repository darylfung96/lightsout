//! lightsout — one binary for the manager: the run contract, artifact gates,
//! the dashboard and the workspace lifecycle. `jobs/<name>/job.sh` stays shell.

mod clock;
mod commands;
mod diagnose;
mod gates;
mod heal;
mod repo;
mod state;
mod status;
mod triage;
mod workspace;

use std::env;
use std::path::{Path, PathBuf};
use std::process;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    process::exit(run(&argv));
}

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
        "list" => with_workspace(commands::list),
        "status" | "report" => with_workspace(|_| status::run(rest)),
        "check" => with_workspace(|root| commands::check(root, rest.first().map(String::as_str))),
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
    if repo::is_workspace(&root) {
        Ok(root)
    } else {
        eprintln!("not a LightsOut workspace: {}", root.display());
        eprintln!("  create one with: lightsout init <dir>");
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
