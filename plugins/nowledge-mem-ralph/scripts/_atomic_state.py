"""Shared atomic state.json writer + per-loop fcntl lock.

Concurrent Stop / SubagentStop hooks from parallel ``parallel-forge`` and
supervisor waves share a single ``state.json`` file under
``CLAUDE_PLUGIN_DATA/<loop_id>/state.json``. Two writers racing on the
historical ``state.json.tmp`` name caused TOCTOU loss of audit rows
(adversarial:A2). This helper:

* Writes through a unique ``tempfile.mkstemp`` tmp name in the same
  directory and atomically replaces ``state.json`` (mirrors the existing
  pattern in ``memory_writer._write_ledger``).
* Serialises the read-modify-write sequence with ``fcntl.flock`` on
  ``state.json.lock`` so the finalization block cannot be torn across
  concurrent writers.

The lock is fail-open: ``fcntl.flock`` failures fall back to an
unlocked write so the hook can stay exit 0 even on read-only mounts.
"""

from __future__ import annotations

import contextlib
import errno
import fcntl
import json
import os
import tempfile
from pathlib import Path
from typing import Any, Iterator


def state_lock_path(state_path: Path) -> Path:
    """Return the per-loop lock file path next to ``state_path``."""
    return state_path.parent / "state.lock"


@contextlib.contextmanager
def _state_lock(state_path: Path) -> Iterator[None]:
    """Acquire an advisory ``fcntl.flock`` on the per-loop lock file.

    A missing parent directory is created on demand. The lock is held
    for the duration of the wrapped block; ``fcntl.flock`` releases on
    file close even when the process exits.
    """
    lock_path = state_lock_path(state_path)
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    handle = lock_path.open("a+")
    try:
        try:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
        except OSError as exc:
            if exc.errno not in (errno.EACCES, errno.EROFS, errno.ENOSYS):
                raise
        yield
    finally:
        try:
            fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
        except OSError:
            pass
        handle.close()


def read_state(state_path: Path) -> dict[str, Any]:
    """Return the current state.json payload as a dict, or ``{}`` on miss."""
    try:
        loaded = json.loads(state_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}
    return dict(loaded) if isinstance(loaded, dict) else {}


def atomic_write_state(state_path: Path, payload: dict[str, Any]) -> None:
    """Atomically write ``payload`` to ``state_path``.

    A unique ``tempfile.mkstemp`` tmp file is created in the same
    directory, populated with the JSON-encoded payload, then
    atomically replaced. Concurrent writers cannot clobber each other
    because each gets a unique tmp name; the surrounding
    :func:`state_writing` helper adds the flock.
    """
    state_path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(prefix="state-", dir=state_path.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(payload, handle, ensure_ascii=False, sort_keys=True)
        os.replace(tmp_name, state_path)
    finally:
        try:
            os.unlink(tmp_name)
        except FileNotFoundError:
            pass


@contextlib.contextmanager
def state_writing(state_path: Path) -> Iterator[dict[str, Any]]:
    """Context manager that yields the current state and writes back on exit.

    Within the block, mutate the yielded dict. On exit, the dict is
    written to ``state_path`` under the per-loop ``fcntl.flock``. Any
    ``OSError`` during the write is logged to stderr but never raised
    so the hook can stay fail-open.
    """
    with _state_lock(state_path):
        current = read_state(state_path)
        yield current
        try:
            atomic_write_state(state_path, current)
        except OSError:
            pass