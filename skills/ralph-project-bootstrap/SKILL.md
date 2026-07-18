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

## Agent Docs Maintenance

`AGENTS.md` and `CLAUDE.md` in the target project are mutated only via
the helpers in `scripts/agent_docs.py`. The helper is a pure stdlib
module that owns every persistent edit through a single managed
section.

**Marker format.** Each doc carries exactly one managed block, fenced
by `<!-- RALPH-BOOTSTRAP-START: <marker_id> v1 -->` /
`<!-- RALPH-BOOTSTRAP-END: <marker_id> -->` lines. The boundary markers
themselves are HTML comments so editors that strip comments will
silently corrupt the section; the writer never inserts anything outside
that block. The prefix `RALPH-BOOTSTRAP-` is reserved and must not be
used elsewhere in the doc. The block ends exactly at the END marker
(no trailing newline emitted by the helper) so any whitespace the
operator wants after the block passes through compose byte-for-byte.

**Distinct from runtime managed blocks.** The runtime loop ledger uses
`RALPH-MANAGED-BLOCK-START` / `END` markers for its own scratch state.
The bootstrap marker prefix is intentionally distinct; the two
namespaces must not overlap. The parser checks the prefix string
literally so a runtime-managed block in the same doc does not
register as a bootstrap section.

**Idempotency.** `compose_agent_docs` is a pure function over its
arguments. Running it twice with the same inputs returns `noop` on the
second call, never rewrites the file, and never emits `updated`. This
guarantee is enforced by `test_compose_is_idempotent_across_two_runs`
in the contract suite.

**Dirty-tree protection.** A bootstrap write batch targets only the
caller-supplied paths via `AtomicWriter`. The writer has no business
walking the tree, never opens directories, and never invents new file
names. `src/lib.rs` and any other operator-owned file must be byte-for-
byte identical before and after a batch. The `dirty-tree` fixture is
the canonical regression for this contract.

**Conflict handling.** A doc with zero markers is `Missing` (compose
appends a fresh section). A doc with one START and one END is `Ok`
(compose may replace). Anything else is a hard blocker: `Duplicate`
(multiple START/END), `Truncated` (START without END), or `Nested`
(END before START). The compose call returns
`ComposeResult(kind="blocker", code="marker_...", reason=...)` and the
operator must reconcile by hand. The skill must surface that blocker;
defaulting to a best-guess rewrite is forbidden by the guardrails
section above.

**Sync between AGENTS.md and CLAUDE.md.** When the caller passes
`sync_with_other_doc=True` with `other_body=...`, the helper checks
that the proposed bodies agree before composing either side. Mismatches
return `blocker(sync_mirror_conflict)` so asymmetric pairs are never
written. The `conflicting-docs` fixture is the canonical regression.

**Atomic writes.** `AtomicWriter` stages every target into a sibling
`.{name}.bootstrap.tmp` first, then commits each target with
`os.replace`. If any stage or commit fails, every already-committed
target is restored to its pre-batch bytes (or deleted if it did not
exist) and every planned target is reported in `rolled_back`. No
`.bootstrap.tmp` file is left behind. The `test_atomic_writer_*`
suite is the canonical regression.

**No shell, no `.ralph/`, no chmod.** The writer uses stdlib file IO
only. The helper never opens a shell, never touches the `.ralph/`
directory in the target project, and never changes file permissions.
On any platform where `os.replace` is unavailable the helper must fail
closed before any partial state is observed.