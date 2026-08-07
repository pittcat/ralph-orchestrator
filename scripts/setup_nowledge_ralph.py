#!/usr/bin/env python3
"""
Migrate a Ralph target project from the generic Nowledge Mem Claude Code
plugin to the Ralph-dedicated read-only plugin.

What this script does (all Claude calls run with the target project as cwd):
1. Locates the target project directory.
2. Verifies that `claude` is available.
3. Installs `nmem-cli` with `uv tool install` when `nmem` is missing.
4. Reads the authoritative plugin inventory (`claude plugin list --json`).
5. Installs `nowledge-mem-ralph@ralph-orchestrator` with project scope
   from this repository's local marketplace (when missing for the target)
   and re-verifies it authoritatively.
6. Only then removes the target project's generic
   `nowledge-mem@nowledge-community` project-scope entry with
   `--keep-data` (when present for the target).
7. Final verification: dedicated project entry present, generic project
   entry gone, and every entry outside that migration deep-equal to the
   initial inventory (user scope and other projects untouched).

Migration matching is exact: full plugin id + scope=project + canonical
projectPath equal to the target root. Entries of other projects are never
migrated. User-scope entries are never created or removed.

It intentionally does NOT modify:
- CLAUDE.md
- AGENTS.md
- Ralph hats/prompts
- ralph.yml

Usage:
    python3 setup_nowledge_ralph.py
    python3 setup_nowledge_ralph.py /path/to/project
    python3 setup_nowledge_ralph.py --dry-run
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Sequence

REPO_ROOT = Path(__file__).resolve().parent.parent
MARKETPLACE_NAME = "ralph-orchestrator"
DEDICATED_ID = f"nowledge-mem-ralph@{MARKETPLACE_NAME}"
GENERIC_ID = "nowledge-mem@nowledge-community"
MIGRATED_IDS = (DEDICATED_ID, GENERIC_ID)


class SetupError(RuntimeError):
    """Raised when setup cannot continue safely."""


def log(message: str) -> None:
    print(f"[INFO] {message}", flush=True)


def warn(message: str) -> None:
    print(f"[WARN] {message}", file=sys.stderr, flush=True)


def fail(message: str, exit_code: int = 1) -> "None":
    print(f"[ERROR] {message}", file=sys.stderr, flush=True)
    raise SystemExit(exit_code)


def format_command(command: Sequence[str]) -> str:
    def quote(value: str) -> str:
        if not value:
            return "''"
        if all(ch.isalnum() or ch in "._-/=:@" for ch in value):
            return value
        return "'" + value.replace("'", "'\"'\"'") + "'"

    return " ".join(quote(part) for part in command)


def run(
    command: Sequence[str],
    *,
    cwd: Path,
    dry_run: bool,
    check: bool = True,
    capture: bool = False,
) -> subprocess.CompletedProcess[str]:
    log(f"$ {format_command(command)}")

    if dry_run:
        return subprocess.CompletedProcess(command, 0, "", "")

    result = subprocess.run(
        list(command),
        cwd=str(cwd),
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
        check=False,
    )

    if check and result.returncode != 0:
        detail = ""
        if capture:
            combined = "\n".join(
                part.strip() for part in (result.stdout, result.stderr) if part and part.strip()
            )
            if combined:
                detail = f"\n{combined}"
        raise SetupError(
            f"Command failed with exit code {result.returncode}: "
            f"{format_command(command)}{detail}"
        )

    return result


def find_project_root(start: Path) -> Path:
    current = start.expanduser().resolve()

    if not current.exists():
        raise SetupError(f"Project path does not exist: {current}")
    if not current.is_dir():
        raise SetupError(f"Project path is not a directory: {current}")

    # Prefer the nearest Git root. If this is not a Git project, use the supplied directory.
    probe = current
    while True:
        if (probe / ".git").exists():
            return probe
        if probe.parent == probe:
            return current
        probe = probe.parent


def require_command(name: str) -> str:
    path = shutil.which(name)
    if not path:
        raise SetupError(f"Required command was not found in PATH: {name}")
    return path


def refresh_path_for_uv_tools() -> None:
    """
    Add common uv tool binary directories to this process PATH.

    `uv tool install` commonly places executables in ~/.local/bin.
    """
    candidates = [
        Path.home() / ".local" / "bin",
        Path.home() / ".cargo" / "bin",
    ]

    current_parts = os.environ.get("PATH", "").split(os.pathsep)
    changed = False

    for candidate in candidates:
        candidate_str = str(candidate)
        if candidate.exists() and candidate_str not in current_parts:
            current_parts.insert(0, candidate_str)
            changed = True

    if changed:
        os.environ["PATH"] = os.pathsep.join(current_parts)


def ensure_nmem(project_root: Path, dry_run: bool) -> None:
    refresh_path_for_uv_tools()

    existing = shutil.which("nmem")
    if existing:
        log(f"Found nmem: {existing}")
        run(["nmem", "--version"], cwd=project_root, dry_run=dry_run, check=False)
        return

    uv = shutil.which("uv")
    if not uv:
        raise SetupError(
            "`nmem` is not installed and `uv` was not found. "
            "Install uv first, then run this script again."
        )

    log("nmem was not found; installing nmem-cli with uv.")
    run([uv, "tool", "install", "nmem-cli"], cwd=project_root, dry_run=dry_run)

    if dry_run:
        return

    refresh_path_for_uv_tools()
    installed = shutil.which("nmem")
    if not installed:
        raise SetupError(
            "nmem-cli was installed, but `nmem` is still not visible in PATH. "
            "Add ~/.local/bin to PATH and run this script again."
        )

    log(f"Installed nmem: {installed}")
    run(["nmem", "--version"], cwd=project_root, dry_run=False, check=False)


# --- scope-aware plugin migration ---------------------------------------------


def _combined_output(result: subprocess.CompletedProcess[str]) -> str:
    return "\n".join(
        part.strip() for part in (result.stdout, result.stderr) if part and part.strip()
    )


def read_plugin_inventory(claude: str, project_root: Path) -> list[dict]:
    """Read the authoritative plugin list; fail closed on unknown shapes."""
    result = run(
        [claude, "plugin", "list", "--json"],
        cwd=project_root,
        dry_run=False,
        check=False,
        capture=True,
    )
    if result.returncode != 0:
        raise SetupError(
            f"`claude plugin list --json` failed with exit code {result.returncode}; "
            f"cannot determine the current plugin state safely.\n{_combined_output(result)}"
        )

    try:
        inventory = json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        raise SetupError(
            f"could not parse `claude plugin list --json` output as JSON: {exc}"
        ) from exc

    if not isinstance(inventory, list):
        raise SetupError(
            "`claude plugin list --json` did not return a JSON array; "
            "refusing to guess the plugin state."
        )

    for entry in inventory:
        if (
            not isinstance(entry, dict)
            or not isinstance(entry.get("id"), str)
            or not isinstance(entry.get("scope"), str)
        ):
            raise SetupError(
                "unrecognized plugin list entry shape (needs string `id` and "
                "`scope`); refusing to guess the plugin state."
            )
        if entry["scope"] == "project" and not isinstance(entry.get("projectPath"), str):
            raise SetupError(
                f"project-scope entry {entry.get('id')!r} has no string "
                "`projectPath`; refusing to guess which project it belongs to."
            )

    return inventory


def is_target_project(entry: dict, target_root: Path) -> bool:
    """Exact match: scope=project and canonical projectPath == target root."""
    if entry.get("scope") != "project":
        return False
    entry_path = Path(entry["projectPath"]).expanduser().resolve()
    return entry_path == target_root


def has_entry(inventory: list[dict], plugin_id: str, target_root: Path) -> bool:
    return any(
        entry.get("id") == plugin_id and is_target_project(entry, target_root)
        for entry in inventory
    )


def plan_migration(inventory: list[dict], target_root: Path) -> dict[str, bool]:
    return {
        "install_dedicated": not has_entry(inventory, DEDICATED_ID, target_root),
        "uninstall_generic": has_entry(inventory, GENERIC_ID, target_root),
    }


def _strip_target_migration(inventory: list[dict], target_root: Path) -> list[str]:
    """Canonical form of every entry outside the target dedicated/generic
    migration — used to prove user scope and other projects are untouched."""
    return sorted(
        json.dumps(entry, sort_keys=True)
        for entry in inventory
        if not (
            entry.get("id") in MIGRATED_IDS and is_target_project(entry, target_root)
        )
    )


def migrate_project_plugins(project_root: Path, dry_run: bool) -> None:
    claude = require_command("claude")
    log(f"Found Claude Code: {claude}")

    inventory = read_plugin_inventory(claude, project_root)
    plan = plan_migration(inventory, project_root)
    log(
        "Initial target-project state: "
        f"dedicated={'present' if not plan['install_dedicated'] else 'missing'}, "
        f"generic={'present' if plan['uninstall_generic'] else 'absent'}."
    )

    if dry_run:
        if plan["install_dedicated"]:
            log(
                "[DRY-RUN] would run: "
                + format_command(
                    [claude, "plugin", "marketplace", "add", "--scope", "project", str(REPO_ROOT)]
                )
            )
            log(
                "[DRY-RUN] would run: "
                + format_command(
                    [claude, "plugin", "install", DEDICATED_ID, "--scope", "project"]
                )
            )
        else:
            log("[DRY-RUN] dedicated project plugin already installed; no install needed.")
        if plan["uninstall_generic"]:
            log(
                "[DRY-RUN] would run: "
                + format_command(
                    [
                        claude,
                        "plugin",
                        "uninstall",
                        GENERIC_ID,
                        "--scope",
                        "project",
                        "--keep-data",
                    ]
                )
            )
        else:
            log("[DRY-RUN] no target project generic to uninstall.")
        return

    if plan["install_dedicated"]:
        # A non-zero marketplace add is recoverable (e.g. already declared);
        # the install and the authoritative re-list decide success (D11).
        add_result = run(
            [claude, "plugin", "marketplace", "add", "--scope", "project", str(REPO_ROOT)],
            cwd=project_root,
            dry_run=False,
            check=False,
            capture=True,
        )
        if add_result.returncode != 0:
            warn(
                "marketplace add did not return success; continuing — "
                "install and final verification are authoritative."
            )
            output = _combined_output(add_result)
            if output:
                warn(output)

        # check=True: on failure we stop BEFORE touching the generic plugin.
        run(
            [claude, "plugin", "install", DEDICATED_ID, "--scope", "project"],
            cwd=project_root,
            dry_run=False,
        )

        verified = read_plugin_inventory(claude, project_root)
        if not has_entry(verified, DEDICATED_ID, project_root):
            raise SetupError(
                "dedicated plugin install reported success but the authoritative "
                f"plugin list has no project-scope {DEDICATED_ID} entry for this "
                "target; keeping the project generic plugin."
            )
        log(f"Verified dedicated project plugin: {DEDICATED_ID}")
    else:
        log(f"Dedicated project plugin already installed: {DEDICATED_ID}")

    if plan["uninstall_generic"]:
        uninstall_result = run(
            [
                claude,
                "plugin",
                "uninstall",
                GENERIC_ID,
                "--scope",
                "project",
                "--keep-data",
            ],
            cwd=project_root,
            dry_run=False,
            check=False,
            capture=True,
        )
        if uninstall_result.returncode != 0:
            output = _combined_output(uninstall_result)
            raise SetupError(
                f"dedicated plugin {DEDICATED_ID} is installed and verified, but "
                f"removing the project-scope generic {GENERIC_ID} failed with exit "
                f"code {uninstall_result.returncode}. Both project plugins now "
                "coexist; this is recoverable.\n"
                f"Recovery: re-run this script, or manually run:\n"
                f"  cd '{project_root}' && claude plugin uninstall {GENERIC_ID} "
                "--scope project --keep-data"
                + (f"\n{output}" if output else "")
            )
        log(f"Removed project-scope generic plugin (data kept): {GENERIC_ID}")
    else:
        log("No project-scope generic plugin for this target; nothing to uninstall.")

    final_inventory = read_plugin_inventory(claude, project_root)
    if not has_entry(final_inventory, DEDICATED_ID, project_root):
        raise SetupError(
            "final verification failed: dedicated project entry "
            f"{DEDICATED_ID} is missing from the authoritative plugin list."
        )
    if has_entry(final_inventory, GENERIC_ID, project_root):
        raise SetupError(
            "final verification failed: project-scope generic "
            f"{GENERIC_ID} is still present for this target."
        )
    if _strip_target_migration(final_inventory, project_root) != _strip_target_migration(
        inventory, project_root
    ):
        raise SetupError(
            "final verification failed: entries outside the target migration "
            "(user scope or other projects) changed unexpectedly."
        )
    log("Final verification passed: dedicated present, generic absent, all other entries unchanged.")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Migrate a Ralph target project from the generic Nowledge Mem "
            "plugin to the dedicated read-only nowledge-mem-ralph plugin "
            "(project scope only; user scope is never touched)."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  python3 setup_nowledge_ralph.py\n"
            "  python3 setup_nowledge_ralph.py ~/Dev/Rust/ralph-orchestrator\n"
            "  python3 setup_nowledge_ralph.py --dry-run\n"
        ),
    )
    parser.add_argument(
        "project",
        nargs="?",
        default=".",
        help="Project directory. Defaults to the current directory.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Read the current plugin state and print planned actions without "
        "running any mutation command.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()

    try:
        project_root = find_project_root(Path(args.project))
        log(f"Project root: {project_root}")

        ensure_nmem(project_root, args.dry_run)
        migrate_project_plugins(project_root, args.dry_run)

        print()
        print("[OK] Nowledge Mem plugin migration is configured for this Ralph project.")
        print("[OK] No CLAUDE.md, AGENTS.md, Hat prompt, or ralph.yml changes were made.")
        print()
        print("Run Ralph normally, for example:")
        print(
            "ralph run -c ralph.yml -H builtin:ce-executor-pipeline "
            "--plan docs/plans/your-plan.md --worktree --reuse-worktree"
        )
        return 0

    except SetupError as exc:
        fail(str(exc))
    except KeyboardInterrupt:
        fail("Interrupted by user.", 130)

    return 1


if __name__ == "__main__":
    raise SystemExit(main())
