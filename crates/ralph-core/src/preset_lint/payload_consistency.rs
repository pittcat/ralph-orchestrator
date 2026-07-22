//! plan 2026-07-22-004 (U5): `payload_consistency` rule sanity lint.
//!
//! The runtime evaluator in
//! [`crate::event_policy_payload_consistency`] is fail-close: a
//! malformed rule (unknown field, broken shape) evaluates to `Hit`
//! and rejects every event on its topic. That is the correct runtime
//! posture, but it means a misconfigured rule silently turns a
//! preset's `fix.done` (or any gated topic) into a hard reject at
//! runtime. This lint surfaces the misconfiguration at preset-load
//! time instead.
//!
//! Rules covered (R6 / R3 / S6 / S3):
//!
//! - [`FINDING_PAYLOAD_CONSISTENCY_DUPLICATE_ID`] — two rules in the
//!   same preset share an `id`. The `id` is the stable identifier the
//!   runtime embeds in the `payload_consistency:<id>` gate; duplicates
//!   scramble the agent-facing rejection reason.
//!
//! - [`FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_TOPIC`] — a rule's `topic`
//!   has no entry in `event_policy.schemas`. Without a schema the rule
//!   references a topic the policy does not otherwise validate, which
//!   is almost always a typo.
//!
//! - [`FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_FIELD`] — a `field`
//!   referenced anywhere in the rule's `when` (recursively through
//!   `all` / `any`) is not declared on the topic's schema
//!   (`required_fields ∪ known_fields ∪ field_docs ∪ allowed_values ∪
//!   element_constraints`). The runtime evaluator would treat the
//!   missing field as a miss for that predicate, so the rule can never
//!   fire as authored — a silent correctness bug.
//!
//! Severity follows the established warn-by-default pattern
//! ([`LintStrictness::ownership_severity`]): `Warn` in default mode,
//! `Error` in strict. The finding ids use a distinct
//! `payload_consistency` prefix so they never collide with the
//! `trigger_context_*` family (which validates a different block).

use std::collections::HashSet;

use serde_json::Value;

use crate::config::{EventSchema, RalphConfig};
use crate::event_policy_payload_consistency::WHITELISTED_PREDICATE_OPS;
use crate::preset_lint::finding_id::{
    FINDING_PAYLOAD_CONSISTENCY_DUPLICATE_ID, FINDING_PAYLOAD_CONSISTENCY_NON_OBJECT_WHEN,
    FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_FIELD, FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_OP,
    FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_TOPIC,
};
use crate::preset_lint::{LintFinding, LintSeverity, LintStrictness};

/// Validate every `event_policy.payload_consistency.rules[]` entry in
/// the preset. See the module docs for the three rules. Returns an
/// empty list when the preset has no `event_policy` or declares no
/// rules (a preset that does not opt in sees no behaviour change).
pub fn check_payload_consistency(
    config: &RalphConfig,
    strictness: LintStrictness,
) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let Some(policy) = config.event_loop.event_policy.as_ref() else {
        return findings;
    };
    let rules = &policy.payload_consistency.rules;
    if rules.is_empty() {
        return findings;
    }

    // Warn-by-default, Error in strict — matches the ownership /
    // schema-parity pattern (`LintStrictness::ownership_severity`).
    let severity = strictness.ownership_severity();

    // Rule 1: rule ids must be unique within the preset.
    let mut seen_ids: HashSet<String> = HashSet::new();
    for rule in rules {
        if !seen_ids.insert(rule.id.clone()) {
            findings.push(duplicate_id_finding(severity, &rule.id));
        }
    }

    // Rules 2 + 3 + 4 + 5: topic must exist in the schema map, every field
    // referenced in `when` must be declared on that topic's schema, the
    // `when` must be a JSON object, and every predicate op inside `when`
    // must be in the runtime whitelist. The object-shape and op-whitelist
    // checks (adversarial:A1) mirror the runtime fail-close paths in
    // `event_policy_payload_consistency` so a typo'd rule is rejected at
    // preset-load time instead of turning the gated topic into a hard
    // runtime reject.
    for rule in rules {
        let when_is_object = matches!(rule.when, Value::Object(_));
        if !when_is_object {
            findings.push(non_object_when_finding(
                severity,
                &rule.id,
                json_kind_label(&rule.when),
            ));
            // No structured shape to walk further (no fields, no ops);
            // the non-object finding is the actionable root cause.
            continue;
        }

        // Predicate-shape (unknown op) check runs even when the topic is
        // unknown: the op whitelist is the runtime's first line of defence
        // and a typo'd op on a typo'd topic is still a typo.
        let mut seen_ops: HashSet<String> = HashSet::new();
        for op in collect_when_predicate_ops(&rule.when) {
            if seen_ops.insert(op.clone()) && !WHITELISTED_PREDICATE_OPS.contains(&op.as_str()) {
                findings.push(unknown_op_finding(severity, &rule.id, &op));
            }
        }

        let Some(schema) = policy.schemas.get(&rule.topic) else {
            findings.push(unknown_topic_finding(severity, &rule.id, &rule.topic));
            // No schema to enumerate fields from; the unknown-topic
            // finding is the actionable root cause, so skip field checks.
            continue;
        };
        let known = schema_field_union(schema);
        let mut fields: Vec<String> = Vec::new();
        collect_when_fields(&rule.when, &mut fields);
        // Deduplicate field references so one unknown field referenced
        // twice produces a single finding.
        let mut checked: HashSet<String> = HashSet::new();
        for field in fields {
            if checked.insert(field.clone()) && !known.contains(&field) {
                findings.push(unknown_field_finding(
                    severity,
                    &rule.id,
                    &rule.topic,
                    &field,
                ));
            }
        }
    }

    findings
}

/// Walk a parsed `when` predicate and collect every `field` reference,
/// recursing through `all` / `any` combinators. Mirrors the evaluator's
/// combinator shape in [`crate::event_policy_payload_consistency`].
fn collect_when_fields(when: &Value, out: &mut Vec<String>) {
    let Value::Object(obj) = when else {
        return;
    };
    if let Some(Value::Array(items)) = obj.get("all").or_else(|| obj.get("any")) {
        for item in items {
            collect_when_fields(item, out);
        }
    }
    if let Some(Value::String(field)) = obj.get("field") {
        out.push(field.clone());
    }
}

/// Walk a parsed `when` predicate and collect every predicate op key
/// (i.e. every object key that is NOT a combinator key `all` / `any` and
/// NOT the `field` selector), recursing through combinators. The set of
/// op names returned is then compared against the runtime whitelist
/// [`WHITELISTED_PREDICATE_OPS`] to surface typos at preset-load time.
///
/// Anything that is not a JSON object is skipped at the helper level —
/// the top-level non-object-when guard is responsible for surfacing
/// that case via [`FINDING_PAYLOAD_CONSISTENCY_NON_OBJECT_WHEN`] before
/// this helper runs.
fn collect_when_predicate_ops(when: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    collect_when_predicate_ops_inner(when, &mut out);
    out
}

fn collect_when_predicate_ops_inner(when: &Value, out: &mut Vec<String>) {
    let Value::Object(obj) = when else {
        return;
    };
    if let Some(Value::Array(items)) = obj.get("all").or_else(|| obj.get("any")) {
        for item in items {
            collect_when_predicate_ops_inner(item, out);
        }
    }
    for (key, value) in obj {
        match key.as_str() {
            "all" | "any" | "field" => {}
            _ => {
                if value.is_object() {
                    continue;
                }
                out.push(key.clone());
            }
        }
    }
}

/// Human-readable label of a non-object `when` shape so the agent can
/// tell scalar / array / null cases apart at a glance.
fn json_kind_label(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// The set of fields a topic's schema declares — the reference surface
/// a `payload_consistency` rule may legitimately predicate on. Mirrors
/// `trigger_context::collect_known_fields` and widens it with
/// `element_constraints` keys (array fields the runtime validates
/// per-element).
fn schema_field_union(schema: &EventSchema) -> HashSet<String> {
    let mut out: HashSet<String> = HashSet::new();
    out.extend(schema.required_fields.iter().cloned());
    out.extend(schema.known_fields.iter().cloned());
    out.extend(schema.field_docs.keys().cloned());
    out.extend(schema.allowed_values.keys().cloned());
    out.extend(schema.element_constraints.keys().cloned());
    out
}

fn duplicate_id_finding(severity: LintSeverity, id: &str) -> LintFinding {
    LintFinding {
        id: FINDING_PAYLOAD_CONSISTENCY_DUPLICATE_ID,
        severity,
        message: format!(
            "event_policy.payload_consistency declares duplicate rule id \"{id}\"; \
             each rule must have a unique id (the runtime embeds it in the \
             payload_consistency:<id> gate)"
        ),
        topic: None,
        hat: None,
        owner: None,
        action_hint: Some(format!(
            "rename one of the duplicate \"{id}\" rules so every payload_consistency \
             rule id is unique"
        )),
    }
}

fn unknown_topic_finding(severity: LintSeverity, rule_id: &str, topic: &str) -> LintFinding {
    LintFinding {
        id: FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_TOPIC,
        severity,
        message: format!(
            "payload_consistency rule \"{rule_id}\" targets topic \"{topic}\" which has \
             no entry in event_policy.schemas; the rule references a topic the policy \
             does not validate"
        ),
        topic: Some(topic.to_string()),
        hat: None,
        owner: None,
        action_hint: Some(format!(
            "add a schema for `{topic}` under event_policy.schemas, or fix the rule's \
             `topic` to an existing schema topic"
        )),
    }
}

fn unknown_field_finding(
    severity: LintSeverity,
    rule_id: &str,
    topic: &str,
    field: &str,
) -> LintFinding {
    LintFinding {
        id: FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_FIELD,
        severity,
        message: format!(
            "payload_consistency rule \"{rule_id}\" (topic \"{topic}\") references \
             unknown field \"{field}\"; the field is not declared on the topic schema, \
             so the predicate can never match"
        ),
        topic: Some(topic.to_string()),
        hat: None,
        owner: None,
        action_hint: Some(format!(
            "add `{field}` to the `{topic}` schema (required_fields, known_fields, \
             field_docs, allowed_values, or element_constraints), or fix the rule's \
             `field` reference"
        )),
    }
}

fn unknown_op_finding(severity: LintSeverity, rule_id: &str, op: &str) -> LintFinding {
    LintFinding {
        id: FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_OP,
        severity,
        message: format!(
            "payload_consistency rule \"{rule_id}\" references unknown op \"{op}\"; \
             the runtime whitelist is {op:?} and any unknown op causes the rule \
             to fail-close on every emit, rejecting the gated topic"
        ),
        topic: None,
        hat: None,
        owner: None,
        action_hint: Some(format!(
            "rename \"{op}\" to one of the whitelisted ops (eq / ne / gt / gte / \
             exists / non_empty) or remove the predicate from the rule"
        )),
    }
}

fn non_object_when_finding(severity: LintSeverity, rule_id: &str, kind: &str) -> LintFinding {
    LintFinding {
        id: FINDING_PAYLOAD_CONSISTENCY_NON_OBJECT_WHEN,
        severity,
        message: format!(
            "payload_consistency rule \"{rule_id}\" has a `when` that is a {kind} \
             instead of a JSON object; the runtime treats a non-object `when` as \
             fail-close Hit, rejecting every emit on the gated topic"
        ),
        topic: None,
        hat: None,
        owner: None,
        action_hint: Some(format!(
            "rewrite \"{rule_id}\" `when` as a JSON object: a single predicate \
             `{{field, <op>, value}}` or a combinator `{{all: [...]}}` / `{{any: [...]}}`"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EventPolicyConfig, EventSchema, PayloadConsistencyConfig};
    use serde_json::json;
    use std::collections::HashMap;

    /// Build a config with a `fix.done` schema (declaring the fields
    /// the real pipeline rule references) and the supplied rules.
    fn config_with_rules(rules: Vec<crate::config::PayloadConsistencyRule>) -> RalphConfig {
        let mut schema = EventSchema::default();
        schema.required_fields = vec![
            "review_verdict".to_string(),
            "fixes_applied".to_string(),
            "planned_fix_units".to_string(),
            "fix_status".to_string(),
            "post_verification_status".to_string(),
            "new_business_regressions_count".to_string(),
        ];
        let mut schemas: HashMap<String, EventSchema> = HashMap::new();
        schemas.insert("fix.done".to_string(), schema);
        let mut policy = EventPolicyConfig::default();
        policy.schemas = schemas;
        policy.payload_consistency = PayloadConsistencyConfig {
            enabled: true,
            rules,
        };
        let mut config = RalphConfig::default();
        config.event_loop.event_policy = Some(policy);
        config
    }

    fn rule(id: &str, topic: &str, when: Value) -> crate::config::PayloadConsistencyRule {
        crate::config::PayloadConsistencyRule {
            id: id.to_string(),
            topic: topic.to_string(),
            when,
            message: "test rule".to_string(),
        }
    }

    /// The canonical real rule shape (matches the pipeline preset).
    fn valid_rule() -> crate::config::PayloadConsistencyRule {
        rule(
            "fix-done-blocked-zero-fixes-applied",
            "fix.done",
            json!({"all": [
                {"field": "review_verdict", "eq": "blocked"},
                {"field": "fixes_applied", "eq": 0},
                {"field": "planned_fix_units", "non_empty": true},
                {"field": "fix_status", "eq": "applied"}
            ]}),
        )
    }

    fn has_finding(findings: &[LintFinding], id: &str) -> bool {
        findings.iter().any(|f| f.id == id)
    }

    /// 1. A well-formed rule (unique id, known topic, known fields)
    ///    produces no findings — in either strictness.
    #[test]
    fn valid_rule_passes() {
        let config = config_with_rules(vec![valid_rule()]);
        for strictness in [LintStrictness::Default, LintStrictness::Strict] {
            let findings = check_payload_consistency(&config, strictness);
            assert!(
                findings.is_empty(),
                "valid rule must pass under {strictness:?}, got {findings:?}"
            );
        }
    }

    /// 2. Duplicate rule id → `FINDING_PAYLOAD_CONSISTENCY_DUPLICATE_ID`.
    #[test]
    fn duplicate_id_fails() {
        let config = config_with_rules(vec![valid_rule(), valid_rule()]);
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        assert!(
            has_finding(&findings, FINDING_PAYLOAD_CONSISTENCY_DUPLICATE_ID),
            "duplicate id must be flagged, got {findings:?}"
        );
        assert!(findings[0].message.contains("fix-done-blocked-zero-fixes-applied"));
    }

    /// 3. Unknown topic → `FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_TOPIC`.
    #[test]
    fn unknown_topic_fails() {
        let config = config_with_rules(vec![rule(
            "r1",
            "no.such.topic",
            json!({"field": "review_verdict", "eq": "blocked"}),
        )]);
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        assert!(
            has_finding(&findings, FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_TOPIC),
            "unknown topic must be flagged, got {findings:?}"
        );
        assert!(findings.iter().any(|f| f.message.contains("no.such.topic")));
    }

    /// 4. Unknown field (referenced in `when`) →
    ///    `FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_FIELD`.
    #[test]
    fn unknown_field_fails() {
        let config = config_with_rules(vec![rule(
            "r1",
            "fix.done",
            json!({"all": [
                {"field": "review_verdict", "eq": "blocked"},
                {"field": "ghost_field", "eq": 1}
            ]}),
        )]);
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        assert!(
            has_finding(&findings, FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_FIELD),
            "unknown field must be flagged, got {findings:?}"
        );
        assert!(findings.iter().any(|f| f.message.contains("ghost_field")));
    }

    /// 5. Unknown field nested inside an `any` combinator is still
    ///    caught (recursion through combinators).
    #[test]
    fn unknown_field_nested_in_any_fails() {
        let config = config_with_rules(vec![rule(
            "r1",
            "fix.done",
            json!({"any": [
                {"field": "review_verdict", "eq": "blocked"},
                {"all": [{"field": "deep_ghost", "gt": 0}]}
            ]}),
        )]);
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        assert!(
            has_finding(&findings, FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_FIELD),
            "nested unknown field must be flagged, got {findings:?}"
        );
        assert!(findings.iter().any(|f| f.message.contains("deep_ghost")));
    }

    /// 6. Unknown predicate op is surfaced in both modes with the
    ///    strictness-appropriate severity.
    #[test]
    fn unknown_op_fails_with_strictness_severity() {
        let config = config_with_rules(vec![rule(
            "r1",
            "fix.done",
            json!({"field": "review_verdict", "eqz": "blocked"}),
        )]);

        for (strictness, expected_severity) in [
            (LintStrictness::Default, LintSeverity::Warn),
            (LintStrictness::Strict, LintSeverity::Error),
        ] {
            let findings = check_payload_consistency(&config, strictness);
            let matching: Vec<_> = findings
                .iter()
                .filter(|finding| {
                    finding.id
                        == crate::preset_lint::finding_id::FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_OP
                })
                .collect();
            assert_eq!(matching.len(), 1, "got {findings:?}");
            assert_eq!(matching[0].severity, expected_severity);
            assert!(matching[0].message.contains("eqz"));
        }
    }

    /// 7. A non-object `when` is surfaced in both modes with the
    ///    strictness-appropriate severity.
    #[test]
    fn non_object_when_fails_with_strictness_severity() {
        let config = config_with_rules(vec![rule(
            "r1",
            "fix.done",
            json!("literal"),
        )]);

        for (strictness, expected_severity) in [
            (LintStrictness::Default, LintSeverity::Warn),
            (LintStrictness::Strict, LintSeverity::Error),
        ] {
            let findings = check_payload_consistency(&config, strictness);
            let matching: Vec<_> = findings
                .iter()
                .filter(|finding| {
                    finding.id
                        == crate::preset_lint::finding_id::FINDING_PAYLOAD_CONSISTENCY_NON_OBJECT_WHEN
                })
                .collect();
            assert_eq!(matching.len(), 1, "got {findings:?}");
            assert_eq!(matching[0].severity, expected_severity);
        }
    }

    /// 8. Nested legal combinators and whitelisted predicate ops do not
    ///    trigger either shape finding.
    #[test]
    fn valid_nested_combinators_do_not_report_shape_findings() {
        let config = config_with_rules(vec![rule(
            "r1",
            "fix.done",
            json!({"all": [
                {"field": "review_verdict", "eq": "blocked"},
                {"any": [
                    {"field": "fixes_applied", "gte": 1},
                    {"field": "planned_fix_units", "non_empty": true}
                ]}
            ]}),
        )]);

        for strictness in [LintStrictness::Default, LintStrictness::Strict] {
            let findings = check_payload_consistency(&config, strictness);
            assert!(
                findings.iter().all(|finding| {
                    finding.id
                        != crate::preset_lint::finding_id::FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_OP
                        && finding.id
                            != crate::preset_lint::finding_id::FINDING_PAYLOAD_CONSISTENCY_NON_OBJECT_WHEN
                }),
                "valid nested predicates must not trigger shape findings under {strictness:?}, got {findings:?}"
            );
        }
    }

    /// 9. Default strictness grades findings as `Warn`; strict grades
    ///    them as `Error` (warn-by-default pattern).
    #[test]
    fn severity_follows_strictness() {
        let config = config_with_rules(vec![rule(
            "r1",
            "no.such.topic",
            json!({"field": "review_verdict", "eq": "blocked"}),
        )]);
        let default_findings = check_payload_consistency(&config, LintStrictness::Default);
        assert!(
            default_findings
                .iter()
                .all(|f| f.severity == LintSeverity::Warn)
        );
        let strict_findings = check_payload_consistency(&config, LintStrictness::Strict);
        assert!(strict_findings
            .iter()
            .all(|f| f.severity == LintSeverity::Error));
    }

    /// 7. No `event_policy` → no findings (opt-in surface).
    #[test]
    fn no_event_policy_is_noop() {
        let config = RalphConfig::default();
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        assert!(findings.is_empty());
    }

    /// 8. Empty rules list → no findings.
    #[test]
    fn empty_rules_is_noop() {
        let config = config_with_rules(vec![]);
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        assert!(findings.is_empty());
    }
}
