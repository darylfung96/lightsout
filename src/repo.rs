//! Where the workspace is, and where a job's files live.
//!
//! A workspace is a directory with a `lightsout.toml` marker. Older checkouts
//! without the marker are still accepted when they have `jobs/` and `lib/`, so
//! the manager keeps working in this repository while `lightsout init` gives
//! new workspaces an unambiguous marker.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const MARKER: &str = "lightsout.toml";
pub const CURRENT_SCHEMA: u32 = 1;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn has_marker(path: &Path) -> bool {
    path.join(MARKER).is_file()
}

fn looks_like_repo(path: &Path) -> bool {
    path.join("jobs").is_dir() && path.join("lib").is_dir()
}

/// A marker wins over the legacy shape, because a directory can contain both a
/// workspace and unrelated `jobs/`/`lib/` directories.
pub fn is_workspace(path: &Path) -> bool {
    has_marker(path) || looks_like_repo(path)
}

/// Every job directory, sorted, with hidden directories and directories that
/// have no `job.sh` left out. One rule for `list`, `check` and `status` keeps a
/// stray directory from appearing in one view and not another.
pub fn job_names(root: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(root.join("jobs")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| {
            entry.path().join("job.sh").is_file()
                && !entry.file_name().to_string_lossy().starts_with('.')
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Resolve a program the way the shell's `command -v` does, including a path
/// that already contains a separator. One implementation keeps `doctor` and
/// the gates from disagreeing about whether a tool exists.
pub fn which(program: &str) -> Option<String> {
    if program.contains('/') {
        return Some(program.to_string());
    }
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        let candidate = dir.join(program);
        if let Ok(meta) = fs::metadata(&candidate) {
            if meta.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if meta.permissions().mode() & 0o111 != 0 {
                        return Some(candidate.to_string_lossy().into_owned());
                    }
                }
                #[cfg(not(unix))]
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    None
}

fn marker_ancestor(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|candidate| has_marker(candidate))
        .map(Path::to_path_buf)
}

pub fn root() -> PathBuf {
    if let Ok(explicit) = env::var("LIGHTSOUT_REPO") {
        if !explicit.is_empty() {
            let path = PathBuf::from(explicit);
            return fs::canonicalize(&path).unwrap_or(path);
        }
    }

    if let Ok(cwd) = env::current_dir() {
        if let Some(found) = marker_ancestor(&cwd) {
            return found;
        }
    }

    if let Ok(exe) = env::current_exe() {
        let exe = fs::canonicalize(&exe).unwrap_or(exe);
        if let Some(parent) = exe.parent() {
            if let Some(found) = marker_ancestor(parent) {
                return found;
            }
            // <repo>/bin/lightsout
            if parent.file_name().and_then(|n| n.to_str()) == Some("bin") {
                if let Some(repo) = parent.parent() {
                    if looks_like_repo(repo) {
                        return repo.to_path_buf();
                    }
                }
            }
            // <repo>/target/{debug,release}/lightsout
            for ancestor in parent.ancestors() {
                if looks_like_repo(ancestor) {
                    return ancestor.to_path_buf();
                }
            }
        }
    }

    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

#[derive(Clone, Debug)]
pub struct Paths {
    pub repo: PathBuf,
    pub name: String,
    pub dir: PathBuf,
    pub state: PathBuf,
    pub out: PathBuf,
    pub log: PathBuf,
    pub unit: String,
}

impl Paths {
    pub fn new(repo: &Path, name: &str) -> Self {
        let dir = repo.join("jobs").join(name);
        let unit = env::var("UNIT")
            .ok()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(crate::clock::today);
        let log = dir
            .join("logs")
            .join(format!("{}.log", crate::clock::today()));
        Paths {
            repo: repo.to_path_buf(),
            name: name.to_string(),
            state: dir.join("state"),
            out: dir.join("out"),
            dir,
            log,
            unit,
        }
    }

    pub fn has_job(&self) -> bool {
        self.dir.join("job.sh").is_file()
    }

    pub fn alerts(&self) -> PathBuf {
        self.repo.join("logs").join("alerts.log")
    }
}
