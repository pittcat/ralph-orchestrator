"""Executable U04 contract tests for the accepted-only writer."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[3]
SCRIPTS = ROOT / "plugins" / "nowledge-mem-ralph" / "scripts"


def _load(name: str, filename: str):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def writer(tmp_path, monkeypatch):
    monkeypatch.setenv("CLAUDE_PLUGIN_DATA", str(tmp_path))
    return _load("_writer_test_module", "memory_writer.py")


def _record() -> dict:
    return {
        "memory_type": "durable_decision",
        "title": "Atomic plugin state writes",
        "claim": "Accepted records are written with an atomic ledger update.",
        "why_it_matters": "Retries must not observe half a ledger.",
        "evidence": "The writer uses a temporary file and replace.",
        "applies_when": "The plugin persists accepted Memory results.",
        "scope": "plugin:nowledge-mem-ralph",
        "verification": "The writer tests inspect the resulting ledger.",
        "memory_digest": "a" * 64,
        "policy_version": "test-policy",
        "source": {"hat": "worker", "scope": "plugin:nowledge-mem-ralph"},
        "result": "ACCEPTED",
    }


class FakeCompleted:
    returncode = 0
    stdout = '{"id":"mem-1"}'
    stderr = ""


def test_only_accepted_record_reaches_nmem(writer):
    calls = []

    def run(argv, **kwargs):
        calls.append((argv, kwargs))
        return FakeCompleted()

    rejected = writer.write_memory({"memory_digest": "x"}, runner=run)
    assert rejected.result == "REJECTED"
    assert calls == []

    saved = writer.write_memory(_record(), runner=run)
    assert saved.result == "SAVED"
    assert saved.memory_id == "mem-1"
    assert len(calls) == 1
    argv = calls[0][0]
    assert argv[:4] == ["nmem", "--json", "m", "add"]
    assert "--title" in argv and "--unit-type" in argv
    assert calls[0][1]["timeout"] == 4.0


def test_successful_digest_is_idempotent_without_second_nmem_call(writer):
    calls = []

    def run(argv, **kwargs):
        calls.append(argv)
        return FakeCompleted()

    assert writer.write_memory(_record(), runner=run).result == "SAVED"
    again = writer.write_memory(_record(), runner=run)
    assert again.result == "ALREADY_SAVED"
    assert again.memory_id == "mem-1"
    assert len(calls) == 1


@pytest.mark.parametrize(
    ("exception", "expected"),
    [(FileNotFoundError(), "FAILED_OPEN"), (TimeoutError(), "FAILED_OPEN")],
)
def test_nmem_failures_never_raise(writer, exception, expected):
    def run(argv, **kwargs):
        if isinstance(exception, TimeoutError):
            import subprocess
            raise subprocess.TimeoutExpired(argv, 4)
        raise exception

    result = writer.write_memory(_record(), runner=run)
    assert result.result == expected if not isinstance(exception, TimeoutError) else result.result == "UNKNOWN"
    assert result.retryable is True
    ledger = json.loads((Path(writer._state_root()) / "memory-ledger.json").read_text())
    assert ledger["pending"][_record()["scope"] + ":" + _record()["memory_digest"]]["status"] == result.result


def test_invalid_json_after_write_is_unknown_and_not_marked_saved(writer):
    class Invalid:
        returncode = 0
        stdout = "not-json"
        stderr = ""

    result = writer.write_memory(_record(), runner=lambda *a, **k: Invalid())
    assert result.result == "UNKNOWN"
    assert result.retryable is True
    ledger = json.loads((Path(writer._state_root()) / "memory-ledger.json").read_text())
    assert ledger["pending"]
