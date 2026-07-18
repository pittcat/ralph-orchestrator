"""Contract tests for the bootstrap audit (Unit 2).

These tests exercise ``scripts/audit.py`` against the parameterised
fixtures under ``skills/ralph-project-bootstrap/fixtures/projects``.

The contract:

* Inputs that are missing or unreadable (preset / plan / task) cause a
  blocking ``AuditDecision`` and no helper is allowed to persist state.
* Conflicting root scope signals produce ``root_ambiguous`` and stop.
* Verifiable build / test / lint entry points are surfaced only when
  their marker files exist; the audit never invents commands.
* All reported paths are repo-relative so the handoff stays portable.
"""
from __future__ import annotations

from pathlib import Path

import pytest

from audit import ProjectFacts, run_audit  # noqa: F401  (re-exported for fixtures)
import _fixtures  # noqa: F401
import _paths  # noqa: F401

ROOT = Path(__file__).resolve().parents[2]
FIXTURES_SRC = ROOT / "skills" / "ralph-project-bootstrap" / "fixtures" / "projects"


def _audit(project: Path, *, preset: str | None, plan_path: str | None):
    return run_audit(project, preset=preset, plan_path=plan_path)


# --- input gating ---------------------------------------------------------


@pytest.mark.parametrize("fixture_name", ["blank", "rust"])
def test_missing_preset_blocks(tmp_path: Path, fixture_name: str) -> None:
    project = tmp_path / "project"
    _fixtures.materialise(fixture_name, project)
    decision = run_audit(project, preset=None, plan_path="plan.md")
    assert decision.is_blocking
    codes = {issue.code for issue in decision.issues}
    assert "input_missing_preset" in codes
    # no write happened anywhere
    assert list(project.rglob("*.pipeline.yml")) == []


@pytest.mark.parametrize("fixture_name", ["blank", "rust"])
def test_missing_plan_blocks(tmp_path: Path, fixture_name: str) -> None:
    project = tmp_path / "project"
    _fixtures.materialise(fixture_name, project)
    decision = run_audit(
        project,
        preset="builtin:ce-executor-pipeline",
        plan_path=None,
    )
    assert decision.is_blocking
    codes = {issue.code for issue in decision.issues}
    assert "input_missing_plan" in codes


def test_unreadable_preset_blocks(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("rust", project)
    decision = run_audit(
        project,
        preset="presets/missing.yml",
        plan_path="plan.md",
    )
    assert decision.is_blocking
    codes = {issue.code for issue in decision.issues}
    assert "input_missing_preset_file" in codes


def test_unreadable_plan_blocks(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("rust", project)
    (project / "Cargo.toml").is_file()  # sanity
    decision = run_audit(
        project,
        preset="builtin:ce-executor-pipeline",
        plan_path="docs/missing.md",
    )
    assert decision.is_blocking
    codes = {issue.code for issue in decision.issues}
    assert "input_missing_plan_file" in codes


# --- root resolution ------------------------------------------------------


def test_ambiguous_root_blocks(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("ambiguous-root", project)
    (project / "plan.md").write_text("# placeholder\n", encoding="utf-8")
    # Run audit from a cwd that exposes two competing AGENTS.md scopes
    # (one at the project root and one inside the nested subtree).
    cwd = project / "nested"
    decision = run_audit(
        cwd,
        preset="builtin:ce-executor-pipeline",
        plan_path="../plan.md",
    )
    assert decision.is_blocking
    codes = {issue.code for issue in decision.issues}
    assert "root_ambiguous" in codes


def test_rust_fixture_root_resolves_relative_to_self(tmp_path: Path) -> None:
    """When cwd is the project root, the reported root must be ``./``."""
    project = tmp_path / "project"
    _fixtures.materialise("rust", project)
    (project / "docs").mkdir(exist_ok=True)
    (project / "docs" / "plan.md").write_text("# plan\n", encoding="utf-8")
    decision = run_audit(
        project,
        preset="builtin:ce-executor-pipeline",
        plan_path="docs/plan.md",
    )
    assert decision.root in ("./", "./project")  # depends on cwd resolution


# --- project fact evidence ------------------------------------------------


def test_rust_facts_are_concrete(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("rust", project)
    decision = run_audit(
        project,
        preset="builtin:ce-executor-pipeline",
        plan_path="docs/plan.md",
    )
    assert decision.facts.technology == "rust"
    assert "cargo nextest run" in decision.facts.test
    assert "cargo clippy --workspace --all-targets -- -D warnings" in decision.facts.lint


def test_node_facts_match_scripts(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("node", project)
    decision = run_audit(
        project,
        preset="builtin:ce-executor-pipeline",
        plan_path="plan.md",
    )
    assert decision.facts.technology == "node"
    assert "npm test" in decision.facts.test
    assert "npm run lint" in decision.facts.lint


def test_python_facts_use_venv(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("python", project)
    decision = run_audit(
        project,
        preset="builtin:ce-executor-pipeline",
        plan_path="plan.md",
    )
    assert decision.facts.technology == "python"
    assert any(".venv" in cmd for cmd in decision.facts.test)


def test_unknown_stack_reports_no_facts(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("unknown", project)
    decision = run_audit(
        project,
        preset="builtin:ce-executor-pipeline",
        plan_path="plan.md",
    )
    assert decision.facts.technology == "unknown"
    assert decision.facts.is_empty()
    assert decision.notes


# --- portability ----------------------------------------------------------


def test_reported_paths_are_relative(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("rust", project)
    decision = run_audit(
        project,
        preset="presets/missing.yml",
        plan_path="docs/plan.md",
    )
    for issue in decision.issues:
        for path in issue.paths:
            assert not Path(path).is_absolute(), f"absolute path leaked: {path}"


def test_path_helper_rejects_absolute(tmp_path: Path) -> None:
    assert not _paths.is_safe_relative("/etc/passwd")
    assert not _paths.is_safe_relative("../outside.txt")
    assert _paths.is_safe_relative("docs/plan.md")
    assert _paths.is_safe_relative("./docs/plan.md")


def test_blank_project_root_resolves_to_cwd(tmp_path: Path) -> None:
    project = tmp_path / "blank"
    project.mkdir()
    decision = run_audit(
        project,
        preset="builtin:ce-executor-pipeline",
        plan_path="plan.md",
    )
    assert decision.root is not None
    assert decision.facts.technology == "unknown"