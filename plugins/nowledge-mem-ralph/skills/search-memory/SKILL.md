---
name: search-memory
description: Search the Nowledge Mem knowledge base when stored insights would improve the current Ralph task. Bounded, read-only JSON queries only. Recognize when past decisions, root causes or conventions are relevant; search proactively, but never write, save or distill anything.
---

# Search Memory (Ralph)

Read-only access to Nowledge Mem for Ralph project work. This skill
queries memories; it never captures sessions and never mutates the
knowledge base.

## When to search

**Strong signals:**

- Continuity: the current task connects to prior work in this repo
- Pattern match: the problem resembles a previously solved issue
- Decision context: "why/how was X chosen" implies documented rationale
- Implicit recall: "that approach", "like before", "the earlier plan"

**Contextual signals:**

- Hard debugging (may match a past root cause)
- Architecture or preset design discussion (choices may be documented)
- Convention questions (testing rules, workflow contracts)

**Skip when:**

- The topic is fundamentally new
- Generic syntax or tooling questions answerable from docs
- A fresh perspective is explicitly requested

## Query contract

Run one bounded JSON memory search:

```bash
nmem --json m search "<semantic core of the question>" --limit 5
```

- Extract the semantic core of the question; keep repo terminology.
- Parse the `memories` array; higher `score` means better relevance.
- 5 results is the ceiling. Do not raise `--limit`.

## Conditional tracing of the original conversation

Only when the user needs the original conversation, or the memory
results are insufficient to answer:

```bash
nmem --json t search "<query>" --limit 5
```

If a memory carries `source_thread`, or thread search finds the likely
conversation, read it one bounded page at a time:

```bash
nmem --json t show <thread_id> --limit 8 --offset 0 --content-limit 1200
```

Increase `--offset` only when the loaded page still lacks the needed
information. Stop paging once the question is answered.

## Stop conditions

- No relevant results: say so clearly and continue the task without
  memory input.
- nmem missing, server unreachable, or unparseable JSON: report the
  original error and stop. Do not retry with other subcommands and
  never substitute a write command.

## Read-only contract

Allowed nmem calls are exactly `--json m search`, `--json t search`
and `--json t show` as above, plus `--json status` for health. Memory
and thread write commands are forbidden for this skill; session
capture and distillation belong to Ralph's own curation process, not
to this plugin.
