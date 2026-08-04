"""Unified bootstrap entry point for ``ralph-project-bootstrap``.

This module owns the single ``run_pipeline`` call operator/agent code
must use to bootstrap a target project onto an existing preset. It
composes the existing helpers — ``audit``, ``pipeline_suite``,
``agent_docs``, ``cli_probe``, ``smoke_runner``, ``handoff`` — into a
failure-short-circuiting state machine so callers cannot accidentally
skip a stage or smuggle free-text evidence into the handoff level.

Design rules:

* **Orchestration only.** No preset text rendering, no managed-section
  composition, no subprocess invocation logic, no smoke harness
  behaviour is duplicated. The pipeline delegates to the existing
  pure helpers; every helper-level contract suite must continue to
  pass unchanged.
* **Writes are bounded.** The pipeline refuses to write a target
  file unless the audit and preset-resolution stages pass and the
  owned-artifact stage reports ``created`` / ``updated`` /
  ``noop``. The preset-bound config/prompt pair and the managed
  AGENTS.md / CLAUDE.md sections are composed into ONE
  ``agent_docs.AtomicWriter`` batch so any conflict or failure rolls
  every target back to its pre-write state.
* **Static evidence is typed.** The four ``StageDecision`` rows from
  ``cli_probe.validate_pipeline`` are recorded verbatim; the handoff
  must reference them rather than re-derive a static-only claim.
* **Smoke authorization is strict.** Authorization compares the
  preset's RESOLVED backend (``cli.backend`` from preset resolution)
  against the single auto-authorised kind ``content_fixed_replay``
  BEFORE any subprocess is constructed and before any injected smoke
  override is honoured; a mismatch produces
  ``SmokeResult(outcome="not_authorized")`` with empty argv regardless
  of the smoke backend's capability label.
* **Handoff level is typed.** The pipeline never produces a
  ``complete`` handoff unless the typed smoke outcome is
  ``bounded_terminal_reached``; a free-text ``smoke_evidence`` line
  cannot promote the level.
* **No environment reads.** The pipeline is pure relative to the
  inputs the caller supplies; tests inject runners so the real
  binary is never spawned.

Public API:

* :class:`PipelineResult`
* :func:`run_pipeline`
* :func:`render_cli_json`
* :func:`main` (CLI entry point)
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Callable, Iterable, Literal, Mapping

import _paths  # type: ignore[import-not-found]  # loaded via skills/ralph-project-bootstrap/scripts on sys.path
import agent_docs  # type: ignore[import-not-found]
import audit  # type: ignore[import-not-found]
import cli_probe  # type: ignore[import-not-found]
import handoff as handoff_module  # type: ignore[import-not-found]
import pipeline_suite  # type: ignore[import-not-found]
import smoke_runner  # type: ignore[import-not-found]

# Stage names — the canonical order in which the pipeline advances.
_PIPELINE_STAGES: tuple[str, ...] = (
    "audit",
    "preset_resolution",
    "generation",
    "reconcile",
    "static_validation",
    "smoke",
    "handoff",
)

# Canonical handoff levels. Mirrors ``handoff.HANDOFF_LEVELS`` but is
# declared locally so this module can be imported even when the
# handoff helper is being refactored.
HANDOFF_LEVELS: tuple[str, ...] = ("complete", "incomplete_static_only", "blocked")

# Marker id for the AGENTS.md / CLAUDE.md managed sections. Mirrors
# the fixture convention (``existing-docs`` / ``conflicting-docs``)
# and the helper-level e2e chain; the agent_docs helper owns the
# marker bytes themselves.
_DOCS_MARKER_ID = "agents-docs-v1"

SubprocessRunner = Callable[..., subprocess.CompletedProcess]


# ---------------------------------------------------------------------------
# Typed result dataclasses
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class ResolvedPreset:
    """A preset fully resolved to YAML + derived runtime fields."""

    preset_id: str  # The literal identifier the caller supplied (file path or builtin id).
    source_kind: Literal["file", "builtin"]
    template_name: str  # The template name (builtin) or file stem.
    text: str  # Full resolved YAML bytes.
    backend: str  # ``cli.backend`` from the resolved preset.
    max_iterations: int  # ``event_loop.max_iterations``.
    max_runtime_seconds: int  # ``event_loop.max_runtime_seconds``.
    inline_prompt_present: bool  # Whether ``event_loop.prompt`` is non-empty.


@dataclass(frozen=True)
class PipelineResult:
    """Aggregate outcome of :func:`run_pipeline`.

    The dataclass is intentionally wide so the CLI text view, the
    JSON view and the markdown report all derive from the same
    typed record — never from a free-text formatter.
    """

    level: Literal["complete", "incomplete_static_only", "blocked"]
    blocked: bool
    stage: str
    code: str
    message: str
    root: str = ""
    preset: str = ""
    config_path: str = ""
    prompt_path: str = ""
    files_created: tuple[str, ...] = ()
    files_updated: tuple[str, ...] = ()
    files_noop: tuple[str, ...] = ()
    validation_evidence: tuple[str, ...] = ()
    stage_decisions: tuple[Any, ...] = ()
    smoke_outcome: str | None = None
    smoke_argv: tuple[str, ...] = ()
    smoke_failure_bucket: str | None = None
    smoke_evidence: tuple[str, ...] = ()
    handoff_command: str = ""
    handoff_report: str = ""
    handoff_argv: tuple[str, ...] = ()
    next_action: str = ""
    blocker_paths: tuple[str, ...] = ()
    resolved_preset: ResolvedPreset | None = None

    def to_json(self) -> str:
        """Return a JSON string representation of the result.

        The ``resolved_preset`` and ``stage_decisions`` fields are
        dataclasses; ``asdict`` flattens them. Callers can ``json.loads``
        the result to drive downstream tooling.
        """
        payload = asdict(self)
        return json.dumps(payload, indent=2, ensure_ascii=False)


@dataclass(frozen=True)
class _GenerationOutcome:
    """Per-artifact dispositions produced by the generation stage.

    ``suite_kind`` applies to BOTH preset-bound files
    (``ralph.<stem>.yml`` + ``PROMPT.<stem>.md``); the two managed
    docs carry independent dispositions because a rerun may need to
    recreate a doc the operator deleted while the suite itself is a
    noop. ``written_docs`` lists the doc paths committed by this run
    so the post-write verify reopens exactly the artifacts the batch
    touched.
    """

    suite_kind: str  # one of {"created", "updated", "noop"}
    agents_kind: str  # one of {"created", "updated", "noop"}
    claude_kind: str  # one of {"created", "updated", "noop"}
    docs_body: str
    marker_id: str
    written_docs: tuple[str, ...] = ()


# ---------------------------------------------------------------------------
# Input normalization + path confinement
# ---------------------------------------------------------------------------


def _check_repo_relative(label: str, value: str | None) -> tuple[bool, str]:
    """Confirm ``value`` is a safe repo-relative path when supplied."""
    if value is None:
        return True, ""
    if not isinstance(value, str) or not value.strip():
        return False, f"{label}_path_missing"
    if not _paths.is_safe_relative(value):
        return False, "input_path_unsafe"
    return True, ""


def _normalize_inputs(
    *,
    cwd: Path | str,
    preset: str,
    plan_path: str | None,
    prompt_file: str | None,
) -> tuple[Path, str, str | None, str | None, str]:
    """Return ``(cwd, preset, plan_path, prompt_file, error_code)``.

    Any error_code other than ``""`` signals a blocker; the caller
    must not proceed past the audit stage with that value.
    """
    cwd_path = Path(cwd).resolve()
    if not cwd_path.is_dir():
        return cwd_path, preset, plan_path, prompt_file, "input_cwd_missing"
    if not isinstance(preset, str):
        return cwd_path, preset, plan_path, prompt_file, "input_missing_preset"
    if not preset.strip():
        return cwd_path, preset, plan_path, prompt_file, "input_missing_preset"
    # File presets pass the SAME repo-relative gate as plan/prompt so
    # absolute paths, ``..`` escapes and control bytes are rejected
    # before any filesystem read or subprocess call. ``builtin:<id>``
    # presets are not path tokens; audit keeps its dedicated id-shape
    # branch for them and this gate must not touch them.
    if not preset.startswith("builtin:") and not _paths.is_safe_relative(preset):
        return cwd_path, preset, plan_path, prompt_file, "input_path_unsafe"
    ok_plan, code_plan = _check_repo_relative("plan", plan_path)
    if not ok_plan:
        return cwd_path, preset, plan_path, prompt_file, code_plan
    ok_prompt, code_prompt = _check_repo_relative("prompt", prompt_file)
    if not ok_prompt:
        return cwd_path, preset, plan_path, prompt_file, code_prompt
    return cwd_path, preset, plan_path, prompt_file, ""


# ---------------------------------------------------------------------------
# Preset resolution
# ---------------------------------------------------------------------------


def _load_yaml_mapping(text: str) -> dict[str, Any]:
    """Parse ``text`` as YAML and return the top-level mapping.

    ``PyYAML`` is required by the existing helpers; this module does
    NOT add a third-party dependency, but it uses ``yaml`` when the
    preset text is available so it can validate the parse result
    before delegating to the helpers.
    """
    import yaml  # type: ignore[import-not-found]

    try:
        loaded = yaml.safe_load(text)
    except yaml.YAMLError as exc:
        raise ValueError(("preset_yaml_invalid", str(exc))) from exc
    if not isinstance(loaded, dict):
        raise ValueError(("preset_yaml_invalid", "top-level YAML must be a mapping"))
    return loaded


def _typed_blocker_payload(exc: ValueError) -> tuple[str, str]:
    """Return the well-formed ``(code, reason)`` payload carried by ``exc``.

    Typed errors in this module carry a single ``(code, reason)`` tuple
    argument. Any other ``ValueError`` shape — e.g. a
    ``UnicodeDecodeError`` whose ``args`` are
    ``(encoding, start, end, reason)`` — is normalised to a typed code
    here so every unpack site yields a locatable blocker instead of an
    unpack error that escapes ``run_pipeline`` and gets misattributed
    to the handoff stage.
    """
    payload = exc.args[0] if exc.args else None
    if (
        isinstance(payload, tuple)
        and len(payload) == 2
        and isinstance(payload[0], str)
        and isinstance(payload[1], str)
    ):
        return payload[0], payload[1]
    return "preset_yaml_invalid", str(exc) or repr(exc)


def _derive_runtime_fields(loaded: Mapping[str, Any]) -> tuple[str, int, int, bool]:
    """Extract ``(backend, max_iterations, max_runtime_seconds, inline_prompt_present)``.

    Raises ``ValueError`` carrying a ``(code, message)`` tuple when
    any field is missing or malformed.
    """
    cli = loaded.get("cli")
    backend = cli.get("backend") if isinstance(cli, dict) else None
    if not isinstance(backend, str) or not backend.strip():
        raise ValueError(
            ("preset_runtime_contract_missing", "cli.backend must be a non-empty string")
        )
    event_loop = loaded.get("event_loop")
    if not isinstance(event_loop, dict):
        raise ValueError(
            ("preset_runtime_contract_missing", "event_loop mapping is required")
        )
    raw_iter = event_loop.get("max_iterations")
    raw_runtime = event_loop.get("max_runtime_seconds")
    try:
        max_iterations = int(raw_iter)
        max_runtime_seconds = int(raw_runtime)
    except (TypeError, ValueError) as exc:
        raise ValueError(
            (
                "preset_runtime_contract_missing",
                f"event_loop.{'max_iterations' if isinstance(raw_iter, (int, str, type(None))) else 'max_runtime_seconds'} must be a positive integer",
            )
        ) from exc
    if max_iterations <= 0 or max_runtime_seconds <= 0:
        raise ValueError(
            ("preset_runtime_contract_missing", "budget fields must be positive")
        )
    prompt = event_loop.get("prompt")
    inline_prompt_present = isinstance(prompt, str) and bool(prompt.strip())
    return backend, max_iterations, max_runtime_seconds, inline_prompt_present


def _resolve_file_preset(root: Path, preset: str) -> ResolvedPreset:
    """Read a repo-relative file preset and derive runtime fields.

    ``root`` is the canonical project root the audit stage already
    validated the preset against; the read MUST stay anchored there so
    "exists at root R" and "reads from root R" can never disagree.
    """
    preset_path = root / preset
    if not preset_path.is_file():
        raise ValueError(("input_missing_preset_file", f"preset file not readable: {preset}"))
    try:
        text = preset_path.read_text(encoding="utf-8")
    except UnicodeDecodeError as exc:
        raise ValueError(
            (
                "preset_yaml_invalid",
                f"preset file is not valid UTF-8: {preset} -- {exc}",
            )
        ) from exc
    except OSError as exc:
        raise ValueError(
            (
                "input_missing_preset_file",
                f"preset file not readable: {preset} -- {exc}",
            )
        ) from exc
    loaded = _load_yaml_mapping(text)
    try:
        backend, max_iterations, max_runtime_seconds, inline_prompt_present = (
            _derive_runtime_fields(loaded)
        )
    except ValueError as exc:
        code, reason = _typed_blocker_payload(exc)
        raise ValueError((code, reason)) from exc
    template_name = Path(preset).stem
    return ResolvedPreset(
        preset_id=preset,
        source_kind="file",
        template_name=template_name,
        text=text,
        backend=backend,
        max_iterations=max_iterations,
        max_runtime_seconds=max_runtime_seconds,
        inline_prompt_present=inline_prompt_present,
    )


def _resolve_builtin_preset(
    *,
    builtin_id: str,
    binary: str,
    runner: SubprocessRunner,
) -> ResolvedPreset:
    """Resolve ``builtin:<id>`` via ``preset list`` → ``preset show``."""
    list_proc = runner(
        [binary, "preset", "list", "--format", "json"],
        timeout=cli_probe.DEFAULT_TIMEOUT,
        capture_output=True,
        text=True,
    )
    if list_proc.returncode != 0:
        raise ValueError(
            (
                "builtin_list_failed",
                f"ralph preset list --format json returned {list_proc.returncode}",
            )
        )
    try:
        listing = json.loads(list_proc.stdout or "{}")
    except json.JSONDecodeError as exc:
        raise ValueError(
            ("builtin_list_unparseable", f"ralph preset list emitted invalid JSON: {exc}")
        ) from exc
    manifests = listing.get("manifests") if isinstance(listing, dict) else None
    if not isinstance(manifests, list):
        raise ValueError(
            ("builtin_list_unparseable", "ralph preset list response must carry a 'manifests' array")
        )
    template_name = None
    for entry in manifests:
        if not isinstance(entry, dict):
            continue
        if entry.get("source") == builtin_id:
            candidate = entry.get("name")
            if isinstance(candidate, str) and candidate.strip():
                template_name = candidate
                break
    if template_name is None:
        raise ValueError(
            ("builtin_source_missing", f"no preset manifest carries source {builtin_id!r}")
        )
    show_proc = runner(
        [binary, "preset", "show", template_name, "--format", "yaml"],
        timeout=cli_probe.DEFAULT_TIMEOUT,
        capture_output=True,
        text=True,
    )
    if show_proc.returncode != 0:
        raise ValueError(
            (
                "builtin_show_failed",
                f"ralph preset show {template_name} returned {show_proc.returncode}: {show_proc.stderr.strip()}",
            )
        )
    text = show_proc.stdout or ""
    if not text.strip():
        raise ValueError(("builtin_show_empty", f"ralph preset show {template_name} emitted empty body"))
    loaded = _load_yaml_mapping(text)
    try:
        backend, max_iterations, max_runtime_seconds, inline_prompt_present = (
            _derive_runtime_fields(loaded)
        )
    except ValueError as exc:
        code, reason = _typed_blocker_payload(exc)
        raise ValueError((code, reason)) from exc
    return ResolvedPreset(
        preset_id=builtin_id,
        source_kind="builtin",
        template_name=template_name,
        text=text,
        backend=backend,
        max_iterations=max_iterations,
        max_runtime_seconds=max_runtime_seconds,
        inline_prompt_present=inline_prompt_present,
    )


def _resolve_preset(
    *,
    root: Path,
    preset: str,
    binary: str,
    runner: SubprocessRunner,
) -> ResolvedPreset:
    """Resolve a file or builtin preset, normalising errors.

    ``root`` is the canonical project root: file presets are read from
    ``root / preset`` (the same anchor the audit existence check used);
    builtin ids ignore it.
    """
    if preset.startswith("builtin:"):
        return _resolve_builtin_preset(
            builtin_id=preset, binary=binary, runner=runner
        )
    return _resolve_file_preset(root, preset)


# ---------------------------------------------------------------------------
# Generation + reconcile
# ---------------------------------------------------------------------------


def _compose_preset_bound(
    *,
    cwd: Path,
    resolved: ResolvedPreset,
    preset_id: str,
) -> pipeline_suite.PresetBoundSuite:
    """Compose the preset-bound two-file suite for the resolved preset."""
    project_facts = audit.collect_project_facts(cwd)
    return pipeline_suite.compose_preset_bound_suite(
        preset=preset_id,
        preset_text=resolved.text,
        backend=resolved.backend,
        budget_max_iterations=resolved.max_iterations,
        budget_wall_clock_seconds=resolved.max_runtime_seconds,
        project_root_marker="./",
        project_facts=project_facts,
    )


def _read_existing_text(cwd: Path, path: str) -> str | None:
    full = cwd / path
    if not full.is_file():
        return None
    return full.read_text(encoding="utf-8")


def _docs_body_from_facts(facts: audit.ProjectFacts) -> str:
    """Derive the managed-section body for AGENTS.md / CLAUDE.md.

    The body mirrors the audited verification commands so both docs
    carry identical, project-backed guidance; the audit never invents
    commands, so an unknown stack carries a single discovery note
    instead of a fabricated gate. The body deliberately depends only
    on project facts (not on the preset or budgets) so reruns with
    identical project state stay byte-stable noops.
    """
    lines: list[str] = []
    if facts.lint:
        lines.append(f"linter: {facts.lint[0]}")
    if facts.test:
        lines.append(f"test_runner: {facts.test[0]}")
    if not lines:
        lines.append(
            "verification: no authoritative verification command was "
            "discovered; inspect project documentation and CI before "
            "choosing a gate"
        )
    return "\n".join(lines) + "\n"


def _compose_managed_docs(
    *,
    cwd: Path,
    docs_body: str,
) -> tuple[agent_docs.ComposeResult, agent_docs.ComposeResult, str, str]:
    """Compose both managed docs against the on-disk mirrors.

    Each side is composed with ``sync_with_other_doc=True`` against
    the OTHER doc's existing text, so any state in which the two
    mirrors disagree with each other — or with the body this run
    wants to write — surfaces ``sync_mirror_conflict`` before the
    batch is staged. Both docs always receive the identical body; an
    asymmetric on-disk pair is a blocker the operator must reconcile.

    Returns ``(agents_result, claude_result, agents_name, claude_name)``.
    """
    agents_name, claude_name = audit.DEFAULT_AGENTS_NAMES
    agents_existing = _read_existing_text(cwd, agents_name)
    claude_existing = _read_existing_text(cwd, claude_name)
    agents_result = agent_docs.compose_agent_docs(
        agents_existing,
        docs_body,
        marker_id=_DOCS_MARKER_ID,
        sync_with_other_doc=True,
        other_existing_text=claude_existing,
    )
    claude_result = agent_docs.compose_agent_docs(
        claude_existing,
        docs_body,
        marker_id=_DOCS_MARKER_ID,
        sync_with_other_doc=True,
        other_existing_text=agents_existing,
    )
    return agents_result, claude_result, agents_name, claude_name


def _run_generation_stage(
    *,
    cwd: Path,
    resolved: ResolvedPreset,
    refresh_existing: bool,
    facts: audit.ProjectFacts,
) -> tuple[
    PipelineResult,
    pipeline_suite.PresetBoundSuite | None,
    _GenerationOutcome | None,
]:
    """Compose + atomic-write the preset-bound suite AND the managed
    AGENTS.md / CLAUDE.md sections as ONE batch.

    Returns ``(partial_result, suite_or_none, outcome_or_none)``.
    ``partial_result`` carries the blocker view on failure; on success
    ``outcome`` records the per-artifact dispositions so the caller
    can build ``files_created`` / ``files_updated`` / ``files_noop``
    and reopen-verify exactly the artifacts that were committed.
    """
    preset_id = resolved.preset_id
    try:
        suite = _compose_preset_bound(cwd=cwd, resolved=resolved, preset_id=preset_id)
    except pipeline_suite.OwnedYamlError as exc:
        return _make_blocker(
            stage="generation",
            code=exc.code,
            message=exc.reason or exc.code,
        ), None, None

    try:
        existing_config = _read_existing_text(cwd, suite.config_path)
        existing_prompt = _read_existing_text(cwd, suite.prompt_path)
    except (OSError, UnicodeDecodeError) as exc:
        # An existing preset-bound artifact that cannot be read /
        # decoded is corrupt on-disk text: provenance cannot be
        # established, so reconcile cannot proceed. Block typed before
        # any write instead of leaking a bare traceback.
        return _make_blocker(
            stage="reconcile",
            code="provenance_corrupt",
            message=f"existing preset-bound artifact is unreadable or not UTF-8: {exc}",
        ), suite, None
    apply = pipeline_suite.reconcile_preset_bound_suite(
        existing_config, existing_prompt, suite
    )
    if apply.is_blocker:
        return _make_blocker(
            stage="reconcile",
            code=apply.code,
            message=apply.reason or apply.code,
        ), suite, None

    docs_body = _docs_body_from_facts(facts)
    try:
        agents_result, claude_result, agents_name, claude_name = _compose_managed_docs(
            cwd=cwd, docs_body=docs_body
        )
    except (OSError, UnicodeDecodeError) as exc:
        # An existing AGENTS.md / CLAUDE.md that cannot be read /
        # decoded is corrupt on-disk text the doc compose cannot
        # reason about; block the whole batch typed before any write.
        return _make_blocker(
            stage="generation",
            code="provenance_corrupt",
            message=f"existing agent docs are unreadable or not UTF-8: {exc}",
        ), suite, None
    for doc_result in (agents_result, claude_result):
        if doc_result.is_blocker:
            return _make_blocker(
                stage="generation",
                code=doc_result.code,
                message=doc_result.reason or doc_result.code,
            ), suite, None

    write_suite = apply.kind != "noop" and (
        refresh_existing or apply.kind in {"created", "updated"}
    )

    config_path = cwd / suite.config_path
    prompt_path = cwd / suite.prompt_path
    operations: list[tuple[Path, str]] = []
    if write_suite:
        operations.extend(
            [
                (config_path, suite.config),
                (prompt_path, suite.prompt),
            ]
        )
    if agents_result.kind in {"created", "updated"}:
        operations.append((cwd / agents_name, agents_result.text or ""))
    if claude_result.kind in {"created", "updated"}:
        operations.append((cwd / claude_name, claude_result.text or ""))

    suite_kind = apply.kind if write_suite else "noop"
    outcome = _GenerationOutcome(
        suite_kind=suite_kind,
        agents_kind=agents_result.kind,
        claude_kind=claude_result.kind,
        docs_body=docs_body,
        marker_id=_DOCS_MARKER_ID,
        written_docs=tuple(
            name
            for name, doc_result in (
                (agents_name, agents_result),
                (claude_name, claude_result),
            )
            if doc_result.kind in {"created", "updated"}
        ),
    )
    if not operations:
        # Operator did not request a refresh and there is nothing new
        # to write: a clean second run short-circuits to noop.
        return _ok_partial(), suite, outcome

    for rel in (suite.config_path, suite.prompt_path, agents_name, claude_name):
        if not _paths.contain(rel, cwd):
            return _make_blocker(
                stage="generation",
                code="input_path_unsafe",
                message="derived artifact paths escape the project root",
            ), suite, None
    try:
        with agent_docs.AtomicWriter(operations) as writer:
            committed, rolled = writer.execute()
    except OSError as exc:
        return _make_blocker(
            stage="generation",
            code="atomic_write_failed",
            message=str(exc),
        ), suite, None
    if rolled or set(committed) != {target for target, _ in operations}:
        rolled_paths = tuple(str(p) for p in rolled)
        return _make_blocker(
            stage="generation",
            code="atomic_write_failed",
            message=f"atomic write rolled back: {rolled_paths}",
            blocker_paths=rolled_paths,
        ), suite, None
    return _ok_partial(), suite, outcome


def _run_post_write_verify(
    *,
    cwd: Path,
    suite: pipeline_suite.PresetBoundSuite,
    outcome: _GenerationOutcome,
) -> PipelineResult:
    """Reopen the written artifacts and verify binding + provenance.

    The suite files are verified through
    ``pipeline_suite.verify_preset_bound_files``; every managed doc
    committed by this run is reopened and must parse as exactly one
    well-formed managed section whose body byte-equals the requested
    body (a noop recompose proves both).
    """
    verify = pipeline_suite.verify_preset_bound_files(cwd, suite)
    if verify.is_blocker:
        return _make_blocker(
            stage="reconcile",
            code=verify.code,
            message=verify.reason or verify.code,
        )
    for name in outcome.written_docs:
        text = _read_existing_text(cwd, name)
        if text is None:
            return _make_blocker(
                stage="reconcile",
                code="managed_section_stale",
                message=f"{name} disappeared after the atomic write",
            )
        parse = agent_docs.parse_managed_section(text, outcome.marker_id)
        if not parse.is_ok:
            return _make_blocker(
                stage="reconcile",
                code="managed_section_stale",
                message=f"{name} managed section is not well-formed after write",
            )
        recompose = agent_docs.compose_agent_docs(
            text, outcome.docs_body, marker_id=outcome.marker_id
        )
        if recompose.kind != "noop":
            return _make_blocker(
                stage="reconcile",
                code="managed_section_stale",
                message=f"{name} managed section does not match the requested body",
            )
    return _ok_partial()


# ---------------------------------------------------------------------------
# Static validation stage
# ---------------------------------------------------------------------------


def _run_static_stage(
    *,
    binary: str,
    config_path: str,
    preset: str,
    runner: SubprocessRunner,
    prompt_file: str | None = None,
    plan_path: str | None = None,
) -> tuple[PipelineResult, tuple[Any, ...]]:
    """Invoke ``cli_probe.validate_pipeline`` and classify the result.

    ``prompt_file`` / ``plan_path`` forward the suite's source tokens so
    the dry-run stage emits ``--prompt-file`` / ``--plan`` and proves
    the effective ``Prompt file`` label against the suite; a mismatch
    surfaces as ``blocked_command`` instead of a silent pass.
    """
    decisions = cli_probe.validate_pipeline(
        binary=binary,
        config_path=config_path,
        preset=preset,
        prompt_file=prompt_file,
        plan_path=plan_path,
        runner=runner,
    )
    evidence: list[str] = []
    blocker: PipelineResult | None = None
    for decision in decisions:
        evidence.append(f"{decision.stage}:{decision.outcome}")
        if decision.outcome != "ok" and blocker is None:
            blocker = _make_blocker(
                stage="static_validation",
                code=decision.outcome,
                message=decision.blocked_reason or decision.outcome,
            )
    if blocker is not None:
        # Keep ``validation_evidence`` populated so callers can still
        # inspect which stage fired the block.
        return _replace_evidence(blocker, evidence), decisions
    return _replace_evidence(_ok_partial(), evidence), decisions


# ---------------------------------------------------------------------------
# Smoke stage
# ---------------------------------------------------------------------------


def _run_smoke_stage(
    *,
    backend: smoke_runner.SafeBackend | smoke_runner.UnsafeBackend | None,
    resolved_backend: str,
    binary: str,
    config_path: str,
    preset: str,
    plan_path: str | None,
    prompt_file: str | None,
    runner: SubprocessRunner,
    smoke_result_override: smoke_runner.SmokeResult | None = None,
) -> tuple[smoke_runner.SmokeResult | None, PipelineResult]:
    """Run the bounded smoke harness against ``backend``.

    Returns ``(smoke_result, partial_result)``.

    Authorization is decided BEFORE any subprocess is constructed and
    BEFORE ``smoke_result_override`` is honoured: the preset's RESOLVED
    backend (``cli.backend`` from preset resolution) must equal the
    single auto-authorised kind ``content_fixed_replay``. The smoke
    backend's capability label alone can never authorise a spawn — a
    preset resolving to a real backend gets a typed
    ``SmokeResult(outcome="not_authorized")`` with empty argv, so the
    handoff level can never be promoted to ``complete`` through a
    mislabelled smoke. ``smoke_result_override`` still bypasses the
    harness (test seam for typed failures) but only on setups that
    already passed the resolved-backend check.
    """
    if backend is None:
        return None, _ok_partial()
    if resolved_backend != smoke_runner.SAFE_BACKEND_KIND:
        smoke_result = smoke_runner.SmokeResult(
            outcome="not_authorized",
            evidence=(
                f"preset resolves to backend {resolved_backend!r}; only "
                f"{smoke_runner.SAFE_BACKEND_KIND!r} is auto-authorised "
                f"for bounded smoke",
                "refused before any subprocess was constructed",
            ),
            argv=(),
            stderr_excerpt="",
            stdout_excerpt="",
            elapsed_seconds=0.0,
            failure_bucket="none",
        )
        return smoke_result, _ok_partial()
    if smoke_result_override is not None:
        smoke_result = smoke_result_override
    else:
        smoke_cfg = smoke_runner.SmokeConfig(
            binary=Path(binary),
            config_path=config_path,
            preset=preset,
            prompt_file=prompt_file,
            plan_path=plan_path,
            max_iterations=3,
        )
        # The replay transcript the operator staged via
        # ``--replay-transcript`` rides on the SafeBackend capability
        # token; hand it to the harness so the authorised path records
        # which transcript the smoke was staged against.
        transcript_dir = getattr(backend, "transcript_path", None)
        smoke_result = smoke_runner.run_smoke(
            backend, smoke_cfg, transcript_dir=transcript_dir, runner=runner
        )
    if smoke_result.outcome not in smoke_runner.OUTCOMES:
        return smoke_result, _make_blocker(
            stage="smoke",
            code="smoke_outcome_unknown",
            message=f"smoke harness emitted unknown outcome {smoke_result.outcome!r}",
        )
    return smoke_result, _ok_partial()


# ---------------------------------------------------------------------------
# Handoff stage
# ---------------------------------------------------------------------------


def _build_handoff_args(
    *,
    binary: str,
    config_path: str,
    preset: str,
    use_worktree: bool,
    reuse_worktree: bool,
    plan_arg: str | None,
    worktree_name: str | None,
    plan_path: str | None,
    prompt_file: str | None,
    requires_plan: bool,
) -> handoff_module.HandoffInputs:
    """Compose the :class:`handoff.HandoffInputs` for the final stage."""
    return handoff_module.HandoffInputs(
        binary=binary,
        config_path=config_path,
        preset=preset,
        plan_path=plan_path,
        prompt_file=prompt_file,
        level="incomplete_static_only",  # placeholder; _enforce_typed_outcome reconciles
        requires_plan=requires_plan,
        use_worktree=use_worktree,
        reuse_worktree=reuse_worktree,
        plan_arg=plan_arg,
        worktree_name=worktree_name,
    )


def _level_for_smoke(outcome: str | None) -> str:
    """Map a typed smoke outcome to the handoff level.

    Uses the same classification the handoff helper applies so the two
    layers cannot disagree: bounded terminal is ``complete``; a refused
    (not-run) smoke is ``incomplete_static_only``; a timeout / non-zero /
    error-event outcome is ``blocked``; anything else stays static-only.
    """
    if outcome is None:
        return "incomplete_static_only"
    if outcome in handoff_module.SMOKE_COMPLETE_OUTCOMES:
        return "complete"
    if outcome in handoff_module.SMOKE_NOT_RUN_OUTCOMES:
        return "incomplete_static_only"
    if outcome in handoff_module.SMOKE_BLOCKED_OUTCOMES:
        return "blocked"
    return "incomplete_static_only"


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


def _ok_partial() -> PipelineResult:
    return PipelineResult(
        level="incomplete_static_only",
        blocked=False,
        stage="audit",
        code="",
        message="",
    )


def _make_blocker(
    *,
    stage: str,
    code: str,
    message: str,
    blocker_paths: tuple[str, ...] = (),
) -> PipelineResult:
    return PipelineResult(
        level="blocked",
        blocked=True,
        stage=stage,
        code=code,
        message=message,
        blocker_paths=blocker_paths,
    )


def _replace_evidence(result: PipelineResult, evidence: Iterable[str]) -> PipelineResult:
    """Return a copy of ``result`` with ``validation_evidence`` populated."""
    return PipelineResult(
        level=result.level,
        blocked=result.blocked,
        stage=result.stage,
        code=result.code,
        message=result.message,
        root=result.root,
        preset=result.preset,
        config_path=result.config_path,
        prompt_path=result.prompt_path,
        files_created=result.files_created,
        files_updated=result.files_updated,
        files_noop=result.files_noop,
        validation_evidence=tuple(evidence),
        stage_decisions=result.stage_decisions,
        smoke_outcome=result.smoke_outcome,
        smoke_argv=result.smoke_argv,
        smoke_failure_bucket=result.smoke_failure_bucket,
        smoke_evidence=result.smoke_evidence,
        handoff_command=result.handoff_command,
        handoff_report=result.handoff_report,
        handoff_argv=result.handoff_argv,
        next_action=result.next_action,
        blocker_paths=result.blocker_paths,
        resolved_preset=result.resolved_preset,
    )


def _attach_fields(
    result: PipelineResult,
    *,
    root: str = "",
    preset: str = "",
    config_path: str = "",
    prompt_path: str = "",
    files_created: tuple[str, ...] = (),
    files_updated: tuple[str, ...] = (),
    files_noop: tuple[str, ...] = (),
    validation_evidence: tuple[str, ...] = (),
    stage_decisions: tuple[Any, ...] = (),
    smoke_outcome: str | None = None,
    smoke_argv: tuple[str, ...] = (),
    smoke_failure_bucket: str | None = None,
    smoke_evidence: tuple[str, ...] = (),
    handoff_command: str = "",
    handoff_report: str = "",
    handoff_argv: tuple[str, ...] = (),
    next_action: str = "",
    resolved_preset: ResolvedPreset | None = None,
) -> PipelineResult:
    """Return a copy of ``result`` with the supplied fields populated."""
    return PipelineResult(
        level=result.level,
        blocked=result.blocked,
        stage=result.stage,
        code=result.code,
        message=result.message,
        root=root,
        preset=preset,
        config_path=config_path,
        prompt_path=prompt_path,
        files_created=files_created or result.files_created,
        files_updated=files_updated or result.files_updated,
        files_noop=files_noop or result.files_noop,
        validation_evidence=validation_evidence or result.validation_evidence,
        stage_decisions=stage_decisions or result.stage_decisions,
        smoke_outcome=smoke_outcome,
        smoke_argv=smoke_argv,
        smoke_failure_bucket=smoke_failure_bucket,
        smoke_evidence=smoke_evidence or result.smoke_evidence,
        handoff_command=handoff_command,
        handoff_report=handoff_report,
        handoff_argv=handoff_argv,
        next_action=next_action,
        blocker_paths=result.blocker_paths,
        resolved_preset=resolved_preset if resolved_preset is not None else result.resolved_preset,
    )


# ---------------------------------------------------------------------------
# Public entry point
# ---------------------------------------------------------------------------


def run_pipeline(
    *,
    cwd: Path | str,
    preset: str,
    plan_path: str | None = None,
    prompt_file: str | None = None,
    binary: str = "ralph",
    refresh_existing: bool = False,
    use_worktree: bool = False,
    reuse_worktree: bool = False,
    plan_arg: str | None = None,
    worktree_name: str | None = None,
    runner: SubprocessRunner | None = None,
    smoke_backend: smoke_runner.SafeBackend | smoke_runner.UnsafeBackend | None = None,
    smoke_result_override: smoke_runner.SmokeResult | None = None,
) -> PipelineResult:
    """Execute the unified bootstrap pipeline.

    Parameters mirror the public contract from plan
    ``2026-08-03-005-refactor-project-bootstrap-skill-plan``. Tests
    inject a deterministic ``runner`` so the real ``ralph`` binary
    is never spawned; production callers omit ``runner`` to inherit
    :func:`subprocess.run`.
    """
    active_runner: SubprocessRunner = runner if runner is not None else subprocess.run
    cwd_path, preset_clean, plan_path, prompt_file, input_code = _normalize_inputs(
        cwd=cwd, preset=preset, plan_path=plan_path, prompt_file=prompt_file
    )
    if input_code:
        return _make_blocker(
            stage="audit",
            code=input_code,
            message=f"input rejected: {input_code}",
        )

    # --- audit stage ----------------------------------------------------
    audit_decision = audit.run_audit(
        cwd_path,
        preset=preset_clean,
        plan_path=plan_path,
        prompt_file=prompt_file,
    )
    if audit_decision.is_blocking:
        first = audit_decision.issues[0] if audit_decision.issues else None
        return _make_blocker(
            stage="audit",
            code=first.code if first else "audit_blocked",
            message=first.message if first else "audit blocked",
        )

    # --- preset resolution stage ---------------------------------------
    # The audit stage validated the preset against the audit-resolved
    # root; resolution MUST read from the same canonical anchor so
    # "exists at root R" implies "reads from root R" — even when the
    # pipeline runs from a subdirectory and the bare cwd differs from
    # the project root.
    preset_root = (cwd_path / (audit_decision.root or "./")).resolve()
    try:
        resolved = _resolve_preset(
            root=preset_root,
            preset=preset_clean,
            binary=binary,
            runner=active_runner,
        )
    except ValueError as exc:
        code, reason = _typed_blocker_payload(exc)
        return _make_blocker(
            stage="preset_resolution",
            code=code,
            message=reason,
        )

    if not resolved.inline_prompt_present and not plan_path and not prompt_file:
        return _make_blocker(
            stage="preset_resolution",
            code="preset_prompt_missing",
            message="preset has no inline event_loop.prompt; supply --plan or --prompt-file",
        )

    # --- generation stage ----------------------------------------------
    partial, suite, outcome = _run_generation_stage(
        cwd=cwd_path,
        resolved=resolved,
        refresh_existing=refresh_existing,
        facts=audit_decision.facts,
    )
    if partial.blocked:
        return _attach_fields(
            partial,
            root=audit_decision.root or "./",
            preset=preset_clean,
            resolved_preset=resolved,
            next_action="reconcile on-disk owned artifacts before retrying",
        )
    assert suite is not None and outcome is not None

    # --- post-write verify (reopen + provenance) -----------------------
    verify_partial = _run_post_write_verify(cwd=cwd_path, suite=suite, outcome=outcome)
    if verify_partial.blocked:
        return _attach_fields(
            verify_partial,
            root=audit_decision.root or "./",
            preset=preset_clean,
            config_path=suite.config_path,
            prompt_path=suite.prompt_path,
            resolved_preset=resolved,
        )

    files_created: list[str] = []
    files_updated: list[str] = []
    files_noop: list[str] = []
    dispositions = (
        (suite.config_path, outcome.suite_kind),
        (suite.prompt_path, outcome.suite_kind),
        (audit.DEFAULT_AGENTS_NAMES[0], outcome.agents_kind),
        (audit.DEFAULT_AGENTS_NAMES[1], outcome.claude_kind),
    )
    for path, kind in dispositions:
        if kind == "created":
            files_created.append(path)
        elif kind == "updated":
            files_updated.append(path)
        else:
            files_noop.append(path)

    # --- static validation stage ---------------------------------------
    static_partial, decisions = _run_static_stage(
        binary=binary,
        config_path=suite.config_path,
        preset=preset_clean,
        runner=active_runner,
        prompt_file=suite.prompt_path,
        plan_path=plan_path,
    )
    validation_evidence: tuple[str, ...] = static_partial.validation_evidence

    if static_partial.blocked:
        return _attach_fields(
            static_partial,
            root=audit_decision.root or "./",
            preset=preset_clean,
            config_path=suite.config_path,
            prompt_path=suite.prompt_path,
            files_created=tuple(files_created),
            files_updated=tuple(files_updated),
            files_noop=tuple(files_noop),
            validation_evidence=validation_evidence,
            stage_decisions=decisions,
            resolved_preset=resolved,
        )

    # --- smoke stage ---------------------------------------------------
    smoke_result, smoke_partial = _run_smoke_stage(
        backend=smoke_backend,
        resolved_backend=resolved.backend,
        binary=binary,
        config_path=suite.config_path,
        preset=preset_clean,
        plan_path=plan_path,
        prompt_file=suite.prompt_path,
        runner=active_runner,
        smoke_result_override=smoke_result_override,
    )
    if smoke_partial.blocked:
        return _attach_fields(
            smoke_partial,
            root=audit_decision.root or "./",
            preset=preset_clean,
            config_path=suite.config_path,
            prompt_path=suite.prompt_path,
            files_created=tuple(files_created),
            files_updated=tuple(files_updated),
            files_noop=tuple(files_noop),
            validation_evidence=validation_evidence,
            stage_decisions=decisions,
            resolved_preset=resolved,
        )

    smoke_outcome = smoke_result.outcome if smoke_result is not None else None
    smoke_argv = smoke_result.argv if smoke_result is not None else ()
    smoke_bucket = smoke_result.failure_bucket if smoke_result is not None else None
    smoke_evidence = smoke_result.evidence if smoke_result is not None else ()

    # --- handoff stage -------------------------------------------------
    # A missing first-run plan must NOT block provisioning: when the
    # caller supplied neither a plan nor a prompt file (and the launch
    # is not worktree-bound, where the reuse key governs), the
    # handoff command carries the ``--plan PLAN_PATH`` template so the
    # operator fills in the real plan path. The pipeline never invents
    # a plan file on disk.
    requires_plan = plan_path is None and prompt_file is None and not use_worktree
    inputs = _build_handoff_args(
        binary=binary,
        config_path=suite.config_path,
        preset=preset_clean,
        use_worktree=use_worktree,
        reuse_worktree=reuse_worktree,
        plan_arg=plan_arg,
        worktree_name=worktree_name,
        plan_path=plan_path,
        prompt_file=prompt_file,
        requires_plan=requires_plan,
    )
    level = _level_for_smoke(smoke_outcome)
    # A blocked handoff requires a non-empty blocker_summary at construction
    # time; surface the typed smoke outcome + bucket so the operator can see
    # why the run stopped.
    blocker_summary = ""
    if level == "blocked":
        bucket = smoke_bucket or "unknown"
        blocker_summary = (
            f"bounded smoke returned {smoke_outcome!r} "
            f"(failure bucket: {bucket}); reconcile before launch"
        )
    inputs = handoff_module.HandoffInputs(
        binary=inputs.binary,
        config_path=inputs.config_path,
        preset=inputs.preset,
        plan_path=inputs.plan_path,
        prompt_file=inputs.prompt_file,
        level=level,  # type: ignore[arg-type]
        requires_plan=inputs.requires_plan,
        use_worktree=inputs.use_worktree,
        reuse_worktree=inputs.reuse_worktree,
        plan_arg=inputs.plan_arg,
        worktree_name=inputs.worktree_name,
        files_created=tuple(files_created),
        files_updated=tuple(files_updated),
        files_noop=tuple(files_noop),
        validation_evidence=validation_evidence,
        smoke_evidence=smoke_evidence,
        smoke_outcome=smoke_outcome,
        smoke_failure_bucket=smoke_bucket,
        blocker_summary=blocker_summary,
    )
    artifact = handoff_module.build_handoff(inputs)
    final_level = artifact.level
    blocked = final_level == "blocked"
    return PipelineResult(
        level=final_level,
        blocked=blocked,
        stage="handoff",
        code="",
        message=artifact.blocker_summary,
        root=audit_decision.root or "./",
        preset=preset_clean,
        config_path=suite.config_path,
        prompt_path=suite.prompt_path,
        files_created=tuple(files_created),
        files_updated=tuple(files_updated),
        files_noop=tuple(files_noop),
        validation_evidence=validation_evidence,
        stage_decisions=decisions,
        smoke_outcome=smoke_outcome,
        smoke_argv=smoke_argv,
        smoke_failure_bucket=smoke_bucket,
        smoke_evidence=smoke_evidence,
        handoff_command=artifact.command,
        handoff_report=artifact.report,
        handoff_argv=artifact.command_argv,
        next_action="",
        resolved_preset=resolved,
    )


# ---------------------------------------------------------------------------
# CLI plumbing
# ---------------------------------------------------------------------------


def render_cli_json(result: PipelineResult) -> str:
    """Return the canonical JSON view of ``result``."""
    return result.to_json()


def _render_cli_text(result: PipelineResult) -> str:
    """Render the default text view of ``result`` for terminal output."""
    lines: list[str] = []
    lines.append(f"level: {result.level}")
    lines.append(f"stage: {result.stage}")
    if result.code:
        lines.append(f"code: {result.code}")
    if result.message:
        lines.append(f"message: {result.message}")
    if result.root:
        lines.append(f"root: {result.root}")
    if result.preset:
        lines.append(f"preset: {result.preset}")
    if result.config_path:
        lines.append(f"config: {result.config_path}")
    if result.prompt_path:
        lines.append(f"prompt: {result.prompt_path}")
    if result.files_created:
        lines.append("files_created:")
        for path in result.files_created:
            lines.append(f"  - {path}")
    if result.files_updated:
        lines.append("files_updated:")
        for path in result.files_updated:
            lines.append(f"  - {path}")
    if result.files_noop:
        lines.append("files_noop:")
        for path in result.files_noop:
            lines.append(f"  - {path}")
    if result.validation_evidence:
        lines.append("validation:")
        for entry in result.validation_evidence:
            lines.append(f"  - {entry}")
    if result.smoke_outcome:
        lines.append(f"smoke_outcome: {result.smoke_outcome}")
        if result.smoke_failure_bucket:
            lines.append(f"smoke_failure_bucket: {result.smoke_failure_bucket}")
    if result.handoff_command:
        lines.append("command:")
        lines.append(result.handoff_command)
    elif result.level == "blocked":
        lines.append("command: <blocked>")
    if result.next_action:
        lines.append(f"next_action: {result.next_action}")
    return "\n".join(lines)


def build_cli_parser() -> argparse.ArgumentParser:
    """Build the argparse CLI for the pipeline entry point."""
    parser = argparse.ArgumentParser(
        prog="bootstrap_pipeline",
        description=(
            "Run the unified ralph-project-bootstrap pipeline against a "
            "target project. Produces a structured PipelineResult on stdout."
        ),
    )
    parser.add_argument("--cwd", default=".", help="Target project cwd (default: current dir).")
    parser.add_argument(
        "--preset",
        required=True,
        help="Repo-relative preset YAML or builtin id (builtin:<id>).",
    )
    parser.add_argument("--plan", help="Repo-relative plan path.")
    parser.add_argument("--prompt-file", help="Repo-relative prompt file path.")
    parser.add_argument(
        "--binary",
        default="ralph",
        help="Path or name of the ralph binary (default: ralph).",
    )
    parser.add_argument(
        "--refresh-existing",
        action="store_true",
        help="Overwrite an existing preset-bound suite when provenance matches.",
    )
    parser.add_argument(
        "--replay-transcript",
        help=(
            "Repo-relative path (anchored on --cwd) to a "
            "content_fixed_replay transcript dir; enables the SafeBackend "
            "smoke path. Absolute paths and .. escapes are rejected."
        ),
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Print the PipelineResult as JSON instead of the default text view.",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_cli_parser()
    args = parser.parse_args(argv)
    smoke_backend: smoke_runner.SafeBackend | smoke_runner.UnsafeBackend | None = None
    result: PipelineResult | None = None
    if args.replay_transcript:
        # The transcript token passes the SAME repo-relative input gate
        # family as plan/prompt/preset: absolute paths, ``..`` escapes
        # and control bytes are rejected typed BEFORE any pipeline work.
        # Safe tokens are anchored on ``--cwd`` so the staged transcript
        # the harness consumes is exactly the one the audit validated.
        if not _paths.is_safe_relative(args.replay_transcript):
            result = PipelineResult(
                level="blocked",
                blocked=True,
                stage="audit",
                code="input_path_unsafe",
                message="input rejected: input_path_unsafe",
            )
        else:
            smoke_backend = smoke_runner.SafeBackend(
                name="replay",
                kind="content_fixed_replay",
                transcript_path=(Path(args.cwd) / args.replay_transcript).resolve(),
            )
    if result is None:
        try:
            result = run_pipeline(
                cwd=args.cwd,
                preset=args.preset,
                plan_path=args.plan,
                prompt_file=args.prompt_file,
                binary=args.binary,
                refresh_existing=args.refresh_existing,
                smoke_backend=smoke_backend,
            )
        except ValueError as exc:
            # The handoff helper rejects malformed launch inputs at
            # construction time (e.g. a worktree run without an explicit
            # reuse key) by raising ``ValueError``. Render the rejection
            # as a blocked-shaped result instead of a traceback so the
            # CLI contract holds: blocked always exits 2 with a typed
            # code the operator can act on. No business logic is
            # duplicated — ``run_pipeline`` remains the only layer that
            # classifies pipeline stages.
            message = str(exc)
            code = (
                "worktree_reuse_key_missing"
                if "worktree reuse key" in message
                else "handoff_inputs_rejected"
            )
            result = PipelineResult(
                level="blocked",
                blocked=True,
                stage="handoff",
                code=code,
                message=message,
            )
    if args.json:
        sys.stdout.write(render_cli_json(result))
        sys.stdout.write("\n")
    else:
        sys.stdout.write(_render_cli_text(result))
        sys.stdout.write("\n")
    if result.level == "blocked":
        return 2
    return 0


__all__ = (
    "HANDOFF_LEVELS",
    "PipelineResult",
    "ResolvedPreset",
    "build_cli_parser",
    "main",
    "render_cli_json",
    "run_pipeline",
)


if __name__ == "__main__":  # pragma: no cover - CLI entry point
    raise SystemExit(main())