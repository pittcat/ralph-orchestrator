# Handoff Reference

This document is the implementation reference for the **official launch
command builder + handoff report** that ``ralph-project-bootstrap``
emits after Units 1-6 finish. End agents do not see this file; the
skill owner does.

## What the handoff is for

The handoff is the single artifact the operator consumes at the end of
the bootstrap pipeline. It bundles:

* the **items** the pipeline created / updated / left alone,
* the **validation evidence** the static gate recorded (Unit 5),
* the **smoke evidence** the bounded smoke harness recorded (Unit 6),
* the **residual risks** the operator must re-confirm by hand,
* and the **official launch command** the operator runs from the
  target-project root.

The handoff is rendered by ``scripts/handoff.py`` and is a pure
stdlib module — no subprocess calls, no env-var reads, no
filesystem access. The caller feeds it a ``HandoffInputs`` record and
gets back a ``HandoffArtifact`` (structured data plus a Markdown
report).

## Three handoff levels

The handoff must declare exactly one of three levels. The level is
set by the bootstrap pipeline based on the upstream unit outcomes;
``build_handoff`` itself does not infer it from any other source.

### `complete`

* **Conditions:** Units 1-5 all green AND a SafeBackend smoke
  reached ``bounded_terminal_reached`` (authorized smoke passed).
* **Command shape:** the official launch command, no prefix. The
  operator may paste it into a terminal verbatim.
* **Report language:** the report states that static load passed
  AND the loop reached the bounded terminal marker.

### `incomplete_static_only`

* **Conditions:** Units 1-5 all green, Unit 6 smoke either not
  authorised or not run.
* **Command shape:** the command is prefixed
  ``[CANDIDATE - operator must run manually]`` and the report
  explicitly says **smoke-not-authorized**.
* **Contract:** the operator MUST NOT receive a "ready to run"
  message. Static load has passed; the loop has not been closed.
  The operator must run the candidate command explicitly after
  re-confirming the target backend.
* **Missing first-run input:** provisioning may still be complete. When a
  required plan is not available, render `[TEMPLATE - replace PLAN_PATH before
  running]` and append `--plan PLAN_PATH`; do not downgrade the provisioning
  result to `blocked` and do not suppress the files already created.

### `blocked`

* **Conditions:** any earlier unit returned a blocker.
* **Command shape:** empty string. The report explains why.
* **Contract:** the operator receives a precise failure summary
  and stops.

## The worktree reuse-key rule

When ``use_worktree=True`` the handoff MUST include BOTH
``--reuse-worktree`` AND one of:

* ``--plan <plan_arg>`` — the plan the operator wants Ralph to
  reuse, OR
* ``--worktree-name <name>`` — the precise worktree name Ralph
  should reuse.

Missing the reuse key is a hard reject. ``build_handoff`` raises
``ValueError("worktree reuse key required")`` when ``use_worktree``
is True but neither key is supplied. Worktree mode is opt-in. Outside
worktree mode the helper appends exactly one optional prompt source:
``--plan`` when present, otherwise ``--prompt-file``. Preset-native mode
appends neither.

When ``use_worktree`` is True AND a reuse key is supplied the argv
shape is exactly:

```
<binary> -c <config_path> -H <preset>
         [--prompt-file <prompt_file> | --plan <plan>]
         --worktree --reuse-worktree
         (--plan <plan_arg> | --worktree-name <worktree_name>)
```

`--prompt-file` and `--plan` are mutually exclusive prompt sources. When the
operator supplies an external `prompt_file`, keep `--prompt-file` and omit the
top-level `--plan`; the external prompt is authoritative and may direct the
agent to read the plan. When no external prompt is supplied, use `--plan` as
the prompt source. A worktree `plan_arg` remains an independent reuse key.

Note that the explicit reuse ``--plan`` (when supplied) replaces
the top-level ``--plan <plan_path>`` so the operator's explicit
reuse key wins. The reuse key must always be repo-relative; absolute
paths are rejected.

## Why "complete" requires U6 green, not just U5 green

The static gate (Unit 5) proves that the runtime can statically
load the suite: it can parse the config, resolve the preset id,
resolve the selected prompt/plan paths when present, perform backend
auto-detection, and complete its auto-preflight step. None of that
proves a loop can reach any business event or that the configured
backend can produce a coherent response. The smoke (Unit 6) is the
only path that exercises the loop end-to-end under the triple cap.

A ``complete`` handoff therefore demands the smoke reached
``bounded_terminal_reached``. Anything less — even a green static
gate — is at best ``incomplete_static_only``. Anything worse — a
bucket failure, a timeout, an error event — is ``blocked``.

## Report structure

The rendered Markdown report always contains, in this order:

1. H1 title `# Ralph Bootstrap Handoff`.
2. `Level: <level>` line.
3. Optional `## Blocker` section when level is `blocked`.
4. `## Items` sub-table (created / updated / noop).
5. `## Validation` sub-section.
6. `## Smoke` sub-section with status token
   (`complete` / `static-only -- smoke-not-authorized` /
   `blocked -- <bucket>`).
7. `## Residual Risks` sub-section (or `_none_` when empty).
8. `## Launch Command` sub-section (copyable code block) — empty
   when level is `blocked`.

All paths in the report are repo-relative. The helper rejects
absolute paths at the API boundary with ``ValueError``. The
helper itself is English-only; downstream localisation, if any,
happens outside this module.

## Acceptance checklist

When extending ``handoff.py``:

```bash
.venv/bin/python -m pytest skills/tests/test_project_bootstrap_contract.py -k handoff
.venv/bin/python -m pytest skills/tests/test_project_bootstrap_contract.py
```

Both must remain green. Every new handoff field must be backed by a
test asserting both the structured dataclass and the rendered
Markdown. The module must never import a third-party package; the
test suite would refuse to load it.
