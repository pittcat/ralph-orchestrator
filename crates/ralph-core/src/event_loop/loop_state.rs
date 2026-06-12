//! Loop state tracking for the event loop.
//!
//! This module contains the `LoopState` struct that tracks the current
//! state of the orchestration loop including iteration count, failures,
//! timing, and hat activation tracking.

use ralph_proto::{Event, HatId};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::{Duration, Instant};

/// Maximum number of times the same rejection key may be retried
/// before the runner stops attempting targeted `task.resume` and
/// escalates to a fail-closed terminal reason.  Chosen to match
/// the historical `consecutive_hard_gates` ceiling so operators see
/// a single, consistent retry budget across failure modes.
pub const U2_REJECTION_RETRY_LIMIT: u32 = 3;

/// Fingerprint of the last emitted event for stale loop detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSignature {
    pub topic: String,
    pub source: Option<HatId>,
    pub payload_fingerprint: u64,
}

/// Composite progress marker for the stale-breaker mechanism.
///
/// Captures all forms of meaningful progress: accepted business events,
/// task state changes, workflow advancement, and state machine transitions.
/// Compared between consecutive completion rejections to determine whether
/// real work has occurred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressFingerprint {
    /// Count of distinct business topics accepted (excludes system/diagnostic topics).
    pub accepted_business_count: usize,
    /// Task store snapshot: (open_count, closed_count) at fingerprint time.
    pub task_snapshot: (usize, usize),
    /// Total workflow instances tracked across all chains.
    pub workflow_instances: usize,
    /// Sum of highest phases across all workflow instances.
    pub workflow_phase_sum: usize,
    /// State machine accepted transition count (0 when SM disabled).
    pub sm_transition_count: u32,
}

impl ProgressFingerprint {
    /// Computes a stable u64 hash from this fingerprint for quick comparison.
    pub fn hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.accepted_business_count.hash(&mut hasher);
        self.task_snapshot.hash(&mut hasher);
        self.workflow_instances.hash(&mut hasher);
        self.workflow_phase_sum.hash(&mut hasher);
        self.sm_transition_count.hash(&mut hasher);
        hasher.finish()
    }
}

/// Current state of the event loop.
#[derive(Debug)]
pub struct LoopState {
    /// Current iteration number (1-indexed).
    pub iteration: u32,
    /// Number of consecutive failures.
    pub consecutive_failures: u32,
    /// Cumulative cost in USD (if tracked).
    pub cumulative_cost: f64,
    /// When the loop started.
    pub started_at: Instant,
    /// The last hat that executed.
    pub last_hat: Option<HatId>,
    /// Consecutive blocked events from the same hat.
    pub consecutive_blocked: u32,
    /// Hat that emitted the last blocked event.
    pub last_blocked_hat: Option<HatId>,
    /// Per-task block counts for task-level thrashing detection.
    pub task_block_counts: HashMap<String, u32>,
    /// Tasks that have been abandoned after 3+ blocks.
    pub abandoned_tasks: Vec<String>,
    /// Count of times planner dispatched an already-abandoned task.
    pub abandoned_task_redispatches: u32,
    /// Consecutive malformed JSONL lines encountered (for validation backpressure).
    pub consecutive_malformed_events: u32,
    /// Consecutive hard-gate triggers when agent claims emit but writes no event.
    pub consecutive_hard_gates: u32,
    /// Whether a completion event has been observed in JSONL.
    pub completion_requested: bool,
    /// Whether the completion event has already been honored (prevents duplicate side effects).
    pub completion_honored: bool,

    /// U3 P0 fix (post-review): sticky flag tracking whether a business
    /// event has been accepted in the current isolated-mode turn.
    /// Reset at the start of each new turn (in `process_output`).
    /// Read by `check_default_publishes` so a `default_publishes` injection
    /// cannot co-exist with a JSONL business event in the same turn, and
    /// written by `check_default_publishes` so a later JSONL business event
    /// in the same turn still hits the boundary_violation gate.
    pub isolated_turn_business_event_accepted: bool,

    /// Per-hat activation counts (used for max_activations).
    pub hat_activation_counts: HashMap<HatId, u32>,

    /// Hats for which `<hat_id>.exhausted` has been emitted.
    pub exhausted_hats: HashSet<HatId>,

    /// When the last Telegram check-in message was sent.
    /// `None` means no check-in has been sent yet.
    pub last_checkin_at: Option<Instant>,

    /// Hat IDs that were active in the last iteration.
    /// Used to inject `default_publishes` when agent writes no events.
    pub last_active_hat_ids: Vec<HatId>,

    /// Events that activated `last_active_hat_ids` for the current execution.
    pub last_activation_events: Vec<Event>,

    /// Topics seen during the loop's lifetime (for event chain validation).
    pub seen_topics: HashSet<String>,

    /// The last event signature emitted (for stale loop detection).
    pub last_emitted_signature: Option<EventSignature>,

    /// Per-rejection-key retry counts (2026-06-07 plan U2).  The key is
    /// the stable string returned by `Rejection::compute_retry_key`
    /// (`stage:source_hat:topic:violation_class`).  Increments each
    /// time the runner sees the same key in a fresh batch.  When a
    /// key crosses `U2_REJECTION_RETRY_LIMIT` the next attempt
    /// emits a fail-closed `NonRetryableReason::RetryBudgetExhausted`
    /// so the runner can terminate or escalate.
    pub rejection_retry_counts: HashMap<String, u32>,

    /// Iteration at which each rejection key was last seen, used by
    /// the recovery responder (U6) to de-duplicate recovery envelopes
    /// written for the same key across adjacent iterations.
    pub rejection_last_iteration: HashMap<String, u32>,

    /// Consecutive times the same event signature was emitted (for stale loop detection).
    pub consecutive_same_signature: u32,

    /// Set to true when a loop.cancel event is detected.
    pub cancellation_requested: bool,

    /// The hat currently selected for isolated execution.
    /// Set in isolated mode so `process_parse_result` knows which hat's scope to enforce.
    pub current_isolated_hat: Option<HatId>,

    /// Workflow progress tracking for guarded chains (chain name -> instance key -> phase).
    pub workflow_progress: WorkflowProgress,

    /// Event policy runtime state (opt-in, None when policy is disabled).
    pub policy_runtime_state: Option<crate::event_policy::PolicyRuntimeState>,

    /// State machine runtime state (opt-in, None when state machine is disabled).
    pub state_machine_runtime_state: Option<crate::state_machine::StateMachineRuntimeState>,

    /// Payload of the most recent event whose topic matches the configured
    /// verdict gate topic. Used to enforce that the latest review verdict was
    /// a pass before the loop can terminate. `None` when no such event has
    /// been observed (or no verdict gate is configured).
    pub last_verdict_payload: Option<String>,

    /// Topic of the most recent event whose topic matched the configured
    /// verdict gate. Tracked alongside `last_verdict_payload` so the
    /// fail-path auto-termination check (P0-C, 2026-06-10) can detect
    /// when the verdict has fully propagated to the gate's last
    /// downstream mirror topic (e.g. `report.done` for ce-executor).
    /// `None` when no verdict has been observed.
    pub last_verdict_topic: Option<String>,

    /// Signature of the most recent completion rejection (for stale-breaker).
    pub completion_rejection_signature: Option<String>,

    /// Count of consecutive completion rejections with the same signature.
    pub consecutive_completion_rejections: u32,

    /// Progress fingerprint hash at the time of the last completion rejection.
    /// Used to detect whether real progress has occurred between rejections.
    pub last_rejection_fingerprint: u64,

    /// Count of invariant assertion violations detected (U3).
    /// Incremented each time an invariant check fails.
    pub invariant_violation_count: u32,

    /// The most recent violation rule ID (e.g. "INV-1") for diagnostic display.
    pub last_invariant_violation: Option<String>,

    /// The git HEAD SHA at the moment the loop was started.
    ///
    /// Used by execution contract git-evidence validation to distinguish
    /// "this loop iteration produced new commits" from "the repository
    /// merely has commits from prior history". `None` when the SHA was
    /// not recorded (e.g., the workspace is not a git repository, or the
    /// loop runner could not resolve HEAD at startup).
    pub loop_start_sha: Option<String>,

    /// Per-step review terminal tracker for plan-gate hard enforcement (U1).
    pub review_step_tracker: super::review_step_state::ReviewStepTracker,

    /// WRC-U4 (2026-06-12-003): per-loop handoff deadline tracker.
    /// Records accepted handoff events for unique-consumer topics
    /// and surfaces escalations when a consumer hat fails to
    /// activate within `handoff_dispatch_timeout_seconds`. The
    /// tracker is wired into the main loop's three hook points
    /// (policy-accept, hat-activation, iteration tick) and is
    /// no-op for coordinator mode (the consumer-of-`*-only` path
    /// never runs in coordinator mode because the priority pass
    /// is disabled there). `Default::default()` builds a tracker
    /// with the documented 30s timeout (replaced from
    /// `WorkflowContractConfig` at construction time in
    /// `EventLoop::with_diagnostics`).
    pub handoff_tracker: crate::workflow_contract::HandoffTracker,

    /// Count of consecutive stall_no_events recoveries (U5).
    pub stall_recovery_counts: HashMap<String, u32>,

    /// U3 (2026-06-13-001 plan): hat pinning target for the next
    /// iteration after a hard gate or schema-level wave recovery
    /// fires.  When set, [`EventLoop::next_hat`] overrides the
    /// round-robin / coordinator selection and activates this hat
    /// instead, then clears the value.  The hat stays pinned for
    /// exactly **one** activation so the loop cannot get stuck on
    /// a single hat when the obligation is actually satisfied
    /// (the helper clears the field on activation, and
    /// `process_output` clears it as a belt-and-braces fallback at
    /// every turn boundary).
    ///
    /// Populated by:
    ///   - `inject_missing_event_hard_gate_guidance` (hard gate)
    ///   - `inject_wave_policy_rejection_guidance` (schema-level
    ///     wave recovery)
    pub pending_recovery_hat: Option<HatId>,
}

impl Default for LoopState {
    fn default() -> Self {
        Self {
            iteration: 0,
            consecutive_failures: 0,
            cumulative_cost: 0.0,
            started_at: Instant::now(),
            last_hat: None,
            consecutive_blocked: 0,
            last_blocked_hat: None,
            task_block_counts: HashMap::new(),
            abandoned_tasks: Vec::new(),
            abandoned_task_redispatches: 0,
            consecutive_malformed_events: 0,
            consecutive_hard_gates: 0,
            completion_requested: false,
            completion_honored: false,
            isolated_turn_business_event_accepted: false,
            hat_activation_counts: HashMap::new(),
            exhausted_hats: HashSet::new(),
            last_checkin_at: None,
            last_active_hat_ids: Vec::new(),
            last_activation_events: Vec::new(),
            seen_topics: HashSet::new(),
            last_emitted_signature: None,
            consecutive_same_signature: 0,
            cancellation_requested: false,
            current_isolated_hat: None,
            workflow_progress: WorkflowProgress::new(),
            policy_runtime_state: None,
            state_machine_runtime_state: None,
            last_verdict_payload: None,
            last_verdict_topic: None,
            completion_rejection_signature: None,
            consecutive_completion_rejections: 0,
            last_rejection_fingerprint: 0,
            loop_start_sha: None,
            rejection_retry_counts: HashMap::new(),
            rejection_last_iteration: HashMap::new(),
            invariant_violation_count: 0,
            last_invariant_violation: None,
            review_step_tracker: super::review_step_state::ReviewStepTracker::default(),
            // WRC-U4: default 30s timeout; the runtime replaces
            // this from `WorkflowContractConfig` in
            // `EventLoop::with_diagnostics` so the per-loop config
            // value (clamped to 120s) takes effect.
            handoff_tracker: crate::workflow_contract::HandoffTracker::new(),
            stall_recovery_counts: HashMap::new(),
            pending_recovery_hat: None,
        }
    }
}

/// Progress tracking for a single workflow instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowInstanceProgress {
    /// The chain this instance belongs to.
    pub chain_name: String,
    /// The instance key (e.g., experiment_id) or None for global instances.
    pub instance_key: Option<String>,
    /// The highest phase index reached (0-indexed into the chain's topics).
    pub highest_phase: usize,
}

/// Tracks workflow progress for guarded chains.
///
/// Maps chain_name -> instance_key -> WorkflowInstanceProgress.
/// When a chain has no correlation key, instance_key is None and a single
/// global instance is tracked.
#[derive(Debug, Default)]
pub struct WorkflowProgress {
    /// Per-chain, per-instance progress. The outer HashMap key is chain_name.
    instances: HashMap<String, HashMap<Option<String>, WorkflowInstanceProgress>>,
}

impl WorkflowProgress {
    /// Creates a new empty workflow progress tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the highest phase reached for a given chain and instance.
    pub fn get_phase(&self, chain_name: &str, instance_key: Option<&str>) -> Option<usize> {
        self.instances
            .get(chain_name)?
            .get(&instance_key.map(String::from))
            .map(|p| p.highest_phase)
    }

    /// Returns a reference to the progress for a specific chain/instance.
    pub fn get(
        &self,
        chain_name: &str,
        instance_key: Option<&str>,
    ) -> Option<&WorkflowInstanceProgress> {
        self.instances
            .get(chain_name)?
            .get(&instance_key.map(String::from))
    }

    /// Returns the next valid phase index for a given chain.
    ///
    /// Returns 0 if no progress exists yet. Otherwise returns `highest_phase + 1`.
    pub fn next_phase(&self, chain_name: &str, instance_key: Option<&str>) -> usize {
        self.get_phase(chain_name, instance_key)
            .map(|p| p + 1)
            .unwrap_or(0)
    }

    /// Returns true if the given phase is the next valid one to advance to.
    ///
    /// A phase is valid for advancement if:
    /// - No progress exists yet and phase is 0 (chain start)
    /// - phase equals current highest_phase + 1 (sequential advancement)
    /// - phase equals current highest_phase (idempotent re-emission of same phase)
    pub fn is_phase_valid(
        &self,
        chain_name: &str,
        instance_key: Option<&str>,
        phase: usize,
    ) -> bool {
        let current_highest = self.get_phase(chain_name, instance_key);
        match current_highest {
            None => phase == 0,
            Some(highest) => phase == highest || phase == highest + 1,
        }
    }

    /// Records advancement to a new phase for a chain instance.
    ///
    /// If the given phase is not valid (skipping ahead), this is a no-op.
    /// If the phase is <= current highest, this is idempotent (no update).
    pub fn advance(&mut self, chain_name: &str, instance_key: Option<&str>, phase: usize) {
        if !self.is_phase_valid(chain_name, instance_key, phase) {
            return;
        }

        let current_highest = self.get_phase(chain_name, instance_key);
        if current_highest.is_some_and(|h| phase <= h) {
            // Idempotent: already at or past this phase
            return;
        }

        let instances = self.instances.entry(chain_name.to_string()).or_default();
        let progress = WorkflowInstanceProgress {
            chain_name: chain_name.to_string(),
            instance_key: instance_key.map(String::from),
            highest_phase: phase,
        };
        instances.insert(instance_key.map(String::from), progress);
    }

    /// Returns the total number of tracked instances across all chains.
    pub fn instance_count(&self) -> usize {
        self.instances.values().map(|m| m.len()).sum()
    }

    /// Returns the sum of highest phases across all tracked instances.
    ///
    /// Used as part of the progress fingerprint to detect workflow advancement.
    /// A phase advancement increases this sum, indicating real progress.
    pub fn phase_sum(&self) -> usize {
        self.instances
            .values()
            .flat_map(|m| m.values())
            .map(|p| p.highest_phase)
            .sum()
    }

    /// Returns all tracked instance keys for a given chain.
    pub fn instance_keys(&self, chain_name: &str) -> Vec<Option<String>> {
        self.instances
            .get(chain_name)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }
}

impl LoopState {
    /// Creates a new loop state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the elapsed time since the loop started.
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Increment the per-rejection-key retry counter and return the
    /// post-increment value.  When the result exceeds
    /// [`U2_REJECTION_RETRY_LIMIT`] the caller must mark the rejection
    /// as fail-closed (R2: bounded retry to prevent infinite loops).
    pub fn record_rejection_key(&mut self, key: &str) -> u32 {
        let entry = self
            .rejection_retry_counts
            .entry(key.to_string())
            .or_insert(0);
        *entry = entry.saturating_add(1);
        *entry
    }

    /// Current count of retries observed for a given rejection key.
    /// Returns 0 when the key has never been recorded.
    pub fn rejection_retry_count(&self, key: &str) -> u32 {
        self.rejection_retry_counts.get(key).copied().unwrap_or(0)
    }

    /// Returns `true` if a rejection key has crossed the bounded-retry
    /// threshold.  Used by the runner to decide between
    /// `task.resume` (retryable) and `TerminationReason::Recovered`
    /// (fail-closed escalation).
    ///
    /// Semantics: the budget allows `U2_REJECTION_RETRY_LIMIT` retries.
    /// When the *post-increment* count strictly **exceeds** the limit
    /// the budget is exhausted and the next attempt must terminate.
    /// This means counts 1..=LIMIT all permit a `task.resume`; the
    /// (LIMIT+1)-th attempt is the first one marked exhausted.  The
    /// `>` comparison is intentional — `>=` would silently drop the
    /// last retry attempt.
    pub fn rejection_key_is_exhausted(&self, key: &str) -> bool {
        self.rejection_retry_count(key) > U2_REJECTION_RETRY_LIMIT
    }

    fn event_counts_toward_stale_loop(event: &Event) -> bool {
        !matches!(event.topic.as_str(), "task.complete")
    }

    /// Record that an event has been seen during this loop run.
    ///
    /// Also tracks consecutive same-signature emissions for stale loop detection.
    pub fn record_event(&mut self, event: &Event) {
        self.seen_topics.insert(event.topic.to_string());

        if !Self::event_counts_toward_stale_loop(event) {
            self.consecutive_same_signature = 0;
            self.last_emitted_signature = Some(EventSignature::from_event(event));
            return;
        }

        let signature = EventSignature::from_event(event);
        if self.last_emitted_signature.as_ref() == Some(&signature) {
            self.consecutive_same_signature += 1;
        } else {
            self.consecutive_same_signature = 1;
            self.last_emitted_signature = Some(signature);
        }
    }

    /// Check if all required topics have been seen.
    pub fn missing_required_events<'a>(&self, required: &'a [String]) -> Vec<&'a String> {
        required
            .iter()
            .filter(|topic| !self.seen_topics.contains(topic.as_str()))
            .collect()
    }

    /// Records the payload of an event if its topic matches the configured verdict gate.
    ///
    /// Called alongside `record_event` at every site. The most recent matching
    /// event's payload is retained so `check_completion_event` can read the
    /// verdict without re-scanning event history. No-op when `verdict_topics`
    /// is `None` / empty or the event topic does not match any entry.
    ///
    /// 2026-06-09 fix: now accepts a slice of topics so a single
    /// gate can cover both the upstream verdict topic (e.g.
    /// `REVIEW_COMPLETE`) and downstream events that mirror the
    /// verdict payload (e.g. `report.done`).  When ANY of the
    /// listed topics carries a failing verdict, the gate fires.
    pub fn record_verdict_if_match(&mut self, event: &Event, verdict_topics: Option<&[String]>) {
        let Some(topics) = verdict_topics else {
            return;
        };
        if topics.is_empty() {
            return;
        }
        if topics.iter().any(|t| t == event.topic.as_str()) {
            // 2026-06-10 P0-C fix: track the topic alongside the payload
            // so the fail-path auto-termination check can detect when
            // the verdict has propagated to the LAST configured mirror
            // topic (e.g. `report.done`).
            self.last_verdict_topic = Some(event.topic.to_string());
            self.last_verdict_payload = Some(event.payload.clone());
        }
    }

    /// Computes a composite progress fingerprint capturing all meaningful progress signals.
    ///
    /// The fingerprint includes:
    /// - Count of accepted business topics (excludes system/diagnostic topics)
    /// - Task store snapshot (open/closed counts)
    /// - Workflow instance count and phase sum
    /// - State machine transition count
    ///
    /// This replaces the naive `seen_topics.len()` check which could be fooled by
    /// irrelevant topics (e.g., `event.malformed`, `human.guidance`).
    pub fn compute_progress_fingerprint(&self) -> ProgressFingerprint {
        // Count only business topics (exclude system/diagnostic/recovery topics)
        let accepted_business_count = self
            .seen_topics
            .iter()
            .filter(|t| !Self::is_system_topic(t))
            .count();

        // Workflow progress: instance count and sum of all highest phases
        let workflow_instances = self.workflow_progress.instance_count();
        let workflow_phase_sum = self.workflow_progress.phase_sum();

        // SM transition count (0 when disabled)
        let sm_transition_count = self
            .state_machine_runtime_state
            .as_ref()
            .map(|sm| sm.accepted_transition_count())
            .unwrap_or(0);

        ProgressFingerprint {
            accepted_business_count,
            task_snapshot: (0, 0), // Caller must fill in task counts
            workflow_instances,
            workflow_phase_sum,
            sm_transition_count,
        }
    }

    /// Returns true if the given topic is a system/diagnostic topic that should
    /// not count as business progress.
    fn is_system_topic(topic: &str) -> bool {
        matches!(
            topic,
            "task.resume"
                | "task.start"
                | "event.malformed"
                | "event.scope_violation"
                | "event.workflow_guard_rejected"
                | "event.state_machine.rejected"
                | "event.state_machine.ignored"
                | "event.state_machine.diagnostic"
                | "event.policy_warning"
                | "event.completion.blocked"
                | "event.completion.ignored"
                | "event.isolation.boundary_violation"
                | "human.interact"
                | "human.response"
                | "human.guidance"
                | "human.timeout"
                | "loop.cancel"
                | "build.task.abandoned"
        ) || topic.ends_with(".exhausted")
            || topic.ends_with(".scope_violation")
    }
}

impl EventSignature {
    pub fn from_event(event: &Event) -> Self {
        Self {
            topic: event.topic.to_string(),
            source: event.source.clone(),
            payload_fingerprint: fingerprint_payload(&event.payload),
        }
    }
}

fn fingerprint_payload(payload: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    payload.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::{LoopState, U2_REJECTION_RETRY_LIMIT, WorkflowProgress};
    use ralph_proto::Event;

    #[test]
    fn repeated_task_complete_does_not_accumulate_stale_loop_count() {
        let mut state = LoopState::new();

        state.record_event(&Event::new("task.complete", "task 1 complete"));
        assert_eq!(state.consecutive_same_signature, 0);

        state.record_event(&Event::new("task.complete", "task 2 complete"));
        state.record_event(&Event::new("task.complete", "task 3 complete"));

        assert_eq!(state.consecutive_same_signature, 0);
        assert_eq!(
            state
                .last_emitted_signature
                .as_ref()
                .map(|s| s.topic.as_str()),
            Some("task.complete")
        );
    }

    #[test]
    fn repeated_non_progress_topics_still_accumulate_stale_loop_count() {
        let mut state = LoopState::new();

        state.record_event(&Event::new("task.resume", "same payload"));
        state.record_event(&Event::new("task.resume", "same payload"));
        state.record_event(&Event::new("task.resume", "same payload"));

        assert_eq!(state.consecutive_same_signature, 3);
        assert_eq!(
            state
                .last_emitted_signature
                .as_ref()
                .map(|s| s.topic.as_str()),
            Some("task.resume")
        );
    }

    // -------------------------------------------------------------------------
    // WorkflowProgress tests
    // -------------------------------------------------------------------------

    #[test]
    fn workflow_progress_single_instance_sequential_phases() {
        // Test: one experiment progresses through all configured topics in order.
        let mut progress = WorkflowProgress::new();
        let chain = "experiment";
        let instance: Option<&str> = None; // global instance

        // Phase 0: experiment.planned
        progress.advance(chain, instance, 0);
        assert_eq!(progress.get_phase(chain, instance), Some(0));

        // Phase 1: experiment.ready
        progress.advance(chain, instance, 1);
        assert_eq!(progress.get_phase(chain, instance), Some(1));

        // Phase 2: experiment.measured
        progress.advance(chain, instance, 2);
        assert_eq!(progress.get_phase(chain, instance), Some(2));

        // Phase 3: experiment.scored
        progress.advance(chain, instance, 3);
        assert_eq!(progress.get_phase(chain, instance), Some(3));

        // Phase 4: experiment.evaluated
        progress.advance(chain, instance, 4);
        assert_eq!(progress.get_phase(chain, instance), Some(4));

        assert_eq!(progress.instance_count(), 1);
    }

    #[test]
    fn workflow_progress_two_instances_independent() {
        // Test: two experiment IDs progress independently.
        let mut progress = WorkflowProgress::new();
        let chain = "experiment";

        // Experiment 1: scored (phase 3)
        progress.advance(chain, Some("exp-1"), 0);
        progress.advance(chain, Some("exp-1"), 1);
        progress.advance(chain, Some("exp-1"), 2);
        progress.advance(chain, Some("exp-1"), 3);

        // Experiment 2: only at measured (phase 2)
        progress.advance(chain, Some("exp-2"), 0);
        progress.advance(chain, Some("exp-2"), 1);
        progress.advance(chain, Some("exp-2"), 2);

        assert_eq!(progress.get_phase(chain, Some("exp-1")), Some(3));
        assert_eq!(progress.get_phase(chain, Some("exp-2")), Some(2));

        // exp-1's scored should NOT advance exp-2
        progress.advance(chain, Some("exp-2"), 3); // This should work since exp-2 is at phase 2, and 3 == 2+1
        assert_eq!(progress.get_phase(chain, Some("exp-2")), Some(3));
    }

    #[test]
    fn workflow_progress_instance_isolation() {
        // Test: experiment.scored for experiment 1 does not allow
        // experiment.evaluated for experiment 2 (instance isolation).
        let mut progress = WorkflowProgress::new();
        let chain = "experiment";

        // Experiment 1 is at scored (phase 3)
        progress.advance(chain, Some("exp-1"), 0);
        progress.advance(chain, Some("exp-1"), 1);
        progress.advance(chain, Some("exp-1"), 2);
        progress.advance(chain, Some("exp-1"), 3);
        assert_eq!(progress.get_phase(chain, Some("exp-1")), Some(3));

        // Experiment 2 is only at measured (phase 2) - cannot skip to evaluated (phase 4)
        progress.advance(chain, Some("exp-2"), 0);
        progress.advance(chain, Some("exp-2"), 1);
        progress.advance(chain, Some("exp-2"), 2);
        assert_eq!(progress.get_phase(chain, Some("exp-2")), Some(2));

        // Attempt to advance exp-2 to evaluated (phase 4) should be rejected
        // because current highest is 2, and 4 > 2 + 1
        progress.advance(chain, Some("exp-2"), 4);
        assert_eq!(
            progress.get_phase(chain, Some("exp-2")),
            Some(2),
            "exp-2 should remain at phase 2 - cannot skip to evaluated"
        );

        // But exp-2 can advance to scored (phase 3)
        progress.advance(chain, Some("exp-2"), 3);
        assert_eq!(progress.get_phase(chain, Some("exp-2")), Some(3));

        // Now exp-2 can advance to evaluated (phase 4)
        progress.advance(chain, Some("exp-2"), 4);
        assert_eq!(progress.get_phase(chain, Some("exp-2")), Some(4));
    }

    #[test]
    fn workflow_progress_idempotent_same_phase() {
        // Test: repeated same-phase event is handled idempotently.
        let mut progress = WorkflowProgress::new();
        let chain = "experiment";
        let instance = Some("exp-1");

        // Phase 0
        progress.advance(chain, instance, 0);
        assert_eq!(progress.get_phase(chain, instance), Some(0));

        // Re-emit same phase 0 - should be idempotent (no change)
        progress.advance(chain, instance, 0);
        assert_eq!(progress.get_phase(chain, instance), Some(0));

        // Advance to phase 1
        progress.advance(chain, instance, 1);
        assert_eq!(progress.get_phase(chain, instance), Some(1));

        // Re-emit phase 0 again - should still be idempotent
        progress.advance(chain, instance, 0);
        assert_eq!(progress.get_phase(chain, instance), Some(1));

        // Re-emit phase 1 - should be idempotent
        progress.advance(chain, instance, 1);
        assert_eq!(progress.get_phase(chain, instance), Some(1));
    }

    #[test]
    fn workflow_progress_global_vs_per_instance() {
        // Test: chains with no correlation key share a global instance (None key).
        let mut progress = WorkflowProgress::new();
        let chain = "experiment";

        // Global instance advances
        progress.advance(chain, None, 0);
        progress.advance(chain, None, 1);
        assert_eq!(progress.get_phase(chain, None), Some(1));

        // Per-instance tracking is independent
        progress.advance(chain, Some("exp-1"), 0);
        assert_eq!(progress.get_phase(chain, Some("exp-1")), Some(0));
        assert_eq!(progress.get_phase(chain, None), Some(1));

        // Global and per-instance are separate entries
        assert_eq!(progress.instance_count(), 2);
        let global_keys = progress.instance_keys(chain);
        assert!(global_keys.contains(&None));
        assert!(global_keys.contains(&Some("exp-1".to_string())));
    }

    #[test]
    fn workflow_progress_is_phase_valid() {
        let mut progress = WorkflowProgress::new();
        let chain = "experiment";
        let instance = Some("exp-1");

        // No progress yet: only phase 0 is valid
        assert!(progress.is_phase_valid(chain, instance, 0));
        assert!(!progress.is_phase_valid(chain, instance, 1)); // skipping
        assert!(!progress.is_phase_valid(chain, instance, 4)); // way ahead

        // At phase 2: can accept 2 (idempotent re-emit), 3 (next)
        progress.advance(chain, instance, 0);
        progress.advance(chain, instance, 1);
        progress.advance(chain, instance, 2);
        assert_eq!(progress.get_phase(chain, instance), Some(2));

        assert!(!progress.is_phase_valid(chain, instance, 0)); // old phase — no longer accepted
        assert!(!progress.is_phase_valid(chain, instance, 1)); // old phase — no longer accepted
        assert!(progress.is_phase_valid(chain, instance, 2)); // idempotent re-emit
        assert!(progress.is_phase_valid(chain, instance, 3)); // next
        assert!(!progress.is_phase_valid(chain, instance, 4)); // skip
    }

    #[test]
    fn u2_rejection_retry_counter_increments_and_saturates() {
        // 2026-06-07 plan U2: per-key retry counter must increment
        // monotonically and surface exhaustion at the configured limit.
        // Semantics: budget allows U2_REJECTION_RETRY_LIMIT retries
        // (counts 1..=LIMIT all permit a task.resume); the (LIMIT+1)-th
        // attempt is the first one marked exhausted.
        let mut state = LoopState::new();
        let key = "execution_contract:executor:work.done:missing_field";

        assert_eq!(state.rejection_retry_count(key), 0);
        assert!(!state.rejection_key_is_exhausted(key));

        // First LIMIT attempts must all be marked *not* exhausted so the
        // runner actually issues task.resume for each.
        for i in 1..=U2_REJECTION_RETRY_LIMIT {
            let n = state.record_rejection_key(key);
            assert_eq!(n, i, "post-increment count must match attempt {i}");
            assert!(
                !state.rejection_key_is_exhausted(key),
                "attempt {i} (count={i}) must NOT be exhausted; \
                 the budget is `LIMIT` retries, not `LIMIT-1`"
            );
        }

        // The (LIMIT+1)-th attempt is the first exhausted one.
        let n = state.record_rejection_key(key);
        assert_eq!(n, U2_REJECTION_RETRY_LIMIT + 1);
        assert!(
            state.rejection_key_is_exhausted(key),
            "attempt {} (count={}) MUST be exhausted",
            U2_REJECTION_RETRY_LIMIT + 1,
            U2_REJECTION_RETRY_LIMIT + 1
        );

        // Further attempts are still allowed to increment (saturating) —
        // the runner is the one that reads `is_exhausted` and stops
        // publishing `task.resume`.  We must not panic on overflow.
        let n = state.record_rejection_key(key);
        assert_eq!(n, U2_REJECTION_RETRY_LIMIT + 2);
        assert!(state.rejection_key_is_exhausted(key));
    }

    #[test]
    fn u2_rejection_retry_keys_are_independent() {
        // Two distinct rejection keys must not share counts.
        let mut state = LoopState::new();
        let k1 = "execution_contract:executor:work.done:missing_field";
        let k2 = "execution_contract:executor:work.done:type_mismatch";
        state.record_rejection_key(k1);
        state.record_rejection_key(k1);
        state.record_rejection_key(k2);
        assert_eq!(state.rejection_retry_count(k1), 2);
        assert_eq!(state.rejection_retry_count(k2), 1);
        assert!(!state.rejection_key_is_exhausted(k1));
        assert!(!state.rejection_key_is_exhausted(k2));
    }

    #[test]
    fn u2_rejection_retry_counter_starts_at_zero_for_unknown_key() {
        let state = LoopState::new();
        assert_eq!(state.rejection_retry_count("nonexistent"), 0);
        assert!(!state.rejection_key_is_exhausted("nonexistent"));
    }
}
