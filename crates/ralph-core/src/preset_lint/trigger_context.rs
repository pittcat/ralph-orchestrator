//! 2026-07-09-003 plan (U4): schema-backed trigger context
//! static lint.
//!
//! U1 introduced the `EventSchema::trigger_context` data model
//! (`TriggerContextConfig` / `RoutingHintConfig` /
//! `HintCondition` / `HintOp`). U2 turned that data model into
//! a pure builder. U3 wired the builder into the isolated
//! prompt chain. This module is the **lint** half of the
//! contract: it walks `event_policy.schemas.<topic>.
//! trigger_context` and emits stable findings for the
//! following shape errors.
//!
//! Rules covered here (R2 / R8 / R9 / R11 / R19 / R20 / SC5):
//!
//! - [`FINDING_TRIGGER_CONTEXT_UNKNOWN_FIELD`]
//!   `summary_fields` and condition `field` paths must
//!   resolve into one of `required_fields ∪ known_fields ∪
//!   field_docs.keys() ∪ allowed_values.keys()`. The runtime
//!   cannot extract a field that is not declared anywhere on
//!   the topic schema.
//!
//! - [`FINDING_TRIGGER_CONTEXT_UNSUPPORTED_PREDICATE`]
//!   `op` must be one of the v1 allowlist
//!   (`eq` / `ne` / `gt` / `gte` / `lt` / `lte` / `exists` /
//!   `missing`). Anything else is preserved at parse time as
//!   `HintOp::Unknown(String)`; this lint surfaces the
//!   original string.
//!
//! - [`FINDING_TRIGGER_CONTEXT_VALUE_SHAPE`]
//!   Comparison ops require a JSON value; numeric
//!   comparisons require a JSON number. `exists` / `missing`
//!   must not carry a `value`.
//!
//! - [`FINDING_TRIGGER_CONTEXT_DUPLICATE_LABEL`]
//!   Two routing hints sharing the same `label` would
//!   scramble the matched-hint sequence the agent sees in
//!   the prompt. Labels are the agent / lint / BDD stable
//!   identifier.
//!
//! Topology-aware leakage (R21 / R22) lives in the sibling
//! `trigger_context_topology` module; this module is
//! I/O-free and shape-only.

use std::collections::HashSet;

use ralph_proto::Topic;

use crate::config::{
    EventSchema, HintCondition, HintOp, RalphConfig, RoutingHintConfig, TriggerContextConfig,
};
use crate::preset_lint::finding_id::{
    FINDING_TRIGGER_CONTEXT_DUPLICATE_LABEL, FINDING_TRIGGER_CONTEXT_NO_CONSUMER,
    FINDING_TRIGGER_CONTEXT_UNKNOWN_FIELD, FINDING_TRIGGER_CONTEXT_UNSUPPORTED_PREDICATE,
    FINDING_TRIGGER_CONTEXT_VALUE_SHAPE,
};
use crate::preset_lint::{LintFinding, LintStrictness};

/// Plan 2026-07-09-003 (U4): check every topic schema's
/// `trigger_context` block for shape, predicate, and value
/// errors.
///
/// The lint is opt-in via `strictness`:
/// - `Default` mode: skip the lint entirely. Presets that do
///   not declare a `trigger_context` already get a free
///   pass, and presets that do declare one may want to ship
///   early without forcing the strict gate. This matches the
///   R3 / R29 "未声明 Trigger Context 的 preset 行为不变"
///   contract.
pub fn check_trigger_context(
    schemas: &std::collections::HashMap<String, EventSchema>,
    strictness: LintStrictness,
) -> Vec<LintFinding> {
    if !matches!(strictness, LintStrictness::Strict) {
        return Vec::new();
    }
    let mut findings = Vec::new();
    for (topic, schema) in schemas {
        let cfg = &schema.trigger_context;
        if cfg.summary_fields.is_empty() && cfg.routing_hints.is_empty() {
            continue;
        }
        let known = collect_known_fields(schema);
        check_summary_fields(topic, cfg, &known, &mut findings);
        check_routing_hints(topic, cfg, &known, &mut findings);
    }
    findings
}

/// Plan 2026-07-09-003 (U5): topology-aware check that the
/// `trigger_context` declared on a topic schema can actually
/// reach a downstream hat. The lint walks every hat's
/// `triggers` list (glob-aware, mirroring
/// `Topic::matches_str`); when a schema declares a
/// `trigger_context` block but no hat subscribes to that
/// topic, the block is dead and we emit
/// `FINDING_TRIGGER_CONTEXT_NO_CONSUMER`.
///
/// The check is **strict-only** by design (same default-mode
/// invariant as `check_trigger_context`). R21 / R22 / SC5.
///
/// Topology safety is **also** enforced at runtime by
/// `EventLoop::prepend_trigger_context`, which filters by
/// the current hat's trigger list. The lint + the runtime
/// filter form a defence-in-depth pair: the lint catches
/// dead declarations before the loop starts, and the
/// runtime filter protects against a hat that subscribes
/// to a different topic receiving the block by accident.
pub fn check_trigger_context_topology(
    config: &RalphConfig,
    strictness: LintStrictness,
) -> Vec<LintFinding> {
    if !matches!(strictness, LintStrictness::Strict) {
        return Vec::new();
    }
    let mut findings = Vec::new();
    let Some(policy) = config.event_loop.event_policy.as_ref() else {
        return findings;
    };
    // Pre-compute every hat's resolved trigger topics once.
    let hat_triggers: Vec<(&str, Vec<Topic>)> = config
        .hats
        .iter()
        .map(|(id, hat)| (id.as_str(), hat.trigger_topics()))
        .collect();
    for (topic, schema) in &policy.schemas {
        let cfg = &schema.trigger_context;
        if cfg.summary_fields.is_empty() && cfg.routing_hints.is_empty() {
            continue;
        }
        if !any_hat_subscribes(&hat_triggers, topic) {
            findings.push(no_consumer_finding(topic));
        }
    }
    findings
}

fn any_hat_subscribes(hat_triggers: &[(&str, Vec<Topic>)], topic: &str) -> bool {
    hat_triggers
        .iter()
        .any(|(_, triggers)| triggers.iter().any(|t| t.matches_str(topic)))
}

fn no_consumer_finding(topic: &str) -> LintFinding {
    LintFinding {
        id: FINDING_TRIGGER_CONTEXT_NO_CONSUMER,
        severity: crate::preset_lint::LintSeverity::Error,
        message: format!(
            "schema topic \"{topic}\" declares a trigger_context block but no hat \
             subscribes to \"{topic}\" in its `triggers` list; the block is dead. Add \
             the topic to a hat's `triggers:` or remove the trigger_context declaration"
        ),
        topic: Some(topic.to_string()),
        hat: None,
        owner: None,
        action_hint: Some(format!(
            "add `{topic}` to a hat's `triggers:` list, or remove the trigger_context \
             block from event_policy.schemas.{topic}"
        )),
    }
}

/// Walk every `required_fields` / `known_fields` /
/// `field_docs` / `allowed_values` key the schema exposes.
/// The set is the closure of fields the runtime is allowed
/// to extract or evaluate conditions against.
fn collect_known_fields(schema: &EventSchema) -> HashSet<String> {
    let mut out: HashSet<String> = HashSet::new();
    out.extend(schema.required_fields.iter().cloned());
    out.extend(schema.known_fields.iter().cloned());
    out.extend(schema.field_docs.keys().cloned());
    out.extend(schema.allowed_values.keys().cloned());
    out
}

fn check_summary_fields(
    topic: &str,
    cfg: &TriggerContextConfig,
    known: &HashSet<String>,
    out: &mut Vec<LintFinding>,
) {
    for field in &cfg.summary_fields {
        if !known.contains(field) {
            out.push(unknown_field_finding(topic, field, "summary_fields"));
        }
    }
}

fn check_routing_hints(
    topic: &str,
    cfg: &TriggerContextConfig,
    known: &HashSet<String>,
    out: &mut Vec<LintFinding>,
) {
    let mut seen_labels: HashSet<String> = HashSet::new();
    for hint in &cfg.routing_hints {
        if !hint.label.is_empty() {
            if !seen_labels.insert(hint.label.clone()) {
                out.push(duplicate_label_finding(topic, &hint.label));
            }
        }
        for cond in &hint.conditions {
            check_condition(topic, hint, cond, known, out);
        }
    }
}

fn check_condition(
    topic: &str,
    hint: &RoutingHintConfig,
    cond: &HintCondition,
    known: &HashSet<String>,
    out: &mut Vec<LintFinding>,
) {
    // R2: field must resolve to a known schema field.
    if !cond.field.is_empty() && !known.contains(&cond.field) {
        out.push(unknown_field_finding_in_hint(
            topic,
            &hint.label,
            &cond.field,
        ));
    }
    match &cond.op {
        HintOp::Unknown(raw) => {
            out.push(unsupported_predicate_finding(topic, &hint.label, raw));
            // R8 / R9: when the op is unknown we do not have
            // a value-shape contract to enforce. Stop here
            // so the operator sees only the predicate
            // finding (more actionable) instead of a stack
            // of cascading "value shape" findings on top of
            // it.
        }
        HintOp::Exists | HintOp::Missing => {
            // R8 / R9: `exists` / `missing` must NOT carry
            // a `value`. Authors who want to also assert a
            // shape should add a second condition.
            if !cond.value.is_null() {
                out.push(value_shape_finding(
                    topic,
                    &hint.label,
                    format!(
                        "op '{}' must not carry a value; remove `value:` from the condition",
                        cond.op.op_name()
                    ),
                ));
            }
        }
        HintOp::Eq | HintOp::Ne => {
            // R8: any non-null value is allowed (the runtime
            // compares via serde_json::Value PartialEq).
            // We only flag the `null` default since that is
            // almost always a forgotten `value:`.
            if cond.value.is_null() {
                out.push(value_shape_finding(
                    topic,
                    &hint.label,
                    format!(
                        "op '{}' requires a `value:`; null is treated as 'never matches' \
                         by the runtime",
                        cond.op.op_name()
                    ),
                ));
            }
        }
        HintOp::Gt | HintOp::Gte | HintOp::Lt | HintOp::Lte => {
            // R8 / R9: numeric comparison ops require a
            // JSON number; otherwise the runtime returns
            // false on the condition, which the lint
            // cannot see through. We must catch this at
            // lint time.
            match cond.value.as_f64() {
                Some(_) => {}
                None => {
                    out.push(value_shape_finding(
                        topic,
                        &hint.label,
                        format!(
                            "op '{}' requires a JSON number `value:`; got {:?}",
                            cond.op.op_name(),
                            cond.value
                        ),
                    ));
                }
            }
        }
    }
}

impl HintOp {
    /// Stable lowercase op name used in lint messages. We
    /// compute it locally rather than relying on serde's
    /// `Debug`/`Display` so the message stays a stable
    /// agent-facing string independent of the enum's
    /// formatting choices.
    fn op_name(&self) -> &'static str {
        match self {
            HintOp::Eq => "eq",
            HintOp::Ne => "ne",
            HintOp::Gt => "gt",
            HintOp::Gte => "gte",
            HintOp::Lt => "lt",
            HintOp::Lte => "lte",
            HintOp::Exists => "exists",
            HintOp::Missing => "missing",
            HintOp::Unknown(_) => "unknown",
        }
    }
}

fn unknown_field_finding(topic: &str, field: &str, surface: &str) -> LintFinding {
    LintFinding {
        id: FINDING_TRIGGER_CONTEXT_UNKNOWN_FIELD,
        severity: crate::preset_lint::LintSeverity::Error,
        message: format!(
            "schema topic \"{topic}\" trigger_context.{surface} references \
             unknown field \"{field}\"; add the field to required_fields, known_fields, \
             field_docs, or allowed_values first"
        ),
        topic: Some(topic.to_string()),
        hat: None,
        owner: None,
        action_hint: Some(format!(
            "add `{field}` to one of: required_fields, known_fields, field_docs, \
             allowed_values under event_policy.schemas.{topic}"
        )),
    }
}

fn unknown_field_finding_in_hint(topic: &str, hint_label: &str, field: &str) -> LintFinding {
    LintFinding {
        id: FINDING_TRIGGER_CONTEXT_UNKNOWN_FIELD,
        severity: crate::preset_lint::LintSeverity::Error,
        message: format!(
            "schema topic \"{topic}\" trigger_context routing_hints[\"{hint_label}\"] \
             condition references unknown field \"{field}\""
        ),
        topic: Some(topic.to_string()),
        hat: None,
        owner: None,
        action_hint: Some(format!(
            "add `{field}` to one of: required_fields, known_fields, field_docs, \
             allowed_values under event_policy.schemas.{topic}"
        )),
    }
}

fn unsupported_predicate_finding(topic: &str, hint_label: &str, raw: &str) -> LintFinding {
    LintFinding {
        id: FINDING_TRIGGER_CONTEXT_UNSUPPORTED_PREDICATE,
        severity: crate::preset_lint::LintSeverity::Error,
        message: format!(
            "schema topic \"{topic}\" trigger_context routing_hints[\"{hint_label}\"] \
             uses unsupported predicate op \"{raw}\"; the v1 allowlist is \
             eq, ne, gt, gte, lt, lte, exists, missing"
        ),
        topic: Some(topic.to_string()),
        hat: None,
        owner: None,
        action_hint: Some(
            "replace the predicate with one from the v1 allowlist: \
             eq, ne, gt, gte, lt, lte, exists, missing"
                .to_string(),
        ),
    }
}

fn value_shape_finding(topic: &str, hint_label: &str, message: String) -> LintFinding {
    LintFinding {
        id: FINDING_TRIGGER_CONTEXT_VALUE_SHAPE,
        severity: crate::preset_lint::LintSeverity::Error,
        message: format!(
            "schema topic \"{topic}\" trigger_context routing_hints[\"{hint_label}\"] \
             {message}"
        ),
        topic: Some(topic.to_string()),
        hat: None,
        owner: None,
        action_hint: None,
    }
}

fn duplicate_label_finding(topic: &str, label: &str) -> LintFinding {
    LintFinding {
        id: FINDING_TRIGGER_CONTEXT_DUPLICATE_LABEL,
        severity: crate::preset_lint::LintSeverity::Error,
        message: format!(
            "schema topic \"{topic}\" trigger_context routing_hints declares \
             duplicate label \"{label}\"; each hint must have a unique label"
        ),
        topic: Some(topic.to_string()),
        hat: None,
        owner: None,
        action_hint: Some(format!(
            "rename one of the duplicate \"{label}\" labels so each hint has a \
             unique stable identifier"
        )),
    }
}

#[cfg(test)]
mod u6_trigger_context_lint_tests {
    //! U4 acceptance tests. These tests pin the lint output
    //! for the four rules in this module. The lint is
    //! strict-only by design (R3 / R29): default mode skips
    //! the check entirely so undeclared presets see no
    //! behaviour change.
    use super::*;
    use crate::config::HintCondition;
    use serde_json::json;
    use std::collections::HashMap;

    fn schema_with_trigger_context(cfg: TriggerContextConfig) -> EventSchema {
        EventSchema {
            trigger_context: cfg,
            required_fields: vec!["known_field".to_string()],
            ..Default::default()
        }
    }

    fn empty_schemas() -> HashMap<String, EventSchema> {
        HashMap::new()
    }

    fn map_with(topic: &str, schema: EventSchema) -> HashMap<String, EventSchema> {
        let mut m = HashMap::new();
        m.insert(topic.to_string(), schema);
        m
    }

    /// 1. Default strictness is a hard no-op (R3 / R29).
    #[test]
    fn u4_default_strictness_is_noop() {
        let schema = schema_with_trigger_context(TriggerContextConfig {
            summary_fields: vec!["unknown_field".to_string()],
            ..Default::default()
        });
        let findings =
            check_trigger_context(&map_with("any.topic", schema), LintStrictness::Default);
        assert!(
            findings.is_empty(),
            "default mode must skip trigger_context lint to preserve the R3 contract, got: {findings:?}"
        );
    }

    /// 2. Empty `trigger_context` declaration yields no
    /// findings (R3 / R29 / SC6).
    #[test]
    fn u4_empty_trigger_context_is_noop() {
        let schema = EventSchema::default();
        let findings =
            check_trigger_context(&map_with("any.topic", schema), LintStrictness::Strict);
        assert!(findings.is_empty());
    }

    /// 3. Known summary field passes.
    #[test]
    fn u4_known_summary_field_passes() {
        let schema = schema_with_trigger_context(TriggerContextConfig {
            summary_fields: vec!["known_field".to_string()],
            ..Default::default()
        });
        let findings =
            check_trigger_context(&map_with("any.topic", schema), LintStrictness::Strict);
        assert!(findings.is_empty(), "got unexpected findings: {findings:?}");
    }

    /// 4. Unknown summary field is reported as
    /// `FINDING_TRIGGER_CONTEXT_UNKNOWN_FIELD`.
    #[test]
    fn u4_unknown_summary_field_reports_error() {
        let schema = schema_with_trigger_context(TriggerContextConfig {
            summary_fields: vec!["unknown_count".to_string()],
            ..Default::default()
        });
        let findings =
            check_trigger_context(&map_with("any.topic", schema), LintStrictness::Strict);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, FINDING_TRIGGER_CONTEXT_UNKNOWN_FIELD);
        assert!(findings[0].message.contains("unknown_count"));
        assert!(findings[0].message.contains("any.topic"));
        assert!(
            findings[0]
                .message
                .contains("trigger_context.summary_fields")
        );
    }

    /// 5. Unknown hint condition field reports the same
    /// finding with the hint label in the message.
    #[test]
    fn u4_unknown_condition_field_reports_error_with_hint_label() {
        let schema = schema_with_trigger_context(TriggerContextConfig {
            routing_hints: vec![RoutingHintConfig {
                label: "broken_hint".to_string(),
                exclusive_group: String::new(),
                conditions: vec![HintCondition {
                    field: "ghost_field".to_string(),
                    op: HintOp::Exists,
                    value: json!(null),
                }],
                guidance: "never runs".to_string(),
            }],
            ..Default::default()
        });
        let findings =
            check_trigger_context(&map_with("any.topic", schema), LintStrictness::Strict);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, FINDING_TRIGGER_CONTEXT_UNKNOWN_FIELD);
        assert!(findings[0].message.contains("ghost_field"));
        assert!(findings[0].message.contains("broken_hint"));
    }

    /// 6. Unknown op surfaces as the
    /// `FINDING_TRIGGER_CONTEXT_UNSUPPORTED_PREDICATE` finding
    /// (R20 / SC5).
    #[test]
    fn u4_unknown_op_reports_unsupported_predicate() {
        let schema = schema_with_trigger_context(TriggerContextConfig {
            routing_hints: vec![RoutingHintConfig {
                label: "bad_predicate".to_string(),
                exclusive_group: String::new(),
                conditions: vec![HintCondition {
                    field: "known_field".to_string(),
                    op: HintOp::Unknown("contains".to_string()),
                    value: json!("hot"),
                }],
                guidance: "should never run".to_string(),
            }],
            ..Default::default()
        });
        let findings =
            check_trigger_context(&map_with("any.topic", schema), LintStrictness::Strict);
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].id,
            FINDING_TRIGGER_CONTEXT_UNSUPPORTED_PREDICATE
        );
        assert!(findings[0].message.contains("contains"));
    }

    /// 7. `gt` with a non-numeric value is reported as
    /// value-shape error.
    #[test]
    fn u4_numeric_op_with_non_number_value_reports_error() {
        let schema = schema_with_trigger_context(TriggerContextConfig {
            routing_hints: vec![RoutingHintConfig {
                label: "broken_value".to_string(),
                exclusive_group: String::new(),
                conditions: vec![HintCondition {
                    field: "known_field".to_string(),
                    op: HintOp::Gt,
                    value: json!("not a number"),
                }],
                guidance: "never matches".to_string(),
            }],
            ..Default::default()
        });
        let findings =
            check_trigger_context(&map_with("any.topic", schema), LintStrictness::Strict);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, FINDING_TRIGGER_CONTEXT_VALUE_SHAPE);
        assert!(findings[0].message.contains("JSON number"));
    }

    /// 8. `exists` with a `value` is reported (R8 / R9).
    #[test]
    fn u4_exists_with_value_reports_error() {
        let schema = schema_with_trigger_context(TriggerContextConfig {
            routing_hints: vec![RoutingHintConfig {
                label: "exists_with_value".to_string(),
                exclusive_group: String::new(),
                conditions: vec![HintCondition {
                    field: "known_field".to_string(),
                    op: HintOp::Exists,
                    value: json!(42),
                }],
                guidance: "should not carry value".to_string(),
            }],
            ..Default::default()
        });
        let findings =
            check_trigger_context(&map_with("any.topic", schema), LintStrictness::Strict);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, FINDING_TRIGGER_CONTEXT_VALUE_SHAPE);
    }

    /// 9. Duplicate hint label is reported (R11 / SC5).
    #[test]
    fn u4_duplicate_hint_label_reports_error() {
        let schema = schema_with_trigger_context(TriggerContextConfig {
            routing_hints: vec![
                RoutingHintConfig {
                    label: "shared".to_string(),
                    exclusive_group: String::new(),
                    conditions: vec![HintCondition {
                        field: "known_field".to_string(),
                        op: HintOp::Exists,
                        value: json!(null),
                    }],
                    guidance: "first".to_string(),
                },
                RoutingHintConfig {
                    label: "shared".to_string(),
                    exclusive_group: String::new(),
                    conditions: vec![HintCondition {
                        field: "known_field".to_string(),
                        op: HintOp::Missing,
                        value: json!(null),
                    }],
                    guidance: "second".to_string(),
                },
            ],
            ..Default::default()
        });
        let findings =
            check_trigger_context(&map_with("any.topic", schema), LintStrictness::Strict);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, FINDING_TRIGGER_CONTEXT_DUPLICATE_LABEL);
        assert!(findings[0].message.contains("shared"));
    }

    /// 10. Different labels with non-overlapping conditions do
    /// NOT trip the duplicate-label finding (R11 sanity
    /// check).
    #[test]
    fn u4_distinct_labels_do_not_report_duplicate() {
        let schema = schema_with_trigger_context(TriggerContextConfig {
            routing_hints: vec![
                RoutingHintConfig {
                    label: "first".to_string(),
                    exclusive_group: String::new(),
                    conditions: vec![HintCondition {
                        field: "known_field".to_string(),
                        op: HintOp::Exists,
                        value: json!(null),
                    }],
                    guidance: "first".to_string(),
                },
                RoutingHintConfig {
                    label: "second".to_string(),
                    exclusive_group: String::new(),
                    conditions: vec![HintCondition {
                        field: "known_field".to_string(),
                        op: HintOp::Missing,
                        value: json!(null),
                    }],
                    guidance: "second".to_string(),
                },
            ],
            ..Default::default()
        });
        let findings =
            check_trigger_context(&map_with("any.topic", schema), LintStrictness::Strict);
        assert!(findings.is_empty(), "got unexpected findings: {findings:?}");
    }

    /// 11. Unknown op with a value shape problem reports
    /// ONLY the unsupported-predicate finding — the value
    /// shape finding is suppressed so the operator sees the
    /// root cause, not a cascade.
    #[test]
    fn u4_unknown_op_suppresses_cascading_value_shape_finding() {
        let schema = schema_with_trigger_context(TriggerContextConfig {
            routing_hints: vec![RoutingHintConfig {
                label: "broken".to_string(),
                exclusive_group: String::new(),
                conditions: vec![HintCondition {
                    field: "known_field".to_string(),
                    op: HintOp::Unknown("contains".to_string()),
                    // Gt-shaped value that would normally trip
                    // the value-shape finding.
                    value: json!("not a number"),
                }],
                guidance: "never matches".to_string(),
            }],
            ..Default::default()
        });
        let findings =
            check_trigger_context(&map_with("any.topic", schema), LintStrictness::Strict);
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].id,
            FINDING_TRIGGER_CONTEXT_UNSUPPORTED_PREDICATE
        );
    }

    /// 12. Empty schema map: no findings.
    #[test]
    fn u4_empty_schemas_map_produces_no_findings() {
        let findings = check_trigger_context(&empty_schemas(), LintStrictness::Strict);
        assert!(findings.is_empty());
    }

    /// 13. Numeric `gt` with a number value passes.
    #[test]
    fn u4_numeric_op_with_number_passes() {
        let schema = schema_with_trigger_context(TriggerContextConfig {
            routing_hints: vec![RoutingHintConfig {
                label: "gt_zero".to_string(),
                exclusive_group: String::new(),
                conditions: vec![HintCondition {
                    field: "known_field".to_string(),
                    op: HintOp::Gt,
                    value: json!(0),
                }],
                guidance: "ok".to_string(),
            }],
            ..Default::default()
        });
        let findings =
            check_trigger_context(&map_with("any.topic", schema), LintStrictness::Strict);
        assert!(findings.is_empty(), "got unexpected findings: {findings:?}");
    }

    /// 14. `eq` with a non-null value passes shape check
    /// (any JSON value is valid for eq / ne).
    #[test]
    fn u4_eq_with_non_null_value_passes() {
        let schema = schema_with_trigger_context(TriggerContextConfig {
            routing_hints: vec![RoutingHintConfig {
                label: "eq_zero".to_string(),
                exclusive_group: String::new(),
                conditions: vec![HintCondition {
                    field: "known_field".to_string(),
                    op: HintOp::Eq,
                    value: json!("anything"),
                }],
                guidance: "ok".to_string(),
            }],
            ..Default::default()
        });
        let findings =
            check_trigger_context(&map_with("any.topic", schema), LintStrictness::Strict);
        assert!(findings.is_empty(), "got unexpected findings: {findings:?}");
    }

    /// 15. `eq` with a null value reports value-shape error
    /// (operators who forget the value: field).
    #[test]
    fn u4_eq_with_null_value_reports_error() {
        let schema = schema_with_trigger_context(TriggerContextConfig {
            routing_hints: vec![RoutingHintConfig {
                label: "eq_null".to_string(),
                exclusive_group: String::new(),
                conditions: vec![HintCondition {
                    field: "known_field".to_string(),
                    op: HintOp::Eq,
                    value: json!(null),
                }],
                guidance: "forgot value".to_string(),
            }],
            ..Default::default()
        });
        let findings =
            check_trigger_context(&map_with("any.topic", schema), LintStrictness::Strict);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, FINDING_TRIGGER_CONTEXT_VALUE_SHAPE);
    }
}

#[cfg(test)]
mod u7_trigger_context_topology_lint_tests {
    //! U5 acceptance tests. These tests pin the
    //! topology-aware half of the trigger context lint.
    //! The default-mode no-op (R3 / R29) is exercised in
    //! the U6 test module above and is not duplicated
    //! here.
    use super::*;
    use crate::config::hat::HatConfig;
    use std::collections::HashMap;

    fn schema_with_trigger_context(cfg: TriggerContextConfig) -> EventSchema {
        EventSchema {
            trigger_context: cfg,
            required_fields: vec!["known_field".to_string()],
            ..Default::default()
        }
    }

    fn config_with_hats_and_policy(
        hats: Vec<(&str, Vec<&str>)>,
        schemas: Vec<(&str, EventSchema)>,
    ) -> RalphConfig {
        let mut config = RalphConfig::default();
        for (id, triggers) in hats {
            let mut hat = HatConfig::default();
            hat.name = id.to_string();
            hat.triggers = triggers.into_iter().map(|s| s.to_string()).collect();
            config.hats.insert(id.to_string(), hat);
        }
        let mut policy: crate::config::EventPolicyConfig = Default::default();
        for (topic, schema) in schemas {
            policy.schemas.insert(topic.to_string(), schema);
        }
        config.event_loop.event_policy = Some(policy);
        config
    }

    /// 1. Default strictness is a no-op (R3 / R29).
    #[test]
    fn u5_default_strictness_is_noop() {
        let schema = schema_with_trigger_context(TriggerContextConfig {
            summary_fields: vec!["known_field".into()],
            ..Default::default()
        });
        let config = config_with_hats_and_policy(
            vec![("a", vec!["any.topic"])],
            vec![("any.topic", schema)],
        );
        let findings = check_trigger_context_topology(&config, LintStrictness::Default);
        assert!(
            findings.is_empty(),
            "default mode must skip topology lint, got: {findings:?}"
        );
    }

    /// 2. No `event_policy` ⇒ no findings (the U4 / U5
    /// topology check has nothing to evaluate).
    #[test]
    fn u5_no_event_policy_is_noop() {
        let config = RalphConfig::default();
        let findings = check_trigger_context_topology(&config, LintStrictness::Strict);
        assert!(findings.is_empty());
    }

    /// 3. Schema declares trigger_context AND a hat
    /// subscribes → no finding.
    #[test]
    fn u5_subscriber_keeps_block_alive() {
        let schema = schema_with_trigger_context(TriggerContextConfig {
            summary_fields: vec!["known_field".into()],
            ..Default::default()
        });
        let config = config_with_hats_and_policy(
            vec![("reviewer", vec!["review.synthesized"])],
            vec![("review.synthesized", schema)],
        );
        let findings = check_trigger_context_topology(&config, LintStrictness::Strict);
        assert!(findings.is_empty(), "got unexpected findings: {findings:?}");
    }

    /// 4. No hat subscribes → `FINDING_TRIGGER_CONTEXT_NO_CONSUMER`.
    #[test]
    fn u5_no_consumer_reports_error() {
        let schema = schema_with_trigger_context(TriggerContextConfig {
            summary_fields: vec!["known_field".into()],
            ..Default::default()
        });
        let config = config_with_hats_and_policy(
            // Subscriber is for a *different* topic.
            vec![("reviewer", vec!["other.topic"])],
            vec![("review.synthesized", schema)],
        );
        let findings = check_trigger_context_topology(&config, LintStrictness::Strict);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, FINDING_TRIGGER_CONTEXT_NO_CONSUMER);
        assert!(findings[0].message.contains("review.synthesized"));
    }

    /// 5. Glob trigger `review.*` matches `review.synthesized`.
    #[test]
    fn u5_glob_trigger_satisfies_consumer_check() {
        let schema = schema_with_trigger_context(TriggerContextConfig {
            summary_fields: vec!["known_field".into()],
            ..Default::default()
        });
        let config = config_with_hats_and_policy(
            vec![("reviewer", vec!["review.*"])],
            vec![("review.synthesized", schema)],
        );
        let findings = check_trigger_context_topology(&config, LintStrictness::Strict);
        assert!(findings.is_empty(), "got unexpected findings: {findings:?}");
    }

    /// 6. Empty trigger_context declaration on a topic with
    /// no subscriber is NOT a no-consumer finding (the
    /// block is empty, so there is nothing to leak).
    #[test]
    fn u5_empty_trigger_context_is_not_a_no_consumer_finding() {
        let schema = EventSchema::default();
        let config = config_with_hats_and_policy(vec![], vec![("review.synthesized", schema)]);
        let findings = check_trigger_context_topology(&config, LintStrictness::Strict);
        assert!(
            findings.is_empty(),
            "empty trigger_context must not trip no-consumer, got: {findings:?}"
        );
    }

    /// 7. Multiple hats, only one subscribes → block is
    /// alive (R21 / R22 contract is "any subscriber keeps
    /// it alive", not "all hats must subscribe").
    #[test]
    fn u5_single_subscriber_among_many_keeps_block_alive() {
        let schema = schema_with_trigger_context(TriggerContextConfig {
            summary_fields: vec!["known_field".into()],
            ..Default::default()
        });
        let config = config_with_hats_and_policy(
            vec![
                ("reviewer", vec!["other.topic"]),
                ("gate", vec!["review.synthesized"]),
                ("alignment", vec!["third.topic"]),
            ],
            vec![("review.synthesized", schema)],
        );
        let findings = check_trigger_context_topology(&config, LintStrictness::Strict);
        assert!(findings.is_empty(), "got unexpected findings: {findings:?}");
    }
}
