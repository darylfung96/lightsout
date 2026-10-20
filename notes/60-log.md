# Log

## [2026-10-20] decision | LightsOut started as a standard office for unattended jobs

Created the repository after the Everyday Laws daily pipeline needed the same
guards twice: resume-from-disk, a day guard, an attempt cap, a blocked state that
notifies, and a memory pass. The six-verb run contract, `lib/guards.sh`,
`lib/gates.py`, and the `noop` fixture job are in place; the scaffolder and the
agent adapter are next.
