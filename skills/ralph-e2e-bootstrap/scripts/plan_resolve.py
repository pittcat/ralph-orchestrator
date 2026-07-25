"""Plan resolution for ``ralph-e2e-bootstrap`` (R15 / dual-plan model).

Two plan roles:

* **Change plan** (from the orchestrator / current repo) — verification
  intent: what was modified and what the E2E run should prove. Never
  used as ``ralph run --plan``.
* **Workload plan** (sandbox-local) — the business scenario agents
  execute. Always discovered under ``<sandbox>/docs/plans/``. Creating
  or editing sandbox plans requires an explicit operator combo-box
  (this module never silently authors).

Public surface:

* :func:`assess_workload_fitness` — sandbox-local E2E suitability.
* :func:`discover_sandbox_plans` / :func:`pick_best_discovered`
* :func:`change_plan_needs_preset_author` — hard-handoff signal.
* :func:`extract_change_summary` — short text for PROMPT injection.
* :func:`author_minimal_plan` — write only after operator confirms.
* :func:`resolve_plans` — main entry.

Hard rules:

* Never rewrite a caller-supplied change plan (R13).
* Never bind an orchestrator change plan as the workload ``--plan``.
* Never silently create/edit files under ``<sandbox>/docs/plans/``.
* Pure stdlib.
"""

from __future__ import annotations

import hashlib
import re
from dataclasses import dataclass
from datetime import date
from pathlib import Path
from typing import Literal

_PATH_TOKEN_RE = re.compile(
    r"`(?P<back>[^`\n]+?\.[A-Za-z0-9]+)`"
    r"|(?P<bare>(?:[A-Za-z0-9_.-]+/)+[A-Za-z0-9_.-]+\.[A-Za-z0-9]+)"
)

_ORCH_PREFIXES = ("crates/", "presets/", "scripts/")
_SAFE_BASENAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,200}\.md$")

WorkloadSource = Literal["discovered", "none"]


@dataclass(frozen=True)
class FitnessReport:
    suitable: bool
    reason: str
    intent_paths: tuple[str, ...]
    orch_intent_count: int
    sandbox_local_hint_count: int


@dataclass(frozen=True)
class ResolveResult:
    """Dual-plan resolution outcome.

    * ``ok`` True ⇒ a workload plan is ready for audit / suite.
    * ``needs_author_confirmation`` True ⇒ no suitable workload;
      skill MUST combo-box before calling :func:`author_minimal_plan`.
    * ``change_plan_touches_presets`` True ⇒ change plan mentions
      ``presets/``; skill MUST ask whether preset work is done or
      hard-handoff ``ralph-preset-author`` (do not silently skip).
    * ``workload_plan_path`` is the only path allowed for ``--plan``.
    * ``change_plan_path`` is verification context for PROMPT injection.
    """

    ok: bool
    blocked: bool
    workload_plan_path: str
    workload_source: WorkloadSource | str
    change_plan_path: str | None = None
    change_plan_hash: str = ""
    change_summary: str = ""
    change_plan_touches_presets: bool = False
    needs_author_confirmation: bool = False
    message: str = ""
    fitness: FitnessReport | None = None

    # Back-compat alias used by older call sites / tests.
    @property
    def plan_path(self) -> str:
        return self.workload_plan_path

    @property
    def needs_preset_author(self) -> bool:
        """Deprecated alias — prefer ``change_plan_touches_presets`` + combo-box."""
        return self.change_plan_touches_presets


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


def _hash_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def assess_workload_fitness(plan_path: Path, sandbox: Path) -> FitnessReport:
    """Return whether ``plan_path`` is a suitable **workload** for ``sandbox``."""
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

    if orch >= 2 and not sandbox_has_crates and not sandbox_has_presets:
        return FitnessReport(
            suitable=False,
            reason=(
                "plan intent targets orchestrator trees; not a sandbox "
                "workload (use as change plan / verification intent only)"
            ),
            intent_paths=intents,
            orch_intent_count=orch,
            sandbox_local_hint_count=local_hits,
        )

    if cross and orch >= 1 and not sandbox_has_crates:
        return FitnessReport(
            suitable=False,
            reason=(
                "plan is outside the sandbox git repo and declares "
                "orchestrator intent; refuse as workload --plan"
            ),
            intent_paths=intents,
            orch_intent_count=orch,
            sandbox_local_hint_count=local_hits,
        )

    return FitnessReport(
        suitable=True,
        reason="plan intent is compatible with sandbox workload",
        intent_paths=intents,
        orch_intent_count=orch,
        sandbox_local_hint_count=local_hits,
    )


# Back-compat name for older imports/tests.
assess_fitness = assess_workload_fitness


def change_plan_needs_preset_author(change_plan: Path) -> bool:
    """True when the change plan declares ``presets/`` intent paths."""
    try:
        text = Path(change_plan).read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return False
    return any(p.startswith("presets/") for p in _extract_intent_paths(text))


def extract_change_summary(change_plan: Path, *, max_chars: int = 1200) -> str:
    """Extract a short verification-context blurb for PROMPT injection."""
    try:
        text = Path(change_plan).read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        return f"(unreadable change plan: {exc.__class__.__name__})"

    lines = text.splitlines()
    # Prefer Goal Capsule / Objective bullets when present.
    blob: list[str] = []
    in_capsule = False
    for line in lines:
        if line.strip().startswith("## Goal Capsule"):
            in_capsule = True
            blob.append(line.strip())
            continue
        if in_capsule:
            if line.startswith("## ") and not line.startswith("## Goal"):
                break
            if line.strip():
                blob.append(line.rstrip())
            if sum(len(x) + 1 for x in blob) >= max_chars:
                break
    if not blob:
        for line in lines[:40]:
            if line.strip():
                blob.append(line.rstrip())
            if sum(len(x) + 1 for x in blob) >= max_chars:
                break
    summary = "\n".join(blob).strip()
    if len(summary) > max_chars:
        summary = summary[: max_chars - 3] + "..."
    return summary or "(empty change plan)"


def discover_sandbox_plans(sandbox: Path) -> tuple[Path, ...]:
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
    if re.match(r"^\d{4}-\d{2}-\d{2}-", path.name):
        score += 1
    return score


def pick_best_discovered(
    sandbox: Path,
    *,
    preset: str | None = None,
) -> Path | None:
    best: Path | None = None
    best_score = -10_000
    for candidate in discover_sandbox_plans(sandbox):
        report = assess_workload_fitness(candidate, sandbox)
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
    """Write a new minimal workload plan under the sandbox.

    Call **only** after the operator confirms via combo-box.
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
        f"> purpose: sandbox-local workload (operator-confirmed)\n"
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


def resolve_plans(
    sandbox: Path,
    *,
    change_plan: Path | str | None = None,
    preset: str | None = None,
) -> ResolveResult:
    """Resolve change-plan context + sandbox workload for the skill.

    Never treats ``change_plan`` as the workload ``--plan``. Never
    authors a plan; when discovery fails sets
    ``needs_author_confirmation=True`` for the skill combo-box.
    """
    sandbox = Path(sandbox).resolve()
    if not sandbox.is_dir():
        return ResolveResult(
            ok=False,
            blocked=True,
            workload_plan_path="",
            workload_source="none",
            message=f"sandbox is not a directory: {sandbox}",
        )

    change_path: Path | None = Path(change_plan) if change_plan else None
    change_hash = ""
    change_summary = ""
    touches_presets = False
    if change_path is not None:
        try:
            raw = change_path.read_bytes()
            change_hash = _hash_bytes(raw)
            change_summary = extract_change_summary(change_path)
            touches_presets = change_plan_needs_preset_author(change_path)
        except (OSError, UnicodeDecodeError) as exc:
            return ResolveResult(
                ok=False,
                blocked=True,
                workload_plan_path="",
                workload_source="none",
                change_plan_path=str(change_path),
                message=f"change plan unreadable: {exc.__class__.__name__}",
            )

    discovered = pick_best_discovered(sandbox, preset=preset)
    if discovered is not None:
        report = assess_workload_fitness(discovered, sandbox)
        msg = "workload discovered under sandbox docs/plans/"
        if touches_presets:
            msg += (
                "; change plan touches presets/ — ask operator: preset "
                "already updated, or hard-handoff ralph-preset-author"
            )
        return ResolveResult(
            ok=True,
            blocked=False,
            workload_plan_path=str(discovered.resolve()),
            workload_source="discovered",
            change_plan_path=str(change_path.resolve()) if change_path else None,
            change_plan_hash=change_hash,
            change_summary=change_summary,
            change_plan_touches_presets=touches_presets,
            fitness=report,
            message=msg,
        )

    return ResolveResult(
        ok=False,
        blocked=False,
        workload_plan_path="",
        workload_source="none",
        change_plan_path=str(change_path.resolve()) if change_path else None,
        change_plan_hash=change_hash,
        change_summary=change_summary,
        change_plan_touches_presets=touches_presets,
        needs_author_confirmation=True,
        message=(
            "no suitable sandbox workload plan; ask operator before "
            "authoring a minimal plan (do not write silently)"
        ),
    )


def resolve_plan(
    sandbox: Path,
    *,
    candidate: Path | str | None = None,
    preset: str | None = None,
    allow_author: bool = True,
) -> ResolveResult:
    """Backward-compatible wrapper.

    * ``candidate`` is treated as the **change plan** (verification
      intent), never as the workload.
    * ``allow_author`` is ignored for silent writes; discovery failure
      always sets ``needs_author_confirmation`` instead of auto-authoring.
    """
    del allow_author  # silent author removed by product contract
    return resolve_plans(sandbox, change_plan=candidate, preset=preset)


__all__ = [
    "FitnessReport",
    "ResolveResult",
    "assess_fitness",
    "assess_workload_fitness",
    "author_minimal_plan",
    "change_plan_needs_preset_author",
    "discover_sandbox_plans",
    "extract_change_summary",
    "pick_best_discovered",
    "resolve_plan",
    "resolve_plans",
]
