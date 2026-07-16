//! Plan 001 §4.5 (2026-06-15-001): Schema-coverage checks for `preset_lint`.
//!
//! `check_publishes_have_schema` verifies that every topic a hat declares
//! in `publishes` has an entry in `event_policy.schemas`. Without this
//! guard the CLI pre-publish check would have nothing to validate
//! against and a payload contract for that topic would be undefined.
//!
//! `check_schema_reference_parity` is the runtime counterpart of the
//! `presets.rs` byte-equality tests: it surfaces drift between the
//! inline `event_policy.schemas` block and a sibling `presets/schemas/`
//! reference file. This is exposed for `ralph preset check --strict`
//! to flag drift in CI without depending on the Rust test binary.

use crate::config::RalphConfig;
use crate::preset_lint::finding_id::{
    FINDING_PUBLISHES_MISSING_SCHEMA, FINDING_SCHEMA_REFERENCE_PARITY,
};
use crate::preset_lint::{LintFinding, LintSeverity, LintStrictness};

/// Plan 001 §4.5 R1: every hat `publishes` topic must have a schema.
///
/// Findings are emitted per (hat, topic) pair. Severity matches the
/// project's lint strictness convention: `Warn` by default, `Error`
/// in strict mode so the gate refuses to start a loop on an under-
/// specified topic surface.
pub fn check_publishes_have_schema(
    config: &RalphConfig,
    strictness: LintStrictness,
) -> Vec<LintFinding> {
    let mut findings = Vec::new();

    let schemas = match config.event_loop.event_policy.as_ref() {
        Some(policy) if policy.enabled => &policy.schemas,
        _ => return findings,
    };

    for (hat_id, hat_config) in &config.hats {
        for topic in &hat_config.publishes {
            if !schemas.contains_key(topic) {
                findings.push(schema_missing_finding(hat_id, topic, strictness));
            }
        }
    }

    findings
}

fn schema_missing_finding(hat_id: &str, topic: &str, strictness: LintStrictness) -> LintFinding {
    let message = format!(
        "hat \"{hat_id}\" publishes topic \"{topic}\" but \
         event_policy.schemas has no entry for it"
    );
    let severity = match strictness {
        LintStrictness::Strict => LintSeverity::Error,
        LintStrictness::Default => LintSeverity::Warn,
    };
    let finding = LintFinding {
        id: FINDING_PUBLISHES_MISSING_SCHEMA,
        severity,
        message,
        topic: None,
        hat: None,
        owner: None,
        action_hint: None,
    };
    finding
        .with_hat(hat_id)
        .with_topic(topic)
        .with_action_hint(format!(
            "Add a `{topic}` block under event_policy.schemas in the preset, \
             or remove the topic from hat \"{hat_id}\".publishes"
        ))
}

/// Plan 001 §4.5 R2: surface drift between inline schemas and the
/// sibling `presets/schemas/<name>.yml` reference file.
///
/// This is a lightweight runtime check: the byte-equality tests in
/// `presets.rs` remain the authoritative CI gate. The lint is here so
/// `ralph preset check --strict` (which doesn't compile-test) can also
/// flag drift.
pub fn check_schema_reference_parity(
    config: &RalphConfig,
    preset_name: &str,
    reference_yaml: Option<&str>,
) -> Vec<LintFinding> {
    let mut findings = Vec::new();

    let Some(reference_yaml) = reference_yaml else {
        return findings;
    };

    let inline = match &config.event_loop.event_policy {
        Some(p) if p.enabled => &p.schemas,
        _ => return findings,
    };

    let inline_value = match serde_yaml::to_value(inline) {
        Ok(v) => v,
        Err(e) => {
            findings.push(
                LintFinding::error(
                    FINDING_SCHEMA_REFERENCE_PARITY,
                    format!(
                        "Could not serialise inline event_policy.schemas for parity check: {e}"
                    ),
                )
                .with_action_hint("Inspect the preset YAML for malformed schema fields"),
            );
            return findings;
        }
    };

    let reference_value: serde_yaml::Value = match serde_yaml::from_str(reference_yaml) {
        Ok(v) => v,
        Err(e) => {
            findings.push(
                LintFinding::error(
                    FINDING_SCHEMA_REFERENCE_PARITY,
                    format!("Could not parse presets/schemas/{preset_name}.yml: {e}"),
                )
                .with_action_hint(format!(
                    "Run `git diff presets/schemas/{preset_name}.yml` to inspect"
                )),
            );
            return findings;
        }
    };

    // Round-trip both sides through serde_yaml::Value to get a canonical
    // representation. This handles inline-vs-block sequences and missing
    // nulls that serde_yaml treats differently from the original struct.
    // HashMap-backed payloads (e.g. `required_fields`) serialise in
    // non-deterministic order — sort by re-parsing to a `Vec` of strings.
    let inline_canonical = canonical_yaml(&inline_value);
    let reference_canonical = canonical_yaml(&reference_value);

    if inline_canonical != reference_canonical {
        findings.push(
            LintFinding::error(
                FINDING_SCHEMA_REFERENCE_PARITY,
                format!("Inline event_policy.schemas differs from presets/schemas/{preset_name}"),
            )
            // Plan 2026-06-16-002 Unit 1: with the build.rs SSOT
            // merge, drift between the inline block and the SSOT file
            // is the **authoring-time** signal that the operator
            // needs to either (a) align the two, or (b) rebuild so
            // the merged preset picks up the SSOT changes. Rebuilding
            // without aligning will not fix the drift because the
            // inline override still wins per-key — but it will at
            // least surface the divergence here.
            .with_action_hint(format!(
                "Rebuild to apply SSOT (`cargo build -p ralph-cli`), then either align \
                 presets/schemas/{preset_name} with the inline block, or remove the inline \
                 entries that override SSOT defaults. Rerun `ralph preset check` to confirm."
            )),
        );
    }

    findings
}

/// Re-serialise a `serde_yaml::Value` with deterministic key ordering so
/// that two semantically equal payloads (one parsed from inline
/// `[a, b]`, another from a block sequence) compare equal.
///
/// Empty mappings (e.g. `{}`) are stripped before comparison because
/// `serde_yaml::Value` round-trips default-initialised `HashMap`s as
/// `{}`, while the reference YAML simply omits the field. Semantically
/// both mean "no constraints", so we treat them as equal.
fn canonical_yaml(value: &serde_yaml::Value) -> String {
    let sorted = sort_yaml(value);
    let pruned = prune_empty_mappings(&sorted);
    serde_yaml::to_string(&pruned).unwrap_or_default()
}

fn sort_yaml(value: &serde_yaml::Value) -> serde_yaml::Value {
    match value {
        serde_yaml::Value::Mapping(m) => {
            let mut entries: Vec<(serde_yaml::Value, serde_yaml::Value)> = m
                .iter()
                .map(|(k, v)| (sort_yaml(k), sort_yaml(v)))
                .collect();
            entries.sort_by(|a, b| yaml_key_cmp(&a.0, &b.0));
            let mut out = serde_yaml::Mapping::new();
            for (k, v) in entries {
                out.insert(k, v);
            }
            serde_yaml::Value::Mapping(out)
        }
        serde_yaml::Value::Sequence(s) => {
            serde_yaml::Value::Sequence(s.iter().map(sort_yaml).collect())
        }
        other => other.clone(),
    }
}

fn prune_empty_mappings(value: &serde_yaml::Value) -> serde_yaml::Value {
    match value {
        serde_yaml::Value::Mapping(m) => {
            let mut out = serde_yaml::Mapping::new();
            for (k, v) in m {
                let pv = prune_empty_mappings(v);
                if is_empty_mapping(&pv) || is_empty_sequence(&pv) {
                    continue;
                }
                out.insert(k.clone(), pv);
            }
            serde_yaml::Value::Mapping(out)
        }
        serde_yaml::Value::Sequence(s) => {
            serde_yaml::Value::Sequence(s.iter().map(prune_empty_mappings).collect())
        }
        other => other.clone(),
    }
}

fn is_empty_mapping(value: &serde_yaml::Value) -> bool {
    matches!(value, serde_yaml::Value::Mapping(m) if m.is_empty())
}

fn is_empty_sequence(value: &serde_yaml::Value) -> bool {
    matches!(value, serde_yaml::Value::Sequence(s) if s.is_empty())
}

fn yaml_key_cmp(a: &serde_yaml::Value, b: &serde_yaml::Value) -> std::cmp::Ordering {
    match (a, b) {
        (serde_yaml::Value::String(x), serde_yaml::Value::String(y)) => x.cmp(y),
        _ => a
            .as_str()
            .map(|s| s.to_string())
            .cmp(&b.as_str().map(|s| s.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EventPolicyConfig, EventSchema, HatConfig, PayloadType};
    use std::collections::HashMap;

    fn policy_with_schemas() -> EventPolicyConfig {
        let mut schemas = HashMap::new();
        schemas.insert(
            "work.ready".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec!["plan_name".to_string()],
                allowed_values: HashMap::new(),
                hat_allowed_values: HashMap::new(),
                ..Default::default()
            },
        );
        EventPolicyConfig {
            enabled: true,
            schemas,
            ..EventPolicyConfig::default()
        }
    }

    fn hat_publishing(hat_id: &str, topic: &str) -> HatConfig {
        HatConfig {
            name: hat_id.to_string(),
            description: None,
            triggers: vec![],
            publishes: vec![topic.to_string()],
            ..HatConfig::default()
        }
    }

    #[test]
    fn publishes_with_schema_emits_no_finding() {
        let mut config = RalphConfig::default();
        config.event_loop.event_policy = Some(policy_with_schemas());
        config.hats.insert(
            "coordinator".to_string(),
            hat_publishing("coordinator", "work.ready"),
        );
        let findings = check_publishes_have_schema(&config, LintStrictness::Default);
        assert!(
            findings.is_empty(),
            "publishes with schema must not be flagged: {findings:?}"
        );
    }

    #[test]
    fn publishes_without_schema_emits_finding_with_default_severity() {
        let mut config = RalphConfig::default();
        config.event_loop.event_policy = Some(policy_with_schemas());
        config.hats.insert(
            "orphan".to_string(),
            hat_publishing("orphan", "work.unknown"),
        );
        let findings = check_publishes_have_schema(&config, LintStrictness::Default);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.id, FINDING_PUBLISHES_MISSING_SCHEMA);
        assert_eq!(f.severity, LintSeverity::Warn);
        assert!(f.message.contains("work.unknown"));
    }

    #[test]
    fn publishes_without_schema_is_error_in_strict_mode() {
        let mut config = RalphConfig::default();
        config.event_loop.event_policy = Some(policy_with_schemas());
        config.hats.insert(
            "orphan".to_string(),
            hat_publishing("orphan", "work.unknown"),
        );
        let findings = check_publishes_have_schema(&config, LintStrictness::Strict);
        assert_eq!(findings[0].severity, LintSeverity::Error);
    }

    #[test]
    fn no_policy_means_no_findings() {
        let config = RalphConfig::default();
        let findings = check_publishes_have_schema(&config, LintStrictness::Default);
        assert!(findings.is_empty());
    }

    #[test]
    fn parity_check_flags_divergence() {
        let mut config = RalphConfig::default();
        config.event_loop.event_policy = Some(policy_with_schemas());
        let reference = "work.ready:\n  required_fields: [other]\n  payload: json_object\n";
        let findings = check_schema_reference_parity(&config, "test-preset.yml", Some(reference));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, FINDING_SCHEMA_REFERENCE_PARITY);
    }

    #[test]
    fn parity_check_passes_on_match() {
        let mut config = RalphConfig::default();
        config.event_loop.event_policy = Some(policy_with_schemas());
        let reference = "work.ready:\n  required_fields:\n  - plan_name\n  payload: json_object\n";
        let findings = check_schema_reference_parity(&config, "test-preset.yml", Some(reference));
        assert!(
            findings.is_empty(),
            "canonicalised match should produce no findings: {findings:?}"
        );
    }

    #[test]
    fn parity_check_returns_empty_without_reference() {
        let mut config = RalphConfig::default();
        config.event_loop.event_policy = Some(policy_with_schemas());
        let findings = check_schema_reference_parity(&config, "test-preset.yml", None);
        assert!(findings.is_empty());
    }

    #[test]
    fn parity_check_treats_empty_sequence_as_absent() {
        // Plan 001 P1-4: `required_fields: []` inline vs absent in
        // reference must canonicalise to the same value. Without the
        // empty-sequence prune, the parity check would falsely flag a
        // real divergence.
        let mut config = RalphConfig::default();
        let mut schemas = HashMap::new();
        schemas.insert(
            "work.ready".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec![], // empty list — canonicalised to absent
                allowed_values: HashMap::new(),
                hat_allowed_values: HashMap::new(),
                ..Default::default()
            },
        );
        config.event_loop.event_policy = Some(EventPolicyConfig {
            enabled: true,
            schemas,
            ..EventPolicyConfig::default()
        });
        let reference = "work.ready:\n  payload: json_object\n";
        let findings = check_schema_reference_parity(&config, "test-preset.yml", Some(reference));
        assert!(
            findings.is_empty(),
            "inline required_fields: [] should canonicalise as absent: {findings:?}"
        );
    }

    #[test]
    fn desugared_precheck_gate_publishes_have_schemas() {
        use crate::config::RalphConfig;
        let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
  precheck:
    enabled: true
    rules:
      work.done:
        prompt: ["ok"]
        on_fail:
          target: executor
hats:
  executor:
    name: "Executor"
    triggers: ["task.start"]
    publishes: ["work.done"]
"#;
        let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        config.normalize();
        let findings = check_publishes_have_schema(&config, LintStrictness::Strict);
        assert!(
            findings.is_empty(),
            "desugared precheck topics must have schemas injected by normalize(): {findings:?}"
        );
        let schemas = &config.event_loop.event_policy.as_ref().unwrap().schemas;
        assert!(schemas.contains_key("work.done.proposed"));
        assert!(schemas.contains_key("work.done.rejected"));
    }
}
