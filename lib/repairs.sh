# repairs.sh — the allowlist `bin/job retry` may run.
#
# A repair changes the world; it never changes the verdict. The gate that
# rejected the artifact is the only thing that may pass it, so a repair here
# does the smallest environmental fix it can and then returns; `bin/job retry`
# re-runs the job from the top and the same gate decides again.
#
#   * a repair is registered by defining `repair_<name>()` in this file —
#     `bin/job retry` refuses every name that is not defined here, so no
#     arbitrary command can be reached through the verb;
#   * a repair must be idempotent: a retry may run it after a half-finished one;
#   * a repair returns 0 when it changed (or confirmed) the world and non-zero
#     when it could not;
#   * a repair must not write state/blocked, state/done, or call a gate. It is
#     not allowed to decide anything.
#
# Environment: REPO JOB JOB_DIR STATE OUT LOG UNIT, set by bin/job before the
# call. Plain shell only: this file is sourced without guards.

# repair_touch-missing — the noop fixture's repair. It makes sure the artifact
# the fixture writes exists, so the next run's `exists` gate has something to
# judge; it creates an empty file only when absent and never writes content.
# `NOOP_REPAIR` selects it by name (the default in bin/job retry), and a real
# job passes `--repair <name>` for its own failure mode.
repair_touch-missing() {
  [ -n "${OUT:-}" ] || return 1
  mkdir -p "$OUT" || return 1
  if [ -e "$OUT/hello.txt" ]; then
    printf 'already present: %s\n' "$OUT/hello.txt"
  else
    : > "$OUT/hello.txt" || return 1
    printf 'created %s\n' "$OUT/hello.txt"
  fi
}
