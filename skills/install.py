#!/usr/bin/env python3
"""Install operator skills into local or global skills directories.

Default source: this repository's ``skills/`` directory (the parent of
this script).

Default (local): install into both ``./.claude/skills`` and
``./.agents/skills``. Use ``--global`` for ``~/.claude/skills`` and
``~/.agents/skills``, or ``--dir <path>`` for a single custom directory.

By default, only the public operator skills listed in
``skills/README.md`` are exposed as installable units. Each preset-author
and preset-review skill carries its own ``references/`` directory as
plain files, so there is no shared ``ralph-preset-common`` directory to
materialise at install time (plan 2026-08-02-001).

Examples
--------
    # Local: ./.claude/skills + ./.agents/skills
    ./skills/install.py

    # Install a subset (still both local targets)
    ./skills/install.py ralph-preset-author ralph-run-diagnosis

    # Global: ~/.claude/skills + ~/.agents/skills
    ./skills/install.py --global

    # Dry-run an install into a custom directory
    ./skills/install.py --dir /tmp/skills --dry-run

    # Remove skills that are no longer requested
    ./skills/install.py --prune
"""

from __future__ import annotations

import argparse
import shutil
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

PUBLIC_SKILLS: tuple[str, ...] = (
    "ralph-e2e-bootstrap",
    "ralph-preset-author",
    "ralph-preset-review",
    "ralph-project-bootstrap",
    "ralph-run-diagnosis",
)

# The catalog above is the single source of truth for which skills are
# public. ``discover_skills`` filters filesystem candidates by name so
# stray ``SKILL.md`` directories cannot leak into install/listing.
CATALOG_NAMES: frozenset[str] = frozenset(PUBLIC_SKILLS)

# Plan 2026-08-02-001: ``ralph-loop`` and the shared
# ``ralph-preset-common`` directory are removed; each preset-author /
# preset-review skill now ships its own references/ directory.
REMOVED_LEGACY_SKILLS: frozenset[str] = frozenset({"ralph-loop"})

SCRIPT_DIR = Path(__file__).resolve().parent
SOURCE_ROOT = SCRIPT_DIR
TARGET_LOCAL = Path(".claude/skills")
TARGET_GLOBAL = Path.home() / ".claude/skills"
TARGET_AGENTS_LOCAL = Path(".agents/skills")
TARGET_AGENTS_GLOBAL = Path.home() / ".agents/skills"


@dataclass(frozen=True)
class SkillSpec:
    name: str
    src: Path


class InstallError(RuntimeError):
    """Raised for user-visible install failures."""


def discover_skills(source: Path) -> dict[str, SkillSpec]:
    """Return the catalog members that exist under ``source``.

    Discovery is intentionally catalog-driven: ``CATALOG_NAMES`` is the
    single source of truth, so any stray ``SKILL.md`` directory outside
    the catalog is ignored.
    """
    if not source.is_dir():
        raise InstallError(f"source directory not found: {source}")
    found: dict[str, SkillSpec] = {}
    for entry in sorted(source.iterdir()):
        if not entry.is_dir():
            continue
        if entry.name in REMOVED_LEGACY_SKILLS:
            # Plan 2026-08-02-001: ``ralph-loop`` was retired; refuse to
            # re-introduce it even if a stray directory reappears.
            continue
        if entry.name not in CATALOG_NAMES:
            continue
        if not (entry / "SKILL.md").is_file():
            continue
        found[entry.name] = SkillSpec(name=entry.name, src=entry)
    return found


def resolve_targets(args: argparse.Namespace) -> list[tuple[str, Path]]:
    """Return ``[(label, path)]`` for every install target.

    - default (local): ``./.claude/skills`` + ``./.agents/skills``
    - ``--global``: ``~/.claude/skills`` + ``~/.agents/skills``
    - ``--dir``: a single custom directory (no paired agents target)
    """
    if args.global_install and args.target_dir is not None:
        raise InstallError("--global and --dir are mutually exclusive")
    if args.target_dir is not None:
        return [("custom", Path(args.target_dir).expanduser().resolve())]
    if args.global_install:
        return [
            ("claude-global", TARGET_GLOBAL.expanduser().resolve()),
            ("agents-global", TARGET_AGENTS_GLOBAL.expanduser().resolve()),
        ]
    return [
        ("claude-local", (Path.cwd() / TARGET_LOCAL).resolve()),
        ("agents-local", (Path.cwd() / TARGET_AGENTS_LOCAL).resolve()),
    ]


def select_skills(
    available: dict[str, SkillSpec],
    requested: Iterable[str],
) -> list[SkillSpec]:
    selected: list[SkillSpec] = []
    seen: set[str] = set()
    for name in requested:
        if name not in available:
            known = ", ".join(sorted(available))
            raise InstallError(
                f"unknown skill '{name}'. Known public skills: {known}"
            )
        if name in seen:
            continue
        seen.add(name)
        selected.append(available[name])
    if not selected:
        # Default: install all public skills.
        selected = list(available.values())
    selected.sort(key=lambda spec: spec.name)
    return selected


def plan_install(
    target: Path, requested: list[SkillSpec]
) -> tuple[set[str], set[str], set[str]]:
    """Return (to_install, to_keep, to_prune) relative to ``target``."""
    if not target.exists():
        return {s.name for s in requested}, set(), set()
    existing = {
        entry.name
        for entry in target.iterdir()
        if entry.is_dir() and (entry / "SKILL.md").is_file()
    }
    requested_names = {s.name for s in requested}
    to_install = requested_names & existing
    to_prune = existing - requested_names
    to_keep = requested_names & existing
    return requested_names, to_keep, to_prune


def copy_skill(spec: SkillSpec, target: Path, *, force: bool) -> str:
    """Copy one skill and return ``installed``, ``replaced``, or ``skipped``.

    Installation is always a physical directory copy. Source symlinks are
    skipped or materialised as regular directories; destination symlinks and
    hard-link based installs are never created.
    """
    dest = target / spec.name
    existed = dest.is_symlink() or dest.exists()
    if existed:
        if not force:
            answer = input(
                f"  '{spec.name}' already exists at {dest}. Overwrite? [y/N] "
            ).strip().lower()
            if answer not in {"y", "yes"}:
                return "skipped"
        if dest.is_symlink() or dest.is_file():
            dest.unlink()
        else:
            shutil.rmtree(dest)

    # Plan 2026-08-02-001: each skill's references/ directory is a real
    # directory of plain files (no symlinks); we copy it verbatim. Sources
    # still containing any stray symlink are dropped to satisfy the
    # "no destination symlinks" guarantee.
    def _skip_symlinks(directory: str, contents: list[str]) -> list[str]:
        return [name for name in contents if (Path(directory) / name).is_symlink()]

    shutil.copytree(spec.src, dest, symlinks=False, ignore=_skip_symlinks)
    linked_paths = [path for path in dest.rglob("*") if path.is_symlink()]
    if linked_paths:
        rendered = ", ".join(str(path) for path in linked_paths)
        raise InstallError(
            f"copied skill contains forbidden destination symlink(s): {rendered}"
        )
    return "replaced" if existed else "installed"


def run_install(
    targets: list[tuple[str, Path]],
    requested: list[SkillSpec],
    *,
    dry_run: bool,
    prune: bool,
    force: bool,
) -> None:
    mode = (
        "global"
        if any(label.endswith("-global") for label, _ in targets)
        else "custom" if len(targets) == 1 and targets[0][0] == "custom"
        else "local"
    )
    print(f"Install mode: {mode}")
    print("Install method: physical copy (replace destination; no symlinks or hardlinks)")
    print(f"Selected skills ({len(requested)}):")
    for spec in requested:
        print(f"  - {spec.name}")
    print(f"Install targets ({len(targets)}):")
    for label, target in targets:
        print(f"  - {label}: {target}")

    for label, target in targets:
        target.mkdir(parents=True, exist_ok=True)
        to_install, _, to_prune = plan_install(target, requested)
        print(f"\n[{label}] Installing into: {target}")
        if dry_run:
            for name in sorted(to_install):
                spec = next(s for s in requested if s.name == name)
                destination = target / spec.name
                if destination.is_symlink() or destination.exists():
                    result = (
                        "would replace"
                        if force
                        else "would prompt before replacing"
                    )
                else:
                    result = "would install"
                print(f"  {spec.name}")
                print(f"    source:      {spec.src}")
                print(f"    destination: {destination}")
                print(f"    result:      {result}")
            if prune and to_prune:
                for name in sorted(to_prune):
                    print(f"  {name}")
                    print(f"    destination: {target / name}")
                    print("    result:      would prune")
            continue
        results: dict[str, int] = {"installed": 0, "replaced": 0, "skipped": 0}
        for spec in requested:
            destination = target / spec.name
            result = copy_skill(spec, target, force=force)
            results[result] += 1
            print(f"  {spec.name}")
            print(f"    source:      {spec.src}")
            print(f"    destination: {destination}")
            print(f"    result:      {result}")
        if prune and to_prune:
            for name in sorted(to_prune):
                victim = target / name
                if victim.exists():
                    shutil.rmtree(victim)
                    print(f"  {name}")
                    print(f"    destination: {victim}")
                    print("    result:      pruned")
        summary = ", ".join(
            f"{count} {name}" for name, count in results.items() if count
        )
        print(f"  Summary: {summary or 'no skill changes'}")


def run_list(targets: list[tuple[str, Path]]) -> None:
    available = discover_skills(SOURCE_ROOT)
    print("Available public skills:")
    for spec in available.values():
        print(f"  - {spec.name}")
    for label, target in targets:
        if target.exists():
            existing = sorted(
                entry.name
                for entry in target.iterdir()
                if entry.is_dir() and (entry / "SKILL.md").is_file()
            )
            if existing:
                print(f"\n[{label}] Installed in {target}:")
                for name in existing:
                    marker = " (managed)" if name in available else " (unknown)"
                    print(f"  - {name}{marker}")
            else:
                print(f"\n[{label}] No skills currently installed in {target}.")
        else:
            print(f"\n[{label}] {target}: (directory not present)")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="skills/install.py",
        description=(
            "Install public Ralph operator skills. "
            "Local default writes .claude/skills + .agents/skills; "
            "--global writes ~/.claude/skills + ~/.agents/skills."
        ),
    )
    parser.add_argument(
        "skills",
        nargs="*",
        help=(
            "Subset of public skills to install. "
            f"Available: {', '.join(PUBLIC_SKILLS)}. "
            "Defaults to all public skills."
        ),
    )
    target = parser.add_mutually_exclusive_group()
    target.add_argument(
        "--global",
        dest="global_install",
        action="store_true",
        help=(
            f"Install into {TARGET_GLOBAL} and {TARGET_AGENTS_GLOBAL} "
            "(instead of the local .claude + .agents pair)."
        ),
    )
    target.add_argument(
        "--dir",
        dest="target_dir",
        help=(
            "Install into a single custom directory "
            "(absolute path recommended; skips the paired agents target)."
        ),
    )
    parser.add_argument(
        "--prune",
        action="store_true",
        help=(
            "Remove existing skills in each target that are not part of "
            "this install request."
        ),
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would change without writing to disk.",
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="List available skills and any existing installation.",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="Overwrite existing skill copies without prompting.",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        targets = resolve_targets(args)
        if args.list:
            run_list(targets)
            return 0
        available = discover_skills(SOURCE_ROOT)
        requested = select_skills(available, args.skills)
        run_install(
            targets,
            requested,
            dry_run=args.dry_run,
            prune=args.prune,
            force=args.force,
        )
    except InstallError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
