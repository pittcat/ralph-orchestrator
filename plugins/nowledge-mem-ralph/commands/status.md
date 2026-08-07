---
description: Read-only Nowledge Mem connection and server status check
---

# Status (Ralph)

Check the Nowledge Mem server connection. This command performs
exactly one read-only JSON call and nothing else.

## Command

```bash
nmem --json status
```

## Output

The JSON payload reports connection health, typically including:

- **server** — reachable or not
- **api url** — where the CLI connects (local or remote)
- **database** — connected or disconnected
- **version** — nmem CLI version

Summarize the health state for the operator in one or two lines.

## Failure handling

If the command exits non-zero, or the JSON reports an unreachable
server or an authentication error:

1. Report the original error message **as-is**, including exit code
   and any JSON error fields. Do not paraphrase away the detail.
2. **Stop.** Do not run a second nmem subcommand, do not retry with
   different arguments, and never fall back to any write operation.
3. Point the operator at the environment, not at nmem: the Nowledge
   Mem server/app must be running and reachable before any query can
   succeed.

A failed status check is an actionable fault, not a degraded mode.
