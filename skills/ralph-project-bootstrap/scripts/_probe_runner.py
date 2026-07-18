"""Deterministic runner that replays a fixture's recorded invocations.

The staged state machine in ``cli_probe`` accepts a ``runner`` argument
so the test suite can drive the gate without spawning the real ``ralph``
binary. ``make_runner(invocations)`` returns a callable compatible with
``subprocess.run``'s signature; the callable matches the requested
argv against the recorded invocations and replays the recorded
stdout/stderr/exit-code triple.

When the runner is asked for an argv it has no record of, it raises
``AssertionError`` — the test suite treats that as a hard fail because
it means a stage advanced past an argv the helper should not have
emitted.

The runner is NOT thread-safe; tests should construct a fresh runner
per scenario.
"""
from __future__ import annotations

import subprocess
from typing import Iterable

import cli_probe  # type: ignore[import-not-found]  # loaded via skills/ralph-project-bootstrap/scripts on sys.path


def make_runner(
    invocations: Iterable[cli_probe.FakeInvocation],
) -> "Callable[..., subprocess.CompletedProcess]":
    """Return a ``subprocess.run``-compatible callable that replays
    ``invocations`` in the order they were loaded.

    The returned callable accepts the same positional/keyword
    arguments as ``subprocess.run`` (``args``, ``timeout``,
    ``capture_output``, ``text``). All but ``args`` are ignored —
    timeout is enforced by the staged state machine itself.
    """
    queue = list(invocations)

    def _runner(args, timeout=None, capture_output=None, text=None):  # noqa: ARG001
        argv = tuple(args)
        for index, invocation in enumerate(queue):
            if invocation.argv_expected == argv:
                stdout = "".join(invocation.stdout_chunks)
                stderr = "".join(invocation.stderr_chunks)
                # Consume the matched invocation so a second request
                # for the same argv fails closed (deterministic).
                queue.pop(index)
                return subprocess.CompletedProcess(
                    args=argv,
                    returncode=invocation.exit_code,
                    stdout=stdout,
                    stderr=stderr,
                )
        raise AssertionError(
            f"cli_probe runner: no recorded invocation for argv={list(argv)}; "
            f"recorded={[(list(i.argv_expected), i.exit_code) for i in queue]}"
        )

    return _runner