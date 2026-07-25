"""Tests for dual-plan ``plan_resolve`` (change plan + workload)."""

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


def _write_change_plan(path: Path, *, with_presets: bool = False) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    body = (
        "# Orch fix\n"
        "\n"
        "## Goal Capsule\n"
        "\n"
        "- Objective: fix exec.unit.done flow.\n"
        "\n"
        "### U1. Fix flow\n"
        "\n"
        "Touch `crates/ralph-core/src/event_loop/mod.rs`.\n"
        "\n"
        "### U2. Drift\n"
        "\n"
        "Touch `crates/ralph-core/src/drift/engine.rs`.\n"
    )
    if with_presets:
        body += (
            "\n### U3. Preset\n\n"
            "Touch `presets/en/ce-executor-supervisor.yml`.\n"
        )
    path.write_text(body, encoding="utf-8")


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


def test_change_plan_not_used_as_workload(tmp_path: Path) -> None:
    orch = _git_init(tmp_path / "orch")
    sand = _git_init(tmp_path / "sand")
    change = orch / "docs" / "plans" / "005.md"
    _write_change_plan(change)
    good = sand / "docs" / "plans" / "2026-07-22-001-feat-multi-sort-supervisor-e2e-plan.md"
    _write_e2e_plan(good)

    result = plan_resolve.resolve_plans(
        sand,
        change_plan=change,
        preset="builtin:ce-executor-supervisor",
    )
    assert result.ok is True
    assert result.workload_source == "discovered"
    assert Path(result.workload_plan_path).name == good.name
    assert result.change_plan_path is not None
    assert "005" in result.change_plan_path
    assert result.change_plan_hash
    assert "Objective" in result.change_summary
    assert result.needs_author_confirmation is False


def test_workload_fitness_rejects_orch_as_workload(tmp_path: Path) -> None:
    orch = _git_init(tmp_path / "orch")
    sand = _git_init(tmp_path / "sand")
    plan = orch / "docs" / "plans" / "005.md"
    _write_change_plan(plan)
    report = plan_resolve.assess_workload_fitness(plan, sand)
    assert report.suitable is False


def test_change_plan_touches_presets_flagged_but_workload_ok(tmp_path: Path) -> None:
    orch = _git_init(tmp_path / "orch")
    sand = _git_init(tmp_path / "sand")
    change = orch / "docs" / "plans" / "005.md"
    _write_change_plan(change, with_presets=True)
    good = sand / "docs" / "plans" / "2026-07-22-001-feat-multi-sort-supervisor-e2e-plan.md"
    _write_e2e_plan(good)
    result = plan_resolve.resolve_plans(sand, change_plan=change)
    assert result.ok is True
    assert result.change_plan_touches_presets is True
    assert Path(result.workload_plan_path).name == good.name


def test_no_workload_asks_confirmation_no_silent_author(tmp_path: Path) -> None:
    sand = _git_init(tmp_path / "sand")
    # Only unfit local plan.
    bad = sand / "docs" / "plans" / "accidental-orch.md"
    _write_change_plan(bad)
    before = list((sand / "docs" / "plans").glob("*.md"))
    result = plan_resolve.resolve_plans(sand, change_plan=None)
    assert result.ok is False
    assert result.needs_author_confirmation is True
    after = list((sand / "docs" / "plans").glob("*.md"))
    assert after == before  # no silent write


def test_author_minimal_only_when_called(tmp_path: Path) -> None:
    sand = tmp_path / "sand"
    sand.mkdir()
    path = plan_resolve.author_minimal_plan(
        sand, preset="builtin:ce-executor-supervisor", today=date(2026, 7, 25)
    )
    assert path.is_file()
    assert "e2e_smoke_marker.txt" in path.read_text(encoding="utf-8")


def test_resolve_without_change_plan_still_discovers(tmp_path: Path) -> None:
    sand = _git_init(tmp_path / "sand")
    good = sand / "docs" / "plans" / "2026-07-22-001-feat-multi-sort-supervisor-e2e-plan.md"
    _write_e2e_plan(good)
    result = plan_resolve.resolve_plans(sand, change_plan=None)
    assert result.ok is True
    assert result.change_plan_path is None
    assert Path(result.workload_plan_path).resolve() == good.resolve()
