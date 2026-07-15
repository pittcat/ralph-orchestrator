//! Configuration types for the Ralph Orchestrator.
//!
//! This module supports both v1.x flat configuration format and v2.0 nested format.
//! Users can switch from Python v1.x to Rust v2.0 with zero config changes.

pub mod agent_doc_sync;
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
mod precheck;
mod preflight_ext;
mod ralph_config;
mod skills;
mod state_files;
pub(crate) mod state_machine;
mod tasks;
mod telemetry;
mod v1_adapters;
mod warning;
mod workflow_contract;
pub(crate) mod workflow_guards;

pub mod multi_hat_policy;
mod state_projection;
// U1 of plan 2026-06-25-002: `profiles.default` config block. Pure data
// types + deserialization — no FS access, no fragment loading (U2).
pub mod profiles;

pub use agent_doc_sync::{AgentDocSyncConfig, OnErrorPolicy};
pub use cli::{CliConfig, TuiConfig};
pub use core::{CoreConfig, ScratchpadConfig};
pub use error::ConfigError;
pub use event_filter::{EventFilterConfig, EventFilterMode};
pub use event_policy::{
    CompletionAfterTerminalAction, EventPolicyConfig, EventPolicyMode, TopicDenyRule,
    ViolationAction,
};
pub use event_projection::{EventProjectionConfig, ProjectionMode, ProjectionRule};
pub use execution_contracts::ExecutionContractRule;
pub use features::FeaturesConfig;
pub use hat::{
    ActivationObligation, AggregateConfig, AggregateMode, ConditionalEmission, EventMetadata,
    HatBackend, HatConfig, TriggerContext, TriggerPredicate, obligation_satisfied,
    resolve_missing_event_grace_secs,
};
pub use hooks::{
    HookDefaults, HookMutationConfig, HookOnError, HookPhaseEvent, HookSpec, HookSuspendMode,
    HooksConfig,
};
pub use loop_config::{
    ElementConstraint, EventFieldDoc, EventLoopConfig, EventSchema, FlowDeclarationConfig,
    FlowStepConfig, HandoffEnvelopeConfig, HatAllowedValues, HintCondition, HintOp,
    MechanismConfig, PayloadType, Phase, PhaseConfig, ProgressStewardConfig, RoutingHintConfig,
    SupervisorConfig, TriggerContextConfig, VerdictGateConfig, WarmupConfig,
};
pub use memories::{InjectMode, MemoriesConfig, MemoriesFilter};
pub use multi_hat_policy::{
    MULTI_HAT_ISOLATION_LIMIT, MultiHatPolicyViolation, evaluate_multi_hat_isolation,
};
pub use precheck::{PrecheckConfig, PrecheckOnFail, PrecheckRule, precheck_runtime_enabled};
pub use preflight_ext::{HookStage, PreflightExtensionsConfig, PreflightHook};
pub use profiles::{ProfileScope, ProfileSpec, ProfilesConfig};
pub use skills::{SkillOverride, SkillsConfig};
pub use state_files::{StateFileEntry, StateFileFormat, StateFilesConfig};
pub use state_machine::{BusinessAfterTerminalAction, DuplicateTerminalAction, StateMachineConfig};
pub use state_projection::{StateProjectionAction, StateProjectionConfig};
pub use tasks::TasksConfig;
pub use telemetry::{
    CoordJoinMode, DriftConfig, MalformedJsonlPolicy, RuntimeDiagnosisConfig, TelemetryConfig,
};
pub use v1_adapters::{AdapterSettings, AdaptersConfig};
pub use warning::ConfigWarning;
pub use workflow_contract::{
    HANDOFF_DISPATCH_TIMEOUT_DEFAULT_SECONDS, HANDOFF_DISPATCH_TIMEOUT_MAX_SECONDS,
    HANDOFF_TOPIC_SEEDS, StepHandoffConfig, WorkflowContractConfig,
};
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

    /// P0-3 (2026-06-27 adversarial review):
    /// top-level `mechanism:` block. The
    /// `FlowStepScopeStage` (U9) and the
    /// `flow_declaration_missing` lint both
    /// consume this field; the runtime
    /// `build_stage_pipeline_from_config`
    /// reads it via `serde_yaml::to_string`
    /// (the round-trip now preserves the
    /// `mechanism:` block because the field
    /// lives on `RalphConfig`). Optional —
    /// presets that have not opted in fall
    /// back to the minimal flow declaration
    /// (see `event_loop::mod::minimal_flow_declaration_yaml`).
    #[serde(default)]
    pub mechanism: Option<MechanismConfig>,

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
    /// Values: "claude", "gemini", "codex", "opencode", "pi", "traecli", "auto", or "custom".
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

    /// Agent doc sync configuration for managed agent doc blocks.
    /// When enabled (default), the sync engine injects curated constraint
    /// blocks into `CLAUDE.md` / `AGENTS.md` before backend spawn.
    #[serde(default)]
    pub agent_doc_sync: AgentDocSyncConfig,

    /// Profile overlays (U1 of plan 2026-06-25-002). The `default` list
    /// activates whenever `ralph run` starts; the CLI `--profile` flags
    /// stack on top of it (U3 / U4). Omitting `profiles:` from
    /// `ralph.yml` is a no-op — the field defaults to an empty list.
    #[serde(default)]
    pub profiles: ProfilesConfig,

    // ─────────────────────────────────────────────────────────────────────────
    // PRESET LINT FIELDS (U1 of plan 2026-06-08-003)
    // ─────────────────────────────────────────────────────────────────────────
    /// Topic ownership map: topic → owner hat(s).
    ///
    /// In strict mode, a topic's owner is its sole direct publisher.
    /// Non-owner hats publishing the owner's topic produce
    /// `cross_hat_unauthorized_publish` findings. Omitting this field
    /// (empty map) means no ownership constraints are enforced.
    #[serde(default)]
    pub topic_owners: HashMap<String, Vec<String>>,

    /// Tokens exempt from the lowercase dot-case topic format validator.
    ///
    /// Whitelisted tokens are reported as "exempt" rather than
    /// "invalid" in lint output. The format check still runs on
    /// non-whitelisted tokens.
    #[serde(default)]
    pub topic_format_whitelist: Vec<String>,

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
            // Agent doc sync
            agent_doc_sync: AgentDocSyncConfig::default(),
            // Profile overlays (U1 of plan 2026-06-25-002)
            profiles: ProfilesConfig::default(),
            // Preset lint (U1 of plan 2026-06-08-003)
            topic_owners: HashMap::new(),
            topic_format_whitelist: Vec::new(),
            // Config file path (set at load time)
            config_path: None,
            // P0-3 (2026-06-27 adversarial review):
            // the mechanism foundation opt-in.
            // None by default so the runtime falls
            // back to the minimal flow declaration
            // (see
            // `event_loop::mod::minimal_flow_declaration_yaml`).
            mechanism: None,
        }
    }
}
