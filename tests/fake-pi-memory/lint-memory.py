#!/usr/bin/env python3
"""fake lint-memory.py — the test double for ~/pi-memory/lint-memory.py.

Tests point U_PI_MEMORY here so no check ever reaches the real store: this
script records the call and exits with FAKE_MEMORY_RC, or fails the lint step
when FAKE_MEMORY_FAIL=lint (tests/run.sh, Task 11).
"""
import os
import sys


def main():
    log = os.environ.get("FAKE_MEMORY_LOG")
    if log:
        with open(log, "a", encoding="utf-8") as fh:
            fh.write("lint " + " ".join(sys.argv[1:]) + "\n")
    if os.environ.get("FAKE_MEMORY_FAIL") == "lint":
        print("fake lint-memory: failed", file=sys.stderr)
        return 1
    return int(os.environ.get("FAKE_MEMORY_RC", "0"))


if __name__ == "__main__":
    sys.exit(main())
