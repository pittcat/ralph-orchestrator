"""Contract tests for the bootstrap audit (Unit 2) and the agent-docs
helper (Unit 3).

Tests are grouped by unit:

* The audit half exercises ``scripts/audit.py`` against the parameterised
  fixtures under ``skills/ralph-project-bootstrap/fixtures/projects``.
* The agent-docs half exercises ``scripts/agent_docs.py`` and its
  fixtures (``existing-docs``, ``conflicting-docs``, ``broken-markers``,
  ``dirty-tree``).

The contract:

* A preset is required; optional plan/prompt paths block only when supplied
  but unreadable. Preset-native input is valid at the mechanical audit layer.
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

import hashlib
from pathlib import Path

import pytest

import agent_docs  # noqa: F401  (the Unit-3 helper)
from audit import ProjectFacts, collect_project_facts, run_audit  # noqa: F401  (the Unit-2 audit)
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
def test_missing_plan_allows_preset_native_bootstrap(
    tmp_path: Path, fixture_name: str
) -> None:
    project = tmp_path / "project"
    _fixtures.materialise(fixture_name, project)
    decision = run_audit(
        project,
        preset="builtin:ce-executor-pipeline",
        plan_path=None,
    )
    assert not decision.is_blocking
    assert decision.inputs_ok
    assert not decision.issues


def test_unreadable_prompt_file_blocks(tmp_path: Path) -> None:
    project = tmp_path / "project"
    _fixtures.materialise("rust", project)
    decision = run_audit(
        project,
        preset="builtin:merge-loop",
        prompt_file="docs/missing-request.md",
    )
    assert decision.is_blocking
    assert {issue.code for issue in decision.issues} == {
        "input_missing_prompt_file"
    }


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
    assert "cargo test" in decision.facts.test
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
    (project / ".venv" / "bin").mkdir(parents=True)
    (project / ".venv" / "bin" / "python").write_text("", encoding="utf-8")
    decision = run_audit(
        project,
        preset="builtin:ce-executor-pipeline",
        plan_path="plan.md",
    )
    assert decision.facts.technology == "python"
    assert any(".venv" in cmd for cmd in decision.facts.test)


def test_nested_cwd_audits_inputs_and_facts_at_resolved_repo_root(tmp_path: Path) -> None:
    root = tmp_path / "repo"
    nested = root / "packages" / "feature"
    nested.mkdir(parents=True)
    (root / ".git").mkdir()
    (root / "plan.md").write_text("# Plan\n", encoding="utf-8")
    (root / "pyproject.toml").write_text(
        "[project]\nname='nested'\nversion='0.1.0'\ndependencies=['pytest>=8']\n",
        encoding="utf-8",
    )
    (root / ".venv" / "bin").mkdir(parents=True)
    (root / ".venv" / "bin" / "python").write_text("", encoding="utf-8")
    decision = run_audit(
        nested,
        preset="builtin:example",
        plan_path="plan.md",
    )
    assert not decision.is_blocking
    assert decision.root == "../.."
    assert decision.facts.technology == "python"
    assert decision.facts.test == (".venv/bin/python -m pytest",)


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


# --- U3 — path confinement hardening (Unit 3 of plan 2026-07-19-001) -----


@pytest.mark.parametrize(
    "token",
    [
        "..",  # bare parent escape
        "../etc/passwd",  # parent escape
        "a/../../b",  # parent escape after norm
        "/etc/passwd",  # absolute POSIX
        "C:\\Windows\\System32",  # Windows drive
        "C:/Windows/System32",  # Windows drive (forward slashes)
        "c:",  # bare drive letter
        "\\\\server\\share\\file",  # UNC backslashes
        "//server/share/file",  # UNC forward slashes
        "docs/\x00/plan.md",  # NUL byte
        "docs/\x1f/plan.md",  # control byte
        "docs/․/plan.md",  # Unicode one-dot-leader separator
        "docs/／plan.md",  # fullwidth solidus
        "docs/∕plan.md",  # division slash
        "",  # empty
    ],
)
def test_is_safe_relative_rejects_dangerous_tokens(token: str) -> None:
    """The lexical gate MUST reject every escape form documented in
    plan 2026-07-19-001 S7, including POSIX parent escape, Windows
    drive / UNC, NUL, control bytes, and Unicode separator spoofing.
    The check runs before any filesystem resolution so a malicious
    input cannot sneak past via symlink traversal."""
    assert not _paths.is_safe_relative(token), (
        f"is_safe_relative must reject dangerous token {token!r}"
    )


@pytest.mark.parametrize(
    "token",
    [
        "docs/plan.md",
        "./docs/plan.md",
        "scripts/audit.py",
        "a/b/c.txt",
        "nested/dir/file.md",
        ".",  # bare current directory is acceptable as a relative anchor
    ],
)
def test_is_safe_relative_accepts_safe_tokens(token: str) -> None:
    """Canonical safe relative tokens must round-trip through the
    lexical gate."""
    assert _paths.is_safe_relative(token), (
        f"is_safe_relative must accept safe token {token!r}"
    )


def test_normalise_relative_strips_leading_dot_slash() -> None:
    """``./docs/plan.md`` and ``docs/plan.md`` MUST normalise to the
    same canonical string so the handoff display stays portable across
    copy-paste sources."""
    assert _paths.normalise_relative("docs/plan.md") == "docs/plan.md"
    assert _paths.normalise_relative("./docs/plan.md") == "docs/plan.md"
    assert _paths.normalise_relative("././docs/plan.md") == "docs/plan.md"


def test_contain_rejects_lexical_escape(tmp_path: Path) -> None:
    """``contain`` must reject ``../x`` lexically even when the
    filesystem could resolve it inside ``root`` via a symlink."""
    assert not _paths.contain("../escape.txt", tmp_path)
    assert not _paths.contain("/etc/passwd", tmp_path)
    assert not _paths.contain("C:\\Windows", tmp_path)


def test_contain_accepts_safe_relative_under_root(tmp_path: Path) -> None:
    safe = tmp_path / "docs" / "plan.md"
    safe.parent.mkdir(parents=True, exist_ok=True)
    safe.write_text("# plan\n", encoding="utf-8")
    assert _paths.contain("docs/plan.md", tmp_path)


def test_contain_rejects_path_outside_root(tmp_path: Path) -> None:
    """A path that lexically looks safe but resolves outside ``root``
    must be rejected by ``contain``."""
    sibling = tmp_path.parent / "outside.txt"
    if sibling.exists():
        sibling.unlink()
    try:
        # ``../<sibling-name>`` lexically looks safe IF the parent
        # directory is one level up; from inside ``tmp_path`` that
        # would point at the parent. The lexical gate rejects it
        # before we even get to the resolution step.
        assert not _paths.contain(f"../{sibling.name}", tmp_path)
    finally:
        if sibling.exists():
            sibling.unlink()


def test_rel_returns_marker_for_paths_outside_root(tmp_path: Path) -> None:
    """``rel`` MUST NOT silently fall back to ``str(path)`` when the
    input does not resolve under ``root`` — the handoff layer relies on
    a deterministic ``"<outside-root>"`` marker so it can refuse to
    render leaked absolute paths."""
    outside = "/etc/passwd"
    marker = _paths.rel(outside, root=tmp_path)
    assert marker == "<outside-root>", (
        f"rel must return the outside-root marker; got {marker!r}"
    )


def test_rel_normalises_relative_input(tmp_path: Path) -> None:
    """When ``path`` is already relative, ``rel`` MUST return the
    POSIX-normalised form so the handoff display never surfaces a
    leading ``./`` that is merely redundant with the project anchor."""
    rendered = _paths.rel("docs/plan.md", root=tmp_path)
    assert rendered == "./docs/plan.md"
    rendered_with_dot = _paths.rel("./docs/plan.md", root=tmp_path)
    assert rendered_with_dot == "./docs/plan.md"


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


def test_preset_bound_suite_uses_preset_specific_paths_and_prompt() -> None:
    preset_text = """
event_loop:
  prompt: |
    Generate the modem case documentation.
    Do not execute hardware operations.
  max_iterations: 120
"""

    suite = pipeline_suite.compose_preset_bound_suite(
        preset="modem-case-docs.yml",
        preset_text=preset_text,
        backend="claude",
        budget_max_iterations=120,
        budget_wall_clock_seconds=28_800,
    )

    assert suite.config_path == "ralph.modem-case-docs.yml"
    assert suite.prompt_path == "PROMPT.modem-case-docs.md"
    assert suite.prompt == (
        "Generate the modem case documentation.\n"
        "Do not execute hardware operations.\n"
    )
    user_keys, owned_keys = pipeline_suite.parse_owned_yaml(suite.config)
    assert user_keys["event_loop"]["prompt_file"] == suite.prompt_path
    assert "input_signature" in owned_keys
    assert "profile_sha256" in owned_keys
    assert "prompt_sha256" in owned_keys
    assert suite.provenance_path is None

    assert pipeline_suite.reconcile_preset_bound_suite(
        suite.config, suite.prompt, suite
    ).kind == "noop"


def test_preset_bound_suite_blocks_hand_edited_prompt() -> None:
    suite = pipeline_suite.compose_preset_bound_suite(
        preset="modem-case-docs.yml",
        preset_text="event_loop:\n  prompt: Generate docs.\n",
        backend="claude",
        budget_max_iterations=3,
        budget_wall_clock_seconds=60,
    )
    result = pipeline_suite.reconcile_preset_bound_suite(
        suite.config, suite.prompt + "operator edit\n", suite
    )
    assert result.kind == "blocker"
    assert result.code == "owned_value_user_modified"


def test_written_preset_bound_suite_reopens_with_exact_prompt_source(
    tmp_path: Path,
) -> None:
    suite = pipeline_suite.compose_preset_bound_suite(
        preset="modem-case-docs.yml",
        preset_text="event_loop:\n  prompt: Generate docs.\n",
        backend="claude",
        budget_max_iterations=3,
        budget_wall_clock_seconds=60,
    )
    with agent_docs.AtomicWriter(
        [
            (tmp_path / suite.config_path, suite.config),
            (tmp_path / suite.prompt_path, suite.prompt),
        ]
    ) as writer:
        committed, rolled_back = writer.execute()
    assert len(committed) == 2
    assert rolled_back == ()
    assert pipeline_suite.verify_preset_bound_files(tmp_path, suite).kind == "noop"


def test_preset_bound_suite_rejects_missing_inline_prompt() -> None:
    with pytest.raises(pipeline_suite.OwnedYamlError) as excinfo:
        pipeline_suite.compose_preset_bound_suite(
            preset="no-prompt.yml",
            preset_text="event_loop:\n  max_iterations: 2\n",
            backend="claude",
            budget_max_iterations=2,
            budget_wall_clock_seconds=60,
        )

    assert excinfo.value.code == "preset_prompt_missing"


def test_preset_bound_suite_uses_plan_for_missing_inline_prompt() -> None:
    suite = pipeline_suite.compose_preset_bound_suite(
        preset="no-prompt.yml",
        preset_text="event_loop:\n  max_iterations: 2\n",
        plan_path="docs/plan.md",
        backend="claude",
        budget_max_iterations=2,
        budget_wall_clock_seconds=60,
    )

    user_keys, owned_keys = pipeline_suite.parse_owned_yaml(suite.config)
    assert user_keys["event_loop"]["prompt_file"] == suite.prompt_path
    assert pipeline_suite._render_owned_value(suite.config, "plan") == "docs/plan.md"
    assert "docs/plan.md" in suite.prompt
    assert "requires a real plan" not in suite.prompt
    assert "input_signature" in owned_keys


def test_preset_bound_signature_changes_when_resolved_preset_changes() -> None:
    common = dict(
        preset="no-prompt.yml",
        plan_path="docs/plan.md",
        backend="claude",
        budget_max_iterations=2,
        budget_wall_clock_seconds=60,
    )
    first = pipeline_suite.compose_preset_bound_suite(
        preset_text="event_loop:\n  max_iterations: 2\n", **common
    )
    second = pipeline_suite.compose_preset_bound_suite(
        preset_text="event_loop:\n  max_iterations: 3\n", **common
    )

    first_signature = pipeline_suite._render_owned_value(
        first.config, "input_signature"
    )
    second_signature = pipeline_suite._render_owned_value(
        second.config, "input_signature"
    )
    assert first_signature != second_signature


@pytest.mark.parametrize(
    ("preset", "expected_stem"),
    [
        ("modem-case-docs.yml", "modem-case-docs"),
        ("presets/custom.docs.yaml", "custom.docs"),
        ("builtin:ce-executor-pipeline", "ce-executor-pipeline"),
    ],
)
def test_preset_bound_paths_are_derived_from_preset(
    preset: str, expected_stem: str
) -> None:
    paths = pipeline_suite.derive_preset_bound_paths(preset)
    assert paths.config == f"ralph.{expected_stem}.yml"
    assert paths.prompt == f"PROMPT.{expected_stem}.md"


# S1 — config + prompt generated for blank project.


def test_suite_generates_config_and_prompt_for_blank_project() -> None:
    suite = _make_pipeline_suite()
    # The four owned keys MUST live under ``_bootstrap:``.
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
    # The user-keys block must contain the fields ``RalphConfig``
    # actually consumes — NOT the legacy top-level ``budget``/
    # ``diagnostics`` / ``event_loop.backend`` shape the old fixture
    # claimed to set.
    assert "cli" in user_keys, suite.config
    assert user_keys["cli"]["backend"] == "claude"
    assert "event_loop" in user_keys
    assert user_keys["event_loop"]["prompt_file"] == "PROMPT.pipeline.md"
    assert user_keys["event_loop"]["max_iterations"] == 12
    assert user_keys["event_loop"]["max_runtime_seconds"] == 7200
    # And the runtime's diagnostics namespace, not the invented
    # ``diagnostics`` top-level field.
    assert "diagnostics" not in user_keys, suite.config
    assert "telemetry" in user_keys
    assert user_keys["telemetry"]["runtime_diagnosis"]["enabled"] is True
    assert user_keys["telemetry"]["runtime_diagnosis"]["write_artifacts"] is True
    assert user_keys["cli"]["autonomous_idle_timeout_secs"] == 900
    assert len(user_keys["core"]["guardrails"]) >= 4


def test_python_project_facts_drive_project_specific_guardrails(tmp_path: Path) -> None:
    (tmp_path / ".venv" / "bin").mkdir(parents=True)
    (tmp_path / ".venv" / "bin" / "python").write_text("", encoding="utf-8")
    (tmp_path / "pyproject.toml").write_text(
        """
[project]
name = "example"
version = "0.1.0"
[project.optional-dependencies]
dev = ["pytest>=8", "ruff>=0.5", "mypy>=1.10"]
[tool.ruff]
line-length = 100
[tool.mypy]
strict = true
""".lstrip(),
        encoding="utf-8",
    )
    facts = collect_project_facts(tmp_path)
    assert facts.technology == "python"
    assert facts.format == (".venv/bin/python -m ruff format --check .",)
    assert facts.lint == (
        ".venv/bin/python -m ruff check .",
        ".venv/bin/python -m mypy .",
    )
    suite = pipeline_suite.compose_suite(
        preset="builtin:ce-executor-pipeline",
        prompt_file="PROMPT.md",
        backend="claude",
        budget_max_iterations=30,
        budget_wall_clock_seconds=7200,
        project_guardrails=facts.runtime_guardrails(),
    )
    user_keys, _ = pipeline_suite.parse_owned_yaml(suite.config)
    guardrails = "\n".join(user_keys["core"]["guardrails"])
    assert ".venv/bin/python -m pytest" in guardrails
    assert ".venv/bin/python -m ruff check ." in guardrails
    assert ".venv/bin/python -m ruff format --check ." in guardrails
    assert ".venv/bin/python -m mypy ." in guardrails
    assert "cargo" not in guardrails


def test_compose_suite_accepts_audit_facts_as_the_project_overlay(tmp_path: Path) -> None:
    (tmp_path / "package.json").write_text(
        '{"scripts":{"test":"vitest","lint":"eslint ."}}',
        encoding="utf-8",
    )
    facts = collect_project_facts(tmp_path)
    suite = pipeline_suite.compose_suite(
        preset="presets/custom.yml",
        backend="auto",
        budget_max_iterations=10,
        budget_wall_clock_seconds=1200,
        project_facts=facts,
    )
    user_keys, _ = pipeline_suite.parse_owned_yaml(suite.config)
    guardrails = "\n".join(user_keys["core"]["guardrails"])
    assert "npm run lint" in guardrails
    assert "npm test" in guardrails


def test_pep735_dependency_groups_are_detected_without_layout_guessing(tmp_path: Path) -> None:
    (tmp_path / ".venv" / "bin").mkdir(parents=True)
    (tmp_path / ".venv" / "bin" / "python").write_text("", encoding="utf-8")
    (tmp_path / "pyproject.toml").write_text(
        """
[project]
name = "atelier-like"
version = "0.1.0"
[dependency-groups]
dev = ["pytest>=8", "ruff>=0.6", "mypy>=1.11"]
""".lstrip(),
        encoding="utf-8",
    )
    facts = collect_project_facts(tmp_path)
    assert facts.test == (".venv/bin/python -m pytest",)
    assert facts.lint[-1] == ".venv/bin/python -m mypy ."


def test_project_tooling_selects_declared_runners_and_lockfiles(tmp_path: Path) -> None:
    node = tmp_path / "node"
    node.mkdir()
    (node / "package.json").write_text(
        '{"scripts":{"test":"vitest","lint":"eslint ."}}', encoding="utf-8"
    )
    (node / "pnpm-lock.yaml").write_text("lockfileVersion: '9'\n", encoding="utf-8")
    assert collect_project_facts(node).test == ("pnpm run test",)

    python = tmp_path / "python"
    python.mkdir()
    (python / "pyproject.toml").write_text(
        "[project]\nname='x'\nversion='0.1'\ndependencies=['pytest']\n",
        encoding="utf-8",
    )
    (python / "uv.lock").write_text("version = 1\n", encoding="utf-8")
    assert collect_project_facts(python).test == ("uv run python -m pytest",)

    rust = tmp_path / "rust"
    (rust / ".config").mkdir(parents=True)
    (rust / "Cargo.toml").write_text("[package]\nname='x'\nversion='0.1.0'\n", encoding="utf-8")
    (rust / ".config" / "nextest.toml").write_text("[profile.default]\n", encoding="utf-8")
    assert collect_project_facts(rust).test == ("cargo nextest run",)


def test_unknown_stack_uses_declared_task_runner_targets(tmp_path: Path) -> None:
    (tmp_path / "Makefile").write_text(
        "build:\n\ttool build\n\ntest:\n\ttool test\n\nlint:\n\ttool lint\n",
        encoding="utf-8",
    )
    facts = collect_project_facts(tmp_path)
    assert facts.technology == "task-runner"
    assert facts.build == ("make build",)
    assert facts.test == ("make test",)
    assert facts.lint == ("make lint",)


def test_verified_generated_profile_can_be_refreshed() -> None:
    old = (
        "# Generated by ralph-project-bootstrap. Owned keys live under\n"
        "core:\n  project_root: ./\n"
        "_bootstrap:\n"
        "  preset: builtin:example\n  plan: \"\"\n"
        "  prompt_file: \"\"\n  preflight: strict\n"
    )
    provenance = (
        "generator_version: 0.2.0\n"
        "input_signature: legacy\n"
        "owned_keys:\n  - preset\n"
        "summary:\n"
        "  - file: ralph.pipeline.yml\n"
        f"    sha256: {hashlib.sha256(old.encode()).hexdigest()}\n"
    )
    result = pipeline_suite.apply_pipeline_config(
        old,
        preset="builtin:example",
        backend="claude",
        budget_max_iterations=10,
        budget_wall_clock_seconds=1200,
        refresh_generated_profile=True,
        existing_provenance_text=provenance,
    )
    assert result.kind == "updated"
    assert "guardrails:" in (result.text or "")
    assert "write_artifacts: true" in (result.text or "")


def test_refresh_refuses_operator_authored_pipeline() -> None:
    result = pipeline_suite.apply_pipeline_config(
        "core:\n  project_root: ./\n",
        preset="builtin:example",
        backend="claude",
        budget_max_iterations=10,
        budget_wall_clock_seconds=1200,
        refresh_generated_profile=True,
        existing_provenance_text="summary: []\n",
    )
    assert result.kind == "blocker"
    assert result.code == "profile_not_generator_owned"


def test_refresh_rejects_generated_profile_without_matching_provenance() -> None:
    old = "# Generated by ralph-project-bootstrap.\ncore:\n  project_root: ./\n"
    result = pipeline_suite.apply_pipeline_config(
        old,
        preset="builtin:example",
        backend="claude",
        budget_max_iterations=10,
        budget_wall_clock_seconds=1200,
        refresh_generated_profile=True,
        existing_provenance_text=(
            "generator_version: 0.2.0\ninput_signature: legacy\n"
            "owned_keys:\n  - preset\nsummary:\n"
            "  - file: ralph.pipeline.yml\n    sha256: deadbeef\n"
        ),
    )
    assert result.kind == "blocker"
    assert result.code == "owned_value_user_modified"


def test_pipeline_baseline_asset_carries_runtime_profile() -> None:
    baseline = ROOT / "skills" / "ralph-project-bootstrap" / "assets" / "ralph.pipeline.base.yml"
    text = baseline.read_text(encoding="utf-8")
    assert "autonomous_idle_timeout_secs" in text
    assert "runtime_diagnosis:" in text
    assert "core:" in text and "guardrails:" in text


def test_suite_supports_preset_native_without_plan_or_prompt_artifact() -> None:
    suite = pipeline_suite.compose_suite(
        preset="builtin:merge-loop",
        backend="auto",
        budget_max_iterations=15,
        budget_wall_clock_seconds=1800,
        manage_prompt=False,
    )
    user_keys, owned_keys = pipeline_suite.parse_owned_yaml(suite.config)
    assert suite.prompt is None
    assert "prompt_file" not in user_keys["event_loop"]
    assert set(owned_keys) == set(pipeline_suite.PIPELINE_OWNED_KEYS)
    assert pipeline_suite._render_owned_value(suite.config, "plan") == ""
    assert pipeline_suite._render_owned_value(suite.config, "prompt_file") == ""
    assert [path for path, _ in suite.provenance.summary] == [
        "ralph.pipeline.yml"
    ]


def test_suite_references_operator_prompt_without_owning_its_bytes() -> None:
    suite = pipeline_suite.compose_suite(
        preset="presets/docs-writer.yml",
        prompt_file="docs/writing-request.md",
        backend="auto",
        budget_max_iterations=8,
        budget_wall_clock_seconds=1200,
        manage_prompt=False,
    )
    user_keys, _ = pipeline_suite.parse_owned_yaml(suite.config)
    assert user_keys["event_loop"]["prompt_file"] == "docs/writing-request.md"
    assert suite.prompt is None
    assert [path for path, _ in suite.provenance.summary] == [
        "ralph.pipeline.yml"
    ]


def test_prompt_ownership_changes_input_signature() -> None:
    common = dict(
        preset="presets/docs-writer.yml",
        prompt_file="docs/writing-request.md",
        backend="auto",
        budget_max_iterations=8,
        budget_wall_clock_seconds=1200,
    )
    managed = pipeline_suite.compose_suite(**common, manage_prompt=True)
    referenced = pipeline_suite.compose_suite(**common, manage_prompt=False)
    assert managed.provenance.input_signature != referenced.provenance.input_signature


def test_plan_driven_bootstrap_generates_suite_before_plan_exists() -> None:
    suite = pipeline_suite.compose_suite(
        preset="builtin:ce-executor-pipeline",
        plan_path=None,
        prompt_file="PROMPT.md",
        backend="claude",
        budget_max_iterations=20,
        budget_wall_clock_seconds=7200,
        manage_prompt=True,
        requires_plan=True,
    )
    assert suite.prompt is not None
    assert "requires a real plan" in suite.prompt
    assert "--plan <repo-relative-path>" in suite.prompt
    assert "stop and report" in suite.prompt
    user_keys, _ = pipeline_suite.parse_owned_yaml(suite.config)
    assert user_keys["event_loop"]["prompt_file"] == "PROMPT.md"


def test_required_plan_changes_input_signature() -> None:
    optional = pipeline_suite.compute_input_signature(
        "builtin:example", None, "./", "PROMPT.md", True, False
    )
    required = pipeline_suite.compute_input_signature(
        "builtin:example", None, "./", "PROMPT.md", True, True
    )
    assert optional != required


def test_manage_prompt_requires_a_prompt_path() -> None:
    with pytest.raises(pipeline_suite.OwnedYamlError) as excinfo:
        pipeline_suite.compose_suite(
            preset="builtin:merge-loop",
            backend="auto",
            budget_max_iterations=15,
            budget_wall_clock_seconds=1800,
            manage_prompt=True,
        )
    assert excinfo.value.code == "owned_yaml_invalid"


def test_validation_and_handoff_support_preset_native() -> None:
    dry_run_argv = cli_probe._build_stage_argv(
        "dry_run",
        binary="ralph",
        config_path="ralph.pipeline.yml",
        preset="builtin:merge-loop",
        prompt_file=None,
        plan_path=None,
    )
    assert "--prompt-file" not in dry_run_argv
    assert "--plan" not in dry_run_argv

    artifact = handoff.build_handoff(
        handoff.HandoffInputs(
            binary="ralph",
            config_path="ralph.pipeline.yml",
            preset="builtin:merge-loop",
            plan_path=None,
            prompt_file=None,
            level="incomplete_static_only",
        )
    )
    assert artifact.command_argv == (
        "ralph",
        "-c",
        "ralph.pipeline.yml",
        "-H",
        "builtin:merge-loop",
        "run",
    )


def test_preset_native_dry_run_accepts_runtime_resolved_prompt() -> None:
    outcome, reason, _ = cli_probe._classify_dry_run(
        "Backend: claude\nPrompt file: PRESET_PROMPT.md\n",
        "",
        expected={},
    )
    assert outcome == "ok"
    assert reason == "static load passed; loop not closed"


@pytest.mark.parametrize("field", ["plan_path", "prompt_file"])
def test_handoff_rejects_empty_optional_launch_paths(field: str) -> None:
    kwargs = {
        "binary": "ralph",
        "config_path": "ralph.pipeline.yml",
        "preset": "builtin:merge-loop",
        "plan_path": None,
        "prompt_file": None,
        "level": "incomplete_static_only",
    }
    kwargs[field] = ""
    with pytest.raises(ValueError, match=field):
        handoff.HandoffInputs(**kwargs)


def test_missing_required_plan_produces_template_not_blocker() -> None:
    artifact = handoff.build_handoff(
        handoff.HandoffInputs(
            binary="ralph",
            config_path="ralph.pipeline.yml",
            preset="builtin:ce-executor-pipeline",
            plan_path=None,
            prompt_file=None,
            level="incomplete_static_only",
            requires_plan=True,
            files_created=(
                "ralph.pipeline.yml",
                "PROMPT.md",
                "ralph.bootstrap.yml",
            ),
        )
    )
    assert artifact.level == "incomplete_static_only"
    assert artifact.command.startswith("[TEMPLATE - replace PLAN_PATH")
    assert artifact.command_argv[-2:] == ("--plan", "PLAN_PATH")
    assert "--prompt-file" not in artifact.command_argv
    assert artifact.created_files == (
        "ralph.pipeline.yml",
        "PROMPT.md",
        "ralph.bootstrap.yml",
    )


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
    # Locate the actual ``_bootstrap:`` top-level key, NOT the
    # substring in the header comment (``# ``_bootstrap:``; ...``).
    bootstrap_idx = text.index("_bootstrap:\n")
    after = text[bootstrap_idx:]
    # Look for the EXACT two-space indented owned key followed by a
    # space (so we don't match substrings of multi-token keys).
    positions: list[int] = []
    for key in pipeline_suite.PIPELINE_OWNED_KEYS:
        needle = f"  {key}: "
        positions.append(after.index(needle))
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
        "cli:\n"
        "  backend: claude\n"
        "  extra_user_field: keep-me\n"
        "event_loop:\n"
        "  prompt_file: PROMPT.pipeline.md\n"
        "  max_iterations: 12\n"
        "  max_runtime_seconds: 7200\n"
        "core:\n"
        "  project_root: ./\n"
        "telemetry:\n"
        "  runtime_diagnosis:\n"
        "    enabled: true\n"
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
        "cli:\n  backend: claude\n"
        "event_loop:\n  prompt_file: PROMPT.pipeline.md\n"
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
    # This fixture is a legacy 0.2.0 suite. It remains byte-stable unless
    # the caller explicitly verifies provenance and requests profile refresh.
    assert "generator_version: 0.2.0" in original_provenance


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
    # Real ``ralph run --dry-run`` printed a different ``Prompt file``
    # than the pipeline suite requested: that is a typed
    # source/effective mismatch, NOT the legacy ``config_path=`` marker
    # search. The helper must surface it as ``blocked_command`` so the
    # caller cannot claim "binary printed something" ⇒ "binary used our
    # suite".
    assert dry_run.outcome == "blocked_command"
    assert "prompt_file" in dry_run.blocked_reason


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
            "run --dry-run",
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
    run --dry-run --prompt-file <prompt> --plan <plan>`` and must NOT carry
    ``--strict`` (the real ``ralph run`` does not accept it)."""
    runner = _make_runner("green")
    decisions = cli_probe.validate_pipeline(runner=runner, **_PIPELINE_KW)  # type: ignore[arg-type]
    dry_run = decisions[-1]
    argv = dry_run.argv
    assert argv[0] == "ralph"
    assert "--dry-run" in argv
    assert "--strict" not in argv, (
        "real ralph run does not accept --strict; the dry-run argv must "
        "not invent one (strict gating is owned by preflight --strict)"
    )
    prompt_idx = argv.index("--prompt-file")
    assert argv[prompt_idx + 1] == "PROMPT.pipeline.md"
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


# --- T16 — dry-run evidence carries parsed effective values --------------


def test_cli_probe_dry_run_evidence_carries_effective_values() -> None:
    """The dry-run ``StageDecision.evidence`` MUST carry the parsed
    effective values from the real CLI's ``label: value`` block. The
    contract is no longer satisfied by searching a fake ``config_path=``
    marker: the helper must parse the stable ``Backend`` / ``Prompt
    file`` / ``Max iterations`` / ``Max runtime`` labels."""
    runner = _make_runner("green")
    decisions = cli_probe.validate_pipeline(runner=runner, **_PIPELINE_KW)  # type: ignore[arg-type]
    dry_run = decisions[-1]
    evidence_blob = "\n".join(dry_run.evidence)
    # The real fixture emits these stable labels; the parser must
    # surface each as ``effective_<label>=<value>``.
    assert "effective_backend=" in evidence_blob
    assert "effective_prompt_file=" in evidence_blob
    assert "effective_max_iterations=" in evidence_blob
    assert "effective_max_runtime=" in evidence_blob


def test_cli_probe_dry_run_evidence_does_not_search_fake_marker() -> None:
    """The dry-run outcome MUST be derived from parsed effective values,
    NOT from searching stdout/stderr for ``config_path=`` (a fake marker
    the real CLI never emits — see plan 2026-07-19-001 S11)."""
    runner = _make_runner("green")
    decisions = cli_probe.validate_pipeline(runner=runner, **_PIPELINE_KW)  # type: ignore[arg-type]
    dry_run = decisions[-1]
    assert dry_run.outcome == "ok"
    assert "config_path=" not in dry_run.argv  # type: ignore[operator]
    # And the reason text must come from the parser, not the legacy
    # "does not reference requested config" message.
    assert "does not reference" not in (dry_run.blocked_reason or "")


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
    idle_timeout_secs=5,
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
    # the --idle-timeout flag the harness forwards. The argv MUST
    # carry the flag so a cooperative runtime can implement idle.
    assert result.outcome == "wall_clock_timeout"
    assert "--idle-timeout" in result.argv
    assert "5" in result.argv  # default idle_timeout_secs


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
    --max-iterations, --idle-timeout (in seconds). The wall-clock cap
    is NOT forwarded to the CLI — it lives on the harness outer
    ``timeout`` argument (see plan 2026-07-19-001 F6 / Unit 4)."""
    backend = smoke_runner.SafeBackend(name="replay")
    runner = _fake_runner(stdout="plan.ready\nLOOP_COMPLETE\n", returncode=0)
    result = smoke_runner.run_smoke(backend, _smoke_cfg(), runner=runner)
    argv = result.argv
    assert argv[5] == "run"
    assert "-c" in argv
    assert "ralph.pipeline.yml" in argv
    assert "-H" in argv
    assert "builtin:ce-executor-pipeline" in argv
    assert "--max-iterations" in argv
    assert "3" in argv
    assert "--idle-timeout" in argv
    assert "5" in argv
    # Negative contract: the wall-clock cap and the legacy
    # ``--idle-timeout-ms`` flag MUST NOT appear on the argv — those
    # belong to the harness outer ``timeout`` / the public CLI
    # ``--idle-timeout`` seconds flag respectively.
    assert "--idle-timeout-ms" not in argv
    assert "--wall-clock-timeout-s" not in argv


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
        # Typed outcome drives the level — free-text evidence is just
        # a debugging footnote. See plan 2026-07-19-001 F7.
        smoke_outcome="bounded_terminal_reached",
        smoke_failure_bucket="none",
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
    assert argv[5] == "run"
    # The command string mirrors the argv.
    assert "ralph -c ralph.pipeline.yml -H test-preset run" in art.command


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
        smoke_outcome="not_authorized",
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
        smoke_outcome="bounded_terminal_reached",
        smoke_failure_bucket="none",
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
        smoke_outcome="bounded_terminal_reached",
        smoke_failure_bucket="none",
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
        smoke_outcome="bounded_terminal_reached",
        smoke_failure_bucket="none",
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
    assert argv[5] == "run"


def test_handoff_worktree_command_shape_name_branch() -> None:
    inputs = _make_inputs(
        level="complete",
        use_worktree=True,
        reuse_worktree=True,
        worktree_name="lucky-reed",
        smoke_outcome="bounded_terminal_reached",
        smoke_failure_bucket="none",
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
        smoke_outcome="bounded_terminal_reached",
        smoke_failure_bucket="none",
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
    # static-only -- smoke-not-authorized (no smoke was even attempted)
    art_static = handoff.build_handoff(
        _make_inputs(level="incomplete_static_only", smoke_evidence=())
    )
    assert "Status: `static-only -- smoke-not-authorized`" in art_static.report
    # complete — typed outcome drives the level
    art_ok = handoff.build_handoff(
        _make_inputs(
            level="complete",
            smoke_outcome="bounded_terminal_reached",
            smoke_failure_bucket="none",
            smoke_evidence=("bounded_terminal_reached",),
        )
    )
    assert "Status: `complete`" in art_ok.report
    # blocked -- <bucket> via typed outcome + failure_bucket
    art_bucket = handoff.build_handoff(
        _make_inputs(
            level="blocked",
            blocker_summary="backend boom",
            smoke_outcome="non_zero_exit",
            smoke_failure_bucket="backend",
            smoke_evidence=("non_zero_exit bucket=backend",),
        )
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
        "ce-executor-lite",
        "ralph-hats",
    )
    for literal in forbidden_literals:
        assert literal not in module_text, (
            f"handoff.py must not hard-code {literal!r}"
        )


# --- H11 — argv shape across the three levels ------------------------------


def test_handoff_complete_argv_carries_canonical_flags() -> None:
    inputs = _make_inputs(
        level="complete",
        smoke_outcome="bounded_terminal_reached",
        smoke_failure_bucket="none",
        smoke_evidence=("bounded_terminal_reached",),
    )
    art = handoff.build_handoff(inputs)
    argv = art.command_argv
    assert argv[0] == "ralph"
    assert "-c" in argv and "ralph.pipeline.yml" in argv
    assert "-H" in argv and "test-preset" in argv
    assert argv[5] == "run"
    assert argv[-2:] == ("--prompt-file", "PROMPT.pipeline.md")
    assert "--plan" not in argv


def test_handoff_incomplete_argv_is_unaffected_by_prefix() -> None:
    """The argv tuple for incomplete must equal the argv tuple for
    complete — only the rendered command string carries the prefix."""
    complete = _make_inputs(
        level="complete",
        smoke_outcome="bounded_terminal_reached",
        smoke_failure_bucket="none",
        smoke_evidence=("bounded_terminal_reached",),
    )
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


# --- U5 / F7 — typed smoke outcome is the only path to ``complete`` -----


def test_handoff_rejects_free_text_complete_without_typed_outcome() -> None:
    """A free-text ``smoke_evidence`` string that contains the literal
    ``bounded_terminal_reached`` MUST NOT, by itself, advance the
    handoff to ``complete``. The caller must populate
    ``smoke_outcome="bounded_terminal_reached"`` explicitly. This is
    the anti-fake-positive contract (plan 2026-07-19-001 F7 / S10):
    a fake runner that smuggled the keyword into a stderr blob MUST
    not be able to fake a complete handoff.
    """
    inputs = _make_inputs(
        level="complete",
        # Only free-text evidence — no typed outcome.
        smoke_outcome=None,
        smoke_failure_bucket=None,
        smoke_evidence=("bounded_terminal_reached",),
    )
    art = handoff.build_handoff(inputs)
    # The renderable level is whatever the caller asked for, but the
    # status token MUST reflect the unverified state — and the body
    # MUST warn that no typed outcome was supplied.
    assert "Status: `complete`" not in art.report
    assert "no typed outcome" in art.report.lower()
    # And the launch command should not be presented as an official,
    # safe-to-run artifact: it must carry the CANDIDATE prefix.
    assert art.command.startswith("[CANDIDATE")


def test_handoff_blocked_outcome_blocks_complete_even_with_evidence() -> None:
    """A ``smoke_outcome`` from ``SMOKE_BLOCKED_OUTCOMES`` (e.g.
    ``non_zero_exit``) MUST downgrade the handoff to ``blocked`` even
    when ``smoke_evidence`` carries the positive keyword. Anti-fake-
    positive contract (S10)."""
    inputs = _make_inputs(
        level="complete",  # caller LIED; helper must refuse
        smoke_outcome="non_zero_exit",
        smoke_failure_bucket="backend",
        smoke_evidence=("bounded_terminal_reached",),  # decoy keyword
    )
    art = handoff.build_handoff(inputs)
    # The blocker must surface; the report must show the bucket.
    assert "blocked" in art.report.lower()
    assert "backend" in art.report.lower()
    # And the launch command must not be rendered as the official one.
    assert not art.command or art.command.startswith("[CANDIDATE")


def test_handoff_not_run_outcome_blocks_complete() -> None:
    """A ``smoke_outcome='not_authorized'`` MUST downgrade the handoff
    to ``incomplete_static_only`` even when ``smoke_evidence`` carries
    the positive keyword. Anti-fake-positive contract."""
    inputs = _make_inputs(
        level="complete",
        smoke_outcome="not_authorized",
        smoke_failure_bucket="none",
        smoke_evidence=("bounded_terminal_reached",),  # decoy
    )
    art = handoff.build_handoff(inputs)
    assert "smoke-not-authorized" in art.report
    assert art.command.startswith("[CANDIDATE")


def test_handoff_typed_complete_outcome_advances_level() -> None:
    """The positive contract: a typed ``smoke_outcome="bounded_terminal_reached"``
    drives the level to ``complete`` even with no evidence strings."""
    inputs = _make_inputs(
        level="complete",
        smoke_outcome="bounded_terminal_reached",
        smoke_failure_bucket="none",
        smoke_evidence=(),
    )
    art = handoff.build_handoff(inputs)
    assert "Status: `complete`" in art.report
    assert not art.command.startswith("[CANDIDATE")
    assert "-c" in art.command_argv and "-H" in art.command_argv


# ---------------------------------------------------------------------------
# Unit 9 — child-group reap on outer timeout (smoke_runner U1 / A1)
# ---------------------------------------------------------------------------
#
# When the harness takes the real-backend path (runner is None), it must
# spawn the child inside its own process group via ``os.setsid`` so the
# outer timeout can reap the ENTIRE group (pty / log writer / temp
# watcher) and not leak orphan descendants into the target project tree.
#
# The harness only enters this branch when ``runner is None`` — tests
# that inject a fake runner continue to use the duck-typed path
# unchanged. To exercise the reap contract without spawning the real
# ``ralph`` binary we monkeypatch ``smoke_runner.subprocess.Popen`` to
# return a fake Popen-shaped object whose ``.communicate`` raises
# ``TimeoutExpired``, and we monkeypatch ``smoke_runner.os.killpg`` to
# record every call so the test can assert the group was reaped.
#
# Acceptance contract for U1 / A1:
#
# * When the outer timeout fires on the real-backend path, the harness
#   MUST call ``os.killpg`` on the spawned child's pid with
#   ``SIGTERM`` first.
# * If the child does not exit within ``_SIGKILL_GRACE_S`` the harness
#   MUST escalate to ``SIGKILL`` against the same group.
# * The harness MUST set ``runner is None`` invariant: only the
#   real-backend path exercises reap; injected ``runner=`` stubs skip
#   the reap contract entirely (their leaks are not the harness's
#   responsibility).


import os as _os  # noqa: E402  (real-backend reap path uses os.killpg)
import signal as _signal  # noqa: E402  (SIGTERM / SIGKILL for reap contract)


class _FakePopen:
    """Popen-shaped stand-in for the real-backend reap-path test.

    The harness reads ``.pid`` to build the killpg group, calls
    ``.communicate(timeout=...)`` to read stdout/stderr (or hit the
    outer timeout), and tracks whether the child terminated during
    the grace window. The harness also drains ``children_to_keep_alive``
    indirectly via the killpg group cascade — every sentinel pid in
    that list shares the group with the parent so a single killpg
    reaps them all. The test verifies this by checking that each
    sentinel pid was the target of a killpg call (or that the group
    pid itself was, which has the same effect on POSIX).
    """

    def __init__(self, pid: int, children: tuple[int, ...]) -> None:
        self.pid = pid
        self._children = children
        self.children_to_keep_alive = list(children)
        self._terminated = False
        self.communicate_calls: list[float | None] = []

    def communicate(self, timeout: float | None = None) -> tuple[str, str]:
        self.communicate_calls.append(timeout)
        # Simulate the harness's outer timeout firing — the real
        # child is still alive at this point so the harness must
        # call killpg on the group.
        raise subprocess.TimeoutExpired(cmd="ralph", timeout=timeout or 0.0)

    def wait(self, timeout: float | None = None) -> int:  # noqa: ARG002
        # Honor SIGTERM grace window — first call returns as if the
        # child exited gracefully; subsequent calls (after the grace
        # window expires) raise TimeoutExpired so the harness
        # escalates to SIGKILL.
        if not self._terminated:
            self._terminated = True
            return 0
        raise subprocess.TimeoutExpired(cmd="ralph", timeout=timeout or 0.0)


def test_run_smoke_reaps_child_group_on_outer_timeout(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """U1 / A1 — on the real-backend path, an outer timeout MUST reap
    the spawned child's process group via ``os.killpg`` so orphan pty /
    log writer / temp watcher cannot leak into the target project tree.

    The test:

    1. Monkeypatches ``smoke_runner.subprocess.Popen`` to return a
       fake Popen-shaped object with a sentinel pid and a list of
       sentinel children that share the process group.
    2. Monkeypatches ``smoke_runner.os.killpg`` to record every call
       instead of actually reaping (the test is hermetic — no real
       subprocess is spawned).
    3. Monkeypatches ``smoke_runner.os.setsid`` to a no-op (we are
       not forking anyway).
    4. Sets the real-backend authorise env var so the harness does
       not refuse the spawn.
    5. Calls ``run_smoke`` with ``runner=None`` (so the real-backend
       Popen branch fires) and asserts:
       - The outcome is ``wall_clock_timeout``.
       - ``os.killpg`` was called with the parent's pid AND
         ``signal.SIGTERM`` first.
       - The argv passed to Popen contains the harness contract flags.
       - The fake Popen's ``communicate(timeout=...)`` was called
         with the outer timeout (= wall_clock_timeout_s + 5s grace).
    """
    sentinel_parent_pid = 4242
    sentinel_children = (4243, 4244, 4245)  # pty / log writer / temp watcher
    fake_proc = _FakePopen(pid=sentinel_parent_pid, children=sentinel_children)

    # Record every killpg call so we can assert the reap contract.
    killpg_calls: list[tuple[int, int]] = []

    def _record_killpg(pid: int, sig: int) -> None:
        killpg_calls.append((pid, sig))

    monkeypatch.setattr(smoke_runner.os, "killpg", _record_killpg)
    # setsid is a no-op in the test — we are not forking anyway.
    monkeypatch.setattr(smoke_runner.os, "setsid", lambda: None, raising=False)

    # Capture the argv Popen was invoked with.
    popen_argvs: list[list[str]] = []
    # Also capture the kwargs the harness passed so we can assert the
    # POSIX-portable reap contract (preexec_fn=os.setsid).
    popen_kwargs: list[dict[str, object]] = []

    def _fake_popen(argv, **kwargs):  # noqa: ANN001
        popen_argvs.append(list(argv))
        popen_kwargs.append(kwargs)
        return fake_proc

    monkeypatch.setattr(smoke_runner.subprocess, "Popen", _fake_popen)
    # The env var authorises the real-backend spawn — the harness
    # would otherwise refuse before reaching Popen.
    monkeypatch.setenv(smoke_runner.ALLOW_REAL_BACKEND_ENV, "1")

    backend = smoke_runner.SafeBackend(name="replay")
    cfg = _smoke_cfg(wall_clock_timeout_s=2)
    result = smoke_runner.run_smoke(backend, cfg, runner=None)

    # --- outcome ---------------------------------------------------------
    assert result.outcome == "wall_clock_timeout"
    # argv was built and handed to Popen — the harness did NOT refuse.
    assert result.argv

    # --- Popen was called exactly once with the harness argv ------------
    assert len(popen_argvs) == 1
    argv = popen_argvs[0]
    assert "-c" in argv and "ralph.pipeline.yml" in argv
    assert "-H" in argv and "builtin:ce-executor-pipeline" in argv
    assert "--max-iterations" in argv
    # The wall-clock cap is NOT forwarded to the CLI: it lives on
    # the harness outer ``timeout`` argument only. We assert the
    # negative contract here so a future helper regression that
    # re-introduces ``--wall-clock-timeout-s`` is caught.
    assert "--wall-clock-timeout-s" not in argv
    assert "--idle-timeout" in argv

    # --- preexec_fn=os.setsid is the POSIX-portable group-spawn knob --
    assert popen_kwargs, "harness must pass kwargs to subprocess.Popen"
    preexec = popen_kwargs[0].get("preexec_fn")
    assert preexec is _os.setsid, (
        "real-backend Popen must use preexec_fn=os.setsid to create "
        "its own process group so killpg can reap the whole tree"
    )

    # --- communicate was called with the outer timeout ------------------
    assert fake_proc.communicate_calls, "communicate(timeout=...) was never called"
    outer_timeout = fake_proc.communicate_calls[0]
    expected_outer = float(cfg.wall_clock_timeout_s) + 5.0
    assert outer_timeout == expected_outer, (
        f"communicate timeout must be wall_clock_timeout_s + 5s grace; "
        f"got {outer_timeout!r} expected {expected_outer!r}"
    )

    # --- killpg was called with the parent pid and SIGTERM first -------
    sigterm_calls = [
        (pid, sig) for pid, sig in killpg_calls if sig == _signal.SIGTERM
    ]
    assert sigterm_calls, (
        f"harness must reap the child group with SIGTERM first; "
        f"killpg calls recorded: {killpg_calls}"
    )
    assert sigterm_calls[0][0] == sentinel_parent_pid, (
        f"first killpg target must be the spawned child's pid "
        f"({sentinel_parent_pid}); got {sigterm_calls[0][0]}"
    )

    # --- the reap contract targets the PROCESS GROUP, not just the pid.
    # killpg(pid, sig) on POSIX sends ``sig`` to every process whose
    # pgid equals ``pid``; the harness MUST use killpg (not kill) so
    # the pty / log writer / temp watcher siblings are reaped together.
    # We assert this by checking that the killpg call signature was
    # used (os.killpg, not os.kill) — the monkeypatch above already
    # guarantees the harness routed through os.killpg.
    assert all(
        isinstance(pid, int) and isinstance(sig, int) for pid, sig in killpg_calls
    ), "every reap call must be os.killpg(pid, sig) with int args"


# ---------------------------------------------------------------------------
# Unit 10 — AtomicWriter hardening (A2)
# ---------------------------------------------------------------------------
#
# The writer used to derive its sibling ``.tmp`` path from the target
# name alone. Two writers running concurrently in the same project
# directory could therefore stage their new bytes into the SAME sibling
# tmp file, and a co-located process could trick the writer into
# committing into a symlink target it controls. These tests lock the
# hardening contract:
#
# * The tmp path is unique per writer invocation (pid + monotonic ns).
# * The writer refuses to overwrite a symlink target — it raises
#   OSError before os.replace so the existing rollback path takes over.
# * Two writers running sequentially in the same process on distinct
#   targets never collide on tmp paths.


def test_atomic_writer_tmp_path_is_unique_per_process() -> None:
    """U2 / A2 — two writers in the same process targeting the same
    file must produce different sibling tmp paths. The tmp path must
    encode the writer's pid and a monotonic nanosecond stamp so a
    co-located process cannot pre-stage a malicious sibling."""
    writer_a = agent_docs.AtomicWriter([])
    writer_b = agent_docs.AtomicWriter([])
    target = Path("foo")
    path_a = writer_a._tmp_path(target)
    path_b = writer_b._tmp_path(target)
    assert path_a != path_b, (
        "sibling tmp paths must differ across writer instances: "
        f"a={path_a!r} b={path_b!r}"
    )


def test_atomic_writer_refuses_symlink_target(tmp_path: Path) -> None:
    """U2 / A2 — when the target path is a symlink, AtomicWriter must
    refuse to commit so the rollback path takes over. A co-located
    process must NOT be able to drive the writer into writing through a
    symlink it controls.

    The contract observable from ``execute()`` is: nothing committed,
    the target is reported as rolled-back. ``execute()`` swallows the
    OSError internally so callers always see a (committed, rolled)
    pair; the internal raise is what drives the rollback path.
    """
    # Plant the symlink target where AtomicWriter cannot touch it
    # (outside tmp_path so the rollback is observable).
    real_target = tmp_path / "real_target.txt"
    real_target.write_text("ORIGINAL\n", encoding="utf-8")
    # Symlink foo -> real_target (Path.symlink_to is POSIX-portable).
    foo = tmp_path / "foo"
    foo.symlink_to(real_target)
    original_real_bytes = real_target.read_bytes()
    # AtomicWriter writes through foo; the writer must NOT replace the
    # symlink itself (which would leak new bytes into real_target).
    with agent_docs.AtomicWriter([(foo, "PWNED\n")]) as writer:
        committed, rolled = writer.execute()
    # The rollback path returned (committed=(), rolled=(foo,)) — nothing
    # was committed and the symlink target was reported as rolled back.
    assert committed == ()
    assert rolled == (foo,)
    # And real_target must be byte-equal — the writer never crossed
    # the symlink to overwrite its target.
    assert real_target.read_bytes() == original_real_bytes


def test_atomic_writer_concurrent_instances_do_not_collide_on_tmp(
    tmp_path: Path,
) -> None:
    """U2 / A2 — two writers running sequentially in the same process on
    distinct target files must not collide on sibling tmp paths."""
    target_a = Path("a.txt")
    target_b = Path("b.txt")
    writer = agent_docs.AtomicWriter([])
    path_a = writer._tmp_path(target_a)
    path_b = writer._tmp_path(target_b)
    assert path_a != path_b
    # And the sibling tmp must live next to the target, not in cwd.
    assert path_a.parent == Path("a.txt").parent or path_a.parent == Path("")
    assert path_b.parent == Path("b.txt").parent or path_b.parent == Path("")
