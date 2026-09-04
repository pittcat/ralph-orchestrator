//! 2026-09-03-0959 plan U5 (R11/R12; S13/S14; D1/D2/D13;
//! E6/E7/E13/E16): observation-only shadow sink. No execution
//! side effects (R11 / §7 U5 #8) — driver (U6+) feeds snapshots.

use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::supervisor::dag_scheduler::{
    AdmissionCaps, AdmissionDecision, AdmissionReason, AdmissionSnapshot, compute_admissions,
};

/// One tick of shadow observations. Sanitized — no payload /
/// paths / prompts / secrets (R11 / E7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShadowObservation {
    pub plan_key: String,
    pub tick_epoch: u64,
    pub observed_at_ms: u64,
    pub ready_count: u32,
    pub admitted_count: u32,
    pub blocked_dependency_count: u32,
    pub blocked_resource_count: u32,
    /// Aggregates `BlockedGlobalCap` + `BlockedExecutorPoolCap` + `BlockedNoTargetHead` (U5 S14).
    pub blocked_cap_count: u32,
    /// Per-unit `(unit_id, reason_str)` in U4-stable `(integration_order, unit_id)` order.
    pub decisions: Vec<(String, String)>,
}

/// Thread-safe observation store (Arc<Mutex<Vec>>). Cheap clone.
#[derive(Debug, Clone, Default)]
pub struct ShadowSink {
    observations: Arc<Mutex<Vec<ShadowObservation>>>,
}

impl ShadowSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, observation: ShadowObservation) -> ShadowObservation {
        let mut g = self.observations.lock().expect("ShadowSink mutex poisoned");
        g.push(observation.clone());
        observation
    }

    pub fn latest_for_plan(&self, plan_key: &str) -> Option<ShadowObservation> {
        let g = self.observations.lock().expect("ShadowSink mutex poisoned");
        g.iter().rev().find(|o| o.plan_key == plan_key).cloned()
    }

    /// Distinct `plan_key`s, in first-seen order.
    pub fn list_plans(&self) -> Vec<String> {
        let g = self.observations.lock().expect("ShadowSink mutex poisoned");
        let mut seen: Vec<String> = Vec::new();
        for obs in g.iter() {
            if !seen.iter().any(|p| p == &obs.plan_key) {
                seen.push(obs.plan_key.clone());
            }
        }
        seen
    }

    pub fn observation_count(&self) -> usize {
        self.observations
            .lock()
            .expect("ShadowSink mutex poisoned")
            .len()
    }

    /// Apply `f` to a snapshot under the mutex — do NOT call back into the sink from `f`.
    pub fn with_observations<R>(&self, f: impl FnOnce(&[ShadowObservation]) -> R) -> R {
        let g = self.observations.lock().expect("ShadowSink mutex poisoned");
        f(&g)
    }
}

/// Pure decision fn: reuse U4 [`compute_admissions`]; never touch
/// store / worktree / task / event channel (R11). Caller records.
pub fn compute_shadow_observation(
    snapshot: &AdmissionSnapshot<'_>,
    caps: &AdmissionCaps,
    sink: &ShadowSink,
) -> ShadowObservation {
    let decisions = compute_admissions(snapshot, caps);
    let (admitted_count, blocked_dependency_count, blocked_resource_count, blocked_cap_count) =
        count_reasons(&decisions);
    ShadowObservation {
        // U4 snapshot has no plan_key; U6 driver replaces this.
        plan_key: String::new(),
        tick_epoch: next_tick_epoch(sink),
        observed_at_ms: system_time_ms(),
        ready_count: decisions.len() as u32,
        admitted_count,
        blocked_dependency_count,
        blocked_resource_count,
        blocked_cap_count,
        decisions: decisions
            .iter()
            .map(|d| (d.unit_id.clone(), reason_str(&d.reason).to_string()))
            .collect(),
    }
}

/// Collapse `Vec<AdmissionDecision>` into four counters; three
/// cap reasons merge into `blocked_cap_count` (U5 S14).
fn count_reasons(decisions: &[AdmissionDecision]) -> (u32, u32, u32, u32) {
    let mut admitted = 0u32;
    let mut blocked_dep = 0u32;
    let mut blocked_res = 0u32;
    let mut blocked_cap = 0u32;
    for d in decisions {
        match d.reason {
            AdmissionReason::Admitted => admitted = admitted.saturating_add(1),
            AdmissionReason::BlockedDependencies => blocked_dep = blocked_dep.saturating_add(1),
            AdmissionReason::BlockedResources => blocked_res = blocked_res.saturating_add(1),
            AdmissionReason::BlockedGlobalCap
            | AdmissionReason::BlockedExecutorPoolCap
            | AdmissionReason::BlockedNoTargetHead => blocked_cap = blocked_cap.saturating_add(1),
        }
    }
    (admitted, blocked_dep, blocked_res, blocked_cap)
}

fn reason_str(r: &AdmissionReason) -> &'static str {
    match r {
        AdmissionReason::Admitted => "Admitted",
        AdmissionReason::BlockedDependencies => "BlockedDependencies",
        AdmissionReason::BlockedResources => "BlockedResources",
        AdmissionReason::BlockedGlobalCap => "BlockedGlobalCap",
        AdmissionReason::BlockedExecutorPoolCap => "BlockedExecutorPoolCap",
        AdmissionReason::BlockedNoTargetHead => "BlockedNoTargetHead",
    }
}

fn next_tick_epoch(sink: &ShadowSink) -> u64 {
    sink.with_observations(|obs| {
        obs.iter()
            .map(|o| o.tick_epoch)
            .max()
            .map(|m| m + 1)
            .unwrap_or(0)
    })
}

fn system_time_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parallel_forge_handoff::{ResourceCapacity, ResourceClaim};
    use crate::supervisor::dag_scheduler::UnitAdmissionInput;

    fn rc(key: &str, cap: u32) -> ResourceCapacity {
        ResourceCapacity {
            key: key.to_string(),
            capacity: cap,
        }
    }
    fn rcl(key: &str, permits: u32) -> ResourceClaim {
        ResourceClaim {
            key: key.to_string(),
            permits,
        }
    }

    fn cap(global: u32, pool: u32, res: &[(&str, u32)]) -> AdmissionCaps {
        AdmissionCaps {
            global_cap: global,
            executor_pool_cap: pool,
            resource_capacities: res.iter().map(|(k, v)| rc(k, *v)).collect(),
        }
    }

    fn unit(
        id: &str,
        order: u32,
        deps: &[&str],
        integrated: &[&str],
        claims: &[(&str, u32)],
    ) -> UnitAdmissionInput {
        UnitAdmissionInput {
            unit_id: id.to_string(),
            integration_order: order,
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            integrated_units: integrated.iter().map(|s| s.to_string()).collect(),
            resource_claims: claims.iter().map(|(k, v)| rcl(k, *v)).collect(),
        }
    }

    /// Two independent units, no deps → both admitted; tick epoch monotonic.
    #[test]
    fn shadow_sink_records_and_retrieves_latest() {
        let sink = ShadowSink::new();
        let units = vec![unit("U1", 1, &[], &[], &[]), unit("U2", 2, &[], &[], &[])];
        let snap = AdmissionSnapshot {
            units: &units,
            integration_target_head: Some("h"),
        };
        sink.record(compute_shadow_observation(&snap, &cap(10, 10, &[]), &sink));
        let latest = sink.record(compute_shadow_observation(&snap, &cap(10, 10, &[]), &sink));
        assert_eq!(latest.tick_epoch, 1);
        assert_eq!(latest.admitted_count, 2);
        assert_eq!(latest.ready_count, 2);
        assert_eq!(latest.decisions.len(), 2);
    }

    /// Two plans' observations stay isolated under `latest_for_plan`.
    #[test]
    fn shadow_sink_distinguishes_plans() {
        let sink = ShadowSink::new();
        let mk = |key: &str, adm: u32, dep: u32, reason: &str| ShadowObservation {
            plan_key: key.to_string(),
            tick_epoch: 0,
            observed_at_ms: 0,
            ready_count: 1,
            admitted_count: adm,
            blocked_dependency_count: dep,
            blocked_resource_count: 0,
            blocked_cap_count: 0,
            decisions: vec![("U1".to_string(), reason.to_string())],
        };
        sink.record(mk("plan-A", 1, 0, "Admitted"));
        sink.record(mk("plan-B", 0, 1, "BlockedDependencies"));
        let a = sink.latest_for_plan("plan-A").expect("plan-A");
        let b = sink.latest_for_plan("plan-B").expect("plan-B");
        assert_eq!(a.admitted_count, 1);
        assert_eq!(b.blocked_dependency_count, 1);
        assert_eq!(
            sink.list_plans(),
            vec!["plan-A".to_string(), "plan-B".to_string()]
        );
    }

    /// `compute_shadow_observation` must NOT record by itself — sink stays empty until commit.
    /// Also proves `BlockedNoTargetHead` collapses into `blocked_cap_count`.
    #[test]
    fn shadow_observation_zero_side_effects() {
        let sink = ShadowSink::new();
        assert_eq!(sink.observation_count(), 0);
        let units = vec![
            unit("U1", 1, &[], &[], &[]),
            unit("U2", 2, &[], &[], &[]),
            unit("U3", 3, &[], &[], &[]),
        ];
        let snap = AdmissionSnapshot {
            units: &units,
            integration_target_head: Some("h"),
        };
        let obs = compute_shadow_observation(&snap, &cap(1, 1, &[]), &sink);
        assert_eq!(sink.observation_count(), 0);
        sink.record(obs);
        let snap_no_head = AdmissionSnapshot {
            units: &units,
            integration_target_head: None,
        };
        let obs2 = compute_shadow_observation(&snap_no_head, &cap(10, 10, &[]), &sink);
        assert_eq!(obs2.admitted_count, 0);
        assert_eq!(obs2.blocked_cap_count, 3);
        for (_, reason) in &obs2.decisions {
            assert_eq!(reason, "BlockedNoTargetHead");
        }
    }

    /// Per-unit decision counts match the breakdown; `decisions`
    /// vec mirrors the same split. Ordering: global_cap BEFORE
    /// resource_capacity → integration_order=2 reaches resource.
    #[test]
    fn compute_shadow_observation_counts_match_decisions() {
        let sink = ShadowSink::new();
        let units = vec![
            unit("U1", 1, &[], &[], &[]),
            unit("U4", 2, &[], &[], &[("db", 5)]),
            unit("U2", 3, &[], &[], &[]),
            unit("U3", 4, &["U1"], &[], &[]),
            unit("U5", 5, &[], &[], &[]),
        ];
        let snap = AdmissionSnapshot {
            units: &units,
            integration_target_head: Some("h"),
        };
        let obs = compute_shadow_observation(&snap, &cap(2, 10, &[("db", 1)]), &sink);
        assert_eq!(obs.ready_count, 5);
        assert_eq!(obs.admitted_count, 2);
        assert_eq!(obs.blocked_dependency_count, 1);
        assert_eq!(obs.blocked_resource_count, 1);
        assert_eq!(obs.blocked_cap_count, 1);
        assert_eq!(obs.decisions.len(), 5);
        let mut admitted = 0u32;
        let mut dep = 0u32;
        let mut res = 0u32;
        let mut cap_n = 0u32;
        for (_, reason) in &obs.decisions {
            match reason.as_str() {
                "Admitted" => admitted += 1,
                "BlockedDependencies" => dep += 1,
                "BlockedResources" => res += 1,
                "BlockedGlobalCap" | "BlockedExecutorPoolCap" | "BlockedNoTargetHead" => cap_n += 1,
                other => panic!("unexpected sanitized reason: {other}"),
            }
        }
        assert_eq!(admitted, obs.admitted_count);
        assert_eq!(dep, obs.blocked_dependency_count);
        assert_eq!(res, obs.blocked_resource_count);
        assert_eq!(cap_n, obs.blocked_cap_count);
    }
}
