//! Shared backend metadata for CLI validation and user-facing error messages.

/// Supported LLM backend identifiers in ralph-cli.
pub const VALID_BACKENDS: &[&str] = &[
    "claude", "gemini", "codex", "opencode", "pi", "traecli", "custom",
];

/// Human-readable list for CLI messages and docs.
pub const VALID_BACKENDS_LABEL: &str = "claude, gemini, codex, opencode, pi, traecli, custom";

/// Returns `true` if the backend identifier is known.
pub fn is_known_backend(name: &str) -> bool {
    VALID_BACKENDS.contains(&name)
}

/// Formats the canonical unknown-backend error with all supported backends.
pub fn unknown_backend_message(name: &str) -> String {
    format!(
        "Unknown backend: {}\n\nValid backends: {}",
        name, VALID_BACKENDS_LABEL
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_backends_does_not_contain_copilot() {
        assert!(
            !VALID_BACKENDS.contains(&"copilot"),
            "VALID_BACKENDS must not contain deleted backend 'copilot'"
        );
        assert!(
            !VALID_BACKENDS_LABEL.contains("copilot"),
            "VALID_BACKENDS_LABEL must not contain deleted backend 'copilot'"
        );
    }

    #[test]
    fn test_valid_backends_does_not_contain_amp() {
        assert!(
            !VALID_BACKENDS.contains(&"amp"),
            "VALID_BACKENDS must not contain deleted backend 'amp'"
        );
        assert!(
            !VALID_BACKENDS_LABEL.contains("amp"),
            "VALID_BACKENDS_LABEL must not contain deleted backend 'amp'"
        );
    }

    #[test]
    fn test_valid_backends_does_not_contain_roo() {
        assert!(
            !VALID_BACKENDS.contains(&"roo"),
            "VALID_BACKENDS must not contain deleted backend 'roo'"
        );
        assert!(
            !VALID_BACKENDS_LABEL.contains("roo"),
            "VALID_BACKENDS_LABEL must not contain deleted backend 'roo'"
        );
    }

    #[test]
    fn test_valid_backends_does_not_contain_kiro_and_kiro_acp() {
        assert!(
            !VALID_BACKENDS.contains(&"kiro"),
            "VALID_BACKENDS must not contain deleted backend 'kiro'"
        );
        assert!(
            !VALID_BACKENDS.contains(&"kiro-acp"),
            "VALID_BACKENDS must not contain deleted backend 'kiro-acp'"
        );
        assert!(
            !VALID_BACKENDS_LABEL.contains("kiro"),
            "VALID_BACKENDS_LABEL must not contain deleted backend 'kiro'"
        );
    }
}
