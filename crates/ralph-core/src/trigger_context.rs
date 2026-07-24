// SPDX-License-Identifier: Apache-2.0
//! Trigger context builder + markdown renderer.
//!
//! ## Plan reference
//!
//! 2026-07-09-003 (schema-backed trigger context) U2. U1 already
//! introduced the `EventSchema::trigger_context` data model
//! (`TriggerContextConfig` / `RoutingHintConfig` / `HintCondition`
//! / `HintOp`). This module adds the **pure** functions that
//! turn one accepted trigger event into a structured
//! [`TriggerContextView`] and then render it as a short markdown
//! block. The builder is I/O-free: it never reads
//! `.ralph/events.jsonl`, the event bus, the lint state, or any
//! preset YAML file. U3 wires it into the isolated prompt
//! prepend chain. U4/U5 add the strict lint that consumes the
//! same data model.
//!
//! ## Agent-facing promise
//!
//! The renderer output is the **only** thing the agent will see
//! under `## TRIGGER CONTEXT`. The contract is:
//!
//! - When `EventSchema::trigger_context` is empty (the
//!   pre-feature default), the builder returns
//!   `TriggerContextView::noop` and the renderer returns `None`,
//!   leaving the prompt block absent. SC6 / R3 / R29.
//! - When `summary_fields` is declared, every field in
//!   declaration order is rendered, with missing fields shown
//!   as `<missing>`. SC4 / AE3.
//! - When `routing_hints` is declared, only the hints whose
//!   conjunctive conditions all match are rendered, in
//!   declaration order. R11 / R12.
//! - The renderer never injects fields or hints that the
//!   schema did not declare, even if they exist in the
//!   payload. R22 / payload-leakage guard.
//! - Numeric comparisons only accept JSON numbers; type
//!   mismatches make the condition return false (no panic, no
//!   implicit coercion). R8 / R9.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use ralph_proto::Topic;

use crate::config::{EventSchema, HintCondition, HintOp, TriggerContextConfig};

/// Topics that the runner treats as internal bookkeeping. They
/// are never "the trigger" for an isolated hat activation and
/// must not be considered when selecting the most recent
/// trigger event for `## TRIGGER CONTEXT`. Mirrors
/// `EventLoop::is_system_event`; kept as a string list so the
/// `trigger_context` module stays I/O-free.
const SYSTEM_TOPICS: &[&str] = &[
    "task.resume",
    "human.guidance",
    "loop.cancel",
    "loop.suspend",
    "plan.complete",
    "plan.blocked",
    "LOOP_COMPLETE",
    "REVIEW_COMPLETE",
    "ralph.control",
];

/// Resolved trigger: the most recent accepted event that
/// `hat_triggers` declared interest in. `payload` is the parsed
/// JSON value (or `Value::Null` when the payload was not valid
/// JSON; the renderer treats that case as "no fields present").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedTrigger<'a> {
    pub topic: &'a str,
    pub payload: Value,
}

/// Walk `events` in reverse, return the most recent non-system
/// event whose topic is matched by at least one of
/// `hat_triggers` (glob-aware, see [`Topic::matches_str`]). The
/// search starts from the **end** so the latest matching event
/// wins — older events are stale by definition.
///
/// The hat-triggers filter is the same check the runner uses to
/// pick the active hat in the first place, so the trigger
/// context never leaks across hat subscriptions (R21 / R22).
/// When the hat declares no triggers, the helper returns
/// `None` (caller should treat this as a no-op).
pub fn find_matching_trigger_event<'a>(
    events: &'a [ralph_proto::Event],
    hat_triggers: &[String],
) -> Option<MatchedTrigger<'a>> {
    for ev in events.iter().rev() {
        let topic = ev.topic.as_str();
        if SYSTEM_TOPICS.contains(&topic) {
            continue;
        }
        let matched = hat_triggers
            .iter()
            .any(|t| Topic::from(t.as_str()).matches_str(topic));
        if !matched {
            continue;
        }
        let payload: Value = serde_json::from_str(&ev.payload).unwrap_or(Value::Null);
        return Some(MatchedTrigger { topic, payload });
    }
    None
}

/// Stable source-of-truth for one trigger context. Built by
/// [`build`] and consumed by [`render`]. Keeping the typed
/// structure separate from the markdown string means callers
/// can write their own snapshots (snapshot tests, BDD scenario
/// asserts, log printers) without re-parsing markdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerContextView {
    /// Source topic that drove the current hat activation. The
    /// renderer writes this verbatim into the markdown block
    /// header.
    pub source_topic: String,

    /// Source hat that published the trigger event, if the
    /// caller supplied it. `None` is rendered as
    /// `(unknown source hat)`.
    pub source_hat: Option<String>,

    /// Current hat being activated. Recorded for downstream
    /// observability (U5 topology-aware lint) but never rendered
    /// into the prompt block.
    pub current_hat: String,

    /// Per-field summary extracted from the trigger payload in
    /// `summary_fields` declaration order. Missing fields show
    /// up as [`FieldValue::Missing`].
    pub summary: Vec<FieldSummary>,

    /// Routing hints whose conjunctive conditions all matched
    /// the payload. Order is the schema's declaration order
    /// (R11), not evaluation order.
    pub matched_hints: Vec<MatchedHint>,
}

impl TriggerContextView {
    /// Convenience constructor for the no-op case (empty
    /// `trigger_context` declaration). `TriggerContextView::noop`
    /// is what `build` returns when the schema has nothing to
    /// declare; the renderer then produces no markdown.
    pub fn noop(current_hat: impl Into<String>) -> Self {
        Self {
            source_topic: String::new(),
            source_hat: None,
            current_hat: current_hat.into(),
            summary: Vec::new(),
            matched_hints: Vec::new(),
        }
    }

    /// True when the builder produced no summary fields and no
    /// matched hints and the source topic is empty. Mirrors
    /// the "schema has no declaration" branch.
    pub fn is_noop(&self) -> bool {
        self.source_topic.is_empty() && self.summary.is_empty() && self.matched_hints.is_empty()
    }
}

/// One row in the `field: <value>` summary list rendered into
/// the markdown block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldSummary {
    /// Field path as declared in `summary_fields`. Used as the
    /// key the agent sees (e.g. `must_fix_now_count`).
    pub field: String,

    /// Field value, or `Missing` when the payload has no value
    /// at the declared path. `Missing` is **not** the same as
    /// `Null`: a `null` payload value still serialises as
    /// `Value::Null`, whereas `Missing` means the field is
    /// absent. AE3 / SC4 require the renderer to surface the
    /// distinction as `<missing>`.
    pub value: FieldValue,
}

/// Rendered form of one summary field. The structured enum lets
/// tests assert on "what the agent will see" without parsing
/// markdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum FieldValue {
    /// Field was present in the payload and we kept the
    /// original JSON value (number, string, bool, object, array,
    /// or null).
    Present(Value),

    /// Field was absent at the declared path. Renderer outputs
    /// `<missing>`. We must **not** infer defaults (no `0`, no
    /// `false`, no empty string). AE3 / SC4 / R5.
    Missing,
}

/// One matched routing hint, ready for rendering. `label` and
/// `guidance` are both agent-facing and stable across releases
/// (R10 / R15). The `label` is the lint-finding key (U4) and
/// the `guidance` is the body of the markdown bullet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchedHint {
    pub label: String,
    pub guidance: String,
}

/// Inputs to the builder. The caller (U3 prompt helper) hands
/// the current hat identity, the schema, the trigger event, and
/// the source metadata. Builder code never reads from disk or
/// the bus.
#[derive(Debug, Clone)]
pub struct TriggerContextInput<'a> {
    /// Current hat being activated. Used for observability
    /// only; the builder does not branch on it.
    pub current_hat: &'a str,

    /// Source topic of the accepted trigger event. Mandatory;
    /// an empty string is treated as "no trigger event".
    pub source_topic: &'a str,

    /// Source hat that published the trigger event. `None` is
    /// legal (e.g. events synthesised by the runner).
    pub source_hat: Option<&'a str>,

    /// Schema that owns the `trigger_context` declaration. The
    /// builder reads the schema's `summary_fields` /
    /// `routing_hints`; nothing else.
    pub schema: &'a EventSchema,

    /// Trigger payload. The builder extracts summary fields
    /// and evaluates conditions against this value. A non-object
    /// payload is treated as "all summary fields missing, no
    /// conditions match".
    pub payload: &'a Value,
}

/// Build a structured [`TriggerContextView`] from the inputs.
///
/// Returns a no-op view when the schema has no `trigger_context`
/// declaration (no `summary_fields`, no `routing_hints`) or when
/// the caller did not pass a source topic. The no-op path is
/// the SC6 / R3 / R29 contract that pre-feature prompts are
/// byte-identical to post-feature prompts.
pub fn build(input: &TriggerContextInput<'_>) -> TriggerContextView {
    if input.source_topic.is_empty() {
        return TriggerContextView::noop(input.current_hat);
    }
    let cfg: &TriggerContextConfig = &input.schema.trigger_context;
    if cfg.summary_fields.is_empty() && cfg.routing_hints.is_empty() {
        return TriggerContextView::noop(input.current_hat);
    }

    let summary = extract_summary_fields(&cfg.summary_fields, input.payload);
    let matched_hints = evaluate_hints(&cfg.routing_hints, input.payload);

    TriggerContextView {
        source_topic: input.source_topic.to_string(),
        source_hat: input.source_hat.map(str::to_string),
        current_hat: input.current_hat.to_string(),
        summary,
        matched_hints,
    }
}

/// Extract one row per declared `summary_field`, in declaration
/// order. Missing fields are recorded as `FieldValue::Missing`.
/// The function intentionally does not fail or warn on
/// undeclared fields; that is a U4 lint responsibility.
fn extract_summary_fields(declared: &[String], payload: &Value) -> Vec<FieldSummary> {
    declared
        .iter()
        .map(|field| FieldSummary {
            field: field.clone(),
            value: read_field(payload, field)
                .map(|v| FieldValue::Present(v.clone()))
                .unwrap_or(FieldValue::Missing),
        })
        .collect()
}

/// Evaluate all routing hints in declaration order. A hint is
/// emitted only when **every** condition in `conditions` matches
/// the payload. Type mismatches silently fail (no panic, no
/// coercion) per R8 / R9.
fn evaluate_hints(
    declared: &[crate::config::RoutingHintConfig],
    payload: &Value,
) -> Vec<MatchedHint> {
    declared
        .iter()
        .filter(|hint| hint.conditions.iter().all(|c| eval_condition(c, payload)))
        .map(|hint| MatchedHint {
            label: hint.label.clone(),
            guidance: hint.guidance.clone(),
        })
        .collect()
}

/// Evaluate a single `HintCondition` against the payload. The
/// `HintOp::Unknown` branch always returns false so the U4 lint
/// owns the "unsupported predicate" finding. Numeric comparisons
/// (`gt` / `gte` / `lt` / `lte`) require both the payload value
/// and the declared value to be JSON numbers; otherwise the
/// condition is false.
fn eval_condition(cond: &HintCondition, payload: &Value) -> bool {
    let present = read_field(payload, &cond.field);
    match cond.op {
        HintOp::Exists => present.is_some(),
        HintOp::Missing => present.is_none(),
        HintOp::Eq => present.is_some() && present == Some(&cond.value),
        HintOp::Ne => match present {
            // R8: missing != literal ⇒ false (not "true because
            // there is no value to compare"). Authors who want
            // "always except when present" should use
            // `missing` or an explicit `exists` precondition.
            Some(v) => v != &cond.value,
            None => false,
        },
        HintOp::Gt | HintOp::Gte | HintOp::Lt | HintOp::Lte => {
            match (present, cond.value.as_f64()) {
                (Some(v), Some(threshold)) => match v.as_f64() {
                    Some(actual) => match cond.op {
                        HintOp::Gt => actual > threshold,
                        HintOp::Gte => actual >= threshold,
                        HintOp::Lt => actual < threshold,
                        HintOp::Lte => actual <= threshold,
                        // unreachable: outer match already gated
                        // on gt/gte/lt/lte
                        _ => false,
                    },
                    // Payload value is non-numeric (e.g. string,
                    // bool, null). R8 / R9: type-mismatch ⇒ no
                    // match, no panic, no implicit coercion.
                    None => false,
                },
                _ => false,
            }
        }
        HintOp::Unknown(_) => false,
    }
}

/// Read a top-level or dot-notation field from the payload.
/// Returns `None` when the path is empty, the payload is not an
/// object, or any intermediate node is not an object (e.g.
/// indexing into an array or a string). Top-level field names
/// that contain literal `.` characters are not supported in
/// v1; that limitation is documented in the data model.
fn read_field<'a>(payload: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() {
        return None;
    }
    let mut current = payload;
    for segment in path.split('.') {
        match current {
            Value::Object(map) => {
                current = map.get(segment)?;
            }
            // Intermediate node is not an object: indexing is
            // not defined (array / scalar). v1 limitation.
            _ => return None,
        }
    }
    Some(current)
}

/// Render a [`TriggerContextView`] as a short markdown block.
///
/// Returns `None` when the view is a no-op. The `None` contract
/// lets the U3 prompt helper ask "should I inject this block?"
/// without having to inspect the view's internals.
///
/// The block shape is fixed and tested as a golden string in
/// `tests::render_golden_string`. Schema/preset authors may
/// not customise the header, the section ordering, or the
/// missing-field marker. These are agent-facing surface
/// contracts; the field labels and hint text come from the
/// schema declaration.
pub fn render(view: &TriggerContextView) -> Option<String> {
    if view.is_noop() {
        return None;
    }
    let mut out = String::new();
    out.push_str("## TRIGGER CONTEXT\n");
    out.push_str("- source topic: ");
    out.push_str(&view.source_topic);
    out.push('\n');
    out.push_str("- source hat: ");
    match &view.source_hat {
        Some(hat) => out.push_str(hat),
        None => out.push_str("(unknown source hat)"),
    }
    out.push('\n');
    if !view.summary.is_empty() {
        out.push_str("- summary fields:\n");
        for row in &view.summary {
            out.push_str("  - ");
            out.push_str(&row.field);
            out.push_str(": ");
            match &row.value {
                FieldValue::Present(v) => out.push_str(&render_value(v)),
                FieldValue::Missing => out.push_str("<missing>"),
            }
            out.push('\n');
        }
    }
    if !view.matched_hints.is_empty() {
        out.push_str("- matched routing hints:\n");
        for hint in &view.matched_hints {
            out.push_str("  - [");
            out.push_str(&hint.label);
            out.push_str("] ");
            out.push_str(&hint.guidance);
            out.push('\n');
        }
    }
    Some(out)
}

/// Serialise a JSON value into a short agent-facing string.
/// The representation is **not** a round-trip: large objects or
/// arrays are abbreviated so the markdown block stays scannable
/// (R14). Scalar values (number, bool, null) render verbatim;
/// strings render with surrounding quotes.
fn render_value(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => format!("\"{s}\""),
        Value::Array(items) => format!("[{} items]", items.len()),
        Value::Object(map) => format!("{{{} keys}}", map.len()),
    }
}

#[cfg(test)]
mod u4_trigger_context_builder_tests {
    //! U2 acceptance tests. These tests pin the builder
    //! behaviour against the U1 data model; they do not touch
    //! any preset YAML, prompt code, or event loop. Coverage:
    //!
    //! 1. summary happy path: JSON values are preserved in
    //!    declaration order (U1 test
    //!    `u1_trigger_context_preserves_declaration_order`).
    //! 2. missing field: declared `summary_field` absent from
    //!    the payload renders as `FieldValue::Missing`. AE3 /
    //!    SC4.
    //! 3. no-op: schema with empty `trigger_context` returns
    //!    `TriggerContextView::noop` and the renderer returns
    //!    `None`. SC6 / R3 / R29.
    //! 4. non-object payload: all summary fields render as
    //!    `Missing`, no hint matches.
    //! 5. single hint match: `eq 0` matches when the payload
    //!    value equals the declared literal.
    //! 6. multi-hint ordering: hints are evaluated in schema
    //!    declaration order; only matching ones are kept in
    //!    declaration order.
    //! 7. dot-path: `nested.count` reads through objects;
    //!    intermediate non-object nodes do not panic.
    //! 8. numeric comparison type guard: a non-numeric payload
    //!    value never matches `gt 0` (no coercion).
    //! 9. `exists` / `missing`: present field matches `exists`,
    //!    absent field matches `missing`.
    //! 10. unknown op never matches: `HintOp::Unknown` returns
    //!     false even when the payload has a valid value.
    //! 11. render golden: the markdown block has a fixed,
    //!     agent-facing shape. U3 prompt helper and BDD
    //!     scenario assert against this golden string.

    use super::*;
    use crate::config::{HintCondition, RoutingHintConfig};
    use serde_json::json;

    fn schema_with_summary_fields(fields: &[&str]) -> EventSchema {
        let mut s = EventSchema::default();
        for f in fields {
            s.trigger_context.summary_fields.push((*f).to_string());
        }
        s
    }

    fn schema_with_summary_and_hints(
        fields: &[&str],
        hints: Vec<RoutingHintConfig>,
    ) -> EventSchema {
        let mut s = schema_with_summary_fields(fields);
        s.trigger_context.routing_hints = hints;
        s
    }

    fn input<'a>(
        current: &'a str,
        topic: &'a str,
        schema: &'a EventSchema,
        payload: &'a Value,
    ) -> TriggerContextInput<'a> {
        TriggerContextInput {
            current_hat: current,
            source_topic: topic,
            source_hat: Some("review-synthesizer"),
            schema,
            payload,
        }
    }

    /// 1. Summary happy path: JSON values of every primitive
    /// type are preserved in declaration order.
    #[test]
    fn u2_summary_happy_path_preserves_values_and_order() {
        let schema = schema_with_summary_fields(&[
            "review_round",
            "must_fix_now_count",
            "verdict",
            "synthesized_review_file",
        ]);
        let payload = json!({
            "review_round": 3,
            "must_fix_now_count": 2,
            "verdict": "fix_required",
            "synthesized_review_file": "/tmp/review.md",
        });
        let view = build(&input(
            "review-gate",
            "review.synthesized",
            &schema,
            &payload,
        ));
        let fields: Vec<&str> = view.summary.iter().map(|f| f.field.as_str()).collect();
        assert_eq!(
            fields,
            vec![
                "review_round",
                "must_fix_now_count",
                "verdict",
                "synthesized_review_file",
            ]
        );
        assert_eq!(view.summary[0].value, FieldValue::Present(json!(3)));
        assert_eq!(view.summary[1].value, FieldValue::Present(json!(2)));
        assert_eq!(
            view.summary[2].value,
            FieldValue::Present(json!("fix_required"))
        );
        assert_eq!(
            view.summary[3].value,
            FieldValue::Present(json!("/tmp/review.md"))
        );
    }

    /// 2. Missing field: declared `summary_field` absent from
    /// the payload is recorded as `Missing`, not as a default
    /// value.
    #[test]
    fn u2_missing_field_renders_as_missing_never_default() {
        let schema = schema_with_summary_fields(&["must_fix_now_count", "residual_findings_count"]);
        let payload = json!({"must_fix_now_count": 0});
        let view = build(&input(
            "review-gate",
            "review.synthesized",
            &schema,
            &payload,
        ));
        assert_eq!(view.summary[0].value, FieldValue::Present(json!(0)));
        assert_eq!(
            view.summary[1].value,
            FieldValue::Missing,
            "absent summary field must be FieldValue::Missing, never a default"
        );

        // Renderer must surface <missing> for FieldValue::Missing.
        let rendered = render(&view).expect("non-noop view must render");
        assert!(
            rendered.contains("residual_findings_count: <missing>"),
            "renderer must surface absent summary field as <missing>, got: {rendered}"
        );
        // Negative guard: must NOT be rendered as 0.
        assert!(
            !rendered.contains("residual_findings_count: 0"),
            "renderer must not coerce missing field to 0"
        );
    }

    /// 3. No-op: empty `trigger_context` declaration produces
    /// `TriggerContextView::noop`; the renderer returns `None`.
    /// SC6 / R3 / R29.
    #[test]
    fn u2_empty_trigger_context_is_noop() {
        let schema = EventSchema::default();
        let payload = json!({"anything": 1});
        let view = build(&input("any-hat", "review.synthesized", &schema, &payload));
        assert!(view.is_noop());
        assert!(render(&view).is_none());
    }

    /// 4. Non-object payload: builder treats non-object payload
    /// as "no fields present, no conditions match". Required
    /// because events with non-object payloads (e.g. string
    /// payloads for `cancellations`) must not panic.
    #[test]
    fn u2_non_object_payload_yields_missing_and_no_match() {
        let mut schema = schema_with_summary_fields(&["must_fix_now_count"]);
        schema
            .trigger_context
            .routing_hints
            .push(RoutingHintConfig {
                label: "anything".into(),
                guidance: "should never match".into(),
                conditions: vec![HintCondition {
                    field: "must_fix_now_count".into(),
                    op: HintOp::Exists,
                    value: Value::Null,
                }],
                exclusive_group: String::new(),
            });
        let payload = json!("just a string");
        let view = build(&input(
            "review-gate",
            "review.synthesized",
            &schema,
            &payload,
        ));
        assert_eq!(view.summary.len(), 1);
        assert_eq!(view.summary[0].value, FieldValue::Missing);
        assert!(view.matched_hints.is_empty());
    }

    /// 5. Single hint match: `eq 0` matches when the payload
    /// value equals 0; `gt 0` does not.
    #[test]
    fn u2_single_hint_eq_zero_matches_and_gt_zero_does_not() {
        let schema = schema_with_summary_and_hints(
            &[],
            vec![
                RoutingHintConfig {
                    label: "accept".into(),
                    guidance: "accept residual".into(),
                    conditions: vec![HintCondition {
                        field: "must_fix_now_count".into(),
                        op: HintOp::Eq,
                        value: json!(0),
                    }],
                    exclusive_group: String::new(),
                },
                RoutingHintConfig {
                    label: "must_fix".into(),
                    guidance: "fix required".into(),
                    conditions: vec![HintCondition {
                        field: "must_fix_now_count".into(),
                        op: HintOp::Gt,
                        value: json!(0),
                    }],
                    exclusive_group: String::new(),
                },
            ],
        );
        let payload = json!({"must_fix_now_count": 0});
        let view = build(&input(
            "review-gate",
            "review.synthesized",
            &schema,
            &payload,
        ));
        assert_eq!(view.matched_hints.len(), 1);
        assert_eq!(view.matched_hints[0].label, "accept");

        let payload2 = json!({"must_fix_now_count": 2});
        let view2 = build(&input(
            "review-gate",
            "review.synthesized",
            &schema,
            &payload2,
        ));
        assert_eq!(view2.matched_hints.len(), 1);
        assert_eq!(view2.matched_hints[0].label, "must_fix");
    }

    /// 6. Multi-hint ordering: hints are returned in schema
    /// declaration order, not evaluation order. Both hints in
    /// this fixture match the same payload.
    #[test]
    fn u2_multi_hint_match_preserves_declaration_order() {
        let schema = schema_with_summary_and_hints(
            &[],
            vec![
                RoutingHintConfig {
                    label: "first".into(),
                    guidance: "shown first".into(),
                    conditions: vec![HintCondition {
                        field: "ready".into(),
                        op: HintOp::Exists,
                        value: Value::Null,
                    }],
                    exclusive_group: String::new(),
                },
                RoutingHintConfig {
                    label: "second".into(),
                    guidance: "shown second".into(),
                    conditions: vec![HintCondition {
                        field: "ready".into(),
                        op: HintOp::Exists,
                        value: Value::Null,
                    }],
                    exclusive_group: String::new(),
                },
            ],
        );
        let payload = json!({"ready": true});
        let view = build(&input("any", "any.topic", &schema, &payload));
        assert_eq!(view.matched_hints.len(), 2);
        assert_eq!(view.matched_hints[0].label, "first");
        assert_eq!(view.matched_hints[1].label, "second");
    }

    /// 7. Dot-path: `nested.count` reads through an object;
    /// intermediate non-object nodes do not panic and produce
    /// `Missing` instead.
    #[test]
    fn u2_dot_path_reads_through_object_intermediate_misses_are_safe() {
        let schema = schema_with_summary_fields(&["nested.count", "broken.index"]);
        let payload = json!({
            "nested": {"count": 5},
            "broken": "not an object"
        });
        let view = build(&input("any", "any.topic", &schema, &payload));
        assert_eq!(view.summary[0].value, FieldValue::Present(json!(5)));
        assert_eq!(view.summary[1].value, FieldValue::Missing);
    }

    /// 8. Numeric comparison type guard: a string payload must
    /// not match `gt 0`, no implicit coercion.
    #[test]
    fn u2_numeric_compare_rejects_non_numeric_payload() {
        let schema = schema_with_summary_and_hints(
            &[],
            vec![RoutingHintConfig {
                label: "gt_zero".into(),
                guidance: "should never match".into(),
                conditions: vec![HintCondition {
                    field: "value".into(),
                    op: HintOp::Gt,
                    value: json!(0),
                }],
                exclusive_group: String::new(),
            }],
        );
        let payload = json!({"value": "two"});
        let view = build(&input("any", "any.topic", &schema, &payload));
        assert!(view.matched_hints.is_empty());
    }

    /// 9. `exists` / `missing` semantics.
    #[test]
    fn u2_exists_and_missing_predicates() {
        let schema = schema_with_summary_and_hints(
            &[],
            vec![
                RoutingHintConfig {
                    label: "when_present".into(),
                    guidance: "shown when field present".into(),
                    conditions: vec![HintCondition {
                        field: "round".into(),
                        op: HintOp::Exists,
                        value: Value::Null,
                    }],
                    exclusive_group: String::new(),
                },
                RoutingHintConfig {
                    label: "when_absent".into(),
                    guidance: "shown when field missing".into(),
                    conditions: vec![HintCondition {
                        field: "round".into(),
                        op: HintOp::Missing,
                        value: Value::Null,
                    }],
                    exclusive_group: String::new(),
                },
            ],
        );
        let present = json!({"round": 1});
        let view_present = build(&input("any", "any.topic", &schema, &present));
        assert_eq!(view_present.matched_hints.len(), 1);
        assert_eq!(view_present.matched_hints[0].label, "when_present");

        let absent = json!({});
        let view_absent = build(&input("any", "any.topic", &schema, &absent));
        assert_eq!(view_absent.matched_hints.len(), 1);
        assert_eq!(view_absent.matched_hints[0].label, "when_absent");
    }

    /// 10. `HintOp::Unknown` never matches; U4 lint owns the
    /// "unsupported predicate" finding.
    #[test]
    fn u2_unknown_op_never_matches_even_when_field_present() {
        let schema = schema_with_summary_and_hints(
            &[],
            vec![RoutingHintConfig {
                label: "broken".into(),
                guidance: "must not render".into(),
                conditions: vec![HintCondition {
                    field: "field".into(),
                    op: HintOp::Unknown("contains".into()),
                    value: json!("hot"),
                }],
                exclusive_group: String::new(),
            }],
        );
        let payload = json!({"field": "hot"});
        let view = build(&input("any", "any.topic", &schema, &payload));
        assert!(view.matched_hints.is_empty());
    }

    /// 11. Render golden: the markdown block has a fixed
    /// agent-facing shape; this test pins the exact output so
    /// U3 prompt helper and BDD scenarios can match against a
    /// known string without re-deriving the format.
    #[test]
    fn u2_render_golden_string() {
        let schema = schema_with_summary_and_hints(
            &[
                "review_round",
                "must_fix_now_count",
                "residual_findings_count",
            ],
            vec![RoutingHintConfig {
                label: "accept_residual".into(),
                guidance: "Residual findings are report-only; do not generate fix units.".into(),
                conditions: vec![HintCondition {
                    field: "must_fix_now_count".into(),
                    op: HintOp::Eq,
                    value: json!(0),
                }],
                exclusive_group: String::new(),
            }],
        );
        let payload = json!({
            "review_round": 3,
            "must_fix_now_count": 0
        });
        let view = build(&input(
            "review-gate",
            "review.synthesized",
            &schema,
            &payload,
        ));
        let rendered = render(&view).expect("non-noop view must render");
        let expected = "## TRIGGER CONTEXT\n\
                        - source topic: review.synthesized\n\
                        - source hat: review-synthesizer\n\
                        - summary fields:\n  \
                          - review_round: 3\n  \
                          - must_fix_now_count: 0\n  \
                          - residual_findings_count: <missing>\n\
                        - matched routing hints:\n  \
                          - [accept_residual] Residual findings are report-only; do not generate fix units.\n";
        assert_eq!(rendered, expected);
    }

    /// 12. Empty source topic yields a no-op even when the
    /// schema declares a `trigger_context`. The runner calls
    /// `build` only for matched triggers, but the safety net
    /// keeps `build` total and panic-free.
    #[test]
    fn u2_empty_source_topic_yields_noop() {
        let schema = schema_with_summary_fields(&["must_fix_now_count"]);
        let payload = json!({"must_fix_now_count": 0});
        let view = build(&input("any", "", &schema, &payload));
        assert!(view.is_noop());
    }

    /// 13. No-op render: `render(&noop)` returns `None` even
    /// when the caller constructed the view directly.
    #[test]
    fn u2_render_noop_is_none() {
        let view = TriggerContextView::noop("any");
        assert!(render(&view).is_none());
    }

    /// 14. Source hat unknown: caller may pass `None` for
    /// events that did not originate from a hat (e.g. runner
    /// synthesised events). The renderer must surface
    /// `(unknown source hat)`, never panic.
    #[test]
    fn u2_source_hat_none_renders_unknown_marker() {
        let schema = schema_with_summary_fields(&["count"]);
        let payload = json!({"count": 1});
        let view = build(&TriggerContextInput {
            current_hat: "any",
            source_topic: "any.topic",
            source_hat: None,
            schema: &schema,
            payload: &payload,
        });
        let rendered = render(&view).expect("non-noop view must render");
        assert!(rendered.contains("- source hat: (unknown source hat)"));
    }

    /// 15. Render value abbreviation: large objects / arrays
    /// are surfaced as `{N keys}` / `[N items]` so the markdown
    /// block stays scannable (R14). Scalars render verbatim.
    #[test]
    fn u2_render_value_abbreviates_compound_types() {
        let schema = schema_with_summary_fields(&[
            "obj",
            "arr",
            "scalar_num",
            "scalar_str",
            "scalar_null",
            "scalar_bool",
        ]);
        let payload = json!({
            "obj": {"a": 1, "b": 2, "c": 3},
            "arr": [1, 2, 3, 4],
            "scalar_num": 7,
            "scalar_str": "ok",
            "scalar_null": null,
            "scalar_bool": true,
        });
        let view = build(&input("any", "any.topic", &schema, &payload));
        let rendered = render(&view).expect("non-noop view must render");
        assert!(rendered.contains("  - obj: {3 keys}\n"));
        assert!(rendered.contains("  - arr: [4 items]\n"));
        assert!(rendered.contains("  - scalar_num: 7\n"));
        assert!(rendered.contains("  - scalar_str: \"ok\"\n"));
        assert!(rendered.contains("  - scalar_null: null\n"));
        assert!(rendered.contains("  - scalar_bool: true\n"));
    }
}

#[cfg(test)]
mod u5_trigger_event_matcher_tests {
    //! U3 / U5 acceptance tests for `find_matching_trigger_event`.
    //!
    //! The matcher is the runtime-side analogue of the U5
    //! topology-leakage lint: a `## TRIGGER CONTEXT` block must
    //! never be injected for a hat that did not subscribe to
    //! the source topic. We pin the helper's behaviour with
    //! focused tests so the EventLoop wiring in
    //! `prepend_trigger_context` can stay a thin pass-through.
    use super::*;
    use ralph_proto::Event;
    use serde_json::json;

    fn evt(topic: &str, payload: &str) -> Event {
        Event::new(topic, payload)
    }

    /// 1. No events at all → no trigger (caller should no-op).
    #[test]
    fn u3_no_events_returns_none() {
        let events: Vec<Event> = vec![];
        let triggers = vec!["review.synthesized".to_string()];
        assert!(find_matching_trigger_event(&events, &triggers).is_none());
    }

    /// 2. Event matches the hat's declared trigger: helper
    /// returns the most recent matching event and parses its
    /// payload into JSON.
    #[test]
    fn u3_returns_most_recent_matching_event() {
        let events = vec![
            evt("work.done", r#"{"x":1}"#),
            evt("review.synthesized", r#"{"must_fix_now_count":0}"#),
        ];
        let triggers = vec!["review.synthesized".to_string()];
        let matched = find_matching_trigger_event(&events, &triggers).expect("must match");
        assert_eq!(matched.topic, "review.synthesized");
        assert_eq!(matched.payload, json!({"must_fix_now_count": 0}));
    }

    /// 3. Glob trigger matches: `review.*` matches
    /// `review.synthesized`. We must not silently ignore glob
    /// semantics — that would diverge from the runtime's own
    /// subscription matcher.
    #[test]
    fn u3_glob_trigger_matches_suffix() {
        let events = vec![evt("review.synthesized", r#"{"x":1}"#)];
        let triggers = vec!["review.*".to_string()];
        let matched = find_matching_trigger_event(&events, &triggers).expect("must match");
        assert_eq!(matched.topic, "review.synthesized");
    }

    /// 4. System events are never the trigger. Even if a
    /// malformed preset declared `task.resume` as a hat
    /// trigger, the helper ignores system topics so the block
    /// never injects.
    #[test]
    fn u3_system_events_never_match() {
        let events = vec![
            evt("task.resume", r#"{"x":1}"#),
            evt("human.guidance", r#"{"x":2}"#),
        ];
        let triggers = vec!["task.resume".to_string(), "human.guidance".to_string()];
        assert!(find_matching_trigger_event(&events, &triggers).is_none());
    }

    /// 5. Event present but not in the hat's trigger list:
    /// the helper must return `None`. This is the runtime
    /// half of the R22 leakage guard — a hat that does not
    /// subscribe to `plan.complete` must not see the trigger
    /// context for `plan.complete`, even if the event reached
    /// its event bus.
    #[test]
    fn u3_non_subscriber_event_returns_none() {
        let events = vec![evt("plan.complete", r#"{"x":1}"#)];
        let triggers = vec!["work.done".to_string()];
        assert!(find_matching_trigger_event(&events, &triggers).is_none());
    }

    /// 6. Empty hat trigger list is a hard no-op (defensive:
    /// a hat with no triggers should never receive a context
    /// block).
    #[test]
    fn u3_empty_trigger_list_returns_none() {
        let events = vec![evt("any.topic", r#"{"x":1}"#)];
        let triggers: Vec<String> = vec![];
        assert!(find_matching_trigger_event(&events, &triggers).is_none());
    }

    /// 7. Non-JSON payload is surfaced as `Value::Null` rather
    /// than panicking. The builder treats the null payload as
    /// "all fields missing", which renders as `<missing>`.
    #[test]
    fn u3_non_json_payload_becomes_null() {
        let events = vec![evt("review.synthesized", "not-json")];
        let triggers = vec!["review.synthesized".to_string()];
        let matched = find_matching_trigger_event(&events, &triggers).expect("must match");
        assert_eq!(matched.payload, Value::Null);
    }

    /// 8. Most-recent wins: when several matching events are
    /// present, the helper returns the **last** one, mirroring
    /// the "scan backwards" rule used by the runner's own
    /// trigger search.
    #[test]
    fn u3_returns_last_matching_event() {
        let events = vec![
            evt("review.synthesized", r#"{"round":1}"#),
            evt("other.topic", r#"{"ignored":true}"#),
            evt("review.synthesized", r#"{"round":2}"#),
        ];
        let triggers = vec!["review.synthesized".to_string()];
        let matched = find_matching_trigger_event(&events, &triggers).expect("must match");
        assert_eq!(matched.topic, "review.synthesized");
        assert_eq!(matched.payload, json!({"round": 2}));
    }
}
