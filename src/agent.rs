//! Agent profiles: which runner, model and dsh patch a job's agent step uses.
//!
//! The manager never selects a model itself; it resolves a small, human-edited
//! profile into the environment the shell adapter already understands. The
//! profile is the one place a job records its pin, so a lesson call and the
//! repair call that follows it use the same runner and model.

use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

pub const PROFILES: &str = "agents.conf";
pub const JOB_PROFILE: &str = "agent.conf";

const RUNNERS: [&str; 4] = ["dsh", "claude", "codex", "fake"];
const DEFAULT_SOURCE: &str = "agent profile defaults";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// A malformed or contradictory profile. The manager must not guess.
    Config,
    /// A valid profile whose external dependency is absent, such as a patch
    /// path outside this checkout. `doctor` warns; a run still refuses.
    Unavailable,
}

#[derive(Clone, Debug)]
pub struct ProfileError {
    pub source: String,
    pub line: usize,
    pub message: String,
    pub severity: Severity,
}

impl ProfileError {
    fn config(source: &str, line: usize, message: impl Into<String>) -> Self {
        ProfileError {
            source: source.to_string(),
            line,
            message: message.into(),
            severity: Severity::Config,
        }
    }

    fn unavailable(source: &str, line: usize, message: impl Into<String>) -> Self {
        ProfileError {
            source: source.to_string(),
            line,
            message: message.into(),
            severity: Severity::Unavailable,
        }
    }

    pub fn is_unavailable(&self) -> bool {
        self.severity == Severity::Unavailable
    }
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.source.is_empty() {
            write!(f, "{}", self.message)
        } else if self.line > 0 {
            write!(f, "{}:{}: {}", self.source, self.line, self.message)
        } else {
            write!(f, "{}: {}", self.source, self.message)
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Section {
    name: Option<String>,
    line: usize,
    values: BTreeMap<String, String>,
}

impl Section {
    fn new(name: Option<String>, line: usize) -> Self {
        Section {
            name,
            line,
            values: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Profile {
    runner: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    patch: Option<String>,
    effort: Option<String>,
    name: Option<String>,
    source: String,
    line: usize,
}

#[derive(Clone, Debug)]
struct JobFile {
    selected: Option<String>,
    profile: Profile,
}

#[derive(Clone, Debug)]
pub struct Resolved {
    pub runner: String,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub effort: Option<String>,
    pub patch: Option<PathBuf>,
    pub profile: Option<String>,
    pub source: String,
    pub configured: bool,
    line: usize,
}

impl Default for Resolved {
    fn default() -> Self {
        Resolved {
            runner: String::new(),
            model: None,
            provider: None,
            effort: None,
            patch: None,
            profile: None,
            source: DEFAULT_SOURCE.into(),
            configured: false,
            line: 0,
        }
    }
}

impl Resolved {
    pub fn model_label(&self) -> Option<String> {
        match (&self.provider, &self.model) {
            (Some(provider), Some(model)) => Some(format!("{provider}/{model}")),
            (None, Some(model)) => Some(model.clone()),
            _ => None,
        }
    }

    /// One line for `plan`, `triage`, logs and the heal history.
    pub fn describe(&self) -> String {
        let mut parts = vec![self.runner.clone()];
        if let Some(model) = self.model_label() {
            parts.push(model);
        }
        if let Some(effort) = &self.effort {
            parts.push(format!("effort={effort}"));
        }
        if let Some(patch) = &self.patch {
            parts.push(format!("patch={}", patch.display()));
        }
        if let Some(profile) = &self.profile {
            parts.push(format!("profile={profile}"));
        }
        parts.join(" ")
    }

    pub fn to_json(&self) -> Value {
        json!({
            "runner": &self.runner,
            "provider": &self.provider,
            "model": &self.model,
            "model_label": self.model_label(),
            "effort": &self.effort,
            "patch": self.patch.as_ref().map(|path| path.display().to_string()),
            "profile": &self.profile,
            "source": &self.source,
        })
    }

    fn error(&self, message: impl Into<String>) -> ProfileError {
        ProfileError::config(&self.source, self.line, message)
    }

    fn unavailable(&self, message: impl Into<String>) -> ProfileError {
        ProfileError::unavailable(&self.source, self.line, message)
    }
}

fn parse_assignments(
    source: &str,
    body: &str,
    sections_allowed: bool,
) -> Result<Vec<Section>, ProfileError> {
    let mut sections = vec![Section::new(None, 1)];
    for (index, raw) in body.lines().enumerate() {
        let line = index + 1;
        let text = strip_comment(raw).trim();
        if text.is_empty() {
            continue;
        }
        if text.starts_with('[') {
            if !sections_allowed {
                return Err(ProfileError::config(
                    source,
                    line,
                    "a job profile is one unnamed block; sections belong in agents.conf",
                ));
            }
            let Some(end) = text.find(']') else {
                return Err(ProfileError::config(
                    source,
                    line,
                    "section header has no closing ]",
                ));
            };
            let name = text[1..end].trim();
            let rest = text[end + 1..].trim();
            if !rest.is_empty() {
                return Err(ProfileError::config(
                    source,
                    line,
                    "unexpected text after the section header",
                ));
            }
            if !valid_name(name) {
                return Err(ProfileError::config(
                    source,
                    line,
                    format!("invalid profile name '{name}'"),
                ));
            }
            if sections
                .iter()
                .any(|section| section.name.as_deref() == Some(name))
            {
                return Err(ProfileError::config(
                    source,
                    line,
                    format!("duplicate profile [{name}]"),
                ));
            }
            sections.push(Section::new(Some(name.to_string()), line));
            continue;
        }
        let Some((key, value)) = text.split_once('=') else {
            return Err(ProfileError::config(source, line, "expected key = value"));
        };
        let key = key.trim();
        let value = unquote(value.trim());
        if !matches!(
            key,
            "profile" | "runner" | "model" | "provider" | "patch" | "effort"
        ) {
            return Err(ProfileError::config(
                source,
                line,
                format!(
                    "unknown key '{key}' (use profile, runner, model, provider, patch or effort)"
                ),
            ));
        }
        if key == "profile" && sections_allowed {
            return Err(ProfileError::config(
                source,
                line,
                "a workspace profile cannot select another profile",
            ));
        }
        if value.is_empty() {
            return Err(ProfileError::config(
                source,
                line,
                format!("'{key}' needs a value"),
            ));
        }
        let section = sections.last_mut().expect("a section always exists");
        if section.values.contains_key(key) {
            return Err(ProfileError::config(
                source,
                line,
                format!("duplicate key '{key}'"),
            ));
        }
        section.values.insert(key.to_string(), value);
    }
    Ok(sections)
}

/// Strip an unquoted `#` comment from one line. A path that contains `#`
/// has to be quoted; the profile is hand-written, so the usual shell rule is
/// easier to explain than a second escape syntax.
fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    for (index, character) in line.char_indices() {
        match quote {
            Some(open) if character == open => quote = None,
            Some(_) => {}
            None if character == '\'' || character == '"' => quote = Some(character),
            None if character == '#' => return &line[..index],
            None => {}
        }
    }
    line
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn unquote(value: &str) -> String {
    let value = strip_comment(value).trim();
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0] as char;
        let last = bytes[bytes.len() - 1] as char;
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return value[1..value.len() - 1].trim().to_string();
        }
    }
    value.to_string()
}

fn profile_from_section(
    source: &str,
    section: &Section,
    selected: Option<String>,
) -> Result<Profile, ProfileError> {
    let get = |key: &str| section.values.get(key).cloned();
    Ok(Profile {
        runner: get("runner"),
        model: get("model"),
        provider: get("provider"),
        patch: get("patch"),
        effort: get("effort"),
        name: selected,
        source: source.to_string(),
        line: section.line.max(1),
    })
}

fn load_workspace(root: &Path) -> Result<BTreeMap<String, Profile>, ProfileError> {
    let path = root.join(PROFILES);
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    let body = fs::read_to_string(&path).map_err(|error| {
        ProfileError::config(
            &path.display().to_string(),
            0,
            format!("cannot read: {error}"),
        )
    })?;
    let source = path.display().to_string();
    let sections = parse_assignments(&source, &body, true)?;
    let mut profiles = BTreeMap::new();
    for section in sections {
        let name = section
            .name
            .clone()
            .unwrap_or_else(|| "default".to_string());
        if section.name.is_none() && section.values.is_empty() {
            continue;
        }
        if profiles.contains_key(&name) {
            return Err(ProfileError::config(
                &source,
                section.line,
                format!("duplicate profile [{name}]"),
            ));
        }
        profiles.insert(
            name.clone(),
            profile_from_section(&source, &section, Some(name))?,
        );
    }
    Ok(profiles)
}

fn load_job(job_dir: &Path) -> Result<Option<JobFile>, ProfileError> {
    let path = job_dir.join(JOB_PROFILE);
    if !path.is_file() {
        return Ok(None);
    }
    let body = fs::read_to_string(&path).map_err(|error| {
        ProfileError::config(
            &path.display().to_string(),
            0,
            format!("cannot read: {error}"),
        )
    })?;
    let source = path.display().to_string();
    let mut sections = parse_assignments(&source, &body, false)?;
    let section = sections.remove(0);
    let selected = section.values.get("profile").cloned();
    Ok(Some(JobFile {
        selected: selected.clone(),
        profile: profile_from_section(&source, &section, selected)?,
    }))
}

fn load(root: &Path, job_dir: &Path) -> Result<Resolved, ProfileError> {
    let Some(job_file) = load_job(job_dir)? else {
        return Ok(Resolved::default());
    };
    let mut profile = if let Some(selected) = &job_file.selected {
        let profiles = load_workspace(root)?;
        let Some(profile) = profiles.get(selected) else {
            return Err(ProfileError::config(
                &job_file.profile.source,
                job_file.profile.line,
                format!(
                    "profile '{selected}' is not in {} (defined: {})",
                    PROFILES,
                    if profiles.is_empty() {
                        "none".to_string()
                    } else {
                        profiles.keys().cloned().collect::<Vec<_>>().join(", ")
                    }
                ),
            ));
        };
        profile.clone()
    } else {
        Profile::default()
    };
    merge(&mut profile, &job_file.profile);
    profile.source = job_file.profile.source;
    profile.line = job_file.profile.line;
    profile.name = job_file.selected.clone();

    // A job file that is present but empty is still an explicit choice; it
    // resolves to the adapter's own default rather than silently reading the
    // workspace library.
    let resolved = resolved_from(&profile, true);
    Ok(resolved)
}

fn resolved_from(profile: &Profile, configured: bool) -> Resolved {
    Resolved {
        runner: profile.runner.clone().unwrap_or_default(),
        model: profile.model.clone(),
        provider: profile.provider.clone(),
        effort: profile.effort.clone(),
        patch: profile.patch.clone().map(PathBuf::from),
        profile: profile.name.clone(),
        source: profile.source.clone(),
        configured,
        line: profile.line,
    }
}

fn merge(base: &mut Profile, over: &Profile) {
    if let Some(runner) = &over.runner {
        if Some(runner) != base.runner.as_ref() {
            // A different adapter cannot inherit the previous adapter's patch
            // or model: those fields mean different things to each CLI.
            base.model = None;
            base.provider = None;
            base.patch = None;
            base.effort = None;
        }
    }
    if over.runner.is_some() {
        base.runner = over.runner.clone();
    }
    if over.model.is_some() {
        base.model = over.model.clone();
    }
    if over.provider.is_some() {
        base.provider = over.provider.clone();
    }
    if over.patch.is_some() {
        base.patch = over.patch.clone();
    }
    if over.effort.is_some() {
        base.effort = over.effort.clone();
    }
}

fn environment(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

impl Resolved {
    fn apply_environment(&mut self, for_heal: bool) {
        let runner = if for_heal {
            environment("U_HEAL_RUNNER").or_else(|| environment("U_RUNNER"))
        } else {
            environment("U_RUNNER")
        };
        if let Some(runner) = runner {
            if runner != self.runner {
                self.runner = runner;
                self.model = None;
                self.provider = None;
                self.patch = None;
                self.effort = None;
                self.source = "environment".into();
                self.line = 0;
                self.configured = true;
            }
        }
        if let Some(patch) = environment("U_AGENT_PATCH") {
            self.patch = Some(PathBuf::from(patch));
            self.model = None;
            self.provider = None;
            self.effort = None;
            self.source = "environment".into();
            self.line = 0;
            self.configured = true;
        }
        if let Some(model) = environment("U_AGENT_MODEL") {
            self.model = Some(model);
            self.provider = None;
            self.source = "environment".into();
            self.line = 0;
            self.configured = true;
        }
        if let Some(effort) = environment("U_AGENT_EFFORT") {
            self.effort = Some(effort);
            self.source = "environment".into();
            self.line = 0;
            self.configured = true;
        }
    }

    fn finalize(&mut self, root: &Path) -> Result<(), ProfileError> {
        if self.runner.is_empty() {
            self.runner = "dsh".into();
        }
        if !RUNNERS.contains(&self.runner.as_str()) {
            // An environment override is an operator escape hatch: the shell
            // adapter owns the unknown-runner refusal, which is exit 1 for a
            // job and 64 for a heal. A profile file must name a real adapter.
            if self.source == "environment" {
                return Ok(());
            }
            return Err(self.error(format!(
                "unknown runner '{}' (use dsh, claude, codex or fake)",
                self.runner
            )));
        }
        match self.runner.as_str() {
            "dsh" => self.finalize_dsh(root),
            "claude" | "codex" => {
                if self.patch.is_some() {
                    return Err(self.error(format!(
                        "patch is a dsh option, not a {} option",
                        self.runner
                    )));
                }
                if self.provider.is_some() {
                    return Err(self.error("provider is only supported by the dsh runner"));
                }
                if self.effort.is_some() {
                    return Err(self.error(format!(
                        "effort is pinned in the dsh patch; {} needs its own configuration",
                        self.runner
                    )));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn finalize_dsh(&mut self, root: &Path) -> Result<(), ProfileError> {
        if self.provider.is_none() {
            if let Some(model) = self.model.clone() {
                if let Some((provider, rest)) = model.split_once('/') {
                    if !provider.is_empty() && !rest.is_empty() {
                        self.provider = Some(provider.to_string());
                        self.model = Some(rest.to_string());
                    }
                }
            }
        }

        let Some(raw_patch) = self.patch.clone() else {
            if self.model.is_some() || self.provider.is_some() || self.effort.is_some() {
                return Err(self.error(
                    "dsh has no --model flag; set patch to the overlay that pins provider, model and effort",
                ));
            }
            return Ok(());
        };

        let patch = expand_path(root, &raw_patch);
        if !patch.is_file() {
            return Err(self.unavailable(format!("dsh patch does not exist: {}", patch.display())));
        }
        let defaults = read_patch(&patch).map_err(|message| self.error(message))?;
        if let Some(model) = &self.model {
            match &defaults.model {
                Some(patch_model) if patch_model == model => {}
                Some(patch_model) => {
                    return Err(self.error(format!(
                        "model '{model}' does not match the dsh patch's '{patch_model}' ({})",
                        patch.display()
                    )));
                }
                None => {
                    return Err(
                        self.error(format!("the dsh patch has no model: {}", patch.display()))
                    );
                }
            }
        }
        if let Some(provider) = &self.provider {
            match &defaults.provider {
                Some(patch_provider) if patch_provider == provider => {}
                Some(patch_provider) => {
                    return Err(self.error(format!(
                        "provider '{provider}' does not match the dsh patch's '{patch_provider}' ({})",
                        patch.display()
                    )));
                }
                None => {
                    return Err(self.error(format!(
                        "the dsh patch has no provider: {}",
                        patch.display()
                    )));
                }
            }
        }
        if let Some(effort) = &self.effort {
            match &defaults.effort {
                Some(patch_effort) if patch_effort == effort => {}
                Some(patch_effort) => {
                    return Err(self.error(format!(
                        "effort '{effort}' does not match the dsh patch's '{patch_effort}' ({})",
                        patch.display()
                    )));
                }
                None => {
                    return Err(self.error(format!(
                        "the dsh patch has no reasoningEffort: {}",
                        patch.display()
                    )));
                }
            }
        }
        self.model = self.model.clone().or(defaults.model);
        self.provider = self.provider.clone().or(defaults.provider);
        self.effort = self.effort.clone().or(defaults.effort);
        self.patch = Some(patch);
        Ok(())
    }
}

#[derive(Default)]
struct PatchDefault {
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

fn read_patch(path: &Path) -> Result<PatchDefault, String> {
    let body = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut found = false;
    let mut in_block = false;
    let mut out = PatchDefault::default();
    for line in body.lines() {
        let text = line.trim();
        if let Some(id) = text.strip_prefix("- id:") {
            if in_block {
                break;
            }
            let id = unquote(id.trim());
            if id == "agent-default-model" {
                found = true;
                in_block = true;
            }
            continue;
        }
        if !in_block {
            continue;
        }
        if text.starts_with("- ") {
            break;
        }
        if let Some(value) = text.strip_prefix("provider:") {
            out.provider = Some(unquote(value));
        } else if let Some(value) = text.strip_prefix("model:") {
            out.model = Some(unquote(value));
        } else if let Some(value) = text
            .strip_prefix("reasoningEffort:")
            .or_else(|| text.strip_prefix("reasoning_effort:"))
        {
            out.effort = Some(unquote(value));
        }
    }
    if !found {
        return Err(format!(
            "{} has no '- id: agent-default-model' block",
            path.display()
        ));
    }
    if out.provider.is_none() || out.model.is_none() {
        return Err(format!(
            "{} does not declare both provider and model in agent-default-model",
            path.display()
        ));
    }
    Ok(out)
}

fn expand_path(root: &Path, raw: &Path) -> PathBuf {
    let text = raw.to_string_lossy();
    let path = if text == "~" {
        home()
    } else if let Some(rest) = text.strip_prefix("~/") {
        home().join(rest)
    } else if let Some(rest) = text.strip_prefix("$HOME/") {
        home().join(rest)
    } else if let Some(rest) = text.strip_prefix("$VIDEO_REPO/") {
        match environment("VIDEO_REPO") {
            Some(repo) => PathBuf::from(repo).join(rest),
            None => raw.to_path_buf(),
        }
    } else {
        raw.to_path_buf()
    };
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Resolve the effective profile for a job, including environment overrides.
pub fn resolve(root: &Path, job: &str, for_heal: bool) -> Result<Resolved, ProfileError> {
    // A registered job keeps its own directory and workspace; the resolver
    // reads `agent.conf` from the job and `agents.conf` from that workspace.
    let paths = crate::repo::Paths::new(root, job);
    let mut resolved = load(&paths.repo, &paths.dir)?;
    resolved.apply_environment(for_heal);
    resolved.finalize(&paths.repo)?;
    Ok(resolved)
}

/// Resolve only the files on disk; `doctor` uses this so a test's `U_RUNNER`
/// cannot hide the profile that will run in a scheduler.
pub fn resolve_config(root: &Path, job: &str) -> Result<Resolved, ProfileError> {
    let paths = crate::repo::Paths::new(root, job);
    let mut resolved = load(&paths.repo, &paths.dir)?;
    resolved.finalize(&paths.repo)?;
    Ok(resolved)
}

pub fn workspace_profiles(root: &Path) -> Result<Vec<String>, ProfileError> {
    let profiles = load_workspace(root)?;
    for (name, profile) in &profiles {
        let mut resolved = resolved_from(profile, true);
        if let Err(mut error) = resolved.finalize(root) {
            error.message = format!("profile [{name}]: {}", error.message);
            return Err(error);
        }
    }
    Ok(profiles.keys().cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_selects_a_named_profile_and_overrides_one_field() {
        let root = std::env::temp_dir().join(format!(
            "lightsout-agent-unit-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("jobs/demo")).unwrap();
        fs::write(
            root.join(PROFILES),
            "[lesson]\nrunner = fake\nmodel = first-model\n",
        )
        .unwrap();
        fs::write(
            root.join("jobs/demo/agent.conf"),
            "profile = lesson\nmodel = second-model\n",
        )
        .unwrap();
        let resolved = resolve_config(&root, "demo").expect("profile");
        assert_eq!(resolved.runner, "fake");
        assert_eq!(resolved.model.as_deref(), Some("second-model"));
        assert_eq!(resolved.profile.as_deref(), Some("lesson"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_dsh_patch_fills_provider_model_and_effort() {
        let root = std::env::temp_dir().join(format!(
            "lightsout-agent-patch-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("jobs/demo")).unwrap();
        fs::write(
            root.join("pinned.yml"),
            "- id: agent-default-model\n  config:\n    provider: opencode\n    model: pinned\n    reasoningEffort: high\n",
        )
        .unwrap();
        fs::write(
            root.join("jobs/demo/agent.conf"),
            "runner = dsh\npatch = pinned.yml\n",
        )
        .unwrap();
        let resolved = resolve_config(&root, "demo").expect("profile");
        assert_eq!(resolved.model_label().as_deref(), Some("opencode/pinned"));
        assert_eq!(resolved.effort.as_deref(), Some("high"));
        assert!(resolved
            .patch
            .as_ref()
            .map(|p| p.ends_with("pinned.yml"))
            .unwrap_or(false));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_model_that_disagrees_with_the_patch_is_refused() {
        let root = std::env::temp_dir().join(format!(
            "lightsout-agent-mismatch-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("jobs/demo")).unwrap();
        fs::write(
            root.join("pinned.yml"),
            "- id: agent-default-model\n  config:\n    provider: opencode\n    model: pinned\n",
        )
        .unwrap();
        fs::write(
            root.join("jobs/demo/agent.conf"),
            "runner = dsh\npatch = pinned.yml\nmodel = other\n",
        )
        .unwrap();
        let error = resolve_config(&root, "demo").expect_err("mismatch");
        assert!(error.to_string().contains("does not match"), "{error}");
        let _ = fs::remove_dir_all(&root);
    }
}
