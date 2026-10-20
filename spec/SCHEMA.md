# Spec schema

One page of intent, written before code, fixed headings so a machine can read
it.

```
# spec: <name>
deliverable   what exists when a run succeeds, in one measured sentence
cadence       when it runs, and the "at most once" statement
unit          what one run produces: a date, an episode number, a batch
inputs        files, APIs, or queues it reads
stages        ordered; every stage names the gate that decides it passed
guards        day guard, attempts, disk, spend — with numbers
publish       where the artifact goes, and the gate that proves it landed
memory        which notes the job writes, so the pass knows what to save
budget        money and minutes per run, and per day
```

## Rules

1. **Every stage names a gate.** A stage without a gate is a stage nobody can
   trust. If the gate does not exist yet, it is `gates.py never` and the job may
   not be enabled.
2. **Numbers, not adjectives.** "About a minute" is unverifiable; `duration
   --min 55 --max 65` is a gate.
3. **The unit is explicit.** A job that cannot name its unit cannot know what it
   already finished.
4. **Publish is separate from build.** A build that succeeds and a publish that
   lands are two facts, recorded separately.
5. **The spec is the contract the dashboard reads.** If the running job and the
   spec disagree, the spec is wrong and gets edited that day.

## What `bin/scaffold` enforces

`bin/scaffold <spec.md>` reads the page literally and writes
`jobs/<name>/job.sh` plus a copy at `jobs/<name>/spec.md`. It exits `64` and
writes nothing at all when:

- there is no `# spec: <name>` line, or the name is not lowercase letters,
  digits and dashes (the name becomes a directory, so a path-like name is
  refused);
- any of the nine headings is missing: `deliverable`, `cadence`, `unit`,
  `inputs`, `stages`, `guards`, `publish`, `memory`, `budget`;
- the `stages` block has no numbered stage, or a numbered stage's own lines do
  not contain `gate:`;
- `jobs/<name>` already exists — an existing job is never overwritten, even when
  the spec is identical.

A heading is a line that is one of the nine words, optionally after markdown
`#` characters and whitespace. In the `stages` block a stage starts at a line
whose first token is `<n>.` or `<n>)` — optionally after the definition-list
colon `:` — and its continuation lines belong to it until the next numbered
line. The words before the first ` — ` or ` - ` are the stage label, and
`gate:` ends it too.

Every emitted stage gate is `u_gate never`, whatever the spec's `gate:` text
says: a generated gate would check what someone imagined, and `never` is the
only honest gate nobody has written yet (`docs/design.md`). `bin/job plan
<name>` then prints every stage and its unimplemented gate, and a run blocks at
the first one. The generated guards come from the `guards` block: the day guard
and attempt cap are always emitted, the run lock only when the block asks for a
lock (not `no lock`), and the disk guard takes the number after `disk`, falling
back to `u_guard_disk` (2000 MB). A guard the block does not mention is not
invented.
