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

## [2026-10-21] decision | v0.1.0 is published, with a public installer and tap

The repository is public at `darylfung96/lightsout`, because the anonymous
`curl` installer and the Homebrew tap need unauthenticated release assets. The
tag `v0.1.0` built four targets, published checksums plus the installer and its
checksum, and the tap lives at `darylfung96/homebrew-lightsout`; the formula
builds from the tag tarball and its checksum was pinned after the first
publish attempt exposed a missing checkout in the release job. Verified
2026-10-21: the release workflow passed, a public install from the release URL
verified its checksum, initialized a workspace, passed `doctor` and ran `noop`,
and the Homebrew formula passed `brew audit --strict --online`.


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

Verified 2026-10-21: `cargo test` passes with 36 auto-repair tests, the 261
ported checks, the workspace tests and the shell suite; `tests/install.sh` and
`cargo clippy` are clean. Still open: Task 7 documentation, Task 8's 0.2.0
release, and the first supervised `heal` on `everyday-laws`.
