//! Payload Contract — Static field-reference extraction from hat instructions.
//!
//! This module provides conservative extraction of payload field dependencies
//! from hat instructions text. It is used by the static payload contract
//! validator (U3) to check whether upstream topics provide the fields
//! required by downstream hats.
//!
//! Extraction rules (conservative — only explicit payload references):
//! - `From event payload: field1, field2` — extracts field1, field2
//! - `payload MUST include: field1, field2` — extracts field1, field2
//! - Lines containing `payload` (case-insensitive) with backtick-quoted field names
//!
//! Does NOT affect runtime event policy enforcement.

use crate::config::RalphConfig;
use crate::hat_registry::HatRegistry;
use ralph_proto::HatId;
use std::collections::{BTreeSet, HashMap};
use regex::Regex;
use serde::{Deserialize, Serialize};

/// A single extracted payload field reference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PayloadFieldRef {
    /// Hat ID this field was extracted from.
    pub hat_id: String,
    /// Field name (e.g., `task_id`, `plan_name`).
    pub field: String,
    /// Line number in the instructions (1-indexed).
    pub line: usize,
    /// The extraction pattern that matched.
    pub pattern: String,
    /// The source line excerpt.
    pub source_excerpt: String,
}

/// Extraction pattern identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionPattern {
    /// `From event payload: ...` pattern.
    FromEventPayload,
    /// `payload MUST include: ...` pattern.
    PayloadMustInclude,
    /// Backtick-quoted field on a line containing `payload`.
    BacktickField,
}

/// Extracts payload field references from hat instructions.
///
/// Returns a sorted, deduplicated list of field references.
/// Only extracts from lines that explicitly reference payload fields.
/// Does not follow references or do deep analysis.
///
/// Deduplication is based on (hat_id, field) — only the first occurrence
/// is kept, with its original line number and pattern preserved.
pub fn extract_payload_field_refs(
    hat_id: &str,
    instructions: &str,
    ignore_fields: &[String],
) -> Vec<PayloadFieldRef> {
    let from_payload_regex = Regex::new(r"(?i)From\s+event\s+payload\s*:\s*").unwrap();
    let must_include_regex = Regex::new(r"(?i)payload\s+MUST\s+include\s*:\s*").unwrap();
    let backtick_field_regex = Regex::new(r"`([^`]+)`").unwrap();
    let payload_line_regex = Regex::new(r"(?i).*payload.*").unwrap();

    let ignore_set: BTreeSet<String> = ignore_fields.iter().cloned().collect();
    // Track seen (hat_id, field) pairs for deduplication
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut refs: Vec<PayloadFieldRef> = Vec::new();

    for (line_idx, line) in instructions.lines().enumerate() {
        let line_number = line_idx + 1;

        // Pattern 1: "From event payload: field1, field2"
        if let Some(caps) = from_payload_regex.captures(line) {
            let after = caps.get(0).map(|m| m.end()).unwrap_or(0);
            let content = &line[after..];
            for field in extract_comma_separated_fields(content) {
                if !ignore_set.contains(&field) && seen.insert((hat_id.to_string(), field.clone())) {
                    refs.push(PayloadFieldRef {
                        hat_id: hat_id.to_string(),
                        field,
                        line: line_number,
                        pattern: "From event payload".to_string(),
                        source_excerpt: line.trim().to_string(),
                    });
                }
            }
        }

        // Pattern 2: "payload MUST include: field1, field2"
        if let Some(caps) = must_include_regex.captures(line) {
            let after = caps.get(0).map(|m| m.end()).unwrap_or(0);
            let content = &line[after..];
            for field in extract_comma_separated_fields(content) {
                if !ignore_set.contains(&field) && seen.insert((hat_id.to_string(), field.clone())) {
                    refs.push(PayloadFieldRef {
                        hat_id: hat_id.to_string(),
                        field,
                        line: line_number,
                        pattern: "payload MUST include".to_string(),
                        source_excerpt: line.trim().to_string(),
                    });
                }
            }
        }

        // Pattern 3: Backtick-quoted fields on lines that explicitly mention
        // payload with intent. We require either:
        //   - a "From event payload:" prefix (same trigger as Pattern 1)
        //   - a "payload MUST include:" prefix (same trigger as Pattern 2)
        //   - the bare phrase "event payload" (e.g., "Read from event
        //     payload: task_id")
        // Other lines that just happen to contain the word "payload" (e.g.,
        // "the payload MUST include" elsewhere, or "the payload's
        // reviewer") do NOT count — this avoids false positives from
        // backticked topic names like `work.done` or file paths like
        // `fix-log.md` that appear on those lines.
        let backtick_intent_regex =
            Regex::new(r"(?i)from\s+event\s+payload|event\s+payload\s*:|payload\s+MUST\s+include")
                .unwrap();
        if backtick_intent_regex.is_match(line) {
            for caps in backtick_field_regex.captures_iter(line) {
                if let Some(field_match) = caps.get(1) {
                    let raw = field_match.as_str().trim();
                    let is_identifier = !raw.is_empty()
                        && raw.chars().all(|c| {
                            c.is_ascii_alphanumeric() || c == '_'
                        })
                        && raw.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
                    if is_identifier
                        && !ignore_set.contains(raw)
                        && seen.insert((hat_id.to_string(), raw.to_string()))
                    {
                        refs.push(PayloadFieldRef {
                            hat_id: hat_id.to_string(),
                            field: raw.to_string(),
                            line: line_number,
                            pattern: "backtick-field".to_string(),
                            source_excerpt: line.trim().to_string(),
                        });
                    }
                }
            }
        }
    }

    // Sort by hat_id, then field for stable ordering
    refs.sort_by(|a, b| a.hat_id.cmp(&b.hat_id).then_with(|| a.field.cmp(&b.field)));

    refs
}

/// Extract comma-separated field names, splitting on commas and cleaning whitespace.
fn extract_comma_separated_fields(content: &str) -> Vec<String> {
    content
        .split(',')
        .map(|s| s.trim().to_string())
        .map(|s| s.trim_matches('`').to_string()) // strip surrounding backticks
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .filter(|s| !s.starts_with('#')) // Skip comments
        .filter(|s| !s.starts_with("```")) // Skip code blocks
        .filter(|s| {
            // Conservative: only bare identifiers.
            s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && s.chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        })
        .collect()
}

// ──────────────────────────────────────────────────────────────────────────
// Payload Contract Validator (U3)
// ──────────────────────────────────────────────────────────────────────────

/// Kind of payload contract error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayloadContractErrorKind {
    /// Hat instructions reference a payload field that the trigger topic's
    /// schema does not declare as a required field.
    FieldMissingFromSchema,
    /// Hat instructions reference payload fields for a trigger topic, but
    /// the topic has no schema defined. (Strict-mode only; otherwise a warning.)
    SchemaMissingForRequiredTopic,
}

/// A single payload contract error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadContractError {
    pub kind: PayloadContractErrorKind,
    /// Hat whose instructions reference a payload field.
    pub hat_id: String,
    /// Trigger topic the hat subscribes to.
    pub topic: String,
    /// Field name extracted from instructions. None for schema-missing errors.
    pub field: Option<String>,
    /// Hat IDs that can publish the trigger topic (one or more).
    pub source_hats: Vec<String>,
    /// Where the schema was defined: "inline", "file:schemas.yml", or "(none)".
    pub schema_defined_in: String,
    /// Line number in the hat instructions (if known).
    pub instructions_line: Option<usize>,
    /// Pattern that extracted the field reference.
    pub pattern: Option<String>,
    /// Original excerpt from the instructions.
    pub source_excerpt: Option<String>,
    /// Human-readable message.
    pub message: String,
}

/// Result of payload contract validation.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PayloadContractValidationResult {
    pub errors: Vec<PayloadContractError>,
    pub warnings: Vec<String>,
}

impl PayloadContractValidationResult {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Runtime payload contract violation report (U6)
// ──────────────────────────────────────────────────────────────────────────

/// Classification of the runtime violation. Mirrors the static
/// `PayloadContractErrorKind` plus a `TypeMismatch` variant.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayloadContractViolationKind {
    /// Required field missing from the actual payload.
    MissingRequiredField,
    /// Actual payload shape does not match schema's declared type.
    PayloadTypeMismatch,
    /// Field value not in the schema's allowed values.
    AllowedValueMismatch,
    /// Trigger topic has no schema defined (caller did not provide one).
    SchemaMissingForRequiredTopic,
}

/// Runtime payload contract violation (U6).
///
/// Emitted by the event policy layer when a real event violates the
/// configured schema. Distinct from the static `PayloadContractError`,
/// which is generated from hat instructions at validate time.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PayloadContractViolation {
    /// Type of the violation.
    pub error_type: PayloadContractViolationKind,
    /// Timestamp (RFC3339) when the violation was detected.
    pub timestamp: String,
    /// Topic that triggered the violation.
    pub topic: String,
    /// Field name involved in the violation (if applicable).
    pub field: Option<String>,
    /// Source hat(s) that published the topic (the producer side).
    pub source_hat: Vec<String>,
    /// Target hat(s) that subscribe to the topic (the consumer side).
    pub target_hat: Vec<String>,
    /// Where the schema was defined ("inline", "file:schemas.yml", etc).
    pub schema_defined_in: String,
    /// Snippet from the consumer hat's instructions that referenced the
    /// field (when available).
    pub downstream_reference: Option<String>,
    /// Snippet from the producer-side that published the topic.
    pub upstream_reference: Option<String>,
    /// Human-readable fix suggestion.
    pub fix_hint: String,
    /// Raw payload text that violated the schema (truncated for safety).
    pub payload_excerpt: Option<String>,
}


///
/// For each hat trigger topic that has at least one payload field reference
/// in its instructions, the validator checks:
/// - Whether the topic has a schema defined (inline or via `schema_file`).
/// - Whether every referenced field is declared in the schema's
///   `required_fields`.
///
/// In strict mode, missing schemas for required topics are reported as errors;
/// in default mode they are reported as warnings. Missing required fields
/// are always errors regardless of mode.
///
/// When multiple hats publish the same trigger topic, the error lists all
/// candidate source hats (the validator never picks one).
///
/// Hatless/solo mode (no custom hats) is a pass: "no contract to validate".
pub fn validate_payload_contract(
    config: &RalphConfig,
    registry: &HatRegistry,
    strict: bool,
) -> PayloadContractValidationResult {
    let mut result = PayloadContractValidationResult::default();

    // Collect effective (hat_id, Hat) pairs. We iterate registry hats,
    // excluding the runtime-only `ralph` fallback. Config-only hats (declared
    // in `hats:` but not registered) are handled separately below.
    let registry_hats: Vec<(String, &ralph_proto::Hat)> = registry
        .all()
        .filter(|h| !h.is_fallback_only())
        .map(|h| (h.id.as_str().to_string(), h))
        .collect();

    if registry_hats.is_empty() && config.hats.is_empty() {
        // Hatless / solo mode: pass.
        return result;
    }

    // Build the source-hats index: topic -> list of publishing hat IDs.
    let mut source_hats_by_topic: HashMap<String, Vec<String>> = HashMap::new();
    for (hat_id, hat_config) in &config.hats {
        let mut pub_topics: Vec<String> = hat_config.publishes.clone();
        if let Some(default) = &hat_config.default_publishes {
            if !pub_topics.contains(default) {
                pub_topics.push(default.clone());
            }
        }
        for t in pub_topics {
            source_hats_by_topic.entry(t).or_default().push(hat_id.clone());
        }
    }
    // Also include the registry hat's declared publishes for completeness.
    for (id, _) in &registry_hats {
        if let Some(hat) = registry.get(&HatId::new(id.clone())) {
            for t in hat.publishes.iter().map(|p| p.as_str().to_string()) {
                source_hats_by_topic.entry(t).or_default().push(id.clone());
            }
        }
    }

    // Resolve schemas from event_policy (inline takes priority; file schemas
    // are merged into `schemas` by `resolve_schema_files` before validation).
    let schemas: HashMap<String, &crate::config::EventSchema> = config
        .event_loop
        .event_policy
        .as_ref()
        .map(|p| p.schemas.iter().map(|(k, v)| (k.clone(), v)).collect())
        .unwrap_or_default();

    let schema_file_label = config
        .event_loop
        .event_policy
        .as_ref()
        .and_then(|p| p.schema_file.clone());

    // Per-hat validation
    for (hat_id, hat) in &registry_hats {
        // Find the matching HatConfig for instructions and ignore list.
        let hat_config = config.hats.get(hat_id);

        let instructions = hat_config.map(|c| c.instructions.as_str()).unwrap_or("");
        let ignore_fields: Vec<String> = hat_config
            .map(|c| c.ignore_payload_fields.clone())
            .unwrap_or_default();

        for sub in &hat.subscriptions {
            let topic_str = sub.as_str();
            // Skip wildcard subscriptions: payload contract checks
            // apply to concrete topics only (we cannot know which concrete
            // events will arrive under a wildcard).
            if topic_str.contains('*') {
                continue;
            }

            // Extract payload field references from this hat's instructions.
            let refs = extract_payload_field_refs(hat_id, instructions, &ignore_fields);
            if refs.is_empty() {
                continue;
            }

            // Find the source hats that publish this topic.
            let mut source_hats: Vec<String> = source_hats_by_topic
                .get(topic_str)
                .cloned()
                .unwrap_or_default();
            source_hats.sort();
            source_hats.dedup();

            // Find the schema for this topic.
            let schema_defined_in = match schemas.get(topic_str) {
                Some(_) => match &schema_file_label {
                    Some(f) => format!("inline + file:{}", f),
                    None => "inline".to_string(),
                },
                None => "(none)".to_string(),
            };

            match schemas.get(topic_str) {
                None => {
                    // Topic has no schema. Strict-mode: error. Default: warning.
                    let msg = format!(
                        "Hat '{}' references payload fields for topic '{}' but no schema is defined. \
                         Add an `event_policy.schemas.{}` entry or a `schema_file`.",
                        hat_id, topic_str, topic_str
                    );
                    if strict {
                        result.errors.push(PayloadContractError {
                            kind: PayloadContractErrorKind::SchemaMissingForRequiredTopic,
                            hat_id: hat_id.clone(),
                            topic: topic_str.to_string(),
                            field: None,
                            source_hats: source_hats.clone(),
                            schema_defined_in: schema_defined_in.clone(),
                            instructions_line: None,
                            pattern: None,
                            source_excerpt: None,
                            message: msg,
                        });
                    } else {
                        result.warnings.push(msg);
                    }
                }
                Some(schema) => {
                    // Check each referenced field against the schema.
                    for r in &refs {
                        if !schema.required_fields.iter().any(|f| f == &r.field) {
                            let msg = format!(
                                "Hat '{}' (topic '{}') references payload field '{}' which is NOT \
                                 in the schema's required_fields. Source hat(s): [{}]. \
                                 Add '{}' to event_policy.schemas.{}.required_fields.",
                                hat_id,
                                topic_str,
                                r.field,
                                source_hats.join(", "),
                                r.field,
                                topic_str
                            );
                            result.errors.push(PayloadContractError {
                                kind: PayloadContractErrorKind::FieldMissingFromSchema,
                                hat_id: hat_id.clone(),
                                topic: topic_str.to_string(),
                                field: Some(r.field.clone()),
                                source_hats: source_hats.clone(),
                                schema_defined_in: schema_defined_in.clone(),
                                instructions_line: Some(r.line),
                                pattern: Some(r.pattern.clone()),
                                source_excerpt: Some(r.source_excerpt.clone()),
                                message: msg,
                            });
                        }
                    }
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_event_payload() {
        let instructions = "From event payload: task_id, plan_name, step";
        let refs = extract_payload_field_refs("test-hat", instructions, &[]);
        assert_eq!(refs.len(), 3);
        assert!(refs.iter().any(|r| r.field == "task_id" && r.pattern == "From event payload"));
        assert!(refs.iter().any(|r| r.field == "plan_name" && r.pattern == "From event payload"));
        assert!(refs.iter().any(|r| r.field == "step" && r.pattern == "From event payload"));
    }

    #[test]
    fn test_payload_must_include() {
        let instructions = "payload MUST include: task_id, task_key";
        let refs = extract_payload_field_refs("test-hat", instructions, &[]);
        assert_eq!(refs.len(), 2);
        assert!(refs.iter().any(|r| r.field == "task_id" && r.pattern == "payload MUST include"));
        assert!(refs.iter().any(|r| r.field == "task_key" && r.pattern == "payload MUST include"));
    }

    #[test]
    fn test_backtick_field_on_payload_line() {
        let instructions = "From event payload: read `task_id`, `task_key`, `step`";
        let refs = extract_payload_field_refs("test-hat", instructions, &[]);
        // Should extract from both "From event payload" and backtick patterns
        let fields: Vec<_> = refs.iter().map(|r| r.field.clone()).collect();
        assert!(fields.contains(&"task_id".to_string()));
        assert!(fields.contains(&"task_key".to_string()));
        assert!(fields.contains(&"step".to_string()));
    }

    #[test]
    fn test_ignore_fields() {
        let instructions = "From event payload: task_id, plan_name, step";
        let refs = extract_payload_field_refs("test-hat", instructions, &["plan_name".to_string()]);
        assert_eq!(refs.len(), 2);
        assert!(!refs.iter().any(|r| r.field == "plan_name"));
        assert!(refs.iter().any(|r| r.field == "task_id"));
        assert!(refs.iter().any(|r| r.field == "step"));
    }

    #[test]
    fn test_deduplication() {
        let instructions = "From event payload: task_id\npayload MUST include: task_id";
        let refs = extract_payload_field_refs("test-hat", instructions, &[]);
        // Should only have one entry for task_id
        let task_id_count = refs.iter().filter(|r| r.field == "task_id").count();
        assert_eq!(task_id_count, 1);
    }

    #[test]
    fn test_case_insensitive_payload() {
        let instructions = "PAYLOAD MUST include: task_id\nfrom event payload: task_key";
        let refs = extract_payload_field_refs("test-hat", instructions, &[]);
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn test_empty_instructions() {
        let refs = extract_payload_field_refs("test-hat", "", &[]);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_multiline_instructions() {
        let instructions = r#"
        ## COORDINATOR MODE
        Read state from event payload: plan_name, task_id
        payload MUST include: step, complexity
       "#;
        let refs = extract_payload_field_refs("coordinator", instructions, &[]);
        let fields: Vec<_> = refs.iter().map(|r| r.field.clone()).collect();
        assert!(fields.contains(&"plan_name".to_string()));
        assert!(fields.contains(&"task_id".to_string()));
        assert!(fields.contains(&"step".to_string()));
        assert!(fields.contains(&"complexity".to_string()));
    }

    #[test]
    fn test_stable_sort_order() {
        let instructions = "From event payload: z_field, a_field, m_field";
        let refs = extract_payload_field_refs("test-hat", instructions, &[]);
        let fields: Vec<_> = refs.iter().map(|r| r.field.clone()).collect();
        // Should be sorted
        let mut sorted = fields.clone();
        sorted.sort();
        assert_eq!(fields, sorted);
    }

    #[test]
    fn test_line_numbers_correct() {
        let instructions = "Line 1 no match\nFrom event payload: task_id\nLine 3 no match\npayload MUST include: task_key";
        let refs = extract_payload_field_refs("test-hat", instructions, &[]);
        let task_id_ref = refs.iter().find(|r| r.field == "task_id").unwrap();
        let task_key_ref = refs.iter().find(|r| r.field == "task_key").unwrap();
        assert_eq!(task_id_ref.line, 2);
        assert_eq!(task_key_ref.line, 4);
    }

    #[test]
    fn test_backtick_fields_with_spaces_filtered() {
        // Backtick pattern requires explicit payload intent: lines without
        // "from event payload" or "payload MUST include" do NOT extract.
        let instructions = "From event payload: read `field with spaces` and `normal_field`";
        let refs = extract_payload_field_refs("test-hat", instructions, &[]);
        // Should only extract normal_field, not "field with spaces"
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].field, "normal_field");
    }

    #[test]
    fn test_comment_lines_skipped() {
        let instructions = "From event payload: task_id, # comment, task_key";
        let refs = extract_payload_field_refs("test-hat", instructions, &[]);
        // Should not extract "# comment" as a field
        let fields: Vec<_> = refs.iter().map(|r| r.field.clone()).collect();
        assert!(!fields.contains(&"# comment".to_string()));
    }

    #[test]
    fn test_source_excerpt_preserved() {
        let line = "From event payload: task_id, plan_name";
        let instructions = format!("Some context\n{}\nMore context", line);
        let refs = extract_payload_field_refs("test-hat", &instructions, &[]);
        let task_id_ref = refs.iter().find(|r| r.field == "task_id").unwrap();
        assert_eq!(task_id_ref.source_excerpt, line);
    }

    // ──────────────────────────────────────────────────────────────────────
    // U3 Payload Contract Validator tests
    // ──────────────────────────────────────────────────────────────────────

    fn runtime_registry(yaml: &str) -> HatRegistry {
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        HatRegistry::from_runtime_config(&config)
    }

    // A small, complete fixture helper that keeps the YAML small and
    // unambiguous. Each test injects only the policy/hats/instructions it
    // cares about.
    fn two_hat_fixture(
        event_policy_block: &str,
        b_instructions: &str,
        a_instructions: &str,
    ) -> String {
        let policy = if event_policy_block.is_empty() {
            String::new()
        } else {
            format!("  event_policy:\n{}\n", event_policy_block)
        };
        format!(
            "event_loop:\n\
             {policy}\
             hats:\n\
             \x20 a:\n\
             \x20\x20 name: \"A\"\n\
             \x20\x20 triggers: [\"work.start\"]\n\
             \x20\x20 publishes: [\"work.ready\"]\n\
             \x20\x20 instructions: \"{a_instructions}\"\n\
             \x20 b:\n\
             \x20\x20 name: \"B\"\n\
             \x20\x20 triggers: [\"work.ready\"]\n\
             \x20\x20 publishes: [\"LOOP_COMPLETE\"]\n\
             \x20\x20 instructions: |\n\
             \x20\x20\x20 {b_instructions}\n"
        )
    }

    #[test]
    fn validator_empty_hats_passes() {
        // Solo / hatless mode: no custom hats, no contract to validate.
        let config = RalphConfig::default();
        let registry = runtime_registry("hats: {}");
        let result = validate_payload_contract(&config, &registry, false);
        assert!(result.is_valid(), "Empty hats should be valid: {:?}", result);
    }

    #[test]
    fn validator_no_payload_refs_passes() {
        // Hat has no payload field references in instructions → no contract to enforce.
        let yaml = two_hat_fixture("", "Just complete.", "Just publish.");
        let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
        let registry = runtime_registry(&yaml);
        let result = validate_payload_contract(&config, &registry, true);
        assert!(
            result.is_valid(),
            "Hat with no payload refs should be valid: {:?}",
            result
        );
    }

    #[test]
    fn validator_missing_schema_strict_is_error() {
        // Hat b references payload fields for `work.ready` but no schema is
        // defined. In strict mode this must be an error.
        let yaml = two_hat_fixture(
            "",
            "From event payload: task_id, plan_name",
            "Publish a work.ready event.",
        );
        let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
        let registry = runtime_registry(&yaml);
        let result = validate_payload_contract(&config, &registry, true);
        assert!(!result.is_valid(), "Strict mode: missing schema must be error");
        let err = result
            .errors
            .iter()
            .find(|e| e.topic == "work.ready"
                && e.kind == PayloadContractErrorKind::SchemaMissingForRequiredTopic)
            .expect("expected SchemaMissingForRequiredTopic error for work.ready");
        assert_eq!(err.hat_id, "b");
    }

    #[test]
    fn validator_missing_schema_default_is_warning() {
        // Same as above but in default (non-strict) mode: must be a warning, not an error.
        let yaml = two_hat_fixture(
            "",
            "From event payload: task_id, plan_name",
            "Publish a work.ready event.",
        );
        let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
        let registry = runtime_registry(&yaml);
        let result = validate_payload_contract(&config, &registry, false);
        assert!(
            result.is_valid(),
            "Default mode: missing schema should be a warning, not error"
        );
        assert!(
            result.warnings.iter().any(|w| w.contains("work.ready")),
            "Expected a warning mentioning work.ready: {:?}",
            result.warnings
        );
    }

    #[test]
    fn validator_field_not_in_required_fields_is_error() {
        // Schema declares only `task_id` as required, but the consumer
        // references both `task_id` and `plan_name`. `plan_name` is missing
        // from the schema's required_fields, so this is an error.
        let policy = "    enabled: true\n    mode: observe\n    schemas:\n      work.ready:\n        required_fields: [\"task_id\"]";
        let yaml = two_hat_fixture(
            policy,
            "From event payload: task_id, plan_name",
            "Publish a work.ready event.",
        );
        let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
        let registry = runtime_registry(&yaml);
        let result = validate_payload_contract(&config, &registry, false);
        assert!(!result.is_valid(), "Missing field must be error");
        let err = result
            .errors
            .iter()
            .find(|e| e.field.as_deref() == Some("plan_name"))
            .expect("expected FieldMissingFromSchema error for plan_name");
        assert_eq!(err.hat_id, "b");
        assert_eq!(err.topic, "work.ready");
        assert_eq!(err.kind, PayloadContractErrorKind::FieldMissingFromSchema);
    }

    #[test]
    fn validator_all_fields_in_required_fields_passes() {
        let policy = "    enabled: true\n    mode: observe\n    schemas:\n      work.ready:\n        required_fields: [\"task_id\", \"plan_name\"]";
        let yaml = two_hat_fixture(
            policy,
            "From event payload: task_id, plan_name",
            "Publish.",
        );
        let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
        let registry = runtime_registry(&yaml);
        let result = validate_payload_contract(&config, &registry, true);
        assert!(
            result.is_valid(),
            "All fields covered should be valid: {:?}",
            result
        );
    }

    #[test]
    fn validator_multiple_source_hats_listed_in_error() {
        // Two hats publish the same trigger topic. The error must list both
        // as candidate source hats. The error must also be
        // FieldMissingFromSchema (not SchemaMissingForRequiredTopic) — the
        // schema exists but doesn't declare `extra_field`.
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
  a2:
    name: "A2"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish too."
  b:
    name: "B"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id, extra_field
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_payload_contract(&config, &registry, false);
        assert!(!result.is_valid());
        let err = result
            .errors
            .iter()
            .find(|e| e.hat_id == "b"
                && e.topic == "work.ready"
                && e.field.as_deref() == Some("extra_field"))
            .expect("expected FieldMissingFromSchema error for extra_field");
        assert_eq!(err.kind, PayloadContractErrorKind::FieldMissingFromSchema);
        // Must list BOTH source hats, not guess one.
        assert!(
            err.source_hats.contains(&"a".to_string()),
            "source_hats must include 'a': {:?}",
            err.source_hats
        );
        assert!(
            err.source_hats.contains(&"a2".to_string()),
            "source_hats must include 'a2': {:?}",
            err.source_hats
        );
    }

    #[test]
    fn validator_ignore_payload_fields_excluded() {
        // `plan_name` is in ignore_payload_fields → must NOT trigger an error
        // even if the schema does not declare it.
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
    ignore_payload_fields: ["plan_name"]
    instructions: |
      From event payload: task_id, plan_name
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_payload_contract(&config, &registry, false);
        assert!(
            result.is_valid(),
            "ignored field should not trigger error: {:?}",
            result
        );
    }

    #[test]
    fn validator_error_includes_line_and_source_excerpt() {
        // Error must include the line number and source excerpt for diagnostics.
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
      Some preamble
      From event payload: task_id, plan_name
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = runtime_registry(yaml);
        let result = validate_payload_contract(&config, &registry, false);
        assert!(!result.is_valid());
        let err = result
            .errors
            .iter()
            .find(|e| e.field.as_deref() == Some("plan_name"))
            .expect("expected plan_name error");
        // The literal block scalar "  |" with 3 lines of content starts at
        // logical line 1 for hat b's instructions; the From-event-payload
        // line is the 2nd content line.
        assert!(
            err.instructions_line.unwrap_or(0) >= 2,
            "instructions_line should be 2 or later: {:?}",
            err.instructions_line
        );
        assert!(err
            .source_excerpt
            .as_deref()
            .unwrap_or("")
            .contains("plan_name"));
    }

    #[test]
    fn validator_error_message_mentions_hat_topic_and_field() {
        // Error message must be actionable: include hat id, topic, and field.
        let policy = "    enabled: true\n    mode: observe\n    schemas:\n      work.ready:\n        required_fields: [\"task_id\"]";
        let yaml = two_hat_fixture(
            policy,
            "From event payload: task_id, plan_name",
            "Publish.",
        );
        let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
        let registry = runtime_registry(&yaml);
        let result = validate_payload_contract(&config, &registry, false);
        let err = result
            .errors
            .iter()
            .find(|e| e.field.as_deref() == Some("plan_name"))
            .expect("expected plan_name error");
        assert!(err.message.contains("b"), "msg should mention hat: {}", err.message);
        assert!(err.message.contains("work.ready"), "msg should mention topic: {}", err.message);
        assert!(err.message.contains("plan_name"), "msg should mention field: {}", err.message);
    }
}
