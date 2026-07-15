//! CLI backend and TUI configuration.

use serde::{Deserialize, Serialize};

/// CLI backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    /// Backend to use: "claude", "gemini", "codex", "opencode", "pi", "traecli", or "custom".
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

    /// Watchdog timeout (seconds) for autonomous / RPC / worktree paths
    /// (`ralph run --no-tui`, `ralph run --rpc`, `ralph run --worktree`, etc.).
    /// Resets on every stdout/stderr/stream-json byte the backend emits;
    /// fires `IdleTimeout` and SIGTERMs the child when no activity is observed
    /// for the full duration.
    ///
    /// Semantics (R6 / R8 of plan 2026-06-06-001):
    /// - `None` (default) — fall back to the per-adapter `adapters.<backend>.timeout`
    ///   (typically 300s). This is the recommended source; it already carries
    ///   the right "CLI execution inactivity timeout" semantics.
    /// - `Some(0)` — explicitly DISABLE the autonomous watchdog. Use with care:
    ///   the outer loop will wait forever on a silent, non-exiting backend.
    /// - `Some(N)` where `N > 0` — fire the watchdog after N seconds of inactivity.
    ///
    /// This field is intentionally separate from `idle_timeout_secs` (which
    /// governs interactive mode and defaults to 30s); reusing the 30s default
    /// for autonomous / RPC / worktree would kill any backend that legitimately
    /// needs >30s of silence (e.g. long-running tool calls, model thinking,
    /// network-bound operations).
    #[serde(default)]
    pub autonomous_idle_timeout_secs: Option<u64>,

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
            autonomous_idle_timeout_secs: None,
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
    fn cli_config_default_idle_timeout_is_30_seconds_for_interactive_mode() {
        // The Default impl and the serde default fn must agree so that an
        // absent field in YAML/JSON produces the same value as a freshly
        // constructed `CliConfig::default()`. The value must remain 30s;
        // this is the interactive-mode default documented in the field
        // (see the `idle_timeout_secs` doc comment, R6 of the plan).
        let config = CliConfig::default();
        assert_eq!(
            config.idle_timeout_secs, 30,
            "CliConfig::default().idle_timeout_secs must remain 30s"
        );
        assert_eq!(
            default_idle_timeout(),
            config.idle_timeout_secs,
            "serde default fn and Default impl must agree"
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

    #[test]
    fn cli_config_autonomous_idle_timeout_defaults_to_none() {
        // Plan 2026-06-06-001 R5: when `cli.autonomous_idle_timeout_secs` is
        // absent, the runner must fall back to `adapters.<backend>.timeout`
        // (the existing per-adapter inactivity timeout, default 300s). The
        // `None` default is the wire that connects those two fields.
        let config = CliConfig::default();
        assert!(
            config.autonomous_idle_timeout_secs.is_none(),
            "CliConfig::default().autonomous_idle_timeout_secs must be None \
             (fall back to adapters.<backend>.timeout), got {:?}",
            config.autonomous_idle_timeout_secs
        );
    }

    #[test]
    fn cli_config_autonomous_idle_timeout_parses_explicit_value() {
        // Both `Some(0)` (explicit disable) and `Some(N)` (override) must
        // round-trip through serde without losing the user intent. The
        // presence-vs-meaning distinction is what makes "0 = disabled" safe
        // to add on top of the inherited default (R6 / R8).
        let yaml = "autonomous_idle_timeout_secs: 0\n";
        let parsed: CliConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.autonomous_idle_timeout_secs, Some(0));

        let yaml = "autonomous_idle_timeout_secs: 600\n";
        let parsed: CliConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.autonomous_idle_timeout_secs, Some(600));
    }

    #[test]
    fn cli_config_autonomous_idle_timeout_absent_in_yaml_yields_none() {
        // Backwards compat: a YAML that does NOT mention
        // `autonomous_idle_timeout_secs` must keep its `None` default and
        // let `RalphConfig::autonomous_idle_timeout_secs(backend)` fall
        // back to the per-adapter timeout. Any pre-Unit-2 config keeps
        // working unchanged.
        let yaml = "backend: claude\nidle_timeout_secs: 30\n";
        let parsed: CliConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(parsed.autonomous_idle_timeout_secs.is_none());
        assert_eq!(parsed.backend, "claude");
        assert_eq!(parsed.idle_timeout_secs, 30);
    }
}
