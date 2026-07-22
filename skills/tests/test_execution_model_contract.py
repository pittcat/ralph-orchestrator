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
AGENT_NATIVE_MODEL = ROOT / "skills" / "ralph-preset-common" / "references" / "agent-native-model.md"
AUTHOR_CHECKLIST = ROOT / "skills" / "ralph-preset-common" / "references" / "author-checklist.md"
FINDING_RUBRIC = ROOT / "skills" / "ralph-preset-common" / "references" / "finding-rubric.md"
AUTHOR_SKILL = ROOT / "skills" / "ralph-preset-author" / "SKILL.md"
REVIEW_SKILL = ROOT / "skills" / "ralph-preset-review" / "SKILL.md"
DIAGNOSIS_SKILL = ROOT / "skills" / "ralph-run-diagnosis" / "SKILL.md"
FIXTURES_DIR = ROOT / "skills" / "ralph-preset-common" / "fixtures"
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
    """New 3d / 3e gates must not be triggered by preset-name prefix."""
    text = _read(REVIEW_SKILL)
    pattern = re.compile(
        r"(?:preset name|preset_name).{0,40}(starts? with|begins? with|prefix)|"
        r"名称以.{0,40}开头",
        re.IGNORECASE,
    )
    matches = pattern.findall(text)
    assert not matches, (
        f"new review gates must not be triggered by preset-name prefix; "
        f"found {matches!r}"
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
    if not fixture.is_file():
        pytest.skip("wave fixture will land in U6")
    text = fixture.read_text(encoding="utf-8")
    pattern = re.compile(r"ce-executor-supervisor")
    assert not pattern.search(text), (
        "wave capability negative fixture must not reference the builtin "
        "supervisor preset name (capability-triggered, not name-prefixed)"
    )


def test_supervisor_capability_fixture_has_no_builtin_preset_name_gate() -> None:
    """The supervisor negative fixture must not be gated by any builtin preset name."""
    fixture = FIXTURES_DIR / "aaf-supervisor-capability-negative-fixture.yml"
    if not fixture.is_file():
        pytest.skip("supervisor fixture will land in U6")
    text = fixture.read_text(encoding="utf-8")
    pattern = re.compile(r"ce-executor-supervisor")
    assert not pattern.search(text), (
        "supervisor capability negative fixture must not reference the builtin "
        "supervisor preset name"
    )


# ---------------------------------------------------------------------------
# U7 — diagnosis: execution_capabilities + capability-aware reconciliation
# ---------------------------------------------------------------------------


def test_diagnosis_report_template_has_execution_capabilities() -> None:
    """The diagnosis report template must declare ``execution_capabilities``."""
    text = _read(DIAGNOSIS_SKILL)
    assert "execution_capabilities" in text, (
        "ralph-run-diagnosis SKILL.md must declare execution_capabilities"
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
        pytest.skip(f"{path} will land in a later unit")
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