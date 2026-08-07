"""Public installer contract tests (Unit 1).

These tests lock the contract between the public catalog
(``skills/install.py`` + ``.claude-plugin/marketplace.json``) and the set of
directories under ``skills/`` that ship as installable skills.

The contract is intentionally behavioural rather than textual:

* The single source of truth for *which* skills are public lives in the
  installer (``PUBLIC_SKILLS``). The marketplace manifest mirrors that list
  so plugin hosts can advertise the same set; drift between the two fails
  this suite.
* ``discover_skills`` must surface *only* the catalog members — adding a
  bare ``SKILL.md`` outside the catalog must not leak into install/listing.
* Dry-run installs do not touch disk.
* Force install overwrites existing copies; default install keeps user copy.
* ``--prune`` only removes skills that are not part of the requested set;
  shared references for preset skills remain readable.
* Unknown skill names fail closed with the canonical error.
* Duplicate explicit requests collapse to one install.
* ``ralph-project-bootstrap`` is present in the catalog, marketplace and
  on disk with a non-empty ``SKILL.md`` and matching agent metadata.
* ``ralph-task-discovery`` is present in the catalog (6 public skills),
  marketplace and on disk, installs as a physical symlink-free copy into
  custom, local (paired ``.claude`` + ``.agents``) and global targets.
"""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

import install  # type: ignore[import-not-found]  # added via conftest sys.path


ROOT = Path(__file__).resolve().parents[2]
SKILLS_DIR = ROOT / "skills"
MARKETPLACE = ROOT / ".claude-plugin" / "marketplace.json"
README = SKILLS_DIR / "README.md"
PROJECT_BOOTSTRAP = SKILLS_DIR / "ralph-project-bootstrap"
SKILL_DOC = PROJECT_BOOTSTRAP / "SKILL.md"
AGENT_METADATA = PROJECT_BOOTSTRAP / "agents" / "openai.yaml"
TASK_DISCOVERY = SKILLS_DIR / "ralph-task-discovery"
TASK_DISCOVERY_SKILL_DOC = TASK_DISCOVERY / "SKILL.md"
TASK_DISCOVERY_AGENT_METADATA = TASK_DISCOVERY / "agents" / "openai.yaml"


def _marketplace_skill_names() -> set[str]:
    """Skill names advertised by the root plugin entry (SSOT mirror).

    Name-based lookup — the marketplace carries multiple plugin entries
    (e.g. ``nowledge-mem-ralph``), so positional ``plugins[0]`` or a
    cross-plugin skill union would conflate distinct plugins.
    """
    data = json.loads(MARKETPLACE.read_text(encoding="utf-8"))
    entries = [
        plugin
        for plugin in data.get("plugins", [])
        if plugin.get("name") == "ralph-orchestrator"
    ]
    assert len(entries) == 1, (
        "marketplace must carry exactly one ralph-orchestrator entry"
    )
    return {Path(skill).name for skill in entries[0].get("skills", [])}


@pytest.fixture
def fresh_target(tmp_path: Path) -> Path:
    """Return a clean custom directory that should never exist beforehand."""
    target = tmp_path / "skills-target"
    assert not target.exists()
    return target


@pytest.fixture
def catalog_names() -> set[str]:
    """Names currently exposed by ``install.PUBLIC_SKILLS``."""
    return set(install.PUBLIC_SKILLS)


# --- catalog / marketplace parity -----------------------------------------


def test_public_skills_constant_matches_marketplace(catalog_names: set[str]) -> None:
    """The marketplace manifest must list every public skill exactly once."""
    advertised = _marketplace_skill_names()
    assert advertised == catalog_names, (
        "marketplace manifest drifted from install.PUBLIC_SKILLS: "
        f"missing={catalog_names - advertised} extra={advertised - catalog_names}"
    )


def test_raph_project_bootstrap_in_catalog(catalog_names: set[str]) -> None:
    """``ralph-project-bootstrap`` must be in the catalog from day one."""
    assert "ralph-project-bootstrap" in catalog_names


def test_raph_project_bootstrap_in_marketplace() -> None:
    """The marketplace manifest must advertise the new skill."""
    assert "ralph-project-bootstrap" in _marketplace_skill_names()


def test_raph_project_bootstrap_on_disk() -> None:
    """The new skill must ship with at least a SKILL.md and agent metadata."""
    assert SKILL_DOC.is_file(), f"missing {SKILL_DOC}"
    assert SKILL_DOC.read_text(encoding="utf-8").strip(), "SKILL.md is empty"
    assert AGENT_METADATA.is_file(), f"missing {AGENT_METADATA}"
    agent_text = AGENT_METADATA.read_text(encoding="utf-8")
    assert "ralph-project-bootstrap" in agent_text, (
        "agent metadata must reference the new skill name"
    )


def test_raph_project_bootstrap_still_installable() -> None:
    """Regression — ``ralph-project-bootstrap`` must remain installable
    via ``select_skills`` after the ``ralph-hats`` deletion (U8)."""
    available = install.discover_skills(SKILLS_DIR)
    selected = install.select_skills(available, ["ralph-project-bootstrap"])
    assert [s.name for s in selected] == ["ralph-project-bootstrap"]


@pytest.mark.parametrize("banned", ["ralph-hats"])
def test_public_skills_excludes_banned(banned: str) -> None:
    """The public catalog must not include ``ralph-hats`` (U8 deletion)."""
    assert banned not in install.PUBLIC_SKILLS


@pytest.mark.parametrize("banned_dir", [SKILLS_DIR / "ralph-hats"])
def test_banned_skill_dir_not_shipped(banned_dir: Path) -> None:
    """The deleted ``ralph-hats`` directory must not be present under
    ``skills/`` (U8 atomic deletion)."""
    assert not banned_dir.exists(), f"{banned_dir} must be deleted"


def test_ralph_hats_not_installable() -> None:
    """``select_skills`` must reject ``ralph-hats`` with the canonical
    'unknown skill' message (U8 deletion guard)."""
    available = install.discover_skills(SKILLS_DIR)
    with pytest.raises(install.InstallError) as excinfo:
        install.select_skills(available, ["ralph-hats"])
    assert "unknown skill 'ralph-hats'" in str(excinfo.value)
    assert "Known public skills" in str(excinfo.value)
    # And the canonical error must NOT mention the now-deleted
    # ``ralph-hats`` skill in the available set: it is no longer a
    # known public skill.
    assert "ralph-hats" not in install.PUBLIC_SKILLS


# --- ralph-task-discovery catalog parity + installability (U5) -----------


def test_task_discovery_in_catalog(catalog_names: set[str]) -> None:
    """``ralph-task-discovery`` must be a public catalog member."""
    assert "ralph-task-discovery" in catalog_names


def test_task_discovery_in_marketplace() -> None:
    """The marketplace manifest must advertise the task-discovery skill."""
    assert "ralph-task-discovery" in _marketplace_skill_names()


def test_catalog_and_marketplace_both_have_six_skills(catalog_names: set[str]) -> None:
    """Catalog ↔ marketplace parity must hold at the new size of 6."""
    advertised = _marketplace_skill_names()
    assert advertised == catalog_names, (
        f"missing={catalog_names - advertised} extra={advertised - catalog_names}"
    )
    assert len(catalog_names) == 6
    assert len(advertised) == 6


def test_readme_lists_every_catalog_skill(catalog_names: set[str]) -> None:
    """catalog ↔ README SSOT reconciliation: every public skill must be
    documented in ``skills/README.md``."""
    readme_text = README.read_text(encoding="utf-8")
    missing = {name for name in catalog_names if name not in readme_text}
    assert not missing, f"skills/README.md is missing catalog entries: {missing}"


def test_task_discovery_on_disk() -> None:
    """The skill must ship a non-empty SKILL.md and agent metadata."""
    assert TASK_DISCOVERY_SKILL_DOC.is_file(), f"missing {TASK_DISCOVERY_SKILL_DOC}"
    assert TASK_DISCOVERY_SKILL_DOC.read_text(encoding="utf-8").strip(), (
        "ralph-task-discovery SKILL.md is empty"
    )
    assert TASK_DISCOVERY_AGENT_METADATA.is_file(), (
        f"missing {TASK_DISCOVERY_AGENT_METADATA}"
    )
    assert TASK_DISCOVERY_AGENT_METADATA.read_text(encoding="utf-8").strip(), (
        "ralph-task-discovery agents/openai.yaml is empty"
    )


def test_task_discovery_selectable() -> None:
    """``select_skills`` must accept the new skill via discovery."""
    available = install.discover_skills(SKILLS_DIR)
    selected = install.select_skills(available, ["ralph-task-discovery"])
    assert [s.name for s in selected] == ["ralph-task-discovery"]


def test_task_discovery_custom_install_physical_no_symlinks(fresh_target: Path) -> None:
    """Custom install must physically copy the full skill, symlink-free."""
    result = _run(["--dir", str(fresh_target), "--force", "ralph-task-discovery"])
    assert result.returncode == 0, result.stderr
    installed = fresh_target / "ralph-task-discovery"
    assert installed.is_dir()
    assert (installed / "SKILL.md").is_file()
    assert (installed / "agents" / "openai.yaml").is_file()
    assert (installed / "references").is_dir() and any(
        (installed / "references").iterdir()
    )
    assert (installed / "scripts").is_dir() and any((installed / "scripts").iterdir())
    assert not any(path.is_symlink() for path in installed.rglob("*")), (
        "installed task-discovery tree must contain no symlinks"
    )
    assert "result:      installed" in result.stdout


def test_task_discovery_local_install_copies_to_both_targets(tmp_path: Path) -> None:
    """Default local install must copy into .claude/skills AND .agents/skills."""
    result = subprocess.run(
        [
            sys.executable,
            str(SKILLS_DIR / "install.py"),
            "--force",
            "ralph-task-discovery",
        ],
        capture_output=True,
        text=True,
        check=False,
        cwd=tmp_path,
    )
    assert result.returncode == 0, result.stderr
    for target in (tmp_path / ".claude" / "skills", tmp_path / ".agents" / "skills"):
        installed = target / "ralph-task-discovery"
        assert installed.is_dir(), f"missing install target {installed}"
        assert (installed / "SKILL.md").is_file()
        assert not any(path.is_symlink() for path in installed.rglob("*"))


def test_task_discovery_global_dry_run_no_write() -> None:
    """Global dry-run must print both absolute destinations without writing."""
    result = _run(["--global", "--dry-run", "ralph-task-discovery"])
    assert result.returncode == 0, result.stderr
    claude_dest = install.TARGET_GLOBAL / "ralph-task-discovery"
    agents_dest = install.TARGET_AGENTS_GLOBAL / "ralph-task-discovery"
    assert "Install mode: global" in result.stdout
    assert "Install targets (2):" in result.stdout
    assert f"destination: {claude_dest}" in result.stdout
    assert f"destination: {agents_dest}" in result.stdout
    assert result.stdout.count("result:      would install") == 2
    # Dry-run must not have materialised either destination.
    assert not claude_dest.exists()
    assert not agents_dest.exists()


# --- discover_skills selection --------------------------------------------


def test_discover_skills_filters_to_catalog(tmp_path: Path) -> None:
    """Discovery must hide any stray SKILL.md outside the catalog."""
    decoy = tmp_path / "not-in-catalog"
    decoy.mkdir()
    (decoy / "SKILL.md").write_text("# decoy\n", encoding="utf-8")
    # Mix the catalog with the decoy; only catalog entries should surface.
    catalog_root = tmp_path / "skills"
    catalog_root.mkdir()
    for name in install.PUBLIC_SKILLS:
        skill = catalog_root / name
        skill.mkdir()
        (skill / "SKILL.md").write_text("# stub\n", encoding="utf-8")
    (catalog_root / "decoy-candidate").mkdir()
    (catalog_root / "decoy-candidate" / "SKILL.md").write_text("# x\n", encoding="utf-8")
    discovered = install.discover_skills(catalog_root)
    assert set(discovered) == set(install.PUBLIC_SKILLS), (
        f"unexpected selection: extra={set(discovered) - set(install.PUBLIC_SKILLS)} "
        f"missing={set(install.PUBLIC_SKILLS) - set(discovered)}"
    )


def test_select_skills_rejects_unknown() -> None:
    """Unknown skill names must fail closed with the canonical message."""
    available = install.discover_skills(SKILLS_DIR)
    with pytest.raises(install.InstallError) as excinfo:
        install.select_skills(available, ["ralph-does-not-exist"])
    assert "unknown skill 'ralph-does-not-exist'" in str(excinfo.value)
    assert "Known public skills" in str(excinfo.value)


def test_select_skills_dedupes() -> None:
    """Duplicate explicit requests collapse to a single install."""
    available = install.discover_skills(SKILLS_DIR)
    selected = install.select_skills(available, ["ralph-preset-author", "ralph-preset-author"])
    assert [s.name for s in selected] == ["ralph-preset-author"]


def test_select_skills_default_returns_catalog(tmp_path: Path) -> None:
    """An empty request list returns every catalog entry, sorted by name."""
    catalog_root = tmp_path / "skills"
    catalog_root.mkdir()
    for name in install.PUBLIC_SKILLS:
        skill = catalog_root / name
        skill.mkdir()
        (skill / "SKILL.md").write_text("# stub\n", encoding="utf-8")
    available = install.discover_skills(catalog_root)
    selected = install.select_skills(available, [])
    assert [s.name for s in selected] == sorted(install.PUBLIC_SKILLS)


# --- dry-run/install/prune semantics --------------------------------------


def _run(argv: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SKILLS_DIR / "install.py"), *argv],
        capture_output=True,
        text=True,
        check=False,
        cwd=ROOT,
    )


def test_dry_run_does_not_write(fresh_target: Path) -> None:
    result = _run(["--dir", str(fresh_target), "--dry-run"])
    assert result.returncode == 0, result.stderr
    assert not fresh_target.exists() or not any(fresh_target.iterdir()), (
        "dry-run must not create files"
    )


def test_force_install_creates_skill(fresh_target: Path) -> None:
    result = _run(
        [
            "--dir",
            str(fresh_target),
            "--force",
            "ralph-project-bootstrap",
        ]
    )
    assert result.returncode == 0, result.stderr
    installed = fresh_target / "ralph-project-bootstrap"
    assert installed.is_dir()
    assert (installed / "SKILL.md").is_file()
    assert "Install mode: custom" in result.stdout
    assert "Install method: physical copy" in result.stdout
    assert "Selected skills (1):" in result.stdout
    assert "Install targets (1):" in result.stdout
    assert f"destination: {installed}" in result.stdout
    assert "result:      installed" in result.stdout
    assert not any(path.is_symlink() for path in installed.rglob("*"))
    source_skill_md = PROJECT_BOOTSTRAP / "SKILL.md"
    installed_skill_md = installed / "SKILL.md"
    assert source_skill_md.stat().st_ino != installed_skill_md.stat().st_ino, (
        "installed files must be independent copies, not hard links"
    )


def test_global_dry_run_prints_both_absolute_destinations() -> None:
    result = _run(["--global", "--dry-run", "ralph-project-bootstrap"])
    assert result.returncode == 0, result.stderr
    claude_dest = install.TARGET_GLOBAL / "ralph-project-bootstrap"
    agents_dest = install.TARGET_AGENTS_GLOBAL / "ralph-project-bootstrap"
    assert "Install mode: global" in result.stdout
    assert "Install targets (2):" in result.stdout
    assert f"destination: {claude_dest}" in result.stdout
    assert f"destination: {agents_dest}" in result.stdout
    assert result.stdout.count("result:      would") == 2


def test_prune_removes_unrequested(fresh_target: Path) -> None:
    """``--prune`` must only remove skills outside the requested set."""
    # Seed the target with a skill that is NOT in the request set.
    seed = fresh_target / "ralph-not-in-catalog"
    seed.mkdir(parents=True)
    (seed / "SKILL.md").write_text("# user-edited\n", encoding="utf-8")
    result = _run(
        [
            "--dir",
            str(fresh_target),
            "--force",
            "--prune",
            "ralph-project-bootstrap",
        ]
    )
    assert result.returncode == 0, result.stderr
    assert not seed.exists(), "prune must remove skills outside the request"
    assert (fresh_target / "ralph-project-bootstrap").is_dir()


def test_install_keeps_existing_without_force(fresh_target: Path) -> None:
    """Default install keeps an existing copy unless ``--force`` is supplied."""
    fresh_target.mkdir(parents=True)
    kept = fresh_target / "ralph-preset-author"
    kept.mkdir()
    sentinel = kept / "user-note.txt"
    sentinel.write_text("user edited me\n", encoding="utf-8")
    (kept / "SKILL.md").write_text("# user\n", encoding="utf-8")
    # Feed ``n`` over stdin so the interactive prompt resolves to "keep".
    result = subprocess.run(
        [sys.executable, str(SKILLS_DIR / "install.py"), "--dir", str(fresh_target), "ralph-preset-author"],
        input="n\n",
        capture_output=True,
        text=True,
        check=False,
        cwd=ROOT,
    )
    assert result.returncode == 0, result.stderr
    assert sentinel.read_text(encoding="utf-8") == "user edited me\n"


# --- preset shared references ---------------------------------------------


def test_preset_skills_keep_shared_references_readable(fresh_target: Path) -> None:
    """After install, preset skills must expose readable shared references."""
    result = _run(["--dir", str(fresh_target), "--force", "ralph-preset-author"])
    assert result.returncode == 0, result.stderr
    references = fresh_target / "ralph-preset-author" / "references"
    assert references.is_dir()
    assert any(references.iterdir()), "shared references must be installed"


# --- help / unknown input -------------------------------------------------


def test_unknown_skill_cli_fails() -> None:
    result = _run(["--dir", "/tmp/ralph-test-unused", "ralph-not-a-real-skill"])
    assert result.returncode != 0
    assert "unknown skill" in result.stderr
