# heal.sh — run one repair agent, headless, with a wall-clock timeout.
#
# The manager writes the prompt and reads `$LIGHTSOUT_HEAL_OUT`; this file only
# chooses the runner and enforces the timeout. It is sourced by the manager,
# not by a job, so it does not install traps or take the job lock.

# u_heal_agent <prompt> — run the configured runner on a prompt file.
u_heal_agent() {
  local prompt="${1:-}"
  local runner="${U_HEAL_RUNNER:-${U_RUNNER:-dsh}}"
  local timeout="${U_AGENT_TIMEOUT:-600}"
  local marker="${LIGHTSOUT_HEAL_TIMEOUT:-}"
  local pid watchdog rc=0

  [ -n "$prompt" ] || { echo "heal: a prompt file is required" >&2; return 64; }
  [ -f "$prompt" ] || { echo "heal: no prompt at $prompt" >&2; return 66; }
  case "$timeout" in
    ''|*[!0-9]*) echo "heal: U_AGENT_TIMEOUT must be a whole number" >&2; return 64 ;;
  esac
  [ "$timeout" -gt 0 ] || { echo "heal: U_AGENT_TIMEOUT must be positive" >&2; return 64; }

  run_heal_runner() {
    case "$runner" in
      fake)
        exec "$REPO/tests/fake-agent.sh" "$prompt"
        ;;
      dsh)
        if [ -n "${U_AGENT_PATCH:-}" ]; then
          exec dsh --profile headless --patch "$U_AGENT_PATCH" < "$prompt"
        else
          exec dsh --profile headless < "$prompt"
        fi
        ;;
      claude)
        if [ -n "${U_AGENT_MODEL:-}" ]; then
          exec claude -p --model "$U_AGENT_MODEL" "$(cat "$prompt")"
        else
          exec claude -p "$(cat "$prompt")"
        fi
        ;;
      codex)
        if [ -n "${U_AGENT_MODEL:-}" ]; then
          exec codex exec --model "$U_AGENT_MODEL" "$(cat "$prompt")"
        else
          exec codex exec "$(cat "$prompt")"
        fi
        ;;
      *)
        echo "heal: unknown runner '$runner' (use dsh, claude, codex or fake)" >&2
        return 64
        ;;
    esac
  }

  rm -f "$marker" 2>/dev/null || true
  run_heal_runner &
  pid=$!
  (
    sleep "$timeout"
    [ -n "$marker" ] && : > "$marker"
    kill -TERM "$pid" 2>/dev/null || true
    sleep 2
    kill -KILL "$pid" 2>/dev/null || true
  ) &
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
