//! Configuration warning types.

/// Configuration warnings emitted during validation.
#[derive(Debug, Clone)]
pub enum ConfigWarning {
    /// Feature is enabled but not yet available in v2.
    DeferredFeature { field: String, message: String },
    /// Field is present but ignored in v2.
    DroppedField { field: String, reason: String },
    /// Field has an invalid value.
    InvalidValue { field: String, message: String },
    /// Hat has empty terminal_events (legacy / non-participating hat).
    EmptyTerminalEvents { hat: String },
}

impl std::fmt::Display for ConfigWarning {
    #[allow(clippy::match_same_arms)] // Different arms have different messages despite similar structure
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigWarning::DeferredFeature { field, message }
            | ConfigWarning::InvalidValue { field, message } => {
                write!(f, "Warning [{field}]: {message}")
            }
            ConfigWarning::DroppedField { field, reason } => {
                write!(f, "Warning [{field}]: Field ignored - {reason}")
            }
            ConfigWarning::EmptyTerminalEvents { hat } => {
                write!(
                    f,
                    "Warning [terminal_events]: Hat '{hat}' has no terminal events configured; \
                     lifecycle tracking will not apply. Add 'terminal_events' to opt in."
                )
            }
        }
    }
}
