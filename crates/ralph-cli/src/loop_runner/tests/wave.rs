// U2b: wave / acp / MockAcpExecution / forced_test_wave_pty_failure 测试 + wave 特定 helper。
//
// 从 `loop_runner/tests/legacy.rs` 迁出两段:
//   - 段 1: 行 3713-6504(wave worker execution mode + execute_wave + helpers)
//   - 段 2: 行 8628-8958(test_wave_policy_rejection_* 4 个测试 + compute_agent_wrote helper)
//
// 本文件命名空间 = `loop_runner::tests::wave`,通过 `mod wave;` 在 `tests/mod.rs` 引入。
//
// Import 策略(R3 / KTD4):
//   - `use super::super::*;` 引入 `loop_runner::*` glob(execute_wave / run_wave_worker_pty
//     / extract_readable_delta / wave_worker_execution_mode / merge_wave_results_to_events_file
//     / WaveWorkerExecutionMode / BackendOutputFormat / CliBackend / EventLoop / RalphConfig
//     / runner 子模块等)。`runner` 模块名(`mod runner;`)通过 sibling access path
//     在 `loop_runner::tests::*` 命名空间下可访问,`runner::agent_wrote_any_valid_or_rejected`
//     短名调用可解析为 `loop_runner::runner::agent_wrote_any_valid_or_rejected`(`pub fn`)。
//   - `use super::common::*;` 引入 install_mock_acp_executions(common.rs 内 pub(super))
//   - `use super::fake_path::*;` 引入 write_fake_executable / install_fake_path_backends /
//     FakePathBackendsGuard / read_fake_path_backend_bin(fake_path.rs 内 pub(super))
//   - `use std::collections::HashSet;` 用于 `make_test_wave_with_timeout_and_payload` 的
//     `trigger_multi_consumer_topics: HashSet::new()`(原 `legacy.rs` 已 import)
//
// R3 锁定:本文件不重新声明 `MOCK_ACP_*` Mutex,通过 `super::common::*` 走
// `tests/common.rs` 的 `install_mock_acp_executions` / `MockAcpExecutionGuard`(共享 helper),
// 后者通过 `super::super::wave::MOCK_ACP_*` 引用 `loop_runner/wave/acp_mock.rs` 的 pub static。
//
// R6 锁定:本文件零修改 fn 签名 / 断言 / 输出;所有 `#[test]` / `#[tokio::test]` 函数
// 与对应 `#[cfg(unix)]` 属性**逐字节**迁自 `legacy.rs`。

use super::super::*;
use super::common::*;
use super::fake_path::*;
use std::collections::HashSet;

#[test]
fn test_wave_worker_execution_mode_supports_all_backend_formats() {
    assert_eq!(
        wave_worker_execution_mode(BackendOutputFormat::Text),
        WaveWorkerExecutionMode::Pty
    );
    assert_eq!(
        wave_worker_execution_mode(BackendOutputFormat::StreamJson),
        WaveWorkerExecutionMode::Pty
    );
    assert_eq!(
        wave_worker_execution_mode(BackendOutputFormat::PiStreamJson),
        WaveWorkerExecutionMode::Pty
    );
    assert_eq!(
        wave_worker_execution_mode(BackendOutputFormat::CopilotStreamJson),
        WaveWorkerExecutionMode::Pty
    );
    assert_eq!(
        wave_worker_execution_mode(BackendOutputFormat::Acp),
        WaveWorkerExecutionMode::Acp
    );
}

#[cfg(unix)]
#[test]
fn test_wave_worker_execution_mode_matches_supported_named_backend_roster() {
    for (name, expected_output_format, expected_mode, marker_id) in [
        (
            "claude",
            BackendOutputFormat::StreamJson,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:named:claude",
        ),
        (
            "pi",
            BackendOutputFormat::PiStreamJson,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:named:pi",
        ),
        (
            "kiro-acp",
            BackendOutputFormat::Acp,
            WaveWorkerExecutionMode::Acp,
            "execution-mode:named:kiro-acp",
        ),
        (
            "kiro",
            BackendOutputFormat::Text,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:named:kiro",
        ),
        (
            "gemini",
            BackendOutputFormat::Text,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:named:gemini",
        ),
        (
            "codex",
            BackendOutputFormat::Text,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:named:codex",
        ),
        (
            "amp",
            BackendOutputFormat::Text,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:named:amp",
        ),
        (
            "copilot",
            BackendOutputFormat::CopilotStreamJson,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:named:copilot",
        ),
        (
            "opencode",
            BackendOutputFormat::Text,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:named:opencode",
        ),
        (
            "roo",
            BackendOutputFormat::Text,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:named:roo",
        ),
    ] {
        let backend = CliBackend::from_name(name).expect("supported named backend");
        assert_eq!(
            backend.output_format, expected_output_format,
            "unexpected output format for {name}"
        );
        assert_eq!(
            wave_worker_execution_mode(backend.output_format),
            expected_mode,
            "unexpected wave worker execution mode for {name}"
        );
        emit_wave_validation_marker(marker_id, &["backend"]);
    }
}

#[cfg(unix)]
#[test]
fn test_wave_worker_execution_mode_matches_supported_hat_backend_families() {
    for (hat_backend, expected_output_format, expected_mode, marker_id) in [
        (
            ralph_core::HatBackend::Named("claude".to_string()),
            BackendOutputFormat::StreamJson,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:hat:named-claude",
        ),
        (
            ralph_core::HatBackend::NamedWithArgs {
                backend_type: "opencode".to_string(),
                args: vec!["--from-hat-backend".to_string()],
            },
            BackendOutputFormat::Text,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:hat:named-with-args",
        ),
        (
            ralph_core::HatBackend::KiroAgent {
                backend_type: "kiro".to_string(),
                agent: "reviewer-agent".to_string(),
                args: vec!["--kiro-extra".to_string()],
            },
            BackendOutputFormat::Text,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:hat:kiro-agent",
        ),
        (
            ralph_core::HatBackend::KiroAgent {
                backend_type: "kiro-acp".to_string(),
                agent: "reviewer-agent".to_string(),
                args: vec!["--unused-extra".to_string()],
            },
            BackendOutputFormat::Acp,
            WaveWorkerExecutionMode::Acp,
            "execution-mode:hat:kiro-acp-agent",
        ),
        (
            ralph_core::HatBackend::Custom {
                command: "/tmp/custom-wave-worker".to_string(),
                args: vec!["--from-custom-backend".to_string()],
            },
            BackendOutputFormat::Text,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:hat:custom",
        ),
    ] {
        let backend = CliBackend::from_hat_backend(&hat_backend).expect("supported hat backend");
        assert_eq!(
            backend.output_format, expected_output_format,
            "unexpected output format for {hat_backend:?}"
        );
        assert_eq!(
            wave_worker_execution_mode(backend.output_format),
            expected_mode,
            "unexpected wave worker execution mode for {hat_backend:?}"
        );
        emit_wave_validation_marker(marker_id, &["backend"]);
    }
}

#[test]
fn test_extract_readable_delta_handles_pi_stream_events() {
    let text_delta = extract_readable_delta(
        "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\"Hello from Pi\"}}",
        BackendOutputFormat::PiStreamJson,
    );
    assert_eq!(text_delta.as_deref(), Some("Hello from Pi"));

    let tool_delta = extract_readable_delta(
            "{\"type\":\"tool_execution_start\",\"toolCallId\":\"toolu_1\",\"toolName\":\"bash\",\"args\":{\"command\":\"echo hi\"}}",
            BackendOutputFormat::PiStreamJson,
        )
        .expect("pi tool start delta");
    assert!(tool_delta.contains("⚙ bash"));
    assert!(tool_delta.contains("echo hi"));

    let result_delta = extract_readable_delta(
        "{\"type\":\"tool_execution_end\",\"toolCallId\":\"toolu_1\",\"toolName\":\"bash\",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"hi\\n\"}]},\"isError\":false}",
        BackendOutputFormat::PiStreamJson,
    );
    assert_eq!(result_delta.as_deref(), Some("→ hi\n\n"));
}

#[cfg(unix)]
fn make_test_wave(publishes: Vec<String>) -> ralph_core::DetectedWave {
    make_test_wave_with_timeout(publishes, 30)
}

#[cfg(unix)]
fn make_test_wave_with_timeout(
    publishes: Vec<String>,
    timeout_secs: u32,
) -> ralph_core::DetectedWave {
    make_test_wave_with_timeout_and_payload(
        publishes,
        timeout_secs,
        "ROLE: Validate this backend".to_string(),
    )
}

#[cfg(unix)]
fn make_test_wave_with_timeout_and_payload(
    publishes: Vec<String>,
    timeout_secs: u32,
    payload: String,
) -> ralph_core::DetectedWave {
    let event = ralph_core::Event {
        topic: "review.perspective".to_string(),
        payload: Some(payload),
        ts: "2026-01-01T00:00:00Z".to_string(),
        hat: None,
        triggered: None,
        source: None,
        wave_id: Some("w-test".to_string()),
        wave_index: Some(0),
        wave_total: Some(1),
        system_injected: None,
    };

    ralph_core::DetectedWave {
        wave_id: "w-test".to_string(),
        target_hat: "reviewer".into(),
        hat_config: ralph_core::HatConfig {
            name: "Reviewer".to_string(),
            description: Some("Wave worker test".to_string()),
            triggers: vec!["review.perspective".to_string()],
            publishes,
            terminal_events: vec![],
            instructions: "Emit review.done when finished.".to_string(),
            extra_instructions: vec![],
            backend: None,
            backend_args: None,
            default_publishes: None,
            max_activations: None,
            disallowed_tools: vec![],
            timeout: Some(timeout_secs),
            // 2026-06-17-004 U2 (R3): explicit `None` for new
            // field keeps the test helper aligned with
            // `HatConfig::default()`.
            missing_event_grace_secs: None,
            concurrency: 1,
            aggregate: None,
            scratchpad: None,
            event_filter: None,
            // 2026-06-26 plan U2: test fixture does not exercise
            // the exempt list; default empty.
            exempt_topics: vec![],
            // 2026-06-29-007 plan U5a: test fixture does not
            // exercise write paths; default `None` mirrors
            // production default.
            allowed_write_paths: None,
            phase_triggers: None,
            ignore_payload_fields: vec![],
            obligations: vec![],
            trigger_multi_consumer_topics: HashSet::new(),
        },
        events: vec![event],
        total: 1,
        partial: false,
        consumer_aggregate_timeout: None,
    }
}

#[cfg(unix)]
async fn run_wave_for_backend(
    output_format: BackendOutputFormat,
    body: &str,
) -> ralph_core::CompletedWave {
    run_wave_for_backend_with_timeout(output_format, body, 30).await
}

#[cfg(unix)]
async fn run_wave_for_backend_with_timeout(
    output_format: BackendOutputFormat,
    body: &str,
    timeout_secs: u32,
) -> ralph_core::CompletedWave {
    run_wave_for_backend_with_test_env(output_format, body, timeout_secs, vec![]).await
}

#[cfg(unix)]
async fn run_wave_for_backend_with_test_env(
    output_format: BackendOutputFormat,
    body: &str,
    timeout_secs: u32,
    env_vars: Vec<(&str, &str)>,
) -> ralph_core::CompletedWave {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let bin_dir = temp_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    let worker_path = write_fake_executable(&bin_dir, "wave-worker", body);

    let backend = CliBackend {
        command: worker_path.display().to_string(),
        args: vec![],
        prompt_mode: ralph_adapters::PromptMode::Arg,
        prompt_flag: None,
        output_format,
        env_vars: env_vars
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
    };

    let events_file = temp_dir.path().join("events.jsonl");
    let wave = make_test_wave_with_timeout(vec!["review.done".to_string()], timeout_secs);
    execute_wave(
        &wave,
        &backend,
        &events_file,
        false,
        false,
        None,
        None,
        "test-loop",
        None,
    )
    .await
    .expect("wave execution")
}

#[cfg(unix)]
async fn run_wave_for_named_backend(name: &str, body: &str) -> ralph_core::CompletedWave {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let bin_dir = temp_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");

    let mut backend = CliBackend::from_name(name).expect("named backend");
    let executable_name = Path::new(&backend.command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(backend.command.as_str())
        .to_string();
    write_fake_executable(&bin_dir, &executable_name, body);

    let existing_path = std::env::var("PATH").unwrap_or_default();
    let path_value = if existing_path.is_empty() {
        bin_dir.display().to_string()
    } else {
        format!("{}:{}", bin_dir.display(), existing_path)
    };
    backend.env_vars.push(("PATH".to_string(), path_value));

    let events_file = temp_dir.path().join("events.jsonl");
    let wave = make_test_wave(vec!["review.done".to_string()]);
    execute_wave(
        &wave,
        &backend,
        &events_file,
        false,
        false,
        None,
        None,
        "test-loop",
        None,
    )
    .await
    .expect("wave execution")
}

#[cfg(unix)]
#[derive(Debug, serde::Deserialize)]
struct CapturedWaveInvocation {
    args: Vec<String>,
    env: std::collections::BTreeMap<String, String>,
    prompt: String,
}

#[cfg(unix)]
async fn run_wave_for_named_backend_with_capture(
    name: &str,
    payload: &str,
) -> (ralph_core::CompletedWave, CapturedWaveInvocation) {
    run_wave_for_named_backend_with_capture_and_task_payload(
        name,
        payload,
        "ROLE: Validate this backend",
    )
    .await
}

#[cfg(unix)]
async fn run_wave_for_named_backend_with_capture_and_task_payload(
    name: &str,
    payload: &str,
    task_payload: &str,
) -> (ralph_core::CompletedWave, CapturedWaveInvocation) {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let bin_dir = temp_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");

    let worker_capture_path = temp_dir.path().join("wave-w-test-0.jsonl.capture");
    let mut backend = CliBackend::from_name(name).expect("named backend");
    let executable_name = Path::new(&backend.command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(backend.command.as_str())
        .to_string();
    write_fake_executable(
        &bin_dir,
        &executable_name,
        &invocation_capture_backend_body(payload),
    );

    let existing_path = std::env::var("PATH").unwrap_or_default();
    let path_value = if existing_path.is_empty() {
        bin_dir.display().to_string()
    } else {
        format!("{}:{}", bin_dir.display(), existing_path)
    };
    backend.env_vars.push(("PATH".to_string(), path_value));

    let events_file = temp_dir.path().join("events.jsonl");
    let wave = make_test_wave_with_timeout_and_payload(
        vec!["review.done".to_string()],
        30,
        task_payload.to_string(),
    );
    let completed = execute_wave(
        &wave,
        &backend,
        &events_file,
        false,
        false,
        None,
        None,
        "test-loop",
        None,
    )
    .await
    .expect("wave execution");
    let captured: CapturedWaveInvocation = serde_json::from_str(
        &std::fs::read_to_string(&worker_capture_path).expect("read captured invocation"),
    )
    .expect("parse captured invocation");
    (completed, captured)
}

#[cfg(unix)]
fn missing_global_wave_backend() -> CliBackend {
    let mut backend = CliBackend {
        command: "/definitely/missing-wave-worker".to_string(),
        args: vec![],
        prompt_mode: ralph_adapters::PromptMode::Arg,
        prompt_flag: None,
        output_format: BackendOutputFormat::Text,
        env_vars: vec![],
    };

    if let Some(bin_dir) = read_fake_path_backend_bin() {
        let existing_path = std::env::var("PATH").unwrap_or_default();
        let path_value = if existing_path.is_empty() {
            bin_dir.display().to_string()
        } else {
            format!("{}:{}", bin_dir.display(), existing_path)
        };
        backend.env_vars.push(("PATH".to_string(), path_value));
    }

    backend
}

#[cfg(unix)]
async fn run_wave_for_hat_backend(
    hat_backend: ralph_core::HatBackend,
    backend_args: Option<Vec<String>>,
    global_backend: CliBackend,
) -> ralph_core::CompletedWave {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let events_file = temp_dir.path().join("events.jsonl");
    let mut wave = make_test_wave(vec!["review.done".to_string()]);
    wave.hat_config.backend = Some(hat_backend);
    wave.hat_config.backend_args = backend_args;

    execute_wave(
        &wave,
        &global_backend,
        &events_file,
        false,
        false,
        None,
        None,
        "test-loop",
        None,
    )
    .await
    .expect("wave execution")
}

#[cfg(unix)]
async fn run_wave_for_hat_backend_with_capture(
    hat_backend: ralph_core::HatBackend,
    backend_args: Option<Vec<String>>,
) -> (ralph_core::CompletedWave, CapturedWaveInvocation) {
    run_wave_for_hat_backend_with_capture_and_task_payload(
        hat_backend,
        backend_args,
        "ROLE: Validate this backend",
    )
    .await
}

#[cfg(unix)]
async fn run_wave_for_hat_backend_with_capture_and_task_payload(
    hat_backend: ralph_core::HatBackend,
    backend_args: Option<Vec<String>>,
    task_payload: &str,
) -> (ralph_core::CompletedWave, CapturedWaveInvocation) {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let worker_capture_path = temp_dir.path().join("wave-w-test-0.jsonl.capture");
    let events_file = temp_dir.path().join("events.jsonl");
    let mut wave = make_test_wave_with_timeout_and_payload(
        vec!["review.done".to_string()],
        30,
        task_payload.to_string(),
    );
    wave.hat_config.backend = Some(hat_backend);
    wave.hat_config.backend_args = backend_args;

    let completed = execute_wave(
        &wave,
        &missing_global_wave_backend(),
        &events_file,
        false,
        false,
        None,
        None,
        "test-loop",
        None,
    )
    .await
    .expect("wave execution");
    let captured: CapturedWaveInvocation = serde_json::from_str(
        &std::fs::read_to_string(&worker_capture_path).expect("read captured invocation"),
    )
    .expect("parse captured invocation");
    (completed, captured)
}

#[cfg(unix)]
#[derive(Debug, serde::Deserialize)]
struct CapturedAcpWaveInvocation {
    command: String,
    args: Vec<String>,
    env: std::collections::BTreeMap<String, String>,
    prompt: String,
}

#[cfg(unix)]
async fn run_wave_for_named_acp_backend_with_capture(
    backend_args: Option<Vec<String>>,
    payload: &str,
) -> (ralph_core::CompletedWave, CapturedAcpWaveInvocation) {
    let _mock = install_mock_acp_executions(vec![MockAcpExecution::success(
        true,
        vec![make_worker_event("review.done", payload)],
    )]);
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let worker_capture_path = temp_dir.path().join("wave-w-test-0.jsonl.capture");
    let events_file = temp_dir.path().join("events.jsonl");
    let mut wave = make_test_wave(vec!["review.done".to_string()]);
    wave.hat_config.backend_args = backend_args;
    let backend = CliBackend::from_name("kiro-acp").expect("named ACP backend");

    let completed = execute_wave(
        &wave,
        &backend,
        &events_file,
        false,
        false,
        None,
        None,
        "test-loop",
        None,
    )
    .await
    .expect("wave execution");
    let captured: CapturedAcpWaveInvocation = serde_json::from_str(
        &std::fs::read_to_string(&worker_capture_path).expect("read captured ACP invocation"),
    )
    .expect("parse captured ACP invocation");
    (completed, captured)
}

#[cfg(unix)]
async fn run_wave_for_hat_backend_with_acp_capture(
    hat_backend: ralph_core::HatBackend,
    backend_args: Option<Vec<String>>,
    payload: &str,
) -> (ralph_core::CompletedWave, CapturedAcpWaveInvocation) {
    run_wave_for_hat_backend_with_acp_capture_and_task_payload(
        hat_backend,
        backend_args,
        payload,
        "ROLE: Validate this backend",
    )
    .await
}

#[cfg(unix)]
async fn run_wave_for_hat_backend_with_acp_capture_and_task_payload(
    hat_backend: ralph_core::HatBackend,
    backend_args: Option<Vec<String>>,
    payload: &str,
    task_payload: &str,
) -> (ralph_core::CompletedWave, CapturedAcpWaveInvocation) {
    let _mock = install_mock_acp_executions(vec![MockAcpExecution::success(
        true,
        vec![make_worker_event("review.done", payload)],
    )]);
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let worker_capture_path = temp_dir.path().join("wave-w-test-0.jsonl.capture");
    let events_file = temp_dir.path().join("events.jsonl");
    let mut wave = make_test_wave_with_timeout_and_payload(
        vec!["review.done".to_string()],
        30,
        task_payload.to_string(),
    );
    wave.hat_config.backend = Some(hat_backend);
    wave.hat_config.backend_args = backend_args;

    let completed = execute_wave(
        &wave,
        &missing_global_wave_backend(),
        &events_file,
        false,
        false,
        None,
        None,
        "test-loop",
        None,
    )
    .await
    .expect("wave execution");
    let captured: CapturedAcpWaveInvocation = serde_json::from_str(
        &std::fs::read_to_string(&worker_capture_path).expect("read captured ACP invocation"),
    )
    .expect("parse captured ACP invocation");
    (completed, captured)
}

#[cfg(unix)]
fn make_worker_event(topic: &str, payload: &str) -> ralph_core::Event {
    ralph_core::Event {
        topic: topic.to_string(),
        payload: Some(payload.to_string()),
        ts: "2026-01-01T00:00:00Z".to_string(),
        hat: None,
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    }
}

#[cfg(unix)]
fn text_backend_body(payload: &str) -> String {
    format!(
        r#"printf 'plain text from worker\n'
cat <<EOF > "$RALPH_EVENTS_FILE"
{{"topic":"review.done","payload":"{payload}","ts":"2026-01-01T00:00:00Z","hat":"${{RALPH_CURRENT_HAT:-}}","source":"${{RALPH_CURRENT_HAT:-}}"}}
EOF"#,
    )
}

#[cfg(unix)]
fn claude_backend_body(payload: &str) -> String {
    format!(
        r#"printf '%s\n' \
'{{"type":"assistant","message":{{"content":[{{"type":"text","text":"hello from named claude"}}]}}}}' \
'{{"type":"result","duration_ms":1,"total_cost_usd":0.0,"num_turns":1,"is_error":false}}'
cat <<EOF > "$RALPH_EVENTS_FILE"
{{"topic":"review.done","payload":"{payload}","ts":"2026-01-01T00:00:00Z","hat":"${{RALPH_CURRENT_HAT:-}}","source":"${{RALPH_CURRENT_HAT:-}}"}}
EOF"#,
    )
}

#[cfg(unix)]
fn pi_backend_body(payload: &str) -> String {
    format!(
        r#"printf '%s\n' \
'{{"type":"message_update","assistantMessageEvent":{{"type":"text_delta","contentIndex":0,"delta":"hello from named pi"}}}}' \
'{{"type":"tool_execution_start","toolCallId":"toolu_1","toolName":"bash","args":{{"command":"echo hi"}}}}' \
'{{"type":"tool_execution_end","toolCallId":"toolu_1","toolName":"bash","result":{{"content":[{{"type":"text","text":"hi\n"}}]}},"isError":false}}'
cat <<EOF > "$RALPH_EVENTS_FILE"
{{"topic":"review.done","payload":"{payload}","ts":"2026-01-01T00:00:00Z","hat":"${{RALPH_CURRENT_HAT:-}}","source":"${{RALPH_CURRENT_HAT:-}}"}}
EOF"#,
    )
}

#[cfg(unix)]
fn invocation_capture_backend_body(payload: &str) -> String {
    format!(
        r#"python3 -c '
import json
import os
import pathlib
import select
import sys

args = sys.argv[1:]
prompt = ""
if "--prompt-file" in args:
    prompt_flag_index = args.index("--prompt-file")
    prompt = pathlib.Path(args[prompt_flag_index + 1]).read_text()
elif "--print" in args:
    chunks = []
    fd = sys.stdin.fileno()
    while True:
        ready, _, _ = select.select([fd], [], [], 2.0)
        if not ready:
            break
        chunk = os.read(fd, 65536)
        if not chunk:
            break
        chunks.append(chunk)
    prompt = b"".join(chunks).decode()
elif args:
    prompt = args[-1]
    temp_file_prefix = "Please read and execute the task in "
    if prompt.startswith(temp_file_prefix):
        prompt = pathlib.Path(prompt[len(temp_file_prefix):]).read_text()

pathlib.Path(os.environ["RALPH_EVENTS_FILE"] + ".capture").write_text(json.dumps({{
    "args": args,
    "env": {{
        "RALPH_WAVE_WORKER": os.environ.get("RALPH_WAVE_WORKER", ""),
        "RALPH_WAVE_ID": os.environ.get("RALPH_WAVE_ID", ""),
        "RALPH_WAVE_INDEX": os.environ.get("RALPH_WAVE_INDEX", ""),
        "RALPH_EVENTS_FILE": os.environ.get("RALPH_EVENTS_FILE", ""),
        "TERM": os.environ.get("TERM", ""),
        "NO_COLOR": os.environ.get("NO_COLOR", ""),
    }},
    "prompt": prompt,
}}))
' "$@"
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{{"topic":"review.done","payload":"{payload}","ts":"2026-01-01T00:00:00Z"}}
EOF"#,
    )
}

#[cfg(unix)]
enum PromptDeliveryExpectation {
    Flag(&'static str),
    Positional,
    Stdin,
    TempFileFlag(&'static str),
    TempFilePositional,
    PromptFile,
}

#[cfg(unix)]
fn assert_captured_wave_prompt(prompt: &str) {
    assert!(
        prompt.contains("# Instructions"),
        "missing instructions: {prompt}"
    );
    assert!(
        prompt.contains("Emit review.done when finished."),
        "missing worker instructions: {prompt}"
    );
    assert!(
        prompt.contains("# Wave Context"),
        "missing wave context: {prompt}"
    );
    assert!(
        prompt.contains("worker **1/1**"),
        "missing worker index: {prompt}"
    );
    assert!(prompt.contains("w-test"), "missing wave id: {prompt}");
    assert!(
        prompt.contains("# Your Task"),
        "missing task section: {prompt}"
    );
    assert!(
        prompt.contains("ROLE: Validate this backend"),
        "missing task payload: {prompt}"
    );
    assert!(
        prompt.contains("ralph emit review.done"),
        "missing publishing guidance: {prompt}"
    );
    assert!(prompt.contains("DO NOT"), "missing constraints: {prompt}");
}

#[cfg(unix)]
fn assert_captured_wave_env(
    env: &std::collections::BTreeMap<String, String>,
    expect_terminal_env: bool,
) {
    assert_eq!(env.get("RALPH_WAVE_WORKER").map(String::as_str), Some("1"));
    assert_eq!(env.get("RALPH_WAVE_ID").map(String::as_str), Some("w-test"));
    assert_eq!(env.get("RALPH_WAVE_INDEX").map(String::as_str), Some("0"));
    if expect_terminal_env {
        assert_eq!(env.get("TERM").map(String::as_str), Some("dumb"));
        assert_eq!(env.get("NO_COLOR").map(String::as_str), Some("1"));
    }
    assert!(
        env.get("RALPH_EVENTS_FILE")
            .is_some_and(|path| path.ends_with("wave-w-test-0.jsonl")),
        "missing wave events file env: {:?}",
        env
    );
}

#[cfg(unix)]
fn assert_temp_file_prompt_instruction(instruction: &str, captured_prompt: &str) {
    let prefix = "Please read and execute the task in ";
    assert!(
        instruction.starts_with(prefix),
        "expected temp-file handoff instruction, got {instruction:?}"
    );
    let path = &instruction[prefix.len()..];
    assert!(
        !path.is_empty(),
        "missing temp-file path in {instruction:?}"
    );
    assert!(
        path.starts_with('/'),
        "expected absolute temp-file path, got {instruction:?}"
    );
    assert_ne!(
        captured_prompt, instruction,
        "captured prompt should contain temp-file contents, not the handoff instruction"
    );
}

#[cfg(unix)]
fn assert_named_backend_invocation_contract(
    captured: &CapturedWaveInvocation,
    expected_prefix: &[&str],
    prompt_delivery: PromptDeliveryExpectation,
) {
    let args = captured.args.iter().map(String::as_str).collect::<Vec<_>>();

    assert_eq!(
        &args[..expected_prefix.len()],
        expected_prefix,
        "unexpected fixed args: {:?}",
        captured.args
    );

    match prompt_delivery {
        PromptDeliveryExpectation::Flag(flag) => {
            assert_eq!(
                args.len(),
                expected_prefix.len() + 2,
                "unexpected arg count: {:?}",
                captured.args
            );
            assert_eq!(args[expected_prefix.len()], flag, "missing prompt flag");
            assert_eq!(
                captured.prompt,
                args[expected_prefix.len() + 1],
                "prompt arg should match captured prompt"
            );
        }
        PromptDeliveryExpectation::Positional => {
            assert_eq!(
                args.len(),
                expected_prefix.len() + 1,
                "unexpected arg count: {:?}",
                captured.args
            );
            assert_eq!(
                captured.prompt,
                args[expected_prefix.len()],
                "positional prompt should match captured prompt"
            );
        }
        PromptDeliveryExpectation::Stdin => {
            assert_eq!(
                args.len(),
                expected_prefix.len(),
                "unexpected arg count: {:?}",
                captured.args
            );
            assert!(
                !captured.prompt.is_empty(),
                "stdin-delivered prompt should be captured"
            );
        }
        PromptDeliveryExpectation::TempFileFlag(flag) => {
            assert_eq!(
                args.len(),
                expected_prefix.len() + 2,
                "unexpected arg count: {:?}",
                captured.args
            );
            assert_eq!(args[expected_prefix.len()], flag, "missing prompt flag");
            assert_temp_file_prompt_instruction(args[expected_prefix.len() + 1], &captured.prompt);
        }
        PromptDeliveryExpectation::TempFilePositional => {
            assert_eq!(
                args.len(),
                expected_prefix.len() + 1,
                "unexpected arg count: {:?}",
                captured.args
            );
            assert_temp_file_prompt_instruction(args[expected_prefix.len()], &captured.prompt);
        }
        PromptDeliveryExpectation::PromptFile => {
            assert_eq!(
                args.len(),
                expected_prefix.len() + 2,
                "unexpected arg count: {:?}",
                captured.args
            );
            assert_eq!(
                args[expected_prefix.len()],
                "--prompt-file",
                "missing roo prompt file flag"
            );
            assert!(
                !args[expected_prefix.len() + 1].is_empty(),
                "missing roo prompt file path"
            );
            assert!(
                args[expected_prefix.len() + 1].contains("tmp")
                    || args[expected_prefix.len() + 1].contains("Temp"),
                "expected temp prompt file path, got {:?}",
                captured.args
            );
        }
    }

    assert_captured_wave_prompt(&captured.prompt);
    assert_captured_wave_env(&captured.env, true);
}

#[cfg(unix)]
fn assert_acp_invocation_contract(captured: &CapturedAcpWaveInvocation, expected_args: &[&str]) {
    assert_eq!(captured.command, "kiro-cli");
    assert_eq!(
        captured.args.iter().map(String::as_str).collect::<Vec<_>>(),
        expected_args,
        "unexpected ACP args: {:?}",
        captured.args
    );
    assert_captured_wave_prompt(&captured.prompt);
    assert_captured_wave_env(&captured.env, false);
}

#[cfg(unix)]
fn body_with_post_event_sleep(body: String) -> String {
    format!("{body}\npython3 - <<'PY'\nimport time\ntime.sleep(2)\nPY")
}

#[cfg(unix)]
macro_rules! named_text_wave_backend_test {
    ($test_name:ident, $backend_name:literal, $payload:literal) => {
        #[tokio::test]
        async fn $test_name() {
            let completed =
                run_wave_for_named_backend($backend_name, &text_backend_body($payload)).await;
            assert_single_success_marked(
                &completed,
                $payload,
                concat!("named-backend:", $backend_name),
            );
        }
    };
}

#[cfg(unix)]
fn assert_single_success(completed: &ralph_core::CompletedWave, expected_payload: &str) {
    assert!(
        completed.failures.is_empty(),
        "unexpected failures: {:?}",
        completed.failures
    );
    assert_eq!(
        completed.results.len(),
        1,
        "unexpected results: {:?}",
        completed.results
    );
    assert_eq!(completed.results[0].events.len(), 1);
    assert_eq!(completed.results[0].events[0].topic.as_str(), "review.done");
    assert_eq!(completed.results[0].events[0].payload, expected_payload);
}

#[cfg(unix)]
fn emit_wave_validation_marker(id: &str, tags: &[&str]) {
    println!("WAVE_VALIDATION_MARKER id={id} tags={}", tags.join(","));
}

#[cfg(unix)]
fn assert_single_success_marked(
    completed: &ralph_core::CompletedWave,
    expected_payload: &str,
    marker_id: &str,
) {
    assert_single_success(completed, expected_payload);
    emit_wave_validation_marker(marker_id, &["backend"]);
}

#[cfg(unix)]
fn assert_single_failure_with_synthetic_events_marked(
    completed: &ralph_core::CompletedWave,
    expected_error: &str,
    marker_id: &str,
) {
    assert!(
        completed.results.is_empty(),
        "unexpected results: {completed:?}"
    );
    assert_eq!(
        completed.failures.len(),
        1,
        "unexpected failures: {completed:?}"
    );
    assert!(
        completed.failures[0].error.contains(expected_error),
        "unexpected failure: {:?}",
        completed.failures[0]
    );

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let merged_events_path = temp_dir.path().join("events.jsonl");
    merge_wave_results_to_events_file(
        completed,
        &merged_events_path,
        &["review.done".to_string(), "review.audit".to_string()],
        "reviewer",
        // 2026-06-16-001 U2: tests use the same default.
        None,
    )
    .expect("merge wave failure results");

    let merged = std::fs::read_to_string(&merged_events_path).expect("read merged events");
    let records: Vec<serde_json::Value> = merged
        .lines()
        .map(|line| serde_json::from_str(line).expect("json event"))
        .collect();

    assert_eq!(records.len(), 3, "unexpected merged records: {records:?}");
    // 2026-06-16-001 U2: the synthetic `wave.worker.failed` payload is
    // a JSON object `{reason, wave_id, wave_index, error}` instead of a
    // free-form string. The legacy string check (`payload.contains(...)`)
    // is replaced with structured-field checks. The `error` substring
    // remains a useful correlation marker for downstream log scrapers.
    assert!(records.iter().any(|record| {
        record["topic"] == "wave.worker.failed"
            && record["hat"] == "review-synthesizer"
            && record["source"] == "review-synthesizer"
            && record["wave_index"] == 0
            // The JSONL `payload` field is a string carrying the
            // serialized JSON object. Parse it before asserting on
            // the structured fields.
            && record["payload"].as_str()
                .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
                .and_then(|obj| obj.get("error").and_then(|v| v.as_str()).map(|s| s.to_string()))
                .is_some_and(|err| err.contains(expected_error))
            && record["payload"].as_str()
                .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
                .and_then(|obj| obj.get("reason").and_then(|v| v.as_str()).map(|s| s.to_string()))
                .is_some_and(|reason| reason.starts_with("worker_failed:"))
    }));
    for topic in ["review.done", "review.audit"] {
        assert!(records.iter().any(|record| {
            record["topic"] == topic
                && record["payload"].as_str().is_some_and(|payload| {
                    payload.contains("## Worker 0 (FAILED)") && payload.contains(expected_error)
                })
        }));
    }

    emit_wave_validation_marker(marker_id, &["backend", "error", "synthetic"]);
}

#[cfg(unix)]
fn assert_partial_timeout_events_visible_marked(
    completed: &ralph_core::CompletedWave,
    expected_payload: &str,
    marker_id: &str,
) {
    assert!(
        completed.failures.is_empty(),
        "unexpected failures: {completed:?}"
    );
    assert_eq!(
        completed.results.len(),
        1,
        "unexpected results: {completed:?}"
    );
    assert_eq!(
        completed.results[0].events.len(),
        1,
        "unexpected result events: {completed:?}"
    );
    assert_eq!(completed.results[0].events[0].topic.as_str(), "review.done");
    assert_eq!(completed.results[0].events[0].payload, expected_payload);

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let merged_events_path = temp_dir.path().join("events.jsonl");
    merge_wave_results_to_events_file(
        completed,
        &merged_events_path,
        &["review.done".to_string()],
        "reviewer",
        // 2026-06-16-001 U2: tests use the same default.
        None,
    )
    .expect("merge partial-timeout results");

    let merged = std::fs::read_to_string(&merged_events_path).expect("read merged events");
    let records: Vec<serde_json::Value> = merged
        .lines()
        .map(|line| serde_json::from_str(line).expect("json event"))
        .collect();

    assert_eq!(records.len(), 1, "unexpected merged records: {records:?}");
    assert_eq!(records[0]["topic"], "review.done");
    assert_eq!(records[0]["payload"], expected_payload);
    assert!(
        records
            .iter()
            .all(|record| record["topic"] != "wave.worker.failed"),
        "partial timeout should not synthesize worker failures: {records:?}"
    );

    emit_wave_validation_marker(marker_id, &["backend", "error"]);
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_text_backend() {
    let completed = run_wave_for_backend(
        BackendOutputFormat::Text,
        r#"printf 'plain text from worker\n'
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{"topic":"review.done","payload":"text backend ok","ts":"2026-01-01T00:00:00Z"}
EOF"#,
    )
    .await;

    assert_single_success_marked(&completed, "text backend ok", "output-format:text");
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_claude_stream_json_backend() {
    let completed = run_wave_for_backend(
        BackendOutputFormat::StreamJson,
        r#"printf '%s\n' \
'{"type":"assistant","message":{"content":[{"type":"text","text":"hello from claude stream"}]}}' \
'{"type":"result","duration_ms":1,"total_cost_usd":0.0,"num_turns":1,"is_error":false}'
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{"topic":"review.done","payload":"claude stream ok","ts":"2026-01-01T00:00:00Z"}
EOF"#,
    )
    .await;

    assert_single_success_marked(
        &completed,
        "claude stream ok",
        "output-format:claude-stream-json",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_pi_stream_json_backend() {
    let completed = run_wave_for_backend(
            BackendOutputFormat::PiStreamJson,
            r#"printf '%s\n' \
'{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"hello from pi"}}' \
'{"type":"tool_execution_start","toolCallId":"toolu_1","toolName":"bash","args":{"command":"echo hi"}}' \
'{"type":"tool_execution_end","toolCallId":"toolu_1","toolName":"bash","result":{"content":[{"type":"text","text":"hi\n"}]},"isError":false}' \
'{"type":"turn_end","message":{"usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"cost":{"total":0.0}}}}'
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{"topic":"review.done","payload":"pi stream ok","ts":"2026-01-01T00:00:00Z"}
EOF"#,
        )
        .await;

    assert_single_success_marked(&completed, "pi stream ok", "output-format:pi-stream-json");
}

#[cfg(unix)]
named_text_wave_backend_test!(
    test_execute_wave_supports_named_kiro_backend,
    "kiro",
    "kiro backend ok"
);

#[cfg(unix)]
named_text_wave_backend_test!(
    test_execute_wave_supports_named_gemini_backend,
    "gemini",
    "gemini backend ok"
);

#[cfg(unix)]
named_text_wave_backend_test!(
    test_execute_wave_supports_named_codex_backend,
    "codex",
    "codex backend ok"
);

#[cfg(unix)]
named_text_wave_backend_test!(
    test_execute_wave_supports_named_amp_backend,
    "amp",
    "amp backend ok"
);

#[cfg(unix)]
named_text_wave_backend_test!(
    test_execute_wave_supports_named_copilot_backend,
    "copilot",
    "copilot backend ok"
);

#[cfg(unix)]
named_text_wave_backend_test!(
    test_execute_wave_supports_named_opencode_backend,
    "opencode",
    "opencode backend ok"
);

#[cfg(unix)]
named_text_wave_backend_test!(
    test_execute_wave_supports_named_roo_backend,
    "roo",
    "roo backend ok"
);

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_named_claude_backend() {
    let completed =
        run_wave_for_named_backend("claude", &claude_backend_body("claude backend ok")).await;
    assert_single_success_marked(&completed, "claude backend ok", "named-backend:claude");
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_named_pi_backend() {
    let completed = run_wave_for_named_backend("pi", &pi_backend_body("pi backend ok")).await;
    assert_single_success_marked(&completed, "pi backend ok", "named-backend:pi");
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_named_kiro_acp_backend() {
    let _mock = install_mock_acp_executions(vec![MockAcpExecution::success(
        true,
        vec![make_worker_event("review.done", "kiro-acp backend ok")],
    )]);

    let completed = run_wave_for_named_backend("kiro-acp", &text_backend_body("unused")).await;
    assert_single_success_marked(&completed, "kiro-acp backend ok", "named-backend:kiro-acp");
}

#[cfg(unix)]
fn large_wave_task_payload() -> String {
    format!(
        "ROLE: Validate this backend\n{}",
        "large-temp-file-wave-payload ".repeat(320)
    )
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_named_backend_invocation_contracts() {
    struct NamedBackendInvocationCase {
        name: &'static str,
        success_payload: &'static str,
        expected_prefix: &'static [&'static str],
        prompt_delivery: PromptDeliveryExpectation,
        marker_id: &'static str,
    }

    for case in [
        NamedBackendInvocationCase {
            name: "claude",
            success_payload: "claude invocation contract ok",
            expected_prefix: &[
                "--dangerously-skip-permissions",
                "--verbose",
                "--output-format",
                "stream-json",
                "--setting-sources",
                "project,local",
                "--print",
                "--disallowedTools=TodoWrite,TaskCreate,TaskUpdate,TaskList,TaskGet",
            ],
            prompt_delivery: PromptDeliveryExpectation::Stdin,
            marker_id: "invocation-contract:named:claude",
        },
        NamedBackendInvocationCase {
            name: "pi",
            success_payload: "pi invocation contract ok",
            expected_prefix: &["-p", "--mode", "json", "--no-session"],
            prompt_delivery: PromptDeliveryExpectation::Positional,
            marker_id: "invocation-contract:named:pi",
        },
        NamedBackendInvocationCase {
            name: "kiro",
            success_payload: "kiro invocation contract ok",
            expected_prefix: &["chat", "--no-interactive", "--trust-all-tools"],
            prompt_delivery: PromptDeliveryExpectation::Positional,
            marker_id: "invocation-contract:named:kiro",
        },
        NamedBackendInvocationCase {
            name: "gemini",
            success_payload: "gemini invocation contract ok",
            expected_prefix: &["--yolo"],
            prompt_delivery: PromptDeliveryExpectation::Flag("-p"),
            marker_id: "invocation-contract:named:gemini",
        },
        NamedBackendInvocationCase {
            name: "codex",
            success_payload: "codex invocation contract ok",
            expected_prefix: &["exec", "--yolo"],
            prompt_delivery: PromptDeliveryExpectation::Positional,
            marker_id: "invocation-contract:named:codex",
        },
        NamedBackendInvocationCase {
            name: "amp",
            success_payload: "amp invocation contract ok",
            expected_prefix: &["--dangerously-allow-all"],
            prompt_delivery: PromptDeliveryExpectation::Flag("-x"),
            marker_id: "invocation-contract:named:amp",
        },
        NamedBackendInvocationCase {
            name: "copilot",
            success_payload: "copilot invocation contract ok",
            expected_prefix: &["--allow-all-tools", "--output-format", "json"],
            prompt_delivery: PromptDeliveryExpectation::Flag("-p"),
            marker_id: "invocation-contract:named:copilot",
        },
        NamedBackendInvocationCase {
            name: "opencode",
            success_payload: "opencode invocation contract ok",
            expected_prefix: &["run"],
            prompt_delivery: PromptDeliveryExpectation::Positional,
            marker_id: "invocation-contract:named:opencode",
        },
        NamedBackendInvocationCase {
            name: "roo",
            success_payload: "roo invocation contract ok",
            expected_prefix: &["--print", "--ephemeral"],
            prompt_delivery: PromptDeliveryExpectation::PromptFile,
            marker_id: "invocation-contract:named:roo",
        },
    ] {
        let (completed, captured) =
            run_wave_for_named_backend_with_capture(case.name, case.success_payload).await;
        assert_single_success(&completed, case.success_payload);
        assert_named_backend_invocation_contract(
            &captured,
            case.expected_prefix,
            case.prompt_delivery,
        );
        emit_wave_validation_marker(case.marker_id, &["backend"]);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_named_backend_large_prompt_contracts() {
    struct NamedBackendLargePromptCase {
        name: &'static str,
        success_payload: &'static str,
        expected_prefix: &'static [&'static str],
        prompt_delivery: PromptDeliveryExpectation,
        marker_id: &'static str,
    }

    let task_payload = large_wave_task_payload();
    assert!(task_payload.len() > 7000, "expected large task payload");

    for case in [
        NamedBackendLargePromptCase {
            name: "claude",
            success_payload: "claude large prompt contract ok",
            expected_prefix: &[
                "--dangerously-skip-permissions",
                "--verbose",
                "--output-format",
                "stream-json",
                "--setting-sources",
                "project,local",
                "--print",
                "--disallowedTools=TodoWrite,TaskCreate,TaskUpdate,TaskList,TaskGet",
            ],
            prompt_delivery: PromptDeliveryExpectation::Stdin,
            marker_id: "large-prompt-contract:named:claude",
        },
        NamedBackendLargePromptCase {
            name: "pi",
            success_payload: "pi large prompt contract ok",
            expected_prefix: &["-p", "--mode", "json", "--no-session"],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            marker_id: "large-prompt-contract:named:pi",
        },
        NamedBackendLargePromptCase {
            name: "kiro",
            success_payload: "kiro large prompt contract ok",
            expected_prefix: &["chat", "--no-interactive", "--trust-all-tools"],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            marker_id: "large-prompt-contract:named:kiro",
        },
        NamedBackendLargePromptCase {
            name: "gemini",
            success_payload: "gemini large prompt contract ok",
            expected_prefix: &["--yolo"],
            prompt_delivery: PromptDeliveryExpectation::TempFileFlag("-p"),
            marker_id: "large-prompt-contract:named:gemini",
        },
        NamedBackendLargePromptCase {
            name: "codex",
            success_payload: "codex large prompt contract ok",
            expected_prefix: &["exec", "--yolo"],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            marker_id: "large-prompt-contract:named:codex",
        },
        NamedBackendLargePromptCase {
            name: "amp",
            success_payload: "amp large prompt contract ok",
            expected_prefix: &["--dangerously-allow-all"],
            prompt_delivery: PromptDeliveryExpectation::TempFileFlag("-x"),
            marker_id: "large-prompt-contract:named:amp",
        },
        NamedBackendLargePromptCase {
            name: "copilot",
            success_payload: "copilot large prompt contract ok",
            expected_prefix: &["--allow-all-tools", "--output-format", "json"],
            prompt_delivery: PromptDeliveryExpectation::TempFileFlag("-p"),
            marker_id: "large-prompt-contract:named:copilot",
        },
        NamedBackendLargePromptCase {
            name: "opencode",
            success_payload: "opencode large prompt contract ok",
            expected_prefix: &["run"],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            marker_id: "large-prompt-contract:named:opencode",
        },
    ] {
        let (completed, captured) = run_wave_for_named_backend_with_capture_and_task_payload(
            case.name,
            case.success_payload,
            &task_payload,
        )
        .await;
        assert_single_success(&completed, case.success_payload);
        assert_named_backend_invocation_contract(
            &captured,
            case.expected_prefix,
            case.prompt_delivery,
        );
        assert!(
            captured.prompt.contains(task_payload.trim()),
            "captured prompt should include full large task payload for {}",
            case.name
        );
        emit_wave_validation_marker(case.marker_id, &["backend"]);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_hat_backend_invocation_contracts() {
    {
        let body = invocation_capture_backend_body("hat named invocation contract ok");
        let _fake = install_fake_path_backends(&[("gemini", body.as_str())]);
        let (completed, captured) = run_wave_for_hat_backend_with_capture(
            ralph_core::HatBackend::Named("gemini".to_string()),
            Some(vec!["--hat-runtime-arg".to_string()]),
        )
        .await;

        assert_single_success(&completed, "hat named invocation contract ok");
        assert_named_backend_invocation_contract(
            &captured,
            &["--yolo", "--hat-runtime-arg"],
            PromptDeliveryExpectation::Flag("-p"),
        );
        emit_wave_validation_marker("invocation-contract:hat:named", &["backend"]);
    }

    {
        let body = invocation_capture_backend_body("hat named-with-args invocation contract ok");
        let _fake = install_fake_path_backends(&[("opencode", body.as_str())]);
        let (completed, captured) = run_wave_for_hat_backend_with_capture(
            ralph_core::HatBackend::NamedWithArgs {
                backend_type: "opencode".to_string(),
                args: vec!["--from-hat-backend".to_string()],
            },
            Some(vec!["--hat-runtime-arg".to_string()]),
        )
        .await;

        assert_single_success(&completed, "hat named-with-args invocation contract ok");
        assert_named_backend_invocation_contract(
            &captured,
            &["run", "--from-hat-backend", "--hat-runtime-arg"],
            PromptDeliveryExpectation::Positional,
        );
        emit_wave_validation_marker("invocation-contract:hat:named-with-args", &["backend"]);
    }

    {
        struct HatNamedWithArgsInvocationCase {
            backend_type: &'static str,
            executable_name: &'static str,
            extra_args: &'static [&'static str],
            expected_prefix: &'static [&'static str],
            prompt_delivery: PromptDeliveryExpectation,
            success_payload: &'static str,
            marker_id: &'static str,
        }

        for case in [
            HatNamedWithArgsInvocationCase {
                backend_type: "claude",
                executable_name: "claude",
                extra_args: &["--model", "claude-sonnet-4"],
                expected_prefix: &[
                    "--dangerously-skip-permissions",
                    "--verbose",
                    "--output-format",
                    "stream-json",
                    "--setting-sources",
                    "project,local",
                    "--print",
                    "--disallowedTools=TodoWrite,TaskCreate,TaskUpdate,TaskList,TaskGet",
                    "--model",
                    "claude-sonnet-4",
                    "--hat-runtime-arg",
                ],
                prompt_delivery: PromptDeliveryExpectation::Stdin,
                success_payload: "hat claude named-with-args invocation contract ok",
                marker_id: "invocation-contract:hat:named-with-args:claude",
            },
            HatNamedWithArgsInvocationCase {
                backend_type: "pi",
                executable_name: "pi",
                extra_args: &["--provider", "anthropic", "--model", "claude-sonnet-4"],
                expected_prefix: &[
                    "-p",
                    "--mode",
                    "json",
                    "--no-session",
                    "--provider",
                    "anthropic",
                    "--model",
                    "claude-sonnet-4",
                    "--hat-runtime-arg",
                ],
                prompt_delivery: PromptDeliveryExpectation::Positional,
                success_payload: "hat pi named-with-args invocation contract ok",
                marker_id: "invocation-contract:hat:named-with-args:pi",
            },
            HatNamedWithArgsInvocationCase {
                backend_type: "kiro",
                executable_name: "kiro-cli",
                extra_args: &["--profile", "reviewer"],
                expected_prefix: &[
                    "chat",
                    "--no-interactive",
                    "--trust-all-tools",
                    "--profile",
                    "reviewer",
                    "--hat-runtime-arg",
                ],
                prompt_delivery: PromptDeliveryExpectation::Positional,
                success_payload: "hat kiro named-with-args invocation contract ok",
                marker_id: "invocation-contract:hat:named-with-args:kiro",
            },
            HatNamedWithArgsInvocationCase {
                backend_type: "gemini",
                executable_name: "gemini",
                extra_args: &["--model", "gemini-2.5-pro"],
                expected_prefix: &["--yolo", "--model", "gemini-2.5-pro", "--hat-runtime-arg"],
                prompt_delivery: PromptDeliveryExpectation::Flag("-p"),
                success_payload: "hat gemini named-with-args invocation contract ok",
                marker_id: "invocation-contract:hat:named-with-args:gemini",
            },
            HatNamedWithArgsInvocationCase {
                backend_type: "codex",
                executable_name: "codex",
                extra_args: &["--dangerously-bypass-approvals-and-sandbox"],
                expected_prefix: &["exec", "--yolo", "--hat-runtime-arg"],
                prompt_delivery: PromptDeliveryExpectation::Positional,
                success_payload: "hat codex named-with-args invocation contract ok",
                marker_id: "invocation-contract:hat:named-with-args:codex",
            },
            HatNamedWithArgsInvocationCase {
                backend_type: "amp",
                executable_name: "amp",
                extra_args: &["--model", "gpt-5"],
                expected_prefix: &[
                    "--dangerously-allow-all",
                    "--model",
                    "gpt-5",
                    "--hat-runtime-arg",
                ],
                prompt_delivery: PromptDeliveryExpectation::Flag("-x"),
                success_payload: "hat amp named-with-args invocation contract ok",
                marker_id: "invocation-contract:hat:named-with-args:amp",
            },
            HatNamedWithArgsInvocationCase {
                backend_type: "copilot",
                executable_name: "copilot",
                extra_args: &["--model", "gpt-5"],
                expected_prefix: &[
                    "--allow-all-tools",
                    "--output-format",
                    "json",
                    "--model",
                    "gpt-5",
                    "--hat-runtime-arg",
                ],
                prompt_delivery: PromptDeliveryExpectation::Flag("-p"),
                success_payload: "hat copilot named-with-args invocation contract ok",
                marker_id: "invocation-contract:hat:named-with-args:copilot",
            },
            HatNamedWithArgsInvocationCase {
                backend_type: "roo",
                executable_name: "roo",
                extra_args: &["--model", "claude-sonnet-4"],
                expected_prefix: &[
                    "--print",
                    "--ephemeral",
                    "--model",
                    "claude-sonnet-4",
                    "--hat-runtime-arg",
                ],
                prompt_delivery: PromptDeliveryExpectation::PromptFile,
                success_payload: "hat roo named-with-args invocation contract ok",
                marker_id: "invocation-contract:hat:named-with-args:roo",
            },
        ] {
            let body = invocation_capture_backend_body(case.success_payload);
            let _fake = install_fake_path_backends(&[(case.executable_name, body.as_str())]);
            let (completed, captured) = run_wave_for_hat_backend_with_capture(
                ralph_core::HatBackend::NamedWithArgs {
                    backend_type: case.backend_type.to_string(),
                    args: case
                        .extra_args
                        .iter()
                        .map(|arg| (*arg).to_string())
                        .collect(),
                },
                Some(vec!["--hat-runtime-arg".to_string()]),
            )
            .await;

            assert_single_success(&completed, case.success_payload);
            assert_named_backend_invocation_contract(
                &captured,
                case.expected_prefix,
                case.prompt_delivery,
            );
            emit_wave_validation_marker(case.marker_id, &["backend"]);
        }
    }

    {
        let body = invocation_capture_backend_body("hat kiro agent invocation contract ok");
        let _fake = install_fake_path_backends(&[("kiro-cli", body.as_str())]);
        let (completed, captured) = run_wave_for_hat_backend_with_capture(
            ralph_core::HatBackend::KiroAgent {
                backend_type: "kiro".to_string(),
                agent: "reviewer-agent".to_string(),
                args: vec!["--kiro-extra".to_string()],
            },
            Some(vec!["--hat-runtime-arg".to_string()]),
        )
        .await;

        assert_single_success(&completed, "hat kiro agent invocation contract ok");
        assert_named_backend_invocation_contract(
            &captured,
            &[
                "chat",
                "--no-interactive",
                "--trust-all-tools",
                "--agent",
                "reviewer-agent",
                "--kiro-extra",
                "--hat-runtime-arg",
            ],
            PromptDeliveryExpectation::Positional,
        );
        emit_wave_validation_marker("invocation-contract:hat:kiro-agent", &["backend"]);
    }

    {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let body = invocation_capture_backend_body("hat custom invocation contract ok");
        let worker_path = write_fake_executable(temp_dir.path(), "custom-wave-worker", &body);
        let (completed, captured) = run_wave_for_hat_backend_with_capture(
            ralph_core::HatBackend::Custom {
                command: worker_path.display().to_string(),
                args: vec!["--from-custom-backend".to_string()],
            },
            Some(vec!["--hat-runtime-arg".to_string()]),
        )
        .await;

        assert_single_success(&completed, "hat custom invocation contract ok");
        assert_named_backend_invocation_contract(
            &captured,
            &["--from-custom-backend", "--hat-runtime-arg"],
            PromptDeliveryExpectation::Positional,
        );
        emit_wave_validation_marker("invocation-contract:hat:custom", &["backend"]);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_hat_backend_large_prompt_contracts() {
    struct HatNamedLargePromptCase {
        backend_type: &'static str,
        executable_name: &'static str,
        expected_prefix: &'static [&'static str],
        prompt_delivery: PromptDeliveryExpectation,
        success_payload: &'static str,
        marker_id: &'static str,
    }

    struct HatNamedWithArgsLargePromptCase {
        backend_type: &'static str,
        executable_name: &'static str,
        extra_args: &'static [&'static str],
        expected_prefix: &'static [&'static str],
        prompt_delivery: PromptDeliveryExpectation,
        success_payload: &'static str,
        marker_id: &'static str,
    }

    let task_payload = large_wave_task_payload();
    assert!(task_payload.len() > 7000, "expected large task payload");

    for case in [
        HatNamedLargePromptCase {
            backend_type: "claude",
            executable_name: "claude",
            expected_prefix: &[
                "--dangerously-skip-permissions",
                "--verbose",
                "--output-format",
                "stream-json",
                "--setting-sources",
                "project,local",
                "--print",
                "--disallowedTools=TodoWrite,TaskCreate,TaskUpdate,TaskList,TaskGet",
                "--hat-runtime-arg",
            ],
            prompt_delivery: PromptDeliveryExpectation::Stdin,
            success_payload: "hat claude named large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named:claude",
        },
        HatNamedLargePromptCase {
            backend_type: "pi",
            executable_name: "pi",
            expected_prefix: &["-p", "--mode", "json", "--no-session", "--hat-runtime-arg"],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            success_payload: "hat pi named large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named:pi",
        },
        HatNamedLargePromptCase {
            backend_type: "kiro",
            executable_name: "kiro-cli",
            expected_prefix: &[
                "chat",
                "--no-interactive",
                "--trust-all-tools",
                "--hat-runtime-arg",
            ],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            success_payload: "hat kiro named large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named:kiro",
        },
        HatNamedLargePromptCase {
            backend_type: "gemini",
            executable_name: "gemini",
            expected_prefix: &["--yolo", "--hat-runtime-arg"],
            prompt_delivery: PromptDeliveryExpectation::TempFileFlag("-p"),
            success_payload: "hat gemini named large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named:gemini",
        },
        HatNamedLargePromptCase {
            backend_type: "codex",
            executable_name: "codex",
            expected_prefix: &["exec", "--yolo", "--hat-runtime-arg"],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            success_payload: "hat codex named large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named:codex",
        },
        HatNamedLargePromptCase {
            backend_type: "amp",
            executable_name: "amp",
            expected_prefix: &["--dangerously-allow-all", "--hat-runtime-arg"],
            prompt_delivery: PromptDeliveryExpectation::TempFileFlag("-x"),
            success_payload: "hat amp named large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named:amp",
        },
        HatNamedLargePromptCase {
            backend_type: "copilot",
            executable_name: "copilot",
            expected_prefix: &[
                "--allow-all-tools",
                "--output-format",
                "json",
                "--hat-runtime-arg",
            ],
            prompt_delivery: PromptDeliveryExpectation::TempFileFlag("-p"),
            success_payload: "hat copilot named large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named:copilot",
        },
        HatNamedLargePromptCase {
            backend_type: "opencode",
            executable_name: "opencode",
            expected_prefix: &["run", "--hat-runtime-arg"],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            success_payload: "hat opencode named large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named:opencode",
        },
        HatNamedLargePromptCase {
            backend_type: "roo",
            executable_name: "roo",
            expected_prefix: &["--print", "--ephemeral", "--hat-runtime-arg"],
            prompt_delivery: PromptDeliveryExpectation::PromptFile,
            success_payload: "hat roo named large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named:roo",
        },
    ] {
        let body = invocation_capture_backend_body(case.success_payload);
        let _fake = install_fake_path_backends(&[(case.executable_name, body.as_str())]);
        let (completed, captured) = run_wave_for_hat_backend_with_capture_and_task_payload(
            ralph_core::HatBackend::Named(case.backend_type.to_string()),
            Some(vec!["--hat-runtime-arg".to_string()]),
            &task_payload,
        )
        .await;

        assert_single_success(&completed, case.success_payload);
        assert_named_backend_invocation_contract(
            &captured,
            case.expected_prefix,
            case.prompt_delivery,
        );
        // build_wave_worker_prompt trims the payload, so compare against the trimmed form
        assert!(
            captured.prompt.contains(task_payload.trim()),
            "captured prompt should include full large task payload for {}",
            case.backend_type
        );
        emit_wave_validation_marker(case.marker_id, &["backend"]);
    }

    {
        let (completed, captured) = run_wave_for_hat_backend_with_acp_capture_and_task_payload(
            ralph_core::HatBackend::Named("kiro-acp".to_string()),
            Some(vec!["--hat-runtime-arg".to_string()]),
            "hat kiro-acp named large prompt contract ok",
            &task_payload,
        )
        .await;

        assert_single_success(&completed, "hat kiro-acp named large prompt contract ok");
        assert_acp_invocation_contract(&captured, &["acp", "--hat-runtime-arg"]);
        // build_wave_worker_prompt trims the payload, so compare against the trimmed form
        assert!(
            captured.prompt.contains(task_payload.trim()),
            "captured prompt should include full large task payload for kiro-acp named hat"
        );
        emit_wave_validation_marker("large-prompt-contract:hat:named:kiro-acp", &["backend"]);
    }

    for case in [
        HatNamedWithArgsLargePromptCase {
            backend_type: "claude",
            executable_name: "claude",
            extra_args: &["--model", "claude-sonnet-4"],
            expected_prefix: &[
                "--dangerously-skip-permissions",
                "--verbose",
                "--output-format",
                "stream-json",
                "--setting-sources",
                "project,local",
                "--print",
                "--disallowedTools=TodoWrite,TaskCreate,TaskUpdate,TaskList,TaskGet",
                "--model",
                "claude-sonnet-4",
                "--hat-runtime-arg",
            ],
            prompt_delivery: PromptDeliveryExpectation::Stdin,
            success_payload: "hat claude named-with-args large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named-with-args:claude",
        },
        HatNamedWithArgsLargePromptCase {
            backend_type: "pi",
            executable_name: "pi",
            extra_args: &["--provider", "anthropic", "--model", "claude-sonnet-4"],
            expected_prefix: &[
                "-p",
                "--mode",
                "json",
                "--no-session",
                "--provider",
                "anthropic",
                "--model",
                "claude-sonnet-4",
                "--hat-runtime-arg",
            ],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            success_payload: "hat pi named-with-args large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named-with-args:pi",
        },
        HatNamedWithArgsLargePromptCase {
            backend_type: "kiro",
            executable_name: "kiro-cli",
            extra_args: &["--profile", "reviewer"],
            expected_prefix: &[
                "chat",
                "--no-interactive",
                "--trust-all-tools",
                "--profile",
                "reviewer",
                "--hat-runtime-arg",
            ],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            success_payload: "hat kiro named-with-args large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named-with-args:kiro",
        },
        HatNamedWithArgsLargePromptCase {
            backend_type: "gemini",
            executable_name: "gemini",
            extra_args: &["--model", "gemini-2.5-pro"],
            expected_prefix: &["--yolo", "--model", "gemini-2.5-pro", "--hat-runtime-arg"],
            prompt_delivery: PromptDeliveryExpectation::TempFileFlag("-p"),
            success_payload: "hat gemini named-with-args large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named-with-args:gemini",
        },
        HatNamedWithArgsLargePromptCase {
            backend_type: "codex",
            executable_name: "codex",
            extra_args: &["--dangerously-bypass-approvals-and-sandbox"],
            expected_prefix: &["exec", "--yolo", "--hat-runtime-arg"],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            success_payload: "hat codex named-with-args large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named-with-args:codex",
        },
        HatNamedWithArgsLargePromptCase {
            backend_type: "amp",
            executable_name: "amp",
            extra_args: &["--model", "gpt-5"],
            expected_prefix: &[
                "--dangerously-allow-all",
                "--model",
                "gpt-5",
                "--hat-runtime-arg",
            ],
            prompt_delivery: PromptDeliveryExpectation::TempFileFlag("-x"),
            success_payload: "hat amp named-with-args large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named-with-args:amp",
        },
        HatNamedWithArgsLargePromptCase {
            backend_type: "copilot",
            executable_name: "copilot",
            extra_args: &["--model", "gpt-5"],
            expected_prefix: &[
                "--allow-all-tools",
                "--output-format",
                "json",
                "--model",
                "gpt-5",
                "--hat-runtime-arg",
            ],
            prompt_delivery: PromptDeliveryExpectation::TempFileFlag("-p"),
            success_payload: "hat copilot named-with-args large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named-with-args:copilot",
        },
        HatNamedWithArgsLargePromptCase {
            backend_type: "roo",
            executable_name: "roo",
            extra_args: &["--model", "claude-sonnet-4"],
            expected_prefix: &[
                "--print",
                "--ephemeral",
                "--model",
                "claude-sonnet-4",
                "--hat-runtime-arg",
            ],
            prompt_delivery: PromptDeliveryExpectation::PromptFile,
            success_payload: "hat roo named-with-args large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named-with-args:roo",
        },
    ] {
        let body = invocation_capture_backend_body(case.success_payload);
        let _fake = install_fake_path_backends(&[(case.executable_name, body.as_str())]);
        let (completed, captured) = run_wave_for_hat_backend_with_capture_and_task_payload(
            ralph_core::HatBackend::NamedWithArgs {
                backend_type: case.backend_type.to_string(),
                args: case
                    .extra_args
                    .iter()
                    .map(|arg| (*arg).to_string())
                    .collect(),
            },
            Some(vec!["--hat-runtime-arg".to_string()]),
            &task_payload,
        )
        .await;

        assert_single_success(&completed, case.success_payload);
        assert_named_backend_invocation_contract(
            &captured,
            case.expected_prefix,
            case.prompt_delivery,
        );
        assert!(
            captured.prompt.contains(task_payload.trim()),
            "captured prompt should include full large task payload for {}",
            case.backend_type
        );
        emit_wave_validation_marker(case.marker_id, &["backend"]);
    }

    {
        let body = invocation_capture_backend_body("hat kiro agent large prompt contract ok");
        let _fake = install_fake_path_backends(&[("kiro-cli", body.as_str())]);
        let (completed, captured) = run_wave_for_hat_backend_with_capture_and_task_payload(
            ralph_core::HatBackend::KiroAgent {
                backend_type: "kiro".to_string(),
                agent: "reviewer-agent".to_string(),
                args: vec!["--kiro-extra".to_string()],
            },
            Some(vec!["--hat-runtime-arg".to_string()]),
            &task_payload,
        )
        .await;

        assert_single_success(&completed, "hat kiro agent large prompt contract ok");
        assert_named_backend_invocation_contract(
            &captured,
            &[
                "chat",
                "--no-interactive",
                "--trust-all-tools",
                "--agent",
                "reviewer-agent",
                "--kiro-extra",
                "--hat-runtime-arg",
            ],
            PromptDeliveryExpectation::TempFilePositional,
        );
        assert!(
            captured.prompt.contains(task_payload.trim()),
            "captured prompt should include full large task payload for kiro agent hat"
        );
        emit_wave_validation_marker("large-prompt-contract:hat:kiro-agent:kiro", &["backend"]);
    }

    {
        let (completed, captured) = run_wave_for_hat_backend_with_acp_capture_and_task_payload(
            ralph_core::HatBackend::KiroAgent {
                backend_type: "kiro-acp".to_string(),
                agent: "reviewer-agent".to_string(),
                args: vec!["--ignored-extra".to_string()],
            },
            Some(vec!["--hat-runtime-arg".to_string()]),
            "hat kiro-acp agent large prompt contract ok",
            &task_payload,
        )
        .await;

        assert_single_success(&completed, "hat kiro-acp agent large prompt contract ok");
        assert_acp_invocation_contract(
            &captured,
            &["acp", "--agent", "reviewer-agent", "--hat-runtime-arg"],
        );
        assert!(
            captured.prompt.contains(task_payload.trim()),
            "captured prompt should include full large task payload for kiro-acp agent hat"
        );
        emit_wave_validation_marker(
            "large-prompt-contract:hat:kiro-agent:kiro-acp",
            &["backend"],
        );
    }

    {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let body = invocation_capture_backend_body("hat custom large prompt contract ok");
        let worker_path = write_fake_executable(temp_dir.path(), "custom-wave-worker", &body);
        let (completed, captured) = run_wave_for_hat_backend_with_capture_and_task_payload(
            ralph_core::HatBackend::Custom {
                command: worker_path.display().to_string(),
                args: vec!["--from-custom-backend".to_string()],
            },
            Some(vec!["--hat-runtime-arg".to_string()]),
            &task_payload,
        )
        .await;

        assert_single_success(&completed, "hat custom large prompt contract ok");
        assert_named_backend_invocation_contract(
            &captured,
            &["--from-custom-backend", "--hat-runtime-arg"],
            PromptDeliveryExpectation::TempFilePositional,
        );
        assert!(
            captured.prompt.contains(task_payload.trim()),
            "captured prompt should include full large task payload for custom backend"
        );
        emit_wave_validation_marker("large-prompt-contract:hat:custom", &["backend"]);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_acp_backend_invocation_contracts() {
    {
        let (completed, captured) = run_wave_for_named_acp_backend_with_capture(
            Some(vec!["--hat-runtime-arg".to_string()]),
            "named ACP invocation contract ok",
        )
        .await;

        assert_single_success(&completed, "named ACP invocation contract ok");
        assert_acp_invocation_contract(&captured, &["acp", "--hat-runtime-arg"]);
        emit_wave_validation_marker("invocation-contract:acp:named:kiro-acp", &["backend"]);
    }

    {
        let (completed, captured) = run_wave_for_hat_backend_with_acp_capture(
            ralph_core::HatBackend::KiroAgent {
                backend_type: "kiro-acp".to_string(),
                agent: "reviewer-agent".to_string(),
                args: vec!["--ignored-extra".to_string()],
            },
            Some(vec!["--hat-runtime-arg".to_string()]),
            "hat ACP kiro-agent invocation contract ok",
        )
        .await;

        assert_single_success(&completed, "hat ACP kiro-agent invocation contract ok");
        assert_acp_invocation_contract(
            &captured,
            &["acp", "--agent", "reviewer-agent", "--hat-runtime-arg"],
        );
        emit_wave_validation_marker("invocation-contract:acp:hat:kiro-agent", &["backend"]);
    }

    {
        let (completed, captured) = run_wave_for_hat_backend_with_acp_capture(
            ralph_core::HatBackend::NamedWithArgs {
                backend_type: "kiro-acp".to_string(),
                args: vec!["--model".to_string(), "claude-sonnet-4".to_string()],
            },
            Some(vec!["--hat-runtime-arg".to_string()]),
            "hat ACP named-with-args invocation contract ok",
        )
        .await;

        assert_single_success(&completed, "hat ACP named-with-args invocation contract ok");
        assert_acp_invocation_contract(
            &captured,
            &["acp", "--model", "claude-sonnet-4", "--hat-runtime-arg"],
        );
        emit_wave_validation_marker("invocation-contract:acp:hat:named-with-args", &["backend"]);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_surfaces_named_kiro_acp_executor_error_with_synthetic_events() {
    let _mock = install_mock_acp_executions(vec![MockAcpExecution::error(
        "mock acp executor exploded",
        vec![],
    )]);

    let completed = run_wave_for_named_backend("kiro-acp", &text_backend_body("unused")).await;

    assert_single_failure_with_synthetic_events_marked(
        &completed,
        "mock acp executor exploded",
        "acp:named-executor-error",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_surfaces_hat_kiro_acp_timeout_without_events_with_synthetic_events() {
    let _mock = install_mock_acp_executions(vec![MockAcpExecution::timeout(vec![])]);

    let completed = run_wave_for_hat_backend(
        ralph_core::HatBackend::KiroAgent {
            backend_type: "kiro-acp".to_string(),
            agent: "reviewer-agent".to_string(),
            args: vec!["--unused-extra".to_string()],
        },
        Some(vec!["--hat-runtime-arg".to_string()]),
        missing_global_wave_backend(),
    )
    .await;

    assert_single_failure_with_synthetic_events_marked(
        &completed,
        "Worker timed out after 30s without emitting events",
        "acp:hat-timeout-without-events",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_hat_backend_named_with_backend_args() {
    let _fake = install_fake_path_backends(&[(
        "gemini",
        r#"found_hat_arg=0
for arg in "$@"; do
  if [ "$arg" = "--hat-runtime-arg" ]; then
    found_hat_arg=1
  fi
done
if [ "$found_hat_arg" -ne 1 ]; then
  echo "missing --hat-runtime-arg: $*" >&2
  exit 1
fi
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{"topic":"review.done","payload":"hat named backend ok","ts":"2026-01-01T00:00:00Z"}
EOF"#,
    )]);

    let completed = run_wave_for_hat_backend(
        ralph_core::HatBackend::Named("gemini".to_string()),
        Some(vec!["--hat-runtime-arg".to_string()]),
        missing_global_wave_backend(),
    )
    .await;

    assert_single_success_marked(&completed, "hat named backend ok", "hat-backend:named");
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_hat_backend_named_with_args_and_backend_args() {
    let _fake = install_fake_path_backends(&[(
        "opencode",
        r#"found_hat_backend_arg=0
found_hat_runtime_arg=0
for arg in "$@"; do
  if [ "$arg" = "--from-hat-backend" ]; then
    found_hat_backend_arg=1
  fi
  if [ "$arg" = "--hat-runtime-arg" ]; then
    found_hat_runtime_arg=1
  fi
done
if [ "$found_hat_backend_arg" -ne 1 ] || [ "$found_hat_runtime_arg" -ne 1 ]; then
  echo "missing expected args: $*" >&2
  exit 1
fi
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{"topic":"review.done","payload":"hat named-with-args backend ok","ts":"2026-01-01T00:00:00Z"}
EOF"#,
    )]);

    let completed = run_wave_for_hat_backend(
        ralph_core::HatBackend::NamedWithArgs {
            backend_type: "opencode".to_string(),
            args: vec!["--from-hat-backend".to_string()],
        },
        Some(vec!["--hat-runtime-arg".to_string()]),
        missing_global_wave_backend(),
    )
    .await;

    assert_single_success_marked(
        &completed,
        "hat named-with-args backend ok",
        "hat-backend:named-with-args",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_hat_backend_kiro_agent_with_backend_args() {
    let _fake = install_fake_path_backends(&[(
        "kiro-cli",
        r#"found_agent=0
found_kiro_arg=0
found_hat_runtime_arg=0
prev=''
for arg in "$@"; do
  if [ "$prev" = "--agent" ] && [ "$arg" = "reviewer-agent" ]; then
    found_agent=1
  fi
  if [ "$arg" = "--kiro-extra" ]; then
    found_kiro_arg=1
  fi
  if [ "$arg" = "--hat-runtime-arg" ]; then
    found_hat_runtime_arg=1
  fi
  prev="$arg"
done
if [ "$found_agent" -ne 1 ] || [ "$found_kiro_arg" -ne 1 ] || [ "$found_hat_runtime_arg" -ne 1 ]; then
  echo "missing expected kiro args: $*" >&2
  exit 1
fi
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{"topic":"review.done","payload":"hat kiro agent backend ok","ts":"2026-01-01T00:00:00Z"}
EOF"#,
    )]);

    let completed = run_wave_for_hat_backend(
        ralph_core::HatBackend::KiroAgent {
            backend_type: "kiro".to_string(),
            agent: "reviewer-agent".to_string(),
            args: vec!["--kiro-extra".to_string()],
        },
        Some(vec!["--hat-runtime-arg".to_string()]),
        missing_global_wave_backend(),
    )
    .await;

    assert_single_success_marked(
        &completed,
        "hat kiro agent backend ok",
        "hat-backend:kiro-agent",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_hat_backend_kiro_acp_agent() {
    let _mock = install_mock_acp_executions(vec![MockAcpExecution::success(
        true,
        vec![make_worker_event(
            "review.done",
            "hat kiro-acp agent backend ok",
        )],
    )]);

    let completed = run_wave_for_hat_backend(
        ralph_core::HatBackend::KiroAgent {
            backend_type: "kiro-acp".to_string(),
            agent: "reviewer-agent".to_string(),
            args: vec!["--unused-extra".to_string()],
        },
        Some(vec!["--hat-runtime-arg".to_string()]),
        missing_global_wave_backend(),
    )
    .await;

    assert_single_success_marked(
        &completed,
        "hat kiro-acp agent backend ok",
        "hat-backend:kiro-acp-agent",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_custom_hat_backend_with_backend_args() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let worker_path = write_fake_executable(
        temp_dir.path(),
        "custom-wave-worker",
        r#"found_custom_arg=0
found_hat_runtime_arg=0
for arg in "$@"; do
  if [ "$arg" = "--from-custom-backend" ]; then
    found_custom_arg=1
  fi
  if [ "$arg" = "--hat-runtime-arg" ]; then
    found_hat_runtime_arg=1
  fi
done
if [ "$found_custom_arg" -ne 1 ] || [ "$found_hat_runtime_arg" -ne 1 ]; then
  echo "missing expected custom args: $*" >&2
  exit 1
fi
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{"topic":"review.done","payload":"hat custom backend ok","ts":"2026-01-01T00:00:00Z"}
EOF"#,
    );

    let completed = run_wave_for_hat_backend(
        ralph_core::HatBackend::Custom {
            command: worker_path.display().to_string(),
            args: vec!["--from-custom-backend".to_string()],
        },
        Some(vec!["--hat-runtime-arg".to_string()]),
        missing_global_wave_backend(),
    )
    .await;

    assert_single_success_marked(&completed, "hat custom backend ok", "hat-backend:custom");
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_synthesizes_failure_events_for_missing_custom_hat_backend_command() {
    let completed = run_wave_for_hat_backend(
        ralph_core::HatBackend::Custom {
            command: "/definitely/missing-custom-wave-worker".to_string(),
            args: vec!["--from-custom-backend".to_string()],
        },
        Some(vec!["--hat-runtime-arg".to_string()]),
        missing_global_wave_backend(),
    )
    .await;

    assert_single_failure_with_synthetic_events_marked(
        &completed,
        "missing-custom-wave-worker",
        "hat-backend:custom-missing-command",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_synthesizes_failure_events_for_missing_text_backend_command() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let events_file = temp_dir.path().join("events.jsonl");
    let wave = make_test_wave(vec!["review.done".to_string()]);

    let completed = execute_wave(
        &wave,
        &missing_global_wave_backend(),
        &events_file,
        false,
        false,
        None,
        None,
        "test-loop",
        None,
    )
    .await
    .expect("wave execution");

    assert_single_failure_with_synthetic_events_marked(
        &completed,
        "missing-wave-worker",
        "execution-mode:pty-spawn-failure-visible",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_synthesizes_failure_events_for_pty_open_failure() {
    let completed = run_wave_for_backend_with_test_env(
        BackendOutputFormat::Text,
        &text_backend_body("unused"),
        30,
        vec![("RALPH_TEST_FORCE_PTY_OPEN_FAIL", "mock openpty exploded")],
    )
    .await;

    assert_single_failure_with_synthetic_events_marked(
        &completed,
        "mock openpty exploded",
        "pty:open-failure-visible",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_synthesizes_failure_events_for_pty_reader_failure() {
    let completed = run_wave_for_backend_with_test_env(
        BackendOutputFormat::Text,
        &text_backend_body("unused"),
        30,
        vec![(
            "RALPH_TEST_FORCE_PTY_READER_FAIL",
            "mock reader clone exploded",
        )],
    )
    .await;

    assert_single_failure_with_synthetic_events_marked(
        &completed,
        "mock reader clone exploded",
        "pty:reader-failure-visible",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_falls_back_to_global_backend_when_hat_backend_is_invalid() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let worker_path = write_fake_executable(
        temp_dir.path(),
        "wave-worker",
        r#"found_fallback_arg=0
for arg in "$@"; do
  if [ "$arg" = "--fallback-arg" ]; then
    found_fallback_arg=1
  fi
done
if [ "$found_fallback_arg" -ne 1 ]; then
  echo "missing --fallback-arg: $*" >&2
  exit 1
fi
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{"topic":"review.done","payload":"hat backend fallback ok","ts":"2026-01-01T00:00:00Z"}
EOF"#,
    );
    let global_backend = CliBackend {
        command: worker_path.display().to_string(),
        args: vec![],
        prompt_mode: ralph_adapters::PromptMode::Arg,
        prompt_flag: None,
        output_format: BackendOutputFormat::Text,
        env_vars: vec![],
    };

    let completed = run_wave_for_hat_backend(
        ralph_core::HatBackend::Named("definitely-invalid-backend".to_string()),
        Some(vec!["--fallback-arg".to_string()]),
        global_backend,
    )
    .await;

    assert_single_success_marked(
        &completed,
        "hat backend fallback ok",
        "hat-backend:invalid-fallback",
    );
    emit_wave_validation_marker("hat-backend:invalid-fallback", &["error"]);
}

#[cfg(unix)]
#[tokio::test]
async fn test_run_wave_worker_acp_timeout_with_partial_events_keeps_events_visible() {
    let _mock =
        install_mock_acp_executions(vec![MockAcpExecution::timeout(vec![make_worker_event(
            "review.done",
            "partial acp result",
        )])]);
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let worker_events_path = temp_dir.path().join("wave-events.jsonl");
    let merged_events_path = temp_dir.path().join("events.jsonl");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let backend = CliBackend::kiro_acp();

    let (_index, outcome) = run_wave_worker_acp(
        0,
        &backend,
        "prompt",
        &worker_events_path,
        Duration::from_millis(1),
        tx,
        None,
        None,
        None,
    )
    .await;

    let (events, _duration, success) =
        outcome.expect("partial ACP timeout should preserve emitted events");
    assert!(!success, "timed out worker should not be marked successful");
    assert_eq!(events.len(), 1, "unexpected partial events: {events:?}");
    assert_eq!(events[0].topic.as_str(), "review.done");
    assert_eq!(events[0].payload.as_deref(), Some("partial acp result"));

    let completed = ralph_core::CompletedWave {
        wave_id: "w-acp".to_string(),
        wave_total: 1,
        results: vec![ralph_core::WaveResult {
            index: 0,
            events: events.into_iter().map(ralph_proto::Event::from).collect(),
        }],
        failures: vec![],
        duration: Duration::from_millis(1),
        partial: false,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: Vec::new(),
    };

    merge_wave_results_to_events_file(
        &completed,
        &merged_events_path,
        &["review.done".to_string()],
        "reviewer",
        // 2026-06-16-001 U2: tests use the same default.
        None,
    )
    .expect("merge partial ACP results");

    let merged = std::fs::read_to_string(&merged_events_path).expect("read merged events");
    let records: Vec<serde_json::Value> = merged
        .lines()
        .map(|line| serde_json::from_str(line).expect("json event"))
        .collect();

    assert_eq!(records.len(), 1, "unexpected merged records: {records:?}");
    assert_eq!(records[0]["topic"], "review.done");
    assert_eq!(records[0]["payload"], "partial acp result");
    assert!(
        records
            .iter()
            .all(|record| record["topic"] != "wave.worker.failed"),
        "partial ACP timeout should not synthesize worker failures: {records:?}"
    );
    emit_wave_validation_marker("acp:partial-timeout-visible-events", &["backend", "error"]);
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_keeps_text_partial_timeout_events_visible() {
    let completed = run_wave_for_backend_with_timeout(
        BackendOutputFormat::Text,
        &body_with_post_event_sleep(text_backend_body("text partial timeout ok")),
        1,
    )
    .await;

    assert_partial_timeout_events_visible_marked(
        &completed,
        "text partial timeout ok",
        "pty:text-partial-timeout-visible-events",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_keeps_claude_stream_partial_timeout_events_visible() {
    let completed = run_wave_for_backend_with_timeout(
        BackendOutputFormat::StreamJson,
        &body_with_post_event_sleep(claude_backend_body("claude partial timeout ok")),
        1,
    )
    .await;

    assert_partial_timeout_events_visible_marked(
        &completed,
        "claude partial timeout ok",
        "pty:claude-stream-partial-timeout-visible-events",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_keeps_pi_stream_partial_timeout_events_visible() {
    let completed = run_wave_for_backend_with_timeout(
        BackendOutputFormat::PiStreamJson,
        &body_with_post_event_sleep(pi_backend_body("pi partial timeout ok")),
        1,
    )
    .await;

    assert_partial_timeout_events_visible_marked(
        &completed,
        "pi partial timeout ok",
        "pty:pi-stream-partial-timeout-visible-events",
    );
}

#[cfg(unix)]
#[test]
fn test_wave_worker_execution_mode_uses_acp_for_kiro_acp_backend() {
    let backend = CliBackend::kiro_acp();
    assert_eq!(
        wave_worker_execution_mode(backend.output_format),
        WaveWorkerExecutionMode::Acp
    );
    emit_wave_validation_marker("execution-mode:kiro-acp-backend", &["backend"]);
}

#[cfg(unix)]
#[test]
fn test_wave_worker_execution_mode_uses_acp_for_kiro_acp_hat_backend() {
    let backend = CliBackend::from_hat_backend(&ralph_core::HatBackend::KiroAgent {
        backend_type: "kiro-acp".to_string(),
        agent: "reviewer".to_string(),
        args: vec![],
    })
    .expect("kiro-acp backend");
    assert_eq!(
        wave_worker_execution_mode(backend.output_format),
        WaveWorkerExecutionMode::Acp
    );
    emit_wave_validation_marker("execution-mode:kiro-acp-hat-backend", &["backend"]);
}

#[cfg(unix)]
#[tokio::test]
async fn test_run_wave_worker_pty_surfaces_spawn_failure() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let missing_cmd = temp_dir.path().join("missing-wave-worker");
    let worker_events_path = temp_dir.path().join("wave-events.jsonl");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let backend = CliBackend {
        command: missing_cmd.display().to_string(),
        args: vec![],
        prompt_mode: ralph_adapters::PromptMode::Arg,
        prompt_flag: None,
        output_format: BackendOutputFormat::Text,
        env_vars: vec![],
    };

    let (_index, outcome) = run_wave_worker_pty(
        0,
        &backend,
        "prompt",
        &worker_events_path,
        Duration::from_secs(1),
        tx,
        None,
        None,
        None,
    )
    .await;

    let (error, _duration) = outcome.expect_err("missing worker should fail to spawn");
    assert!(
        error.contains("PTY spawn failed"),
        "unexpected error: {error}"
    );
    emit_wave_validation_marker("pty:spawn-failure", &["error"]);
}

#[cfg(unix)]
#[test]
fn test_merge_wave_results_to_events_file_synthesizes_failure_events() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let events_file = temp_dir.path().join("events.jsonl");
    let completed = ralph_core::CompletedWave {
        wave_id: "w-test".to_string(),
        wave_total: 2,
        results: vec![ralph_core::WaveResult {
            index: 0,
            events: vec![ralph_proto::Event::new("review.done", "worker ok")],
        }],
        failures: vec![ralph_core::WaveFailure {
            index: 1,
            error: "PTY spawn failed: missing-worker".to_string(),
            duration: Duration::from_secs(1),
            expected_dimension: None,
            actual_dimension: None,
        }],
        duration: Duration::from_secs(1),
        partial: false,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: Vec::new(),
    };

    merge_wave_results_to_events_file(
        &completed,
        &events_file,
        &["review.done".to_string(), "review.audit".to_string()],
        "reviewer",
        // 2026-06-16-001 U2: tests use the same default.
        None,
    )
    .expect("merge wave results");

    let content = std::fs::read_to_string(&events_file).expect("read merged events");
    let records: Vec<serde_json::Value> = content
        .lines()
        .map(|line| serde_json::from_str(line).expect("json event"))
        .collect();

    assert_eq!(records.len(), 4, "unexpected merged records: {records:?}");
    assert!(records.iter().any(|record| {
        record["topic"] == "wave.worker.failed"
            && record["payload"]
                .as_str()
                .is_some_and(|payload| payload.contains("PTY spawn failed: missing-worker"))
            && record["wave_index"] == 1
    }));
    assert!(records.iter().any(|record| {
        record["topic"] == "review.done"
            && record["payload"]
                .as_str()
                .is_some_and(|payload| payload.contains("## Worker 1 (FAILED)"))
    }));
    assert!(records.iter().any(|record| {
        record["topic"] == "review.audit"
            && record["payload"]
                .as_str()
                .is_some_and(|payload| payload.contains("Error: PTY spawn failed: missing-worker"))
    }));
    emit_wave_validation_marker(
        "merge-wave-results:synthetic-failure-events",
        &["error", "synthetic"],
    );
}
// ─────────────────────────────────────────────────────────────────────────
// 2026-06-13 plan U2: wave policy rejection must skip the missing-event
// gate (mirror of the contract-rejection case above).
//
// The agent emits a wave batch of 7 `review.wave.ready` events, all of
// which are policy-rejected (e.g. they lack the required `depth` field).
// `wave_events` is empty because the dispatcher never started, but
// `wave_policy_rejections` is non-empty. Without the U2 fix the runner
// sees an empty `processed` AND an empty `wave_events`, concludes the
// agent forgot to emit, and triggers `missing_event_gate` → wrong hat
// activation → `payload_contract_violation` loop death.
//
// After U2:
//   - `agent_wrote_any_valid_or_rejected` includes
//     `wave_had_policy_rejections` so the gate is skipped.
//   - `candidate_topics` includes the rejected topic so
//     `obligation_satisfied` treats the obligation as satisfied.
//   - A new `inject_wave_policy_rejection_guidance` helper writes a
//     `human.guidance` event listing the missing field, replacing the
//     generic "did not emit" message (mutually exclusive with the
//     missing-event gate at the call site).
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_wave_policy_rejection_skips_missing_event_gate() {
    // 2026-06-13 plan U2 — happy path: 7 `review.wave.ready` events
    // missing `depth` are policy-rejected. Mirror the runner's gate
    // decision expression and assert the gate does NOT fire.
    let yaml = r#"
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["work.done"]
    publishes: ["review.wave.ready", "review.passed"]
    obligations:
      - on_trigger: "work.done"
        must_emit_any_of: ["review.wave.ready", "review.passed"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.state_mut().last_activation_events =
        vec![ralph_proto::Event::new("work.done", "{}")];
    let hat = HatId::new("review-coordinator");

    // Construct a `ProcessedEventsWithWaves` shape with no regular
    // accepted/rejected events but a 7-element wave policy rejection
    // list — the exact shape U1 surfaces from the event loop and
    // U2 consumes in the runner.
    let policy_rejection = || ralph_core::PolicyRejection {
        topic: "review.wave.ready".to_string(),
        source_hat: Some("review-coordinator".to_string()),
        finding: ralph_core::PolicyFinding {
            topic: "review.wave.ready".to_string(),
            violation_type: ralph_core::ViolationType::MissingRequiredField {
                field: "depth".to_string(),
            },
            message: "Missing required field: depth".to_string(),
        },
        reason_class: None,
    };
    let wave_policy_rejections: Vec<ralph_core::PolicyRejection> =
        (0..7).map(|_| policy_rejection()).collect();
    let wave_raw_count = wave_policy_rejections.len();

    // The runner's gate condition uses:
    //   1. `agent_wrote_any_valid_or_rejected` — which U2 expands to
    //      include `wave_had_policy_rejections`.
    //   2. `wave_events.is_empty()` — the dispatcher never started.
    //   3. `should_gate_missing_events(display_hat, &event_loop, &candidate_topics)`
    //      where U2 merges wave rejected topics into `candidate_topics`.
    //
    // The boolean expression that the gate uses is therefore:
    //   !agent_wrote_any_valid_or_rejected
    //   && wave_events.is_empty()
    //   && !hard_gate_triggered_this_iteration
    //   && late_termination_reason.is_none()
    //   && should_gate_missing_events(...)
    //
    // U2 also routes to `inject_wave_policy_rejection_guidance` (a
    // separate branch) when `wave_had_policy_rejections && wave_events.is_empty()`,
    // which is mutually exclusive with the missing-event gate at the
    // call site. So when wave rejections are present, the gate's first
    // condition is already false → it never reaches
    // `should_gate_missing_events` to even evaluate the obligation.
    let agent_wrote_any_valid_or_rejected = compute_agent_wrote_any_valid_or_rejected(
        Some(ralph_core::ProcessedEvents {
            had_raw_events: false,
            had_rejected_events: false,
            ..Default::default()
        }),
        &wave_policy_rejections,
    );
    let wave_events_is_empty = true;
    let hard_gate_triggered_this_iteration = false;
    let late_termination_reason: Option<ralph_core::TerminationReason> = None;

    // U2 merge: candidate_topics gets the rejected topic too.
    let candidate_topics: Vec<String> = wave_policy_rejections
        .iter()
        .map(|r| r.topic.clone())
        .collect();

    // Sanity: the runner's gate expression (mirrored here) must be
    // false, meaning the gate is skipped.
    let gate_would_fire = !agent_wrote_any_valid_or_rejected
        && wave_events_is_empty
        && !hard_gate_triggered_this_iteration
        && late_termination_reason.is_none()
        && should_gate_missing_events(&hat, &event_loop, &candidate_topics);
    assert!(
        !gate_would_fire,
        "Missing-event gate MUST NOT fire when wave policy rejected a batch (U2)"
    );

    // Sanity: the runner's `should_gate_missing_events` call with the
    // merged candidate_topics would also be satisfied, but U2's
    // short-circuit (`agent_wrote_any_valid_or_rejected` already true)
    // is what actually saves us in production. The merged candidate
    // topics give us a defence-in-depth check.
    assert!(
        !should_gate_missing_events(&hat, &event_loop, &candidate_topics),
        "obligation_satisfied must treat the rejected topic as satisfying the obligation"
    );

    // Sanity: when candidates is empty (the broken pre-U2 state), the
    // gate WOULD fire — this is the regression we are guarding.
    let empty_candidates: Vec<String> = Vec::new();
    assert!(
        should_gate_missing_events(&hat, &event_loop, &empty_candidates),
        "Without U2's merge, the gate wrongly fires (this is the regression)"
    );

    // Sanity: `wave_raw_count` should match the rejection count for
    // the recovery envelope evidence.
    assert_eq!(
        wave_raw_count, 7,
        "wave_raw_count should match rejection count"
    );
}

#[test]
fn test_wave_policy_rejection_gate_still_fires_when_no_wave_attempt() {
    // 2026-06-13 plan U2 — error path: the agent emits nothing and
    // there are no wave rejections. The gate MUST still fire (this
    // protects against the U2 fix accidentally disabling the
    // missing-event gate entirely).
    let yaml = r#"
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["work.done"]
    publishes: ["review.wave.ready", "review.passed"]
    obligations:
      - on_trigger: "work.done"
        must_emit_any_of: ["review.wave.ready", "review.passed"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.state_mut().last_activation_events =
        vec![ralph_proto::Event::new("work.done", "{}")];
    let hat = HatId::new("review-coordinator");

    let wave_policy_rejections: Vec<ralph_core::PolicyRejection> = Vec::new();
    let agent_wrote_any_valid_or_rejected = compute_agent_wrote_any_valid_or_rejected(
        Some(ralph_core::ProcessedEvents {
            had_raw_events: false,
            had_rejected_events: false,
            ..Default::default()
        }),
        &wave_policy_rejections,
    );
    // No regular events at all AND no wave policy rejections →
    // agent_wrote_any_valid_or_rejected = false.
    assert!(
        !agent_wrote_any_valid_or_rejected,
        "agent_wrote_any_valid_or_rejected must be false when nothing was emitted"
    );

    let candidate_topics: Vec<String> = Vec::new();
    assert!(
        should_gate_missing_events(&hat, &event_loop, &candidate_topics),
        "Missing-event gate MUST fire when agent emitted nothing and no wave rejections"
    );
}

#[test]
fn test_wave_policy_rejection_gate_skipped_with_regular_accept_too() {
    // 2026-06-13 plan U2 — edge case: regular accept + wave reject
    // in the same iteration. The gate must NOT fire (the agent
    // emitted a valid event AND tried to emit a wave batch — both
    // prove it was active).
    let yaml = r#"
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["work.done"]
    publishes: ["review.wave.ready", "review.passed"]
    obligations:
      - on_trigger: "work.done"
        must_emit_any_of: ["review.wave.ready", "review.passed"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.state_mut().last_activation_events =
        vec![ralph_proto::Event::new("work.done", "{}")];
    let hat = HatId::new("review-coordinator");

    // Regular: had_raw_events = true (one accepted event).
    // Wave: 5 events policy-rejected.
    let wave_rejection = || ralph_core::PolicyRejection {
        topic: "review.wave.ready".to_string(),
        source_hat: Some("review-coordinator".to_string()),
        finding: ralph_core::PolicyFinding {
            topic: "review.wave.ready".to_string(),
            violation_type: ralph_core::ViolationType::MissingRequiredField {
                field: "depth".to_string(),
            },
            message: "Missing required field: depth".to_string(),
        },
        reason_class: None,
    };
    let wave_policy_rejections: Vec<ralph_core::PolicyRejection> =
        (0..5).map(|_| wave_rejection()).collect();
    let agent_wrote_any_valid_or_rejected = compute_agent_wrote_any_valid_or_rejected(
        Some(ralph_core::ProcessedEvents {
            had_raw_events: true,
            had_rejected_events: false,
            ..Default::default()
        }),
        &wave_policy_rejections,
    );
    assert!(
        agent_wrote_any_valid_or_rejected,
        "regular accept + wave reject must satisfy any_valid_or_rejected"
    );

    // With `agent_wrote_any_valid_or_rejected = true` the gate is
    // short-circuited at the runner call site.
    assert!(
        agent_wrote_any_valid_or_rejected,
        "U2 short-circuit must skip the gate"
    );

    // Defence-in-depth: even if the short-circuit ever failed, the
    // merged candidate_topics would still satisfy the obligation.
    let mut candidate_topics: Vec<String> = vec!["review.passed".to_string()];
    candidate_topics.extend(wave_policy_rejections.iter().map(|r| r.topic.clone()));
    assert!(
        !should_gate_missing_events(&hat, &event_loop, &candidate_topics),
        "merged candidate_topics (accepted + wave-rejected) must satisfy the obligation"
    );
}

/// Local helper: mirror the production expression in
/// `runner.rs::agent_wrote_any_valid_or_rejected`. The regular-path
/// U2 (2026-06-13-001): the production boolean is
/// `runner::agent_wrote_any_valid_or_rejected`; the test mirrors
/// that fn directly (passing a synthetic ProcessedEvents) so the
/// expression cannot drift between test and production.
fn compute_agent_wrote_any_valid_or_rejected(
    processed: Option<ralph_core::ProcessedEvents>,
    wave_policy_rejections: &[ralph_core::PolicyRejection],
) -> bool {
    runner::agent_wrote_any_valid_or_rejected(processed.as_ref(), wave_policy_rejections)
}
