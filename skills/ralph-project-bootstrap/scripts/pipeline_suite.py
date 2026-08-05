"""Pipeline-suite authoring helpers for ``ralph-project-bootstrap``.

The preset-bound bootstrap pipeline owns two artifacts inside the target project:

* ``ralph.<preset-stem>.yml`` — runtime config plus embedded provenance.
* ``PROMPT.<preset-stem>.md`` — the resolved preset's inline prompt snapshot.

Legacy low-level helpers for generic config/prompt/provenance composition remain
available for existing callers. New bootstrap flows must enter through
``compose_preset_bound_suite``.

Every persistent edit goes through the helpers in this module. They are
deliberately pure: no shell, no ``.ralph`` writes, no chmod, no PyYAML
round-trip of user keys. The only filesystem touch that ever reaches
the target project is the AtomicWriter from ``agent_docs``; this
module produces the new bytes it hands over.

Design rules (enforced by the helpers themselves):

* **Owned state lives under a top-level ``_bootstrap:`` mapping.** The
  preset-bound flow adds hashes for the generated profile and prompt.
* **The prompt is executable input.** Snapshot the literal
  ``event_loop.prompt`` because Ralph intentionally filters that field from
  hats-source overlays.
* **Provenance is embedded.** ``reconcile_preset_bound_suite`` blocks when
  either on-disk file disagrees with its recorded digest.
* **Hand-rolled YAML emitter for the owned block.** PyYAML is imported
  when available for parsing only; emission of the owned block uses
  explicit string composition so quote style, ordering and indentation
  stay stable across runs (no idempotency drift).
"""
from __future__ import annotations

import hashlib
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence

GENERATOR_VERSION = "0.3.0"

BASELINE_GUARDRAILS: tuple[str, ...] = (
    "Re-read the supplied business input and the nearest project instructions at the start of each activation.",
    "Keep changes scoped to the requested work and preserve unrelated operator changes.",
    "Never push remotes, create or switch branches/worktrees, or stop the Ralph process; the operator owns those actions.",
    "Never report completion while required verification is failing, skipped, or replaced by placeholder evidence.",
)

# Owned keys that always exist under the top-level ``_bootstrap:`` mapping
# of ``ralph.pipeline.yml``. Operators may add additional top-level user
# keys; they must never be re-quoted under ``_bootstrap:``.
PIPELINE_OWNED_KEYS: tuple[str, ...] = (
    "preset",
    "plan",
    "prompt_file",
    "preflight",
)

# Forbidden strings inside emitted prompt bytes: the prompt must never
# reference the runtime's hat registry, the project's managed-block
# markers, or any internal ledger path.
PROMPT_FORBIDDEN_PATTERNS: tuple[str, ...] = (
    "ralph-hats",
    "RALPH-MANAGED-BLOCK",
    "RALPH-BOOTSTRAP-START",
    "events.jsonl",
    ".ralph/supervisor.db",
)


class OwnedYamlError(ValueError):
    """Raised when owned YAML parsing / mutation fails."""

    def __init__(self, code: str, reason: str = "") -> None:
        super().__init__(reason or code)
        self.code = code
        self.reason = reason or code


@dataclass(frozen=True)
class Provenance:
    """Provenance record for a generated pipeline suite.

    ``owned_keys`` is the tuple of owned keys that the suite ships
    with (relative to the suite files). ``summary`` enumerates the
    on-disk bytes the suite owns, paired with their SHA-256 digest; the
    pair makes upgrades blockable when the operator hand-edits a
    owned section.
    """

    generator_version: str
    input_signature: str
    owned_keys: tuple[str, ...]
    summary: tuple[tuple[str, str], ...]

    @property
    def is_well_formed(self) -> bool:
        if not self.generator_version:
            return False
        if not self.input_signature:
            return False
        if not self.owned_keys:
            return False
        return True


@dataclass(frozen=True)
class PipelineSuite:
    """In-memory representation of a generated pipeline suite."""

    config: str
    prompt: str | None
    provenance: Provenance
    owned_keys_in_config: tuple[str, ...]
    owned_keys_in_prompt: tuple[str, ...]


@dataclass(frozen=True)
class PresetBoundPaths:
    """Repo-relative artifact paths derived from one preset identity."""

    config: str
    prompt: str


@dataclass(frozen=True)
class PresetBoundSuite:
    """Self-contained bootstrap output for a preset-backed Ralph run."""

    config_path: str
    prompt_path: str
    config: str
    prompt: str

    @property
    def provenance_path(self) -> None:
        """Provenance is embedded in ``config``; no sidecar is emitted."""
        return None


@dataclass(frozen=True)
class PresetBoundApplyResult:
    """Safe reconciliation result for a preset-bound two-file suite."""

    kind: str
    suite: PresetBoundSuite | None = None
    code: str = ""
    reason: str = ""

    @property
    def is_blocker(self) -> bool:
        return self.kind == "blocker"


@dataclass(frozen=True)
class ApplyResult:
    """Outcome of composing a suite against an existing config file."""

    kind: str  # one of {"created", "updated", "noop", "blocker"}
    text: str | None = None
    code: str = ""
    reason: str = ""

    @property
    def is_blocker(self) -> bool:
        return self.kind == "blocker"


@dataclass(frozen=True)
class UpgradeResult:
    """Outcome of reconciling an on-disk provenance file."""

    kind: str  # one of {"noop", "upgraded", "blocker"}
    text: str | None = None
    code: str = ""
    reason: str = ""

    @property
    def is_blocker(self) -> bool:
        return self.kind == "blocker"


def _guardrails_from_facts(project_facts: Any | None) -> tuple[str, ...]:
    """Extract the audited project overlay without coupling to audit.py."""
    if project_facts is None:
        return ()
    renderer = getattr(project_facts, "runtime_guardrails", None)
    if not callable(renderer):
        raise OwnedYamlError(
            "owned_yaml_invalid",
            "project_facts must expose runtime_guardrails()",
        )
    return tuple(renderer())


def derive_preset_bound_paths(preset: str) -> PresetBoundPaths:
    """Derive collision-resistant suite paths from a file or builtin preset."""
    if not preset:
        raise OwnedYamlError("owned_yaml_invalid", "preset is required")
    raw_name = preset.removeprefix("builtin:") if preset.startswith("builtin:") else Path(preset).name
    stem = raw_name
    for suffix in (".yaml", ".yml"):
        if stem.lower().endswith(suffix):
            stem = stem[: -len(suffix)]
            break
    if not stem or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", stem):
        raise OwnedYamlError(
            "owned_yaml_invalid",
            f"preset name cannot produce a safe artifact stem: {preset}",
        )
    return PresetBoundPaths(
        config=f"ralph.{stem}.yml",
        prompt=f"PROMPT.{stem}.md",
    )


def extract_inline_preset_prompt(preset_text: str) -> str:
    """Return the literal ``event_loop.prompt`` carried by a resolved preset."""
    try:
        import yaml  # type: ignore[import-not-found]
    except ImportError as exc:  # pragma: no cover - environment contract
        raise OwnedYamlError(
            "owned_yaml_invalid", "PyYAML is required to parse preset YAML"
        ) from exc
    try:
        loaded = yaml.safe_load(preset_text)
    except yaml.YAMLError as exc:
        raise OwnedYamlError("owned_yaml_invalid", f"preset YAML is invalid: {exc}") from exc
    event_loop = loaded.get("event_loop") if isinstance(loaded, dict) else None
    prompt = event_loop.get("prompt") if isinstance(event_loop, dict) else None
    if not isinstance(prompt, str) or not prompt.strip():
        raise OwnedYamlError(
            "preset_prompt_missing",
            "preset does not contain a non-empty event_loop.prompt; supply a plan or prompt explicitly",
        )
    return prompt


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


def _sha256_hex(data: str | bytes) -> str:
    if isinstance(data, str):
        data = data.encode("utf-8")
    return hashlib.sha256(data).hexdigest()


def compute_input_signature(
    preset: str,
    plan_path: str | None,
    cwd_anchor: str,
    prompt_file: str | None = None,
    manage_prompt: bool = False,
    requires_plan: bool = False,
    project_guardrails: Sequence[str] = (),
) -> str:
    """Compute the deterministic input signature for a suite.

    The signature feeds the provenance record so two generates with
    the same inputs collide on the same digest. ``cwd_anchor`` is the
    repo-relative anchor (typically ``"./"``); callers must not pass
    absolute paths.
    """
    ownership = "managed" if manage_prompt else "referenced"
    plan_contract = "required" if requires_plan else "optional"
    payload = (
        f"{preset}|{prompt_file or ''}|{plan_path or ''}|{ownership}|"
        f"{plan_contract}|{cwd_anchor}|" + "\n".join(project_guardrails)
    )
    return _sha256_hex(payload)


def _yaml_escape(value: str) -> str:
    """Minimal scalar escape for the hand-rolled YAML emitter.

    We deliberately avoid PyYAML's quoting so the on-disk form stays
    byte-stable across runs. Strings that contain YAML special
    characters are quoted with double quotes and have their inner
    double quote and backslash escaped.
    """
    if value == "":
        return '""'
    safe = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-./,"
    if all(ch in safe for ch in value) and value.strip() == value:
        return value
    escaped = value.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


def _render_mapping_lines(items: Mapping[str, str], indent: str = "  ") -> list[str]:
    return [f"{indent}{_yaml_escape(key)}: {_yaml_escape(value)}" for key, value in items.items()]


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def render_pipeline_yml(
    *,
    preset: str,
    plan_path: str | None = None,
    prompt_file: str | None = None,
    backend: str,
    budget_max_iterations: int,
    budget_wall_clock_seconds: int,
    preflight_strict: bool = True,
    diagnostics_enabled: bool = True,
    project_root_marker: str = "./",
    project_guardrails: Sequence[str] = (),
) -> str:
    """Render ``ralph.pipeline.yml`` with explicit owned keys.

    The owned keys live under a top-level ``_bootstrap:`` mapping; user
    keys live at the top level and are preserved byte-for-byte by
    ``apply_owned_keys_to_existing_config``.

    The user-keys block emits the fields that ``RalphConfig`` actually
    consumes (see ``crates/ralph-core/src/config/mod.rs`` /
    ``loop_config.rs`` / ``cli.rs``):

    * ``cli.backend`` — overrides the per-config backend (see
      ``CliConfig.backend``); the real loader does NOT consult
      ``event_loop.backend`` or a top-level ``budget``.
    * ``event_loop.prompt_file`` — the prompt file the runtime reads.
    * ``event_loop.max_iterations`` — iteration ceiling.
    * ``event_loop.max_runtime_seconds`` — wall-clock ceiling.
    * ``core.project_root`` — anchor for repo-relative paths.

    The runtime does NOT understand a top-level ``diagnostics`` key. We
    emit diagnostic intent under the runtime's actual
    ``telemetry.runtime_diagnosis`` namespace when ``diagnostics_enabled``
    is set; see ``crates/ralph-core/src/config/telemetry.rs``. If the
    caller wants a different schema, the call must surface a blocker
    rather than silently invent a new top-level field.
    """
    if not preset:
        raise OwnedYamlError("owned_yaml_invalid", "preset is required")
    if plan_path and Path(plan_path).is_absolute():
        raise OwnedYamlError("owned_yaml_invalid", "plan path must be repo-relative")
    if prompt_file and Path(prompt_file).is_absolute():
        raise OwnedYamlError("owned_yaml_invalid", "prompt_file must be repo-relative")
    if budget_max_iterations <= 0:
        raise OwnedYamlError(
            "owned_yaml_invalid",
            "budget_max_iterations must be a positive integer",
        )
    if budget_wall_clock_seconds <= 0:
        raise OwnedYamlError(
            "owned_yaml_invalid",
            "budget_wall_clock_seconds must be a positive integer",
        )

    owned: dict[str, str] = {
        "preset": preset,
        "plan": plan_path or "",
        "prompt_file": prompt_file or "",
        "preflight": "strict" if preflight_strict else "lenient",
    }
    header = (
        "# Generated by ralph-project-bootstrap. Owned keys live under\n"
        "# ``_bootstrap:``; everything else is operator-owned and preserved\n"
        "# byte-for-byte across recompositions.\n"
        f"# generator_version: {GENERATOR_VERSION}\n"
    )
    lines: list[str] = []
    lines.extend(header.splitlines())
    # The runtime does NOT consult ``event_loop.backend``; the canonical
    # place to set the backend is ``cli.backend``.
    lines.append("cli:")
    lines.append(f"  backend: {_yaml_escape(backend)}")
    lines.append("  autonomous_idle_timeout_secs: 900")
    lines.append("event_loop:")
    if prompt_file:
        lines.append(f"  prompt_file: {_yaml_escape(prompt_file)}")
    lines.append(f"  max_iterations: {int(budget_max_iterations)}")
    lines.append(f"  max_runtime_seconds: {int(budget_wall_clock_seconds)}")
    lines.append("core:")
    lines.append(f"  project_root: {_yaml_escape(project_root_marker)}")
    lines.append("  guardrails:")
    guardrails = tuple(dict.fromkeys((*BASELINE_GUARDRAILS, *project_guardrails)))
    for guardrail in guardrails:
        lines.append(f"    - {_yaml_escape(guardrail)}")
    # Diagnostic intent goes under the runtime's actual namespace
    # (``telemetry.runtime_diagnosis``), not under an invented top-level
    # ``diagnostics`` key. We only emit the field when the operator
    # asked for diagnostics to be enabled so a user who wants plain
    # RalphConfig output is not silently forced into a nested telemetry
    # block.
    if diagnostics_enabled:
        lines.append("telemetry:")
        lines.append("  runtime_diagnosis:")
        lines.append("    enabled: true")
        lines.append("    write_artifacts: true")
        lines.append("    prompt_injection_enabled: true")
        lines.append("    max_repeated_recoveries: 2")
        lines.append("    max_prompt_findings: 8")
        lines.append("    artifact_retention: 20")
        lines.append("    max_prompt_chars: 4096")
        lines.append("    retry_window_iterations: 8")
    lines.append("_bootstrap:")
    for key in PIPELINE_OWNED_KEYS:
        lines.append(f"  {key}: {_yaml_escape(owned[key])}")
    return "\n".join(lines) + "\n"


def render_prompt_md(
    *,
    plan_path: str | None,
    preset: str,
    project_root: str,
    prompt_file: str = "PROMPT.pipeline.md",
    requires_plan: bool = False,
) -> str:
    """Render ``PROMPT.pipeline.md`` referencing the plan + preset.

    The prompt body never embeds hat instructions or preset contents:
    it just declares the inputs the runtime needs and points at the
    plan file the suite was generated against.
    """
    if plan_path and Path(plan_path).is_absolute():
        raise OwnedYamlError("owned_yaml_invalid", "plan path must be repo-relative")
    if Path(project_root).is_absolute():
        raise OwnedYamlError("owned_yaml_invalid", "project_root must be repo-relative")
    if prompt_file and Path(prompt_file).is_absolute():
        raise OwnedYamlError("owned_yaml_invalid", "prompt_file must be repo-relative")
    prompt_file_line = (
        f"- prompt_file: `{prompt_file}`\n" if prompt_file else ""
    )
    source_line = f"- plan: `{plan_path}`\n" if plan_path else ""
    instruction = (
        "Read the plan at the referenced path. "
        if plan_path
        else (
            "This is a safe bootstrap fallback. The selected preset requires "
            "a real plan supplied with `ralph run --plan <repo-relative-path>`. "
            "If this fallback prompt reaches an agent, stop and report that "
            "the required plan was not supplied; do not perform project work. "
            if requires_plan
            else "Use this prompt together with the selected preset. "
        )
    )
    body = (
        f"# Ralph Pipeline Prompt\n\n"
        f"- project_root: `{project_root}`\n"
        f"- preset: `{preset}`\n"
        f"{source_line}"
        f"{prompt_file_line}"
        f"\n"
        f"{instruction}Do not invent preset\n"
        "contents, do not look up hat collections by name, and do not\n"
        "read any runtime-managed block from the target project. The\n"
        "runtime injects the preset-specific instructions downstream.\n"
    )
    for forbidden in PROMPT_FORBIDDEN_PATTERNS:
        if forbidden in body:
            raise OwnedYamlError(
                "owned_yaml_invalid",
                f"prompt template inadvertently references '{forbidden}'",
            )
    return body


def render_provenance(suite: PipelineSuite) -> str:
    """Render ``ralph.bootstrap.yml`` for ``suite``.

    The provenance record carries the ``generator_version``, the
    ``input_signature`` computed from the suite's owned keys, the
    tuple of owned keys, and the per-file owned-bytes SHA-256 digest.
    """
    lines: list[str] = []
    lines.append("# Generated by ralph-project-bootstrap.")
    lines.append("# Do not hand-edit; regenerate via the bootstrap helper.")
    lines.append(f"generator_version: {_yaml_escape(suite.provenance.generator_version)}")
    lines.append(f"input_signature: {_yaml_escape(suite.provenance.input_signature)}")
    lines.append("owned_keys:")
    for key in suite.provenance.owned_keys:
        lines.append(f"  - {_yaml_escape(key)}")
    lines.append("summary:")
    for file_name, digest in suite.provenance.summary:
        lines.append(f"  - file: {_yaml_escape(file_name)}")
        lines.append(f"    sha256: {_yaml_escape(digest)}")
    return "\n".join(lines) + "\n"


def parse_owned_yaml(text: str) -> tuple[dict[str, Any], tuple[str, ...]]:
    """Split ``text`` into ``(user_keys, owned_keys)``.

    The owned block is identified by a literal top-level
    ``_bootstrap:`` mapping. Keys inside that mapping are returned as
    ``owned_keys`` (the keys of the mapping) and excluded from the
    user dict. Keys outside that mapping are returned as ``user_keys``.

    If ``_bootstrap:`` is missing, ``owned_keys`` is empty and
    ``user_keys`` carries every parsed key. If ``_bootstrap:`` is
    present but malformed (not a mapping, or any value is not a
    string) the function raises ``OwnedYamlError("owned_yaml_invalid")``.
    """
    try:
        import yaml  # type: ignore[import-not-found]
    except ImportError:
        # Without PyYAML we cannot safely round-trip arbitrary user YAML;
        # callers should arrange for PyYAML to be available. We surface a
        # deterministic error rather than guess at structure.
        raise OwnedYamlError(
            "owned_yaml_invalid",
            "PyYAML is required to parse existing ralph.pipeline.yml",
        )

    loaded = yaml.safe_load(text)
    if loaded is None:
        return {}, ()
    if not isinstance(loaded, dict):
        raise OwnedYamlError(
            "owned_yaml_invalid",
            "top-level YAML must be a mapping",
        )

    user_keys: dict[str, Any] = {}
    owned_keys: tuple[str, ...] = ()
    for key, value in loaded.items():
        if key == "_bootstrap":
            if not isinstance(value, dict):
                raise OwnedYamlError(
                    "owned_yaml_invalid",
                    "_bootstrap: must be a mapping of string keys to string values",
                )
            for sub_key, sub_value in value.items():
                if not isinstance(sub_value, str):
                    raise OwnedYamlError(
                        "owned_yaml_invalid",
                        f"_bootstrap.{sub_key} must be a string, got {type(sub_value).__name__}",
                    )
            owned_keys = tuple(value.keys())
        else:
            user_keys[key] = value
    return user_keys, owned_keys


def _split_owned_block(text: str) -> tuple[str, str, str] | None:
    """Find the ``_bootstrap:`` mapping block in ``text``.

    Returns ``(pre, owned_block_text, post)`` or ``None`` when there
    is no ``_bootstrap:`` top-level mapping.
    """
    lines = text.splitlines(keepends=True)
    bootstrap_idx = None
    for idx, line in enumerate(lines):
        stripped = line.lstrip()
        if stripped.startswith("_bootstrap:"):
            # Top-level only; ignore indented variants.
            if line.startswith("_bootstrap:"):
                bootstrap_idx = idx
                break
    if bootstrap_idx is None:
        return None

    indent = "  "
    block_end = len(lines)
    for idx in range(bootstrap_idx + 1, len(lines)):
        line = lines[idx]
        if line.strip() == "":
            continue
        if line.startswith(indent):
            continue
        block_end = idx
        break
    pre = "".join(lines[:bootstrap_idx])
    owned = "".join(lines[bootstrap_idx:block_end])
    post = "".join(lines[block_end:])
    return pre, owned, post


def _ensure_no_duplicate_top_level_keys(text: str) -> None:
    """Reject any duplicate top-level key (other than ``_bootstrap:``)."""
    seen: dict[str, int] = {}
    for line in text.splitlines():
        if not line or line[0] in {" ", "\t"}:
            continue
        if line.startswith("#"):
            continue
        if ":" not in line:
            continue
        key = line.split(":", 1)[0].strip()
        if key == "_bootstrap":
            continue
        seen[key] = seen.get(key, 0) + 1
    duplicates = [key for key, count in seen.items() if count > 1]
    if duplicates:
        raise OwnedYamlError(
            "duplicate_yaml_key",
            f"duplicate top-level keys detected: {sorted(duplicates)}",
        )


def apply_owned_keys_to_existing_config(
    existing_yaml_text: str,
    new_owned: dict[str, str],
) -> str:
    """Splice ``new_owned`` into ``existing_yaml_text``.

    The helper mutates **only** the ``_bootstrap:`` block. User keys,
    comments, blank lines, and ordering of any other top-level key
    pass through byte-for-byte.

    If ``existing_yaml_text`` contains any duplicate top-level keys
    other than ``_bootstrap:`` (which may legitimately appear once),
    the function raises ``OwnedYamlError("duplicate_yaml_key")``.
    """
    _ensure_no_duplicate_top_level_keys(existing_yaml_text)

    split = _split_owned_block(existing_yaml_text)
    if split is None:
        # No ``_bootstrap:`` yet: append a new block at end of file.
        base = existing_yaml_text
        if base and not base.endswith("\n"):
            base = base + "\n"
        if base and not base.endswith("\n\n"):
            base = base + "\n"
        new_block_lines = ["_bootstrap:"]
        for key in PIPELINE_OWNED_KEYS:
            new_block_lines.append(f"  {key}: {_yaml_escape(new_owned[key])}")
        new_block = "\n".join(new_block_lines) + "\n"
        return base + new_block

    pre, owned_block, post = split

    # The owned block is keyed by ``_bootstrap:`` followed by indented
    # children. We render a fresh owned block keyed in the canonical
    # order so the output stays idempotent across recompositions.
    new_block_lines = ["_bootstrap:"]
    for key in PIPELINE_OWNED_KEYS:
        new_block_lines.append(f"  {key}: {_yaml_escape(new_owned[key])}")
    new_owned_block = "\n".join(new_block_lines) + "\n"
    # Preserve the trailing blank-line gap that originally followed the
    # owned block: when the original block ended with a blank line we
    # keep one; otherwise we keep the leading line of ``post`` glued.
    if post.startswith("\n"):
        new_owned_block = new_owned_block + "\n"
    return pre + new_owned_block + post


# ---------------------------------------------------------------------------
# Compose + upgrade flows
# ---------------------------------------------------------------------------


def compose_suite(
    *,
    preset: str,
    plan_path: str | None = None,
    prompt_file: str | None = None,
    backend: str,
    budget_max_iterations: int,
    budget_wall_clock_seconds: int,
    preflight_strict: bool = True,
    diagnostics_enabled: bool = True,
    project_root_marker: str = "./",
    manage_prompt: bool | None = None,
    requires_plan: bool = False,
    project_guardrails: Sequence[str] = (),
    project_facts: Any | None = None,
) -> PipelineSuite:
    """Render the full suite in one shot.

    The provenance signature covers the preset, optional prompt/plan paths,
    prompt ownership, whether a plan is required, and the project anchor.
    """
    if plan_path and Path(plan_path).is_absolute():
        raise OwnedYamlError("owned_yaml_invalid", "plan path must be repo-relative")
    audited_guardrails = _guardrails_from_facts(project_facts)
    effective_guardrails = tuple(
        dict.fromkeys((*audited_guardrails, *project_guardrails))
    )
    config = render_pipeline_yml(
        preset=preset,
        plan_path=plan_path,
        prompt_file=prompt_file,
        backend=backend,
        budget_max_iterations=budget_max_iterations,
        budget_wall_clock_seconds=budget_wall_clock_seconds,
        preflight_strict=preflight_strict,
        diagnostics_enabled=diagnostics_enabled,
        project_root_marker=project_root_marker,
        project_guardrails=effective_guardrails,
    )
    manage_prompt = bool(prompt_file) if manage_prompt is None else manage_prompt
    if manage_prompt and not prompt_file:
        raise OwnedYamlError(
            "owned_yaml_invalid",
            "manage_prompt requires a repo-relative prompt_file",
        )
    prompt = None
    if prompt_file and manage_prompt:
        prompt = render_prompt_md(
            plan_path=plan_path,
            preset=preset,
            project_root=project_root_marker,
            prompt_file=prompt_file,
            requires_plan=requires_plan,
        )
    input_signature = compute_input_signature(
        preset,
        plan_path,
        project_root_marker,
        prompt_file,
        manage_prompt,
        requires_plan,
        effective_guardrails,
    )
    summary = [("ralph.pipeline.yml", _sha256_hex(config))]
    if prompt is not None:
        summary.append((prompt_file or "PROMPT.pipeline.md", _sha256_hex(prompt)))
    provenance = Provenance(
        generator_version=GENERATOR_VERSION,
        input_signature=input_signature,
        owned_keys=PIPELINE_OWNED_KEYS,
        summary=tuple(summary),
    )
    return PipelineSuite(
        config=config,
        prompt=prompt,
        provenance=provenance,
        owned_keys_in_config=PIPELINE_OWNED_KEYS,
        owned_keys_in_prompt=(),
    )


def compose_preset_bound_suite(
    *,
    preset: str,
    preset_text: str,
    plan_path: str | None = None,
    backend: str,
    budget_max_iterations: int,
    budget_wall_clock_seconds: int,
    preflight_strict: bool = True,
    diagnostics_enabled: bool = True,
    project_root_marker: str = "./",
    project_guardrails: Sequence[str] = (),
    project_facts: Any | None = None,
) -> PresetBoundSuite:
    """Compose the runnable, preset-specific two-file bootstrap contract.

    A hats source cannot supply ``event_loop.prompt`` through Ralph's preset
    overlay boundary. Bootstrap therefore snapshots that resolved preset text
    into a dedicated prompt file and points the generated core config at it.
    Provenance stays inside ``_bootstrap`` so no ``ralph.bootstrap.yml``
    sidecar is needed.
    """
    if plan_path and Path(plan_path).is_absolute():
        raise OwnedYamlError("owned_yaml_invalid", "plan path must be repo-relative")
    paths = derive_preset_bound_paths(preset)
    try:
        prompt = extract_inline_preset_prompt(preset_text)
        requires_plan = False
    except OwnedYamlError as exc:
        if exc.code != "preset_prompt_missing" or not plan_path:
            raise
        # Hats-source presets cannot carry event_loop.prompt through the
        # operator/preset merge boundary. A supplied plan is therefore the
        # authoritative prompt source; keep a managed, inert prompt file so
        # the preset-bound suite remains complete and provenance-protected.
        prompt = render_prompt_md(
            plan_path=plan_path,
            preset=preset,
            project_root=project_root_marker,
            prompt_file=paths.prompt,
            requires_plan=True,
        )
        requires_plan = True
    audited_guardrails = _guardrails_from_facts(project_facts)
    effective_guardrails = tuple(
        dict.fromkeys((*audited_guardrails, *project_guardrails))
    )
    config = render_pipeline_yml(
        preset=preset,
        plan_path=plan_path,
        prompt_file=paths.prompt,
        backend=backend,
        budget_max_iterations=budget_max_iterations,
        budget_wall_clock_seconds=budget_wall_clock_seconds,
        preflight_strict=preflight_strict,
        diagnostics_enabled=diagnostics_enabled,
        project_root_marker=project_root_marker,
        project_guardrails=effective_guardrails,
    )
    input_signature = _sha256_hex(
        "|".join(
            (
                compute_input_signature(
                    preset,
                    plan_path,
                    project_root_marker,
                    paths.prompt,
                    True,
                    requires_plan,
                    effective_guardrails,
                ),
                _sha256_hex(preset_text),
            )
        )
    )
    metadata = (
        f"  generator_version: {_yaml_escape(GENERATOR_VERSION)}\n"
        f"  input_signature: {_yaml_escape(input_signature)}\n"
        f"  profile_sha256: {_yaml_escape(_sha256_hex(config))}\n"
        f"  prompt_sha256: {_yaml_escape(_sha256_hex(prompt))}\n"
    )
    config = config + metadata
    return PresetBoundSuite(
        config_path=paths.config,
        prompt_path=paths.prompt,
        config=config,
        prompt=prompt,
    )


_EMBEDDED_PROVENANCE_KEYS: tuple[str, ...] = (
    "generator_version",
    "input_signature",
    "profile_sha256",
    "prompt_sha256",
)


def _config_without_embedded_provenance(config_text: str) -> str:
    prefixes = tuple(f"  {key}:" for key in _EMBEDDED_PROVENANCE_KEYS)
    split = _split_owned_block(config_text)
    if split is None:
        return config_text
    pre, owned, post = split
    clean_owned = "".join(
        line
        for line in owned.splitlines(keepends=True)
        if not line.startswith(prefixes)
    )
    return pre + clean_owned + post


def reconcile_preset_bound_suite(
    existing_config: str | None,
    existing_prompt: str | None,
    requested: PresetBoundSuite,
) -> PresetBoundApplyResult:
    """Create, refresh, or reject a preset-bound suite without a sidecar."""
    if existing_config is None and existing_prompt is None:
        return PresetBoundApplyResult(kind="created", suite=requested)
    if existing_config is None or existing_prompt is None:
        return PresetBoundApplyResult(
            kind="blocker",
            code="preset_suite_incomplete",
            reason="config and prompt must either both exist or both be absent",
        )
    try:
        import yaml  # type: ignore[import-not-found]
    except ImportError as exc:  # pragma: no cover - environment contract
        return PresetBoundApplyResult(
            kind="blocker", code="provenance_corrupt", reason=str(exc)
        )
    try:
        loaded = yaml.safe_load(existing_config)
    except yaml.YAMLError as exc:
        return PresetBoundApplyResult(
            kind="blocker", code="provenance_corrupt", reason=str(exc)
        )
    bootstrap = loaded.get("_bootstrap") if isinstance(loaded, dict) else None
    if not isinstance(bootstrap, dict) or any(
        not isinstance(bootstrap.get(key), str) for key in _EMBEDDED_PROVENANCE_KEYS
    ):
        return PresetBoundApplyResult(
            kind="blocker",
            code="provenance_corrupt",
            reason="embedded bootstrap provenance is missing or malformed",
        )
    if bootstrap["profile_sha256"] != _sha256_hex(
        _config_without_embedded_provenance(existing_config)
    ):
        return PresetBoundApplyResult(
            kind="blocker",
            code="owned_value_user_modified",
            reason="config bytes do not match embedded provenance",
        )
    if bootstrap["prompt_sha256"] != _sha256_hex(existing_prompt):
        return PresetBoundApplyResult(
            kind="blocker",
            code="owned_value_user_modified",
            reason="prompt bytes do not match embedded provenance",
        )
    if existing_config == requested.config and existing_prompt == requested.prompt:
        return PresetBoundApplyResult(kind="noop", suite=requested)
    return PresetBoundApplyResult(kind="updated", suite=requested)


def verify_preset_bound_files(
    project_root: Path, suite: PresetBoundSuite
) -> PresetBoundApplyResult:
    """Reopen a written suite and verify paths, bytes, and prompt binding."""
    root = project_root.resolve()
    config_path = root / suite.config_path
    prompt_path = root / suite.prompt_path
    if not config_path.is_file() or not prompt_path.is_file():
        return PresetBoundApplyResult(
            kind="blocker",
            code="preset_suite_incomplete",
            reason="preset-bound config and prompt must both exist before validation",
        )
    config_text = config_path.read_text(encoding="utf-8")
    prompt_text = prompt_path.read_text(encoding="utf-8")
    reconciled = reconcile_preset_bound_suite(config_text, prompt_text, suite)
    if reconciled.is_blocker:
        return reconciled
    if reconciled.kind != "noop":
        return PresetBoundApplyResult(
            kind="blocker",
            code="preset_suite_stale",
            reason="written suite does not match the requested preset inputs",
        )
    try:
        import yaml  # type: ignore[import-not-found]

        loaded = yaml.safe_load(config_text)
        effective_prompt = loaded["event_loop"]["prompt_file"]
    except Exception as exc:
        return PresetBoundApplyResult(
            kind="blocker", code="preset_suite_invalid", reason=str(exc)
        )
    if effective_prompt != suite.prompt_path:
        return PresetBoundApplyResult(
            kind="blocker",
            code="prompt_source_mismatch",
            reason=f"config references {effective_prompt!r}, expected {suite.prompt_path!r}",
        )
    return PresetBoundApplyResult(kind="noop", suite=suite)


def upgrade_provenance(existing: str, new: PipelineSuite) -> UpgradeResult:
    """Reconcile an on-disk provenance file against a fresh compose.

    Outcomes:

    * ``noop`` — on-disk provenance byte-equals the fresh one. The
      caller should skip any write.
    * ``upgraded`` — on-disk provenance differs but is structurally
      valid AND every recorded SHA-256 still matches the suite's
      current owned bytes AND the input signature is unchanged. The
      helper returns the freshly-rendered provenance text so the
      caller can write it back.
    * ``blocker`` — one of:

      - ``owned_value_user_modified`` — on-disk provenance disagrees
        with the suite's current owned bytes; the operator edited a
        owned section, refuse to overwrite.
      - ``provenance_corrupt`` — on-disk text cannot be parsed or is
        missing required fields.
      - ``input_signature_changed`` — on-disk provenance reports a
        different ``input_signature`` than the current compose. The
        suite must be regenerated from scratch.
    """
    if not existing.strip():
        # First-time generate: render and treat as upgraded.
        return UpgradeResult(
            kind="upgraded",
            text=render_provenance(new),
            code="",
            reason="",
        )
    try:
        parsed = _parse_provenance_text(existing)
    except OwnedYamlError as exc:
        return UpgradeResult(
            kind="blocker",
            code="provenance_corrupt",
            reason=exc.reason or exc.code,
        )
    if parsed is None:
        return UpgradeResult(
            kind="blocker",
            code="provenance_corrupt",
            reason="provenance file is not a mapping",
        )

    expected_generator = new.provenance.generator_version
    expected_signature = new.provenance.input_signature
    expected_summary = dict(new.provenance.summary)

    if parsed.get("generator_version") != expected_generator:
        return UpgradeResult(
            kind="blocker",
            code="provenance_corrupt",
            reason="generator_version mismatch on upgrade",
        )
    if parsed.get("input_signature") != expected_signature:
        return UpgradeResult(
            kind="blocker",
            code="input_signature_changed",
            reason="input_signature changed; regenerate the suite",
        )

    on_disk_summary = _summary_to_dict(parsed.get("summary"))
    if on_disk_summary != expected_summary:
        return UpgradeResult(
            kind="blocker",
            code="owned_value_user_modified",
            reason=(
                "owned-bytes SHA-256 mismatch; the operator edited the "
                "owned section by hand"
            ),
        )

    fresh_text = render_provenance(new)
    if fresh_text.strip() == existing.strip():
        return UpgradeResult(kind="noop", text=existing)
    return UpgradeResult(kind="upgraded", text=fresh_text)


def apply_pipeline_config(
    existing_text: str | None,
    *,
    preset: str,
    plan_path: str | None = None,
    prompt_file: str | None = None,
    backend: str,
    budget_max_iterations: int,
    budget_wall_clock_seconds: int,
    preflight_strict: bool = True,
    diagnostics_enabled: bool = True,
    project_root_marker: str = "./",
    manage_prompt: bool | None = None,
    requires_plan: bool = False,
    project_guardrails: Sequence[str] = (),
    project_facts: Any | None = None,
    refresh_generated_profile: bool = False,
    existing_provenance_text: str | None = None,
) -> ApplyResult:
    """Compute the new ``ralph.pipeline.yml`` bytes for a target project.

    The function is pure: callers feed it the on-disk text (or
    ``None`` for a fresh project) and receive an ``ApplyResult`` that
    either carries the new text or a blocker code. The atomic disk op
    lives in ``AtomicWriter``; this helper only computes the bytes.

    ``ApplyResult.kind`` is one of:

    * ``created`` — ``existing_text`` was ``None``. ``text`` carries
      the freshly-authored suite.
    * ``updated`` — the owned block was added or replaced; user
      content is preserved byte-for-byte.
    * ``noop`` — the existing owned block byte-equalled the requested
      owned values. ``text`` is ``existing_text`` verbatim.
    * ``blocker`` — a duplicate key was detected; ``code`` and
      ``reason`` are populated and ``text`` is the original input.
    """
    new_suite = compose_suite(
        preset=preset,
        plan_path=plan_path,
        prompt_file=prompt_file,
        backend=backend,
        budget_max_iterations=budget_max_iterations,
        budget_wall_clock_seconds=budget_wall_clock_seconds,
        preflight_strict=preflight_strict,
        diagnostics_enabled=diagnostics_enabled,
        project_root_marker=project_root_marker,
        manage_prompt=manage_prompt,
        requires_plan=requires_plan,
        project_guardrails=project_guardrails,
        project_facts=project_facts,
    )

    if existing_text is None:
        return ApplyResult(kind="created", text=new_suite.config)

    if refresh_generated_profile:
        if not existing_text.startswith("# Generated by ralph-project-bootstrap."):
            return ApplyResult(
                kind="blocker",
                code="profile_not_generator_owned",
                reason="refuse to refresh a pipeline not marked as bootstrap-generated",
                text=existing_text,
            )
        if not existing_provenance_text:
            return ApplyResult(
                kind="blocker",
                code="profile_provenance_required",
                reason="refresh requires the existing ralph.bootstrap.yml bytes",
                text=existing_text,
            )
        try:
            recorded = _parse_provenance_text(existing_provenance_text)
            recorded_summary = _summary_to_dict(
                recorded.get("summary") if recorded else None
            )
        except OwnedYamlError as exc:
            return ApplyResult(
                kind="blocker",
                code="provenance_corrupt",
                reason=exc.reason,
                text=existing_text,
            )
        if recorded_summary.get("ralph.pipeline.yml") != _sha256_hex(existing_text):
            return ApplyResult(
                kind="blocker",
                code="owned_value_user_modified",
                reason="pipeline bytes do not match existing provenance",
                text=existing_text,
            )
        if existing_text == new_suite.config:
            return ApplyResult(kind="noop", text=existing_text)
        return ApplyResult(kind="updated", text=new_suite.config)

    # Fast path: if the on-disk ``_bootstrap:`` block already carries the
    # exact values we would write, we treat the operation as a noop. This
    # is the canonical second-run behaviour expected by the fixture and
    # the contract suite; it lets on-disk files with non-canonical quote
    # styles (e.g. manually edited) round-trip without forcing a rewrite.
    try:
        _, existing_owned = parse_owned_yaml(existing_text)
    except OwnedYamlError as exc:
        return ApplyResult(
            kind="blocker",
            code=exc.code,
            reason=exc.reason,
            text=existing_text,
        )
    if set(existing_owned) == set(PIPELINE_OWNED_KEYS):
        try:
            existing_values = {
                key: _render_owned_value(existing_text, key)
                for key in PIPELINE_OWNED_KEYS
            }
        except OwnedYamlError as exc:
            return ApplyResult(
                kind="blocker",
                code=exc.code,
                reason=exc.reason,
                text=existing_text,
            )
        desired_values = {
            key: _render_owned_value(new_suite.config, key)
            for key in PIPELINE_OWNED_KEYS
        }
        if existing_values == desired_values:
            return ApplyResult(kind="noop", text=existing_text)

    try:
        new_owned = {key: _render_owned_value(new_suite.config, key) for key in PIPELINE_OWNED_KEYS}
    except OwnedYamlError as exc:
        return ApplyResult(
            kind="blocker",
            code=exc.code,
            reason=exc.reason,
            text=existing_text,
        )

    try:
        updated = apply_owned_keys_to_existing_config(existing_text, new_owned)
    except OwnedYamlError as exc:
        return ApplyResult(
            kind="blocker",
            code=exc.code,
            reason=exc.reason,
            text=existing_text,
        )

    if updated == existing_text:
        return ApplyResult(kind="noop", text=existing_text)
    return ApplyResult(kind="updated", text=updated)


# ---------------------------------------------------------------------------
# Internal parsing helpers (provenance)
# ---------------------------------------------------------------------------


def _parse_provenance_text(text: str) -> dict[str, Any] | None:
    """Minimal hand-rolled YAML reader for the provenance file.

    We deliberately avoid PyYAML's full loader for the provenance file
    so the parser stays stdlib-only and the round-trip is byte-stable
    for the limited owned shape we emit.
    """
    lines = [line.rstrip("\n") for line in text.splitlines()]
    parsed: dict[str, Any] = {}
    current_list: list[Any] | None = None
    current_list_key: str | None = None
    current_mapping: dict[str, Any] | None = None
    current_mapping_parent_key: str | None = None
    current_mapping_list_index: int | None = None

    def flush_mapping_entry() -> None:
        nonlocal current_mapping, current_mapping_parent_key, current_mapping_list_index
        if current_mapping is not None and current_list is not None:
            current_list.append(current_mapping)
        current_mapping = None
        current_mapping_parent_key = None
        current_mapping_list_index = None

    for raw in lines:
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        if raw.startswith("  - "):
            flush_mapping_entry()
            if current_list is None:
                current_list = []
                parsed[current_list_key] = current_list  # type: ignore[index]
            entry_value = raw[4:].strip()
            if ":" in entry_value:
                # List-of-mapping entries: ``- file: foo`` is the
                # first line of a multi-line mapping whose continuations
                # arrive as ``    sha256: bar``.
                key, _, value = entry_value.partition(":")
                current_mapping = {key.strip(): _coerce_scalar(value.strip())}
            else:
                current_list.append(_coerce_scalar(entry_value))
            continue
        if raw.startswith("    "):
            if current_mapping is None:
                raise OwnedYamlError(
                    "owned_yaml_invalid",
                    "indented mapping line without a parent list entry",
                )
            inner = raw.strip()
            if ":" in inner:
                key, _, value = inner.partition(":")
                current_mapping[key.strip()] = _coerce_scalar(value.strip())
            continue
        # flush any pending list item before a top-level scalar key
        flush_mapping_entry()
        current_list = None
        current_list_key = None
        if ":" not in raw:
            raise OwnedYamlError(
                "owned_yaml_invalid",
                f"malformed provenance line: {raw!r}",
            )
        key, _, value = raw.partition(":")
        key = key.strip()
        value = value.strip()
        if value == "":
            # Either a list (next line ``- foo``) or a mapping (next
            # line ``  file: ...``).
            current_list = []
            parsed[key] = current_list
            current_list_key = key
        else:
            parsed[key] = _coerce_scalar(value)
    flush_mapping_entry()
    return parsed


def _coerce_scalar(value: str) -> Any:
    if value.startswith('"') and value.endswith('"'):
        return value[1:-1]
    if value.startswith("'") and value.endswith("'"):
        return value[1:-1]
    if value in {"true", "True"}:
        return True
    if value in {"false", "False"}:
        return False
    return value


def _summary_to_dict(summary: Any) -> dict[str, str]:
    out: dict[str, str] = {}
    if not isinstance(summary, list):
        return out
    for entry in summary:
        if not isinstance(entry, dict):
            continue
        file_name = entry.get("file")
        digest = entry.get("sha256")
        if isinstance(file_name, str) and isinstance(digest, str):
            out[file_name] = digest
    return out


def _render_owned_value(config_text: str, key: str) -> str:
    """Extract the rendered value of an owned key from the suite config."""
    lines = config_text.splitlines()
    in_block = False
    for line in lines:
        if not in_block and line.startswith("_bootstrap:"):
            in_block = True
            continue
        if in_block:
            if line.startswith("  " + key + ":"):
                _, _, value = line.partition(":")
                value = value.strip()
                if value.startswith('"') and value.endswith('"'):
                    return value[1:-1].replace('\\"', '"').replace("\\\\", "\\")
                return value
            if line and not line.startswith("  "):
                break
    raise OwnedYamlError(
        "owned_yaml_invalid",
        f"could not find _bootstrap.{key} in freshly rendered config",
    )
