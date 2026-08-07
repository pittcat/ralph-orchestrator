"""Integration tests for ``scripts/setup_nowledge_ralph.py`` (U2).

The installer is exercised as a real subprocess against a **fake
``claude`` executable** whose responses come from a per-call queue:
every ``plugin list --json`` call consumes the next snapshot, so a
stale fixed snapshot can never mask a verification bug. The fake logs
every argv plus its cwd, letting tests assert the exact mutation
sequence, scope flags and working directory.

Coverage (plan 2026-08-07-010, S7–S16):

* first install migrates project generic → dedicated, user untouched
* idempotent re-run performs no mutations
* dry-run reads state but issues zero mutation calls
* invalid inventory JSON / missing ``scope`` fail closed
* dedicated install or post-install verification failure keeps generic
* generic uninstall failure reports the coexisting partial state
* absent user generic is never created
* marketplace-add non-zero is a recoverable warning (plus the
  warning-then-install-failure combination)
* entries of other projects are never migrated
* final verifier catches missing dedicated, surviving generic and
  out-of-band user/other-project changes
* paths containing spaces survive argv (non-shell) dispatch
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "setup_nowledge_ralph.py"
REPO_ROOT = str(ROOT)

DEDICATED = "nowledge-mem-ralph@ralph-orchestrator"
GENERIC = "nowledge-mem@nowledge-community"

LIST = ("plugin", "list", "--json")
MARKETPLACE_ADD = ("plugin", "marketplace", "add", "--scope", "project", REPO_ROOT)
INSTALL_DEDICATED = ("plugin", "install", DEDICATED, "--scope", "project")
UNINSTALL_GENERIC = (
    "plugin",
    "uninstall",
    GENERIC,
    "--scope",
    "project",
    "--keep-data",
)

FAKE_CLAUDE_SOURCE = '''#!/usr/bin/env python3
import json, os, sys

plan_path = os.environ.get("FAKE_CLAUDE_PLAN")
if not plan_path:
    print("FAKE_CLAUDE_PLAN is not set", file=sys.stderr)
    sys.exit(98)
with open(plan_path, "r", encoding="utf-8") as fh:
    plan = json.load(fh)
with open(plan["log"], "a", encoding="utf-8") as fh:
    fh.write(json.dumps({"argv": sys.argv[1:], "cwd": os.getcwd()}) + "\\n")
with open(plan["log"], "r", encoding="utf-8") as fh:
    index = sum(1 for _ in fh) - 1
responses = plan.get("responses", [])
if index >= len(responses):
    print("FAKE_CLAUDE_QUEUE_EXHAUSTED", file=sys.stderr)
    sys.exit(97)
response = responses[index]
if response.get("stdout"):
    sys.stdout.write(response["stdout"])
if response.get("stderr"):
    sys.stderr.write(response["stderr"])
sys.exit(int(response.get("exit_code", 0)))
'''

FAKE_NMEM_SOURCE = "#!/bin/sh\necho 'nmem fake 0.0.0'\nexit 0\n"


# --- fixtures -----------------------------------------------------------------


@pytest.fixture
def fake_bin(tmp_path: Path) -> Path:
    """A bin dir containing fake ``claude`` (queue-driven) and ``nmem``."""
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    claude = bin_dir / "claude"
    claude.write_text(FAKE_CLAUDE_SOURCE, encoding="utf-8")
    claude.chmod(0o755)
    nmem = bin_dir / "nmem"
    nmem.write_text(FAKE_NMEM_SOURCE, encoding="utf-8")
    nmem.chmod(0o755)
    return bin_dir


@pytest.fixture
def target_project(tmp_path: Path) -> Path:
    target = tmp_path / "target project"  # spaces exercise argv dispatch
    target.mkdir()
    return target


def entry(
    plugin_id: str,
    scope: str,
    *,
    project_path: str | None = None,
    version: str = "1.0.0",
) -> dict:
    """Build a realistic ``plugin list --json`` entry."""
    value = {
        "id": plugin_id,
        "version": version,
        "scope": scope,
        "enabled": True,
        "installPath": f"/fake/plugins/{plugin_id}/{version}",
        "installedAt": "2026-08-07T00:00:00.000Z",
        "lastUpdated": "2026-08-07T00:00:00.000Z",
    }
    if project_path is not None:
        value["projectPath"] = project_path
    return value


def run_installer(
    tmp_path: Path,
    fake_bin: Path,
    target: Path,
    responses: list[dict],
    *,
    dry_run: bool = False,
) -> tuple[subprocess.CompletedProcess[str], list[dict]]:
    """Run the real installer subprocess against the fake claude queue."""
    log_path = tmp_path / "claude-calls.jsonl"
    plan_path = tmp_path / "fake-claude-plan.json"
    plan_path.write_text(
        json.dumps({"log": str(log_path), "responses": responses}),
        encoding="utf-8",
    )

    # Isolated HOME: the installer prepends ~/.local/bin and
    # ~/.cargo/bin to PATH (uv tool locations). Pointing HOME at a
    # scratch dir keeps the fake ``claude`` authoritative and protects
    # real user-level plugin state from any accidental mutation.
    home = tmp_path / "home"
    home.mkdir(exist_ok=True)

    env = dict(os.environ)
    env["HOME"] = str(home)
    env["PATH"] = f"{fake_bin}{os.pathsep}/usr/bin{os.pathsep}/bin"
    env["FAKE_CLAUDE_PLAN"] = str(plan_path)

    argv = [sys.executable, str(SCRIPT)]
    if dry_run:
        argv.append("--dry-run")
    argv.append(str(target))

    result = subprocess.run(
        argv, capture_output=True, text=True, env=env, check=False
    )
    calls = []
    if log_path.exists():
        calls = [
            json.loads(line)
            for line in log_path.read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
    return result, calls


def argv_sequence(calls: list[dict]) -> list[tuple[str, ...]]:
    return [tuple(call["argv"]) for call in calls]


def list_response(entries: list[dict]) -> dict:
    return {"stdout": json.dumps(entries), "exit_code": 0}


def failed(stdout: str = "", stderr: str = "", exit_code: int = 1) -> dict:
    return {"stdout": stdout, "stderr": stderr, "exit_code": exit_code}


OK = {"stdout": "", "exit_code": 0}


# --- S7: first install migrates project generic --------------------------------


def test_first_install_migrates_project_generic(
    tmp_path: Path, fake_bin: Path, target_project: Path
) -> None:
    """Dedicated installed+verified before the generic is removed; the
    user-scope entry survives byte-for-byte."""
    target = str(target_project)
    user_generic = entry(GENERIC, "user", version="0.7.21")
    other_user = entry("other@some-marketplace", "user", version="2.0.0")
    project_generic = entry(GENERIC, "project", project_path=target, version="0.7.20")
    dedicated_project = entry(DEDICATED, "project", project_path=target, version="0.1.0")

    initial = [user_generic, other_user, project_generic]
    after_install = [user_generic, other_user, project_generic, dedicated_project]
    final = [user_generic, other_user, dedicated_project]

    responses = [
        list_response(initial),        # 1. initial inventory
        OK,                            # 2. marketplace add --scope project
        OK,                            # 3. dedicated install
        list_response(after_install),  # 4. post-install verification
        OK,                            # 5. generic uninstall --keep-data
        list_response(final),          # 6. final verification
    ]

    result, calls = run_installer(tmp_path, fake_bin, target_project, responses)
    assert result.returncode == 0, result.stdout + result.stderr

    assert argv_sequence(calls) == [
        LIST,
        MARKETPLACE_ADD,
        INSTALL_DEDICATED,
        LIST,
        UNINSTALL_GENERIC,
        LIST,
    ], f"unexpected claude call sequence: {argv_sequence(calls)}"

    for call in calls:
        assert Path(call["cwd"]).resolve() == target_project.resolve(), (
            f"every claude call must run in the target project cwd: {call}"
        )


# --- S8: idempotent re-run ------------------------------------------------------


def test_rerun_is_idempotent(
    tmp_path: Path, fake_bin: Path, target_project: Path
) -> None:
    target = str(target_project)
    user_generic = entry(GENERIC, "user", version="0.7.21")
    dedicated_project = entry(DEDICATED, "project", project_path=target, version="0.1.0")
    stable = [user_generic, dedicated_project]

    result, calls = run_installer(
        tmp_path, fake_bin, target_project, [list_response(stable), list_response(stable)]
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert argv_sequence(calls) == [LIST, LIST], (
        f"idempotent re-run must only read state: {argv_sequence(calls)}"
    )


def test_partial_state_converges_on_rerun(
    tmp_path: Path, fake_bin: Path, target_project: Path
) -> None:
    """Dedicated present + generic still present (e.g. after a failed
    uninstall) converges with exactly one uninstall, no reinstall."""
    target = str(target_project)
    user_generic = entry(GENERIC, "user", version="0.7.21")
    project_generic = entry(GENERIC, "project", project_path=target, version="0.7.20")
    dedicated_project = entry(DEDICATED, "project", project_path=target, version="0.1.0")

    responses = [
        list_response([user_generic, project_generic, dedicated_project]),
        OK,
        list_response([user_generic, dedicated_project]),
    ]
    result, calls = run_installer(tmp_path, fake_bin, target_project, responses)
    assert result.returncode == 0, result.stdout + result.stderr
    assert argv_sequence(calls) == [LIST, UNINSTALL_GENERIC, LIST]


# --- S9: dry-run ----------------------------------------------------------------


def test_dry_run_makes_no_mutations(
    tmp_path: Path, fake_bin: Path, target_project: Path
) -> None:
    target = str(target_project)
    user_generic = entry(GENERIC, "user", version="0.7.21")
    project_generic = entry(GENERIC, "project", project_path=target, version="0.7.20")

    result, calls = run_installer(
        tmp_path,
        fake_bin,
        target_project,
        [list_response([user_generic, project_generic])],
        dry_run=True,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert argv_sequence(calls) == [LIST], (
        f"dry-run must only read the inventory: {argv_sequence(calls)}"
    )
    for token in ("marketplace", "install", "uninstall"):
        assert token not in " ".join(" ".join(c["argv"]) for c in calls)
    assert "would run" in result.stdout.lower()


# --- S10: unparseable state fails closed ------------------------------------------


def test_invalid_json_fails_closed_without_uninstall(
    tmp_path: Path, fake_bin: Path, target_project: Path
) -> None:
    responses = [{"stdout": "{not valid json", "exit_code": 0}]
    result, calls = run_installer(tmp_path, fake_bin, target_project, responses)
    assert result.returncode != 0
    assert argv_sequence(calls) == [LIST]
    assert "parse" in result.stderr.lower(), (
        "invalid JSON must be reported as a parse failure"
    )


def test_entry_missing_scope_fails_closed(
    tmp_path: Path, fake_bin: Path, target_project: Path
) -> None:
    responses = [{"stdout": json.dumps([{"id": "something@x"}]), "exit_code": 0}]
    result, calls = run_installer(tmp_path, fake_bin, target_project, responses)
    assert result.returncode != 0
    assert argv_sequence(calls) == [LIST]


def test_list_failure_fails_closed(
    tmp_path: Path, fake_bin: Path, target_project: Path
) -> None:
    responses = [failed(stderr="boom")]
    result, calls = run_installer(tmp_path, fake_bin, target_project, responses)
    assert result.returncode != 0
    assert argv_sequence(calls) == [LIST]


# --- S11: dedicated failure keeps the generic -------------------------------------


def test_install_failure_keeps_generic(
    tmp_path: Path, fake_bin: Path, target_project: Path
) -> None:
    target = str(target_project)
    project_generic = entry(GENERIC, "project", project_path=target, version="0.7.20")

    responses = [
        list_response([project_generic]),
        OK,
        failed(stderr="install exploded"),
    ]
    result, calls = run_installer(tmp_path, fake_bin, target_project, responses)
    assert result.returncode != 0
    assert argv_sequence(calls) == [LIST, MARKETPLACE_ADD, INSTALL_DEDICATED], (
        f"install failure must stop before any uninstall: {argv_sequence(calls)}"
    )


def test_post_install_verification_failure_keeps_generic(
    tmp_path: Path, fake_bin: Path, target_project: Path
) -> None:
    """Install claims success but the authoritative re-list lacks the
    dedicated entry — stop and keep the generic."""
    target = str(target_project)
    project_generic = entry(GENERIC, "project", project_path=target, version="0.7.20")

    responses = [
        list_response([project_generic]),
        OK,
        OK,
        list_response([project_generic]),  # dedicated missing after install
    ]
    result, calls = run_installer(tmp_path, fake_bin, target_project, responses)
    assert result.returncode != 0
    assert "keeping the project generic" in result.stderr
    assert argv_sequence(calls) == [LIST, MARKETPLACE_ADD, INSTALL_DEDICATED, LIST]


# --- S12: uninstall failure reports the partial state ------------------------------


def test_uninstall_failure_reports_partial_state(
    tmp_path: Path, fake_bin: Path, target_project: Path
) -> None:
    target = str(target_project)
    user_generic = entry(GENERIC, "user", version="0.7.21")
    project_generic = entry(GENERIC, "project", project_path=target, version="0.7.20")
    dedicated_project = entry(DEDICATED, "project", project_path=target, version="0.1.0")

    responses = [
        list_response([user_generic, project_generic]),
        OK,
        OK,
        list_response([user_generic, project_generic, dedicated_project]),
        failed(stderr="uninstall exploded"),
    ]
    result, calls = run_installer(tmp_path, fake_bin, target_project, responses)
    assert result.returncode != 0
    assert argv_sequence(calls) == [
        LIST,
        MARKETPLACE_ADD,
        INSTALL_DEDICATED,
        LIST,
        UNINSTALL_GENERIC,
    ], "no final verification or cleanup may follow a failed uninstall"
    assert "coexist" in result.stderr
    assert "claude plugin uninstall" in result.stderr, (
        "the partial-state report must carry a concrete recovery command"
    )


# --- S13: absent user generic is never created --------------------------------------


def test_absent_user_generic_not_created(
    tmp_path: Path, fake_bin: Path, target_project: Path
) -> None:
    target = str(target_project)
    other_user = entry("other@some-marketplace", "user", version="2.0.0")
    project_generic = entry(GENERIC, "project", project_path=target, version="0.7.20")
    dedicated_project = entry(DEDICATED, "project", project_path=target, version="0.1.0")

    responses = [
        list_response([other_user, project_generic]),
        OK,
        OK,
        list_response([other_user, project_generic, dedicated_project]),
        OK,
        list_response([other_user, dedicated_project]),
    ]
    result, calls = run_installer(tmp_path, fake_bin, target_project, responses)
    assert result.returncode == 0, result.stdout + result.stderr
    # No mutation may ever target user scope.
    for call in calls:
        argv = call["argv"]
        if "install" in argv or "uninstall" in argv:
            assert "user" not in argv, f"user scope touched: {argv}"
            assert "project" in argv, f"mutation without explicit scope: {argv}"


# --- S15: marketplace add non-zero is recoverable ------------------------------------


def test_marketplace_add_nonzero_is_recoverable_warning(
    tmp_path: Path, fake_bin: Path, target_project: Path
) -> None:
    target = str(target_project)
    project_generic = entry(GENERIC, "project", project_path=target, version="0.7.20")
    dedicated_project = entry(DEDICATED, "project", project_path=target, version="0.1.0")

    responses = [
        list_response([project_generic]),
        failed(stdout="marketplace already declared", exit_code=1),
        OK,
        list_response([project_generic, dedicated_project]),
        OK,
        list_response([dedicated_project]),
    ]
    result, calls = run_installer(tmp_path, fake_bin, target_project, responses)
    assert result.returncode == 0, result.stdout + result.stderr
    assert "marketplace add" in result.stderr.lower(), (
        "the add failure must surface as a warning, not silently swallowed"
    )
    assert argv_sequence(calls) == [
        LIST,
        MARKETPLACE_ADD,
        INSTALL_DEDICATED,
        LIST,
        UNINSTALL_GENERIC,
        LIST,
    ]


def test_marketplace_add_warning_then_install_failure(
    tmp_path: Path, fake_bin: Path, target_project: Path
) -> None:
    target = str(target_project)
    project_generic = entry(GENERIC, "project", project_path=target, version="0.7.20")

    responses = [
        list_response([project_generic]),
        failed(stdout="marketplace already declared", exit_code=1),
        failed(stderr="install exploded"),
    ]
    result, calls = run_installer(tmp_path, fake_bin, target_project, responses)
    assert result.returncode != 0
    assert argv_sequence(calls) == [LIST, MARKETPLACE_ADD, INSTALL_DEDICATED]


# --- S16: other projects are never migrated -------------------------------------------


def test_other_project_entries_not_migrated(
    tmp_path: Path, fake_bin: Path, target_project: Path
) -> None:
    target = str(target_project)
    other_path = "/elsewhere/another project"
    user_generic = entry(GENERIC, "user", version="0.7.21")
    generic_other_project = entry(GENERIC, "project", project_path=other_path, version="0.7.19")
    dedicated_other_project = entry(DEDICATED, "project", project_path=other_path, version="0.1.0")
    dedicated_target = entry(DEDICATED, "project", project_path=target, version="0.1.0")

    initial = [user_generic, generic_other_project, dedicated_other_project]
    after_install = [user_generic, generic_other_project, dedicated_other_project, dedicated_target]
    final = [user_generic, generic_other_project, dedicated_other_project, dedicated_target]

    responses = [
        list_response(initial),
        OK,
        OK,
        list_response(after_install),
        list_response(final),
    ]
    result, calls = run_installer(tmp_path, fake_bin, target_project, responses)
    assert result.returncode == 0, result.stdout + result.stderr
    # A dedicated entry under ANOTHER projectPath does not count as the
    # target's dedicated, and the other project's generic is untouched:
    # install happens, uninstall never does.
    assert argv_sequence(calls) == [
        LIST,
        MARKETPLACE_ADD,
        INSTALL_DEDICATED,
        LIST,
        LIST,
    ], f"unexpected sequence: {argv_sequence(calls)}"


# --- final verifier teeth ---------------------------------------------------------------


def test_final_missing_dedicated_fails(
    tmp_path: Path, fake_bin: Path, target_project: Path
) -> None:
    target = str(target_project)
    project_generic = entry(GENERIC, "project", project_path=target, version="0.7.20")
    dedicated_project = entry(DEDICATED, "project", project_path=target, version="0.1.0")

    responses = [
        list_response([project_generic]),
        OK,
        OK,
        list_response([project_generic, dedicated_project]),
        OK,
        list_response([project_generic]),  # dedicated vanished before final check
    ]
    result, calls = run_installer(tmp_path, fake_bin, target_project, responses)
    assert result.returncode != 0
    assert "final verification failed" in result.stderr
    assert argv_sequence(calls) == [
        LIST,
        MARKETPLACE_ADD,
        INSTALL_DEDICATED,
        LIST,
        UNINSTALL_GENERIC,
        LIST,
    ]


def test_final_surviving_generic_fails(
    tmp_path: Path, fake_bin: Path, target_project: Path
) -> None:
    target = str(target_project)
    project_generic = entry(GENERIC, "project", project_path=target, version="0.7.20")
    dedicated_project = entry(DEDICATED, "project", project_path=target, version="0.1.0")

    responses = [
        list_response([project_generic]),
        OK,
        OK,
        list_response([project_generic, dedicated_project]),
        OK,
        list_response([project_generic, dedicated_project]),  # uninstall had no effect
    ]
    result, _ = run_installer(tmp_path, fake_bin, target_project, responses)
    assert result.returncode != 0
    assert "still present" in result.stderr


def test_final_user_entry_changed_fails(
    tmp_path: Path, fake_bin: Path, target_project: Path
) -> None:
    """An out-of-band change to a user-scope entry must fail the final
    invariant even though every mutation 'succeeded'."""
    target = str(target_project)
    user_generic = entry(GENERIC, "user", version="0.7.21")
    project_generic = entry(GENERIC, "project", project_path=target, version="0.7.20")
    dedicated_project = entry(DEDICATED, "project", project_path=target, version="0.1.0")
    mutated_user_generic = entry(GENERIC, "user", version="0.7.99")

    responses = [
        list_response([user_generic, project_generic]),
        OK,
        OK,
        list_response([user_generic, project_generic, dedicated_project]),
        OK,
        list_response([mutated_user_generic, dedicated_project]),
    ]
    result, _ = run_installer(tmp_path, fake_bin, target_project, responses)
    assert result.returncode != 0
    assert "changed unexpectedly" in result.stderr
