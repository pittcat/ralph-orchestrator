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
///   require_verify_for_cli_mutate: true
///   allow_unsafe_task_mutate: false
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

    /// U7 (2026-07-04-003 plan): enforce a two-step verify-then-apply
    /// gate for `add` / `ensure` mutations invoked by agents.
    ///
    /// When `true` (the safe default), an agent must first invoke
    /// `ralph tools task verify <verb>` and obtain an `Allow` outcome;
    /// only then can the same agent invoke the real `<verb>` for the
    /// same payload. The matching is done by a stable fingerprint
    /// stored in `.ralph/agent/.ralph-task-verify-ticket`. This
    /// prevents drift between "I would have written X" and "I
    /// actually wrote X" that previously caused over-emitting
    /// `task.add` payloads.
    ///
    /// Human CLI invocations are exempt — operators must not be
    /// forced through the verify precheck for ad-hoc edits.
    #[serde(default)]
    pub require_verify_for_cli_mutate: bool,

    /// U7 escape hatch: skip the verify gate even for agent
    /// invocations. Defaults to `false`. Operators set this to
    /// `true` only for one-off recovery flows where the verify
    /// precheck itself is broken and an agent must be able to
    /// bypass it to make forward progress. Set at your own risk.
    #[serde(default)]
    pub allow_unsafe_task_mutate: bool,
}

impl Default for TasksConfig {
    fn default() -> Self {
        Self {
            enabled: true, // Tasks enabled by default
            coordinator_hats: Vec::new(),
            require_verify_for_cli_mutate: false,
            allow_unsafe_task_mutate: false,
        }
    }
}

#[cfg(test)]
mod tasks_config_gate_fields_tests {
    use super::TasksConfig;

    #[test]
    fn test_tasks_config_defaults_require_verify_false() {
        // Empty / minimal YAML must yield the conservative defaults:
        // verify gate OFF (humans + agents work without precheck)
        // and unsafe-mutate OFF (no escape hatch for agents).
        let cfg: TasksConfig = serde_yaml::from_str("{}").expect("parse empty yaml");
        assert!(
            !cfg.require_verify_for_cli_mutate,
            "require_verify_for_cli_mutate default must be false"
        );
        assert!(
            !cfg.allow_unsafe_task_mutate,
            "allow_unsafe_task_mutate default must be false"
        );
    }

    #[test]
    fn test_tasks_config_missing_fields_default_conservatively() {
        // Only `enabled` set — verify fields must still default to false.
        let cfg: TasksConfig =
            serde_yaml::from_str("enabled: true\n").expect("parse minimal yaml");
        assert!(!cfg.require_verify_for_cli_mutate);
        assert!(!cfg.allow_unsafe_task_mutate);
    }

    #[test]
    fn test_tasks_config_explicit_true() {
        // Operators can opt-in to the verify gate.
        let cfg: TasksConfig = serde_yaml::from_str(
            "enabled: true\nrequire_verify_for_cli_mutate: true\nallow_unsafe_task_mutate: true\n",
        )
        .expect("parse explicit yaml");
        assert!(cfg.require_verify_for_cli_mutate);
        assert!(cfg.allow_unsafe_task_mutate);
    }

    #[test]
    fn test_tasks_config_mixed_explicit_values() {
        // Verify gate ON, unsafe escape hatch OFF — the recommended
        // production posture.
        let cfg: TasksConfig = serde_yaml::from_str(
            "enabled: true\nrequire_verify_for_cli_mutate: true\nallow_unsafe_task_mutate: false\n",
        )
        .expect("parse mixed yaml");
        assert!(cfg.require_verify_for_cli_mutate);
        assert!(!cfg.allow_unsafe_task_mutate);
    }

    #[test]
    fn test_tasks_config_default_impl_matches_yaml_default() {
        // `TasksConfig::default()` must agree with the serde-derived
        // default of an empty YAML document. This guards against
        // future drift between the `impl Default` and the `serde(default)`
        // attribute on the new fields.
        let from_default = TasksConfig::default();
        let from_yaml: TasksConfig = serde_yaml::from_str("{}").expect("parse empty yaml");
        assert_eq!(
            from_default.require_verify_for_cli_mutate, from_yaml.require_verify_for_cli_mutate,
            "Default and yaml-default must agree on require_verify_for_cli_mutate"
        );
        assert_eq!(
            from_default.allow_unsafe_task_mutate, from_yaml.allow_unsafe_task_mutate,
            "Default and yaml-default must agree on allow_unsafe_task_mutate"
        );
    }
}
