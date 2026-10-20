# Decisions

## D1 — State is files, not a database

A job's state lives under `jobs/<name>/state/`: a lock, attempts, done markers,
one blocked line, one last-run line. The dashboard rebuilds everything from
those files. A database would let the record drift from the artifacts, and the
artifacts are what a viewer sees.

## D2 — Three exit codes, and blocked is not failed

`0` done or held, `1` failed, `2` blocked. Failure is retried; blocked notifies
once and then persists quietly. Collapsing the two is how a pipeline retries
every ten minutes against a problem only a human can fix.

## D3 — Gates fail closed

An unimplemented gate is `gates.py never`, so a job cannot reach publish while a
check is missing. This is the difference between an automation that reports what
it still needs and one that is confidently wrong.

## D4 — The memory pass is a stage, not a habit

Every job that writes notes ends with it, and its failure blocks the run. An
unattended agent obeys the same replace-never-append rules a session does,
because the next session reads the same store.

## D5 — The spec is written before the code

Every stage names a gate with numbers. A job that cannot state its unit cannot
know what it already finished.
