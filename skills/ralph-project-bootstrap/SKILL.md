---
name: ralph-project-bootstrap
description: Audit a target project, generate or safely update its Ralph runtime suite (AGENTS.md / CLAUDE.md / ralph.pipeline.yml / PROMPT.pipeline.md) from an existing preset + plan/task, run staged validation, and deliver a verification-level-tagged handoff with the official launch command. Use this skill whenever an operator asks to bring a brand-new or under-provisioned project onto Ralph, restore a missing pipeline suite, or fix a target whose static checks fail because the suite is incomplete. Do NOT use it to author or review presets — that belongs to `ralph-preset-author` / `ralph-preset-review`. Do NOT use it for day-to-day loop monitoring / resume / merge — that belongs to `ralph-loop`. Do NOT use it for diagnosing a loop that already ran — that belongs to `ralph-run-diagnosis`.
---

# Ralph Project Bootstrap

Bring an arbitrary target project onto an existing Ralph preset with a
verified runtime suite and a clear handoff. This skill does **not** author
or review presets; it consumes presets produced by `ralph-preset-author`
and vetted by `ralph-preset-review`.

## Boundaries

- **Inputs.** Caller supplies the project directory (current cwd), the
  preset path or builtin id (already passing strict lint), and the
  plan/task path that will drive the first run. Skill must stop with a
  precise missing-input report if any of the three is absent or unreadable.
- **No preset authoring.** Hat routing, AAF tables, preset schemas and
  builtin completions live in `ralph-preset-author` / `ralph-preset-review`.
  If the caller needs to change the preset, hand off there.
- **No day-to-day loop ops.** Once the suite ships, monitor / resume /
  merge / debug work belongs to `ralph-loop`. Once a loop has run and
  needs post-mortem diagnosis, hand off to `ralph-run-diagnosis`.
- **No silent backend spawning.** A safe loop smoke (if available) is
  always bounded by iteration / idle / wall-clock caps; any target-project
  smoke that touches non-replay backends requires explicit operator
  authorization captured by the handoff.

## Workflow

1. **Audit** the target project. Confirm root, gather verifiable project
   facts (build / test / lint / format entry points), classify the
   technology stack from evidence (not from a fixed list), and stop with
   a blocker if inputs or root are ambiguous.
2. **Generate / safely update** the four owned artifacts
   (`AGENTS.md`, `CLAUDE.md`, `ralph.pipeline.yml`, `PROMPT.pipeline.md`)
   and the `ralph.bootstrap.yml` provenance file. Preserve user content
   outside owned sections / keys; abort on marker / YAML / ownership
   conflict.
3. **Stage validation** in this strict order: strict preset check →
   strict preflight → `ralph run --dry-run`. Capture structured evidence
   for each stage; downgrade reports to "static-only" when the loop has
   not been smoke-verified.
4. **Optional authorized smoke.** Only when the target backend is a
   content-fixed replay harness shipped with this skill. Any other
   backend (mock / custom / real) must be authorized by the operator
   after the side-effect surface is shown. Without authorization the
   skill reports `incomplete` and stops.
5. **Handoff.** Emit a verification-level-tagged report plus the official
   launch command. Worktree invocations must include an explicit reuse
   key (`--plan <plan>` or `--worktree-name <name>`); missing keys are
   rejected by the handoff.

## Guardrails

- Never overwrite user content outside the owned sections / keys.
- Never touch `presets/` or `crates/ralph-cli/` source.
- Never create, switch, or rename git branches / worktrees on the
  operator's behalf.
- Every verification command must carry explicit `-c ralph.pipeline.yml
  -H <preset>` so `$RALPH_CONFIG` / `ralph.yml` cannot preempt the
  target suite.
- All paths written to disk must be repo-relative.
- Stop and request operator decision on any conflict / ambiguity;
  defaulting to "best guess" is forbidden.