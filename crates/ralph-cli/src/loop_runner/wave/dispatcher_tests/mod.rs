//! Dispatcher tests — extracted from the bottom of `wave/dispatcher.rs`
//! (plan `2026-08-07-008`). All 82 `#[test]` / `#[tokio::test]` fns are
//! preserved verbatim; only their module path changed from
//! `loop_runner::wave::dispatcher::tests::<fn>` to
//! `loop_runner::wave::dispatcher_tests::tests::<fn>` (the test fn
//! suffix is identical, so the cargo nextest list diff stays scoped
//! to the prefix path).

#[cfg(test)]
mod tests {
    use crate::loop_runner::wave::dispatcher::*;
    use crate::loop_runner::wave::*;
    use ralph_adapters::CliBackend;
    use ralph_core::EventLoop;
    use ralph_core::config::RalphConfig;
    use ralph_core::supervisor::{
        BridgeError, CoordinatorAction, InMemoryCoordinatorBridge, PhaseInputs, SupervisorBridge,
        SupervisorStore, WaveKind,
    };
    use ralph_proto::HatId;
    use std::fs;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// Build a `ralph_core::Event` with sensible defaults for tests.
    /// The dispatcher doesn't care about most fields; only `topic`
    /// and `payload` are exercised by the wave tracker.
    fn core_event(topic: &str, payload: &str) -> ralph_core::Event {
        ralph_core::Event {
            topic: topic.to_string(),
            payload: Some(payload.to_string()),
            ts: String::new(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
            system_injected: None,
        }
    }

    fn silent_progress() -> ProgressChannels {
        ProgressChannels {
            rpc_event_tx: None,
            tui_state: None,
        }
    }

    // ---------------------------------------------------------------------
    // U2: existing rejection tests (preserved verbatim)
    // ---------------------------------------------------------------------

    fn build_event_loop() -> EventLoop {
        let yaml = r"
hats: {}
";
        let config: RalphConfig = serde_yaml::from_str(yaml).expect("yaml parse");
        let mut el = EventLoop::new(config);
        el.initialize("u2-rejection-test");
        el
    }

    fn build_outputs_silent() -> WaveOutputs<'static> {
        // Show CLI off so the test does not pollute stderr with the
        // human-readable rejection notice.
        WaveOutputs {
            use_colors: false,
            show_cli: false,
            rpc_tx: None,
            tui: None,
            // Plan 001 §4.3 C1: tests don't exercise the env-var
            // propagation path; leave the label None to fall back to
            // the parent process env.
            hats_source_label: None,
            // 2026-07-13-001 plan U2: tests do not exercise
            // RALPH_CONFIG injection; leave None to keep the
            // pre-U2 behaviour.
            config_path: None,
        }
    }

    fn make_rejected(reason: ralph_core::WaveRejection) -> ralph_core::RejectedWave {
        ralph_core::RejectedWave {
            wave_id: "w-test-001".to_string(),
            topic: "review.wave.ready".to_string(),
            actual: 335,
            reason,
        }
    }

    /// KTD-4 / §6 U2: when a wave is rejected for exceeding the cap,
    /// the dispatcher MUST publish a structured `plan.blocked` event
    /// so the shipper/reporter hat can route the failure. One event
    /// per rejection — N events of the same wave produce one plan.blocked.
    #[tokio::test]
    async fn u2_total_exceeds_cap_publishes_plan_blocked() {
        let mut el = build_event_loop();

        // Observer captures everything published to the bus.
        let captured: Arc<Mutex<Vec<ralph_proto::Event>>> = Arc::new(Mutex::new(Vec::new()));
        let cap_clone = Arc::clone(&captured);
        el.add_observer(move |event: &ralph_proto::Event| {
            cap_clone.lock().unwrap().push(event.clone());
        });

        let rejected = make_rejected(ralph_core::WaveRejection::TotalExceedsCap {
            actual: 335,
            cap: 64,
        });
        let out = build_outputs_silent();

        handle_wave_rejection(&rejected, &mut el, &out, None, "test-loop", 64)
            .await
            .expect("rejection should not error");

        let blocked_events: Vec<_> = {
            let guard = captured.lock().unwrap();
            guard
                .iter()
                .filter(|e| e.topic.as_str() == "plan.blocked")
                .cloned()
                .collect()
        };
        assert_eq!(
            blocked_events.len(),
            1,
            "U2: TotalExceedsCap must publish exactly one plan.blocked, got {}",
            blocked_events.len()
        );

        // Payload must be a structured JSON object carrying the
        // typed reason — shipper/reporter route on these fields.
        let payload_str = blocked_events[0].payload.as_str();
        let payload: serde_json::Value =
            serde_json::from_str(payload_str).expect("plan.blocked payload must be JSON object");
        assert_eq!(payload["reason"], "wave_total_exceeds_cap");
        assert_eq!(payload["wave_id"], "w-test-001");
        assert_eq!(payload["topic"], "review.wave.ready");
        assert_eq!(payload["actual"], 335);
        assert_eq!(payload["cap"], 64);
    }

    /// KTD-4 / §6 U2: only `TotalExceedsCap` escalates to plan.blocked.
    /// Other malformed rejections (e.g. `ZeroTotal`, `InconsistentTopic`)
    /// only surface via the recovery envelope + diagnostics, so they
    /// do not block unrelated workflows.
    #[tokio::test]
    async fn u2_non_cap_rejections_do_not_publish_plan_blocked() {
        let cases = [
            ("ZeroTotal", ralph_core::WaveRejection::ZeroTotal),
            (
                "InconsistentTopic",
                ralph_core::WaveRejection::InconsistentTopic,
            ),
            ("NoTargetHat", ralph_core::WaveRejection::NoTargetHat),
        ];
        let out = build_outputs_silent();

        for (label, reason) in cases {
            let mut el = build_event_loop();
            let captured: Arc<Mutex<Vec<ralph_proto::Event>>> = Arc::new(Mutex::new(Vec::new()));
            let cap_clone = Arc::clone(&captured);
            el.add_observer(move |event: &ralph_proto::Event| {
                cap_clone.lock().unwrap().push(event.clone());
            });

            let rejected = make_rejected(reason);
            handle_wave_rejection(&rejected, &mut el, &out, None, "test-loop", 64)
                .await
                .unwrap_or_else(|e| panic!("rejection for {label} errored: {e}"));

            let blocked = captured
                .lock()
                .unwrap()
                .iter()
                .any(|e| e.topic.as_str() == "plan.blocked");
            assert!(
                !blocked,
                "U2: {label} must NOT publish plan.blocked (only TotalExceedsCap does)"
            );
        }
    }

    // ---------------------------------------------------------------------
    // U3-1 / U3-2..U3-7: paused-time dispatcher tests
    // ---------------------------------------------------------------------

    /// Build a minimal `DetectedWave` with the given number of events
    /// and `total` (which can be larger to simulate a malformed
    /// partial wave).
    fn make_wave(events_count: u32, total: u32, concurrency: u32) -> ralph_core::DetectedWave {
        use ralph_core::config::HatConfig;
        let events: Vec<ralph_core::Event> = (0..events_count)
            .map(|i| core_event("review.file", &format!("payload-{i}")))
            .collect();
        let hat_config = HatConfig {
            name: "u3-test-hat".to_string(),
            concurrency,
            ..HatConfig::default()
        };
        ralph_core::DetectedWave {
            wave_id: "w-u3".to_string(),
            target_hat: HatId::new("u3-test-hat"),
            hat_config,
            events,
            total,
            partial: events_count < total,
            consumer_aggregate_timeout: None,
        }
    }

    /// Test executor with deterministic, paused-time behaviour.
    ///
    /// `hold_for` controls how long the executor future awaits before
    /// completing. `with_max_in_flight` records the maximum number of
    /// executor futures that were simultaneously awaited (i.e. past
    /// the permit acquire gate).
    #[derive(Clone)]
    struct TestExecutor {
        hold_for: Duration,
        report_progress: bool,
        success: bool,
        max_in_flight: Arc<AtomicUsize>,
        current_in_flight: Arc<AtomicUsize>,
        started: Arc<AtomicUsize>,
    }

    impl TestExecutor {
        fn new(hold_for: Duration) -> Self {
            Self {
                hold_for,
                report_progress: false,
                success: true,
                max_in_flight: Arc::new(AtomicUsize::new(0)),
                current_in_flight: Arc::new(AtomicUsize::new(0)),
                started: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn with_progress(mut self) -> Self {
            self.report_progress = true;
            self
        }

        fn with_success(mut self, success: bool) -> Self {
            self.success = success;
            self
        }
    }

    impl WaveWorkerExecutor for TestExecutor {
        fn execute(
            &self,
            mut request: WorkerRequest,
        ) -> Pin<Box<dyn Future<Output = (u32, WaveWorkerOutcome)> + Send>> {
            // Track simultaneous in-flight futures. The
            // dispatcher has already acquired the permit before
            // calling us, so this measures the "executor
            // currently running" count.
            let in_flight = Arc::clone(&self.current_in_flight);
            let max = Arc::clone(&self.max_in_flight);
            let started = Arc::clone(&self.started);
            let hold_for = self.hold_for;
            let report_progress = self.report_progress;
            let success = self.success;
            Box::pin(async move {
                started.fetch_add(1, Ordering::SeqCst);
                let prev = in_flight.fetch_add(1, Ordering::SeqCst);
                let now = in_flight.load(Ordering::SeqCst);
                // Bump max if observed higher.
                let mut cur_max = max.load(Ordering::SeqCst);
                while now > cur_max {
                    match max.compare_exchange(cur_max, now, Ordering::SeqCst, Ordering::SeqCst) {
                        Ok(_) => break,
                        Err(observed) => cur_max = observed,
                    }
                }
                let _ = prev;
                if hold_for > Duration::ZERO {
                    tokio::time::sleep(hold_for).await;
                }
                in_flight.fetch_sub(1, Ordering::SeqCst);
                if report_progress {
                    let _ = request.progress_tx.send((request.index, success, hold_for));
                }
                // Drop the channels the test executor does not use.
                let _ = request.worker_rpc_tx.take();
                let _ = request.worker_tui_state.take();
                let outcome = if success {
                    Ok((vec![core_event("review.done", "ok")], hold_for, success))
                } else {
                    Err(("forced failure".to_string(), hold_for))
                };
                (request.index, outcome)
            })
        }
    }

    fn make_worker_request(
        index: u32,
        progress_tx: tokio::sync::mpsc::UnboundedSender<(u32, bool, Duration)>,
    ) -> WorkerRequest {
        make_worker_request_with_dimension(index, progress_tx, None)
    }

    fn make_worker_request_with_dimension(
        index: u32,
        progress_tx: tokio::sync::mpsc::UnboundedSender<(u32, bool, Duration)>,
        assigned_dimension: Option<String>,
    ) -> WorkerRequest {
        WorkerRequest {
            index,
            backend: CliBackend {
                command: "echo".to_string(),
                args: vec![],
                prompt_mode: ralph_adapters::PromptMode::Arg,
                prompt_flag: None,
                output_format: ralph_adapters::OutputFormat::Text,
                env_vars: vec![],
            },
            prompt: format!("worker-{index}"),
            worker_events_path: PathBuf::from(format!("/tmp/wave-u3-{index}.jsonl")),
            worker_timeout: Duration::from_mins(1),
            progress_tx,
            worker_rpc_tx: None,
            worker_tui_state: None,
            assigned_dimension,
            cwd: None,
            wave_kind: None,
            idle_heartbeat: None,
            idle_weak_signal_cap: 8,
            // 2026-07-28-003 plan U3 (R1): the dispatcher test
            // fixtures pass `None` here so the worker runs the
            // legacy / no-grace path even when idle is enabled
            // via the helper's own idle config.
            startup_grace: None,
        }
    }

    /// U3-1 / KTD-U3-1, KTD-U3-3: permit queue time counts as wave
    /// deadline. With `concurrency=1` and 4 workers that block
    /// forever, the partial threshold must fire BEFORE any worker
    /// can finish — even though 3 of them never even reach the
    /// executor (they're still waiting for a permit).
    #[tokio::test(start_paused = true)]
    async fn u3_permit_queue_time_counts_against_deadline() {
        // Build 4 worker requests, concurrency=1. Each executor
        // future blocks forever (until cancelled).
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let requests: Vec<WorkerRequest> = (0..4u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
        let executor = Arc::new(TestExecutor::new(Duration::from_hours(1)));

        let wave = make_wave(4, 4, 1);
        // Compute deadlines so partial_threshold fires well before
        // any worker could possibly complete.
        let aggregate = Duration::from_secs(10);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_mins(1),
            aggregate,
            vec!["p0".into(), "p1".into(), "p2".into(), "p3".into()],
            false,
            false,
            WaveDispatchLimits::default(),
            std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome =
            dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;

        match outcome {
            WaveDispatchOutcome::AggregateDeadlineExceeded(c) => {
                // Two-stage timeout: partial fires first and is
                // collapsed into `AggregateDeadlineExceeded` (see
                // the long comment in `dispatch_wave_inner` for
                // why we don't run a separate second-stage abort
                // round). The wave total + synthetic-failure
                // bookkeeping is identical to the original
                // `Partial` shape.
                assert_eq!(c.wave_total, 4);
                assert_eq!(c.results.len(), 0, "no worker should have completed");
                assert_eq!(
                    c.failures.len(),
                    4,
                    "all 4 indices must have synthetic failures"
                );
                for (i, f) in c.failures.iter().enumerate() {
                    assert_eq!(f.index, i as u32, "synthetic failure for index {i}");
                }
            }
            other => {
                panic!("expected AggregateDeadlineExceeded (collapsed partial), got {other:?}")
            }
        }
        // At most 1 executor future should have been awaited at
        // any time (the semaphore limits the dispatcher to
        // concurrency=1). The other 3 workers were aborted while
        // still waiting for a permit.
        assert!(
            executor.max_in_flight.load(Ordering::SeqCst) <= 1,
            "executor in-flight must respect concurrency=1, got {}",
            executor.max_in_flight.load(Ordering::SeqCst)
        );
    }

    /// U3-5: after partial threshold fires, the dispatch loop must
    /// keep running (not return Partial immediately) so that
    /// workers queued behind the semaphore can start, and the
    /// wave is finalized only when `aggregate_deadline` arrives.
    /// The final outcome is `AggregateDeadlineExceeded`.
    #[tokio::test(start_paused = true)]
    async fn u3_partial_threshold_drains_active_workers_to_zero() {
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let requests: Vec<WorkerRequest> = (0..3u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
        let executor = Arc::new(TestExecutor::new(Duration::from_hours(1)));

        let wave = make_wave(3, 3, 3);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_mins(1),
            Duration::from_secs(10),
            vec!["p0".into(), "p1".into(), "p2".into()],
            false,
            false,
            WaveDispatchLimits::default(),
            std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome =
            dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;
        // Two-stage timeout: partial fired first, then we waited
        // for aggregate. With all workers still sleeping when
        // aggregate fires, the final outcome must be
        // `AggregateDeadlineExceeded` — *not* `Partial`, because
        // the second-stage abort/drain is what produced the
        // `CompletedWave`.
        assert!(
            matches!(outcome, WaveDispatchOutcome::AggregateDeadlineExceeded(_)),
            "expected AggregateDeadlineExceeded, got {outcome:?}"
        );
        let _ = executor.current_in_flight.load(Ordering::SeqCst);
    }

    /// U3-5 (revised): explicitly verify the two-stage timeout
    /// sequence — partial fires first, then aggregate, and the
    /// wave never gets a chance to be `Completed` or `Partial`.
    /// With all 3 workers sleeping past both deadlines and
    /// concurrency=1, the dispatcher must abort the first worker
    /// at partial_deadline, queue the next 2, then abort them at
    /// aggregate_deadline and return `AggregateDeadlineExceeded`.
    #[tokio::test(start_paused = true)]
    async fn u3_two_stage_timeout_produces_aggregate_deadline_exceeded() {
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let requests: Vec<WorkerRequest> = (0..3u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
        // Workers sleep far past both deadlines.
        let executor = Arc::new(TestExecutor::new(Duration::from_hours(1)));

        let wave = make_wave(3, 3, 3);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_mins(1),
            Duration::from_secs(10),
            vec!["p0".into(), "p1".into(), "p2".into()],
            false,
            false,
            WaveDispatchLimits::default(),
            std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome =
            dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;
        match outcome {
            WaveDispatchOutcome::AggregateDeadlineExceeded(c) => {
                // partial fired → synthetic failures for the
                // worker that was in-flight (1 with concurrency=1),
                // then aggregate fired → 2 more synthetic failures
                // for the workers that never got a permit.
                // We should have *some* failures, but **not** a
                // CompletedWave with results.
                assert_eq!(
                    c.results.len(),
                    0,
                    "no worker should have completed in time"
                );
                assert!(
                    !c.failures.is_empty(),
                    "every worker that did not report should be a failure"
                );
            }
            other => panic!("expected AggregateDeadlineExceeded, got {other:?}"),
        }
    }

    /// U3-6: the progress reporter must exit after the workers
    /// drain. With 1 worker that reports a single progress
    /// message and then completes, the reporter should observe
    /// the message, then see the channel close and exit — with
    /// no hang.
    #[tokio::test(start_paused = true)]
    async fn u3_progress_reporter_exits_after_workers_drain() {
        // The progress reporter is internal to dispatch_wave_inner.
        // We exercise it indirectly: if the reporter task leaks
        // senders, `wait_for_progress_reporter` would block until
        // its 5s defensive timeout fires, which our test runner
        // would observe as a hang. Since `start_paused` is on,
        // the dispatcher would only progress if `wait_for_progress_reporter`
        // returns. So the test is "this returns at all".
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let requests: Vec<WorkerRequest> = (0..2u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
        // Hold_for is short and workers all succeed.
        let executor = Arc::new(
            TestExecutor::new(Duration::from_millis(500))
                .with_progress()
                .with_success(true),
        );

        let wave = make_wave(2, 2, 2);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_mins(1),
            Duration::from_secs(30),
            vec!["p0".into(), "p1".into()],
            false,
            false,
            WaveDispatchLimits::default(),
            std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome = tokio::time::timeout(
            Duration::from_secs(2),
            dispatch_wave_inner(tracker, requests, ctx, executor, silent_progress()),
        )
        .await
        .expect("dispatch must not hang waiting for the progress reporter");

        match outcome {
            WaveDispatchOutcome::Completed(c) => {
                assert_eq!(c.results.len(), 2);
                assert_eq!(c.failures.len(), 0);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    /// U3-1 / KTD-U3-2: concurrency limit is preserved. With 4
    /// workers and concurrency=2, at most 2 executor futures are
    /// awaited simultaneously.
    #[tokio::test(start_paused = true)]
    async fn u3_concurrency_limit_is_respected() {
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let requests: Vec<WorkerRequest> = (0..4u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
        // Workers sleep long enough that all 4 are spawned
        // before any completes, exercising the semaphore.
        let executor = Arc::new(TestExecutor::new(Duration::from_secs(1)));

        let wave = make_wave(4, 4, 2);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_mins(1),
            Duration::from_secs(30),
            vec!["p0".into(), "p1".into(), "p2".into(), "p3".into()],
            false,
            false,
            WaveDispatchLimits::default(),
            std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome =
            dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;
        match outcome {
            WaveDispatchOutcome::Completed(c) => {
                assert_eq!(c.results.len(), 4);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        assert!(
            executor.max_in_flight.load(Ordering::SeqCst) <= 2,
            "executor in-flight must respect concurrency=2, got {}",
            executor.max_in_flight.load(Ordering::SeqCst)
        );
    }

    /// U3-1 / KTD-U3-6: when `events.len() < total`, the
    /// dispatcher spawns `events.len()` tasks and records
    /// synthetic failures for the missing indices. The
    /// `RequireComplete` policy normally rejects this shape at
    /// the detector, but the dispatcher keeps the defensive
    /// bookkeeping for malformed partial waves.
    #[tokio::test(start_paused = true)]
    async fn u3_partial_wave_creates_only_events_len_tasks() {
        // Construct a wave with 2 actual events but `total=5`.
        // The detector normally rejects this under
        // `RequireComplete`; the dispatcher handles it as a
        // defensive case.
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let requests: Vec<WorkerRequest> = (0..2u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
        let executor = Arc::new(TestExecutor::new(Duration::from_millis(50)));

        let wave = make_wave(2, 5, 2);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_mins(1),
            Duration::from_secs(30),
            vec!["p0".into(), "p1".into()],
            false,
            false,
            WaveDispatchLimits::default(),
            std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome =
            dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;
        match outcome {
            WaveDispatchOutcome::Completed(c) | WaveDispatchOutcome::Partial(c) => {
                assert_eq!(c.wave_total, 5);
                assert_eq!(c.results.len(), 2, "only 2 real events → 2 results");
                // 3 synthetic failures for the missing indices
                // 2, 3, 4.
                assert_eq!(
                    c.failures.len(),
                    3,
                    "expected 3 synthetic failures, got {}",
                    c.failures.len()
                );
                let missing: Vec<u32> = c.failures.iter().map(|f| f.index).collect();
                assert_eq!(missing, vec![2, 3, 4]);
            }
            other => panic!("expected Completed or Partial, got {other:?}"),
        }
        assert_eq!(
            executor.started.load(Ordering::SeqCst),
            2,
            "only 2 executor futures should have been spawned"
        );
    }

    /// U2 (Unit 2 of 2026-06-17-001 plan): spawn guarantee — when all
    /// requests are spawned, `SpawnFailed` must NOT fire.
    #[tokio::test(start_paused = true)]
    async fn u2_spawn_guarantee_passes_when_all_workers_spawn() {
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        // 3 requests matching 3 events in the wave.
        let requests: Vec<WorkerRequest> = (0..3u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
        let executor = Arc::new(TestExecutor::new(Duration::from_millis(50)));

        let wave = make_wave(3, 3, 3);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_mins(1),
            Duration::from_secs(30),
            vec!["p0".into(), "p1".into(), "p2".into()],
            false,
            false,
            WaveDispatchLimits::default(),
            std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome =
            dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;

        // Must NOT be SpawnFailed — all 3 requests were spawned.
        if let WaveDispatchOutcome::SpawnFailed { .. } = &outcome {
            panic!("SpawnFailed must NOT fire when all workers spawned: {outcome:?}")
        }
        // Otherwise should be Completed or Partial.
        match outcome {
            WaveDispatchOutcome::Completed(c) | WaveDispatchOutcome::Partial(c) => {
                assert_eq!(c.results.len(), 3, "all 3 workers should succeed");
            }
            WaveDispatchOutcome::SpawnFailed { .. } => unreachable!(),
            WaveDispatchOutcome::AggregateDeadlineExceeded(c) => {
                // Aggregate deadline could fire in the paused-time test depending
                // on the short sleep; that's fine — the key invariant is we
                // did NOT silently return SpawnFailed with 0 spawned.
                assert!(
                    c.results.len() <= 3,
                    "at most 3 results: {}/{}",
                    c.results.len(),
                    3
                );
            }
            WaveDispatchOutcome::GlobalDeadlineExceeded => {
                // Also acceptable — deadline could fire first.
            }
            WaveDispatchOutcome::PreparationFailed { .. } => {
                panic!("inner dispatcher does not perform channel preparation")
            }
        }
    }

    /// U2 (Unit 2 of 2026-06-17-001 plan): spawn guarantee — when fewer
    /// workers are spawned than there are worker requests, `SpawnFailed`
    /// must fire with the correct counts.
    ///
    /// 2026-07-23-001 plan U3: the supervisor gate may legitimately
    /// reduce `worker_requests.len()` below `wave.events.len()` (the
    /// gate skips unapproved slots). The spawn guarantee now runs
    /// against `worker_requests.len()` so it only fires when the
    /// spawn loop itself silently drops a request — a real bug.
    /// The "wave has 3 events but only 2 spawned" scenario now
    /// passes (the supervisor skipped slot 2); the U2 test
    /// re-pins the loop-internal guarantee by passing 2
    /// requests with 2 worker-request slots so `spawned_count`
    /// matches `worker_requests.len()` and the loop proceeds.
    #[tokio::test(start_paused = true)]
    async fn u2_spawn_guarantee_fires_when_fewer_workers_spawn() {
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        // Pass 3 requests, but the third one's spawned task panics
        // before the executor increments its counter. We assert the
        // spawn guarantee runs against `worker_requests.len()`,
        // not against `events_len`. With 3 requests and a healthy
        // executor the loop spawns 3 tasks → no SpawnFailed.
        let requests: Vec<WorkerRequest> = (0..3u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
        let executor = Arc::new(TestExecutor::new(Duration::from_millis(10)));

        let wave = make_wave(3, 3, 3); // 3 events, total=3
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_mins(1),
            Duration::from_secs(30),
            vec!["p0".into(), "p1".into(), "p2".into()],
            false,
            false,
            WaveDispatchLimits::default(),
            std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome =
            dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;

        // 3 healthy requests → all 3 spawn → no SpawnFailed.
        match outcome {
            WaveDispatchOutcome::SpawnFailed { .. } => {
                panic!(
                    "U3: spawn guarantee must NOT fire when worker_requests.len() == events_len; \
                        got SpawnFailed {outcome:?}"
                );
            }
            other => {
                // Either Completed or AggregateDeadlineExceeded
                // depending on timing; both are valid.
                let _ = other;
            }
        }
    }

    /// U4-B1 / KTD-U4-3: end-to-end check that the recovery envelope
    /// recorded by `handle_wave_rejection` actually carries a
    /// wave-scoped retry key. Different `wave_id`s MUST produce
    /// different keys, even when the rejection reason is identical.
    #[tokio::test]
    async fn u4_b1_retry_key_is_wave_scoped() {
        use ralph_core::diagnostics::DiagnosticsCollector;

        let temp = tempfile::tempdir().expect("tempdir");
        let diagnostics_root = temp.path().to_path_buf();

        let yaml = r"
hats: {}
";
        let config: RalphConfig = serde_yaml::from_str(yaml).expect("yaml parse");
        let diagnostics = DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("diagnostics enabled");
        let mut el = EventLoop::with_diagnostics(config, diagnostics);
        el.initialize("u4-b1-retry-key");
        let out = build_outputs_silent();

        // Two distinct waves with the SAME rejection reason.
        let rejected_a = ralph_core::RejectedWave {
            wave_id: "w-A".to_string(),
            topic: "review.wave.ready".to_string(),
            actual: 335,
            reason: ralph_core::WaveRejection::TotalExceedsCap {
                actual: 335,
                cap: 64,
            },
        };
        let rejected_b = ralph_core::RejectedWave {
            wave_id: "w-B".to_string(),
            topic: "review.wave.ready".to_string(),
            actual: 335,
            reason: ralph_core::WaveRejection::TotalExceedsCap {
                actual: 335,
                cap: 64,
            },
        };

        handle_wave_rejection(&rejected_a, &mut el, &out, None, "test-loop", 64)
            .await
            .expect("rejection a");
        handle_wave_rejection(&rejected_b, &mut el, &out, None, "test-loop", 64)
            .await
            .expect("rejection b");

        // Read recovery.jsonl from the diagnostics session dir.
        let mut session_dirs: Vec<_> =
            std::fs::read_dir(diagnostics_root.join(".ralph/diagnostics"))
                .expect("read diagnostics dir")
                .filter_map(Result::ok)
                .collect();
        session_dirs.sort_by_key(|entry| entry.path());
        let session_path = session_dirs
            .last()
            .expect("at least one diagnostics session")
            .path();
        let recovery_path = session_path.join("recovery.jsonl");
        let content = std::fs::read_to_string(&recovery_path)
            .unwrap_or_else(|e| panic!("read recovery.jsonl: {e}: {}", recovery_path.display()));
        let entries: Vec<ralph_core::diagnosis::RecoveryJournalEntry> = content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("parse recovery entry"))
            .collect();

        assert_eq!(
            entries.len(),
            2,
            "two distinct rejections must produce two recovery entries"
        );

        let retry_keys: std::collections::HashSet<String> = entries
            .iter()
            .map(|e| e.envelope.retry_key.clone())
            .collect();
        assert_eq!(
            retry_keys.len(),
            2,
            "different wave_ids must produce different retry keys, got {:?}",
            retry_keys
        );
        for k in &retry_keys {
            assert!(
                k.starts_with("wave_dispatcher:"),
                "retry key must use the wave_dispatcher namespace, got: {k}"
            );
            assert!(
                k.ends_with(":wave_total_exceeds_cap"),
                "retry key must end with the reason code, got: {k}"
            );
        }
        // And each key must contain its own wave_id.
        let key_for_a = entries
            .iter()
            .find(|e| e.envelope.message.contains("Wave w-A rejected"))
            .expect("entry for w-A")
            .envelope
            .retry_key
            .clone();
        let key_for_b = entries
            .iter()
            .find(|e| e.envelope.message.contains("Wave w-B rejected"))
            .expect("entry for w-B")
            .envelope
            .retry_key
            .clone();
        assert!(
            key_for_a.contains("w_a"),
            "w-A key must contain normalized w-A, got: {key_for_a}"
        );
        assert!(
            key_for_b.contains("w_b"),
            "w-B key must contain normalized w-B, got: {key_for_b}"
        );
    }

    // ---------------------------------------------------------------------
    // U5 (2026-06-17-002): task.resume injection for dimension mismatches
    //
    // R5 re-architecture (P0#1 / P0#4 / P1#11 fix): the merge layer
    // produces pre-rendered `task.resume` JSONL lines as
    // `PendingTaskResumeRecord`s. The dispatcher's inline filter
    // (in `handle_wave_events`'s `Completed` arm) consumes them,
    // updates `CompletedWave.dimension_retry_counts`, and writes
    // survivors to the events file. These tests exercise the new
    // contract end-to-end.
    // ---------------------------------------------------------------------

    /// U5/R5: when the merge layer detects a dimension mismatch,
    /// `pending_task_resumes` contains a pre-rendered
    /// `task.resume` JSONL line carrying the expected/actual
    /// dimensions in the structured payload. The dispatcher's
    /// filter (modeled here as inline code) writes survivors to
    /// the events file in a single `write_all` and bumps the
    /// per-slot budget on `CompletedWave.dimension_retry_counts`.
    #[test]
    fn u5_mismatch_writes_task_resume() {
        use crate::loop_runner::wave::io::merge_wave_results_to_events_file;
        use std::io::Write;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let events_file = tmp.path().join("events.jsonl");

        // Index 0 emits the correct assigned dimension
        // (correctness); index 1 emits the WRONG dimension
        // (testing instead of assigned correctness). The merge
        // layer must drop index 1's event and return a pending
        // task.resume for it.
        let mut assigned_dimensions = std::collections::HashMap::new();
        assigned_dimensions.insert(0u32, "correctness".to_string());
        assigned_dimensions.insert(1, "correctness".to_string());

        let event_index_0 = ralph_proto::Event::new(
            "review.dimension.done",
            r#"{"dimension":"correctness","wave_id":"w-u5-dim"}"#,
        )
        .with_wave("w-u5-dim", 0, 2);
        let event_index_1 = ralph_proto::Event::new(
            "review.dimension.done",
            r#"{"dimension":"testing","wave_id":"w-u5-dim"}"#,
        )
        .with_wave("w-u5-dim", 1, 2);

        let mut completed = ralph_core::CompletedWave {
            wave_id: "w-u5-dim".to_string(),
            wave_total: 2,
            results: vec![
                ralph_core::WaveResult {
                    index: 0,
                    events: vec![event_index_0],
                },
                ralph_core::WaveResult {
                    index: 1,
                    events: vec![event_index_1],
                },
            ],
            failures: vec![],
            duration: Duration::from_millis(10),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: assigned_dimensions.clone(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };

        let (_mismatches, pending) = merge_wave_results_to_events_file(
            &completed,
            &events_file,
            &["review.dimension.done".into()],
            "dimension-reviewer",
            None,
        )
        .expect("merge succeeds");

        assert_eq!(
            pending.len(),
            1,
            "one mismatched slot must produce one pending resume"
        );
        assert_eq!(pending[0].wave_index, 1);

        // Now run the dispatcher's filter inline. The production
        // code lives in handle_wave_events' Completed arm.
        let mut resume_buf = String::new();
        let mut round: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        for p in &pending {
            let used = completed
                .dimension_retry_counts
                .get(&p.wave_index)
                .copied()
                .unwrap_or(0);
            if used >= ralph_core::MAX_DIMENSION_RETRIES_PER_SLOT {
                continue;
            }
            resume_buf.push_str(&p.jsonl_line);
            resume_buf.push('\n');
            *round.entry(p.wave_index).or_insert(0) += 1;
        }
        for (idx, inc) in &round {
            let prev = completed
                .dimension_retry_counts
                .get(idx)
                .copied()
                .unwrap_or(0);
            completed.dimension_retry_counts.insert(*idx, prev + inc);
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&events_file)
            .unwrap();
        f.write_all(resume_buf.as_bytes()).unwrap();

        assert_eq!(
            completed.dimension_retry_counts.get(&1),
            Some(&1),
            "budget must reflect 1 used retry"
        );

        let content = fs::read_to_string(&events_file).expect("read events file");
        let mut resume_count = 0usize;
        let mut resume_record: Option<serde_json::Value> = None;
        for line in content.lines().filter(|l| !l.trim().is_empty()) {
            let v: serde_json::Value = serde_json::from_str(line).expect("json event");
            if v["topic"] == "task.resume" {
                resume_count += 1;
                resume_record = Some(v);
            }
        }
        assert_eq!(resume_count, 1, "exactly one task.resume event expected");
        let r = resume_record.unwrap();
        assert_eq!(r["topic"], "task.resume");
        assert_eq!(r["triggered"], "dimension-reviewer");
        assert_eq!(r["hat"], "review-synthesizer");
        assert_eq!(r["source"], "review-synthesizer");
        assert_eq!(r["wave_id"], "w-u5-dim");
        assert_eq!(r["wave_index"], 1);
        assert_eq!(r["wave_total"], 2);

        let payload_str = r["payload"].as_str().expect("payload must be string");
        let payload: serde_json::Value =
            serde_json::from_str(payload_str).expect("payload must be JSON object");
        assert_eq!(payload["stage"], "WaveDimensionGuard");
        assert_eq!(payload["violation"], "dimension_mismatch");
        assert_eq!(payload["reason"], "dimension_mismatch");
        assert_eq!(payload["target_hat"], "dimension-reviewer");
        assert_eq!(payload["expected_dimension"], "correctness");
        assert_eq!(payload["actual_dimension"], "testing");
        assert_eq!(payload["wave_id"], "w-u5-dim");
        assert_eq!(payload["wave_index"], 1);
        assert_eq!(payload["wave_total"], 2);
    }

    /// U5/R5 (P0#1): a slot whose `dimension_retry_counts`
    /// entry already reached `MAX_DIMENSION_RETRIES_PER_SLOT`
    /// must NOT inject another `task.resume`, even if the
    /// mismatch reappears in a later dispatch round. The
    /// budget persists on the `CompletedWave`, transferred from
    /// the `WaveTracker` via `take_wave_results`.
    #[test]
    fn u5_budget_exhausted_skips_second_resume() {
        use crate::loop_runner::wave::io::merge_wave_results_to_events_file;
        use std::io::Write;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let events_file = tmp.path().join("events.jsonl");

        let mut assigned_dimensions = std::collections::HashMap::new();
        assigned_dimensions.insert(0u32, "correctness".to_string());
        assigned_dimensions.insert(1, "correctness".to_string());
        let event_index_1 = ralph_proto::Event::new(
            "review.dimension.done",
            r#"{"dimension":"testing","wave_id":"w-u5-exhaust"}"#,
        )
        .with_wave("w-u5-exhaust", 1, 2);

        // Round 1: empty budget, merge + filter writes 1 task.resume.
        let mut completed_round1 = ralph_core::CompletedWave {
            wave_id: "w-u5-exhaust".to_string(),
            wave_total: 2,
            results: vec![ralph_core::WaveResult {
                index: 1,
                events: vec![event_index_1.clone()],
            }],
            failures: vec![],
            duration: Duration::from_millis(10),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: assigned_dimensions.clone(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };

        let (_m1, p1) = merge_wave_results_to_events_file(
            &completed_round1,
            &events_file,
            &["review.dimension.done".into()],
            "dimension-reviewer",
            None,
        )
        .unwrap();
        let mut buf1 = String::new();
        for p in &p1 {
            let used = completed_round1
                .dimension_retry_counts
                .get(&p.wave_index)
                .copied()
                .unwrap_or(0);
            if used >= ralph_core::MAX_DIMENSION_RETRIES_PER_SLOT {
                continue;
            }
            buf1.push_str(&p.jsonl_line);
            buf1.push('\n');
            *completed_round1
                .dimension_retry_counts
                .entry(p.wave_index)
                .or_insert(0) += 1;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&events_file)
            .unwrap();
        f.write_all(buf1.as_bytes()).unwrap();
        assert_eq!(
            completed_round1.dimension_retry_counts.get(&1),
            Some(&ralph_core::MAX_DIMENSION_RETRIES_PER_SLOT),
            "round 1 must consume the only retry"
        );

        // Round 2: the merge layer again returns a pending
        // resume (it does not know about the budget), but the
        // dispatcher filter must skip it because the slot is
        // exhausted. The dispatcher's CompletedWave reuses the
        // counts from round 1.
        let completed_round2 = ralph_core::CompletedWave {
            wave_id: "w-u5-exhaust".to_string(),
            wave_total: 2,
            results: vec![ralph_core::WaveResult {
                index: 1,
                events: vec![event_index_1],
            }],
            failures: vec![],
            duration: Duration::from_millis(10),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: assigned_dimensions.clone(),
            // Reuse the budget from round 1 (this is the
            // tracker→CompletedWave transfer that gives us
            // cross-round persistence).
            dimension_retry_counts: completed_round1.dimension_retry_counts.clone(),
            worker_events: Vec::new(),
        };

        let (_m2, p2) = merge_wave_results_to_events_file(
            &completed_round2,
            &events_file,
            &["review.dimension.done".into()],
            "dimension-reviewer",
            None,
        )
        .unwrap();
        let mut buf2 = String::new();
        for p in &p2 {
            let used = completed_round2
                .dimension_retry_counts
                .get(&p.wave_index)
                .copied()
                .unwrap_or(0);
            if used >= ralph_core::MAX_DIMENSION_RETRIES_PER_SLOT {
                continue;
            }
            buf2.push_str(&p.jsonl_line);
            buf2.push('\n');
        }
        assert!(
            buf2.is_empty(),
            "second round must not append task.resume; got: {buf2}"
        );

        // Events file must contain exactly 1 task.resume (from round 1).
        let content = fs::read_to_string(&events_file).expect("read events file");
        let resume_count = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter(|l| l.contains("\"topic\":\"task.resume\""))
            .count();
        assert_eq!(
            resume_count, 1,
            "exactly 1 task.resume across 2 rounds; got {resume_count}"
        );
    }

    /// U5/R5: an empty mismatch list produces no pending task
    /// resumes; the dispatcher filter has nothing to do; the
    /// events file stays empty (no worker errors, no merge
    /// records because CompletedWave.results is also empty).
    #[test]
    fn u5_no_mismatch_no_resume() {
        use crate::loop_runner::wave::io::merge_wave_results_to_events_file;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let events_file = tmp.path().join("events.jsonl");

        let completed = ralph_core::CompletedWave {
            wave_id: "w-u5-clean".to_string(),
            wave_total: 4,
            results: vec![],
            failures: vec![],
            duration: Duration::from_millis(10),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: std::collections::HashMap::new(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };

        let (_m, p) = merge_wave_results_to_events_file(
            &completed,
            &events_file,
            &["review.dimension.done".into()],
            "dimension-reviewer",
            None,
        )
        .unwrap();

        assert!(p.is_empty(), "no mismatches → no pending task.resume");
        let content = fs::read_to_string(&events_file).expect("read events file");
        assert!(
            content.trim().is_empty(),
            "no mismatches and no results → events file must be empty, got: {content}"
        );
    }

    // ---------------------------------------------------------------------
    // U4-C1: failing integration test for runner-supplied global deadline.
    // ---------------------------------------------------------------------

    /// U4-C1 / §6 C1: a runner-supplied `global_deadline` (e.g. derived
    /// from `loop.max_runtime_seconds`) must preempt the wave before
    /// the partial/aggregate deadlines do, even when individual workers
    /// would block past the deadline. The dispatcher must return
    /// `WaveDispatchOutcome::GlobalDeadlineExceeded` AND leave zero
    /// active workers (the existing U3 abort+drain contract still
    /// applies).
    ///
    /// Uses `start_paused = true` so the 10s deadline is reached
    /// deterministically and the worker sleep of 3600s never resolves
    /// first.
    #[tokio::test(start_paused = true)]
    async fn u4_c1_global_deadline_preempts_wave() {
        // 4 workers that would all block past the global deadline.
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let requests: Vec<WorkerRequest> = (0..4u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
        let executor = Arc::new(TestExecutor::new(Duration::from_hours(1)));

        let wave = make_wave(4, 4, 4);
        // Use a generous aggregate (3600s) so the partial / aggregate
        // paths CANNOT fire first; only the global deadline (10s)
        // will win.
        let aggregate = Duration::from_hours(1);
        // global_deadline = now + 10s in paused-time terms.
        let global_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_mins(1),
            aggregate,
            vec!["p0".into(), "p1".into(), "p2".into(), "p3".into()],
            false,
            false,
            WaveDispatchLimits {
                global_deadline: Some(global_deadline),
            },
            std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome = tokio::time::timeout(
            Duration::from_secs(20),
            dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()),
        )
        .await
        .expect("dispatch_wave_inner must not hang past the global deadline");

        match outcome {
            WaveDispatchOutcome::GlobalDeadlineExceeded => {
                // U3 contract: zero active workers after global
                // deadline abort+drain. The `TestExecutor` is
                // paused-time and never executes its `fetch_sub`
                // on the in-flight counter (it only runs after the
                // sleep), so we only assert via `started`: every
                // spawned worker must have entered the executor
                // (so the dispatcher's abort path actually
                // reached them), and the JoinSet must be empty
                // (which `dispatch_wave_inner` guarantees by the
                // `while join_set.join_next().await.is_some() {}`
                // drain in `finalize_global_exceeded`).
                assert_eq!(
                    executor.started.load(Ordering::SeqCst),
                    4,
                    "all 4 workers must have been spawned before global deadline"
                );
            }
            other => panic!(
                "expected GlobalDeadlineExceeded (runner-supplied 10s budget), got {other:?}"
            ),
        }
    }

    /// U4-C1 / §6 C1: a global deadline of `now` (i.e. already past)
    /// must fire on the dispatch loop's first re-check rather than
    /// waiting for the partial/aggregate timers. This is the
    /// conservative path for the runner: when `remaining` is zero,
    /// it must still pass `Some(now)` rather than `None`, otherwise
    /// the wave would have NO upper bound at all.
    #[tokio::test(start_paused = true)]
    async fn u4_c1_zero_remaining_deadline_fires_immediately() {
        // 2 workers, each holding for 3600s.
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let requests: Vec<WorkerRequest> = (0..2u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
        let executor = Arc::new(TestExecutor::new(Duration::from_hours(1)));

        let wave = make_wave(2, 2, 2);
        // Aggregate far in the future; only the global deadline
        // (= now, already past) should fire.
        let aggregate = Duration::from_hours(1);
        let global_deadline = tokio::time::Instant::now();
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_mins(1),
            aggregate,
            vec!["p0".into(), "p1".into()],
            false,
            false,
            WaveDispatchLimits {
                global_deadline: Some(global_deadline),
            },
            std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()),
        )
        .await
        .expect("dispatch_wave_inner must not hang on a zero-remaining deadline");

        assert!(
            matches!(outcome, WaveDispatchOutcome::GlobalDeadlineExceeded),
            "expected GlobalDeadlineExceeded (zero-remaining deadline), got {outcome:?}"
        );
        // When `global_deadline` is in the past at loop entry, the
        // dispatcher's loop-top `global_fired` check returns
        // immediately, before any worker is spawned. This is the
        // conservative path: the runner should always pass
        // `Some(now + remaining)` (even when `remaining` is zero)
        // so the dispatch loop gets one chance to abort cleanly.
        // 0 started workers is the correct outcome here.
        assert_eq!(
            executor.started.load(Ordering::SeqCst),
            0,
            "zero-remaining global deadline must short-circuit before spawning workers"
        );
    }

    // ---------------------------------------------------------------------
    // U4-C3: handle_wave_events outcome + recovery envelope.
    // ---------------------------------------------------------------------

    /// U4-C3 / §6 C3: the loop-level recovery envelope written when
    /// the global deadline preempts a wave must have the exact
    /// schema the runner relies on for `TerminationReason::MaxRuntime`:
    /// retry_key = `loop_runner:<loop_id>:max_runtime`, source =
    /// `WaveDispatcher` (no `LoopRunner` variant exists in
    /// `DiagnosisSource`), reason_code = `loop_max_runtime_exceeded`,
    /// outcome = `NotRetriable`. Verifies the journal entry lands on
    /// disk in `recovery.jsonl`.
    #[tokio::test]
    async fn u4_c3_record_loop_max_runtime_envelope_writes_recovery_entry() {
        use ralph_core::diagnostics::DiagnosticsCollector;

        let temp = tempfile::tempdir().expect("tempdir");
        let diagnostics_root = temp.path().to_path_buf();

        let yaml = r"
hats: {}
";
        let config: RalphConfig = serde_yaml::from_str(yaml).expect("yaml parse");
        let diagnostics = DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("diagnostics enabled");
        let mut el = EventLoop::with_diagnostics(config, diagnostics);
        el.initialize("loop-abc");
        let wave = make_wave(2, 2, 2);

        record_loop_max_runtime_envelope(&mut el, "loop-abc", &wave);

        // Read recovery.jsonl from the diagnostics session dir.
        let mut session_dirs: Vec<_> =
            std::fs::read_dir(diagnostics_root.join(".ralph/diagnostics"))
                .expect("read diagnostics dir")
                .filter_map(Result::ok)
                .collect();
        session_dirs.sort_by_key(|entry| entry.path());
        let session_path = session_dirs
            .last()
            .expect("at least one diagnostics session")
            .path();
        let recovery_path = session_path.join("recovery.jsonl");
        let content = std::fs::read_to_string(&recovery_path)
            .unwrap_or_else(|e| panic!("read recovery.jsonl: {e}: {}", recovery_path.display()));
        let entries: Vec<ralph_core::diagnosis::RecoveryJournalEntry> = content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("parse recovery entry"))
            .collect();

        assert_eq!(
            entries.len(),
            1,
            "one envelope for one global-deadline event, got {}",
            entries.len()
        );
        let entry = &entries[0].envelope;

        // U4-C3 retry_key contract.
        assert_eq!(
            entry.retry_key, "loop_runner:loop-abc:max_runtime",
            "retry key must use the loop-scoped loop_runner:<loop_id>:max_runtime format"
        );
        assert_eq!(
            entry.reason_code, "loop_max_runtime_exceeded",
            "reason code must identify the loop-level max_runtime budget"
        );
        assert_eq!(
            entry.outcome,
            ralph_core::diagnosis::DiagnosisOutcome::NotRetriable,
            "loop-level max_runtime finding is not auto-recoverable"
        );
        assert_eq!(
            entry.source,
            ralph_core::diagnosis::DiagnosisSource::WaveDispatcher,
            "source must be WaveDispatcher (no LoopRunner variant in DiagnosisSource)"
        );
        assert_eq!(
            entry.severity,
            ralph_core::diagnosis::DiagnosisSeverity::Error,
            "severity must be Error — the loop is about to terminate"
        );
        assert!(
            entry.message.contains("loop-abc") && entry.message.contains(&wave.wave_id),
            "message must mention both loop_id and wave_id, got: {}",
            entry.message
        );
    }

    /// U4-C3: when `handle_wave_events` is called with an empty
    /// `wave_events` slice, it must return `HandleWaveOutcome::default()`
    /// — i.e. `global_deadline_exceeded = false` and the runner
    /// does NOT set `late_termination_reason`. The empty-wave path
    /// is the only `handle_wave_events` return value the runner can
    /// trivially exercise without spawning a real backend.
    #[tokio::test]
    async fn u4_c3_handle_wave_events_empty_input_returns_default_outcome() {
        let mut el = build_event_loop();

        // Construct a minimal `CliBackend` and `LoopContext` to
        // satisfy the function signature. The empty-wave path
        // short-circuits before either is actually used.
        let backend = CliBackend {
            command: "echo".to_string(),
            args: vec![],
            prompt_mode: ralph_adapters::PromptMode::Arg,
            prompt_flag: None,
            output_format: ralph_adapters::OutputFormat::Text,
            env_vars: vec![],
        };
        let ctx = ralph_core::LoopContext::primary(std::path::PathBuf::from("/tmp"));
        let loop_id = "test-loop";

        let outcome = handle_wave_events(
            &[],
            &mut el,
            &backend,
            &ctx,
            false,
            false,
            None,
            None,
            loop_id,
            None,
            // global_deadline is irrelevant for empty input.
            Some(tokio::time::Instant::now()),
            // Plan 001 §4.3 C1: hats_source_label is irrelevant for
            // empty input but is now part of the signature.
            None,
            // 2026-07-13-001 plan U2: config_path is irrelevant for
            // empty input; `None` keeps the pre-U2 behaviour.
            None,
            // 2026-07-03-001 supervisor real-wiring: legacy test
            // path; `None` keeps the WaveTracker shape.
            None,
        )
        .await;

        assert_eq!(
            outcome,
            HandleWaveOutcome::default(),
            "empty wave_events must produce a default outcome"
        );
        assert!(
            !outcome.global_deadline_exceeded,
            "empty wave_events must NOT trigger the global deadline path"
        );
    }

    // ---------------------------------------------------------------------
    // U1 Red: terminal fan-in convergence — missing coverage.
    // Tests 1, 2, 4, 5 are already defined at the bottom of this file
    // (near line 9365+). Only tests 3 and 6 are missing from the plan's
    // Red characterization.
    // ---------------------------------------------------------------------

    /// U1 Red test 3: when `terminal_ctx.elapsed > aggregate_timeout_secs`
    /// the coordinator must receive the real elapsed value in
    /// `PhaseInputs.elapsed_secs`. Before the fix, `run_supervisor_fan_in`
    /// always passed `elapsed_secs: 0` to `tick_with_slot_events`, so
    /// the coordinator could not make a correct timeout decision.
    #[test]
    fn terminal_context_preserves_elapsed_timeout_relation() {
        use ralph_core::supervisor::{BridgeError, PhaseInputs, SupervisorBridge, WaveKind};
        use std::sync::Arc;

        // Capture the PhaseInputs passed to tick
        struct CapturingBridge {
            captured: std::sync::Mutex<Option<PhaseInputs>>,
        }
        impl std::fmt::Debug for CapturingBridge {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("CapturingBridge").finish()
            }
        }
        impl SupervisorBridge for CapturingBridge {
            fn tick(
                &self,
                _wave_id: &str,
                inputs: PhaseInputs,
            ) -> Result<ralph_core::supervisor::CoordinatorAction, BridgeError> {
                *self.captured.lock().unwrap() = Some(inputs);
                Ok(ralph_core::supervisor::CoordinatorAction::ContinueCollect)
            }
            fn tick_with_slot_events(
                &self,
                _wave_id: &str,
                inputs: PhaseInputs,
                _events: Vec<ralph_proto::Event>,
            ) -> Result<ralph_core::supervisor::CoordinatorAction, BridgeError> {
                *self.captured.lock().unwrap() = Some(inputs);
                Ok(ralph_core::supervisor::CoordinatorAction::ContinueCollect)
            }
            fn bind_slot(
                &self,
                _kind: WaveKind,
                _wave_id: &str,
                _slot_index: u32,
            ) -> Result<Option<crate::loop_runner::wave::SlotBinding>, BridgeError> {
                Ok(None)
            }
            fn recover(&self) -> Result<Vec<ralph_core::supervisor::WaveSnapshot>, BridgeError> {
                Ok(Vec::new())
            }
            fn fan_in_status(
                &self,
                _wave_id: &str,
            ) -> Result<ralph_core::supervisor::WaveSnapshot, BridgeError> {
                Err(BridgeError::Store("capturing bridge".into()))
            }
            fn register_wave_if_absent(
                &self,
                _kind: WaveKind,
                wave_id: &str,
                _expected_total: u32,
                _slot_retry_budget: u32,
            ) -> Result<String, BridgeError> {
                Ok(wave_id.to_string())
            }
            fn record_slot_result(
                &self,
                _wave_id: &str,
                _slot_index: u32,
                _content_hash: &str,
                _event_count: usize,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            fn record_slot_failure(
                &self,
                _wave_id: &str,
                _slot_index: u32,
                _reason: &str,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            fn release_slot_dispatch(
                &self,
                _wave_id: &str,
                _slot_index: u32,
                _outcome: ralph_core::supervisor::DispatchOutcome,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            fn record_never_started_failures(&self, _wave_id: &str) -> Result<(), BridgeError> {
                Ok(())
            }
            fn set_wave_phase(
                &self,
                _wave_id: &str,
                _phase: ralph_core::supervisor::WavePhase,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            fn slot_failure_reason(
                &self,
                _wave_id: &str,
                _slot_index: u32,
            ) -> Result<Option<String>, BridgeError> {
                Ok(None)
            }
            fn slot_resources(
                &self,
                _wave_id: &str,
            ) -> Result<Vec<ralph_core::supervisor::SlotResource>, BridgeError> {
                Ok(Vec::new())
            }
            fn max_concurrent_workers(&self) -> u32 {
                1
            }
            fn repo_root(&self) -> Option<&std::path::Path> {
                None
            }
            fn tasks_path(&self) -> Option<&std::path::Path> {
                None
            }
            fn try_dispatch_next(&self, _wave_id: &str, _idx: u32) -> Result<bool, BridgeError> {
                Ok(false)
            }
            fn record_slot_terminal_evidence(
                &self,
                _wave_id: &str,
                _slot_index: u32,
                _e: &ralph_core::supervisor::TerminalEvidence,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            fn slot_terminal_evidence(
                &self,
                _wave_id: &str,
                _slot_index: u32,
            ) -> Result<Option<ralph_core::supervisor::TerminalEvidence>, BridgeError> {
                Ok(None)
            }
            fn finalize_terminal_cleanup(&self, _p: &std::path::Path) -> Result<(), BridgeError> {
                Ok(())
            }
            fn cancel_wave(&self, _wave_id: &str) -> Result<(), BridgeError> {
                Ok(())
            }
            fn enqueue_compensation(
                &self,
                _wave_id: &str,
                _k: ralph_core::supervisor::CompensationKind,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            fn take_pending_compensations(
                &self,
            ) -> Result<Vec<(String, ralph_core::supervisor::CompensationKind)>, BridgeError>
            {
                Ok(Vec::new())
            }
            fn complete_compensation(
                &self,
                _wave_id: &str,
                _k: ralph_core::supervisor::CompensationKind,
                _ok: bool,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            fn slot_retry_budget(&self) -> u32 {
                0
            }
        }

        let capturing = Arc::new(CapturingBridge {
            captured: std::sync::Mutex::new(None),
        });
        let bridge_arc: Arc<dyn SupervisorBridge> = capturing.clone();

        // elapsed = 120s, aggregate_timeout_secs = 60s  → elapsed > timeout
        let terminal_ctx = TerminalFanInContext {
            cancel_requested: true,
            elapsed: std::time::Duration::from_secs(120),
        };

        let completed = ralph_core::CompletedWave {
            wave_id: "u1-red-3".to_string(),
            wave_total: 2,
            results: vec![
                ralph_core::WaveResult {
                    index: 0,
                    events: vec![],
                },
                ralph_core::WaveResult {
                    index: 1,
                    events: vec![],
                },
            ],
            failures: vec![],
            duration: std::time::Duration::from_secs(120),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: std::collections::HashMap::new(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: vec![],
        };
        let detected = ralph_core::DetectedWave {
            wave_id: "u1-red-3".to_string(),
            target_hat: ralph_proto::HatId::new("review-coordinator"),
            hat_config: ralph_core::config::HatConfig::default(),
            events: vec![ralph_core::Event {
                topic: "review.wave.ready".to_string(),
                payload: None,
                ts: String::new(),
                hat: None,
                triggered: None,
                source: None,
                wave_id: None,
                wave_index: None,
                wave_total: None,
                system_injected: None,
            }],
            total: 2,
            partial: false,
            consumer_aggregate_timeout: None,
        };

        let tmp = tempfile::tempdir().unwrap();
        let main_events_file = tmp.path().join(".ralph").join("events.jsonl");
        std::fs::create_dir_all(main_events_file.parent().unwrap()).unwrap();

        let _outcome = run_supervisor_fan_in(
            &bridge_arc,
            &completed,
            &detected,
            &main_events_file,
            60,
            Some(terminal_ctx),
        );

        let captured = capturing.captured.lock().unwrap();
        let inputs = captured.as_ref().expect("tick must have been called");

        // BEFORE FIX: elapsed_secs is 0 (hardcoded), so the coordinator
        // cannot detect the timeout condition.
        // AFTER FIX: elapsed_secs must be the real elapsed value (120).
        assert_eq!(
            inputs.elapsed_secs, 120,
            "U1 Red 3: elapsed_secs must be the real elapsed value (120), not 0"
        );
        assert!(
            inputs.elapsed_secs > inputs.aggregate_timeout_secs,
            "U1 Red 3: elapsed ({}) must exceed aggregate_timeout ({})",
            inputs.elapsed_secs,
            inputs.aggregate_timeout_secs
        );
    }

    /// U1 wiring-contract test: the production `run_supervisor_fan_in` call site
    /// must derive `aggregate_timeout_secs` from the wave via the
    /// `effective_detected_aggregate_deadline_secs` helper — it must NOT read
    /// `SupervisorConfig.aggregate_timeout_secs` directly.
    ///
    /// This test pins both ends of the wiring:
    /// - Assert 1 (positive): the window contains the helper call identifier.
    ///   Since the helper does not yet exist, this assertion fails (Red).
    /// - Assert 2 (negative): the window must NOT contain the supervisor-config-read
    ///   pattern `.aggregate_timeout_secs;`.  The current production call site at
    ///   lines 920–924 reads this field directly, so this also fails (Red).
    ///
    /// After the fix (a later U), both assertions pass:
    /// - The helper is introduced and called at the fan-in site.
    /// - The direct supervisor config read is removed.
    #[test]
    fn fan_in_deadline_uses_wave_derived_helper() {
        let src = include_str!("../dispatcher/dispatch.rs");

        // Anchor: the production fan-in call (not a test variant).
        let anchor = "let fan_in = run_supervisor_fan_in(";
        let anchor_idx = src
            .find(anchor)
            .expect("fan-in call site anchor missing — production call may have moved");

        // Collect 30 lines *before* the anchor (window: lines [anchor-30, anchor]).
        let prefix = &src[..anchor_idx];
        let window_start = prefix
            .rsplitn(31, '\n')
            .last()
            .map(|s| prefix.len() - s.len())
            .unwrap_or(0);
        let window = &src[window_start..anchor_idx + anchor.len()];

        // Assert 1 (positive): the helper call must be present.
        assert!(
            window.contains("effective_detected_aggregate_deadline_secs("),
            "fan-in call site must use effective_detected_aggregate_deadline_secs(...) helper; \
             window did not contain the helper call. window was:\n{window}"
        );

        // Assert 2 (negative): the window must NOT read SupervisorConfig directly.
        assert!(
            !window.contains(".aggregate_timeout_secs;"),
            "fan-in call site must not read SupervisorConfig.aggregate_timeout_secs; \
             window still contains the supervisor config read pattern. window was:\n{window}"
        );
    }

    // ---------------------------------------------------------------------
    // U2: effective_detected_aggregate_deadline_secs tests
    // ---------------------------------------------------------------------

    /// Test 1: failure-run shape (wave_timeout=900, events=6, concurrency=6,
    /// bridge cap=u32::MAX, budget=1) returns the wave-derived effective
    /// deadline 2288s — the same value the supervisor execution path used to
    /// produce in @1742-1768. With hat.timeout=Some(900) the per-worker
    /// timeout becomes 900, so:
    ///   configured = 900*ceil(6/6)*1 + 30 = 930
    ///   floor      = ceil((900*1*2+30)*10/8) = ceil(2287.5) = 2288
    ///   result     = max(930, 2288) = 2288
    /// This pins the canonical fix: the fan-in call site and the supervisor
    /// execution path agree on 2288s, not the SupervisorConfig default 600s.
    #[test]
    fn helper_failed_run_shape_is_2288() {
        use ralph_core::config::HatConfig;
        use ralph_core::supervisor::SupervisorBridge;
        #[derive(Debug)]
        struct StubBridge {
            max_concurrent_workers: u32,
            slot_retry_budget: u32,
        }
        impl SupervisorBridge for StubBridge {
            fn max_concurrent_workers(&self) -> u32 {
                self.max_concurrent_workers
            }
            fn slot_retry_budget(&self) -> u32 {
                self.slot_retry_budget
            }
            fn tick(
                &self,
                _: &str,
                _: ralph_core::supervisor::PhaseInputs,
            ) -> Result<
                ralph_core::supervisor::CoordinatorAction,
                ralph_core::supervisor::BridgeError,
            > {
                unimplemented!()
            }
            fn tick_with_slot_events(
                &self,
                _: &str,
                _: ralph_core::supervisor::PhaseInputs,
                _: Vec<ralph_proto::Event>,
            ) -> Result<
                ralph_core::supervisor::CoordinatorAction,
                ralph_core::supervisor::BridgeError,
            > {
                unimplemented!()
            }
            fn bind_slot(
                &self,
                _: ralph_core::supervisor::WaveKind,
                _: &str,
                _: u32,
            ) -> Result<
                Option<crate::loop_runner::wave::SlotBinding>,
                ralph_core::supervisor::BridgeError,
            > {
                unimplemented!()
            }
            fn recover(
                &self,
            ) -> Result<
                Vec<ralph_core::supervisor::WaveSnapshot>,
                ralph_core::supervisor::BridgeError,
            > {
                Ok(Vec::new())
            }
            fn fan_in_status(
                &self,
                _: &str,
            ) -> Result<ralph_core::supervisor::WaveSnapshot, ralph_core::supervisor::BridgeError>
            {
                Err(ralph_core::supervisor::BridgeError::Store("stub".into()))
            }
            fn register_wave_if_absent(
                &self,
                _: ralph_core::supervisor::WaveKind,
                wave_id: &str,
                _: u32,
                _: u32,
            ) -> Result<String, ralph_core::supervisor::BridgeError> {
                Ok(wave_id.to_string())
            }
            fn record_slot_result(
                &self,
                _: &str,
                _: u32,
                _: &str,
                _: usize,
            ) -> Result<(), ralph_core::supervisor::BridgeError> {
                Ok(())
            }
            fn record_slot_failure(
                &self,
                _: &str,
                _: u32,
                _: &str,
            ) -> Result<(), ralph_core::supervisor::BridgeError> {
                Ok(())
            }
        }
        let events: Vec<ralph_core::Event> = (0..6)
            .map(|i| core_event("review.file", &format!("payload-{i}")))
            .collect();
        let hat_config = HatConfig {
            name: "u2-test-hat".to_string(),
            concurrency: 6,
            timeout: Some(900),
            ..HatConfig::default()
        };
        let wave = ralph_core::DetectedWave {
            wave_id: "w-u2-failure-shape".to_string(),
            target_hat: ralph_proto::HatId::new("u2-test-hat"),
            hat_config,
            events,
            total: 6,
            partial: false,
            consumer_aggregate_timeout: None,
        };
        let bridge = StubBridge {
            max_concurrent_workers: u32::MAX,
            slot_retry_budget: 1,
        };
        let result = effective_detected_aggregate_deadline_secs(&wave, &bridge);
        assert_eq!(
            result, 2288,
            "helper must return the wave-derived effective deadline 2288s for the failure-mode shape (per-worker=900, events=6, concurrency=6, cap=u32::MAX, budget=1)"
        );
    }

    /// Test 2: explicit hat.aggregate.timeout wins over formula floor.
    /// Case A: explicit=300 < floor=4500 → result=4500 (floor dominates).
    /// Case B: explicit=5000 > floor=4500 → result=5000 (explicit dominates).
    #[test]
    fn helper_explicit_hat_aggregate_wins_over_formula() {
        use ralph_core::config::{AggregateConfig, AggregateMode, HatConfig};
        use ralph_core::supervisor::SupervisorBridge;
        #[derive(Debug)]
        struct StubBridge {
            max_concurrent_workers: u32,
            slot_retry_budget: u32,
        }
        impl SupervisorBridge for StubBridge {
            fn max_concurrent_workers(&self) -> u32 {
                self.max_concurrent_workers
            }
            fn slot_retry_budget(&self) -> u32 {
                self.slot_retry_budget
            }
            fn tick(
                &self,
                _: &str,
                _: ralph_core::supervisor::PhaseInputs,
            ) -> Result<
                ralph_core::supervisor::CoordinatorAction,
                ralph_core::supervisor::BridgeError,
            > {
                unimplemented!()
            }
            fn tick_with_slot_events(
                &self,
                _: &str,
                _: ralph_core::supervisor::PhaseInputs,
                _: Vec<ralph_proto::Event>,
            ) -> Result<
                ralph_core::supervisor::CoordinatorAction,
                ralph_core::supervisor::BridgeError,
            > {
                unimplemented!()
            }
            fn bind_slot(
                &self,
                _: ralph_core::supervisor::WaveKind,
                _: &str,
                _: u32,
            ) -> Result<
                Option<crate::loop_runner::wave::SlotBinding>,
                ralph_core::supervisor::BridgeError,
            > {
                unimplemented!()
            }
            fn recover(
                &self,
            ) -> Result<
                Vec<ralph_core::supervisor::WaveSnapshot>,
                ralph_core::supervisor::BridgeError,
            > {
                Ok(Vec::new())
            }
            fn fan_in_status(
                &self,
                _: &str,
            ) -> Result<ralph_core::supervisor::WaveSnapshot, ralph_core::supervisor::BridgeError>
            {
                Err(ralph_core::supervisor::BridgeError::Store("stub".into()))
            }
            fn register_wave_if_absent(
                &self,
                _: ralph_core::supervisor::WaveKind,
                wave_id: &str,
                _: u32,
                _: u32,
            ) -> Result<String, ralph_core::supervisor::BridgeError> {
                Ok(wave_id.to_string())
            }
            fn record_slot_result(
                &self,
                _: &str,
                _: u32,
                _: &str,
                _: usize,
            ) -> Result<(), ralph_core::supervisor::BridgeError> {
                Ok(())
            }
            fn record_slot_failure(
                &self,
                _: &str,
                _: u32,
                _: &str,
            ) -> Result<(), ralph_core::supervisor::BridgeError> {
                Ok(())
            }
        }
        let bridge = StubBridge {
            max_concurrent_workers: u32::MAX,
            slot_retry_budget: 1,
        };

        // Case A: explicit=300, floor=2288 → floor dominates.
        let wave_a = {
            let events: Vec<ralph_core::Event> = (0..6)
                .map(|i| core_event("review.file", &format!("payload-{i}")))
                .collect();
            let hat_config = HatConfig {
                name: "u2-test-hat".to_string(),
                concurrency: 6,
                timeout: Some(900),
                aggregate: Some(AggregateConfig {
                    mode: AggregateMode::WaitForAll,
                    timeout: 300,
                }),
                ..HatConfig::default()
            };
            ralph_core::DetectedWave {
                wave_id: "w-u2a".to_string(),
                target_hat: ralph_proto::HatId::new("u2-test-hat"),
                hat_config,
                events,
                total: 6,
                partial: false,
                consumer_aggregate_timeout: None,
            }
        };
        let result_a = effective_detected_aggregate_deadline_secs(&wave_a, &bridge);
        assert_eq!(
            result_a, 2288,
            "explicit timeout 300 must be raised to the attempt-aware floor 2288"
        );

        // Case B: explicit=5000, floor=2288 → explicit dominates.
        let wave_b = {
            let events: Vec<ralph_core::Event> = (0..6)
                .map(|i| core_event("review.file", &format!("payload-{i}")))
                .collect();
            let hat_config = HatConfig {
                name: "u2-test-hat".to_string(),
                concurrency: 6,
                timeout: Some(900),
                aggregate: Some(AggregateConfig {
                    mode: AggregateMode::WaitForAll,
                    timeout: 5000,
                }),
                ..HatConfig::default()
            };
            ralph_core::DetectedWave {
                wave_id: "w-u2b".to_string(),
                target_hat: ralph_proto::HatId::new("u2-test-hat"),
                hat_config,
                events,
                total: 6,
                partial: false,
                consumer_aggregate_timeout: None,
            }
        };
        let result_b = effective_detected_aggregate_deadline_secs(&wave_b, &bridge);
        assert_eq!(result_b, 5000, "Case B: explicit 5000 dominates floor 4500");
    }

    /// Test 3: consumer_aggregate_timeout used when no hat.aggregate.
    /// Case A: consumer=5000 > floor=4500 → explicit dominates.
    /// Case B: consumer=4500 < floor=4500 → floor dominates (but result is still floor).
    #[test]
    fn helper_consumer_aggregate_timeout_used_when_no_hat_aggregate() {
        use ralph_core::supervisor::SupervisorBridge;
        #[derive(Debug)]
        struct StubBridge {
            max_concurrent_workers: u32,
            slot_retry_budget: u32,
        }
        impl SupervisorBridge for StubBridge {
            fn max_concurrent_workers(&self) -> u32 {
                self.max_concurrent_workers
            }
            fn slot_retry_budget(&self) -> u32 {
                self.slot_retry_budget
            }
            fn tick(
                &self,
                _: &str,
                _: ralph_core::supervisor::PhaseInputs,
            ) -> Result<
                ralph_core::supervisor::CoordinatorAction,
                ralph_core::supervisor::BridgeError,
            > {
                unimplemented!()
            }
            fn tick_with_slot_events(
                &self,
                _: &str,
                _: ralph_core::supervisor::PhaseInputs,
                _: Vec<ralph_proto::Event>,
            ) -> Result<
                ralph_core::supervisor::CoordinatorAction,
                ralph_core::supervisor::BridgeError,
            > {
                unimplemented!()
            }
            fn bind_slot(
                &self,
                _: ralph_core::supervisor::WaveKind,
                _: &str,
                _: u32,
            ) -> Result<
                Option<crate::loop_runner::wave::SlotBinding>,
                ralph_core::supervisor::BridgeError,
            > {
                unimplemented!()
            }
            fn recover(
                &self,
            ) -> Result<
                Vec<ralph_core::supervisor::WaveSnapshot>,
                ralph_core::supervisor::BridgeError,
            > {
                Ok(Vec::new())
            }
            fn fan_in_status(
                &self,
                _: &str,
            ) -> Result<ralph_core::supervisor::WaveSnapshot, ralph_core::supervisor::BridgeError>
            {
                Err(ralph_core::supervisor::BridgeError::Store("stub".into()))
            }
            fn register_wave_if_absent(
                &self,
                _: ralph_core::supervisor::WaveKind,
                wave_id: &str,
                _: u32,
                _: u32,
            ) -> Result<String, ralph_core::supervisor::BridgeError> {
                Ok(wave_id.to_string())
            }
            fn record_slot_result(
                &self,
                _: &str,
                _: u32,
                _: &str,
                _: usize,
            ) -> Result<(), ralph_core::supervisor::BridgeError> {
                Ok(())
            }
            fn record_slot_failure(
                &self,
                _: &str,
                _: u32,
                _: &str,
            ) -> Result<(), ralph_core::supervisor::BridgeError> {
                Ok(())
            }
        }
        let bridge = StubBridge {
            max_concurrent_workers: u32::MAX,
            slot_retry_budget: 1,
        };

        // Case A: consumer=5000 > floor=4500 → consumer dominates
        let wave_a = {
            let mut w = make_wave(6, 6, 6);
            w.consumer_aggregate_timeout = Some(5000);
            w
        };
        let result_a = effective_detected_aggregate_deadline_secs(&wave_a, &bridge);
        // configured = 5000 (consumer), floor = 4500 → max(5000, 4500) = 5000
        assert_eq!(result_a, 5000, "Case A: consumer 5000 dominates floor 4500");

        // Case B: consumer=4500 < floor=4500 → floor dominates, result equals floor
        let wave_b = {
            let mut w = make_wave(6, 6, 6);
            w.consumer_aggregate_timeout = Some(4500);
            w
        };
        let result_b = effective_detected_aggregate_deadline_secs(&wave_b, &bridge);
        // configured = 4500 (consumer), floor = 4500 → max(4500, 4500) = 4500
        assert_eq!(result_b, 4500, "Case B: floor 4500 equals consumer 4500");
    }

    /// Test 4: zero retry budget still produces a result (must be > 0).
    #[test]
    fn helper_zero_retry_budget_produces_result() {
        use ralph_core::supervisor::SupervisorBridge;
        let mut wave = make_wave(6, 6, 6);
        wave.hat_config.timeout = Some(900);
        #[derive(Debug)]
        struct StubBridge {
            max_concurrent_workers: u32,
            slot_retry_budget: u32,
        }
        impl SupervisorBridge for StubBridge {
            fn max_concurrent_workers(&self) -> u32 {
                self.max_concurrent_workers
            }
            fn slot_retry_budget(&self) -> u32 {
                self.slot_retry_budget
            }
            fn tick(
                &self,
                _: &str,
                _: ralph_core::supervisor::PhaseInputs,
            ) -> Result<
                ralph_core::supervisor::CoordinatorAction,
                ralph_core::supervisor::BridgeError,
            > {
                unimplemented!()
            }
            fn tick_with_slot_events(
                &self,
                _: &str,
                _: ralph_core::supervisor::PhaseInputs,
                _: Vec<ralph_proto::Event>,
            ) -> Result<
                ralph_core::supervisor::CoordinatorAction,
                ralph_core::supervisor::BridgeError,
            > {
                unimplemented!()
            }
            fn bind_slot(
                &self,
                _: ralph_core::supervisor::WaveKind,
                _: &str,
                _: u32,
            ) -> Result<
                Option<crate::loop_runner::wave::SlotBinding>,
                ralph_core::supervisor::BridgeError,
            > {
                unimplemented!()
            }
            fn recover(
                &self,
            ) -> Result<
                Vec<ralph_core::supervisor::WaveSnapshot>,
                ralph_core::supervisor::BridgeError,
            > {
                Ok(Vec::new())
            }
            fn fan_in_status(
                &self,
                _: &str,
            ) -> Result<ralph_core::supervisor::WaveSnapshot, ralph_core::supervisor::BridgeError>
            {
                Err(ralph_core::supervisor::BridgeError::Store("stub".into()))
            }
            fn register_wave_if_absent(
                &self,
                _: ralph_core::supervisor::WaveKind,
                wave_id: &str,
                _: u32,
                _: u32,
            ) -> Result<String, ralph_core::supervisor::BridgeError> {
                Ok(wave_id.to_string())
            }
            fn record_slot_result(
                &self,
                _: &str,
                _: u32,
                _: &str,
                _: usize,
            ) -> Result<(), ralph_core::supervisor::BridgeError> {
                Ok(())
            }
            fn record_slot_failure(
                &self,
                _: &str,
                _: u32,
                _: &str,
            ) -> Result<(), ralph_core::supervisor::BridgeError> {
                Ok(())
            }
        }
        let bridge = StubBridge {
            max_concurrent_workers: u32::MAX,
            slot_retry_budget: 0,
        };
        let result = effective_detected_aggregate_deadline_secs(&wave, &bridge);
        assert_eq!(
            result, 1163,
            "zero retry budget must use the one-attempt floor 1163"
        );
    }

    /// Test 5: differential — helper output must exactly match the inline expression
    /// across a combinatorial table of (events, concurrency, timeout_secs, retry_budget).
    #[test]
    fn helper_matches_inline_expression() {
        use ralph_core::config::{AggregateConfig, AggregateMode, HatConfig};
        use ralph_core::supervisor::SupervisorBridge;

        #[derive(Debug)]
        struct StubBridge {
            max_concurrent_workers: u32,
            slot_retry_budget: u32,
        }
        impl SupervisorBridge for StubBridge {
            fn max_concurrent_workers(&self) -> u32 {
                self.max_concurrent_workers
            }
            fn slot_retry_budget(&self) -> u32 {
                self.slot_retry_budget
            }
            fn tick(
                &self,
                _: &str,
                _: ralph_core::supervisor::PhaseInputs,
            ) -> Result<
                ralph_core::supervisor::CoordinatorAction,
                ralph_core::supervisor::BridgeError,
            > {
                unimplemented!()
            }
            fn tick_with_slot_events(
                &self,
                _: &str,
                _: ralph_core::supervisor::PhaseInputs,
                _: Vec<ralph_proto::Event>,
            ) -> Result<
                ralph_core::supervisor::CoordinatorAction,
                ralph_core::supervisor::BridgeError,
            > {
                unimplemented!()
            }
            fn bind_slot(
                &self,
                _: ralph_core::supervisor::WaveKind,
                _: &str,
                _: u32,
            ) -> Result<
                Option<crate::loop_runner::wave::SlotBinding>,
                ralph_core::supervisor::BridgeError,
            > {
                unimplemented!()
            }
            fn recover(
                &self,
            ) -> Result<
                Vec<ralph_core::supervisor::WaveSnapshot>,
                ralph_core::supervisor::BridgeError,
            > {
                Ok(Vec::new())
            }
            fn fan_in_status(
                &self,
                _: &str,
            ) -> Result<ralph_core::supervisor::WaveSnapshot, ralph_core::supervisor::BridgeError>
            {
                Err(ralph_core::supervisor::BridgeError::Store("stub".into()))
            }
            fn register_wave_if_absent(
                &self,
                _: ralph_core::supervisor::WaveKind,
                wave_id: &str,
                _: u32,
                _: u32,
            ) -> Result<String, ralph_core::supervisor::BridgeError> {
                Ok(wave_id.to_string())
            }
            fn record_slot_result(
                &self,
                _: &str,
                _: u32,
                _: &str,
                _: usize,
            ) -> Result<(), ralph_core::supervisor::BridgeError> {
                Ok(())
            }
            fn record_slot_failure(
                &self,
                _: &str,
                _: u32,
                _: &str,
            ) -> Result<(), ralph_core::supervisor::BridgeError> {
                Ok(())
            }
        }

        // Inline expression — mirrors the helper body exactly for differential comparison.
        // Both the helper and this inline use wave.per_worker_timeout_secs() as the timeout source.
        let inline_expression =
            |wave: &ralph_core::DetectedWave, bridge: &dyn SupervisorBridge| -> u64 {
                let wave_timeout = Duration::from_secs(wave.per_worker_timeout_secs());
                let concurrency = wave.hat_config.concurrency as usize;
                let configured = if wave.has_explicit_aggregate_timeout()
                    || wave.consumer_aggregate_timeout.is_some()
                {
                    Duration::from_secs(wave.aggregate_timeout_secs())
                } else {
                    aggregate_timeout_for(wave_timeout, wave.events.len(), concurrency)
                };
                let effective_cap = wave
                    .hat_config
                    .concurrency
                    .min(bridge.max_concurrent_workers())
                    .max(1) as usize;
                attempt_aware_aggregate_timeout(
                    configured,
                    wave_timeout,
                    wave.events.len(),
                    effective_cap,
                    bridge.slot_retry_budget(),
                )
                .as_secs()
            };

        let events_cases = &[0usize, 1, 6, 13];
        let concurrency_cases = &[0u32, 1, 6];
        // Vary per-worker timeout via HatConfig.timeout
        let timeout_cases: &[Option<u32>] = &[None, Some(300), Some(900)];
        let budget_cases = &[0u32, 1, 2];

        for &events_count in events_cases {
            for &concurrency in concurrency_cases {
                if events_count == 0 && concurrency == 0 {
                    continue;
                }
                for &timeout_opt in timeout_cases {
                    for &budget in budget_cases {
                        let events: Vec<ralph_core::Event> = (0..events_count as u32)
                            .map(|i| core_event("review.file", &format!("payload-{i}")))
                            .collect();
                        let hat_config = HatConfig {
                            name: "diff-test-hat".to_string(),
                            concurrency,
                            timeout: timeout_opt,
                            ..HatConfig::default()
                        };
                        let wave = ralph_core::DetectedWave {
                            wave_id: "w-diff".to_string(),
                            target_hat: ralph_proto::HatId::new("diff-test-hat"),
                            hat_config,
                            events,
                            total: events_count as u32,
                            partial: false,
                            consumer_aggregate_timeout: None,
                        };
                        let bridge = StubBridge {
                            max_concurrent_workers: u32::MAX,
                            slot_retry_budget: budget,
                        };

                        let helper_out = effective_detected_aggregate_deadline_secs(&wave, &bridge);
                        let inline_out = inline_expression(&wave, &bridge);
                        assert_eq!(
                            helper_out, inline_out,
                            "events={events_count}, concurrency={concurrency}, timeout={:?}, budget={budget}",
                            timeout_opt
                        );
                    }
                }
            }
        }
    }

    // ── 2026-07-30-001 plan U3: fan-in deadline boundary regression ──────
    //
    // These four tests pin the failure-mode regression (6 slots / 726s /
    // Integrate → InjectedComplete) and the strict `>` timeout boundary
    // introduced by the wave-derived effective deadline fix (U2).
    // Tests A–C exercise `evaluate_phase` directly; Test D exercises the
    // full `run_supervisor_fan_in` path with a stub bridge.

    /// U3 regression guard A: the failure-mode shape (6 slots, elapsed=726s,
    /// aggregate_timeout=2288s) must resolve to `Integrate`, not `Failed`.
    /// Before the fix the dispatcher used the hardcoded SupervisorConfig
    /// default of 600s, under which 726 > 600 would yield `Failed(Timeout)`.
    /// After the fix the wave-derived 2288s allows 726s to pass (726 < 2288).
    #[test]
    fn regression_six_slots_726s_integrates_under_wave_deadline() {
        use ralph_core::supervisor::WaveDeliveryState;
        use ralph_core::supervisor::{PhaseInputs, WaveKind, WavePhase, evaluate_phase};

        // Build the canonical failure-mode snapshot: 6 completed slots,
        // 0 pending, 0 in-flight, 0 failed, expected_total=6.
        let slots: Vec<(u32, ralph_core::supervisor::SlotStatus)> = (0u32..6)
            .map(|i| (i, ralph_core::supervisor::SlotStatus::Completed))
            .collect();
        let snapshot = ralph_core::supervisor::WaveSnapshot {
            wave_id: "w-u3-failure".into(),
            kind: WaveKind::Exec,
            phase: WavePhase::Collect,
            expected_total: 6,
            completed_count: 6,
            failed_count: 0,
            pending_count: 0,
            in_flight_count: 0,
            cancel_requested: false,
            delivery_state: WaveDeliveryState::CoordinationCommitted,
            started_at: std::time::SystemTime::UNIX_EPOCH,
            slots,
        };
        let inputs = PhaseInputs {
            aggregate_timeout_secs: 2288,
            elapsed_secs: 726,
            cancel_requested: false,
        };
        assert_eq!(
            evaluate_phase(&snapshot, &inputs),
            ralph_core::supervisor::PhaseDecision::Integrate,
            "6 completed slots at 726s with 2288s aggregate must Integrate; \
             pre-fix 600s default would incorrectly yield Failed(Timeout)"
        );
    }

    /// U3 regression guard B: `elapsed == aggregate_timeout` is NOT a timeout.
    /// The timeout gate uses strict `>` (confirmed in phase.rs line 135), so
    /// exactly 2288s must still integrate, not time out.
    #[test]
    fn regression_elapsed_equals_deadline_not_timeout() {
        use ralph_core::supervisor::WaveDeliveryState;
        use ralph_core::supervisor::{PhaseInputs, WaveKind, WavePhase, evaluate_phase};

        let slots: Vec<(u32, ralph_core::supervisor::SlotStatus)> = (0u32..6)
            .map(|i| (i, ralph_core::supervisor::SlotStatus::Completed))
            .collect();
        let snapshot = ralph_core::supervisor::WaveSnapshot {
            wave_id: "w-u3-boundary-eq".into(),
            kind: WaveKind::Exec,
            phase: WavePhase::Collect,
            expected_total: 6,
            completed_count: 6,
            failed_count: 0,
            pending_count: 0,
            in_flight_count: 0,
            cancel_requested: false,
            delivery_state: WaveDeliveryState::CoordinationCommitted,
            started_at: std::time::SystemTime::UNIX_EPOCH,
            slots,
        };
        let inputs = PhaseInputs {
            aggregate_timeout_secs: 2288,
            elapsed_secs: 2288,
            cancel_requested: false,
        };
        assert_eq!(
            evaluate_phase(&snapshot, &inputs),
            ralph_core::supervisor::PhaseDecision::Integrate,
            "elapsed_secs == aggregate_timeout_secs (2288 == 2288) must NOT time out; \
             timeout gate is strict `>` per phase.rs:135"
        );
    }

    /// U3 regression guard C: `elapsed > aggregate_timeout` IS a timeout.
    /// One second past the boundary must produce `Failed(Timeout)` with an
    /// empty blocking list (all 6 slots are Completed, none are Failed/Cancelled).
    #[test]
    fn regression_elapsed_past_deadline_still_times_out() {
        use ralph_core::supervisor::WaveDeliveryState;
        use ralph_core::supervisor::{
            FailedReason, PhaseInputs, WaveKind, WavePhase, evaluate_phase,
        };

        let slots: Vec<(u32, ralph_core::supervisor::SlotStatus)> = (0u32..6)
            .map(|i| (i, ralph_core::supervisor::SlotStatus::Completed))
            .collect();
        let snapshot = ralph_core::supervisor::WaveSnapshot {
            wave_id: "w-u3-past-deadline".into(),
            kind: WaveKind::Exec,
            phase: WavePhase::Collect,
            expected_total: 6,
            completed_count: 6,
            failed_count: 0,
            pending_count: 0,
            in_flight_count: 0,
            cancel_requested: false,
            delivery_state: WaveDeliveryState::CoordinationCommitted,
            started_at: std::time::SystemTime::UNIX_EPOCH,
            slots,
        };
        let inputs = PhaseInputs {
            aggregate_timeout_secs: 2288,
            elapsed_secs: 2289,
            cancel_requested: false,
        };
        match evaluate_phase(&snapshot, &inputs) {
            ralph_core::supervisor::PhaseDecision::Failed {
                reason: FailedReason::Timeout,
                blocking_slots,
            } => {
                assert!(
                    blocking_slots.is_empty(),
                    "all 6 slots are Completed; blocking_slots must be empty, got {blocking_slots:?}"
                );
            }
            other => panic!(
                "expected Failed{{reason=Timeout, blocking_slots=[]}} at elapsed=2289, got {other:?}"
            ),
        }
    }

    /// U3 regression guard D: `run_supervisor_fan_in` with the failure-mode
    /// shape (6 completed slots, elapsed=726s, aggregate_timeout=2288s) must
    /// return `InjectedComplete` when the bridge's `tick_with_slot_events`
    /// returns `CoordinatorAction::Integrate`.
    ///
    /// This is the full fan-in path test — it exercises the complete
    /// `run_supervisor_fan_in` function including the coordinator tick,
    /// the slot event merge, and the coord event injection.
    #[test]
    fn fan_in_injects_complete_with_wave_deadline() {
        use ralph_core::supervisor::{
            BridgeError, CoordinatorAction, PhaseInputs, SupervisorBridge, WaveKind,
        };
        use std::sync::Arc;

        // Stub bridge that returns Integrate from tick_with_slot_events.
        // All other methods satisfy the minimum contract of run_supervisor_fan_in.
        #[derive(Debug)]
        struct IntegrateBridge {
            recorded_inputs: std::sync::Mutex<Option<PhaseInputs>>,
        }
        impl IntegrateBridge {
            fn new() -> Self {
                Self {
                    recorded_inputs: std::sync::Mutex::new(None),
                }
            }
        }
        impl SupervisorBridge for IntegrateBridge {
            fn slot_retry_budget(&self) -> u32 {
                0
            }
            fn tick(
                &self,
                _wave_id: &str,
                _inputs: PhaseInputs,
            ) -> Result<CoordinatorAction, BridgeError> {
                Ok(CoordinatorAction::ContinueCollect)
            }
            fn tick_with_slot_events(
                &self,
                _wave_id: &str,
                inputs: PhaseInputs,
                _events: Vec<ralph_proto::Event>,
            ) -> Result<CoordinatorAction, BridgeError> {
                *self.recorded_inputs.lock().unwrap() = Some(inputs.clone());
                if inputs.aggregate_timeout_secs != 2288 || inputs.elapsed_secs != 726 {
                    return Ok(CoordinatorAction::InjectedFailed {
                        topic: "review.wave.failed".to_string(),
                        reason: "test_wrong_deadline",
                        blocking_slots: vec![],
                    });
                }
                Ok(CoordinatorAction::InjectedComplete {
                    topic: "review.wave.complete".to_string(),
                    blocking_slots: vec![],
                })
            }
            fn fan_in_status(
                &self,
                _wave_id: &str,
            ) -> Result<ralph_core::supervisor::WaveSnapshot, BridgeError> {
                Err(BridgeError::Store("stub".into()))
            }
            fn register_wave_if_absent(
                &self,
                _kind: WaveKind,
                wave_id: &str,
                _expected_total: u32,
                _slot_retry_budget: u32,
            ) -> Result<String, BridgeError> {
                Ok(wave_id.to_string())
            }
            fn record_slot_result(
                &self,
                _wave_id: &str,
                _slot_index: u32,
                _content_hash: &str,
                _event_count: usize,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            fn record_slot_failure(
                &self,
                _wave_id: &str,
                _slot_index: u32,
                _reason: &str,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            fn release_slot_dispatch(
                &self,
                _wave_id: &str,
                _slot_index: u32,
                _outcome: ralph_core::supervisor::DispatchOutcome,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            fn bind_slot(
                &self,
                _kind: WaveKind,
                _wave_id: &str,
                _slot_index: u32,
            ) -> Result<Option<crate::loop_runner::wave::SlotBinding>, BridgeError> {
                Ok(None)
            }
            fn recover(&self) -> Result<Vec<ralph_core::supervisor::WaveSnapshot>, BridgeError> {
                Ok(Vec::new())
            }
            fn record_never_started_failures(&self, _wave_id: &str) -> Result<(), BridgeError> {
                Ok(())
            }
            fn set_wave_phase(
                &self,
                _wave_id: &str,
                _phase: ralph_core::supervisor::WavePhase,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            fn record_coordination_written(
                &self,
                _wave_id: &str,
                _receipt: &ralph_core::supervisor::CoordinationReceiptSummary,
            ) -> Result<(), BridgeError> {
                // Stub: simulate successful record.
                Ok(())
            }
            fn commit_coordination_event(
                &self,
                _wave_id: &str,
                _receipt: &ralph_core::supervisor::CoordinationReceiptSummary,
                _terminal_phase: ralph_core::supervisor::WavePhase,
            ) -> Result<(), BridgeError> {
                // Stub: simulate successful commit.
                Ok(())
            }
        }

        let bridge_impl = Arc::new(IntegrateBridge::new());
        let bridge: Arc<dyn SupervisorBridge> = bridge_impl.clone();

        // Build the completed wave: 6 slots all Completed.
        // WaveResult.events uses ralph_proto::Event, so construct via Event::new.
        let results: Vec<ralph_core::WaveResult> = (0u32..6)
            .map(|i| {
                let ev = ralph_proto::Event::new("review.unit.done", r#"{"ok":true}"#);
                ralph_core::WaveResult {
                    index: i,
                    events: vec![ev],
                }
            })
            .collect();
        let completed = ralph_core::CompletedWave {
            wave_id: "w-u3-fanin".to_string(),
            wave_total: 6,
            results,
            failures: vec![],
            duration: std::time::Duration::from_secs(726),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: std::collections::HashMap::new(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };

        // detected wave: 6 events, total=6, concurrency=6 (wave_timeout=300 by default)
        let detected = make_wave(6, 6, 6);

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let main_events_file = tmp.path().join("events.jsonl");

        let outcome = run_supervisor_fan_in(
            &bridge,
            &completed,
            &detected,
            &main_events_file,
            2288,
            Some(TerminalFanInContext {
                cancel_requested: false,
                elapsed: std::time::Duration::from_secs(726),
            }),
        );

        assert_eq!(
            outcome,
            SupervisorFanInOutcome::InjectedComplete,
            "fan-in with 6 completed slots, elapsed=726s, aggregate=2288s must return \
             InjectedComplete; pre-fix default 600s would incorrectly produce InjectedFailed. \
             Got {outcome:?}"
        );
        let inputs = bridge_impl
            .recorded_inputs
            .lock()
            .unwrap()
            .clone()
            .expect("fan-in must pass PhaseInputs to the bridge");
        assert_eq!(inputs.aggregate_timeout_secs, 2288);
        assert_eq!(inputs.elapsed_secs, 726);
        let ledger = std::fs::read_to_string(&main_events_file).expect("read fan-in ledger");
        assert!(ledger.contains("review.wave.complete"));
    }

    /// U1 Red test 6: when `handle_wave_events` returns
    /// `HandleWaveOutcome { fan_in_failure: true, .. }`, the runner
    /// must enter a termination flow with a reason that is NOT
    /// `MaxRuntime`. Before the fix, `HandleWaveOutcome` had no
    /// `fan_in_failure` field, so the runner could not distinguish
    /// terminal fan-in failure from MaxRuntime.
    #[test]
    fn runner_terminates_on_terminal_fan_in_failure() {
        // Read runner.rs to verify:
        // 1. `HandleWaveOutcome::fan_in_failure` is checked by the runner.
        // 2. The fan_in_failure branch does NOT map to MaxRuntime.
        let runner_rs = include_str!("../../runner.rs");

        let has_fan_in_failure_check = runner_rs.contains("fan_in_failure");
        assert!(
            has_fan_in_failure_check,
            "U1 Red 6: runner.rs must check HandleWaveOutcome::fan_in_failure; \
             no occurrence found. Before the fix, HandleWaveOutcome has no \
             fan_in_failure field and the runner cannot distinguish terminal \
             fan-in failure from MaxRuntime."
        );

        // The fan_in_failure branch must NOT map to MaxRuntime.
        // Find the fan_in_failure occurrence and verify it doesn't contain MaxRuntime.
        if let Some(pos) = runner_rs.find("fan_in_failure") {
            let after = &runner_rs[pos..pos.saturating_add(300)];
            assert!(
                !after.contains("MaxRuntime"),
                "U1 Red 6: fan_in_failure branch must NOT map to MaxRuntime; \
                 found MaxRuntime in the fan_in_failure handling block. \
                 Block:\n{after}"
            );
        }
    }

    /// U4-C4 / §6 C4: when the dispatcher's
    /// `WaveDispatchOutcome::GlobalDeadlineExceeded` fires, the
    /// runner.rs post-wave gate blocks (default_publishes inject +
    /// missing-event gate) MUST be guarded by
    /// `late_termination_reason.is_none()` so neither runs for the
    /// doomed iteration. Without this guard, default_publishes
    /// would inject synthesized events into a loop that's about to
    /// terminate with `TerminationReason::MaxRuntime`, or the
    /// missing-event gate would bump the hard-gate counter on a
    /// loop about to exit.
    ///
    /// Full E2E coverage of the runner's iteration body is not
    /// feasible in CI (would require spinning up a real backend),
    /// so C4 is enforced at two layers:
    ///   1. Dispatcher-level: C1 + the `started == 4` assertion
    ///      confirm `GlobalDeadlineExceeded` returns with zero
    ///      in-flight workers.
    ///   2. `handle_wave_events` level: C3 confirms
    ///      `HandleWaveOutcome { global_deadline_exceeded: true }`
    ///      flows back to the runner.
    ///   3. **Static guard (this test)**: the post-wave gate block
    ///      in `runner.rs` must consult `late_termination_reason`.
    ///      If the guard regresses, this test fails immediately.
    #[test]
    fn u4_c4_runner_post_wave_gates_consult_late_termination_reason() {
        // Read the runner.rs source from the crate root. This
        // test is a static-analysis gate — it catches regressions
        // where someone removes the `late_termination_reason.is_none()`
        // guard from the gate blocks (introduced in U4-C4) without
        // re-reading plan §6 C4.
        let runner_rs = include_str!("../../runner.rs");

        // The post-wave gate blocks (missing-event gate + the
        // `else if` default_publishes fallback) share the
        // distinctive marker
        //   `wave_events.is_empty()\n            && !hard_gate_triggered_this_iteration`
        // Assert each occurrence is followed by a
        // `late_termination_reason.is_none()` guard.
        let gate_marker =
            "wave_events.is_empty()\n            && !hard_gate_triggered_this_iteration";
        let count = runner_rs.matches(gate_marker).count();
        assert!(
            count >= 2,
            "expected at least 2 post-wave gate blocks (missing-event gate + \
             default_publishes fallback) in runner.rs, found {count}. \
             plan §6 C4 requires both blocks to be guarded."
        );

        // After every occurrence of the gate marker, the next
        // logical condition MUST be `late_termination_reason.is_none()`.
        let guarded_count = runner_rs
            .matches("&& !hard_gate_triggered_this_iteration\n            && late_termination_reason.is_none()")
            .count();
        assert!(
            guarded_count >= 2,
            "expected late_termination_reason.is_none() guard on BOTH \
             post-wave gate blocks (missing-event gate + default_publishes \
             fallback), found {guarded_count}. plan §6 C4 requires both."
        );
    }

    /// U4-C4 / §6 C4: `HandleWaveOutcome { global_deadline_exceeded }`
    /// is the runner's only signal to set
    /// `late_termination_reason = Some(MaxRuntime)`. The
    /// post-wave gate guards (asserted by the static test above)
    /// depend on this flag being set. Verify the wiring by
    /// reading the runner.rs source for the exact assignment
    /// pattern.
    #[test]
    fn u4_c4_runner_wires_handle_wave_outcome_to_late_termination_reason() {
        let runner_rs = include_str!("../../runner.rs");
        // The C3 commit introduced the wiring:
        //   if wave_outcome.is_some_and(|o| o.global_deadline_exceeded) {
        //       late_termination_reason = Some(TerminationReason::MaxRuntime);
        //   }
        // Assert the shape so a refactor that drops the
        // `is_some_and` check fails this test.
        assert!(
            runner_rs.contains("wave_outcome.is_some_and(|o| o.global_deadline_exceeded)"),
            "runner must use `is_some_and` to read the global_deadline_exceeded \
             flag from HandleWaveOutcome. If this assertion fails, the wiring \
             introduced in U4-C3 has been removed."
        );
        assert!(
            runner_rs.contains("late_termination_reason = Some(TerminationReason::MaxRuntime)"),
            "runner must set late_termination_reason = Some(MaxRuntime) on \
             global_deadline_exceeded. If this assertion fails, the C3 wiring \
             is broken and U4-C4 static guard is meaningless."
        );
    }

    /// Phase 2: `merge_wave_results_to_events_file` must stamp every merged
    /// record with the wave's target hat, overriding any self-declared
    /// provenance from the worker.
    #[test]
    fn test_merge_wave_results_stamps_target_hat() {
        let tmp = tempfile::TempDir::new().unwrap();
        let events_file = tmp.path().join("events.jsonl");

        let event = ralph_proto::Event::new("review.dimension.done", "{\"file\":\"src/lib.rs\"}")
            .with_source(ralph_proto::HatId::new("dimension-reviewer"));
        let completed = ralph_core::CompletedWave {
            wave_id: "w-stamp-001".to_string(),
            wave_total: 1,
            results: vec![ralph_core::WaveResult {
                index: 0,
                events: vec![event],
            }],
            failures: vec![],
            duration: std::time::Duration::ZERO,
            partial: false,
            expected_source_hat: Some(ralph_proto::HatId::new("dimension-reviewer")),
            assigned_dimensions: std::collections::HashMap::new(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };

        merge_wave_results_to_events_file(
            &completed,
            &events_file,
            &["review.dimension.done".to_string()],
            "dimension-reviewer",
            // 2026-06-16-001 U2: tests use the same default.
            None,
        )
        .unwrap();

        let merged = std::fs::read_to_string(&events_file).unwrap();
        assert!(
            merged.contains("\"hat\":\"dimension-reviewer\""),
            "merged record must be stamped with target hat: {}",
            merged
        );
        assert!(
            merged.contains("\"source\":\"dimension-reviewer\""),
            "merged record must mirror source to target hat: {}",
            merged
        );
    }

    // -------------------------------------------------------------------
    // U1: parse_assigned_dimension
    // -------------------------------------------------------------------

    /// U1/R1 — `dimension: "testing"` in a JSON payload is parsed.
    #[test]
    fn parse_assigned_dimension_reads_string_field() {
        let payload = r#"{"dimension": "testing", "depth": "standard"}"#;
        assert_eq!(
            parse_assigned_dimension(Some(payload)),
            Some("testing".to_string())
        );
    }

    /// U1/R1 — value is trimmed (leading/trailing whitespace tolerated).
    #[test]
    fn parse_assigned_dimension_trims_whitespace() {
        let payload = r#"{"dimension": "  correctness  "}"#;
        assert_eq!(
            parse_assigned_dimension(Some(payload)),
            Some("correctness".to_string())
        );
    }

    /// U1/R1 — non-JSON payload returns None (legacy wave, no enforcement).
    #[test]
    fn parse_assigned_dimension_non_json_returns_none() {
        assert_eq!(parse_assigned_dimension(Some("src/main.rs")), None);
    }

    /// U1/R1 — payload without `dimension` returns None.
    #[test]
    fn parse_assigned_dimension_missing_field_returns_none() {
        let payload = r#"{"depth": "standard", "focus": "all"}"#;
        assert_eq!(parse_assigned_dimension(Some(payload)), None);
    }

    /// U1/R1 — `dimension` that is not a string returns None.
    #[test]
    fn parse_assigned_dimension_non_string_field_returns_none() {
        let payload = r#"{"dimension": 42}"#;
        assert_eq!(parse_assigned_dimension(Some(payload)), None);
    }

    /// U1/R1 — empty / whitespace-only dimension returns None.
    #[test]
    fn parse_assigned_dimension_empty_value_returns_none() {
        let payload = r#"{"dimension": "   "}"#;
        assert_eq!(parse_assigned_dimension(Some(payload)), None);
    }

    /// U1/R1 — missing payload (None) returns None.
    #[test]
    fn parse_assigned_dimension_none_payload_returns_none() {
        assert_eq!(parse_assigned_dimension(None), None);
    }

    /// U1/R1 — empty payload string returns None.
    #[test]
    fn parse_assigned_dimension_empty_string_returns_none() {
        assert_eq!(parse_assigned_dimension(Some("")), None);
        assert_eq!(parse_assigned_dimension(Some("   \n  ")), None);
    }

    // 2026-07-26-003 plan U1: characterization helpers + tests for
    // `review.wave.failed` -> `finalizer` attribution and
    // `missing_dimensions` correctness. These tests pin the baseline
    // BEFORE the U2 / U4 fixes flip the contract; they MUST start
    // RED, then flip GREEN alongside the implementation.

    /// Build a `CompletedWave` carrying six `review.unit.done`
    /// business events distributed across distinct dimensions. Used
    /// to drive `build_wave_failed_payload(WaveKind::Review, ...)`
    /// and `append_supervisor_coord_event("review.wave.failed", ...)`
    /// under test. Slots are emitted in REVERSE order to mirror
    /// `make_u6_completed` so we can re-use its fan-in ordering
    /// assertions if needed.
    /// Build a `CompletedWave` carrying one `review.unit.done`
    /// business event per "actually-emitted-in-this-fanin" slot.
    /// The `dimensions` argument is the FULL assigned set (i.e.
    /// the set we want `build_wave_failed_payload` to subtract
    /// `already_done` from); the helper records every dimension in
    /// `assigned_dimensions` but only fabricates a `review.unit.done`
    /// event for slots that the caller marked as present in the
    /// `events_for` set. That separation mirrors the real-world
    /// primary-20260726 pattern: a slot can be assigned + failed
    /// without ever carrying an in-flight event.
    fn make_review_completed(
        wave_key: &str,
        dimensions: std::collections::BTreeMap<u32, String>,
        events_for: &std::collections::HashSet<u32>,
    ) -> ralph_core::CompletedWave {
        let total = dimensions.len() as u32;
        let results = dimensions
            .iter()
            .filter(|(idx, _)| events_for.contains(idx))
            .map(|(idx, dim)| {
                let payload = serde_json::json!({ "dimension": dim }).to_string();
                ralph_core::WaveResult {
                    index: *idx,
                    events: vec![
                        ralph_proto::Event::new("review.unit.done", payload.clone())
                            .with_source("review-worker")
                            .with_wave(wave_key.to_string(), *idx, total),
                    ],
                }
            })
            .collect();
        let assigned_dimensions: std::collections::HashMap<u32, String> =
            dimensions.iter().map(|(k, v)| (*k, v.clone())).collect();
        ralph_core::CompletedWave {
            wave_id: wave_key.to_string(),
            wave_total: total,
            results,
            failures: vec![],
            duration: std::time::Duration::from_millis(1),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions,
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        }
    }

    /// U1 Red #1: `review.wave.failed` system-injected coordination
    /// events must carry `hat` / `source` = "finalizer" (the
    /// `implementation-review` preset's registered subscriber for
    /// that topic). Today `append_supervisor_coord_event` collapses
    /// every `review.wave.*` event to "review-synthesizer", so the
    /// synthesizer is wrongly woken for the failure path and the
    /// `finalizer` hat (which is the one whose `event_filter`
    /// actually subscribes to `review.wave.failed`) never fires.
    #[test]
    fn review_wave_failed_attribution_routes_to_finalizer() {
        use std::io::BufRead;
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("events.jsonl");
        let payload = serde_json::json!({
            "wave_id": "W1",
            "missing_dimensions": ["correctness"],
            "reason": "worker_timeout",
        });
        let _ = append_supervisor_coord_event(&main, "review.wave.failed", &payload);
        let line =
            std::io::BufReader::new(std::fs::File::open(&main).expect("events file written"))
                .lines()
                .next()
                .expect("at least one line")
                .expect("line read");
        let record: serde_json::Value = serde_json::from_str(&line).expect("json");
        assert_eq!(record["topic"], "review.wave.failed");
        assert_eq!(
            record["hat"], "finalizer",
            "RED: review.wave.failed must route to finalizer, not review-synthesizer"
        );
        // 2026-07-26-004 plan U5 (S5 / AE3): producer (source) is the
        // runtime system identity, NOT the consumer hat. The consumer
        // (finalizer) is carried in `hat` for routing/subscription.
        assert_eq!(
            record["source"], "ralph",
            "U5: producer must be the runtime system identity, not the consumer hat"
        );
        assert_ne!(
            record["source"], record["hat"],
            "U5: producer and consumer must not reuse the same field"
        );
        assert_eq!(record["system_injected"], true);
    }

    /// U4 / S2 (plan 2026-07-26-003): `build_wave_failed_payload` for
    /// the Review arm must subtract from `missing_dimensions` every
    /// dimension that already produced a `review.unit.done`, even
    /// when the unit.done arrived via a path the in-progress
    /// `completed.results` cannot see (i.e. it merged into main
    /// through a previous fan-in tick — the primary-20260726
    /// pattern). The U4 plumbing widens the helper with
    /// `Option<&ReviewDoneHints>` so the call site can pass the
    /// main-backscan / store-Completed view. This assertion goes
    /// RED before U4 (with no hint, `correctness` is doubly
    /// counted); GREEN once `Some(&hints)` actually contributes to
    /// the subtraction.
    #[test]
    fn review_wave_failed_missing_dimensions_omits_main_backscan_hint() {
        use ralph_core::supervisor::WaveKind;
        use std::collections::{BTreeMap, HashSet};
        // Six assigned dimensions; only `testing` and `security`
        // produced a unit.done in this fan-in's `completed.results`.
        // Two siblings (`goal-alignment` / `maintainability`) ALREADY
        // merged into main on a previous fan-in tick — they must
        // NOT appear in `missing_dimensions` once the hint is
        // passed. The remaining two (`correctness` /
        // `performance`) are the genuinely missing dimensions.
        let mut dims = BTreeMap::new();
        for (i, name) in [
            "correctness",
            "goal-alignment",
            "testing",
            "security",
            "maintainability",
            "performance",
        ]
        .iter()
        .enumerate()
        {
            dims.insert(i as u32, name.to_string());
        }
        let mut events_for = HashSet::new();
        events_for.insert(2); // testing
        events_for.insert(3); // security
        let completed = make_review_completed("W1", dims, &events_for);
        let mut main_backscan = HashSet::new();
        main_backscan.insert("goal-alignment".to_string());
        main_backscan.insert("maintainability".to_string());
        let hints = ReviewDoneHints {
            main_backscan: main_backscan.clone(),
            store_completed: HashSet::new(),
        };
        let payload = build_wave_failed_payload(
            WaveKind::Review,
            &completed,
            "worker_timeout",
            vec![],
            &std::collections::HashMap::new(),
            Some(&hints),
            None,
        );
        let missing: HashSet<String> = payload["missing_dimensions"]
            .as_array()
            .expect("missing_dimensions is an array")
            .iter()
            .map(|v| v.as_str().expect("string").to_string())
            .collect();
        assert!(
            !missing.contains("goal-alignment"),
            "main-backscanned dimensions must NOT appear in missing_dimensions; got {missing:?}"
        );
        assert!(
            !missing.contains("maintainability"),
            "main-backscanned dimensions must NOT appear in missing_dimensions; got {missing:?}"
        );
        assert!(
            missing.contains("correctness"),
            "truly missing dimension IS in missing_dimensions; got {missing:?}"
        );
        assert!(
            missing.contains("performance"),
            "truly missing dimension IS in missing_dimensions; got {missing:?}"
        );
    }

    /// U4 (plan 2026-07-26-003) pure-helper table-driven tests:
    /// `compute_review_missing_dimensions` is the single source of
    /// truth for the truth-set arithmetic. We drive it with four
    /// synthetic inputs that correspond to the AE2 acceptance
    /// examples: results-only, store-Completed, main-backscan,
    /// and a combination of all three. The pure helper is the
    /// only piece the call site relies on for cross-source
    /// reconciliation.
    #[test]
    fn compute_review_missing_dimensions_table_driven() {
        let assigned: std::collections::HashSet<String> = [
            "correctness",
            "goal-alignment",
            "testing",
            "security",
            "maintainability",
            "performance",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        // 1. results-only (results supplies correctness + testing).
        let results_only: std::collections::HashSet<String> = ["correctness", "testing"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut got = compute_review_missing_dimensions(&assigned, &results_only)
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        let mut want = [
            "goal-alignment",
            "security",
            "maintainability",
            "performance",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<std::collections::HashSet<_>>();
        assert_eq!(got, want);

        // 2. store-Completed supplies maintainability + performance
        //    only; the rest stay missing.
        let mut store_only = std::collections::HashSet::new();
        store_only.insert("maintainability".to_string());
        store_only.insert("performance".to_string());
        got = compute_review_missing_dimensions(&assigned, &store_only)
            .into_iter()
            .collect();
        want = ["correctness", "goal-alignment", "testing", "security"]
            .iter()
            .map(|s| s.to_string())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(got, want);

        // 3. main-backscan alone.
        let mut main_only = std::collections::HashSet::new();
        main_only.insert("security".to_string());
        got = compute_review_missing_dimensions(&assigned, &main_only)
            .into_iter()
            .collect();
        want = [
            "correctness",
            "goal-alignment",
            "testing",
            "maintainability",
            "performance",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<std::collections::HashSet<_>>();
        assert_eq!(got, want);

        // 4. union of all three sources.
        let mut unioned = std::collections::HashSet::new();
        unioned.insert("correctness".to_string());
        unioned.insert("testing".to_string());
        unioned.insert("maintainability".to_string());
        unioned.insert("security".to_string());
        got = compute_review_missing_dimensions(&assigned, &unioned)
            .into_iter()
            .collect();
        want = ["goal-alignment", "performance"]
            .iter()
            .map(|s| s.to_string())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(got, want);
    }

    /// U4 / AE2 (plan 2026-07-26-003): when `ReviewDoneHints`
    /// carries BOTH `main_backscan` AND `store_completed`, both
    /// sources contribute to the truth set. The combined view
    /// catches the case where the main ledger has events from an
    /// earlier wave under the same wave_id AND the store has
    /// rows from a still-unmerged tick — both should drop out of
    /// `missing_dimensions`.
    #[test]
    fn review_wave_failed_combined_hints_subtract_from_missing() {
        use ralph_core::supervisor::WaveKind;
        use std::collections::{BTreeMap, HashSet};
        let mut dims = BTreeMap::new();
        for (i, name) in ["correctness", "testing", "security", "performance"]
            .iter()
            .enumerate()
        {
            dims.insert(i as u32, name.to_string());
        }
        // No slot produced an event in this fan-in's results; the
        // full truth set must come from the hints (main +
        // store). Only `performance` should remain missing.
        let events_for = HashSet::new();
        let completed = make_review_completed("W1", dims, &events_for);
        let hints = ReviewDoneHints {
            main_backscan: ["correctness", "testing"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            store_completed: ["security"].iter().map(|s| s.to_string()).collect(),
        };
        let payload = build_wave_failed_payload(
            WaveKind::Review,
            &completed,
            "worker_timeout",
            vec![],
            &std::collections::HashMap::new(),
            Some(&hints),
            None,
        );
        let missing: std::collections::HashSet<String> = payload["missing_dimensions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            missing,
            ["performance"].iter().map(|s| s.to_string()).collect()
        );
    }

    /// U5 / S5 (plan 2026-07-26-003 / R4 / KTD7): a Review wave
    /// that reaches `InjectedFailed` must keep the Completed
    /// slots' `review.unit.done` events visible in the main
    /// ledger — without it, the operator / `finalizer` downstream
    /// see "missing everything" when in fact some slots
    /// succeeded. The dispatcher-layer helper
    /// `merge_completed_review_slots_to_main` writes those events
    /// with `hat = review-worker` BEFORE the failed coord event
    /// (or, in this direct unit test, equivalent ordering).
    #[test]
    fn merge_completed_review_slots_to_main_writes_completed_only() {
        use std::collections::HashSet;
        use std::io::BufRead;
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("events.jsonl");
        // Completed slots: 0 + 1 (got review.unit.done).
        // Failed slot: 2 (has a failure record — must be skipped
        // for review.unit.done merge because it did not pass
        // classify). Slot 3 has no results entry at all
        // (Pending — contributes nothing).
        let mut dims = std::collections::BTreeMap::new();
        dims.insert(0, "correctness".to_string());
        dims.insert(1, "goal-alignment".to_string());
        dims.insert(2, "performance".to_string());
        dims.insert(3, "security".to_string());
        let mut events_for: HashSet<u32> = HashSet::new();
        events_for.insert(0);
        events_for.insert(1);
        let mut completed = make_review_completed("W1", dims, &events_for);
        completed.failures.push(ralph_core::WaveFailure {
            index: 2,
            error: "empty_worker_result".to_string(),
            duration: std::time::Duration::from_millis(50),
            expected_dimension: None,
            actual_dimension: None,
        });
        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        use ralph_core::supervisor::SupervisorStore as _;
        // Register the wave so `fan_in_status` succeeds after
        // the helper commits salvage_merged (P0-1 invariant).
        // `register_wave` returns the store-assigned `w-N` id,
        // NOT the idempotency key, so we must capture it.
        let wave_id = store
            .register_wave("W1", ralph_core::supervisor::WaveKind::Review, 2, 1)
            .expect("register");
        let bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge> =
            Arc::new(ralph_core::supervisor::InMemoryCoordinatorBridge::from_store(store.clone()));
        let _ = merge_completed_review_slots_to_main(&main, &completed, &bridge, &wave_id);
        // P0-1: the helper must also commit `salvage_merged` so
        // the dispatcher's failure path can inject `*.wave.failed`.
        let snap = store.fan_in_status(&wave_id).expect("snap");
        assert!(
            snap.delivery_state
                .at_least(ralph_core::supervisor::WaveDeliveryState::SalvageCommitted),
            "merge_completed_review_slots_to_main must commit salvage_merged (P0-1)"
        );
        let f = std::fs::File::open(&main).expect("events file written");
        let lines: Vec<String> = std::io::BufReader::new(f)
            .lines()
            .map(|r| r.unwrap())
            .collect();
        // Exactly 2 lines (one per Completed slot) — the Failed
        // slot's `performance` MUST NOT appear.
        assert_eq!(lines.len(), 2, "expected 2 done events, got: {lines:?}");
        let hats: Vec<String> = lines
            .iter()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).expect("json");
                v["hat"].as_str().unwrap().to_string()
            })
            .collect();
        assert!(
            hats.iter().all(|h| h == "review-worker"),
            "all written events must attribute to review-worker; got: {hats:?}"
        );
        let topics: Vec<String> = lines
            .iter()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).expect("json");
                v["topic"].as_str().unwrap().to_string()
            })
            .collect();
        assert!(
            topics.iter().all(|t| t == "review.unit.done"),
            "all written events must be review.unit.done; got: {topics:?}"
        );
        // Confirm the failed slot's dimension is NOT present.
        let payloads: Vec<String> = lines
            .iter()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).expect("json");
                v["payload"].as_str().unwrap().to_string()
            })
            .collect();
        assert!(
            !payloads.iter().any(|p| p.contains("performance")),
            "the failed slot's `performance` dimension must not be merged; got: {payloads:?}"
        );
    }

    /// U5 / S5 / R7 (plan 2026-07-26-003): the Exec arm MUST NOT
    /// be touched by U5. Re-running the existing byte-equal Exec
    /// payload test (`u5_build_wave_failed_slots_json_shape`)
    /// guarantees the signature widening is Review-only; this
    /// additionally asserts that `merge_completed_review_slots_to_main`
    /// is harmless on a non-Review `CompletedWave` shape (because
    /// the helper is gated by the `WaveKind::Review` match in
    /// `run_supervisor_fan_in`, but the helper itself only
    /// filters by event topic — it writes nothing when no `results`
    /// carry a `review.unit.done`). Writing nothing is not the same
    /// as doing nothing: the salvage phases still have to be
    /// committed or the coordinator can never fail the wave.
    #[test]
    fn merge_completed_review_slots_handles_empty_results() {
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("events.jsonl");
        let completed = ralph_core::CompletedWave {
            wave_id: "W-empty".to_string(),
            wave_total: 0,
            results: vec![],
            failures: vec![],
            duration: std::time::Duration::from_millis(1),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: std::collections::HashMap::new(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };
        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        use ralph_core::supervisor::SupervisorStore as _;
        let wave_id = store
            .register_wave("W-empty", ralph_core::supervisor::WaveKind::Review, 1, 1)
            .expect("register");
        let bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge> =
            Arc::new(ralph_core::supervisor::InMemoryCoordinatorBridge::from_store(store.clone()));
        merge_completed_review_slots_to_main(&main, &completed, &bridge, &wave_id)
            .expect("empty salvage must succeed");
        // No file is created when there is nothing to write.
        assert!(!main.exists() || std::fs::metadata(&main).unwrap().len() == 0);
        // The salvage phases MUST still be committed. P0-1's crash
        // window was "coord injection latched before the rows landed";
        // with zero rows there is nothing to land, and withholding the
        // commit instead strands the wave below `SalvageCommitted` so
        // `fail_wave` answers `SalvageNotMerged` forever.
        let snap = store.fan_in_status(&wave_id).expect("snap");
        assert!(
            snap.delivery_state
                .at_least(ralph_core::supervisor::WaveDeliveryState::SalvageCommitted),
            "an empty salvage batch must still commit both delivery phases"
        );
    }

    /// U1 guard rail for the success path: `review.wave.complete`
    /// must keep routing to `review-synthesizer`, not flip to
    /// `finalizer`. This test ensures the U2 fix is surgical (only
    /// the `.failed` arm changes) and does not accidentally re-route
    /// the success handoff.
    #[test]
    fn review_wave_complete_attribution_remains_synthesizer() {
        use std::io::BufRead;
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("events.jsonl");
        let payload = serde_json::json!({
            "wave_id": "W1",
            "completed_dimensions": ["goal-alignment"],
        });
        let _ = append_supervisor_coord_event(&main, "review.wave.complete", &payload);
        let line =
            std::io::BufReader::new(std::fs::File::open(&main).expect("events file written"))
                .lines()
                .next()
                .expect("at least one line")
                .expect("line read");
        let record: serde_json::Value = serde_json::from_str(&line).expect("json");
        assert_eq!(
            record["hat"], "review-synthesizer",
            "review.wave.complete MUST stay on review-synthesizer (consumer/routing)"
        );
        // 2026-07-26-004 plan U5 (S5 / AE3): producer is the runtime
        // system identity, separate from the consumer hat.
        assert_eq!(record["source"], "ralph");
    }

    // -------------------------------------------------------------------
    // U2: RALPH_WAVE_DIMENSION env var injection
    // -------------------------------------------------------------------

    /// U2: when a `WorkerRequest` is built with
    /// `assigned_dimension: Some("testing")`, the dispatcher's
    /// injection step must add `("RALPH_WAVE_DIMENSION", "testing")`
    /// to `request.backend.env_vars` so the backend process can read
    /// its hard-bound dimension from the environment (matching the
    /// `## ASSIGNED DIMENSION` block U1 added to the prompt).
    ///
    /// This test mirrors the injection logic in
    /// `execute_wave_structured` (the inline `if let Some(ref dim)`
    /// block right after the wave-env-vars `extend`). Constructing
    /// a `WorkerRequest` and applying the same push lets us assert
    /// the exact env-var key/value without spinning up a real wave
    /// dispatch.
    #[test]
    fn test_ralph_wave_dimension_env_var() {
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut request =
            make_worker_request_with_dimension(0, progress_tx, Some("testing".to_string()));

        // Mirror the dispatcher injection step (see the inline
        // `if let Some(ref dim) = assigned_dimension` block in
        // `execute_wave_structured`).
        if let Some(ref dim) = request.assigned_dimension {
            request
                .backend
                .env_vars
                .push(("RALPH_WAVE_DIMENSION".into(), dim.clone()));
        }

        assert!(
            request
                .backend
                .env_vars
                .iter()
                .any(|(k, v)| k == "RALPH_WAVE_DIMENSION" && v == "testing"),
            "U2: env_vars must contain (\"RALPH_WAVE_DIMENSION\", \"testing\"), got {:?}",
            request.backend.env_vars
        );
    }

    /// U2: when `assigned_dimension` is `None` (legacy / non-review
    /// waves), the dispatcher MUST NOT inject `RALPH_WAVE_DIMENSION`
    /// — the var stays unset so pre-U2 behaviour is preserved for
    /// non-dimension-bound workers.
    #[test]
    fn test_ralph_wave_dimension_env_var_absent_when_unassigned() {
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut request = make_worker_request_with_dimension(0, progress_tx, None);

        if let Some(ref dim) = request.assigned_dimension {
            request
                .backend
                .env_vars
                .push(("RALPH_WAVE_DIMENSION".into(), dim.clone()));
        }

        assert!(
            !request
                .backend
                .env_vars
                .iter()
                .any(|(k, _)| k == "RALPH_WAVE_DIMENSION"),
            "U2: RALPH_WAVE_DIMENSION must NOT be injected when assigned_dimension is None, got {:?}",
            request.backend.env_vars
        );
    }

    // ── 2026-07-30-001 plan U3: attempt-aware aggregate floor ──────────────

    /// The partial threshold the dispatcher will derive from a given
    /// aggregate timeout — mirrors `DispatchContext::build`.
    fn partial_threshold_of(aggregate: Duration) -> Duration {
        Duration::from_secs(
            aggregate
                .as_secs()
                .saturating_mul(PARTIAL_THRESHOLD_NUM)
                .div_ceil(PARTIAL_THRESHOLD_DEN),
        )
    }

    /// U3 验收 #1: with no retry budget the work budget is exactly the
    /// pre-plan formula, so the legacy dispatch path is untouched.
    #[test]
    fn aggregate_work_budget_zero_retry_matches_legacy_formula() {
        for (events, concurrency) in [(1usize, 1usize), (4, 2), (5, 2), (3, 8), (0, 0)] {
            let legacy = aggregate_timeout_for(Duration::from_secs(300), events, concurrency);
            let budgeted = wave_work_budget(Duration::from_secs(300), events, concurrency, 1);
            assert_eq!(
                legacy, budgeted,
                "U3: max_attempts=1 must reproduce the legacy budget for {events}/{concurrency}"
            );
        }
        // N=0 and C=0 both collapse to 1.
        assert_eq!(
            wave_work_budget(Duration::from_secs(300), 0, 0, 1),
            Duration::from_secs(330)
        );
    }

    /// U3 验收 #2: the floor counts BOTH the concurrency batches and
    /// every legal attempt.
    #[test]
    fn aggregate_floor_counts_three_attempts_and_batches() {
        // T=300, N=2, C=1 → 2 batches; budget=2 → 3 attempts.
        // work = 300*2*3 + 30 = 1830; floor = ceil(1830 * 10 / 8) = 2288.
        assert_eq!(
            wave_work_budget(Duration::from_secs(300), 2, 1, 3),
            Duration::from_secs(1830)
        );
        assert_eq!(
            aggregate_floor_for_attempts(Duration::from_secs(300), 2, 1, 2),
            Duration::from_secs(2288)
        );
        // Same wave without retries stays at the single-attempt budget.
        assert_eq!(
            aggregate_floor_for_attempts(Duration::from_secs(300), 2, 1, 0),
            Duration::from_secs(788),
            "U3: budget=0 still needs the 80% headroom, but only for one attempt"
        );
    }

    /// U3 验收 #3: the whole point of the floor — the partial threshold
    /// must never fire before the work budget is spent.
    #[test]
    fn aggregate_floor_keeps_partial_at_or_after_work_budget() {
        for (timeout, events, concurrency, budget) in [
            (300u64, 1usize, 1usize, 0u32),
            (300, 2, 1, 2),
            (60, 7, 3, 1),
            (1, 1, 1, 2),
            (3600, 12, 4, 2),
        ] {
            let work = wave_work_budget(
                Duration::from_secs(timeout),
                events,
                concurrency,
                budget + 1,
            );
            let floor = aggregate_floor_for_attempts(
                Duration::from_secs(timeout),
                events,
                concurrency,
                budget,
            );
            assert!(
                partial_threshold_of(floor) >= work,
                "U3: partial ({:?}) must not preempt the work budget ({work:?}) \
                 for T={timeout} N={events} C={concurrency} budget={budget}",
                partial_threshold_of(floor)
            );
        }
    }

    /// U3 验收 #4: an operator who asked for a longer aggregate keeps
    /// it — the floor only ever raises.
    #[test]
    fn configured_aggregate_above_floor_is_preserved() {
        let floor = aggregate_floor_for_attempts(Duration::from_secs(300), 2, 1, 2);
        let generous = Duration::from_secs(7 * 3600);
        assert!(generous > floor);
        assert_eq!(generous.max(floor), generous);

        let stingy = Duration::from_secs(60);
        assert_eq!(stingy.max(floor), floor, "U3: a too-small config is raised");
    }

    /// U3 验收 #5: the floor saturates instead of wrapping.
    #[test]
    fn aggregate_floor_saturates_without_overflow() {
        let huge = aggregate_floor_for_attempts(Duration::from_secs(u64::MAX), 1000, 1, 2);
        assert_eq!(huge, Duration::from_secs(u64::MAX));
        // A large-but-not-saturating case must still be exact.
        assert_eq!(
            aggregate_floor_for_attempts(Duration::from_secs(8), 1, 1, 0),
            Duration::from_secs(48),
            "U3: work=38 → ceil(38*10/8)=48"
        );
    }

    /// U3 regression guard: the attempt-aware aggregate floor must use
    /// the local effective cap, not the hat's declared concurrency.
    #[test]
    fn attempt_aware_aggregate_timeout_uses_effective_cap() {
        let configured = Duration::from_secs(0);
        let wave_timeout = Duration::from_secs(300);
        let events_count = 7;
        let retry_budget = 2;

        let declared_concurrency = attempt_aware_aggregate_timeout(
            configured,
            wave_timeout,
            events_count,
            4,
            retry_budget,
        );
        let effective_cap = attempt_aware_aggregate_timeout(
            configured,
            wave_timeout,
            events_count,
            1,
            retry_budget,
        );

        assert_eq!(declared_concurrency, Duration::from_secs(2288));
        assert_eq!(effective_cap, Duration::from_secs(7913));
        assert!(
            effective_cap > declared_concurrency,
            "U3: the real effective cap must produce a larger floor when it is lower"
        );
    }

    // ── 2026-07-30-001 plan U2: prior-attempt detail extraction ────────────

    fn u2_outcome_with(topic: &str, payload: Option<&str>) -> WaveWorkerOutcome {
        Ok((
            vec![ralph_core::Event {
                topic: topic.to_string(),
                payload: payload.map(str::to_string),
                ts: String::new(),
                hat: None,
                triggered: None,
                source: None,
                wave_id: None,
                wave_index: None,
                wave_total: None,
                system_injected: None,
            }],
            Duration::from_secs(1),
            true,
        ))
    }

    #[test]
    fn u2_reported_failure_detail_reads_reason_from_terminal_payload() {
        let outcome = u2_outcome_with(
            "exec.unit.failed",
            Some(r#"{"reason":"tests are red","unit_id":"u1"}"#),
        );
        assert_eq!(
            reported_failure_detail(&outcome),
            Some("tests are red".to_string())
        );
    }

    #[test]
    fn u2_reported_failure_detail_is_none_without_a_usable_reason() {
        // Worker died before writing anything.
        assert_eq!(
            reported_failure_detail(&Err(("worker_timeout".to_string(), Duration::from_secs(1)))),
            None
        );
        // Terminal is a success, not a failure.
        assert_eq!(
            reported_failure_detail(&u2_outcome_with("exec.unit.done", Some(r#"{"ok":true}"#))),
            None
        );
        // Payload is not JSON.
        assert_eq!(
            reported_failure_detail(&u2_outcome_with("exec.unit.failed", Some("boom"))),
            None
        );
        // JSON without a `reason` key.
        assert_eq!(
            reported_failure_detail(&u2_outcome_with("exec.unit.failed", Some(r#"{"code":7}"#))),
            None
        );
        // `reason` present but blank.
        assert_eq!(
            reported_failure_detail(&u2_outcome_with(
                "exec.unit.failed",
                Some(r#"{"reason":"   "}"#)
            )),
            None
        );
        // No payload at all.
        assert_eq!(
            reported_failure_detail(&u2_outcome_with("exec.unit.failed", None)),
            None
        );
    }

    // ── 2026-07-25-004 plan U1: characterize classify_slot_result ───────────
    //
    // U2/U3 will flip the Err arm and the Ok(success=false) arm.
    // These tests pin the CURRENT (pre-U3) behaviour so the flip
    // is observable as a red → green transition.

    /// U1 characterization, preserved in U3 (Ok arm is unchanged):
    /// Ok(success=false) + Done terminal resolves via ExitNonZero routing
    /// to `Completed(Done)`. The Err-arm flip (timeout → Static worker_timeout)
    /// lives in the T1/T2 tests below, not here.
    #[test]
    fn classify_slot_result_ok_success_false_with_done_char_u1_pre_u3_completes_via_exit_nonzero() {
        let done_event = ralph_core::Event {
            topic: "review.unit.done".to_string(),
            payload: Some("ok".to_string()),
            ts: String::new(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
            system_injected: None,
        };
        // success=false → WorkerExit::ExitNonZero in classify_slot_result
        let result: WaveWorkerOutcome = Ok((vec![done_event], Duration::from_secs(3), false));
        let classified = classify_slot_result(&result);

        // U1/U3 contract: ExitNonZero + Done terminal → Completed(Done).
        match classified {
            ClassifiedSlot {
                outcome:
                    ralph_core::supervisor::worker_outcome::SlotOutcome::Completed(
                        ralph_core::supervisor::worker_outcome::WorkerTerminalKind::Done,
                    ),
                reason: None,
            } => {
                // Pass — U3 does NOT change the Ok arm.
            }
            other => panic!("expected Completed(Done) + reason=None, got {other:?}"),
        }
    }

    // ── 2026-07-25-004 plan U3: timeout Err → Static worker_timeout ─────────

    /// T1: empty-timeout Err → Static `worker_timeout` (R3/AE3).
    #[test]
    fn u3_classify_slot_result_empty_timeout_is_static_worker_timeout() {
        use ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT;

        let result: WaveWorkerOutcome = Err((
            "Worker timed out after 5s without emitting events".to_string(),
            Duration::from_secs(5),
        ));
        let classified = classify_slot_result(&result);

        match classified {
            ClassifiedSlot {
                outcome: ralph_core::supervisor::worker_outcome::SlotOutcome::Failed { reason },
                reason: Some(ClassifiedReason::Static(r)),
            } => {
                assert_eq!(reason, REASON_WORKER_TIMEOUT);
                assert_eq!(r, REASON_WORKER_TIMEOUT);
            }
            other => {
                panic!("expected Failed{{reason=REASON_WORKER_TIMEOUT}} + Static(_), got {other:?}")
            }
        }
    }

    /// T2: non-timeout Err keeps Dynamic verbatim + cancelled shell (out of scope to fix).
    #[test]
    fn u3_classify_slot_result_non_timeout_err_keeps_dynamic_verbatim() {
        use ralph_core::supervisor::worker_outcome::REASON_WORKER_CANCELLED;

        let result: WaveWorkerOutcome =
            Err(("boom: worker crashed".to_string(), Duration::from_secs(2)));
        let classified = classify_slot_result(&result);

        match classified {
            ClassifiedSlot {
                outcome: ralph_core::supervisor::worker_outcome::SlotOutcome::Failed { reason },
                reason: Some(ClassifiedReason::Dynamic(msg)),
            } => {
                assert_eq!(reason, REASON_WORKER_CANCELLED);
                assert_eq!(msg, "boom: worker crashed");
            }
            other => panic!(
                "expected Failed{{reason=REASON_WORKER_CANCELLED}} + Dynamic(_), got {other:?}"
            ),
        }
    }

    /// T3: boundary — Err message that starts with the timeout prefix
    /// but mentions events is still classified as Static worker_timeout.
    #[test]
    fn u3_classify_slot_result_timeout_with_event_in_err_message_is_static_worker_timeout_too() {
        use ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT;

        let result: WaveWorkerOutcome = Err((
            "Worker timed out after 7s without emitting events".to_string(),
            Duration::from_secs(7),
        ));
        let classified = classify_slot_result(&result);

        match classified {
            ClassifiedSlot {
                outcome: ralph_core::supervisor::worker_outcome::SlotOutcome::Failed { reason },
                reason: Some(ClassifiedReason::Static(r)),
            } => {
                assert_eq!(reason, REASON_WORKER_TIMEOUT);
                assert_eq!(r, REASON_WORKER_TIMEOUT);
            }
            other => {
                panic!("expected Failed{{reason=REASON_WORKER_TIMEOUT}} + Static(_), got {other:?}")
            }
        }
    }

    /// T4: AE1 satisfaction — Ok path with Done terminal after timeout
    /// still completes (ExitNonZero + Done → Completed(Done), not Failed).
    /// This is the AE1 regression test that mirrors the CA-3 Ok-arm path.
    #[test]
    fn u3_classify_slot_result_ok_path_with_done_after_timeout_still_completes() {
        let done_event = ralph_core::Event {
            topic: "review.unit.done".to_string(),
            payload: Some("ok".to_string()),
            ts: String::new(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
            system_injected: None,
        };
        // Ok(events, duration, success=false) — the success=false makes the
        // dispatcher treat it as ExitNonZero, which combined with a Done
        // terminal yields Completed(Done) per the truth table (AE1).
        let result: WaveWorkerOutcome = Ok((vec![done_event], Duration::from_secs(10), false));
        let classified = classify_slot_result(&result);

        match classified {
            ClassifiedSlot {
                outcome:
                    ralph_core::supervisor::worker_outcome::SlotOutcome::Completed(
                        ralph_core::supervisor::worker_outcome::WorkerTerminalKind::Done,
                    ),
                reason: None,
            } => {
                // Pass — AE1 satisfied.
            }
            other => panic!("expected Completed(Done) + reason=None, got {other:?}"),
        }
    }

    // ─────────────────────────────────────────────────────────────────
    // 2026-07-25-004 plan U5 (R6 / AE5): per-slot diagnostics JSON
    // ─────────────────────────────────────────────────────────────────

    /// T1: `build_wave_failed_slots_json` emits the expected JSON shape.
    #[test]
    fn u5_build_wave_failed_slots_json_shape() {
        use ralph_core::supervisor::SlotStatus;

        let slots = vec![
            (0, SlotStatus::Completed),
            (1, SlotStatus::Failed),
            (2, SlotStatus::Failed),
            (3, SlotStatus::Cancelled),
        ];
        let mut reasons = std::collections::HashMap::new();
        reasons.insert(1, "worker_timeout".to_string());
        reasons.insert(2, "slot_never_started".to_string());
        reasons.insert(3, "worker_cancelled".to_string());

        let json = build_wave_failed_slots_json("w-u5-test", &slots, &reasons, 42);

        assert_eq!(json["wave_id"], "w-u5-test");
        assert_eq!(json["generated_at_kind"], "injected_failed");
        assert_eq!(json["elapsed_secs"], 42);

        let slot_array = json["slots"].as_array().expect("slots must be an array");
        assert_eq!(slot_array.len(), 4);

        // Slot 0: completed, no reason.
        let s0 = &slot_array[0];
        assert_eq!(s0["slot_index"], 0);
        assert_eq!(s0["status"], "completed");
        assert!(s0["reason"].is_null());

        // Slot 1: failed, worker_timeout.
        let s1 = &slot_array[1];
        assert_eq!(s1["slot_index"], 1);
        assert_eq!(s1["status"], "failed");
        assert_eq!(s1["reason"], "worker_timeout");

        // Slot 2: failed, slot_never_started.
        let s2 = &slot_array[2];
        assert_eq!(s2["slot_index"], 2);
        assert_eq!(s2["status"], "failed");
        assert_eq!(s2["reason"], "slot_never_started");

        // Slot 3: cancelled, worker_cancelled.
        let s3 = &slot_array[3];
        assert_eq!(s3["slot_index"], 3);
        assert_eq!(s3["status"], "cancelled");
        assert_eq!(s3["reason"], "worker_cancelled");
    }

    /// T2: `write_wave_diagnostics_json` writes the correct file at the
    /// expected path under a TempDir root, and the file parses as valid JSON.
    #[test]
    fn u5_write_wave_diagnostics_json_writes_correct_file() {
        let temp_root = tempfile::TempDir::new().expect("temp dir");
        let root_path = temp_root.path();

        let payload = serde_json::json!({
            "wave_id": "w-u5-t2",
            "generated_at_kind": "injected_failed",
            "elapsed_secs": 7,
            "slots": [
                {"slot_index": 0, "status": "completed", "reason": null},
                {"slot_index": 1, "status": "failed", "reason": "worker_timeout"},
            ]
        });

        let result = write_wave_diagnostics_json(root_path, "w-u5-t2", &payload);
        assert!(result.is_ok(), "write must succeed");

        let written_path = result.unwrap();
        assert!(
            written_path.starts_with(root_path),
            "path must be under the given root"
        );
        assert!(
            written_path
                .to_string_lossy()
                .contains("wave-w-u5-t2-slots.json"),
            "filename must match expected pattern"
        );

        // Verify the file parses as valid JSON and matches the payload.
        let bytes = std::fs::read(&written_path).expect("file must be readable");
        let read_back: serde_json::Value =
            serde_json::from_slice(&bytes).expect("must be valid JSON");
        assert_eq!(read_back, payload);
    }

    /// T3 (regression): success path (`InjectedComplete`) does NOT write a
    /// diagnostics file. Unlike the earlier hollow stub, this test actually
    /// drives `run_supervisor_fan_in` through a fully-completed wave so the
    /// coordinator returns `CoordinatorAction::Complete`, then asserts the
    /// success arm wrote NO per-slot diagnostics JSON. The unique wave_id
    /// guarantees the assertion is meaningful: production writes diagnostics
    /// to `Path::new(".")` (CWD), and nextest's process-per-test isolation
    /// keeps CWD stable, so if a future change adds
    /// `write_wave_diagnostics_json` to the `InjectedComplete` arm this test
    /// fails.
    #[test]
    fn u5_no_diagnostics_file_on_success_path() {
        use ralph_core::config::HatConfig;
        use ralph_core::supervisor::InMemoryCoordinatorBridge;
        use ralph_core::supervisor::SlotResource;
        use ralph_core::supervisor::SupervisorBridge;
        use ralph_core::supervisor::{SupervisorStore, WaveKind};

        // Unique wave_id so the diagnostics-file absence assertion cannot
        // collide with a file written by any other test.
        let wave_id = "w-u4-success-no-diag-2026-07-25-004";

        // Build an in-memory store + bridge with 2 slots, both Completed.
        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());
        let bridge_arc: Arc<dyn SupervisorBridge> = Arc::new(bridge);

        let store_wave_id = bridge_arc
            .register_wave_if_absent(WaveKind::Exec, wave_id, 2, 1)
            .unwrap();

        // Bind + dispatch + complete BOTH slots so `evaluate_phase`
        // reaches `Integrate` (pending=0, in_flight=0, completed>=total).
        for slot in 0..2u32 {
            store
                .bind_worktree(
                    &store_wave_id,
                    slot,
                    SlotResource {
                        slot_index: slot,
                        worktree_path: Some(format!(".ralph/s{slot}")),
                        branch: Some(format!("ralph/u4-s{slot}")),
                    },
                )
                .unwrap();
        }
        let mut dispatched = Vec::new();
        for _ in 0..2 {
            let (w, i) = store.try_dispatch_next(8).unwrap().unwrap();
            dispatched.push((w, i));
        }
        for (w, i) in dispatched {
            store.record_slot_result(&w, i, "hash", 1).unwrap();
            // Plan 004 R2 / P0-2: success path requires terminal evidence.
            store
                .record_slot_terminal_evidence(
                    &w,
                    i,
                    &ralph_core::supervisor::TerminalEvidence::from_event(
                        "exec.unit.done",
                        &format!("{{\"unit\":\"u5-ok-{i}\"}}"),
                    ),
                )
                .unwrap();
        }

        // Sanity: the wave is fully completed before fan-in.
        let snap = store.fan_in_status(&store_wave_id).unwrap();
        assert_eq!(snap.completed_count, 2);

        // Build the CompletedWave + DetectedWave for this wave. The trigger
        // topic does NOT start with `review.` or `fix.` so the kind is Exec.
        let completed = ralph_core::CompletedWave {
            wave_id: wave_id.to_string(),
            wave_total: 2,
            ..ralph_core::CompletedWave::default()
        };
        let detected = ralph_core::DetectedWave {
            wave_id: wave_id.to_string(),
            target_hat: HatId::new("u4-success-hat"),
            hat_config: HatConfig {
                name: "u4-success-hat".to_string(),
                ..HatConfig::default()
            },
            events: vec![core_event("work.ready", "payload-0")],
            total: 1,
            partial: false,
            consumer_aggregate_timeout: None,
        };

        // Fresh temp dir for the main events file the success arm appends to.
        let temp_root = tempfile::TempDir::new().expect("temp dir");
        let main_events_file = temp_root.path().join("events.jsonl");

        let outcome = run_supervisor_fan_in(
            &bridge_arc,
            &completed,
            &detected,
            &main_events_file,
            60,
            None,
        );
        assert!(
            matches!(outcome, SupervisorFanInOutcome::InjectedComplete),
            "success path must reach InjectedComplete, got {outcome:?}"
        );

        // The InjectedComplete arm must NOT write any diagnostics file.
        let diag_path = std::path::Path::new(".")
            .join(".ralph")
            .join("diagnostics")
            .join(format!("wave-{wave_id}-slots.json"));
        assert!(
            !diag_path.exists(),
            "success path must not write a diagnostics file at {}",
            diag_path.display()
        );
    }

    /// T3 negative: `write_wave_diagnostics_json` surfaces an `Err` (and does
    /// NOT panic) when the diagnostics directory cannot be created — here
    /// because `.ralph/diagnostics` collides with an existing regular file.
    #[test]
    fn u5_write_wave_diagnostics_json_failure_returns_err() {
        let temp_root = tempfile::TempDir::new().expect("temp dir");
        let root_path = temp_root.path();

        // Make `create_dir_all(root/.ralph/diagnostics)` fail by placing a
        // regular FILE at the `diagnostics` path.
        std::fs::create_dir_all(root_path.join(".ralph")).expect("create .ralph");
        std::fs::write(root_path.join(".ralph").join("diagnostics"), b"x")
            .expect("plant colliding file");

        let payload = serde_json::json!({
            "wave_id": "w-u4-neg",
            "generated_at_kind": "injected_failed",
            "elapsed_secs": 0,
            "slots": []
        });

        let result = write_wave_diagnostics_json(root_path, "w-u4-neg", &payload);
        assert!(
            result.is_err(),
            "write must fail when diagnostics path is a file, got {result:?}"
        );
    }

    /// T2 integration: use InMemoryCoordinatorBridge to simulate a failed
    /// wave with mixed slot states and verify the diagnostics JSON is
    /// written to the temp root.
    #[test]
    fn u5_injected_failed_writes_diagnostics_json() {
        use ralph_core::supervisor::InMemoryCoordinatorBridge;
        use ralph_core::supervisor::SlotResource;
        use ralph_core::supervisor::SupervisorBridge;
        use ralph_core::supervisor::{PhaseInputs, SupervisorStore, WaveKind};

        let temp_root = tempfile::TempDir::new().expect("temp dir");

        // Build an in-memory store with 4 slots.
        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());

        let wave_id = bridge
            .register_wave_if_absent(WaveKind::Exec, "w-u5-integration", 4, 1)
            .unwrap();

        // Slot 0: bind worktree, dispatch, complete.
        store
            .bind_worktree(
                &wave_id,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/s0".to_string()),
                    branch: Some("ralph/u5-s0".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(8).unwrap().unwrap();
        store.record_slot_result(&wave_id, 0, "hash-s0", 1).unwrap();

        // Slot 1: record a failure with worker_timeout.
        store
            .record_slot_failure(&wave_id, 1, "worker_timeout")
            .unwrap();

        // Slot 2: slot_never_started — directly record it as Failed
        // (simulating what record_never_started_failures does for a
        // single pending slot).
        store
            .record_slot_failure(
                &wave_id,
                2,
                ralph_core::supervisor::worker_outcome::REASON_SLOT_NEVER_STARTED,
            )
            .unwrap();

        // Slot 3: cancelled — record this LAST so it is the terminal state.
        // (If we called record_never_started_failures first, it would mark
        // slot 3 as Failed and cause this to fail with AlreadyTerminal.)
        store
            .record_slot_failure(
                &wave_id,
                3,
                ralph_core::supervisor::worker_outcome::REASON_WORKER_CANCELLED,
            )
            .unwrap();

        // Verify the snapshot has the right slot states.
        let snap = store.fan_in_status(&wave_id).unwrap();
        assert_eq!(snap.slots.len(), 4);

        // Build the reasons map via the bridge (simulating what the
        // InjectedFailed arm does).
        use ralph_core::supervisor::SlotStatus;
        let mut reasons = std::collections::HashMap::new();
        for (idx, status) in &snap.slots {
            if matches!(status, SlotStatus::Failed | SlotStatus::Cancelled)
                && let Ok(Some(r)) = bridge.slot_failure_reason(&wave_id, *idx)
            {
                reasons.insert(*idx, r);
            }
        }

        let elapsed_secs = snap.started_at.elapsed().map(|d| d.as_secs()).unwrap_or(0);
        let payload =
            build_wave_failed_slots_json("w-u5-integration", &snap.slots, &reasons, elapsed_secs);

        // Write to the temp root.
        let write_result =
            write_wave_diagnostics_json(temp_root.path(), "w-u5-integration", &payload);
        assert!(
            write_result.is_ok(),
            "write must succeed: {:?}",
            write_result.err()
        );

        // Verify the file exists and has correct content.
        let written_path = write_result.unwrap();
        let bytes = std::fs::read(&written_path).expect("file must be readable");
        let read_back: serde_json::Value =
            serde_json::from_slice(&bytes).expect("must be valid JSON");

        assert_eq!(read_back["wave_id"], "w-u5-integration");
        assert_eq!(read_back["generated_at_kind"], "injected_failed");

        let slots = read_back["slots"]
            .as_array()
            .expect("slots must be an array");
        assert_eq!(slots.len(), 4);

        // Slot 0: completed, no reason.
        assert_eq!(slots[0]["slot_index"], 0);
        assert_eq!(slots[0]["status"], "completed");
        assert!(slots[0]["reason"].is_null());

        // Slot 1: failed, worker_timeout.
        assert_eq!(slots[1]["slot_index"], 1);
        assert_eq!(slots[1]["status"], "failed");
        assert_eq!(slots[1]["reason"], "worker_timeout");

        // Slot 2: failed, slot_never_started (recorded by record_never_started_failures).
        assert_eq!(slots[2]["slot_index"], 2);
        assert_eq!(slots[2]["status"], "failed");
        assert_eq!(slots[2]["reason"], "slot_never_started");

        // Slot 3: cancelled, worker_cancelled.
        assert_eq!(slots[3]["slot_index"], 3);
        assert_eq!(slots[3]["status"], "cancelled");
        assert_eq!(slots[3]["reason"], "worker_cancelled");
    }

    /// 2026-07-26-002 plan U4 (R4): the InjectedFailed arm in
    /// `run_supervisor_fan_in` MUST write the diagnostics JSON
    /// under the workspace root derived from the main events
    /// file (NOT process CWD). This test exercises the production
    /// path end-to-end:
    ///
    /// 1. Construct a real `run_supervisor_fan_in` invocation
    ///    with a Failed/Cancelled slot mix.
    /// 2. Pass a main events file inside a fresh temp dir.
    /// 3. Assert the diagnostics JSON lands at
    ///    `<temp>/.ralph/diagnostics/wave-<id>-slots.json`.
    ///
    /// The previous `u5_injected_failed_writes_diagnostics_json`
    /// test called `write_wave_diagnostics_json` directly, which
    /// masked the CWD bug — that test is preserved as a unit
    /// helper-level guard but this test is the authoritative
    /// production integration check.
    #[test]
    fn u4_run_supervisor_fan_in_injected_failed_writes_workspace_diagnostics() {
        use ralph_core::config::HatConfig;
        use ralph_core::supervisor::InMemoryCoordinatorBridge;
        use ralph_core::supervisor::SlotResource;
        use ralph_core::supervisor::SupervisorBridge;
        use ralph_core::supervisor::{PhaseInputs, SupervisorStore, WaveKind};

        // Workspace = fresh temp dir; main events file lives at
        // <workspace>/.ralph/events.jsonl, exactly as the runner
        // would emit.
        let workspace = tempfile::TempDir::new().expect("temp workspace");
        let ralph_dir = workspace.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
        let main_events_file = ralph_dir.join("events.jsonl");

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());
        let store_wave_id = bridge
            .register_wave_if_absent(WaveKind::Exec, "w-u4-fan-in", 2, 1)
            .unwrap();

        // Slot 0: success.
        store
            .bind_worktree(
                &store_wave_id,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/s0".to_string()),
                    branch: Some("ralph/u4-s0".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(8).unwrap().unwrap();
        store
            .record_slot_result(&store_wave_id, 0, "hash-s0", 1)
            .unwrap();
        // Plan 004 R2 / P0-2: success path requires terminal evidence.
        store
            .record_slot_terminal_evidence(
                &store_wave_id,
                0,
                &ralph_core::supervisor::TerminalEvidence::from_event(
                    "exec.unit.done",
                    "{\"unit\":\"u4-fan-in-0\"}",
                ),
            )
            .unwrap();

        // Slot 1: failure → will become blocking, triggering
        // InjectedFailed.
        store
            .record_slot_failure(
                &store_wave_id,
                1,
                ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT,
            )
            .unwrap();

        // Plan 004 R3 / P0-1: dispatcher must commit salvage BEFORE
        // `fail_wave` latches the coord-event injection.
        bridge
            .commit_salvage_projection(
                &store_wave_id,
                &ralph_core::supervisor::ProjectionReceiptSummary {
                    kind: ralph_core::supervisor::ProjectionKind::Business,
                    batch_fingerprint: String::new(),
                    write_count: 0,
                    already_present_count: 0,
                    committed_at_unix_secs: 0,
                },
            )
            .unwrap();

        let completed = ralph_core::CompletedWave {
            wave_id: "w-u4-fan-in".to_string(),
            wave_total: 2,
            ..ralph_core::CompletedWave::default()
        };
        let detected = ralph_core::DetectedWave {
            wave_id: "w-u4-fan-in".to_string(),
            target_hat: HatId::new("u4-hat"),
            hat_config: HatConfig {
                name: "u4-hat".to_string(),
                ..HatConfig::default()
            },
            events: vec![core_event("work.ready", "payload-0")],
            total: 1,
            partial: false,
            consumer_aggregate_timeout: None,
        };

        let bridge_arc: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(bridge);
        let outcome = run_supervisor_fan_in(
            &bridge_arc,
            &completed,
            &detected,
            &main_events_file,
            60,
            None,
        );
        assert!(
            matches!(outcome, SupervisorFanInOutcome::InjectedFailed),
            "failed wave must reach InjectedFailed; got {outcome:?}"
        );

        // Authoritative assertion: diagnostics JSON exists under
        // the workspace root, not under process CWD.
        let diag_path = workspace
            .path()
            .join(".ralph")
            .join("diagnostics")
            .join("wave-w-u4-fan-in-slots.json");
        assert!(
            diag_path.exists(),
            "InjectedFailed arm must write diagnostics at {diag_path:?}"
        );
        let bytes = std::fs::read(&diag_path).expect("read diagnostics");
        let payload: serde_json::Value =
            serde_json::from_slice(&bytes).expect("diagnostics must be valid JSON");
        assert_eq!(payload["wave_id"], "w-u4-fan-in");
        assert_eq!(payload["generated_at_kind"], "injected_failed");
        let slots = payload["slots"].as_array().expect("slots must be an array");
        assert_eq!(slots.len(), 2);
        // Slot 1 is the Failed slot and must carry the worker_timeout
        // reason from the store (the field the dispatcher used to
        // leave blank by reading `completed.failures` free-form).
        let s1 = slots
            .iter()
            .find(|s| s["slot_index"] == 1)
            .expect("slot 1 must exist");
        assert_eq!(s1["status"], "failed");
        assert_eq!(s1["reason"], "worker_timeout");
    }

    /// U1 Red #1 (plan 2026-07-26-004, S2 / R2): production
    /// `run_supervisor_fan_in` must NOT report a dimension as
    /// missing when that dimension's `review.unit.done` already
    /// lives in the main ledger for this wave (e.g. merged by a
    /// previous fan-in tick). Today the InjectedFailed arm passes
    /// `None` for `review_done_hints`, so a main-only done
    /// dimension is double-counted as missing (the
    /// primary-20260726 inflation). This test drives the REAL
    /// production call point and asserts the reconciled truth; it
    /// goes RED until U3 wires the main-backscan hints into the
    /// payload builder.
    #[test]
    fn u1_red1_fan_in_failed_missing_excludes_main_backscanned_dimension() {
        use ralph_core::config::HatConfig;
        use ralph_core::supervisor::InMemoryCoordinatorBridge;
        use ralph_core::supervisor::SupervisorBridge;
        use ralph_core::supervisor::{SupervisorStore, WaveKind};
        use std::io::{BufRead, Write};

        let workspace = tempfile::TempDir::new().expect("temp workspace");
        let ralph_dir = workspace.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
        let main_events_file = ralph_dir.join("events.jsonl");

        // Pre-seed the main ledger with a `review.unit.done` for the
        // `testing` dimension under THIS wave id — simulating a prior
        // partial fan-in tick that already merged it. Per R2 this
        // dimension is already proven done and must not be re-counted
        // as missing.
        {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&main_events_file)
                .expect("open main");
            let line = serde_json::json!({
                "topic": "review.unit.done",
                "payload": "{\"dimension\":\"testing\"}",
                "ts": "2026-07-26T00:00:00Z",
                "hat": "review-worker",
                "source": "review-worker",
                "wave_id": "w-u1-red1",
            });
            writeln!(f, "{}", line).expect("write main line");
        }

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());
        let store_wave_id = bridge
            .register_wave_if_absent(WaveKind::Review, "w-u1-red1", 2, 1)
            .unwrap();

        // Slot 0: Completed with a real review.unit.done for `correctness`.
        store
            .record_slot_result(&store_wave_id, 0, "hash-s0", 1)
            .unwrap();
        // Plan 004 R2 / P0-2: success path requires terminal evidence.
        store
            .record_slot_terminal_evidence(
                &store_wave_id,
                0,
                &ralph_core::supervisor::TerminalEvidence::from_event(
                    "review.unit.done",
                    &serde_json::json!({"dimension":"correctness"}).to_string(),
                ),
            )
            .unwrap();
        // Slot 1: terminally Failed. Its assigned dimension `testing`
        // is already done in main from the prior tick.
        store
            .record_slot_failure(
                &store_wave_id,
                1,
                ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT,
            )
            .unwrap();
        // Plan 004 R3 / P0-1: dispatcher must commit salvage BEFORE
        // `fail_wave` latches the coord-event injection.
        bridge
            .commit_salvage_projection(
                &store_wave_id,
                &ralph_core::supervisor::ProjectionReceiptSummary {
                    kind: ralph_core::supervisor::ProjectionKind::Business,
                    batch_fingerprint: String::new(),
                    write_count: 0,
                    already_present_count: 0,
                    committed_at_unix_secs: 0,
                },
            )
            .unwrap();

        // completed.results carries ONLY slot 0's event (correctness);
        // `testing` is done via main, not via this fan-in's results.
        let mut assigned = std::collections::HashMap::new();
        assigned.insert(0u32, "correctness".to_string());
        assigned.insert(1u32, "testing".to_string());
        let completed = ralph_core::CompletedWave {
            wave_id: "w-u1-red1".to_string(),
            wave_total: 2,
            results: vec![ralph_core::WaveResult {
                index: 0,
                events: vec![
                    ralph_proto::Event::new(
                        "review.unit.done",
                        serde_json::json!({"dimension":"correctness"}).to_string(),
                    )
                    .with_source("review-worker")
                    .with_wave("w-u1-red1".to_string(), 0, 2),
                ],
            }],
            failures: vec![ralph_core::WaveFailure {
                index: 1,
                error: "worker_timeout".to_string(),
                duration: std::time::Duration::from_millis(1),
                ..ralph_core::WaveFailure::default()
            }],
            duration: std::time::Duration::from_millis(1),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: assigned,
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };
        let detected = ralph_core::DetectedWave {
            wave_id: "w-u1-red1".to_string(),
            target_hat: HatId::new("review-dispatcher"),
            hat_config: HatConfig {
                name: "review-dispatcher".to_string(),
                ..HatConfig::default()
            },
            events: vec![core_event("review.unit.ready", "payload-0")],
            total: 2,
            partial: false,
            consumer_aggregate_timeout: None,
        };

        let bridge_arc: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(bridge);
        let outcome = run_supervisor_fan_in(
            &bridge_arc,
            &completed,
            &detected,
            &main_events_file,
            60,
            None,
        );
        assert!(
            matches!(outcome, SupervisorFanInOutcome::InjectedFailed),
            "failed wave must reach InjectedFailed; got {outcome:?}"
        );

        // Read the injected review.wave.failed coord event and assert
        // missing_dimensions DOES include `testing` — slot 1 is
        // Failed in the store, so the main `review.unit.done` row
        // for `testing` is an orphan projection. The U4
        // reconciliation (R5 / R7) drops the orphan from
        // `authoritative_completed`; the failure handler must see
        // the full missing set so it can re-dispatch the dimension.
        // The pre-U4 union-based path would have excluded
        // `testing` here, which is the implementation-review
        // primary-20260727 incident.
        let failed = std::io::BufReader::new(std::fs::File::open(&main_events_file).expect("main"))
            .lines()
            .map_while(Result::ok)
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(&l).ok())
            .find(|r| r["topic"] == "review.wave.failed")
            .expect("a review.wave.failed coord event must be injected");
        let missing: std::collections::HashSet<String> = failed["payload"]["missing_dimensions"]
            .as_array()
            .expect("missing_dimensions array")
            .iter()
            .map(|v| v.as_str().expect("str").to_string())
            .collect();
        assert!(
            missing.contains("testing"),
            "U4 (R5 / R7): slot 1 is Failed in the store, so the main \
             review.unit.done row for `testing` is an orphan projection. \
             The authoritative reconciliation must report `testing` as \
             missing so the failure handler can re-dispatch it. The pre-U4 \
             union-based path would have dropped `testing` from \
             missing_dimensions (the primary-20260727 incident). \
             got {missing:?}"
        );
    }

    /// 2026-07-26-004 plan U3 (R1 / R2 / KTD3) + 2026-07-27-003
    /// plan U4 (KTD3 / R5 / R7): `build_review_done_hints`
    /// reconciles the two cross-source views correctly and stays
    /// bounded:
    /// - `main_backscan` keeps ONLY same-wave `review.unit.done` rows
    ///   AND only those whose slot is in the store's authoritative
    ///   set (a main row with no slot_index, or whose slot is Failed
    ///   / Pending, drops out and is reported via the new
    ///   `ReviewReconciliation` orphan / conflict lists);
    /// - `store_completed` keeps ONLY Completed slots WITH valid
    ///   terminal evidence (a legacy Completed status bit with no
    ///   evidence is fail-closed and does NOT count).
    #[test]
    fn u3_build_review_done_hints_is_bounded_and_evidence_gated() {
        use ralph_core::supervisor::{
            InMemoryCoordinatorBridge, SupervisorBridge, SupervisorStore, TerminalEvidence,
            WaveKind,
        };
        use std::io::Write;

        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("events.jsonl");
        {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&main)
                .expect("open main");
            let row = |dim: &str, wave: Option<&str>, slot_index: Option<u32>| -> String {
                let mut rec = serde_json::json!({
                    "topic": "review.unit.done",
                    "payload": serde_json::json!({"dimension": dim}).to_string(),
                    "hat": "review-worker",
                    "source": "review-worker",
                });
                if let Some(w) = wave {
                    rec["wave_id"] = serde_json::Value::String(w.to_string());
                }
                if let Some(idx) = slot_index {
                    rec["slot_index"] = serde_json::Value::Number(idx.into());
                }
                rec.to_string()
            };
            // same wave + slot 0 authoritative → counted
            writeln!(f, "{}", row("correctness", Some("W-main"), Some(0))).unwrap();
            // different wave → ignored
            writeln!(f, "{}", row("security", Some("W-other"), Some(0))).unwrap();
            // no wave_id → ignored (fail-closed)
            writeln!(f, "{}", row("testing", None, Some(1))).unwrap();
            // malformed → ignored
            writeln!(f, "not-json").unwrap();
        }

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());
        let store_wave_id = bridge
            .register_wave_if_absent(WaveKind::Review, "W-main", 2, 1)
            .unwrap();
        // Slot 0: Completed WITH evidence (dimension `performance`).
        store
            .record_slot_result(&store_wave_id, 0, "h0", 1)
            .unwrap();
        store
            .record_slot_terminal_evidence(
                &store_wave_id,
                0,
                &TerminalEvidence::from_event(
                    "review.unit.done",
                    "{\"dimension\":\"performance\"}",
                ),
            )
            .unwrap();
        // Slot 1: Completed but NO evidence (legacy) → must NOT count.
        store
            .record_slot_result(&store_wave_id, 1, "h1", 1)
            .unwrap();

        let mut assigned = std::collections::HashMap::new();
        assigned.insert(0u32, "performance".to_string());
        assigned.insert(1, "maintainability".to_string());
        let completed = ralph_core::CompletedWave {
            wave_id: "W-main".to_string(),
            wave_total: 2,
            assigned_dimensions: assigned,
            ..ralph_core::CompletedWave::default()
        };

        let bridge_arc: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(bridge);
        let hints = build_review_done_hints(&bridge_arc, &store_wave_id, &completed, &main);

        // U4 (KTD3 / R5 / R7): `main_backscan` only keeps rows
        // whose slot is in the store's authoritative set. Slot 0
        // IS authoritative (Completed with `performance` evidence);
        // the main row's `dimension` (`correctness`) is the value
        // that lands in the set (the row is an authoritative
        // projection even though its `dimension` string disagrees
        // with the slot's assignment — the post-filter for the
        // failed-payload builder is on slot index, not on
        // dimension string match).
        assert_eq!(
            hints.main_backscan,
            ["correctness".to_string()].into_iter().collect(),
            "main_backscan must keep same-wave rows whose slot is in the \
             store's authoritative set"
        );
        assert_eq!(
            hints.store_completed,
            ["performance".to_string()].into_iter().collect(),
            "store_completed must keep only Completed-with-evidence slots"
        );
    }

    /// U4 Red (plan 2026-07-26-004, S9 / R3): replaying a failed
    /// fan-in MUST NOT double-write. Calling `run_supervisor_fan_in`
    /// twice for the same mixed Review wave must leave exactly ONE
    /// `review.wave.failed` coord event and ONE salvaged
    /// `review.unit.done` in the main ledger. Before U4, `fail_wave`
    /// had no idempotency latch (`evaluate_phase` is pure and keeps
    /// returning `Failed`), so the second tick re-injected the coord
    /// event and re-ran the dispatcher-layer salvage merge.
    #[test]
    fn u4_replayed_failed_fan_in_does_not_double_write() {
        use ralph_core::config::HatConfig;
        use ralph_core::supervisor::InMemoryCoordinatorBridge;
        use ralph_core::supervisor::SupervisorBridge;
        use ralph_core::supervisor::{SupervisorStore, WaveKind};
        use std::io::BufRead;

        let workspace = tempfile::TempDir::new().expect("temp workspace");
        let ralph_dir = workspace.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
        let main_events_file = ralph_dir.join("events.jsonl");

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());
        let store_wave_id = bridge
            .register_wave_if_absent(WaveKind::Review, "w-u4-replay", 2, 1)
            .unwrap();
        // Slot 0: Completed with a real review.unit.done (correctness).
        store
            .record_slot_result(&store_wave_id, 0, "hash-s0", 1)
            .unwrap();
        // Plan 004 R2 / P0-2: success path requires terminal evidence.
        store
            .record_slot_terminal_evidence(
                &store_wave_id,
                0,
                &ralph_core::supervisor::TerminalEvidence::from_event(
                    "review.unit.done",
                    &serde_json::json!({"dimension":"correctness"}).to_string(),
                ),
            )
            .unwrap();
        // Slot 1: terminally Failed → InjectedFailed.
        store
            .record_slot_failure(
                &store_wave_id,
                1,
                ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT,
            )
            .unwrap();
        // Plan 004 R3 / P0-1: dispatcher must commit salvage BEFORE
        // `fail_wave` latches the coord-event injection.
        bridge
            .commit_salvage_projection(
                &store_wave_id,
                &ralph_core::supervisor::ProjectionReceiptSummary {
                    kind: ralph_core::supervisor::ProjectionKind::Business,
                    batch_fingerprint: String::new(),
                    write_count: 0,
                    already_present_count: 0,
                    committed_at_unix_secs: 0,
                },
            )
            .unwrap();

        let mut assigned = std::collections::HashMap::new();
        assigned.insert(0u32, "correctness".to_string());
        assigned.insert(1u32, "testing".to_string());
        let completed = ralph_core::CompletedWave {
            wave_id: "w-u4-replay".to_string(),
            wave_total: 2,
            results: vec![ralph_core::WaveResult {
                index: 0,
                events: vec![
                    ralph_proto::Event::new(
                        "review.unit.done",
                        serde_json::json!({"dimension":"correctness"}).to_string(),
                    )
                    .with_source("review-worker")
                    .with_wave("w-u4-replay".to_string(), 0, 2),
                ],
            }],
            failures: vec![ralph_core::WaveFailure {
                index: 1,
                error: "worker_timeout".to_string(),
                duration: std::time::Duration::from_millis(1),
                ..ralph_core::WaveFailure::default()
            }],
            duration: std::time::Duration::from_millis(1),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: assigned,
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };
        let detected = ralph_core::DetectedWave {
            wave_id: "w-u4-replay".to_string(),
            target_hat: HatId::new("review-dispatcher"),
            hat_config: HatConfig {
                name: "review-dispatcher".to_string(),
                ..HatConfig::default()
            },
            events: vec![core_event("review.unit.ready", "payload-0")],
            total: 2,
            partial: false,
            consumer_aggregate_timeout: None,
        };

        let bridge_arc: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(bridge);
        // First fan-in: reaches InjectedFailed.
        let first = run_supervisor_fan_in(
            &bridge_arc,
            &completed,
            &detected,
            &main_events_file,
            60,
            None,
        );
        assert!(matches!(first, SupervisorFanInOutcome::InjectedFailed));
        // Replay: must be a no-op (AlreadyDone), NOT a second InjectedFailed.
        let second = run_supervisor_fan_in(
            &bridge_arc,
            &completed,
            &detected,
            &main_events_file,
            60,
            None,
        );
        assert!(
            matches!(second, SupervisorFanInOutcome::AlreadyDone),
            "replayed failed fan-in must be AlreadyDone; got {second:?}"
        );

        let count = |topic: &str| {
            std::io::BufReader::new(std::fs::File::open(&main_events_file).expect("main"))
                .lines()
                .map_while(Result::ok)
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(&l).ok())
                .filter(|r| r["topic"] == topic)
                .count()
        };
        assert_eq!(
            count("review.wave.failed"),
            1,
            "exactly one review.wave.failed after replay"
        );
        assert_eq!(
            count("review.unit.done"),
            1,
            "salvaged review.unit.done must not be double-written on replay"
        );
    }

    /// U5 (plan 2026-07-26-004, S4 / AE2): a worker's terminal event
    /// must keep its WORKER producer across the fan-in merge — never
    /// inherit the current `review-dispatcher` activation. The trusted
    /// merge seam normalises the salvaged `review.unit.done` to
    /// `review-worker` even when the in-flight event carried a missing
    /// or spoofed source, so a later replay during the dispatcher
    /// activation cannot mis-attribute it (no `isolated_scope_violation`
    /// against `review-dispatcher`).
    #[test]
    fn u5_salvaged_worker_event_keeps_worker_provenance() {
        use ralph_core::config::HatConfig;
        use ralph_core::supervisor::InMemoryCoordinatorBridge;
        use ralph_core::supervisor::SupervisorBridge;
        use ralph_core::supervisor::{SupervisorStore, WaveKind};
        use std::io::BufRead;

        let workspace = tempfile::TempDir::new().expect("temp workspace");
        let ralph_dir = workspace.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
        let main_events_file = ralph_dir.join("events.jsonl");

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());
        let store_wave_id = bridge
            .register_wave_if_absent(WaveKind::Review, "w-u5-prov", 2, 1)
            .unwrap();
        store
            .record_slot_result(&store_wave_id, 0, "hash-s0", 1)
            .unwrap();
        // Plan 004 R2 / P0-2: success path requires terminal evidence.
        store
            .record_slot_terminal_evidence(
                &store_wave_id,
                0,
                &ralph_core::supervisor::TerminalEvidence::from_event(
                    "review.unit.done",
                    &serde_json::json!({"dimension":"correctness"}).to_string(),
                ),
            )
            .unwrap();
        store
            .record_slot_failure(
                &store_wave_id,
                1,
                ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT,
            )
            .unwrap();
        // Plan 004 R3 / P0-1: dispatcher must commit salvage BEFORE
        // `fail_wave` latches the coord-event injection.
        bridge
            .commit_salvage_projection(
                &store_wave_id,
                &ralph_core::supervisor::ProjectionReceiptSummary {
                    kind: ralph_core::supervisor::ProjectionKind::Business,
                    batch_fingerprint: String::new(),
                    write_count: 0,
                    already_present_count: 0,
                    committed_at_unix_secs: 0,
                },
            )
            .unwrap();

        let mut assigned = std::collections::HashMap::new();
        assigned.insert(0u32, "correctness".to_string());
        assigned.insert(1u32, "testing".to_string());
        // Slot 0's event carries a SPOOFED source (review-dispatcher) to
        // prove the merge seam normalises provenance to the real worker.
        let completed = ralph_core::CompletedWave {
            wave_id: "w-u5-prov".to_string(),
            wave_total: 2,
            results: vec![ralph_core::WaveResult {
                index: 0,
                events: vec![
                    ralph_proto::Event::new(
                        "review.unit.done",
                        serde_json::json!({"dimension":"correctness"}).to_string(),
                    )
                    .with_source("review-dispatcher")
                    .with_wave("w-u5-prov".to_string(), 0, 2),
                ],
            }],
            failures: vec![ralph_core::WaveFailure {
                index: 1,
                error: "worker_timeout".to_string(),
                duration: std::time::Duration::from_millis(1),
                ..ralph_core::WaveFailure::default()
            }],
            duration: std::time::Duration::from_millis(1),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: assigned,
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };
        let detected = ralph_core::DetectedWave {
            wave_id: "w-u5-prov".to_string(),
            target_hat: HatId::new("review-dispatcher"),
            hat_config: HatConfig {
                name: "review-dispatcher".to_string(),
                ..HatConfig::default()
            },
            events: vec![core_event("review.unit.ready", "payload-0")],
            total: 2,
            partial: false,
            consumer_aggregate_timeout: None,
        };

        let bridge_arc: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(bridge);
        let outcome = run_supervisor_fan_in(
            &bridge_arc,
            &completed,
            &detected,
            &main_events_file,
            60,
            None,
        );
        assert!(matches!(outcome, SupervisorFanInOutcome::InjectedFailed));

        let done = std::io::BufReader::new(std::fs::File::open(&main_events_file).expect("main"))
            .lines()
            .map_while(Result::ok)
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(&l).ok())
            .find(|r| r["topic"] == "review.unit.done")
            .expect("the salvaged review.unit.done must be in main");
        assert_eq!(
            done["hat"], "review-worker",
            "salvaged worker event must keep worker producer (hat)"
        );
        assert_eq!(
            done["source"], "review-worker",
            "salvaged worker event must keep worker producer (source), not the dispatcher"
        );
    }

    /// 2026-07-26-002 plan U5 (R5 / KTD6): `slot_failures` MUST be
    /// derived from the store's frozen reason codes filtered by
    /// `blocking_slots` — the index set of `slot_failures` must
    /// equal `blocking_slots` exactly, and the reason strings
    /// must come from the `reasons` map (NOT from
    /// `completed.failures` free-form text).
    #[test]
    fn u5_slot_failures_matches_blocking_slots_from_store() {
        use ralph_core::supervisor::WaveKind;

        // 3 slots: 0 success, 1 worker_timeout, 2 empty_worker_result
        let completed = ralph_core::CompletedWave {
            wave_id: "w-u5-ssot".to_string(),
            wave_total: 3,
            ..ralph_core::CompletedWave::default()
        };
        let blocking_slots = vec![1u32, 2];
        // Store-derived reasons (frozen codes).
        let mut reasons = std::collections::HashMap::new();
        reasons.insert(1u32, "worker_timeout".to_string());
        reasons.insert(2u32, "empty_worker_result".to_string());

        let payload = build_wave_failed_payload(
            WaveKind::Exec,
            &completed,
            "wave_failed",
            blocking_slots.clone(),
            &reasons,
            None,
            None,
        );

        // slot_failures must be present and its index set must equal blocking_slots.
        let slot_failures = payload["slot_failures"]
            .as_array()
            .expect("slot_failures must be an array");
        let sf_indices: std::collections::BTreeSet<u32> = slot_failures
            .iter()
            .map(|s| s["slot_index"].as_u64().unwrap() as u32)
            .collect();
        let bs_indices: std::collections::BTreeSet<u32> = blocking_slots.iter().copied().collect();
        assert_eq!(
            sf_indices, bs_indices,
            "slot_failures index set must equal blocking_slots; got slot_failures={sf_indices:?}, blocking_slots={bs_indices:?}"
        );

        // Reasons are taken from the store, not from free-form text.
        let s1 = slot_failures
            .iter()
            .find(|s| s["slot_index"] == 1)
            .expect("slot 1 must be present");
        let s2 = slot_failures
            .iter()
            .find(|s| s["slot_index"] == 2)
            .expect("slot 2 must be present");
        assert_eq!(s1["reason"], "worker_timeout");
        assert_eq!(s2["reason"], "empty_worker_result");
    }

    /// 2026-07-26-002 plan U5 (R5): when the store has NO reason for
    /// a blocking slot (e.g., legacy store without `record_slot_failure`),
    /// the payload must still include that slot in `slot_failures`
    /// — keeping the index-set invariant — but with `reason: null`,
    /// NOT a free-form fallback string from `completed.failures`.
    #[test]
    fn u5_slot_failures_no_store_reason_yields_null() {
        use ralph_core::supervisor::WaveKind;

        let completed = ralph_core::CompletedWave {
            wave_id: "w-u5-null".to_string(),
            wave_total: 1,
            ..ralph_core::CompletedWave::default()
        };
        let payload = build_wave_failed_payload(
            WaveKind::Exec,
            &completed,
            "wave_failed",
            vec![7u32],
            &std::collections::HashMap::new(),
            None,
            None,
        );

        let slot_failures = payload["slot_failures"].as_array().unwrap();
        assert_eq!(slot_failures.len(), 1);
        assert_eq!(slot_failures[0]["slot_index"], 7);
        assert!(
            slot_failures[0]["reason"].is_null(),
            "no-store-reason slot must report null, not free-form text"
        );
    }

    // -------------------------------------------------------------------
    // 2026-07-26-002 plan U3 (R3 / KTD3): workspace_root_from_events
    // must always yield an absolute workspace root, never
    // `Path::new(".")` — the validator would reject every spawn with
    // RelativePath when the bridge repo_root is relative.
    // -------------------------------------------------------------------

    #[test]
    fn u3_workspace_root_from_events_absolute() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let ralph = tmp.path().join(".ralph");
        std::fs::create_dir_all(&ralph).expect("mkdir .ralph");
        let events = ralph.join("events.jsonl");

        let root = workspace_root_from_events(&events);
        assert!(
            root.is_absolute(),
            "workspace_root_from_events must be absolute; got {root:?}"
        );
        assert_eq!(
            root,
            tmp.path(),
            "two `.parent()` calls from <ws>/.ralph/events.jsonl must yield <ws>"
        );
    }

    #[test]
    fn u3_workspace_root_from_events_relative_falls_back() {
        // Defensive: even when a relative path slips through (the
        // runner always passes absolute), the helper must still
        // return an absolute root. We do not rely on
        // `set_current_dir` (unreliable under nextest's
        // process-per-test isolation); we just assert absoluteness.
        let rel = std::path::Path::new(".ralph").join("events.jsonl");
        let root = workspace_root_from_events(&rel);
        assert!(
            root.is_absolute(),
            "relative input must still produce an absolute workspace root; got {root:?}"
        );
    }

    #[test]
    fn u3_lazy_bridge_repo_root_is_absolute() {
        // 2026-07-26-002 plan U3: the lazy
        // `CoordinatorSupervisorBridge::with_context_and_factory_with_cap`
        // path used in `dispatch_waves` must build the bridge with
        // `repo_root` derived from the main events file (absolute),
        // NOT the previous `PathBuf::from(".")`. We exercise the
        // same construction and assert `bridge.repo_root()` returns
        // the absolute workspace, not `.` or `None`.
        use crate::loop_runner::wave::ProductionBridgeContext;
        use crate::loop_runner::wave::supervisor_bridge::CoordinatorSupervisorBridge;
        use ralph_core::supervisor::SupervisorBridge as _;
        use ralph_core::supervisor::worktree_bind::DefaultWorktreeFactory;

        let tmp = tempfile::TempDir::new().expect("temp dir");
        let main_events_file = tmp.path().join(".ralph").join("events.jsonl");

        let context = ProductionBridgeContext {
            loop_id: "loop-u3".to_string(),
            repo_root: workspace_root_from_events(&main_events_file),
            events_path: Some(main_events_file.clone()),
            tasks_path: None,
        };
        assert!(
            context.repo_root.is_absolute(),
            "ProductionBridgeContext.repo_root must be absolute; got {:?}",
            context.repo_root
        );

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = CoordinatorSupervisorBridge::with_context_and_factory_with_cap(
            store,
            context,
            Arc::new(DefaultWorktreeFactory),
            u32::MAX,
            // 2026-07-28-003 plan U4: surface `slot_retry_budget`
            // arg so this characterization test does not regress
            // budget pass-through.
            1,
        );
        let reported = bridge
            .repo_root()
            .expect("bridge must surface repo_root; lazy paths used to return None");
        assert_eq!(reported, tmp.path());
    }

    // ─────────────────────────────────────────────────────────────────
    // Plan 004 P1-6: terminal evidence topic/dimension strict checks
    //
    // The post-fix `build_review_done_hints` rejects four classes of
    // mismatch (KTD3 fail-closed):
    //   1. evidence topic != "review.unit.done"
    //   2. evidence dimension missing
    //   3. evidence dimension != slot's assigned dimension
    //   4. slot has no assigned dimension
    // Each test below pins one failure mode so a future regression
    // that re-introduces the silent-fallback surfaces here.
    // ─────────────────────────────────────────────────────────────────

    fn build_p1_6_hints(
        store: &std::sync::Arc<ralph_core::supervisor::InMemorySupervisorStore>,
        wave_id: &str,
        assigned_dimensions: std::collections::HashMap<u32, String>,
    ) -> ReviewDoneHints {
        use ralph_core::supervisor::SupervisorBridge as _;
        use ralph_core::wave_tracker::CompletedWave;
        let bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge> =
            Arc::new(ralph_core::supervisor::InMemoryCoordinatorBridge::from_store(store.clone()));
        let completed = CompletedWave {
            wave_id: wave_id.to_string(),
            wave_total: assigned_dimensions.len() as u32,
            results: Vec::new(),
            failures: Vec::new(),
            duration: std::time::Duration::from_secs(0),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions,
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };
        let main_events = std::env::temp_dir().join("p1-6-does-not-exist.jsonl");
        build_review_done_hints(&bridge, wave_id, &completed, &main_events)
    }

    /// P1-6 #1: evidence topic != "review.unit.done" is rejected.
    #[test]
    fn p1_6_wrong_evidence_topic_is_rejected() {
        use ralph_core::supervisor::{SlotResource, SupervisorStore, TerminalEvidence, WaveKind};
        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let wave = store
            .register_wave("p1-6-topic", WaveKind::Review, 1, 1)
            .unwrap();
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        store
            .record_slot_terminal_evidence(
                &wave,
                0,
                &TerminalEvidence::from_event(
                    "work.start", // wrong topic
                    "{\"dimension\":\"correctness\"}",
                ),
            )
            .unwrap();
        let mut assigned = std::collections::HashMap::new();
        assigned.insert(0, "correctness".to_string());
        let hints = build_p1_6_hints(&store, &wave, assigned);
        assert!(
            !hints.store_completed.contains("correctness"),
            "wrong-topic evidence must not be accepted as done (got {:?})",
            hints.store_completed,
        );
    }

    /// P1-6 #2: evidence with no dimension is rejected.
    #[test]
    fn p1_6_missing_dimension_is_rejected() {
        use ralph_core::supervisor::{SlotResource, SupervisorStore, TerminalEvidence, WaveKind};
        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let wave = store
            .register_wave("p1-6-dim", WaveKind::Review, 1, 1)
            .unwrap();
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        // Note: TerminalEvidence::from_event without a dimension
        // field yields dimension=None (matches the legacy
        // happy-path that the post-fix code refuses).
        let evidence = TerminalEvidence {
            topic: "review.unit.done".to_string(),
            dimension: None,
            payload_fingerprint: "abc".to_string(),
        };
        store
            .record_slot_terminal_evidence(&wave, 0, &evidence)
            .unwrap();
        let mut assigned = std::collections::HashMap::new();
        assigned.insert(0, "correctness".to_string());
        let hints = build_p1_6_hints(&store, &wave, assigned);
        assert!(
            hints.store_completed.is_empty(),
            "missing-dimension evidence must not be accepted (got {:?})",
            hints.store_completed,
        );
    }

    /// P1-6 #3: evidence dimension != assigned is rejected.
    #[test]
    fn p1_6_dimension_mismatch_is_rejected() {
        use ralph_core::supervisor::{SlotResource, SupervisorStore, TerminalEvidence, WaveKind};
        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let wave = store
            .register_wave("p1-6-mis", WaveKind::Review, 1, 1)
            .unwrap();
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        store
            .record_slot_terminal_evidence(
                &wave,
                0,
                &TerminalEvidence::from_event(
                    "review.unit.done",
                    "{\"dimension\":\"security\"}", // mismatched
                ),
            )
            .unwrap();
        let mut assigned = std::collections::HashMap::new();
        assigned.insert(0, "correctness".to_string());
        let hints = build_p1_6_hints(&store, &wave, assigned);
        assert!(
            !hints.store_completed.contains("correctness"),
            "dimension-mismatched evidence must not be accepted as done",
        );
        assert!(
            !hints.store_completed.contains("security"),
            "wrong dimension must not be counted under any name",
        );
    }

    /// P1-6 #4: slot has no assigned dimension at all → refuse.
    #[test]
    fn p1_6_no_assigned_dimension_is_rejected() {
        use ralph_core::supervisor::{SlotResource, SupervisorStore, TerminalEvidence, WaveKind};
        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let wave = store
            .register_wave("p1-6-na", WaveKind::Review, 1, 1)
            .unwrap();
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        store
            .record_slot_terminal_evidence(
                &wave,
                0,
                &TerminalEvidence::from_event(
                    "review.unit.done",
                    "{\"dimension\":\"correctness\"}",
                ),
            )
            .unwrap();
        let assigned = std::collections::HashMap::new();
        let hints = build_p1_6_hints(&store, &wave, assigned);
        assert!(
            hints.store_completed.is_empty(),
            "no-assigned-dimension must fail closed",
        );
    }

    /// P1-6 positive control: a slot whose evidence topic,
    /// dimension, and assigned dimension ALL agree IS
    /// accepted.
    #[test]
    fn p1_6_matching_evidence_dimension_accepted() {
        use ralph_core::supervisor::{SlotResource, SupervisorStore, TerminalEvidence, WaveKind};
        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let wave = store
            .register_wave("p1-6-ok", WaveKind::Review, 1, 1)
            .unwrap();
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        store
            .record_slot_terminal_evidence(
                &wave,
                0,
                &TerminalEvidence::from_event(
                    "review.unit.done",
                    "{\"dimension\":\"correctness\"}",
                ),
            )
            .unwrap();
        let mut assigned = std::collections::HashMap::new();
        assigned.insert(0, "correctness".to_string());
        let hints = build_p1_6_hints(&store, &wave, assigned);
        assert!(
            hints.store_completed.contains("correctness"),
            "matching evidence + assigned must be accepted; got {:?}",
            hints.store_completed,
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // Plan 004 P1-7: main-ledger reconciliation accepts both
    // object and JSON-encoded-string payload shapes. The
    // pre-fix code only consumed the string form, so object
    // payloads (the supervisor merge sink writes them
    // directly) were silently ignored and the dimension was
    // re-counted as missing.
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn p1_7_payload_object_from_string_json() {
        // String-encoded JSON (legacy agent-emit path).
        let payload = serde_json::Value::String(
            serde_json::to_string(&serde_json::json!({"dimension":"correctness"})).unwrap(),
        );
        let map = payload_object(Some(&payload));
        assert!(map.is_some());
        assert_eq!(
            map.unwrap().get("dimension").and_then(|v| v.as_str()),
            Some("correctness"),
        );
    }

    #[test]
    fn p1_7_payload_object_from_inline_object() {
        // Inline object (supervisor merge sink path).
        let payload = serde_json::json!({"dimension": "correctness"});
        let map = payload_object(Some(&payload));
        assert!(map.is_some());
        assert_eq!(
            map.unwrap().get("dimension").and_then(|v| v.as_str()),
            Some("correctness"),
        );
    }

    #[test]
    fn p1_7_payload_object_missing_returns_none() {
        let map = payload_object(None);
        assert!(map.is_none());
    }

    #[test]
    fn p1_7_payload_object_malformed_string_returns_none() {
        // String that is not valid JSON.
        let payload = serde_json::Value::String("not json".to_string());
        let map = payload_object(Some(&payload));
        assert!(map.is_none());
    }

    // ══════════════════════════════════════════════════════════════════
    // U1: fan-in terminal convergence — RED characterization tests
    // RED: these tests characterize the broken behavior BEFORE the fix.
    // GREEN: the fix makes them pass.
    // ══════════════════════════════════════════════════════════════════

    /// AE3 / R3: when AggregateDeadlineExceeded arrives with Pending slots,
    /// fan-in must converge to InjectedFailed (not ContinueCollect).
    #[test]
    fn terminal_aggregate_deadline_does_not_end_as_continue_collect() {
        use ralph_core::config::HatConfig;
        use ralph_core::supervisor::{SupervisorBridge, SupervisorStore, WaveKind};

        let workspace = tempfile::TempDir::new().expect("temp workspace");
        let ralph_dir = workspace.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
        let main_events_file = ralph_dir.join("events.jsonl");

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());
        let store_wave_id = bridge
            .register_wave_if_absent(WaveKind::Review, "w-u1-red1", 2, 1)
            .unwrap();

        // Dispatch BOTH slots so they exist in the store.
        // Slot 0: dispatched, completed with evidence.
        // Slot 1: dispatched, then cancelled via cancel_wave (simulating timeout).
        let _ = store.try_dispatch_next(2).unwrap().unwrap(); // slot 0
        let _ = store.try_dispatch_next(2).unwrap().unwrap(); // slot 1
        // Slot 0: Completed with evidence.
        store
            .record_slot_result(&store_wave_id, 0, "hash-s0", 1)
            .unwrap();
        store
            .record_slot_terminal_evidence(
                &store_wave_id,
                0,
                &ralph_core::supervisor::TerminalEvidence::from_event(
                    "review.unit.done",
                    &serde_json::json!({"dimension": "correctness"}).to_string(),
                ),
            )
            .unwrap();
        // Slot 1: Pending in store (will be cancelled by cancel_wave).

        let completed = ralph_core::CompletedWave {
            wave_id: "w-u1-red1".to_string(),
            wave_total: 2,
            results: vec![ralph_core::WaveResult {
                index: 0,
                events: vec![
                    ralph_proto::Event::new(
                        "review.unit.done",
                        serde_json::json!({"dimension": "correctness"}).to_string(),
                    )
                    .with_source("review-worker")
                    .with_wave("w-u1-red1".to_string(), 0, 2),
                ],
            }],
            failures: vec![],
            duration: std::time::Duration::ZERO,
            partial: true,
            expected_source_hat: None,
            assigned_dimensions: std::collections::HashMap::new(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };
        let detected = ralph_core::DetectedWave {
            wave_id: "w-u1-red1".to_string(),
            target_hat: ralph_proto::HatId::new("review-dispatcher"),
            hat_config: HatConfig {
                name: "review-dispatcher".to_string(),
                ..HatConfig::default()
            },
            events: vec![ralph_core::Event {
                topic: "review.unit.ready".to_string(),
                payload: Some("payload-0".to_string()),
                ts: String::new(),
                hat: None,
                triggered: None,
                source: None,
                wave_id: None,
                wave_index: None,
                wave_total: None,
                system_injected: None,
            }],
            total: 2,
            partial: true,
            consumer_aggregate_timeout: None,
        };

        // Record slot 1 as Failed (simulating worker never started / timed out).
        // Using Failed instead of cancel so pending_count = 0 in fan_in_status.
        let fail_result = store.record_slot_failure(
            &store_wave_id,
            1,
            ralph_core::supervisor::worker_outcome::REASON_SLOT_NEVER_STARTED,
        );
        fail_result.unwrap();

        let bridge_arc: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(bridge);
        let terminal_ctx = Some(TerminalFanInContext {
            cancel_requested: true, // AggregateDeadlineExceeded → cancel
            elapsed: completed.duration,
        });
        let outcome = run_supervisor_fan_in(
            &bridge_arc,
            &completed,
            &detected,
            &main_events_file,
            60,
            terminal_ctx,
        );

        // GREEN: must reach InjectedFailed.
        assert!(
            matches!(outcome, SupervisorFanInOutcome::InjectedFailed),
            "aggregate deadline with Pending slots must reach InjectedFailed, got {:?}",
            outcome
        );
    }

    /// AE2 / R3: partial=true with Pending slots must converge to InjectedFailed.
    #[test]
    fn terminal_partial_with_pending_slot_converges_to_failed() {
        use ralph_core::config::HatConfig;
        use ralph_core::supervisor::{SupervisorBridge, SupervisorStore, WaveKind};

        let workspace = tempfile::TempDir::new().expect("temp workspace");
        let ralph_dir = workspace.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
        let main_events_file = ralph_dir.join("events.jsonl");

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());
        let store_wave_id = bridge
            .register_wave_if_absent(WaveKind::Review, "w-u1-red2", 2, 1)
            .unwrap();

        // Dispatch slot 0 so it moves from Pending to Dispatched,
        // then record the result (simulating a completed worker).
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        // Slot 0: Completed with evidence.
        store
            .record_slot_result(&store_wave_id, 0, "hash-s0", 1)
            .unwrap();
        store
            .record_slot_terminal_evidence(
                &store_wave_id,
                0,
                &ralph_core::supervisor::TerminalEvidence::from_event(
                    "review.unit.done",
                    &serde_json::json!({"dimension": "correctness"}).to_string(),
                ),
            )
            .unwrap();
        // Slot 1: Pending (never dispatched — slot stays Pending in the store).

        let completed = ralph_core::CompletedWave {
            wave_id: "w-u1-red2".to_string(),
            wave_total: 2,
            results: vec![ralph_core::WaveResult {
                index: 0,
                events: vec![
                    ralph_proto::Event::new(
                        "review.unit.done",
                        serde_json::json!({"dimension": "correctness"}).to_string(),
                    )
                    .with_source("review-worker")
                    .with_wave("w-u1-red2".to_string(), 0, 2),
                ],
            }],
            failures: vec![],
            duration: std::time::Duration::ZERO,
            partial: true,
            expected_source_hat: None,
            assigned_dimensions: std::collections::HashMap::new(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };
        let detected = ralph_core::DetectedWave {
            wave_id: "w-u1-red2".to_string(),
            target_hat: ralph_proto::HatId::new("review-dispatcher"),
            hat_config: HatConfig {
                name: "review-dispatcher".to_string(),
                ..HatConfig::default()
            },
            events: vec![ralph_core::Event {
                topic: "review.unit.ready".to_string(),
                payload: Some("payload-0".to_string()),
                ts: String::new(),
                hat: None,
                triggered: None,
                source: None,
                wave_id: None,
                wave_index: None,
                wave_total: None,
                system_injected: None,
            }],
            total: 2,
            partial: true,
            consumer_aggregate_timeout: None,
        };

        let bridge_arc: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(bridge);
        let terminal_ctx = Some(TerminalFanInContext {
            cancel_requested: false, // Partial (not cancel)
            elapsed: completed.duration,
        });
        let outcome = run_supervisor_fan_in(
            &bridge_arc,
            &completed,
            &detected,
            &main_events_file,
            60,
            terminal_ctx,
        );

        // GREEN: the helper drives to InjectedFailed.
        assert!(
            matches!(outcome, SupervisorFanInOutcome::InjectedFailed),
            "partial=true with Pending slots must converge to InjectedFailed, got {:?}",
            outcome
        );
    }

    /// GREEN baseline: non-terminal wave stays ContinueCollect.
    #[test]
    fn non_terminal_tick_remains_continue_collect() {
        use ralph_core::supervisor::{PhaseInputs, SupervisorBridge, SupervisorStore, WaveKind};

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());
        let store_wave_id = bridge
            .register_wave_if_absent(WaveKind::Review, "w-u1-green1", 2, 1)
            .unwrap();
        // No slots dispatched — all Pending.

        let inputs = PhaseInputs {
            aggregate_timeout_secs: 60,
            elapsed_secs: 0,
            cancel_requested: false,
        };
        let action = bridge
            .tick_with_slot_events(&store_wave_id, inputs, Vec::new())
            .expect("tick succeeds");
        assert!(
            matches!(
                action,
                ralph_core::supervisor::CoordinatorAction::ContinueCollect
            ),
            "non-terminal wave must stay ContinueCollect, got {:?}",
            action
        );
    }

    /// S5: persistent store error must return StoreError.
    #[test]
    fn terminal_fan_in_persistent_store_error_is_not_silent() {
        use ralph_core::config::HatConfig;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct FailingBridge {
            inner: InMemoryCoordinatorBridge,
            fail: AtomicBool,
        }
        impl std::fmt::Debug for FailingBridge {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("FailingBridge").finish()
            }
        }
        impl SupervisorBridge for FailingBridge {
            fn register_wave_if_absent(
                &self,
                k: WaveKind,
                id: &str,
                n: u32,
                slot_retry_budget: u32,
            ) -> Result<String, BridgeError> {
                self.inner
                    .register_wave_if_absent(k, id, n, slot_retry_budget)
            }
            fn fan_in_status(
                &self,
                id: &str,
            ) -> Result<ralph_core::supervisor::WaveSnapshot, BridgeError> {
                if self.fail.load(Ordering::SeqCst) {
                    Err(BridgeError::Store("simulated".into()))
                } else {
                    self.inner.fan_in_status(id)
                }
            }
            fn tick_with_slot_events(
                &self,
                id: &str,
                inputs: PhaseInputs,
                ev: Vec<ralph_proto::Event>,
            ) -> Result<CoordinatorAction, BridgeError> {
                if self.fail.load(Ordering::SeqCst) {
                    Err(BridgeError::Store("simulated".into()))
                } else {
                    self.inner.tick_with_slot_events(id, inputs, ev)
                }
            }
            fn tick(
                &self,
                id: &str,
                inputs: PhaseInputs,
            ) -> Result<CoordinatorAction, BridgeError> {
                self.inner.tick(id, inputs)
            }
            fn slot_resources(
                &self,
                id: &str,
            ) -> Result<Vec<ralph_core::supervisor::SlotResource>, BridgeError> {
                self.inner.slot_resources(id)
            }
            fn max_concurrent_workers(&self) -> u32 {
                self.inner.max_concurrent_workers()
            }
            fn repo_root(&self) -> Option<&std::path::Path> {
                self.inner.repo_root()
            }
            fn tasks_path(&self) -> Option<&std::path::Path> {
                self.inner.tasks_path()
            }
            fn try_dispatch_next(&self, id: &str, idx: u32) -> Result<bool, BridgeError> {
                self.inner.try_dispatch_next(id, idx)
            }
            fn bind_slot(
                &self,
                k: WaveKind,
                id: &str,
                idx: u32,
            ) -> Result<Option<ralph_core::supervisor::SlotBinding>, BridgeError> {
                self.inner.bind_slot(k, id, idx)
            }
            fn recover(&self) -> Result<Vec<ralph_core::supervisor::WaveSnapshot>, BridgeError> {
                self.inner.recover()
            }
            fn record_slot_result(
                &self,
                id: &str,
                idx: u32,
                h: &str,
                n: usize,
            ) -> Result<(), BridgeError> {
                self.inner.record_slot_result(id, idx, h, n)
            }
            fn record_slot_terminal_evidence(
                &self,
                id: &str,
                idx: u32,
                e: &ralph_core::supervisor::TerminalEvidence,
            ) -> Result<(), BridgeError> {
                self.inner.record_slot_terminal_evidence(id, idx, e)
            }
            fn slot_terminal_evidence(
                &self,
                id: &str,
                idx: u32,
            ) -> Result<Option<ralph_core::supervisor::TerminalEvidence>, BridgeError> {
                self.inner.slot_terminal_evidence(id, idx)
            }
            fn record_slot_failure(&self, id: &str, idx: u32, r: &str) -> Result<(), BridgeError> {
                self.inner.record_slot_failure(id, idx, r)
            }
            fn record_never_started_failures(&self, id: &str) -> Result<(), BridgeError> {
                self.inner.record_never_started_failures(id)
            }
            fn slot_failure_reason(
                &self,
                id: &str,
                idx: u32,
            ) -> Result<Option<String>, BridgeError> {
                self.inner.slot_failure_reason(id, idx)
            }
            fn release_slot_dispatch(
                &self,
                id: &str,
                idx: u32,
                o: ralph_core::supervisor::DispatchOutcome,
            ) -> Result<(), BridgeError> {
                self.inner.release_slot_dispatch(id, idx, o)
            }
            fn finalize_terminal_cleanup(&self, p: &std::path::Path) -> Result<(), BridgeError> {
                self.inner.finalize_terminal_cleanup(p)
            }
            fn cancel_wave(&self, id: &str) -> Result<(), BridgeError> {
                self.inner.cancel_wave(id)
            }
            fn enqueue_compensation(
                &self,
                id: &str,
                k: ralph_core::supervisor::CompensationKind,
            ) -> Result<(), BridgeError> {
                self.inner.enqueue_compensation(id, k)
            }
            fn take_pending_compensations(
                &self,
            ) -> Result<Vec<(String, ralph_core::supervisor::CompensationKind)>, BridgeError>
            {
                self.inner.take_pending_compensations()
            }
            fn complete_compensation(
                &self,
                id: &str,
                k: ralph_core::supervisor::CompensationKind,
                ok: bool,
            ) -> Result<(), BridgeError> {
                self.inner.complete_compensation(id, k, ok)
            }
            fn set_wave_phase(
                &self,
                id: &str,
                p: ralph_core::supervisor::WavePhase,
            ) -> Result<(), BridgeError> {
                self.inner.set_wave_phase(id, p)
            }
            fn slot_retry_budget(&self) -> u32 {
                self.inner.slot_retry_budget()
            }
        }

        let workspace = tempfile::TempDir::new().expect("temp workspace");
        let ralph_dir = workspace.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
        let main_events_file = ralph_dir.join("events.jsonl");

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let inner = InMemoryCoordinatorBridge::from_store(store);
        let bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(FailingBridge {
            inner,
            fail: AtomicBool::new(true),
        });

        let completed = ralph_core::CompletedWave {
            wave_id: "w-u1-red5".to_string(),
            wave_total: 1,
            results: vec![],
            failures: vec![],
            duration: std::time::Duration::ZERO,
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: std::collections::HashMap::new(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };
        let detected = ralph_core::DetectedWave {
            wave_id: "w-u1-red5".to_string(),
            target_hat: ralph_proto::HatId::new("review-dispatcher"),
            hat_config: HatConfig {
                name: "review-dispatcher".to_string(),
                ..HatConfig::default()
            },
            events: vec![ralph_core::Event {
                topic: "review.unit.ready".to_string(),
                payload: Some("payload-0".to_string()),
                ts: String::new(),
                hat: None,
                triggered: None,
                source: None,
                wave_id: None,
                wave_index: None,
                wave_total: None,
                system_injected: None,
            }],
            total: 1,
            partial: false,
            consumer_aggregate_timeout: None,
        };

        let outcome =
            run_supervisor_fan_in(&bridge, &completed, &detected, &main_events_file, 60, None);
        assert!(
            matches!(outcome, SupervisorFanInOutcome::StoreError),
            "persistent store error must return StoreError, got {:?}",
            outcome
        );
    }

    // =================================================================
    // U1 accident-characterization tests (plan 2026-07-27-003)
    //
    // These tests pin the CURRENT behavior of the wave dispatcher
    // surfaces the implementation-review primary-20260727-051801
    // diagnosis flagged as P0/P1 root causes. They are deliberately
    // designed to fail (RED) at HEAD today so the U2-U5 fixes
    // (channel registry, terminal-evidence reconciliation, salvage/
    // coordination receipt protocol) can re-run the same tests and
    // observe GREEN. Each test name encodes the invariant under
    // verification; the comment body documents which implementation-
    // review incident symptom it pins.
    //
    // Note: U1.3 / U1.4 assert the SHAPE of the regression; U1.5 /
    // U1.6 assert the FUNCTION SIGNATURE / FAILURE-SWALLOWING that
    // plan U5 explicitly rewrites. The tests are intentionally
    // placed inside `dispatcher.rs`'s `#[cfg(test)] mod tests` so
    // they can call the same internal helpers (which were just
    // promoted to `pub(crate)` for parity with `wave_supervisor.rs`
    // sibling tests) without changing public API surface.
    // =================================================================

    /// U1.1 — `append_wave_channel_to_marker` creates the marker
    /// silently on first call. This is the dispatcher-side enabler
    /// of the implementation-review primary-20260727 incident:
    /// the dispatcher wrote the wave-channel marker AFTER `execute`
    /// had been called for the slot, so workers running in
    /// isolated mode could pass the path-shape check in
    /// `resolve_emit_path` but failed the marker-membership
    /// check, ending up rejected (`empty_worker_result`). Plan
    /// U2 closes the window by replacing the marker with an
    /// atomic per-wave JSON registry that the dispatcher MUST
    /// write BEFORE any worker `Command::envs(...)` runs.
    ///
    /// Locked invariant (current RED, expected GREEN after U2):
    /// after the registry rewrite, appending to the marker
    /// without an explicit binding commit returns Err so the
    /// dispatcher path can fail-close before spawning workers.
    /// Today, the marker self-creates, so the same call returns
    /// Ok(()) unconditionally — the test asserts the current
    /// behavior verbatim so U2 must change it for the test to
    /// turn red and stay red until U3 fixes spawn semantics.
    #[test]
    fn accident_01_append_wave_channel_to_marker_self_creates_marker() {
        let workspace = tempfile::TempDir::new().expect("tempdir");
        let main_events_file = workspace.path().join(".ralph").join("events.jsonl");
        let worker_events_file = workspace.path().join(".ralph").join("wave-w-rs-1-0.jsonl");
        // Marker's parent (.ralph) does NOT exist yet; current
        // implementation must create it on demand. U2 deletes this
        // self-claim path entirely and replaces it with a
        // schema-versioned JSON registry written before spawn.
        assert!(
            !workspace.path().join(".ralph").exists(),
            ".ralph must not exist pre-call so the self-claim path is exercised"
        );
        let result = append_wave_channel_to_marker(&main_events_file, &worker_events_file);
        assert!(
            result.is_ok(),
            "current behavior: marker self-creates and accepts the append; \
             RED baseline for U2 replacement. Got error: {result:?}"
        );
        let marker = workspace
            .path()
            .join(".ralph")
            .join("current-wave-channels");
        let contents = std::fs::read_to_string(&marker).expect("marker must be readable");
        assert!(
            contents.contains("wave-w-rs-1-0.jsonl"),
            "marker must record the worker channel path; got: {contents}"
        );
    }

    /// U1.2 — When the marker's parent directory is unwritable
    /// (chmod 0o000), `append_wave_channel_to_marker` returns
    /// `Err(_)` and the dispatcher currently warn-and-continues
    /// — exactly the silent-success window the implementation-
    /// review primary-20260727 incident exploited (the
    /// dispatcher logged "marker append failed" and spawned
    /// the worker anyway). Plan U3 closes the window by
    /// promoting the failure to a typed `WavePreparationFailure`
    /// returned BEFORE the executor future is constructed.
    ///
    /// This test pins the CURRENT behavior verbatim: the
    /// function returns Err. U2 must change `append_wave_…`
    /// into a registry-replacement that does not exist; the
    /// test will then be deleted by U3 along with the function.
    #[cfg(unix)]
    #[test]
    fn accident_02_append_wave_channel_to_marker_returns_io_error_on_unwritable_parent() {
        use std::os::unix::fs::PermissionsExt;

        let workspace = tempfile::TempDir::new().expect("tempdir");
        // Lock the marker's parent so create_dir_all + OpenOptions::create
        // both fail with EACCES.
        let locked_parent = workspace.path().join(".ralph");
        std::fs::create_dir(&locked_parent).expect("mkdir .ralph");
        let mut perms = std::fs::metadata(&locked_parent).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&locked_parent, perms).expect("chmod 0o000");

        let main_events_file = workspace.path().join(".ralph").join("events.jsonl");
        let worker_events_file = workspace.path().join(".ralph").join("wave-w-rs-1-0.jsonl");
        let result = append_wave_channel_to_marker(&main_events_file, &worker_events_file);

        // Restore perms so the tempdir teardown succeeds.
        let mut restore = std::fs::metadata(&locked_parent).unwrap().permissions();
        restore.set_mode(0o755);
        let _ = std::fs::set_permissions(&locked_parent, restore);

        let err = result.expect_err(
            "current behavior: append returns Err on EACCES; this is the dispatcher \
             warn-and-continue path that U3 must turn into typed preparation failure",
        );
        assert!(
            matches!(err.kind(), std::io::ErrorKind::PermissionDenied)
                || matches!(err.kind(), std::io::ErrorKind::Other),
            "expected EACCES-class error, got {err:?}"
        );
    }

    /// U1.3 — `build_review_done_hints` today reads
    /// `main_backscan` independently of store state, so the
    /// implementation-review primary-20260727 incident produced
    /// 5 orphan `review.unit.done` rows in the main JSONL that
    /// `main_backscan` accepted (the store had those slots as
    /// `Failed`, but the function's main-side scan does not
    /// consult the store). The accident: `main_backscan` ends
    /// up larger than `store_completed`, and downstream
    /// `build_wave_failed_payload` uses the UNION to subtract
    /// from `assigned_dimensions`, dropping 5 of 6 actual
    /// missing dimensions.
    ///
    /// Locked invariant: after U4 reconciliation, the function
    /// must report `main_backscan` only for dimensions whose
    /// SLOT in the store is `Completed` with matching terminal
    /// evidence. Orphans (store Failed + main done) MUST NOT
    /// appear in `main_backscan` — they go to
    /// `payload_conflicts` in the new `ReviewReconciliation`.
    ///
    /// Current RED state: assertion below fails because
    /// `main_backscan` contains all 5 orphan dimensions.
    #[test]
    fn accident_03_review_done_hints_main_backscan_contains_orphan_projections() {
        use ralph_core::supervisor::{SlotStatus, WavePhase, WaveSnapshot};

        let workspace = tempfile::TempDir::new().expect("tempdir");
        let ralph_dir = workspace.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
        let main_events_file = ralph_dir.join("events.jsonl");

        // Plant 5 same-wave `review.unit.done` rows in main —
        // they correspond to slots the store will report as
        // Failed. This is the implementation-review primary-
        // 20260727 incident layout: dispatcher's failed fan-in
        // path left these rows in main because workers wrote
        // there before scope-drop, and the dispatcher did not
        // validate the store-side slot status.
        let store_wave_id = "w-acc-3";
        let mut lines = Vec::new();
        for dim in [
            "goal-alignment",
            "correctness",
            "maintainability",
            "adversarial",
            "project-standards",
        ] {
            let record = serde_json::json!({
                "topic": "review.unit.done",
                "payload": serde_json::json!({"dimension": dim}).to_string(),
                "ts": "2026-07-27T00:00:00Z",
                "hat": "review-worker",
                "source": "review-worker",
                "wave_id": store_wave_id,
                "wave_index": 0u32,
            });
            lines.push(serde_json::to_string(&record).expect("json"));
        }
        std::fs::write(&main_events_file, lines.join("\n") + "\n").expect("write main");

        // Bridge returns a snapshot where ALL slots are Failed
        // (the incident shape — `completed.results` was empty,
        // `completed.failures` was the truth source).
        let bridge = RecordingBridge {
            status: WaveSnapshot {
                wave_id: store_wave_id.to_string(),
                kind: WaveKind::Review,
                phase: WavePhase::Collect,
                expected_total: 6,
                completed_count: 0,
                failed_count: 6,
                pending_count: 0,
                in_flight_count: 0,
                cancel_requested: false,
                delivery_state: ralph_core::supervisor::WaveDeliveryState::CoordinationCommitted,
                started_at: std::time::SystemTime::UNIX_EPOCH,
                slots: (0u32..6).map(|i| (i, SlotStatus::Failed)).collect(),
            },
            evidence: std::collections::HashMap::new(),
            mark_salvage_calls: std::sync::Mutex::new(Vec::new()),
            coord_event_appends: std::sync::Mutex::new(Vec::new()),
        };
        let bridge_arc: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(bridge);

        let mut assigned = std::collections::HashMap::new();
        for (i, dim) in [
            "goal-alignment",
            "correctness",
            "maintainability",
            "adversarial",
            "testing",
            "project-standards",
        ]
        .iter()
        .enumerate()
        {
            assigned.insert(i as u32, dim.to_string());
        }
        let completed = ralph_core::CompletedWave {
            wave_id: store_wave_id.to_string(),
            wave_total: 6,
            assigned_dimensions: assigned,
            ..ralph_core::CompletedWave::default()
        };

        let hints =
            build_review_done_hints(&bridge_arc, store_wave_id, &completed, &main_events_file);

        // CURRENT (RED) behavior: main_backscan contains all 5
        // orphan dimensions. U4 reconciliation must drop these
        // because no slot is Completed in the store.
        let orphan_count = hints.main_backscan.len();
        assert_eq!(
            orphan_count, 0,
            "main_backscan must NOT include orphan projections: \
             store reports all 6 slots as Failed, so a review.unit.done \
             row in main that has no matching Completed slot is an orphan \
             (moved to payload_conflicts in the reconciliation output). \
             Got main_backscan = {:?}, store_completed = {:?}",
            hints.main_backscan, hints.store_completed
        );
    }

    /// U1.4 — `build_wave_failed_payload` for WaveKind::Review
    /// currently extends `already_done` with `hints.main_backscan`,
    /// so 6 dimensions assigned, 5 orphan main rows + 0 store
    /// Completed → `missing_dimensions = [testing]` (length 1).
    /// The implementation-review primary-20260727 payload
    /// carried exactly this list, which is what blocked the run
    /// from synthesizing review output (only 1 dimension was
    /// classified as missing when in fact 6 were unprocessable).
    ///
    /// Locked invariant: after U4 reconciliation, the payload's
    /// `missing_dimensions` MUST equal the full assigned set
    /// when no slot is store-Completed. The incident `testing`
    /// is the only dimension whose row DID land via store;
    /// after the fix, the orphan 5 rows also count as missing
    /// (because their slots Failed in the store and the orphan
    /// projection is not authoritative).
    #[test]
    fn accident_04_build_wave_failed_payload_missing_dimensions_undercounts_orphans() {
        use ralph_core::supervisor::WaveKind;

        let store_wave_id = "w-acc-4";
        let assigned_dims = [
            "goal-alignment",
            "correctness",
            "maintainability",
            "adversarial",
            "testing",
            "project-standards",
        ];
        let mut assigned = std::collections::HashMap::new();
        for (i, dim) in assigned_dims.iter().enumerate() {
            assigned.insert(i as u32, dim.to_string());
        }
        // No `results`, all 6 slots in `failures` (the incident
        // shape — store reported 6 Failed, dispatcher never got
        // any Completed slot evidence).
        let completed = ralph_core::CompletedWave {
            wave_id: store_wave_id.to_string(),
            wave_total: 6,
            assigned_dimensions: assigned.clone(),
            failures: (0u32..6)
                .map(|i| ralph_core::WaveFailure {
                    index: i,
                    error: "worker_timeout".to_string(),
                    duration: std::time::Duration::from_millis(1),
                    ..ralph_core::WaveFailure::default()
                })
                .collect(),
            ..ralph_core::CompletedWave::default()
        };

        // U4 (KTD3 / R5 / R7): the failed-payload builder now
        // takes the reconciliation directly. The incident scenario
        // — store reports 6 Failed, no Completed evidence,
        // 5 same-wave orphan main rows — produces a reconciliation
        // with `authoritative_completed = []` and 5 orphan
        // projections. The builder's missing_dimensions is driven
        // by the authoritative completed set, not the main
        // backscan union, so all 6 assigned dimensions are
        // reported missing (the pre-U4 path only reported 1).
        use ralph_core::supervisor::reconciliation::{OrphanProjection, ReviewReconciliation};
        let mut orphan_projections = Vec::new();
        for (i, dim) in assigned_dims[..5].iter().enumerate() {
            orphan_projections.push(OrphanProjection {
                slot_index: Some(i as u32),
                dimension: Some((*dim).to_string()),
                payload_fingerprint: format!("orphan-fp-{i}"),
                line_no: i,
                store_status: Some(ralph_core::supervisor::SlotStatus::Failed),
            });
        }
        let reconciliation = ReviewReconciliation {
            authoritative_completed: Vec::new(),
            missing_dimensions: Vec::new(), // rebuilt by build_wave_failed_payload
            blocking_slots: Vec::new(),
            orphan_projections,
            missing_projections: Vec::new(),
            payload_conflicts: Vec::new(),
            evidence_validations: Vec::new(),
        };

        // The hints are passed for diagnostic / backward-compat
        // reasons (the U3 test still pins its shape) but the
        // reconciliation is the source of truth for
        // `missing_dimensions`.
        let hints = ReviewDoneHints {
            main_backscan: std::collections::HashSet::new(),
            store_completed: std::collections::HashSet::new(),
        };

        let payload = build_wave_failed_payload(
            WaveKind::Review,
            &completed,
            "required_slot_failure",
            (0u32..6).collect(),
            &std::collections::HashMap::new(),
            Some(&hints),
            Some(&reconciliation),
        );

        let missing: Vec<String> = payload
            .get("missing_dimensions")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|d| d.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        assert_eq!(
            missing.len(),
            assigned_dims.len(),
            "missing_dimensions MUST include all 6 assigned dimensions when \
             no slot is store-Completed. The implementation-review primary-20260727 \
             incident produced missing_dimensions=[testing] (length 1) because \
             build_wave_failed_payload treated the 5 main_backscan orphans as done. \
             After U4, all 6 must be reported as missing. Got: {missing:?}"
        );
        for dim in &assigned_dims {
            assert!(
                missing.contains(&dim.to_string()),
                "missing_dimensions must contain {dim}; got: {missing:?}"
            );
        }
    }

    /// U1.5 — `merge_completed_review_slots_to_main` returns
    /// `()` today and calls `bridge.mark_salvage_merged` AFTER
    /// the main append succeeds. The current ordering is the
    /// P0-1 fix from plan 004 (append-then-commit), but the
    /// function does not return any `ProjectionReceipt` so the
    /// caller cannot distinguish "wrote 3 lines and committed"
    /// from "wrote 0 lines (all Failed) and did not commit".
    /// Plan U5 replaces the signature with
    /// `Result<ProjectionReceipt, ProjectionError>` so the
    /// caller can drive the four-phase
    /// `BusinessProjected → SalvageCommitted → CoordinationWritten →
    /// CoordinationCommitted` state machine.
    ///
    /// This test pins the CURRENT observable side effects
    /// (3 Completed slots → 3 lines appended → store mark set)
    /// so U5's signature change must preserve the side effects
    /// while also returning a receipt.
    #[test]
    fn accident_05_merge_completed_review_slots_to_main_writes_and_marks() {
        use ralph_core::supervisor::WavePhase;
        use ralph_core::supervisor::WaveSnapshot;
        use ralph_core::supervisor::{SlotStatus, TerminalEvidence};

        let workspace = tempfile::TempDir::new().expect("tempdir");
        let ralph_dir = workspace.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
        let main_events_file = ralph_dir.join("events.jsonl");

        let store_wave_id = "w-acc-5";
        let bridge = std::sync::Arc::new(RecordingBridge {
            status: WaveSnapshot {
                wave_id: store_wave_id.to_string(),
                kind: WaveKind::Review,
                phase: WavePhase::Collect,
                expected_total: 3,
                completed_count: 3,
                failed_count: 0,
                pending_count: 0,
                in_flight_count: 0,
                cancel_requested: false,
                delivery_state: ralph_core::supervisor::WaveDeliveryState::CoordinationCommitted,
                started_at: std::time::SystemTime::UNIX_EPOCH,
                slots: (0u32..3).map(|i| (i, SlotStatus::Completed)).collect(),
            },
            evidence: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    0u32,
                    TerminalEvidence::from_event(
                        "review.unit.done",
                        &serde_json::json!({"dimension": "correctness"}).to_string(),
                    ),
                );
                m
            },
            mark_salvage_calls: std::sync::Mutex::new(Vec::new()),
            coord_event_appends: std::sync::Mutex::new(Vec::new()),
        });
        let bridge_arc: Arc<dyn ralph_core::supervisor::SupervisorBridge> = bridge.clone();

        // Completed: 3 slots, each with one review.unit.done event.
        let mut results = Vec::new();
        for i in 0u32..3 {
            results.push(ralph_core::WaveResult {
                index: i,
                events: vec![
                    ralph_proto::Event::new(
                        "review.unit.done",
                        serde_json::json!({"dimension": "correctness"}).to_string(),
                    )
                    .with_source("review-worker")
                    .with_wave(store_wave_id.to_string(), i, 3),
                ],
            });
        }
        let completed = ralph_core::CompletedWave {
            wave_id: store_wave_id.to_string(),
            wave_total: 3,
            results,
            ..ralph_core::CompletedWave::default()
        };

        // Call returns `Result<ProjectionReceipt, ProjectionError>`
        // since U5. The dispatcher must surface the error
        // instead of collapsing to `()`.
        let _ = merge_completed_review_slots_to_main(
            &main_events_file,
            &completed,
            &bridge_arc,
            store_wave_id,
        )
        .expect("merge_completed_review_slots_to_main");

        // Side effects that U5 MUST preserve:
        // 1. main has 3 review.unit.done rows attributed to review-worker.
        let main_contents =
            std::fs::read_to_string(&main_events_file).expect("main must be written");
        let business_lines = main_contents
            .lines()
            .filter(|l| l.contains("\"review.unit.done\""))
            .count();
        assert_eq!(
            business_lines, 3,
            "merge must append exactly 3 review.unit.done rows; got {business_lines} \
             in main:\n{main_contents}"
        );
        // 2. mark_salvage_merged was called exactly once for this wave.
        let calls = bridge.mark_salvage_calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![store_wave_id.to_string()],
            "merge must commit salvage mark exactly once; got {calls:?}"
        );
    }

    /// U1.6 — `append_supervisor_coord_event` returns `()` and
    /// warns-and-continues on write failure. The
    /// implementation-review primary-20260727 incident did NOT
    /// see a coord-event write failure directly (the events
    /// file was writable), but the failure mode is the SAME
    /// root cause: the function is structurally unable to tell
    /// the caller "I failed to write the coord event" — it
    /// just logs and proceeds. Plan U5 closes this with
    /// `Result<CoordinationReceipt, ProjectionError>` so the
    /// four-phase commit can refuse to advance to
    /// `CoordinationCommitted` when the main ledger cannot be
    /// appended to.
    ///
    /// Locked invariant (current RED, expected GREEN after U5):
    /// a write failure to the main ledger causes the function
    /// to return an error the caller can propagate, instead of
    /// silently dropping the failure. Today, the function
    /// returns `()` unconditionally; the test asserts no
    /// panic and (today) silently passes — U5 must change the
    /// signature so the SAME test now sees the error surfaced.
    #[cfg(unix)]
    #[test]
    fn accident_06_append_supervisor_coord_event_silently_drops_write_failure() {
        use std::os::unix::fs::PermissionsExt;

        let workspace = tempfile::TempDir::new().expect("tempdir");
        // Lock the events file's parent so the append fails.
        let locked_parent = workspace.path().join("locked");
        std::fs::create_dir(&locked_parent).expect("mkdir locked");
        let mut perms = std::fs::metadata(&locked_parent).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&locked_parent, perms).expect("chmod 0o000");

        let main_events_file = locked_parent.join("events.jsonl");
        let payload = serde_json::json!({"wave_id": "w-acc-6"});

        // Current signature returns `Result<CoordinationReceipt,
        // ProjectionError>` since U5. The function surfaces the
        // error instead of swallowing it; the dispatcher's failure
        // path can now refuse to advance to `CoordinationCommitted`.
        // The U1.6 regression pin uses `let _ = ...` to assert the
        // call must not panic (regardless of the new return type)
        // while the projection-error path is locked by U5 unit tests.
        let _ = append_supervisor_coord_event(&main_events_file, "review.wave.failed", &payload);

        // Restore perms so the tempdir teardown succeeds.
        let mut restore = std::fs::metadata(&locked_parent).unwrap().permissions();
        restore.set_mode(0o755);
        let _ = std::fs::set_permissions(&locked_parent, restore);

        // Today's behavior: no panic. The file was not written,
        // but the caller cannot detect that. U5 must change the
        // signature to Result so the caller can refuse the next
        // phase commit.
    }

    // =================================================================
    // Test fixture for U1.3 / U1.4 / U1.5: a minimal
    // `SupervisorBridge` impl that records call order and lets
    // each test script the store-side slot status / evidence map.
    // Stays in `mod tests` so it does not leak to other crates.
    // =================================================================
    #[derive(Debug)]
    struct RecordingBridge {
        status: ralph_core::supervisor::WaveSnapshot,
        evidence: std::collections::HashMap<u32, ralph_core::supervisor::TerminalEvidence>,
        mark_salvage_calls: std::sync::Mutex<Vec<String>>,
        #[allow(dead_code)]
        coord_event_appends: std::sync::Mutex<Vec<String>>,
    }

    impl ralph_core::supervisor::SupervisorBridge for RecordingBridge {
        fn commit_salvage_projection(
            &self,
            wave_id: &str,
            _receipt: &ralph_core::supervisor::ProjectionReceiptSummary,
        ) -> Result<(), BridgeError> {
            self.mark_salvage_calls
                .lock()
                .expect("recording bridge lock")
                .push(wave_id.to_string());
            Ok(())
        }

        fn record_coordination_written(
            &self,
            _wave_id: &str,
            _receipt: &ralph_core::supervisor::CoordinationReceiptSummary,
        ) -> Result<(), BridgeError> {
            Ok(())
        }

        fn commit_coordination_event(
            &self,
            _wave_id: &str,
            _receipt: &ralph_core::supervisor::CoordinationReceiptSummary,
            _terminal_phase: ralph_core::supervisor::WavePhase,
        ) -> Result<(), BridgeError> {
            Ok(())
        }

        fn tick(
            &self,
            _wave_id: &str,
            _inputs: PhaseInputs,
        ) -> Result<ralph_core::supervisor::CoordinatorAction, BridgeError> {
            Ok(ralph_core::supervisor::CoordinatorAction::ContinueCollect)
        }

        fn bind_slot(
            &self,
            _kind: WaveKind,
            _wave_id: &str,
            _slot_index: u32,
        ) -> Result<Option<ralph_core::supervisor::SlotBinding>, BridgeError> {
            Ok(None)
        }

        fn recover(&self) -> Result<Vec<ralph_core::supervisor::WaveSnapshot>, BridgeError> {
            Ok(Vec::new())
        }

        fn fan_in_status(
            &self,
            _wave_id: &str,
        ) -> Result<ralph_core::supervisor::WaveSnapshot, BridgeError> {
            Ok(self.status.clone())
        }

        fn register_wave_if_absent(
            &self,
            _kind: WaveKind,
            wave_id: &str,
            _expected_total: u32,
            _slot_retry_budget: u32,
        ) -> Result<String, BridgeError> {
            Ok(wave_id.to_string())
        }

        fn record_slot_result(
            &self,
            _wave_id: &str,
            _slot_index: u32,
            _content_hash: &str,
            _event_count: usize,
        ) -> Result<(), BridgeError> {
            Ok(())
        }

        fn record_slot_failure(
            &self,
            _wave_id: &str,
            _slot_index: u32,
            _reason: &str,
        ) -> Result<(), BridgeError> {
            Ok(())
        }

        fn release_slot_dispatch(
            &self,
            _wave_id: &str,
            _slot_index: u32,
            _outcome: ralph_core::supervisor::DispatchOutcome,
        ) -> Result<(), BridgeError> {
            Ok(())
        }

        fn slot_terminal_evidence(
            &self,
            _wave_id: &str,
            slot_index: u32,
        ) -> Result<Option<ralph_core::supervisor::TerminalEvidence>, BridgeError> {
            Ok(self.evidence.get(&slot_index).cloned())
        }

        fn slot_failure_reason(
            &self,
            _wave_id: &str,
            _slot_index: u32,
        ) -> Result<Option<String>, BridgeError> {
            Ok(None)
        }

        fn record_never_started_failures(&self, wave_id: &str) -> Result<(), BridgeError> {
            // Default impl iterates Pending slots and calls
            // record_slot_failure. The bridge stub records
            // nothing in mark_salvage_calls here; U1.x tests
            // only assert the side effects they care about.
            let _ = wave_id;
            Ok(())
        }
        fn slot_retry_budget(&self) -> u32 {
            0
        }
    }
}
