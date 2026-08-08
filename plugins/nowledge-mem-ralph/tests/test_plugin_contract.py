"""Contract tests for the Ralph-dedicated Nowledge Mem plugin (U1).

These tests lock the *structural* contract of
``plugins/nowledge-mem-ralph`` (plan ``2026-08-07-010``):

* The plugin manifest exists, is named ``nowledge-mem-ralph`` at version
  ``0.1.0``, and is exposed by the root marketplace through a
  name-based lookup (never a positional ``plugins[0]`` assumption).
* The plugin declares **no lifecycle entrypoints**: no ``hooks`` key in
  the manifest, no ``hooks/`` directory, and no shipped script files —
  only markdown resources plus the manifest.
* The query surface is bounded and read-only: ``search`` runs a JSON
  memory search capped at 5 results; ``status`` runs exactly one
  ``nmem --json status``; optional thread tracing is bounded
  (``t search --limit 5``, ``t show --limit 8 --content-limit 1200``)
  and only reachable behind an explicit trace-the-source condition.
* An empty search query stops with usage help and never reaches nmem.
* Every fenced ``nmem`` invocation in commands/skills is on the read
  allowlist; all memory/thread write commands and Working Memory reads
  are on the denylist.
* The design document (``.ralph/specs``) and the plugin README cover
  the stable section set required by R6/R7.

The assertions are deliberately structural (manifest fields, argv
parsed out of shell code blocks, heading coverage) rather than textual:
prose may evolve, the capability surface may not.
"""

from __future__ import annotations

import json
import re
import shlex
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
PLUGIN_DIR = ROOT / "plugins" / "nowledge-mem-ralph"
MANIFEST = PLUGIN_DIR / ".claude-plugin" / "plugin.json"
MARKETPLACE = ROOT / ".claude-plugin" / "marketplace.json"
DESIGN_DOC = ROOT / ".ralph" / "specs" / "nowledge-mem-ralph-plugin-design.md"
README = PLUGIN_DIR / "README.md"
SEARCH_COMMAND = PLUGIN_DIR / "commands" / "search.md"
STATUS_COMMAND = PLUGIN_DIR / "commands" / "status.md"
SKILL_DOC = PLUGIN_DIR / "skills" / "search-memory" / "SKILL.md"

PLUGIN_NAME = "nowledge-mem-ralph"
PLUGIN_VERSION = "0.2.0"
MARKETPLACE_SOURCE = "./plugins/nowledge-mem-ralph"
ROOT_PLUGIN_NAME = "ralph-orchestrator"

# Lifecycle events the plugin MUST register on its hooks manifest.
LIFECYCLE_HOOK_EVENTS = (
    "SessionStart",
    "Stop",
    "SubagentStop",
)

# Hooks the plugin MUST NOT register (reserved for U05 and beyond, or
# deliberately out of scope).
DISALLOWED_HOOK_EVENTS = (
    "PreCompact",
    "UserPromptSubmit",
    "SessionEnd",
)

# Read-only nmem subcommand surface allowed anywhere in the plugin
# (short and long CLI forms). Pairs are (sub1, sub2-or-empty) after
# stripping flags.
ALLOWED_NMEM_SUBCOMMANDS = {
    ("m", "search"),
    ("memories", "search"),
    ("t", "search"),
    ("threads", "search"),
    ("t", "show"),
    ("threads", "show"),
    ("status", ""),
}

# Explicit write / Working-Memory denylist (R8). Anything here must
# never appear as an nmem subcommand pair.
DENIED_NMEM_SUBCOMMANDS = {
    ("m", "add"),
    ("m", "update"),
    ("m", "delete"),
    ("memories", "add"),
    ("memories", "update"),
    ("memories", "delete"),
    ("t", "create"),
    ("t", "append"),
    ("t", "save"),
    ("t", "distill"),
    ("threads", "create"),
    ("threads", "append"),
    ("threads", "save"),
    ("threads", "distill"),
    ("wm", "read"),
    ("wm", "add"),
    ("wm", "update"),
    ("wm", "delete"),
}

# Stable heading keys the design document must cover (R6). Checked
# case-insensitively against markdown heading lines; prose around them
# may evolve.
REQUIRED_DESIGN_HEADING_KEYS = (
    "目标",
    "非目标",
    "边界",
    "组件",
    "合同",
    "信任",
    "scope",
    "installer",
    "隐私",
    "版本",
    "追踪",
)

# Stable coverage keys the README must contain somewhere in its body
# (R7). Operators must be able to select, install, verify, use,
# troubleshoot and uninstall without reading source. The lifecycle key
# (added in U01 / 0.2.0) covers the documented bounded save-memory
# lifecycle contract.
REQUIRED_README_KEYS = (
    "选型",
    "前置",
    "安装",
    "验证",
    "search",
    "status",
    "排障",
    "卸载",
    "隐私",
    "lifecycle",
    "适配",
)


# --- helpers ----------------------------------------------------------------


def _load_manifest() -> dict:
    assert MANIFEST.is_file(), (
        f"plugin manifest does not exist: {MANIFEST.relative_to(ROOT)} "
        "(the dedicated plugin package has not been created yet)"
    )
    return json.loads(MANIFEST.read_text(encoding="utf-8"))


def _load_marketplace() -> dict:
    assert MARKETPLACE.is_file(), f"missing marketplace manifest: {MARKETPLACE}"
    return json.loads(MARKETPLACE.read_text(encoding="utf-8"))


def _marketplace_entry_by_name(name: str) -> dict:
    """Name-based marketplace lookup — never positional."""
    data = _load_marketplace()
    entries = [
        entry
        for entry in data.get("plugins", [])
        if entry.get("name") == name
    ]
    assert entries, (
        f"marketplace has no plugin entry with name={name!r}; "
        f"known names={[e.get('name') for e in data.get('plugins', [])]}"
    )
    assert len(entries) == 1, f"marketplace has duplicate entries for {name!r}"
    return entries[0]


def _frontmatter(text: str) -> dict[str, str]:
    """Parse a flat ``key: value`` YAML frontmatter block (--- delimited)."""
    match = re.match(r"\A---\s*\n(.*?)\n---\s*\n", text, re.DOTALL)
    assert match, "document does not start with a --- frontmatter block"
    fields: dict[str, str] = {}
    for line in match.group(1).splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        key, sep, value = line.partition(":")
        assert sep, f"unparseable frontmatter line: {line!r}"
        fields[key.strip()] = value.strip().strip("'\"")
    return fields


def _shell_blocks(text: str) -> list[str]:
    """Return the contents of fenced ```bash / ```sh code blocks."""
    return re.findall(r"```(?:bash|sh)\s*\n(.*?)```", text, re.DOTALL)


def _nmem_argvs(text: str) -> list[list[str]]:
    """Parse every ``nmem`` invocation out of the shell blocks of a doc."""
    argvs: list[list[str]] = []
    for block in _shell_blocks(text):
        for line in block.splitlines():
            stripped = line.strip()
            if not stripped or stripped.startswith("#"):
                continue
            try:
                argv = shlex.split(stripped)
            except ValueError as exc:
                raise AssertionError(f"unparseable shell line {stripped!r}: {exc}")
            if argv and Path(argv[0]).name == "nmem":
                argvs.append(argv)
    return argvs


def _subcommand_pair(argv: list[str]) -> tuple[str, str]:
    """Reduce an nmem argv to its (sub1, sub2) pair, stripping flags."""
    tokens = [tok for tok in argv[1:] if not tok.startswith("-")]
    sub1 = tokens[0] if tokens else ""
    sub2 = tokens[1] if len(tokens) > 1 else ""
    return (sub1, sub2)


def _sections(text: str) -> list[tuple[str, str]]:
    """Split markdown into (heading-text, body-until-next-heading) pairs."""
    parts: list[tuple[str, str]] = []
    heading_re = re.compile(r"^#{1,6}\s+(.*)$", re.MULTILINE)
    matches = list(heading_re.finditer(text))
    for index, match in enumerate(matches):
        end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        parts.append((match.group(1).strip(), text[match.end():end]))
    return parts


def _flag_value(argv: list[str], flag: str) -> str | None:
    for index, token in enumerate(argv):
        if token == flag and index + 1 < len(argv):
            return argv[index + 1]
        if token.startswith(flag + "="):
            return token.split("=", 1)[1]
    return None


def _plugin_content_files() -> list[Path]:
    """All shipped content files, excluding this test suite and bytecode.

    ``__pycache__`` directories are generated by the test runner and are
    already covered by the root gitignore; treating them as shipped
    content would flag every Python import the test does as a contract
    violation, which is the opposite of what this check exists for.
    """
    excluded_parts = ("tests", "__pycache__")
    return sorted(
        path
        for path in PLUGIN_DIR.rglob("*")
        if path.is_file()
        and not any(part in excluded_parts for part in path.relative_to(PLUGIN_DIR).parts)
    )


# --- manifest + marketplace -------------------------------------------------


def test_manifest_and_marketplace_expose_dedicated_plugin_by_name() -> None:
    """R1/R3 — manifest fields and name-based marketplace exposure."""
    manifest = _load_manifest()
    assert manifest.get("name") == PLUGIN_NAME, (
        f"plugin manifest name must be {PLUGIN_NAME!r}, got {manifest.get('name')!r}"
    )
    assert manifest.get("version") == PLUGIN_VERSION, (
        f"plugin manifest version must be {PLUGIN_VERSION!r}, "
        f"got {manifest.get('version')!r}"
    )
    assert str(manifest.get("description", "")).strip(), (
        "plugin manifest must carry a non-empty description"
    )

    entry = _marketplace_entry_by_name(PLUGIN_NAME)
    assert entry.get("source") == MARKETPLACE_SOURCE, (
        f"marketplace source for {PLUGIN_NAME} must be pinned to "
        f"{MARKETPLACE_SOURCE!r}, got {entry.get('source')!r}"
    )
    resolved = (MARKETPLACE.parent.parent / entry["source"]).resolve()
    assert resolved == PLUGIN_DIR.resolve(), (
        f"marketplace source must resolve to the plugin dir: {resolved}"
    )

    # R3 regression — the pre-existing root plugin keeps its skill set.
    root_entry = _marketplace_entry_by_name(ROOT_PLUGIN_NAME)
    assert set(root_entry.get("skills", [])) == {
        "./skills/ralph-e2e-bootstrap",
        "./skills/ralph-preset-author",
        "./skills/ralph-preset-review",
        "./skills/ralph-project-bootstrap",
        "./skills/ralph-run-diagnosis",
        "./skills/ralph-task-discovery",
    }, "root plugin skill set changed while adding the dedicated entry"


def test_plugin_resource_set_is_complete() -> None:
    """U1 resource set — manifest, two commands, one skill, README."""
    for path in (MANIFEST, SEARCH_COMMAND, STATUS_COMMAND, SKILL_DOC, README):
        assert path.is_file(), f"missing plugin resource: {path.relative_to(ROOT)}"


# --- lifecycle entrypoints ----------------------------------------------------


def test_plugin_has_lifecycle_hooks() -> None:
    """U01 — the plugin declares SessionStart and Stop hooks.

    U01 ships the lifecycle skeleton: SessionStart + Stop. SubagentStop
    is added in U05. The manifest MUST point at ``hooks/hooks.json`` and
    that file MUST register exactly the events in ``LIFECYCLE_HOOK_EVENTS``
    (no more, no less) so the plugin's capability surface stays auditable.
    """
    manifest = _load_manifest()
    assert "hooks" in manifest, (
        "plugin manifest must declare a 'hooks' entry pointing at "
        "hooks/hooks.json (U01 lifecycle foundation)"
    )
    hooks_ref = manifest["hooks"]
    # Manifest may use either the shorthand string (path) or the full
    # object form. Both are valid in the Claude Code plugin spec.
    if isinstance(hooks_ref, str):
        hooks_path = PLUGIN_DIR / hooks_ref
    else:
        hooks_path = PLUGIN_DIR / "hooks" / "hooks.json"
    assert hooks_path.is_file(), (
        f"hooks manifest missing on disk: {hooks_path.relative_to(ROOT)}"
    )
    hooks_payload = json.loads(hooks_path.read_text(encoding="utf-8"))
    declared = set(hooks_payload.get("hooks", {}).keys())
    expected = set(LIFECYCLE_HOOK_EVENTS)
    forbidden = set(DISALLOWED_HOOK_EVENTS)
    assert expected.issubset(declared), (
        f"hooks manifest is missing required events: "
        f"missing={expected - declared}; declared={sorted(declared)}"
    )
    assert not (declared & forbidden), (
        f"hooks manifest declares events reserved for later units: "
        f"{declared & forbidden}"
    )

    # The plugin ships Python runtime under scripts/ (U01) and the
    # hooks manifest itself. Other content files are still restricted
    # to markdown/json so the plugin's blast radius stays small.
    for path in _plugin_content_files():
        relative = path.relative_to(PLUGIN_DIR)
        suffix = path.suffix
        assert suffix in {".md", ".json", ".py"}, (
            f"unexpected non-markdown/json/py content file in plugin: {relative}"
        )
        if suffix == ".json":
            assert relative in {
                Path(".claude-plugin") / "plugin.json",
                Path("hooks") / "hooks.json",
            }, f"only the manifest json and hooks.json may ship as json: {relative}"
        if suffix == ".py":
            parts = relative.parts
            assert parts and parts[0] == "scripts", (
                f"python files must live under scripts/: {relative}"
            )

    # Commands + skills must remain read-only capability docs and must
    # NOT register or invoke lifecycle hook events as tokens (those
    # belong to hooks/hooks.json). We match event names only when
    # they appear as keys/strings that look like hook registration
    # (e.g. `"SessionStart":`, `event: Stop`), so user-facing prose
    # like "**Stop.** Do not retry…" remains free to use the verb.
    _HOOK_KEY_RE = re.compile(
        r'(?P<q>["\']?)(?P<event>'
        + "|".join(re.escape(event) for event in LIFECYCLE_HOOK_EVENTS)
        + r')(?P=q)\s*:',
    )
    for path in [MANIFEST, *_all_capability_docs()]:
        relative = path.relative_to(PLUGIN_DIR)
        text = path.read_text(encoding="utf-8")
        match = _HOOK_KEY_RE.search(text)
        assert match is None, (
            f"hook event {match.group('event')!r} registered by "
            f"registerable plugin content {relative}; lifecycle hooks "
            "belong in hooks/hooks.json, not in commands/skills/manifest prose"
        )


# --- search command contract --------------------------------------------------


def test_search_contract_is_bounded_and_read_only() -> None:
    """R2/R8/S3 — bounded JSON memory search, conditional thread tracing."""
    assert SEARCH_COMMAND.is_file(), f"missing {SEARCH_COMMAND}"
    meta = _frontmatter(SEARCH_COMMAND.read_text(encoding="utf-8"))
    assert meta.get("description"), "search command needs a description"
    assert "<query>" in meta.get("argument-hint", ""), (
        "search command must declare a required <query> argument"
    )

    text = SEARCH_COMMAND.read_text(encoding="utf-8")
    argvs = _nmem_argvs(text)
    assert argvs, "search command must document at least one nmem invocation"

    memory_searches = [
        argv for argv in argvs if _subcommand_pair(argv)[0] in {"m", "memories"}
    ]
    assert memory_searches, "search command must document the memory search call"
    for argv in memory_searches:
        assert "--json" in argv, f"memory search must emit JSON: {argv}"
        limit = _flag_value(argv, "--limit") or _flag_value(argv, "-n")
        assert limit == "5", (
            f"memory search must be capped at --limit 5, got {limit!r}: {argv}"
        )

    for argv in argvs:
        sub1, sub2 = _subcommand_pair(argv)
        if sub1 in {"t", "threads"} and sub2 == "search":
            limit = _flag_value(argv, "--limit") or _flag_value(argv, "-n")
            assert limit is not None and int(limit) <= 5, (
                f"thread search must be bounded to <=5 results: {argv}"
            )
        if sub1 in {"t", "threads"} and sub2 == "show":
            limit = _flag_value(argv, "--limit") or _flag_value(argv, "-n")
            content_limit = _flag_value(argv, "--content-limit")
            offset = _flag_value(argv, "--offset")
            assert limit is not None and int(limit) <= 8, (
                f"thread show must read <=8 messages per page: {argv}"
            )
            assert content_limit is not None and int(content_limit) <= 1200, (
                f"thread show must truncate content to <=1200 chars: {argv}"
            )
            assert offset is not None and int(offset) >= 0, (
                f"thread show must pin an explicit --offset: {argv}"
            )

    # Thread tracing is conditional: the doc must anchor it to tracing a
    # source conversation, not present it as a default action.
    if any(_subcommand_pair(argv)[0] in {"t", "threads"} for argv in argvs):
        assert "source_thread" in text or "原对话" in text, (
            "thread tracing must be gated on tracing the original "
            "conversation (source_thread / 原对话)"
        )

    for argv in argvs:
        assert _subcommand_pair(argv) in ALLOWED_NMEM_SUBCOMMANDS, (
            f"search command documents a non-allowlisted nmem call: {argv}"
        )


def test_search_empty_query_stops_before_nmem() -> None:
    """R2/S4 — empty query shows usage and never reaches nmem."""
    text = SEARCH_COMMAND.read_text(encoding="utf-8")
    sections = _sections(text)
    empty_sections = [
        body
        for heading, body in sections
        if "空" in heading or "empty" in heading.lower()
    ]
    assert empty_sections, (
        "search command must document the empty-query branch under a "
        "dedicated heading (空输入/empty)"
    )
    for body in empty_sections:
        assert not _nmem_argvs(body), (
            "the empty-query branch must not invoke nmem"
        )
        lowered = body.lower()
        assert "用法" in body or "usage" in lowered, (
            "the empty-query branch must tell the operator to show usage"
        )


# --- status command contract ---------------------------------------------------


def test_status_contract_is_single_read_only_call() -> None:
    """R2/R8/S5 — exactly one `nmem --json status`, no fallback commands."""
    assert STATUS_COMMAND.is_file(), f"missing {STATUS_COMMAND}"
    text = STATUS_COMMAND.read_text(encoding="utf-8")
    argvs = _nmem_argvs(text)
    assert argvs, "status command must document its nmem invocation"
    for argv in argvs:
        assert argv == ["nmem", "--json", "status"], (
            "status command may only document `nmem --json status`; "
            f"found {argv} — failures must be reported as-is, never "
            "answered with a second nmem subcommand"
        )

    headings = [heading for heading, _ in _sections(text)]
    assert any(
        "失败" in heading or "错误" in heading
        or "failure" in heading.lower() or "error" in heading.lower()
        for heading in headings
    ), (
        "status command must document failure handling under a dedicated heading"
    )


# --- capability allow/deny across the whole plugin ------------------------------


def _all_capability_docs() -> list[Path]:
    docs = sorted(PLUGIN_DIR.glob("commands/*.md"))
    docs += sorted(PLUGIN_DIR.glob("skills/*/SKILL.md"))
    return docs


def test_capability_allowlist_is_read_only_json() -> None:
    """R8 — every documented nmem call is a bounded read-only JSON query."""
    docs = _all_capability_docs()
    assert docs, "plugin ships no command/skill capability documents"
    for doc in docs:
        text = doc.read_text(encoding="utf-8")
        for argv in _nmem_argvs(text):
            assert "--json" in argv, (
                f"{doc.relative_to(ROOT)}: nmem calls must use --json: {argv}"
            )
            assert _subcommand_pair(argv) in ALLOWED_NMEM_SUBCOMMANDS, (
                f"{doc.relative_to(ROOT)}: non-allowlisted nmem call: {argv}"
            )


def test_capability_denylist_has_no_write_or_working_memory() -> None:
    """R8/S2 — no memory/thread writes, no Working Memory access."""
    docs = _all_capability_docs()
    assert docs, "plugin ships no command/skill capability documents"
    for doc in docs:
        text = doc.read_text(encoding="utf-8")
        for argv in _nmem_argvs(text):
            pair = _subcommand_pair(argv)
            assert pair not in DENIED_NMEM_SUBCOMMANDS, (
                f"{doc.relative_to(ROOT)} documents a forbidden write/WM "
                f"call: {argv}"
            )
        lowered = text.lower()
        assert "wm read" not in lowered and "working memory" not in lowered, (
            f"{doc.relative_to(ROOT)} must not reference Working Memory reads"
        )


def test_skill_metadata_is_read_only_search() -> None:
    """R2 — the shipped skill is the read-only search-memory skill."""
    assert SKILL_DOC.is_file(), f"missing {SKILL_DOC}"
    meta = _frontmatter(SKILL_DOC.read_text(encoding="utf-8"))
    assert meta.get("name") == "search-memory", (
        f"skill name must be search-memory, got {meta.get('name')!r}"
    )
    assert meta.get("description"), "skill needs a description"


# --- documentation coverage -----------------------------------------------------


def _markdown_headings(text: str) -> list[str]:
    return [
        match.group(1).strip()
        for match in re.finditer(r"^#{1,6}\s+(.*)$", text, re.MULTILINE)
    ]


def test_design_and_readme_cover_required_contracts() -> None:
    """R6/R7/S6 — design doc and README cover the stable section set."""
    assert DESIGN_DOC.is_file(), (
        f"missing design document: {DESIGN_DOC.relative_to(ROOT)}"
    )
    design_text = DESIGN_DOC.read_text(encoding="utf-8")
    design_headings = [heading.lower() for heading in _markdown_headings(design_text)]
    for key in REQUIRED_DESIGN_HEADING_KEYS:
        assert any(key.lower() in heading for heading in design_headings), (
            f"design document is missing a heading covering {key!r}; "
            f"headings={design_headings}"
        )

    assert README.is_file(), f"missing plugin README: {README}"
    readme_text = README.read_text(encoding="utf-8")
    lowered = readme_text.lower()
    for key in REQUIRED_README_KEYS:
        assert key.lower() in lowered, (
            f"plugin README must cover {key!r} so operators never need "
            "to read source"
        )
