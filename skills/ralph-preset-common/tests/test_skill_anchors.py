#!/usr/bin/env python3
"""2026-07-27-002 plan Unit 4: anchor contract tests for preset-author / preset-review SKILLs."""
import re
import sys
from pathlib import Path

# Anchor list shared by Unit 4. Tests pin these strings exist exactly once in
# the corresponding doc.
ANCHORS = [
    ("skills/ralph-preset-author/SKILL.md", "Capability discovery"),
    ("skills/ralph-preset-review/SKILL.md", "Capability-triggered audit"),
    ("skills/ralph-preset-common/references/commands.md", "Capability inventory"),
    ("skills/ralph-preset-common/references/agent-native-model.md", "Runtime Audit Model"),
]

def test_anchor_present(path: str, anchor: str) -> bool:
    full = Path(__file__).resolve().parent.parent.parent.parent / path
    if not full.exists():
        print(f"MISSING file: {full}")
        return False
    content = full.read_text(encoding="utf-8")
    if anchor in content:
        print(f"OK anchor {anchor!r} in {path}")
        return True
    print(f"FAIL anchor {anchor!r} not found in {path}")
    return False

if __name__ == "__main__":
    ok = True
    for path, anchor in ANCHORS:
        ok = test_anchor_present(path, anchor) and ok
    sys.exit(0 if ok else 1)
