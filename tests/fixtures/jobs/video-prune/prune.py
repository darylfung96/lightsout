#!/usr/bin/env python3
"""video-prune — remove render intermediates for episodes that are already shipped.

The safe policy is deliberately narrow: an episode is eligible only when its
master exists and its queue entry is `posted`.  The only directory the pruner
removes is `episodes/<NNN>/comp/<name>/renders/`; media, audio, comp assets,
snapshots and the outbox are never touched.  The last N episodes are kept in
full so a recent episode can still be re-rendered without a rebuild.

The pruner writes a TSV manifest before deleting anything.  `--apply 0` is the
default, so a run is a reviewable plan; `--apply 1` performs the deletions.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import time
from pathlib import Path

HEADER = "episode\tpath\tmb\taction\treason\n"


def fail(message: str) -> int:
    print(f"prune: {message}", file=sys.stderr)
    return 2


def mb(size: int) -> float:
    return size / (1024 * 1024)


def dir_bytes(path: Path) -> int:
    total = 0
    for root, dirs, files in os.walk(path, followlinks=False):
        dirs[:] = [d for d in dirs if not os.path.islink(os.path.join(root, d))]
        for name in files:
            try:
                total += os.lstat(os.path.join(root, name)).st_size
            except OSError:
                pass
    return total


def write_manifest(path: Path, rows: list[tuple[str, str, float, str, str]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as fh:
        fh.write(HEADER)
        for episode, target, size, action, reason in rows:
            fh.write(f"{episode}\t{target}\t{size:.1f}\t{action}\t{reason}\n")


def safe_renders(repo: Path, episode: str, path: Path) -> tuple[bool, str]:
    """Accept only repo/episodes/<episode>/comp/<name>/renders, no symlinks."""
    if path.is_symlink():
        return False, "symlink"
    try:
        rel = path.relative_to(repo)
    except ValueError:
        return False, "outside repo"
    parts = rel.parts
    if (
        len(parts) != 5
        or parts[0] != "episodes"
        or parts[1] != episode
        or parts[2] != "comp"
        or parts[4] != "renders"
    ):
        return False, "not an episode renders directory"
    current = repo
    for part in parts:
        current = current / part
        if current.is_symlink():
            return False, "symlink component"
    try:
        resolved = path.resolve()
        resolved.relative_to(repo.resolve())
    except (OSError, ValueError):
        return False, "resolves outside repo"
    if not resolved.is_dir():
        return False, "not a directory"
    return True, ""


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--video-repo", required=True)
    parser.add_argument("--keep", type=int, default=3)
    parser.add_argument("--min-age-hours", type=float, default=24.0)
    parser.add_argument("--apply", type=int, choices=(0, 1), default=0)
    parser.add_argument("--plan", required=True)
    parser.add_argument("--applied")
    args = parser.parse_args()

    repo = Path(args.video_repo).expanduser().resolve()
    if not repo.is_dir():
        return fail(f"video repo is not a directory: {repo}")
    if args.keep < 0:
        return fail("--keep must be zero or greater")
    if args.min_age_hours < 0:
        return fail("--min-age-hours must be zero or greater")

    queue_path = repo / "pipeline" / "publish" / "queue.json"
    try:
        queue = json.loads(queue_path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        return fail(f"cannot read {queue_path}: {exc}")
    if not isinstance(queue, dict):
        return fail(f"{queue_path} is not a JSON object")

    episodes_root = repo / "episodes"
    if not episodes_root.is_dir():
        return fail(f"no episodes directory: {episodes_root}")
    episodes = sorted(
        entry.name
        for entry in episodes_root.iterdir()
        if entry.is_dir() and len(entry.name) == 3 and entry.name.isdigit()
    )
    keep = set(episodes[-args.keep :]) if args.keep else set()

    rows: list[tuple[str, str, float, str, str]] = []
    candidates: list[tuple[str, Path, float]] = []
    now = time.time()

    for episode in episodes:
        episode_dir = episodes_root / episode
        if episode in keep:
            rows.append((episode, str(episode_dir), 0.0, "keep", "last N episodes"))
            continue

        master = repo / "outbox" / f"{episode}-master.mp4"
        if not master.is_file() or master.is_symlink():
            rows.append((episode, str(episode_dir), 0.0, "skip", "no master"))
            continue

        status = queue.get(episode, {})
        status = status.get("status") if isinstance(status, dict) else None
        if status != "posted":
            rows.append(
                (episode, str(episode_dir), 0.0, "skip", f"queue status is {status!r}")
            )
            continue

        comp_root = episode_dir / "comp"
        if not comp_root.is_dir():
            rows.append((episode, str(comp_root), 0.0, "skip", "no comp directory"))
            continue

        found = False
        for render in sorted(comp_root.glob("*/renders")):
            found = True
            ok, reason = safe_renders(repo, episode, render)
            if not ok:
                rows.append((episode, str(render), 0.0, "skip", reason))
                continue
            age_h = (now - render.stat().st_mtime) / 3600.0
            if age_h < args.min_age_hours:
                rows.append(
                    (episode, str(render), 0.0, "skip", f"younger than {args.min_age_hours:g}h")
                )
                continue
            size = dir_bytes(render)
            if size == 0:
                rows.append((episode, str(render), 0.0, "skip", "empty"))
                continue
            candidates.append((episode, render, mb(size)))
            rows.append((episode, str(render), mb(size), "delete", "master + posted"))

        if not found:
            rows.append((episode, str(comp_root), 0.0, "skip", "no renders directory"))

    plan_path = Path(args.plan)
    write_manifest(plan_path, rows)
    would_free = sum(size for _, _, size in candidates)
    before = shutil.disk_usage(repo).free
    print(
        f"prune: {len(candidates)} candidate(s), {would_free:.1f} MB, "
        f"apply={args.apply}, keep={args.keep}, free={mb(before):.1f} MB"
    )
    print(f"prune: plan {plan_path}")

    if args.apply == 0:
        for episode, render, size in candidates:
            print(f"would remove\t{episode}\t{render}\t{size:.1f} MB")
        return 0

    removed = 0
    removed_rows: list[tuple[str, str, float, str, str]] = []
    for episode, render, size in candidates:
        try:
            shutil.rmtree(render)
        except OSError as exc:
            print(f"prune: cannot remove {render}: {exc}", file=sys.stderr)
            return 1
        removed += 1
        removed_rows.append((episode, str(render), size, "removed", "master + posted"))
        print(f"removed\t{episode}\t{render}\t{size:.1f} MB")

    if args.applied:
        write_manifest(Path(args.applied), removed_rows)

    after = shutil.disk_usage(repo).free
    print(
        f"prune: removed {removed} director{'y' if removed == 1 else 'ies'}, "
        f"{mb(after - before):.1f} MB reclaimed, free={mb(after):.1f} MB"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
