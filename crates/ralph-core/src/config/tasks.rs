//! Task configuration.

use serde::{Deserialize, Serialize};

use super::default_true;

/// Tasks configuration.
///
/// Controls the runtime task tracking system that allows Ralph to manage
/// work items across iterations. Tasks are stored in `.ralph/agent/tasks.jsonl`.
///
/// When enabled, tasks replace scratchpad for loop completion verification.
///
/// Example configuration:
/// ```yaml
/// tasks:
///   enabled: true
///   coordinator_hats:
///     - coordinator
///     - executor
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TasksConfig {
    /// Whether the tasks feature is enabled.
    ///
    /// When true, tasks are used for loop completion verification.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Hats allowed to perform cross-hat task lifecycle operations.
    ///
    /// When a task is owned by hat A, only hat A or one of the
    /// `coordinator_hats` may `start` / `close` / `fail` / `reopen` it.
    /// Empty list means only the owner hat can mutate its own tasks.
    #[serde(default)]
    pub coordinator_hats: Vec<String>,
}

impl Default for TasksConfig {
    fn default() -> Self {
        Self {
            enabled: true, // Tasks enabled by default
            coordinator_hats: Vec::new(),
        }
    }
}
