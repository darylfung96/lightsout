# AGENTS.md — LightsOut

A standard office for jobs that run without a human. Read
`docs/run-contract.md` before writing a job; read `docs/memory-pass.md` before
letting any job write notes.

## Hard rules

- **Exit codes are the interface.** `0` done or deliberately held, `1` failed,
  `2` blocked and needs a human. A job that cannot finish must exit `2` and
  record why — a silent `0` is the failure this repository exists to prevent.
- **Gates fail closed.** An unimplemented check is `lightsout gate never`
  (`u_gate never` in a job), never a missing line. A job may not publish until
  every gate in its spec resolves.
- **State is files under `jobs/<name>/state/`.** No database, no hidden cache.
  If `bin/job status` cannot explain a job, the job is wrong.
- **Scripts before agents.** Anything checkable becomes a gate, not a sentence
  in a prompt. See `~/agent-memory/shared/deterministic-first.md`.
- **An agent step is staged, gated, and never retried.** See `docs/agent-step.md`.
- **Auto-repair edits artifacts, never the checklist.** An agent proposal is
  data: the validator refuses `job.sh`, `spec.md`, `state/` and `lib/`, the
  manager applies edits atomically with a backup, and only the job's gate may
  pass a gate. `triage.conf` is human-edited; the manager never learns a rule.
  See `docs/auto-repair.md`.
- **No secrets in the repo.** Reference a keyring or an env file outside it.

## Memory (full durable-memory suite)

This repository follows `~/agent-memory` conventions, because the memory pass and the wiki both run out of that store. Owner pages:
`~/agent-memory/shared/memory-update-policy.md` (steps 0–5),
`memory-auto-index-policy.md` (which collection owns a fact),
`wiki-policy.md` (page schema), `deterministic-first.md`.

- The wiki is `notes/`. Update the page, `00-index.md`, and `60-log.md` on every
  ingest; one fact, one home.
- **Replace, never append.** A superseded fact is edited out in the same write
  that adds its replacement. No "update:" paragraphs, no parallel copies.
- **Owner check before creating a page**, because two pages stating one rule
  make retrieval ambiguous.
- **Every job ends with the memory pass** (`lib/memory.sh`): save the notes it
  touched, `lint-memory.py --deep`, then the receipt
  `memory-pass-check.py --latest --cwd "$PWD"` must exit 0. A failed pass is a
  blocked job, not a warning.
- Durability is judgment: balances, build IDs and take numbers belong in a
  progress file, not in memory.
- Search: `~/agent-memory/qmd-mem.sh search "<terms>" -c lightsout` (needs file
  access outside the workspace sandbox, so run it unsandboxed).

<!-- wiki: decisions=05-standing-decisions.md active=10-active.md -->
