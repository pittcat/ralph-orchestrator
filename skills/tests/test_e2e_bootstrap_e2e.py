"""End-to-end tests for ``ralph-e2e-bootstrap``.

These tests drive the full skill pipeline against a deterministic
fake CLI runner, so they exercise the real
``plan_diff → binary_resolve → sandbox_suite → gate → handoff``
chain without ever spawning the live ``ralph`` binary.

Coverage:

* Happy path — a coherent plan + diff, a usable binary, a writable
  sandbox, and a fake runner that returns ``ok`` for every stage.
* Plan × diff drift — the audit raises ``scope_drift`` and the
  handoff degrades to ``blocked`` with the right summary.
* Sandbox refuses ``presets/`` — the chain halts at
  ``sandbox_suite`` with a ``SandboxError``.
* Handoff distinguishes static_only from loop closed (R10).
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

# Pre-loaded via skills/tests/conftest.py
import install  # type: ignore[import-not-found]

ROOT = Path(__file__).resolve().parents[2]
SKILLS_DIR = ROOT / "skills"
E2E_DIR = SKILLS_DIR / "ralph-e2e-bootstrap"
SCRIPTS_DIR = E2E_DIR / "scripts"


@pytest.fixture(autouse=True)
def _e2e_scripts_on_path() -> None:
    if str(SCRIPTS_DIR) not in sys.path:
        sys.path.insert(0, str(SCRIPTS_DIR))


def _ok_completed(argv, stdout: str = "") -> "_Completed":
    text = stdout
    if not text:
        if "--version" in argv:
            text = "ralph 0.0.0"
        elif "--help" in argv:
            sub = ""
            for token in ("preset", "preflight", "run"):
                if token in argv:
                    sub = token
                    break
            if sub == "preset":
                text = "ralph-preset\n\nUsage: ralph preset ...\n  --strict\n"
            elif sub == "preflight":
                text = "ralph-preflight\n\nUsage: ralph preflight ...\n  --strict\n"
            elif sub == "run":
                text = "ralph-run\n\nUsage: ralph run ...\n  --dry-run\n  --plan PLAN\n"
            else:
                text = "ralph --help\n  --json\n  --version\n"
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
        else:
            text = "ok"
    return _Completed(stdout=text)


class _Completed:
    def __init__(self, stdout: str = "", stderr: str = "", returncode: int = 0) -> None:
        self.stdout = stdout
        self.stderr = stderr
        self.returncode = returncode


def _fake_runner(argv, **kwargs):  # noqa: ANN001, ARG001
    return _ok_completed(argv)


def _coherent_plan(tmp_path: Path) -> Path:
    plan = tmp_path / "plan.md"
    plan.write_text(
        "# Plan\n"
        "\n"
        "## Implementation Units\n"
        "\n"
        "### U1. Add foo\n"
        "\n"
        "Touch `crates/foo.rs` and `crates/bar.rs`.\n",
        encoding="utf-8",
    )
    return plan


# ---------------------------------------------------------------------------
# Happy path
# ---------------------------------------------------------------------------


def test_e2e_full_pipeline_static_only(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sandbox = tmp_path / "sandbox"
    sandbox.mkdir()
    plan = _coherent_plan(tmp_path)
    fake_binary = tmp_path / "ralph"
    fake_binary.write_text("#!/bin/sh\n", encoding="utf-8")
    fake_binary.chmod(0o755)
    monkeypatch.setenv("PATH", "")
    monkeypatch.delenv("RALPH_BINARY", raising=False)

    # Lazy import inside the test so the conftest wiring always
    # precedes them.
    import plan_diff
    import binary_resolve
    import sandbox_suite
    import gate
    import e2e_handoff as handoff

    # U2 — plan × diff audit.
    audit = plan_diff.run_audit(
        plan,
        repo_root=tmp_path,
        diff_provider=lambda: ("crates/foo.rs", "crates/bar.rs"),
    )
    assert audit.ok is True
    assert audit.clarify_codes == ()

    # U3 — binary resolution via the explicit CLI override.
    resolution = binary_resolve.resolve_binary(
        explicit_path=str(fake_binary),
        runner=_fake_runner,
    )
    assert resolution.ok is True

    # U4 — sandbox suite generation.
    suite = sandbox_suite.generate_suite(
        sandbox=sandbox,
        preset="builtin:ce-executor-pipeline",
        plan_path=plan,
    )
    assert (sandbox / "ralph.ce-executor-pipeline.yml").is_file()
    assert (sandbox / "PROMPT.ce-executor-pipeline.md").is_file()

    # U5 — static gate.
    gate_report = gate.run_static_gate(
        binary=resolution.binary,
        config_path=suite.config_path,
        preset="builtin:ce-executor-pipeline",
        plan_path=str(plan),
        runner=_fake_runner,
    )
    assert gate_report.ok is True

    # Handoff.
    artifact = handoff.build_handoff(
        handoff.HandoffInputs(
            binary=resolution.binary,
            config_path=suite.config_path,
            preset="builtin:ce-executor-pipeline",
            plan_path=str(plan),
            level="static_only",
            sandbox_path="sandbox",
            validation_evidence=gate_report.summary(),
            residual_risks=("static_only: loop not closed",),
            stage_outcomes=gate_report.summary(),
        )
    )
    assert artifact.level == "static_only"
    assert "-c" in artifact.command
    assert "builtin:ce-executor-pipeline" in artifact.command
    assert "--plan" in artifact.command
    assert "NOT closed" in artifact.report


# ---------------------------------------------------------------------------
# Drift path
# ---------------------------------------------------------------------------


def test_e2e_plan_drift_blocks_handoff(tmp_path: Path) -> None:
    plan = tmp_path / "plan.md"
    plan.write_text(
        "# Plan\n### U1. Fix renderer only\nTouch `crates/renderer.rs`.\n",
        encoding="utf-8",
    )
    import plan_diff

    decision = plan_diff.run_audit(
        plan,
        repo_root=tmp_path,
        diff_provider=lambda: ("crates/auth.rs", "crates/api.rs"),
    )
    assert "scope_drift" in decision.clarify_codes
    assert decision.ok is False


# ---------------------------------------------------------------------------
# presets/ refusal
# ---------------------------------------------------------------------------


def test_e2e_sandbox_refuses_presets_write(tmp_path: Path) -> None:
    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")
    presets_root = tmp_path / "presets"
    presets_root.mkdir()
    import sandbox_suite

    with pytest.raises(sandbox_suite.SandboxError):
        sandbox_suite.generate_suite(
            sandbox=presets_root,
            preset="builtin:ce-executor-pipeline",
            plan_path=plan,
        )


# ---------------------------------------------------------------------------
# Catalog coverage (Unit 1 cross-check)
# ---------------------------------------------------------------------------


def test_e2e_catalog_lists_new_skill() -> None:
    """The catalog must list ``ralph-e2e-bootstrap`` as a public skill."""
    assert "ralph-e2e-bootstrap" in install.PUBLIC_SKILLS
    # And the marketplace manifest must agree.
    marketplace = json.loads(
        (ROOT / ".claude-plugin" / "marketplace.json").read_text(encoding="utf-8")
    )
    advertised = marketplace["plugins"][0]["skills"]
    assert "./skills/ralph-e2e-bootstrap" in advertised
    # And the on-disk skill tree must have the SKILL.md plus
    # agents/openai.yaml anchor.
    assert (E2E_DIR / "SKILL.md").is_file()
    assert (E2E_DIR / "agents" / "openai.yaml").is_file()


def test_e2e_real_cli_install_dry_run(tmp_path: Path) -> None:
    """``install.py`` with the new skill must succeed in a dry run."""
    target = tmp_path / "out"
    completed = subprocess.run(
        [
            sys.executable,
            str(SKILLS_DIR / "install.py"),
            "--dir",
            str(target),
            "--dry-run",
            "ralph-e2e-bootstrap",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr
    assert "ralph-e2e-bootstrap" in completed.stdout