"""Hook runtime for the Nowledge Mem Ralph plugin.

The runtime is the single entry point invoked by Claude Code for every
lifecycle hook declared in ``hooks/hooks.json``. It enforces two
non-negotiable contracts before any other module is allowed to run:

1. **Ralph env gate** — If ``RALPH_CURRENT_LOOP_ID`` is missing, the hook
   is a no-op (exit 0, empty stdout, no file writes, no ``nmem``
   invocation). This guarantees that a human Claude Code session — which
   never has Ralph loop env — cannot be silently captured or have
   additional context injected by the plugin.

2. **Timeout discipline** — Every subprocess spawned by hook handlers
   must use ``subprocess.run([...], timeout=...)``. The hook itself has
   a 5-second budget declared in ``hooks/hooks.json``.

Subsequent units (U02 recall, U03 memory policy, U04 writer, U05 audit)
extend this skeleton. ``resolve_nowledge_env`` is the canonical mapping
between Ralph's existing ``RALPH_*`` env keys and the plugin's internal
``Nowledge*`` field names; downstream code MUST read the env through this
function so the mapping stays in one place.
"""

from __future__ import annotations

import importlib.util
import json
import os
import sys
import tempfile
from pathlib import Path
from typing import Any

# Exit code contract (locked in U01; U02-U06 must not change):
#   0  — success (incl. no-op when env missing)
#   1  — recoverable error (logged to stderr, stdout empty)
#   2  — internal bug (stdout/stderr both empty, no auto-retry)
EXIT_OK = 0
EXIT_RECOVERABLE = 1
EXIT_BUG = 2

# Ralph env keys the plugin reuses (no RALPH_NOWLEDGE_* namespace is
# created — see F2 in the inspection report).
_RALPH_ENV_KEYS = (
    "RALPH_CURRENT_HAT",
    "RALPH_CURRENT_LOOP_ID",
    "RALPH_EVENTS_FILE",
    "RALPH_TRIGGERED_HAT",
    "RALPH_HATS_SOURCE",
    "RALPH_CONFIG",
    "RALPH_WORKSPACE_ROOT",
)


def _log(kind: str, **fields: Any) -> None:
    """Structured stderr log line; never raises."""
    payload = {"event": kind, "plugin": "nowledge-mem-ralph"}
    payload.update(fields)
    try:
        sys.stderr.write(json.dumps(payload, ensure_ascii=False) + "\n")
        sys.stderr.flush()
    except Exception:
        # Logging must never break a hook.
        pass


def _nowledge_env_present() -> bool:
    """Return True only if the process carries Ralph loop env."""
    return bool(os.environ.get("RALPH_CURRENT_LOOP_ID", "").strip())


def resolve_nowledge_env() -> dict[str, str]:
    """Map Ralph's existing env keys to plugin-internal field names.

    The returned dict is the canonical view of "what Ralph loop is this
    hook running in". Downstream code (U02 recall, U03 policy, U04
    writer) MUST go through this function instead of re-reading
    ``os.environ`` directly so the env contract has a single owner.

    Missing keys are surfaced as empty strings; callers must check for
    emptiness when a field is required (e.g. ``loop_id``).
    """
    return {key: os.environ.get(key, "").strip() for key in _RALPH_ENV_KEYS}


def _read_hook_stdin() -> dict[str, Any]:
    """Read the Claude Code hook stdin payload (best-effort).

    A missing or malformed payload must NEVER turn into a hook failure —
    hooks that can't parse their input should log and return OK so the
    session can still start.
    """
    try:
        raw = sys.stdin.read()
    except Exception as exc:
        _log("hook_stdin_unreadable", error=str(exc))
        return {}
    if not raw.strip():
        return {}
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError as exc:
        _log("hook_stdin_invalid_json", error=str(exc))
        return {}
    if not isinstance(payload, dict):
        _log("hook_stdin_not_object", type=type(payload).__name__)
        return {}
    return payload


def _state_root() -> Path:
    """Compute the plugin state root under ``CLAUDE_PLUGIN_DATA``.

    Falls back to a per-process temp dir when ``CLAUDE_PLUGIN_DATA`` is
    not set (e.g. tests). The fallback never collides with another
    process because it embeds the PID.
    """
    base = os.environ.get("CLAUDE_PLUGIN_DATA", "").strip()
    if base:
        return Path(base)
    return Path(tempfile.gettempdir()) / f"nowledge-mem-ralph-fallback-{os.getpid()}"


def _atomic_write_json(path: Path, payload: dict[str, Any]) -> None:
    """Write ``payload`` to ``path`` via temp file + os.replace."""
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(payload, ensure_ascii=False, sort_keys=True), encoding="utf-8")
    os.replace(tmp, path)


# ---------------------------------------------------------------------------
# Recall module loader (U02)
# ---------------------------------------------------------------------------


_RECALL_MODULE: Any | None = None


def _load_recall_module() -> Any:
    """Load ``scripts/recall.py`` as a module regardless of plugin name.

    The plugin directory contains a hyphen (``nowledge-mem-ralph``) so it
    is *not* importable as a Python package by name. We load via
    ``importlib`` and cache the result so the hook process pays the
    import cost once. The module is registered in ``sys.modules`` under
    its canonical name so dataclass introspection works on Python 3.14+
    (otherwise the frozen dataclass decorator fails with a NoneType
    traceback from ``sys.modules.get``).
    """
    global _RECALL_MODULE
    if _RECALL_MODULE is not None:
        return _RECALL_MODULE
    here = Path(__file__).resolve().parent
    module_name = "_nowledge_mem_ralph_recall"
    spec = importlib.util.spec_from_file_location(module_name, here / "recall.py")
    if spec is None or spec.loader is None:
        raise RuntimeError("failed to load recall.py spec")
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    _RECALL_MODULE = module
    return module


def _handle_session_start(payload: dict[str, Any]) -> int:
    """SessionStart handler — U01 scaffold + U02 recall wiring.

    U02 layered bounded recall + loop cache on top of U01's
    env-gated state marker. The hook still writes a state marker
    (now enriched with ``cache_status``) and emits an
    ``additionalContext`` block via stdout when the recall path
    produces non-empty output. Empty stdout = Claude starts without
    injected context.
    """
    env = resolve_nowledge_env()
    loop_id = env["RALPH_CURRENT_LOOP_ID"]
    state_path = _state_root() / loop_id / "state.json"

    cache_status = "noop"
    additional_context = ""
    try:
        recall = _load_recall_module()
        result = recall.run_loop_recall(env, payload)
        cache_status = result.source_metadata.get("cache_status", "noop")
        additional_context = result.context_xml
        _log(
            "session_start_recall_complete",
            loop_id=loop_id,
            state=result.state,
            cache_status=cache_status,
            context_bytes=len(additional_context.encode("utf-8")) if additional_context else 0,
        )
    except Exception as exc:
        # Recall is best-effort — log and continue with empty context.
        _log(
            "session_start_recall_unhandled",
            loop_id=loop_id,
            error=str(exc),
            type=type(exc).__name__,
        )
        cache_status = "err"

    _atomic_write_json(
        state_path,
        {
            "hook": "SessionStart",
            "loop_id": loop_id,
            "hat": env["RALPH_CURRENT_HAT"],
            "session_id": str(payload.get("session_id", "")),
            "source": str(payload.get("source", "")),
            "cache_status": cache_status,
        },
    )

    if additional_context:
        # Encode as JSON so Claude Code can parse it as hook output.
        sys.stdout.write(json.dumps({"additionalContext": additional_context}))
    return EXIT_OK


def _handle_stop(payload: dict[str, Any]) -> int:
    """Stop handler — audit-only.

    U05 will replace the body with a full Stop audit (read state.json,
    append audit record). U01's job is to lock the contract: no
    transcript reads, no ``nmem`` invocation, no second save attempt.
    The Stop handler must NEVER block the agent — it logs and exits.
    """
    env = resolve_nowledge_env()
    # Explicit guard: we are not allowed to read transcript_path or any
    # last_assistant_message — write a marker that proves the guard
    # fired and stop.
    state_path = _state_root() / env["RALPH_CURRENT_LOOP_ID"] / "state.json"
    _atomic_write_json(
        state_path,
        {
            "hook": "Stop",
            "loop_id": env["RALPH_CURRENT_LOOP_ID"],
            "hat": env["RALPH_CURRENT_HAT"],
            "audit_only": True,
            "stop_hook_fired": True,
        },
    )
    _log("stop_audit_recorded", loop_id=env["RALPH_CURRENT_LOOP_ID"])
    return EXIT_OK


_HANDLERS = {
    "SessionStart": _handle_session_start,
    "Stop": _handle_stop,
}


def main(argv: list[str] | None = None) -> int:
    """Hook entry point dispatched by ``argv[1]`` event name."""
    argv = list(sys.argv if argv is None else argv)
    event = argv[1] if len(argv) > 1 else os.environ.get("NOWLEDGE_HOOK_EVENT", "")
    if event not in _HANDLERS:
        _log("hook_unknown_event", event=event)
        return EXIT_BUG

    # Gate 1: Ralph env must be present. Missing env = noop.
    if not _nowledge_env_present():
        _log("hook_skipped_no_ralph_env", event=event)
        # Empty stdout, exit 0 — Claude session starts normally.
        return EXIT_OK

    # Gate 2: parse stdin. Failures are recoverable.
    payload = _read_hook_stdin()

    try:
        return _HANDLERS[event](payload)
    except Exception as exc:
        # Hooks must never crash Claude. Log and exit 1.
        _log("hook_unhandled_exception", event=event, error=str(exc), type=type(exc).__name__)
        return EXIT_RECOVERABLE


if __name__ == "__main__":
    raise SystemExit(main())