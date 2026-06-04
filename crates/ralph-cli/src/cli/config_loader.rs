//! Config discovery, loading, and CLI override application.
//!
//! These functions are the sync (non-async) configuration pipeline used by
//! `resume_command`, `clean_command`, `events_command`, and the policy /
//! provenance paths in `emit_command`. The async pipeline that supports
//! remote URLs lives in `preflight::load_config_for_preflight`.
//!
//! U4 lifts these from `main.rs` so the call sites can stay tight while
//! `main.rs` itself focuses on `Cli` / `Commands` dispatch. Visibility
//! (`pub(crate)` for cross-command helpers, private for single-use fns)
//! is preserved byte-for-byte.

use anyhow::Context;
use ralph_core::RalphConfig;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use super::shared::ConfigSource;

/// Returns the default config source path.
///
/// `RALPH_CONFIG` (if set) is used before the hardcoded fallback to `ralph.yml`.
pub(crate) fn default_config_path() -> PathBuf {
    if let Ok(value) = std::env::var("RALPH_CONFIG")
        && !value.trim().is_empty()
    {
        return PathBuf::from(value);
    }

    PathBuf::from("ralph.yml")
}

pub(crate) fn resolve_workspace_root(root: Option<&PathBuf>) -> PathBuf {
    if let Some(root) = root {
        return root.clone();
    }

    if let Ok(value) = std::env::var("RALPH_WORKSPACE_ROOT")
        && !value.trim().is_empty()
    {
        return PathBuf::from(value);
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    discover_workspace_root(&cwd).unwrap_or(cwd)
}

pub(crate) fn resolve_path_from_workspace(
    path: impl AsRef<Path>,
    root: Option<&PathBuf>,
) -> PathBuf {
    resolve_workspace_root(root).join(path)
}

pub(crate) fn urgent_steer_path_from_workspace(root: Option<&PathBuf>) -> PathBuf {
    resolve_workspace_root(root).join(".ralph/urgent-steer.json")
}

pub(crate) fn discover_workspace_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().find_map(|dir| {
        let has_ralph = dir.join(".ralph").is_dir();
        let has_git = dir.join(".git").exists();
        if has_ralph || has_git {
            Some(dir.to_path_buf())
        } else {
            None
        }
    })
}

/// Known core fields that can be overridden via CLI.
const KNOWN_CORE_FIELDS: &[&str] = &["scratchpad", "specs_dir"];

/// Applies CLI config overrides to the loaded configuration.
///
/// Overrides are in the format `core.field=value` and take precedence
/// over values from the config file.
pub(crate) fn apply_config_overrides(
    config: &mut RalphConfig,
    sources: &[ConfigSource],
) -> anyhow::Result<()> {
    for source in sources {
        if let ConfigSource::Override { key, value } = source {
            match key.as_str() {
                "core.scratchpad" => {
                    config.core.scratchpad.path = value.clone();
                }
                "core.specs_dir" => {
                    config.core.specs_dir = value.clone();
                }
                other => {
                    // Note: with core.* prefix requirement in parse(), this branch
                    // only handles unknown core.* fields
                    let field = other.strip_prefix("core.").unwrap_or(other);
                    warn!(
                        "Unknown core field '{}'. Known fields: {}",
                        field,
                        KNOWN_CORE_FIELDS.join(", ")
                    );
                }
            }
        }
    }
    Ok(())
}

/// Ensures the scratchpad's parent directory exists, creating it if needed.
pub(crate) fn ensure_scratchpad_directory(config: &RalphConfig) -> anyhow::Result<()> {
    let scratchpad_path = config.core.resolve_path(&config.core.scratchpad.path);
    if let Some(parent) = scratchpad_path.parent()
        && !parent.exists()
    {
        info!("Creating scratchpad directory: {}", parent.display());
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Loads configuration from file sources with override support.
///
/// This is the common sync path used by resume_command and clean_command.
/// For the full async path (including Remote URLs), see run_command.
///
/// Returns the loaded config with overrides applied and workspace_root set.
pub(crate) fn load_config_with_overrides(
    config_sources: &[ConfigSource],
) -> anyhow::Result<RalphConfig> {
    let (primary_sources, overrides) =
        crate::config_resolution::split_config_sources(config_sources);
    if primary_sources.len() > 1 {
        warn!("Multiple config sources specified, using first one. Others ignored.");
    }

    let (primary_value, primary_label, primary_uses_defaults) = match primary_sources.first() {
        Some(ConfigSource::File(path)) => {
            if path.exists() {
                let label = path.display().to_string();
                let content = std::fs::read_to_string(path)
                    .with_context(|| format!("Failed to load config from {}", label))?;
                let value = crate::config_resolution::parse_yaml_value(&content, &label)?;
                (Some(value), label, false)
            } else {
                warn!("Config file {:?} not found, using defaults", path);
                (None, path.display().to_string(), false)
            }
        }
        Some(ConfigSource::Builtin(name)) => {
            anyhow::bail!(
                "`-c builtin:{name}` is no longer supported.\n\nBuiltin presets are now hat collections.\nUse:\n  ralph run -c ralph.yml -H builtin:{name}"
            );
        }
        Some(ConfigSource::Remote(url)) => {
            anyhow::bail!(
                "Remote core config sources are not supported for this command: {}",
                url
            );
        }
        Some(ConfigSource::Override { .. }) => unreachable!("Overrides are partitioned out"),
        None => {
            let default_path = default_config_path();
            if default_path.exists() {
                let label = default_path.display().to_string();
                let content = std::fs::read_to_string(&default_path)
                    .with_context(|| format!("Failed to load config from {}", label))?;
                let value = crate::config_resolution::parse_yaml_value(&content, &label)?;
                (Some(value), label, false)
            } else {
                warn!(
                    "Config file {} not found, using defaults",
                    default_path.display()
                );
                (None, default_path.display().to_string(), true)
            }
        }
    };

    let user_layer = crate::config_resolution::load_optional_user_config_value()?;
    let mut merged_value = crate::config_resolution::default_core_value()?;
    if let Some((user_value, _)) = &user_layer {
        merged_value =
            crate::config_resolution::merge_yaml_values(merged_value, user_value.clone())?;
    }
    if let Some(primary_value) = primary_value {
        merged_value = crate::config_resolution::merge_yaml_values(merged_value, primary_value)?;
    }

    let merged_label = crate::config_resolution::compose_core_label(
        user_layer.as_ref().map(|(_, label)| label.as_str()),
        &primary_label,
        primary_uses_defaults,
    );

    let mut config: RalphConfig = serde_yaml::from_value(merged_value)
        .with_context(|| format!("Failed to parse merged core config from {}", merged_label))?;

    config.normalize();

    // Set workspace_root to current directory
    config.core.workspace_root =
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    // Record the primary config file path for template substitution in hooks.
    config.config_path = primary_sources
        .iter()
        .find(|s| matches!(s, ConfigSource::File(_)))
        .and_then(|s| match s {
            ConfigSource::File(path) => Some(path.clone()),
            _ => None,
        });

    // Resolve external schema files referenced in event_policy.schema_file.
    // Base path is the config file's directory, or workspace_root if no config file.
    let schema_base_path = config
        .config_path
        .as_ref()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| config.core.workspace_root.clone());
    if let Err(e) = config.resolve_schema_files(&schema_base_path) {
        anyhow::bail!("Failed to resolve schema files: {}", e);
    }

    // Apply CLI config overrides
    apply_config_overrides(&mut config, &overrides)?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_core::RalphConfig;
    use tempfile::TempDir;

    #[test]
    fn test_resolve_workspace_root_discovers_ancestor_ralph_dir() {
        let temp_dir = TempDir::new().expect("temp dir");
        std::fs::create_dir_all(temp_dir.path().join(".ralph")).expect("ralph dir");
        let nested = temp_dir.path().join("a/b/c");
        std::fs::create_dir_all(&nested).expect("nested dir");

        assert_eq!(
            discover_workspace_root(&nested),
            Some(temp_dir.path().to_path_buf())
        );
    }

    #[test]
    fn test_apply_config_overrides_scratchpad() {
        let mut config = RalphConfig::default();
        let sources = vec![ConfigSource::Override {
            key: "core.scratchpad".to_string(),
            value: ".custom/scratch.md".to_string(),
        }];
        apply_config_overrides(&mut config, &sources).unwrap();
        assert_eq!(config.core.scratchpad.path, ".custom/scratch.md");
    }

    #[test]
    fn test_apply_config_overrides_specs_dir() {
        let mut config = RalphConfig::default();
        let sources = vec![ConfigSource::Override {
            key: "core.specs_dir".to_string(),
            value: "./specifications/".to_string(),
        }];
        apply_config_overrides(&mut config, &sources).unwrap();
        assert_eq!(config.core.specs_dir, "./specifications/");
    }

    #[test]
    fn test_apply_config_overrides_multiple() {
        let mut config = RalphConfig::default();
        let sources = vec![
            ConfigSource::Override {
                key: "core.scratchpad".to_string(),
                value: ".custom/scratch.md".to_string(),
            },
            ConfigSource::Override {
                key: "core.specs_dir".to_string(),
                value: "./my-specs/".to_string(),
            },
        ];
        apply_config_overrides(&mut config, &sources).unwrap();
        assert_eq!(config.core.scratchpad.path, ".custom/scratch.md");
        assert_eq!(config.core.specs_dir, "./my-specs/");
    }

    #[test]
    fn test_apply_config_overrides_unknown_field() {
        // Unknown core.* fields should warn but not error
        let mut config = RalphConfig::default();
        let original_scratchpad = config.core.scratchpad.path.clone();
        let sources = vec![ConfigSource::Override {
            key: "core.unknown_field".to_string(),
            value: "some_value".to_string(),
        }];
        // Should not error
        apply_config_overrides(&mut config, &sources).unwrap();
        // Original values should be unchanged
        assert_eq!(config.core.scratchpad.path, original_scratchpad);
    }

    #[test]
    fn test_ensure_scratchpad_directory_creates_nested() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = RalphConfig::default();
        config.core.workspace_root = temp_dir.path().to_path_buf();

        config.core.scratchpad.path = "a/b/c/scratchpad.md".to_string();

        let result = ensure_scratchpad_directory(&config);
        assert!(result.is_ok());

        // Verify directory was created
        let expected_dir = temp_dir.path().join("a/b/c");
        assert!(expected_dir.exists());
    }

    #[test]
    fn test_ensure_scratchpad_directory_noop_when_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = RalphConfig::default();
        config.core.workspace_root = temp_dir.path().to_path_buf();

        // Pre-create the directory
        let subdir = temp_dir.path().join("existing");
        std::fs::create_dir_all(&subdir).unwrap();
        config.core.scratchpad.path = "existing/scratchpad.md".to_string();

        // Should succeed without error (no-op)
        let result = ensure_scratchpad_directory(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_load_config_from_file_with_overrides() {
        // Integration test: load a real config file and apply overrides
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("test.yml");
        std::fs::write(
            &config_path,
            r"
cli:
  backend: claude
core:
  scratchpad: .agent/scratchpad.md
  specs_dir: ./specs/
",
        )
        .unwrap();

        let mut config = RalphConfig::from_file(&config_path).unwrap();
        assert_eq!(config.core.scratchpad.path, ".agent/scratchpad.md");

        // Apply override
        let overrides = vec![ConfigSource::Override {
            key: "core.scratchpad".to_string(),
            value: ".custom/scratch.md".to_string(),
        }];
        apply_config_overrides(&mut config, &overrides).unwrap();

        assert_eq!(config.core.scratchpad.path, ".custom/scratch.md");
        assert_eq!(config.core.specs_dir, "./specs/"); // Unchanged
    }
}
