"""Helpers for building throwaway target-project fixtures used by the
bootstrap contract tests.

The fixtures live under ``skills/ralph-project-bootstrap/fixtures/projects``
and are deliberately tiny: each project is a self-contained directory tree
that exercises one branch of the audit logic (blank, Rust, Node, Python,
unknown, ambiguous-root).
"""
from __future__ import annotations

import shutil
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FIXTURES_DIR = ROOT / "fixtures" / "projects"


def _write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def materialise(name: str, target: Path) -> Path:
    """Copy the named fixture into ``target`` and return the project root."""
    src = FIXTURES_DIR / name
    if not src.is_dir():
        raise FileNotFoundError(f"missing fixture source: {src}")
    target.mkdir(parents=True, exist_ok=True)
    # Copytree refuses to copy into an existing dir; fall back to manual copy.
    if target.exists():
        shutil.rmtree(target)
    shutil.copytree(src, target, ignore=shutil.ignore_patterns(".gitkeep"))
    return target


def write_blank_project(target: Path) -> Path:
    target.mkdir(parents=True, exist_ok=True)
    return target


def write_rust_project(target: Path) -> Path:
    target.mkdir(parents=True, exist_ok=True)
    _write(target / "Cargo.toml", '[package]\nname = "demo"\nedition = "2024"\n')
    _write(target / "src" / "lib.rs", "pub fn ping() {}\n")
    _write(target / ".github" / "workflows" / "ci.yml", "name: ci\non: push\njobs:\n  test:\n    runs-on: ubuntu-latest\n")
    return target


def write_node_project(target: Path) -> Path:
    target.mkdir(parents=True, exist_ok=True)
    _write(target / "package.json", '{"name":"demo","scripts":{"test":"node test.js"}}\n')
    _write(target / "test.js", "// node smoke\n")
    return target


def write_python_project(target: Path) -> Path:
    target.mkdir(parents=True, exist_ok=True)
    _write(target / "pyproject.toml", "[project]\nname = 'demo'\nversion = '0.0.1'\n")
    _write(target / "tests" / "test_smoke.py", "def test_ok(): assert True\n")
    return target


def write_unknown_project(target: Path) -> Path:
    target.mkdir(parents=True, exist_ok=True)
    _write(target / "README.md", "no build system here\n")
    return target


def write_ambiguous_root(target: Path) -> Path:
    """Two competing AGENTS.md scopes with no Git anchor."""
    target.mkdir(parents=True, exist_ok=True)
    inner = target / "nested"
    _write(target / "AGENTS.md", "# top-level\n")
    _write(inner / "AGENTS.md", "# nested scope\n")
    _write(inner / "package.json", '{"name":"nested"}\n')
    return target
