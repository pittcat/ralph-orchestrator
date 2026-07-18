"""AGENTS.md / CLAUDE.md managed-section helpers for ``ralph-project-bootstrap``.

The helpers in this module own every persistent edit to the two
operator-facing docs that the bootstrap pipeline touches:

* ``AGENTS.md``
* ``CLAUDE.md``

Both files are mutated only via the ``RALPH-BOOTSTRAP-START`` /
``RALPH-BOOTSTRAP-END`` markers defined here. The marker prefix is
deliberately distinct from ``RALPH-MANAGED-BLOCK-START`` / ``END`` used
by the runtime loop ledger so the two namespaces never collide.

Design rules (enforced by the helpers themselves):

* **0 / 1 / many markers are not silent.** ``parse_managed_section``
  classifies the document and stops composes that would otherwise hide a
  real conflict (duplicate or truncated block, END before START).
* **Byte-equal re-runs are noops.** ``compose_agent_docs`` never rewrites
  a file when the only change would be a noop update.
* **User prose is preserved.** Anything outside the managed block is
  passed through byte-for-byte; the helper never rewrites unknown
  markers, comments, or trailing prose.
* **Writes are atomic per batch.** ``AtomicWriter`` queues every change
  into a sibling ``.bootstrap.tmp`` (uniquely named per writer
  invocation via pid + monotonic-ns) first and rolls back
  already-staged siblings on the first failure so a partial write can
  never leave a half-updated pair of docs.
* **No shell, no chmod, no ``.ralph/`` in the target project.** All
  operations are pure stdlib file I/O scoped to the project directory.
"""
from __future__ import annotations

import os
import re
import time
from dataclasses import dataclass
from pathlib import Path
from types import TracebackType
from typing import Iterable, Sequence

MARKER_PREFIX = "RALPH-BOOTSTRAP"
RUNTIME_MARKER_PREFIX = "RALPH-MANAGED-BLOCK"  # must never appear here.
MARKER_VERSION = "v1"

# Sibling-tmp suffix for ``AtomicWriter._tmp_path``. The writer derives
# the rest of the tmp filename from the writer's pid and a monotonic
# nanosecond stamp so two writers in the same process never collide on
# the sibling path.
_TMP_SUFFIX = ".bootstrap.tmp"


def _start_marker(marker_id: str) -> str:
    return f"<!-- {MARKER_PREFIX}-START: {marker_id} {MARKER_VERSION} -->"


def _end_marker(marker_id: str) -> str:
    return f"<!-- {MARKER_PREFIX}-END: {marker_id} -->"


def render_managed_section(marker_id: str, body_lines: Sequence[str]) -> str:
    """Render a managed section wrapped in START / END markers.

    ``body_lines`` are joined with ``"\\n"`` and the resulting block
    ends exactly at the END marker (no trailing newline). The byte-
    for-byte preservation contract requires this: any whitespace
    following the END marker is owned by the user and must pass
    through compose unchanged.
    """
    start = _start_marker(marker_id)
    end = _end_marker(marker_id)
    body = "\n".join(body_lines)
    if body and not body.endswith("\n"):
        body = body + "\n"
    return f"{start}\n{body}{end}"


@dataclass(frozen=True)
class MarkerParse:
    """Outcome of scanning a document for managed-section markers."""

    marker_id: str
    kind: str  # one of {"Missing", "Ok", "Duplicate", "Truncated", "Nested"}
    start: int | None = None
    end: int | None = None

    @property
    def is_ok(self) -> bool:
        return self.kind == "Ok"


def _marker_positions(text: str, marker_id: str) -> tuple[list[int], list[int]]:
    """Return the absolute char offsets of every START / END match."""
    starts = [m.start() for m in re.finditer(re.escape(_start_marker(marker_id)), text)]
    ends = [m.start() for m in re.finditer(re.escape(_end_marker(marker_id)), text)]
    return starts, ends


def parse_managed_section(text: str, marker_id: str) -> MarkerParse:
    """Classify the document with respect to ``marker_id``.

    Classification rules:

    * ``Missing`` — zero START and zero END markers.
    * ``Ok`` — exactly one START followed by exactly one END.
    * ``Duplicate`` — more than one START or more than one END.
    * ``Truncated`` — START exists but END does not.
    * ``Nested`` — END comes before the START marker in the document;
      i.e. the markers are out of order even when both exist.

    ``start`` / ``end`` carry the absolute offsets into ``text`` for the
    Ok variant. For every other variant both offsets are ``None``.
    """
    starts, ends = _marker_positions(text, marker_id)
    if not starts and not ends:
        return MarkerParse(marker_id=marker_id, kind="Missing")
    if len(starts) > 1 or len(ends) > 1:
        return MarkerParse(marker_id=marker_id, kind="Duplicate")
    if starts and not ends:
        return MarkerParse(marker_id=marker_id, kind="Truncated")
    if ends and not starts:
        return MarkerParse(marker_id=marker_id, kind="Nested")
    # Exactly one START and one END: when END precedes START we treat
    # that as Nested (markers are out of order).
    start_idx = starts[0]
    end_idx = ends[0]
    if end_idx < start_idx:
        return MarkerParse(marker_id=marker_id, kind="Nested")
    return MarkerParse(
        marker_id=marker_id,
        kind="Ok",
        start=start_idx,
        end=end_idx + len(_end_marker(marker_id)),
    )


@dataclass(frozen=True)
class ComposeResult:
    """Result of a compose call.

    Exactly one of ``text`` / ``code`` is populated depending on the
    ``kind``:

    * ``kind == "created"`` — ``existing_text`` was ``None``; ``text``
      is the freshly-authored document.
    * ``kind == "updated"`` — the managed section was added or replaced;
      ``text`` is the new document.
    * ``kind == "noop"`` — the managed section byte-equalled the desired
      section and no change is required; ``text`` is ``existing_text``
      verbatim.
    * ``kind == "blocker"`` — a conflict was detected and no write is
      safe; ``code`` carries the parser code and ``reason`` explains
      the stop.
    """

    kind: str
    text: str | None = None
    code: str = ""
    reason: str = ""

    @property
    def is_blocker(self) -> bool:
        return self.kind == "blocker"


def _slice_ok(text: str, marker_id: str, parse: MarkerParse) -> tuple[str, str, str]:
    """Return ``(pre, body, post)`` for a successful ``Ok`` parse.

    ``pre`` is the text before the START marker. ``body`` is everything
    between the START and END markers, with a single trailing newline
    stripped. ``post`` is everything after the END marker.
    """
    assert parse.start is not None and parse.end is not None
    start_marker = _start_marker(marker_id)
    end_marker = _end_marker(marker_id)
    body_start = parse.start + len(start_marker) + 1
    body_end = parse.end - len(end_marker) - 1
    pre = text[: parse.start]
    body = text[body_start:body_end]
    if body.endswith("\n"):
        body = body[:-1]
    post = text[parse.end :]
    return pre, body, post


def compose_agent_docs(
    existing_text: str | None,
    owned_section_body: str,
    *,
    marker_id: str,
    sync_with_other_doc: bool = False,
    other_existing_text: str | None = None,
    other_body: str | None = None,
) -> ComposeResult:
    """Compose a managed-section update against an existing doc.

    When ``existing_text`` is ``None`` the helper authors a fresh
    document containing only the managed section. When the doc already
    carries one well-formed managed section the helper replaces it and
    preserves the surrounding user prose byte-for-byte.

    When ``sync_with_other_doc`` is ``True`` the helper cross-checks
    that the prospective other-doc body matches the body the caller is
    asking for. If they disagree the helper returns
    ``ComposeResult(kind="blocker", code="sync_mirror_conflict", ...)``
    so the caller can stop instead of writing asymmetric pairs.
    """
    if sync_with_other_doc:
        if other_body is not None:
            if owned_section_body.strip() != other_body.strip():
                return ComposeResult(
                    kind="blocker",
                    code="sync_mirror_conflict",
                    reason=(
                        "AGENTS.md and CLAUDE.md disagree on the "
                        "managed-section body; reconcile before "
                        "composing"
                    ),
                )
        elif other_existing_text is not None:
            other_parse = parse_managed_section(other_existing_text, marker_id)
            if other_parse.kind == "Ok":
                _, other_current_body, _ = _slice_ok(
                    other_existing_text, marker_id, other_parse
                )
                if other_current_body.strip() != owned_section_body.strip():
                    return ComposeResult(
                        kind="blocker",
                        code="sync_mirror_conflict",
                        reason=(
                            "AGENTS.md and CLAUDE.md disagree on the "
                            "managed-section body; reconcile before "
                            "composing"
                        ),
                    )

    if existing_text is None:
        section = render_managed_section(marker_id, owned_section_body.splitlines())
        return ComposeResult(kind="created", text=section)

    parse = parse_managed_section(existing_text, marker_id)
    if parse.kind == "Missing":
        # No managed section yet — append at end of file (or top if empty).
        section = render_managed_section(marker_id, owned_section_body.splitlines())
        base = existing_text if existing_text else ""
        if base and not base.endswith("\n"):
            base = base + "\n"
        new_text = base + section
        return ComposeResult(kind="updated", text=new_text)

    if parse.kind in {"Truncated", "Duplicate", "Nested"}:
        codes = {
            "Truncated": "marker_truncated",
            "Duplicate": "marker_duplicate",
            "Nested": "marker_nested",
        }
        return ComposeResult(
            kind="blocker",
            code=codes[parse.kind],
            reason=(
                f"managed section is {parse.kind.lower()}; "
                "manual reconciliation required before composing"
            ),
        )

    assert parse.kind == "Ok" and parse.start is not None and parse.end is not None
    pre, current_body, post = _slice_ok(existing_text, marker_id, parse)
    if current_body.strip() == owned_section_body.strip():
        return ComposeResult(kind="noop", text=existing_text)
    section = render_managed_section(marker_id, owned_section_body.splitlines())
    # ``post`` still carries whatever whitespace originally followed the
    # END marker (commonly ``"\n\n"`` so the doc ends with a blank line).
    # We splice the new section in front of ``post`` so the surrounding
    # whitespace is preserved byte-for-byte.
    new_text = pre + section + post
    return ComposeResult(kind="updated", text=new_text)


@dataclass
class _PlannedWrite:
    target: Path
    original: str | None  # None when the file did not exist before
    new_content: str
    tmp: Path  # sibling tmp path locked in at stage time


class AtomicWriter:
    """Atomic batch writer with rollback on first failure.

    Usage::

        with AtomicWriter(operations) as writer:
            committed, rolled_back = writer.execute()

    ``operations`` is an iterable of ``(target_path, new_content)``
    pairs. The writer first stages every pair into a sibling
    ``.{name}.{pid}.{monotonic_ns}.bootstrap.tmp`` file, then commits
    each target with an atomic ``os.replace``. If staging or
    committing any target fails, the writer rolls back every staged
    sibling so a partial update can never be observed by the next
    read. Commits also refuse to follow symlink targets so a
    co-located process cannot drive the writer into writing through a
    symlink it controls.
    """

    def __init__(self, operations: Iterable[tuple[Path | str, str]]) -> None:
        self._operations: list[tuple[Path, str]] = [
            (Path(target), content) for target, content in operations
        ]
        self._planned: list[_PlannedWrite] = []

    def __enter__(self) -> AtomicWriter:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        # ``execute`` already performs commits / rollback synchronously;
        # the context manager exists so ``with AtomicWriter(...) as w``
        # is the canonical usage that mirrors the dispatch contract.
        return None

    def execute(self) -> tuple[tuple[Path, ...], tuple[Path, ...]]:
        # Stage every target's new bytes into a sibling .tmp first; only
        # after all stages succeed do we commit each target with an
        # atomic rename. That way a failure on file N never leaves a
        # half-applied N-1 pair: either all targets are committed or
        # none are.
        try:
            for target, new_content in self._operations:
                self._stage(target, new_content)
        except OSError:
            # Staging the N-th target failed; the first N-1 have a .tmp
            # sibling but the original targets are still untouched.
            rolled = self._rollback(committed=(), report_all_planned=True)
            return (), tuple(rolled)
        committed: list[Path] = []
        try:
            for target, new_content in self._operations:
                self._commit(target, new_content)
                committed.append(target)
        except OSError:
            # Committing the N-th target failed; we have to undo every
            # already-committed target (1..N-1) by restoring its
            # original bytes. Targets that were staged-only also have
            # their sibling .tmp cleaned up.
            rolled = self._rollback(committed=committed, report_all_planned=True)
            return (), tuple(rolled)
        return tuple(committed), ()

    def _stage(self, target: Path, new_content: str) -> _PlannedWrite:
        original = target.read_text(encoding="utf-8") if target.exists() else None
        # Lock the tmp path at stage time so _commit and _rollback see the
        # same sibling file even though _tmp_path embeds a fresh
        # monotonic-ns stamp per call.
        tmp = self._tmp_path(target)
        planned = _PlannedWrite(
            target=target, original=original, new_content=new_content, tmp=tmp
        )
        self._planned.append(planned)
        tmp.parent.mkdir(parents=True, exist_ok=True)
        tmp.write_text(new_content, encoding="utf-8")
        return planned

    def _commit(self, target: Path, new_content: str) -> None:
        if target.is_symlink():
            # A co-located process can replace the target with a
            # symlink between _stage and _commit. Refuse to follow
            # symlinks here so the existing rollback path takes over
            # instead of writing through the symlink into attacker-
            # controlled bytes.
            raise OSError("AtomicWriter refuses to overwrite symlink target")
        planned = self._planned_for(target)
        os.replace(planned.tmp, target)

    def _planned_for(self, target: Path) -> _PlannedWrite:
        for planned in self._planned:
            if planned.target == target:
                return planned
        raise LookupError(f"no planned write for target {target!r}")

    def _rollback(
        self, *, committed: Sequence[Path], report_all_planned: bool = False
    ) -> list[Path]:
        rolled: list[Path] = []
        for planned in self._planned:
            target = planned.target
            tmp = planned.tmp
            if tmp.exists():
                try:
                    tmp.unlink()
                except OSError:
                    pass
            if target in committed:
                # The target was already replaced with new_content;
                # restore its original bytes when we have them,
                # otherwise delete it so partial updates can never be
                # observed.
                if planned.original is None:
                    try:
                        target.unlink()
                    except FileNotFoundError:
                        pass
                else:
                    target.write_text(planned.original, encoding="utf-8")
                rolled.append(target)
            elif report_all_planned:
                # Staged-only targets are reported as rolled-back so the
                # caller can reason about which files were touched by
                # the batch even when no commit happened yet.
                rolled.append(target)
        return rolled

    @staticmethod
    def _tmp_path(target: Path) -> Path:
        # Sibling tmp file with a unique-per-invocation suffix. Two
        # writers in the same process running on the same target must
        # never collide on the sibling tmp path, and a co-located
        # process must not be able to predict the path so it cannot
        # pre-stage a malicious sibling.
        return (
            target.parent
            / f".{target.name}.{os.getpid()}.{time.monotonic_ns()}{_TMP_SUFFIX}"
        )
