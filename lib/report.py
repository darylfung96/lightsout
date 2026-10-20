#!/usr/bin/env python3
"""report.py — rebuild the picture of every job from its state files alone."""
import argparse
import json
import os
import re
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CLASSES = ("retryable", "judgment", "human")


def blocked_fields(raw):
    """state/blocked is <iso>TAB<class>TAB<source>TAB<reason>.

    A two-field file from an older run reads as human with field 2 as the
    reason; missing fields fall back to unknown / a placeholder.
    """
    parts = raw.split("\t", 3)
    if len(parts) == 2:
        return parts[0], "human", "unknown", parts[1]
    iso_s = parts[0]
    cls = parts[1] if len(parts) > 1 and parts[1] in CLASSES else "human"
    src = parts[2] if len(parts) > 2 and parts[2] else "unknown"
    reason = parts[3] if len(parts) > 3 and parts[3] else "no reason recorded"
    return iso_s, cls, src, reason


def iso(path, fallback=None):
    try:
        return time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime(os.path.getmtime(path)))
    except OSError:
        return fallback


def spend_today(st):
    """Cents booked today in state/ledger.tsv.

    The ledger is <iso>TAB<cents>TAB<label>TAB<unit>, one line per charge; only
    lines whose date is today are today's money, which is the same filter
    u_guard_spend applies.
    """
    today = time.strftime("%Y-%m-%d")
    try:
        lines = open(os.path.join(st, "ledger.tsv"), errors="replace").read().splitlines()
    except OSError:
        return 0
    total = 0
    for line in lines:
        parts = line.split("\t")
        if len(parts) < 2 or not parts[0].startswith(today):
            continue
        try:
            total += int(parts[1])
        except ValueError:
            continue
    return total


def expect_time(st):
    """state/expect is one line, HH:MM local: the time a run is due.

    Returns (minutes since midnight, "HH:MM") or None. A missing or malformed
    file means the job states no expectation, and a job that is not expected is
    never late; guessing a schedule from its history would invent the alarm.
    """
    try:
        raw = open(os.path.join(st, "expect"), errors="replace").read()
    except OSError:
        return None
    lines = raw.strip().splitlines()
    line = lines[0].strip() if lines else ""
    m = re.match(r"^([01][0-9]|2[0-3]):([0-5][0-9])$", line)
    if not m:
        return None
    return int(m.group(1)) * 60 + int(m.group(2)), line


def held_today(st):
    """True when a run today ended rc 0 without writing done/<today>.

    bin/job records every completed run in state/last as <iso>TAB<rc>TAB<verb>,
    and only a run that reaches u_done writes today's done file. So rc 0 with no
    done is a legitimate hold — the day guard after another unit, or the lock —
    and must not read as a missed run. bin/job's check verb reads the file the
    same way, so the dashboard and the alarm cannot disagree.
    """
    prefix = time.strftime("%Y-%m-%d") + "T"
    try:
        lines = open(os.path.join(st, "last"), errors="replace").read().splitlines()
    except OSError:
        return False
    for line in lines:
        parts = line.split("\t")
        if len(parts) >= 2 and parts[0].startswith(prefix) and parts[1] == "0":
            return True
    return False


def job_state(name):
    d = os.path.join(REPO, "jobs", name)
    st = os.path.join(d, "state")
    out = os.path.join(d, "out")
    log = os.path.join(d, "logs", time.strftime("%Y-%m-%d") + ".log")
    rec = {
        "name": name,
        "has_job": os.path.isfile(os.path.join(d, "job.sh")),
        "has_spec": os.path.isfile(os.path.join(d, "spec.md")),
        "state": "never run",
        "detail": "",
        "class": None,
        "source": None,
        "since": None,
        "stage": "",
        "attempts_today": 0,
        "spend_today": 0,
        "last_exit": None,
        "last_run": None,
        "out_files": 0,
        "expect": None,
        "stale": False,
    }
    today = time.strftime("%Y-%m-%d")
    attempts = os.path.join(st, "attempts")
    if os.path.isdir(attempts):
        rec["attempts_today"] = len([f for f in os.listdir(attempts)
                                     if f.startswith(today + ".attempt-")])
    rec["spend_today"] = spend_today(st)
    last = os.path.join(st, "last")
    if os.path.isfile(last):
        rows = [r for r in open(last).read().splitlines() if r.strip()]
        if rows:
            parts = rows[-1].split("\t")
            if len(parts) >= 3:
                rec["last_run"], rec["last_exit"] = parts[0], int(parts[1])
    done = os.path.join(st, "done")
    done_today = os.path.isdir(done) and today in os.listdir(done)
    if done_today:
        rec["state"] = "done today"
        rec["since"] = iso(os.path.join(done, today))
    blocked = os.path.join(st, "blocked")
    blocked_now = os.path.isfile(blocked)
    if blocked_now:
        raw = open(blocked, errors="replace").read().strip()
        rec["since"], rec["class"], rec["source"], rec["detail"] = blocked_fields(raw)
        rec["state"] = "blocked"
    locked = os.path.isdir(os.path.join(st, "lock"))
    if locked:
        rec["state"] = "running"
    if os.path.isfile(log):
        hits = re.findall(r"\[(.+?)\] stage (\d+): (.*)", open(log, errors="replace").read())
        if hits:
            when, num, what = hits[-1]
            rec["stage"] = f"stage {num}: {what}"
            rec["last_run"] = rec["last_run"] or when
    if os.path.isdir(out):
        rec["out_files"] = sum(len(f) for _, _, f in os.walk(out))
    # A run is missed when the clock has reached the expected time and nothing
    # explains the gap: done, a lock, a block, or a run that ended held. A
    # blocked job is not stale because "blocked" is the more specific stop; the
    # check verb still exits 2 for one that missed its run, so a scheduler keeps
    # seeing a non-zero state.
    exp = expect_time(st)
    if exp:
        rec["expect"] = exp[1]
        now = time.localtime()
        rec["stale"] = bool(now.tm_hour * 60 + now.tm_min >= exp[0] and not done_today
                            and not locked and not blocked_now and not held_today(st))
    return rec


def jobs():
    base = os.path.join(REPO, "jobs")
    names = [n for n in sorted(os.listdir(base)) if os.path.isdir(os.path.join(base, n))]
    return [job_state(n) for n in names]


ORDER = {"blocked": 0, "running": 1, "never run": 3, "done today": 2}


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--html", default="")
    ap.add_argument("name", nargs="?")
    ap.add_argument("--all", action="store_true")
    args = ap.parse_args(argv)
    data = [r for r in jobs() if not args.name or r["name"] == args.name]
    data = sorted(data, key=lambda r: (ORDER.get(r["state"], 9), r["name"]))

    if args.json:
        print(json.dumps(data, indent=2))
        return 0

    if args.html:
        rows = "\n".join(
            f"<tr class='{r['state'].replace(' ', '-')}'>"
            f"<td><b>{r['name']}</b></td><td>{r['state']}</td>"
            f"<td>{r['detail']}</td><td>{r['stage'] or '—'}</td>"
            f"<td>{r['last_run'] or '—'}</td><td>{r['attempts_today']}</td></tr>"
            for r in data) or "<tr><td colspan=6>no jobs yet</td></tr>"
        page = f"""<!doctype html><meta charset=utf-8>
<title>LightsOut</title>
<style>
body{{font:15px/1.5 -apple-system,system-ui,sans-serif;margin:2rem;color:#1b1b1b}}
table{{border-collapse:collapse;width:100%}}
th,td{{text-align:left;padding:.5rem .75rem;border-bottom:1px solid #e5e5e5;vertical-align:top}}
th{{font-size:.8rem;text-transform:uppercase;letter-spacing:.05em;color:#666}}
tr.blocked{{background:#fdeceb}} tr.blocked td:nth-child(2){{color:#b42318;font-weight:600}}
tr.running{{background:#fff8e6}} tr.done-today td:nth-child(2){{color:#087443}}
caption{{text-align:left;font-weight:600;margin-bottom:.75rem}}
</style><table><caption>LightsOut — {time.strftime('%Y-%m-%d %H:%M')}</caption>
<tr><th>job</th><th>state</th><th>why</th><th>stage</th><th>last run</th><th>attempts</th></tr>
{rows}</table>"""
        os.makedirs(os.path.dirname(os.path.abspath(args.html)), exist_ok=True)
        open(args.html, "w").write(page)
        print(args.html)
        return 0

    for r in data:
        print(f"{r['state']:<11} {r['name']:<14} {r['detail'][:70]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(__import__("sys").argv[1:]))
