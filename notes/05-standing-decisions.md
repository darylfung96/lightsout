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
