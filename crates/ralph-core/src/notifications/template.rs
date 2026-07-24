//! Lightweight `{{ident}}` template rendering for webhook bodies (plan KTD-5).
//!
//! This is a deliberately minimal substitution engine: it replaces each
//! `{{ident}}` placeholder with the JSON-string-escaped value of the matching
//! variable. There are no conditionals, no loops, and no external template
//! engine (no handlebars/tera) — just enough to keep a Feishu `text` JSON body
//! valid when a variable value contains quotes or newlines.
//!
//! # Semantics
//!
//! - `ident` matches `[A-Za-z_][A-Za-z0-9_]*`.
//! - A template containing no `{{` renders to itself unchanged (identity).
//! - Each substituted value is JSON-string-escaped (see [`json_string_escape`])
//!   so the surrounding JSON stays well-formed.
//! - A `{{name}}` whose `name` is absent from `vars` is an
//!   [`RenderError::UnknownVariable`] error (no panic).
//! - A malformed placeholder — unterminated `{{`, empty `{{}}`, or an invalid
//!   identifier such as `{{foo bar}}` — is a [`RenderError::MalformedPlaceholder`]
//!   error (no panic).

use std::collections::HashMap;
use std::fmt;

/// Errors produced by [`render`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    /// A `{{name}}` placeholder referenced a variable not present in `vars`.
    UnknownVariable(String),
    /// The template contained a syntactically invalid placeholder.
    MalformedPlaceholder(String),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::UnknownVariable(name) => {
                write!(f, "unknown template variable `{}`", name)
            }
            RenderError::MalformedPlaceholder(p) => {
                write!(f, "malformed template placeholder `{}`", p)
            }
        }
    }
}

impl std::error::Error for RenderError {}

/// Returns `true` if `s` is a valid placeholder identifier:
/// `[A-Za-z_][A-Za-z0-9_]*`.
fn is_valid_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Renders `template` by substituting each `{{ident}}` placeholder with the
/// JSON-string-escaped value of `vars[ident]`.
///
/// See the [module docs](self) for the full semantics. This function never
/// panics on malformed input; it returns a [`RenderError`] instead.
pub fn render(template: &str, vars: &HashMap<String, String>) -> Result<String, RenderError> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        // Copy literal text preceding the placeholder verbatim. `open` is the
        // byte offset of an ASCII `{`, so the slice boundary is char-safe.
        out.push_str(&rest[..open]);
        let after_open = &rest[open + 2..];
        let close = after_open
            .find("}}")
            .ok_or_else(|| RenderError::MalformedPlaceholder(rest[open..].to_string()))?;
        let name = &after_open[..close];
        if !is_valid_ident(name) {
            return Err(RenderError::MalformedPlaceholder(name.to_string()));
        }
        let value = vars
            .get(name)
            .ok_or_else(|| RenderError::UnknownVariable(name.to_string()))?;
        out.push_str(&json_string_escape(value));
        rest = &after_open[close + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Escapes `s` for safe embedding inside a JSON string literal (without the
/// surrounding quotes).
///
/// Escapes `"`, `\`, the standard control-character shorthands (`\b`, `\t`,
/// `\n`, `\f`, `\r`), and every other ASCII control character (`< 0x20`) as
/// `\u00XX`. Non-control characters (including multi-byte UTF-8) are passed
/// through unchanged.
pub fn json_string_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{0C}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // ── 1. Feishu text template renders to valid JSON ─────────────────────────

    #[test]
    fn notifications_template_feishu_renders_valid_json() {
        let template = r#"{"msg_type":"text","content":{"text":"Ralph {{status}} {{loop_id}} ({{termination_reason}})"}}"#;
        let v = vars(&[
            ("status", "success"),
            ("loop_id", "loop-123"),
            ("termination_reason", "CompletionPromise"),
        ]);
        let out = render(template, &v).expect("render should succeed");
        let parsed: serde_json::Value =
            serde_json::from_str(&out).expect("output must be valid JSON");
        let text = parsed["content"]["text"]
            .as_str()
            .expect("text is a string");
        assert_eq!(text, "Ralph success loop-123 (CompletionPromise)");
        assert!(text.contains("Ralph success loop-123 (CompletionPromise)"));
    }

    // ── 2. Unknown variable is an error ───────────────────────────────────────

    #[test]
    fn notifications_template_unknown_variable_errors() {
        let empty: HashMap<String, String> = HashMap::new();
        let err = render("hi {{unknown}}", &empty).expect_err("must be an error");
        match err {
            RenderError::UnknownVariable(name) => assert_eq!(name, "unknown"),
            other => panic!("expected UnknownVariable, got {:?}", other),
        }
    }

    #[test]
    fn notifications_template_unknown_variable_with_partial_vars_errors() {
        let v = vars(&[("status", "success")]);
        let err = render("{{status}} {{missing}}", &v).expect_err("must be an error");
        assert!(matches!(err, RenderError::UnknownVariable(ref n) if n == "missing"));
    }

    // ── 3. Value with quote + newline round-trips through JSON ────────────────

    #[test]
    fn notifications_template_value_with_quote_and_newline_round_trips() {
        let original = "a\"b\nc";
        let v = vars(&[("name", original)]);
        let out = render(r#"{"t":"{{name}}"}"#, &v).expect("render should succeed");
        let parsed: serde_json::Value =
            serde_json::from_str(&out).expect("output must be valid JSON");
        assert_eq!(parsed["t"].as_str().expect("t is a string"), original);
    }

    // ── 4. Identity: no `{{` renders unchanged ────────────────────────────────

    #[test]
    fn notifications_template_identity_no_placeholder() {
        let template = r#"plain text {"a":1}"#;
        let empty: HashMap<String, String> = HashMap::new();
        let out = render(template, &empty).expect("render should succeed");
        assert_eq!(out, template);
    }

    // ── 5. Malformed placeholders are errors (no panic) ───────────────────────

    #[test]
    fn notifications_template_malformed_unclosed_placeholder() {
        let empty: HashMap<String, String> = HashMap::new();
        let err = render("x {{ oops", &empty).expect_err("must be an error");
        assert!(matches!(err, RenderError::MalformedPlaceholder(_)));
    }

    #[test]
    fn notifications_template_malformed_empty_placeholder() {
        let empty: HashMap<String, String> = HashMap::new();
        let err = render("{{}}", &empty).expect_err("must be an error");
        assert!(matches!(err, RenderError::MalformedPlaceholder(_)));
    }

    #[test]
    fn notifications_template_malformed_invalid_ident_with_space() {
        let v = vars(&[("foo", "1"), ("bar", "2")]);
        let err = render("{{foo bar}}", &v).expect_err("must be an error");
        assert!(matches!(err, RenderError::MalformedPlaceholder(_)));
    }

    // ── 6. json_string_escape unit test ───────────────────────────────────────

    #[test]
    fn notifications_template_json_string_escape_basic() {
        // input:  a"b\c<newline>
        // output: a\"b\\c\n   (backslash-n is the two-char shorthand)
        assert_eq!(json_string_escape("a\"b\\c\n"), "a\\\"b\\\\c\\n");
    }

    #[test]
    fn notifications_template_json_string_escape_other_control_chars() {
        // A non-named control char (0x01) must become .
        assert_eq!(json_string_escape("\u{1}"), "\\u0001");
        // Tab and carriage return use the named shorthands.
        assert_eq!(json_string_escape("\t\r"), "\\t\\r");
        // Plain text passes through unchanged.
        assert_eq!(json_string_escape("hello"), "hello");
    }
}
