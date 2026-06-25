//! Hooks configuration.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::error::ConfigError;

/// Hooks configuration.
///
/// Controls per-project orchestrator lifecycle hooks. Hooks are disabled by
/// default and are inert until explicitly enabled.
///
/// Example configuration:
/// ```yaml
/// hooks:
///   enabled: true
///   defaults:
///     timeout_seconds: 30
///     max_output_bytes: 8192
///     suspend_mode: wait_for_resume
///   events:
///     pre.loop.start:
///       - name: env-guard
///         command: ["./scripts/hooks/env-guard.sh"]
///         on_error: block
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HooksConfig {
    /// Whether lifecycle hooks are enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Default guardrails applied to hook specs when per-hook values are absent.
    #[serde(default)]
    pub defaults: HookDefaults,

    /// Hook lists by lifecycle phase-event key.
    #[serde(default)]
    pub events: HashMap<HookPhaseEvent, Vec<HookSpec>>,

    /// Unknown keys captured for v1 guardrails.
    #[serde(default, flatten)]
    pub extra: HashMap<String, serde_yaml::Value>,
}

/// Hook defaults applied when a hook spec omits optional limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDefaults {
    /// Maximum execution time per hook in seconds.
    #[serde(default = "default_hook_timeout_seconds")]
    pub timeout_seconds: u64,

    /// Maximum stdout/stderr bytes stored per stream.
    #[serde(default = "default_hook_max_output_bytes")]
    pub max_output_bytes: u64,

    /// Suspend strategy used when `on_error: suspend` and no per-hook mode is set.
    #[serde(default)]
    pub suspend_mode: HookSuspendMode,
}

fn default_hook_timeout_seconds() -> u64 {
    30
}

fn default_hook_max_output_bytes() -> u64 {
    8192
}

impl Default for HookDefaults {
    fn default() -> Self {
        Self {
            timeout_seconds: default_hook_timeout_seconds(),
            max_output_bytes: default_hook_max_output_bytes(),
            suspend_mode: HookSuspendMode::default(),
        }
    }
}

/// Supported lifecycle phase-event keys for v1 hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookPhaseEvent {
    #[serde(rename = "pre.loop.start")]
    PreLoopStart,
    #[serde(rename = "post.loop.start")]
    PostLoopStart,
    #[serde(rename = "pre.iteration.start")]
    PreIterationStart,
    #[serde(rename = "post.iteration.start")]
    PostIterationStart,
    #[serde(rename = "pre.plan.created")]
    PrePlanCreated,
    #[serde(rename = "post.plan.created")]
    PostPlanCreated,
    #[serde(rename = "pre.loop.complete")]
    PreLoopComplete,
    #[serde(rename = "post.loop.complete")]
    PostLoopComplete,
    #[serde(rename = "pre.loop.error")]
    PreLoopError,
    #[serde(rename = "post.loop.error")]
    PostLoopError,
}

impl HookPhaseEvent {
    /// Parses a phase-event key (e.g. "pre.loop.start") into a HookPhaseEvent variant.
    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "pre.loop.start" => Self::PreLoopStart,
            "post.loop.start" => Self::PostLoopStart,
            "pre.iteration.start" => Self::PreIterationStart,
            "post.iteration.start" => Self::PostIterationStart,
            "pre.plan.created" => Self::PrePlanCreated,
            "post.plan.created" => Self::PostPlanCreated,
            "pre.loop.complete" => Self::PreLoopComplete,
            "post.loop.complete" => Self::PostLoopComplete,
            "pre.loop.error" => Self::PreLoopError,
            "post.loop.error" => Self::PostLoopError,
            _ => return None,
        })
    }

    /// Parses a phase-event key from a string.
    pub fn parse(s: &str) -> Option<Self> {
        Self::from_key(s)
    }

    /// Returns the canonical key string for this phase-event.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreLoopStart => "pre.loop.start",
            Self::PostLoopStart => "post.loop.start",
            Self::PreIterationStart => "pre.iteration.start",
            Self::PostIterationStart => "post.iteration.start",
            Self::PrePlanCreated => "pre.plan.created",
            Self::PostPlanCreated => "post.plan.created",
            Self::PreLoopComplete => "pre.loop.complete",
            Self::PostLoopComplete => "post.loop.complete",
            Self::PreLoopError => "pre.loop.error",
            Self::PostLoopError => "post.loop.error",
        }
    }
}

impl std::fmt::Display for HookPhaseEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str((*self).as_str())
    }
}

/// Validates that all keys in the `hooks.events` mapping are supported v1 phase-event keys.
pub(super) fn validate_hooks_phase_event_keys(
    value: &serde_yaml::Value,
) -> Result<(), ConfigError> {
    let Some(root) = value.as_mapping() else {
        return Ok(());
    };

    let Some(hooks) = root.get(serde_yaml::Value::String("hooks".to_string())) else {
        return Ok(());
    };

    let Some(hooks_map) = hooks.as_mapping() else {
        return Ok(());
    };

    let Some(events) = hooks_map.get(serde_yaml::Value::String("events".to_string())) else {
        return Ok(());
    };

    let Some(events_map) = events.as_mapping() else {
        return Ok(());
    };

    for (k, _) in events_map.iter() {
        let Some(key) = k.as_str() else {
            continue;
        };
        if HookPhaseEvent::from_key(key).is_none() {
            return Err(ConfigError::InvalidHookPhaseEvent {
                phase_event: key.to_string(),
            });
        }
    }
    Ok(())
}

/// Failure behavior for hooks (`warn`, `block`, `suspend`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HookOnError {
    /// Log the failure but do not block.
    #[default]
    Warn,
    /// Reject the triggering event (publishes task.resume).
    Block,
    /// Suspend the loop until a human resumes it.
    Suspend,
}

/// Suspend strategy for hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HookSuspendMode {
    /// Pause the loop until an explicit operator resume signal is received.
    #[default]
    WaitForResume,
    /// Retry automatically with bounded backoff.
    RetryBackoff,
    /// Wait for resume, then retry once.
    WaitThenRetry,
}

/// Hook mutation policy. JSON-only payload contract enforced by validation/runtime.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookMutationConfig {
    /// Whether mutation is enabled for this hook.
    #[serde(default)]
    pub enabled: bool,

    /// Optional payload format (only "json" supported in v1).
    #[serde(default)]
    pub format: Option<String>,

    /// Unknown keys captured for v1 guardrails.
    #[serde(default, flatten)]
    pub extra: HashMap<String, serde_yaml::Value>,
}

/// A single hook specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookSpec {
    /// Stable hook identifier used in telemetry and diagnostics.
    #[serde(default)]
    pub name: String,

    /// Command argv form (`command[0]` executable + args).
    #[serde(default)]
    pub command: Vec<String>,

    /// Optional working directory override.
    #[serde(default)]
    pub cwd: Option<std::path::PathBuf>,

    /// Optional environment variable overrides.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Per-hook timeout override in seconds.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,

    /// Per-hook output cap override in bytes (applies per stream).
    #[serde(default)]
    pub max_output_bytes: Option<u64>,

    /// Failure behavior (`warn`, `block`, `suspend`). Required in v1.
    #[serde(default)]
    pub on_error: Option<HookOnError>,

    /// Optional suspend strategy override for `on_error: suspend`.
    #[serde(default)]
    pub suspend_mode: Option<HookSuspendMode>,

    /// Mutation policy (opt-in, JSON-only contract enforced by validation/runtime).
    #[serde(default)]
    pub mutate: HookMutationConfig,

    /// Unknown keys captured for v1 guardrails.
    #[serde(default, flatten)]
    pub extra: HashMap<String, serde_yaml::Value>,
}
