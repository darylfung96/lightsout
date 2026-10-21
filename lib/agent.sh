# agent.sh — one wrapper for every agent runner: prompt file in, marker out.
# Sourced by lib/guards.sh, so every job has u_agent beside the other guards.
# Contract: docs/agent-step.md (why), docs/run-contract.md (state layout).
#
# An agent step is staged, gated and never retried: the prompt is a file, the
# runner is invoked headless exactly once, and a gate on the artifact is the
# only thing that decides pass/fail (docs/agent-step.md). u_agent owns the
# headless call and the freeze:
#
#   * it writes the marker only after the runner exits 0, so a failed runner
#     leaves nothing behind and the job decides how to report it;
#   * a call that finds the marker returns 0 without invoking the runner,
#     because a second model call costs again and produces different text;
#   * it returns the runner's non-zero exit to the job, so a transient CLI
#     failure can be failed and a produced-but-bad artifact can be blocked;
#   * it never retries, never answers a question and does nothing in plan mode.
#
# The marker freezes the call, never the verdict. The job gates the artifact on
# every attempt, outside any "marker exists" test, so a second run re-checks the
# same artifact instead of passing it because the marker is there.
#
# U_RUNNER picks the adapter: dsh (default) | claude | codex | fake. The
# invocations are the ones docs/agent-step.md documents, with the model pinned
# by the runner's own configuration (U_AGENT_PATCH names the dsh patch) because
# an unpinned model turns a passing pipeline into a different one next week.
# A runner that is not installed is a code error, not a job result: it dies with
# exit 1 rather than pretending a stage succeeded.
#
# U_RUNNER=fake runs tests/fake-agent.sh by its path in this repository, never
# a same-named program off PATH, so a fake step can never reach a real model.

# u_agent_marker <marker> — a bare name is a file under $STATE; a path is used
# as given, so a job may freeze its marker beside the prompt.
u_agent_marker() {
  case "${1:-}" in
    */*) printf '%s' "$1" ;;
    *)   printf '%s/%s' "${STATE:?u_agent: STATE is not set}" "$1" ;;
  esac
}

# u_bounded <seconds> <timeout-marker> <command...> — run a command with a
# wall-clock deadline. The caller's command is exec'd, so the watchdog kills
# the runner itself rather than a wrapper shell. A timeout returns 124; any
# other result is the child's own exit. This is shared by the lesson agent and
# the repair agent, because a hung runner must never hold the run lock forever.
u_bounded() {
  local timeout="${1:-}" marker="${2:-}" pid watchdog rc=0
  shift 2 2>/dev/null || true
  case "$timeout" in
    ''|*[!0-9]*) echo "agent: U_AGENT_TIMEOUT must be a whole number" >&2; return 64 ;;
  esac
  [ "$timeout" -gt 0 ] || { echo "agent: U_AGENT_TIMEOUT must be positive" >&2; return 64; }
  [ "$#" -gt 0 ] || { echo "agent: u_bounded needs a command" >&2; return 64; }
  rm -f "$marker" 2>/dev/null || true
  "$@" &
  pid=$!
  (
    # The watchdog must not inherit the job's stdout/stderr: a caller that
    # captures the job through a pipe would otherwise wait for this sleep to
    # finish even after the runner is gone.
    sleep "$timeout"
    [ -n "$marker" ] && : > "$marker"
    kill -TERM "$pid" 2>/dev/null || true
    sleep 2
    kill -KILL "$pid" 2>/dev/null || true
  ) </dev/null >/dev/null 2>&1 &
  watchdog=$!
  wait "$pid" || rc=$?
  kill "$watchdog" 2>/dev/null || true
  wait "$watchdog" 2>/dev/null || true
  if [ -n "$marker" ] && [ -f "$marker" ]; then
    rm -f "$marker"
    return 124
  fi
  return "$rc"
}

# u_agent <prompt-file> <marker> — run the runner once on a prompt already on
# disk, then freeze success. Returns non-zero when the runner fails or exceeds
# U_AGENT_TIMEOUT (600s by default), and writes no marker then.
u_agent() {
  local prompt="${1:-}" marker_arg="${2:-}" runner="${U_RUNNER:-dsh}" marker rc=0 text
  local timeout="${U_AGENT_TIMEOUT:-600}" timeout_marker
  [ -n "$prompt" ] || u_die "u_agent: a prompt file is required"
  [ -n "$marker_arg" ] || u_die "u_agent: a marker is required"
  marker=$(u_agent_marker "$marker_arg")
  if [ -f "$marker" ]; then
    u_say "agent: $prompt already frozen ($marker); runner not invoked"
    return 0
  fi
  if [ "${DRY:-0}" = 1 ]; then
    u_say "plan: would run agent ($runner) on $prompt"
    return 0
  fi
  [ -s "$prompt" ] || u_die "u_agent: no prompt at $prompt"
  timeout_marker="${STATE:-${TMPDIR:-/tmp}}/.agent-timeout.$$"

  run_agent() {
    case "$runner" in
      fake)
        local fake="${REPO:?u_agent: REPO is not set}/tests/fake-agent.sh"
        [ -x "$fake" ] || u_die "u_agent: the fake runner is missing: $fake"
        u_say "agent: runner fake on $prompt"
        exec "$fake" "$prompt"
        ;;
      dsh)
        command -v dsh >/dev/null 2>&1 || u_die "u_agent: runner dsh is not installed"
        u_say "agent: runner dsh (headless) on $prompt${U_AGENT_PROFILE:+ [$U_AGENT_PROFILE]}"
        if [ -n "${U_AGENT_PATCH:-}" ]; then
          exec dsh --profile headless --patch "$U_AGENT_PATCH" < "$prompt"
        else
          exec dsh --profile headless < "$prompt"
        fi
        ;;
      claude)
        command -v claude >/dev/null 2>&1 || u_die "u_agent: runner claude is not installed"
        text=$(cat "$prompt")
        u_say "agent: runner claude (-p) on $prompt${U_AGENT_MODEL:+ [model $U_AGENT_MODEL]}"
        if [ -n "${U_AGENT_MODEL:-}" ]; then
          exec claude -p --model "$U_AGENT_MODEL" "$text"
        else
          exec claude -p "$text"
        fi
        ;;
      codex)
        command -v codex >/dev/null 2>&1 || u_die "u_agent: runner codex is not installed"
        text=$(cat "$prompt")
        u_say "agent: runner codex (exec) on $prompt${U_AGENT_MODEL:+ [model $U_AGENT_MODEL]}"
        if [ -n "${U_AGENT_MODEL:-}" ]; then
          exec codex exec --model "$U_AGENT_MODEL" "$text"
        else
          exec codex exec "$text"
        fi
        ;;
      *)
        u_die "u_agent: unknown runner '$runner' (use dsh, claude, codex or fake)"
        ;;
    esac
  }

  u_bounded "$timeout" "$timeout_marker" run_agent || rc=$?
  if [ "$rc" -eq 124 ]; then
    u_say "agent: runner $runner timed out after ${timeout}s; no marker written"
    return 124
  fi
  if [ "$rc" -ne 0 ]; then
    u_say "agent: runner $runner failed (exit $rc); no marker written"
    return "$rc"
  fi
  mkdir -p "$(dirname "$marker")"
  : > "$marker"
  u_say "agent: runner $runner passed; frozen $marker"
  return 0
}
