//! 2026-07-01-001 plan U6: engine-computed `expected_event`
//! and `## ORCHESTRATOR STATE` injection.
//!
//! The orchestrator (engine) is the only authoritative source
//! of "what should the next coordinator activation emit?". The
//! previous design forced the agent to count `### U{N}.`
//! headings in `plan.md` — that drifted whenever the plan
//! was edited, fix-plan was missing, or the LLM counted wrong.
//!
//! This module owns three responsibilities:
//!
//! 1. [`PlanTopologyCache`] — scan `plan.md` / fix-plan once at
//!    the appropriate lifecycle boundary and cache the ordered
//!    step ids (`["step-01", "step-02", ...]` / `["fix-01", ...]`).
//!    Scanning fails closed (empty list + diagnostic) — the
//!    engine never guesses `N_total`.
//!
//! 2. [`compute_expected_event`] — given the most recent
//!    `test.passed` payload + plan/fix topology + ledger facts
//!    (tasks, lifecycle, review-walk), derive the single
//!    `expected_event` the coordinator should emit next.
//!
//! 3. [`OrchestratorState`] — the JSON-serialisable struct that
//!    is rendered into the `## ORCHESTRATOR STATE` block at
//!    the head of every coordinator prompt (not just on
//!    `task.resume` paths).
//!
//! Why SSOT lives here: U3's `coordinator_decision_gate_stage`
//! rewrites `work.ready` → `plan.complete` only for the last
//! fix-unit, and U1's budget priority treats terminal topics
//! as first-class. U6 is the upstream instruction card — it
//! tells the agent what to do, U3 enforces it on emit, and
//! U1/U2 are the backstop for over-emits.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::event_loop::review_step_state::{
    scan_unit_headings, scan_unit_headings_as_steps,
};
use crate::state::CommitDelta;
use crate::state::StateLedger;

/// The set of phases a coordinator can be in. The mapping
/// `phase → expected_event` is the canonical SSOT in
/// `coordinator_decision_gate_stage::topic_for_phase`; we keep
/// the enum aligned with [`crate::event_loop::stages::coordinator_decision_gate_stage::PhaseClass`]
/// and extend it with the cases that the stage's table does
/// not yet cover (`ReviewWalk`, `Ship`, `Terminal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorPhase {
    /// Walking through plan units (`step-NN`).
    PlanUnit,
    /// Walking through fix units (`fix-NN`).
    FixUnit,
    /// Final review walk (review-coordinator →
    /// dimension-reviewer → review-synthesizer).
    ReviewWalk,
    /// Ship phase (after review passed, before LOOP_COMPLETE).
    Ship,
    /// Terminal — completion honored; no further business emit.
    Terminal,
}

impl CoordinatorPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PlanUnit => "plan_unit",
            Self::FixUnit => "fix_unit",
            Self::ReviewWalk => "review_walk",
            Self::Ship => "ship",
            Self::Terminal => "terminal",
        }
    }
}

/// Cached topology of the plan + the fix-plan (if any).
///
/// The cache is owned by `LoopState`; it is filled in:
/// - on loop start (after the runner has resolved `plan_path`)
///   for the **plan** topology
/// - on `review.complete` landing with a `fix_plan_file`
///   payload (or directly via the runner dropping the fix-plan
///   file when review fails) for the **fix** topology
///
/// The cache never silently guesses: an empty topology
/// surfaces as `plan_topology_unparseable` /
/// `fix_topology_unparseable` diagnostics, and
/// `compute_expected_event` returns `expected_event = None`
/// (fail-closed). The agent then has no
/// `expected_event` to follow and is forced to fall back to
/// the existing top-down review / U3 rewrite path; U1/U2
/// still back-stop misbehaviour.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PlanTopologyCache {
    /// `["step-01", "step-02", ...]` scanned from `plan.md`.
    pub plan_unit_ids: Vec<String>,
    /// `["fix-01", ...]` scanned from the fix-plan file.
    pub fix_unit_ids: Vec<String>,
    /// Absolute path to the file that produced `plan_unit_ids`.
    pub plan_path: Option<String>,
    /// Absolute path to the file that produced `fix_unit_ids`.
    pub fix_plan_path: Option<String>,
}

impl PlanTopologyCache {
    /// Total plan units (1-based for the agent-facing copy).
    pub fn plan_unit_total(&self) -> Option<u32> {
        if self.plan_unit_ids.is_empty() {
            None
        } else {
            Some(self.plan_unit_ids.len() as u32)
        }
    }

    /// Total fix units (1-based for the agent-facing copy).
    pub fn fix_unit_total(&self) -> Option<u32> {
        if self.fix_unit_ids.is_empty() {
            None
        } else {
            Some(self.fix_unit_ids.len() as u32)
        }
    }

    /// Resolve the 1-based position of `step` in the plan
    /// topology, if it is a known plan unit.
    pub fn plan_position(&self, step: &str) -> Option<u32> {
        self.plan_unit_ids
            .iter()
            .position(|s| s == step)
            .map(|p| p as u32 + 1)
    }

    /// Resolve the 1-based position of `fix-NN` in the fix
    /// topology, if it is a known fix unit.
    pub fn fix_position(&self, fix_step: &str) -> Option<u32> {
        self.fix_unit_ids
            .iter()
            .position(|s| s == fix_step)
            .map(|p| p as u32 + 1)
    }

    /// Scan `path` (a plan markdown file) and return the
    /// ordered step ids as `step-NN`. Returns an empty Vec
    /// on any failure — callers should publish a diagnostic.
    pub fn scan(path: &Path) -> Vec<String> {
        scan_unit_headings_as_steps(path)
    }

    /// Scan `path` (a fix-plan markdown file) and return the
    /// ordered step ids as `fix-NN`. Returns an empty Vec on
    /// any failure.
    pub fn scan_fix_plan(path: &Path) -> Vec<String> {
        scan_unit_headings(path)
    }

    /// Convenience: install the plan topology, replacing any
    /// existing entries. Publishes a diagnostic on empty
    /// results so the agent can surface the parse failure.
    pub fn install_plan_topology(
        &mut self,
        ledger: &mut StateLedger,
        path: &Path,
    ) -> Result<(), String> {
        let ids = Self::scan(path);
        if ids.is_empty() {
            // The ledger's RejectionRecorded variant is the
            // generic "loud failure" channel; the agent reads
            // it via `ralph diagnose` and the prompt can
            // surface the message verbatim.
            let _ = ledger.commit(
                CommitDelta::RejectionRecorded {
                    key: "plan_topology_unparseable".to_string(),
                    message: Some(format!(
                        "plan_topology: '{}' did not match the `### U{{N}}.` convention; \
                         `expected_event` for plan-unit events is unavailable",
                        path.display()
                    )),
                    topic: None,
                },
                None,
            );
            return Err(format!(
                "plan_topology_unparseable: '{}' did not match the `### U{{N}}.` convention",
                path.display()
            ));
        }
        self.plan_path = Some(path.display().to_string());
        self.plan_unit_ids = ids;
        Ok(())
    }

    /// Convenience: install the fix topology.
    pub fn install_fix_topology(
        &mut self,
        ledger: &mut StateLedger,
        path: &Path,
    ) -> Result<(), String> {
        let ids = Self::scan_fix_plan(path);
        if ids.is_empty() {
            let _ = ledger.commit(
                CommitDelta::RejectionRecorded {
                    key: "fix_topology_unparseable".to_string(),
                    message: Some(format!(
                        "fix_topology: '{}' did not match the `### U{{N}}.` convention",
                        path.display()
                    )),
                    topic: None,
                },
                None,
            );
            return Err(format!(
                "fix_topology_unparseable: '{}' did not match the `### U{{N}}.` convention",
                path.display()
            ));
        }
        self.fix_plan_path = Some(path.display().to_string());
        self.fix_unit_ids = ids;
        Ok(())
    }
}

/// Input to [`compute_expected_event`]. The caller constructs
/// this from the loop's known state at the moment the
/// coordinator is about to be activated.
#[derive(Debug, Default, Clone)]
pub struct ComputeInput<'a> {
    /// Step id that just landed in the most recent accepted
    /// `test.passed` payload. None when the trigger is not
    /// `test.passed` (e.g. `review.complete`).
    pub last_test_passed_step: Option<&'a str>,
    /// True when the previous test.passed was a fix-unit
    /// (`fix-NN`). Derived from the step prefix; both flags
    /// are kept so the lookup logic stays explicit.
    pub last_was_fix_unit: bool,
    /// True when the review-walk has already closed
    /// (`review.complete` accepted with a pass verdict).
    pub review_walk_closed: bool,
    /// True when the runtime has honored the completion
    /// promise (i.e. `LOOP_COMPLETE` accepted). Once true,
    /// `expected_event` is always `None` and the phase is
    /// `Terminal`.
    pub completion_honored: bool,
}

/// Result of [`compute_expected_event`]. All fields are
/// `Option` to allow the fail-closed path
/// (`expected_event = None`) to flow through to the prompt as
/// an explicit "engine could not determine" line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrchestratorState {
    pub phase: CoordinatorPhase,
    /// Topic the engine believes the coordinator should emit
    /// next. `None` when the engine cannot determine (fail-closed).
    pub expected_event: Option<String>,
    /// True when the upcoming emit is the last one in the
    /// current phase (e.g. the last plan unit, the last fix
    /// unit). The agent must consult U3 to honour this — the
    /// gate rewrites the topic and the budget protects the
    /// slot.
    pub last_in_phase: bool,
    /// The step id that triggered this computation (e.g.
    /// `step-01` / `fix-02`). Echoes the ledger so the
    /// agent does not have to re-derive it.
    pub completed_step: Option<String>,
    /// The next step id (when `expected_event = Some("work.ready")`).
    pub next_step: Option<String>,
    pub plan_unit_total: Option<u32>,
    pub fix_unit_total: Option<u32>,
    /// Human-readable reason — surfaced as the "reason" field
    /// in the prompt block so the agent (and `ralph diagnose`)
    /// can audit why the engine chose a particular
    /// `expected_event`.
    pub reason: String,
}

/// Pure function — given the loop's state, derive the next
/// `expected_event`. SSOT for the engine-side decision; the
/// agent never has to count `### U{N}.` itself.
pub fn compute_expected_event(
    topology: &PlanTopologyCache,
    input: &ComputeInput<'_>,
) -> OrchestratorState {
    // 1. Terminal: completion already honored. The engine
    //    does not have a legitimate `expected_event` for any
    //    hat; U2's persistent guard will reject any business
    //    emit, but we tell the coordinator explicitly so it
    //    doesn't try to be helpful.
    if input.completion_honored {
        return OrchestratorState {
            phase: CoordinatorPhase::Terminal,
            expected_event: None,
            last_in_phase: true,
            completed_step: None,
            next_step: None,
            plan_unit_total: topology.plan_unit_total(),
            fix_unit_total: topology.fix_unit_total(),
            reason: "completion_honored: LOOP_COMPLETE accepted; \
                     no further business event should be emitted"
                .to_string(),
        };
    }

    // 2. No trigger step yet — the coordinator is waking up
    //    cold (no test.passed preceded this activation).
    //    Surface the topology snapshot so the agent has
    //    N_total but no directive.
    let Some(step) = input.last_test_passed_step else {
        let phase = if input.last_was_fix_unit {
            CoordinatorPhase::FixUnit
        } else {
            CoordinatorPhase::PlanUnit
        };
        return OrchestratorState {
            phase,
            expected_event: None,
            last_in_phase: false,
            completed_step: None,
            next_step: None,
            plan_unit_total: topology.plan_unit_total(),
            fix_unit_total: topology.fix_unit_total(),
            reason: "no_test_passed_trigger: engine has no directive; \
                     agent should follow plan-level top-down review"
                .to_string(),
        };
    };

    // 3. test.passed(fix-NN) — fix-unit branch.
    if input.last_was_fix_unit || step.starts_with("fix-") {
        let fix_total = topology.fix_unit_total();
        let fix_pos = topology.fix_position(step);
        return match fix_pos {
            None if fix_total.is_none() => OrchestratorState {
                phase: CoordinatorPhase::FixUnit,
                expected_event: None,
                last_in_phase: false,
                completed_step: Some(step.to_string()),
                next_step: None,
                plan_unit_total: topology.plan_unit_total(),
                fix_unit_total: None,
                reason: format!(
                    "fix_topology_unparseable: '{}' not in fix topology; \
                     engine cannot decide between work.ready and plan.complete",
                    step
                ),
            },
            None => OrchestratorState {
                phase: CoordinatorPhase::FixUnit,
                expected_event: Some("work.ready".to_string()),
                last_in_phase: false,
                completed_step: Some(step.to_string()),
                next_step: Some(format!("fix-{:02}", fix_total.unwrap() + 1)),
                plan_unit_total: topology.plan_unit_total(),
                fix_unit_total: fix_total,
                reason: format!(
                    "fix_position_unknown: '{}' is not in the cached fix topology; \
                     defaulting to work.ready",
                    step
                ),
            },
            Some(pos) => {
                let total = fix_total.unwrap_or(pos);
                if pos >= total {
                    // Last fix unit — U3 rewrites to plan.complete.
                    OrchestratorState {
                        phase: CoordinatorPhase::FixUnit,
                        expected_event: Some("plan.complete".to_string()),
                        last_in_phase: true,
                        completed_step: Some(step.to_string()),
                        next_step: None,
                        plan_unit_total: topology.plan_unit_total(),
                        fix_unit_total: fix_total,
                        reason: format!(
                            "fix_unit_last: '{step}' is the {pos}/{total} fix unit; \
                             U3 gate rewrites work.ready → plan.complete"
                        ),
                    }
                } else {
                    let next = format!("fix-{:02}", pos + 1);
                    OrchestratorState {
                        phase: CoordinatorPhase::FixUnit,
                        expected_event: Some("work.ready".to_string()),
                        last_in_phase: false,
                        completed_step: Some(step.to_string()),
                        next_step: Some(next.clone()),
                        plan_unit_total: topology.plan_unit_total(),
                        fix_unit_total: fix_total,
                        reason: format!(
                            "fix_unit_mid: '{step}' is {pos}/{total}; next is '{next}'"
                        ),
                    }
                }
            }
        };
    }

    // 4. test.passed(step-NN) — plan-unit branch.
    let plan_total = topology.plan_unit_total();
    let plan_pos = topology.plan_position(step);
    match plan_pos {
        None if plan_total.is_none() => OrchestratorState {
            phase: CoordinatorPhase::PlanUnit,
            expected_event: None,
            last_in_phase: false,
            completed_step: Some(step.to_string()),
            next_step: None,
            plan_unit_total: None,
            fix_unit_total: topology.fix_unit_total(),
            reason: format!(
                "plan_topology_unparseable: '{step}' not in plan topology; \
                 engine cannot decide between work.ready and review.start"
            ),
        },
        None => OrchestratorState {
            phase: CoordinatorPhase::PlanUnit,
            expected_event: Some("work.ready".to_string()),
            last_in_phase: false,
            completed_step: Some(step.to_string()),
            next_step: Some(format!("step-{:02}", plan_total.unwrap() + 1)),
            plan_unit_total: plan_total,
            fix_unit_total: topology.fix_unit_total(),
            reason: format!(
                "plan_position_unknown: '{step}' is not in the cached plan topology; \
                 defaulting to work.ready"
            ),
        },
        Some(pos) => {
            let total = plan_total.unwrap_or(pos);
            if pos >= total {
                // Last plan unit — the next event is review.start
                // (or plan.complete if the review walk is
                // already closed, e.g. trivial plan).
                let next_topic = if input.review_walk_closed {
                    "plan.complete"
                } else {
                    "review.start"
                };
                OrchestratorState {
                    phase: CoordinatorPhase::PlanUnit,
                    expected_event: Some(next_topic.to_string()),
                    last_in_phase: true,
                    completed_step: Some(step.to_string()),
                    next_step: None,
                    plan_unit_total: plan_total,
                    fix_unit_total: topology.fix_unit_total(),
                    reason: format!(
                        "plan_unit_last: '{step}' is the {pos}/{total} plan unit; \
                         review_walk_closed={}; next is '{next_topic}'",
                        input.review_walk_closed
                    ),
                }
            } else {
                let next = format!("step-{:02}", pos + 1);
                OrchestratorState {
                    phase: CoordinatorPhase::PlanUnit,
                    expected_event: Some("work.ready".to_string()),
                    last_in_phase: false,
                    completed_step: Some(step.to_string()),
                    next_step: Some(next.clone()),
                    plan_unit_total: plan_total,
                    fix_unit_total: topology.fix_unit_total(),
                    reason: format!(
                        "plan_unit_mid: '{step}' is {pos}/{total}; next is '{next}'"
                    ),
                }
            }
        }
    }
}

/// Render the `## ORCHESTRATOR STATE` markdown block for the
/// given `OrchestratorState`. Returns the empty string when
/// the engine has no directive (the block is omitted from
/// the prompt to keep noise low; U1/U2 still back-stop the
/// emit).
pub fn render_orchestrator_state_block(state: &OrchestratorState) -> String {
    let payload = match serde_json::to_string_pretty(state) {
        Ok(s) => s,
        Err(_) => return String::new(),
    };
    format!(
        "## ORCHESTRATOR STATE\n\
         The engine computed the directive below from the events\n\
         ledger, the plan/fix-plan topology caches, and the loop\n\
         lifecycle. Do NOT count `### U{{N}}.` headings in `plan.md`\n\
         or `fix-plan.md` — read this block instead.\n\n\
         ```json\n\
         {payload}\n\
         ```\n\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topo(plan: &[&str], fix: &[&str]) -> PlanTopologyCache {
        PlanTopologyCache {
            plan_unit_ids: plan.iter().map(|s| s.to_string()).collect(),
            fix_unit_ids: fix.iter().map(|s| s.to_string()).collect(),
            plan_path: None,
            fix_plan_path: None,
        }
    }

    #[test]
    fn plan_unit_mid_emits_work_ready_for_next() {
        let topology = topo(&["step-01", "step-02"], &[]);
        let input = ComputeInput {
            last_test_passed_step: Some("step-01"),
            last_was_fix_unit: false,
            review_walk_closed: false,
            completion_honored: false,
        };
        let out = compute_expected_event(&topology, &input);
        assert_eq!(out.phase, CoordinatorPhase::PlanUnit);
        assert_eq!(out.expected_event.as_deref(), Some("work.ready"));
        assert_eq!(out.next_step.as_deref(), Some("step-02"));
        assert!(!out.last_in_phase);
    }

    #[test]
    fn plan_unit_last_emits_review_start_when_walk_open() {
        let topology = topo(&["step-01", "step-02"], &[]);
        let input = ComputeInput {
            last_test_passed_step: Some("step-02"),
            last_was_fix_unit: false,
            review_walk_closed: false,
            completion_honored: false,
        };
        let out = compute_expected_event(&topology, &input);
        assert_eq!(out.phase, CoordinatorPhase::PlanUnit);
        assert_eq!(out.expected_event.as_deref(), Some("review.start"));
        assert!(out.last_in_phase);
    }

    #[test]
    fn plan_unit_last_emits_plan_complete_when_walk_closed() {
        let topology = topo(&["step-01"], &[]);
        let input = ComputeInput {
            last_test_passed_step: Some("step-01"),
            last_was_fix_unit: false,
            review_walk_closed: true,
            completion_honored: false,
        };
        let out = compute_expected_event(&topology, &input);
        assert_eq!(out.expected_event.as_deref(), Some("plan.complete"));
        assert!(out.last_in_phase);
    }

    #[test]
    fn fix_unit_mid_emits_work_ready_for_next() {
        let topology = topo(&[], &["fix-01", "fix-02"]);
        let input = ComputeInput {
            last_test_passed_step: Some("fix-01"),
            last_was_fix_unit: true,
            review_walk_closed: true,
            completion_honored: false,
        };
        let out = compute_expected_event(&topology, &input);
        assert_eq!(out.phase, CoordinatorPhase::FixUnit);
        assert_eq!(out.expected_event.as_deref(), Some("work.ready"));
        assert_eq!(out.next_step.as_deref(), Some("fix-02"));
        assert!(!out.last_in_phase);
    }

    #[test]
    fn fix_unit_last_emits_plan_complete() {
        let topology = topo(&[], &["fix-01", "fix-02"]);
        let input = ComputeInput {
            last_test_passed_step: Some("fix-02"),
            last_was_fix_unit: true,
            review_walk_closed: true,
            completion_honored: false,
        };
        let out = compute_expected_event(&topology, &input);
        assert_eq!(out.phase, CoordinatorPhase::FixUnit);
        assert_eq!(out.expected_event.as_deref(), Some("plan.complete"));
        assert!(out.last_in_phase);
        assert!(out.reason.contains("U3 gate rewrites"));
    }

    #[test]
    fn empty_plan_topology_fails_closed() {
        let topology = topo(&[], &[]);
        let input = ComputeInput {
            last_test_passed_step: Some("step-01"),
            last_was_fix_unit: false,
            review_walk_closed: false,
            completion_honored: false,
        };
        let out = compute_expected_event(&topology, &input);
        assert_eq!(out.expected_event, None);
        assert!(out.reason.contains("plan_topology_unparseable"));
    }

    #[test]
    fn completion_honored_short_circuits_to_terminal() {
        let topology = topo(&["step-01"], &["fix-01"]);
        let input = ComputeInput {
            last_test_passed_step: Some("step-01"),
            last_was_fix_unit: false,
            review_walk_closed: true,
            completion_honored: true,
        };
        let out = compute_expected_event(&topology, &input);
        assert_eq!(out.phase, CoordinatorPhase::Terminal);
        assert_eq!(out.expected_event, None);
        assert!(out.last_in_phase);
    }

    #[test]
    fn no_test_passed_trigger_yields_no_directive() {
        let topology = topo(&["step-01"], &[]);
        let input = ComputeInput {
            last_test_passed_step: None,
            last_was_fix_unit: false,
            review_walk_closed: false,
            completion_honored: false,
        };
        let out = compute_expected_event(&topology, &input);
        assert_eq!(out.expected_event, None);
        assert!(out.reason.contains("no_test_passed_trigger"));
    }

    #[test]
    fn position_lookups_use_one_based_index() {
        let topology = topo(&["step-01", "step-02", "step-03"], &[]);
        assert_eq!(topology.plan_position("step-01"), Some(1));
        assert_eq!(topology.plan_position("step-02"), Some(2));
        assert_eq!(topology.plan_position("step-03"), Some(3));
        assert_eq!(topology.plan_position("step-04"), None);
        assert_eq!(topology.plan_unit_total(), Some(3));
        assert_eq!(topology.fix_unit_total(), None);
    }

    #[test]
    fn render_block_includes_expected_event_field() {
        let topology = topo(&["step-01", "step-02"], &[]);
        let input = ComputeInput {
            last_test_passed_step: Some("step-01"),
            last_was_fix_unit: false,
            review_walk_closed: false,
            completion_honored: false,
        };
        let out = compute_expected_event(&topology, &input);
        let block = render_orchestrator_state_block(&out);
        assert!(block.starts_with("## ORCHESTRATOR STATE"));
        assert!(block.contains("\"expected_event\""));
        assert!(block.contains("work.ready"));
    }

    // 2026-07-01-001 review P1-2: install path wiring tests.
    // The install helpers are the production SSOT for
    // populating `plan_unit_ids` / `fix_unit_ids`; without
    // them `compute_expected_event` falls back to
    // `*_topology_unparseable` and the coordinator never
    // sees the directive block.

    #[test]
    fn p1_2_install_plan_topology_populates_plan_unit_ids() {
        let dir = tempfile::TempDir::new().unwrap();
        let plan_path = dir.path().join("plan.md");
        std::fs::write(
            &plan_path,
            "### U1. intro\n### U2. body\n### U3. outro\n",
        )
        .unwrap();
        let mut cache = PlanTopologyCache::default();
        // The runtime always passes a `&mut StateLedger`.
        // For the unit test we use a fresh ledger with the
        // on-disk file disabled — install_plan_topology emits
        // the diagnostic through that channel but the cache
        // population is what we care about.
        let mut ledger = crate::state::StateLedger::new(dir.path(), false);
        cache
            .install_plan_topology(&mut ledger, &plan_path)
            .unwrap();
        assert_eq!(
            cache.plan_unit_ids,
            vec!["step-01".to_string(), "step-02".to_string(), "step-03".to_string()],
            "install_plan_topology must populate plan_unit_ids with step-NN"
        );
        assert_eq!(cache.plan_unit_total(), Some(3));
        assert_eq!(cache.plan_position("step-02"), Some(2));
    }

    #[test]
    fn p1_2_install_fix_topology_populates_fix_unit_ids() {
        let dir = tempfile::TempDir::new().unwrap();
        let fix_plan = dir.path().join("fix-plan.md");
        std::fs::write(
            &fix_plan,
            "### U1. fix A\n### U2. fix B\n",
        )
        .unwrap();
        let mut cache = PlanTopologyCache::default();
        let mut ledger = crate::state::StateLedger::new(dir.path(), false);
        cache
            .install_fix_topology(&mut ledger, &fix_plan)
            .unwrap();
        assert_eq!(
            cache.fix_unit_ids,
            vec!["fix-01".to_string(), "fix-02".to_string()],
            "install_fix_topology must populate fix_unit_ids with fix-NN"
        );
        assert_eq!(cache.fix_unit_total(), Some(2));
        assert_eq!(cache.fix_position("fix-01"), Some(1));
    }

    #[test]
    fn p1_2_install_plan_topology_empty_returns_err() {
        // A plan that does not match the `### U{N}.` convention
        // must fail-closed: empty `plan_unit_ids` and an Err
        // that callers can route into the diagnostic ledger.
        let dir = tempfile::TempDir::new().unwrap();
        let bogus = dir.path().join("bogus.md");
        std::fs::write(&bogus, "no headings here\n").unwrap();
        let mut cache = PlanTopologyCache::default();
        let mut ledger = crate::state::StateLedger::new(dir.path(), false);
        let result = cache.install_plan_topology(&mut ledger, &bogus);
        assert!(result.is_err());
        assert!(cache.plan_unit_ids.is_empty());
        assert_eq!(cache.plan_unit_total(), None);
    }

    #[test]
    fn p1_2_compute_expected_event_after_install_emits_directive() {
        // After install_plan_topology, the runtime is no
        // longer in the fail-closed state: compute_expected_event
        // produces a real `expected_event` instead of `None`.
        let dir = tempfile::TempDir::new().unwrap();
        let plan_path = dir.path().join("plan.md");
        std::fs::write(
            &plan_path,
            "### U1. one\n### U2. two\n",
        )
        .unwrap();
        let mut cache = PlanTopologyCache::default();
        let mut ledger = crate::state::StateLedger::new(dir.path(), false);
        cache
            .install_plan_topology(&mut ledger, &plan_path)
            .unwrap();
        let input = ComputeInput {
            last_test_passed_step: Some("step-01"),
            last_was_fix_unit: false,
            review_walk_closed: false,
            completion_honored: false,
        };
        let out = compute_expected_event(&cache, &input);
        assert_eq!(out.expected_event.as_deref(), Some("work.ready"));
        assert_eq!(out.next_step.as_deref(), Some("step-02"));
    }
}
