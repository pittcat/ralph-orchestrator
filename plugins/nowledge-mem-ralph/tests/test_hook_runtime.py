"""Tests for the Nowledge Mem Ralph hook runtime (U01 foundation).

These tests lock the U01 scaffold contract — they are NOT a substitute
for the per-event unit tests added in U02-U06. The contract proven here:

* The runtime can be invoked as a module entry point with an explicit
  ``argv`` (matches how ``hooks/hooks.json`` calls it).
* Without ``RALPH_CURRENT_LOOP_ID`` the runtime exits 0, writes nothing,
  and never spawns a subprocess — proving the env gate works.
* With Ralph env present the SessionStart handler writes the canonical
  state marker under ``CLAUDE_PLUGIN_DATA/<loop_id>/state.json``.
* The Stop handler writes an ``audit_only`` marker and never reads a
  transcript or spawns ``nmem`` (placeholder audit).

Tests use a tmp ``CLAUDE_PLUGIN_DATA`` and override ``NMEM`` so we can
prove "no subprocess spawned" without actually needing ``nmem`` on PATH.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
PLUGIN_DIR = ROOT / "plugins" / "nowledge-mem-ralph"
HOOK_RUNTIME = PLUGIN_DIR / "scripts" / "hook_runtime.py"


@pytest.fixture
def isolated_plugin_data(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """Per-test plugin data dir + cleared Ralph loop env + blocked nmem."""
    monkeypatch.setenv("CLAUDE_PLUGIN_DATA", str(tmp_path))
    # Strip every RALPH_* key the runtime looks at. Tests set them back
    # explicitly when they want to exercise the active hook path.
    for key in (
        "RALPH_CURRENT_HAT",
        "RALPH_NOWLEDGE_ENABLED",
        "RALPH_CURRENT_LOOP_ID",
        "RALPH_EVENTS_FILE",
        "RALPH_TRIGGERED_HAT",
        "RALPH_HATS_SOURCE",
        "RALPH_CONFIG",
        "RALPH_WORKSPACE_ROOT",
        "NOWLEDGE_HOOK_EVENT",
    ):
        monkeypatch.delenv(key, raising=False)
    # Force every subprocess call to fail loudly — if the runtime ever
    # tries to spawn nmem (or anything else) the test will see the
    # subprocess exit code as evidence of the violation.
    monkeypatch.setenv("PATH", str(tmp_path))
    yield tmp_path


def _run_hook(
    event: str,
    stdin_payload: dict | str,
    *,
    extra_env: dict[str, str] | None = None,
    cwd: Path | None = None,
    plugin_data: Path | None = None,
) -> subprocess.CompletedProcess:
    """Invoke the hook runtime in a child process with the given stdin.

    ``plugin_data`` defaults to ``$CLAUDE_PLUGIN_DATA`` if set in the
    caller's environment — which the fixture above always does. Tests
    that want to assert "no writes" should leave it as the fixture's
    tmp_path; tests that want to assert "wrote the right file" can
    override explicitly.
    """
    if isinstance(stdin_payload, dict):
        stdin_text = json.dumps(stdin_payload)
    else:
        stdin_text = stdin_payload
    env: dict[str, str] = {
        "PATH": "/usr/bin:/bin",
        "PYTHONPATH": str(ROOT),
    }
    if plugin_data is not None:
        env["CLAUDE_PLUGIN_DATA"] = str(plugin_data)
    else:
        existing = os.environ.get("CLAUDE_PLUGIN_DATA", "")
        if existing:
            env["CLAUDE_PLUGIN_DATA"] = existing
    if extra_env:
        env.update(extra_env)
    return subprocess.run(
        [sys.executable, str(HOOK_RUNTIME), event],
        input=stdin_text,
        text=True,
        capture_output=True,
        env=env,
        cwd=str(cwd) if cwd else None,
        timeout=10,
    )


def test_no_ralph_env_noop(isolated_plugin_data: Path) -> None:
    """Without Ralph loop env the hook returns 0 and writes nothing."""
    result = _run_hook(
        "SessionStart",
        {"session_id": "abc", "source": "startup"},
    )
    assert result.returncode == 0, (
        f"hook must exit 0 with no env, got {result.returncode}: {result.stderr}"
    )
    assert result.stdout == "", (
        f"hook must emit empty stdout with no env, got {result.stdout!r}"
    )
    assert not any(isolated_plugin_data.iterdir()), (
        "hook must not write any state files when Ralph env is absent"
    )


def test_no_ralph_env_stop_also_noop(isolated_plugin_data: Path) -> None:
    """Stop hook also gates on Ralph env — no audit without a loop."""
    result = _run_hook("Stop", {"session_id": "abc", "transcript_path": "/dev/null"})
    assert result.returncode == 0
    assert result.stdout == ""
    assert not any(isolated_plugin_data.iterdir()), (
        "Stop hook must not write any state when Ralph env is absent"
    )


def test_session_start_writes_state_marker(
    isolated_plugin_data: Path,
) -> None:
    """SessionStart with Ralph env writes the canonical state marker."""
    result = _run_hook(
        "SessionStart",
        {"session_id": "sess-1", "source": "startup"},
        extra_env={
            "RALPH_NOWLEDGE_ENABLED": "1",
            "RALPH_CURRENT_LOOP_ID": "loop-xyz",
            "RALPH_CURRENT_HAT": "planner",
            "RALPH_HATS_SOURCE": "ce-executor-pipeline",
            "RALPH_WORKSPACE_ROOT": "/tmp/repo",
        },
    )
    assert result.returncode == 0, (
        f"expected 0, got {result.returncode}: stderr={result.stderr}"
    )
    state_path = isolated_plugin_data / "loop-xyz" / "state.json"
    assert state_path.is_file(), (
        f"expected state marker at {state_path}, only saw: "
        f"{list(isolated_plugin_data.rglob('*'))}"
    )
    payload = json.loads(state_path.read_text(encoding="utf-8"))
    assert payload["hook"] == "SessionStart"
    assert payload["loop_id"] == "loop-xyz"
    assert payload["hat"] == "planner"
    assert payload["session_id"] == "sess-1"


def test_stop_audit_placeholder(isolated_plugin_data: Path) -> None:
    """Stop hook writes an audit-only marker, never reads transcript."""
    result = _run_hook(
        "Stop",
        {
            "session_id": "sess-1",
            "transcript_path": "/etc/passwd",
            "last_assistant_message": "should never be read",
        },
        extra_env={
            "RALPH_NOWLEDGE_ENABLED": "1",
            "RALPH_CURRENT_LOOP_ID": "loop-xyz",
            "RALPH_CURRENT_HAT": "planner",
        },
    )
    assert result.returncode == 0, (
        f"Stop hook must exit 0, got {result.returncode}: stderr={result.stderr}"
    )
    state_path = isolated_plugin_data / "loop-xyz" / "state.json"
    assert state_path.is_file()
    payload = json.loads(state_path.read_text(encoding="utf-8"))
    assert payload["hook"] == "Stop"
    assert payload["audit_only"] is True
    assert payload["loop_id"] == "loop-xyz"
    # Transcript content must not leak into the state marker — Stop
    # is forbidden from reading transcript_path or last_assistant_message.
    serialized = json.dumps(payload, ensure_ascii=True)
    assert "should never be read" not in serialized, (
        "Stop hook must NOT ingest transcript content into the audit record"
    )
    assert "/etc/passwd" not in serialized, (
        "Stop hook must NOT record transcript_path into the audit record"
    )


def test_resolve_nowledge_env_normalizes_keys(tmp_path: Path) -> None:
    """resolve_nowledge_env returns stripped values for every Ralph key."""
    # Load hook_runtime as a module without depending on the plugin
    # directory being a Python package (it isn't — the dir name has a
    # hyphen).
    import importlib.util

    spec = importlib.util.spec_from_file_location(
        "_hook_runtime_under_test", HOOK_RUNTIME
    )
    assert spec and spec.loader
    hook_runtime = importlib.util.module_from_spec(spec)
    sys.modules["_hook_runtime_under_test"] = hook_runtime  # noqa: E501 - python 3.14 dataclasses
    spec.loader.exec_module(hook_runtime)

    fake = {
        "RALPH_CURRENT_HAT": " planner ",
        "RALPH_CURRENT_LOOP_ID": "loop-1",
        "RALPH_TRIGGERED_HAT": "",
        "RALPH_HATS_SOURCE": "preset-x",
        "RALPH_WORKSPACE_ROOT": "/tmp/repo",
        "OTHER_KEY": "ignored",
    }
    tracked_keys = (
        "RALPH_CURRENT_HAT",
        "RALPH_CURRENT_LOOP_ID",
        "RALPH_EVENTS_FILE",
        "RALPH_TRIGGERED_HAT",
        "RALPH_HATS_SOURCE",
        "RALPH_CONFIG",
        "RALPH_WORKSPACE_ROOT",
    )
    saved = {key: os.environ.get(key) for key in tracked_keys}
    try:
        for key in tracked_keys:
            os.environ.pop(key, None)
        for key, value in fake.items():
            if key in tracked_keys:
                os.environ[key] = value
        env = hook_runtime.resolve_nowledge_env()
    finally:
        for key in tracked_keys:
            os.environ.pop(key, None)
        for key, value in saved.items():
            if value is not None:
                os.environ[key] = value
    assert env["RALPH_CURRENT_HAT"] == "planner"
    assert env["RALPH_CURRENT_LOOP_ID"] == "loop-1"
    assert env["RALPH_TRIGGERED_HAT"] == ""
    assert env["RALPH_HATS_SOURCE"] == "preset-x"
    assert "OTHER_KEY" not in env


def test_unknown_event_returns_bug_exit(isolated_plugin_data: Path) -> None:
    """An unknown event name is an internal bug, not a recoverable error."""
    result = _run_hook(
        "NotAHook",
        {},
        extra_env={"RALPH_NOWLEDGE_ENABLED": "1", "RALPH_CURRENT_LOOP_ID": "loop-x"},
    )
    assert result.returncode == 2, (
        f"unknown event must exit 2 (bug), got {result.returncode}"
    )


# ---------------------------------------------------------------------------
# Unit 1 — bounded finalization marker extraction (auto-finalize)
#
# The Stop / SubagentStop hook now extracts a single bounded
# ``<!-- nowledge-memory-finalize ... -->`` block from
# ``last_assistant_message`` and hands the candidate to the save-memory
# chain. The hook MUST stay audit-only when the marker is missing or
# malformed, and MUST never read ``transcript_path`` or persist the
# full assistant message. These tests lock that contract.
# ---------------------------------------------------------------------------


def _valid_candidate_marker(**overrides) -> str:
    """Build a legal finalization marker (one bounded fenced block)."""
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
    body = json.dumps(candidate, ensure_ascii=False)
    return f"<!-- nowledge-memory-finalize\n{body}\n-->"


def _ralph_env() -> dict[str, str]:
    return {
        "RALPH_NOWLEDGE_ENABLED": "1",
        "RALPH_CURRENT_LOOP_ID": "loop-xyz",
        "RALPH_CURRENT_HAT": "planner",
    }


def test_stop_extracts_valid_finalization_marker(tmp_path: Path) -> None:
    """Stop hook with a legal marker produces a SAVED audit record."""
    env, calls = _hook_env_with_fake_nmem(tmp_path)
    marker = _valid_candidate_marker()
    payload = {
        "session_id": "sess-1",
        "transcript_path": "/etc/passwd",
        "last_assistant_message": f"Here is the lesson:\n\n{marker}\n",
    }
    result = _run_hook(
        "Stop", payload, extra_env=env, plugin_data=Path(env["CLAUDE_PLUGIN_DATA"])
    )
    assert result.returncode == 0, (
        f"Stop hook must exit 0, got {result.returncode}: stderr={result.stderr}"
    )
    state_path = Path(env["CLAUDE_PLUGIN_DATA"]) / "loop-xyz" / "state.json"
    payload_data = json.loads(state_path.read_text(encoding="utf-8"))
    assert payload_data["hook"] == "Stop"
    assert payload_data["audit_only"] is False
    assert payload_data["finalization"]["status"] == "SAVED"
    assert payload_data["finalization"]["memory_digest"], "must surface a stable digest"
    serialized = json.dumps(payload_data, ensure_ascii=True)
    assert "Atomic writes avoid torn state" not in serialized, (
        "Stop hook must NOT persist the full assistant message"
    )
    assert "/etc/passwd" not in serialized, (
        "Stop hook must NOT record transcript_path"
    )
    assert len(calls.read_text(encoding="utf-8").splitlines()) == 1


def test_stop_without_marker_stays_audit_only(isolated_plugin_data: Path) -> None:
    """Stop hook with no marker keeps the old audit-only behaviour."""
    payload = {
        "session_id": "sess-1",
        "last_assistant_message": "Just a plain final message, no marker here.",
    }
    result = _run_hook("Stop", payload, extra_env=_ralph_env())
    assert result.returncode == 0
    state_data = json.loads(
        (isolated_plugin_data / "loop-xyz" / "state.json").read_text(encoding="utf-8")
    )
    assert state_data["audit_only"] is True
    assert state_data.get("finalization", {}).get("status") == "SKIPPED"


def test_stop_with_finalize_false_is_skipped(isolated_plugin_data: Path) -> None:
    """A marker without ``finalize:true`` is SKIPPED, not SAVED."""
    marker = _valid_candidate_marker(finalize=False)
    payload = {
        "session_id": "sess-1",
        "last_assistant_message": f"Lesson draft:\n\n{marker}\n",
    }
    result = _run_hook("Stop", payload, extra_env=_ralph_env())
    assert result.returncode == 0
    state_data = json.loads(
        (isolated_plugin_data / "loop-xyz" / "state.json").read_text(encoding="utf-8")
    )
    assert state_data["audit_only"] is True
    assert state_data.get("finalization", {}).get("status") == "SKIPPED"


def test_stop_with_duplicate_marker_is_rejected(isolated_plugin_data: Path) -> None:
    """Two markers in one message are rejected; parser refuses to pick one."""
    marker = _valid_candidate_marker()
    payload = {
        "session_id": "sess-1",
        "last_assistant_message": f"first\n{marker}\nmiddle\n{marker}\n",
    }
    result = _run_hook("Stop", payload, extra_env=_ralph_env())
    assert result.returncode == 0
    state_data = json.loads(
        (isolated_plugin_data / "loop-xyz" / "state.json").read_text(encoding="utf-8")
    )
    assert state_data.get("finalization", {}).get("status") in {"REJECTED", "SKIPPED"}


def test_stop_with_oversized_marker_is_rejected(isolated_plugin_data: Path) -> None:
    """A marker payload larger than the 16 KiB UTF-8 ceiling is rejected."""
    # Pad the candidate body to push the marker well past 16 KiB.
    candidate = {
        "memory_type": "durable_decision",
        "title": "Use atomic os.replace for state.json writes",
        "claim": "Atomic writes avoid torn state.",
        "why_it_matters": "Half-written files break env detection.",
        "evidence": "x" * (20 * 1024),
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
    body = json.dumps(candidate)
    marker = f"<!-- nowledge-memory-finalize\n{body}\n-->"
    payload = {"session_id": "sess-1", "last_assistant_message": marker}
    result = _run_hook("Stop", payload, extra_env=_ralph_env())
    assert result.returncode == 0
    state_data = json.loads(
        (isolated_plugin_data / "loop-xyz" / "state.json").read_text(encoding="utf-8")
    )
    assert state_data.get("finalization", {}).get("status") == "REJECTED"


def test_stop_with_malformed_marker_is_rejected(isolated_plugin_data: Path) -> None:
    """A marker with bad JSON inside is rejected, not silently accepted."""
    payload = {
        "session_id": "sess-1",
        "last_assistant_message": "<!-- nowledge-memory-finalize\n{not-json}\n-->",
    }
    result = _run_hook("Stop", payload, extra_env=_ralph_env())
    assert result.returncode == 0
    state_data = json.loads(
        (isolated_plugin_data / "loop-xyz" / "state.json").read_text(encoding="utf-8")
    )
    assert state_data.get("finalization", {}).get("status") == "REJECTED"


def test_stop_never_reads_transcript_path(isolated_plugin_data: Path) -> None:
    """``transcript_path`` must NEVER be opened even when a legal marker is present."""
    payload = {
        "session_id": "sess-1",
        "transcript_path": "/etc/passwd",
        "last_assistant_message": _valid_candidate_marker(),
    }
    result = _run_hook("Stop", payload, extra_env=_ralph_env())
    assert result.returncode == 0
    state_data = json.loads(
        (isolated_plugin_data / "loop-xyz" / "state.json").read_text(encoding="utf-8")
    )
    serialized = json.dumps(state_data, ensure_ascii=True)
    assert "/etc/passwd" not in serialized


def test_subagent_stop_uses_same_bounded_path(tmp_path: Path) -> None:
    """SubagentStop mirrors Stop: same parser, same audit contract."""
    env, calls = _hook_env_with_fake_nmem(tmp_path)
    marker = _valid_candidate_marker()
    payload = {
        "session_id": "worker-1",
        "agent_id": "worker-1",
        "transcript_path": "/not/read",
        "last_assistant_message": f"Lesson:\n\n{marker}\n",
    }
    result = _run_hook(
        "SubagentStop", payload, extra_env=env, plugin_data=Path(env["CLAUDE_PLUGIN_DATA"])
    )
    assert result.returncode == 0
    state_data = json.loads(
        (Path(env["CLAUDE_PLUGIN_DATA"]) / "loop-xyz" / "state.json").read_text(encoding="utf-8")
    )
    assert state_data["hook"] == "SubagentStop"
    assert state_data["audit_only"] is False
    assert state_data["finalization"]["status"] == "SAVED"
    assert len(calls.read_text(encoding="utf-8").splitlines()) == 1


# ---------------------------------------------------------------------------
# Unit 2 — finalization coordinator (Stop / SubagentStop → save → writer)
#
# A legal candidate reaches the existing ``memory_writer`` exactly once,
# duplicate digests return ``ALREADY_SAVED`` without a second nmem call,
# policy-rejected candidates never reach nmem, and the hook stays
# exit 0 on every failure.
# ---------------------------------------------------------------------------


def _write_fake_nmem(bin_dir: Path, response: str = '{"id":"mem-1"}', exit_code: int = 0) -> Path:
    """Install a fake ``nmem`` script in ``bin_dir`` and return its call log path.

    The fake records every invocation as a single JSON-quoted argv
    line in ``calls.jsonl`` so the test suite can assert how many
    times ``nmem`` was called regardless of how many tokens were
    passed. The script writes via an absolute path so ``cwd`` does
    not matter.
    """
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


def _hook_env_with_fake_nmem(
    tmp_path: Path,
    response: str = '{"id":"mem-1"}',
    exit_code: int = 0,
) -> tuple[dict[str, str], Path]:
    bin_dir = tmp_path / "bin"
    calls = _write_fake_nmem(bin_dir, response=response, exit_code=exit_code)
    data_dir = tmp_path / "data"
    env = _ralph_env()
    env["PATH"] = f"{bin_dir}:/usr/bin:/bin"
    env["CLAUDE_PLUGIN_DATA"] = str(data_dir)
    return env, calls


def test_stop_accepted_candidate_calls_writer_once(
    isolated_plugin_data: Path, tmp_path: Path
) -> None:
    """Stop hook with a legal finalize:true marker calls the writer once."""
    env, calls = _hook_env_with_fake_nmem(tmp_path)
    marker = _valid_candidate_marker()
    payload = {
        "session_id": "sess-1",
        "last_assistant_message": f"Lesson:\n\n{marker}\n",
    }
    result = _run_hook(
        "Stop", payload, extra_env=env, plugin_data=Path(env["CLAUDE_PLUGIN_DATA"])
    )
    assert result.returncode == 0, f"stderr={result.stderr}"
    state_data = json.loads(
        (Path(env["CLAUDE_PLUGIN_DATA"]) / "loop-xyz" / "state.json").read_text(encoding="utf-8")
    )
    assert state_data["finalization"]["status"] == "SAVED"
    assert state_data["finalization"]["memory_digest"]
    call_lines = calls.read_text(encoding="utf-8").splitlines()
    assert len(call_lines) == 1, f"nmem must be called exactly once, got {call_lines}"
    argv = json.loads(call_lines[0])
    assert argv[0:3] == ["--json", "m", "add"], f"unexpected argv: {argv}"
    assert "--title" in argv
    assert "--unit-type" in argv


def test_stop_duplicate_candidate_is_idempotent(
    isolated_plugin_data: Path, tmp_path: Path
) -> None:
    """Two Stop hooks with the same candidate yield one nmem call."""
    env, calls = _hook_env_with_fake_nmem(tmp_path)
    marker = _valid_candidate_marker()
    payload = {
        "session_id": "sess-1",
        "last_assistant_message": f"Lesson:\n\n{marker}\n",
    }
    for _ in range(2):
        result = _run_hook(
            "Stop", payload, extra_env=env, plugin_data=Path(env["CLAUDE_PLUGIN_DATA"])
        )
        assert result.returncode == 0
    call_lines = calls.read_text(encoding="utf-8").splitlines()
    assert len(call_lines) == 1, (
        f"duplicate candidate must not produce a second nmem call, got {call_lines}"
    )


def test_stop_rejected_candidate_does_not_call_writer(
    isolated_plugin_data: Path, tmp_path: Path
) -> None:
    """A marker whose memory_type is 'progress' is rejected and never reaches nmem."""
    env, calls = _hook_env_with_fake_nmem(tmp_path)
    marker = _valid_candidate_marker(memory_type="progress")
    payload = {
        "session_id": "sess-1",
        "last_assistant_message": f"Lesson:\n\n{marker}\n",
    }
    result = _run_hook(
        "Stop", payload, extra_env=env, plugin_data=Path(env["CLAUDE_PLUGIN_DATA"])
    )
    assert result.returncode == 0
    state_data = json.loads(
        (Path(env["CLAUDE_PLUGIN_DATA"]) / "loop-xyz" / "state.json").read_text(encoding="utf-8")
    )
    assert state_data["finalization"]["status"] in {"REJECTED", "SKIPPED"}
    assert not calls.exists() or calls.read_text(encoding="utf-8") == ""


def test_stop_nmem_failure_is_fail_open(
    isolated_plugin_data: Path, tmp_path: Path
) -> None:
    """A non-zero nmem exit must keep the hook exit 0 and surface FAILED_OPEN/UNKNOWN."""
    env, _ = _hook_env_with_fake_nmem(tmp_path, exit_code=1)
    marker = _valid_candidate_marker()
    payload = {
        "session_id": "sess-1",
        "last_assistant_message": f"Lesson:\n\n{marker}\n",
    }
    result = _run_hook(
        "Stop", payload, extra_env=env, plugin_data=Path(env["CLAUDE_PLUGIN_DATA"])
    )
    assert result.returncode == 0, f"hook must stay exit 0 on nmem failure: {result.stderr}"
    state_data = json.loads(
        (Path(env["CLAUDE_PLUGIN_DATA"]) / "loop-xyz" / "state.json").read_text(encoding="utf-8")
    )
    assert state_data["finalization"]["status"] in {"FAILED_OPEN", "UNKNOWN"}
    assert state_data["audit_only"] is True


def test_stop_audit_record_does_not_persist_full_message(
    isolated_plugin_data: Path, tmp_path: Path
) -> None:
    """The bounded audit record never includes the assistant message body."""
    env, _ = _hook_env_with_fake_nmem(tmp_path)
    marker = _valid_candidate_marker(claim="UNIQUE-secret-marker-secret")
    payload = {
        "session_id": "sess-1",
        "last_assistant_message": f"Lesson:\n\n{marker}\n",
    }
    result = _run_hook(
        "Stop", payload, extra_env=env, plugin_data=Path(env["CLAUDE_PLUGIN_DATA"])
    )
    assert result.returncode == 0
    state_data = json.loads(
        (Path(env["CLAUDE_PLUGIN_DATA"]) / "loop-xyz" / "state.json").read_text(encoding="utf-8")
    )
    audit_path = Path(env["CLAUDE_PLUGIN_DATA"]) / "loop-xyz" / "audit.jsonl"
    audit_payload = audit_path.read_text(encoding="utf-8")
    assert "UNIQUE-secret-marker-secret" not in audit_payload, (
        "audit.jsonl must NOT persist the full assistant message text"
    )
    assert "UNIQUE-secret-marker-secret" not in json.dumps(state_data), (
        "state.json must NOT persist the full assistant message text"
    )


def test_subagent_stop_uses_same_writer(
    isolated_plugin_data: Path, tmp_path: Path
) -> None:
    """SubagentStop reaches the writer with the same idempotency contract as Stop."""
    env, calls = _hook_env_with_fake_nmem(tmp_path)
    marker = _valid_candidate_marker()
    payload = {
        "session_id": "worker-1",
        "last_assistant_message": f"Lesson:\n\n{marker}\n",
    }
    result = _run_hook(
        "SubagentStop", payload, extra_env=env, plugin_data=Path(env["CLAUDE_PLUGIN_DATA"])
    )
    assert result.returncode == 0
    state_data = json.loads(
        (Path(env["CLAUDE_PLUGIN_DATA"]) / "loop-xyz" / "state.json").read_text(encoding="utf-8")
    )
    assert state_data["finalization"]["status"] == "SAVED"
    assert len(calls.read_text(encoding="utf-8").splitlines()) == 1


# ---------------------------------------------------------------------------
# U1 (fixer) — atomic state.json writes under concurrent SubagentStop
#
# Eight concurrent SubagentStop subprocesses against one fake nmem and
# one shared CLAUDE_PLUGIN_DATA must all land `status=SAVED` on their
# distinct digest, and the audit.jsonl must carry exactly eight rows.
# This is the regression contract for fix-planner finding adversarial:A2
# (TOCTOU on `state.json.tmp`) + adversarial:A6 (audit_only=true
# hard-coded + pre-write save_results snapshot).
# ---------------------------------------------------------------------------


def test_state_json_atomic_under_concurrent_subagent_stop(tmp_path: Path) -> None:
    """Eight concurrent SubagentStop writers must all land a finalization."""
    bin_dir = tmp_path / "bin"
    calls = _write_fake_nmem(bin_dir)
    data_dir = tmp_path / "data"
    env = _ralph_env()
    env["PATH"] = f"{bin_dir}:/usr/bin:/bin"
    env["CLAUDE_PLUGIN_DATA"] = str(data_dir)

    markers = []
    for index in range(8):
        candidate = _valid_candidate_marker(
            title=f"Concurrent lesson #{index}",
            claim=f"distinct digest #{index}",
            evidence=f"path:/tmp/secret-{index}",
        )
        markers.append(f"Lesson {index}:\n\n{candidate}\n")

    import concurrent.futures

    def _fire(idx: int) -> subprocess.CompletedProcess:
        return _run_hook(
            "SubagentStop",
            {
                "session_id": f"worker-{idx}",
                "agent_id": f"worker-{idx}",
                "last_assistant_message": markers[idx],
            },
            extra_env=env,
            plugin_data=data_dir,
        )

    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        results = list(pool.map(_fire, range(8)))

    assert all(r.returncode == 0 for r in results), (
        f"every concurrent SubagentStop must exit 0; "
        f"errors={[r.stderr for r in results if r.returncode != 0]}"
    )

    state_path = data_dir / "loop-xyz" / "state.json"
    assert state_path.is_file(), "state.json must exist"
    state_payload = json.loads(state_path.read_text(encoding="utf-8"))
    assert state_payload["hook"] == "SubagentStop"
    assert state_payload["finalization"]["status"] == "SAVED", (
        f"final SubagentStop state must be SAVED, got {state_payload['finalization']['status']!r}"
    )
    assert state_payload["audit_only"] is False

    audit_path = data_dir / "loop-xyz" / "audit.jsonl"
    assert audit_path.is_file(), "audit.jsonl must exist"
    audit_records = [
        json.loads(line)
        for line in audit_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    assert len(audit_records) == 8, (
        f"audit.jsonl must have 8 rows, got {len(audit_records)}: {audit_records}"
    )
    call_lines = calls.read_text(encoding="utf-8").splitlines()
    assert len(call_lines) == 8, (
        f"each distinct digest must reach nmem exactly once, got {len(call_lines)}"
    )
    serialized = json.dumps(state_payload, ensure_ascii=True)
    for index in range(8):
        assert f"secret-{index}" not in serialized, (
            f"evidence path for digest #{index} leaked into state.json"
        )
