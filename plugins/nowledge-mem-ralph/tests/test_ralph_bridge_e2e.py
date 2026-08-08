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


# ---------------------------------------------------------------------------
# Unit 4 — end-to-end Stop / SubagentStop auto-finalization
#
# Subprocess tests exercise the real plugin entrypoint against a fake
# nmem runner; the test asserts the full chain (parser → save →
# writer → nmem) lands the audit block in state.json and the bounded
# result in memory-results.jsonl.
# ---------------------------------------------------------------------------


def _write_fake_nmem(bin_dir, response='{"id":"mem-1"}', exit_code=0):
    bin_dir.mkdir(parents=True, exist_ok=True)
    calls = bin_dir / "calls.jsonl"
    script = bin_dir / "nmem"
    calls_path = str(calls.resolve())
    script.write_text(
        "#!/usr/bin/env python3\n"
        "import json, sys\n"
        f"with open({calls_path!r}, 'a', encoding='utf-8') as _h:\n"
        "    _h.write(json.dumps(sys.argv[1:]) + '\\n')\n"
        "sys.stdout.write(" + repr(response) + " + '\\n')\n"
        f"sys.exit({exit_code})\n",
        encoding="utf-8",
    )
    script.chmod(0o755)
    return calls


def _run_hook(event, data, env, payload):
    process_env = {"PATH": "/usr/bin:/bin", "PYTHONPATH": str(ROOT), **env}
    return subprocess.run(
        [sys.executable, str(HOOK), event],
        input=json.dumps(payload), text=True, capture_output=True,
        env=process_env, timeout=10,
    )


def _candidate_marker(**overrides):
    candidate = {
        "memory_type": "durable_decision",
        "title": "Use atomic os.replace for state.json writes",
        "claim": "Atomic writes avoid torn state.",
        "why_it_matters": "Half-written files break env detection.",
        "evidence": "hooks/hooks.json timeout=5; writer test.",
        "applies_when": "any state.json write",
        "scope": "plugin:knowledge-mem-ralph",
        "verification": "pytest proves no torn writes.",
        "critical_assumptions": [],
        "critical_ambiguities": [],
        "metrics": {
            "confidence": 95,
            "evidence_coverage": 88,
            "reusability": 90,
            "stability": 92,
            "scope_clarity": 96,
            "verifiability": 90,
            "novelty": 40,
        },
        "finalize": True,
    }
    candidate.update(overrides)
    return "<!-- nowledge-memory-finalize\n" + json.dumps(candidate, ensure_ascii=False) + "\n-->"


def _base_env(tmp_path, bin_dir, *, loop_id="loop-e2e", hat="executor"):
    data = tmp_path / "data"
    return {
        "CLAUDE_PLUGIN_DATA": str(data),
        "RALPH_NOWLEDGE_ENABLED": "1",
        "RALPH_CURRENT_LOOP_ID": loop_id,
        "RALPH_CURRENT_HAT": hat,
        "RALPH_HATS_SOURCE": "supervisor",
        "RALPH_WORKSPACE_ROOT": str(tmp_path / "repo"),
        "PATH": f"{bin_dir}:/usr/bin:/bin",
    }


def test_stop_e2e_finalization(tmp_path):
    """Stop with a valid finalize:true marker produces a SAVED audit block."""
    bin_dir = tmp_path / "bin"
    calls = _write_fake_nmem(bin_dir)
    env = _base_env(tmp_path, bin_dir)
    marker = _candidate_marker()
    res = _run_hook(
        "Stop",
        env["CLAUDE_PLUGIN_DATA"],
        env,
        {
            "session_id": "ralph-session-1",
            "transcript_path": "/outside/plugin/transcript.jsonl",
            "last_assistant_message": f"Lesson:\n\n{marker}\n",
        },
    )
    assert res.returncode == 0, f"stderr={res.stderr}"
    state = json.loads(
        (Path(env["CLAUDE_PLUGIN_DATA"]) / "loop-e2e" / "state.json").read_text(encoding="utf-8")
    )
    assert state["hook"] == "Stop"
    assert state["finalization"]["status"] == "SAVED"
    call_lines = calls.read_text(encoding="utf-8").splitlines()
    assert len(call_lines) == 1, f"expected one nmem call, got {call_lines}"


def test_subagent_stop_e2e_finalization(tmp_path):
    """SubagentStop mirrors Stop: same writer chain, same audit."""
    bin_dir = tmp_path / "bin"
    calls = _write_fake_nmem(bin_dir)
    env = _base_env(tmp_path, bin_dir, hat="executor")
    marker = _candidate_marker()
    res = _run_hook(
        "SubagentStop",
        env["CLAUDE_PLUGIN_DATA"],
        env,
        {
            "session_id": "ralph-worker-1",
            "agent_id": "worker-1",
            "transcript_path": "/outside/plugin/worker-transcript.jsonl",
            "last_assistant_message": f"Lesson:\n\n{marker}\n",
        },
    )
    assert res.returncode == 0, f"stderr={res.stderr}"
    state = json.loads(
        (Path(env["CLAUDE_PLUGIN_DATA"]) / "loop-e2e" / "state.json").read_text(encoding="utf-8")
    )
    assert state["hook"] == "SubagentStop"
    assert state["finalization"]["status"] == "SAVED"
    assert len(calls.read_text(encoding="utf-8").splitlines()) == 1


def test_duplicate_cross_event_idempotency(tmp_path):
    """Same digest across Stop then SubagentStop must only call nmem once."""
    bin_dir = tmp_path / "bin"
    calls = _write_fake_nmem(bin_dir)
    env = _base_env(tmp_path, bin_dir)
    marker = _candidate_marker()
    payload = {
        "session_id": "ralph-session-1",
        "last_assistant_message": f"Lesson:\n\n{marker}\n",
    }
    stop = _run_hook("Stop", env["CLAUDE_PLUGIN_DATA"], env, payload)
    assert stop.returncode == 0
    sub = _run_hook("SubagentStop", env["CLAUDE_PLUGIN_DATA"], env, payload)
    assert sub.returncode == 0
    call_lines = calls.read_text(encoding="utf-8").splitlines()
    assert len(call_lines) == 1, (
        f"duplicate digest across events must not invoke nmem twice, got {call_lines}"
    )


def test_no_ralph_env_e2e_noop(tmp_path):
    """Without Ralph env, Stop must exit 0 and write nothing."""
    bin_dir = tmp_path / "bin"
    data = tmp_path / "data"
    data.mkdir(parents=True, exist_ok=True)
    env = {
        "CLAUDE_PLUGIN_DATA": str(data),
        "PATH": f"{bin_dir}:/usr/bin:/bin",
    }
    marker = _candidate_marker()
    res = _run_hook(
        "Stop",
        str(data),
        env,
        {
            "session_id": "human-session",
            "last_assistant_message": f"Lesson:\n\n{marker}\n",
        },
    )
    assert res.returncode == 0
    assert res.stdout == ""
    assert list(data.iterdir()) == [], "no plugin state must be written"


def test_e2e_transcript_path_safety(tmp_path):
    """``transcript_path`` must NEVER appear in any plugin state file."""
    bin_dir = tmp_path / "bin"
    _write_fake_nmem(bin_dir)
    env = _base_env(tmp_path, bin_dir)
    marker = _candidate_marker()
    res = _run_hook(
        "Stop",
        env["CLAUDE_PLUGIN_DATA"],
        env,
        {
            "session_id": "ralph-session-1",
            "transcript_path": "/etc/passwd",
            "last_assistant_message": f"Lesson:\n\n{marker}\n",
        },
    )
    assert res.returncode == 0
    state_path = Path(env["CLAUDE_PLUGIN_DATA"]) / "loop-e2e" / "state.json"
    audit_path = Path(env["CLAUDE_PLUGIN_DATA"]) / "loop-e2e" / "audit.jsonl"
    for path in (state_path, audit_path):
        if path.exists():
            text = path.read_text(encoding="utf-8")
            assert "/etc/passwd" not in text, (
                f"{path} must not contain transcript_path"
            )
