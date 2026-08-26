"""Cross-layer end-to-end wiring proof for ``ralph-project-bootstrap``.

U9 closes the loop between Unit 2 (audit), Unit 3 (agent-docs), Unit 4
(pipeline-suite), Unit 5 (cli-probe), Unit 6 (smoke-runner), and Unit 7
(handoff). The previous units exercised each module in isolation; this
suite drives them as a single pipeline against the public fixtures and
asserts that the observable contract holds end-to-end:

* A blank project can be seeded, validated, smoke-tested (with a fake
  runner), and rendered into an official launch command — without
  ever spawning the real ``ralph`` binary.
* An existing project round-trips through the same flow as noop
  (compose, apply_pipeline_config, upgrade_provenance).
* A conflicting-doc project surfaces a ``sync_mirror_conflict`` blocker
  before any persistent write.
* An ``UnsafeBackend`` smoke refusal demotes the handoff to
  ``incomplete_static_only`` and prefixes the command with the
  canonical ``[CANDIDATE`` marker.
* The deleted ``ralph-hats`` skill is not installable via the
  subprocess entry point; the other public skills are.

Two static guards run inside the suite:

* ``scripts/check-cli-doc-drift.sh`` must report PASSED (or list only
  ``KNOWN_DRIFTS``).
* ``git grep -l 'ralph-hats'`` over non-historical paths must surface
  no new occurrences — the only legitimate mentions are inside
  ``docs/achieved/``, ``docs/brainstorms/``, ``docs/plans/``, the
  bootstrap prompt template's forbidden-pattern list, and the
  bootstrap ``SKILL.md``.

Hard rules (per the unit dispatch):

* No Rust crates / presets / ``.ralph/`` are touched.
* All subprocess calls are stubbed — the real ``ralph`` binary is
  never spawned.
* Only ``tmp_path`` and the existing public fixtures are used.
* No new Python deps are installed.
"""
from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

import pytest

import agent_docs  # noqa: F401  (Unit-3 helper)
import bootstrap_pipeline  # noqa: F401  (unified entry)
import cli_probe  # noqa: F401  (Unit-5 helper)
import handoff  # noqa: F401  (Unit-7 helper)
import pipeline_suite  # noqa: F401  (Unit-4 helper)
import smoke_runner  # noqa: F401  (Unit-6 helper)
import _fixtures  # noqa: F401
import _paths  # noqa: F401
import _probe_runner  # noqa: F401
from audit import collect_project_facts, run_audit  # noqa: F401  (Unit-2 audit)


ROOT = Path(__file__).resolve().parents[2]
FIXTURES_PROJECTS = ROOT / "skills" / "ralph-project-bootstrap" / "fixtures" / "projects"
FIXTURES_CLI = ROOT / "skills" / "ralph-project-bootstrap" / "fixtures" / "cli"
SKILLS_DIR = ROOT / "skills"
INSTALL_SCRIPT = SKILLS_DIR / "install.py"
DOC_DRIFT_SCRIPT = ROOT / "scripts" / "check-cli-doc-drift.sh"

MARKER_ID = "agents-docs-v1"

# Pipeline-suite kwargs that match the public ``existing-suite`` fixture
# byte-for-byte (verified by ``test_existing_suite_fixture_round_trip``
# in the contract suite). Keeping them centralised here means the
# idempotent tests below exercise the same authoring contract as the
# single-unit tests.
PIPELINE_KWARGS: dict[str, object] = dict(
    preset="builtin:ce-executor-pipeline",
    plan_path="plan.md",
    prompt_file="PROMPT.pipeline.md",
    backend="claude",
    budget_max_iterations=12,
    budget_wall_clock_seconds=7200,
    preflight_strict=True,
    diagnostics_enabled=True,
    project_root_marker="./",
)


# ---------------------------------------------------------------------------
# Shared helpers
# ---------------------------------------------------------------------------


def _seed_blank_project(project: Path) -> Path:
    """Materialise the ``blank`` fixture and add a plan so audit passes."""
    _fixtures.materialise("blank", project)
    (project / "plan.md").write_text(
        "# plan\n\nBootstrap pipeline fixture plan.\n", encoding="utf-8"
    )
    return project


def _seed_existing_suite(project: Path) -> Path:
    """Materialise the ``existing-suite`` fixture and add matching
    AGENTS.md / CLAUDE.md managed sections so doc-compose is a noop.

    The pipeline files already match the rendered suite byte-for-byte;
    the docs we add below carry the same body the test will request
    so ``compose_agent_docs`` classifies the run as a noop too.
    """
    _fixtures.materialise("existing-suite", project)
    body = (
        "linter: cargo clippy --workspace --all-targets -- -D warnings\n"
        "test_runner: cargo nextest run\n"
    )
    doc_files = (project / "AGENTS.md", project / "CLAUDE.md")
    section = agent_docs.render_managed_section(MARKER_ID, body.splitlines())
    for doc in doc_files:
        doc.write_text(
            (
                "# Existing Project Agent Doc\n\n"
                "User-authored prose that must survive a managed-section update.\n\n"
                f"{section}\n\n"
                "## Operator appendix\n\n"
                "Anything below this line is operator-authored.\n"
            ),
            encoding="utf-8",
        )
    return project


def _apply_compose(
    project: Path,
    agents_body: str,
) -> tuple[
    "agent_docs.ComposeResult", "agent_docs.ComposeResult", "pipeline_suite.ApplyResult"
]:
    """Run the doc-compose + pipeline-config-compose stage for ``project``.

    The function is pure: it never touches the disk; callers wrap the
    returned tuples in an ``AtomicWriter`` if they want a real write.
    """
    agents_text = (
        (project / "AGENTS.md").read_text(encoding="utf-8")
        if (project / "AGENTS.md").exists()
        else None
    )
    claude_text = (
        (project / "CLAUDE.md").read_text(encoding="utf-8")
        if (project / "CLAUDE.md").exists()
        else None
    )
    agents_result = agent_docs.compose_agent_docs(
        agents_text, agents_body, marker_id=MARKER_ID
    )
    claude_result = agent_docs.compose_agent_docs(
        claude_text, agents_body, marker_id=MARKER_ID
    )
    config_text = (
        (project / "ralph.pipeline.yml").read_text(encoding="utf-8")
        if (project / "ralph.pipeline.yml").exists()
        else None
    )
    pipeline_result = pipeline_suite.apply_pipeline_config(
        config_text,
        **PIPELINE_KWARGS,  # type: ignore[arg-type]
        project_facts=collect_project_facts(project),
    )
    return agents_result, claude_result, pipeline_result


def _write_pipeline_suite(project: Path) -> tuple[Path, Path, Path]:
    """Render and atomically write all three pipeline files.

    Returns ``(pipeline_yml_path, prompt_path, provenance_path)``.
    """
    suite = pipeline_suite.compose_suite(
        **PIPELINE_KWARGS,  # type: ignore[arg-type]
        project_facts=collect_project_facts(project),
    )
    prompt_text = pipeline_suite.render_prompt_md(
        plan_path=str(PIPELINE_KWARGS["plan_path"]),
        preset=str(PIPELINE_KWARGS["preset"]),
        project_root=str(PIPELINE_KWARGS["project_root_marker"]),
        prompt_file=str(PIPELINE_KWARGS["prompt_file"]),
    )
    provenance_text = pipeline_suite.render_provenance(suite)
    config_path = project / "ralph.pipeline.yml"
    prompt_path = project / "PROMPT.pipeline.md"
    provenance_path = project / "ralph.bootstrap.yml"
    ops = [
        (config_path, suite.config),
        (prompt_path, prompt_text),
        (provenance_path, provenance_text),
    ]
    with agent_docs.AtomicWriter(ops) as writer:
        committed, rolled = writer.execute()
    assert not rolled, f"atomic write rolled back: {rolled}"
    assert set(committed) == {config_path, prompt_path, provenance_path}
    return config_path, prompt_path, provenance_path


def _write_agent_docs(
    project: Path,
    agents_result: agent_docs.ComposeResult,
    claude_result: agent_docs.ComposeResult,
) -> tuple[Path, Path]:
    """Atomically write AGENTS.md / CLAUDE.md with the produced texts."""
    agents_path = project / "AGENTS.md"
    claude_path = project / "CLAUDE.md"
    ops = [
        (agents_path, agents_result.text or ""),
        (claude_path, claude_result.text or ""),
    ]
    with agent_docs.AtomicWriter(ops) as writer:
        committed, rolled = writer.execute()
    assert not rolled, f"atomic write rolled back: {rolled}"
    assert set(committed) == {agents_path, claude_path}
    return agents_path, claude_path


def _fake_green_runner():
    """Return a runner that replays the public ``green`` CLI fixture.

    Drives ``probe_capability`` / ``validate_pipeline`` to a four-stage
    OK classification without ever spawning the real ``ralph`` binary.
    """
    invocations = cli_probe.load_fixture("green")
    return _probe_runner.make_runner(invocations)


def _fake_smoke_runner(stdout: str = "", returncode: int = 0):
    """Return a runner that returns a fixed CompletedProcess for smoke.

    Defaults to the LOOP_COMPLETE marker so the harness classifies
    ``bounded_terminal_reached``; override ``stdout`` for other paths.
    """

    def _runner(args, **kwargs):  # noqa: ARG001
        return subprocess.CompletedProcess(
            args=tuple(args),
            returncode=returncode,
            stdout=stdout,
            stderr="",
        )

    return _runner


def _run_doc_drift() -> subprocess.CompletedProcess[str]:
    """Run ``scripts/check-cli-doc-drift.sh`` and return the result.

    The script is a bash program (shebang + bash-only ``case``); we
    dispatch through ``bash`` so the test does not depend on the
    file's executable bit and stays portable across CI runners.
    """
    return subprocess.run(
        ["bash", str(DOC_DRIFT_SCRIPT)],
        capture_output=True,
        text=True,
        check=False,
        cwd=str(ROOT),
    )


def _git_grep_ralph_hats() -> list[str]:
    """Return repo-relative paths that still mention ``ralph-hats``.

    Excludes the historical brainstorms / plans / achieved folders
    where archived plans legitimately keep the name.
    """
    proc = subprocess.run(
        [
            "git",
            "grep",
            "-l",
            "ralph-hats",
            "--",
            ".",
            ":!docs/achieved",
            ":!docs/brainstorms",
            ":!docs/plans",
            ":!docs/solutions",
        ],
        capture_output=True,
        text=True,
        check=False,
        cwd=str(ROOT),
    )
    if proc.returncode not in (0, 1):
        # 0 = matches found; 1 = no matches. Anything else is a git error.
        raise RuntimeError(
            f"git grep failed: rc={proc.returncode} stderr={proc.stderr}"
        )
    return [
        line.strip()
        for line in proc.stdout.splitlines()
        if line.strip()
    ]


# ---------------------------------------------------------------------------
# Static guards (run once per session; also reachable as individual tests)
# ---------------------------------------------------------------------------


class TestCrossLayerBootstrap:
    """End-to-end wiring proof for the bootstrap pipeline (U9)."""

    def test_doc_drift_script_passes_or_only_known(self) -> None:
        """``scripts/check-cli-doc-drift.sh`` must pass, OR list only
        ``KNOWN_DRIFTS`` entries. This plan does not touch CLI, so
        exit code 0 is the expected outcome.
        """
        result = _run_doc_drift()
        if result.returncode == 0:
            return
        # Any drift surfaced must be a baseline / known entry — the
        # check script already filters these by default and exits 0
        # for them. If we still ended up non-zero, fail loudly.
        assert "drift detected" in result.stderr.lower(), (
            f"unexpected doc-drift failure:\nstdout={result.stdout}\n"
            f"stderr={result.stderr}"
        )
        pytest.fail(
            "check-cli-doc-drift.sh reported drift outside KNOWN_DRIFTS:\n"
            f"stdout={result.stdout}\nstderr={result.stderr}"
        )

    def test_ralph_hats_not_referenced_outside_historical_paths(self) -> None:
        """``ralph-hats`` must not leak back into the active surface.

        Legitimate mentions are confined to:
          * historical brainstorms / plans / achieved folders;
          * the bootstrap prompt template's forbidden-pattern list;
          * the bootstrap SKILL.md (which references the ban);
          * the SKILL.md guardrail sections.
        """
        offenders = _git_grep_ralph_hats()
        # Strip the leading ``./`` for readable assertions.
        normalised = sorted(
            p[2:] if p.startswith("./") else p for p in offenders
        )
        # Build the allow-list of legitimate paths (relative to repo root).
        # Each entry is a substring that, when present in a matched path,
        # marks the occurrence as expected. The references and SKILL.md
        # document the ban; the test files explicitly lock the deletion
        # guard; the installed `.claude/skills/README.md` mirrors the
        # top-level skills README (stale catalogue copy).
        allowed_substrings = (
            "docs/achieved/",
            "docs/brainstorms/",
            "docs/plans/",
            "docs/solutions/",
            # The bootstrap forbidden-pattern list + its docs / SKILL.md.
            "skills/ralph-project-bootstrap/scripts/pipeline_suite.py",
            "skills/ralph-project-bootstrap/SKILL.md",
            "skills/ralph-project-bootstrap/references/",
            # Test files that lock the deletion guard contract.
            "skills/tests/test_install.py",
            "skills/tests/test_project_bootstrap_contract.py",
            "skills/tests/test_project_bootstrap_e2e.py",
            # Top-level skills README mirrored into the installed copy.
            ".claude/skills/README.md",
        )
        unexpected = [
            path
            for path in normalised
            if not any(needle in path for needle in allowed_substrings)
        ]
        assert not unexpected, (
            "ralph-hats leaked into the active surface: "
            f"{unexpected!r} (full scan: {normalised!r})"
        )

    # --- U9 end-to-end wiring tests --------------------------------------

    def test_e2e_blank_project_to_complete(self, tmp_path: Path) -> None:
        """Blank project: audit -> compose -> write -> probe -> smoke
        (SafeBackend + green runner) -> handoff with official command.

        The project's working tree must end up with exactly the files
        the bootstrap pipeline owns: AGENTS.md, CLAUDE.md, plan.md,
        ralph.pipeline.yml, PROMPT.pipeline.md, ralph.bootstrap.yml.
        """
        project = _seed_blank_project(tmp_path / "project")

        # Stage 1: audit.
        decision = run_audit(
            project,
            preset=str(PIPELINE_KWARGS["preset"]),
            plan_path=str(PIPELINE_KWARGS["plan_path"]),
        )
        assert not decision.is_blocking, decision.issues

        # Stage 2: compose docs + pipeline config (pure).
        body = (
            "linter: cargo clippy --workspace --all-targets -- -D warnings\n"
            "test_runner: cargo nextest run\n"
        )
        agents_result, claude_result, _ = _apply_compose(project, body)
        assert agents_result.kind == "created"
        assert claude_result.kind == "created"

        # Stage 3: atomic write of docs + pipeline suite.
        _write_agent_docs(project, agents_result, claude_result)
        _write_pipeline_suite(project)

        # Stage 4: 4-stage static gate (fake runner). ``validate_pipeline``
        # invokes ``probe_capability`` internally, so we do NOT call the
        # probe separately — the fixture runner consumes each argv
        # exactly once.
        runner = _fake_green_runner()
        decisions = cli_probe.validate_pipeline(
            binary="ralph",
            config_path="ralph.pipeline.yml",
            preset=str(PIPELINE_KWARGS["preset"]),
            prompt_file=str(PIPELINE_KWARGS["prompt_file"]),
            plan_path=str(PIPELINE_KWARGS["plan_path"]),
            runner=runner,
        )
        assert len(decisions) == 4
        for stage in decisions:
            assert stage.outcome == "ok", stage.blocked_reason

        # Stage 5: bounded smoke (SafeBackend + fake runner).
        backend = smoke_runner.SafeBackend(
            name="replay-fixture",
            transcript_path=project / "transcript.jsonl",
        )
        cfg = smoke_runner.SmokeConfig(
            binary="/tmp/fake-ralph",
            config_path="ralph.pipeline.yml",
            preset=str(PIPELINE_KWARGS["preset"]),
            prompt_file=str(PIPELINE_KWARGS["prompt_file"]),
            plan_path=str(PIPELINE_KWARGS["plan_path"]),
        )
        smoke_result = smoke_runner.run_smoke(
            backend,
            cfg,
            transcript_dir=tmp_path / "transcripts",
            runner=_fake_smoke_runner(stdout="LOOP_COMPLETE\n"),
        )
        assert smoke_result.outcome == "bounded_terminal_reached"

        # Stage 6: handoff at level=complete. Per the Unit 5 anti-fake-
        # positive contract (plan 2026-07-19-001 F7), the level MUST
        # be driven by a typed ``smoke_outcome`` field, not by a free-
        # text evidence string.
        inputs = handoff.HandoffInputs(
            binary="ralph",
            config_path="ralph.pipeline.yml",
            preset=str(PIPELINE_KWARGS["preset"]),
            plan_path=str(PIPELINE_KWARGS["plan_path"]),
            prompt_file=str(PIPELINE_KWARGS["prompt_file"]),
            level="complete",
            files_created=(
                "AGENTS.md",
                "CLAUDE.md",
                "ralph.pipeline.yml",
                "PROMPT.pipeline.md",
                "ralph.bootstrap.yml",
            ),
            validation_evidence=tuple(
                f"{stage.stage}:{stage.outcome}" for stage in decisions
            ),
            smoke_outcome=smoke_result.outcome,
            smoke_failure_bucket=smoke_result.failure_bucket,
            smoke_evidence=(smoke_result.outcome,),
        )
        artifact = handoff.build_handoff(inputs)
        assert artifact.level == "complete"
        assert artifact.command
        assert "[CANDIDATE" not in artifact.command
        argv_text = " ".join(artifact.command_argv)
        assert "-c" in argv_text and "-H" in argv_text
        assert artifact.command_argv[5] == "run"
        assert "PROMPT.pipeline.md" in argv_text
        assert "plan.md" not in argv_text

        # Final state of the working tree: exactly the owned files,
        # nothing else (no .tmp siblings leaked).
        expected = {
            "AGENTS.md",
            "CLAUDE.md",
            "plan.md",
            "ralph.bootstrap.yml",
            "ralph.pipeline.yml",
            "PROMPT.pipeline.md",
        }
        actual = {
            p.relative_to(project).as_posix()
            for p in project.rglob("*")
            if p.is_file()
        }
        assert actual == expected, (
            f"unexpected files in project tree: "
            f"extra={actual - expected} missing={expected - actual}"
        )

    def test_e2e_existing_project_idempotent_run(self, tmp_path: Path) -> None:
        """Existing project: compose + apply_pipeline_config + upgrade
        provenance all return ``noop`` on the second run.

        Re-uses the existing-suite pipeline fixture (already
        byte-equal to the rendered suite) plus AGENTS.md / CLAUDE.md
        carrying the same body the test requests.
        """
        project = _seed_existing_suite(tmp_path / "project")

        # Audit must not block (existing suite carries plan.md).
        decision = run_audit(
            project,
            preset=str(PIPELINE_KWARGS["preset"]),
            plan_path=str(PIPELINE_KWARGS["plan_path"]),
        )
        assert not decision.is_blocking, decision.issues

        body = (
            "linter: cargo clippy --workspace --all-targets -- -D warnings\n"
            "test_runner: cargo nextest run\n"
        )

        # First compose round: writes new docs (created), pipeline
        # config + provenance (noop / noop because the fixture bytes
        # already match the rendered suite).
        agents_result, claude_result, pipeline_result = _apply_compose(
            project, body
        )
        # The seed wrote AGENTS.md / CLAUDE.md with the exact body the
        # test requests, so compose returns noop (byte-equal section).
        assert agents_result.kind == "noop"
        assert claude_result.kind == "noop"
        assert pipeline_result.kind == "noop"

        # The fixture deliberately represents a legacy 0.2.0 suite. Ordinary
        # recomposition leaves it byte-stable; provenance upgrade blocks until
        # the explicit verified whole-profile refresh path is selected.
        suite = pipeline_suite.compose_suite(**PIPELINE_KWARGS)  # type: ignore[arg-type]
        existing_provenance = (project / "ralph.bootstrap.yml").read_text(
            encoding="utf-8"
        )
        upgrade = pipeline_suite.upgrade_provenance(existing_provenance, suite)
        assert upgrade.kind == "blocker"
        assert upgrade.code == "provenance_corrupt"

        # Smoke still ends OK, so handoff stays at level=complete.
        backend = smoke_runner.SafeBackend(name="replay-fixture")
        cfg = smoke_runner.SmokeConfig(
            binary="/tmp/fake-ralph",
            config_path="ralph.pipeline.yml",
            preset=str(PIPELINE_KWARGS["preset"]),
            prompt_file=str(PIPELINE_KWARGS["prompt_file"]),
            plan_path=str(PIPELINE_KWARGS["plan_path"]),
        )
        smoke_result = smoke_runner.run_smoke(
            backend,
            cfg,
            transcript_dir=tmp_path / "transcripts",
            runner=_fake_smoke_runner(stdout="LOOP_COMPLETE\n"),
        )
        assert smoke_result.outcome == "bounded_terminal_reached"

        inputs = handoff.HandoffInputs(
            binary="ralph",
            config_path="ralph.pipeline.yml",
            preset=str(PIPELINE_KWARGS["preset"]),
            plan_path=str(PIPELINE_KWARGS["plan_path"]),
            prompt_file=str(PIPELINE_KWARGS["prompt_file"]),
            level="complete",
            files_noop=(
                "AGENTS.md",
                "CLAUDE.md",
                "ralph.pipeline.yml",
                "PROMPT.pipeline.md",
                "ralph.bootstrap.yml",
            ),
            validation_evidence=("capability:ok",),
            smoke_outcome=smoke_result.outcome,
            smoke_failure_bucket=smoke_result.failure_bucket,
            smoke_evidence=(smoke_result.outcome,),
        )
        artifact = handoff.build_handoff(inputs)
        assert artifact.level == "complete"

    def test_e2e_conflicting_docs_blocks(self, tmp_path: Path) -> None:
        """Conflicting fixture: doc-compose must surface
        ``sync_mirror_conflict`` BEFORE any write reaches the disk.
        """
        project = tmp_path / "project"
        _fixtures.materialise("conflicting-docs", project)
        agents_text = (project / "AGENTS.md").read_text(encoding="utf-8")
        claude_text = (project / "CLAUDE.md").read_text(encoding="utf-8")

        # Compose AGENTS.md (single doc, no sync required) → ok.
        agents_body = "linter: cargo clippy --workspace --all-targets\n"
        agents_result = agent_docs.compose_agent_docs(
            agents_text, agents_body, marker_id=MARKER_ID
        )
        assert agents_result.kind in {"created", "updated", "noop"}

        # Compose CLAUDE.md with sync_with_other_doc=True, disagreeing
        # body → must surface the blocker.
        claude_body = "linter: ruff check src tests\n"
        claude_result = agent_docs.compose_agent_docs(
            claude_text,
            claude_body,
            marker_id=MARKER_ID,
            sync_with_other_doc=True,
            other_body=agents_body,
        )
        assert claude_result.kind == "blocker"
        assert claude_result.code == "sync_mirror_conflict"

        # Confirm the AtomicWriter was never invoked: neither file on
        # disk was mutated by the failed compose path.
        assert (project / "AGENTS.md").read_text(encoding="utf-8") == agents_text
        assert (project / "CLAUDE.md").read_text(encoding="utf-8") == claude_text

    def test_e2e_unsafe_backend_blocks_smoke(self, tmp_path: Path) -> None:
        """UnsafeBackend at the smoke stage demotes the handoff to
        ``incomplete_static_only`` and prefixes the command with
        the canonical ``[CANDIDATE`` marker.
        """
        project = _seed_blank_project(tmp_path / "project")

        decision = run_audit(
            project,
            preset=str(PIPELINE_KWARGS["preset"]),
            plan_path=str(PIPELINE_KWARGS["plan_path"]),
        )
        assert not decision.is_blocking, decision.issues

        body = "linter: cargo clippy\n"
        agents_result, claude_result, _ = _apply_compose(project, body)
        _write_agent_docs(project, agents_result, claude_result)
        _write_pipeline_suite(project)

        # Static gate still passes (we feed a green runner).
        decisions = cli_probe.validate_pipeline(
            binary="ralph",
            config_path="ralph.pipeline.yml",
            preset=str(PIPELINE_KWARGS["preset"]),
            prompt_file=str(PIPELINE_KWARGS["prompt_file"]),
            plan_path=str(PIPELINE_KWARGS["plan_path"]),
            runner=_fake_green_runner(),
        )
        assert all(stage.outcome == "ok" for stage in decisions)

        # UnsafeBackend at the smoke stage.
        backend = smoke_runner.UnsafeBackend(name="user-mock", kind="mock")
        cfg = smoke_runner.SmokeConfig(
            binary="/tmp/fake-ralph",
            config_path="ralph.pipeline.yml",
            preset=str(PIPELINE_KWARGS["preset"]),
            prompt_file=str(PIPELINE_KWARGS["prompt_file"]),
            plan_path=str(PIPELINE_KWARGS["plan_path"]),
        )
        smoke_result = smoke_runner.run_smoke(backend, cfg)
        assert smoke_result.outcome == "not_authorized"
        assert smoke_result.argv == ()

        inputs = handoff.HandoffInputs(
            binary="ralph",
            config_path="ralph.pipeline.yml",
            preset=str(PIPELINE_KWARGS["preset"]),
            plan_path=str(PIPELINE_KWARGS["plan_path"]),
            prompt_file=str(PIPELINE_KWARGS["prompt_file"]),
            level="incomplete_static_only",
            files_created=("AGENTS.md", "CLAUDE.md", "ralph.pipeline.yml"),
            validation_evidence=tuple(
                f"{stage.stage}:{stage.outcome}" for stage in decisions
            ),
            smoke_outcome=smoke_result.outcome,
            smoke_failure_bucket=smoke_result.failure_bucket,
            smoke_evidence=(smoke_result.outcome,),
        )
        artifact = handoff.build_handoff(inputs)
        assert artifact.level == "incomplete_static_only"
        assert artifact.command.startswith("[CANDIDATE")
        assert "smoke-not-authorized" in artifact.report

    def test_e2e_raph_hats_not_installable(self, tmp_path: Path) -> None:
        """The deleted ``ralph-hats`` skill must be rejected by the
        subprocess installer with the canonical 'unknown skill' message,
        and the tmp target must NOT contain a ``ralph-hats`` directory.
        """
        target = tmp_path / "skills"
        target.mkdir()
        result = subprocess.run(
            [
                sys.executable,
                str(INSTALL_SCRIPT),
                "--dir",
                str(target),
                "ralph-hats",
            ],
            capture_output=True,
            text=True,
            check=False,
            cwd=str(ROOT),
        )
        assert result.returncode != 0, (
            f"install ralph-hats unexpectedly succeeded: rc={result.returncode}\n"
            f"stdout={result.stdout}\nstderr={result.stderr}"
        )
        assert "unknown skill" in result.stderr, (
            f"stderr missing 'unknown skill': {result.stderr!r}"
        )
        assert not (target / "ralph-hats").exists(), (
            "ralph-hats directory was materialised despite the rejection"
        )

    def test_e2e_other_skills_still_installable(self, tmp_path: Path) -> None:
        """The other five public skills must install cleanly via the
        subprocess entry point with ``--force``.
        """
        target = tmp_path / "skills"
        target.mkdir()
        # Plan 2026-08-02-001: ``ralph-loop`` retired.
        expected = (
            "ralph-preset-author",
            "ralph-preset-review",
            "ralph-project-bootstrap",
            "ralph-run-diagnosis",
        )
        result = subprocess.run(
            [
                sys.executable,
                str(INSTALL_SCRIPT),
                "--dir",
                str(target),
                "--force",
                *expected,
            ],
            capture_output=True,
            text=True,
            check=False,
            cwd=str(ROOT),
        )
        assert result.returncode == 0, (
            f"install failed: rc={result.returncode}\n"
            f"stdout={result.stdout}\nstderr={result.stderr}"
        )
        for name in expected:
            assert (target / name).is_dir(), (
                f"missing installed skill: {name}"
            )
            assert (target / name / "SKILL.md").is_file(), (
                f"{name}/SKILL.md missing"
            )


# ---------------------------------------------------------------------------
# Unified-entry (``run_pipeline``) cross-layer coverage
# ---------------------------------------------------------------------------


_BUILTIN_LIST_JSON = json.dumps(
    {
        "presets": [
            {
                "id": "debug",
                "description": "Debug preset",
                "source": "builtin:debug",
                "public": True,
            }
        ]
    }
)

_BUILTIN_SHOW_YAML = (
    "name: debug\n"
    "cli:\n"
    "  backend: claude\n"
    "event_loop:\n"
    "  prompt: |\n"
    "    # debug prompt\n"
    "    Read the supplied plan and follow it end-to-end.\n"
    "  max_iterations: 8\n"
    "  max_runtime_seconds: 1800\n"
)


def _unified_entry_runner(argv, timeout=None, capture_output=False, text=False):
    """Fake ``subprocess.run`` for ``run_pipeline`` cross-layer tests.

    Honours the U03 builtin-resolution argv
    (``preset builtin list --format json`` /
    ``preset builtin show <id> --format yaml``) plus the minimum
    capability / static-gate surface. The legacy ``preset list`` /
    ``preset show`` argv is intentionally NOT honoured — a regression
    that falls back to the old surface fails loudly here.
    """
    ok = subprocess.CompletedProcess(args=tuple(argv), returncode=0, stdout="", stderr="")
    if argv[1:] == ["preset", "builtin", "list", "--format", "json"]:
        return subprocess.CompletedProcess(
            args=tuple(argv), returncode=0, stdout=_BUILTIN_LIST_JSON, stderr=""
        )
    if (
        len(argv) >= 6
        and argv[1:4] == ["preset", "builtin", "show"]
        and argv[-2] == "--format"
    ):
        if argv[4] != "debug":
            return subprocess.CompletedProcess(
                args=tuple(argv), returncode=2, stdout="", stderr="unknown builtin preset"
            )
        return subprocess.CompletedProcess(
            args=tuple(argv), returncode=0, stdout=_BUILTIN_SHOW_YAML, stderr=""
        )
    if argv[1:] == ["--version"]:
        return subprocess.CompletedProcess(
            args=tuple(argv), returncode=0, stdout="ralph 0.1.0-test", stderr=""
        )
    if argv[1:] in (["--help"], ["--json", "--help"]):
        return subprocess.CompletedProcess(
            args=tuple(argv), returncode=0, stdout="usage: ralph ...", stderr=""
        )
    if len(argv) >= 2 and argv[-1] == "--help":
        return subprocess.CompletedProcess(
            args=tuple(argv), returncode=0, stdout="--strict --dry-run", stderr=""
        )
    if len(argv) >= 2 and argv[-1] == "--strict":
        return subprocess.CompletedProcess(
            args=tuple(argv), returncode=0, stdout="ok", stderr=""
        )
    if len(argv) >= 2 and "--dry-run" in argv:
        return subprocess.CompletedProcess(
            args=tuple(argv),
            returncode=0,
            stdout=(
                "Dry run mode - configuration:\n"
                "  Prompt file: PROMPT.debug.md\n"
                "  Max iterations: 8\n"
                "  Max runtime: 1800s\n"
                "  Backend: claude\n"
                "  Idle timeout: 30s\n"
            ),
            stderr="",
        )
    raise AssertionError(f"unified-entry runner: unexpected argv={list(argv)}")


class TestUnifiedEntryPipeline:
    """Cross-layer proof for the single ``run_pipeline`` entry point.

    The helper-level tests above stitch audit / compose / write /
    probe / smoke / handoff by hand; the unified entry must expose the
    same observable contract through one call. These tests exercise
    ``bootstrap_pipeline.run_pipeline`` end-to-end against the public
    fixtures without spawning the real binary.
    """

    def test_e2e_unified_entry_blank_project(self, tmp_path: Path) -> None:
        """Blank project → ``run_pipeline`` produces the suite, the
        managed docs, and typed static evidence in one call; a second
        identical call is a noop."""
        project = _seed_blank_project(tmp_path / "project")

        result = bootstrap_pipeline.run_pipeline(
            cwd=project,
            preset="builtin:debug",
            plan_path="plan.md",
            binary="ralph",
            runner=_unified_entry_runner,
        )

        assert result.blocked is False
        assert result.level == "incomplete_static_only"
        # Owned suite + managed docs all report ``created``.
        assert result.files_created == (
            "ralph.debug.yml",
            "PROMPT.debug.md",
            "AGENTS.md",
            "CLAUDE.md",
        )
        assert (project / "ralph.debug.yml").is_file()
        assert (project / "PROMPT.debug.md").is_file()
        for name in ("AGENTS.md", "CLAUDE.md"):
            doc_text = (project / name).read_text(encoding="utf-8")
            assert doc_text.count("RALPH-BOOTSTRAP-START") == 1
            assert doc_text.count("RALPH-BOOTSTRAP-END") == 1
            assert agent_docs.parse_managed_section(doc_text, MARKER_ID).is_ok
        # Static evidence is typed: one row per gate stage, all ok.
        assert len(result.validation_evidence) == 4
        assert all(entry.endswith(":ok") for entry in result.validation_evidence)
        # Write boundaries hold end-to-end.
        assert not (project / "ralph.pipeline.yml").exists()
        assert not (project / "PROMPT.pipeline.md").exists()
        assert not (project / "ralph.bootstrap.yml").exists()
        assert not list(project.rglob("*.bootstrap.tmp"))

        # Second identical run: every owned artifact is a noop.
        second = bootstrap_pipeline.run_pipeline(
            cwd=project,
            preset="builtin:debug",
            plan_path="plan.md",
            binary="ralph",
            runner=_unified_entry_runner,
        )
        assert second.blocked is False
        assert second.files_created == ()
        assert second.files_updated == ()
        assert second.files_noop == (
            "ralph.debug.yml",
            "PROMPT.debug.md",
            "AGENTS.md",
            "CLAUDE.md",
        )

    def test_e2e_unified_entry_conflicting_docs_blocks(self, tmp_path: Path) -> None:
        """Conflicting-docs project → ``run_pipeline`` blocks with
        ``sync_mirror_conflict`` and leaves the project untouched."""
        project = tmp_path / "project"
        _fixtures.materialise("conflicting-docs", project)
        (project / "plan.md").write_text(
            "# plan\n\nBootstrap pipeline fixture plan.\n", encoding="utf-8"
        )
        agents_before = (project / "AGENTS.md").read_text(encoding="utf-8")
        claude_before = (project / "CLAUDE.md").read_text(encoding="utf-8")

        result = bootstrap_pipeline.run_pipeline(
            cwd=project,
            preset="builtin:debug",
            plan_path="plan.md",
            binary="ralph",
            runner=_unified_entry_runner,
        )

        assert result.blocked is True
        assert result.code == "sync_mirror_conflict"
        assert result.files_created == ()
        assert result.files_updated == ()
        assert result.files_noop == ()
        assert not (project / "ralph.debug.yml").exists()
        assert not (project / "PROMPT.debug.md").exists()
        assert (project / "AGENTS.md").read_text(encoding="utf-8") == agents_before
        assert (project / "CLAUDE.md").read_text(encoding="utf-8") == claude_before
        assert not list(project.rglob("*.bootstrap.tmp"))
