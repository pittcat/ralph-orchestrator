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
* The caller-supplied plan file is *read-only*. ``generate_suite``
  reads its bytes once, hashes them, and stages them into the sandbox
  at ``<sandbox>/docs/plans/<basename>``. The source plan file is
  never modified; the staged copy is a sandbox-scoped artefact that
  the live loop sees when launched from the sandbox cwd.
* The generated argv always carries ``-c ralph.<stem>.yml`` /
  ``-H <preset>`` so :envvar:`RALPH_CONFIG` / ``ralph.yml`` cannot
  preempt the target suite. ``--plan`` uses a sandbox-relative path
  (``docs/plans/<basename>``) so the launch command is portable
  across machines.
* Atomic: every write goes to a sibling ``.tmp`` first and is
  renamed via :func:`os.replace`. On failure, every partially-written
  file (including the staged plan) is removed.
* Pure stdlib. No third-party imports. No subprocess invocations at
  import time.
"""

from __future__ import annotations

import hashlib
import os
import re
import tempfile
from dataclasses import dataclass
from pathlib import Path

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
    """Read the caller-supplied plan bytes once.

    The source plan is read-only; this helper only reads it. The
    bytes are then staged into the sandbox via
    :func:`_stage_plan_into_sandbox` so the live loop can find the
    plan when launched from the sandbox cwd.
    """
    try:
        return plan_path.read_bytes()
    except (FileNotFoundError, IsADirectoryError, PermissionError, OSError) as exc:
        raise SandboxError(f"plan file not readable: {exc.__class__.__name__}") from exc


# Basenames that can be safely staged into the sandbox. We disallow
# path separators and ``..`` so a caller cannot trick the script into
# placing the plan outside ``<sandbox>/docs/plans/``.
_SAFE_BASENAME_RE = re.compile(r"^[A-Za-z0-9._-]+$")


def _stage_plan_into_sandbox(
    sandbox: Path, plan_bytes: bytes, basename: str
) -> Path:
    """Atomically stage the plan bytes into ``<sandbox>/docs/plans/<basename>``.

    Returns the absolute path of the staged file.

    Raises :class:`SandboxError` when:

    * ``basename`` contains a path separator or ``..`` (defence-in-depth
      against caller-supplied names that would escape the sandbox);
    * the staged destination already exists with different bytes
      (content conflict — the operator must reconcile the sandbox
      before re-running, or pass a different basename).
    """
    if not basename or not _SAFE_BASENAME_RE.match(basename):
        raise SandboxError(
            f"plan_stage: unsafe basename {basename!r}; "
            "must match [A-Za-z0-9._-]+"
        )

    plan_dir = sandbox / "docs" / "plans"
    staged = plan_dir / basename

    if staged.exists():
        try:
            existing = staged.read_bytes()
        except OSError as exc:
            raise SandboxError(
                f"plan_stage: cannot read existing {staged}: "
                f"{exc.__class__.__name__}"
            ) from exc
        if existing != plan_bytes:
            raise SandboxError(
                f"plan_stage: destination {staged} has different bytes "
                "than the caller-supplied plan; refusing to overwrite"
            )
        # Idempotent: identical bytes — leave the staged file alone.
        return staged

    plan_dir.mkdir(parents=True, exist_ok=True)

    tmp_dir = plan_dir
    fd, tmp_name = tempfile.mkstemp(
        prefix=f".{basename}.", suffix=".tmp", dir=tmp_dir
    )
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(plan_bytes)
        os.replace(tmp_name, staged)
    except Exception:
        # Clean up the staged file if it was created (e.g. on a
        # follow-up failure after the rename) and the temp file.
        try:
            os.unlink(tmp_name)
        except OSError:
            pass
        try:
            staged.unlink()
        except FileNotFoundError:
            pass
        except OSError:
            pass
        raise

    return staged


def _check_writable(directory: Path) -> None:
    """Refuse to write to orchestration SSOT subtrees (canonical-roots).

    The canonical SSOT root names are ``presets`` / ``crates`` /
    ``.ralph``. Any directory whose resolved path contains a segment
    that exactly equals a canonical root OR starts with a canonical
    root followed by a dash is rejected. This catches both subtrees
    under the SSOT (e.g. ``<repo>/presets/en/``) and sandbox roots
    that share a canonical-root prefix (e.g. ``/tmp/presets-foo/``).
    Directories whose name merely contains a canonical root name as
    an interior substring (e.g. ``real_presets/``) are NOT blocked
    by this guard.
    """
    resolved = directory.resolve()
    parts = resolved.parts
    canonical_roots = ("presets", "crates", ".ralph")
    for part in parts:
        for root in canonical_roots:
            if part == root or (part.startswith(f"{root}-") and len(part) > len(root)):
                raise SandboxError(
                    f"refusing to write inside orchestration SSOT ({part}/): {resolved}"
                )
    if not directory.exists():
        raise SandboxError(f"sandbox directory does not exist: {directory}")
    if not directory.is_dir():
        raise SandboxError(f"sandbox path is not a directory: {directory}")
    if not os.access(directory, os.W_OK):
        raise SandboxError(f"sandbox directory is not writable: {directory}")


def _is_owned_payload(raw: bytes) -> bool:
    lines = raw.splitlines()
    return bool(
        len(lines) >= 3
        and lines[0] == b"# generated_by: ralph-e2e-bootstrap"
        and lines[1].startswith(b"# profile_sha256: ")
        and lines[2].startswith(b"# prompt_sha256: ")
    )


def _atomic_write_with_provenance(
    path: Path,
    payload: bytes,
    profile_sha256: str,
    prompt_sha256: str,
    *,
    refresh_existing: bool = False,
) -> None:
    """Atomic write with provenance header + reuse check.

    The first 3 lines are a reproducible provenance comment::

        # generated_by: ralph-e2e-bootstrap
        # profile_sha256: <hex>
        # prompt_sha256: <hex>

    If ``path`` already exists, parse its first 3 lines; if the
    embedded ``profile_sha256`` / ``prompt_sha256`` differ from the
    current call, raise ``SandboxError("write_conflict: provenance
    mismatch on <path>: existing=<old> current=<new>")``.

    Append the payload (without duplicating the header) so the
    produced file has exactly one provenance block.
    """
    header = (
        f"# generated_by: ralph-e2e-bootstrap\n"
        f"# profile_sha256: {profile_sha256}\n"
        f"# prompt_sha256: {prompt_sha256}\n"
    )
    header_bytes = header.encode("utf-8")

    # Check for write conflict on existing file.
    if path.exists():
        try:
            raw = path.read_bytes()
        except OSError:
            raw = b""
        lines = raw.splitlines()
        if len(lines) >= 3:
            existing = b"\n".join(lines[:3]) + b"\n"
            if existing != header_bytes:
                if refresh_existing and _is_owned_payload(raw):
                    pass
                else:
                    def _extract_sha(line: bytes) -> str:
                        return line.decode("utf-8").split(":", 1)[1].strip()

                    old_profile = _extract_sha(lines[1])
                    old_prompt = _extract_sha(lines[2])
                    raise SandboxError(
                        f"write_conflict: provenance mismatch on {path}: "
                        f"existing=profile:{old_profile} prompt:{old_prompt} "
                        f"current=profile:{profile_sha256} prompt:{prompt_sha256}"
                    )

    def _write() -> None:
        tmp_dir = path.parent
        # A. Orphan cleanup — remove stale .tmp files from prior runs.
        for stale in tmp_dir.glob(f".{path.name}.*.tmp"):
            try:
                stale.unlink()
            except OSError:
                pass
        # B. Stage to .tmp sibling.
        fd, tmp_name = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=tmp_dir)
        try:
            with os.fdopen(fd, "wb") as handle:
                handle.write(header_bytes)
                handle.write(payload)
            os.replace(tmp_name, path)
        except Exception:
            try:
                os.unlink(tmp_name)
            except OSError:
                pass
            raise

    _write()


def _atomic_pair_write(
    config_path: Path,
    prompt_path: Path,
    config_payload: bytes,
    prompt_payload: bytes,
    *,
    profile_sha256: str,
    prompt_sha256: str,
    refresh_existing: bool = False,
    rollback_paths: tuple[Path, ...] = (),
    updated_pair: tuple[Path, Path] | None = None,
) -> None:
    """Write config + prompt payloads atomically across the pair.

    Each half is written via _atomic_write_with_provenance, which
    stages a .tmp sibling, writes a 3-line provenance header + payload,
    then os.replace-s it into place.

    On any failure:
    * Clean up staged ``.tmp`` siblings.
    * For **create** paths (``updated_pair is None``): unlink any
      halves this call created (``rollback_paths``) so a partial
      create does not leave orphan owned files.
    * For **update** paths (``updated_pair`` set): restore both
      originals from the pre-call byte snapshots so a half-failed
      update never destroys the prior owned suite.
    """
    # Snapshot originals for updated_pair restore.
    original_config_bytes: bytes | None = None
    original_prompt_bytes: bytes | None = None
    if updated_pair is not None:
        if config_path.exists():
            try:
                original_config_bytes = config_path.read_bytes()
            except OSError:
                original_config_bytes = None
        if prompt_path.exists():
            try:
                original_prompt_bytes = prompt_path.read_bytes()
            except OSError:
                original_prompt_bytes = None

    errors: dict[Path, Exception] = {}

    # Write each half with provenance header.
    for path, payload in [
        (config_path, config_payload),
        (prompt_path, prompt_payload),
    ]:
        try:
            if refresh_existing:
                _atomic_write_with_provenance(
                    path,
                    payload,
                    profile_sha256,
                    prompt_sha256,
                    refresh_existing=True,
                )
            else:
                _atomic_write_with_provenance(
                    path,
                    payload,
                    profile_sha256,
                    prompt_sha256,
                )
        except Exception as exc:  # noqa: PERF203
            errors[path] = exc

    if not errors:
        return  # both halves written successfully

    # At least one half failed.  Clean up .tmp / orphan files first.
    tmp_dir = config_path.parent
    for stem in [f".{config_path.name}.", f".{prompt_path.name}."]:
        for stale in tmp_dir.glob(f"{stem}*.tmp"):
            try:
                stale.unlink()
            except OSError:
                pass

    if updated_pair is None:
        # Create path: remove any halves this call produced.
        for path_ in list(rollback_paths) + [config_path, prompt_path]:
            try:
                path_.unlink()
            except FileNotFoundError:
                pass
            except OSError:
                pass
        raise errors[config_path if config_path in errors else prompt_path]

    # Update path: restore pre-call snapshots so the owned suite
    # is never left half-written or deleted.
    for path, original_bytes in [
        (config_path, original_config_bytes),
        (prompt_path, original_prompt_bytes),
    ]:
        if original_bytes is None:
            # Did not exist before this call — remove any partial write.
            try:
                path.unlink()
            except FileNotFoundError:
                pass
            except OSError:
                pass
            continue
        try:
            fd, tmp_name = tempfile.mkstemp(
                prefix=f".{path.name}.restore.", suffix=".tmp", dir=path.parent
            )
            try:
                with os.fdopen(fd, "wb") as handle:
                    handle.write(original_bytes)
                os.replace(tmp_name, path)
            except Exception:
                try:
                    os.unlink(tmp_name)
                except OSError:
                    pass
                raise
        except OSError as restore_exc:
            raise SandboxError(
                f"pair-half-written: failed to restore {path} after write error "
                f"({restore_exc}); original error: {errors.get(path) or next(iter(errors.values()))}"
            ) from restore_exc

    # Both originals restored (or never existed). Re-raise the first write error.
    raise errors[config_path if config_path in errors else prompt_path]


# ---------------------------------------------------------------------------
# Suite generation
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# Helpers for generate_suite
# ---------------------------------------------------------------------------


def _render_payloads(plan_relpath: str, stem: str, preset: str) -> tuple[bytes, bytes]:
    """Return the (config_payload, prompt_payload) bytes for the suite pair."""
    config_payload = CONFIG_TEMPLATE.format(
        stem=stem,
        preset=preset.removeprefix("builtin:"),
    ).encode("utf-8")
    prompt_payload = PROMPT_TEMPLATE.format(
        stem=stem,
        preset=preset,
        plan_relpath=plan_relpath,
    ).encode("utf-8")
    return config_payload, prompt_payload


def _compute_disposition(
    path: Path,
    payload: bytes,
    config_payload: bytes,
    prompt_payload: bytes,
    created: list[str],
    updated: list[str],
    noop: list[str],
    *,
    refresh_existing: bool = False,
) -> str:
    """Determine file disposition using provenance comparison.

    Reads the existing file's first 3 lines as a provenance header.
    Raises ``SandboxError("write_conflict: …")`` on provenance mismatch.
    Otherwise falls back to bytes-equality for noop/updated/created.
    """
    if path.exists():
        try:
            raw = path.read_bytes()
        except OSError:
            raw = b""
        lines = raw.splitlines()
        if len(lines) >= 3:

            def _extract_sha(line: bytes) -> str | None:
                # Header lines look like `# profile_sha256: <hex>` — only
                # those carry a `:` separator in the form `<key>: <value>`.
                # Lines without that shape (Markdown comments, bare YAML keys,
                # etc.) return None so we fall through to bytes-equality.
                parts = line.decode("utf-8", errors="replace").split(":", 1)
                if len(parts) != 2:
                    return None
                return parts[1].strip()

            existing_profile = _extract_sha(lines[1])
            existing_prompt = _extract_sha(lines[2])
            if existing_profile is not None and existing_prompt is not None:
                current_profile = _sha256_bytes(config_payload)
                current_prompt = _sha256_bytes(prompt_payload)
                if existing_profile != current_profile or existing_prompt != current_prompt:
                    if not (refresh_existing and _is_owned_payload(raw)):
                        raise SandboxError(
                            f"write_conflict: provenance mismatch on {path}: "
                            f"existing=profile:{existing_profile} prompt:{existing_prompt} "
                            f"current=profile:{current_profile} prompt:{current_prompt}"
                        )
            # else: legacy file without header → fall through to
            # bytes-equality check below.
        if raw == payload:
            noop.append(str(path))
            return "noop"
        updated.append(str(path))
        return "updated"
    created.append(str(path))
    return "created"


def _build_argv(
    binary: str, config_path: Path, preset: str, plan_path: Path
) -> tuple[tuple[str, ...], tuple[str, ...]]:
    """Return (base_argv, launch_argv) for the suite."""
    base = (
        binary,
        "-c",
        str(config_path),
        "-H",
        preset,
        "run",
        "--dry-run",
        "--plan",
        str(plan_path),
    )
    launch = (
        binary,
        "-c",
        str(config_path),
        "-H",
        preset,
        "run",
        "--plan",
        str(plan_path),
    )
    return base, launch


# ---------------------------------------------------------------------------
# Suite generation
# ---------------------------------------------------------------------------


def generate_suite(
    *,
    sandbox: Path,
    preset: str,
    plan_path: Path,
    stem: str | None = None,
    binary: str = "ralph",
    refresh_existing: bool = False,
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
    binary:
        Absolute path (or PATH name) of the ``ralph`` binary that
        gate / handoff must use. Defaults to ``"ralph"`` only for
        callers that resolve PATH later; production skill wiring
        MUST pass the value from :func:`binary_resolve.resolve_binary`.
    refresh_existing:
        Refresh an existing pair only when both files carry this skill's
        provenance header. Unowned files and ordinary calls still fail
        closed with ``write_conflict``.

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

    # Stage the plan into the sandbox so the live loop (launched from
    # the sandbox cwd) can find it. argv then uses the staged
    # sandbox-relative path. The source plan bytes remain untouched.
    staged_plan_pre_existed = (sandbox / "docs" / "plans" / plan_path.name).exists()
    staged_plan = _stage_plan_into_sandbox(sandbox, plan_bytes, plan_path.name)
    staged_plan_relpath = staged_plan.relative_to(sandbox).as_posix()

    config_payload, prompt_payload = _render_payloads(
        staged_plan_relpath, resolved_stem, preset
    )

    created: list[str] = []
    updated: list[str] = []
    noop: list[str] = []

    _compute_disposition(
        config_path,
        config_payload,
        config_payload,
        prompt_payload,
        created,
        updated,
        noop,
        refresh_existing=refresh_existing,
    )
    _compute_disposition(
        prompt_path,
        prompt_payload,
        config_payload,
        prompt_payload,
        created,
        updated,
        noop,
        refresh_existing=refresh_existing,
    )

    config_sha256 = _sha256_bytes(config_payload)
    prompt_sha256 = _sha256_bytes(prompt_payload)

    # Only set updated_pair when BOTH halves are being updated (not
    # when one is a noop and the other is updated).  Pair-half-written
    # verification is only meaningful when both files had pending writes;
    # when one is a noop the result is always consistent regardless.
    updated_config = str(config_path) in updated
    updated_prompt = str(prompt_path) in updated

    # Track whether the staged plan was newly created by this call so
    # we can roll it back on pair-write failure.
    try:
        _atomic_pair_write(
            config_path,
            prompt_path,
            config_payload,
            prompt_payload,
            profile_sha256=config_sha256,
            prompt_sha256=prompt_sha256,
            refresh_existing=refresh_existing,
            rollback_paths=tuple(Path(p) for p in created),
            updated_pair=(config_path, prompt_path)
            if (updated_config and updated_prompt)
            else None,
        )
    except Exception as exc:
        # If we created the staged plan in this call (the staged file
        # did not exist before _stage_plan_into_sandbox ran), remove it
        # so the sandbox is not left half-populated.
        if not staged_plan_pre_existed:
            try:
                staged_plan.unlink()
            except FileNotFoundError:
                pass
            except OSError:
                pass
        raise SandboxError(f"atomic write failed: {exc.__class__.__name__}: {exc}") from exc

    binary_token = binary.strip() or "ralph"
    argv, launch_argv = _build_argv(
        binary_token, config_path, preset, Path(staged_plan_relpath)
    )

    return SuiteResult(
        config_path=str(config_path),
        prompt_path=str(prompt_path),
        argv=argv,
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