//! `EmitSchemaGateStage` — wraps the U1 `emit_schema_gate`
//! pure-logic check in an `EmitStage` (U6).
//!
//! Why this stage lives between `RepairDispatch` and
//! `FlowStepScope`: the schema check is a *type-level* gate —
//! it does not care which step the event belongs to. Putting
//! it before the flow-scope check means a missing-field event
//! is rejected with a stable `missing_required_fields` reason
//! code regardless of which step the hat thought it was in.
//!
//! Cross-platform / concurrency semantics: pure CPU. No FS,
//! no threading. The same event + the same `required_fields`
//! always produces the same decision.

use crate::event_loop::emit_schema_gate::{check as check_payload, EmitDecision};
use crate::event_loop::stage_pipeline::{EmitStage, StageContext, StageReject};
use ralph_proto::Event;
use serde_json::Value;
use std::collections::HashMap;

/// Default per-topic `required_fields` table used when the
/// preset does not ship one. Mirrors the schema entries in
/// `presets/schemas/ce-executor-serial.yml` so the stage is
/// useful out of the box; U10 keeps the SSOT in lockstep.
pub fn default_required_fields() -> HashMap<&'static str, Vec<&'static str>> {
    let mut map: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
    map.insert("plan.blocked", vec!["reason"]);
    map.insert("task.resume", vec!["reason", "target_hat", "kind"]);
    map.insert("human.guidance", vec!["message"]);
    map.insert("work.done", vec!["task_id"]);
    map.insert("work.failed", vec!["task_id", "reason"]);
    map.insert("test.passed", vec!["task_id"]);
    map.insert("test.failed", vec!["task_id", "reason"]);
    map.insert("fix.applied", vec!["task_id"]);
    map.insert("fix.exhausted", vec!["task_id", "reason"]);
    map.insert("review.start", vec!["plan_id"]);
    map.insert("review.complete", vec!["fix_plan_file", "verdict"]);
    map.insert("plan.complete", vec![]);
    map
}

/// Hard gate that rejects events whose payload is missing one
/// of the topic's required fields.
pub struct EmitSchemaGateStage {
    /// Topic → required fields. Missing topics are accepted
    /// (no required fields → no constraint). This is the
    /// safety property: topics the operator has not opted
    /// into are not blocked.
    required: HashMap<String, Vec<String>>,
}

impl EmitSchemaGateStage {
    /// Build the stage from a topic → required-fields table.
    pub fn new(required: HashMap<String, Vec<String>>) -> Self {
        Self { required }
    }

    /// Build the stage from `default_required_fields` —
    /// useful for tests and as a fallback when a preset
    /// ships no schema at all.
    pub fn with_defaults() -> Self {
        let required = default_required_fields()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.into_iter().map(String::from).collect()))
            .collect();
        Self::new(required)
    }
}

impl EmitStage for EmitSchemaGateStage {
    fn name(&self) -> &'static str {
        "EmitSchemaGate"
    }

    fn check(&self, _ctx: &StageContext, event: &Event) -> Result<(), StageReject> {
        let required = match self.required.get(event.topic.as_str()) {
            Some(req) if !req.is_empty() => req,
            _ => return Ok(()), // no schema = no gate.
        };

        let payload: Value = match serde_json::from_str(event.payload.as_str()) {
            Ok(v) => v,
            // Malformed JSON is treated as "no fields present",
            // which is the same as missing every required field.
            // The recovery envelope is the consumer's signal.
            Err(_) => {
                return Err(StageReject::new(self.name(), "missing_required_fields")
                    .with_missing_fields(required.clone()));
            }
        };

        match check_payload(&payload, required) {
            EmitDecision::Accept => Ok(()),
            EmitDecision::Reject(missing) => {
                Err(StageReject::new(self.name(), "missing_required_fields")
                    .with_missing_fields(missing))
            }
        }
    }
}

#[cfg(test)]
mod tests;
