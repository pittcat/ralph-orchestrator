"""Sandbox suite generation for ``ralph-e2e-bootstrap``.

The skill runs this script after the binary is resolved. It owns the
E2E sandbox directory the operator designated, generating the
preset-bound pair (``ralph.<stem>.yml`` + ``PROMPT.<stem>.md``) the
downstream static gate consumes.

Public surface (everything else is private):

* :class:`SandboxError` — raised for user-visible generation failures.
* :func:`derive_stem` — resolve the preset stem from the resolved
  preset reference.
* :func:`generate_suite` — main entry point; pure stdlib, atomic.

Hard rules:

* The sandbox directory is the **only** directory the script writes
  to. It refuses to write inside ``presets/`` (the orchestration SSOT
  is read-only for this skill).
* The supplied plan file is *read-only*. The script captures its
  SHA-256 once and references it via ``--plan <abs>`` in the
  rendered argv; it MUST NOT rewrite the plan file.
* The generated argv always carries ``-c ralph.<stem>.yml`` /
  ``-H <preset>`` so :envvar:`RALPH_CONFIG` / ``ralph.yml`` cannot
  preempt the target suite.
* Atomic: every write goes to a sibling ``.tmp`` first and is
  renamed via :func:`os.replace`. On failure, every partially-written
  file is removed.
* Pure stdlib. No third-party imports. No subprocess invocations at
  import time.
"""

from __future__ import annotations

import hashlib
import os
import re
import shutil
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Mapping

# ---------------------------------------------------------------------------
# Stem derivation
# ---------------------------------------------------------------------------

# Recognised preset id shapes: ``builtin:<name>`` and ``file:`` /
# relative-or-absolute paths. We accept arbitrary ``[A-Za-z0-9_.-]+``
# segments so user-built presets pass without modification.
_BUILTIN_RE = re.compile(r"^builtin:(?P<name>[A-Za-z0-9_.-]+)$")
_FILE_RE = re.compile(r"^(?P<path>.+\.ya?ml)$")


def derive_stem(preset: str) -> str:
    """Return the preset stem the sandbox filenames are derived from.

    Examples:

    * ``builtin:ce-executor-pipeline`` → ``ce-executor-pipeline``
    * ``presets/en/ce-executor-pipeline.yml`` → ``ce-executor-pipeline``
    * ``/abs/path/to/my-team.yml`` → ``my-team``

    The stem is the literal segment we pass to the
    ``ralph.<stem>.yml`` / ``PROMPT.<stem>.md`` filename contract.
    """
    builtin = _BUILTIN_RE.match(preset)
    if builtin is not None:
        return builtin.group("name")
    file_match = _FILE_RE.match(preset)
    if file_match is not None:
        path = Path(file_match.group("path"))
        return path.stem
    # Fallback: treat the whole token as the stem. Operators can
    # override via the explicit ``stem_override`` argument downstream.
    return preset


# ---------------------------------------------------------------------------
# Errors
# ---------------------------------------------------------------------------


class SandboxError(RuntimeError):
    """Raised for user-visible generation failures."""


# ---------------------------------------------------------------------------
# Result
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class SuiteResult:
    """Outcome of :func:`generate_suite`.

    * ``created`` / ``updated`` / ``noop`` are the file disposition
      tuples (repo-relative). They are disjoint.
    * ``config_path`` / ``prompt_path`` are the absolute file paths
      the static gate will reference.
    * ``argv`` is the canonical argv the static gate passes to
      ``ralph run --dry-run``. It includes ``-c`` / ``-H`` /
      ``--plan`` and is rendered with ``--dry-run`` removed so the
      operator's launch command stays identical to the static-gate
      argv except for the dry-run switch.
    * ``launch_argv`` is the operator-facing argv (same shape but
      without ``--dry-run``).
    """

    config_path: str
    prompt_path: str
    argv: tuple[str, ...]
    launch_argv: tuple[str, ...]
    created: tuple[str, ...] = ()
    updated: tuple[str, ...] = ()
    noop: tuple[str, ...] = ()
    config_sha256: str = ""
    prompt_sha256: str = ""
    plan_sha256: str = ""

    def repo_relative(self, sandbox: Path, repo_root: Path) -> "SuiteResult":
        """Return a copy with repo-relative path strings."""
        rel = lambda p: (  # noqa: E731
            str(Path(os.path.relpath(p, repo_root))) if p.is_absolute() else str(p)
        )
        return SuiteResult(
            config_path=rel(Path(self.config_path)),
            prompt_path=rel(Path(self.prompt_path)),
            argv=self.argv,
            launch_argv=self.launch_argv,
            created=tuple(rel(Path(p)) for p in self.created),
            updated=tuple(rel(Path(p)) for p in self.updated),
            noop=tuple(rel(Path(p)) for p in self.noop),
            config_sha256=self.config_sha256,
            prompt_sha256=self.prompt_sha256,
            plan_sha256=self.plan_sha256,
        )


# ---------------------------------------------------------------------------
# Templates
# ---------------------------------------------------------------------------

# Minimal preset-bound config body. The static gate validates this
# against the resolved preset via ``ralph preset check --strict``;
# the skill trusts the gate's verdict and never re-parses the
# config here. ``core.project_root`` is set to ``./`` so the suite
# resolves paths from the sandbox root regardless of cwd.
CONFIG_TEMPLATE = """# Ralph preset-bound suite — generated by ralph-e2e-bootstrap.
# Do not hand-edit; refresh via the skill.
core:
  project_root: ./
event_loop:
  supervisor:
    enabled: false
  prompt_file: PROMPT.{stem}.md
hats_source: builtin:{preset}
"""

# Minimal prompt body. The plan-driven default delegates to the plan
# file referenced via ``--plan``; the inline prompt is the static
# fallback the skill hands to ``ralph preset check`` so the strict
# lint can read a non-empty ``event_loop.prompt``.
PROMPT_TEMPLATE = """# Ralph E2E bootstrap — preset-bound prompt

This preset-bound suite is generated by ``ralph-e2e-bootstrap``.
The authoritative prompt source at runtime is the plan file referenced
via ``--plan`` on the launch command. Do not write business content
here; the skill will regenerate this file on every refresh.

Plan path: {plan_relpath}
Preset:    {preset}
Stem:      {stem}
"""


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def _read_plan(plan_path: Path) -> bytes:
    try:
        return plan_path.read_bytes()
    except (FileNotFoundError, IsADirectoryError, PermissionError, OSError) as exc:
        raise SandboxError(f"plan file not readable: {exc.__class__.__name__}") from exc


def _check_writable(directory: Path) -> None:
    """Refuse to write to ``presets/`` subtrees (orchestration SSOT)."""
    resolved = directory.resolve()
    parts = resolved.parts
    if "presets" in parts:
        raise SandboxError(f"refusing to write inside orchestration SSOT: {resolved}")
    if not directory.exists():
        raise SandboxError(f"sandbox directory does not exist: {directory}")
    if not directory.is_dir():
        raise SandboxError(f"sandbox path is not a directory: {directory}")
    if not os.access(directory, os.W_OK):
        raise SandboxError(f"sandbox directory is not writable: {directory}")


def _atomic_write(path: Path, payload: bytes) -> None:
    """Write ``payload`` to ``path`` atomically.

    The sibling ``.tmp`` is created in the same directory so the
    rename is on the same filesystem. ``os.replace`` is atomic on
    POSIX and on Windows (when the destination is on the same
    filesystem as the temp file).
    """
    tmp_dir = path.parent
    fd, tmp_name = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=tmp_dir)
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(payload)
        os.replace(tmp_name, path)
    except Exception:
        # Best-effort cleanup of the temp file on the failure path.
        try:
            os.unlink(tmp_name)
        except OSError:
            pass
        raise


# ---------------------------------------------------------------------------
# Suite generation
# ---------------------------------------------------------------------------


def generate_suite(
    *,
    sandbox: Path,
    preset: str,
    plan_path: Path,
    stem: str | None = None,
    extra_hats: Mapping[str, Any] | None = None,
) -> SuiteResult:
    """Generate the preset-bound pair in ``sandbox``.

    Parameters
    ----------
    sandbox:
        Caller-supplied E2E sandbox directory. Must exist, be a
        directory, be writable, and not live under ``presets/``.
    preset:
        Resolved preset reference. May be a ``builtin:`` id or a
        relative / absolute file path.
    plan_path:
        Path to the development plan file. Read-only — its bytes
        are hashed once and never modified.
    stem:
        Override for the derived preset stem. When ``None`` the
        function derives it from ``preset`` via :func:`derive_stem`.
    extra_hats:
        Optional mapping injected into the generated config under
        ``event_loop.hats_source`` for tests that need to control
        the resolved preset. Production callers leave this ``None``.

    Raises
    ------
    SandboxError
        When the sandbox is unwritable, lives under ``presets/``,
        or the plan file is unreadable. The exception message is
        operator-facing.
    """
    sandbox = Path(sandbox).resolve()
    _check_writable(sandbox)

    resolved_stem = stem or derive_stem(preset)
    config_path = sandbox / f"ralph.{resolved_stem}.yml"
    prompt_path = sandbox / f"PROMPT.{resolved_stem}.md"

    plan_bytes = _read_plan(plan_path)
    plan_sha256 = _sha256_bytes(plan_bytes)

    # Plan file is referenced by absolute path in the argv; only its
    # repo-relative form is embedded in the prompt body for operator
    # legibility. We never mutate the plan file itself.
    plan_relpath = str(Path(os.path.relpath(plan_path, sandbox)))

    config_payload = CONFIG_TEMPLATE.format(
        stem=resolved_stem,
        preset=preset.removeprefix("builtin:"),
    ).encode("utf-8")
    prompt_payload = PROMPT_TEMPLATE.format(
        stem=resolved_stem,
        preset=preset,
        plan_relpath=plan_relpath,
    ).encode("utf-8")

    created: list[str] = []
    updated: list[str] = []
    noop: list[str] = []

    def _dispose(path: Path, payload: bytes) -> str:
        if path.exists():
            try:
                existing = path.read_bytes()
            except OSError:
                existing = b""
            if existing == payload:
                noop.append(str(path))
                return "noop"
            updated.append(str(path))
            return "updated"
        created.append(str(path))
        return "created"

    _dispose(config_path, config_payload)
    _dispose(prompt_path, prompt_payload)

    try:
        if not config_path.exists() or config_path.read_bytes() != config_payload:
            _atomic_write(config_path, config_payload)
        if not prompt_path.exists() or prompt_path.read_bytes() != prompt_payload:
            _atomic_write(prompt_path, prompt_payload)
    except Exception as exc:  # pragma: no cover - re-raised as SandboxError
        # Best-effort rollback: any file we created in this call is
        # removed; any file we updated is left untouched (the caller
        # owns the prior state).
        for path in created:
            try:
                os.unlink(Path(path))
            except OSError:
                pass
        raise SandboxError(f"atomic write failed: {exc.__class__.__name__}: {exc}") from exc

    config_sha256 = _sha256_bytes(config_payload)
    prompt_sha256 = _sha256_bytes(prompt_payload)

    binary_token = "ralph"
    base_argv = (
        binary_token,
        "-c",
        str(config_path),
        "-H",
        preset,
        "run",
        "--dry-run",
        "--plan",
        str(plan_path),
    )
    launch_argv = (
        binary_token,
        "-c",
        str(config_path),
        "-H",
        preset,
        "run",
        "--plan",
        str(plan_path),
    )

    return SuiteResult(
        config_path=str(config_path),
        prompt_path=str(prompt_path),
        argv=base_argv,
        launch_argv=launch_argv,
        created=tuple(created),
        updated=tuple(updated),
        noop=tuple(noop),
        config_sha256=config_sha256,
        prompt_sha256=prompt_sha256,
        plan_sha256=plan_sha256,
    )


__all__ = [
    "SandboxError",
    "SuiteResult",
    "derive_stem",
    "generate_suite",
]