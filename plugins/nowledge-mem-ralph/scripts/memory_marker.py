"""Bounded finalization marker parser for the Stop / SubagentStop hook.

Claude can flag a final assistant message as a finalization candidate by
embedding exactly one HTML-comment fenced block:

    <!-- nowledge-memory-finalize
    {"finalize": true, "memory_type": "durable_decision", ...}
    -->

The parser enforces:

* **Single marker** — at most one fenced block per message; a second
  block is rejected.
* **Fixed name** — the marker tag must be ``nowledge-memory-finalize``.
  Any other marker name (or absent marker) is a skip.
* **UTF-8 byte cap** — the payload between the markers is bounded by
  :data:`MAX_MARKER_BYTES` (16 KiB). Larger payloads are rejected
  before they can inflate state files or audit logs.
* **JSON object only** — the payload must parse as a JSON object
  (lists / scalars / primitives are rejected).
* **finalize:true** — the object must explicitly set ``finalize`` to the
  JSON boolean ``true``. Anything else is treated as a non-final
  message and skipped.

The parser never opens ``transcript_path`` and never reads any file —
it only consumes the string passed by the caller. Output is always a
:func:`ParserResult` (never a raised exception) so the hook can stay
fail-open.
"""

from __future__ import annotations

import dataclasses
import json
from typing import Any, Mapping


MARKER_NAME = "nowledge-memory-finalize"
MAX_MARKER_BYTES = 16 * 1024

# ``finalize`` must be the JSON boolean ``true`` — not truthy, not "true".
_FINALIZE_REQUIRED = True


@dataclasses.dataclass(frozen=True)
class ParserResult:
    """Bounded outcome of a single :func:`extract_finalization_marker` call.

    * ``status`` — ``"PARSED"`` only when a legal candidate was extracted;
      ``"SKIPPED"`` when the message had no finalization intent;
      ``"REJECTED"`` when the marker was present but malformed.
    * ``candidate`` — the parsed JSON object (without the marker
      wrapper). Always ``None`` for non-``PARSED`` outcomes so callers
      cannot accidentally persist a partial payload.
    * ``memory_digest`` — a stable hex digest of the candidate
      (empty for non-``PARSED`` outcomes). Distinct from the writer's
      memory_digest (which is computed by ``memory_dedupe``) — this
      digest is the audit-log identifier for the candidate.
    * ``reason`` — human-readable summary surfaced to the audit log.
    """

    status: str
    candidate: Mapping[str, Any] | None
    memory_digest: str
    reason: str


def _stable_digest(payload: Mapping[str, Any]) -> str:
    """Compute a stable digest over the candidate JSON payload.

    We use a sorted-key JSON dump so structurally equal candidates
    always hash identically, then SHA-256 hex for the audit log.
    Imports are local to avoid taking on extra dependencies for a
    single hashing call.
    """
    import hashlib

    encoded = json.dumps(payload, ensure_ascii=False, sort_keys=True).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _extract_block(text: str) -> tuple[int, int, str] | None:
    """Return ``(start, end, inner)`` for the single marker block, or None.

    The block is the substring between the opening marker line and the
    closing ``-->`` line that immediately follows. The function returns
    ``None`` when no opening marker is present or when more than one
    opening marker is present (the second occurrence is a contract
    violation).
    """
    open_marker = f"<!-- {MARKER_NAME}"
    close_marker = "-->"
    open_index = text.find(open_marker)
    if open_index == -1:
        return None
    # The marker must not appear more than once in a single message.
    second_index = text.find(open_marker, open_index + 1)
    if second_index != -1:
        return None
    # Skip past the opening tag to the first newline so we capture only
    # the JSON body, not the wrapper text.
    open_end = text.find("\n", open_index)
    if open_end == -1:
        return None
    close_index = text.find(close_marker, open_end + 1)
    if close_index == -1:
        return None
    inner = text[open_end + 1 : close_index]
    return open_index, close_index + len(close_marker), inner


def extract_finalization_marker(message: Any) -> ParserResult:
    """Return the bounded finalization candidate from ``message``.

    The function accepts any value type and returns a :class:`ParserResult`
    without raising. Non-string messages, missing markers, malformed
    JSON, duplicate markers, oversized payloads, and ``finalize`` not
    equal to ``True`` all map to a non-``PARSED`` outcome with a
    descriptive reason; callers should treat them as no-ops.
    """
    if not isinstance(message, str):
        return ParserResult(
            status="SKIPPED",
            candidate=None,
            memory_digest="",
            reason="last_assistant_message is not a string",
        )

    block = _extract_block(message)
    if block is None:
        return ParserResult(
            status="SKIPPED",
            candidate=None,
            memory_digest="",
            reason="no finalization marker found in message",
        )

    _, _, inner = block
    # Bound by UTF-8 byte length to prevent unbounded state growth.
    encoded = inner.encode("utf-8")
    if len(encoded) > MAX_MARKER_BYTES:
        return ParserResult(
            status="REJECTED",
            candidate=None,
            memory_digest="",
            reason=(
                f"finalization marker payload exceeds {MAX_MARKER_BYTES} bytes"
            ),
        )

    try:
        parsed = json.loads(inner)
    except json.JSONDecodeError as exc:
        return ParserResult(
            status="REJECTED",
            candidate=None,
            memory_digest="",
            reason=f"finalization marker body is not valid JSON: {exc.msg}",
        )

    if not isinstance(parsed, dict):
        return ParserResult(
            status="REJECTED",
            candidate=None,
            memory_digest="",
            reason="finalization marker body must be a JSON object",
        )

    if parsed.get("finalize") is not _FINALIZE_REQUIRED:
        return ParserResult(
            status="SKIPPED",
            candidate=None,
            memory_digest="",
            reason=(
                "finalization marker present but finalize is not true "
                f"(got {parsed.get('finalize')!r})"
            ),
        )

    return ParserResult(
        status="PARSED",
        candidate=dict(parsed),
        memory_digest=_stable_digest(parsed),
        reason="finalization marker parsed",
    )