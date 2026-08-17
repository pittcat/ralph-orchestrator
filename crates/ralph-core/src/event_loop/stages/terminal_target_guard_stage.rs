//! Plan 2026-08-16-1015 Unit 4: `TerminalTargetGuardStage` —
//! fail-closes terminal events whose schema declares a
//! `required_target_hat` contract.
//!
//! Why this lives next to `TargetHatGuard`: the existing guard
//! catches `task.resume → source_hat` self-loops; this new guard
//! catches terminal events whose explicit target does not match
//! the schema-declared contract (e.g. `report.done` must target
//! `reporter`, never `executor`). P0 was `report.done →
//! triggered=executor` being accepted and the handoff tracker
//! still registering `reporter`'s deadline while routing to
//! `executor`, leaving the loop with duplicate `work.done` and
//! no progress.
//!
//! The guard only inspects events whose topic has a non-`None`
//! `required_target_hat` entry in `contracts`; everything else
//! falls through to existing target semantics.

use crate::event_loop::stage_pipeline::{EmitStage, StageContext, StageReject};
use ralph_proto::Event;
use serde_json::Value;
use std::collections::HashMap;

/// Plan 2026-08-16-1015 U9: lifted from `extract_payload_target`
/// so the test module can also reference the same canonical list
/// when asserting 5-source priority.
const PAYLOAD_TARGET_KEYS: &[&str] = &["target_hat", "triggered", "target"];

/// Per-stage reason codes. Stable strings used by callers and
/// downstream diagnostic tooling.
pub const REASON_MISSING_TARGET: &str = "terminal_target_missing";
pub const REASON_TARGET_MISMATCH: &str = "terminal_target_mismatch";

pub struct TerminalTargetGuardStage {
    /// Map of `topic -> required_target_hat`. Built from
    /// `EventSchema::required_target_hat` by the caller (see
    /// `flow_wiring`).
    contracts: HashMap<String, String>,
}

impl TerminalTargetGuardStage {
    pub fn new(contracts: HashMap<String, String>) -> Self {
        Self { contracts }
    }

    pub fn empty() -> Self {
        Self::new(HashMap::new())
    }

    pub fn contracts(&self) -> &HashMap<String, String> {
        &self.contracts
    }
}

impl Default for TerminalTargetGuardStage {
    fn default() -> Self {
        Self::empty()
    }
}

impl EmitStage for TerminalTargetGuardStage {
    fn name(&self) -> &'static str {
        "TerminalTargetGuard"
    }

    fn check(&self, _ctx: &mut StageContext, event: &Event) -> Result<(), StageReject> {
        let required = match self.contracts.get(event.topic.as_str()) {
            Some(target) if !target.is_empty() => target,
            _ => return Ok(()), // No contract declared → no gate.
        };

        // The explicit target hat comes from one of FOUR places,
        // in priority order:
        //   1. payload.target_hat — explicit emit override.
        //   2. payload.triggered — CLI/agent alias.
        //   3. payload.target — generic schema field alias.
        //   4. event.target — runtime carrier field (set by
        //      `EventReader` from the JSONL `triggered` field).
        // `event.source` is the publishing hat, never a target fallback.
        let payload_target = extract_payload_target(&event.payload);
        let carrier_target = event.target.as_ref().map(|h| h.as_str().to_string());

        let actual = payload_target.or(carrier_target);

        match actual {
            None => Err(StageReject::new(self.name(), REASON_MISSING_TARGET)),
            Some(t) if t.as_str() != required.as_str() => {
                Err(StageReject::new(self.name(), REASON_TARGET_MISMATCH))
            }
            Some(_) => Ok(()),
        }
    }
}

/// Pull `target_hat` (or common aliases) out of a JSON payload.
fn extract_payload_target(payload: &str) -> Option<String> {
    let value: Value = serde_json::from_str(payload).ok()?;
    let obj = value.as_object()?;
    for key in PAYLOAD_TARGET_KEYS {
        if let Some(s) = obj.get(*key).and_then(|v| v.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_loop::repair_flow::RepairStateMachine;
    use crate::event_loop::stage_pipeline::{FlowStep, StageContext};
    use ralph_proto::HatId;

    fn ctx(repair: &mut RepairStateMachine) -> StageContext<'_> {
        StageContext::for_test_machine(FlowStep::new("unit_loop"), "loop-1", 1, repair)
    }

    fn stage() -> TerminalTargetGuardStage {
        let mut contracts = HashMap::new();
        contracts.insert("report.done".into(), "reporter".into());
        TerminalTargetGuardStage::new(contracts)
    }

    fn event_for(topic: &str, target: Option<&str>, triggered: Option<&str>) -> Event {
        let mut payload = serde_json::Map::new();
        if let Some(t) = target {
            payload.insert("target_hat".into(), Value::String(t.into()));
        }
        if let Some(t) = triggered {
            payload.insert("triggered".into(), Value::String(t.into()));
        }
        let mut e = Event::new(topic, serde_json::Value::Object(payload).to_string());
        if let Some(t) = target {
            e.target = Some(HatId::new(t));
        }
        e
    }

    #[test]
    fn topic_without_contract_passes() {
        let s = stage();
        let mut repair = RepairStateMachine::default();
        let mut c = ctx(&mut repair);
        let e = event_for("work.done", Some("executor"), None);
        assert!(s.check(&mut c, &e).is_ok());
    }

    #[test]
    fn correct_target_accepts() {
        let s = stage();
        let mut repair = RepairStateMachine::default();
        let mut c = ctx(&mut repair);
        let e = event_for("report.done", Some("reporter"), None);
        assert!(s.check(&mut c, &e).is_ok());
    }

    #[test]
    fn wrong_target_rejected() {
        let s = stage();
        let mut repair = RepairStateMachine::default();
        let mut c = ctx(&mut repair);
        let e = event_for("report.done", Some("executor"), None);
        let err = s.check(&mut c, &e).unwrap_err();
        assert_eq!(err.reason_code, "terminal_target_mismatch");
    }

    #[test]
    fn triggered_field_carries_target() {
        let s = stage();
        let mut repair = RepairStateMachine::default();
        let mut c = ctx(&mut repair);
        // target_hat absent, but `triggered` says reporter.
        let e = event_for("report.done", None, Some("reporter"));
        assert!(s.check(&mut c, &e).is_ok());
    }

    #[test]
    fn missing_target_rejected() {
        let s = stage();
        let mut repair = RepairStateMachine::default();
        let mut c = ctx(&mut repair);
        let e = event_for("report.done", None, None);
        let err = s.check(&mut c, &e).unwrap_err();
        assert_eq!(err.reason_code, "terminal_target_missing");
    }

    #[test]
    fn source_without_target_is_rejected() {
        let s = stage();
        let mut repair = RepairStateMachine::default();
        let mut c = ctx(&mut repair);
        let mut e = Event::new("report.done", "{}");
        e.source = Some(HatId::new("reporter"));
        let err = s.check(&mut c, &e).unwrap_err();
        assert_eq!(err.reason_code, "terminal_target_missing");
    }

    // --- U9: new coverage tests (5-source priority + non-JSON fallthrough) ---

    /// `triggered` key carries the wrong target → mismatch, not missing.
    #[test]
    fn triggered_field_carries_wrong_target_rejected() {
        let s = stage();
        let mut repair = RepairStateMachine::default();
        let mut c = ctx(&mut repair);
        let e = event_for("report.done", None, Some("executor"));
        let err = s.check(&mut c, &e).unwrap_err();
        assert_eq!(err.reason_code, "terminal_target_mismatch");
    }

    /// Non-JSON payload: `extract_payload_target` returns None, falling
    /// through to "terminal_target_missing" — no panic.
    #[test]
    fn non_json_payload_falls_through_to_terminal_target_missing() {
        let s = stage();
        let mut repair = RepairStateMachine::default();
        let mut c = ctx(&mut repair);
        let e = Event::new("report.done", "not-json");
        let err = s.check(&mut c, &e).unwrap_err();
        assert_eq!(err.reason_code, "terminal_target_missing");
    }

    /// JSON array payload is valid JSON but not an object → None from
    /// `extract_payload_target` → falls through to "terminal_target_missing".
    #[test]
    fn json_array_payload_falls_through_to_terminal_target_missing() {
        let s = stage();
        let mut repair = RepairStateMachine::default();
        let mut c = ctx(&mut repair);
        let e = Event::new("report.done", "[\"executor\"]");
        let err = s.check(&mut c, &e).unwrap_err();
        assert_eq!(err.reason_code, "terminal_target_missing");
    }

    // --- PMI-001 (post-merge-converge / TS-01): empty-string
    //     `required_target_hat` is a silent fail-open across three
    //     layers (wiring, guard, CLI). Only the lint surfaces a signal,
    //     and it is Warn (non-blocking). Post-fix at least one layer
    //     must fail-closed. Today all three runtime layers pass the
    //     empty string silently — this test is the reproducer's stable
    //     failing automation for PMI-001.

    /// Build a minimal config with `required_target_hat = ""` for
    /// `report.done`, mirroring `target_routing_tests::minimal_config`
    /// so the lint layer can resolve a valid handoff consumer.
    fn pmi001_empty_target_hat_config() -> crate::config::RalphConfig {
        use crate::config::{
            EventLoopConfig, EventPolicyConfig, EventSchema, HatConfig, HatExecutionMode,
            RalphConfig,
        };
        let mut hats = std::collections::HashMap::new();
        hats.insert(
            "executor".to_string(),
            HatConfig {
                name: "Executor".to_string(),
                triggers: vec!["work.start".to_string()],
                publishes: vec!["report.done".to_string()],
                ..Default::default()
            },
        );
        hats.insert(
            "reporter".to_string(),
            HatConfig {
                name: "Reporter".to_string(),
                triggers: vec!["report.done".to_string()],
                publishes: vec![],
                ..Default::default()
            },
        );
        // Coordinator mode forces HandoffIndex consumer lookups to None,
        // breaking the routing check. Set Isolated so the lint can
        // derive a consumer (matches `target_routing_tests::minimal_config`).
        let mut event_loop = EventLoopConfig::default();
        event_loop.execution_mode = HatExecutionMode::Isolated;
        let mut policy = EventPolicyConfig::default();
        let mut schema = EventSchema::default();
        schema.required_target_hat = Some(String::new()); // the literal empty string
        policy.schemas.insert("report.done".to_string(), schema);
        event_loop.event_policy = Some(policy);
        RalphConfig {
            hats,
            event_loop,
            ..Default::default()
        }
    }

    /// PMI-001 repro (TS-01): empty-string `required_target_hat` must
    /// fail-closed at SOME layer (parse-time, wiring, guard, or lint).
    /// Today all three probeable layers pass the empty string silently:
    /// wiring drops the entry, the guard sees no contract and short-circuits
    /// to `Ok(())`, and the lint surfaces only a Warn finding. The fix may
    /// route through any of the four paths — this test validates the
    /// system-level invariant regardless of which layer is hardened.
    #[test]
    fn pmi001_empty_string_required_target_hat_fails_closed_at_some_layer() {
        use crate::event_loop::flow_wiring::build_terminal_target_contracts_from_loop_config;
        use crate::preset_lint::target_routing::check_target_routing;

        let config = pmi001_empty_target_hat_config();

        // Layer 2 (wiring): post-fix must NOT silently drop empty-string.
        let contracts = build_terminal_target_contracts_from_loop_config(&config.event_loop);
        let wiring_fail_closed = contracts.contains_key("report.done");

        // Layer 3 (guard): post-fix must reject wrong-target terminal emit.
        let guard = TerminalTargetGuardStage::new(contracts);
        let mut repair = RepairStateMachine::default();
        let mut c = StageContext::for_test_machine(
            FlowStep::new("unit_loop"),
            "loop-1",
            1,
            &mut repair,
        );
        let event = Event::new("report.done", r#"{"target_hat":"executor"}"#);
        let guard_fail_closed = guard.check(&mut c, &event).is_err();

        // Layer 4 (lint): post-fix must escalate to Error severity.
        let findings = check_target_routing(&config);
        let lint_fail_closed = findings.iter().any(|f| {
            f.id == crate::preset_lint::finding_id::FINDING_TERMINAL_TARGET_CONTRACT_EMPTY_STRING
                && f.severity == crate::preset_lint::LintSeverity::Error
        });

        // POST-FIX: at least one of the three runtime probeable layers
        // must fail-closed. Today all three are silent — this assertion
        // FAILS, demonstrating that the bug is present and reproducible.
        assert!(
            wiring_fail_closed || guard_fail_closed || lint_fail_closed,
            "PMI-001 silent fail-open: empty-string `required_target_hat` \
             must fail-closed at SOME layer (wiring, guard, or lint). \
             Observed all three layers silent: wiring_fail_closed={}, \
             guard_fail_closed={}, lint_fail_closed={}",
            wiring_fail_closed, guard_fail_closed, lint_fail_closed,
        );
    }
}
