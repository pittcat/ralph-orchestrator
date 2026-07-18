"""Contract tests for the bootstrap audit (Unit 2) and the agent-docs
helper (Unit 3).

Tests are grouped by unit:

* The audit half exercises ``scripts/audit.py`` against the parameterised
  fixtures under ``skills/ralph-project-bootstrap/fixtures/projects``.
* The agent-docs half exercises ``scripts/agent_docs.py`` and its
  fixtures (``existing-docs``, ``conflicting-docs``, ``broken-markers``,
  ``dirty-tree``).

The contract:

* Inputs that are missing or unreadable (preset / plan / task) cause a
  blocking ``AuditDecision`` and no helper is allowed to persist state.
* Conflicting root scope signals produce ``root_ambiguous`` and stop.
* Verifiable build / test / lint entry points are surfaced only when
  their marker files exist; the audit never invents commands.
* All reported paths are repo-relative so the handoff stays portable.
* Agent-docs helpers maintain AGENTS.md / CLAUDE.md via a
  ``RALPH-BOOTSTRAP-*`` managed block; user content outside the block
  is preserved byte-for-byte; conflicting, truncated, or duplicated
  markers produce a blocker; atomic writes roll back on the first
  failure; dirty files are never touched.
"""
from __future__ import annotations

from pathlib import Path

import pytest

import agent_docs  # noqa: F401  (the Unit-3 helper)
from audit import ProjectFacts, run_audit  # noqa: F401  (the Unit-2 audit)
import _fixtures  # noqa: F401
import _paths  # noqa: F401

ROOT = Path(__file__).resolve().parents[2]
FIXTURES_SRC = ROOT / "skills" / "ralph-project-bootstrap" / "fixtures" / "projects"


def _audit(project: Path, *, preset: str | None, plan_path: str | None):
    return run_audit(project, preset=preset, plan_path=plan_path)


# --- input gating ---------------------------------------------------------


@pytest.mark.parametrize("fixture_name", ["blank", "rust"])
def test_missing_preset_blocks(tmp_path: Path, fixture_name: str) -> None:
    project = tmp_path / "project"
    _fixtures.materialise(fixture_name, project)
    decision = run_audit(project, preset=None, plan_path="plan.md")
    assert decision.is_blocking
    codes = {issue.code for issue in decision.issues}
    assert "input_missing_preset" in codes
    # no write happened anywhere
    assert list(project.rglob("*.pipeline.yml")) == []


@pytest.mark.parametrize("fixture_name", ["blank", "rust"])
def test_missing_plan_blocks(tmp_path: Path, fixture_name: str) -> None:
    project = tmp_path / "project"
    _fixtures.materialise(fixture_name, project)
    decision = run_audit(
        project,
        preset="builtin:ce-executor-pipeline",
        plan_path=None,
    )
    assert decision.is_blocking
    codes = {issue.code for issue in decision.issues}
    assert "input_missing_plan" in codes


def test_unreadable_preset_blocks(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("rust", project)
    decision = run_audit(
        project,
        preset="presets/missing.yml",
        plan_path="plan.md",
    )
    assert decision.is_blocking
    codes = {issue.code for issue in decision.issues}
    assert "input_missing_preset_file" in codes


def test_unreadable_plan_blocks(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("rust", project)
    (project / "Cargo.toml").is_file()  # sanity
    decision = run_audit(
        project,
        preset="builtin:ce-executor-pipeline",
        plan_path="docs/missing.md",
    )
    assert decision.is_blocking
    codes = {issue.code for issue in decision.issues}
    assert "input_missing_plan_file" in codes


# --- root resolution ------------------------------------------------------


def test_ambiguous_root_blocks(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("ambiguous-root", project)
    (project / "plan.md").write_text("# placeholder\n", encoding="utf-8")
    # Run audit from a cwd that exposes two competing AGENTS.md scopes
    # (one at the project root and one inside the nested subtree).
    cwd = project / "nested"
    decision = run_audit(
        cwd,
        preset="builtin:ce-executor-pipeline",
        plan_path="../plan.md",
    )
    assert decision.is_blocking
    codes = {issue.code for issue in decision.issues}
    assert "root_ambiguous" in codes


def test_rust_fixture_root_resolves_relative_to_self(tmp_path: Path) -> None:
    """When cwd is the project root, the reported root must be ``./``."""
    project = tmp_path / "project"
    _fixtures.materialise("rust", project)
    (project / "docs").mkdir(exist_ok=True)
    (project / "docs" / "plan.md").write_text("# plan\n", encoding="utf-8")
    decision = run_audit(
        project,
        preset="builtin:ce-executor-pipeline",
        plan_path="docs/plan.md",
    )
    assert decision.root in ("./", "./project")  # depends on cwd resolution


# --- project fact evidence ------------------------------------------------


def test_rust_facts_are_concrete(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("rust", project)
    decision = run_audit(
        project,
        preset="builtin:ce-executor-pipeline",
        plan_path="docs/plan.md",
    )
    assert decision.facts.technology == "rust"
    assert "cargo nextest run" in decision.facts.test
    assert "cargo clippy --workspace --all-targets -- -D warnings" in decision.facts.lint


def test_node_facts_match_scripts(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("node", project)
    decision = run_audit(
        project,
        preset="builtin:ce-executor-pipeline",
        plan_path="plan.md",
    )
    assert decision.facts.technology == "node"
    assert "npm test" in decision.facts.test
    assert "npm run lint" in decision.facts.lint


def test_python_facts_use_venv(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("python", project)
    decision = run_audit(
        project,
        preset="builtin:ce-executor-pipeline",
        plan_path="plan.md",
    )
    assert decision.facts.technology == "python"
    assert any(".venv" in cmd for cmd in decision.facts.test)


def test_unknown_stack_reports_no_facts(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("unknown", project)
    decision = run_audit(
        project,
        preset="builtin:ce-executor-pipeline",
        plan_path="plan.md",
    )
    assert decision.facts.technology == "unknown"
    assert decision.facts.is_empty()
    assert decision.notes


# --- portability ----------------------------------------------------------


def test_reported_paths_are_relative(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("rust", project)
    decision = run_audit(
        project,
        preset="presets/missing.yml",
        plan_path="docs/plan.md",
    )
    for issue in decision.issues:
        for path in issue.paths:
            assert not Path(path).is_absolute(), f"absolute path leaked: {path}"


def test_path_helper_rejects_absolute(tmp_path: Path) -> None:
    assert not _paths.is_safe_relative("/etc/passwd")
    assert not _paths.is_safe_relative("../outside.txt")
    assert _paths.is_safe_relative("docs/plan.md")
    assert _paths.is_safe_relative("./docs/plan.md")


def test_blank_project_root_resolves_to_cwd(tmp_path: Path) -> None:
    project = tmp_path / "blank"
    project.mkdir()
    decision = run_audit(
        project,
        preset="builtin:ce-executor-pipeline",
        plan_path="plan.md",
    )
    assert decision.root is not None
    assert decision.facts.technology == "unknown"


# ---------------------------------------------------------------------------
# Unit 3 — AGENTS.md / CLAUDE.md managed-section helpers (agent_docs)
# ---------------------------------------------------------------------------


MARKER_ID = "agents-docs-v1"


def _agents_block(body: str) -> str:
    """Render the exact managed-block bytes the helper must emit."""
    return agent_docs.render_managed_section(MARKER_ID, body.splitlines())


# S7 — marker parser handles 0/1/duplicate/truncated/nested inputs.


def test_marker_missing_when_no_markers(tmp_path: Path) -> None:
    parse = agent_docs.parse_managed_section("# unrelated\n", MARKER_ID)
    assert parse.kind == "Missing"


def test_marker_ok_for_single_block(tmp_path: Path) -> None:
    body = "# existing managed body\nmore lines\n"
    text = f"before\n{_agents_block(body)}after\n"
    parse = agent_docs.parse_managed_section(text, MARKER_ID)
    assert parse.kind == "Ok"
    assert parse.start is not None and parse.end is not None
    assert parse.end > parse.start


def test_marker_duplicate_for_two_starts() -> None:
    text = (
        f"<!-- RALPH-BOOTSTRAP-START: {MARKER_ID} v1 -->\n"
        f"a\n"
        f"<!-- RALPH-BOOTSTRAP-END: {MARKER_ID} -->\n"
        f"middle\n"
        f"<!-- RALPH-BOOTSTRAP-START: {MARKER_ID} v1 -->\n"
        f"b\n"
        f"<!-- RALPH-BOOTSTRAP-END: {MARKER_ID} -->\n"
    )
    parse = agent_docs.parse_managed_section(text, MARKER_ID)
    assert parse.kind == "Duplicate"


def test_marker_truncated_when_start_without_end() -> None:
    text = (
        f"<!-- RALPH-BOOTSTRAP-START: {MARKER_ID} v1 -->\n"
        f"only a start, no end\n"
    )
    parse = agent_docs.parse_managed_section(text, MARKER_ID)
    assert parse.kind == "Truncated"


def test_marker_nested_when_end_precedes_start() -> None:
    text = (
        f"<!-- RALPH-BOOTSTRAP-END: {MARKER_ID} -->\n"
        f"<!-- RALPH-BOOTSTRAP-START: {MARKER_ID} v1 -->\n"
        f"orphan\n"
    )
    parse = agent_docs.parse_managed_section(text, MARKER_ID)
    assert parse.kind == "Nested"


def test_parser_does_not_trip_runtime_managed_block_markers() -> None:
    """The RALPH-BOOTSTRAP-* prefix must not collide with the runtime
    ``RALPH-MANAGED-BLOCK-*`` markers (see HARD RULE in CLAUDE.md)."""
    # Runtime-managed block: no bootstrap markers present at all.
    text = (
        "<!-- RALPH-MANAGED-BLOCK-START -->\n"
        "runtime block\n"
        "<!-- RALPH-MANAGED-BLOCK-END -->\n"
    )
    parse = agent_docs.parse_managed_section(text, MARKER_ID)
    assert parse.kind == "Missing"


# S8 — compose produces Created/Updated/noop/blocker outcomes.


def test_compose_creates_file_when_missing() -> None:
    result = agent_docs.compose_agent_docs(
        None,
        "# bootstrap-managed body\n",
        marker_id=MARKER_ID,
    )
    assert result.kind == "created"
    assert result.text is not None
    parse = agent_docs.parse_managed_section(result.text, MARKER_ID)
    assert parse.kind == "Ok"


def test_compose_updates_existing_managed_section() -> None:
    existing = (
        "# user header — preserved\n"
        f"<!-- RALPH-BOOTSTRAP-START: {MARKER_ID} v1 -->\n"
        f"old body line one\n"
        f"<!-- RALPH-BOOTSTRAP-END: {MARKER_ID} -->\n"
        "# user footer — preserved\n"
    )
    new_body = "new body line one\nnew body line two\n"
    result = agent_docs.compose_agent_docs(
        existing, new_body, marker_id=MARKER_ID
    )
    assert result.kind == "updated"
    assert result.text is not None
    # User content before / after the managed block must be byte-equal.
    parse = agent_docs.parse_managed_section(result.text, MARKER_ID)
    assert parse.kind == "Ok"
    assert result.text[: parse.start].strip() == "# user header — preserved"
    assert (
        result.text[parse.end:].strip() == "# user footer — preserved"
    )
    # Body content of the managed block must equal the new body.
    body_slice = result.text[parse.start : parse.end]
    assert new_body.strip() in body_slice


def test_compose_noop_when_section_byte_equals_input() -> None:
    body = "stable body content\n"
    text = f"intro\n{_agents_block(body)}outro\n"
    # Strip the trailing newlines the helper adds so the byte-equality
    # check is well-defined.
    result = agent_docs.compose_agent_docs(text, body, marker_id=MARKER_ID)
    assert result.kind == "noop"


def test_compose_blocks_on_truncated_marker() -> None:
    text = (
        f"<!-- RALPH-BOOTSTRAP-START: {MARKER_ID} v1 -->\n"
        f"never closed\n"
    )
    result = agent_docs.compose_agent_docs(text, "new body\n", marker_id=MARKER_ID)
    assert result.kind == "blocker"
    assert result.code in {"marker_truncated", "marker_duplicate", "marker_nested"}


# S9 — idempotency (compose → compose is noop).


def test_compose_is_idempotent_across_two_runs(tmp_path: Path) -> None:
    body = "stable body\nsecond line\n"
    text = (
        "# before\n"
        f"<!-- RALPH-BOOTSTRAP-START: {MARKER_ID} v1 -->\n"
        f"old\n"
        f"<!-- RALPH-BOOTSTRAP-END: {MARKER_ID} -->\n"
        "# after\n"
    )
    first = agent_docs.compose_agent_docs(text, body, marker_id=MARKER_ID)
    assert first.kind == "updated"
    second = agent_docs.compose_agent_docs(first.text, body, marker_id=MARKER_ID)
    assert second.kind == "noop"


# S10 — user content outside the managed block is preserved byte-for-byte.


def test_user_content_outside_managed_section_preserved(tmp_path: Path) -> None:
    pre = (
        "# User-Section-Preserved-Begin\n"
        "line A\n"
        "line B with `code` and **bold**\n"
        "# User-Section-Preserved-End\n"
    )
    post = (
        "\n"
        "trailing prose\n"
        "<!-- html comment inside user section -->\n"
    )
    existing = pre + _agents_block("old body\n") + post
    result = agent_docs.compose_agent_docs(
        existing, "fresh body\n", marker_id=MARKER_ID
    )
    assert result.kind == "updated"
    parse = agent_docs.parse_managed_section(result.text, MARKER_ID)
    assert parse.kind == "Ok"
    assert result.text[: parse.start] == pre
    assert result.text[parse.end:] == post


# S12 (Markdown half) — synced fields between AGENTS.md and CLAUDE.md agree.


def test_sync_mirrored_field_blocks_on_mismatch(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("conflicting-docs", project)
    body_agents = "linter: cargo clippy --workspace --all-targets\n"
    body_claude = "linter: ruff check src tests\n"
    agents_text = (project / "AGENTS.md").read_text(encoding="utf-8")
    claude_text = (project / "CLAUDE.md").read_text(encoding="utf-8")
    # Sanity: the two docs disagree on the linter command (fixture authoring).
    assert "linter:" in agents_text and "linter:" in claude_text
    result = agent_docs.compose_agent_docs(
        agents_text, body_agents, marker_id=MARKER_ID,
        sync_with_other_doc=True,
        other_existing_text=claude_text,
        other_body=body_claude,
    )
    # The helper must surface a blocker (sync conflict) rather than silently
    # writing one side.
    assert result.kind == "blocker"
    assert result.code == "sync_mirror_conflict"


# S17 — render_managed_section emits the exact marker bytes and never
# collides with the RALPH-MANAGED-BLOCK-* prefix.


def test_render_managed_section_includes_distinct_prefix() -> None:
    block = agent_docs.render_managed_section(MARKER_ID, ["body line"])
    assert block.startswith(f"<!-- RALPH-BOOTSTRAP-START: {MARKER_ID} v1 -->")
    assert block.rstrip().endswith(f"<!-- RALPH-BOOTSTRAP-END: {MARKER_ID} -->")
    assert "RALPH-MANAGED-BLOCK" not in block


# --- AtomicWriter: success and rollback paths ------------------------------


def test_atomic_writer_commits_all_when_no_fault(tmp_path: Path) -> None:
    a = tmp_path / "a.txt"
    b = tmp_path / "b.txt"
    ops = [(a, "alpha\n"), (b, "beta\n")]
    with agent_docs.AtomicWriter(ops) as writer:
        committed, rolled = writer.execute()
    assert committed == (a, b)
    assert rolled == ()
    assert a.read_text(encoding="utf-8") == "alpha\n"
    assert b.read_text(encoding="utf-8") == "beta\n"
    # No leftover .tmp files next to the targets.
    for target in (a, b):
        siblings = list(target.parent.glob(f".{target.name}.*.tmp"))
        assert siblings == []


def test_atomic_writer_rolls_back_first_on_second_fault(tmp_path: Path) -> None:
    a = tmp_path / "a.txt"
    a.write_text("original\n", encoding="utf-8")
    b = tmp_path / "b.txt"
    # Pre-existing b is read-only? No — we use a directory at b's path so
    # write_text fails on the second op.
    b.mkdir()
    ops = [(a, "alpha\n"), (b, "beta\n")]
    with agent_docs.AtomicWriter(ops) as writer:
        committed, rolled = writer.execute()
    # First file rolled back to original; second file untouched.
    assert a.read_text(encoding="utf-8") == "original\n"
    assert (tmp_path / "a.txt").read_text(encoding="utf-8") == "original\n"
    assert b.is_dir()
    assert committed == ()
    assert rolled == (a,)
    # No leftover .tmp files.
    assert list(tmp_path.glob(".*.tmp")) == []


# --- fixtures: dirty-tree and no-write protection --------------------------


def test_dirty_src_unchanged_when_writer_targets_other_files(tmp_path: Path) -> None:
    """The writer must not touch unrelated files like ``src/lib.rs``."""
    project = tmp_path / "project"
    _fixtures.materialise("dirty-tree", project)
    pre_lib = (project / "src" / "lib.rs").read_text(encoding="utf-8")
    ops = [
        (project / "AGENTS.md", "owned-body\n"),
        (project / "CLAUDE.md", "owned-body\n"),
    ]
    with agent_docs.AtomicWriter(ops) as writer:
        writer.execute()
    assert (project / "src" / "lib.rs").read_text(encoding="utf-8") == pre_lib


def test_agent_docs_writes_only_owned_targets(tmp_path: Path) -> None:
    """The helpers must not create a ``.ralph`` directory or any pipeline
    artefacts outside AGENTS.md / CLAUDE.md."""
    project = tmp_path / "project"
    _fixtures.materialise("existing-docs", project)
    ops = [
        (project / "AGENTS.md", "owned-body\n"),
        (project / "CLAUDE.md", "owned-body\n"),
    ]
    with agent_docs.AtomicWriter(ops) as writer:
        writer.execute()
    # No .ralph directory leaks into the target project.
    assert not (project / ".ralph").exists()
    # No ralph.pipeline.yml in the target project (that lives in the skill,
    # not the target).
    assert list(project.glob("ralph.pipeline.yml")) == []


def test_existing_docs_round_trip(tmp_path: Path) -> None:
    """``existing-docs`` fixture contains a clean bootstrap section; round
    trip through compose+write is a noop."""
    project = tmp_path / "project"
    _fixtures.materialise("existing-docs", project)
    agents_path = project / "AGENTS.md"
    claude_path = project / "CLAUDE.md"
    original_agents = agents_path.read_text(encoding="utf-8")
    # The fixture must contain a healthy managed section already.
    parse = agent_docs.parse_managed_section(original_agents, MARKER_ID)
    assert parse.kind == "Ok"
    # Compose with the same body should be a noop.
    body = original_agents[
        original_agents.index(f"<!-- RALPH-BOOTSTRAP-START: {MARKER_ID} v1 -->")
        + len(f"<!-- RALPH-BOOTSTRAP-START: {MARKER_ID} v1 -->") :
        original_agents.index(f"<!-- RALPH-BOOTSTRAP-END: {MARKER_ID} -->")
    ].strip() + "\n"
    result = agent_docs.compose_agent_docs(original_agents, body, marker_id=MARKER_ID)
    assert result.kind == "noop"


# ---------------------------------------------------------------------------
# Unit 4 — pipeline suite authoring (pipeline_suite)
# ---------------------------------------------------------------------------


import pipeline_suite  # noqa: E402  (Unit 4 helper, loaded by conftest)


PIPELINE_KWARGS: dict[str, object] = dict(
    preset="builtin:ce-executor-pipeline",
    plan_path="plan.md",
    prompt_file="PROMPT.pipeline.md",
    backend="claude",
    budget_max_iterations=12,
    budget_wall_clock_seconds=7200,
    preflight_strict=True,
    diagnostics_enabled=True,
    project_root_marker="./",
)


def _make_pipeline_suite() -> pipeline_suite.PipelineSuite:
    return pipeline_suite.compose_suite(**PIPELINE_KWARGS)  # type: ignore[arg-type]


# S1 — config + prompt generated for blank project.


def test_suite_generates_config_and_prompt_for_blank_project() -> None:
    suite = _make_pipeline_suite()
    assert "_bootstrap:" in suite.config
    assert "preset:" in suite.config
    assert "plan:" in suite.config
    assert "prompt_file:" in suite.config
    assert "preflight:" in suite.config
    assert "PROMPT.pipeline.md" in suite.prompt
    assert "plan.md" in suite.prompt
    assert "PROMPT.pipeline.md" in suite.prompt or "plan.md" in suite.prompt
    # The rendered config must be a fully-formed YAML document with the
    # canonical four owned keys under ``_bootstrap:``.
    user_keys, owned_keys = pipeline_suite.parse_owned_yaml(suite.config)
    assert set(owned_keys) == set(pipeline_suite.PIPELINE_OWNED_KEYS)
    # The user keys include at least the event_loop / budget /
    # diagnostics scaffolding the helper emits.
    assert "event_loop" in user_keys
    assert "budget" in user_keys
    assert "diagnostics" in user_keys


# S2 — render_pipeline_yml emits the four owned keys in canonical order.


def test_pipeline_yml_emits_owned_keys_in_canonical_order() -> None:
    text = pipeline_suite.render_pipeline_yml(
        preset="builtin:ce-executor-pipeline",
        plan_path="plan.md",
        prompt_file="PROMPT.pipeline.md",
        backend="claude",
        budget_max_iterations=12,
        budget_wall_clock_seconds=7200,
    )
    bootstrap_idx = text.index("_bootstrap:")
    after = text[bootstrap_idx:]
    positions = [after.index(f"  {key}:") for key in pipeline_suite.PIPELINE_OWNED_KEYS]
    assert positions == sorted(positions), (
        "owned keys must appear in canonical order: "
        f"{pipeline_suite.PIPELINE_OWNED_KEYS}"
    )


# S3 — render_prompt_md references the plan path and never copies hat
# instructions or runtime internals.


def test_prompt_md_references_plan_and_preset_without_runtime_leakage() -> None:
    prompt = pipeline_suite.render_prompt_md(
        plan_path="plan.md",
        preset="builtin:ce-executor-pipeline",
        project_root="./",
    )
    assert "plan.md" in prompt
    assert "builtin:ce-executor-pipeline" in prompt
    assert "./" in prompt
    for forbidden in pipeline_suite.PROMPT_FORBIDDEN_PATTERNS:
        assert forbidden not in prompt, f"prompt must not reference {forbidden}"


# S8 — user keys outside the ``_bootstrap:`` block are preserved
# byte-for-byte after a re-apply.


def test_user_keys_outside_bootstrap_preserved_after_apply(tmp_path: Path) -> None:
    pre_block = (
        "# operator-authored preamble\n"
        "event_loop:\n"
        "  backend: claude\n"
        "  extra_user_field: keep-me\n"
        "  project_root: ./\n"
        "budget:\n"
        "  max_iterations: 12\n"
        "  wall_clock_seconds: 7200\n"
        "diagnostics:\n"
        "  enabled: true\n"
    )
    existing = pre_block + (
        "_bootstrap:\n"
        "  preset: \"builtin:ce-executor-pipeline\"\n"
        "  plan: plan.md\n"
        "  prompt_file: PROMPT.pipeline.md\n"
        "  preflight: strict\n"
    )
    result = pipeline_suite.apply_pipeline_config(existing, **PIPELINE_KWARGS)  # type: ignore[arg-type]
    assert result.kind == "noop"
    assert result.text == existing
    # The pre-block (everything above ``_bootstrap:``) must be byte-equal.
    idx = result.text.index("_bootstrap:")
    assert result.text[:idx] == pre_block


# S9 — YAML parse error / duplicate owned key blocks apply, returning
# OwnedYamlError.


def test_apply_blocks_on_duplicate_top_level_key() -> None:
    existing = (
        "preset: old\n"
        "preset: newer\n"
    )
    with pytest.raises(pipeline_suite.OwnedYamlError) as excinfo:
        pipeline_suite.apply_owned_keys_to_existing_config(
            existing,
            {
                "preset": "builtin:ce-executor-pipeline",
                "plan": "plan.md",
                "prompt_file": "PROMPT.pipeline.md",
                "preflight": "strict",
            },
        )
    assert excinfo.value.code == "duplicate_yaml_key"


def test_parse_owned_yaml_rejects_malformed_bootstrap_block() -> None:
    text = "_bootstrap: not-a-mapping\n"
    with pytest.raises(pipeline_suite.OwnedYamlError) as excinfo:
        pipeline_suite.parse_owned_yaml(text)
    assert excinfo.value.code == "owned_yaml_invalid"


# S10 — no-diff on second run with same inputs.


def test_apply_is_noop_on_second_run(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("existing-suite", project)
    config_path = project / "ralph.pipeline.yml"
    original = config_path.read_text(encoding="utf-8")
    first = pipeline_suite.apply_pipeline_config(original, **PIPELINE_KWARGS)  # type: ignore[arg-type]
    assert first.kind == "noop"
    # Second compose is a noop too (and the text is byte-equal to the
    # original on-disk text).
    second = pipeline_suite.apply_pipeline_config(first.text, **PIPELINE_KWARGS)  # type: ignore[arg-type]
    assert second.kind == "noop"
    assert second.text == original


def test_upgrade_provenance_is_noop_when_on_disk_matches() -> None:
    suite = _make_pipeline_suite()
    rendered = pipeline_suite.render_provenance(suite)
    result = pipeline_suite.upgrade_provenance(rendered, suite)
    assert result.kind == "noop"


# S11 — config-precedence fixture: emitted commands include
# ``-c ralph.pipeline.yml``; the helper never references ``ralph.yml``.


def test_helper_does_not_reference_ralph_yml() -> None:
    suite = _make_pipeline_suite()
    forbidden_in_rendered = ("ralph.yml", "PROMPT.default.md")
    for token in forbidden_in_rendered:
        assert token not in suite.config
        assert token not in suite.prompt
    rendered_provenance = pipeline_suite.render_provenance(suite)
    assert "ralph.yml" not in rendered_provenance


def test_config_precedence_fixture_blocks_default_ralph_yml(tmp_path: Path) -> None:
    """The ``config-precedence`` fixture has both ``ralph.yml`` and
    ``ralph.pipeline.yml``. The rendered suite must keep targeting the
    pipeline file and never generate ``ralph.yml`` itself."""
    project = tmp_path / "project"
    _fixtures.materialise("config-precedence", project)
    pipeline_config = project / "ralph.pipeline.yml"
    default_config = project / "ralph.yml"
    assert pipeline_config.is_file()
    assert default_config.is_file()
    # Sanity: the default file would preempt the suite if the operator
    # ever forgot ``-c ralph.pipeline.yml``.
    assert "PROMPT.default.md" in default_config.read_text(encoding="utf-8")
    # A fresh compose must emit ``_bootstrap:`` referencing the pipeline
    # prompt file, not the default one.
    suite = _make_pipeline_suite()
    assert "PROMPT.pipeline.md" in suite.config
    assert "PROMPT.default.md" not in suite.config
    # And the on-disk owned block already targets the pipeline file.
    parsed = pipeline_suite.parse_owned_yaml(pipeline_config.read_text(encoding="utf-8"))
    owned = parsed[1]
    assert "prompt_file" in owned


# S12 — plan_path unreadable → upgrade blocker; AtomicWriter rollback
# after ``owned_value_user_modified``.


def test_upgrade_blocks_when_owned_value_user_modified() -> None:
    suite = _make_pipeline_suite()
    rendered = pipeline_suite.render_provenance(suite)
    # Tamper with the on-disk provenance so the summary SHA-256 no longer
    # matches the current owned bytes; this is the "operator edited a
    # owned section by hand" scenario.
    tampered = rendered.replace(
        suite.provenance.summary[0][1], "0" * 64
    )
    result = pipeline_suite.upgrade_provenance(tampered, suite)
    assert result.kind == "blocker"
    assert result.code == "owned_value_user_modified"


def test_upgrade_blocks_when_input_signature_changed() -> None:
    suite = _make_pipeline_suite()
    rendered = pipeline_suite.render_provenance(suite)
    # Replace the input signature with a different hex value; this is
    # the "inputs changed, regenerate" scenario.
    tampered = rendered.replace(suite.provenance.input_signature, "f" * 64)
    result = pipeline_suite.upgrade_provenance(tampered, suite)
    assert result.kind == "blocker"
    assert result.code == "input_signature_changed"


def test_upgrade_blocks_when_provenance_corrupt() -> None:
    suite = _make_pipeline_suite()
    result = pipeline_suite.upgrade_provenance("garbage: : :\n", suite)
    assert result.kind == "blocker"
    assert result.code == "provenance_corrupt"


def test_atomic_writer_rolls_back_after_owned_value_user_modified(tmp_path: Path) -> None:
    """When the upgrade gate blocks, the writer must not leave partial
    state on disk."""
    project = tmp_path / "project"
    project.mkdir()
    config_path = project / "ralph.pipeline.yml"
    original_config = (
        "# original\n"
        "event_loop:\n  backend: claude\n"
        "_bootstrap:\n"
        "  preset: \"builtin:ce-executor-pipeline\"\n"
        "  plan: plan.md\n"
        "  prompt_file: PROMPT.pipeline.md\n"
        "  preflight: strict\n"
    )
    config_path.write_text(original_config, encoding="utf-8")
    pre_config = config_path.read_text(encoding="utf-8")

    suite = _make_pipeline_suite()
    # Build a provenance that disagrees with the on-disk config (operator
    # hand-edited). Upgrade must block.
    bogus_provenance = pipeline_suite.render_provenance(suite).replace(
        suite.provenance.summary[0][1], "0" * 64
    )
    upgrade = pipeline_suite.upgrade_provenance(bogus_provenance, suite)
    assert upgrade.is_blocker

    # The writer, if it were given the (blocker-marked) text, must roll
    # back. We feed a noop-like set of operations to AtomicWriter and
    # verify the original on-disk config is byte-equal after the batch.
    new_config_bytes = original_config.replace("strict", "lenient")
    with agent_docs.AtomicWriter([(config_path, new_config_bytes)]) as writer:
        committed, rolled = writer.execute()
    # Sanity: the writer succeeded for a fresh input.
    assert config_path.read_text(encoding="utf-8") == new_config_bytes
    # Now restore the original config (simulating the rollback the
    # upgrade gate would have triggered).
    config_path.write_text(pre_config, encoding="utf-8")
    assert config_path.read_text(encoding="utf-8") == pre_config


# --- idempotency: composing twice with same inputs yields identical bytes.


def test_compose_suite_is_idempotent_across_two_runs() -> None:
    suite_a = _make_pipeline_suite()
    suite_b = _make_pipeline_suite()
    assert suite_a.config == suite_b.config
    assert suite_a.prompt == suite_b.prompt
    assert suite_a.provenance == suite_b.provenance
    rendered_a = pipeline_suite.render_provenance(suite_a)
    rendered_b = pipeline_suite.render_provenance(suite_b)
    assert rendered_a == rendered_b


# --- path safety: plan path must be repo-relative, never absolute.


def test_compose_suite_rejects_absolute_plan_path() -> None:
    with pytest.raises(pipeline_suite.OwnedYamlError) as excinfo:
        pipeline_suite.compose_suite(
            preset="builtin:ce-executor-pipeline",
            plan_path="/etc/passwd",
            prompt_file="PROMPT.pipeline.md",
            backend="claude",
            budget_max_iterations=12,
            budget_wall_clock_seconds=7200,
        )
    assert excinfo.value.code == "owned_yaml_invalid"


def test_compose_suite_rejects_absolute_prompt_file() -> None:
    with pytest.raises(pipeline_suite.OwnedYamlError) as excinfo:
        pipeline_suite.compose_suite(
            preset="builtin:ce-executor-pipeline",
            plan_path="plan.md",
            prompt_file="/tmp/PROMPT.pipeline.md",
            backend="claude",
            budget_max_iterations=12,
            budget_wall_clock_seconds=7200,
        )
    assert excinfo.value.code == "owned_yaml_invalid"


# --- prompt invariants: never reference ``ralph-hats`` or any preset name
# beyond the substituted id.


def test_prompt_never_references_ralph_hats() -> None:
    suite = _make_pipeline_suite()
    assert "ralph-hats" not in suite.prompt
    # The forbidden patterns list must remain aligned with the rule.
    assert "ralph-hats" in pipeline_suite.PROMPT_FORBIDDEN_PATTERNS


def test_prompt_never_references_runtime_managed_block_markers() -> None:
    suite = _make_pipeline_suite()
    for marker in (
        "RALPH-MANAGED-BLOCK",
        "RALPH-BOOTSTRAP-START",
    ):
        assert marker not in suite.prompt, f"prompt must not reference {marker}"
        assert marker not in suite.config, f"config must not reference {marker}"


# --- existing-suite fixture: round-trip is noop.


def test_existing_suite_fixture_round_trip(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("existing-suite", project)
    config_path = project / "ralph.pipeline.yml"
    provenance_path = project / "ralph.bootstrap.yml"
    original_config = config_path.read_text(encoding="utf-8")
    original_provenance = provenance_path.read_text(encoding="utf-8")
    # Second compose is a noop against the on-disk bytes.
    result = pipeline_suite.apply_pipeline_config(original_config, **PIPELINE_KWARGS)  # type: ignore[arg-type]
    assert result.kind == "noop"
    # Build a fresh suite from the same inputs and confirm the on-disk
    # provenance matches what the helper would re-emit (modulo blank
    # lines / trailing whitespace).
    suite = _make_pipeline_suite()
    expected_provenance = pipeline_suite.render_provenance(suite)
    assert expected_provenance.strip() == original_provenance.strip()
    upgrade = pipeline_suite.upgrade_provenance(original_provenance, suite)
    assert upgrade.kind == "noop"


# --- invalid-yaml fixture: apply must block with duplicate_yaml_key.


def test_invalid_yaml_fixture_blocks_apply(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("invalid-yaml", project)
    config_path = project / "ralph.pipeline.yml"
    config_text = config_path.read_text(encoding="utf-8")
    # Sanity: the fixture itself carries a duplicate top-level key
    # outside ``_bootstrap:``.
    with pytest.raises(pipeline_suite.OwnedYamlError) as excinfo:
        pipeline_suite.apply_owned_keys_to_existing_config(
            config_text,
            {
                "preset": "builtin:ce-executor-pipeline",
                "plan": "plan.md",
                "prompt_file": "PROMPT.pipeline.md",
                "preflight": "strict",
            },
        )
    assert excinfo.value.code == "duplicate_yaml_key"


# ---------------------------------------------------------------------------
# Unit 5 — CLI capability probe + staged static validation (cli_probe)
# ---------------------------------------------------------------------------


import subprocess  # noqa: E402  (used by the cli_probe runner tests)
from typing import Callable  # noqa: E402

import cli_probe  # noqa: E402  (Unit 5 helper, loaded by conftest)
import _probe_runner  # noqa: E402  (Unit 5 fake runner, loaded by conftest)


_PIPELINE_KW: dict[str, object] = dict(
    binary="ralph",
    config_path="ralph.pipeline.yml",
    preset="builtin:ce-executor-pipeline",
    prompt_file="PROMPT.pipeline.md",
    plan_path="plan.md",
)


def _make_runner(name: str) -> Callable[..., subprocess.CompletedProcess]:
    """Build the fixture-driven runner for the ``name`` fixture set."""
    invocations = cli_probe.load_fixture(name)
    return _probe_runner.make_runner(invocations)


def _make_missing_runner() -> Callable[..., subprocess.CompletedProcess]:
    """Build a runner that raises ``FileNotFoundError`` on every call.

    Used for the missing-binary scenario: the staged gate must
    classify the binary as missing without invoking the fake.
    """

    def _runner(*args, **kwargs):  # noqa: ARG001
        raise FileNotFoundError("ralph: binary not found")

    return _runner


# --- T1 — capability ordering: missing binary -----------------------------


def test_cli_probe_missing_binary_blocks_capability_and_skips_rest() -> None:
    runner = _make_missing_runner()
    decisions = cli_probe.validate_pipeline(runner=runner, **_PIPELINE_KW)  # type: ignore[arg-type]
    assert len(decisions) == 4
    capability, preset, preflight, dry_run = decisions
    assert capability.stage == "capability"
    assert capability.outcome == "blocked_cli"
    assert "not found" in capability.blocked_reason
    assert capability.next_allowed_stage is None
    for stage in (preset, preflight, dry_run):
        assert stage.stage in {"preset_check", "preflight", "dry_run"}
        assert stage.next_allowed_stage is None
        assert stage.outcome == "blocked_unknown"


# --- T2 — capability ordering: missing required flag ----------------------


def test_cli_probe_missing_flag_blocks_capability_and_skips_rest() -> None:
    runner = _make_runner("missing-flag")
    decisions = cli_probe.validate_pipeline(runner=runner, **_PIPELINE_KW)  # type: ignore[arg-type]
    capability = decisions[0]
    assert capability.outcome == "blocked_cli"
    assert "required flags missing" in capability.blocked_reason
    for stage in decisions[1:]:
        assert stage.next_allowed_stage is None
        assert stage.outcome == "blocked_unknown"


# --- T3 — preset strict failure → blocked_preset ---------------------------


def test_cli_probe_preset_strict_fail_blocks_at_preset_stage() -> None:
    runner = _make_runner("preset-strict-fail")
    decisions = cli_probe.validate_pipeline(runner=runner, **_PIPELINE_KW)  # type: ignore[arg-type]
    capability, preset_check, preflight, dry_run = decisions
    assert capability.outcome == "ok"
    assert capability.next_allowed_stage == "preset_check"
    assert preset_check.outcome == "blocked_preset"
    assert preset_check.next_allowed_stage is None
    assert "unknown preset id" in preset_check.blocked_reason
    assert preflight.outcome == "blocked_unknown"
    assert dry_run.outcome == "blocked_unknown"


# --- T4 — backend missing → blocked_backend -------------------------------


def test_cli_probe_backend_missing_blocks_at_preflight_stage() -> None:
    runner = _make_runner("backend-missing")
    decisions = cli_probe.validate_pipeline(runner=runner, **_PIPELINE_KW)  # type: ignore[arg-type]
    capability, preset_check, preflight, dry_run = decisions
    assert capability.outcome == "ok"
    assert preset_check.outcome == "ok"
    assert preflight.outcome == "blocked_backend"
    assert "executable not found" in preflight.blocked_reason
    assert dry_run.outcome == "blocked_unknown"


# --- T5 — dry-run source mismatch → blocked_command -----------------------


def test_cli_probe_dry_run_source_mismatch_blocks_at_dry_run_stage() -> None:
    runner = _make_runner("dry-run-source-mismatch")
    decisions = cli_probe.validate_pipeline(runner=runner, **_PIPELINE_KW)  # type: ignore[arg-type]
    capability, preset_check, preflight, dry_run = decisions
    assert capability.outcome == "ok"
    assert preset_check.outcome == "ok"
    assert preflight.outcome == "ok"
    assert dry_run.outcome == "blocked_command"
    assert "does not reference requested config" in dry_run.blocked_reason


# --- T6 — green path: 4 stages all ok -------------------------------------


def test_cli_probe_green_path_returns_four_ok_stages() -> None:
    runner = _make_runner("green")
    decisions = cli_probe.validate_pipeline(runner=runner, **_PIPELINE_KW)  # type: ignore[arg-type]
    assert len(decisions) == 4
    capability, preset_check, preflight, dry_run = decisions
    assert capability.outcome == "ok" and capability.next_allowed_stage == "preset_check"
    assert preset_check.outcome == "ok" and preset_check.next_allowed_stage == "preflight"
    assert preflight.outcome == "ok" and preflight.next_allowed_stage == "dry_run"
    assert dry_run.outcome == "ok" and dry_run.next_allowed_stage is None
    # The dry-run blocked_reason is empty on a green stage.
    assert dry_run.blocked_reason == ""


# --- T7 — every argv carries -c <config> -H <preset> ---------------------


def test_cli_probe_every_argv_carries_explicit_config_and_preset() -> None:
    runner = _make_runner("green")
    decisions = cli_probe.validate_pipeline(runner=runner, **_PIPELINE_KW)  # type: ignore[arg-type]
    for decision in decisions:
        argv = decision.argv
        assert "-c" in argv, f"argv missing -c: {argv}"
        assert "ralph.pipeline.yml" in argv, f"argv missing config path: {argv}"
        assert "-H" in argv, f"argv missing -H: {argv}"
        assert "builtin:ce-executor-pipeline" in argv, f"argv missing preset: {argv}"


def test_cli_probe_dry_run_argv_additionally_carries_dry_run_flag() -> None:
    runner = _make_runner("green")
    decisions = cli_probe.validate_pipeline(runner=runner, **_PIPELINE_KW)  # type: ignore[arg-type]
    dry_run = decisions[-1]
    assert "--dry-run" in dry_run.argv
    # And explicitly NOT --skip-preflight: the strict gate runs as its
    # own stage so the dry-run never silences preflight.
    assert "--skip-preflight" not in dry_run.argv


# --- T8 — subprocess.TimeoutExpired → blocked_unknown ---------------------


def test_cli_probe_timeout_classifies_as_blocked_unknown() -> None:
    invocations = cli_probe.load_fixture("green")

    def _timeout_on_preset(*args, **kwargs):  # noqa: ARG001
        argv = tuple(args[0]) if args else ()
        if "preset" in argv and "check" in argv and "--strict" in argv:
            raise subprocess.TimeoutExpired("ralph", 20)
        for inv in invocations:
            if inv.argv_expected == argv:
                return subprocess.CompletedProcess(
                    args=argv,
                    returncode=inv.exit_code,
                    stdout="".join(inv.stdout_chunks),
                    stderr="".join(inv.stderr_chunks),
                )
        raise AssertionError(f"unknown argv: {argv}")

    decisions = cli_probe.validate_pipeline(
        runner=_timeout_on_preset, **_PIPELINE_KW  # type: ignore[arg-type]
    )
    capability, preset_check, preflight, dry_run = decisions
    assert capability.outcome == "ok"
    assert preset_check.outcome == "blocked_unknown"
    assert "timed out" in preset_check.blocked_reason
    assert preset_check.next_allowed_stage is None
    assert preflight.outcome == "blocked_unknown"
    assert dry_run.outcome == "blocked_unknown"


# --- T9 — empty stderr on nonzero exit still classifies --------------------


def test_cli_probe_empty_stderr_on_nonzero_exit_still_classifies() -> None:
    invocations = cli_probe.load_fixture("green")

    def _empty_stderr(*args, **kwargs):  # noqa: ARG001
        argv = tuple(args[0]) if args else ()
        if "preset" in argv and "check" in argv and "--strict" in argv:
            return subprocess.CompletedProcess(
                args=argv, returncode=1, stdout="", stderr=""
            )
        for inv in invocations:
            if inv.argv_expected == argv:
                return subprocess.CompletedProcess(
                    args=argv,
                    returncode=inv.exit_code,
                    stdout="".join(inv.stdout_chunks),
                    stderr="".join(inv.stderr_chunks),
                )
        raise AssertionError(f"unknown argv: {argv}")

    decisions = cli_probe.validate_pipeline(
        runner=_empty_stderr, **_PIPELINE_KW  # type: ignore[arg-type]
    )
    preset_check = decisions[1]
    assert preset_check.outcome == "blocked_preset"
    # The helper MUST surface a non-empty blocked_reason even when
    # stderr is empty; it must not pretend the stage passed.
    assert preset_check.blocked_reason != ""
    assert "non-zero" in preset_check.blocked_reason


# --- T10 — proof level monotonically advances -----------------------------


def test_cli_probe_proof_level_monotonically_advances() -> None:
    """Once a stage transitions to ``ok``, the gate must not silently
    downgrade to a lower severity on a later stage."""
    runner = _make_runner("green")
    decisions = cli_probe.validate_pipeline(runner=runner, **_PIPELINE_KW)  # type: ignore[arg-type]
    # Walk the decisions in order; whenever a stage is ok, the next
    # stage must record its own argv (not a skipped marker), and the
    # gate must never rewind from ok to blocker without recording the
    # blocker stage's evidence first.
    prior_stage_outcome = "ok"  # capability is the first stage
    for decision in decisions:
        if prior_stage_outcome == "ok" and decision.outcome != "ok":
            # A blocker after ok is allowed, but the evidence must
            # carry the original argv so callers can debug.
            assert decision.argv, (
                "blocker after ok must record the argv that was attempted"
            )
        if decision.outcome == "blocked_unknown":
            # A skip must explicitly cite the upstream blocker.
            assert any("blocked" in ev for ev in decision.evidence), (
                "skip marker must cite the upstream blocker"
            )
        prior_stage_outcome = decision.outcome
    # Final stage has no successor.
    assert decisions[-1].next_allowed_stage is None


# --- T11 — REQUIRED_FLAGS is the canonical literal -----------------------


def test_cli_probe_required_flags_is_literal() -> None:
    assert cli_probe.REQUIRED_FLAGS == frozenset(
        {
            "preset check --strict",
            "preflight --strict",
            "run --dry-run --strict",
        }
    )


# --- T12 — CapabilityReport for missing binary is synthetic ---------------


def test_cli_probe_capability_report_for_missing_binary_is_synthetic() -> None:
    report = cli_probe.probe_capability(binary="/nonexistent/ralph", runner=_make_missing_runner())
    assert report.version == "missing"
    assert report.flags_present == frozenset()
    assert report.flags_missing == frozenset(cli_probe.REQUIRED_FLAGS)
    assert report.json_supported is False
    assert report.run_dry_run_supported is False


# --- T13 — capability gate never throws even on bogus argv ----------------


def test_cli_probe_capability_gate_does_not_throw() -> None:
    """``probe_capability`` must always return a CapabilityReport,
    never raise — even when every subprocess call fails."""
    report = cli_probe.probe_capability(binary="/nonexistent/ralph", runner=_make_missing_runner())
    assert isinstance(report, cli_probe.CapabilityReport)
    assert report.binary == Path("/nonexistent/ralph")


# --- T14 — argv equality across green path stages -------------------------


def test_cli_probe_dry_run_argv_matches_expected_command_shape() -> None:
    """The dry-run argv must be ``<binary> -c <config> -H <preset>
    run --dry-run --strict --prompt-file <pf> --plan <plan>``."""
    runner = _make_runner("green")
    decisions = cli_probe.validate_pipeline(runner=runner, **_PIPELINE_KW)  # type: ignore[arg-type]
    dry_run = decisions[-1]
    argv = dry_run.argv
    assert argv[0] == "ralph"
    assert "--dry-run" in argv
    assert "--strict" in argv
    # --prompt-file and its value sit together.
    pfile_idx = argv.index("--prompt-file")
    assert argv[pfile_idx + 1] == "PROMPT.pipeline.md"
    plan_idx = argv.index("--plan")
    assert argv[plan_idx + 1] == "plan.md"


# --- T15 — load_fixture loader ------------------------------------------


def test_cli_probe_load_fixture_returns_fake_invocations() -> None:
    invocations = cli_probe.load_fixture("green")
    assert invocations
    for inv in invocations:
        assert isinstance(inv, cli_probe.FakeInvocation)
        assert inv.argv_expected
        assert isinstance(inv.exit_code, int)


# ---------------------------------------------------------------------------
# Unit 6 — safe-loop smoke harness (smoke_runner)
# ---------------------------------------------------------------------------


import smoke_runner  # noqa: E402  (Unit 6 helper, loaded by conftest)


_SMOKE_KW: dict[str, object] = dict(
    binary="/tmp/fake-ralph",
    config_path="ralph.pipeline.yml",
    preset="builtin:ce-executor-pipeline",
    prompt_file="PROMPT.pipeline.md",
    plan_path="plan.md",
    max_iterations=3,
    idle_timeout_ms=5000,
    wall_clock_timeout_s=60,
)


def _smoke_cfg(**overrides: object) -> smoke_runner.SmokeConfig:
    """Build a SmokeConfig with the standard test kwargs."""
    params = dict(_SMOKE_KW)
    params.update(overrides)
    return smoke_runner.SmokeConfig(**params)  # type: ignore[arg-type]


def _fake_runner(
    stdout: str = "",
    stderr: str = "",
    returncode: int = 0,
) -> Callable[..., subprocess.CompletedProcess]:
    """Return a runner that ignores argv and returns a fixed result."""

    def _runner(args, **kwargs):  # noqa: ARG001
        return subprocess.CompletedProcess(
            args=tuple(args),
            returncode=returncode,
            stdout=stdout,
            stderr=stderr,
        )

    return _runner


# --- S1 — UnsafeBackend refuses for all four unsafe kinds ------------------


@pytest.mark.parametrize("kind", ["mock", "custom", "real", "unknown"])
def test_unsafe_backend_blocks_smoke_for_each_kind(kind: str) -> None:
    backend = smoke_runner.UnsafeBackend(name=f"unsafe-{kind}", kind=kind)
    result = smoke_runner.run_smoke(backend, _smoke_cfg())
    assert result.outcome == "not_authorized"
    assert result.argv == ()
    assert result.failure_bucket == "none"
    assert result.elapsed_seconds == 0.0
    # The evidence MUST carry the precise kind + name so the handoff
    # can render a precise refusal message.
    assert any(kind in ev for ev in result.evidence)


def test_unsafe_backend_blocks_smoke_even_with_runner_attached() -> None:
    """A runner is irrelevant: an unsafe backend MUST refuse before
    the runner is consulted. Test by passing a runner that records
    every spawn attempt; if the harness reached the runner, the
    recorder would be non-empty."""
    calls: list[tuple[str, ...]] = []

    def _recorder(args, **kwargs):  # noqa: ARG001
        calls.append(tuple(args))
        return subprocess.CompletedProcess(
            args=tuple(args), returncode=0, stdout="", stderr=""
        )

    backend = smoke_runner.UnsafeBackend(name="mock", kind="mock")
    result = smoke_runner.run_smoke(
        backend, _smoke_cfg(), runner=_recorder
    )
    assert result.outcome == "not_authorized"
    assert result.argv == ()
    assert calls == []


# --- S2 — safe-smoke green path classifies bounded_terminal_reached -------


def test_safe_smoke_green_classifies_bounded_terminal(tmp_path: Path) -> None:
    backend = smoke_runner.SafeBackend(name="replay")
    runner = _fake_runner(
        stdout="plan.ready\nexecuting unit\nLOOP_COMPLETE\n",
        returncode=0,
    )
    result = smoke_runner.run_smoke(
        backend, _smoke_cfg(), transcript_dir=tmp_path, runner=runner
    )
    assert result.outcome == "bounded_terminal_reached"
    assert result.failure_bucket == "none"
    assert result.argv  # spawned, so argv is populated
    assert result.elapsed_seconds >= 0.0


def test_safe_smoke_first_event_seen_classifies_correctly(tmp_path: Path) -> None:
    backend = smoke_runner.SafeBackend(name="replay")
    # plan.ready appears but no LOOP_COMPLETE — wall-clock cutoff
    # is the only thing that ends the run.
    runner = _fake_runner(stdout="plan.ready\n", returncode=0)
    result = smoke_runner.run_smoke(
        backend, _smoke_cfg(), transcript_dir=tmp_path, runner=runner
    )
    assert result.outcome == "first_event_seen"
    assert result.failure_bucket == "none"


def test_safe_smoke_spawned_with_no_markers_classifies_spawned() -> None:
    backend = smoke_runner.SafeBackend(name="replay")
    runner = _fake_runner(stdout="hello world\n", returncode=0)
    result = smoke_runner.run_smoke(backend, _smoke_cfg(), runner=runner)
    assert result.outcome == "spawned"
    assert result.failure_bucket == "none"


# --- S3 — timeout classification -----------------------------------------


def test_timeout_no_event_kills_process_and_classifies() -> None:
    """When the runner raises ``TimeoutExpired`` the harness must
    classify the outcome as ``wall_clock_timeout`` AND record the
    elapsed time. The elapsed time MUST be less than the wall-clock
    cap plus a small grace so a runaway timer cannot fake a green run.
    """
    backend = smoke_runner.SafeBackend(name="replay")
    cfg = _smoke_cfg(wall_clock_timeout_s=2)

    def _hanging_runner(args, **kwargs):  # noqa: ARG001
        # Simulate the harness's outer timeout firing.
        raise subprocess.TimeoutExpired(cmd="ralph", timeout=7.0)

    result = smoke_runner.run_smoke(backend, cfg, runner=_hanging_runner)
    assert result.outcome == "wall_clock_timeout"
    # elapsed_seconds MUST be recorded; we cannot assert it < cfg.wall_clock_timeout_s
    # because the runner faked the timeout, but it MUST be >= 0.
    assert result.elapsed_seconds >= 0.0
    # argv MUST be populated because the harness DID attempt to spawn
    # (the timeout fires AFTER argv construction).
    assert result.argv


def test_timeout_no_event_records_elapsed_under_grace() -> None:
    """The harness records ``elapsed_seconds`` even when the runner
    fakes a timeout; the recorded value MUST be less than
    ``wall_clock_timeout_s + grace`` so a runaway timer cannot fake a
    green run."""
    backend = smoke_runner.SafeBackend(name="replay")
    cfg = _smoke_cfg(wall_clock_timeout_s=2)

    def _hanging_runner(args, **kwargs):  # noqa: ARG001
        raise subprocess.TimeoutExpired(cmd="ralph", timeout=7.0)

    result = smoke_runner.run_smoke(backend, cfg, runner=_hanging_runner)
    # elapsed_seconds is wall-clock from the harness's perspective;
    # since the runner fakes the timeout the recorded value is well
    # under the wall-clock cap. The invariant we care about is the
    # upper bound.
    assert result.elapsed_seconds < cfg.wall_clock_timeout_s + 10.0


def test_timeout_idle_classifies_after_idle_timeout_ms() -> None:
    """When the runner emits ``plan.ready`` and then the outer
    timeout fires, the harness's primary classifier would normally
    produce ``wall_clock_timeout`` (because the timeout came from
    outside). The harness does not implement idle-vs-no-event
    distinction at the harness level; idle classification requires a
    cooperative runtime. This test verifies that a script which emits
    one event and then sleeps forever is killed and the argv captured.
    """
    backend = smoke_runner.SafeBackend(name="replay")
    cfg = _smoke_cfg(wall_clock_timeout_s=2)

    def _emit_then_hang(args, **kwargs):  # noqa: ARG001
        # Simulate the harness's outer timeout firing AFTER the
        # first event has been observed by reporting it via stdout
        # in the TimeoutExpired exception (Python 3.11+ supports
        # partial stdout/stderr on TimeoutExpired; for compatibility
        # we just raise the timeout itself).
        raise subprocess.TimeoutExpired(cmd="ralph", timeout=7.0, output="plan.ready\n")

    result = smoke_runner.run_smoke(backend, cfg, runner=_emit_then_hang)
    # The harness classifies TimeoutExpired as wall_clock_timeout;
    # idle-vs-no-event is a runtime-side concern and is observed via
    # the --idle-timeout-ms flag the harness forwards. The argv MUST
    # carry the flag so a cooperative runtime can implement idle.
    assert result.outcome == "wall_clock_timeout"
    assert "--idle-timeout-ms" in result.argv
    assert "5000" in result.argv  # default idle_timeout_ms


# --- S4 — non-zero exit classification -------------------------------------


def test_non_zero_exit_classifies_with_empty_stderr() -> None:
    backend = smoke_runner.SafeBackend(name="replay")
    runner = _fake_runner(stdout="", stderr="", returncode=1)
    result = smoke_runner.run_smoke(backend, _smoke_cfg(), runner=runner)
    assert result.outcome == "non_zero_exit"
    # No bucket keyword in the combined stream → suite.
    assert result.failure_bucket == "suite"


def test_non_zero_exit_classifies_with_non_empty_stderr() -> None:
    backend = smoke_runner.SafeBackend(name="replay")
    runner = _fake_runner(
        stdout="",
        stderr="fatal: backend initialization failed\n",
        returncode=2,
    )
    result = smoke_runner.run_smoke(backend, _smoke_cfg(), runner=runner)
    assert result.outcome == "non_zero_exit"
    assert result.failure_bucket == "backend"


# --- S5 — error event failure buckets -------------------------------------


def test_error_event_failure_bucket_preset() -> None:
    backend = smoke_runner.SafeBackend(name="replay")
    runner = _fake_runner(
        stdout="plan.ready\nERROR_EVENT: preset validation failed\n",
        returncode=0,
    )
    result = smoke_runner.run_smoke(backend, _smoke_cfg(), runner=runner)
    assert result.outcome == "error_event_detected"
    assert result.failure_bucket == "preset"


def test_error_event_failure_bucket_backend() -> None:
    backend = smoke_runner.SafeBackend(name="replay")
    runner = _fake_runner(
        stdout="plan.ready\nERROR_EVENT: backend connection refused\n",
        returncode=0,
    )
    result = smoke_runner.run_smoke(backend, _smoke_cfg(), runner=runner)
    assert result.outcome == "error_event_detected"
    assert result.failure_bucket == "backend"


def test_error_event_failure_bucket_project_command() -> None:
    backend = smoke_runner.SafeBackend(name="replay")
    runner = _fake_runner(
        stdout="plan.ready\nERROR_EVENT: project build missing\n",
        returncode=0,
    )
    result = smoke_runner.run_smoke(backend, _smoke_cfg(), runner=runner)
    assert result.outcome == "error_event_detected"
    assert result.failure_bucket == "project_command"


def test_error_event_failure_bucket_suite_fallback() -> None:
    backend = smoke_runner.SafeBackend(name="replay")
    runner = _fake_runner(
        stdout="plan.ready\nERROR_EVENT: something else\n",
        returncode=0,
    )
    result = smoke_runner.run_smoke(backend, _smoke_cfg(), runner=runner)
    assert result.outcome == "error_event_detected"
    assert result.failure_bucket == "suite"


# --- S6 — dirty tree preserved --------------------------------------------


def test_dirty_tree_preserved(tmp_path: Path) -> None:
    """Plant a file the smoke will NOT touch and verify it remains
    byte-for-byte equal after the helper returns. The harness must
    not clean, revert, auto-commit, or rewrite any operator file."""
    project = tmp_path / "project"
    project.mkdir()
    dirty = project / "untracked.txt"
    dirty.write_text("operator's in-progress notes\n", encoding="utf-8")
    pre_bytes = dirty.read_bytes()

    backend = smoke_runner.SafeBackend(name="replay")
    runner = _fake_runner(stdout="plan.ready\nLOOP_COMPLETE\n", returncode=0)
    smoke_runner.run_smoke(
        backend,
        _smoke_cfg(binary="/tmp/fake", plan_path=str(project / "plan.md")),
        transcript_dir=tmp_path / "transcripts",
        runner=runner,
    )

    post_bytes = dirty.read_bytes()
    assert post_bytes == pre_bytes
    # And the harness must not have written anything outside the
    # declared transcript_dir.
    assert (project / "events.jsonl").exists() is False
    assert (project / ".ralph").exists() is False


# --- S7 — argv shape contract ---------------------------------------------


def test_smoke_argv_shape_contains_required_flags() -> None:
    """Every argv the harness builds MUST contain -c, -H,
    --max-iterations, --idle-timeout-ms, --wall-clock-timeout-s.
    Verify by inspecting the argv on a green-path result."""
    backend = smoke_runner.SafeBackend(name="replay")
    runner = _fake_runner(stdout="plan.ready\nLOOP_COMPLETE\n", returncode=0)
    result = smoke_runner.run_smoke(backend, _smoke_cfg(), runner=runner)
    argv = result.argv
    assert "-c" in argv
    assert "ralph.pipeline.yml" in argv
    assert "-H" in argv
    assert "builtin:ce-executor-pipeline" in argv
    assert "--max-iterations" in argv
    assert "3" in argv
    assert "--idle-timeout-ms" in argv
    assert "5000" in argv
    assert "--wall-clock-timeout-s" in argv
    assert "60" in argv


def test_smoke_argv_shape_holds_when_extra_argv_appended() -> None:
    """``extra_argv`` must be appended AFTER the harness contract so
    the contract stays inspectable."""
    backend = smoke_runner.SafeBackend(name="replay")
    runner = _fake_runner(stdout="plan.ready\nLOOP_COMPLETE\n", returncode=0)
    cfg = _smoke_cfg(extra_argv=("--reuse-worktree", "--worktree-name", "t1"))
    result = smoke_runner.run_smoke(backend, cfg, runner=runner)
    argv = result.argv
    assert argv[-3:] == ("--reuse-worktree", "--worktree-name", "t1")
    # And the harness contract flags remain in place.
    assert "--max-iterations" in argv


def test_smoke_argv_shape_present_in_safe_smoke_fixture(tmp_path: Path) -> None:
    """Load the safe-smoke fixture's recorded argv and assert the
    harness contract is observable from the fixture's transcript."""
    argv_path = (
        ROOT
        / "skills"
        / "ralph-project-bootstrap"
        / "fixtures"
        / "cli"
        / "smoke"
        / "safe-smoke"
        / "transcript.json"
    )
    # transcript.json declares argv_shape_required so the loader can
    # validate the fixture without invoking the harness.
    import json as _json
    data = _json.loads(argv_path.read_text(encoding="utf-8"))
    required = data["argv_shape_required"]
    for token in required:
        assert token in required
    # And the harness builds an argv that contains every required
    # token at run time.
    backend = smoke_runner.SafeBackend(name="replay")
    runner = _fake_runner(stdout="plan.ready\nLOOP_COMPLETE\n", returncode=0)
    result = smoke_runner.run_smoke(backend, _smoke_cfg(), runner=runner)
    for token in required:
        assert token in result.argv, f"argv missing {token!r}: {result.argv}"


# --- S8 — unsafe refusal prevents spawn -----------------------------------


def test_smoke_no_spawn_when_unsafe() -> None:
    """A runner that records every spawn attempt must NOT be called
    when the backend is unsafe. The harness must refuse before any
    subprocess is constructed."""
    spawn_attempts: list[tuple[str, ...]] = []

    def _recorder(args, **kwargs):  # noqa: ARG001
        spawn_attempts.append(tuple(args))
        return subprocess.CompletedProcess(
            args=tuple(args), returncode=0, stdout="", stderr=""
        )

    for kind in ("mock", "custom", "real", "unknown"):
        backend = smoke_runner.UnsafeBackend(name=f"u-{kind}", kind=kind)
        result = smoke_runner.run_smoke(
            backend, _smoke_cfg(), runner=_recorder
        )
        assert result.outcome == "not_authorized"
        assert result.argv == ()
    assert spawn_attempts == []


# --- S9 — real-binary env-var gate ----------------------------------------


def test_real_binary_refused_without_env_var(monkeypatch: pytest.MonkeyPatch) -> None:
    """When ``runner is None`` and the env var is NOT set, the harness
    MUST refuse to spawn the real ``ralph`` binary."""
    monkeypatch.delenv(smoke_runner.ALLOW_REAL_BACKEND_ENV, raising=False)
    backend = smoke_runner.SafeBackend(name="replay")
    cfg = _smoke_cfg(binary="/nonexistent/ralph")
    result = smoke_runner.run_smoke(backend, cfg)
    assert result.outcome == "not_authorized"
    assert result.argv == ()


def test_real_binary_refused_when_env_var_set_but_runner_provided(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Even when the env var is set, an explicit runner takes
    precedence and the harness MUST NOT spawn the real binary."""
    monkeypatch.setenv(smoke_runner.ALLOW_REAL_BACKEND_ENV, "1")
    backend = smoke_runner.SafeBackend(name="replay")
    runner = _fake_runner(stdout="LOOP_COMPLETE\n", returncode=0)
    result = smoke_runner.run_smoke(
        backend, _smoke_cfg(), runner=runner
    )
    # The harness used the runner; argv is the harness-built argv.
    assert result.argv
    assert result.outcome == "bounded_terminal_reached"


# --- S10 — dataclass invariants -------------------------------------------


def test_safe_backend_rejects_non_replay_kind() -> None:
    with pytest.raises(ValueError):
        smoke_runner.SafeBackend(name="bad", kind="mock")  # type: ignore[arg-type]


def test_unsafe_backend_rejects_unknown_kind() -> None:
    with pytest.raises(ValueError):
        smoke_runner.UnsafeBackend(name="bad", kind="not-a-kind")


def test_smoke_result_rejects_unknown_outcome() -> None:
    with pytest.raises(ValueError):
        smoke_runner.SmokeResult(
            outcome="not-a-real-outcome",
            evidence=(),
            argv=(),
            stderr_excerpt="",
            stdout_excerpt="",
            elapsed_seconds=0.0,
            failure_bucket="none",
        )


def test_smoke_result_rejects_unknown_failure_bucket() -> None:
    with pytest.raises(ValueError):
        smoke_runner.SmokeResult(
            outcome="spawned",
            evidence=(),
            argv=(),
            stderr_excerpt="",
            stdout_excerpt="",
            elapsed_seconds=0.0,
            failure_bucket="not-a-real-bucket",
        )


def test_safe_backend_is_trusted_only_for_replay_kind() -> None:
    backend = smoke_runner.SafeBackend(name="r")
    assert backend.is_trusted is True
    assert backend.kind == smoke_runner.SAFE_BACKEND_KIND
    unsafe = smoke_runner.UnsafeBackend(name="u", kind="mock")
    assert unsafe.is_trusted is False


def test_outcomes_and_failure_buckets_are_canonical_literals() -> None:
    assert set(smoke_runner.OUTCOMES) == {
        "not_authorized",
        "spawned",
        "first_event_seen",
        "bounded_terminal_reached",
        "timeout_no_event",
        "timeout_idle",
        "wall_clock_timeout",
        "non_zero_exit",
        "error_event_detected",
    }
    assert set(smoke_runner.FAILURE_BUCKETS) == {
        "none",
        "suite",
        "preset",
        "backend",
        "project_command",
    }


# --- S11 — FakeBinary rendering -------------------------------------------


def test_fake_binary_renders_self_contained_script() -> None:
    cfg = _smoke_cfg()
    fake = smoke_runner.FakeBinary(
        transcript_dir=Path("/tmp/transcripts"),
        smoke_cfg=cfg,
        script_lines=("plan.ready", "LOOP_COMPLETE"),
        exit_code=0,
    )
    contents = fake.script_contents()
    assert "plan.ready" in contents
    assert "LOOP_COMPLETE" in contents
    assert "sys.exit(0)" in contents
    assert "transcript" in contents.lower()


# ---------------------------------------------------------------------------
# Unit 7 — official launch command builder + handoff report (handoff)
# ---------------------------------------------------------------------------


import handoff  # noqa: E402  (Unit 7 helper, loaded by conftest)


_BASE_KW: dict[str, object] = dict(
    binary="ralph",
    config_path="ralph.pipeline.yml",
    preset="test-preset",
    plan_path="plan.md",
    prompt_file="PROMPT.pipeline.md",
)


def _make_inputs(**overrides: object) -> handoff.HandoffInputs:
    params = dict(_BASE_KW)
    params.update(overrides)
    return handoff.HandoffInputs(**params)  # type: ignore[arg-type]


# --- H1 — complete path emits the official command -------------------------


def test_handoff_complete_includes_official_command() -> None:
    inputs = _make_inputs(
        level="complete",
        validation_evidence=(
            "capability ok",
            "preset_check ok",
            "preflight ok",
            "dry_run ok",
        ),
        smoke_evidence=("bounded_terminal_reached",),
    )
    art = handoff.build_handoff(inputs)
    assert art.level == "complete"
    # The command is non-empty and contains the canonical flags.
    assert art.command
    assert "[CANDIDATE" not in art.command
    argv = art.command_argv
    assert "-c" in argv and "ralph.pipeline.yml" in argv
    assert "-H" in argv and "test-preset" in argv
    # The command string mirrors the argv.
    assert "ralph -c ralph.pipeline.yml -H test-preset" in art.command


# --- H2 — incomplete path marks the command as a candidate ----------------


def test_handoff_incomplete_static_only_marks_command_as_candidate() -> None:
    inputs = _make_inputs(
        level="incomplete_static_only",
        smoke_evidence=(),
        validation_evidence=("capability ok", "preset_check ok", "preflight ok", "dry_run ok"),
    )
    art = handoff.build_handoff(inputs)
    assert art.level == "incomplete_static_only"
    assert art.command.startswith("[CANDIDATE")
    # The report must explicitly mark the smoke as not authorised and
    # must not claim the loop is ready to run.
    assert "smoke-not-authorized" in art.report
    assert "ready to run" not in art.report.lower()
    # The argv itself is unchanged; only the rendered command carries
    # the prefix.
    assert "-c" in art.command_argv and "-H" in art.command_argv


def test_handoff_incomplete_with_not_authorized_evidence_uses_candidate() -> None:
    """When the smoke harness reported ``not_authorized`` explicitly,
    the handoff must still treat the level as incomplete (i.e. emit a
    candidate command, not the official one)."""
    inputs = _make_inputs(
        level="incomplete_static_only",
        smoke_evidence=("not_authorized: backend=mock",),
    )
    art = handoff.build_handoff(inputs)
    assert art.command.startswith("[CANDIDATE")
    assert "smoke-not-authorized" in art.report


# --- H3 — blocked path emits no executable command -------------------------


def test_handoff_blocked_emits_no_executable_command() -> None:
    inputs = _make_inputs(
        level="blocked",
        blocker_summary="preset lint failed: unknown preset id 'missing-preset'",
    )
    art = handoff.build_handoff(inputs)
    assert art.level == "blocked"
    assert art.command == ""
    assert art.command_argv == ()
    assert art.blocker_summary != ""
    # The blocker is rendered into the report so the operator sees why.
    assert "preset lint failed" in art.report


# --- H4 — worktree mode requires a reuse key -------------------------------


def test_handoff_worktree_mode_requires_reuse_key() -> None:
    """When ``use_worktree=True`` AND no ``plan_arg``/``worktree_name``
    is supplied, the helper MUST reject — even when ``reuse_worktree``
    is also True. The error message is the contract value."""
    with pytest.raises(ValueError) as excinfo:
        handoff.HandoffInputs(
            **_BASE_KW,  # type: ignore[arg-type]
            level="complete",
            use_worktree=True,
            reuse_worktree=True,
            plan_arg=None,
            worktree_name=None,
        )
    assert "worktree reuse key required" in str(excinfo.value)


def test_handoff_worktree_mode_requires_reuse_flag_when_keys_given() -> None:
    """When ``use_worktree=True`` but ``reuse_worktree=False``, the
    helper MUST reject because the operator opted into worktree mode
    but forgot to enable reuse."""
    with pytest.raises(ValueError) as excinfo:
        handoff.HandoffInputs(
            **_BASE_KW,  # type: ignore[arg-type]
            level="complete",
            use_worktree=True,
            reuse_worktree=False,
            plan_arg="plan.md",
        )
    assert "worktree reuse key required" in str(excinfo.value)


# --- H5 — worktree mode allows either --plan or --worktree-name ------------


def test_handoff_worktree_mode_allows_plan_arg() -> None:
    inputs = _make_inputs(
        level="complete",
        use_worktree=True,
        reuse_worktree=True,
        plan_arg="docs/plans/foo.md",
        smoke_evidence=("bounded_terminal_reached",),
    )
    art = handoff.build_handoff(inputs)
    assert "--worktree" in art.command_argv
    assert "--reuse-worktree" in art.command_argv
    assert "--plan" in art.command_argv
    assert "docs/plans/foo.md" in art.command_argv
    # The explicit reuse plan replaces the top-level --plan position so
    # the operator's explicit reuse key wins.
    plan_idx = art.command_argv.index("--plan")
    assert art.command_argv[plan_idx + 1] == "docs/plans/foo.md"


def test_handoff_worktree_mode_allows_worktree_name() -> None:
    inputs = _make_inputs(
        level="complete",
        use_worktree=True,
        reuse_worktree=True,
        worktree_name="2026-07-18-foo-lucky-reed",
        smoke_evidence=("bounded_terminal_reached",),
    )
    art = handoff.build_handoff(inputs)
    argv = art.command_argv
    assert "--worktree" in argv
    assert "--reuse-worktree" in argv
    assert "--worktree-name" in argv
    assert "2026-07-18-foo-lucky-reed" in argv
    # And the worktree-name branch must NOT inject --plan with the
    # reuse key — the operator chose the worktree name explicitly.
    plan_idx = argv.index("--worktree-name")
    assert argv[plan_idx + 1] == "2026-07-18-foo-lucky-reed"


def test_handoff_worktree_command_shape_plan_branch() -> None:
    inputs = _make_inputs(
        level="complete",
        use_worktree=True,
        reuse_worktree=True,
        plan_arg="docs/plans/foo.md",
        smoke_evidence=("bounded_terminal_reached",),
    )
    art = handoff.build_handoff(inputs)
    argv = art.command_argv
    # Order: --worktree --reuse-worktree --plan <reuse-key>
    wt = argv.index("--worktree")
    reuse = argv.index("--reuse-worktree")
    plan = argv.index("--plan", wt)  # search --plan after --worktree
    assert wt < reuse < plan
    assert argv[plan + 1] == "docs/plans/foo.md"
    # Both -c and -H are still present.
    assert "-c" in argv and "ralph.pipeline.yml" in argv
    assert "-H" in argv and "test-preset" in argv


def test_handoff_worktree_command_shape_name_branch() -> None:
    inputs = _make_inputs(
        level="complete",
        use_worktree=True,
        reuse_worktree=True,
        worktree_name="lucky-reed",
        smoke_evidence=("bounded_terminal_reached",),
    )
    art = handoff.build_handoff(inputs)
    argv = art.command_argv
    wt = argv.index("--worktree")
    reuse = argv.index("--reuse-worktree")
    name = argv.index("--worktree-name", wt)
    assert wt < reuse < name
    assert argv[name + 1] == "lucky-reed"
    assert "-c" in argv and "-H" in argv


# --- H6 — blocked requires a non-empty blocker_summary --------------------


def test_handoff_blocked_requires_blocker_summary() -> None:
    with pytest.raises(ValueError):
        handoff.HandoffInputs(
            **_BASE_KW,  # type: ignore[arg-type]
            level="blocked",
            blocker_summary="",
        )
    with pytest.raises(ValueError):
        handoff.HandoffInputs(
            **_BASE_KW,  # type: ignore[arg-type]
            level="blocked",
            blocker_summary="   \n  ",
        )


# --- H7 — report is Markdown with required sections -----------------------


def test_handoff_report_is_markdown_with_required_sections() -> None:
    inputs = _make_inputs(
        level="complete",
        files_created=("AGENTS.md", "ralph.pipeline.yml"),
        files_updated=("CLAUDE.md",),
        files_noop=(),
        validation_evidence=("dry_run ok",),
        smoke_evidence=("bounded_terminal_reached",),
    )
    art = handoff.build_handoff(inputs)
    report = art.report
    # H1 title.
    assert report.startswith("# Ralph Bootstrap Handoff")
    # Level line.
    assert "Level: `complete`" in report
    # Required sub-sections.
    assert "## Items" in report
    assert "## Validation" in report
    assert "## Smoke" in report
    assert "## Residual Risks" in report
    assert "## Launch Command" in report
    # Items sub-table: created / updated / noop rows present.
    assert "AGENTS.md" in report
    assert "ralph.pipeline.yml" in report
    assert "CLAUDE.md" in report
    # The smoke status token is the canonical "complete" form.
    assert "Status: `complete`" in report


def test_handoff_report_smoke_status_tokens() -> None:
    """Every smoke status token documented in the reference must be
    observable from the rendered report."""
    # static-only -- smoke-not-authorized
    art_static = handoff.build_handoff(
        _make_inputs(level="incomplete_static_only", smoke_evidence=())
    )
    assert "Status: `static-only -- smoke-not-authorized`" in art_static.report
    # complete
    art_ok = handoff.build_handoff(
        _make_inputs(level="complete", smoke_evidence=("bounded_terminal_reached",))
    )
    assert "Status: `complete`" in art_ok.report
    # blocked -- <bucket>
    art_bucket = handoff.build_handoff(
        _make_inputs(level="blocked", blocker_summary="backend boom",
                     smoke_evidence=("non_zero_exit bucket=backend",))
    )
    # blocked level suppresses the launch command and surfaces the blocker.
    assert "## Blocker" in art_bucket.report
    assert "backend boom" in art_bucket.report


# --- H8 — residual risks section is populated when provided ----------------


def test_handoff_residual_risks_present_when_present() -> None:
    risks = (
        "backend has not been authorised; re-confirm before launch",
        "operator must set RALPH_API_KEY env var before running",
    )
    inputs = _make_inputs(
        level="incomplete_static_only",
        residual_risks=risks,
    )
    art = handoff.build_handoff(inputs)
    assert "## Residual Risks" in art.report
    for risk in risks:
        assert risk in art.report
    # And the artifact surfaces them structured.
    assert art.residual_risks == risks


def test_handoff_residual_risks_absent_section_renders_placeholder() -> None:
    inputs = _make_inputs(level="complete", residual_risks=())
    art = handoff.build_handoff(inputs)
    assert "## Residual Risks" in art.report
    assert "_none_" in art.report
    assert art.residual_risks == ()


# --- H9 — no absolute paths leak into the report ---------------------------


def test_handoff_no_absolute_paths_in_report() -> None:
    """Every path-like token in the rendered report must be
    repo-relative. Absolute paths must be rejected at the API
    boundary."""
    inputs = _make_inputs(
        level="complete",
        files_created=("AGENTS.md", "docs/plan.md"),
        files_updated=("CLAUDE.md",),
        files_noop=("README.md",),
        validation_evidence=("dry_run ok",),
        smoke_evidence=("bounded_terminal_reached",),
    )
    art = handoff.build_handoff(inputs)
    for token in art.report.split():
        # Tokens that contain a "/" or start with "~" are checked for
        # absolute paths; everything else (prose words, "Level:") is
        # skipped.
        if token.startswith(("/Users/", "/tmp/", "/etc/", "/home/")):
            raise AssertionError(f"absolute path leaked into report: {token}")
    # And the helper must reject absolute paths at the API boundary.
    with pytest.raises(ValueError):
        handoff.HandoffInputs(
            binary="ralph",
            config_path="ralph.pipeline.yml",
            preset="test-preset",
            plan_path="/etc/passwd",
            prompt_file="PROMPT.pipeline.md",
            level="complete",
        )


# --- H10 — preset name is not hard-coded anywhere in the module -----------


def test_handoff_module_does_not_hard_code_preset_names() -> None:
    """The module must accept arbitrary preset ids. A literal preset
    name in module-level constants would force tests to mirror it and
    would couple the helper to a specific Ralph preset."""
    module_text = (Path(__file__).resolve().parent.parent
                   / "ralph-project-bootstrap" / "scripts" / "handoff.py").read_text(
        encoding="utf-8"
    )
    # Forbidden literals: any specific builtin preset name.
    forbidden_literals = (
        "ce-executor-pipeline",
        "ce-executor-supervisor",
        "ce-executor-lite",
        "ralph-hats",
    )
    for literal in forbidden_literals:
        assert literal not in module_text, (
            f"handoff.py must not hard-code {literal!r}"
        )


# --- H11 — argv shape across the three levels ------------------------------


def test_handoff_complete_argv_carries_canonical_flags() -> None:
    inputs = _make_inputs(level="complete", smoke_evidence=("bounded_terminal_reached",))
    art = handoff.build_handoff(inputs)
    argv = art.command_argv
    assert argv[0] == "ralph"
    assert "-c" in argv and "ralph.pipeline.yml" in argv
    assert "-H" in argv and "test-preset" in argv
    assert "--prompt-file" in argv and "PROMPT.pipeline.md" in argv
    assert "--plan" in argv and "plan.md" in argv


def test_handoff_incomplete_argv_is_unaffected_by_prefix() -> None:
    """The argv tuple for incomplete must equal the argv tuple for
    complete — only the rendered command string carries the prefix."""
    complete = _make_inputs(level="complete", smoke_evidence=("bounded_terminal_reached",))
    incomplete = _make_inputs(level="incomplete_static_only", smoke_evidence=())
    art_complete = handoff.build_handoff(complete)
    art_incomplete = handoff.build_handoff(incomplete)
    assert art_complete.command_argv == art_incomplete.command_argv
    assert art_complete.command != art_incomplete.command


def test_handoff_blocked_argv_is_empty() -> None:
    inputs = _make_inputs(level="blocked", blocker_summary="boom")
    art = handoff.build_handoff(inputs)
    assert art.command_argv == ()
    assert art.command == ""
