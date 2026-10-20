#!/usr/bin/env python3
"""diagnose.py — explain why a job is stopped, writing only state/diagnosis.md.

Usage: diagnose.py <job-dir>

Reads state/blocked, today's log and out/ from the job directory and writes a
markdown report at state/diagnosis.md. Nothing else is written: the block is
input, never something this file clears or edits.
"""
import datetime
import os
import re
import shlex
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
sys.dont_write_bytecode = True  # state/diagnosis.md is the only file we may write
sys.path.insert(0, HERE)
from report import blocked_fields  # noqa: E402  one parser for the blocked line

CLASS_HELP = {
    "retryable": "transient: an allowlisted repair may clear the cause. The repair "
                 "changes the world, never the verdict — the gate still decides.",
    "judgment": "the artifact did not meet its gate. A human edits the artifact, "
                "then the same gate decides again.",
    "human": "a human must change the environment first: credentials, disk space, "
             "or the decision the reason names.",
}
LOG_TS = re.compile(r"^\[(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})\]")
GATE_FAIL = re.compile(r"gate failed:\s*(\S+)(.*)")


def read(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            return fh.read()
    except OSError:
        return ""


def wall(stamp):
    """A block ISO stamp or a log timestamp as a naive local datetime."""
    stamp = (stamp or "").strip()
    if not stamp or stamp == "unknown":
        return None
    try:
        dt = datetime.datetime.fromisoformat(stamp)
    except ValueError:
        try:
            dt = datetime.datetime.strptime(stamp, "%Y-%m-%d %H:%M:%S")
        except ValueError:
            return None
    if dt.tzinfo is not None:
        dt = dt.astimezone().replace(tzinfo=None)
    return dt


def line_stamp(line):
    m = LOG_TS.match(line)
    return wall(m.group(1)) if m else None


def human_age(seconds):
    if seconds < 60:
        return f"{seconds}s"
    if seconds < 3600:
        return f"{seconds // 60}m"
    if seconds < 86400:
        return f"{seconds // 3600}h{seconds % 3600 // 60:02d}m"
    return f"{seconds // 86400}d{seconds % 86400 // 3600:02d}h"


def human_size(n):
    if n < 1024:
        return f"{n} B"
    n = float(n)
    for unit in ("KB", "MB", "GB"):
        n /= 1024.0
        if n < 1024 or unit == "GB":
            return f"{n:.1f} {unit}"


def rel(path):
    try:
        return os.path.relpath(path, REPO)
    except ValueError:
        return path


def out_files(out_dir):
    files = []
    for root, _dirs, names in os.walk(out_dir):
        for name in names:
            path = os.path.join(root, name)
            try:
                size = os.path.getsize(path)
            except OSError:
                size = 0
            files.append((os.path.relpath(path, out_dir), size))
    return sorted(files)


def blocked(job_dir):
    path = os.path.join(os.environ.get("STATE") or os.path.join(job_dir, "state"),
                        "blocked")
    raw = read(path).strip()
    if not raw:
        return None
    iso_s, cls, src, reason = blocked_fields(raw)
    return {"path": path, "raw": raw, "iso": iso_s or "unknown", "class": cls,
            "source": src, "reason": reason}


def log_path_and_lines(job_dir):
    path = os.environ.get("LOG") or os.path.join(
        job_dir, "logs", datetime.date.today().isoformat() + ".log")
    return path, [ln for ln in read(path).splitlines() if ln.strip()]


def log_tail(lines, when):
    """The last 40 log lines at or before the block (all lines if no block)."""
    if when is None:
        cut = len(lines)
        for i in range(len(lines) - 1, -1, -1):
            if "BLOCKED" in lines[i]:
                cut = i + 1
                break
        chosen = lines[:cut]
    else:
        cutoff = when + datetime.timedelta(seconds=1)
        chosen = []
        for line in lines:
            stamp = line_stamp(line)
            if stamp is None or stamp <= cutoff:
                chosen.append(line)
    return chosen[-40:]


def reason_bullets(blk):
    reason = blk["reason"]
    low = reason.lower()
    out = []
    gate = GATE_FAIL.search(reason)
    if gate:
        target = re.search(r"--file\s+(\S+)", gate.group(2))
        where = f" on `{target.group(1)}`" if target else ""
        out.append(f"- gate `{gate.group(1)}` rejected the artifact{where}; the "
                   f"artifact is the suspect, not the runner.")
    if "attempt cap" in low:
        out.append("- the day's attempt cap is spent: the same stage has failed "
                   "repeatedly, so another tick would only repeat it.")
    if "low disk" in low:
        out.append("- free space is under the job's threshold; find what the job "
                   "has been writing before rerunning.")
    if "memory pass" in low:
        out.append("- the memory pass (the last stage) failed; check the notes it "
                   "saved and the lint/receipt output.")
    if not gate and not any(k in low for k in ("attempt cap", "low disk", "memory pass")):
        out.append("- no gate line explains it; the reason above is the only "
                   "verdict on record.")
    if blk["source"] not in ("", "unknown"):
        out.append(f"- raised by `{blk['source']}`.")
    return out


def registered_repairs():
    body = read(os.path.join(REPO, "lib", "repairs.sh"))
    return re.findall(r"^repair_([A-Za-z0-9_-]+)\s*\(\)", body, re.M)


def gate_command(reason):
    """The gate invocation a human can re-run by hand, rebuilt from the reason."""
    m = GATE_FAIL.search(reason)
    if not m:
        return ""
    try:
        args = shlex.split(m.group(2).strip())
    except ValueError:
        args = m.group(2).split()
    return " ".join(["python3 lib/gates.py", shlex.quote(m.group(1))]
                    + [shlex.quote(a) for a in args])


def build(job_dir):
    name = os.path.basename(os.path.normpath(job_dir))
    out_dir = os.environ.get("OUT") or os.path.join(job_dir, "out")
    blk = blocked(job_dir)
    log_path, lines = log_path_and_lines(job_dir)
    block_dt = wall(blk["iso"]) if blk else None
    tail = log_tail(lines, block_dt)
    fails = [ln for ln in lines if "FAIL " in ln]
    last_fail = fails[-1] if fails else ""
    files = out_files(out_dir)
    age = None
    if block_dt is not None:
        age = max(0, int((datetime.datetime.now() - block_dt).total_seconds()))

    L = [f"# Diagnosis — {name}", "",
         f"_Read-only report from `bin/job diagnose {name}`. It explains the "
         f"current stop from files; it does not change `state/blocked`, "
         f"`state/last`, or any artifact._", ""]
    if blk:
        since = f"blocked since `{blk['iso']}`"
        if age is not None:
            since += f" ({human_age(age)} ago)"
        L += [f"**Status:** {since} — class `{blk['class']}`, source "
              f"`{blk['source']}`",
              f"**Reason:** {blk['reason'] or 'no reason recorded'}"]
    else:
        L += ["**Status:** not blocked (no `state/blocked` marker)"]

    L += ["", "## Cause candidates", ""]
    if blk:
        L.append(f"- class `{blk['class']}` — "
                 f"{CLASS_HELP.get(blk['class'], CLASS_HELP['human'])}")
        L += reason_bullets(blk)
        if last_fail:
            L.append("- the last explicit `FAIL` line in today's log is quoted "
                     "below.")
        if age is not None and age >= 3600:
            L.append(f"- the block has stood for {human_age(age)}; nothing has "
                     f"run since.")
    else:
        L.append("- no block is recorded. If the job stopped, the log tail below "
                 "is the only evidence on disk.")

    L += ["", "## Evidence", ""]
    if blk:
        L += [f"- `{rel(blk['path'])}`, byte for byte:", "", "```tsv",
              blk["raw"], "```", ""]
    if last_fail:
        L += ["- last failing gate line in today's log:", "", "```log",
              last_fail, "```", ""]
    else:
        L.append("- no `FAIL` line in today's log (the block was raised "
                 "directly, or nothing failed).")
    if tail:
        L += [f"- last {len(tail)} log line(s) up to the block "
              f"(`{rel(log_path)}`):", "", "```log", *tail, "```", ""]
    else:
        L.append(f"- log `{rel(log_path)}` is empty or missing.")
    if files:
        total = sum(size for _, size in files)
        L += [f"- `out/` holds {len(files)} file(s), {human_size(total)}:", "",
              "```out", *[f"{p}\t{human_size(s)}" for p, s in files[:25]], "```"]
        if len(files) > 25:
            L.append(f"- {len(files) - 25} more file(s) not shown.")
        L.append("")
    else:
        L.append("- `out/` is empty: no artifact was produced.")

    L += ["## Next commands", ""]
    if not blk:
        L += ["```sh", "# nothing to clear; see what would run now:",
              f"bin/job plan {name}", f"bin/job run {name}", "```"]
    elif blk["class"] == "retryable":
        L += ["```sh", "# run the allowlisted repair; the gate still decides",
              f"bin/job retry {name}", f"bin/job status {name}", "```"]
        repairs = registered_repairs()
        if repairs:
            L += ["", "Registered in `lib/repairs.sh`: "
                  + ", ".join(f"`{r}`" for r in repairs) + "."]
    elif blk["class"] == "judgment":
        L += ["```sh", "# 1. edit the artifact the gate rejected",
              "# 2. clear the block, then rerun so the same gate decides",
              f"bin/job unblock {name}", f"bin/job run {name}", "```"]
        command = gate_command(blk["reason"])
        if command:
            L += ["", "Check the artifact by hand first:", "", "```sh",
                  command, "```"]
    else:
        L += ["```sh",
              "# 1. fix what the reason names: credentials, disk space, or the "
              "decision",
              "# 2. clear the marker only after a human looks",
              f"bin/job unblock {name}", f"bin/job run {name}", "```"]
    L += ["", f"Re-run `bin/job diagnose {name}` after a change to refresh this "
              f"report.", ""]
    return "\n".join(L)


def main(argv):
    if len(argv) != 1:
        print("usage: diagnose.py <job-dir>", file=sys.stderr)
        return 64
    job_dir = os.path.abspath(argv[0])
    if not os.path.isdir(job_dir):
        print(f"no such job directory: {job_dir}", file=sys.stderr)
        return 64
    state = os.environ.get("STATE") or os.path.join(job_dir, "state")
    path = os.path.join(state, "diagnosis.md")
    try:
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(build(job_dir))
    except OSError as exc:
        print(f"cannot write {path}: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
