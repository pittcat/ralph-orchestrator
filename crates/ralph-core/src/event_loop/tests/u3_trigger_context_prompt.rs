//! 2026-07-09-003 plan U3 wiring tests.
//!
//! These tests pin the *runtime* contract that complements
//! the U2 builder unit tests: `## TRIGGER CONTEXT` is rendered
//! into the isolated hat prompt **iff** the schema declares a
//! `trigger_context` block AND the hat subscribes to the
//! matched source topic AND the source event is present in
//! the runner's pending queue. We do **not** test prompt
//! contents that U2 already pins (the markdown shape, the
//! `<missing>` marker, dot-path reads, etc.) — that would
//! duplicate the unit suite and risk drift between the two
//! layers. The runtime tests focus on the wiring decisions:
//!
//! 1. No event policy → no block (SC6 / R3).
//! 2. Empty `trigger_context` declaration → no block (R3).
//! 3. Hat does not subscribe to the source topic → no block
//!    (R22 / R21).
//! 4. Hat subscribes; schema has a `summary_fields` block; a
//!    matching event is in the bus → block is prepended
//!    (SC1 / AE1).
//! 5. Missing declared summary field → `<missing>` is
//!    rendered (SC4 / AE3).
//! 6. Matched routing hint's `guidance` is rendered into the
//!    block (SC2 / SC3).
//! 7. No matching event in the bus → no block (default no-op
//!    for an empty queue).

use ralph_proto::Event;
use ralph_proto::HatId;

use super::*;

/// Build a config with two hats (`reviewer` and `synthesizer`)
/// plus a caller-supplied event policy. The hat topology lets
/// the leakage tests prove that the helper filters by the
/// **current** hat's trigger list, not by the event's
/// presence on the bus.
fn two_hat_config_with_policy(yaml_policy: &str) -> RalphConfig {
    let yaml = format!(
        r#"
prompt_file: PROMPT.md
hats:
  reviewer:
    name: "Reviewer"
    subscribes_to: ["review.request"]
    publishes: ["review.done"]
  synthesizer:
    name: "Synthesizer"
    subscribes_to: ["synthesize.request"]
    publishes: ["synthesize.done"]
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
  starting_event: "task.start"
{yaml_policy}
"#
    );
    serde_yaml::from_str(&yaml).expect("config parses")
}

#[test]
fn u3_prepend_trigger_context_no_event_policy_is_noop() {
    // SC6 / R3: no event policy means no schemas declared,
    // and the helper must not invent a block.
    let cfg = two_hat_config_with_policy("");
    let mut event_loop = EventLoop::new(cfg);
    event_loop.initialize("unit test");

    // Push a real event into the bus so a naïve implementation
    // that always injects would emit the heading. The hat's
    // `triggers` is non-empty, so the gate that fails first
    // is the "no event policy" one.
    event_loop
        .bus
        .publish(Event::new("review.request", r#"{"x":1}"#));

    let prompt = event_loop
        .build_prompt(&HatId::new("reviewer"))
        .expect("isolated build_prompt returns Some");
    // Anchor on the renderer's first body line (`- source topic:`)
    // instead of the `## TRIGGER CONTEXT` heading text. The heading
    // name also appears in agent-facing skill docs that mention the
    // block, so substring-checking the heading produces false
    // positives when such skills are injected into the prompt.
    assert!(
        !prompt.contains("- source topic:"),
        "without an event policy the prompt must not include the block, got: {prompt}"
    );
}

#[test]
fn u3_prepend_trigger_context_empty_declaration_is_noop() {
    // R3 / R29: schema exists for the topic, but its
    // `trigger_context` is the default-empty struct. The
    // helper must short-circuit and not inject.
    let cfg = two_hat_config_with_policy(
        r#"
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      review.request:
        required_fields:
          - x
"#,
    );
    let mut event_loop = EventLoop::new(cfg);
    event_loop.initialize("unit test");
    event_loop
        .bus
        .publish(Event::new("review.request", r#"{"x":1}"#));

    let prompt = event_loop
        .build_prompt(&HatId::new("reviewer"))
        .expect("isolated build_prompt returns Some");
    assert!(
        !prompt.contains("- source topic:"),
        "empty trigger_context declaration must yield no block, got: {prompt}"
    );
}

#[test]
fn u3_prepend_trigger_context_non_subscriber_hat_is_noop() {
    // R21 / R22 (runtime half): the schema has a
    // `trigger_context` declaration for `synthesize.request`,
    // but the `reviewer` hat does not subscribe to that
    // topic. The helper must not leak the synthesize context
    // into the reviewer's prompt just because the synthesize
    // event reached the bus.
    let cfg = two_hat_config_with_policy(
        r#"
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      synthesize.request:
        required_fields:
          - count
        field_docs:
          count:
            meaning: synthesize count
        trigger_context:
          summary_fields:
            - count
"#,
    );
    let mut event_loop = EventLoop::new(cfg);
    event_loop.initialize("unit test");
    // Publish the synthesize event. The reviewer should not
    // see a context block even though the event is in the bus.
    event_loop
        .bus
        .publish(Event::new("synthesize.request", r#"{"count": 4}"#));

    let prompt = event_loop
        .build_prompt(&HatId::new("reviewer"))
        .expect("isolated build_prompt returns Some");
    assert!(
        !prompt.contains("- source topic:"),
        "non-subscriber hat must not see the synthesize trigger context, got: {prompt}"
    );
}

#[test]
fn u3_prepend_trigger_context_subscriber_injects_block() {
    // SC1 / AE1: hat subscribes, schema declares summary
    // fields, the matching event is in the bus — the block
    // appears at the top of the prompt and contains the
    // declared field name and value.
    let cfg = two_hat_config_with_policy(
        r#"
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      review.request:
        required_fields:
          - verdict
          - count
        field_docs:
          verdict:
            meaning: review verdict
          count:
            meaning: finding count
        trigger_context:
          summary_fields:
            - verdict
            - count
"#,
    );
    let mut event_loop = EventLoop::new(cfg);
    event_loop.initialize("unit test");
    event_loop.bus.publish(Event::new(
        "review.request",
        r#"{"verdict": "pass", "count": 3}"#,
    ));

    let prompt = event_loop
        .build_prompt(&HatId::new("reviewer"))
        .expect("isolated build_prompt returns Some");
    // Anchor on the renderer's body line (`- source topic: <name>`)
    // instead of the heading text. The heading name also appears
    // in agent-facing skill docs that mention the block, so
    // substring-checking the heading produces false positives when
    // such skills are injected into the prompt.
    assert!(
        prompt.contains("- source topic: review.request"),
        "subscriber hat must see the trigger context block, got: {prompt}"
    );
    assert!(
        prompt.contains("verdict: \"pass\""),
        "block must surface verdict value, got: {prompt}"
    );
    assert!(
        prompt.contains("count: 3"),
        "block must surface count value, got: {prompt}"
    );
}

#[test]
fn u3_prepend_trigger_context_missing_field_renders_marker() {
    // SC4 / AE3: declared summary field absent from payload
    // → rendered as `<missing>`, never as a default.
    let cfg = two_hat_config_with_policy(
        r#"
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      review.request:
        required_fields:
          - present_field
        trigger_context:
          summary_fields:
            - present_field
            - absent_field
"#,
    );
    let mut event_loop = EventLoop::new(cfg);
    event_loop.initialize("unit test");
    // Payload has present_field, lacks absent_field.
    event_loop
        .bus
        .publish(Event::new("review.request", r#"{"present_field": 7}"#));

    let prompt = event_loop
        .build_prompt(&HatId::new("reviewer"))
        .expect("isolated build_prompt returns Some");
    assert!(
        prompt.contains("present_field: 7"),
        "present value must be rendered verbatim, got: {prompt}"
    );
    assert!(
        prompt.contains("absent_field: <missing>"),
        "absent declared field must be rendered as <missing>, got: {prompt}"
    );
    // Negative guard: must NOT be rendered as 0.
    assert!(
        !prompt.contains("absent_field: 0"),
        "absent field must not be coerced to 0, got: {prompt}"
    );
}

#[test]
fn u3_prepend_trigger_context_matched_hint_appears_in_block() {
    // SC2 / SC3: a routing hint whose conditions match the
    // payload must surface its `guidance` text in the
    // rendered block.
    let cfg = two_hat_config_with_policy(
        r#"
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      review.request:
        required_fields:
          - must_fix_now_count
        trigger_context:
          summary_fields:
            - must_fix_now_count
          routing_hints:
            - label: accept_residual
              guidance: "Residual findings are report-only; do not generate fix units."
              conditions:
                - field: must_fix_now_count
                  op: eq
                  value: 0
"#,
    );
    let mut event_loop = EventLoop::new(cfg);
    event_loop.initialize("unit test");
    event_loop
        .bus
        .publish(Event::new("review.request", r#"{"must_fix_now_count": 0}"#));

    let prompt = event_loop
        .build_prompt(&HatId::new("reviewer"))
        .expect("isolated build_prompt returns Some");
    assert!(
        prompt.contains("matched routing hints:"),
        "matched hint section must appear, got: {prompt}"
    );
    assert!(
        prompt.contains("[accept_residual] Residual findings are report-only"),
        "matched hint label and guidance must appear in the block, got: {prompt}"
    );
}

#[test]
fn u3_prepend_trigger_context_no_matching_event_is_noop() {
    // Default no-op for an empty queue. The hat triggers on
    // `review.request`, the schema declares a context block,
    // but the bus has no event for the hat to react to — the
    // helper must not invent a context from thin air.
    let cfg = two_hat_config_with_policy(
        r#"
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      review.request:
        required_fields:
          - x
        trigger_context:
          summary_fields:
            - x
"#,
    );
    let mut event_loop = EventLoop::new(cfg);
    event_loop.initialize("unit test");
    // No events published — the bus is empty.

    let prompt = event_loop
        .build_prompt(&HatId::new("reviewer"))
        .expect("isolated build_prompt returns Some");
    assert!(
        !prompt.contains("- source topic:"),
        "with no matching event the prompt must not include the block, got: {prompt}"
    );
}
