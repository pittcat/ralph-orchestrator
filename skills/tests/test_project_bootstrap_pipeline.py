"""Unit tests for the unified ``bootstrap_pipeline`` entry point.

The pipeline owns the end-to-end bootstrap flow: input/root audit,
preset resolution, owned-artifact generation, static validation,
bounded smoke, and typed handoff. This module covers Units U1-U5 of
plan ``2026-08-03-005-refactor-project-bootstrap-skill-plan``.

Conventions:

* Every test invokes :func:`bootstrap_pipeline.run_pipeline` as the
  public surface; the helper-level contract suite continues to cover
  the individual modules.
* Tests inject deterministic ``runner`` callables so no real
  ``ralph`` binary is spawned during the test run.
* File presets are read from the existing public fixtures so the
  pipeline integrates with the canonical byte-stability contract.
* Builtin resolution is exercised against a captured
  ``ralph preset list`` transcript so the source→template mapping is
  verified without spawning the real binary.
"""
from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path
from typing import Callable

import pytest
import yaml

import _fixtures
import agent_docs
import bootstrap_pipeline
import cli_probe
import handoff
import pipeline_suite
import smoke_runner

ROOT = Path(__file__).resolve().parents[2]
FIXTURES_PROJECTS = ROOT / "skills" / "ralph-project-bootstrap" / "fixtures" / "projects"

# Marker id the pipeline owns for the AGENTS.md / CLAUDE.md managed
# sections. Mirrors the fixture convention (``existing-docs`` /
# ``conflicting-docs``) and the helper-level e2e chain.
DOCS_MARKER_ID = "agents-docs-v1"


def _managed_bodies(project: Path) -> tuple[str, str]:
    """Extract the managed-section bodies from AGENTS.md / CLAUDE.md.

    The marker bytes are a public contract (see
    ``agent_docs.render_managed_section``); the extraction below uses
    only those literal markers so the test never reaches into private
    helper internals.
    """
    start_marker = f"<!-- RALPH-BOOTSTRAP-START: {DOCS_MARKER_ID} v1 -->"
    end_marker = f"<!-- RALPH-BOOTSTRAP-END: {DOCS_MARKER_ID} -->"
    bodies: list[str] = []
    for name in ("AGENTS.md", "CLAUDE.md"):
        text = (project / name).read_text(encoding="utf-8")
        start = text.index(start_marker) + len(start_marker) + 1
        end = text.index(end_marker)
        bodies.append(text[start:end].rstrip("\n"))
    return bodies[0], bodies[1]


# ---------------------------------------------------------------------------
# Reusable fixtures / helpers
# ---------------------------------------------------------------------------


def _seed_blank_project(tmp_path: Path) -> Path:
    """Materialise the ``blank`` fixture under ``tmp_path`` and add a plan."""
    project = tmp_path / "blank-project"
    _fixtures.materialise("blank", project)
    (project / "plan.md").write_text(
        "# Plan\n\nplaceholder for bootstrap_pipeline tests\n",
        encoding="utf-8",
    )
    return project


def _seed_existing_suite(tmp_path: Path) -> Path:
    """Materialise the ``existing-suite`` fixture under ``tmp_path``."""
    project = tmp_path / "existing-project"
    _fixtures.materialise("existing-suite", project)
    return project


def _seed_ambiguous_root(tmp_path: Path) -> Path:
    """Materialise the ``ambiguous-root`` fixture under ``tmp_path``."""
    project = tmp_path / "ambiguous-project"
    _fixtures.materialise("ambiguous-root", project)
    (project / "plan.md").write_text(
        "# Plan\n\nplaceholder for ambiguous-root test\n", encoding="utf-8"
    )
    return project


def _file_preset_text(relative_path: str) -> str:
    """Return the bytes of a fixture file used as a file preset."""
    return (FIXTURES_PROJECTS / relative_path).read_text(encoding="utf-8")


# Stub transcripts the fake ``subprocess.run`` replays for builtin
# resolution. The transcript mirrors the real ``ralph preset list
# --format json`` output described in plan E14: ``source`` is the
# hats-source id (``builtin:debug``), ``name`` is the template name.
_BUILTIN_PRESET_LIST: dict[str, object] = {
    "manifests": [
        {
            "name": "debug",
            "description": "Debug preset",
            "source": "builtin:debug",
            "tags": ["debug"],
        },
        {
            "name": "ce-executor-lite",
            "description": "Lite preset",
            "source": "builtin:ce-executor-lite",
            "tags": [],
        },
        {
            "name": "ce-executor-pipeline",
            "description": "Pipeline preset",
            "source": "builtin:ce-executor-pipeline",
            "tags": [],
        },
    ]
}


_BUILTIN_PRESET_SHOW: dict[str, str] = {
    "debug": (
        "name: debug\n"
        "cli:\n"
        "  backend: claude\n"
        "event_loop:\n"
        "  prompt: |\n"
        "    # debug prompt\n"
        "    Read the supplied plan and follow it end-to-end.\n"
        "  max_iterations: 8\n"
        "  max_runtime_seconds: 1800\n"
    ),
    "ce-executor-lite": (
        "name: ce-executor-lite\n"
        "cli:\n"
        "  backend: claude\n"
        "event_loop:\n"
        "  prompt: |\n"
        "    # lite prompt\n"
        "    Use this prompt as the agent instructions.\n"
        "  max_iterations: 4\n"
        "  max_runtime_seconds: 600\n"
    ),
    "ce-executor-pipeline": (
        "name: ce-executor-pipeline\n"
        "cli:\n"
        "  backend: claude\n"
        "event_loop:\n"
        "  prompt: |\n"
        "    # ce-executor-pipeline prompt\n"
        "    Read the supplied plan and follow it end-to-end.\n"
        "  max_iterations: 12\n"
        "  max_runtime_seconds: 7200\n"
    ),
}


def _builtin_resolver_runner(
    argv: list[str],
    timeout: object = None,
    capture_output: bool = False,
    text: bool = False,
) -> subprocess.CompletedProcess:
    """Return a fake ``subprocess.run`` reply for the builtin resolver path.

    Only the two argv shapes the pipeline's builtin resolver emits
    are honoured:

    * ``[binary, "preset", "list", "--format", "json"]``
    * ``[binary, "preset", "show", <name>, "--format", "yaml"]``

    Anything else raises ``AssertionError`` to surface unexpected
    resolution argv as a regression.
    """
    binary = argv[0]
    # The capability probe emits several --help / --version calls;
    # honour the minimum contract for the static stage.
    if argv[1:] == ["--version"]:
        return subprocess.CompletedProcess(
            args=argv,
            returncode=0,
            stdout="ralph 0.1.0-test",
            stderr="",
        )
    if argv[1:] == ["--help"]:
        return subprocess.CompletedProcess(
            args=argv, returncode=0, stdout="usage: ralph ...", stderr=""
        )
    if argv[1:] == ["--json", "--help"]:
        return subprocess.CompletedProcess(
            args=argv, returncode=0, stdout="usage: ralph --json ...", stderr=""
        )
    if len(argv) >= 3 and argv[1:3] == ["preset", "check"] and argv[-1] == "--help":
        return subprocess.CompletedProcess(
            args=argv, returncode=0, stdout="--strict", stderr=""
        )
    if len(argv) >= 2 and argv[1:2] == ["preflight"] and argv[-1] == "--help":
        return subprocess.CompletedProcess(
            args=argv, returncode=0, stdout="--strict", stderr=""
        )
    if len(argv) >= 2 and argv[1:2] == ["run"] and argv[-1] == "--help":
        return subprocess.CompletedProcess(
            args=argv, returncode=0, stdout="--dry-run", stderr=""
        )
    if len(argv) >= 4 and "preset" in argv and "check" in argv and argv[-1] == "--strict":
        return subprocess.CompletedProcess(
            args=argv, returncode=0, stdout="", stderr=""
        )
    if len(argv) >= 3 and "preflight" in argv and argv[-1] == "--strict":
        return subprocess.CompletedProcess(
            args=argv, returncode=0, stdout="", stderr=""
        )
    if len(argv) >= 3 and "run" in argv and "--dry-run" in argv:
        return subprocess.CompletedProcess(
            args=argv,
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
    # Smoke harness argv carries --max-iterations and --idle-timeout; the
    # fake binary reaches the bounded terminal (LOOP_COMPLETE) so a
    # trusted replay backend promotes the handoff to ``complete``.
    if "--max-iterations" in argv and "--idle-timeout" in argv:
        return subprocess.CompletedProcess(
            args=argv,
            returncode=0,
            stdout="plan.ready\nLOOP_COMPLETE\n",
            stderr="",
        )
    if len(argv) >= 5 and argv[1:3] == ["preset", "list"] and argv[3] == "--format":
        return subprocess.CompletedProcess(
            args=argv,
            returncode=0,
            stdout=json.dumps(_BUILTIN_PRESET_LIST),
            stderr="",
        )
    if (
        len(argv) >= 6
        and argv[1:3] == ["preset", "show"]
        and argv[-2] == "--format"
    ):
        # form: ``[binary, "preset", "show", <name>, "--format", "yaml"]``
        name = argv[3]
        body = _BUILTIN_PRESET_SHOW.get(name)
        if body is None:
            return subprocess.CompletedProcess(
                args=argv,
                returncode=2,
                stdout="",
                stderr=f"unknown template: {name}",
            )
        return subprocess.CompletedProcess(
            args=argv,
            returncode=0,
            stdout=body,
            stderr="",
        )
    raise AssertionError(
        f"builtin resolver runner: unexpected argv={list(argv)}"
    )


# ---------------------------------------------------------------------------
# B2 / R2 — Root / input blocker before any write
# ---------------------------------------------------------------------------


def test_pipeline_blocker_does_not_write_or_validate(tmp_path: Path) -> None:
    """B2: a root_ambiguous cwd short-circuits the pipeline before writes.

    No config / prompt / docs are produced and no static gate is run.
    The result must be ``blocked`` with the original ``root_ambiguous``
    audit code, plus an explicit next-action hint that the operator
    should reconcile the scope.
    """
    project = _seed_ambiguous_root(tmp_path)
    # Add a top-level plan.md so the input gate passes; the cwd we
    # pass to the pipeline is ``nested/`` so audit sees two competing
    # AGENTS.md scopes and rejects the run.
    cwd = project / "nested"
    captured: dict[str, object] = {"called": False}

    def _never_called_runner(*args, **kwargs):  # pragma: no cover - asserts below
        captured["called"] = True
        captured["argv"] = list(args[0]) if args else kwargs.get("args")
        raise AssertionError(
            "pipeline must not invoke subprocess after a blocking audit"
        )

    result = bootstrap_pipeline.run_pipeline(
        cwd=cwd,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=_never_called_runner,
    )

    assert result.level == "blocked"
    assert result.blocked is True
    assert result.stage == "audit"
    assert result.code == "root_ambiguous"
    assert result.config_path == ""
    assert result.prompt_path == ""
    assert result.files_created == ()
    assert result.files_updated == ()
    assert result.files_noop == ()
    assert result.validation_evidence == ()
    assert result.handoff_command == ""
    assert captured["called"] is False
    # No owned files written by the pipeline.
    assert not (project / "ralph.debug.yml").exists()
    assert not (project / "PROMPT.debug.md").exists()


def test_pipeline_missing_preset_blocker(tmp_path: Path) -> None:
    """Missing preset argument is a typed input blocker."""
    project = _seed_blank_project(tmp_path)
    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="",
        plan_path="plan.md",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )
    assert result.level == "blocked"
    assert result.stage == "audit"
    assert result.code == "input_missing_preset"


def test_pipeline_absolute_path_blocker(tmp_path: Path) -> None:
    """Absolute / escape plan paths are blocked at the input boundary."""
    project = _seed_blank_project(tmp_path)
    captured: dict[str, object] = {}

    def _fail_runner(*args, **kwargs):
        captured["called"] = True
        raise AssertionError(
            "absolute / escape plan paths must not trigger subprocess calls"
        )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="/etc/passwd",
        binary="ralph",
        runner=_fail_runner,
    )
    assert result.level == "blocked"
    assert result.code == "input_path_unsafe"
    assert captured.get("called") is not True


def test_pipeline_escape_path_blocker(tmp_path: Path) -> None:
    """``..`` escapes are blocked at the input boundary."""
    project = _seed_blank_project(tmp_path)
    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="../../etc/passwd",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )
    assert result.level == "blocked"
    assert result.code == "input_path_unsafe"


# ---------------------------------------------------------------------------
# B3 / R3 — Builtin resolution uses manifest source → template
# ---------------------------------------------------------------------------


def test_builtin_resolution_uses_source_then_template(tmp_path: Path) -> None:
    """B3: ``builtin:<id>`` is resolved via ``preset list`` source lookup,
    not by stripping the prefix.

    The fake runner only honours the ``preset list`` then
    ``preset show <template>`` sequence; a regression that invokes
    ``preset show builtin:debug`` would fail the fake runner.
    """
    project = _seed_blank_project(tmp_path)
    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )
    # The resolver must surface the resolved preset id and template.
    assert result.preset == "builtin:debug"
    assert result.resolved_preset is not None
    assert result.resolved_preset.preset_id == "builtin:debug"
    assert result.resolved_preset.source_kind == "builtin"
    assert result.resolved_preset.template_name == "debug"
    assert result.resolved_preset.backend == "claude"
    assert result.resolved_preset.max_iterations == 8
    assert result.resolved_preset.max_runtime_seconds == 1800
    assert result.resolved_preset.inline_prompt_present is True


def test_builtin_resolution_unknown_id_blocker(tmp_path: Path) -> None:
    """B4: an unknown builtin id is a typed blocker.

    The fake list transcript intentionally omits the requested id so
    the resolver must surface ``builtin_source_missing``.
    """
    project = _seed_blank_project(tmp_path)
    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:does-not-exist",
        plan_path="plan.md",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )
    assert result.level == "blocked"
    assert result.stage == "preset_resolution"
    assert result.code == "builtin_source_missing"


# ---------------------------------------------------------------------------
# B4 / R2 — File preset resolution + invalid YAML
# ---------------------------------------------------------------------------


def test_file_preset_resolution_loads_yaml(tmp_path: Path) -> None:
    """A repo-relative file preset is parsed without invoking subprocess."""
    project = _seed_blank_project(tmp_path)
    preset_path = "preset.yml"
    (project / preset_path).write_text(
        "name: file-preset\n"
        "cli:\n"
        "  backend: claude\n"
        "event_loop:\n"
        "  prompt: |\n"
        "    inline prompt\n"
        "  max_iterations: 6\n"
        "  max_runtime_seconds: 900\n",
        encoding="utf-8",
    )
    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset=preset_path,
        plan_path="plan.md",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )
    assert result.preset == preset_path
    assert result.resolved_preset is not None
    assert result.resolved_preset.source_kind == "file"
    assert result.resolved_preset.inline_prompt_present is True
    assert result.resolved_preset.backend == "claude"


def test_invalid_yaml_preset_blocker(tmp_path: Path) -> None:
    """B4: malformed preset YAML produces a typed blocker."""
    project = _seed_blank_project(tmp_path)
    (project / "broken.yml").write_text(
        "name: [\ninvalid\n",
        encoding="utf-8",
    )
    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="broken.yml",
        plan_path="plan.md",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )
    assert result.level == "blocked"
    assert result.stage == "preset_resolution"
    assert result.code == "preset_yaml_invalid"


def test_preset_without_prompt_blocker(tmp_path: Path) -> None:
    """B4: a preset with no inline prompt and no supplied plan/prompt
    blocks provisioning (the fallback path belongs to U2's no-plan
    template handling, not to U1's resolution blocker).

    The test materialises a preset whose ``event_loop`` omits the
    ``prompt`` key and the pipeline MUST NOT proceed past the
    resolver.
    """
    project = _seed_blank_project(tmp_path)
    (project / "no-prompt.yml").write_text(
        "name: no-prompt\n"
        "cli:\n"
        "  backend: claude\n"
        "event_loop:\n"
        "  max_iterations: 4\n"
        "  max_runtime_seconds: 600\n",
        encoding="utf-8",
    )
    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="no-prompt.yml",
        plan_path="plan.md",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )
    assert result.level == "blocked"
    assert result.code == "preset_prompt_missing"


# ---------------------------------------------------------------------------
# B5 / R4 — Owned artifacts are created on a fresh project
# ---------------------------------------------------------------------------


def test_pipeline_success_creates_owned_outputs(tmp_path: Path) -> None:
    """B5: a fresh blank project receives the preset-bound two-file
    suite plus managed agent-doc sections on a single ``run_pipeline``
    call.

    The fake runner only honours builtin resolution; no static
    gate is invoked here (that test belongs to U3).
    """
    project = _seed_blank_project(tmp_path)
    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )
    assert result.level in {"incomplete_static_only", "complete"}
    assert result.code == ""
    assert result.config_path == "ralph.debug.yml"
    assert result.prompt_path == "PROMPT.debug.md"
    assert (project / "ralph.debug.yml").is_file()
    assert (project / "PROMPT.debug.md").is_file()
    assert "ralph.debug.yml" in result.files_created
    assert "PROMPT.debug.md" in result.files_created
    config_text = (project / "ralph.debug.yml").read_text(encoding="utf-8")
    assert "event_loop:" in config_text
    assert "prompt_file: PROMPT.debug.md" in config_text
    assert "_bootstrap:" in config_text
    prompt_text = (project / "PROMPT.debug.md").read_text(encoding="utf-8")
    assert "debug prompt" in prompt_text

    # U2: the four provenance keys live individually under ``_bootstrap:``.
    loaded = yaml.safe_load(config_text)
    bootstrap = loaded["_bootstrap"]
    assert set(bootstrap) >= {
        "generator_version",
        "input_signature",
        "profile_sha256",
        "prompt_sha256",
    }
    assert bootstrap["generator_version"] == pipeline_suite.GENERATOR_VERSION
    assert len(bootstrap["input_signature"]) == 64
    assert len(bootstrap["profile_sha256"]) == 64
    assert len(bootstrap["prompt_sha256"]) == 64

    # U2: ``core.guardrails`` is populated (baseline + project overlay).
    guardrails = loaded["core"]["guardrails"]
    assert isinstance(guardrails, list) and guardrails
    for baseline in pipeline_suite.BASELINE_GUARDRAILS:
        assert baseline in guardrails

    # U2: AGENTS.md + CLAUDE.md are part of the same write batch and
    # each carries exactly one well-formed managed block.
    assert "AGENTS.md" in result.files_created
    assert "CLAUDE.md" in result.files_created
    for name in ("AGENTS.md", "CLAUDE.md"):
        assert (project / name).is_file()
        doc_text = (project / name).read_text(encoding="utf-8")
        assert doc_text.count("RALPH-BOOTSTRAP-START") == 1
        assert doc_text.count("RALPH-BOOTSTRAP-END") == 1
        parse = agent_docs.parse_managed_section(doc_text, DOCS_MARKER_ID)
        assert parse.kind == "Ok"
    agents_body, claude_body = _managed_bodies(project)
    assert agents_body == claude_body
    assert agents_body.strip()

    # Write boundaries: no legacy artifacts, no ``.ralph/`` in the
    # target project, no ``.bootstrap.tmp`` residue.
    assert not (project / "ralph.pipeline.yml").exists()
    assert not (project / "PROMPT.pipeline.md").exists()
    assert not (project / "ralph.bootstrap.yml").exists()
    assert not (project / ".ralph").exists()
    assert not list(project.rglob("*.bootstrap.tmp"))


def test_pipeline_second_run_is_noop(tmp_path: Path) -> None:
    """B6: a second invocation with identical inputs is a noop.

    The noop disposition covers the preset-bound suite AND the
    AGENTS.md / CLAUDE.md managed sections: nothing is rewritten and
    the on-disk doc bytes survive the second run verbatim.
    """
    project = _seed_blank_project(tmp_path)
    first = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )
    agents_before = (project / "AGENTS.md").read_text(encoding="utf-8")
    claude_before = (project / "CLAUDE.md").read_text(encoding="utf-8")
    second = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )
    assert first.level in {"incomplete_static_only", "complete"}
    assert second.level == first.level
    # Second run reports every owned artifact as noop rather than
    # recreating it: suite files plus both managed docs.
    assert second.files_created == ()
    assert second.files_updated == ()
    assert second.files_noop == (
        "ralph.debug.yml",
        "PROMPT.debug.md",
        "AGENTS.md",
        "CLAUDE.md",
    )
    assert (project / "AGENTS.md").read_text(encoding="utf-8") == agents_before
    assert (project / "CLAUDE.md").read_text(encoding="utf-8") == claude_before
    assert not list(project.rglob("*.bootstrap.tmp"))


def test_pipeline_conflicting_docs_blocker(tmp_path: Path) -> None:
    """U2: disagreeing AGENTS.md / CLAUDE.md managed bodies block the
    whole batch with ``sync_mirror_conflict`` before any write.

    The ``conflicting-docs`` fixture seeds the two mirrors with
    different managed bodies. The pipeline must classify the run as
    ``blocked`` and leave the target project byte-for-byte untouched:
    no suite files, no doc rewrite, no ``.bootstrap.tmp`` residue.
    """
    project = tmp_path / "conflicting-project"
    _fixtures.materialise("conflicting-docs", project)
    (project / "plan.md").write_text(
        "# Plan\n\nplaceholder for conflicting-docs test\n", encoding="utf-8"
    )
    agents_before = (project / "AGENTS.md").read_text(encoding="utf-8")
    claude_before = (project / "CLAUDE.md").read_text(encoding="utf-8")

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )

    assert result.level == "blocked"
    assert result.blocked is True
    assert result.code == "sync_mirror_conflict"
    assert result.files_created == ()
    assert result.files_updated == ()
    assert result.files_noop == ()
    # Nothing written: suite absent, docs preserved byte-for-byte.
    assert not (project / "ralph.debug.yml").exists()
    assert not (project / "PROMPT.debug.md").exists()
    assert (project / "AGENTS.md").read_text(encoding="utf-8") == agents_before
    assert (project / "CLAUDE.md").read_text(encoding="utf-8") == claude_before
    assert not list(project.rglob("*.bootstrap.tmp"))


def test_pipeline_dirty_tree_preserves_operator_files(tmp_path: Path) -> None:
    """U2: operator-owned files stay byte-identical through a batch.

    The ``dirty-tree`` fixture carries a Cargo.toml plus a hand-edited
    ``src/lib.rs``. The pipeline writes only its owned targets
    (``ralph.<stem>.yml``, ``PROMPT.<stem>.md``, the managed sections)
    and leaves every other file untouched; no ``.bootstrap.tmp``
    sibling survives the run.
    """
    project = tmp_path / "dirty-project"
    _fixtures.materialise("dirty-tree", project)
    (project / "plan.md").write_text(
        "# Plan\n\nplaceholder for dirty-tree test\n", encoding="utf-8"
    )
    cargo_before = (project / "Cargo.toml").read_bytes()
    lib_before = (project / "src" / "lib.rs").read_bytes()

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )

    assert result.blocked is False
    assert (project / "ralph.debug.yml").is_file()
    assert (project / "PROMPT.debug.md").is_file()
    # Operator-owned files are byte-for-byte identical after the run.
    assert (project / "Cargo.toml").read_bytes() == cargo_before
    assert (project / "src" / "lib.rs").read_bytes() == lib_before
    # The managed body carries the discovered rust verification commands.
    agents_text = (project / "AGENTS.md").read_text(encoding="utf-8")
    claude_text = (project / "CLAUDE.md").read_text(encoding="utf-8")
    for text in (agents_text, claude_text):
        assert "cargo clippy --workspace --all-targets -- -D warnings" in text
        assert "cargo test" in text
    assert not list(project.rglob("*.bootstrap.tmp"))


def test_pipeline_conflict_rolls_back(tmp_path: Path) -> None:
    """B7: ownership/provenance/mirror conflicts prevent any write.

    The fixture rolls a config that fails provenance verification
    (embedded hash does not match the actual bytes). The pipeline
    must classify the attempt as ``blocked`` and leave the target
    project untouched.
    """
    project = _seed_blank_project(tmp_path)
    # Tamper the on-disk preset-bound config BEFORE running pipeline:
    # inject a config whose embedded profile_sha256 does NOT match the
    # actual bytes. The pipeline will see this as a refresh conflict.
    config_path = project / "ralph.debug.yml"
    config_path.write_text(
        (
            "cli:\n"
            "  backend: claude\n"
            "event_loop:\n"
            "  prompt_file: PROMPT.debug.md\n"
            "  max_iterations: 8\n"
            "  max_runtime_seconds: 1800\n"
            "_bootstrap:\n"
            "  generator_version: '0.3.0'\n"
            "  input_signature: deadbeef\n"
            "  profile_sha256: 0000000000000000000000000000000000000000000000000000000000000000\n"
            "  prompt_sha256: 1111111111111111111111111111111111111111111111111111111111111111\n"
        ),
        encoding="utf-8",
    )
    (project / "PROMPT.debug.md").write_text(
        "# tampered\n", encoding="utf-8"
    )
    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )
    assert result.level == "blocked"
    assert result.stage == "reconcile"
    # Tampered config + tampered prompt is a typed blocker; either
    # ``provenance_corrupt`` (embedded profile_sha256 missing/malformed)
    # or ``owned_value_user_modified`` (hashes disagree with bytes).
    assert result.code in {"provenance_corrupt", "owned_value_user_modified"}
    # No half-written state: the conflicting config bytes are preserved.
    assert (project / "ralph.debug.yml").read_text(encoding="utf-8").startswith(
        "cli:"
    )


# ---------------------------------------------------------------------------
# B8 / R5 — Static validation stage order / evidence preservation
# ---------------------------------------------------------------------------


def _static_runner_factory(
    invocations: list[cli_probe.FakeInvocation],
) -> Callable[..., subprocess.CompletedProcess]:
    """Return a fake ``subprocess.run`` that replays ``invocations``.

    Mirrors :mod:`_probe_runner` semantics: argv must match exactly,
    each invocation is consumed on use. When the requested argv is not
    present in the fixture transcript, fall back to the builtin
    resolver runner (which handles ``preset list`` / ``preset show``
    and the capability probe's --help/--version calls).
    """

    queue = list(invocations)

    def _runner(argv, timeout=None, capture_output=False, text=False):
        requested = tuple(argv)
        for index, invocation in enumerate(queue):
            if invocation.argv_expected == requested:
                queue.pop(index)
                return subprocess.CompletedProcess(
                    args=requested,
                    returncode=invocation.exit_code,
                    stdout="".join(invocation.stdout_chunks),
                    stderr="".join(invocation.stderr_chunks),
                )
        # Fall back to the builtin-resolution / capability-probe runner.
        return _builtin_resolver_runner(list(requested), timeout=timeout,
                                        capture_output=capture_output, text=text)

    return _runner


def test_pipeline_static_stage_evidence_preserved(tmp_path: Path) -> None:
    """U3: when all four static stages pass, the handoff carries
    ``validation_evidence`` describing the static load outcome.

    The fake runner drives the ``debug-green`` cli fixture: capability →
    preset check → preflight → dry-run. The pipeline must record a
    4-tuple of stage decisions and surface the static-only handoff.
    """
    project = _seed_blank_project(tmp_path)
    invocations = cli_probe.load_fixture("debug-green")
    runner = _static_runner_factory(invocations)
    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=runner,
    )
    assert result.level in {"incomplete_static_only", "complete"}
    assert len(result.stage_decisions) == 4
    assert [d.stage for d in result.stage_decisions] == [
        "capability",
        "preset_check",
        "preflight",
        "dry_run",
    ]
    assert all(d.outcome == "ok" for d in result.stage_decisions)
    # Static load alone never promotes to complete; smoke is required.
    assert result.level == "incomplete_static_only"


def test_pipeline_static_blocker_short_circuits(tmp_path: Path) -> None:
    """A blocker in the capability stage produces typed ``blocked``.

    Subsequent stages are recorded as skipped with the standard
    ``blocked_unknown`` outcome and ``skipped:`` evidence. No
    smoke is run.
    """
    project = _seed_blank_project(tmp_path)
    invocations = cli_probe.load_fixture("debug-missing-flag")
    runner = _static_runner_factory(invocations)
    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=runner,
    )
    assert result.level == "blocked"
    assert result.stage == "static_validation"
    assert result.code == "blocked_cli"
    assert result.smoke_outcome is None
    # First stage is blocked; subsequent stages carry skipped evidence.
    assert len(result.stage_decisions) == 4
    assert result.stage_decisions[0].outcome == "blocked_cli"
    for decision in result.stage_decisions[1:]:
        assert decision.outcome == "blocked_unknown"


# ---------------------------------------------------------------------------
# U3 — Per-class static blocker gate over the full cli fixture corpus
# ---------------------------------------------------------------------------


# The helper-level cli fixture corpus was authored against the contract
# suite's literal source tokens (``_PIPELINE_KW``: ``ralph.pipeline.yml``
# / ``builtin:ce-executor-pipeline`` / ``PROMPT.pipeline.md``). The
# unified pipeline derives its artifact stem from the preset id
# (``builtin:ce-executor-pipeline`` → ``ralph.ce-executor-pipeline.yml``),
# so replaying the corpus through ``run_pipeline`` rebinds the fixture
# tokens onto the pipeline-derived artifacts. Only the "as requested"
# tokens are rebound: a dry-run stdout that reports a DIFFERENT prompt
# file (the source-mismatch fixture) keeps its mismatching value so the
# gate still classifies it.
_FIXTURE_SOURCE_TOKENS = {
    "config_path": "ralph.pipeline.yml",
    "prompt_file": "PROMPT.pipeline.md",
}


def _rebind_fixture_invocations(
    invocations: list[cli_probe.FakeInvocation],
    *,
    config_path: str,
    prompt_path: str,
) -> list[cli_probe.FakeInvocation]:
    """Rewrite the fixture's source tokens onto pipeline-derived tokens."""
    argv_map = {
        _FIXTURE_SOURCE_TOKENS["config_path"]: config_path,
        _FIXTURE_SOURCE_TOKENS["prompt_file"]: prompt_path,
    }
    stdout_requested = (
        f"Prompt file: {_FIXTURE_SOURCE_TOKENS['prompt_file']}"
    )
    stdout_rebound = f"Prompt file: {prompt_path}"
    rebound: list[cli_probe.FakeInvocation] = []
    for invocation in invocations:
        argv = tuple(argv_map.get(token, token) for token in invocation.argv_expected)
        stdout = tuple(
            chunk.replace(stdout_requested, stdout_rebound)
            for chunk in invocation.stdout_chunks
        )
        rebound.append(
            cli_probe.FakeInvocation(
                argv_expected=argv,
                stdout_chunks=stdout,
                stderr_chunks=invocation.stderr_chunks,
                exit_code=invocation.exit_code,
            )
        )
    return rebound


def _bound_fixture_runner(
    invocations: list[cli_probe.FakeInvocation],
    *,
    config_path: str,
    prompt_path: str,
    requested: list[tuple[str, ...]],
) -> tuple[Callable[..., subprocess.CompletedProcess], list[cli_probe.FakeInvocation]]:
    """Return ``(runner, remaining_queue)`` replaying the rebound corpus.

    The runner records every requested argv, raises loudly on any
    smoke-shaped argv (a static blocker must never spawn smoke), and
    replays the rebound fixture invocations on an exact-argv match —
    consuming each once. Only the builtin-resolution argv
    (``preset list`` / ``preset show``) may bypass the fixture replay;
    any other unmatched argv fails closed.
    """
    queue = _rebind_fixture_invocations(
        invocations, config_path=config_path, prompt_path=prompt_path
    )

    def _runner(argv, timeout=None, capture_output=False, text=False):
        argv = tuple(argv)
        if "--max-iterations" in argv and "--idle-timeout" in argv:
            raise AssertionError(
                f"static short-circuit violated: smoke argv requested: {list(argv)}"
            )
        requested.append(argv)
        for index, invocation in enumerate(queue):
            if invocation.argv_expected == argv:
                queue.pop(index)
                return subprocess.CompletedProcess(
                    args=argv,
                    returncode=invocation.exit_code,
                    stdout="".join(invocation.stdout_chunks),
                    stderr="".join(invocation.stderr_chunks),
                )
        if len(argv) >= 3 and argv[1] == "preset" and argv[2] in ("list", "show"):
            return _builtin_resolver_runner(
                list(argv), timeout=timeout, capture_output=capture_output, text=text
            )
        raise AssertionError(
            f"bound fixture runner: no recorded invocation for argv={list(argv)}"
        )

    return _runner, queue


def _stage_of_argv(argv: tuple[str, ...]) -> str:
    """Classify a requested argv into the pipeline stage that emits it."""
    if "--max-iterations" in argv and "--idle-timeout" in argv:
        return "smoke"
    if "preset" in argv and "list" in argv:
        return "preset_list"
    if "preset" in argv and "show" in argv:
        return "preset_show"
    if argv[1:] == ("--version",) or argv[-1] == "--help":
        return "capability"
    if "check" in argv and "--strict" in argv:
        return "preset_check"
    if "preflight" in argv and "--strict" in argv:
        return "preflight"
    if "--dry-run" in argv:
        return "dry_run"
    return "unknown"


_STATIC_STAGE_INDEX = {"capability": 0, "preset_check": 1, "preflight": 2, "dry_run": 3}

# One scenario per blocker class the fixture corpus encodes. The
# expected codes mirror the canonical classification locked by the
# helper-level contract suite (``-k cli_probe``).
_U3_BLOCKER_SCENARIOS = (
    {
        "fixture": "preset-strict-fail",
        "expected_code": "blocked_preset",
        "blocked_stage": "preset_check",
        "reason_token": "unknown preset id",
        "requested_stages": ["capability"] * 6 + ["preset_check"],
        "evidence": (
            "capability:ok",
            "preset_check:blocked_preset",
            "preflight:blocked_unknown",
            "dry_run:blocked_unknown",
        ),
    },
    {
        "fixture": "backend-missing",
        "expected_code": "blocked_backend",
        "blocked_stage": "preflight",
        "reason_token": "executable not found",
        "requested_stages": ["capability"] * 6 + ["preset_check", "preflight"],
        "evidence": (
            "capability:ok",
            "preset_check:ok",
            "preflight:blocked_backend",
            "dry_run:blocked_unknown",
        ),
    },
    {
        "fixture": "dry-run-source-mismatch",
        "expected_code": "blocked_command",
        "blocked_stage": "dry_run",
        "reason_token": "prompt_file",
        "requested_stages": ["capability"] * 6
        + ["preset_check", "preflight", "dry_run"],
        "evidence": (
            "capability:ok",
            "preset_check:ok",
            "preflight:ok",
            "dry_run:blocked_command",
        ),
    },
)


@pytest.mark.parametrize(
    "scenario",
    _U3_BLOCKER_SCENARIOS,
    ids=[s["fixture"] for s in _U3_BLOCKER_SCENARIOS],
)
def test_pipeline_static_blocker_classification(tmp_path: Path, scenario) -> None:
    """U3: each blocker class is classified at the pipeline level and
    short-circuits the gate — tail stages are recorded skipped, the
    executed evidence prefix is preserved, and smoke never runs even
    when an authorized smoke backend is supplied.
    """
    project = _seed_blank_project(tmp_path)
    invocations = cli_probe.load_fixture(scenario["fixture"])
    requested: list[tuple[str, ...]] = []
    runner, queue = _bound_fixture_runner(
        invocations,
        config_path="ralph.ce-executor-pipeline.yml",
        prompt_path="PROMPT.ce-executor-pipeline.md",
        requested=requested,
    )
    transcript = tmp_path / "transcripts"
    transcript.mkdir(parents=True, exist_ok=True)
    backend = smoke_runner.SafeBackend(
        name="replay",
        kind="content_fixed_replay",
        transcript_path=transcript,
    )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:ce-executor-pipeline",
        plan_path="plan.md",
        binary="ralph",
        runner=runner,
        smoke_backend=backend,
    )

    # Classification: the pipeline surfaces the fixture's blocker class.
    assert result.level == "blocked"
    assert result.blocked is True
    assert result.stage == "static_validation"
    assert result.code == scenario["expected_code"]
    assert scenario["reason_token"] in result.message
    assert result.config_path == "ralph.ce-executor-pipeline.yml"
    assert result.prompt_path == "PROMPT.ce-executor-pipeline.md"

    # Typed evidence: four decisions in strict stage order; the prefix
    # before the blocker is ok, the blocker row carries the class, the
    # tail is skipped.
    assert len(result.stage_decisions) == 4
    assert [d.stage for d in result.stage_decisions] == [
        "capability",
        "preset_check",
        "preflight",
        "dry_run",
    ]
    blocked_index = _STATIC_STAGE_INDEX[scenario["blocked_stage"]]
    for decision in result.stage_decisions[:blocked_index]:
        assert decision.outcome == "ok"
    blocker = result.stage_decisions[blocked_index]
    assert blocker.outcome == scenario["expected_code"]
    assert blocker.next_allowed_stage is None
    assert scenario["reason_token"] in blocker.blocked_reason
    for decision in result.stage_decisions[blocked_index + 1 :]:
        assert decision.outcome == "blocked_unknown"
        assert decision.next_allowed_stage is None
        assert decision.evidence
        assert decision.evidence[0].startswith("skipped:")

    # Evidence records the executed prefix (plus blocker and skip rows).
    assert result.validation_evidence == scenario["evidence"]

    # Proof of argv: every preserved decision carries the explicit
    # ``-c <config> -H <preset>`` sources; the dry-run argv additionally
    # carries the source tokens and never ``--strict``.
    for decision in result.stage_decisions:
        argv = decision.argv
        assert "-c" in argv and result.config_path in argv
        assert "-H" in argv and "builtin:ce-executor-pipeline" in argv
    dry_run_argv = result.stage_decisions[3].argv
    assert "--dry-run" in dry_run_argv
    assert "--prompt-file" in dry_run_argv
    assert "PROMPT.ce-executor-pipeline.md" in dry_run_argv
    assert "--plan" in dry_run_argv and "plan.md" in dry_run_argv
    assert "--strict" not in dry_run_argv

    # Strict order at the spawn level: resolution → capability ×6 → the
    # executed static prefix; no smoke argv is ever requested, and every
    # fixture invocation is consumed exactly once.
    assert [_stage_of_argv(argv) for argv in requested] == [
        "preset_list",
        "preset_show",
    ] + scenario["requested_stages"]
    assert result.smoke_outcome is None
    assert result.smoke_argv == ()
    assert result.smoke_failure_bucket is None
    assert queue == []


def test_pipeline_static_green_is_not_loop_closed(tmp_path: Path) -> None:
    """U3: a four-stage green run is ``incomplete_static_only`` — the
    handoff makes "static load passed; loop not closed" explicit and
    never presents a ready / loop-closed command.

    Driven by the ``debug-green`` corpus whose argv matches the
    pipeline-derived ``builtin:debug`` artifacts byte-for-byte, so the
    full fixture queue must be consumed without any fallback replay.
    """
    project = _seed_blank_project(tmp_path)
    invocations = cli_probe.load_fixture("debug-green")
    requested: list[tuple[str, ...]] = []
    runner, queue = _bound_fixture_runner(
        invocations,
        config_path="ralph.debug.yml",
        prompt_path="PROMPT.debug.md",
        requested=requested,
    )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=runner,
    )

    assert result.blocked is False
    assert result.level == "incomplete_static_only"
    assert result.smoke_outcome is None
    assert [d.outcome for d in result.stage_decisions] == ["ok"] * 4

    # The command is a candidate (plan supplied), never a ready command
    # and never the bare PLAN_PATH template.
    assert result.handoff_command.startswith(
        "[CANDIDATE - operator must run manually]"
    )
    assert "PLAN_PATH" not in result.handoff_command
    assert "-c ralph.debug.yml" in result.handoff_command
    assert "--plan plan.md" in result.handoff_command

    # The report states the static-load-only semantics explicitly.
    assert "Level: `incomplete_static_only`" in result.handoff_report
    assert "Static load passed" in result.handoff_report
    assert "loop has not been verified end-to-end" in result.handoff_report

    # The full fixture corpus was consumed byte-exactly, in order.
    assert queue == []
    assert [_stage_of_argv(argv) for argv in requested] == [
        "preset_list",
        "preset_show",
    ] + ["capability"] * 6 + ["preset_check", "preflight", "dry_run"]


def test_pipeline_static_green_rebound_fixture(tmp_path: Path) -> None:
    """U3: the pipeline-bound fixture shape (``ralph.pipeline.yml``
    source tokens) also reaches a static-only handoff once rebound onto
    the pipeline-derived artifacts — locking the dry-run argv contract
    (``--prompt-file`` / ``--plan``) and the as-requested stdout
    rewrite against the ``green`` corpus.
    """
    project = _seed_blank_project(tmp_path)
    invocations = cli_probe.load_fixture("green")
    requested: list[tuple[str, ...]] = []
    runner, queue = _bound_fixture_runner(
        invocations,
        config_path="ralph.ce-executor-pipeline.yml",
        prompt_path="PROMPT.ce-executor-pipeline.md",
        requested=requested,
    )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:ce-executor-pipeline",
        plan_path="plan.md",
        binary="ralph",
        runner=runner,
    )

    assert result.blocked is False
    assert result.level == "incomplete_static_only"
    assert [d.outcome for d in result.stage_decisions] == ["ok"] * 4
    assert result.smoke_outcome is None
    assert queue == []


# ---------------------------------------------------------------------------
# B9-B11 / R6 — Smoke authorization + typed handoff
# ---------------------------------------------------------------------------


def _transcript_dir(tmp_path: Path) -> Path:
    return tmp_path / "transcripts"


def test_pipeline_replay_smoke_terminal_promotes_to_complete(tmp_path: Path) -> None:
    """Replay-bounded smoke that reaches ``LOOP_COMPLETE`` advances the
    handoff level to ``complete`` and emits the official command.

    The pipeline constructs a ``SafeBackend`` from the
    ``content_fixed_replay`` kind; the fake transcript is staged by
    the helper when the operator supplies ``--replay-transcript``
    later, so this test injects a pre-built transcript directly into
    the smoke harness.
    """
    project = _seed_blank_project(tmp_path)
    invocations = cli_probe.load_fixture("green")
    runner = _static_runner_factory(invocations)

    transcript = _transcript_dir(tmp_path)
    transcript.mkdir(parents=True, exist_ok=True)
    backend = smoke_runner.SafeBackend(
        name="replay",
        kind="content_fixed_replay",
        transcript_path=transcript,
    )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=runner,
        smoke_backend=backend,
    )
    assert result.level == "complete"
    assert result.smoke_outcome == "bounded_terminal_reached"
    assert result.handoff_command
    assert "ralph -c ralph.debug.yml" in result.handoff_command
    assert "-H builtin:debug" in result.handoff_command
    assert "[CANDIDATE" not in result.handoff_command
    assert "PLAN_PATH" not in result.handoff_command


def test_pipeline_unsafe_backend_no_spawn(tmp_path: Path) -> None:
    """U4: an unsafe backend never causes a subprocess to spawn.

    The pipeline must return ``incomplete_static_only`` because the
    typed outcome is ``not_authorized``. The ``smoke_outcome`` is
    the literal refusal; the smoke argv tuple MUST be empty.
    """
    project = _seed_blank_project(tmp_path)
    invocations = cli_probe.load_fixture("green")
    runner = _static_runner_factory(invocations)
    backend = smoke_runner.UnsafeBackend(name="mock")
    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=runner,
        smoke_backend=backend,
    )
    assert result.level == "incomplete_static_only"
    assert result.smoke_outcome == "not_authorized"
    assert result.smoke_argv == ()
    assert "[CANDIDATE" in result.handoff_command


def test_pipeline_typed_smoke_failure_blocks(tmp_path: Path) -> None:
    """A typed smoke failure (``timeout_no_event``) blocks the handoff."""
    project = _seed_blank_project(tmp_path)
    invocations = cli_probe.load_fixture("green")
    runner = _static_runner_factory(invocations)
    backend = smoke_runner.SafeBackend(
        name="replay", kind="content_fixed_replay"
    )
    # Inject a smoke result that classifies as a blocked outcome.
    fake_result = smoke_runner.SmokeResult(
        outcome="timeout_no_event",
        evidence=("idle timeout elapsed",),
        argv=("ralph", "-c", "ralph.debug.yml", "-H", "builtin:debug"),
        stderr_excerpt="",
        stdout_excerpt="",
        elapsed_seconds=30.0,
        failure_bucket="suite",
    )
    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=runner,
        smoke_backend=backend,
        smoke_result_override=fake_result,
    )
    assert result.level == "blocked"
    assert result.smoke_outcome == "timeout_no_event"
    assert result.handoff_command == ""


# ---------------------------------------------------------------------------
# B12 / R7 — CLI / JSON parity
# ---------------------------------------------------------------------------


def test_pipeline_cli_json_matches_result(tmp_path: Path) -> None:
    """The CLI ``--json`` output must equal ``PipelineResult.to_json()``.

    Default text mode prints the structured fields with the same
    provenance. Exit code is 0 for provisioning success, 2 for
    blocked.
    """
    project = _seed_blank_project(tmp_path)
    invocations = cli_probe.load_fixture("green")
    runner = _static_runner_factory(invocations)

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=runner,
    )
    cli_json = bootstrap_pipeline.render_cli_json(result)
    assert json.loads(cli_json) == json.loads(result.to_json())


# ---------------------------------------------------------------------------
# U5 — Skill install parity
# ---------------------------------------------------------------------------


def test_pipeline_skill_files_canonical() -> None:
    """U5: the public skill description references the unified entry
    point and its three handoff levels."""
    skill_md = (ROOT / "skills" / "ralph-project-bootstrap" / "SKILL.md").read_text(
        encoding="utf-8"
    )
    openai_yaml = (ROOT / "skills" / "ralph-project-bootstrap" / "agents" / "openai.yaml").read_text(
        encoding="utf-8"
    )
    # Unified entry point must be discoverable from operator docs.
    assert "bootstrap_pipeline" in skill_md or "run_pipeline" in skill_md
    assert "bootstrap_pipeline" in openai_yaml or "run_pipeline" in openai_yaml