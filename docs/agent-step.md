# The agent step

An agent is a command-line program with three unusual properties: it consumes a
text file, it costs money every time, and its exit code says nothing about
whether the output is any good. A scheduler can only see the exit code, so the
step has to be wrapped.

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
python3 lib/gates.py sections --file lesson.md --require Motivation,Mechanism \
  --min-words 120 --max-words 160 || u_block "lesson failed the contract"
```

## Runner adapters

The step is the same for every runner; only the command differs.

| runner | non-interactive invocation |
|---|---|
| dsh | `dsh --profile headless --patch <patch.yml> < prompt.md` |
| claude | `claude -p "$(cat prompt.md)"` |
| codex | `codex exec "$(cat prompt.md)"` |
| fake | `tests/fake-agent.sh prompt.md` (the test double) |

Pin the model and the reasoning effort in the patch or the flag, because an
unpinned model turns a passing pipeline into a different one next week.

## The wrapper

`u_agent <prompt-file> <marker>` (`lib/agent.sh`, sourced by `lib/guards.sh`)
is the only way a job runs a runner; every adapter above goes through it.

- It runs `U_RUNNER` (`dsh` by default, or `claude`, `codex`, `fake`) once,
  headless, on a prompt already on disk, and never answers a question.
- It writes the marker only after the runner exits `0`; a failed runner
  returns its non-zero exit to the job and writes nothing, so the job decides
  whether that is failed (`1`) or blocked (`2`).
- A call that finds the marker returns `0` without invoking the runner, so a
  rerun after a later failure re-checks the artifact instead of paying again.
- It writes nothing in plan mode and logs one `agent:` line per decision.

The marker freezes the call, never the verdict: the gate must run outside any
marker test, because a marker that skipped the gate would pass a bad artifact.
A bare marker name lands under `$STATE`; a path is used as given.

`U_AGENT_PATCH` names the dsh patch that pins the model. `U_RUNNER=fake` runs
`tests/fake-agent.sh` by its path in this repository, never a same-named
program on `PATH`, so a fake step can never reach a real model and the adapter
is testable offline.

## Cost

Log the spend per run next to the exit code. A job that quietly spends ten
dollars a night is a bug that only the ledger notices.
