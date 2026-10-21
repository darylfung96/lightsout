# Universal mode — a job outside one workspace

A workspace owns `lib/`, `triage.conf` and the shared alert log, because every
job in it reads those files. A job does not have to live inside that workspace,
so the registry can point at a directory anywhere. Register a job directory by
name and `run`, `status`, `check`, `triage`, `heal` and `diagnose` resolve that
name from anywhere:

```sh
lightsout job add backup --command 'rsync -a ~/work /Volumes/backup/work'
lightsout run backup
lightsout schedule backup --at 02:30
```

The registry is `$LIGHTSOUT_HOME/jobs.json`, and `LIGHTSOUT_HOME` defaults to
`~/.config/lightsout`. `job add` initializes that home with the same embedded
`lib/` templates `init` writes, so a generated job can source `lib/guards.sh`
without a separate workspace.

## Resolution

A job in the current workspace wins over a registry entry of the same name:
`jobs/<name>/job.sh` is the definition the operator can see, so a name means
that job whenever they are standing in its workspace. Everywhere else the
registry supplies the job directory, the state directory and the workspace, so
the current directory does not have to be a workspace.

`list`, `check` and `status` show the union of workspace jobs and registered
jobs, and a name that exists in both appears once because the workspace
definition wins.

## `job add`

```
lightsout job add <name> [--job-dir DIR] [--workspace DIR] [--workdir DIR]
                           [--state-dir DIR] [--command CMD] [--force]
lightsout job list
lightsout job remove <name>
```

- `--command CMD` generates `$LIGHTSOUT_HOME/jobs/<name>/job.sh` from the
  command, so "anything" means a command job with no shell to write.
- `--job-dir DIR` registers an existing directory in place; it must already
  contain `job.sh`.
- `--workspace DIR` chooses the workspace whose `lib/`, `triage.conf` and
  `agents.conf` the job uses. Without it, a job under `<workspace>/jobs/`
  inherits that workspace and any other directory falls back to
  `LIGHTSOUT_HOME`.
- `--workdir DIR` is where the job runs (default: its own directory), and
  `--state-dir DIR` moves `state/` away from the job directory.
- `--force` replaces an existing name or generated `job.sh`; nothing else is
  deleted.

A name is lower-case letters, digits and dash, because it is also a launchd
label segment. `job remove` deletes the name, never the files.

## A command job

The generated `job.sh` is small on purpose:

```bash
set -euo pipefail
. "${LIGHTSOUT_LIB:-$REPO/lib}/guards.sh"
u_say "command job: backup"
rsync -a ~/work /Volumes/backup/work
u_done
```

The command's exit code is the gate: 0 writes `state/done/<date>`, and any
other code fails the run and `status` reports `failed` with that code. Gates,
memory and auto-repair stay opt-in; edit the generated file to add them, and
`spec.md` records what "done" means.

## `schedule` prints, it does not schedule

`lightsout schedule <name>` prints the cron line for the job's `state/expect`
time, or for `--at HH:MM` when the job has no schedule, so the trigger stays
outside LightsOut. `--launchd` prints a plist instead, and nothing is
installed.

cron, launchd, systemd or a DSH reminder still decides *when*, so a crashed
scheduler cannot take the run history with it and LightsOut keeps one job: the
run contract and the state files. The printed cron line runs
`<binary> run <name>` with no `--workspace`, because the registry resolves the
name. The launchd form sets `LIGHTSOUT_HOME`, `HOME` and a `PATH`, because
launchd starts with a minimal environment.

## Failure modes

- A malformed `jobs.json` fails every job-resolving verb with exit 2 instead of
  falling back to a workspace job of the same name.
- `doctor` checks each entry: the job directory must have `job.sh`, and a named
  workspace must be a LightsOut workspace with `lib/guards.sh`. Outside a
  workspace, `doctor` reports the registry alone.
- Relative paths in the registry are refused, because a relative job directory
  would resolve differently under cron and under a shell.
- An unknown key in an entry is refused, because a typo such as `work_dir`
  would otherwise silently keep the default.
