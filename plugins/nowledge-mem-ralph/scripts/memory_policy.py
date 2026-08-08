"""Memory policy (U03).

The policy module owns three deterministic gates between an incoming
Memory candidate and the writer (U04):

1. **Hard gates** — content_type in
   ``memory_schema.DISALLOWED_CONTENT_TYPES`` is rejected
   unconditionally, regardless of how strong the rest of the
   candidate looks. This blocks progress/log/command/transcript
   records from polluting the knowledge store.
2. **Schema** — every required field and every required metric must
   be present and well-typed.
3. **Quality thresholds** — confidence, evidence_coverage,
   reusability, stability, scope_clarity, verifiability, novelty
   each have to clear a floor. The most consequential rule is the
   anti-hallucination gate: confidence ≥ 90 with
   evidence_coverage < 70 is rejected because high confidence with
   thin evidence is the canonical hallucination shape.
4. **Critical assumption / ambiguity** — any non-empty value in
   either list turns the verdict into NEEDS_REWRITE; the candidate
   is recorded locally but never reaches the writer.

The verdict is a frozen dataclass so downstream code (memory.save /
writer / evaluator) can rely on immutability. The policy itself is
versioned via :data:`POLICY_VERSION` so U04 can detect drift if a
future iteration relaxes a threshold.

The thresholds are deliberate trade-offs:

* ``MIN_CONFIDENCE = 80`` — high enough to filter "I'm not sure"
  guesses, low enough that a useful early-stage lesson is not
  blocked.
* ``MIN_EVIDENCE_COVERAGE = 70`` — anything weaker than this and
  the candidate is at risk of hallucination.
* ``MIN_CONFIDENCE_FOR_LOW_EVIDENCE = 90`` — only this aggressive
  confidence score can compensate for evidence_coverage below 70,
  and even then the anti-hallucination rule above still fires.
* ``MIN_REUSABILITY = 50`` — half the metric range, a candidate
  below this is usually a one-shot workaround.
* ``MIN_VERIFIABILITY = 50`` — without an external reproducer the
  record ages badly.
* ``MIN_NOVELTY = 20`` — a candidate below this is almost always
  duplicating an existing memory; let dedupe catch it instead.
* ``MIN_STABILITY = 60`` — borderline suggestions should not be
  promoted to Memory.
* ``MIN_SCOPE_CLARITY = 70`` — a vague "applies to all" record
  poisons search results.
"""

from __future__ import annotations

import dataclasses
import datetime as _dt
from typing import Mapping

import sys as _sys

# ``memory_schema`` is loaded via importlib.util under the canonical
# name ``_nowledge_mem_ralph_memory_schema``; reference it from
# ``sys.modules`` rather than a relative import (the latter fails
# because there is no parent package — the plugin scripts/ dir is not
# a regular Python package, just a directory).

_memory_schema_module = _sys.modules.get("_nowledge_mem_ralph_memory_schema")
if _memory_schema_module is None:
    # Defensive fallback: load memory_schema.py from the same dir if
    # the importlib chain above hasn't already done so. This keeps
    # the module usable in isolation (e.g. ``python -m memory_policy``).
    import importlib.util as _il
    from pathlib import Path as _Path

    _here = _Path(__file__).resolve().parent
    _spec = _il.spec_from_file_location(
        "_nowledge_mem_ralph_memory_schema",
        _here / "memory_schema.py",
    )
    if _spec is None or _spec.loader is None:
        raise RuntimeError("failed to locate memory_schema.py alongside memory_policy.py")
    _memory_schema_module = _il.module_from_spec(_spec)
    _sys.modules["_nowledge_mem_ralph_memory_schema"] = _memory_schema_module
    _spec.loader.exec_module(_memory_schema_module)


def _disallowed_content_types():
    return getattr(_memory_schema_module, "DISALLOWED_CONTENT_TYPES")


def _validate_memory_schema(candidate):
    return getattr(_memory_schema_module, "validate_memory_schema")(candidate)


# ---------------------------------------------------------------------------
# Policy version
# ---------------------------------------------------------------------------


# Bump POLICY_VERSION when changing any threshold below or adding new
# gates. U04 records the version with every write so future readers
# can detect drift and re-evaluate if the rules were tightened.

POLICY_VERSION = "0.3.0-memory-schema-and-policy"


# ---------------------------------------------------------------------------
# Thresholds
# ---------------------------------------------------------------------------


MIN_CONFIDENCE = 80
MIN_EVIDENCE_COVERAGE = 70
MIN_REUSABILITY = 50
MIN_VERIFIABILITY = 50
MIN_NOVELTY = 20
MIN_STABILITY = 60
MIN_SCOPE_CLARITY = 70

# Anti-hallucination gate. A confidence at or above this floor with
# evidence_coverage below the floor is treated as a high-confidence
# hallucination and rejected outright.

HIGH_CONFIDENCE_FLOOR = 90
LOW_EVIDENCE_CEILING = 70


# ---------------------------------------------------------------------------
# Verdict
# ---------------------------------------------------------------------------


@dataclasses.dataclass(frozen=True)
class Verdict:
    """Result of evaluating one Memory candidate.

    ``result`` is one of:

    * ``ACCEPTED`` — passes every gate; eligible for the writer.
    * ``REJECTED`` — fails one or more hard gates or thresholds; do
      not write.
    * ``NEEDS_REWRITE`` — candidate is structurally valid but has
      unresolved assumptions or ambiguities; do not write.
    * ``OBSERVATION`` — reserved for future use; U03 emits it only
      when ``memory_type`` is recognised but explicitly marked as
      observation-style by the caller (currently unused).

    ``reason`` is a single, human-readable string. ``missing_fields``
    lists the field names that drove a REJECTED or NEEDS_REWRITE
    verdict. ``rewrite_suggestion`` is a free-form hint that the
    ``save-memory`` command / skill surfaces back to the caller.
    """

    result: str
    reason: str
    missing_fields: tuple[str, ...]
    rewrite_suggestion: str
    policy_version: str

    @classmethod
    def accepted(cls) -> "Verdict":
        return cls(
            result="ACCEPTED",
            reason="all gates passed",
            missing_fields=(),
            rewrite_suggestion="",
            policy_version=POLICY_VERSION,
        )

    @classmethod
    def rejected(
        cls,
        reason: str,
        missing_fields: tuple[str, ...] = (),
        rewrite_suggestion: str = "",
    ) -> "Verdict":
        return cls(
            result="REJECTED",
            reason=reason,
            missing_fields=missing_fields,
            rewrite_suggestion=rewrite_suggestion,
            policy_version=POLICY_VERSION,
        )

    @classmethod
    def needs_rewrite(
        cls,
        reason: str,
        missing_fields: tuple[str, ...] = (),
        rewrite_suggestion: str = "",
    ) -> "Verdict":
        return cls(
            result="NEEDS_REWRITE",
            reason=reason,
            missing_fields=missing_fields,
            rewrite_suggestion=rewrite_suggestion,
            policy_version=POLICY_VERSION,
        )


# ---------------------------------------------------------------------------
# Gate runners
# ---------------------------------------------------------------------------


def _hard_gate(candidate: Mapping[str, object]) -> Verdict | None:
    content_type = candidate.get("memory_type")
    if not isinstance(content_type, str):
        return Verdict.rejected(
            reason="memory_type must be a string",
            missing_fields=("memory_type",),
            rewrite_suggestion="set memory_type to one of the durable_* values",
        )
    if content_type in _disallowed_content_types():
        return Verdict.rejected(
            reason=(
                f"hard gate: memory_type={content_type!r} is in the "
                "disallowed set (progress/log/command/transcript); "
                "save-memory only accepts durable knowledge"
            ),
            missing_fields=("memory_type",),
            rewrite_suggestion=(
                "rewrite the candidate as a durable_decision or "
                "durable_procedure; raw process state does not belong in Memory"
            ),
        )
    return None


def _schema_gate(candidate: Mapping[str, object]) -> Verdict | None:
    errors = _validate_memory_schema(candidate)
    if errors:
        return Verdict.rejected(
            reason=f"schema validation failed: missing/invalid fields {errors}",
            missing_fields=tuple(errors),
            rewrite_suggestion=(
                "fill every required field with a non-empty value; "
                "see MemorySchema.REQUIRED_FIELDS for the canonical set"
            ),
        )
    return None


def _threshold_gate(candidate: Mapping[str, object]) -> Verdict | None:
    metrics = candidate.get("metrics") or {}
    # Anti-hallucination: high confidence + low evidence_coverage
    # is the canonical hallucination shape; reject even before the
    # other floors so the rule is obvious in the audit trail.
    if (
        metrics.get("confidence", 0) >= HIGH_CONFIDENCE_FLOOR
        and metrics.get("evidence_coverage", 0) < LOW_EVIDENCE_CEILING
    ):
        return Verdict.rejected(
            reason=(
                "anti-hallucination gate: confidence >= "
                f"{HIGH_CONFIDENCE_FLOOR} with evidence_coverage < "
                f"{LOW_EVIDENCE_CEILING}"
            ),
            missing_fields=("metrics.evidence_coverage",),
            rewrite_suggestion=(
                "either raise evidence_coverage above "
                f"{LOW_EVIDENCE_CEILING} with concrete reproducer steps, "
                "or lower confidence below "
                f"{HIGH_CONFIDENCE_FLOOR}"
            ),
        )

    floors: list[tuple[str, int]] = [
        ("confidence", MIN_CONFIDENCE),
        ("reusability", MIN_REUSABILITY),
        ("verifiability", MIN_VERIFIABILITY),
        ("novelty", MIN_NOVELTY),
        ("stability", MIN_STABILITY),
        ("scope_clarity", MIN_SCOPE_CLARITY),
        ("evidence_coverage", MIN_EVIDENCE_COVERAGE),
    ]
    below: list[str] = []
    for metric, floor in floors:
        if metrics.get(metric, 0) < floor:
            below.append(f"{metric}<{floor}")
    if below:
        return Verdict.rejected(
            reason="quality threshold(s) not met: " + ", ".join(sorted(below)),
            missing_fields=tuple(name.split("<")[0] for name in below),
            rewrite_suggestion=(
                "raise each failing metric above its floor or drop the "
                "candidate — Memory is for reusable knowledge, not workarounds"
            ),
        )
    return None


def _critical_gate(candidate: Mapping[str, object]) -> Verdict | None:
    assumptions = candidate.get("critical_assumptions") or []
    ambiguities = candidate.get("critical_ambiguities") or []
    if assumptions:
        return Verdict.needs_rewrite(
            reason=(
                f"critical_assumptions is non-empty ({len(assumptions)} "
                "entries); a confirmed Memory must not depend on "
                "unverified assumptions"
            ),
            missing_fields=("critical_assumptions",),
            rewrite_suggestion=(
                "either verify the assumptions inline or rewrite the "
                "claim so it does not depend on them"
            ),
        )
    if ambiguities:
        return Verdict.needs_rewrite(
            reason=(
                f"critical_ambiguities is non-empty ({len(ambiguities)} "
                "entries); Memory candidates must resolve scope ambiguity"
            ),
            missing_fields=("critical_ambiguities",),
            rewrite_suggestion=(
                "narrow the scope or resolve the ambiguity in the claim "
                "before saving"
            ),
        )
    return None


# ---------------------------------------------------------------------------
# Public entry point
# ---------------------------------------------------------------------------


def evaluate(candidate: Mapping[str, object]) -> Verdict:
    """Evaluate one Memory candidate.

    The function is pure: no I/O, no ``nmem`` invocation, no global
    state. It is safe to call from inside the hook (U03 boundary) and
    inside the writer (U04 idempotency check).
    """
    if not isinstance(candidate, Mapping):
        return Verdict.rejected(
            reason="candidate must be a JSON object",
            missing_fields=tuple(sorted(_memory_schema_module.REQUIRED_FIELDS)),
            rewrite_suggestion="submit a JSON object with the fixed schema",
        )

    for gate in (_hard_gate, _schema_gate, _threshold_gate, _critical_gate):
        verdict = gate(candidate)
        if verdict is not None:
            return verdict
    return Verdict.accepted()


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def policy_metadata() -> dict[str, object]:
    """Return the policy thresholds for diagnostics and U06 audit."""
    return {
        "policy_version": POLICY_VERSION,
        "thresholds": {
            "min_confidence": MIN_CONFIDENCE,
            "min_evidence_coverage": MIN_EVIDENCE_COVERAGE,
            "min_reusability": MIN_REUSABILITY,
            "min_verifiability": MIN_VERIFIABILITY,
            "min_novelty": MIN_NOVELTY,
            "min_stability": MIN_STABILITY,
            "min_scope_clarity": MIN_SCOPE_CLARITY,
            "high_confidence_floor": HIGH_CONFIDENCE_FLOOR,
            "low_evidence_ceiling": LOW_EVIDENCE_CEILING,
        },
        "evaluated_at": _dt.datetime.now(_dt.timezone.utc).isoformat(),
        "schema_version": "0.3.0",
    }