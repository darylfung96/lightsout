# The agent step

Because an agent consumes a text file, costs money every time, and its exit
code says nothing about whether the output is any good, a scheduler that sees
only the exit code has to wrap the step before it can trust the result.

## The recipe

1. **Render a prompt file.** A standing template with slots, committed to the
   repository — not a sentence improvised at runtime.
2. **Run headless with permissions pre-decided.** No human is at the keyboard at
   03:00, so every question must already be answered.
3. **Check the artifact with a gate.** This is the only step that decides
   pass/fail.
4. **Freeze the result.** Write a marker after the runner exits `0`, so a retry
   skips the agent entirely — but never skips the gate.
5. **Never retry the agent.** A second run costs again and produces different
   text.
6. **Bound any repair loop.** One attempt to fix a failed gate, then blocked.

## Shape

```sh
prompt="$STATE/prompt-$(date +%F).md"
render_prompt > "$prompt"
# u_agent skips the runner when the marker exists, so a retry re-checks the
# artifact instead of paying again; the gate runs every time and is the only
# verdict.
u_agent "$prompt" "$prompt.done" || u_die "agent failed"
bin/lightsout gate sections --file lesson.md --require Motivation,Mechanism \
  --min-words 120 --max-words 160 || u_block "lesson failed the contract"
```

## Runner adapters

The step is the same for every runner; only the command differs.

| runner | non-interactive invocation |
|---|---|
| dsh | `dsh --profile headless --patch <patch.yml> < prompt.md` |
| claude | `claude -p [--model <id>] "$(cat prompt.md)"` |
| codex | `codex exec [--model <id>] "$(cat prompt.md)"` |
| fake | `tests/fake-agent.sh prompt.md` (the test double) |

Pin the model in the job's agent profile, because an unpinned model turns a
passing pipeline into a different one next week. For `dsh` the profile points at
the patch that carries the pin; the `claude` and `codex` adapters pass the
profile's `model` as `--model`.

## Agent profiles

A job records its runner and model in `jobs/<name>/agent.conf`; a workspace can
lend named profiles from `agents.conf`. The manager resolves the file, applies
environment overrides, and exports one result to `u_agent` and `u_heal_agent`,
so the lesson call and the repair that follows it use the same pin.

```ini
# agents.conf
[daily]
runner = dsh
patch = ~/programming/video-automation/automation/headless.patch.yml
```

```ini
# jobs/everyday-laws/agent.conf
profile = daily
```

| key | meaning |
|---|---|
| `runner` | `dsh`, `claude`, `codex`, or `fake` |
| `model` | the adapter's model id; for `dsh` it is checked against the patch |
| `provider` | `dsh` only; the provider named by the patch |
| `patch` | the `dsh` overlay that pins provider, model, and effort |
| `effort` | `dsh` only; checked against the patch |
| `profile` | a named profile from `agents.conf` |

Resolution runs built-in defaults, then the named profile, then the job file's
own keys, then the environment. `U_RUNNER` and `U_HEAL_RUNNER` select the
adapter; `U_AGENT_PATCH`, `U_AGENT_MODEL`, and `U_AGENT_EFFORT` override the
file for one invocation. A `dsh` profile that names a model without a patch is a
config error, because `dsh` has no `--model` flag. The manager reads the
patch's `agent-default-model` block and refuses a profile that contradicts it,
so the profile and the patch cannot drift. Paths may be absolute, relative to
the workspace, or begin with `~`, `$HOME/`, or `$VIDEO_REPO/`.

`lightsout plan` prints the resolved profile, because an operator needs to see
the pin before spending a call. `triage` carries the same object in text and
`--json`, `doctor` checks the runner and patch, and `state/heal/history.tsv`
records it in the last column. The file is human-edited; the manager never
writes or learns it.

## The wrapper

`u_agent <prompt-file> <marker>` (`lib/agent.sh`, sourced by `lib/guards.sh`)
is the only way a job runs a runner; every adapter above goes through it.

- It runs the resolved runner (`dsh` by default, or `claude`, `codex`, `fake`)
  once, headless, on a prompt already on disk, and never answers a question.
  The runner and model come from the job's agent profile.
- It writes the marker only after the runner exits `0`; a failed runner
  returns its non-zero exit to the job and writes nothing, so the job decides
  whether that is failed (`1`) or blocked (`2`).
- A call that finds the marker returns `0` without invoking the runner, so a
  rerun after a later failure re-checks the artifact instead of paying again.
- It writes nothing in plan mode and logs one `agent:` line per decision.

The marker freezes the call, never the verdict: the gate must run outside any
marker test, because a marker that skipped the gate would pass a bad artifact.
A bare marker name lands under `$STATE`; a path is used as given.

`U_AGENT_PATCH` names the dsh patch that pins the model, and `U_AGENT_MODEL`
is passed to `claude -p --model` or `codex exec --model`. `U_RUNNER=fake` runs
`tests/fake-agent.sh` by its path in this repository, never a same-named
program on `PATH`, so a fake step can never reach a real model and the adapter
is testable offline.

## Cost

Log the spend per run next to the exit code, because a job that quietly spends
ten dollars a night is a bug that only the ledger notices.
