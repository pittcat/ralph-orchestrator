//! 2026-07-06-004 plan U6 tests: wire the handoff envelope
//! extractor (U5) + the prompt injection gate (U4) into the
//! real isolated prompt chain.
//!
//! These tests assert two contracts:
//!
//! * `isolated_prompt_omits_handoff_envelope_by_default` — when
//!   `HandoffEnvelopeConfig` is the default (all flags false), the
//!   rendered prompt does NOT contain `## HANDOFF ENVELOPE`, even
//!   if recent events carry an envelope.
//! * `isolated_prompt_includes_handoff_envelope_when_enabled_and_event_has_payload` —
//!   when the master switch is on and an event carries a valid
//!   envelope, the rendered prompt starts with the
//!   `## HANDOFF ENVELOPE` block derived from that event.

#![allow(clippy::needless_update)]

use crate::config::HandoffEnvelopeConfig;
use crate::event_loop::prompt_helpers::{IsolatedPromptInputs, build_isolated_prompt_with_handoff};
use crate::handoff_envelope::HANDOFF_ENVELOPE_SCHEMA_VERSION;
use ralph_proto::{Event, HatId, Topic};

fn envelope_value(receiver: &str, step: &str) -> serde_json::Value {
    serde_json::json!({
        "plan_name": "2026-07-06-u6-fixture",
        "plan_path": "docs/plans/2026-07-06-u6-fixture.md",
        "task_id": "task-live-id",
        "task_key": "2026-07-06-u6-fixture:step-2:implement",
        "step": "step-2",
        "handoff_envelope": {
            "schema_version": HANDOFF_ENVELOPE_SCHEMA_VERSION,
            "root_goal": "ship the plan without regressions",
            "plan": {
                "name": "2026-07-06-u6-fixture",
                "path": "docs/plans/2026-07-06-u6-fixture.md",
                "current_step": step,
                "completed_steps": ["step-1"]
            },
            "state": {
                "current_status": "ready_for_review",
                "last_signal": "work.done",
                "blocking_reason": null
            },
            "receiver_contract": {
                "to_hat": receiver,
                "must_do": ["review step-2"],
                "must_not_do": ["regress step-1"],
                "success_signal": "work.done",
                "failure_signal": "work.failed"
            }
        }
    })
}

fn envelope_event(payload: serde_json::Value) -> Event {
    Event {
        topic: Topic::new("work.done"),
        payload: payload.to_string(),
        source: Some(HatId::new("executor".to_string())),
        target: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    }
}

#[test]
fn isolated_prompt_omits_handoff_envelope_by_default() {
    // Default config: all flags false. Even though the recent
    // events carry a valid envelope, the prompt must NOT include
    // the rendered block.
    let events = vec![envelope_event(envelope_value(
        "goal-alignment-reviewer",
        "step-2",
    ))];
    let inputs = IsolatedPromptInputs {
        base_prompt: "BASE PROMPT".to_string(),
        events: &events,
        current_hat: "goal-alignment-reviewer",
        config: &HandoffEnvelopeConfig::default(),
    };
    let rendered = build_isolated_prompt_with_handoff(inputs);
    assert!(
        !rendered.contains("## HANDOFF ENVELOPE"),
        "default config must not inject; got:\n{rendered}"
    );
    assert!(rendered.ends_with("BASE PROMPT"));
}

#[test]
fn isolated_prompt_includes_handoff_envelope_when_enabled_and_event_has_payload() {
    // Master switch + prompt_injection enabled; recent events
    // carry a valid envelope. The prompt must include the
    // rendered block at the very top.
    let events = vec![envelope_event(envelope_value(
        "goal-alignment-reviewer",
        "step-2",
    ))];
    let config = HandoffEnvelopeConfig {
        enabled: true,
        prompt_injection: true,
        validate_payload: false,
        emit_result_summary: false,
    };
    let inputs = IsolatedPromptInputs {
        base_prompt: "BASE PROMPT".to_string(),
        events: &events,
        current_hat: "goal-alignment-reviewer",
        config: &config,
    };
    let rendered = build_isolated_prompt_with_handoff(inputs);
    assert!(
        rendered.starts_with("## HANDOFF ENVELOPE\n"),
        "enabled + payload must prepend the rendered block; got first line: {:?}",
        rendered.lines().next()
    );
    assert!(rendered.contains("Receiver: goal-alignment-reviewer"));
    assert!(rendered.contains("Current step: step-2"));
    assert!(
        rendered.ends_with("BASE PROMPT"),
        "prepended block must keep the original prompt body at the end"
    );
}

#[test]
fn isolated_prompt_with_enabled_but_no_envelope_events_stays_clean() {
    // Master switch + prompt_injection enabled, but no recent
    // event carries a valid envelope. The helper must stay
    // silent (no ## HANDOFF ENVELOPE block) — the gate is
    // "enabled AND envelope present".
    let events = vec![envelope_event(serde_json::json!({"plan_name": "p"}))];
    let config = HandoffEnvelopeConfig {
        enabled: true,
        prompt_injection: true,
        validate_payload: false,
        emit_result_summary: false,
    };
    let inputs = IsolatedPromptInputs {
        base_prompt: "BASE PROMPT".to_string(),
        events: &events,
        current_hat: "goal-alignment-reviewer",
        config: &config,
    };
    let rendered = build_isolated_prompt_with_handoff(inputs);
    assert!(
        !rendered.contains("## HANDOFF ENVELOPE"),
        "missing envelope must short-circuit; got:\n{rendered}"
    );
    assert_eq!(rendered, "BASE PROMPT");
}
