use super::super::*;
    use crate::event_loop::rejection::Rejection;

    fn rejection_with_ts(ts: &str) -> Rejection {
        Rejection {
            stage: crate::event_loop::rejection::RejectionStage::Policy,
            source_hat: Some("review-worker".to_string()),
            business_hat: Some("review-worker".to_string()),
            topic: "review.unit.done".to_string(),
            violation: "test".to_string(),
            retry_key: "rk-1".to_string(),
            retry_eligible: true,
            non_retryable_reason: None,
            target_hat: Some("review-worker".to_string()),
            original_event_id: None,
            original_ts: Some(ts.to_string()),
            kind: None,
            duplicate_work_done_hint: None,
            seen_count: None,
        }
    }

    #[test]
    fn ttl_zero_disables_filter() {
        // ttl=0 must mean "do not filter" — used by scenario /
        // regression suites that pin aged fixtures.
        let rejection = rejection_with_ts("2024-01-01T00:00:00Z");
        assert!(!is_rejection_stale(&rejection, 0));
    }

    #[test]
    fn missing_original_ts_is_non_stale() {
        // Legacy / synthesised rejections without an
        // `original_ts` must survive the filter; otherwise the
        // failure-convergence path could lose recovery telemetry
        // it depends on.
        let mut rejection = rejection_with_ts("2024-01-01T00:00:00Z");
        rejection.original_ts = None;
        assert!(!is_rejection_stale(&rejection, 300));
    }

    #[test]
    fn past_older_than_ttl_is_stale() {
        // An event older than the 300s default is stale. We
        // build a ts 1000s in the past relative to `now`.
        let past = chrono::Utc::now() - chrono::Duration::seconds(1000);
        let rejection = rejection_with_ts(&past.to_rfc3339());
        assert!(
            is_rejection_stale(&rejection, 300),
            "1000s-old rejection with 300s TTL must be stale"
        );
    }

    #[test]
    fn future_timestamp_is_stale() {
        // Clock skew / forgery guard: a future ts means we
        // cannot trust the timestamp; the rejection must
        // NOT be re-injected into the loop.
        let future = chrono::Utc::now() + chrono::Duration::seconds(60);
        let rejection = rejection_with_ts(&future.to_rfc3339());
        assert!(is_rejection_stale(&rejection, 300));
    }

    #[test]
    fn default_config_default_is_300s() {
        // Pin the SSOT default. U7 records that plan 003 did
        // NOT widen / shrink this — the failure-convergence
        // path does not depend on stale-resume activation,
        // so we leave the default exactly where the
        // 2026-06-16-001 U3 plan left it.
        let cfg: crate::config::EventLoopConfig = Default::default();
        assert_eq!(
            cfg.task_resume_ttl_seconds,
            Some(300),
            "task_resume_ttl_seconds default must remain 300s; \
             changing this requires new plan coverage (U7 invariants)"
        );
    }
