# The memory pass

An unattended agent writing notes is still bound by the durable-memory rules:
`~/agent-memory/shared/memory-update-policy.md`, `wiki-policy.md`,
`memory-auto-index-policy.md`.

The policy's steps 0–5 appear in a job like this:

| policy step | where it lives in a job |
|---|---|
| 0 durability gate | judgment: the job names in its spec which notes it touches |
| 1 owner check | the job edits the page that already owns the rule, never a new one |
| 2 replace in place | one edit that removes the old wording and adds the new |
| 3 re-index | `lib/memory.sh` → `qmd-mem.sh save <file>` |
| 4 prove it landed | a page hash and mtime in `state/memory-pass.json`, or an agent's `memory-update.py` proof |
| 5 consolidate | `lint-memory.py --deep`, then `scan-duplicates.py --from-index` reporting zero exact repeats |

## How a job calls it

The last stage of the job:

```sh
u_say "stage N: memory pass"
U_MEMORY=1 U_MEMORY_FILES="notes/60-log.md notes/decisions.md" \
  u_memory_pass || u_block "memory pass failed"
```

`U_MEMORY` defaults to `0` so tests and dry runs never touch the real store.
`lib/memory.sh` passes `--receipt "$STATE/memory-pass.json"` whenever the
manager set `STATE`, so a normal job run takes the shell path below.

## Two kinds of receipt

`~/agent-memory/memory-pass-check.py` reads a DSH session transcript, because
the proof it looks for is a tool call made inside that session. A shell job
leaves no such transcript, so the gate records the same evidence itself instead
of asking the session checker to invent one.

- **Shell job (`--receipt PATH`).** The gate saves each named page, runs
  `lint-memory.py --deep`, then runs `scan-duplicates.py --from-index`. Any
  failure, any missing repeat count, or any non-zero repeat count fails the
  gate. On success it writes `PATH` atomically: cwd, job name, timestamp, and
  for every page its name, absolute path, byte count, mtime and an FNV-1a
  64-bit content hash. The job blocks unless this receipt was written.
- **Agent session (no `--receipt`).** The gate runs `memory-pass-check.py
  --latest --cwd "$PWD"` against the session that did the work, so an agent
  step still proves its pass with `memory-update.py` proofs and the session
  transcript.

## Why the failure is a block and not a warning

A job that publishes a video and then silently leaves memory stale has produced
two artifacts, one of them wrong. The notes are the record a future session
reads, so a failed pass means the run is not finished — it is blocked.

## Practical notes

- `qmd` keeps its database outside the workspace, so a sandboxed run fails with
  `SQLITE_CANTOPEN`. Memory passes run unsandboxed.
- The shell receipt is audit evidence, not a security boundary: its hash is
  FNV-1a 64-bit, chosen so the manager needs no extra code or dependency. It
  catches a page that changed after the pass, not a determined forger.
- `memory-pass-check.py --files` cannot close the gap for scripts: it stamps the
  page as changed at the moment of the check, so no proof recorded earlier can
  satisfy it. Use `--receipt` for shell jobs instead.
