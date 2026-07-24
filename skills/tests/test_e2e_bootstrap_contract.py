"""Contract tests for ``ralph-e2e-bootstrap``.

These tests are the public, behavioural contract for the skill:

* The catalog (Unit 1) — ``PUBLIC_SKILLS`` /
  ``.claude-plugin/marketplace.json`` / on-disk ``SKILL.md`` plus
  ``agents/openai.yaml`` all agree the skill exists.
* The combo-box / interaction contract (R12 / S7) — every user
  decision in the skill is rendered as a combo-box with 2–4 options,
  the recommended option first, every option carries a consequence
  clause, and an ``Other`` escape hatch is always present.
* No preset writes (R5) and no live ``ralph run`` (R10) — the
  scripts in this skill are static-analysis only.
* No plan rewrite (R13) — the sandbox suite generator never
  mutates the caller-supplied plan file.
* Plan × diff audit (Unit 2) — ``scripts/plan_diff.py`` returns
  ``ok=True`` for a coherent plan + diff, raises ``clarify_codes``
  for drift, and ``blocked=True`` for an unreadable plan.
* Binary resolution (Unit 3) — ``scripts/binary_resolve.py``
  honours explicit > env > PATH priority and is testable without a
  real ``ralph`` on the host ``PATH``.
* Sandbox suite (Unit 4) — ``scripts/sandbox_suite.py`` generates
  the preset-bound pair and refuses ``presets/`` writes.
* Static gate + handoff (Unit 5) — ``scripts/gate.py`` /
  ``scripts/handoff.py`` produce a ``static_only`` report whose
  argv carries ``-c <config> -H <preset> --plan <abs>``.
"""

from __future__ import annotations

import json
from pathlib import Path
import sys

import pytest

# Probe runner factory (shared with ralph-project-bootstrap).
import _probe_runner_common  # type: ignore[import-not-found]

# Loaded via skills/tests/conftest.py: the sibling
# ``ralph-project-bootstrap`` helpers are pre-registered in
# ``sys.modules`` so we can import them by name.
import install  # type: ignore[import-not-found]
import plan_diff  # type: ignore[import-not-found]
import binary_resolve  # type: ignore[import-not-found]
import sandbox_suite  # type: ignore[import-not-found]
import gate  # type: ignore[import-not-found]
import e2e_handoff as handoff  # type: ignore[import-not-found]

ROOT = Path(__file__).resolve().parents[2]
SKILLS_DIR = ROOT / "skills"
MARKETPLACE = ROOT / ".claude-plugin" / "marketplace.json"
SKILL_DIR = SKILLS_DIR / "ralph-e2e-bootstrap"
SKILL_DOC = SKILL_DIR / "SKILL.md"
AGENT_METADATA = SKILL_DIR / "agents" / "openai.yaml"
INTERACTION_DOC = SKILL_DIR / "references" / "interaction.md"

# Ensure the helper modules under ``skills/ralph-e2e-bootstrap/scripts``
# are importable by name.
import importlib.util
import sys

_SCRIPTS = SKILL_DIR / "scripts"


def _load_into_syspath() -> None:
    if str(_SCRIPTS) not in sys.path:
        sys.path.insert(0, str(_SCRIPTS))


_load_into_syspath()


# ---------------------------------------------------------------------------
# Unit 1 — catalog & skill scaffolding
# ---------------------------------------------------------------------------


def test_e2e_bootstrap_in_public_skills() -> None:
    assert "ralph-e2e-bootstrap" in install.PUBLIC_SKILLS


def test_e2e_bootstrap_in_marketplace_manifest() -> None:
    data = json.loads(MARKETPLACE.read_text(encoding="utf-8"))
    advertised = data["plugins"][0]["skills"]
    assert "./skills/ralph-e2e-bootstrap" in advertised


def test_e2e_bootstrap_directory_has_skill_md_and_agent_metadata() -> None:
    assert SKILL_DIR.is_dir(), "skills/ralph-e2e-bootstrap must exist"
    assert SKILL_DOC.is_file(), "SKILL.md must exist"
    assert AGENT_METADATA.is_file(), "agents/openai.yaml must exist"
    agent_text = AGENT_METADATA.read_text(encoding="utf-8")
    assert "ralph-e2e-bootstrap" in agent_text


def test_e2e_bootstrap_skill_doc_mentions_boundaries() -> None:
    """SKILL.md must declare the no-Rust-mutation boundary (R14)."""
    text = SKILL_DOC.read_text(encoding="utf-8")
    text_lower = text.lower()
    for anchor in (
        "Boundaries",
        "combo-box",
        "static_only",
        "ralph-project-bootstrap",
        "ralph-preset-author",
        "--plan",
    ):
        assert anchor in text or anchor.lower() in text_lower, (
            f"SKILL.md must mention {anchor!r}"
        )


def test_e2e_bootstrap_interaction_doc_has_decision_table() -> None:
    text = INTERACTION_DOC.read_text(encoding="utf-8")
    # Every decision point from SKILL.md must appear in
    # references/interaction.md and be documented as a combo-box.
    for decision in (
        "plan_diff_clarify",
        "binary_resolution",
        "preset_gap",
        "write_conflict",
        "argv_shape",
        "live_run",
    ):
        assert decision in text, f"interaction.md must document {decision!r}"
    for token in ("recommended option", "Other", "consequence", "2–4"):
        assert token in text, f"interaction.md must contain combo-box token {token!r}"


def test_e2e_bootstrap_no_preset_write_anchor() -> None:
    """SKILL.md must forbid preset authoring (R5)."""
    text = SKILL_DOC.read_text(encoding="utf-8")
    assert "ralph-preset-author" in text
    assert "preset-bound" in text or "preset-bound" in text.lower()


def test_e2e_bootstrap_no_live_run_default() -> None:
    """SKILL.md must declare the static-only default (R10)."""
    text = SKILL_DOC.read_text(encoding="utf-8")
    # Either 'static_only' appears literally, or the document
    # explicitly states the skill does not spawn live runs by default.
    assert "static_only" in text
    assert "NEVER spawn a live" in text or "never spawn a live" in text.lower()


def test_e2e_bootstrap_no_plan_rewrite() -> None:
    """SKILL.md must declare the plan-read-only invariant (R13)."""
    text = SKILL_DOC.read_text(encoding="utf-8")
    assert "NEVER rewrites the plan file" in text or "read-only" in text.lower()
    assert "--plan" in text


# ---------------------------------------------------------------------------
# T1 — standalone import (no conftest pre-load)
# ---------------------------------------------------------------------------


def test_gate_standalone_import() -> None:
    """``import gate`` must succeed without conftest pre-load.

    Runs in a subprocess so sys.modules state is clean. The gate.py
    import shim resolves ``cli_probe`` via ``importlib.spec_from_file_location``
    without relying on the conftest sys.modules registration.
    """
    import subprocess

    result = subprocess.run(
        [
            sys.executable,
            "-c",
            (
                "import sys; "
                "sys.modules.pop('cli_probe', None); "
                "sys.modules.pop('gate', None); "
                "sys.path.insert(0, 'skills/ralph-e2e-bootstrap/scripts'); "
                "import gate; "
                "print('import_ok')"
            ),
        ],
        capture_output=True,
        text=True,
        cwd=str(ROOT),
    )
    assert result.returncode == 0, f"import gate failed: {result.stderr}"
    assert "import_ok" in result.stdout


# ---------------------------------------------------------------------------
# Unit 2 — plan × diff audit
# ---------------------------------------------------------------------------


def test_plan_diff_returns_ok_for_coherent_inputs(tmp_path: Path) -> None:
    plan = tmp_path / "plan.md"
    plan.write_text(
        "# Plan\n"
        "\n"
        "## Implementation Units\n"
        "\n"
        "### U1. Add foo\n"
        "\n"
        "Touch `crates/foo.rs` and `crates/bar.rs`.\n"
        "\n"
        "### U2. Update baz\n"
        "\n"
        "Touch `crates/baz.rs`.\n",
        encoding="utf-8",
    )
    decision = plan_diff.run_audit(
        plan,
        repo_root=tmp_path,
        diff_provider=lambda: ("crates/foo.rs", "crates/bar.rs", "crates/baz.rs"),
    )
    assert decision.ok is True
    assert decision.blocked is False
    assert decision.clarify_codes == ()
    assert decision.plan_hash != ""
    assert "crates/foo.rs" in decision.plan_intent_paths


def test_plan_diff_emits_clarify_codes_for_drift(tmp_path: Path) -> None:
    plan = tmp_path / "plan.md"
    plan.write_text(
        "# Plan\n"
        "\n"
        "### U1. Fix only the renderer\n"
        "\n"
        "Touch `crates/renderer.rs`.\n",
        encoding="utf-8",
    )
    decision = plan_diff.run_audit(
        plan,
        repo_root=tmp_path,
        diff_provider=lambda: ("crates/renderer.rs", "crates/auth.rs", "crates/api.rs"),
    )
    assert decision.ok is False
    assert decision.blocked is False
    assert "scope_drift" in decision.clarify_codes


def test_plan_diff_blocks_on_unreadable_plan(tmp_path: Path) -> None:
    decision = plan_diff.run_audit(
        tmp_path / "missing.md",
        repo_root=tmp_path,
        diff_provider=lambda: (),
    )
    assert decision.ok is False
    assert decision.blocked is True
    assert any(issue.code == "plan_unreadable" for issue in decision.issues)


def test_plan_diff_emits_stale_when_diff_empty(tmp_path: Path) -> None:
    plan = tmp_path / "plan.md"
    plan.write_text(
        "# Plan\n"
        "\n"
        "### U1. Fix thing\n"
        "\n"
        "Touch `crates/thing.rs`.\n",
        encoding="utf-8",
    )
    decision = plan_diff.run_audit(
        plan,
        repo_root=tmp_path,
        diff_provider=lambda: (),
    )
    assert "plan_stale" in decision.clarify_codes


def test_plan_diff_has_no_write_side_effects(tmp_path: Path) -> None:
    """No files are created under the sandbox tree by the audit."""
    plan = tmp_path / "plan.md"
    plan.write_text("### U1. Touch `crates/x.rs`.\n", encoding="utf-8")
    plan_diff.run_audit(
        plan,
        repo_root=tmp_path,
        diff_provider=lambda: ("crates/x.rs",),
    )
    # Only the plan file should exist in tmp_path; nothing else.
    assert list(tmp_path.iterdir()) == [plan]


# ---------------------------------------------------------------------------
# Unit 3 — binary resolution
# ---------------------------------------------------------------------------


def test_binary_resolve_prefers_explicit_over_env_and_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = tmp_path / "ralph-explicit"
    fake.write_text("#!/bin/sh\necho 'explicit'\n", encoding="utf-8")
    fake.chmod(0o755)
    monkeypatch.setenv("PATH", "")
    monkeypatch.delenv("RALPH_BINARY", raising=False)

    # No PATH entry, no env, explicit path present → explicit wins.
    resolution = binary_resolve.resolve_binary(
        explicit_path=str(fake),
        runner=lambda argv, **kwargs: _ok_completed(argv, "fake-ralph 0.0.0"),
    )
    assert resolution.ok is True
    assert resolution.source == "explicit"
    assert resolution.version == "fake-ralph 0.0.0"


def test_binary_resolve_falls_back_to_env(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("PATH", "")
    monkeypatch.setenv("RALPH_BINARY", "/does/not/exist/ralph")

    def fail_runner(argv, **kwargs):  # noqa: ANN001, ARG001
        raise FileNotFoundError("no such file")

    resolution = binary_resolve.resolve_binary(
        runner=fail_runner,
    )
    # Env entry exists but the binary is not executable → combo-box
    # so the operator can rebuild / install / override.
    assert resolution.ok is False
    assert resolution.reason == "combo_box"
    assert resolution.source == "env"


def test_binary_resolve_combo_box_when_path_is_empty(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("PATH", "")
    monkeypatch.delenv("RALPH_BINARY", raising=False)
    resolution = binary_resolve.resolve_binary(
        path_iter=lambda: (),
        runner=lambda argv, **kwargs: _ok_completed(argv, ""),
    )
    assert resolution.ok is False
    assert resolution.reason == "combo_box"
    assert resolution.source == "missing"
    assert resolution.binary == binary_resolve.MISSING_TOKEN


def test_binary_resolve_uses_fake_path_iter(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("PATH", "")
    monkeypatch.delenv("RALPH_BINARY", raising=False)
    fake = tmp_path / "ralph"
    fake.write_text("#!/bin/sh\necho fake\n", encoding="utf-8")
    fake.chmod(0o755)

    # We use the fake path iterator and a runner that fakes the
    # version probe so the resolved binary does not have to be a
    # real Ralph binary.
    resolution = binary_resolve.resolve_binary(
        path_iter=lambda: iter([tmp_path]),
        runner=lambda argv, **kwargs: _ok_completed(argv, "fake-ralph 0.0.0"),
        require_version=True,
    )
    # The fake binary is on the fake PATH and answers --version.
    assert resolution.ok is True
    assert resolution.source == "path"


# ---------------------------------------------------------------------------
# T6 — binary resolution: strengthened cases
# ---------------------------------------------------------------------------


def test_binary_resolve_explicit_fail_blocked(tmp_path: Path) -> None:
    """Explicit path to a non-existent binary → ``reason='blocked'``."""
    resolution = binary_resolve.resolve_binary(
        explicit_path="/nonexistent/ralph",
        runner=lambda argv, **kw: (_ for _ in ()).throw(FileNotFoundError("no such binary")),
    )
    assert resolution.reason == "blocked"
    assert resolution.source == "explicit"


def test_binary_resolve_path_version_fail_combo_box(tmp_path: Path) -> None:
    """PATH has a binary that exits non-zero on ``--version`` → combo_box."""
    fake = tmp_path / "ralph"
    fake.write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
    fake.chmod(0o755)

    resolution = binary_resolve.resolve_binary(
        path_iter=lambda: iter([tmp_path]),
        runner=lambda argv, **kw: _Completed(stderr="version probe failed", returncode=1),
    )
    assert resolution.reason == "combo_box"
    assert resolution.source == "path"


def test_binary_resolve_require_version_false(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """``require_version=False`` — PATH fake with non-zero exit is accepted as ok."""
    monkeypatch.setenv("PATH", "")
    monkeypatch.delenv("RALPH_BINARY", raising=False)
    fake = tmp_path / "ralph"
    fake.write_text("#!/bin/sh\necho 'ralph'\n", encoding="utf-8")
    fake.chmod(0o755)

    resolution = binary_resolve.resolve_binary(
        path_iter=lambda: iter([tmp_path]),
        runner=lambda argv, **kw: _Completed(stderr="error", returncode=1),
        require_version=False,
    )
    assert resolution.reason == "ok"


def test_binary_resolve_exception_file_not_found() -> None:
    """FileNotFoundError during version probe → ``reason='missing'`` (explicit → blocked)."""

    def fnf_runner(argv, **kw):  # noqa: ANN001, ARG001
        raise FileNotFoundError("no such file")

    resolution = binary_resolve.resolve_binary(
        explicit_path="/some/ralph",
        runner=fnf_runner,
    )
    # explicit + missing → blocked per A8 / trusted_path
    assert resolution.reason == "blocked"
    assert resolution.source == "explicit"


def test_binary_resolve_exception_timeout_expired() -> None:
    """subprocess.TimeoutExpired during version probe → ``reason='blocked'`` (explicit path)."""
    import subprocess

    def timeout_runner(argv, **kw):  # noqa: ANN001, ARG001
        raise subprocess.TimeoutExpired(argv, 5.0)

    resolution = binary_resolve.resolve_binary(
        explicit_path="/some/ralph",
        runner=timeout_runner,
    )
    assert resolution.reason == "blocked"
    assert resolution.source == "explicit"


def test_binary_resolve_exception_os_error() -> None:
    """OSError during version probe → ``reason='blocked'`` (explicit path)."""

    def oserror_runner(argv, **kw):  # noqa: ANN001, ARG001
        raise OSError("permission denied")

    resolution = binary_resolve.resolve_binary(
        explicit_path="/some/ralph",
        runner=oserror_runner,
    )
    assert resolution.reason == "blocked"
    assert resolution.source == "explicit"


def test_binary_resolve_priority_explicit_none_env_hit_path_hit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Priority: explicit None + env hit + PATH hit → env wins."""
    env_fake = tmp_path / "ralph-env"
    env_fake.write_text("#!/bin/sh\necho env-ralph 1.0.0\n", encoding="utf-8")
    env_fake.chmod(0o755)

    path_fake = tmp_path / "ralph-path"
    path_fake.write_text("#!/bin/sh\necho path-ralph 1.0.0\n", encoding="utf-8")
    path_fake.chmod(0o755)

    monkeypatch.setenv("RALPH_BINARY", str(env_fake))
    monkeypatch.setenv("PATH", str(tmp_path))

    resolution = binary_resolve.resolve_binary(
        runner=lambda argv, **kw: _ok_completed(argv, "ralph 1.0.0"),
    )
    # Env wins over PATH.
    assert resolution.source == "env"
    assert resolution.reason == "ok"


def test_binary_resolve_trusted_path_explicit_rejects_tmp(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Explicit path under /tmp/ is blocked by trusted_path guard."""
    monkeypatch.setenv("PATH", "")
    monkeypatch.delenv("RALPH_BINARY", raising=False)

    fake_tmp = tmp_path / "fake-ralph"
    fake_tmp.write_text("#!/bin/sh\necho fake\n", encoding="utf-8")
    fake_tmp.chmod(0o755)

    # We need to use a real /tmp path (not pytest's tmp_path which is /private/var/...).
    real_tmp = Path("/tmp/ralph-e2e-test-fake-ralph")
    real_tmp.write_text("#!/bin/sh\necho fake\n", encoding="utf-8")
    real_tmp.chmod(0o755)
    try:
        resolution = binary_resolve.resolve_binary(
            explicit_path=str(real_tmp),
            runner=lambda argv, **kw: _ok_completed(argv, "ralph 0.0.0"),
        )
        assert resolution.reason == "blocked"
        assert "untrusted path" in resolution.detail.lower()
    finally:
        real_tmp.unlink(missing_ok=True)


# ---------------------------------------------------------------------------
# Unit 4 — sandbox suite
# ---------------------------------------------------------------------------


def test_sandbox_suite_generates_preset_bound_pair(tmp_path: Path) -> None:
    sandbox = tmp_path / "sandbox"
    sandbox.mkdir()
    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")
    result = sandbox_suite.generate_suite(
        sandbox=sandbox,
        preset="builtin:ce-executor-pipeline",
        plan_path=plan,
    )
    assert (sandbox / "ralph.ce-executor-pipeline.yml").is_file()
    assert (sandbox / "PROMPT.ce-executor-pipeline.md").is_file()
    assert result.config_sha256 and result.prompt_sha256 and result.plan_sha256
    assert "-c" in result.argv and "-H" in result.argv
    assert "--plan" in result.argv and str(plan) in result.argv
    # No preset mutation.
    assert "presets" not in result.argv


def test_sandbox_suite_refuses_presets_subtree(tmp_path: Path) -> None:
    bad = tmp_path / "presets"
    bad.mkdir()
    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")
    with pytest.raises(sandbox_suite.SandboxError) as excinfo:
        sandbox_suite.generate_suite(
            sandbox=bad,
            preset="builtin:ce-executor-pipeline",
            plan_path=plan,
        )
    assert "presets" in str(excinfo.value).lower()


def test_sandbox_suite_plan_file_hash_unchanged(tmp_path: Path) -> None:
    sandbox = tmp_path / "sandbox"
    sandbox.mkdir()
    plan = tmp_path / "plan.md"
    original = "# Plan\n"
    plan.write_text(original, encoding="utf-8")
    before = plan.read_bytes()
    sandbox_suite.generate_suite(
        sandbox=sandbox,
        preset="builtin:ce-executor-pipeline",
        plan_path=plan,
    )
    after = plan.read_bytes()
    assert before == after


def test_sandbox_suite_blocks_unwritable_sandbox(tmp_path: Path) -> None:
    # A non-existent directory must raise.
    missing = tmp_path / "no-such-dir"
    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")
    with pytest.raises(sandbox_suite.SandboxError):
        sandbox_suite.generate_suite(
            sandbox=missing,
            preset="builtin:ce-executor-pipeline",
            plan_path=plan,
        )


def test_derive_stem_for_builtin_and_file() -> None:
    assert (
        sandbox_suite.derive_stem("builtin:ce-executor-pipeline")
        == "ce-executor-pipeline"
    )
    assert (
        sandbox_suite.derive_stem("presets/en/ce-executor-pipeline.yml")
        == "ce-executor-pipeline"
    )
    assert sandbox_suite.derive_stem("/abs/path/my-team.yml") == "my-team"


# ---------------------------------------------------------------------------
# T3 — sandbox suite: strengthened cases
# ---------------------------------------------------------------------------


def test_launch_argv_excludes_dry_run(tmp_path: Path) -> None:
    """``launch_argv`` must NOT contain ``--dry-run``; ``argv`` must contain it."""
    sandbox = tmp_path / "sandbox"
    sandbox.mkdir()
    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")
    result = sandbox_suite.generate_suite(
        sandbox=sandbox,
        preset="builtin:ce-executor-pipeline",
        plan_path=plan,
    )
    assert "--dry-run" not in result.launch_argv
    assert "--dry-run" in result.argv


def test_disposition_write_conflict_raises_sandbox_error(tmp_path: Path) -> None:
    """Pre-write a file with valid 3-line header but WRONG SHA → ``SandboxError(write_conflict)``."""
    sandbox = tmp_path / "sandbox"
    sandbox.mkdir()
    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")

    # Pre-write with valid 3-line header format but WRONG SHA values.
    config_file = sandbox / "ralph.ce-executor-pipeline.yml"
    config_file.write_text(
        "# generated_by: ralph-e2e-bootstrap\n"
        "# profile_sha256: 0000000000000000000000000000000000000000000000000000000000000000\n"
        "# prompt_sha256: 1111111111111111111111111111111111111111111111111111111111111111\n"
        "# body content\n",
        encoding="utf-8",
    )

    with pytest.raises(sandbox_suite.SandboxError) as excinfo:
        sandbox_suite.generate_suite(
            sandbox=sandbox,
            preset="builtin:ce-executor-pipeline",
            plan_path=plan,
        )
    assert "write_conflict" in str(excinfo.value).lower()
    assert "provenance mismatch" in str(excinfo.value).lower()


def test_sandbox_is_file_rejected(tmp_path: Path) -> None:
    """Pass a regular file as ``sandbox`` → ``SandboxError``."""
    file_sandbox = tmp_path / "somefile"
    file_sandbox.write_text("not a directory\n", encoding="utf-8")
    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")

    with pytest.raises(sandbox_suite.SandboxError) as excinfo:
        sandbox_suite.generate_suite(
            sandbox=file_sandbox,
            preset="builtin:ce-executor-pipeline",
            plan_path=plan,
        )
    assert "not a directory" in str(excinfo.value).lower()


def test_mid_write_oserror(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Monkeypatch _atomic_pair_write to raise OSError → ``SandboxError``."""
    sandbox = tmp_path / "sandbox"
    sandbox.mkdir()
    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")

    original = sandbox_suite._atomic_pair_write

    def broken_write(*args, **kwargs):
        raise OSError("simulated disk error")

    monkeypatch.setattr(sandbox_suite, "_atomic_pair_write", broken_write)

    with pytest.raises(sandbox_suite.SandboxError) as excinfo:
        sandbox_suite.generate_suite(
            sandbox=sandbox,
            preset="builtin:ce-executor-pipeline",
            plan_path=plan,
        )
    assert "atomic write failed" in str(excinfo.value).lower()


def test_update_pair_restores_originals_on_second_half_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Half-failed update must restore both originals (not unlink them)."""
    sandbox = tmp_path / "sandbox"
    sandbox.mkdir()
    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")

    # First write succeeds — establishes owned pair.
    first = sandbox_suite.generate_suite(
        sandbox=sandbox,
        preset="builtin:ce-executor-pipeline",
        plan_path=plan,
    )
    config_path = Path(first.config_path)
    prompt_path = Path(first.prompt_path)
    original_config = config_path.read_bytes()
    original_prompt = prompt_path.read_bytes()

    # Simulate: config half replaces successfully, prompt half raises.
    # Bypass provenance gate so we exercise the restore path, not
    # write_conflict short-circuit.
    calls = {"n": 0}

    def flaky_write(path, payload, profile_sha256, prompt_sha256):  # noqa: ANN001, ARG001
        calls["n"] += 1
        if path.name.startswith("PROMPT."):
            raise OSError("simulated prompt write failure")
        # Successful half: overwrite in place (production uses
        # os.replace; byte-identity of the write is enough here).
        path.write_bytes(
            f"# generated_by: ralph-e2e-bootstrap\n"
            f"# profile_sha256: {profile_sha256}\n"
            f"# prompt_sha256: {prompt_sha256}\n".encode("utf-8")
            + payload
        )

    monkeypatch.setattr(sandbox_suite, "_atomic_write_with_provenance", flaky_write)

    with pytest.raises(OSError, match="simulated prompt write failure"):
        sandbox_suite._atomic_pair_write(
            config_path,
            prompt_path,
            b"# new config body\n",
            b"# new prompt body\n",
            profile_sha256="a" * 64,
            prompt_sha256="b" * 64,
            updated_pair=(config_path, prompt_path),
        )

    assert config_path.read_bytes() == original_config
    assert prompt_path.read_bytes() == original_prompt
    assert calls["n"] >= 2


def test_generate_suite_uses_resolved_binary(tmp_path: Path) -> None:
    """R6: suite argv/launch_argv must carry the resolved binary token."""
    sandbox = tmp_path / "sandbox"
    sandbox.mkdir()
    plan = tmp_path / "plan.md"
    plan.write_text("# Plan\n", encoding="utf-8")
    resolved = str(tmp_path / "target" / "debug" / "ralph")

    result = sandbox_suite.generate_suite(
        sandbox=sandbox,
        preset="builtin:ce-executor-pipeline",
        plan_path=plan,
        binary=resolved,
    )
    assert result.argv[0] == resolved
    assert result.launch_argv[0] == resolved
    assert result.argv[0] == result.launch_argv[0]


# ---------------------------------------------------------------------------
# Unit 5 — static gate + handoff
# ---------------------------------------------------------------------------


def test_static_gate_blocks_without_plan_path() -> None:
    report = gate.run_static_gate(
        binary="ralph",
        config_path="ralph.foo.yml",
        preset="builtin:ce-executor-pipeline",
        plan_path=None,
    )
    assert report.ok is False
    assert report.plan_required is True
    assert report.dry_run.outcome == "blocked_input"


def test_static_gate_runs_four_stages_with_fake_runner() -> None:
    argv_seen: list[tuple[str, ...]] = []

    def fake_runner(argv, **kwargs):  # noqa: ANN001, ARG001
        argv_seen.append(tuple(argv))
        return _ok_completed(argv, "fake 0.0.0")

    report = gate.run_static_gate(
        binary="ralph",
        config_path="ralph.foo.yml",
        preset="builtin:ce-executor-pipeline",
        plan_path="/abs/plan.md",
        runner=fake_runner,
    )
    # All four stages executed.
    assert report.capability.stage == "capability"
    assert report.preset_check.stage == "preset_check"
    assert report.preflight.stage == "preflight"
    assert report.dry_run.stage == "dry_run"


def test_static_gate_per_stage_argv_shape() -> None:
    """Assert per-stage argv shape: each stage has a distinct argv signature.

    The capability probe calls: --version, --help, --json --help,
    preset --help, preflight --help (no config).  validate_pipeline then
    calls preset check --strict and preflight --strict (with config).
    dry_run calls the full ralph -c cfg -H preset run --dry-run --plan path.
    """
    plan_path = "/abs/plan.md"
    cfg = "ralph.foo.yml"
    preset = "builtin:ce-executor-pipeline"
    invocations = [
        _probe_runner_common.version_probe_invocation("ralph"),
        _probe_runner_common.capability_probe_invocation("ralph"),
        # probe_capability also calls --json --help, preset --help, preflight --help:
        _probe_runner_common.FakeInvocation(
            argv_expected=("ralph", "--json", "--help"),
            stdout_chunks=("ralph --help\n  --json\n  --version\n",),
            stderr_chunks=(),
            exit_code=0,
        ),
        _probe_runner_common.FakeInvocation(
            argv_expected=("ralph", "preset", "check", "--help"),
            stdout_chunks=("ralph-preset\n\nUsage: ralph preset ...\n  --strict\n",),
            stderr_chunks=(),
            exit_code=0,
        ),
        _probe_runner_common.FakeInvocation(
            argv_expected=("ralph", "preflight", "--help"),
            stdout_chunks=("ralph-preflight\n\nUsage: ralph preflight ...\n  --strict\n",),
            stderr_chunks=(),
            exit_code=0,
        ),
        _probe_runner_common.FakeInvocation(
            argv_expected=("ralph", "run", "--help"),
            stdout_chunks=(
                "ralph-run\n\nUsage: ralph run ...\n  --dry-run\n  --plan PLAN\n",
            ),
            stderr_chunks=(),
            exit_code=0,
        ),
        _probe_runner_common.preset_check_ok_invocation("ralph", cfg, preset),
        _probe_runner_common.preflight_ok_invocation("ralph", cfg, preset),
        _probe_runner_common.dry_run_ok_invocation("ralph", cfg, preset, plan_path),
    ]
    runner = _probe_runner_common.e2e_make_runner(invocations)

    report = gate.run_static_gate(
        binary="ralph",
        config_path=cfg,
        preset=preset,
        plan_path=plan_path,
        runner=runner,
    )

    # capability: _build_stage_argv returns (binary, -c, cfg, -H, preset) for capability stage
    cap = report.capability
    assert cap.argv[:2] == ("ralph", "-c")
    assert "-H" in cap.argv
    assert "preset" not in cap.argv  # capability stage has no subcommand

    # preset_check: ralph -c cfg -H preset preset check --strict
    pc = report.preset_check
    assert pc.argv[:2] == ("ralph", "-c")
    assert "-H" in pc.argv
    assert "preset" in pc.argv

    # preflight: ralph -c cfg -H preset preflight --strict
    pf = report.preflight
    assert pf.argv[:2] == ("ralph", "-c")
    assert "-H" in pf.argv
    assert "preflight" in pf.argv
    assert "preflight" in pf.argv

    # dry_run: ralph -c cfg -H preset run --dry-run --plan <path>
    dr = report.dry_run
    assert dr.argv[:2] == ("ralph", "-c")
    assert "-H" in dr.argv
    assert "run" in dr.argv
    assert "--dry-run" in dr.argv
    assert "--plan" in dr.argv
    assert plan_path in dr.argv
    assert dr.outcome == "ok"


def test_static_gate_plan_path_none_returns_blocked_input() -> None:
    """plan_path=None → dry_run.outcome == 'blocked_input' and plan_required == True."""
    report = gate.run_static_gate(
        binary="ralph",
        config_path="ralph.foo.yml",
        preset="builtin:ce-executor-pipeline",
        plan_path=None,
    )
    assert report.dry_run.outcome == "blocked_input"
    assert report.plan_required is True


def test_handoff_static_only_command_shape(tmp_path: Path) -> None:
    inputs = handoff.HandoffInputs(
        binary="ralph",
        config_path="ralph.foo.yml",
        preset="builtin:ce-executor-pipeline",
        plan_path="/abs/plan.md",
        level="static_only",
        sandbox_path="sandbox",
        validation_evidence=("capability=ok", "preset_check=ok"),
        residual_risks=("live loop not run",),
        stage_outcomes=("capability=ok", "preset_check=ok", "preflight=ok", "dry_run=ok"),
    )
    artifact = handoff.build_handoff(inputs)
    assert artifact.level == "static_only"
    assert "ralph" in artifact.command
    assert "-c" in artifact.command
    assert "builtin:ce-executor-pipeline" in artifact.command
    assert "--plan" in artifact.command
    assert "Static load passed" in artifact.report
    assert "NOT closed" in artifact.report


def test_handoff_blocked_requires_summary() -> None:
    with pytest.raises(ValueError):
        handoff.HandoffInputs(
            binary="ralph",
            config_path="ralph.foo.yml",
            preset="builtin:ce-executor-pipeline",
            plan_path="/abs/plan.md",
            level="blocked",
            sandbox_path="sandbox",
            blocker_summary="",
        )


def test_handoff_blocked_command_empty() -> None:
    """level='blocked' → command == '' and command_argv == ()."""
    inputs = handoff.HandoffInputs(
        binary="ralph",
        config_path="ralph.foo.yml",
        preset="builtin:ce-executor-pipeline",
        plan_path="/abs/plan.md",
        level="blocked",
        sandbox_path="sandbox",
        blocker_summary="something went wrong",
        validation_evidence=(),
    )
    artifact = handoff.build_handoff(inputs)
    assert artifact.command == ""
    assert artifact.command_argv == ()


def test_handoff_abs_sandbox_raises() -> None:
    """sandbox_path='/abs/path' (absolute on POSIX) raises ValueError.

    Note: on macOS, Path('/abs/path').is_absolute() returns True for paths
    starting with '/'. On Windows it would be different. We use an absolute
    path that would fail on all platforms to ensure the guard fires.
    """
    with pytest.raises(ValueError) as excinfo:
        handoff.HandoffInputs(
            binary="ralph",
            config_path="ralph.foo.yml",
            preset="builtin:ce-executor-pipeline",
            plan_path="/abs/plan.md",
            level="static_only",
            sandbox_path="/abs/path",
        )
    assert "sandbox_path" in str(excinfo.value)


def test_handoff_blocker_markdown_injection_sanitised() -> None:
    """blocker_summary with ``](https://evil.example)`` → ``\]`` in rendered report."""
    inputs = handoff.HandoffInputs(
        binary="ralph",
        config_path="ralph.foo.yml",
        preset="builtin:ce-executor-pipeline",
        plan_path="/abs/plan.md",
        level="blocked",
        sandbox_path="sandbox",
        blocker_summary="see [link](https://evil.example)",
        validation_evidence=(),
    )
    artifact = handoff.build_handoff(inputs)
    # The link must be escaped to prevent markdown injection.
    assert r"\]" in artifact.report
    assert "https://evil.example" in artifact.report


def test_handoff_empty_validation_evidence() -> None:
    """validation_evidence=() → report contains ``(no static gate evidence recorded)``."""
    inputs = handoff.HandoffInputs(
        binary="ralph",
        config_path="ralph.foo.yml",
        preset="builtin:ce-executor-pipeline",
        plan_path="/abs/plan.md",
        level="static_only",
        sandbox_path="sandbox",
        validation_evidence=(),
    )
    artifact = handoff.build_handoff(inputs)
    assert "(no static gate evidence recorded)" in artifact.report


def test_handoff_notes_per_branch() -> None:
    """static_only vs blocked produce different ``notes`` tuples."""
    static_inputs = handoff.HandoffInputs(
        binary="ralph",
        config_path="ralph.foo.yml",
        preset="builtin:ce-executor-pipeline",
        plan_path="/abs/plan.md",
        level="static_only",
        sandbox_path="sandbox",
        validation_evidence=("capability=ok",),
    )
    static_artifact = handoff.build_handoff(static_inputs)

    blocked_inputs = handoff.HandoffInputs(
        binary="ralph",
        config_path="ralph.foo.yml",
        preset="builtin:ce-executor-pipeline",
        plan_path="/abs/plan.md",
        level="blocked",
        sandbox_path="sandbox",
        blocker_summary="gate failed",
        validation_evidence=(),
    )
    blocked_artifact = handoff.build_handoff(blocked_inputs)

    # Notes differ between the two branches.
    assert static_artifact.notes != blocked_artifact.notes
    # static_only notes mention "loop is NOT closed".
    assert any("NOT closed" in n for n in static_artifact.notes)
    # blocked notes mention "resolve the blocker".
    assert any("resolve the blocker" in n for n in blocked_artifact.notes)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


class _Completed:
    def __init__(self, stdout: str = "", stderr: str = "", returncode: int = 0) -> None:
        self.stdout = stdout
        self.stderr = stderr
        self.returncode = returncode


def _ok_completed(argv, stdout: str = "") -> _Completed:
    text = stdout
    if not text:
        if "--version" in argv:
            text = "ralph 0.0.0"
        elif "--help" in argv:
            # Capability probe needs the human-form help page to
            # contain the literal `--strict` / `--dry-run` tokens.
            sub = ""
            for token in ("preset", "preflight", "run"):
                if token in argv:
                    sub = token
                    break
            if sub == "preset":
                text = "ralph-preset\n\nUsage: ralph preset ...\n  --strict\n"
            elif sub == "preflight":
                text = "ralph-preflight\n\nUsage: ralph preflight ...\n  --strict\n"
            elif sub == "run":
                text = "ralph-run\n\nUsage: ralph run ...\n  --dry-run\n  --plan PLAN\n"
            else:
                text = "ralph --help\n  --json\n  --version\n"
        elif "preset" in argv and "check" in argv:
            text = "preset check OK"
        elif "preflight" in argv:
            text = "preflight OK"
        elif "run" in argv and "--dry-run" in argv:
            text = (
                "Dry run mode - configuration:\n"
                "  Backend: fake\n"
                "  Prompt file: PROMPT.foo.md\n"
                "  Max iterations: 1\n"
                "  Max runtime: 60\n"
            )
        else:
            text = "ok"
    return _Completed(stdout=text)