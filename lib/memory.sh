# memory.sh — the durable-memory pass as a job stage.
# Rules: ~/pi-memory/shared/memory-update-policy.md, wiki-policy.md.
# qmd needs file access outside the workspace sandbox; run jobs unsandboxed.
# Set U_MEMORY=1 to enable, U_MEMORY_FILES="notes/a.md notes/b.md" to name what changed.

U_PI_MEMORY="${U_PI_MEMORY:-$HOME/pi-memory}"

u_memory_save() {
  local f
  for f in "$@"; do
    [ -f "$f" ] || u_block "memory pass: $f does not exist"
    "$U_PI_MEMORY/qmd-mem.sh" save "$f" || return 1
  done
}

u_memory_lint() {
  python3 "$U_PI_MEMORY/lint-memory.py" --deep
}

u_memory_receipt() {
  python3 "$U_PI_MEMORY/memory-pass-check.py" --latest --cwd "$PWD"
}

# Steps 0-5 of the policy, minus the judgment in step 0: a job only calls this
# for notes it actually changed. Returns non-zero so the caller can block.
u_memory_pass() {
  if [ "${U_MEMORY:-0}" != 1 ]; then
    u_say "memory pass: skipped (U_MEMORY!=1)"
    return 0
  fi
  if [ -n "${U_MEMORY_FILES:-}" ]; then
    # shellcheck disable=SC2086
    u_memory_save $U_MEMORY_FILES || { u_say "memory pass: save failed"; return 1; }
  fi
  u_memory_lint || { u_say "memory pass: lint failed"; return 1; }
  u_memory_receipt || { u_say "memory pass: receipt failed"; return 1; }
  u_say "memory pass: ok"
  return 0
}
