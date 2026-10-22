#!/usr/bin/env python3
"""fake scan-duplicates.py — the test double for ~/agent-memory's exact-repeat scan.

Tests point U_PI_MEMORY here so no check reaches the real store. The script
records the call, honors FAKE_MEMORY_FAIL=scan, and prints the count from
FAKE_MEMORY_DUPES (default 0) so tests can prove the gate fails closed.
"""
import os
import sys


def main() -> int:
    log = os.environ.get("FAKE_MEMORY_LOG")
    if log:
        with open(log, "a", encoding="utf-8") as fh:
            fh.write("scan " + " ".join(sys.argv[1:]) + "\n")
    if os.environ.get("FAKE_MEMORY_FAIL") == "scan":
        print("fake scan-duplicates: failed", file=sys.stderr)
        return 1
    dupes = os.environ.get("FAKE_MEMORY_DUPES", "0")
    print(f"exact repeats across pages: {dupes}")
    return int(os.environ.get("FAKE_MEMORY_RC", "0"))


if __name__ == "__main__":
    sys.exit(main())
