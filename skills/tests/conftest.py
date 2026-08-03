"""Pytest fixtures and import shims for the public skill test suites.

The skills ship helpers as flat modules under
``skills/<name>/scripts/`` rather than installable packages. We need
those modules to be importable by name (``audit``, ``_paths``, etc.) so
both the bootstrap contract suite and the installer suite can exercise
them without spinning up a package.

This conftest lives under ``skills/tests/`` and pre-loads the helper
modules into ``sys.modules`` so the tests do not have to mutate
``sys.path`` themselves.
"""
from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SKILLS_DIR = ROOT / "skills"
if str(SKILLS_DIR) not in sys.path:
    sys.path.insert(0, str(SKILLS_DIR))


def _load(module_name: str, file_path: Path) -> None:
    if module_name in sys.modules:
        return
    spec = importlib.util.spec_from_file_location(module_name, file_path)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load spec for {module_name} at {file_path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)


_BOOTSTRAP_SCRIPTS = SKILLS_DIR / "ralph-project-bootstrap" / "scripts"
for _name, _path in (
    ("_paths", _BOOTSTRAP_SCRIPTS / "_paths.py"),
    ("audit", _BOOTSTRAP_SCRIPTS / "audit.py"),
    ("_fixtures", _BOOTSTRAP_SCRIPTS / "_fixtures.py"),
    ("agent_docs", _BOOTSTRAP_SCRIPTS / "agent_docs.py"),
    ("pipeline_suite", _BOOTSTRAP_SCRIPTS / "pipeline_suite.py"),
    ("cli_probe", _BOOTSTRAP_SCRIPTS / "cli_probe.py"),
    ("_probe_runner", _BOOTSTRAP_SCRIPTS / "_probe_runner.py"),
    ("smoke_runner", _BOOTSTRAP_SCRIPTS / "smoke_runner.py"),
    ("handoff", _BOOTSTRAP_SCRIPTS / "handoff.py"),
    ("bootstrap_pipeline", _BOOTSTRAP_SCRIPTS / "bootstrap_pipeline.py"),
):
    if _path.is_file():
        _load(_name, _path)


# Pre-load the ralph-e2e-bootstrap helpers under distinct module
# names so they don't clash with ralph-project-bootstrap's
# identically-named ``handoff`` module.
_E2E_BOOTSTRAP_SCRIPTS = SKILLS_DIR / "ralph-e2e-bootstrap" / "scripts"
for _name, _path in (
    ("plan_diff", _E2E_BOOTSTRAP_SCRIPTS / "plan_diff.py"),
    ("plan_resolve", _E2E_BOOTSTRAP_SCRIPTS / "plan_resolve.py"),
    ("binary_resolve", _E2E_BOOTSTRAP_SCRIPTS / "binary_resolve.py"),
    ("sandbox_suite", _E2E_BOOTSTRAP_SCRIPTS / "sandbox_suite.py"),
    ("gate", _E2E_BOOTSTRAP_SCRIPTS / "gate.py"),
    ("e2e_handoff", _E2E_BOOTSTRAP_SCRIPTS / "e2e_handoff.py"),
    # Distinct name: ralph-project-bootstrap's ``bootstrap_pipeline``
    # is preloaded above under the plain name; loading the e2e module
    # under its own name keeps both suites importable in one process.
    ("e2e_bootstrap_pipeline", _E2E_BOOTSTRAP_SCRIPTS / "bootstrap_pipeline.py"),
):
    if _path.is_file():
        _load(_name, _path)
