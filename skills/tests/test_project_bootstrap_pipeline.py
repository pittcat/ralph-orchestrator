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
import sys
from pathlib import Path
from typing import Callable

import pytest
import yaml

import _fixtures
import agent_docs
import audit
import bootstrap_pipeline
import cli_probe
import handoff
import install  # type: ignore[import-not-found]  # added via conftest sys.path
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


# A complete, valid file preset used by the unsafe-preset gate tests.
# The file EXISTS on disk wherever the test plants it, so any pipeline
# run that fails to block at the input boundary would read it, parse
# it and proceed past the audit stage — the typed blocker is the proof
# that no filesystem read of the path happened.
_VALID_FILE_PRESET_YAML = (
    "name: outside-preset\n"
    "cli:\n"
    "  backend: claude\n"
    "event_loop:\n"
    "  prompt: |\n"
    "    inline prompt\n"
    "  max_iterations: 5\n"
    "  max_runtime_seconds: 600\n"
)


# Stub transcripts the fake ``subprocess.run`` replays for builtin
# resolution. Mirrors the ``ralph preset builtin list --format json``
# envelope produced by U01: ``{presets: [{id, source, description,
# public}]}``; every ``source`` is strictly ``builtin:<id>`` and
# ``public`` reflects whether the preset surfaces via ``list``.
_BUILTIN_PRESET_LIST: dict[str, object] = {
    "presets": [
        {
            "id": "debug",
            "description": "Debug preset",
            "source": "builtin:debug",
            "public": True,
        },
        {
            "id": "ce-executor-lite",
            "description": "Lite preset",
            "source": "builtin:ce-executor-lite",
            "public": True,
        },
        {
            "id": "ce-executor-pipeline",
            "description": "Pipeline preset",
            "source": "builtin:ce-executor-pipeline",
            "public": True,
        },
        {
            "id": "replay-demo",
            "description": "Replay-safe demo preset",
            "source": "builtin:replay-demo",
            "public": True,
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
    # The ONLY builtin stub whose resolved ``cli.backend`` equals the
    # smoke harness's auto-authorised kind; the corrected authorization
    # model promotes replay smoke positives through this preset.
    "replay-demo": (
        "name: replay-demo\n"
        "cli:\n"
        "  backend: content_fixed_replay\n"
        "event_loop:\n"
        "  prompt: |\n"
        "    # replay prompt\n"
        "    Replay the fixed transcript end-to-end.\n"
        "  max_iterations: 3\n"
        "  max_runtime_seconds: 300\n"
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
    are honoured (U03 migration):

    * ``[binary, "preset", "builtin", "list", "--format", "json"]``
    * ``[binary, "preset", "builtin", "show", <id>, "--format", "yaml"]``

    The legacy argv shapes (``preset list`` / ``preset show``) are NOT
    honoured — a regression that invokes the old template-based path
    fails the fake runner so the resolver cannot silently fall back.

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
        # Echo the prompt file the caller requested so the dry-run
        # effective-value gate matches for EVERY preset stem (the
        # pipeline forwards ``--prompt-file PROMPT.<stem>.md``); fall
        # back to the historical debug token when the argv carries none.
        prompt_file = "PROMPT.debug.md"
        if "--prompt-file" in argv:
            prompt_file = argv[argv.index("--prompt-file") + 1]
        return subprocess.CompletedProcess(
            args=argv,
            returncode=0,
            stdout=(
                "Dry run mode - configuration:\n"
                f"  Prompt file: {prompt_file}\n"
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
    if (
        len(argv) >= 6
        and argv[1:4] == ["preset", "builtin", "list"]
        and argv[4] == "--format"
    ):
        return subprocess.CompletedProcess(
            args=argv,
            returncode=0,
            stdout=json.dumps(_BUILTIN_PRESET_LIST),
            stderr="",
        )
    if (
        len(argv) >= 7
        and argv[1:4] == ["preset", "builtin", "show"]
        and argv[-2] == "--format"
    ):
        # form: ``[binary, "preset", "builtin", "show", <id>, "--format", "yaml"]``
        builtin_id = argv[4]
        body = _BUILTIN_PRESET_SHOW.get(builtin_id)
        if body is None:
            return subprocess.CompletedProcess(
                args=argv,
                returncode=2,
                stdout="",
                stderr=f"unknown builtin preset: {builtin_id}",
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
# U1-fix — File presets pass the SAME repo-relative input gate as plan/prompt
# ---------------------------------------------------------------------------


def test_pipeline_absolute_preset_path_blocker(tmp_path: Path) -> None:
    """U1-fix: an absolute preset path is blocked at the input boundary.

    Mirrors the plan-path gate. The outside file EXISTS and carries a
    valid preset YAML, so any filesystem read of it would promote the
    run past the audit stage — the typed blocker proves no read
    happened, no subprocess was spawned and nothing was written.
    """
    project = _seed_blank_project(tmp_path)
    outside = tmp_path / "outside.yml"
    outside.write_text(_VALID_FILE_PRESET_YAML, encoding="utf-8")
    captured: dict[str, object] = {"called": False}

    def _fail_runner(*args, **kwargs):  # pragma: no cover - asserts below
        captured["called"] = True
        raise AssertionError(
            "unsafe preset paths must not trigger subprocess calls"
        )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset=str(outside),
        plan_path="plan.md",
        binary="ralph",
        runner=_fail_runner,
    )
    assert result.level == "blocked"
    assert result.blocked is True
    assert result.stage == "audit"
    assert result.code == "input_path_unsafe"
    assert captured["called"] is False
    # Blocked before resolution / generation: nothing resolved or written.
    assert result.resolved_preset is None
    assert result.files_created == ()
    assert not list(project.glob("ralph.*.yml"))


def test_pipeline_escape_preset_path_blocker(tmp_path: Path) -> None:
    """U1-fix: a ``..``-escaping preset is blocked at the input boundary.

    The escape target exists one level above the project with valid
    preset YAML, so a run that reached the YAML parse / generation /
    static stages would have read it. The typed blocker stage/code plus
    zero runner calls prove the run stopped before all of them.
    """
    project = _seed_blank_project(tmp_path)
    (tmp_path / "outside.yml").write_text(
        _VALID_FILE_PRESET_YAML, encoding="utf-8"
    )
    captured: dict[str, object] = {"called": False}

    def _fail_runner(*args, **kwargs):  # pragma: no cover - asserts below
        captured["called"] = True
        raise AssertionError(
            "escape preset paths must not trigger subprocess calls"
        )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="../outside.yml",
        plan_path="plan.md",
        binary="ralph",
        runner=_fail_runner,
    )
    assert result.level == "blocked"
    assert result.blocked is True
    assert result.stage == "audit"
    assert result.code == "input_path_unsafe"
    assert captured["called"] is False
    assert result.resolved_preset is None
    assert result.files_created == ()
    # Never reaches generation: no owned artifacts in the project.
    assert not list(project.glob("ralph.*.yml"))
    assert not list(project.glob("PROMPT.*.md"))


def test_pipeline_control_byte_preset_blocker(tmp_path: Path) -> None:
    """U1-fix: presets carrying C0 control bytes are rejected by the
    same lexical gate before any filesystem call."""
    project = _seed_blank_project(tmp_path)
    captured: dict[str, object] = {"called": False}

    def _fail_runner(*args, **kwargs):  # pragma: no cover - asserts below
        captured["called"] = True
        raise AssertionError(
            "control-byte preset paths must not trigger subprocess calls"
        )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="evil\x00preset.yml",
        plan_path="plan.md",
        binary="ralph",
        runner=_fail_runner,
    )
    assert result.level == "blocked"
    assert result.stage == "audit"
    assert result.code == "input_path_unsafe"
    assert captured["called"] is False
    assert result.resolved_preset is None


def test_audit_rejects_unsafe_preset_path_directly(tmp_path: Path) -> None:
    """U1-fix (defense in depth): ``audit.run_audit`` refuses a preset
    that is not a safe repo-relative token BEFORE any existence probe,
    so direct audit callers share the pipeline's input gate.

    The absolute target exists, so a bare existence check would
    silently pass — the typed ``input_path_unsafe`` issue proves the
    lexical gate fired first.
    """
    project = _seed_blank_project(tmp_path)
    outside = tmp_path / "outside.yml"
    outside.write_text(_VALID_FILE_PRESET_YAML, encoding="utf-8")
    decision = audit.run_audit(project, preset=str(outside), plan_path="plan.md")
    assert decision.is_blocking
    codes = {issue.code for issue in decision.issues}
    assert "input_path_unsafe" in codes


# ---------------------------------------------------------------------------
# U1-fix — Audit existence check and preset resolution share ONE anchor
# ---------------------------------------------------------------------------


def test_pipeline_file_preset_resolves_at_audit_root_from_subdirectory(
    tmp_path: Path,
) -> None:
    """U1-fix: audit says "exists at root R" ⇒ resolution reads from R.

    The project carries a vcs root marker at the top level while the
    pipeline is invoked from a nested subdirectory, so the audit root
    is the project root (``..`` relative to the bare cwd). The preset
    lives ONLY at that audit root; the resolver must read it from the
    audit-resolved root instead of re-anchoring on the bare cwd (which
    would surface a spurious ``input_missing_preset_file``).
    """
    project = tmp_path / "anchor-project"
    _fixtures.materialise("blank", project)
    (project / ".git").mkdir()
    (project / "debug.yml").write_text(_VALID_FILE_PRESET_YAML, encoding="utf-8")
    cwd = project / "nested"
    cwd.mkdir()

    result = bootstrap_pipeline.run_pipeline(
        cwd=cwd,
        preset="debug.yml",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )
    # Resolution succeeds at the audit root; the run reaches the static
    # stage (green with the fake runner) instead of a resolution blocker.
    assert result.blocked is False
    assert result.level == "incomplete_static_only"
    assert result.root == ".."
    assert result.preset == "debug.yml"
    assert result.resolved_preset is not None
    assert result.resolved_preset.source_kind == "file"
    assert result.resolved_preset.template_name == "debug"
    assert result.resolved_preset.backend == "claude"
    assert result.resolved_preset.inline_prompt_present is True


def test_pipeline_file_preset_only_under_bare_cwd_is_rejected(
    tmp_path: Path,
) -> None:
    """U1-fix: canonical anchor direction — a preset present ONLY under
    the bare cwd but missing at the audit root is rejected.

    The audit root (vcs root at the project top level) is the single
    anchor for BOTH the existence check and the resolution read; a
    preset that exists only in the nested cwd is outside that root and
    must be refused by the audit stage — the resolver never falls back
    to the bare cwd to pick it up.
    """
    project = tmp_path / "anchor-project"
    _fixtures.materialise("blank", project)
    (project / ".git").mkdir()
    cwd = project / "nested"
    cwd.mkdir()
    (cwd / "only-here.yml").write_text(_VALID_FILE_PRESET_YAML, encoding="utf-8")

    result = bootstrap_pipeline.run_pipeline(
        cwd=cwd,
        preset="only-here.yml",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )
    assert result.level == "blocked"
    assert result.stage == "audit"
    assert result.code == "input_missing_preset_file"
    assert result.resolved_preset is None
    assert result.files_created == ()


# ---------------------------------------------------------------------------
# B3 / R3 — Builtin resolution uses manifest source → template
# ---------------------------------------------------------------------------


def test_builtin_resolution_uses_builtin_id_and_show(tmp_path: Path) -> None:
    """S6 (U03): ``builtin:<id>`` is resolved via ``preset builtin list``
    → ``preset builtin show <id>`` with the resolved text byte-for-byte
    equal to the show stdout.

    The fake runner only honours the new ``preset builtin list`` then
    ``preset builtin show <id>`` sequence; a regression that invokes
    the old ``preset list`` / ``preset show <template-name>`` path
    fails the fake runner. The fake runner's argv gate also rejects
    ``preset show builtin:<id>`` — the resolver must never use the
    builtin-prefixed id as a show argument (S7 contract).
    """
    project = _seed_blank_project(tmp_path)
    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )
    # The resolver must surface the resolved preset id; the show stdout
    # is fed straight into the YAML parser, so the resolver's
    # ``text`` field is byte-for-byte equal to the fixture body.
    assert result.preset == "builtin:debug"
    assert result.resolved_preset is not None
    assert result.resolved_preset.preset_id == "builtin:debug"
    assert result.resolved_preset.source_kind == "builtin"
    # The builtin ID is the provenance label — the legacy
    # ``template_name`` slot was sourced from the old
    # ``manifests[i].name`` field; U03 deliberately collapses the two
    # so a future template manifest rename cannot misroute the
    # resolver.
    assert result.resolved_preset.template_name == "debug"
    assert result.resolved_preset.backend == "claude"
    assert result.resolved_preset.max_iterations == 8
    assert result.resolved_preset.max_runtime_seconds == 1800
    assert result.resolved_preset.inline_prompt_present is True
    # The full YAML body lands in ``text`` (no template placeholders).
    assert "name: debug" in result.resolved_preset.text
    assert "backend: claude" in result.resolved_preset.text


def test_builtin_resolution_does_not_use_template_alias(tmp_path: Path) -> None:
    """S7 (U03): the resolver never falls back to a template name when
    the builtin list contains the canonical id.

    Both ``ce-executor-pipeline`` and ``ce-executor-lite`` appear in
    the fake list. A regression that looked up the template alias
    (``ce-executor-lite``) and called ``preset show ce-executor-lite``
    would fail the fake runner — only the builtin ID lookup path
    must produce the show call.
    """
    project = _seed_blank_project(tmp_path)
    captured: list[tuple[str, ...]] = []

    def _recording(argv, timeout=None, capture_output=False, text=False):
        captured.append(tuple(argv))
        return _builtin_resolver_runner(
            list(argv), timeout=timeout, capture_output=capture_output, text=text
        )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:ce-executor-pipeline",
        plan_path="plan.md",
        binary="ralph",
        runner=_recording,
    )
    assert result.level in {"incomplete_static_only", "complete"}
    assert result.resolved_preset is not None
    assert result.resolved_preset.preset_id == "builtin:ce-executor-pipeline"
    # The fake binary records EVERY argv it sees; the show argv must
    # carry the builtin id, NOT a template alias or the bare id strip.
    show_argvs = [
        argv
        for argv in captured
        if len(argv) >= 7 and argv[1:4] == ("preset", "builtin", "show")
    ]
    assert show_argvs, "resolver must invoke preset builtin show exactly once"
    show_argv = show_argvs[0]
    assert show_argv[4] == "ce-executor-pipeline"
    # Never the template alias.
    assert "ce-executor-lite" not in show_argv
    # Never the builtin-prefixed id (the old buggy shape).
    assert "builtin:ce-executor-pipeline" not in show_argv


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
# U03 (S8-S10) — Builtin resolver fault-injection: list / show failures
# ---------------------------------------------------------------------------


def test_builtin_list_unparseable_blocks_before_show_or_write(tmp_path: Path) -> None:
    """S8 (U03): a malformed ``preset builtin list`` envelope blocks at
    ``preset_resolution`` with ``builtin_list_unparseable`` BEFORE any
    show call or owned-artifact write.

    The fake runner records every argv it sees. A regression that
    parsed the bad-JSON path and then issued a show call would surface
    here: the captured argv list contains a ``preset builtin show``
    invocation, proving the resolver short-circuited the failure
    branch.
    """
    project = _seed_blank_project(tmp_path)
    captured: list[tuple[str, ...]] = []

    def _bad_json_runner(argv, timeout=None, capture_output=False, text=False):
        captured.append(tuple(argv))
        if (
            len(argv) >= 6
            and argv[1:4] == ["preset", "builtin", "list"]
            and argv[4] == "--format"
        ):
            return subprocess.CompletedProcess(
                args=argv, returncode=0, stdout="{this is not valid json", stderr=""
            )
        # Show / capability / smoke paths fall through to the standard
        # fake runner; the test asserts NO show call ever happens, so
        # an unhandled argv is an AssertionError guard.
        return _builtin_resolver_runner(
            list(argv), timeout=timeout, capture_output=capture_output, text=text
        )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=_bad_json_runner,
    )
    assert result.level == "blocked"
    assert result.stage == "preset_resolution"
    assert result.code == "builtin_list_unparseable"
    # Show never runs.
    assert not any(
        len(argv) >= 7 and argv[1:4] == ("preset", "builtin", "show")
        for argv in captured
    )
    # No owned artifacts.
    assert result.files_created == ()
    assert result.files_updated == ()
    assert not list(project.glob("ralph.*.yml"))
    assert not list(project.glob("PROMPT.*.md"))


def test_builtin_list_failed_blocks_without_template_fallback(tmp_path: Path) -> None:
    """S8 (U03): ``preset builtin list`` returning non-zero blocks the
    resolver with ``builtin_list_failed`` — the resolver MUST NOT
    fall back to the legacy ``preset list`` argv on a non-zero exit.
    """
    project = _seed_blank_project(tmp_path)
    captured: list[tuple[str, ...]] = []

    def _failing_list_runner(argv, timeout=None, capture_output=False, text=False):
        captured.append(tuple(argv))
        if (
            len(argv) >= 6
            and argv[1:4] == ["preset", "builtin", "list"]
            and argv[4] == "--format"
        ):
            return subprocess.CompletedProcess(
                args=argv,
                returncode=3,
                stdout="",
                stderr="cli unavailable",
            )
        return _builtin_resolver_runner(
            list(argv), timeout=timeout, capture_output=capture_output, text=text
        )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=_failing_list_runner,
    )
    assert result.level == "blocked"
    assert result.stage == "preset_resolution"
    assert result.code == "builtin_list_failed"
    # Show never runs (no fallback to legacy argv).
    assert not any(
        len(argv) >= 7 and argv[1:4] == ["preset", "builtin", "show"]
        for argv in captured
    )
    # Legacy argv shape MUST NOT appear.
    assert not any(
        len(argv) >= 5 and argv[1:3] == ("preset", "list") for argv in captured
    )
    assert not any(
        len(argv) >= 6 and argv[1:3] == ("preset", "show") for argv in captured
    )
    assert result.files_created == ()


def test_builtin_show_failed_blocks_before_write(tmp_path: Path) -> None:
    """S9 (U03): ``preset builtin show <id>`` returning non-zero blocks
    with ``builtin_show_failed`` BEFORE any write — the resolver MUST
    NOT swallow the exit code.
    """
    project = _seed_blank_project(tmp_path)
    captured: list[tuple[str, ...]] = []

    def _failing_show_runner(argv, timeout=None, capture_output=False, text=False):
        captured.append(tuple(argv))
        if (
            len(argv) >= 7
            and argv[1:4] == ["preset", "builtin", "show"]
            and argv[-2] == "--format"
        ):
            return subprocess.CompletedProcess(
                args=argv,
                returncode=2,
                stdout="",
                stderr="boom",
            )
        return _builtin_resolver_runner(
            list(argv), timeout=timeout, capture_output=capture_output, text=text
        )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=_failing_show_runner,
    )
    assert result.level == "blocked"
    assert result.stage == "preset_resolution"
    assert result.code == "builtin_show_failed"
    assert "debug" in result.message
    assert "2" in result.message
    # No owned artifacts.
    assert result.files_created == ()
    assert result.files_updated == ()
    assert not list(project.glob("ralph.*.yml"))
    assert not list(project.glob("PROMPT.*.md"))


def test_builtin_show_empty_blocks_before_write(tmp_path: Path) -> None:
    """S10 (U03): ``preset builtin show <id>`` returning empty stdout
    blocks with ``builtin_show_empty`` BEFORE any write — empty body
    is a typed failure, not a defaulting path.
    """
    project = _seed_blank_project(tmp_path)
    captured: list[tuple[str, ...]] = []

    def _empty_show_runner(argv, timeout=None, capture_output=False, text=False):
        captured.append(tuple(argv))
        if (
            len(argv) >= 7
            and argv[1:4] == ["preset", "builtin", "show"]
            and argv[-2] == "--format"
        ):
            return subprocess.CompletedProcess(
                args=argv, returncode=0, stdout="", stderr=""
            )
        return _builtin_resolver_runner(
            list(argv), timeout=timeout, capture_output=capture_output, text=text
        )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=_empty_show_runner,
    )
    assert result.level == "blocked"
    assert result.stage == "preset_resolution"
    assert result.code == "builtin_show_empty"
    assert "debug" in result.message
    assert result.files_created == ()
    assert result.files_updated == ()
    assert not list(project.glob("ralph.*.yml"))
    assert not list(project.glob("PROMPT.*.md"))


def test_builtin_list_envelope_rejects_old_manifests_shape(tmp_path: Path) -> None:
    """S8 (U03): the new resolver MUST reject the legacy
    ``{manifests: [...]}`` envelope — accepting it would leak
    template-data-source semantics into the builtin resolver path.

    The fake runner emits the legacy ``manifests`` shape; the resolver
    surfaces ``builtin_list_unparseable`` instead of silently using a
    template-name lookup.
    """
    project = _seed_blank_project(tmp_path)

    def _legacy_envelope_runner(argv, timeout=None, capture_output=False, text=False):
        if (
            len(argv) >= 6
            and argv[1:4] == ["preset", "builtin", "list"]
            and argv[4] == "--format"
        ):
            legacy = {
                "manifests": [
                    {
                        "name": "debug",
                        "description": "Debug preset",
                        "source": "builtin:debug",
                    }
                ]
            }
            return subprocess.CompletedProcess(
                args=argv, returncode=0, stdout=json.dumps(legacy), stderr=""
            )
        return _builtin_resolver_runner(
            list(argv), timeout=timeout, capture_output=capture_output, text=text
        )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=_legacy_envelope_runner,
    )
    assert result.level == "blocked"
    assert result.stage == "preset_resolution"
    assert result.code == "builtin_list_unparseable"
    assert result.files_created == ()


def test_file_preset_resolution_does_not_call_subprocess(tmp_path: Path) -> None:
    """U03 (file preset): a file preset path MUST NOT invoke any
    builtin-resolution subprocess — the file read is the only IO and
    the parser feeds ``_derive_runtime_fields`` directly.

    This is the file-preset side of U03: the resolver's two paths are
    ``builtin`` (subprocess) and ``file`` (no subprocess); the guard
    below proves the migration did not accidentally route the file
    path through ``preset builtin list`` or ``preset builtin show``.

    Capability / static / smoke argv are deliberately allowed through
    — those belong to the post-resolution stages and are exercised by
    other tests.
    """
    project = _seed_blank_project(tmp_path)
    captured: list[tuple[str, ...]] = []

    def _guard_builtin_only(argv, *args, **kwargs):
        captured.append(tuple(argv))
        if len(argv) >= 6 and argv[1:4] == ["preset", "builtin", "list"]:
            raise AssertionError(
                f"file preset path must not invoke builtin list: argv={list(argv)}"
            )
        if len(argv) >= 7 and argv[1:4] == ["preset", "builtin", "show"]:
            raise AssertionError(
                f"file preset path must not invoke builtin show: argv={list(argv)}"
            )
        return _builtin_resolver_runner(list(argv), *args, **kwargs)

    preset_path = "demo-file-preset.yml"
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
        runner=_guard_builtin_only,
    )
    assert result.resolved_preset is not None
    assert result.resolved_preset.source_kind == "file"
    assert result.resolved_preset.template_name == "demo-file-preset"
    # Belt-and-braces: explicitly confirm no builtin-resolution argv
    # ever reached the runner.
    assert not any(
        len(argv) >= 6 and argv[1:4] == ("preset", "builtin", "list")
        for argv in captured
    )
    assert not any(
        len(argv) >= 7 and argv[1:4] == ("preset", "builtin", "show")
        for argv in captured
    )


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
# U2-fix — B4: unreadable / non-UTF-8 inputs normalize to typed blockers
# ---------------------------------------------------------------------------
#
# Contract pinned here: every IO/decode failure while reading the preset
# file or pre-existing owned artifacts becomes a TYPED blocker — blocked
# level, locatable code, CLI exit 2, zero disk writes before the block,
# no bare traceback and no misattribution to the handoff catch-all.
# Non-UTF-8 preset bytes pin plan requirement B4 ("preset file 不可读 →
# blocked + 可定位错误码") which previously had neither implementation
# nor tests.


def test_pipeline_non_utf8_preset_file_blocker(tmp_path: Path) -> None:
    """B4 (U2): a preset file carrying invalid UTF-8 bytes blocks at
    ``preset_resolution`` with the typed ``preset_yaml_invalid`` code.

    The file EXISTS and passes the ``is_file()`` short-circuit, so the
    typed blocker proves the decode failure was normalized instead of
    leaking as an unpacked ``UnicodeDecodeError``. Nothing is written
    and no subprocess is spawned.
    """
    project = _seed_blank_project(tmp_path)
    (project / "binary.yml").write_bytes(
        b"\xff\xfe\x00binary\nname: not-decodable\n"
    )
    captured: dict[str, object] = {"called": False}

    def _never_called_runner(*args, **kwargs):  # pragma: no cover - asserts below
        captured["called"] = True
        raise AssertionError(
            "non-decodable preset files must not trigger subprocess calls"
        )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="binary.yml",
        plan_path="plan.md",
        binary="ralph",
        runner=_never_called_runner,
    )

    assert result.level == "blocked"
    assert result.blocked is True
    assert result.stage == "preset_resolution"
    assert result.code == "preset_yaml_invalid"
    # The message stays informative: it names the offending preset token.
    assert "binary.yml" in result.message
    assert result.files_created == ()
    assert result.files_updated == ()
    assert captured["called"] is False
    assert not list(project.glob("ralph.*.yml"))
    assert not list(project.glob("PROMPT.*.md"))


def test_cli_main_non_utf8_preset_exits_two_with_real_stage(
    tmp_path: Path, capsys
) -> None:
    """B4 (U2, CLI contract): the binary preset through ``main()`` exits
    2 and the JSON view names the TRUE stage (``preset_resolution``) and
    typed code — never ``handoff`` / ``handoff_inputs_rejected``.

    Before U2 the ``UnicodeDecodeError`` reached ``run_pipeline``'s
    ``except ValueError`` unpack site, whose ``exc.args[0]`` is the
    encoding string; the resulting unpack error escaped and ``main``'s
    outer catch misattributed the failure to the handoff stage.
    """
    project = _seed_blank_project(tmp_path)
    (project / "binary.yml").write_bytes(
        b"\xff\xfe\x00binary\nname: not-decodable\n"
    )

    exit_code = bootstrap_pipeline.main(
        [
            "--cwd", str(project),
            "--preset", "binary.yml",
            "--plan", "plan.md",
            "--json",
        ]
    )
    captured = capsys.readouterr()
    assert exit_code == 2
    payload = json.loads(captured.out)
    assert payload["level"] == "blocked"
    assert payload["blocked"] is True
    assert payload["stage"] == "preset_resolution"
    assert payload["code"] == "preset_yaml_invalid"
    # Explicit misattribution guards.
    assert payload["stage"] != "handoff"
    assert payload["code"] != "handoff_inputs_rejected"
    assert payload["files_created"] == []
    assert "Traceback" not in captured.out + captured.err


def test_pipeline_unreadable_preset_file_permission_blocker(
    tmp_path: Path, capsys
) -> None:
    """B4 (U2): a chmod-0 preset file (exists, ``is_file()`` green, but
    unreadable) blocks with ``input_missing_preset_file`` and exit 2.

    ``PermissionError`` is an ``OSError`` and was previously uncaught on
    the preset read path; the typed code keeps the "not readable"
    semantics the audit stage already uses for missing files.
    """
    project = _seed_blank_project(tmp_path)
    preset_file = project / "locked.yml"
    preset_file.write_text(_VALID_FILE_PRESET_YAML, encoding="utf-8")
    preset_file.chmod(0)
    try:
        preset_file.read_text(encoding="utf-8")
    except PermissionError:
        pass
    else:
        preset_file.chmod(0o644)
        pytest.skip("chmod 0 does not block reads for this user")
    try:
        result = bootstrap_pipeline.run_pipeline(
            cwd=project,
            preset="locked.yml",
            plan_path="plan.md",
            binary="ralph",
            runner=_builtin_resolver_runner,
        )
        assert result.level == "blocked"
        assert result.blocked is True
        assert result.stage == "preset_resolution"
        assert result.code == "input_missing_preset_file"
        assert "locked.yml" in result.message
        assert result.files_created == ()

        exit_code = bootstrap_pipeline.main(
            [
                "--cwd", str(project),
                "--preset", "locked.yml",
                "--plan", "plan.md",
                "--json",
            ]
        )
        captured = capsys.readouterr()
        assert exit_code == 2
        payload = json.loads(captured.out)
        assert payload["stage"] == "preset_resolution"
        assert payload["code"] == "input_missing_preset_file"
        assert payload["code"] != "handoff_inputs_rejected"
    finally:
        preset_file.chmod(0o644)


def test_pipeline_existing_config_non_utf8_blocks_before_write(
    tmp_path: Path,
) -> None:
    """U2: a pre-existing preset-bound config carrying non-UTF-8 bytes
    blocks the batch before any write.

    Stage choice: the failing read feeds
    ``pipeline_suite.reconcile_preset_bound_suite`` (the provenance gate
    over the existing suite files), so the blocker is reported at
    ``stage="reconcile"`` with the existing ``provenance_corrupt`` code
    ("on-disk text cannot be parsed or is corrupt") — the stage that
    owns the read is the reconcile input path. AtomicWriter is never
    constructed: the undecodable original bytes survive verbatim and no
    ``.bootstrap.tmp`` residue exists.
    """
    project = _seed_blank_project(tmp_path)
    config = project / "ralph.debug.yml"
    binary_bytes = b"\xff\xfe\x00\x01binary-config"
    config.write_bytes(binary_bytes)

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )

    assert result.level == "blocked"
    assert result.blocked is True
    assert result.stage == "reconcile"
    assert result.code == "provenance_corrupt"
    assert result.files_created == ()
    assert result.files_updated == ()
    assert result.files_noop == ()
    # No half-applied state: original bytes intact, nothing else written.
    assert config.read_bytes() == binary_bytes
    assert not (project / "PROMPT.debug.md").exists()
    assert not (project / "AGENTS.md").exists()
    assert not (project / "CLAUDE.md").exists()
    assert not list(project.rglob("*.bootstrap.tmp"))


def test_pipeline_existing_agents_doc_non_utf8_blocks_before_write(
    tmp_path: Path,
) -> None:
    """U2: a pre-existing AGENTS.md carrying non-UTF-8 bytes blocks the
    batch before any write.

    Stage choice: the failing read happens while composing the managed
    docs, so the blocker is reported at ``stage="generation"`` (the
    stage that owns the doc compose) with the same
    ``provenance_corrupt`` code for corrupt on-disk text. The whole
    batch stops: mirror doc, config and prompt are never written.
    """
    project = _seed_blank_project(tmp_path)
    agents = project / "AGENTS.md"
    binary_bytes = b"\xff\xfe\x00\x01binary-doc"
    agents.write_bytes(binary_bytes)

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )

    assert result.level == "blocked"
    assert result.blocked is True
    assert result.stage == "generation"
    assert result.code == "provenance_corrupt"
    assert result.files_created == ()
    assert result.files_updated == ()
    # Original bytes intact; no other artifact written; no tmp residue.
    assert agents.read_bytes() == binary_bytes
    assert not (project / "CLAUDE.md").exists()
    assert not (project / "ralph.debug.yml").exists()
    assert not (project / "PROMPT.debug.md").exists()
    assert not list(project.rglob("*.bootstrap.tmp"))


def test_pipeline_untyped_resolution_value_error_normalized(
    tmp_path: Path, monkeypatch
) -> None:
    """U2 (guard): a ``ValueError`` without a well-formed ``(code,
    reason)`` tuple payload can never escape preset resolution — the
    unpack sites normalize it to a typed ``preset_resolution`` blocker
    instead of raising an unpack error that ``main`` would misattribute
    to the handoff stage.
    """

    def _raising(*args, **kwargs):
        raise ValueError("plain untyped resolver failure")

    monkeypatch.setattr(bootstrap_pipeline, "_resolve_preset", _raising)
    project = _seed_blank_project(tmp_path)

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=_builtin_resolver_runner,
    )

    assert result.level == "blocked"
    assert result.blocked is True
    assert result.stage == "preset_resolution"
    assert result.code == "preset_yaml_invalid"
    assert "plain untyped resolver failure" in result.message


def test_atomic_writer_non_utf8_target_rolls_back_without_residue(
    tmp_path: Path,
) -> None:
    """U2 (agent_docs contract): ``AtomicWriter`` treats an undecodable
    existing target like any other staging failure — the whole batch
    rolls back, every original byte survives and no sibling ``.tmp``
    remains. Pins the ``_stage`` normalization of ``UnicodeDecodeError``
    into the already-handled ``OSError`` rollback path.
    """
    a = tmp_path / "a.txt"
    a.write_text("original\n", encoding="utf-8")
    b = tmp_path / "b.txt"
    b.write_bytes(b"\xff\xfe\x00binary")

    with agent_docs.AtomicWriter([(a, "new-a\n"), (b, "new-b\n")]) as writer:
        committed, rolled = writer.execute()

    assert committed == ()
    # The staged-only first target is reported rolled back; the
    # undecodable second target never reaches the planned set.
    assert a in rolled
    assert a.read_text(encoding="utf-8") == "original\n"
    assert b.read_bytes() == b"\xff\xfe\x00binary"
    assert not list(tmp_path.glob("*.bootstrap.tmp"))


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
        if (
            len(argv) >= 6
            and argv[1:4] == ("preset", "builtin", "list")
            or (
                len(argv) >= 7
                and argv[1:4] == ("preset", "builtin", "show")
            )
        ):
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
    # U03: the builtin resolver emits
    # ``[binary, "preset", "builtin", "list", "--format", "json"]`` and
    # ``[binary, "preset", "builtin", "show", <id>, "--format", "yaml"]``;
    # the legacy ``preset list`` / ``preset show`` argv must NOT appear
    # in the recorded sequence any more.
    if len(argv) >= 6 and argv[1:4] == ("preset", "builtin", "list"):
        return "preset_list"
    if len(argv) >= 7 and argv[1:4] == ("preset", "builtin", "show"):
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

    Under the corrected authorization model smoke is authorised ONLY
    when the preset's RESOLVED backend equals ``content_fixed_replay``,
    so the positive path uses the replay-safe builtin stub
    (``builtin:replay-demo``) — ``builtin:debug`` resolves to
    ``claude`` and is not_authorized even under a replay-labelled
    smoke backend.
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
        preset="builtin:replay-demo",
        plan_path="plan.md",
        binary="ralph",
        runner=runner,
        smoke_backend=backend,
    )
    assert result.level == "complete"
    assert result.smoke_outcome == "bounded_terminal_reached"
    assert result.handoff_command
    assert "ralph -c ralph.replay-demo.yml" in result.handoff_command
    assert "-H builtin:replay-demo" in result.handoff_command
    assert "[CANDIDATE" not in result.handoff_command
    assert "PLAN_PATH" not in result.handoff_command


def test_pipeline_real_backend_under_replay_label_is_not_authorized(
    tmp_path: Path,
) -> None:
    """U3-fix: a preset whose RESOLVED backend is a real backend
    (``claude`` via ``builtin:debug``) must NOT be smoked even when the
    smoke backend carries the replay capability label. Authorization
    compares the resolved backend against ``content_fixed_replay``
    BEFORE any subprocess is constructed: the smoke argv stays empty,
    no smoke-shaped argv ever reaches the runner, and the handoff level
    can never be promoted to ``complete``.
    """
    project = _seed_blank_project(tmp_path)
    invocations = cli_probe.load_fixture("green")
    fallback = _static_runner_factory(invocations)
    requested: list[tuple[str, ...]] = []

    def _recording_runner(argv, timeout=None, capture_output=False, text=False):
        requested.append(tuple(argv))
        return fallback(
            argv, timeout=timeout, capture_output=capture_output, text=text
        )

    transcript = _transcript_dir(tmp_path)
    transcript.mkdir(parents=True, exist_ok=True)
    backend = smoke_runner.SafeBackend(
        name="replay",
        kind="content_fixed_replay",
        transcript_path=transcript,
    )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",  # resolves to backend ``claude``
        plan_path="plan.md",
        binary="ralph",
        runner=_recording_runner,
        smoke_backend=backend,
    )

    assert result.level == "incomplete_static_only"
    assert result.blocked is False
    assert result.smoke_outcome == "not_authorized"
    # No subprocess was constructed for the smoke.
    assert result.smoke_argv == ()
    # The evidence explains the resolved-backend mismatch.
    assert any("claude" in ev for ev in result.smoke_evidence)
    assert any(
        smoke_runner.SAFE_BACKEND_KIND in ev for ev in result.smoke_evidence
    )
    # The runner never saw a smoke-shaped argv: the static-stage calls
    # are the only ones recorded.
    assert not any(
        "--max-iterations" in argv and "--idle-timeout" in argv
        for argv in requested
    )
    # The handoff stays a static-only candidate command.
    assert "[CANDIDATE" in result.handoff_command


def test_pipeline_smoke_override_cannot_launder_real_backend(
    tmp_path: Path,
) -> None:
    """U3-fix: ``smoke_result_override`` bypasses the harness but NOT
    the resolved-backend authorization check. For a preset resolving to
    ``claude``, an injected override claiming
    ``bounded_terminal_reached`` must never reach the handoff — the
    authorization gate fires before the override is honoured, so the
    level can never be promoted to ``complete``.
    """
    project = _seed_blank_project(tmp_path)
    invocations = cli_probe.load_fixture("green")
    runner = _static_runner_factory(invocations)
    backend = smoke_runner.SafeBackend(
        name="replay", kind="content_fixed_replay"
    )
    fake_result = smoke_runner.SmokeResult(
        outcome="bounded_terminal_reached",
        evidence=("injected by test",),
        argv=("ralph", "-c", "ralph.debug.yml", "-H", "builtin:debug"),
        stderr_excerpt="",
        stdout_excerpt="",
        elapsed_seconds=1.0,
        failure_bucket="none",
    )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",  # resolves to backend ``claude``
        plan_path="plan.md",
        binary="ralph",
        runner=runner,
        smoke_backend=backend,
        smoke_result_override=fake_result,
    )

    assert result.level == "incomplete_static_only"
    assert result.blocked is False
    assert result.smoke_outcome == "not_authorized"
    assert result.smoke_argv == ()
    assert result.smoke_evidence != ("injected by test",)
    assert "[CANDIDATE" in result.handoff_command


def test_pipeline_replay_smoke_wires_transcript_dir(tmp_path: Path) -> None:
    """U3-fix happy path: for a replay-safe preset the staged
    transcript dir is genuinely wired through ``main`` →
    ``run_pipeline`` → ``_run_smoke_stage`` → ``run_smoke``: the
    authorised-path evidence records it, and the bounded-terminal
    outcome still promotes the handoff to ``complete``.
    """
    project = _seed_blank_project(tmp_path)
    invocations = cli_probe.load_fixture("green")
    runner = _static_runner_factory(invocations)

    transcript = _transcript_dir(tmp_path)
    transcript.mkdir(parents=True, exist_ok=True)
    (transcript / "events.jsonl").write_text(
        "plan.ready\nLOOP_COMPLETE\n", encoding="utf-8"
    )
    backend = smoke_runner.SafeBackend(
        name="replay",
        kind="content_fixed_replay",
        transcript_path=transcript,
    )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:replay-demo",
        plan_path="plan.md",
        binary="ralph",
        runner=runner,
        smoke_backend=backend,
    )

    assert result.level == "complete"
    assert result.smoke_outcome == "bounded_terminal_reached"
    # The transcript dir reached ``run_smoke``: the authorised-path
    # evidence carries the recorded ``transcript_dir=`` entry.
    assert any(
        "transcript_dir=" in ev and str(transcript) in ev
        for ev in result.smoke_evidence
    )
    assert result.handoff_command
    assert "[CANDIDATE" not in result.handoff_command


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
    """A typed smoke failure (``timeout_no_event``) blocks the handoff.

    Uses the replay-safe builtin stub so the resolved-backend
    authorization passes and the injected override stays reachable —
    the test's intent is "typed smoke failure ⇒ blocked handoff".
    """
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
        argv=("ralph", "-c", "ralph.replay-demo.yml", "-H", "builtin:replay-demo"),
        stderr_excerpt="",
        stdout_excerpt="",
        elapsed_seconds=30.0,
        failure_bucket="suite",
    )
    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:replay-demo",
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
# U4 — Missing-plan template / typed failure buckets / worktree reuse keys
# ---------------------------------------------------------------------------


def _seed_blank_project_without_plan(tmp_path: Path) -> Path:
    """Materialise the ``blank`` fixture WITHOUT any plan file."""
    project = tmp_path / "blank-project"
    _fixtures.materialise("blank", project)
    return project


def test_pipeline_missing_plan_template_does_not_block(tmp_path: Path) -> None:
    """U4: a missing first-run plan must NOT block provisioning.

    An inline-prompt preset with ``plan_path=None`` still provisions
    the owned artifacts; the handoff stays ``incomplete_static_only``
    and the launch command carries the ``--plan PLAN_PATH`` template.
    The pipeline NEVER invents a plan file in the target project.
    """
    project = _seed_blank_project_without_plan(tmp_path)
    invocations = cli_probe.load_fixture("debug-green")
    runner = _static_runner_factory(invocations)

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path=None,
        binary="ralph",
        runner=runner,
    )

    # Provisioning succeeds; the static gate is green end-to-end.
    assert result.blocked is False
    assert result.level == "incomplete_static_only"
    assert [d.outcome for d in result.stage_decisions] == ["ok"] * 4
    assert result.smoke_outcome is None

    # The command is the PLAN_PATH template: never a bare candidate and
    # never an invented concrete plan path.
    assert result.handoff_command.startswith(
        "[TEMPLATE - replace PLAN_PATH before running]"
    )
    assert "[CANDIDATE" not in result.handoff_command
    assert "--plan PLAN_PATH" in result.handoff_command
    plan_idx = result.handoff_argv.index("--plan")
    assert result.handoff_argv[plan_idx + 1] == "PLAN_PATH"

    # Owned artifacts are written; NO plan file is invented.
    assert (project / "ralph.debug.yml").is_file()
    assert (project / "PROMPT.debug.md").is_file()
    assert not (project / "plan.md").exists()
    assert not (project / "PLAN_PATH").exists()
    assert sorted(p.name for p in project.iterdir()) == [
        "AGENTS.md",
        "CLAUDE.md",
        "PROMPT.debug.md",
        "ralph.debug.yml",
    ]


_U4_TYPED_FAILURE_SCENARIOS = (
    {
        "outcome": "timeout_no_event",
        "bucket": "suite",
        "evidence": "idle timeout elapsed before any event was observed",
    },
    {
        "outcome": "non_zero_exit",
        "bucket": "backend",
        "evidence": "ralph exited with code 1 before reaching the terminal",
    },
    {
        "outcome": "error_event_detected",
        "bucket": "project_command",
        "evidence": "ERROR_EVENT: project verification command failed",
    },
)


@pytest.mark.parametrize(
    "scenario",
    _U4_TYPED_FAILURE_SCENARIOS,
    ids=[s["outcome"] for s in _U4_TYPED_FAILURE_SCENARIOS],
)
def test_pipeline_typed_smoke_failure_bucket_blocks_and_reports(
    tmp_path: Path, scenario
) -> None:
    """U4: every typed smoke failure bucket blocks the handoff.

    The handoff level is ``blocked``, the command is empty, the typed
    ``smoke_failure_bucket`` flows into ``PipelineResult``, and the
    report surfaces the outcome + bucket so the operator can reconcile.

    Uses the replay-safe builtin stub so the resolved-backend
    authorization passes and the injected override stays reachable.
    """
    project = _seed_blank_project(tmp_path)
    invocations = cli_probe.load_fixture("green")
    runner = _static_runner_factory(invocations)
    backend = smoke_runner.SafeBackend(
        name="replay", kind="content_fixed_replay"
    )
    fake_result = smoke_runner.SmokeResult(
        outcome=scenario["outcome"],
        evidence=(scenario["evidence"],),
        argv=("ralph", "-c", "ralph.replay-demo.yml", "-H", "builtin:replay-demo"),
        stderr_excerpt="smoke failed",
        stdout_excerpt="",
        elapsed_seconds=12.0,
        failure_bucket=scenario["bucket"],
    )

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:replay-demo",
        plan_path="plan.md",
        binary="ralph",
        runner=runner,
        smoke_backend=backend,
        smoke_result_override=fake_result,
    )

    assert result.level == "blocked"
    assert result.blocked is True
    assert result.stage == "handoff"
    assert result.code == ""
    assert result.smoke_outcome == scenario["outcome"]
    # The typed failure bucket flows into the PipelineResult.
    assert result.smoke_failure_bucket == scenario["bucket"]
    assert result.smoke_evidence == (scenario["evidence"],)
    # Blocked handoff never carries an executable command.
    assert result.handoff_command == ""
    assert result.handoff_argv == ()
    # The blocker summary + report surface outcome and bucket.
    assert scenario["outcome"] in result.message
    assert scenario["bucket"] in result.message
    assert "## Blocker" in result.handoff_report
    assert f"Status: `blocked -- {scenario['bucket']}`" in result.handoff_report
    assert f"`{scenario['outcome']}`" in result.handoff_report
    assert f"`{scenario['bucket']}`" in result.handoff_report
    assert "must reconcile before launch" in result.handoff_report


def test_pipeline_unauthorized_smoke_report_surfaces_residual_risk(
    tmp_path: Path,
) -> None:
    """U4: a refused (unsafe-backend) smoke leaves a residual-risk note.

    The level stays ``incomplete_static_only``; the report states that
    static load passed but the loop has not been verified end-to-end,
    and the candidate command is the only actionable output.
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
    assert result.blocked is False
    assert result.smoke_outcome == "not_authorized"
    assert result.smoke_failure_bucket == "none"
    assert result.smoke_argv == ()
    assert "[CANDIDATE" in result.handoff_command
    assert "Status: `static-only -- smoke-not-authorized`" in result.handoff_report
    assert "Static load passed" in result.handoff_report
    assert "loop has not been verified end-to-end" in result.handoff_report


def test_pipeline_worktree_plan_arg_reuse_key_in_command(tmp_path: Path) -> None:
    """U4: a worktree launch carries the explicit ``--plan`` reuse key."""
    project = _seed_blank_project(tmp_path)
    invocations = cli_probe.load_fixture("debug-green")
    runner = _static_runner_factory(invocations)

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=runner,
        use_worktree=True,
        reuse_worktree=True,
        plan_arg="plan.md",
    )

    assert result.blocked is False
    assert result.level == "incomplete_static_only"
    argv = result.handoff_argv
    assert "--worktree" in argv
    assert "--reuse-worktree" in argv
    plan_idx = argv.index("--plan")
    assert argv[plan_idx + 1] == "plan.md"
    assert "--worktree-name" not in argv
    assert "[CANDIDATE" in result.handoff_command
    assert "--worktree --reuse-worktree --plan plan.md" in result.handoff_command


def test_pipeline_worktree_name_reuse_key_in_command(tmp_path: Path) -> None:
    """U4: a worktree launch may carry ``--worktree-name`` as the reuse
    key instead of ``--plan``; the plan path never doubles as a key."""
    project = _seed_blank_project(tmp_path)
    invocations = cli_probe.load_fixture("debug-green")
    runner = _static_runner_factory(invocations)

    result = bootstrap_pipeline.run_pipeline(
        cwd=project,
        preset="builtin:debug",
        plan_path="plan.md",
        binary="ralph",
        runner=runner,
        use_worktree=True,
        reuse_worktree=True,
        worktree_name="my-plan-worktree",
    )

    assert result.blocked is False
    assert result.level == "incomplete_static_only"
    argv = result.handoff_argv
    assert "--worktree" in argv
    assert "--reuse-worktree" in argv
    name_idx = argv.index("--worktree-name")
    assert argv[name_idx + 1] == "my-plan-worktree"
    # The reuse key is the worktree name; no --plan flag is emitted.
    assert "--plan" not in argv
    assert "[CANDIDATE" in result.handoff_command


@pytest.mark.parametrize(
    ("reuse_worktree", "plan_arg", "worktree_name"),
    [
        (True, None, None),
        (False, "plan.md", None),
    ],
    ids=["no-reuse-key", "reuse-flag-missing"],
)
def test_pipeline_worktree_missing_reuse_key_rejected(
    tmp_path: Path, reuse_worktree, plan_arg, worktree_name
) -> None:
    """U4: the handoff module's reuse-key rule rejects worktree runs
    without an explicit key.

    ``handoff.HandoffInputs.__post_init__`` raises
    ``ValueError("worktree reuse key required")`` both when the reuse
    flag is missing and when neither ``plan_arg`` nor ``worktree_name``
    is supplied; the pipeline propagates the error rather than
    rendering a launch command.
    """
    project = _seed_blank_project(tmp_path)
    invocations = cli_probe.load_fixture("debug-green")
    runner = _static_runner_factory(invocations)

    with pytest.raises(ValueError, match="worktree reuse key required"):
        bootstrap_pipeline.run_pipeline(
            cwd=project,
            preset="builtin:debug",
            plan_path="plan.md",
            binary="ralph",
            runner=runner,
            use_worktree=True,
            reuse_worktree=reuse_worktree,
            plan_arg=plan_arg,
            worktree_name=worktree_name,
        )


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
    # All three handoff levels must be described in both surfaces.
    for level in bootstrap_pipeline.HANDOFF_LEVELS:
        assert level in skill_md, f"SKILL.md must describe level {level!r}"
        assert level in openai_yaml, f"openai.yaml must describe level {level!r}"
    # The static-load caveat must stay explicit in the operator docs.
    assert "`dry-run green != loop closed`" in skill_md


# ---------------------------------------------------------------------------
# U5 — CLI contract: in-process main() + exit codes + backend switch
# ---------------------------------------------------------------------------


PIPELINE_SCRIPT = (
    ROOT / "skills" / "ralph-project-bootstrap" / "scripts" / "bootstrap_pipeline.py"
)


def _patch_pipeline_runner(monkeypatch, runner) -> None:
    """Route ``main``'s ``run_pipeline`` call through ``runner``.

    The real ``run_pipeline`` stays in charge of every stage; only the
    subprocess runner is injected so no real binary spawns. ``main``
    itself is exercised unchanged.
    """
    real = bootstrap_pipeline.run_pipeline

    def _patched(**kwargs):
        kwargs["runner"] = runner
        return real(**kwargs)

    monkeypatch.setattr(bootstrap_pipeline, "run_pipeline", _patched)


def test_cli_main_blocked_when_worktree_reuse_key_missing(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    """U5: the handoff helper's ``ValueError("worktree reuse key
    required")`` must surface as a blocked-shaped result view with exit
    code 2 — never a traceback."""
    project = _seed_blank_project(tmp_path)

    def _raising(**kwargs):
        raise ValueError("worktree reuse key required")

    monkeypatch.setattr(bootstrap_pipeline, "run_pipeline", _raising)

    exit_code = bootstrap_pipeline.main(
        ["--cwd", str(project), "--preset", "builtin:debug"]
    )
    captured = capsys.readouterr()
    assert exit_code == 2
    assert "level: blocked" in captured.out
    assert "stage: handoff" in captured.out
    assert "code: worktree_reuse_key_missing" in captured.out
    assert "message: worktree reuse key required" in captured.out
    assert "Traceback" not in captured.out + captured.err


def test_cli_main_json_blocked_when_worktree_reuse_key_missing(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    """U5: ``--json`` renders the same blocked-shaped record."""
    project = _seed_blank_project(tmp_path)

    def _raising(**kwargs):
        raise ValueError("worktree reuse key required")

    monkeypatch.setattr(bootstrap_pipeline, "run_pipeline", _raising)

    exit_code = bootstrap_pipeline.main(
        ["--cwd", str(project), "--preset", "builtin:debug", "--json"]
    )
    captured = capsys.readouterr()
    assert exit_code == 2
    payload = json.loads(captured.out)
    assert payload["level"] == "blocked"
    assert payload["blocked"] is True
    assert payload["stage"] == "handoff"
    assert payload["code"] == "worktree_reuse_key_missing"
    assert payload["message"] == "worktree reuse key required"
    assert payload["handoff_command"] == ""


def test_cli_exit_code_zero_for_static_only(tmp_path: Path, monkeypatch, capsys) -> None:
    """U5: ``incomplete_static_only`` (green static gate, no smoke)
    exits 0 in the text view."""
    project = _seed_blank_project(tmp_path)
    invocations = cli_probe.load_fixture("debug-green")
    _patch_pipeline_runner(monkeypatch, _static_runner_factory(invocations))

    exit_code = bootstrap_pipeline.main(
        ["--cwd", str(project), "--preset", "builtin:debug", "--plan", "plan.md"]
    )
    captured = capsys.readouterr()
    assert exit_code == 0
    assert "level: incomplete_static_only" in captured.out
    assert "config: ralph.debug.yml" in captured.out
    assert "prompt: PROMPT.debug.md" in captured.out


def test_cli_exit_code_zero_for_complete(tmp_path: Path, monkeypatch, capsys) -> None:
    """U5: a replay-bounded smoke that reaches the terminal promotes to
    ``complete`` and exits 0 via ``main``.

    The transcript token is repo-relative (anchored on ``--cwd``) and
    the preset resolves to ``content_fixed_replay`` — the only
    combination the corrected authorization model authorises.
    """
    project = _seed_blank_project(tmp_path)
    invocations = cli_probe.load_fixture("green")
    _patch_pipeline_runner(monkeypatch, _static_runner_factory(invocations))
    transcript = project / "transcripts"
    transcript.mkdir(parents=True, exist_ok=True)

    exit_code = bootstrap_pipeline.main(
        [
            "--cwd", str(project),
            "--preset", "builtin:replay-demo",
            "--plan", "plan.md",
            "--replay-transcript", "transcripts",
        ]
    )
    captured = capsys.readouterr()
    assert exit_code == 0
    assert "level: complete" in captured.out
    assert "smoke_outcome: bounded_terminal_reached" in captured.out


@pytest.mark.parametrize(
    "token",
    ["/tmp/transcripts", "../transcripts"],
    ids=["absolute", "dotdot-escape"],
)
def test_cli_replay_transcript_unsafe_path_blocked(
    tmp_path: Path, monkeypatch, capsys, token
) -> None:
    """U3-fix: ``--replay-transcript`` passes the SAME repo-relative
    input gate family as plan/prompt/preset: absolute paths and ``..``
    escapes are rejected typed ``input_path_unsafe`` with exit 2,
    BEFORE ``run_pipeline`` is invoked.
    """
    project = _seed_blank_project(tmp_path)

    def _never_called(**kwargs):  # pragma: no cover - asserts below
        raise AssertionError(
            "unsafe replay transcript must not reach run_pipeline"
        )

    monkeypatch.setattr(bootstrap_pipeline, "run_pipeline", _never_called)

    exit_code = bootstrap_pipeline.main(
        [
            "--cwd", str(project),
            "--preset", "builtin:debug",
            "--replay-transcript", token,
        ]
    )
    captured = capsys.readouterr()
    assert exit_code == 2
    assert "level: blocked" in captured.out
    assert "stage: audit" in captured.out
    assert "code: input_path_unsafe" in captured.out


def test_cli_exit_code_two_for_blocked_input(tmp_path: Path, capsys) -> None:
    """U5: a typed input blocker exits 2 (no runner injection needed —
    the pipeline short-circuits before any subprocess call)."""
    exit_code = bootstrap_pipeline.main(
        [
            "--cwd", str(tmp_path / "does-not-exist"),
            "--preset", "builtin:debug",
        ]
    )
    captured = capsys.readouterr()
    assert exit_code == 2
    assert "level: blocked" in captured.out
    assert "code: input_cwd_missing" in captured.out


def test_cli_text_and_json_express_same_result(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    """U5: the text view and ``--json`` derive from the SAME
    ``PipelineResult`` — every structured field the JSON view carries
    must surface in the text view for an identical run."""
    project_text = _seed_blank_project(tmp_path / "text")
    project_json = _seed_blank_project(tmp_path / "json")
    argv_tail = ["--preset", "builtin:debug", "--plan", "plan.md"]

    invocations = cli_probe.load_fixture("debug-green")
    _patch_pipeline_runner(monkeypatch, _static_runner_factory(invocations))
    assert bootstrap_pipeline.main(["--cwd", str(project_json), "--json", *argv_tail]) == 0
    payload = json.loads(capsys.readouterr().out)

    invocations = cli_probe.load_fixture("debug-green")
    _patch_pipeline_runner(monkeypatch, _static_runner_factory(invocations))
    assert bootstrap_pipeline.main(["--cwd", str(project_text), *argv_tail]) == 0
    text_out = capsys.readouterr().out

    # Scalar fields render verbatim in the text view.
    assert f"level: {payload['level']}" in text_out
    assert f"stage: {payload['stage']}" in text_out
    assert f"root: {payload['root']}" in text_out
    assert f"preset: {payload['preset']}" in text_out
    assert f"config: {payload['config_path']}" in text_out
    assert f"prompt: {payload['prompt_path']}" in text_out
    # File lists and validation evidence render one line per entry.
    for path in payload["files_created"]:
        assert f"  - {path}" in text_out
    for entry in payload["validation_evidence"]:
        assert f"  - {entry}" in text_out
    # The handoff command block is identical in both views.
    if payload["handoff_command"]:
        assert payload["handoff_command"] in text_out


def test_cli_replay_transcript_is_only_safe_backend_switch(
    tmp_path: Path, monkeypatch
) -> None:
    """U5: ``--replay-transcript`` is the ONLY switch that enables a
    ``SafeBackend``; without it the smoke backend stays ``None``.

    The repo-relative transcript token is anchored on ``--cwd``: the
    constructed ``SafeBackend`` carries the cwd-anchored resolved path,
    never a process-cwd interpretation of the token.
    """
    project = _seed_blank_project(tmp_path)
    captured_kwargs: dict[str, object] = {}

    def _capture(**kwargs):
        captured_kwargs.update(kwargs)
        return bootstrap_pipeline.PipelineResult(
            level="incomplete_static_only",
            blocked=False,
            stage="handoff",
            code="",
            message="",
        )

    monkeypatch.setattr(bootstrap_pipeline, "run_pipeline", _capture)

    # No switch: the smoke backend must stay disabled.
    bootstrap_pipeline.main(["--cwd", str(project), "--preset", "builtin:debug"])
    assert captured_kwargs["smoke_backend"] is None

    # --replay-transcript: the ONLY enabling switch constructs the
    # bounded SafeBackend; the token is anchored on ``--cwd``.
    transcript = project / "transcripts"
    transcript.mkdir(parents=True, exist_ok=True)
    bootstrap_pipeline.main(
        [
            "--cwd", str(project),
            "--preset", "builtin:debug",
            "--replay-transcript", "transcripts",
        ]
    )
    backend = captured_kwargs["smoke_backend"]
    assert isinstance(backend, smoke_runner.SafeBackend)
    assert backend.kind == "content_fixed_replay"
    assert backend.transcript_path == transcript.resolve()

    # Structural guard: no other CLI flag mentions smoke / backend.
    parser = bootstrap_pipeline.build_cli_parser()
    smoke_flags = [
        action.dest
        for action in parser._actions
        if "smoke" in action.dest or "backend" in action.dest or "replay" in action.dest
    ]
    assert smoke_flags == ["replay_transcript"]


def test_cli_help_subprocess_smoke() -> None:
    """U5: the script is directly executable and its --help lists the
    full public flag surface."""
    proc = subprocess.run(
        [sys.executable, str(PIPELINE_SCRIPT), "--help"],
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert proc.returncode == 0, proc.stderr
    for token in (
        "--cwd",
        "--preset",
        "--plan",
        "--prompt-file",
        "--binary",
        "--refresh-existing",
        "--replay-transcript",
        "--json",
    ):
        assert token in proc.stdout, f"--help must list {token}"


# ---------------------------------------------------------------------------
# U5 — Installed skill copy parity
# ---------------------------------------------------------------------------


def _relative_files(root: Path) -> set[str]:
    """Relative file set under ``root`` excluding bytecode caches.

    ``__pycache__`` contents are environment-dependent (present only
    after the modules were imported) and carry no contract value, so
    both sides of the parity comparison drop them.
    """
    return {
        str(path.relative_to(root))
        for path in root.rglob("*")
        if path.is_file() and "__pycache__" not in path.parts
    }


def test_project_bootstrap_skill_copies_are_in_sync(tmp_path: Path) -> None:
    """U5: an installed copy of the skill is byte-for-byte identical to
    the source tree.

    The test installs into a temp dir via the installer's ``--dir``
    entry so it passes in environments where the gitignored local
    copies (``.claude/skills`` / ``.agents/skills``) do not exist.
    """
    target = tmp_path / "skills-target"
    exit_code = install.main(
        ["--dir", str(target), "--force", "ralph-project-bootstrap"]
    )
    assert exit_code == 0

    source = ROOT / "skills" / "ralph-project-bootstrap"
    copied = target / "ralph-project-bootstrap"
    assert copied.is_dir()

    source_files = _relative_files(source)
    copied_files = _relative_files(copied)
    assert source_files == copied_files, (
        f"copy diverged from source: missing={source_files - copied_files} "
        f"extra={copied_files - source_files}"
    )
    # Every copied file is a byte-equal regular file, never a symlink.
    for rel in sorted(source_files):
        copied_path = copied / rel
        assert not copied_path.is_symlink(), rel
        assert copied_path.read_bytes() == (source / rel).read_bytes(), rel
    # The unified entry point and its public description are covered
    # by the parity walk above; name them explicitly so a future
    # ignore-pattern that drops them fails loudly.
    for rel in (
        "SKILL.md",
        "agents/openai.yaml",
        "scripts/bootstrap_pipeline.py",
    ):
        assert rel in source_files