//! U5 (R6) pure-function unit tests: text and json mode output shapes are
//! pinned separately.
//!
//! Lifted verbatim from `commands/emit.rs` lines 800-843 of HEAD `7909f159`.

use super::command_impl::format_emit_reject_summary;

#[test]
fn test_format_text_mode_emits_rejected_with_code() {
    let line = format_emit_reject_summary(
        false,
        "cwd_workspace_drift",
        "current_dir=/x workspace_root=/y",
    )
    .expect("text mode always returns Some");
    assert_eq!(
        line,
        "emit rejected [cwd_workspace_drift]: current_dir=/x workspace_root=/y"
    );
}

#[test]
fn test_format_json_mode_is_valid_envelope() {
    let line = format_emit_reject_summary(true, "path_resolution_failed", "not in allowlist")
        .expect("json mode always returns Some");
    let parsed: serde_json::Value =
        serde_json::from_str(&line).expect("json mode must produce valid JSON");
    assert_eq!(
        parsed.get("emit_rejected"),
        Some(&serde_json::Value::Bool(true))
    );
    assert_eq!(
        parsed.get("code"),
        Some(&serde_json::Value::String(
            "path_resolution_failed".to_string()
        ))
    );
    let detail = parsed
        .get("detail")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        detail.contains("not in allowlist"),
        "detail should preserve message text, got: {detail}"
    );
}
