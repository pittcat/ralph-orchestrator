"""Tests for U03 memory-schema-and-policy.

U03 locks the **fixed Memory schema**, **hard gates** (progress/log/
command/transcript rejection), **seven quality metrics** with
deterministic thresholds, **memory_digest** normalisation, and the
``save-memory`` entry point contract.

These tests prove the following contract from
``.ralph/specs/knowledge-mem-ralph-plugin-design.md`` §5.3 lifecycle
state machine + KTD matrix:

* ``memory.evaluate(candidate) -> Verdict`` is pure-Python (no ``nmem``).
* Field-level schema validation rejects missing required fields.
* Hard-gate content types (``progress`` / ``log`` / ``command`` /
  ``transcript``) are always ``REJECTED`` regardless of metrics.
* High confidence with low evidence_coverage is ``REJECTED``
  (anti-hallucination gate).
* Non-empty ``critical_assumptions`` / ``critical_ambiguities`` produce
  ``NEEDS_REWRITE`` and never reach the writer.
* ``memory_digest`` is stable across calls: identical (title, claim,
  evidence, scope, applies_when, verification) always yields the same
  SHA-256 hex digest — this is what U04 will reuse for idempotency.
* The save-memory entry accepts a dict and routes through schema →
  policy without ever spawning ``nmem`` (writer is a forbidden path
  for U03).
* ``memory_dedupe`` recognises that a previously accepted record with
  the same digest must short-circuit (returning the dedup signal so
  U04 can pick it up).

No live ``nmem`` binary is required; everything is exercised against
in-process fixtures under ``tests/fixtures/memory``.
"""

from __future__ import annotations

import dataclasses
import importlib.util
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
PLUGIN_DIR = ROOT / "plugins" / "nowledge-mem-ralph"
SCRIPTS_DIR = PLUGIN_DIR / "scripts"
FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures" / "memory"


# ---------------------------------------------------------------------------
# importlib loader helpers (Python 3.14 frozen-dataclass friendly — see
# test_recall.py for the same dance; U03 ships four modules that all
# carry frozen dataclasses and must be registered in sys.modules).
# ---------------------------------------------------------------------------


def _load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load module spec for {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture(scope="session")
def memory_schema_module():
    return _load_module("_nowledge_mem_ralph_memory_schema", SCRIPTS_DIR / "memory_schema.py")


@pytest.fixture(scope="session")
def memory_policy_module(memory_schema_module):
    return _load_module(
        "_nowledge_mem_ralph_memory_policy",
        SCRIPTS_DIR / "memory_policy.py",
    )


@pytest.fixture(scope="session")
def memory_dedupe_module():
    return _load_module(
        "_nowledge_mem_ralph_memory_dedupe",
        SCRIPTS_DIR / "memory_dedupe.py",
    )


@pytest.fixture(scope="session")
def memory_module(memory_schema_module, memory_policy_module, memory_dedupe_module):
    return _load_module(
        "_nowledge_mem_ralph_memory",
        SCRIPTS_DIR / "memory.py",
    )


@pytest.fixture
def valid_candidate() -> dict:
    return json.loads((FIXTURES_DIR / "valid_candidate.json").read_text(encoding="utf-8"))


# Module-level dedupe ledger is reset before every test so the
# fixtures do not leak between test functions. The ledger is
# process-local by design (U04 owns the durable store); U03 only
# exposes a short-circuit signal.

@pytest.fixture(autouse=True)
def _reset_dedupe_ledger(memory_dedupe_module):
    memory_dedupe_module.clear_local_ledger()
    yield
    memory_dedupe_module.clear_local_ledger()


@pytest.fixture
def missing_evidence_candidate() -> dict:
    return json.loads((FIXTURES_DIR / "missing_evidence.json").read_text(encoding="utf-8"))


@pytest.fixture
def progress_candidate() -> dict:
    return json.loads((FIXTURES_DIR / "progress_content.json").read_text(encoding="utf-8"))


@pytest.fixture
def log_candidate() -> dict:
    return json.loads((FIXTURES_DIR / "log_content.json").read_text(encoding="utf-8"))


@pytest.fixture
def high_confidence_low_evidence_candidate() -> dict:
    return json.loads(
        (FIXTURES_DIR / "high_confidence_low_evidence.json").read_text(encoding="utf-8")
    )


@pytest.fixture
def critical_assumption_candidate() -> dict:
    return json.loads((FIXTURES_DIR / "critical_assumption.json").read_text(encoding="utf-8"))


# ---------------------------------------------------------------------------
# Schema
# ---------------------------------------------------------------------------


def test_memory_schema_accepts_valid_candidate(memory_schema_module, valid_candidate):
    """Schema validator returns no errors on a fully populated candidate."""
    errors = memory_schema_module.validate_memory_schema(valid_candidate)
    assert errors == [], f"expected no schema errors, got {errors}"


def test_memory_schema_rejects_missing_required_field(
    memory_schema_module, missing_evidence_candidate
):
    """Missing required field (``evidence``) is rejected with a list of field names."""
    errors = memory_schema_module.validate_memory_schema(missing_evidence_candidate)
    assert errors, "expected schema errors for missing evidence"
    assert "evidence" in errors


def test_memory_schema_lists_all_missing_fields(memory_schema_module):
    """When several fields are missing, every missing field is reported."""
    candidate = {"memory_type": "durable_decision", "title": "incomplete"}
    errors = memory_schema_module.validate_memory_schema(candidate)
    for required in (
        "claim",
        "why_it_matters",
        "evidence",
        "applies_when",
        "scope",
        "verification",
        "metrics",
    ):
        assert required in errors


def test_memory_schema_required_fields_constant(memory_schema_module):
    """REQUIRED_FIELDS is the SSOT field set and never silently changes."""
    assert memory_schema_module.REQUIRED_FIELDS == frozenset(
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


def test_memory_schema_required_metrics(memory_schema_module):
    """The seven metrics are an explicit, exhaustive set."""
    assert memory_schema_module.REQUIRED_METRICS == frozenset(
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


def test_memory_schema_rejects_short_title(memory_schema_module, valid_candidate):
    """A title shorter than the minimum length is reported as a schema error."""
    bad = dict(valid_candidate)
    bad["title"] = "x"  # one character
    errors = memory_schema_module.validate_memory_schema(bad)
    assert "title" in errors


# ---------------------------------------------------------------------------
# Hard gates (progress / log / command / transcript)
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "content_type",
    ["progress", "log", "command", "transcript"],
)
def test_hard_gate_rejects_disallowed_content_types(
    memory_policy_module, valid_candidate, content_type
):
    """Disallowed content types must produce REJECTED regardless of metrics."""
    candidate = dict(valid_candidate)
    candidate["memory_type"] = content_type
    verdict = memory_policy_module.evaluate(candidate)
    assert verdict.result == "REJECTED", (
        f"hard gate failed for content_type={content_type}: {verdict!r}"
    )
    assert content_type in verdict.reason or "content_type" in verdict.reason


def test_hard_gate_rejects_progress_via_save_memory(
    memory_module, progress_candidate
):
    """``memory.save`` propagates the hard gate verdict and never invokes the writer."""
    result = memory_module.save(progress_candidate)
    assert result.result == "REJECTED"
    assert "progress" in result.reason or "content_type" in result.reason
    # SaveResult must NOT carry a memory_id (writer was never called).
    assert getattr(result, "memory_id", None) is None


# ---------------------------------------------------------------------------
# Quality thresholds (confidence >= 90 + evidence_coverage < 70 → REJECTED)
# ---------------------------------------------------------------------------


def test_high_confidence_low_evidence_is_rejected(
    memory_policy_module, high_confidence_low_evidence_candidate
):
    """Anti-hallucination gate: high confidence + low evidence_coverage = REJECTED."""
    verdict = memory_policy_module.evaluate(high_confidence_low_evidence_candidate)
    assert verdict.result == "REJECTED"
    assert "evidence_coverage" in verdict.reason or "evidence coverage" in verdict.reason.lower()


def test_low_confidence_is_rejected(memory_policy_module, valid_candidate):
    """confidence below threshold (>= 80 required) is REJECTED."""
    bad = dict(valid_candidate)
    bad["metrics"] = dict(valid_candidate["metrics"])
    bad["metrics"]["confidence"] = 60
    verdict = memory_policy_module.evaluate(bad)
    assert verdict.result == "REJECTED"
    assert "confidence" in verdict.reason


# ---------------------------------------------------------------------------
# Critical assumption / ambiguity gate
# ---------------------------------------------------------------------------


def test_critical_assumption_blocks_save(memory_policy_module, critical_assumption_candidate):
    """Non-empty critical_assumptions → NEEDS_REWRITE, never ACCEPTED."""
    verdict = memory_policy_module.evaluate(critical_assumption_candidate)
    assert verdict.result == "NEEDS_REWRITE"
    assert "critical_assumption" in verdict.reason or "critical_assumption" in verdict.missing_fields


def test_critical_ambiguity_blocks_save(memory_policy_module, valid_candidate):
    """Non-empty critical_ambiguities also blocks the save."""
    bad = dict(valid_candidate)
    bad["critical_ambiguities"] = ["unresolved scope boundary"]
    verdict = memory_policy_module.evaluate(bad)
    assert verdict.result == "NEEDS_REWRITE"


# ---------------------------------------------------------------------------
# Happy path through ``memory.save`` (U03 boundary — no nmem call)
# ---------------------------------------------------------------------------


def test_memory_save_accepts_valid_candidate(memory_module, valid_candidate):
    """A clean candidate yields an ACCEPTED verdict with a stable memory_digest."""
    result = memory_module.save(valid_candidate)
    assert result.result == "ACCEPTED"
    assert result.memory_digest, "ACCEPTED record must carry a memory_digest"
    assert len(result.memory_digest) == 64  # SHA-256 hex
    assert result.missing_fields == ()


def test_memory_save_never_invokes_writer(memory_module, valid_candidate, monkeypatch):
    """U03 contract: ``memory.save`` is forbidden from spawning nmem.

    The writer belongs to U04. We monkeypatch ``subprocess.run`` so any
    attempted shell-out raises; if U03 accidentally introduces it the
    test fails loudly rather than silently passing.
    """
    import subprocess

    def _explode(*args, **kwargs):
        raise AssertionError(
            "memory.save must not invoke subprocess (writer belongs to U04)"
        )

    monkeypatch.setattr(subprocess, "run", _explode)
    result = memory_module.save(valid_candidate)
    assert result.result == "ACCEPTED"


# ---------------------------------------------------------------------------
# Any hat can call save (R6)
# ---------------------------------------------------------------------------


def test_any_hat_can_call_save(memory_module, valid_candidate):
    """``memory.save`` does not depend on hat name, role, or env context.

    Two callers see the same verdict progression for identical content:
    ACCEPTED on the first call, OBSERVATION on the second (the dedupe
    short-circuit). The caller identity (``source``) does not affect
    either verdict.
    """
    result_a = memory_module.save(valid_candidate, source={"hat": "executor"})
    result_b = memory_module.save(valid_candidate, source={"hat": "reviewer"})
    assert result_a.result == "ACCEPTED", (
        "first call from any hat must yield ACCEPTED when the candidate "
        f"is valid; got {result_a.result}: {result_a.reason}"
    )
    assert result_b.result == "OBSERVATION", (
        "second identical call from a different hat must hit the dedupe "
        f"short-circuit (OBSERVATION), not silently re-accept; got {result_b.result}"
    )
    # Stable digest regardless of caller identity (the hat is recorded in
    # the record but the digest is content-scoped, see U04 SSOT).
    assert result_a.memory_digest == result_b.memory_digest
    assert result_a.record is not None
    assert result_b.record is None  # OBSERVATION never carries a record


# ---------------------------------------------------------------------------
# memory_digest stability
# ---------------------------------------------------------------------------


def test_memory_digest_stable_for_equal_content(memory_dedupe_module, valid_candidate):
    """Identical content → byte-equal digest across repeated calls."""
    digest_a = memory_dedupe_module.compute_memory_digest(valid_candidate)
    digest_b = memory_dedupe_module.compute_memory_digest(valid_candidate)
    assert digest_a == digest_b
    assert len(digest_a) == 64


def test_memory_digest_strips_whitespace(memory_dedupe_module, valid_candidate):
    """Whitespace-only differences in claim/evidence do not change the digest."""
    mutated = dict(valid_candidate)
    mutated["claim"] = valid_candidate["claim"] + "\n\n   "
    mutated["evidence"] = " " + valid_candidate["evidence"] + "\t"
    digest_a = memory_dedupe_module.compute_memory_digest(valid_candidate)
    digest_b = memory_dedupe_module.compute_memory_digest(mutated)
    assert digest_a == digest_b, "whitespace must be normalised before digest"


def test_memory_digest_changes_with_scope(memory_dedupe_module, valid_candidate):
    """Different scope yields different digest (scope is part of the dedupe key)."""
    other = dict(valid_candidate)
    other["scope"] = "plugin:another-plugin"
    digest_a = memory_dedupe_module.compute_memory_digest(valid_candidate)
    digest_b = memory_dedupe_module.compute_memory_digest(other)
    assert digest_a != digest_b


# ---------------------------------------------------------------------------
# Dedupe signal for U04
# ---------------------------------------------------------------------------


def test_memory_dedupe_signals_repeat_save(memory_dedupe_module, valid_candidate):
    """``memory_dedupe.is_already_saved`` returns True once the digest is in the local ledger."""
    digest = memory_dedupe_module.compute_memory_digest(valid_candidate)
    assert memory_dedupe_module.is_already_saved(digest) is False
    memory_dedupe_module.record_save(digest, scope=valid_candidate["scope"])
    assert memory_dedupe_module.is_already_saved(digest) is True


# ---------------------------------------------------------------------------
# Verdict dataclass
# ---------------------------------------------------------------------------


def test_verdict_is_frozen_dataclass(memory_policy_module, valid_candidate):
    """Verdict is a frozen dataclass so downstream code can rely on immutability."""
    verdict = memory_policy_module.evaluate(valid_candidate)
    assert dataclasses.is_dataclass(verdict)
    assert verdict.__dataclass_params__.frozen is True
    with pytest.raises(dataclasses.FrozenInstanceError):
        verdict.result = "MUTATED"  # type: ignore[misc]


def test_save_result_carries_evaluation_version(memory_module, valid_candidate):
    """SaveResult records the policy version so U04 can detect drift on a future re-evaluation."""
    result = memory_module.save(valid_candidate)
    assert result.policy_version
    assert isinstance(result.policy_version, str)
    assert len(result.policy_version) >= 3