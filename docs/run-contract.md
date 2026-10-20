# Run contract

Every job in `jobs/` implements this interface and nothing invents its own.

## Verbs

| verb | meaning | exit |
|---|---|---|
| `job plan <name>` | say what would run now; touch nothing | 0 |
| `job run <name>` | do the work, resuming from disk | 0 / 1 / 2 |
| `job status [name]` | describe state from files | 0 |
| `job diagnose <name>` | explain the current stop from files; writes `state/diagnosis.md`, changes nothing else | 0 |
| `job resume <name>` | identical to `run`; kept as a word for humans | 0 / 1 / 2 |
| `job unblock <name>` | clear a blocked marker, after a human looks | 0 |

`plan` is not a simulation of the code path; it is the same guards run in a
read-only mode that reports what `run` would decide.

## Exit codes

- `0` — finished, or deliberately held (already done today, another run holds
  the lock, the day guard says wait). Held is not a failure.
- `1` — failed. Something is broken and retrying may fix it.
- `2` — blocked. Something needs a human: a gate failed, a budget ran out, a
  credential expired. **Retrying will not fix it.**

The distinction is the whole point. A blocked job notifies once and then stays
quiet while still exiting `2`, so a scheduler sees a persistent non-zero state
without a notification every ten minutes.

## Environment a job receives

`bin/job` sets these before sourcing the job:

```
REPO      repository root
JOB       job name
JOB_DIR   jobs/<name>
STATE     jobs/<name>/state
OUT       jobs/<name>/out
LOG       jobs/<name>/logs/<YYYY-MM-DD>.log
UNIT      the unit of work: a date by default, overridable per job
DRY       1 for plan, 0 for run
U_BLOCK_CLASS   default class for a block raised in this run (human unless
                `bin/job run --class` says otherwise)
U_BLOCK_SOURCE  default source for a block raised in this run (unknown unless
                `bin/job run --source` says otherwise)
```

## State layout

```
state/
  lock/pid            one run at a time; stale pids are reclaimed
  attempts/<date>.attempt-N
  done/<unit>         this unit finished; the day guard reads it
  blocked             <iso>TAB<class>TAB<source>TAB<reason> — one line, current reason only
  diagnosis.md        read-only report from `job diagnose`; the block is its input, not its output
  last                <iso>TAB<exit code>TAB<verb>
```

`lib/diagnose.py` reads the blocked line, the log up to the block, and `out/`,
then writes `state/diagnosis.md` and exits `0` even while the job stays blocked,
so a human can read what happened without disturbing the stop. Because only a
human (`unblock`) or a passing run (`u_done`) may clear the marker, diagnose
never clears it and never runs a repair; its report is the only file it writes.

The blocked line's class says what may be done about it: `retryable` (an
allowlisted repair may fix it, so `job retry` is offered), `judgment` (a human
must edit the artifact and re-run the gate), or `human` (credentials, disk, a
decision). `source` names what raised the block and defaults to `unknown`. A
two-field line written by an older run reads as `human` with field 2 as the
reason. `bin/job run` and `resume` accept `--class C --source S` to set the
default for every block raised during that run.

`done/<unit>` is the notebook: a restarted job asks "is my output already on
disk?" and skips. It is the single most important file in the directory, and
every stage that costs money must consult it.

## Log lines

One line per decision, timestamped, appended to `$LOG` and echoed to stdout:

```
[2026-10-20 09:17:26] stage 3: voice kept (radio 59.980s, bounds present)
```

A run's progress is the sequence of `stage N:` lines, which is what the
dashboard renders. Write them before the slow work, not after.

## The rules behind the surface

1. **Resume from disk.** A stage that finished is detected from its artifact,
   never from a variable in memory.
2. **At most once per day.** Cheap guard first, so a tick that cannot work does
   not start services or take a lock.
3. **Attempt cap.** N failures per day, then blocked — because a broken pipeline
   retrying every ten minutes is how a disk fills.
4. **Lock.** A render can outlive several ticks; a stale pid is reclaimed.
5. **Blocked, not failed.** Failure is retryable, blocked is not, and only
   blocked notifies a human.
