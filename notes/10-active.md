# Active

What is in flight, so the next session knows where to start. Replace the list;
do not append to it.

## Now

- **Scaffolder** (`bin/scaffold`): spec → `jobs/<name>/` with every gate implied
  by the spec emitted as `gates.py never`, so the job cannot run until the
  checks exist.
- **Agent adapter** (`lib/agent.sh`): one wrapper for `dsh headless`, `claude -p`,
  and `codex exec`, with the prompt file, the gate, and the stage marker in one
  place instead of copied into each job.
- **Port the Everyday Laws job** from `video-automation/automation/dsh-daily.sh`
  to a `jobs/` consumer, to test whether the contract survives contact with a
  real, expensive pipeline.

## Known gaps

- No footage-relevance gate exists anywhere yet; the video job's asset stage
  ships whatever the API returns.
- The dashboard shows state but not spend; the agent-step ledger is not wired.
