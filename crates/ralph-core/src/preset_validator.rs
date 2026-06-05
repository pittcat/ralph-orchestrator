//! Preset topology validator.
//!
//! Validates that a preset's hat configuration forms a valid topology:
//! - The starting event can reach at least one hat.
//! - The completion promise can be reached from the starting event.
//! - Required events are reachable and appear on all completion paths.
//!
//! The validator builds a topic-hat bipartite graph from the configured hats
//! (excluding the builtin `ralph` fallback) and performs BFS-based reachability
//! analysis. Wildcard subscriptions (e.g. `review.*`) are resolved using
//! `Topic::matches` semantics. Cycles are handled via a visited set to prevent
//! path explosion.

use crate::config::RalphConfig;
use crate::hat_registry::HatRegistry;
use crate::payload_contract::{
    PayloadContractError, PayloadContractValidationResult, validate_payload_contract,
};
use ralph_proto::{Hat, Topic};
use std::collections::{HashMap, HashSet, VecDeque};

/// Re-export payload contract types from `payload_contract` for callers
/// that prefer to import everything from the validator surface.
pub use crate::payload_contract::{
    PayloadContractErrorKind, PayloadContractValidationResult as _PayloadContractValidationResult,
};

/// Maximum number of BFS iterations to prevent infinite loops on cyclic graphs.
const MAX_BFS_ITERATIONS: usize = 1000;

/// Kind of topology error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyErrorKind {
    /// Starting event has no reachable hats.
    UnreachableStart,
    /// Completion promise has no reachable publisher.
    UnreachableCompletion,
    /// Required event is not reachable from the start.
    UnreachableRequired,
    /// Required event is not on all completion paths.
    RequiredEventNotOnAllPaths,
}

/// A single topology validation error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyError {
    pub kind: TopologyErrorKind,
    pub message: String,
}

/// Result of preset topology validation.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TopologyValidationResult {
    pub errors: Vec<TopologyError>,
    pub warnings: Vec<String>,
}

impl TopologyValidationResult {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Validates that the preset topology is sound.
///
/// Builds a topic-hat bipartite graph and checks:
/// 1. Starting event reaches at least one configured hat.
/// 2. Completion promise is reachable from the start.
/// 3. Every required event is reachable.
/// 4. Every required event appears on ALL completion paths.
///
/// Runtime-only fallback hats such as builtin `ralph` are ignored for path
/// analysis. Their wildcard subscription is useful at runtime, but treating it
/// as a normal graph edge would connect disconnected preset branches and hide
/// topology errors.
pub fn validate_preset_topology(
    config: &RalphConfig,
    registry: &HatRegistry,
) -> TopologyValidationResult {
    let mut result = TopologyValidationResult::default();

    if config.hats.is_empty() {
        return result;
    }

    let graph = TopologyGraph::build(config, registry);

    // 1. Check starting event reachability
    let start = config
        .event_loop
        .starting_event
        .clone()
        .unwrap_or_else(|| "task.start".to_string());

    if !graph.has_subscriber(&start) {
        result.errors.push(TopologyError {
            kind: TopologyErrorKind::UnreachableStart,
            message: format!(
                "Starting event '{}' has no configured hat subscribers",
                start
            ),
        });
    }

    // 2. Check completion promise reachability from start
    let completion = &config.event_loop.completion_promise;
    if !graph.is_topic_reachable_from(&start, completion) {
        result.errors.push(TopologyError {
            kind: TopologyErrorKind::UnreachableCompletion,
            message: format!(
                "Completion promise '{}' is not reachable from starting event '{}'",
                completion, start
            ),
        });
    }

    // 3. & 4. Check required events
    for required in &config.event_loop.required_events {
        if !graph.is_topic_reachable_from(&start, required) {
            result.errors.push(TopologyError {
                kind: TopologyErrorKind::UnreachableRequired,
                message: format!(
                    "Required event '{}' is not reachable from the starting event '{}'",
                    required, start
                ),
            });
            continue;
        }

        // Check that the required event is on all completion paths from start.
        // A required event is NOT on all completion paths if there exists
        // a completion path that avoids it. We check by blocking this topic
        // and all hats that publish it, then seeing if completion is still reachable.
        if !graph.is_required_on_all_paths(&start, completion, required) {
            result.errors.push(TopologyError {
                kind: TopologyErrorKind::RequiredEventNotOnAllPaths,
                message: format!(
                    "Required event '{}' is not on all completion paths from '{}'. \
                     Choose a topic that every successful path emits, or adjust hat topology.",
                    required, start
                ),
            });
        }
    }

    result
}

/// Check if a topic matches a pattern using the same topic glob semantics as runtime routing.
fn topic_matches(topic: &str, pattern: &str) -> bool {
    Topic::new(pattern).matches_str(topic)
}

/// Bipartite graph of topics <-> hats.
struct TopologyGraph<'a> {
    /// topic -> hats that subscribe to it
    topic_to_hats: HashMap<String, Vec<&'a Hat>>,
    /// hat -> topics it can publish (including default_publishes)
    hat_to_topics: HashMap<String, Vec<String>>,
}

impl<'a> TopologyGraph<'a> {
    fn build(config: &RalphConfig, registry: &'a HatRegistry) -> Self {
        let mut topic_to_hats: HashMap<String, Vec<&'a Hat>> = HashMap::new();
        let mut hat_to_topics: HashMap<String, Vec<String>> = HashMap::new();

        // Index configured hats from the registry. Runtime-only fallback hats
        // such as builtin `ralph` subscribe to `*`; including them here would
        // make disconnected branches appear reachable.
        for hat in registry.all().filter(|hat| !hat.is_fallback_only()) {
            // subscriptions (triggers)
            for sub in &hat.subscriptions {
                let topic_str = sub.as_str().to_string();
                topic_to_hats.entry(topic_str).or_default().push(hat);
            }

            // publishes
            let mut publishes = Vec::new();
            for pub_topic in &hat.publishes {
                let topic_str = pub_topic.as_str().to_string();
                publishes.push(topic_str);
            }

            // default_publishes from config
            if let Some(hat_config) = config.hats.get(hat.id.as_str()) {
                if let Some(default) = &hat_config.default_publishes {
                    let topic_str = default.clone();
                    if !publishes.contains(&topic_str) {
                        publishes.push(topic_str);
                    }
                }
                // Also include config.publishes (which may differ from hat.publishes)
                for pub_topic in &hat_config.publishes {
                    let topic_str = pub_topic.clone();
                    if !publishes.contains(&topic_str) {
                        publishes.push(topic_str);
                    }
                }
            }

            hat_to_topics.insert(hat.id.as_str().to_string(), publishes);
        }

        // Also index hats from config that might not be in registry yet
        for (hat_name, hat_config) in &config.hats {
            if hat_to_topics.contains_key(hat_name) {
                continue;
            }
            let mut publishes = Vec::new();
            for pub_topic in &hat_config.publishes {
                let topic_str = pub_topic.clone();
                publishes.push(topic_str);
            }
            if let Some(default) = &hat_config.default_publishes {
                let topic_str = default.clone();
                if !publishes.contains(&topic_str) {
                    publishes.push(topic_str);
                }
            }
            for trigger in &hat_config.triggers {
                let topic_str = trigger.clone();
                topic_to_hats.entry(topic_str).or_default();
            }
            hat_to_topics.insert(hat_name.clone(), publishes);
        }

        Self {
            topic_to_hats,
            hat_to_topics,
        }
    }

    /// Check if a topic is reachable from the starting event through the graph.
    /// Uses BFS starting from `start_topic`, following topic -> hat -> topic edges.
    fn is_topic_reachable_from(&self, start_topic: &str, target: &str) -> bool {
        let mut visited_topics: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();

        // Seed with the concrete starting event even when the matching trigger
        // is a wildcard pattern like `work.*`.
        visited_topics.insert(start_topic.to_string());
        queue.push_back(start_topic.to_string());

        let mut iterations = 0;

        while let Some(topic) = queue.pop_front() {
            iterations += 1;
            if iterations > MAX_BFS_ITERATIONS {
                break; // Cycle protection
            }

            if topic == target {
                return true;
            }

            // Find hats that subscribe to this topic (exact match or wildcard)
            let subscribing_hats: Vec<&Hat> = self
                .topic_to_hats
                .iter()
                .filter(|(t, _)| topic_matches(&topic, t))
                .flat_map(|(_, hats)| hats.iter().copied())
                .collect();

            for hat in subscribing_hats {
                // Get topics this hat can publish
                if let Some(publishes) = self.hat_to_topics.get(hat.id.as_str()) {
                    for pub_topic in publishes {
                        if visited_topics.insert(pub_topic.clone()) {
                            queue.push_back(pub_topic.clone());
                        }
                    }
                }
            }
        }

        false
    }

    fn has_subscriber(&self, topic: &str) -> bool {
        self.topic_to_hats
            .iter()
            .any(|(trigger, hats)| topic_matches(topic, trigger) && !hats.is_empty())
    }

    /// Check if a required event appears on ALL completion paths from the starting event.
    ///
    /// Strategy: block the required topic from BFS traversal. If the completion
    /// promise is still reachable without visiting the required topic, then there
    /// exists a path that avoids it — meaning it is NOT on all paths.
    ///
    /// This correctly handles:
    /// - Linear chains: blocking the required topic breaks the only path
    /// - Branching paths: an alternative branch can bypass the required topic
    /// - Hats publishing both required and completion topics: the direct
    ///   completion edge is blocked while checking for bypass paths
    /// - Wildcard triggers: resolved via topic_matches
    /// - Cycles: bounded by MAX_BFS_ITERATIONS and visited set
    fn is_required_on_all_paths(
        &self,
        start_topic: &str,
        completion: &str,
        required: &str,
    ) -> bool {
        // BFS from start, blocking the required topic from being visited
        let mut visited_topics: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();

        // Seed with the concrete starting event even when the matching trigger
        // is represented by a wildcard pattern in the graph.
        if start_topic != required {
            visited_topics.insert(start_topic.to_string());
            queue.push_back(start_topic.to_string());
        }

        let mut iterations = 0;

        while let Some(topic) = queue.pop_front() {
            iterations += 1;
            if iterations > MAX_BFS_ITERATIONS {
                break; // Cycle protection
            }

            if topic == completion {
                // Completion is reachable without the required event
                return false;
            }

            // Find hats that subscribe to this topic (exact or wildcard match)
            let subscribing_hats: Vec<&Hat> = self
                .topic_to_hats
                .iter()
                .filter(|(t, _)| topic_matches(&topic, t))
                .flat_map(|(_, hats)| hats.iter().copied())
                .collect();

            for hat in subscribing_hats {
                // Get topics this hat can publish (excluding the required topic)
                if let Some(publishes) = self.hat_to_topics.get(hat.id.as_str()) {
                    let hat_publishes_required = publishes.iter().any(|topic| topic == required);
                    for pub_topic in publishes {
                        if pub_topic == required {
                            continue; // Block the required topic from being visited
                        }
                        if hat_publishes_required && pub_topic == completion {
                            continue;
                        }
                        if pub_topic == completion {
                            // Completion reachable without visiting required topic
                            return false;
                        }
                        if visited_topics.insert(pub_topic.clone()) {
                            queue.push_back(pub_topic.clone());
                        }
                    }
                }
            }
        }

        // Completion not reachable without required topic -> it's on all paths
        true
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Combined preset validation (U3 integration)
// ──────────────────────────────────────────────────────────────────────────

/// Combined result of preset validation (topology + payload contracts).
#[derive(Debug, Default, Clone)]
pub struct PresetValidationResult {
    pub topology: TopologyValidationResult,
    pub payload_contracts: PayloadContractValidationResult,
}

impl PresetValidationResult {
    pub fn is_valid(&self) -> bool {
        self.topology.is_valid() && self.payload_contracts.is_valid()
    }

    /// Number of topology errors + payload contract errors.
    pub fn error_count(&self) -> usize {
        self.topology.errors.len() + self.payload_contracts.errors.len()
    }
}

/// Run both topology and payload-contract validation in one call.
///
/// `strict` controls payload-contract strictness (see
/// `validate_payload_contract`).
pub fn validate_preset(
    config: &RalphConfig,
    registry: &HatRegistry,
    strict: bool,
) -> PresetValidationResult {
    let topology = validate_preset_topology(config, registry);
    let payload_contracts = validate_payload_contract(config, registry, strict);
    PresetValidationResult {
        topology,
        payload_contracts,
    }
}

/// Format a payload contract error as a single human-readable line.
pub fn format_payload_contract_error(err: &PayloadContractError) -> String {
    match &err.field {
        Some(field) => format!(
            "[{}] hat={} topic={} field={} source_hats=[{}] schema={} line={:?} pattern={:?}",
            match err.kind {
                PayloadContractErrorKind::FieldMissingFromSchema => "FieldMissingFromSchema",
                PayloadContractErrorKind::SchemaMissingForRequiredTopic =>
                    "SchemaMissingForRequiredTopic",
            },
            err.hat_id,
            err.topic,
            field,
            err.source_hats.join(", "),
            err.schema_defined_in,
            err.instructions_line,
            err.pattern,
        ),
        None => format!(
            "[{}] hat={} topic={} source_hats=[{}] schema={}",
            match err.kind {
                PayloadContractErrorKind::FieldMissingFromSchema => "FieldMissingFromSchema",
                PayloadContractErrorKind::SchemaMissingForRequiredTopic =>
                    "SchemaMissingForRequiredTopic",
            },
            err.hat_id,
            err.topic,
            err.source_hats.join(", "),
            err.schema_defined_in,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RalphConfig;

    fn empty_registry() -> HatRegistry {
        HatRegistry::default()
    }

    fn runtime_registry(yaml: &str) -> HatRegistry {
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        HatRegistry::from_runtime_config(&config)
    }

    #[test]
    fn empty_registry_is_valid() {
        let config = RalphConfig::default();
        let registry = empty_registry();
        let result = validate_preset_topology(&config, &registry);
        assert!(result.is_valid());
    }

    #[test]
    fn runtime_registry_with_only_ralph_is_valid() {
        // Solo mode: only builtin ralph in registry, no custom hats.
        let config = RalphConfig::default();
        let registry = HatRegistry::from_runtime_config(&config);
        assert!(!registry.is_empty(), "Runtime registry has ralph");
        let result = validate_preset_topology(&config, &registry);
        assert!(result.is_valid(), "Solo mode should be valid");
    }

    // -- Reachability tests ------------------------------------------------

    #[test]
    fn linear_chain_start_to_completion() {
        // start -> A(mid) -> B(done) -> LOOP_COMPLETE
        // Required: "done" -> passes (on all paths)
        let yaml = r#"
hats:
  a:
    name: "A"
    triggers: ["start"]
    publishes: ["mid"]
  b:
    name: "B"
    triggers: ["mid"]
    publishes: ["done"]
  c:
    name: "C"
    triggers: ["done"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "start"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["done"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(
            result.is_valid(),
            "Linear chain with 'done' required should pass: {:?}",
            result.errors
        );
    }

    #[test]
    fn branching_path_required_not_on_all() {
        // start -> A(mid) -> B(done) -> LOOP_COMPLETE
        // start -> A2(mid2) -> LOOP_COMPLETE (bypass)
        // Required: "done" -> fails (bypass exists)
        let yaml = r#"
hats:
  a:
    name: "A"
    triggers: ["start"]
    publishes: ["mid"]
  b:
    name: "B"
    triggers: ["mid"]
    publishes: ["done"]
  a2:
    name: "A2"
    triggers: ["start"]
    publishes: ["mid2"]
  c:
    name: "C"
    triggers: ["done", "mid2"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "start"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["done"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(
            !result.is_valid(),
            "Branching path: 'done' should NOT be on all paths"
        );
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.kind == TopologyErrorKind::RequiredEventNotOnAllPaths)
        );
    }

    #[test]
    fn unreachable_required_event() {
        let yaml = r#"
hats:
  a:
    name: "A"
    triggers: ["start"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "start"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["nonexistent"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(!result.is_valid());
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.kind == TopologyErrorKind::UnreachableRequired)
        );
    }

    #[test]
    fn unreachable_completion_promise() {
        let yaml = r#"
hats:
  a:
    name: "A"
    triggers: ["start"]
    publishes: ["done"]
event_loop:
  starting_event: "start"
  completion_promise: "FAR_AWAY"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(!result.is_valid());
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.kind == TopologyErrorKind::UnreachableCompletion)
        );
    }

    // -- Wildcard trigger tests --------------------------------------------

    #[test]
    fn wildcard_trigger_resolves_correctly() {
        // Hat subscribes to "review.*" which should match "review.done"
        // The starting event "review.start" is consumed by the reviewer via wildcard
        let yaml = r#"
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["review.*"]
    publishes: ["review.done"]
  c:
    name: "C"
    triggers: ["review.done"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "review.start"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["review.done"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(
            result.is_valid(),
            "Wildcard trigger 'review.*' should match 'review.done': {:?}",
            result.errors
        );
    }

    // -- default_publishes tests -------------------------------------------

    #[test]
    fn default_publishes_to_completion() {
        let yaml = r#"
hats:
  a:
    name: "A"
    triggers: ["start"]
    publishes: []
    default_publishes: "LOOP_COMPLETE"
event_loop:
  starting_event: "start"
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(
            result.is_valid(),
            "default_publishes to completion should be recognized: {:?}",
            result.errors
        );
    }

    #[test]
    fn default_publishes_is_reachable_topic() {
        let yaml = r#"
hats:
  a:
    name: "A"
    triggers: ["start"]
    default_publishes: "work.done"
  b:
    name: "B"
    triggers: ["work.done"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "start"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["work.done"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(
            result.is_valid(),
            "default_publishes 'work.done' should be reachable and required: {:?}",
            result.errors
        );
    }

    // -- Cycle detection tests ---------------------------------------------

    #[test]
    fn cyclic_topology_does_not_stack_overflow() {
        let yaml = r#"
hats:
  a:
    name: "A"
    triggers: ["start", "b.done"]
    publishes: ["a.done"]
  b:
    name: "B"
    triggers: ["a.done"]
    publishes: ["b.done"]
  c:
    name: "C"
    triggers: ["b.done"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "start"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["C"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(!result.is_valid());
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.kind == TopologyErrorKind::UnreachableRequired)
        );
    }

    #[test]
    fn graph_cycle_does_not_cause_infinite_loop() {
        let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.start", "work.retry"]
    publishes: ["work.done", "work.retry"]
  reporter:
    name: "Reporter"
    triggers: ["work.done"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["work.done"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(
            result.is_valid(),
            "graph with cycle should still validate correctly: {:?}",
            result.errors
        );
    }

    // -- Mutually exclusive branch events ----------------------------------

    #[test]
    fn mutually_exclusive_branch_events_fail() {
        let yaml = r#"
hats:
  review-synthesizer:
    name: "Synthesizer"
    triggers: ["review.dimension.done"]
    publishes: ["review.passed", "review.complete"]
  shipper:
    name: "Shipper"
    triggers: ["review.passed"]
    publishes: ["REVIEW_COMPLETE"]
  reporter:
    name: "Reporter"
    triggers: ["REVIEW_COMPLETE", "review.complete"]
    publishes: ["report.done", "LOOP_COMPLETE"]
event_loop:
  starting_event: "review.dimension.done"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["review.passed", "review.complete"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        // review.passed is NOT on all paths because:
        // review.dimension.done -> review-synthesizer -> review.complete -> reporter -> LOOP_COMPLETE
        // This path avoids review.passed entirely.
        assert!(
            !result.is_valid(),
            "Mutually exclusive branch events should fail validation: {:?}",
            result.errors
        );
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.kind == TopologyErrorKind::RequiredEventNotOnAllPaths),
            "Should report RequiredEventNotOnAllPaths"
        );
    }

    // -- ce-executor topology tests ----------------------------------------

    #[test]
    fn ce_executor_topology_is_valid() {
        let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["work.start"]
    publishes: ["work.ready", "work.failed"]
    default_publishes: "work.failed"
  executor:
    name: "Executor"
    triggers: ["work.ready", "queue.advance", "work.retry"]
    publishes: ["work.done", "work.failed"]
    # U2: default_publishes removed — executor MUST explicitly emit
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["work.done", "fix.applied"]
    publishes: ["review.wave.ready", "review.passed"]
  dimension-reviewer:
    name: "Dimension Reviewer"
    triggers: ["review.wave.ready"]
    publishes: ["review.dimension.done"]
  review-synthesizer:
    name: "Review Synthesizer"
    triggers: ["review.dimension.done"]
    publishes: ["review.passed", "review.failed", "review.complete"]
    default_publishes: "review.complete"
  fixer:
    name: "Fixer"
    triggers: ["review.failed"]
    publishes: ["fix.applied", "fix.exhausted"]
    default_publishes: "fix.exhausted"
  plan-gate:
    name: "Plan Gate"
    triggers: ["review.passed", "review.complete"]
    publishes: ["queue.advance", "plan.complete", "plan.blocked"]
    default_publishes: "plan.blocked"
  shipper:
    name: "Shipper"
    triggers: ["plan.complete", "plan.blocked", "fix.exhausted"]
    publishes: ["REVIEW_COMPLETE"]
    default_publishes: "REVIEW_COMPLETE"
  reporter:
    name: "Reporter"
    triggers: ["REVIEW_COMPLETE"]
    publishes: ["report.done", "LOOP_COMPLETE"]
    default_publishes: "report.done"
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]
  verdict_gate:
    topic: "REVIEW_COMPLETE"
    fail_field: "pass_or_fail"
    fail_value: "fail"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(
            result.is_valid(),
            "ce-executor topology should be valid: {:?}",
            result.errors
        );
    }

    #[test]
    fn ce_executor_verdict_gate_rejects_fail_review_complete() {
        // R10: The verdict gate must reject LOOP_COMPLETE when REVIEW_COMPLETE
        // carries pass_or_fail == "fail". This is a backstop for plan.blocked
        // and fix.exhausted paths that still emit REVIEW_COMPLETE.
        let yaml = r#"
hats:
  shipper:
    name: "Shipper"
    triggers: ["plan.complete", "plan.blocked"]
    publishes: ["REVIEW_COMPLETE"]
  reporter:
    name: "Reporter"
    triggers: ["REVIEW_COMPLETE"]
    publishes: ["report.done", "LOOP_COMPLETE"]
    default_publishes: "report.done"
event_loop:
  starting_event: "plan.complete"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]
  verdict_gate:
    topic: "REVIEW_COMPLETE"
    fail_field: "pass_or_fail"
    fail_value: "fail"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(
            result.is_valid(),
            "verdict_gate on REVIEW_COMPLETE should be valid: {:?}",
            result.errors
        );
    }

    #[test]
    fn ce_executor_report_done_is_on_all_paths() {
        let yaml = r#"
hats:
  reporter:
    name: "Reporter"
    triggers: ["REVIEW_COMPLETE"]
    publishes: ["report.done", "LOOP_COMPLETE"]
    default_publishes: "report.done"
event_loop:
  starting_event: "REVIEW_COMPLETE"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(
            result.is_valid(),
            "report.done should be on all paths to LOOP_COMPLETE: {:?}",
            result.errors
        );
    }

    // -- Fallback path detection -------------------------------------------

    #[test]
    fn starting_event_with_no_subscriber_fails_static_topology_validation() {
        let yaml = r#"
hats:
  a:
    name: "A"
    triggers: ["other.topic"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "start"
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(
            result.errors.iter().any(|error| {
                error.kind == TopologyErrorKind::UnreachableStart && error.message.contains("start")
            }),
            "Static topology validation should not use fallback ralph to connect start: {:?}",
            result.errors
        );
    }

    // -- Config-only hats (not in registry) --------------------------------

    #[test]
    fn config_hats_not_in_registry_are_indexed() {
        let yaml = r#"
hats:
  a:
    name: "A"
    triggers: ["start"]
    publishes: ["mid"]
  b:
    name: "B"
    triggers: ["mid"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "start"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["mid"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(
            result.is_valid(),
            "Config hats should be indexed for topology analysis: {:?}",
            result.errors
        );
    }

    // -- Error message quality ---------------------------------------------

    #[test]
    fn error_messages_are_descriptive() {
        let yaml = r#"
hats:
  a:
    name: "A"
    triggers: ["start"]
    publishes: ["done"]
  b:
    name: "B"
    triggers: ["done"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "start"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["nonexistent"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(!result.is_valid());
        let err = result
            .errors
            .iter()
            .find(|error| error.message.contains("nonexistent"))
            .expect("expected required-event error for nonexistent topic");
        assert!(
            err.message.contains("nonexistent"),
            "Error should mention the topic: {}",
            err.message
        );
        assert!(
            err.message.contains("start"),
            "Error should mention starting event: {}",
            err.message
        );
    }

    // -- Regression: old off-graph topic test updated ----------------------

    #[test]
    fn off_graph_completion_topic_fails() {
        // FAR_AWAY_COMPLETE is not published by any non-fallback hat.
        // The builtin ralph is excluded from graph analysis, so this
        // should now correctly fail.
        let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["work.done"]
event_loop:
  starting_event: "work.start"
  completion_promise: "FAR_AWAY_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(
            !result.is_valid(),
            "FAR_AWAY_COMPLETE should fail: not reachable from start"
        );
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.kind == TopologyErrorKind::UnreachableCompletion),
            "Should report UnreachableCompletion"
        );
    }

    // -- Required event on one branch only (regression) --------------------

    #[test]
    fn required_event_only_on_one_completion_branch_fails() {
        let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["work.start"]
    publishes: ["needs.review", "skip.review"]
  reviewer:
    name: "Reviewer"
    triggers: ["needs.review"]
    publishes: ["review.passed"]
  reviewed_reporter:
    name: "Reviewed Reporter"
    triggers: ["review.passed"]
    publishes: ["LOOP_COMPLETE"]
  direct_reporter:
    name: "Direct Reporter"
    triggers: ["skip.review"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["review.passed"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);

        assert!(
            result.errors.iter().any(|error| {
                error.kind == TopologyErrorKind::RequiredEventNotOnAllPaths
                    && error.message.contains("review.passed")
            }),
            "required event missing from one completion branch should fail: {:?}",
            result.errors
        );
    }

    // -- Configured completion promise (not LOOP_COMPLETE) -----------------

    #[test]
    fn validator_uses_configured_completion_promise() {
        let yaml = r#"
hats:
  analyzer:
    name: "Analyzer"
    triggers: ["review.start"]
    publishes: ["analysis.complete"]
  finalizer:
    name: "Finalizer"
    triggers: ["analysis.complete"]
    publishes: ["REVIEW_COMPLETE"]
event_loop:
  starting_event: "review.start"
  completion_promise: "REVIEW_COMPLETE"
  required_events: ["analysis.complete"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);

        assert!(
            result.is_valid(),
            "configured non-LOOP completion promise should validate: {:?}",
            result.errors
        );
    }

    // -- Completion unreachable from start (disconnected branch) -----------

    #[test]
    fn completion_unreachable_from_start_fails_even_if_topic_exists() {
        let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["work.done"]
  orphan_reporter:
    name: "Orphan Reporter"
    triggers: ["orphan.done"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);

        assert!(
            result.errors.iter().any(|error| {
                error.kind == TopologyErrorKind::UnreachableCompletion
                    && error.message.contains("LOOP_COMPLETE")
            }),
            "completion topic that exists only on disconnected branch should fail: {:?}",
            result.errors
        );
    }

    // -- topic_matches helper tests ----------------------------------------

    #[test]
    fn topic_matches_wildcard() {
        assert!(topic_matches("review.passed", "review.*"));
        assert!(topic_matches("review.failed", "review.*"));
        assert!(topic_matches("review.complete", "review.*"));
        assert!(!topic_matches("review", "review.*")); // missing dot
        assert!(!topic_matches("review.passed.extra", "review.*")); // extra segment
        assert!(!topic_matches("work.done", "review.*"));
        assert!(topic_matches("exact", "exact")); // exact match
        assert!(!topic_matches("exact.match", "exact")); // no wildcard
    }

    // -- Required event unreachable (disconnected branch) ------------------

    #[test]
    fn required_event_on_disconnected_branch_fails() {
        let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["work.done"]
  reporter:
    name: "Reporter"
    triggers: ["work.done"]
    publishes: ["LOOP_COMPLETE"]
  reviewer:
    name: "Reviewer"
    triggers: ["orphan.start"]
    publishes: ["review.passed"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["review.passed"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);

        assert!(
            result.errors.iter().any(|error| {
                error.kind == TopologyErrorKind::UnreachableRequired
                    && error.message.contains("review.passed")
            }),
            "required event on disconnected branch should fail: {:?}",
            result.errors
        );
    }

    // -- ce-executor multi-step advancement regression tests ---------------

    #[test]
    fn ce_executor_queue_advance_is_reachable_from_start() {
        // R11: After a step passes review, plan-gate must be able to emit
        // queue.advance so the executor can run the next step.
        let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready", "queue.advance"]
    publishes: ["work.done"]
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["work.done"]
    publishes: ["review.wave.ready"]
  dimension-reviewer:
    name: "Dimension Reviewer"
    triggers: ["review.wave.ready"]
    publishes: ["review.dimension.done"]
  review-synthesizer:
    name: "Review Synthesizer"
    triggers: ["review.dimension.done"]
    publishes: ["review.passed"]
  plan-gate:
    name: "Plan Gate"
    triggers: ["review.passed"]
    publishes: ["queue.advance", "plan.complete"]
    default_publishes: "plan.complete"
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let graph = TopologyGraph::build(&config, &registry);
        assert!(
            graph.is_topic_reachable_from("work.start", "queue.advance"),
            "queue.advance must be reachable from work.start through plan-gate"
        );
    }

    #[test]
    fn ce_executor_plan_complete_path_is_reachable() {
        // R11: The final completion path via plan.complete -> REVIEW_COMPLETE
        // -> report.done -> LOOP_COMPLETE must be reachable.
        let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
  review-synthesizer:
    name: "Review Synthesizer"
    triggers: ["work.done"]
    publishes: ["review.passed"]
  plan-gate:
    name: "Plan Gate"
    triggers: ["review.passed"]
    publishes: ["plan.complete"]
  shipper:
    name: "Shipper"
    triggers: ["plan.complete"]
    publishes: ["REVIEW_COMPLETE"]
  reporter:
    name: "Reporter"
    triggers: ["REVIEW_COMPLETE"]
    publishes: ["report.done", "LOOP_COMPLETE"]
event_loop:
  starting_event: "work.ready"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(
            result.is_valid(),
            "plan.complete completion path should be valid: {:?}",
            result.errors
        );
    }

    #[test]
    fn ce_executor_report_done_still_on_all_paths_with_plan_gate() {
        // R12: Adding plan-gate must not break the existing completion gate.
        // report.done must still be on all paths to LOOP_COMPLETE.
        let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready", "queue.advance"]
    publishes: ["work.done"]
  review-synthesizer:
    name: "Review Synthesizer"
    triggers: ["work.done"]
    publishes: ["review.passed", "review.complete"]
  plan-gate:
    name: "Plan Gate"
    triggers: ["review.passed", "review.complete"]
    publishes: ["queue.advance", "plan.complete", "plan.blocked"]
  shipper:
    name: "Shipper"
    triggers: ["plan.complete", "plan.blocked"]
    publishes: ["REVIEW_COMPLETE"]
  reporter:
    name: "Reporter"
    triggers: ["REVIEW_COMPLETE"]
    publishes: ["report.done", "LOOP_COMPLETE"]
    default_publishes: "report.done"
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(
            result.is_valid(),
            "report.done must remain on all paths after adding plan-gate: {:?}",
            result.errors
        );
    }

    // ──────────────────────────────────────────────────────────────────────
    // U3 integration: combined preset validation
    // ──────────────────────────────────────────────────────────────────────

    #[test]
    fn validate_preset_combines_topology_and_payload_contracts() {
        // Topology: valid. Payload contracts: schema exists but field is
        // missing. Combined result must report the payload-contract error.
        let yaml = r#"
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  event_policy:
    enabled: true
    mode: observe
    schemas:
      work.ready:
        required_fields: ["task_id"]
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  b:
    name: "B"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id, plan_name
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset(&config, &registry, false);
        assert!(result.topology.is_valid());
        assert!(!result.payload_contracts.is_valid());
        assert!(!result.is_valid());
        assert!(
            result
                .payload_contracts
                .errors
                .iter()
                .any(|e| e.field.as_deref() == Some("plan_name"))
        );
    }

    #[test]
    fn validate_preset_default_mode_missing_schema_is_warning() {
        // Default mode: missing schema is a warning, not an error.
        let yaml = r#"
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  b:
    name: "B"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset(&config, &registry, false);
        assert!(result.is_valid());
        assert!(!result.payload_contracts.warnings.is_empty());
    }

    #[test]
    fn validate_preset_strict_mode_missing_schema_is_error() {
        // Strict mode: missing schema is an error.
        let yaml = r#"
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  b:
    name: "B"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset(&config, &registry, true);
        assert!(!result.is_valid());
        assert!(!result.payload_contracts.errors.is_empty());
    }

    #[test]
    fn format_payload_contract_error_field_missing_includes_required_fields() {
        let err = PayloadContractError {
            kind: PayloadContractErrorKind::FieldMissingFromSchema,
            hat_id: "executor".to_string(),
            topic: "work.ready".to_string(),
            field: Some("plan_name".to_string()),
            source_hats: vec!["coordinator".to_string()],
            schema_defined_in: "inline".to_string(),
            instructions_line: Some(12),
            pattern: Some("From event payload".to_string()),
            source_excerpt: Some("From event payload: task_id, plan_name".to_string()),
            message: "msg".to_string(),
        };
        let line = format_payload_contract_error(&err);
        assert!(line.contains("executor"));
        assert!(line.contains("work.ready"));
        assert!(line.contains("plan_name"));
        assert!(line.contains("coordinator"));
        assert!(line.contains("FieldMissingFromSchema"));
    }

    // ──────────────────────────────────────────────────────────────────────
    // U0 characterization: lock in `validate_preset_topology` and
    // `validate_preset` payload contract behavior so the U1/U2 shared
    // contract layer does not silently change strict semantics.
    // ──────────────────────────────────────────────────────────────────────

    /// U0 characterization: `validate_preset_topology` is independent of
    /// payload contract strict mode. A topology that is reachable must
    /// remain `is_valid() = true` regardless of whether strict mode is
    /// passed downstream.
    #[test]
    fn u0_topology_validity_independent_of_payload_strict_mode() {
        // Linear chain, no payload field references → no payload errors.
        // The topology must validate in both strict and non-strict.
        let yaml = r#"
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  b:
    name: "B"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["work.ready"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_preset_topology(&config, &registry);
        assert!(
            result.is_valid(),
            "valid linear topology must be is_valid()=true: {:?}",
            result
        );
        // Topology result has no payload semantics — its is_valid must not
        // depend on strict flag.
        let strict_replay = validate_preset_topology(&config, &registry);
        assert_eq!(
            strict_replay.is_valid(),
            result.is_valid(),
            "validate_preset_topology must not change is_valid() based on strict mode"
        );
    }

    /// U0 characterization: `validate_preset(strict=true)` must fail on
    /// `SchemaMissingForRequiredTopic`. `validate_preset(strict=false)` must
    /// succeed (the missing schema becomes a warning, not an error). This
    /// pins the two-dimensional strict semantics that U1/U2 must preserve.
    #[test]
    fn u0_validate_preset_strict_split_semantics_for_missing_schema() {
        let yaml = r#"
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  b:
    name: "B"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);

        // non-strict: missing schema is a warning, not an error.
        let non_strict = validate_preset(&config, &registry, false);
        assert!(
            non_strict.is_valid(),
            "non-strict validate_preset must be valid (warning only): {:?}",
            non_strict.payload_contracts
        );
        assert!(
            !non_strict.payload_contracts.warnings.is_empty(),
            "non-strict mode should produce a payload warning, not error"
        );
        assert!(
            non_strict.payload_contracts.errors.is_empty(),
            "non-strict mode must not produce payload errors: {:?}",
            non_strict.payload_contracts.errors
        );

        // strict: missing schema is an error.
        let strict = validate_preset(&config, &registry, true);
        assert!(
            !strict.is_valid(),
            "strict validate_preset must fail on missing schema: {:?}",
            strict
        );
        assert!(
            !strict.payload_contracts.errors.is_empty(),
            "strict mode must produce a payload error"
        );
    }
}
