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
2. Then `lightsout triage everyday-laws` and one `lightsout heal everyday-laws`
   by hand, so the proposal, the validator and the gate are seen together.
3. Only then point the launchd plist at `bin/job run everyday-laws
   --auto-repair` and retire the old launcher.
4. Decide whether `bin/job check` (the stale alarm) becomes a launchd timer, so
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

## Auto-repair

Bounded auto-repair is implemented as of 2026-10-21. `triage.conf` routes a
blocked job to `human`, a deterministic `repair:<name>`, `agent:propose` or
`agent:apply`; the manager adds `triage` (read-only decision), `heal` (one
attempt, with `--apply` and `--revert N`) and `run --auto-repair`. An agent
proposal is validated against `repair.allow` (default `out/`), applied
atomically with a backup, and re-checked by the same gate; the same failure key
after an attempt stops the loop. Caps are `MAX_HEALS` (2 per job per day),
`MAX_HEALS_TOTAL` (5 per workspace per day), `MAX_AUTO_ATTEMPTS` (3 per
invocation) and `U_AGENT_TIMEOUT` (600 s). Read `docs/auto-repair.md` for the prompt contract and the failure
verdicts, because this page only records the state.

Verified 2026-10-21: `cargo test` passes with 36 auto-repair tests, the 261
ported checks, the workspace tests and the shell suite; `tests/install.sh`
remains green. v0.1.0 is published at
`https://github.com/darylfung96/lightsout/releases/tag/v0.1.0` (release
`419883596`, tag at `a50270c`), the four
archive checksums and `install.sh` were verified, the public installer
initialized a workspace and ran `noop`, and the Homebrew formula at 0.1.0
passes `brew audit --strict --online`. Keep the first real use supervised by
running `triage` and one `heal` by hand on `everyday-laws`, because a proposal
and a gate have not been seen together until they run on the real job; only
then add `--auto-repair` to launchd.

## Agent profiles

Agent profiles are implemented as of 2026-10-21. A job pins its runner and
model in `jobs/<name>/agent.conf`; `agents.conf` holds named profiles, and the
manager resolves the file once for the lesson call and once for the repair call.
A `dsh` profile points at the patch that pins provider, model and effort, and
the manager reads the patch's `agent-default-model` block to fill or check the
profile; `claude` and `codex` receive `--model`. The resolved profile is printed
by `plan`, carried by `triage` text and JSON, checked by `doctor`, and written
as the last column of `state/heal/history.tsv`. `jobs/everyday-laws/agent.conf`
now points at the video workspace's `headless.patch.yml`, so a repair no longer
falls back to the bare headless profile.

Verified 2026-10-21: `cargo test` passes 103 tests (27 unit, 7 agent-profile,
36 auto-repair, 1 manifest, 12 + 11 contract groups, 9 workspace), `tests/run.sh`
passes 261 checks, and `tests/install.sh` passes 11 checks. Because each path
has its own failure mode, the profile tests exercise named selection, dsh patch
validation, a model mismatch, the repair path, the missing-patch warning/refusal
and an unknown key. Commit `48a15de` is on `main`, and CI run `39645810534`
passed; release run `39648697624` also succeeded.

## Port shape

- `bin/job` and `bin/report` are launchers that find the built binary;
  `bin/lightsout` is the built artifact and is gitignored.
- `lightsout` owns `list`, `status`, `report`, `check`, `plan`, `diagnose`,
  `run`, `resume`, `retry`, `triage`, `heal`, `unblock` and `gate`.
- `lib/guards.sh` and `lib/memory.sh` call `$LIGHTSOUT_BIN gate`, so a job and
  the manager run the same gate implementation.
- `tests/tasks_01_12.rs` and `tests/tasks_13_23.rs` group the ported checks by
  the shell suite's task sections, and `tests/check_manifest.rs` asserts the
  per-section counts sum to 261.

## Open questions

- Should `job check` be the only stale detector, or should the dashboard compute
  it too? Both read the same files, so this is a display choice.
