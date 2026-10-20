#!/usr/bin/env python3
"""report.py — rebuild the picture of every job from its state files alone."""
import argparse
import json
import os
import re
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def iso(path, fallback=None):
    try:
        return time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime(os.path.getmtime(path)))
    except OSError:
        return fallback


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
        "since": None,
        "stage": "",
        "attempts_today": 0,
        "last_exit": None,
        "last_run": None,
        "out_files": 0,
    }
    today = time.strftime("%Y-%m-%d")
    attempts = os.path.join(st, "attempts")
    if os.path.isdir(attempts):
        rec["attempts_today"] = len([f for f in os.listdir(attempts)
                                     if f.startswith(today + ".attempt-")])
    last = os.path.join(st, "last")
    if os.path.isfile(last):
        rows = [r for r in open(last).read().splitlines() if r.strip()]
        if rows:
            parts = rows[-1].split("\t")
            if len(parts) >= 3:
                rec["last_run"], rec["last_exit"] = parts[0], int(parts[1])
    done = os.path.join(st, "done")
    if os.path.isdir(done) and today in os.listdir(done):
        rec["state"] = "done today"
        rec["since"] = iso(os.path.join(done, today))
    blocked = os.path.join(st, "blocked")
    if os.path.isfile(blocked):
        raw = open(blocked).read().strip().split("\t")
        rec["state"] = "blocked"
        rec["since"] = raw[0] if raw else None
        rec["detail"] = raw[1] if len(raw) > 1 else "no reason recorded"
    if os.path.isdir(os.path.join(st, "lock")):
        rec["state"] = "running"
    if os.path.isfile(log):
        hits = re.findall(r"\[(.+?)\] stage (\d+): (.*)", open(log, errors="replace").read())
        if hits:
            when, num, what = hits[-1]
            rec["stage"] = f"stage {num}: {what}"
            rec["last_run"] = rec["last_run"] or when
    if os.path.isdir(out):
        rec["out_files"] = sum(len(f) for _, _, f in os.walk(out))
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
