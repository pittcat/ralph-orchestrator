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
from pathlib import Path
from typing import Callable, Sequence

# Reuse the project-bootstrap probe. The two skills share the same
# capability surface; only the argv shape and outcome vocabulary
# differ.
from cli_probe import (  # type: ignore[import-not-found]
    CapabilityReport,
    StageDecision,
    probe_capability,
    validate_pipeline,
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
    binary_path = Path(binary)
    plan_required = plan_path is None

    placeholder_argv = (
        str(binary_path),
        "-c",
        config_path,
        "-H",
        preset,
        "run",
        "--dry-run",
    )
    placeholder_argv_with_plan = placeholder_argv + ("--plan", plan_path or "<abs-plan-path>")

    if plan_required:
        capability = _stub_decision(
            "capability",
            "ok",
            "skipped: plan_path required by E2E bootstrap gate",
            placeholder_argv,
        )
        preset_check = _stub_decision(
            "preset_check",
            "blocked_unknown",
            "skipped: plan_path required by E2E bootstrap gate",
            placeholder_argv,
        )
        preflight = _stub_decision(
            "preflight",
            "blocked_unknown",
            "skipped: plan_path required by E2E bootstrap gate",
            placeholder_argv,
        )
        dry_run = _stub_decision(
            "dry_run",
            "blocked_input",
            "plan_path is required by the E2E bootstrap gate (R13)",
            placeholder_argv_with_plan,
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
        runner=runner,  # type: ignore[arg-type]
    )

    by_stage = {decision.stage: decision for decision in decisions}
    capability = by_stage.get("capability") or _stub_decision(
        "capability",
        "blocked_unknown",
        "missing capability decision",
        placeholder_argv,
    )
    preset_check = by_stage.get("preset_check") or _stub_decision(
        "preset_check",
        "blocked_unknown",
        "missing preset_check decision",
        placeholder_argv,
    )
    preflight = by_stage.get("preflight") or _stub_decision(
        "preflight",
        "blocked_unknown",
        "missing preflight decision",
        placeholder_argv,
    )
    dry_run = by_stage.get("dry_run") or _stub_decision(
        "dry_run",
        "blocked_unknown",
        "missing dry_run decision",
        placeholder_argv_with_plan,
    )

    ok = all(decision.outcome == "ok" for decision in decisions)
    return GateReport(
        ok=ok,
        capability=capability,
        preset_check=preset_check,
        preflight=preflight,
        dry_run=dry_run,
        plan_required=False,
        plan_missing=False,
    )


__all__ = [
    "CapabilityReport",
    "GateReport",
    "run_static_gate",
]