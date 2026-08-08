---
description: Bounded read-only Nowledge Mem memory search for Ralph projects
argument-hint: <query>
---

# Search Memory (Ralph)

> **0.2.0 lifecycle context.** This command is the **manual** search
> path. The plugin also runs a **bounded loop-scoped recall** on every
> Ralph SessionStart (first session only — subsequent sessions, compact
> restarts, supervisor workers and retries all reuse the same loop
> cache). If you find yourself running `/nowledge-mem-ralph:search`
> during a normal Ralph activation, it usually means the recall cache
> has nothing on this topic yet or has aged out; this command is the
> escape hatch. **Do not** call it on every iteration — one bounded
> recall per loop is the contract (see
> `plugins/nowledge-mem-ralph/scripts/recall.py`).

Search the Nowledge Mem knowledge base for memories relevant to the
current Ralph work. This command is **read-only**: it never creates,
updates, deletes, saves or distills anything, and it never reads or
injects any session-start context.

## Empty input

If `$ARGUMENTS` is empty or whitespace-only, **stop before calling
nmem**. Show the usage below and ask for a concrete query. An empty
query is never a whole-store dump.

```text
Usage: /nowledge-mem-ralph:search <query>
Example: /nowledge-mem-ralph:search wave supervisor merge strategy
```

## Command

Run exactly one bounded JSON memory search:

```bash
nmem --json m search "$ARGUMENTS" --limit 5
```

Parse the JSON `memories` array. Higher `score` means a better
semantic match. Report the matching memories concisely; cite titles
when helpful. If nothing scores relevant, say so clearly and stop.

## Progressive tracing of the original conversation (conditional)

Only when the user explicitly needs the original conversation, or the
memory results above are insufficient to answer, you may trace the
source. Do not run these by default.

If a memory result carries `source_thread`, or the user asks about a
prior conversation itself, search threads with a bounded query:

```bash
nmem --json t search "$ARGUMENTS" --limit 5
```

To inspect a specific thread, read one bounded page starting at the
beginning:

```bash
nmem --json t show <thread_id> --limit 8 --offset 0 --content-limit 1200
```

Increase `--offset` by the page size only when the loaded page still
lacks the information the user asked for. Never load a whole thread
eagerly.

## Read-only contract

- Allowed calls are exactly: `nmem --json m search`, and — only for
  conditional tracing — `nmem --json t search` / `nmem --json t show`.
- Never call memory/thread write commands (`m add`, `t save`,
  `t distill`, ...) from this plugin.
- If nmem fails (missing CLI, server unreachable, invalid JSON),
  report the original error and stop; do not retry with a different
  subcommand.
