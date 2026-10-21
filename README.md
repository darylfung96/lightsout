# LightsOut

A standard office for jobs that run without a human.

Each automation is a robot worker. Every robot needs the same four things: an
alarm clock, a notebook of what it already finished, a red light for when it is
stuck, and a checklist that decides whether its output is good. Building those
by hand per automation is how you get five robots that each fail differently.

This repository builds them once.

## What it gives a job

- **A run contract** — `plan`, `run`, `status`, `resume`, plus exit codes that
  distinguish *done*, *failed*, and *blocked, needs a human*. `docs/run-contract.md`
- **Guards** — day guard, attempt cap, single-run lock, disk check, blocked
  marker, one desktop notification. `lib/guards.sh`
- **Gates** — named pass/fail checks on artifacts, each a stub that fails closed
  until implemented. `lightsout gate`
- **An agent step** — prompt file, headless run, contract gate, staged output,
  never retried. A per-job profile pins the runner and model, and the repair
  path uses the same one. `docs/agent-step.md`
- **Bounded auto-repair** — `triage.conf` routes a blocked job to a
  deterministic repair, an agent proposal, an agent apply, or a human; caps,
  validation and backups bound the attempt. `docs/auto-repair.md`
- **A memory pass** — the last stage of every job, so an unattended agent obeys
  the same replace-never-append rules a session does. `docs/memory-pass.md`
- **Visibility** — `bin/report` renders every job's state from its state files.

## Install

```sh
brew install darylfung96/lightsout/lightsout
# or use the checksum-verified release installer in docs/install.md
lightsout init ~/my-office         # write a versioned workspace
cd ~/my-office
lightsout doctor                   # check the workspace and its tools
lightsout run noop                 # the fixture proves the loop
```

A release installer, Homebrew formula and the workspace marker are documented
in [docs/install.md](docs/install.md). The repository itself is a workspace, so
the development commands below still work from this checkout.

## Quickstart in this checkout

```sh
cargo build --release              # build the manager once
bin/job list                       # jobs that exist
bin/job status --all               # one table, red rows first
bin/job status --all --json        # same data for a page or a reminder
bin/job run noop                   # run the fixture job
bin/report --html out/status.html  # static page, no server
```

## Layout

```
docs/         run contract, install, agent step, memory pass, design notes
src/          the Rust manager: verbs, gates, dashboard, diagnose, workspace
lib/          the shell job libraries: guards, locks, notify, agent, memory, repairs
bin/          job and report launchers, plus scaffold
jobs/<name>/  one directory per automation: job.sh, spec.md, state/, logs/, out/
spec/         the intent format, plus examples
triage.conf   the workspace auto-repair policy
agents.conf   named agent profiles (a job opts in with jobs/<name>/agent.conf)
tests/        the 261 contract checks, auto-repair tests and workspace tests
packaging/    Homebrew formula and macOS signing helper
install.sh    checksum-verifying release installer
lightsout.toml  this repository's workspace marker
notes/        the wiki: index, log, decisions
```

## Name

Lights-out manufacturing is the old name for a factory that runs with the lights
off: nobody is walking the floor, and the machinery is trusted to stop itself
safely when something is wrong.

## Tests

```sh
cargo test      # 261 ported checks, auto-repair, agent-profile and workspace tests (macOS)
tests/run.sh    # the same 261 checks through the shell jobs and the binary
tests/install.sh  # installer checks against a local fake release (portable)
```

`jobs/<name>/job.sh` stays shell because it is the project-specific part, and it
calls the shared `lib/*.sh` guards that every job uses. The manager's verbs,
exit codes, gates, status, report and diagnose live in the single `lightsout`
binary, so there is one implementation, one exit-code contract and one cargo
test suite; the shell suite stays as a second executable check of the same
contract.

## What it deliberately is not

Not a scheduler — launchd, cron, systemd, or a DSH reminder decides *when*.
Not an orchestrator — it does not hold a DAG in a database; the artifacts on
disk are the state. Not a workflow editor.

The rule that keeps it honest: a job is a directory, its state is files, and
`bin/job status` can rebuild the whole picture from those files alone.
