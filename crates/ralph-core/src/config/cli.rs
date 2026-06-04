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
