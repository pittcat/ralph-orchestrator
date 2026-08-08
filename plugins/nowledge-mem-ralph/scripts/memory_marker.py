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
* **Context guard (fix U3 adversarial:A3)** — markers appearing inside
  fenced code blocks (`````…`````) or markdown blockquote lines
  (``> …``) are rejected as quoted/attacker content. The guard walks
  line-by-line from the start of the message to the marker open tag,
  tracking fenced-block state.

The parser never opens ``transcript_path`` and never reads any file —
it only consumes the string passed by the caller. Output is always a
:func:`ParserResult` (never a raised exception) so the hook can stay
fail-open.
"""

from __future__ import annotations

import dataclasses
import json
import re
from typing import Any, Mapping


MARKER_NAME = "nowledge-memory-finalize"
MAX_MARKER_BYTES = 16 * 1024

# ``finalize`` must be the JSON boolean ``true`` — not truthy, not "true".
_FINALIZE_REQUIRED = True

# Exact-tag regex (fix U5 correctness:C1). The opening tag must be
# followed by either a newline (canonical body separator) or by the
# ``>`` that closes an inline tag. Variants such as
# ``nowledge-memory-finalize-v2`` or ``nowledge-memory-finalize-debug``
# do not match because they are followed by additional characters
# that are not in the allowed set.
_OPEN_TAG_RE = re.compile(rf"<!--\s*{re.escape(MARKER_NAME)}\s*(?=[\n>])")
_CLOSE_MARKER = "-->"

# Hat allowlist (fix U3 adversarial:A3). Read-only reviewer hats must
# never trigger auto-finalization. The allowlist is enforced by
# ``memory_finalization.run_finalization`` (the parser itself stays
# agnostic so it remains reusable for non-Ralph entrypoints).
ALLOWED_FINALIZATION_HATS = (
    r"^executor$",
    r"^test-stabilizer$",
    r"^fixer$",
)


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
    ``None`` when no opening marker is present. A second opening marker
    is reported via :class:`DuplicateMarker` so the caller can map it
    to a ``REJECTED`` outcome instead of silently picking one.
    """
    match = _OPEN_TAG_RE.search(text)
    if match is None:
        return None
    # The marker must not appear more than once in a single message.
    second = _OPEN_TAG_RE.search(text, match.end())
    if second is not None:
        raise DuplicateMarker
    open_index = match.start()
    # Skip past the opening tag to the first newline so we capture only
    # the JSON body, not the wrapper text.
    open_end = text.find("\n", open_index)
    if open_end == -1:
        return None
    close_index = text.find(_CLOSE_MARKER, open_end + 1)
    if close_index == -1:
        return None
    inner = text[open_end + 1 : close_index]
    return open_index, close_index + len(_CLOSE_MARKER), inner


class DuplicateMarker(Exception):
    """Raised when more than one ``nowledge-memory-finalize`` tag is present."""


def _line_is_inside_fence(text: str, open_index: int) -> bool:
    """Return True when the line at ``open_index`` lives inside a code fence.

    Walks lines from the start of the message to ``open_index``,
    counting triple-backtick fences as they open and close. A line is
    "inside a fence" when its open-count exceeds its close-count at
    that point. The check is intentionally simple — Claude may emit
    ``text`` or no-language fences (`` ``` ``), and both must trip
    the guard.
    """
    fence_re = re.compile(r"^\s*(```+|~~~+)")
    inside = False
    for line in text.splitlines():
        if fence_re.match(line):
            inside = not inside
            continue
        if line.find("```") != -1 or line.find("~~~") != -1:
            # Inline fences on a single line still toggle state.
            inside = not inside
        # Check whether ``open_index`` is past this line.
        if open_index <= len(line):
            break
        open_index -= len(line) + 1
    return inside


def _line_is_blockquote(text: str, open_index: int) -> bool:
    """Return True when the line containing ``open_index`` starts with ``>``."""
    line_start = text.rfind("\n", 0, open_index) + 1
    line_end = text.find("\n", open_index)
    if line_end == -1:
        line_end = len(text)
    line = text[line_start:line_end]
    return line.lstrip().startswith(">")


def extract_finalization_marker(message: Any) -> ParserResult:
    """Return the bounded finalization candidate from ``message``.

    The function accepts any value type and returns a :class:`ParserResult`
    without raising. Non-string messages, missing markers, malformed
    JSON, duplicate markers, oversized payloads, markers inside code
    fences or blockquotes, and ``finalize`` not equal to ``True`` all
    map to a non-``PARSED`` outcome with a descriptive reason; callers
    should treat them as no-ops.
    """
    if not isinstance(message, str):
        return ParserResult(
            status="SKIPPED",
            candidate=None,
            memory_digest="",
            reason="last_assistant_message is not a string",
        )

    try:
        block = _extract_block(message)
    except DuplicateMarker:
        return ParserResult(
            status="REJECTED",
            candidate=None,
            memory_digest="",
            reason="duplicate marker: more than one finalization marker in message",
        )
    if block is None:
        return ParserResult(
            status="SKIPPED",
            candidate=None,
            memory_digest="",
            reason="no finalization marker found in message",
        )

    open_index, _, inner = block
    if _line_is_inside_fence(message, open_index):
        return ParserResult(
            status="REJECTED",
            candidate=None,
            memory_digest="",
            reason="marker is inside fenced code block",
        )
    if _line_is_blockquote(message, open_index):
        return ParserResult(
            status="REJECTED",
            candidate=None,
            memory_digest="",
            reason="marker is inside blockquote",
        )

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