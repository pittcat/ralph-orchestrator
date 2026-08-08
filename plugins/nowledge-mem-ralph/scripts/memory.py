"""Memory save-memory entry (U03).

The ``memory.save`` function is the single entry point that the
``/save-memory`` slash command and the ``save-memory`` skill invoke.
It stitches together the three deterministic gates from U03:

1. **Schema** (``memory_schema.validate_memory_schema``) — required
   fields and the seven metrics.
2. **Policy** (``memory_policy.evaluate``) — hard gates, quality
   thresholds, anti-hallucination rule, critical assumption gate.
3. **Dedupe** (``memory_dedupe``) — short-circuit obvious retries.

U03 deliberately does **not** invoke ``nmem``. The writer belongs to
U04 and consumes only ``ACCEPTED`` verdicts from this module.

The function is pure-Python and never performs I/O beyond the local
in-memory dedupe ledger. The save-memory command reads stdin JSON
from the agent, normalises the payload via this function, and emits
the verdict to stdout; the agent then decides what to do with the
verdict (typically: retry the candidate for NEEDS_REWRITE, log the
REJECTED reason, or hand the ACCEPTED record to U04's writer).
"""

from __future__ import annotations

import dataclasses
import sys as _sys
from typing import Any, Mapping

# ---------------------------------------------------------------------------
# Cross-module resolution
#
# ``memory_policy`` and ``memory_dedupe`` are loaded via importlib
# under canonical sys.modules names. We resolve them lazily so this
# module can also be imported standalone (the fallback loader mirrors
# the one in memory_policy).
# ---------------------------------------------------------------------------


def _resolve(name: str, fallback_path: str):
    module = _sys.modules.get(name)
    if module is not None:
        return module
    import importlib.util as _il
    from pathlib import Path as _Path

    _here = _Path(__file__).resolve().parent
    _spec = _il.spec_from_file_location(name, _here / fallback_path)
    if _spec is None or _spec.loader is None:
        raise RuntimeError(f"failed to resolve {fallback_path} for memory.py")
    module = _il.module_from_spec(_spec)
    _sys.modules[name] = module
    _spec.loader.exec_module(module)
    return module


def _policy():
    return _resolve("_nowledge_mem_ralph_memory_policy", "memory_policy.py")


def _dedupe():
    return _resolve("_nowledge_mem_ralph_memory_dedupe", "memory_dedupe.py")


def _evaluator():
    return _resolve("_nowledge_mem_ralph_memory_evaluator", "memory_evaluator.py")


# ---------------------------------------------------------------------------
# Result dataclass
# ---------------------------------------------------------------------------


@dataclasses.dataclass(frozen=True)
class SaveResult:
    """Outcome of :func:`save`.

    Mirrors the verdict produced by ``memory_policy.evaluate`` plus
    the fields the caller needs to decide what to do next:

    * ``memory_digest`` — set for ``ACCEPTED`` (and ``ALREADY_SAVED``,
      see below) so the writer (U04) can pick up the canonical
      idempotency key. Empty for ``REJECTED`` / ``NEEDS_REWRITE`` /
      ``OBSERVATION``.
    * ``missing_fields`` — the same list ``Verdict.missing_fields``
      carries, surfaced verbatim so the agent does not have to parse
      the reason string.
    * ``rewrite_suggestion`` — human-readable hint surfaced from the
      verdict.
    * ``policy_version`` — version of the policy that produced the
      verdict; U04 records this with every write for drift
      detection.
    * ``record`` — the structured ``ACCEPTED`` record when
      ``result == ACCEPTED``. Downstream code can serialise it
      directly. ``None`` for non-ACCEPTED verdicts (the writer must
      never see them).
    * ``memory_id`` — always ``None`` at the U03 boundary because
      U03 does not write to ``nmem``. U04 sets it.
    """

    result: str
    reason: str
    missing_fields: tuple[str, ...]
    rewrite_suggestion: str
    policy_version: str
    memory_digest: str
    record: Mapping[str, Any] | None
    memory_id: str | None

    @classmethod
    def from_verdict(
        cls,
        verdict,
        digest: str,
        record: Mapping[str, Any] | None,
    ) -> "SaveResult":
        return cls(
            result=verdict.result,
            reason=verdict.reason,
            missing_fields=verdict.missing_fields,
            rewrite_suggestion=verdict.rewrite_suggestion,
            policy_version=verdict.policy_version,
            memory_digest=digest,
            record=record,
            memory_id=None,
        )


# ---------------------------------------------------------------------------
# Save entry
# ---------------------------------------------------------------------------


def _accepted_record(
    candidate: Mapping[str, Any], digest: str, source: Mapping[str, Any] | None
) -> dict[str, Any]:
    """Build the ACCEPTED record handed off to U04.

    The record is a shallow copy of the candidate plus the digest and
    source metadata so the writer does not have to re-resolve
    anything. Source defaults to an empty mapping when the caller
    does not pass any (the entry does not require a hat name).
    """
    record: dict[str, Any] = dict(candidate)
    record["memory_digest"] = digest
    record["source"] = dict(source or {})
    return record


def save(
    candidate: Mapping[str, Any],
    source: Mapping[str, Any] | None = None,
) -> SaveResult:
    """Evaluate ``candidate`` and return a :class:`SaveResult`.

    The function does not write to ``nmem``; the writer (U04) is the
    sole owner of that side-effect. The result is always a
    ``SaveResult`` (never a raised exception) so the save-memory
    command can always emit a verdict to stdout.
    """
    if not isinstance(candidate, Mapping):
        # ``memory_schema`` is loaded by ``memory_policy`` via importlib;
        # pull the canonical field set from there rather than a relative
        # import (the latter fails under the importlib loader).
        schema_mod = _sys.modules.get("_nowledge_mem_ralph_memory_schema")
        if schema_mod is None:
            schema_mod = _resolve(
                "_nowledge_mem_ralph_memory_schema", "memory_schema.py"
            )
        verdict = _policy().Verdict.rejected(
            reason="candidate must be a JSON object",
            missing_fields=tuple(sorted(schema_mod.REQUIRED_FIELDS)),
            rewrite_suggestion="submit a JSON object with the fixed schema",
        )
        return SaveResult.from_verdict(verdict, digest="", record=None)

    digest = _dedupe().compute_memory_digest(candidate)
    verdict = _policy().evaluate(candidate)

    if verdict.result == "ACCEPTED" and candidate.get("semantic_review") is True:
        evaluator_result, evaluator_reason, _details = _evaluator().evaluate(candidate)
        if evaluator_result != "ACCEPTED":
            evaluated = _policy().Verdict(
                result=evaluator_result,
                reason=evaluator_reason or "semantic evaluator rejected the candidate",
                missing_fields=(),
                rewrite_suggestion="review the evaluator reasons and resubmit only after correction",
                policy_version=verdict.policy_version,
            )
            return SaveResult.from_verdict(evaluated, digest=digest, record=None)

    # Dedupe short-circuit only fires for ACCEPTED candidates. A
    # rejected candidate still gets a verdict returned (and the
    # caller can fix and resubmit); we do not want to silently
    # swallow rejections.
    if verdict.result == "ACCEPTED":
        scope = candidate.get("scope", "")
        scope_key = scope if isinstance(scope, str) else ""
        if _dedupe().is_already_saved(digest, scope=scope_key):
            already_verdict = _policy().Verdict(
                result="OBSERVATION",
                reason="digest already accepted in this scope (dedupe signal)",
                missing_fields=(),
                rewrite_suggestion=(
                    "if this is a different observation, change the scope "
                    "or refine the claim so the digest differs"
                ),
                policy_version=verdict.policy_version,
            )
            return SaveResult.from_verdict(already_verdict, digest=digest, record=None)

    record: Mapping[str, Any] | None = None
    if verdict.result == "ACCEPTED":
        record = _accepted_record(candidate, digest=digest, source=source)
        # Record the dedupe signal so a follow-up call with the same
        # digest returns OBSERVATION rather than re-running the
        # evaluation pipeline. U04 may replace this with a durable
        # ledger but the contract stays the same.
        scope = candidate.get("scope", "")
        scope_key = scope if isinstance(scope, str) else ""
        _dedupe().record_save(digest, scope=scope_key)

    return SaveResult.from_verdict(verdict, digest=digest if verdict.result == "ACCEPTED" else "", record=record)


# ---------------------------------------------------------------------------
# CLI bridge (called from commands/save-memory.md)
# ---------------------------------------------------------------------------


def _read_stdin() -> str:
    return _sys.stdin.read()


def _source_from_environment() -> dict[str, str]:
    """Capture only bounded Ralph metadata, never prompt or transcript text."""
    keys = {
        "loop_id": "RALPH_CURRENT_LOOP_ID",
        "hat": "RALPH_CURRENT_HAT",
        "wave": "RALPH_WAVE",
        "attempt": "RALPH_ATTEMPT",
        "repo": "RALPH_WORKSPACE_ROOT",
    }
    import os as _os
    return {name: _os.environ.get(env_key, "").strip()[:300] for name, env_key in keys.items() if _os.environ.get(env_key, "").strip()}


def _load_writer():
    writer_name = "_nowledge_mem_ralph_memory_writer"
    writer = _sys.modules.get(writer_name)
    if writer is not None:
        return writer
    import importlib.util as _il
    from pathlib import Path as _Path
    _path = _Path(__file__).resolve().parent / "memory_writer.py"
    _spec = _il.spec_from_file_location(writer_name, _path)
    if _spec is None or _spec.loader is None:
        raise RuntimeError("failed to load memory_writer.py")
    writer = _il.module_from_spec(_spec)
    _sys.modules[writer_name] = writer
    _spec.loader.exec_module(writer)
    return writer


def main() -> int:
    """Read candidate JSON from stdin and print the verdict to stdout.

    Exit code contract (mirrors hook_runtime.py):

    * ``0`` — verdict is ACCEPTED, REJECTED, NEEDS_REWRITE, or
      OBSERVATION; the verdict JSON is on stdout.
    * ``1`` — stdin payload could not be parsed as JSON. Nothing is
      written to stdout; the reason is on stderr.

    The save-memory command surfaces this exit code to the agent;
    non-zero means the command itself failed (not that the candidate
    was rejected).
    """
    import json as _json

    raw = _read_stdin()
    try:
        candidate = _json.loads(raw)
    except Exception as exc:
        _sys.stderr.write(f"save-memory: failed to parse stdin as JSON: {exc}\n")
        return 1

    source = _source_from_environment()
    # Scope is part of the candidate's explicit schema; source metadata is
    # plugin-owned and comes from the current Ralph activation environment.
    result = save(candidate, source=source)
    write_result = None
    if result.result == "ACCEPTED" and result.record is not None:
        # Resolve U04 lazily so the pure U03 ``save`` API remains side-effect
        # free and testable. The command boundary owns the actual write.
        writer = _load_writer()
        write_result = writer.write_from_save_result(result)
        if write_result.result in {"FAILED_OPEN", "UNKNOWN"}:
            scope = candidate.get("scope", "")
            _dedupe().forget_save(result.memory_digest, scope=scope if isinstance(scope, str) else "")
    else:
        # Stop/SubagentStop must be able to report deterministic rejections.
        _load_writer().record_policy_result(result, source)
    payload = {
        "result": result.result,
        "reason": result.reason,
        "missing_fields": list(result.missing_fields),
        "rewrite_suggestion": result.rewrite_suggestion,
        "policy_version": result.policy_version,
        "memory_digest": result.memory_digest,
        "memory_id": result.memory_id,
        "record": result.record,
        "write": write_result.as_dict() if write_result is not None else None,
    }
    _sys.stdout.write(_json.dumps(payload, ensure_ascii=False))
    _sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
