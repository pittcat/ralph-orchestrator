//! Backend detection and authentication checking.
//!
//! This module provides functionality to detect which AI backends are available
//! and whether they are properly authenticated.

use std::fmt;
use std::time::Duration;

/// Supported AI backends for E2E testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Backend {
    /// Claude CLI backend
    Claude,
    /// OpenCode CLI backend
    OpenCode,
}

impl Backend {
    /// Returns the CLI command name for this backend.
    pub fn command(&self) -> &'static str {
        match self {
            Backend::Claude => "claude",
            Backend::OpenCode => "opencode",
        }
    }

    /// Returns all available backends.
    pub fn all() -> &'static [Backend] {
        &[Backend::Claude, Backend::OpenCode]
    }

    /// Returns the default timeout for this backend.
    pub fn default_timeout(&self) -> Duration {
        match self {
            Backend::Claude => Duration::from_mins(10), // 10 minutes - Claude iterations can take 60-120s each
            Backend::OpenCode => Duration::from_mins(5), // 5 minutes
        }
    }

    /// Returns the default max iterations for this backend.
    pub fn default_max_iterations(&self) -> u32 {
        match self {
            Backend::Claude => 5, // Extra buffer for LLM non-determinism
            Backend::OpenCode => 3,
        }
    }

    /// Returns the backend name in lowercase (for config files).
    pub fn as_config_str(&self) -> &'static str {
        match self {
            Backend::Claude => "claude",
            Backend::OpenCode => "opencode",
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Backend::Claude => write!(f, "Claude"),
            Backend::OpenCode => write!(f, "OpenCode"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_enum_excludes_kiro() {
        // The Backend enum in this crate contained `Backend::Kiro` (a
        // distinct variant from `Backend::OpenCode`). U5 deletes
        // `Backend::Kiro` because the kiro backend is gone from the
        // core adapters. We assert the enum now contains exactly
        // Claude + OpenCode.
        let variants = [Backend::Claude, Backend::OpenCode];
        let names: Vec<&'static str> = variants.iter().map(|b| b.as_config_str()).collect();
        assert_eq!(names, vec!["claude", "opencode"]);
        assert_eq!(Backend::all().len(), 2);
        // No Kiro entry in any helper:
        assert!(Backend::all().contains(&Backend::Claude));
        // Visual confirmation that the only Display strings are Claude/OpenCode.
        let disp: Vec<String> = Backend::all().iter().map(|b| b.to_string()).collect();
        assert_eq!(disp, vec!["Claude", "OpenCode"]);
    }
}
