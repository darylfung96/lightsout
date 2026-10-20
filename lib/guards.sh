# guards.sh — the four things every robot needs, written once.
# Source from jobs/<name>/job.sh. Requires REPO JOB JOB_DIR STATE OUT LOG UNIT DRY.
# Contract: docs/run-contract.md

u_say() {
  local m="[$(date '+%F %T')] $*"
  printf '%s\n' "$m"
  printf '%s\n' "$m" >> "$LOG"
}

u_held()  { u_say "held: $*"; exit 0; }
u_die()   { u_say "FATAL: $*"; exit 1; }

# u_notify lives in lib/notify.sh: the channel list and its timeouts are one
# file, so an alarm raised by `bin/job check` and a block raised by a job reach
# the same places (docs/run-contract.md).
. "$REPO/lib/notify.sh"

# blocked = retrying will not fix it. One notification, then quiet persistence.
# state/blocked is TSV <iso>TAB<class>TAB<source>TAB<reason>; the class decides
# what may be done about it. Defaults: human, unknown (docs/run-contract.md).
u_block() {
  local cls="${U_BLOCK_CLASS:-human}" src="${U_BLOCK_SOURCE:-unknown}" reason
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --class)    [ "$#" -ge 2 ] || u_die "u_block: --class needs a value"
                  cls="$2"; shift 2 ;;
      --class=*)  cls="${1#*=}"; shift ;;
      --source)   [ "$#" -ge 2 ] || u_die "u_block: --source needs a value"
                  src="$2"; shift 2 ;;
      --source=*) src="${1#*=}"; shift ;;
      --)         shift; break ;;
      *)          break ;;
    esac
  done
  case "$cls" in
    retryable|judgment|human) ;;
    *) u_die "u_block: class '$cls' is not retryable, judgment or human" ;;
  esac
  [ -n "$src" ] || src=unknown
  # one line only: a tab or newline in a field would corrupt the TSV
  cls="${cls//$'\t'/ }"; cls="${cls//$'\n'/ }"
  src="${src//$'\t'/ }"; src="${src//$'\n'/ }"
  reason="$*"
  reason="${reason//$'\t'/ }"; reason="${reason//$'\n'/ }"
  if [ "$DRY" = 1 ]; then u_say "plan: would block [$cls]: $reason"; exit 0; fi
  printf '%s\t%s\t%s\t%s\n' "$(date -Iseconds)" "$cls" "$src" "$reason" > "$STATE/blocked"
  u_say "BLOCKED [$cls]: $reason"
  u_notify "LightsOut: $JOB blocked" "$reason"
  exit 2
}

# u_sh <description> <command...> — the only way a stage should run, so plan is free.
u_sh() {
  local desc="$1"; shift
  if [ "$DRY" = 1 ]; then u_say "plan: would $desc"; return 0; fi
  u_say "$desc"
  "$@"
}

# u_gate <name> [args] — the only way a stage says it succeeded.
u_gate() {
  if [ "$DRY" = 1 ]; then u_say "plan: would check $1"; return 0; fi
  python3 "$REPO/lib/gates.py" "$@" || u_block "gate failed: $*"
}

u_guard_day() {
  local unit="${1:-$UNIT}"
  if [ -f "$STATE/done/$unit" ]; then
    u_held "$unit already done ($(date -r "$STATE/done/$unit" '+%F %T'))"
  fi
  if [ "$DRY" = 1 ]; then u_say "plan: day guard open for $unit"; fi
}

u_attempts() {
  find "$STATE/attempts" -maxdepth 1 -name "$(date +%F).attempt-*" 2>/dev/null | wc -l | tr -d ' '
}

u_guard_attempts() {
  local max="${MAX_ATTEMPTS:-2}" n
  n=$(u_attempts)
  if [ "$n" -ge "$max" ]; then
    u_block "attempt cap reached ($n of $max today)"
  fi
  if [ "$DRY" = 1 ]; then u_say "plan: would take attempt $((n + 1)) of $max"; return 0; fi
  : > "$STATE/attempts/$(date +%F).attempt-$((n + 1))"
  u_say "attempt $((n + 1)) of $max"
}

# u_stage_attempts <id> — attempts this stage has taken today. The day cap's
# files are <date>.attempt-N, so a stage's <date>.<id>.attempt-N never count
# against the job as a whole and vice versa.
u_stage_attempts() {
  local id="$1"
  find "$STATE/attempts" -maxdepth 1 -name "$(date +%F).$id.attempt-*" 2>/dev/null \
    | wc -l | tr -d ' '
}

# u_guard_stage <id> — the per-stage attempt cap. The day cap says the job has
# spent its budget; this says which stage is spending it, so a stage that keeps
# failing stops the job with a reason a human can act on. Files are
# state/attempts/<date>.<id>.attempt-N and the block is class human: a stage
# that failed MAX_STAGE_ATTEMPTS times today is a code or input problem, not
# something the next tick can fix.
u_guard_stage() {
  local id="${1:-}" max="${MAX_STAGE_ATTEMPTS:-2}" n
  [ -n "$id" ] || u_die "u_guard_stage: stage id required"
  case "$id" in
    *[!A-Za-z0-9._-]*) u_die "u_guard_stage: stage id '$id' must be letters, digits, dot, dash or underscore" ;;
  esac
  case "$max" in
    ''|*[!0-9]*) u_die "MAX_STAGE_ATTEMPTS must be a whole number" ;;
  esac
  n=$(u_stage_attempts "$id")
  if [ "$n" -ge "$max" ]; then
    u_block --class human "stage $id attempt cap reached ($n of $max today)"
  fi
  if [ "$DRY" = 1 ]; then
    u_say "plan: would take stage $id attempt $((n + 1)) of $max"
    return 0
  fi
  : > "$STATE/attempts/$(date +%F).$id.attempt-$((n + 1))"
  u_say "stage $id attempt $((n + 1)) of $max"
}

u_guard_lock() {
  local pid=""
  if [ -d "$STATE/lock" ]; then
    pid=$(cat "$STATE/lock/pid" 2>/dev/null || true)
    if [ -n "${pid:-}" ] && kill -0 "$pid" 2>/dev/null; then
      u_held "another run (pid $pid) holds the lock"
    fi
    u_say "clearing stale lock (pid ${pid:-unknown})"
    rm -rf "$STATE/lock"
  fi
  if [ "$DRY" = 1 ]; then u_say "plan: would take the lock"; return 0; fi
  mkdir -p "$STATE/lock"
  printf '%s\n' "$$" > "$STATE/lock/pid"
  trap 'rm -rf "$STATE/lock"' EXIT
}

u_guard_disk() {
  local need="${1:-2000}" free
  free=$(df -kP "$REPO" | awk 'NR==2{print int($4/1024)}')
  if [ "${free:-0}" -lt "$need" ]; then
    u_block "low disk: ${free} MB free, need ${need} MB"
  fi
  u_say "disk ok (${free} MB free)"
}

# --- spend ---------------------------------------------------------------
# state/ledger.tsv is <iso>TAB<cents>TAB<label>TAB<unit>, one line per charge.
# The label says what was paid for and the unit says what it bought, so a job's
# money is attributable; only lines whose iso starts with today's date count
# against today's budget, because a budget is a day.

u_spent_today() {
  local day
  day=$(date +%F)
  if [ ! -f "$STATE/ledger.tsv" ]; then printf '0'; return 0; fi
  awk -F'\t' -v day="$day" '
    $1 ~ "^" day && $2 ~ /^[0-9]+$/ { sum += $2 }
    END { printf "%d", sum + 0 }' "$STATE/ledger.tsv"
}

# u_spend <cents> <label> — book one charge. A charge is a fact about money
# already gone, so it is written when it is spent, never speculatively, and
# never in plan mode.
u_spend() {
  local cents="${1:-}" label="${2:-}" unit="${UNIT:-unknown}"
  [ -n "$cents" ] || u_die "u_spend: cents required"
  case "$cents" in *[!0-9]*) u_die "u_spend: '$cents' is not whole cents" ;; esac
  [ -n "$label" ] || u_die "u_spend: a label is required"
  # one line per charge: a tab or newline in a field would corrupt the TSV
  label="${label//$'\t'/ }"; label="${label//$'\n'/ }"
  unit="${unit//$'\t'/ }"; unit="${unit//$'\n'/ }"
  if [ "$DRY" = 1 ]; then
    u_say "plan: would spend ${cents}c on $label (unit $unit)"
    return 0
  fi
  mkdir -p "$STATE"
  printf '%s\t%s\t%s\t%s\n' "$(date -Iseconds)" "$cents" "$label" "$unit" \
    >> "$STATE/ledger.tsv"
  u_say "spend: ${cents}c $label (unit $unit)"
}

# u_guard_spend <max_cents> — the day's ceiling for this job, read from the
# ledger rather than from a variable, so a retry cannot forget what earlier
# runs spent. A job calls it after a charge: money already gone cannot be
# unspent, so the run that crosses the ceiling is the run that stops and names
# the two numbers; the block is class human because a spent budget is not
# something the next tick can fix.
u_guard_spend() {
  local max="${1:-}" total
  [ -n "$max" ] || u_die "u_guard_spend: a cap in cents is required"
  case "$max" in *[!0-9]*) u_die "u_guard_spend: '$max' is not whole cents" ;; esac
  total=$(u_spent_today)
  if [ "$total" -gt "$max" ]; then
    u_block --class human "spend cap reached (${total} of ${max} cents today)"
  fi
  if [ "$DRY" = 1 ]; then
    u_say "plan: spend ${total}c of ${max}c today"
  else
    u_say "spend ok (${total}c of ${max}c today)"
  fi
}

u_done() {
  local unit="${1:-$UNIT}"
  if [ "$DRY" = 1 ]; then u_say "plan: would mark $unit done"; return 0; fi
  : > "$STATE/done/$unit"
  rm -f "$STATE/blocked"
  u_say "done: $unit"
}
