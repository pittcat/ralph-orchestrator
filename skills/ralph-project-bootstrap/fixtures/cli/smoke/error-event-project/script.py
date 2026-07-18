#!/usr/bin/env python3
"""Fake ralph binary for the error-event-project fixture.

Emits ``plan.ready`` followed by an ``ERROR_EVENT:`` line whose text
includes the substring ``project``. The harness is expected to
classify the outcome as ``error_event_detected`` with
``failure_bucket="project_command"``.
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
            handle.write("error-event-project: spawn detected\n")
    sys.stdout.write("plan.ready\n")
    sys.stdout.write("ERROR_EVENT: project build command not found\n")
    sys.stdout.flush()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())