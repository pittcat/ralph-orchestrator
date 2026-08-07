// Cross-submodule imports (previously in same file)
use super::types::{
    DuplicateWorkDoneHint, PolicyDecision, PolicyFinding, ReasonClass, ViolationType,
};
use crate::config::{EventPolicyConfig, PayloadType};
use crate::event_reader::EventReader;
use crate::hat_registry::HatRegistry;
use ralph_proto::{Hat, HatId, Topic};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Runtime state for policy validation across events.
#[derive(Debug, Default, Clone)]
pub struct PolicyRuntimeState {
    pub terminal_observed: bool,
    pub observed_topics: HashSet<String>,
    /// Whether a completion promise has been honored in this loop.
    pub completion_honored: bool,
    /// The topic that triggered the honored completion.
    pub completion_topic: Option<String>,
    /// The event index at which completion was honored.
    pub completion_event_index: Option<u64>,
    /// The iteration at which completion was honored.
    pub completion_iteration: Option<u32>,
    /// The current plan_name extracted from the most recent `work.ready` event.
    /// Used for plan_name equality validation (U4).
    pub current_plan_name: Option<String>,
    /// U4 (2026-06-17-003 plan): dedup set for `work.done` events.
    /// Key format: `{plan_name}::{step}::{task_id}`. Populated when
    /// a `work.done` is accepted by `validate_event_with_hat`;
    /// consumed by the event loop for per-batch pruning. The
    /// per-loop lifetime set lives in `LoopState::work_done_seen_tasks`
    /// (see `event_loop/loop_state.rs`); this set is the
    /// `PolicyRuntimeState` mirror used during `validate_event`
    /// for **in-batch** dedup (when the same `work.done` appears
    /// twice in the same `process_output` batch).
    pub work_done_seen_keys: HashSet<String>,
    /// U-fixes-2026-07-04: canonical `task_id` → `task_key`
    /// binding observed on the first accepted `work.done`.
    /// Used to surface `task_id_task_key_mismatch` BEFORE
    /// dedup so agent retry storms that swap `task_key` on
    /// re-emit get an actionable error (not a generic
    /// "duplicate"). Per-loop lifetime set; pruned on
    /// step boundaries alongside `work_done_seen_tasks`.
    pub work_done_task_id_to_key: HashMap<String, String>,
    /// U5 (2026-06-17-003 plan, R6): dedup set for
    /// `review.dimension.ready` events. Key format:
    /// `{plan_name}::{step}::{task_id}::{dimension}`. Populated
    /// when a `review.dimension.ready` is accepted by
    /// `validate_event_with_hat`; a 2nd emit with the same key
    /// is rejected as `DuplicateWorkDone` (variant reused —
    /// same retry-key semantics, smaller blast radius than
    /// introducing a new ViolationType). Mirrors the
    /// `work.done` dedup pattern: this is the in-batch mirror;
    /// the per-loop lifetime set is also populated in
    /// `from_events` for cross-batch replay.
    pub review_dimension_ready_seen_keys: HashSet<String>,
    /// U5 (2026-06-18-004 plan, R4, KTD3): dedup set for
    /// `review.dimensions.complete` events. Key format:
    /// `{plan_name}::{step}::{task_id}::{fix_round}`. Populated
    /// when a `review.dimensions.complete` is accepted by
    /// `validate_event_with_hat`; a 2nd emit with the same key
    /// is rejected as `DuplicateWorkDone`. The `fix_round`
    /// segment distinguishes re-review rounds so a
    /// `fix.applied`-pruned bucket allows a 2nd
    /// `review.dimensions.complete` to land for a new fix round
    /// without colliding with the 1st round's key. Defaults to
    /// `0` when the payload omits `fix_round` so legacy emitters
    /// still get deduped.
    pub review_dimensions_complete_seen_keys: HashSet<String>,
    /// 2026-06-24 P1-3: dedup map for `work.ready` events. Key
    /// format: `{plan_name}::{step}::{task_id}`. A 2nd
    /// `work.ready` with the same key (same task, same step) is
    /// rejected as `DuplicateWorkDone` so the agent stops
    /// re-announcing an already-started unit. Pruned on
    /// step-boundary events (`fix.applied` / step close) so a
    /// legitimate re-emit after a fix round is allowed.
    ///
    /// U5 of plan 2026-07-05-005 (R8): the value carries the
    /// dedup hit count so post-mortem tooling can distinguish
    /// a single duplicate from a "dup storm" (the same key
    /// re-emitted 50 times in a tight loop). The count is
    /// bumped on every observed hit; `fix.applied` pruning
    /// does NOT reset the counter (count is observation, not
    /// dedup state). Only the work.ready bucket is instrumented
    /// — the other 7 seen_keys fields stay as `HashSet<String>`
    /// to keep the change blast radius small (plan U5 §
    /// "scope-bounded").
    pub work_ready_seen_keys: HashMap<String, u32>,
    /// U5 of plan 2026-07-05-005 (fix-plan §R8): side-table
    /// recording which `work_ready_seen_keys` entries have
    /// had their `(plan_name, step)` bucket pruned. The bucket
    /// classification lives here, separate from the dedup
    /// count in `work_ready_seen_keys`, so a re-emit after
    /// `fix.applied` continues to increment the count without
    /// resetting it.
    pub pruned_work_ready_buckets: HashSet<String>,
    /// 2026-06-24 P1-3: dedup set for `test.passed` events. Key
    /// format: `{plan_name}::{step}::{task_id}::{fix_round}`.
    /// The `fix_round` segment distinguishes re-test rounds so
    /// a `fix.applied`-pruned bucket allows a 2nd `test.passed`
    /// to land for a new fix round without colliding with the
    /// prior round's entry. Missing `fix_round` falls through
    /// (mirrors `review.dimensions.complete` U6 KTD4 behavior)
    /// so the schema validator reports `missing_required_field`
    /// instead of hiding the failure behind `DuplicateWorkDone`.
    pub test_passed_seen_keys: HashSet<String>,
    /// 2026-06-24 P1-3: dedup set for `test.failed` events. Key
    /// format mirrors `test_passed_seen_keys`. Same fall-through
    /// rule for missing/non-numeric `fix_round`.
    pub test_failed_seen_keys: HashSet<String>,
    /// Parallel-forge wave verification deduplication. Key format:
    /// `{plan_key}::{wave_id}::{candidate_commit_sha}`. A wave candidate
    /// can be verified only once; a later wave or a different candidate
    /// remains valid.
    pub forge_wave_verified_seen_keys: HashSet<String>,
    /// 2026-07-01-001 U1: dedup set for `review.start` events.
    /// Key format: `{plan_name}::{task_id}` when `step` is absent,
    /// `{plan_name}::{task_id}::{step}` when present. A 2nd emit
    /// with the same key is rejected as `DuplicateWorkDone` so the
    /// runtime stops a coordinator from starting multiple review
    /// sequences for the same plan/task. Pruned on `fix.applied`
    /// so a legitimate re-review after a fix round is allowed.
    pub review_start_seen_keys: HashSet<String>,
    /// 2026-07-02-004 U7 (R6): pending precheck candidate keys.
    /// Format: `{guarded_topic}::{payload}`. Populated when
    /// `<X>.proposed` is accepted; pruned when the gate emits
    /// `<X>` (pass) or `<X>.rejected` (fail) so a retry after
    /// rejection can re-emit the same payload.
    pub precheck_proposed_pending_keys: HashSet<String>,
    /// U7 of plan 2026-07-02-005: last accepted `plan.blocked.reason`
    /// for shipper strict-match runtime routing on `REVIEW_COMPLETE`.
    pub last_plan_blocked_reason: Option<String>,
}

/// Dedup key for a precheck `<X>.proposed` candidate (U7 / R6).
pub fn precheck_proposed_dedup_key(guarded_topic: &str, payload: &str) -> String {
    format!("{guarded_topic}::{}", payload.trim())
}

/// Build the dedup key for `review.start`.
///
/// U8 of plan 2026-07-02-005: prefer the semantic key
/// `(plan_name, fix_round, total_units)` when the payload
/// carries both. This is the 175407 root-cause fix: the
/// 2nd `review.start` had identical `plan_name + task_id + step`
/// but a different `triggered` value (e.g. `ralph` vs
/// `review-coordinator`); byte equality rejected the 1st
/// emit but the semantic-identity 2nd slipped through. The
/// semantic key is `triggered`-agnostic by construction.
///
/// When `fix_round` / `total_units` are absent from the
/// payload (legacy / pre-fix emits), fall back to the
/// pre-U8 `(plan_name, task_id [, step])` key to preserve
/// backward compatibility.
pub(crate) fn review_start_dedup_key(
    plan_name: &str,
    step: Option<&str>,
    task_id: &str,
    fix_round: Option<u32>,
    total_units: Option<u32>,
) -> String {
    match (fix_round, total_units) {
        (Some(fr), Some(tu)) => format!("{plan_name}::fr={fr}::tu={tu}"),
        _ => {
            if let Some(st) = step {
                format!("{plan_name}::{task_id}::{st}")
            } else {
                format!("{plan_name}::{task_id}")
            }
        }
    }
}

impl PolicyRuntimeState {
    /// U1 (2026-06-18-004 plan, R1, KTD1): prune every
    /// `review_dimension_ready_seen_keys` entry that belongs to a
    /// given `(plan_name, step, task_id)` bucket. Called when
    /// `fix.applied` is policy-accepted so that
    /// `review-coordinator` can legally re-emit
    /// `review.dimension.ready` for the same `(plan, step, task)`
    /// in a new fix round (the original dedup key lacks
    /// `fix_round`, so without this prune a fix → re-review
    /// attempt always gets `DuplicateWorkDone` — this is the
    /// root cause of the perky-maple P1-3 / P2-5 spiral).
    ///
    /// The companion `LoopState::prune_work_done_bucket` (callers
    /// in `event_loop/mod.rs`) handles the per-loop lifetime
    /// mirror; this method only touches the in-batch
    /// `PolicyRuntimeState` mirror. Both must be pruned
    /// together at the `fix.applied` accept site.
    pub fn prune_review_dimension_ready_bucket(
        &mut self,
        plan_name: &str,
        step: &str,
        task_id: &str,
    ) {
        let prefix = format!("{plan_name}::{step}::{task_id}::");
        self.review_dimension_ready_seen_keys
            .retain(|key| !key.starts_with(&prefix));
    }

    /// U1 (2026-06-18-004 plan, KTD1, symmetry fix): mirror of
    /// `LoopState::prune_work_done_bucket` for the
    /// `PolicyRuntimeState::work_done_seen_keys` mirror. Prior
    /// to this addition the in-batch mirror was never pruned on
    /// step-boundary events, leaving a 1-batch stale window
    /// after `queue.advance` / `review.failed` / `fix.applied`
    /// where a re-emit would still be rejected by
    /// `validate_event_with_hat`. Always pair with the
    /// `LoopState::prune_work_done_bucket` call at the accept
    /// site.
    pub fn prune_work_done_bucket(&mut self, plan_name: &str, step: &str) {
        let prefix = format!("{plan_name}::{step}::");
        self.work_done_seen_keys
            .retain(|key| !key.starts_with(&prefix));
        // U-fixes-2026-07-04: step boundary invalidates every
        // (task_id, task_key) binding too — task_ids from a
        // closed step can be re-minted under a new task_key in
        // the next step, so keeping stale bindings would
        // produce false `task_id_task_key_mismatch` rejections.
        self.work_done_task_id_to_key.clear();
    }

    /// U5 (2026-06-18-004 plan, R4): prune every
    /// `review_dimensions_complete_seen_keys` entry that
    /// belongs to a given `(plan_name, step, task_id)` bucket
    /// across ALL `fix_round` values. Called when `fix.applied`
    /// is policy-accepted so that the next round's
    /// `review.dimensions.complete` (carrying `fix_round=N+1`)
    /// lands without colliding with the previous round's
    /// `fix_round=N` entry. The implementation deliberately
    /// does NOT scope the prune to a single `fix_round` —
    /// scoping would require re-doing the dedup key for every
    /// possible round, and the per-task bucket is small
    /// enough (4 dims × at most a handful of rounds) that
    /// over-pruning has no observable blast radius.
    pub fn prune_review_dimensions_complete_bucket(
        &mut self,
        plan_name: &str,
        step: &str,
        task_id: &str,
    ) {
        let prefix = format!("{plan_name}::{step}::{task_id}::");
        self.review_dimensions_complete_seen_keys
            .retain(|key| !key.starts_with(&prefix));
    }

    /// 2026-06-24 P1-3: prune the `work_ready_seen_keys` entries
    /// that belong to a given `(plan_name, step)` bucket. Called
    /// on `fix.applied` / step close so a legitimate re-emit
    /// after a fix round is allowed. Mirrors
    /// `prune_work_done_bucket` (same key shape).
    ///
    /// U5 of plan 2026-07-05-005 (fix-plan §R8 / §C5): the dedup
    /// hit counter is **preserved** across pruning — the count is
    /// observation, not dedup state, so losing it would hide
    /// legitimate dup-storm signals. We achieve this by:
    ///
    /// 1. **Not** removing the pruned entries from
    ///    `work_ready_seen_keys` (the HashMap value carries the
    ///    running count and must survive the prune).
    /// 2. Carrying the bucket classification in a separate
    ///    `pruned_work_ready_buckets: HashSet<String>` side-table
    ///    so the dedup validator can recognise "this key is
    ///    bucket-pruned but the count is still real".
    ///
    /// On the next `work.ready` emit, `validate_event_with_hat`
    /// sees `pruned_work_ready_buckets.contains(&key)` and
    /// increments `work_ready_seen_keys[key]` (no reset to 1).
    pub fn prune_work_ready_bucket(&mut self, plan_name: &str, step: &str) {
        let prefix = format!("{plan_name}::{step}::");
        // Record the bucket as pruned. We intentionally do NOT
        // remove the dedup entries — their counts survive.
        for key in self.work_ready_seen_keys.keys() {
            if key.starts_with(&prefix) {
                self.pruned_work_ready_buckets.insert(key.clone());
            }
        }
    }

    /// 2026-06-24 P1-3: prune the `test_passed_seen_keys` /
    /// `test_failed_seen_keys` entries that belong to a given
    /// `(plan_name, step, task_id)` bucket across ALL
    /// `fix_round` values. Called when `fix.applied` is
    /// policy-accepted so the next round's `test.passed` /
    /// `test.failed` (carrying `fix_round=N+1`) lands without
    /// colliding with the previous round's entry. Mirrors
    /// `prune_review_dimensions_complete_bucket`.
    pub fn prune_test_result_buckets(&mut self, plan_name: &str, step: &str, task_id: &str) {
        let prefix = format!("{plan_name}::{step}::{task_id}::");
        self.test_passed_seen_keys
            .retain(|key| !key.starts_with(&prefix));
        self.test_failed_seen_keys
            .retain(|key| !key.starts_with(&prefix));
    }

    /// 2026-07-02-004 U7: drop pending precheck candidates for
    /// guarded topic `X` after the gate emits `<X>` or
    /// `<X>.rejected`.
    pub fn prune_precheck_proposed_bucket(&mut self, guarded_topic: &str) {
        let prefix = format!("{guarded_topic}::");
        self.precheck_proposed_pending_keys
            .retain(|key| !key.starts_with(&prefix));
    }

    /// 2026-07-01-001 U1: prune every `review_start_seen_keys`
    /// entry that belongs to a given `(plan_name, task_id)` bucket,
    /// including keys that carry an optional `step` suffix. Called
    /// when `fix.applied` is policy-accepted so that a coordinator
    /// can legally start a fresh review round after fixes land.
    pub fn prune_review_start_bucket(&mut self, plan_name: &str, task_id: &str) {
        let base = format!("{plan_name}::{task_id}");
        self.review_start_seen_keys
            .retain(|key| !(key == &base || key.starts_with(&format!("{base}::"))));
    }

    /// Replays events from a JSONL file to build up the policy runtime state.
    ///
    /// Reads all events from the file, tracking which terminal topics have been
    /// observed and which business topics have been seen. Malformed lines are
    /// skipped. String, object, and null payloads are all handled with the same
    /// compatibility semantics as `EventReader`.
    ///
    /// Also extracts `current_plan_name` from the most recent `work.ready` event,
    /// used by the plan_name equality guard (U4).
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened or read.
    pub fn from_events(
        events_path: impl AsRef<std::path::Path>,
        policy: &EventPolicyConfig,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut reader = EventReader::new(events_path.as_ref());
        let result = reader.read_new_events()?;

        let mut state = Self::default();
        for event in result.events {
            state.observed_topics.insert(event.topic.clone());
            if policy.terminal_topics.contains(&event.topic) {
                state.terminal_observed = true;
            }
            // U4: Extract current_plan_name from work.ready events
            if event.topic == "work.ready"
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
                && let Some(name) = obj.get("plan_name").and_then(|v| v.as_str())
            {
                state.current_plan_name = Some(name.to_string());
            }
            // U5 (2026-06-17-003 plan, R6): replay prior
            // `review.dimension.ready` events to populate the
            // dedup set so cross-batch re-emits (e.g. on loop
            // restart or in a new process_output batch) are
            // still rejected. The key shape matches the
            // in-batch check: `{plan_name}::{step}::{task_id}::{dimension}`.
            if event.topic == "review.dimension.ready"
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
            {
                let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
                let step = obj.get("step").and_then(|v| v.as_str());
                let task_id = obj.get("task_id").and_then(|v| v.as_str());
                let dimension = obj.get("dimension").and_then(|v| v.as_str());
                if let (Some(pn), Some(st), Some(ti), Some(dim)) =
                    (plan_name, step, task_id, dimension)
                {
                    state
                        .review_dimension_ready_seen_keys
                        .insert(format!("{pn}::{st}::{ti}::{dim}"));
                }
            }
            // Replay prior forge.wave.verified events so a process restart
            // cannot accept the same wave/candidate twice.
            if event.topic == "forge.wave.verified"
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
            {
                let plan_key = obj.get("plan_key").and_then(|v| v.as_str());
                let wave_id = obj.get("wave_id").and_then(|v| v.as_str());
                let candidate = obj.get("candidate_commit_sha").and_then(|v| v.as_str());
                if let (Some(plan_key), Some(wave_id), Some(candidate)) =
                    (plan_key, wave_id, candidate)
                {
                    state
                        .forge_wave_verified_seen_keys
                        .insert(format!("{plan_key}::{wave_id}::{candidate}"));
                }
            }
            // 2026-07-01-001 U1: replay prior `review.start` events
            // so a loop restart or new `process_output` batch does
            // not accept a duplicate review kick-off for the same
            // `(plan_name, task_id)`.
            if event.topic == "review.start"
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
            {
                let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
                let step = obj.get("step").and_then(|v| v.as_str());
                let task_id = obj.get("task_id").and_then(|v| v.as_str());
                let fix_round = obj
                    .get("fix_round")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as u32);
                let total_units = obj
                    .get("total_units")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as u32);
                if let (Some(pn), Some(ti)) = (plan_name, task_id) {
                    state.review_start_seen_keys.insert(review_start_dedup_key(
                        pn,
                        step,
                        ti,
                        fix_round,
                        total_units,
                    ));
                }
            }
            // U1 (2026-06-18-004 plan, KTD1): replay prior
            // `work.done` events so the in-batch mirror mirrors
            // `LoopState::work_done_seen_tasks`. Without this,
            // the very next `process_output` batch after a
            // loop rehydrate would accept a duplicate `work.done`
            // for the same `(plan, step, task)` because
            // `validate_event_with_hat` only consults
            // `PolicyRuntimeState::work_done_seen_keys`.
            if event.topic == "work.done"
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
            {
                let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
                let step = obj.get("step").and_then(|v| v.as_str());
                let task_id = obj.get("task_id").and_then(|v| v.as_str());
                let task_key = obj.get("task_key").and_then(|v| v.as_str());
                if let (Some(pn), Some(st), Some(ti)) = (plan_name, step, task_id) {
                    state
                        .work_done_seen_keys
                        .insert(format!("{pn}::{st}::{ti}"));
                    // U-fixes-2026-07-04: mirror (task_id) →
                    // task_key binding so rehydrate produces
                    // the same task_id_task_key_mismatch
                    // detection as the live accept path.
                    if let Some(tk) = task_key {
                        state
                            .work_done_task_id_to_key
                            .insert(ti.to_string(), tk.to_string());
                    }
                }
            }
            // 2026-07-02-004 U7: replay precheck gate lifecycle.
            if let Some(guarded) = event.topic.strip_suffix(".rejected") {
                state.prune_precheck_proposed_bucket(guarded);
            } else if event.topic.ends_with(".proposed")
                && let Some(p) = event.payload.as_deref()
            {
                let guarded = event
                    .topic
                    .strip_suffix(".proposed")
                    .unwrap_or(event.topic.as_str());
                state
                    .precheck_proposed_pending_keys
                    .insert(precheck_proposed_dedup_key(guarded, p));
            } else if !event.topic.ends_with(".proposed") {
                state.prune_precheck_proposed_bucket(&event.topic);
            }
            if event.topic == "plan.blocked" {
                state.last_plan_blocked_reason =
                    crate::shipper_reason::extract_plan_blocked_reason(event.payload.as_deref());
            } else if event.topic == "plan.complete" {
                state.last_plan_blocked_reason = None;
            }
            // U1 (2026-06-18-004 plan, KTD1, symmetry fix):
            // when a `fix.applied` is replayed, also prune the
            // `(plan, step, task)` bucket for both
            // `review_dimension_ready_seen_keys` and
            // `work_done_seen_keys` mirrors. This is the
            // `from_events` analog of the live accept-site
            // pruning in `event_loop/mod.rs` — both paths must
            // execute the same prune or loop rehydrate would
            // re-introduce the perky-maple P1-3 dedup block.
            if event.topic == "fix.applied"
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
            {
                let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
                let step = obj.get("step").and_then(|v| v.as_str());
                let task_id = obj.get("task_id").and_then(|v| v.as_str());
                if let (Some(pn), Some(st), Some(ti)) = (plan_name, step, task_id) {
                    state.prune_review_dimension_ready_bucket(pn, st, ti);
                    state.prune_work_done_bucket(pn, st);
                    // U5 (2026-06-18-004 plan, R4):
                    // also prune the
                    // `review.dimensions.complete`
                    // bucket so the next round's
                    // `review.dimensions.complete`
                    // with `fix_round=N+1` lands
                    // without colliding with the
                    // prior round's
                    // `fix_round=N` entry.
                    state.prune_review_dimensions_complete_bucket(pn, st, ti);
                    // 2026-06-24 P1-3: prune the new
                    // `work.ready` / `test.passed` /
                    // `test.failed` buckets so the next
                    // round's emits land without colliding
                    // with the prior round's entries.
                    state.prune_work_ready_bucket(pn, st);
                    state.prune_test_result_buckets(pn, st, ti);
                    // 2026-07-01-001 U1: prune `review.start`
                    // so a coordinator can start a fresh review
                    // sequence after fixes land.
                    state.prune_review_start_bucket(pn, ti);
                }
            }
            // U5 (2026-06-18-004 plan, R4): replay prior
            // `review.dimensions.complete` events so the
            // in-batch mirror reflects the dedup key shape
            // `{plan}::{step}::{task}::{fix_round}`. Missing
            // `fix_round` defaults to `0` so legacy emitters
            // are deduped against the same key the live
            // accept site would record.
            if event.topic == "review.dimensions.complete"
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
            {
                let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
                let step = obj.get("step").and_then(|v| v.as_str());
                let task_id = obj.get("task_id").and_then(|v| v.as_str());
                let fix_round = obj.get("fix_round").and_then(|v| v.as_u64()).unwrap_or(0);
                if let (Some(pn), Some(st), Some(ti)) = (plan_name, step, task_id) {
                    state
                        .review_dimensions_complete_seen_keys
                        .insert(format!("{pn}::{st}::{ti}::{fix_round}"));
                }
            }
            // 2026-06-24 P1-3: replay prior `work.ready` events
            // so the in-batch mirror reflects the dedup key
            // shape `{plan}::{step}::{task_id}`. Without this,
            // the very next `process_output` batch after a loop
            // rehydrate would accept a duplicate `work.ready`
            // for the same `(plan, step, task)`.
            if event.topic == "work.ready"
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
            {
                let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
                let step = obj.get("step").and_then(|v| v.as_str());
                let task_id = obj.get("task_id").and_then(|v| v.as_str());
                if let (Some(pn), Some(st), Some(ti)) = (plan_name, step, task_id) {
                    // U5 of plan 2026-07-05-005 (R8): bump the
                    // per-key counter on every replayed hit so
                    // cross-loop resume keeps the dup-storm
                    // signal consistent with the in-memory view.
                    let key = format!("{pn}::{st}::{ti}");
                    let entry = state.work_ready_seen_keys.entry(key).or_insert(0);
                    *entry = entry.saturating_add(1);
                }
            }
            // 2026-06-24 P1-3: replay prior `test.passed` /
            // `test.failed` events so the in-batch mirror
            // reflects the dedup key shape
            // `{plan}::{step}::{task_id}::{fix_round}`. Missing
            // or non-numeric `fix_round` falls through (mirrors
            // the live accept-site U6 KTD4 rule) so the schema
            // validator reports the precise error on rehydrate.
            if (event.topic == "test.passed" || event.topic == "test.failed")
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
            {
                let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
                let step = obj.get("step").and_then(|v| v.as_str());
                let task_id = obj.get("task_id").and_then(|v| v.as_str());
                let fix_round = match obj.get("fix_round") {
                    Some(Value::Number(n)) => n.as_u64(),
                    _ => None,
                };
                if let (Some(pn), Some(st), Some(ti), Some(fr)) =
                    (plan_name, step, task_id, fix_round)
                {
                    let key = format!("{pn}::{st}::{ti}::{fr}");
                    if event.topic == "test.passed" {
                        state.test_passed_seen_keys.insert(key);
                    } else {
                        state.test_failed_seen_keys.insert(key);
                    }
                }
            }
            // U1 (2026-06-18-004 plan, KTD1, symmetry fix):
            // `queue.advance` and `review.failed` are the other
            // step-boundary events that should clear the
            // work_done mirror on rehydrate (matches the live
            // accept-site behavior).
            if (event.topic == "queue.advance" || event.topic == "review.failed")
                && let Some(obj) = Self::payload_object(event.payload.as_deref())
            {
                let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
                let step = obj
                    .get("completed_step")
                    .or_else(|| obj.get("step"))
                    .and_then(|v| v.as_str());
                if let (Some(pn), Some(st)) = (plan_name, step) {
                    state.prune_work_done_bucket(pn, st);
                    // 2026-06-24 P1-3: mirror the live
                    // accept-site behavior for `work.ready`.
                    state.prune_work_ready_bucket(pn, st);
                }
            }
        }
        Ok(state)
    }

    /// Parse an event payload string into an owned JSON object map.
    ///
    /// Returns `Some(map)` only when the payload is a valid JSON object
    /// (i.e. `{...}`). String payloads, null, arrays, and malformed
    /// JSON all return `None`. The map is owned because
    /// `serde_json::from_str` produces owned `Value`s — we cannot
    /// borrow into the transient `Value` while the caller lives.
    /// 2026-06-18-006 plan U7 (R7, KTD3): collapses six near-identical
    /// payload-parsing blocks in `from_events` into one helper.
    fn payload_object(payload: Option<&str>) -> Option<serde_json::Map<String, Value>> {
        let p = payload?;
        let val = serde_json::from_str::<Value>(p).ok()?;
        if let Value::Object(obj) = val {
            Some(obj)
        } else {
            None
        }
    }
}
