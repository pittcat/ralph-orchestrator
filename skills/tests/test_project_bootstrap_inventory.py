"""Unit tests for the ``preset_inventory`` triage helper.

Phase 0 of ``ralph-project-bootstrap`` recommends a preset before the
unified pipeline runs. This module locks the deterministic half of that
phase: the typed inventory of reachable presets (builtin ids via the
binary's introspection surface, file presets via repo-relative reads),
the issue vocabulary, and the derived ``input_contract`` /
``shadows_builtin`` / ``contract_complete`` flags the agent compares
candidates on.

Conventions:

* Tests inject deterministic ``runner`` callables so no real ``ralph``
  binary is spawned; the fake honours ONLY the two introspection argv
  shapes (``preset builtin list`` / ``preset builtin show``).
* File presets are materialised under ``tmp_path``; every read is
  anchored on the supplied ``root``.
"""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

import preset_inventory

ROOT = Path(__file__).resolve().parents[2]


# ---------------------------------------------------------------------------
# Fake introspection runner
# ---------------------------------------------------------------------------

_LIST_ENVELOPE = {
    "presets": [
        {
            "id": "debug",
            "description": "Bug investigation and root-cause analysis",
            "source": "builtin:debug",
            "public": True,
        },
        {
            "id": "merge-batch",
            "description": "Git-first batch merge of worktree branches",
            "source": "builtin:merge-batch",
            "public": True,
        },
    ]
}

_SHOW_BODIES = {
    "debug": (
        "name: debug\n"
        "cli:\n"
        "  backend: claude\n"
        "event_loop:\n"
        "  execution_mode: isolated\n"
        "  max_iterations: 8\n"
        "  max_runtime_seconds: 1800\n"
        "hats:\n"
        "  investigator:\n"
        "    description: Finds root cause through systematic investigation.\n"
        "  verifier:\n"
        "    description: Verifies the fix solves the original problem.\n"
    ),
    "merge-batch": (
        "name: merge-batch\n"
        "cli:\n"
        "  backend: auto\n"
        "event_loop:\n"
        "  execution_mode: isolated\n"
        "  prompt: |\n"
        "    Batch-merge git worktree branches into the target branch.\n"
        "    Read the prompt for target branch and ordered branch list.\n"
        "  completion_promise: merge.batch.complete\n"
        "  required_events:\n"
        "    - merge.stabilized\n"
        "  max_iterations: 40\n"
        "  max_runtime_seconds: 7200\n"
        "hats:\n"
        "  integrator:\n"
        "    description: Merges branches in order.\n"
        "  stabilizer:\n"
        "    description: Verify-fix-retest self-loop.\n"
        "  reporter:\n"
        "    description: Writes the merge report.\n"
    ),
}


def _introspection_runner(
    argv: list[str],
    timeout: object = None,
    capture_output: bool = False,
    text: bool = False,
) -> subprocess.CompletedProcess:
    """Fake ``subprocess.run`` honouring only the introspection argv shapes."""
    if (
        len(argv) >= 6
        and argv[1:4] == ["preset", "builtin", "list"]
        and argv[4] == "--format"
    ):
        return subprocess.CompletedProcess(
            args=argv, returncode=0, stdout=json.dumps(_LIST_ENVELOPE), stderr=""
        )
    if (
        len(argv) >= 7
        and argv[1:4] == ["preset", "builtin", "show"]
        and argv[-2] == "--format"
    ):
        body = _SHOW_BODIES.get(argv[4])
        if body is None:
            return subprocess.CompletedProcess(
                args=argv,
                returncode=2,
                stdout="",
                stderr=f"unknown builtin preset: {argv[4]}",
            )
        return subprocess.CompletedProcess(
            args=argv, returncode=0, stdout=body, stderr=""
        )
    raise AssertionError(f"unexpected introspection argv={list(argv)}")


def _failing_runner(
    argv: list[str],
    timeout: object = None,
    capture_output: bool = False,
    text: bool = False,
) -> subprocess.CompletedProcess:
    raise FileNotFoundError(argv[0])


# ---------------------------------------------------------------------------
# Builtin inventory
# ---------------------------------------------------------------------------


def test_builtin_inventory_collects_typed_candidates() -> None:
    candidates, issues = preset_inventory.collect_builtin_presets(
        binary="ralph", runner=_introspection_runner
    )
    assert issues == ()
    assert [c.preset_id for c in candidates] == ["debug", "merge-batch"]
    debug = candidates[0]
    assert debug.source == "builtin:debug"
    assert debug.origin == "builtin"
    assert debug.description == "Bug investigation and root-cause analysis"
    assert debug.description_source == "envelope"
    assert debug.backend == "claude"
    assert debug.hat_count == 2
    assert debug.execution_mode == "isolated"
    assert debug.inline_prompt_present is False
    assert debug.input_contract == "needs_operator_input"
    assert debug.contract_complete is True
    merge = candidates[1]
    assert merge.inline_prompt_present is True
    assert merge.input_contract == "self_prompted"
    assert "target branch" in merge.prompt_excerpt
    assert merge.completion_promise == "merge.batch.complete"
    assert merge.required_events == ("merge.stabilized",)
    assert merge.hat_summaries.startswith("integrator: Merges branches")


def test_missing_binary_produces_binary_missing_issue() -> None:
    candidates, issues = preset_inventory.collect_builtin_presets(
        binary="definitely-not-ralph", runner=_failing_runner
    )
    assert candidates == ()
    assert [issue.code for issue in issues] == ["binary_missing"]
    assert issues[0].source == "builtin_list"


def test_builtin_list_unparseable_json() -> None:
    def bad_json(argv, **_kwargs):
        return subprocess.CompletedProcess(
            args=argv, returncode=0, stdout="not json {", stderr=""
        )

    candidates, issues = preset_inventory.collect_builtin_presets(
        binary="ralph", runner=bad_json
    )
    assert candidates == ()
    assert [issue.code for issue in issues] == ["builtin_list_unparseable"]


def test_builtin_list_wrong_envelope_shape() -> None:
    def wrong_shape(argv, **_kwargs):
        return subprocess.CompletedProcess(
            args=argv, returncode=0, stdout=json.dumps(["debug"]), stderr=""
        )

    candidates, issues = preset_inventory.collect_builtin_presets(
        binary="ralph", runner=wrong_shape
    )
    assert candidates == ()
    assert [issue.code for issue in issues] == ["builtin_list_unparseable"]


def test_builtin_show_failure_records_issue_and_continues() -> None:
    def show_fails(argv, **kwargs):
        if argv[1:4] == ["preset", "builtin", "show"] and argv[4] == "merge-batch":
            return subprocess.CompletedProcess(
                args=argv, returncode=2, stdout="", stderr="boom"
            )
        return _introspection_runner(argv, **kwargs)

    candidates, issues = preset_inventory.collect_builtin_presets(
        binary="ralph", runner=show_fails
    )
    assert [c.preset_id for c in candidates] == ["debug"]
    assert [issue.code for issue in issues] == ["builtin_show_failed"]
    assert issues[0].source == "builtin:merge-batch"


def test_builtin_show_invalid_yaml() -> None:
    def bad_yaml(argv, **kwargs):
        if argv[1:4] == ["preset", "builtin", "show"] and argv[4] == "debug":
            return subprocess.CompletedProcess(
                args=argv, returncode=0, stdout="- a\n- b\n", stderr=""
            )
        return _introspection_runner(argv, **kwargs)

    candidates, issues = preset_inventory.collect_builtin_presets(
        binary="ralph", runner=bad_yaml
    )
    assert [c.preset_id for c in candidates] == ["merge-batch"]
    assert [issue.code for issue in issues] == ["builtin_show_unparseable"]


# ---------------------------------------------------------------------------
# File preset inventory
# ---------------------------------------------------------------------------


def _write_preset(project: Path, name: str, text: str) -> str:
    (project / name).write_text(text, encoding="utf-8")
    return name


_HEADER_PRESET = (
    "# Merge Batch Preset\n"
    "#\n"
    "# Git-first batch merge: merge worktree branches into a target.\n"
    "cli:\n"
    "  backend: auto\n"
    "event_loop:\n"
    "  prompt: |\n"
    "    Read the prompt for the target branch.\n"
    "  max_iterations: 10\n"
    "  max_runtime_seconds: 600\n"
)

_NO_HEADER_PRESET = (
    "cli:\n"
    "  backend: claude\n"
    "event_loop:\n"
    "  max_iterations: 5\n"
    "  max_runtime_seconds: 300\n"
    "hats:\n"
    "  only-hat:\n"
    "    description: Does the whole job alone.\n"
)


def test_file_preset_header_comment_becomes_description(tmp_path: Path) -> None:
    token = _write_preset(tmp_path, "merge-batch.yml", _HEADER_PRESET)
    candidates, issues = preset_inventory.collect_file_presets(
        [token], root=tmp_path
    )
    assert issues == ()
    (candidate,) = candidates
    assert candidate.origin == "file"
    assert candidate.source == token
    assert candidate.description_source == "header_comment"
    assert "Git-first batch merge" in candidate.description
    assert candidate.backend == "auto"
    assert candidate.inline_prompt_present is True


def test_file_preset_without_header_uses_hat_summaries(tmp_path: Path) -> None:
    token = _write_preset(tmp_path, "plain.yml", _NO_HEADER_PRESET)
    candidates, issues = preset_inventory.collect_file_presets(
        [token], root=tmp_path
    )
    assert issues == ()
    (candidate,) = candidates
    assert candidate.description == ""
    assert candidate.description_source == ""
    assert candidate.hat_ids == ("only-hat",)
    assert "Does the whole job alone" in candidate.hat_summaries


def test_file_preset_unsafe_path_rejected_before_read(tmp_path: Path) -> None:
    candidates, issues = preset_inventory.collect_file_presets(
        ["../outside.yml", "/abs/path.yml"], root=tmp_path
    )
    assert candidates == ()
    assert {issue.code for issue in issues} == {"input_path_unsafe"}


def test_file_preset_missing_and_invalid(tmp_path: Path) -> None:
    token = _write_preset(tmp_path, "broken.yml", "cli: [unclosed\n")
    candidates, issues = preset_inventory.collect_file_presets(
        [token, "does-not-exist.yml"], root=tmp_path
    )
    assert candidates == ()
    codes = {issue.code for issue in issues}
    assert codes == {"preset_yaml_invalid", "input_missing_preset_file"}


def test_duplicate_tokens_deduped(tmp_path: Path) -> None:
    token = _write_preset(tmp_path, "merge-batch.yml", _HEADER_PRESET)
    candidates, issues = preset_inventory.collect_file_presets(
        [token, token], root=tmp_path
    )
    assert issues == ()
    assert len(candidates) == 1


# ---------------------------------------------------------------------------
# run_inventory — ordering, shadowing, dirs, envelope
# ---------------------------------------------------------------------------


def test_run_inventory_shadow_flag_and_ordering(tmp_path: Path) -> None:
    (tmp_path / "presets").mkdir()
    _write_preset(tmp_path / "presets", "debug.yml", _HEADER_PRESET)
    _write_preset(tmp_path / "presets", "local-only.yml", _HEADER_PRESET)
    inventory = preset_inventory.run_inventory(
        binary="ralph",
        presets_dirs=("presets",),
        cwd=tmp_path,
        runner=_introspection_runner,
    )
    assert inventory.issues == ()
    # Builtin candidates come first (envelope order), then file candidates
    # sorted by token.
    sources = [c.source for c in inventory.presets]
    assert sources[:2] == ["builtin:debug", "builtin:merge-batch"]
    assert sources[2:] == ["presets/debug.yml", "presets/local-only.yml"]
    by_id = {c.source: c for c in inventory.presets}
    assert by_id["presets/debug.yml"].shadows_builtin is True
    assert by_id["presets/local-only.yml"].shadows_builtin is False
    assert by_id["builtin:debug"].shadows_builtin is False


def test_presets_dir_missing_records_issue(tmp_path: Path) -> None:
    inventory = preset_inventory.run_inventory(
        binary="ralph",
        presets_dirs=("no-such-dir",),
        cwd=tmp_path,
        include_builtins=False,
    )
    assert inventory.presets == ()
    assert [issue.code for issue in inventory.issues] == ["presets_dir_missing"]


def test_no_builtin_skips_introspection(tmp_path: Path) -> None:
    def forbidden(argv, **_kwargs):
        raise AssertionError("runner must not be invoked")

    token = _write_preset(tmp_path, "merge-batch.yml", _HEADER_PRESET)
    inventory = preset_inventory.run_inventory(
        binary="ralph",
        preset_paths=(token,),
        cwd=tmp_path,
        include_builtins=False,
        runner=forbidden,
    )
    assert len(inventory.presets) == 1
    assert inventory.presets[0].origin == "file"


def test_contract_incomplete_flagged(tmp_path: Path) -> None:
    token = _write_preset(
        tmp_path,
        "no-backend.yml",
        "event_loop:\n  max_iterations: 5\n  max_runtime_seconds: 300\n",
    )
    candidates, _ = preset_inventory.collect_file_presets([token], root=tmp_path)
    (candidate,) = candidates
    assert candidate.backend == ""
    assert candidate.contract_complete is False


def test_json_envelope_carries_derived_fields(tmp_path: Path) -> None:
    token = _write_preset(tmp_path, "merge-batch.yml", _HEADER_PRESET)
    inventory = preset_inventory.run_inventory(
        binary="ralph",
        preset_paths=(token,),
        cwd=tmp_path,
        runner=_introspection_runner,
    )
    payload = json.loads(inventory.to_json())
    assert payload["binary"] == "ralph"
    rows = {row["source"]: row for row in payload["presets"]}
    assert rows["builtin:merge-batch"]["input_contract"] == "self_prompted"
    assert rows[token]["input_contract"] == "self_prompted"
    assert rows[token]["shadows_builtin"] is True
    assert isinstance(payload["issues"], list)


def test_prompt_excerpt_is_capped(tmp_path: Path) -> None:
    long_prompt = "word " * 200
    token = _write_preset(
        tmp_path,
        "long.yml",
        "cli:\n  backend: claude\nevent_loop:\n  prompt: |\n    "
        + long_prompt
        + "\n  max_iterations: 5\n  max_runtime_seconds: 300\n",
    )
    candidates, _ = preset_inventory.collect_file_presets([token], root=tmp_path)
    (candidate,) = candidates
    assert len(candidate.prompt_excerpt) <= 400
    assert candidate.prompt_excerpt.endswith("…")
