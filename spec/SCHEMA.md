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
