//! Guard test for the `agent-client-protocol` dependency-removal contract.
//!
//! Plan U6 (in the `2026-07-14-001-refactor-remove-5-backends` fix-plan)
//! requires a programmatic regression test so that future changes cannot
//! silently re-introduce a dependency on the (now removed) agent-client-
//! protocol crate. The test reads the three files where such a
//! dependency declaration could be expressed — the workspace
//! `Cargo.toml`, the crate-local `Cargo.toml`, and the resolved
//! `Cargo.lock` — and asserts that none of them contains the literal
//! substring `agent-client-protocol`.
//!
//! The test is fully self-contained: it uses only `std::fs` and has no
//! external dependencies, so it runs against any cargo invocation that
//! can resolve the workspace.

use std::fs;
use std::path::PathBuf;

const FORBIDDEN_SUBSTRING: &str = "agent-client-protocol";

/// Returns the workspace root, computed as the parent of the `crates/`
/// directory that contains this crate. CARGO_MANIFEST_DIR for
/// `ralph-adapters` resolves to `crates/ralph-adapters/`, so the
/// workspace root is two levels up: `<repo_root>/`.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Loads a UTF-8 text file by relative-to-workspace path and fails the
/// test with the offending path and underlying IO error if the file
/// cannot be read. This produces an actionable diagnostic if a file is
/// missing or unreadable in CI, rather than a generic panic.
fn read_workspace_file(workspace_root: &PathBuf, relative: &str) -> String {
    let path = workspace_root.join(relative);
    fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "test_agent_client_protocol_dependency_removed: failed to read {}: {}",
            path.display(),
            err
        )
    })
}

/// Scans `contents` for `FORBIDDEN_SUBSTRING` and returns `Some(line_number)`
/// (1-indexed) of the first hit, or `None` if absent. A separate helper
/// keeps the assertion messages precise: they can point at the exact line
/// in the file where the dependency re-appeared.
fn first_match_line(contents: &str) -> Option<usize> {
    contents
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains(FORBIDDEN_SUBSTRING))
        .map(|(idx, _)| idx + 1)
}

/// Asserts that no file participating in the
/// `agent-client-protocol`-removal contract mentions the crate by name.
#[test]
fn agent_client_protocol_dependency_removed() {
    let workspace = workspace_root();

    let files = [
        "Cargo.toml",
        "crates/ralph-adapters/Cargo.toml",
        "Cargo.lock",
    ];

    for relative in files {
        let contents = read_workspace_file(&workspace, relative);
        if let Some(line_no) = first_match_line(&contents) {
            panic!(
                "ralph-adapters must not depend on agent-client-protocol, \
                 but found literal `{forbidden}` in {path}:{line_no}. \
                 Plan 2026-07-14-001 / fix-plan U6 requires this dependency \
                 to stay removed. Re-introduction would re-add the ACP runtime \
                 that was intentionally dropped when kiro/kiro-acp were removed.",
                forbidden = FORBIDDEN_SUBSTRING,
                path = workspace.join(relative).display(),
                line_no = line_no,
            );
        }
    }
}
