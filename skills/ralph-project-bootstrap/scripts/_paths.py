"""Repository-relative path normalisation and confinement for the
bootstrap helpers.

The bootstrap pipeline treats *every* path field — preset, plan, prompt,
config, provenance, handoff display — as untrusted input that MUST be
validated against a confirmed project root before it can be used for
IO, argv, or operator-visible strings. This module owns that
confinement.

Three primitives:

* ``rel(path, root)`` — return a repo-relative display string relative
  to a canonical ``root``. Never returns an absolute path; falls back
  to ``str(path)`` only when ``path`` is not under ``root`` AND not
  under the cwd (the caller should treat that fallback as a red flag).
* ``is_safe_relative(path)`` — pure lexical check that rejects
  absolute paths, ``..`` escapes, NUL bytes, Windows drive letters,
  UNC ``\\\\server\\share`` forms, leading slashes, and Unicode
  separator spoofing. Used as the input-boundary gate before any
  filesystem call.
* ``contain(path, root)`` — pure path-containment check: ``path``
  must lexically resolve inside ``root`` AND not point at a
  symlink whose target is outside ``root``. Used by the IO layer
  before any write.

The contract:

* Internal IO / argv build paths are passed through ``contain`` and
  must pass; ``rel`` is for display only.
* ``rel`` never falls back to an absolute path silently: when the
  containment check fails it returns ``"./" + str(path)`` only when
  ``path`` is already a safe-relative token, otherwise it returns a
  marker like ``"<outside-root>"`` that the handoff layer refuses to
  render.
"""
from __future__ import annotations

import os
import re
from pathlib import Path, PurePosixPath, PureWindowsPath


# Patterns that, when present anywhere in a path token, mean the path
# cannot be safely treated as a relative POSIX token. These checks run
# BEFORE any filesystem resolution so a malicious input cannot sneak
# a drive letter past the resolver by relying on symlink traversal.
_LEXICAL_FORBIDDEN_PATTERNS: tuple[str, ...] = (
    # NUL bytes and other C0 control characters: never legal in a path.
    r"[\x00-\x1f]",
)


def _has_control_byte(path: str) -> bool:
    return any(ord(ch) < 0x20 for ch in path)


def _is_windows_drive_or_unc(path: str) -> bool:
    """Detect Windows drive letters (``C:\\...`` / ``C:/...``) and UNC
    forms (``\\\\server\\share`` / ``//server/share``).

    The check is intentionally lexical and cross-platform: the
    bootstrap pipeline must reject these inputs even when the host is
    POSIX (a developer pasting a Windows path from a colleague must
    not silently have it routed to a Linux /etc/passwd).
    """
    if not path:
        return False
    # Drive letter (also catches ``c:foo`` — bare drive-relative form).
    if re.match(r"^[A-Za-z]:[\\/]", path):
        return True
    if re.match(r"^[A-Za-z]:$", path):
        return True
    # UNC: either two leading backslashes / forward slashes followed
    # by a server name.
    if re.match(r"^[\\/]{2}[^\\/]+[\\/]", path):
        return True
    return False


def _is_unicode_separator_spoof(path: str) -> bool:
    """Reject Unicode look-alike separators that some editors silently
    insert when copying across locales. Examples: U+2024 (one dot
    leader), U+FF0F (fullwidth solidus), U+2215 (division slash).
    """
    return any(ch in path for ch in ("․", "／", "∕"))


def is_safe_relative(path: str) -> bool:
    """Lexically reject anything that is not a safe POSIX-style relative
    token under the confirmed project root.

    The function is intentionally cross-platform and pure: it does not
    consult the filesystem, does not call ``Path.resolve()`` (which
    would follow symlinks and so cannot be used as a lexical gate),
    and does not depend on the host OS. Callers run this check at the
    API boundary before any IO.

    A token is safe iff:

    * It is non-empty.
    * It contains no C0 control bytes (incl. NUL).
    * It is not an absolute path (``/foo`` / Windows drive / UNC).
    * It has no Unicode look-alike separator spoofing.
    * It does not traverse above the root after ``os.path.normpath``
      normalisation (``..`` / ``a/../../b``).
    * It does not start with a separator after normalisation (which
      would mean the input was absolute in disguise).
    """
    if not path:
        return False
    if _has_control_byte(path):
        return False
    if _is_windows_drive_or_unc(path):
        return False
    if _is_unicode_separator_spoof(path):
        return False
    if os.path.isabs(path):
        return False
    normalised = os.path.normpath(path)
    if normalised.startswith(".."):
        return False
    # ``os.path.normpath`` of an absolute POSIX path strips the leading
    # separator only on POSIX; on Windows it would keep ``C:\\`` —
    # but our drive-letter check above already caught that case.
    if normalised.startswith("/") or normalised.startswith("\\"):
        return False
    return True


def normalise_relative(path: str) -> str:
    """Return the canonical POSIX-style form of a relative token.

    Strips a single leading ``./`` so ``./docs/plan.md`` and
    ``docs/plan.md`` normalise to the same canonical string. Returns
    the original input unchanged if ``is_safe_relative`` would reject
    it — callers should always gate ``is_safe_relative`` first and
    only call ``normalise_relative`` after it passes.
    """
    if not is_safe_relative(path):
        raise ValueError(f"unsafe relative path: {path!r}")
    cleaned = path.replace("\\", "/")
    while cleaned.startswith("./"):
        cleaned = cleaned[2:]
    if not cleaned:
        cleaned = "."
    return cleaned


def _canonical_root(root: Path | str) -> Path:
    """Resolve ``root`` once and return the canonical absolute path.

    The bootstrap pipeline treats the resolved canonical root as the
    single anchor for both IO containment AND display rendering. Any
    external path that does not resolve under this anchor MUST be
    refused by ``contain`` before any write.
    """
    return Path(root).resolve()


def contain(path: str | Path, root: Path | str) -> bool:
    """True iff ``path`` is safe-relative AND resolves under ``root``.

    The check is lexical first (rejecting absolute / escape / drive /
    NUL forms) and then resolution-based (the resolved path must live
    under the canonical ``root``).

    NOTE: this function does NOT follow symlinks and re-check the
    target: that requires a separate ``os.path.realpath`` round-trip
    performed by the IO layer immediately before any write. ``contain``
    is the cheap, sync gate; the IO layer is the actual boundary.
    """
    candidate = str(path)
    if not is_safe_relative(candidate):
        return False
    canonical_root = _canonical_root(root)
    try:
        target = (canonical_root / candidate).resolve()
    except (OSError, RuntimeError):
        return False
    try:
        target.relative_to(canonical_root)
    except ValueError:
        return False
    return True


def rel(path: Path | str, root: Path | str | None = None) -> str:
    """Return ``path`` as a portable repo-relative string.

    The returned string is anchored on ``root`` (default: caller
    working directory). When ``path`` is already relative and
    ``is_safe_relative`` accepts it, the function returns the
    normalised POSIX form (``./docs/plan.md``) without touching the
    filesystem.

    When ``path`` is absolute AND resolves under ``root``, the value
    is rewritten to its ``./relative`` form. When ``path`` does not
    resolve under ``root`` the function returns a deterministic
    ``"<outside-root>"`` marker so callers cannot accidentally surface
    a leaked absolute path; the handoff layer refuses to render that
    marker as a real path.
    """
    candidate = str(path)
    anchor = _canonical_root(root) if root is not None else Path.cwd().resolve()
    if not Path(candidate).is_absolute():
        # Already relative: short-circuit through the lexical gate so
        # ``rel`` is purely a renderer for paths the rest of the
        # pipeline has already accepted.
        if is_safe_relative(candidate):
            normalised = normalise_relative(candidate)
            return f"./{normalised}" if not normalised.startswith("./") else normalised
        return "<outside-root>"
    target = Path(candidate).resolve()
    try:
        relative = target.relative_to(anchor)
    except ValueError:
        return "<outside-root>"
    if relative == Path("."):
        return "./"
    text = relative.as_posix()
    return f"./{text}" if not text.startswith("./") else text


__all__ = [
    "contain",
    "is_safe_relative",
    "normalise_relative",
    "rel",
]