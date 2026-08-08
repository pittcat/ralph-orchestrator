"""Direct unit tests for the bounded finalization marker parser.

These tests are intentionally tiny (≤ 1s each) and exercise
:func:`memory_marker.extract_finalization_marker` directly without
spinning up the hook subprocess. They pin every parser edge
contractually so future refactors cannot silently weaken the gate.
"""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "plugins" / "nowledge-mem-ralph" / "scripts" / "memory_marker.py"


@pytest.fixture
def marker():
    spec = importlib.util.spec_from_file_location("_marker_under_test", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["_marker_under_test"] = module
    spec.loader.exec_module(module)
    return module


def _valid_body(**overrides) -> str:
    candidate = {
        "memory_type": "durable_decision",
        "title": "Direct parser test",
        "claim": "Every edge is pinned.",
        "why_it_matters": "Refactors cannot silently weaken the gate.",
        "evidence": "test_memory_marker.py.",
        "applies_when": "any parser change",
        "scope": "plugin:knowledge-mem-ralph",
        "verification": "pytest covers all listed cases.",
        "critical_assumptions": [],
        "critical_ambiguities": [],
        "metrics": {
            "confidence": 95,
            "evidence_coverage": 88,
            "reusability": 90,
            "stability": 92,
            "scope_clarity": 96,
            "verifiability": 90,
            "novelty": 40,
        },
        "finalize": True,
    }
    candidate.update(overrides)
    return json.dumps(candidate, ensure_ascii=False)


def test_non_string_message_is_skipped(marker) -> None:
    """Non-string ``last_assistant_message`` values surface as SKIPPED."""
    for value in (None, 0, 1, 3.14, [], {}, True):
        result = marker.extract_finalization_marker(value)
        assert result.status == "SKIPPED", (
            f"{value!r} should be SKIPPED, got {result.status!r}"
        )


def test_empty_string_is_skipped(marker) -> None:
    result = marker.extract_finalization_marker("")
    assert result.status == "SKIPPED"


def test_message_without_marker_is_skipped(marker) -> None:
    result = marker.extract_finalization_marker("just a plain message")
    assert result.status == "SKIPPED"
    assert "no finalization marker" in result.reason


def test_finalize_false_is_skipped(marker) -> None:
    body = _valid_body(finalize=False)
    message = f"<!-- nowledge-memory-finalize\n{body}\n-->"
    result = marker.extract_finalization_marker(message)
    assert result.status == "SKIPPED"
    assert "finalize" in result.reason.lower()


def test_finalize_null_is_skipped(marker) -> None:
    body = _valid_body(finalize=None)
    message = f"<!-- nowledge-memory-finalize\n{body}\n-->"
    result = marker.extract_finalization_marker(message)
    assert result.status == "SKIPPED"


def test_finalize_int_one_is_skipped(marker) -> None:
    """`finalize: 1` (int) is not the JSON boolean true — must be SKIPPED."""
    body = _valid_body(finalize=1)
    message = f"<!-- nowledge-memory-finalize\n{body}\n-->"
    result = marker.extract_finalization_marker(message)
    assert result.status == "SKIPPED"


def test_json_array_body_is_rejected(marker) -> None:
    """JSON arrays must surface as REJECTED (must be a JSON object)."""
    message = "<!-- nowledge-memory-finalize\n[1,2,3]\n-->"
    result = marker.extract_finalization_marker(message)
    assert result.status == "REJECTED"
    assert "object" in result.reason


def test_json_scalar_body_is_rejected(marker) -> None:
    """JSON scalars must surface as REJECTED (must be a JSON object)."""
    message = '<!-- nowledge-memory-finalize\n"hello"\n-->'
    result = marker.extract_finalization_marker(message)
    assert result.status == "REJECTED"


def test_lone_surrogate_in_body_is_rejected(marker) -> None:
    """A lone surrogate inside the body must surface as REJECTED."""
    body = json.dumps(
        {"finalize": True, "title": "lone \ud800 surrogate"},
        ensure_ascii=False,
    )
    message = f"<!-- nowledge-memory-finalize\n{body}\n-->"
    result = marker.extract_finalization_marker(message)
    assert result.status == "REJECTED", (
        f"lone surrogate should be REJECTED, got {result.status!r}: {result.reason!r}"
    )


def test_single_line_marker_is_skipped(marker) -> None:
    """Marker on a single line without a newline after the tag is not parsed."""
    body = _valid_body()
    message = f"<!-- nowledge-memory-finalize {body} -->"
    result = marker.extract_finalization_marker(message)
    assert result.status in {"SKIPPED", "REJECTED"}
    if result.status == "SKIPPED":
        assert "no finalization marker" in result.reason or "newline" in result.reason.lower()
    else:
        assert "newline" in result.reason.lower() or "object" in result.reason.lower()


def test_missing_close_marker_is_skipped(marker) -> None:
    """A marker without ``-->`` closer must surface as SKIPPED."""
    body = _valid_body()
    message = f"<!-- nowledge-memory-finalize\n{body}\n"
    result = marker.extract_finalization_marker(message)
    assert result.status == "SKIPPED"


def test_two_markers_is_rejected(marker) -> None:
    """Two markers in one message must surface as REJECTED with `duplicate`."""
    body = _valid_body()
    message = (
        f"<!-- nowledge-memory-finalize\n{body}\n-->\n"
        f"middle prose\n"
        f"<!-- nowledge-memory-finalize\n{body}\n-->\n"
    )
    result = marker.extract_finalization_marker(message)
    assert result.status == "REJECTED"
    assert "duplicate" in result.reason.lower()


def test_three_markers_is_rejected(marker) -> None:
    """Three markers in one message still surface as REJECTED."""
    body = _valid_body()
    message = (
        f"<!-- nowledge-memory-finalize\n{body}\n-->\n"
        f"<!-- nowledge-memory-finalize\n{body}\n-->\n"
        f"<!-- nowledge-memory-finalize\n{body}\n-->\n"
    )
    result = marker.extract_finalization_marker(message)
    assert result.status == "REJECTED"
    assert "duplicate" in result.reason.lower()


def test_marker_variant_v2_is_rejected(marker) -> None:
    """``-v2`` variant must surface as REJECTED, never PARSED."""
    body = _valid_body()
    message = f"<!-- nowledge-memory-finalize-v2\n{body}\n-->"
    result = marker.extract_finalization_marker(message)
    assert result.status == "REJECTED"
    assert "variant" in result.reason.lower() or "no finalization" in result.reason.lower()


def test_oversized_marker_is_rejected(marker) -> None:
    """Payload larger than 16 KiB must surface as REJECTED."""
    body = _valid_body(evidence="x" * (20 * 1024))
    message = f"<!-- nowledge-memory-finalize\n{body}\n-->"
    result = marker.extract_finalization_marker(message)
    assert result.status == "REJECTED"
    assert "bytes" in result.reason.lower()


def test_canonical_marker_parses(marker) -> None:
    """A canonical legal marker must surface as PARSED with non-empty digest."""
    body = _valid_body()
    message = f"<!-- nowledge-memory-finalize\n{body}\n-->"
    result = marker.extract_finalization_marker(message)
    assert result.status == "PARSED", (
        f"canonical marker should be PARSED, got {result.status!r}: {result.reason!r}"
    )
    assert result.memory_digest
    assert result.candidate is not None
    assert result.candidate["finalize"] is True