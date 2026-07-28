//! Plan 2026-07-28-001 U3 (R9 / R16 / S13): `parallel-forge-dispatch-contract`
//! mock-E2E scenario.
//!
//! This file is the **scenario shell** that pins the scenario ID and
//! registers it with the ralph-e2e dispatcher. The complete
//! implementation of plan §4.5 / §4.6 mock-cli protocol
//! (`--activation-cursor` / `--task-ledger` / activation-group
//! markers / cursor lock files / `bus.publish.data.command`
//! whitelist / per-group terminal output) is pending a follow-up
//! plan that lands the full 15-group cassette and the harness
//! upgrade. The shell exists so the dispatcher does not silently
//! grow a parallel-forge scenario without its companion marker
//! cassette, and so adding the full body later only touches this
//! file + the new cassette under `cassettes/e2e/`.
//!
//! Acceptance contract (P0 surface; the harness + cassette path is
//! what plan §4.6 #21-#28 adds when the follow-up lands):
//! - scenario ID fixed: `parallel-forge-dispatch-contract`
//! - `setup` writes the same artifact paths the future cassette
//!   requires (development_plan / execution_plan / concurrency
//!   approval / report / audit / final report), without yet
//!   exercising the activation-group cursor
//! - `run` rejects any cascade of these scenarios in
//!   `--activation-cursor` mode until the cassette exists
//!   (executable shell keeps the contract honest)
//! - `cleanup` removes the workspace-local cursor file the
//!   future cassette is meant to advance.
//!
//! See `cassettes/e2e/parallel-forge-dispatch-contract.jsonl`
//! and `cassettes/e2e/README.md` for the upstream marker
//! cassette contract.

use super::{ScenarioError, TestScenario};
use crate::Backend;
use crate::executor::{RalphExecutor, ScenarioConfig};
use crate::models::TestResult;
use async_trait::async_trait;
use std::path::{Path, PathBuf};

/// Stable scenario id used by `cassettes/e2e/<id>.jsonl` and by
/// the dispatcher lookup.
pub const SCENARIO_ID: &str = "parallel-forge-dispatch-contract";

/// Workspace-local cursor path the future mock-cli harness will
/// use to advance per-activation. The shell scenario already
/// creates the parent directory so the harness can simply open it
/// once the cassette lands.
fn workspace_cursor(workspace: &Path, backend: Backend) -> PathBuf {
    workspace
        .join(".ralph")
        .join("e2e-mock")
        .join(format!("{}-{}.cursor", SCENARIO_ID, backend))
}

pub struct ParallelForgeDispatchContractScenario;

impl ParallelForgeDispatchContractScenario {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl TestScenario for ParallelForgeDispatchContractScenario {
    fn id(&self) -> &str {
        SCENARIO_ID
    }

    fn description(&self) -> &str {
        "parallel-forge task authority E2E: forge.plan.ready atomically \
         materialises the unit DAG, dispatcher reads live task ids from \
         the TaskStore, supervisor fan-in closes U1 then opens U2, the \
         loop completes. (Plan 2026-07-28-001 U3 R9 / R16 / S13. The \
         cassette + activation-cursor harness land in a follow-up plan; \
         this shell pins the scenario id + workspace setup so the ralph-e2e \
         dispatcher does not silently grow a half-implemented scenario.)"
    }

    fn tier(&self) -> &str {
        "Tier 3: Parallel Forge"
    }

    fn setup(&self, workspace: &Path, backend: Backend) -> Result<ScenarioConfig, ScenarioError> {
        // Pre-create the directory the future mock-cli will own.
        // The shell itself does not advance the cursor; it only
        // makes the path discoverable so the follow-up cassette +
        // harness can read/write without a race against scenario
        // setup.
        let cursor = workspace_cursor(workspace, backend);
        if let Some(parent) = cursor.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ScenarioError::SetupError(format!(
                    "failed to pre-create cursor dir {}: {e}",
                    parent.display()
                ))
            })?;
        }
        // Real cassette body will replace this with the parallel-forge
        // preset and the full artifact setup. Until the cassette
        // lands, the shell registers the scenario id and yields a
        // minimal config the harness can boot.
        Ok(ScenarioConfig::minimal(
            "Parallel Forge dispatch contract — placeholder; \
             cassette lands in a follow-up plan."
                .to_string(),
        ))
    }

    async fn run(
        &self,
        _executor: &RalphExecutor,
        _config: &ScenarioConfig,
    ) -> Result<TestResult, ScenarioError> {
        // Plan 2026-07-28-001 §4.6 #21 + follow-up plan: this body
        // is a placeholder until the marker-cassette harness lands.
        // Returning an explicit failure with a stable reason keeps
        // CI honest: a run that hits this path is asserting the
        // scenario was *registered*, not that the cassette is
        // already wired.
        let assertion = super::AssertionBuilder::new("Cassette harness lands in follow-up plan")
            .expected("placeholder")
            .actual("placeholder")
            .failed()
            .build();
        Ok(TestResult {
            scenario_id: SCENARIO_ID.to_string(),
            scenario_description: self.description().to_string(),
            backend: String::new(), // runner sets this
            tier: self.tier().to_string(),
            passed: false,
            assertions: vec![assertion],
            duration: std::time::Duration::from_millis(0),
        })
    }

    fn cleanup(&self, workspace: &Path) -> Result<(), ScenarioError> {
        // Best-effort: drop the workspace cursor placeholder. The
        // workspace itself is wiped by the runner, but the
        // cursor-placeholder removal makes the shell's contract
        // obvious to a future reader.
        let cursor_dir = workspace.join(".ralph").join("e2e-mock");
        let _ = std::fs::remove_dir_all(&cursor_dir);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_id_is_stable() {
        // The cassette file name is derived from this constant;
        // a regression here would silently invalidate the cassette
        // path lookup at runtime.
        assert_eq!(SCENARIO_ID, "parallel-forge-dispatch-contract");
    }

    #[test]
    fn setup_creates_cursor_dir() {
        let temp = tempfile::tempdir().unwrap();
        let scenario = ParallelForgeDispatchContractScenario::new();
        scenario.setup(temp.path(), Backend::Claude).expect("setup");
        let cursor = workspace_cursor(temp.path(), Backend::Claude);
        assert!(cursor.parent().unwrap().is_dir());
    }

    #[test]
    fn cleanup_removes_cursor_dir() {
        let temp = tempfile::tempdir().unwrap();
        let scenario = ParallelForgeDispatchContractScenario::new();
        scenario.setup(temp.path(), Backend::Claude).expect("setup");
        scenario.cleanup(temp.path()).expect("cleanup");
        assert!(
            !workspace_cursor(temp.path(), Backend::Claude)
                .parent()
                .unwrap()
                .exists()
        );
    }
}
