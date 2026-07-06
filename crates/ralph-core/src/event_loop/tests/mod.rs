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
mod handoff_dispatch;
mod hat_backend;
mod hat_exhaustion;
mod hat_lifecycle_integration;
mod hat_lifecycle_jsonl_e2e;
mod incident_fixture;
mod initialization;
mod isolated_complex_regression;
mod isolated_wave_budget;
mod loop_context;
mod next_hat_topic_preemption;
mod objective;
mod origin_guard;
mod payload_types;
mod persistent_mode;
// 2026-07-04-001 plan U16 (KTD-13): task.resume consumer triggers
// routing validation tests.
mod u16_resume_routing;
// 2026-06-28 P1-1: budget-exhaustion escalation test.
mod p1_1_plan_blocked_escalation;
// 2026-06-23-005 U3: typed dead-letter termination path (R4+R8+AE-3).
mod plan_blocked_termination;
mod progress_steward;
/// 2026-07-06 plan U12: `progress_steward.enabled==false` ⇒ no
/// `loop.stalled` wake from any code path. Pins the
/// consumer_stall_repeat gate that U12 adds on top of the
/// pre-existing U5 stall-detector gate.
mod progress_steward_disabled;
mod r5_hard_gate_routing;
mod recovery_envelope_u7_u8;
mod replay_light_integration;
mod review_step_gate;
// 2026-06-23 T2: `## RUNTIME CONFIG` block injection for `max_residuals`.
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
/// U11 (2026-06-27 mechanism foundation): `archive_state_for_loop`
/// wired into `EventLoop::with_context_and_diagnostics` so worktree
/// reuse archives previous-loop `.ralph/*.jsonl` before
/// `IdempotentLog::open` writes the new `loop-version.json`.
mod u11_wiring;
mod wave_context_env_var;
mod wave_context_injection;
mod wave_isolated_scope;
mod wave_policy_rejection;
mod wave_recovery_timeout;
mod wave_results;
mod workflow_guard;

/// U10 (2026-06-27-002 plan): `VerdictGate` is the
/// sole termination dispatcher. The stage pipeline's
/// `is_terminal` probe writes a loop-termination
/// record when a `LOOP_COMPLETE` event clears the gate.
mod u10_verdict_dispatcher;
/// U13 (2026-06-27-002 plan): archive failures
/// abort the loop start. `with_context_and_diagnostics`
/// returns `Err` instead of warning + continuing.
mod u13_archive_fail_closed;
/// U2 (2026-06-27-002 plan): `publish_event` routes
/// through the single `evaluate_emit_gate` facade.
mod u2_publish_emit_gate;
/// U3 (2026-06-27-002 plan): `process_parse_result` /
/// `process_events_from_jsonl` route through the same
/// `evaluate_emit_gate` facade so the JSONL ingest path
/// cannot bypass the gate.
mod u3_jsonl_emit_gate;
/// U4 (2026-06-27-002 plan): `stage_pipeline` re-exports
/// `repair_flow::RepairStateMachine`; the stub is gone.
mod u4_repair_sm_unify;
/// 2026-07-06-004 plan U4: prompt injection no-op wiring test
/// (helper is gated on `HandoffEnvelopeConfig::enabled &&
/// prompt_injection`, no-op otherwise).
mod u4_handoff_envelope_prompt;
/// U6 (2026-06-27-001 plan): StagePipeline is wired into
/// EventLoop::publish_event so every hat emit passes through
/// the locked default stages before reaching the event bus.
mod u6_wiring;
/// U7 (2026-06-27-002 plan): the U6 `RepairStreamSink`
/// is wired into both `publish_event` and
/// `process_parse_result`. The bus never receives a
/// repair topic.
mod u7_repair_sink_wiring;
/// U8 (2026-06-27-002 plan): loop start invokes
/// `relocate_legacy_tasks`; `repair.close` clears the
/// per-task stall recovery counter.
mod u8_legacy_relocate_and_close;
/// U9 (2026-06-27-002 plan): retire legacy
/// `verdict_gate.additional_topics: ["report.done"]`
/// from schema + runtime. Only `LOOP_COMPLETE`
/// terminates the dispatcher.
mod u9_verdict_legacy_retire;
