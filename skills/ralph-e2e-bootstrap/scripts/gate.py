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

from dataclasses import dataclass
from pathlib import Path as _Path
from typing import Any, Callable, Sequence, cast
import importlib.util
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


def run_static_gate(
    *,
    binary: Path | str,
    config_path: str,
    preset: str,
    plan_path: str | None,
    runner: Callable[..., object] | None = None,
) -> GateReport:
    """Run the four-stage gate and bundle the per-stage decisions.

    ``plan_path`` is **required** for the E2E sandbox flow (R13). When
    absent the gate short-circuits with a single blocked dry-run
    decision; the capability / preset / preflight stages are recorded
    as skipped so the handoff evidence block is still informative.
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

    decisions = validate_pipeline(
        binary=binary_path,
        config_path=config_path,
        preset=preset,
        prompt_file=None,
        plan_path=plan_path,
        runner=cast(Any, runner),
    )

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