# notify.sh — where a notification goes, and how long it may take to get there.
# Sourced by lib/guards.sh, so jobs and `bin/job check` share one channel list.
# Contract: docs/run-contract.md
#
# u_notify <title> <body> is the whole interface:
#   1. the alert is recorded in $U_ALERTS before any channel is tried, so the
#      file is the record even when every delivery fails;
#   2. each configured channel is then tried with a 5-second timeout — the
#      command in $U_NOTIFY_CMD (title and body appended as arguments), a JSON
#      POST to $U_NOTIFY_WEBHOOK, and the desktop through osascript;
#   3. a channel that fails or hangs is forgotten. A notification is never a
#      reason for a job to fail, so nothing in this file returns non-zero.
# U_NOTIFY=0 silences the channels; the alert line is still written, because a
# job's own record is not a delivery.

# _u_notify_say <message> — log when the caller supplied the logging function.
# The check keeps notify.sh readable on its own.
_u_notify_say() {
  if [ -n "${LOG:-}" ] && command -v u_say >/dev/null 2>&1; then
    u_say "$1"
  fi
}

# _u_notify_timeout <seconds> <command...> — run one channel with a deadline.
# macOS has no timeout(1), so perl's alarm does it; if perl is missing, the
# command runs in the background and a watchdog kills it. Either way the helper
# returns 0: a channel's exit code is never a verdict.
_u_notify_timeout() {
  local secs="$1" pid wd
  shift
  [ "$#" -gt 0 ] || return 0
  if command -v perl >/dev/null 2>&1; then
    perl -e 'alarm shift; exec @ARGV' "$secs" "$@" </dev/null >/dev/null 2>&1 || true
    return 0
  fi
  "$@" </dev/null >/dev/null 2>&1 &
  pid=$!
  ( sleep "$secs"; kill -TERM "$pid" 2>/dev/null ) </dev/null >/dev/null 2>&1 &
  wd=$!
  wait "$pid" 2>/dev/null || true
  kill -TERM "$wd" 2>/dev/null || true
  wait "$wd" 2>/dev/null || true
  return 0
}

# _u_notify_cmd <title> <body> — U_NOTIFY_CMD with the title and body appended
# as arguments, so a notify-send-shaped command works unchanged and a command
# that prints a format string (printf "%s|%s\n") receives the two fields.
_u_notify_cmd() {
  local cmd="${U_NOTIFY_CMD:-}"
  [ -n "$cmd" ] || return 0
  _u_notify_timeout 5 sh -c "$cmd \"\$@\"" lightsout "$1" "$2" || true
  return 0
}

# _u_notify_json <title> <body> — a JSON object that survives any reason text.
_u_notify_json() {
  if command -v python3 >/dev/null 2>&1; then
    python3 -c 'import json, sys
print(json.dumps({"title": sys.argv[1], "body": sys.argv[2]}))' "$1" "$2" 2>/dev/null && return 0
  fi
  printf '{"title": "LightsOut", "body": "notification"}'
}

# _u_notify_webhook <title> <body> — POST the event to U_NOTIFY_WEBHOOK. curl
# carries it; when curl is absent python3's urllib does, because the repository
# already requires python3.
_u_notify_webhook() {
  local url="${U_NOTIFY_WEBHOOK:-}" payload
  [ -n "$url" ] || return 0
  payload=$(_u_notify_json "$1" "$2")
  if command -v curl >/dev/null 2>&1; then
    _u_notify_timeout 5 curl -fsS -m 5 -X POST \
      -H 'Content-Type: application/json' --data-binary "$payload" "$url" || true
  elif command -v python3 >/dev/null 2>&1; then
    _u_notify_timeout 5 python3 -c 'import sys, urllib.request
req = urllib.request.Request(sys.argv[1], data=sys.argv[2].encode(),
                             headers={"Content-Type": "application/json"},
                             method="POST")
urllib.request.urlopen(req, timeout=5).read()' "$url" "$payload" || true
  fi
  return 0
}

# _u_notify_desktop <title> <body> — the fallback every Mac has.
_u_notify_desktop() {
  local title="$1" body="$2"
  command -v osascript >/dev/null 2>&1 || return 0
  body="${body//$'\t'/ }"; body="${body//$'\n'/ }"
  title="${title//$'\t'/ }"; title="${title//$'\n'/ }"
  body="${body//\\/\\\\}"; body="${body//\"/\\\"}"
  title="${title//\\/\\\\}"; title="${title//\"/\\\"}"
  _u_notify_timeout 5 osascript \
    -e "display notification \"$body\" with title \"$title\"" || true
  return 0
}

u_notify() { # u_notify <title> <body>
  local title="${1:-LightsOut}" body="${2:-}" alerts="${U_ALERTS:-}"
  # One alert line, whatever the reason contains: the line is a TSV record.
  title="${title//$'\t'/ }"; title="${title//$'\n'/ }"
  body="${body//$'\t'/ }"; body="${body//$'\n'/ }"
  if [ -n "$alerts" ]; then
    mkdir -p "$(dirname "$alerts")" 2>/dev/null || true
    printf '%s\t%s\t%s\n' "$(date -Iseconds)" "$title" "$body" >> "$alerts" 2>/dev/null || true
  fi
  if [ "${U_NOTIFY:-1}" = 0 ]; then
    _u_notify_say "notify suppressed: $title — $body"
    return 0
  fi
  _u_notify_cmd "$title" "$body" || true
  _u_notify_webhook "$title" "$body" || true
  _u_notify_desktop "$title" "$body" || true
  return 0
}
