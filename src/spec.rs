//! The `spec.md` reader a job uses to describe itself.
//!
//! `status <name>` answers "what is this and what would it do" from the spec
//! beside the job, not from a prompt or a guess. Two shapes are read because
//! the repository's own specs use both: a definition list (`deliverable`,
//! `cadence`, `unit`, `stages`, ...) as in `jobs/everyday-laws/spec.md`, and
//! prose with a `## What a run does` section as in `jobs/brag-sweep/spec.md`.
//! A third shape is not an error: it degrades to the title and first
//! paragraph, because a dashboard must not fail because a spec was written
//! differently.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct Spec {
    pub title: String,
    pub summary: String,
    pub fields: BTreeMap<String, String>,
    pub stages: Vec<String>,
}

impl Spec {
    pub fn field(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }
}

pub fn read(path: &Path) -> Option<Spec> {
    let body = fs::read_to_string(path).ok()?;
    if body.trim().is_empty() {
        return None;
    }
    Some(parse(&body))
}

pub fn parse(body: &str) -> Spec {
    let lines: Vec<&str> = body.lines().map(str::trim_end).collect();
    let mut spec = Spec::default();

    let title_at = lines.iter().position(|line| line.starts_with("# "));
    if let Some(at) = title_at {
        spec.title = lines[at][2..].trim().to_string();
    }

    let mut field_lines: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut i = 0;
    while i + 1 < lines.len() {
        let key = lines[i];
        let value = lines[i + 1];
        if !key.is_empty()
            && !key.starts_with([' ', '\t', '#', '-', '*', '>', '|'])
            && !key.contains(": ")
            && key.split_whitespace().count() == 1
            && value.starts_with(": ")
        {
            let mut parts = vec![value[2..].trim().to_string()];
            let mut j = i + 2;
            while j < lines.len() && starts_indented(lines[j]) {
                if !lines[j].trim().is_empty() {
                    parts.push(lines[j].trim().to_string());
                }
                j += 1;
            }
            spec.fields.insert(
                key.trim().to_ascii_lowercase(),
                parts.join(" ").trim().to_string(),
            );
            field_lines.insert(key.trim().to_ascii_lowercase(), parts);
            i = j;
            continue;
        }
        i += 1;
    }

    if let Some(lines) = field_lines.get("stages") {
        spec.stages = numbered_items(lines);
    }
    if spec.stages.is_empty() {
        spec.stages = numbered_items(&section(&lines, "What a run does"));
    }

    spec.summary = spec
        .fields
        .get("deliverable")
        .cloned()
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| first_paragraph(&lines, title_at));
    spec
}

fn starts_indented(line: &str) -> bool {
    line.starts_with("  ") || line.starts_with('\t')
}

/// The lines under a `#`/`##` heading, whatever its level, up to the next one.
fn section<'a>(lines: &'a [&'a str], heading: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in lines {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('#') {
            let title = rest.trim_start_matches('#').trim();
            inside = title.trim_end_matches(':').eq_ignore_ascii_case(heading);
            continue;
        }
        if inside {
            out.push(trimmed.to_string());
        }
    }
    out
}

/// Items begin "N. " and a continuation line belongs to the item above it, so
/// a wrapped stage stays one line in the block instead of becoming a stage of
/// its own.
fn numbered_items(lines: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in lines {
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if !starts_number(text) => {
                last.push(' ');
                last.push_str(text);
            }
            _ => out.push(text.to_string()),
        }
    }
    out
}

fn starts_number(text: &str) -> bool {
    let rest = text.trim_start_matches(|c: char| c.is_ascii_digit());
    rest.len() < text.len() && rest.starts_with(". ")
}

/// The first paragraph after the title, skipping a definition-list key (a line
/// whose next line is its value).
fn first_paragraph(lines: &[&str], title_at: Option<usize>) -> String {
    let start = title_at.map(|at| at + 1).unwrap_or(0);
    let mut out: Vec<&str> = Vec::new();
    for (offset, line) in lines[start..].iter().enumerate() {
        let text = line.trim();
        if text.is_empty() {
            if !out.is_empty() {
                break;
            }
            continue;
        }
        if text.starts_with('#') {
            if out.is_empty() {
                continue;
            }
            break;
        }
        let next = lines
            .get(start + offset + 1)
            .map(|l| l.trim())
            .unwrap_or("");
        if out.is_empty() && next.starts_with(": ") {
            // A definition-list key, not prose: the value is the summary.
            return next[2..].trim().to_string();
        }
        out.push(text);
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_definition_list_spec_is_fields() {
        let spec = parse(
            r#"# spec: video-prune

deliverable
: Reclaimed disk from render intermediates.

cadence
: Daily at 23:30, before the midnight build.

unit
: The calendar date.

stages
: 1. inventory — read the queue, write a plan
  and its gate
  2. apply — remove the planned directories
"#,
        );
        assert_eq!(spec.title, "spec: video-prune");
        assert_eq!(spec.summary, "Reclaimed disk from render intermediates.");
        assert_eq!(
            spec.field("cadence"),
            Some("Daily at 23:30, before the midnight build.")
        );
        assert_eq!(spec.field("unit"), Some("The calendar date."));
        assert_eq!(spec.stages.len(), 2, "{:?}", spec.stages);
        assert!(spec.stages[0].contains("inventory"));
        assert!(spec.stages[0].contains("and its gate"));
        assert!(spec.stages[1].starts_with("2. apply"));
    }

    #[test]
    fn a_prose_spec_reads_the_intro_and_the_run_section() {
        let spec = parse(
            r#"# brag-sweep — spec

The daily job sweep for the Brag resume repo.
It runs once a day.

## What a run does

1. `jobtool doctor` — credentials.
   A failure is fatal.
2. `jobtool digest -q` — new postings only.

## What "done" means

A digest exists.
"#,
        );
        assert_eq!(spec.title, "brag-sweep — spec");
        assert_eq!(
            spec.summary,
            "The daily job sweep for the Brag resume repo. It runs once a day."
        );
        assert_eq!(spec.stages.len(), 2, "{:?}", spec.stages);
        assert!(spec.stages[0].contains("A failure is fatal"));
        assert!(spec.stages[1].contains("digest -q"));
    }

    #[test]
    fn an_unknown_shape_is_not_an_error() {
        let spec = parse("just some text\nwith no heading\n");
        assert!(spec.title.is_empty());
        assert_eq!(spec.summary, "just some text with no heading");
        assert!(spec.stages.is_empty());
        assert!(spec.fields.is_empty());
    }

    #[test]
    fn an_empty_file_reads_as_none() {
        let dir = std::env::temp_dir().join(format!("lightsout-spec-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("empty.md");
        fs::write(&path, "\n\n").unwrap();
        assert!(read(&path).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
