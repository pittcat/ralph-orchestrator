---
name: ralph-project-bootstrap
description: >-
  Read an existing Ralph preset, audit a target project, generate a useful
  project-specific runtime suite, run staged validation, and deliver the exact
  launch command plus unresolved preconditions. Use when bringing a project
  onto a builtin or file preset or repairing an incomplete suite. A plan/task
  is optional. Do not use for preset authoring/review, loop operations, or
  post-run diagnosis.
---

# Ralph Project Bootstrap

Bring an arbitrary target project onto an existing Ralph preset with a
verified runtime suite and a clear handoff. This skill does **not** author
or review presets; it consumes presets produced by `ralph-preset-author`
and vetted by `ralph-preset-review`.

## Boundaries

- **Inputs.** Caller supplies the project directory (current cwd) and the
  preset path or builtin id. Read the resolved preset in full before deciding
  what else is needed. A plan, ordinary prompt file, environment variable,
  loop id, worktree name, or other runtime context is required only when the
  preset's actual launch contract needs it. Missing first-run business input
  does **not** block provisioning: generate the reusable suite, mark the
  handoff incomplete, and show the exact command template the operator must
  finish. Never invent a placeholder plan file.
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

1. **Read the preset, then audit.** Resolve and read the supplied preset in
   full. Confirm the target root, then inspect the nearest `AGENTS.md` /
   `CLAUDE.md`, dependency manifests, task runners, CI workflows, and test /
   lint configuration. Gather verifiable build / test / lint / format entry
   points and derive a launch contract from the preset plus
   operator input. Record optional `prompt_file`, optional `plan_path`,
   runtime environment/argument preconditions, worktree strategy, and which
   files (if any) need bootstrap ownership. A hats source cannot carry
   `event_loop.prompt` across Ralph's operator/preset merge boundary, so an
   inline preset prompt is generation input, not a runtime prompt source.
   Distinguish a **provisioning blocker** (invalid preset,
   ambiguous root, ownership conflict) from a **first-run input gap** (missing
   plan, loop id, document brief): only the former stops writes. Do not
   classify by preset name.
   For a file preset, read the repo-relative YAML directly. For a builtin,
   first validate the hats source with `ralph -H builtin:<id> preset check
   --strict`. `preset show` addresses **template names**, which may differ
   from the builtin hats id. Run `ralph preset list --format json`, find the
   manifest whose `source` equals `builtin:<id>`, then run `ralph preset show
   <template-name> --format yaml`. If no template maps to that source, resolve
   the builtin from the installed Ralph distribution; never assume that
   stripping `builtin:` yields a template name. A manifest description alone
   is not sufficient evidence.
2. **Generate / safely update the preset-bound suite.** Derive the preset
   stem with `derive_preset_bound_paths`, then call
   `compose_preset_bound_suite` with the full resolved preset YAML. For
   `<stem>`, own exactly `ralph.<stem>.yml` and `PROMPT.<stem>.md`. Copy the
   preset's literal `event_loop.prompt` into the prompt artifact and point the
   generated config at it. Never emit generic `ralph.pipeline.yml`,
   `PROMPT.md`, `PROMPT.pipeline.md`, or a separate `ralph.bootstrap.yml` for
   this flow. If the preset has no non-empty inline prompt and the operator
   supplied neither a plan nor an external prompt, stop with
   `preset_prompt_missing`. Preserve
   user content outside owned sections / keys; abort on marker / YAML /
   ownership conflict. Use `assets/ralph.pipeline.base.yml` as the structural
   baseline rather than emitting a skeletal config. Adapt that baseline to
   the target project: populate `core.guardrails` with concise rules supported
   by project evidence and include the discovered verification commands. Do
   not copy source-project language, commands, paths, or assumptions into the
   target. A newly generated config containing only `core.project_root` is an
   incomplete bootstrap result.

   Reconcile an existing pair with `reconcile_preset_bound_suite`. Embedded
   `profile_sha256` and `prompt_sha256` must match before refresh; a mismatch
   is an ownership blocker. Provenance lives under the config's `_bootstrap:`
   mapping, never in a sidecar.
   For a plan-driven preset with no plan yet, generate the preset-bound config,
   agent-doc managed sections when needed, and a managed fallback prompt. The fallback
   must state that a real repo-relative plan is supplied with `--plan`, and
   must stop without changing the project if executed directly. Do not create
   a fake `plan.md`.
   The final plan-driven launch template must pass only `--plan PLAN_PATH` as
   its CLI prompt source; do **not** also pass `--prompt-file` because CLI
   prompt-file precedence would select the fallback instead of the real plan.
   The generated config may still reference the fallback so standalone
   preflight/config loading has a safe, readable file.
3. **Stage validation** in this strict order: strict preset check →
   strict preflight → `ralph run --dry-run`. Capture structured evidence
   for each stage; downgrade reports to "static-only" when the loop has
   not been smoke-verified. A missing first-run value may prevent validation
   of the final command, but it does not undo generated artifacts.
4. **Optional authorized smoke.** Only when the target backend is a
   content-fixed replay harness shipped with this skill. Any other
   backend (mock / custom / real) must be authorized by the operator
   after the side-effect surface is shown. Without authorization the
   skill reports `incomplete` and stops.
5. **Handoff.** Always emit a verification-level-tagged report plus the
   command matching the launch contract. Include required environment
   variables and dynamic arguments. Missing dynamic values produce an
   `incomplete_static_only` command template such as `--plan PLAN_PATH`, not
   a `blocked` handoff and not a ready command. Worktree invocations must
   include an explicit reuse key (`--plan <plan>` or
   `--worktree-name <name>`); missing keys are rejected.

## Guardrails

- Never overwrite user content outside the owned sections / keys.
- Never touch `presets/` or `crates/ralph-cli/` source.
- Never create, switch, or rename git branches / worktrees on the
  operator's behalf.
- Every verification command must carry explicit `-c ralph.<stem>.yml
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

## Pipeline Suite Authoring

The pipeline suite (`ralph.<stem>.yml`, `PROMPT.<stem>.md`) is generated and
safely maintained by `scripts/pipeline_suite.py`. The helper owns exactly two files
inside the target project and never touches `AGENTS.md` /
`CLAUDE.md` (those flow through `agent_docs.py`) or the runtime
ledger under `.ralph/`.

**Baseline plus project overlay.** Read `assets/ralph.pipeline.base.yml`
before composing a new config. The asset defines reusable runtime safety and
diagnosis defaults. Pass the audited `ProjectFacts` directly as
`project_facts=decision.facts` to `compose_preset_bound_suite`; do not
manually reconstruct or omit this link.
Only emit commands proven by manifests, project docs, task
runners, or CI. For an unknown stack, retain the generic baseline and tell the
agent to discover the authoritative gate instead of inventing one. Builtin
presets and file presets use the same project-overlay path; builtin status is
only a preset-resolution detail.

**Owned keys.** `ralph.<stem>.yml` carries launch inputs plus embedded
`generator_version`, `input_signature`, `profile_sha256`, and
`prompt_sha256` under `_bootstrap:`. The hashes prove both generated files
still match before an idempotent refresh. There is no provenance sidecar.

**Config precedence.** The runtime auto-discovers `ralph.yml` as a
default config. To prevent it from preempting the suite, every
verification command the helper emits MUST carry
`-c ralph.<stem>.yml -H <preset>`. The helper never writes
`ralph.yml`, never references it in commands, and the
`config-precedence` fixture is the canonical regression for this
contract.

**Prompt ownership.** The preset-bound flow snapshots the exact resolved
`event_loop.prompt` bytes into `PROMPT.<stem>.md`. A later preset change
changes `input_signature`; refresh is allowed only when both existing files
still match their embedded hashes. An operator-owned external prompt is a
different launch contract and is referenced rather than copied.

**Forbidden in emitted bytes.** The prompt must never reference
`ralph-hats` or any specific preset name; it must never mention the
runtime managed-block markers (`RALPH-MANAGED-BLOCK`,
`RALPH-BOOTSTRAP-START`) or any internal ledger path
(`.ralph/events.jsonl`, `.ralph/supervisor.db`). Every persisted
scalar must be repo-relative: absolute paths are rejected by the
helper with `OwnedYamlError("owned_yaml_invalid")` at every render
entry point.

**Atomic writes.** The pipeline suite is persisted via
`AtomicWriter` from `agent_docs.py`. The writer is the only place
that touches the filesystem; `pipeline_suite.py` itself is pure and
returns `ApplyResult(kind, text, code, reason)` records that the
writer then commits. See `references/suite-authoring.md` for the
full contract.

## Static Validation

After the suite ships, the skill runs a four-stage static gate in
strict order: capability → preset check → preflight → dry-run.
The gate is implemented by `scripts/cli_probe.py`; the fake runner
that drives it deterministically in the test suite lives in
`scripts/_probe_runner.py`. The fixture corpus under
`fixtures/cli/` records every argv the staged gate emits so tests
exercise the exact contract without spawning the real binary.

**Stage ordering.** Stages run in this strict order and never
overlap. A blocker at stage N skips every stage N+1..4 with
`outcome="blocked_unknown"` and `next_allowed_stage=None`. The
proof level monotonically advances: `capability` → `preset_check`
→ `preflight` → `dry_run` → terminal. A green dry-run is the
**highest** proof level the static gate offers.

**Argv shape.** Every argv the helper builds starts with
`<binary> -c ralph.<stem>.yml -H <preset>` so the runtime cannot
silently substitute `ralph.yml` or the default preset. The dry-run
argv additionally carries `--dry-run` so the runtime takes its
static-only branch and never spawns the configured backend. The
skill NEVER adds `--skip-preflight` to the dry-run argv: strict
preflight runs as its own stage so its failure is observable and
fail-closed.

**Static load is not loop closed.** A green dry-run proves that
the runtime successfully parsed the config, resolved the preset
id, resolved the selected prompt source when one exists, performed
backend auto-detection, and completed its auto-preflight. It does
NOT prove that a loop can reach any business event or that the
configured backend can produce a coherent response. Downstream
reports must surface "static load passed; loop not closed" and
never claim "loop closed" based on a green dry-run alone.

**Blocker classification.** Each non-green stage is classified so
callers can route the failure correctly: `blocked_cli` for
binary-missing or unknown-command, `blocked_preset` for strict
preset lint failure, `blocked_backend` for missing executable /
unknown backend / auth-not-ready, `blocked_command` for
dry-run source mismatch, `blocked_unknown` for timeout or skipped
stages. See `references/validation.md` for the full contract.

**Runner injection.** `validate_pipeline(..., runner=...)` accepts
a callable compatible with `subprocess.run`'s signature so tests
can drive the gate with a fake. The default runner is
`subprocess.run`; production callers pass nothing and inherit it.
The helper module never spawns a real binary at import time.
