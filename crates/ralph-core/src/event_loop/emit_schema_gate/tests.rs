use super::{EmitDecision, check};
use serde_json::json;

fn req(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn emit_schema_gate_accepts_when_all_required_present() {
    let payload = json!({"reason": "unit_failed", "extra": 1});
    let required = req(&["reason"]);
    assert_eq!(check(&payload, &required), EmitDecision::Accept);
}

#[test]
fn emit_schema_gate_accepts_with_empty_required_list() {
    let payload = json!({"anything": 1});
    let required = req(&[]);
    assert_eq!(check(&payload, &required), EmitDecision::Accept);

    // also on the empty payload
    let empty = json!({});
    assert_eq!(check(&empty, &required), EmitDecision::Accept);
}

#[test]
fn emit_schema_gate_rejects_non_object_payload() {
    let payload = json!("a string");
    let required = req(&["reason"]);
    let decision = check(&payload, &required);
    let EmitDecision::Reject(missing) = decision else {
        panic!("expected Reject");
    };
    assert_eq!(missing, vec!["__payload_must_be_object".to_string()]);
}

#[test]
fn emit_schema_gate_rejects_when_one_required_field_missing() {
    let payload = json!({"other": 1});
    let required = req(&["reason"]);
    let decision = check(&payload, &required);
    let EmitDecision::Reject(missing) = decision else {
        panic!("expected Reject");
    };
    assert_eq!(missing, vec!["reason".to_string()]);
}

#[test]
fn emit_schema_gate_rejects_when_multiple_required_fields_missing() {
    let payload = json!({});
    let required = req(&["reason", "kind", "target_hat"]);
    let decision = check(&payload, &required);
    let EmitDecision::Reject(missing) = decision else {
        panic!("expected Reject");
    };
    assert_eq!(
        missing,
        vec![
            "reason".to_string(),
            "kind".to_string(),
            "target_hat".to_string()
        ]
    );
}

#[test]
fn emit_schema_gate_treats_null_as_missing() {
    // `reason` is present but explicitly null — treated as missing to
    // match the 2026-06-26 drift engine behaviour. Without this rule
    // `plan.blocked(reason=null)` would still be accepted as
    // semantically empty, which is exactly the bug U1 is closing.
    let payload = json!({"reason": null});
    let required = req(&["reason"]);
    let decision = check(&payload, &required);
    let EmitDecision::Reject(missing) = decision else {
        panic!("expected Reject");
    };
    assert_eq!(missing, vec!["reason".to_string()]);
}

#[test]
fn emit_schema_gate_distinguishes_null_from_empty_string() {
    // An empty string is still a value; only `null` is treated as
    // missing. The actual "empty reason" check happens at the
    // U9 FlowStepScope level, not here.
    let payload = json!({"reason": ""});
    let required = req(&["reason"]);
    assert_eq!(check(&payload, &required), EmitDecision::Accept);
}
