# Pipeline Suite Authoring

This document is implementation guidance for the next skill implementor
working on `ralph-project-bootstrap` and the `scripts/pipeline_suite.py`
helper. It explains the owned-key contract for `ralph.pipeline.yml`, the
provenance model in `ralph.bootstrap.yml`, and the upgrade rules that
govern re-running the helper against an existing suite.

End agents (the hat processes Ralph spawns) do **not** see this file.
The skill owner does.

## Files the helper owns

The bootstrap pipeline owns exactly three files inside the target
project:

* `ralph.pipeline.yml` — runtime config binding preset + optional plan +
  optional prompt file + preflight strictness, plus a baseline runtime profile
  and project-backed `core.guardrails`. User-owned keys live outside the owned
  block.
* `PROMPT.pipeline.md` — generated only when bootstrap owns a prompt. It
  references the optional plan path and preset id and never copies hat
  instructions. Operator-owned prompt files may be referenced but never
  rendered or hashed by bootstrap. Preset-native mode creates no prompt file.
  For a plan-driven preset whose first plan is not available yet, the managed
  prompt is a safe fallback: it requires `--plan`, performs no project work if
  reached directly, and allows the reusable suite to be provisioned now.
* `ralph.bootstrap.yml` — provenance: generator version, input
  signature, owned-key tuple, per-file owned-bytes SHA-256 digest.

No other files are touched by `pipeline_suite.py`. The skill never
writes `.ralph/` content, never edits `AGENTS.md` / `CLAUDE.md`
through this helper, and never creates derivative files such as
`ralph.yml` or `PROMPT.default.md`.

## Owned-key contract

`ralph.pipeline.yml` carries **exactly four owned keys** under a
top-level `_bootstrap:` mapping:

| Key | Type | Notes |
| --- | --- | --- |
| `preset` | string | preset path or `builtin:<id>`. The helper quotes this value when it contains a `:`. |
| `plan` | string | Optional repo-relative plan/task path; empty for non-plan runs. Absolute paths are rejected. |
| `prompt_file` | string | Optional repo-relative prompt path; empty when the preset supplies its own prompt. Absolute paths are rejected. |
| `preflight` | string | `"strict"` (default) or `"lenient"`. |

The owned block is delimited by the `_bootstrap:` line and the first
non-indented line that follows it. Anything outside that block is
operator-owned: it is preserved byte-for-byte across recompositions,
including comments, blank lines, key ordering, and quote style on
non-owned keys.

For a newly generated file, start from `../assets/ralph.pipeline.base.yml` and
overlay evidence gathered from the target project. The user-keys block commonly
carries:

```yaml
cli:
  backend: <one of claude|codex|kiro|...>
event_loop:
  max_iterations: <int>
  max_runtime_seconds: <int>
core:
  project_root: ./
  guardrails:
    - <generic baseline rule>
    - <rule containing only commands verified in this project>
telemetry:
  runtime_diagnosis:
    enabled: true|false
```

…but operators may add additional top-level keys (e.g. `state_projection:`,
`event_policy:`). The helper never rewrites or reorders those keys.

## Render contract

`render_pipeline_yml(...)` emits the canonical owned-block bytes in
this order:

```yaml
_bootstrap:
  preset: <value>
  plan: <value>
  prompt_file: <value>
  preflight: strict|lenient
```

The owned keys are rendered with the minimum quoting needed to keep
the YAML valid: identifiers made of `[A-Za-z0-9_-./,]` are emitted
bare; values that contain YAML special characters (notably `:`) are
double-quoted with `\"` and `\\` escapes applied.

## Apply contract

`apply_owned_keys_to_existing_config(existing_yaml_text, new_owned)`
mutates **only** the `_bootstrap:` block. User keys, comments, blank
lines, and the order of any other top-level key pass through
byte-for-byte. The function rejects duplicate top-level keys (other
than a single `_bootstrap:` entry) by raising
`OwnedYamlError("duplicate_yaml_key")` so a malformed suite cannot be
silently overwritten.

`apply_pipeline_config(existing_text | None, ...)` is the higher-level
entry point:

* `existing_text is None` → returns `ApplyResult(kind="created", text=...)`.
* owned values byte-equal → returns `ApplyResult(kind="noop", text=existing_text)`.
* owned values differ but file is well-formed → returns
  `ApplyResult(kind="updated", text=...)`.
* duplicate top-level key detected → returns
  `ApplyResult(kind="blocker", code="duplicate_yaml_key", ...)`.

For a whole-profile refresh of an older generated config, pass
`refresh_generated_profile=True` together with the existing provenance text.
The helper verifies the recorded `ralph.pipeline.yml` SHA-256 before replacing
the generated profile; a header alone never proves ownership.

Atomic disk ops live in `agent_docs.AtomicWriter`. This helper only
computes the new bytes; the writer is the only place that touches
the filesystem.

## Provenance model

`render_provenance(suite)` emits a YAML file with these fields:

| Field | Type | Notes |
| --- | --- | --- |
| `generator_version` | string | semver-like; currently `0.3.0`. |
| `input_signature` | string | SHA-256 of preset + optional prompt path + optional plan path + prompt ownership (`managed` / `referenced`) + plan requirement (`required` / `optional`) + cwd anchor. |
| `owned_keys` | list of string | the four owned keys, in canonical order. |
| `summary` | list of `{file, sha256}` | SHA-256 of each suite file's on-disk owned bytes. |

The `summary` SHA-256 is computed over the on-disk bytes of the suite
file. The hash is what the upgrade gate inspects to detect hand-edits.

## Upgrade rules

`upgrade_provenance(existing, new)` returns one of:

* `noop` — on-disk provenance byte-equals the freshly-rendered one.
  No write is needed.
* `upgraded` — on-disk provenance differs but is structurally valid,
  the `input_signature` matches the new compose, and the recorded
  `summary` SHA-256s still match the current owned bytes. The helper
  returns the freshly-rendered provenance text so the caller can
  write it back.
* `blocker(code, reason)` — exactly one of:

  - `owned_value_user_modified` — the on-disk `summary` SHA-256s do
    not match the current suite bytes. The operator edited the owned
    section by hand; the helper refuses to overwrite.
  - `provenance_corrupt` — on-disk text cannot be parsed or is
    missing required fields. The operator must reconcile by hand.
  - `input_signature_changed` — the inputs the suite was generated
    against no longer match the current compose. The suite must be
    regenerated from scratch.

The "compute inputs to a stable SHA-256" property is what makes the
upgrade path safe: a regenerated suite with identical inputs yields
the same `input_signature`, so the upgrade gate treats it as the
"upgraded" outcome rather than blocking.

## Prohibited patterns

The following are forbidden in any artifact the helper emits and will
be rejected by reviewers:

* **Reference to `ralph-hats` or any preset name inside the prompt.**
  A managed prompt is template-shaped: it REFs the preset id and optional plan path,
  but never copies hat instructions and never names specific hat
  collections.
* **Reference to runtime-internal ledgers.** The prompt must never
  mention `.ralph/events.jsonl`, `.ralph/supervisor.db`, or any
  internal ledger path. These are runtime-owned, not skill-owned.
* **Reuse of the runtime managed-block markers.** The bootstrap
  marker prefix is `RALPH-BOOTSTRAP-`; the runtime prefix is
  `RALPH-MANAGED-BLOCK-`. The two namespaces must never overlap.
* **Absolute paths in YAML or in the prompt.** Every persisted
  scalar must be repo-relative. The helper rejects `Path.is_absolute()`
  inputs with `OwnedYamlError("owned_yaml_invalid")` at the top of
  every render entry point.
* **Hand-rolled quoting that round-trips through PyYAML.** The helper
  uses a hand-rolled emitter so the on-disk form stays stable across
  runs. Do not introduce PyYAML round-trips for the owned block;
  PyYAML is used only to *parse* an existing file and split it into
  user / owned halves.
* **Side channels.** `pipeline_suite.py` is pure stdlib + PyYAML (for
  parsing). It does not import `subprocess`, does not call `os.system`,
  does not chmod files, and does not read environment variables.

## Acceptance checklist

When extending `pipeline_suite.py`, run the contract suite:

```bash
.venv/bin/python -m pytest skills/tests/test_project_bootstrap_contract.py
```

Plus a targeted run for each new branch:

```bash
.venv/bin/python -m pytest skills/tests/test_project_bootstrap_contract.py -k pipeline
```

The full suite must remain green; the new helper must not regress any
existing audit or agent-docs behaviour.
