//! [`LedgerSnapshot`] — the unified state projection derived from
//! the [`StateLedger`] commit log.
//!
//! Plan ref: U1 of
//! `docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md`.
//!
//! The snapshot is a pure value type. State changes apply through
//! [`LedgerSnapshot::apply_delta`] and are the only path to mutate
//! fields. The `Default` impl is a cold-start ledger (all counters
//! at zero, all collections empty). The commit log is replayed on
//! top of the cold start to reconstruct the live snapshot.
//!
//! ## U1 scope
//!
//! U1 implements the structural model only. The runtime does not
//! yet read or write this snapshot — see U2 for the projector
//! migration. The struct exists now so U2 / U3 / U5 can plug into
//! it without further refactor.
//!
//! Fields mirror the inventory in
//! `docs/plans/2026-06-21-002-unified-state-inventory.md` table
//! 1.1 — every field marked "进入 ledger" is present.

use std::collections::{BTreeMap, HashMap, HashSet};

use ralph_proto::HatId;
use serde::{Deserialize, Serialize};

use crate::event_loop::review_step_state::ReviewStepTracker;
use crate::event_loop::{RejectionDigestEntry, TerminationReason};
use crate::flow_lifecycle::FlowLifecycleRegistry;
use crate::state_projector::ProjectionContext;
use crate::step_handoff::ProgressSnapshot;
use crate::task::Task;
use crate::workflow_contract::HandoffTracker;
use crate::event_policy::PolicyRuntimeState;
use crate::state_machine::StateMachineRuntimeState;

use super::commit::{CommitDelta, TaskTransition};

/// Single source of truth for the loop's runtime state.
///
/// Replaces the in-memory trackers spread across `LoopState`,
/// `StateProjector::ProjectionContext`, `RecoveryResponder`,
/// `DriftEngine`, `PolicyRuntimeState`, `StateMachineRuntimeState`,
/// `FlowLifecycleRegistry`, `HandoffTracker`, `ReviewStepTracker`,
/// `WorkflowProgress`, and the legacy `tasks.jsonl` / `progress.md`
/// ledgers.
///
/// U1 defines the model and persistence; U2 onwards migrate the
/// read/write sites.
#[derive(Debug, Default, Clone)]
pub struct LedgerSnapshot {
    // ---- iter / flow control ------------------------------------
    /// Current loop iteration. Mirrors `LoopState::iteration`.
    pub iteration: u32,

    /// `## ORCHESTRATOR CONTEXT` `completion_requested` flag.
    pub completion_requested: bool,
    /// `completion_honored` flag.
    pub completion_honored: bool,
    /// `cancellation_requested` flag.
    pub cancellation_requested: bool,
    /// Bootstrap complete signal (U3 coordinator).
    pub bootstrap_complete: bool,
    /// Bootstrap failed signal (U3 coordinator).
    pub bootstrap_failed: bool,

    /// `isolated_turn_business_event_accepted` flag (U3 P0).
    pub isolated_turn_business_event_accepted: bool,
    /// `steward_woken_this_turn` flag.
    pub steward_woken_this_turn: bool,
    /// `stall_detector_had_events` per-turn flag.
    pub stall_detector_had_events: bool,
    /// `current_isolated_hat` pin (isolated mode).
    pub current_isolated_hat: Option<HatId>,

    // ---- runtime counters ---------------------------------------
    /// hat→hat handoff sequence within the current iteration.
    pub hat_handoff_seq: u32,
    /// Consecutive failures.
    pub consecutive_failures: u32,
    /// Cumulative cost in USD.
    pub cumulative_cost: f64,
    /// Consecutive blocked events from the same hat.
    pub consecutive_blocked: u32,
    /// Per-task thrash counts.
    pub task_block_counts: HashMap<String, u32>,
    /// Tasks that have been abandoned.
    pub abandoned_tasks: Vec<String>,
    /// Planner re-dispatches of abandoned tasks.
    pub abandoned_task_redispatches: u32,
    /// Consecutive malformed JSONL lines.
    pub consecutive_malformed_events: u32,
    /// Consecutive hard-gate triggers.
    pub consecutive_hard_gates: u32,
    /// Consecutive same-signature events (stale loop detection).
    pub consecutive_same_signature: u32,
    /// Consecutive no-progress turns (U5).
    pub consecutive_no_progress_turns: u32,
    /// Consecutive steward activations (U5).
    pub consecutive_steward_activations: u32,
    /// Consecutive completion rejections (stale-breaker).
    pub consecutive_completion_rejections: u32,
    /// Consecutive engine gate rejections (2026-06-20-001 KTD-7).
    pub consecutive_engine_gate_rejections: u32,
    /// `lint_circuit_breaker_tripped` latch.
    pub lint_circuit_breaker_tripped: bool,

    /// Per-hat activation counts (max_activations).
    pub hat_activation_counts: HashMap<HatId, u32>,
    /// Per-stall-recovery counts.
    pub stall_recovery_counts: HashMap<String, u32>,
    /// Per-rejection-key retry counts (U2).
    pub rejection_retry_counts: HashMap<String, u32>,
    /// Per-rejection-key last-iteration seen (responder dedup).
    pub rejection_last_iteration: HashMap<String, u32>,

    /// Hats that have emitted `<hat>.exhausted`.
    pub exhausted_hats: HashSet<HatId>,

    /// Last completed rejection signature.
    pub completion_rejection_signature: Option<String>,
    /// Last progress-fingerprint hash at completion rejection.
    pub last_rejection_fingerprint: u64,

    /// Invariant violation count (U3).
    pub invariant_violation_count: u32,
    /// Last invariant violation rule id.
    pub last_invariant_violation: Option<String>,

    // ---- runtime cache: hats / topics ---------------------------
    /// Last hat that executed.
    pub last_hat: Option<HatId>,
    /// Hat that emitted the last blocked event.
    pub last_blocked_hat: Option<HatId>,
    /// Hat IDs active in the last iteration.
    pub last_active_hat_ids: Vec<HatId>,
    /// Topics serialized as `(topic, payload)` for the obligation
    /// replay. The original `LoopState::last_activation_events`
    /// is `Vec<Event>`; we store a slim serialized form because
    /// the full `Event` envelope is also persisted in
    /// `recovery.jsonl` (the loop's source of truth for replays).
    pub last_activation_events: Vec<ObligationTriggerRecord>,
    /// All topics seen during the loop lifetime.
    pub seen_topics: HashSet<String>,
    /// Last emitted event signature (stale loop detection).
    pub last_emitted_signature: Option<String>,

    // ---- runtime cache: rejection digest ------------------------
    /// Rejection digest for prompt injection.
    pub recent_rejection_digest: BTreeMap<String, RejectionDigestEntry>,

    // ---- tasks / progress (legacy) -----------------------------
    /// Tasks ledger, in-memory mirror of `.ralph/agent/tasks.jsonl`.
    pub tasks: Vec<Task>,
    /// Progress snapshot, mirror of `.ralph/agent/progress.md`.
    pub progress: ProgressSnapshot,

    // ---- workflow / review / handoff ----------------------------
    /// Workflow progress for guarded chains. The keys are
    /// `chain_name::instance_key` (or `chain_name::` for the
    /// global instance); the value is the highest phase reached.
    pub workflow_phases: HashMap<String, u32>,

    /// Per-step review terminal tracker. The runtime continues to
    /// mutate the embedded tracker via its existing public API
    /// (`observe_accepted` / `close_wave`); the commit log records
    /// the diff so `replay_from_disk` can rebuild an equivalent
    /// tracker on cold start.
    pub review_step_tracker: ReviewStepTracker,

    /// Handoff deadline tracker (WRC-U4). Same note as
    /// `review_step_tracker` — the runtime mutates the embedded
    /// tracker; the commit log records the diff.
    pub handoff_tracker: HandoffTracker,

    // ---- policy / state machine / flow --------------------------
    /// Event policy runtime state (opt-in).
    pub policy_runtime: Option<PolicyRuntimeState>,
    /// State machine runtime state (opt-in).
    pub state_machine_runtime: Option<StateMachineRuntimeState>,
    /// Flow lifecycle registry.
    pub flow_lifecycle: FlowLifecycleRegistry,

    // ---- scope-violation circuit breaker (R6) -------------------
    /// Original termination reason when the scope-violation
    /// circuit breaker trips.
    pub scope_violation_circuit_breaker_tripped: Option<TerminationReason>,

    // ---- flow control: pending handoffs -------------------------
    /// Pending recovery hat pin (U3 hard-gate).
    pub pending_recovery_hat: Option<HatId>,
    /// Pending synthesizer timeout pin (R1).
    pub pending_synthesizer_timeout: Option<String>,
    /// Ephemeral relocations cleared on build_prompt. Stored as
    /// the original `RelocationRecord` is more complex than the
    /// ledger needs; the runtime reads the rich form from
    /// `LoopState`, the ledger records the audit trail.
    pub last_ephemeral_relocations: Vec<String>,

    // ---- recoverable exhaustion buffer (U2) --------------------
    /// Reason codes that hit the recoverable bucket. The
    /// `Vec<String>` is a placeholder for the full
    /// `RecoverableExhaustion` struct; full type lands in U2.
    pub recoverable_exhaustion_buffer: Vec<String>,

    // ---- work.done dedup (U4) ----------------------------------
    /// `work.done` keys seen at any point in the loop.
    pub work_done_seen_tasks: HashSet<String>,

    // ---- hat activation clock (U2 R3) --------------------------
    /// RFC3339 timestamp of the most recent activation of each
    /// hat. Stored as RFC3339 (not `Instant`) so the snapshot
    /// round-trips through JSON without an epoch anchor.
    pub hat_activation_at: HashMap<HatId, String>,

    // ---- obligation triggers (R4/R5) ---------------------------
    /// Snapshot of the trigger events that activated the most
    /// recent hat. Mirrors `last_activation_events` semantically;
    /// kept separate so `pending_obligation_triggers` is its own
    /// clear-once buffer.
    pub pending_obligation_triggers: Vec<ObligationTriggerRecord>,

    // ---- lint resume hint (U4b) --------------------------------
    /// In-memory lint failure hint. Stays in-memory per the U4b
    /// plan; replicated to the ledger for crash-recovery parity
    /// with `LoopState::pending_lint_resume`.
    pub pending_lint_resume: Option<SerializedLintResumeHint>,

    // ---- timing ------------------------------------------------
    /// Wall-clock start of the loop (RFC3339). Replaces
    /// `LoopState::started_at` (which used `Instant`).
    pub started_at_ts: Option<String>,

    // ---- verdict tracking --------------------------------------
    /// Most recent verdict gate payload.
    pub last_verdict_payload: Option<String>,
    /// Most recent verdict topic.
    pub last_verdict_topic: Option<String>,
    /// Most recent upstream verdict payload.
    pub last_upstream_verdict_payload: Option<String>,

    // ---- recovery envelope state (U6 responder) ---------------
    /// Sticky findings the responder still has to publish. U6
    /// wires the responder; U1 models the storage only.
    pub pending_recovery_findings: Vec<String>,
    /// Tracked recovery keys (de-dup window).
    pub recovery_tracked_keys: HashSet<String>,

    // ---- drift engine state (U6) ------------------------------
    /// Last drift engine action. U6 wires the engine; U1 models
    /// the storage only.
    pub last_drift_action: Option<String>,
}

/// Serialized form of an obligation trigger event. We do not
/// persist full `Event` objects (the `EventReader` envelope is
/// the source of truth on disk via `recovery.jsonl`); the ledger
/// only needs enough state to drive `last_activation_events` in
/// `LoopState`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObligationTriggerRecord {
    pub topic: String,
    #[serde(default)]
    pub payload: Option<String>,
    #[serde(default)]
    pub hat: Option<String>,
}

/// Serialized form of `LintResumeHint`. Mirrors the public
/// fields of the original struct so U4b can rebuild it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedLintResumeHint {
    pub failing_topic: String,
    pub target_hat: String,
    pub reason: String,
    pub created_at_ts: String,
}

impl LedgerSnapshot {
    /// Build a cold-start snapshot with every collection empty and
    /// every counter at zero. This is the starting point for
    /// `replay_from_disk`.
    pub fn cold_start() -> Self {
        Self::default()
    }

    /// Apply a [`CommitDelta`] to the snapshot.
    ///
    /// This is the SSOT for "every delta must be handled" — the
    /// unit test [`crate::state::tests::apply_delta_is_exhaustive`]
    /// enforces that every variant has a concrete branch here.
    ///
    /// For variants that target an embedded registry
    /// (`HandoffTracker`, `FlowLifecycleRegistry`, etc.) the
    /// mutation is delegated to the registry's own public API.
    /// The commit log records the diff so `replay_from_disk` can
    /// rebuild an equivalent registry on cold start; until U2
    /// wires the full replay path, the runtime continues to
    /// rebuild the registries from the live `LoopState` fields.
    pub fn apply_delta(&mut self, delta: &CommitDelta) {
        match delta {
            CommitDelta::NoOp => {}
            CommitDelta::TaskLifecycle { task_id, transition } => {
                apply_task_lifecycle(&mut self.tasks, task_id, *transition);
            }
            CommitDelta::ProgressUpdate {
                completed_step,
                current_step,
            } => {
                if let Some(done) = completed_step {
                    let trimmed = done.trim();
                    if !trimmed.is_empty()
                        && !self
                            .progress
                            .completed_steps
                            .iter()
                            .any(|s| s == trimmed)
                    {
                        self.progress.completed_steps.push(trimmed.to_string());
                    }
                }
                if let Some(step) = current_step {
                    self.progress.current_step = Some(step.clone());
                }
            }
            CommitDelta::PlanComplete {
                final_step,
                closed_count: _,
            } => {
                if let Some(step) = final_step {
                    let trimmed = step.trim();
                    if !trimmed.is_empty()
                        && !self
                            .progress
                            .completed_steps
                            .iter()
                            .any(|s| s == trimmed)
                    {
                        self.progress.completed_steps.push(trimmed.to_string());
                    }
                    self.progress.current_step = Some(step.clone());
                }
                for task in self.tasks.iter_mut() {
                    if !task.status.is_terminal() {
                        task.status = crate::task::TaskStatus::Closed;
                        task.closed = Some(chrono::Utc::now().to_rfc3339());
                    }
                }
            }
            CommitDelta::RejectionRecorded { key, .. } => {
                *self.rejection_retry_counts.entry(key.clone()).or_insert(0) += 1;
            }
            CommitDelta::RejectionBudgetTripped { .. } => {
                // The runner consumes the trip directly off the
                // commit log; the snapshot is not mutated.
            }
            CommitDelta::HandoffAccepted { .. } => {
                // The handoff tracker is updated via the runtime's
                // own `on_handoff_accepted` call; the commit log
                // records the diff for replay. U2 will wire the
                // replay path to `HandoffTracker::on_handoff_accepted`.
            }
            CommitDelta::WorkflowPhaseAdvanced {
                chain_name,
                instance_key,
                new_phase,
            } => {
                let key = match instance_key {
                    Some(k) => format!("{chain_name}::{k}"),
                    None => format!("{chain_name}::"),
                };
                let entry = self.workflow_phases.entry(key).or_insert(0);
                if *new_phase > *entry {
                    *entry = *new_phase;
                }
            }
            CommitDelta::CounterChanged { counter, new_value } => {
                apply_counter_change(self, counter, *new_value);
            }
            CommitDelta::SeenTopic { topic } => {
                self.seen_topics.insert(topic.clone());
            }
            CommitDelta::CompletionRequested => self.completion_requested = true,
            CommitDelta::CompletionHonored => self.completion_honored = true,
            CommitDelta::CancellationRequested => self.cancellation_requested = true,
            CommitDelta::StewardWoken => self.steward_woken_this_turn = true,
            CommitDelta::SnapshotReset => {
                // No-op (see variant docs).
            }
            CommitDelta::HatActivationCounted { hat, new_count } => {
                self.hat_activation_counts.insert(hat.clone(), *new_count);
            }
            CommitDelta::HatExhausted { hat } => {
                self.exhausted_hats.insert(hat.clone());
            }
            CommitDelta::RejectionLastIteration { key, iteration } => {
                self.rejection_last_iteration.insert(key.clone(), *iteration);
            }
            CommitDelta::StallRecoveryCounted { key, new_count } => {
                self.stall_recovery_counts.insert(key.clone(), *new_count);
            }
            CommitDelta::TaskBlockCounted { task_id, new_count } => {
                self.task_block_counts.insert(task_id.clone(), *new_count);
            }
            CommitDelta::TaskAbandoned { task_id } => {
                if !self.abandoned_tasks.iter().any(|t| t == task_id) {
                    self.abandoned_tasks.push(task_id.clone());
                }
            }
            CommitDelta::ReviewStepUpdated { .. } => {
                // The review step tracker is updated via the
                // runtime's own `observe_accepted` call; the
                // commit log records the diff for replay.
            }
            CommitDelta::HandoffTrackerUpdated { .. } => {
                // See `HandoffAccepted` above.
            }
            CommitDelta::FlowLifecycleUpdated { .. } => {
                // See `ReviewStepUpdated` above.
            }
            CommitDelta::RejectionDigestUpdated {
                reason_code,
                count,
                last_message,
                last_ts,
                last_topic,
            } => {
                self.recent_rejection_digest.insert(
                    reason_code.clone(),
                    RejectionDigestEntry {
                        count: *count,
                        last_message: last_message.clone(),
                        last_ts: last_ts.clone(),
                        last_topic: last_topic.clone(),
                    },
                );
            }
        }
    }

    /// Inherit task + progress state from a legacy
    /// `ProjectionContext` (used on cold start when the legacy
    /// `tasks.jsonl` / `progress.md` files are present but
    /// `ledger.jsonl` is not). U3 will wire this into the
    /// bootstrap path; U1 just exposes the helper.
    pub fn seed_from_projection_context(&mut self, ctx: &ProjectionContext) {
        self.tasks = ctx.tasks_cache.clone();
        self.progress = ctx.progress_cache.clone();
    }

    /// Borrow the embedded `ReviewStepTracker`. Provided so
    /// downstream code does not have to reach through a private
    /// field.
    pub fn review_step_tracker(&self) -> &ReviewStepTracker {
        &self.review_step_tracker
    }

    /// Mutable access to the `ReviewStepTracker`. Reserved for
    /// legacy call sites that still mutate the tracker directly;
    /// the ledger path is preferred.
    pub fn review_step_tracker_mut(&mut self) -> &mut ReviewStepTracker {
        &mut self.review_step_tracker
    }
}

fn apply_task_lifecycle(tasks: &mut [Task], task_id: &str, transition: TaskTransition) {
    if let Some(task) = tasks.iter_mut().find(|t| t.id == task_id) {
        match transition {
            TaskTransition::Started => task.start(),
            TaskTransition::Reopened => task.reopen(),
            TaskTransition::Closed => {
                task.status = crate::task::TaskStatus::Closed;
                task.closed = Some(chrono::Utc::now().to_rfc3339());
            }
            TaskTransition::Failed => {
                task.status = crate::task::TaskStatus::Failed;
                task.closed = Some(chrono::Utc::now().to_rfc3339());
            }
            TaskTransition::Opened => {
                // The state projector supplies the full `Task`
                // payload via a separate path (U2). U1's
                // `apply_delta` is invoked only for state changes
                // on already-known tasks; the projector is
                // responsible for inserting new ones before
                // applying the delta.
            }
        }
    }
}

fn apply_counter_change(snap: &mut LedgerSnapshot, counter: &str, new_value: i64) {
    // Saturating cast: counters are non-negative; clamp at i64
    // boundary. Negative values (e.g. from corrupt commits) are
    // clipped to 0 to avoid surprising downstream code.
    let v = if new_value < 0 { 0u64 } else { new_value as u64 };
    match counter {
        "iteration" => snap.iteration = v as u32,
        "hat_handoff_seq" => snap.hat_handoff_seq = v as u32,
        "consecutive_failures" => snap.consecutive_failures = v as u32,
        "consecutive_blocked" => snap.consecutive_blocked = v as u32,
        "abandoned_task_redispatches" => snap.abandoned_task_redispatches = v as u32,
        "consecutive_malformed_events" => snap.consecutive_malformed_events = v as u32,
        "consecutive_hard_gates" => snap.consecutive_hard_gates = v as u32,
        "consecutive_same_signature" => snap.consecutive_same_signature = v as u32,
        "consecutive_no_progress_turns" => snap.consecutive_no_progress_turns = v as u32,
        "consecutive_steward_activations" => snap.consecutive_steward_activations = v as u32,
        "consecutive_completion_rejections" => snap.consecutive_completion_rejections = v as u32,
        "consecutive_engine_gate_rejections" => snap.consecutive_engine_gate_rejections = v as u32,
        "invariant_violation_count" => snap.invariant_violation_count = v as u32,
        "last_rejection_fingerprint" => snap.last_rejection_fingerprint = v,
        "cumulative_cost" => snap.cumulative_cost = new_value as f64,
        _ => {
            // Unknown counter: ignore. The exhaustive match is
            // enforced by the `apply_delta_is_exhaustive` test,
            // but unknown strings (e.g. from a future commit
            // version) are best-effort no-ops so replay does not
            // abort on version skew.
        }
    }
}
