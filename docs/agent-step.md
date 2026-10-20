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
4. **Freeze the result.** Write a marker, so a retry skips the agent entirely.
5. **Never retry the agent.** A second run costs again and produces different
   text.
6. **Bound any repair loop.** One attempt to fix a failed gate, then blocked.

## Shape

```sh
prompt="$STATE/prompt-$(date +%F).md"
render_prompt > "$prompt"
if [ ! -f "$STATE/lesson.ok" ]; then
  run_agent "$prompt" || u_die "agent failed"
  python3 lib/gates.py sections --file lesson.md --require Motivation,Mechanism \
    --min-words 120 --max-words 160 || u_block "lesson failed the contract"
  : > "$STATE/lesson.ok"
fi
```

## Runner adapters

The step is the same for every runner; only the command differs.

| runner | non-interactive invocation |
|---|---|
| dsh | `dsh --profile headless --patch <patch.yml> < prompt.md` |
| claude | `claude -p "$(cat prompt.md)"` |
| codex | `codex exec "$(cat prompt.md)"` |

Pin the model and the reasoning effort in the patch or the flag, because an
unpinned model turns a passing pipeline into a different one next week.

## Cost

Log the spend per run next to the exit code. A job that quietly spends ten
dollars a night is a bug that only the ledger notices.
