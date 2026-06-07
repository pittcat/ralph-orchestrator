//! Configuration types for the Ralph Orchestrator.
//!
//! This module supports both v1.x flat configuration format and v2.0 nested format.
//! Users can switch from Python v1.x to Rust v2.0 with zero config changes.

mod cli;
mod core;
mod error;
mod event_filter;
mod event_policy;
mod event_projection;
pub(crate) mod execution_contracts;
mod features;
pub mod hat;
mod hooks;
mod loop_config;
mod memories;
mod preflight_ext;
mod ralph_config;
pub(crate) mod robot;
mod skills;
mod state_files;
pub(crate) mod state_machine;
mod tasks;
mod telemetry;
mod v1_adapters;
mod warning;
pub(crate) mod workflow_guards;

pub use cli::{CliConfig, TuiConfig};
pub use core::{CoreConfig, ScratchpadConfig};
pub use error::ConfigError;
pub use event_filter::{EventFilterConfig, EventFilterMode};
pub use event_policy::{
    CompletionAfterTerminalAction, EventPolicyConfig, EventPolicyMode, ViolationAction,
};
pub use event_projection::{EventProjectionConfig, ProjectionMode, ProjectionRule};
pub use execution_contracts::ExecutionContractRule;
pub use features::FeaturesConfig;
pub use hat::{
    ActivationObligation, AggregateConfig, AggregateMode, EventMetadata, HatBackend, HatConfig,
    obligation_satisfied,
};
pub use hooks::{
    HookDefaults, HookMutationConfig, HookOnError, HookPhaseEvent, HookSpec, HookSuspendMode,
    HooksConfig,
};
pub use loop_config::{
    EventLoopConfig, EventSchema, PayloadType, Phase, PhaseConfig, VerdictGateConfig, WarmupConfig,
};
pub use memories::{InjectMode, MemoriesConfig, MemoriesFilter};
pub use preflight_ext::{HookStage, PreflightExtensionsConfig, PreflightHook};
pub use robot::RobotConfig;
pub use skills::{SkillOverride, SkillsConfig};
pub use state_files::{StateFileEntry, StateFileFormat, StateFilesConfig};
pub use state_machine::{BusinessAfterTerminalAction, DuplicateTerminalAction, StateMachineConfig};
pub use tasks::TasksConfig;
pub use telemetry::{DriftConfig, MalformedJsonlPolicy, RuntimeDiagnosisConfig, TelemetryConfig};
pub use v1_adapters::{AdapterSettings, AdaptersConfig};
pub use warning::ConfigWarning;
pub use workflow_guards::{
    HatExecutionMode, WorkflowChain, WorkflowChainMode, WorkflowGuardsConfig,
};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Top-level configuration for Ralph Orchestrator.
///
/// Supports both v1.x flat format and v2.0 nested format:
/// - v1: `agent: claude`, `max_iterations: 100`
/// - v2: `cli: { backend: claude }`, `event_loop: { max_iterations: 100 }`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // Configuration struct with multiple feature flags
pub struct RalphConfig {
    /// Event loop configuration (v2 nested style).
    #[serde(default)]
    pub event_loop: EventLoopConfig,

    /// CLI backend configuration (v2 nested style).
    #[serde(default)]
    pub cli: CliConfig,

    /// Core paths and settings shared across all hats.
    #[serde(default)]
    pub core: CoreConfig,

    /// Custom hat definitions (optional).
    /// If empty, default planner and builder hats are used.
    #[serde(default)]
    pub hats: HashMap<String, HatConfig>,

    /// Event metadata definitions (optional).
    /// Defines what each event topic means, enabling auto-derived instructions.
    /// If a hat uses custom events, define them here for proper behavior injection.
    #[serde(default)]
    pub events: HashMap<String, EventMetadata>,

    // ─────────────────────────────────────────────────────────────────────────
    // V1 COMPATIBILITY FIELDS (flat format)
    // These map to nested v2 fields for backwards compatibility.
    // ─────────────────────────────────────────────────────────────────────────
    /// V1 field: Backend CLI (maps to cli.backend).
    /// Values: "claude", "kiro", "gemini", "codex", "amp", "pi", "auto", or "custom".
    #[serde(default)]
    pub agent: Option<String>,

    /// V1 field: Fallback order for auto-detection.
    #[serde(default)]
    pub agent_priority: Vec<String>,

    /// V1 field: Path to prompt file (maps to `event_loop.prompt_file`).
    #[serde(default)]
    pub prompt_file: Option<String>,

    /// V1 field: Completion detection string (maps to event_loop.completion_promise).
    #[serde(default)]
    pub completion_promise: Option<String>,

    /// V1 field: Maximum loop iterations (maps to event_loop.max_iterations).
    #[serde(default)]
    pub max_iterations: Option<u32>,

    /// V1 field: Maximum runtime in seconds (maps to event_loop.max_runtime_seconds).
    #[serde(default)]
    pub max_runtime: Option<u64>,

    /// V1 field: Maximum cost in USD (maps to event_loop.max_cost_usd).
    #[serde(default)]
    pub max_cost: Option<f64>,

    // ─────────────────────────────────────────────────────────────────────────
    // FEATURE FLAGS
    // ─────────────────────────────────────────────────────────────────────────
    /// Enable verbose output.
    #[serde(default)]
    pub verbose: bool,

    /// Archive prompts after completion (DEFERRED: warn if enabled).
    #[serde(default)]
    pub archive_prompts: bool,

    /// Enable metrics collection (DEFERRED: warn if enabled).
    #[serde(default)]
    pub enable_metrics: bool,

    // ─────────────────────────────────────────────────────────────────────────
    // DROPPED FIELDS (accepted but ignored with warning)
    // ─────────────────────────────────────────────────────────────────────────
    /// V1 field: Token limits (DROPPED: controlled by CLI tool).
    #[serde(default)]
    pub max_tokens: Option<u32>,

    /// V1 field: Retry delay (DROPPED: handled differently in v2).
    #[serde(default)]
    pub retry_delay: Option<u32>,

    /// V1 adapter settings (partially supported).
    #[serde(default)]
    pub adapters: AdaptersConfig,

    // ─────────────────────────────────────────────────────────────────────────
    // WARNING CONTROL
    // ─────────────────────────────────────────────────────────────────────────
    /// Suppress all warnings (for CI environments).
    #[serde(default, rename = "_suppress_warnings")]
    pub suppress_warnings: bool,

    /// TUI configuration.
    #[serde(default)]
    pub tui: TuiConfig,

    /// Memories configuration for persistent learning across sessions.
    #[serde(default)]
    pub memories: MemoriesConfig,

    /// Tasks configuration for runtime work tracking.
    #[serde(default)]
    pub tasks: TasksConfig,

    /// Lifecycle hooks configuration.
    #[serde(default)]
    pub hooks: HooksConfig,

    /// Skills configuration for the skill discovery and injection system.
    #[serde(default)]
    pub skills: SkillsConfig,

    /// Feature flags for optional capabilities.
    #[serde(default)]
    pub features: FeaturesConfig,

    /// Telemetry and runtime-diagnosis configuration (U1 of the 2026-06-04
    /// Runtime Diagnosis plan). All sub-fields are opt-in: omitting
    /// `telemetry:` from `ralph.yml` is a no-op.
    #[serde(default)]
    pub telemetry: TelemetryConfig,

    /// RObot (Ralph-Orchestrator bot) configuration for Telegram-based interaction.
    #[serde(default, rename = "RObot")]
    pub robot: RobotConfig,

    /// Path to the config file that was loaded (not serialized).
    #[serde(skip)]
    pub config_path: Option<PathBuf>,
}

fn default_true() -> bool {
    true
}

#[allow(clippy::derivable_impls)] // Cannot derive due to serde default functions
impl Default for RalphConfig {
    fn default() -> Self {
        Self {
            event_loop: EventLoopConfig::default(),
            cli: CliConfig::default(),
            core: CoreConfig::default(),
            hats: HashMap::new(),
            events: HashMap::new(),
            // V1 compatibility fields
            agent: None,
            agent_priority: vec![],
            prompt_file: None,
            completion_promise: None,
            max_iterations: None,
            max_runtime: None,
            max_cost: None,
            // Feature flags
            verbose: false,
            archive_prompts: false,
            enable_metrics: false,
            // Dropped fields
            max_tokens: None,
            retry_delay: None,
            adapters: AdaptersConfig::default(),
            // Warning control
            suppress_warnings: false,
            // TUI
            tui: TuiConfig::default(),
            // Memories
            memories: MemoriesConfig::default(),
            // Tasks
            tasks: TasksConfig::default(),
            // Hooks
            hooks: HooksConfig::default(),
            // Skills
            skills: SkillsConfig::default(),
            // Features
            features: FeaturesConfig::default(),
            // Telemetry / runtime diagnosis (U1)
            telemetry: TelemetryConfig::default(),
            // RObot (Ralph-Orchestrator bot)
            robot: RobotConfig::default(),
            // Config file path (set at load time)
            config_path: None,
        }
    }
}
