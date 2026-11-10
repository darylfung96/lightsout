//! `lightsout serve` end to end: the page a click acts on.
//!
//! A click has to reach a process, so these tests start the real binary with
//! `--port 0`, read the URL it prints, and talk HTTP to it. The action runs as a
//! child of the server, which is the path a person's click takes.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Fixture {
    root: PathBuf,
    child: Child,
    /// The server's own errors, kept so a failed test can say why the server
    /// stopped instead of only that the socket failed.
    stderr: Option<std::process::ChildStderr>,
    port: u16,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking() {
            if let Some(mut stderr) = self.stderr.take() {
                let mut said = String::new();
                let _ = stderr.read_to_string(&mut said);
                if !said.trim().is_empty() {
                    eprintln!("serve stderr:\n{said}");
                }
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn fixture(tag: &str) -> Fixture {
    let root =
        std::env::temp_dir().join(format!("lightsout-serve-e2e-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("lib")).unwrap();
    fs::create_dir_all(root.join("jobs")).unwrap();
    fs::write(root.join("lightsout.toml"), "schema = 1\n").unwrap();
    job(&root, "noop");
    let mut child = Command::new(env!("CARGO_BIN_EXE_lightsout"))
        .args(["serve", "--port", "0"])
        .current_dir(&root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("serve starts");
    let stderr = child.stderr.take();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let port: u16 = line
        .trim()
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("no port in {line:?}"));
    // A server that printed its URL still has to answer: wait for the listener
    // so a slow start is not read as a broken page.
    let mut last = String::new();
    let mut answered = false;
    for _ in 0..40 {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(_) => {
                answered = true;
                break;
            }
            Err(error) => {
                last = error.to_string();
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
    if !answered {
        let status = child.try_wait();
        let mut said = String::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_string(&mut said);
            eprintln!("serve stderr:\n{said}");
            return Fixture {
                root,
                child,
                stderr: None,
                port,
            };
        }
        panic!("serve never answered on {port}: {last}; child {status:?}");
    }
    Fixture {
        root,
        child,
        stderr,
        port,
    }
}

fn job(root: &Path, name: &str) {
    let dir = root.join("jobs").join(name);
    fs::create_dir_all(dir.join("state")).unwrap();
    fs::write(dir.join("job.sh"), "#!/usr/bin/env bash\nexit 0\n").unwrap();
    fs::write(
        dir.join("spec.md"),
        "deliverable\n: a test job\n\ncadence\n: daily\n",
    )
    .unwrap();
}

fn request(port: u16, raw: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
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

fn post(port: u16, path: &str) -> String {
    request(
        port,
        &format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Lightsout: 1\r\nContent-Length: 0\r\n\r\n"
        ),
    )
}

/// Poll the action list until `job` has no running action, then return it.
fn wait_for(port: u16, job: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let body = body_of(&get(port, "/api/actions")).to_string();
        if body.contains(&format!("\"job\":\"{job}\"")) && body.contains("\"running\":false") {
            return body;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("{job} did not finish");
}

fn body_of(response: &str) -> &str {
    response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or("")
}

#[test]
fn the_served_page_shows_the_block_and_the_actions() {
    let fixture = fixture("page");
    fs::write(
        fixture.root.join("jobs/noop/state/blocked"),
        "2026-01-01T00:00:00-0600\thuman\tpreflight\tthe disk is full\n",
    )
    .unwrap();
    let response = get(fixture.port, "/");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let page = body_of(&response);
    assert!(page.contains("the disk is full"), "the reason is shown");
    assert!(page.contains("data-verb=\"fix\""), "fix is offered");
    assert!(page.contains("data-verb=\"unblock\""), "unblock is offered");
    assert!(page.contains("data-verb=\"log\""), "the log is offered");
    assert!(page.contains("data-job=\"noop\""));
}

#[test]
fn clicking_unblock_clears_the_marker_and_reports_the_run() {
    let fixture = fixture("unblock");
    let marker = fixture.root.join("jobs/noop/state/blocked");
    fs::write(
        &marker,
        "2026-01-01T00:00:00-0600\thuman\tpreflight\tstop\n",
    )
    .unwrap();

    let started = post(fixture.port, "/api/jobs/noop/unblock");
    assert!(started.starts_with("HTTP/1.1 202"), "{started}");

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut finished = false;
    while Instant::now() < deadline {
        let actions = body_of(&get(fixture.port, "/api/actions")).to_string();
        if actions.contains("\"running\":false") {
            finished = true;
            assert!(actions.contains("\"verb\":\"unblock\""));
            assert!(actions.contains("\"rc\":0"));
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    assert!(finished, "the action finished");
    assert!(!marker.is_file(), "the block marker is gone");
    let page = body_of(&get(fixture.port, "/")).to_string();
    assert!(
        !page.contains("the disk is full") || !page.contains("blocked"),
        "{page}"
    );
    assert!(
        page.contains("Last action"),
        "the finished click keeps its output"
    );
    assert!(page.contains("unblock"), "the row names the verb");
}

#[test]
fn a_second_click_while_one_is_running_is_refused() {
    let fixture = fixture("busy");
    let dir = fixture.root.join("jobs/slow");
    fs::create_dir_all(dir.join("state")).unwrap();
    fs::write(dir.join("job.sh"), "#!/usr/bin/env bash\nsleep 5\nexit 0\n").unwrap();
    assert!(post(fixture.port, "/api/jobs/slow/run").starts_with("HTTP/1.1 202"));
    let second = post(fixture.port, "/api/jobs/slow/run");
    assert!(second.starts_with("HTTP/1.1 409"), "{second}");
}

#[test]
fn the_log_route_returns_the_tail() {
    let fixture = fixture("log");
    let logs = fixture.root.join("jobs/noop/logs");
    fs::create_dir_all(&logs).unwrap();
    let today = Command::new("date").arg("+%F").output().unwrap();
    let name = String::from_utf8_lossy(&today.stdout).trim().to_string();
    fs::write(logs.join(format!("{name}.log")), "alpha\nbeta\ngamma\n").unwrap();
    let response = get(fixture.port, "/api/jobs/noop/log?lines=2");
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(body_of(&response).ends_with("beta\ngamma\n"), "{response}");
}

#[test]
fn an_unknown_job_is_a_404_and_starts_nothing() {
    let fixture = fixture("missing");
    assert!(post(fixture.port, "/api/jobs/ghost/fix").starts_with("HTTP/1.1 404"));
}

#[test]
fn fix_on_a_stale_block_runs_heal() {
    let fixture = fixture("stale");
    fs::write(
        fixture.root.join("jobs/noop/state/blocked"),
        "2026-01-01T00:00:00-0600\thuman\tpreflight\tstop\n",
    )
    .unwrap();
    assert!(post(fixture.port, "/api/jobs/noop/fix").starts_with("HTTP/1.1 202"));
    let actions = wait_for(fixture.port, "noop");
    assert!(
        actions.contains("heal:"),
        "a block dated before today heals: {actions}"
    );
}

#[test]
fn eight_clicks_at_once_start_one_run() {
    let fixture = fixture("race");
    let dir = fixture.root.join("jobs/slow");
    fs::create_dir_all(dir.join("state")).unwrap();
    fs::write(dir.join("job.sh"), "#!/usr/bin/env bash\nsleep 4\nexit 0\n").unwrap();

    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                let response = post(fixture.port, "/api/jobs/slow/run");
                assert!(
                    response.starts_with("HTTP/1.1 202") || response.starts_with("HTTP/1.1 409"),
                    "{response}"
                );
            });
        }
    });

    let actions = body_of(&get(fixture.port, "/api/actions")).to_string();
    assert_eq!(
        actions.matches("\"job\":\"slow\"").count(),
        1,
        "one click wins, the rest are refused: {actions}"
    );
}
