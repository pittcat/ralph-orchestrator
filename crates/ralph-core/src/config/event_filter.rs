//! Hat-level event filter configuration.

use serde::{Deserialize, Serialize};

/// Event filter mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EventFilterMode {
    /// Only allow events in the allowlist.
    #[default]
    Allowlist,
}

/// Hat-level event filter configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventFilterConfig {
    /// Whether event filtering is enabled for this hat.
    #[serde(default)]
    pub enabled: bool,

    /// Filter mode (default: allowlist).
    #[serde(default)]
    pub mode: EventFilterMode,

    /// Event topics to allow.
    #[serde(default)]
    pub events: Vec<String>,
}
