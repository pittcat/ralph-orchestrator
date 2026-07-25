"""Tests for ``plan_resolve`` — fitness / discover / author / resolve."""

from __future__ import annotations

import subprocess
from datetime import date
from pathlib import Path

import plan_resolve  # type: ignore[import-not-found]


def _git_init(repo: Path) -> Path:
    repo.mkdir(parents=True, exist_ok=True)
    subprocess.run(["git", "init"], cwd=repo, check=True, capture_output=True)
    subprocess.run(
        ["git", "config", "user.email", "test@example.com"],
        cwd=repo,
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "config", "user.name", "test"],
        cwd=repo,
        check=True,
        capture_output=True,
    )
    (repo / ".keep").write_text("", encoding="utf-8")
    subprocess.run(["git", "add", ".keep"], cwd=repo, check=True, capture_output=True)
    subprocess.run(
        ["git", "commit", "-m", "init"],
        cwd=repo,
        check=True,
        capture_output=True,
    )
    return repo.resolve()


def _write_orch_plan(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "# Orch fix\n"
        "\n"
        "### U1. Fix flow\n"
        "\n"
        "Touch `crates/ralph-core/src/event_loop/mod.rs`.\n"
        "\n"
        "### U2. Preset\n"
        "\n"
        "Touch `presets/en/ce-executor-supervisor.yml`.\n"
        "\n"
        "### U3. Drift\n"
        "\n"
        "Touch `crates/ralph-core/src/drift/engine.rs`.\n",
        encoding="utf-8",
    )


def _write_e2e_plan(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "# Multi-sort supervisor E2E\n"
        "\n"
        "### U1. Bubble\n"
        "\n"
        "Touch `sorts/bubble.py`.\n"
        "\n"
        "### U2. Insert\n"
        "\n"
        "Touch `sorts/insertion.py`.\n",
        encoding="utf-8",
    )


def test_assess_fitness_rejects_orch_plan_for_product_sandbox(tmp_path: Path) -> None:
    orch = _git_init(tmp_path / "orch")
    sand = _git_init(tmp_path / "sand")
    plan = orch / "docs" / "plans" / "005.md"
    _write_orch_plan(plan)
    report = plan_resolve.assess_fitness(plan, sand)
    assert report.suitable is False
    assert "orchestrator" in report.reason.lower() or "crates" in report.reason


def test_assess_fitness_accepts_sandbox_e2e_plan(tmp_path: Path) -> None:
    sand = _git_init(tmp_path / "sand")
    plan = sand / "docs" / "plans" / "2026-07-22-001-feat-multi-sort-supervisor-e2e-plan.md"
    _write_e2e_plan(plan)
    report = plan_resolve.assess_fitness(plan, sand)
    assert report.suitable is True


def test_resolve_rejects_unfit_caller_and_discovers_local(tmp_path: Path) -> None:
    orch = _git_init(tmp_path / "orch")
    sand = _git_init(tmp_path / "sand")
    bad = orch / "docs" / "plans" / "005-fix.md"
    _write_orch_plan(bad)
    good = sand / "docs" / "plans" / "2026-07-22-001-feat-multi-sort-supervisor-e2e-plan.md"
    _write_e2e_plan(good)

    result = plan_resolve.resolve_plan(
        sand,
        candidate=bad,
        preset="builtin:ce-executor-supervisor",
    )
    assert result.ok is True
    assert result.source == "discovered"
    assert result.rejected_candidate == str(bad)
    assert Path(result.plan_path).name == good.name


def test_resolve_without_candidate_discovers(tmp_path: Path) -> None:
    sand = _git_init(tmp_path / "sand")
    good = sand / "docs" / "plans" / "2026-07-22-001-feat-multi-sort-supervisor-e2e-plan.md"
    _write_e2e_plan(good)
    result = plan_resolve.resolve_plan(
        sand,
        candidate=None,
        preset="builtin:ce-executor-supervisor",
    )
    assert result.ok is True
    assert result.source == "discovered"
    assert Path(result.plan_path).resolve() == good.resolve()


def test_resolve_authors_when_nothing_suitable(tmp_path: Path) -> None:
    sand = _git_init(tmp_path / "sand")
    # Unsuitable local plan only (orch-style intents, no crates in sandbox).
    bad_local = sand / "docs" / "plans" / "accidental-orch.md"
    _write_orch_plan(bad_local)

    result = plan_resolve.resolve_plan(
        sand,
        candidate=None,
        preset="builtin:ce-executor-supervisor",
        allow_author=True,
    )
    assert result.ok is True
    assert result.source == "authored"
    authored = Path(result.plan_path)
    assert authored.is_file()
    assert "e2e-bootstrap-minimal" in authored.name
    assert "e2e_smoke_marker.txt" in authored.read_text(encoding="utf-8")


def test_author_minimal_plan_is_idempotent(tmp_path: Path) -> None:
    sand = tmp_path / "sand"
    sand.mkdir()
    a = plan_resolve.author_minimal_plan(
        sand, preset="builtin:ce-executor-supervisor", today=date(2026, 7, 25)
    )
    b = plan_resolve.author_minimal_plan(
        sand, preset="builtin:ce-executor-supervisor", today=date(2026, 7, 25)
    )
    assert a == b
    assert a.read_text(encoding="utf-8") == b.read_text(encoding="utf-8")


def test_resolve_accepts_fit_caller(tmp_path: Path) -> None:
    sand = _git_init(tmp_path / "sand")
    plan = sand / "docs" / "plans" / "local-e2e.md"
    _write_e2e_plan(plan)
    result = plan_resolve.resolve_plan(sand, candidate=plan)
    assert result.ok is True
    assert result.source == "caller"
    assert result.rejected_candidate is None
