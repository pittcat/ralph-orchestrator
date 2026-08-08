//! Tests for `--schema EMIT_RESULT` (the read-only EmitResult protocol-view
//! short-circuit at the top of `emit_command_with_root_and_hats`).
//!
//! Lifted verbatim from `commands/emit.rs` lines 5937-6013 of HEAD
//! `7909f159`. Behaviour is identical.

use super::EmitArgs;
use crate::cli::ColorMode;
use std::path::PathBuf;

#[test]
fn test_emit_schema_emit_result_prints_version() {
    let args = EmitArgs {
        topic: Some("work.done".to_string()),
        payload: "{}".to_string(),
        json: true,
        file: PathBuf::from(".ralph/events.jsonl"),
        policy_check: false,
        no_policy_check: false,
        hat: Some("executor".to_string()),
        triggered: None,
        source: None,
        schema: Some("EMIT_RESULT".to_string()),
        output: "text".to_string(),
        policy_check_token: None,
    };

    let workspace = tempfile::TempDir::new()
        .expect("temp dir")
        .path()
        .to_path_buf();

    super::emit_command_with_root(ColorMode::Never, args, Some(&workspace))
        .expect("EMIT_RESULT schema path must ignore payload/json");
}

#[test]
fn test_emit_schema_emit_result_mutually_exclusive_with_payload() {
    let args = EmitArgs {
        topic: Some("work.done".to_string()),
        payload: "{\"x\":1}".to_string(),
        json: true,
        file: PathBuf::from(".ralph/events.jsonl"),
        policy_check: false,
        no_policy_check: false,
        hat: Some("executor".to_string()),
        triggered: None,
        source: None,
        schema: Some("EMIT_RESULT".to_string()),
        output: "text".to_string(),
        policy_check_token: None,
    };

    let workspace = tempfile::TempDir::new()
        .expect("temp dir")
        .path()
        .to_path_buf();

    super::emit_command_with_root(ColorMode::Never, args, Some(&workspace))
        .expect("EMIT_RESULT schema path must ignore payload/json");
}
