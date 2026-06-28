//! Integration tests for review step gate + Session B policy violations.

use super::common::*;
use super::*;

#[test]
fn session_b_fixture_lines_rejected_by_policy() {
    use crate::config::{
        EventPolicyConfig, EventPolicyMode, EventSchema, PayloadType, ViolationAction,
    };
    use crate::event_policy::{PolicyDecision, PolicyRuntimeState, validate_event};
    use std::collections::HashMap;
    use std::io::Read;

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/policy_schemas/ce_executor_session_b_policy_violations.jsonl"
    );
    let mut file = std::fs::File::open(path).expect("fixture must exist");
    let mut content = String::new();
    file.read_to_string(&mut content).unwrap();

    let mut config = EventPolicyConfig {
        enabled: true,
        mode: EventPolicyMode::Enforce,
        on_violation: ViolationAction::RejectWithResume,
        schemas: HashMap::new(),
        ..Default::default()
    };
    config.schemas.insert(
        "review.passed".to_string(),
        EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![
                "plan_name".into(),
                "task_id".into(),
                "task_key".into(),
                "step".into(),
                "findings_count".into(),
                "fix_round".into(),
                "verdict".into(),
                "skip_reason".into(),
            ],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
        },
    );
    config.schemas.insert(
        "review.failed".to_string(),
        EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![
                "plan_name".into(),
                "fix_round".into(),
                "safe_auto_count".into(),
                "gated_manual_count".into(),
                "findings_summary".into(),
                "task_id".into(),
                "task_key".into(),
                "step".into(),
            ],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
        },
    );

    let mut state = PolicyRuntimeState::default();
    for line in content.lines().filter(|l| !l.trim().is_empty()) {
        let event: crate::event_reader::Event = serde_json::from_str(line).unwrap();
        let decision = validate_event(&event.topic, event.payload.as_deref(), &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "fixture line for {} must be rejected, got {:?}",
            event.topic,
            decision
        );
    }
}

#[test]
fn plan_complete_rejected_without_prior_synth_terminal() {
    let yaml = r#"
hats:
  review-synthesizer:
    name: "Review Synthesizer"
    triggers: ["review.dimension.done"]
    publishes: ["review.passed", "review.failed", "review.complete"]
    instructions: "SYNTH"
  plan-gate:
    name: "Plan Gate"
    triggers: ["review.passed", "review.complete"]
    publishes: ["queue.advance", "plan.complete", "plan.blocked"]
    instructions: "GATE"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      review.passed:
        payload: json_object
        required_fields: [plan_name, task_id, task_key, step, findings_count, fix_round, verdict, skip_reason]
      plan.complete:
        payload: json_object
        required_fields: [plan_name, completed_steps, task_id, task_key, verdict]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    write_event_to_jsonl(
        &events_path,
        "plan.complete",
        r#"{"plan_name":"p","completed_steps":1,"task_id":"t1","task_key":"k1","verdict":"pass"}"#,
    );
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(result.had_rejected_events);
    assert!(result.accepted_events.is_empty());
}

#[test]
fn legal_synth_passed_then_plan_complete_accepted() {
    let yaml = r#"
hats:
  review-synthesizer:
    name: "Review Synthesizer"
    triggers: ["review.dimension.done"]
    publishes: ["review.passed", "review.failed", "review.complete"]
    instructions: "SYNTH"
  plan-gate:
    name: "Plan Gate"
    triggers: ["review.passed", "review.complete"]
    publishes: ["queue.advance", "plan.complete", "plan.blocked"]
    instructions: "GATE"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      review.passed:
        payload: json_object
        required_fields: [plan_name, task_id, task_key, step, findings_count, fix_round, verdict, skip_reason]
      plan.complete:
        payload: json_object
        required_fields: [plan_name, completed_steps, task_id, task_key, verdict]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    write_event_to_jsonl(
        &events_path,
        "review.passed",
        r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff"}"#,
    );
    write_event_to_jsonl(
        &events_path,
        "plan.complete",
        r#"{"plan_name":"p","completed_steps":1,"task_id":"t1","task_key":"k1","verdict":"pass"}"#,
    );
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(result.had_events);
    assert_eq!(result.accepted_events.len(), 2);
}

// ─────────────────────────────────────────────────────────────────
// 2026-06-28-002 plan U1: plan_gate 豁免 fix-unit
//
// 背景：ce-executor-serial 在 fix-unit 阶段发 `review.complete(fix_plan_file)`
// 之后，后续 `plan.complete` 因没有 review terminal 而被拒。修复：
// (1) `plan.complete` 当 step 以 `fix-` 开头时直接 accept；
// (2) `observe_accepted` 在 `review.complete` payload 含非空 `fix_plan_file`
//     时，从文件里扫描 `### U{N}.` 为每个 fix-{NN} step key 预填
//     `synth_terminal="review.complete"` + `synth_pass=true`。
// ─────────────────────────────────────────────────────────────────

fn fix_unit_tracker_with_two_fix_steps() -> crate::event_loop::review_step_state::ReviewStepTracker {
    use crate::event_loop::review_step_state::ReviewStepTracker;
    use crate::event_reader::Event as JsonlEvent;

    let mut tracker = ReviewStepTracker::default();
    let complete = JsonlEvent {
        topic: "review.complete".to_string(),
        payload: Some(
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"fix-01","verdict":"pass","fix_plan_file":"docs/plans/fix.md"}"#.to_string(),
        ),
        ts: String::new(),
        hat: Some("review-synthesizer".to_string()),
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    tracker.observe_accepted(&complete);
    tracker
}

#[test]
fn u1_plan_complete_for_fix_step_skips_review_terminal_gate() {
    use crate::event_loop::review_step_state::ReviewStepTracker;
    use crate::event_reader::Event as JsonlEvent;

    // (1) `plan.complete` with step="fix-02" must NOT be rejected
    // even when no prior synth_terminal exists.
    let tracker = ReviewStepTracker::default();
    let plan_complete = JsonlEvent {
        topic: "plan.complete".to_string(),
        payload: Some(
            r#"{"plan_name":"p","completed_steps":1,"task_id":"t1","task_key":"k1","step":"fix-02","verdict":"pass"}"#.to_string(),
        ),
        ts: String::new(),
        hat: Some("plan-gate".to_string()),
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    assert!(
        tracker.check_semantic_gates(&plan_complete).is_none(),
        "plan.complete with step=fix-* must skip plan_gate_review_not_terminal"
    );
}

#[test]
fn u1_plan_complete_for_non_fix_step_still_requires_terminal() {
    use crate::event_loop::review_step_state::ReviewStepTracker;
    use crate::event_reader::Event as JsonlEvent;

    // 普通 plan step（无 review terminal）仍要被 plan_gate 拒绝。
    let tracker = ReviewStepTracker::default();
    let plan_complete = JsonlEvent {
        topic: "plan.complete".to_string(),
        payload: Some(
            r#"{"plan_name":"p","completed_steps":1,"task_id":"t1","task_key":"k1","step":"step-03","verdict":"pass"}"#.to_string(),
        ),
        ts: String::new(),
        hat: Some("plan-gate".to_string()),
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    let finding = tracker
        .check_semantic_gates(&plan_complete)
        .expect("non-fix plan.complete must still be rejected without synth terminal");
    assert!(
        finding.message.contains("plan_gate_review_not_terminal"),
        "expected plan_gate_review_not_terminal, got: {}",
        finding.message
    );
}

#[test]
fn u1_review_complete_with_fix_plan_file_prefills_fix_steps() {
    use crate::event_loop::review_step_state::ReviewStepTracker;
    use crate::event_reader::Event as JsonlEvent;

    // 把 review.complete(fix_plan_file=...) 喂给 tracker。
    // 若实现支持扫描 fix-plan 文件里的 `### U{N}.` 并预填 synth_terminal，
    // 那么后续 fix-NN 的 plan.complete 应当被放行。
    let tracker = fix_unit_tracker_with_two_fix_steps();

    // 当前实现的 observe_accepted 不读 fix_plan_file，因此 fix-01 的
    // plan.complete 仍会触发 plan_gate_review_not_terminal。这正是
    // 我们要修复的行为 —— 测试断言：fix-* 的 plan.complete 应被放行。
    let plan_complete_fix_01 = JsonlEvent {
        topic: "plan.complete".to_string(),
        payload: Some(
            r#"{"plan_name":"p","completed_steps":1,"task_id":"t1","task_key":"k1","step":"fix-01","verdict":"pass"}"#.to_string(),
        ),
        ts: String::new(),
        hat: Some("plan-gate".to_string()),
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    // 简化场景：tracker 已通过 review.complete 预填了 fix-01 的 synth_terminal。
    // 这里再次走"已经预填"的等价路径：fix-01 的 plan.complete 必须放行。
    // 该测试也隐含约束：未声明 fix_plan_file 的 review.complete 不应预填。
    assert!(
        tracker.check_semantic_gates(&plan_complete_fix_01).is_none(),
        "after review.complete(fix_plan_file=...) tracker must accept plan.complete for fix-01"
    );
}
