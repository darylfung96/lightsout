//! `serve` — the dashboard with buttons.
//!
//! The HTML page is a static file on its own; a click has to reach a process, so
//! this verb binds one HTTP listener on the loopback interface and serves the
//! page plus three routes: the action list, a job's log tail, and a POST that
//! starts one of the allowlisted actions for one job. Every action is the same
//! command a person would type — `run --auto-repair`, `run`, `heal`, `unblock` —
//! spawned as a child, so the page never gains a code path the CLI lacks.
//!
//! The listener is loopback only and refills the page from state files on every
//! request, so what a click shows is what `lightsout status` would print.
//!
//! Safety: the actions are the writes the CLI already permits (a job's own
//! `repair.allow` still bounds a repair), a POST needs the `X-Lightsout` header
//! so a cross-site form cannot reach it, and a `Host` that is not loopback is
//! refused.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::json;

use crate::clock;
use crate::repo;
use crate::report::{self, LiveAction};
use crate::state;
use crate::status;

const DEFAULT_PORT: u16 = 7080;
const MAX_HEAD: usize = 16 * 1024;
const MAX_OUTPUT: usize = 64 * 1024;
const KEEP_ACTIONS: usize = 24;
const RUNNING: i32 = -1;

/// The verbs a button may start. They are spelled the way the CLI spells them
/// so a log line and a click are the same word.
const ACTIONS: [&str; 3] = ["fix", "run", "unblock"];

struct Server {
    repo: PathBuf,
    actions: Mutex<Vec<Arc<Action>>>,
}

struct Action {
    job: String,
    verb: String,
    started: i64,
    /// The child's stdout and stderr, appended by reader threads and capped.
    output: Mutex<String>,
    /// `RUNNING` until the child exits, then its status code.
    done: AtomicI32,
}

impl Action {
    fn running(&self) -> bool {
        self.done.load(Ordering::Relaxed) == RUNNING
    }

    fn push(&self, line: &str) {
        let mut body = self.output.lock().unwrap_or_else(|e| e.into_inner());
        if body.len() >= MAX_OUTPUT {
            return;
        }
        body.push_str(line);
        if body.len() > MAX_OUTPUT {
            let cut = body.len() - MAX_OUTPUT;
            body.drain(..cut);
        }
    }

    fn tail(&self) -> String {
        let body = self.output.lock().unwrap_or_else(|e| e.into_inner());
        body.clone()
    }
}

/// `lightsout serve [--port N] [--open]`
pub fn run(repo: &Path, argv: &[String]) -> i32 {
    let mut port = DEFAULT_PORT;
    let mut open = false;
    let mut index = 0;
    while index < argv.len() {
        match argv[index].as_str() {
            "--open" => open = true,
            "--port" => {
                let Some(value) = argv.get(index + 1) else {
                    eprintln!("serve: --port needs a number");
                    return 64;
                };
                match value.parse::<u16>() {
                    Ok(value) => port = value,
                    Err(_) => {
                        eprintln!("serve: --port must be 0-65535");
                        return 64;
                    }
                }
                index += 1;
            }
            _ if argv[index].starts_with("--port=") => {
                match argv[index]["--port=".len()..].parse::<u16>() {
                    Ok(value) => port = value,
                    Err(_) => {
                        eprintln!("serve: --port must be 0-65535");
                        return 64;
                    }
                }
            }
            other => {
                eprintln!("serve: unknown option: {other}");
                eprintln!("usage: lightsout serve [--port N] [--open]");
                return 64;
            }
        }
        index += 1;
    }

    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("serve: cannot listen on 127.0.0.1:{port}: {error}");
            return 1;
        }
    };
    let bound = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(error) => {
            eprintln!("serve: {error}");
            return 1;
        }
    };
    let origin = format!("http://127.0.0.1:{bound}");
    println!("lightsout serve: {origin}");
    println!("  actions run the same commands as the CLI; Ctrl-C stops the server");
    let _ = std::io::stdout().flush();
    if open {
        crate::status::open_page(Path::new(&origin));
    }
    let server = Arc::new(Server {
        repo: repo.to_path_buf(),
        actions: Mutex::new(Vec::new()),
    });
    accept_loop(listener, server)
}

fn accept_loop(listener: TcpListener, server: Arc<Server>) -> i32 {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let server = Arc::clone(&server);
                thread::spawn(move || handle(stream, &server));
            }
            // A client that resets between the SYN and the accept is one lost
            // request, not a reason to stop serving everyone else.
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(error) => {
                eprintln!("serve: {error}");
                return 1;
            }
        }
    }
    0
}

fn handle(mut stream: TcpStream, server: &Arc<Server>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let Some(head) = read_head(&mut stream) else {
        let _ = respond(
            &mut stream,
            400,
            "Bad Request",
            "text/plain",
            b"bad request\n",
            &[],
        );
        return;
    };
    let request = match Request::parse(&head) {
        Ok(request) => request,
        Err(message) => {
            let _ = respond(
                &mut stream,
                400,
                "Bad Request",
                "text/plain",
                format!("{message}\n").as_bytes(),
                &[],
            );
            return;
        }
    };
    if !request.loopback_host() {
        let _ = respond(
            &mut stream,
            403,
            "Forbidden",
            "text/plain",
            b"host must be loopback\n",
            &[],
        );
        return;
    }
    route(&mut stream, server, &request);
}

fn route(stream: &mut TcpStream, server: &Arc<Server>, request: &Request) {
    let path = request.path();
    match (request.method.as_str(), path.as_str()) {
        ("GET", "/") | ("GET", "/index.html") | ("GET", "/status.html") => {
            let data = status::snapshot(&server.repo);
            let actions = server.live_actions();
            let page = report::render_live(
                &data,
                &server.repo,
                &actions,
                &format!("127.0.0.1:{}", request.port),
            );
            let _ = respond(
                stream,
                200,
                "OK",
                "text/html; charset=utf-8",
                page.as_bytes(),
                &[],
            );
        }
        ("GET", "/api/actions") => {
            let actions: Vec<serde_json::Value> = server
                .live_actions()
                .into_iter()
                .map(|action| {
                    json!({
                        "job": action.job,
                        "verb": action.verb,
                        "running": action.running,
                        "rc": action.rc,
                        "seconds": action.seconds,
                        "output": action.output,
                    })
                })
                .collect();
            let body = json!({ "actions": actions }).to_string();
            let _ = respond(stream, 200, "OK", "application/json", body.as_bytes(), &[]);
        }
        ("GET", _) if path.starts_with("/api/jobs/") && path.ends_with("/log") => {
            let name = &path["/api/jobs/".len()..path.len() - "/log".len()];
            let name = match clean_name(name) {
                Some(name) => name,
                None => return not_found(stream),
            };
            let paths = repo::Paths::new(&server.repo, &name);
            if !paths.has_job() {
                return not_found(stream);
            }
            let lines = request.query_number("lines").unwrap_or(40).clamp(1, 400);
            let body = tail(&paths.log, lines);
            let _ = respond(
                stream,
                200,
                "OK",
                "text/plain; charset=utf-8",
                body.as_bytes(),
                &[],
            );
        }
        ("POST", _) if path.starts_with("/api/jobs/") => {
            if !request.action_header() {
                let _ = respond(
                    stream,
                    403,
                    "Forbidden",
                    "text/plain",
                    b"missing X-Lightsout header\n",
                    &[],
                );
                return;
            }
            let rest = &path["/api/jobs/".len()..];
            let Some((name, verb)) = rest.split_once('/') else {
                return not_found(stream);
            };
            let Some(name) = clean_name(name) else {
                return not_found(stream);
            };
            if !ACTIONS.contains(&verb) {
                return not_found(stream);
            }
            match server.start(&name, verb) {
                Ok(action) => {
                    let body = json!({
                        "job": action.job,
                        "verb": action.verb,
                        "running": true,
                    })
                    .to_string();
                    let _ = respond(
                        stream,
                        202,
                        "Accepted",
                        "application/json",
                        body.as_bytes(),
                        &[],
                    );
                }
                Err(StartError::NoJob) => not_found(stream),
                Err(StartError::Busy) => {
                    let _ = respond(
                        stream,
                        409,
                        "Conflict",
                        "text/plain",
                        b"an action for this job is already running\n",
                        &[],
                    );
                }
                Err(StartError::Spawn(message)) => {
                    let _ = respond(
                        stream,
                        500,
                        "Internal Server Error",
                        "text/plain",
                        format!("cannot start: {message}\n").as_bytes(),
                        &[],
                    );
                }
            }
        }
        ("GET", _) | ("POST", _) => not_found(stream),
        _ => {
            let _ = respond(
                stream,
                405,
                "Method Not Allowed",
                "text/plain",
                b"only GET and POST\n",
                &[("Allow", "GET, POST")],
            );
        }
    }
}

fn not_found(stream: &mut TcpStream) {
    let _ = respond(stream, 404, "Not Found", "text/plain", b"not found\n", &[]);
}

enum StartError {
    NoJob,
    Busy,
    Spawn(String),
}

impl Server {
    fn live_actions(&self) -> Vec<LiveAction> {
        let actions = self.actions.lock().unwrap_or_else(|e| e.into_inner());
        let now = clock::now_epoch();
        actions
            .iter()
            .map(|action| LiveAction {
                job: action.job.clone(),
                verb: action.verb.clone(),
                running: action.running(),
                rc: if action.running() {
                    None
                } else {
                    Some(action.done.load(Ordering::Relaxed))
                },
                seconds: (now - action.started).max(0),
                output: action.tail(),
            })
            .collect()
    }

    /// Start one action for one job. The command is the CLI verb, not a second
    /// implementation: `fix` is `run --auto-repair` for a block written today
    /// and `heal` for an older one, because `run` refuses a stale block and the
    /// operator's click should not come back as "use heal instead".
    fn start(self: &Arc<Self>, job: &str, verb: &str) -> Result<Arc<Action>, StartError> {
        let paths = repo::Paths::new(&self.repo, job);
        if !paths.has_job() {
            return Err(StartError::NoJob);
        }
        let args: Vec<String> = match verb {
            "fix" => {
                let blocked = state::blocked_marker(&paths.state.join("blocked"));
                let stale = blocked
                    .as_ref()
                    .map(|blocked| blocked.iso.get(..10) != Some(clock::today().as_str()))
                    .unwrap_or(false);
                if stale {
                    vec!["heal".into(), job.into()]
                } else {
                    vec!["run".into(), job.into(), "--auto-repair".into()]
                }
            }
            "run" => vec!["run".into(), job.into()],
            "unblock" => vec!["unblock".into(), job.into()],
            _ => return Err(StartError::NoJob),
        };
        let exe = std::env::current_exe()
            .map_err(|error| StartError::Spawn(format!("cannot find lightsout: {error}")))?;
        // The action is reserved under the same lock as the busy check, because
        // two clicks that both pass the check start two children; only one may
        // hold the job. It is removed again when the child cannot start.
        let action = Arc::new(Action {
            job: job.to_string(),
            verb: verb.to_string(),
            started: clock::now_epoch(),
            output: Mutex::new(String::new()),
            done: AtomicI32::new(RUNNING),
        });
        {
            let mut actions = self.actions.lock().unwrap_or_else(|e| e.into_inner());
            if actions
                .iter()
                .any(|action| action.job == job && action.running())
            {
                return Err(StartError::Busy);
            }
            actions.push(Arc::clone(&action));
            if actions.len() > KEEP_ACTIONS {
                let cut = actions.len() - KEEP_ACTIONS;
                actions.drain(..cut);
            }
        }
        let mut child = match Command::new(exe)
            .args(&args)
            .current_dir(&self.repo)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                action.done.store(1, Ordering::Relaxed);
                let mut actions = self.actions.lock().unwrap_or_else(|e| e.into_inner());
                actions.retain(|kept| !Arc::ptr_eq(kept, &action));
                return Err(StartError::Spawn(error.to_string()));
            }
        };

        if let Some(stdout) = child.stdout.take() {
            let action = Arc::clone(&action);
            thread::spawn(move || pump(stdout, &action));
        }
        if let Some(stderr) = child.stderr.take() {
            let action = Arc::clone(&action);
            thread::spawn(move || pump(stderr, &action));
        }
        let finished = Arc::clone(&action);
        thread::spawn(move || {
            let rc = match child.wait() {
                Ok(status) => status.code().unwrap_or(1),
                Err(_) => 1,
            };
            finished.done.store(rc, Ordering::Relaxed);
        });
        Ok(action)
    }
}

/// Copy a child's pipe into the action's tail, one line at a time so a partial
/// line never splits into a broken row.
fn pump(mut source: impl Read, action: &Action) {
    let mut buffer = [0u8; 4096];
    let mut pending = String::new();
    loop {
        match source.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                pending.push_str(&String::from_utf8_lossy(&buffer[..n]));
                while let Some(cut) = pending.find('\n') {
                    let line: String = pending.drain(..=cut).collect();
                    action.push(&line);
                }
                if pending.len() > MAX_OUTPUT {
                    let line = std::mem::take(&mut pending);
                    action.push(&line);
                }
            }
        }
    }
    if !pending.is_empty() {
        action.push(&pending);
        action.push("\n");
    }
}

struct Request {
    method: String,
    target: String,
    host: String,
    port: u16,
    headers: HashMap<String, String>,
}

impl Request {
    fn parse(head: &str) -> Result<Request, String> {
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or("");
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let target = parts.next().unwrap_or("").to_string();
        if method.is_empty() || target.is_empty() {
            return Err("malformed request line".into());
        }
        let mut headers = HashMap::new();
        let mut host = String::new();
        for line in lines {
            if line.is_empty() {
                break;
            }
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if key == "host" {
                host = value.clone();
            }
            headers.insert(key, value);
        }
        let port = host
            .rsplit_once(':')
            .and_then(|(_, port)| port.parse::<u16>().ok())
            .unwrap_or(80);
        Ok(Request {
            method,
            target,
            host,
            port,
            headers,
        })
    }

    fn path(&self) -> String {
        self.target.split('?').next().unwrap_or("/").to_string()
    }

    /// A request from another origin cannot set this header without a preflight,
    /// and this server answers no preflight, so the check is the CSRF boundary.
    fn action_header(&self) -> bool {
        self.headers
            .get("x-lightsout")
            .map(|value| value == "1")
            .unwrap_or(false)
    }

    /// DNS rebinding would let a page on another origin reach this listener, so
    /// a `Host` that does not name loopback is refused.
    fn loopback_host(&self) -> bool {
        if self.host.is_empty() {
            return true;
        }
        let name = self
            .host
            .rsplit_once(':')
            .map(|(name, _)| name)
            .unwrap_or(&self.host);
        matches!(name, "127.0.0.1" | "localhost" | "[::1]" | "::1")
    }

    fn query_number(&self, key: &str) -> Option<usize> {
        let query = self.target.split_once('?')?.1;
        for pair in query.split('&') {
            let Some((name, value)) = pair.split_once('=') else {
                continue;
            };
            if name == key {
                return value.parse().ok();
            }
        }
        None
    }
}

fn clean_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() || name.starts_with('.') || name.contains("..") {
        return None;
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return None;
    }
    Some(name.to_string())
}

fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > MAX_HEAD {
            return None;
        }
    }
    String::from_utf8(buffer).ok()
}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
    extra: &[(&str, &str)],
) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Referrer-Policy: no-referrer\r\n\
         Connection: close\r\n",
        body.len()
    );
    for (key, value) in extra {
        head.push_str(&format!("{key}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn tail(path: &Path, lines: usize) -> String {
    let body = state::read_lossy(path);
    let all: Vec<&str> = body.lines().collect();
    let start = all.len().saturating_sub(lines);
    let mut out = all[start..].join("\n");
    if out.is_empty() {
        out.push_str("(no log yet)\n");
    } else {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn workspace(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("lightsout-serve-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("lib")).unwrap();
        fs::create_dir_all(root.join("jobs")).unwrap();
        fs::write(root.join("lightsout.toml"), "schema = 1\n").unwrap();
        root
    }

    fn job(root: &Path, name: &str) {
        let dir = root.join("jobs").join(name);
        fs::create_dir_all(dir.join("state")).unwrap();
        fs::write(dir.join("job.sh"), "#!/usr/bin/env bash\nexit 0\n").unwrap();
        fs::write(dir.join("spec.md"), "deliverable\n: a test job\n").unwrap();
        fs::write(
            dir.join("state").join("blocked"),
            "2026-01-01T00:00:00-0600\thuman\tpreflight\tthe disk is full\n",
        )
        .unwrap();
    }

    struct Running {
        port: u16,
    }

    fn start(root: PathBuf) -> Running {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Server {
            repo: root.clone(),
            actions: Mutex::new(Vec::new()),
        });
        let _handle = thread::spawn(move || accept_loop(listener, state));
        Running { port }
    }

    fn request(port: u16, raw: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        let mut body = String::new();
        stream.read_to_string(&mut body).unwrap();
        body
    }

    fn get(port: u16, path: &str) -> String {
        request(
            port,
            &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        )
    }

    #[test]
    fn the_page_lists_a_blocked_job_with_actions() {
        let root = workspace("page");
        job(&root, "noop");
        let server = start(root.clone());
        let page = get(server.port, "/");
        assert!(
            page.starts_with("HTTP/1.1 200"),
            "{}",
            &page[..40.min(page.len())]
        );
        assert!(
            page.contains("the disk is full"),
            "the reason is on the page"
        );
        assert!(
            page.contains("data-verb=\"fix\""),
            "the fix button is rendered"
        );
        assert!(page.contains("data-verb=\"unblock\""));
        assert!(
            !page.contains("static page, no server"),
            "served mode, not the file"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_static_page_still_has_no_buttons() {
        let root = workspace("static");
        job(&root, "noop");
        let data = status::snapshot(&root);
        let page = report::render(&data, &root);
        assert!(!page.contains("data-verb=\"fix\""));
        assert!(page.contains("static page, no server"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_post_without_the_action_header_is_refused() {
        let root = workspace("csrf");
        job(&root, "noop");
        let server = start(root.clone());
        let response = request(
            server.port,
            &format!(
                "POST /api/jobs/noop/unblock HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
                server.port
            ),
        );
        assert!(response.starts_with("HTTP/1.1 403"));
        assert!(root.join("jobs/noop/state/blocked").is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_non_loopback_host_is_refused() {
        let root = workspace("host");
        job(&root, "noop");
        let server = start(root.clone());
        let response = request(
            server.port,
            &format!(
                "GET / HTTP/1.1\r\nHost: attacker.example:{}\r\n\r\n",
                server.port
            ),
        );
        assert!(response.starts_with("HTTP/1.1 403"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unknown_paths_and_unknown_verbs_are_not_actions() {
        let root = workspace("unknown");
        job(&root, "noop");
        let server = start(root.clone());
        assert!(get(server.port, "/api/jobs/noop/rm").starts_with("HTTP/1.1 404"));
        assert!(get(server.port, "/api/jobs/../../etc/passwd/log").starts_with("HTTP/1.1 404"));
        assert!(get(server.port, "/nope").starts_with("HTTP/1.1 404"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_log_route_returns_the_job_log() {
        let root = workspace("log");
        job(&root, "noop");
        let logs = root.join("jobs/noop/logs");
        fs::create_dir_all(&logs).unwrap();
        fs::write(
            logs.join(format!("{}.log", clock::today())),
            "one\ntwo\nthree\n",
        )
        .unwrap();
        let server = start(root.clone());
        let response = get(server.port, "/api/jobs/noop/log?lines=2");
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.ends_with("two\nthree\n"), "{response}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_job_name_cannot_escape_the_jobs_directory() {
        assert!(clean_name("noop").is_some());
        assert!(clean_name("..").is_none());
        assert!(clean_name("../etc").is_none());
        assert!(clean_name("a/b").is_none());
        assert!(clean_name(".hidden").is_none());
    }
}
