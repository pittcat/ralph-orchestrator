"""Deterministic preset inventory for the guided triage phase.

Phase 0 of ``ralph-project-bootstrap`` answers "which preset fits this
task?" before the unified pipeline runs. This module owns the factual
half of that answer: it collects every reachable preset — builtin ids
via ``ralph preset builtin list`` / ``preset builtin show`` and file
presets the caller names — and reduces each one to a typed
``PresetCandidate`` describing its launch contract. The semantic match
(task intent -> candidate ranking) stays agent judgement; this module
only guarantees the facts the agent compares are real.

Design rules:

* **Read-only.** The inventory never writes to the target project and
  never runs ``ralph run``; the only subprocess calls are the two
  introspection commands the pipeline resolver already trusts.
* **Fail-closed, never raise.** A missing binary, an unparseable list
  envelope, or a malformed preset YAML produces an ``InventoryIssue``
  row, not an exception; partial inventory is still returned so the
  agent can recommend from whatever resolved.
* **Runner injection.** ``runner`` follows ``subprocess.run``'s
  signature so tests drive the module with a deterministic fake; the
  real binary is never spawned at import time.
* **Descriptions are best-effort.** Builtin candidates take the
  ``description`` field from the list envelope. File presets have no
  description key; the inventory lifts the leading ``#`` comment block
  (the canonical preset-doc convention) as the description source and
  flags the candidate ``description_source="header_comment"`` so the
  agent knows it was derived, not declared.
* **File preset shadowing.** When a file preset's stem equals a live
  builtin id the candidate is flagged ``shadows_builtin=True``; the
  builtin copy is the vetted source and the triage phase prefers it.

Public surface:

* :class:`PresetCandidate`
* :class:`InventoryIssue`
* :class:`Inventory`
* :func:`collect_builtin_presets`
* :func:`collect_file_presets`
* :func:`run_inventory`
* :func:`render_json`
* :func:`main`
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Callable, Iterable, Mapping

import _paths  # type: ignore[import-not-found]  # loaded via skills/ralph-project-bootstrap/scripts on sys.path
import cli_probe  # type: ignore[import-not-found]  # DEFAULT_TIMEOUT

SubprocessRunner = Callable[..., subprocess.CompletedProcess]

# Cap on the operator-facing prompt excerpt embedded in each candidate.
# Long enough to expose the preset's input contract ("read the prompt
# for target branch + ordered branch list"), short enough to keep the
# inventory JSON scannable.
_PROMPT_EXCERPT_LIMIT = 400

# Cap on the derived file-preset description (leading comment block).
_DESCRIPTION_LIMIT = 600

# Cap on the joined per-hat "id: description" summary. Hat descriptions
# carry the preset's shape signal ("Finds root cause...", "Designs and
# runs experiments...") and are the fallback semantic material when no
# top-level description exists.
_HAT_SUMMARY_LIMIT = 500


# ---------------------------------------------------------------------------
# Typed records
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class PresetCandidate:
    """One reachable preset reduced to its triage-relevant contract."""

    preset_id: str  # builtin id or file stem
    source: str  # ``builtin:<id>`` or the file path token as supplied
    origin: str  # "builtin" | "file"
    description: str = ""
    description_source: str = ""  # "envelope" | "header_comment" | ""
    backend: str = ""  # ``cli.backend``; empty when absent
    hat_count: int = 0
    hat_ids: tuple[str, ...] = ()
    hat_summaries: str = ""  # joined "id: description" per hat, capped
    execution_mode: str = ""  # ``event_loop.execution_mode``
    supervisor_enabled: bool = False
    inline_prompt_present: bool = False
    prompt_excerpt: str = ""
    completion_promise: str = ""
    required_events: tuple[str, ...] = ()
    max_iterations: int | None = None
    max_runtime_seconds: int | None = None
    tasks_enabled: bool | None = None
    public: bool | None = None  # builtin list envelope flag; None for files
    shadows_builtin: bool = False  # file stem collides with a live builtin id
    contract_complete: bool = False  # backend + positive budgets parsed

    @property
    def input_contract(self) -> str:
        """Coarse launch-input hint derived from the prompt contract.

        ``self_prompted`` means the preset ships a non-empty
        ``event_loop.prompt``; the excerpt may still ask the operator
        for fields inside that prompt. ``needs_operator_input`` means
        the pipeline will require ``--plan`` or ``--prompt-file``
        before a launch command can be emitted.
        """
        return "self_prompted" if self.inline_prompt_present else "needs_operator_input"


@dataclass(frozen=True)
class InventoryIssue:
    """A single reason part of the inventory could not resolve."""

    code: str
    message: str
    source: str = ""  # which preset/call produced the issue


@dataclass(frozen=True)
class Inventory:
    """Aggregate outcome of :func:`run_inventory`."""

    binary: str
    presets: tuple[PresetCandidate, ...] = ()
    issues: tuple[InventoryIssue, ...] = ()

    def to_json(self) -> str:
        """Serialise the inventory to a JSON string.

        ``input_contract`` is a derived property, so it is merged into
        each preset row explicitly rather than relying on ``asdict``.
        """
        rows: list[dict[str, Any]] = []
        for candidate in self.presets:
            row = asdict(candidate)
            row["input_contract"] = candidate.input_contract
            rows.append(row)
        payload = {
            "binary": self.binary,
            "presets": rows,
            "issues": [asdict(issue) for issue in self.issues],
        }
        return json.dumps(payload, indent=2, ensure_ascii=False)


# ---------------------------------------------------------------------------
# YAML extraction
# ---------------------------------------------------------------------------


def _load_yaml_mapping(text: str) -> dict[str, Any]:
    """Parse ``text`` as YAML and return the top-level mapping."""
    import yaml  # type: ignore[import-not-found]

    loaded = yaml.safe_load(text)
    if not isinstance(loaded, dict):
        raise ValueError("top-level YAML must be a mapping")
    return loaded


def _excerpt(text: str, limit: int) -> str:
    """Collapse ``text`` to single-spaced excerpt capped at ``limit``."""
    collapsed = " ".join(text.split())
    if len(collapsed) <= limit:
        return collapsed
    return collapsed[: max(0, limit - 1)].rstrip() + "…"


def _extract_comment_description(text: str) -> str:
    """Lift the leading ``#`` comment block as a file-preset description.

    Canonical presets document themselves in a contiguous header comment
    (``# Merge Batch Preset`` / ``# ce-executor-pipeline: ...``). The
    inventory joins that block, drops the comment markers, and caps it.
    A file that starts with YAML keys (no comment block) yields ``""``.
    """
    lines: list[str] = []
    for raw in text.splitlines():
        stripped = raw.lstrip()
        if not stripped:
            if lines:
                # A blank line ends the header block once content started.
                break
            continue
        if not stripped.startswith("#"):
            break
        lines.append(stripped.lstrip("#").strip())
    return _excerpt("\n".join(lines), _DESCRIPTION_LIMIT)


def _candidate_from_mapping(
    *,
    preset_id: str,
    source: str,
    origin: str,
    loaded: Mapping[str, Any],
    description: str,
    description_source: str,
    public: bool | None,
) -> PresetCandidate:
    """Reduce a parsed preset mapping to a :class:`PresetCandidate`.

    Missing or malformed contract fields degrade to empty values plus
    ``contract_complete=False``; the inventory reports what it can
    rather than dropping the candidate outright.
    """
    cli = loaded.get("cli")
    backend = cli.get("backend") if isinstance(cli, dict) else ""
    backend = backend.strip() if isinstance(backend, str) else ""

    event_loop = loaded.get("event_loop")
    event_loop = event_loop if isinstance(event_loop, dict) else {}
    prompt = event_loop.get("prompt")
    inline_prompt = prompt.strip() if isinstance(prompt, str) else ""
    completion = event_loop.get("completion_promise")
    execution_mode = event_loop.get("execution_mode")
    supervisor = event_loop.get("supervisor")
    supervisor_enabled = (
        isinstance(supervisor, dict) and supervisor.get("enabled") is True
    )
    required = event_loop.get("required_events")
    required_events = (
        tuple(str(item) for item in required) if isinstance(required, list) else ()
    )

    def _budget(value: Any) -> int | None:
        try:
            parsed = int(value)
        except (TypeError, ValueError):
            return None
        return parsed if parsed > 0 else None

    max_iterations = _budget(event_loop.get("max_iterations"))
    max_runtime_seconds = _budget(event_loop.get("max_runtime_seconds"))

    hats = loaded.get("hats")
    hat_ids = (
        tuple(str(key) for key in hats.keys()) if isinstance(hats, dict) else ()
    )
    hat_summaries = ""
    if isinstance(hats, dict):
        hat_summaries = _excerpt(
            "; ".join(
                f"{key}: {value.get('description')}"
                for key, value in hats.items()
                if isinstance(value, dict)
                and isinstance(value.get("description"), str)
                and value.get("description").strip()
            ),
            _HAT_SUMMARY_LIMIT,
        )

    tasks = loaded.get("tasks")
    tasks_enabled = tasks.get("enabled") if isinstance(tasks, dict) else None

    contract_complete = bool(backend) and max_iterations is not None and (
        max_runtime_seconds is not None
    )
    return PresetCandidate(
        preset_id=preset_id,
        source=source,
        origin=origin,
        description=description,
        description_source=description_source if description else "",
        backend=backend,
        hat_count=len(hat_ids),
        hat_ids=hat_ids,
        hat_summaries=hat_summaries,
        execution_mode=execution_mode if isinstance(execution_mode, str) else "",
        supervisor_enabled=supervisor_enabled,
        inline_prompt_present=bool(inline_prompt),
        prompt_excerpt=_excerpt(inline_prompt, _PROMPT_EXCERPT_LIMIT),
        completion_promise=completion if isinstance(completion, str) else "",
        required_events=required_events,
        max_iterations=max_iterations,
        max_runtime_seconds=max_runtime_seconds,
        tasks_enabled=tasks_enabled,
        public=public,
        contract_complete=contract_complete,
    )


# ---------------------------------------------------------------------------
# Builtin inventory (binary introspection)
# ---------------------------------------------------------------------------


def _run_introspection(
    argv: list[str],
    runner: SubprocessRunner,
) -> tuple[subprocess.CompletedProcess | None, InventoryIssue | None]:
    """Run one introspection argv; return ``(proc, issue)``.

    Exactly one of the two slots is populated. Missing binary,
    timeouts, and non-zero exits all surface as typed issues.
    """
    try:
        proc = runner(
            argv,
            timeout=cli_probe.DEFAULT_TIMEOUT,
            capture_output=True,
            text=True,
        )
    except FileNotFoundError:
        return None, InventoryIssue(
            code="binary_missing",
            message=f"ralph binary not found: {argv[0]}",
            source=argv[0],
        )
    except subprocess.TimeoutExpired:
        return None, InventoryIssue(
            code="introspection_timeout",
            message=f"{' '.join(argv)} timed out",
            source=argv[0],
        )
    if proc.returncode != 0:
        return None, InventoryIssue(
            code="introspection_failed",
            message=(
                f"{' '.join(argv)} returned {proc.returncode}: "
                f"{(proc.stderr or '').strip()}"
            ),
            source=argv[0],
        )
    return proc, None


def collect_builtin_presets(
    *,
    binary: str,
    runner: SubprocessRunner = subprocess.run,
) -> tuple[tuple[PresetCandidate, ...], tuple[InventoryIssue, ...]]:
    """Inventory the binary's embedded builtin presets.

    Drives ``preset builtin list --format json`` then
    ``preset builtin show <id> --format yaml`` per listed id. The list
    envelope is the canonical ``{presets: [{id, source, description,
    public}]}`` shape — the same contract ``bootstrap_pipeline``'s
    builtin resolver consumes.
    """
    proc, issue = _run_introspection(
        [binary, "preset", "builtin", "list", "--format", "json"], runner
    )
    if issue is not None:
        return (), (InventoryIssue(code=issue.code, message=issue.message, source="builtin_list"),)
    assert proc is not None  # for type checkers; issue/None was handled above
    try:
        listing = json.loads(proc.stdout or "{}")
    except json.JSONDecodeError as exc:
        return (), (
            InventoryIssue(
                code="builtin_list_unparseable",
                message=f"builtin list emitted invalid JSON: {exc}",
                source="builtin_list",
            ),
        )
    entries = listing.get("presets") if isinstance(listing, dict) else None
    if not isinstance(entries, list):
        return (), (
            InventoryIssue(
                code="builtin_list_unparseable",
                message="builtin list response must carry a 'presets' array",
                source="builtin_list",
            ),
        )

    candidates: list[PresetCandidate] = []
    issues: list[InventoryIssue] = []
    for entry in entries:
        if not isinstance(entry, dict):
            continue
        preset_id = entry.get("id")
        if not isinstance(preset_id, str) or not preset_id:
            issues.append(
                InventoryIssue(
                    code="builtin_list_entry_invalid",
                    message="builtin list entry missing 'id'",
                    source="builtin_list",
                )
            )
            continue
        description = entry.get("description")
        public = entry.get("public")
        show_proc, show_issue = _run_introspection(
            [binary, "preset", "builtin", "show", preset_id, "--format", "yaml"],
            runner,
        )
        if show_issue is not None:
            issues.append(
                InventoryIssue(
                    code="builtin_show_failed",
                    message=show_issue.message,
                    source=f"builtin:{preset_id}",
                )
            )
            continue
        assert show_proc is not None
        text = show_proc.stdout or ""
        try:
            loaded = _load_yaml_mapping(text)
        except Exception as exc:  # YAML error or non-mapping payload
            issues.append(
                InventoryIssue(
                    code="builtin_show_unparseable",
                    message=f"builtin show {preset_id} emitted invalid YAML: {exc}",
                    source=f"builtin:{preset_id}",
                )
            )
            continue
        candidates.append(
            _candidate_from_mapping(
                preset_id=preset_id,
                source=f"builtin:{preset_id}",
                origin="builtin",
                loaded=loaded,
                description=description if isinstance(description, str) else "",
                description_source="envelope",
                public=public if isinstance(public, bool) else None,
            )
        )
    return tuple(candidates), tuple(issues)


# ---------------------------------------------------------------------------
# File preset inventory
# ---------------------------------------------------------------------------


def collect_file_presets(
    paths: Iterable[str],
    *,
    root: Path,
) -> tuple[tuple[PresetCandidate, ...], tuple[InventoryIssue, ...]]:
    """Inventory caller-supplied repo-relative file preset paths.

    Paths pass the same lexical input boundary as the pipeline
    (``_paths.is_safe_relative``): absolute paths and ``..`` escapes are
    reported as ``input_path_unsafe`` issues, never read. All reads are
    anchored on ``root`` so "exists at root R" and "reads from root R"
    agree.
    """
    candidates: list[PresetCandidate] = []
    issues: list[InventoryIssue] = []
    seen: set[str] = set()
    for raw in paths:
        token = raw.strip()
        if not token or token in seen:
            continue
        seen.add(token)
        if not _paths.is_safe_relative(token):
            issues.append(
                InventoryIssue(
                    code="input_path_unsafe",
                    message=f"preset path must be a safe repo-relative token: {raw!r}",
                    source="file_preset",
                )
            )
            continue
        path = root / token
        if not path.is_file():
            issues.append(
                InventoryIssue(
                    code="input_missing_preset_file",
                    message=f"preset file not readable: {token}",
                    source=token,
                )
            )
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as exc:
            issues.append(
                InventoryIssue(
                    code="input_missing_preset_file",
                    message=f"preset file not readable: {token} -- {exc}",
                    source=token,
                )
            )
            continue
        try:
            loaded = _load_yaml_mapping(text)
        except Exception as exc:
            issues.append(
                InventoryIssue(
                    code="preset_yaml_invalid",
                    message=f"preset file is not valid YAML: {token} -- {exc}",
                    source=token,
                )
            )
            continue
        candidates.append(
            _candidate_from_mapping(
                preset_id=Path(token).stem,
                source=token,
                origin="file",
                loaded=loaded,
                description=_extract_comment_description(text),
                description_source="header_comment",
                public=None,
            )
        )
    return tuple(candidates), tuple(issues)


def _scan_presets_dir(root: Path, directory: str) -> tuple[tuple[str, ...], InventoryIssue | None]:
    """Return sorted repo-relative ``*.yml``/``*.yaml`` tokens under ``directory``."""
    if not _paths.is_safe_relative(directory):
        return (), InventoryIssue(
            code="input_path_unsafe",
            message=f"presets dir must be a safe repo-relative token: {directory!r}",
            source="presets_dir",
        )
    target = root / directory
    if not target.is_dir():
        return (), InventoryIssue(
            code="presets_dir_missing",
            message=f"presets dir not found: {directory}",
            source=directory,
        )
    tokens = tuple(
        sorted(
            path.relative_to(root).as_posix()
            for pattern in ("*.yml", "*.yaml")
            for path in target.glob(pattern)
            if path.is_file()
        )
    )
    return tokens, None


# ---------------------------------------------------------------------------
# Top-level entry
# ---------------------------------------------------------------------------


def run_inventory(
    *,
    binary: str = "ralph",
    preset_paths: Iterable[str] = (),
    presets_dirs: Iterable[str] = (),
    cwd: Path | str = ".",
    include_builtins: bool = True,
    runner: SubprocessRunner = subprocess.run,
) -> Inventory:
    """Collect the full preset inventory for one target project.

    ``cwd`` anchors every file-preset read; builtin introspection runs
    under the caller's environment (the binary lives on ``PATH`` or at
    the explicit ``binary`` path). Results are deterministic given the
    same inputs; ordering is builtin candidates first (envelope order),
    then file candidates (caller order, deduped by token).
    """
    root = Path(cwd).resolve()
    issues: list[InventoryIssue] = []
    candidates: list[PresetCandidate] = []

    if include_builtins:
        builtin_candidates, builtin_issues = collect_builtin_presets(
            binary=binary, runner=runner
        )
        candidates.extend(builtin_candidates)
        issues.extend(builtin_issues)

    file_tokens: list[str] = list(preset_paths)
    for directory in presets_dirs:
        scanned, dir_issue = _scan_presets_dir(root, directory)
        if dir_issue is not None:
            issues.append(dir_issue)
        else:
            file_tokens.extend(scanned)
    file_candidates, file_issues = collect_file_presets(file_tokens, root=root)
    issues.extend(file_issues)

    builtin_ids = {c.preset_id for c in candidates if c.origin == "builtin"}
    for candidate in file_candidates:
        candidates.append(
            PresetCandidate(
                **{
                    **asdict(candidate),
                    "shadows_builtin": candidate.preset_id in builtin_ids,
                }
            )
        )
    return Inventory(binary=binary, presets=tuple(candidates), issues=tuple(issues))


def render_json(inventory: Inventory) -> str:
    """Return the canonical JSON view of an :class:`Inventory`."""
    return inventory.to_json()


def render_text(inventory: Inventory) -> str:
    """Return a compact human-readable inventory table."""
    lines: list[str] = []
    for candidate in inventory.presets:
        markers: list[str] = []
        if candidate.shadows_builtin:
            markers.append("shadows-builtin")
        if not candidate.contract_complete:
            markers.append("incomplete-contract")
        suffix = f"  [{', '.join(markers)}]" if markers else ""
        lines.append(
            f"{candidate.source}  ({candidate.origin}, {candidate.hat_count} hats, "
            f"backend={candidate.backend or '?'}, {candidate.input_contract}){suffix}"
        )
        if candidate.description:
            lines.append(f"    {candidate.description}")
    for issue in inventory.issues:
        lines.append(f"! {issue.code}: {issue.message}")
    if not lines:
        return "no presets discovered"
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    """CLI entry point for the inventory helper."""
    parser = argparse.ArgumentParser(
        description="Collect the reachable preset inventory (builtin + file) "
        "as typed JSON for the bootstrap triage phase."
    )
    parser.add_argument("--binary", default="ralph", help="ralph binary path/name")
    parser.add_argument(
        "--preset",
        dest="preset_paths",
        action="append",
        default=[],
        help="repo-relative file preset path (repeatable)",
    )
    parser.add_argument(
        "--presets-dir",
        dest="presets_dirs",
        action="append",
        default=[],
        help="repo-relative directory of *.yml/*.yaml presets (repeatable)",
    )
    parser.add_argument("--cwd", default=".", help="target project root (default .)")
    parser.add_argument(
        "--no-builtin",
        action="store_true",
        help="skip builtin introspection (file presets only)",
    )
    parser.add_argument(
        "--format",
        choices=("json", "text"),
        default="json",
        help="output format (default json)",
    )
    args = parser.parse_args(argv)
    inventory = run_inventory(
        binary=args.binary,
        preset_paths=args.preset_paths,
        presets_dirs=args.presets_dirs,
        cwd=args.cwd,
        include_builtins=not args.no_builtin,
    )
    if args.format == "json":
        print(render_json(inventory))
    else:
        print(render_text(inventory))
    return 0


if __name__ == "__main__":
    sys.exit(main())
