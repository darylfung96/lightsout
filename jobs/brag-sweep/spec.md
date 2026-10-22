# brag-sweep — spec

The daily job sweep for the Brag resume repo, run once a day with the standing
filter unchanged: Canada-eligible, remote-only, ML/AI engineer titles, posted
within 21 days, title match required.

## What a run does

1. `jobtool doctor` — the Adzuna credentials and the source health. The engine
   retries a failed live check three times with a short backoff, because a 503
   is usually a flap; a failure after those attempts is fatal, because a sweep
   with no sources is worse than no sweep.
2. `jobtool digest -q` — incremental, so only postings never seen before are
   reported. The same-day digest is archived first, because `digest` rewrites
   that file.
3. A headless agent pass annotates each new posting: verdict, remote verdict,
   strengths, gaps. It may not run `digest` again — that would rewrite the day's
   record with "no new postings".
4. `jobtool render --top 25` rebuilds `jobtool/output/index.html`, then headless
   Chrome screenshots it to `jobtool/output/dashboard.png`.

## What "done" means

- `jobtool/output/digests/<today>.md` exists and reports today's sweep.
- `jobtool/output/index.html` exists (rebuilt when there were new postings).
- No posting is ever marked applied by this job. Applied is the owner's tick.

## What it is allowed to skip

- Zero new postings: the archived digest is restored, the page is left alone,
  and the run still counts as done. An empty morning is a result.
- A failed agent pass: the digest and the page are still fresh, so the run
  continues and logs the failure. The annotation is a refinement, not the point.

## Not this job's business

- Tailoring resumes or writing plans. That stays a session with a human in it.
- Widening the filter. A quiet week is not a reason to loosen a gate.
- Deleting or pruning `applied.json`.
