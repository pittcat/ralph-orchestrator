use crate::config::{EventLoopConfig, EventPolicyConfig, EventSchema, RalphConfig};
use crate::event_loop::flow_declaration::FlowDeclaration;
use crate::event_loop::flow_wiring::build_terminal_target_contracts_from_loop_config;
use crate::event_loop::stage_pipeline::{
    EmitStage, FlowStep, RepairStateMachine, StageContext, StagePipeline, StageReject,
};
use ralph_proto::Event;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Counter stage that accepts every event and increments a counter.
struct AcceptCounter {
    name: &'static str,
    counter: Arc<AtomicUsize>,
}

impl AcceptCounter {
    fn new(name: &'static str, counter: Arc<AtomicUsize>) -> Self {
        Self { name, counter }
    }
}

impl EmitStage for AcceptCounter {
    fn name(&self) -> &'static str {
        self.name
    }

    fn check(&self, _ctx: &mut StageContext, _event: &Event) -> Result<(), StageReject> {
        self.counter.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Stage that rejects every event with a fixed reason.
struct AlwaysReject {
    name: &'static str,
}

impl EmitStage for AlwaysReject {
    fn name(&self) -> &'static str {
        self.name
    }

    fn check(&self, _ctx: &mut StageContext, _event: &Event) -> Result<(), StageReject> {
        Err(StageReject::new(self.name, "always_reject"))
    }
}

fn dummy_event() -> Event {
    Event::new("work.ready", "{}")
}

fn dummy_ctx(repair: &mut RepairStateMachine) -> StageContext<'_> {
    StageContext::for_test_machine(FlowStep::new("unit_loop"), "loop-1", 1, repair)
}

#[test]
fn stage_pipeline_skeleton_empty_accepts_everything() {
    let mut repair = RepairStateMachine::default();
    let pipeline = StagePipeline::default();
    let event = dummy_event();
    assert!(pipeline.run(&mut dummy_ctx(&mut repair), &event).is_ok());
}

#[test]
fn stage_pipeline_skeleton_three_counters_run_in_order() {
    let mut repair = RepairStateMachine::default();
    let a = Arc::new(AtomicUsize::new(0));
    let b = Arc::new(AtomicUsize::new(0));
    let c = Arc::new(AtomicUsize::new(0));

    let pipeline = StagePipeline::new(vec![
        Box::new(AcceptCounter::new("Alpha", a.clone())),
        Box::new(AcceptCounter::new("Beta", b.clone())),
        Box::new(AcceptCounter::new("Gamma", c.clone())),
    ]);

    let event = dummy_event();
    assert!(pipeline.run(&mut dummy_ctx(&mut repair), &event).is_ok());

    assert_eq!(a.load(Ordering::SeqCst), 1);
    assert_eq!(b.load(Ordering::SeqCst), 1);
    assert_eq!(c.load(Ordering::SeqCst), 1);
}

#[test]
fn stage_pipeline_skeleton_reject_short_circuits() {
    let mut repair = RepairStateMachine::default();
    let a = Arc::new(AtomicUsize::new(0));
    let b = Arc::new(AtomicUsize::new(0));
    let c = Arc::new(AtomicUsize::new(0));

    let pipeline = StagePipeline::new(vec![
        Box::new(AcceptCounter::new("Alpha", a.clone())),
        Box::new(AlwaysReject { name: "Beta" }),
        Box::new(AcceptCounter::new("Gamma", c.clone())),
    ]);

    let event = dummy_event();
    let err = pipeline
        .run(&mut dummy_ctx(&mut repair), &event)
        .unwrap_err();

    assert_eq!(err.stage_name, "Beta");
    assert_eq!(err.reason_code, "always_reject");
    assert_eq!(a.load(Ordering::SeqCst), 1);
    assert_eq!(b.load(Ordering::SeqCst), 0);
    assert_eq!(c.load(Ordering::SeqCst), 0);
}

// Dummy named stages used only for order-assertion compile-time tests.
struct ArchiveVersionStage;
struct RepairDispatchStage;
struct EmitSchemaGateStage;
struct FlowStepScopeStage;
struct VerdictGateStage;

impl EmitStage for ArchiveVersionStage {
    fn name(&self) -> &'static str {
        "ArchiveVersion"
    }
    fn check(&self, _ctx: &mut StageContext, _event: &Event) -> Result<(), StageReject> {
        Ok(())
    }
}

impl EmitStage for RepairDispatchStage {
    fn name(&self) -> &'static str {
        "RepairDispatch"
    }
    fn check(&self, _ctx: &mut StageContext, _event: &Event) -> Result<(), StageReject> {
        Ok(())
    }
}

impl EmitStage for EmitSchemaGateStage {
    fn name(&self) -> &'static str {
        "EmitSchemaGate"
    }
    fn check(&self, _ctx: &mut StageContext, _event: &Event) -> Result<(), StageReject> {
        Ok(())
    }
}

impl EmitStage for FlowStepScopeStage {
    fn name(&self) -> &'static str {
        "FlowStepScope"
    }
    fn check(&self, _ctx: &mut StageContext, _event: &Event) -> Result<(), StageReject> {
        Ok(())
    }
}

impl EmitStage for VerdictGateStage {
    fn name(&self) -> &'static str {
        "VerdictGate"
    }
    fn check(&self, _ctx: &mut StageContext, _event: &Event) -> Result<(), StageReject> {
        Ok(())
    }
}

#[test]
fn stage_pipeline_skeleton_locked_order_matches() {
    let pipeline = StagePipeline::new(vec![
        Box::new(ArchiveVersionStage),
        Box::new(RepairDispatchStage),
        Box::new(EmitSchemaGateStage),
        Box::new(FlowStepScopeStage),
        // P1-4 (2026-06-27 adversarial review):
        // the locked emit order now also
        // includes `StepCloseObligation`
        // between `FlowStepScope` and
        // `VerdictGate`.
        Box::new(
            crate::event_loop::stages::step_close_obligation_stage::StepCloseObligationStage::new(
                FlowDeclaration::from_yaml(
                    "mechanism:\n  flow:\n    type: declared\n    version: 1\n    steps: []\n",
                )
                .unwrap(),
            ),
        ),
        Box::new(VerdictGateStage),
    ]);

    // P1-4 (2026-06-27 adversarial review): the
    // locked emit order now also includes
    // `StepCloseObligation` between
    // `FlowStepScope` and `VerdictGate`. The
    // `ArchiveVersion` stage is a loop-start hook,
    // not an emit stage, so it does not appear in
    // the runtime pipeline.
    crate::assert_stage_order!(
        pipeline,
        [
            ArchiveVersion,
            RepairDispatch,
            EmitSchemaGate,
            FlowStepScope,
            StepCloseObligation,
            VerdictGate
        ]
    );
}

#[test]
fn stage_pipeline_order_default_matches_locked_emit_order() {
    use crate::event_loop::flow_declaration::FlowDeclaration;

    let flow = FlowDeclaration::from_yaml(
        r"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: unit_loop
        allowed_emits: [work.ready]
",
    )
    .unwrap();
    let pipeline = StagePipeline::with_default_stages(flow);
    assert_eq!(
        pipeline.names(),
        // P1-4 (2026-06-27 adversarial review):
        // `StepCloseObligation` is now part of
        // the locked emit order (between
        // `FlowStepScope` and `VerdictGate`).
        // Plan 2026-08-16-1015 U1: `TerminalTargetGuard` is
        // inserted after `EmitSchemaGate` (schema validation first,
        // then terminal-target contract check).
        vec![
            "RepairDispatch",
            "EmitSchemaGate",
            "TerminalTargetGuard",
            "FlowStepScope",
            "StepCloseObligation",
            "VerdictGate"
        ]
    );
}

#[test]
fn hat_only_pipeline_omits_flow_step_scope_and_accepts_plan_ready() {
    use crate::event_loop::emit_gate::{EmitGateOutcome, evaluate_emit_gate};
    use crate::event_loop::repair_flow::RepairStateMachine;
    use ralph_proto::Event;

    let pipeline = StagePipeline::with_hat_only_stages_for_loop_config(None);
    assert_eq!(
        pipeline.names(),
        // Plan 2026-08-16-1015 U1: `TerminalTargetGuard` is inserted
        // after `EmitSchemaGate` (schema validation first, then
        // terminal-target contract check).
        vec![
            "RepairDispatch",
            "EmitSchemaGate",
            "TerminalTargetGuard",
            "VerdictGate"
        ]
    );

    let mut sm: std::collections::HashMap<String, RepairStateMachine> =
        std::collections::HashMap::new();
    let mut ctx =
        StageContext::with_pipeline(FlowStep::new("unit_loop"), "loop-1", 1, &mut sm, &pipeline);
    let event = Event::new(
        "plan.ready",
        r#"{"plan_name":"p","plan_path":"docs/plans/p.md","plan_revised":false,"review_summary":"ok"}"#,
    );
    let outcome = evaluate_emit_gate(&mut ctx, &event);
    assert!(
        matches!(outcome, EmitGateOutcome::AcceptMainBus),
        "hat-only pipeline must not reject plan.ready via FlowStepScope: {outcome:?}"
    );
}

#[test]
fn stage_pipeline_skeleton_wrong_order_fails_at_runtime() {
    let pipeline = StagePipeline::new(vec![
        Box::new(RepairDispatchStage),
        Box::new(ArchiveVersionStage),
    ]);

    let actual = pipeline.names();
    let expected: &[&str] = &["ArchiveVersion", "RepairDispatch"];
    assert_ne!(
        actual, expected,
        "deliberately wrong order to test assertion utility"
    );
}

// Plan 2026-08-16-1015 U1: all three pipeline constructors must
// inject `TerminalTargetGuardStage` so the guard actually fires in
// production runs (previously it was only unit-tested in isolation).
//
// Slot: after `EmitSchemaGate` and before the next stage in each
// constructor — schema validation fires first, then terminal-target
// guard, then preset-specific stages.
//
// Test entry: `cargo nextest run -p ralph-core -- test_stage_pipeline_constructors_wire_terminal_target_guard`

/// Build a minimal `RalphConfig` with one schema entry.
fn config_with_required_target_hat(topic: &str, required_target_hat: Option<&str>) -> RalphConfig {
    let mut cfg = RalphConfig::default();
    let mut schemas = std::collections::HashMap::new();
    schemas.insert(
        topic.to_string(),
        EventSchema {
            required_target_hat: required_target_hat.map(String::from),
            ..Default::default()
        },
    );
    cfg.event_loop.event_policy = Some(EventPolicyConfig {
        enabled: true,
        schemas,
        ..EventPolicyConfig::default()
    });
    cfg
}

/// Build an `EventLoopConfig` with no schemas (empty contract map).
#[allow(dead_code)]
fn empty_event_loop_config() -> EventLoopConfig {
    EventLoopConfig::default()
}

/// Helper: assert `TerminalTargetGuard` is in the pipeline names.
fn assert_terminal_target_guard_present(pipeline: &StagePipeline) {
    assert!(
        pipeline.names().contains(&"TerminalTargetGuard"),
        "pipeline.names() = {:?}",
        pipeline.names()
    );
}

#[test]
fn test_stage_pipeline_constructors_wire_terminal_target_guard() {
    // config_a: `report.done` requires `reporter`
    let config_a = config_with_required_target_hat("report.done", Some("reporter"));

    // config_b: no schemas at all
    let config_b = RalphConfig::default();

    // config_c: `report.done` has `required_target_hat: ""` (empty string —
    // guard must omit it, per `!target.is_empty()` semantic)
    let config_c = config_with_required_target_hat("report.done", Some(""));

    // --- Helper unit tests (build_terminal_target_contracts_from_loop_config) ---
    let contracts_a = build_terminal_target_contracts_from_loop_config(&config_a.event_loop);
    assert_eq!(
        contracts_a.get("report.done"),
        Some(&"reporter".to_string()),
        "config_a: report.done should map to reporter"
    );

    let contracts_b = build_terminal_target_contracts_from_loop_config(&config_b.event_loop);
    assert!(
        contracts_b.is_empty(),
        "config_b (no schemas): helper should return empty map"
    );

    let contracts_c = build_terminal_target_contracts_from_loop_config(&config_c.event_loop);
    assert!(
        !contracts_c.contains_key("report.done"),
        "config_c (empty-string contract): helper must omit the topic"
    );

    // --- Constructor wiring: with_default_stages_for_loop_config ---
    let flow = FlowDeclaration::from_yaml(
        r"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: unit_loop
        allowed_emits: [work.ready]
",
    )
    .unwrap();

    let pipeline_default_a = StagePipeline::with_default_stages_for_loop_config(
        flow.clone(),
        Some(&config_a.event_loop),
    );
    assert_terminal_target_guard_present(&pipeline_default_a);

    let pipeline_default_none =
        StagePipeline::with_default_stages_for_loop_config(flow.clone(), None);
    assert_terminal_target_guard_present(&pipeline_default_none);

    // --- Constructor wiring: with_phase_authority_stages_for_loop_config ---
    use crate::event_loop::phase_authority::WorkflowPhaseAuthority;
    let authority = WorkflowPhaseAuthority::disabled();

    let pipeline_phase_a = StagePipeline::with_phase_authority_stages_for_loop_config(
        flow.clone(),
        Some(&config_a.event_loop),
        Arc::new(authority.clone()),
    );
    assert_terminal_target_guard_present(&pipeline_phase_a);

    let pipeline_phase_none = StagePipeline::with_phase_authority_stages_for_loop_config(
        flow.clone(),
        None,
        Arc::new(authority),
    );
    assert_terminal_target_guard_present(&pipeline_phase_none);

    // --- Constructor wiring: with_hat_only_stages_for_loop_config ---
    let pipeline_hat_a =
        StagePipeline::with_hat_only_stages_for_loop_config(Some(&config_a.event_loop));
    assert_terminal_target_guard_present(&pipeline_hat_a);

    let pipeline_hat_none = StagePipeline::with_hat_only_stages_for_loop_config(None);
    assert_terminal_target_guard_present(&pipeline_hat_none);
}

// -------------------------------------------------------------------------
// PMI-007 (post-merge-converge / concurrency-idempotency):
//
// The three StagePipeline constructors
// (`with_default_stages_for_loop_config`,
// `with_hat_only_stages_for_loop_config`,
// `with_phase_authority_stages_for_loop_config`) each document the
// `TerminalTargetGuard BEFORE VerdictGate` invariant in source comments
// but enforce it ONLY via two unit-test `assert_eq!` comparisons against
// `pipeline.names()` (`stage_pipeline_order_default_matches_locked_emit_order`
// and `hat_only_pipeline_omits_flow_step_scope_and_accepts_plan_ready`).
// Neither `crate::assert_stage_order!` nor any runtime order assertion
// lives inside the production constructor path. A future refactor that
// swaps the two stages breaks only when those two specific tests run;
// no compile-time and no runtime guard prevents the swap.
//
// Note on behavioural demo: swapping `VerdictGateStage` and
// `TerminalTargetGuardStage` does NOT cause wrong-target terminal
// emits to slip through — `VerdictGateStage::check` always returns
// `Ok(())`, so the next stage is always reached and
// `TerminalTargetGuardStage::check` rejects wrong-target. The defect
// is purely structural (the comment-only order constraint has no
// enforcement); the test below reproduces that structural absence.
//
// This test PASSES today (bug is observable). It FAILS after the
// U-fix adds enforcement (compile-time `crate::assert_stage_order!`
// inside any production constructor body, OR a runtime
// `debug_assert!` inside `StagePipeline::run` checking
// TerminalTargetGuard precedes VerdictGate).
// -------------------------------------------------------------------------

/// Locate the body of `fn_signature` in `src` by brace-matching.
fn extract_fn_body(src: &str, fn_signature: &str) -> String {
    let Some(rel_start) = src.find(fn_signature) else {
        return String::new();
    };
    let Some(open_off) = src[rel_start..].find('{') else {
        return String::new();
    };
    let open_abs = rel_start + open_off;
    let bytes = src.as_bytes();
    let mut depth: usize = 0;
    let mut i = open_abs;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return src[open_abs..=i].to_string();
                }
            }
            _ => {}
        }
        i += 1;
    }
    String::new()
}

#[test]
fn pmi007_production_constructors_lack_compile_time_or_runtime_stage_order_lock() {
    const STAGE_PIPELINE_SRC: &str = include_str!("../stage_pipeline.rs");

    // Locate the body of `StagePipeline::run` so we can later assert
    // it has no runtime order check on TerminalTargetGuard /
    // VerdictGate (PMI-007 fix would add such a check).
    let run_body = extract_fn_body(
        STAGE_PIPELINE_SRC,
        "pub fn run(&self, ctx: &mut StageContext, event: &Event)",
    );
    assert!(
        !run_body.is_empty(),
        "could not locate StagePipeline::run in stage_pipeline.rs",
    );
    // Today `run` is a flat loop. After fix, the body must contain
    // either a `debug_assert!` or `assert!` mentioning both stages.
    let run_has_order_assertion = run_body.contains("debug_assert")
        || (run_body.contains("assert!")
            && (run_body.contains("TerminalTargetGuard")
                || run_body.contains("VerdictGate")));
    assert!(
        !run_has_order_assertion,
        "PMI-007 fix landed: StagePipeline::run now runtime-checks \
         stage order. The bug is closed at runtime.",
    );

    // Each production constructor body must NOT contain
    // `assert_stage_order!`. Today none do; once U-fix lands, at
    // least one calls the macro and this assertion reverses.
    let ctors = [
        "with_default_stages_for_loop_config",
        "with_hat_only_stages_for_loop_config",
        "with_phase_authority_stages_for_loop_config",
    ];

    for ctor in &ctors {
        let body = extract_fn_body(STAGE_PIPELINE_SRC, &format!("pub fn {ctor}"));
        assert!(
            !body.is_empty(),
            "could not locate body for {ctor}",
        );
        // Body extends to closing brace of the function, which is
        // fine — we just need to scan its content for any
        // `assert_stage_order!` invocation.
        assert!(
            !body.contains("assert_stage_order!"),
            "PMI-007 fix landed: production constructor `{ctor}` \
             now invokes `assert_stage_order!`. The bug is closed at \
             compile time. Body excerpt: {:.240}…",
            body,
        );
    }

    // Sanity: the three constructors still produce pipelines with
    // TerminalTargetGuard in their `names()` (PMI-007 doesn't drop the
    // guard, only fails to lock its position).
    let flow = FlowDeclaration::from_yaml(
        "mechanism:\n  flow:\n    type: declared\n    version: 1\n    terminal_emits: [LOOP_COMPLETE]\n    steps: []\n",
    )
    .unwrap();
    let p_default =
        StagePipeline::with_default_stages_for_loop_config(flow.clone(), None);
    let p_hat_only = StagePipeline::with_hat_only_stages_for_loop_config(None);
    let p_phase = StagePipeline::with_phase_authority_stages_for_loop_config(
        flow,
        None,
        std::sync::Arc::new(
            crate::event_loop::phase_authority::WorkflowPhaseAuthority::disabled(),
        ),
    );
    assert!(p_default.names().contains(&"TerminalTargetGuard"));
    assert!(p_hat_only.names().contains(&"TerminalTargetGuard"));
    assert!(p_phase.names().contains(&"TerminalTargetGuard"));
}
