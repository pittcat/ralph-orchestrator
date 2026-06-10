//! Core configuration types: scratchpad, core paths, workspace.

use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};

use super::event_projection::EventProjectionConfig;
use super::preflight_ext::PreflightExtensionsConfig;
use super::state_files::StateFilesConfig;

/// Scratchpad configuration with enabled flag and path.
///
/// Supports both plain string (legacy) and structured object in YAML:
/// ```yaml
/// # Legacy (plain string) — treated as { enabled: true, path: "..." }
/// core:
///   scratchpad: ".ralph/agent/scratchpad.md"
///
/// # Structured object
/// core:
///   scratchpad:
///     enabled: true
///     path: .ralph/agent/scratchpad.md
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScratchpadConfig {
    #[serde(default = "scratchpad_enabled_default")]
    pub enabled: bool,

    #[serde(default = "default_scratchpad_path")]
    pub path: String,
}

fn scratchpad_enabled_default() -> bool {
    true
}

fn default_scratchpad_path() -> String {
    ".ralph/agent/scratchpad.md".to_string()
}

impl Default for ScratchpadConfig {
    fn default() -> Self {
        Self {
            enabled: scratchpad_enabled_default(),
            path: default_scratchpad_path(),
        }
    }
}

impl ScratchpadConfig {
    /// Resolves the effective scratchpad config for a hat run.
    ///
    /// Resolution order: hat override → global core config → defaults.
    pub fn resolve(
        hat_config: Option<&ScratchpadConfig>,
        global: &ScratchpadConfig,
    ) -> ScratchpadConfig {
        match hat_config {
            Some(override_config) => override_config.clone(),
            None => global.clone(),
        }
    }
}

/// Custom deserializer that accepts both a plain string and a structured object.
///
/// - Plain string → `ScratchpadConfig { enabled: true, path: <string> }`
/// - Map → normal `ScratchpadConfig` deserialization
pub fn deserialize_scratchpad_config<'de, D>(deserializer: D) -> Result<ScratchpadConfig, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de;

    struct ScratchpadConfigVisitor;

    impl<'de> de::Visitor<'de> for ScratchpadConfigVisitor {
        type Value = ScratchpadConfig;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or a scratchpad config object")
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<ScratchpadConfig, E> {
            Ok(ScratchpadConfig {
                enabled: true,
                path: value.to_string(),
            })
        }

        fn visit_map<M: de::MapAccess<'de>>(self, map: M) -> Result<ScratchpadConfig, M::Error> {
            Deserialize::deserialize(de::value::MapAccessDeserializer::new(map))
        }
    }

    deserializer.deserialize_any(ScratchpadConfigVisitor)
}

/// Custom deserializer for optional scratchpad config on hats.
///
/// Handles: absent (None), plain string, or structured object.
pub fn deserialize_optional_scratchpad_config<'de, D>(
    deserializer: D,
) -> Result<Option<ScratchpadConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de;

    struct OptionalScratchpadConfigVisitor;

    impl<'de> de::Visitor<'de> for OptionalScratchpadConfigVisitor {
        type Value = Option<ScratchpadConfig>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("null, a string, or a scratchpad config object")
        }

        fn visit_none<E: de::Error>(self) -> Result<Option<ScratchpadConfig>, E> {
            Ok(None)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Option<ScratchpadConfig>, E> {
            Ok(None)
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Option<ScratchpadConfig>, E> {
            Ok(Some(ScratchpadConfig {
                enabled: true,
                path: value.to_string(),
            }))
        }

        fn visit_map<M: de::MapAccess<'de>>(
            self,
            map: M,
        ) -> Result<Option<ScratchpadConfig>, M::Error> {
            let config: ScratchpadConfig =
                Deserialize::deserialize(de::value::MapAccessDeserializer::new(map))?;
            Ok(Some(config))
        }
    }

    deserializer.deserialize_any(OptionalScratchpadConfigVisitor)
}

/// Core paths and settings shared across all hats.
///
/// Per spec: "Core behaviors (always injected, can customize paths)"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreConfig {
    /// Scratchpad configuration (path and enabled flag).
    /// Accepts both plain string (legacy) and structured object.
    #[serde(default, deserialize_with = "deserialize_scratchpad_config")]
    pub scratchpad: ScratchpadConfig,

    /// Path to the specs directory (source of truth for requirements).
    #[serde(default = "default_specs_dir")]
    pub specs_dir: String,

    /// Guardrails injected into every prompt (core behaviors).
    ///
    /// Per spec: These are always present regardless of hat.
    #[serde(default = "default_guardrails")]
    pub guardrails: Vec<String>,

    /// Event projection configuration.
    ///
    /// When enabled, matching events are projected to target files.
    #[serde(default)]
    pub event_projection: Option<EventProjectionConfig>,

    /// State file injection configuration.
    ///
    /// When enabled, specified files are read and injected into the prompt preamble.
    #[serde(default)]
    pub state_files: Option<StateFilesConfig>,

    /// Preflight extension hooks configuration.
    ///
    /// When enabled, custom hooks run before or after native preflight checks.
    #[serde(default)]
    pub preflight_extensions: Option<PreflightExtensionsConfig>,

    /// Root directory for workspace-relative paths (.ralph/, specs, etc.).
    ///
    /// All relative paths (scratchpad, specs_dir, memories) are resolved relative
    /// to this directory. Defaults to the current working directory.
    ///
    /// This is especially important for E2E tests that run in isolated workspaces.
    #[serde(skip)]
    pub workspace_root: std::path::PathBuf,

    /// Enable invariant assertion checks (U3, defense-in-depth).
    ///
    /// When true, the event loop checks for impersonation/source violations
    /// on each iteration and records findings to diagnostics and LoopState.
    /// Default is false (no runtime overhead).
    #[serde(default)]
    pub invariant_assertions: bool,
}

fn default_specs_dir() -> String {
    ".ralph/specs/".to_string()
}

fn default_guardrails() -> Vec<String> {
    vec![
        "Fresh context each iteration - scratchpad is memory".to_string(),
        "Don't assume 'not implemented' - search first".to_string(),
        "Backpressure is law - tests/typecheck/lint/audit must pass".to_string(),
        "When behavior is runnable or user-facing, exercise the real app with the strongest available harness (Playwright, tmux, real CLI/API) and try at least one adversarial path before reporting done".to_string(),
        "Confidence protocol: score decisions 0-100. >80 proceed autonomously; 50-80 proceed + document in .ralph/agent/decisions.md; <50 choose safe default + document".to_string(),
        "Commit atomically - one logical change per commit, capture the why".to_string(),
    ]
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            scratchpad: ScratchpadConfig::default(),
            specs_dir: default_specs_dir(),
            guardrails: default_guardrails(),
            event_projection: None,
            state_files: None,
            preflight_extensions: None,
            workspace_root: std::env::var("RALPH_WORKSPACE_ROOT")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
                }),
            invariant_assertions: false,
        }
    }
}

impl CoreConfig {
    /// Sets the workspace root for resolving relative paths.
    ///
    /// This is used by E2E tests to point to their isolated test workspace.
    pub fn with_workspace_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.workspace_root = root.into();
        self
    }

    /// Resolves a relative path against the workspace root.
    ///
    /// If the path is already absolute, it is returned as-is.
    /// Otherwise, it is joined with the workspace root.
    pub fn resolve_path(&self, relative: &str) -> PathBuf {
        let path = std::path::Path::new(relative);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace_root.join(path)
        }
    }
}
