"""U05 audit tests prove Stop never consumes transcript input."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "plugins/nowledge-mem-ralph/scripts/audit_hook.py"


def _module():
    spec = importlib.util.spec_from_file_location("_audit_test", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["_audit_test"] = module
    spec.loader.exec_module(module)
    return module


def test_stop_and_subagent_stop_append_audit_without_transcript(tmp_path):
    audit = _module()
    env = {"RALPH_CURRENT_LOOP_ID": "loop-audit", "RALPH_CURRENT_HAT": "executor"}
    payload = {
        "session_id": "sess-1",
        "transcript_path": str(tmp_path / "secret-transcript"),
        "last_assistant_message": "secret message that must not be copied",
    }
    audit.record_stop(env, payload, event="Stop", state_root=tmp_path)
    audit.record_stop(env, payload, event="SubagentStop", state_root=tmp_path)
    lines = (tmp_path / "loop-audit" / "audit.jsonl").read_text(encoding="utf-8").splitlines()
    records = [json.loads(line) for line in lines]
    assert [record["event"] for record in records] == ["Stop", "SubagentStop"]
    serialized = json.dumps(records)
    assert "secret message" not in serialized
    assert "secret-transcript" not in serialized
    assert all(record["transcript_read"] is False for record in records)


def test_audit_hook_survives_surrogate_session_id(tmp_path):
    """A lone-surrogate session_id must not raise inside record_stop."""
    audit = _module()
    env = {"RALPH_CURRENT_LOOP_ID": "loop-sur", "RALPH_CURRENT_HAT": "executor"}
    payload = {
        "session_id": "s\ud800",  # lone surrogate
        "transcript_path": str(tmp_path / "transcript"),
    }
    # Must not raise — the audit row lands and the session_id is
    # sanitised to a placeholder so the JSONL stays ASCII-safe.
    audit.record_stop(env, payload, event="Stop", state_root=tmp_path)
    audit_lines = (tmp_path / "loop-sur" / "audit.jsonl").read_text(
        encoding="utf-8"
    ).splitlines()
    assert audit_lines, "audit row must still land"
    record = json.loads(audit_lines[-1])
    assert record["event"] == "Stop"
    assert "\ud800" not in audit_lines[-1], (
        "audit row must not embed the raw surrogate — "
        "ensure_ascii=True must coerce or drop the lone surrogate"
    )
