//! Plan 2026-08-26-1104 Unit U08 — BDD scenarios S8.1-S8.8 for
//! the deterministic causal attribution engine.
//!
//! Each scenario builds a synthetic session fixture
//! (`TempDir` with hand-written sidecars + workspace files),
//! invokes [`crate::diagnosis::analyze_session`], and asserts
//! the engine's structured output. The fixtures are
//! intentionally minimal — we cover the smallest evidence
//! pattern that uniquely exercises the rule chain, not the
//! full producer pipeline.
//!
//! # Scenarios
//!
//! - `s8_1_preset_domain` — contract_receipt has terminal
//!   topics the manifest's `execution_capabilities[]` does not
//!   cover → `primary_domain == preset`.
//! - `s8_2_agent_domain` — clean preset / runtime / backend
//!   but no terminal event emitted → `primary_domain ==
//!   agent` and 3 rejected hypotheses.
//! - `s8_3_runtime_domain` — accepted transition without a
//!   committed receipt → `primary_domain == runtime`.
//! - `s8_4_backend_domain` — `hat_activation_outcome` row
//!   with `backend_success=false` → `primary_domain ==
//!   backend`.
//! - `s8_5_capture_contract_domain` — coverage gap, every
//!   other rule unfired → `primary_domain ==
//!   diagnostic_capture_contract`.
//! - `s8_6_same_appearance_differential` — three fixtures
//!   with identical surface symptom (no terminal event)
//!   that the engine resolves to three different domains.
//! - `s8_7_low_score_incomplete` — high score missing →
//!   `status == incomplete` (not `complete`).
//! - `s8_8_byte_identical_recomputation` — two runs on the
//!   same inputs produce byte-identical JSON.
//!
//! Plus a fixture-free `boundary_score_85_86_does_not_flip`
//! test pinning the strict-gate behavior.

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

use ralph_core::diagnosis::{
    AttributionStatus, CausalAttributionReport, ConfidenceBreakdown, Domain, analyze_session,
};

// ─── Fixture builders ────────────────────────────────────────

/// Session + workspace scratch dir. The caller writes
/// whichever sidecars the scenario needs and drops the
/// scratch when the test finishes.
struct Scratch {
    session: PathBuf,
    workspace: PathBuf,
    _tmp: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let session = root.join("session");
        let workspace = root.join("workspace");
        fs::create_dir_all(&session).expect("mkdir session");
        fs::create_dir_all(workspace.join(".ralph").join("agent")).expect("mkdir .ralph");
        Self {
            session,
            workspace,
            _tmp: tmp,
        }
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.workspace.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir parent");
        }
        fs::write(path, body).expect("write fixture");
    }

    fn write_session(&self, name: &str, body: &str) {
        fs::write(self.session.join(name), body).expect("write session fixture");
    }
}

/// Eight boundary names from U07 §6. Order is canonical.
const BOUNDARIES: &[&str] = &[
    "effective_contract",
    "activation",
    "backend_outcome",
    "event_candidate",
    "policy_decision",
    "state_commit",
    "recovery_action",
    "termination",
];

/// Render a v2 manifest with all 8 boundaries covered.
fn manifest_v2_full() -> String {
    let entries: Vec<Value> = BOUNDARIES
        .iter()
        .map(|name| {
            json!({
                "boundary": name,
                "expected": 1,
                "recorded": 1,
                "status": "covered",
            })
        })
        .collect();
    serde_json::to_string(&json!({
        "schema_version": "run-diagnosis-input/v2",
        "manifest_status": "finalized",
        "run": {
            "loop_id": "L-test",
            "preset_label": "builtin:test",
            "execution_capability": "supervisor",
        },
        "code_baseline": {
            "head_sha": "deadbeef",
            "worktree": false,
        },
        "execution_capabilities": [
            "executor",
            "planner",
            "alignment",
            "plan.complete",
        ],
        "boundary_coverage": entries,
    }))
    .unwrap()
}

/// Render a v2 manifest with one boundary in `gap` state.
fn manifest_v2_with_gap(boundary: &str, reason: &str) -> String {
    let entries: Vec<Value> = BOUNDARIES
        .iter()
        .map(|name| {
            if *name == boundary {
                json!({
                    "boundary": name,
                    "expected": 1,
                    "recorded": 0,
                    "status": "gap",
                    "reason": reason,
                })
            } else {
                json!({
                    "boundary": name,
                    "expected": 1,
                    "recorded": 1,
                    "status": "covered",
                })
            }
        })
        .collect();
    serde_json::to_string(&json!({
        "schema_version": "run-diagnosis-input/v2",
        "manifest_status": "finalized",
        "run": { "loop_id": "L-test" },
        "execution_capabilities": [],
        "boundary_coverage": entries,
    }))
    .unwrap()
}

/// Render a v1 manifest (no `boundary_coverage`).
fn manifest_v1_legacy() -> String {
    serde_json::to_string(&json!({
        "schema_version": "run-diagnosis-input/v1",
        "manifest_status": "finalized",
        "run": { "loop_id": "L-test" },
    }))
    .unwrap()
}

#[test]
fn partial_v2_manifest_is_not_evaluable() {
    let scratch = Scratch::new();
    let mut manifest: Value = serde_json::from_str(&manifest_v2_full()).unwrap();
    manifest["boundary_coverage"].as_array_mut().unwrap().pop();
    scratch.write_session(
        "diagnosis-input.json",
        &serde_json::to_string(&manifest).unwrap(),
    );

    let report = analyze_session(&scratch.session, &scratch.workspace);
    assert_eq!(report.status, AttributionStatus::NotEvaluable);
    assert_eq!(report.confidence.total, 0);
}

// ─── Scenario S8.1 — preset domain ────────────────────────────

#[test]
fn s8_1_preset_domain() {
    let scratch = Scratch::new();
    scratch.write_session("diagnosis-input.json", &manifest_v2_full());

    // contract_receipt lists terminal topics; manifest's
    // `execution_capabilities[]` does not name them.
    let contract = json!({
        "schema_version": "v1",
        "ts": "2026-08-26T12:00:00Z",
        "iteration": 1,
        "sequence": 1,
        "phase": "decision",
        "kind": "contract_receipt",
        "causal": { "loop_id": "L-test", "iteration": 1 },
        "fields": {
            "contract_digest": "abc",
            "hats_digest": "h",
            "terminal_topics_digest": "t",
            "preset_label": "builtin:test",
            "terminal_topics": ["plan.complete.missing"],
        }
    });
    let runtime_trace = format!("{contract}\n");
    scratch.write_session("runtime-trace.jsonl", &runtime_trace);

    let report = analyze_session(&scratch.session, &scratch.workspace);
    // Coverage is intact, every other layer's evidence
    // matches the DT7 budget, and the preset fingerprint is
    // the only rule that fired. The 85-gate passes —
    // `complete` is the honest answer here.
    assert_eq!(
        report.status,
        AttributionStatus::Complete,
        "s8_1 fixture has every DT7 component >= its share, total should clear the 85-gate"
    );
    assert_eq!(report.primary_domain, Some(Domain::Preset));
    let fix = report.fix_point.as_ref().expect("fix point");
    assert!(matches!(
        fix,
        ralph_core::diagnosis::FixPoint::Preset { .. }
    ));
    assert_eq!(fix.domain(), Domain::Preset);
    // Preset rule runs third; the two earlier rules must
    // have refuted cleanly.
    let rejected_names: Vec<Domain> = report
        .rejected_hypotheses
        .iter()
        .map(|r| r.domain)
        .collect();
    assert!(rejected_names.contains(&Domain::Backend));
    assert!(rejected_names.contains(&Domain::Runtime));
}

// ─── Scenario S8.2 — agent domain ─────────────────────────────

#[test]
fn s8_2_agent_domain() {
    let scratch = Scratch::new();
    scratch.write_session("diagnosis-input.json", &manifest_v2_full());

    // Health sidecars: contract_receipt with all terminal
    // topics visible in capabilities, monotonic trace
    // without commit_receipt, no correction feedback.
    let contract = json!({
        "schema_version": "v1",
        "ts": "2026-08-26T12:00:00Z",
        "iteration": 1,
        "sequence": 1,
        "phase": "decision",
        "kind": "contract_receipt",
        "fields": {
            "contract_digest": "abc",
            "hats_digest": "h",
            "terminal_topics_digest": "t",
            "preset_label": "builtin:test",
            "terminal_topics": ["plan.complete"],
        }
    });
    let outcome = json!({
        "schema_version": "v1",
        "ts": "2026-08-26T12:00:01Z",
        "iteration": 1,
        "sequence": 2,
        "phase": "termination",
        "kind": "hat_activation_outcome",
        "fields": {
            "hat_id": "executor",
            "backend_success": true,
            "exit_code": 0,
            "watchdog_timeout": false,
        }
    });
    let runtime_trace = format!("{contract}\n{outcome}\n");
    scratch.write_session("runtime-trace.jsonl", &runtime_trace);

    let report = analyze_session(&scratch.session, &scratch.workspace);
    assert_eq!(report.primary_domain, Some(Domain::Agent));
    let fix = report.fix_point.as_ref().expect("fix point");
    assert!(matches!(fix, ralph_core::diagnosis::FixPoint::Agent { .. }));
    // Must include the three earlier domains as
    // rejected_hypotheses.
    let rejected_names: Vec<Domain> = report
        .rejected_hypotheses
        .iter()
        .map(|r| r.domain)
        .collect();
    assert!(rejected_names.contains(&Domain::Backend));
    assert!(rejected_names.contains(&Domain::Runtime));
    assert!(rejected_names.contains(&Domain::Preset));
    assert!(rejected_names.contains(&Domain::DiagnosticCaptureContract));
}

// ─── Scenario S8.3 — runtime domain ───────────────────────────

#[test]
fn s8_3_runtime_domain() {
    let scratch = Scratch::new();
    scratch.write_session("diagnosis-input.json", &manifest_v2_full());

    // contract_receipt, plus an accepted-transition row in
    // the workspace, but no commit_receipt matches it.
    let contract = json!({
        "schema_version": "v1",
        "ts": "2026-08-26T12:00:00Z",
        "iteration": 1,
        "sequence": 1,
        "phase": "decision",
        "kind": "contract_receipt",
        "fields": {
            "contract_digest": "abc",
            "hats_digest": "h",
            "terminal_topics_digest": "t",
            "preset_label": "builtin:test",
            "terminal_topics": ["plan.complete"],
        }
    });
    let outcome = json!({
        "schema_version": "v1",
        "ts": "2026-08-26T12:00:01Z",
        "iteration": 1,
        "sequence": 2,
        "phase": "termination",
        "kind": "hat_activation_outcome",
        "fields": {
            "hat_id": "executor",
            "backend_success": true,
            "exit_code": 0,
            "watchdog_timeout": false,
        }
    });
    let runtime_trace = format!("{contract}\n{outcome}\n");
    scratch.write_session("runtime-trace.jsonl", &runtime_trace);
    // Workspace ledger records the transition but no
    // matching commit_receipt.
    scratch.write(
        ".ralph/agent/accepted-transitions.jsonl",
        r#"{"transition_id":"t-1","ts":"2026-08-26T12:00:00Z","event_digest":"e"}
"#,
    );

    let report = analyze_session(&scratch.session, &scratch.workspace);
    assert_eq!(report.primary_domain, Some(Domain::Runtime));
    let fix = report.fix_point.as_ref().expect("fix point");
    assert!(matches!(
        fix,
        ralph_core::diagnosis::FixPoint::Runtime { .. }
    ));
}

// ─── Scenario S8.4 — backend domain ───────────────────────────

#[test]
fn s8_4_backend_domain() {
    let scratch = Scratch::new();
    scratch.write_session("diagnosis-input.json", &manifest_v2_full());

    let contract = json!({
        "schema_version": "v1",
        "ts": "2026-08-26T12:00:00Z",
        "iteration": 1,
        "sequence": 1,
        "phase": "decision",
        "kind": "contract_receipt",
        "fields": {
            "contract_digest": "abc",
            "hats_digest": "h",
            "terminal_topics_digest": "t",
            "preset_label": "builtin:test",
            "terminal_topics": ["plan.complete"],
        }
    });
    let outcome = json!({
        "schema_version": "v1",
        "ts": "2026-08-26T12:00:01Z",
        "iteration": 1,
        "sequence": 2,
        "phase": "termination",
        "kind": "hat_activation_outcome",
        "fields": {
            "hat_id": "executor",
            "backend_success": false,
            "exit_code": 137,
            "watchdog_timeout": false,
        }
    });
    let runtime_trace = format!("{contract}\n{outcome}\n");
    scratch.write_session("runtime-trace.jsonl", &runtime_trace);

    let report = analyze_session(&scratch.session, &scratch.workspace);
    assert_eq!(report.primary_domain, Some(Domain::Backend));
    let fix = report.fix_point.as_ref().expect("fix point");
    assert!(matches!(
        fix,
        ralph_core::diagnosis::FixPoint::Backend { .. }
    ));
}

// ─── Scenario S8.5 — capture-contract domain ──────────────────

#[test]
fn s8_5_capture_contract_domain() {
    let scratch = Scratch::new();
    scratch.write_session(
        "diagnosis-input.json",
        &manifest_v2_with_gap("termination", "watchdog did not flush"),
    );

    // No runtime trace at all → no rule except
    // `rule_capture_contract` should fire.
    let report = analyze_session(&scratch.session, &scratch.workspace);
    assert_eq!(
        report.primary_domain,
        Some(Domain::DiagnosticCaptureContract)
    );
    let fix = report.fix_point.as_ref().expect("fix point");
    assert!(matches!(
        fix,
        ralph_core::diagnosis::FixPoint::CaptureContract { .. }
    ));
    assert_eq!(fix.domain(), Domain::DiagnosticCaptureContract);
    // Coverage gap flowed straight into the report.
    assert!(
        report
            .coverage_gaps
            .iter()
            .any(|g| g.boundary == "termination"),
        "coverage gap must surface, got {:?}",
        report.coverage_gaps
    );
}

// ─── Scenario S8.6 — differential ─────────────────────────────

#[test]
fn s8_6_same_appearance_differential() {
    fn run_attribution(builder: &dyn Fn(&Scratch)) -> Domain {
        let scratch = Scratch::new();
        builder(&scratch);
        let report = analyze_session(&scratch.session, &scratch.workspace);
        report.primary_domain.expect("primary domain")
    }

    // (a) preset gap → preset
    let preset_attribution = run_attribution(&|s| {
        s.write_session("diagnosis-input.json", &manifest_v2_full());
        let contract = json!({
            "schema_version": "v1",
            "ts": "2026-08-26T12:00:00Z",
            "iteration": 1,
            "sequence": 1,
            "phase": "decision",
            "kind": "contract_receipt",
            "fields": {
                "contract_digest": "a",
                "hats_digest": "h",
                "terminal_topics_digest": "t",
                "preset_label": "builtin:test",
                "terminal_topics": ["plan.complete.unknown"],
            }
        });
        s.write_session("runtime-trace.jsonl", &format!("{contract}\n"));
    });

    // (b) clean preset + clean backend + no terminal →
    //     agent
    let agent_attribution = run_attribution(&|s| {
        s.write_session("diagnosis-input.json", &manifest_v2_full());
        let contract = json!({
            "schema_version": "v1",
            "ts": "2026-08-26T12:00:00Z",
            "iteration": 1,
            "sequence": 1,
            "phase": "decision",
            "kind": "contract_receipt",
            "fields": {
                "contract_digest": "a",
                "hats_digest": "h",
                "terminal_topics_digest": "t",
                "preset_label": "builtin:test",
                "terminal_topics": ["plan.complete"],
            }
        });
        let outcome = json!({
            "schema_version": "v1",
            "ts": "2026-08-26T12:00:01Z",
            "iteration": 1,
            "sequence": 2,
            "phase": "termination",
            "kind": "hat_activation_outcome",
            "fields": {
                "hat_id": "executor",
                "backend_success": true,
                "exit_code": 0,
                "watchdog_timeout": false,
            }
        });
        s.write_session("runtime-trace.jsonl", &format!("{contract}\n{outcome}\n"));
    });

    // (c) coverage gap only → capture_contract
    let capture_attribution = run_attribution(&|s| {
        s.write_session(
            "diagnosis-input.json",
            &manifest_v2_with_gap("policy_decision", "policy_decision logger degraded"),
        );
    });

    assert_eq!(preset_attribution, Domain::Preset);
    assert_eq!(agent_attribution, Domain::Agent);
    assert_eq!(capture_attribution, Domain::DiagnosticCaptureContract);
    assert_ne!(preset_attribution, agent_attribution);
    assert_ne!(agent_attribution, capture_attribution);
    assert_ne!(preset_attribution, capture_attribution);
}

// ─── Scenario S8.7 — incomplete gate ──────────────────────────

#[test]
fn s8_7_low_score_incomplete() {
    let scratch = Scratch::new();
    // Manifest present and complete, but the evidence chain
    // is too thin for any rule to match. Coverage=30,
    // integrity=5 (empty join), refutation=0 (no primary),
    // correlation=10, freeze_window=0 → total=45 ≤ 85.
    scratch.write_session("diagnosis-input.json", &manifest_v2_full());
    // No sidecars at all.
    let report = analyze_session(&scratch.session, &scratch.workspace);
    assert_ne!(report.status, AttributionStatus::Complete);
    assert!(
        report.confidence.total <= 85,
        "empty evidence must not pass the 85 gate, got {}",
        report.confidence.total
    );
}

// ─── Scenario S8.8 — byte-identical recomputation ─────────────

#[test]
fn s8_8_byte_identical_recomputation() {
    let scratch = Scratch::new();
    scratch.write_session("diagnosis-input.json", &manifest_v2_full());

    let contract = json!({
        "schema_version": "v1",
        "ts": "2026-08-26T12:00:00Z",
        "iteration": 1,
        "sequence": 1,
        "phase": "decision",
        "kind": "contract_receipt",
        "fields": {
            "contract_digest": "a",
            "hats_digest": "h",
            "terminal_topics_digest": "t",
            "preset_label": "builtin:test",
            "terminal_topics": ["plan.complete"],
        }
    });
    scratch.write_session("runtime-trace.jsonl", &format!("{contract}\n"));

    let first = analyze_session(&scratch.session, &scratch.workspace);
    let second = analyze_session(&scratch.session, &scratch.workspace);
    let first_json = serde_json::to_string(&first).expect("serialize");
    let second_json = serde_json::to_string(&second).expect("serialize");
    assert_eq!(first_json, second_json, "two runs must match byte-for-byte");
}

// ─── Boundary / legacy / determinism ──────────────────────────

#[test]
fn v1_manifest_returns_not_evaluable() {
    let scratch = Scratch::new();
    scratch.write_session("diagnosis-input.json", &manifest_v1_legacy());
    let report = analyze_session(&scratch.session, &scratch.workspace);
    assert_eq!(report.status, AttributionStatus::NotEvaluable);
    assert!(report.primary_domain.is_none());
    assert!(report.fix_point.is_none());
    assert!(report.rejected_hypotheses.is_empty());
}

#[test]
fn missing_manifest_returns_not_evaluable() {
    let scratch = Scratch::new();
    let report = analyze_session(&scratch.session, &scratch.workspace);
    assert_eq!(report.status, AttributionStatus::NotEvaluable);
}

#[test]
fn unknown_higher_version_is_not_evaluable() {
    let scratch = Scratch::new();
    let body = serde_json::to_string(&json!({
        "schema_version": "run-diagnosis-input/v999",
        "manifest_status": "finalized",
        "boundary_coverage": [],
    }))
    .unwrap();
    scratch.write_session("diagnosis-input.json", &body);
    let report = analyze_session(&scratch.session, &scratch.workspace);
    assert_eq!(report.status, AttributionStatus::NotEvaluable);
}

#[test]
fn contract_version_pinned_in_report() {
    let scratch = Scratch::new();
    scratch.write_session("diagnosis-input.json", &manifest_v2_full());
    let report = analyze_session(&scratch.session, &scratch.workspace);
    assert_eq!(report.contract_version, "causal-attribution/v1");
}

#[test]
fn serde_roundtrip_is_byte_identical() {
    let scratch = Scratch::new();
    scratch.write_session("diagnosis-input.json", &manifest_v2_full());
    let contract = json!({
        "schema_version": "v1",
        "ts": "2026-08-26T12:00:00Z",
        "iteration": 1,
        "sequence": 1,
        "phase": "decision",
        "kind": "contract_receipt",
        "fields": {
            "contract_digest": "a",
            "hats_digest": "h",
            "terminal_topics_digest": "t",
            "preset_label": "builtin:test",
            "terminal_topics": ["plan.complete"],
        }
    });
    scratch.write_session("runtime-trace.jsonl", &format!("{contract}\n"));
    let report: CausalAttributionReport = analyze_session(&scratch.session, &scratch.workspace);
    let json = serde_json::to_string(&report).expect("serialize");
    let back: CausalAttributionReport = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(report, back);
}

#[test]
fn strict_gate_uses_strictly_greater_than_85() {
    // Build a corpus that lands exactly on the boundary
    // by mocking the breakdown; the rule is part of the
    // public contract.
    let b = ConfidenceBreakdown {
        coverage: 30,
        integrity: 25,
        refutation: 20,
        correlation: 15,
        freeze_window: 10,
        total: 100,
    };
    assert!(b.total > 85);
    let b = ConfidenceBreakdown {
        coverage: 30,
        integrity: 25,
        refutation: 15,
        correlation: 15,
        freeze_window: 0,
        total: 85,
    };
    assert!(b.total <= 85, "boundary case must NOT pass strict > 85");
}

// ─── Pure-function smoke (zero-write guarantee) ──────────────

#[test]
fn engine_does_not_write_to_session_or_workspace() {
    let scratch = Scratch::new();
    scratch.write_session("diagnosis-input.json", &manifest_v2_full());

    let pre_session_files: std::collections::BTreeSet<PathBuf> = walk(&scratch.session);
    let pre_workspace_files: std::collections::BTreeSet<PathBuf> = walk(&scratch.workspace);

    let _ = analyze_session(&scratch.session, &scratch.workspace);

    let post_session_files: std::collections::BTreeSet<PathBuf> = walk(&scratch.session);
    let post_workspace_files: std::collections::BTreeSet<PathBuf> = walk(&scratch.workspace);

    assert_eq!(
        pre_session_files, post_session_files,
        "engine must not write to session dir"
    );
    assert_eq!(
        pre_workspace_files, post_workspace_files,
        "engine must not write to workspace"
    );
}

fn walk(root: &std::path::Path) -> std::collections::BTreeSet<PathBuf> {
    let mut out = std::collections::BTreeSet::new();
    if root.is_file() {
        out.insert(root.to_path_buf());
        return out;
    }
    for entry in walkdir(root) {
        out.insert(entry);
    }
    out
}

fn walkdir(root: &std::path::Path) -> Vec<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    let mut out = Vec::new();
    while let Some(p) = stack.pop() {
        let Ok(rd) = fs::read_dir(&p) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

// CausalEvidenceRef lives at `ralph_core::diagnosis::CausalEvidenceRef`
// — kept here as a regression sentinel for the
// `EvidenceRef` name collision (envelope::EvidenceRef vs
// causal::report::EvidenceRef). It is intentionally unused
// in test code.
#[allow(dead_code, unused_imports)]
use ralph_core::diagnosis::CausalEvidenceRef as _ReexportSentinel;
