//! WAC-U3 / WAC-U5: Handoff Index — runtime view of "topics that
//! trigger a single downstream hat and must be dispatched within
//! the configured timeout".
//!
//! `HandoffIndex` is the runtime mirror of the static
//! [`HandoffGraph`](crate::preset_lint::workflow_activation::HandoffGraph).
//! Where the static graph answers "is this preset well-formed?",
//! the index answers "which handoffs must the dispatcher watch
//! for deadline escalation, and which hat is the unique consumer
//! of each?".
//!
//! Construction is pure: given a `RalphConfig` and the
//! `WorkflowContractConfig` block, the index is deterministic
//! and side-effect-free. The runtime path reads the index once
//! at loop start (or whenever the preset changes) and feeds
//! `consumer_of(topic)` lookups to the dispatcher's priority
//! pass.
//!
//! Plan Unit: WAC-U3 (configuration model) + part of WAC-U5
//! (priority-pass index) of
//! `2026-06-12-002-feat-workflow-activation-contract-plan`.

use std::collections::{BTreeMap, BTreeSet};

use crate::config::RalphConfig;
use crate::preset_lint::workflow_activation::HandoffGraph;

/// KTD-6: effective handoff topic set, sorted alphabetically,
/// keyed by topic. `BTreeMap` keeps the index deterministic for
/// snapshot tests and avoids accidental order dependency in
/// the dispatcher.
pub type HandoffIndexMap = BTreeMap<String, HandoffEntry>;

/// One entry in the [`HandoffIndex`].
///
/// `consumer` is `Some(hat_id)` for unique-consumer handoffs
/// (the dispatcher's priority-pass target). `None` means the
/// topic is a seed or wildcard-multi-consumer handoff and the
/// dispatcher treats it as a passive observability signal
/// (R7 / KTD-12: `queue.advance`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffEntry {
    /// Source of the topic: explicit seed from
    /// `workflow_contract.handoff_topic_seeds`, auto-derived
    /// from a unique consumer in the graph, or both.
    pub source: HandoffSource,
    /// Unique non-wildcard consumer, if any. `None` indicates
    /// the topic has multiple consumers or a wildcard
    /// subscriber — the dispatcher does not enable priority
    /// dispatch in that case (R9 / KTD-6).
    pub consumer: Option<String>,
}

impl HandoffEntry {
    /// `true` when the entry has a single deterministic
    /// consumer and is therefore eligible for priority
    /// dispatch.
    pub fn is_priority_dispatchable(&self) -> bool {
        self.consumer.is_some()
    }
}

/// Origin classification of a handoff topic.
///
/// `Both` is the KTD-6 conflict surface: the seed list
/// declared T but the graph also derived it (or, conversely,
/// the seed list omitted T but the graph has it). WAC-U3 lint
/// surfaces this as
/// [`FINDING_HANDOFF_SEED_DERIVED_CONFLICT`](crate::preset_lint::finding_id::FINDING_HANDOFF_SEED_DERIVED_CONFLICT).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffSource {
    /// Topic appears in the configured seed list only.
    Seed,
    /// Topic was auto-derived from the static graph only.
    Derived,
    /// Topic appears in both seeds and the graph.
    Both,
}

/// Runtime handoff index built from a `RalphConfig` and its
/// `workflow_contract` block.
///
/// Construction is cheap (O(hat_count + topic_count)) and the
/// index is immutable thereafter. The dispatcher's priority
/// pass consults `consumer_of(topic)` on every selection tick.
#[derive(Debug, Clone, Default)]
pub struct HandoffIndex {
    /// `topic → entry` for the effective set
    /// (`seeds ∪ unique_consumer_topics(graph)`).
    pub entries: HandoffIndexMap,
    /// KTD-6 conflicts: topics that appear in `seeds` but have
    /// no unique consumer (or vice versa). Surfaced via
    /// `conflicts()`.
    conflicts: Vec<HandoffConflict>,
}

/// A KTD-6 conflict between the seed list and the auto-derived
/// unique-consumer set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffConflict {
    pub topic: String,
    pub kind: ConflictKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// Seed listed T but T has no unique consumer in the
    /// graph (multi-consumer or wildcard). The derived view
    /// wins (KTD-6); the seed entry is preserved but with
    /// `consumer = None`.
    SeedWithoutUniqueConsumer,
    /// The graph derived T as a unique consumer but T is not
    /// in the seed list. The derived entry is added; this is
    /// not a failure but a notification for the operator.
    DerivedNotInSeed,
}

impl HandoffIndex {
    /// Build a `HandoffIndex` from a config + workflow contract.
    ///
    /// The defaults applied when `workflow_contract` is `None`
    /// are the documented ones
    /// ([`HANDOFF_DISPATCH_TIMEOUT_DEFAULT_SECONDS`](crate::config::HANDOFF_DISPATCH_TIMEOUT_DEFAULT_SECONDS),
    /// [`HANDOFF_TOPIC_SEEDS`](crate::config::HANDOFF_TOPIC_SEEDS)).
    pub fn from_config(config: &RalphConfig) -> Self {
        let contract = config
            .event_loop
            .workflow_contract
            .clone()
            .unwrap_or_default();
        let graph = HandoffGraph::from_config(config);

        // When the runtime is in coordinator mode, the index
        // is still constructed (so callers can introspect it)
        // but every consumer lookup is forced to `None` — the
        // priority pass is a no-op (R8: coordinator mode does
        // not enable handoff priority).
        let coordinator_mode = !matches!(
            config.event_loop.execution_mode,
            crate::config::HatExecutionMode::Isolated
        );

        let mut entries: HandoffIndexMap = BTreeMap::new();
        let mut conflicts: Vec<HandoffConflict> = Vec::new();

        // The starting_event is injected by the loop runner, not
        // by any user-defined hat. The dispatcher must not
        // enable priority for it: the runner owns the emit
        // timing and a handoff-style priority pass would
        // double-track the same event.
        let starting_event = config.event_loop.starting_event.as_deref();

        // Walk seeds first.
        for seed in contract.effective_seeds() {
            // Skip starting_event from the priority-eligible set
            // (it is recorded as a passive entry for observability
            // only).
            let consumer = if coordinator_mode || Some(seed.as_str()) == starting_event {
                None
            } else {
                graph.unique_consumer_of(seed).map(String::from)
            };
            let derived_present = graph.unique_consumer_topics().contains(seed);
            let source = if derived_present {
                HandoffSource::Both
            } else {
                HandoffSource::Seed
            };
            if consumer.is_none() && derived_present {
                // Should not happen (Both implies derived → unique_consumer_of
                // returns Some), but log defensively.
                conflicts.push(HandoffConflict {
                    topic: seed.clone(),
                    kind: ConflictKind::SeedWithoutUniqueConsumer,
                });
            } else if consumer.is_none() {
                // Seed listed but no unique consumer in graph.
                conflicts.push(HandoffConflict {
                    topic: seed.clone(),
                    kind: ConflictKind::SeedWithoutUniqueConsumer,
                });
            }
            entries.insert(seed.clone(), HandoffEntry { source, consumer });
        }

        // Walk graph-derived unique topics that are not already
        // in entries. starting_event is exempt (runner-owned).
        for topic in graph.unique_consumer_topics() {
            if entries.contains_key(&topic) {
                continue;
            }
            if Some(topic.as_str()) == starting_event {
                continue;
            }
            let consumer = if coordinator_mode {
                None
            } else {
                graph.unique_consumer_of(&topic).map(String::from)
            };
            // KTD-6: derived topics absent from the seed list are
            // not conflicts in the operator-facing sense — they
            // are exactly what auto-derivation is for. But the
            // spec asks us to surface the omission for audit.
            conflicts.push(HandoffConflict {
                topic: topic.clone(),
                kind: ConflictKind::DerivedNotInSeed,
            });
            entries.insert(
                topic,
                HandoffEntry {
                    source: HandoffSource::Derived,
                    consumer,
                },
            );
        }

        Self { entries, conflicts }
    }

    /// `true` if the index has any topic eligible for priority
    /// dispatch (i.e. any entry with a known consumer). The
    /// dispatcher checks this before doing the priority pass.
    pub fn has_any_priority(&self) -> bool {
        self.entries.values().any(|e| e.is_priority_dispatchable())
    }

    /// Look up the unique consumer of a topic, if any.
    pub fn consumer_of(&self, topic: &str) -> Option<&str> {
        self.entries.get(topic).and_then(|e| e.consumer.as_deref())
    }

    /// KTD-6 conflict list (cloned so callers cannot mutate the
    /// index).
    pub fn conflicts(&self) -> Vec<HandoffConflict> {
        self.conflicts.clone()
    }

    /// Sorted effective topic set.
    pub fn topics(&self) -> BTreeSet<String> {
        self.entries.keys().cloned().collect()
    }
}

/// U8 of plan 2026-07-05-005 (R5): shared hat-triggers checker
/// for the three handoff paths (`next_hat`, `process_output`
/// handoff escalation, `validate_resume_routing`). The
/// underlying logic — "does the hat's `triggers` list match
/// the topic?" — was previously inlined only at
/// `validate_resume_routing` (U16). Pulling it out as a
/// stand-alone helper means a hat that has the topic in its
/// `triggers` list is the single source of truth across all
/// three call sites. No new validation is added; the helper
/// just unifies the existing check.
pub fn check_hat_triggers(
    hat_triggers: &[String],
    topic: &str,
) -> Result<(), HandoffRoutingError> {
    let topic_obj = ralph_proto::Topic::new(topic);
    let matches = hat_triggers.iter().any(|t| {
        let pattern = ralph_proto::Topic::new(t);
        pattern.matches(&topic_obj)
    });
    if matches {
        Ok(())
    } else {
        Err(HandoffRoutingError::HatDoesNotConsume {
            hat_triggers: hat_triggers.to_vec(),
            topic: topic.to_string(),
        })
    }
}

/// U8 of plan 2026-07-05-005: error variants returned by
/// [`check_hat_triggers`]. The variant mirrors the existing
/// `validate_resume_routing` block-message style so callers can
/// surface the same diagnostic without inventing a new shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandoffRoutingError {
    /// The hat's `triggers` list does not declare the topic;
    /// routing the event to this hat would result in the
    /// activation dropping on the floor.
    HatDoesNotConsume {
        hat_triggers: Vec<String>,
        topic: String,
    },
}

impl std::fmt::Display for HandoffRoutingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandoffRoutingError::HatDoesNotConsume { hat_triggers, topic } => write!(
                f,
                "hat does not declare `{topic}` in its `triggers` list (declared: {:?})",
                hat_triggers
            ),
        }
    }
}

impl std::error::Error for HandoffRoutingError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HatExecutionMode;

    fn make_two_hat_yaml() -> String {
        r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  execution_mode: isolated
  workflow_contract:
    handoff_dispatch_timeout_seconds: 30
    handoff_topic_seeds:
      - queue.advance
      - work.ready
hats:
  plan_gate:
    name: "PlanGate"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
"#
        .to_string()
    }

    #[test]
    fn default_seeds_present() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let index = HandoffIndex::from_config(&config);
        for required in crate::config::HANDOFF_TOPIC_SEEDS {
            assert!(
                index.topics().contains(*required),
                "default seeds must include `{required}`: {:?}",
                index.topics()
            );
        }
    }

    #[test]
    fn derived_unique_consumer_joins_effective_set() {
        let config: RalphConfig = serde_yaml::from_str(&make_two_hat_yaml()).unwrap();
        let index = HandoffIndex::from_config(&config);
        // work.ready is a unique consumer (executor). It's also
        // a seed, so entry.source == Both and consumer == executor.
        let entry = index
            .entries
            .get("work.ready")
            .expect("work.ready must be in index");
        assert_eq!(entry.source, HandoffSource::Both);
        assert_eq!(entry.consumer.as_deref(), Some("executor"));
        assert!(index.has_any_priority());
        assert_eq!(index.consumer_of("work.ready"), Some("executor"));
    }

    #[test]
    fn multi_consumer_topic_yields_no_priority() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  execution_mode: isolated
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  b:
    name: "B"
    triggers: ["work.ready"]
    publishes: ["work.done"]
  c:
    name: "C"
    triggers: ["work.ready"]
    publishes: ["work.alt"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let index = HandoffIndex::from_config(&config);
        // work.ready has 2 explicit consumers → not unique, no
        // priority, but still in the effective set as a Seed entry.
        let entry = index
            .entries
            .get("work.ready")
            .expect("work.ready must be in index (seed)");
        assert_eq!(entry.consumer, None);
        assert!(!entry.is_priority_dispatchable());
        // Debug: every entry's consumer should be None for this
        // fixture. The preset has no unique consumer for any
        // seed topic, so the dispatcher must not enable
        // priority.
        for (topic, e) in &index.entries {
            assert_eq!(
                e.consumer, None,
                "topic {topic} unexpectedly has consumer {:?} in multi-consumer fixture",
                e.consumer
            );
        }
        assert!(!index.has_any_priority());
    }

    #[test]
    fn timeout_above_ceiling_clamped() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  execution_mode: isolated
  workflow_contract:
    handoff_dispatch_timeout_seconds: 2000
    handoff_topic_seeds: []
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let contract = config.event_loop.workflow_contract.unwrap();
        assert_eq!(
            contract.effective_timeout_seconds(),
            crate::config::HANDOFF_DISPATCH_TIMEOUT_MAX_SECONDS
        );
    }

    #[test]
    fn coordinator_mode_builds_index_but_consumer_is_none() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  execution_mode: coordinator
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  b:
    name: "B"
    triggers: ["work.ready"]
    publishes: ["work.done"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.event_loop.execution_mode,
            HatExecutionMode::Coordinator
        );
        let index = HandoffIndex::from_config(&config);
        // Index builds, but the priority pass target is None
        // because the runtime mode is coordinator.
        let entry = index
            .entries
            .get("work.ready")
            .expect("work.ready must be in index");
        assert_eq!(entry.consumer, None);
        assert!(!index.has_any_priority());
    }

    // ─────────────────────────────────────────────────────────────────
    // U8 of plan 2026-07-05-005 (R5): shared hat-triggers checker.
    // The helper is the single source of truth for the
    // "does this hat subscribe to this topic?" predicate used
    // across the three handoff paths.
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn u8_check_hat_triggers_matches_exact_topic() {
        let triggers = vec!["work.ready".to_string(), "task.resume".to_string()];
        check_hat_triggers(&triggers, "work.ready").expect("exact match");
    }

    #[test]
    fn u8_check_hat_triggers_matches_topic_pattern() {
        // The Topic::matches predicate supports `*` wildcard
        // segments (each `*` matches one topic segment). Pin
        // the helper so a future refactor of pattern matching
        // does not silently desync from the handoff_index's
        // `consumer_of` lookup.
        let triggers = vec!["review.*".to_string()];
        check_hat_triggers(&triggers, "review.dimension").expect("single-segment pattern match");
    }

    #[test]
    fn u8_check_hat_triggers_rejects_undeclared_topic() {
        let triggers = vec!["work.ready".to_string()];
        let err = check_hat_triggers(&triggers, "plan.complete").unwrap_err();
        match err {
            HandoffRoutingError::HatDoesNotConsume { topic, hat_triggers } => {
                assert_eq!(topic, "plan.complete");
                assert_eq!(hat_triggers, vec!["work.ready".to_string()]);
            }
        }
    }

    #[test]
    fn u8_check_hat_triggers_rejects_empty_triggers_list() {
        let err = check_hat_triggers(&[], "any.topic").unwrap_err();
        assert!(matches!(err, HandoffRoutingError::HatDoesNotConsume { .. }));
    }
}
