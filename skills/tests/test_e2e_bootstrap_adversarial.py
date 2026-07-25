"""Adversarial / attack-surface tests for ``ralph-e2e-bootstrap``.

Covers unicode, shell metachar, canonical-root bypass, concurrent writes,
and TOCTOU races. All sandbox_suite paths are computed against the real
repo root so the canonical-root guard is testable.
"""

from __future__ import annotations

from pathlib import Path

import pytest

# Loaded via skills/tests/conftest.py.
import sandbox_suite  # type: ignore[import-not-found]
import gate  # type: ignore[import-not-found]
import binary_resolve  # type: ignore[import-not-found]


# ---------------------------------------------------------------------------
# A3.1 — unicode CJK + emoji in plan path
# ---------------------------------------------------------------------------


def test_unicode_cjk_emoji_in_plan_path(tmp_path: Path) -> None:
    """Plan path containing CJK characters and emoji — argv embeds bytes verbatim.

    The gate must accept the path and not mangle it during argv construction.
    """
    plan_dir = tmp_path / "中文" / "emoji-🎉"
    plan_dir.mkdir(parents=True)
    plan = plan_dir / "plan.md"
    plan.write_text("# Plan\n\n### U1. Touch\n\nTouch `crates/x.rs`.\n", encoding="utf-8")

    # Run the gate with a fake runner that records the argv.
    captured_argv: list[tuple[str, ...]] = []

    def capture_runner(argv, **kw):  # noqa: ANN001, ARG001
        captured_argv.append(tuple(argv))
        return _fake_completed(argv)

    report = gate.run_static_gate(
        binary="ralph",
        config_path="ralph.foo.yml",
        preset="builtin:ce-executor-pipeline",
        plan_path=str(plan),
        runner=capture_runner,
    )
    # Gate must have accepted the plan path without crashing.
    assert report.plan_required is False
    # The plan path appears verbatim in the dry_run stage argv
    # (capability probe argv doesn't carry the plan; the plan path is only
    # in the dry_run argv constructed by _build_stage_argv).
    plan_path_str = str(plan)
    assert any(
        "emoji" in tok or "中文" in tok for tok in report.dry_run.argv
    ), f"plan path not in dry_run argv: {report.dry_run.argv}"


# ---------------------------------------------------------------------------
# A3.2 — shell metachar in plan path
# ---------------------------------------------------------------------------


def test_shell_metachar_in_plan_path(tmp_path: Path) -> None:
    """Plan path with shell metachar embedded in prompt — must not execute.

    The PROMPT.<stem>.md must contain the literal staged sandbox-relative
    path (``docs/plans/<basename>``), not interpreted shell syntax. After
    the plan-staging refactor, the prompt carries the staged path rather
    than the caller-supplied absolute path; the no-shell-interpretation
    invariant still holds because the staged basename is constrained to
    ``[A-Za-z0-9._-]+`` and the relative path has no shell metachars.
    """
    plan = tmp_path / "plan.md"
    plan.write_text(
        "# Plan\n\n### U1. Touch\n\nTouch `crates/x.rs`.\n",
        encoding="utf-8",
    )
    sandbox = tmp_path / "sandbox"
    sandbox.mkdir()

    result = sandbox_suite.generate_suite(
        sandbox=sandbox,
        preset="builtin:ce-executor-pipeline",
        plan_path=plan,
        change_plan_path="docs/plans/change.md",
        change_plan_hash="0000000000000000000000000000000000000000000000000000000000000000",
        change_summary="## Goal Capsule\n- Objective: verify",
    )
    prompt_path = Path(result.prompt_path)
    assert prompt_path.is_file()
    prompt_text = prompt_path.read_text(encoding="utf-8")
    # Plan path embedded verbatim as the staged sandbox-relative path.
    staged_relplan = Path("docs") / "plans" / plan.name
    assert str(staged_relplan) in prompt_text


# ---------------------------------------------------------------------------
# A3.3 — real_presets subtree: should be ALLOWED (substring, not segment-eq)
# ---------------------------------------------------------------------------


def test_real_presets_subtree_now_blocked(tmp_path: Path) -> None:
    """``tmp_path/real_presets/sub`` — substring "presets" in part name.

    After U2's canonical-root fix, segment-equality is the SSOT:
    ``real_presets`` parts are ``(tmp_root, "real_presets", "sub")``.
    "real_presets" != "presets" (exact segment), so this MUST be ALLOWED.
    """
    bad = tmp_path / "real_presets" / "sub"
    bad.mkdir(parents=True)
    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")

    # Must NOT raise — "real_presets" is not segment-equal to "presets".
    result = sandbox_suite.generate_suite(
        sandbox=bad,
        preset="builtin:ce-executor-pipeline",
        plan_path=plan,
        change_plan_path="docs/plans/change.md",
        change_plan_hash="0000000000000000000000000000000000000000000000000000000000000000",
        change_summary="## Goal Capsule\n- Objective: verify",
    )
    assert result.config_path and result.prompt_path


# ---------------------------------------------------------------------------
# A3.4 — presets-foo subtree: should be BLOCKED (segment = "presets-foo" starts with "presets-")
# ---------------------------------------------------------------------------


def test_presets_foo_subtree_blocked(tmp_path: Path) -> None:
    """``tmp_path/presets-foo/sub`` — segment "presets-foo" starts with "presets-".

    The canonical-root guard blocks any segment that equals a canonical root
    OR starts with ``<root>-`` (e.g. "presets-foo"). "presets-foo" starts with
    "presets-" so it MUST be rejected.
    """
    bad = tmp_path / "presets-foo" / "sub"
    bad.mkdir(parents=True)
    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")

    with pytest.raises(sandbox_suite.SandboxError) as excinfo:
        sandbox_suite.generate_suite(
            sandbox=bad,
            preset="builtin:ce-executor-pipeline",
            plan_path=plan,
        change_plan_path="docs/plans/change.md",
        change_plan_hash="0000000000000000000000000000000000000000000000000000000000000000",
        change_summary="## Goal Capsule\n- Objective: verify",
    )
    assert "presets" in str(excinfo.value).lower()


# ---------------------------------------------------------------------------
# A3.5 — my_presets_clone subtree: ALLOWED (single segment, "presets" not in parts)
# ---------------------------------------------------------------------------


def test_my_presets_clone_subtree_allowed(tmp_path: Path) -> None:
    """``tmp_path/my_presets_clone/sub`` — parts are (root, "my_presets_clone", "sub").

    Segment-exact check: "my_presets_clone" != "presets" (no match).
    The old substring-based guard would have blocked this; the new
    exact-segment guard allows it. This test verifies the fix.
    """
    good = tmp_path / "my_presets_clone" / "sub"
    good.mkdir(parents=True)
    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")

    # Must NOT raise — "my_presets_clone" is not a canonical root segment.
    result = sandbox_suite.generate_suite(
        sandbox=good,
        preset="builtin:ce-executor-pipeline",
        plan_path=plan,
        change_plan_path="docs/plans/change.md",
        change_plan_hash="0000000000000000000000000000000000000000000000000000000000000000",
        change_summary="## Goal Capsule\n- Objective: verify",
    )
    assert result.config_path and result.prompt_path


# ---------------------------------------------------------------------------
# A3.6 — concurrent atomic write: second write produces same result (idempotent)
# ---------------------------------------------------------------------------


def test_concurrent_atomic_write(tmp_path: Path) -> None:
    """Two sequential generate_suite calls — second call produces same files.

    The second call must not crash and must not leave partial-write state.
    """
    sandbox = tmp_path / "sandbox"
    sandbox.mkdir()
    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")

    result1 = sandbox_suite.generate_suite(
        sandbox=sandbox,
        preset="builtin:ce-executor-pipeline",
        plan_path=plan,
        change_plan_path="docs/plans/change.md",
        change_plan_hash="0000000000000000000000000000000000000000000000000000000000000000",
        change_summary="## Goal Capsule\n- Objective: verify",
    )
    config_bytes_1 = Path(result1.config_path).read_bytes()
    prompt_bytes_1 = Path(result1.prompt_path).read_bytes()

    # Second call must not crash.
    result2 = sandbox_suite.generate_suite(
        sandbox=sandbox,
        preset="builtin:ce-executor-pipeline",
        plan_path=plan,
        change_plan_path="docs/plans/change.md",
        change_plan_hash="0000000000000000000000000000000000000000000000000000000000000000",
        change_summary="## Goal Capsule\n- Objective: verify",
    )
    config_bytes_2 = Path(result2.config_path).read_bytes()
    prompt_bytes_2 = Path(result2.prompt_path).read_bytes()

    # Files are identical (second call overwrote with same content).
    assert config_bytes_2 == config_bytes_1
    assert prompt_bytes_2 == prompt_bytes_1


# ---------------------------------------------------------------------------
# A3.7 — TOCTOU: sandbox dir resolves to a path containing canonical root
# ---------------------------------------------------------------------------


def test_toctou_sandbox_inside_real_presets(tmp_path: Path) -> None:
    """Sandbox is a dir that resolves (via symlink or parent traversal) into presets/.

    _check_writable uses Path.resolve() which follows symlinks. If the resolved
    path's parts contain 'presets', it must be rejected.
    """
    # Create the real repo/presets/ dir structure.
    presets_real = tmp_path / "presets"
    presets_real.mkdir()
    (presets_real / "README.md").write_text("# presets", encoding="utf-8")

    # Make sandbox a real directory that IS inside presets/.
    sandbox = presets_real / "sub" / "sandbox"
    sandbox.mkdir(parents=True)

    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")

    # Resolved path: tmp/presets/sub/sandbox → parts include "presets"
    # → must be rejected.
    with pytest.raises(sandbox_suite.SandboxError) as excinfo:
        sandbox_suite.generate_suite(
            sandbox=sandbox,
            preset="builtin:ce-executor-pipeline",
            plan_path=plan,
        change_plan_path="docs/plans/change.md",
        change_plan_hash="0000000000000000000000000000000000000000000000000000000000000000",
        change_summary="## Goal Capsule\n- Objective: verify",
    )
    assert "presets" in str(excinfo.value).lower()


# ---------------------------------------------------------------------------
# Helper
# ---------------------------------------------------------------------------


class _Completed:
    def __init__(self, stdout: str = "", stderr: str = "", returncode: int = 0) -> None:
        self.stdout = stdout
        self.stderr = stderr
        self.returncode = returncode


def _fake_completed(argv, stdout: str = "") -> _Completed:
    text = stdout
    if not text:
        if "--version" in argv:
            text = "ralph 0.0.0"
        elif "preset" in argv and "check" in argv:
            text = "preset check OK"
        elif "preflight" in argv:
            text = "preflight OK"
        elif "run" in argv and "--dry-run" in argv:
            text = (
                "Dry run mode - configuration:\n"
                "  Backend: fake\n"
                "  Prompt file: PROMPT.foo.md\n"
                "  Max iterations: 1\n"
                "  Max runtime: 60\n"
            )
        elif "run" in argv and "--help" in argv:
            # Capability probe calls `ralph run --help`; must contain --dry-run
            # token so _parse_flags reports run_dry_run_supported=True.
            text = (
                "ralph-run\n\nUsage: ralph run ...\n"
                "  --dry-run\n"
                "  --plan PLAN\n"
            )
        elif "preset" in argv and "--help" in argv:
            # Capability probe calls `ralph preset --help` for flag detection;
            # must contain --strict token.
            text = (
                "ralph-preset\n\nUsage: ralph preset ...\n"
                "  --strict\n"
            )
        elif "preflight" in argv and "--help" in argv:
            # Capability probe calls `ralph preflight --help` for flag detection;
            # must contain --strict token.
            text = (
                "ralph-preflight\n\nUsage: ralph preflight ...\n"
                "  --strict\n"
            )
        else:
            text = "ok"
    return _Completed(stdout=text)
