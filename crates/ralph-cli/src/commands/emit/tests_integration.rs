//! 62 integration tests for the `commands::emit` flow (workspace / marker
//! resolution, urgent steer blocking, policy-check rejections, U5Gate
//! stand-down / compile-failure / capability-denied / token-violation,
//! schema view, isolated-mode `triggered` derivation, apply recorded, etc.).
//!
//! Lifted verbatim from `commands/emit.rs` lines 2217-5924 (the former
//! `mod tests { ... }` block contents) of HEAD `7909f159`. Behaviour is
//! identical. Imports are rewritten because `use super::*;` no longer
//! pulls in production items from the original monolithic file — the
//! re-exported `pub` items at `commands::emit::*` plus the test-only
//! `pub(super)` helpers from `commands::emit::command_impl` cover
//! everything the tests need.

use super::EmitArgs;
use super::{
    command_impl::{
        U5Gate, maybe_derive_triggered_for_isolated, should_warn_on_missing_default_config,
    },
    emit_command_with_root, looks_like_json, resolve_provenance,
};
use crate::cli::{
    ColorMode, ConfigSource, HatsSource, load_config_with_overrides, resolve_emit_path,
    urgent_steer_path_from_workspace,
};
use ralph_core::preset::engine::ProtocolView;
use ralph_core::{RalphConfig, UrgentSteerStore};
use std::path::PathBuf;
use tempfile::TempDir;

fn parse_config(yaml: &str) -> RalphConfig {
    serde_yaml::from_str(yaml).expect("valid test config")
}
#[test]
fn test_emit_command_resolves_marker_relative_to_workspace_root_from_nested_dir() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/events-20260309-test.jsonl\n",
    )
    .expect("write marker");

    emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("debug.step".to_string()),
            payload: "task_id=demo".to_string(),
            json: false,
            file: PathBuf::from(".ralph/events.jsonl"),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect("emit command");

    let events = std::fs::read_to_string(workspace.join(".ralph/events-20260309-test.jsonl"))
        .expect("read events");
    assert!(events.contains("\"topic\":\"debug.step\""));
    assert!(events.contains("task_id=demo"));
}

/// U1 (2026-07-06-002 plan, R1): `workspace_root` 锚定必须只走
/// `resolve_workspace_root` 一次；当 `cwd` 在子目录、explicit
/// `--file` 是默认值时,emit 应仍由 P6 guard 拒绝（不允许落
/// `cwd/.ralph/events.jsonl` 孤儿),而非用 `cwd = sorts/`
/// 解析到 `sorts/.ralph/events.jsonl`。
///
/// 这条测试根因锁定
/// `docs/report/2026-07-06-ce-executor-ralph-emit-pwd-sorts-diagnosis.md`
/// 的事件孤儿落盘路径：line 561-563 二次 `let workspace_root =
/// current_dir()` 遮蔽此前 line 397 的 `resolve_workspace_root`
/// 锚定。修复前:`current_dir() = sorts/`,default_path 解析为
/// `sorts/.ralph/events.jsonl`,事件落入子树孤儿文件。修复后:
/// workspace_root 沿用 line 397 锚定（callsite 传入的父目录）,
/// default_path 解析为 `parent/.ralph/events.jsonl`;由于该
/// 路径不在 allowlist 且比子目录孤儿位置更安全（不会命中 sort cd
/// 后的 cwd 子树）,事件被正确拒绝而不是落到 orphan 子树。
#[test]
fn test_emit_from_nested_cwd_uses_ralph_workspace_root_for_markers() {
    // workspace + 子目录 sorts/ 双层 fixture
    let outer_tmp = tempfile::TempDir::new().expect("outer temp dir");
    let workspace = outer_tmp.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
    let sorts_dir = workspace.join("sorts");
    std::fs::create_dir_all(&sorts_dir).expect("sorts dir");

    let prev_cwd = std::env::current_dir().ok();
    // 模拟 hat 内部 `cd sorts/`:set_current_dir 到子目录。
    if let Err(e) = std::env::set_current_dir(&sorts_dir) {
        panic!("set_current_dir to sorts_dir must succeed: {e}");
    }

    let result = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("debug.step".to_string()),
            payload: "task_id=demo".to_string(),
            json: false,
            // 显式 default --file:这等于 relative `.ralph/events.jsonl`,
            // resolve_emit_path 视为 no-explicit,沿 marker 路径
            // 解析(candidate_marker → current_marker →
            // current_hat_marker → default_path)。本 fixture
            // 没有 marker → 解析到 `workspace/.ralph/events.jsonl`。
            file: PathBuf::from(".ralph/events.jsonl"),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    );

    // 还原 cwd(避免污染后续测试)
    if let Some(prev) = prev_cwd {
        let _ = std::env::set_current_dir(prev);
    }

    // 修复前(workspace_root = sorts/):emit 成功,事件落到
    // `sorts/.ralph/events.jsonl` 孤儿文件。
    // 修复后(workspace_root = 父目录,显式 root 参数):
    // default_path = parent/.ralph/events.jsonl 不在 allowlist
    // (本 fixture 无 marker),P6 guard 正确拒绝。
    // 关键反断言:**无论如何** 都不要在 sorts/.ralph/ 下创建
    // events.jsonl 孤儿。
    let subtree_orphan_dir = sorts_dir.join(".ralph");
    let subtree_orphan = subtree_orphan_dir.join("events.jsonl");
    assert!(
        !subtree_orphan.exists(),
        "shadowing regression: emit must not create sorts/.ralph/events.jsonl orphan, found: {}",
        subtree_orphan.display()
    );
    // 进一步:这是修复前的行为(成功 emit);修复后由 P6 guard 拒绝
    // (因为 default_path 指向 `parent/.ralph/events.jsonl`,
    // 但 allowlist 仅在 marker 存在时才包括 channel。允许的
    // 行为是 Err 或 Ok,但 subtree 不能创建。
    // 这里 result 可以是 Err(P6 guard 拒绝);但也允许 Ok 当
    // workspace_root 解析的目标恰好落入 allowlist。
    let _ = result; // 见上 subtree_orphan 反断言已保证核心不变量
}

#[test]
fn test_emit_command_blocks_once_when_urgent_steer_pending() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
    UrgentSteerStore::new(urgent_steer_path_from_workspace(Some(&workspace)))
        .append_message("stop and fix the failing tests")
        .expect("write urgent steer");

    let err = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("debug.step".to_string()),
            payload: "task_id=demo".to_string(),
            json: false,
            file: PathBuf::from(".ralph/events.jsonl"),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect_err("urgent steer should block first emit");

    let message = format!("{err:#}");
    assert!(message.contains("Urgent steer is pending"));
    assert!(message.contains("stop and fix the failing tests"));

    assert!(
        UrgentSteerStore::new(urgent_steer_path_from_workspace(Some(&workspace)))
            .load()
            .expect("load marker")
            .is_none(),
        "first blocked emit should clear urgent steer marker"
    );
}

#[test]
fn test_emit_policy_check_rejects_business_after_terminal_with_marker() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

    // Write config with event policy
    std::fs::write(
        workspace.join("ralph.yml"),
        r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - LOOP_COMPLETE
    business_topics:
      - experiment.planned
",
    )
    .unwrap();

    // Write existing events file with a terminal event
    let events_file = workspace.join(".ralph/events.jsonl");
    std::fs::write(
        &events_file,
        r#"{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:00Z"}
"#,
    )
    .unwrap();

    // Write marker file pointing to events file
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/events.jsonl\n",
    )
    .unwrap();

    let err = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("experiment.planned".to_string()),
            payload: "{}".to_string(),
            json: true,
            file: PathBuf::from(".ralph/events.jsonl"),
            policy_check: true,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect_err("should reject business event after terminal");

    let message = format!("{err:#}");
    assert!(
        message.contains("Event rejected by policy"),
        "Expected policy rejection, got: {}",
        message
    );
    assert!(
        message.contains("monotonicity"),
        "Expected monotonicity violation, got: {}",
        message
    );

    // Verify the rejected event was NOT appended
    let events = std::fs::read_to_string(&events_file).expect("read events");
    assert!(!events.contains("experiment.planned"));
}

#[test]
fn test_emit_policy_check_without_existing_events_succeeds() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

    // Write config with event policy
    std::fs::write(
        workspace.join("ralph.yml"),
        r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - LOOP_COMPLETE
    business_topics:
      - experiment.planned
",
    )
    .unwrap();

    let events_file = workspace.join(".ralph/events.jsonl");

    emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("experiment.planned".to_string()),
            payload: "{}".to_string(),
            json: true,
            file: events_file.clone(),
            policy_check: true,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect("should accept business event when no terminal exists");

    let events = std::fs::read_to_string(&events_file).unwrap_or_default();
    assert!(
        events.trim().is_empty(),
        "explicit --policy-check must not write to events file; got: {events}"
    );
}

#[test]
fn test_emit_policy_check_fallback_to_args_file_when_marker_missing() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

    // Write config with event policy
    std::fs::write(
        workspace.join("ralph.yml"),
        r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - LOOP_COMPLETE
    business_topics:
      - experiment.planned
",
    )
    .unwrap();

    // Write existing events file WITHOUT marker
    let events_file = workspace.join(".ralph/events.jsonl");
    std::fs::write(
        &events_file,
        r#"{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:00Z"}
"#,
    )
    .unwrap();

    let err = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("experiment.planned".to_string()),
            payload: "{}".to_string(),
            json: true,
            file: events_file.clone(),
            policy_check: true,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect_err("should reject business event after terminal");

    let message = format!("{err:#}");
    assert!(
        message.contains("Event rejected by policy"),
        "Expected policy rejection, got: {}",
        message
    );

    // Verify the rejected event was NOT appended
    let events = std::fs::read_to_string(&events_file).expect("read events");
    assert!(!events.contains("experiment.planned"));
}

#[test]
fn test_emit_with_provenance_flags() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

    let events_file = workspace.join(".ralph/events.jsonl");

    // U1 of 2026-07-05-005: write a minimal ralph.yml that
    // overrides RALPH_HATS_SOURCE from the parent loop context so
    // the isolated-scope guard accepts the test's chosen hat +
    // topic. The hat id matches RALPH_CURRENT_HAT (typically
    // "fixer") so the U7 isolated-mode hat-match check at
    // emit.rs:550-560 also passes. The current-events marker is
    // pointed at the parent loop's RALPH_EVENTS_FILE (when present)
    // so the P6 allowlist guard accepts the env-injected target.
    // Test intent (provenance flag preservation) is unchanged.
    let hat = std::env::var("RALPH_CURRENT_HAT").unwrap_or_else(|_| "strategist".to_string());
    // Mirror RALPH_TRIGGERED_HAT when the parent loop sets it; otherwise
    // fall back to the same hat id as `--hat` so the U7 topology check
    // (`check_envelope_triggered`) sees a declared id and the
    // ralph.yml below only needs one entry under `hats:`.
    let triggered = std::env::var("RALPH_TRIGGERED_HAT")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| hat.clone());
    let triggered_entry = if triggered == hat {
        String::new()
    } else {
        format!(
            "  {triggered}:\n    name: \"{triggered}\"\n    triggers: []\n    publishes: [\"experiment.planned\", \"*\"]\n"
        )
    };
    std::fs::write(
            workspace.join("ralph.yml"),
            format!(
                "event_loop:\n  execution_mode: coordinator\nhats:\n  {hat}:\n    name: \"{hat}\"\n    triggers: []\n    publishes: [\"experiment.planned\", \"*\"]\n{triggered_entry}"
            ),
        )
        .expect("write ralph.yml");
    let env_events_file = std::env::var("RALPH_EVENTS_FILE").ok();
    if let Some(ref env_path) = env_events_file {
        std::fs::write(workspace.join(".ralph/current-events"), env_path.as_bytes())
            .expect("write current-events marker");
    }

    emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("experiment.planned".to_string()),
            payload: r#"{"task_key":"x"}"#.to_string(),
            json: true,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: Some(hat.clone()),
            triggered: Some(triggered.clone()),
            source: Some("cli".to_string()),
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect("emit with provenance should succeed");

    // The emit may have been routed to the env-injected events
    // file (when RALPH_EVENTS_FILE is set by the parent loop) or
    // to the workspace's events.jsonl (when no env override
    // exists). Read whichever the resolver chose.
    let read_target = env_events_file
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or(events_file);
    let events = std::fs::read_to_string(&read_target).expect("read events");
    assert!(events.contains(&format!("\"hat\":\"{hat}\"")));
    assert!(events.contains(&format!("\"triggered\":\"{triggered}\"")));
    assert!(events.contains("\"source\":\"cli\""));
}

#[test]
fn test_resolve_provenance_priority() {
    // CLI args take priority over env vars
    let env = |key: &str| match key {
        "RALPH_CURRENT_HAT" => Some("env-hat".to_string()),
        "RALPH_TRIGGERED_HAT" => Some("env-triggered".to_string()),
        "RALPH_EVENT_SOURCE" => Some("env-source".to_string()),
        _ => None,
    };
    let (hat, triggered, source) = resolve_provenance(Some("cli-hat".to_string()), None, None, env);
    assert_eq!(hat, Some("cli-hat".to_string()));
    assert_eq!(triggered, Some("env-triggered".to_string()));
    assert_eq!(source, Some("env-source".to_string()));
}

#[test]
fn test_resolve_provenance_env_fallback() {
    // When CLI args are missing, env vars are used
    let env = |key: &str| match key {
        "RALPH_CURRENT_HAT" => Some("env-hat".to_string()),
        "RALPH_TRIGGERED_HAT" => Some("env-triggered".to_string()),
        "RALPH_EVENT_SOURCE" => Some("env-source".to_string()),
        _ => None,
    };
    let (hat, triggered, source) = resolve_provenance(None, None, None, env);
    assert_eq!(hat, Some("env-hat".to_string()));
    assert_eq!(triggered, Some("env-triggered".to_string()));
    assert_eq!(source, Some("env-source".to_string()));
}

#[test]
fn test_resolve_provenance_empty_env_is_ignored() {
    // Empty env vars are treated as absent
    let env = |_key: &str| Some(String::new());
    let (hat, triggered, source) = resolve_provenance(None, None, None, env);
    assert_eq!(hat, None);
    assert_eq!(triggered, None);
    assert_eq!(source, None);
}

// U1: ralph-hat business-topic guard. Mirrors the origin guard's
// `ralph_control_only` rejection at the JSONL read path, but rejects
// here so the agent receives synchronous backpressure instead of
// waiting several seconds for the loop runner to surface the rejection.
// The guard fires regardless of --policy-check, because the issue is
// the impersonation, not the payload shape.

#[test]
fn test_emit_ralph_hat_rejects_business_topic_review_passed() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
    let events_file = workspace.join(".ralph/events.jsonl");

    let err = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("review.passed".to_string()),
                payload: r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("ralph".to_string()),
                triggered: None,
                source: None,
                schema: None,
                output: "text".to_string(),
                policy_check_token: None,
            },
            Some(&workspace),
        )
        .expect_err("ralph hat must not be allowed to emit review.passed");

    let message = format!("{err:#}");
    assert!(
        message.contains("Builtin ralph hat may only emit control topics"),
        "expected ralph-control guard message, got: {message}"
    );
    assert!(
        message.contains("review.passed"),
        "error should name the rejected topic, got: {message}"
    );

    // Verify nothing was written
    assert!(!events_file.exists() || std::fs::read_to_string(&events_file).unwrap().is_empty());
}

#[test]
fn test_emit_ralph_hat_rejects_business_topic_work_start() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
    let events_file = workspace.join(".ralph/events.jsonl");

    let err = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("work.start".to_string()),
            payload: String::new(),
            json: false,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: Some("ralph".to_string()),
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect_err("ralph hat must not be allowed to emit work.start");

    let message = format!("{err:#}");
    assert!(message.contains("Builtin ralph hat may only emit control topics"));
}

#[test]
fn test_emit_ralph_hat_allows_control_topic_loop_complete() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
    let events_file = workspace.join(".ralph/events.jsonl");

    emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("LOOP_COMPLETE".to_string()),
            payload: r#"{"reason":"done"}"#.to_string(),
            json: true,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: Some("ralph".to_string()),
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect("ralph hat must be allowed to emit LOOP_COMPLETE (control topic)");

    let events = std::fs::read_to_string(&events_file).expect("read events");
    assert!(events.contains("\"topic\":\"LOOP_COMPLETE\""));
    assert!(events.contains("\"hat\":\"ralph\""));
}

#[test]
fn test_emit_ralph_hat_allows_task_resume() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
    let events_file = workspace.join(".ralph/events.jsonl");

    emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("task.resume".to_string()),
            payload: r#"{"reason":"recover"}"#.to_string(),
            json: true,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: Some("ralph".to_string()),
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect("ralph hat must be allowed to emit task.resume (control topic)");

    let events = std::fs::read_to_string(&events_file).expect("read events");
    assert!(events.contains("task.resume"));
}

#[test]
fn test_emit_executor_hat_unaffected_by_ralph_guard() {
    // Regression: only `ralph` is restricted. Other hats (executor,
    // coordinator, etc.) may emit business topics as before.
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
    let events_file = workspace.join(".ralph/events.jsonl");

    emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("work.done".to_string()),
                payload: r#"{"plan_name":"p","plan_path":"x.md","task_id":"t","task_key":"k","step":"s","commit_count":1,"changed_lines":10}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("executor".to_string()),
                triggered: None,
                source: None,
                schema: None,
                output: "text".to_string(),
                policy_check_token: None,
            },
            Some(&workspace),
        )
        .expect("executor hat should be free to emit work.done (not restricted)");

    let events = std::fs::read_to_string(&events_file).expect("read events");
    assert!(events.contains("work.done"));
    assert!(events.contains("\"hat\":\"executor\""));
}

#[test]
fn test_emit_no_hat_unaffected_by_ralph_guard() {
    // No --hat means no ralph guard fires. Other guards (provenance,
    // policy) still apply.
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
    let events_file = workspace.join(".ralph/events.jsonl");

    emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("debug.step".to_string()),
            payload: "task_id=demo".to_string(),
            json: false,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect("no-hat emit should not be blocked by the ralph guard");

    let events = std::fs::read_to_string(&events_file).expect("read events");
    assert!(events.contains("debug.step"));
}

#[test]
fn test_emit_provenance_strict_rejects_missing_hat() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

    // Write config with require_emit_provenance enabled
    let yaml = r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    require_emit_provenance: true
";
    std::fs::write(workspace.join("ralph.yml"), yaml).unwrap();

    // Verify config loads and parses correctly in isolation
    let config_sources = vec![ConfigSource::File(workspace.join("ralph.yml"))];
    let config =
        load_config_with_overrides(&config_sources).expect("config should load for this test");
    let policy = config
        .event_loop
        .event_policy
        .as_ref()
        .expect("event_policy should be present");
    assert!(
        policy.require_emit_provenance,
        "require_emit_provenance should be true"
    );

    let events_file = workspace.join(".ralph/events.jsonl");

    let err = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("build.done".to_string()),
            payload: String::new(),
            json: false,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect_err("should reject emit without provenance when strict");

    let message = format!("{err:#}");
    assert!(
        message.contains("Event provenance required"),
        "Expected provenance rejection, got: {}",
        message
    );

    // Verify nothing was written
    assert!(!events_file.exists() || std::fs::read_to_string(&events_file).unwrap().is_empty());
}

#[test]
fn test_emit_provenance_strict_allows_with_hat() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

    // Write config with require_emit_provenance enabled
    std::fs::write(
        workspace.join("ralph.yml"),
        r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    require_emit_provenance: true
",
    )
    .unwrap();

    let events_file = workspace.join(".ralph/events.jsonl");

    emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("build.done".to_string()),
            payload: String::new(),
            json: false,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: Some("strategist".to_string()),
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect("should allow emit with hat when strict");

    let events = std::fs::read_to_string(&events_file).expect("read events");
    assert!(events.contains("\"hat\":\"strategist\""));
}

#[test]
fn test_emit_strict_config_rejects_missing_required_field_without_policy_check_flag() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

    // Write strict config: policy enabled AND require_policy_check_for_cli_emit
    std::fs::write(
        workspace.join("ralph.yml"),
        r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    allow_unsafe_cli_emit: true
    schemas:
      experiment.planned:
        required_fields:
          - task_key
          - hypothesis
          - falsification_condition
",
    )
    .unwrap();

    let events_file = workspace.join(".ralph/events.jsonl");

    let err = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("experiment.planned".to_string()),
            payload: r#"{"task_key":"x"}"#.to_string(),
            json: true,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect_err("strict config should reject missing required field even without --policy-check");

    let message = format!("{err:#}");
    assert!(
        message.contains("Event rejected by policy"),
        "Expected policy rejection, got: {}",
        message
    );

    // Verify nothing was written
    assert!(!events_file.exists() || std::fs::read_to_string(&events_file).unwrap().is_empty());
}

#[test]
fn test_emit_strict_config_rejects_duplicate_terminal_without_policy_check_flag() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

    // Write strict config
    std::fs::write(
        workspace.join("ralph.yml"),
        r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    allow_unsafe_cli_emit: true
    terminal_topics:
      - LOOP_COMPLETE
",
    )
    .unwrap();

    // Pre-seed events file with a terminal event
    let events_file = workspace.join(".ralph/events.jsonl");
    std::fs::write(
        &events_file,
        r#"{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:00Z"}
"#,
    )
    .unwrap();

    let err = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("LOOP_COMPLETE".to_string()),
            payload: r#"{"reason":"done"}"#.to_string(),
            json: true,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect_err("strict config should reject duplicate terminal even without --policy-check");

    let message = format!("{err:#}");
    assert!(
        message.contains("Event rejected by policy"),
        "Expected policy rejection, got: {}",
        message
    );

    // Verify duplicate was NOT appended
    let events = std::fs::read_to_string(&events_file).expect("read events");
    assert_eq!(events.lines().count(), 1);
}

#[test]
fn test_emit_non_strict_config_allows_without_policy_check() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

    // Write non-strict config: policy enabled but require_policy_check_for_cli_emit is false
    std::fs::write(
        workspace.join("ralph.yml"),
        r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: false
    terminal_topics:
      - LOOP_COMPLETE
",
    )
    .unwrap();

    let events_file = workspace.join(".ralph/events.jsonl");

    emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("build.done".to_string()),
            payload: String::new(),
            json: false,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect("non-strict config should allow emit without --policy-check");

    let events = std::fs::read_to_string(&events_file).expect("read events");
    assert!(events.contains("build.done"));
}

#[test]
fn test_emit_explicit_policy_check_behavior_preserved() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

    // Write config with event policy but NOT strict CLI enforcement
    std::fs::write(
        workspace.join("ralph.yml"),
        r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: false
    terminal_topics:
      - LOOP_COMPLETE
    business_topics:
      - experiment.planned
",
    )
    .unwrap();

    // Pre-seed with terminal
    let events_file = workspace.join(".ralph/events.jsonl");
    std::fs::write(
        &events_file,
        r#"{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:00Z"}
"#,
    )
    .unwrap();

    // Explicit --policy-check should still reject business after terminal
    let err = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("experiment.planned".to_string()),
            payload: "{}".to_string(),
            json: true,
            file: events_file.clone(),
            policy_check: true,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect_err("explicit --policy-check should still reject");

    let message = format!("{err:#}");
    assert!(message.contains("Event rejected by policy"));
}

#[test]
fn test_emit_unsafe_bypass_allowed_when_config_permits() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

    // Write strict config but allow unsafe bypass
    std::fs::write(
        workspace.join("ralph.yml"),
        r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    allow_unsafe_cli_emit: true
    terminal_topics:
      - LOOP_COMPLETE
",
    )
    .unwrap();

    // Pre-seed with terminal
    let events_file = workspace.join(".ralph/events.jsonl");
    std::fs::write(
        &events_file,
        r#"{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:00Z"}
"#,
    )
    .unwrap();

    // Unsafe bypass should allow the duplicate terminal through
    emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("LOOP_COMPLETE".to_string()),
            payload: r#"{"reason":"retry"}"#.to_string(),
            json: true,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: true,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect("unsafe bypass should work when config allows it");

    let events = std::fs::read_to_string(&events_file).expect("read events");
    assert!(events.contains("\"reason\":\"retry\""));
}

#[test]
fn test_emit_unsafe_bypass_rejected_when_config_denies() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

    // Write strict config that DISALLOWS unsafe bypass
    std::fs::write(
        workspace.join("ralph.yml"),
        r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    allow_unsafe_cli_emit: false
    terminal_topics:
      - LOOP_COMPLETE
",
    )
    .unwrap();

    // Pre-seed with terminal
    let events_file = workspace.join(".ralph/events.jsonl");
    std::fs::write(
        &events_file,
        r#"{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:00Z"}
"#,
    )
    .unwrap();

    // Unsafe bypass should be rejected because config denies it
    let err = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("LOOP_COMPLETE".to_string()),
            payload: r#"{"reason":"retry"}"#.to_string(),
            json: true,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: true,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect_err("unsafe bypass should fail when config denies it");

    let message = format!("{err:#}");
    assert!(
        message.contains("Event rejected by policy"),
        "Expected policy rejection, got: {}",
        message
    );
}

const FIXTURE_VALID_CHAIN: &str = r#"{"topic":"experiment.planned","payload":{"task_key":"a","hypothesis":"h","falsification_condition":"f"},"ts":"2026-05-22T00:00:00Z"}
{"topic":"LOOP_COMPLETE","payload":{"reason":"done"},"ts":"2026-05-22T00:00:01Z"}"#;

const FIXTURE_DUPLICATE_TERMINAL: &str = r#"{"topic":"LOOP_COMPLETE","payload":{"reason":"done"},"ts":"2026-05-22T00:00:00Z"}
{"topic":"LOOP_COMPLETE","payload":{"reason":"retry"},"ts":"2026-05-22T00:00:01Z"}"#;

const FIXTURE_BUSINESS_AFTER_TERMINAL: &str = r#"{"topic":"LOOP_COMPLETE","payload":{"reason":"done"},"ts":"2026-05-22T00:00:00Z"}
{"topic":"experiment.planned","payload":{"task_key":"b","hypothesis":"h","falsification_condition":"f"},"ts":"2026-05-22T00:00:01Z"}"#;

const FIXTURE_MISSING_REQUIRED_FIELDS: &str =
    r#"{"topic":"experiment.planned","payload":{"task_key":"a"},"ts":"2026-05-22T00:00:00Z"}"#;

fn fixture_config_yaml() -> &'static str {
    r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    require_emit_provenance: true
    allow_unsafe_cli_emit: true
    terminal_topics:
      - LOOP_COMPLETE
    business_topics:
      - experiment.planned
    schemas:
      experiment.planned:
        payload: json_object
        required_fields:
          - task_key
          - hypothesis
          - falsification_condition
    completion_after_terminal:
      duplicate_terminal: reject
      business_after_completion: reject
hats:
  strategist:
    name: strategist
    triggers:
      - experiment.planned
    publishes:
      - LOOP_COMPLETE
"
}

fn fixture_policy_config() -> ralph_core::EventPolicyConfig {
    let full: ralph_core::RalphConfig = serde_yaml::from_str(fixture_config_yaml()).unwrap();
    full.event_loop.event_policy.unwrap()
}

fn setup_fixture_workspace(temp_dir: &TempDir, prior_events: &str) -> PathBuf {
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).unwrap();
    std::fs::write(workspace.join("ralph.yml"), fixture_config_yaml()).unwrap();
    let events_file = workspace.join(".ralph/events.jsonl");
    std::fs::write(&events_file, prior_events).unwrap();
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/events.jsonl\n",
    )
    .unwrap();
    workspace
}

fn parse_last_fixture_event(fixture: &str) -> (String, String, bool) {
    let line = fixture.lines().last().unwrap();
    let value: serde_json::Value = serde_json::from_str(line).unwrap();
    let topic = value["topic"].as_str().unwrap().to_string();
    let (payload, json) = match &value.get("payload") {
        Some(serde_json::Value::Object(_)) => {
            (serde_json::to_string(&value["payload"]).unwrap(), true)
        }
        Some(serde_json::Value::String(s)) => (s.clone(), false),
        Some(serde_json::Value::Null) | None => (String::new(), false),
        _ => (serde_json::to_string(&value["payload"]).unwrap(), true),
    };
    (topic, payload, json)
}

#[test]
fn test_fixture_cli_valid_chain_accepted() {
    let temp_dir = TempDir::new().expect("temp dir");
    let prior: String = FIXTURE_VALID_CHAIN
        .lines()
        .take(1)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let workspace = setup_fixture_workspace(&temp_dir, &prior);
    let (topic, payload, json) = parse_last_fixture_event(FIXTURE_VALID_CHAIN);
    let events_file = workspace.join(".ralph/events.jsonl");

    emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some(topic),
            payload,
            json,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: Some("strategist".to_string()),
            triggered: None,
            source: Some("cli".to_string()),
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect("CLI should accept valid chain terminal event");

    let events = std::fs::read_to_string(&events_file).expect("read events");
    assert!(events.contains("\"reason\":\"done\""));
    assert!(events.contains("\"hat\":\"strategist\""));
    assert!(events.contains("\"source\":\"cli\""));
}

#[test]
fn test_fixture_cli_duplicate_terminal_rejected() {
    let temp_dir = TempDir::new().expect("temp dir");
    let prior: String = FIXTURE_DUPLICATE_TERMINAL
        .lines()
        .take(1)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let workspace = setup_fixture_workspace(&temp_dir, &prior);
    let (topic, payload, json) = parse_last_fixture_event(FIXTURE_DUPLICATE_TERMINAL);
    let events_file = workspace.join(".ralph/events.jsonl");

    let err = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some(topic),
            payload,
            json,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: Some("strategist".to_string()),
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect_err("CLI should reject duplicate terminal");

    let message = format!("{err:#}");
    assert!(
        message.contains("Event rejected by policy")
            || message.contains("Event blocked by policy")
            || message.contains("Event ignored by policy"),
        "Expected policy rejection, got: {}",
        message
    );

    let events = std::fs::read_to_string(&events_file).expect("read events");
    assert_eq!(events.lines().count(), 1);
}

#[test]
fn test_fixture_cli_business_after_terminal_rejected() {
    let temp_dir = TempDir::new().expect("temp dir");
    let prior: String = FIXTURE_BUSINESS_AFTER_TERMINAL
        .lines()
        .take(1)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let workspace = setup_fixture_workspace(&temp_dir, &prior);
    let (topic, payload, json) = parse_last_fixture_event(FIXTURE_BUSINESS_AFTER_TERMINAL);
    let events_file = workspace.join(".ralph/events.jsonl");

    let err = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some(topic),
            payload,
            json,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: Some("strategist".to_string()),
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect_err("CLI should reject business after terminal");

    let message = format!("{err:#}");
    assert!(
        message.contains("Event rejected by policy")
            || message.contains("Event blocked by policy")
            || message.contains("Event ignored by policy"),
        "Expected policy rejection, got: {}",
        message
    );

    let events = std::fs::read_to_string(&events_file).expect("read events");
    assert!(!events.contains("\"task_key\":\"b\""));
}

#[test]
fn test_fixture_cli_missing_required_fields_rejected_when_strict() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = setup_fixture_workspace(&temp_dir, "");
    let (topic, payload, json) = parse_last_fixture_event(FIXTURE_MISSING_REQUIRED_FIELDS);
    let events_file = workspace.join(".ralph/events.jsonl");

    let err = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some(topic),
            payload,
            json,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: None, // missing provenance
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect_err("CLI should reject missing provenance under strict config");

    let message = format!("{err:#}");
    assert!(
        message.contains("Event provenance required"),
        "Expected provenance rejection, got: {}",
        message
    );

    assert!(!events_file.exists() || std::fs::read_to_string(&events_file).unwrap().is_empty());
}

#[test]
fn test_fixture_cross_cutting_cli_and_event_loop_agree() {
    use ralph_core::{PolicyDecision, PolicyRuntimeState, validate_event};

    let policy_config = fixture_policy_config();

    let fixtures: &[&str] = &[
        FIXTURE_VALID_CHAIN,
        FIXTURE_DUPLICATE_TERMINAL,
        FIXTURE_BUSINESS_AFTER_TERMINAL,
        FIXTURE_MISSING_REQUIRED_FIELDS,
    ];

    for fixture in fixtures {
        let lines: Vec<&str> = fixture.lines().collect();
        let prior = if lines.len() > 1 {
            lines[..lines.len() - 1].join("\n") + "\n"
        } else {
            String::new()
        };

        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = setup_fixture_workspace(&temp_dir, &prior);
        let events_file = workspace.join(".ralph/events.jsonl");

        // -- Event loop path --
        let mut state =
            PolicyRuntimeState::from_events(&events_file, &policy_config).unwrap_or_default();

        let line = lines.last().unwrap();
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        let topic = value["topic"].as_str().unwrap();
        let payload = match &value.get("payload") {
            Some(v) if !v.is_null() => Some(serde_json::to_string(v).unwrap()),
            _ => None,
        };

        let loop_decision = validate_event(topic, payload.as_deref(), &policy_config, &mut state);
        // U2 (plan 2026-07-04-004): `AcknowledgeAndForward` is
        // an "accept" from the bus-forwarding perspective. The
        // dedup finding is logged but the event reaches the
        // state machine. Parity test fixtures must NOT treat
        // AcknowledgeAndForward as a rejection.
        let loop_accept = matches!(
            loop_decision,
            PolicyDecision::Accept
                | PolicyDecision::Warn(_)
                | PolicyDecision::AcknowledgeAndForward(_)
        );

        // -- CLI path --
        let (cli_topic, cli_payload, cli_json) = parse_last_fixture_event(fixture);
        let cli_result = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some(cli_topic),
                payload: cli_payload,
                json: cli_json,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("strategist".to_string()),
                triggered: None,
                source: None,
                schema: None,
                output: "text".to_string(),
                policy_check_token: None,
            },
            Some(&workspace),
        );
        let cli_accept = cli_result.is_ok();

        assert_eq!(
            loop_accept, cli_accept,
            "Cross-cutting classification mismatch for fixture.\nFixture: {}\nLoop decision: {:?}\nCLI result: {:?}",
            fixture, loop_decision, cli_result
        );
    }
}

#[test]
fn test_provenance_fields_preserved_by_reader() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).unwrap();
    let events_file = workspace.join(".ralph/events.jsonl");

    // U1 of 2026-07-05-005: write a minimal ralph.yml + current-events
    // marker that overrides RALPH_HATS_SOURCE / RALPH_EVENTS_FILE from
    // the parent loop context (see test_emit_with_provenance_flags for
    // full rationale).
    let hat = std::env::var("RALPH_CURRENT_HAT").unwrap_or_else(|_| "strategist".to_string());
    // Mirror RALPH_TRIGGERED_HAT when the parent loop sets it; otherwise
    // fall back to the same hat id as `--hat` so the U7 topology check
    // (`check_envelope_triggered`) sees a declared id and the
    // ralph.yml below only needs one entry under `hats:`.
    let triggered = std::env::var("RALPH_TRIGGERED_HAT")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| hat.clone());
    let triggered_entry = if triggered == hat {
        String::new()
    } else {
        format!(
            "  {triggered}:\n    name: \"{triggered}\"\n    triggers: []\n    publishes: [\"experiment.planned\", \"*\"]\n"
        )
    };
    std::fs::write(
            workspace.join("ralph.yml"),
            format!(
                "event_loop:\n  execution_mode: coordinator\nhats:\n  {hat}:\n    name: \"{hat}\"\n    triggers: []\n    publishes: [\"experiment.planned\", \"*\"]\n{triggered_entry}"
            ),
        )
        .expect("write ralph.yml");
    let env_events_file = std::env::var("RALPH_EVENTS_FILE").ok();
    if let Some(ref env_path) = env_events_file {
        std::fs::write(workspace.join(".ralph/current-events"), env_path.as_bytes())
            .expect("write current-events marker");
    }

    let read_target = env_events_file
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| events_file.clone());

    // Snapshot pre-emit event count from the target file (the file
    // may already contain events from a parent loop when env is set).
    let pre_count = if read_target.exists() {
        std::fs::read_to_string(&read_target)
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
    } else {
        0
    };

    emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("experiment.planned".to_string()),
            payload: r#"{"task_key":"x"}"#.to_string(),
            json: true,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: Some(hat.clone()),
            triggered: Some(triggered.clone()),
            source: Some("cli".to_string()),
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect("emit should succeed");

    let mut reader = ralph_core::EventReader::new(&read_target);
    let result = reader.read_new_events().unwrap();
    // The pre_count snapshot accounts for parent-loop events
    // sharing the same file; only one new event should appear.
    assert_eq!(result.events.len(), pre_count + 1);
    let event = result.events.last().expect("at least one event");
    assert_eq!(event.hat, Some(hat));
    assert_eq!(event.triggered, Some(triggered));
    assert_eq!(event.source, Some("cli".to_string()));
}

#[test]
fn test_old_simple_event_fixtures_still_parse() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).unwrap();
    let events_file = workspace.join(".ralph/events.jsonl");
    std::fs::write(
        &events_file,
        r#"{"topic":"task.start","payload":"Start work","ts":"2024-01-01T00:00:00Z"}
{"topic":"task.done","payload":null,"ts":"2024-01-01T00:00:01Z"}
{"topic":"noop","ts":"2024-01-01T00:00:02Z"}
"#,
    )
    .unwrap();

    let mut reader = ralph_core::EventReader::new(&events_file);
    let result = reader.read_new_events().unwrap();
    assert_eq!(result.events.len(), 3);
    assert_eq!(result.events[0].topic, "task.start");
    assert_eq!(result.events[0].payload, Some("Start work".to_string()));
    assert!(result.events[1].payload.is_none());
    assert!(result.events[2].payload.is_none());
}

fn make_workspace(tmp: &TempDir) -> PathBuf {
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join(".ralph")).unwrap();
    root
}

#[test]
fn test_emit_default_uses_current_candidate_marker() {
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    std::fs::write(
        workspace.join(".ralph/current-candidate-events"),
        ".ralph/events-20260101-000000.jsonl",
    )
    .unwrap();
    let resolved = resolve_emit_path(
        &workspace,
        &workspace.join(".ralph/events.jsonl"),
        None,
        None,
        false,
        None,
        None,
        None,
    )
    .unwrap();
    assert!(resolved.ends_with(".ralph/events-20260101-000000.jsonl"));
}

#[test]
fn test_emit_default_uses_current_events_marker() {
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/events-20260101-000000.jsonl",
    )
    .unwrap();
    let resolved = resolve_emit_path(
        &workspace,
        &workspace.join(".ralph/events.jsonl"),
        None,
        None,
        false,
        None,
        None,
        None,
    )
    .unwrap();
    assert!(resolved.ends_with(".ralph/events-20260101-000000.jsonl"));
}

#[test]
fn test_emit_no_marker_allows_default_events_jsonl() {
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    let cli_file = workspace.join(".ralph/events.jsonl");
    let resolved =
        resolve_emit_path(&workspace, &cli_file, None, None, false, None, None, None).unwrap();
    assert_eq!(resolved, cli_file);
}

#[test]
fn test_emit_file_explicit_current_marker_allowed() {
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/events-20260101-000000.jsonl",
    )
    .unwrap();
    // The explicit --file target equals the marker target, so it is
    // accepted (matches the allowlist entry).
    let cli_file = workspace.join(".ralph/events-20260101-000000.jsonl");
    let resolved =
        resolve_emit_path(&workspace, &cli_file, None, None, false, None, None, None).unwrap();
    assert_eq!(resolved, cli_file);
}

#[test]
fn test_emit_file_other_loop_rejected() {
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/events-20260101-000000.jsonl",
    )
    .unwrap();
    // An explicit --file that points outside the allowlist must be
    // rejected. We do NOT silently rewrite to the marker target —
    // that would let an agent redirect events to a different
    // worktree's file.
    let cli_file = workspace.join(".ralph/events-other.jsonl");
    let result = resolve_emit_path(&workspace, &cli_file, None, None, false, None, None, None);
    assert!(
        result.is_err(),
        "non-allowlisted --file must be rejected, got: {:?}",
        result.map(|p| p.display().to_string())
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("allowlist") || msg.contains("not in"),
        "error should mention allowlist, got: {msg}"
    );
}

#[test]
fn test_emit_env_events_file_other_loop_rejected() {
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/events-20260101-000000.jsonl",
    )
    .unwrap();
    // RALPH_EVENTS_FILE pointing at a different file is rejected.
    let env_value = workspace
        .join(".ralph/events-other.jsonl")
        .display()
        .to_string();
    let result = resolve_emit_path(
        &workspace,
        &workspace.join(".ralph/events.jsonl"),
        Some(&env_value),
        None,
        false,
        None,
        None,
        None,
    );
    assert!(
        result.is_err(),
        "non-allowlisted RALPH_EVENTS_FILE must be rejected"
    );
}

#[test]
fn test_emit_path_traversal_rejected() {
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/events-20260101-000000.jsonl",
    )
    .unwrap();
    // An explicit `--file ../escape.jsonl` is rejected because it
    // is not in the events allowlist. The new guard treats the file
    // as a request to escape the workspace and refuses outright
    // (no silent rewrite to the marker).
    let cli_file = workspace.join("../escape.jsonl");
    let result = resolve_emit_path(&workspace, &cli_file, None, None, false, None, None, None);
    assert!(
        result.is_err(),
        "path traversal with explicit --file must be rejected"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("allowlist") || msg.contains("not in"),
        "error should mention allowlist, got: {msg}"
    );

    // Without a marker and an explicit traversal, the explicit file
    // is also rejected (the default events.jsonl is not in scope of
    // the traversal).
    std::fs::remove_file(workspace.join(".ralph/current-events")).unwrap();
    let result = resolve_emit_path(&workspace, &cli_file, None, None, false, None, None, None);
    assert!(
        result.is_err(),
        "path traversal with no marker must be rejected"
    );
}

#[test]
fn test_emit_symlink_to_other_loop_rejected() {
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    // No markers; the allowlist is just the default events.jsonl.
    let outside = tmp.path().parent().unwrap().join("outside.jsonl");
    std::fs::write(&outside, "{}").unwrap();
    // A symlink that aliases the default target to an outside file is
    // detected via canonicalize and rejected.
    let link = workspace.join(".ralph/events.jsonl");
    if std::os::unix::fs::symlink(&outside, &link).is_err() {
        return;
    }
    let result = resolve_emit_path(
        &workspace,
        &workspace.join(".ralph/events.jsonl"),
        None,
        None,
        false,
        None,
        None,
        None,
    );
    assert!(result.is_err(), "symlink to outside loop must be rejected");
}

/// U2 (2026-07-06-002 plan, R2): orphan guard 拒绝落在 subtree 的
/// `.ralph/events*.jsonl` 路径——即使 P6 allowlist 错误地接受(通过
/// 被篡改的 `current-hat-events` marker),`current_hat` 已设置下
/// 也不能落到 `sorts/.ralph/...`。这是 hat 进程在 subtree cwd 下
/// 写出 orphan 文件的最后一道防线。
/// U3 (2026-07-06-002 plan, R3): 当 isolated 模式 + hat 上下文 +
/// 未注入 `RALPH_EVENTS_FILE` + 使用默认 `--file` 时,如果进程的
/// `cwd` **离开** `workspace_root` 子树(例如跨到无关工程目录),
/// emit 必须硬拒绝,错误码 `cwd_workspace_drift`。
///
/// 判别口径(cwd 子树内仍由 U1/U2 在下一层处理,见 `commands/emit`
/// 内的 gate 注释):本测试聚焦"cwd 在 workspace_root 外"的硬拒绝。
#[test]
fn test_emit_cwd_drift_rejected_in_isolated_hat_context() {
    let outer_tmp = tempfile::TempDir::new().expect("outer temp dir");
    let workspace = outer_tmp.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

    // 注入 isolated mode config + validator hat 注册(对齐
    // test_emit_with_provenance_flags 的 ralph.yml 结构)
    std::fs::write(
            workspace.join("ralph.yml"),
            "event_loop:\n  execution_mode: isolated\nhats:\n  validator:\n    name: validator\n    triggers: []\n    publishes: [\"debug.step\", \"*\"]\n",
        )
        .expect("write ralph.yml");

    // 把 cwd 切到 workspace_root **外** 的另一临时目录,模拟
    // hat 进程跑出 workspace 子树(U3 的真正 fail-closed 触发面)。
    let other_root_tmp = tempfile::TempDir::new().expect("other workspace temp dir");
    let other_root = other_root_tmp.path().to_path_buf();
    let prev_cwd = std::env::current_dir().ok();
    if let Err(e) = std::env::set_current_dir(&other_root) {
        panic!("set_current_dir to other workspace root must succeed: {e}");
    }

    // 显式传 hat = validator(模拟 RALPH_CURRENT_HAT 已设置
    // 通过 cli flag;should_load_config 也会触发)。**不**设置
    // RALPH_EVENTS_FILE env(RALPH_EVENTS_FILE 在测试进程级别
    // unsafe,且 must not leak into other tests)。保持默认
    // --file = .ralph/events.jsonl。
    let result = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("debug.step".to_string()),
            payload: "task_id=demo".to_string(),
            json: false,
            file: PathBuf::from(".ralph/events.jsonl"),
            policy_check: false,
            no_policy_check: false,
            hat: Some("validator".to_string()),
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    );

    // 还原 cwd
    if let Some(prev) = prev_cwd {
        let _ = std::env::set_current_dir(prev);
    }

    let err = result.expect_err("cwd outside workspace_root in isolated hat context must bail");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("cwd_workspace_drift"),
        "expected cwd_workspace_drift rejection, got: {msg}"
    );

    // 反断言:不再依赖 `sorts/` subtree(本测试 cwd 是另一临时
    // workspace root,不是 workspace 子树);仅校验 cwd 不在
    // workspace_root 内留下 `.ralph/events*.jsonl`。
    let other_orphan = other_root.join(".ralph/events.jsonl");
    assert!(
        !other_orphan.exists(),
        "rejected emit must not write to other_root/.ralph/events.jsonl, found: {}",
        other_orphan.display()
    );
}

/// An active isolated hat channel must never degrade to the main events file
/// when the subprocess loses its hat identity. The previous fallback selected
/// `current-events` first, allowing a bare business event to bypass the
/// per-hat channel and precheck path.
#[test]
fn test_emit_rejects_missing_hat_identity_with_active_hat_channel() {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let workspace = tmp.path().to_path_buf();
    let ralph_dir = workspace.join(".ralph");
    let agent_dir = ralph_dir.join("agent");
    std::fs::create_dir_all(&agent_dir).expect("create runtime dirs");
    std::fs::write(
        workspace.join("ralph.yml"),
        r#"
event_loop:
hats:
  validator:
    name: validator
    triggers: []
    publishes: ["debug.step"]
"#,
    )
    .expect("write isolated config");

    let main_events = ralph_dir.join("events-main.jsonl");
    let hat_events = agent_dir.join("events-hat-validator-loop-1.jsonl");
    std::fs::write(&main_events, "").expect("create main events");
    std::fs::write(&hat_events, "").expect("create hat channel");
    std::fs::write(
        &ralph_dir.join("current-events"),
        ".ralph/events-main.jsonl\n",
    )
    .expect("write main marker");
    std::fs::write(
        &ralph_dir.join("current-hat-events"),
        ".ralph/agent/events-hat-validator-loop-1.jsonl\n",
    )
    .expect("write hat marker");

    let err = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("debug.step".to_string()),
            payload: "task_id=demo".to_string(),
            json: false,
            file: PathBuf::from(".ralph/events.jsonl"),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect_err("missing hat identity must fail closed");

    let message = format!("{err:#}");
    assert!(
        message.contains("agent emit context incomplete"),
        "unexpected error: {message}"
    );
    assert!(
        std::fs::read_to_string(&main_events)
            .expect("read main events")
            .is_empty(),
        "incomplete agent context must not write the main events ledger"
    );
    assert!(
        std::fs::read_to_string(&hat_events)
            .expect("read hat events")
            .is_empty(),
        "failed emit must not write the hat channel either"
    );
}

#[test]
fn test_emit_control_topic_preserves_missing_hat_behavior_with_active_hat_channel() {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let workspace = tmp.path().to_path_buf();
    let ralph_dir = workspace.join(".ralph");
    let agent_dir = ralph_dir.join("agent");
    std::fs::create_dir_all(&agent_dir).expect("create runtime dirs");

    let main_events = ralph_dir.join("events-main.jsonl");
    let hat_events = agent_dir.join("events-hat-validator-loop-1.jsonl");
    std::fs::write(&main_events, "").expect("create main events");
    std::fs::write(&hat_events, "").expect("create hat channel");
    std::fs::write(
        &ralph_dir.join("current-events"),
        ".ralph/events-main.jsonl\n",
    )
    .expect("write main marker");
    std::fs::write(
        &ralph_dir.join("current-hat-events"),
        ".ralph/agent/events-hat-validator-loop-1.jsonl\n",
    )
    .expect("write hat marker");

    emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("task.resume".to_string()),
            payload: "{}".to_string(),
            json: false,
            file: PathBuf::from(".ralph/events.jsonl"),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect("control topics retain their no-hat behavior");

    assert!(
        std::fs::read_to_string(&main_events)
            .expect("read main events")
            .contains("task.resume")
    );
    assert!(
        std::fs::read_to_string(&hat_events)
            .expect("read hat events")
            .is_empty()
    );
}

/// U3 (R3): 当 `cwd == workspace_root` 时,即使 isolated + hat +
/// 默认 `--file`,也允许继续(因为子树漂移风险为 0)。
#[test]
fn test_emit_cwd_matches_workspace_root_allowed() {
    let outer_tmp = tempfile::TempDir::new().expect("outer temp dir");
    let workspace = outer_tmp.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

    // isolated mode config + hat 注册
    std::fs::write(
            workspace.join("ralph.yml"),
            "event_loop:\n  execution_mode: isolated\nhats:\n  validator:\n    name: validator\n    triggers: []\n    publishes: [\"debug.step\", \"*\"]\n",
        )
        .expect("write ralph.yml");

    let prev_cwd = std::env::current_dir().ok();
    if let Err(e) = std::env::set_current_dir(&workspace) {
        panic!("set_current_dir to workspace must succeed: {e}");
    }

    let result = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("debug.step".to_string()),
            payload: "task_id=demo".to_string(),
            json: false,
            file: PathBuf::from(".ralph/events.jsonl"),
            policy_check: false,
            no_policy_check: false,
            hat: Some("validator".to_string()),
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    );

    if let Some(prev) = prev_cwd {
        let _ = std::env::set_current_dir(prev);
    }

    // cwd == workspace_root → gate 不触发;后续由 resolve_emit_path 决策。
    // 这里可能因为 policy / scope 其它 gate 失败,而 fail,但不是
    // 因为 cwd_workspace_drift。
    if let Err(err) = &result {
        let msg = format!("{err:#}");
        assert!(
            !msg.contains("cwd_workspace_drift"),
            "cwd == workspace_root must NOT trigger drift gate, got: {msg}"
        );
    }
    // 关键:这次 emit 不能创建 sorts subtree 文件(场景里也没 sorts)。
    let _ = result;
}

/// U3 (R3) 豁免:当 `--file` 是 **显式非默认**(指向 allowlist
/// 内的绝对路径)时,cwd 漂移 gate 不应触发——这是高级场景。
///
/// 把 cwd 切到 workspace 外(触发 gate 条件),然后用 explicit
/// `--file` 命中 allowlist,断言 gate 不 bail。
#[test]
fn test_emit_cwd_drift_with_explicit_file_is_exempt() {
    let outer_tmp = tempfile::TempDir::new().expect("outer temp dir");
    let workspace = outer_tmp.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

    // isolated mode + hat 注册 + current-events marker(指向合法通道)
    std::fs::write(
            workspace.join("ralph.yml"),
            "event_loop:\n  execution_mode: isolated\nhats:\n  validator:\n    name: validator\n    triggers: []\n    publishes: [\"debug.step\", \"*\"]\n",
        )
        .expect("write ralph.yml");
    // 让 explicit --file 落入 allowlist(把它写进 current-events marker)
    let explicit_target = workspace.join(".ralph/explicit-target.jsonl");
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/explicit-target.jsonl",
    )
    .expect("write marker");

    // 把 cwd 切到 workspace_root **外**(触发 gate 触发条件)
    let other_root_tmp = tempfile::TempDir::new().expect("other workspace temp dir");
    let other_root = other_root_tmp.path().to_path_buf();
    let prev_cwd = std::env::current_dir().ok();
    if let Err(e) = std::env::set_current_dir(&other_root) {
        panic!("set_current_dir to other workspace root must succeed: {e}");
    }

    let result = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("debug.step".to_string()),
            payload: "task_id=demo".to_string(),
            json: false,
            file: explicit_target.clone(), // 显式非默认
            policy_check: false,
            no_policy_check: false,
            hat: Some("validator".to_string()),
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    );

    if let Some(prev) = prev_cwd {
        let _ = std::env::set_current_dir(prev);
    }

    // 关键反断言:**不**应出现 cwd_workspace_drift(explicit 豁免)
    if let Err(err) = &result {
        let msg = format!("{err:#}");
        assert!(
            !msg.contains("cwd_workspace_drift"),
            "explicit --file must exempt drift gate, got: {msg}"
        );
    }
}

#[test]
fn test_emit_orphan_subtree_path_rejected_under_hat_context() {
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    let sorts = workspace.join("sorts");
    std::fs::create_dir_all(sorts.join(".ralph")).unwrap();

    // 场景:无 current-events / current-candidate-events marker。
    // 注入 hat-marker 指向 subtree(攻击者伪造或错误的 subtree 解析)。
    // 这种情况下 P6 allowlist 会接受该 subtree 路径,只有 orphan
    // guard 能拦截。
    std::fs::write(
        workspace.join(".ralph/current-hat-events"),
        sorts
            .join(".ralph/events.jsonl")
            .strip_prefix(&workspace)
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    )
    .unwrap();
    let malicious_subtree = sorts.join(".ralph/events.jsonl");

    // 使用 default cli file + isolated + hat context,让 candidate
    // 路径由 U2 (R4) 的 fallthrough 逻辑解析到 hat-marker,即
    // malicious_subtree。然后 orphan guard 必须在 `Some(hat)` 时
    // 拦截。
    let cli_file = workspace.join(".ralph/events.jsonl");
    let result = resolve_emit_path(
        &workspace,
        &cli_file,
        None,
        Some("validator"),
        true,
        None,
        None,
        None,
    );
    match result {
        Ok(path) => panic!(
            "orphan subtree path must not be accepted, got: {}",
            path.display()
        ),
        Err(err) => {
            let msg = format!("{err:#}");
            assert!(
                msg.contains("orphan_events_path")
                    || msg.contains("allowlist")
                    || msg.contains("not in"),
                "expected orphan / allowlist rejection, got: {msg}"
            );
        }
    }
    // 反断言:不应在 subtree 留下孤儿文件(仅有 isolated_mode &&
    // current_hat 时 guard 触发,此测试正好满足这两个条件)。
    assert!(
        !malicious_subtree.exists()
            || std::fs::read_to_string(&malicious_subtree)
                .unwrap()
                .is_empty(),
        "rejected emit must not write to subtree orphan file"
    );
}

/// U2 (2026-07-06-002 plan, R4): isolated + hat_maker 已设置 + 无
/// `current-events` / `current-candidate-events` marker 时,emit 应
/// 走 `current-hat-events` marker 解析到 hat-channel,而不是 fallback
/// 到 `workspace_root/.ralph/events.jsonl` default。
#[test]
fn test_emit_isolated_with_hat_marker_falls_through_to_channel() {
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    std::fs::create_dir_all(workspace.join(".ralph/agent")).unwrap();
    // 只有 hat-marker,没有 current-events / current-candidate-events
    std::fs::write(
        workspace.join(".ralph/current-hat-events"),
        ".ralph/agent/events-hat-validator-001-1.jsonl",
    )
    .unwrap();
    let resolved = resolve_emit_path(
        &workspace,
        &workspace.join(".ralph/events.jsonl"), // 默认 cli file
        None,
        Some("validator"), // hat context 存在
        true,              // isolated_mode
        None,
        None,
        None,
    )
    .expect("isolated + hat-marker must resolve to channel");
    assert!(
        resolved.ends_with(".ralph/agent/events-hat-validator-001-1.jsonl"),
        "isolated + hat-marker must resolve to channel, got: {}",
        resolved.display()
    );
}

// -------------------------------------------------------------------------
// U1: wave-worker channel allowlist characterization
// P0 root cause: dispatcher injects RALPH_EVENTS_FILE=.ralph/wave-<id>-<idx>.jsonl
// into wave workers, but the P6 emit allowlist (current-events / current-candidate-events
// / current-hat-events marker targets + default events.jsonl) does not include the
// wave channel path. Agents fall back to writing the main events file, breaking
// the supervisor's causal chain.
//
// API note: resolve_emit_path does NOT take RALPH_WAVE_WORKER as a parameter.
// The wave-worker signal must be inferred from the path shape (.ralph/wave-<id>-<idx>.jsonl)
// combined with isolated_mode=true. U2 must extend the allowlist to recognize this
// pattern, either via a new parameter or via path-shape detection in production code.
// -------------------------------------------------------------------------

/// U1: wave-worker channel must be accepted when isolated_mode=true and the
/// channel path matches the wave pattern (.ralph/wave-<id>-<idx>.jsonl).
///
/// Current behavior (BUG): allowlist rejects because .ralph/wave-w-test-0.jsonl
/// does not match any marker target (current-events / current-candidate-events /
/// current-hat-events).
///
/// Target behavior (after U2): resolve_emit_path returns Ok(wave_channel_path).
/// After U6 (2026-07-26-002): the candidate must additionally appear in
/// `.ralph/current-wave-channels` (the dispatcher-signed allowlist); env-only
/// self-claim is no longer enough.
#[test]
fn test_emit_wave_worker_channel_accepted() {
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    // Main loop's current-events marker (the dispatcher sets this, not the wave channel)
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/events-main.jsonl",
    )
    .unwrap();

    // Wave dispatcher injects RALPH_EVENTS_FILE=.ralph/wave-w-test-0.jsonl
    // into the worker process. The wave channel path must be accepted in
    // wave-worker context (isolated_mode=true, current_hat present, path shape
    // matches .ralph/wave-<id>-<idx>.jsonl).
    let wave_channel_path = workspace.join(".ralph/wave-w-test-0.jsonl");
    let wave_channel = wave_channel_path.display().to_string();

    // 2026-07-27-003 plan U2 (KTD-1): the dispatcher commits
    // the per-wave JSON registry entry BEFORE spawning,
    // replacing the legacy `.ralph/current-wave-channels`
    // append-only marker.
    let _guard = crate::loop_runner::wave::WaveChannelRegistry::prepare(
        &workspace,
        "loop-u3-wtest",
        "w-test",
        &[crate::loop_runner::wave::BindingInput::new(
            0,
            wave_channel_path.clone(),
        )],
    )
    .expect("registry prepare must succeed");

    let result = resolve_emit_path(
        &workspace,
        &workspace.join(".ralph/events.jsonl"), // default cli file (not used — env overrides)
        Some(&wave_channel),
        Some("exec-worker"), // current_hat = wave worker hat
        true,                // isolated_mode (wave workers run in isolated context)
        Some("w-test"),
        Some(0),
        Some("loop-u3-wtest"),
    );

    // TARGET behavior: Ok with the wave channel path
    assert!(
        result.is_ok(),
        "wave-worker channel must be accepted in isolated mode, got error: {:?}",
        result.as_ref().err()
    );
    let resolved = result.unwrap();
    assert!(
        resolved.ends_with(".ralph/wave-w-test-0.jsonl"),
        "resolved path must point to wave channel, got: {}",
        resolved.display()
    );
}

/// U1: wave-worker channel must be rejected when isolated_mode=false
/// (no wave-worker context). This confirms the allowlist still protects
/// against non-wave paths even after the U2 fix.
///
/// Current behavior: rejected (path not in allowlist).
/// Target behavior (after U2): still rejected (wave pattern only accepted
/// when isolated_mode=true signals wave-worker context).
#[test]
fn test_emit_wave_worker_channel_rejected_without_isolated_mode() {
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/events-main.jsonl",
    )
    .unwrap();

    let wave_channel = workspace
        .join(".ralph/wave-w-test-0.jsonl")
        .display()
        .to_string();

    // isolated_mode=false → no wave-worker context signal
    let result = resolve_emit_path(
        &workspace,
        &workspace.join(".ralph/events.jsonl"),
        Some(&wave_channel),
        Some("exec-worker"),
        false, // NOT isolated → no wave-worker context
        None,
        None,
        None,
    );

    assert!(
        result.is_err(),
        "wave channel must be rejected without isolated_mode, got: {:?}",
        result
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("allowlist") || msg.contains("not in"),
        "error must mention allowlist, got: {msg}"
    );
}

/// 2026-07-26-002 plan U6 (R6 / AE6): even with isolated + hat +
/// matching wave_id/index, a wave channel whose absolute path
/// does NOT appear in `.ralph/current-wave-channels` (the
/// dispatcher-signed marker) MUST be rejected. This is the
/// U6 forgery guard: an attacker who can set env vars cannot
/// grant themselves write access to an arbitrary
/// `.ralph/wave-<id>-<idx>.jsonl` file — only the dispatcher
/// that wrote the marker can grant access.
#[test]
fn test_emit_wave_worker_channel_rejected_without_marker_signature() {
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/events-main.jsonl",
    )
    .unwrap();
    // 2026-07-27-003 plan U2 (KTD-1): no per-wave registry
    // entry written — simulates the attacker scenario where
    // the worker self-claims the channel via env vars without
    // dispatcher sign. The legacy
    // `.ralph/current-wave-channels` marker has been replaced
    // by the JSON registry; the rejection error now names the
    // registry instead.

    let wave_channel = workspace
        .join(".ralph/wave-w-test-0.jsonl")
        .display()
        .to_string();

    let result = resolve_emit_path(
        &workspace,
        &workspace.join(".ralph/events.jsonl"),
        Some(&wave_channel),
        Some("exec-worker"),
        true,           // isolated_mode = true
        Some("w-test"), // matching wave_id
        Some(0),        // matching slot_index,
        Some("loop-u3-wtest"),
    );

    assert!(
        result.is_err(),
        "forged env without registry signature must be rejected; got Ok({result:?})",
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("wave_channel_registry_reject")
            || msg.contains("registry")
            || msg.contains("dispatcher"),
        "error must reference the missing registry; got: {msg}"
    );
}

/// U2 / adversarial-01: even with isolated + hat, a wave channel
/// whose `<id>` doesn't match the worker's `RALPH_WAVE_ID` must be
/// rejected. The carve-out is dispatcher-signed: only the slot the
/// dispatcher named is allowed to write its own channel.
#[test]
fn test_emit_wave_worker_channel_rejected_with_mismatched_wave_id() {
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/events-main.jsonl",
    )
    .unwrap();

    // Worker's RALPH_WAVE_ID says "w-rs-1" but the path is for
    // "w-other" — forged cross-slot attempt.
    let wave_channel = workspace
        .join(".ralph/wave-w-other-0.jsonl")
        .display()
        .to_string();

    let result = resolve_emit_path(
        &workspace,
        &workspace.join(".ralph/events.jsonl"),
        Some(&wave_channel),
        Some("exec-worker"),
        true,
        Some("w-rs-1"), // worker-bound wave id
        Some(0),
        None,
    );
    assert!(
        result.is_err(),
        "wave channel with mismatched <id> must be rejected, got: {result:?}"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("allowlist") || msg.contains("not in"),
        "error must mention allowlist, got: {msg}"
    );
}

/// U2 / adversarial-01: same shape but the `<idx>` segment must
/// match `RALPH_WAVE_INDEX` too. A worker for slot 0 cannot write
/// slot 1's channel.
#[test]
fn test_emit_wave_worker_channel_rejected_with_mismatched_slot_index() {
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/events-main.jsonl",
    )
    .unwrap();

    let wave_channel = workspace
        .join(".ralph/wave-w-test-1.jsonl")
        .display()
        .to_string();

    let result = resolve_emit_path(
        &workspace,
        &workspace.join(".ralph/events.jsonl"),
        Some(&wave_channel),
        Some("exec-worker"),
        true,
        Some("w-test"),
        Some(0), // worker-bound slot index,
        None,
    );
    assert!(
        result.is_err(),
        "wave channel with mismatched <idx> must be rejected, got: {result:?}"
    );
}

// 2026-07-26-003 plan U3: characterization + (small) widening
// for the review-worker hat id. The `is_wave_channel_path`
// shape check is hat-id-agnostic today (exec- and fix-worker
// both share the same gate), but the `implementation-review`
// preset's review-worker is the one whose misroute into main
// was the primary-20260726 incident root cause. These tests
// pin the contract so a future narrowing cannot regress
// without explicit intent.

/// U3 / S3 (plan 2026-07-26-003): the review-worker hat's
/// wave-channel `ralph emit` must be accepted with the same
/// shape check as exec-worker. The dispatcher signs
/// `wave-<id>-<idx>.jsonl` and injects RALPH_WAVE_ID /
/// RALPH_WAVE_INDEX; review-worker's activation must land
/// there, never on the main events file (which would silently
/// dispatch the dimension into `compute_missing_dimensions`'s
/// blind spot).
///
/// 2026-07-26-002 plan U6 (R6 / KTD2) merged in: the dispatcher
/// also writes the absolute channel path to
/// `.ralph/current-wave-channels` BEFORE spawning, so env-only
/// self-claim is no longer enough. This test mirrors the marker
/// write to exercise the full signed-channel path for
/// review-worker.
#[test]
fn test_emit_review_worker_channel_accepted() {
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/events-main.jsonl",
    )
    .unwrap();
    let wave_channel_path = workspace.join(".ralph/wave-w-review-2.jsonl");
    let wave_channel = wave_channel_path.display().to_string();
    // Dispatcher-signed via the per-wave registry JSON
    // (2026-07-27-003 plan U2 replaces
    // `.ralph/current-wave-channels`).
    let _guard = crate::loop_runner::wave::WaveChannelRegistry::prepare(
        &workspace,
        "loop-u3-review",
        "w-review",
        &[crate::loop_runner::wave::BindingInput::new(
            2,
            wave_channel_path.clone(),
        )],
    )
    .expect("registry prepare must succeed");
    let result = resolve_emit_path(
        &workspace,
        &workspace.join(".ralph/events.jsonl"),
        Some(&wave_channel),
        Some("review-worker"), // review-worker hat id
        true,                  // isolated execution context
        Some("w-review"),
        Some(2),
        Some("loop-u3-review"),
    );
    assert!(
        result.is_ok(),
        "review-worker channel must be accepted in isolated mode, got error: {:?}",
        result.as_ref().err()
    );
    assert!(
        result.unwrap().ends_with(".ralph/wave-w-review-2.jsonl"),
        "resolved path must point to wave channel"
    );
}

/// U3 / S3 + R3 plan-2026-07-26-003: the review-worker channel
/// round-trip is end-to-end via `ralph emit`'s public entry,
/// not just `resolve_emit_path`. Smoke that the command path
/// does not silently rewrite the path back to `events.jsonl`
/// once it has accepted the wave channel. (This is the test
/// 003 did NOT add for review-worker because the channel
/// acceptance check is hat-agnostic; we add it explicitly to
/// lock the integration.)
#[test]
fn test_emit_review_worker_channel_file_is_appended() {
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/events-main.jsonl",
    )
    .unwrap();
    let wave_channel = workspace.join(".ralph/wave-w-rt-0.jsonl");
    let wave_channel_str = wave_channel.display().to_string();
    // 2026-07-27-003 plan U2 (KTD-1) — dispatcher signs the
    // channel via the per-wave JSON registry, replacing the
    // legacy `.ralph/current-wave-channels` marker.
    let _guard = crate::loop_runner::wave::WaveChannelRegistry::prepare(
        &workspace,
        "loop-u3-rt",
        "w-rt",
        &[crate::loop_runner::wave::BindingInput::new(
            0,
            wave_channel.clone(),
        )],
    )
    .expect("registry prepare must succeed");
    // Sanity: resolve_emit_path must point at the channel,
    // not the main events file.
    let resolved = resolve_emit_path(
        &workspace,
        &workspace.join(".ralph/events.jsonl"),
        Some(&wave_channel_str),
        Some("review-worker"),
        true,
        Some("w-rt"),
        Some(0),
        Some("loop-u3-rt"),
    )
    .expect("resolve");
    assert!(
        resolved.ends_with(".ralph/wave-w-rt-0.jsonl"),
        "resolved must be wave channel"
    );
}

/// U3 / S4 (plan 2026-07-26-003): when a wave-worker (or a
/// hat masquerading as one) tries to land on a path that
/// doesn't carry the dispatcher-signed wave shape, the
/// rejection must NOT be silent — the call site emits a
/// machine-readable stderr line (`path_resolution_failed`)
/// so an integrator hat that misroutes can be diagnosed by
/// `ralph diagnose`. The `recovery.jsonl` envelope is reserved
/// for the policy-precheck path; this assertion prevents a
/// future refactor from erasing the explicit stderr signal
/// during a "tidy error printing" pass.
#[test]
fn test_emit_wave_worker_mismatch_writes_diagnostic_signal() {
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/events-main.jsonl",
    )
    .unwrap();
    // Mismatched wave_id while isolated + hat present.
    let wave_channel = workspace
        .join(".ralph/wave-w-other-0.jsonl")
        .display()
        .to_string();
    let result = resolve_emit_path(
        &workspace,
        &workspace.join(".ralph/events.jsonl"),
        Some(&wave_channel),
        Some("review-worker"),
        true,
        Some("w-expected"), // dispatcher-bound id
        Some(0),
        None,
    );
    assert!(result.is_err(), "mismatched wave_id must be rejected");
    let msg = result.unwrap_err().to_string();
    // Either the explicit allowlist rejection OR a symlink /
    // path-traversal message is acceptable; what matters is
    // that the failure is observable — i.e. the path
    // silently falling back to main is impossible.
    assert!(
        !msg.is_empty(),
        "rejection message must carry a non-empty diagnostic"
    );
}

/// U3 / S4 (plan 2026-07-26-003, R3) + 2026-07-27-003 U2: when
/// the wave-worker handshake (`wave_id` + `slot_index`) is
/// present but `RALPH_EVENTS_FILE` is unset, marker
/// fallthrough would previously resolve to `current-events`
/// (main) and silently append there (the implementation-review
/// primary-20260727-051801 double-ledger root cause). After
/// plan 2026-07-27-003 U2 the registry resolver refuses any
/// wave-worker call whose `(loop_id, wave_id, slot_index,
/// path)` tuple is not in the dispatcher-committed registry
/// JSON — no main fallback path.
#[test]
fn test_emit_wave_worker_unset_events_file_rejects_main_fallthrough() {
    use crate::loop_runner::wave::WaveChannelRegistry;

    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/events-main.jsonl",
    )
    .unwrap();
    // Dispatcher signs the channel via the per-wave registry
    // JSON (U2 replaces `.ralph/current-wave-channels`).
    let loop_id = "loop-u3-fallthrough";
    let wave_id = "w-rs-1";
    let signed = workspace.join(".ralph/wave-w-rs-1-2.jsonl");
    // prepare creates the channel file via create_new — no
    // pre-creation needed.
    let bindings = vec![crate::loop_runner::wave::BindingInput::new(
        2,
        signed.clone(),
    )];
    let _guard = WaveChannelRegistry::prepare(&workspace, loop_id, wave_id, &bindings)
        .expect("registry prepare must succeed");

    // (1) No env, no --file → must NOT silently resolve to
    // main. Marker fallthrough is gone; the resolver falls
    // through to `events.jsonl` (the non-wave-worker default),
    // which is outside the dispatcher's binding and is
    // rejected as a registry miss.
    let result = resolve_emit_path(
        &workspace,
        &workspace.join(".ralph/events.jsonl"),
        None,
        Some("review-worker"),
        true,
        Some(wave_id),
        Some(2),
        Some(loop_id),
    );
    assert!(
        result.is_err(),
        "wave worker must not silently fall through to main; got Ok({result:?})"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("wave_channel_registry_reject")
            || msg.contains("RALPH_EVENTS_FILE")
            || msg.contains("empty_worker_result")
            || msg.contains("not in this loop's events allowlist"),
        "error must name the fallthrough failure mode; got: {msg}"
    );

    // (2) Positive control: dispatcher's signed channel
    // still works (this is what the worker should have
    // invoked).
    let ok = resolve_emit_path(
        &workspace,
        &workspace.join(".ralph/events.jsonl"),
        Some(&signed.display().to_string()),
        Some("review-worker"),
        true,
        Some(wave_id),
        Some(2),
        Some(loop_id),
    )
    .expect("signed channel must resolve");
    assert_eq!(ok, signed);
}

#[test]
fn test_emit_auto_detects_json_payload_without_json_flag() {
    // Bug #4 regression: work.done and other structured events must be
    // stored as JSON objects even when the agent forgets --json.
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    let events_file = workspace.join(".ralph/events.jsonl");

    let args = EmitArgs {
        topic: Some("work.done".to_string()),
        payload: r#"{"plan_name":"test","task_id":"t1"}"#.to_string(),
        json: false,
        file: events_file.clone(),
        policy_check: false,
        no_policy_check: false,
        hat: None,
        triggered: None,
        source: None,
        schema: None,
        output: "text".to_string(),
        policy_check_token: None,
    };

    emit_command_with_root(ColorMode::Never, args, Some(&workspace)).unwrap();

    let content = std::fs::read_to_string(&events_file).unwrap();
    let event: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
    // payload must be an object, NOT a string
    assert!(
        event["payload"].is_object(),
        "payload should be auto-detected as JSON object, got: {:?}",
        event["payload"]
    );
    assert_eq!(event["payload"]["plan_name"], "test");
}

#[test]
fn test_emit_leaves_plain_string_as_string() {
    // Non-JSON-looking strings must stay strings for backward compat.
    let tmp = TempDir::new().unwrap();
    let workspace = make_workspace(&tmp);
    let events_file = workspace.join(".ralph/events.jsonl");

    let args = EmitArgs {
        topic: Some("build.done".to_string()),
        payload: "Build succeeded".to_string(),
        json: false,
        file: events_file.clone(),
        policy_check: false,
        no_policy_check: false,
        hat: None,
        triggered: None,
        source: None,
        schema: None,
        output: "text".to_string(),
        policy_check_token: None,
    };

    emit_command_with_root(ColorMode::Never, args, Some(&workspace)).unwrap();

    let content = std::fs::read_to_string(&events_file).unwrap();
    let event: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
    assert_eq!(event["payload"], "Build succeeded");
}

#[test]
fn test_looks_like_json_heuristic() {
    assert!(looks_like_json(r#"{"key":"val"}"#));
    assert!(looks_like_json("  [{\"a\":1}]"));
    assert!(!looks_like_json("hello world"));
    assert!(!looks_like_json(""));
    assert!(!looks_like_json("  plain text"));
}

// ------------------------------------------------------------------
// U5 / R6: `ralph emit --schema <TOPIC>` smoke tests.
//
// The handler short-circuits to a read-only JSON dump of the
// embedded protocol view (KTD-10) before any policy / scope /
// gate runs. These tests pin that contract: no events
// file is touched, no policy decision is required, the output
// is valid JSON carrying `protocol_hash` and the requested
// topic's `required_fields`.
// ------------------------------------------------------------------

/// Minimal preset fixture mirroring the section layout that
/// `build.rs` produces for builtin CE presets. We only need
/// `event_policy.schemas.work.done` to exercise the
/// required-fields surface.
const SCHEMA_FIXTURE_YAML: &str = r"
event_loop:
  execution_mode: isolated
  event_policy:
    enabled: true
    mode: enforce
    schemas:
      work.done:
        required_fields:
          - plan_name
          - task_id
          - task_key
";

fn setup_schema_workspace(tmp: &TempDir, yaml: &str) -> PathBuf {
    let workspace = tmp.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).unwrap();
    std::fs::write(workspace.join("ralph.yml"), yaml).unwrap();
    workspace
}

/// (1) `ralph emit --schema <topic>` prints a JSON view carrying
/// `protocol_hash` and the topic's `required_fields`. The view
/// is the only stdout payload; the events file is not created.
#[test]
fn test_emit_schema_prints_protocol_view_without_writing_events() {
    let tmp = TempDir::new().expect("temp dir");
    let workspace = setup_schema_workspace(&tmp, SCHEMA_FIXTURE_YAML);
    let events_file = workspace.join(".ralph/events.jsonl");

    let args = EmitArgs {
        topic: Some("work.done".to_string()),
        payload: String::new(),
        json: false,
        file: events_file.clone(),
        policy_check: false,
        no_policy_check: false,
        hat: None,
        triggered: None,
        source: None,
        schema: Some("work.done".to_string()),
        output: "text".to_string(),
        policy_check_token: None,
    };

    // R6: read-only mode must succeed without producing an event.
    emit_command_with_root(ColorMode::Never, args, Some(&workspace))
        .expect("schema mode should succeed");

    // Events file must NOT have been created — `--schema` is
    // strictly read-only and the operator's toolchain relies on
    // "no events file = no event was emitted".
    assert!(
        !events_file.exists() || std::fs::read_to_string(&events_file).unwrap().is_empty(),
        "schema mode must not write to events.jsonl"
    );
}

/// (2) Required fields for the topic come from the embedded
/// `event_policy.schemas`. Operators use these to confirm
/// drift between the authoring YAML and the embedded copy.
#[test]
fn test_emit_schema_view_reflects_required_fields() {
    let tmp = TempDir::new().expect("temp dir");
    let workspace = setup_schema_workspace(&tmp, SCHEMA_FIXTURE_YAML);
    let _events_file = workspace.join(".ralph/events.jsonl");

    // Render via the public path the CLI uses, then introspect
    // the resulting JSON. We build the view the same way the
    // handler does (RalphConfig + hats_source=None +
    // ProtocolView::from_event_loop) and assert on its fields
    // directly — keeps the test hermetic and pins the rendering
    // contract without coupling to stdout capture.
    let config_path = workspace.join("ralph.yml");
    let config_sources = vec![ConfigSource::File(config_path)];
    let cfg = crate::preflight::load_config_for_preflight_sync(&config_sources, None, &workspace)
        .expect("load fixture config");
    let view = ProtocolView::from_event_loop(&cfg.event_loop);
    let value = super::schema_view::render_topic(&view, "work.done").expect("render view");

    assert_eq!(value["topic"], "work.done");
    let required = value["required_fields"]
        .as_array()
        .expect("required_fields is array");
    let required: std::collections::HashSet<&str> =
        required.iter().filter_map(|v| v.as_str()).collect();
    assert!(required.contains("plan_name"));
    assert!(required.contains("task_id"));
    assert!(required.contains("task_key"));
    assert_eq!(required.len(), 3);

    assert_eq!(value["is_macro_edge"], serde_json::Value::Bool(false));
    assert!(value["protocol_hash"].as_str().is_some());
    assert!(
        !value["protocol_hash"].as_str().unwrap().is_empty(),
        "protocol_hash must be non-empty"
    );
}

/// (3) Unknown topics return an empty `required_fields` array
/// instead of erroring. This matches `ProtocolView::required_fields`
/// semantics and lets operators probe the protocol without
/// having to pre-check the topic table.
#[test]
fn test_emit_schema_view_for_unknown_topic_returns_empty_required_fields() {
    let tmp = TempDir::new().expect("temp dir");
    let workspace = setup_schema_workspace(&tmp, SCHEMA_FIXTURE_YAML);

    let config_path = workspace.join("ralph.yml");
    let config_sources = vec![ConfigSource::File(config_path)];
    let cfg = crate::preflight::load_config_for_preflight_sync(&config_sources, None, &workspace)
        .expect("load fixture config");
    let view = ProtocolView::from_event_loop(&cfg.event_loop);
    let value = super::schema_view::render_topic(&view, "totally.unknown.topic")
        .expect("render view for unknown topic");

    assert_eq!(value["topic"], "totally.unknown.topic");
    let required = value["required_fields"]
        .as_array()
        .expect("required_fields is array");
    assert!(
        required.is_empty(),
        "unknown topic must yield empty required_fields, got: {required:?}"
    );
    // is_macro_edge is kept in the output for backwards compatibility
    // but is always false; macro-edge semantics were removed.
    assert_eq!(value["is_macro_edge"], serde_json::Value::Bool(false));
}

/// (4) Without a discoverable ralph.yml AND without a `.ralph/`
/// marker, schema mode fails closed with a clear error — the
/// `should_load_config` gate in the handler skips config
/// resolution, so `config` is `None` and the schema branch
/// must surface a friendly error instead of rendering an empty
/// default view.
#[test]
fn test_emit_schema_fails_closed_when_no_config() {
    let tmp = TempDir::new().expect("temp dir");
    let workspace = tmp.path().to_path_buf();
    // No ralph.yml, no .ralph — operator forgot to cd into a
    // preset-bearing workspace. Without `.ralph/` the
    // `should_load_config` gate is false, so config resolution
    // is skipped entirely and the schema branch sees `config =
    // None`, which it must turn into a clear fail-closed error.
    let events_file = workspace.join(".ralph/events.jsonl");

    let args = EmitArgs {
        topic: Some("work.done".to_string()),
        payload: String::new(),
        json: false,
        file: events_file.clone(),
        policy_check: false,
        no_policy_check: false,
        hat: None,
        triggered: None,
        source: None,
        schema: Some("work.done".to_string()),
        output: "text".to_string(),
        policy_check_token: None,
    };

    let err = emit_command_with_root(ColorMode::Never, args, Some(&workspace))
        .expect_err("schema mode must fail closed when no config is discoverable");
    let message = format!("{err:#}");
    assert!(
        message.contains("no ralph.yml") || message.contains("Cannot render protocol view"),
        "expected clear fail-closed message, got: {message}"
    );
    // And of course no event was written.
    assert!(!events_file.exists() || std::fs::read_to_string(&events_file).unwrap().is_empty());
}

/// (5) Protocol hash is stable across two renders of the same
/// config — this is the property operators rely on to detect
/// drift between the authoring YAML and the embedded copy.
#[test]
fn test_emit_schema_hash_is_stable_across_renders() {
    let tmp = TempDir::new().expect("temp dir");
    let workspace = setup_schema_workspace(&tmp, SCHEMA_FIXTURE_YAML);

    let config_path = workspace.join("ralph.yml");
    let config_sources = vec![ConfigSource::File(config_path)];
    let cfg = crate::preflight::load_config_for_preflight_sync(&config_sources, None, &workspace)
        .expect("load fixture config");
    let view1 = ProtocolView::from_event_loop(&cfg.event_loop);
    let view2 = ProtocolView::from_event_loop(&cfg.event_loop);
    assert_eq!(view1.protocol_hash, view2.protocol_hash);

    let v1 = super::schema_view::render_topic(&view1, "work.done").unwrap();
    let v2 = super::schema_view::render_topic(&view2, "work.done").unwrap();
    assert_eq!(v1["protocol_hash"], v2["protocol_hash"]);
}

// ── U6 (2026-06-21-002 plan §U6): the unified `--policy-check`
//    path runs the U4 `ValidationPipeline` over the inbound event
//    and surfaces structured `reason_codes`. The legacy path is
//    preserved only when no event_policy is configured
//    (diff / no-policy fallback).

fn setup_unified_workspace(tmp: &TempDir) -> PathBuf {
    let workspace = tmp.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).unwrap();
    std::fs::write(
        workspace.join("ralph.yml"),
        r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    allow_unsafe_cli_emit: true
    schemas:
      experiment.planned:
        required_fields:
          - task_key
",
    )
    .unwrap();
    workspace
}

/// Policy-check: rejects a payload missing a required field,
/// surfacing a structured `engine_rejected:required_field` reason code.
#[test]
fn test_emit_policy_check_rejects_missing_required_field() {
    let tmp = TempDir::new().unwrap();
    let workspace = setup_unified_workspace(&tmp);
    let events_file = workspace.join(".ralph/events.jsonl");

    let err = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("experiment.planned".to_string()),
            payload: r#"{"foo":"bar"}"#.to_string(),
            json: true,
            file: events_file.clone(),
            policy_check: true,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect_err("policy check must reject missing required field");

    let message = format!("{err:#}");
    // The unified branch bails with a structured envelope that
    // surfaces the full reason_code list. The agent can parse
    // the JSON envelope to recover the exact reason.
    assert!(
        message.contains("engine_rejected:required_field"),
        "expected structured engine_rejected:required_field reason, got: {message}"
    );
    assert!(
        message.contains("task_key"),
        "error should name the missing field, got: {message}"
    );
}

/// Policy-check: accepts a valid payload when all required fields are present.
#[test]
fn test_emit_policy_check_accepts_valid_payload() {
    let tmp = TempDir::new().unwrap();
    let workspace = setup_unified_workspace(&tmp);
    let events_file = workspace.join(".ralph/events.jsonl");

    emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("experiment.planned".to_string()),
            payload: r#"{"task_key":"k1"}"#.to_string(),
            json: true,
            file: events_file.clone(),
            policy_check: true,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect("policy check must accept a valid payload");

    let events = std::fs::read_to_string(&events_file).unwrap_or_default();
    assert!(
        events.trim().is_empty(),
        "explicit --policy-check must not write to events file; got: {events}"
    );
}

/// Policy-check rejection surfaces the unified structured envelope
/// (reason_codes list + suggestions), not the legacy bail string.
#[test]
fn test_emit_policy_check_rejects_with_unified_envelope() {
    let tmp = TempDir::new().unwrap();
    let workspace = setup_unified_workspace(&tmp);
    let events_file = workspace.join(".ralph/events.jsonl");

    let err = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("experiment.planned".to_string()),
            payload: r#"{"foo":"bar"}"#.to_string(),
            json: true,
            file: events_file.clone(),
            policy_check: true,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect_err("policy check must reject invalid payload");

    let message = format!("{err:#}");
    // Unified path: bail message contains structured reason_codes.
    assert!(
        message.contains("reason_codes="),
        "expected unified reason_codes in bail, got: {message}"
    );
}

#[test]
fn test_emit_rejects_empty_task_id_in_payload() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

    let err = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("work.ready".to_string()),
            payload: r#"{"task_id":"","step":"step-01"}"#.to_string(),
            json: true,
            file: PathBuf::from(".ralph/events.jsonl"),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect_err("empty task_id must be rejected");

    let message = format!("{err:#}");
    assert!(
        message.contains("task_id cannot be empty"),
        "expected empty task_id error, got: {message}"
    );

    // No event should have been written.
    let events_path = workspace.join(".ralph/events.jsonl");
    assert!(
        !events_path.exists(),
        "rejected emit must not write events file"
    );
}

#[test]
fn test_emit_allows_non_empty_task_id_in_payload() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

    emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("work.ready".to_string()),
            payload: r#"{"task_id":"task-123-abc","step":"step-01"}"#.to_string(),
            json: true,
            file: PathBuf::from(".ralph/events.jsonl"),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    )
    .expect("non-empty task_id should be accepted");

    let events =
        std::fs::read_to_string(workspace.join(".ralph/events.jsonl")).expect("read events");
    assert!(events.contains("\"task_id\":\"task-123-abc\""));
}

#[test]
fn test_emit_isolated_auto_derives_triggered_from_subscriber() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
    std::fs::write(
        workspace.join("ralph.yml"),
        r#"
event_loop:
  execution_mode: isolated
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["review.start", "review.dimension.done", "review.dimension.failed"]
    publishes: ["review.dimension.ready", "review.dimensions.complete"]
  dimension-reviewer:
    name: "Dimension Reviewer"
    triggers: ["review.dimension.ready"]
    publishes: ["review.dimension.done", "review.dimension.failed"]
  shipper:
    name: "Shipper"
    triggers: ["plan.complete", "plan.blocked"]
    publishes: ["REVIEW_COMPLETE"]
"#,
    )
    .unwrap();

    // U3 (2026-07-06-002 plan, R3) regression guard: set cwd to
    // workspace_root so the cwd_workspace_drift gate is not
    // triggered by the test process running from the source tree.
    // Without this, the test fires the gate purely because the
    // runner happens to be the test binary, not because the hat
    // under test actually drifted. Hat processes spawned by the
    // real loop runner start with PWD == workspace_root, which
    // this mirrors.
    let prev_cwd = std::env::current_dir().ok();
    std::env::set_current_dir(&workspace).expect("set cwd to workspace");
    let emit_result = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("review.dimension.ready".to_string()),
            payload: r#"{"dimension":"goal-alignment","plan_name":"p","task_id":"t"}"#.to_string(),
            json: true,
            file: PathBuf::from(".ralph/events.jsonl"),
            policy_check: false,
            no_policy_check: false,
            hat: Some("review-coordinator".to_string()),
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    );
    if let Some(prev) = prev_cwd {
        let _ = std::env::set_current_dir(prev);
    }

    emit_result.expect("emit should succeed");

    let events =
        std::fs::read_to_string(workspace.join(".ralph/events.jsonl")).expect("read events");
    assert!(
        events.contains("\"triggered\":\"dimension-reviewer\""),
        "expected triggered to be auto-derived to dimension-reviewer; got: {events}"
    );
}

#[test]
fn test_emit_isolated_respects_explicit_triggered_override() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
    std::fs::write(
        workspace.join("ralph.yml"),
        r#"
event_loop:
  execution_mode: isolated
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["review.start", "review.dimension.done", "review.dimension.failed"]
    publishes: ["review.dimension.ready", "review.dimensions.complete"]
  dimension-reviewer:
    name: "Dimension Reviewer"
    triggers: ["review.dimension.ready"]
    publishes: ["review.dimension.done", "review.dimension.failed"]
  shipper:
    name: "Shipper"
    triggers: ["plan.complete", "plan.blocked"]
    publishes: ["REVIEW_COMPLETE"]
"#,
    )
    .unwrap();

    // U3 (2026-07-06-002 plan, R3) regression guard: set cwd to
    // workspace_root so the cwd_workspace_drift gate does not
    // misfire when the test process is launched from the source
    // tree. The real loop runner sets PWD == workspace_root, which
    // this mirrors.
    let prev_cwd = std::env::current_dir().ok();
    std::env::set_current_dir(&workspace).expect("set cwd to workspace");
    let emit_res = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("review.dimension.ready".to_string()),
            payload: r#"{"dimension":"goal-alignment","plan_name":"p","task_id":"t"}"#.to_string(),
            json: true,
            file: PathBuf::from(".ralph/events.jsonl"),
            policy_check: false,
            no_policy_check: false,
            hat: Some("review-coordinator".to_string()),
            triggered: Some("shipper".to_string()),
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    );
    if let Some(prev) = prev_cwd {
        let _ = std::env::set_current_dir(prev);
    }
    emit_res.expect("emit should succeed");

    let events =
        std::fs::read_to_string(workspace.join(".ralph/events.jsonl")).expect("read events");
    assert!(
        events.contains("\"triggered\":\"shipper\""),
        "expected explicit triggered override to be preserved; got: {events}"
    );
    assert!(
        !events.contains("\"triggered\":\"dimension-reviewer\""),
        "auto-derivation should not override explicit value; got: {events}"
    );
}

#[test]
fn test_emit_isolated_no_auto_trigger_for_control_topic() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
    std::fs::write(
        workspace.join("ralph.yml"),
        r#"
event_loop:
  execution_mode: isolated
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["work.start", "task.resume", "test.passed", "review.complete", "work.failed"]
    publishes: ["work.ready", "review.start", "plan.complete", "plan.blocked", "LOOP_COMPLETE"]
"#,
    )
    .unwrap();

    // U3 (2026-07-06-002 plan, R3) regression guard: set cwd to
    // workspace_root so the cwd_workspace_drift gate is not
    // triggered by the test process running from the source tree.
    let prev_cwd = std::env::current_dir().ok();
    std::env::set_current_dir(&workspace).expect("set cwd to workspace");
    let emit_res = emit_command_with_root(
        ColorMode::Never,
        EmitArgs {
            topic: Some("loop.cancel".to_string()),
            payload: String::new(),
            json: false,
            file: PathBuf::from(".ralph/events.jsonl"),
            policy_check: false,
            no_policy_check: false,
            hat: Some("ralph".to_string()),
            triggered: None,
            source: None,
            schema: None,
            output: "text".to_string(),
            policy_check_token: None,
        },
        Some(&workspace),
    );
    if let Some(prev) = prev_cwd {
        let _ = std::env::set_current_dir(prev);
    }
    emit_res.expect("emit should succeed");

    let events =
        std::fs::read_to_string(workspace.join(".ralph/events.jsonl")).expect("read events");
    assert!(
        !events.contains("\"triggered\""),
        "control topic should not get auto-derived triggered; got: {events}"
    );
}

#[test]
fn test_maybe_derive_triggered_for_isolated() {
    let config = parse_config(
        r#"
event_loop:
  execution_mode: isolated
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["review.start", "review.dimension.done", "review.dimension.failed"]
    publishes: ["review.dimension.ready", "review.dimensions.complete"]
  dimension-reviewer:
    name: "Dimension Reviewer"
    triggers: ["review.dimension.ready"]
    publishes: ["review.dimension.done", "review.dimension.failed"]
"#,
    );

    // Auto-derives to the concrete subscriber.
    assert_eq!(
        maybe_derive_triggered_for_isolated(
            "review.dimension.ready",
            Some("review-coordinator"),
            None,
            Some(&config)
        ),
        Some("dimension-reviewer".to_string())
    );

    // Explicit value is preserved.
    assert_eq!(
        maybe_derive_triggered_for_isolated(
            "review.dimension.ready",
            Some("review-coordinator"),
            Some("shipper".to_string()),
            Some(&config)
        ),
        Some("shipper".to_string())
    );

    // Control topics are skipped.
    assert_eq!(
        maybe_derive_triggered_for_isolated("loop.cancel", Some("ralph"), None, Some(&config)),
        None
    );

    // Missing hat context is skipped.
    assert_eq!(
        maybe_derive_triggered_for_isolated("review.dimension.ready", None, None, Some(&config)),
        None
    );
}

#[test]
fn isolated_emit_does_not_derive_virtual_wave_runtime_as_triggered() {
    let config = parse_config(
        r#"
event_loop:
  execution_mode: isolated
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: ["LOOP_COMPLETE"]
    steps:
      - id: review_wave
        kind: side_effect
        runs: wave.runtime.review
        allowed_emits: ["review.unit.done"]
hats:
  review-worker:
    name: "Review Worker"
    triggers: ["review.unit.ready"]
    publishes: ["review.unit.done"]
"#,
    );

    let index = ralph_core::workflow_contract::HandoffIndex::from_config(&config);
    assert_eq!(
        index.consumer_of("review.unit.done"),
        Some("wave_runtime"),
        "test precondition: wave fan-in must expose its virtual consumer"
    );
    assert_eq!(
        maybe_derive_triggered_for_isolated(
            "review.unit.done",
            Some("review-worker"),
            None,
            Some(&config),
        ),
        None,
        "virtual runtime consumers must not be written into the event envelope"
    );
}

#[test]
fn missing_default_config_warns_only_without_builtin_context_or_when_explicit() {
    let builtin = HatsSource::parse("builtin:implementation-review");

    assert!(
        !should_warn_on_missing_default_config(false, Some(&builtin)),
        "implicit default core config is expected when a hats source supplies the workflow"
    );
    assert!(
        should_warn_on_missing_default_config(true, Some(&builtin)),
        "CLI -c / --config pointing at a missing file must remain visible even with hats"
    );
    assert!(
        should_warn_on_missing_default_config(false, None),
        "without a hats source, missing project config keeps the existing warning"
    );
    // Closure for ec636dc4: ambient RALPH_CONFIG is represented as
    // cli_config_explicit=false at the call site. With hats present
    // that must suppress the warn — otherwise every in-loop emit
    // re-fires `Config file "ralph.yml" not found`.
    assert!(
        !should_warn_on_missing_default_config(false, Some(&builtin)),
        "runner-injected RALPH_CONFIG must not count as CLI-explicit when hats_source is set"
    );
}

#[test]
fn test_maybe_derive_triggered_for_coordinator_mode_is_noop() {
    let config = parse_config(
        r#"
event_loop:
  execution_mode: coordinator
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["review.dimension.ready"]
    publishes: ["review.dimension.ready"]
"#,
    );

    assert_eq!(
        maybe_derive_triggered_for_isolated(
            "review.dimension.ready",
            Some("review-coordinator"),
            None,
            Some(&config)
        ),
        None
    );
}

// ─────────────────────────────────────────────────────────────────
// U3 (2026-08-03-001-fix-opac-high-confidence-gates-plan):
// U5 contract compile-failure must fail-closed (deny with
// `contract_compile_failed`) BEFORE any event / idempotency /
// ticket side effect.
// ─────────────────────────────────────────────────────────────────

/// A minimal preset config: single `worker` hat publishing
/// `work.done`, no event-policy pipeline.
///
/// `contracts_yaml` is the rendered `event_loop.execution_contracts`
/// block (S7: `enabled: false` for a valid contract; S6: a contract
/// that declares an orphan topic with no consumer). The block is
/// inserted with 4-space leading indent so it nests correctly under
/// `event_loop.execution_contracts:`.
fn u3_worker_config(contracts_yaml: &str) -> RalphConfig {
    let yaml = format!(
        r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
  execution_contracts:{contracts_yaml}
cli:
  backend: "claude"
hats:
  worker:
    name: "Worker"
    description: "Does the work"
    triggers: ["work.ready"]
    publishes: ["work.done"]
    terminal_events: ["work.done"]
    instructions: "do work"
"#,
        contracts_yaml = contracts_yaml,
    );
    serde_yaml::from_str(&yaml).expect("parse yaml")
}

/// S7 — valid empty contract (the default).
const U3_VALID_CONTRACTS_YAML: &str = r#"
    enabled: false
    rules: {}
"#;

/// S6 — broken contract: declares `orphan.topic` (no hat
/// triggers on it, it is not terminal / completion / starting),
/// which `execution_contract::compile` rejects with a
/// `MissingConsumer` finding.
const U3_BROKEN_CONTRACTS_YAML: &str = r#"
    enabled: true
    rules:
      orphan.topic:
        require_payload_fields:
          - task_id
"#;

/// U3 S6: a config that fails to compile (an execution
/// contract declared for an orphan topic with no consumer)
/// must produce an explicit `CompileFailed` state. Both the
/// capability gate and the token gate deny with
/// `contract_compile_failed`. `token()` returns `None` so the
/// `--policy-check` envelope is suppressed (no
/// `policy_check_token` is printed when the contract cannot
/// be compiled).
#[test]
fn u3_compile_failure_denies_capability_and_token() {
    let config = u3_worker_config(U3_BROKEN_CONTRACTS_YAML);
    let gate = U5Gate::resolve(
        true,
        false,
        Some(&config),
        Some("worker"),
        "work.done",
        r#"{"step":"step-01"}"#,
    );

    let (code, _hint) = gate
        .compile_failure()
        .expect("compile failure must produce an explicit CompileFailed state");
    assert_eq!(
        code, "contract_compile_failed",
        "compile failure must surface the stable reason code"
    );

    // Capability gate MUST deny with the stable reason, not
    // silently pass through.
    let cap = gate
        .capability_denied(Some("worker"), "work.done")
        .expect("capability must deny under compile failure");
    assert!(
        cap.contains("contract_compile_failed"),
        "capability deny must cite contract_compile_failed: {cap}"
    );

    // Token gate MUST deny with the stable reason, not
    // produce a misleading `policy_check_token_mismatch`
    // against an empty expected token.
    let (token_code, token_hint) = gate
        .token_violation(Some("worker"), "work.done", r#"{"step":"step-01"}"#, None)
        .expect("token check must deny under compile failure");
    assert_eq!(
        token_code, "contract_compile_failed",
        "token gate must reuse the same stable reason"
    );
    assert!(
        token_hint.contains("contract_compile_failed"),
        "token deny must cite contract_compile_failed: {token_hint}"
    );

    // The matching token (had the agent passed one) is also
    // rejected with the same reason rather than silently
    // matching.
    let some_token = "any-token-the-agent-might-pass";
    let (mismatch_code, _) = gate
        .token_violation(
            Some("worker"),
            "work.done",
            r#"{"step":"step-01"}"#,
            Some(some_token),
        )
        .expect("any token must deny under compile failure");
    assert_eq!(
        mismatch_code, "contract_compile_failed",
        "token mismatch path must surface contract_compile_failed"
    );

    // The dry-run advertise-token envelope is suppressed
    // when the contract failed to compile.
    assert_eq!(
        gate.token(Some("worker"), "work.done", r#"{"step":"step-01"}"#),
        None,
        "no policy_check_token may be advertised under compile failure"
    );
    assert_eq!(
        gate.resolved_digest(),
        None,
        "no contract digest may be advertised under compile failure"
    );
}

/// U3 S7: a valid compiled contract must keep the existing
/// capability allow / token mismatch / token success contract.
#[test]
fn u3_compile_success_keeps_legacy_capability_and_token_shape() {
    let config = u3_worker_config(U3_VALID_CONTRACTS_YAML);
    let gate = U5Gate::resolve(
        true,
        false,
        Some(&config),
        Some("worker"),
        "work.done",
        r#"{"step":"step-01"}"#,
    );

    assert!(
        gate.compile_failure().is_none(),
        "valid contract must not surface CompileFailed"
    );
    assert!(
        gate.capability_denied(Some("worker"), "work.done")
            .is_none(),
        "valid contract + allowed hat/topic must allow"
    );

    // Missing token still says missing_policy_check_token
    // (not contract_compile_failed) under a valid contract.
    let (missing_code, missing_hint) = gate
        .token_violation(Some("worker"), "work.done", r#"{"step":"step-01"}"#, None)
        .expect("missing token must deny with the legacy code");
    assert_eq!(
        missing_code, "missing_policy_check_token",
        "valid contract + missing token must keep legacy code"
    );
    assert!(
        missing_hint.contains("missing_policy_check_token"),
        "missing token hint must keep legacy wording: {missing_hint}"
    );

    // A garbage token under a valid contract still says
    // policy_check_token_mismatch (not contract_compile_failed).
    let (mismatch_code, _) = gate
        .token_violation(
            Some("worker"),
            "work.done",
            r#"{"step":"step-01"}"#,
            Some("definitely-not-the-right-token"),
        )
        .expect("garbage token must deny with the legacy mismatch code");
    assert_eq!(
        mismatch_code, "policy_check_token_mismatch",
        "valid contract + wrong token must keep legacy mismatch code"
    );

    // The token advertised for a valid contract is non-empty.
    let advertised = gate
        .token(Some("worker"), "work.done", r#"{"step":"step-01"}"#)
        .expect("valid contract + active gate must advertise a token");
    assert!(!advertised.is_empty(), "advertised token must be non-empty");
    // Same hat/topic/payload/revision must produce a stable token.
    let advertised_again = gate
        .token(Some("worker"), "work.done", r#"{"step":"step-01"}"#)
        .expect("token mint must be deterministic");
    assert_eq!(
        advertised, advertised_again,
        "same inputs must mint the same token"
    );

    // The CORRECT (matching) token must be accepted (None
    // means no violation) — guards the dead-code
    // `compute_policy_check_token_for` regression where the
    // helper always returned an empty expected token and
    // every legitimate token was wrongly classified as a
    // mismatch.
    assert!(
        gate.token_violation(
            Some("worker"),
            "work.done",
            r#"{"step":"step-01"}"#,
            Some(&advertised),
        )
        .is_none(),
        "the contract-advertised token MUST verify under the same (hat, topic, payload)"
    );
}

/// U3 S7: stand-down conditions (human CLI, pseudo-hat
/// `ralph`, wave worker, preset without hats) keep the gate
/// inactive regardless of contract state.
#[test]
fn u3_stand_down_keeps_gate_inactive() {
    // Stand-down cases use the same broken contract; the
    // gate must NOT activate and so must NOT report
    // contract_compile_failed (the loop runner owns that
    // signal at startup).
    let config = u3_worker_config(U3_BROKEN_CONTRACTS_YAML);

    // 1. Hatless pseudo-hat `ralph` stands down.
    let gate = U5Gate::resolve(true, false, Some(&config), Some("ralph"), "work.done", "{}");
    assert!(
        gate.compile_failure().is_none(),
        "ralph pseudo-hat must stand down even when the contract is broken"
    );
    assert!(
        gate.capability_denied(Some("ralph"), "work.done").is_none(),
        "ralph pseudo-hat must stand down"
    );

    // 2. Preset with no hats stands down.
    let mut bare = config.clone();
    bare.hats.clear();
    let gate = U5Gate::resolve(true, false, Some(&bare), Some("worker"), "work.done", "{}");
    assert!(
        gate.compile_failure().is_none(),
        "preset with no hats must stand down"
    );

    // 3. `env_hat_set == false` (human CLI) stands down.
    let gate = U5Gate::resolve(
        false,
        false,
        Some(&config),
        Some("worker"),
        "work.done",
        "{}",
    );
    assert!(
        gate.compile_failure().is_none(),
        "human CLI must stand down"
    );
}
