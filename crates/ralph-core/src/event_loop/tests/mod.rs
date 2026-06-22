//! Test suite for event_loop module, organized by topic.

use super::*;

mod active_hat;
mod backpressure;
mod build_prompt;
mod ce_executor;
mod chain_validation;
mod common;
mod completion_honored;
mod default_publishes;
mod deterministic_routing;
mod drift_integration;
mod ephemeral_isolation_integration;
mod event_filter;
mod event_policy;
mod execution_contract;
mod guidance_dedup;
mod handoff_dispatch;
mod hat_backend;
mod hat_exhaustion;
mod hat_lifecycle_integration;
mod hat_lifecycle_jsonl_e2e;
mod human_timeout;
mod incident_fixture;
mod initialization;
mod isolated_complex_regression;
mod isolated_wave_budget;
mod loop_context;
mod objective;
mod origin_guard;
mod payload_types;
mod persistent_mode;
mod progress_steward;
mod r5_hard_gate_routing;
mod recovery_envelope_u7_u8;
mod replay_light_integration;
mod review_step_gate;
mod robot_skill;
mod runtime_state_injection;
mod scope_enforcement;
mod scratchpad;
mod serial_lint;
mod stale_breaker;
mod state_machine;
mod structured_evidence;
mod task_resume_ttl;
mod termination;
mod text_fallback;
mod topic_format_recovery;
/// U7a / U7b (plan 2026-06-21-002): deterministic-correction
/// integration tests for the `CorrectionContext` /
/// `ResumeContext` prompt injection path.  See
/// `correction::tests` for unit-level coverage.
mod u7_correction;
/// U9 (plan 2026-06-21-002): migration test additions that
/// pin the unified `correction_context` / `loop.resume`
/// surface. The legacy `task.resume` tests in
/// `task_resume_ttl.rs` and `loop_runner/tests.rs` continue
/// to pass without these assertions — the new tests verify
/// the *new* deterministic-correction path on top of the
/// legacy task.resume injection.
mod u9_correction_assertions;
/// U11-T2 (plan 2026-06-22-u11-unified-state-production-wiring):
/// per-event unified `ValidationPipeline` integration tests.
mod u11_unified_pipeline_integration;
mod wave_context_env_var;
mod wave_context_injection;
mod wave_isolated_scope;
mod wave_policy_rejection;
mod wave_recovery_timeout;
mod wave_results;
mod workflow_guard;
