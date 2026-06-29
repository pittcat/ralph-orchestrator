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

use crate::event_loop::emit_schema_gate::{EmitDecision, check as check_payload};
use crate::event_loop::stage_pipeline::{EmitStage, StageContext, StageReject};
use ralph_proto::Event;
use serde_json::Value;
use std::collections::HashMap;

/// Default per-topic `required_fields` table used when the
/// preset does not ship one. Mirrors the schema entries in
/// `presets/schemas/ce-executor-serial.yml` so the stage is
/// useful out of the box; U10 keeps the SSOT in lockstep.
///
/// IMPORTANT: this table is the **generic baseline** gate —
/// it covers only topics whose payload contract is fixed
/// across ALL presets (e.g. `work.*`, `test.*`, `fix.*`,
/// `plan.blocked`, `task.resume`, `review.start`).
/// Preset-specific contracts like `review.complete` (which
/// requires `fix_plan_file`, `verdict`, `findings_count`, …
/// in `ce-executor-serial` but is a free-form topic in the
/// harness FR-1 integration tests) MUST be injected via
/// `EmitSchemaGateStage::new(preset_required_fields)` so the
/// baseline stays permissive for legacy / harness fixtures.
///
/// Note (2026-06-28-005): `human.guidance` was removed from
/// this baseline because the topic itself was deleted
/// (no external operator channel). The corresponding
/// `field_completeness` R13 bypass in
/// `drift::detector::check_field_completeness` is now
/// safe to drop as well (see plan 2026-06-28-005 U1).
pub fn default_required_fields() -> HashMap<&'static str, Vec<&'static str>> {
    let mut map: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
    map.insert("plan.blocked", vec!["reason"]);
    map.insert("task.resume", vec!["reason", "target_hat", "kind"]);
    map.insert("work.done", vec!["task_id"]);
    map.insert("work.failed", vec!["task_id", "reason"]);
    map.insert("test.passed", vec!["task_id"]);
    map.insert("test.failed", vec!["task_id", "reason"]);
    map.insert("fix.applied", vec!["task_id"]);
    map.insert("fix.exhausted", vec!["task_id", "reason"]);
    map.insert("review.start", vec!["plan_id"]);
    map.insert("plan.complete", vec![]);
    map
}

/// Merge generic baseline required-fields with the preset's
/// `ProtocolView` (same SSOT as the engine gate / preset_lint).
///
/// Preset-specific topics such as `review.complete` override the
/// baseline when the operator has declared `event_policy.schemas`.
pub(crate) fn required_fields_from_loop_config(
    config: &crate::config::EventLoopConfig,
) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = default_required_fields()
        .into_iter()
        .map(|(topic, fields)| {
            (
                topic.to_string(),
                fields.into_iter().map(String::from).collect(),
            )
        })
        .collect();

    let view = crate::preset::engine::ProtocolView::from_event_loop(config);
    for (topic, fields) in &view.effective_required_fields {
        if fields.is_empty() {
            continue;
        }
        map.insert(topic.clone(), fields.iter().cloned().collect());
    }
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

    fn check(&self, _ctx: &mut StageContext, event: &Event) -> Result<(), StageReject> {
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
