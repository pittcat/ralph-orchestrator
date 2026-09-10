//! Acceptance tests for the injected agent skill docs
//! (`crates/ralph-core/data/ralph-tools.md` and
//! `crates/ralph-core/data/ralph-tools-emit.md`) staying in lockstep
//! with the real DAG runtime contract.
//!
//! Three invariants are locked here:
//!
//! 1. `ralph-tools.md` documents the 12 environment variables that a
//!    DAG job actually receives (the typed `JobContext` env
//!    injection). Agents read these names verbatim, so the doc must
//!    use the real `RALPH_DAG_*` keys — not prose paraphrases.
//! 2. Neither doc mentions the placeholder table name
//!    `dag_units_completion_ack`; the shipped v20 migration creates
//!    `dag_terminal_deliveries`.
//! 3. Neither doc carries a plan-ID anchor in a section title
//!    ("AI skill guide 去计划化规则" hard rule): injected skills are
//!    shared by every agent and must not reference a single plan.

use std::path::PathBuf;

/// Repo-root-relative paths of the two injected skill docs covered
/// by these tests.
const RALPH_TOOLS_MD: &str = "data/ralph-tools.md";
const RALPH_TOOLS_EMIT_MD: &str = "data/ralph-tools-emit.md";

/// Read a doc relative to `crates/ralph-core/` so the test is
/// independent of the test runner's cwd.
fn read_doc(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "read_doc({relative}): {err} (resolved to {})",
            path.display()
        )
    })
}

/// The 12 env keys emitted by the typed `JobContext` env injection.
/// Keep in lockstep with `JobContext::to_env()`.
const JOB_CONTEXT_ENV_KEYS: &[&str] = &[
    "RALPH_DAG_PLAN_KEY",
    "RALPH_DAG_UNIT_KEY",
    "RALPH_DAG_TASK_KEY",
    "RALPH_DAG_TASK_ID",
    "RALPH_DAG_JOB_ID",
    "RALPH_DAG_JOB_TOKEN",
    "RALPH_DAG_STAGE",
    "RALPH_DAG_ATTEMPT",
    "RALPH_DAG_WORKTREE",
    "RALPH_DAG_BASE",
    "RALPH_DAG_VERIFIED_EXECUTION_PLAN_PATH",
    "RALPH_DAG_ARTIFACT_REFS",
];

/// Placeholder table name that must never reach an injected skill
/// doc; the real v20 migration creates `dag_terminal_deliveries`.
const WRONG_V20_TABLE: &str = "dag_units_completion_ack";
const REAL_V20_TABLE: &str = "dag_terminal_deliveries";

/// Plan-ID anchor that must not appear in injected skill docs.
const PLAN_ID_ANCHOR: &str = "2026-09-09-0917";

#[test]
fn ralph_tools_md_documents_12_env_fields() {
    let body = read_doc(RALPH_TOOLS_MD);
    let missing: Vec<&str> = JOB_CONTEXT_ENV_KEYS
        .iter()
        .copied()
        .filter(|key| !body.contains(key))
        .collect();
    assert!(
        missing.is_empty(),
        "{RALPH_TOOLS_MD} must document every typed JobContext env key; missing: {missing:?}",
    );
    assert_eq!(
        JOB_CONTEXT_ENV_KEYS.len(),
        12,
        "the JobContext env contract is 12 keys",
    );
}

#[test]
fn ralph_tools_docs_use_real_v20_table_name() {
    for doc in [RALPH_TOOLS_MD, RALPH_TOOLS_EMIT_MD] {
        let body = read_doc(doc);
        assert!(
            !body.contains(WRONG_V20_TABLE),
            "{doc} must NOT mention placeholder v20 table {WRONG_V20_TABLE:?}",
        );
    }
    assert!(
        read_doc(RALPH_TOOLS_EMIT_MD).contains(REAL_V20_TABLE),
        "{RALPH_TOOLS_EMIT_MD} must name the real v20 table {REAL_V20_TABLE:?}",
    );
}

#[test]
fn ralph_tools_docs_carry_no_plan_id_anchor() {
    for doc in [RALPH_TOOLS_MD, RALPH_TOOLS_EMIT_MD] {
        let body = read_doc(doc);
        let hits: Vec<&str> = body
            .lines()
            .filter(|line| line.contains(PLAN_ID_ANCHOR))
            .collect();
        assert!(
            hits.is_empty(),
            "{doc} must NOT anchor content to plan {PLAN_ID_ANCHOR}; hits: {hits:?}",
        );
    }
}
