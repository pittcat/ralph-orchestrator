#!/usr/bin/env python3
"""Fake ralph binary for the safe-smoke green-path fixture.

Emits ``plan.ready`` then ``LOOP_COMPLETE`` to stdout and exits zero.
The harness is expected to classify the outcome as
``bounded_terminal_reached``.

Args:
  argv is ignored except for diagnostic echo; the script writes the
  captured argv to ``$TRANSCRIPT_DIR/argv.json`` so tests can assert
  the argv-shape contract.
"""
from __future__ import annotations

import json
import os
import sys


def main() -> int:
    transcript_dir = os.environ.get("TRANSCRIPT_DIR", "")
    if transcript_dir:
        os.makedirs(transcript_dir, exist_ok=True)
        argv_path = os.path.join(transcript_dir, "argv.json")
        with open(argv_path, "w", encoding="utf-8") as handle:
            json.dump({"argv": list(sys.argv[1:])}, handle)
        events_path = os.path.join(transcript_dir, "events.jsonl")
        with open(events_path, "w", encoding="utf-8") as handle:
            handle.write("plan.ready\n")
            handle.write("executing unit\n")
            handle.write("all checks passed\n")
            handle.write("LOOP_COMPLETE\n")
    sys.stdout.write("plan.ready\n")
    sys.stdout.write("executing unit\n")
    sys.stdout.write("all checks passed\n")
    sys.stdout.write("LOOP_COMPLETE\n")
    sys.stdout.flush()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())