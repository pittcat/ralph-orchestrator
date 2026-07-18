#!/usr/bin/env python3
"""Fake ralph binary for the non-zero-exit fixture.

Exits with code 1 and writes a short stderr line. The harness is
expected to classify the outcome as ``non_zero_exit`` with a
``failure_bucket`` resolved from the stderr text.
"""
from __future__ import annotations

import os
import sys


def main() -> int:
    transcript_dir = os.environ.get("TRANSCRIPT_DIR", "")
    if transcript_dir:
        os.makedirs(transcript_dir, exist_ok=True)
        with open(
            os.path.join(transcript_dir, "spawned.txt"), "w", encoding="utf-8"
        ) as handle:
            handle.write("non-zero-exit: spawn detected\n")
    sys.stderr.write("synthetic: script exits 1\n")
    sys.stderr.flush()
    return 1


if __name__ == "__main__":
    raise SystemExit(main())