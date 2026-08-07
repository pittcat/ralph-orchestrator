use super::*;
use ralph_core::PolicyFinding;

fn payload_consistency_finding(rule_id: &str) -> PolicyFinding {
    PolicyFinding {
        topic: "fix.done".to_string(),
        violation_type: ViolationType::SemanticGateViolation {
            gate: format!("payload_consistency:{rule_id}"),
            context: "test context".to_string(),
            referenced_fields: Vec::new(),
        },
        message: format!("payload_consistency rule '{rule_id}' violated"),
        evidence: None,
    }
}

fn other_namespace_warn_finding() -> PolicyFinding {
    PolicyFinding {
        topic: "work.done".to_string(),
        violation_type: ViolationType::SemanticGateViolation {
            gate: "other_namespace:rule-1".to_string(),
            context: "test context".to_string(),
            referenced_fields: Vec::new(),
        },
        message: "other namespace warning".to_string(),
        evidence: None,
    }
}

fn non_semantic_gate_warn_finding() -> PolicyFinding {
    PolicyFinding {
        topic: "work.done".to_string(),
        violation_type: ViolationType::MissingRequiredField {
            field: "task_id".to_string(),
        },
        message: "missing required field".to_string(),
        evidence: None,
    }
}

#[test]
fn u1_payload_consistency_warn_is_not_escalated() {
    // RF1: `Warn` carrying a `payload_consistency:` gate must
    // match the runtime Apply disposition (non-fatal). The
    // precheck no longer escalates by gate prefix.
    let decision = PolicyDecision::Warn(vec![payload_consistency_finding(
        "fix-done-blocked-zero-fixes-applied",
    )]);
    let result = finding_to_validation_error(&decision, "fix.done");
    assert!(
        result.is_none(),
        "payload_consistency Warn must match Apply (non-fatal), got ValidationError"
    );
}

#[test]
fn u1_other_namespace_warn_remains_non_fatal() {
    let decision = PolicyDecision::Warn(vec![other_namespace_warn_finding()]);
    let result = finding_to_validation_error(&decision, "work.done");
    assert!(
        result.is_none(),
        "non-payload_consistency Warn must remain non-fatal, got ValidationError"
    );
}

#[test]
fn u1_non_semantic_gate_warn_remains_non_fatal() {
    let decision = PolicyDecision::Warn(vec![non_semantic_gate_warn_finding()]);
    let result = finding_to_validation_error(&decision, "work.done");
    assert!(
        result.is_none(),
        "non-SemanticGateViolation Warn must remain non-fatal, got ValidationError"
    );
}

#[test]
fn u1_mixed_warn_findings_remain_non_fatal() {
    // A Warn batch that mixes consistency + other-namespace
    // findings stays non-fatal; the precheck no longer picks
    // the consistency finding to escalate.
    let decision = PolicyDecision::Warn(vec![
        other_namespace_warn_finding(),
        payload_consistency_finding("rule-x"),
    ]);
    let result = finding_to_validation_error(&decision, "fix.done");
    assert!(
        result.is_none(),
        "mixed Warn must remain non-fatal, got ValidationError"
    );
}

#[test]
fn u1_accept_remains_none() {
    let result = finding_to_validation_error(&PolicyDecision::Accept, "fix.done");
    assert!(result.is_none());
}

#[test]
fn u1_acknowledge_and_forward_remains_none() {
    // AcknowledgeAndForward is the dedup carve-out: same as
    // Accept from the precheck's perspective.
    let finding = payload_consistency_finding("rule-z");
    let decision = PolicyDecision::AcknowledgeAndForward(finding);
    let result = finding_to_validation_error(&decision, "fix.done");
    assert!(result.is_none());
}

#[test]
fn u1_reject_with_resume_surfaces_error() {
    // Enforce + RejectWithResume still surfaces as a typed
    // error; only Warn disposition is unified.
    let finding = payload_consistency_finding("rule-y");
    let decision = PolicyDecision::RejectWithResume(finding);
    let result = finding_to_validation_error(&decision, "fix.done");
    assert!(result.is_some());
    assert_eq!(result.unwrap().reason_code, "semantic_gate_violation");
}

#[test]
fn u1_hold_surfaces_error() {
    let finding = payload_consistency_finding("rule-h");
    let decision = PolicyDecision::Hold(finding);
    let result = finding_to_validation_error(&decision, "fix.done");
    assert!(result.is_some());
    assert_eq!(result.unwrap().reason_code, "semantic_gate_violation");
}

#[test]
fn u1_block_surfaces_error() {
    let finding = payload_consistency_finding("rule-b");
    let decision = PolicyDecision::Block(finding);
    let result = finding_to_validation_error(&decision, "fix.done");
    assert!(result.is_some());
    assert_eq!(result.unwrap().reason_code, "semantic_gate_violation");
}

#[test]
fn u1_ignore_surfaces_error() {
    let finding = payload_consistency_finding("rule-i");
    let decision = PolicyDecision::Ignore(finding);
    let result = finding_to_validation_error(&decision, "fix.done");
    assert!(result.is_some());
    assert_eq!(result.unwrap().reason_code, "semantic_gate_violation");
}
