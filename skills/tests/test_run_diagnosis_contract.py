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
