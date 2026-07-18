# Agent-Docs Managed-Section Helper

This document is implementation guidance for the next skill implementor
working on `ralph-project-bootstrap`. It explains the marker format,
sync rules, and anti-patterns enforced by `scripts/agent_docs.py`. End
agents do not see this file; the skill owner does.

## What the helper owns

The helper persists exactly two files inside the target project:

- `AGENTS.md`
- `CLAUDE.md`

Both are mutated only via a single managed section. Anything outside
that section — operator headers, prose, hand-written checklists,
embedded HTML comments — passes through compose byte-for-byte.

## Marker format (exact bytes)

```
<!-- RALPH-BOOTSTRAP-START: <marker_id> v1 -->
<body lines, each terminated by a single newline>
<!-- RALPH-BOOTSTRAP-END: <marker_id> -->
```

Notes:

- The literal prefix `RALPH-BOOTSTRAP-` is non-negotiable and must not
  appear anywhere outside the START / END markers.
- `v1` is the only supported marker version today. Bump it only when
  the contract for the body changes.
- The boundary markers themselves are HTML comments: `<!-- ... -->`.
  Editors that strip comments will silently corrupt the section, so
  anyone editing the file by hand must keep the comments intact.
- The END marker has no version suffix; the START marker always
  carries one so a future rotation is detectable by the parser.

## Parser contract

`parse_managed_section(text, marker_id)` returns a `MarkerParse` with
`kind` in `{Missing, Ok, Duplicate, Truncated, Nested}`.

- **Missing** — no START, no END. Compose appends a fresh section.
- **Ok** — exactly one START followed by exactly one END. Compose may
  replace the section.
- **Duplicate** — more than one START or more than one END. Compose
  returns `blocker(marker_duplicate)`; the operator must reconcile by
  hand.
- **Truncated** — START present without a matching END. Compose
  returns `blocker(marker_truncated)`.
- **Nested** — END marker precedes the START marker (out-of-order).
  Compose returns `blocker(marker_nested)`. The parser never returns
  `Ok` for out-of-order markers even when both endpoints exist.

The byte ranges `start` / `end` on the `Ok` variant are absolute
offsets into the input text. Callers extract the body via
`_slice_ok(text, marker_id, parse)`; they must not compute body offsets
themselves.

## Compose contract

`compose_agent_docs(existing_text, owned_section_body, *, marker_id, ...)`
returns a `ComposeResult`. The four outcomes:

- `created` — `existing_text` was `None`. The helper authors a fresh
  document containing only the managed section.
- `updated` — the managed section was added or replaced. User prose
  outside the section is preserved byte-for-byte.
- `noop` — the existing section byte-equalled the requested body and
  no change is needed.
- `blocker(code, reason)` — a conflict was detected; nothing is
  written.

When the caller passes `sync_with_other_doc=True` plus
`other_body=...`, compose compares the prospective bodies of the two
docs and blocks if they disagree. The code is always
`sync_mirror_conflict` so callers can match the string literal.

## Idempotency guarantee

`compose_agent_docs` is a pure function over its arguments. Running it
twice with the same inputs returns:

- First call: `created` or `updated` (with the new text).
- Second call (using the new text as `existing_text`): `noop`, with
  the original `existing_text` returned verbatim.

This guarantee is exercised by `test_compose_is_idempotent_across_two_runs`.

## AtomicWriter

`AtomicWriter(operations)` is a context manager. The operations list
is `(target_path, new_content)` pairs. Calling `writer.execute()`
returns `(committed, rolled_back)`.

Behavior:

1. Stage every target into a sibling `.{name}.bootstrap.tmp` first.
2. If any stage raises `OSError`, all staged .tmp files are removed
   and every planned target is reported in `rolled_back`.
3. Otherwise, commit each target with `os.replace`. If any commit
   raises, every committed target is reverted to its original bytes
   (or deleted if it did not exist) and every planned target is
   reported in `rolled_back`.
4. On success, `committed` lists the targets that were renames in
   and `rolled_back` is empty.

The writer leaves no `.bootstrap.tmp` files behind. The next read
either sees the pre-batch state or the post-batch state — never an
in-between state.

## Dirty-tree protection

A batch that targets `AGENTS.md` and `CLAUDE.md` must not touch any
file the caller did not name. Implementation guidance:

- Use `AtomicWriter` with `(target_path, new_content)` pairs only.
- Never call `Path.glob`, `Path.rglob`, or `Path.iterdir` to discover
  files; the writer has no business walking the tree.
- The `dirty-tree` fixture is the canonical regression test: it
  contains `src/lib.rs` with a hash-stable body. After the writer
  runs, `src/lib.rs` content must be unchanged.

## Sync rules for AGENTS.md / CLAUDE.md

The two docs are mirror surfaces: their managed sections must agree on
every owned field. When a caller updates one, the helper enforces:

- If the caller passes the prospective body for the *other* doc
  (`sync_with_other_doc=True, other_body=...`), the helper compares
  the proposed bodies. Mismatches return
  `blocker(sync_mirror_conflict)`; the caller must reconcile before
  retrying.
- If the caller only provides `existing_text` (no `other_body`), the
  helper compares the existing other-doc body against the proposed
  AGENTS body. Same blocker code.

The canonical "what goes in the managed section" body is owned by the
caller; the helper does not invent fields. Today the bodies in
fixtures contain:

```yaml
linter: <one command, no comments>
test_runner: <one command, no comments>
```

…but the helper must not assume that shape — callers can put any
text inside as long as it is internally consistent.

## Anti-patterns

The following are forbidden and will be rejected by reviewers:

- **Symlinks.** The managed section lives in a plain text file.
  Symlinks hide the file from the writer and break the byte-for-byte
  preservation contract.
- **Absolute paths.** Every `ComposeResult` payload and every target
  passed to `AtomicWriter` must be repo-relative. Use
  `_paths.rel(target, root)` to normalise before storage.
- **References to internal ledgers.** The helper must never write to
  `.ralph/`, never read `events.jsonl` / `agent/tasks.jsonl`, and
  never reference the runtime loop's internal state. All
  cross-process state reaches the helper through arguments only.
- **References to specific hat collections or preset names.** The
  helper is technology-agnostic; it must not hard-code any
  collection name, preset id, or agent topology. Marker ids passed
  in by the caller are the only place those identifiers may appear.
- **Shell-out or chmod.** `AtomicWriter` uses `os.replace` and
  `pathlib` only. No `subprocess`, no `os.system`, no permission
  hacks. On any platform where `os.replace` is unavailable the
  helper must fail closed before any partial state is observed.
- **Side channels through compose.** `compose_agent_docs` returns a
  `ComposeResult`; it never imports platform-specific modules,
  reads environment variables, or touches the filesystem directly.
  Composition is pure; the writer is the only place that
  persists bytes.

## Acceptance checklist

When extending `agent_docs.py`, run the entire contract suite:

```bash
.venv/bin/python -m pytest skills/tests/test_project_bootstrap_contract.py
```

Plus at least one targeted run that exercises the new behaviour:

```bash
.venv/bin/python -m pytest skills/tests/test_project_bootstrap_contract.py -k <your-test-name>
```

The full suite must remain green; the new helper must not regress any
existing audit behaviour.
