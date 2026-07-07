# Ralph Hat Collections

This directory contains the canonical built-in hat collections Ralph still ships and support.

Built-ins are embedded into the CLI from these files and exposed through `ralph init --list-presets`.

## Builtin vs Template: What's the Difference?

| Term | What it is | Where to find it |
|------|------------|-----------------|
| **Builtin Preset** | Ralph's officially shipped workflow | `presets/*.yml` (this directory) |
| **Template** | Scaffold for creating your own workflow | `ralph preset list` (authoring tool) |
| **Local Preset** | Your customized YAML file | You create with `ralph preset new` |

Templates generate **ordinary YAML** with `x_preset` metadata. They do not become builtins or require special maintenance. See [Preset Authoring Guide](../docs/guide/preset-authoring.md) for details.

## Supported Builtins

| Collection | Source | Best for |
|---|---|---|
| `autoresearch` | `presets/en/autoresearch.yml` | Autonomous experiment loop for any measurable improvement |
| `ce-executor-pipeline` | `presets/en/ce-executor-pipeline.yml` | One-shot whole-plan execution: plan reviewer → executor → 6 serial dimension reviewers → review-synthesizer → fix-planner → fixer → alignment → reporter (isolated multi-hat execution) |
| `debug` | `presets/en/debug.yml` | Investigation and fix verification |

## Internal Presets

These remain loadable for Ralph internals or testing, but are intentionally hidden from normal builtin listings:

- `merge-loop`

## Product Positioning

- `ce-executor-pipeline` is the recommended default for plan-driven implementation work.
- `debug` is the dedicated preset for bug investigation and adversarial fix verification.
- `autoresearch` is a specialized loop for metric-driven experimentation.
- Other historical presets (e.g. `code-assist`, `research`, `review`, `pdd-to-code-assist`) are now treated as documentation examples instead of supported builtins.

## Quick Start

```bash
ralph init --backend claude
ralph init --list-presets

ralph run -c ralph.yml -H builtin:autoresearch -p "Improve test coverage in src/core/"
ralph run -c ralph.yml -H builtin:ce-executor-pipeline -p "docs/plans/my-plan.md"
ralph run -c ralph.yml -H builtin:debug -p "Investigate intermittent timeout"
```

## Examples Instead of Builtins

Example workflow patterns now live in the docs rather than as shipped preset files. See:

- `docs/examples/`
- `presets/COLLECTION.md`

## Source Of Truth

- Canonical builtins: `presets/*.yml`
- Builtin index: `presets/index.json`
- Embedded CLI mirror: `crates/ralph-cli/presets/*.yml`
- Sync script: `./scripts/sync-embedded-files.sh`

## Authoring Workflow: `ralph preset check`

Editing or creating a preset? Run the contract check before pushing:

```bash
# Strict authoring check (recommended for CI and PR gates)
ralph preset check -H builtin:ce-executor-pipeline --strict

# Non-strict smoke (faster, ignores warnings)
ralph preset check -H builtin:ce-executor-pipeline

# JSON output for diagnostics / CI
ralph preset check -H builtin:ce-executor-pipeline --strict --format json
```

`ralph preset check` covers four authoring concerns in one pass:

- `config` — `RalphConfig::validate()` semantic warnings and errors
- `topology` — starting event, completion promise, required events reachability
- `orphan` — published topics with no specific subscriber (typos and stale publishes)
- `payload` — declared schemas vs. fields actually referenced by downstream hats

This is the recommended entry point for preset authors. `ralph hats validate`
keeps its narrower hat-debug focus; `ralph preflight` is for environment +
config checks before a run. See `docs/guide/runtime-contracts.md` for the
full behavior matrix and strict-semantics table.

### Batch Validation

To run the same check across every public builtin preset:

```bash
./scripts/validate-builtin-presets.sh           # non-strict, exempts known topology gaps
./scripts/validate-builtin-presets.sh --strict  # strict, no exemptions
```

The script reads its preset list from `presets/index.json` (single source of
truth), so adding a public preset to the index automatically widens the
regression gate.
