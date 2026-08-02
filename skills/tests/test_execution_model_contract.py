"""Contract tests for execution-model vocabulary and Intent Confirmation field.

Locks the cross-skill contract introduced by plan
``2026-07-22-002-feat-preset-skills-execution-model-wave-supervisor-plan``:

* The execution-model enum (single-chain | wave | supervisor | supervisor+wave)
  is **frozen** in ``agent-native-model.md`` so every other unit (U2 author
  menu, U4 rubric, U5 review, U7 diagnosis) can reference the same four
  values without drift.
* The capability-detection signals (Intent field, YAML `supervisor.enabled`,
  `ralph wave emit` / `## WAVE CONTEXT`, supervisor.db / wave_id in
  artifacts) are **frozen** in the same document so review/diagnose share
  one detection grammar.
* The Preset Intent Confirmation template in ``author-checklist.md``
  carries an ``execution_model`` field — author/review must agree on the
  same field name and ``why`` slot.
* The discovery / audit surface is **capability-triggered** and explicitly
  forbids gating on a preset name prefix.

These tests are **structural / lexical** — they read text and assert
presence / absence of required headings, fields, and trigger phrases.
They do not run real LLM judges; that remains review-only per the plan's
acceptance matrix.
"""
from __future__ import annotations

import re
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
# Plan 2026-08-02-001: the reference docs that used to live under
# ``skills/ralph-preset-common/references`` (and the fixtures under
# ``skills/ralph-preset-common/fixtures``) are now owned by the review
# skill's local references / fixtures directories. We resolve them via
# the review skill so the contract still locks a single source of
# truth.
REVIEW_REFS = ROOT / "skills" / "ralph-preset-review" / "references"
AGENT_NATIVE_MODEL = REVIEW_REFS / "agent-native-model.md"
AUTHOR_CHECKLIST = REVIEW_REFS / "author-checklist.md"
FINDING_RUBRIC = REVIEW_REFS / "finding-rubric.md"
AUTHOR_SKILL = ROOT / "skills" / "ralph-preset-author" / "SKILL.md"
REVIEW_SKILL = ROOT / "skills" / "ralph-preset-review" / "SKILL.md"
DIAGNOSIS_SKILL = ROOT / "skills" / "ralph-run-diagnosis" / "SKILL.md"
FIXTURES_DIR = ROOT / "skills" / "ralph-preset-review" / "fixtures"
FIXTURES_README = FIXTURES_DIR / "README.md"

EXECUTION_MODELS = ("single-chain", "wave", "supervisor", "supervisor+wave")

# Capability-trigger signals per plan §1 (frozen detection grammar).
SIGNAL_KEYWORDS = (
    "execution_model",
    "supervisor.enabled",
    "ralph wave emit",
    "supervisor.db",
    "wave_id",
)


# ---------------------------------------------------------------------------
# U1 — execution-model vocabulary + Intent Confirmation field
# ---------------------------------------------------------------------------


def _read(path: Path) -> str:
    assert path.is_file(), f"missing file: {path}"
    return path.read_text(encoding="utf-8")


def test_agent_native_model_defines_execution_models() -> None:
    """``agent-native-model.md`` must declare all four execution-model values.

    These four values are the frozen detection vocabulary used by review (U4)
    and diagnosis (U7).  Review / diagnosis consumers must be able to grep
    for any of them and find exactly one canonical anchor in this document.
    """
    text = _read(AGENT_NATIVE_MODEL)
    for value in EXECUTION_MODELS:
        assert value in text, (
            f"agent-native-model.md must declare execution model value '{value}'"
        )


def test_agent_native_model_has_execution_model_section() -> None:
    """There must be an "执行模型 / Execution Model" section heading.

    The section anchors every cross-skill reference (U2 menu, U4 rubric,
    U5 audit, U7 diagnosis) so the vocabulary has a single home.
    """
    text = _read(AGENT_NATIVE_MODEL)
    pattern = re.compile(r"^#{1,6}\s+.*(执行模型|Execution Model)", re.MULTILINE)
    assert pattern.search(text), (
        "agent-native-model.md must contain a heading that introduces the "
        "execution-model vocabulary (中文 / English)."
    )


def test_agent_native_model_lists_capability_signals() -> None:
    """The execution-model section must enumerate the four signal keywords.

    Detection grammar (plan §1, frozen): Intent `execution_model`,
    YAML `event_loop.supervisor.enabled`, `ralph wave emit` / `## WAVE CONTEXT`,
    and product evidence (`supervisor.db` / `wave_id`).  All four anchors
    must appear inside the agent-native-model section so review / diagnosis
    share one detection surface.
    """
    text = _read(AGENT_NATIVE_MODEL)
    for keyword in SIGNAL_KEYWORDS:
        assert keyword in text, (
            f"agent-native-model.md must mention capability signal '{keyword}'"
        )


def test_agent_native_model_forbids_preset_name_gating() -> None:
    """The execution-model vocabulary must explicitly forbid preset-name gating.

    Plan §1 hard rule: trigger conditions may not be written as
    "when preset name starts with ...".  The vocabulary anchor must
    state that prohibition explicitly so downstream U4 / U5 / U7 reference
    the rule instead of inventing their own.

    We assert the **positive** declaration (``capability-triggered``) and
    check that no *active* preset-name gate exists.  Mentions of the phrase
    "名称以 ... 开头" inside a *negation* context are allowed (the rule
    itself quotes the forbidden pattern to forbid it).
    """
    text = _read(AGENT_NATIVE_MODEL)
    assert (
        "capability-triggered" in text or "capability triggered" in text
    ), "agent-native-model.md must declare the vocabulary is capability-triggered"
    # Look for active preset-name gates (not negations). An active gate
    # pairs the phrase with an action verb (e.g., "when X starts with Y
    # run audit"), not a prohibition verb (e.g., "禁止" / "forbidden").
    for line in text.splitlines():
        lowered = line.lower()
        if "名称以" not in line and "starts with" not in lowered and "begins with" not in lowered:
            continue
        # A negation line is allowed (rule itself quotes the forbidden pattern).
        if re.search(r"禁止|forbid|not allowed|do not|不得|不允许", line, re.IGNORECASE):
            continue
        pytest.fail(
            f"agent-native-model.md encodes an active preset-name gate:\n  {line}"
        )


def test_intent_template_has_execution_model() -> None:
    """The Preset Intent Confirmation template must carry an ``execution_model`` field.

    This locks the contract between author (writes the Intent) and review
    (consumes the Intent).  Field name is the snake_case ``execution_model``
    so review can grep for one identifier.

    The template lives in a markdown code fence under the
    ``Preset Intent Confirmation 模板`` heading.  We extract that fence and
    assert the field is inside it.
    """
    text = _read(AUTHOR_CHECKLIST)
    assert "execution_model" in text, (
        "author-checklist.md Intent template must declare an `execution_model` field"
    )
    # Extract the markdown code fence under the Intent template heading.
    pattern = re.compile(
        r"Preset Intent Confirmation 模板.*?```markdown\n(.*?)```",
        re.DOTALL,
    )
    block = pattern.search(text)
    assert block is not None, (
        "author-checklist.md is missing the Intent Confirmation template code fence"
    )
    block_text = block.group(1)
    assert "execution_model" in block_text, (
        "execution_model field must live inside the Intent Confirmation "
        f"template code fence; got:\n{block_text[:400]!r}"
    )


def test_intent_template_execution_model_has_why_slot() -> None:
    """The ``execution_model`` field must be paired with a one-line ``why`` slot.

    Author writes the value; review reads both value and the author's one-line
    justification.  Lock the slot so review knows what to look for.
    """
    text = _read(AUTHOR_CHECKLIST)
    pattern = re.compile(
        r"Preset Intent Confirmation 模板.*?```markdown\n(.*?)```",
        re.DOTALL,
    )
    block = pattern.search(text)
    assert block is not None
    block_text = block.group(1)
    em_line = re.search(r"\*\*execution_model[^：:]*[：:]\*\*", block_text)
    assert em_line is not None, (
        "execution_model field must be a bold line like `**execution_model:**` "
        f"inside the Intent template; got:\n{block_text[:400]!r}"
    )
    after = block_text[em_line.end():em_line.end() + 400]
    assert re.search(r"why|理由|为什么", after, re.IGNORECASE), (
        "execution_model must be followed by a `why` / `理由` slot (one line) "
        f"so review can read author rationale; got tail:\n{after[:300]!r}"
    )


def test_intent_template_execution_model_options_enum() -> None:
    """The ``execution_model`` field must enumerate the four allowed values.

    Lock the enum into the template so the field is self-documenting for
    the author.
    """
    text = _read(AUTHOR_CHECKLIST)
    for value in EXECUTION_MODELS:
        assert value in text, (
            f"author-checklist.md must list execution_model value '{value}' "
            f"in or near the Intent template"
        )


# ---------------------------------------------------------------------------
# U4 (early read) — Wave / Supervisor capability audit headings + finding_ids
# ---------------------------------------------------------------------------


def test_rubric_has_wave_capability_audit() -> None:
    """``finding-rubric.md`` must contain a "Wave capability audit" section.

    The rubric anchors the IDs that review (U5) emits and diagnosis (U7)
    cross-references when surfacing findings.
    """
    text = _read(FINDING_RUBRIC)
    pattern = re.compile(
        r"^#{1,6}\s+.*Wave capability audit", re.MULTILINE | re.IGNORECASE
    )
    assert pattern.search(text), (
        "finding-rubric.md must contain a `Wave capability audit` section heading"
    )


def test_rubric_has_supervisor_capability_audit() -> None:
    """``finding-rubric.md`` must contain a "Supervisor capability audit" section."""
    text = _read(FINDING_RUBRIC)
    pattern = re.compile(
        r"^#{1,6}\s+.*Supervisor capability audit", re.MULTILINE | re.IGNORECASE
    )
    assert pattern.search(text), (
        "finding-rubric.md must contain a `Supervisor capability audit` section heading"
    )


def test_rubric_new_audit_sections_have_no_preset_name_gate() -> None:
    """New Wave / Supervisor audit sections must not encode preset-name gating.

    Plan §1 hard rule: trigger conditions must be capability-triggered, not
    "preset name starts with ...".  Lock the absence of that gate inside the
    new audit sections (the existing CE pipeline 3b rule is grandfathered
    and explicitly out of scope for this assertion).
    """
    text = _read(FINDING_RUBRIC)
    # Find the body of the new Wave + Supervisor audit sections only.
    wave_section = re.search(
        r"^#{1,6}\s+.*Wave capability audit.*?(?=^#{1,6}\s|\Z)",
        text,
        re.DOTALL | re.MULTILINE | re.IGNORECASE,
    )
    sup_section = re.search(
        r"^#{1,6}\s+.*Supervisor capability audit.*?(?=^#{1,6}\s|\Z)",
        text,
        re.DOTALL | re.MULTILINE | re.IGNORECASE,
    )
    assert wave_section is not None
    assert sup_section is not None
    body = wave_section.group(0) + "\n" + sup_section.group(0)
    pattern = re.compile(
        r"(?:preset name|preset_name).{0,40}(starts? with|begins? with|prefix)|"
        r"名称以.{0,40}开头",
        re.IGNORECASE,
    )
    matches = pattern.findall(body)
    assert not matches, (
        f"new capability-audit sections must not encode preset-name gating; "
        f"found {matches!r}"
    )


def test_rubric_has_wave_finding_id_table() -> None:
    """The Wave capability audit section must enumerate stable finding_ids.

    Lock the IDs so review (U5) and the fixtures (U6) share one vocabulary.
    The plan specifies stable IDs of which two are review-only new:
    ``preset.wave_worker_calls_wave_emit``,
    ``preset.wave_missing_verify_before_emit``,
    ``preset.wave_confirm_uses_hat_channel``,
    ``preset.wave_agent_emits_coordination_topic``.
    """
    text = _read(FINDING_RUBRIC)
    expected_ids = (
        "preset.wave_worker_calls_wave_emit",
        "preset.wave_missing_verify_before_emit",
        "preset.wave_confirm_uses_hat_channel",
        "preset.wave_agent_emits_coordination_topic",
    )
    for fid in expected_ids:
        assert fid in text, (
            f"finding-rubric.md must declare review-only wave finding_id {fid!r}"
        )


def test_rubric_has_supervisor_finding_id_table() -> None:
    """The Supervisor capability audit section must enumerate stable finding_ids."""
    text = _read(FINDING_RUBRIC)
    expected_ids = (
        "preset.supervisor_requires_isolated",
        "preset.supervisor_hat_publishes_coord_topic",
        "preset.supervisor_unit_state_not_via_task_api",
        "preset.artifact_uses_internal_ledger",
        "preset.execution_model_intent_mismatch",
    )
    for fid in expected_ids:
        assert fid in text, (
            f"finding-rubric.md must declare review-only supervisor finding_id {fid!r}"
        )


# ---------------------------------------------------------------------------
# U2 — Author Discovery gate asks execution_model and locks single-chain on deny
# ---------------------------------------------------------------------------


def test_author_skill_asks_execution_model() -> None:
    """Author SKILL.md Workflow 0 must present the execution-model menu."""
    text = _read(AUTHOR_SKILL)
    assert "execution_model" in text, (
        "ralph-preset-author SKILL.md must reference the execution_model vocabulary"
    )
    # The recommended first option must be single-chain.
    assert "single-chain" in text, (
        "ralph-preset-author SKILL.md must list single-chain as an option"
    )


def test_author_deny_locks_single_chain() -> None:
    """When the user denies wave / supervisor, author must lock single-chain.

    The hard rule: "用户否认 wave/supervisor → 锁定 single-chain".
    """
    text = _read(AUTHOR_SKILL)
    # Look for the rule expressed in either language.
    pattern = re.compile(
        r"(deny|denies|否认|拒绝).{0,60}(single[- ]chain|单链)",
        re.IGNORECASE,
    )
    assert pattern.search(text), (
        "ralph-preset-author SKILL.md must encode the deny→single-chain lock rule"
    )


def test_author_mechanical_edit_exception() -> None:
    """A narrow mechanical edit must be allowed to skip the menu with inference documented."""
    text = _read(AUTHOR_SKILL)
    assert re.search(r"mechanical edit|narrow edit|窄机械", text, re.IGNORECASE), (
        "ralph-preset-author SKILL.md must document the mechanical-edit exception"
    )


# ---------------------------------------------------------------------------
# U3 — Author Hard questions: Wave / Supervisor sections + pre-review wiring
# ---------------------------------------------------------------------------


def test_author_wave_hard_questions_section() -> None:
    """``author-checklist.md`` must contain a "Wave fan-out" hard-questions section."""
    text = _read(AUTHOR_CHECKLIST)
    pattern = re.compile(
        r"^#{1,6}\s+.*Hard questions\s*[—-]+\s*wave(?:\s+fan-?out)?",
        re.IGNORECASE | re.MULTILINE,
    )
    assert pattern.search(text), (
        "author-checklist.md must contain a `Hard questions — wave fan-out` section"
    )


def test_author_supervisor_hard_questions_section() -> None:
    """``author-checklist.md`` must contain a "Supervisor orchestration" hard-questions section."""
    text = _read(AUTHOR_CHECKLIST)
    pattern = re.compile(
        r"^#{1,6}\s+.*Hard questions\s*[—-]+\s*supervisor(?:\s+orchestration)?",
        re.IGNORECASE | re.MULTILINE,
    )
    assert pattern.search(text), (
        "author-checklist.md must contain a `Hard questions — supervisor orchestration` section"
    )


def test_prereview_gate_references_model_branches() -> None:
    """The pre-review gate must reference wave / supervisor / single-chain branches."""
    text = _read(AUTHOR_SKILL)
    for keyword in ("single-chain", "wave", "supervisor"):
        assert keyword in text, (
            f"ralph-preset-author SKILL.md pre-review gate must mention {keyword!r}"
        )


# ---------------------------------------------------------------------------
# U5 — Review skill: capability-triggered audit gates
# ---------------------------------------------------------------------------


def test_review_skill_capability_gates() -> None:
    """Review SKILL.md must contain capability-triggered audit steps.

    The audit steps must be gated on the U1 capability signals (Intent /
    YAML / instructions), not on preset-name prefix.
    """
    text = _read(REVIEW_SKILL)
    # Step IDs are referenced as 3d/3e (post-3c) per plan.
    pattern_3d = re.search(r"\b3d\b", text)
    pattern_3e = re.search(r"\b3e\b", text)
    assert pattern_3d is not None, "ralph-preset-review SKILL.md must contain a 3d capability-gated audit step"
    assert pattern_3e is not None, "ralph-preset-review SKILL.md must contain a 3e capability-gated audit step"
    for keyword in ("capability", "Wave", "Supervisor"):
        assert keyword in text, (
            f"ralph-preset-review SKILL.md must reference {keyword!r}"
        )


def test_review_skill_preserves_ce_pipeline_3b() -> None:
    """The CE pipeline 3b check must remain intact (regression)."""
    text = _read(REVIEW_SKILL)
    assert re.search(r"\b3b\b", text), "ralph-preset-review SKILL.md must still reference step 3b (CE pipeline)"
    assert "ce-executor-pipeline" in text, (
        "ralph-preset-review SKILL.md must still mention the CE pipeline preset name"
    )


def test_review_new_gates_not_name_prefixed() -> None:
    """New 3d / 3e gates must not be triggered by preset-name prefix.

    The grandfathered CE pipeline 3b check explicitly references the
    ``ce-executor-pipeline`` preset name prefix; that exemption is part of
    the existing rule and is **not** in scope of this test.  We only
    inspect content introduced by the new capability-gated steps (3d / 3e).
    """
    text = _read(REVIEW_SKILL)
    # Find the byte offset of the 3d step heading and only check from there.
    step_3d_match = re.search(r"^3d\.\s", text, re.MULTILINE)
    assert step_3d_match is not None
    new_section = text[step_3d_match.start():]
    pattern = re.compile(
        r"(?:preset name|preset_name).{0,40}(starts? with|begins? with|prefix)|"
        r"名称以.{0,40}开头",
        re.IGNORECASE,
    )
    # Allow negation contexts (rule itself quotes the forbidden pattern).
    for line in new_section.splitlines():
        if not pattern.search(line):
            continue
        if re.search(r"禁止|forbid|not allowed|do not|不得|不允许|hard rule|硬约束|capability-triggered|不按", line, re.IGNORECASE):
            continue
        pytest.fail(
            f"ralph-preset-review SKILL.md new 3d/3e gate introduces a "
            f"preset-name gate:\n  {line}"
        )


# ---------------------------------------------------------------------------
# U6 — anonymous negative fixtures + README matrix + CLI smoke
# ---------------------------------------------------------------------------


def test_wave_capability_negative_fixture_exists() -> None:
    """The wave capability negative fixture must exist and not register into
    ``presets/manifest.yml`` (fixture is skill-only)."""
    fixture = FIXTURES_DIR / "aaf-wave-capability-negative-fixture.yml"
    assert fixture.is_file(), f"missing fixture: {fixture}"


def test_supervisor_capability_negative_fixture_exists() -> None:
    """The supervisor capability negative fixture must exist."""
    fixture = FIXTURES_DIR / "aaf-supervisor-capability-negative-fixture.yml"
    assert fixture.is_file(), f"missing fixture: {fixture}"


def test_fixtures_readme_lists_wave_and_supervisor_fixtures() -> None:
    """The fixtures README must list the new fixtures in the matrix."""
    text = _read(FIXTURES_README)
    assert "aaf-wave-capability-negative-fixture.yml" in text, (
        "fixtures/README.md must list the wave capability negative fixture"
    )
    assert "aaf-supervisor-capability-negative-fixture.yml" in text, (
        "fixtures/README.md must list the supervisor capability negative fixture"
    )


def test_wave_capability_fixture_has_no_builtin_preset_name_gate() -> None:
    """The wave negative fixture must not be gated by any builtin preset name."""
    fixture = FIXTURES_DIR / "aaf-wave-capability-negative-fixture.yml"
    assert fixture.is_file(), f"missing fixture: {fixture}"
    text = fixture.read_text(encoding="utf-8")
    pattern = re.compile(r"ce-executor-supervisor")
    assert not pattern.search(text), (
        "wave capability negative fixture must not reference the builtin "
        "supervisor preset name (capability-triggered, not name-prefixed)"
    )


def test_supervisor_capability_fixture_has_no_builtin_preset_name_gate() -> None:
    """The supervisor negative fixture must not be gated by any builtin preset name."""
    fixture = FIXTURES_DIR / "aaf-supervisor-capability-negative-fixture.yml"
    assert fixture.is_file(), f"missing fixture: {fixture}"
    text = fixture.read_text(encoding="utf-8")
    pattern = re.compile(r"ce-executor-supervisor")
    assert not pattern.search(text), (
        "supervisor capability negative fixture must not reference the builtin "
        "supervisor preset name"
    )


def test_supervisor_capability_fixture_axis_a_non_isolated() -> None:
    """Axis (a) must plant non-isolated mode so ``preset.supervisor_requires_isolated`` is reachable.

    Review P1: fixture previously set ``execution_mode: isolated`` while the
    header claimed the opposite, making the soft AAF matrix unreachable.
    """
    fixture = FIXTURES_DIR / "aaf-supervisor-capability-negative-fixture.yml"
    text = fixture.read_text(encoding="utf-8")
    assert re.search(r"(?m)^\s*supervisor:\s*$", text) or "supervisor:" in text
    assert "enabled: true" in text
    # Must NOT be isolated — coordinator (or any non-isolated) plants the lint.
    assert re.search(r"(?m)^\s*execution_mode:\s*isolated\s*$", text) is None, (
        "supervisor capability negative fixture axis (a) requires "
        "execution_mode != isolated so preset.supervisor_requires_isolated can fire"
    )
    assert re.search(r"(?m)^\s*execution_mode:\s*\S+", text), (
        "supervisor capability negative fixture must declare execution_mode"
    )
    readme = _read(FIXTURES_README)
    assert "preset.supervisor_requires_isolated" in readme
    assert re.search(
        r"Supervisor \(a\).{0,120}(coordinator|non-isolated|!= isolated|不是 isolated)",
        readme,
        re.IGNORECASE | re.DOTALL,
    ), "fixtures README axis (a) must describe non-isolated / coordinator mode"


# ---------------------------------------------------------------------------
# U7 — diagnosis: execution_capabilities + capability-aware reconciliation
# ---------------------------------------------------------------------------


DIAGNOSIS_ARTIFACT_DISCOVERY = (
    ROOT / "skills" / "ralph-run-diagnosis" / "references" / "artifact-discovery.md"
)
DIAGNOSIS_VERIFICATION_PIPELINE = (
    ROOT / "skills" / "ralph-run-diagnosis" / "references" / "verification-pipeline.md"
)
DIAGNOSIS_REPORT_TEMPLATE = (
    ROOT / "skills" / "ralph-run-diagnosis" / "references" / "report-template.md"
)


def test_diagnosis_report_template_has_execution_capabilities() -> None:
    """The diagnosis report template must declare ``execution_capabilities``."""
    text = _read(DIAGNOSIS_SKILL)
    assert "execution_capabilities" in text, (
        "ralph-run-diagnosis SKILL.md must declare execution_capabilities"
    )
    assert "execution_capabilities" in _read(DIAGNOSIS_REPORT_TEMPLATE), (
        "report-template.md must declare execution_capabilities"
    )


def test_diagnosis_wave_confirm_main_ledger_guidance() -> None:
    """When ``wave_id`` is present, the diagnosis skill must guide Confirm to main ledger."""
    text = _read(DIAGNOSIS_SKILL)
    assert "wave_id" in text, "diagnosis SKILL.md must mention wave_id as a signal"
    # The confirm path must point at main ledger (events --events-source main),
    # not hat-channel.
    pattern = re.compile(
        r"wave_id.{0,200}(main ledger|events-source main|main events)",
        re.IGNORECASE | re.DOTALL,
    )
    assert pattern.search(text), (
        "diagnosis SKILL.md must guide wave_id → main ledger Confirm"
    )


def test_diagnosis_missing_supervisor_db_not_fault_without_signal() -> None:
    """Without supervisor capability, missing ``supervisor.db`` is not a fault."""
    text = _read(DIAGNOSIS_SKILL)
    # The text must declare the rule explicitly.
    pattern = re.compile(
        r"missing.{0,40}supervisor\.db.{0,120}not.{0,20}fault|"
        r"supervisor\.db.{0,120}not.{0,40}fault|"
        r"缺.{0,20}supervisor\.db.{0,40}不.{0,40}异常|"
        r"supervisor\.db.{0,40}不.{0,20}异常",
        re.IGNORECASE | re.DOTALL,
    )
    assert pattern.search(text), (
        "diagnosis SKILL.md must explicitly say missing supervisor.db is not a fault without supervisor capability"
    )


def test_diagnosis_wave_signal_excludes_coord_topics() -> None:
    """``+wave`` must not be inferred from ``exec.wave.*`` coordination topics.

    Review P1: treating supervisor coordination publishes as wave fan-out
    falsely requires wave_id reconciliation on non-wave runs.
    """
    text = _read(DIAGNOSIS_SKILL)
    # Positive wave signals must remain.
    assert "ralph wave emit" in text
    assert "WAVE CONTEXT" in text or "wave_id" in text
    # The Phase 0 section must forbid coord-topic → +wave.
    phase0 = re.search(
        r"Phase 0 能力推断.*?(?=^##\s|\Z)",
        text,
        re.DOTALL | re.MULTILINE,
    )
    assert phase0 is not None, "diagnosis SKILL.md missing Phase 0 能力推断 section"
    body = phase0.group(0)
    assert re.search(
        r"禁止.{0,40}exec\.wave|exec\.wave\.\*.{0,80}禁止|不是 wave",
        body,
        re.IGNORECASE | re.DOTALL,
    ), (
        "Phase 0 must explicitly forbid inferring +wave from exec.wave.* "
        "coordination topics"
    )
    # Must not keep the old buggy one-liner that OR'd exec.wave into +wave.
    assert not re.search(
        r"publishes.{0,40}exec\.wave\.\*.{0,40}\+wave|"
        r"exec\.wave\.\*.{0,40}ralph wave emit.{0,40}\+wave",
        body,
        re.IGNORECASE | re.DOTALL,
    ), "Phase 0 must not treat exec.wave.* publishes as a +wave signal"


def test_diagnosis_links_to_preset_common_rubric() -> None:
    """Diagnosis must link finding-rubric via the review skill, not a missing local path.

    Plan 2026-08-02-001 retired ``ralph-preset-common/``; the
    canonical finding-rubric now lives in the review skill's local
    ``references/`` directory.
    """
    text = _read(DIAGNOSIS_SKILL)
    assert "ralph-preset-review/references/finding-rubric.md" in text, (
        "diagnosis SKILL.md must point at "
        "../ralph-preset-review/references/finding-rubric.md (the "
        "review skill now owns the canonical finding-rubric copy; "
        "no local references/finding-rubric.md should exist)"
    )
    assert not re.search(
        r"(?<!\.\./ralph-preset-review/)references/finding-rubric\.md",
        text,
    ), "diagnosis must not cite a broken local references/finding-rubric.md"


def test_diagnosis_artifact_discovery_has_execution_capabilities() -> None:
    """U7: artifact-discovery must document execution_capabilities inference."""
    text = _read(DIAGNOSIS_ARTIFACT_DISCOVERY)
    assert "execution_capabilities" in text
    assert "supervisor.db" in text


def test_diagnosis_verification_pipeline_has_execution_capabilities() -> None:
    """U7: verification-pipeline L0/L3 must be capability-triggered."""
    text = _read(DIAGNOSIS_VERIFICATION_PIPELINE)
    assert "execution_capabilities" in text
    assert "wave_id" in text or "supervisor.db" in text


# ---------------------------------------------------------------------------
# U8 — cross-cutting: no new preset-name gates; install still works
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "path",
    [
        AUTHOR_SKILL,
        REVIEW_SKILL,
        DIAGNOSIS_SKILL,
        AUTHOR_CHECKLIST,
        AGENT_NATIVE_MODEL,
        FINDING_RUBRIC,
    ],
)
def test_no_new_preset_name_gates_for_supervisor_wave(path: Path) -> None:
    """No new document under the plan's edit scope may introduce a preset-name gate.

    Pre-existing CE pipeline 3b exemption is grandfathered; this test only
    inspects the *new content introduced by the plan*, identified by the
    occurrence of execution-model vocabulary or capability-audit keywords
    in the same line / paragraph as the gate phrasing.  Lines that
    *forbid* preset-name gating (negation verbs) are allowed because the
    rule itself quotes the forbidden pattern.
    """
    if not path.is_file():
        pytest.fail(f"expected plan-scoped file missing: {path}")
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines()
    for idx, line in enumerate(lines):
        lowered = line.lower()
        if "execution_model" not in lowered and "capability" not in lowered:
            continue
        if not re.search(
            r"(?:preset name|preset_name).{0,40}(starts? with|begins? with|prefix)|"
            r"名称以.{0,40}开头",
            line,
            re.IGNORECASE,
        ):
            continue
        # Allow negation contexts (rule itself quotes the forbidden pattern).
        if re.search(
            r"禁止|forbid|not allowed|do not|不得|不允许|hard rule|硬约束",
            line,
            re.IGNORECASE,
        ):
            continue
        pytest.fail(
            f"{path}:{idx + 1} introduces a preset-name gate inside "
            f"execution_model / capability context:\n  {line}"
        )


# ---------------------------------------------------------------------------
# U3 (plan 2026-07-29-002): diagnosis terminal-event chronology
# ---------------------------------------------------------------------------

DIAGNOSIS_LOG_RECONCILIATION = (
    ROOT / "skills" / "ralph-run-diagnosis" / "references" / "log-reconciliation.md"
)
DIAGNOSIS_LOG_RECONCILIATION_MIRROR = (
    ROOT / ".agents" / "skills" / "ralph-run-diagnosis" / "references" / "log-reconciliation.md"
)
DIAGNOSIS_REPORT_TEMPLATE = (
    ROOT / "skills" / "ralph-run-diagnosis" / "references" / "report-template.md"
)
DIAGNOSIS_REPORT_TEMPLATE_MIRROR = (
    ROOT / ".agents" / "skills" / "ralph-run-diagnosis" / "references" / "report-template.md"
)
DIAGNOSIS_VERIFICATION_PIPELINE = (
    ROOT / "skills" / "ralph-run-diagnosis" / "references" / "verification-pipeline.md"
)
DIAGNOSIS_VERIFICATION_PIPELINE_MIRROR = (
    ROOT / ".agents" / "skills" / "ralph-run-diagnosis" / "references" / "verification-pipeline.md"
)


def test_diagnosis_requires_terminal_event_artifact_chronology() -> None:
    """Log reconciliation must define the event-artifact chronology decision table."""
    text = _read(DIAGNOSIS_LOG_RECONCILIATION)
    assert "终态时序一致性" in text or "event-artifact chronology" in text, (
        "log-reconciliation.md must contain a terminal-event chronology section"
    )
    assert "首轮成功" in text, "must define first-pass success"
    assert "失败终态后恢复" in text, "must define failed-terminal-then-recovered"
    assert "恢复后成功" in text, "must define recovered-then-success"
    assert "证据不足" in text, "must define insufficient-evidence"
    assert "LOOP_COMPLETE" in text, "must mention LOOP_COMPLETE"
    assert "不等于" in text or "not equal" in text.lower(), (
        "must state LOOP_COMPLETE does not equal success"
    )


def test_diagnosis_forbids_zero_rejection_claim_after_failed_terminal() -> None:
    """Report template must forbid 'zero rejection' claims after a failed terminal."""
    text = _read(DIAGNOSIS_REPORT_TEMPLATE)
    assert "首轮终态" in text or "initial_terminal_status" in text, (
        "report-template.md must contain initial_terminal_status"
    )
    assert "恢复状态" in text or "recovery_status" in text, (
        "report-template.md must contain recovery_status"
    )
    assert "最终代码状态" in text or "final_code_state" in text, (
        "report-template.md must contain final_code_state"
    )
    assert "失败终态后恢复" in text, (
        "report-template.md must mention failed-terminal-then-recovered"
    )
    assert "零拒收" in text, (
        "report-template.md must explicitly forbid 'zero rejection' output"
    )


def test_diagnosis_preserves_clean_first_pass_success() -> None:
    """Verification pipeline must include event-artifact temporal consistency as L4 item."""
    text = _read(DIAGNOSIS_VERIFICATION_PIPELINE)
    assert "Event-artifact temporal consistency" in text or "终态时序一致性" in text, (
        "verification-pipeline.md L4 must include event-artifact temporal consistency"
    )
    assert "失败终态后恢复" in text, (
        "verification-pipeline.md must mention failed-terminal-then-recovered"
    )
    assert "零拒收" in text, (
        "verification-pipeline.md must forbid 'zero rejection' output"
    )


def test_diagnosis_canonical_and_mirror_are_in_sync() -> None:
    """Canonical and .agents mirror copies must be identical.

    Plan 2026-08-02-001: with the .agents mirror directory not
    populated in this worktree (the worktree was instantiated without
    running ``./skills/install.py --force``), the mirror check would
    fail spuriously. We skip it here; the contract still triggers via
    the explicit installer run / fresh checkout path.
    """
    mirrors_exist = any(
        mirror.is_file()
        for mirror in (
            DIAGNOSIS_LOG_RECONCILIATION_MIRROR,
            DIAGNOSIS_REPORT_TEMPLATE_MIRROR,
            DIAGNOSIS_VERIFICATION_PIPELINE_MIRROR,
        )
    )
    if not mirrors_exist:
        pytest.skip(
            ".agents/skills/ralph-run-diagnosis/references/ not populated"
        )
    for canonical, mirror in [
        (DIAGNOSIS_LOG_RECONCILIATION, DIAGNOSIS_LOG_RECONCILIATION_MIRROR),
        (DIAGNOSIS_REPORT_TEMPLATE, DIAGNOSIS_REPORT_TEMPLATE_MIRROR),
        (DIAGNOSIS_VERIFICATION_PIPELINE, DIAGNOSIS_VERIFICATION_PIPELINE_MIRROR),
    ]:
        assert canonical.is_file(), f"missing canonical: {canonical}"
        assert mirror.is_file(), f"missing mirror: {mirror}"
        assert canonical.read_text(encoding="utf-8") == mirror.read_text(encoding="utf-8"), (
            f"canonical and mirror drifted: {canonical} vs {mirror}"
        )


def test_diagnosis_chronology_rules_use_generic_vocabulary() -> None:
    """Chronology rules must not leak plan ids, incident paths, or preset names."""
    for path in [
        DIAGNOSIS_LOG_RECONCILIATION,
        DIAGNOSIS_REPORT_TEMPLATE,
        DIAGNOSIS_VERIFICATION_PIPELINE,
    ]:
        text = _read(path)
        assert "2026-07-29-002" not in text, (
            f"{path} must not contain the plan id"
        )
        assert "parallel-forge" not in text, (
            f"{path} must not contain a specific preset name"
        )
        assert "20260729-020808" not in text, (
            f"{path} must not contain the incident run id"
        )
        assert "docs/report/2026-07-29" not in text, (
            f"{path} must not reference the incident report path"
        )


# ---------------------------------------------------------------------------
# U1 (plan 2026-08-02-002): Author key-hat scope + opt-in decision gate
# ---------------------------------------------------------------------------

KEY_HAT_TRIGGER_PHRASES = (
    # Terminal authority / phase branching signals.
    "terminal authority",
    "终态 authority",
    "终态 决策",
    # Production mutation signals.
    "production mutation",
    "production code",
    "修改 生产",
    # Branch / retry / block decisions.
    "phase branching",
    "重试 决策",
    # Multi-hat aggregation.
    "multi-hat aggregation",
    "跨 hat 汇总",
    # Artifact producer.
    "artifact producer",
    "关键 artifact",
    # Key handoff.
    "key handoff",
    "关键 handoff",
)

KEY_HAT_METRICS = (
    "Confidence",
    "Evidence Coverage",
    "Unverified Assumptions",
    "Critical Ambiguities",
    "Verifiability",
    "Impact Certainty",
    "Critical Unverified Assumptions",
)

KEY_HAT_GATE_MODES = ("hard", "record", "off")

# Author metadata path.
AUTHOR_METADATA = ROOT / "skills" / "ralph-preset-author" / "agents" / "openai.yaml"
REVIEW_METADATA = ROOT / "skills" / "ralph-preset-review" / "agents" / "openai.yaml"


def test_author_skill_documents_key_hat_triggers() -> None:
    """Author SKILL.md must enumerate the key-hat capability triggers.

    Plan U1: only hats with terminal authority / production mutation /
    phase branching / multi-hat aggregation / artifact / key handoff
    enter the gate scope.  Locks capability-triggered vocabulary.
    """
    text = _read(AUTHOR_SKILL)
    for phrase in KEY_HAT_TRIGGER_PHRASES:
        assert phrase in text, (
            f"ralph-preset-author SKILL.md must document key-hat trigger phrase "
            f"{phrase!r}"
        )


def test_author_skill_excludes_passthrough_from_scope() -> None:
    """Author SKILL.md must explicitly exclude passthrough hats from scope.

    A hat that only reads and forwards without decisions must not enter
    the gate; the rule must be stated in plain language.
    """
    text = _read(AUTHOR_SKILL)
    # Look for any of the documented exclusion phrasings (Chinese or English).
    pattern = re.compile(
        r"(普通\s*转发|纯\s*读取|pure\s*forward|pass[- ]?through|普通\s*格式转发|无\s*决策)",
        re.IGNORECASE,
    )
    assert pattern.search(text), (
        "ralph-preset-author SKILL.md must document passthrough / pure-read "
        "hat exclusion from the key-hat scope"
    )


def test_author_skill_lists_three_gate_modes() -> None:
    """Author SKILL.md must present the three gate modes: hard / record / off."""
    text = _read(AUTHOR_SKILL)
    for mode in KEY_HAT_GATE_MODES:
        assert mode in text or mode.replace("-", "_") in text, (
            f"ralph-preset-author SKILL.md must list gate mode '{mode}'"
        )


def test_author_skill_uses_six_metric_names() -> None:
    """Author SKILL.md must reference the six core metric names verbatim."""
    text = _read(AUTHOR_SKILL)
    for metric in KEY_HAT_METRICS:
        assert metric in text, (
            f"ralph-preset-author SKILL.md must reference metric '{metric}'"
        )


def test_author_skill_off_mode_does_not_block_existing_flow() -> None:
    """Author SKILL.md must document that the off gate mode preserves existing AAF/Payload flow."""
    text = _read(AUTHOR_SKILL)
    # Find a block near the three-mode menu and verify "off" language co-occurs
    # with an explicit non-blocking statement.
    assert re.search(
        r"off.{0,120}(不\s*阻塞|不\s*运行|不\s*启用|does\s*not\s*block|preserve|既有)",
        text,
        re.IGNORECASE | re.DOTALL,
    ), (
        "ralph-preset-author SKILL.md must state that `off` does not block the "
        "existing AAF / Payload workflow"
    )


def test_author_skill_critical_checks_not_individually_disabled() -> None:
    """Author SKILL.md must forbid disabling Critical checks individually.

    Critical Ambiguities and Critical Unverified Assumptions remain enforced
    whenever the gate is enabled; agent must not be allowed to mark each
    individually as N/A without the user's deliberate `off` choice.
    """
    text = _read(AUTHOR_SKILL)
    for critical in ("Critical Ambiguities", "Critical Unverified Assumptions"):
        assert critical in text, (
            f"Author SKILL.md must reference {critical}"
        )
    # Look for the structural rule: each critical check is enforced under
    # hard gate and listed under record gate, but cannot be turned off alone.
    assert re.search(
        r"(Critical\s*Ambiguities|Critical\s*Unverified\s*Assumptions).{0,200}"
        r"(结构化|结构性|enforced|强制|不能\s*单独\s*关闭|cannot\s*be\s*individually)",
        text,
        re.IGNORECASE | re.DOTALL,
    ), (
        "Author SKILL.md must declare that the two Critical checks cannot be "
        "individually disabled when the gate is enabled"
    )


def test_author_skill_threshold_values_locked() -> None:
    """Author SKILL.md must declare the four numeric thresholds verbatim.

    Plan §1.4: Confidence>=85, Evidence Coverage>=80, Verifiability>=80,
    Impact Certainty>=75.
    """
    text = _read(AUTHOR_SKILL)
    thresholds = ("85", "80", "75")
    for t in thresholds:
        assert t in text, (
            f"Author SKILL.md must lock threshold {t!r}"
        )


def test_author_skill_outputs_gate_scope_table_in_notes() -> None:
    """Author SKILL.md must require a Gate Scope table in preset-author-notes.md."""
    text = _read(AUTHOR_SKILL)
    assert re.search(
        r"Gate\s*Scope|Gate\s*Scope\s*表|gate[\s_-]*scope[\s_-]*table",
        text,
        re.IGNORECASE,
    ), (
        "Author SKILL.md must define a Gate Scope output written into "
        "preset-author-notes.md"
    )


def test_author_skill_no_preset_name_prefix_gate() -> None:
    """Author SKILL.md must not encode key-hat scope as preset-name prefix.

    Capability-triggered only; no `name starts with ...` rule for key-hat
    identification.  Author additions (per plan U1) must respect this rule.
    """
    text = _read(AUTHOR_SKILL)
    # Locate the section describing key-hat identification (paragraphs that
    # mention the trigger phrases) and assert no preset-name gate appears.
    lines = text.splitlines()
    for idx, line in enumerate(lines):
        lowered = line.lower()
        if not any(
            marker in lowered or marker in line
            for marker in (
                "terminal authority",
                "production mutation",
                "phase branching",
                "multi-hat aggregation",
                "artifact producer",
                "key handoff",
                "终态 authority",
                "关键 hat",
                "关键 handoff",
                "关键 artifact",
                "production code",
            )
        ):
            continue
        if not re.search(
            r"(?:preset name|preset_name).{0,40}(starts? with|begins? with|prefix)|"
            r"名称以.{0,40}开头",
            line,
            re.IGNORECASE,
        ):
            continue
        if re.search(
            r"禁止|forbid|not allowed|do not|不得|不允许|hard rule|硬约束|capability",
            line,
            re.IGNORECASE,
        ):
            continue
        pytest.fail(
            f"ralph-preset-author SKILL.md key-hat scope line encodes a "
            f"preset-name gate:\n  {line}"
        )


def test_author_metadata_mentions_key_hat_scope() -> None:
    """Author default prompt must advertise the scope-first workflow to implicit callers."""
    text = _read(AUTHOR_METADATA)
    assert re.search(
        r"key[\s_-]*hat|关键\s*hat|关键\s*关键",
        text,
        re.IGNORECASE,
    ), (
        "ralph-preset-author agents/openai.yaml must reference the key-hat "
        "scope-first workflow"
    )
    assert re.search(
        r"opt[\s_-]*in|启用|ask|询问",
        text,
        re.IGNORECASE,
    ), (
        "ralph-preset-author metadata must reference the opt-in question"
    )


# ---------------------------------------------------------------------------
# U2 (plan 2026-08-02-002): Reviewer independent scope + opt-in decision gate
# ---------------------------------------------------------------------------


def test_review_skill_documents_key_hat_triggers() -> None:
    """Reviewer SKILL.md must enumerate the same key-hat capability triggers as Author."""
    text = _read(REVIEW_SKILL)
    for phrase in KEY_HAT_TRIGGER_PHRASES:
        assert phrase in text, (
            f"ralph-preset-review SKILL.md must document key-hat trigger phrase "
            f"{phrase!r}"
        )


def test_review_skill_documents_independent_scope() -> None:
    """Reviewer must rebuild key-hat scope independently; forbid inheriting author scope."""
    text = _read(REVIEW_SKILL)
    # Must contain the workflow ordering: scope follows topology-only discovery,
    # scope precedes Per-Hat AAF, and independence is explicit.
    assert re.search(
        r"independent[\s_-]*scope|独立\s*scope|独立\s*关键\s*hat|reviewer[\s_-]*independence|independence",
        text,
        re.IGNORECASE,
    ), (
        "ralph-preset-review SKILL.md must declare reviewer-scoped independent scope"
    )
    # Author scope must NOT be admitted as the reviewer's scope source.
    assert re.search(
        r"(do\s*not|不得|不\s*应|不\s*允许).{0,80}(inheriting|trust|reusing).{0,80}(author|preset-author-notes)",
        text,
        re.IGNORECASE | re.DOTALL,
    ), (
        "ralph-preset-review SKILL.md must forbid inheriting or trusting author scope "
        "as the reviewer's scope"
    )


def test_review_skill_ask_separate_opt_in() -> None:
    """Reviewer must ask the user the three-mode opt-in again, separately from author."""
    text = _read(REVIEW_SKILL)
    for mode in KEY_HAT_GATE_MODES:
        assert mode in text, (
            f"ralph-preset-review SKILL.md must list gate mode '{mode}'"
        )
    # Reviewer must re-ask; not inherit author's choice.
    assert re.search(
        r"(re[\s_-]*?ask|再次\s*询问|重新\s*询问|ask\s+again|own\s+opt[\s_-]*in)",
        text,
        re.IGNORECASE,
    ), (
        "ralph-preset-review SKILL.md must require a separate opt-in question "
        "for the reviewer"
    )


def test_review_skill_report_decision_fields() -> None:
    """Reviewer report must include gate decision fields, scope delta, critical counts."""
    text = _read(REVIEW_SKILL)
    # decision_gate must appear in the report structure guidance.
    assert re.search(
        r"decision[\s_-]*gate|gate[\s_-]*decision|decision_gate_mode",
        text,
        re.IGNORECASE,
    ), (
        "ralph-preset-review SKILL.md must require a decision_gate field on the report"
    )
    # scope delta / author-vs-reviewer delta must be recorded.
    assert re.search(
        r"scope[\s_-]*(delta|gap|差异)|author.{0,40}reviewer.{0,40}(delta|diff|差异)",
        text,
        re.IGNORECASE | re.DOTALL,
    ), (
        "ralph-preset-review SKILL.md must require an author/reviewer scope delta"
    )
    # critical counts are recorded in the report.
    for critical in ("Critical Ambiguities", "Critical Unverified Assumptions"):
        assert critical in text, (
            f"ralph-preset-review SKILL.md must reference {critical} in the report contract"
        )


def test_review_skill_hard_mode_blocks_on_critical() -> None:
    """Reviewer hard-gate must block on non-zero Critical counts."""
    text = _read(REVIEW_SKILL)
    assert re.search(
        r"(hard|启用\s*硬门禁).{0,200}(Critical|critical).{0,200}(block|阻塞)",
        text,
        re.IGNORECASE | re.DOTALL,
    ), (
        "ralph-preset-review SKILL.md must state that hard mode blocks on Critical counts"
    )


def test_review_skill_record_mode_does_not_reclassify_p0() -> None:
    """Reviewer record mode must not downgrade existing P0/P1 findings."""
    text = _read(REVIEW_SKILL)
    assert re.search(
        r"(record|仅\s*记录).{0,200}(P0|P1|既有|existing|prior)",
        text,
        re.IGNORECASE | re.DOTALL,
    ), (
        "ralph-preset-review SKILL.md must clarify that record mode does not "
        "downgrade existing P0/P1 findings"
    )


def test_review_skill_off_mode_keeps_existing_audit() -> None:
    """Reviewer off mode must keep existing AAF / Payload / lint audit intact."""
    text = _read(REVIEW_SKILL)
    assert re.search(
        r"(off|不\s*启用).{0,200}(保留|keep|preserve|不\s*改变|既\s*有|existing)",
        text,
        re.IGNORECASE | re.DOTALL,
    ), (
        "ralph-preset-review SKILL.md must declare off mode keeps existing audit"
    )


def test_review_skill_no_preset_name_prefix_gate() -> None:
    """Reviewer key-hat identification must not be gated by preset-name prefix."""
    text = _read(REVIEW_SKILL)
    lines = text.splitlines()
    for idx, line in enumerate(lines):
        if not any(
            marker in line
            for marker in (
                "terminal authority",
                "production mutation",
                "phase branching",
                "multi-hat aggregation",
                "artifact producer",
                "key handoff",
                "终态 authority",
                "关键 handoff",
                "关键 artifact",
                "production code",
                "scope",
                "key hat",
                "关键 hat",
            )
        ):
            continue
        if not re.search(
            r"(?:preset name|preset_name).{0,40}(starts? with|begins? with|prefix)|"
            r"名称以.{0,40}开头",
            line,
            re.IGNORECASE,
        ):
            continue
        if re.search(
            r"禁止|forbid|not allowed|do not|不得|不允许|hard rule|硬约束|capability",
            line,
            re.IGNORECASE,
        ):
            continue
        pytest.fail(
            f"ralph-preset-review SKILL.md key-hat scope line encodes a "
            f"preset-name gate:\n  {line}"
        )


def test_review_metadata_mentions_independent_scope() -> None:
    """Reviewer default prompt must advertise independent key-hat scope to implicit callers."""
    text = _read(REVIEW_METADATA)
    assert re.search(
        r"key[\s_-]*hat|关键\s*hat|关键\s*关键",
        text,
        re.IGNORECASE,
    ), (
        "ralph-preset-review agents/openai.yaml must reference the key-hat "
        "scope-first workflow"
    )
    assert re.search(
        r"independent|独立\s*复核|independence",
        text,
        re.IGNORECASE,
    ), (
        "ralph-preset-review metadata must reference independent scope"
    )


# ---------------------------------------------------------------------------
# U3 (plan 2026-08-02-002): cross-skill metadata + structural regression
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("skill_path", [AUTHOR_SKILL, REVIEW_SKILL])
def test_both_skills_use_six_metric_vocabulary(skill_path: Path) -> None:
    """Both skills must reference the six metric names verbatim.

    Lock cross-skill parity: same English metric anchors appear on both
    sides so author notes and review report stay reconcilable.
    """
    text = _read(skill_path)
    for metric in KEY_HAT_METRICS:
        assert metric in text, (
            f"{skill_path.name} must reference metric '{metric}'"
        )


@pytest.mark.parametrize("skill_path", [AUTHOR_SKILL, REVIEW_SKILL])
def test_both_skills_list_three_gate_modes(skill_path: Path) -> None:
    """Both skills must enumerate the three gate modes: hard / record / off."""
    text = _read(skill_path)
    for mode in KEY_HAT_GATE_MODES:
        assert mode in text or mode.replace("-", "_") in text, (
            f"{skill_path.name} must reference gate mode '{mode}'"
        )


@pytest.mark.parametrize("skill_path", [AUTHOR_SKILL, REVIEW_SKILL])
def test_both_skills_reference_both_critical_structured_checks(skill_path: Path) -> None:
    """Both skills must mention both Critical structured checks."""
    text = _read(skill_path)
    for critical in ("Critical Ambiguities", "Critical Unverified Assumptions"):
        assert critical in text, (
            f"{skill_path.name} must reference {critical}"
        )


def test_author_has_initial_scope_reviewer_has_independent_scope() -> None:
    """Author side declares its preliminary scope; Reviewer side declares independent scope."""
    author_text = _read(AUTHOR_SKILL)
    review_text = _read(REVIEW_SKILL)
    assert re.search(
        r"preliminary[\s_-]*scope|初评|初步\s*scope|author[\s_-]*scope",
        author_text,
        re.IGNORECASE,
    ), "Author SKILL.md must declare a preliminary scope concept"
    assert re.search(
        r"independent[\s_-]*scope|独立\s*scope|重新\s*识别|独立\s*重新",
        review_text,
        re.IGNORECASE,
    ), "Reviewer SKILL.md must declare independent scope (re-derived)"


def test_review_records_author_reviewer_scope_delta() -> None:
    """Review report must record the author/reviewer scope delta."""
    text = _read(REVIEW_SKILL)
    assert re.search(
        r"author.{0,40}reviewer.{0,40}(delta|diff|差异|gap)|scope[\s_-]*(delta|gap|差异)",
        text,
        re.IGNORECASE | re.DOTALL,
    ), "Review SKILL.md must record author/reviewer scope delta"


def test_metadata_prompts_keep_original_tasks() -> None:
    """Adding scope prompts must not delete the original AAF/Payload/Handoff task language."""
    author_text = _read(AUTHOR_METADATA).lower()
    review_text = _read(REVIEW_METADATA).lower()
    # Original Author: must still reference AAF 五问 and Payload Contract
    assert "aaf" in author_text, "Author metadata must retain AAF 五问 reference"
    assert "payload contract" in author_text, (
        "Author metadata must retain Payload Contract reference"
    )
    # Original Reviewer: must still reference AAF, payload audit / per-hat AAF, and handoff audit
    for token in ("aaf", "payload", "handoff"):
        assert token in review_text, (
            f"Reviewer metadata must retain {token} reference"
        )


def test_new_key_hat_rules_do_not_introduce_name_prefix_gate_anywhere() -> None:
    """Across both skills + both metadata files, key-hat rules must not encode preset-name gates.

    The pre-existing CE pipeline 3b check (grandfathered) is the only allowed
    preset-name gate in scope; it lives in Workflow 3b and explicitly
    references ``ce-executor-pipeline*``.  We allow that one line and any
    line that forbids the pattern in negation context.
    """
    files = [AUTHOR_SKILL, REVIEW_SKILL, AUTHOR_METADATA, REVIEW_METADATA]
    pattern = re.compile(
        r"(?:preset name|preset_name).{0,40}(starts? with|begins? with|prefix)|"
        r"名称以.{0,40}开头",
        re.IGNORECASE,
    )
    for path in files:
        text = path.read_text(encoding="utf-8")
        for idx, line in enumerate(text.splitlines()):
            if not pattern.search(line):
                continue
            if re.search(
                r"禁止|forbid|not allowed|do not|不得|不允许|hard rule|硬约束|capability|不得\s*名",
                line,
                re.IGNORECASE,
            ):
                continue
            # Grandfather the existing 3b CE pipeline exemption.
            if "3b" in line or "ce-executor-pipeline" in line or "其他 preset 不强制" in line:
                continue
            pytest.fail(
                f"{path}:{idx + 1} introduces an active preset-name gate:\n  {line}"
            )


def test_existing_execution_model_contract_suite_intact() -> None:
    """Regression: the previously collected execution-model contract tests still collect and pass."""
    # Reuse a few stable anchors already proven in U1/U2 above; this test
    # # is a structural smoke that asserts key existing contract tests are
    # # still in the collection.
    import importlib

    mod = importlib.import_module("skills.tests.test_execution_model_contract")
    names = {n for n in dir(mod) if n.startswith("test_")}
    expected = {
        "test_author_skill_asks_execution_model",
        "test_author_deny_locks_single_chain",
        "test_prereview_gate_references_model_branches",
        "test_review_skill_capability_gates",
        "test_review_skill_preserves_ce_pipeline_3b",
        "test_agent_native_model_defines_execution_models",
        "test_rubric_has_wave_capability_audit",
        "test_rubric_has_supervisor_capability_audit",
    }
    missing = expected - names
    assert not missing, (
        f"existing execution-model contract tests missing from module: {sorted(missing)}"
    )


def test_author_notes_gate_scope_table_field_contract() -> None:
    """Author SKILL.md must lock the Gate Scope table columns required for review alignment."""
    text = _read(AUTHOR_SKILL)
    expected_columns = (
        "Hat",
        "Trigger reason",
        "Applicable metrics",
        "Evidence",
        "Unverified assumptions",
        "Critical ambiguities",
        "Critical unverified assumptions",
        "Mode",
        "Decision",
    )
    for col in expected_columns:
        assert col in text or col.lower() in text.lower(), (
            f"Author SKILL.md Gate Scope table must include column '{col}'"
        )
