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
  until implemented. `lib/gates.py`
- **An agent step** — prompt file, headless run, contract gate, staged output,
  never retried. `docs/agent-step.md`
- **A memory pass** — the last stage of every job, so an unattended agent obeys
  the same replace-never-append rules a session does. `docs/memory-pass.md`
- **Visibility** — `bin/report` renders every job's state from its state files.

## Quickstart

```sh
bin/job list                       # jobs that exist
bin/job status --all               # one table, red rows first
bin/job status --all --json        # same data for a page or a reminder
bin/job run noop                   # run the fixture job
bin/report --html out/status.html  # static page, no server
```

## Layout

```
docs/         run contract, agent step, memory pass, design notes
lib/          guards.sh, gates.py, memory.sh, report.py
bin/          job, report
jobs/<name>/  one directory per automation: job.sh, spec.md, state/, logs/, out/
spec/         the intent format, plus examples
tests/        a fixture job the kit runs against itself
notes/        the wiki: index, log, decisions
```

## Name

Lights-out manufacturing is the old name for a factory that runs with the lights
off: nobody is walking the floor, and the machinery is trusted to stop itself
safely when something is wrong.

## Tests

```sh
tests/run.sh    # 18 checks against the noop fixture: held, blocked, unblock, plan
```

## What it deliberately is not

Not a scheduler — launchd, cron, systemd, or a DSH reminder decides *when*.
Not an orchestrator — it does not hold a DAG in a database; the artifacts on
disk are the state. Not a workflow editor.

The rule that keeps it honest: a job is a directory, its state is files, and
`bin/job status` can rebuild the whole picture from those files alone.
