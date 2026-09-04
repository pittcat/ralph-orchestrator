//! 2026-09-03-0959 plan U6 — read-only projection of the U5
//! `dag_shadow::ShadowSink` for the inspect command.
//!
//! U5 owns the write side (the sink records a
//! `ShadowObservation` per tick). U6 owns the read side — this
//! file gives the `inspect` command a typed handle that does
//! NOT mutate the sink and does NOT depend on the
//! `ralph_core::supervisor` internals beyond the U5 type.

#[cfg(test)]
use ralph_core::supervisor::dag_shadow::ShadowSink;

/// Aggregate read-side view the inspect command renders. The
/// fields mirror the U5 `ShadowObservation` shape but are
/// normalised for stable serialisation.
///
/// `#[cfg(test)]` for U6: the only consumer is the shadow
/// tests mod (which projects the U5 sink) and the inspect
/// integration test (which uses the reader to render a
/// summary). U7 promotes it once the inspect command renders
/// against a live U5 sink at runtime.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShadowSummary {
    pub plan_keys: Vec<String>,
    pub observation_count: usize,
    pub last_tick_epoch: u64,
    pub total_admitted: u64,
    pub total_blocked_dependency: u64,
    pub total_blocked_resource: u64,
    pub total_blocked_cap: u64,
}

/// Read-only reader. Holds a reference to the U5 sink.
///
/// `#[cfg(test)]` for U6 — see `ShadowSummary` rationale.
/// U7 promotes it once the inspect command renders against a
/// live U5 sink at runtime.
#[cfg(test)]
pub struct ShadowSinkReader<'a> {
    sink: &'a ShadowSink,
}

#[cfg(test)]
impl<'a> ShadowSinkReader<'a> {
    pub fn new(sink: &'a ShadowSink) -> Self {
        Self { sink }
    }

    /// Project the sink into a `ShadowSummary`.
    pub fn read(&self) -> ShadowSummary {
        let observations = self.sink.with_observations(|obs| obs.to_vec());
        let plan_keys = self.sink.list_plans();
        let mut summary = ShadowSummary {
            plan_keys,
            observation_count: observations.len(),
            ..ShadowSummary::default()
        };
        for obs in &observations {
            summary.last_tick_epoch = summary.last_tick_epoch.max(obs.tick_epoch);
            summary.total_admitted += u64::from(obs.admitted_count);
            summary.total_blocked_dependency += u64::from(obs.blocked_dependency_count);
            summary.total_blocked_resource += u64::from(obs.blocked_resource_count);
            summary.total_blocked_cap += u64::from(obs.blocked_cap_count);
        }
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_core::supervisor::dag_shadow::{ShadowObservation, compute_shadow_observation};

    /// Empty sink → empty summary.
    #[test]
    fn empty_sink_yields_empty_summary() {
        let sink = ShadowSink::new();
        let reader = ShadowSinkReader::new(&sink);
        let summary = reader.read();
        assert!(summary.plan_keys.is_empty());
        assert_eq!(summary.observation_count, 0);
        assert_eq!(summary.last_tick_epoch, 0);
    }

    /// Two observations aggregate counts and bump
    /// `last_tick_epoch` to the max.
    #[test]
    fn observations_aggregate_into_summary() {
        let sink = ShadowSink::new();
        let a = ShadowObservation {
            plan_key: "plan-A".to_string(),
            tick_epoch: 7,
            observed_at_ms: 0,
            ready_count: 3,
            admitted_count: 2,
            blocked_dependency_count: 1,
            blocked_resource_count: 0,
            blocked_cap_count: 0,
            decisions: Vec::new(),
        };
        let b = ShadowObservation {
            plan_key: "plan-B".to_string(),
            tick_epoch: 11,
            observed_at_ms: 1,
            ready_count: 5,
            admitted_count: 4,
            blocked_dependency_count: 0,
            blocked_resource_count: 1,
            blocked_cap_count: 1,
            decisions: Vec::new(),
        };
        sink.record(a);
        sink.record(b);
        let summary = ShadowSinkReader::new(&sink).read();
        assert_eq!(summary.observation_count, 2);
        assert_eq!(summary.last_tick_epoch, 11);
        assert_eq!(summary.total_admitted, 6);
        assert_eq!(summary.total_blocked_dependency, 1);
        assert_eq!(summary.total_blocked_resource, 1);
        assert_eq!(summary.total_blocked_cap, 1);
    }

    /// `compute_shadow_observation` (the U5 helper) writes
    /// observations; we exercise it here so a regression in
    /// the helper is caught even when the inspection reader
    /// is the consumer.
    #[test]
    fn compute_helper_round_trip() {
        use ralph_core::supervisor::dag_scheduler::{
            AdmissionCaps, AdmissionSnapshot, UnitAdmissionInput,
        };
        let sink = ShadowSink::new();
        let units: Vec<UnitAdmissionInput> = Vec::new();
        let snapshot = AdmissionSnapshot {
            units: &units,
            integration_target_head: None,
        };
        let caps = AdmissionCaps {
            global_cap: 8,
            executor_pool_cap: 4,
            resource_capacities: Vec::new(),
        };
        let obs = compute_shadow_observation(&snapshot, &caps, &sink);
        sink.record(obs);
        let summary = ShadowSinkReader::new(&sink).read();
        assert_eq!(summary.observation_count, 1);
    }
}
