# locks.sh — cross-job resource locks: two jobs that need the same GPU, media
# encoder or API slot take turns through one file each.
# Sourced by lib/guards.sh, so every job has u_resource beside the other guards.
# Contract: docs/run-contract.md
#
# state/resources/<name>.lock is one line, <pid>TAB<expiry-epoch>. The pid names
# the holder and the expiry is the deadline after which the file is stale even
# if that pid is alive, because a wedged holder (or a pid recycled after a
# reboot) must not stall every later run. A lock whose pid no longer runs is
# stale too: SIGKILL leaves the file behind and the next run has to reclaim it
# rather than deadlock on it. A stale lock is replaced; a live one makes the run
# wait up to U_RESOURCE_WAIT seconds (default 0) and then exit 0 held, because
# being busy is not a failure.

U_RESOURCES=""                      # names this run holds, space separated
U_RESOURCE_TRAPPED="${U_RESOURCE_TRAPPED:-0}"
U_RESOURCE_PREV_TRAP="${U_RESOURCE_PREV_TRAP:-}"

# u_resource_stale <file> — true when nobody holds the lock: no readable owner,
# a pid that no longer runs, or a deadline already passed. A lock that cannot
# name a live holder is not a hold.
u_resource_stale() {
  local file="${1:-}" pid="" exp="" now
  [ -f "$file" ] || return 0
  IFS=$'\t' read -r pid exp < "$file" 2>/dev/null || true
  case "${pid:-}" in ''|*[!0-9]*) return 0 ;; esac
  [ "$pid" -gt 0 ] 2>/dev/null || return 0
  kill -0 "$pid" 2>/dev/null || return 0
  case "${exp:-}" in ''|*[!0-9]*) return 0 ;; esac
  now=$(date +%s)
  [ "$now" -lt "$exp" ] 2>/dev/null || return 0
  return 1
}

# u_resource_holder <file> — the pid a message should name, or unknown.
u_resource_holder() {
  local file="${1:-}" pid=""
  if [ -f "$file" ]; then
    IFS=$'\t' read -r pid _ < "$file" 2>/dev/null || true
  fi
  printf '%s' "${pid:-unknown}"
}

# u_resource_release_all — remove only the locks this run still owns. A lock
# whose deadline passed and another run reclaimed now names that run, so
# removing it by path alone would delete a stranger's hold.
u_resource_release_all() {
  local name file pid
  [ -n "${U_RESOURCES:-}" ] || return 0
  for name in $U_RESOURCES; do
    file="$STATE/resources/$name.lock"
    pid=""
    if [ -f "$file" ]; then
      IFS=$'\t' read -r pid _ < "$file" 2>/dev/null || true
    fi
    [ "${pid:-}" = "$$" ] && rm -f "$file"
  done
  U_RESOURCES=""
  return 0
}

# u_resource_cleanup — the EXIT trap. bash keeps one EXIT trap per shell, so the
# trap that was already installed (lib/guards.sh removes the run lock) is
# captured and run after the resource files, instead of being shadowed by this
# one. The run's own exit status survives, because a block must stay exit 2.
u_resource_cleanup() {
  local rc=$?
  u_resource_release_all
  if [ -n "${U_RESOURCE_PREV_TRAP:-}" ]; then
    eval "$U_RESOURCE_PREV_TRAP" || true
  fi
  return "$rc"
}

# u_resource_trap — install the release trap once, remembering what it replaces.
u_resource_trap() {
  local prev cmd
  [ "${U_RESOURCE_TRAPPED:-0}" = 1 ] && return 0
  U_RESOURCE_TRAPPED=1
  prev=$(trap -p EXIT 2>/dev/null || true)
  if [ -n "$prev" ]; then
    cmd=${prev#trap -- }
    cmd=${cmd% EXIT}
    eval "U_RESOURCE_PREV_TRAP=$cmd" 2>/dev/null || U_RESOURCE_PREV_TRAP=""
  fi
  trap 'u_resource_cleanup' EXIT
}

# u_resource_take <name> <file> — write the lock when it is free; false when a
# live holder has it. The deadline is U_RESOURCE_TTL seconds out (default one
# hour), so a holder that dies without cleaning up cannot hold the resource
# forever.
u_resource_take() {
  local name="$1" file="$2" ttl pid
  u_resource_stale "$file" || return 1
  ttl="${U_RESOURCE_TTL:-3600}"
  case "$ttl" in ''|*[!0-9]*) u_die "U_RESOURCE_TTL must be a whole number of seconds" ;; esac
  [ "$ttl" -gt 0 ] 2>/dev/null || u_die "U_RESOURCE_TTL must be more than zero"
  pid=$(u_resource_holder "$file")
  # Written beside the lock and renamed, so a reader never sees half a line.
  printf '%s\t%s\n' "$$" "$(( $(date +%s) + ttl ))" > "$file.tmp.$$"
  mv -f "$file.tmp.$$" "$file"
  U_RESOURCES="${U_RESOURCES}${U_RESOURCES:+ }$name"
  if [ "$pid" != unknown ]; then
    u_say "resource $name: reclaimed stale lock (pid $pid)"
  fi
  u_say "resource $name acquired (pid $$)"
  return 0
}

# u_resource <name> — hold one named resource for this run. A busy resource is
# a hold, not a failure: wait up to U_RESOURCE_WAIT seconds (default 0) for the
# holder to finish, then exit 0 with the reason in the log. The hold happens
# before an attempt is taken, because a tick that could not work has not tried.
u_resource() {
  local name="${1:-}" file wait_s deadline
  [ -n "$name" ] || u_die "u_resource: a resource name is required"
  case "$name" in
    *[!A-Za-z0-9._-]*) u_die "u_resource: resource name '$name' must be letters, digits, dot, dash or underscore" ;;
  esac
  wait_s="${U_RESOURCE_WAIT:-0}"
  case "$wait_s" in ''|*[!0-9]*) u_die "U_RESOURCE_WAIT must be a whole number of seconds" ;; esac
  file="$STATE/resources/$name.lock"
  if [ "$DRY" = 1 ]; then
    u_say "plan: would take resource $name"
    return 0
  fi
  case " ${U_RESOURCES:-} " in
    *" $name "*) u_say "resource $name already held"; return 0 ;;
  esac
  mkdir -p "$STATE/resources"
  u_resource_trap
  deadline=$(( $(date +%s) + wait_s ))
  while :; do
    u_resource_take "$name" "$file" && return 0
    if [ "$(date +%s)" -ge "$deadline" ]; then
      u_held "resource $name is busy (pid $(u_resource_holder "$file"))"
    fi
    sleep 1
  done
}
