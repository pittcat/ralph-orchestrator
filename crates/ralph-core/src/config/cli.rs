//! CLI backend and TUI configuration.

use serde::{Deserialize, Serialize};

/// CLI backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    /// Backend to use: "claude", "kiro", "gemini", "codex", "amp", "pi", or "custom".
    #[serde(default = "default_backend")]
    pub backend: String,

    /// Command override. Required for "custom" backend.
    /// For named backends, overrides the default binary path.
    pub command: Option<String>,

    /// How to pass prompts: "arg" or "stdin".
    #[serde(default = "default_prompt_mode")]
    pub prompt_mode: String,

    /// Execution mode when --interactive not specified.
    /// Values: "autonomous" (default), "interactive"
    #[serde(default = "default_mode")]
    pub default_mode: String,

    /// Idle timeout in seconds for interactive mode.
    /// Process is terminated after this many seconds of inactivity (no output AND no user input).
    /// Set to 0 to disable idle timeout.
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_secs: u32,

    /// Custom arguments to pass to the CLI command (for backend: "custom").
    /// These are inserted before the prompt argument.
    #[serde(default)]
    pub args: Vec<String>,

    /// Custom prompt flag for arg mode (for backend: "custom").
    /// If None, defaults to "-p" for arg mode.
    #[serde(default)]
    pub prompt_flag: Option<String>,
}

fn default_backend() -> String {
    "claude".to_string()
}

fn default_prompt_mode() -> String {
    "arg".to_string()
}

fn default_mode() -> String {
    "autonomous".to_string()
}

fn default_idle_timeout() -> u32 {
    30 // 30 seconds per spec
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            command: None,
            prompt_mode: default_prompt_mode(),
            default_mode: default_mode(),
            idle_timeout_secs: default_idle_timeout(),
            args: Vec::new(),
            prompt_flag: None,
        }
    }
}

/// TUI configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    /// Prefix key combination (e.g., "ctrl-a", "ctrl-b").
    #[serde(default = "default_prefix_key")]
    pub prefix_key: String,
}

fn default_prefix_key() -> String {
    "ctrl-a".to_string()
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            prefix_key: default_prefix_key(),
        }
    }
}

impl TuiConfig {
    /// Parses the prefix_key string into KeyCode and KeyModifiers.
    /// Returns an error if the format is invalid.
    pub fn parse_prefix(
        &self,
    ) -> Result<(crossterm::event::KeyCode, crossterm::event::KeyModifiers), String> {
        use crossterm::event::{KeyCode, KeyModifiers};

        let parts: Vec<&str> = self.prefix_key.split('-').collect();
        if parts.len() != 2 {
            return Err(format!(
                "Invalid prefix_key format: '{}'. Expected format: 'ctrl-<key>' (e.g., 'ctrl-a', 'ctrl-b')",
                self.prefix_key
            ));
        }

        let modifier = match parts[0].to_lowercase().as_str() {
            "ctrl" => KeyModifiers::CONTROL,
            _ => {
                return Err(format!(
                    "Invalid modifier: '{}'. Only 'ctrl' is supported (e.g., 'ctrl-a')",
                    parts[0]
                ));
            }
        };

        let key_str = parts[1];
        if key_str.len() != 1 {
            return Err(format!(
                "Invalid key: '{}'. Expected a single character (e.g., 'a', 'b')",
                key_str
            ));
        }

        let key_char = key_str.chars().next().unwrap();
        let key_code = KeyCode::Char(key_char);

        Ok((key_code, modifier))
    }
}

#[cfg(test)]
mod tests {
    //! Characterization tests (Unit 1 of plan 2026-06-06-001).
    //!
    //! These pin down the CURRENT `cli.idle_timeout_secs` default and semantics
    //! so that Unit 2 cannot accidentally reuse this 30-second interactive
    //! default as the autonomous / RPC watchdog default (R6). The watchdog for
    //! autonomous / RPC paths must come from a different source (e.g. adapter
    //! `timeout`, which defaults to 300s, or a new explicit field), not from
    //! `cli.idle_timeout_secs`.

    use super::*;

    #[test]
    fn cli_config_default_idle_timeout_is_30_seconds() {
        let config = CliConfig::default();
        assert_eq!(
            config.idle_timeout_secs, 30,
            "CliConfig::default().idle_timeout_secs must remain 30s; \
             this is the interactive-mode default documented in the field"
        );
    }

    #[test]
    fn cli_config_default_idle_timeout_matches_serde_default() {
        // The serde default and the Default impl must agree so that an
        // absent field in YAML/JSON produces the same value as a freshly
        // constructed `CliConfig::default()`.
        assert_eq!(default_idle_timeout(), 30);
        assert_eq!(
            CliConfig::default().idle_timeout_secs,
            default_idle_timeout()
        );
    }

    #[test]
    fn cli_config_zero_idle_timeout_means_disabled_in_documentation() {
        // The doc comment on `idle_timeout_secs` (this file, lines 25-27)
        // says: "Idle timeout in seconds for interactive mode. ...
        // Process is terminated after this many seconds of inactivity
        // (no output AND no user input). Set to 0 to disable idle timeout."
        // This is a contract that callers and tests rely on. Unit 2/3
        // must NOT silently change `0` semantics (R8) — if the autonomous
        // watchdog gets its own field, `0` must continue to mean "disabled"
        // for the field that documents it as such.
        //
        // We deliberately read the source file via `include_str!` (rather
        // than re-stating the doc string as a literal here) so that this
        // test fails if a future edit removes the documented phrases from
        // the actual `idle_timeout_secs` doc comment. This is the regression
        // guard for R6 / R8.
        //
        // The two search needles are assembled at runtime via `format!`
        // from disjoint string fragments so that the full needle never
        // appears as a literal in this file. Otherwise `include_str!`
        // would always find the literal in the test code itself and the
        // assertions would trivially pass. The `    ///` prefix further
        // restricts each match to the field's actual doc-comment block
        // (this file's line comments start with `//`, not `///`).
        let source = include_str!("cli.rs");
        let open_doc_needle = format!(
            "    /// Idle timeout in seconds for {}",
            "interactive mode."
        );
        let disable_doc_needle = format!("    /// Set to {}", "0 to disable");
        assert!(
            source.contains(&open_doc_needle),
            "idle_timeout_secs doc comment must still open with `Idle \
             timeout in seconds for interactive mode.` — if this field is \
             being reused for autonomous / RPC, the doc must explicitly \
             call that out and the R6 contract is being broken."
        );
        assert!(
            source.contains(&disable_doc_needle),
            "idle_timeout_secs doc comment must still document the `0` \
             value as `disabled` — Unit 2/3 may not silently change the \
             semantics of `0` (R8)."
        );
    }
}
