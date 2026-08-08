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
    env = {"RALPH_CURRENT_LOOP_ID": "loop-audit", "RALPH_CURRENT_HAT": "worker"}
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
