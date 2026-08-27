//! Wave tracking state machine for concurrent hat execution.
//!
//! Tracks active waves, records results and failures, and determines
//! when all workers have reported back.

use ralph_proto::{Event, HatId};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Central state machine for tracking active waves.
#[derive(Debug, Default)]
pub struct WaveTracker {
    active_waves: HashMap<String, WaveState>,
}

/// State of a single active wave.
#[derive(Debug)]
pub(crate) struct WaveState {
    wave_id: String,
    expected_total: u32,
    results: Vec<WaveResult>,
    failures: Vec<WaveFailure>,
    started_at: Instant,
    /// Hat the dispatcher expects each worker's `event.source` to
    /// carry. `None` skips the merge-layer check.
    expected_source_hat: Option<HatId>,
    /// 2026-06-17-002 U5/R5: per-worker retry counts. Each worker
    /// index may be retried at most `MAX_DIMENSION_RETRIES_PER_SLOT`
    /// times across the lifetime of the wave (initial attempt + N
    /// retries). Persisted in the tracker so a permanently-mismatched
    /// worker cannot drain an unbounded number of dispatches — once
    /// the per-slot budget is exhausted the merge layer's
    /// `wave.worker.failed(reason=dimension_mismatch)` record is the
    /// terminal signal for the synthesizer. See
    /// `take_retry_quota` / `consume_retry_quota`.
    dimension_retry_counts: std::collections::HashMap<u32, u32>,
}

/// 2026-06-17-002 U5/R5: per-slot retry cap. The plan explicitly
/// requires that "after two attempts (initial + one retry) the slot
/// is treated as missing and the wave may proceed to
/// incomplete_wave_gate". Persisted in the WaveTracker so a
/// permanent mismatch across dispatches does not slip past the
/// cap.
pub const MAX_DIMENSION_RETRIES_PER_SLOT: u32 = 1;

/// A successful result from a wave instance.
#[derive(Debug, Clone)]
pub struct WaveResult {
    pub index: u32,
    pub events: Vec<Event>,
}

/// A failure from a wave instance.
#[derive(Debug, Clone)]
pub struct WaveFailure {
    pub index: u32,
    pub error: String,
    pub duration: Duration,
    /// Optional: when this failure is a dimension mismatch (R4 of
    /// 2026-06-17-002), the expected (assigned) and actual
    /// (worker-emitted) dimensions. `Some` only on the synthetic
    /// `wave.worker.failed(reason=dimension_mismatch|dimension_missing)`
    /// records the merge layer writes when the worker's emitted
    /// `review.dimension.done` does not match its assigned slot.
    /// `None` for legacy / non-review waves and for plain
    /// worker-error failures (timeout, crash, etc).
    pub expected_dimension: Option<String>,
    pub actual_dimension: Option<String>,
}

impl Default for WaveFailure {
    fn default() -> Self {
        Self {
            index: 0,
            error: String::new(),
            duration: Duration::ZERO,
            expected_dimension: None,
            actual_dimension: None,
        }
    }
}

impl WaveFailure {
    /// Build a `WaveFailure` whose `error` string is dimension_mismatch
    /// (R4 of 2026-06-17-002). The merge layer stamps these into
    /// `wave.worker.failed` records so the synthesizer's
    /// `WaveContext.missing_dimensions` covers both "never reported"
    /// and "reported wrong dimension" slots.
    pub fn dimension_mismatch(
        index: u32,
        expected: String,
        actual: String,
        duration: Duration,
    ) -> Self {
        Self {
            index,
            error: format!("dimension_mismatch: expected={expected} actual={actual}"),
            duration,
            expected_dimension: Some(expected),
            actual_dimension: Some(actual),
        }
    }

    /// Build a `WaveFailure` whose `error` string is dimension_missing
    /// (R4 timeout path of 2026-06-17-002). The dispatcher's
    /// synthetic-failure path stamps these into `wave.worker.failed`
    /// records when a worker never reported for a slot that carried
    /// a dimension assignment, so the synthesizer's
    /// `WaveContext.missing_dimensions` covers both "never reported"
    /// and "reported wrong dimension" slots.
    pub fn dimension_missing(index: u32, expected: String, duration: Duration) -> Self {
        Self {
            index,
            error: format!("dimension_missing: expected={expected}"),
            duration,
            expected_dimension: Some(expected),
            actual_dimension: None,
        }
    }
}

/// A completed wave with all results and failures.
#[derive(Debug, Clone)]
pub struct CompletedWave {
    pub wave_id: String,
    /// Total number of workers the dispatcher expected (R8: every
    /// merged record must carry this so the aggregator can tell a
    /// partial wave from a full one without re-reading the registry).
    pub wave_total: u32,
    pub results: Vec<WaveResult>,
    pub failures: Vec<WaveFailure>,
    pub duration: Duration,
    /// Whether this wave completed with fewer results than expected.
    /// When true, the aggregator should note that some workers did not
    /// report back and list missing dimensions in Coverage.
    pub partial: bool,
    /// The hat the dispatcher promised the per-worker env
    /// (`inject_hat_execution_env` sets `target_hat` on each
    /// worker). The merge layer uses this to reject worker-written
    /// `event.source` that does not match, defending against
    /// ADV-2 (hat-spoofing via per-worker JSONL). `None` for
    /// waves dispatched before the field was added (legacy
    /// records); the merge layer treats `None` as
    /// "unverified" and falls back to `default_source_hat` (so
    /// the fix is non-breaking for old fixtures).
    pub expected_source_hat: Option<HatId>,
    /// Per-worker dimension assignment produced by the dispatcher
    /// from each wave event's `dimension` payload field (R1 of
    /// 2026-06-17-002). Carried on the `CompletedWave` so the
    /// merge layer can drop mismatched `review.dimension.done`
    /// events without the dispatcher having to thread the map
    /// through every call site (R4 of 2026-06-17-002). `None` /
    /// empty map means "no assignment" — the merge layer skips the
    /// dimension check entirely so legacy / non-review waves pass
    /// through unchanged.
    pub assigned_dimensions: std::collections::HashMap<u32, String>,
    /// 2026-06-17-002 U5/R5: per-slot dimension-mismatch retry
    /// counts. The tracker increments this map in
    /// `try_consume_dimension_retry`; when the wave is
    /// `take_wave_results`-ed the counts transfer to the
    /// `CompletedWave` so the caller (dispatcher) can decide
    /// whether to inject a `task.resume` based on a quota that
    /// survives across `handle_wave_events` invocations. The
    /// previous process-local HashMap in the dispatcher was
    /// reset on every dispatch round, allowing a permanent
    /// mismatch to loop indefinitely.
    pub dimension_retry_counts: std::collections::HashMap<u32, u32>,
    /// 2026-07-03-001 supervisor real-wiring: per-wave worker
    /// events drained from the merge sink. The supervisor
    /// path's `run_supervisor_fan_in` reads this field (instead
    /// of re-reading the per-worker JSONL files) to merge worker
    /// events into the main events file when the coordinator
    /// returns `InjectedComplete`. The legacy `WaveTracker`
    /// path leaves this empty — its merge function reads
    /// `result.events` directly, so the field is a no-op for
    /// non-supervisor dispatch.
    pub worker_events: Vec<crate::Event>,
}

impl Default for CompletedWave {
    fn default() -> Self {
        Self {
            wave_id: String::new(),
            wave_total: 0,
            results: Vec::new(),
            failures: Vec::new(),
            duration: Duration::ZERO,
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: std::collections::HashMap::new(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        }
    }
}

/// Progress indicator returned by `record_result`.
#[derive(Debug, PartialEq, Eq)]
pub enum WaveProgress {
    /// More results expected.
    InProgress { received: u32, expected: u32 },
    /// All results received, wave complete.
    Complete,
}

impl WaveState {
    /// Returns the current progress of this wave.
    fn progress(&self) -> WaveProgress {
        let received = self.results.len() as u32 + self.failures.len() as u32;
        if received >= self.expected_total {
            WaveProgress::Complete
        } else {
            WaveProgress::InProgress {
                received,
                expected: self.expected_total,
            }
        }
    }

    /// Returns true if the given worker index has already submitted a result or failure.
    fn has_index(&self, index: u32) -> bool {
        self.results.iter().any(|r| r.index == index)
            || self.failures.iter().any(|f| f.index == index)
    }
}

impl WaveTracker {
    /// Creates a new empty wave tracker.
    pub fn new() -> Self {
        Self {
            active_waves: HashMap::new(),
        }
    }

    /// Register a new wave with the dispatcher-expected source hat.
    /// The merge layer uses `expected_source_hat` to reject
    /// worker-written `event.source` that does not match,
    /// defending against ADV-2 (hat-spoofing). `None` skips
    /// the check (used by waves whose workers cannot be
    /// attributed, e.g. smoke fixtures).
    pub fn register_wave_with_source(
        &mut self,
        wave_id: String,
        expected_total: u32,
        expected_source_hat: Option<HatId>,
    ) {
        if self.active_waves.contains_key(&wave_id) {
            tracing::warn!(wave_id, "Overwriting existing active wave state");
        }
        let state = WaveState {
            wave_id: wave_id.clone(),
            expected_total,
            results: Vec::new(),
            failures: Vec::new(),
            started_at: Instant::now(),
            expected_source_hat,
            dimension_retry_counts: std::collections::HashMap::new(),
        };
        self.active_waves.insert(wave_id, state);
    }

    /// Register a new wave (back-compat: skips source-hat check).
    ///
    /// Warns and overwrites if a wave with the same ID is already active.
    pub fn register_wave(&mut self, wave_id: String, expected_total: u32) {
        self.register_wave_with_source(wave_id, expected_total, None);
    }

    /// Record result events for a wave instance.
    /// Returns the wave progress after recording.
    pub fn record_result(&mut self, wave_id: &str, index: u32, events: Vec<Event>) -> WaveProgress {
        let Some(state) = self.active_waves.get_mut(wave_id) else {
            tracing::warn!(wave_id, index, "Received result for unknown wave, ignoring");
            return WaveProgress::InProgress {
                received: 0,
                expected: 0,
            };
        };
        if state.has_index(index) {
            tracing::warn!(wave_id, index, "Duplicate worker index, ignoring");
            return state.progress();
        }
        state.results.push(WaveResult { index, events });
        state.progress()
    }

    /// Record a failure for a wave instance.
    /// Returns the wave progress after recording.
    pub fn record_failure(
        &mut self,
        wave_id: &str,
        index: u32,
        error: String,
        duration: Duration,
    ) -> WaveProgress {
        let Some(state) = self.active_waves.get_mut(wave_id) else {
            tracing::warn!(
                wave_id,
                index,
                "Failure recorded for unknown wave, ignoring"
            );
            return WaveProgress::InProgress {
                received: 0,
                expected: 0,
            };
        };
        if state.has_index(index) {
            tracing::warn!(
                wave_id,
                index,
                "Duplicate worker index in failure, ignoring"
            );
            return state.progress();
        }
        state.failures.push(WaveFailure {
            index,
            error,
            duration,
            // R4 (2026-06-17-002): the tracker-side `record_failure`
            // path is reached on real worker errors (timeout, crash,
            // panic). It does not have the dimension context — only
            // the dispatcher-side synthetic-failure path does. Leave
            // the dimension fields None so the merge layer emits a
            // plain `wave.worker.failed(worker_failed:...)` record,
            // not the dimension_mismatch variant.
            expected_dimension: None,
            actual_dimension: None,
        });
        state.progress()
    }

    /// Record a failure for a wave instance with explicit dimension
    /// context (U4/R4 of 2026-06-17-002). Used by the dispatcher's
    /// synthetic-failure path so timeout / never-reported slots that
    /// had a dimension assignment are recorded as `dimension_missing`
    /// (rather than a plain "worker did not report") and the merge
    /// layer's `wave.worker.failed(reason=worker_failed:dimension_missing)`
    /// payload carries the expected dimension for downstream
    /// `WaveContext.missing_dimensions` resolution.
    pub fn record_failure_with_dimensions(
        &mut self,
        wave_id: &str,
        index: u32,
        error: String,
        duration: Duration,
        expected_dimension: Option<String>,
        actual_dimension: Option<String>,
    ) -> WaveProgress {
        let Some(state) = self.active_waves.get_mut(wave_id) else {
            tracing::warn!(
                wave_id,
                index,
                "Failure recorded for unknown wave, ignoring"
            );
            return WaveProgress::InProgress {
                received: 0,
                expected: 0,
            };
        };
        if state.has_index(index) {
            tracing::warn!(
                wave_id,
                index,
                "Duplicate worker index in failure, ignoring"
            );
            return state.progress();
        }
        state.failures.push(WaveFailure {
            index,
            error,
            duration,
            expected_dimension,
            actual_dimension,
        });
        state.progress()
    }

    /// Check if a specific worker index has already reported (result or failure).
    pub fn has_reported(&self, wave_id: &str, index: u32) -> bool {
        self.active_waves
            .get(wave_id)
            .is_some_and(|state| state.has_index(index))
    }

    /// Check if a wave is complete (all results + failures == expected total).
    pub fn is_complete(&self, wave_id: &str) -> bool {
        self.active_waves
            .get(wave_id)
            .is_some_and(|state| state.progress() == WaveProgress::Complete)
    }

    /// 2026-06-17-002 U5/R5: read the number of dimension-mismatch
    /// retries that have been issued for `index` in `wave_id`.
    /// Returns 0 if the wave is not tracked or the slot has not
    /// been retried yet. Persisted on the tracker so the budget
    /// survives across `handle_wave_events` invocations (the prior
    /// process-local `HashMap` in the dispatcher was reset on every
    /// dispatch round, allowing a permanent mismatch to loop
    /// indefinitely).
    pub fn dimension_retry_count(&self, wave_id: &str, index: u32) -> u32 {
        self.active_waves
            .get(wave_id)
            .and_then(|s| s.dimension_retry_counts.get(&index).copied())
            .unwrap_or(0)
    }

    /// 2026-06-17-002 U5/R5: increment the per-slot retry counter
    /// and return the new total. Returns `None` if the wave is
    /// not tracked, `Some(used)` (the new count, capped at
    /// `MAX_DIMENSION_RETRIES_PER_SLOT`) otherwise. The caller is
    /// responsible for NOT incrementing if the previous count
    /// already reached the cap (use `dimension_retry_count` to
    /// check first). Increment-only — the counter never
    /// decreases, so a disk-write failure that drops the retry
    /// cannot be retried again, which closes the previously-found
    /// hole where retry-on-failure looped indefinitely.
    pub fn bump_dimension_retry(&mut self, wave_id: &str, index: u32) -> Option<u32> {
        let state = self.active_waves.get_mut(wave_id)?;
        let used = state
            .dimension_retry_counts
            .get(&index)
            .copied()
            .unwrap_or(0);
        let next = used + 1;
        state.dimension_retry_counts.insert(index, next);
        Some(next)
    }

    /// Consume a completed wave, removing it from tracking.
    pub fn take_wave_results(&mut self, wave_id: &str) -> Option<CompletedWave> {
        let state = self.active_waves.remove(wave_id)?;
        // partial=true when not all workers produced successful results
        let partial = (state.results.len() as u32) < state.expected_total;
        Some(CompletedWave {
            wave_id: state.wave_id,
            wave_total: state.expected_total,
            results: state.results,
            failures: state.failures,
            duration: state.started_at.elapsed(),
            partial,
            expected_source_hat: state.expected_source_hat,
            // R4 (2026-06-17-002): the dispatcher stamps this on the
            // returned CompletedWave (see dispatcher.rs); the tracker
            // itself has no per-slot dimension map, so it ships an
            // empty default. Callers that need the dimension gate
            // must set it explicitly.
            assigned_dimensions: std::collections::HashMap::new(),
            // R5 (2026-06-17-002): transfer the per-slot retry
            // counts so the dispatcher (caller) can use them to
            // decide whether to inject a `task.resume` for the
            // mismatched slot, with the budget surviving across
            // dispatch rounds (P0#1 fix).
            dimension_retry_counts: state.dimension_retry_counts,
            // 2026-07-03-001 supervisor real-wiring: the tracker
            // path leaves worker_events empty; the dispatcher's
            // supervisor branch fills this from the merge sink.
            worker_events: Vec::new(),
        })
    }

    /// Force-take wave results even when the wave is not complete.
    ///
    /// Unlike `take_wave_results`, this does not require all workers to have
    /// reported.  Use this for partial wave dispatch after staleness threshold,
    /// or for emergency wave recovery.
    ///
    /// Returns `None` if the wave_id is not tracked.
    pub fn force_take_wave_results(&mut self, wave_id: &str) -> Option<CompletedWave> {
        let state = self.active_waves.remove(wave_id)?;
        // partial=true when not all workers produced successful results
        let partial = (state.results.len() as u32) < state.expected_total;
        Some(CompletedWave {
            wave_id: state.wave_id,
            wave_total: state.expected_total,
            results: state.results,
            failures: state.failures,
            duration: state.started_at.elapsed(),
            partial,
            expected_source_hat: state.expected_source_hat,
            // R4 (2026-06-17-002): see `take_wave_results` for why
            // this starts empty. The dispatcher stamps the actual
            // per-slot assignments before the merge layer reads it.
            assigned_dimensions: std::collections::HashMap::new(),
            dimension_retry_counts: state.dimension_retry_counts,
            worker_events: Vec::new(),
        })
    }

    /// Check if any wave is currently active.
    pub fn has_active_waves(&self) -> bool {
        !self.active_waves.is_empty()
    }

    /// Returns wave IDs that have exceeded the given timeout since registration.
    ///
    /// Useful for enforcing aggregate timeouts — callers can force-complete
    /// these waves with partial results.
    pub fn timed_out_waves(&self, timeout: Duration) -> Vec<&str> {
        self.active_waves
            .values()
            .filter(|state| state.started_at.elapsed() > timeout)
            .map(|state| state.wave_id.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_result_event(topic: &str, payload: &str) -> Event {
        Event::new(topic, payload)
    }

    #[test]
    fn test_register_and_record_results_until_complete() {
        let mut tracker = WaveTracker::new();
        tracker.register_wave("w-abc".into(), 3);

        assert!(tracker.has_active_waves());
        assert!(!tracker.is_complete("w-abc"));

        // Record first result
        let progress = tracker.record_result(
            "w-abc",
            0,
            vec![make_result_event("review.done", "result 0")],
        );
        assert_eq!(
            progress,
            WaveProgress::InProgress {
                received: 1,
                expected: 3
            }
        );
        assert!(!tracker.is_complete("w-abc"));

        // Record second result
        let progress = tracker.record_result(
            "w-abc",
            1,
            vec![make_result_event("review.done", "result 1")],
        );
        assert_eq!(
            progress,
            WaveProgress::InProgress {
                received: 2,
                expected: 3
            }
        );

        // Record third result — should be complete
        let progress = tracker.record_result(
            "w-abc",
            2,
            vec![make_result_event("review.done", "result 2")],
        );
        assert_eq!(progress, WaveProgress::Complete);
        assert!(tracker.is_complete("w-abc"));
    }

    #[test]
    fn test_record_results_and_failure_completes_wave() {
        let mut tracker = WaveTracker::new();
        tracker.register_wave("w-def".into(), 3);

        // Two successes
        tracker.record_result("w-def", 0, vec![make_result_event("review.done", "ok 0")]);
        tracker.record_result("w-def", 1, vec![make_result_event("review.done", "ok 1")]);

        assert!(!tracker.is_complete("w-def"));

        // One failure — should complete the wave (2 results + 1 failure = 3 total)
        let progress =
            tracker.record_failure("w-def", 2, "backend crashed".into(), Duration::from_secs(5));

        assert_eq!(progress, WaveProgress::Complete);
        assert!(tracker.is_complete("w-def"));
    }

    #[test]
    fn test_take_wave_results_returns_all_and_removes() {
        let mut tracker = WaveTracker::new();
        tracker.register_wave("w-take".into(), 3);

        tracker.record_result("w-take", 0, vec![make_result_event("review.done", "r0")]);
        tracker.record_result("w-take", 1, vec![make_result_event("review.done", "r1")]);
        tracker.record_failure("w-take", 2, "failed".into(), Duration::from_secs(3));

        let completed = tracker.take_wave_results("w-take").unwrap();
        assert_eq!(completed.wave_id, "w-take");
        assert_eq!(completed.results.len(), 2);
        assert_eq!(completed.failures.len(), 1);
        assert_eq!(completed.failures[0].index, 2);
        assert_eq!(completed.failures[0].error, "failed");

        // Wave should be removed
        assert!(!tracker.has_active_waves());
        assert!(tracker.take_wave_results("w-take").is_none());
    }

    #[test]
    fn test_multiple_concurrent_waves_tracked_independently() {
        let mut tracker = WaveTracker::new();
        tracker.register_wave("w-1".into(), 2);
        tracker.register_wave("w-2".into(), 3);

        assert!(tracker.has_active_waves());

        // Complete wave 1
        tracker.record_result("w-1", 0, vec![make_result_event("done", "a")]);
        tracker.record_result("w-1", 1, vec![make_result_event("done", "b")]);
        assert!(tracker.is_complete("w-1"));
        assert!(!tracker.is_complete("w-2"));

        // Take wave 1 results
        let w1 = tracker.take_wave_results("w-1").unwrap();
        assert_eq!(w1.results.len(), 2);

        // Wave 2 still active
        assert!(tracker.has_active_waves());
        assert!(!tracker.is_complete("w-2"));

        // Complete wave 2
        tracker.record_result("w-2", 0, vec![make_result_event("done", "x")]);
        tracker.record_failure("w-2", 1, "error".into(), Duration::from_secs(1));
        tracker.record_result("w-2", 2, vec![make_result_event("done", "z")]);

        assert!(tracker.is_complete("w-2"));
        let w2 = tracker.take_wave_results("w-2").unwrap();
        assert_eq!(w2.results.len(), 2);
        assert_eq!(w2.failures.len(), 1);

        assert!(!tracker.has_active_waves());
    }

    #[test]
    fn test_record_result_for_unknown_wave() {
        let mut tracker = WaveTracker::new();
        let progress =
            tracker.record_result("w-unknown", 0, vec![make_result_event("done", "orphan")]);
        assert_eq!(
            progress,
            WaveProgress::InProgress {
                received: 0,
                expected: 0
            }
        );
    }

    #[test]
    fn test_result_with_multiple_events() {
        let mut tracker = WaveTracker::new();
        tracker.register_wave("w-multi".into(), 1);

        // Worker emits multiple events
        let events = vec![
            make_result_event("review.done", "main review"),
            make_result_event("review.note", "additional note"),
        ];
        let progress = tracker.record_result("w-multi", 0, events);
        assert_eq!(progress, WaveProgress::Complete);

        let completed = tracker.take_wave_results("w-multi").unwrap();
        assert_eq!(completed.results.len(), 1);
        assert_eq!(completed.results[0].events.len(), 2);
    }

    #[test]
    fn test_default_impl() {
        let tracker = WaveTracker::default();
        assert!(!tracker.has_active_waves());
    }

    #[test]
    fn test_timed_out_waves_none_when_fresh() {
        let mut tracker = WaveTracker::new();
        tracker.register_wave("w-fresh".into(), 3);

        // Just registered — should not be timed out with any reasonable timeout
        let timed_out = tracker.timed_out_waves(Duration::from_mins(5));
        assert!(timed_out.is_empty());
    }

    #[test]
    fn test_timed_out_waves_returns_expired() {
        let mut tracker = WaveTracker::new();
        tracker.register_wave("w-old".into(), 2);

        // Zero-duration timeout means everything is timed out immediately
        let timed_out = tracker.timed_out_waves(Duration::ZERO);
        assert_eq!(timed_out.len(), 1);
        assert_eq!(timed_out[0], "w-old");
    }

    #[test]
    fn test_timed_out_waves_excludes_completed() {
        let mut tracker = WaveTracker::new();
        tracker.register_wave("w-done".into(), 1);
        tracker.record_result("w-done", 0, vec![make_result_event("done", "ok")]);
        tracker.take_wave_results("w-done");

        // Completed wave should not appear in timed_out_waves
        let timed_out = tracker.timed_out_waves(Duration::ZERO);
        assert!(timed_out.is_empty());
    }

    #[test]
    fn take_wave_results_propagates_wave_total() {
        // R8: the merge function needs to know the original expected
        // total to stamp every merged record with `wave_total`.  The
        // tracker stores `expected_total` in the wave state — verify it
        // makes it into the `CompletedWave` returned by `take_wave_results`.
        let mut tracker = WaveTracker::new();
        tracker.register_wave("w-u3".to_string(), 8);

        // Record 8 results so the wave completes.
        for i in 0..8 {
            let ev = ralph_proto::Event::new("review.dimension.done", "{\"dim\":\"d\"}");
            tracker.record_result("w-u3", i, vec![ev]);
        }
        let completed = tracker
            .take_wave_results("w-u3")
            .expect("wave must complete");
        assert_eq!(completed.wave_total, 8);
        assert_eq!(completed.results.len(), 8);
    }

    #[test]
    fn take_wave_results_wave_total_preserved_on_partial_completion() {
        // Even when not all workers report back (the rest are failures
        // or panics), `wave_total` must reflect the *expected* count, not
        // the *received* count.  Aggregator uses this to detect
        // partial-wave vs full-wave without re-querying the registry.
        let mut tracker = WaveTracker::new();
        tracker.register_wave("w-partial".to_string(), 8);
        for i in 0..5 {
            let ev = ralph_proto::Event::new("review.dimension.done", "");
            tracker.record_result("w-partial", i, vec![ev]);
        }
        for i in 5..8 {
            tracker.record_failure(
                "w-partial",
                i,
                "worker panicked".into(),
                Duration::from_millis(100),
            );
        }
        let completed = tracker
            .take_wave_results("w-partial")
            .expect("wave must complete");
        assert_eq!(
            completed.wave_total, 8,
            "expected_total must travel through take_wave_results"
        );
        assert_eq!(completed.results.len(), 5);
        assert_eq!(completed.failures.len(), 3);
    }

    // -------------------------------------------------------------------------
    // U1: force_take_wave_results tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_force_take_wave_results_returns_partial_when_incomplete() {
        // Register 8 workers, only 3 report → force_take returns partial wave
        let mut tracker = WaveTracker::new();
        tracker.register_wave("w-partial-ft".to_string(), 8);
        for i in 0..3 {
            let ev = ralph_proto::Event::new("review.dimension.done", "");
            tracker.record_result("w-partial-ft", i, vec![ev]);
        }
        // 5 workers never report — force take
        let completed = tracker
            .force_take_wave_results("w-partial-ft")
            .expect("force_take must return Some for tracked wave");
        assert!(completed.partial, "incomplete wave must be marked partial");
        assert_eq!(completed.results.len(), 3);
        assert_eq!(completed.failures.len(), 0);
        assert_eq!(completed.wave_total, 8);
        // Wave should be removed from tracker
        assert!(!tracker.has_active_waves());
    }

    #[test]
    fn test_force_take_wave_results_returns_none_for_unknown() {
        let mut tracker = WaveTracker::new();
        assert!(tracker.force_take_wave_results("w-unknown").is_none());
    }

    #[test]
    fn test_force_take_wave_results_complete_wave_not_partial() {
        // All workers report → force_take should also set partial=false
        let mut tracker = WaveTracker::new();
        tracker.register_wave("w-full".to_string(), 3);
        for i in 0..3 {
            let ev = ralph_proto::Event::new("review.dimension.done", "");
            tracker.record_result("w-full", i, vec![ev]);
        }
        let completed = tracker
            .force_take_wave_results("w-full")
            .expect("force_take must return Some for complete wave");
        assert!(
            !completed.partial,
            "complete wave must NOT be marked partial"
        );
        assert_eq!(completed.results.len(), 3);
    }

    #[test]
    fn test_take_wave_results_sets_partial_when_failures_exist() {
        // All workers accounted (some failures) but not all succeeded → partial
        let mut tracker = WaveTracker::new();
        tracker.register_wave("w-mixed".to_string(), 5);
        for i in 0..3 {
            let ev = ralph_proto::Event::new("review.dimension.done", "");
            tracker.record_result("w-mixed", i, vec![ev]);
        }
        for i in 3..5 {
            tracker.record_failure(
                "w-mixed",
                i,
                "worker failed".into(),
                Duration::from_millis(50),
            );
        }
        let completed = tracker
            .take_wave_results("w-mixed")
            .expect("wave must complete");
        assert!(
            completed.partial,
            "mixed result/failure wave must be partial (not all succeeded)"
        );
        assert_eq!(completed.results.len(), 3);
        assert_eq!(completed.failures.len(), 2);
    }
}
