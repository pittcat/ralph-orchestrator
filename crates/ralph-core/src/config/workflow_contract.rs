//! WAC-U3: Workflow Activation Contract runtime configuration.
//!
//! Provides the `WorkflowContractConfig` block under
//! `event_loop.workflow_contract` so operators can tune the
//! runtime handoff dispatch behaviour without touching code.
//!
//! Plan Unit: WAC-U3 of `2026-06-12-002-feat-workflow-activation-contract-plan`.
//!
//! See [`crate::preset_lint::workflow_activation`] for the
//! matching static-rule family. The two modules share the
//! `HandoffGraph` data structure (constructed in either place)
//! so the static and runtime views cannot drift.

use serde::{Deserialize, Serialize};

/// Maximum allowed `handoff_dispatch_timeout_seconds`.
///
/// The 30s default + 120s ceiling is a deliberate usability
/// choice: a longer timeout turns a benign dispatch stall into
/// a multi-minute loop freeze. Operators who genuinely need
/// longer windows should redesign the workflow (split the
/// handoff into multiple stages with intermediate observable
/// state) rather than raise this knob.
pub const HANDOFF_DISPATCH_TIMEOUT_MAX_SECONDS: u64 = 120;

/// Default handoff dispatch timeout in seconds. KTD-11: 30s
/// is the operator-friendly default that surfaces dispatch
/// stalls within one operator attention cycle.
pub const HANDOFF_DISPATCH_TIMEOUT_DEFAULT_SECONDS: u64 = 30;

/// R7 seed handoff topics that the runtime dispatcher monitors
/// even when the static graph does not surface them as unique
/// consumers. The list is intentionally narrow — every entry
/// must be the kind of handoff that, if dropped, blocks the
/// entire workflow.
///
/// Plan KTD-12: `queue.advance` is a progress/audit signal, not
/// a dispatch guarantee; the runtime dispatcher still tracks it
/// for the priority pass but its absence does not trigger
/// escalation (R8 dispatch guarantee is reserved for topics
/// with a unique consumer — e.g. `work.ready`).
pub const HANDOFF_TOPIC_SEEDS: &[&str] = &[
    "queue.advance",
    "work.ready",
    "fix.plan.ready",
    "work.failed",
];

/// Workflow Activation Contract runtime configuration.
///
/// Loaded from the `event_loop.workflow_contract` block. The
/// block is optional; when absent the defaults
/// ([`HANDOFF_DISPATCH_TIMEOUT_DEFAULT_SECONDS`] and
/// [`HANDOFF_TOPIC_SEEDS`]) apply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowContractConfig {
    /// Maximum time (in seconds) the runtime dispatcher waits
    /// for a unique-consumer hat to activate after the handoff
    /// event is accepted. On expiry, the dispatcher emits a
    /// `stall_recovery` recovery envelope (KTD-13) and routes
    /// `task.resume` to a safe target. Clamped to
    /// [`HANDOFF_DISPATCH_TIMEOUT_MAX_SECONDS`].
    #[serde(default = "default_handoff_dispatch_timeout_seconds")]
    pub handoff_dispatch_timeout_seconds: u64,

    /// Explicit handoff topic seeds. The runtime effective
    /// set is `seeds ∪ unique_consumer_topics(graph)`. Conflicts
    /// (a seed that resolves to a multi-consumer topic) are
    /// surfaced as the
    /// [`FINDING_HANDOFF_SEED_DERIVED_CONFLICT`](crate::preset_lint::finding_id::FINDING_HANDOFF_SEED_DERIVED_CONFLICT)
    /// lint finding (KTD-6).
    #[serde(default = "default_handoff_topic_seeds")]
    pub handoff_topic_seeds: Vec<String>,
}

impl Default for WorkflowContractConfig {
    fn default() -> Self {
        Self {
            handoff_dispatch_timeout_seconds: default_handoff_dispatch_timeout_seconds(),
            handoff_topic_seeds: default_handoff_topic_seeds(),
        }
    }
}

fn default_handoff_dispatch_timeout_seconds() -> u64 {
    HANDOFF_DISPATCH_TIMEOUT_DEFAULT_SECONDS
}

fn default_handoff_topic_seeds() -> Vec<String> {
    HANDOFF_TOPIC_SEEDS.iter().map(|s| (*s).to_string()).collect()
}

impl WorkflowContractConfig {
    /// Clamp the dispatch timeout to the documented ceiling.
    /// Returns the effective value (caller may compare to detect
    /// coercion).
    pub fn effective_timeout_seconds(&self) -> u64 {
        self.handoff_dispatch_timeout_seconds
            .min(HANDOFF_DISPATCH_TIMEOUT_MAX_SECONDS)
    }

    /// Effective seed set after defaulting. The runtime may
    /// extend this with auto-derived unique-consumer topics
    /// (see [`crate::workflow_contract::handoff_index::HandoffIndex`]).
    pub fn effective_seeds(&self) -> &[String] {
        &self.handoff_topic_seeds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_timeout_is_30s() {
        let cfg = WorkflowContractConfig::default();
        assert_eq!(cfg.handoff_dispatch_timeout_seconds, 30);
    }

    #[test]
    fn default_seeds_contain_r7_topics() {
        let cfg = WorkflowContractConfig::default();
        for required in HANDOFF_TOPIC_SEEDS {
            assert!(
                cfg.handoff_topic_seeds.iter().any(|s| s == required),
                "default seeds must include `{required}`: {:?}",
                cfg.handoff_topic_seeds
            );
        }
    }

    #[test]
    fn timeout_above_ceiling_is_clamped() {
        let cfg = WorkflowContractConfig {
            handoff_dispatch_timeout_seconds: 600,
            handoff_topic_seeds: vec![],
        };
        assert_eq!(cfg.effective_timeout_seconds(), HANDOFF_DISPATCH_TIMEOUT_MAX_SECONDS);
    }

    #[test]
    fn timeout_at_ceiling_is_preserved() {
        let cfg = WorkflowContractConfig {
            handoff_dispatch_timeout_seconds: HANDOFF_DISPATCH_TIMEOUT_MAX_SECONDS,
            handoff_topic_seeds: vec![],
        };
        assert_eq!(
            cfg.effective_timeout_seconds(),
            HANDOFF_DISPATCH_TIMEOUT_MAX_SECONDS
        );
    }

    #[test]
    fn config_round_trips_through_yaml() {
        let yaml = r#"
handoff_dispatch_timeout_seconds: 45
handoff_topic_seeds:
  - queue.advance
  - work.ready
"#;
        let cfg: WorkflowContractConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.handoff_dispatch_timeout_seconds, 45);
        assert_eq!(cfg.handoff_topic_seeds, vec!["queue.advance", "work.ready"]);
    }
}
