#!/usr/bin/env python3
"""Fake ralph binary for the timeout-no-event fixture.

Writes no events to stdout and sleeps for a long time. The harness is
expected to kill the subprocess via its outer timeout and classify the
outcome as ``timeout_no_event``.

The script honours ``SMOKE_WALL_CLOCK_TIMEOUT_S`` so the test can drive
the fixture with a small wall-clock cap and keep CI fast.
"""
from __future__ import annotations

import os
import sys
import time


def main() -> int:
    transcript_dir = os.environ.get("TRANSCRIPT_DIR", "")
    if transcript_dir:
        os.makedirs(transcript_dir, exist_ok=True)
        with open(
            os.path.join(transcript_dir, "spawned.txt"), "w", encoding="utf-8"
        ) as handle:
            handle.write("timeout-no-event: spawn detected\n")
    # Default to 120s for safety; tests will cap the outer timeout.
    sleep_for = float(os.environ.get("SMOKE_HANG_SECONDS", "120"))
    time.sleep(sleep_for)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())