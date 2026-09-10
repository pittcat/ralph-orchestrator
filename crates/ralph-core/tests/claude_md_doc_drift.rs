//! Plan 2026-09-09-0917 Unit U9 acceptance tests: CLAUDE.md /
//! AGENTS.md migration table-name + path drift.
//!
//! Locks the migration table inventory documented in
//! `CLAUDE.md:142-143` (mirrored to `AGENTS.md:142-143`):
//!
//! - v19 → `dag_checkout_intents` (per-Unit capacity slot reservation)
//! - v20 → `dag_terminal_deliveries` (exactly-once close acknowledgement)
//! - v21 → `dag_registration_evidence` + `dag_approval_evidence`
//!   (correction reentry persistence)
//! - v22 → `dag_correction_requests` (typed artifact_refs column)
//! - v23 → `dag_integration_failures` (`dag_runtime` virtual target
//!   routing table)
//!
//! and the migrations directory path
//! `crates/ralph-core/src/supervisor/migrations/`.
//!
//! Both CLAUDE.md and AGENTS.md must remain byte-identical (see
//! "CLAUDE.md 与 AGENTS.md 同步规则" hard rule); divergence is
//! asserted via `diff`.

use std::path::PathBuf;
use std::process::Command;

/// Repository root used for both CLAUDE.md / AGENTS.md discovery
/// and the `diff` invariant check. We resolve it relative to this
/// test file's location (`crates/ralph-core/tests/`).
fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // CARGO_MANIFEST_DIR points at crates/ralph-core; the repo
    // root is two levels up.
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("repo_root: expected <repo>/crates/ralph-core/Cargo.toml")
        .to_path_buf()
}

/// Read the full contents of `relative` (resolved against the repo
/// root) as a `String`. Panics with a useful message on miss so the
/// test failure points at the missing file.
fn read_repo_file(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "read_repo_file({relative}): {err} (resolved to {})",
            path.display()
        )
    })
}

/// Returns `Vec<usize>` of 1-based line numbers in `body` that
/// contain `needle`. Used to pin both presence and absence.
fn lines_containing(body: &str, needle: &str) -> Vec<usize> {
    body.lines()
        .enumerate()
        .filter_map(|(idx, line)| line.contains(needle).then_some(idx + 1))
        .collect()
}

/// Run `diff -u` against two repo-relative paths. Empty `Ok(())`
/// means the files are byte-identical; `Err(diff_output)` carries
/// the unified diff for debugging.
fn diff_repo_files(a: &str, b: &str) -> Result<(), String> {
    let out = Command::new("diff")
        .arg("-u")
        .arg(a)
        .arg(b)
        .current_dir(repo_root())
        .output()
        .expect("spawn diff");
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{}\n--- exit: {:?}\n",
            String::from_utf8_lossy(&out.stdout),
            out.status.code()
        ))
    }
}

/// Real table names exposed by v19-v23.sql `CREATE TABLE`
/// statements. Keep this list in lockstep with the migrations.
const REAL_TABLE_NAMES: &[(&str, &str)] = &[
    ("dag_checkout_intents", "v19"),
    ("dag_terminal_deliveries", "v20"),
    ("dag_registration_evidence", "v21"),
    ("dag_approval_evidence", "v21"),
    ("dag_correction_requests", "v22"),
    ("dag_integration_failures", "v23"),
];

/// Wrong / placeholder table names previously (or potentially)
/// leaked into the documentation. Each must be absent from
/// CLAUDE.md / AGENTS.md.
const WRONG_TABLE_NAMES: &[(&str, &str)] = &[
    ("dag_capacity_reservations", "v19 placeholder"),
    ("dag_units_completion_ack", "v20 placeholder"),
    ("dag_corrected_state_machine", "v21 placeholder"),
    ("dag_artifact_refs", "v22 placeholder"),
    ("dag_runtime_virtual_target", "v23 placeholder"),
];

const CORRECT_MIGRATIONS_PATH: &str = "crates/ralph-core/src/supervisor/migrations/";
const WRONG_MIGRATIONS_PATH: &str =
    "crates/ralph-cli/src/loop_runner/dag_scheduler/store/migrations/";

/// Asserts that the doc uses the *real* migrations directory path
/// and does NOT mention the non-existent
/// `dag_scheduler/store/migrations/` shim. Required by U9 S2.
#[test]
fn claude_md_documents_correct_migration_path() {
    let body = read_repo_file("CLAUDE.md");
    assert!(
        body.contains(CORRECT_MIGRATIONS_PATH),
        "CLAUDE.md must reference real migrations path \
         {CORRECT_MIGRATIONS_PATH:?}; lines: {:?}",
        lines_containing(&body, CORRECT_MIGRATIONS_PATH),
    );
    assert!(
        !body.contains(WRONG_MIGRATIONS_PATH),
        "CLAUDE.md must NOT reference non-existent migrations path \
         {WRONG_MIGRATIONS_PATH:?}; lines: {:?}",
        lines_containing(&body, WRONG_MIGRATIONS_PATH),
    );
}

/// Asserts each of the six real v19-v23 DAG tables is mentioned in
/// CLAUDE.md, and each of the five placeholder names is absent.
/// Required by U9 S3.
#[test]
fn claude_md_documents_5_real_table_names() {
    let body = read_repo_file("CLAUDE.md");

    for (table, version) in REAL_TABLE_NAMES {
        let lines = lines_containing(&body, table);
        assert!(
            !lines.is_empty(),
            "CLAUDE.md must document real {version} table \
             {table:?}; lines: {lines:?}",
        );
    }

    for (table, label) in WRONG_TABLE_NAMES {
        let lines = lines_containing(&body, table);
        assert!(
            lines.is_empty(),
            "CLAUDE.md must NOT mention placeholder {label} \
             table {table:?}; lines: {lines:?}",
        );
    }
}

/// Mirror of the table-name check for AGENTS.md — CLAUDE.md /
/// AGENTS.md must agree on every documented table.
#[test]
fn agents_md_documents_5_real_table_names() {
    let body = read_repo_file("AGENTS.md");

    for (table, version) in REAL_TABLE_NAMES {
        let lines = lines_containing(&body, table);
        assert!(
            !lines.is_empty(),
            "AGENTS.md must document real {version} table \
             {table:?}; lines: {lines:?}",
        );
    }

    for (table, label) in WRONG_TABLE_NAMES {
        let lines = lines_containing(&body, table);
        assert!(
            lines.is_empty(),
            "AGENTS.md must NOT mention placeholder {label} \
             table {table:?}; lines: {lines:?}",
        );
    }
}

/// Mirror of the path check for AGENTS.md.
#[test]
fn agents_md_documents_correct_migration_path() {
    let body = read_repo_file("AGENTS.md");
    assert!(
        body.contains(CORRECT_MIGRATIONS_PATH),
        "AGENTS.md must reference real migrations path \
         {CORRECT_MIGRATIONS_PATH:?}; lines: {:?}",
        lines_containing(&body, CORRECT_MIGRATIONS_PATH),
    );
    assert!(
        !body.contains(WRONG_MIGRATIONS_PATH),
        "AGENTS.md must NOT reference non-existent migrations path \
         {WRONG_MIGRATIONS_PATH:?}; lines: {:?}",
        lines_containing(&body, WRONG_MIGRATIONS_PATH),
    );
}

/// CLAUDE.md and AGENTS.md must be byte-identical (sync rule).
/// `diff -u` must return 0.
#[test]
fn claude_md_and_agents_md_are_byte_identical() {
    if let Err(diff_output) = diff_repo_files("CLAUDE.md", "AGENTS.md") {
        panic!(
            "CLAUDE.md and AGENTS.md drifted apart; `diff -u` output:\n{diff_output}"
        );
    }
}