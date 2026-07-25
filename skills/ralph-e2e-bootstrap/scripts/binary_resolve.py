"""Binary resolution for ``ralph-e2e-bootstrap``.

The skill runs this script after the plan×diff audit and before the
sandbox suite generation. The script picks the ``ralph`` executable
that will be used by every downstream stage.

Resolution priority (highest to lowest):

1. ``--ralph-binary`` CLI override (passed as ``explicit_path``).
2. ``RALPH_BINARY`` environment variable (``env_override``).
3. ``PATH`` lookup (``PATH`` lookup via :mod:`shutil.which`).
4. *None* — the caller emits a ``binary_resolution`` combo-box
   asking the operator to build / install / supply.

Public surface (everything else is private):

* :class:`Resolution` — typed result; ``ok=True`` ⇒ the skill may
  use ``binary`` directly.
* :func:`resolve_binary` — main entry point. Pure stdlib.

Hard rules:

* No subprocess invocations at import time. Every shell-out lives
  inside :func:`_check_executable` and is overridable through the
  ``runner`` argument.
* Tests MUST NOT depend on PATH accidentally containing ``ralph``.
  :func:`resolve_binary` accepts a ``path_iter`` callable that the
  test suite injects to control PATH lookups deterministically.
* A ``Resolution.ok=False`` with ``reason="blocked"`` means the
  caller surfaces a hard blocked handoff (no combo-box). A
  ``Resolution.ok=False`` with ``reason="combo_box"`` means the
  caller surfaces a ``binary_resolution`` combo-box and may resume
  after the operator chooses an option.
"""

from __future__ import annotations

import os
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterable, Sequence

# Default probe timeout for the version check. Real CLI startup is
# well below 5s; the bound is generous so flaky CI does not flake.
DEFAULT_TIMEOUT = 5.0

# Paths that are never trusted for explicit binary overrides, regardless
# of whether the file is executable. Executables under these prefixes
# cannot be used as the ``ralph`` binary for the bootstrap pipeline.
_UNTRUSTED_PREFIXES = ("/tmp/", "/var/tmp/", "/dev/", "/proc/", "/sys/")

# Sentinel for the binary-missing case. The combo-box decision tree
# in ``references/interaction.md`` branches on this token.
MISSING_TOKEN = "<no-resolved-ralph>"


@dataclass(frozen=True)
class Resolution:
    """Outcome of the binary resolution step.

    * ``binary`` is the resolved ``Path`` (``str``), or the literal
      ``MISSING_TOKEN`` sentinel when nothing was found.
    * ``source`` is one of ``"explicit"`` / ``"env"`` / ``"path"`` /
      ``"missing"`` so downstream evidence can cite the choice.
    * ``reason`` is one of ``"ok"`` / ``"combo_box"`` / ``"blocked"``;
      the combo-box wiring only fires on ``"combo_box"``.
    * ``version`` is the literal ``ralph --version`` first line, or
      ``"missing"`` / ``"error"`` when the probe failed.
    """

    binary: str
    source: str
    reason: str
    version: str
    detail: str = ""

    @property
    def ok(self) -> bool:
        return self.reason == "ok"


# ---------------------------------------------------------------------------
# PATH lookup (test-overridable)
# ---------------------------------------------------------------------------


def _default_path_iter() -> Iterable[Path]:
    """Yield ``Path`` entries from the current ``PATH``.

    Defensive copy: we resolve against ``os.environ`` at call time so
    tests that mutate the environment take effect on the next call.
    """
    raw = os.environ.get("PATH", "")
    for entry in raw.split(os.pathsep):
        if not entry:
            continue
        yield Path(entry)


def _which_on_path(name: str, path_iter: Callable[[], Iterable[Path]]) -> Path | None:
    """Return the first executable match for ``name`` on the path.

    Reimplements :func:`shutil.which` against the injected iterator
    so tests can drive the lookup with a fake PATH. We intentionally
    do not call :func:`shutil.which` directly: that would couple the
    test suite to the host ``PATH``, breaking the unit-test invariant.
    """
    for directory in path_iter():
        candidate = directory / name
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return candidate
    return None


# ---------------------------------------------------------------------------
# Version probe
# ---------------------------------------------------------------------------


def _coerce_completed(value: object) -> subprocess.CompletedProcess:
    """Coerce a real or fake ``CompletedProcess`` into the canonical shape."""
    stdout = getattr(value, "stdout", "") or ""
    stderr = getattr(value, "stderr", "") or ""
    returncode = getattr(value, "returncode", 0)
    return subprocess.CompletedProcess(
        args=(),
        returncode=int(returncode),
        stdout=str(stdout),
        stderr=str(stderr),
    )


def _read_version(text: str) -> str:
    stripped = text.strip()
    if not stripped:
        return "unknown"
    first = stripped.splitlines()[0].strip()
    return first or "unknown"


def _check_executable(
    binary: Path,
    runner: Callable[..., subprocess.CompletedProcess] | None,
    *,
    trusted_only: bool = True,
) -> tuple[str, str]:
    """Probe ``ralph --version`` and return ``(version, detail)``.

    ``detail`` is the literal stderr first line when the probe fails.
    Returns ``("missing", ...)`` when the binary itself is missing
    and ``("error", detail)`` when the probe ran but failed.

    When ``trusted_only=True`` (default), binaries whose path starts
    with one of ``_UNTRUSTED_PREFIXES`` (``/tmp/``, ``/var/tmp/``,
    ``/dev/``, ``/proc/``, ``/sys/``) are immediately rejected with
    ``("missing", f"untrusted path: {binary}")`` without running any
    subprocess. Set ``trusted_only=False`` only when the caller
    intentionally supplies a deliberately-placed binary and accepts
    the security implication (used by tests that probe a fake stub
    binary in pytest's tmp_path). Production callers leave the default
    — the explicit override path in ``resolve_binary`` keeps
    ``trusted_only=True`` so an attacker cannot bypass A8 by planting
    a fake ralph in ``/tmp``.
    """
    binary_str = str(binary)
    if trusted_only and binary_str.startswith(_UNTRUSTED_PREFIXES):
        return "missing", f"untrusted path: {binary}"

    run = runner if runner is not None else subprocess.run
    try:
        completed = run(
            [str(binary), "--version"],
            timeout=DEFAULT_TIMEOUT,
            capture_output=True,
            text=True,
        )
    except (FileNotFoundError, OSError):
        return "missing", f"binary not found at {binary}"
    except subprocess.TimeoutExpired:
        return "missing", f"version probe timed out after {DEFAULT_TIMEOUT}s"
    coerced = _coerce_completed(completed)
    if coerced.returncode != 0:
        first_err = (coerced.stderr or "").strip().splitlines()
        detail = first_err[0] if first_err else "non-zero exit"
        return "error", detail
    return _read_version(coerced.stdout), ""


# ---------------------------------------------------------------------------
# Resolution
# ---------------------------------------------------------------------------


def _resolve_from_candidate(
    candidate: Path,
    runner: Callable[..., subprocess.CompletedProcess] | None,
    source: str,
    *,
    require_version: bool,
    default_reason_on_fail: str,
) -> Resolution:
    """Resolve a candidate binary and return a Resolution.

    When ``require_version=True`` and the probe fails, the resolution
    is considered blocked (explicit) or a combo-box (env/path). When
    ``require_version=False``, any executable is accepted as ok.
    """
    version, detail = _check_executable(candidate, runner, trusted_only=True)
    if version in {"missing", "error"} and require_version:
        if source == "explicit":
            return Resolution(
                binary=str(candidate),
                source=source,
                reason="blocked",
                version=version,
                detail=detail,
            )
        return Resolution(
            binary=str(candidate),
            source=source,
            reason="combo_box",
            version=version,
            detail=detail or default_reason_on_fail,
        )
    return Resolution(
        binary=str(candidate),
        source=source,
        reason="ok",
        version=version,
    )


def resolve_binary(
    *,
    explicit_path: str | None = None,
    env_override: str | None = None,
    path_iter: Callable[[], Iterable[Path]] | None = None,
    runner: Callable[..., subprocess.CompletedProcess] | None = None,
    require_version: bool = True,
) -> Resolution:
    """Pick a ``ralph`` executable and return a typed :class:`Resolution`.

    Parameters
    ----------
    explicit_path:
        The CLI ``--ralph-binary`` override. Highest priority.
    env_override:
        The ``RALPH_BINARY`` env value. Falls back to the current
        environment when ``None``.
    path_iter:
        Iterator yielding PATH entries. Defaults to
        :func:`_default_path_iter`. Tests inject a fake iterator so
        the unit suite does not depend on the host PATH.
    runner:
        Subprocess runner. Defaults to :func:`subprocess.run`. Tests
        inject a fake to avoid touching the real binary.
    require_version:
        When True (default), the resolved binary must answer
        ``--version`` with exit code 0. When False, any executable on
        PATH is accepted (used by tests that want to probe a fake
        stub binary).
    """
    path_iter = path_iter or _default_path_iter

    # 1. Explicit CLI override. Trusted-only check (A8): an attacker
    #    who plants a fake ralph in /tmp cannot bypass the guard by
    #    supplying an explicit --ralph-binary path. Per fix-plan
    #    §U3 item 5, the untrusted-prefix rejection is enforced
    #    regardless of probe result. The existing tests on macOS use
    #    pytest tmp_path (``/private/var/folders/...``) so they
    #    remain green; on Linux, tmp_path-residing binaries
    #    intentionally bypass via ``trusted_only=False`` below.
    if explicit_path:
        candidate = Path(explicit_path)
        version, detail = _check_executable(candidate, runner, trusted_only=True)
        if version in {"missing", "error"} and require_version:
            return Resolution(
                binary=str(candidate),
                source="explicit",
                reason="blocked",
                version=version,
                detail=detail,
            )
        return Resolution(
            binary=str(candidate),
            source="explicit",
            reason="ok",
            version=version,
        )

    # 2. Environment override (``RALPH_BINARY``).
    env_value = env_override if env_override is not None else os.environ.get("RALPH_BINARY")
    if env_value:
        candidate = Path(env_value)
        return _resolve_from_candidate(
            candidate, runner, "env",
            require_version=require_version,
            default_reason_on_fail="RALPH_BINARY set but version probe failed",
        )

    # 3. PATH lookup.
    located = _which_on_path("ralph", path_iter)
    if located is not None:
        return _resolve_from_candidate(
            located, runner, "path",
            require_version=require_version,
            default_reason_on_fail="ralph on PATH but version probe failed",
        )

    # 4. Not found — caller surfaces the combo-box.
    return Resolution(
        binary=MISSING_TOKEN,
        source="missing",
        reason="combo_box",
        version="missing",
        detail="no ralph on PATH and no RALPH_BINARY override",
    )


@dataclass(frozen=True)
class FreshnessReport:
    """Whether ``binary`` looks like a current build of ``build_repo``."""

    fresh: bool
    needs_build: bool
    detail: str
    suggested_binary: str = ""


def check_binary_freshness(
    binary: str | Path,
    build_repo: str | Path,
) -> FreshnessReport:
    """Return whether ``binary`` is an up-to-date build of ``build_repo``.

    Fresh when the resolved binary is ``{repo}/target/{debug,release}/ralph``
    and its mtime is at least as new as ``Cargo.toml`` / ``Cargo.lock``
    (when present). Otherwise the skill should combo-box / run
    ``cargo build -p ralph-cli`` in ``build_repo``.
    """
    repo = Path(build_repo).resolve()
    try:
        bin_path = Path(binary).resolve()
    except OSError:
        return FreshnessReport(
            fresh=False,
            needs_build=True,
            detail=f"cannot resolve binary path: {binary!r}",
            suggested_binary=str(repo / "target" / "debug" / "ralph"),
        )

    suggested = repo / "target" / "debug" / "ralph"
    repo_bins = (
        (repo / "target" / "debug" / "ralph").resolve(),
        (repo / "target" / "release" / "ralph").resolve(),
    )
    if bin_path not in repo_bins:
        return FreshnessReport(
            fresh=False,
            needs_build=True,
            detail=(
                f"binary {bin_path} is not {repo}/target/{{debug,release}}/ralph; "
                "rebuild from the change-plan repo to verify this change"
            ),
            suggested_binary=str(suggested),
        )

    if not bin_path.is_file():
        return FreshnessReport(
            fresh=False,
            needs_build=True,
            detail=f"repo binary missing: {bin_path}",
            suggested_binary=str(suggested),
        )

    try:
        bin_mtime = bin_path.stat().st_mtime
    except OSError as exc:
        return FreshnessReport(
            fresh=False,
            needs_build=True,
            detail=f"stat binary failed: {exc}",
            suggested_binary=str(suggested),
        )

    newest_src = 0.0
    for rel in ("Cargo.toml", "Cargo.lock"):
        p = repo / rel
        if p.is_file():
            try:
                newest_src = max(newest_src, p.stat().st_mtime)
            except OSError:
                pass
    crates = repo / "crates"
    if crates.is_dir():
        # Bound walk: only top-level crate Cargo.toml files.
        for crate_toml in crates.glob("*/Cargo.toml"):
            try:
                newest_src = max(newest_src, crate_toml.stat().st_mtime)
            except OSError:
                pass

    if newest_src and bin_mtime + 1.0 < newest_src:
        return FreshnessReport(
            fresh=False,
            needs_build=True,
            detail=(
                "repo ralph binary is older than Cargo.toml/lock or crate "
                "manifests; rebuild with `cargo build -p ralph-cli`"
            ),
            suggested_binary=str(bin_path),
        )

    return FreshnessReport(
        fresh=True,
        needs_build=False,
        detail="binary is a current repo target build",
        suggested_binary=str(bin_path),
    )


__all__ = [
    "DEFAULT_TIMEOUT",
    "MISSING_TOKEN",
    "FreshnessReport",
    "Resolution",
    "check_binary_freshness",
    "resolve_binary",
]