"""CLI capability probe + staged static-validation state machine.

The bootstrap pipeline only trusts the local ``ralph`` binary when its
surface matches a canonical contract. This module owns:

* ``probe_capability`` — observes the binary's ``--version`` / ``--help``
  output and reports which of the required command/flag combinations are
  present.
* ``validate_pipeline`` — runs the four-stage static gate in strict order
  (``capability`` → ``preset check --strict`` → ``preflight --strict`` →
  ``run --dry-run``) and classifies each stage's outcome into a
  blocker category.
* ``load_fixture`` — reads a fixture directory produced by the test
  authoring helpers into a list of ``FakeInvocation`` records the fake
  runner can replay.

The module is pure stdlib. It NEVER spawns the real ``ralph`` binary at
import time. Every subprocess call lives inside ``probe_capability`` /
``validate_pipeline`` and is overridable through the ``runner`` argument
so the test suite can drive the state machine with a deterministic fake.

Public surface (everything else is private):

* ``CapabilityReport`` — the result of probing the binary surface.
* ``StageDecision`` — one row of the staged state machine.
* ``REQUIRED_FLAGS`` — the literal expected command/flag combinations.
* ``probe_capability(binary)`` — return a ``CapabilityReport``.
* ``validate_pipeline(..., runner=...)`` — return a tuple of four
  ``StageDecision`` records (capability, preset_check, preflight,
  dry_run).
* ``load_fixture(name)`` — load a fixture into ``FakeInvocation`` rows.

Hard rules:

* All subprocess argv tuples MUST contain ``-c <config_path>`` and
  ``-H <preset>``; the dry-run argv additionally carries ``--dry-run``
  (only).
* The capability gate never throws: missing binary / missing flag /
  timeout / nonzero exit all populate ``flags_missing`` or block a
  stage, never raise.
* ``backend`` errors reported by ``preflight --strict`` translate to
  ``blocked_backend``; effective-config mismatch in ``run --dry-run``
  translates to ``blocked_command``; everything else non-backend is
  ``blocked_cli`` (preset/preflight) or ``blocked_command`` (dry-run).
* The dry-run stage's source/effective proof comes from the explicit
  ``-c <config_path> -H <preset>`` argv AND parsed effective-value
  labels (``backend:`` / ``prompt_file:`` / ``max_iterations:`` /
  ``max_runtime_seconds:``) emitted by the real ``ralph run --dry-run``
  human-readable output. We do NOT search for fake markers like
  ``config_path=...`` because the real CLI does not emit that token.
* Strict mode is owned by ``ralph preset check --strict`` and
  ``ralph preflight --strict``; ``ralph run`` does not accept
  ``--strict``. ``run --dry-run`` therefore never carries that flag.
"""
from __future__ import annotations

import json
import re
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Iterable

# Canonical command+flag combinations the bootstrap pipeline needs.
# The literal set is the single source of truth used by both
# ``probe_capability`` (to populate ``flags_missing``) and the staged
# gate (to decide whether to short-circuit on capability). Each entry
# is the public capability id; the probe translates each id into one
# or more concrete flag-presence checks against the observed help
# output.
#
# Strict mode is split across two subcommands. ``ralph run`` does NOT
# accept ``--strict``; dry-run relies on the strict preflight stage
# to gate bad config. We therefore do not require ``run --strict``
# here. (See plan 2026-07-19-001 S1 / S2.)
REQUIRED_FLAGS: frozenset[str] = frozenset(
    {
        "preset check --strict",
        "preflight --strict",
        "run --dry-run",
    }
)

# Mapping from capability id to the concrete flag tokens the probe
# must observe in the relevant help page. The probe also records
# whether ``--dry-run`` exists on ``ralph run --help`` separately
# because ``run --dry-run`` is its own capability row.
_CAPABILITY_FLAG_TOKENS: dict[str, tuple[str, ...]] = {
    "preset check --strict": ("--strict",),
    "preflight --strict": ("--strict",),
    "run --dry-run": ("--dry-run",),
}

# Which subcommand's help page to inspect for each capability id.
_CAPABILITY_HELP_TARGET: dict[str, str] = {
    "preset check --strict": "preset check",
    "preflight --strict": "preflight",
    "run --dry-run": "run",
}

# Best-effort detection of ``--json --help`` support so we can populate
# ``json_supported`` / ``human_fallback_observed`` from real evidence
# rather than guessing by semver.
JSON_FLAG = "--json"

# Default probe timeout for every subprocess call. Real CLI startup
# is well below 20s; the bound is generous so flaky CI does not flake
# while still failing fast on a runaway binary.
DEFAULT_TIMEOUT = 20.0


@dataclass(frozen=True)
class CapabilityReport:
    """Observed surface of the ``ralph`` binary.

    ``flags_present`` and ``flags_missing`` are derived from
    ``--help`` output, never from semver guessing. When the binary is
    missing entirely, ``version`` is the literal ``"missing"`` and both
    flag sets are empty.
    """

    binary: Path
    version: str
    flags_present: frozenset[str]
    flags_missing: frozenset[str]
    json_supported: bool
    human_fallback_observed: bool
    run_dry_run_supported: bool


@dataclass(frozen=True)
class FakeInvocation:
    """One recorded subprocess call replayed by the fixture runner."""

    argv_expected: tuple[str, ...]
    stdout_chunks: tuple[str, ...]
    stderr_chunks: tuple[str, ...]
    exit_code: int


@dataclass(frozen=True)
class StageDecision:
    """One row of the staged static-validation state machine.

    ``argv`` is the argv tuple the helper would have executed. For
    skipped stages the argv is still emitted so callers can inspect
    what *would* have been run.

    ``next_allowed_stage`` is the stage the gate advances to when the
    current stage is ``ok``; it is ``None`` when the current stage is
    a blocker or has been skipped.
    """

    stage: str  # one of: capability, preset_check, preflight, dry_run
    outcome: str  # one of: ok, blocked_cli, blocked_preset,
                  #      blocked_backend, blocked_command, blocked_input,
                  #      blocked_unknown
    evidence: tuple[str, ...]
    next_allowed_stage: str | None
    blocked_reason: str
    argv: tuple[str, ...]


# ---------------------------------------------------------------------------
# Internal helpers — subprocess + text parsing
# ---------------------------------------------------------------------------


def _safe_run(
    runner: Callable[..., subprocess.CompletedProcess],
    argv: tuple[str, ...],
) -> subprocess.CompletedProcess | None:
    """Invoke ``runner`` with a deterministic timeout.

    Returns ``None`` on ``FileNotFoundError`` / ``OSError`` /
    ``TimeoutExpired`` so the probe can record the surface as
    incomplete without raising. The staged gate has its own
    invocation path (``_invoke``) that *does* surface
    ``TimeoutExpired`` as ``blocked_unknown`` so callers see the
    timeout instead of a silent skip.
    """
    try:
        return runner(
            list(argv),
            timeout=DEFAULT_TIMEOUT,
            capture_output=True,
            text=True,
        )
    except (FileNotFoundError, OSError, subprocess.TimeoutExpired):
        return None


def _coerce_completed(value: Any) -> subprocess.CompletedProcess:
    """Best-effort coercion of an injected runner result.

    Tests may pass either a real ``subprocess.CompletedProcess`` or a
    duck-typed stub. We only access attributes we actually need.
    """
    stdout = getattr(value, "stdout", "") or ""
    stderr = getattr(value, "stderr", "") or ""
    returncode = getattr(value, "returncode", 0)
    return subprocess.CompletedProcess(
        args=(),
        returncode=int(returncode),
        stdout=str(stdout),
        stderr=str(stderr),
    )


def _parse_flags(help_text: str, capabilities: Iterable[str]) -> frozenset[str]:
    """Return the subset of ``capabilities`` whose flag tokens appear
    in ``help_text``.

    Each capability id maps to one or more concrete flag tokens; every
    token must appear in ``help_text`` as its own word for the
    capability to be considered present. We check the literal token
    on a per-line basis so prose paragraphs do not produce false
    positives.
    """
    found: set[str] = set()
    lines = help_text.splitlines()
    for capability in capabilities:
        tokens = _CAPABILITY_FLAG_TOKENS.get(capability, ())
        if not tokens:
            continue
        all_present = True
        for token in tokens:
            pattern = r"(^|\s)" + re.escape(token) + r"(\s|$)"
            token_seen = False
            for line in lines:
                if re.search(pattern, line.strip()):
                    token_seen = True
                    break
            if not token_seen:
                all_present = False
                break
        if all_present:
            found.add(capability)
    return frozenset(found)


def _parse_json_support(help_text: str) -> bool:
    """True iff ``--json`` appears as its own token in ``help_text``."""
    for line in help_text.splitlines():
        if re.search(r"(^|\s)" + re.escape(JSON_FLAG) + r"(\s|$)", line.strip()):
            return True
    return False


def _read_version(version_text: str) -> str:
    """Extract a sensible ``version`` string from ``ralph --version``.

    Falls back to the trimmed first non-empty line, then to ``"unknown"``
    when the binary produced no output.
    """
    text = version_text.strip()
    if not text:
        return "unknown"
    first = text.splitlines()[0].strip()
    return first or "unknown"


# ---------------------------------------------------------------------------
# Public API — capability probe
# ---------------------------------------------------------------------------


def probe_capability(
    binary: Path | str = Path("ralph"),
    *,
    runner: Callable[..., subprocess.CompletedProcess] | None = None,
) -> CapabilityReport:
    """Observe the binary's surface and classify required flags.

    The probe NEVER throws. When the binary is missing or no subprocess
    call succeeds, the returned ``CapabilityReport`` carries
    ``version="missing"`` and every required flag in ``flags_missing``.

    The probe invokes, in order: ``<binary> --version``,
    ``<binary> --help``, ``<binary> --json --help``,
    ``<binary> preset check --help``, ``<binary> preflight --help``,
    ``<binary> run --help``, and ``<binary> run --dry-run --help``.
    Each call is bounded by ``DEFAULT_TIMEOUT`` (20 s).
    """
    binary_path = Path(binary)
    run = runner if runner is not None else subprocess.run

    version = "missing"
    flags_present: set[str] = set()
    json_supported = False
    human_fallback_observed = False
    run_dry_run_supported = False

    # 1. --version (informational only).
    version_proc = _safe_run(run, (str(binary_path), "--version"))
    if version_proc is not None:
        completed = _coerce_completed(version_proc)
        if completed.returncode == 0:
            version = _read_version(completed.stdout)
        else:
            version = "error"

    # 2. --help (human-form). Even when JSON is supported, the human
    # form is the authoritative source for the flag inventory.
    help_proc = _safe_run(run, (str(binary_path), "--help"))
    if help_proc is not None:
        completed = _coerce_completed(help_proc)
        if completed.returncode == 0:
            human_fallback_observed = True

    # 3. --json --help (machine form). When JSON is supported we
    # double-check the flag inventory there as well. When JSON is NOT
    # supported we leave ``json_supported=False`` and keep the human
    # form as our source of truth.
    json_help_proc = _safe_run(run, (str(binary_path), JSON_FLAG, "--help"))
    if json_help_proc is not None:
        completed = _coerce_completed(json_help_proc)
        if completed.returncode == 0:
            json_supported = _parse_json_support(completed.stdout)
        else:
            # JSON probe failed but the human probe may have succeeded;
            # record the fallback so downstream stages can warn.
            human_fallback_observed = human_fallback_observed or True

    # 4. Per-subcommand help pages. Each capability maps to a target
    # subcommand; the probe inspects that subcommand's help page for
    # the capability's flag tokens.
    for capability, target in _CAPABILITY_HELP_TARGET.items():
        sub_proc = _safe_run(run, (str(binary_path), *target.split(), "--help"))
        if sub_proc is None:
            continue
        completed = _coerce_completed(sub_proc)
        if completed.returncode != 0:
            continue
        found_for_sub = _parse_flags(completed.stdout, (capability,))
        flags_present.update(found_for_sub)
        if capability == "run --dry-run":
            run_dry_run_supported = (
                run_dry_run_supported or bool(found_for_sub)
            )

    flags_missing = REQUIRED_FLAGS - flags_present

    # If the binary is missing entirely, the synthetic report must
    # show every required flag as missing (in case a caller only
    # inspects ``flags_missing``).
    if version == "missing":
        return CapabilityReport(
            binary=binary_path,
            version="missing",
            flags_present=frozenset(),
            flags_missing=frozenset(REQUIRED_FLAGS),
            json_supported=False,
            human_fallback_observed=False,
            run_dry_run_supported=False,
        )

    return CapabilityReport(
        binary=binary_path,
        version=version,
        flags_present=frozenset(flags_present),
        flags_missing=frozenset(flags_missing),
        json_supported=json_supported,
        human_fallback_observed=human_fallback_observed,
        run_dry_run_supported=run_dry_run_supported,
    )


# ---------------------------------------------------------------------------
# Public API — staged state machine
# ---------------------------------------------------------------------------


def _build_stage_argv(
    stage: str,
    *,
    binary: Path | str,
    config_path: str,
    preset: str,
    prompt_file: str | None,
    plan_path: str | None,
) -> tuple[str, ...]:
    """Compose the argv tuple for a stage invocation.

    Every argv starts with the binary, then ``-c <config_path>`` and
    ``-H <preset>``. The ``dry_run`` stage additionally carries
    ``--dry-run`` only — ``ralph run`` does not accept ``--strict``;
    strict gating lives in the dedicated ``preflight --strict`` stage.
    The dry-run may carry ``--prompt-file`` and/or ``--plan``. When both
    are present, Ralph uses ``--prompt-file`` as the agent-visible prompt
    and ``--plan`` as the workload / worktree key.

    The contract is enforced by the test suite: every argv the helper
    builds must contain ``-c <config_path>`` and ``-H <preset>``; the
    dry-run argv must additionally contain ``--dry-run`` and must NOT
    contain ``--strict``.
    """
    base = (
        str(binary),
        "-c",
        config_path,
        "-H",
        preset,
    )
    if stage == "capability":
        return base
    if stage == "preset_check":
        return base + ("preset", "check", "--strict")
    if stage == "preflight":
        return base + ("preflight", "--strict")
    if stage == "dry_run":
        argv = base + ("run", "--dry-run")
        # Prefer emitting both when supplied: ``--prompt-file`` is the
        # agent-visible prompt (wins over ``--plan`` inside ``ralph run``),
        # while ``--plan`` remains the workload identity / worktree key.
        # Callers that pass only one keep the historical single-source shape.
        if prompt_file:
            argv = argv + ("--prompt-file", prompt_file)
        if plan_path:
            argv = argv + ("--plan", plan_path)
        return argv
    raise ValueError(f"unknown stage: {stage!r}")


def _classify_preflight_stderr(stderr: str) -> tuple[str, str]:
    """Best-effort classification of a preflight ``--strict`` failure.

    Returns ``(outcome, reason)`` where ``outcome`` is one of
    ``blocked_backend`` / ``blocked_cli``. Backend-class signals
    include missing executables, unknown backends, and auth-readiness
    failures; everything else is ``blocked_cli``.
    """
    backend_patterns = (
        r"unknown backend",
        r"executable not found",
        r"auth not ready",
        r"auth .* missing",
        r"backend .* missing",
        r"backend .* not available",
    )
    for pattern in backend_patterns:
        match = re.search(pattern, stderr, re.IGNORECASE)
        if match is not None:
            reason = match.group(0).strip()
            return "blocked_backend", f"backend-class failure: {reason}"
    return "blocked_cli", stderr.strip().splitlines()[0] if stderr.strip() else "preflight failed"


def _classify_preset_stderr(stderr: str) -> str:
    """Return the human-readable reason for a preset strict failure."""
    stripped = stderr.strip()
    if not stripped:
        return "preset check --strict returned non-zero"
    first = stripped.splitlines()[0]
    return first


# Stable labels emitted by ``ralph run --dry-run``'s human-readable
# configuration block. The contract relies on these labels being
# produced by the real CLI rather than on a fake JSON / dotenv marker
# we invent here. The probes accept any non-empty value next to the
# label and pass the captured evidence through unchanged.
_DRY_RUN_LABEL_PATTERNS: tuple[tuple[str, str], ...] = (
    (r"^\s*Backend:\s*(\S.*)$", "backend"),
    (r"^\s*Prompt file:\s*(\S.*)$", "prompt_file"),
    (r"^\s*Max iterations:\s*(\S+)$", "max_iterations"),
    (r"^\s*Max runtime:\s*(\S+)$", "max_runtime"),
)


def _parse_dry_run_effective_values(stdout: str) -> dict[str, str]:
    """Parse the stable ``label: value`` lines from a real ``ralph run
    --dry-run`` invocation.

    The real CLI emits a human-readable configuration block under
    ``Dry run mode - configuration:`` where every line has the form
    ``Label: value``. We only look at the four labels we care about
    (``Backend`` / ``Prompt file`` / ``Max iterations`` / ``Max runtime``)
    and ignore everything else. Missing labels do NOT cause failure
    here — they propagate to the caller which can decide whether the
    label set is sufficient for the proof level it is targeting.
    """
    values: dict[str, str] = {}
    for line in stdout.splitlines():
        for pattern, key in _DRY_RUN_LABEL_PATTERNS:
            match = re.match(pattern, line)
            if match is None:
                continue
            values[key] = match.group(1).strip()
            break
    return values


def _classify_dry_run(
    stdout: str, stderr: str, expected: dict[str, str]
) -> tuple[str, str, dict[str, str]]:
    """Classify a dry-run stage outcome based on parsed effective values.

    Returns ``(outcome, reason, evidence)`` where ``evidence`` is the
    raw ``label: value`` map parsed from the dry-run stdout. A passing
    dry-run means the runtime successfully *statically loaded* the
    config / preset / prompt / backend detection / preflight. It is NOT
    a loop-closed claim.

    The contract: a mismatch between any *expected* label (from the
    pipeline suite) and the *parsed* label from stdout must surface as
    ``blocked_command`` so callers cannot mistake "the binary printed
    something" for "the binary used the suite we asked for".
    """
    parsed = _parse_dry_run_effective_values(stdout)
    mismatches: list[str] = []
    for key, expected_value in expected.items():
        if not expected_value:
            continue
        if key not in parsed:
            mismatches.append(f"{key}: missing in dry-run output")
            continue
        if parsed[key] != expected_value:
            mismatches.append(
                f"{key}: dry-run reports {parsed[key]!r}, expected {expected_value!r}"
            )
    if mismatches:
        return (
            "blocked_command",
            "dry-run effective values do not match suite: " + "; ".join(mismatches),
            parsed,
        )
    return ("ok", "static load passed; loop not closed", parsed)


def _ok_decision(
    stage: str,
    next_stage: str | None,
    evidence: Iterable[str],
    argv: tuple[str, ...],
) -> StageDecision:
    return StageDecision(
        stage=stage,
        outcome="ok",
        evidence=tuple(evidence),
        next_allowed_stage=next_stage,
        blocked_reason="",
        argv=argv,
    )


def _block_decision(
    stage: str,
    outcome: str,
    reason: str,
    evidence: Iterable[str],
    argv: tuple[str, ...],
) -> StageDecision:
    return StageDecision(
        stage=stage,
        outcome=outcome,
        evidence=tuple(evidence),
        next_allowed_stage=None,
        blocked_reason=reason,
        argv=argv,
    )


def _skip_decision(
    stage: str,
    argv: tuple[str, ...],
    reason: str,
) -> StageDecision:
    return StageDecision(
        stage=stage,
        outcome="blocked_unknown",
        evidence=(f"skipped: {reason}",),
        next_allowed_stage=None,
        blocked_reason=reason,
        argv=argv,
    )


def _invoke(
    runner: Callable[..., subprocess.CompletedProcess],
    argv: tuple[str, ...],
) -> tuple[str, str, int] | str:
    """Run ``argv`` and return ``(stdout, stderr, exit_code)`` or
    the literal ``"timeout"`` / ``"missing"`` sentinel."""
    try:
        result = runner(list(argv), timeout=DEFAULT_TIMEOUT, capture_output=True, text=True)
    except subprocess.TimeoutExpired:
        return "timeout"
    except FileNotFoundError:
        return "missing"
    except OSError:
        return "missing"
    completed = _coerce_completed(result)
    return completed.stdout, completed.stderr, completed.returncode


# Stage names in pipeline order — the iteration source of truth for
# skip-after-block branches inside ``validate_pipeline``. Using a
# module-level constant prevents per-call reallocation and keeps the
# ordered tuple-style Literal / dict-key comparison cheap.
_PIPELINE_STAGES: tuple[str, ...] = ("capability", "preset_check", "preflight", "dry_run")


def _argv_for(stage: str, argv_by_stage: dict[str, tuple[str, ...]]) -> tuple[str, ...]:
    """Return the precomputed argv tuple for ``stage``.

    ``argv_by_stage`` is built once per ``validate_pipeline`` call so
    every block + skip branch collapses to a dictionary lookup instead
    of rebuilding the same argv tuple from scratch. We re-raise the
    underlying ``KeyError`` as ``ValueError`` to match ``_build_stage_argv``'s
    "unknown stage" contract that public callers may rely on.
    """
    try:
        return argv_by_stage[stage]
    except KeyError as exc:  # pragma: no cover - invariant guard
        raise ValueError(f"unknown stage: {stage!r}") from exc


def _record_block_then_skip(
    decisions: list[StageDecision],
    argv_by_stage: dict[str, tuple[str, ...]],
    *,
    block_stage: str,
    outcome: str,
    reason: str,
    evidence: Iterable[str],
    skip_stages: Iterable[str],
    skip_reason: str,
) -> None:
    """Append a blocker decision followed by one skip decision per stage.

    The 13 ``_block_decision`` + 9 ``_skip_decision`` pattern inside
    ``validate_pipeline`` collapses to a single helper call: record a
    blocker for ``block_stage`` and a skip for every stage listed in
    ``skip_stages``. ``argv`` for each row comes from the precomputed
    ``argv_by_stage`` map, so per-branch string composition is gone.
    """
    decisions.append(
        _block_decision(
            stage=block_stage,
            outcome=outcome,
            reason=reason,
            evidence=evidence,
            argv=_argv_for(block_stage, argv_by_stage),
        )
    )
    for stage in skip_stages:
        decisions.append(
            _skip_decision(stage, _argv_for(stage, argv_by_stage), skip_reason)
        )


def validate_pipeline(
    *,
    binary: Path | str,
    config_path: str,
    preset: str,
    prompt_file: str | None = None,
    plan_path: str | None = None,
    runner: Callable[..., subprocess.CompletedProcess] | None = None,
) -> tuple[StageDecision, ...]:
    """Run the four-stage static gate and return one decision per stage.

    Stages run in strict order: ``capability`` → ``preset_check`` →
    ``preflight`` → ``dry_run``. On the first blocker every subsequent
    stage is recorded as skipped (``outcome="blocked_unknown"``,
    ``next_allowed_stage=None``, evidence noting the upstream
    blocker).

    The runner injection point lets tests drive the state machine
    with a fake without ever spawning the real binary.
    """
    binary_path = Path(binary)
    run = runner if runner is not None else subprocess.run
    decisions: list[StageDecision] = []

    # Build every stage's argv once so block + skip branches become
    # cheap dict lookups instead of re-running ``_build_stage_argv``.
    argv_by_stage: dict[str, tuple[str, ...]] = {
        stage: _build_stage_argv(
            stage,
            binary=binary_path,
            config_path=config_path,
            preset=preset,
            prompt_file=prompt_file,
            plan_path=plan_path,
        )
        for stage in _PIPELINE_STAGES
    }

    # ----- Stage 1: capability ----------------------------------------------
    capability_argv = argv_by_stage["capability"]
    report = probe_capability(binary_path, runner=run)
    capability_evidence: list[str] = [
        f"version={report.version!r}",
        f"flags_present={sorted(report.flags_present)}",
        f"flags_missing={sorted(report.flags_missing)}",
        f"json_supported={report.json_supported}",
        f"run_dry_run_supported={report.run_dry_run_supported}",
    ]
    if report.version == "missing":
        _record_block_then_skip(
            decisions,
            argv_by_stage,
            block_stage="capability",
            outcome="blocked_cli",
            reason=f"binary not found at {binary_path}",
            evidence=capability_evidence,
            skip_stages=("preset_check", "preflight", "dry_run"),
            skip_reason="capability gate blocked",
        )
        return tuple(decisions)

    if report.flags_missing or not report.run_dry_run_supported:
        missing = sorted(report.flags_missing) if report.flags_missing else ["run --dry-run"]
        _record_block_then_skip(
            decisions,
            argv_by_stage,
            block_stage="capability",
            outcome="blocked_cli",
            reason=f"required flags missing: {missing}",
            evidence=capability_evidence,
            skip_stages=("preset_check", "preflight", "dry_run"),
            skip_reason="capability gate blocked",
        )
        return tuple(decisions)

    decisions.append(
        _ok_decision(
            "capability",
            "preset_check",
            capability_evidence,
            capability_argv,
        )
    )

    # ----- Stage 2: preset check --strict -----------------------------------
    preset_argv = argv_by_stage["preset_check"]
    preset_result = _invoke(run, preset_argv)
    if preset_result == "timeout":
        _record_block_then_skip(
            decisions,
            argv_by_stage,
            block_stage="preset_check",
            outcome="blocked_unknown",
            reason=f"preset check --strict timed out after {DEFAULT_TIMEOUT}s",
            evidence=(f"argv={list(preset_argv)}",),
            skip_stages=("preflight", "dry_run"),
            skip_reason="preset_check blocked",
        )
        return tuple(decisions)

    if preset_result == "missing":
        _record_block_then_skip(
            decisions,
            argv_by_stage,
            block_stage="preset_check",
            outcome="blocked_cli",
            reason=f"binary not found at {binary_path}",
            evidence=(f"argv={list(preset_argv)}",),
            skip_stages=("preflight", "dry_run"),
            skip_reason="preset_check blocked",
        )
        return tuple(decisions)

    stdout, stderr, exit_code = preset_result
    if exit_code != 0:
        reason = _classify_preset_stderr(stderr)
        _record_block_then_skip(
            decisions,
            argv_by_stage,
            block_stage="preset_check",
            outcome="blocked_preset",
            reason=reason,
            evidence=(
                f"exit_code={exit_code}",
                f"stderr={stderr.strip()[:400]}",
            ),
            skip_stages=("preflight", "dry_run"),
            skip_reason="preset_check blocked",
        )
        return tuple(decisions)

    decisions.append(
        _ok_decision(
            "preset_check",
            "preflight",
            (f"exit_code={exit_code}",),
            preset_argv,
        )
    )

    # ----- Stage 3: preflight --strict --------------------------------------
    preflight_argv = argv_by_stage["preflight"]
    preflight_result = _invoke(run, preflight_argv)
    if preflight_result == "timeout":
        _record_block_then_skip(
            decisions,
            argv_by_stage,
            block_stage="preflight",
            outcome="blocked_unknown",
            reason=f"preflight --strict timed out after {DEFAULT_TIMEOUT}s",
            evidence=(f"argv={list(preflight_argv)}",),
            skip_stages=("dry_run",),
            skip_reason="preflight blocked",
        )
        return tuple(decisions)

    if preflight_result == "missing":
        _record_block_then_skip(
            decisions,
            argv_by_stage,
            block_stage="preflight",
            outcome="blocked_cli",
            reason=f"binary not found at {binary_path}",
            evidence=(f"argv={list(preflight_argv)}",),
            skip_stages=("dry_run",),
            skip_reason="preflight blocked",
        )
        return tuple(decisions)

    stdout, stderr, exit_code = preflight_result
    if exit_code != 0:
        outcome, reason = _classify_preflight_stderr(stderr)
        _record_block_then_skip(
            decisions,
            argv_by_stage,
            block_stage="preflight",
            outcome=outcome,
            reason=reason,
            evidence=(
                f"exit_code={exit_code}",
                f"stderr={stderr.strip()[:400]}",
            ),
            skip_stages=("dry_run",),
            skip_reason="preflight blocked",
        )
        return tuple(decisions)

    decisions.append(
        _ok_decision(
            "preflight",
            "dry_run",
            (f"exit_code={exit_code}",),
            preflight_argv,
        )
    )

    # ----- Stage 4: run --dry-run --strict ----------------------------------
    dry_run_argv = argv_by_stage["dry_run"]
    dry_result = _invoke(run, dry_run_argv)
    if dry_result == "timeout":
        _record_block_then_skip(
            decisions,
            argv_by_stage,
            block_stage="dry_run",
            outcome="blocked_unknown",
            reason=f"run --dry-run timed out after {DEFAULT_TIMEOUT}s",
            evidence=(f"argv={list(dry_run_argv)}",),
            skip_stages=(),
            skip_reason="",
        )
        return tuple(decisions)

    if dry_result == "missing":
        _record_block_then_skip(
            decisions,
            argv_by_stage,
            block_stage="dry_run",
            outcome="blocked_cli",
            reason=f"binary not found at {binary_path}",
            evidence=(f"argv={list(dry_run_argv)}",),
            skip_stages=(),
            skip_reason="",
        )
        return tuple(decisions)

    stdout, stderr, exit_code = dry_result
    if exit_code != 0:
        # Dry-run failed; classify as command-level unless stderr
        # screams about backend (which would imply preflight missed it,
        # but we still surface the backend class so callers can see it).
        outcome, reason = _classify_preflight_stderr(stderr)
        if outcome == "blocked_backend":
            _record_block_then_skip(
                decisions,
                argv_by_stage,
                block_stage="dry_run",
                outcome="blocked_backend",
                reason=reason,
                evidence=(
                    f"exit_code={exit_code}",
                    f"stderr={stderr.strip()[:400]}",
                ),
                skip_stages=(),
                skip_reason="",
            )
            return tuple(decisions)
        _record_block_then_skip(
            decisions,
            argv_by_stage,
            block_stage="dry_run",
            outcome="blocked_command",
            reason=reason,
            evidence=(
                f"exit_code={exit_code}",
                f"stderr={stderr.strip()[:400]}",
            ),
            skip_stages=(),
            skip_reason="",
        )
        return tuple(decisions)

    expected: dict[str, str] = {}
    if prompt_file:
        expected["prompt_file"] = prompt_file
    outcome, reason, evidence_values = _classify_dry_run(
        stdout,
        stderr,
        expected=expected,
    )
    evidence = [f"exit_code={exit_code}", f"reason={reason}"]
    for key, value in sorted(evidence_values.items()):
        evidence.append(f"effective_{key}={value!r}")
    decisions.append(
        StageDecision(
            stage="dry_run",
            outcome=outcome,
            evidence=tuple(evidence),
            next_allowed_stage=None,
            blocked_reason="" if outcome == "ok" else reason,
            argv=dry_run_argv,
        )
    )
    return tuple(decisions)


# ---------------------------------------------------------------------------
# Fixture loader
# ---------------------------------------------------------------------------


def load_fixture(name: str) -> list[FakeInvocation]:
    """Load a fixture directory into a list of ``FakeInvocation`` rows.

    The fixture root is ``skills/ralph-project-bootstrap/fixtures/cli``.
    Each subdirectory contains one or more ``*.json`` files; each file
    declares:

    * ``argv_expected`` — list of strings the fake runner must match.
    * ``stdout_chunks`` — strings to concatenate into ``stdout``.
    * ``stderr_chunks`` — strings to concatenate into ``stderr``.
    * ``exit_code`` — int the fake runner must return.
    * ``expected_stage`` — name of the stage this fixture targets.
    * ``expected_outcome`` — outcome string the staged gate must
      classify the invocation as.
    * ``expected_blocked_reason_regex`` — regex the
      ``blocked_reason`` must match.

    The loader orders the records by ``argv_expected`` length
    descending so more-specific argv tuples match before catch-all
    fallbacks.
    """
    fixture_root = Path(__file__).resolve().parents[1] / "fixtures" / "cli"
    fixture_dir = fixture_root / name
    if not fixture_dir.is_dir():
        raise FileNotFoundError(f"missing cli fixture: {fixture_dir}")

    invocations: list[FakeInvocation] = []
    for path in sorted(fixture_dir.glob("*.json")):
        with path.open("r", encoding="utf-8") as handle:
            data = json.load(handle)
        invocations.append(
            FakeInvocation(
                argv_expected=tuple(data["argv_expected"]),
                stdout_chunks=tuple(data.get("stdout_chunks", ())),
                stderr_chunks=tuple(data.get("stderr_chunks", ())),
                exit_code=int(data.get("exit_code", 0)),
            )
        )

    invocations.sort(key=lambda inv: -len(inv.argv_expected))
    return invocations
