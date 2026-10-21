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
| 4 prove it landed | `memory-update.py` receipt for each edited page |
| 5 consolidate | `lint-memory.py --deep`, then the pass receipt |

## How a job calls it

The last stage of the job:

```sh
u_say "stage N: memory pass"
U_MEMORY=1 U_MEMORY_FILES="notes/60-log.md notes/decisions.md" \
  u_memory_pass || u_block "memory pass failed"
```

`U_MEMORY` defaults to `0` so tests and dry runs never touch the real store.

## Why the failure is a block and not a warning

A job that publishes a video and then silently leaves memory stale has produced
two artifacts, one of them wrong. The notes are the record a future session
reads, so a failed pass means the run is not finished — it is blocked.

## Practical notes

- `qmd` keeps its database outside the workspace, so a sandboxed run fails with
  `SQLITE_CANTOPEN`. Memory passes run unsandboxed.
- The pass receipt is `python3 ~/agent-memory/memory-pass-check.py --latest --cwd
  "$PWD"`; it must exit 0, and `--files` covers pages written by a script.
