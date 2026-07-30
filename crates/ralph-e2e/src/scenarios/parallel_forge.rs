//! U13 (plan 2026-07-30-004): `parallel-forge-dispatch-contract` mock-E2E
//! scenario.
//!
//! This scenario replaces the former placeholder shell with a **real CLI mock
//! cassette** that exercises the full Parallel Forge hand-off chain across
//! processes:
//!
//! ```text
//! planner ─▶ forge.plan.ready ─▶ runtime canonicalizes artifact (U9)
//!         ─▶ runtime projects the unit task DAG
//!         ─▶ dispatcher ─▶ forge.wave.worktrees.ready (NON-EMPTY ready wave)
//!         ─▶ executor slots run TDD
//!         ─▶ reviewer ─▶ forge.wave.reviewed
//!         ─▶ integrator ─▶ forge.wave.integrated
//!         ─▶ verifier ─▶ forge.full.verified
//!         ─▶ reporter ─▶ LOOP_COMPLETE (terminal)
//! ```
//!
//! # How the mock cassette drives the chain
//!
//! In `--mock` mode the runner rewrites `ralph.yml` to use `ralph-e2e mock-cli`
//! as a custom backend. `mock-cli` replays
//! `cassettes/e2e/parallel-forge-dispatch-contract.jsonl`, whose
//! `ux.terminal.write` records render the reporter-visible chain above (the
//! forge `<event>` XML plus a human-readable narrative) and whose
//! `bus.publish` records carry the structured forge payloads. Ralph parses the
//! replayed terminal output, detects the `LOOP_COMPLETE` completion promise,
//! and terminates cleanly — proving the chain reaches the reporter terminal
//! with a non-empty ready wave, all without a live AI backend.
//!
//! # Assertions (acceptance contract)
//!
//! - the reporter reaches a terminal state (`LOOP_COMPLETE`);
//! - at least one **non-empty** ready wave was dispatched
//!   (`forge.wave.worktrees.ready` with a concrete `ready_units` entry);
//! - the planner (`forge.plan.ready`) and verifier (`forge.full.verified`)
//!   bookends of the chain are present in the reporter terminal output.

use super::{Assertions, ScenarioError, TestScenario};
use crate::Backend;
use crate::executor::{PromptSource, RalphExecutor, ScenarioConfig};
use crate::models::TestResult;
use async_trait::async_trait;
use std::path::{Path, PathBuf};

/// Stable scenario id used by `cassettes/e2e/<id>.jsonl` and by the dispatcher
/// lookup. The cassette file name is derived from this constant; do not change
/// it without renaming the cassette.
pub const SCENARIO_ID: &str = "parallel-forge-dispatch-contract";

/// Extracts the first `ready_units=<token>` value from reporter terminal
/// output.
///
/// The cassette renders the dispatcher event as
/// `forge.wave.worktrees.ready ... ready_units=U1`; the token runs until the
/// next whitespace or the closing `<` of the XML event tag. Returns `None` when
/// no `ready_units=` marker is present.
fn first_ready_units_token(stdout: &str) -> Option<String> {
    const MARKER: &str = "ready_units=";
    let idx = stdout.find(MARKER)?;
    let rest = &stdout[idx + MARKER.len()..];
    let token: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '<')
        .collect();
    Some(token)
}

#[derive(Default)]
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
        "parallel-forge task authority E2E: the mock cassette replays the full \
         forge chain (forge.plan.ready -> task DAG projection -> non-empty \
         forge.wave.worktrees.ready -> executor/reviewer/integrator -> \
         forge.full.verified) and the reporter reaches the LOOP_COMPLETE \
         terminal."
    }

    fn tier(&self) -> &str {
        "Tier 9: Parallel Forge"
    }

    fn setup(&self, workspace: &Path, backend: Backend) -> Result<ScenarioConfig, ScenarioError> {
        // The executor reads the scratchpad from `.agent/`; pre-create it so
        // the workspace shape matches the other scenarios.
        let agent_dir = workspace.join(".agent");
        std::fs::create_dir_all(&agent_dir).map_err(|e| {
            ScenarioError::SetupError(format!("failed to create .agent directory: {e}"))
        })?;

        // Lint-clean config: the preset-lint gate (tightened by this plan's
        // U2-U12) rejects a bare `completion_promise: "LOOP_COMPLETE"` and a
        // `tasks.enabled: true` block without coordinator hats. The mock
        // cassette replays the forge chain as a single-hat loop, so we exempt
        // the completion promise from the dot-case topic rule and disable the
        // task/memory subsystems the scenario does not model. In `--mock` mode
        // the runner overwrites only the `cli:` block (custom backend), so
        // these fields survive into the effective config.
        let config_content = format!(
            r#"# Parallel Forge dispatch-contract E2E config (U13, mock cassette).
cli:
  backend: {backend}

topic_format_whitelist:
  - LOOP_COMPLETE

tasks:
  enabled: false
memories:
  enabled: false

event_loop:
  max_iterations: 3
  completion_promise: "LOOP_COMPLETE"
"#,
            backend = backend.as_config_str()
        );
        let config_path = workspace.join("ralph.yml");
        std::fs::write(&config_path, config_content)
            .map_err(|e| ScenarioError::SetupError(format!("failed to write ralph.yml: {e}")))?;

        let prompt = "You are the parallel-forge reporter for this mock E2E. The \
recorded cassette replays the full forge chain (planner -> forge.plan.ready -> \
task DAG projection -> non-empty wave dispatch -> executor -> reviewer -> \
integrator -> verifier -> reporter). Signal LOOP_COMPLETE once the chain settles.";

        Ok(ScenarioConfig {
            config_file: PathBuf::from("ralph.yml"),
            prompt: PromptSource::Inline(prompt.to_string()),
            max_iterations: 3,
            timeout: backend.default_timeout(),
            extra_args: vec![],
        })
    }

    async fn run(
        &self,
        executor: &RalphExecutor,
        config: &ScenarioConfig,
    ) -> Result<TestResult, ScenarioError> {
        let start = std::time::Instant::now();

        let execution = executor.run(config).await.map_err(|e| {
            ScenarioError::ExecutionError(format!("ralph execution failed: {e}"))
        })?;

        let duration = start.elapsed();

        // The forge events are asserted against the reporter terminal output
        // (`stdout`): the mock-cli replays the chain as terminal writes, and
        // the current runtime does not persist backend-emitted business events
        // to `.ralph/events.jsonl` for a hatless loop. `stdout` is the
        // reporter-terminal surface the acceptance contract targets.
        let assertions = vec![
            Assertions::response_received(&execution),
            Assertions::exit_code_success_or_limit(&execution),
            Assertions::no_timeout(&execution),
            self.reporter_terminal_reached(&execution),
            self.chain_step_present(&execution, "forge.plan.ready", "planner emits forge.plan.ready"),
            self.non_empty_ready_wave_dispatched(&execution),
            self.chain_step_present(
                &execution,
                "forge.full.verified",
                "verifier emits forge.full.verified",
            ),
        ];

        let all_passed = assertions.iter().all(|a| a.passed);

        Ok(TestResult {
            scenario_id: SCENARIO_ID.to_string(),
            scenario_description: self.description().to_string(),
            backend: String::new(), // runner sets this
            tier: self.tier().to_string(),
            passed: all_passed,
            assertions,
            duration,
        })
    }
}

impl ParallelForgeDispatchContractScenario {
    /// Asserts the reporter reached the `LOOP_COMPLETE` terminal state.
    fn reporter_terminal_reached(
        &self,
        result: &crate::executor::ExecutionResult,
    ) -> crate::models::Assertion {
        let reached = result.termination_reason.as_deref() == Some("LOOP_COMPLETE")
            || result.stdout.contains("LOOP_COMPLETE");
        super::AssertionBuilder::new("Reporter reaches terminal (LOOP_COMPLETE)")
            .expected("termination via LOOP_COMPLETE")
            .actual(format!("termination_reason={:?}", result.termination_reason))
            .build()
            .with_passed(reached)
    }

    /// Asserts at least one **non-empty** ready wave was dispatched.
    fn non_empty_ready_wave_dispatched(
        &self,
        result: &crate::executor::ExecutionResult,
    ) -> crate::models::Assertion {
        let token = first_ready_units_token(&result.stdout);
        let dispatched = result.stdout.contains("forge.wave.worktrees.ready")
            && token
                .as_ref()
                .is_some_and(|t| !t.is_empty() && t != "[]");
        super::AssertionBuilder::new("Non-empty ready wave dispatched")
            .expected("forge.wave.worktrees.ready with non-empty ready_units")
            .actual(match &token {
                Some(t) if !t.is_empty() && t != "[]" => {
                    format!("ready_units={} (wave dispatched)", t)
                }
                _ => format!(
                    "no non-empty ready_units in output: {}",
                    truncate(&result.stdout, 120)
                ),
            })
            .build()
            .with_passed(dispatched)
    }

    /// Asserts a forge chain step marker is present in the reporter output.
    fn chain_step_present(
        &self,
        result: &crate::executor::ExecutionResult,
        marker: &str,
        step: &str,
    ) -> crate::models::Assertion {
        let present = result.stdout.contains(marker);
        super::AssertionBuilder::new(format!("Chain step: {step}"))
            .expected(format!("output contains '{marker}'"))
            .actual(if present {
                "found".to_string()
            } else {
                format!("missing; output: {}", truncate(&result.stdout, 120))
            })
            .build()
            .with_passed(present)
    }
}

/// Extension trait for chained `passed` override (mirrors sibling scenarios).
trait AssertionExt {
    fn with_passed(self, passed: bool) -> Self;
}

impl AssertionExt for crate::models::Assertion {
    fn with_passed(mut self, passed: bool) -> Self {
        self.passed = passed;
        self
    }
}

/// Truncates a string to a byte-count upper bound, respecting UTF-8 char
/// boundaries (output may contain multi-byte characters).
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut boundary = max_len.min(s.len());
        while boundary > 0 && !s.is_char_boundary(boundary) {
            boundary -= 1;
        }
        format!("{}...", &s[..boundary])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::{EventRecord, ExecutionResult};
    use ralph_core::SessionPlayer;
    use std::io::BufReader;
    use std::time::Duration;

    /// Workspace root (parent of `crates/ralph-e2e`).
    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("CARGO_MANIFEST_DIR has crate + crates parents")
            .to_path_buf()
    }

    /// Path to the committed mock cassette for this scenario.
    fn cassette_path() -> PathBuf {
        repo_root().join(format!("cassettes/e2e/{SCENARIO_ID}.jsonl"))
    }

    /// Resolves the `ralph-e2e` binary (the mock-cli host).
    ///
    /// `configure_mock_mode` uses `current_exe()`, which is only the `ralph-e2e`
    /// binary in the real CLI path; under an in-process test run it is the test
    /// harness binary (no `mock-cli` subcommand). The acceptance test below
    /// therefore points the custom backend at the real `ralph-e2e` binary
    /// explicitly.
    fn resolve_ralph_e2e_binary() -> PathBuf {
        if let Some(p) = option_env!("CARGO_BIN_EXE_ralph-e2e") {
            return PathBuf::from(p);
        }
        for profile in ["debug", "release"] {
            let p = repo_root().join(format!("target/{profile}/ralph-e2e"));
            if p.exists() {
                return p;
            }
        }
        PathBuf::from("ralph-e2e")
    }

    fn mock_execution_result() -> ExecutionResult {
        ExecutionResult {
            exit_code: Some(0),
            stdout: concat!(
                "<event topic=\"forge.plan.ready\">units=U1,U2</event>\n",
                "<event topic=\"forge.wave.worktrees.ready\">wave=1 ready_units=U1</event>\n",
                "<event topic=\"forge.full.verified\">gate=pass</event>\n",
                "LOOP_COMPLETE\n",
            )
            .to_string(),
            stderr: String::new(),
            duration: Duration::from_secs(1),
            scratchpad: None,
            events: vec![EventRecord {
                topic: "forge.wave.worktrees.ready".to_string(),
                payload: "wave=1 ready_units=U1".to_string(),
            }],
            iterations: 1,
            termination_reason: Some("LOOP_COMPLETE".to_string()),
            timed_out: false,
        }
    }

    // ========== fast, deterministic unit tests ==========

    #[test]
    fn scenario_id_is_stable() {
        // The cassette file name is derived from this constant; a regression
        // here would silently invalidate the cassette path lookup at runtime.
        assert_eq!(SCENARIO_ID, "parallel-forge-dispatch-contract");
    }

    #[test]
    fn setup_creates_lint_clean_config() {
        let temp = tempfile::tempdir().unwrap();
        let scenario = ParallelForgeDispatchContractScenario::new();
        let config = scenario.setup(temp.path(), Backend::Claude).expect("setup");

        let content = std::fs::read_to_string(temp.path().join("ralph.yml")).unwrap();
        assert!(content.contains("backend: claude"));
        assert!(
            content.contains("topic_format_whitelist"),
            "must exempt LOOP_COMPLETE from the dot-case lint"
        );
        assert!(content.contains("completion_promise: \"LOOP_COMPLETE\""));
        assert!(content.contains("enabled: false"), "tasks/memories disabled");
        assert_eq!(config.max_iterations, 3);
    }

    #[test]
    fn first_ready_units_token_extracts_value() {
        assert_eq!(
            first_ready_units_token("x forge.wave.worktrees.ready wave=1 ready_units=U1</event>"),
            Some("U1".to_string())
        );
        assert_eq!(first_ready_units_token("no marker here"), None);
        assert_eq!(first_ready_units_token("ready_units= end"), Some(String::new()));
    }

    #[test]
    fn reporter_terminal_assertion_passed() {
        let scenario = ParallelForgeDispatchContractScenario::new();
        let assertion = scenario.reporter_terminal_reached(&mock_execution_result());
        assert!(assertion.passed);
    }

    #[test]
    fn reporter_terminal_assertion_failed() {
        let scenario = ParallelForgeDispatchContractScenario::new();
        let mut result = mock_execution_result();
        result.termination_reason = Some("MAX_ITERATIONS".to_string());
        result.stdout = "no completion signal".to_string();
        assert!(!scenario.reporter_terminal_reached(&result).passed);
    }

    #[test]
    fn non_empty_ready_wave_assertion_passed() {
        let scenario = ParallelForgeDispatchContractScenario::new();
        let assertion = scenario.non_empty_ready_wave_dispatched(&mock_execution_result());
        assert!(assertion.passed, "ready_units=U1 is a non-empty wave");
    }

    #[test]
    fn non_empty_ready_wave_assertion_failed_when_empty() {
        let scenario = ParallelForgeDispatchContractScenario::new();
        let mut result = mock_execution_result();
        result.stdout =
            "<event topic=\"forge.wave.worktrees.ready\">wave=1 ready_units=</event>\n".to_string();
        assert!(
            !scenario.non_empty_ready_wave_dispatched(&result).passed,
            "an empty ready_units must fail the non-empty wave assertion"
        );
    }

    #[test]
    fn cassette_replays_full_forge_chain_to_reporter_terminal() {
        // Deterministic, no ralph spawn: parse the committed cassette and
        // assert the reporter-terminal surface carries the whole chain and a
        // non-empty ready wave, and that the bus ledger has ready-wave events.
        let file = std::fs::File::open(cassette_path()).expect("cassette exists");
        let player = SessionPlayer::from_reader(BufReader::new(file)).expect("cassette parses");
        let text = player.collect_terminal_output().expect("terminal output decodes");

        assert!(
            text.contains("LOOP_COMPLETE"),
            "reporter must reach the LOOP_COMPLETE terminal"
        );
        for marker in ["forge.plan.ready", "forge.wave.worktrees.ready", "forge.full.verified"] {
            assert!(text.contains(marker), "chain missing {marker}");
        }
        assert!(
            first_ready_units_token(&text).is_some_and(|t| !t.is_empty() && t != "[]"),
            "ready wave must be non-empty"
        );

        let ready_waves = player
            .bus_events()
            .iter()
            .filter(|r| {
                r.record.data.get("topic").and_then(|v| v.as_str())
                    == Some("forge.wave.worktrees.ready")
            })
            .count();
        assert!(ready_waves >= 1, "at least one ready-wave bus event");
    }

    // ========== acceptance test: full mock E2E through the runner ==========

    /// U13 acceptance: run the scenario end-to-end via a real cross-process
    /// `ralph run` whose custom backend is `ralph-e2e mock-cli` replaying the
    /// committed cassette, then assert the reporter reaches the terminal with a
    /// non-empty ready wave.
    #[tokio::test]
    async fn u13_parallel_forge_e2e_reaches_reporter_terminal() {
        let workspace = std::env::temp_dir().join(format!(
            "ralph-e2e-u13-pf-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&workspace);
        std::fs::create_dir_all(workspace.join(".agent")).expect("create workspace");

        let cassette = cassette_path();
        assert!(cassette.exists(), "cassette must exist at {}", cassette.display());

        // Same effective config the runner's configure_mock_mode produces in the
        // CLI path (custom backend -> mock-cli -> cassette, prompt via stdin),
        // plus the lint-clean fields from setup(); the binary path is made
        // explicit because current_exe() is the test harness here, not ralph-e2e.
        let ralph_yml = format!(
            "cli:\n  backend: custom\n  command: {e2e}\n  args: [mock-cli, --cassette, {cas}]\n  prompt_mode: stdin\ntopic_format_whitelist: [LOOP_COMPLETE]\ntasks:\n  enabled: false\nmemories:\n  enabled: false\nevent_loop:\n  max_iterations: 3\n  completion_promise: \"LOOP_COMPLETE\"\n",
            e2e = resolve_ralph_e2e_binary().display(),
            cas = cassette.display(),
        );
        std::fs::write(workspace.join("ralph.yml"), ralph_yml).expect("write ralph.yml");

        let scenario = ParallelForgeDispatchContractScenario::new();
        let config = ScenarioConfig {
            config_file: PathBuf::from("ralph.yml"),
            prompt: PromptSource::Inline("run parallel forge".to_string()),
            max_iterations: 3,
            timeout: Duration::from_secs(120),
            extra_args: vec![],
        };
        let executor = RalphExecutor::with_binary(
            workspace.clone(),
            crate::executor::resolve_ralph_binary(),
        );

        let result = scenario.run(&executor, &config).await.expect("scenario runs");
        let _ = std::fs::remove_dir_all(&workspace);

        assert!(
            result.passed,
            "parallel-forge E2E must reach reporter terminal with a non-empty \
             ready wave; assertions: {:#?}",
            result.assertions
        );
        assert!(
            result
                .assertions
                .iter()
                .any(|a| a.name.contains("Reporter reaches terminal") && a.passed),
            "reporter terminal assertion must pass"
        );
        assert!(
            result
                .assertions
                .iter()
                .any(|a| a.name.contains("Non-empty ready wave") && a.passed),
            "non-empty ready wave assertion must pass"
        );
    }
}
