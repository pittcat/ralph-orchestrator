"""Shared test fixtures for the Nowledge Mem Ralph plugin tests.

Fix U7 maintenance:M3 consolidates ``_write_fake_nmem``,
``_valid_candidate_marker`` / ``_candidate_marker``, and the Ralph
env builder into one module so individual test files stop drifting.
The helpers are intentionally narrow: they are plugin-internal
plumbing, not a public fixture API.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
PLUGIN_DIR = ROOT / "plugins" / "nowledge-mem-ralph"
HOOK_RUNTIME = PLUGIN_DIR / "scripts" / "hook_runtime.py"
SCRIPTS_DIR = PLUGIN_DIR / "scripts"


def _write_fake_nmem(bin_dir: Path, response: str = '{"id":"mem-1"}', exit_code: int = 0) -> Path:
    """Install a fake ``nmem`` script in ``bin_dir`` and return its call log path."""
    bin_dir.mkdir(parents=True, exist_ok=True)
    calls = bin_dir / "calls.jsonl"
    script = bin_dir / "nmem"
    calls_path = str(calls.resolve())
    script.write_text(
        "#!/usr/bin/env python3\n"
        "import json, sys\n"
        f"with open({calls_path!r}, 'a', encoding='utf-8') as _h:\n"
        "    _h.write(json.dumps(sys.argv[1:]) + '\\n')\n"
        "sys.stdout.write(" + repr(response) + " + '\\n')\n"
        f"sys.exit({exit_code})\n",
        encoding="utf-8",
    )
    script.chmod(0o755)
    return calls


def _valid_candidate_marker(**overrides) -> str:
    """Build a legal finalization marker (one bounded fenced block)."""
    candidate = {
        "memory_type": "durable_decision",
        "title": "Use atomic os.replace for state.json writes",
        "claim": "Atomic writes avoid torn state.",
        "why_it_matters": "Half-written files break env detection.",
        "evidence": "hooks/hooks.json timeout=5; writer test.",
        "applies_when": "any state.json write",
        "scope": "plugin:knowledge-mem-ralph",
        "verification": "pytest proves no torn writes.",
        "critical_assumptions": [],
        "critical_ambiguities": [],
        "metrics": {
            "confidence": 95,
            "evidence_coverage": 88,
            "reusability": 90,
            "stability": 92,
            "scope_clarity": 96,
            "verifiability": 90,
            "novelty": 40,
        },
        "finalize": True,
    }
    candidate.update(overrides)
    body = json.dumps(candidate, ensure_ascii=False)
    return f"<!-- nowledge-memory-finalize\n{body}\n-->"


def _candidate_marker(**overrides) -> str:
    """Bridge-lens variant of ``_valid_candidate_marker`` (U4 e2e)."""
    return _valid_candidate_marker(**overrides)


def _ralph_env(**overrides) -> dict[str, str]:
    """Return a minimal Ralph loop env suitable for hook subprocess tests."""
    env = {
        "RALPH_NOWLEDGE_ENABLED": "1",
        "RALPH_CURRENT_LOOP_ID": "loop-xyz",
        "RALPH_CURRENT_HAT": "executor",
    }
    env.update(overrides)
    return env