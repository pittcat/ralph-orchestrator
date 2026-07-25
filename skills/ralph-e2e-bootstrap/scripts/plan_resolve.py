"""Plan resolution for ``ralph-e2e-bootstrap``.

Resolves which development plan the skill may bind into an E2E
sandbox **before** ``plan_diff`` / suite generation.

Public surface:

* :class:`ResolveResult` — typed outcome.
* :func:`assess_fitness` — whether a plan is a suitable E2E workload
  for the given sandbox.
* :func:`discover_sandbox_plans` — candidate plans under
  ``<sandbox>/docs/plans/``.
* :func:`author_minimal_plan` — write a new sandbox-local minimal
  E2E plan when none are suitable.
* :func:`resolve_plan` — main entry: optional caller candidate →
  fitness gate → discover → author.

Hard rules:

* Never rewrite a caller-supplied plan file (R13). Authored plans are
  **new** files under the sandbox only.
* Unfit caller candidates are **hard-rejected** (no combo-box override
  that re-binds an orchestrator crates/preset fix into a product
  sandbox).
* Pure stdlib. No writes except :func:`author_minimal_plan`.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from datetime import date
from pathlib import Path
from typing import Literal

# Reuse the same path-token extractor as plan_diff (duplicated lightly
# to keep this module importable without circular deps on classify).
_PATH_TOKEN_RE = re.compile(
    r"`(?P<back>[^`\n]+?\.[A-Za-z0-9]+)`"
    r"|(?P<bare>(?:[A-Za-z0-9_.-]+/)+[A-Za-z0-9_.-]+\.[A-Za-z0-9]+)"
)

_ORCH_PREFIXES = ("crates/", "presets/", "scripts/")
_SAFE_BASENAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,200}\.md$")

ResolveSource = Literal["caller", "discovered", "authored"]


@dataclass(frozen=True)
class FitnessReport:
    """Whether ``plan_path`` is a suitable E2E workload for ``sandbox``."""

    suitable: bool
    reason: str
    intent_paths: tuple[str, ...]
    orch_intent_count: int
    sandbox_local_hint_count: int


@dataclass(frozen=True)
class ResolveResult:
    """Outcome of plan resolution.

    * ``ok`` True ⇒ caller may pass ``plan_path`` to ``plan_diff`` /
      ``generate_suite``.
    * ``blocked`` True ⇒ hard stop (sandbox unusable / write failure).
    * ``rejected_candidate`` records a caller path that failed fitness.
    """

    ok: bool
    blocked: bool
    plan_path: str
    source: ResolveSource | str
    rejected_candidate: str | None = None
    reject_reason: str | None = None
    message: str = ""
    fitness: FitnessReport | None = None


def _extract_intent_paths(plan_text: str) -> tuple[str, ...]:
    seen: list[str] = []
    for match in _PATH_TOKEN_RE.finditer(plan_text):
        token = match.group("back") or match.group("bare")
        if token is None or token.startswith("http"):
            continue
        if token not in seen:
            seen.append(token)
    return tuple(seen)


def _git_toplevel(path: Path) -> Path | None:
    import subprocess

    probe = path if path.is_dir() else path.parent
    try:
        completed = subprocess.run(
            ["git", "-C", str(probe), "rev-parse", "--show-toplevel"],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired, OSError):
        return None
    if completed.returncode != 0:
        return None
    raw = completed.stdout.strip()
    if not raw:
        return None
    try:
        return Path(raw).resolve()
    except OSError:
        return None


def assess_fitness(plan_path: Path, sandbox: Path) -> FitnessReport:
    """Return whether ``plan_path`` is a suitable E2E workload for ``sandbox``.

    A plan is **unfit** when it primarily targets orchestrator trees
    (``crates/``, ``presets/``, ``scripts/``) that do not exist in the
    sandbox — the classic failure mode of binding an orchestrator fix
    plan into a product E2E harness.
    """
    sandbox = Path(sandbox).resolve()
    plan_path = Path(plan_path)
    try:
        text = plan_path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        return FitnessReport(
            suitable=False,
            reason=f"plan unreadable: {exc.__class__.__name__}",
            intent_paths=(),
            orch_intent_count=0,
            sandbox_local_hint_count=0,
        )

    intents = _extract_intent_paths(text)
    orch = sum(1 for p in intents if p.startswith(_ORCH_PREFIXES))
    sandbox_has_crates = (sandbox / "crates").is_dir()
    sandbox_has_presets = (sandbox / "presets").is_dir()
    local_hits = 0
    for p in intents:
        # Paths that resolve under the sandbox tree count as local.
        if not p.startswith(_ORCH_PREFIXES) and (sandbox / p).exists():
            local_hits += 1
        elif p.startswith("sorts/") or p.startswith("docs/"):
            local_hits += 1

    plan_top = _git_toplevel(plan_path)
    sand_top = _git_toplevel(sandbox)
    cross = (
        plan_top is not None
        and sand_top is not None
        and plan_top != sand_top
    )

    # Hard unfit: orchestrator-heavy intent and sandbox is not an
    # orchestrator checkout (no crates/ + no presets/).
    if orch >= 2 and not sandbox_has_crates and not sandbox_has_presets:
        return FitnessReport(
            suitable=False,
            reason=(
                "plan intent targets orchestrator trees (crates/presets/scripts) "
                "but sandbox has neither crates/ nor presets/; "
                "not an E2E workload for this sandbox"
            ),
            intent_paths=intents,
            orch_intent_count=orch,
            sandbox_local_hint_count=local_hits,
        )

    # Cross-repo + any orch intent against a product sandbox → unfit.
    if cross and orch >= 1 and not sandbox_has_crates:
        return FitnessReport(
            suitable=False,
            reason=(
                "plan lives in a different git repo than the sandbox and "
                "declares orchestrator intent paths; refuse binding"
            ),
            intent_paths=intents,
            orch_intent_count=orch,
            sandbox_local_hint_count=local_hits,
        )

    return FitnessReport(
        suitable=True,
        reason="plan intent is compatible with sandbox layout",
        intent_paths=intents,
        orch_intent_count=orch,
        sandbox_local_hint_count=local_hits,
    )


def discover_sandbox_plans(sandbox: Path) -> tuple[Path, ...]:
    """Return ``*.md`` plans under ``<sandbox>/docs/plans/`` (sorted)."""
    plans_dir = Path(sandbox).resolve() / "docs" / "plans"
    if not plans_dir.is_dir():
        return ()
    return tuple(sorted(p for p in plans_dir.glob("*.md") if p.is_file()))


def _score_candidate(path: Path, preset: str | None) -> int:
    name = path.name.lower()
    score = 0
    if "e2e" in name:
        score += 5
    if "multi-sort" in name or "multisort" in name:
        score += 4
    if "supervisor" in name and preset and "supervisor" in preset:
        score += 3
    if "minimal" in name or "e2e-bootstrap-minimal" in name:
        score += 1
    # Prefer newer dated prefixes roughly (YYYY-MM-DD).
    if re.match(r"^\d{4}-\d{2}-\d{2}-", path.name):
        score += 1
    return score


def pick_best_discovered(
    sandbox: Path,
    *,
    preset: str | None = None,
) -> Path | None:
    """Pick the highest-scoring **suitable** plan under the sandbox."""
    best: Path | None = None
    best_score = -10_000
    for candidate in discover_sandbox_plans(sandbox):
        report = assess_fitness(candidate, sandbox)
        if not report.suitable:
            continue
        score = _score_candidate(candidate, preset) + report.sandbox_local_hint_count
        if score > best_score:
            best_score = score
            best = candidate
    return best


def author_minimal_plan(
    sandbox: Path,
    *,
    preset: str | None = None,
    today: date | None = None,
) -> Path:
    """Write a new minimal E2E plan under ``<sandbox>/docs/plans/``.

    Does not modify any existing plan file. Returns the new path.
    """
    sandbox = Path(sandbox).resolve()
    plans_dir = sandbox / "docs" / "plans"
    plans_dir.mkdir(parents=True, exist_ok=True)
    day = today or date.today()
    stem = "sandbox"
    if preset:
        stem = preset.split(":")[-1].split("/")[-1].removesuffix(".yml")
    basename = f"{day.isoformat()}-e2e-bootstrap-minimal-{stem}-plan.md"
    if not _SAFE_BASENAME_RE.match(basename):
        raise ValueError(f"unsafe authored basename: {basename!r}")
    dest = plans_dir / basename
    if dest.exists():
        return dest

    body = (
        f"# Minimal E2E smoke plan (skill-authored)\n"
        f"\n"
        f"> generated_by: ralph-e2e-bootstrap\n"
        f"> preset: {preset or '(unspecified)'}\n"
        f"> purpose: sandbox-local workload when no suitable plan was found\n"
        f"\n"
        f"## Goal Capsule\n"
        f"\n"
        f"- Objective: produce a tiny, sandbox-local change the selected\n"
        f"  preset can execute end-to-end without touching orchestrator\n"
        f"  ``crates/`` or ``presets/``.\n"
        f"\n"
        f"## Implementation Units\n"
        f"\n"
        f"### U1. Write smoke marker\n"
        f"\n"
        f"Create `e2e_smoke_marker.txt` at the sandbox root with the\n"
        f"exact contents `ok\\n`. Do not modify other files.\n"
        f"\n"
        f"### U2. Verify marker\n"
        f"\n"
        f"Confirm `e2e_smoke_marker.txt` exists and reads `ok`.\n"
    )
    dest.write_text(body, encoding="utf-8")
    return dest


def resolve_plan(
    sandbox: Path,
    *,
    candidate: Path | str | None = None,
    preset: str | None = None,
    allow_author: bool = True,
) -> ResolveResult:
    """Resolve the plan path the rest of the skill may bind.

    Order:

    1. If ``candidate`` is given and :func:`assess_fitness` passes →
       use it (``source=caller``).
    2. If ``candidate`` is unfit → record rejection; fall through
       (hard reject, no override).
    3. Discover suitable plans under the sandbox; pick best.
    4. If none and ``allow_author`` → :func:`author_minimal_plan`.
    5. Otherwise blocked.
    """
    sandbox = Path(sandbox).resolve()
    if not sandbox.is_dir():
        return ResolveResult(
            ok=False,
            blocked=True,
            plan_path="",
            source="caller",
            message=f"sandbox is not a directory: {sandbox}",
        )

    rejected: str | None = None
    reject_reason: str | None = None
    fitness: FitnessReport | None = None

    if candidate is not None:
        cand = Path(candidate)
        fitness = assess_fitness(cand, sandbox)
        if fitness.suitable:
            return ResolveResult(
                ok=True,
                blocked=False,
                plan_path=str(cand.resolve() if cand.exists() else cand),
                source="caller",
                fitness=fitness,
                message="caller plan accepted",
            )
        rejected = str(cand)
        reject_reason = fitness.reason

    discovered = pick_best_discovered(sandbox, preset=preset)
    if discovered is not None:
        report = assess_fitness(discovered, sandbox)
        return ResolveResult(
            ok=True,
            blocked=False,
            plan_path=str(discovered.resolve()),
            source="discovered",
            rejected_candidate=rejected,
            reject_reason=reject_reason,
            fitness=report,
            message=(
                "using sandbox-local plan"
                + (f"; rejected unfit candidate {rejected}" if rejected else "")
            ),
        )

    if allow_author:
        try:
            authored = author_minimal_plan(sandbox, preset=preset)
        except (OSError, ValueError) as exc:
            return ResolveResult(
                ok=False,
                blocked=True,
                plan_path="",
                source="authored",
                rejected_candidate=rejected,
                reject_reason=reject_reason,
                message=f"failed to author minimal plan: {exc}",
            )
        report = assess_fitness(authored, sandbox)
        return ResolveResult(
            ok=True,
            blocked=False,
            plan_path=str(authored.resolve()),
            source="authored",
            rejected_candidate=rejected,
            reject_reason=reject_reason,
            fitness=report,
            message=(
                "authored minimal sandbox-local plan"
                + (f"; rejected unfit candidate {rejected}" if rejected else "")
            ),
        )

    return ResolveResult(
        ok=False,
        blocked=True,
        plan_path="",
        source="discovered",
        rejected_candidate=rejected,
        reject_reason=reject_reason,
        fitness=fitness,
        message="no suitable sandbox plan and authoring disabled",
    )


__all__ = [
    "FitnessReport",
    "ResolveResult",
    "assess_fitness",
    "author_minimal_plan",
    "discover_sandbox_plans",
    "pick_best_discovered",
    "resolve_plan",
]
