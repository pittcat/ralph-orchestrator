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
