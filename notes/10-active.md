# Active

What is in flight, so the next session knows where to start. Replace this page;
do not append to it.

## Now

All fourteen tasks of `docs/superpowers/plans/2026-10-20-close-the-gaps.md` are
done. The video pipeline is ported as `jobs/everyday-laws`, and nothing schedules
it yet: the launchd job still runs `video-automation/automation/dsh-daily.sh`.

Next, in order:

1. One supervised run: `bin/job run everyday-laws` by hand, comparing the master
   to the previous night's before trusting it overnight.
2. Only then point the launchd plist at `bin/job run everyday-laws` and retire
   the old launcher.
3. Decide whether `bin/job check` (the stale alarm) becomes a launchd timer, so
   a night with no run announces itself.

## Rust port

Decided 2026-10-20: port the *manager* to Rust, not the jobs. `bin/job`,
`lib/gates.py` and `lib/report.py` become one binary `lightsout` with the same
verbs and exit codes. Per-project `jobs/<name>/job.sh` stay shell, because they
are the project-specific part.

Why: a single static binary with no `python3` and no bash-version drift (the
plan had to forbid associative arrays and `${var,,}`), typed parsing of the
state files whose `blocked` line has already grown from two fields to four, and
`cargo test` in place of a 1,400-line shell suite that cannot run twice at once.

Not why: speed. Measured 2026-10-20, a gate call is 80 ms and the dashboard 47 ms
against a sixty-minute build, so the language changes at most two seconds a day.

Order, so the shell version stays the reference until the port matches it:

1. `status` and `report` — read-only, no state writes, easiest to compare.
2. `gates` — the seven gates and their exit codes.
3. `plan`, `run`, `retry`, `unblock` — the guards and the repair allowlist.
4. Port `tests/run.sh` to `cargo test` check by check; the suite is the spec.

Switch over only when the Rust binary passes every check the shell suite passes.

## Open questions

- Should `job check` be the only stale detector, or should the dashboard compute
  it too? Both read the same files, so this is a display choice.
