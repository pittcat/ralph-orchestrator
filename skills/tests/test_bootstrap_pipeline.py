"""Tests for forced ``bootstrap_pipeline`` entry (P0/P1)."""

from __future__ import annotations

import subprocess
from pathlib import Path

import e2e_bootstrap_pipeline as bootstrap_pipeline  # type: ignore[import-not-found]
import plan_resolve  # type: ignore[import-not-found]
import sandbox_suite  # type: ignore[import-not-found]
import pytest


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


def _write_change(path: Path, *, presets: bool = False) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    body = (
        "# Orch\n\n## Goal Capsule\n\n- Objective: verify flow.\n\n"
        "### U1\n\nTouch `crates/ralph-core/src/event_loop/mod.rs`.\n"
        "### U2\n\nTouch `crates/ralph-core/src/drift/engine.rs`.\n"
    )
    if presets:
        body += "\n### U3\n\nTouch `presets/en/ce-executor-supervisor.yml`.\n"
    path.write_text(body, encoding="utf-8")


def _write_workload(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "# E2E\n\n### U1\n\nTouch `sorts/bubble.py`.\n"
        "### U2\n\nTouch `sorts/insertion.py`.\n",
        encoding="utf-8",
    )


def test_generate_suite_requires_change_context(tmp_path: Path) -> None:
    sandbox = tmp_path / "sandbox"
    sandbox.mkdir()
    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")
    with pytest.raises(sandbox_suite.SandboxError, match="change_plan_path"):
        sandbox_suite.generate_suite(
            sandbox=sandbox,
            preset="builtin:ce-executor-pipeline",
            plan_path=plan,
        )


def test_pipeline_requires_change_plan(tmp_path: Path) -> None:
    sand = _git_init(tmp_path / "sand")
    result = bootstrap_pipeline.run_pipeline(
        sandbox=sand,
        change_plan="",
        preset="builtin:ce-executor-pipeline",
        skip_plan_diff=True,
    )
    assert result.ok is False
    assert result.blocked is True


def test_pipeline_preset_gap_before_suite(tmp_path: Path) -> None:
    orch = _git_init(tmp_path / "orch")
    sand = _git_init(tmp_path / "sand")
    change = orch / "docs" / "plans" / "005.md"
    _write_change(change, presets=True)
    _write_workload(
        sand / "docs" / "plans" / "2026-07-22-001-feat-multi-sort-supervisor-e2e-plan.md"
    )
    result = bootstrap_pipeline.run_pipeline(
        sandbox=sand,
        change_plan=change,
        preset="builtin:ce-executor-supervisor",
        skip_plan_diff=True,
        preset_continue_confirmed=False,
    )
    assert result.ok is False
    assert result.needs == "preset_gap"
    assert not (sand / "ralph.ce-executor-supervisor.yml").exists()


def test_pipeline_binary_freshness_gate(tmp_path: Path) -> None:
    orch = _git_init(tmp_path / "orch")
    sand = _git_init(tmp_path / "sand")
    change = orch / "docs" / "plans" / "005.md"
    _write_change(change)
    _write_workload(
        sand / "docs" / "plans" / "2026-07-22-001-feat-multi-sort-supervisor-e2e-plan.md"
    )
    fake = tmp_path / "ralph"
    fake.write_text("#!/bin/sh\necho ralph 0.0.0\n", encoding="utf-8")
    fake.chmod(0o755)
    # build_repo with Cargo.toml newer than binary outside target/
    (orch / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
    result = bootstrap_pipeline.run_pipeline(
        sandbox=sand,
        change_plan=change,
        preset="builtin:ce-executor-pipeline",
        binary_explicit=str(fake),
        build_repo=orch,
        skip_plan_diff=True,
        preset_continue_confirmed=True,
        trusted_only=False,
    )
    assert result.ok is False
    assert result.needs == "binary_resolution"


def test_prompt_embeds_change_and_workload(tmp_path: Path) -> None:
    sandbox = tmp_path / "sandbox"
    sandbox.mkdir()
    plan = tmp_path / "plan.md"
    plan.write_text("# Workload Body Unique\n", encoding="utf-8")
    suite = sandbox_suite.generate_suite(
        sandbox=sandbox,
        preset="builtin:ce-executor-pipeline",
        plan_path=plan,
        change_plan_path="/orch/docs/plans/005.md",
        change_plan_hash="abcd",
        change_summary="## Goal Capsule\n- Objective: UNIQUE_CHANGE_INTENT",
    )
    prompt = Path(suite.prompt_path).read_text(encoding="utf-8")
    assert "UNIQUE_CHANGE_INTENT" in prompt
    assert "Workload Body Unique" in prompt
    assert "--prompt-file" in suite.launch_argv
    assert "--plan" in suite.launch_argv
    # --prompt-file precedes --plan in argv
    assert suite.launch_argv.index("--prompt-file") < suite.launch_argv.index("--plan")
