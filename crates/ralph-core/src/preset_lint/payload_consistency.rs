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

use crate::config::scope_topics::SCOPE_TOPICS;
use crate::config::{EventSchema, RalphConfig};
use crate::event_policy_payload_consistency::WHITELISTED_PREDICATE_OPS;
use crate::preset_lint::finding_id::{
    FINDING_PAYLOAD_CONSISTENCY_DUPLICATE_ID, FINDING_PAYLOAD_CONSISTENCY_NON_OBJECT_WHEN,
    FINDING_PAYLOAD_CONSISTENCY_SCOPE_POSITIVE_ASSERTION,
    FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_FIELD, FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_OP,
    FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_TOPIC, FINDING_PAYLOAD_CONSISTENCY_UNSAFE_MESSAGE,
};
use crate::preset_lint::{LintFinding, LintSeverity, LintStrictness};

/// Topics whose payload contracts own structural presence of legal
/// values. The four scope handoff topics — same set as
/// `check_scope_handoff_guard` in `crates/ralph-cli/src/policy_check/gates.rs`.
/// Rules on these topics are checked for the polarity anti-pattern
/// (positive existence / positive threshold against legal values) that
/// the runtime evaluator would silently convert into a hard reject.
///
/// Sourced from [`crate::config::scope_topics::SCOPE_TOPICS`] so the
/// polarity walker and the scope handoff guard never drift apart
/// (plan 2026-08-10-002 U4 / M3). The previous `PROTECTED_SCOPE_TOPICS`
/// literal was a verbatim 4-element duplicate; it is intentionally
/// re-exported as a thin alias for callers that already import it.
pub(super) const PROTECTED_SCOPE_TOPICS: &[&str] = SCOPE_TOPICS;

/// Structural fields whose legal value is presence of the field
/// (path / digest / status / base SHA / patch fields / predecessor /
/// merge boundary). A `payload_consistency` rule that asserts
/// `exists:true` or `non_empty:true` on one of these fields inside a
/// protected scope topic is a positive assertion against the legal
/// condition; runtime treats `Hit` as reject, so the rule would reject
/// every legitimate handoff. The typed scope guard and schema's
/// `required_fields` already enforce presence.
pub(super) const PROTECTED_SCOPE_STRUCTURAL_FIELDS: &[&str] = &[
    // scope manifest contract (all four scope topics).
    "scope_manifest_path",
    "scope_digest",
    "scope_status",
    "scope_base_sha",
    "scope_source",
    // patch artifact contract.
    "resolved_patch_path",
    "patch_digest",
    // redteam predecessor literal.
    "predecessor_event",
    // merge boundary contract.
    "merge_boundary_path",
    "merge_boundary_digest",
    "merge_boundary_status",
];

/// Threshold fields whose legal value is the resolved-side value
/// (`overall_confidence >= 90`, `critical_unknown_count == 0`,
/// `resolved_count >= 1`, `coverage >= 90`). Positive assertions are
/// only linted where a positive bound can reject a legal resolved value
/// (`overall_confidence`, `resolved_count`, `coverage`). Positive
/// predicates on `critical_unknown_count` describe an invalid state and
/// therefore remain available as negative contradiction rules.
pub(super) const PROTECTED_SCOPE_THRESHOLD_FIELDS: &[&str] = &[
    "overall_confidence",
    "critical_unknown_count",
    "resolved_count",
    "coverage",
];

/// Validate every `event_policy.payload_consistency.rules[]` entry in
/// the preset. See the module docs for the three rules. Returns an
/// empty list when the preset has no `event_policy` or declares no
/// rules (a preset that does not opt in sees no behaviour change).
///
/// U3 (2026-08-10-002 plan, R8/M4/S2): the 107-line monolith is split
/// into 5 named helpers — `check_id_uniqueness`,
/// `check_message_safety`, `check_predicate_shape`,
/// `check_field_known`, `check_scope_polarity` — so the polarity
/// walker becomes a structural fact (R7) instead of a fragile
/// `if`-branch buried inside one `for` loop. The order of helpers
/// is preserved from the original; the helper bodies mirror the
/// in-loop checks byte-for-byte so the lint output is stable.
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
    findings.extend(check_id_uniqueness(rules, severity));

    // Rules 2 + 3 + 4 + 5: the per-rule shape, schema, and polarity
    // checks. Each helper is responsible for one orthogonal
    // concern; the orchestrator below only concatenates the
    // finding lists.
    for rule in rules {
        findings.extend(check_message_safety(rule, severity));
        let shape = check_predicate_shape(rule, severity);
        let shape_skipped = shape.iter().any(|f| f.id == FINDING_PAYLOAD_CONSISTENCY_NON_OBJECT_WHEN);
        findings.extend(shape);
        if shape_skipped {
            // No structured shape to walk further (no fields, no ops);
            // the non-object finding is the actionable root cause.
            continue;
        }
        // 2026-08-10-002 plan U3 (R7/C2): scope polarity runs BEFORE
        // the `unknown_topic` short-circuit. Even when a rule's topic
        // is missing from `event_policy.schemas`, the polarity walker
        // still fires for any rule whose topic is in the canonical
        // `SCOPE_TOPICS` list — a missing schema means the field
        // check has nothing to walk, but the polarity anti-pattern
        // is observable from the `when` alone.
        findings.extend(check_scope_polarity(rule, severity));
        // Field-known check still depends on a schema; surface the
        // unknown-topic finding first when both apply.
        findings.extend(check_field_known(rule, policy, severity));
    }

    findings
}

/// Rule 1: rule ids must be unique within the preset.
fn check_id_uniqueness(
    rules: &[crate::config::PayloadConsistencyRule],
    severity: LintSeverity,
) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();
    for rule in rules {
        if !seen_ids.insert(rule.id.clone()) {
            findings.push(duplicate_id_finding(severity, &rule.id));
        }
    }
    findings
}

/// Rule 2: rule `message` is free of unsafe content (ANSI escapes,
/// control chars, zero-width chars) and is not excessively long. The
/// runtime `safe_display` API strips these at render time, but the
/// lint surfaces the misconfiguration at preset-load time so the
/// rule author fixes the message rather than relying on runtime
/// stripping.
fn check_message_safety(
    rule: &crate::config::PayloadConsistencyRule,
    severity: LintSeverity,
) -> Vec<LintFinding> {
    if let Some(reason) = check_message_unsafe(&rule.message) {
        vec![unsafe_message_finding(severity, &rule.id, reason)]
    } else {
        Vec::new()
    }
}

/// Rule 3: the `when` must be a JSON object and every predicate op
/// inside `when` (recursive through `all` / `any` combinators) must
/// be in the runtime whitelist `WHITELISTED_PREDICATE_OPS`. The
/// op-whitelist check mirrors the runtime fail-close path in
/// `event_policy_payload_consistency` so a typo'd op is rejected at
/// preset-load time instead of turning the gated topic into a hard
/// runtime reject.
///
/// Returns the list of findings; the caller uses the
/// `NON_OBJECT_WHEN` finding id as the "skip further rule checks"
/// signal (mirrors the original loop's `continue;` semantics).
fn check_predicate_shape(
    rule: &crate::config::PayloadConsistencyRule,
    severity: LintSeverity,
) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    if !matches!(rule.when, Value::Object(_)) {
        findings.push(non_object_when_finding(
            severity,
            &rule.id,
            json_kind_label(&rule.when),
        ));
        return findings;
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
    findings
}

/// Rule 4 (R7/C2 structural fix): scope polarity check. Fires for
/// any rule whose topic is in the canonical `SCOPE_TOPICS` list
/// regardless of whether the topic is in `event_policy.schemas`.
/// This is the structural inversion of the previous
/// `unknown_topic` short-circuit — the polarity anti-pattern is
/// observable from the `when` alone, so the walker no longer
/// depends on the schema being declared.
fn check_scope_polarity(
    rule: &crate::config::PayloadConsistencyRule,
    severity: LintSeverity,
) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    if !PROTECTED_SCOPE_TOPICS.contains(&rule.topic.as_str()) {
        return findings;
    }
    for positive in collect_scope_positive_assertions(&rule.when) {
        findings.push(scope_positive_assertion_finding(
            severity,
            &rule.id,
            &rule.topic,
            &positive.field,
            &positive.op,
        ));
    }
    findings
}

/// Rule 5: every `field` referenced in `when` (recursive through
/// `all` / `any`) must be declared on the topic's schema. Surfaces
/// the `unknown_topic` and `unknown_field` findings; the field
/// check is a no-op when the topic is not in `event_policy.schemas`.
fn check_field_known(
    rule: &crate::config::PayloadConsistencyRule,
    policy: &crate::config::EventPolicyConfig,
    severity: LintSeverity,
) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let Some(schema) = policy.schemas.get(&rule.topic) else {
        findings.push(unknown_topic_finding(severity, &rule.id, &rule.topic));
        return findings;
    };
    let known = schema_field_union(schema);
    let mut fields: Vec<String> = Vec::new();
    collect_when_fields(&rule.when, &mut fields);
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

/// One structural-field positive assertion detected inside a scope
/// topic's `when`. The lint walks `when` (recursing through `all` /
/// `any`) and records each predicate that asserts legal structural
/// presence (`exists:true`, `non_empty:true`) on a protected scope
/// structural field, or a positive bound (`gt: <positive>`, `gte:
/// <positive>`, `eq: <positive>`) on a protected scope threshold
/// field whose legal resolved-side value is fixed by
/// `policy_check/gates.rs` (overall_confidence >= 90,
/// critical_unknown_count == 0, resolved_count >= 1, coverage >= 90).
///
/// `op` is the predicate key that triggered the finding
/// (`exists` / `non_empty` / `gt` / `gte` / `eq`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScopePositiveAssertion {
    field: String,
    op: String,
}

/// 2026-08-10-002 plan U4 (R7 / D6): walk a `when` predicate and
/// collect every positive structural assertion on a protected scope
/// field. Mirrors the structural recursion of
/// [`collect_when_predicate_ops`] / [`collect_when_fields`]. The
/// runtime evaluator fails closed (`Hit` = reject), so a rule that
/// asserts the legal condition silently rejects every legitimate
/// handoff — the lint must surface this at preset-load time so the
/// typed scope guard / schema can own presence instead.
///
/// Only direct predicates count. A combinator (`all:` / `any:`) that
/// contains a positive assertion is reported once via recursion, with
/// the inner field/op reported on the rule id. We do NOT match `ne`,
/// `gt: 0`, `eq: 0`, or any other negative-bound form — those are the
/// legal same-payload contradiction shape (e.g. `{field:
/// critical_unknown_count, gt: 0}` paired with `scope_status ==
/// "resolved"`). The lint refuses to misfire on those because the
/// plan's D1/R3 requires the public evaluator's Hit/Miss semantics to
/// stay intact.
fn collect_scope_positive_assertions(when: &Value) -> Vec<ScopePositiveAssertion> {
    let mut out: Vec<ScopePositiveAssertion> = Vec::new();
    collect_scope_positive_assertions_inner(when, &mut out);
    out
}

fn collect_scope_positive_assertions_inner(
    when: &Value,
    out: &mut Vec<ScopePositiveAssertion>,
) {
    let Value::Object(obj) = when else {
        return;
    };
    if let Some(Value::Array(items)) = obj.get("all").or_else(|| obj.get("any")) {
        for item in items {
            collect_scope_positive_assertions_inner(item, out);
        }
    }
    let Some(Value::String(field)) = obj.get("field") else {
        return;
    };
    // Structural positive assertions: `exists: true` or
    // `non_empty: true` on a protected structural field.
    if PROTECTED_SCOPE_STRUCTURAL_FIELDS.contains(&field.as_str()) {
        if matches!(
            obj.get("exists"),
            Some(Value::Bool(true)) | Some(Value::Number(_)) | Some(Value::String(_))
        ) {
            out.push(ScopePositiveAssertion {
                field: field.clone(),
                op: "exists".to_string(),
            });
        }
        if matches!(obj.get("non_empty"), Some(Value::Bool(true))) {
            out.push(ScopePositiveAssertion {
                field: field.clone(),
                op: "non_empty".to_string(),
            });
        }
    }
    // Threshold positive assertions on a protected threshold field:
    // - `gt: <positive>` / `gte: <positive>` against a positive bound
    //   is a positive assertion on the legal value.
    // - `eq: <positive>` is also a positive assertion on a legal
    //   non-zero literal (e.g. `eq: 100` against overall_confidence
    //   when the legal value is exactly 100).
    //
    // We deliberately do NOT match `gt: 0`, `gte: 0`, `eq: 0`, or
    // other zero/negative-bound forms — those are the canonical
    // same-payload contradiction shape and must keep working for
    // `ce-executor-pipeline` and friends.
    if PROTECTED_SCOPE_THRESHOLD_FIELDS.contains(&field.as_str())
        && field != "critical_unknown_count"
    {
        if let Some(Value::Number(n)) = obj.get("gt") {
            if n.as_f64().unwrap_or(0.0) > 0.0 {
                out.push(ScopePositiveAssertion {
                    field: field.clone(),
                    op: "gt".to_string(),
                });
            }
        }
        if let Some(Value::Number(n)) = obj.get("gte") {
            if n.as_f64().unwrap_or(0.0) > 0.0 {
                out.push(ScopePositiveAssertion {
                    field: field.clone(),
                    op: "gte".to_string(),
                });
            }
        }
        if let Some(Value::Number(n)) = obj.get("eq") {
            if n.as_f64().unwrap_or(0.0) > 0.0 {
                out.push(ScopePositiveAssertion {
                    field: field.clone(),
                    op: "eq".to_string(),
                });
            }
        }
    }
}

fn scope_positive_assertion_finding(
    severity: LintSeverity,
    rule_id: &str,
    topic: &str,
    field: &str,
    op: &str,
) -> LintFinding {
    LintFinding {
        id: FINDING_PAYLOAD_CONSISTENCY_SCOPE_POSITIVE_ASSERTION,
        severity,
        message: format!(
            "payload_consistency rule \"{rule_id}\" on scope topic \"{topic}\" \
             declares a positive assertion `{op}: <legal>` against the protected \
             scope field \"{field}\"; the runtime evaluator treats `Hit` as \
             rejection, so this rule silently rejects every legitimate handoff. \
             The structural presence of \"{field}\" must be enforced by the typed \
             scope guard in `policy_check/gates.rs` or the schema's \
             `required_fields` / `allowed_values`, not by a same-payload rule"
        ),
        topic: Some(topic.to_string()),
        hat: None,
        owner: None,
        action_hint: Some(format!(
            "delete the rule \"{rule_id}\" and move the structural check for \
             \"{field}\" to the typed scope guard or the topic's schema; if the \
             rule is a legitimate contradiction (e.g. `gt: 0` on \
             `critical_unknown_count`), rewrite it in the negative-bound shape \
             with no `gt:`/`gte:`/`eq:` against a positive literal"
        )),
    }
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

/// U3 (2026-07-23-002 plan, KTD3): check a `payload_consistency`
/// rule's `message` for unsafe content. Returns `Some(reason)` when
/// the message is unsafe, `None` when it is clean.
///
/// Unsafe content is:
/// - Exceeds [`MAX_RULE_MESSAGE_BYTES`] UTF-8 bytes (1024).
/// - Contains ANSI escape sequences (CSI `ESC [` or OSC `ESC ]`).
/// - Contains C0 control characters except `\n` and `\t`.
/// - Contains C1 control characters (`U+0080`–`U+009F`).
/// - Contains zero-width characters (`U+200B`, `U+200C`, `U+200D`,
///   `U+FEFF`, `U+2060`, `U+00AD`).
fn check_message_unsafe(message: &str) -> Option<&'static str> {
    use crate::safe_display::MAX_RULE_MESSAGE_BYTES;

    if message.len() > MAX_RULE_MESSAGE_BYTES {
        return Some("exceeds the 1024-byte limit");
    }

    // Check for ANSI escape sequences and control characters.
    let bytes = message.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // ESC character — start of ANSI escape
        if bytes[i] == 0x1B {
            return Some("contains ANSI escape sequences");
        }
        // C0 control chars (except \n=0x0A and \t=0x09)
        if bytes[i] < 0x20 && bytes[i] != 0x0A && bytes[i] != 0x09 {
            return Some("contains C0 control characters");
        }
        i += 1;
    }

    // Check for C1 control chars and zero-width chars (multibyte).
    for ch in message.chars() {
        let code = ch as u32;
        if (0x80..=0x9F).contains(&code) {
            return Some("contains C1 control characters");
        }
        if matches!(
            ch,
            '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{2060}' | '\u{00AD}'
        ) {
            return Some("contains zero-width characters");
        }
    }

    None
}

fn unsafe_message_finding(severity: LintSeverity, rule_id: &str, reason: &str) -> LintFinding {
    LintFinding {
        id: FINDING_PAYLOAD_CONSISTENCY_UNSAFE_MESSAGE,
        severity,
        message: format!(
            "payload_consistency rule \"{rule_id}\" has a `message` that {reason}; \
             the runtime `safe_display` API will strip/truncate it at render time, \
             but the message should be clean diagnostic text"
        ),
        topic: None,
        hat: None,
        owner: None,
        action_hint: Some(format!(
            "rewrite \"{rule_id}\" `message` as plain diagnostic text without ANSI \
             escapes, control characters, zero-width characters, or excessive length \
             (≤ 1024 UTF-8 bytes)"
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
        assert!(
            findings[0]
                .message
                .contains("fix-done-blocked-zero-fixes-applied")
        );
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
        let config = config_with_rules(vec![rule("r1", "fix.done", json!("literal"))]);

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
        assert!(
            strict_findings
                .iter()
                .all(|f| f.severity == LintSeverity::Error)
        );
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

    // ── U3 (2026-07-23-002 plan, KTD3): message safety lint ──────

    /// Helper: build a rule with a custom message.
    fn rule_with_message(id: &str, message: &str) -> crate::config::PayloadConsistencyRule {
        crate::config::PayloadConsistencyRule {
            id: id.to_string(),
            topic: "fix.done".to_string(),
            when: json!({"field": "fix_status", "eq": "applied"}),
            message: message.to_string(),
        }
    }

    /// U3: a clean message produces no finding.
    #[test]
    fn clean_message_produces_no_finding() {
        let config = config_with_rules(vec![rule_with_message(
            "clean-rule",
            "fix_status=applied is inconsistent with review_verdict=blocked",
        )]);
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        let unsafe_findings: Vec<_> = findings
            .iter()
            .filter(|f| f.id == FINDING_PAYLOAD_CONSISTENCY_UNSAFE_MESSAGE)
            .collect();
        assert!(
            unsafe_findings.is_empty(),
            "clean message should not produce an unsafe-message finding, got {unsafe_findings:?}"
        );
    }

    /// U3: a message with ANSI escape sequences is flagged.
    #[test]
    fn ansi_escape_in_message_is_flagged() {
        let config = config_with_rules(vec![rule_with_message(
            "ansi-rule",
            "\x1b[31mred text\x1b[0m",
        )]);
        let findings = check_payload_consistency(&config, LintStrictness::Default);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_PAYLOAD_CONSISTENCY_UNSAFE_MESSAGE)
        );
    }

    /// U3: a message with C0 control characters (except \n/\t) is flagged.
    #[test]
    fn c0_control_in_message_is_flagged() {
        let config =
            config_with_rules(vec![rule_with_message("c0-rule", "has\x00null and\x01soh")]);
        let findings = check_payload_consistency(&config, LintStrictness::Default);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_PAYLOAD_CONSISTENCY_UNSAFE_MESSAGE)
        );
    }

    /// U3: newlines and tabs in message are allowed (legitimate
    /// multi-line diagnostic text).
    #[test]
    fn newline_and_tab_in_message_are_allowed() {
        let config = config_with_rules(vec![rule_with_message(
            "multiline-rule",
            "line one\nline two\tindented",
        )]);
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        let unsafe_findings: Vec<_> = findings
            .iter()
            .filter(|f| f.id == FINDING_PAYLOAD_CONSISTENCY_UNSAFE_MESSAGE)
            .collect();
        assert!(unsafe_findings.is_empty());
    }

    /// U3: a message with C1 control characters is flagged.
    #[test]
    fn c1_control_in_message_is_flagged() {
        let config = config_with_rules(vec![rule_with_message(
            "c1-rule",
            "has\u{0085}NEL and\u{009f}APC",
        )]);
        let findings = check_payload_consistency(&config, LintStrictness::Default);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_PAYLOAD_CONSISTENCY_UNSAFE_MESSAGE)
        );
    }

    /// U3: a message with zero-width characters is flagged.
    #[test]
    fn zero_width_in_message_is_flagged() {
        let config = config_with_rules(vec![rule_with_message("zw-rule", "fix\u{200B}_status")]);
        let findings = check_payload_consistency(&config, LintStrictness::Default);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_PAYLOAD_CONSISTENCY_UNSAFE_MESSAGE)
        );
    }

    /// U3: a message exceeding 1024 UTF-8 bytes is flagged.
    #[test]
    fn oversized_message_is_flagged() {
        let long_message = "x".repeat(1025);
        let config = config_with_rules(vec![rule_with_message("long-rule", &long_message)]);
        let findings = check_payload_consistency(&config, LintStrictness::Default);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_PAYLOAD_CONSISTENCY_UNSAFE_MESSAGE)
        );
    }

    /// U3: a message at exactly 1024 bytes is NOT flagged (boundary).
    #[test]
    fn message_at_byte_limit_is_not_flagged() {
        let message = "x".repeat(1024);
        let config = config_with_rules(vec![rule_with_message("boundary-rule", &message)]);
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        let unsafe_findings: Vec<_> = findings
            .iter()
            .filter(|f| f.id == FINDING_PAYLOAD_CONSISTENCY_UNSAFE_MESSAGE)
            .collect();
        assert!(unsafe_findings.is_empty());
    }

    /// U3: the finding is `Warn` in default mode, `Error` in strict.
    #[test]
    fn unsafe_message_finding_severity_follows_strictness() {
        let config = config_with_rules(vec![rule_with_message(
            "severity-rule",
            "\x1b[31mansi\x1b[0m",
        )]);
        let warn_findings = check_payload_consistency(&config, LintStrictness::Default);
        let strict_findings = check_payload_consistency(&config, LintStrictness::Strict);
        let warn_unsafe = warn_findings
            .iter()
            .find(|f| f.id == FINDING_PAYLOAD_CONSISTENCY_UNSAFE_MESSAGE)
            .expect("Warn mode should still surface the finding");
        assert_eq!(warn_unsafe.severity, LintSeverity::Warn);
        let strict_unsafe = strict_findings
            .iter()
            .find(|f| f.id == FINDING_PAYLOAD_CONSISTENCY_UNSAFE_MESSAGE)
            .expect("Strict mode should surface the finding");
        assert_eq!(strict_unsafe.severity, LintSeverity::Error);
    }

    // ── 2026-08-10-002 plan U4 (R7 / D6): scope polarity lint ─────

    /// Build a config with a `redteam.plan.resolved` schema declaring
    /// every protected scope field (so unknown-field checks don't
    /// interleave with the polarity check we want to exercise).
    fn scope_topic_config(
        topic: &str,
        rules: Vec<crate::config::PayloadConsistencyRule>,
    ) -> RalphConfig {
        let mut schema = EventSchema::default();
        schema.required_fields = vec![
            // Structural fields
            "scope_manifest_path".to_string(),
            "scope_digest".to_string(),
            "scope_status".to_string(),
            "scope_base_sha".to_string(),
            "resolved_patch_path".to_string(),
            "patch_digest".to_string(),
            "predecessor_event".to_string(),
            "merge_boundary_path".to_string(),
            "merge_boundary_digest".to_string(),
            "merge_boundary_status".to_string(),
            // Threshold fields
            "overall_confidence".to_string(),
            "critical_unknown_count".to_string(),
            "resolved_count".to_string(),
            "coverage".to_string(),
            // Pipeline fields (so cross-topic isolation tests can use the
            // same config builder without losing required_fields).
            "review_verdict".to_string(),
            "fixes_applied".to_string(),
        ];
        let mut schemas: HashMap<String, EventSchema> = HashMap::new();
        schemas.insert(topic.to_string(), schema);
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

    /// Helper: build a rule on the given topic.
    fn rule_on(topic: &str, id: &str, when: Value) -> crate::config::PayloadConsistencyRule {
        crate::config::PayloadConsistencyRule {
            id: id.to_string(),
            topic: topic.to_string(),
            when,
            message: "test rule".to_string(),
        }
    }

    /// U4 (R7 / D6): `exists:true` against a protected scope
    /// structural field on a scope topic is flagged with
    /// `FINDING_PAYLOAD_CONSISTENCY_SCOPE_POSITIVE_ASSERTION`. This is
    /// the original `red-team-attack.yml:215-294` anti-pattern.
    #[test]
    fn exists_true_on_protected_scope_structural_field_is_flagged() {
        let config = scope_topic_config(
            "redteam.plan.resolved",
            vec![rule_on(
                "redteam.plan.resolved",
                "redteam-scope-manifest-path-exists",
                json!({"field": "scope_manifest_path", "exists": true}),
            )],
        );
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        let polarity: Vec<_> = findings
            .iter()
            .filter(|f| {
                f.id == FINDING_PAYLOAD_CONSISTENCY_SCOPE_POSITIVE_ASSERTION
            })
            .collect();
        assert_eq!(polarity.len(), 1, "got {findings:?}");
        assert_eq!(polarity[0].severity, LintSeverity::Error);
        assert!(polarity[0].message.contains("scope_manifest_path"));
        assert!(polarity[0].message.contains("exists"));
    }

    /// U4: `non_empty:true` against a protected scope structural
    /// field is also a positive structural assertion (the original
    /// merge-batch inverted rule). Must be flagged.
    #[test]
    fn non_empty_true_on_protected_scope_structural_field_is_flagged() {
        let config = scope_topic_config(
            "merge.integrated",
            vec![rule_on(
                "merge.integrated",
                "merge-batch-boundary-path-root",
                json!({"field": "merge_boundary_path", "non_empty": true}),
            )],
        );
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        let polarity: Vec<_> = findings
            .iter()
            .filter(|f| {
                f.id == FINDING_PAYLOAD_CONSISTENCY_SCOPE_POSITIVE_ASSERTION
            })
            .collect();
        assert_eq!(polarity.len(), 1, "got {findings:?}");
        assert!(polarity[0].message.contains("merge_boundary_path"));
        assert!(polarity[0].message.contains("non_empty"));
    }

    /// U4: `gt: <positive>` against a protected scope threshold
    /// field on a scope topic is a positive assertion on the legal
    /// value (the original `redteam-scope-resolved-confidence` rule
    /// `gt: 89` against legal 90/100).
    #[test]
    fn gt_positive_on_protected_scope_threshold_field_is_flagged() {
        let config = scope_topic_config(
            "redteam.plan.resolved",
            vec![rule_on(
                "redteam.plan.resolved",
                "redteam-scope-resolved-confidence",
                json!({"field": "overall_confidence", "gt": 89}),
            )],
        );
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        let polarity: Vec<_> = findings
            .iter()
            .filter(|f| {
                f.id == FINDING_PAYLOAD_CONSISTENCY_SCOPE_POSITIVE_ASSERTION
            })
            .collect();
        assert_eq!(polarity.len(), 1, "got {findings:?}");
        assert!(polarity[0].message.contains("overall_confidence"));
        assert!(polarity[0].message.contains("gt"));
    }

    /// U4: positive assertions nested inside an `all:` combinator on a
    /// scope topic are still flagged (recursion through combinators,
    /// mirroring the existing `unknown_field_nested_in_any_fails`
    /// test).
    #[test]
    fn positive_assertion_nested_in_all_is_flagged() {
        let config = scope_topic_config(
            "redteam.plan.resolved",
            vec![rule_on(
                "redteam.plan.resolved",
                "redteam-scope-coverage-nested",
                json!({"all": [
                    {"field": "scope_status", "eq": "resolved"},
                    {"field": "coverage", "gt": 89}
                ]}),
            )],
        );
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        let polarity: Vec<_> = findings
            .iter()
            .filter(|f| {
                f.id == FINDING_PAYLOAD_CONSISTENCY_SCOPE_POSITIVE_ASSERTION
            })
            .collect();
        assert_eq!(polarity.len(), 1, "got {findings:?}");
        assert!(polarity[0].message.contains("coverage"));
    }

    /// U4: the legitimate negative contradiction shape
    /// (`gt: 0` on `critical_unknown_count` paired with
    /// `scope_status == "resolved"`) does NOT trigger the polarity
    /// finding. This is the legal same-payload rule shape used by the
    /// U3-corrected `redteam-scope-resolved-no-critical-unknown` rule.
    #[test]
    fn valid_negative_threshold_rule_passes_polarity_check() {
        let config = scope_topic_config(
            "redteam.plan.resolved",
            vec![rule_on(
                "redteam.plan.resolved",
                "redteam-scope-resolved-no-critical-unknown",
                json!({"all": [
                    {"field": "scope_status", "eq": "resolved"},
                    {"field": "critical_unknown_count", "gt": 0}
                ]}),
            )],
        );
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        let polarity: Vec<_> = findings
            .iter()
            .filter(|f| {
                f.id == FINDING_PAYLOAD_CONSISTENCY_SCOPE_POSITIVE_ASSERTION
            })
            .collect();
        assert!(
            polarity.is_empty(),
            "negative threshold rule must not trigger polarity finding, got {findings:?}"
        );
    }

    #[test]
    fn positive_critical_unknown_threshold_remains_negative_contradiction() {
        let config = scope_topic_config(
            "redteam.plan.resolved",
            vec![rule_on(
                "redteam.plan.resolved",
                "redteam-scope-critical-unknown-limit",
                json!({"all": [
                    {"field": "scope_status", "eq": "resolved"},
                    {"field": "critical_unknown_count", "gt": 1}
                ]}),
            )],
        );
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        assert!(
            findings
                .iter()
                .all(|finding| finding.id
                    != FINDING_PAYLOAD_CONSISTENCY_SCOPE_POSITIVE_ASSERTION),
            "critical_unknown_count > 1 is an invalid-state contradiction, got {findings:?}"
        );
    }

    /// U4: `eq: 0` against a protected threshold field is the legal
    /// `resolved_count` contradiction form (resolved requires
    /// `resolved_count >= 1`). Must NOT trigger polarity finding.
    #[test]
    fn eq_zero_on_resolved_count_does_not_trigger_polarity() {
        let config = scope_topic_config(
            "redteam.plan.resolved",
            vec![rule_on(
                "redteam.plan.resolved",
                "redteam-scope-resolved-count",
                json!({"all": [
                    {"field": "scope_status", "eq": "resolved"},
                    {"field": "resolved_count", "eq": 0}
                ]}),
            )],
        );
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        let polarity: Vec<_> = findings
            .iter()
            .filter(|f| {
                f.id == FINDING_PAYLOAD_CONSISTENCY_SCOPE_POSITIVE_ASSERTION
            })
            .collect();
        assert!(
            polarity.is_empty(),
            "`eq: 0` on `resolved_count` is the legal contradiction shape, got {findings:?}"
        );
    }

    /// U4: `eq: "blocked"` on a protected structural field is the
    /// legal contradiction form for the postmerge not-resolved rule
    /// (postmerge requires `scope_status != "blocked"` paired with
    /// `proceed: true`). Must NOT trigger polarity finding.
    #[test]
    fn eq_blocked_on_scope_status_does_not_trigger_polarity() {
        let config = scope_topic_config(
            "postmerge.changemap.ready",
            vec![rule_on(
                "postmerge.changemap.ready",
                "postmerge-scope-not-resolved-proceed-false",
                json!({"any": [
                    {"all": [
                        {"field": "scope_status", "eq": "blocked"},
                        {"field": "proceed", "eq": true}
                    ]},
                    {"all": [
                        {"field": "scope_status", "eq": "ambiguous"},
                        {"field": "proceed", "eq": true}
                    ]}
                ]}),
            )],
        );
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        let polarity: Vec<_> = findings
            .iter()
            .filter(|f| {
                f.id == FINDING_PAYLOAD_CONSISTENCY_SCOPE_POSITIVE_ASSERTION
            })
            .collect();
        assert!(
            polarity.is_empty(),
            "`eq: blocked/ambiguous` is the legal contradiction shape, got {findings:?}"
        );
    }

    /// U4: `ne: "redteam.plan.resolved"` on `predecessor_event` is the
    /// legal negative contradiction form (legal literal Misses; wrong
    /// literal Hits). Must NOT trigger polarity finding.
    #[test]
    fn ne_legal_literal_on_predecessor_does_not_trigger_polarity() {
        let config = scope_topic_config(
            "redteam.attack.mapped",
            vec![rule_on(
                "redteam.attack.mapped",
                "redteam-attack-mapped-predecessor-must-be-resolved",
                json!({"field": "predecessor_event", "ne": "redteam.plan.resolved"}),
            )],
        );
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        let polarity: Vec<_> = findings
            .iter()
            .filter(|f| {
                f.id == FINDING_PAYLOAD_CONSISTENCY_SCOPE_POSITIVE_ASSERTION
            })
            .collect();
        assert!(
            polarity.is_empty(),
            "`ne:` legal literal on `predecessor_event` must not trigger polarity, got {findings:?}"
        );
    }

    /// U4: pipeline rules (non-scope topic `fix.done` /
    /// `work.done`) keep their legal same-payload contradiction shape
    /// and never trigger the polarity finding. This is the explicit
    /// R3 / D6 isolation: `ce-executor-pipeline` and
    /// `ce-executor-pipeline-loop` are NOT in `PROTECTED_SCOPE_TOPICS`.
    #[test]
    fn pipeline_fix_done_rule_does_not_trigger_polarity() {
        let config = scope_topic_config(
            "fix.done",
            vec![valid_rule()],
        );
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        let polarity: Vec<_> = findings
            .iter()
            .filter(|f| {
                f.id == FINDING_PAYLOAD_CONSISTENCY_SCOPE_POSITIVE_ASSERTION
            })
            .collect();
        assert!(
            polarity.is_empty(),
            "ce-executor-pipeline fix.done rule must not trigger polarity, got {findings:?}"
        );
    }

    /// U4: severity is `Warn` in default mode, `Error` in strict,
    /// matching the rest of the `payload_consistency_*` family.
    #[test]
    fn polarity_finding_severity_follows_strictness() {
        let config = scope_topic_config(
            "redteam.plan.resolved",
            vec![rule_on(
                "redteam.plan.resolved",
                "r1",
                json!({"field": "scope_manifest_path", "exists": true}),
            )],
        );
        let default_findings =
            check_payload_consistency(&config, LintStrictness::Default);
        let strict_findings =
            check_payload_consistency(&config, LintStrictness::Strict);
        let default_polarity = default_findings
            .iter()
            .find(|f| {
                f.id == FINDING_PAYLOAD_CONSISTENCY_SCOPE_POSITIVE_ASSERTION
            })
            .expect("Default mode should still surface the polarity finding");
        assert_eq!(default_polarity.severity, LintSeverity::Warn);
        let strict_polarity = strict_findings
            .iter()
            .find(|f| {
                f.id == FINDING_PAYLOAD_CONSISTENCY_SCOPE_POSITIVE_ASSERTION
            })
            .expect("Strict mode should surface the polarity finding");
        assert_eq!(strict_polarity.severity, LintSeverity::Error);
    }

    /// U4: the polarity finding's diagnostic message names the
    /// `rule_id`, `topic`, `field`, and `op` so author/reviewer
    /// tooling can localize the offending predicate without parsing
    /// the message text.
    #[test]
    fn polarity_finding_message_identifies_predicate() {
        let config = scope_topic_config(
            "redteam.plan.resolved",
            vec![rule_on(
                "redteam.plan.resolved",
                "demo-rule",
                json!({"field": "patch_digest", "exists": true}),
            )],
        );
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        let polarity = findings
            .iter()
            .find(|f| {
                f.id == FINDING_PAYLOAD_CONSISTENCY_SCOPE_POSITIVE_ASSERTION
            })
            .expect("polarity finding should fire");
        assert!(polarity.message.contains("demo-rule"));
        assert!(polarity.message.contains("redteam.plan.resolved"));
        assert!(polarity.message.contains("patch_digest"));
        assert!(polarity.message.contains("exists"));
    }

    /// U3 (2026-08-10-002 plan, R7/C2): polarity fires even when the
    /// rule's topic is missing from `event_policy.schemas`. The
    /// previous ordering short-circuited on `unknown_topic` BEFORE
    /// the polarity walker ran, so a rule with a typo'd topic on a
    /// protected scope field saw its polarity finding silently
    /// skipped. After U3 the polarity walker is structural and
    /// independent of schema presence.
    #[test]
    fn polarity_fires_when_topic_missing_from_schemas() {
        // Build a config with NO `event_policy.schemas` entries, so
        // every scope topic is "unknown" from the field-known
        // helper's perspective.
        let mut policy = EventPolicyConfig::default();
        policy.payload_consistency = PayloadConsistencyConfig {
            enabled: true,
            rules: vec![rule_on(
                "redteam.plan.resolved",
                "c2-regression",
                json!({"field": "scope_manifest_path", "exists": true}),
            )],
        };
        let mut config = RalphConfig::default();
        config.event_loop.event_policy = Some(policy);
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        let polarity = findings
            .iter()
            .find(|f| {
                f.id == FINDING_PAYLOAD_CONSISTENCY_SCOPE_POSITIVE_ASSERTION
            })
            .expect("polarity finding must fire even without schema");
        assert_eq!(polarity.message.contains("scope_manifest_path"), true);
        let unknown_topic = findings
            .iter()
            .find(|f| f.id == FINDING_PAYLOAD_CONSISTENCY_UNKNOWN_TOPIC);
        assert!(
            unknown_topic.is_some(),
            "unknown_topic finding should still surface alongside polarity"
        );
    }

    /// U3: non-scope topics (e.g. `fix.done`) never trigger the
    /// polarity walker regardless of the rule's `when`. This guards
    /// against a regression where `check_scope_polarity` was hoisted
    /// above the topic allowlist check.
    #[test]
    fn polarity_silently_skipped_for_non_scope_topics() {
        let config = scope_topic_config(
            "fix.done",
            vec![rule_on(
                "fix.done",
                "non-scope-rule",
                json!({"field": "fixes_applied", "exists": true}),
            )],
        );
        let findings = check_payload_consistency(&config, LintStrictness::Strict);
        let polarity = findings
            .iter()
            .find(|f| f.id == FINDING_PAYLOAD_CONSISTENCY_SCOPE_POSITIVE_ASSERTION);
        assert!(
            polarity.is_none(),
            "polarity finding must not fire on non-scope topics"
        );
    }
}
