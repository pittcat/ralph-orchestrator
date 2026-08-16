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

        // The explicit target hat comes from one of three places,
        // in priority order:
        //   1. payload.target_hat — explicit emit override.
        //   2. event.target — runtime carrier field (set by
        //      `EventReader` from the JSONL `triggered` field).
        //   3. event.source_hat — older semantic, sometimes carried
        //      alongside target.
        let payload_target = extract_payload_target(&event.payload);
        let carrier_target = event
            .target
            .as_ref()
            .map(|h| h.as_str().to_string());
        let carrier_source = event
            .source
            .as_ref()
            .map(|h| h.as_str().to_string());

        let actual = payload_target
            .or(carrier_target)
            .or(carrier_source);

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
    for key in ["target_hat", "triggered", "target"] {
        if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
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
}