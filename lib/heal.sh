# heal.sh — run one repair agent, headless, with a wall-clock timeout.
#
# The manager writes the prompt and reads `$LIGHTSOUT_HEAL_OUT`; this file only
# chooses the runner and enforces the timeout. It is sourced by the manager,
# not by a job, so it does not install traps or take the job lock.

# The watchdog is shared with the lesson agent: one implementation, one
# timeout rule (lib/agent.sh).
. "$REPO/lib/agent.sh"

# u_heal_agent <prompt> — run the configured runner on a prompt file.
u_heal_agent() {
  local prompt="${1:-}"
  local runner="${U_HEAL_RUNNER:-${U_RUNNER:-dsh}}"
  local timeout="${U_AGENT_TIMEOUT:-600}"
  local marker="${LIGHTSOUT_HEAL_TIMEOUT:-}"

  [ -n "$prompt" ] || { echo "heal: a prompt file is required" >&2; return 64; }
  [ -f "$prompt" ] || { echo "heal: no prompt at $prompt" >&2; return 66; }

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

  u_bounded "$timeout" "$marker" run_heal_runner
}
