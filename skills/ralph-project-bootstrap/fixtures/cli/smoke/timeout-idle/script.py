#!/usr/bin/env python3
"""Fake ralph binary for the timeout-idle fixture.

Emits ``plan.ready`` once and then sleeps forever. The harness is
expected to classify the outcome as ``timeout_idle`` once the outer
timeout fires.
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
            handle.write("timeout-idle: spawn detected\n")
    sys.stdout.write("plan.ready\n")
    sys.stdout.flush()
    sleep_for = float(os.environ.get("SMOKE_HANG_SECONDS", "120"))
    time.sleep(sleep_for)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())