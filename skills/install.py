#!/usr/bin/env python3
"""Install operator skills into local or global skills directories.

Default source: this repository's ``skills/`` directory (the parent of
this script).

Default (local): install into both ``./.claude/skills`` and
``./.agents/skills``. Use ``--global`` for ``~/.claude/skills`` and
``~/.agents/skills``, or ``--dir <path>`` for a single custom directory.

By default, only the five public operator skills listed in
``skills/README.md`` are exposed as installable units. The shared
``ralph-preset-common`` fixtures/references directory is bundled as a
regular copy when one of the preset skills requests it.

Examples
--------
    # Local: ./.claude/skills + ./.agents/skills
    ./skills/install.py

    # Install a subset (still both local targets)
    ./skills/install.py ralph-loop ralph-hats

    # Global: ~/.claude/skills + ~/.agents/skills
    ./skills/install.py --global

    # Dry-run an install into a custom directory
    ./skills/install.py --dir /tmp/skills --dry-run

    # Remove skills that are no longer requested
    ./skills/install.py --prune
"""

from __future__ import annotations

import argparse
import os
import shutil
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

PUBLIC_SKILLS: tuple[str, ...] = (
    "ralph-hats",
    "ralph-loop",
    "ralph-preset-author",
    "ralph-preset-review",
    "ralph-project-bootstrap",
    "ralph-run-diagnosis",
)

# The catalog above is the single source of truth for which skills are
# public. ``discover_skills`` filters filesystem candidates by name so
# stray ``SKILL.md`` directories cannot leak into install/listing.
CATALOG_NAMES: frozenset[str] = frozenset(PUBLIC_SKILLS)

SHARED_COMMON = "ralph-preset-common"
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
    carries_common_refs: bool


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
        if entry.name == SHARED_COMMON:
            continue
        if entry.name not in CATALOG_NAMES:
            continue
        if not (entry / "SKILL.md").is_file():
            continue
        found[entry.name] = SkillSpec(
            name=entry.name,
            src=entry,
            carries_common_refs=(entry / "references").is_symlink()
            or (entry / "references").is_dir(),
        )
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


def _resolve_common_refs_source(skill_src: Path) -> Path | None:
    """Return the directory backing ``references`` if it is a symlink.

    Resolves the symlink target relative to the directory containing the
    symlink itself (not its parent), matching the `..` semantics used in
    ``skills/README.md``.
    """
    refs = skill_src / "references"
    if not refs.is_symlink():
        return None
    raw = os.readlink(refs)
    target = Path(raw)
    if target.is_absolute():
        return target
    # `os.readlink` returns the link string verbatim; resolve it against
    # the directory the symlink lives in so `../ralph-preset-common/...`
    # resolves to the shared directory rather than nesting twice.
    return (refs.parent / target).resolve()


def copy_skill(spec: SkillSpec, target: Path, *, force: bool) -> None:
    dest = target / spec.name
    if dest.is_symlink() or dest.exists():
        if not force:
            answer = input(
                f"  '{spec.name}' already exists at {dest}. Overwrite? [y/N] "
            ).strip().lower()
            if answer not in {"y", "yes"}:
                print(f"  skip {spec.name} (kept existing copy)")
                return
        if dest.is_symlink() or dest.is_file():
            dest.unlink()
        else:
            shutil.rmtree(dest)
    # Skip symlinks at the source: ``references`` at the skill root
    # points to ``../ralph-preset-common/references`` relative to the
    # source skill directory (which does not exist in the destination),
    # and the shared ``ralph-preset-common/references/`` directory
    # itself contains a stale ``references`` symlink. Both are
    # reconstructed as real directories below.
    def _skip_symlinks(directory: str, contents: list[str]) -> list[str]:
        return [name for name in contents if (Path(directory) / name).is_symlink()]

    shutil.copytree(spec.src, dest, symlinks=False, ignore=_skip_symlinks)
    common_refs = _resolve_common_refs_source(spec.src)
    if common_refs is not None and common_refs.is_dir():
        common_dest = dest / "references"
        if common_dest.exists() or common_dest.is_symlink():
            if common_dest.is_symlink() or common_dest.is_file():
                common_dest.unlink()
            elif common_dest.is_dir():
                shutil.rmtree(common_dest)
        shutil.copytree(common_refs, common_dest, ignore=_skip_symlinks)


def run_install(
    targets: list[tuple[str, Path]],
    requested: list[SkillSpec],
    *,
    dry_run: bool,
    prune: bool,
    force: bool,
) -> None:
    for label, target in targets:
        target.mkdir(parents=True, exist_ok=True)
        to_install, _, to_prune = plan_install(target, requested)
        print(f"[{label}] Target: {target}")
        if dry_run:
            for name in sorted(to_install):
                spec = next(s for s in requested if s.name == name)
                print(f"  would install {spec.name} from {spec.src}")
            if prune and to_prune:
                for name in sorted(to_prune):
                    print(f"  would prune {name}")
            continue
        for spec in requested:
            print(f"  install {spec.name} <- {spec.src}")
            copy_skill(spec, target, force=force)
        if prune and to_prune:
            for name in sorted(to_prune):
                victim = target / name
                if victim.exists():
                    print(f"  prune {name}")
                    shutil.rmtree(victim)


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