//! # ralph-core
//!
//! Core orchestration functionality for the Ralph Orchestrator framework.
//!
//! This crate provides:
//! - The main orchestration loop for coordinating multiple agents
//! - Configuration loading and management
//! - State management for agent sessions
//! - Message routing between agents
//! - Terminal capture for session recording
//! - Benchmark task definitions and workspace isolation

pub mod agent_doc_sync;
#[cfg(feature = "recording")]
mod cli_capture;
pub mod completion_emit;
pub mod config;
/// U7a deterministic-correction injection — replaces
/// `task.resume` events on the policy rejection path with
/// in-prompt `## ORCHESTRATOR CORRECTION` blocks.
pub mod correction;
pub mod diagnosis;
pub mod diagnostics;
pub mod drift;
pub mod emit_schema_hint;
pub mod ephemeral_isolation;
mod event_logger;
pub mod event_loop;
pub mod event_origin;
mod event_parser;
mod event_policy;
mod event_projection;
mod event_reader;
pub mod execution_contract;
pub mod file_lock;
pub mod flow_lifecycle;
mod git_ops;
mod handoff;
pub mod hat_identity;
pub mod hat_lifecycle;
mod hat_registry;
mod hatless_ralph;
pub mod hooks;
pub mod recovery_runtime;
pub mod shipper_reason;
/// 2026-07-03-001 plan U2: rusqlite-backed wave orchestration
/// domain types + persistence trait. U3-U5 introduce the in-memory
/// and SQLite implementations; U8 wires the coordinator; U11/U12
/// surface the dispatcher/recover branches. Until then this module
/// stays open: only the type-level contract exists.
pub mod supervisor;
pub use emit_schema_hint::{
    build_publish_emit_section, fix_hint_for_hat_topic, format_emit_json_example,
};
mod instructions;
mod landing;
pub mod loop_authorization;
pub mod loop_completion;
pub mod loop_context;
pub mod loop_history;
pub mod loop_lock;
mod loop_name;
pub mod loop_registry;
mod loop_state_snapshot;
mod memory;
pub mod memory_parser;
mod memory_store;
pub mod merge_queue;
pub mod payload_contract;
pub mod plan_baseline;
pub mod planning_session;
pub mod preflight;
/// `preset::engine` — preset-agnostic execution engine (plan
/// 2026-06-20-001 U1/U2). Reads protocol SSOT from the embedded
/// `EventLoopConfig`; performs gate / projection / lint operations
/// without duplicating payload field tables in Rust.
pub mod preset;
pub mod preset_lint;
pub mod preset_validator;
/// Runtime profile overlay loader (U2 of plan 2026-06-25-002). Parses
/// `<scope>:<name>` specs, resolves them under the repo or user XDG
/// config root, reads `<profile>/<preset>/<hat-id>.md` fragments, and
/// appends them to `RalphConfig.hats[hat].instructions`. U1 (the
/// `profiles.default` config block) lives in `crate::config::profiles`.
pub mod profiles;
pub mod runtime_contract;
#[cfg(feature = "recording")]
mod session_player;
#[cfg(feature = "recording")]
mod session_recorder;
pub mod skill;
pub mod skill_registry;
pub mod state;
mod state_file_injector;
pub mod state_machine;
pub mod state_projector;
pub mod step_handoff;
mod summary_writer;
pub mod task;
pub mod task_definition;
pub mod task_store;
pub mod testing;
mod text;
mod urgent_steer;
pub mod utils;
/// U4 unified validation pipeline — wraps origin / publisher /
/// required-fields / step-handoff / execution-contract /
/// workflow-guard as stateless [`ValidationRule`]s.
pub mod validation;
pub mod wave_context;
pub mod wave_detection;
pub mod wave_prompt;
pub mod wave_tracker;
pub mod workflow_contract;
pub mod workspace;
pub mod worktree;

#[cfg(feature = "recording")]
pub use cli_capture::{CliCapture, CliCapturePair};
pub use config::{
    ActivationObligation, AgentDocSyncConfig, AggregateConfig, AggregateMode, CliConfig,
    ConditionalEmission, ConfigError, CoordJoinMode, CoreConfig, DriftConfig, EventFilterConfig,
    EventFilterMode, EventLoopConfig, EventMetadata, EventPolicyConfig, EventPolicyMode,
    EventProjectionConfig, EventSchema, FeaturesConfig, HatBackend, HatConfig, HookStage,
    InjectMode, MalformedJsonlPolicy, MemoriesConfig, MemoriesFilter, OnErrorPolicy, PayloadType,
    Phase, PhaseConfig, PreflightExtensionsConfig, PreflightHook, ProfileScope, ProfileSpec,
    ProfilesConfig, ProjectionMode, ProjectionRule, RalphConfig, RuntimeDiagnosisConfig,
    ScratchpadConfig, SkillOverride, SkillsConfig, StateFileEntry, StateFileFormat,
    StateFilesConfig, StepHandoffConfig, TelemetryConfig, TriggerContext, TriggerPredicate,
    ViolationAction, WarmupConfig, obligation_satisfied,
};
pub use profiles::{
    ProfileFragment, ProfilesError, ResolvedProfileFragments, apply_profile_fragments,
    apply_profile_fragments_with, parse_profile_spec, resolve_profile_dir,
    resolve_profile_dir_with, resolve_profile_fragments, resolve_profile_fragments_with,
};

// Re-export loop_name types (also available via FeaturesConfig.loop_naming)
pub use diagnosis::{
    DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, DriftJournalEntry, DriftMetric,
    EvidenceKind, EvidenceRef, RecoveryDiagnosisEnvelope, RecoveryDiagnosisEnvelopeBuilder,
    RecoveryJournalEntry,
};
pub use diagnostics::{DiagnosisSummary, DiagnosticsCollector, DiagnosticsOptions};
pub use drift::{
    DeclaredEdges, DriftDetector, DriftFinding, DriftObserver, DriftWindow,
    EMIT_CADENCE_MIN_SAMPLES, EventSnapshot, RequiredFields,
};
pub use event_logger::{EventHistory, EventLogger, EventRecord};
pub use event_loop::{
    EventLoop, LoopState, ProcessedEvents, ProcessedEventsWithWaves, TerminationReason,
    U2_REJECTION_RETRY_LIMIT, UserPrompt,
    rejection::{
        NonRetryableReason, Rejection, RejectionStage, build_task_resume_payload,
        rejection_from_origin, resolve_target_hat,
    },
};
pub use event_origin::{
    RALPH_CONTROL_TOPICS, is_orchestrator_control_topic, is_orchestrator_diagnostic_topic,
};
pub use event_parser::EventParser;
pub use event_policy::{
    PolicyDecision, PolicyFinding, PolicyRejection, PolicyRuntimeState, ViolationType,
    check_topic_deny_rules, validate_event, validate_event_with_hat,
};
pub use event_projection::apply_projection;
pub use event_reader::{Event, EventReader, MalformedLine, ParseResult};
pub use file_lock::{FileLock, LockGuard as FileLockGuard, LockedFile};
pub use flow_lifecycle::{
    FlowLifecycleRecord, FlowLifecycleRegistry, FlowPhase, WaveDeadlines, effective_wave_deadlines,
    reconcile_wave_timeouts,
};
pub use git_ops::{
    AutoCommitResult, GitOpsError, auto_commit_changes, clean_stashes, get_changed_files_between,
    get_commit_summary, get_current_branch, get_head_sha, get_recent_files,
    has_uncommitted_changes, is_working_tree_clean, prune_remote_refs,
};
pub use handoff::{HandoffError, HandoffResult, HandoffWriter};
pub use hat_lifecycle::{
    ActivationKey, ActivationLifecycleTracker, ActivationSnapshot, Clock, FakeClock,
    SystemTimeClock,
};
pub use hat_registry::HatRegistry;
pub use hatless_ralph::{HatInfo, HatTopology, HatlessRalph};
pub use hooks::{
    HookDefaults, HookEngine, HookExecutor, HookExecutorContract, HookExecutorError,
    HookInvocationPayload, HookMutationConfig, HookOnError, HookPayloadBuilderInput,
    HookPayloadContext, HookPayloadContextInput, HookPayloadIteration, HookPayloadLoop,
    HookPayloadMetadata, HookPhaseEvent, HookRunRequest, HookRunResult, HookSpec, HookStreamOutput,
    HookSuspendMode, HooksConfig, ResolvedHookSpec, SUSPEND_STATE_SCHEMA_VERSION,
    SuspendLifecycleState, SuspendStateRecord, SuspendStateStore, SuspendStateStoreError,
};
pub use instructions::InstructionBuilder;
pub use landing::{LandingConfig, LandingError, LandingHandler, LandingResult};
pub use loop_completion::{CompletionAction, CompletionError, LoopCompletionHandler};
pub use loop_context::LoopContext;
pub use loop_history::{HistoryError, HistoryEvent, HistoryEventType, HistorySummary, LoopHistory};
pub use loop_lock::{LockError, LockGuard, LockMetadata, LockStatus, LoopLock};
pub use loop_name::{LoopNameGenerator, LoopNamingConfig};
pub use loop_registry::{LoopEntry, LoopRegistry, RegistryError};
pub use loop_state_snapshot::{
    LoopStateSnapshot, PolicyFindingSnapshot, WorkflowInstanceSnapshot, replay_events_to_snapshot,
};
pub use memory::{Memory, MemoryType, MemoryVisibility};
pub use memory_store::{
    DEFAULT_MEMORIES_PATH, MarkdownMemoryStore, format_memories_as_markdown, truncate_to_budget,
};
pub use merge_queue::{
    MergeButtonState, MergeEntry, MergeEvent, MergeEventType, MergeOption, MergeQueue,
    MergeQueueError, MergeState, SteeringDecision, merge_button_state, merge_execution_summary,
    merge_needs_steering, smart_merge_summary,
};
pub use plan_baseline::{
    PlanBaselineError, derive_baseline_key, derive_plan_id, ensure_plan_baseline,
    ensure_plan_baseline_from_head, plan_baseline_path, read_plan_baseline,
    write_plan_baseline_from_head,
};
pub use planning_session::{
    ConversationEntry, ConversationType, PlanningSession, PlanningSessionError, SessionMetadata,
    SessionStatus,
};
pub use preflight::{
    AcceptanceCriterion, CheckResult, CheckStatus, PreflightCheck, PreflightReport,
    PreflightRunner, extract_acceptance_criteria, extract_all_criteria, extract_criteria_from_file,
};
pub use preset::engine::gates::RejectionKind;
pub mod runtime_state;
pub use preset_validator::{
    TopologyError, TopologyErrorKind, TopologyValidationResult, validate_preset_topology,
};
pub use runtime_contract::{
    FindingSeverity, FindingSource, FindingStage, LOOP_RUNNER_INTERNAL_TOPICS,
    RuntimeContractAggregator, RuntimeContractFinding, RuntimeContractReport,
    RuntimeContractStrictness, detect_orphan_topics,
};
#[cfg(feature = "recording")]
pub use session_player::{PlayerConfig, ReplayMode, SessionPlayer, TimestampedRecord};
#[cfg(feature = "recording")]
pub use session_recorder::{Record, SessionRecorder};
pub use skill::{SkillEntry, SkillFrontmatter, SkillSource, parse_frontmatter};
pub use skill_registry::SkillRegistry;
pub use state_file_injector::inject_state_files;
pub use state_machine::{
    InstanceState, StateMachineDecision, StateMachineFinding, StateMachineRuntimeState,
    StateMachineStateSummary,
};
pub use summary_writer::{DiagnosisHint, DiagnosisReference, SummaryWriter};
pub use task::{Task, TaskStatus};
pub use task_definition::{
    TaskDefinition, TaskDefinitionError, TaskSetup, TaskSuite, Verification,
};
pub use task_store::TaskStore;
pub use text::{floor_char_boundary, truncate_with_ellipsis};
pub use urgent_steer::{UrgentSteerRecord, UrgentSteerStore};
pub use wave_detection::{
    DEFAULT_MAX_WAVE_TOTAL, DetectedWave, PartialWavePolicy, RejectedWave, WaveDetectionOutcome,
    WaveRejection, detect_all_wave_events, detect_all_wave_events_capped,
    detect_all_wave_events_with_policy, detect_wave_events, detect_wave_events_capped,
};
pub use wave_prompt::{WaveWorkerContext, build_wave_worker_prompt};
pub use wave_tracker::{
    CompletedWave, MAX_DIMENSION_RETRIES_PER_SLOT, WaveFailure, WaveProgress, WaveResult,
    WaveTracker,
};
pub use workspace::{
    CleanupPolicy, TaskWorkspace, VerificationResult, WorkspaceError, WorkspaceInfo,
    WorkspaceManager,
};
pub use worktree::{
    ReusableWorktree, SyncStats, Worktree, WorktreeConfig, WorktreeError,
    clean_worktree_runtime_artifacts, create_worktree, ensure_gitignore, find_reusable_worktree,
    list_ralph_worktrees, list_worktrees, remove_worktree, sync_working_directory_to_worktree,
    worktree_exists,
};
