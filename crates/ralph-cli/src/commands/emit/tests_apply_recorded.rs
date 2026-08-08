//! U9 tests: apply (no `--policy-check`) on a payload that passes the
//! unified pipeline prints `EmitResult { ok: true, recorded: true,
//! target_path: <absolute workspace-root .ralph/events.jsonl> }` and
//! exits 0.
//!
//! Lifted verbatim from `commands/emit.rs` lines 6324-6521 of HEAD
//! `7909f159`. Behaviour is identical.

use super::EmitArgs;
use crate::cli::ColorMode;

const APPLY_RECORDED_YAML: &str = r"
event_loop:
  execution_mode: coordinator
  event_policy:
    enabled: true
    mode: enforce
    schemas:
      work.done:
        required_fields:
          - plan_name
          - task_id
hats:
  coordinator:
    name: coordinator
    triggers: []
    publishes:
      - work.done
";

#[test]
fn test_apply_json_recorded_true() {
    let workspace = tempfile::TempDir::new()
        .expect("temp dir")
        .path()
        .to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
    std::fs::write(workspace.join("ralph.yml"), APPLY_RECORDED_YAML).expect("ralph.yml");

    let events_file = workspace.join(".ralph/events.jsonl");
    let args = EmitArgs {
        topic: Some("work.done".to_string()),
        payload: r#"{"plan_name":"my-plan","task_id":"task-1"}"#.to_string(),
        json: true,
        file: events_file.clone(),
        policy_check: false,
        no_policy_check: false,
        hat: Some("coordinator".to_string()),
        triggered: None,
        source: None,
        schema: None,
        output: "json".to_string(),
        policy_check_token: None,
    };

    let result = super::emit_command_with_root(ColorMode::Never, args, Some(&workspace));
    assert!(
        result.is_ok(),
        "apply must yield Ok on accepted payload, got: {result:?}"
    );

    assert!(
        events_file.exists(),
        "apply must write to events file at {}",
        events_file.display()
    );
}

#[test]
fn test_apply_json_emits_target_path_in_result() {
    let workspace = tempfile::TempDir::new()
        .expect("temp dir")
        .path()
        .to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
    std::fs::write(workspace.join("ralph.yml"), APPLY_RECORDED_YAML).expect("ralph.yml");

    let events_file = workspace.join(".ralph/events.jsonl");
    let args = EmitArgs {
        topic: Some("work.done".to_string()),
        payload: r#"{"plan_name":"my-plan","task_id":"task-1"}"#.to_string(),
        json: true,
        file: events_file.clone(),
        policy_check: false,
        no_policy_check: false,
        hat: Some("coordinator".to_string()),
        triggered: None,
        source: None,
        schema: None,
        output: "json".to_string(),
        policy_check_token: None,
    };

    let _ = super::emit_command_with_root(ColorMode::Never, args, Some(&workspace));

    // Re-run through the production EmitResult assembly path so we can
    // assert `recorded=true` and a non-empty absolute target_path.
    let cfg_yaml = std::fs::read_to_string(workspace.join("ralph.yml")).unwrap();
    let config: ralph_core::RalphConfig = serde_yaml::from_str(&cfg_yaml).unwrap();
    let parts = crate::policy_check::build_emit_result_parts(
        "work.done".to_string(),
        true,
        true,
        Vec::new(),
        Some(&config),
        &workspace,
        Some("coordinator"),
        Some(events_file.display().to_string()),
        None,
    );
    let result_obj = ralph_core::emit_result::EmitResult::assemble(parts);
    let json: serde_json::Value =
        serde_json::to_value(&result_obj).expect("EmitResult must serialize");
    let obj = json.as_object().expect("must be object");
    assert_eq!(obj.get("recorded"), Some(&serde_json::Value::Bool(true)));
    let target_path = obj
        .get("target_path")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        !target_path.is_empty(),
        "target_path must be non-empty for recorded=true apply, got: {target_path}"
    );
    assert!(
        target_path.ends_with(".ralph/events.jsonl"),
        "target_path must point at workspace root .ralph/events.jsonl (got: {target_path})"
    );
}
