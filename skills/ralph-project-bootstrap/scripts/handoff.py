"""Handoff builder for ``ralph-project-bootstrap``.

This module renders the final handoff an operator consumes when the
bootstrap pipeline finishes. The handoff bundles:

* the **items** the bootstrap pipeline created / updated / left alone
  in the target project,
* the **validation evidence** the static gate recorded (Unit 5),
* the **smoke evidence** the bounded smoke harness recorded (Unit 6),
* the **residual risks** the operator must re-confirm by hand,
* and the **official launch command** the operator runs from the
  target-project root.

Three handoff levels are supported:

* ``complete`` — U1-U5 all green AND a SafeBackend smoke reached
  ``bounded_terminal_reached`` (a *typed* outcome, NOT a free-text
  match). The launch command is the official command the operator may
  paste into a terminal.
* ``incomplete_static_only`` — U1-U5 all green, U6 smoke either not
  authorised or not run. The launch command is presented as a
  ``[CANDIDATE - operator must run manually]`` snippet and the
  report explicitly states "static load passed; loop not closed".
* ``blocked`` — any earlier unit returned a blocker, OR the typed
  smoke outcome is one of the timeout / non-zero / error-event
  buckets. The launch command is the empty string and the report
  states why.

Hard rules:

* Pure stdlib. No third-party imports.
* No preset name is hard-coded. Tests construct inputs with arbitrary
  preset ids (``test-preset``, etc.).
* The helper does not import or reference any other Ralph skill
  package — preset authoring, hat operations, and runtime CLI
  invocations all live elsewhere.
* No env vars are read or set by the helper.
* All paths in the rendered report are repo-relative — absolute paths
  are rejected with ``ValueError`` at the API boundary.
* Worktree mode requires an explicit reuse key — either ``--plan
  <plan>`` or ``--worktree-name <name>``. Missing keys are rejected.
* The level is computed from **typed** inputs (smoke outcome / smoke
  failure bucket / blocker summary). Free-text evidence strings
  (e.g. ``"bounded_terminal_reached"`` smuggled into
  ``smoke_evidence``) MUST NOT be sufficient to reach ``complete``
  — the level must come from a typed ``smoke_outcome`` field
  populated by the smoke harness.
"""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import PurePosixPath
from typing import Iterable, Literal, Sequence

# Canonical typed smoke outcomes. We re-declare the subset we care
# about rather than importing the helper so handoff.py stays
# decoupled from the harness's specific failure-bucket taxonomy.
SMOKE_COMPLETE_OUTCOMES: frozenset[str] = frozenset({"bounded_terminal_reached"})
SMOKE_NOT_RUN_OUTCOMES: frozenset[str] = frozenset({"not_authorized"})
SMOKE_BLOCKED_OUTCOMES: frozenset[str] = frozenset(
    {
        "wall_clock_timeout",
        "timeout_no_event",
        "timeout_idle",
        "non_zero_exit",
        "error_event_detected",
    }
)

HandoffLevel = Literal["complete", "incomplete_static_only", "blocked"]

HANDOFF_LEVELS: tuple[HandoffLevel, ...] = (
    "complete",
    "incomplete_static_only",
    "blocked",
)


def _ensure_level(level: str) -> HandoffLevel:
    if level not in HANDOFF_LEVELS:
        raise ValueError(
            f"unknown handoff level: {level!r}; must be one of {HANDOFF_LEVELS}"
        )
    return level  # type: ignore[return-value]


@dataclass(frozen=True)
class HandoffInputs:
    """Inputs the bootstrap pipeline hands to ``build_handoff``.

    All path fields are repo-relative. ``binary`` is the path to the
    ``ralph`` binary the operator will run (typically the literal
    string ``"ralph"``).

    Smoke classification contract:

    * ``smoke_outcome`` — typed outcome from ``smoke_runner.SmokeResult``.
      When present, it is the authoritative source of truth for the
      smoke level. ``smoke_evidence`` is then reduced to a debugging
      footnote.
    * ``smoke_failure_bucket`` — typed bucket (``"preset"`` /
      ``"backend"`` / ``"project_command"`` / ``"suite"`` /
      ``"none"``). Used together with ``smoke_outcome`` to render the
      ``blocked -- <bucket>`` status.
    * ``smoke_evidence`` — list of free-text evidence strings. They
      appear in the report's Smoke > Evidence block but MUST NOT be
      the sole reason the level advances to ``complete``.
    """

    binary: str
    config_path: str
    preset: str
    plan_path: str | None
    prompt_file: str | None
    level: HandoffLevel
    requires_plan: bool = False
    use_worktree: bool = False
    reuse_worktree: bool = False
    plan_arg: str | None = None
    worktree_name: str | None = None
    files_created: tuple[str, ...] = ()
    files_updated: tuple[str, ...] = ()
    files_noop: tuple[str, ...] = ()
    validation_evidence: tuple[str, ...] = ()
    smoke_evidence: tuple[str, ...] = ()
    smoke_outcome: str | None = None
    smoke_failure_bucket: str | None = None
    residual_risks: tuple[str, ...] = ()
    blocker_summary: str = ""

    def __post_init__(self) -> None:
        _ensure_level(self.level)
        for label, value in (
            ("binary", self.binary),
            ("config_path", self.config_path),
            ("preset", self.preset),
        ):
            if not isinstance(value, str) or not value.strip():
                raise ValueError(f"{label} must be a non-empty string")
        for label, value in (
            ("plan_path", self.plan_path),
            ("prompt_file", self.prompt_file),
            ("plan_arg", self.plan_arg),
            ("worktree_name", self.worktree_name),
        ):
            if value is not None and (not isinstance(value, str) or not value.strip()):
                raise ValueError(f"{label} must be None or a non-empty string")
        if self.use_worktree:
            if not self.reuse_worktree:
                raise ValueError("worktree reuse key required")
            if self.plan_arg is None and self.worktree_name is None:
                raise ValueError("worktree reuse key required")
        if self.level == "blocked" and not self.blocker_summary.strip():
            raise ValueError("blocker_summary must be non-empty when level='blocked'")
        for label, path in (
            ("config_path", self.config_path),
            ("plan_path", self.plan_path),
            ("prompt_file", self.prompt_file),
            ("plan_arg", self.plan_arg),
            ("worktree_name", self.worktree_name),
        ):
            if path is not None and PurePosixPath(path).is_absolute():
                raise ValueError(f"{label} must be repo-relative: {path!r}")


@dataclass(frozen=True)
class HandoffArtifact:
    """Rendered handoff: structured data plus a Markdown report."""

    level: HandoffLevel
    command: str
    command_argv: tuple[str, ...]
    report: str
    created_files: tuple[str, ...]
    updated_files: tuple[str, ...]
    noop_files: tuple[str, ...]
    validation_summary: tuple[str, ...]
    smoke_summary: tuple[str, ...]
    residual_risks: tuple[str, ...]
    blocker_summary: str
    notes: tuple[str, ...]


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


def _shell_quote(token: str) -> str:
    """Quote a shell token so a copy-paste command is safe."""
    if not token:
        return "''"
    safe_chars = set(
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"
        "0123456789-_./:+=@"
    )
    if all(ch in safe_chars for ch in token):
        return token
    return "'" + token.replace("'", "'\"'\"'") + "'"


def _join_argv(argv: Sequence[str]) -> str:
    return " ".join(_shell_quote(tok) for tok in argv)


def _normalise_paths(items: Iterable[str]) -> tuple[str, ...]:
    """Return the iterable as a tuple, rejecting absolute paths."""
    out: list[str] = []
    for item in items:
        if not isinstance(item, str) or not item.strip():
            raise ValueError("path entries must be non-empty strings")
        if PurePosixPath(item).is_absolute():
            raise ValueError(f"absolute paths not allowed in handoff: {item!r}")
        out.append(item)
    return tuple(out)


def _build_argv(inputs: HandoffInputs) -> tuple[str, ...]:
    """Compose the argv tuple for the launch command.

    Every command starts with ``<binary> -c <config_path> -H <preset> run``.
    Non-worktree launches append exactly one optional prompt source:
    ``--prompt-file`` when supplied, otherwise ``--plan``. Worktree mode
    appends ``--worktree --reuse-worktree`` and
    exactly one of ``--plan <plan_arg>`` or
    ``--worktree-name <worktree_name>``; the optional prompt file remains
    independent of that reuse key.
    """
    argv: list[str] = [
        inputs.binary,
        "-c",
        inputs.config_path,
        "-H",
        inputs.preset,
        "run",
    ]
    # An operator-supplied prompt file is the authoritative prompt source.
    # Only use --plan as the prompt source when no external prompt was given;
    # Ralph gives --prompt-file precedence when both flags are present.
    if inputs.prompt_file is not None:
        argv.append("--prompt-file")
        argv.append(inputs.prompt_file)
    if inputs.use_worktree:
        argv.append("--worktree")
        argv.append("--reuse-worktree")
        if inputs.plan_arg is not None:
            argv.append("--plan")
            argv.append(inputs.plan_arg)
        elif inputs.worktree_name is not None:
            argv.append("--worktree-name")
            argv.append(inputs.worktree_name)
    elif inputs.plan_path is not None and inputs.prompt_file is None:
        argv.append("--plan")
        argv.append(inputs.plan_path)
    elif inputs.requires_plan:
        argv.append("--plan")
        argv.append("PLAN_PATH")
    return tuple(argv)


def _smoke_section_text(inputs: HandoffInputs) -> tuple[str, str]:
    """Return ``(section_body, status_token)`` for the Smoke sub-section.

    ``status_token`` is the canonical short label that downstream
    tooling may switch on: ``complete`` / ``static-only -- smoke-not-authorized``
    / ``blocked -- <bucket>``. The status is computed from **typed**
    inputs first; the free-text ``smoke_evidence`` strings only ever
    appear as a debugging footnote, never as the source of truth.

    Anti-fake-positive contract (plan 2026-07-19-001 S10 / F7): an
    evidence string that merely *contains* the literal
    ``bounded_terminal_reached`` MUST NOT, by itself, advance the
    level to ``complete``. The caller must populate
    ``smoke_outcome="bounded_terminal_reached"`` explicitly.
    """
    typed_outcome = inputs.smoke_outcome
    if typed_outcome is not None:
        if typed_outcome in SMOKE_COMPLETE_OUTCOMES:
            body = (
                "Smoke reached the bounded terminal marker (`LOOP_COMPLETE`). "
                "The end-to-end loop is verified under the triple cap."
            )
            return body, "complete"
        if typed_outcome in SMOKE_NOT_RUN_OUTCOMES:
            body = (
                "Smoke was not authorised by the operator. Static load passed; "
                "loop has not been verified end-to-end."
            )
            return body, "static-only -- smoke-not-authorized"
        if typed_outcome in SMOKE_BLOCKED_OUTCOMES:
            bucket = inputs.smoke_failure_bucket or "unknown"
            body = (
                f"Smoke classified a `{typed_outcome}` failure bucket of "
                f"`{bucket}`. Static load is insufficient; the operator "
                "must reconcile before launch."
            )
            return body, f"blocked -- {bucket}"
        # Unrecognised typed outcome: refuse to climb to ``complete``
        # and surface the unknown class so the operator can decide.
        body = (
            f"Smoke emitted an unrecognised typed outcome `{typed_outcome}`. "
            "Static load passed; loop status is unknown."
        )
        return body, "blocked -- unknown"
    # No typed outcome: downgrade to ``incomplete_static_only``. Even
    # if the free-text evidence happens to contain the
    # ``bounded_terminal_reached`` substring, we MUST NOT advance to
    # ``complete`` — that would let a fake runner smuggle the keyword
    # past the helper. We DO render the free-text evidence as a
    # debugging footnote so the operator can see why we refused.
    combined = "\n".join(inputs.smoke_evidence).lower()
    if not combined.strip():
        body = (
            "Smoke was not authorised. Static load passed; loop has not been "
            "verified end-to-end. Operator must run the candidate command "
            "explicitly after re-confirming the target backend."
        )
        return body, "static-only -- smoke-not-authorized"
    body = (
        "Smoke evidence was provided as free-text strings only — no typed "
        "outcome. Static load passed; loop has not been verified end-to-end. "
        "Run the smoke harness with a typed ``SmokeResult`` so the handoff "
        "can advance to ``complete``."
    )
    return body, "incomplete_static_only"

# ---------------------------------------------------------------------------
# Markdown report rendering
# ---------------------------------------------------------------------------


def _render_table_row(label: str, items: tuple[str, ...]) -> str:
    if not items:
        return f"| {label} | _none_ |"
    lines = [f"| {label} | `{items[0]}` |"]
    for item in items[1:]:
        lines.append(f"|  | `{item}` |")
    return "\n".join(lines)


def _render_items_section(inputs: HandoffInputs) -> str:
    rows = [
        _render_table_row("created", inputs.files_created),
        _render_table_row("updated", inputs.files_updated),
        _render_table_row("noop", inputs.files_noop),
    ]
    return "## Items\n\n| kind | path |\n| --- | --- |\n" + "\n".join(rows)


def _render_validation_section(inputs: HandoffInputs) -> str:
    if not inputs.validation_evidence:
        return "## Validation\n\n_no recorded stages_"
    body = "\n".join(f"- `{line}`" for line in inputs.validation_evidence)
    return f"## Validation\n\n{body}"


def _render_smoke_section(inputs: HandoffInputs) -> str:
    body, status = _smoke_section_text(inputs)
    if inputs.smoke_evidence:
        evidence_block = "\n".join(f"- `{line}`" for line in inputs.smoke_evidence)
        return (
            f"## Smoke\n\n"
            f"Status: `{status}`\n\n"
            f"{body}\n\n"
            f"### Evidence\n\n{evidence_block}"
        )
    return f"## Smoke\n\nStatus: `{status}`\n\n{body}"


def _render_risks_section(inputs: HandoffInputs) -> str:
    if not inputs.residual_risks:
        return "## Residual Risks\n\n_none_"
    body = "\n".join(f"- {line}" for line in inputs.residual_risks)
    return f"## Residual Risks\n\n{body}"


def _render_command_section(command: str) -> str:
    if not command:
        return "## Launch Command\n\n_no executable command -- see blocker above_"
    return f"## Launch Command\n\n```bash\n{command}\n```"


def _render_blocker_section(inputs: HandoffInputs) -> str:
    if not inputs.blocker_summary.strip():
        return ""
    return f"## Blocker\n\n{inputs.blocker_summary.strip()}"


def _render_report(inputs: HandoffInputs, command: str) -> str:
    parts: list[str] = [
        "# Ralph Bootstrap Handoff",
        "",
        f"Level: `{inputs.level}`",
        "",
        _render_blocker_section(inputs),
        _render_items_section(inputs),
        "",
        _render_validation_section(inputs),
        "",
        _render_smoke_section(inputs),
        "",
        _render_risks_section(inputs),
        "",
        _render_command_section(command),
        "",
    ]
    return "\n".join(part for part in parts if part is not None)


# ---------------------------------------------------------------------------
# Public entry point
# ---------------------------------------------------------------------------


def _enforce_typed_outcome(inputs: HandoffInputs) -> tuple[HandoffLevel, str]:
    """Reconcile the caller-supplied ``level`` against the typed
    ``smoke_outcome`` so that a caller cannot lie about evidence.

    Returns ``(effective_level, effective_blocker_summary)``:

    * If the typed outcome is in ``SMOKE_BLOCKED_OUTCOMES`` the handoff
      MUST render as ``blocked`` regardless of the caller's level.
      The blocker summary is auto-populated with the typed outcome
      + bucket when the caller did not supply one.
    * If the typed outcome is in ``SMOKE_NOT_RUN_OUTCOMES`` the
      handoff MUST render as ``incomplete_static_only`` even if the
      caller asked for ``complete``.
    * If the caller asked for ``complete`` but no typed outcome was
      supplied (only free-text evidence), the helper MUST downgrade
      the level to ``incomplete_static_only`` so the launch command
      cannot be presented as "official" / "loop-closed".
    * Otherwise the caller's level is preserved.
    """
    typed_outcome = inputs.smoke_outcome
    if typed_outcome is None:
        if inputs.level == "complete":
            return "incomplete_static_only", inputs.blocker_summary
        return inputs.level, inputs.blocker_summary
    if typed_outcome in SMOKE_BLOCKED_OUTCOMES:
        bucket = inputs.smoke_failure_bucket or "unknown"
        auto_summary = (
            inputs.blocker_summary.strip()
            or f"smoke typed outcome {typed_outcome!r} with bucket {bucket!r}"
        )
        return "blocked", auto_summary
    if typed_outcome in SMOKE_NOT_RUN_OUTCOMES:
        return "incomplete_static_only", inputs.blocker_summary
    return inputs.level, inputs.blocker_summary


def build_handoff(inputs: HandoffInputs) -> HandoffArtifact:
    """Render a handoff from the supplied inputs.

    The function is pure: no filesystem access, no env-var reads, no
    subprocess calls. Callers wire it into the bootstrap pipeline by
    constructing a ``HandoffInputs`` from the audit / validation /
    smoke evidence already on disk.

    The rendered ``command`` is:

    * the official launch command (no prefix) when ``level ==
      "complete"`` AND the typed smoke outcome (when supplied) is
      ``bounded_terminal_reached``,
    * a ``[CANDIDATE - operator must run manually]`` snippet when
      ``level == "incomplete_static_only"``,
    * the empty string when ``level == "blocked"`` (the report
      explains why).

    Anti-fake-positive (F7): the helper refuses to render an
    "official" launch command when the typed smoke outcome contradicts
    the caller's level. The reconciliation happens in
    ``_enforce_typed_outcome`` and is the only path through which the
    level can change after this function is called.
    """
    effective_level, effective_blocker = _enforce_typed_outcome(inputs)
    # Bind the effective level / blocker into a derived inputs view so
    # downstream rendering uses the reconciled values without
    # mutating the caller's dataclass (which is frozen).
    rendered_inputs = HandoffInputs(
        binary=inputs.binary,
        config_path=inputs.config_path,
        preset=inputs.preset,
        plan_path=inputs.plan_path,
        prompt_file=inputs.prompt_file,
        level=effective_level,
        requires_plan=inputs.requires_plan,
        use_worktree=inputs.use_worktree,
        reuse_worktree=inputs.reuse_worktree,
        plan_arg=inputs.plan_arg,
        worktree_name=inputs.worktree_name,
        files_created=inputs.files_created,
        files_updated=inputs.files_updated,
        files_noop=inputs.files_noop,
        validation_evidence=inputs.validation_evidence,
        smoke_evidence=inputs.smoke_evidence,
        smoke_outcome=inputs.smoke_outcome,
        smoke_failure_bucket=inputs.smoke_failure_bucket,
        residual_risks=inputs.residual_risks,
        blocker_summary=effective_blocker,
    )

    # Blocked → no executable command.
    if rendered_inputs.level == "blocked":
        return HandoffArtifact(
            level="blocked",
            command="",
            command_argv=(),
            report=_render_report(rendered_inputs, ""),
            created_files=_normalise_paths(rendered_inputs.files_created),
            updated_files=_normalise_paths(rendered_inputs.files_updated),
            noop_files=_normalise_paths(rendered_inputs.files_noop),
            validation_summary=tuple(rendered_inputs.validation_evidence),
            smoke_summary=tuple(rendered_inputs.smoke_evidence),
            residual_risks=tuple(rendered_inputs.residual_risks),
            blocker_summary=rendered_inputs.blocker_summary.strip(),
            notes=("blocked: no executable command",),
        )

    argv = _build_argv(rendered_inputs)
    raw_command = _join_argv(argv)

    notes: tuple[str, ...]
    command: str
    if rendered_inputs.level == "incomplete_static_only":
        if rendered_inputs.requires_plan and rendered_inputs.plan_path is None:
            notes = ("incomplete: fill PLAN_PATH before running",)
            command = f"[TEMPLATE - replace PLAN_PATH before running]\n{raw_command}"
        else:
            notes = ("incomplete: candidate command only; operator must run manually",)
            command = f"[CANDIDATE - operator must run manually]\n{raw_command}"
    else:  # complete
        notes = ("complete: official launch command",)
        command = raw_command

    return HandoffArtifact(
        level=rendered_inputs.level,
        command=command,
        command_argv=argv,
        report=_render_report(rendered_inputs, command),
        created_files=_normalise_paths(rendered_inputs.files_created),
        updated_files=_normalise_paths(rendered_inputs.files_updated),
        noop_files=_normalise_paths(rendered_inputs.files_noop),
        validation_summary=tuple(rendered_inputs.validation_evidence),
        smoke_summary=tuple(rendered_inputs.smoke_evidence),
        residual_risks=tuple(rendered_inputs.residual_risks),
        blocker_summary=rendered_inputs.blocker_summary.strip(),
        notes=notes,
    )


__all__ = (
    "HANDOFF_LEVELS",
    "HandoffArtifact",
    "HandoffInputs",
    "HandoffLevel",
    "build_handoff",
)
