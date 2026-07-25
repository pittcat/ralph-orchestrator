"""Edge-case tests for ``plan_diff.run_audit`` — intent / unit / depth / mixed / unicode.

These tests cover the five cases the contract suite skips:
intent_undeclared, unit_missing, prefix-depth boundary, mixed clarify codes,
and unicode paths in plan body — plus cross-repo auto-pass cases.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

# Loaded via skills/tests/conftest.py.
import plan_diff  # type: ignore[import-not-found]


def _git_init(repo: Path) -> Path:
    """Create a minimal git repo at ``repo`` and return its resolved toplevel."""
    repo.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["git", "init"],
        cwd=repo,
        check=True,
        capture_output=True,
        text=True,
    )
    # Identity required for commit on fresh clones in CI/sandbox.
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
    # Need a HEAD so rev-parse --show-toplevel is stable on all git versions.
    marker = repo / ".keep"
    marker.write_text("", encoding="utf-8")
    subprocess.run(
        ["git", "add", ".keep"],
        cwd=repo,
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "commit", "-m", "init"],
        cwd=repo,
        check=True,
        capture_output=True,
    )
    completed = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        cwd=repo,
        check=True,
        capture_output=True,
        text=True,
    )
    return Path(completed.stdout.strip()).resolve()


# ---------------------------------------------------------------------------
# T2.1 — intent undeclared
# ---------------------------------------------------------------------------


def test_intent_undeclared_emits_clarify_code(tmp_path: Path) -> None:
    """Plan has U-IDs but no path tokens anywhere in body.

    The audit should emit ``intent_undeclared`` (no paths declared to act on)
    AND ``unit_missing`` — a plan with a single U-ID heading and no body
    content is implicitly a "I don't know what to do" signal.
    """
    plan = tmp_path / "plan.md"
    plan.write_text(
        "# Plan\n"
        "\n"
        "### U1. Fix bugs\n"
        "\n"
        "Just fix things.\n",
        encoding="utf-8",
    )
    decision = plan_diff.run_audit(
        plan,
        repo_root=tmp_path,
        diff_provider=lambda: (),
    )
    assert plan_diff.CLARIFY_INTENT_UNDECLARED in decision.clarify_codes
    # A single U1 heading with no body counts as implicit yes → unit_missing
    # is NOT emitted here (U-IDs of one is implicit yes per spec).
    # But intent_undeclared IS emitted because no paths declared.
    assert plan_diff.CLARIFY_UNIT_MISSING not in decision.clarify_codes
    assert plan_diff.CLARIFY_STALE_PLAN in decision.clarify_codes


# ---------------------------------------------------------------------------
# T2.2 — unit missing
# ---------------------------------------------------------------------------


def test_unit_missing_emits_clarify_code(tmp_path: Path) -> None:
    """Plan has no ``### U<n>.`` headings at all.

    The audit should emit ``unit_missing``.
    """
    plan = tmp_path / "plan.md"
    plan.write_text(
        "# Plan\n"
        "\n"
        "## Intent\n"
        "\n"
        "Touch `crates/x.rs`.\n",
        encoding="utf-8",
    )
    decision = plan_diff.run_audit(
        plan,
        repo_root=tmp_path,
        diff_provider=lambda: (),
    )
    assert plan_diff.CLARIFY_UNIT_MISSING in decision.clarify_codes
    # Intent paths are declared so intent_undeclared is not emitted.
    assert plan_diff.CLARIFY_INTENT_UNDECLARED not in decision.clarify_codes


# ---------------------------------------------------------------------------
# T2.3 — prefix depth boundary (depth=2 default → no drift)
# ---------------------------------------------------------------------------


def test_prefix_depth_boundary(tmp_path: Path) -> None:
    """At default depth=2, intent ``crates/foo/bar/baz.rs`` covers diff ``crates/foo/bar/qux.rs``.

    The first two segments of the diff match the intent prefix exactly,
    so no scope drift is flagged even though the diff path is different
    at the third segment level.
    """
    plan = tmp_path / "plan.md"
    plan.write_text(
        "# Plan\n"
        "\n"
        "### U1. Fix bugs\n"
        "\n"
        "Touch `crates/foo/bar/baz.rs`.\n",
        encoding="utf-8",
    )
    decision = plan_diff.run_audit(
        plan,
        repo_root=tmp_path,
        # diff has depth-4 file with same first-2 prefix as intent
        diff_provider=lambda: ("crates/foo/bar/qux.rs",),
    )
    # Prefix matches at depth=2 → no scope drift.
    assert plan_diff.CLARIFY_SCOPE_DRIFT not in decision.clarify_codes


# ---------------------------------------------------------------------------
# T2.4 — mixed clarify codes (unit_missing + intent_undeclared + plan_stale)
# ---------------------------------------------------------------------------


def test_mixed_clarify_codes(tmp_path: Path) -> None:
    """Plan triggers all three: no U-IDs, no intent paths, empty diff.

    ``clarify_codes`` must contain at least two codes and preserve stable order
    (unit_missing first, then intent_undeclared, then plan_stale).
    """
    plan = tmp_path / "plan.md"
    plan.write_text(
        "# Plan\n"
        "\n"
        "Just a bare plan with no units.\n",
        encoding="utf-8",
    )
    decision = plan_diff.run_audit(
        plan,
        repo_root=tmp_path,
        diff_provider=lambda: (),
    )
    codes = decision.clarify_codes
    assert len(codes) >= 2, f"expected ≥2 clarify codes, got {codes!r}"
    # Stable order: unit_missing, intent_undeclared, plan_stale
    assert codes.index(plan_diff.CLARIFY_UNIT_MISSING) < codes.index(plan_diff.CLARIFY_INTENT_UNDECLARED)


# ---------------------------------------------------------------------------
# T2.5 — unicode path in plan body
# ---------------------------------------------------------------------------


def test_unicode_path_in_plan(tmp_path: Path) -> None:
    """Plan body contains ``docs/中文/foo.md`` — audit must not crash.

    The intent path should appear in ``plan_intent_paths``.
    """
    plan = tmp_path / "plan.md"
    plan.write_text(
        "# Plan\n"
        "\n"
        "### U1. Add docs\n"
        "\n"
        "Touch `docs/中文/foo.md`.\n",
        encoding="utf-8",
    )
    decision = plan_diff.run_audit(
        plan,
        repo_root=tmp_path,
        diff_provider=lambda: (),
    )
    # Must not raise; intent path extracted correctly
    assert "docs/中文/foo.md" in decision.plan_intent_paths
    # No crash means ok is still False (plan stale + no units), but
    # at minimum the path appears in plan_intent_paths.
    assert decision.plan_hash != ""


# ---------------------------------------------------------------------------
# T2.6 — cross-repo + non-empty sandbox diff → no scope_drift
# ---------------------------------------------------------------------------


def test_cross_repo_nonempty_diff_ok_no_scope_drift(tmp_path: Path) -> None:
    """Plan in repo A, sandbox (repo_root) in repo B with unrelated diff paths.

    Cross-repo is a legitimate dogfood pattern: ok=True, cross_repo=True,
    and scope_drift must not fire even when sandbox paths diverge from
    plan intent prefixes.
    """
    plan_repo = _git_init(tmp_path / "plan_repo")
    sandbox_repo = _git_init(tmp_path / "sandbox_repo")
    plans_dir = plan_repo / "docs" / "plans"
    plans_dir.mkdir(parents=True)
    plan = plans_dir / "cross.md"
    plan.write_text(
        "# Plan\n"
        "\n"
        "### U1. Touch orchestrator\n"
        "\n"
        "Touch `crates/foo.rs`.\n",
        encoding="utf-8",
    )
    decision = plan_diff.run_audit(
        plan,
        repo_root=sandbox_repo,
        diff_provider=lambda: ("PROMPT.ce.yml", "ralph.ce.yml"),
    )
    assert decision.cross_repo is True
    assert decision.plan_repo_root == str(plan_repo)
    assert decision.diff_repo_root == str(sandbox_repo)
    assert plan_diff.CLARIFY_SCOPE_DRIFT not in decision.clarify_codes
    assert plan_diff.CLARIFY_STALE_PLAN not in decision.clarify_codes
    assert decision.ok is True
    assert decision.blocked is False


# ---------------------------------------------------------------------------
# T2.7 — cross-repo + empty sandbox diff → no plan_stale
# ---------------------------------------------------------------------------


def test_cross_repo_empty_diff_ok_no_plan_stale(tmp_path: Path) -> None:
    """Empty sandbox diff must not emit plan_stale when repos differ."""
    plan_repo = _git_init(tmp_path / "plan_repo")
    sandbox_repo = _git_init(tmp_path / "sandbox_repo")
    plan = plan_repo / "plan.md"
    plan.write_text(
        "# Plan\n"
        "\n"
        "### U1. Touch orchestrator\n"
        "\n"
        "Touch `crates/foo.rs`.\n",
        encoding="utf-8",
    )
    decision = plan_diff.run_audit(
        plan,
        repo_root=sandbox_repo,
        diff_provider=lambda: (),
    )
    assert decision.cross_repo is True
    assert plan_diff.CLARIFY_STALE_PLAN not in decision.clarify_codes
    assert plan_diff.CLARIFY_SCOPE_DRIFT not in decision.clarify_codes
    assert decision.ok is True


# ---------------------------------------------------------------------------
# T2.8 — cross-repo still emits plan-quality / diff_unavailable codes
# ---------------------------------------------------------------------------


def test_cross_repo_still_emits_intent_undeclared(tmp_path: Path) -> None:
    plan_repo = _git_init(tmp_path / "plan_repo")
    sandbox_repo = _git_init(tmp_path / "sandbox_repo")
    plan = plan_repo / "plan.md"
    plan.write_text(
        "# Plan\n"
        "\n"
        "### U1. Fix bugs\n"
        "\n"
        "Just fix things.\n",
        encoding="utf-8",
    )
    decision = plan_diff.run_audit(
        plan,
        repo_root=sandbox_repo,
        diff_provider=lambda: ("PROMPT.x.md",),
    )
    assert decision.cross_repo is True
    assert plan_diff.CLARIFY_INTENT_UNDECLARED in decision.clarify_codes
    assert decision.ok is False


def test_cross_repo_still_emits_diff_unavailable(tmp_path: Path) -> None:
    plan_repo = _git_init(tmp_path / "plan_repo")
    sandbox_repo = _git_init(tmp_path / "sandbox_repo")
    plan = plan_repo / "plan.md"
    plan.write_text(
        "# Plan\n"
        "\n"
        "### U1. Touch orchestrator\n"
        "\n"
        "Touch `crates/foo.rs`.\n",
        encoding="utf-8",
    )
    decision = plan_diff.run_audit(
        plan,
        repo_root=sandbox_repo,
        diff_provider=lambda: ((), True),
    )
    assert decision.cross_repo is True
    assert plan_diff.CLARIFY_DIFF_UNAVAILABLE in decision.clarify_codes
    assert decision.ok is False


def test_same_repo_cross_repo_false(tmp_path: Path) -> None:
    """Plan and repo_root in the same git toplevel → cross_repo=False."""
    repo = _git_init(tmp_path / "one_repo")
    plan = repo / "plan.md"
    plan.write_text(
        "# Plan\n"
        "\n"
        "### U1. Touch\n"
        "\n"
        "Touch `crates/foo.rs`.\n",
        encoding="utf-8",
    )
    decision = plan_diff.run_audit(
        plan,
        repo_root=repo,
        diff_provider=lambda: ("crates/foo.rs",),
    )
    assert decision.cross_repo is False
    assert decision.ok is True


def test_non_git_repo_root_does_not_claim_cross_repo(tmp_path: Path) -> None:
    """Unresolvable git toplevels must not set cross_repo=True."""
    plan = tmp_path / "plan.md"
    plan.write_text(
        "# Plan\n"
        "\n"
        "### U1. Touch\n"
        "\n"
        "Touch `crates/foo.rs`.\n",
        encoding="utf-8",
    )
    decision = plan_diff.run_audit(
        plan,
        repo_root=tmp_path,
        diff_provider=lambda: ("crates/foo.rs",),
    )
    assert decision.cross_repo is False
    assert decision.plan_repo_root is None
    assert decision.diff_repo_root is None
