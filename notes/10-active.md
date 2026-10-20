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

## Open questions

- Should `job check` be the only stale detector, or should the dashboard compute
  it too? Both read the same files, so this is a display choice.
