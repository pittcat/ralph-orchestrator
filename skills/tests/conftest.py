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
):
    if _path.is_file():
        _load(_name, _path)