#!/usr/bin/env python3
"""Fake ralph binary for the unsafe-real fixture.

This fixture is exercised via ``UnsafeBackend(kind="real")``. The
harness MUST refuse before any subprocess is constructed; this script
is therefore never executed in a passing test. The script writes a
"would have spawned" record to ``$TRANSCRIPT_DIR`` so a buggy harness
that DOES spawn is immediately visible.
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
            handle.write("unsafe-real: harness spawned when it should not have\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())