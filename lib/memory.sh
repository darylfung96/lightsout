# memory.sh — the durable-memory pass as a job stage.
# Rules: ~/agent-memory/shared/memory-update-policy.md, wiki-policy.md.
# qmd needs file access outside the workspace sandbox; run jobs unsandboxed.
# Set U_MEMORY=1 to enable, U_MEMORY_FILES="notes/a.md notes/b.md" to name what changed.
# U_PI_MEMORY names the store; tests point it at tests/fake-pi-memory.

export U_PI_MEMORY="${U_PI_MEMORY:-$HOME/agent-memory}"

# u_memory_check — the whole pass as one gate: save each named note, lint the
# store, then prove the receipt, and fail when any step does (lightsout gate
# memory). A plain status, not a block, because the job decides how to report
# the stop; U_MEMORY_FILES is the space-separated list the job already sets.
u_memory_check() {
  local files="${U_MEMORY_FILES:-}"
  "${LIGHTSOUT_BIN:-$REPO/bin/lightsout}" gate memory --files "${files// /,}"
}

# Steps 0-5 of the policy, minus the judgment in step 0: a job only calls this
# for notes it actually changed. Returns non-zero so the caller can block.
u_memory_pass() {
  if [ "${U_MEMORY:-0}" != 1 ]; then
    u_say "memory pass: skipped (U_MEMORY!=1)"
    return 0
  fi
  u_memory_check || { u_say "memory pass: failed"; return 1; }
  u_say "memory pass: ok"
  return 0
}
