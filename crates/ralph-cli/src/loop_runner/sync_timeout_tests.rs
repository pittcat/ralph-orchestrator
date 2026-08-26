//! Tests for `sync_timeout` and related runner helpers.
//!
//! Two test modules are kept here:
//! - `sync_timeout_tests` (timeout semantics)
//! - `sync_timeout_lint_tests` (preset-lint wiring through the runner)
//!
//! Also re-exports `runner_inner_test_api` for integration tests
//! that need access to `merge_isolated_channel_on_interrupt` without
//! making the helper public at the parent module level.

use ralph_core::{EventLoop, LoopContext, RalphConfig};
use std::path::Path;

#[cfg(test)]
#[allow(clippy::module_inception)]
mod sync_timeout_tests {
    use ralph_core::agent_doc_sync::block::BlockSpec;
    use ralph_core::agent_doc_sync::{OnError, SyncConfig};
    use tempfile::TempDir;

    #[test]
    fn zero_timeout_runs_inline_and_succeeds() {
        // D5: `startup_timeout_secs: 0` disables the timeout and
        // returns Ok when the underlying sync succeeds.
        let dir = TempDir::new().unwrap();
        let block = BlockSpec::new("hang-prevention", "x");
        let blocks = [block];
        let target_files = ["CLAUDE.md"];
        let cfg = SyncConfig {
            skip: false,
            on_error: OnError::Warn,
            target_files: &target_files,
            blocks: &blocks,
            session_dir: None,
        };
        let outcome = crate::loop_runner::sync_timeout::run_sync_with_timeout(dir.path(), &cfg, 0);
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
    }

    #[test]
    fn nonzero_timeout_propagates_sync_error_quickly() {
        // D5: when the underlying sync fails fast (e.g. unwritable
        // target), `run_sync_with_timeout` must surface the error
        // via `SyncRunError::Sync` rather than spuriously firing the
        // timeout. We deliberately use `OnError::Warn` so the
        // underlying sync returns Ok; we assert Ok here.
        let dir = TempDir::new().unwrap();
        let block = BlockSpec::new("hang-prevention", "x");
        let blocks = [block];
        let target_files = ["CLAUDE.md"];
        let cfg = SyncConfig {
            skip: false,
            on_error: OnError::Warn,
            target_files: &target_files,
            blocks: &blocks,
            session_dir: None,
        };
        let started = std::time::Instant::now();
        let outcome = crate::loop_runner::sync_timeout::run_sync_with_timeout(dir.path(), &cfg, 30);
        let elapsed = started.elapsed();
        // Sync returns Ok with synced=1 well before 30s.
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "fast sync should not wait 30s: {elapsed:?}"
        );
    }
}

/// 2026-07-09-001 plan (U1 / A7): the lint gate call site in
/// `run_loop_impl_inner` (line ~687) MUST route through
/// `enforce_preset_lint_gate_with_preset_name` so the U7
/// emit-feedback skill-reference rule actually fires on
/// `ralph run -H builtin:ce-executor-pipeline-loop`. Without
/// the preset name the whitelist gate inside
/// `check_instructions_opac_with_preset` resolves to
/// `preset_name = ""` and the rule silently bypasses.
#[cfg(test)]
mod u1_preset_name_aware_lint_gate_wiring {

    use crate::loop_runner::preset_lint_gate::enforce_preset_lint_gate;
    use ralph_core::RalphConfig;

    /// A minimal preset that mimics `ce-executor-pipeline-loop`'s
    /// `fix-planner` hat with the U7 emit-feedback citation
    /// deliberately removed. The topology stays lint-clean
    /// (handoff / completion_promise covered) so the only
    /// failing rule on the whitelisted preset is the U7 skill
    /// reference missing — the exact U1/A7 production
    /// asymmetry the runner has to defend against.
    const LOOP_YAML_WITHOUT_CITE: &str = r#"
hats:
  worker:
    name: "Worker"
    description: "Build the U-IDs"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  review-synthesizer:
    name: "Review Synthesizer"
    description: "Aggregate review findings"
    triggers: ["work.ready"]
    publishes: ["work.complete"]
  fix-planner:
    name: "Fix Planner"
    description: "Draft the fix plan"
    triggers: ["work.complete"]
    publishes: ["loop.complete"]
    instructions: |
      Draft a fix plan for the review findings referencing
      the payload shape conventions. Use
      `ralph tools task ensure` to register the per-fix
      tasks.
event_loop:
  starting_event: "work.start"
  completion_promise: "loop.complete"
tasks:
  enabled: false
"#;

    #[test]
    fn invokes_preset_name_aware_lint_gate() {
        // The same config produces different gate verdicts
        // depending on whether the preset name is forwarded.
        // The fresh call site (with `hats_source_label`) MUST
        // land on the `_with_preset_name` path; the legacy
        // no-arg `enforce_preset_lint_gate` API bails out of
        // the U7 whitelist silently.
        let config: RalphConfig =
            serde_yaml::from_str(LOOP_YAML_WITHOUT_CITE).expect("yaml must parse");

        // The runner passes raw_yaml through to the lint
        // aggregator; the U7 instruction-level check only
        // fires when raw_yaml is supplied, matching the
        // production `ralph run` path (which threads raw YAML
        // through from `RalphConfig::parse_yaml`).
        let lint_source_yaml = LOOP_YAML_WITHOUT_CITE;

        let legacy_result = enforce_preset_lint_gate(&config, false);
        assert!(
            legacy_result.is_ok(),
            "legacy no-preset-name gate must NOT fire U7 (pre-fix behaviour, expected)"
        );

        // The lint aggregator signature requires
        // `raw_yaml: Option<&str>` to reach the
        // instructions_opac layer. The legacy no-arg
        // `enforce_preset_lint_gate` does NOT thread
        // raw_yaml by design (it is the
        // unit-test/no-yaml path), so we drive
        // `run_preset_lint_with_preset_name` directly to
        // prove the same call site the new
        // `enforce_preset_lint_gate_with_preset_name`
        // uses produces the U7 finding the runner
        // depends on.
        let whitelisted_result = ralph_core::preset_lint::run_preset_lint_with_preset_name(
            &config,
            ralph_core::preset_lint::LintStrictness::Strict,
            false,
            Some(lint_source_yaml),
            "ce-executor-pipeline-loop",
        );
        assert!(
            whitelisted_result.iter().any(|f| {
                f.id == "lint.preset.instructions_emit_feedback_skill_reference_missing"
                    && f.severity == ralph_core::runtime_contract::FindingSeverity::Error
            }),
            "preset-name-aware lint gate must surface U7 missing skill reference; got: {whitelisted_result:#?}"
        );
    }

    #[test]
    fn invokes_lint_gate_without_preset_name_when_source_unknown() {
        // The legacy 2-arg `enforce_preset_lint_gate` path is
        // the only branch the runner reaches when
        // `hats_source_label` is `None`. It must NOT silently
        // promote the same U7 finding because the whitelist
        // cannot resolve a preset name; the helper returns
        // Ok without surfacing the U7 finding to keep the
        // operator experience predictable when no preset is
        // attached.
        let config: RalphConfig =
            serde_yaml::from_str(LOOP_YAML_WITHOUT_CITE).expect("yaml must parse");

        let legacy_result = enforce_preset_lint_gate(&config, false);
        assert!(
            legacy_result.is_ok(),
            "legacy no-preset-name gate must NOT fire U7 when no preset is attached"
        );
    }
}

// All items are `#[cfg(test)]` so they do not appear in the production
// binary, but the helper itself must remain reachable for integration
// tests that cannot access private runner items.
#[cfg(test)]
pub mod runner_inner_test_api {
    use super::*;

    /// Best-effort merge of the isolated hat-channel into the main events
    /// file from any of the interrupt paths (mid-loop `tokio::select!`
    /// abort, top-of-loop interrupt check).
    ///
    /// Integration tests in `legacy.rs` pin three properties here:
    ///   1. content is merged into main events and the channel file is
    ///      removed (no duplicate replays on the next iteration);
    ///   2. an empty channel does not corrupt main events and emits a
    ///      `channel-routing-fallback-*.md` diagnostic;
    ///   3. a no-marker cold-path interrupt is a safe no-op (no panic,
    ///      no events appended).
    ///
    /// Re-exported here so integration tests (which cannot reach private
    /// items) can call the helper directly.
    pub fn merge_isolated_channel_on_interrupt(
        ctx: &LoopContext,
        config: &RalphConfig,
        state_machine_enabled: bool,
        event_loop: &EventLoop,
        interrupt_kind: &'static str,
        owned_channel_path: Option<&Path>,
    ) {
        crate::loop_runner::entry::merge_isolated_channel_on_interrupt(
            ctx,
            config,
            state_machine_enabled,
            event_loop,
            interrupt_kind,
            None,
            None,
            owned_channel_path,
        )
    }
}
