//! 2026-07-02-006 plan U4: `WhitelistIndex`.
//!
//! Pure decision function
//! `allows(hat, topic, phase_id, &decl) -> bool`.
//!
//! The whitelist is the per-phase per-hat allowed topic set
//! declared in `mechanism.phase_authority.phases[*].allowed_emits`.
//! When a phase id is unknown to the index, the function
//! conservatively **denies** the emit — the runtime should not
//! evaluate anything past U2's parsed declaration. The function
//! takes the `PhaseAuthorityDeclaration` by reference so it is
//! trivially composable with U11's facade.

use super::declaration::PhaseAuthorityDeclaration;

/// Outcome of an `allows(...)` lookup. The struct preserves the
/// raw allow-list so `PhaseAuthorityStage` (U13) can render a
/// useful diagnostic when the engine rejects an emit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhitelistDecision {
    pub allowed: bool,
    /// Phase id the lookup was against (resolved from `phase_id`).
    pub phase_id: String,
    /// Topics the hat is permitted to emit in that phase, in the
    /// declaration order. Empty when the phase has no entry.
    pub allowed_topics: Vec<String>,
}

impl WhitelistDecision {
    pub fn deny(phase_id: impl Into<String>) -> Self {
        Self {
            allowed: false,
            phase_id: phase_id.into(),
            allowed_topics: Vec::new(),
        }
    }
}

/// Pure lookup against a `PhaseAuthorityDeclaration`.
///
/// `hat_id` is the hat (or role) attempting the emit. `topic`
/// is the candidate topic. `phase_id` must reference a phase
/// known to `decl`; unknown phases deny the lookup. An empty
/// `allowed_emits` map for a phase denies **everything** — the
/// engine treats the phase as terminal-closed.
pub fn allows(
    hat_id: &str,
    topic: &str,
    phase_id: &str,
    decl: &PhaseAuthorityDeclaration,
) -> WhitelistDecision {
    let Some(phase) = decl.phases.iter().find(|p| p.id == phase_id) else {
        return WhitelistDecision::deny(phase_id);
    };

    let allowed_topics: Vec<String> = phase
        .allowed_emits.values().flat_map(|topics| topics.iter().cloned())
        .collect();

    // Per-hat precise match takes priority; the catch-all entry
    // (hat_id == "*") is the fallback. When neither matches,
    // the emit is denied.
    let hat_topics = phase.allowed_emits.get(hat_id);
    let wildcard_topics = phase.allowed_emits.get("*");

    let allowed = hat_topics
        .map(|t| t.iter().any(|x| x == topic))
        .unwrap_or(false)
        || wildcard_topics
            .map(|t| t.iter().any(|x| x == topic))
            .unwrap_or(false);

    WhitelistDecision {
        allowed,
        phase_id: phase_id.to_string(),
        allowed_topics,
    }
}

#[cfg(test)]
mod tests {
    use super::super::config::*;
    use super::super::declaration::*;
    use super::*;

    fn build_decl() -> PhaseAuthorityDeclaration {
        let cfg = PhaseAuthorityConfig {
            enabled: true,
            initial_phase: Some("plan_end".to_string()),
            phases: vec![
                PhaseDeclConfig {
                    id: "unit_loop".to_string(),
                    label: None,
                    allowed_emits: [
                        ("coordinator".to_string(), vec!["work.ready".to_string()]),
                        ("executor".to_string(), vec!["work.done".to_string()]),
                    ]
                    .into_iter()
                    .collect(),
                },
                PhaseDeclConfig {
                    id: "plan_end".to_string(),
                    label: None,
                    allowed_emits: [(
                        "coordinator".to_string(),
                        vec!["plan.complete".to_string(), "plan.blocked".to_string()],
                    )]
                    .into_iter()
                    .collect(),
                },
            ],
            transitions: Vec::new(),
            violation_policy: ViolationPolicyConfig::default(),
            progress_projection: ProgressProjectionConfig::default(),
        };
        PhaseAuthorityDeclaration::try_from_config(&cfg).unwrap()
    }

    #[test]
    fn plan_end_rejects_review_start_for_coordinator() {
        let decl = build_decl();
        let d = allows("coordinator", "review.start", "plan_end", &decl);
        assert!(!d.allowed);
        assert_eq!(d.phase_id, "plan_end");
    }

    #[test]
    fn plan_end_allows_plan_complete_for_coordinator() {
        let decl = build_decl();
        let d = allows("coordinator", "plan.complete", "plan_end", &decl);
        assert!(d.allowed);
        assert!(d.allowed_topics.iter().any(|t| t == "plan.complete"));
    }

    #[test]
    fn unit_loop_allows_work_done_for_executor() {
        let decl = build_decl();
        let d = allows("executor", "work.done", "unit_loop", &decl);
        assert!(d.allowed);
    }

    #[test]
    fn unit_loop_rejects_plan_complete_for_executor() {
        let decl = build_decl();
        let d = allows("executor", "plan.complete", "unit_loop", &decl);
        assert!(!d.allowed);
    }

    #[test]
    fn unknown_phase_denies() {
        let decl = build_decl();
        let d = allows("coordinator", "plan.complete", "ship", &decl);
        assert!(!d.allowed);
        assert!(d.allowed_topics.is_empty());
    }

    #[test]
    fn wildcard_entry_permits_any_hat() {
        let cfg = PhaseAuthorityConfig {
            enabled: true,
            initial_phase: Some("terminal".to_string()),
            phases: vec![PhaseDeclConfig {
                id: "terminal".to_string(),
                label: None,
                allowed_emits: [("*".to_string(), vec!["report.done".to_string()])]
                    .into_iter()
                    .collect(),
            }],
            transitions: Vec::new(),
            violation_policy: ViolationPolicyConfig::default(),
            progress_projection: ProgressProjectionConfig::default(),
        };
        let decl = PhaseAuthorityDeclaration::try_from_config(&cfg).unwrap();
        let d = allows("reporter", "report.done", "terminal", &decl);
        assert!(d.allowed);
    }
}
