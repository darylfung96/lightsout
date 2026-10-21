# Run contract

Every job in `jobs/` implements this interface and nothing invents its own.

## Verbs

| verb | meaning | exit |
|---|---|---|
| `job plan <name>` | say what would run now; touch nothing | 0 |
| `job run <name>` | do the work, resuming from disk | 0 / 1 / 2 |
| `job status [name]` | describe state from files | 0 |
| `job check [name]` | alarm when a run that was due never happened; one notification per job per day | 0 / 2 |
| `job diagnose <name>` | explain the current stop from files; writes `state/diagnosis.md`, changes nothing else | 0 |
| `job retry <name>` | run an allowlisted repair for a `retryable` block, then the job again so the same gate decides | 0 / 1 / 2 |
| `job triage <name>` | print how the current block routes through `triage.conf`; touch nothing | 0 / 64 |
| `job heal <name>` | run one agent attempt in the policy's mode; `--apply` applies the newest stored proposal, `--revert N` restores attempt N | 0 / 1 / 2 / 64 |
| `job run <name> --auto-repair` | run, then make bounded repair or agent attempts from the policy | 0 / 1 / 2 / 64 |
| `job resume <name>` | identical to `run`; kept as a word for humans | 0 / 1 / 2 |
| `job unblock <name>` | clear a blocked marker, after a human looks | 0 |

`plan` is not a simulation of the code path; it is the same guards run in a
read-only mode that reports what `run` would decide.

## Triage policy

`triage.conf` at the workspace root maps `(job, source)` to one of four
actions — `human`, `repair:<name>`, `agent:propose` or `agent:apply`; a human
edits the file and the manager only reads it. Write one rule per line,
whitespace-separated; `#` starts a comment and `*` is a wildcard. Resolution
tries exact `(job, source)`, then `(job, *)`, then `(*, source)`, then
`(*, *)`; the first match wins and a duplicate exact rule is a config error. A
missing file routes every block to `human`, so auto-repair is opt-in.

`lightsout triage <job>` prints the action, the matching rule, the failure key
and today's attempt counts, and writes nothing, because the decision has to be
inspectable before anyone acts on it. `--json` is the same data for a
scheduler. A malformed file makes `triage`, `heal` and `run --auto-repair` exit
`64` with the line number; plain `run`, `status`, `report` and `check` never
read it.

## Auto-repair

`lightsout run <name> --auto-repair` (or `LIGHTSOUT_AUTO_REPAIR=1`) wraps a
normal run in a bounded loop. It reads `triage.conf`, performs the action for
the current block, re-runs the job, and compares the new failure key with the
old one; a different key earns another attempt, while the same key stops the
loop because the fix did not move the failure.

| setting | scope | default |
|---|---|---|
| `MAX_HEALS` | agent calls per job per day | 2 |
| `MAX_HEALS_TOTAL` | agent calls per workspace per day | 5 |
| `MAX_AUTO_ATTEMPTS` | attempts per invocation | 3 |
| `MAX_REPAIRS` | deterministic repairs per job per day | 2 |
| `U_AGENT_TIMEOUT` | seconds per runner call | 600 |
| `U_RUNNER` | the job's adapter (`dsh`, `claude`, `codex`, `fake`) | profile or `dsh` |
| `U_HEAL_RUNNER` | the repair adapter; overrides `U_RUNNER` for a heal | profile or `dsh` |
| `U_AGENT_MODEL` | adapter model id; passed to `claude`/`codex`, checked for `dsh` | profile |
| `U_AGENT_PATCH` | `dsh` overlay that pins provider, model and effort | profile |
| `U_AGENT_EFFORT` | `dsh` reasoning effort; checked against the patch | profile |
| `U_AGENT_PROFILE` | the resolved profile, set by the manager for the job | resolved |

An agent returns a JSON proposal of file edits under `jobs/<name>/` ; the
manager validates every path before it writes, because the proposal is data and
the prompt is not a boundary. The default allowlist is `out/` , widened by
`jobs/<name>/repair.allow` ; `job.sh` , `spec.md` , `state/` , `logs/` , `lib/`
, and `spec/` are refused even when an allow entry names them. Apply is atomic
per file with a backup, so a failed batch rolls back and `heal --revert` can
restore an applied attempt. The gate that rejected the artifact is still the
only thing that may pass it; an agent edit is input, not a verdict.

The manager does not sandbox the agent, because adapter-level read-only modes
are a follow-up; v1 takes a metadata fingerprint of the job and code directories
before the runner, detects unexpected writes, and refuses the proposal. Read
`docs/auto-repair.md` for the prompt contract, the validation rules, and the
failure verdicts, because this contract only names the interface.

## Exit codes

- `0` — finished, or deliberately held (already done today, another run holds
  the lock, the day guard says wait). Held is not a failure.
- `1` — failed. Something is broken and retrying may fix it.
- `2` — blocked. Something needs a human: a gate failed, a budget ran out, a
  credential expired. **Retrying will not fix it.**

The distinction is the whole point. A blocked job notifies once and then stays
quiet while still exiting `2`, so a scheduler sees a persistent non-zero state
without a notification every ten minutes.

## Workspace

A workspace is a directory with a `lightsout.toml` marker recording `schema`
and the `manager` version that wrote it. `lightsout init` writes the marker and
the minimum runnable office; `lightsout doctor` checks it. A job may add
`jobs/<name>/agent.conf` to pin its agent runner and model, and the workspace
may add `agents.conf` with named profiles a job selects by name; a missing
`agent.conf` leaves the runner's own default in place. Read
[agent-step.md](agent-step.md) for the grammar and precedence. The manager
walks up from the current directory for the marker, or from its own installation
path, and accepts `--workspace DIR` or `LIGHTSOUT_REPO` as an explicit
override. A
legacy checkout with `jobs/` and `lib/` but no marker is still readable, and
`doctor` reports it as a legacy workspace.

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
LIGHTSOUT_BIN   path to the manager binary, so lib/guards.sh runs the same
                gate implementation
```

## State layout

```
state/
  lock/pid            one run at a time; stale pids are reclaimed
  attempts/<date>.attempt-N
  done/<unit>         this unit finished; the day guard reads it
  blocked             <iso>TAB<class>TAB<source>TAB<reason> — one line, current reason only
  diagnosis.md        read-only report from `job diagnose`; the block is its input, not its output
  repairs/<date>.count repairs run today; `MAX_REPAIRS` (default 2) caps the loop
  heal/<date>/attempt-N/  one repair or agent attempt: prompt, proposal, verdict, backup
  heal/history.tsv    one line per attempt, including repair and apply events;
                      the last column is the resolved agent profile
  last                <iso>TAB<exit code>TAB<verb>
  start               <iso>TAB<epoch> written before the job runs, so the history has the job's own clock
  runs.tsv            <start-iso>TAB<end-iso>TAB<rc>TAB<verb>TAB<unit>TAB<seconds>, one line per completed run
  expect              one line, HH:MM local: the time a run is due
  alerts/<date>.stale the stale alarm for this job was raised today
```

Workspace-level resources live outside a job, under
`$REPO/state/resources/<name>.lock/` (`pid`, `expiry`), because two jobs must
contend for the same path; see “Resource locks” below.

`lightsout diagnose` reads the blocked line, the log up to the block, and `out/`,
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

A repair runs through `job retry` after two checks, because a repair executes
with the job's privileges: the block's class must be `retryable`, and the
requested name must be defined as `repair_<name>()` in `lib/repairs.sh`. The
registry has to be reviewable for the same reason, so an unknown name exits `2`
and touches nothing; every class other than `retryable` is refused the same way,
because a `judgment` block needs an edited artifact before anything re-runs.
Repairs are counted in `state/repairs/<date>.count`, and `MAX_REPAIRS` (default
2) stops the loop that a repair which never fixes anything would otherwise
create. `--repair R` chooses a registered repair; the default is `touch-missing`
(the noop fixture's repair, also settable with `NOOP_REPAIR`). A repair may
change the world but never the verdict, because only the gate that rejected the
artifact may pass it: the retry leaves `state/blocked` in place, re-runs the
job, and the marker stays until the job passes and `u_done` clears it or blocks
again.

`done/<unit>` is the notebook: a restarted job asks "is my output already on
disk?" and skips. It is the single most important file in the directory, and
every stage that costs money must consult it.

## A run that never happened

`state/expect` is one line, `HH:MM` local: the time a run is due. Whoever
schedules the run writes it before the run, because a run cannot report that it
failed to start. A job whose time has been reached with no `done/<today>`, no
`lock`, no `blocked` marker and no run today that ended held has missed a run,
and `status --json` reports `stale: true` for it.

A hold is not a miss. Another run holding the `lock` is doing the work only when
the pid in `state/lock/pid` is still running; a lock directory left by SIGKILL
or a reboot is not evidence of work, so `check` and `status` ignore it and the
missed run is reported. A run that exits `0` without writing `done` was
deliberately held (the day guard, the lock, or a busy resource); `bin/job`
records every completed run in
`state/last`, so rc `0` without `done` is the file evidence of that hold and it
suppresses stale for the rest of the day. A job with no `expect` file is never
late; inventing a schedule from its history would raise alarms nobody
configured.

`bin/job check` is the alarm. For every stale job it appends one notification
through `u_notify` and writes `state/alerts/<date>.stale`, so a scheduler calling
it every ten minutes raises one alarm a day instead of one per tick. A blocked
job is not reported stale, because the block is the more specific stop and it
notified when it was raised, but `check` exits `2` for a blocked job that missed
its run: the scheduler keeps seeing a non-zero state without a notification every
tick. `check` reads and notifies; it never runs a job, because an alarm that
repaired would be a second runner.

## Run history

`state/last` holds the last 50 completed runs as `<iso>TAB<rc>TAB<verb>`,
because the stale check needs only the newest line. `state/runs.tsv` writes the
same event with a duration, one line per completed run:
`<start-iso>TAB<end-iso>TAB<rc>TAB<verb>TAB<unit>TAB<seconds>`. A held run, a
failed run and a retry are appended the same way as a success, because the
history is a record of what the scheduler did, not only of the work that
finished.

Before it execs the job, `bin/job` writes `state/start` as `<iso>TAB<epoch>`,
and `record` subtracts that start from the clock when the job returns. Timing
the whole `bin/job` process instead would turn a fast held tick into a slow run
and hide the job's own time. A missing or unreadable start line records zero
seconds so that one damaged file cannot remove a run from the history.

`status --json` reports the last ten runs as `runs`, oldest first, each with
`start`, `end`, `rc`, `verb`, `unit` and `seconds`, plus the newest run's
integer `last_duration` (or `null` before the first run). The HTML table from
`lightsout report --html` adds `duration` and `last 5 runs`, because the page is read
by a person scanning for a run that got slower and the file keeps the full
history.

## Resource locks

`u_resource <name>` (`lib/locks.sh`, sourced by `lib/guards.sh`) takes a
cross-job resource before the stage that needs it, because two jobs that share a
GPU, a media encoder or an API slot have to take turns rather than collide. A
job takes the resource after the run lock and before an attempt is counted,
because a tick that found the resource busy did not try and must not spend the
day's attempt budget on it.

The lock is `$REPO/state/resources/<name>.lock`, a directory holding `pid` and
`expiry` (a legacy one-line file is still read). It is workspace-level on
purpose: a lock under `jobs/<name>/state/` would give every job its own copy and
never make two jobs contend. `mkdir` is the atomic claim, so two jobs starting
at the same moment cannot both acquire it; a stale lock is renamed out of the
way and replaced, and the loser sees the winner's new lock and reports busy.

The pid names the holder, and the deadline (`U_RESOURCE_TTL`, default 3600
seconds) marks the point after which the lock is stale even while that pid is
alive, so a wedged holder cannot stall every later run. A lock whose pid no
longer runs is stale too, so the directory SIGKILL leaves behind is reclaimed by
the next run instead of deadlocking it. A live lock makes the run wait up to
`U_RESOURCE_WAIT` seconds (default 0) and then exit `0` held, with `held:
resource <name> is busy (pid N)` in the log. That hold writes no `done`, takes
no attempt and records like any other held run, so it never reads as a missed
run.

The lock is released by an EXIT trap, and only while it still names this run's
pid, so a run that waited past another run's deadline cannot delete the new
holder's lock. The trap composes with the one `lib/guards.sh` installs for the
run lock, because bash keeps one EXIT trap per shell and the second trap would
otherwise silently replace the first.

## Notifications

`u_notify <title> <body>` (`lib/notify.sh`) is where a job and `check` both send
notifications, because one file keeps the channel list and its deadlines honest.
It appends `<iso>TAB<title>TAB<body>` to `$U_ALERTS` (`logs/alerts.log`) first,
then tries each configured channel with a 5-second timeout: `U_NOTIFY_CMD` as a
shell command with the title and body appended as arguments, `U_NOTIFY_WEBHOOK`
as a JSON POST, and the desktop through `osascript`. A channel that fails or
hangs is forgotten, and no notification ever fails a job — the exit codes stay a
statement about the work. `U_NOTIFY=0` silences the channels but still records
the alert line, so a quiet run keeps the same record as a noisy one.

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
