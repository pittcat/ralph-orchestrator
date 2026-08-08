"""Small subprocess-shaped lifecycle proof for ordinary and worker sessions."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
HOOK = ROOT / "plugins/nowledge-mem-ralph/scripts/hook_runtime.py"
FIXTURES = ROOT / "plugins/nowledge-mem-ralph/tests/fixtures"


def _run(event: str, data: Path, env: dict[str, str], payload: dict):
    process_env = {"PATH": "/usr/bin:/bin", "PYTHONPATH": str(ROOT), **env}
    return subprocess.run(
        [sys.executable, str(HOOK), event],
        input=json.dumps(payload), text=True, capture_output=True,
        env=process_env, timeout=10,
    )


def test_worker_reuses_cache_and_can_audit_stop(tmp_path):
    data = tmp_path / "claude-plugin-data"
    base = {
        "CLAUDE_PLUGIN_DATA": str(data),
        "RALPH_NOWLEDGE_ENABLED": "1",
        "RALPH_CURRENT_LOOP_ID": "loop-e2e",
        "RALPH_CURRENT_HAT": "worker",
        "RALPH_HATS_SOURCE": "supervisor",
        "RALPH_WORKSPACE_ROOT": str(tmp_path / "repo"),
    }
    start = _run("SessionStart", data, base, {"session_id": "worker-1", "source": "startup"})
    assert start.returncode == 0
    stop = _run(
        "SubagentStop", data, base,
        {"session_id": "worker-1", "transcript_path": "/not-read"},
    )
    assert stop.returncode == 0
    audit = data / "loop-e2e" / "audit.jsonl"
    assert audit.is_file()
    record = json.loads(audit.read_text(encoding="utf-8").splitlines()[-1])
    assert record["event"] == "SubagentStop"
    assert record["transcript_read"] is False


def test_real_shape_hook_fixtures_are_safe():
    start = json.loads((FIXTURES / "claude-hooks/session-start.json").read_text())
    stop = json.loads((FIXTURES / "claude-hooks/stop-worker.json").read_text())
    assert start["source"] == "startup"
    assert "last_assistant_message" in stop
    context = json.loads((FIXTURES / "ralph/loop-context.json").read_text())
    assert context["RALPH_SESSION_ROLE"] == "worker"
    assert context["RALPH_CURRENT_LOOP_ID"]
