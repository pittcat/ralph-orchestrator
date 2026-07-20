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

    #[tokio::test]
    async fn headless_agent_stream_json_success_terminal_payload() {
        let executor = CliExecutor::new(agent_stream_json_backend());
        let mut output = Vec::new();

        let result = executor
            .execute(SCRIPT_SUCCESS, &mut output, None, false)
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
            .execute(SCRIPT_ERROR_ONLY, &mut output, None, false)
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
            .execute(SCRIPT_MIXED, &mut output, None, false)
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
}
