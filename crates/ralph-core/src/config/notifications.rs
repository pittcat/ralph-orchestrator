//! Notifications configuration.
//!
//! Controls loop-completion webhook notifications. Notifications are disabled
//! by default and are inert until explicitly enabled via `enabled: true`.
//!
//! Example configuration:
//! ```yaml
//! notifications:
//!   enabled: true
//!   timeout_seconds: 5
//!   endpoints:
//!     - name: feishu-success
//!       url: "https://open.feishu.cn/open-apis/bot/v2/hook/********"
//!       on: [success]
//!       headers:
//!         Content-Type: application/json
//!       body: '{"msg_type":"text","content":{"text":"Ralph OK {{loop_id}}"}}'
//!     - name: feishu-failure
//!       url: "https://open.feishu.cn/open-apis/bot/v2/hook/********"
//!       on: [failure]
//!       body: '{"msg_type":"text","content":{"text":"Ralph FAIL {{loop_id}}"}}'
//! ```

use serde::{Deserialize, Serialize};

use super::error::ConfigError;

/// Top-level notifications configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationsConfig {
    /// Whether notifications are enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Request timeout in seconds per endpoint.
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,

    /// List of notification endpoints.
    #[serde(default)]
    pub endpoints: Vec<NotificationEndpoint>,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            timeout_seconds: default_timeout_seconds(),
            endpoints: Vec::new(),
        }
    }
}

impl NotificationsConfig {
    /// Validates the notifications configuration.
    ///
    /// Validation only runs when `enabled == true`. When disabled, the
    /// entire section is inert and no errors are raised regardless of
    /// other field values.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !self.enabled {
            return Ok(());
        }

        if self.timeout_seconds == 0 {
            return Err(ConfigError::NotificationsValidation {
                field: "notifications.timeout_seconds".to_string(),
                message: "must be greater than 0 when notifications are enabled".to_string(),
            });
        }

        if self.endpoints.is_empty() {
            return Err(ConfigError::NotificationsValidation {
                field: "notifications.endpoints".to_string(),
                message: "must be non-empty when notifications are enabled".to_string(),
            });
        }

        for (i, endpoint) in self.endpoints.iter().enumerate() {
            endpoint.validate_with_index(i)?;
        }

        Ok(())
    }
}

/// A single notification endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationEndpoint {
    /// Optional display name for this endpoint.
    #[serde(default)]
    pub name: String,

    /// Target URL for the webhook POST request.
    #[serde(default)]
    pub url: String,

    /// Which termination statuses this endpoint receives.
    ///
    /// Accepts either a single value (`on: success`) or a sequence
    /// (`on: [success, failure]`). Unknown values deserialize to
    /// `OnStatus::Unknown` and are rejected by `validate()`, so an invalid
    /// filter surfaces as a validation error with a field path rather than
    /// a parse error.
    #[serde(default, deserialize_with = "deserialize_on_list")]
    pub on: Vec<OnStatus>,

    /// Optional HTTP headers to include in the request.
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,

    /// Request body template. Supports `{{var}}` placeholders.
    #[serde(default)]
    pub body: String,
}

impl NotificationEndpoint {
    fn validate_with_index(&self, index: usize) -> Result<(), ConfigError> {
        let base = format!("notifications.endpoints[{}]", index);

        if self.url.trim().is_empty() {
            return Err(ConfigError::NotificationsValidation {
                field: format!("{}.url", base),
                message: "must be non-empty when notifications are enabled".to_string(),
            });
        }

        if self.body.trim().is_empty() {
            return Err(ConfigError::NotificationsValidation {
                field: format!("{}.body", base),
                message: "must be non-empty when notifications are enabled".to_string(),
            });
        }

        if self.on.is_empty() {
            return Err(ConfigError::NotificationsValidation {
                field: format!("{}.on", base),
                message: "must be non-empty (allowed values: success, failure)".to_string(),
            });
        }

        for status in &self.on {
            if !status.is_valid() {
                return Err(ConfigError::NotificationsValidation {
                    field: format!("{}.on", base),
                    message: format!(
                        "invalid status '{}'; allowed values: success, failure",
                        status.as_str()
                    ),
                });
            }
        }

        Ok(())
    }
}

/// Status filter for a notification endpoint.
///
/// Unknown values (e.g. `on: [bogus]`) deserialize to `Unknown` via
/// `#[serde(other)]` instead of failing the parse; `validate()` rejects
/// them with a field-path error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OnStatus {
    #[default]
    Success,
    Failure,
    /// Catch-all for unrecognized status strings.
    #[serde(other)]
    Unknown,
}

impl OnStatus {
    /// Returns `true` for `Success`.
    pub fn is_success(self) -> bool {
        matches!(self, OnStatus::Success)
    }

    /// Returns `true` for `Failure`.
    pub fn is_failure(self) -> bool {
        matches!(self, OnStatus::Failure)
    }

    /// Returns the string representation.
    pub fn as_str(self) -> &'static str {
        match self {
            OnStatus::Success => "success",
            OnStatus::Failure => "failure",
            OnStatus::Unknown => "unknown",
        }
    }

    /// Returns `true` if the value is a known variant.
    fn is_valid(self) -> bool {
        matches!(self, OnStatus::Success | OnStatus::Failure)
    }
}

// ── Default value helpers ─────────────────────────────────────────────────────

fn default_timeout_seconds() -> u64 {
    5
}

/// Deserializes `on` from either a single scalar (`on: success`) or a
/// sequence (`on: [success, failure]`), always producing a `Vec`.
fn deserialize_on_list<'de, D>(deserializer: D) -> Result<Vec<OnStatus>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(OnStatus),
        Many(Vec<OnStatus>),
    }

    Ok(match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── NotificationsConfig defaults ──────────────────────────────────────────

    #[test]
    fn test_notifications_config_default_enabled_is_false() {
        let cfg = NotificationsConfig::default();
        assert!(!cfg.enabled);
    }

    #[test]
    fn test_notifications_config_default_timeout_is_5() {
        let cfg = NotificationsConfig::default();
        assert_eq!(cfg.timeout_seconds, 5);
    }

    #[test]
    fn test_notifications_config_default_endpoints_is_empty() {
        let cfg = NotificationsConfig::default();
        assert!(cfg.endpoints.is_empty());
    }

    // ── serde round-trip ─────────────────────────────────────────────────────

    #[test]
    fn test_notifications_config_round_trip() {
        let yaml = r#"enabled: true
timeout_seconds: 7
endpoints:
  - name: ep1
    url: "https://example.com/hook"
    on: [success, failure]
    headers:
      Content-Type: application/json
    body: '{"text":"done"}'
"#;
        let parsed: NotificationsConfig = serde_yaml::from_str(yaml).unwrap();
        let reloaded: NotificationsConfig =
            serde_yaml::from_str(&serde_yaml::to_string(&parsed).unwrap()).unwrap();
        assert_eq!(parsed, reloaded);
    }

    // ── OnStatus ─────────────────────────────────────────────────────────────

    #[test]
    fn test_on_status_is_success() {
        assert!(OnStatus::Success.is_success());
        assert!(!OnStatus::Failure.is_success());
    }

    #[test]
    fn test_on_status_is_failure() {
        assert!(!OnStatus::Success.is_failure());
        assert!(OnStatus::Failure.is_failure());
    }

    #[test]
    fn test_on_status_as_str() {
        assert_eq!(OnStatus::Success.as_str(), "success");
        assert_eq!(OnStatus::Failure.as_str(), "failure");
    }

    #[test]
    fn test_on_status_default_is_success() {
        assert_eq!(OnStatus::default(), OnStatus::Success);
    }

    // ── validate: disabled is always clean ───────────────────────────────────

    #[test]
    fn test_validate_disabled_with_invalid_timeout_is_clean() {
        let cfg = NotificationsConfig {
            enabled: false,
            timeout_seconds: 0,
            endpoints: vec![],
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_validate_disabled_with_empty_endpoints_is_clean() {
        let cfg = NotificationsConfig {
            enabled: false,
            timeout_seconds: 0,
            endpoints: vec![],
        };
        assert!(cfg.validate().is_ok());
    }

    // ── validate: enabled hard errors ────────────────────────────────────────

    #[test]
    fn test_validate_enabled_rejects_zero_timeout() {
        let cfg = NotificationsConfig {
            enabled: true,
            timeout_seconds: 0,
            endpoints: vec![NotificationEndpoint {
                name: "ep".to_string(),
                url: "https://example.com".to_string(),
                on: vec![OnStatus::Success],
                headers: std::collections::HashMap::new(),
                body: "hi".to_string(),
            }],
        };
        let err = cfg.validate().unwrap_err();
        assert!(matches!(
            &err,
            ConfigError::NotificationsValidation { field, .. }
            if field == "notifications.timeout_seconds"
        ));
    }

    #[test]
    fn test_validate_enabled_rejects_empty_endpoints() {
        let cfg = NotificationsConfig {
            enabled: true,
            timeout_seconds: 5,
            endpoints: vec![],
        };
        let err = cfg.validate().unwrap_err();
        assert!(matches!(
            &err,
            ConfigError::NotificationsValidation { field, .. }
            if field == "notifications.endpoints"
        ));
    }

    #[test]
    fn test_validate_enabled_rejects_endpoint_missing_url() {
        let cfg = NotificationsConfig {
            enabled: true,
            timeout_seconds: 5,
            endpoints: vec![NotificationEndpoint {
                name: "ep".to_string(),
                url: "   ".to_string(),
                on: vec![OnStatus::Success],
                headers: std::collections::HashMap::new(),
                body: "hi".to_string(),
            }],
        };
        let err = cfg.validate().unwrap_err();
        assert!(matches!(
            &err,
            ConfigError::NotificationsValidation { field, .. }
            if field.contains("notifications.endpoints[0].url")
        ));
    }

    #[test]
    fn test_validate_enabled_rejects_endpoint_missing_body() {
        let cfg = NotificationsConfig {
            enabled: true,
            timeout_seconds: 5,
            endpoints: vec![NotificationEndpoint {
                name: "ep".to_string(),
                url: "https://example.com".to_string(),
                on: vec![OnStatus::Success],
                headers: std::collections::HashMap::new(),
                body: "  ".to_string(),
            }],
        };
        let err = cfg.validate().unwrap_err();
        assert!(matches!(
            &err,
            ConfigError::NotificationsValidation { field, .. }
            if field.contains("notifications.endpoints[0].body")
        ));
    }

    #[test]
    fn test_validate_enabled_rejects_endpoint_empty_on() {
        let cfg = NotificationsConfig {
            enabled: true,
            timeout_seconds: 5,
            endpoints: vec![NotificationEndpoint {
                name: "ep".to_string(),
                url: "https://example.com".to_string(),
                on: vec![],
                headers: std::collections::HashMap::new(),
                body: "hi".to_string(),
            }],
        };
        let err = cfg.validate().unwrap_err();
        assert!(matches!(
            &err,
            ConfigError::NotificationsValidation { field, .. }
            if field.contains("notifications.endpoints[0].on")
        ));
    }

    // ── validate: success path ───────────────────────────────────────────────

    #[test]
    fn test_validate_enabled_with_valid_endpoint_is_clean() {
        let cfg = NotificationsConfig {
            enabled: true,
            timeout_seconds: 5,
            endpoints: vec![NotificationEndpoint {
                name: "ep".to_string(),
                url: "https://example.com".to_string(),
                on: vec![OnStatus::Success, OnStatus::Failure],
                headers: std::collections::HashMap::new(),
                body: "done".to_string(),
            }],
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_validate_enabled_multi_endpoint_is_clean() {
        let cfg = NotificationsConfig {
            enabled: true,
            timeout_seconds: 5,
            endpoints: vec![
                NotificationEndpoint {
                    name: "success-ep".to_string(),
                    url: "https://example.com/success".to_string(),
                    on: vec![OnStatus::Success],
                    headers: std::collections::HashMap::new(),
                    body: "ok".to_string(),
                },
                NotificationEndpoint {
                    name: "failure-ep".to_string(),
                    url: "https://example.com/failure".to_string(),
                    on: vec![OnStatus::Failure],
                    headers: std::collections::HashMap::new(),
                    body: "fail".to_string(),
                },
            ],
        };
        assert!(cfg.validate().is_ok());
    }
}
