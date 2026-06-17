# Hat Collection Presets

> **The Philosophy**: Each preset embodies a multi-agent architecture pattern optimized for specific workflows. Hats are specialized agents with clear triggers and publications—think of them as team members with defined roles in an event-driven orchestra.

## Architecture Patterns Reference

| Pattern | Description | Best For |
|---------|-------------|----------|
| **Pipeline** | Linear A→B→C flow | Sequential workflows with clear stages |
| **Supervisor-Worker** | Coordinator delegates to specialists | Complex tasks requiring decomposition |
| **Critic-Actor** | One proposes, another critiques | Quality-critical work |
| **Swarm/Handoff** | Autonomous agents hand off based on context | Dynamic, unpredictable workflows |
| **Hierarchical** | Strategic→Tactical→Operational tiers | Large-scale enterprise tasks |
| **Blackboard** | Shared workspace, agents contribute asynchronously | Research & synthesis |

---

## 1. `tdd-red-green.yml` — Test-Driven Development Cycle

**Pattern**: Critic-Actor Pipeline
**Philosophy**: Enforce the red-green-refactor discipline through agent separation.

```yaml
# TDD workflow: Write failing test → Implement → Refactor
# Forces the discipline: you can't skip the red phase

hats:
  test_writer:
    name: "🔴 Test Writer"
    triggers: ["task.start", "refactor.done"]
    publishes: ["test.written"]
    instructions: |
      You write FAILING tests first. This is non-negotiable.

      1. Read the spec/requirement
      2. Write the minimum test that captures the requirement
      3. Verify the test FAILS (red phase)
      4. Publish test.written with the test file path

      NEVER write implementation code. Your job is tests only.

  implementer:
    name: "🟢 Implementer"
    triggers: ["test.written"]
    publishes: ["test.passing"]
    instructions: |
      Make the failing test pass with MINIMAL code.

      1. Read the failing test
      2. Write the simplest code that makes it pass
      3. Run the test to confirm green
      4. Publish test.passing

      Do NOT refactor. Do NOT add extra functionality.
      Just make the test pass.

  refactorer:
    name: "🔵 Refactorer"
    triggers: ["test.passing"]
    publishes: ["refactor.done", "cycle.complete"]
    instructions: |
      Clean up the code while keeping tests green.

      1. Review the implementation for code smells
      2. Refactor for clarity, DRY, and maintainability
      3. Run tests to confirm still passing
      4. If more tests needed: publish refactor.done
      5. If feature complete: publish cycle.complete → LOOP_COMPLETE
```

---

## 2. `adversarial-review.yml` — Red Team / Blue Team

**Pattern**: Adversarial Critic-Actor
**Philosophy**: One agent builds, another actively tries to break it.

```yaml
# Security-conscious development with built-in adversarial review
# The red team agent actively hunts for vulnerabilities

hats:
  builder:
    name: "🔵 Blue Team (Builder)"
    triggers: ["task.start", "fix.applied"]
    publishes: ["build.ready"]
    instructions: |
      Implement the feature with security in mind.

      Consider: input validation, injection attacks, auth/authz,
      data exposure, error handling, dependency vulnerabilities.

      When implementation is ready, publish build.ready.

  red_team:
    name: "🔴 Red Team (Attacker)"
    triggers: ["build.ready"]
    publishes: ["vulnerability.found", "security.approved"]
    instructions: |
      You are a penetration tester. Your job is to BREAK this code.

      Attack vectors to explore:
      - Injection (SQL, command, XSS, template)
      - Authentication/authorization bypass
      - Data exposure and leakage
      - Race conditions and TOCTOU
      - Dependency vulnerabilities
      - Error message information disclosure

      If vulnerabilities found: publish vulnerability.found with details
      If code passes security review: publish security.approved

  fixer:
    name: "🛡️ Security Fixer"
    triggers: ["vulnerability.found"]
    publishes: ["fix.applied"]
    instructions: |
      Remediate the security vulnerability.

      1. Understand the attack vector
      2. Implement the fix with defense in depth
      3. Add regression test for the vulnerability
      4. Publish fix.applied for re-review
```

---

## 3. `socratic-learning.yml` — Teaching Through Questions

**Pattern**: Socratic Dialogue
**Philosophy**: Learn by being questioned, not lectured.

```yaml
# For learning new codebases or concepts
# The questioner forces deep understanding

hats:
  explorer:
    name: "🔍 Explorer"
    triggers: ["task.start", "answer.provided"]
    publishes: ["understanding.claimed"]
    instructions: |
      Explore the codebase/concept and form an understanding.

      1. Read relevant files and documentation
      2. Form a mental model of how things work
      3. Publish understanding.claimed with your explanation

      Be specific. Cite file paths and line numbers.

  questioner:
    name: "❓ Socratic Questioner"
    triggers: ["understanding.claimed"]
    publishes: ["question.asked", "understanding.verified"]
    instructions: |
      Challenge the Explorer's understanding with probing questions.

      Ask questions that:
      - Expose gaps in understanding
      - Challenge assumptions
      - Probe edge cases
      - Connect to deeper principles

      If understanding is solid: publish understanding.verified
      Otherwise: publish question.asked with your challenge

  answerer:
    name: "💡 Answer Synthesizer"
    triggers: ["question.asked"]
    publishes: ["answer.provided"]
    instructions: |
      Research and answer the Socratic question.

      1. Investigate the specific question
      2. Find evidence in the code
      3. Synthesize a clear answer
      4. Publish answer.provided
```

---

## 4. `spec-driven.yml` — Specification-First Development

**Pattern**: Contract-First Pipeline
**Philosophy**: The spec is the contract. Implementation follows.

```yaml
# Forces specification before implementation
# Catches ambiguity before code is written

hats:
  spec_writer:
    name: "📋 Spec Writer"
    triggers: ["task.start", "spec.rejected"]
    publishes: ["spec.ready"]
    instructions: |
      Create a precise, unambiguous specification.

      Include:
      - Given-When-Then acceptance criteria
      - Input/output examples
      - Edge cases and error conditions
      - Non-functional requirements

      Publish spec.ready when complete.

  spec_reviewer:
    name: "🔎 Spec Critic"
    triggers: ["spec.ready"]
    publishes: ["spec.approved", "spec.rejected"]
    instructions: |
      Review the spec for completeness and clarity.

      Check:
      - Is it implementable by someone who hasn't seen the task?
      - Are edge cases covered?
      - Are acceptance criteria testable?
      - Are there ambiguities?

      Reject with specific feedback or approve.

  implementer:
    name: "⚙️ Implementer"
    triggers: ["spec.approved"]
    publishes: ["implementation.done"]
    instructions: |
      Implement EXACTLY what the spec says.

      - Follow the spec literally
      - Satisfy all acceptance criteria
      - Handle all specified edge cases
      - Add tests for each criterion

  verifier:
    name: "✅ Spec Verifier"
    triggers: ["implementation.done"]
    publishes: ["task.complete", "spec.violated"]
    default_publishes: ["task.complete"]
    instructions: |
      Verify implementation matches the spec.

      Go through each acceptance criterion.
      Run the implementation against examples.

      If all pass: LOOP_COMPLETE
      If violations: publish spec.violated with details
```

---

## 5. `mob-programming.yml` — Virtual Mob Session

**Pattern**: Rotating Roles
**Philosophy**: Multiple perspectives on the same code.

```yaml
# Simulates mob programming with rotating driver/navigator roles
# Each agent brings a different perspective

hats:
  navigator:
    name: "🧭 Navigator"
    triggers: ["task.start", "code.written"]
    publishes: ["direction.set", "mob.complete"]
    instructions: |
      You are the navigator. Think strategically.

      1. Understand the high-level goal
      2. Decide the next small step
      3. Give CLEAR, SPECIFIC instructions to the driver
      4. Do NOT write code—describe what to write

      If task complete: publish mob.complete → LOOP_COMPLETE
      Otherwise: publish direction.set with instructions

  driver:
    name: "⌨️ Driver"
    triggers: ["direction.set"]
    publishes: ["code.written"]
    instructions: |
      You are the driver. Execute the navigator's instructions.

      1. Follow the navigator's direction EXACTLY
      2. Write the code they described
      3. If instructions are unclear, implement your best interpretation
      4. Publish code.written when done

      You're the hands, not the brain. Stay tactical.

  observer:
    name: "👁️ Observer"
    triggers: ["code.written"]
    publishes: ["observation.noted"]
    instructions: |
      You are the observer. Provide fresh-eyes feedback.

      Look for:
      - Potential bugs the driver/navigator missed
      - Simpler approaches
      - Missing error handling
      - Code style issues

      Add brief comments, then publish observation.noted.
      The navigator will decide what to act on.
```

---

## 6. `scientific-method.yml` — Hypothesis-Driven Debugging

**Pattern**: Scientific Investigation
**Philosophy**: Debug like a scientist—hypothesize, experiment, conclude.

```yaml
# Systematic debugging through the scientific method
# Prevents random "try this" debugging

hats:
  observer:
    name: "🔬 Observer"
    triggers: ["task.start", "hypothesis.rejected"]
    publishes: ["observation.made"]
    instructions: |
      Gather observations about the bug.

      1. Reproduce the bug
      2. Collect symptoms (error messages, stack traces, logs)
      3. Note what DOES work vs what DOESN'T
      4. Identify patterns

      Publish observation.made with your findings.

  theorist:
    name: "🧠 Theorist"
    triggers: ["observation.made"]
    publishes: ["hypothesis.formed"]
    instructions: |
      Form a testable hypothesis about the root cause.

      Based on observations, propose:
      - A specific cause
      - WHY you believe this is the cause
      - How to TEST this hypothesis

      Be specific and falsifiable.

  experimenter:
    name: "🧪 Experimenter"
    triggers: ["hypothesis.formed"]
    publishes: ["hypothesis.confirmed", "hypothesis.rejected"]
    instructions: |
      Design and run an experiment to test the hypothesis.

      1. Create a minimal test case
      2. Add logging/debugging to verify the hypothesis
      3. Run the experiment
      4. Record results

      If confirmed: publish hypothesis.confirmed
      If rejected: publish hypothesis.rejected (back to observation)

  fixer:
    name: "🔧 Fixer"
    triggers: ["hypothesis.confirmed"]
    publishes: ["fix.applied"]
    instructions: |
      Apply a fix based on the confirmed hypothesis.

      1. Implement the fix
      2. Verify the bug is resolved
      3. Add a regression test
      4. Publish fix.applied → LOOP_COMPLETE
```

---

## 7. `code-archaeology.yml` — Legacy Code Understanding

**Pattern**: Archaeological Dig
**Philosophy**: Understand before you change.

```yaml
# For understanding and safely modifying legacy code
# Maps the territory before making changes

hats:
  surveyor:
    name: "🗺️ Surveyor"
    triggers: ["task.start"]
    publishes: ["map.created"]
    instructions: |
      Create a map of the relevant code.

      Document:
      - Key files and their responsibilities
      - Data flow through the system
      - Dependencies (what calls what)
      - Entry points and exit points

      Create a visual or textual map in ANALYSIS.md

  historian:
    name: "📜 Historian"
    triggers: ["map.created"]
    publishes: ["history.documented"]
    instructions: |
      Research the history of this code.

      Use git history to understand:
      - Why was this code written this way?
      - What problems was it solving?
      - What changes have been made and why?
      - Are there related issues or PRs?

      Document your findings.

  archaeologist:
    name: "⛏️ Archaeologist"
    triggers: ["history.documented"]
    publishes: ["artifacts.catalogued"]
    instructions: |
      Identify patterns, anti-patterns, and gotchas.

      Look for:
      - Hidden assumptions
      - Implicit contracts
      - Technical debt
      - Fragile areas
      - Undocumented behavior

      Catalog these "artifacts" for the modifier.

  modifier:
    name: "🔨 Careful Modifier"
    triggers: ["artifacts.catalogued"]
    publishes: ["change.complete"]
    instructions: |
      Now make the change, informed by the archaeology.

      1. Review the map, history, and artifacts
      2. Identify the safest modification approach
      3. Write tests FIRST for existing behavior
      4. Make the minimal change
      5. Verify nothing broke

      LOOP_COMPLETE when done.
```

---

## 8. `performance-optimization.yml` — Measure-Optimize-Verify

**Pattern**: Data-Driven Optimization
**Philosophy**: No optimization without measurement.

```yaml
# Prevents premature optimization
# Forces measurement before and after changes

hats:
  profiler:
    name: "📊 Profiler"
    triggers: ["task.start", "optimization.applied"]
    publishes: ["baseline.measured", "improvement.verified"]
    instructions: |
      Measure performance with hard data.

      First run (baseline.measured):
      - Profile the code
      - Identify bottlenecks with data
      - Record metrics (time, memory, etc.)

      Subsequent runs (improvement.verified):
      - Re-measure after optimization
      - Compare to baseline
      - If improved: LOOP_COMPLETE
      - If not improved or regressed: report findings

  analyst:
    name: "🔍 Bottleneck Analyst"
    triggers: ["baseline.measured"]
    publishes: ["analysis.complete"]
    instructions: |
      Analyze the profiling data to identify the real bottleneck.

      Remember:
      - 80/20 rule applies—find the 20% causing 80% of slowness
      - Don't guess—use the data
      - Consider algorithmic vs constant factor improvements

      Recommend ONE specific optimization to try.

  optimizer:
    name: "⚡ Optimizer"
    triggers: ["analysis.complete"]
    publishes: ["optimization.applied"]
    instructions: |
      Implement the recommended optimization.

      Rules:
      - ONE optimization at a time
      - Keep original code commented for comparison
      - Don't break functionality for performance
      - Write a benchmark test if none exists
```

---

## 9. `api-design.yml` — Consumer-Driven API Design

**Pattern**: Outside-In Design
**Philosophy**: Design APIs from the consumer's perspective.

```yaml
# Forces API design from usage patterns
# Consumer experience drives the interface

hats:
  consumer:
    name: "👤 API Consumer"
    triggers: ["task.start", "api.refined"]
    publishes: ["usage.examples"]
    instructions: |
      Write code AS IF the API already exists.

      Create realistic usage examples:
      - Happy path usage
      - Error handling
      - Edge cases
      - Integration patterns

      Write the code you WISH you could write.
      Don't worry about implementation—focus on ergonomics.

  designer:
    name: "✏️ API Designer"
    triggers: ["usage.examples"]
    publishes: ["api.designed"]
    instructions: |
      Design the API to support the usage examples.

      Create:
      - Interface definitions
      - Type signatures
      - Error types
      - Documentation

      Prioritize: clarity > cleverness, consistency > convenience

  critic:
    name: "🎯 API Critic"
    triggers: ["api.designed"]
    publishes: ["api.approved", "api.refined"]
    instructions: |
      Review the API design for usability issues.

      Check:
      - Is it intuitive?
      - Are there footguns?
      - Is it consistent with platform conventions?
      - Does it handle errors gracefully?
      - Is it extensible without breaking changes?

      If issues: publish api.refined with feedback
      If solid: publish api.approved

  implementer:
    name: "🔧 Implementer"
    triggers: ["api.approved"]
    publishes: ["task.complete"]
    instructions: |
      Implement the approved API design.

      - Follow the design exactly
      - Write tests from the usage examples
      - Document any implementation constraints

      LOOP_COMPLETE when done.
```

---

## 10. `documentation-first.yml` — README-Driven Development

**Pattern**: Documentation-First
**Philosophy**: If you can't explain it simply, you don't understand it.

```yaml
# Write the docs before the code
# Forces clarity of thought

hats:
  documenter:
    name: "📝 Documenter"
    triggers: ["task.start", "docs.rejected"]
    publishes: ["docs.ready"]
    instructions: |
      Write the documentation BEFORE any code exists.

      Include:
      - What problem does this solve?
      - How do you use it? (with examples)
      - What are the edge cases?
      - What are the limitations?

      Write as if explaining to a new team member.

  reviewer:
    name: "🔎 Docs Reviewer"
    triggers: ["docs.ready"]
    publishes: ["docs.approved", "docs.rejected"]
    instructions: |
      Review docs for completeness and clarity.

      Test: Could someone implement this from the docs alone?

      Check:
      - Are examples runnable?
      - Are edge cases covered?
      - Is the API clear from usage examples?
      - Are limitations documented?

  implementer:
    name: "⚙️ Implementer"
    triggers: ["docs.approved"]
    publishes: ["implementation.done"]
    instructions: |
      Implement to match the documentation.

      The docs are the spec. Follow them exactly.
      Every example in the docs should work.

  verifier:
    name: "✅ Docs Verifier"
    triggers: ["implementation.done"]
    publishes: ["task.complete"]
    instructions: |
      Verify implementation matches documentation.

      Run every example in the docs.
      Check every edge case mentioned.

      LOOP_COMPLETE when all docs are accurate.
```

---

## 11. `incident-response.yml` — Production Incident Handling

**Pattern**: OODA Loop (Observe-Orient-Decide-Act)
**Philosophy**: Fast, structured response to production issues.

```yaml
# Incident response workflow
# Prioritizes mitigation over root cause

hats:
  observer:
    name: "👁️ Observer"
    triggers: ["task.start"]
    publishes: ["situation.assessed"]
    instructions: |
      Rapidly assess the situation.

      Gather:
      - What is the user impact?
      - When did it start?
      - What changed recently?
      - What are the symptoms?

      Timebox: 5 minutes max. Ship imperfect information.

  mitigator:
    name: "🚨 Mitigator"
    triggers: ["situation.assessed"]
    publishes: ["mitigation.applied", "mitigation.failed"]
    instructions: |
      Stop the bleeding. Mitigate NOW.

      Options (fastest first):
      - Rollback recent deploy
      - Feature flag disable
      - Scale up resources
      - Redirect traffic

      Don't fix the root cause—just stop the impact.

  investigator:
    name: "🔍 Root Cause Investigator"
    triggers: ["mitigation.applied"]
    publishes: ["root_cause.found"]
    instructions: |
      NOW investigate the root cause.

      With pressure off:
      - Analyze logs and metrics
      - Review recent changes
      - Form and test hypotheses
      - Document findings

  fixer:
    name: "🔧 Permanent Fixer"
    triggers: ["root_cause.found"]
    publishes: ["incident.resolved"]
    instructions: |
      Implement a permanent fix.

      - Fix the root cause
      - Add monitoring/alerting
      - Write regression test
      - Document in post-mortem

      LOOP_COMPLETE when fix is deployed.
```

---

## 12. `migration-safety.yml` — Safe Database/System Migration

**Pattern**: Expand-Contract Migration
**Philosophy**: Never break production during migrations.

```yaml
# Safe migration pattern for databases, APIs, etc.
# Expand → Migrate → Contract

hats:
  planner:
    name: "📋 Migration Planner"
    triggers: ["task.start"]
    publishes: ["plan.ready"]
    instructions: |
      Plan the expand-contract migration.

      Expand phase: Add new alongside old
      Migrate phase: Move data/traffic
      Contract phase: Remove old

      Plan rollback at each phase.
      Identify verification checkpoints.

  expander:
    name: "📈 Expander"
    triggers: ["plan.ready", "contract.rollback"]
    publishes: ["expand.done"]
    instructions: |
      Implement the expand phase.

      - Add new schema/API alongside old
      - Dual-write to both old and new
      - Keep old system fully functional
      - Verify new system works

      This phase must be safe to rollback instantly.

  migrator:
    name: "🔄 Migrator"
    triggers: ["expand.done"]
    publishes: ["migrate.done", "expand.rollback"]
    instructions: |
      Execute the migration.

      - Migrate data/traffic incrementally
      - Verify at each step
      - Monitor for errors
      - Be ready to rollback

      If issues: publish expand.rollback
      If successful: publish migrate.done

  contractor:
    name: "📉 Contractor"
    triggers: ["migrate.done"]
    publishes: ["migration.complete", "contract.rollback"]
    instructions: |
      Contract phase: Remove the old system.

      - Remove dual-write
      - Drop old schema/API
      - Clean up migration code
      - Verify nothing broke

      If issues: publish contract.rollback
      If successful: LOOP_COMPLETE
```

---

## 13. `confession-loop.yml` - Confidence-Gated Completion

**Pattern**: Critic-Actor with Verification Gate  
**Philosophy**: Separate usefulness from honesty. Do not allow completion until a self-audit reports a high confidence score.

```yaml
event_loop:
  starting_event: "build.task"

hats:
  builder:
    triggers: ["build.task"]
    publishes: ["build.done"]

  confessor:
    triggers: ["build.done"]
    publishes: ["confession.clean", "confession.issues_found"]

  confession_handler:
    triggers: ["confession.clean", "confession.issues_found"]
    publishes: ["build.task", "escalate.human"]
    # Emits LOOP_COMPLETE only when confidence >= 80 and nothing material is found.
```

---

## Quick Reference: When to Use Each Preset

| Preset | Use When |
|--------|----------|
| `tdd-red-green` | Building new features with test coverage |
| `adversarial-review` | Security-critical code |
| `socratic-learning` | Learning a new codebase |
| `spec-driven` | Features with complex requirements |
| `mob-programming` | Need multiple perspectives |
| `scientific-method` | Debugging mysterious bugs |
| `code-archaeology` | Modifying legacy code |
| `performance-optimization` | Performance tuning |
| `api-design` | Designing new APIs |
| `documentation-first` | Features that need clear docs |
| `incident-response` | Production incidents |
| `migration-safety` | Database/system migrations |
| `confession-loop` | Confidence-gated completion via self-assessment |

---

## Creating Your Own Presets

The key constraint: **Each trigger must map to exactly one hat.**

```yaml
# Template
hats:
  <hat_id>:
    name: "<Emoji> <Human Name>"
    triggers: ["<event.pattern>"]    # What activates this hat
    publishes: ["<event.type>"]      # What this hat can emit
    default_publishes: "task.complete"   # Fallback if hat forgets
    instructions: |
      <Clear instructions for this role>

      End with: LOOP_COMPLETE when fully done
```

**Event naming conventions:**
- `task.start` / `task.resume` — Entry points
- `<phase>.ready` / `<phase>.done` — Phase transitions
- `<thing>.approved` / `<thing>.rejected` — Review gates
- `<noun>.found` / `<noun>.missing` — Discovery events

---

## `default_publishes` vs Execution Contracts

### The Core Problem

When a hat forgets to emit an event, `default_publishes` provides a fallback. But not all fallbacks are equal:

| Hat Type | Should Use `default_publishes`? | Should Use Execution Contract? |
|----------|-------------------------------|------------------------------|
| **实施型 (Implementation)** | ❌ NO | ✅ YES |
| **Gate 型** | ✅ YES (fail-closed default) | ❌ NO |
| **Report 型** | ⚠️ Use with caution | ⚠️ Add defensive checks |

### When NOT to use `default_publishes`

**实施型 hats** (e.g., executor, implementer) that publish `work.done` should NOT use `default_publishes: "work.done"`:

```yaml
# ❌ WRONG: executor with success default
hats:
  executor:
    publishes: ["work.done", "work.failed"]
    default_publishes: "work.done"  # DANGEROUS: masks forgetfulness

# ✅ CORRECT: executor without default
hats:
  executor:
    publishes: ["work.done", "work.failed"]
    # No default_publishes - must explicitly emit
```

**Why?** If the executor forgets to emit `work.done`, Ralph injects a fake `work.done`, making it appear the work succeeded when it didn't. The review-coordinator then triggers on fake success.

### When TO use `default_publishes`

**Gate 型 hats** can use fail-closed defaults:

```yaml
# ✅ GOOD: plan-gate blocks if no decision
hats:
  plan-gate:
    publishes: ["queue.advance", "plan.complete", "plan.blocked"]
    default_publishes: "plan.blocked"  # Safe: fail-closed

# ✅ GOOD: fixer exhausts after 3 rounds
hats:
  fixer:
    publishes: ["fix.applied", "fix.exhausted"]
    default_publishes: "fix.exhausted"  # Safe: fail-closed
```

**Report 型 hats** can use report defaults, but should add defensive checks:

```yaml
# ⚠️ OK with caution: reporter defaults to report.done
hats:
  reporter:
    publishes: ["report.done", "LOOP_COMPLETE"]
    default_publishes: "report.done"  # Caution: ensure completion checks exist
```

### Using Execution Contracts for Implementation Hats

For实施型 hats, use execution contracts to validate completion:

```yaml
event_loop:
  execution_contracts:
    enabled: true
    rules:
      work.done:
        require_payload_fields: ["task_id", "task_key", "step"]
        require_task:
          id_field: "task_id"
          loop_scoped: true
          allowed_terminal_statuses: ["closed"]
        require_git_change:
          mode: "diff_or_commit"
          allow_empty_for_steps: ["trivial"]
```

This ensures:
1. **Payload validation**: Required fields present
2. **Task validation**: Task is closed
3. **Git validation**: Real changes exist (unless trivial)

### Checklist for Preset Authors

- [ ] Does this hat's success default mask forgetfulness?
- [ ] Is the completion topic entering downstream without real validation?
- [ ] Can the completion state be reconstructed from task/git/test evidence?
- [ ] Should execution contracts be enabled for this workflow?

---

## Auto-Fix to Debug-Resolver Escalation Guidance

When a preset includes an automatic fix loop:

- The `fixer` hat is a fast-patcher for `safe_auto` findings only. It is not a debugger.
- Cap `safe_auto` attempts at a small number (e.g., 3 rounds). After that, emit `fix.exhausted`.
- Do not route `fix.exhausted` straight to a final failure report. Instead, hand it off to a `debug-resolver` hat that:
  - performs root-cause diagnosis (investigation, assumption audit, causal chain gate, smart escalation),
  - emits `fix.plan.ready` when the root cause is confirmed,
  - emits `debug.exhausted` or `plan.blocked` when the cause cannot be confirmed.
- The `executor` should have a dedicated `FIX PLAN EXECUTION MODE` triggered by `fix.plan.ready`, execute the plan, then re-enter the normal review loop via `work.done`.
- Port diagnostic discipline from skills like `ce-debug`, but do **not** port their interactive menus, branch creation, or PR workflows into a preset.

---

## Payload Contracts

Payload contracts verify that events flowing between hats carry the
fields the downstream hat actually reads. They run at preset-load
time (as part of `ralph hats validate` and the `ralph run` startup
hard gate) and at runtime (when `event_policy.mode: enforce`).

Payload contracts are complementary to execution contracts: execution
contracts verify the *completion declaration* (e.g. `work.done` has
`task_id` and a closed runtime task); payload contracts verify the
*event shape* itself.

### Schema Sources

Schemas are declared inline in the preset yml under
`event_policy.schemas`. The previous `event_policy.schema_file` form
(relative to a sibling file under `presets/schemas/`) was broken for
builtin presets — `resolve_schema_files` has no on-disk preset file
to anchor the relative path against, so the schemas silently went
unloaded and `ralph hats validate` / `ralph run` reported
`SchemaMissingForRequiredTopic` for every topic the preset declared.
As of 2026-06-03 the builtin presets inline the schemas directly:

```yaml
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    schemas:
      work.ready:
        required_fields: [plan_name, plan_path, task_id, task_key, step, complexity]
        payload: json_object
      # ...
```

The `presets/schemas/` directory is kept as a deprecated reference
copy of the inlined schemas. For file-based hat collections (i.e.
`ralph run -H .ralph/hats/my.yml`) the relative `schema_file` form
still works, but the inline form is preferred for consistency.

### Builtin Schema Library

| Schema file | Referenced by | Status |
|---|---|---|
| `schemas/ce-executor-isolated.yml` | `ce-executor-isolated` | Deprecated reference copy (schemas inlined into `presets/en/ce-executor-isolated.yml`) |
| `schemas/ce-executor-serial.yml`   | `ce-executor-serial`   | Deprecated reference copy (schemas inlined into `presets/en/ce-executor-serial.yml`) |
| `schemas/ce-executor-wave.yml`   | `ce-executor-wave`    | Deprecated reference copy (schemas inlined into `presets/en/ce-executor-wave.yml`) |

### Task Split Heuristics (ce-executor-isolated)

**Task Split Heuristics**: `ce-executor-isolated` coordinator 默认每 U 拆 1 张 task；
只有在 plan 显式列子单元、需要 wave 并行、或跨 worktree 时才拆多张。
详见 `presets/en/ce-executor-isolated.yml` 的 coordinator "Task Split Heuristics" 段。

### Checklist for Preset Authors (Payload Contracts)

- [ ] Do any hats' instructions reference payload fields with `From event payload:` or `payload MUST include:`? If yes, declare the field in the topic's schema `required_fields` directly under `event_policy.schemas` in the preset yml.
- [ ] Do NOT reintroduce `event_policy.schema_file` in a builtin preset. It is broken by design for builtins (no on-disk anchor). The inline form is the only correct option.
- [ ] For Chinese (`-zh`) presets: do not declare a separate schema. The Chinese preset is a hat collection overlay, and `event_policy` flows through from the English preset. If `-zh` needs different schemas, you have bigger design problems.
- [ ] Does `ralph hats validate --strict` exit 0 for the preset?
- [ ] Does `./scripts/sync-embedded-files.sh check` pass? (Not applicable if no schema files are mirrored.)

See `docs/guide/payload-contracts.md` for the full schema format,
extractor behaviour, and the boundary with execution contracts.

## Runtime Contract Quality Gate

`ralph preset check -H builtin:<name> --strict` is the recommended
authoring entry point and supersedes `ralph hats validate` for new work.
It folds config / topology / orphan / payload into a single
`RuntimeContractReport` and supports a stable JSON output for CI.

To validate every public builtin preset in one pass:

```bash
./scripts/validate-builtin-presets.sh --strict
```

The script reads its preset list from `presets/index.json` (single source
of truth for user-facing presets) and exempts `autoresearch` and `debug`
in non-strict mode for their known branching-topology design. In strict
mode no exemption is applied — every warning or non-topology error
fails the gate. See `docs/guide/runtime-contracts.md` for the strict
semantics matrix and `scripts/validate-builtin-presets.sh` for the
regression matrix source.

### Checklist for Preset Authors (Runtime Contract)

- [ ] Does `ralph preset check -H builtin:<name> --strict` exit 0?
- [ ] Does `./scripts/validate-builtin-presets.sh --strict` pass after the change?
- [ ] If a new finding id appears, is it documented in `docs/guide/runtime-contracts.md`?
- [ ] For new public builtin presets, is the name added to `presets/index.json`?
- [ ] For development presets (`ce-executor-isolated`, `ce-executor-serial`, `ce-executor-wave`), does `ralph preset check --strict --format json` show zero findings?
