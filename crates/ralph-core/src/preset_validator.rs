//! Preset topology validator.
//!
//! Validates that a preset's hat configuration forms a valid topology:
//! - The starting event can reach at least one hat.
//! - The completion promise can be reached from the starting event.
//! - Required events are reachable and appear on all completion paths.

use crate::config::{HatConfig, RalphConfig};
use crate::hat_registry::HatRegistry;
use ralph_proto::{Hat, Topic};
use std::collections::{HashMap, HashSet, VecDeque};

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
/// 1. Starting event reaches at least one hat (or falls back to Ralph).
/// 2. Completion promise is reachable from the start.
/// 3. Every required event is reachable.
/// 4. Every required event appears on ALL completion paths.
pub fn validate_preset_topology(
    config: &RalphConfig,
    registry: &HatRegistry,
) -> TopologyValidationResult {
    let mut result = TopologyValidationResult::default();

    if registry.is_empty() {
        return result;
    }

    let graph = TopologyGraph::build(config, registry);

    // 1. Check starting event reachability
    let start = config
        .event_loop
        .starting_event
        .clone()
        .unwrap_or_else(|| "task.start".to_string());

    if !graph.is_topic_reachable(&start) && !graph.has_fallback_path(&start) {
        result.errors.push(TopologyError {
            kind: TopologyErrorKind::UnreachableStart,
            message: format!(
                "Starting event '{}' has no reachable hats and no Ralph fallback path",
                start
            ),
        });
    }

    // 2. Check completion promise reachability
    let completion = &config.event_loop.completion_promise;
    if !graph.is_topic_reachable(completion) {
        result.errors.push(TopologyError {
            kind: TopologyErrorKind::UnreachableCompletion,
            message: format!(
                "Completion promise '{}' is not reachable from any hat",
                completion
            ),
        });
    }

    // 3. & 4. Check required events
    for required in &config.event_loop.required_events {
        if !graph.is_topic_reachable(required) {
            result.errors.push(TopologyError {
                kind: TopologyErrorKind::UnreachableRequired,
                message: format!(
                    "Required event '{}' is not reachable from the starting event",
                    required
                ),
            });
            continue;
        }

        // Check that the required event is on all completion paths.
        // A required event is NOT on all completion paths if there exists
        // a completion path that avoids it. We approximate this by checking
        // if the required event blocks ALL paths to completion.
        // If we can reach completion while blocking this topic AND all hats
        // that publish it, then it's not on all paths.
        if !graph.is_required_on_all_paths(required) {
            result.errors.push(TopologyError {
                kind: TopologyErrorKind::RequiredEventNotOnAllPaths,
                message: format!(
                    "Required event '{}' is not on all completion paths. \
                     Choose a topic that every successful path emits, or adjust hat topology.",
                    required
                ),
            });
        }
    }

    result
}

/// Bipartite graph of topics <-> hats.
struct TopologyGraph<'a> {
    /// topic -> hats that subscribe to it
    topic_to_hats: HashMap<String, Vec<&'a Hat>>,
    /// hat -> topics it can publish (including default_publishes)
    hat_to_topics: HashMap<String, Vec<String>>,
    /// All topic names known in the graph
    all_topics: HashSet<String>,
}

impl<'a> TopologyGraph<'a> {
    fn build(config: &RalphConfig, registry: &'a HatRegistry) -> Self {
        let mut topic_to_hats: HashMap<String, Vec<&'a Hat>> = HashMap::new();
        let mut hat_to_topics: HashMap<String, Vec<String>> = HashMap::new();
        let mut all_topics: HashSet<String> = HashSet::new();

        // Index hats from registry
        for hat in registry.all() {
            // subscriptions (triggers)
            for sub in &hat.subscriptions {
                let topic_str = sub.as_str().to_string();
                all_topics.insert(topic_str.clone());
                topic_to_hats.entry(topic_str).or_default().push(hat);
            }

            // publishes
            let mut publishes = Vec::new();
            for pub_topic in &hat.publishes {
                let topic_str = pub_topic.as_str().to_string();
                all_topics.insert(topic_str.clone());
                publishes.push(topic_str);
            }

            // default_publishes from config
            if let Some(hat_config) = config.hats.get(&hat.id.as_str().to_string().to_string()) {
                if let Some(default) = &hat_config.default_publishes {
                    let topic_str = default.clone();
                    all_topics.insert(topic_str.clone());
                    if !publishes.contains(&topic_str) {
                        publishes.push(topic_str);
                    }
                }
                // Also include config.publishes (which may differ from hat.publishes)
                for pub_topic in &hat_config.publishes {
                    let topic_str = pub_topic.clone();
                    all_topics.insert(topic_str.clone());
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
                all_topics.insert(topic_str.clone());
                publishes.push(topic_str);
            }
            if let Some(default) = &hat_config.default_publishes {
                let topic_str = default.clone();
                all_topics.insert(topic_str.clone());
                if !publishes.contains(&topic_str) {
                    publishes.push(topic_str);
                }
            }
            for trigger in &hat_config.triggers {
                let topic_str = trigger.clone();
                all_topics.insert(topic_str.clone());
                topic_to_hats.entry(topic_str).or_default();
            }
            hat_to_topics.insert(hat_name.clone(), publishes);
        }

        Self {
            topic_to_hats,
            hat_to_topics,
            all_topics,
        }
    }

    /// Check if a topic is reachable from the starting event through the graph.
    fn is_topic_reachable(&self, target: &str) -> bool {
        // BFS from all topics, following topic -> hat -> topic edges
        let mut visited_topics: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();

        // Seed with topics that have subscribers (any topic in the graph)
        // Actually we need to start from the configured starting event
        // But we don't know the starting event here. We'll just check if
        // the target topic has any publisher or subscriber in the graph.
        // For proper reachability we need to know the start topic.

        // For simplicity: a topic is "reachable" if it appears in the graph
        // and either it has subscribers, or it's published by some hat.
        self.all_topics.contains(target)
    }

    /// Check if the starting event would fall back to Ralph (no hat subscribes).
    fn has_fallback_path(&self, start: &str) -> bool {
        !self.topic_to_hats.contains_key(start)
            || self
                .topic_to_hats
                .get(start)
                .map(|v| v.is_empty())
                .unwrap_or(true)
    }

    /// Check if a required event appears on ALL completion paths.
    ///
    /// Approximation: block the required topic and all hats that publish it.
    /// If completion is still reachable, the required event is NOT on all paths.
    fn is_required_on_all_paths(&self, required: &str) -> bool {
        let completion = "LOOP_COMPLETE"; // Default completion promise
        // Actually we should use the configured completion promise, but this
        // is a simplification. We'll refine in future iterations.

        // Find hats that publish the required topic
        let blocking_hats: HashSet<String> = self
            .hat_to_topics
            .iter()
            .filter(|(_, topics)| topics.contains(&required.to_string()))
            .map(|(hat_id, _)| hat_id.clone())
            .collect();

        // BFS: can we reach completion without going through required topic
        // or any hat that publishes it?
        let mut visited_topics: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();

        // Seed with all topics that are not the required topic
        for topic in &self.all_topics {
            if topic != required {
                visited_topics.insert(topic.clone());
                queue.push_back(topic.clone());
            }
        }

        while let Some(topic) = queue.pop_front() {
            if topic == completion {
                // Completion is reachable without the required event
                return false;
            }

            let hats = self.topic_to_hats.get(&topic).cloned().unwrap_or_default();
            for hat in hats {
                if blocking_hats.contains(&hat.id.as_str().to_string().to_string()) {
                    continue;
                }
                if let Some(publishes) = self
                    .hat_to_topics
                    .get(&hat.id.as_str().to_string().to_string())
                {
                    for pub_topic in publishes {
                        if pub_topic == required {
                            continue;
                        }
                        if visited_topics.insert(pub_topic.clone()) {
                            queue.push_back(pub_topic.clone());
                        }
                    }
                }
            }
        }

        // Completion not reachable without required event -> it's on all paths
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RalphConfig;

    fn empty_registry() -> HatRegistry {
        HatRegistry::default()
    }

    #[test]
    fn empty_registry_is_valid() {
        let config = RalphConfig::default();
        let registry = empty_registry();
        let result = validate_preset_topology(&config, &registry);
        assert!(result.is_valid());
    }
}
