/// U2 (2026-07-23-002 plan, KTD2): structurally actionable
/// consistency feedback.
///
/// `ValidationError` must carry an independent `gate` field and a
/// `referenced_fields` list so agent repair tooling can locate the
/// offending payload fields without parsing `message`. The legacy
/// `field` slot must NOT carry the gate ID for `SemanticGateViolation`
/// — `field` is reserved for single-field schema violations.
use super::*;
use ralph_core::PolicyFinding;

fn consistency_finding_with_fields(rule_id: &str, fields: &[&str]) -> PolicyFinding {
    PolicyFinding {
        topic: "fix.done".to_string(),
        violation_type: ViolationType::SemanticGateViolation {
            gate: format!("payload_consistency:{rule_id}"),
            context: "contradiction between fix_status and fixes_applied".to_string(),
            referenced_fields: fields.iter().map(|s| s.to_string()).collect(),
        },
        message: format!("payload_consistency rule '{rule_id}' violated"),
        evidence: None,
    }
}

fn finding_with_evidence_unavailable() -> PolicyFinding {
    use ralph_core::correction::{EvidenceDetail, ObservationEntry, ObservationValue};
    PolicyFinding {
        topic: "fix.done".to_string(),
        violation_type: ViolationType::SemanticGateViolation {
            gate: "payload_consistency:rule-u3".to_string(),
            context: "null vs sentinel".to_string(),
            referenced_fields: vec!["status".to_string()],
        },
        message: "payload_consistency rule 'rule-u3' violated".to_string(),
        evidence: Some(EvidenceDetail {
            observed: vec![ObservationEntry {
                field: "status".to_string(),
                value: ObservationValue::Unavailable,
            }],
            invariant: String::new(),
            proof: String::new(),
            synthetic: false,
            guidance: None,
            failed_check_keys: None,
        }),
    }
}

#[test]
fn u3_observe_unavailable_projection_emits_json_null() {
    // AC3 Part B: ObservationValue::Unavailable must project to
    // serde_json::Value::Null (JSON null), NOT the string
    // "unavailable", so the CLI output contains a JSON null
    // rather than a string sentinel.
    let finding = finding_with_evidence_unavailable();
    let decision = PolicyDecision::RejectWithResume(finding);
    let err = finding_to_validation_error(&decision, "fix.done")
        .expect("RejectWithResume must surface as ValidationError");
    let observed = err.observed.expect("observed must be present");
    assert_eq!(observed.len(), 1);
    // The value field of the JSON object must be JSON null, not a string.
    let json_str = serde_json::to_string(&observed[0]).unwrap();
    // JSON null serialises as "null"; the string "unavailable" would appear as "\"unavailable\"".
    assert!(
        json_str.contains("null"),
        "Unavailable must project to JSON null, got: {json_str}"
    );
    assert!(
        !json_str.contains("unavailable"),
        "Unavailable must NOT be the string 'unavailable', got: {json_str}"
    );
}

#[test]
fn u2_reject_with_resume_carries_independent_gate() {
    // KTD2: gate is on its own field, not stuffed into `field`.
    let finding = consistency_finding_with_fields(
        "fix-done-blocked-zero-fixes-applied",
        &["fix_status", "fixes_applied"],
    );
    let decision = PolicyDecision::RejectWithResume(finding);
    let err = finding_to_validation_error(&decision, "fix.done")
        .expect("RejectWithResume must surface as ValidationError");
    assert_eq!(
        err.gate.as_deref(),
        Some("payload_consistency:fix-done-blocked-zero-fixes-applied"),
        "gate must be its own structured field, got {:?}",
        err.gate
    );
}

#[test]
fn u2_reject_with_resume_carries_referenced_fields() {
    // KTD2: referenced_fields is the static declared set, in
    // declaration order, deduplicated by first occurrence.
    let finding = consistency_finding_with_fields("rule-x", &["fix_status", "fixes_applied"]);
    let decision = PolicyDecision::RejectWithResume(finding);
    let err = finding_to_validation_error(&decision, "fix.done")
        .expect("RejectWithResume must surface as ValidationError");
    assert_eq!(
        err.referenced_fields,
        Some(vec!["fix_status".to_string(), "fixes_applied".to_string(),]),
        "referenced_fields must carry the declared set in order, got {:?}",
        err.referenced_fields
    );
}

#[test]
fn u2_field_is_not_gate_id_for_semantic_gate_violation() {
    // KTD2 / RF3: `field` must not carry the gate ID. For
    // SemanticGateViolation the `field` slot is empty because
    // the violation is not field-scoped at the schema level.
    let finding = consistency_finding_with_fields("rule-y", &["fix_status"]);
    let decision = PolicyDecision::RejectWithResume(finding);
    let err = finding_to_validation_error(&decision, "fix.done")
        .expect("RejectWithResume must surface as ValidationError");
    assert!(
        err.field.is_empty(),
        "field must not carry the gate ID for SemanticGateViolation, got {:?}",
        err.field
    );
}

#[test]
fn u2_empty_referenced_fields_serialises_as_empty_array() {
    // Timing/state gates (e.g. review_passed_while_wave_open)
    // carry an empty referenced_fields list — the violation is
    // not field-scoped. Agent tooling treats empty as "no
    // payload field to inspect; check state/context instead".
    let finding = PolicyFinding {
        topic: "review.passed".to_string(),
        violation_type: ViolationType::SemanticGateViolation {
            gate: "review_passed_while_wave_open".to_string(),
            context: "wave='w-1' received=0/3 expected".to_string(),
            referenced_fields: Vec::new(),
        },
        message: "review.passed while wave open".to_string(),
        evidence: None,
    };
    let decision = PolicyDecision::RejectWithResume(finding);
    let err = finding_to_validation_error(&decision, "review.passed")
        .expect("RejectWithResume must surface as ValidationError");
    assert_eq!(
        err.referenced_fields,
        Some(Vec::new()),
        "empty referenced_fields must serialise as empty Vec, got {:?}",
        err.referenced_fields
    );
}

#[test]
fn u2_hold_and_block_carry_structured_metadata() {
    // All enforce dispositions (Hold/Block/Ignore) share the
    // same finding_record path; they must all surface the
    // structured gate + referenced_fields.
    let finding = consistency_finding_with_fields("rule-h", &["fix_status"]);
    for decision in [
        PolicyDecision::Hold(finding.clone()),
        PolicyDecision::Block(finding.clone()),
        PolicyDecision::Ignore(finding.clone()),
    ] {
        let err = finding_to_validation_error(&decision, "fix.done")
            .expect("enforce disposition must surface as ValidationError");
        assert_eq!(
            err.gate.as_deref(),
            Some("payload_consistency:rule-h"),
            "Hold/Block/Ignore must carry structured gate"
        );
        assert_eq!(
            err.referenced_fields,
            Some(vec!["fix_status".to_string()]),
            "Hold/Block/Ignore must carry structured referenced_fields"
        );
    }
}
