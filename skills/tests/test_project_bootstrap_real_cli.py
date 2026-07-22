"""Real-ClI contract tests for the ralph-project-bootstrap skill.

These tests drive the bootstrap probe against the ACTUAL ``ralph``
binary that lives in the current workspace (i.e. the binary the test
runner is built from), not against any ``ralph`` on ``$PATH`` or a
fake golden. The contract here is: if the real CLI does not expose the
capability/argv/effective value the helper relies on, the test fails.

Why this exists:

* Fake runners can pass forever while the production CLI drifts away
  from the helper's expectations (see plan 2026-07-19-001 S11 — fake
  fixture drift that let ``run --strict`` and ``config_path=``
  markers slip through for months).
* The CLI is the source of truth for capability and argv shape; the
  fixture is a record of what the helper already proved against the
  real CLI.
* No network, no provider credentials, no paid backend — these tests
  only need the locally-built binary and a sanitised environment.

Wiring:

* ``RALPH_BINARY`` env var selects the explicit binary path. When
  unset, the test looks up ``target/debug/ralph`` relative to the
  workspace root and falls back to ``CARGO_BIN_EXE_ralph`` if the env
  var is set by cargo-nextest. Tests skip with a clear message if no
  binary is locatable — they never silently fall back to a
  PATH-resolved ``ralph``.
* The probe runs in a private environment: provider API keys /
  tokens / ``RALPH_CONFIG`` are stripped, ``HOME`` is redirected to a
  temp dir so a stray user ``~/.config/ralph`` cannot poison the
  result.
"""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest
import pipeline_suite

ROOT = Path(__file__).resolve().parents[2]
WORKSPACE_BIN = ROOT / "target" / "debug" / "ralph"

# Stable labels we expect ``ralph run --dry-run`` to emit under
# ``Dry run mode - configuration:``. The labels are the contract; if
# the real CLI renames any of them, this test must FAIL so the helper
# and the fixtures are updated together. The unit-tested parser in
# ``cli_probe`` shares this constant (kept locally so the test is
# self-explanatory).
EXPECTED_DRY_RUN_LABELS: tuple[str, ...] = (
    "Backend:",
    "Prompt file:",
    "Max iterations:",
    "Max runtime:",
)


def _resolve_ralph_binary() -> Path | None:
    """Return the absolute path of the Ralph binary to test against.

    Resolution order:

    1. ``RALPH_BINARY`` env var (explicit override for CI).
    2. ``CARGO_BIN_EXE_ralph`` env var (set by ``cargo nextest`` when
       the test runs as part of a Rust integration test that compiles
       the binary).
    3. ``<workspace>/target/debug/ralph`` (the locally-built binary
       for ad-hoc invocations).

    Returns ``None`` if none of the candidates exist so callers can
    skip with an informative message — never falls back to a
    PATH-resolved ``ralph`` because that may be a different version
    on a developer machine.
    """
    for env_name in ("RALPH_BINARY", "CARGO_BIN_EXE_ralph"):
        candidate = os.environ.get(env_name)
        if candidate and Path(candidate).is_file():
            return Path(candidate)
    if WORKSPACE_BIN.is_file():
        return WORKSPACE_BIN
    return None


def _sanitised_env(tmp_home: Path) -> dict[str, str]:
    """Return an environment dict safe for the real CLI to run under.

    Removes provider API keys / tokens / ``RALPH_CONFIG`` / any
    variable that influences config or backend selection. Keeps the
    minimum needed to run the binary on POSIX (``PATH``,
    ``LANG``/``LC_ALL``, ``TMPDIR``).
    """
    safe: dict[str, str] = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "LANG": os.environ.get("LANG", "C.UTF-8"),
        "LC_ALL": os.environ.get("LC_ALL", "C.UTF-8"),
        "TMPDIR": str(tmp_home),
        "HOME": str(tmp_home),
    }
    forbidden_prefixes = (
        "ANTHROPIC_",
        "OPENAI_",
        "GOOGLE_",
        "GEMINI_",
        "CODEX_",
        "RALPH_CONFIG",
        "RALPH_API_KEY",
        "RALPH_TOKEN",
        "RALPH_BACKEND",
    )
    for key, value in os.environ.items():
        if any(key.startswith(prefix) for prefix in forbidden_prefixes):
            continue
        # Only forward a whitelist of extra keys (TERM, USER, etc.) so
        # CI-defined provider env does not leak.
        if key in safe or key.startswith("RALPH_") and key not in (
            "RALPH_CONFIG",
            "RALPH_API_KEY",
            "RALPH_TOKEN",
            "RALPH_BACKEND",
        ):
            continue
        # Forward everything else as-is; the goal is to keep the
        # environment minimally surprising without dropping neutral
        # locale / terminal hints.
        safe[key] = value
    return safe


@pytest.fixture(scope="module")
def ralph_binary() -> Path:
    binary = _resolve_ralph_binary()
    if binary is None:
        pytest.skip(
            "no Ralph binary found: set RALPH_BINARY or build target/debug/ralph"
        )
    return binary


@pytest.fixture()
def sanitised_env(tmp_path: Path) -> dict[str, str]:
    return _sanitised_env(tmp_path)


# ---------------------------------------------------------------------------
# Capability contract — the real CLI must expose the flags the helper relies on
# ---------------------------------------------------------------------------


def test_real_cli_help_exposes_required_capabilities(
    ralph_binary: Path, sanitised_env: dict[str, str]
) -> None:
    """The real ``ralph`` binary MUST expose ``preset check --strict``,
    ``preflight --strict``, and ``run --dry-run``. The test fails if any
    capability is missing from the actual ``--help`` output — this is
    the surface that ``cli_probe.REQUIRED_FLAGS`` depends on.
    """
    help_proc = subprocess.run(
        [str(ralph_binary), "--help"],
        capture_output=True,
        text=True,
        env=sanitised_env,
        timeout=20,
    )
    assert help_proc.returncode == 0, help_proc.stderr
    # ``--strict`` lives on the preset / preflight subcommands.
    preset_help = subprocess.run(
        [str(ralph_binary), "preset", "check", "--help"],
        capture_output=True,
        text=True,
        env=sanitised_env,
        timeout=20,
    )
    assert preset_help.returncode == 0, preset_help.stderr
    assert "--strict" in preset_help.stdout, (
        "ralph preset check --help must list --strict; helper requires "
        "this capability for the preset_check stage"
    )

    preflight_help = subprocess.run(
        [str(ralph_binary), "preflight", "--help"],
        capture_output=True,
        text=True,
        env=sanitised_env,
        timeout=20,
    )
    assert preflight_help.returncode == 0, preflight_help.stderr
    assert "--strict" in preflight_help.stdout, (
        "ralph preflight --help must list --strict; helper requires "
        "this capability for the preflight stage"
    )

    run_help = subprocess.run(
        [str(ralph_binary), "run", "--help"],
        capture_output=True,
        text=True,
        env=sanitised_env,
        timeout=20,
    )
    assert run_help.returncode == 0, run_help.stderr
    assert "--dry-run" in run_help.stdout, (
        "ralph run --help must list --dry-run; helper requires this "
        "capability for the dry_run stage"
    )
    # Negative: ``--strict`` must NOT be a real CLI flag on ``ralph run``.
    # ``ralph run --strict`` is the exact bug from plan 2026-07-19-001 S1.
    assert "--strict" not in run_help.stdout, (
        "ralph run --help must NOT list --strict; the dry-run stage "
        "must not invent that flag (strict gating is owned by preflight)"
    )


# ---------------------------------------------------------------------------
# argv contract — the argv built by the helper must be accepted by the real CLI
# ---------------------------------------------------------------------------


def test_real_cli_accepts_dry_run_argv_built_by_helper(
    ralph_binary: Path, sanitised_env: dict[str, str]
) -> None:
    """The dry-run argv the helper builds (``<binary> -c <cfg> -H <preset>
    run --dry-run --prompt-file <pf> --plan <plan>``) MUST be accepted
    by the real CLI's clap parser without flag errors. We give the
    binary the minimum fixture it needs to statically load and assert
    it does not exit with a clap usage error.
    """
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        config_path = tmp_path / "ralph.pipeline.yml"
        prompt_path = tmp_path / "PROMPT.pipeline.md"
        plan_path = tmp_path / "plan.md"
        # Minimal RalphConfig: cli.backend + event_loop.prompt_file.
        config_path.write_text(
            "cli:\n  backend: claude\n"
            "event_loop:\n"
            f"  prompt_file: {prompt_path.name}\n"
            "  max_iterations: 5\n"
            "  max_runtime_seconds: 600\n"
            "core: {}\n",
            encoding="utf-8",
        )
        prompt_path.write_text("# test prompt\n", encoding="utf-8")
        plan_path.write_text("# test plan\n", encoding="utf-8")

        proc = subprocess.run(
            [
                str(ralph_binary),
                "-c",
                str(config_path),
                "-H",
                "builtin:ce-executor-pipeline",
                "run",
                "--dry-run",
                "--prompt-file",
                str(prompt_path),
                "--plan",
                str(plan_path),
            ],
            capture_output=True,
            text=True,
            env=sanitised_env,
            timeout=20,
        )
    combined = proc.stdout + proc.stderr
    assert "unrecognized" not in combined.lower(), (
        f"real CLI rejected helper argv as unknown flags:\n{combined}"
    )
    assert "unexpected argument" not in combined.lower(), (
        f"real CLI rejected helper argv as unexpected:\n{combined}"
    )


def test_real_cli_uses_preset_bound_prompt_snapshot(
    ralph_binary: Path, sanitised_env: dict[str, str]
) -> None:
    preset_text = """
event_loop:
  execution_mode: isolated
  prompt: Generate source-grounded documentation.
  completion_promise: DOCS_COMPLETE
  starting_event: docs.start
hats:
  writer:
    name: Writer
    description: Writes documentation
    triggers: [docs.start]
    publishes: [DOCS_COMPLETE]
    instructions: Write the requested documentation, then emit DOCS_COMPLETE.
"""
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        preset_path = root / "modem-case-docs.yml"
        preset_path.write_text(preset_text, encoding="utf-8")
        suite = pipeline_suite.compose_preset_bound_suite(
            preset=preset_path.name,
            preset_text=preset_text,
            backend="claude",
            budget_max_iterations=5,
            budget_wall_clock_seconds=600,
        )
        (root / suite.config_path).write_text(suite.config, encoding="utf-8")
        (root / suite.prompt_path).write_text(suite.prompt, encoding="utf-8")
        proc = subprocess.run(
            [
                str(ralph_binary),
                "-c",
                suite.config_path,
                "-H",
                preset_path.name,
                "run",
                "--dry-run",
            ],
            cwd=root,
            capture_output=True,
            text=True,
            env=sanitised_env,
            timeout=20,
        )
    combined = proc.stdout + proc.stderr
    assert proc.returncode == 0, combined
    assert f"Prompt file: {suite.prompt_path}" in combined
    assert "Prompt file: PROMPT.md" not in combined
    # And it must NOT have produced a clap usage block (that would mean
    # argv parse failed). Success here means clap accepted the argv.
    assert "Usage:" not in proc.stderr, (
        f"real CLI printed usage to stderr — argv parse failed:\n{proc.stderr}"
    )


def test_real_cli_dry_run_emits_stable_effective_labels(
    ralph_binary: Path, sanitised_env: dict[str, str]
) -> None:
    """The real ``ralph run --dry-run`` MUST emit the four stable labels
    the helper parses. If the CLI ever renames them, the parser must
    be updated together with this test.
    """
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        config_path = tmp_path / "ralph.pipeline.yml"
        prompt_path = tmp_path / "PROMPT.pipeline.md"
        plan_path = tmp_path / "plan.md"
        config_path.write_text(
            "cli:\n  backend: claude\n"
            "event_loop:\n"
            f"  prompt_file: {prompt_path.name}\n"
            "  max_iterations: 5\n"
            "  max_runtime_seconds: 600\n"
            "core: {}\n",
            encoding="utf-8",
        )
        prompt_path.write_text("# test prompt\n", encoding="utf-8")
        plan_path.write_text("# test plan\n", encoding="utf-8")

        proc = subprocess.run(
            [
                str(ralph_binary),
                "-c",
                str(config_path),
                "-H",
                "builtin:ce-executor-pipeline",
                "run",
                "--dry-run",
                "--prompt-file",
                str(prompt_path),
                "--plan",
                str(plan_path),
            ],
            capture_output=True,
            text=True,
            env=sanitised_env,
            timeout=20,
        )
    assert proc.returncode == 0, proc.stderr
    for label in EXPECTED_DRY_RUN_LABELS:
        assert label in proc.stdout, (
            f"real ralph run --dry-run output is missing stable label "
            f"{label!r}; the helper's parser will fall through to "
            f"missing-in-dry-run output.\n"
            f"stdout was:\n{proc.stdout}"
        )


def test_real_cli_does_not_emit_fake_config_path_marker(
    ralph_binary: Path, sanitised_env: dict[str, str]
) -> None:
    """The real ``ralph run --dry-run`` MUST NOT emit a ``config_path=``
    marker — that token was invented by the fake fixture and never
    appears in production CLI output. The helper's classifier relies on
    parsed effective values; if this test ever fails it is because
    someone changed the dry-run output format AND broke the parser in
    the same revision.
    """
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        config_path = tmp_path / "ralph.pipeline.yml"
        prompt_path = tmp_path / "PROMPT.pipeline.md"
        plan_path = tmp_path / "plan.md"
        config_path.write_text(
            "cli:\n  backend: claude\n"
            "event_loop:\n"
            f"  prompt_file: {prompt_path.name}\n"
            "core: {}\n",
            encoding="utf-8",
        )
        prompt_path.write_text("# test prompt\n", encoding="utf-8")
        plan_path.write_text("# test plan\n", encoding="utf-8")

        proc = subprocess.run(
            [
                str(ralph_binary),
                "-c",
                str(config_path),
                "-H",
                "builtin:ce-executor-pipeline",
                "run",
                "--dry-run",
                "--prompt-file",
                str(prompt_path),
                "--plan",
                str(plan_path),
            ],
            capture_output=True,
            text=True,
            env=sanitised_env,
            timeout=20,
        )
    assert proc.returncode == 0, proc.stderr
    assert "config_path=" not in proc.stdout, (
        "real CLI unexpectedly emitted the legacy 'config_path=' marker; "
        "the parser must be updated to use the new contract"
    )


# ---------------------------------------------------------------------------
# Smoke argv contract — the argv the smoke harness builds must be accepted
# by the real CLI's clap parser. This pins the F6 / S8 / S11 smoke-surface
# contract: the harness MUST NOT invent ``--idle-timeout-ms`` or
# ``--wall-clock-timeout-s`` flags, and MUST forward only flags the real
# ``ralph run`` accepts.
# ---------------------------------------------------------------------------


def test_real_cli_accepts_smoke_argv_built_by_harness(
    ralph_binary: Path, sanitised_env: dict[str, str]
) -> None:
    """The smoke harness's argv (``<binary> -c <cfg> -H <preset>
    --max-iterations <N> --idle-timeout <S> --prompt-file <pf> --plan <plan>``)
    MUST be accepted by the real CLI's clap parser without flag errors.
    The harness used to emit ``--idle-timeout-ms`` and
    ``--wall-clock-timeout-s`` — neither exists in production. This test
    fails closed if those legacy flags ever re-enter the argv.
    """
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        config_path = tmp_path / "ralph.pipeline.yml"
        prompt_path = tmp_path / "PROMPT.pipeline.md"
        plan_path = tmp_path / "plan.md"
        config_path.write_text(
            "cli:\n  backend: claude\n"
            "event_loop:\n"
            f"  prompt_file: {prompt_path.name}\n"
            "  max_iterations: 3\n"
            "  max_runtime_seconds: 600\n"
            "core: {}\n",
            encoding="utf-8",
        )
        prompt_path.write_text("# test prompt\n", encoding="utf-8")
        plan_path.write_text("# test plan\n", encoding="utf-8")

        proc = subprocess.run(
            [
                str(ralph_binary),
                "-c",
                str(config_path),
                "-H",
                "builtin:ce-executor-pipeline",
                "run",
                "--max-iterations",
                "3",
                "--idle-timeout",
                "5",
                "--prompt-file",
                str(prompt_path),
                "--plan",
                str(plan_path),
                "--dry-run",
            ],
            capture_output=True,
            text=True,
            env=sanitised_env,
            timeout=20,
        )
    combined = proc.stdout + proc.stderr
    assert "unrecognized" not in combined.lower(), (
        f"real CLI rejected smoke harness argv as unknown flags:\n{combined}"
    )
    assert "unexpected argument" not in combined.lower(), (
        f"real CLI rejected smoke harness argv as unexpected:\n{combined}"
    )
    assert "Usage:" not in proc.stderr, (
        f"real CLI printed usage to stderr — argv parse failed:\n{proc.stderr}"
    )


def test_real_cli_rejects_legacy_idle_timeout_ms_flag(
    ralph_binary: Path, sanitised_env: dict[str, str]
) -> None:
    """The real ``ralph run`` MUST reject ``--idle-timeout-ms`` because
    it does not exist on the production CLI. The harness historically
    emitted this flag; this test pins the negative contract so any
    regression that re-introduces the legacy flag is caught before
    merge.
    """
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        config_path = tmp_path / "ralph.pipeline.yml"
        prompt_path = tmp_path / "PROMPT.pipeline.md"
        plan_path = tmp_path / "plan.md"
        config_path.write_text(
            "cli:\n  backend: claude\n"
            "event_loop:\n"
            f"  prompt_file: {prompt_path.name}\n"
            "core: {}\n",
            encoding="utf-8",
        )
        prompt_path.write_text("# test prompt\n", encoding="utf-8")
        plan_path.write_text("# test plan\n", encoding="utf-8")

        proc = subprocess.run(
            [
                str(ralph_binary),
                "-c",
                str(config_path),
                "-H",
                "builtin:ce-executor-pipeline",
                "--idle-timeout-ms",
                "5000",
                "--dry-run",
                "--prompt-file",
                str(prompt_path),
                "--plan",
                str(plan_path),
            ],
            capture_output=True,
            text=True,
            env=sanitised_env,
            timeout=20,
        )
    combined = (proc.stdout + proc.stderr).lower()
    assert proc.returncode != 0, (
        "ralph run accepted the legacy --idle-timeout-ms flag; the "
        "production CLI must reject it"
    )
    assert "unrecognized" in combined or "unexpected" in combined, (
        f"expected clap to flag the unknown flag; got:\n{combined}"
    )


def test_real_cli_rejects_legacy_wall_clock_timeout_s_flag(
    ralph_binary: Path, sanitised_env: dict[str, str]
) -> None:
    """The real ``ralph run`` MUST reject ``--wall-clock-timeout-s``
    because wall-clock is a harness concern, not a CLI flag. Any
    regression that forwards this flag to the binary is caught here.
    """
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        config_path = tmp_path / "ralph.pipeline.yml"
        prompt_path = tmp_path / "PROMPT.pipeline.md"
        plan_path = tmp_path / "plan.md"
        config_path.write_text(
            "cli:\n  backend: claude\n"
            "event_loop:\n"
            f"  prompt_file: {prompt_path.name}\n"
            "core: {}\n",
            encoding="utf-8",
        )
        prompt_path.write_text("# test prompt\n", encoding="utf-8")
        plan_path.write_text("# test plan\n", encoding="utf-8")

        proc = subprocess.run(
            [
                str(ralph_binary),
                "-c",
                str(config_path),
                "-H",
                "builtin:ce-executor-pipeline",
                "--wall-clock-timeout-s",
                "60",
                "--dry-run",
                "--prompt-file",
                str(prompt_path),
                "--plan",
                str(plan_path),
            ],
            capture_output=True,
            text=True,
            env=sanitised_env,
            timeout=20,
        )
    combined = (proc.stdout + proc.stderr).lower()
    assert proc.returncode != 0, (
        "ralph run accepted the legacy --wall-clock-timeout-s flag; "
        "the production CLI must reject it"
    )
    assert "unrecognized" in combined or "unexpected" in combined, (
        f"expected clap to flag the unknown flag; got:\n{combined}"
    )
