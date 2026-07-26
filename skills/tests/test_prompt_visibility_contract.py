"""Contract tests for the prompt-visibility procedure introduced by plan
``2026-07-26-001-feat-prompt-visibility-and-agent-skill-audit-plan``.

The plan locks a cross-skill contract:

* ``skills/ralph-preset-common/references/prompt-visibility.md`` is the
  SSOT for the ``ralph inspect prompt`` workflow (auto vs on-demand
  classification, outer-repo fallback).
* ``skills/ralph-preset-author/SKILL.md`` (Workflow step 3, Drafting
  phase) MUST mandate a per-hat ``inspect prompt`` run before
  drafting or editing instructions, and MUST NOT direct operators to
  install-tree ``.claude/skills/<name>``.
* ``skills/ralph-preset-review/SKILL.md`` (Per-hat AAF Visible context)
  MUST bind Visible context to ``ralph inspect prompt`` or the
  shared ``prompt-visibility`` reference.
* ``skills/ralph-run-diagnosis/SKILL.md`` (and ``references/``) MUST
  include a checklist item that uses ``inspect prompt`` for
  reconciliation against skill visibility / Confirm path.
* Workflow gating for agent-skill audits (review SKILL step 0):
  default is "review YAML only" with an opt-in combo for "also
  review injected skills". The default MUST be YAML-only; the opt-in
  MUST be a clearly-described alternative; the report MUST carry an
  ``agent_skill_audit: <state>`` field.
* ``finding-rubric.md`` MUST register at least three ``agent_skill.*``
  finding IDs (leaks_internals / unreadable / inject_claim_false by
  default).
* ``commands.md`` MUST list ``inspect prompt`` for outer-repo and
  builtin-preset operators.

These tests are **structural / lexical** — they read skill files and
assert presence / absence of required headings, fields, and trigger
phrases. They do not run real LLM judges (that remains a reviewer /
audit concern per plan acceptance matrix).
"""
from __future__ import annotations

from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
COMMON_REFS = ROOT / "skills" / "ralph-preset-common" / "references"
PROMPT_VISIBILITY = COMMON_REFS / "prompt-visibility.md"
FINDING_RUBRIC = COMMON_REFS / "finding-rubric.md"
COMMANDS = COMMON_REFS / "commands.md"
AUTHOR_CHECKLIST = COMMON_REFS / "author-checklist.md"
AGENT_SKILL_AUDIT = COMMON_REFS / "agent-skill-audit.md"

AUTHOR_SKILL = ROOT / "skills" / "ralph-preset-author" / "SKILL.md"
REVIEW_SKILL = ROOT / "skills" / "ralph-preset-review" / "SKILL.md"
DIAGNOSIS_SKILL = ROOT / "skills" / "ralph-run-diagnosis" / "SKILL.md"
DIAGNOSIS_REFS = ROOT / "skills" / "ralph-run-diagnosis" / "references"

CLAUDE_INSTALL_SKILLS = ROOT / ".claude" / "skills"


def _read(path: Path) -> str:
    assert path.is_file(), f"missing file: {path}"
    return path.read_text(encoding="utf-8")


# ---------------------------------------------------------------------------
# S7 / R5 / R10 — author skill must reference inspect prompt
# ---------------------------------------------------------------------------


def test_prompt_visibility_reference_exists() -> None:
    """The shared procedure file must exist under skills/ralph-preset-common/references/."""
    assert PROMPT_VISIBILITY.is_file(), (
        f"missing shared reference: {PROMPT_VISIBILITY}"
    )
    text = _read(PROMPT_VISIBILITY)
    # Anchor: the procedure must point at `ralph inspect prompt` as the
    # source of truth and forbid claiming on-demand skills are
    # auto-injected.
    assert "ralph inspect prompt" in text
    assert "auto_inject" in text
    assert "on_demand" in text


def test_author_skill_mandates_inspect_prompt_per_hat() -> None:
    """The author skill must require ``inspect prompt`` BEFORE drafting instructions."""
    text = _read(AUTHOR_SKILL)
    assert "inspect prompt" in text, (
        "author SKILL must reference 'inspect prompt' for the per-hat "
        "drafting / editing gate (plan §5.U7)"
    )
    assert "prompt-visibility" in text, (
        "author SKILL must reference the shared prompt-visibility procedure"
    )


def test_author_skill_forbids_install_tree_edits() -> None:
    """Author SKILL must not direct operators to edit .claude/skills/<name>.

    The plan (Product Contract preservation) forbids writing to the
    install tree — the only edit target is skills/<name>/.
    """
    text = _read(AUTHOR_SKILL)
    # Naive substring match is fine here: the SKILL is a human-facing
    # document and any reference to .claude/skills would be a step,
    # not a passing reference. Disallow both forms.
    for forbidden in [
        "编辑 `.claude/skills`",
        "edit .claude/skills",
        "modify .claude/skills",
    ]:
        assert forbidden not in text, (
            f"author SKILL must not direct operators to {forbidden!r}"
        )


def test_author_skill_forbids_on_demand_claim_in_instructions() -> None:
    """Author SKILL must tell the author NOT to claim on-demand skills are auto-injected."""
    text = _read(AUTHOR_SKILL)
    assert "on_demand" in text or "on-demand" in text, (
        "author SKILL must mention the on-demand category so authors "
        "do not accidentally claim on-demand skills are auto-injected"
    )


# ---------------------------------------------------------------------------
# S8 / R6 — review skill binds Visible context to inspect prompt
# ---------------------------------------------------------------------------


def test_review_skill_anchors_visible_context_to_inspect_prompt() -> None:
    """Per-hat AAF Visible context must use inspect prompt / prompt-visibility."""
    text = _read(REVIEW_SKILL)
    assert "inspect prompt" in text or "prompt-visibility" in text, (
        "review SKILL must anchor Per-hat AAF Visible context to "
        "'inspect prompt' or the shared 'prompt-visibility' procedure"
    )


# ---------------------------------------------------------------------------
# S9 / R7 — diagnose skill reconciliation hook
# ---------------------------------------------------------------------------


def test_diagnosis_skill_references_inspect_prompt() -> None:
    """diagnosis skill must include 'inspect prompt' as a reconciliation step."""
    text = _read(DIAGNOSIS_SKILL)
    assert "inspect prompt" in text, (
        "diagnosis SKILL must include an 'inspect prompt' step or "
        "checklist item for skill-visibility / Confirm reconciliation"
    )


def test_diagnosis_references_directory_under_skills() -> None:
    """If diagnosis adds references, they must live under skills/ralph-run-diagnosis/."""
    if DIAGNOSIS_REFS.is_dir():
        for entry in DIAGNOSIS_REFS.iterdir():
            # The references/ directory is owned by the diagnose skill;
            # there must be no path-only redirect to a sibling common.
            assert entry.name != "ralph-preset-common", (
                "diagnosis references/ must not redirect into the shared "
                "ralph-preset-common/ directory (would silently drift "
                "between author/review and diagnose)"
            )


# ---------------------------------------------------------------------------
# S10 / R8 — review combo gate: default skipped, opt-in to audit
# ---------------------------------------------------------------------------


def test_review_skill_default_skips_agent_skill_audit() -> None:
    """The review skill must default to 'review YAML only' for agent-skill audit."""
    text = _read(REVIEW_SKILL)
    # Recommended (default) option MUST be YAML-only.
    assert "仅审查 preset YAML" in text or "仅审查 YAML" in text, (
        "review SKILL must list 'only review preset YAML' as the "
        "recommended default for the agent-skill audit gate"
    )


def test_review_skill_offers_opt_in_alternative() -> None:
    """The opt-in alternative 'also review injected skills' MUST exist."""
    text = _read(REVIEW_SKILL)
    assert "同时审查注入 skill" in text or "同时审查 skill" in text, (
        "review SKILL must list the 'also review injected skills' "
        "option as a clear alternative"
    )


def test_review_skill_records_agent_skill_audit_state() -> None:
    """The review SKILL must require recording agent_skill_audit: <state>."""
    text = _read(REVIEW_SKILL)
    assert "agent_skill_audit" in text, (
        "review SKILL must require the report to carry an "
        "'agent_skill_audit: skipped|performed' field"
    )


def test_review_skill_default_does_not_audit_data() -> None:
    """The default flow must NOT mention auditing data/*.md."""
    text = _read(REVIEW_SKILL)
    # The default path is YAML-only; if the file describes 'by default
    # we audit data/*.md', that is a regression. We assert the
    # negative explicitly: at least one place in the SKILL must say
    # '默认不审 data' (or equivalent) so a future edit cannot silently
    # invert the default.
    assert (
        "默认不审" in text
        or "默认跳过" in text
        or "skip the data" in text.lower()
        or "default to yaml-only" in text.lower()
    ), (
        "review SKILL must explicitly say the default does NOT audit "
        "data/*.md (plan §5.U9 + KTD-4)"
    )


# ---------------------------------------------------------------------------
# S11 / S12 / R9 — agent-skill audit procedure + finding_id family
# ---------------------------------------------------------------------------


def test_agent_skill_audit_procedure_exists() -> None:
    """The shared agent-skill-audit reference must exist."""
    assert AGENT_SKILL_AUDIT.is_file(), (
        f"missing shared reference: {AGENT_SKILL_AUDIT}"
    )


def test_finding_rubric_lists_agent_skill_finding_ids() -> None:
    """``finding-rubric.md`` must register at least three ``agent_skill.*`` IDs."""
    text = _read(FINDING_RUBRIC)
    for fid in [
        "agent_skill.leaks_internals",
        "agent_skill.unreadable",
        "agent_skill.inject_claim_false",
    ]:
        assert fid in text, (
            f"finding-rubric.md must register {fid!r} (plan §5.U10)"
        )


def test_finding_rubric_audit_section_present() -> None:
    """A dedicated Agent skill audit section must exist in finding-rubric.md."""
    text = _read(FINDING_RUBRIC)
    assert "Agent skill audit" in text or "agent_skill" in text, (
        "finding-rubric.md must contain an Agent skill audit section"
    )


def test_agent_skill_audit_documents_outer_repo_source() -> None:
    """The audit procedure must call out the outer-repo (binary-embedded) source."""
    text = _read(AGENT_SKILL_AUDIT)
    assert "二进制内嵌" in text or "binary-embedded" in text or "binary embed" in text.lower(), (
        "agent-skill-audit.md must call out the outer-repo audit "
        "source as 'binary-embedded' so reports do not confuse "
        "operators about where the audited content comes from"
    )


# ---------------------------------------------------------------------------
# U12 — commands.md + author-checklist must register the new command
# ---------------------------------------------------------------------------


def test_commands_md_lists_inspect_prompt() -> None:
    """``commands.md`` must surface ``inspect prompt`` for operators."""
    text = _read(COMMANDS)
    assert "inspect prompt" in text, (
        "commands.md must list 'inspect prompt' so operators can "
        "discover the prompt-visibility command"
    )


def test_author_checklist_references_prompt_visibility() -> None:
    """author-checklist.md must cite the prompt-visibility evidence rule."""
    text = _read(AUTHOR_CHECKLIST)
    assert (
        "prompt-visibility" in text or "inspect prompt" in text
    ), (
        "author-checklist.md must cross-reference prompt-visibility "
        "evidence so the checklist is consistent with the Workflow step"
    )


# ---------------------------------------------------------------------------
# Cross-cutting guard: the install tree must not gain a copy of the
# reference / SKILL edits (plan Product Contract preservation).
# ---------------------------------------------------------------------------


def test_install_tree_does_not_contain_prompt_visibility_copy() -> None:
    """``.claude/skills/<name>/`` must NOT contain a prompt-visibility.md copy.

    The plan (Product Contract preservation) forbids editing the
    install tree. If a copy accidentally lands there, the operator
    may diverge from the source.
    """
    if not CLAUDE_INSTALL_SKILLS.is_dir():
        pytest.skip(".claude/skills/ is not present (install tree absent)")
    for path in CLAUDE_INSTALL_SKILLS.rglob("prompt-visibility.md"):
        pytest.fail(
            f"install tree contains a copy at {path}; remove it — the "
            "shared reference lives only at "
            f"{PROMPT_VISIBILITY.relative_to(ROOT)}"
        )