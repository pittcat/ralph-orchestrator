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


# ---------------------------------------------------------------------------
# U2 (fixer) — pending ledger TTL
#
# A SIGKILL mid-nmem or a slow nmem must not trap ``scope:digest`` in
# ``IN_FLIGHT``/``UNKNOWN`` forever. Once the TTL elapses, the next
# attempt proceeds to nmem instead of being short-circuited.
# ---------------------------------------------------------------------------


def test_pending_entry_expires_after_ttl(tmp_path, monkeypatch):
    """IN_FLIGHT entry older than TTL is cleared on the next attempt."""
    monkeypatch.setenv("CLAUDE_PLUGIN_DATA", str(tmp_path))
    writer = _load("_writer_ttl_test_module", "memory_writer.py")
    calls = []

    def run(argv, **kwargs):
        calls.append(argv)
        class Fake:
            returncode = 0
            stdout = '{"id":"mem-1"}'
            stderr = ""

        return Fake()

    from datetime import datetime, timedelta, timezone

    record = {
        "memory_type": "durable_decision",
        "title": "TTL regression",
        "claim": "TTL clears IN_FLIGHT",
        "why_it_matters": "SIGKILL deadlocks.",
        "evidence": "manual seed",
        "applies_when": "writer retry",
        "scope": "plugin:nowledge-mem-ralph",
        "verification": "pytest",
        "critical_assumptions": [],
        "critical_ambiguities": [],
        "metrics": {"confidence": 90},
        "memory_digest": "c" * 64,
        "policy_version": "test-policy",
        "source": {"hat": "worker", "scope": "plugin:nowledge-mem-ralph"},
        "result": "ACCEPTED",
    }
    key = record["scope"] + ":" + record["memory_digest"]
    ledger = {
        "saved": {},
        "pending": {
            key: {
                "record": record,
                "status": "IN_FLIGHT",
                "pending_at": (
                    datetime.now(timezone.utc) - timedelta(seconds=700)
                ).isoformat(),
            }
        },
    }
    ledger_path = Path(writer._state_root()) / "memory-ledger.json"
    ledger_path.write_text(json.dumps(ledger), encoding="utf-8")

    result = writer.write_memory(record, runner=run)
    assert result.result == "SAVED"
    assert len(calls) == 1
    re_ledger = json.loads(ledger_path.read_text(encoding="utf-8"))
    assert key not in re_ledger.get("pending", {})


def test_pending_entry_within_ttl_is_unknown(tmp_path, monkeypatch):
    """A recent IN_FLIGHT entry still blocks the second attempt."""
    monkeypatch.setenv("CLAUDE_PLUGIN_DATA", str(tmp_path))
    writer = _load("_writer_ttl_recent_test_module", "memory_writer.py")
    from datetime import datetime, timedelta, timezone

    record = {
        "memory_type": "durable_decision",
        "title": "TTL still active",
        "claim": "Recent entry is honoured.",
        "why_it_matters": "Don't blow past recent state.",
        "evidence": "manual seed",
        "applies_when": "writer retry",
        "scope": "plugin:nowledge-mem-ralph",
        "verification": "pytest",
        "critical_assumptions": [],
        "critical_ambiguities": [],
        "metrics": {"confidence": 90},
        "memory_digest": "d" * 64,
        "policy_version": "test-policy",
        "source": {"hat": "worker", "scope": "plugin:nowledge-mem-ralph"},
        "result": "ACCEPTED",
    }
    key = record["scope"] + ":" + record["memory_digest"]
    ledger = {
        "saved": {},
        "pending": {
            key: {
                "record": record,
                "status": "IN_FLIGHT",
                "pending_at": (
                    datetime.now(timezone.utc) - timedelta(seconds=60)
                ).isoformat(),
            }
        },
    }
    ledger_path = Path(writer._state_root()) / "memory-ledger.json"
    ledger_path.write_text(json.dumps(ledger), encoding="utf-8")

    calls = []

    def run(argv, **kwargs):
        calls.append(argv)
        return None

    result = writer.write_memory(record, runner=run)
    assert result.result == "UNKNOWN"
    assert calls == []


def test_unknown_pending_entry_expires_after_ttl(tmp_path, monkeypatch):
    """An UNKNOWN entry older than TTL also expires."""
    monkeypatch.setenv("CLAUDE_PLUGIN_DATA", str(tmp_path))
    writer = _load("_writer_ttl_unknown_test_module", "memory_writer.py")
    from datetime import datetime, timedelta, timezone

    record = {
        "memory_type": "durable_decision",
        "title": "TTL unknown regression",
        "claim": "TTL clears UNKNOWN.",
        "why_it_matters": "Stale UNKNOWN deadlocks.",
        "evidence": "manual seed",
        "applies_when": "writer retry",
        "scope": "plugin:nowledge-mem-ralph",
        "verification": "pytest",
        "critical_assumptions": [],
        "critical_ambiguities": [],
        "metrics": {"confidence": 90},
        "memory_digest": "e" * 64,
        "policy_version": "test-policy",
        "source": {"hat": "worker", "scope": "plugin:nowledge-mem-ralph"},
        "result": "ACCEPTED",
    }
    key = record["scope"] + ":" + record["memory_digest"]
    ledger = {
        "saved": {},
        "pending": {
            key: {
                "record": record,
                "status": "UNKNOWN",
                "pending_at": (
                    datetime.now(timezone.utc) - timedelta(seconds=900)
                ).isoformat(),
            }
        },
    }
    ledger_path = Path(writer._state_root()) / "memory-ledger.json"
    ledger_path.write_text(json.dumps(ledger), encoding="utf-8")

    calls = []

    def run(argv, **kwargs):
        calls.append(argv)
        class Fake:
            returncode = 0
            stdout = '{"id":"mem-2"}'
            stderr = ""

        return Fake()

    result = writer.write_memory(record, runner=run)
    assert result.result == "SAVED"
    assert len(calls) == 1


def test_hook_budget_propagates_to_writer(tmp_path, monkeypatch):
    """Hook budget propagation shrinks writer timeout when budget tight."""
    monkeypatch.setenv("CLAUDE_PLUGIN_DATA", str(tmp_path))
    monkeypatch.setenv("NOWLEDGE_HOOK_TIMEOUT_SECONDS", "1.5")

    record = {
        "memory_type": "durable_decision",
        "title": "Budget regression",
        "claim": "Budget tightens writer timeout.",
        "why_it_matters": "Hooks must respect 5s budget.",
        "evidence": "manual seed",
        "applies_when": "writer timeout",
        "scope": "plugin:nowledge-mem-ralph",
        "verification": "pytest",
        "critical_assumptions": [],
        "critical_ambiguities": [],
        "metrics": {"confidence": 90},
        "memory_digest": "f" * 64,
        "policy_version": "test-policy",
        "source": {"hat": "worker", "scope": "plugin:nowledge-mem-ralph"},
        "result": "ACCEPTED",
    }
    record_digest = record["memory_digest"]
    record_policy = record["policy_version"]

    def make_save_result(record_arg):
        record = record_arg
        record_digest_local = record["memory_digest"]
        record_policy_local = record["policy_version"]

        class FakeSaveResult:
            result = "ACCEPTED"
            reason = "ok"
            memory_digest = record_digest_local
            policy_version = record_policy_local
            record = None  # set in __init__

            def __init__(inner_self):
                inner_self.record = record

        return FakeSaveResult()

    def make_write_result(record_arg):
        record_policy_local = record_arg["policy_version"]

        class FakeWriteResult:
            result = "SAVED"
            reason = "ok"
            memory_id = "mem-budget"
            policy_version = record_policy_local
            source = {"hat": "worker"}
            retryable = False

        return FakeWriteResult()

    captured = {}

    def fake_save(candidate, source=None):
        captured["save_called"] = True
        return make_save_result(record)

    def fake_write(record_arg, runner=None, timeout=4.0):
        captured["writer_timeout"] = timeout
        captured["writer_called"] = True
        return make_write_result(record)

    import importlib.util
    import sys
    import types

    fake_save_mod = types.ModuleType("save_budget")
    fake_save_mod.save = fake_save

    fake_writer_mod = types.ModuleType("writer_budget")
    fake_writer_mod.write_from_save_result = fake_write

    scripts_dir = ROOT / "plugins" / "nowledge-mem-ralph" / "scripts"
    atomic_spec = importlib.util.spec_from_file_location(
        "_atomic_state_for_budget", scripts_dir / "_atomic_state.py"
    )
    atomic_mod = importlib.util.module_from_spec(atomic_spec)
    atomic_spec.loader.exec_module(atomic_mod)

    fin_spec = importlib.util.spec_from_file_location(
        "_finalization_for_budget_test",
        scripts_dir / "memory_finalization.py",
    )
    assert fin_spec and fin_spec.loader
    fin = importlib.util.module_from_spec(fin_spec)
    sys.modules["_finalization_for_budget_test"] = fin
    fin_spec.loader.exec_module(fin)

    fin._load_save_module = lambda scripts_dir: fake_save_mod  # type: ignore[assignment]
    fin._load_writer_module = lambda scripts_dir: fake_writer_mod  # type: ignore[assignment]
    fin._load_atomic_state = lambda scripts_dir: atomic_mod  # type: ignore[assignment]

    parser_result = types.SimpleNamespace(
        status="PARSED",
        candidate=record,
        memory_digest=record["memory_digest"],
        reason="ok",
    )
    env = {"RALPH_CURRENT_LOOP_ID": "loop-budget", "RALPH_CURRENT_HAT": "worker"}
    payload = {"event": "Stop", "session_id": "s-budget"}
    state_root = tmp_path / "state"
    state_root.mkdir(parents=True, exist_ok=True)

    result = fin.run_finalization(
        env,
        payload,
        parser_result=parser_result,
        state_root=state_root,
    )
    assert result.status == "SAVED"
    assert captured.get("writer_called") is True
    # With hook_timeout=1.5s the writer timeout must shrink to fit.
    assert captured["writer_timeout"] <= 1.5
