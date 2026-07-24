"""Contract tests for ``ralph-e2e-bootstrap``.

These tests are the public, behavioural contract for the skill:

* The catalog (Unit 1) — ``PUBLIC_SKILLS`` /
  ``.claude-plugin/marketplace.json`` / on-disk ``SKILL.md`` plus
  ``agents/openai.yaml`` all agree the skill exists.
* The combo-box / interaction contract (R12 / S7) — every user
  decision in the skill is rendered as a combo-box with 2–4 options,
  the recommended option first, every option carries a consequence
  clause, and an ``Other`` escape hatch is always present.
* No preset writes (R5) and no live ``ralph run`` (R10) — the
  scripts in this skill are static-analysis only.
* No plan rewrite (R13) — the sandbox suite generator never
  mutates the caller-supplied plan file.
* Plan × diff audit (Unit 2) — ``scripts/plan_diff.py`` returns
  ``ok=True`` for a coherent plan + diff, raises ``clarify_codes``
  for drift, and ``blocked=True`` for an unreadable plan.
* Binary resolution (Unit 3) — ``scripts/binary_resolve.py``
  honours explicit > env > PATH priority and is testable without a
  real ``ralph`` on the host ``PATH``.
* Sandbox suite (Unit 4) — ``scripts/sandbox_suite.py`` generates
  the preset-bound pair and refuses ``presets/`` writes.
* Static gate + handoff (Unit 5) — ``scripts/gate.py`` /
  ``scripts/handoff.py`` produce a ``static_only`` report whose
  argv carries ``-c <config> -H <preset> --plan <abs>``.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

# Loaded via skills/tests/conftest.py: the sibling
# ``ralph-project-bootstrap`` helpers are pre-registered in
# ``sys.modules`` so we can import them by name.
import install  # type: ignore[import-not-found]
import plan_diff  # type: ignore[import-not-found]
import binary_resolve  # type: ignore[import-not-found]
import sandbox_suite  # type: ignore[import-not-found]
import gate  # type: ignore[import-not-found]
import e2e_handoff as handoff  # type: ignore[import-not-found]

ROOT = Path(__file__).resolve().parents[2]
SKILLS_DIR = ROOT / "skills"
MARKETPLACE = ROOT / ".claude-plugin" / "marketplace.json"
SKILL_DIR = SKILLS_DIR / "ralph-e2e-bootstrap"
SKILL_DOC = SKILL_DIR / "SKILL.md"
AGENT_METADATA = SKILL_DIR / "agents" / "openai.yaml"
INTERACTION_DOC = SKILL_DIR / "references" / "interaction.md"

# Ensure the helper modules under ``skills/ralph-e2e-bootstrap/scripts``
# are importable by name.
import importlib.util
import sys

_SCRIPTS = SKILL_DIR / "scripts"


def _load_into_syspath() -> None:
    if str(_SCRIPTS) not in sys.path:
        sys.path.insert(0, str(_SCRIPTS))


_load_into_syspath()


# ---------------------------------------------------------------------------
# Unit 1 — catalog & skill scaffolding
# ---------------------------------------------------------------------------


def test_e2e_bootstrap_in_public_skills() -> None:
    assert "ralph-e2e-bootstrap" in install.PUBLIC_SKILLS


def test_e2e_bootstrap_in_marketplace_manifest() -> None:
    data = json.loads(MARKETPLACE.read_text(encoding="utf-8"))
    advertised = data["plugins"][0]["skills"]
    assert "./skills/ralph-e2e-bootstrap" in advertised


def test_e2e_bootstrap_directory_has_skill_md_and_agent_metadata() -> None:
    assert SKILL_DIR.is_dir(), "skills/ralph-e2e-bootstrap must exist"
    assert SKILL_DOC.is_file(), "SKILL.md must exist"
    assert AGENT_METADATA.is_file(), "agents/openai.yaml must exist"
    agent_text = AGENT_METADATA.read_text(encoding="utf-8")
    assert "ralph-e2e-bootstrap" in agent_text


def test_e2e_bootstrap_skill_doc_mentions_boundaries() -> None:
    """SKILL.md must declare the no-Rust-mutation boundary (R14)."""
    text = SKILL_DOC.read_text(encoding="utf-8")
    text_lower = text.lower()
    for anchor in (
        "Boundaries",
        "combo-box",
        "static_only",
        "ralph-project-bootstrap",
        "ralph-preset-author",
        "--plan",
    ):
        assert anchor in text or anchor.lower() in text_lower, (
            f"SKILL.md must mention {anchor!r}"
        )


def test_e2e_bootstrap_interaction_doc_has_decision_table() -> None:
    text = INTERACTION_DOC.read_text(encoding="utf-8")
    # Every decision point from SKILL.md must appear in
    # references/interaction.md and be documented as a combo-box.
    for decision in (
        "plan_diff_clarify",
        "binary_resolution",
        "preset_gap",
        "write_conflict",
        "argv_shape",
        "live_run",
    ):
        assert decision in text, f"interaction.md must document {decision!r}"
    for token in ("recommended option", "Other", "consequence", "2–4"):
        assert token in text, f"interaction.md must contain combo-box token {token!r}"


def test_e2e_bootstrap_no_preset_write_anchor() -> None:
    """SKILL.md must forbid preset authoring (R5)."""
    text = SKILL_DOC.read_text(encoding="utf-8")
    assert "ralph-preset-author" in text
    assert "preset-bound" in text or "preset-bound" in text.lower()


def test_e2e_bootstrap_no_live_run_default() -> None:
    """SKILL.md must declare the static-only default (R10)."""
    text = SKILL_DOC.read_text(encoding="utf-8")
    # Either 'static_only' appears literally, or the document
    # explicitly states the skill does not spawn live runs by default.
    assert "static_only" in text
    assert "NEVER spawn a live" in text or "never spawn a live" in text.lower()


def test_e2e_bootstrap_no_plan_rewrite() -> None:
    """SKILL.md must declare the plan-read-only invariant (R13)."""
    text = SKILL_DOC.read_text(encoding="utf-8")
    assert "NEVER rewrites the plan file" in text or "read-only" in text.lower()
    assert "--plan" in text


# ---------------------------------------------------------------------------
# Unit 2 — plan × diff audit
# ---------------------------------------------------------------------------


def test_plan_diff_returns_ok_for_coherent_inputs(tmp_path: Path) -> None:
    plan = tmp_path / "plan.md"
    plan.write_text(
        "# Plan\n"
        "\n"
        "## Implementation Units\n"
        "\n"
        "### U1. Add foo\n"
        "\n"
        "Touch `crates/foo.rs` and `crates/bar.rs`.\n"
        "\n"
        "### U2. Update baz\n"
        "\n"
        "Touch `crates/baz.rs`.\n",
        encoding="utf-8",
    )
    decision = plan_diff.run_audit(
        plan,
        repo_root=tmp_path,
        diff_provider=lambda: ("crates/foo.rs", "crates/bar.rs", "crates/baz.rs"),
    )
    assert decision.ok is True
    assert decision.blocked is False
    assert decision.clarify_codes == ()
    assert decision.plan_hash != ""
    assert "crates/foo.rs" in decision.plan_intent_paths


def test_plan_diff_emits_clarify_codes_for_drift(tmp_path: Path) -> None:
    plan = tmp_path / "plan.md"
    plan.write_text(
        "# Plan\n"
        "\n"
        "### U1. Fix only the renderer\n"
        "\n"
        "Touch `crates/renderer.rs`.\n",
        encoding="utf-8",
    )
    decision = plan_diff.run_audit(
        plan,
        repo_root=tmp_path,
        diff_provider=lambda: ("crates/renderer.rs", "crates/auth.rs", "crates/api.rs"),
    )
    assert decision.ok is False
    assert decision.blocked is False
    assert "scope_drift" in decision.clarify_codes


def test_plan_diff_blocks_on_unreadable_plan(tmp_path: Path) -> None:
    decision = plan_diff.run_audit(
        tmp_path / "missing.md",
        repo_root=tmp_path,
        diff_provider=lambda: (),
    )
    assert decision.ok is False
    assert decision.blocked is True
    assert any(issue.code == "plan_unreadable" for issue in decision.issues)


def test_plan_diff_emits_stale_when_diff_empty(tmp_path: Path) -> None:
    plan = tmp_path / "plan.md"
    plan.write_text(
        "# Plan\n"
        "\n"
        "### U1. Fix thing\n"
        "\n"
        "Touch `crates/thing.rs`.\n",
        encoding="utf-8",
    )
    decision = plan_diff.run_audit(
        plan,
        repo_root=tmp_path,
        diff_provider=lambda: (),
    )
    assert "plan_stale" in decision.clarify_codes


def test_plan_diff_has_no_write_side_effects(tmp_path: Path) -> None:
    """No files are created under the sandbox tree by the audit."""
    plan = tmp_path / "plan.md"
    plan.write_text("### U1. Touch `crates/x.rs`.\n", encoding="utf-8")
    plan_diff.run_audit(
        plan,
        repo_root=tmp_path,
        diff_provider=lambda: ("crates/x.rs",),
    )
    # Only the plan file should exist in tmp_path; nothing else.
    assert list(tmp_path.iterdir()) == [plan]


# ---------------------------------------------------------------------------
# Unit 3 — binary resolution
# ---------------------------------------------------------------------------


def test_binary_resolve_prefers_explicit_over_env_and_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = tmp_path / "ralph-explicit"
    fake.write_text("#!/bin/sh\necho 'explicit'\n", encoding="utf-8")
    fake.chmod(0o755)
    monkeypatch.setenv("PATH", "")
    monkeypatch.delenv("RALPH_BINARY", raising=False)

    # No PATH entry, no env, explicit path present → explicit wins.
    resolution = binary_resolve.resolve_binary(
        explicit_path=str(fake),
        runner=lambda argv, **kwargs: _ok_completed(argv, "fake-ralph 0.0.0"),
    )
    assert resolution.ok is True
    assert resolution.source == "explicit"
    assert resolution.version == "fake-ralph 0.0.0"


def test_binary_resolve_falls_back_to_env(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("PATH", "")
    monkeypatch.setenv("RALPH_BINARY", "/does/not/exist/ralph")

    def fail_runner(argv, **kwargs):  # noqa: ANN001, ARG001
        raise FileNotFoundError("no such file")

    resolution = binary_resolve.resolve_binary(
        runner=fail_runner,
    )
    # Env entry exists but the binary is not executable → combo-box
    # so the operator can rebuild / install / override.
    assert resolution.ok is False
    assert resolution.reason == "combo_box"
    assert resolution.source == "env"


def test_binary_resolve_combo_box_when_path_is_empty(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("PATH", "")
    monkeypatch.delenv("RALPH_BINARY", raising=False)
    resolution = binary_resolve.resolve_binary(
        path_iter=lambda: (),
        runner=lambda argv, **kwargs: _ok_completed(argv, ""),
    )
    assert resolution.ok is False
    assert resolution.reason == "combo_box"
    assert resolution.source == "missing"
    assert resolution.binary == binary_resolve.MISSING_TOKEN


def test_binary_resolve_uses_fake_path_iter(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("PATH", "")
    monkeypatch.delenv("RALPH_BINARY", raising=False)
    fake = tmp_path / "ralph"
    fake.write_text("#!/bin/sh\necho fake\n", encoding="utf-8")
    fake.chmod(0o755)

    # We use the fake path iterator and a runner that fakes the
    # version probe so the resolved binary does not have to be a
    # real Ralph binary.
    resolution = binary_resolve.resolve_binary(
        path_iter=lambda: iter([tmp_path]),
        runner=lambda argv, **kwargs: _ok_completed(argv, "fake-ralph 0.0.0"),
        require_version=True,
    )
    # The fake binary is on the fake PATH and answers --version.
    assert resolution.ok is True
    assert resolution.source == "path"


# ---------------------------------------------------------------------------
# Unit 4 — sandbox suite
# ---------------------------------------------------------------------------


def test_sandbox_suite_generates_preset_bound_pair(tmp_path: Path) -> None:
    sandbox = tmp_path / "sandbox"
    sandbox.mkdir()
    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")
    result = sandbox_suite.generate_suite(
        sandbox=sandbox,
        preset="builtin:ce-executor-pipeline",
        plan_path=plan,
    )
    assert (sandbox / "ralph.ce-executor-pipeline.yml").is_file()
    assert (sandbox / "PROMPT.ce-executor-pipeline.md").is_file()
    assert result.config_sha256 and result.prompt_sha256 and result.plan_sha256
    assert "-c" in result.argv and "-H" in result.argv
    assert "--plan" in result.argv and str(plan) in result.argv
    # No preset mutation.
    assert "presets" not in result.argv


def test_sandbox_suite_refuses_presets_subtree(tmp_path: Path) -> None:
    bad = tmp_path / "presets"
    bad.mkdir()
    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")
    with pytest.raises(sandbox_suite.SandboxError) as excinfo:
        sandbox_suite.generate_suite(
            sandbox=bad,
            preset="builtin:ce-executor-pipeline",
            plan_path=plan,
        )
    assert "presets" in str(excinfo.value).lower()


def test_sandbox_suite_plan_file_hash_unchanged(tmp_path: Path) -> None:
    sandbox = tmp_path / "sandbox"
    sandbox.mkdir()
    plan = tmp_path / "plan.md"
    original = "# Plan\n"
    plan.write_text(original, encoding="utf-8")
    before = plan.read_bytes()
    sandbox_suite.generate_suite(
        sandbox=sandbox,
        preset="builtin:ce-executor-pipeline",
        plan_path=plan,
    )
    after = plan.read_bytes()
    assert before == after


def test_sandbox_suite_blocks_unwritable_sandbox(tmp_path: Path) -> None:
    # A non-existent directory must raise.
    missing = tmp_path / "no-such-dir"
    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")
    with pytest.raises(sandbox_suite.SandboxError):
        sandbox_suite.generate_suite(
            sandbox=missing,
            preset="builtin:ce-executor-pipeline",
            plan_path=plan,
        )


def test_derive_stem_for_builtin_and_file() -> None:
    assert (
        sandbox_suite.derive_stem("builtin:ce-executor-pipeline")
        == "ce-executor-pipeline"
    )
    assert (
        sandbox_suite.derive_stem("presets/en/ce-executor-pipeline.yml")
        == "ce-executor-pipeline"
    )
    assert sandbox_suite.derive_stem("/abs/path/my-team.yml") == "my-team"


# ---------------------------------------------------------------------------
# Unit 5 — static gate + handoff
# ---------------------------------------------------------------------------


def test_static_gate_blocks_without_plan_path() -> None:
    report = gate.run_static_gate(
        binary="ralph",
        config_path="ralph.foo.yml",
        preset="builtin:ce-executor-pipeline",
        plan_path=None,
    )
    assert report.ok is False
    assert report.plan_required is True
    assert report.dry_run.outcome == "blocked_input"


def test_static_gate_runs_four_stages_with_fake_runner() -> None:
    argv_seen: list[tuple[str, ...]] = []

    def fake_runner(argv, **kwargs):  # noqa: ANN001, ARG001
        argv_seen.append(tuple(argv))
        return _ok_completed(argv, "fake 0.0.0")

    report = gate.run_static_gate(
        binary="ralph",
        config_path="ralph.foo.yml",
        preset="builtin:ce-executor-pipeline",
        plan_path="/abs/plan.md",
        runner=fake_runner,
    )
    # All four stages executed.
    assert report.capability.stage == "capability"
    assert report.preset_check.stage == "preset_check"
    assert report.preflight.stage == "preflight"
    assert report.dry_run.stage == "dry_run"


def test_handoff_static_only_command_shape(tmp_path: Path) -> None:
    inputs = handoff.HandoffInputs(
        binary="ralph",
        config_path="ralph.foo.yml",
        preset="builtin:ce-executor-pipeline",
        plan_path="/abs/plan.md",
        level="static_only",
        sandbox_path="sandbox",
        validation_evidence=("capability=ok", "preset_check=ok"),
        residual_risks=("live loop not run",),
        stage_outcomes=("capability=ok", "preset_check=ok", "preflight=ok", "dry_run=ok"),
    )
    artifact = handoff.build_handoff(inputs)
    assert artifact.level == "static_only"
    assert "ralph" in artifact.command
    assert "-c" in artifact.command
    assert "builtin:ce-executor-pipeline" in artifact.command
    assert "--plan" in artifact.command
    assert "Static load passed" in artifact.report
    assert "NOT closed" in artifact.report


def test_handoff_blocked_requires_summary() -> None:
    with pytest.raises(ValueError):
        handoff.HandoffInputs(
            binary="ralph",
            config_path="ralph.foo.yml",
            preset="builtin:ce-executor-pipeline",
            plan_path="/abs/plan.md",
            level="blocked",
            sandbox_path="sandbox",
            blocker_summary="",
        )


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


class _Completed:
    def __init__(self, stdout: str = "", stderr: str = "", returncode: int = 0) -> None:
        self.stdout = stdout
        self.stderr = stderr
        self.returncode = returncode


def _ok_completed(argv, stdout: str = "") -> _Completed:
    text = stdout
    if not text:
        if "--version" in argv:
            text = "ralph 0.0.0"
        elif "--help" in argv:
            # Capability probe needs the human-form help page to
            # contain the literal `--strict` / `--dry-run` tokens.
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