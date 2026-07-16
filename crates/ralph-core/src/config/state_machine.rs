//! State machine configuration for instance lifecycle validation.

use serde::{Deserialize, Serialize};

/// State machine configuration for instance lifecycle validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct StateMachineConfig {
    /// When true, enable state machine validation.
    #[serde(default)]
    pub enabled: bool,

    /// Configuration for extracting instance keys from event payloads.
    #[serde(default)]
    pub instance_key: InstanceKeyConfig,

    /// Event topics that represent terminal/completion states.
    #[serde(default)]
    pub terminal_topics: Vec<String>,

    /// Event topics that represent business progress events.
    #[serde(default)]
    pub business_topics: Vec<String>,

    /// Guards for terminal event behavior.
    #[serde(default)]
    pub terminal_guard: TerminalGuardConfig,

    /// Ordered list of state transitions defining the valid lifecycle paths.
    #[serde(default)]
    pub transitions: Vec<TransitionConfig>,
}


/// Configuration for extracting an instance key from an event payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct InstanceKeyConfig {
    /// The JSON field name within the payload to extract as the instance key.
    /// Example: "task_key" extracts `payload.task_key`.
    pub from_payload: String,

    /// Event topics whose payloads must contain a valid instance key.
    #[serde(default)]
    pub required_for: Vec<String>,
}


/// A single state transition in the state machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionConfig {
    /// The event topic that triggers this transition.
    pub topic: String,

    /// The source states this transition can fire from.
    /// "idle" is a special state representing "no prior state".
    pub from: Vec<String>,

    /// The target state after this transition completes.
    pub to: String,

    /// When true, this transition opens a new instance (inserts into open map).
    #[serde(default)]
    pub opens_instance: bool,

    /// When true, this transition closes the instance (removes from open map).
    #[serde(default)]
    pub closes_instance: bool,
}

/// Guards for terminal event behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalGuardConfig {
    /// When true, terminal events are rejected if any instances are still open.
    #[serde(default)]
    pub require_no_open_instances: bool,

    /// Action for duplicate terminal events after terminal has been honored.
    #[serde(default = "default_duplicate_terminal_action")]
    pub duplicate_terminal: DuplicateTerminalAction,

    /// Action for business events that arrive after a terminal event.
    #[serde(default = "default_business_after_terminal_action")]
    pub business_after_terminal: BusinessAfterTerminalAction,
}

fn default_duplicate_terminal_action() -> DuplicateTerminalAction {
    DuplicateTerminalAction::Reject
}

fn default_business_after_terminal_action() -> BusinessAfterTerminalAction {
    BusinessAfterTerminalAction::Reject
}

impl Default for TerminalGuardConfig {
    fn default() -> Self {
        Self {
            require_no_open_instances: true,
            duplicate_terminal: default_duplicate_terminal_action(),
            business_after_terminal: default_business_after_terminal_action(),
        }
    }
}

/// Action for duplicate terminal events.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateTerminalAction {
    /// Reject duplicate terminal events (publish diagnostic, no task.resume).
    #[default]
    Reject,
    /// Silently ignore duplicate terminal events.
    Ignore,
}

/// Action for business events arriving after terminal.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BusinessAfterTerminalAction {
    /// Reject business events after terminal.
    #[default]
    Reject,
    /// Silently ignore business events after terminal.
    Ignore,
}
