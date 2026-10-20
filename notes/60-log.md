# Log

## [2026-10-20] decision | LightsOut started as a standard office for unattended jobs

Created the repository after the Everyday Laws daily pipeline needed the same
guards twice: resume-from-disk, a day guard, an attempt cap, a blocked state that
notifies, and a memory pass. The six-verb run contract, `lib/guards.sh`,
`lib/gates.py`, and the `noop` fixture job are in place; the scaffolder and the
agent adapter are next.

## [2026-10-20] decision | all fourteen gaps closed; the video job is ported

The plan at `docs/superpowers/plans/2026-10-20-close-the-gaps.md` is implemented
task by task, each by a fresh agent that had to leave `tests/run.sh` green and
commit once: blocked failures now carry a class, `diagnose` writes a read-only
cause, `retry` runs only an allowlisted repair and lets the gate decide again,
attempts and repairs are capped per stage, spend is a ledger, a run that never
happened is detected, notifications reach more than the desktop, run history and
resource locks exist, the footage approval manifest is a gate, and the memory
pass is enforced by a fake store the tests can fail on purpose. The daily video
pipeline is ported as `jobs/everyday-laws`.

Two defects were found by review after the subagents signed off. The ported job
treated the driver's own one-per-day hold as a failed gate, so a hold became a
block, and a block refuses every later run: the first unattended night would
have killed the pipeline. It also spent an attempt per held tick, reaching the
cap in half an hour. The job now asks the driver's rule before counting an
attempt and treats a driver that exits 0 without a master as held. Separately,
two concurrent `tests/run.sh` runs trampled the same state directories and
reported phantom failures, so the suite now refuses to run twice at once.
