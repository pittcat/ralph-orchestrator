//! Schema-view rendering for `ralph emit --schema <TOPIC>` (U5 / R6).
//!
//! The view is a JSON-serialisable snapshot of the embedded protocol
//! SSOT for one topic. Operators and agents use it to verify:
//!   * which fields the gate will require for `TOPIC`
//!   * the stable protocol hash, so a drift between the authoring
//!     `presets/schemas/<name>.yml` and the embedded copy is detectable
//!     without rebuilding
//!
//! The submodule lives at `commands::emit::schema_view` (path-contract
//! preserved verbatim) so callers keep the historical import shape:
//!   `use crate::commands::emit::schema_view;`
//!
//! Lifted verbatim from `commands/emit.rs` lines 6535-6652 of HEAD
//! `7909f159`. Behaviour is identical.

use anyhow::{Context, Result};
use ralph_core::preset::engine::ProtocolView;
use ralph_core::preset::engine::protocol::payload_field_set;
use std::collections::BTreeMap;

/// Render the protocol JSON view for `topic`.
///
/// `topic` may be a topic that is *not* in the protocol — the
/// returned `required_fields` will simply be empty and the
/// other sections (`verdict_gate`, `workflow_contract`, ...)
/// remain populated so operators can see the protocol-wide
/// settings without changing the gate behaviour.
pub fn render_topic(view: &ProtocolView, topic: &str) -> Result<serde_json::Value> {
    let mut payload_keys: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for (t, schema) in &view.effective_required_fields {
        // Per-topic entry keeps the SSOT visible when an operator
        // inspects multiple topics at once. The serialised `schema`
        // is the *embedded* copy, post build.rs merge.
        let fields_vec: Vec<&String> = {
            let mut v: Vec<&String> = schema.iter().collect();
            v.sort();
            v
        };
        payload_keys.insert(
            t.clone(),
            serde_json::json!({
                "required_fields": fields_vec,
            }),
        );
    }

    // Topic-scoped required fields (empty when topic is unknown).
    let required_fields: Vec<String> = {
        let mut v: Vec<String> = view.required_fields(topic).into_iter().collect();
        v.sort();
        v
    };

    let mut out = serde_json::json!({
        "topic": topic,
        "protocol_hash": view.protocol_hash,
        "is_macro_edge": false, // kept for backwards compatibility; macro-edge semantics removed
        "required_fields": required_fields,
        "all_topics": payload_keys,
    });

    // Protocol-wide sections. Each is `null` when absent so the
    // operator can see at a glance whether the loaded config
    // enables the corresponding gate / projection machinery.
    let obj = out.as_object_mut().expect("json!() returns object");

    if let Some(vg) = &view.verdict_gate {
        obj.insert(
            "verdict_gate".to_string(),
            serde_json::to_value(vg).context("serialise verdict_gate")?,
        );
    } else {
        obj.insert("verdict_gate".to_string(), serde_json::Value::Null);
    }

    if let Some(wc) = &view.workflow_contract {
        obj.insert(
            "workflow_contract".to_string(),
            serde_json::to_value(wc).context("serialise workflow_contract")?,
        );
    } else {
        obj.insert("workflow_contract".to_string(), serde_json::Value::Null);
    }

    if let Some(sp) = &view.state_projection {
        obj.insert(
            "state_projection".to_string(),
            serde_json::to_value(sp).context("serialise state_projection")?,
        );
    } else {
        obj.insert("state_projection".to_string(), serde_json::Value::Null);
    }

    if let Some(ec) = &view.execution_contracts {
        obj.insert(
            "execution_contracts".to_string(),
            serde_json::to_value(ec).context("serialise execution_contracts")?,
        );
    } else {
        obj.insert("execution_contracts".to_string(), serde_json::Value::Null);
    }

    Ok(out)
}

/// Pretty-printed variant for human reading. Uses 2-space indent
/// to match the project's other JSON dumps (`recovery.jsonl`
/// envelopes, `protocol_view` debug output).
pub fn render_pretty(view: &ProtocolView, topic: &str) -> Result<String> {
    let value = render_topic(view, topic)?;
    Ok(serde_json::to_string_pretty(&value)?)
}

// Re-export for tests that want to introspect the view without
// going through the rendered JSON.
#[allow(dead_code)]
pub(crate) fn topic_field_set(
    view: &ProtocolView,
    topic: &str,
) -> std::collections::HashSet<String> {
    view.required_fields(topic)
}

// Silence the unused import warning when `payload_field_set` is
// not referenced from tests (kept for future schema-aware payload
// introspection helpers, e.g. "show which fields an event with
// this shape would pass / fail").
#[allow(dead_code)]
fn _unused_payload_field_set(payload: &serde_json::Value) -> std::collections::HashSet<String> {
    payload_field_set(payload)
}
