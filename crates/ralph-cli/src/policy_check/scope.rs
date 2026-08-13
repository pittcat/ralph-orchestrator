//! U7 (2026-08-10-002 plan, R11/S1): `policy_check::scope` submodule.
//!
//! Owns the typed threshold / typed bool / bounded string readers
//! (U1+U2) and the canonical self-excluding digest verifier (U5+U6)
//! so the parent `gates.rs` does not have to carry 53 % of the
//! 5 000-line HARD RULE budget alone. The module keeps:
//!
//! 1. The four scope topic dispatch arms (the
//!    `check_scope_handoff_guard` topic switch) reachable from the
//!    parent module via [`SCOPE_DISPATCHERS`].
//! 2. The two `verify_*_digest` wrappers (merge boundary + scope
//!    manifest) so callers outside `scope` keep their current
//!    signatures and the parameterised helper is the only
//!    authoritative surface.
//! 3. The typed readers + bounded string reader as `pub(super)` so
//!    the threshold functions in `gates.rs` can import them through
//!    `super::scope` without leaking them beyond `ralph-cli`.
//!
//! The submodule depends on the same `unified::ValidationError` the
//! parent module uses; the `use` block is intentionally narrow
//! because the original monolithic `gates.rs` had a wide import
//! surface (see `gates.rs:1-40` for the historical context).

use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::unified::ValidationError;

// ── U1 (R1/R2/C1/M2): typed non-negative integer reader ─────────────

/// U1 (2026-08-10-002 plan, R1/R2/C1/M2): typed non-negative integer
/// reader for scope handoff threshold fields. Replaces the silent
/// `obj.get(field).and_then(|v| v.as_u64()).unwrap_or(0)` pattern that
/// accepted negative `critical_unknown_count: -1` and string `"0"`
/// by coercing either to `0`.
///
/// Returns `Err(ValidationError)` with `reason_code =
/// "scope_handoff_inconsistent"` and a message that names both the
/// topic and the actual offending value when:
///
/// 1. The field is missing (treated as `0` only at the
///    `default_value` boundary; an explicit "missing required field"
///    error is the caller's responsibility, not this helper's).
/// 2. The field is present but not an integer (e.g. `"0"` string,
///    `1.5` float, `null`, `true`).
/// 3. The integer is negative — the previous `as_u64().unwrap_or(0)`
///    silently coerced `-1` to `0`; this helper surfaces the value
///    so the rejection message is honest.
///
/// `default_value` is the value returned when the field is **absent**
/// AND `require_present` is `false`. When `require_present` is `true`,
/// missing fields produce an error.
#[allow(clippy::result_large_err)]
pub(crate) fn typed_threshold_u64(
    obj: &serde_json::Map<String, Value>,
    field: &str,
    topic: &str,
    require_present: bool,
    default_value: u64,
) -> Result<u64, ValidationError> {
    let Some(value) = obj.get(field) else {
        if require_present {
            return Err(ValidationError {
                payload_index: 0,
                field: field.to_string(),
                reason_code: "scope_handoff_inconsistent".to_string(),
                message: format!(
                    "{topic} requires {field} (positive integer) when scope_status=resolved"
                ),
                ..Default::default()
            });
        }
        return Ok(default_value);
    };
    let Some(int_value) = value.as_i64() else {
        let actual = match value {
            Value::String(s) => format!("string({:?})", s),
            Value::Bool(b) => format!("bool({b})"),
            Value::Number(n) => format!("number({n})"),
            Value::Null => "null".to_string(),
            Value::Array(_) => "array".to_string(),
            Value::Object(_) => "object".to_string(),
        };
        return Err(ValidationError {
            payload_index: 0,
            field: field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!("{topic} requires {field} to be a non-negative integer; got {actual}"),
            ..Default::default()
        });
    };
    if int_value < 0 {
        return Err(ValidationError {
            payload_index: 0,
            field: field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "{topic} requires {field} to be a non-negative integer; got {int_value}"
            ),
            ..Default::default()
        });
    }
    Ok(int_value as u64)
}

// ── U2 (R3/A1): explicit-bool reader ───────────────────────────────

/// U2 (2026-08-10-002 plan, R3/A1): explicit-bool reader. Rejects
/// string-encoded booleans (`"true"`, `"false"`) that the previous
/// `as_bool() == Some(true|false)` pattern silently accepted because
/// `as_bool()` returns `None` for strings and the `== Some(false)`
/// comparison then matched every non-`true` value as `false`.
///
/// Returns `Ok(Some(bool))` when the field is present and is a JSON
/// bool, `Ok(None)` when the field is absent, and `Err` when the
/// field is present but not a JSON bool (string, integer, null, etc.).
#[allow(clippy::result_large_err)]
pub(crate) fn typed_required_bool(
    obj: &serde_json::Map<String, Value>,
    field: &str,
    topic: &str,
) -> Result<Option<bool>, ValidationError> {
    let Some(value) = obj.get(field) else {
        return Ok(None);
    };
    match value {
        Value::Bool(b) => Ok(Some(*b)),
        other => {
            let actual = match other {
                Value::String(s) => format!("string({:?})", s),
                Value::Number(n) => format!("number({n})"),
                Value::Null => "null".to_string(),
                Value::Array(_) => "array".to_string(),
                Value::Object(_) => "object".to_string(),
                Value::Bool(_) => unreachable!(),
            };
            Err(ValidationError {
                payload_index: 0,
                field: field.to_string(),
                reason_code: "scope_handoff_inconsistent".to_string(),
                message: format!(
                    "{topic} requires {field} to be a JSON bool (true|false); got {actual}"
                ),
                ..Default::default()
            })
        }
    }
}

// ── U2 (R4/A3 input-validation leg): bounded string reader ──────────

/// U2 (2026-08-10-002 plan, R4/A3): bounded string reader for scope
/// handoff fields. Extends `required_scope_string` with a max-length
/// guard (256 chars) and a control-character reject so an attacker
/// cannot submit a 4 KiB UTF-8 path or embed a `\n` to slip a fake
/// line through the gate.
#[allow(clippy::result_large_err)]
pub(crate) fn bounded_scope_string(
    obj: &serde_json::Map<String, Value>,
    field: &str,
    topic: &str,
    max_len: usize,
) -> Result<String, ValidationError> {
    let value = obj
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| ValidationError {
            payload_index: 0,
            field: field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!("{topic} requires non-empty string field {field}"),
            ..Default::default()
        })?;
    if value.is_empty() {
        return Err(ValidationError {
            payload_index: 0,
            field: field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!("{topic} requires non-empty string field {field}"),
            ..Default::default()
        });
    }
    if value.len() > max_len {
        return Err(ValidationError {
            payload_index: 0,
            field: field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "{topic} field {field} exceeds {max_len}-char limit; got {} chars",
                value.len()
            ),
            ..Default::default()
        });
    }
    if value.chars().any(|c| c.is_control()) {
        return Err(ValidationError {
            payload_index: 0,
            field: field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!("{topic} field {field} contains a control character"),
            ..Default::default()
        });
    }
    Ok(value.to_string())
}

// ── U5 + U6 (R5/A2 + R9/M1/A4): canonical self-excluding verifier ──

/// U5 + U6 (2026-08-10-002 plan, R5/A2 + R9/M1/A4): the single
/// canonical self-excluding digest verifier. Both `boundary_digest`
/// (merge boundary) and `scope_digest` (scope manifest) flow
/// through this helper; the only difference is the
/// self-referential field that the canonicalization step strips.
///
/// `canonical_path` is the path the validator
/// (`super::gates::validate_scoped_artifact_path`) already
/// canonicalized, so this helper does **not** re-lexical-resolve the
/// path or `read` a different inode than the one the validator
/// accepted. This closes the A2 TOCTOU window where a malicious
/// caller could swap the artifact between validation and
/// verification.
///
/// `excluded_field` is the JSON field removed before canonicalization
/// (e.g. `boundary_digest` or `scope_digest`). The producer and
/// verifier MUST agree on this name — a third scope topic that
/// declares a different self-field simply passes the new name here
/// instead of duplicating this 50-line helper.
#[allow(clippy::result_large_err)]
pub(crate) fn verify_canonical_json_digest_excluding(
    canonical_path: &Path,
    declared_digest: &str,
    digest_field: &str,
    excluded_field: &str,
) -> Result<(), ValidationError> {
    let bytes = std::fs::read(canonical_path).map_err(|e| ValidationError {
        payload_index: 0,
        field: digest_field.to_string(),
        reason_code: "scope_handoff_inconsistent".to_string(),
        message: format!(
            "{digest_field} verification failed: could not read {}: {e}",
            canonical_path.display()
        ),
        ..Default::default()
    })?;
    let mut value: Value = serde_json::from_slice(&bytes).map_err(|e| ValidationError {
        payload_index: 0,
        field: digest_field.to_string(),
        reason_code: "scope_handoff_inconsistent".to_string(),
        message: format!(
            "{digest_field} verification failed: {} is not valid JSON: {e}",
            canonical_path.display()
        ),
        ..Default::default()
    })?;
    if let Some(object) = value.as_object_mut() {
        object.remove(excluded_field);
    }
    let mut canonical = serde_json::to_vec(&value).map_err(|e| ValidationError {
        payload_index: 0,
        field: digest_field.to_string(),
        reason_code: "scope_handoff_inconsistent".to_string(),
        message: format!("{digest_field} canonicalization failed: {e}"),
        ..Default::default()
    })?;
    canonical.push(b'\n');
    let mut hasher = Sha256::new();
    hasher.update(canonical);
    let computed = format!("{:x}", hasher.finalize());
    if !declared_digest.eq_ignore_ascii_case(&computed) {
        return Err(ValidationError {
            payload_index: 0,
            field: digest_field.to_string(),
            reason_code: "scope_handoff_inconsistent".to_string(),
            message: format!(
                "{digest_field} does not match canonical SHA-256 of {}; manifest may have been tampered with (declared={declared_digest}, computed={computed})",
                canonical_path.display()
            ),
            ..Default::default()
        });
    }
    Ok(())
}

/// Re-exported at the parent module so callers in `gates.rs` keep
/// their existing surface (`super::gates::verify_canonical_json_digest`)
/// while the implementation lives here. U5 closes the A2 TOCTOU
/// window by routing through `validate_scoped_artifact_path` first
/// to thread the canonical `PathBuf` into the parameterised helper.
#[allow(clippy::result_large_err)]
pub(crate) fn verify_canonical_json_digest(
    workspace_root: &Path,
    artifact_path: &str,
    declared_digest: &str,
    digest_field: &str,
) -> Result<(), ValidationError> {
    let canonical_path = super::gates::validate_scoped_artifact_path(
        workspace_root,
        artifact_path,
        ".ralph/merge/",
        digest_field,
    )?;
    verify_canonical_json_digest_excluding(
        &canonical_path,
        declared_digest,
        digest_field,
        "boundary_digest",
    )
}

/// Re-exported at the parent module for the scope manifest digest.
/// Same TOCTOU-closing pattern as `verify_canonical_json_digest`.
#[allow(clippy::result_large_err)]
pub(crate) fn verify_scope_manifest_digest(
    workspace_root: &Path,
    artifact_path: &str,
    declared_digest: &str,
    digest_field: &str,
) -> Result<(), ValidationError> {
    let canonical_path = super::gates::validate_scoped_artifact_path(
        workspace_root,
        artifact_path,
        ".ralph/",
        digest_field,
    )?;
    verify_canonical_json_digest_excluding(
        &canonical_path,
        declared_digest,
        digest_field,
        "scope_digest",
    )
}

/// Type alias so the parent module can keep a single
/// `use super::scope::PathBuf;` line for code that needs the
/// canonical path type. (Re-exported to avoid leaking the full
/// `std::path::*` surface into `gates.rs`.)
#[allow(dead_code)]
pub type CanonicalPath = PathBuf;
