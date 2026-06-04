//! Preflight extension hooks configuration.

use serde::{Deserialize, Serialize};

/// Preflight hook stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HookStage {
    /// Run before native preflight checks.
    BeforeNative,
    /// Run after native preflight checks.
    #[default]
    AfterNative,
}

fn default_hook_stage() -> HookStage {
    HookStage::AfterNative
}

/// A single preflight extension hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightHook {
    /// Human-readable name for this hook.
    pub name: String,

    /// Shell command to execute.
    pub command: String,

    /// Stage at which this hook runs.
    #[serde(default = "default_hook_stage")]
    pub stage: HookStage,

    /// Whether a failing hook should fail the preflight.
    #[serde(default)]
    pub fail_on_error: bool,
}

/// Preflight extension hooks configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightExtensionsConfig {
    /// Whether preflight extension hooks are enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Preflight hooks to run.
    #[serde(default)]
    pub hooks: Vec<PreflightHook>,
}
