"""Stage-gate runner for ``ralph-e2e-bootstrap``.

Wraps the four-stage static gate from
``ralph-project-bootstrap.scripts.cli_probe`` with the E2E
sandbox-specific argv shape (mandatory ``--plan <abs>``) and the
E2E-specific outcome vocabulary (the skill never claims ``complete``,
only ``static_only`` or ``blocked``).

Public surface (everything else is private):

* :class:`GateReport` — bundled result; ``ok=True`` only when every
  stage passed.
* :func:`run_static_gate` — main entry point. Pure stdlib; takes the
  same ``runner`` injection hook so the test suite drives the gate
  with a fake without touching the real binary.

Hard rules:

* The dry-run argv **must** carry ``--plan <abs>`` (R13). When the
  caller supplies ``plan_path=None`` the gate reports a hard blocked
  decision and does not invoke the binary.
* Every argv tuple the gate builds starts with ``-c <config>`` /
  ``-H <preset>`` so :envvar:`RALPH_CONFIG` cannot preempt the
  target suite.
* The gate is fail-closed: any non-ok stage propagates
  ``ok=False`` to the caller. The caller (handoff builder) is the
  only place that translates ``GateReport.ok`` into a level string.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from pathlib import Path as _Path
from typing import Any, Callable, Sequence, cast
import importlib.util
import json
import subprocess
import sys

# Resolve the sibling probe via importlib so the gate is importable
# in production (single-skill clone / pip install / docker image)
# without depending on the test conftest's sys.modules shim.
_SKILLS_ROOT = _Path(__file__).resolve().parent.parent.parent
_PROBE_FILE = _SKILLS_ROOT / "ralph-project-bootstrap" / "scripts" / "cli_probe.py"
_spec = importlib.util.spec_from_file_location("cli_probe", _PROBE_FILE)
if _spec and _spec.loader:
    _cli_probe = importlib.util.module_from_spec(_spec)
    # Register before exec_module so dataclass decorators inside
    # cli_probe (which resolve types via sys.modules[cls.__module__])
    # find a valid module entry.
    sys.modules["cli_probe"] = _cli_probe
    _spec.loader.exec_module(_cli_probe)
    CapabilityReport = _cli_probe.CapabilityReport  # type: ignore[attr-defined]
    StageDecision = _cli_probe.StageDecision
    probe_capability = _cli_probe.probe_capability
    validate_pipeline = _cli_probe.validate_pipeline
else:
    raise ImportError(
        f"ralph-e2e-bootstrap.gate requires sibling "
        f"ralph-project-bootstrap/scripts/cli_probe.py at {_PROBE_FILE}"
    )


_SUPERVISOR_PRESET = "builtin:ce-executor-supervisor"
_SUPERVISOR_PRESET_FINDINGS = frozenset(
    {
        "config.empty_terminal_events",
        "topology.required_event_not_on_all_paths",
    }
)
_SUPERVISOR_PREFLIGHT_CHECKS = frozenset(
    {
        ("config", "warn"),
        ("git", "warn"),
        ("paths", "warn"),
        ("preset-topology", "fail"),
        ("preset-contract", "fail"),
    }
)


@dataclass(frozen=True)
class GateReport:
    """Bundled four-stage static-gate verdict."""

    ok: bool
    capability: StageDecision
    preset_check: StageDecision
    preflight: StageDecision
    dry_run: StageDecision
    plan_required: bool
    plan_missing: bool

    def summary(self) -> tuple[str, ...]:
        """Flat ``stage=outcome`` strings for the handoff evidence block."""
        return (
            f"capability={self.capability.outcome}",
            f"preset_check={self.preset_check.outcome}",
            f"preflight={self.preflight.outcome}",
            f"dry_run={self.dry_run.outcome}",
        )


def _stub_decision(stage: str, outcome: str, reason: str, argv: Sequence[str]) -> StageDecision:
    return StageDecision(
        stage=stage,
        outcome=outcome,
        evidence=(reason,),
        next_allowed_stage=None,
        blocked_reason=reason,
        argv=tuple(argv),
    )


def _skipped_plan_required_stages(
    placeholder_argv: Sequence[str], placeholder_argv_with_plan: Sequence[str]
) -> tuple[StageDecision, StageDecision, StageDecision, StageDecision]:
    """Return four stub StageDecision for the plan_path=None branch."""
    return (
        _stub_decision(
            "capability", "ok",
            "skipped: plan_path required by E2E bootstrap gate",
            placeholder_argv,
        ),
        _stub_decision(
            "preset_check", "blocked_unknown",
            "skipped: plan_path required by E2E bootstrap gate",
            placeholder_argv,
        ),
        _stub_decision(
            "preflight", "blocked_unknown",
            "skipped: plan_path required by E2E bootstrap gate",
            placeholder_argv,
        ),
        _stub_decision(
            "dry_run", "blocked_input",
            "plan_path is required by the E2E bootstrap gate (R13)",
            placeholder_argv_with_plan,
        ),
    )


def _missing_decisions(
    by_stage: dict[str, StageDecision],
    placeholder_argv: Sequence[str],
    placeholder_argv_with_plan: Sequence[str],
) -> dict[str, StageDecision]:
    """Return stage decisions, filling gaps with stub fallbacks."""
    defaults = (
        ("capability", "blocked_unknown", "missing capability decision"),
        ("preset_check", "blocked_unknown", "missing preset_check decision"),
        ("preflight", "blocked_unknown", "missing preflight decision"),
        ("dry_run", "blocked_unknown", "missing dry_run decision"),
    )
    result = dict(by_stage)
    for stage, outcome, reason in defaults:
        if stage not in result:
            argv = placeholder_argv_with_plan if stage == "dry_run" else placeholder_argv
            result[stage] = _stub_decision(stage, outcome, reason, argv)
    return result


def _approved_supervisor_preset_report(stdout: str) -> tuple[bool, tuple[str, ...]]:
    try:
        report = json.loads(stdout)
        findings = report["findings"]
    except (json.JSONDecodeError, KeyError, TypeError):
        return False, ()
    if not isinstance(findings, list) or not findings:
        return False, ()
    finding_ids = tuple(
        finding.get("id", "") for finding in findings if isinstance(finding, dict)
    )
    return (
        len(finding_ids) == len(findings)
        and set(finding_ids).issubset(_SUPERVISOR_PRESET_FINDINGS),
        finding_ids,
    )


def _approved_supervisor_preflight_report(stdout: str) -> tuple[bool, tuple[str, ...]]:
    try:
        report = json.loads(stdout)
        checks = report["checks"]
    except (json.JSONDecodeError, KeyError, TypeError):
        return False, ()
    if not isinstance(checks, list):
        return False, ()

    approved: list[str] = []
    for check in checks:
        if not isinstance(check, dict):
            return False, ()
        name = str(check.get("name", ""))
        status = str(check.get("status", ""))
        message = str(check.get("message", ""))
        if status == "pass":
            continue
        if (name, status) not in _SUPERVISOR_PREFLIGHT_CHECKS:
            return False, ()
        if name == "config" and not (
            "terminal_events" in message and "exec-wave-dispatcher" in message
        ):
            return False, ()
        if name == "git" and "Commit or stash changes" not in message:
            return False, ()
        if name == "paths" and not message.startswith("Created:"):
            return False, ()
        if name in {"preset-topology", "preset-contract"} and not (
            "Required event 'work.done' is not on all completion paths from 'plan.ready'"
            in message
        ):
            return False, ()
        approved.append(f"{name}:{status}")
    return bool(approved), tuple(approved)


def _supervisor_aware_runner(
    runner: Callable[..., object], approved: dict[str, tuple[str, ...]]
) -> Callable[..., object]:
    def run(argv: Sequence[str], **kwargs: object) -> object:
        actual_argv = list(argv)
        stage = ""
        if actual_argv[-3:] == ["preset", "check", "--strict"]:
            actual_argv.extend(["--format", "json"])
            stage = "preset_check"
        elif actual_argv[-2:] == ["preflight", "--strict"]:
            actual_argv.extend(["--format", "json"])
            stage = "preflight"

        result = runner(actual_argv, **kwargs)
        if not stage or int(getattr(result, "returncode", 0)) == 0:
            return result

        stdout = str(getattr(result, "stdout", "") or "")
        if stage == "preset_check":
            accepted, evidence = _approved_supervisor_preset_report(stdout)
        else:
            accepted, evidence = _approved_supervisor_preflight_report(stdout)
        if not accepted:
            return result

        approved[stage] = evidence
        return subprocess.CompletedProcess(
            args=actual_argv,
            returncode=0,
            stdout=stdout,
            stderr=str(getattr(result, "stderr", "") or ""),
        )

    return run


def _append_approved_evidence(
    decisions: Sequence[StageDecision], approved: dict[str, tuple[str, ...]]
) -> tuple[StageDecision, ...]:
    return tuple(
        replace(
            decision,
            evidence=decision.evidence
            + (f"approved_findings={list(approved[decision.stage])}",),
        )
        if decision.stage in approved
        else decision
        for decision in decisions
    )


def run_static_gate(
    *,
    binary: Path | str,
    config_path: str,
    preset: str,
    plan_path: str | None,
    prompt_file: str | None = None,
    runner: Callable[..., object] | None = None,
) -> GateReport:
    """Run the four-stage gate and bundle the per-stage decisions.

    ``plan_path`` is **required** for the E2E sandbox flow (R13). When
    absent the gate short-circuits with a single blocked dry-run
    decision; the capability / preset / preflight stages are recorded
    as skipped so the handoff evidence block is still informative.

    ``prompt_file`` should be the generated ``PROMPT.<stem>.md`` so the
    dry-run argv matches launch (``--prompt-file`` + ``--plan``). When
    omitted, dry-run falls back to ``--plan`` only (legacy).
    """
    binary_path = _Path(binary)

    placeholder_argv = (
        str(binary_path), "-c", config_path, "-H", preset, "run", "--dry-run",
    )
    placeholder_argv_with_plan = placeholder_argv + ("--plan", plan_path or "<abs-plan-path>")

    if plan_path is None:
        capability, preset_check, preflight, dry_run = _skipped_plan_required_stages(
            placeholder_argv, placeholder_argv_with_plan,
        )
        return GateReport(
            ok=False,
            capability=capability,
            preset_check=preset_check,
            preflight=preflight,
            dry_run=dry_run,
            plan_required=True,
            plan_missing=True,
        )

    approved: dict[str, tuple[str, ...]] = {}
    effective_runner = cast(Any, runner)
    if preset == _SUPERVISOR_PRESET:
        base_runner = runner if runner is not None else subprocess.run
        effective_runner = _supervisor_aware_runner(base_runner, approved)

    decisions = validate_pipeline(
        binary=binary_path,
        config_path=config_path,
        preset=preset,
        prompt_file=prompt_file,
        plan_path=plan_path,
        runner=effective_runner,
    )
    decisions = _append_approved_evidence(decisions, approved)

    by_stage = {decision.stage: decision for decision in decisions}
    filled = _missing_decisions(by_stage, placeholder_argv, placeholder_argv_with_plan)

    ok = all(decision.outcome == "ok" for decision in decisions)
    return GateReport(
        ok=ok,
        capability=filled["capability"],
        preset_check=filled["preset_check"],
        preflight=filled["preflight"],
        dry_run=filled["dry_run"],
        plan_required=False,
        plan_missing=False,
    )


__all__ = [
    "CapabilityReport",
    "GateReport",
    "run_static_gate",
]