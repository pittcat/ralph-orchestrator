"""Safe-loop smoke harness for ``ralph-project-bootstrap``.

The smoke harness is the only path that ever spawns the ``ralph`` binary
in this skill. It is deliberately conservative:

* The harness only spawns a backend when the caller presents a
  ``SafeBackend`` capability token. ``UnsafeBackend`` tokens are
  refused before any subprocess is constructed.
* Every spawned invocation is bounded by three orthogonal caps:
  ``max_iterations``, ``idle_timeout_ms`` and ``wall_clock_timeout_s``.
* The harness classifies its outcome into nine discrete buckets so the
  handoff can render a precise message without reading raw stdout/stderr.
* The harness never touches the operator's working tree: it never
  cleans, reverts, auto-commits, or rewrites files outside the supplied
  ``transcript_dir``.
* Failures are bucketed into ``suite`` / ``preset`` / ``backend`` /
  ``project_command`` so callers can route follow-up actions.

Public API (everything else is private):

* ``SafeBackend`` — capability token required for the harness to spawn.
* ``UnsafeBackend`` — capability token that always refuses spawn.
* ``SmokeConfig`` — bounded smoke configuration.
* ``SmokeResult`` — outcome + evidence + argv + excerpts.
* ``run_smoke(backend, smoke_cfg, transcript_dir=None, runner=None)`` —
  bounded smoke entry point.
* ``FakeBinary`` — test fixture wrapper that renders a self-contained
  shell-style python script the harness can hand to the runner.

Hard rules:

* Pure stdlib. No third-party imports.
* The harness MUST refuse to spawn the real ``ralph`` binary unless the
  caller sets ``RALPH_BOOTSTRAP_ALLOW_REAL_BACKEND=1``. This gate is
  enforced before any subprocess is constructed.
* The harness MUST NOT set ``RALPH_*`` environment variables on the
  spawned process. The fake binary in tests is responsible for staging
  whatever it needs to read.
* The argv tuple the harness builds always contains
  ``-c <config_path> -H <preset> --max-iterations <N>
  --idle-timeout <S>``. Tests assert this contract. The wall-clock
  cap belongs to the harness outer ``timeout`` parameter and is NOT
  forwarded to the CLI.
"""
from __future__ import annotations

import os
import re
import signal
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Iterable, Literal

# Single source of truth for the auto-trusted backend kind. The literal
# is also referenced by ``references/smoke.md`` so the operator doc and
# the helper cannot drift apart.
SAFE_BACKEND_KIND: Literal["content_fixed_replay"] = "content_fixed_replay"

# Backends that NEVER auto-spawn. The harness MUST refuse to construct
# a subprocess when the caller presents one of these kinds.
UNSAFE_BACKEND_KINDS: frozenset[str] = frozenset({"mock", "custom", "real", "unknown"})

# Outcomes. Listed exhaustively so callers can switch on the literal.
OUTCOMES: tuple[str, ...] = (
    "not_authorized",
    "spawned",
    "first_event_seen",
    "bounded_terminal_reached",
    "timeout_no_event",
    "timeout_idle",
    "wall_clock_timeout",
    "non_zero_exit",
    "error_event_detected",
)

# Failure buckets. ``none`` means the smoke concluded without an
# error-class signal; the rest partition failure attribution so the
# handoff can pick the right follow-up action.
FAILURE_BUCKETS: tuple[str, ...] = (
    "none",
    "suite",
    "preset",
    "backend",
    "project_command",
)

# Env var the operator must set to authorise a real ``ralph`` spawn.
ALLOW_REAL_BACKEND_ENV = "RALPH_BOOTSTRAP_ALLOW_REAL_BACKEND"

# Event-name regexes used by the fake-binary scripts and the harness
# outcome classifier. The harness recognises the FIRST observable event
# (``plan.ready``) and the BOUNDED TERMINAL marker (``LOOP_COMPLETE``)
# from captured stdout; error events are classified by substring match.
FIRST_EVENT_PATTERN = re.compile(r"plan\.ready")
TERMINAL_EVENT_PATTERN = re.compile(r"LOOP_COMPLETE")
ERROR_EVENT_PATTERN = re.compile(r"ERROR_EVENT:")

# Grace window for SIGTERM → SIGKILL escalation on the reap path.
# When the outer timeout fires the harness sends SIGTERM to the entire
# child process group (parent + pty / log writer / temp watcher); if
# the group has not exited within this many seconds the harness
# escalates to SIGKILL against the same group. POSIX-portable.
_SIGKILL_GRACE_S: float = 2.0


@dataclass(frozen=True)
class SafeBackend:
    """Capability token that authorises the harness to spawn.

    A ``SafeBackend`` is the ONLY kind that the harness will accept for
    spawning a real subprocess. The ``is_trusted`` property enforces
    this at runtime — any code path that constructs a
    ``SafeBackend`` with a non-replay kind is a programming error.
    """

    name: str
    kind: Literal["content_fixed_replay"] = SAFE_BACKEND_KIND
    deterministic: bool = True
    transcript_path: Path | None = None

    def __post_init__(self) -> None:
        if self.kind != SAFE_BACKEND_KIND:
            raise ValueError(
                f"SafeBackend.kind must be {SAFE_BACKEND_KIND!r}; got {self.kind!r}"
            )

    @property
    def is_trusted(self) -> bool:
        return self.kind == SAFE_BACKEND_KIND


@dataclass(frozen=True)
class UnsafeBackend:
    """Capability token that NEVER authorises a spawn.

    The harness refuses to construct any subprocess for an
    ``UnsafeBackend`` regardless of the supplied config or env vars.
    The ``note`` field is surfaced in the resulting ``evidence`` so
    callers can render a precise refusal reason in the handoff.
    """

    name: str
    kind: str = "unknown"
    note: str = "unsafe backend kind requires explicit operator authorization"

    def __post_init__(self) -> None:
        if self.kind not in UNSAFE_BACKEND_KINDS:
            raise ValueError(
                f"UnsafeBackend.kind must be one of {sorted(UNSAFE_BACKEND_KINDS)}; "
                f"got {self.kind!r}"
            )

    @property
    def is_trusted(self) -> bool:
        return False


# ---------------------------------------------------------------------------
# Public configuration + result dataclasses
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class SmokeConfig:
    """Configuration for a single bounded smoke invocation.

    All timeout fields are *harness-side* caps. The harness enforces
    them by passing an outer ``timeout`` to the runner and by reading
    the captured stdout for the FIRST / TERMINAL event markers. The
    two caps are intentionally orthogonal:

    * ``max_iterations`` — the runtime-level iteration cap forwarded
      as ``--max-iterations`` (a real ``ralph run`` flag).
    * ``idle_timeout_secs`` — the runtime-level idle cap forwarded as
      ``--idle-timeout`` (a real ``ralph run`` flag with unit
      seconds). The runtime fires ``IdleTimeout`` when no output has
      been observed for that many seconds.
    * ``wall_clock_timeout_s`` — the harness-side wall-clock cap. This
      is NOT forwarded to the CLI (the real ``ralph run`` does not
      accept a wall-clock flag); the harness enforces it via the
      outer ``timeout`` parameter on the runner plus the process-group
      reap path in ``_spawn_real_backend``.

    Unit semantics:

    * ``idle_timeout_secs`` is in SECONDS (matches the real CLI's
      ``--idle-timeout``). Callers that need sub-second precision must
      round up; the runtime treats 0 as "disabled".
    * ``wall_clock_timeout_s`` is in SECONDS (matches Python's
      ``subprocess.communicate(timeout=...)``). It is an outer
      safety net, not an iteration cap.
    """

    binary: Path
    config_path: str
    preset: str
    prompt_file: str | None
    plan_path: str | None
    max_iterations: int = 3
    idle_timeout_secs: int = 30
    wall_clock_timeout_s: int = 60
    extra_argv: tuple[str, ...] = ()


@dataclass(frozen=True)
class SmokeResult:
    """Outcome + evidence for a single bounded smoke run.

    ``argv`` records the EXACT argv the harness would have executed
    (or did execute when ``outcome != "not_authorized"``). For the
    refusal path the argv is the empty tuple so callers can assert
    "no subprocess was constructed" by inspecting ``argv``.
    """

    outcome: str
    evidence: tuple[str, ...]
    argv: tuple[str, ...]
    stderr_excerpt: str
    stdout_excerpt: str
    elapsed_seconds: float
    failure_bucket: str

    def __post_init__(self) -> None:
        if self.outcome not in OUTCOMES:
            raise ValueError(f"unknown outcome: {self.outcome!r}")
        if self.failure_bucket not in FAILURE_BUCKETS:
            raise ValueError(f"unknown failure_bucket: {self.failure_bucket!r}")


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


def _build_argv(smoke_cfg: SmokeConfig) -> tuple[str, ...]:
    """Compose the argv tuple the harness will hand to the runner.

    The shape is the single source of truth used by both the harness
    and the contract suite: every argv starts with
    ``<binary> -c <config_path> -H <preset> --max-iterations <N>
    --idle-timeout <S>``. The wall-clock cap is NOT forwarded — it
    belongs to the harness outer ``timeout`` parameter, NOT to the
    CLI surface (see plan 2026-07-19-001 F6 / Unit 4 / S8).

    Exactly one optional prompt source follows: ``--plan`` when present,
    otherwise ``--prompt-file``;
    ``extra_argv`` is appended last so callers can layer in stable
    flags without disturbing the harness contract.
    """
    argv: list[str] = [
        str(smoke_cfg.binary),
        "-c",
        smoke_cfg.config_path,
        "-H",
        smoke_cfg.preset,
        "--max-iterations",
        str(smoke_cfg.max_iterations),
        "--idle-timeout",
        str(smoke_cfg.idle_timeout_secs),
    ]
    if smoke_cfg.plan_path:
        argv.extend(["--plan", smoke_cfg.plan_path])
    elif smoke_cfg.prompt_file:
        argv.extend(["--prompt-file", smoke_cfg.prompt_file])
    argv.extend(smoke_cfg.extra_argv)
    return tuple(argv)


def _classify_failure_bucket(stdout: str, stderr: str) -> str:
    """Map an error-class signal to its failure bucket.

    The classification prefers error-event markers in stdout
    (``ERROR_EVENT: <class> <message>``); when no marker is present the
    harness falls back to scanning stderr for the bucket keyword. The
    keywords ``preset``, ``backend`` and ``project`` MUST appear as
    standalone substrings — substring match without word boundaries is
    intentional so ``preset_error`` and ``backend_failure`` both hit.
    """
    combined = f"{stdout}\n{stderr}"
    if "preset" in combined:
        return "preset"
    if "backend" in combined:
        return "backend"
    if "project" in combined:
        return "project_command"
    return "suite"


def _summarise(stdout: str, stderr: str, limit: int = 400) -> tuple[str, str]:
    """Return ``(stdout_excerpt, stderr_excerpt)`` truncated to ``limit`` chars."""
    return stdout[:limit], stderr[:limit]


def _spawn_real_backend(
    argv: tuple[str, ...], outer_timeout: float
) -> tuple[str, str, int, float]:
    """Spawn the real ``ralph`` binary with POSIX-portable process-group reap.

    Returns ``(stdout, stderr, returncode, elapsed_seconds)``. When
    the outer timeout fires the harness reaps the entire child
    process group (parent + pty / log writer / temp watcher siblings)
    via ``os.killpg`` so orphan descendants cannot leak into the
    target project tree. POSIX-portable; uses
    ``preexec_fn=os.setsid`` rather than ``start_new_session=True``
    for compatibility with the widest range of POSIX libcs (setsid is
    the canonical mechanism for detaching into a new session / new
    process group).

    On ``subprocess.TimeoutExpired`` (or any equivalent timeout
    signal) the function reaps the group FIRST, then re-raises
    ``subprocess.TimeoutExpired`` with the partial stdout/stderr
    attached so ``run_smoke`` can classify the outcome uniformly
    across the real-backend path and the duck-typed runner path.
    The reap happens before the raise so the caller never sees a
    TimeoutExpired without the group already being reaped.
    """
    started = time.monotonic()
    proc = subprocess.Popen(
        list(argv),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        preexec_fn=os.setsid,
    )
    try:
        stdout, stderr = proc.communicate(timeout=outer_timeout)
    except subprocess.TimeoutExpired as exc:
        _reap_child_group(proc)
        elapsed = time.monotonic() - started
        # Re-raise with the partial captures attached so the caller
        # can produce the same wall_clock_timeout evidence shape as
        # the duck-typed runner path.
        raise subprocess.TimeoutExpired(
            cmd=exc.cmd,
            timeout=exc.timeout,
            output=exc.stdout,
            stderr=exc.stderr,
        ) from exc
    elapsed = time.monotonic() - started
    returncode = proc.returncode if proc.returncode is not None else 0
    return stdout or "", stderr or "", returncode, elapsed


def _reap_child_group(proc: subprocess.Popen) -> None:
    """Reap ``proc``'s entire process group via ``os.killpg``.

    SIGTERM is sent first to give the group a chance to exit
    gracefully. If the group has not exited within
    ``_SIGKILL_GRACE_S`` the function escalates to SIGKILL against
    the same group. POSIX-portable.

    Safe to call when the group has already exited: ``ProcessLookupError``
    is swallowed so a benign race (process reaped by the kernel just
    before our signal) does not mask the actual classification the
    caller wants to make.
    """
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        proc.wait(timeout=_SIGKILL_GRACE_S)
        return
    except subprocess.TimeoutExpired:
        pass
    # Grace window expired — escalate.
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except ProcessLookupError:
        return
    # Best-effort final reap so we do not leak a zombie. ``wait()``
    # may raise again if the kernel is slow to deliver SIGKILL; swallow
    # so the caller still gets the wall_clock_timeout classification.
    try:
        proc.wait(timeout=_SIGKILL_GRACE_S)
    except subprocess.TimeoutExpired:
        pass


# ---------------------------------------------------------------------------
# Fake-binary helper (used by the test suite)
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class FakeBinary:
    """Renders a self-contained shell-style script that stands in for
    the real ``ralph`` binary during smoke tests.

    The fake reads its argv, optionally stages a ``transcript_dir``
    (so tests can simulate the runtime's ``events.jsonl`` discovery),
    then emits the lines declared on the wrapped ``SmokeConfig``. The
    script is pure stdlib Python; it requires no third-party imports.

    The helper exposes ``script_contents()`` so tests can write the
    script into a tmp file and pass that file as ``SmokeConfig.binary``.
    """

    transcript_dir: Path
    smoke_cfg: SmokeConfig
    # The fake-binary "script plan" is rendered from these fields.
    script_lines: tuple[str, ...] = field(default_factory=tuple)
    exit_code: int = 0
    hang_seconds: float = 0.0  # when > 0, the script sleeps this long.

    def script_contents(self) -> str:
        """Render the self-contained fake-ralph script.

        The script is intentionally defensive: it accepts any argv, only
        acts on the config / preset it was told to act on, prints the
        configured stdout lines on stdout, prints the configured stderr
        lines on stderr, optionally sleeps, then exits with the
        configured code. The harness never sees anything other than
        what this script emits.
        """
        transcript = str(self.transcript_dir)
        stdout_lines = "\n".join(self.script_lines) + ("\n" if self.script_lines else "")
        sleep_block = (
            f"import time; time.sleep({self.hang_seconds})"
            if self.hang_seconds > 0
            else ""
        )
        return _FAKE_BINARY_TEMPLATE.format(
            transcript=transcript,
            stdout=stdout_lines,
            stderr="",
            exit_code=self.exit_code,
            sleep_block=sleep_block,
            argv_repr=repr(tuple()),
        )


_FAKE_BINARY_TEMPLATE = """#!/usr/bin/env python3
# Auto-generated by FakeBinary; do not edit by hand.
import os
import sys
import time
transcript_dir = {transcript!r}
try:
    os.makedirs(transcript_dir, exist_ok=True)
    with open(os.path.join(transcript_dir, "events.jsonl"), "w", encoding="utf-8") as handle:
        handle.write("plan.ready\\n")
except Exception:
    pass
{sleep_block}
sys.stdout.write({stdout!r})
sys.stderr.write({stderr!r})
sys.exit({exit_code})
"""


# ---------------------------------------------------------------------------
# Public entry point
# ---------------------------------------------------------------------------


def _make_smoke_result(
    outcome: str,
    argv: tuple[str, ...],
    evidence: tuple[str, ...],
    *,
    failure_bucket: str,
    elapsed: float,
    stdout_excerpt: str,
    stderr_excerpt: str,
) -> SmokeResult:
    return SmokeResult(
        outcome=outcome,
        evidence=evidence,
        argv=argv,
        stderr_excerpt=stderr_excerpt,
        stdout_excerpt=stdout_excerpt,
        elapsed_seconds=elapsed,
        failure_bucket=failure_bucket,
    )


def run_smoke(
    backend: SafeBackend | UnsafeBackend,
    smoke_cfg: SmokeConfig,
    transcript_dir: Path | None = None,
    runner: Callable[..., subprocess.CompletedProcess] | None = None,
) -> SmokeResult:
    """Run a bounded safe-loop smoke against the configured backend.

    The harness refuses to spawn unless:

    * ``backend`` is a ``SafeBackend`` (with the canonical replay kind),
      AND
    * the caller has supplied an explicit ``runner`` for tests OR set
      the ``RALPH_BOOTSTRAP_ALLOW_REAL_BACKEND`` env var to ``"1"``.

    Otherwise it returns
    ``SmokeResult(outcome="not_authorized", argv=(), ...)`` with a
    precise reason in ``evidence``. The argv tuple is empty so callers
    can assert "no subprocess was constructed" purely by inspecting
    ``argv``.

    When spawning is authorised, the harness builds the canonical argv,
    hands it to the runner with a deterministic outer timeout, and
    classifies the captured stdout/stderr into one of the nine
    ``SmokeResult.outcome`` values.

    Transcript contract (``transcript_dir``):

    When ``transcript_dir`` is supplied the harness consumes it on the
    authorised path: the directory is recorded as
    ``transcript_dir=<path>`` in ``evidence`` ahead of the spawn, so
    EVERY classification branch (terminal / first-event / spawned /
    non-zero / error-event / wall-clock timeout) carries proof of which
    transcript the replay smoke was staged against. Existence is NOT
    enforced here (the harness cannot observe whether the runner
    actually replayed it); authorization integrity is owned by the
    caller's resolved-backend gate. ``transcript_dir=None`` preserves
    the historical semantics unchanged (no evidence entry).

    Real-backend reap contract (runner is None):

    When ``runner is None`` the harness spawns the real ``ralph``
    binary via ``subprocess.Popen`` with ``preexec_fn=os.setsid`` so
    the child is the leader of its own process group. If the outer
    timeout fires, the harness calls ``os.killpg(proc.pid, SIGTERM)``
    to reap the ENTIRE group — the parent, any pty / log writer /
    temp watcher children share the group so a single killpg
    prevents orphan descendants from leaking into the target project
    tree. If the group does not exit within ``_SIGKILL_GRACE_S`` the
    harness escalates to ``SIGKILL`` against the same group.

    The reap contract applies ONLY to the real-backend path
    (``runner is None``). When the caller injects a ``runner``
    callable, the harness routes through that callable unchanged and
    does NOT invoke ``os.killpg`` — the injected runner owns its own
    subprocess lifecycle and the harness cannot tell what pid to
    reap. Tests inject fakes; tests therefore take the duck-typed
    branch and the killpg branch is exercised in production by the
    real backend, with the contract verified by
    ``test_run_smoke_reaps_child_group_on_outer_timeout``.
    """
    argv = _build_argv(smoke_cfg)
    stdout_excerpt = ""
    stderr_excerpt = ""
    elapsed = 0.0
    failure_bucket: str = "none"
    evidence: list[str] = []

    # --- gate 1: backend must be trusted ---------------------------------
    if not getattr(backend, "is_trusted", False):
        reason = (
            f"backend {backend.name!r} of kind {getattr(backend, 'kind', 'unknown')!r} "
            f"is not auto-trusted; {getattr(backend, 'note', 'operator authorization required')}"
        )
        return _make_smoke_result(
            "not_authorized",
            (),
            (reason, "refused before any subprocess was constructed"),
            failure_bucket="none", elapsed=0.0,
            stdout_excerpt="", stderr_excerpt="",
        )

    # --- gate 2: real binary needs explicit operator override -------------
    if runner is None and os.environ.get(ALLOW_REAL_BACKEND_ENV) != "1":
        reason = (
            f"real ralph binary refused: set {ALLOW_REAL_BACKEND_ENV}=1 to authorise; "
            f"tests must pass an explicit runner instead"
        )
        return _make_smoke_result(
            "not_authorized",
            (),
            (
                reason,
                "harness default runner would spawn the real binary; tests must inject a fake",
            ),
            failure_bucket="none", elapsed=0.0,
            stdout_excerpt="", stderr_excerpt="",
        )

    # --- transcript consumption -------------------------------------------
    # When the caller supplies ``transcript_dir`` the authorised path
    # records it in ``evidence`` so every classification branch
    # (terminal / first-event / spawned / failure / timeout) carries
    # proof of WHICH transcript the replay smoke was staged against;
    # the entry flows through ``SmokeResult.evidence`` into the handoff.
    # ``transcript_dir=None`` preserves the legacy harness semantics
    # unchanged for callers that do not stage a transcript. Existence
    # of the directory is deliberately NOT enforced here: the harness
    # cannot observe whether the runner actually replayed the
    # transcript, and existing harness callers pass staging paths that
    # may only materialise inside the runner. Authorization integrity
    # is owned by the pipeline's resolved-backend gate, not by this
    # recording step.
    if transcript_dir is not None:
        evidence.append(f"transcript_dir={Path(transcript_dir)}")

    run = runner if runner is not None else subprocess.run
    outer_timeout = float(smoke_cfg.wall_clock_timeout_s) + 5.0

    # --- spawn -----------------------------------------------------------
    started = time.monotonic()
    try:
        if runner is None:
            # Real-backend path: use POSIX-portable Popen with
            # preexec_fn=os.setsid so the harness can reap the entire
            # child process group via os.killpg on outer timeout.
            # Tests inject ``runner=`` fakes and continue to use the
            # duck-typed branch below unchanged.
            stdout, stderr, returncode, elapsed = _spawn_real_backend(
                argv, outer_timeout
            )
        else:
            completed = run(
                list(argv),
                timeout=outer_timeout,
                capture_output=True,
                text=True,
            )
            elapsed = time.monotonic() - started
            # The runner may be a stub that returns a duck-typed object.
            stdout = getattr(completed, "stdout", "") or ""
            stderr = getattr(completed, "stderr", "") or ""
            returncode = int(getattr(completed, "returncode", 0))
    except subprocess.TimeoutExpired as exc:
        elapsed = time.monotonic() - started
        stdout_excerpt, stderr_excerpt = _summarise(
            exc.stdout.decode("utf-8", errors="replace") if isinstance(exc.stdout, bytes) else (exc.stdout or ""),
            exc.stderr.decode("utf-8", errors="replace") if isinstance(exc.stderr, bytes) else (exc.stderr or ""),
        )
        reap_note = (
            "; child process group reaped via os.killpg"
            if runner is None
            else ""
        )
        evidence.append(
            f"subprocess exceeded wall_clock_timeout_s={smoke_cfg.wall_clock_timeout_s}; "
            f"elapsed={elapsed:.3f}s{reap_note}"
        )
        return _make_smoke_result(
            "wall_clock_timeout",
            argv,
            tuple(evidence),
            failure_bucket="suite", elapsed=elapsed,
            stdout_excerpt=stdout_excerpt, stderr_excerpt=stderr_excerpt,
        )
    stdout_excerpt, stderr_excerpt = _summarise(str(stdout), str(stderr))

    # --- classify ---------------------------------------------------------
    # Priority order (most-specific first):
    #   1. non-zero exit      -> non_zero_exit
    #   2. error event line   -> error_event_detected (with bucket)
    #   3. terminal marker    -> bounded_terminal_reached
    #   4. first event seen   -> first_event_seen
    #   5. spawned with no event yet -> spawned
    if returncode != 0:
        failure_bucket = _classify_failure_bucket(stdout, stderr)
        evidence.append(f"exit_code={returncode}")
        if stderr.strip():
            evidence.append(f"stderr={stderr.strip()[:200]}")
        return _make_smoke_result(
            "non_zero_exit",
            argv,
            tuple(evidence),
            failure_bucket=failure_bucket, elapsed=elapsed,
            stdout_excerpt=stdout_excerpt, stderr_excerpt=stderr_excerpt,
        )

    if ERROR_EVENT_PATTERN.search(stdout):
        bucket = _classify_failure_bucket(stdout, stderr)
        evidence.append(f"error_event_detected bucket={bucket}")
        return _make_smoke_result(
            "error_event_detected",
            argv,
            tuple(evidence),
            failure_bucket=bucket, elapsed=elapsed,
            stdout_excerpt=stdout_excerpt, stderr_excerpt=stderr_excerpt,
        )

    if TERMINAL_EVENT_PATTERN.search(stdout):
        return _make_smoke_result(
            "bounded_terminal_reached",
            argv,
            tuple(evidence) + ("LOOP_COMPLETE marker observed in captured stdout",),
            failure_bucket="none", elapsed=elapsed,
            stdout_excerpt=stdout_excerpt, stderr_excerpt=stderr_excerpt,
        )

    if FIRST_EVENT_PATTERN.search(stdout):
        return _make_smoke_result(
            "first_event_seen",
            argv,
            tuple(evidence) + ("plan.ready marker observed; bounded terminal not reached",),
            failure_bucket="none", elapsed=elapsed,
            stdout_excerpt=stdout_excerpt, stderr_excerpt=stderr_excerpt,
        )

    return _make_smoke_result(
        "spawned",
        argv,
        tuple(evidence) + ("spawned; no first or terminal event observed in captured stdout",),
        failure_bucket="none", elapsed=elapsed,
        stdout_excerpt=stdout_excerpt, stderr_excerpt=stderr_excerpt,
    )


__all__ = (
    "ALLOW_REAL_BACKEND_ENV",
    "FAILURE_BUCKETS",
    "FakeBinary",
    "OUTCOMES",
    "SAFE_BACKEND_KIND",
    "SafeBackend",
    "SmokeConfig",
    "SmokeResult",
    "UNSAFE_BACKEND_KINDS",
    "UnsafeBackend",
    "run_smoke",
)
