"""Plan × Git-diff reconciliation for ``ralph-e2e-bootstrap``.

The skill runs this script *before* any persistent write or backend
call. The script compares the supplied development plan against the
current git working-tree diff and surfaces an :class:`AuditDecision`.

Public surface (everything else is private):

* :class:`AuditDecision` — typed result; ``ok=True`` ⇒ caller may
  proceed to binary resolution / sandbox generation.
* :class:`AuditIssue` — single reason the audit cannot proceed.
* :func:`run_audit` — main entry point. Pure stdlib.

Hard rules:

* No subprocess invocations at import time. ``run_audit`` accepts a
  ``diff_provider`` so tests can drive the audit with a fake.
* No file writes. The script is a pure function over its inputs.
* The plan file is *read-only* — its hash is captured for downstream
  evidence, never mutated.
* When the plan file is missing / unreadable, the decision is
  ``ok=False`` with ``blocked=True`` (U2 completion gate).
* ``clarify_codes`` is a tuple of stable short strings
  (``intent_undeclared``, ``scope_drift``, ``unit_missing``, …) that
  downstream combo-box wiring uses to pick the right
  ``plan_diff_clarify`` option.
"""

from __future__ import annotations

import hashlib
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

# Stable codes that ``clarify_codes`` may emit. Downstream UI maps
# each code to a combo-box option in ``references/interaction.md``.
CLARIFY_INTENT_UNDECLARED = "intent_undeclared"
CLARIFY_SCOPE_DRIFT = "scope_drift"
CLARIFY_UNIT_MISSING = "unit_missing"
CLARIFY_STALE_PLAN = "plan_stale"
CLARIFY_DIFF_UNAVAILABLE = "diff_unavailable"


@dataclass(frozen=True)
class AuditIssue:
    """A single reason the audit cannot proceed."""

    code: str
    message: str
    paths: tuple[str, ...] = ()


@dataclass(frozen=True)
class AuditDecision:
    """Outcome of the plan × diff reconciliation.

    * ``ok`` is True only when the plan is readable AND no
      ``clarify_codes`` were emitted.
    * ``blocked`` is True when the plan file is missing / unreadable
      (U2 completion gate "plan 文件不可读 → blocked").
    * ``plan_hash`` is the SHA-256 of the plan bytes as read; the
      handoff reuses this for tamper-evidence.
    * ``plan_repo_root`` / ``diff_repo_root`` are resolved git
      toplevels when available; ``cross_repo`` is True when both
      resolve and differ (legitimate dogfood pattern — no combo-box).
    """

    plan_path: str
    plan_hash: str
    ok: bool
    blocked: bool
    clarify_codes: tuple[str, ...] = ()
    issues: tuple[AuditIssue, ...] = ()
    plan_intent_paths: tuple[str, ...] = ()
    diff_paths: tuple[str, ...] = ()
    diff_unavailable: bool = False
    plan_repo_root: str | None = None
    diff_repo_root: str | None = None
    cross_repo: bool = False

    @property
    def is_blocking(self) -> bool:
        return self.blocked or not self.ok


# ---------------------------------------------------------------------------
# Plan parsing
# ---------------------------------------------------------------------------

# A U-ID heading looks like "### U1. ..." or "### U12. ...". We treat
# the leading "U<digits>" token as the stable Unit identifier and the
# remainder of the line as the title. The regex matches the literal
# "### U<n>." prefix; we keep the title intact for downstream
# evidence (no destructive normalisation).
_U_HEADING_RE = re.compile(r"^###\s+U(\d+)\.\s*(?P<title>.+?)\s*$", re.MULTILINE)

# A path token inside the plan body. We accept the common Markdown
# shapes: backticked paths (`` `crates/foo.rs` ``), bare relative
# paths ending in a recognised extension, and the literal ``./``.
_PATH_TOKEN_RE = re.compile(
    r"`(?P<back>[^`\n]+?\.[A-Za-z0-9]+)`"
    r"|(?P<bare>(?:[A-Za-z0-9_.-]+/)+[A-Za-z0-9_.-]+\.[A-Za-z0-9]+)"
)


def _extract_u_ids(plan_text: str) -> tuple[str, ...]:
    """Return the ordered list of U-IDs declared in the plan.

    The order is the order in which the headings appear. Duplicates
    are preserved so the caller can flag the plan as malformed; the
    comparison against ``diff_paths`` later does not require
    uniqueness.
    """
    return tuple(f"U{match.group(1)}" for match in _U_HEADING_RE.finditer(plan_text))


def _extract_intent_paths(plan_text: str) -> tuple[str, ...]:
    """Return path-shaped tokens that appear in the plan body.

    The plan author signals intent by naming files / directories they
    expect to touch. We accept backticked tokens first (the most
    deliberate shape) and bare tokens second (the most common
    accidental shape). Dedup preserves order.
    """
    seen: list[str] = []
    for match in _PATH_TOKEN_RE.finditer(plan_text):
        token = match.group("back") or match.group("bare")
        if token is None or token.startswith("http"):
            continue
        if token not in seen:
            seen.append(token)
    return tuple(seen)


def _hash_plan(plan_bytes: bytes) -> str:
    return hashlib.sha256(plan_bytes).hexdigest()


# ---------------------------------------------------------------------------
# Diff provider protocol
# ---------------------------------------------------------------------------


DiffProvider = Callable[[], tuple[str, ...]]


def _git_toplevel(path: Path) -> Path | None:
    """Return the git toplevel for ``path``, or ``None`` if unavailable.

    Uses ``git rev-parse --show-toplevel`` with the same timeout /
    failure posture as :func:`_git_diff_paths`. Never raises.
    """
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


def _git_diff_paths(repo_root: Path) -> tuple[tuple[str, ...], bool]:
    """Default ``diff_provider`` implementation.

    Returns ``(paths, diff_unavailable)`` where ``paths`` is the
    repo-relative paths from ``git diff HEAD`` and ``diff_unavailable``
    is True when git is missing, the timeout fired, the directory is
    not a repo, or the command otherwise failed. Distinguishes "diff
    unavailable" (environment fault) from "diff empty" (no changes).
    """
    import subprocess

    try:
        completed = subprocess.run(
            ["git", "-C", str(repo_root), "diff", "--name-only", "HEAD"],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired, OSError):
        return (), True
    if completed.returncode != 0:
        return (), True
    return tuple(line.strip() for line in completed.stdout.splitlines() if line.strip()), False


# ---------------------------------------------------------------------------
# Audit
# ---------------------------------------------------------------------------


def _classify(
    plan_text: str,
    u_ids: tuple[str, ...],
    intent_paths: tuple[str, ...],
    diff_paths: tuple[str, ...],
    diff_unavailable: bool = False,
    *,
    cross_repo: bool = False,
) -> tuple[tuple[str, ...], tuple[AuditIssue, ...]]:
    """Return ``(clarify_codes, issues)``.

    * ``clarify_codes`` is non-empty only when the plan *is* readable
      but the plan↔diff comparison fails.
    * ``issues`` is non-empty only when the plan is unreadable /
      missing (the caller surfaces these as a hard blocked handoff).
    * When ``cross_repo`` is True, ``scope_drift`` and ``plan_stale``
      are suppressed — the diff repo is not the plan's repo, so those
      comparisons are not meaningful. Plan-quality codes and
      ``diff_unavailable`` still apply.
    """
    issues: list[AuditIssue] = []
    clarify: list[str] = []

    if not u_ids:
        clarify.append(CLARIFY_UNIT_MISSING)
    if not intent_paths:
        clarify.append(CLARIFY_INTENT_UNDECLARED)

    # Scope drift: every diff path whose first two path segments do
    # not appear in any declared intent path's first two segments is a
    # flag. We compare on the first two path segments to keep
    # unrelated trees (e.g. both in ``crates/``) from triggering a
    # false positive while still catching cross-area drift (e.g.
    # ``crates/auth.rs`` against a plan that only mentions
    # ``crates/renderer.rs``).
    # Cross-repo: sandbox diff paths (e.g. PROMPT.*.md) will never
    # match orchestrator intent prefixes — skip this check.
    def _prefixes(path: str, depth: int = 2) -> tuple[str, ...]:
        parts = path.split("/")
        return tuple(parts[:depth])

    if intent_paths and not cross_repo:
        declared_prefixes = {_prefixes(path) for path in intent_paths}
        drift = sorted(
            path
            for path in diff_paths
            if _prefixes(path) not in declared_prefixes
        )
        if drift:
            clarify.append(CLARIFY_SCOPE_DRIFT)

    # Diff unavailable: git binary missing / timeout / not-a-repo —
    # distinct from "diff empty" (legitimately no changes in progress).
    if diff_unavailable:
        clarify.append(CLARIFY_DIFF_UNAVAILABLE)
    elif u_ids and not diff_paths and not cross_repo:
        # Stale plan: plan declares U-IDs but the diff is empty (no
        # work in progress) — usually a sign the plan is out of date.
        # Cross-repo: empty sandbox diff does not mean the plan is stale.
        clarify.append(CLARIFY_STALE_PLAN)

    return tuple(clarify), tuple(issues)


def run_audit(
    plan_path: str | Path,
    *,
    repo_root: str | Path | None = None,
    diff_provider: DiffProvider | None = None,
) -> AuditDecision:
    """Run the plan × diff audit and return a typed decision.

    * ``plan_path`` is the caller-supplied path; may be relative or
      absolute.
    * ``repo_root`` is the directory the diff provider should run
      against. When omitted, falls back to ``plan_path``'s parent.
    * ``diff_provider`` is a no-arg callable returning the ordered
      tuple of changed repo-relative paths. Defaults to
      :func:`_git_diff_paths` which shells out to ``git diff
      --name-only HEAD``.
    * When plan and ``repo_root`` resolve to different git toplevels,
      the decision sets ``cross_repo=True`` and skips ``scope_drift`` /
      ``plan_stale`` (legitimate dogfood; no combo-box).
    """
    plan_path = Path(plan_path)
    repo_root_path = Path(repo_root) if repo_root is not None else (
        plan_path.parent if plan_path.parent != Path("") else Path.cwd()
    )
    provider = diff_provider or (lambda: _git_diff_paths(repo_root_path))

    plan_toplevel = _git_toplevel(plan_path)
    diff_toplevel = _git_toplevel(repo_root_path)
    cross_repo = (
        plan_toplevel is not None
        and diff_toplevel is not None
        and plan_toplevel != diff_toplevel
    )
    plan_repo_root = str(plan_toplevel) if plan_toplevel is not None else None
    diff_repo_root = str(diff_toplevel) if diff_toplevel is not None else None

    # Read the plan. ``OSError`` and ``UnicodeDecodeError`` both
    # surface as a hard blocked decision (U2 completion gate).
    try:
        plan_bytes = plan_path.read_bytes()
        plan_text = plan_bytes.decode("utf-8")
    except (FileNotFoundError, IsADirectoryError, PermissionError, UnicodeDecodeError, OSError) as exc:
        return AuditDecision(
            plan_path=str(plan_path),
            plan_hash="",
            ok=False,
            blocked=True,
            issues=(
                AuditIssue(
                    code="plan_unreadable",
                    message=f"plan file not readable: {exc.__class__.__name__}",
                    paths=(str(plan_path),),
                ),
            ),
            plan_repo_root=plan_repo_root,
            diff_repo_root=diff_repo_root,
            cross_repo=cross_repo,
        )

    plan_hash = _hash_plan(plan_bytes)
    u_ids = _extract_u_ids(plan_text)
    intent_paths = _extract_intent_paths(plan_text)

    raw_provider_result = provider()
    # Detect new 2-tuple form: (diff_paths: tuple[str,...], diff_unavailable: bool)
    # Fall back to plain tuple of paths for back-compat with existing test providers.
    if (
        isinstance(raw_provider_result, tuple)
        and len(raw_provider_result) == 2
        and isinstance(raw_provider_result[1], bool)
    ):
        diff_paths, diff_unavailable = raw_provider_result
    else:
        diff_paths = raw_provider_result
        diff_unavailable = False

    clarify, issues = _classify(
        plan_text,
        u_ids,
        intent_paths,
        diff_paths,
        diff_unavailable,
        cross_repo=cross_repo,
    )

    return AuditDecision(
        plan_path=str(plan_path),
        plan_hash=plan_hash,
        ok=not issues and not clarify,
        blocked=bool(issues),
        clarify_codes=clarify,
        issues=issues,
        plan_intent_paths=intent_paths,
        diff_paths=diff_paths,
        diff_unavailable=diff_unavailable,
        plan_repo_root=plan_repo_root,
        diff_repo_root=diff_repo_root,
        cross_repo=cross_repo,
    )


__all__ = [
    "AuditDecision",
    "AuditIssue",
    "CLARIFY_DIFF_UNAVAILABLE",
    "CLARIFY_INTENT_UNDECLARED",
    "CLARIFY_SCOPE_DRIFT",
    "CLARIFY_STALE_PLAN",
    "CLARIFY_UNIT_MISSING",
    "run_audit",
]