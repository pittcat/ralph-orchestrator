//! # ralph-proto
//!
//! Shared types, error definitions, and traits for the Ralph Orchestrator framework.
//!
//! This crate provides the foundational abstractions used across all Ralph crates,
//! including:
//! - Event and `EventBus` types for pub/sub messaging
//! - Hat definitions for agent personas
//! - Topic matching for event routing
//! - Common error types

pub mod daemon;
mod error;
mod event;
mod event_bus;
mod hat;
pub mod json_rpc;
pub mod robot;
mod topic;
/// U7b: well-known event topic constants.  The new
/// `LOOP_RESUME` constant replaces the legacy `task.resume`
/// boot event on `--continue`.
pub mod topics;
mod ux_event;

pub use daemon::{DaemonAdapter, StartLoopFn};
pub use error::{Error, Result};
pub use event::Event;
pub use event_bus::EventBus;
pub use hat::{Hat, HatId};
pub use json_rpc::{
    GuidanceTarget, RpcCommand, RpcEvent, RpcIterationInfo, RpcState, RpcTaskCounts,
    RpcTaskSummary, TerminationReason, emit_event, emit_event_line, parse_command,
};
pub use robot::{CheckinContext, RobotService};
pub use topic::Topic;
pub use topics::{
    EVENT_ISOLATION_BOUNDARY_VIOLATION, HUMAN_GUIDANCE, LOOP_CANCEL, LOOP_COMPLETE, LOOP_RESUME,
    TASK_RESUME, is_orchestrator_control,
};
pub use ux_event::{
    FrameCapture, TerminalColorMode, TerminalResize, TerminalWrite, TuiFrame, UxEvent,
};
