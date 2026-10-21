# Decisions

## D1 — State is files, not a database

A job's state lives under `jobs/<name>/state/`: a lock, attempts, done markers,
one blocked line, one last-run line. The dashboard rebuilds everything from
those files. A database would let the record drift from the artifacts, and the
artifacts are what a viewer sees.

## D2 — Three exit codes, and blocked is not failed

`0` done or held, `1` failed, `2` blocked. Failure is retried; blocked notifies
once and then persists quietly. Collapsing the two is how a pipeline retries
every ten minutes against a problem only a human can fix.

## D3 — Gates fail closed

An unimplemented gate is `lightsout gate never`, so a job cannot reach publish
while a check is missing. As of 2026-10-20 the manager is the Rust binary, and a
shell job spells the same fail-closed stub `u_gate never`; this is the
difference between an automation that reports what it still needs and one that
is confidently wrong.

## D4 — The memory pass is a stage, not a habit

Every job that writes notes ends with it, and its failure blocks the run. An
unattended agent obeys the same replace-never-append rules a session does,
because the next session reads the same store.

## D5 — The spec is written before the code

Every stage names a gate with numbers. A job that cannot state its unit cannot
know what it already finished.

## D6 — The binary is global, the workspace is local

A workspace is a directory with a `lightsout.toml` marker, because the marker
lets the binary find project state without guessing from an ancestor named
`jobs/`. `lightsout init` writes the marker plus the runnable office, and
`lightsout doctor` checks the schema, libraries, jobs and the tools those jobs
actually reference. The binary is installed globally through Homebrew, cargo or
the checksum-verifying release installer, because the manager upgrades
independently of the job state; `--workspace` and `LIGHTSOUT_REPO` select a
workspace explicitly for commands that run outside it. Verified 2026-10-21:
`init`, `doctor`, the workspace launchers and the installer pass their tests,
and both 261-check suites remain green.

## D7 — Auto-repair is bounded and routed by a human policy

A blocked job may be repaired by the manager, because the state files already
name the failure class and the source, and a small set of failures is
environmental. `triage.conf` maps `(job, source)` to `human`, a deterministic
repair, an agent proposal, or an agent apply; a human edits the file and the
manager never learns a rule. An agent attempt returns a JSON proposal of file
edits, the validator refuses every path outside the job's allowlist and every
path that judges the work, and the manager applies the batch atomically with a
backup so `heal --revert` can restore it. The same failure key after an attempt
stops the loop, because a fix that does not move the failure is not progress;
per-job and per-workspace caps bound the bill. The owner chose `agent:apply` as
the default; `agent:propose` remains for a source that needs approval. The gate
still decides, because the proposal is input and no agent can mark a gate
passed.

## D8 — Agent model choice is a per-job profile

A job's agent runner and model live in `jobs/<name>/agent.conf`, because the
lesson writer and the repair that follows it must use the same pin; a model
chosen at runtime would make a passing pipeline a different pipeline next week.
`agents.conf` holds named profiles for sharing, and a job selects one with
`profile = <name>`. For `dsh` the profile points at the patch that pins
provider, model and effort, since `dsh` has no `--model` flag; the manager reads
the patch and refuses a profile that contradicts it. Because `claude` and `codex`
accept `--model`, their adapter passes the profile's `model` as that argument.
Environment variables still override the file for one invocation, so an
operator can test a different runner without editing the job. The resolved
profile appears in `plan`, `triage`, `doctor` and the heal history, because the
pin has to be visible after the call. The manager never writes the profile,
because a learned model rule would hide which model ran.

## D9 — A job is a directory, and the registry names it

A job does not have to live in the workspace that owns `lib/` and
`triage.conf`, because a backup or a log rotation has no reason to sit in the
LightsOut checkout. `lightsout job add` writes `$LIGHTSOUT_HOME/jobs.json`,
which maps a name to a job directory, an optional workspace and an optional
workdir; `run`, `status`, `check`, `triage`, `heal` and `diagnose` resolve the
name through it, so the current directory does not have to be a workspace. A
workspace job of the same name wins, because the operator pointed at that
workspace and its definition is the one they can see. `job add --command`
generates a command job whose exit code is the gate, so "anything" means an
arbitrary command under the same guards and state files. `schedule` prints the
cron line or launchd plist and installs nothing, because cron or launchd still
owns *when* and a crashed scheduler must not take the run history with it. A
malformed registry fails closed with exit 2 instead of falling back to a
workspace job of the same name.

Verified 2026-10-21: the generated command job's exit code is the gate in
`tests/universal_mode.rs`.
