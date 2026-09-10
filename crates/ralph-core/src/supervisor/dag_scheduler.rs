//! 2026-09-03-0959 plan U4 (R3, R4, R6; S3-S6; D5, D6, D10; E1, E5, E8, E9):
//! pure work-conserving admission engine for the runtime-owned DAG
//! scheduler.
//!
//! This module is the **pure decision core** the future runtime
//! driver (U5+) will call once per tick. It does NOT touch the
//! store, the lane, the executor, the worktree, the event ledger,
//! or any I/O — it takes a snapshot of plan state + admission
//! caps and returns an ordered `Vec<AdmissionDecision>`. The
//! caller is responsible for materialising the resulting
//! admissions as durable leases in the same transaction that
//! invoked this function (per plan §7 U4 #13: "最小实现：纯
//! decision function + store transaction apply").
//!
//! Algorithm (from plan §7 U4):
//!   1. Sort candidates by `(integration_order, unit_id)` so
//!      two identical snapshots always yield identical decisions
//!      (replay determinism — plan §7 U4 #20).
//!   2. A unit is `Ready` iff every `depends_on` is in
//!      `integrated_units` AND `integration_target_head.is_some()`
//!      (or the unit has no deps). The latter clause is what
//!      `integrated_units_only_is_not_sufficient` exercises:
//!      having deps integrated is necessary but not sufficient.
//!   3. Walk the sorted list, attempting to admit each Ready
//!      unit against three independent gates in this order:
//!      (a) global cap, (b) executor pool cap, (c) resource
//!      capacity per claim. The first gate that fails becomes
//!      the unit's `AdmissionReason`. Already-admitted units in
//!      this same tick hold their lease for the duration of the
//!      walk — only the cumulative leased-permits map is
//!      consulted, never the store.
//!   4. A claim whose `permits > capacity` is fail-closed
//!      (`BlockedResources`) — U2's parse-time validator accepts
//!      it explicitly so U4 admission can centralise the rule.
//!   5. A claim referencing an unknown capacity key is also
//!      fail-closed (`BlockedResources`) — defence in depth, U2
//!      normally rejects this at parse time.
//!
//! Time does NOT enter the selection key. Inspection tools may
//! surface "oldest waiting unit" by a separate annotation pass
//! (U9 recovery / inspect), but the decisions themselves are
//! purely a function of the snapshot — two snapshots with the
//! same inputs always yield identical `Vec<AdmissionDecision>`.

use std::collections::{BTreeMap, HashSet};

use crate::parallel_forge_handoff::{ResourceCapacity, ResourceClaim};

/// Per-unit admission input handed to [`compute_admissions`].
///
/// `integrated_units` is the per-unit view of "what my plan has
/// already integrated", passed in by the caller rather than
/// looked up from the store so the function stays pure (no
/// store I/O).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitAdmissionInput {
    pub unit_id: String,
    pub integration_order: u32,
    pub depends_on: Vec<String>,
    pub integrated_units: HashSet<String>,
    pub resource_claims: Vec<ResourceClaim>,
}

/// Snapshot of plan state passed to [`compute_admissions`].
///
/// `integration_target_head` is the runtime-known tip of the
/// integration branch (e.g. the SHA produced by the last
/// integrated unit). `None` means no integration lane has been
/// initialised yet — every Ready unit that would otherwise be
/// admitted is blocked with `BlockedNoTargetHead` until the
/// runtime publishes the first integration head.
///
/// `live_stage_counts` (U1, 2026-09-09) is the per-stage in-flight
/// headcount the runtime already holds: keys are the stable stage
/// names (`"execute"` / `"review"` / `"verify"` / `"fix"`),
/// values are the counts of Units currently occupying the stage. Each
/// live entry already consumes one slot of `global_cap`, so
/// `compute_admissions` seeds `admitted_count` with the sum.
///
/// `current_job_unit_ids` is the set of Unit IDs that already
/// hold a live job. The candidate filter in `dag_scheduler::tick`
/// strips these from the candidate list before calling
/// `compute_admissions`, but the snapshot still carries the set as
/// a defence-in-depth: if a Unit with a live job ever reaches
/// `compute_admissions` it is reported as `BlockedDependencies`
/// (admission is idempotent for live Units) rather than silently
/// re-admitted and double-booking the global cap.
///
/// Both fields are owned (`String`-keyed maps/sets) for ergonomic
/// construction at the `tick` seam; the borrowed slice
/// `units: &'a [UnitAdmissionInput]` keeps the existing
/// zero-allocation hot path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionSnapshot<'a> {
    pub units: &'a [UnitAdmissionInput],
    pub integration_target_head: Option<&'a str>,
    pub live_stage_counts: BTreeMap<String, u32>,
    pub current_job_unit_ids: HashSet<String>,
}

impl<'a> AdmissionSnapshot<'a> {
    /// 2026-09-09-0917 plan U3 (production path): build the borrowed
    /// snapshot from owned runtime data captured at the `tick` seam.
    ///
    /// The runtime pipeline hands `live_stage_counts` /
    /// `current_job_unit_ids` in owned form (BTreeMap / HashSet) so
    /// the borrow-based `units: &'a [_]` stays the only lifetime the
    /// caller must thread through. `target_head` is borrowed for the
    /// lifetime of the candidate slice (the integration lane tip
    /// lives at least as long as the per-tick plan borrow).
    pub fn from_owned(
        units: &'a [UnitAdmissionInput],
        target_head: Option<&'a str>,
        live_stage_counts: BTreeMap<String, u32>,
        current_job_unit_ids: HashSet<String>,
    ) -> Self {
        Self {
            units,
            integration_target_head: target_head,
            live_stage_counts,
            current_job_unit_ids,
        }
    }
}

/// Pool + capacity caps handed to [`compute_admissions`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionCaps {
    pub global_cap: u32,
    pub executor_pool_cap: u32,
    pub resource_capacities: Vec<ResourceCapacity>,
}

/// Reason a unit was admitted or blocked. Stable, comparable,
/// serde-friendly — U5 inspect summaries will reuse this enum
/// verbatim so two snapshots yield JSON with the same key set.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AdmissionReason {
    /// Admitted into this tick; resources leased.
    Admitted,
    /// At least one `depends_on` is not in `integrated_units`.
    BlockedDependencies,
    /// Resource capacity exceeded (permits > capacity, or
    /// already-leased permits + this claim would exceed
    /// capacity, or claim references an unknown capacity key).
    BlockedResources,
    /// `global_cap` already saturated by earlier admissions in
    /// this tick.
    BlockedGlobalCap,
    /// `executor_pool_cap` already saturated.
    BlockedExecutorPoolCap,
    /// `integration_target_head` is `None` — runtime has not
    /// published the first integration tip yet.
    BlockedNoTargetHead,
}

/// One unit's admission outcome. `admitted == true` iff
/// `reason == AdmissionReason::Admitted`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionDecision {
    pub unit_id: String,
    pub admitted: bool,
    pub reason: AdmissionReason,
}

/// Compute admissions for one tick. Pure, deterministic, no
/// I/O. Output is in `(integration_order, unit_id)` order so two
/// identical snapshots always yield identical output.
pub fn compute_admissions(
    state: &AdmissionSnapshot<'_>,
    caps: &AdmissionCaps,
) -> Vec<AdmissionDecision> {
    let mut sorted: Vec<&UnitAdmissionInput> = state.units.iter().collect();
    sorted.sort_by(|a, b| {
        a.integration_order
            .cmp(&b.integration_order)
            .then_with(|| a.unit_id.cmp(&b.unit_id))
    });

    let cap_map: BTreeMap<&str, u32> = caps
        .resource_capacities
        .iter()
        .map(|c| (c.key.as_str(), c.capacity))
        .collect();

    let mut leased: BTreeMap<String, u32> = BTreeMap::new();
    // U1 (2026-09-09): live jobs already consume `global_cap`. Seed
    // the running counter with the sum of live stage counts so the
    // first gate sees the right "remaining" capacity on entry.
    let mut admitted_count: u32 = state.live_stage_counts.values().copied().sum();
    let mut executor_count: u32 = 0;

    let mut decisions: Vec<AdmissionDecision> = Vec::with_capacity(sorted.len());
    for unit in sorted {
        // U1 (2026-09-09): defence-in-depth — a Unit whose live
        // job is already on the books must never be re-admitted
        // (the candidate filter at the `tick` seam strips these,
        // but if a Unit reaches this function with a live job we
        // mark it `BlockedDependencies` rather than letting it
        // double-book a global slot).
        if state.current_job_unit_ids.contains(&unit.unit_id) {
            decisions.push(AdmissionDecision {
                unit_id: unit.unit_id.clone(),
                admitted: false,
                reason: AdmissionReason::BlockedDependencies,
            });
            continue;
        }

        let deps_blocked = unit
            .depends_on
            .iter()
            .any(|d| !unit.integrated_units.contains(d));

        let reason = if deps_blocked {
            AdmissionReason::BlockedDependencies
        } else if state.integration_target_head.is_none() {
            AdmissionReason::BlockedNoTargetHead
        } else if admitted_count >= caps.global_cap {
            AdmissionReason::BlockedGlobalCap
        } else if executor_count >= caps.executor_pool_cap {
            AdmissionReason::BlockedExecutorPoolCap
        } else {
            let mut resource_blocked = false;
            for claim in &unit.resource_claims {
                let cap = match cap_map.get(claim.key.as_str()) {
                    Some(c) => *c,
                    None => {
                        resource_blocked = true;
                        break;
                    }
                };
                let already = leased.get(&claim.key).copied().unwrap_or(0);
                if claim.permits > cap || already.saturating_add(claim.permits) > cap {
                    resource_blocked = true;
                    break;
                }
            }
            if resource_blocked {
                AdmissionReason::BlockedResources
            } else {
                for claim in &unit.resource_claims {
                    let entry = leased.entry(claim.key.clone()).or_insert(0);
                    *entry = entry.saturating_add(claim.permits);
                }
                admitted_count = admitted_count.saturating_add(1);
                executor_count = executor_count.saturating_add(1);
                AdmissionReason::Admitted
            }
        };

        decisions.push(AdmissionDecision {
            unit_id: unit.unit_id.clone(),
            admitted: matches!(reason, AdmissionReason::Admitted),
            reason,
        });
    }
    decisions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap_global(global: u32) -> AdmissionCaps {
        AdmissionCaps {
            global_cap: global,
            executor_pool_cap: global,
            resource_capacities: vec![],
        }
    }

    fn cap_pool(global: u32, pool: u32) -> AdmissionCaps {
        AdmissionCaps {
            global_cap: global,
            executor_pool_cap: pool,
            resource_capacities: vec![],
        }
    }

    fn cap_resources(global: u32, pool: u32, res: &[(&str, u32)]) -> AdmissionCaps {
        AdmissionCaps {
            global_cap: global,
            executor_pool_cap: pool,
            resource_capacities: res
                .iter()
                .map(|(k, v)| ResourceCapacity {
                    key: (*k).to_string(),
                    capacity: *v,
                })
                .collect(),
        }
    }

    fn unit(id: &str, order: u32, deps: &[&str], integrated: &[&str]) -> UnitAdmissionInput {
        UnitAdmissionInput {
            unit_id: id.to_string(),
            integration_order: order,
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            integrated_units: integrated.iter().map(|s| s.to_string()).collect(),
            resource_claims: vec![],
        }
    }

    fn unit_with_claims(
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
            resource_claims: claims
                .iter()
                .map(|(k, v)| ResourceClaim {
                    key: (*k).to_string(),
                    permits: *v,
                })
                .collect(),
        }
    }

    fn decision<'a>(decisions: &'a [AdmissionDecision], id: &str) -> &'a AdmissionDecision {
        decisions
            .iter()
            .find(|d| d.unit_id == id)
            .unwrap_or_else(|| panic!("no decision for {id}"))
    }

    // --- 10 contract tests --------------------------------------------------

    /// S3 baseline: three independent units, no deps, no
    /// resources, target head present → all admitted in stable
    /// order.
    #[test]
    fn admit_all_when_no_dependencies_and_no_resource_contention() {
        let units = vec![
            unit("U1", 1, &[], &[]),
            unit("U2", 2, &[], &[]),
            unit("U3", 3, &[], &[]),
        ];
        let snapshot = AdmissionSnapshot {
            units: &units,
            integration_target_head: Some("deadbeef"),
            live_stage_counts: BTreeMap::new(),
            current_job_unit_ids: HashSet::new(),
        };
        let decisions = compute_admissions(&snapshot, &cap_global(10));
        assert_eq!(decisions.len(), 3);
        assert_eq!(decision(&decisions, "U1").reason, AdmissionReason::Admitted);
        assert_eq!(decision(&decisions, "U2").reason, AdmissionReason::Admitted);
        assert_eq!(decision(&decisions, "U3").reason, AdmissionReason::Admitted);
        // Stable sorted output.
        assert_eq!(decisions[0].unit_id, "U1");
        assert_eq!(decisions[1].unit_id, "U2");
        assert_eq!(decisions[2].unit_id, "U3");
    }

    /// R6 / S5: a unit whose dep has NOT yet integrated must
    /// stay blocked even though the global cap is wide open.
    #[test]
    fn block_unit_with_pending_dependency() {
        let units = vec![unit("U1", 1, &[], &[]), unit("U2", 2, &["U1"], &[])];
        let snapshot = AdmissionSnapshot {
            units: &units,
            integration_target_head: Some("deadbeef"),
            live_stage_counts: BTreeMap::new(),
            current_job_unit_ids: HashSet::new(),
        };
        let decisions = compute_admissions(&snapshot, &cap_global(10));
        assert_eq!(decision(&decisions, "U1").reason, AdmissionReason::Admitted);
        assert_eq!(
            decision(&decisions, "U2").reason,
            AdmissionReason::BlockedDependencies
        );
    }

    /// R6 / S5: the moment U1 is in `integrated_units`, U2
    /// becomes Ready and gets admitted.
    #[test]
    fn unblock_unit_when_dependency_integrated() {
        let units = vec![unit("U1", 1, &[], &[]), unit("U2", 2, &["U1"], &["U1"])];
        let snapshot = AdmissionSnapshot {
            units: &units,
            integration_target_head: Some("cafebabe"),
            live_stage_counts: BTreeMap::new(),
            current_job_unit_ids: HashSet::new(),
        };
        let decisions = compute_admissions(&snapshot, &cap_global(10));
        assert_eq!(decision(&decisions, "U1").reason, AdmissionReason::Admitted);
        assert_eq!(decision(&decisions, "U2").reason, AdmissionReason::Admitted);
    }

    /// S5 / D10: with no integration head yet, even an
    /// independent unit (no deps) cannot be admitted.
    #[test]
    fn block_when_no_target_head() {
        let units = vec![unit("U1", 1, &[], &[])];
        let snapshot = AdmissionSnapshot {
            units: &units,
            integration_target_head: None,
            live_stage_counts: BTreeMap::new(),
            current_job_unit_ids: HashSet::new(),
        };
        let decisions = compute_admissions(&snapshot, &cap_global(10));
        assert_eq!(
            decision(&decisions, "U1").reason,
            AdmissionReason::BlockedNoTargetHead
        );
        assert!(!decision(&decisions, "U1").admitted);
    }

    /// R16 / S3: global_cap caps the total number of
    /// simultaneously admitted units across all waves.
    #[test]
    fn respect_global_cap() {
        let units = vec![
            unit("U1", 1, &[], &[]),
            unit("U2", 2, &[], &[]),
            unit("U3", 3, &[], &[]),
        ];
        let snapshot = AdmissionSnapshot {
            units: &units,
            integration_target_head: Some("h"),
            live_stage_counts: BTreeMap::new(),
            current_job_unit_ids: HashSet::new(),
        };
        let decisions = compute_admissions(&snapshot, &cap_global(2));
        assert_eq!(decision(&decisions, "U1").reason, AdmissionReason::Admitted);
        assert_eq!(decision(&decisions, "U2").reason, AdmissionReason::Admitted);
        assert_eq!(
            decision(&decisions, "U3").reason,
            AdmissionReason::BlockedGlobalCap
        );
    }

    /// R16 / S4: executor_pool_cap is independent of global_cap.
    /// Even with global_cap=10, only `executor_pool_cap` units
    /// can be admitted; the rest hit the pool gate.
    #[test]
    fn respect_executor_pool_cap() {
        let units = vec![
            unit("U1", 1, &[], &[]),
            unit("U2", 2, &[], &[]),
            unit("U3", 3, &[], &[]),
        ];
        let snapshot = AdmissionSnapshot {
            units: &units,
            integration_target_head: Some("h"),
            live_stage_counts: BTreeMap::new(),
            current_job_unit_ids: HashSet::new(),
        };
        let decisions = compute_admissions(&snapshot, &cap_pool(10, 2));
        assert_eq!(decision(&decisions, "U1").reason, AdmissionReason::Admitted);
        assert_eq!(decision(&decisions, "U2").reason, AdmissionReason::Admitted);
        assert_eq!(
            decision(&decisions, "U3").reason,
            AdmissionReason::BlockedExecutorPoolCap
        );
    }

    /// R4 / S6: shared resource capacity. Capacity 1 + two units
    /// claiming 1 each → first admitted, second blocked on
    /// resources.
    #[test]
    fn respect_resource_capacity() {
        let units = vec![
            unit_with_claims("U1", 1, &[], &[], &[("db", 1)]),
            unit_with_claims("U2", 2, &[], &[], &[("db", 1)]),
        ];
        let snapshot = AdmissionSnapshot {
            units: &units,
            integration_target_head: Some("h"),
            live_stage_counts: BTreeMap::new(),
            current_job_unit_ids: HashSet::new(),
        };
        let caps = cap_resources(10, 10, &[("db", 1)]);
        let decisions = compute_admissions(&snapshot, &caps);
        assert_eq!(decision(&decisions, "U1").reason, AdmissionReason::Admitted);
        assert_eq!(
            decision(&decisions, "U2").reason,
            AdmissionReason::BlockedResources
        );
    }

    /// R4 / D6 / U2 hand-off: U2 accepts `permits > capacity`
    /// at parse time so U4 admission can centralise the rule.
    /// Such a claim is rejected with `BlockedResources`.
    #[test]
    fn block_when_claim_exceeds_capacity_at_parse_time() {
        let units = vec![unit_with_claims("U1", 1, &[], &[], &[("db", 5)])];
        let snapshot = AdmissionSnapshot {
            units: &units,
            integration_target_head: Some("h"),
            live_stage_counts: BTreeMap::new(),
            current_job_unit_ids: HashSet::new(),
        };
        let caps = cap_resources(10, 10, &[("db", 1)]);
        let decisions = compute_admissions(&snapshot, &caps);
        assert_eq!(
            decision(&decisions, "U1").reason,
            AdmissionReason::BlockedResources
        );
    }

    /// Plan §7 U4 #20: two Ready units, two snapshots with
    /// identical inputs → identical decisions. The one with
    /// lower `integration_order` is admitted first; ties break
    /// on `unit_id` lexicographically.
    #[test]
    fn stable_ordering() {
        let units = vec![
            unit("U_B", 1, &[], &[]),
            unit("U_A", 2, &[], &[]),
            unit("U_C", 1, &[], &[]),
        ];
        let snapshot = AdmissionSnapshot {
            units: &units,
            integration_target_head: Some("h"),
            live_stage_counts: BTreeMap::new(),
            current_job_unit_ids: HashSet::new(),
        };
        let decisions = compute_admissions(&snapshot, &cap_global(10));
        // integration_order tie: U_B < U_C lex.
        assert_eq!(decisions[0].unit_id, "U_B");
        assert_eq!(decisions[1].unit_id, "U_C");
        assert_eq!(decisions[2].unit_id, "U_A");
        // Replay-determinism: a second call on the same inputs
        // yields the same order.
        let decisions2 = compute_admissions(&snapshot, &cap_global(10));
        assert_eq!(decisions, decisions2);
    }

    /// S5 / D10: having `integrated_units` contain the
    /// dependency is necessary but NOT sufficient. The unit
    /// must also pass the target-head gate.
    #[test]
    fn integrated_units_only_is_not_sufficient() {
        let units = vec![unit("U1", 1, &[], &[]), unit("U2", 2, &["U1"], &["U1"])];
        // No integration head yet — even though U1 is already
        // integrated from U2's point of view, U2 stays blocked.
        let snapshot = AdmissionSnapshot {
            units: &units,
            integration_target_head: None,
            live_stage_counts: BTreeMap::new(),
            current_job_unit_ids: HashSet::new(),
        };
        let decisions = compute_admissions(&snapshot, &cap_global(10));
        assert_eq!(
            decision(&decisions, "U1").reason,
            AdmissionReason::BlockedNoTargetHead
        );
        // U2's dep IS integrated, so the dep gate does not fire
        // first — it must surface as `BlockedNoTargetHead`, not
        // `BlockedDependencies`. That proves "integrated_units
        // only" is not enough.
        assert_eq!(
            decision(&decisions, "U2").reason,
            AdmissionReason::BlockedNoTargetHead
        );
    }

    // --- U1 boundary tests (2026-09-09-0917) -------------------------------

    /// U1 boundary: `snapshot_subtracts_live_stages`. A live execute
    /// already consumes one slot of `global_cap`. With cap=2 and one
    /// live execute, only ONE further unit can be admitted — not
    /// two. The admission engine must seed `admitted_count` with the
    /// sum of live stage counts so this subtraction is honoured.
    #[test]
    fn snapshot_subtracts_live_stages() {
        let units = vec![
            unit("U1", 1, &[], &[]),
            unit("U2", 2, &[], &[]),
            unit("U3", 3, &[], &[]),
        ];
        let mut live = BTreeMap::new();
        live.insert("execute".to_string(), 1u32);
        let snapshot = AdmissionSnapshot {
            units: &units,
            integration_target_head: Some("h"),
            live_stage_counts: live,
            current_job_unit_ids: HashSet::new(),
        };
        let decisions = compute_admissions(&snapshot, &cap_global(2));
        let admitted: Vec<&str> = decisions
            .iter()
            .filter(|d| d.reason == AdmissionReason::Admitted)
            .map(|d| d.unit_id.as_str())
            .collect();
        assert_eq!(
            admitted.len(),
            1,
            "global_cap=2 minus 1 live execute must admit exactly 1"
        );
        // The third independent unit must be `BlockedGlobalCap`,
        // proving the live count was honoured (not silently dropped).
        assert_eq!(
            decision(&decisions, "U3").reason,
            AdmissionReason::BlockedGlobalCap
        );
    }

    /// U1 boundary: `blocked_dependency_does_not_consume_capacity`.
    /// A unit that fails the dependency gate must NOT count against
    /// `global_cap` — only `Admitted` units consume a slot. Two
    /// cap-2 scenarios: (a) one dep-blocked unit + one ready unit
    /// admits exactly one; (b) three dep-blocked units admit zero.
    #[test]
    fn blocked_dependency_does_not_consume_capacity() {
        // Scenario (a): one dep-blocked + one ready.
        let units = vec![
            unit("U1", 1, &[], &[]),
            unit("U2", 2, &["U1"], &[]), // U1 not yet integrated → blocked
        ];
        let snapshot = AdmissionSnapshot {
            units: &units,
            integration_target_head: Some("h"),
            live_stage_counts: BTreeMap::new(),
            current_job_unit_ids: HashSet::new(),
        };
        let decisions = compute_admissions(&snapshot, &cap_global(1));
        assert_eq!(decision(&decisions, "U1").reason, AdmissionReason::Admitted);
        assert_eq!(
            decision(&decisions, "U2").reason,
            AdmissionReason::BlockedDependencies
        );
        // Scenario (b): all blocked by deps → zero admissions even
        // though cap is wide open. Proves dep-blocked units are free.
        let units_b = vec![
            unit("U1", 1, &["U_MISSING"], &[]),
            unit("U2", 2, &["U_MISSING"], &[]),
            unit("U3", 3, &["U_MISSING"], &[]),
        ];
        let snapshot_b = AdmissionSnapshot {
            units: &units_b,
            integration_target_head: Some("h"),
            live_stage_counts: BTreeMap::new(),
            current_job_unit_ids: HashSet::new(),
        };
        let decisions_b = compute_admissions(&snapshot_b, &cap_global(10));
        let admitted_b = decisions_b
            .iter()
            .filter(|d| d.reason == AdmissionReason::Admitted)
            .count();
        assert_eq!(admitted_b, 0, "all dep-blocked → zero admissions");
    }
}
