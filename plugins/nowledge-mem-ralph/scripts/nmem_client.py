"""Small argv-only client for the nmem commands used by save-memory.

The client intentionally has no shell mode.  A caller can inject ``runner``
in tests; production calls are bounded by ``timeout`` and return structured
failure information instead of raising into the agent session.
"""

from __future__ import annotations

import json
import os
import subprocess
from typing import Any, Callable, Sequence


DEFAULT_TIMEOUT_SECONDS = 4.0
Runner = Callable[..., subprocess.CompletedProcess[str]]


def _text(value: object, limit: int = 2000) -> str:
    if not isinstance(value, str):
        value = str(value)
    return " ".join(value.split())[:limit]


def memory_content(record: dict[str, Any]) -> str:
    """Build bounded human-readable content without transcript material."""
    parts = [
        record.get("claim", ""),
        f"Why it matters: {record.get('why_it_matters', '')}",
        f"Evidence: {record.get('evidence', '')}",
        f"Applies when: {record.get('applies_when', '')}",
        f"Verification: {record.get('verification', '')}",
    ]
    return "\n".join(_text(part) for part in parts if _text(part))[:9000]


def build_add_argv(record: dict[str, Any]) -> list[str]:
    """Return the only nmem write argv shape used by this plugin."""
    argv = ["nmem", "--json", "m", "add", memory_content(record), "--title", _text(record["title"], 300)]
    source = record.get("source")
    if isinstance(source, dict):
        scope = _text(source.get("scope", record.get("scope", "")), 300)
    else:
        scope = _text(record.get("scope", ""), 300)
    if scope:
        argv.extend(["-s", scope])
    argv.extend(["--unit-type", "learning"])
    return argv


def add_memory(
    record: dict[str, Any],
    *,
    runner: Runner | None = None,
    timeout: float = DEFAULT_TIMEOUT_SECONDS,
) -> tuple[str, str, str | None]:
    """Run bounded nmem add and return ``(status, reason, memory_id)``."""
    run = runner or subprocess.run
    try:
        completed = run(
            build_add_argv(record),
            capture_output=True,
            text=True,
            check=False,
            timeout=timeout,
            env=os.environ.copy(),
        )
    except FileNotFoundError:
        return "FAILED_OPEN", "nmem command is not available", None
    except subprocess.TimeoutExpired:
        return "UNKNOWN", "nmem write exceeded its bounded timeout", None
    except OSError as exc:
        return "FAILED_OPEN", f"nmem process could not start: {exc}", None
    if completed.returncode != 0:
        return "FAILED_OPEN", f"nmem write failed with exit code {completed.returncode}", None
    try:
        payload = json.loads(completed.stdout or "{}")
    except json.JSONDecodeError:
        return "UNKNOWN", "nmem returned invalid JSON after write", None
    if not isinstance(payload, dict):
        return "UNKNOWN", "nmem returned a non-object write response", None
    memory_id = payload.get("id") or payload.get("memory_id")
    return "SAVED", "nmem accepted the Memory", str(memory_id) if memory_id else None
