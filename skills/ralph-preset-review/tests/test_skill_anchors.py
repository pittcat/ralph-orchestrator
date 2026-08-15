#!/usr/bin/env python3
"""2026-07-27-002 plan Unit 4: anchor contract tests for preset-author /
preset-review SKILLs.

After plan 2026-08-02-001 (capability-triggered operator skill split),
the review skill owns its own fixtures/ and tests/ tree. This test:

1. Pins a small set of stable anchors in the author / review SKILL
   files and their references (so future edits cannot silently remove
   the sections reviewers rely on).
2. Verifies the four capability-triggered fixtures added by U3
   (worktree-reuse / readonly / correction-exhaustion / terminal-ownership)
   are present, each one declares its expected finding axis in a
   stable top-of-file comment, and no two fixtures collide on the
   review-only finding ID they advertise.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path
from typing import Iterable

import yaml

# Anchor list: tests pin these strings exist at least once in the
# corresponding doc. We deliberately do NOT lock full prompt text:
# plans evolve, but stable section headings stay.
ANCHORS: tuple[tuple[str, str], ...] = (
    ("skills/ralph-preset-author/SKILL.md", "Capability discovery"),
    ("skills/ralph-preset-review/SKILL.md", "Capability-triggered audit"),
    ("skills/ralph-preset-author/references/commands.md", "Capability inventory"),
    ("skills/ralph-preset-author/references/agent-native-model.md", "Runtime Audit Model"),
    # Key-stage event gate anchors (plan 2026-08-05-007). Both author and
    # review SKILL.md must contain the section heading; both author and
    # review finding-rubric.md must contain the "Key-stage event gate"
    # segment marker so reviewers can pair findings with the rubric.
    ("skills/ralph-preset-author/SKILL.md", "Key-stage event gate"),
    ("skills/ralph-preset-review/SKILL.md", "Key-stage event gate audit"),
    ("skills/ralph-preset-author/references/finding-rubric.md", "Key-stage event gate finding_id"),
    ("skills/ralph-preset-review/references/finding-rubric.md", "Key-stage event gate finding_id"),
    # Evidence-bound correction anchors (plan 2026-08-06-001 U4). Both author
    # and review must have the four evidence-bound finding IDs documented
    # in finding-rubric.md, patterns.md, prompt-visibility.md, commands.md.
    ("skills/ralph-preset-author/references/finding-rubric.md", "evidence_bound_missing_invariant"),
    ("skills/ralph-preset-review/references/finding-rubric.md", "evidence_bound_missing_invariant"),
    ("skills/ralph-preset-author/references/patterns.md", "Evidence-bound correction pattern"),
    ("skills/ralph-preset-review/references/patterns.md", "Evidence-bound correction pattern"),
    ("skills/ralph-preset-author/references/prompt-visibility.md", "evidence-bound"),
    ("skills/ralph-preset-review/references/prompt-visibility.md", "evidence-bound"),
    ("skills/ralph-preset-author/references/commands.md", "Scope handoff contract"),
    ("skills/ralph-preset-review/references/commands.md", "Scope handoff contract"),
    ("skills/ralph-preset-author/references/patterns.md", "Scope handoff guard pattern"),
    ("skills/ralph-preset-review/references/patterns.md", "Scope handoff guard pattern"),
    ("skills/ralph-preset-author/references/prompt-visibility.md", "Scope resolution is agent-owned"),
    ("skills/ralph-preset-review/references/prompt-visibility.md", "Scope resolution is agent-owned"),
    # Scope polarity anchors (plan 2026-08-10-002 U5). Both author and
    # review finding-rubric.md must contain the new polarity finding id
    # so reviewers can pair scope positive-assertion regressions with
    # the rubric entry.
    (
        "skills/ralph-preset-author/references/finding-rubric.md",
        "preset.payload_consistency_scope_positive_assertion",
    ),
    (
        "skills/ralph-preset-review/references/finding-rubric.md",
        "preset.payload_consistency_scope_positive_assertion",
    ),
    ("skills/ralph-preset-author/references/finding-rubric.md", "preset.triggered_self_or_static_target"),
    ("skills/ralph-preset-review/references/finding-rubric.md", "preset.triggered_self_or_static_target"),
    # Runtime verification anchors. Both author
    # and review commands.md must surface `ralph preset verify`, and
    # both finding-rubric.md files must contain the dynamic evidence
    # finding ids so reviewers can pair preset coverage gaps with the
    # rubric entry.
    ("skills/ralph-preset-author/references/commands.md", "ralph preset verify"),
    ("skills/ralph-preset-review/references/commands.md", "ralph preset verify"),
    ("skills/ralph-preset-author/references/finding-rubric.md", "verify.dynamic_evidence_missing"),
    ("skills/ralph-preset-review/references/finding-rubric.md", "verify.dynamic_evidence_missing"),
    ("skills/ralph-preset-author/references/finding-rubric.md", "verify.scenario_coverage_gap"),
    ("skills/ralph-preset-review/references/finding-rubric.md", "verify.scenario_coverage_gap"),
)

# Capability-triggered fixtures from plan 2026-08-02-001 U3.
# Each entry: (fixture filename, expected finding id advertised in
# the fixture header comment). Key-stage fixtures additionally carry a
# companion `*.preset-author-notes.md` contract that is parsed below.
CAPABILITY_FIXTURES: tuple[tuple[str, str], ...] = (
    (
        "worktree-reuse-negative-fixture.yml",
        "preset.worktree_reuse_fabricates_settlement",
    ),
    (
        "readonly-hat-gate-negative-fixture.yml",
        "preset.readonly_hat_writes_artifacts",
    ),
    (
        "correction-exhaustion-negative-fixture.yml",
        "preset.correction_round_below_final_min",
    ),
    (
        "terminal-ownership-negative-fixture.yml",
        "preset.auditor_multi_terminal_publisher",
    ),
    # Evidence-bound correction fixtures from plan 2026-08-06-001 U4.
    # Each entry advertises the primary review-only finding id the
    # fixture is meant to anchor.
    (
        "evidence-bound-negative-fixture.yml",
        "evidence_bound_missing_invariant",
    ),
    (
        "emitter-skill-load-negative-fixture.yml",
        "preset.instructions_emit_skill_load_missing",
    ),
    # Key-stage event gate fixtures from plan 2026-08-05-007.
    # Each entry advertises the primary review-only finding id the
    # fixture is meant to anchor. The positive fixture advertises
    # absence (no review-only finding) by listing a baseline
    # `key_stage_event_gate` anchor and is still loadable.
    (
        "scope_missing_negative_fixture.yml",
        "scope.contract.missing_manifest_field",
    ),
    (
        "scope_boundary_dependency_negative_fixture.yml",
        "scope.contract.boundary_authority",
    ),
    (
        "scope_placeholder_base_negative_fixture.yml",
        "scope.contract.placeholder_base",
    ),
    (
        "scope_confidence_gate_negative_fixture.yml",
        "scope.contract.confidence_gate_bypass",
    ),
    # Scope polarity positive-assertion fixture from plan 2026-08-10-002
    # U5. The fixture encodes the inverted polarity anti-pattern
    # (`exists:true` on a protected scope structural field) and anchors
    # the new strict-lint finding id.
    (
        "scope_polarity_negative_fixture.yml",
        "preset.payload_consistency_scope_positive_assertion",
    ),
    (
        "key-stage-event-gate-positive-fixture.yml",
        "key_stage_event_gate_baseline",
    ),
    (
        "key-stage-event-gate-missing-selection-negative-fixture.yml",
        "preset.key_stage_event_gate_missing_selection",
    ),
    (
        "key-stage-event-gate-divergence-negative-fixture.yml",
        "preset.key_stage_event_gate_notes_preset_diverge",
    ),
    (
        "key-stage-event-gate-no-reason-negative-fixture.yml",
        "preset.key_stage_event_gate_no_reason",
    ),
    # Runtime verification negative fixture.
    # The fixture advertises `verify.dynamic_evidence_missing` and
    # `verify.scenario_coverage_gap` as the primary review-only
    # finding ids it is meant to anchor; it is generic (no preset /
    # accident binding).
    (
        "runtime-verify-negative-fixture.yml",
        "verify.dynamic_evidence_missing",
    ),
)


def _project_root() -> Path:
    return Path(__file__).resolve().parent.parent.parent.parent


def _read(path: Path) -> str:
    assert path.is_file(), f"missing file: {path}"
    return path.read_text(encoding="utf-8")


def _check_anchor(path: str, anchor: str) -> bool:
    full = _project_root() / path
    if not full.exists():
        print(f"MISSING file: {full}")
        return False
    content = full.read_text(encoding="utf-8")
    if anchor in content:
        print(f"OK anchor {anchor!r} in {path}")
        return True
    print(f"FAIL anchor {anchor!r} not found in {path}")
    return False


def _check_anchor_unique(path: str, anchor: str) -> bool:
    full = _project_root() / path
    if not full.exists():
        print(f"MISSING file: {full}")
        return False
    content = full.read_text(encoding="utf-8")
    occurrences = content.count(anchor)
    if occurrences == 0:
        print(f"FAIL anchor {anchor!r} not found in {path}")
        return False
    if occurrences > 1:
        print(
            f"FAIL anchor {anchor!r} appears {occurrences} times in {path}; "
            "must be unique"
        )
        return False
    print(f"OK anchor {anchor!r} unique in {path}")
    return True


def _iter_all() -> Iterable[tuple[str, bool]]:
    yield from _anchor_results()
    yield from _anchor_uniqueness_results()
    yield from _capability_fixture_results()


def _anchor_results() -> Iterable[tuple[str, bool]]:
    for path, anchor in ANCHORS:
        yield f"anchor_present:{path}:{anchor}", _check_anchor(path, anchor)


def _anchor_uniqueness_results() -> Iterable[tuple[str, bool]]:
    # Stable SKILL.md headings must each appear at least once; lock
    # against accidental duplicate sections by only requiring
    # presence-not-multiple for capability headings that look stable.
    # We pick the two highest-signal anchors (must be unique).
    unique_anchors = (
        ("skills/ralph-preset-author/SKILL.md", "Capability discovery"),
        ("skills/ralph-preset-review/SKILL.md", "Capability-triggered audit"),
        # Use the precise 0e heading line so the string is present only
        # once in each SKILL.md (the field-name references inside the
        # body / guardrails contribute additional matches of the shorter
        # phrase and must not be the unique anchor).
        (
            "skills/ralph-preset-author/SKILL.md",
            "0e. **Key-stage event gate",
        ),
        (
            "skills/ralph-preset-review/SKILL.md",
            "3a.7. **Key-stage event gate audit",
        ),
    )
    for path, anchor in unique_anchors:
        yield f"anchor_unique:{path}:{anchor}", _check_anchor_unique(path, anchor)


def _capability_fixture_results() -> Iterable[tuple[str, bool]]:
    fixtures_dir = _project_root() / "skills" / "ralph-preset-review" / "fixtures"
    advertised_ids: dict[str, str] = {}
    yield "all_finding_ids_unique", _check_unique_advertised_ids(
        fixtures_dir, advertised_ids
    )
    for filename, finding_id in CAPABILITY_FIXTURES:
        yield f"fixture_present:{filename}", _check_fixture_present(
            fixtures_dir, filename, finding_id
        )
    yield from _evidence_bound_results(fixtures_dir)
    yield from _key_stage_event_gate_results(fixtures_dir)


KEY_STAGE_REQUIRED_FIELDS = {
    "key_stage",
    "guard_selection",
    "precheck_guard",
    "precheck_retry_budget",
    "payload_consistency_guard",
    "payload_consistency_retry_budget",
    "reason",
    "confirmation_status",
}
KEY_STAGE_SELECTIONS = {"precheck", "payload_consistency", "both", "neither"}
VAGUE_REASONS = {"", "用户偏好", "后续再说", "先这样"}


def _load_key_stage_notes(fixture: Path) -> dict[str, object]:
    notes = fixture.with_suffix(".preset-author-notes.md")
    text = _read(notes)
    match = re.search(r"```yaml\n(.*?)```", text, re.DOTALL)
    assert match is not None, f"notes missing YAML contract: {notes}"
    value = yaml.safe_load(match.group(1))
    assert isinstance(value, dict), f"notes contract must be a mapping: {notes}"
    return value


def _key_stage_findings(fixture: Path) -> set[str]:
    preset = yaml.safe_load(_read(fixture))
    notes = _load_key_stage_notes(fixture)
    assert isinstance(preset, dict)
    stages = notes.get("key_stages")
    assert isinstance(stages, list) and stages, f"notes has no key_stages: {fixture}"

    findings: set[str] = set()
    wants_precheck = False
    wants_payload_consistency = False
    for stage in stages:
        assert isinstance(stage, dict)
        missing = KEY_STAGE_REQUIRED_FIELDS - set(stage)
        if missing:
            findings.add("preset.key_stage_event_gate_missing_selection")
        selection = stage.get("guard_selection")
        selection_valid = selection in KEY_STAGE_SELECTIONS
        if selection not in KEY_STAGE_SELECTIONS:
            findings.add("preset.key_stage_event_gate_missing_selection")
            selection = "neither"
        if selection in {"precheck", "both"}:
            wants_precheck = True
        if selection in {"payload_consistency", "both"}:
            wants_payload_consistency = True
        if selection_valid and stage.get("precheck_guard") != (
            selection in {"precheck", "both"}
        ):
            findings.add("preset.key_stage_event_gate_field_reuse")
        if selection_valid and stage.get("payload_consistency_guard") != (
            selection in {"payload_consistency", "both"}
        ):
            findings.add("preset.key_stage_event_gate_field_reuse")
        for guard, budget in (
            ("precheck_guard", "precheck_retry_budget"),
            ("payload_consistency_guard", "payload_consistency_retry_budget"),
        ):
            if stage.get(guard) is True and stage.get(budget) not in {1, 2, 3}:
                findings.add("preset.key_stage_event_gate_shared_budget")
            if stage.get(guard) is False and stage.get(budget) is not None:
                findings.add("preset.key_stage_event_gate_shared_budget")
        if stage.get("confirmation_status") != "confirmed":
            findings.add("preset.key_stage_event_gate_pending_status")
        reason = stage.get("reason")
        low_budget = any(
            stage.get(field) in {1, 2}
            for field in ("precheck_retry_budget", "payload_consistency_retry_budget")
        )
        if selection == "neither" or low_budget:
            if not isinstance(reason, str) or len(reason) > 80 or reason in VAGUE_REASONS:
                findings.add("preset.key_stage_event_gate_no_reason")

    event_loop = preset.get("event_loop", {})
    assert isinstance(event_loop, dict)
    if event_loop.get("retry_budget") is not None:
        findings.add("preset.key_stage_event_gate_shared_budget")
    precheck_rules = event_loop.get("precheck", {}).get("rules", {})
    if wants_precheck and not isinstance(precheck_rules, dict) or (
        wants_precheck and not precheck_rules
    ):
        findings.add("preset.key_stage_event_gate_notes_preset_diverge")
    event_policy = preset.get("event_policy", {})
    payload_rules = event_policy.get("payload_consistency", {}).get("rules", [])
    if wants_payload_consistency and not isinstance(payload_rules, list) or (
        wants_payload_consistency and not payload_rules
    ):
        findings.add("preset.key_stage_event_gate_notes_preset_diverge")
    return findings


def _evidence_bound_findings(fixture: Path) -> set[str]:
    """Detect evidence-bound correction anti-patterns in a fixture.

    Returns a set of finding IDs (subset of the four evidence-bound IDs).
    """
    preset = yaml.safe_load(_read(fixture))
    assert isinstance(preset, dict)

    findings: set[str] = set()
    event_policy = preset.get("event_policy", {})
    assert isinstance(event_policy, dict)
    schemas = event_policy.get("schemas", {})
    assert isinstance(schemas, dict)

    # Check each schema for correction/feedback topics with semantic rejection shape.
    for topic, schema in schemas.items():
        if not isinstance(schema, dict):
            continue
        required = schema.get("required_fields", [])
        known = schema.get("known_fields", [])
        all_fields = set(required) | set(known)

        # (a) missing_invariant: schema has correction-like fields but no violated_invariant
        if topic.startswith("correction") or "feedback" in topic:
            has_evidence_fields = any(
                f in all_fields
                for f in ("observed", "required_proof", "violated_invariant")
            )
            if has_evidence_fields and "violated_invariant" not in all_fields:
                findings.add("evidence_bound_missing_invariant")
            # (b) replacement_payload: schema allows replacement fields on semantic rejection
            if "replacement" in all_fields or "suggested_payload" in all_fields:
                findings.add("evidence_bound_replacement_payload")
            # (c) no_target: schema lacks target_hat for routing
            if "target_hat" not in all_fields:
                findings.add("evidence_bound_no_target")

    # (d) unbounded_retry: event_loop.max_iterations exists without evidence progression
    event_loop = preset.get("event_loop", {})
    assert isinstance(event_loop, dict)
    if event_loop.get("max_iterations") is not None:
        # No evidence progression mechanism if there is no per-iteration
        # evidence uniqueness constraint declared anywhere in the preset.
        # Simple proxy: max_iterations is set AND there is no
        # event_loop.precheck.rules or payload_consistency rule that ties
        # retry to new evidence.
        precheck = event_loop.get("precheck", {})
        payload_rules = (
            event_policy.get("payload_consistency", {}).get("rules") or []
        )
        has_evidence_gate = (
            isinstance(precheck, dict) and precheck.get("rules")
        ) or bool(payload_rules)
        if not has_evidence_gate:
            findings.add("evidence_bound_unbounded_retry")

    return findings


def _evidence_bound_results(
    fixtures_dir: Path,
) -> Iterable[tuple[str, bool]]:
    """Yield test results for evidence-bound correction fixture findings."""
    filename = "evidence-bound-negative-fixture.yml"
    fixture_path = fixtures_dir / filename
    if not fixture_path.is_file():
        print(f"MISSING evidence-bound fixture: {fixture_path}")
        yield "evidence_bound_findings:fixture_present", False
        return

    expected_findings = {
        "evidence_bound_missing_invariant",
        "evidence_bound_replacement_payload",
        "evidence_bound_no_target",
        "evidence_bound_unbounded_retry",
    }
    actual = _evidence_bound_findings(fixture_path)
    match = actual == expected_findings
    yield f"evidence_bound_findings:{filename}", match
    if not match:
        print(
            f"FAIL evidence-bound findings {filename}: "
            f"expected={sorted(expected_findings)!r} actual={sorted(actual)!r}"
        )
    else:
        print(f"OK evidence-bound findings {filename}: {sorted(actual)!r}")


def _key_stage_event_gate_results(fixtures_dir: Path) -> Iterable[tuple[str, bool]]:
    expected = {
        "key-stage-event-gate-positive-fixture.yml": set(),
        "key-stage-event-gate-missing-selection-negative-fixture.yml": {
            "preset.key_stage_event_gate_missing_selection",
            "preset.key_stage_event_gate_pending_status",
        },
        "key-stage-event-gate-divergence-negative-fixture.yml": {
            "preset.key_stage_event_gate_notes_preset_diverge",
            "preset.key_stage_event_gate_shared_budget",
            "preset.key_stage_event_gate_no_reason",
        },
        "key-stage-event-gate-no-reason-negative-fixture.yml": {
            "preset.key_stage_event_gate_no_reason",
        },
    }
    for filename, expected_findings in expected.items():
        actual = _key_stage_findings(fixtures_dir / filename)
        yield (
            f"key_stage_findings:{filename}",
            actual == expected_findings,
        )
        if actual != expected_findings:
            print(
                f"FAIL key-stage findings {filename}: "
                f"expected={sorted(expected_findings)!r} actual={sorted(actual)!r}"
            )
        else:
            print(f"OK key-stage findings {filename}: {sorted(actual)!r}")


def _check_fixture_present(
    fixtures_dir: Path, filename: str, finding_id: str
) -> bool:
    full = fixtures_dir / filename
    if not full.is_file():
        print(f"MISSING capability fixture: {full}")
        return False
    text = _read(full)
    # The fixture must advertise its expected review-only finding
    # in a stable, greppable position so reviewers can pair the
    # fixture with the corresponding finding-rubric table.
    if finding_id not in text:
        print(
            f"FAIL capability fixture {filename} does not advertise "
            f"finding id {finding_id!r}"
        )
        return False
    # Must keep a loadable surface (event_loop + hats), not empty.
    if "event_loop:" not in text or "hats:" not in text:
        print(
            f"FAIL capability fixture {filename} missing event_loop/hats"
        )
        return False
    if filename.startswith("key-stage-event-gate-"):
        notes = full.with_suffix(".preset-author-notes.md")
        if not notes.is_file():
            print(f"FAIL key-stage fixture missing notes: {notes}")
            return False
    print(f"OK capability fixture {filename} ({finding_id})")
    return True


def _check_runtime_verification_fixture(fixtures_dir: Path) -> bool:
    """Validate the runtime negative fixture's executable scenario contract."""
    fixture = fixtures_dir / "runtime-verify-negative-fixture.yml"
    value = yaml.safe_load(_read(fixture))
    if not isinstance(value, dict):
        print("FAIL runtime verification fixture is not a YAML mapping")
        return False
    contract = value.get("verification_scenarios")
    if not isinstance(contract, dict) or contract.get("version") != 1:
        print("FAIL runtime verification fixture missing version 1 scenarios")
        return False
    scenarios = contract.get("scenarios")
    if not isinstance(scenarios, list):
        print("FAIL runtime verification fixture scenarios must be a list")
        return False
    by_name = {item.get("name"): item for item in scenarios if isinstance(item, dict)}
    empty = by_name.get("empty-output")
    unclosed = by_name.get("unclosed-terminal")
    if not isinstance(empty, dict) or not isinstance(unclosed, dict):
        print("FAIL runtime fixture must define empty-output and unclosed-terminal")
        return False
    empty_outputs = [
        response.get("output")
        for response in empty.get("responses", [])
        if isinstance(response, dict)
    ]
    if "" not in empty_outputs or empty.get("expected_failure_kind") != "no_progress":
        print("FAIL empty-output case does not assert no_progress")
        return False
    unclosed_expect = unclosed.get("expect", {})
    if (
        not isinstance(unclosed_expect, dict)
        or unclosed.get("expected_failure_kind") != "unclosed_terminal"
        or unclosed_expect.get("terminal_topic") != "LOOP_COMPLETE"
        or not unclosed_expect.get("accepted_events")
    ):
        print("FAIL unclosed-terminal case does not assert accepted events without closure")
        return False
    print("OK runtime verification fixture covers no_progress and unclosed_terminal")
    return True


def _check_unique_advertised_ids(
    fixtures_dir: Path, advertised_ids: dict[str, str]
) -> bool:
    # Avoid silent collisions: each capability fixture announces its
    # own review-only finding id. Two fixtures must not advertise
    # the same id.
    fixtures_dir = _project_root() / "skills" / "ralph-preset-review" / "fixtures"
    seen_ids: dict[str, str] = {}
    collision = False
    for filename, finding_id in CAPABILITY_FIXTURES:
        prior = seen_ids.get(finding_id)
        if prior is not None and prior != filename:
            print(
                f"FAIL capability fixture {filename} collides with {prior} "
                f"on advertised finding id {finding_id!r}"
            )
            collision = True
        seen_ids[finding_id] = filename
    if not collision:
        print(
            "OK capability fixtures have unique advertised finding ids "
            f"({len(seen_ids)} ids)"
        )
    return not collision


def test_skill_anchors() -> None:
    """Run the runtime-verification contract under pytest.

    The direct script retains the repository's complete legacy fixture sweep;
    pytest owns the new runtime-verification anchors so unrelated historical
    fixture expectations do not mask this contract.
    """
    runtime_anchors = [
        item
        for item in ANCHORS
        if "ralph preset verify" in item[1]
        or item[1].startswith("verify.")
    ]
    results = [
        (f"anchor:{path}:{anchor}", _check_anchor(path, anchor))
        for path, anchor in runtime_anchors
    ]
    fixtures_dir = _project_root() / "skills" / "ralph-preset-review" / "fixtures"
    results.append(
        (
            "fixture:runtime-verify-negative-fixture.yml",
            _check_fixture_present(
                fixtures_dir,
                "runtime-verify-negative-fixture.yml",
                "verify.dynamic_evidence_missing",
            ),
        )
    )
    results.append(
        (
            "fixture:runtime-verification-scenarios",
            _check_runtime_verification_fixture(fixtures_dir),
        )
    )
    failures = [label for label, passed in results if not passed]
    assert not failures, f"skill anchor failures: {failures}"


if __name__ == "__main__":
    ok = True
    for label, passed in _iter_all():
        ok = passed and ok
    sys.exit(0 if ok else 1)
