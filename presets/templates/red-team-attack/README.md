# Red Team Attack Templates

Templates for the `red-team-attack` builtin preset.

## Usage

```bash
# Materialize templates into the operator workspace
ralph preset materialize-artifacts red-team-attack --plan-key <plan-key>

# Default output: .ralph/red-team/<plan-key>/templates/
```

## Files

| Template | Purpose | Copied to |
|---|---|---|
| `experiment.template.yml` | Experiment design + results | `.ralph/red-team/experiments/RTE-<NNN>.md` |
| `finding.template.yml` | Formal finding fields | `.ralph/red-team/findings/RTF-<NNN>.md` |
| `report.template.md` | Operator-facing report | `.ralph/red-team/REPORT.md` |
| `plan.template.md` | Repair plan deliverable | `.ralph/red-team/PLAN.md` |

## Rules

- Hats must `cp` from the materialized templates, never write free-form.
- Every section must be filled; write "N/A" with justification if not applicable.
- Templates are compile-time embedded into the `ralph` binary.
