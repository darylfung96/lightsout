# spec: everyday-laws-video

deliverable
: One 60-second vertical video, with narration, music bed, sound effects, and a
  caption, published to Instagram.

cadence
: Daily. At most one build per calendar day; hold if a master exists from today
  or is younger than six hours.

unit
: The episode number (`027`, `028`, ...), taken from `next episode:` in the
  build driver's preflight.

inputs
: Topic queue `pipeline/topic-queue.txt` (stage 0 tops it up when the next
  episode has no line); footage from Pixabay video-first, stills as fallback;
  voice from VoiceStudio; beds from `pipeline/bed-pool.txt`.

stages
: 0. refill topics — agent, prompt `automation/topic-task.md`, only when the
     next episode has no queue line — gate: `pipeline/topic_check.py`
  1. write lesson — agent, prompt `automation/lesson-task.md` — gate: `sections`
  2. acquire footage — 15 shots — gate: `exists` per shot (asset-relevance gate
     not written yet; this is the known hole)
  3. synthesize voice — gate: `duration --min 55 --max 62`
  4. describe captions — gate: `sections`
  5. assemble composition — gate: `resolution --w 1080 --h 1920`
  6. render — gate: `duration --min 59 --max 61`
  7. master — gate: `loudness --target -14 --tol 1`
  8. publish — gate: `permalink`

guards
: day guard on the master's date; 2 attempts; disk 2000 MB; lock across ticks.

publish
: Instagram, existing session profile. The job records a post as done only
  after `permalink` fetches the shared post, so a share that cannot be fetched
  leaves the run unfinished.

memory
: `notes/60-log.md` on every build; the standing-decisions page when a rule
  changes.

budget
: One render and one publish per day; two agent calls per day at most (a topic
  refill and the lesson).
