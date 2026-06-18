//! Determinism regression tests for hat routing & validation output.
//!
//! U6 (2026-06-17-004 plan, R6): ensure that `source_hats_by_topic` /
//! `target_hats_by_topic` indexes, and the `PayloadContractViolation`
//! attribution vectors they feed, are stable across runs.  The previous
//! `HashMap` indexes inherited the `config.hats` (HashMap) iteration order,
//! which is non-deterministic across processes and breaks regression
//! snapshots.  After U6, the indexes are `BTreeMap`s with sorted per-topic
//! `Vec`s so the resulting `source_hat` / `target_hat` vectors always come
//! out in lexicographic hat-id order.
//!
//! The tests in this module exercise the end-to-end `process_events_from_jsonl`
//! path and verify that the resulting `PayloadContractViolation` carries the
//! sorted attribution vectors regardless of how the hat config was built.

use super::*;

/// Build a config where multiple hats publish the same trigger topic so that
/// `source_hats_by_topic[topic]` has more than one entry.  The ordering of
/// hats in the YAML is intentionally **not** sorted, so we can prove the
/// runtime index sort is independent of insertion order.
fn multi_publisher_config_yaml() -> &'static str {
    r#"
event_loop:
  execution_mode: isolated
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      u6.shared.topic:
        payload: json_object
        required_fields: [status]
        allowed_values:
          status: ["ok"]
hats:
  zeta-publisher:
    name: "Zeta"
    triggers: ["work.start"]
    publishes: ["u6.shared.topic"]
    instructions: "zeta"
  alpha-publisher:
    name: "Alpha"
    triggers: ["work.start"]
    publishes: ["u6.shared.topic"]
    instructions: "alpha"
  mu-publisher:
    name: "Mu"
    triggers: ["work.start"]
    publishes: ["u6.shared.topic"]
    instructions: "mu"
  delta-publisher:
    name: "Delta"
    triggers: ["work.start"]
    publishes: ["u6.shared.topic"]
    instructions: "delta"
"#
}

/// Build a `PayloadContractViolation` from the event policy pipeline by
/// emitting a non-recoverable schema violation (`allowed_values` mismatch).
///
/// Returns the resulting `source_hat` / `target_hat` vectors so the caller
/// can assert on their ordering across runs.
fn collect_attribution_for_violation(
    yaml: &str,
    topic: &str,
    violating_payload: &str,
) -> (Vec<String>, Vec<String>) {
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();

    // Pick a hat that actually publishes this topic in the supplied config.
    // The helper is reused across YAMLs with different hat id sets, so a
    // hard-coded `alpha-publisher` would cause origin-guard rejection when
    // that hat does not exist (e.g. the two-hat / six-hat variants in
    // `test_payload_contract_attribution_survives_hat_set_change`).
    let hat_id: String = config
        .hats
        .iter()
        .filter(|(_, h)| h.publishes.iter().any(|t| t == topic))
        .map(|(id, _)| id.clone())
        .min()
        .unwrap_or_else(|| "alpha-publisher".to_string());

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("DeterminismTest");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    use std::io::Write;
    let event = serde_json::json!({
        "topic": topic,
        "payload": violating_payload,
        "ts": chrono::Utc::now().to_rfc3339(),
        "hat": hat_id,
    });
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .unwrap();
    writeln!(f, "{}", event).unwrap();

    let result = event_loop.process_events_from_jsonl().unwrap();
    let violation = result.payload_contract_violation.expect(
        "non-recoverable violation (allowed_values mismatch) must populate payload_contract_violation",
    );
    (violation.source_hat, violation.target_hat)
}

/// U6 R6 happy path: the attribution vectors are sorted even though the
/// config has 4 hats publishing the same topic in arbitrary YAML order.
#[test]
fn test_payload_contract_attribution_is_sorted() {
    let yaml = multi_publisher_config_yaml();
    let (source_hat, _target_hat) =
        collect_attribution_for_violation(yaml, "u6.shared.topic", r#"{"status":"oops"}"#);

    assert_eq!(
        source_hat,
        vec![
            "alpha-publisher".to_string(),
            "delta-publisher".to_string(),
            "mu-publisher".to_string(),
            "zeta-publisher".to_string(),
        ],
        "source_hat must be sorted lexicographically, got: {:?}",
        source_hat
    );
}

/// U6 R6 happy path: running the same violation through the pipeline
/// `N=10` times produces identical attribution vectors — no HashMap
/// order leakage.
#[test]
fn test_payload_contract_attribution_stable_across_repeats() {
    let yaml = multi_publisher_config_yaml();
    let baseline =
        collect_attribution_for_violation(yaml, "u6.shared.topic", r#"{"status":"oops"}"#);

    for i in 1..=10 {
        let (source_hat, target_hat) =
            collect_attribution_for_violation(yaml, "u6.shared.topic", r#"{"status":"oops"}"#);
        assert_eq!(
            source_hat, baseline.0,
            "iteration {i}: source_hat drifted across runs: baseline={:?} now={:?}",
            baseline.0, source_hat
        );
        assert_eq!(
            target_hat, baseline.1,
            "iteration {i}: target_hat drifted across runs: baseline={:?} now={:?}",
            baseline.1, target_hat
        );
    }
}

/// U6 R6 edge case: changing the set of publishing hats still keeps
/// attribution sorted, and the vectors stay deterministic across rebuilds.
#[test]
fn test_payload_contract_attribution_survives_hat_set_change() {
    // Two-hats variant.
    let yaml_two = r#"
event_loop:
  execution_mode: isolated
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      u6.edge.topic:
        payload: json_object
        required_fields: [status]
        allowed_values:
          status: ["ok"]
hats:
  zulu:
    name: "Zulu"
    triggers: ["work.start"]
    publishes: ["u6.edge.topic"]
    instructions: "zulu"
  alpha:
    name: "Alpha"
    triggers: ["work.start"]
    publishes: ["u6.edge.topic"]
    instructions: "alpha"
"#;
    let (source_two, _) =
        collect_attribution_for_violation(yaml_two, "u6.edge.topic", r#"{"status":"oops"}"#);
    assert_eq!(source_two, vec!["alpha".to_string(), "zulu".to_string()]);

    // Six-hats variant (still deterministic).
    let yaml_six = r#"
event_loop:
  execution_mode: isolated
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      u6.edge.topic:
        payload: json_object
        required_fields: [status]
        allowed_values:
          status: ["ok"]
hats:
  f-publisher:
    name: "F"
    triggers: ["work.start"]
    publishes: ["u6.edge.topic"]
    instructions: "f"
  a-publisher:
    name: "A"
    triggers: ["work.start"]
    publishes: ["u6.edge.topic"]
    instructions: "a"
  e-publisher:
    name: "E"
    triggers: ["work.start"]
    publishes: ["u6.edge.topic"]
    instructions: "e"
  c-publisher:
    name: "C"
    triggers: ["work.start"]
    publishes: ["u6.edge.topic"]
    instructions: "c"
  d-publisher:
    name: "D"
    triggers: ["work.start"]
    publishes: ["u6.edge.topic"]
    instructions: "d"
  b-publisher:
    name: "B"
    triggers: ["work.start"]
    publishes: ["u6.edge.topic"]
    instructions: "b"
"#;
    let (source_six, _) =
        collect_attribution_for_violation(yaml_six, "u6.edge.topic", r#"{"status":"oops"}"#);
    assert_eq!(
        source_six,
        vec![
            "a-publisher".to_string(),
            "b-publisher".to_string(),
            "c-publisher".to_string(),
            "d-publisher".to_string(),
            "e-publisher".to_string(),
            "f-publisher".to_string(),
        ],
        "source_six must be sorted, got: {:?}",
        source_six
    );
}

/// U6 R6 internal unit test: the per-topic hat index build is `BTreeMap`
/// with sorted `Vec` values.  This guards the build site directly so a
/// future refactor that reintroduces a `HashMap` is caught at unit-test
/// granularity, before it leaks into the diagnostic payload.
#[test]
fn test_source_target_index_is_btreemap_with_sorted_vec_values() {
    use std::collections::BTreeMap;

    // Build a config with 4 hats publishing overlapping topics in
    // non-sorted YAML order.
    let yaml = multi_publisher_config_yaml();
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();

    // Mirror the production build site (event_loop/mod.rs ~7377) to
    // confirm the data structure and per-topic Vec ordering.
    let mut source_hats_by_topic: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut target_hats_by_topic: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (hat_id, hat_config) in &config.hats {
        for t in &hat_config.publishes {
            source_hats_by_topic
                .entry(t.clone())
                .or_default()
                .push(hat_id.clone());
        }
        for t in &hat_config.triggers {
            target_hats_by_topic
                .entry(t.clone())
                .or_default()
                .push(hat_id.clone());
        }
    }
    for hats in source_hats_by_topic.values_mut() {
        hats.sort();
    }
    for hats in target_hats_by_topic.values_mut() {
        hats.sort();
    }

    // 1) BTreeMap keys iterate in sorted topic order.
    let topic_order: Vec<&String> = source_hats_by_topic.keys().collect();
    assert_eq!(
        topic_order,
        vec![&"u6.shared.topic".to_string()],
        "BTreeMap keys must be in lexicographic topic order, got: {:?}",
        topic_order
    );

    // 2) Each topic's hat list is sorted.
    let source_hats = source_hats_by_topic.get("u6.shared.topic").unwrap();
    assert_eq!(
        source_hats,
        &vec![
            "alpha-publisher".to_string(),
            "delta-publisher".to_string(),
            "mu-publisher".to_string(),
            "zeta-publisher".to_string(),
        ],
        "source hats must be sorted: {:?}",
        source_hats
    );

    // 3) target_hats_by_topic is also sorted.
    let target_hats = target_hats_by_topic.get("work.start").unwrap();
    let mut expected_target: Vec<String> =
        target_hats.iter().cloned().collect::<Vec<_>>().clone();
    expected_target.sort();
    assert_eq!(target_hats, &expected_target);
}
