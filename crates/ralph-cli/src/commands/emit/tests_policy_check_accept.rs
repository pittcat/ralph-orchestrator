//! U7 tests: CLI policy-check acceptance → stdout EmitResult (JSON).
//!
//! Acceptance criteria:
//! 1. `--policy-check --output json` on a payload that passes the
//!    unified pipeline prints `EmitResult { ok: true, recorded: false,
//!    ... }`.
//! 2. The handler exits 0 (dry-run, not written to disk).
//!
//! Lifted verbatim from `commands/emit.rs` lines 6135-6309 of HEAD
//! `7909f159`. Behaviour is identical.

use super::EmitArgs;
use crate::cli::{ColorMode, ConfigSource};
use std::path::PathBuf;

const POLICY_CHECK_ACCEPT_YAML: &str = r"
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
fn test_policy_check_accept_json_recorded_false() {
    let workspace = tempfile::TempDir::new()
        .expect("temp dir")
        .path()
        .to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
    std::fs::write(workspace.join("ralph.yml"), POLICY_CHECK_ACCEPT_YAML).expect("ralph.yml");

    let args = EmitArgs {
        topic: Some("work.done".to_string()),
        payload: r#"{"plan_name":"my-plan","task_id":"task-1"}"#.to_string(),
        json: true,
        file: PathBuf::from(".ralph/events.jsonl"),
        policy_check: true,
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
        "policy-check acceptance must yield Ok, got: {result:?}"
    );

    // The events file must NOT exist — `--policy-check` is a dry-run.
    let events_file = workspace.join(".ralph/events.jsonl");
    assert!(
        !events_file.exists() || std::fs::read_to_string(&events_file).unwrap().is_empty(),
        "policy-check dry-run must not write to events.jsonl, found: {}",
        events_file.display()
    );
}

#[test]
fn test_policy_check_accept_json_includes_phase_and_allowed_next() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let workspace = temp.path();

    let ralph_yml = r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    business_topics:
      - work.done
    schemas:
      work.done:
        required_fields:
          - task_id
  mechanism:
    phase_authority:
      enabled: true
      initial_phase: unit_loop
      phases:
        - id: unit_loop
          allowed_emits:
            coordinator:
              - work.ready
              - work.done
";
    std::fs::write(workspace.join("ralph.yml"), ralph_yml).expect("write ralph.yml");
    std::fs::create_dir_all(workspace.join(".ralph")).expect(".ralph dir");
    std::fs::write(
        workspace.join(".ralph/hats.yml"),
        r"
hats:
  coordinator:
    publishes:
      - work.done
      - work.ready
",
    )
    .expect("write hats.yml");

    let config = crate::preflight::load_config_for_preflight_sync(
        &[ConfigSource::File(workspace.join("ralph.yml"))],
        None,
        workspace,
    )
    .expect("load config");
    let routing = ralph_core::emit_result::resolve_emit_routing_from_config(
        Some(&config),
        workspace,
        Some("coordinator"),
    );
    assert_eq!(routing.phase, "unit_loop");
    assert!(routing.allowed_next.contains(&"work.ready".to_string()));
    assert!(routing.allowed_next.contains(&"work.done".to_string()));

    let parts = crate::policy_check::build_emit_result_parts(
        "work.done".to_string(),
        true,
        false,
        Vec::new(),
        Some(&config),
        workspace,
        Some("coordinator"),
        None,
        // U2: this unit test does not exercise the
        // handoff_envelope summary path — pass `None`
        // for parity with the production rejection path.
        None,
    );
    let result = ralph_core::emit_result::EmitResult::assemble(parts);
    assert_eq!(result.phase, "unit_loop");
    assert!(result.allowed_next.contains(&"work.ready".to_string()));
}
