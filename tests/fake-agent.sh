#!/usr/bin/env bash
# fake-agent.sh — the stand-in behind U_RUNNER=fake. It never calls a model.
#
# lib/agent.sh invokes it with the prompt file as the only argument, which is
# the shape every real runner gets (a prompt file, no interactive questions).
# It records one line per invocation in FAKE_AGENT_LOG, so a test can prove an
# agent step ran once and a rerun did not pay for it again, and writes the
# artifact a job's gate checks next to the prompt as <prompt>.out.
#
#   FAKE_AGENT_LOG  append one "<iso>TAB<prompt>TAB<profile>TAB<runner>TAB<model>"
#                   line per call
#   FAKE_AGENT_RC   exit code (default 0); a non-zero run writes no artifact
#   FAKE_AGENT_OUT  artifact path (default <prompt>.out)
set -euo pipefail

prompt="${1:-}"
if [ -z "$prompt" ]; then
  echo "fake-agent: a prompt file is required" >&2
  exit 64
fi
if [ -n "${FAKE_AGENT_LOG:-}" ]; then
  printf '%s\t%s\t%s\t%s\t%s\n' \
    "$(date -Iseconds)" "$prompt" "${U_AGENT_PROFILE:-}" \
    "${U_RUNNER:-}" "${U_AGENT_MODEL:-}" >> "$FAKE_AGENT_LOG"
fi
rc="${FAKE_AGENT_RC:-0}"
case "$rc" in
  ''|*[!0-9]*) echo "fake-agent: FAKE_AGENT_RC must be a whole number" >&2; exit 64 ;;
esac
if [ "$rc" -ne 0 ]; then
  echo "fake-agent: runner failed (FAKE_AGENT_RC=$rc)" >&2
  exit "$rc"
fi
[ -f "$prompt" ] || { echo "fake-agent: no prompt at $prompt" >&2; exit 66; }
if [ -n "${FAKE_AGENT_SLEEP:-}" ]; then
  sleep "$FAKE_AGENT_SLEEP"
fi
if [ -n "${FAKE_AGENT_AFTER:-}" ]; then
  printf 'after\n' > "$FAKE_AGENT_AFTER"
fi
if [ -n "${FAKE_AGENT_AFTER:-}" ]; then
  printf 'after\n' > "$FAKE_AGENT_AFTER"
fi
if [ -n "${FAKE_AGENT_PROPOSAL_SEQ:-}" ]; then
  [ -n "${FAKE_AGENT_STATE:-}" ] || {
    echo "fake-agent: FAKE_AGENT_PROPOSAL_SEQ needs FAKE_AGENT_STATE" >&2
    exit 64
  }
  [ -n "${LIGHTSOUT_HEAL_OUT:-}" ] || {
    echo "fake-agent: FAKE_AGENT_PROPOSAL_SEQ needs LIGHTSOUT_HEAL_OUT" >&2
    exit 64
  }
  index=0
  [ -f "$FAKE_AGENT_STATE" ] && index=$(cat "$FAKE_AGENT_STATE")
  index=$((index + 1))
  chosen=$(printf '%s' "$FAKE_AGENT_PROPOSAL_SEQ" | cut -d: -f"$index")
  cp "$chosen" "$LIGHTSOUT_HEAL_OUT"
  printf '%s\n' "$index" > "$FAKE_AGENT_STATE"
  printf 'fake agent: proposal %s (sequence %s)\n' "$LIGHTSOUT_HEAL_OUT" "$index"
elif [ -n "${FAKE_AGENT_PROPOSAL:-}" ]; then
  [ -n "${LIGHTSOUT_HEAL_OUT:-}" ] || {
    echo "fake-agent: FAKE_AGENT_PROPOSAL needs LIGHTSOUT_HEAL_OUT" >&2
    exit 64
  }
  cp "$FAKE_AGENT_PROPOSAL" "$LIGHTSOUT_HEAL_OUT"
  printf 'fake agent: proposal %s\n' "$LIGHTSOUT_HEAL_OUT"
fi
if [ -n "${FAKE_AGENT_TOUCH:-}" ]; then
  printf 'tampered\n' >> "$FAKE_AGENT_TOUCH"
fi
# A prompt-matched copy lets a test feed a stage-specific artifact (for example
# the topic proposal) without teaching the runner what that stage is.
if [ -n "${FAKE_AGENT_COPY_FROM:-}" ]; then
  [ -n "${FAKE_AGENT_COPY_TO:-}" ] || {
    echo "fake-agent: FAKE_AGENT_COPY_FROM needs FAKE_AGENT_COPY_TO" >&2
    exit 64
  }
  if [ -z "${FAKE_AGENT_COPY_MATCH:-}" ] ||
     printf '%s' "$prompt" | grep -Fq -- "$FAKE_AGENT_COPY_MATCH"; then
    cp "$FAKE_AGENT_COPY_FROM" "$FAKE_AGENT_COPY_TO"
  fi
fi
out="${FAKE_AGENT_OUT:-$prompt.out}"
printf 'fake agent: %s\n' "${prompt##*/}" > "$out"
printf 'fake agent: wrote %s\n' "$out"
