#!/usr/bin/env python3
"""
Install and configure Nowledge Mem for Claude Code inside a Ralph project.

What this script does:
1. Locates the target project directory.
2. Verifies that `claude` is available.
3. Installs `nmem-cli` with `uv tool install` when `nmem` is missing.
4. Adds the Nowledge community plugin marketplace.
5. Installs `nowledge-mem` with Claude Code project scope.
6. Verifies that the project configuration contains the plugin.

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

MARKETPLACE_URL = "https://github.com/nowledge-co/community"
PLUGIN_NAME = "nowledge-mem@nowledge-community"


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


def configure_claude_plugin(project_root: Path, dry_run: bool) -> None:
    claude = require_command("claude")
    log(f"Found Claude Code: {claude}")

    # Marketplace add may return a non-zero status when the marketplace already exists.
    # Continue to the authoritative plugin-install step and fail only if that step fails.
    result = run(
        [claude, "plugin", "marketplace", "add", MARKETPLACE_URL],
        cwd=project_root,
        dry_run=dry_run,
        check=False,
        capture=True,
    )

    if not dry_run and result.returncode != 0:
        message = "\n".join(
            part.strip() for part in (result.stdout, result.stderr) if part and part.strip()
        )
        warn(
            "Marketplace add did not return success. This is often harmless when it "
            "was already added."
        )
        if message:
            warn(message)

    run(
        [claude, "plugin", "install", PLUGIN_NAME, "--scope", "project"],
        cwd=project_root,
        dry_run=dry_run,
    )


def contains_plugin(value: object) -> bool:
    if isinstance(value, str):
        return "nowledge-mem" in value or "nowledge-community" in value
    if isinstance(value, dict):
        return any(contains_plugin(key) or contains_plugin(item) for key, item in value.items())
    if isinstance(value, list):
        return any(contains_plugin(item) for item in value)
    return False


def verify_project_configuration(project_root: Path, dry_run: bool) -> None:
    settings_path = project_root / ".claude" / "settings.json"

    if dry_run:
        log(f"Would verify project settings: {settings_path}")
        return

    if not settings_path.exists():
        raise SetupError(
            f"Claude project settings were not created: {settings_path}. "
            "The plugin may have been installed to the wrong scope."
        )

    try:
        settings = json.loads(settings_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise SetupError(f"Could not read {settings_path}: {exc}") from exc

    if not contains_plugin(settings):
        raise SetupError(
            f"{settings_path} exists, but no Nowledge Mem plugin entry was found."
        )

    log(f"Verified project-scoped plugin configuration: {settings_path}")

    if shutil.which("nmem"):
        status = run(
            ["nmem", "status"],
            cwd=project_root,
            dry_run=False,
            check=False,
            capture=True,
        )
        if status.returncode == 0:
            output = (status.stdout or "").strip()
            if output:
                print(output)
            log("nmem status check succeeded.")
        else:
            warn(
                "The plugin is installed, but `nmem status` did not succeed. "
                "Open or sign in to Nowledge Mem, then run `nmem status` again."
            )
            combined = "\n".join(
                part.strip()
                for part in (status.stdout, status.stderr)
                if part and part.strip()
            )
            if combined:
                warn(combined)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Install Nowledge Mem for Claude Code using project scope, "
            "suitable for Ralph headless runs."
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
        help="Print commands without changing the system or project.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()

    try:
        project_root = find_project_root(Path(args.project))
        log(f"Project root: {project_root}")

        ensure_nmem(project_root, args.dry_run)
        configure_claude_plugin(project_root, args.dry_run)
        verify_project_configuration(project_root, args.dry_run)

        print()
        print("[OK] Nowledge Mem is configured for this Ralph project.")
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
