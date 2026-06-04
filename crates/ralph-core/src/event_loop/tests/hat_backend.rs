//! Tests for hat_backend.

use super::*;

#[test]
fn test_get_hat_backend_with_named_backend() {
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
    backend: "claude"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let event_loop = EventLoop::new(config);

    let hat_id = HatId::new("builder");
    let backend = event_loop.get_hat_backend(&hat_id);

    assert!(backend.is_some());
    match backend.unwrap() {
        HatBackend::Named(name) => assert_eq!(name, "claude"),
        _ => panic!("Expected Named backend"),
    }
}

#[test]
fn test_get_hat_backend_with_kiro_agent() {
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
    backend:
      type: "kiro"
      agent: "my-agent"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let event_loop = EventLoop::new(config);

    let hat_id = HatId::new("builder");
    let backend = event_loop.get_hat_backend(&hat_id);

    assert!(backend.is_some());
    match backend.unwrap() {
        HatBackend::KiroAgent { agent, .. } => assert_eq!(agent, "my-agent"),
        _ => panic!("Expected KiroAgent backend"),
    }
}

#[test]
fn test_get_hat_backend_inherits_global() {
    let yaml = r#"
cli:
  backend: "gemini"
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let event_loop = EventLoop::new(config);

    let hat_id = HatId::new("builder");
    let backend = event_loop.get_hat_backend(&hat_id);

    // Hat has no backend configured, should return None (inherit global)
    assert!(backend.is_none());
}
