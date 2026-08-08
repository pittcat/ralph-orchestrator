"""Memory schema (U03).

This module is the **single source of truth** for the fixed Memory
schema. Downstream modules (``memory_policy`` / ``memory_dedupe`` /
``memory``) must read the canonical field set from
:data:`REQUIRED_FIELDS` and :data:`REQUIRED_METRICS` instead of
hard-coding names; this prevents the kind of silent drift that
earlier iterations had between schema definition and policy
evaluation.

Schema rules:

* Required fields (see :data:`REQUIRED_FIELDS`) must be present and
  non-empty strings (or non-empty lists, depending on the field).
* Required metrics (see :data:`REQUIRED_METRICS`) must all be present
  as integer/float values in the inclusive range [0, 100].
* ``memory_type`` is one of the high-value content kinds; the four
  disallowed kinds (progress / log / command / transcript) are
  surfaced via :data:`DISALLOWED_CONTENT_TYPES` so policy can pick
  them up without re-typing the literal list.
* ``title`` has a minimum length so the schema rejects accidental
  empty or stub titles.

Nothing in this module performs I/O, calls ``nmem``, or knows about
the loop cache. It is intentionally pure-Python so it can be reused
in offline tests and inside the writer (U04).
"""

from __future__ import annotations

import dataclasses
from typing import Mapping

# ---------------------------------------------------------------------------
# Fixed Memory schema field set
# ---------------------------------------------------------------------------
#
# This frozen set is the SSOT. U04 writer, U05 evaluator, and U06 E2E
# must read it from here; renaming a field requires a coordinated
# schema version bump in ``memory_policy.POLICY_VERSION``.

REQUIRED_FIELDS: frozenset[str] = frozenset(
    {
        "memory_type",
        "title",
        "claim",
        "why_it_matters",
        "evidence",
        "applies_when",
        "scope",
        "verification",
        "critical_assumptions",
        "critical_ambiguities",
        "metrics",
    }
)

# Seven fixed quality metrics. The names are part of the public
# contract; renaming a metric is a breaking change.

REQUIRED_METRICS: frozenset[str] = frozenset(
    {
        "confidence",
        "evidence_coverage",
        "reusability",
        "stability",
        "scope_clarity",
        "verifiability",
        "novelty",
    }
)

# Hard-gate content types. ``memory_policy`` rejects these even when
# the metrics are pristine — the goal of save-memory is durable
# knowledge, not raw process state.

DISALLOWED_CONTENT_TYPES: frozenset[str] = frozenset(
    {"progress", "log", "command", "transcript"}
)

# Allowed ``memory_type`` values for accepted records. The list is
# intentionally narrow; "durable_decision" is the canonical one and
# the others cover procedurally reusable knowledge.

ALLOWED_MEMORY_TYPES: frozenset[str] = frozenset(
    {
        "durable_decision",
        "durable_procedure",
        "durable_root_cause",
        "durable_constraint",
    }
)

# Title must convey an actual claim; the lower bound is generous so
# it does not block stub candidates during early development.

MIN_TITLE_LENGTH = 8

# Metric value bounds. All seven metrics are scored 0..100.

METRIC_MIN = 0
METRIC_MAX = 100


@dataclasses.dataclass(frozen=True)
class MemorySchema:
    """Lightweight wrapper exposing schema constants as a dataclass.

    The dataclass form is convenient for ``memory_policy`` and for
    documentation tooling; the canonical field set still lives in
    :data:`REQUIRED_FIELDS` so this wrapper cannot drift from the
    SSOT.
    """

    required_fields: frozenset[str] = REQUIRED_FIELDS
    required_metrics: frozenset[str] = REQUIRED_METRICS
    disallowed_content_types: frozenset[str] = DISALLOWED_CONTENT_TYPES
    allowed_memory_types: frozenset[str] = ALLOWED_MEMORY_TYPES


def _is_non_empty_string(value: object) -> bool:
    return isinstance(value, str) and bool(value.strip())


def _is_list_of_strings(value: object) -> bool:
    if not isinstance(value, list):
        return False
    return all(isinstance(item, str) for item in value)


def validate_memory_schema(candidate: Mapping[str, object] | None) -> list[str]:
    """Return a sorted list of missing/invalid field names.

    An empty return value means the candidate passes schema
    validation. The returned names match the canonical field set
    exactly so callers can render them as ``missing_fields`` or use
    them to drive a rewrite suggestion.
    """
    if not isinstance(candidate, Mapping):
        return sorted(REQUIRED_FIELDS)

    errors: list[str] = []

    # Field presence + non-emptiness. Strings must be non-empty after
    # stripping; lists (assumptions / ambiguities) must be present
    # (the list may be empty, but the key must exist).
    for field in REQUIRED_FIELDS:
        if field not in candidate:
            errors.append(field)
            continue
        value = candidate[field]
        if field == "metrics":
            if not isinstance(value, Mapping):
                errors.append(field)
            continue
        if field in {"critical_assumptions", "critical_ambiguities"}:
            if not _is_list_of_strings(value):
                errors.append(field)
            continue
        if not _is_non_empty_string(value):
            errors.append(field)
            continue
        if field == "title" and len(value.strip()) < MIN_TITLE_LENGTH:
            errors.append(field)

    # Metrics: every required metric must be present and numeric
    # within [METRIC_MIN, METRIC_MAX].
    metrics = candidate.get("metrics") if isinstance(candidate, Mapping) else None
    if isinstance(metrics, Mapping):
        for metric in REQUIRED_METRICS:
            if metric not in metrics:
                errors.append(metric)
                continue
            raw = metrics[metric]
            if isinstance(raw, bool) or not isinstance(raw, (int, float)):
                errors.append(metric)
                continue
            if not (METRIC_MIN <= float(raw) <= METRIC_MAX):
                errors.append(metric)

    return sorted(set(errors))