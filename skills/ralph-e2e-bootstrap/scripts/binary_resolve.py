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
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterable, Sequence

# Default probe timeout for the version check. Real CLI startup is
# well below 5s; the bound is generous so flaky CI does not flake.
DEFAULT_TIMEOUT = 5.0

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
) -> tuple[str, str]:
    """Probe ``ralph --version`` and return ``(version, detail)``.

    ``detail`` is the literal stderr first line when the probe fails.
    Returns ``("missing", ...)`` when the binary itself is missing
    and ``("error", detail)`` when the probe ran but failed.
    """
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

    # 1. Explicit CLI override.
    if explicit_path:
        candidate = Path(explicit_path)
        version, detail = _check_executable(candidate, runner)
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
        version, detail = _check_executable(candidate, runner)
        if version in {"missing", "error"} and require_version:
            # An env-supplied path that does not exist or fails the
            # version probe is not a hard block: the operator supplied
            # the value themselves, so we surface a combo-box so they
            # can rebuild / install / override. The detail field
            # captures the failure for the handoff evidence block.
            return Resolution(
                binary=str(candidate),
                source="env",
                reason="combo_box",
                version=version,
                detail=detail or "RALPH_BINARY set but version probe failed",
            )
        return Resolution(
            binary=str(candidate),
            source="env",
            reason="ok",
            version=version,
        )

    # 3. PATH lookup.
    located = _which_on_path("ralph", path_iter)
    if located is not None:
        version, detail = _check_executable(located, runner)
        if version in {"missing", "error"} and require_version:
            return Resolution(
                binary=str(located),
                source="path",
                reason="combo_box",
                version=version,
                detail=detail or "ralph on PATH but version probe failed",
            )
        return Resolution(
            binary=str(located),
            source="path",
            reason="ok",
            version=version,
        )

    # 4. Not found — caller surfaces the combo-box.
    return Resolution(
        binary=MISSING_TOKEN,
        source="missing",
        reason="combo_box",
        version="missing",
        detail="no ralph on PATH and no RALPH_BINARY override",
    )


__all__ = [
    "DEFAULT_TIMEOUT",
    "MISSING_TOKEN",
    "Resolution",
    "resolve_binary",
]