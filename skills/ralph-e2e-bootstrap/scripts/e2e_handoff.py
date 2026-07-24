"""Handoff builder for ``ralph-e2e-bootstrap``.

Renders the final handoff an operator consumes when the E2E bootstrap
pipeline finishes. Distinct from ``ralph-project-bootstrap``'s
``handoff.py``: this one is **always** static-only by default
(R10) and never claims ``complete`` even when a smoke runs (the
skill itself does not run smokes; that belongs to a separate flow).

Public surface (everything else is private):

* :class:`HandoffInputs` — typed inputs the bootstrap pipeline hands
  to :func:`build_handoff`.
* :class:`HandoffArtifact` — rendered artifact: level + command +
  markdown report.
* :func:`build_handoff` — main entry point. Pure stdlib.

Hard rules:

* Pure stdlib. No third-party imports.
* All path fields are sandbox-relative or repo-relative; absolute
  argv values are kept inside the rendered command only.
* The level is **always** one of ``static_only`` / ``blocked``.
  ``complete`` is intentionally absent: an E2E sandbox bootstrap
  cannot prove a loop closed, only that the static gates pass.
* ``blocked`` requires a non-empty ``blocker_summary``.
* The rendered report explicitly distinguishes "static load passed"
  from "loop closed"; ``references/interaction.md`` mandates the
  distinction.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import PurePosixPath
from typing import Iterable, Literal, Sequence

HandoffLevel = Literal["static_only", "blocked"]

HANDOFF_LEVELS: tuple[HandoffLevel, ...] = ("static_only", "blocked")


def _ensure_level(level: str) -> HandoffLevel:
    if level not in HANDOFF_LEVELS:
        raise ValueError(
            f"unknown handoff level: {level!r}; must be one of {HANDOFF_LEVELS}"
        )
    return level  # type: ignore[return-value]


@dataclass(frozen=True)
class HandoffInputs:
    """Inputs the E2E bootstrap pipeline hands to ``build_handoff``.

    All path fields are sandbox-relative or repo-relative. ``binary``
    is the path to the ``ralph`` binary the operator will run
    (typically the literal ``"ralph"`` string).
    """

    binary: str
    config_path: str
    preset: str
    plan_path: str
    level: HandoffLevel
    sandbox_path: str
    validation_evidence: tuple[str, ...] = ()
    residual_risks: tuple[str, ...] = ()
    blocker_summary: str = ""
    stage_outcomes: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        _ensure_level(self.level)
        for label, value in (
            ("binary", self.binary),
            ("config_path", self.config_path),
            ("preset", self.preset),
            ("plan_path", self.plan_path),
            ("sandbox_path", self.sandbox_path),
        ):
            if not isinstance(value, str) or not value.strip():
                raise ValueError(f"{label} must be a non-empty string")
        if self.level == "blocked" and not self.blocker_summary.strip():
            raise ValueError("blocker_summary must be non-empty when level='blocked'")
        # ``config_path`` and ``plan_path`` may be absolute — they
        # are argv tokens passed to ``ralph run`` and the dry-run
        # argv in particular carries ``--plan <abs>``. ``sandbox_path``
        # is the only field that must be repo-relative: it is the
        # path persisted to disk for the operator's handoff record.
        if PurePosixPath(self.sandbox_path).is_absolute():
            raise ValueError(
                f"sandbox_path must be sandbox- or repo-relative: {self.sandbox_path!r}"
            )


@dataclass(frozen=True)
class HandoffArtifact:
    """Rendered E2E bootstrap handoff."""

    level: HandoffLevel
    command: str
    command_argv: tuple[str, ...]
    report: str
    sandbox_path: str
    validation_summary: tuple[str, ...]
    residual_risks: tuple[str, ...]
    blocker_summary: str
    stage_outcomes: tuple[str, ...]
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


def _render_command(argv: Sequence[str]) -> str:
    return " ".join(_shell_quote(token) for token in argv)


def _render_validation_block(evidence: Iterable[str]) -> tuple[str, ...]:
    return tuple(line for line in evidence if line.strip())


def _render_report(
    *,
    inputs: HandoffInputs,
    command: str,
    notes: Sequence[str],
) -> str:
    sections: list[str] = []
    sections.append("# Ralph E2E Bootstrap — Handoff")
    sections.append("")
    sections.append(f"- **Level**: `{inputs.level}`")
    sections.append(f"- **Sandbox**: `{inputs.sandbox_path}`")
    sections.append(f"- **Config**: `{inputs.config_path}`")
    sections.append(f"- **Preset**: `{inputs.preset}`")
    sections.append(f"- **Plan**: `{inputs.plan_path}`")
    sections.append(f"- **Binary**: `{inputs.binary}`")
    sections.append("")

    sections.append("## Launch command")
    sections.append("")
    sections.append("```bash")
    sections.append(command)
    sections.append("```")
    sections.append("")

    sections.append("## Static gate evidence")
    if inputs.validation_evidence:
        for line in inputs.validation_evidence:
            sections.append(f"- {line}")
    else:
        sections.append("- (no static gate evidence recorded)")
    sections.append("")

    if inputs.stage_outcomes:
        sections.append("## Stage outcomes")
        for outcome in inputs.stage_outcomes:
            sections.append(f"- {outcome}")
        sections.append("")

    if inputs.residual_risks:
        sections.append("## Residual risks")
        for risk in inputs.residual_risks:
            sections.append(f"- {risk}")
        sections.append("")

    if inputs.level == "blocked":
        sections.append("## Blocker")
        sections.append("")
        sections.append(inputs.blocker_summary)
        sections.append("")

    if notes:
        sections.append("## Notes")
        for note in notes:
            sections.append(f"- {note}")
        sections.append("")

    # Always distinguish static-only from loop-closed (R10).
    sections.append("## Status")
    if inputs.level == "static_only":
        sections.append(
            "Static load passed; **the loop is NOT closed**. The launch command "
            "above is the canonical operator action; running it will start a "
            "live Ralph loop in the supplied sandbox."
        )
    else:
        sections.append(
            "Static load did NOT pass; the launch command is empty. The "
            "blocker above names the missing prerequisite."
        )
    sections.append("")
    return "\n".join(sections)


# ---------------------------------------------------------------------------
# Public entry point
# ---------------------------------------------------------------------------


def build_handoff(inputs: HandoffInputs) -> HandoffArtifact:
    """Render the E2E bootstrap handoff artifact."""
    if inputs.level == "blocked":
        command = ""
        command_argv: tuple[str, ...] = ()
        notes = ("Handoff blocked; resolve the blocker above and re-run.",)
    else:
        command_argv = (
            inputs.binary,
            "-c",
            inputs.config_path,
            "-H",
            inputs.preset,
            "run",
            "--plan",
            inputs.plan_path,
        )
        command = _render_command(command_argv)
        notes = (
            "Static load passed; loop is NOT closed.",
            "Use the launch command above verbatim in the supplied sandbox.",
        )

    return HandoffArtifact(
        level=inputs.level,
        command=command,
        command_argv=command_argv,
        report=_render_report(inputs=inputs, command=command, notes=notes),
        sandbox_path=inputs.sandbox_path,
        validation_summary=_render_validation_block(inputs.validation_evidence),
        residual_risks=tuple(inputs.residual_risks),
        blocker_summary=inputs.blocker_summary,
        stage_outcomes=tuple(inputs.stage_outcomes),
        notes=notes,
    )


__all__ = [
    "HANDOFF_LEVELS",
    "HandoffArtifact",
    "HandoffInputs",
    "HandoffLevel",
    "build_handoff",
]