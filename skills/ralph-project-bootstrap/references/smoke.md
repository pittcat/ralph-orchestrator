# Safe-Loop Smoke Harness

This document is the operator-facing reference for the bounded
safe-loop smoke that ``ralph-project-bootstrap`` may run after the
static gate. End agents do not see this file; the skill owner does.

## What the smoke is for

A green static gate (``capability`` → ``preset check --strict`` →
``preflight --strict`` → ``run --dry-run``) proves that the
runtime can statically load the suite. It does NOT prove that a loop
can reach a business event or that the configured backend can produce
a coherent response. The smoke is the optional next step: it runs the
loop end-to-end under a strict triple cap and classifies the outcome.

The smoke is deliberately opt-in. The bootstrap pipeline only invokes
it when the target backend is a content-fixed replay harness shipped
with the skill. Any other backend (mock / custom / real / unknown)
requires explicit operator authorization captured by the handoff;
without authorization the skill reports ``incomplete`` and stops.

## Backend classification

The harness recognises three classes of backend capability token:

| Class    | Kind                          | Auto-trusted? | Spawn behaviour |
| -------- | ----------------------------- | ------------- | --------------- |
| Safe     | ``content_fixed_replay``      | Yes           | Spawn under the triple cap. |
| Unsafe   | ``mock``                      | No            | Refused before any subprocess is constructed. |
| Unsafe   | ``custom``                    | No            | Refused before any subprocess is constructed. |
| Unsafe   | ``real``                      | No            | Refused unless ``RALPH_BOOTSTRAP_ALLOW_REAL_BACKEND=1`` is set AND an explicit runner is supplied. |
| Unsafe   | ``unknown``                   | No            | Refused before any subprocess is constructed. |

The ``content_fixed_replay`` kind is the ONLY kind the harness will
treat as auto-trusted. Any other kind — including ``mock`` — requires
operator authorization before the harness will spawn.

## The triple timeout bound

Every smoke is bounded by three orthogonal caps:

1. ``max_iterations`` — the runtime-level iteration cap. Forwarded as
   ``--max-iterations <N>`` on the argv; the runtime will refuse to
   advance past N iterations regardless of any other signal.
2. ``idle_timeout_secs`` — the runtime-level idle cap. Forwarded as
   ``--idle-timeout <S>`` on the argv (the real ``ralph run`` flag,
   unit seconds). If the runtime emits no event for that many seconds,
   the harness classifies the outcome as ``timeout_idle``.
3. ``wall_clock_timeout_s`` — the harness-side wall-clock cap. NOT
   forwarded to the CLI (the production ``ralph run`` does not
   accept a wall-clock flag — it lives on the harness outer
   ``timeout`` parameter). When the outer timeout fires the harness
   classifies the outcome as ``wall_clock_timeout`` and records the
   elapsed time in ``evidence``.

The three caps are intentionally independent. A green smoke is one
that reaches the bounded terminal marker (``LOOP_COMPLETE``) inside
all three caps simultaneously. Anything else falls into one of the
nine discrete outcomes below.

## Outcomes

The harness returns exactly one of nine outcomes:

| Outcome                       | Meaning |
| ----------------------------- | ------- |
| ``not_authorized``            | The backend is unsafe, or the real-binary env-var override is not set. No subprocess was constructed. |
| ``spawned``                   | The subprocess ran to completion but neither a first event nor a terminal marker was observed in captured stdout. |
| ``first_event_seen``          | The ``plan.ready`` marker was observed but the bounded terminal marker was not. |
| ``bounded_terminal_reached``  | The ``LOOP_COMPLETE`` marker was observed. The smoke passed. |
| ``timeout_no_event``          | The harness classified the run as a no-event timeout (no markers observed before the wall clock fired). |
| ``timeout_idle``              | The idle cap fired before any terminal marker was observed. |
| ``wall_clock_timeout``        | The outer subprocess timeout fired. |
| ``non_zero_exit``             | The subprocess exited non-zero. ``failure_bucket`` is set. |
| ``error_event_detected``      | An ``ERROR_EVENT:`` line was emitted by the runtime. ``failure_bucket`` is set. |

## Failure-bucket classification

Error-class outcomes (``non_zero_exit`` and ``error_event_detected``)
are routed into one of four buckets so the handoff can pick the right
follow-up action:

| Bucket               | Trigger (case-insensitive substring match in captured stdout / stderr) |
| -------------------- | -------------------------------------------------------------------- |
| ``preset``           | ``preset`` appears in the combined stream. |
| ``backend``          | ``backend`` appears in the combined stream (and ``preset`` does not). |
| ``project_command``  | ``project`` appears in the combined stream (and neither ``preset`` nor ``backend`` does). |
| ``suite``            | No bucket keyword matched. |

The substring rule is intentional: ``preset_error`` and
``backend_failure`` both hit. The classification prefers ``preset``,
then ``backend``, then ``project_command``, then ``suite``.

## Dirty-tree guarantee

The harness NEVER touches the operator's working tree. It does not
``git clean``, ``git reset``, ``git checkout``, ``git stash``, or
``git commit``; it does not ``rm`` files outside ``transcript_dir``;
it does not modify ``AGENTS.md`` / ``CLAUDE.md`` / the preset-bound config
/ prompt pair; it does not write to ``.ralph/``.

The harness only writes into the optional ``transcript_dir`` (when the
caller supplies one) and only as a side-effect of the spawned
subprocess itself. If the harness is called WITHOUT ``transcript_dir``,
no files are created on disk by the harness.

## Argv contract

Every argv the harness builds starts with:

```
<binary> -c <config_path> -H <preset>
        --max-iterations <N>
        --idle-timeout <S>
```

Exactly one optional prompt source follows: ``--plan <path>`` when present,
otherwise ``--prompt-file <path>``. ``extra_argv`` is appended
last so callers can layer in stable flags without disturbing the
harness contract.

The contract is enforced by the test suite
(``test_smoke_argv_shape_*``); every argv recorded in a
``SmokeResult`` MUST contain ``-c``, ``-H``, ``--max-iterations``
and ``--idle-timeout``. The wall-clock cap is NOT on the argv — it
lives on the harness outer ``timeout`` parameter only.

## Real-binary override

By default the harness refuses to spawn the real ``ralph`` binary.
The harness only spawns a real binary when BOTH:

* ``runner is None`` (the caller is using the default ``subprocess.run``)
  AND
* the environment variable ``RALPH_BOOTSTRAP_ALLOW_REAL_BACKEND`` is
  set to ``"1"``.

If either condition is not met, the harness returns
``SmokeResult(outcome="not_authorized", argv=(), ...)`` with a
precise reason in ``evidence``. Tests pass an explicit ``runner`` and
never set the override.

## Acceptance checklist

When extending ``smoke_runner.py``:

```bash
.venv/bin/python -m pytest skills/tests/test_project_bootstrap_contract.py -k smoke
.venv/bin/python -m pytest skills/tests/test_project_bootstrap_contract.py
```

Both must remain green. Every new fixture under
``fixtures/cli/smoke/`` must continue to drive the harness without
spawning the real binary.
