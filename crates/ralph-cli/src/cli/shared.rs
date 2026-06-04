//! Pure-data types shared across the CLI surface.
//!
//! These types were originally declared in `main.rs` and are referenced
//! from every command handler in `commands/`. U4 lifts them here so that
//! `main.rs` and any future command file can import them through one
//! canonical path (`crate::cli::*`). Signatures, derives, and visibility
//! are preserved byte-for-byte.

use clap::ValueEnum;
use std::io::{IsTerminal, stdout};
use std::path::PathBuf;

/// Color output mode for terminal display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum ColorMode {
    /// Automatically detect if stdout is a TTY
    #[default]
    Auto,
    /// Always use colors
    Always,
    /// Never use colors
    Never,
}

impl ColorMode {
    /// Returns true if colors should be used based on mode and terminal detection.
    pub(crate) fn should_use_colors(self) -> bool {
        // NO_COLOR is a de-facto cross-tooling convention and should disable ANSI
        // colors by default, regardless of output mode.
        if std::env::var("NO_COLOR").is_ok() {
            return false;
        }

        match self {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => stdout().is_terminal(),
        }
    }
}

/// Verbosity level for streaming output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Verbosity {
    /// Suppress all streaming output (for CI/scripting)
    Quiet,
    /// Show assistant text and tool invocations (default)
    #[default]
    Normal,
    /// Show everything including tool results and session summary
    Verbose,
}

impl Verbosity {
    /// Resolves verbosity from CLI args, env vars, and config.
    ///
    /// Precedence (highest to lowest):
    /// 1. CLI flags: `--verbose`/`-v` or `--quiet`/`-q`
    /// 2. Environment variables: `RALPH_VERBOSE=1` or `RALPH_QUIET=1`
    /// 3. Config file: (if supported in future)
    /// 4. Default: Normal
    pub(crate) fn resolve(cli_verbose: bool, cli_quiet: bool) -> Self {
        let env_quiet = std::env::var("RALPH_QUIET").is_ok();
        let env_verbose = std::env::var("RALPH_VERBOSE").is_ok();
        Self::resolve_with_env(cli_verbose, cli_quiet, env_quiet, env_verbose)
    }

    #[allow(clippy::fn_params_excessive_bools)]
    fn resolve_with_env(
        cli_verbose: bool,
        cli_quiet: bool,
        env_quiet: bool,
        env_verbose: bool,
    ) -> Self {
        // CLI flags take precedence
        if cli_quiet {
            return Verbosity::Quiet;
        }
        if cli_verbose {
            return Verbosity::Verbose;
        }

        // Environment variables
        if env_quiet {
            return Verbosity::Quiet;
        }
        if env_verbose {
            return Verbosity::Verbose;
        }

        Verbosity::Normal
    }
}

/// Output format for events command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable table format
    #[default]
    Table,
    /// JSON format for programmatic access
    Json,
}

/// Source for core configuration.
#[derive(Debug, Clone)]
pub enum ConfigSource {
    /// Local file path (default behavior)
    File(PathBuf),
    /// Legacy builtin preset source (no longer valid for core config).
    ///
    /// Kept so we can emit actionable migration errors.
    Builtin(String),
    /// Remote URL (e.g., "http://example.com/ralph.core.yml")
    Remote(String),
    /// Config override (e.g., "core.scratchpad=.ralph/feature/scratchpad.md")
    Override { key: String, value: String },
}

impl ConfigSource {
    /// Parse a core config source string into its variant.
    ///
    /// Format:
    /// - `core.field=value` → Override (for core.* fields)
    /// - `builtin:preset-name` → Legacy builtin preset (rejected with migration message)
    /// - `http://...` or `https://...` → Remote URL
    /// - Anything else → File path
    pub(crate) fn parse(s: &str) -> Self {
        // Check for core.* override pattern first (prevents false positives on paths with '=')
        // Only treat as override if it starts with "core." AND contains '='
        if s.starts_with("core.")
            && let Some((key, value)) = s.split_once('=')
        {
            return ConfigSource::Override {
                key: key.to_string(),
                value: value.to_string(),
            };
        }

        if let Some(name) = s.strip_prefix("builtin:") {
            ConfigSource::Builtin(name.to_string())
        } else if s.starts_with("http://") || s.starts_with("https://") {
            ConfigSource::Remote(s.to_string())
        } else {
            ConfigSource::File(PathBuf::from(s))
        }
    }

    /// Convert back to CLI string representation for forwarding to subprocess.
    pub(crate) fn to_cli_string(&self) -> String {
        match self {
            ConfigSource::File(path) => path.display().to_string(),
            ConfigSource::Builtin(name) => format!("builtin:{}", name),
            ConfigSource::Remote(url) => url.clone(),
            ConfigSource::Override { key, value } => format!("{}={}", key, value),
        }
    }
}

/// Source for hat collection configuration.
#[derive(Debug, Clone)]
pub enum HatsSource {
    /// Local file path
    File(PathBuf),
    /// Builtin hat collection name (e.g., "builtin:code-assist")
    Builtin(String),
    /// Remote URL (e.g., "http://example.com/hats.yml")
    Remote(String),
}

impl HatsSource {
    /// Parse a hats source string into its variant.
    pub(crate) fn parse(s: &str) -> Self {
        if let Some(name) = s.strip_prefix("builtin:") {
            HatsSource::Builtin(name.to_string())
        } else if s.starts_with("http://") || s.starts_with("https://") {
            HatsSource::Remote(s.to_string())
        } else {
            HatsSource::File(PathBuf::from(s))
        }
    }

    /// Human-readable source label.
    pub fn label(&self) -> String {
        match self {
            HatsSource::File(path) => path.display().to_string(),
            HatsSource::Builtin(name) => format!("builtin:{}", name),
            HatsSource::Remote(url) => url.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verbosity_cli_quiet() {
        assert_eq!(Verbosity::resolve(false, true), Verbosity::Quiet);
    }

    #[test]
    fn test_verbosity_cli_verbose() {
        assert_eq!(Verbosity::resolve(true, false), Verbosity::Verbose);
    }

    #[test]
    fn test_verbosity_default() {
        assert_eq!(Verbosity::resolve(false, false), Verbosity::Normal);
    }

    #[test]
    fn test_verbosity_env_quiet() {
        assert_eq!(
            Verbosity::resolve_with_env(false, false, true, false),
            Verbosity::Quiet
        );
    }

    #[test]
    fn test_verbosity_env_verbose() {
        assert_eq!(
            Verbosity::resolve_with_env(false, false, false, true),
            Verbosity::Verbose
        );
    }

    #[test]
    fn test_color_mode_should_use_colors() {
        // `NO_COLOR` disables ANSI globally, including `--color always`.
        let expected_always = std::env::var("NO_COLOR").is_err();
        assert_eq!(ColorMode::Always.should_use_colors(), expected_always);
        assert!(!ColorMode::Never.should_use_colors());
    }

    #[test]
    fn test_config_source_parse_builtin() {
        let source = ConfigSource::parse("builtin:code-assist");
        match source {
            ConfigSource::Builtin(name) => assert_eq!(name, "code-assist"),
            _ => panic!("Expected Builtin variant"),
        }
    }

    #[test]
    fn test_hats_source_parse_builtin() {
        let source = HatsSource::parse("builtin:code-assist");
        match source {
            HatsSource::Builtin(name) => assert_eq!(name, "code-assist"),
            _ => panic!("Expected Builtin variant"),
        }
    }

    #[test]
    fn test_hats_source_parse_file() {
        let source = HatsSource::parse("hats/feature.yml");
        match source {
            HatsSource::File(path) => {
                assert_eq!(path, std::path::PathBuf::from("hats/feature.yml"))
            }
            _ => panic!("Expected File variant"),
        }
    }

    #[test]
    fn test_config_source_parse_remote_https() {
        let source = ConfigSource::parse("https://example.com/preset.yml");
        match source {
            ConfigSource::Remote(url) => assert_eq!(url, "https://example.com/preset.yml"),
            _ => panic!("Expected Remote variant"),
        }
    }

    #[test]
    fn test_config_source_parse_remote_http() {
        let source = ConfigSource::parse("http://example.com/preset.yml");
        match source {
            ConfigSource::Remote(url) => assert_eq!(url, "http://example.com/preset.yml"),
            _ => panic!("Expected Remote variant"),
        }
    }

    #[test]
    fn test_config_source_parse_file() {
        let source = ConfigSource::parse("ralph.yml");
        match source {
            ConfigSource::File(path) => assert_eq!(path, std::path::PathBuf::from("ralph.yml")),
            _ => panic!("Expected File variant"),
        }
    }

    #[test]
    fn test_config_source_parse_override_scratchpad() {
        let source = ConfigSource::parse("core.scratchpad=.ralph/feature/scratchpad.md");
        match source {
            ConfigSource::Override { key, value } => {
                assert_eq!(key, "core.scratchpad");
                assert_eq!(value, ".ralph/feature/scratchpad.md");
            }
            _ => panic!("Expected Override variant"),
        }
    }

    #[test]
    fn test_config_source_parse_override_specs_dir() {
        let source = ConfigSource::parse("core.specs_dir=./my-specs/");
        match source {
            ConfigSource::Override { key, value } => {
                assert_eq!(key, "core.specs_dir");
                assert_eq!(value, "./my-specs/");
            }
            _ => panic!("Expected Override variant"),
        }
    }

    #[test]
    fn test_config_source_to_cli_string_roundtrips() {
        // File path
        let source = ConfigSource::File(PathBuf::from("ralph.yml"));
        assert_eq!(source.to_cli_string(), "ralph.yml");

        // Builtin (legacy)
        let source = ConfigSource::Builtin("code-assist".to_string());
        assert_eq!(source.to_cli_string(), "builtin:code-assist");

        // Remote URL
        let source = ConfigSource::Remote("https://example.com/ralph.yml".to_string());
        assert_eq!(source.to_cli_string(), "https://example.com/ralph.yml");

        // Override
        let source = ConfigSource::Override {
            key: "core.scratchpad".to_string(),
            value: ".ralph/feature/scratchpad.md".to_string(),
        };
        assert_eq!(
            source.to_cli_string(),
            "core.scratchpad=.ralph/feature/scratchpad.md"
        );
    }

    #[test]
    fn test_config_source_parse_file_with_equals() {
        // Paths containing '=' but not starting with 'core.' should be treated as files
        let source = ConfigSource::parse("path/with=equals.yml");
        match source {
            ConfigSource::File(path) => {
                assert_eq!(path, std::path::PathBuf::from("path/with=equals.yml"))
            }
            _ => panic!("Expected File variant for path with equals sign"),
        }
    }

    #[test]
    fn test_config_source_parse_core_without_equals() {
        // "core.field" without '=' should be treated as a file path (will fail to load)
        let source = ConfigSource::parse("core.field");
        match source {
            ConfigSource::File(path) => assert_eq!(path, std::path::PathBuf::from("core.field")),
            _ => panic!("Expected File variant for core.field without ="),
        }
    }

    #[test]
    fn test_config_source_parse_non_core_with_equals_is_file() {
        // Non-core.* prefix with '=' should be treated as file path per spec
        let source = ConfigSource::parse("event_loop.max_iterations=5");
        match source {
            ConfigSource::File(path) => {
                assert_eq!(
                    path,
                    std::path::PathBuf::from("event_loop.max_iterations=5")
                )
            }
            _ => panic!("Expected File variant, not Override"),
        }
    }

    #[test]
    fn test_partition_config_sources_separates_overrides() {
        let sources = [
            ConfigSource::File(PathBuf::from("ralph.yml")),
            ConfigSource::Override {
                key: "core.scratchpad".to_string(),
                value: ".custom/scratchpad.md".to_string(),
            },
            ConfigSource::Builtin("tdd".to_string()),
            ConfigSource::Override {
                key: "core.specs_dir".to_string(),
                value: "./specs/".to_string(),
            },
        ];

        let (primary, overrides): (Vec<_>, Vec<_>) = sources
            .iter()
            .partition(|s| !matches!(s, ConfigSource::Override { .. }));

        assert_eq!(primary.len(), 2); // File + Builtin
        assert_eq!(overrides.len(), 2); // Two overrides
        assert!(matches!(primary[0], ConfigSource::File(_)));
        assert!(matches!(primary[1], ConfigSource::Builtin(_)));
    }

    #[test]
    fn test_partition_config_sources_only_overrides() {
        let sources = [ConfigSource::Override {
            key: "core.scratchpad".to_string(),
            value: ".custom/scratchpad.md".to_string(),
        }];

        let (primary, overrides): (Vec<_>, Vec<_>) = sources
            .iter()
            .partition(|s| !matches!(s, ConfigSource::Override { .. }));

        assert_eq!(primary.len(), 0); // No primary sources
        assert_eq!(overrides.len(), 1); // One override
    }
}
