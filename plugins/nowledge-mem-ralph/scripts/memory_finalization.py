"""Finalization coordinator for Stop / SubagentStop auto-Memory.

The hook only owns the finalization lifecycle; the writer (``memory_writer``)
is the sole owner of the ``nmem`` subprocess. This module is the
single bridge that:

1. Resolves the candidate produced by :mod:`memory_marker`.
2. Calls the existing :func:`memory.save` entry — the same entry the
   ``/save-memory`` command and the ``save-memory`` skill use.
3. Hands the resulting ``SaveResult.record`` to
   :func:`memory_writer.write_from_save_result` when ``ACCEPTED`` —
   never before, and never to any other client.
4. Persists a bounded audit block describing the bounded outcome.

The coordinator never reads ``transcript_path`` and never spawns
``nmem`` itself; that responsibility belongs entirely to the writer.
"""

from __future__ import annotations

import dataclasses
import importlib.util
import json
import os
import sys
from pathlib import Path
from typing import Any, Mapping


# Map a ``memory_writer.MemoryWriteResult.result`` value into a final
# audit ``status``. The hook surfaces this string instead of the raw
# writer enum so downstream readers do not have to import the writer.
_STATUS_MAP = {
    "SAVED": "SAVED",
    "ALREADY_SAVED": "ALREADY_SAVED",
    "FAILED_OPEN": "FAILED_OPEN",
    "UNKNOWN": "UNKNOWN",
}


@dataclasses.dataclass(frozen=True)
class CoordinatorResult:
    """Outcome of one coordinator run.

    * ``status`` — bounded terminal status used by ``state.json``. One
    of ``PARSED``, ``SAVED``, ``ALREADY_SAVED``, ``REJECTED``,
    ``SKIPPED``, ``FAILED_OPEN``, ``UNKNOWN``.
    * ``memory_digest`` — the parser-supplied digest (empty when the
    candidate never reached the writer).
    * ``reason`` — human-readable summary surfaced to the audit log.
    * ``policy_version`` — the writer-recorded policy version, when the
    candidate reached the writer. Empty otherwise.
    """

    status: str
    memory_digest: str
    reason: str
    policy_version: str = ""


def _load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load spec for {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def _load_save_module(scripts_dir: Path):
    return _load_module(
        "_nowledge_mem_ralph_memory_via_finalization",
        scripts_dir / "memory.py",
    )


def _load_writer_module(scripts_dir: Path):
    return _load_module(
        "_nowledge_mem_ralph_memory_writer_via_finalization",
        scripts_dir / "memory_writer.py",
    )


def _source_from_env(env: Mapping[str, str]) -> dict[str, str]:
    """Capture only bounded Ralph metadata, never prompt or transcript text."""
    keys = {
        "loop_id": "RALPH_CURRENT_LOOP_ID",
        "hat": "RALPH_CURRENT_HAT",
        "wave": "RALPH_WAVE",
        "attempt": "RALPH_ATTEMPT",
        "repo": "RALPH_WORKSPACE_ROOT",
    }
    return {
        name: str(env.get(env_key, "")).strip()[:300]
        for name, env_key in keys.items()
        if str(env.get(env_key, "")).strip()
    }


def _write_state(
    state_path: Path,
    *,
    event: str,
    env: Mapping[str, str],
    payload: Mapping[str, Any],
    coordinator: CoordinatorResult,
) -> None:
    """Append or refresh the bounded finalization block in ``state.json``.

    The function is fail-open: any I/O failure is logged to stderr but
    never raised, so the hook can stay exit 0.
    """
    try:
        state_path.parent.mkdir(parents=True, exist_ok=True)
        try:
            loaded = json.loads(state_path.read_text(encoding="utf-8"))
            state = dict(loaded) if isinstance(loaded, dict) else {}
        except (OSError, json.JSONDecodeError):
            state = {}
        finalization = {
            "status": coordinator.status,
            "memory_digest": coordinator.memory_digest,
            "reason": coordinator.reason,
            "policy_version": coordinator.policy_version,
            "event": event,
            "hat": str(env.get("RALPH_CURRENT_HAT", "")),
            "session_id": str(payload.get("session_id", "")),
        }
        state["finalization"] = finalization
        state["audit_only"] = coordinator.status not in {"SAVED", "ALREADY_SAVED"}
        state["stop_hook_fired"] = True
        tmp = state_path.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(state, ensure_ascii=False, sort_keys=True), encoding="utf-8")
        os.replace(tmp, state_path)
    except OSError as exc:
        # Audit persistence is observability only — never block the agent.
        sys.stderr.write(
            json.dumps(
                {
                    "event": "finalization_state_write_failed",
                    "error": str(exc),
                    "type": type(exc).__name__,
                }
            )
            + "\n"
        )


def _fallback_result(parser_result: Any) -> CoordinatorResult:
    """Map a parser-only outcome into a bounded coordinator result.

    Used when the parser produced SKIPPED / REJECTED so the hook can
    still surface a deterministic status without invoking save or the
    writer.
    """
    return CoordinatorResult(
        status=str(getattr(parser_result, "status", "SKIPPED")),
        memory_digest=str(getattr(parser_result, "memory_digest", "")),
        reason=str(getattr(parser_result, "reason", "")),
    )


def run_finalization(
    env: Mapping[str, str],
    payload: Mapping[str, Any],
    *,
    parser_result: Any,
    state_root: Path,
) -> CoordinatorResult:
    """Bridge parser → save → writer for one Stop / SubagentStop event.

    The function never raises. Any error is mapped to a fail-open
    coordinator result and recorded in ``state.json`` as ``UNKNOWN``.
    """
    scripts_dir = Path(__file__).resolve().parent
    status = str(getattr(parser_result, "status", "SKIPPED"))
    digest = str(getattr(parser_result, "memory_digest", ""))
    reason = str(getattr(parser_result, "reason", ""))
    loop_id = str(env.get("RALPH_CURRENT_LOOP_ID", "")).strip()
    state_path = state_root / loop_id / "state.json" if loop_id else None

    if status != "PARSED":
        coordinator = CoordinatorResult(status=status, memory_digest=digest, reason=reason)
        if state_path is not None:
            _write_state(state_path, event=str(payload.get("event", "Stop")), env=env, payload=payload, coordinator=coordinator)
        return coordinator

    candidate = getattr(parser_result, "candidate", None)
    if not isinstance(candidate, Mapping):
        coordinator = CoordinatorResult(
            status="REJECTED", memory_digest=digest, reason="parser returned no candidate"
        )
        if state_path is not None:
            _write_state(state_path, event=str(payload.get("event", "Stop")), env=env, payload=payload, coordinator=coordinator)
        return coordinator

    source = _source_from_env(env)
    try:
        save_module = _load_save_module(scripts_dir)
        save_result = save_module.save(candidate, source=source)
    except Exception as exc:  # pragma: no cover — defensive
        coordinator = CoordinatorResult(
            status="UNKNOWN",
            memory_digest=digest,
            reason=f"memory.save raised {type(exc).__name__}: {exc}",
        )
        if state_path is not None:
            _write_state(state_path, event=str(payload.get("event", "Stop")), env=env, payload=payload, coordinator=coordinator)
        return coordinator

    if save_result.result != "ACCEPTED":
        # Deterministic rejection — record the verdict but never reach
        # the writer. This keeps the audit-only contract intact for
        # policy rejection, NEEDS_REWRITE, and OBSERVATION.
        coordinator = CoordinatorResult(
            status=save_result.result if save_result.result in {"REJECTED", "NEEDS_REWRITE", "OBSERVATION"} else "REJECTED",
            memory_digest=save_result.memory_digest or digest,
            reason=save_result.reason or reason,
            policy_version=save_result.policy_version,
        )
        if state_path is not None:
            _write_state(state_path, event=str(payload.get("event", "Stop")), env=env, payload=payload, coordinator=coordinator)
        return coordinator

    # ACCEPTED → hand off to the existing writer. The writer is the
    # only caller of ``nmem``; this module never spawns it.
    try:
        writer = _load_writer_module(scripts_dir)
        write_result = writer.write_from_save_result(save_result)
    except Exception as exc:  # pragma: no cover — defensive
        coordinator = CoordinatorResult(
            status="UNKNOWN",
            memory_digest=save_result.memory_digest,
            reason=f"writer raised {type(exc).__name__}: {exc}",
            policy_version=save_result.policy_version,
        )
        if state_path is not None:
            _write_state(state_path, event=str(payload.get("event", "Stop")), env=env, payload=payload, coordinator=coordinator)
        return coordinator

    coordinator_status = _STATUS_MAP.get(
        getattr(write_result, "result", "UNKNOWN"), "UNKNOWN"
    )
    coordinator = CoordinatorResult(
        status=coordinator_status,
        memory_digest=save_result.memory_digest,
        reason=str(getattr(write_result, "reason", "")) or reason,
        policy_version=str(getattr(write_result, "policy_version", save_result.policy_version)),
    )
    if state_path is not None:
        _write_state(state_path, event=str(payload.get("event", "Stop")), env=env, payload=payload, coordinator=coordinator)
    return coordinator