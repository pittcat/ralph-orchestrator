"""Runtime tests for the optional structured evaluator bridge."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "plugins/nowledge-mem-ralph/scripts/memory_evaluator.py"


def _load():
    spec = importlib.util.spec_from_file_location("_evaluator_runtime_test", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["_evaluator_runtime_test"] = module
    spec.loader.exec_module(module)
    return module


def test_evaluator_accepts_only_structured_verdict(tmp_path, monkeypatch):
    script = tmp_path / "evaluator.py"
    script.write_text(
        "import json, sys; json.load(sys.stdin); print(json.dumps({'verdict':'ACCEPTED','reasons':['stable']}))",
        encoding="utf-8",
    )
    monkeypatch.setenv("RALPH_NOWLEDGE_EVALUATOR", f"{sys.executable} {script}")
    evaluator = _load()
    result, reason, details = evaluator.evaluate({"claim": "stable"})
    assert result == "ACCEPTED"
    assert reason == "stable"
    assert details["verdict"] == "ACCEPTED"


def test_evaluator_rejects_forbidden_side_effect_fields(tmp_path, monkeypatch):
    script = tmp_path / "evaluator.py"
    script.write_text(
        "import json; print(json.dumps({'verdict':'ACCEPTED','write':'nmem'}))",
        encoding="utf-8",
    )
    monkeypatch.setenv("RALPH_NOWLEDGE_EVALUATOR", f"{sys.executable} {script}")
    result, reason, _ = _load().evaluate({"claim": "unsafe"})
    assert result == "REJECTED"
    assert "side-effect" in reason
