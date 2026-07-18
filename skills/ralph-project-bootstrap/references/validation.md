# Static Validation Proof Levels

This document is the implementation reference for the staged static
gate in `ralph-project-bootstrap`. The gate proves that a freshly
authored pipeline suite (preset + plan + prompt + config + preflight)
is structurally compatible with the local `ralph` binary before any
backend call is attempted. It is **not** proof that a loop has run,
that a loop can run, or that the suite is correct.

End agents do not see this file. The skill owner does.

## The four-stage state machine

The gate runs in strict order:

1. **capability** — probe the binary's `--help` / `--version` / JSON
   help output and confirm that every required command+flag
   combination is observable.
2. **preset check** — invoke `ralph -c <config> -H <preset> preset
   check --strict` and require exit zero.
3. **preflight** — invoke `ralph -c <config> -H <preset> preflight
   --strict` and require exit zero. Backend-class failures are
   surfaced as `blocked_backend`; everything else is `blocked_cli`.
4. **dry_run** — invoke `ralph -c <config> -H <preset> run --dry-run
   --strict` and verify the output references the requested config
   path. A passing dry-run is a **static-load pass**, not a loop
   closure.

The proof level advances monotonically: a blocker at stage N skips
every stage N+1..4 with `outcome="blocked_unknown"` and
`next_allowed_stage=None`. A green stage N sets
`next_allowed_stage=<N+1>`; a green stage 4 returns
`next_allowed_stage=None` (there is no fifth gate).

## Blockers

The gate classifies each non-green stage into one blocker category so
callers can route them correctly:

| Category            | Stages that can produce it             | Meaning |
| ------------------- | -------------------------------------- | ------- |
| `blocked_cli`       | capability, preset_check, preflight, dry_run | Binary missing, required flag missing, unknown command, generic CLI failure |
| `blocked_preset`    | preset_check                           | Strict preset lint rejected the suite's preset |
| `blocked_backend`   | preflight, dry_run                     | Backend executable missing, unknown backend, auth not ready |
| `blocked_command`   | dry_run                                | Dry-run succeeded but the output did not reference the requested config path (source mismatch) |
| `blocked_unknown`   | any                                    | Timeout, OSError, or skipped-stage marker |

## The no-`--skip-preflight` rule

The skill must never invoke `ralph run` with `--skip-preflight`. The
dry-run gate runs as its own distinct `ralph preflight --strict`
invocation so the preflight check is observable, structured, and
fail-closed. A successful dry-run is meaningless without an explicit
strict preflight gate in front of it.

## Static load ≠ loop closed

A green `dry_run` proves that the runtime:

* parsed the config file at the requested path,
* resolved the preset id against the registry,
* loaded the prompt file at the requested path,
* resolved the plan path against the file system,
* performed backend auto-detection,
* and completed its auto-preflight step.

It does **not** prove that a loop can reach any business event, that
the prompt produces a coherent response from the configured backend,
or that the budget / iteration caps are reachable. Downstream stages
must surface "static load passed; loop not closed" and never claim
"loop closed".

## Why every argv carries `-c ralph.pipeline.yml -H <preset>`

The runtime auto-discovers `ralph.yml` as a default config and a
default preset. Without explicit `-c` and `-H` flags the binary may
silently substitute a different config / preset than the suite
authored, defeating the whole point of authoring the suite. Every
argv the staged gate builds therefore starts with:

```
<binary> -c <config_path> -H <preset> <stage-command> ...
```

The dry-run stage additionally carries `--dry-run` so the runtime
takes its static-only branch and never spawns the configured
backend.

## Evidence

Each `StageDecision` carries an `evidence` tuple of structured log
lines. Callers that want to render a human-readable report can
format the tuple as-is; callers that want machine-readable evidence
can index into it. The lines record:

* `version=<str>` — binary version line from `ralph --version`.
* `flags_present=[...]` / `flags_missing=[...]` — capability gate
  inventory.
* `exit_code=<int>` — exit code of the stage subprocess.
* `stderr=<str>` — first ~400 chars of stage stderr on failure.

## Acceptance checklist

When extending `cli_probe.py`:

```bash
.venv/bin/python -m pytest skills/tests/test_project_bootstrap_contract.py -k cli_probe
.venv/bin/python -m pytest skills/tests/test_project_bootstrap_contract.py
```

Both must remain green. The new fixture set under
`fixtures/cli/` must continue to drive the staged gate without
spawning the real binary. Any new capability added to
`REQUIRED_FLAGS` must be backed by a corresponding fixture entry
demonstrating both the green path and the missing-flag path.