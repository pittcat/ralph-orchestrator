"""Pure-Python audit helpers for ``ralph-project-bootstrap``.

The bootstrap pipeline runs *before* any persistent write or backend call.
This module owns:

* ``audit_project_root`` — confirm a single target root, fail closed on
  multiple candidates.
* ``audit_inputs`` — verify the preset / plan / task inputs are present
  and readable; never assume them.
* ``collect_project_facts`` — gather verifiable build / test / lint /
  format entry points from the project tree without inventing commands.
* ``AuditDecision`` — typed result that callers translate into handoff
  copy or hard stops.

All functions are deterministic and project-relative; no absolute paths
or shell calls leave this module.
"""
from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterable

import _paths  # type: ignore[import-not-found]  # loaded via skills/ralph-project-bootstrap/scripts on sys.path

DEFAULT_AGENTS_NAMES = ("AGENTS.md", "CLAUDE.md")


@dataclass(frozen=True)
class AuditIssue:
    """A single reason the audit cannot proceed."""

    code: str
    message: str
    paths: tuple[str, ...] = ()


@dataclass(frozen=True)
class ProjectFacts:
    """Verifiable build / test / lint / format commands."""

    build: tuple[str, ...] = ()
    test: tuple[str, ...] = ()
    lint: tuple[str, ...] = ()
    format: tuple[str, ...] = ()
    ci: tuple[str, ...] = ()
    technology: str = "unknown"

    def is_empty(self) -> bool:
        return not (self.build or self.test or self.lint or self.format or self.ci)


@dataclass(frozen=True)
class AuditDecision:
    """Outcome of the bootstrap audit.

    ``issues`` is empty for a successful audit. ``blocking`` is True when
    the caller MUST halt before any persistent write or backend call.
    """

    root: str | None
    inputs_ok: bool
    facts: ProjectFacts
    issues: tuple[AuditIssue, ...] = ()
    blocking: bool = False
    notes: tuple[str, ...] = ()

    @property
    def is_blocking(self) -> bool:
        return self.blocking or bool(self.issues)


def _is_inside(parent: Path, child: Path) -> bool:
    parent = parent.resolve()
    child = child.resolve()
    try:
        child.relative_to(parent)
    except ValueError:
        return False
    return True


def _find_agent_scopes(start: Path) -> list[Path]:
    """Return ascending AGENTS.md / CLAUDE.md scopes from ``start``."""
    seen: list[Path] = []
    for directory in [start, *start.parents]:
        for name in DEFAULT_AGENTS_NAMES:
            candidate = directory / name
            if candidate.is_file():
                seen.append(candidate.resolve())
    # Preserve ordering, drop duplicates.
    deduped: list[Path] = []
    for path in seen:
        if path not in deduped:
            deduped.append(path)
    return deduped


def _detect_vcs_root(start: Path) -> Path | None:
    current = start.resolve()
    for candidate in [current, *current.parents]:
        if (candidate / ".git").exists():
            return candidate.resolve()
    return None


def audit_project_root(cwd: Path) -> tuple[str | None, tuple[AuditIssue, ...]]:
    """Resolve a single target root from ``cwd``.

    Returns the repo-relative root string (or ``None`` if blocked) plus any
    audit issues that explain why we could not pick one.

    The returned string is anchored on ``cwd`` (the caller's working
    directory). When the resolved root equals ``cwd`` the value is
    ``"./"``; otherwise the relative path is computed from ``cwd``.
    """
    cwd = cwd.resolve()
    issues: list[AuditIssue] = []
    vcs_root = _detect_vcs_root(cwd)
    agent_scopes = _find_agent_scopes(cwd)
    if vcs_root is not None and agent_scopes:
        deepest_scope = agent_scopes[0].parent
        if deepest_scope != vcs_root:
            issues.append(
                AuditIssue(
                    code="root_ambiguous",
                    message=(
                        "vcs root and nearest AGENTS.md/CLAUDE.md disagree; "
                        "stop before any persistent write"
                    ),
                    paths=(
                        _paths.rel(vcs_root, cwd),
                        _paths.rel(deepest_scope, cwd),
                    ),
                )
            )
            return None, tuple(issues)
    if vcs_root is None and len({scope.parent for scope in agent_scopes}) > 1:
        paths = sorted({_paths.rel(scope.parent, cwd) for scope in agent_scopes})
        issues.append(
            AuditIssue(
                code="root_ambiguous",
                message="multiple AGENTS.md/CLAUDE.md scopes without a vcs root",
                paths=tuple(paths),
            )
        )
        return None, tuple(issues)
    root = vcs_root or (agent_scopes[0].parent if agent_scopes else cwd)
    return _paths.rel(root, cwd), ()


def audit_inputs(
    preset: str | None,
    plan_path: str | None,
    root: Path,
    prompt_file: str | None = None,
) -> tuple[bool, tuple[AuditIssue, ...]]:
    """Confirm the preset and any operator-supplied input files are usable.

    A plan is deliberately optional.  A preset may carry its own prompt or
    obtain dynamic context at launch time; deciding whether that contract is
    sufficient belongs to the skill agent after it has read the preset.  This
    helper only validates concrete paths the agent elected to use.
    """
    issues: list[AuditIssue] = []
    if not preset:
        issues.append(
            AuditIssue(
                code="input_missing_preset",
                message="preset path or builtin id is required",
            )
        )
    elif preset.startswith("builtin:"):
        # Builtin ids are validated downstream by `ralph preset check`; we
        # only require a syntactically plausible identifier here.
        if not re.match(r"^builtin:[A-Za-z0-9_.-]+$", preset):
            issues.append(
                AuditIssue(
                    code="input_invalid_builtin_id",
                    message=f"unrecognised builtin id '{preset}'",
                )
            )
    elif not (root / preset).is_file():
        issues.append(
            AuditIssue(
                code="input_missing_preset_file",
                message=f"preset file not readable: {preset}",
                paths=(preset,),
            )
        )
    if plan_path and not (root / plan_path).is_file():
        issues.append(
            AuditIssue(
                code="input_missing_plan_file",
                message=f"plan or task file not readable: {plan_path}",
                paths=(plan_path,),
            )
        )
    if prompt_file and not (root / prompt_file).is_file():
        issues.append(
            AuditIssue(
                code="input_missing_prompt_file",
                message=f"prompt file not readable: {prompt_file}",
                paths=(prompt_file,),
            )
        )
    return (not issues), tuple(issues)


def _read_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return ""


def _grep_patterns(text: str, patterns: Iterable[str]) -> tuple[str, ...]:
    matches: list[str] = []
    for pattern in patterns:
        match = re.search(pattern, text)
        if match is None:
            continue
        matches.append(match.group(0).strip())
    return tuple(matches)


def collect_project_facts(root: Path) -> ProjectFacts:
    """Inspect ``root`` and return verified commands/technology.

    The audit never invents commands: every entry below is gated on a
    real file marker (Cargo.toml, package.json, pyproject.toml, CI yml).
    Unknown stacks return ``ProjectFacts(technology='unknown')``.
    """
    root = root.resolve()
    tech = "unknown"
    build: tuple[str, ...] = ()
    test: tuple[str, ...] = ()
    lint: tuple[str, ...] = ()
    fmt: tuple[str, ...] = ()

    if (root / "Cargo.toml").is_file():
        tech = "rust"
        build = ("cargo build",)
        test = ("cargo nextest run",)
        lint = ("cargo clippy --workspace --all-targets -- -D warnings",)
        fmt = ("cargo fmt --all -- --check",)
    elif (root / "package.json").is_file():
        tech = "node"
        pkg = _read_text(root / "package.json")
        scripts = re.findall(r'"(?P<key>[^"]+)"\s*:\s*"[^"]*"', pkg)
        if "build" in scripts:
            build = ("npm run build",)
        if "test" in scripts:
            test = ("npm test",)
        if "lint" in scripts:
            lint = ("npm run lint",)
        if "format" in scripts or "fmt" in scripts:
            fmt = ("npm run format",)
    elif (root / "pyproject.toml").is_file():
        tech = "python"
        build = ()
        test = (".venv/bin/python -m pytest",)
        lint = ()
        fmt = ()

    ci_entries: tuple[str, ...] = ()
    workflow_dir = root / ".github" / "workflows"
    if workflow_dir.is_dir():
        ci_entries = tuple(
            sorted(
                _paths.rel(path)
                for path in workflow_dir.glob("*.y*ml")
            )
        )

    return ProjectFacts(
        build=build,
        test=test,
        lint=lint,
        format=fmt,
        ci=ci_entries,
        technology=tech,
    )


def run_audit(
    cwd: Path,
    *,
    preset: str | None,
    plan_path: str | None = None,
    prompt_file: str | None = None,
) -> AuditDecision:
    """Top-level entry: resolve root, validate inputs, collect facts."""
    cwd = cwd.resolve()
    root_rel, root_issues = audit_project_root(cwd)
    if root_issues:
        return AuditDecision(
            root=None,
            inputs_ok=False,
            facts=ProjectFacts(),
            issues=root_issues,
            blocking=True,
        )
    inputs_ok, input_issues = audit_inputs(preset, plan_path, cwd, prompt_file)
    facts = collect_project_facts(cwd) if root_rel else ProjectFacts()
    notes: tuple[str, ...] = ()
    if facts.is_empty():
        notes = ("no verifiable build/test/lint entry points discovered",)
    blocking = not inputs_ok or not root_rel
    return AuditDecision(
        root=root_rel,
        inputs_ok=inputs_ok,
        facts=facts,
        issues=input_issues,
        blocking=blocking,
        notes=notes,
    )
