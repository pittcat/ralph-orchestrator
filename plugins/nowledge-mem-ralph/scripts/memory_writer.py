"""Idempotent, accepted-only nmem writer for save-memory."""

from __future__ import annotations

import json
import os
import tempfile
import fcntl
from pathlib import Path
from typing import Any, Mapping

import importlib.util
import sys


def _load(name: str, filename: str):
    module = sys.modules.get(name)
    if module is not None:
        return module
    path = Path(__file__).resolve().parent / filename
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {filename}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_client = _load("_nowledge_mem_ralph_nmem_client", "nmem_client.py")
_result = _load("_nowledge_mem_ralph_memory_result", "memory_result.py")


def _state_root() -> Path:
    configured = os.environ.get("CLAUDE_PLUGIN_DATA", "").strip()
    return Path(configured) if configured else Path(tempfile.gettempdir()) / "nowledge-mem-ralph-fallback"


def _ledger_path() -> Path:
    return _state_root() / "memory-ledger.json"


def _lock_path() -> Path:
    return _state_root() / "memory-ledger.lock"


def _read_ledger() -> dict[str, Any]:
    try:
        payload = json.loads(_ledger_path().read_text(encoding="utf-8"))
        return payload if isinstance(payload, dict) else {}
    except (OSError, json.JSONDecodeError):
        return {}


def _write_ledger(payload: dict[str, Any]) -> None:
    path = _ledger_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(prefix="memory-ledger-", dir=path.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(payload, handle, ensure_ascii=False, sort_keys=True)
        os.replace(tmp_name, path)
    finally:
        try:
            os.unlink(tmp_name)
        except FileNotFoundError:
            pass


class _LedgerLock:
    """Cross-process lock covering dedupe claim, nmem write, and commit."""

    def __enter__(self):
        path = _lock_path()
        path.parent.mkdir(parents=True, exist_ok=True)
        self._handle = path.open("a+")
        fcntl.flock(self._handle.fileno(), fcntl.LOCK_EX)
        return self

    def __exit__(self, *_exc):
        fcntl.flock(self._handle.fileno(), fcntl.LOCK_UN)
        self._handle.close()


def record_result(result: Any, source: Mapping[str, Any]) -> None:
    """Persist bounded save outcomes for the Stop audit hook."""
    try:
        loop_id = str(source.get("loop_id", "")).strip()
        if not loop_id:
            return
        path = _state_root() / loop_id / "memory-results.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(result.as_dict(), ensure_ascii=False, sort_keys=True) + "\n")
    except (OSError, TypeError, ValueError):
        # Audit persistence is observability only; it must not change the
        # result of the accepted-only write or block the agent session.
        return


def _accepted_record(record: Mapping[str, Any]) -> bool:
    return (
        isinstance(record, Mapping)
        and record.get("memory_digest")
        and isinstance(record.get("source"), Mapping)
        and bool(record.get("source"))
        and record.get("scope")
        and record.get("policy_version")
    )


def write_memory(record: Mapping[str, Any], *, runner=None, timeout: float = 4.0):
    """Write only a policy-accepted record; all failures are fail-open."""
    if not _accepted_record(record) or record.get("result") not in (None, "ACCEPTED"):
        return _result.MemoryWriteResult("REJECTED", "writer requires an ACCEPTED record")
    canonical = dict(record)
    digest = str(canonical["memory_digest"])
    scope = str(canonical.get("scope", ""))
    key = f"{scope}:{digest}"
    source = canonical.get("source", {})
    with _LedgerLock():
        ledger = _read_ledger()
        saved = ledger.get("saved", {})
        if isinstance(saved, dict) and key in saved:
            entry = saved[key] if isinstance(saved[key], dict) else {}
            outcome = _result.MemoryWriteResult(
                "ALREADY_SAVED", "same digest already has a successful nmem write", digest,
                str(entry.get("memory_id")) if entry.get("memory_id") else None,
                str(canonical.get("policy_version", "")), source, False,
            )
            record_result(outcome, source)
            return outcome

        pending = ledger.get("pending", {})
        pending_entry = pending.get(key) if isinstance(pending, dict) else None
        if isinstance(pending_entry, dict) and pending_entry.get("status") in {"UNKNOWN", "IN_FLIGHT"}:
            outcome = _result.MemoryWriteResult(
                "UNKNOWN", "a previous write has unknown remote state; reconcile before retry", digest,
                None, str(canonical.get("policy_version", "")), source, True,
            )
            record_result(outcome, source)
            return outcome

        # Persist the candidate before the side effect. FAILED_OPEN may be
        # retried; UNKNOWN is held until an explicit reconciliation decision.
        pending = pending if isinstance(pending, dict) else {}
        pending[key] = {"record": canonical, "status": "IN_FLIGHT"}
        ledger["pending"] = pending
        try:
            _write_ledger(ledger)
        except OSError as exc:
            outcome = _result.MemoryWriteResult(
                "FAILED_OPEN", f"could not stage accepted Memory: {exc}", digest,
                None, str(canonical.get("policy_version", "")), source, True,
            )
            record_result(outcome, source)
            return outcome

        status, reason, memory_id = _client.add_memory(canonical, runner=runner, timeout=timeout)
        outcome = _result.MemoryWriteResult(
            status, reason, digest, memory_id, str(canonical.get("policy_version", "")),
            source, status in {"FAILED_OPEN", "UNKNOWN"},
        )
        if status == "SAVED":
            saved = saved if isinstance(saved, dict) else {}
            saved[key] = {"memory_id": memory_id, "source": dict(source)}
            pending.pop(key, None)
            ledger["saved"] = saved
            ledger["pending"] = pending
            try:
                _write_ledger(ledger)
            except OSError:
                # The old ledger still contains the pending record. Do not
                # claim success: the remote side effect is now UNKNOWN.
                outcome = _result.MemoryWriteResult(
                    "UNKNOWN", "nmem accepted the Memory but local commit failed", digest,
                    memory_id, str(canonical.get("policy_version", "")), source, True,
                )
        else:
            pending[key]["status"] = status
            pending[key]["reason"] = reason
            try:
                _write_ledger(ledger)
            except OSError:
                pass
        record_result(outcome, source)
        return outcome


def record_policy_result(save_result, source: Mapping[str, Any]):
    """Record a deterministic rejection before the Stop hook runs."""
    outcome = _result.MemoryWriteResult(
        str(getattr(save_result, "result", "REJECTED")),
        str(getattr(save_result, "reason", "")),
        str(getattr(save_result, "memory_digest", "")),
        None,
        str(getattr(save_result, "policy_version", "")),
        source,
        False,
    )
    record_result(outcome, source)
    return outcome


def write_from_save_result(save_result, *, runner=None, timeout: float = 4.0):
    """Bridge U03's SaveResult to the accepted-only writer."""
    if getattr(save_result, "result", None) != "ACCEPTED" or not getattr(save_result, "record", None):
        return _result.MemoryWriteResult(
            getattr(save_result, "result", "REJECTED"),
            "only ACCEPTED save-memory results may reach the writer",
            getattr(save_result, "memory_digest", ""),
        )
    record = dict(save_result.record)
    record["policy_version"] = save_result.policy_version
    record["result"] = "ACCEPTED"
    return write_memory(record, runner=runner, timeout=timeout)
