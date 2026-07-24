use ralph_core::{ConfigError, RalphConfig};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn write_config_file(temp_dir: &TempDir, yaml: &str) -> PathBuf {
    let path = temp_dir.path().join("ralph.yml");
    fs::write(&path, yaml).expect("failed to write temporary config file");
    path
}

// ── Test 1: default is disabled ─────────────────────────────────────────────

#[test]
fn test_notifications_default_is_disabled() {
    let cfg = ralph_core::config::NotificationsConfig::default();
    assert!(
        !cfg.enabled,
        "NotificationsConfig::default() must have enabled == false"
    );
    assert_eq!(
        cfg.timeout_seconds, 5,
        "NotificationsConfig::default() must have timeout_seconds == 5"
    );
    assert!(
        cfg.endpoints.is_empty(),
        "NotificationsConfig::default() must have empty endpoints"
    );
}

// ── Test 2: absent section uses defaults ─────────────────────────────────────

#[test]
fn test_notifications_section_absent_uses_defaults() {
    let yaml = "agent: claude\nevent_loop:\n  completion_promise: DONE\n";
    let parsed: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(
        !parsed.notifications.enabled,
        "missing notifications: section must default to enabled=false"
    );
    assert_eq!(
        parsed.notifications.timeout_seconds, 5,
        "missing notifications: section must default to timeout_seconds=5"
    );
    assert!(
        parsed.notifications.endpoints.is_empty(),
        "missing notifications: section must have empty endpoints"
    );
}

// ── Test 3: enabled round-trip ────────────────────────────────────────────────

#[test]
fn test_notifications_enabled_round_trip() {
    let yaml = r#"agent: claude
event_loop:
  completion_promise: DONE
notifications:
  enabled: true
  timeout_seconds: 7
  endpoints:
    - name: feishu-ok
      url: "https://open.feishu.cn/open-apis/bot/v2/hook/abc123"
      on: [success]
      headers:
        Content-Type: application/json
      body: '{"msg_type":"text","content":{"text":"OK {{loop_id}}"}}'
"#;
    let parsed: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(parsed.notifications.enabled);
    assert_eq!(parsed.notifications.timeout_seconds, 7);
    assert_eq!(parsed.notifications.endpoints.len(), 1);

    // Reserialize and parse again — must match.
    let reloaded: RalphConfig =
        serde_yaml::from_str(&serde_yaml::to_string(&parsed).unwrap()).unwrap();
    assert_eq!(parsed.notifications, reloaded.notifications);
}

// ── Test 4: enabled with empty endpoints → hard error ────────────────────────

#[test]
fn test_notifications_enabled_no_endpoints_rejected() {
    let yaml = "agent: claude\nevent_loop:\n  completion_promise: DONE\nnotifications:\n  enabled: true\n  endpoints: []\n";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let err = config.validate().unwrap_err();
    assert!(
        matches!(
            &err,
            ConfigError::NotificationsValidation { field, .. }
            if field == "notifications.endpoints"
        ),
        "expected NotificationsValidation for notifications.endpoints, got: {err:?}"
    );
}

// ── Test 5: enabled with zero timeout → hard error ───────────────────────────

#[test]
fn test_notifications_enabled_zero_timeout_rejected() {
    let yaml = r#"agent: claude
event_loop:
  completion_promise: DONE
notifications:
  enabled: true
  timeout_seconds: 0
  endpoints:
    - name: ep
      url: "https://example.com"
      on: [success]
      body: '{"text":"hi"}'
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let err = config.validate().unwrap_err();
    assert!(
        matches!(
            &err,
            ConfigError::NotificationsValidation { field, .. }
            if field == "notifications.timeout_seconds"
        ),
        "expected NotificationsValidation for notifications.timeout_seconds, got: {err:?}"
    );
}

// ── Test 6: endpoint missing url ─────────────────────────────────────────────

#[test]
fn test_notifications_endpoint_missing_url_rejected() {
    let yaml = "agent: claude\nevent_loop:\n  completion_promise: DONE\nnotifications:\n  enabled: true\n  endpoints:\n    - name: ep\n      on: [success]\n      body: '{\"text\":\"hi\"}'\n";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let err = config.validate().unwrap_err();
    assert!(
        matches!(
            &err,
            ConfigError::NotificationsValidation { field, .. }
            if field.contains("notifications.endpoints[0].url")
        ),
        "expected NotificationsValidation for notifications.endpoints[i].url, got: {err:?}"
    );
}

// ── Test 7: endpoint missing body ────────────────────────────────────────────

#[test]
fn test_notifications_endpoint_missing_body_rejected() {
    let yaml = "agent: claude\nevent_loop:\n  completion_promise: DONE\nnotifications:\n  enabled: true\n  endpoints:\n    - name: ep\n      url: \"https://example.com\"\n      on: [success]\n";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let err = config.validate().unwrap_err();
    assert!(
        matches!(
            &err,
            ConfigError::NotificationsValidation { field, .. }
            if field.contains("notifications.endpoints[0].body")
        ),
        "expected NotificationsValidation for notifications.endpoints[i].body, got: {err:?}"
    );
}

// ── Test 8: endpoint invalid on value ────────────────────────────────────────

#[test]
fn test_notifications_endpoint_invalid_on_rejected() {
    let yaml = r#"agent: claude
event_loop:
  completion_promise: DONE
notifications:
  enabled: true
  endpoints:
    - name: ep
      url: "https://example.com"
      on: [bogus]
      body: '{"text":"hi"}'
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let err = config.validate().unwrap_err();
    assert!(
        matches!(
            &err,
            ConfigError::NotificationsValidation { field, .. }
            if field.contains("notifications.endpoints[0].on")
        ),
        "expected NotificationsValidation for notifications.endpoints[i].on, got: {err:?}"
    );
}

// ── Test 9: endpoint empty on array ─────────────────────────────────────────

#[test]
fn test_notifications_endpoint_empty_on_rejected() {
    let yaml = r#"agent: claude
event_loop:
  completion_promise: DONE
notifications:
  enabled: true
  endpoints:
    - name: ep
      url: "https://example.com"
      on: []
      body: '{"text":"hi"}'
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let err = config.validate().unwrap_err();
    assert!(
        matches!(
            &err,
            ConfigError::NotificationsValidation { field, .. }
            if field.contains("notifications.endpoints[0].on")
        ),
        "expected NotificationsValidation for notifications.endpoints[i].on, got: {err:?}"
    );
}

// ── Test 10: disabled with zero timeout passes clean ─────────────────────────

#[test]
fn test_notifications_disabled_with_zero_timeout_passes() {
    let yaml = "agent: claude\nevent_loop:\n  completion_promise: DONE\nnotifications:\n  enabled: false\n  timeout_seconds: 0\n";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let result = config.validate();
    assert!(
        result.is_ok(),
        "enabled=false with timeout_seconds=0 must pass validate, got: {:?}",
        result.unwrap_err()
    );
}

// ── Test 11: 2-endpoint Feishu YAML from plan ─────────────────────────────────

#[test]
fn test_notifications_feishu_yaml_parses_and_validates() {
    let yaml = r#"agent: claude
event_loop:
  completion_promise: DONE
notifications:
  enabled: true
  timeout_seconds: 5
  endpoints:
    - name: feishu-success
      url: "https://open.feishu.cn/open-apis/bot/v2/hook/********"
      on: [success]
      headers:
        Content-Type: application/json
      body: '{"msg_type":"text","content":{"text":"Ralph OK {{loop_id}} ({{termination_reason}}"}}'
    - name: feishu-failure
      url: "https://open.feishu.cn/open-apis/bot/v2/hook/********"
      on: [failure]
      body: '{"msg_type":"text","content":{"text":"Ralph FAIL {{loop_id}}: {{termination_reason}}"}}'
"#;
    let config = RalphConfig::parse_yaml(yaml).unwrap();
    let warnings = config
        .validate()
        .expect("feishu YAML must validate cleanly");
    assert!(
        warnings.is_empty(),
        "feishu YAML must produce zero warnings"
    );
    assert!(config.notifications.enabled);
    assert_eq!(config.notifications.timeout_seconds, 5);
    assert_eq!(config.notifications.endpoints.len(), 2);

    let ep_success = &config.notifications.endpoints[0];
    assert_eq!(ep_success.name, "feishu-success");
    assert_eq!(
        ep_success.url,
        "https://open.feishu.cn/open-apis/bot/v2/hook/********"
    );
    assert_eq!(ep_success.on.len(), 1);
    assert!(ep_success.on[0].is_success());

    let ep_failure = &config.notifications.endpoints[1];
    assert_eq!(ep_failure.name, "feishu-failure");
    assert_eq!(ep_failure.on.len(), 1);
    assert!(ep_failure.on[0].is_failure());
}

// ── Test 12: on: [success, failure] parses ───────────────────────────────────

#[test]
fn test_notifications_endpoint_with_failure_and_success_on_parses() {
    let yaml = r#"agent: claude
event_loop:
  completion_promise: DONE
notifications:
  enabled: true
  endpoints:
    - name: all-events
      url: "https://example.com/notify"
      on: [success, failure]
      body: '{"event":"{{status}}"}'
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let warnings = config
        .validate()
        .expect("on: [success, failure] must validate");
    assert!(warnings.is_empty());
    let ep = &config.notifications.endpoints[0];
    assert_eq!(ep.on.len(), 2);
}

// ── Test 13: error field paths always contain "notifications." ────────────────

#[test]
fn test_notifications_error_field_paths_stable() {
    let cases = [
        (
            r#"
notifications:
  enabled: true
  timeout_seconds: 0
  endpoints:
    - name: e
      url: u
      on: s
      body: b
"#,
            "notifications.timeout_seconds",
        ),
        (
            r#"
notifications:
  enabled: true
  endpoints: []
"#,
            "notifications.endpoints",
        ),
        (
            r#"
notifications:
  enabled: true
  endpoints:
    - name: e
      url: ""
      on: [success]
      body: b
"#,
            "notifications.endpoints",
        ),
        (
            r#"
notifications:
  enabled: true
  endpoints:
    - name: e
      url: u
      on: [bogus]
      body: b
"#,
            "notifications.endpoints",
        ),
    ];

    for (i, (yaml_snippet, expected_prefix)) in cases.into_iter().enumerate() {
        let yaml = format!(
            "agent: claude\nevent_loop:\n  completion_promise: DONE\n{}",
            yaml_snippet
        );
        let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
        let err = config.validate().unwrap_err();

        let err_msg = err.to_string();
        assert!(
            err_msg.contains(expected_prefix),
            "case {i}: error message must contain '{expected_prefix}', got: {err_msg}"
        );
    }
}
