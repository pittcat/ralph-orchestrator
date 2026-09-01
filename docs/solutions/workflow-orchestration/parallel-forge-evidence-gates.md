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
