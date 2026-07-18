"""Repository-relative path normalisation for the bootstrap helpers.

The audit must never surface absolute paths in handoffs. ``rel`` rewrites
any absolute path under the project root to a ``./...`` (or plain)
relative form so the operator-facing report stays portable.
"""
from __future__ import annotations

import os
from pathlib import Path


def rel(path: Path | str, root: Path | None = None) -> str:
    """Return ``path`` as a portable repo-relative string.

    Falls back to ``str(path)`` when the input is not under ``root`` (or
    the current working directory when ``root`` is not supplied).
    """
    target = Path(path)
    if root is None:
        root = Path.cwd()
    root = root.resolve()
    try:
        relative = target.resolve().relative_to(root)
    except (ValueError, OSError):
        try:
            relative = target.resolve().relative_to(Path.cwd().resolve())
        except (ValueError, OSError):
            return str(target)
    if relative == Path("."):
        return "./"
    text = relative.as_posix()
    return f"./{text}" if not text.startswith("./") else text


def is_safe_relative(path: str) -> bool:
    """Reject absolute paths and parent escapes."""
    if not path:
        return False
    if os.path.isabs(path):
        return False
    normalised = os.path.normpath(path)
    return not normalised.startswith("..")