//! Hard emit-time schema gate (U1).
//!
//! Why this exists: prior `event_loop::policy` performed "soft" checks
//! (drift-style auditing) only. That left the runtime free to accept
//! `plan.blocked` events with an empty `reason`, `task.resume` with a
//! missing `kind`, etc. See the diagnosis report
//! `2026-06-27-ce-executor-serial-2026-06-26-001-...` for the full
//! failure chain. This module is the pure-logic core that the
//! `EmitSchemaGateStage` (U6) wraps.
//!
//! Cross-platform / concurrency semantics: pure CPU only — no FS, no
//! threading, no async. The same input always yields the same output.
//!
//! # Example
//!
//! ```
//! use ralph_core::event_loop::emit_schema_gate::{check, EmitDecision};
//!
//! let payload = serde_json::json!({"reason": "unit_failed"});
//! let required = vec!["reason".to_string()];
//! assert!(matches!(check(&payload, &required), EmitDecision::Accept));
//!
//! let bad = serde_json::json!({});
//! let decision = check(&bad, &required);
//! assert!(matches!(decision, EmitDecision::Reject(ref f) if f == &vec!["reason".to_string()]));
//! ```

use serde_json::Value;

/// Result of an emit-time schema check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmitDecision {
    /// All required fields are present and non-null.
    Accept,
    /// One or more required fields are missing or null. The inner
    /// vector is the list of missing field names, in the order they
    /// were declared in `required`.
    Reject(Vec<String>),
}

/// Hard schema check: every entry in `required` must be present in
/// `payload` and non-null. An empty `required` list always accepts.
///
/// - A non-object payload is rejected with the single synthetic
///   field `__payload_must_be_object` so downstream code can
///   report it without a separate enum variant.
/// - A present-but-`null` field is treated as missing — the same
///   rule the drift engine used in 2026-06-26.
pub fn check(payload: &Value, required: &[String]) -> EmitDecision {
    let Some(obj) = payload.as_object() else {
        return EmitDecision::Reject(vec!["__payload_must_be_object".to_string()]);
    };

    let missing: Vec<String> = required
        .iter()
        .filter(|name| match obj.get(*name) {
            None => true,
            Some(v) => v.is_null(),
        })
        .cloned()
        .collect();

    if missing.is_empty() {
        EmitDecision::Accept
    } else {
        EmitDecision::Reject(missing)
    }
}

#[cfg(test)]
mod tests;
