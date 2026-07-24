"""Shared probe-runner factory for ralph-e2e-bootstrap tests.

Re-exports :func:`ralph_project_bootstrap.scripts._probe_runner.make_runner`
as :func:`e2e_make_runner`, plus convenience builders for common
``FakeInvocation`` shapes used across the e2e-bootstrap test suite.

This module MUST NOT import from ``ralph_e2e_bootstrap`` production scripts
— it is a test-only helper loaded by conftest alongside the production scripts.
"""

from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

# Resolve the sibling probe path: we are under skills/tests/, sibling is
# skills/ralph-project-bootstrap/scripts/cli_probe.py and _probe_runner.py.
_TESTS_DIR = Path(__file__).resolve().parent
_SKILLS_ROOT = _TESTS_DIR / ".."
_PROBE_RUNNER_FILE = _SKILLS_ROOT / "ralph-project-bootstrap" / "scripts" / "_probe_runner.py"
_CLI_PROBE_FILE = _SKILLS_ROOT / "ralph-project-bootstrap" / "scripts" / "cli_probe.py"


def _load_sibling(file_path: Path, module_name: str):
    """Load a sibling script into sys.modules via importlib spec."""
    if module_name in sys.modules:
        return sys.modules[module_name]
    spec = importlib.util.spec_from_file_location(module_name, file_path)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load spec for {module_name} at {file_path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    return module


# Load the sibling probe runner + FakeInvocation so we can re-export them.
_probe_runner_mod = _load_sibling(_PROBE_RUNNER_FILE, "_probe_runner")
_cli_probe_mod = _load_sibling(_CLI_PROBE_FILE, "cli_probe")


def e2e_make_runner(
    invocations: list[_cli_probe_mod.FakeInvocation],
) -> "subprocess.run":
    """Return a subprocess-run compatible callable that replays ``invocations``.

    This is a typed alias for
    ``ralph_project_bootstrap.scripts._probe_runner.make_runner``.
    """
    return _probe_runner_mod.make_runner(invocations)


FakeInvocation = _cli_probe_mod.FakeInvocation


def dry_run_ok_invocation(
    binary: str,
    config: str,
    preset: str,
    plan: str,
) -> FakeInvocation:
    """Return a FakeInvocation for a successful ``ralph run --dry-run``.

    The stdout is the canonical dry-run configuration block Ralph emits.
    """
    return FakeInvocation(
        argv_expected=(binary, "-c", config, "-H", preset, "run", "--dry-run", "--plan", plan),
        stdout_chunks=(
            "Dry run mode - configuration:\n"
            "  Backend: fake\n"
            "  Prompt file: PROMPT.foo.md\n"
            "  Max iterations: 1\n"
            "  Max runtime: 60\n",
        ),
        stderr_chunks=(),
        exit_code=0,
    )


def version_probe_invocation(
    binary: str,
    version: str = "ralph 0.0.0",
) -> FakeInvocation:
    """Return a FakeInvocation for a successful ``ralph --version`` probe."""
    return FakeInvocation(
        argv_expected=(binary, "--version"),
        stdout_chunks=(version + "\n",),
        stderr_chunks=(),
        exit_code=0,
    )


def capability_probe_invocation(
    binary: str,
) -> FakeInvocation:
    """Return a FakeInvocation for ``ralph --help`` (capability probe)."""
    return FakeInvocation(
        argv_expected=(binary, "--help"),
        stdout_chunks=(
            "ralph --help\n"
            "  --json\n"
            "  --version\n"
            "  --config PATH\n"
            "\n"
            "Commands:\n"
            "  preset    Manage presets\n"
            "  preflight    Check configuration\n"
            "  run       Run the loop\n",
        ),
        stderr_chunks=(),
        exit_code=0,
    )


def preset_check_ok_invocation(
    binary: str,
    config: str,
    preset: str,
) -> FakeInvocation:
    """Return a FakeInvocation for a passing ``ralph preset check --strict``."""
    return FakeInvocation(
        argv_expected=(binary, "-c", config, "-H", preset, "preset", "check", "--strict"),
        stdout_chunks=("preset check OK\n",),
        stderr_chunks=(),
        exit_code=0,
    )


def preflight_ok_invocation(
    binary: str,
    config: str,
    preset: str,
) -> FakeInvocation:
    """Return a FakeInvocation for a passing ``ralph preflight --strict``."""
    return FakeInvocation(
        argv_expected=(binary, "-c", config, "-H", preset, "preflight", "--strict"),
        stdout_chunks=("preflight OK\n",),
        stderr_chunks=(),
        exit_code=0,
    )
