//! Cross-module tests for the diagnosis data model.
//!
//! These tests cover behavior that spans more than one submodule:
//! builder + builder.evidence interaction, JSONL round-trips, and
//! stable retry key behavior. Per-module unit tests live in the
//! `tests` block at the bottom of each submodule.

use super::envelope::{
    DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, EvidenceKind, EvidenceRef,
    MAX_ENVELOPE_MESSAGE_CHARS, MAX_EVIDENCE_SNIPPET_CHARS, RECOVERY_ENVELOPE_SCHEMA_VERSION,
    RecoveryDiagnosisEnvelope, RecoveryDiagnosisEnvelopeBuilder,
};
use super::journal::{DriftJournalEntry, DriftMetric, RecoveryJournalEntry};
use chrono::Utc;
use serde_json::Value;

#[test]
fn every_source_variant_round_trips_through_serde() {
    let sources = [
        DiagnosisSource::StallRecovery,
        DiagnosisSource::MissingEventGate,
        DiagnosisSource::WorkflowGuard,
        DiagnosisSource::ExecutionContract,
        DiagnosisSource::PayloadContract,
        DiagnosisSource::DriftMonitor,
        DiagnosisSource::HookRetry,
        DiagnosisSource::LoopStale,
        DiagnosisSource::TopicFormat,
        DiagnosisSource::AgentDocSync,
        DiagnosisSource::CliEmit,
        DiagnosisSource::FlowLifecycle,
    ];
    for source in sources {
        let json = serde_json::to_string(&source).unwrap();
        let back: DiagnosisSource = serde_json::from_str(&json).unwrap();
        assert_eq!(back, source);
    }
}

#[test]
fn every_severity_variant_round_trips_through_serde() {
    let severities = [
        DiagnosisSeverity::Info,
        DiagnosisSeverity::Warning,
        DiagnosisSeverity::Error,
        DiagnosisSeverity::Critical,
    ];
    for severity in severities {
        let json = serde_json::to_string(&severity).unwrap();
        let back: DiagnosisSeverity = serde_json::from_str(&json).unwrap();
        assert_eq!(back, severity);
    }
}

#[test]
fn every_outcome_variant_round_trips_through_serde() {
    let outcomes = [
        DiagnosisOutcome::Pending,
        DiagnosisOutcome::Recovered,
        DiagnosisOutcome::Repeated,
        DiagnosisOutcome::Escalated,
        DiagnosisOutcome::Failed,
        DiagnosisOutcome::NotRetriable,
    ];
    for outcome in outcomes {
        let json = serde_json::to_string(&outcome).unwrap();
        let back: DiagnosisOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(back, outcome);
    }
}

#[test]
fn source_strings_match_expected_snake_case() {
    let pairs = [
        (DiagnosisSource::StallRecovery, "stall_recovery"),
        (DiagnosisSource::MissingEventGate, "missing_event_gate"),
        (DiagnosisSource::WorkflowGuard, "workflow_guard"),
        (DiagnosisSource::ExecutionContract, "execution_contract"),
        (DiagnosisSource::PayloadContract, "payload_contract"),
        (DiagnosisSource::DriftMonitor, "drift_monitor"),
        (DiagnosisSource::HookRetry, "hook_retry"),
        (DiagnosisSource::LoopStale, "loop_stale"),
        (DiagnosisSource::TopicFormat, "topic_format"),
        (DiagnosisSource::AgentDocSync, "agent_doc_sync"),
    ];
    for (source, expected) in pairs {
        let json = serde_json::to_value(source).unwrap();
        assert_eq!(json.as_str().unwrap(), expected);
    }
}

#[test]
fn retry_key_is_stable_across_calls() {
    let key_a = RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
        DiagnosisSource::MissingEventGate,
        Some("builder"),
        Some("work.done"),
        "no_emit",
        Some("plan_name"),
    );
    let key_b = RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
        DiagnosisSource::MissingEventGate,
        Some("builder"),
        Some("work.done"),
        "no_emit",
        Some("plan_name"),
    );
    assert_eq!(key_a, key_b);
}

#[test]
fn retry_key_is_pure_no_timestamps() {
    // The retry key must be a deterministic function of its parts.
    // We assert it does not change across two captures taken at
    // different wall-clock times.
    let args = (
        DiagnosisSource::ExecutionContract,
        Some("builder"),
        Some("work.done"),
        "missing_field",
        Some("plan_name"),
    );
    let key_a = RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
        args.0, args.1, args.2, args.3, args.4,
    );
    std::thread::sleep(std::time::Duration::from_millis(5));
    let key_b = RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
        args.0, args.1, args.2, args.3, args.4,
    );
    assert_eq!(key_a, key_b);
    // Sanity: no colons come from inside the parts themselves.
    assert_eq!(key_a.matches(':').count(), 4);
}

#[test]
fn retry_key_normalizes_case_and_special_chars() {
    let key = RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
        DiagnosisSource::ExecutionContract,
        Some("Builder Hat"),
        Some("work.Done"),
        "Missing Field",
        Some("plan-name.v2"),
    );
    assert_eq!(
        key,
        "execution_contract:builder_hat:work_done:missing_field:plan_name_v2"
    );
}

#[test]
fn evidence_ref_truncates_long_snippet() {
    let long = "x".repeat(MAX_EVIDENCE_SNIPPET_CHARS + 100);
    let r = EvidenceRef::new(EvidenceKind::Log, "log", Some(long));
    let snippet = r.snippet.expect("snippet should be present");
    assert_eq!(snippet.chars().count(), MAX_EVIDENCE_SNIPPET_CHARS);
    assert!(snippet.ends_with('\u{2026}'));
}

#[test]
fn evidence_ref_short_snippet_preserved() {
    let r = EvidenceRef::new(EvidenceKind::Log, "log", Some("ERROR: foo".to_string()));
    assert_eq!(r.kind, EvidenceKind::Log);
    assert_eq!(r.ref_path, "log");
    assert_eq!(r.snippet.as_deref(), Some("ERROR: foo"));
}

#[test]
fn evidence_ref_round_trips_through_serde() {
    let r = EvidenceRef::new(EvidenceKind::Log, "log", Some("ERROR: foo".to_string()));
    let s = serde_json::to_string(&r).unwrap();
    let back: EvidenceRef = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
}

#[test]
fn builder_stamps_schema_version_id_and_timestamp() {
    let before = Utc::now();
    let env = RecoveryDiagnosisEnvelope::builder()
        .iteration(7)
        .reason_code("test")
        .message("hello")
        .build();
    let after = Utc::now();
    assert_eq!(env.schema_version, RECOVERY_ENVELOPE_SCHEMA_VERSION);
    assert!(!env.diagnosis_id.is_empty());
    assert_eq!(env.diagnosis_id.len(), 36); // UUIDv4 string length
    assert!(env.timestamp >= before && env.timestamp <= after);
    assert_eq!(env.iteration, Some(7));
    assert_eq!(env.outcome, DiagnosisOutcome::Pending);
    assert_eq!(env.retry_attempt, 0);
    assert!(!env.safe_target); // Default to false (fail closed)
}

#[test]
fn with_outcome_updates_outcome_and_attempt() {
    let env = RecoveryDiagnosisEnvelope::builder()
        .iteration(2)
        .reason_code("r1")
        .message("m")
        .retry_attempt(0)
        .safe_target(true)
        .build();
    let next = env.with_outcome(DiagnosisOutcome::Recovered, 1);
    // Identity preserved.
    assert_eq!(next.diagnosis_id, env.diagnosis_id);
    assert_eq!(next.retry_key, env.retry_key);
    assert_eq!(next.source, env.source);
    assert_eq!(next.reason_code, env.reason_code);
    assert_eq!(next.iteration, env.iteration);
    assert_eq!(next.severity, env.severity);
    // Updated.
    assert_eq!(next.outcome, DiagnosisOutcome::Recovered);
    assert_eq!(next.retry_attempt, 1);
    // Unchanged.
    assert_eq!(next.safe_target, env.safe_target);
}

#[test]
fn recovery_journal_entry_from_envelope() {
    let env = RecoveryDiagnosisEnvelope::builder()
        .iteration(4)
        .reason_code("missing_emit")
        .message("m")
        .build();
    let entry = RecoveryJournalEntry::from_envelope(env.clone(), vec!["a".to_string()]);
    assert_eq!(entry.schema_version, 1);
    assert_eq!(entry.envelope, env);
    assert_eq!(entry.iteration, Some(4));
    assert_eq!(entry.notes, vec!["a".to_string()]);
}

#[test]
fn drift_entry_into_envelope_uses_drift_source() {
    let entry = DriftJournalEntry::builder()
        .metric(DriftMetric::FieldCompleteness)
        .observed_value(0.4)
        .threshold(0.9)
        .severity(DiagnosisSeverity::Warning)
        .topic("work.done")
        .field("plan_name")
        .window_iterations(20)
        .iteration(7)
        .message("plan_name missing in 60% of events")
        .build();
    let env: RecoveryDiagnosisEnvelope = entry.into_envelope();
    assert_eq!(env.source, DiagnosisSource::DriftMonitor);
    assert!(env.retry_key.starts_with("drift_monitor:"));
    assert_eq!(env.severity, DiagnosisSeverity::Warning);
    assert_eq!(env.iteration, Some(7));
    assert_eq!(env.evidence.len(), 1);
    assert_eq!(env.evidence[0].kind, EvidenceKind::Field);
    assert_eq!(env.evidence[0].ref_path, "plan_name");
}

#[test]
fn recovery_journal_entry_jsonl_round_trip() {
    let env = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::ExecutionContract)
        .severity(DiagnosisSeverity::Error)
        .iteration(12)
        .source_hat("builder")
        .target_hat("builder")
        .topic("work.done")
        .reason_code("missing_field")
        .message("plan_name required")
        .expected_action("re-emit with plan_name")
        .evidence(EvidenceRef::new(
            EvidenceKind::Field,
            "plan_name",
            Some("field not present".to_string()),
        ))
        .safe_target(true)
        .outcome(DiagnosisOutcome::Repeated)
        .retry_attempt(2)
        .build();
    let entry = RecoveryJournalEntry::from_envelope(env, vec!["contract-v2".to_string()]);

    let line = serde_json::to_string(&entry).unwrap();
    // Every JSONL line must be a single self-contained JSON object.
    let parsed: Value = serde_json::from_str(&line).expect("valid JSONL line");
    assert!(parsed.is_object());
    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["iteration"], 12);
    assert_eq!(parsed["envelope"]["source"], "execution_contract");
    assert_eq!(parsed["envelope"]["severity"], "error");
    assert_eq!(parsed["envelope"]["retry_key"], entry.envelope.retry_key);

    let back: RecoveryJournalEntry = serde_json::from_str(&line).unwrap();
    assert_eq!(back, entry);
}

#[test]
fn long_message_is_truncated_to_max_chars() {
    let long = "a".repeat(MAX_ENVELOPE_MESSAGE_CHARS + 50);
    let env = RecoveryDiagnosisEnvelope::builder()
        .reason_code("r")
        .message(long)
        .build();
    assert_eq!(env.message.chars().count(), MAX_ENVELOPE_MESSAGE_CHARS);
    assert!(env.message.ends_with('\u{2026}'));
}

#[test]
fn default_retry_key_uses_last_field_evidence() {
    let env = RecoveryDiagnosisEnvelope::builder()
        .reason_code("missing_field")
        .message("m")
        .source(DiagnosisSource::ExecutionContract)
        .target_hat("builder")
        .topic("work.done")
        .evidence(EvidenceRef::new(EvidenceKind::File, "src/x.rs", None))
        .evidence(EvidenceRef::new(EvidenceKind::Field, "plan_name", None))
        .build();
    assert!(
        env.retry_key.ends_with(":plan_name"),
        "retry_key = {}",
        env.retry_key
    );
    // And the full key has the expected shape.
    assert_eq!(
        env.retry_key,
        "execution_contract:builder:work_done:missing_field:plan_name"
    );
}

#[test]
fn explicit_retry_key_overrides_derivation() {
    let env = RecoveryDiagnosisEnvelope::builder()
        .reason_code("r")
        .message("m")
        .retry_key("custom:key")
        .build();
    assert_eq!(env.retry_key, "custom:key");
}

#[test]
fn optional_fields_are_omitted_from_json_when_none() {
    let env = RecoveryDiagnosisEnvelope::builder()
        .reason_code("r")
        .message("m")
        .build();
    let json: Value = serde_json::to_value(&env).unwrap();
    // Optional fields should be skipped (not serialized) when None.
    assert!(json.get("source_hat").is_none() || json["source_hat"].is_null());
    assert!(json.get("target_hat").is_none() || json["target_hat"].is_null());
    assert!(json.get("topic").is_none() || json["topic"].is_null());
    assert!(json.get("expected_action").is_none() || json["expected_action"].is_null());
    assert!(json.get("session_id").is_none() || json["session_id"].is_null());
    // But the required fields ARE present.
    assert!(json["schema_version"].is_number());
    assert!(json["diagnosis_id"].is_string());
    assert!(json["source"].is_string());
    assert!(json["severity"].is_string());
    assert!(json["retry_key"].is_string());
    assert!(json["retry_attempt"].is_number());
    assert!(json["safe_target"].is_boolean());
    assert!(json["outcome"].is_string());
    assert!(json["timestamp"].is_string());
}

#[test]
fn drift_entry_round_trips_through_serde() {
    let entry = DriftJournalEntry::builder()
        .metric(DriftMetric::CoordJoinRate)
        .observed_value(0.3)
        .threshold(0.6)
        .severity(DiagnosisSeverity::Warning)
        .from_topic("work.done")
        .to_topic("review.wave.ready")
        .window_iterations(50)
        .iteration(99)
        .message("coord join rate dropped")
        .build();
    let s = serde_json::to_string(&entry).unwrap();
    let back: DriftJournalEntry = serde_json::from_str(&s).unwrap();
    assert_eq!(back, entry);
}
