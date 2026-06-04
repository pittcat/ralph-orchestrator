//! State file injection configuration.

use serde::{Deserialize, Serialize};

/// State file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum StateFileFormat {
    /// JSON format.
    #[default]
    Json,
    /// JSON Lines format.
    Jsonl,
}

fn default_state_file_format() -> StateFileFormat {
    StateFileFormat::Json
}

/// A single state file entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateFileEntry {
    /// Path to the state file.
    pub path: String,

    /// Format of the state file.
    #[serde(default = "default_state_file_format")]
    pub format: StateFileFormat,

    /// Optional character budget for truncation.
    #[serde(default)]
    pub char_budget: Option<usize>,

    /// Optional number of trailing lines to read.
    #[serde(default)]
    pub tail_lines: Option<usize>,
}

/// State file injection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateFilesConfig {
    /// Whether state file injection is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Optional preamble text injected before state file contents.
    #[serde(default)]
    pub inject_preamble: Option<String>,

    /// State files to inject.
    #[serde(default)]
    pub files: Vec<StateFileEntry>,
}
