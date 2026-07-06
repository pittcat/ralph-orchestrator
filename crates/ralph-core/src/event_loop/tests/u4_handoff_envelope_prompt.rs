//! 2026-07-06-004 plan U4 tests: prompt injection no-op wiring.
//!
//! These tests target the small `prepend_handoff_envelope_if_enabled`
//! helper directly. The helper is intentionally a free function
//! keyed on the typed `HandoffEnvelopeConfig` so it can be exercised
//! without instantiating a full `EventLoop`. U6 will call the same
//! helper from inside the real prompt builder.

#![allow(clippy::needless_update)]

use crate::config::HandoffEnvelopeConfig;
use crate::event_loop::prepend_handoff_envelope_if_enabled;
use crate::handoff_envelope::{
    render_handoff_envelope_prompt, HandoffEnvelopePayload, HandoffEnvelopePlan,
    HandoffEnvelopeReceiverContract, HandoffEnvelopeState, HandoffEnvelopeView,
    HANDOFF_ENVELOPE_SCHEMA_VERSION,
};

fn envelope_fixture() -> HandoffEnvelopePayload {
    HandoffEnvelopePayload {
        schema_version: HANDOFF_ENVELOPE_SCHEMA_VERSION.to_string(),
        root_goal: "ship the plan without regressions".to_string(),
        plan: HandoffEnvelopePlan {
            name: "2026-07-06-u4-fixture".to_string(),
            path: "docs/plans/2026-07-06-u4-fixture.md".to_string(),
            current_step: "step-2".to_string(),
            completed_steps: vec!["step-1".to_string()],
        },
        state: HandoffEnvelopeState {
            current_status: "ready_for_review".to_string(),
            last_signal: "work.done".to_string(),
            blocking_reason: None,
        },
        receiver_contract: HandoffEnvelopeReceiverContract {
            to_hat: "executor".to_string(),
            must_do: vec!["complete step-2".to_string()],
            must_not_do: vec!["regress step-1".to_string()],
            success_signal: "work.done".to_string(),
            failure_signal: "work.failed".to_string(),
        },
    }
}

fn config(enabled: bool, prompt_injection: bool) -> HandoffEnvelopeConfig {
    HandoffEnvelopeConfig {
        enabled,
        prompt_injection,
        validate_payload: false,
        emit_result_summary: false,
    }
}

#[test]
fn handoff_envelope_prompt_is_noop_when_disabled() {
    let env = envelope_fixture();
    let view = HandoffEnvelopeView::from(&env);
    let out = prepend_handoff_envelope_if_enabled(
        "BASE PROMPT".to_string(),
        &config(false, true),
        Some(&view),
    );
    assert_eq!(out, "BASE PROMPT", "disabled config must short-circuit");
}

#[test]
fn handoff_envelope_prompt_is_noop_when_missing_payload() {
    let out = prepend_handoff_envelope_if_enabled(
        "BASE PROMPT".to_string(),
        &config(true, true),
        None,
    );
    assert_eq!(out, "BASE PROMPT", "missing envelope must short-circuit");
}

#[test]
fn handoff_envelope_prompt_is_prepended_when_enabled() {
    let env = envelope_fixture();
    let view = HandoffEnvelopeView::from(&env);
    let out = prepend_handoff_envelope_if_enabled(
        "BASE PROMPT".to_string(),
        &config(true, true),
        Some(&view),
    );
    assert!(
        out.starts_with("## HANDOFF ENVELOPE\n"),
        "enabled + payload must prepend the rendered block; got:\n{}",
        out
    );
    assert!(
        out.ends_with("BASE PROMPT"),
        "prepended block must keep the original prompt body"
    );
    assert!(out.contains("Root goal: ship the plan without regressions"));
    assert!(out.contains("Current step: step-2"));
    assert!(out.contains("- Receiver: executor"));
    // Sanity: the renderer + prepender must agree on the exact
    // opening block, so the test fails loudly if either side
    // drifts away from "## HANDOFF ENVELOPE\n". The prepender
    // joins the rendered block with `\n---\n\n` so the original
    // prompt is unambiguously separated.
    let expected_block = render_handoff_envelope_prompt(&view);
    // The renderer always ends with "\n". The prepender appends
    // "---\n\n" so the original prompt body is unambiguously
    // separated from the rendered block.
    let expected = format!("{}---\n\n{}", expected_block, "BASE PROMPT");
    assert_eq!(out, expected);
}

#[test]
fn handoff_envelope_prompt_is_noop_when_prompt_injection_flag_off() {
    // `enabled` alone is not enough — `prompt_injection` is the
    // actual gate per the plan. Even with enabled=true the helper
    // must stay silent when prompt_injection=false (U10's
    // validate-only mode relies on this distinction).
    let env = envelope_fixture();
    let view = HandoffEnvelopeView::from(&env);
    let out = prepend_handoff_envelope_if_enabled(
        "BASE PROMPT".to_string(),
        &config(true, false),
        Some(&view),
    );
    assert_eq!(
        out, "BASE PROMPT",
        "prompt_injection=false must short-circuit even when enabled"
    );
}