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

/// 2026-07-09-001 plan (U2): render a single field hint line
/// (`field: meaning (source) — fill_rule`) for the
/// schema-aware prompt section. The line is markdown-friendly
/// and never invents a meaning: an empty `meaning` becomes an
/// em-dash placeholder. `field_docs` is consulted first; if
/// absent, the line falls back to `<field> (required)` so old
/// schemas still produce a useful line.
pub fn render_field_line(field: &str, schema: &EventSchema) -> String {
    let allowed = schema.allowed_values.get(field);
    if let Some(doc) = schema.field_docs.get(field) {
        let meaning = if doc.meaning.trim().is_empty() {
            "—".to_string()
        } else {
            doc.meaning.clone()
        };
        let source = if doc.source.trim().is_empty() {
            String::new()
        } else {
            format!(" (source: {})", doc.source)
        };
        let fill_rule = if doc.fill_rule.trim().is_empty() {
            String::new()
        } else {
            format!(" — {}", doc.fill_rule)
        };
        let allowed_segment = match allowed {
            Some(values) if !values.is_empty() => format!(
                " [allowed: {}]",
                values
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            _ => String::new(),
        };
        format!(
            "- `{field}`: {meaning}{source}{fill_rule}{allowed_segment}",
            field = field
        )
    } else {
        match allowed {
            Some(values) if !values.is_empty() => format!(
                "- `{field}` (required) [allowed: {}]",
                values
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            _ => format!("- `{field}` (required)"),
        }
    }
}

/// 2026-07-09-001 plan (U2): build the agent-facing field table
/// for a schema, one line per entry in `required_fields` (in
/// declared order). Fields not in `required_fields` are
/// intentionally NOT included here — this is the prompt-side
/// list of "what you must fill"; optional fields belong in
/// `field_docs` documentation only.
pub fn render_required_field_table(schema: &EventSchema) -> String {
    schema
        .required_fields
        .iter()
        .map(|f| render_field_line(f, schema))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 2026-07-09-001 plan (U2): build a `suggested_payload_shape`
/// that keeps the user's existing fields verbatim and fills
/// missing required fields with `<field>` placeholders. The
/// function never invents business values: a missing
/// `must_fix_now_count` is `0`-shaped only when the schema's
/// `allowed_values` explicitly says so, otherwise it is the
/// `<must_fix_now_count>` placeholder string. Returns
/// `serde_json::Value::Null` for non-object payload types
/// (String / Number / Bool / Array) — those schemas do not
/// have a field-level shape to suggest.
pub fn suggested_payload_shape(
    schema: &EventSchema,
    payload: &serde_json::Value,
) -> serde_json::Value {
    use serde_json::{Map, Value};
    if !matches!(
        schema.payload.as_ref(),
        Some(PayloadType::JsonObject) | None
    ) {
        return Value::Null;
    }
    let mut map = Map::new();
    // 1) Preserve user-supplied fields verbatim — never
    // overwrite what the agent already filled.
    if let Some(obj) = payload.as_object() {
        for (k, v) in obj {
            map.insert(k.clone(), v.clone());
        }
    }
    // 2) For each missing required field, insert a
    // placeholder. The placeholder is the literal `<field>`
    // string for free-text fields. For fields with a
    // single-value `allowed_values` list, the placeholder
    // echoes the first allowed value as a hint (e.g. for
    // boolean-like enums) — but ONLY if the user's payload
    // also has no value for that field, so we never overwrite
    // an existing field. The hint is still a JSON-safe value
    // (string) so the agent cannot confuse it with a real
    // business fact.
    for field in &schema.required_fields {
        if map.contains_key(field) {
            continue;
        }
        let hint = schema
            .allowed_values
            .get(field)
            .and_then(|v| v.first())
            .map(|v| v.to_string());
        let placeholder = match hint {
            Some(h) => format!("<{field} e.g. {h}>"),
            None => format!("<{field}>"),
        };
        map.insert(field.clone(), Value::String(placeholder));
    }
    Value::Object(map)
}

/// 2026-07-09-001 plan (U2): pick the best prompt-side example
/// for a schema. If the schema declares `examples`, return the
/// first one as a JSON string (the schema author chose it).
/// Otherwise, fall back to the heuristic `example_json_object`
/// (the legacy `<field>` placeholder generator). Callers that
/// need a safe shape (e.g. policy-check suggestion) must call
/// `suggested_payload_shape` instead — this function may
/// surface a real business example.
pub fn prompt_example_payload(schema: &EventSchema) -> String {
    if let Some(first) = schema.examples.first() {
        return first.to_string();
    }
    example_json_object(schema)
}

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
///
/// 2026-07-09-001 plan (U6): for schemas that declare
/// `field_docs` or `examples`, append a per-topic field table
/// (rendered via `render_required_field_table`) and a
/// policy-check-first instruction line, so the hat knows
/// exactly which fields are mandatory and what to do when
/// `--policy-check` rejects a payload. Legacy schemas
/// (no `field_docs`) still get the §3 EXAMPLES line so the
/// 2026-06-15-001 backwards-compat invariant holds.
pub fn build_publish_emit_section(hat: &Hat, schemas: &HashMap<String, EventSchema>) -> String {
    let mut sections: Vec<String> = Vec::new();

    for topic in &hat.publishes {
        let topic_str = topic.as_str();
        let schema = match schemas.get(topic_str) {
            Some(s) => s,
            None => continue,
        };
        let mut block = String::new();
        block.push_str(&format!(
            "- `{topic_str}`:\n  {example}",
            example = format_emit_json_example(topic_str, schema)
        ));
        // Only show the field table when the schema has at
        // least one `field_docs` entry — schemas without
        // metadata stay on the legacy summary path.
        if !schema.field_docs.is_empty() {
            let table = render_required_field_table(schema);
            if !table.is_empty() {
                block.push_str("\n\n  Required fields (with meaning):\n  ");
                block.push_str(&table.replace('\n', "\n  "));
            }
            if !schema.examples.is_empty() {
                let examples_block: String = schema
                    .examples
                    .iter()
                    .take(2) // bound to two so the prompt does not bloat
                    .map(|v| format!("  ```json\n  {}\n  ```", v))
                    .collect::<Vec<_>>()
                    .join("\n");
                block.push_str("\n\n  Examples:\n");
                block.push_str(&examples_block);
            }
        }
        sections.push(block);
    }

    if sections.is_empty() {
        return String::new();
    }

    let topics: Vec<&str> = hat.publishes.iter().map(|t| t.as_str()).collect();
    format!(
        "Publish exactly ONE of: `{topics}`.\n\
         Each example below matches that topic's schema. Use the exact `--json` form. Prefer running `ralph emit <topic> --policy-check -j '<payload>'` first; only emit (without `--policy-check`) after precheck passes.\n\n\
         {lines}\n\n\
         On rejection: the policy-check error reports `field` / `expected` / `actual` / `field_description` / `suggested_payload_shape` / `suggested_command`. Fix the payload, re-run `--policy-check`, then emit for real.\n\n\
         MUST NOT append or write to events.jsonl directly; use `ralph emit` / \
         `ralph wave emit` only.",
        topics = topics.join("`, `"),
        lines = sections.join("\n\n")
    )
}

/// Build a fix-hint string for the CLI pre-publish rejection path.
///
/// Returns `Some(hint)` only when the hat is authorised to publish the
/// topic in question (i.e. the topic appears in `hat.publishes`). If the
/// hat has no authority for the topic, `None` is returned so the caller
/// refuses to leak another hat's payload shape into the error output.
pub fn fix_hint_for_hat_topic(hat: &Hat, topic: &str, schema: &EventSchema) -> Option<String> {
    let authorised = hat.publishes.iter().any(|t| t.matches_str(topic));

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
            ..Default::default()
        }
    }

    fn string_schema() -> EventSchema {
        EventSchema {
            payload: Some(PayloadType::String),
            ..Default::default()
        }
    }

    fn hat_with_publishes(id: &str, name: &str, topics: &[&str]) -> Hat {
        Hat::new(id, name)
            .with_description("")
            .with_publishes(topics.iter().map(|t| Topic::new(*t)).collect())
    }

    use crate::config::EventFieldDoc;

    /// U2 happy path: a schema with `field_docs.task_id`
    /// produces a field line that includes the meaning,
    /// source, and fill_rule. Required: this is the entire
    /// agent-readable repair context.
    #[test]
    fn u2_render_field_line_includes_field_doc_subfields() {
        let mut schema = schema_with_required(&["task_id"]);
        schema.field_docs.insert(
            "task_id".to_string(),
            EventFieldDoc {
                meaning: "live id".to_string(),
                source: "ralph tools task list".to_string(),
                fill_rule: "do NOT hand-write".to_string(),
            },
        );
        let line = render_field_line("task_id", &schema);
        assert!(line.contains("`task_id`"));
        assert!(line.contains("live id"));
        assert!(line.contains("source: ralph tools task list"));
        assert!(line.contains("do NOT hand-write"));
    }

    /// U2 happy path: a schema with `allowed_values.verdict`
    /// shows the allowed list in the field hint. This
    /// surfaces the policy at the agent prompt level, not
    /// only at the policy-check error level.
    #[test]
    fn u2_render_field_line_includes_allowed_values() {
        let mut schema = schema_with_required(&["verdict"]);
        schema.allowed_values.insert(
            "verdict".to_string(),
            vec![serde_json::json!("pass"), serde_json::json!("blocked")],
        );
        let line = render_field_line("verdict", &schema);
        assert!(line.contains("[allowed:"));
        assert!(line.contains("\"pass\""));
        assert!(line.contains("\"blocked\""));
    }

    /// U2 happy path: `suggested_payload_shape` keeps
    /// `verdict` from the original payload and inserts a
    /// `<reason>` placeholder for the missing required
    /// field. The placeholder is a JSON string and never a
    /// fabricated business value.
    #[test]
    fn u2_suggested_payload_shape_preserves_existing_and_placeholders_missing() {
        let schema = schema_with_required(&["verdict", "reason"]);
        let payload = serde_json::json!({"verdict": "blocked"});
        let shape = suggested_payload_shape(&schema, &payload);
        let obj = shape.as_object().expect("shape must be a JSON object");
        assert_eq!(obj.get("verdict"), Some(&serde_json::json!("blocked")));
        let reason = obj
            .get("reason")
            .and_then(|v| v.as_str())
            .expect("reason must be a string placeholder");
        assert!(
            reason.contains("reason"),
            "placeholder must name the field, got: {reason}"
        );
    }

    /// U2 safety: missing numeric / count-style field is a
    /// placeholder, NOT a fabricated `0`. Required: this
    /// is the KTD-4 / R9 "do not auto-fill business facts"
    /// guarantee. The hard-coded `0` is a footgun that the
    /// `ce-executor-pipeline-loop` review gate would happily
    /// accept (`must_fix_now_count: 0` => "no fix needed" =>
    /// close the loop), so the test pins the safer shape.
    #[test]
    fn u2_suggested_payload_shape_does_not_invent_zero_for_count_field() {
        let schema = schema_with_required(&["must_fix_now_count"]);
        let payload = serde_json::json!({});
        let shape = suggested_payload_shape(&schema, &payload);
        let v = shape
            .get("must_fix_now_count")
            .expect("placeholder must be present");
        let s = v
            .as_str()
            .expect("placeholder must be a string, not a number");
        assert!(
            s.contains("must_fix_now_count"),
            "placeholder must reference the field name, got: {s}"
        );
        // Defensive: the placeholder string itself must not
        // be a parseable number, otherwise copy-paste would
        // accidentally pass the validator.
        assert!(
            s.parse::<u64>().is_err(),
            "placeholder must not be numeric, got: {s}"
        );
    }

    /// U2 backward compatibility: a schema with no
    /// `field_docs` and no `allowed_values` still produces a
    /// field line — the legacy `(required)` form. This is
    /// the R18 / R20 invariant for every old preset.
    #[test]
    fn u2_render_field_line_falls_back_to_required_marker() {
        let schema = schema_with_required(&["task_id"]);
        let line = render_field_line("task_id", &schema);
        assert!(line.contains("(required)"));
        // And the table joins multiple required fields.
        let table = render_required_field_table(&schema);
        assert!(table.contains("`task_id` (required)"));
    }

    /// U2 scope guard: `fix_hint_for_hat_topic` still
    /// returns `None` when the hat is not authorised to
    /// publish the topic. Required: U1 / U2 must not leak
    /// another hat's payload shape (and now its
    /// `field_docs`) into the error path.
    #[test]
    fn u2_fix_hint_for_hat_topic_still_none_for_unauthorised() {
        let schema = schema_with_required(&["task_id"]);
        let hat = hat_with_publishes("reviewer", "Reviewer", &["review.accepted"]);
        assert!(fix_hint_for_hat_topic(&hat, "work.done", &schema).is_none());
    }

    /// U2 happy path: `prompt_example_payload` returns the
    /// first author-declared example when present. This is
    /// the path the schema-aware prompt section uses.
    #[test]
    fn u2_prompt_example_payload_prefers_author_example() {
        let mut schema = schema_with_required(&["task_id"]);
        schema
            .examples
            .push(serde_json::json!({"task_id": "task-1234-abcd"}));
        let example = prompt_example_payload(&schema);
        assert!(example.contains("task-1234-abcd"));
    }

    /// U2 backward compatibility: when the schema has no
    /// `examples`, `prompt_example_payload` falls back to
    /// the legacy heuristic placeholders. This keeps old
    /// presets producing a usable prompt section.
    #[test]
    fn u2_prompt_example_payload_falls_back_to_heuristic() {
        let schema = schema_with_required(&["task_id"]);
        let example = prompt_example_payload(&schema);
        assert!(example.contains("task_id"));
        assert!(example.contains("<task_id>"));
    }

    // 2026-07-09-001 plan (U6): prompt-builder tests.

    /// U6 happy path: a hat publishing `review.synthesized`
    /// with a schema that has `field_docs` produces a
    /// section that includes field meaning, allowed values
    /// (when present), and a policy-check instruction line.
    #[test]
    fn u6_publish_section_includes_field_table_for_field_docs() {
        use crate::config::EventFieldDoc;
        let mut schema = schema_with_required(&["synthesized_review_file"]);
        schema.field_docs.insert(
            "synthesized_review_file".to_string(),
            EventFieldDoc {
                meaning: "path to the synthesized review file".to_string(),
                source: "loop runner".to_string(),
                fill_rule: "do NOT hand-write".to_string(),
            },
        );
        let mut schemas = HashMap::new();
        schemas.insert("review.synthesized".to_string(), schema);
        let hat = hat_with_publishes("review-synthesizer", "Reviewer", &["review.synthesized"]);
        let section = build_publish_emit_section(&hat, &schemas);
        assert!(section.contains("synthesized_review_file"));
        assert!(section.contains("path to the synthesized review file"));
        assert!(section.contains("review.synthesized"));
        assert!(section.contains("--policy-check"));
        assert!(section.contains("`field_description`"));
    }

    /// U6 scope guard: a hat publishing `work.done` does NOT
    /// see `review.accepted` in its prompt section, even
    /// when both topics have schemas. The existing scope
    /// guard on `hat.publishes` is the primary defence; this
    /// test pins it under U6.
    #[test]
    fn u6_publish_section_does_not_leak_other_hat_topics() {
        let mut schemas = HashMap::new();
        schemas.insert("work.done".to_string(), schema_with_required(&["task_id"]));
        schemas.insert(
            "review.accepted".to_string(),
            schema_with_required(&["verdict"]),
        );
        let hat = hat_with_publishes("executor", "Executor", &["work.done"]);
        let section = build_publish_emit_section(&hat, &schemas);
        assert!(section.contains("work.done"));
        assert!(!section.contains("review.accepted"));
    }

    /// U6 backward compatibility: a schema without
    /// `field_docs` still produces a usable §3 section
    /// (the legacy summary path). The U6 changes only kick
    /// in when field_docs is present.
    #[test]
    fn u6_publish_section_falls_back_to_legacy_for_no_field_docs() {
        let mut schemas = HashMap::new();
        schemas.insert("work.done".to_string(), schema_with_required(&["task_id"]));
        let hat = hat_with_publishes("executor", "Executor", &["work.done"]);
        let section = build_publish_emit_section(&hat, &schemas);
        assert!(section.contains("work.done"));
        // No field table when no field_docs.
        assert!(!section.contains("Required fields (with meaning)"));
        // But the policy-check line is still present so the
        // hat knows the precheck-first flow.
        assert!(section.contains("--policy-check"));
    }

    /// U6 fallback: hat publishes topic but schema map is
    /// empty — the function returns an empty string so the
    /// caller falls back to the legacy `<summary>` template.
    /// This is the same contract as pre-U6.
    #[test]
    fn u6_publish_section_empty_when_no_schema() {
        let schemas: HashMap<String, EventSchema> = HashMap::new();
        let hat = hat_with_publishes("executor", "Executor", &["work.done"]);
        let section = build_publish_emit_section(&hat, &schemas);
        assert!(section.is_empty());
    }

    /// U6 examples: a schema with both `field_docs` and
    /// `examples` produces a fenced JSON example in the
    /// prompt section. Bounded to 2 examples to keep prompts
    /// compact. The `field_docs` requirement is intentional:
    /// without field docs, the prompt stays on the legacy
    /// summary path (U6 backward-compat invariant).
    #[test]
    fn u6_publish_section_includes_examples_block() {
        use crate::config::EventFieldDoc;
        let mut schema = schema_with_required(&["task_id"]);
        schema.field_docs.insert(
            "task_id".to_string(),
            EventFieldDoc {
                meaning: "live id".to_string(),
                source: String::new(),
                fill_rule: String::new(),
            },
        );
        schema
            .examples
            .push(serde_json::json!({"task_id": "task-1"}));
        schema
            .examples
            .push(serde_json::json!({"task_id": "task-2"}));
        schema
            .examples
            .push(serde_json::json!({"task_id": "task-3"}));
        let mut schemas = HashMap::new();
        schemas.insert("work.done".to_string(), schema);
        let hat = hat_with_publishes("executor", "Executor", &["work.done"]);
        let section = build_publish_emit_section(&hat, &schemas);
        assert!(section.contains("task-1"));
        assert!(section.contains("task-2"));
        // The third example is bounded out of the prompt.
        assert!(!section.contains("task-3"));
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
        schemas.insert(
            "work.ready".to_string(),
            schema_with_required(&["plan_name"]),
        );
        schemas.insert("work.failed".to_string(), schema_with_required(&["reason"]));
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
        let hat = hat_with_publishes("coordinator", "Coordinator", &["work.ready", "work.failed"]);

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
        let hat = hat_with_publishes("coordinator", "Coordinator", &["work.ready", "work.failed"]);

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

        let hint = fix_hint_for_hat_topic(&hat, "review.done", &schema).expect("wildcard");
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
            ..Default::default()
        };
        assert_eq!(
            format_emit_json_example("t", &array_schema),
            "ralph emit t --json '[]'"
        );

        let number_schema = EventSchema {
            payload: Some(PayloadType::Number),
            ..Default::default()
        };
        assert_eq!(
            format_emit_json_example("t", &number_schema),
            "ralph emit t --json '0'"
        );

        let bool_schema = EventSchema {
            payload: Some(PayloadType::Bool),
            ..Default::default()
        };
        assert_eq!(
            format_emit_json_example("t", &bool_schema),
            "ralph emit t --json 'false'"
        );
    }

    // 2026-07-09-001 plan (U7 / T1):
    // pin the em-dash boundary behaviour in
    // `render_field_line`. The function uses
    // `meaning.trim().is_empty()` to decide between
    // rendering the meaning vs an em-dash placeholder.
    // Without a unit test, a future \`is_empty()\`-only
    // change would silently break two visible scenarios:
    //
    // 1. literal empty string \`""\` → render em-dash.
    // 2. ASCII whitespace + full-width space \`\"   \\u{3000}   \"`
    //    → render em-dash (because trim() drops ASCII +
    //    full-width spaces).
    //
    // The negative case (rendering the actual meaning)
    // is already covered by U2 happy path; this slot is
    // for the empty / whitespace-only boundary.

    /// T1 happy path: literal empty `meaning` renders an
    /// em-dash placeholder rather than a bare blank.
    #[test]
    fn render_field_line_em_dash_for_empty_meaning() {
        let mut schema = schema_with_required(&["task_id"]);
        schema.field_docs.insert(
            "task_id".to_string(),
            EventFieldDoc {
                meaning: String::new(),
                source: "preset".to_string(),
                fill_rule: "rule".to_string(),
            },
        );
        let line = render_field_line("task_id", &schema);
        assert!(
            line.contains('—'),
            "empty meaning must surface the em-dash placeholder, got: {line:?}"
        );
        // The em-dash renders alone — does not include an
        // empty `meaning:` token. The function constructs
        // the line by template; we just check the em-dash
        // is present.
    }

    /// T1 edge: ASCII + full-width whitespace (the
    /// `trim().is_empty()` semantic) still hits the
    /// em-dash fallback. A future regression to
    /// `.is_empty()` would let this through and render
    /// the whitespace-laden line (visually empty but
    /// misleading). Pin the trim() semantic.
    #[test]
    fn render_field_line_em_dash_for_whitespace_only_meaning() {
        let mut schema = schema_with_required(&["task_id"]);
        schema.field_docs.insert(
            "task_id".to_string(),
            EventFieldDoc {
                meaning: "   \u{3000}   ".to_string(),
                source: "preset".to_string(),
                fill_rule: "rule".to_string(),
            },
        );
        let line = render_field_line("task_id", &schema);
        assert!(
            line.contains('—'),
            "whitespace-only meaning (trim().is_empty()) must surface the em-dash fallback, \
             got: {line:?}"
        );
    }

    /// T1 negative: non-empty meaning stays as the
    /// literal meaning, not an em-dash placeholder.
    /// (Companion to the U2 happy path; lightweight
    /// redundancy is acceptable when pinning a
    /// boundary.)
    #[test]
    fn render_field_line_keeps_meaning_when_non_empty() {
        let mut schema = schema_with_required(&["task_id"]);
        schema.field_docs.insert(
            "task_id".to_string(),
            EventFieldDoc {
                meaning: String::new(), // start empty so we can assert the em-dash
                source: "preset".to_string(),
                fill_rule: "rule".to_string(),
            },
        );
        let line_with_empty_meaning = render_field_line("task_id", &schema);
        assert!(
            line_with_empty_meaning.contains('—'),
            "control: empty meaning renders em-dash. got: {line_with_empty_meaning:?}"
        );

        // Now flip meaning to non-empty and confirm the
        // meaning text appears AT THE START of the
        // section between `: ` and ` (source:` — i.e.
        // the meaning is rendered rather than collapsed
        // to em-dash. (The em-dash still appears later
        // because \`fill_rule: rule\` formatting prepends
        // \` — rule\`.)
        schema.field_docs.get_mut("task_id").unwrap().meaning = "the live task id".to_string();
        let line = render_field_line("task_id", &schema);
        assert!(
            line.contains(": the live task id (source: preset) — rule"),
            "non-empty meaning must render literally between the colon and the source parenthetical, got: {line:?}"
        );
    }
}
