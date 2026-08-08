"""Optional structured evaluator bridge for semantically uncertain candidates."""

from __future__ import annotations

import json
import os
import shlex
import subprocess
from typing import Any, Mapping


def evaluate(candidate: Mapping[str, Any], *, timeout: float = 3.0) -> tuple[str, str, dict[str, Any]]:
    """Run the configured evaluator without shell expansion.

    A candidate requests semantic review with ``semantic_review: true``.
    The command is configured as an argv string in
    ``RALPH_NOWLEDGE_EVALUATOR``; it must return the JSON contract described
    by ``agents/memory-evaluator.md``. Missing, malformed, timed-out, or
    write-attempting evaluators fail closed for this candidate.
    """
    command = os.environ.get("RALPH_NOWLEDGE_EVALUATOR", "").strip()
    if not command:
        return "REJECTED", "semantic evaluator is required but not configured", {}
    try:
        argv = shlex.split(command)
    except ValueError as exc:
        return "REJECTED", f"semantic evaluator command is invalid: {exc}", {}
    if not argv:
        return "REJECTED", "semantic evaluator command is empty", {}
    try:
        completed = subprocess.run(
            argv,
            input=json.dumps(dict(candidate), ensure_ascii=False),
            capture_output=True,
            text=True,
            check=False,
            timeout=timeout,
            env=os.environ.copy(),
        )
    except (FileNotFoundError, OSError) as exc:
        return "REJECTED", f"semantic evaluator could not start: {exc}", {}
    except subprocess.TimeoutExpired:
        return "REJECTED", "semantic evaluator exceeded its bounded timeout", {}
    if completed.returncode != 0:
        return "REJECTED", "semantic evaluator returned a non-zero status", {}
    try:
        verdict = json.loads(completed.stdout or "{}")
    except json.JSONDecodeError:
        return "REJECTED", "semantic evaluator returned invalid JSON", {}
    if not isinstance(verdict, dict):
        return "REJECTED", "semantic evaluator returned a non-object", {}
    if any(key in verdict for key in ("nmem", "write", "transcript", "command")):
        return "REJECTED", "semantic evaluator response contains a forbidden side-effect field", {}
    result = verdict.get("verdict")
    if result not in {"ACCEPTED", "REJECTED", "NEEDS_REWRITE"}:
        return "REJECTED", "semantic evaluator verdict is not in the allowed set", {}
    reasons = verdict.get("reasons", [])
    if not isinstance(reasons, list) or not all(isinstance(item, str) for item in reasons):
        return "REJECTED", "semantic evaluator reasons must be a string list", {}
    return str(result), "; ".join(reasons)[:2000], verdict
