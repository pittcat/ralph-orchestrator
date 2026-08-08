"""Audit-only Stop/SubagentStop handler.

The hook records lifecycle metadata and the last known writer status from the
plugin state directory.  It deliberately never opens ``transcript_path`` or
uses ``last_assistant_message``; a stopped agent cannot be asked to submit a
new Memory safely.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any, Mapping


def record_stop(
    env: Mapping[str, str],
    payload: Mapping[str, Any],
    *,
    event: str,
    state_root: Path,
) -> None:
    loop_id = str(env.get("RALPH_CURRENT_LOOP_ID", "")).strip()
    if not loop_id:
        return
    directory = state_root / loop_id
    directory.mkdir(parents=True, exist_ok=True)
    state_path = directory / "state.json"
    state: dict[str, Any] = {}
    try:
        loaded = json.loads(state_path.read_text(encoding="utf-8"))
        if isinstance(loaded, dict):
            state.update(loaded)
    except (OSError, json.JSONDecodeError):
        pass
    record = {
        "event": event,
        "loop_id": loop_id,
        "hat": str(env.get("RALPH_CURRENT_HAT", "")),
        "session_id": str(payload.get("session_id", "")),
        "audit_only": True,
        "transcript_read": False,
        "save_results": _recent_save_results(directory / "memory-results.jsonl"),
    }
    state.update({
        "hook": event,
        "loop_id": loop_id,
        "hat": str(env.get("RALPH_CURRENT_HAT", "")),
        "audit_only": True,
        "stop_hook_fired": True,
    })
    tmp = state_path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(state, ensure_ascii=False, sort_keys=True), encoding="utf-8")
    os.replace(tmp, state_path)
    audit_path = directory / "audit.jsonl"
    with audit_path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(record, ensure_ascii=False, sort_keys=True) + "\n")


def _recent_save_results(path: Path) -> list[dict[str, Any]]:
    """Read only plugin-owned bounded result records, never hook input files."""
    try:
        lines = path.read_text(encoding="utf-8").splitlines()[-10:]
    except OSError:
        return []
    results: list[dict[str, Any]] = []
    for line in lines:
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            results.append({
                key: value[key]
                for key in ("result", "reason", "memory_digest", "memory_id", "retryable")
                if key in value
            })
    return results
