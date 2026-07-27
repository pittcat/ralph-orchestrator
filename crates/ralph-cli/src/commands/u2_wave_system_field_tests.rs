//! 2026-07-27-004 plan U2 (R5-R7): wave worker payload system
//! fields are runtime-owned. Pure normalisation tests verify
//! the helper handles:
//! - non-wave payloads pass through unchanged (R7)
//! - non-object wave worker payloads are rejected
//! - explicit `wave_id` field conflicting with the registry-bound
//!   context is rejected with `system_field_owned_by_runtime`
//! - explicit `wave_id` field matching the registry-bound context
//!   is accepted (R6 scope is the conflict, not the redundant
//!   stamp)
//! - bare business fields are augmented with `wave_id` and
//!   `slot_index` from the registry-bound context

#[cfg(test)]
mod tests {
    use crate::commands::emit::normalize_wave_worker_system_fields;
    use serde_json::json;

    /// R7 / S6: a non-wave hat emit must NOT receive wave_id /
    /// slot_index injection. The helper passes through the
    /// payload untouched so the existing `topic` / `task_id` /
    /// business field contract keeps working.
    #[test]
    fn u2_non_wave_payload_passes_through_unchanged() {
        let input = json!({
            "task_id": "task-abc",
            "dimension": "default"
        });
        let out =
            normalize_wave_worker_system_fields(input.clone(), false, Some("w-rs-1"), Some(0))
                .expect("non-wave must be passthrough");
        assert_eq!(out, input);
    }

    /// R5 / S4: a bare-business payload from a registry-bound wave
    /// worker is augmented with the runtime-injected system
    /// fields. The caller passes only the business fields; the
    /// helper stamps them.
    #[test]
    fn u2_wave_worker_bare_payload_gets_system_fields_injected() {
        let input = json!({"content_hash": "hash-1"});
        let out = normalize_wave_worker_system_fields(input, true, Some("w-rs-u2-s4"), Some(2))
            .expect("bare injection must succeed");
        let obj = out.as_object().expect("object payload");
        assert_eq!(
            obj.get("wave_id").and_then(|v| v.as_str()),
            Some("w-rs-u2-s4")
        );
        assert_eq!(obj.get("slot_index").and_then(|v| v.as_u64()), Some(2));
        assert_eq!(
            obj.get("content_hash").and_then(|v| v.as_str()),
            Some("hash-1")
        );
    }

    /// R6 / S5: an explicit `wave_id` field that DISAGREES with
    /// the registry-bound context is rejected with the stable
    /// `system_field_owned_by_runtime` reason. The runtime owns
    /// this field full-stop; conflicting stamps are a forgery
    /// attempt and must fail closed.
    ///
    /// A matching `wave_id` is accepted: workers that include the
    /// runtime-bound identity alongside their business payload
    /// (e.g. dispatcher-signed channels that emit
    /// `{"wave_id":"<env>","slot_index":<env>,...}`) must keep
    /// passing — they are NOT a contract violation, just a
    /// redundant field that the runtime overwrites with the
    /// same canonical value (R6 / D8 scope: conflict, not
    /// every stamp).
    #[test]
    fn u2_conflicting_wave_id_is_rejected() {
        let input = json!({"wave_id": "w-rs-WRONG", "content_hash": "h"});
        let err = normalize_wave_worker_system_fields(input, true, Some("w-rs-1"), Some(0))
            .expect_err("conflicting wave_id must be rejected");
        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("system_field_owned_by_runtime"),
            "must surface the stable reason; got {rendered}"
        );
        assert!(
            rendered.contains("wave_id"),
            "must mention the conflicting field; got {rendered}"
        );
    }

    /// R6 / D8: a matching `wave_id` field is accepted; the
    /// runtime still overwrites it with the same canonical value
    /// (idempotent overwrite). Covers the dispatcher-signed
    /// channel regression path.
    #[test]
    fn u2_matching_wave_id_is_accepted() {
        let input = json!({"wave_id": "w-rs-1", "slot_index": 0, "content_hash": "h"});
        let out = normalize_wave_worker_system_fields(input, true, Some("w-rs-1"), Some(0))
            .expect("matching wave_id must be accepted");
        let obj = out.as_object().expect("object");
        assert_eq!(obj.get("wave_id").and_then(|v| v.as_str()), Some("w-rs-1"));
        assert_eq!(obj.get("slot_index").and_then(|v| v.as_u64()), Some(0));
    }

    /// R6 / D8: a `slot_index` value that DISAGREES with the
    /// registry-bound context is rejected. The matching-value
    /// case is symmetric to `u2_matching_wave_id_is_accepted`
    /// above — same accept-then-overwrite semantics.
    #[test]
    fn u2_conflicting_slot_index_is_rejected() {
        let input = json!({"slot_index": 7, "content_hash": "h"});
        let err = normalize_wave_worker_system_fields(input, true, Some("w-rs-1"), Some(0))
            .expect_err("conflicting slot_index must be rejected");
        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("system_field_owned_by_runtime"),
            "must surface the stable reason; got {rendered}"
        );
        assert!(
            rendered.contains("slot_index"),
            "must mention the conflicting field; got {rendered}"
        );
    }

    /// Handshake integrity: when `RALPH_WAVE_WORKER=1` but the
    /// wave-id env is missing (the upstream handshake gate did
    /// not fire), the helper passes the payload through
    /// unchanged so the upstream gate's "incomplete binding"
    /// error still surfaces. This preserves the layering: the
    /// env-completeness gate runs upstream of system-field
    /// injection.
    #[test]
    fn u2_wave_worker_with_missing_handshake_passes_through() {
        let input = json!({"content_hash": "h"});
        let out = normalize_wave_worker_system_fields(input.clone(), true, None, None)
            .expect("missing handshake must pass through unchanged");
        assert_eq!(out, input);
    }

    /// Edge: a wave worker payload that is NOT a JSON object (e.g.
    /// a string payload) is rejected with a stable reason. The
    /// schema validator would reject it later anyway; we fail-closed
    /// here to surface the issue closer to the emit boundary.
    #[test]
    fn u2_wave_worker_string_payload_is_rejected() {
        let input = json!("just a string");
        let err = normalize_wave_worker_system_fields(input, true, Some("w-rs-1"), Some(0))
            .expect_err("non-object payload must be rejected");
        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("system_field_owned_by_runtime"),
            "must surface the stable reason; got {rendered}"
        );
        assert!(
            rendered.contains("string"),
            "must describe the payload kind; got {rendered}"
        );
    }
}
