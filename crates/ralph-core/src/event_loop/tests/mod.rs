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
mod fallback_recovery_fail_close;
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
mod execution_contract_commit_boundary;
mod handoff_dispatch;
mod hat_backend;
mod hat_exhaustion;
mod hat_lifecycle_integration;
mod hat_lifecycle_jsonl_e2e;
mod incident_fixture;
mod initialization;
mod isolated_complex_regression;
/// Plan 2026-07-28-001 U3: generic isolated fixture for the
/// commit-aware over-emit recovery contract. The three production
/// scenarios (committed-first / zero-commit / terminal priority) plus
/// the breaker-reset regression live here — distinct from the U2
/// multi-hat complex regression so the plan-specified nextest
/// substring can pick them out independently.
mod isolated_over_emit_commit;
mod isolated_wave_budget;
mod loop_context;
mod next_hat_topic_preemption;
mod objective;
mod origin_guard;
mod payload_types;
mod persistent_mode;
mod post_terminal_rejection;
/// 2026-07-26-001 plan U2: unit tests for the new
/// `EventLoop::prompt_preview` structured API.
mod preview_api;
/// 2026-07-26-001 plan U1: characterization tests that pin the
/// current auto-inject skill set before introducing the
/// `PromptPreview` API.
mod preview_characterization;
/// 2026-07-07-002 plan Unit 8: protocol-violation bounded retry +
/// fail-close invariants. Regression guard for the
/// `clear_rejection_keys_for_hat` carve-out (DEV-006) that lets
/// the bounded budget actually accumulate to
/// `U2_REJECTION_RETRY_LIMIT + 1` so the runtime falls through
/// to `plan.blocked(reason=protocol_violation_repeated:...)`.
mod protocol_violation_recovery;
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
/// 2026-07-24-005 U1 review fix: production `plan.blocked`
/// synthesis paths must target `reporter`.
mod u1_plan_blocked_reporter_target;
// 2026-06-23 T2: `## RUNTIME CONFIG` block injection for `max_residuals`.
// See `runtime_config_block.rs`.
mod event_policy_lint_resume;
mod runtime_config_block;
mod runtime_state_injection;
mod scope_enforcement;
mod scratchpad;
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
/// 2026-07-09-003 plan U3: `prepend_trigger_context` is
/// wired into the isolated build_prompt path. The helper is
/// a no-op when the schema has no `trigger_context`
/// declaration or the current hat does not subscribe to the
/// source topic (SC6 / R3 / R21 / R22 / R29).
mod u3_trigger_context_prompt;
/// 2026-07-06-004 plan U4: prompt injection no-op wiring test
/// (helper is gated on `HandoffEnvelopeConfig::enabled &&
/// prompt_injection`, no-op otherwise).
mod u4_handoff_envelope_prompt;
/// U4 (2026-06-27-002 plan): `stage_pipeline` re-exports
/// `repair_flow::RepairStateMachine`; the stub is gone.
mod u4_repair_sm_unify;
/// 2026-07-06-004 plan U6: wire the handoff envelope extractor
/// into the isolated prompt chain. The helper is a no-op unless
/// `enabled && prompt_injection` AND recent events carry a valid
/// envelope.
mod u6_handoff_envelope_wiring;
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
