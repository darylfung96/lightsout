//! `lightsout gate <name>` — named pass/fail checks on artifacts.
//!
//! A gate is the only thing allowed to say a step succeeded. Exit 0 passes,
//! 1 fails, 64 is misuse. An unimplemented check is `never`, never a missing
//! line. The messages are the ones the Python gate printed.

use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::repo;

const USAGE: &str = "usage: lightsout gate <gate> [options]

gates: contains duration exists loudness memory never permalink
       resolution sections shots

options: --file PATH --files CSV --url URL --text TEXT --require CSV
         --min F --max F --min-words N --max-words N --w N --h N
         --target F --tol F --receipt PATH";

#[derive(Debug, Clone)]
struct Opts {
    path: String,
    files: String,
    url: String,
    text: String,
    require: String,
    min: f64,
    max: f64,
    min_words: i64,
    max_words: i64,
    w: i64,
    h: i64,
    target: f64,
    tol: f64,
    receipt: String,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            path: String::new(),
            files: String::new(),
            url: String::new(),
            text: String::new(),
            require: String::new(),
            min: 0.0,
            max: 1_000_000_000.0,
            min_words: 0,
            max_words: 1_000_000_000,
            w: 0,
            h: 0,
            target: -14.0,
            tol: 1.0,
            receipt: String::new(),
        }
    }
}

fn value_after<'a>(argv: &'a [String], i: &mut usize, opt: &str) -> Result<&'a str, String> {
    if let Some(value) = argv.get(*i + 1) {
        *i += 1;
        Ok(value)
    } else {
        Err(format!("{opt} needs a value"))
    }
}

fn parse_opts(argv: &[String]) -> Result<Opts, String> {
    let mut opts = Opts::default();
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_string())),
            _ => (arg.as_str(), None),
        };
        let take = |i: &mut usize, opt: &str| -> Result<String, String> {
            match inline.clone() {
                Some(value) => Ok(value),
                None => Ok(value_after(argv, i, opt)?.to_string()),
            }
        };
        match name {
            "--file" => opts.path = take(&mut i, name)?,
            "--files" => opts.files = take(&mut i, name)?,
            "--url" => opts.url = take(&mut i, name)?,
            "--text" => opts.text = take(&mut i, name)?,
            "--require" => opts.require = take(&mut i, name)?,
            "--min" => {
                opts.min = take(&mut i, name)?
                    .parse()
                    .map_err(|_| format!("invalid float value: {arg}"))?
            }
            "--max" => {
                opts.max = take(&mut i, name)?
                    .parse()
                    .map_err(|_| format!("invalid float value: {arg}"))?
            }
            "--min-words" => {
                opts.min_words = take(&mut i, name)?
                    .parse()
                    .map_err(|_| format!("invalid int value: {arg}"))?
            }
            "--max-words" => {
                opts.max_words = take(&mut i, name)?
                    .parse()
                    .map_err(|_| format!("invalid int value: {arg}"))?
            }
            "--w" => {
                opts.w = take(&mut i, name)?
                    .parse()
                    .map_err(|_| format!("invalid int value: {arg}"))?
            }
            "--h" => {
                opts.h = take(&mut i, name)?
                    .parse()
                    .map_err(|_| format!("invalid int value: {arg}"))?
            }
            "--target" => {
                opts.target = take(&mut i, name)?
                    .parse()
                    .map_err(|_| format!("invalid float value: {arg}"))?
            }
            "--tol" => {
                opts.tol = take(&mut i, name)?
                    .parse()
                    .map_err(|_| format!("invalid float value: {arg}"))?
            }
            "--receipt" => opts.receipt = take(&mut i, name)?,
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unrecognized arguments: {other}")),
        }
        i += 1;
    }
    Ok(opts)
}

fn read_text(path: &str) -> io::Result<String> {
    fs::read(path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

fn py_os_error(path: &str, error: &io::Error) -> String {
    let suffix = match error.kind() {
        io::ErrorKind::NotFound => "No such file or directory",
        io::ErrorKind::PermissionDenied => "Permission denied",
        _ => "I/O error",
    };
    let code = match error.kind() {
        io::ErrorKind::NotFound => 2,
        io::ErrorKind::PermissionDenied => 13,
        _ => 5,
    };
    format!("[Errno {code}] {suffix}: '{path}'")
}

pub fn py_float(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

fn py_repr(value: &str) -> String {
    let mut out = String::from("'");
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('\'');
    out
}

fn value_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => {
            if *b {
                "True".into()
            } else {
                "False".into()
            }
        }
        Value::Null => "None".into(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn capture(program: &str, args: &[&str]) -> io::Result<std::process::Output> {
    Command::new(program).args(args).output()
}

fn sections(opts: &Opts) -> Result<(bool, String), String> {
    let body = read_text(&opts.path).map_err(|e| py_os_error(&opts.path, &e))?;
    let words = body.split_whitespace().count();
    if (words as i64) < opts.min_words || (words as i64) > opts.max_words {
        return Ok((
            false,
            format!("{words} words, want {}-{}", opts.min_words, opts.max_words),
        ));
    }
    for name in opts
        .require
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        if !has_section(&body, name) {
            return Ok((
                false,
                format!("missing section {} ({words} words)", py_repr(name)),
            ));
        }
    }
    Ok((
        true,
        format!(
            "{words} words, {} sections",
            opts.require.split(',').count()
        ),
    ))
}

fn has_section(body: &str, name: &str) -> bool {
    let needle = name.to_lowercase();
    for line in body.lines() {
        let trimmed = line.trim_start();
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if hashes == 0 {
            continue;
        }
        let rest = &trimmed[hashes..];
        if !rest.chars().next().is_some_and(|c| c.is_whitespace()) {
            continue;
        }
        if rest.to_lowercase().contains(&needle) {
            return true;
        }
    }
    false
}

fn probe(path: &str) -> (Option<Value>, String) {
    if repo::which("ffprobe").is_none() {
        return (None, "ffprobe not installed".into());
    }
    let output = capture(
        "ffprobe",
        &[
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            path,
        ],
    );
    let output = match output {
        Ok(output) => output,
        Err(e) => return (None, e.to_string()),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .trim()
            .lines()
            .last()
            .unwrap_or("ffprobe failed")
            .to_string();
        return (None, detail);
    }
    match serde_json::from_slice::<Value>(&output.stdout) {
        Ok(value) => (Some(value), String::new()),
        Err(e) => (None, e.to_string()),
    }
}

fn value_f64(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        Some(Value::Bool(true)) => 1.0,
        _ => 0.0,
    }
}

fn value_i64(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(n)) => n
            .as_i64()
            .unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64),
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        Some(Value::Bool(true)) => 1,
        _ => 0,
    }
}

fn duration(opts: &Opts) -> (bool, String) {
    let (probed, err) = probe(&opts.path);
    let Some(probed) = probed else {
        return (false, err);
    };
    let dur = value_f64(probed.get("format").and_then(|f| f.get("duration")));
    (
        opts.min <= dur && dur <= opts.max,
        format!(
            "{dur:.3}s, want {}-{}s",
            py_float(opts.min),
            py_float(opts.max)
        ),
    )
}

fn resolution(opts: &Opts) -> (bool, String) {
    let (probed, err) = probe(&opts.path);
    let Some(probed) = probed else {
        return (false, err);
    };
    if let Some(streams) = probed.get("streams").and_then(Value::as_array) {
        for stream in streams {
            if stream.get("codec_type").and_then(Value::as_str) == Some("video") {
                let cw = value_i64(stream.get("width"));
                let ch = value_i64(stream.get("height"));
                if opts.w == 0 && opts.h == 0 {
                    return (true, format!("{cw}x{ch}"));
                }
                return (
                    cw == opts.w && ch == opts.h,
                    format!("{cw}x{ch}, want {}x{}", opts.w, opts.h),
                );
            }
        }
    }
    (false, "no video stream".into())
}

fn loudness(opts: &Opts) -> (bool, String) {
    if repo::which("ffmpeg").is_none() {
        return (false, "ffmpeg not installed".into());
    }
    let output = capture(
        "ffmpeg",
        &[
            "-hide_banner",
            "-nostats",
            "-i",
            &opts.path,
            "-af",
            "ebur128",
            "-f",
            "null",
            "-",
        ],
    );
    let output = match output {
        Ok(output) => output,
        Err(e) => return (false, e.to_string()),
    };
    let stderr = String::from_utf8_lossy(&output.stderr);
    let Some(lufs) = last_lufs(&stderr) else {
        return (false, "no integrated loudness in ffmpeg output".into());
    };
    (
        (lufs - opts.target).abs() <= opts.tol,
        format!(
            "{lufs:.1} LUFS, want {}+-{}",
            py_float(opts.target),
            py_float(opts.tol)
        ),
    )
}

/// Find the last `I: -14.0 LUFS` reading, requiring a decimal point like the
/// Python regex did.
fn last_lufs(stderr: &str) -> Option<f64> {
    let mut found = None;
    for (index, _) in stderr.match_indices("I:") {
        let rest = &stderr[index + 2..];
        let rest = rest.trim_start();
        let mut chars = rest.char_indices();
        let mut end = 0;
        let mut seen_dot = false;
        for (i, c) in chars.by_ref() {
            if c == '-' && i == 0 {
                end = i + c.len_utf8();
                continue;
            }
            if c.is_ascii_digit() {
                end = i + c.len_utf8();
            } else if c == '.' && !seen_dot {
                seen_dot = true;
                end = i + c.len_utf8();
            } else {
                break;
            }
        }
        if !seen_dot || end == 0 {
            continue;
        }
        let number = &rest[..end];
        let after = rest[end..].trim_start();
        if after.starts_with("LUFS") {
            if let Ok(value) = number.parse::<f64>() {
                found = Some(value);
            }
        }
    }
    found
}

fn shots(opts: &Opts) -> (bool, String) {
    let bytes = match fs::read(&opts.path) {
        Ok(bytes) => bytes,
        Err(e) => return (false, py_os_error(&opts.path, &e)),
    };
    let parsed: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(e) => return (false, format!("{} is not valid JSON: {e}", opts.path)),
    };
    let Value::Array(entries) = parsed else {
        return (false, format!("{} is not a list of shots", opts.path));
    };
    let want = opts.min as i64;
    let plural = if entries.len() == 1 { "" } else { "s" };
    if (entries.len() as i64) < want {
        return (
            false,
            format!("{} shot{plural}, want at least {want}", entries.len()),
        );
    }
    if entries.is_empty() {
        return (false, format!("{} lists no shots", opts.path));
    }

    for (i, entry) in entries.iter().enumerate() {
        let Value::Object(entry) = entry else {
            return (false, format!("shot {} is not an object", i + 1));
        };
        if entry.get("approved") != Some(&Value::Bool(true)) {
            return (false, format!("{} is not approved", who(entry, i)));
        }
    }
    for (i, entry) in entries.iter().enumerate() {
        let Value::Object(entry) = entry else {
            continue;
        };
        let name = entry
            .get("file")
            .filter(|v| truthy(v))
            .map(value_string)
            .unwrap_or_default();
        let present = fs::metadata(&name)
            .map(|m| m.is_file() && m.len() > 0)
            .unwrap_or(false);
        if !present {
            let label = if name.is_empty() { "no file" } else { &name };
            return (
                false,
                format!("{}: {label} is missing or empty", who(entry, i)),
            );
        }
    }
    (true, format!("{} shot{plural} approved", entries.len()))
}

fn who(entry: &serde_json::Map<String, Value>, i: usize) -> String {
    let shot = entry
        .get("shot")
        .map(value_string)
        .unwrap_or_else(|| (i + 1).to_string());
    let label = format!("shot {shot}");
    match entry.get("query").filter(|v| truthy(v)) {
        Some(query) => format!("{label} {}", py_repr(&value_string(query))),
        None => label,
    }
}

fn contains(opts: &Opts) -> (bool, String) {
    let body = match read_text(&opts.path) {
        Ok(body) => body,
        Err(e) => return (false, py_os_error(&opts.path, &e)),
    };
    (
        body.contains(&opts.text),
        format!("{} contains {}", opts.path, py_repr(&opts.text)),
    )
}

fn exists(opts: &Opts) -> (bool, String) {
    let is_file = fs::metadata(&opts.path)
        .map(|m| m.is_file())
        .unwrap_or(false);
    let nonempty = is_file
        && fs::metadata(&opts.path)
            .map(|m| m.len() > 0)
            .unwrap_or(false);
    (
        nonempty,
        if is_file {
            format!("{} exists", opts.path)
        } else {
            format!("{} missing", opts.path)
        },
    )
}

struct Step {
    label: String,
    program: String,
    args: Vec<String>,
}

fn memory(opts: &Opts) -> (bool, String) {
    let store = std::env::var("U_PI_MEMORY")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .filter(|h| !h.is_empty())
                .map(|home| format!("{home}/agent-memory"))
        })
        .unwrap_or_else(|| "/nonexistent/agent-memory".into());
    let mut steps = Vec::new();
    let mut names = Vec::new();
    for name in opts
        .files
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        if !Path::new(name).is_file() {
            return (false, format!("save {name} failed: file does not exist"));
        }
        names.push(name.to_string());
        steps.push(Step {
            label: format!("save {name}"),
            program: format!("{store}/qmd-mem.sh"),
            args: vec!["save".into(), name.into()],
        });
    }
    steps.push(Step {
        label: "lint".into(),
        program: "python3".into(),
        args: vec![format!("{store}/lint-memory.py"), "--deep".into()],
    });

    // An agent session proves its pass with memory-pass-check.py, which reads
    // the session transcript. A shell job has no transcript, so it runs the
    // same save and lint, adds the duplicate scan, and records the evidence
    // itself: page hashes, mtimes and a timestamp under the job's state.
    if opts.receipt.is_empty() {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        steps.push(Step {
            label: "receipt".into(),
            program: "python3".into(),
            args: vec![
                format!("{store}/memory-pass-check.py"),
                "--latest".into(),
                "--cwd".into(),
                cwd,
            ],
        });
        for step in &steps {
            if let Err(message) = run_step(step) {
                return (false, message);
            }
        }
        return (true, format!("{} steps passed", steps.len()));
    }

    steps.push(Step {
        label: "scan".into(),
        program: "python3".into(),
        args: vec![format!("{store}/scan-duplicates.py"), "--from-index".into()],
    });
    let mut scan_out = String::new();
    for step in &steps {
        match run_step(step) {
            Ok(output) => {
                if step.label == "scan" {
                    scan_out = output;
                }
            }
            Err(message) => return (false, message),
        }
    }
    let repeats = match scan_repeats(&scan_out) {
        Some(n) => n,
        None => return (false, "scan failed: no exact-repeat count in output".into()),
    };
    if repeats != 0 {
        return (
            false,
            format!("scan failed: {repeats} exact repeats across pages"),
        );
    }
    match write_receipt(&opts.receipt, &names, repeats) {
        Ok(path) => (
            true,
            format!("{} steps passed; receipt {path}", steps.len()),
        ),
        Err(e) => (false, format!("record receipt failed: {e}")),
    }
}

fn run_step(step: &Step) -> Result<String, String> {
    let output = Command::new(&step.program)
        .args(&step.args)
        .output()
        .map_err(|e| format!("{} failed: {e}", step.label))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let chosen = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    let detail = chosen
        .trim()
        .lines()
        .last()
        .unwrap_or("no output")
        .to_string();
    Err(format!(
        "{} failed (exit {}): {detail}",
        step.label,
        output.status.code().unwrap_or(1)
    ))
}

fn scan_repeats(stdout: &str) -> Option<usize> {
    stdout.lines().find_map(|line| {
        line.trim()
            .strip_prefix("exact repeats across pages:")
            .and_then(|rest| rest.trim().parse().ok())
    })
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn mtime_secs(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// FNV-1a 64-bit. The receipt is an audit trail for accidental drift, not a
/// security boundary, so a dependency-free stable hash is enough.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn write_receipt(path: &str, names: &[String], repeats: usize) -> io::Result<String> {
    let cwd = std::env::current_dir()?;
    let mut files = Vec::new();
    for name in names {
        let page = cwd.join(name);
        let meta = fs::metadata(&page)?;
        let bytes = fs::read(&page)?;
        files.push(serde_json::json!({
            "name": name,
            "path": page.to_string_lossy(),
            "bytes": bytes.len(),
            "mtime": mtime_secs(&meta),
            "hash": format!("fnv1a64:{:016x}", fnv1a64(&bytes)),
        }));
    }
    let receipt = serde_json::json!({
        "version": 1,
        "gate": "memory",
        "job": std::env::var("JOB").ok(),
        "cwd": cwd.to_string_lossy(),
        "recorded_at": now_secs(),
        "files": files,
        "steps": ["save", "lint --deep", "scan --from-index"],
        "exact_repeats": repeats,
    });
    let mut text = serde_json::to_vec_pretty(&receipt).map_err(io::Error::other)?;
    text.push(b'\n');
    let dest = Path::new(path);
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let tmp = format!("{path}.tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, dest)?;
    Ok(dest.to_string_lossy().into_owned())
}

fn permalink(opts: &Opts) -> (bool, String) {
    if repo::which("curl").is_none() {
        return (false, format!("{} requires curl", opts.url));
    }
    let output = capture(
        "curl",
        &[
            "-sS",
            "-L",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "--max-time",
            "20",
            "-A",
            "lightsout-gate/1",
            &opts.url,
        ],
    );
    match output {
        Ok(output) if output.status.success() => {
            let code: i32 = String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse()
                .unwrap_or(0);
            (
                matches!(code, 200 | 301 | 302),
                format!("{} -> HTTP {code}", opts.url),
            )
        }
        Ok(output) => {
            let detail = String::from_utf8_lossy(&output.stderr)
                .trim()
                .lines()
                .last()
                .unwrap_or("curl failed")
                .to_string();
            (false, format!("{} unreachable: {detail}", opts.url))
        }
        Err(e) => (false, format!("{} unreachable: {e}", opts.url)),
    }
}

fn never() -> (bool, String) {
    (
        false,
        "NOT IMPLEMENTED — this gate must be written before the job runs".into(),
    )
}

pub fn run(argv: &[String]) -> i32 {
    let Some(gate) = argv.first() else {
        eprintln!("{USAGE}");
        return 64;
    };
    if gate == "-h" || gate == "--help" {
        println!("{USAGE}");
        return 0;
    }
    let opts = match parse_opts(&argv[1..]) {
        Ok(opts) => opts,
        Err(message) if message.is_empty() => {
            println!("{USAGE}");
            return 0;
        }
        Err(message) => {
            println!("FAIL {gate}: {message}");
            return 64;
        }
    };
    let (ok, message) = match gate.as_str() {
        "contains" => contains(&opts),
        "duration" => duration(&opts),
        "exists" => exists(&opts),
        "loudness" => loudness(&opts),
        "shots" => shots(&opts),
        "memory" => memory(&opts),
        "never" => never(),
        "permalink" => permalink(&opts),
        "resolution" => resolution(&opts),
        "sections" => match sections(&opts) {
            Ok(result) => result,
            Err(message) => (false, message),
        },
        other => {
            println!("FAIL {other}: unknown gate");
            eprintln!("{USAGE}");
            return 64;
        }
    };
    println!("{} {gate}: {message}", if ok { "PASS" } else { "FAIL" });
    if ok {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lufs_reads_the_last_integrated_line() {
        let text = "I: -18.0 LUFS\n[Parsed_ebur128_0] Summary:\n  I: -14.0 LUFS\n";
        assert_eq!(last_lufs(text), Some(-14.0));
    }

    #[test]
    fn section_heading_ignores_body_mentions() {
        assert!(has_section("# Contract\n\nbody", "contract"));
        assert!(!has_section("text contract", "contract"));
    }

    #[test]
    fn scan_repeats_reads_the_count_only() {
        let out = "indexed files: 3\n\nexact repeats across pages: 2\n";
        assert_eq!(scan_repeats(out), Some(2));
        assert_eq!(scan_repeats("nothing here"), None);
    }

    #[test]
    fn fnv1a64_is_stable_across_runs() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
