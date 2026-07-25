"""Forced orchestration entry for ``ralph-e2e-bootstrap``.

Agents MUST call :func:`run_pipeline` (or the CLI below) instead of
stitching ``resolve_plans`` / ``generate_suite`` / ``gate`` by hand.
The pipeline refuses to emit a launch command unless:

* ``change_plan`` is present and readable
* sandbox is an external git repo vs the change plan
* workload is resolved (or operator confirmed author)
* preset-gap is acknowledged when the change plan touches ``presets/``
* binary passes :func:`check_binary_freshness` against the change-plan repo
* suite is generated with change intent embedded in ``PROMPT``
* static gate passes with ``--prompt-file`` + ``--plan``

Public surface:

* :class:`PipelineResult`
* :func:`run_pipeline`
* ``python -m`` / ``python bootstrap_pipeline.py`` CLI
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any, Callable, Literal

import binary_resolve
import e2e_handoff
import gate
import plan_diff
import plan_resolve
import sandbox_suite

NeedKind = Literal[
    "",
    "sandbox_plan_write",
    "preset_gap",
    "binary_resolution",
    "plan_diff_clarify",
    "write_conflict",
]


@dataclass(frozen=True)
class PipelineResult:
    """Outcome of :func:`run_pipeline`.

    * ``ok`` True ⇒ static gates passed; ``launch_argv`` is copy-paste ready.
    * ``needs`` non-empty ⇒ skill MUST surface that combo-box and resume.
    * ``blocked`` True ⇒ hard stop (bad inputs / gate failure).
    """

    ok: bool
    blocked: bool
    needs: NeedKind | str
    message: str
    binary: str = ""
    workload_plan_path: str = ""
    change_plan_path: str = ""
    change_plan_hash: str = ""
    config_path: str = ""
    prompt_path: str = ""
    launch_argv: tuple[str, ...] = ()
    handoff_report: str = ""
    stage_outcomes: tuple[str, ...] = ()

    def to_json(self) -> str:
        payload = asdict(self)
        payload["launch_argv"] = list(self.launch_argv)
        payload["stage_outcomes"] = list(self.stage_outcomes)
        return json.dumps(payload, indent=2, ensure_ascii=False)


def _fail(
    *,
    message: str,
    blocked: bool = True,
    needs: str = "",
    **extra: Any,
) -> PipelineResult:
    return PipelineResult(
        ok=False,
        blocked=blocked,
        needs=needs,
        message=message,
        **extra,
    )


def run_pipeline(
    *,
    sandbox: Path | str,
    change_plan: Path | str,
    preset: str,
    binary_explicit: str | None = None,
    build_repo: Path | str | None = None,
    refresh_existing: bool = False,
    preset_continue_confirmed: bool = False,
    author_confirmed: bool = False,
    skip_plan_diff: bool = False,
    runner: Callable[..., object] | None = None,
    sandbox_label: str | None = None,
    trusted_only: bool = True,
) -> PipelineResult:
    """Run the full static bootstrap pipeline with hard gates.

    Parameters
    ----------
    sandbox / change_plan / preset:
        Required operator inputs. ``change_plan`` is never used as
        ``--plan``.
    binary_explicit:
        Optional ``--ralph-binary`` override; still must pass freshness.
    build_repo:
        Repo whose ``target/*/ralph`` must be fresh. Defaults to the
        change-plan git toplevel.
    preset_continue_confirmed:
        Set True only after ``preset_gap`` combo-box option 1.
    author_confirmed:
        Set True only after ``sandbox_plan_write`` combo-box confirm;
        then the pipeline may call :func:`author_minimal_plan`.
    """
    sandbox_p = Path(sandbox).resolve()
    change_p = Path(change_plan).resolve()
    if not preset or not str(preset).strip():
        return _fail(message="preset is required")

    resolved = plan_resolve.resolve_plans(
        sandbox_p, change_plan=change_p, preset=preset
    )
    base_extra = {
        "change_plan_path": str(change_p),
        "change_plan_hash": resolved.change_plan_hash,
    }

    if resolved.blocked:
        return _fail(message=resolved.message, **base_extra)

    if resolved.change_plan_touches_presets and not preset_continue_confirmed:
        return _fail(
            message=resolved.message or "change plan touches presets/",
            blocked=False,
            needs="preset_gap",
            **base_extra,
        )

    if resolved.needs_author_confirmation:
        if not author_confirmed:
            return _fail(
                message=resolved.message,
                blocked=False,
                needs="sandbox_plan_write",
                **base_extra,
            )
        try:
            plan_resolve.author_minimal_plan(
                sandbox_p,
                preset=preset,
                confirmed=True,
                confirmation_token="sandbox_plan_write",
            )
        except ValueError as exc:
            return _fail(message=str(exc), **base_extra)
        resolved = plan_resolve.resolve_plans(
            sandbox_p, change_plan=change_p, preset=preset
        )
        if not resolved.ok:
            return _fail(
                message=resolved.message or "workload still missing after author",
                blocked=False,
                needs="sandbox_plan_write",
                change_plan_path=str(change_p),
                change_plan_hash=resolved.change_plan_hash,
            )

    if not resolved.ok or not resolved.workload_plan_path:
        return _fail(
            message=resolved.message or "workload unresolved",
            blocked=resolved.blocked,
            needs="sandbox_plan_write" if resolved.needs_author_confirmation else "",
            **base_extra,
        )

    workload = Path(resolved.workload_plan_path)
    base_extra["workload_plan_path"] = str(workload)
    base_extra["change_plan_hash"] = resolved.change_plan_hash

    if not skip_plan_diff:
        audit = plan_diff.run_audit(workload, repo_root=sandbox_p)
        if audit.blocked:
            detail = "; ".join(i.message for i in audit.issues) or "unreadable plan"
            return _fail(
                message=f"plan_diff blocked: {detail}",
                **base_extra,
            )
        if not audit.ok and audit.clarify_codes:
            return _fail(
                message=(
                    "plan_diff needs clarify: "
                    + ",".join(audit.clarify_codes)
                ),
                blocked=False,
                needs="plan_diff_clarify",
                **base_extra,
            )

    bin_res = binary_resolve.resolve_binary(
        explicit_path=binary_explicit,
        trusted_only=trusted_only,
    )
    if not bin_res.ok:
        return _fail(
            message=bin_res.detail or "binary unresolved",
            blocked=bin_res.reason == "blocked",
            needs="binary_resolution",
            binary=bin_res.binary,
            **base_extra,
        )

    repo = Path(build_repo).resolve() if build_repo else None
    if repo is None:
        repo = plan_resolve.git_toplevel(change_p) or change_p.parent
    freshness = binary_resolve.check_binary_freshness(bin_res.binary, repo)
    if not freshness.fresh:
        return _fail(
            message=freshness.detail,
            blocked=False,
            needs="binary_resolution",
            binary=bin_res.binary,
            **base_extra,
        )

    try:
        suite = sandbox_suite.generate_suite(
            sandbox=sandbox_p,
            preset=preset,
            plan_path=workload,
            binary=bin_res.binary,
            refresh_existing=refresh_existing,
            change_plan_path=resolved.change_plan_path,
            change_plan_hash=resolved.change_plan_hash,
            change_summary=resolved.change_summary,
        )
    except sandbox_suite.SandboxError as exc:
        msg = str(exc)
        needs = "write_conflict" if "write_conflict" in msg else ""
        return _fail(
            message=msg,
            blocked=not needs,
            needs=needs,
            binary=bin_res.binary,
            **base_extra,
        )

    config_rel = Path(suite.config_path).name
    prompt_rel = Path(suite.prompt_path).name
    plan_rel = "docs/plans/" + workload.name

    gate_report = gate.run_static_gate(
        binary=bin_res.binary,
        config_path=config_rel,
        preset=preset,
        plan_path=plan_rel,
        prompt_file=prompt_rel,
        runner=runner,
    )
    if not gate_report.ok:
        return _fail(
            message="static gate failed: " + "; ".join(gate_report.summary()),
            binary=bin_res.binary,
            workload_plan_path=str(workload),
            change_plan_path=str(change_p),
            change_plan_hash=resolved.change_plan_hash,
            config_path=config_rel,
            prompt_path=prompt_rel,
            stage_outcomes=gate_report.summary(),
        )

    label = sandbox_label or sandbox_p.name
    artifact = e2e_handoff.build_handoff(
        e2e_handoff.HandoffInputs(
            binary=bin_res.binary,
            config_path=config_rel,
            preset=preset,
            plan_path=plan_rel,
            prompt_path=prompt_rel,
            level="static_only",
            sandbox_path=label,
            validation_evidence=gate_report.summary(),
            residual_risks=(
                "static_only: loop not closed",
                "post-run diagnosis: ralph-run-diagnosis",
            ),
            stage_outcomes=gate_report.summary(),
        )
    )

    return PipelineResult(
        ok=True,
        blocked=False,
        needs="",
        message="static bootstrap ready; loop NOT closed",
        binary=bin_res.binary,
        workload_plan_path=str(workload),
        change_plan_path=str(change_p),
        change_plan_hash=resolved.change_plan_hash,
        config_path=config_rel,
        prompt_path=prompt_rel,
        launch_argv=artifact.command_argv,
        handoff_report=artifact.report,
        stage_outcomes=gate_report.summary(),
    )


def _main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Forced ralph-e2e-bootstrap pipeline entry"
    )
    parser.add_argument("--sandbox", required=True)
    parser.add_argument("--change-plan", required=True)
    parser.add_argument("--preset", required=True)
    parser.add_argument("--ralph-binary", default=None)
    parser.add_argument("--build-repo", default=None)
    parser.add_argument("--refresh-existing", action="store_true")
    parser.add_argument(
        "--preset-continue-confirmed",
        action="store_true",
        help="Set after preset_gap combo-box option 1",
    )
    parser.add_argument(
        "--author-confirmed",
        action="store_true",
        help="Set after sandbox_plan_write combo-box confirm",
    )
    parser.add_argument("--skip-plan-diff", action="store_true")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)

    result = run_pipeline(
        sandbox=args.sandbox,
        change_plan=args.change_plan,
        preset=args.preset,
        binary_explicit=args.ralph_binary,
        build_repo=args.build_repo,
        refresh_existing=args.refresh_existing,
        preset_continue_confirmed=args.preset_continue_confirmed,
        author_confirmed=args.author_confirmed,
        skip_plan_diff=args.skip_plan_diff,
    )
    if args.json:
        print(result.to_json())
    else:
        print(result.message)
        if result.needs:
            print(f"needs={result.needs}", file=sys.stderr)
        if result.ok and result.launch_argv:
            print("launch:", " ".join(result.launch_argv))
        if result.handoff_report:
            print(result.handoff_report)
    return 0 if result.ok else 2


if __name__ == "__main__":
    raise SystemExit(_main())


__all__ = [
    "NeedKind",
    "PipelineResult",
    "run_pipeline",
]
