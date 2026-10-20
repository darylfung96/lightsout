# spec: morning-digest

A deliberately different second job: no media, no publish, and it exists to test
whether the kit fits more than one tenant.

deliverable
: One markdown digest of the last day's notes, under 400 words, with links.

cadence
: Daily at 09:00, at most once per calendar day.

unit
: The date.

inputs
: `~/pi-memory` search results for the last 24 hours; `notes/60-log.md`.

stages
: 1. collect — read the log and the search hits — gate: `exists` on raw.json
  2. summarize — agent — gate: `sections --min-words 150 --max-words 400`
  3. write — `notes/digests/<date>.md` — gate: `exists`
  4. memory pass — gate: `never` until the notes are indexed

guards
: day guard; 2 attempts; disk 200 MB; no lock needed (runs in seconds).

publish
: Local file only, plus one notification.

memory
: `notes/digests/<date>.md`, `notes/00-index.md`, `notes/60-log.md`.

budget
: One agent call per day.
