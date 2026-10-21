# spec: video-prune

deliverable
: Reclaimed disk from render intermediates that are no longer needed, with a
  manifest naming every path considered and every path removed.

cadence
: Daily at 23:30, before the midnight build; at most once per calendar day.
  Because the plan is written before any deletion, a manual run and the 23:30
  scheduled run take the same path and both stop at the plan unless
  `PRUNE_APPLY=1` is set.

unit
: The calendar date.

inputs
: `episodes/<NNN>/comp/*/renders/`, `outbox/<NNN>-master.mp4` and
  `pipeline/publish/queue.json` in the video checkout.

stages
: 1. inventory — read the queue, masters and render directories; write a TSV
  plan — gate: `exists` on the plan file
  2. apply — remove only the planned render directories when `PRUNE_APPLY=1`;
  write the applied manifest — gate: `exists` on the applied file
  3. verify — report the reclaimed bytes and the remaining free space

guards
: day guard, run lock, video driver build lock, two attempts, disk not guarded
  because a full disk is the reason this job runs.

publish
: None. This job deletes regenerable intermediates; it never posts, edits the
  queue, or touches a master.

memory
: None. The job writes no notes; `U_MEMORY=0` makes that explicit.

budget
: Zero. The job uses neither an agent nor a paid API; it reads local files and
  deletes only the allowlisted render directories.
