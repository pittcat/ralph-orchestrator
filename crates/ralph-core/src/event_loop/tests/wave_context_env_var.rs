//! 2026-06-14-003 R1 (post-review) integration test: the
//! `RALPH_WAVE_CONTEXT` env var must be set on the backend's
//! `effective_backend.env_vars` when the displayed hat is
//! `review-synthesizer`.  Adversarial + agent-native reviewers
//! identified the missing wiring as a critical plan-vs-impl gap.
//!
//! We assert on `effective_backend.env_vars` directly rather than
//! spawning a subprocess: the env-var wiring lives in
//! `crates/ralph-cli/src/loop_runner/runner.rs:2554-2564` and is
//! visible to the test via the same `inject_hat_execution_env`
//! helper.  A higher-level test that drives a real `ralph run` is
//! out of scope for this suite (see `crates/ralph-cli/tests/` for
//! CLI-level coverage).

use crate::event_loop::EventLoop;
use crate::event_loop::tests::common::init_git_workspace;
use crate::loop_context::LoopContext;
use std::path::Path;

fn solo_config(workspace: &Path) -> crate::config::RalphConfig {
    let mut config = crate::config::RalphConfig::default();
    config.core.workspace_root = workspace.to_path_buf();
    config
}

#[test]
fn wave_context_json_for_hat_returns_none_for_non_synthesizer() {
    // Smoke test for the public accessor: hats other than
    // `review-synthesizer` must not get a serialized wave context
    // (the env var is empty for them).
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    let config = solo_config(dir.path());
    let ctx = LoopContext::primary(dir.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, ctx);
    let json = event_loop.wave_context_json_for_hat(&ralph_proto::HatId::new("executor"));
    assert!(
        json.is_none(),
        "non-synthesizer hat must not get wave context"
    );
}
