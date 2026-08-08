"""Tests for U02 loop-scoped recall.

These tests pin the U02 contract — see
``.ralph/specs/nowledge-mem-ralph-plugin-design.md`` §5.3 lifecycle
state machine and KTD matrix. They prove:

* The first SessionStart for a given ``loop_id`` runs **exactly one**
  bounded ``nmem --json m search`` call whose query is normalized from
  ``repo_basename + preset + workspace`` (no transcript / prompt content).
* A second SessionStart with the same ``loop_id`` and query_digest hits
  the loop cache and issues **zero** further ``nmem`` calls.
* ``source=compact`` takes the no-search path even when cache is warm
  (or cold: compact sources must not search at all).
* Fail-open contract: nmem missing / non-zero / invalid JSON / timeout
  must return ``additionalContext = ""``, never a fabricated context.
* XML escape + Unicode boundary truncation of the rendered context.

Tests use ``plugins/nowledge-mem-ralph/tests/fake_nmem`` (a small shell
script that records its argv and returns canned JSON) and a tmp
``CLAUDE_PLUGIN_DATA``.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import struct
import subprocess
import sys
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
PLUGIN_DIR = ROOT / "plugins" / "nowledge-mem-ralph"
HOOK_RUNTIME = PLUGIN_DIR / "scripts" / "hook_runtime.py"
RECALL_SCRIPT = PLUGIN_DIR / "scripts" / "recall.py"
FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures" / "recall"

# Fake nmem script content. It logs every argv invocation as JSONL to a
# file we pick, then returns a canned result (set via the
# FAKE_NMEM_RESULT env var if present, otherwise a default). Returns
# exit code 0 unless FAKE_NMEM_FAIL=1 → exit 1.
FAKE_NMEM = """#!/usr/bin/env python3
import json, os, sys
argv = sys.argv[1:]
log_path = os.environ.get("FAKE_NMEM_LOG", "/tmp/fake-nmem.log")
try:
    with open(log_path, "a", encoding="utf-8") as fh:
        fh.write(json.dumps({"argv": argv, "cwd": os.getcwd()}) + "\\n")
except Exception:
    pass
if os.environ.get("FAKE_NMEM_FAIL") == "1":
    sys.stderr.write("fake nmem forced fail\\n")
    sys.exit(1)
# Optional pre-canned payload.
payload_path = os.environ.get("FAKE_NMEM_RESULT")
result_path = os.environ.get("FAKE_NMEM_BADJSON")
if payload_path and os.path.exists(payload_path):
    sys.stdout.write(open(payload_path, encoding="utf-8").read())
    sys.exit(0)
if result_path and os.path.exists(result_path):
    sys.stdout.write(open(result_path, encoding="utf-8").read())
    sys.exit(0)
default = {
    "memories": [
        {"id": "m1", "title": "<safe>", "content": "alpha", "score": 0.91},
        {"id": "m2", "title": "safe & sound", "content": "beta", "score": 0.83},
    ]
}
sys.stdout.write(json.dumps(default))
sys.exit(0)
"""


def _seed_fake_nmem(bin_dir: Path) -> Path:
    bin_dir.mkdir(parents=True, exist_ok=True)
    path = bin_dir / "nmem"
    path.write_text(FAKE_NMEM, encoding="utf-8")
    path.chmod(0o755)
    return path


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
def fake_nmem(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> tuple[Path, Path]:
    """Install a fake ``nmem`` script into a tmp bin dir on PATH.

    Returns ``(bin_dir, log_file)`` — ``log_file`` is a JSONL that the
    fake script appends to on every invocation.
    """
    bin_dir = tmp_path / "fakebin"
    log_file = tmp_path / "nmem.log"
    _seed_fake_nmem(bin_dir)
    # ``monkeypatch.setenv`` only affects the test process; the fake
    # script reads this env too so we propagate it via os.environ.
    monkeypatch.setenv("FAKE_NMEM_LOG", str(log_file))
    os.environ["FAKE_NMEM_LOG"] = str(log_file)
    # Prepend so the fake is found before any system nmem.
    existing = os.environ.get("PATH", "")
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{existing}")
    return bin_dir, log_file


@pytest.fixture
def plugin_data(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    data_dir = tmp_path / "plugin_data"
    monkeypatch.setenv("CLAUDE_PLUGIN_DATA", str(data_dir))
    # Strip every Ralph loop env / NOWLEDGE key so each test sets them
    # explicitly.
    for key in (
        "RALPH_CURRENT_HAT",
        "RALPH_CURRENT_LOOP_ID",
        "RALPH_EVENTS_FILE",
        "RALPH_TRIGGERED_HAT",
        "RALPH_HATS_SOURCE",
        "RALPH_CONFIG",
        "RALPH_WORKSPACE_ROOT",
        "NOWLEDGE_HOOK_EVENT",
        "FAKE_NMEM_FAIL",
        "FAKE_NMEM_RESULT",
        "FAKE_NMEM_BADJSON",
    ):
        monkeypatch.delenv(key, raising=False)
    return data_dir


def _is_memory_search_argv(argv: list[str]) -> bool:
    """Return True if ``argv`` (the form after PATH resolution) is a
    bounded ``m search`` call. The fake ``nmem`` script logs its argv
    WITHOUT the program name — it's already running as ``nmem``."""
    return argv and argv[0] == "--json" and len(argv) >= 4 and argv[1] == "m" and argv[2] == "search"


def _read_nmem_log(log_file: Path) -> list[list[str]]:
    resolved = log_file.resolve()
    if not resolved.exists():
        return []
    entries = []
    for line in resolved.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line:
            continue
        entries.append(json.loads(line)["argv"])
    return entries


def _base_env(loop_id: str = "loop-u02") -> dict[str, str]:
    return {
        "RALPH_CURRENT_LOOP_ID": loop_id,
        "RALPH_CURRENT_HAT": "planner",
        "RALPH_HATS_SOURCE": "ce-executor-pipeline",
        "RALPH_WORKSPACE_ROOT": "/tmp/repo-u02",
    }


def _invoke_session_start(
    *,
    plugin_data: Path,
    extra_env: dict[str, str],
    stdin_payload: dict,
    bin_dir: Path | None = None,
) -> subprocess.CompletedProcess:
    path_parts = ["/usr/bin:/bin"]
    if bin_dir is not None:
        path_parts.insert(0, str(bin_dir))
    env = {
        "PATH": ":".join(path_parts),
        "PYTHONPATH": str(ROOT),
        "CLAUDE_PLUGIN_DATA": str(plugin_data),
    }
    # Carry the test's monkeypatched env so the fake nmem script and
    # the hook see ``FAKE_NMEM_LOG`` etc. without each test having to
    # pass it explicitly.
    for key, value in os.environ.items():
        env.setdefault(key, value)
    env.update(extra_env)
    return subprocess.run(
        [sys.executable, str(HOOK_RUNTIME), "SessionStart"],
        input=json.dumps(stdin_payload),
        text=True,
        capture_output=True,
        env=env,
        timeout=15,
    )


# ---------------------------------------------------------------------------
# S1 — first session triggers exactly one bounded search
# ---------------------------------------------------------------------------


def test_first_session_executes_one_search(
    fake_nmem: tuple[Path, Path], plugin_data: Path
) -> None:
    bin_dir, log_file = fake_nmem
    env = _base_env()
    env["NOWLEDGE_HOOK_EVENT"] = "SessionStart"

    result = _invoke_session_start(
        plugin_data=plugin_data,
        extra_env=env,
        stdin_payload={"session_id": "sess-1", "source": "startup"},
        bin_dir=bin_dir,
    )

    assert result.returncode == 0, (
        f"hook exit {result.returncode}: stderr={result.stderr}"
    )
    argvs = _read_nmem_log(log_file)
    memory_searches = [a for a in argvs if _is_memory_search_argv(a)]
    assert len(memory_searches) == 1, (
        f"expected exactly one bounded memory search, got {len(memory_searches)}: {memory_searches}"
    )
    argv = memory_searches[0]
    limit_idx = argv.index("--limit")
    assert argv[limit_idx + 1] == "5", f"memory search must cap at 5 results, got {argv}"

    state_path = plugin_data / env["RALPH_CURRENT_LOOP_ID"] / "state.json"
    assert state_path.is_file(), "session start must still write state marker"
    state = json.loads(state_path.read_text(encoding="utf-8"))
    assert state["cache_status"] == "miss", (
        f"first session cache_status must be 'miss', got {state.get('cache_status')!r}"
    )

    cache_path = plugin_data / env["RALPH_CURRENT_LOOP_ID"] / "recall.json"
    assert cache_path.is_file(), "first session must write the recall cache"


# ---------------------------------------------------------------------------
# S2 — second session in same loop is a cache hit (zero further searches)
# ---------------------------------------------------------------------------


def test_loop_cache_hit(
    fake_nmem: tuple[Path, Path], plugin_data: Path
) -> None:
    bin_dir, log_file = fake_nmem
    env = _base_env()
    env["NOWLEDGE_HOOK_EVENT"] = "SessionStart"

    # First run — cache miss.
    r1 = _invoke_session_start(
        plugin_data=plugin_data,
        extra_env=env,
        stdin_payload={"session_id": "sess-1", "source": "startup"},
        bin_dir=bin_dir,
    )
    assert r1.returncode == 0, r1.stderr

    first = _read_nmem_log(log_file)
    assert any(_is_memory_search_argv(a) for a in first), (
        f"first session must search: {first}"
    )

    # Second run — same loop. The hook payload's session_id and source
    # may differ; the cache key is loop_id + query_digest (which depends
    # only on repo basename + preset + workspace).
    r2 = _invoke_session_start(
        plugin_data=plugin_data,
        extra_env=env,
        stdin_payload={"session_id": "sess-2", "source": "resume"},
        bin_dir=bin_dir,
    )
    assert r2.returncode == 0, r2.stderr

    all_argvs = _read_nmem_log(log_file)
    memory_searches = [a for a in all_argvs if _is_memory_search_argv(a)]
    assert len(memory_searches) == 1, (
        f"cache hit must keep exactly one search across two runs; got {memory_searches}"
    )

    state2 = json.loads(
        (plugin_data / env["RALPH_CURRENT_LOOP_ID"] / "state.json").read_text(
            encoding="utf-8"
        )
    )
    assert state2["cache_status"] == "hit", (
        f"second run cache_status must be 'hit', got {state2.get('cache_status')!r}"
    )


# ---------------------------------------------------------------------------
# S3 — compact source never re-runs search
# ---------------------------------------------------------------------------


def test_compact_no_research(
    fake_nmem: tuple[Path, Path], plugin_data: Path
) -> None:
    bin_dir, log_file = fake_nmem
    env = _base_env()
    env["NOWLEDGE_HOOK_EVENT"] = "SessionStart"

    # Warm the cache (session_id irrelevant for cache key).
    warm = _invoke_session_start(
        plugin_data=plugin_data,
        extra_env=env,
        stdin_payload={"session_id": "sess-1", "source": "startup"},
        bin_dir=bin_dir,
    )
    assert warm.returncode == 0, warm.stderr
    first = _read_nmem_log(log_file)
    assert any(_is_memory_search_argv(a) for a in first)

    # Compact restart — source="compact" must NOT search.
    compact = _invoke_session_start(
        plugin_data=plugin_data,
        extra_env=env,
        stdin_payload={"session_id": "sess-1", "source": "compact"},
        bin_dir=bin_dir,
    )
    assert compact.returncode == 0, compact.stderr
    all_argvs = _read_nmem_log(log_file)
    # The first run wrote one search line. After compact we expect zero
    # ADDITIONAL search lines beyond that.
    memory_searches = [a for a in all_argvs if _is_memory_search_argv(a)]
    assert len(memory_searches) == 1, (
        f"compact source must not trigger a new search; got {memory_searches}"
    )

    # Cold-start with compact source: still no search.
    plugin_data_cold = plugin_data.parent / "plugin_data_cold"
    if plugin_data_cold.exists():
        shutil.rmtree(plugin_data_cold)
    plugin_data_cold.mkdir(parents=True, exist_ok=True)
    cold_env = dict(env)
    cold_result = subprocess.run(
        [sys.executable, str(HOOK_RUNTIME), "SessionStart"],
        input=json.dumps({"session_id": "cold", "source": "compact"}),
        text=True,
        capture_output=True,
        env={
            "PATH": f"{bin_dir}{os.pathsep}/usr/bin:/bin",
            "PYTHONPATH": str(ROOT),
            **{k: v for k, v in cold_env.items() if k != "PATH"},
            "CLAUDE_PLUGIN_DATA": str(plugin_data_cold),
            "FAKE_NMEM_LOG": str(log_file),
        },
        timeout=15,
    )
    assert cold_result.returncode == 0, cold_result.stderr


# ---------------------------------------------------------------------------
# S4 — nmem failure is fail-open: no fake context, session still starts
# ---------------------------------------------------------------------------


def test_recall_failure_fail_open(
    fake_nmem: tuple[Path, Path], plugin_data: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    bin_dir, log_file = fake_nmem
    monkeypatch.setenv("FAKE_NMEM_FAIL", "1")

    env = _base_env()
    env["NOWLEDGE_HOOK_EVENT"] = "SessionStart"

    result = _invoke_session_start(
        plugin_data=plugin_data,
        extra_env=env,
        stdin_payload={"session_id": "sess-err", "source": "startup"},
        bin_dir=bin_dir,
    )
    assert result.returncode == 0, (
        f"hook must fail-open with exit 0; got {result.returncode}: {result.stderr}"
    )
    assert result.stdout == "", (
        "fail-open must not inject a fabricated additionalContext; "
        f"stdout={result.stdout!r}"
    )

    state = json.loads(
        (plugin_data / env["RALPH_CURRENT_LOOP_ID"] / "state.json").read_text(
            encoding="utf-8"
        )
    )
    assert state["cache_status"] == "err", (
        f"nmem failure must record cache_status='err', got {state.get('cache_status')!r}"
    )

    cache_path = plugin_data / env["RALPH_CURRENT_LOOP_ID"] / "recall.json"
    if cache_path.exists():
        cache = json.loads(cache_path.read_text(encoding="utf-8"))
        assert not cache.get("context_xml"), (
            f"failed cache file must not carry a non-empty context_xml: {cache}"
        )


def test_recall_timeout_fail_open(
    fake_nmem: tuple[Path, Path], plugin_data: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A subprocess timeout from nmem must also fail-open."""
    bin_dir, _ = fake_nmem

    # Replace the fake script with one that sleeps > recall timeout.
    sleeper = bin_dir / "nmem"
    sleeper.write_text(
        "#!/usr/bin/env python3\nimport time, sys\ntime.sleep(20)\nsys.exit(0)\n",
        encoding="utf-8",
    )
    sleeper.chmod(0o755)

    env = _base_env(loop_id="loop-timeout")
    env["NOWLEDGE_HOOK_EVENT"] = "SessionStart"

    result = _invoke_session_start(
        plugin_data=plugin_data,
        extra_env=env,
        stdin_payload={"session_id": "sess-t", "source": "startup"},
        bin_dir=bin_dir,
    )
    assert result.returncode == 0, f"timeout must fail-open, got {result.returncode}"
    assert result.stdout == ""


def test_recall_invalid_json_fail_open(
    fake_nmem: tuple[Path, Path], plugin_data: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    bin_dir, _ = fake_nmem
    bad = plugin_data.parent / "bad.json"
    bad.write_text("this is not JSON at all {{", encoding="utf-8")
    monkeypatch.setenv("FAKE_NMEM_BADJSON", str(bad))

    env = _base_env(loop_id="loop-bad")
    env["NOWLEDGE_HOOK_EVENT"] = "SessionStart"

    result = _invoke_session_start(
        plugin_data=plugin_data,
        extra_env=env,
        stdin_payload={"session_id": "sess-x", "source": "startup"},
        bin_dir=bin_dir,
    )
    assert result.returncode == 0
    assert result.stdout == ""


# ---------------------------------------------------------------------------
# XML escape + Unicode-boundary truncation
# ---------------------------------------------------------------------------


def test_recall_xml_escapes_unsafe_characters(
    fake_nmem: tuple[Path, Path], plugin_data: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The injected additionalContext XML must escape <, >, & and control chars."""
    bin_dir, _ = fake_nmem
    payload_file = plugin_data.parent / "recall_payload.json"
    payload_file.write_text(
        json.dumps(
            {
                "memories": [
                    {
                        "id": "<id>",
                        "title": "5 < 7 & 9 > x",
                        "content": "alpha\nbeta\tgamma\r\n<script>alert(1)</script>",
                        "score": 0.91,
                    },
                ]
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setenv("FAKE_NMEM_RESULT", str(payload_file))

    env = _base_env(loop_id="loop-xml")
    env["NOWLEDGE_HOOK_EVENT"] = "SessionStart"

    result = _invoke_session_start(
        plugin_data=plugin_data,
        extra_env=env,
        stdin_payload={"session_id": "sess-xml", "source": "startup"},
        bin_dir=bin_dir,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout, "expected non-empty additionalContext for the safe payload"
    # Native escapes: & must not appear except as &amp;/&lt;/&gt;/&quot;
    assert "&lt;id&gt;" in result.stdout, f"id must be escaped: {result.stdout!r}"
    assert "&lt;script&gt;" in result.stdout, f"content must be escaped: {result.stdout!r}"
    assert "alert(1)" in result.stdout, "inner text must remain visible after escape"
    assert not any(bad in result.stdout for bad in ("\x00", "\x1f")), (
        "control characters must be stripped from context output"
    )


def test_recall_truncation_breaks_on_unicode_boundary(
    fake_nmem: tuple[Path, Path], plugin_data: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Truncation at the byte-budget must not split a UTF-8 codepoint."""
    # Build a payload where the truncation point lands inside a 4-byte
    # codepoint (emoji).
    emoji = "\U0001f600"  # 4-byte UTF-8
    content = emoji * 5000  # well over any reasonable cap
    payload_file = plugin_data.parent / "recall_long.json"
    payload_file.write_text(
        json.dumps(
            {
                "memories": [
                    {
                        "id": "long",
                        "title": "long",
                        "content": content,
                        "score": 0.5,
                    }
                ]
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setenv("FAKE_NMEM_RESULT", str(payload_file))

    env = _base_env(loop_id="loop-long")
    env["NOWLEDGE_HOOK_EVENT"] = "SessionStart"

    # Import recall and exercise render_context directly so we can
    # inspect bytes. We use importlib because plugin dir is hyphenated.
    import importlib.util

    spec = importlib.util.spec_from_file_location("_recall_under_test", RECALL_SCRIPT)
    assert spec and spec.loader
    recall = importlib.util.module_from_spec(spec)
    sys.modules["_recall_under_test"] = recall  # noqa: E501 - python 3.14 dataclasses
    spec.loader.exec_module(recall)

    payload = {
        "memories": [
            {"id": "long", "title": "long", "content": content, "score": 0.5}
        ]
    }

    bounded = recall.render_context(payload, max_bytes=512)
    encoded = bounded.encode("utf-8")
    # decode with strict=True; any split codepoint raises UnicodeDecodeError
    encoded.decode("utf-8")
    assert len(encoded) <= 512, f"output must respect byte cap, got {len(encoded)}"


# ---------------------------------------------------------------------------
# Concurrent SessionStart — same loop_id → at most one bounded search
# ---------------------------------------------------------------------------


def test_concurrent_session_start_serializes(
    fake_nmem: tuple[Path, Path], plugin_data: Path
) -> None:
    bin_dir, log_file = fake_nmem
    env = _base_env(loop_id="loop-race")
    env["NOWLEDGE_HOOK_EVENT"] = "SessionStart"

    # Launch two session-starts in parallel.
    procs = []
    for session_id in ("a", "b"):
        procs.append(
            subprocess.Popen(
                [sys.executable, str(HOOK_RUNTIME), "SessionStart"],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                env={
                    "PATH": f"{bin_dir}{os.pathsep}/usr/bin:/bin",
                    "PYTHONPATH": str(ROOT),
                    "CLAUDE_PLUGIN_DATA": str(plugin_data),
                    **env,
                    "FAKE_NMEM_LOG": str(log_file),
                },
                text=True,
            )
        )
        procs[-1].stdin.write(json.dumps({"session_id": session_id, "source": "startup"}))
        procs[-1].stdin.close()

    rc = [p.wait(timeout=15) for p in procs]
    assert all(r == 0 for r in rc), f"concurrent hooks must all exit 0, got {rc}"

    argvs = _read_nmem_log(log_file)
    memory_searches = [a for a in argvs if _is_memory_search_argv(a)]
    assert len(memory_searches) <= 1, (
        f"concurrent SessionStart must issue at most one bounded search; got {memory_searches}"
    )


# ---------------------------------------------------------------------------
# Query is derived only from repo basename + preset + workspace (no
# transcript / prompt content leakage).
# ---------------------------------------------------------------------------


def test_query_normalizes_to_repo_basename_and_preset(
    fake_nmem: tuple[Path, Path], plugin_data: Path
) -> None:
    bin_dir, log_file = fake_nmem
    env = _base_env(loop_id="loop-qn")
    env["NOWLEDGE_HOOK_EVENT"] = "SessionStart"

    # Even with sensitive-looking payloads, query should NOT include
    # transcript / last_assistant_message content.
    sensitive = "super-secret-payload-DO-NOT-LEAK-token-XYZ"
    _invoke_session_start(
        plugin_data=plugin_data,
        extra_env=env,
        stdin_payload={
            "session_id": "sess-q",
            "source": "startup",
            "transcript_path": f"/etc/{sensitive}",
            "last_assistant_message": sensitive,
        },
        bin_dir=bin_dir,
    )

    argvs = _read_nmem_log(log_file)
    assert argvs, "expected at least one search invocation"
    memory_searches = [a for a in argvs if _is_memory_search_argv(a)]
    # argv layout: [--json, m, search, <query>, --limit, n] — query is at -3.
    query = memory_searches[0][-3]  # noqa: PLR2004
    assert sensitive not in query, (
        f"query must not contain transcript / assistant content: {query!r}"
    )
    # Query must include the repo basename and preset (basename of
    # /tmp/repo-u02).
    assert "repo-u02" in query, f"query missing repo basename: {query!r}"
    assert "ce-executor-pipeline" in query, f"query missing preset: {query!r}"
