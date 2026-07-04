# Payload Contracts

Payload contracts define what fields each event's payload **must** carry,
enforced both at preset-load time (static) and at runtime (dynamic).
They close the gap where a hat's `instructions` block reads `task_id` from
a `work.ready` event but the producer forgets to include the field.

Payload contracts are complementary to **execution contracts** (which
verify the agent's *completion declaration* — e.g. `work.done` carries
`task_id` and a closed runtime task). Payload contracts verify the
*event shape* that flows between hats.

---

## Why Two Layers?

| Layer | Catches | When |
|---|---|---|
| Execution contract | "agent claimed `work.done` but no closed task / no git change" | `ralph run` event policy, observe-mode (does not pause the loop) |
| Payload contract | "executor reads `plan_name` from `work.ready` but the topic's schema does not declare it" | `ralph run` startup hard gate + `ralph hats validate` + runtime `Enforce` mode |

The two layers are required and cannot replace each other:

- Execution contract without payload contract: a hat can still crash at
  runtime because the upstream topic lacks a field the hat depends on.
- Payload contract without execution contract: a hat can publish a
  topic with all required fields but lie about whether the work is
  actually done.

---

## Three Mechanisms

| Mechanism | When it runs | What it does |
|---|---|---|
| `ralph hats validate` (default mode) | `cargo run -- hats validate` | Reports payload contract warnings for any topic with payload refs but no schema. Does not fail. |
| `ralph hats validate --strict` | `cargo run -- hats validate --strict` | Reports payload contract errors and exits non-zero if any required topic has a missing field or missing schema. |
| `ralph run` startup hard gate | Always, before spawning any backend | Same as `--strict`; cannot be bypassed (no `--skip-payload-check` flag). |
| Runtime `Enforce` mode | When `event_policy.mode: enforce` and a real event arrives missing a required field | Loop pauses, diagnostic JSON written to `.ralph/diagnostics/payload-contract-error-{timestamp}.json`, terminal exit code 1. |

---

## Declaring a Schema

Schemas are declared under `event_policy.schemas.<topic>.required_fields`
inline, or in a separate YAML file referenced by `event_policy.schema_file`.

### Inline

```yaml
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    schemas:
      work.ready:
        required_fields: [plan_name, task_id, task_key, step]
      work.done:
        required_fields: [plan_name, task_id, task_key, step]
```

### External file (preferred for multi-topic presets)

Place the schema in a sibling file and reference it via `schema_file`.
The path is resolved relative to the preset's containing directory.

```yaml
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    schema_file: "schemas/ce-executor.yml"
```

Where `schemas/ce-executor.yml` lives next to the preset yml.

### Schema format

A schema file is a YAML map of topic name → schema:

```yaml
work.ready:
  required_fields:
    - plan_name
    - task_id
    - task_key
    - step
  payload: json_object   # optional; structural type check (default)

work.failed:
  required_fields:
    - reason
```

- `required_fields` — list of field names that must be present in the
  event payload.
- `payload` — optional structural type. `json_object` is the default
  and the only structural type currently enforced at runtime.
- Inline schemas take priority over file schemas when both define the
  same topic.

---

## Extractor Behaviour

The static validator (`validate_payload_contract`) uses a conservative
extractor (`extract_payload_field_refs`) to find payload field
references in hat instructions. Three patterns are matched:

| Pattern | Example line in instructions | Field extracted |
|---|---|---|
| `From event payload: <field1>, <field2>, ...` | `From event payload: task_id, plan_name, step` | `task_id`, `plan_name`, `step` |
| `payload MUST include: <field1>, <field2>, ...` | `payload MUST include: task_id, task_key` | `task_id`, `task_key` |
| Backtick field on line with explicit intent | `From event payload: read \`task_id\`, \`task_key\`` | `task_id`, `task_key` |

What the extractor does **not** match (to avoid false positives):

- Lines that merely contain the word "payload" without the intent
  prefixes above.
- Backticked values that are not bare identifiers (containing spaces,
  dots, slashes, hyphens, or non-alphanumeric chars). Topic names
  like `` `work.done` `` and file paths like `` `fix-log.md` `` are
  ignored.
- Hashed comments or code fences.

### Tuning per-hat

If your hat instructions include a payload field that you want the
extractor to skip (e.g., a field name that happens to collide with a
real identifier but isn't a contract field), use
`HatConfig.ignore_payload_fields`:

```yaml
hats:
  executor:
    ignore_payload_fields: [legacy_field, debug_only]
```

`ignore_payload_fields` is consumed only by the static validator. The
runtime event policy still enforces whatever the schema declares.

---

## Runtime Violation Diagnostic

When a real event violates a schema in `Enforce` mode, the loop pauses
and writes a JSON file under
`.ralph/diagnostics/payload-contract-error-{timestamp}.json`.

The file's contents:

```json
{
  "error_type": "missing_required_field",
  "timestamp": "2026-06-03T12:34:56.789Z",
  "topic": "work.ready",
  "field": "plan_name",
  "source_hat": ["coordinator"],
  "target_hat": ["executor"],
  "schema_defined_in": "inline + file:schemas/ce-executor.yml",
  "downstream_reference": null,
  "upstream_reference": null,
  "fix_hint": "Add the missing field to the payload of the 'work.ready' event. ...",
  "payload_excerpt": "{\"task_id\": \"t-1\"}"
}
```

| Field | Meaning |
|---|---|
| `error_type` | `missing_required_field` / `payload_type_mismatch` / `allowed_value_mismatch` / `schema_missing_for_required_topic` |
| `source_hat` | Hat ID(s) that published the topic (the producer side). Multiple values listed if several hats can publish. |
| `target_hat` | Hat ID(s) subscribed to the topic (the consumer side). |
| `schema_defined_in` | `inline`, `file:<path>`, or `inline + file:<path>` when both define the same topic. |
| `fix_hint` | Actionable hint for the operator or the producing hat. |
| `payload_excerpt` | First 240 chars of the offending payload (truncated for safety). |

The terminal also prints
`[PAYLOAD CONTRACT VIOLATION] Loop paused. Diagnostic written to <path>`.
If the diagnostic write fails (e.g., disk full, permission denied), the
violation summary is still printed on stderr and the loop still
terminates with non-zero exit code — the diagnostic file is
informational, not the source of truth for the violation.

---

## Builtin Schema Library

As of 2026-06-03 the builtin presets inline their payload schemas
directly under `event_policy.schemas` in `presets/en/<name>.yml`. The
previous `event_policy.schema_file: "../schemas/<name>.yml"` form
was broken for builtin presets — `resolve_schema_files` has no
on-disk anchor to resolve the relative path against, so the schemas
silently went unloaded and the payload contract hard gate failed
with `SchemaMissingForRequiredTopic` for every topic the preset
declared. (Symptom: `ralph run -H builtin:ce-executor-serial -p "..."` reports
"Subprocess exited before starting the orchestration loop" with the
real cause buried in `.ralph/diagnostics/logs/`.)

For file-based hat collections (e.g. `ralph run -H .ralph/hats/my.yml`)
the relative `schema_file` form still works, but the inline form is
preferred for consistency and for portability across install
locations.

The `presets/schemas/` directory is the authoring SSOT for builtin
preset schemas. It is merged into the embedded preset at compile time
by `crates/ralph-cli/build.rs`.

| Schema file | Preset that owns the schemas | Status |
|---|---|---|
| `presets/schemas/ce-executor-serial.yml` | `ce-executor-serial` | Authoring SSOT (merged into `presets/en/ce-executor-serial.yml` at build time) |

When adding a new schema to a builtin preset:

1. Edit `presets/schemas/<name>.yml` for the schema SSOT, or add an
   `event_policy.schemas.<topic>` entry directly in `presets/en/<name>.yml`
   as an override layer.
2. Keep `presets/en/<name>.yml` and `presets/schemas/<name>.yml` in sync;
   run `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded`
   for SSOT byte-equality where applicable.
3. Run `ralph hats validate --strict -H builtin:<name>` to confirm
   no schema warnings or errors.

---

## CLI Reference

```bash
# Default mode: report warnings; do not fail.
ralph hats validate -c ralph.yml -H .ralph/hats/my-flow.yml

# Strict mode: report errors; exit non-zero on any payload contract violation.
ralph hats validate --strict -c ralph.yml -H .ralph/hats/my-flow.yml

# `ralph run` automatically runs the strict hard gate. There is no skip flag.
ralph run -c ralph.yml -H builtin:ce-executor-serial -p "..."
```

---

## Boundary With Execution Contracts

| Question | Payload contract | Execution contract |
|---|---|---|
| "Did the agent actually finish the work?" | — | yes (`work.done` requires task closed, git diff or commit, optional test evidence) |
| "Did the agent emit an event with the right shape?" | yes (required fields per topic) | partial (`work.done` only; the `work.done` rule declares `require_payload_fields`) |
| "Where in the loop does it fire?" | Static: at preset load. Runtime: on every JSONL event with `Enforce`. | Static: at preset load. Runtime: on `work.done` events only. |
| "What happens on violation?" | Static: non-zero exit. Runtime: pause + diagnostic JSON. | Runtime: rejection with `task.resume` recovery (does not pause the loop). |
| "Can the agent bypass it?" | No. | No. |

The two contracts are required and complementary; disabling either
opens a regression.

---

## See also

- `harness-extensions.md` — the four opt-in Harness 4 mechanisms
- `execution-contracts.md` — the `work.done` completion gate
- `docs/plans/2026-06-02-005-feat-payload-contract-validation-plan.md` —
  the design and implementation plan
