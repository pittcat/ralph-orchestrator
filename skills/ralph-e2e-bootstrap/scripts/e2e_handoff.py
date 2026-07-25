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
from pathlib import Path
from typing import Iterable, Literal, Sequence, cast

HandoffLevel = Literal["static_only", "blocked"]

HANDOFF_LEVELS: tuple[HandoffLevel, ...] = ("static_only", "blocked")


def _ensure_level(level: str) -> HandoffLevel:
    if level not in HANDOFF_LEVELS:
        raise ValueError(
            f"unknown handoff level: {level!r}; must be one of {HANDOFF_LEVELS}"
        )
    return cast(HandoffLevel, level)


@dataclass(frozen=True)
class HandoffInputs:
    """Inputs the E2E bootstrap pipeline hands to ``build_handoff``.

    All path fields are sandbox-relative or repo-relative. ``binary``
    is the path to the ``ralph`` binary the operator will run
    (typically a repo ``target/debug/ralph`` path).
    """

    binary: str
    config_path: str
    preset: str
    plan_path: str
    level: HandoffLevel
    sandbox_path: str
    prompt_path: str = ""
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
        if Path(self.sandbox_path).is_absolute():
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
        "0123456789_./:+=@"
    )
    if all(ch in safe_chars for ch in token):
        return token
    return "'" + token.replace("'", "'\"'\"'") + "'"


def _render_command(argv: Sequence[str]) -> str:
    return " ".join(_shell_quote(token) for token in argv)


def _render_validation_block(evidence: Iterable[str]) -> tuple[str, ...]:
    return tuple(line for line in evidence if line.strip())


def _render_header(inputs: HandoffInputs) -> list[str]:
    """Render the report header lines."""
    lines = [
        "# Ralph E2E Bootstrap — Handoff",
        "",
        f"- **Level**: `{inputs.level}`",
        f"- **Sandbox**: `{inputs.sandbox_path}`",
        f"- **Config**: `{inputs.config_path}`",
        f"- **Preset**: `{inputs.preset}`",
        f"- **Plan** (``--plan`` workload): `{inputs.plan_path}`",
    ]
    if inputs.prompt_path.strip():
        lines.append(
            f"- **Prompt** (``--prompt-file``, agent-visible): `{inputs.prompt_path}`"
        )
    lines.extend(
        [
            f"- **Binary**: `{inputs.binary}`",
            "",
        ]
    )
    return lines


def _render_blocker_section(blocker_summary: str) -> list[str]:
    """Render the blocker section, sanitising markdown link injection.

    Escapes ``]`` characters inside ``blocker_summary`` to prevent
    ``](url)``-style link injection when the summary contains raw
    markdown link syntax.
    """
    sanitised = blocker_summary.replace("]", r"\]")
    return [
        "## Blocker",
        "",
        sanitised,
        "",
    ]


def _render_status_section(level: HandoffLevel) -> list[str]:
    """Render the static-only vs blocked status block."""
    sections = ["## Status"]
    if level == "static_only":
        sections.append(
            "Static load passed; **the loop is NOT closed**. The launch command "
            "above is the canonical operator action; running it will start a "
            "live Ralph loop in the supplied sandbox. After the live run, use "
            "``ralph-run-diagnosis`` for intermediate artifacts — not this skill."
        )
    else:
        sections.append(
            "Static load did NOT pass; the launch command is empty. The "
            "blocker above names the missing prerequisite."
        )
    sections.append("")
    return sections


def _render_report(
    *,
    inputs: HandoffInputs,
    command: str,
    notes: Sequence[str],
) -> str:
    sections: list[str] = []
    sections.extend(_render_header(inputs))

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
        sections.extend(_render_blocker_section(inputs.blocker_summary))

    if notes:
        sections.append("## Notes")
        for note in notes:
            sections.append(f"- {note}")
        sections.append("")

    sections.extend(_render_status_section(inputs.level))
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
        prompt_rel = inputs.prompt_path.strip()
        if not prompt_rel:
            # Derive from config stem when callers omit prompt_path.
            cfg = Path(inputs.config_path).name
            if cfg.startswith("ralph.") and cfg.endswith(".yml"):
                prompt_rel = f"PROMPT.{cfg[len('ralph.'):-len('.yml')]}.md"
            else:
                prompt_rel = "PROMPT.md"
        command_argv = (
            inputs.binary,
            "-c",
            inputs.config_path,
            "-H",
            inputs.preset,
            "run",
            "--prompt-file",
            prompt_rel,
            "--plan",
            inputs.plan_path,
        )
        command = _render_command(command_argv)
        notes = (
            "Static load passed; loop is NOT closed.",
            "``--prompt-file`` is agent-visible (change intent + workload body).",
            "``--plan`` is the pure sandbox workload identity.",
            "Use the launch command above verbatim from the sandbox cwd.",
            "Post-run diagnosis: ``ralph-run-diagnosis``.",
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