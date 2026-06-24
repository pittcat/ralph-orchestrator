//! Test suite for event_loop module, organized by topic.

use super::*;

mod active_hat;
// 2026-06-23-005 U4: AuditSeverity SSOT integration tests (R5+R12+KTD-8).
mod audit_severity_ssot;
mod backpressure;
mod build_prompt;
mod ce_executor;
mod chain_validation;
mod common;
mod completion_honored;
// 2026-06-23-005 U2: typed dispatch coverage for the three new
// `RejectionKind` variants (MissingEventGate / StallNoEvents /
// ContractViolation). See `coordinator_dispatch_coverage.rs`.
mod coordinator_dispatch_coverage;
mod default_publishes;
mod deterministic_routing;
mod drift_integration;
// 2026-06-23-005 U1: typed kind SSOT wiring for `enrich_task_resume_payload`.
// See `enrich_kind_wiring.rs`.
mod enrich_kind_wiring;
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
// 2026-06-23-005 U3: typed dead-letter termination path (R4+R8+AE-3).
mod plan_blocked_termination;
mod progress_steward;
mod r5_hard_gate_routing;
mod recovery_envelope_u7_u8;
mod replay_light_integration;
mod review_step_gate;
mod robot_skill;
// 2026-06-23 T2: `## RUNTIME CONFIG` block injection for `max_fix_rounds`.
// See `runtime_config_block.rs`.
mod runtime_config_block;
mod runtime_state_injection;
mod scope_enforcement;
mod scratchpad;
mod serial_lint;
mod stale_breaker;
mod state_machine;
mod structured_evidence;
mod termination;
mod text_fallback;
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
