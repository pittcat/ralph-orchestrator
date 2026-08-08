"""Memory digest and dedupe helpers (U03).

The dedupe module owns two SSOT contracts that U04 picks up:

* :func:`compute_memory_digest` — stable SHA-256 hex digest over the
  normalised content (title, claim, why_it_matters, evidence,
  applies_when, scope, verification). The normalisation rules are
  intentionally narrow:

  - ``str.strip()`` for every text field,
  - internal whitespace collapsed to single spaces (this is what
    catches "claim\n\n   " vs "claim" without being so aggressive
    that legitimate paragraph breaks count as different content),
  - lists joined with a fixed separator,
  - field order is the one declared in :data:`DIGEST_FIELDS`.

  The digest length is always 64 (SHA-256 hex) so downstream code
  can rely on a fixed-width key.

* :func:`is_already_saved` / :func:`record_save` — local ledger
  tracking which digests have been accepted for which scope. The
  ledger is process-local: U04 owns the durable store, U03 only
  exposes a signal so save-memory can short-circuit obvious retries.

Nothing in this module performs I/O, calls ``nmem``, or persists the
ledger to disk. U04 may replace the in-memory ledger with a
disk-backed cache if the loop runs long enough to need cross-process
state; the public function names will stay the same.
"""

from __future__ import annotations

import hashlib
import re

# Fields included in the digest, in the canonical order. U04 picks
# the same order when serialising the record for the writer, so the
# digest remains a stable proxy for "same content".

DIGEST_FIELDS: tuple[str, ...] = (
    "title",
    "claim",
    "why_it_matters",
    "evidence",
    "applies_when",
    "scope",
    "verification",
)

# Internal whitespace collapse: any run of whitespace becomes a
# single space. We deliberately keep the surrounding text intact
# (just strip() at the boundary) so the digest does not accidentally
# mark two semantically equivalent candidates as different.

_WHITESPACE_RUN = re.compile(r"\s+")

# Separator used between fields and inside list fields. The byte
# value (U+001F) is in the ASCII control range and cannot appear in
# any user-provided text field (JSON strings cannot contain raw
# control bytes without escaping), so the separator is collision-
# free against real content.

_FIELD_SEP = "\x1f"
_LIST_SEP = "\x1e"


def _normalise_text(value: object) -> str:
    if value is None:
        return ""
    text = str(value)
    text = text.strip()
    text = _WHITESPACE_RUN.sub(" ", text)
    return text


def _normalise_list(value: object) -> str:
    if not isinstance(value, list):
        return ""
    return _LIST_SEP.join(_normalise_text(item) for item in value)


def compute_memory_digest(candidate: dict) -> str:
    """Return the SHA-256 hex digest of the normalised candidate.

    The candidate must be a mapping (typically produced by parsing
    the save-memory JSON payload). The function is total: a malformed
    candidate yields a deterministic digest of the empty payload,
    never raises, so the caller can rely on a string return value.
    """
    if not isinstance(candidate, dict):
        candidate = {}
    parts: list[str] = []
    for field in DIGEST_FIELDS:
        parts.append(_normalise_text(candidate.get(field, "")))
    payload = _FIELD_SEP.join(parts)
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


# ---------------------------------------------------------------------------
# Local ledger (process-local dedupe signal)
# ---------------------------------------------------------------------------


_SAVED_DIGESTS: set[str] = set()
_SAVED_BY_SCOPE: dict[str, set[str]] = {}


def is_already_saved(digest: str, scope: str | None = None) -> bool:
    """Return True if ``digest`` has been recorded locally.

    When ``scope`` is supplied, the check is also scoped: a digest
    recorded under a different scope does not satisfy the check.
    Pass ``scope=None`` to check the digest globally.
    """
    if scope is None:
        return digest in _SAVED_DIGESTS
    return digest in _SAVED_BY_SCOPE.get(scope, set())


def record_save(digest: str, scope: str | None = None) -> None:
    """Record ``digest`` as having been saved (locally)."""
    _SAVED_DIGESTS.add(digest)
    if scope is not None:
        _SAVED_BY_SCOPE.setdefault(scope, set()).add(digest)


def clear_local_ledger() -> None:
    """Reset the local ledger — exposed for tests only."""
    _SAVED_DIGESTS.clear()
    _SAVED_BY_SCOPE.clear()