# Design

## The office

Every automation is a robot worker. Each one needs an alarm clock, a notebook of
what it already finished, a red light for when it is stuck, and a checklist that
decides whether its output is good. Building those by hand for every robot
produces robots that fail in different ways; building them once produces a
building where a new robot is mostly paperwork.

## The four failures this prevents

1. **The silent stall.** A job that cannot proceed exits `2`, records why, and
   notifies once. The original failure this repository is named after: a
   pipeline dead for six hours while every tick logged that it was waiting for a
   human.
2. **The half-done rerun.** Work that already exists is detected on disk, so a
   restart resumes instead of re-paying for the expensive stages.
3. **The double spend.** One lock across ticks, two attempts a day, then a
   human.
4. **The stale note.** The memory pass is the last stage, so a run that changed
   the world also records the change.

## Boundaries

Not a scheduler: launchd, cron, systemd, or a DSH reminder decides when.
Not an orchestrator: no database of task instances; the artifacts are the state.
Not a workflow editor: nothing in the browser may change a schedule.
Not a provenance system: it records what passed a gate, not why someone chose
the gate.

## What would make this wrong

- **One job.** With a single job the kit is overhead; the guards belong inside
  that job.
- **Abstraction before a second tenant.** A helper added for one job's
  convenience is a guess. It earns its place when an unlike job needs it.
- **Generating the gates.** Scaffolding may be generated freely; acceptance
  criteria come from a failure that actually happened. A generated gate checks
  what someone imagined, which is how a cow ships as a staff badge.
