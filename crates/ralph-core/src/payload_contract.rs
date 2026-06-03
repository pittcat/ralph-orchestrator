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

use std::collections::BTreeSet;
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

        // Pattern 3: Lines containing "payload" with backtick-quoted fields
        if payload_line_regex.is_match(line) {
            for caps in backtick_field_regex.captures_iter(line) {
                if let Some(field_match) = caps.get(1) {
                    let field = field_match.as_str().trim().to_string();
                    // Skip if field is empty or is a phrase rather than a single identifier
                    if !field.is_empty()
                        && !field.contains(' ')
                        && !ignore_set.contains(&field)
                        && seen.insert((hat_id.to_string(), field.clone()))
                    {
                        refs.push(PayloadFieldRef {
                            hat_id: hat_id.to_string(),
                            field,
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
        .filter(|s| !s.is_empty())
        .filter(|s| !s.starts_with('#')) // Skip comments
        .filter(|s| !s.starts_with("```")) // Skip code blocks
        .collect()
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
        let instructions = "payload has `field with spaces` and `normal_field`";
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
}
