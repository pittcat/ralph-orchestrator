# Ralph Hat Collections

This directory contains the canonical built-in hat collections Ralph still ships and supports.

Built-ins are embedded into the CLI from these files and exposed through `ralph init --list-presets`.

## Supported Builtins

| Collection | Source | Best for |
|---|---|---|
| `autoresearch` | `presets/autoresearch.yml` | Autonomous experiment loop for any measurable improvement |
| `ce-executor` | `presets/ce-executor.yml` | Plan-driven work execution with wave code review, auto-fix, and manager report |
| `ce-executor-wave` | `presets/en/ce-executor-wave.yml` | Wave-based parallel variant of `ce-executor`; experimental, high-throughput for plans with multiple non-overlapping U-IDs per step |
| `code-assist` | `presets/code-assist.yml` | Default implementation workflow |
| `debug` | `presets/debug.yml` | Investigation and fix verification |
| `research` | `presets/research.yml` | Read-only exploration and synthesis |
| `review` | `presets/review.yml` | Adversarial code review |
| `pdd-to-code-assist` | `presets/pdd-to-code-assist.yml` | Advanced end-to-end idea-to-code workflow |

## Internal Presets

These remain loadable for Ralph internals or testing, but are intentionally hidden from normal builtin listings:

- `merge-loop`

## Product Positioning

- `code-assist` is the recommended default for implementation work.
- `pdd-to-code-assist` is intentionally kept as an advanced, fun example. It is slower, more expensive, and less predictable than `code-assist`.
- Other historical presets are now treated as documentation examples instead of supported builtins.

## Quick Start

```bash
ralph init --backend claude
ralph init --list-presets

ralph run -c ralph.yml -H builtin:autoresearch -p "Improve test coverage in src/core/"
ralph run -c ralph.yml -H builtin:code-assist -p "Add OAuth login"
ralph run -c ralph.yml -H builtin:debug -p "Investigate intermittent timeout"
ralph run -c ralph.yml -H builtin:research -p "Map auth architecture"
ralph run -c ralph.yml -H builtin:review -p "Review changes in src/api/"
ralph run -c ralph.yml -H builtin:pdd-to-code-assist -p "Build a new import pipeline"
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
ralph preset check -H builtin:ce-executor --strict

# Non-strict smoke (faster, ignores warnings)
ralph preset check -H builtin:ce-executor

# JSON output for diagnostics / CI
ralph preset check -H builtin:ce-executor --strict --format json
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
