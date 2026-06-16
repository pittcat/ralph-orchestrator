//! Schema-aware emit hint generation.
//!
//! Single source of truth for `ralph emit <topic> --json '{...}'` examples
//! used by both the agent prompt (B layer) and the CLI pre-publish error
//! message (C layer). The hat-scoping rule is enforced here: a hat must
//! only see `--json` examples for topics it actually declares in
//! `publishes`. Cross-hat example leakage is treated as a bug.
//!
//! See `docs/plans/2026-06-15-001-feat-schema-aware-hat-emit-instructions-plan.md`
//! sections 4.1 and 4.3 C3 for the design rationale.

use crate::config::{EventSchema, PayloadType};
use ralph_proto::Hat;
use std::collections::HashMap;

/// Generate a single copy-pasteable `ralph emit ... --json '...'` line.
///
/// The placeholder values are heuristic placeholders chosen to be obvious
/// (so an agent will replace them with real data), not values that might
/// accidentally satisfy the schema validator.
pub fn format_emit_json_example(topic: &str, schema: &EventSchema) -> String {
    let example_payload = example_json_object(schema);
    format!(
        "ralph emit {topic} --json '{example_payload}'",
        topic = topic,
        example_payload = example_payload
    )
}

/// Build the §3 REPORT block for a hat, listing copy-pasteable `--json`
/// examples for every topic the hat is allowed to publish.
///
/// If the hat publishes nothing, or no schema is registered for any of
/// the hat's publish topics, an empty string is returned so the caller
/// can fall back to the legacy `<summary>` template.
pub fn build_publish_emit_section(
    hat: &Hat,
    schemas: &HashMap<String, EventSchema>,
) -> String {
    let mut sections: Vec<String> = Vec::new();

    for topic in &hat.publishes {
        let topic_str = topic.as_str();
        let schema = match schemas.get(topic_str) {
            Some(s) => s,
            None => continue,
        };
        sections.push(format!(
            "- {example}",
            example = format_emit_json_example(topic_str, schema)
        ));
    }

    if sections.is_empty() {
        return String::new();
    }

    let topics: Vec<&str> = hat.publishes.iter().map(|t| t.as_str()).collect();
    format!(
        "Publish exactly ONE of: `{topics}`.\n\
         Each example below matches that topic's schema. Use the exact `--json` form.\n\n\
         {lines}\n\n\
         MUST NOT append or write to events.jsonl directly; use `ralph emit` / \
         `ralph wave emit` only.",
        topics = topics.join("`, `"),
        lines = sections.join("\n")
    )
}

/// Build a fix-hint string for the CLI pre-publish rejection path.
///
/// Returns `Some(hint)` only when the hat is authorised to publish the
/// topic in question (i.e. the topic appears in `hat.publishes`). If the
/// hat has no authority for the topic, `None` is returned so the caller
/// refuses to leak another hat's payload shape into the error output.
pub fn fix_hint_for_hat_topic(
    hat: &Hat,
    topic: &str,
    schema: &EventSchema,
) -> Option<String> {
    let authorised = hat
        .publishes
        .iter()
        .any(|t| t.matches_str(topic));

    if !authorised {
        return None;
    }

    let topics: Vec<&str> = hat.publishes.iter().map(|t| t.as_str()).collect();
    let example = format_emit_json_example(topic, schema);

    let mut hint = String::new();
    hint.push_str(&format!(
        "Your hat `{hat_id}` may publish: {topics}.\n",
        hat_id = hat.id.as_str(),
        topics = topics.join(", ")
    ));
    hint.push_str("\nFix — run exactly:\n");
    hint.push_str(&format!("  {example}\n"));

    if !schema.required_fields.is_empty() {
        hint.push_str(&format!(
            "\nRequired fields: {fields}",
            fields = schema.required_fields.join(", ")
        ));
    }

    Some(hint)
}

/// Render a JSON object payload example that satisfies the schema's
/// required-fields list and matches the declared payload type. Strings
/// are used for textual fields so the example reads naturally; numbers
/// and booleans use sensible primitives; arrays are rendered as empty
/// arrays so the agent will fill them.
fn example_json_object(schema: &EventSchema) -> String {
    match schema.payload.as_ref() {
        Some(PayloadType::String) | None => {
            // String payloads have no field-level schema to mirror.
            "<summary text>".to_string()
        }
        Some(PayloadType::JsonObject) => {
            let mut parts: Vec<String> = Vec::new();
            for field in &schema.required_fields {
                parts.push(format!("\"{field}\": \"<{field}>\""));
            }
            if parts.is_empty() {
                "{}".to_string()
            } else {
                format!("{{{}}}", parts.join(", "))
            }
        }
        Some(PayloadType::Number) => "0".to_string(),
        Some(PayloadType::Bool) => "false".to_string(),
        Some(PayloadType::Array) => "[]".to_string(),
    }
}

/// `Topic::matches_str` per-segment glob semantics are inherited from
/// the runtime subscription matcher (see `Topic::matches_str` in
/// `ralph-proto/src/topic.rs`), so a hat publishing `review.*` will not
/// authorise `review.sub.done` in the CLI hint path — matching the
/// behaviour the loop will actually enforce.

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_proto::Topic;

    fn schema_with_required(fields: &[&str]) -> EventSchema {
        EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: fields.iter().map(|s| s.to_string()).collect(),
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
        }
    }

    fn string_schema() -> EventSchema {
        EventSchema {
            payload: Some(PayloadType::String),
            required_fields: vec![],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
        }
    }

    fn hat_with_publishes(id: &str, name: &str, topics: &[&str]) -> Hat {
        Hat::new(id, name)
            .with_description("")
            .with_publishes(topics.iter().map(|t| Topic::new(*t)).collect())
    }

    #[test]
    fn format_emit_json_example_object_renders_required_fields() {
        let schema = schema_with_required(&["plan_name", "task_id"]);
        let line = format_emit_json_example("work.ready", &schema);

        assert!(line.starts_with("ralph emit work.ready --json '{"));
        assert!(line.contains("\"plan_name\": \"<plan_name>\""));
        assert!(line.contains("\"task_id\": \"<task_id>\""));
    }

    #[test]
    fn format_emit_json_example_string_payload_uses_summary_placeholder() {
        let schema = string_schema();
        let line = format_emit_json_example("work.failed", &schema);
        assert_eq!(line, "ralph emit work.failed --json '<summary text>'");
    }

    #[test]
    fn format_emit_json_example_object_with_no_required_renders_empty_object() {
        let schema = schema_with_required(&[]);
        let line = format_emit_json_example("work.done", &schema);
        assert!(line.ends_with("--json '{}'"));
    }

    #[test]
    fn build_publish_emit_section_only_lists_schemas_topics() {
        let mut schemas = HashMap::new();
        schemas.insert("work.ready".to_string(), schema_with_required(&["plan_name"]));
        schemas.insert(
            "work.failed".to_string(),
            schema_with_required(&["reason"]),
        );
        // work.done has no schema registered — must be skipped.
        let hat = hat_with_publishes(
            "coordinator",
            "Coordinator",
            &["work.ready", "work.failed", "work.done"],
        );

        let section = build_publish_emit_section(&hat, &schemas);

        assert!(section.contains("work.ready"));
        assert!(section.contains("work.failed"));
        assert!(!section.contains("work.done --json"));
        assert!(section.contains("MUST NOT append or write to events.jsonl directly"));
        assert!(section.contains("ralph emit work.ready --json"));
    }

    #[test]
    fn build_publish_emit_section_returns_empty_when_no_schemas_match() {
        let schemas = HashMap::new();
        let hat = hat_with_publishes("coordinator", "Coordinator", &["work.ready"]);
        assert!(build_publish_emit_section(&hat, &schemas).is_empty());
    }

    #[test]
    fn fix_hint_returns_some_for_authorised_topic() {
        let schema = schema_with_required(&["plan_name", "plan_path"]);
        let hat = hat_with_publishes(
            "coordinator",
            "Coordinator",
            &["work.ready", "work.failed"],
        );

        let hint = fix_hint_for_hat_topic(&hat, "work.ready", &schema)
            .expect("coordinator is authorised to publish work.ready");

        assert!(hint.contains("Your hat `coordinator` may publish"));
        assert!(hint.contains("work.ready"));
        assert!(hint.contains("work.failed"));
        assert!(hint.contains("Required fields: plan_name, plan_path"));
        assert!(hint.contains("ralph emit work.ready --json"));
    }

    #[test]
    fn fix_hint_returns_none_for_unauthorised_topic() {
        let schema = schema_with_required(&["task_id"]);
        let hat = hat_with_publishes(
            "coordinator",
            "Coordinator",
            &["work.ready", "work.failed"],
        );

        assert!(
            fix_hint_for_hat_topic(&hat, "work.done", &schema).is_none(),
            "coordinator is not authorised to publish work.done"
        );
    }

    /// Plan 002 Unit 7: handoff topic `queue.advance` fix_hint coverage.
    /// The plan-gate hat publishes `queue.advance`; the CLI precheck
    /// rejection path must surface a copy-pasteable `--json` example
    /// matching the SSOT schema's required_fields.
    #[test]
    fn fix_hint_covers_handoff_topic_queue_advance() {
        let schema = schema_with_required(&[
            "plan_name",
            "completed_step",
            "next_step",
            "reviewed_task_id",
            "reviewed_task_key",
        ]);
        let hat = hat_with_publishes("plan-gate", "Plan Gate", &["queue.advance"]);

        let hint = fix_hint_for_hat_topic(&hat, "queue.advance", &schema)
            .expect("plan-gate is authorised to publish queue.advance");

        assert!(hint.contains("queue.advance"));
        assert!(hint.contains("ralph emit queue.advance --json"));
        assert!(hint.contains("Required fields: plan_name, completed_step"));
    }

    /// Plan 002 Unit 7: handoff topic `review.passed` fix_hint coverage.
    /// The review-synthesizer hat publishes `review.passed`; the CLI
    /// precheck rejection path must surface a copy-pasteable `--json`
    /// example matching the SSOT schema's required_fields.
    #[test]
    fn fix_hint_covers_handoff_topic_review_passed() {
        let schema = schema_with_required(&[
            "plan_name",
            "task_id",
            "task_key",
            "step",
            "findings_count",
            "fix_round",
            "verdict",
            "skip_reason",
        ]);
        let hat = hat_with_publishes(
            "review-synthesizer",
            "Review Synthesizer",
            &["review.passed", "review.failed", "review.complete"],
        );

        let hint = fix_hint_for_hat_topic(&hat, "review.passed", &schema)
            .expect("review-synthesizer is authorised to publish review.passed");

        assert!(hint.contains("review.passed"));
        assert!(hint.contains("ralph emit review.passed --json"));
        assert!(hint.contains("Required fields: plan_name, task_id"));
    }

    #[test]
    fn fix_hint_honours_wildcard_publish_pattern() {
        let schema = schema_with_required(&["x"]);
        let hat = Hat::new("reviewer", "Reviewer")
            .with_description("")
            .with_publishes(vec![Topic::new("review.*")]);

        let hint =
            fix_hint_for_hat_topic(&hat, "review.done", &schema).expect("wildcard");
        assert!(hint.contains("review.done"));
    }

    #[test]
    fn fix_hint_rejects_multi_segment_wildcard_mismatch() {
        // Plan 001 §4.1 P0: a hat publishing `review.*` must NOT authorise
        // `review.sub.done` in the CLI hint path. Topic::matches_str uses
        // per-segment globbing, so multi-segment topics are rejected.
        let schema = schema_with_required(&["x"]);
        let hat = Hat::new("reviewer", "Reviewer")
            .with_description("")
            .with_publishes(vec![Topic::new("review.*")]);

        assert!(
            fix_hint_for_hat_topic(&hat, "review.sub.done", &schema).is_none(),
            "review.* must not match review.sub.done (per-segment glob)"
        );
    }

    #[test]
    fn fix_hint_omits_required_fields_line_when_empty() {
        let schema = schema_with_required(&[]);
        let hat = hat_with_publishes("a", "A", &["work.ready"]);
        let hint = fix_hint_for_hat_topic(&hat, "work.ready", &schema).unwrap();
        assert!(!hint.contains("Required fields:"));
    }

    #[test]
    fn example_payload_types_render_distinct_shapes() {
        let array_schema = EventSchema {
            payload: Some(PayloadType::Array),
            required_fields: vec![],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
        };
        assert_eq!(
            format_emit_json_example("t", &array_schema),
            "ralph emit t --json '[]'"
        );

        let number_schema = EventSchema {
            payload: Some(PayloadType::Number),
            required_fields: vec![],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
        };
        assert_eq!(
            format_emit_json_example("t", &number_schema),
            "ralph emit t --json '0'"
        );

        let bool_schema = EventSchema {
            payload: Some(PayloadType::Bool),
            required_fields: vec![],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
        };
        assert_eq!(
            format_emit_json_example("t", &bool_schema),
            "ralph emit t --json 'false'"
        );
    }
}
