# Log

## [2026-10-20] decision | LightsOut started as a standard office for unattended jobs

Created the repository after the Everyday Laws daily pipeline needed the same
guards twice: resume-from-disk, a day guard, an attempt cap, a blocked state that
notifies, and a memory pass. The six-verb run contract, `lib/guards.sh`,
`lib/gates.py`, and the `noop` fixture job are in place; the scaffolder and the
agent adapter are next.

## [2026-10-20] decision | all fourteen gaps closed; the video job is ported

The plan at `docs/superpowers/plans/2026-10-20-close-the-gaps.md` is implemented
task by task, each by a fresh agent that had to leave `tests/run.sh` green and
commit once: blocked failures now carry a class, `diagnose` writes a read-only
cause, `retry` runs only an allowlisted repair and lets the gate decide again,
attempts and repairs are capped per stage, spend is a ledger, a run that never
happened is detected, notifications reach more than the desktop, run history and
resource locks exist, the footage approval manifest is a gate, and the memory
pass is enforced by a fake store the tests can fail on purpose. The daily video
pipeline is ported as `jobs/everyday-laws`.

Two defects were found by review after the subagents signed off. The ported job
treated the driver's own one-per-day hold as a failed gate, so a hold became a
block, and a block refuses every later run: the first unattended night would
have killed the pipeline. It also spent an attempt per held tick, reaching the
cap in half an hour. The job now asks the driver's rule before counting an
attempt and treats a driver that exits 0 without a master as held. Separately,
two concurrent `tests/run.sh` runs trampled the same state directories and
reported phantom failures, so the suite now refuses to run twice at once.

## [2026-10-20] decision | the manager is Rust while the jobs stay shell

The manager was ported to one `lightsout` binary because the shell wrapper and
the two Python helpers had grown into three parsers of the same state files, and
a typed Rust parser removes the bash-version drift the earlier plan had to work
around. `bin/job`, `lib/gates.py`, `lib/report.py` and `lib/diagnose.py` are gone;
`jobs/*/job.sh` and the shared `lib/*.sh` guards stay shell because they are the
project-specific work. The shell suite stayed as the executable spec: its 261
checks were ported task by task into `cargo test`, and both suites now print 261
passing checks. `lib/guards.sh` and `lib/memory.sh` call `$LIGHTSOUT_BIN gate`,
so the job and the manager share one gate implementation. Verified 2026-10-20:
`cargo test` passed 29 tests with 261 ported checks, and `tests/run.sh` passed
all 261 shell checks against the binary.

## [2026-10-21] decision | workspace lifecycle and release packaging follow the CLI conventions

The distribution now installs a global binary and creates a project-local
workspace, because the manager and the job state have different upgrade
lifecycles. `lightsout init` writes `lightsout.toml` plus the six shell
libraries, `jobs/noop`, the spec example, the scaffold and the workspace
launchers; `lightsout doctor` checks the schema, files and the tools a job
actually references. `install.sh` pins version 0.1.0 by default, downloads a
release archive, verifies its SHA-256 and never uses sudo. The release workflow
builds four target archives with checksums, and the Homebrew formula builds from
source so the macOS tap avoids a quarantined binary. Verified 2026-10-21: the
new `tests/workspace_cli.rs` passes 7 tests, `tests/install.sh` passes 11 checks
including a tampered-asset refusal and an out-of-band hash pin, and the 261-check cargo and shell suites
remain green.

## [2026-10-21] decision | v0.1.0 is the single published version

The repository is public at `darylfung96/lightsout`, because the anonymous
`curl` installer and the Homebrew tap need unauthenticated release assets.
There is one release tag, `v0.1.0`, and it carries the whole feature set: the
run contract, the workspace lifecycle, bounded auto-repair and agent profiles.
The release workflow builds four targets and publishes checksums plus
`install.sh`, and the tap at `darylfung96/homebrew-lightsout` builds from the
tag tarball. The tag is `v0.1.0` at `a50270c`, release `419883596`, and the
formula pins sha256
`b99eecaaf4b22cfc08355f81dac3416a3df88b4c27d3f2365d97b50e3522dac9`.
Verified 2026-10-21: the release workflow passed, a public install
from the release URL verified its checksum, initialized a workspace, passed
`doctor` and ran `noop`, and the Homebrew formula passed
`brew audit --strict --online`. The release also carries the migration from
`~/pi-memory` to `~/agent-memory`, because the store moved while the features
were in flight.


## [2026-10-21] decision | bounded auto-repair lands behind a policy and caps

Implemented the plan at `docs/superpowers/plans/2026-10-21-auto-repair.md`
through the feature tasks: `triage.conf` plus a read-only `triage` verb,
`heal` propose/apply/revert, `run --auto-repair`, the JSON proposal validator,
atomic apply with backups, the progress check, and the workspace heal budget.
The manager never learns a rule, because the policy is human-edited; the
validator refuses `job.sh`, `spec.md`, `state/` and `lib/`, and the same failure
key after an attempt stops the loop, so a fix that does not move the failure
cannot spend the next attempt.
The owner chose `agent:apply` as the default, so `agent:propose` is the
approval opt-in.

The feature landed with its tests green; the documentation pass, the release
and the first supervised `heal` on `everyday-laws` remained open at that point.


## [2026-10-21] decision | agent profiles pin one runner for a job and its repair

Added `jobs/<name>/agent.conf` and workspace `agents.conf`, because the video
job's lesson call used the video patch while `lightsout heal` ran a separate
manager process with no patch. The resolver normalises runner, model, provider,
patch and effort; for `dsh` it reads the patch's `agent-default-model` block
and refuses a contradiction, and for `claude`/`codex` it passes the model flag.
`plan`, `triage`, `doctor` and the heal history now name the resolved profile.
`jobs/everyday-laws/agent.conf` points at
`~/programming/video-automation/automation/headless.patch.yml`, so the lesson
and the repair use the same OpenCode Zen model.

Verified 2026-10-21: `tests/agent_profile.rs` passes 7 tests, and the existing
auto-repair, workspace and 261-check suites pass. Committed as `48a15de` on
`main`; CI run `39645810534` succeeded.

## [2026-10-21] decision | video-prune removes only old posted render intermediates

Added `jobs/video-prune` because the video checkout had reached 98% capacity
with 4.4 GB free, and the only cleanup was `daily-build.sh`'s emergency janitor,
which waits for 2.5 GB and then removes every render directory it finds. The new
job keeps the last three episodes fully intact and removes only
`episodes/<NNN>/comp/*/renders/` for older episodes whose master exists and whose
`pipeline/publish/queue.json` entry is `posted`; media, audio, comp assets,
snapshots, the outbox and the queue stay untouched, so the master remains and a
recent episode can still be re-rendered. `PRUNE_APPLY=0` is the default and
writes a plan; the launchd form sets `PRUNE_APPLY=1`.

The first dry-run listed ten directories (3,521.6 MB) for episodes 016-025; the
apply removed all ten and moved free space from 3,605 MB to 7,131 MB. A
kickstart after the done marker existed logged `held: 2026-10-21 already done`,
which is the day guard doing its job. The plist is
`packaging/launchd/com.everydaylaws.video-prune.plist`, loaded as
`com.everydaylaws.video-prune` at 23:30 daily. No agent is involved and
`U_MEMORY=0` is explicit because the job writes no notes.

Verified 2026-10-21: `tests/video_prune.rs` passes 4 tests (dry-run, apply, plan-is-read-only
and symlink refusal), `cargo test` passes 107 tests, `tests/run.sh` passes 261
checks, and `tests/install.sh` passes 11 checks.

## [2026-10-21] decision | a job can be registered outside a workspace

Added `lightsout job add|list|remove` and `$LIGHTSOUT_HOME/jobs.json`, because
the review handoff asked whether LightsOut could be universal like cron. The
registry maps a name to a job directory, an optional workspace, workdir and
state directory, and `run`, `status`, `check`, `triage`, `heal` and `diagnose`
resolve it when the current workspace has no job of that name; a workspace job
still wins, so nothing in this repository changed behaviour. `job add
--command` writes a generated command job whose exit code is the gate, and
`init --home` writes the `lib/` templates that job sources, so a command job
needs no workspace at all. `schedule` prints the cron line or launchd plist
from `state/expect` (or `--at`) and installs nothing, because cron and launchd
still own *when* and a crashed scheduler must not take the run history with it.
A malformed registry fails closed with exit 2, and `doctor` checks every entry.

The review fixes from the same session are committed as `12c6658`; they cover
the stale-proposal and revert-order refusals, the workspace-level resource
locks, the shared bounded watchdog, the dead-lock check, the per-day heal cap
and the status/report truthfulness fixes.

Verified 2026-10-21: `tests/universal_mode.rs` passes 9 tests, `cargo test`
passes 139 tests, `tests/run.sh` passes 261 checks, `tests/install.sh` passes
11 checks, and `prose-lint.py` passes the changed docs. Committed as `78b1cce`
on `main`.
