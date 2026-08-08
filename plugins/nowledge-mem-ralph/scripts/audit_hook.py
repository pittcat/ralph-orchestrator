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
    audit_only: bool | None = None,
    save_results: list[dict[str, Any]] | None = None,
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
    if save_results is None:
        save_results = _recent_save_results(directory / "memory-results.jsonl")
    record = {
        "event": event,
        "loop_id": loop_id,
        "hat": str(env.get("RALPH_CURRENT_HAT", "")),
        "session_id": str(payload.get("session_id", "")),
        "audit_only": bool(audit_only) if audit_only is not None else True,
        "transcript_read": False,
        "save_results": save_results,
    }
    state.update({
        "hook": event,
        "loop_id": loop_id,
        "hat": str(env.get("RALPH_CURRENT_HAT", "")),
        "audit_only": record["audit_only"],
        "stop_hook_fired": True,
    })
    record["audit_only"] = state["audit_only"]
    try:
        import fcntl as _fcntl
        import tempfile as _tempfile

        directory.mkdir(parents=True, exist_ok=True)
        lock = directory / "state.lock"
        with lock.open("a+") as lock_handle:
            try:
                _fcntl.flock(lock_handle.fileno(), _fcntl.LOCK_EX)
            except OSError:
                pass
            _atomic_write_json(state_path, state)
        record_serialized = json.dumps(record, ensure_ascii=True, sort_keys=True)
    except OSError:
        try:
            sys = __import__("sys")
            sys.stderr.write(
                json.dumps(
                    {"event": "audit_record_failed", "type": "OSError"}
                )
                + "\n"
            )
        except Exception:
            pass
        return
    audit_path = directory / "audit.jsonl"
    try:
        with audit_path.open("a", encoding="utf-8") as handle:
            handle.write(record_serialized + "\n")
    except OSError:
        try:
            sys = __import__("sys")
            sys.stderr.write(
                json.dumps(
                    {"event": "audit_record_failed", "type": "OSError"}
                )
                + "\n"
            )
        except Exception:
            pass


def _atomic_write_json(path: Path, payload: dict[str, Any]) -> None:
    """Write ``payload`` to ``path`` via tempfile.mkstemp + os.replace."""
    import os as _os
    import tempfile as _tempfile

    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = _tempfile.mkstemp(prefix="state-", dir=path.parent)
    try:
        with _os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(payload, handle, ensure_ascii=True, sort_keys=True)
        _os.replace(tmp_name, path)
    finally:
        try:
            _os.unlink(tmp_name)
        except FileNotFoundError:
            pass


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
