//! Workstate configuration types.
//!
//! Workstate is the loop-scoped key-value store for intermediate state
//! (see `crate::workstate`). This block controls whether the current
//! loop's entries are injected into hat activation prompts as a
//! `## WORKSTATE` block.
//!
//! Example configuration:
//! ```yaml
//! workstate:
//!   enabled: true
//!   inject: auto
//!   budget: 0
//! ```

use serde::{Deserialize, Serialize};

use super::memories::InjectMode;

fn default_enabled() -> bool {
    true
}

/// Workstate configuration.
///
/// Defaults enable the feature with auto-injection and no token budget:
/// an empty store renders nothing, so the default is a strict no-op for
/// loops that never write workstate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkstateConfig {
    /// Whether the workstate feature is enabled.
    ///
    /// When false, no `## WORKSTATE` block is injected regardless of
    /// store contents.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// How workstate entries are injected into agent context.
    #[serde(default)]
    pub inject: InjectMode,

    /// Maximum tokens to inject (0 = unlimited).
    ///
    /// When set, the rendered block is truncated at entry boundaries
    /// with a visible truncation marker.
    #[serde(default)]
    pub budget: usize,
}

impl Default for WorkstateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            inject: InjectMode::Auto,
            budget: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Defaults: a missing `workstate:` block (or an empty one) enables
    /// auto-injection with no budget — safe because an empty store
    /// renders nothing.
    #[test]
    fn workstate_config_defaults_enabled_auto_unlimited() {
        let cfg = WorkstateConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.inject, InjectMode::Auto);
        assert_eq!(cfg.budget, 0);

        let parsed: WorkstateConfig = serde_yaml::from_str("{}\n").unwrap();
        assert_eq!(parsed, cfg);
    }

    /// Serde roundtrip: a fully-declared block survives parse and
    /// re-render unchanged.
    #[test]
    fn workstate_config_roundtrips_through_yaml() {
        let yaml = "enabled: false\ninject: manual\nbudget: 500\n";
        let cfg: WorkstateConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.inject, InjectMode::Manual);
        assert_eq!(cfg.budget, 500);

        let rendered = serde_yaml::to_string(&cfg).unwrap();
        let reparsed: WorkstateConfig = serde_yaml::from_str(&rendered).unwrap();
        assert_eq!(reparsed, cfg);
    }

    /// `deny_unknown_fields`: a typo'd key fails at parse time instead
    /// of silently falling back to defaults.
    #[test]
    fn workstate_config_rejects_unknown_fields() {
        let result: Result<WorkstateConfig, _> = serde_yaml::from_str("bugdet: 100\n");
        assert!(
            result.is_err(),
            "unknown workstate key must fail at the serde boundary"
        );
    }
}
