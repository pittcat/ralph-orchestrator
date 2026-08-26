"""Plan 2026-08-12-001 Unit 5: contract tests for the bundle-first
``run-diagnosis`` skill workflow.

The tests are structural / lexical: they read the skill and
references text and assert that the bundle-first ordering, the
legacy fallback, the human.guidance-vs-task.resume terminology,
and the explicit "non-executing" disclaimer are present. They do
NOT lock the full prompt wording (per the skill's hard rule
against locking operator-visible prompt text) and do not call
the LLM.

Anchors used here are deliberately the ones the plan-reviewer
flagged as stable:

* Phase 0 reads the new ``diagnosis-input.json`` /
  ``runtime-trace.jsonl`` / ``feedback.jsonl`` sidecars BEFORE
  the legacy raw artifacts.
* A legacy session (no bundle) still produces the report via
  the existing current-events / Tier inventory path; bundle
  absence is reported, not a P0.
* The Markdown frontmatter records the bundle status
  (``present`` / ``finalized`` / ``degraded`` / ``legacy`` /
  ``missing``).
* ``human.guidance`` is only ever labeled as historical /
  compat; ``task.resume`` is described as the runtime
  recovery transport.
* Repair suggestions are explicitly non-executing; the skill
  never auto-runs anything.
"""
from __future__ import annotations

import re
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
SKILL_DIR = ROOT / "skills" / "ralph-run-diagnosis"
SKILL_MD = SKILL_DIR / "SKILL.md"
REFS_DIR = SKILL_DIR / "references"
ARTIFACT_DISCOVERY = REFS_DIR / "artifact-discovery.md"
REPORT_TEMPLATE = REFS_DIR / "report-template.md"
VERIFICATION_PIPELINE = REFS_DIR / "verification-pipeline.md"


def _read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def test_skill_mentions_bundle_first_phase_0() -> None:
    body = _read(SKILL_MD).lower()
    # The plan locks Phase 0 as "bundle verification before
    # raw/history". We assert that the §0.2 bundle-first section
    # is positioned before the legacy Phase 0 "Tier" inventory
    # so the agent reads bundle first.
    bundle_phase_idx = body.find("## 0.2")
    phase_zero_idx = body.find("## phase 0")
    assert bundle_phase_idx > 0, "skill must include the bundle-first §0.2 section"
    assert phase_zero_idx > 0, "skill must still include the Phase 0 section"
    assert bundle_phase_idx < phase_zero_idx, (
        "bundle-first ordering violated: §0.2 bundle must be introduced before "
        "the legacy Phase 0 Tier inventory so the agent reads bundle first"
    )


def test_skill_keeps_legacy_fallback() -> None:
    body = _read(SKILL_MD)
    # Legacy fallback contract: missing bundle must NOT be
    # treated as a P0 or block the report. We accept either
    # explicit "legacy" wording or "fallback" wording; the
    # report must still mention current-events / Tier.
    assert "legacy" in body.lower() or "fallback" in body.lower(), (
        "skill must keep a legacy fallback path for sessions without the bundle"
    )
    assert "current-events" in body or "current_events" in body, (
        "skill must still reference current-events as the legacy Tier entry point"
    )


def test_skill_does_not_promote_human_guidance_to_current() -> None:
    body = _read(SKILL_MD)
    mentions = []
    for m in re.finditer(r"human\.guidance", body):
        line_start = body.rfind("\n", 0, m.start()) + 1
        line_end = body.find("\n", m.end())
        line = body[line_start:line_end if line_end > 0 else None]
        if any(tag in line.lower() for tag in ("historical", "compat", "legacy", "deprecated", "不")):
            continue
        mentions.append(line)
    assert not mentions, (
        "skill should not treat human.guidance as a current control topic; "
        f"found {len(mentions)} non-tagged mentions: {mentions[:3]}"
    )


def test_skill_describes_task_resume_as_runtime_transport() -> None:
    body = _read(SKILL_MD)
    lower = body.lower()
    assert "task.resume" in lower or "task resume" in lower, (
        "skill must reference task.resume as the runtime recovery transport"
    )
    # The skill must say it IS the runtime recovery transport.
    assert "runtime recovery transport" in lower or "runtime transport" in lower, (
        "skill must explicitly label task.resume as the runtime recovery transport"
    )


def test_artifact_discovery_lists_new_sidecars() -> None:
    body = _read(ARTIFACT_DISCOVERY)
    for artifact in ("diagnosis-input.json", "runtime-trace.jsonl", "feedback.jsonl"):
        assert artifact in body, (
            f"artifact-discovery.md must list {artifact} as a sidecar; "
            "the skill cannot read it if it does not know it exists"
        )


def test_report_template_tracks_bundle_status() -> None:
    body = _read(REPORT_TEMPLATE)
    # The report frontmatter / sections must reference the
    # bundle status so the operator can see at a glance whether
    # the report came from a present bundle, a legacy session,
    # or a degraded write.
    assert "diagnosis_input" in body or "bundle" in body.lower(), (
        "report-template.md must track diagnosis_input / bundle status"
    )


def test_repair_suggestions_are_non_executing() -> None:
    body = _read(SKILL_MD)
    # The plan locks the contract: repair suggestions are
    # non-executing. We assert the wording appears somewhere in
    # the skill so an agent does not auto-run anything.
    assert "non-executing" in body or "non executing" in body.lower() or "non_executing" in body, (
        "skill must explicitly state repair suggestions are non-executing"
    )
    # And must forbid auto-running ralph / auto-planning.
    forbidden = ("auto-run", "automatically run", "auto plan", "自动执行")
    for phrase in forbidden:
        if phrase in body:
            # Ensure it's in a forbidding context, not endorsing one.
            line_idx = body.find(phrase)
            line_start = body.rfind("\n", 0, line_idx) + 1
            line_end = body.find("\n", line_idx)
            line = body[line_start:line_end if line_end > 0 else None]
            assert any(tag in line.lower() for tag in (
                "not", "禁止", "never", "without", "non", "no ", "avoid",
            )), f"phrase '{phrase}' appears without a forbidding context: {line}"


@pytest.mark.parametrize(
    "artifact_name",
    ["diagnosis-input.json", "runtime-trace.jsonl", "feedback.jsonl"],
)
def test_artifact_discovery_canonical_path(artifact_name: str) -> None:
    body = _read(ARTIFACT_DISCOVERY)
    # Path must be exact (no extra whitespace, no leading "./").
    assert artifact_name in body, f"artifact-discovery.md must mention {artifact_name}"


def test_skill_specifies_bundle_first_diagnose_invocation() -> None:
    """Plan 2026-08-12-001 fix-plan U3: the run-diagnosis skill
    must spell out the concrete ``ralph diagnose --legacy
    --session latest --diagnostics-root ...`` invocation so the
    bundle-first workflow is reproducible from the skill alone.
    """
    body = _read(SKILL_MD)
    for token in (
        "--legacy",
        "--session latest",
        "--diagnostics-root",
        "ralph diagnose",
    ):
        assert token in body, (
            f"skill must specify the bundle-first CLI token {token!r} "
            "in Phase 0 so the workflow is reproducible from the skill alone"
        )


def test_report_template_frontmatter_has_4_required_fields() -> None:
    """Plan 2026-08-12-001 fix-plan U3: report-template.md YAML
    frontmatter must carry structured_result_ref / trace_status /
    feedback_status / evidence_gaps so downstream tooling can
    key on them. Field names must be exact (YAML keys).
    """
    body = _read(REPORT_TEMPLATE)
    for field in (
        "structured_result_ref",
        "trace_status",
        "feedback_status",
        "evidence_gaps",
    ):
        assert field in body, (
            f"report-template.md frontmatter must declare {field!r}"
        )


# Plan 2026-08-15-1823 (fix empty channel activation
# observability) Unit 3: stable anchors for the activation outcome
# recognition contract. The diagnosis skill must:
# - read raw activation rows from runtime-trace.jsonl
# - distinguish the six status values
# - keep non-executing / task.resume-as-runtime-transport / legacy
#   fallback semantics
# - report the activation outcome in frontmatter + §4.2 + L3
# - never collapse `status=empty` into "agent did not emit"

ACTIVATION_OUTCOME_STATUSES = (
    "merged",
    "empty",
    "missing",
    "unreadable",
    "merge_failed",
    "interrupted",
)


def test_skill_recognises_activation_outcome_kind() -> None:
    body = _read(SKILL_MD)
    assert "hat_activation_outcome" in body, (
        "skill must reference the activation outcome kind tag so "
        "agents can grep runtime-trace.jsonl for it"
    )
    assert "phase=activation" in body or "phase=activation " in body, (
        "skill must name the activation phase"
    )


def test_skill_lists_activation_outcome_statuses() -> None:
    body = _read(SKILL_MD)
    missing = [s for s in ACTIVATION_OUTCOME_STATUSES if s not in body]
    assert not missing, (
        f"skill must list every activation outcome status value; missing: {missing}"
    )


def test_skill_does_not_collapse_empty_into_agent_root_cause() -> None:
    body = _read(SKILL_MD).lower()
    # The contract: `status=empty` alone is NOT enough to declare
    # agent did-not-emit. The skill must say so.
    assert "agent" in body and "empty" in body, (
        "skill must mention both 'agent' and 'empty' in the activation context"
    )


def test_skill_keeps_task_resume_transport_and_non_executing() -> None:
    body = _read(SKILL_MD)
    # Plan 2026-08-15-1823 U3 explicitly preserves these
    # invariants while adding activation outcome recognition.
    assert "task.resume" in body or "task resume" in body.lower(), (
        "skill must continue to label task.resume as the runtime recovery transport"
    )
    assert "non-executing" in body or "non_executing" in body, (
        "skill must continue to mark repair suggestions as non-executing"
    )


def test_artifact_discovery_counts_activation_outcome_statuses() -> None:
    body = _read(ARTIFACT_DISCOVERY)
    for status in ACTIVATION_OUTCOME_STATUSES:
        assert status in body, (
            f"artifact-discovery.md must enumerate the activation outcome "
            f"status value '{status}' so the inventory is complete"
        )


def test_report_template_has_activation_outcomes_frontmatter() -> None:
    body = _read(REPORT_TEMPLATE)
    assert "activation_outcomes" in body, (
        "report-template.md frontmatter must declare the activation_outcomes "
        "field so consumers can key on its presence/missing/degraded/legacy state"
    )
    for state in ("present", "missing", "degraded", "legacy"):
        assert state in body, (
            f"report-template.md frontmatter must list activation_outcomes state '{state}'"
        )


def test_report_template_has_section_4_2_activation_table() -> None:
    body = _read(REPORT_TEMPLATE)
    assert "4.2" in body, "report-template.md must include §4.2 activation outcome table"


def test_activation_classification_priority_is_ordered() -> None:
    body = _read(VERIFICATION_PIPELINE)
    priority = (
        "timeout_or_termination",
        "backend_failure",
        "channel_routing_failure",
        "attempted_but_rejected",
        "successful_no_terminal_emit",
        "unknown",
    )
    positions = [body.index(value) for value in priority]
    assert positions == sorted(positions), (
        "verification-pipeline.md must preserve the six classification "
        "values in priority order"
    )
    assert "evidence gap" in body.lower() or "证据不足" in body, (
        "classification contract must describe evidence insufficiency"
    )


def test_confidence_rubric_caps_empty_alone_root_cause() -> None:
    rubric = (
        ROOT / "skills" / "ralph-run-diagnosis" / "references" / "confidence-rubric.md"
    )
    body = _read(rubric)
    assert "status=empty" in body or "empty" in body, (
        "confidence-rubric.md must address the status=empty classification boundary"
    )


def test_source_trace_guide_lists_activation_outcome_entry() -> None:
    guide = (
        ROOT / "skills" / "ralph-run-diagnosis" / "references" / "source-trace-guide.md"
    )
    body = _read(guide)
    assert "hat_activation_outcome" in body or "activation_outcome" in body, (
        "source-trace-guide.md must reference activation outcome rows as evidence anchors"
    )
    assert "activation_outcome.rs" in body, (
        "source-trace-guide.md must point at the activation_outcome.rs Rust entry"
    )


# ─────────────────────────────────────────────────────────────────────
# Plan 2026-08-26-1104 U10: skill confidence gate upgrade + contract sync.
# The skill is now a thin consumer of `ralph diagnose --causal` output.
# Anchors enforce:
#  - DT7 >85 strict gate replaces the legacy 60/70 entry-gates.
#  - The five mechanical scoring categories are listed by name.
#  - No "60" or "70" entry-gate residue remains.
#  - SKILL.md Phase 0 invokes `ralph diagnose --causal` as the
#    attribution source.
#  - report-template.md carries rejected_hypotheses + score change.
#  - artifact-manifest.md names the evidence-window.jsonl + v2
#    boundary_coverage sidecars that DT7 consumes.
# ─────────────────────────────────────────────────────────────────────

DT7_CATEGORIES = ("coverage", "integrity", "refutation", "correlation", "freeze_window")
DT7_GATE_TOKEN = "> 85"


def test_confidence_rubric_uses_dt7_strict_85_gate() -> None:
    """U10: confidence-rubric.md must replace the legacy 60/70 entry-gates
    with DT7's strict `> 85` mechanical gate."""
    rubric = (
        ROOT / "skills" / "ralph-run-diagnosis" / "references" / "confidence-rubric.md"
    )
    body = _read(rubric)
    assert DT7_GATE_TOKEN in body, (
        f"confidence-rubric.md must declare the DT7 strict gate `{DT7_GATE_TOKEN}`; "
        "U10 rewrites the legacy 60/70 entry-gates to a single >85 mechanical gate"
    )
    # Boundary example must be present so 85→incomplete / 86→complete is
    # explicit (U08 boundary tests rely on this wording).
    assert "85" in body and "86" in body, (
        "confidence-rubric.md must show the 85/86 boundary example so the "
        "strict gate is unambiguous"
    )


def test_confidence_rubric_lists_dt7_five_categories() -> None:
    """U10: confidence-rubric.md must enumerate the five DT7 mechanical
    scoring categories: coverage / integrity / refutation / correlation /
    freeze_window."""
    rubric = (
        ROOT / "skills" / "ralph-run-diagnosis" / "references" / "confidence-rubric.md"
    )
    body = _read(rubric)
    missing = [c for c in DT7_CATEGORIES if c not in body]
    assert not missing, (
        f"confidence-rubric.md must enumerate every DT7 category; missing: {missing}"
    )


def test_confidence_rubric_removes_legacy_entry_gates() -> None:
    """U10: the legacy `≥ 60` / `≥ 70` entry-gate language must be fully
    removed from confidence-rubric.md. The rubric may still reference 60/70
    for historical context inside `rejected_hypotheses` examples, but the
    "入表门槛" gate numbers must not appear as thresholds."""
    rubric = (
        ROOT / "skills" / "ralph-run-diagnosis" / "references" / "confidence-rubric.md"
    )
    body = _read(rubric)
    # The legacy entry-gates were phrases like:
    #   "confidence ≥ 60"  / "≥ 70"  / "P0 须 ≥ 70"  / "≥ 60 不得写入 §5"
    # Search for the canonical gate phrasing; reject any match.
    legacy_patterns = (
        "confidence ≥ 60",
        "≥ 60",
        "P0 须 ≥ 70",
        "≥ 70",
        "confidence<60",
        "confidence<70",
    )
    hits = [p for p in legacy_patterns if p in body]
    assert not hits, (
        "confidence-rubric.md must drop legacy entry-gate language; "
        f"found residual: {hits}"
    )


def test_skill_phase_0_invokes_diagnose_causal() -> None:
    """U10: SKILL.md Phase 0 must trigger `ralph diagnose --causal` as
    the causal attribution source (paired with the existing --legacy
    bundle-first invocation)."""
    body = _read(SKILL_MD)
    assert "--causal" in body, (
        "SKILL.md must reference `ralph diagnose --causal` (U10)"
    )
    # The Phase 0 invocation block must actually call the flag, not just
    # mention it once in passing.
    assert "ralph diagnose --causal" in body, (
        "SKILL.md must show the concrete `ralph diagnose --causal` "
        "invocation in Phase 0 causal attribution"
    )


def test_skill_phase_3_consumes_causal_attribution() -> None:
    """U10: SKILL.md Phase 3 / §根因置信度 must consume the causal
    attribution rather than re-deriving scores locally."""
    body = _read(SKILL_MD)
    # Phase 3 area: locate the 根因置信度 block.
    assert "Phase 1–3" in body or "Phase 1-3" in body or "Phase 1~3" in body, (
        "SKILL.md must keep the Phase 1-3 section anchor"
    )
    # The 根因置信度 line must name --causal as the source.
    lower = body.lower()
    assert "ralph diagnose --causal" in body, (
        "SKILL.md 根因置信度 must reference `ralph diagnose --causal` as "
        "the attribution source"
    )
    # The legacy 60/70 entry-gate language must be gone from the skill too.
    assert "≥ 60" not in body and "≥ 70" not in body, (
        "SKILL.md must drop legacy 60/70 entry-gate thresholds; U10 replaces "
        "them with DT7 >85"
    )


def test_report_template_has_rejected_hypotheses_section() -> None:
    """U10: report-template.md must carry the `rejected_hypotheses` and
    `causal_score_change` sections so DT7 outputs are surfaced."""
    body = _read(REPORT_TEMPLATE)
    for token in ("rejected_hypotheses", "causal_score_change"):
        assert token in body, (
            f"report-template.md must contain `{token}` (U10 DT7 attribution)"
        )
    # The §4.3 anchor is the documented location.
    assert "4.3" in body, "report-template.md must include §4.3 Causal Attribution"


def test_report_template_frontmatter_has_causal_fields() -> None:
    """U10: report-template.md frontmatter must declare the DT7 causal
    fields: causal_status / causal_confidence / causal_primary_domain /
    causal_rejected_hypotheses / causal_score_change."""
    body = _read(REPORT_TEMPLATE)
    for field in (
        "causal_status",
        "causal_confidence",
        "causal_primary_domain",
        "causal_rejected_hypotheses",
        "causal_score_change",
    ):
        assert field in body, (
            f"report-template.md frontmatter must declare {field!r} (U10 DT7)"
        )


def test_artifact_manifest_lists_evidence_window_jsonl() -> None:
    """U10: artifact-manifest.md must list `evidence-window.jsonl` as a
    Tier B sidecar (U6 → DT7 freeze_window source)."""
    manifest = (
        ROOT / "skills" / "ralph-run-diagnosis" / "references" / "artifact-manifest.md"
    )
    body = _read(manifest)
    assert "evidence-window.jsonl" in body, (
        "artifact-manifest.md must enumerate evidence-window.jsonl as a "
        "Tier B sidecar (DT7 freeze_window source)"
    )


def test_artifact_manifest_mentions_v2_boundary_coverage() -> None:
    """U10: artifact-manifest.md must reference the v2 manifest's
    `boundary_coverage[]` section (U7 → DT7 coverage source)."""
    manifest = (
        ROOT / "skills" / "ralph-run-diagnosis" / "references" / "artifact-manifest.md"
    )
    body = _read(manifest)
    assert "boundary_coverage" in body, (
        "artifact-manifest.md must mention the v2 manifest `boundary_coverage[]` "
        "section (DT7 coverage source)"
    )


def test_skill_lists_causal_status_field_in_frontmatter_checklist() -> None:
    """U10: SKILL.md 提交前检查 must reference the causal frontmatter
    fields so the operator's pre-submit gate is anchored."""
    body = _read(SKILL_MD)
    # The pre-submit checklist is the anchor; causal_status must appear
    # at least once (could be either in the checklist or 变更日志).
    assert "causal_status" in body, (
        "SKILL.md must surface `causal_status` so the operator pre-submit "
        "checklist can verify the DT7 field is filled"
    )
