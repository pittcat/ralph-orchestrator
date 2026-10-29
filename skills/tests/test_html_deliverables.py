"""Plan 2026-10-29-1053: HTML deliverable contract tests.

Structural / lexical tests for the HTML deliverable migration. They
assert that:

* every preset / operator skill that ships a human-readable final
  document binds the shared `ralph-html` guidance explicitly;
* machine handoff contracts (path fields, `ralph:*` meta, embedded
  YAML contract blocks, brief validators) are preserved;
* pending / blocked / missing-evidence states cannot be hidden behind
  a pretty report;
* vendored templates are self-contained (no external resources) and
  keep their required sections + metadata.

These tests do NOT render HTML in a browser — where no browser tool
exists, the tests record that the visual checks (wide/narrow view,
keyboard navigation, collapse widgets, print, console) were not
verified, per the plan's honest-reporting rule.
"""
from __future__ import annotations

import html as html_module
import re
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[2]
PRESETS = ROOT / "presets"
SKILLS = ROOT / "skills"


def _read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


# ---------------------------------------------------------------------------
# Shared anchors
# ---------------------------------------------------------------------------

REQUIRED_SKILL_LOAD = "ralph tools skill load ralph-html"


def test_binding_inventory_exists_and_lists_every_entry() -> None:
    doc = _read(ROOT / "docs" / "guide" / "html-deliverables.md")
    for entry in (
        "autoresearch",
        "ce-executor-pipeline",
        "debug",
        "implementation-review",
        "merge-batch",
        "merge-loop",
        "parallel-forge",
        "post-merge-converge",
        "red-team-attack",
        "wave-review",
        "ralph.reviewer",
        "preset-evaluator",
        "ralph-run-diagnosis",
        "ralph-preset-author",
        "ralph-task-discovery",
        "ralph-preset-review",
    ):
        assert entry in doc, f"html-deliverables inventory missing {entry}"


# ---------------------------------------------------------------------------
# Preset reporters must load shared guidance before writing the final report
# ---------------------------------------------------------------------------

@pytest.mark.parametrize(
    "preset_path,hat_id",
    [
        ("presets/en/ce-executor-pipeline.yml", "reporter"),
        ("presets/en/parallel-forge.yml", "reporter"),
        ("presets/en/merge-batch.yml", "reporter"),
        ("presets/en/post-merge-converge.yml", "reporter"),
        ("presets/en/red-team-attack.yml", "reporter"),
    ],
)
def test_preset_reporter_loads_html_guidance(preset_path: str, hat_id: str) -> None:
    body = _read(ROOT / preset_path)
    hat_block = body.split(f"\n  {hat_id}:", 1)[1]
    assert REQUIRED_SKILL_LOAD in hat_block, (
        f"{preset_path}:{hat_id} must load ralph-html before writing the report"
    )


def test_implementation_review_terminal_artifacts_are_html() -> None:
    body = _read(ROOT / "presets/en/implementation-review.yml")
    for name in (
        "fix-plan.html",
        "scope-blocked.html",
        "dispatch-blocked.html",
        "review-blocked.html",
        "wave-blocked.html",
    ):
        assert name in body, f"implementation-review must write {name}"
    # The terminal artifact_path field_docs must describe the html files.
    assert "fix-plan.html (success / residual / fixes)" in body


def test_red_team_three_deliverables_are_html() -> None:
    body = _read(ROOT / "presets/en/red-team-attack.yml")
    for name in ("REPORT.html", "PLAN.html", "QUESTIONS.html"):
        assert name in body
    assert "REPORT.md`" not in body


def test_merge_loop_registered_not_applicable() -> None:
    # merge-loop emits `merge.handled` with no report path field; it is
    # registered as not-applicable in the inventory rather than carrying
    # a hidden Markdown deliverable.
    doc = _read(ROOT / "docs" / "guide" / "html-deliverables.md")
    row = next(l for l in doc.splitlines() if l.startswith("| `merge-loop`"))
    assert "Not applicable" in row


# ---------------------------------------------------------------------------
# Blocked / missing-evidence honesty
# ---------------------------------------------------------------------------

def test_autoresearch_guidance_load_failure_blocks() -> None:
    body = _read(ROOT / "presets/en/autoresearch.yml")
    assert "autoresearch.html" in body
    assert REQUIRED_SKILL_LOAD in body
    assert "experiment.blocked" in body


def test_debug_guidance_load_failure_blocks() -> None:
    body = _read(ROOT / "presets/en/debug.yml")
    assert "counter.html" in body
    assert REQUIRED_SKILL_LOAD in body
    assert "fix.blocked" in body


def test_implementation_review_blocked_honesty_overrides_load_failure() -> None:
    body = _read(ROOT / "presets/en/implementation-review.yml")
    # When guidance cannot be loaded, the blocked artifact must still be
    # written honestly (minimal self-contained HTML) — never swallowed.
    assert "still be recorded honestly" in body or "blocked state" in body
    assert "self-contained HTML" in body


def test_ce_pipeline_blocked_report_still_html() -> None:
    body = _read(ROOT / "presets/en/ce-executor-pipeline.yml")
    # blocked path writes the same report.html with verdict=blocked.
    assert '"verdict":"blocked"' in body or "verdict\":\"blocked" in body
    assert "report.html" in body


# ---------------------------------------------------------------------------
# Operator skills
# ---------------------------------------------------------------------------

def test_diagnosis_deliverable_is_html_with_ralph_meta() -> None:
    skill = _read(SKILLS / "ralph-run-diagnosis" / "SKILL.md")
    assert "-diagnosis.html" in skill
    assert REQUIRED_SKILL_LOAD in skill
    assert "ralph:causal_status" in skill or "ralph:history_search" in skill

    template = _read(
        SKILLS / "ralph-run-diagnosis" / "references" / "report-template.md"
    )
    # frontmatter-equivalent fields migrate to ralph meta.
    for field in (
        "ralph:history_search",
        "ralph:causal_status",
        "ralph:activation_outcomes",
        "ralph:evidence_gaps",
        "ralph:diagnostics_mode",
    ):
        assert field in template, f"diagnosis template missing meta {field}"
    # Missing evidence must stay visible, not be hidden behind N/A-free
    # presentation: the N/A placeholders are preserved verbatim.
    assert "N/A (causal attribution unavailable)" in template
    assert "N/A (activation outcomes unavailable)" in template


def test_author_notes_html_keeps_machine_contract() -> None:
    skill = _read(SKILLS / "ralph-preset-author" / "SKILL.md")
    assert "preset-author-notes.html" in skill
    assert "contract-yaml" in skill
    # AAF / Payload Contract / Gate Scope sections keep stable ids.
    for section_id in ("gate-scope", "key-stage-event-gate"):
        assert section_id in skill


def test_task_discovery_yaml_brief_remains_handoff() -> None:
    skill = _read(SKILLS / "ralph-task-discovery" / "SKILL.md")
    # The YAML brief is still the ONLY handoff artifact; the HTML view
    # mirrors it and must not hide pending/blocked state.
    assert "只有 brief 路径" in skill or "仅 task brief 路径" in skill
    assert "<brief>.html" in skill
    assert "blocked" in skill


def test_task_discovery_validator_unchanged() -> None:
    # The hard-gate validator still rejects incomplete briefs — the HTML
    # view is a read view, never an author_ready escape.
    validator = (
        SKILLS / "ralph-task-discovery" / "scripts" / "brief_validator.py"
    )
    assert validator.is_file()
    body = _read(validator)
    assert "author_ready" in body and "handoff_block_reasons" in body


def test_review_report_is_html_with_stable_fields() -> None:
    skill = _read(SKILLS / "ralph-preset-review" / "SKILL.md")
    assert "preset-review-report.html" in skill
    assert REQUIRED_SKILL_LOAD in skill
    # Stable field names for the next review run survive in ralph meta.
    for field in (
        "agent_skill_audit",
        "decision_gate",
        "key_stage_event_gate_audit",
    ):
        assert field in skill


def test_review_notes_fixtures_parse_yaml_from_html() -> None:
    """key-stage fixtures: YAML contract lives inside
    `<pre class="contract-yaml"><code>` and must round-trip."""
    fixtures = sorted(
        (SKILLS / "ralph-preset-review" / "fixtures").glob(
            "key-stage-event-gate-*.preset-author-notes.html"
        )
    )
    assert len(fixtures) == 4, f"expected 4 html notes fixtures, got {fixtures}"
    for fixture in fixtures:
        text = _read(fixture)
        assert "<!DOCTYPE html>" in text
        match = re.search(
            r'<pre class="contract-yaml"><code>(.*?)</code>', text, re.DOTALL
        )
        assert match is not None, f"{fixture.name} missing contract-yaml block"
        value = yaml.safe_load(html_module.unescape(match.group(1)))
        assert isinstance(value.get("key_stages"), list), (
            f"{fixture.name} key_stages must parse from the HTML contract block"
        )


# ---------------------------------------------------------------------------
# Templates: self-contained + escaping + required sections
# ---------------------------------------------------------------------------

EXTERNAL_RE = re.compile(r'(src|href)\s*=\s*"https?://', re.IGNORECASE)


@pytest.mark.parametrize(
    "template,required_markers",
    [
        (
            "presets/templates/parallel-forge/manager-report.template.html",
            [
                "<!DOCTYPE html>",
                'meta name="ralph:status"',
                "<h2>6. Scenario 验收结果</h2>",
                "<strong>TDD 执行情况</strong>",
            ],
        ),
        (
            "presets/templates/red-team-attack/report.template.html",
            ["<!DOCTYPE html>", "Red Team Attack Report", "QUESTIONS.html"],
        ),
        (
            "presets/templates/red-team-attack/plan.template.html",
            [
                "<!DOCTYPE html>",
                'data-unit="0"',
                'meta name="ralph:execution_authorized"',
                "REPORT.html",
            ],
        ),
    ],
)
def test_html_templates_self_contained(template: str, required_markers: list[str]) -> None:
    path = ROOT / template
    assert path.is_file(), f"missing {template}"
    body = _read(path)
    assert not EXTERNAL_RE.search(body), (
        f"{template} must not reference external http(s) resources"
    )
    for marker in required_markers:
        assert marker in body, f"{template} missing marker {marker}"


def test_template_escapes_html_special_chars() -> None:
    """Placeholders like <任务名称> must be entity-escaped inside text
    nodes so the file parses as valid HTML."""
    body = _read(ROOT / "presets/templates/parallel-forge/manager-report.template.html")
    assert "&lt;任务名称&gt;" in body, "visible placeholder must be entity-escaped"


def test_removed_markdown_templates_are_gone() -> None:
    for stale in (
        "presets/templates/parallel-forge/manager-report.template.md",
        "presets/templates/red-team-attack/report.template.md",
        "presets/templates/red-team-attack/plan.template.md",
    ):
        assert not (ROOT / stale).exists(), f"stale template {stale} must be removed"


# ---------------------------------------------------------------------------
# Honest browser-verification record (no browser tool in this environment)
# ---------------------------------------------------------------------------

def test_browser_checks_honestly_unverified_when_unavailable() -> None:
    """The plan requires offline-open / wide-narrow / keyboard / collapse /
    print / console checks. With no browser tool in this repo's test env,
    the contract test records that visual verification is NOT performed —
    it must never be claimed silently."""
    import shutil

    browser = next(
        (
            b
            for b in ("chromium", "chromium-browser", "google-chrome", "firefox")
            if shutil.which(b)
        ),
        None,
    )
    if browser is None:
        pytest.skip(
            "no browser available — visual checks (wide/narrow, keyboard, "
            "collapse, print, console) recorded as NOT VERIFIED"
        )
    # If a browser exists, just sanity-parse the templates.
    from html.parser import HTMLParser

    for template in (
        ROOT / "presets/templates/parallel-forge/manager-report.template.html",
        ROOT / "presets/templates/red-team-attack/report.template.html",
        ROOT / "presets/templates/red-team-attack/plan.template.html",
    ):
        HTMLParser().feed(_read(template))
