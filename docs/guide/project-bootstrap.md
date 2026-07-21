# Cross-Project Bootstrap — Provenance Guide

> Operator-facing reference for the provenance three-key written by
> `ralph-project-bootstrap` into `ralph.bootstrap.yml`. Use this when
> deciding whether to refresh, what each field means, and how the
> bootstrap YAML relates to `ralph.pipeline.yml` and
> `PROMPT.pipeline.md`.

## Why provenance exists

When `ralph-project-bootstrap` fills missing runtime artifacts for an
external project, it ships a deterministic, repeatable record under
`ralph.bootstrap.yml` → `_bootstrap:`. That record lets later runs
detect:

- The suite was produced by a known generator version.
- The inputs the suite was generated against still match the current
  audit (no silent drift).
- The owned block on disk has not been hand-edited since generation.

Without provenance, bootstrap can re-emit identical YAML forever even
when the upstream audit or owned bytes change — that is the failure
mode the three keys exist to prevent.

Source: `skills/ralph-project-bootstrap/scripts/pipeline_suite.py:33-92`.

## The three keys in `ralph.bootstrap.yml`

| Key | Type | What it captures | When it changes |
|-----|------|------------------|-----------------|
| `generator_version` | string | The bootstrap generator version (currently `0.2.0`). | When the bootstrap skill is upgraded. |
| `input_signature` | string | A digest over the preset, optional prompt/plan paths, and project anchor. | When any launch input changes. |
| per-file SHA-256 | string | A `path: sha256` entry for every file the suite owns, under `_bootstrap:`. | When the owned bytes change. |

If any of these three are missing, malformed, or stale, the next
bootstrap run refuses to silently overwrite the owned block and
returns an `OwnedYamlError` instead.

Source: `skills/ralph-project-bootstrap/scripts/pipeline_suite.py:81-95`
(`Provenance` dataclass).

## When to refresh

Refresh the bootstrap suite whenever **any** of the following holds:

1. **Generator upgrade.** Bumping the bootstrap skill's
   `generator_version` requires a clean re-emission so the new
   generator is the one stamping the owned block.
2. **Input drift.** Audit results, fixture sources, or the operator
   preset or optional launch-input choice changed since the last bootstrap. Run
   `ralph-project-bootstrap` again to recompute `input_signature`.
3. **Hand-edit detected.** The per-file SHA-256 entry diverges from
   the on-disk owned bytes (operator edited the block directly). This
   is treated as a blocker — either revert the hand-edit or accept the
   refresh that re-stamps the owned block.

Do **not** hand-edit the `_bootstrap:` block. The bootstrap skill is
the only sanctioned writer.

## Relationship to `ralph.pipeline.yml` and `PROMPT.pipeline.md`

`ralph.bootstrap.yml` carries **provenance** for the suite; it does
**not** run anything by itself. The runtime artifacts are:

- `ralph.pipeline.yml` — runtime configuration for the preset and optional
  prompt/plan inputs. It is written only when the project needs an override.
- `PROMPT.pipeline.md` — optional bootstrap-owned prompt. An existing
  operator-owned prompt can be referenced without being rewritten; a
  preset-native run needs no prompt artifact.

Only files actually owned by bootstrap have their SHA-256 listed in
`ralph.bootstrap.yml` `summary`. If bootstrap refuses to refresh and the inputs have not
changed, the operator's first check should be: does the file on disk
match the SHA-256 listed in provenance? If not, the file was edited
out-of-band.

Source: `skills/ralph-project-bootstrap/scripts/pipeline_suite.py:108-116`
(`PipelineSuite` dataclass).

## Operator checklist

When the bootstrap suite refuses to refresh, walk this list before
overriding:

- [ ] Confirm `generator_version` matches the running bootstrap
      version (`ralph tools memory search "ralph-project-bootstrap"` or
      `python -c "import skills.ralph_project_bootstrap as b; print(b.GENERATOR_VERSION)"`).
- [ ] Confirm `input_signature` matches the current audit output
      (re-run the audit; if its digest changed, refresh is correct).
- [ ] Diff each path in `summary` against the on-disk SHA-256 to
      detect hand-edits (`shasum -a 256 <path>` per entry).
- [ ] If a hand-edit is intentional, accept the refresh explicitly
      and re-stamp the suite; otherwise revert the hand-edit first.
- [ ] After refresh, run the project's authoritative test suite
      (`python -m pytest skills/tests/test_project_bootstrap_contract.py
      skills/tests/test_project_bootstrap_e2e.py`) to confirm the
      refresh did not break existing contracts.
