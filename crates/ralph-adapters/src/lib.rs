//! # ralph-adapters
//!
//! Agent adapters for the Ralph Orchestrator framework.
//!
//! This crate provides implementations for various AI agent backends:
//! - Claude (Anthropic)
//! - Gemini (Google)
//! - Codex (OpenAI)
//! - Pi (pi-coding-agent)
//! - Roo (Roo Code)
//! - Amp
//! - Custom commands
//!
//! Each adapter implements the common CLI executor interface.
//!
//! ## Auto-Detection
//!
//! When config specifies `agent: auto`, the `auto_detect` module handles
//! detecting which backends are available in the system PATH.
//!
//! ## PTY Mode
//!
//! The `pty_executor` module provides PTY-based execution for Claude CLI,
//! preserving rich terminal UI features (colors, spinners, animations) while
//! allowing Ralph to orchestrate iterations. Supports interactive mode (user
//! input forwarded) and observe mode (output-only).

mod acp_executor;
mod auto_detect;
mod claude_stream;
mod cli_backend;
mod cli_executor;
mod copilot_stream;
mod json_rpc_handler;
mod pi_stream;
mod pty_executor;
pub mod pty_handle;
mod stream_handler;
pub mod tool_policy;
pub mod tool_preview;
mod trae_stream;

pub use acp_executor::AcpExecutor;
pub use auto_detect::{
    DEFAULT_PRIORITY, NoBackendError, detect_backend, detect_backend_default, is_backend_available,
};
pub use claude_stream::{
    AssistantMessage, ClaudeStreamEvent, ClaudeStreamParser, ContentBlock, Usage, UserContentBlock,
    UserMessage,
};
pub use cli_backend::{CliBackend, CustomBackendError, OutputFormat, PromptMode};
pub use cli_executor::{CliExecutor, ExecutionResult};
pub use copilot_stream::{CopilotAssistantMessage, CopilotStreamEvent, CopilotStreamParser};
pub use json_rpc_handler::{JsonRpcStreamHandler, stdout_json_rpc_handler};
pub use pi_stream::{
    PiAssistantEvent, PiContentBlock, PiCost, PiSessionState, PiStreamEvent, PiStreamParser,
    PiToolResult, PiTurnMessage, PiUsage, dispatch_pi_stream_event,
};
pub use pty_executor::{
    CtrlCAction, CtrlCState, PtyConfig, PtyExecutionResult, PtyExecutor, TerminationType,
};
pub use pty_handle::{ControlCommand, PtyHandle};
pub use stream_handler::{
    ConsoleStreamHandler, PrettyStreamHandler, QuietStreamHandler, SessionResult, StreamHandler,
    TuiStreamHandler,
};
pub use tool_policy::apply_hat_tool_policy;
pub use trae_stream::{
    TraeAssistantMessage, TraeAssistantToolCall, TraeSessionState, TraeStreamEvent,
    TraeStreamParser, TraeTextContent, TraeToolFunction, TraeToolResultContent, TraeToolUseContent,
    TraeUserMessage, dispatch_trae_stream_event, extract_assistant_text,
    extract_assistant_tool_calls, extract_user_tool_result_text, user_is_tool_result,
};
