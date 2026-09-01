---
module: workflow-orchestration
tags: [parallel-forge, evidence-gates, precheck, payload-consistency]
problem_type: reliability
---

# Parallel Forge evidence gates

## Problem

Critical `parallel-forge` handoffs used to rely on agent instructions alone.
That allowed incomplete wave evidence, false-success receipts, and non-accepted
audits to reach state projection or the finalizer.

## Durable solution

Use two independent guards for irreversible or high-fan-in handoffs:

1. `payload_consistency` rejects deterministic structural contradictions on the
   proposed topic before an LLM activation.
2. `event_loop.precheck` verifies files, identities, live task IDs, and
   cross-artifact relationships, then forwards the payload verbatim or emits a
   rejection/resume signal.

The guarded topics are `forge.worktrees.ready`,
`forge.wave.worktrees.ready`, `forge.wave.reviewed`, `forge.wave.settled`,
`work.failed`, and `forge.audit.done`. Receipt topics use consistency-only
rules so high-frequency slot completion remains ungated.

## Design constraints

- Producer YAML keeps publishing the accepted topic name; desugaring owns the
  `.proposed` rewrite.
- Consistency rules for dual-guard topics target the `.proposed` name.
- Only accepted `forge.wave.settled` reaches task-batch projection.
- Only `forge-failure-handler` publishes `work.failed`; tester failures use
  `forge.full.verification.failed`.
- Only an accepted audit with stable target identity and an ancestor integration
  head may activate finalization.
- Exhausted precheck retries converge to `forge.plan.blocked` with
  `kind=precheck_exhausted`.

## Verification

Keep both structural preset tests and real EventLoop scenarios. Structural
tests protect the builtin contract; scenarios must cover rejection, resume,
accepted forwarding, and exhaustion. Run targeted checks with `cargo nextest`
and finish with `./scripts/run-tests.sh` before declaring the plan complete.

## U11 (2026-08-27-1430) — verifier ingress parity + dynamic CLI

`crates/ralph-core/src/preset_verify.rs::run_scenario` previously parsed
scripted output and wrote events directly to JSONL, bypassing
`config::resolve_precheck_emit_topic` (the same function CLI `ralph emit`
applies when the producer is currently in a precheck-guarded hat). The gap
created three verifier/CLI drifts:

- bare guarded topic emit by the producer was never rewritten to `.proposed`,
  so a scenario could pass with a bare topic that the live runtime would have
  rejected at the `event_origin` boundary;
- the JSONL entry did not stamp `hat`, so the bus-routing trace lacked the
  provenance that real `ralph emit` carries;
- hard-coded fixture SHAs / branch names meant `forge.worktrees.ready`'s
  `target_*` payload was never tied to the verifier workspace's true Git
  state.

The fix is bounded to the verifier ingress adapter. Production gate semantics
(`payload_consistency`, `event_loop.precheck`) are unchanged:

- `normalized_config` is captured before `execution_contract::compile()`
  consumes the config, so the resolver can be called per-iteration with the
  post-desugar hat map;
- the shared `resolve_precheck_emit_topic(config, Some(hat_id), topic)` is
  applied to every parsed event before the JSONL writer; explicit `.proposed`
  is idempotent (Rule 3), so a hand-authored scenario is unaffected;
- the JSONL writer stamps `hat` when the source is a real registered business
  hat. The `ralph` pseudo-hat is the runtime's "no active hat" sentinel —
  when `next_hat()` returns it during a coordinator-mode fixture walkthrough,
  writing `hat="ralph"` would falsely label the event as having come from a
  control-only emitter and trigger `event.isolation.boundary_violation`
  (R6/U2). The writer now drops the field in that case, preserving the
  previous behavior for `preset_verify_supports_coordinator_mode` and
  matching what `ralph emit` itself emits when no specific hat owns the
  event;
- three Git tokens (`{{git.branch}}`, `{{git.head_sha}}`, `{{git.status_fingerprint}}`)
  are resolved against the verifier's tempdir workspace using the same
  canonical status porcelain + SHA-256 algorithm the producer applies
  (D7/D8). Unknown or unresolved tokens fail closed with `FailureKind::InputError`
  and never reach the runtime.

### Bounded acceptance (U11 §7)

- `S22a success`: bare `forge.worktrees.ready` → accepted `.proposed`; no
  rejection; no blocked plan; no typed failure. The full proposed/accepted
  bare pair trace is owned by U5–U9's `run_workflow_guard_scenario` BDD
  scenarios (the verifier driver's single-pass `next_hat()` cannot activate
  the synthesized `precheck-<X>` gate hat, whose bus subscription is
  established after `compile()`).
- `S22b recovery`: same rewrite proof; no `.rejected` in the verifier trace;
  dispatcher does not preempt. Full `proposed/rejected/proposed/accepted`
  chain lives in U4/U5 BDD gate-runtime scenarios.
- `S22c blocked`: `forge.plan.blocked → forge.cleanup.done → forge.report.done
  → LOOP_COMPLETE` (status=BLOCKED); verifier passes.
- `S22d no-output`: `passed=false`, `failure_kind=no_progress`, zero accepted
  events, no `LOOP_COMPLETE`.
- `S22e provenance`: bare guarded topic must be accepted as `.proposed`,
  proving the shared rewrite ran.
- `S22f token safety`: unknown token, Git command failure, and residual
  `{{...}}` all fail closed; error report does not leak tempdir absolute path
  into `trace_digest`.

### Downstream audit (U11 §9)

| downstream | outcome | reason |
| --- | --- | --- |
| `event_loop` step-close / `inject_completion_correction` | no-op | terminal semantics unchanged |
| `preset_lint/*` | no-op (existing tests pass) | gate structural contract stays in preset/schema |
| BDD `scenarios/*.yml` | U5–U9 already covered | U11 changes only the verifier ingress |
| `loop_config.rs` / `PRESET_OPT_IN*` | no-op (E14) | no new opt-in fields |
| `crates/ralph-cli/src/presets.rs` PRESETS names | no-op | no rename |
| `presets/manifest.yml` / `index.json` | no-op | no rename |
| `CLAUDE.md` / `AGENTS.md` | already in sync | U11 did not touch preset description |
| `.cursor/rules` / zsh plugin | no-op | no preset name change |
| `crates/ralph-core/data/*.md` | already in sync | U11 verifier-only change |
| author/review skill | already in sync | U11 verifier-only change |
