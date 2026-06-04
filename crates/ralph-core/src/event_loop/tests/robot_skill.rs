//! Tests for robot_skill.

use super::*;

#[test]
fn test_inject_robot_skill_when_enabled() {
    let yaml = r#"
RObot:
  enabled: true
  telegram:
    bot_token: "fake-token"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test prompt");

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    assert!(
        prompt.contains("<robot-skill>"),
        "Prompt should contain <robot-skill> when RObot is enabled"
    );
    assert!(
        prompt.contains("human.interact"),
        "Robot skill should mention human.interact"
    );
    assert!(
        prompt.contains("</robot-skill>"),
        "Robot skill should have closing tag"
    );
}

#[test]
fn test_inject_robot_skill_skipped_when_disabled() {
    let config = RalphConfig::default(); // RObot disabled by default
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test prompt");

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    assert!(
        !prompt.contains("<robot-skill>"),
        "Prompt should NOT contain <robot-skill> when RObot is disabled"
    );
}
