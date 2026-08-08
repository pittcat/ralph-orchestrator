//! Headless integration tests for `CliExecutor::execute` with the
//! `AgentStreamJson` output format. These tests exercise the
//! `is_error` / `subtype == "error"` readback path that mirrors the
//! PTY executor's `!agent_state.is_error` contract (R1 / correctness:C1).
//!
//! The `#[cfg(unix)]` gate matches `pty_executor_integration.rs` — these
//! tests invoke `sh -c <script>` and are not meaningful on Windows.

#[cfg(unix)]
mod cli_executor_integration {
    use ralph_adapters::{
        AgentSessionState, AgentStreamParser, CliBackend, CliExecutor, OutputFormat, PromptMode,
        SessionResult, StreamHandler, dispatch_agent_stream_event,
    };

    /// Script that emits a `system` init, an assistant text event, and a
    /// successful terminal `result` event with `is_error = false`.
    const SCRIPT_SUCCESS: &str = r#"printf '%s\n' \
'{"type":"system","subtype":"init","session_id":"s1"}' \
'{"type":"assistant","message":{"content":[{"type":"text","text":"headless ok"}]}}' \
'{"type":"result","subtype":"success","result":"final","is_error":false}'"#;

    /// Script that emits ONLY a terminal `result` event with `is_error = true`
    /// and `subtype = "error"`. This is the headless logical-error scenario
    /// the fix-plan's R1 / correctness:C1 surfaces.
    const SCRIPT_ERROR_ONLY: &str = r#"printf '%s\n' \
'{"type":"result","subtype":"error","is_error":true,"result":"boom"}'"#;

    /// Script that interleaves assistant text + tool_call completed + an
    /// `is_error = true` terminal result. Mirrors the PTY S3+S4 contract.
    const SCRIPT_MIXED: &str = r#"printf '%s\n' \
'{"type":"assistant","message":{"content":[{"type":"text","text":"partial answer"}]}}' \
'{"type":"tool_call","subtype":"started","call_id":"c1","tool_call":{"readToolCall":{"args":{"path":"a"}}}}' \
'{"type":"tool_call","subtype":"completed","call_id":"c1","tool_call":{"readToolCall":{"result":{"success":{"content":"file contents"}}}}}' \
'{"type":"result","subtype":"error","is_error":true,"result":"fatal"}'"#;

    /// Regression guard for the headless multi-assistant-delta contract:
    /// every assistant `on_text` event must reach the writer verbatim,
    /// not just the first one. The terminal `result` fallback is suppressed
    /// by the dispatcher when any assistant text was emitted (the
    /// `if extracted_text.is_empty()` guard inside
    /// `dispatch_agent_stream_event`).
    const SCRIPT_MULTIPLE_ASSISTANT_DELTAS: &str = r#"printf '%s\n' \
'{"type":"assistant","message":{"content":[{"type":"text","text":"first "}]}}' \
'{"type":"assistant","message":{"content":[{"type":"text","text":"second "}]}}' \
'{"type":"assistant","message":{"content":[{"type":"text","text":"third"}]}}' \
'{"type":"result","subtype":"success","result":"should-not-appear","is_error":false}'"#;

    fn agent_stream_json_backend() -> CliBackend {
        CliBackend {
            command: "sh".to_string(),
            args: vec!["-c".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::AgentStreamJson,
            env_vars: vec![],
        }
    }

    /// All test paths are agnostic about the workspace directory itself;
    /// passing the parent's current_dir() preserves the historical
    /// ambient-cwd semantics these tests asserted.
    fn integration_workspace() -> std::path::PathBuf {
        std::env::current_dir().unwrap()
    }

    #[tokio::test]
    async fn headless_agent_stream_json_success_terminal_payload() {
        let executor = CliExecutor::new(agent_stream_json_backend());
        let mut output = Vec::new();

        let result = executor
            .execute(
                SCRIPT_SUCCESS,
                &mut output,
                None,
                false,
                &integration_workspace(),
            )
            .await
            .expect("execute ok");

        // R1: a successful terminal payload (is_error=false) reports success.
        assert!(
            result.success,
            "headless success payload should report success=true; got exit_code={:?}",
            result.exit_code
        );
        assert_eq!(result.exit_code, Some(0));
        let written = String::from_utf8(output).expect("utf8");
        assert!(
            written.contains("headless ok"),
            "writer should contain assistant text; got: {written:?}"
        );
    }

    #[tokio::test]
    async fn headless_agent_stream_json_error_terminal_payload_marks_failed() {
        let executor = CliExecutor::new(agent_stream_json_backend());
        let mut output = Vec::new();

        let result = executor
            .execute(
                SCRIPT_ERROR_ONLY,
                &mut output,
                None,
                false,
                &integration_workspace(),
            )
            .await
            .expect("execute ok");

        // R1: an error terminal payload (is_error=true) must NOT be reported
        // as success, even though the shell itself exits 0.
        assert!(
            !result.success,
            "headless error payload should report success=false; got exit_code={:?}",
            result.exit_code
        );
        assert_ne!(
            result.exit_code,
            Some(0),
            "headless error payload must yield non-zero exit_code; got {:?}",
            result.exit_code
        );
    }

    #[tokio::test]
    async fn headless_agent_stream_json_mixed_assistant_then_error_terminal() {
        let executor = CliExecutor::new(agent_stream_json_backend());
        let mut output = Vec::new();

        let result = executor
            .execute(
                SCRIPT_MIXED,
                &mut output,
                None,
                false,
                &integration_workspace(),
            )
            .await
            .expect("execute ok");

        // R1: even when assistant text was streamed, a final error payload
        // overrides success and the assistant text is preserved in output.
        assert!(
            !result.success,
            "mixed assistant+error terminal payload must report success=false; got exit_code={:?}",
            result.exit_code
        );
        assert_ne!(result.exit_code, Some(0));
        let written = String::from_utf8(output).expect("utf8");
        assert!(
            written.contains("partial answer"),
            "writer should preserve assistant text before the error payload; got: {written:?}"
        );
    }

    #[tokio::test]
    async fn headless_agent_stream_json_emits_every_assistant_delta() {
        // Regression guard: previously the headless branch held an
        // `agent_text_written` flag that swallowed every assistant delta
        // after the first one. The contract is: every `on_text` from the
        // dispatcher reaches the writer, in dispatch order.
        let executor = CliExecutor::new(agent_stream_json_backend());
        let mut output = Vec::new();

        let result = executor
            .execute(
                SCRIPT_MULTIPLE_ASSISTANT_DELTAS,
                &mut output,
                None,
                false,
                &integration_workspace(),
            )
            .await
            .expect("execute ok");

        // R1: terminal `is_error=false` keeps the run successful even
        // though no assistant text precedes the result event in a
        // degenerate case — here we have multiple assistant deltas so
        // the terminal fallback is suppressed by the dispatcher.
        assert!(
            result.success,
            "headless multi-assistant success payload should report success=true; got exit_code={:?}",
            result.exit_code
        );
        assert_eq!(result.exit_code, Some(0));

        let written = String::from_utf8(output).expect("utf8");

        // Every assistant delta must appear in the writer in dispatch order.
        assert!(
            written.contains("first "),
            "writer should contain first assistant delta; got: {written:?}"
        );
        assert!(
            written.contains("second "),
            "writer should contain second assistant delta; got: {written:?}"
        );
        assert!(
            written.contains("third"),
            "writer should contain third assistant delta; got: {written:?}"
        );
        // The literal concatenation (each delta ends with a newline because
        // the headless path appends one when the input does not) must be
        // present, asserting dispatch order.
        assert!(
            written.contains("first \nsecond \nthird\n"),
            "writer should contain all three deltas in dispatch order; got: {written:?}"
        );
        // The terminal `result` text must NOT be forwarded when any
        // assistant delta preceded it — the dispatcher's
        // `extracted_text.is_empty()` guard owns that contract.
        assert!(
            !written.contains("should-not-appear"),
            "terminal result fallback must be suppressed when assistant text preceded it; got: {written:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Unit-level coverage for the shared dispatch helper used by both PTY and
    // the (now-fixed) headless path. This guards the `agent_state.is_error`
    // contract that the headless executor consults.
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct CaptureHandler {
        texts: Vec<String>,
    }

    impl StreamHandler for CaptureHandler {
        fn on_text(&mut self, text: &str) {
            self.texts.push(text.to_string());
        }
        fn on_tool_call(&mut self, _name: &str, _id: &str, _input: &serde_json::Value) {}
        fn on_tool_result(&mut self, _id: &str, _output: &str) {}
        fn on_error(&mut self, _error: &str) {}
        fn on_complete(&mut self, _result: &SessionResult) {}
    }

    #[test]
    fn dispatch_marks_state_error_on_is_error_true() {
        let line = r#"{"type":"result","subtype":"error","is_error":true,"result":"x"}"#;
        let event = AgentStreamParser::parse_line(line).expect("parse ok");
        let mut handler = CaptureHandler::default();
        let mut extracted = String::new();
        let mut state = AgentSessionState::default();
        dispatch_agent_stream_event(event, &mut handler, &mut extracted, &mut state);

        assert!(state.is_error, "is_error=true must flip state.is_error");
    }

    #[test]
    fn dispatch_does_not_mark_state_error_on_success_terminal() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"result":"x"}"#;
        let event = AgentStreamParser::parse_line(line).expect("parse ok");
        let mut handler = CaptureHandler::default();
        let mut extracted = String::new();
        let mut state = AgentSessionState::default();
        dispatch_agent_stream_event(event, &mut handler, &mut extracted, &mut state);

        assert!(
            !state.is_error,
            "is_error=false must NOT flip state.is_error"
        );
    }

    // -----------------------------------------------------------------------
    // 2026-08-08-001 plan U2: headless CliExecutor explicit-workspace contract.
    //
    // Each test below drives a real `sh -c <script>` subprocess so we can
    // assert the exact cwd, env, and write-down effects visible to the
    // backend. They exercise the three scenarios from the plan §6:
    //   1. explicit_workspace_controls_cli_executor_cwd         — R1
    //   2. runtime_workspace_overrides_backend_workspace_env    — R2
    //   3. missing_explicit_workspace_returns_spawn_error       — R4
    // -----------------------------------------------------------------------

    /// R1: the explicit workspace passed to `CliExecutor::execute` controls
    /// the backend subprocess's current directory and nothing else does.
    /// We point a marker-writing shell script at `target`; `source` is a
    /// control directory that MUST stay empty (no fallback to parent cwd
    /// or any ambient `RALPH_WORKSPACE_ROOT` value).
    ///
    /// Before the fix, `CliExecutor::execute` resolved cwd by reading
    /// `RALPH_WORKSPACE_ROOT` / `std::env::current_dir()` — both pointed
    /// somewhere other than `target`, so the marker leaked out and the
    /// marker body inside `target` stayed empty.
    #[tokio::test]
    async fn explicit_workspace_controls_cli_executor_cwd() {
        let target = tempfile::TempDir::new().expect("target temp dir");
        let control = tempfile::TempDir::new().expect("control temp dir");
        let target_pwd_marker = target.path().join("pwd-marker");

        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                format!(
                    "pwd > {}; printf 'marked\\n' > marker",
                    target_pwd_marker.display(),
                ),
            ],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let mut output = Vec::new();

        let result = executor
            .execute("", &mut output, None, false, target.path())
            .await
            .expect("execute ok");

        assert!(result.success, "execute should succeed; got: {result:?}");
        assert!(
            target_pwd_marker.exists(),
            "target pwd marker must exist at {} — backend failed to land in the explicit workspace",
            target_pwd_marker.display()
        );
        let recorded_pwd = std::fs::read_to_string(&target_pwd_marker)
            .expect("read pwd marker")
            .trim()
            .to_string();
        assert_eq!(
            recorded_pwd,
            target.path().to_string_lossy(),
            "backend pwd ({recorded_pwd}) must equal the explicit target workspace ({})",
            target.path().display()
        );
        // The script wrote a separate `marker` file via relative path —
        // it must land inside `target`, proving the backend was chdir'd
        // there. (Pre-fix, this file would land in some other directory
        // like the parent cwd).
        let target_marker = target.path().join("marker");
        assert!(
            target_marker.exists(),
            "marker must be written inside the target workspace at {}",
            target_marker.display()
        );
        // The control dir must not be polluted by a leaked marker. We
        // check by listing rather than asserting a specific path, so the
        // test still passes if `tempfile` cleans up its own directory.
        let control_polluted =
            control.path().join("marker").exists() || control.path().join("pwd-marker").exists();
        assert!(
            !control_polluted,
            "control dir {} must not receive any backend-written marker",
            control.path().display()
        );
    }

    /// R2: when the backend's own `env_vars` try to rewrite
    /// `RALPH_WORKSPACE_ROOT` / `PWD` to a hostile value, the runtime
    /// isolation variables set by `inject_ralph_runtime_env` must take
    /// precedence. The child must observe the explicit workspace both as
    /// cwd and as `RALPH_WORKSPACE_ROOT` / `PWD`.
    #[tokio::test]
    async fn runtime_workspace_overrides_backend_workspace_env() {
        let target = tempfile::TempDir::new().expect("target temp dir");
        let hostile = tempfile::TempDir::new().expect("hostile temp dir");

        // Backend env tries to redirect both isolation variables.
        // `hostile` mirrors the bound-workspace source path; if any part
        // of the resolve order forgets to re-set the runtime variables
        // last, the script's `pwd` would record `hostile` rather than
        // `target`.
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "pwd > marker; printf '%s\\n' \"$RALPH_WORKSPACE_ROOT\" >> marker; printf '%s\\n' \"$PWD\" >> marker".to_string(),
            ],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![
                (
                    "RALPH_WORKSPACE_ROOT".into(),
                    hostile.path().to_string_lossy().into_owned(),
                ),
                ("PWD".into(), hostile.path().to_string_lossy().into_owned()),
            ],
        };

        let executor = CliExecutor::new(backend);
        let mut output = Vec::new();
        let result = executor
            .execute("", &mut output, None, false, target.path())
            .await
            .expect("execute ok");

        assert!(result.success, "execute should succeed; got: {result:?}");
        let marker = std::fs::read_to_string(target.path().join("marker")).expect("read marker");
        let lines: Vec<&str> = marker.lines().collect();
        assert_eq!(
            lines.len(),
            3,
            "marker must record pwd + 2 env lines:\n{marker}"
        );
        assert_eq!(
            lines[0],
            target.path().to_string_lossy(),
            "backend must see explicit target as cwd, not the hostile fallback"
        );
        assert_eq!(
            lines[1],
            target.path().to_string_lossy(),
            "backend RALPH_WORKSPACE_ROOT must equal the explicit target, not the hostile env"
        );
        assert_eq!(
            lines[2],
            target.path().to_string_lossy(),
            "backend PWD must equal the explicit target, not the hostile env"
        );
    }

    /// R4: passing an explicit workspace that does not exist must return
    /// an `Err` from the spawn boundary — never silently fall back to the
    /// parent-process cwd or any ambient `RALPH_WORKSPACE_ROOT`.
    #[tokio::test]
    async fn missing_explicit_workspace_returns_spawn_error() {
        // Compose a path that definitely does not exist by leaning on
        // a fresh temp dir we then delete (which removes the directory
        // but leaves the canonical path).
        let temp = tempfile::TempDir::new().expect("temp dir");
        let missing = temp.path().join("child-must-not-exist");
        drop(temp);

        // Use a tiny script that would otherwise succeed if we ever
        // silently fell back to a different cwd.
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "printf 'should-never-run\\n'".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        let executor = CliExecutor::new(backend);
        let mut output = Vec::new();
        let result = executor
            .execute("", &mut output, None, false, &missing)
            .await;

        assert!(
            result.is_err(),
            "execute against a nonexistent workspace must return Err, got: {result:?}"
        );
    }
}
