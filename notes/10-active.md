# Active

What is in flight, so the next session knows where to start. Replace this page;
do not append to it.

## Now

The Rust port is complete as of 2026-10-20: `src/` builds the `lightsout`
binary, and it replaces `bin/job`, `lib/gates.py`, `lib/report.py` and the old
`diagnose.py`. The 261 checks in `tests/run.sh` were ported task by task to
`cargo test`, and both suites pass; the shell suite now builds the release
binary and drives the same checks through the shell jobs. `jobs/<name>/job.sh`
and `lib/*.sh` stay shell because the job is the project-specific part, while
the manager supplies the environment, records history, handles blocks, checks
stale runs, runs allowlisted repairs and renders status. The manager's own
verbs and parsers are compiled into the binary, so no verb is implemented in
`python3`; `gate memory` still runs the memory store's own Python scripts
because that store owns them, and a job may still use Python for its own work.

Verified 2026-10-20: `cargo test` reported 29 tests and 261 ported checks, and
`tests/run.sh` printed 261 `ok` lines and `all checks passed`.

Next, in order:

1. One supervised run: `bin/job run everyday-laws` by hand, comparing the master
   to the previous night's before trusting it overnight.
2. Only then point the launchd plist at `bin/job run everyday-laws` and retire
   the old launcher.
3. Decide whether `bin/job check` (the stale alarm) becomes a launchd timer, so
   a night with no run announces itself.

## Distribution

Workspace lifecycle and packaging are in place as of 2026-10-21. `lightsout
init` writes `lightsout.toml` plus the shell libraries, `jobs/noop`, `spec/`
and the workspace launchers; `lightsout doctor` checks the marker, schema,
libraries, job scripts and the tools those jobs actually reference. The
`install.sh` release installer verifies a SHA-256 before writing, and the
release workflow builds macOS arm64/x86_64 and Linux x86_64-musl/aarch64
archives with checksums. A Homebrew formula template and optional macOS
codesigning/notarization helper live under `packaging/`.

Verified 2026-10-21: `cargo test` passes with the 261 contract checks plus 7
workspace lifecycle tests, both 261-check suites remain green, and
`tests/install.sh` passes against a local fake release, including a
tampered-asset refusal and an out-of-band pinned hash.

## Port shape

- `bin/job` and `bin/report` are launchers that find the built binary;
  `bin/lightsout` is the built artifact and is gitignored.
- `lightsout` owns `list`, `status`, `report`, `check`, `plan`, `diagnose`,
  `run`, `resume`, `retry`, `unblock` and `gate`.
- `lib/guards.sh` and `lib/memory.sh` call `$LIGHTSOUT_BIN gate`, so a job and
  the manager run the same gate implementation.
- `tests/tasks_01_12.rs` and `tests/tasks_13_23.rs` group the ported checks by
  the shell suite's task sections, and `tests/check_manifest.rs` asserts the
  per-section counts sum to 261.

## Open questions

- Should `job check` be the only stale detector, or should the dashboard compute
  it too? Both read the same files, so this is a display choice.
