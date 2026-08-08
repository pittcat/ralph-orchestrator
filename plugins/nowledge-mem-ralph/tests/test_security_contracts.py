"""Regression tests for lifecycle gates and query trust boundaries."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
SCRIPTS = ROOT / "plugins/nowledge-mem-ralph/scripts"


def _load(name: str, filename: str):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def test_disabled_or_unsafe_loop_is_noop(tmp_path, monkeypatch):
    hook = _load("_security_hook_runtime", "hook_runtime.py")
    monkeypatch.setenv("CLAUDE_PLUGIN_DATA", str(tmp_path))
    monkeypatch.setenv("RALPH_CURRENT_LOOP_ID", "../escape")
    monkeypatch.setenv("RALPH_NOWLEDGE_ENABLED", "1")
    assert hook._nowledge_env_present() is False
    monkeypatch.setenv("RALPH_CURRENT_LOOP_ID", "safe-loop")
    monkeypatch.setenv("RALPH_NOWLEDGE_ENABLED", "0")
    assert hook._nowledge_env_present() is False
    assert list(tmp_path.iterdir()) == []


def test_query_never_contains_absolute_workspace_path():
    recall = _load("_security_recall", "recall.py")
    query = recall.normalize_query(
        repo_basename="repo",
        preset="preset",
        workspace_root="/Users/alice/private/repo",
        objective="fix auth",
        plan="plan-1",
    )
    assert query == "repo preset fix auth plan-1"
    assert "/Users" not in query
