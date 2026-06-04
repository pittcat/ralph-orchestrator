//! Event projection configuration.

use serde::{Deserialize, Serialize};

/// Event projection mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProjectionMode {
    /// Append events to the target file.
    #[default]
    Append,
}

fn default_projection_mode() -> ProjectionMode {
    ProjectionMode::Append
}

/// A single event projection rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectionRule {
    /// Human-readable name for this rule.
    pub name: String,

    /// Event topics that trigger this rule.
    #[serde(default)]
    pub trigger_events: Vec<String>,

    /// Fields to extract from matching events.
    #[serde(default)]
    pub fields: Vec<String>,

    /// Target file path for the projection output.
    pub target_file: String,

    /// Projection mode (default: append).
    #[serde(default = "default_projection_mode")]
    pub mode: ProjectionMode,
}

/// Event projection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventProjectionConfig {
    /// Whether event projection is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Projection rules.
    #[serde(default)]
    pub rules: Vec<ProjectionRule>,
}
