//! Hat registry for managing agent personas.

use crate::config::{HatConfig, Phase, RalphConfig};
use ralph_proto::{Hat, HatId, Topic};
use std::collections::{BTreeMap, HashSet};

/// Registry for managing and creating hats from configuration.
#[derive(Debug)]
pub struct HatRegistry {
    hats: BTreeMap<HatId, Hat>,
    configs: BTreeMap<HatId, HatConfig>,
    /// Prefix index for O(1) early-exit on no-match lookups.
    /// Contains all first segments of subscription patterns (e.g., "task" from "task.*").
    /// Also contains "*" if any global wildcard exists.
    prefix_index: HashSet<String>,
    /// Current orchestration phase for phase-aware trigger routing.
    current_phase: Phase,
}

impl Default for HatRegistry {
    fn default() -> Self {
        Self {
            hats: BTreeMap::new(),
            configs: BTreeMap::new(),
            prefix_index: HashSet::new(),
            current_phase: Phase::default(),
        }
    }
}

impl HatRegistry {
    /// Creates a new empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a registry from configuration.
    ///
    /// Empty config → empty registry (HatlessRalph is the fallback, not default hats).
    pub fn from_config(config: &RalphConfig) -> Self {
        let mut registry = Self::new();

        for (id, hat_config) in &config.hats {
            let hat = Self::hat_from_config(id, hat_config);
            registry.register_with_config(hat, hat_config.clone());
        }

        registry
    }

    /// Creates a Hat from HatConfig.
    fn hat_from_config(id: &str, config: &HatConfig) -> Hat {
        let mut hat = Hat::new(id, &config.name);
        hat.description = config.description.clone().unwrap_or_default();
        // Use all_trigger_topics to index all phase triggers for registration
        hat.subscriptions = config.all_trigger_topics();
        hat.publishes = config.publish_topics();
        hat.instructions = config.instructions.clone();
        hat
    }

    /// Registers a hat with the registry.
    pub fn register(&mut self, hat: Hat) {
        self.index_hat_subscriptions(&hat);
        self.hats.insert(hat.id.clone(), hat);
    }

    /// Registers a hat with its configuration.
    pub fn register_with_config(&mut self, hat: Hat, config: HatConfig) {
        let id = hat.id.clone();
        self.index_hat_subscriptions(&hat);
        self.hats.insert(id.clone(), hat);
        self.configs.insert(id, config);
    }

    /// Sets the current orchestration phase for phase-aware routing.
    pub fn set_phase(&mut self, phase: Phase) {
        self.current_phase = phase;
    }

    /// Returns the current orchestration phase.
    pub fn current_phase(&self) -> &Phase {
        &self.current_phase
    }

    /// Indexes a hat's subscriptions for O(1) prefix lookup.
    fn index_hat_subscriptions(&mut self, hat: &Hat) {
        for sub in &hat.subscriptions {
            let pattern = sub.as_str();
            // Global wildcard matches everything - mark it specially
            if pattern == "*" {
                self.prefix_index.insert("*".to_string());
            } else {
                // Extract first segment (e.g., "task" from "task.*" or "task.start")
                if let Some(prefix) = pattern.split('.').next() {
                    self.prefix_index.insert(prefix.to_string());
                }
            }
        }
    }

    /// Gets a hat by ID.
    pub fn get(&self, id: &HatId) -> Option<&Hat> {
        self.hats.get(id)
    }

    /// Gets a hat's configuration by ID.
    pub fn get_config(&self, id: &HatId) -> Option<&HatConfig> {
        self.configs.get(id)
    }

    /// Returns all hats in the registry.
    pub fn all(&self) -> impl Iterator<Item = &Hat> {
        self.hats.values()
    }

    /// Returns all hat IDs.
    pub fn ids(&self) -> impl Iterator<Item = &HatId> {
        self.hats.keys()
    }

    /// Returns the number of registered hats.
    pub fn len(&self) -> usize {
        self.hats.len()
    }

    /// Returns true if no hats are registered.
    pub fn is_empty(&self) -> bool {
        self.hats.is_empty()
    }

    /// Finds all hats subscribed to a topic in the current phase.
    /// BTreeMap iteration is already sorted by key.
    /// Uses phase-aware matching when hats have phase_triggers configured.
    pub fn subscribers(&self, topic: &Topic) -> Vec<&Hat> {
        self.hats
            .values()
            .filter(|hat| self.hat_is_subscribed_in_phase(&hat.id, topic.as_str()))
            .collect()
    }

    /// Finds the first hat that would be triggered by a topic.
    /// Returns the hat ID if found, used for event logging.
    /// BTreeMap iteration is already sorted by key.
    pub fn find_by_trigger(&self, topic: &str) -> Option<&HatId> {
        let topic = Topic::new(topic);
        self.hats
            .values()
            .find(|hat| hat.is_subscribed(&topic))
            .map(|hat| &hat.id)
    }

    /// Returns true if any hat is subscribed to the given topic.
    pub fn has_subscriber(&self, topic: &str) -> bool {
        let topic = Topic::new(topic);
        self.hats.values().any(|hat| hat.is_subscribed(&topic))
    }

    /// Check if a hat is allowed to publish the given topic.
    ///
    /// Returns `true` only for registered hats whose `publishes` list includes
    /// the topic (or a matching wildcard pattern). Unknown/unregistered hats
    /// are rejected — they cannot publish arbitrary topics.
    ///
    /// Uses the same pattern matching as subscription routing.
    pub fn can_publish(&self, hat_id: &HatId, topic: &str) -> bool {
        let Some(hat) = self.hats.get(hat_id) else {
            return false; // Unknown hat — fail closed
        };
        hat.publishes
            .iter()
            .any(|pub_topic| pub_topic.matches_str(topic))
    }

    /// Returns true if a hat is subscribed to the given topic in the current phase.
    /// When the hat has phase_triggers, uses phase-aware matching; otherwise uses
    /// the hat's static subscription list.
    pub fn hat_is_subscribed_in_phase(&self, hat_id: &HatId, topic: &str) -> bool {
        let Some(config) = self.configs.get(hat_id) else {
            // No config means no phase_triggers, use hat's subscriptions directly
            if let Some(hat) = self.hats.get(hat_id) {
                return hat.is_subscribed_str(topic);
            }
            return false;
        };

        // If hat has phase_triggers, use them for the current phase
        if let Some(ref phase_triggers) = config.phase_triggers {
            let phase_name = match self.current_phase {
                Phase::Warmup => "warmup",
                Phase::Production => "production",
            };
            if let Some(triggers) = phase_triggers.get(phase_name) {
                let topic_obj = Topic::new(topic);
                return triggers.iter().any(|t| {
                    let trigger_topic = Topic::new(t);
                    trigger_topic.matches(&topic_obj)
                });
            }
            // Phase not found in phase_triggers, fall back to static subscriptions
        }

        // No phase_triggers or phase not configured, use hat's subscriptions
        if let Some(hat) = self.hats.get(hat_id) {
            hat.is_subscribed_str(topic)
        } else {
            false
        }
    }

    /// Returns the first hat subscribed to the given topic.
    ///
    /// Uses prefix index for O(1) early-exit when the topic prefix doesn't match
    /// any subscription pattern. Respects phase-aware triggers when configured.
    pub fn get_for_topic(&self, topic: &str) -> Option<&Hat> {
        // Fast path: Check if any subscription could possibly match this topic
        // If we have a global wildcard "*", we must do the full scan
        if !self.prefix_index.contains("*") {
            // Extract prefix from topic (e.g., "task" from "task.start")
            let topic_prefix = topic.split('.').next().unwrap_or(topic);
            if !self.prefix_index.contains(topic_prefix) {
                // No subscription has this prefix - early exit
                return None;
            }
        }

        // Fall back to full linear scan with phase-aware matching
        self.hats
            .values()
            .find(|hat| self.hat_is_subscribed_in_phase(&hat.id, topic))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_empty_config_creates_empty_registry() {
        let config = RalphConfig::default();
        let registry = HatRegistry::from_config(&config);

        // Empty config → empty registry (HatlessRalph is the fallback, not default hats)
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_custom_hats_from_config() {
        let yaml = r#"
hats:
  implementer:
    name: "Implementer"
    triggers: ["task.*"]
  reviewer:
    name: "Reviewer"
    triggers: ["impl.*"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = HatRegistry::from_config(&config);

        assert_eq!(registry.len(), 2);

        let impl_hat = registry.get(&HatId::new("implementer")).unwrap();
        assert!(impl_hat.is_subscribed(&Topic::new("task.start")));
        assert!(!impl_hat.is_subscribed(&Topic::new("impl.done")));

        let review_hat = registry.get(&HatId::new("reviewer")).unwrap();
        assert!(review_hat.is_subscribed(&Topic::new("impl.done")));
    }

    #[test]
    fn test_has_subscriber() {
        let yaml = r#"
hats:
  impl:
    name: "Implementer"
    triggers: ["task.*"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = HatRegistry::from_config(&config);

        assert!(registry.has_subscriber("task.start"));
        assert!(!registry.has_subscriber("build.task"));
    }

    #[test]
    fn test_get_for_topic() {
        let yaml = r#"
hats:
  impl:
    name: "Implementer"
    triggers: ["task.*"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = HatRegistry::from_config(&config);

        let hat = registry.get_for_topic("task.start");
        assert!(hat.is_some());
        assert_eq!(hat.unwrap().id.as_str(), "impl");

        let no_hat = registry.get_for_topic("build.task");
        assert!(no_hat.is_none());
    }

    #[test]
    fn test_empty_registry_has_no_subscribers() {
        let config = RalphConfig::default();
        let registry = HatRegistry::from_config(&config);

        // Empty config → no subscribers (HatlessRalph handles orphaned events)
        assert!(!registry.has_subscriber("build.task"));
        assert!(registry.get_for_topic("build.task").is_none());
    }

    #[test]
    fn test_find_subscribers() {
        let yaml = r#"
hats:
  impl:
    name: "Implementer"
    triggers: ["task.*", "review.done"]
  reviewer:
    name: "Reviewer"
    triggers: ["impl.*"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = HatRegistry::from_config(&config);

        let task_subs = registry.subscribers(&Topic::new("task.start"));
        assert_eq!(task_subs.len(), 1);
        assert_eq!(task_subs[0].id.as_str(), "impl");

        let impl_subs = registry.subscribers(&Topic::new("impl.done"));
        assert_eq!(impl_subs.len(), 1);
        assert_eq!(impl_subs[0].id.as_str(), "reviewer");
    }

    /// Benchmark test for get_for_topic() performance.
    /// Run with: cargo test -p ralph-core bench_get_for_topic -- --nocapture
    #[test]
    fn bench_get_for_topic_baseline() {
        // Create registry with 20 hats (realistic production scenario)
        let mut yaml = String::from("hats:\n");
        for i in 0..20 {
            yaml.push_str(&format!(
                "  hat{}:\n    name: \"Hat {}\"\n    triggers: [\"topic{}.*\", \"other{}.event\"]\n",
                i, i, i, i
            ));
        }
        let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
        let registry = HatRegistry::from_config(&config);

        // Topics to test - mix of matches and non-matches
        let topics = [
            "topic0.start",  // First hat match
            "topic10.event", // Middle hat match
            "topic19.done",  // Last hat match
            "nomatch.topic", // No match
        ];

        const ITERATIONS: u32 = 100_000;

        let start = Instant::now();
        for _ in 0..ITERATIONS {
            for topic in &topics {
                let _ = registry.get_for_topic(topic);
            }
        }
        let elapsed = start.elapsed();

        let ops = u64::from(ITERATIONS) * (topics.len() as u64);
        let ns_per_op = elapsed.as_nanos() / u128::from(ops);

        println!("\n=== get_for_topic() Baseline ===");
        println!("Registry size: {} hats", registry.len());
        println!("Operations: {}", ops);
        println!("Total time: {:?}", elapsed);
        println!("Time per operation: {} ns", ns_per_op);
        println!("================================\n");

        // Assert reasonable performance (sanity check)
        assert!(
            ns_per_op < 10_000,
            "Performance degraded: {} ns/op",
            ns_per_op
        );
    }

    #[test]
    fn test_get_for_topic_returns_alphabetically_first_hat() {
        // Two hats subscribing to same wildcard pattern → get_for_topic returns alphabetically first
        let yaml = r#"
hats:
  zebra:
    name: "Zebra"
    triggers: ["task.*"]
  alpha:
    name: "Alpha"
    triggers: ["task.*"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = HatRegistry::from_config(&config);

        // Should deterministically return "alpha" (alphabetically first)
        let hat = registry.get_for_topic("task.start");
        assert!(hat.is_some());
        assert_eq!(
            hat.unwrap().id.as_str(),
            "alpha",
            "get_for_topic should return alphabetically first matching hat"
        );

        // Run multiple times to confirm determinism
        for _ in 0..100 {
            let hat = registry.get_for_topic("task.start").unwrap();
            assert_eq!(hat.id.as_str(), "alpha");
        }
    }

    #[test]
    fn test_find_by_trigger_returns_alphabetically_first_hat() {
        let yaml = r#"
hats:
  zebra:
    name: "Zebra"
    triggers: ["task.*"]
  alpha:
    name: "Alpha"
    triggers: ["task.*"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = HatRegistry::from_config(&config);

        let hat_id = registry.find_by_trigger("task.start");
        assert!(hat_id.is_some());
        assert_eq!(
            hat_id.unwrap().as_str(),
            "alpha",
            "find_by_trigger should return alphabetically first matching hat"
        );
    }

    #[test]
    fn test_subscribers_returns_deterministic_order() {
        let yaml = r#"
hats:
  zebra:
    name: "Zebra"
    triggers: ["task.*"]
  middle:
    name: "Middle"
    triggers: ["task.*"]
  alpha:
    name: "Alpha"
    triggers: ["task.*"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = HatRegistry::from_config(&config);

        let subs = registry.subscribers(&Topic::new("task.start"));
        assert_eq!(subs.len(), 3);
        assert_eq!(subs[0].id.as_str(), "alpha");
        assert_eq!(subs[1].id.as_str(), "middle");
        assert_eq!(subs[2].id.as_str(), "zebra");
    }

    #[test]
    fn test_can_publish_allows_declared_topic() {
        let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done", "build.blocked"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = HatRegistry::from_config(&config);

        assert!(registry.can_publish(&HatId::new("builder"), "build.done"));
        assert!(registry.can_publish(&HatId::new("builder"), "build.blocked"));
    }

    #[test]
    fn test_can_publish_rejects_undeclared_topic() {
        let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = HatRegistry::from_config(&config);

        assert!(!registry.can_publish(&HatId::new("builder"), "LOOP_COMPLETE"));
        assert!(!registry.can_publish(&HatId::new("builder"), "plan.approved"));
    }

    #[test]
    fn test_can_publish_allows_wildcard() {
        let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.*"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = HatRegistry::from_config(&config);

        assert!(registry.can_publish(&HatId::new("builder"), "build.done"));
        assert!(registry.can_publish(&HatId::new("builder"), "build.blocked"));
        assert!(!registry.can_publish(&HatId::new("builder"), "LOOP_COMPLETE"));
    }

    #[test]
    fn test_can_publish_unknown_hat_rejects() {
        let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = HatRegistry::from_config(&config);

        // Unknown/unregistered hats should be rejected — fail closed
        assert!(!registry.can_publish(&HatId::new("ralph"), "anything"));
        assert!(!registry.can_publish(&HatId::new("ralph"), "LOOP_COMPLETE"));
        assert!(!registry.can_publish(&HatId::new("strategist"), "experiment.planned"));
    }
}
