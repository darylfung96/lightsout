#!/usr/bin/env python3
"""gates.py — named pass/fail checks on artifacts.

A gate is the only thing allowed to say a step succeeded. Exit 0 pass, 1 fail,
64 misuse. An unimplemented check is `never`, never a missing line.
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import urllib.request


def exists(path="", **kw):
    import os
    return (os.path.isfile(path) and os.path.getsize(path) > 0,
            f"{path} exists" if os.path.isfile(path) else f"{path} missing")


def contains(path="", text="", **kw):
    try:
        body = open(path, encoding="utf-8", errors="replace").read()
    except OSError as exc:
        return False, str(exc)
    return (text in body, f"{path} contains {text!r}")


def sections(path="", require="", min_words=0, max_words=10**9, **kw):
    try:
        body = open(path, encoding="utf-8", errors="replace").read()
    except OSError as exc:
        return False, str(exc)
    words = len(body.split())
    if not min_words <= words <= max_words:
        return False, f"{words} words, want {min_words}-{max_words}"
    for name in [r.strip() for r in require.split(",") if r.strip()]:
        if not re.search(rf"^#+\s+.*{re.escape(name)}", body, re.M | re.I):
            return False, f"missing section {name!r} ({words} words)"
    return True, f"{words} words, {len(require.split(','))} sections"


def _probe(path):
    if not shutil.which("ffprobe"):
        return None, "ffprobe not installed"
    out = subprocess.run(
        ["ffprobe", "-v", "error", "-print_format", "json",
         "-show_format", "-show_streams", path],
        capture_output=True, text=True)
    if out.returncode != 0:
        return None, (out.stderr or "ffprobe failed").strip().splitlines()[-1]
    return json.loads(out.stdout), ""


def duration(path="", min=0.0, max=10**9, **kw):
    probed, err = _probe(path)
    if probed is None:
        return False, err
    dur = float(probed.get("format", {}).get("duration") or 0)
    return (min <= dur <= max, f"{dur:.3f}s, want {min}-{max}s")


def resolution(path="", w=0, h=0, **kw):
    probed, err = _probe(path)
    if probed is None:
        return False, err
    for stream in probed.get("streams", []):
        if stream.get("codec_type") == "video":
            cw, ch = int(stream.get("width") or 0), int(stream.get("height") or 0)
            if (w, h) == (0, 0):
                return True, f"{cw}x{ch}"
            return ((cw, ch) == (w, h), f"{cw}x{ch}, want {w}x{h}")
    return False, "no video stream"


def loudness(path="", target=-14.0, tol=1.0, **kw):
    if not shutil.which("ffmpeg"):
        return False, "ffmpeg not installed"
    out = subprocess.run(
        ["ffmpeg", "-hide_banner", "-nostats", "-i", path,
         "-af", "ebur128", "-f", "null", "-"],
        capture_output=True, text=True)
    hits = re.findall(r"I:\s+(-?\d+\.\d+)\s+LUFS", out.stderr)
    if not hits:
        return False, "no integrated loudness in ffmpeg output"
    lufs = float(hits[-1])
    return (abs(lufs - target) <= tol, f"{lufs:.1f} LUFS, want {target}+-{tol}")


def manifest(path="", min=0, **kw):
    """A shot list: every shot approved, and every file present and non-empty.

    The file is JSON, a list of {shot, query, file, approved}. Approval is a
    human act recorded in that file, so a gate may only read it. The message
    names the first unapproved shot — or the first approved shot whose file is
    missing or empty — because that is the shot a human has to act on. A list
    that is empty, shorter than --min, or unreadable fails closed: a footage
    stage that produced nothing has not passed (docs/asset-manifest.md).
    """
    try:
        with open(path, encoding="utf-8") as fh:
            shots = json.load(fh)
    except OSError as exc:
        return False, str(exc)
    except ValueError as exc:
        return False, f"{path} is not valid JSON: {exc}"
    if not isinstance(shots, list):
        return False, f"{path} is not a list of shots"
    want = int(min)
    plural = "" if len(shots) == 1 else "s"
    if len(shots) < want:
        return False, f"{len(shots)} shot{plural}, want at least {want}"
    if not shots:
        return False, f"{path} lists no shots"

    def who(entry, i):
        query = entry.get("query")
        label = f"shot {entry.get('shot', i + 1)}"
        return f"{label} {query!r}" if query else label

    for i, entry in enumerate(shots):
        if not isinstance(entry, dict):
            return False, f"shot {i + 1} is not an object"
        if entry.get("approved") is not True:
            return False, f"{who(entry, i)} is not approved"
    for i, entry in enumerate(shots):
        name = entry.get("file") or ""
        if not (os.path.isfile(name) and os.path.getsize(name) > 0):
            return False, f"{who(entry, i)}: {name or 'no file'} is missing or empty"
    return True, f"{len(shots)} shot{plural} approved"


def permalink(url="", **kw):
    req = urllib.request.Request(url, headers={"User-Agent": "lightsout-gate/1"})
    try:
        with urllib.request.urlopen(req, timeout=20) as resp:
            code = resp.status
    except Exception as exc:  # noqa: BLE001 - any network failure is a fail
        return False, f"{url} unreachable: {exc}"
    return (code in (200, 301, 302), f"{url} -> HTTP {code}")


def never(**kw):
    return False, "NOT IMPLEMENTED — this gate must be written before the job runs"


GATES = {f.__name__: f for f in
         (exists, contains, sections, duration, resolution, loudness, manifest,
          permalink, never)}


def main(argv):
    parser = argparse.ArgumentParser(description="artifact gates")
    parser.add_argument("gate", choices=sorted(GATES))
    parser.add_argument("--file", dest="path", default="")
    parser.add_argument("--url", default="")
    parser.add_argument("--text", default="")
    parser.add_argument("--require", default="")
    parser.add_argument("--min", type=float, default=0.0)
    parser.add_argument("--max", type=float, default=10**9)
    parser.add_argument("--min-words", type=int, default=0)
    parser.add_argument("--max-words", type=int, default=10**9)
    parser.add_argument("--w", type=int, default=0)
    parser.add_argument("--h", type=int, default=0)
    parser.add_argument("--target", type=float, default=-14.0)
    parser.add_argument("--tol", type=float, default=1.0)
    args = parser.parse_args(argv)

    kw = vars(args)
    gate = GATES[args.gate]
    try:
        ok, msg = gate(**kw)
    except TypeError as exc:
        print(f"FAIL {args.gate}: {exc}")
        return 64
    print(("PASS " if ok else "FAIL ") + f"{args.gate}: {msg}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
