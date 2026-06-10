//! Agent doc sync configuration.
//!
//! Controls the managed agent doc blocks synchronization that runs
//! before `ralph run` spawns the backend. When enabled (default),
//! the sync engine injects curated constraint blocks (such as
//! "Command Hang Prevention Rules") into `CLAUDE.md` / `AGENTS.md`
//! in the workspace root.
//!
//! # Escape Hatches
//!
//! - `--no-sync-agent-docs` CLI flag: one-shot disable per run
//! - `RALPH_AGENT_DOC_SYNC=0` env var: one-shot disable per run
//! - `agent_doc_sync.enabled: false` in `ralph.yml`: project-level disable
//!
//! Any of the above being "off" (flag/env present or config disabled)
//! causes the entire sync phase to skip. The three sources are
//! evaluated independently (OR semantics).
//!
//! # Example Configuration
//!
//! ```yaml
//! agent_doc_sync:
//!   enabled: true
//!   on_error: warn
//!   blocks:
//!     - "builtin:hang-prevention"
//! ```

use serde::{Deserialize, Serialize};

/// Error policy for agent doc sync failures.
///
/// - `Warn` (default): log a warning and continue spawning the backend.
/// - `Strict`: log an error and exit the process with code 78 (EX_CONFIG).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnErrorPolicy {
    /// Log warning and continue.
    #[default]
    Warn,
    /// Log error and exit with code 78.
    Strict,
}

impl std::fmt::Display for OnErrorPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Warn => write!(f, "warn"),
            Self::Strict => write!(f, "strict"),
        }
    }
}

impl From<OnErrorPolicy> for crate::agent_doc_sync::OnError {
    fn from(policy: OnErrorPolicy) -> Self {
        match policy {
            OnErrorPolicy::Warn => crate::agent_doc_sync::OnError::Warn,
            OnErrorPolicy::Strict => crate::agent_doc_sync::OnError::Strict,
        }
    }
}

/// Configuration for the agent doc sync feature.
///
/// Sits under `agent_doc_sync` in `ralph.yml`. When omitted, the
/// defaults produce a no-op sync (enabled, warn on error, one
/// builtin block).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDocSyncConfig {
    /// Master switch for agent doc sync.
    ///
    /// When `true` (default), the sync engine runs before backend
    /// spawn. When `false`, the entire sync phase is skipped.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Error policy for sync failures.
    ///
    /// `Warn` (default) logs a warning and continues.
    /// `Strict` exits the process with code 78.
    #[serde(default)]
    pub on_error: OnErrorPolicy,

    /// Block references to inject.
    ///
    /// Each entry is a block identifier. Currently only
    /// `"builtin:hang-prevention"` is supported.
    #[serde(default = "default_blocks")]
    pub blocks: Vec<String>,

    /// Hard timeout (seconds) for the sync phase that runs before
    /// backend spawn. Defaults to 30 seconds.
    ///
    /// The sync runs synchronously (blocking I/O) before the
    /// orchestrator spawns the backend. A stuck file lock, slow disk,
    /// or NFS round-trip could otherwise hang the outer loop. The
    /// runner bounds the sync with a worker thread + `recv_timeout`;
    /// when the timeout fires, a `startup_timeout` recovery envelope
    /// is written and the loop continues (or aborts under
    /// `OnError::Strict`).
    ///
    /// Set to `0` to disable the timeout (legacy behaviour, not
    /// recommended).
    #[serde(default = "default_startup_timeout_secs")]
    pub startup_timeout_secs: u64,
}

impl Default for AgentDocSyncConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            on_error: OnErrorPolicy::default(),
            blocks: default_blocks(),
            startup_timeout_secs: default_startup_timeout_secs(),
        }
    }
}

/// Returns the default `enabled` value (`true`).
fn default_enabled() -> bool {
    true
}

/// Returns the default `blocks` list (`["builtin:hang-prevention"]`).
fn default_blocks() -> Vec<String> {
    vec!["builtin:hang-prevention".to_string()]
}

/// Returns the default `startup_timeout_secs` value (`30` seconds).
///
/// 30s is generous for a sync that writes 2 small markdown files on
/// local disk (typical: <100ms). It guards against pathological cases
/// (held file lock, NFS hang, sandboxed FS quirks) without firing on
/// normal slow CI runners.
fn default_startup_timeout_secs() -> u64 {
    30
}

/// Determines whether sync should be skipped based on env, flag, and config.
///
/// Resolution: `should_skip = env_or_flag || !config.enabled`
/// (any source being "off" triggers skip).
///
/// # Arguments
///
/// * `env_skip` — `true` if `RALPH_AGENT_DOC_SYNC=0` was detected.
/// * `flag_skip` — `true` if `--no-sync-agent-docs` was passed.
/// * `config` — The `AgentDocSyncConfig` from `ralph.yml`.
#[must_use]
pub fn should_skip(env_skip: bool, flag_skip: bool, config: &AgentDocSyncConfig) -> bool {
    env_skip || flag_skip || !config.enabled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_doc_sync_config_default() {
        let cfg = AgentDocSyncConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.on_error, OnErrorPolicy::Warn);
        assert_eq!(cfg.blocks, vec!["builtin:hang-prevention"]);
        assert_eq!(cfg.startup_timeout_secs, 30);
    }

    #[test]
    fn agent_doc_sync_config_startup_timeout_default_30() {
        // D5: the default timeout is 30s (generous for local disk,
        // protective against held locks / NFS).
        let cfg = AgentDocSyncConfig::default();
        assert_eq!(cfg.startup_timeout_secs, 30);
    }

    #[test]
    fn agent_doc_sync_config_startup_timeout_zero_disables() {
        // D5: setting to 0 explicitly disables the timeout (legacy
        // behaviour, not recommended).
        let yaml = "startup_timeout_secs: 0\n";
        let cfg: AgentDocSyncConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.startup_timeout_secs, 0);
    }

    #[test]
    fn agent_doc_sync_config_yaml_round_trip() {
        let yaml = r#"
enabled: false
on_error: strict
blocks:
  - "builtin:hang-prevention"
  - "custom:my-block"
"#;
        let cfg: AgentDocSyncConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.on_error, OnErrorPolicy::Strict);
        assert_eq!(cfg.blocks.len(), 2);
        assert_eq!(cfg.blocks[1], "custom:my-block");

        let reserialized = serde_yaml::to_string(&cfg).unwrap();
        let cfg2: AgentDocSyncConfig = serde_yaml::from_str(&reserialized).unwrap();
        assert_eq!(cfg, cfg2);
    }

    #[test]
    fn agent_doc_sync_config_unknown_fields_ignored() {
        // Unknown fields should be silently ignored (forward compatibility)
        // This matches the existing RalphConfig behavior.
        let yaml = r#"
enabled: true
unknown_future_field: "some value"
"#;
        let result: Result<AgentDocSyncConfig, _> = serde_yaml::from_str(yaml);
        assert!(result.is_ok());
    }

    #[test]
    fn agent_doc_sync_strict_policy_parsing() {
        let yaml = "on_error: strict";
        let cfg: AgentDocSyncConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.on_error, OnErrorPolicy::Strict);
    }

    #[test]
    fn agent_doc_sync_strict_policy_case_sensitive() {
        // serde rename_all = "lowercase" means only lowercase is accepted
        let bad = "on_error: STRICT";
        let result: Result<AgentDocSyncConfig, _> = serde_yaml::from_str(bad);
        assert!(result.is_err());
    }

    #[test]
    fn should_skip_returns_true_when_flag_set() {
        let cfg = AgentDocSyncConfig::default();
        assert!(should_skip(false, true, &cfg));
    }

    #[test]
    fn should_skip_returns_true_when_env_set() {
        let cfg = AgentDocSyncConfig::default();
        assert!(should_skip(true, false, &cfg));
    }

    #[test]
    fn should_skip_returns_true_when_config_disabled() {
        let cfg = AgentDocSyncConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(should_skip(false, false, &cfg));
    }

    #[test]
    fn should_skip_returns_false_when_all_defaults() {
        let cfg = AgentDocSyncConfig::default();
        assert!(!should_skip(false, false, &cfg));
    }

    #[test]
    fn should_skip_or_semantics() {
        let cfg = AgentDocSyncConfig::default();
        // All three off → skip
        assert!(should_skip(true, true, &cfg));
        // Config disabled overrides everything
        let cfg_off = AgentDocSyncConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(should_skip(false, false, &cfg_off));
        // All defaults → no skip
        assert!(!should_skip(false, false, &AgentDocSyncConfig::default()));
    }
}
