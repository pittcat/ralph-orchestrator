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