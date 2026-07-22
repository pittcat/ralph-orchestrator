//! Pure same-payload predicate evaluator for `payload_consistency` rules.
//!
//! This module is deliberately **side-effect free**: it takes a parsed
//! `when` predicate (a [`serde_json::Value`] shaped like a single predicate
//! `{field, op, value}` or a combinator `{all: [...]}` / `{any: [...]}`)
//! and a single event payload, and returns whether the rule fired
//! ([`EvalOutcome::Hit`]) or not ([`EvalOutcome::Miss`]).
//!
//! It does **not** depend on the event bus, ledger, or any state — U3
//! is the only consumer that wires the outcome into `validate_event_with_options`
//! and turns a Hit into a `SemanticGateViolation`. This module is the
//! pure core; U3 is the policy wiring.
//!
//! # Whitelisted predicate set (R3, KTD2)
//!
//! Comparison: `eq`, `ne`, `gt`, `gte`. Existence: `exists`, `non_empty`.
//! Combinators: `all`, `any`. No scriptable expressions, no `lt`/`lte`
//! (plan §R3 closure). Adding a new op requires revisiting the
//! `WHITELISTED_PREDICATE_OPS` table below and updating the test matrix.
//!
//! # Type-mismatch policy (fail-close, executor-adopted)
//!
//! When a comparison predicate is asked to compare values whose JSON
//! runtime types disagree (e.g. `eq: 5` against a string field
//! `"hello"`, or `gt: 3` against a string field), the evaluator MUST
//! return [`EvalOutcome::Hit`] rather than `Miss`. The rationale:
//! a rule with a type mismatch is **structurally broken** — the rule
//! author intended one type and got another. Silently passing such a
//! rule is worse than rejecting it: the rejection forces the rule
//! author to fix the predicate so it matches the schema, instead of
//! letting a broken rule lie about payload consistency. This is the
//! inverse of the "rule author is infallible" assumption, which we do
//! not make.
//!
//! The same fail-close rule applies to:
//!
//! - Unknown predicate op (e.g. `gtz`) → `Hit`.
//! - Predicate missing the `field` key → `Hit` (the rule is malformed;
//!   the author must fix it; otherwise every event would silently miss
//!   and the rule would never fire, which is worse than firing once
//!   and surfacing the broken shape via `SemanticGateViolation`).
//! - Predicate shape is not a JSON object → `Hit`.
//! - Empty `when: {}` (no combinator, no predicate) → `Miss` (no
//!   condition means the rule doesn't fire; this is a **vacuous** but
//!   well-formed `when`, distinct from a malformed `when`).
//!
//! # Field lookup
//!
//! The `field` path uses dot notation identical to
//! `event_policy::extract_json_field`: split on `.`, descend into
//! `Value::Object` keys, return `None` on any miss or non-object
//! intermediate. Re-implemented locally rather than imported so this
//! module stays self-contained and the visibility of the private
//! helper in `event_policy.rs` does not have to change.

use serde_json::Value;

/// Result of evaluating a `when` predicate against a single payload.
///
/// `Hit` means the rule fired (and U3 will surface a
/// `SemanticGateViolation` if the rule's topic matched). `Miss` means
/// the rule did not fire for this payload. The fail-close defaults
/// documented at module level mean that a malformed rule yields
/// `Hit`, not `Miss` — U3 is responsible for turning that into an
/// actionable error message for the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalOutcome {
    Hit,
    Miss,
}

/// Whitelisted predicate op names. Anything else triggers fail-close.
///
/// Shared with `crate::preset_lint::payload_consistency` so lint and
/// runtime stay in lockstep on the whitelist. Adding a new op here
/// must revisit both call sites and the lint test matrix.
pub(crate) const WHITELISTED_PREDICATE_OPS: &[&str] = &["eq", "ne", "gt", "gte", "exists", "non_empty"];

/// Evaluate a parsed `when` against a single payload.
///
/// `rule_when` is the value of `PayloadConsistencyRule.when` as parsed
/// from YAML; `payload` is the event payload (already extracted from
/// the line and parsed as `serde_json::Value`). The function is total:
/// it never panics and never returns an error. Failures of any kind
/// degrade to [`EvalOutcome::Hit`] (see module-level docs for why).
pub fn evaluate(rule_when: &Value, payload: &Value) -> EvalOutcome {
    match rule_when {
        // Combinators: { "all": [...] } / { "any": [...] }
        Value::Object(obj) if obj.contains_key("all") => {
            eval_combinator(obj.get("all"), Combinator::All, payload)
        }
        Value::Object(obj) if obj.contains_key("any") => {
            eval_combinator(obj.get("any"), Combinator::Any, payload)
        }
        // Empty object: vacuous well-formed `when` → Miss (rule doesn't fire).
        // Distinct from a malformed `when` (non-object, or object with
        // neither a combinator key nor a known op) → fail-close Hit.
        Value::Object(obj) if obj.is_empty() => EvalOutcome::Miss,
        // Single predicate: { "field": "...", "eq": ..., ... }
        Value::Object(_) => eval_predicate(rule_when, payload),
        // Non-object when (array, scalar, null) → fail-close Hit.
        // An array shape is technically a degenerate combinator with
        // no key; a scalar/null is malformed. Either way, the rule
        // author must fix it.
        _ => EvalOutcome::Hit,
    }
}

#[derive(Debug, Clone, Copy)]
enum Combinator {
    All,
    Any,
}

fn eval_combinator(child: Option<&Value>, kind: Combinator, payload: &Value) -> EvalOutcome {
    let arr = match child {
        Some(Value::Array(items)) => items,
        // Combinator key with non-array value → fail-close Hit.
        _ => return EvalOutcome::Hit,
    };
    if arr.is_empty() {
        // Empty combinator is vacuous. all() over empty → Miss
        // (universal quantifier vacuously true? No — we treat
        // "no conditions means rule doesn't fire" for both, matching
        // the empty-when Miss policy).
        // Per module docs: empty `when` → Miss. Combinator with no
        // children has the same semantics — the rule doesn't fire.
        return EvalOutcome::Miss;
    }
    for item in arr {
        let outcome = evaluate(item, payload);
        match (kind, outcome) {
            (Combinator::All, EvalOutcome::Miss) => return EvalOutcome::Miss,
            (Combinator::Any, EvalOutcome::Hit) => return EvalOutcome::Hit,
            _ => {}
        }
    }
    match kind {
        Combinator::All => EvalOutcome::Hit,
        Combinator::Any => EvalOutcome::Miss,
    }
}

fn eval_predicate(pred: &Value, payload: &Value) -> EvalOutcome {
    let obj = match pred {
        Value::Object(obj) => obj,
        // Not an object (single scalar/array predicate) → fail-close Hit.
        _ => return EvalOutcome::Hit,
    };

    // Detect the op. If multiple ops appear, take the first known one
    // and ignore the rest; if none are known, fail-close Hit.
    let (op_name, op_value) = match first_op(obj) {
        Some((name, value)) => (name, value),
        None => return EvalOutcome::Hit,
    };

    // `field` is required for any predicate.
    let field = match obj.get("field").and_then(Value::as_str) {
        Some(f) => f,
        None => return EvalOutcome::Hit,
    };

    match op_name {
        "exists" => {
            // Per plan §U2: null counts as present.
            if extract_json_field(payload, field).is_some() {
                EvalOutcome::Hit
            } else {
                EvalOutcome::Miss
            }
        }
        "non_empty" => match extract_json_field(payload, field) {
            Some(Value::Null) => EvalOutcome::Miss,
            Some(Value::String(s)) => {
                if s.is_empty() {
                    EvalOutcome::Miss
                } else {
                    EvalOutcome::Hit
                }
            }
            Some(Value::Array(a)) => {
                if a.is_empty() {
                    EvalOutcome::Miss
                } else {
                    EvalOutcome::Hit
                }
            }
            Some(Value::Object(o)) => {
                if o.is_empty() {
                    EvalOutcome::Miss
                } else {
                    EvalOutcome::Hit
                }
            }
            // Numbers and bools are always "present and non-empty".
            Some(_) => EvalOutcome::Hit,
            None => EvalOutcome::Miss,
        },
        "eq" | "ne" => eval_eq_ne(op_name, op_value, payload, field),
        "gt" | "gte" => eval_gt_gte(op_name, op_value, payload, field),
        // Defensive: unknown op (should be caught by `first_op`).
        _ => EvalOutcome::Hit,
    }
}

fn first_op(obj: &serde_json::Map<String, Value>) -> Option<(&'static str, &Value)> {
    for (k, v) in obj {
        if WHITELISTED_PREDICATE_OPS.contains(&k.as_str()) {
            // Map &String to &'static str via the whitelist — the
            // comparison above guarantees the bytes match.
            return Some((
                match k.as_str() {
                    "eq" => "eq",
                    "ne" => "ne",
                    "gt" => "gt",
                    "gte" => "gte",
                    "exists" => "exists",
                    "non_empty" => "non_empty",
                    _ => unreachable!("whitelist guard above"),
                },
                v,
            ));
        }
    }
    None
}

fn eval_eq_ne(op: &str, expected: &Value, payload: &Value, field: &str) -> EvalOutcome {
    let actual = match extract_json_field(payload, field) {
        Some(v) => v,
        None => return EvalOutcome::Miss,
    };
    // Fail-close on type mismatch (see module docs).
    if json_type_tag(&actual) != json_type_tag(expected) {
        return EvalOutcome::Hit;
    }
    let equal = actual == *expected;
    match op {
        "eq" => {
            if equal {
                EvalOutcome::Hit
            } else {
                EvalOutcome::Miss
            }
        }
        "ne" => {
            if equal {
                EvalOutcome::Miss
            } else {
                EvalOutcome::Hit
            }
        }
        _ => EvalOutcome::Hit,
    }
}

fn eval_gt_gte(op: &str, threshold: &Value, payload: &Value, field: &str) -> EvalOutcome {
    let actual = match extract_json_field(payload, field) {
        Some(v) => v,
        None => return EvalOutcome::Miss,
    };
    // Fail-close on non-numeric comparison.
    let (a, t) = match (actual.as_f64(), threshold.as_f64()) {
        (Some(a), Some(t)) => (a, t),
        _ => return EvalOutcome::Hit,
    };
    let fired = match op {
        "gt" => a > t,
        "gte" => a >= t,
        _ => return EvalOutcome::Hit,
    };
    if fired {
        EvalOutcome::Hit
    } else {
        EvalOutcome::Miss
    }
}

/// Stable type tag used by fail-close type-mismatch checks.
/// Numbers collapse to "number" so `eq: 5` against `5.0` is NOT a
/// mismatch (matches `serde_json::Value::PartialEq`, which treats
/// `5 == 5.0` as true anyway).
fn json_type_tag(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Local copy of `event_policy::extract_json_field` (kept identical
/// so U3 can sanity-check the contract stays the same). Public
/// visibility is not changed on the upstream helper.
fn extract_json_field(value: &Value, path: &str) -> Option<Value> {
    let mut current = value;
    for part in path.split('.') {
        match current {
            Value::Object(obj) => {
                current = obj.get(part)?;
            }
            _ => return None,
        }
    }
    Some(current.clone())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- eq ----------------------------------------------------------------

    #[test]
    fn eq_equal_string_is_hit() {
        let when = json!({"field": "x", "eq": "hello"});
        assert_eq!(evaluate(&when, &json!({"x": "hello"})), EvalOutcome::Hit);
    }

    #[test]
    fn eq_equal_number_is_hit() {
        let when = json!({"field": "n", "eq": 5});
        assert_eq!(evaluate(&when, &json!({"n": 5})), EvalOutcome::Hit);
    }

    #[test]
    fn eq_equal_bool_is_hit() {
        let when = json!({"field": "b", "eq": true});
        assert_eq!(evaluate(&when, &json!({"b": true})), EvalOutcome::Hit);
    }

    #[test]
    fn eq_unequal_string_is_miss() {
        let when = json!({"field": "x", "eq": "hello"});
        assert_eq!(evaluate(&when, &json!({"x": "world"})), EvalOutcome::Miss);
    }

    #[test]
    fn eq_type_mismatch_is_hit_fail_close() {
        let when = json!({"field": "x", "eq": 5});
        assert_eq!(evaluate(&when, &json!({"x": "hello"})), EvalOutcome::Hit);
    }

    #[test]
    fn eq_missing_field_is_miss() {
        let when = json!({"field": "x", "eq": "hello"});
        assert_eq!(evaluate(&when, &json!({})), EvalOutcome::Miss);
    }

    // -- ne ----------------------------------------------------------------

    #[test]
    fn ne_unequal_is_hit() {
        let when = json!({"field": "x", "ne": "hello"});
        assert_eq!(evaluate(&when, &json!({"x": "world"})), EvalOutcome::Hit);
    }

    #[test]
    fn ne_equal_is_miss() {
        let when = json!({"field": "x", "ne": "hello"});
        assert_eq!(evaluate(&when, &json!({"x": "hello"})), EvalOutcome::Miss);
    }

    #[test]
    fn ne_type_mismatch_is_hit_fail_close() {
        let when = json!({"field": "x", "ne": 5});
        assert_eq!(evaluate(&when, &json!({"x": "hello"})), EvalOutcome::Hit);
    }

    // -- gt / gte ----------------------------------------------------------

    #[test]
    fn gt_strictly_greater_is_hit() {
        let when = json!({"field": "n", "gt": 5});
        assert_eq!(evaluate(&when, &json!({"n": 6})), EvalOutcome::Hit);
    }

    #[test]
    fn gt_equal_is_miss() {
        let when = json!({"field": "n", "gt": 5});
        assert_eq!(evaluate(&when, &json!({"n": 5})), EvalOutcome::Miss);
    }

    #[test]
    fn gt_less_is_miss() {
        let when = json!({"field": "n", "gt": 5});
        assert_eq!(evaluate(&when, &json!({"n": 4})), EvalOutcome::Miss);
    }

    #[test]
    fn gt_against_string_is_hit_fail_close() {
        let when = json!({"field": "n", "gt": 5});
        assert_eq!(evaluate(&when, &json!({"n": "hello"})), EvalOutcome::Hit);
    }

    #[test]
    fn gt_against_bool_is_hit_fail_close() {
        let when = json!({"field": "n", "gt": 5});
        assert_eq!(evaluate(&when, &json!({"n": true})), EvalOutcome::Hit);
    }

    #[test]
    fn gt_threshold_non_number_is_hit_fail_close() {
        let when = json!({"field": "n", "gt": "five"});
        assert_eq!(evaluate(&when, &json!({"n": 3})), EvalOutcome::Hit);
    }

    #[test]
    fn gte_strictly_greater_is_hit() {
        let when = json!({"field": "n", "gte": 5});
        assert_eq!(evaluate(&when, &json!({"n": 6})), EvalOutcome::Hit);
    }

    #[test]
    fn gte_equal_is_hit() {
        let when = json!({"field": "n", "gte": 5});
        assert_eq!(evaluate(&when, &json!({"n": 5})), EvalOutcome::Hit);
    }

    #[test]
    fn gte_less_is_miss() {
        let when = json!({"field": "n", "gte": 5});
        assert_eq!(evaluate(&when, &json!({"n": 4})), EvalOutcome::Miss);
    }

    #[test]
    fn gte_against_string_is_hit_fail_close() {
        let when = json!({"field": "n", "gte": 5});
        assert_eq!(evaluate(&when, &json!({"n": "hello"})), EvalOutcome::Hit);
    }

    #[test]
    fn gt_missing_field_is_miss() {
        let when = json!({"field": "n", "gt": 5});
        assert_eq!(evaluate(&when, &json!({})), EvalOutcome::Miss);
    }

    // -- exists ------------------------------------------------------------

    #[test]
    fn exists_present_value_is_hit() {
        let when = json!({"field": "x", "exists": true});
        assert_eq!(evaluate(&when, &json!({"x": "hello"})), EvalOutcome::Hit);
    }

    #[test]
    fn exists_null_counts_as_present_is_hit() {
        // Per plan §U2: null is "present".
        let when = json!({"field": "x", "exists": true});
        assert_eq!(evaluate(&when, &json!({"x": null})), EvalOutcome::Hit);
    }

    #[test]
    fn exists_missing_field_is_miss() {
        let when = json!({"field": "x", "exists": true});
        assert_eq!(evaluate(&when, &json!({})), EvalOutcome::Miss);
    }

    #[test]
    fn exists_nested_present_is_hit() {
        let when = json!({"field": "a.b", "exists": true});
        assert_eq!(evaluate(&when, &json!({"a": {"b": 1}})), EvalOutcome::Hit);
    }

    // -- non_empty ---------------------------------------------------------

    #[test]
    fn non_empty_nonempty_array_is_hit() {
        let when = json!({"field": "a", "non_empty": true});
        assert_eq!(evaluate(&when, &json!({"a": [1]})), EvalOutcome::Hit);
    }

    #[test]
    fn non_empty_nonempty_string_is_hit() {
        let when = json!({"field": "s", "non_empty": true});
        assert_eq!(evaluate(&when, &json!({"s": "hi"})), EvalOutcome::Hit);
    }

    #[test]
    fn non_empty_nonempty_object_is_hit() {
        let when = json!({"field": "o", "non_empty": true});
        assert_eq!(evaluate(&when, &json!({"o": {"k": 1}})), EvalOutcome::Hit);
    }

    #[test]
    fn non_empty_empty_array_is_miss() {
        let when = json!({"field": "a", "non_empty": true});
        assert_eq!(evaluate(&when, &json!({"a": []})), EvalOutcome::Miss);
    }

    #[test]
    fn non_empty_empty_string_is_miss() {
        let when = json!({"field": "s", "non_empty": true});
        assert_eq!(evaluate(&when, &json!({"s": ""})), EvalOutcome::Miss);
    }

    #[test]
    fn non_empty_empty_object_is_miss() {
        let when = json!({"field": "o", "non_empty": true});
        assert_eq!(evaluate(&when, &json!({"o": {}})), EvalOutcome::Miss);
    }

    #[test]
    fn non_empty_null_is_miss() {
        let when = json!({"field": "x", "non_empty": true});
        assert_eq!(evaluate(&when, &json!({"x": null})), EvalOutcome::Miss);
    }

    #[test]
    fn non_empty_missing_is_miss() {
        let when = json!({"field": "x", "non_empty": true});
        assert_eq!(evaluate(&when, &json!({})), EvalOutcome::Miss);
    }

    #[test]
    fn non_empty_number_is_hit() {
        // Numbers and bools are not "empty" in any meaningful sense.
        let when = json!({"field": "n", "non_empty": true});
        assert_eq!(evaluate(&when, &json!({"n": 0})), EvalOutcome::Hit);
    }

    // -- combinators -------------------------------------------------------

    #[test]
    fn all_with_one_miss_is_miss() {
        let when = json!({"all": [
            {"field": "x", "eq": "hello"},
            {"field": "y", "eq": "world"},
        ]});
        assert_eq!(
            evaluate(&when, &json!({"x": "hello", "y": "nope"})),
            EvalOutcome::Miss
        );
    }

    #[test]
    fn all_with_all_hits_is_hit() {
        let when = json!({"all": [
            {"field": "x", "eq": "hello"},
            {"field": "y", "eq": "world"},
        ]});
        assert_eq!(
            evaluate(&when, &json!({"x": "hello", "y": "world"})),
            EvalOutcome::Hit
        );
    }

    #[test]
    fn any_with_one_hit_is_hit() {
        let when = json!({"any": [
            {"field": "x", "eq": "hello"},
            {"field": "y", "eq": "world"},
        ]});
        assert_eq!(
            evaluate(&when, &json!({"x": "nope", "y": "world"})),
            EvalOutcome::Hit
        );
    }

    #[test]
    fn any_with_all_misses_is_miss() {
        let when = json!({"any": [
            {"field": "x", "eq": "hello"},
            {"field": "y", "eq": "world"},
        ]});
        assert_eq!(
            evaluate(&when, &json!({"x": "nope", "y": "no"})),
            EvalOutcome::Miss
        );
    }

    #[test]
    fn nested_all_inside_any() {
        // (x == "hello" AND y == 1) OR z == "fallback"
        let when = json!({"any": [
            {"all": [
                {"field": "x", "eq": "hello"},
                {"field": "y", "eq": 1},
            ]},
            {"field": "z", "eq": "fallback"},
        ]});
        assert_eq!(
            evaluate(&when, &json!({"x": "hello", "y": 1})),
            EvalOutcome::Hit
        );
        assert_eq!(
            evaluate(&when, &json!({"x": "hello", "y": 2})),
            EvalOutcome::Miss
        );
        assert_eq!(evaluate(&when, &json!({"z": "fallback"})), EvalOutcome::Hit);
    }

    #[test]
    fn empty_combinator_is_miss() {
        let when_all = json!({"all": []});
        let when_any = json!({"any": []});
        assert_eq!(evaluate(&when_all, &json!({})), EvalOutcome::Miss);
        assert_eq!(evaluate(&when_any, &json!({})), EvalOutcome::Miss);
    }

    // -- malformed `when` shapes (fail-close Hit) -------------------------

    #[test]
    fn unknown_op_is_hit_fail_close() {
        let when = json!({"field": "x", "gtz": 5});
        assert_eq!(evaluate(&when, &json!({"x": 5})), EvalOutcome::Hit);
    }

    #[test]
    fn missing_field_key_is_hit_fail_close() {
        let when = json!({"eq": "hello"});
        assert_eq!(evaluate(&when, &json!({"x": "hello"})), EvalOutcome::Hit);
    }

    #[test]
    fn scalar_when_is_hit_fail_close() {
        let when = json!("just a string");
        assert_eq!(evaluate(&when, &json!({})), EvalOutcome::Hit);
    }

    #[test]
    fn array_when_is_hit_fail_close() {
        let when = json!([{"field": "x", "eq": 1}]);
        assert_eq!(evaluate(&when, &json!({"x": 1})), EvalOutcome::Hit);
    }

    // -- empty `when` -----------------------------------------------------

    #[test]
    fn empty_when_is_miss() {
        // Vacuous when: no combinator, no predicate → rule doesn't fire.
        // Distinct from a malformed `when` (which fails close to Hit).
        let when = json!({});
        assert_eq!(evaluate(&when, &json!({})), EvalOutcome::Miss);
        assert_eq!(
            evaluate(&when, &json!({"anything": "goes"})),
            EvalOutcome::Miss
        );
    }

    // -- field lookup sanity (re-implementation contract) -----------------

    #[test]
    fn field_lookup_nested_dot_notation() {
        let payload = json!({"a": {"b": {"c": 42}}});
        let when = json!({"field": "a.b.c", "eq": 42});
        assert_eq!(evaluate(&when, &payload), EvalOutcome::Hit);
    }

    #[test]
    fn field_lookup_intermediate_non_object_returns_miss() {
        let payload = json!({"a": [1, 2, 3]});
        let when = json!({"field": "a.b", "exists": true});
        assert_eq!(evaluate(&when, &payload), EvalOutcome::Miss);
    }
}
