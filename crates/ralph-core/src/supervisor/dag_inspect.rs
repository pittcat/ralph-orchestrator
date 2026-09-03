//! 2026-09-03-0959 plan U5 (R12; S14; D13; E16): sanitized
//! shadow inspect summary. Distinct from the legacy
//! `SupervisorInspectSummary` (in `mod.rs`) which reads from
//! `SupervisorStore`; this one reads from
//! [`crate::supervisor::dag_shadow::ShadowSink`] only. NO raw
//! payload / DB paths / agent prompt / secrets surface
//! (R12 / E16 / S14). `scheduler_mode` is caller-supplied —
//! the sink itself is mode-agnostic.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::supervisor::dag_shadow::ShadowSink;

/// Sanitized shadow summary. Counts + plan_keys only — all
/// operator-supplied identifiers the agent already knows through
/// `task_id` / `plan_key` channels.
///
/// Fields (U5 S14 / D13):
///   - `scheduler_mode` — stringified `SchedulerMode`
///     (`legacy_wave` / `runtime_dag` / `runtime_dag_shadow`).
///   - `plan_keys` — distinct `plan_key`s, deduped + sorted.
///   - `oldest_observation_ms` — min `observed_at_ms`, `None`
///     when sink is empty.
///   - `total_observations` / `admitted_total` / `blocked_total`
///     — sums across all observations (operators want one
///     "how many waiting" number).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SchedulerInspectSummary {
    pub scheduler_mode: String,
    pub plan_keys: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest_observation_ms: Option<u64>,
    pub total_observations: u64,
    pub admitted_total: u64,
    pub blocked_total: u64,
}

impl SchedulerInspectSummary {
    /// Fresh empty summary with a given scheduler mode.
    pub fn for_mode(scheduler_mode: impl Into<String>) -> Self {
        Self {
            scheduler_mode: scheduler_mode.into(),
            ..Self::default()
        }
    }

    /// Aggregate from the shadow sink. `scheduler_mode` is
    /// caller-supplied (sink is mode-agnostic).
    pub fn from_shadow_sink(sink: &ShadowSink, scheduler_mode: &str) -> Self {
        let mut plan_keys: BTreeSet<String> = BTreeSet::new();
        let mut oldest: Option<u64> = None;
        let mut total_observations: u64 = 0;
        let mut admitted_total: u64 = 0;
        let mut blocked_total: u64 = 0;
        sink.with_observations(|obs| {
            total_observations = obs.len() as u64;
            for o in obs {
                plan_keys.insert(o.plan_key.clone());
                oldest = Some(match oldest {
                    Some(prev) => prev.min(o.observed_at_ms),
                    None => o.observed_at_ms,
                });
                admitted_total = admitted_total.saturating_add(o.admitted_count as u64);
                blocked_total = blocked_total
                    .saturating_add(o.blocked_dependency_count as u64)
                    .saturating_add(o.blocked_resource_count as u64)
                    .saturating_add(o.blocked_cap_count as u64);
            }
        });
        Self {
            scheduler_mode: scheduler_mode.to_string(),
            plan_keys: plan_keys.into_iter().collect(),
            oldest_observation_ms: oldest,
            total_observations,
            admitted_total,
            blocked_total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor::dag_shadow::{ShadowObservation, ShadowSink};

    fn obs(
        plan_key: &str,
        tick: u64,
        observed_at_ms: u64,
        admitted: u32,
        dep: u32,
        res: u32,
        cap: u32,
    ) -> ShadowObservation {
        ShadowObservation {
            plan_key: plan_key.to_string(),
            tick_epoch: tick,
            observed_at_ms,
            ready_count: admitted + dep + res + cap,
            admitted_count: admitted,
            blocked_dependency_count: dep,
            blocked_resource_count: res,
            blocked_cap_count: cap,
            decisions: vec![],
        }
    }

    /// Empty sink → zero counts + `oldest_observation_ms = None`.
    #[test]
    fn inspect_summary_returns_zero_when_sink_empty() {
        let sink = ShadowSink::new();
        let summary = SchedulerInspectSummary::from_shadow_sink(&sink, "runtime_dag_shadow");
        assert_eq!(summary.scheduler_mode, "runtime_dag_shadow");
        assert!(summary.plan_keys.is_empty());
        assert_eq!(summary.oldest_observation_ms, None);
        assert_eq!(summary.total_observations, 0);
        assert_eq!(summary.admitted_total, 0);
        assert_eq!(summary.blocked_total, 0);
    }

    /// Serialized summary must not contain raw payload, DB paths,
    /// secrets, or operator-host-path strings.
    #[test]
    fn inspect_summary_contains_no_raw_payload_or_paths() {
        let sink = ShadowSink::new();
        sink.record(obs("plan-A", 0, 100, 2, 1, 0, 0));
        sink.record(obs("plan-B", 1, 200, 1, 0, 1, 1));
        let summary = SchedulerInspectSummary::from_shadow_sink(&sink, "runtime_dag_shadow");
        let json = serde_json::to_string(&summary).expect("serialize summary");
        for forbidden in [
            "payload", "secret", "password", "/home/", "/root/", "/tmp/", "/var/", "token=",
            "Bearer ", "fn ", "use ", ".jsonl", ".db",
        ] {
            assert!(
                !json.contains(forbidden),
                "summary JSON must not contain forbidden substring {forbidden:?}: {json}"
            );
        }
        // Positive controls: plan_keys + totals + mode surface.
        assert!(json.contains("plan-A"));
        assert!(json.contains("plan-B"));
        assert!(json.contains("runtime_dag_shadow"));
        assert!(json.contains("\"total_observations\":2"));
        assert!(json.contains("\"admitted_total\":3"));
        // blocked_total = (1+0+0) + (0+1+1) = 3.
        assert!(json.contains("\"blocked_total\":3"));
    }

    /// `scheduler_mode` reflects the supervisor config. Three U1
    /// modes round-trip cleanly through both `from_shadow_sink`
    /// and `for_mode`.
    #[test]
    fn scheduler_mode_field_reflects_supervisor_config() {
        for mode in ["legacy_wave", "runtime_dag", "runtime_dag_shadow"] {
            let summary = SchedulerInspectSummary::from_shadow_sink(&ShadowSink::new(), mode);
            assert_eq!(summary.scheduler_mode, mode);
            let zero = SchedulerInspectSummary::for_mode(mode);
            assert_eq!(zero.scheduler_mode, mode);
            assert_eq!(zero.total_observations, 0);
        }
    }
}
