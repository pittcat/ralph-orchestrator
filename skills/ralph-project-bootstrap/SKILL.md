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

## Unified Entry Point

The whole pipeline has ONE entry point. Do not call the helper modules
by hand to re-implement the flow; the entry point orchestrates them in
the strict stage order and refuses to skip a stage.

**CLI (operator path):**

```bash
python skills/ralph-project-bootstrap/scripts/bootstrap_pipeline.py \
  --cwd <project> --preset <preset> \
  [--plan <plan.md>] [--prompt-file <prompt.md>] \
  [--binary <ralph>] [--refresh-existing] \
  [--replay-transcript <transcript-dir>] [--json]
```

**Programmatic path:** `bootstrap_pipeline.run_pipeline(cwd=..., preset=..., ...)`
(same module, same contract; tests inject a fake `runner` so no real
binary spawns).

The entry prints one structured `PipelineResult` — either the default
text view or `--json` — and exits `0` for `complete` /
`incomplete_static_only`, `2` for `blocked`.

### Inputs

| Input | CLI flag | Meaning |
| --- | --- | --- |
| project root | `--cwd` | Target project directory (default `.`) |
| preset | `--preset` | Repo-relative preset YAML path or `builtin:<id>` |
| plan | `--plan` | Repo-relative plan path (optional) |
| prompt file | `--prompt-file` | Repo-relative operator-owned prompt (optional) |
| binary | `--binary` | `ralph` binary path/name (default `ralph`) |
| refresh | `--refresh-existing` | Overwrite an existing suite when provenance matches |
| replay smoke | `--replay-transcript` | The ONLY switch that enables an auto-running smoke backend (`content_fixed_replay`) |

A plan / prompt file is required only when the preset has no non-empty
inline `event_loop.prompt`. Missing first-run business input does
**not** block provisioning (see `incomplete_static_only` below). Never
invent a placeholder plan file.

### Artifacts

For `<stem>` derived from the preset id, the entry owns exactly:

- `ralph.<stem>.yml` — preset-bound config; every verification/launch
  command carries `-c ralph.<stem>.yml -H <preset>`; provenance lives
  under the `_bootstrap:` mapping (no sidecar).
- `PROMPT.<stem>.md` — byte snapshot of the preset's literal
  `event_loop.prompt` (a hats source cannot carry
  `event_loop.prompt` across Ralph's operator/preset merge boundary).
- Managed sections inside `AGENTS.md` / `CLAUDE.md` — exactly one
  `RALPH-BOOTSTRAP-START` / `RALPH-BOOTSTRAP-END` block per doc,
  identical body in both mirrors.

Never emit generic `ralph.pipeline.yml`, `PROMPT.md`,
`PROMPT.pipeline.md`, or a separate `ralph.bootstrap.yml` for this
flow. All four targets are written as ONE atomic batch: any conflict
rolls every target back to its pre-write state.

### Stage order (strict)

1. **audit** — confirm the target root and inputs; ambiguous roots and
   unsafe paths block before any write or subprocess call.
2. **preset resolution** — file presets are read as repo-relative YAML;
   `builtin:<id>` is resolved via `ralph preset list --format json`
   (find the manifest whose `source` equals `builtin:<id>`) then
   `ralph preset show <template-name> --format yaml`. `preset show`
   addresses **template names**, which may differ from the builtin hats
   id; never assume stripping `builtin:` yields a template name. A
   preset with no inline prompt and no supplied plan/prompt blocks with
   `preset_prompt_missing`.
3. **generation + post-write verify** — compose the two suite files and
   the managed doc sections, write them atomically, then reopen every
   written artifact and re-verify binding + provenance hashes.
4. **static validation** — capability → `preset check --strict` →
   `preflight --strict` → `ralph run --dry-run`, every argv carrying
   `-c ralph.<stem>.yml -H <preset>`. A blocker skips the tail stages
   with typed evidence (see Static Validation below).
5. **smoke (authorized replay only)** — only a `content_fixed_replay`
   backend supplied via `--replay-transcript` auto-spawns, bounded by
   iteration / idle / wall-clock caps. Any other backend is refused
   (`not_authorized`) and never spawns.
6. **handoff** — typed level + command + Markdown report, derived from
   the typed smoke outcome alone; free-text evidence can never promote
   the level.

### Verification levels and failure actions

**`dry-run green != loop closed`.** A green static gate proves only
that the runtime can load the suite; it never authorizes the claim
that a loop can run to completion.

| Level | Meaning | Exit | Command shape | Operator action |
| --- | --- | --- | --- | --- |
| `blocked` | A stage returned a typed blocker | 2 | empty | Fix exactly what `code` names, then rerun the entry. `root_ambiguous`: reconcile the project root scope. `sync_mirror_conflict` / marker blockers: reconcile AGENTS.md / CLAUDE.md by hand. `owned_value_user_modified` / `provenance_corrupt`: reconcile the owned suite files. `blocked_cli` / `blocked_preset` / `blocked_backend` / `blocked_command`: repair binary capability, preset lint, backend readiness, or prompt-source binding. `worktree_reuse_key_missing`: pass an explicit reuse key. Never launch. |
| `incomplete_static_only` | Artifacts provisioned, static gate green, loop NOT closed (no authorized smoke) | 0 | `[CANDIDATE - operator must run manually] …`, or `[TEMPLATE - replace PLAN_PATH before running] …` when no plan exists yet | Re-confirm the target backend, then run the candidate command yourself; or rerun the entry with `--replay-transcript` to seek a bounded promotion. Never treat this as a ready command. |
| `complete` | Static gate green AND bounded replay smoke reached the terminal marker | 0 | the official launch command, no prefix | Run it. The loop was verified only under the replay harness caps; real-backend behaviour is still the operator's responsibility. |

Worktree launches (`run_pipeline(..., use_worktree=True, ...)`) must
carry an explicit reuse key (`--plan <plan>` or
`--worktree-name <name>`); a missing key is rejected as a `blocked`
view, never a launch command.

## Boundaries

- **Inputs.** Caller supplies the project directory (current cwd) and the
  preset path or builtin id. Read the resolved preset in full before deciding
  what else is needed. A plan, ordinary prompt file, environment variable,
  loop id, worktree name, or other runtime context is required only when the
  preset's actual launch contract needs it. Missing first-run business input
  does **not** block provisioning: the entry generates the reusable suite,
  marks the handoff `incomplete_static_only`, and shows the exact command
  template the operator must finish. Never invent a placeholder plan file.
- **No preset authoring.** Hat routing, AAF tables, preset schemas and
  builtin completions live in `ralph-preset-author` / `ralph-preset-review`.
  If the caller needs to change the preset, hand off there.
- **No day-to-day loop ops.** Once the suite ships, monitor / resume /
  merge / debug work belongs to the in-loop CLI / web dashboard
  (`ralph run`, `ralph loops`, `ralph diagnose`).  Once a loop has
  run and
  needs post-mortem diagnosis, hand off to `ralph-run-diagnosis`.
- **No silent backend spawning.** A safe loop smoke (if available) is
  always bounded by iteration / idle / wall-clock caps; any target-project
  smoke that touches non-replay backends requires explicit operator
  authorization captured by the handoff.

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
the helpers in `scripts/agent_docs.py`, which the entry point drives as
part of the generation batch. The helper is a pure stdlib module that
owns every persistent edit through a single managed section.

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
operator must reconcile by hand. The entry surfaces that blocker as a
`blocked` result; defaulting to a best-guess rewrite is forbidden by
the guardrails section above.

**Sync between AGENTS.md and CLAUDE.md.** The entry composes both docs
with `sync_with_other_doc=True` so asymmetric mirrors surface
`blocker(sync_mirror_conflict)` before the batch is staged. The
`conflicting-docs` fixture is the canonical regression.

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

The pipeline suite (`ralph.<stem>.yml`, `PROMPT.<stem>.md`) is generated
and safely maintained by `scripts/pipeline_suite.py`, driven by the
entry point. The helper owns exactly two files inside the target project
and never touches `AGENTS.md` / `CLAUDE.md` (those flow through
`agent_docs.py`) or the runtime ledger under `.ralph/`.

**Baseline plus project overlay.** Read `assets/ralph.pipeline.base.yml`
before composing a new config. The asset defines reusable runtime safety and
diagnosis defaults. The entry passes the audited `ProjectFacts` directly as
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
Embedded `profile_sha256` and `prompt_sha256` must match before refresh; a
mismatch is an ownership blocker (`owned_value_user_modified` /
`provenance_corrupt`).

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
different launch contract and is referenced rather than copied. For a
plan-driven preset with no plan yet, the entry keeps a managed fallback
prompt inside the suite and emits the `[TEMPLATE - replace PLAN_PATH before
running]` command carrying only `--plan PLAN_PATH`; do **not** also pass
`--prompt-file`, because CLI prompt-file precedence would select the
fallback instead of the real plan.

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

After the suite ships, the entry runs a four-stage static gate in
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
never claim "loop closed" based on a green dry-run alone. The
`incomplete_static_only` level is exactly this state.

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
