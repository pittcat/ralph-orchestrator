//! 2026-07-02-006 plan: `on_plan_terminal_accepted` primitive.
//!
//! Matches `plan.complete` or legal `plan.blocked` after the emit
//! pipeline has accepted the event (`accepted: true` in YAML).

use serde_yaml::Value;

/// Pure decision: return a hit signal when the trigger matches
/// the event topic and the event was accepted.
pub fn evaluate(trigger: &Value, event_topic: &str, accepted: bool) -> Option<String> {
    if !accepted {
        return None;
    }

    let mapping = trigger.as_mapping()?;
    if mapping.contains_key(&Value::String("primitive".to_string())) {
        let primitive = mapping
            .get(&Value::String("primitive".to_string()))?
            .as_str()?;
        if primitive != "on_plan_terminal_accepted" {
            return None;
        }
    }

    let event_name = mapping
        .get(&Value::String("event".to_string()))
        .and_then(|v| v.as_str())?;
    if event_name != event_topic {
        return None;
    }

    // Accepted terminal events match; target phase comes from `to:`.
    Some(event_name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_complete_accepted_matches() {
        let trigger: Value = serde_yaml::from_str(
            r#"
primitive: on_plan_terminal_accepted
event: plan.complete
accepted: true
"#,
        )
        .unwrap();
        assert_eq!(
            evaluate(&trigger, "plan.complete", true),
            Some("plan.complete".to_string())
        );
    }

    #[test]
    fn plan_blocked_accepted_matches() {
        let trigger: Value = serde_yaml::from_str(
            r#"
primitive: on_plan_terminal_accepted
event: plan.blocked
accepted: true
"#,
        )
        .unwrap();
        assert_eq!(
            evaluate(&trigger, "plan.blocked", true),
            Some("plan.blocked".to_string())
        );
    }

    #[test]
    fn rejected_event_does_not_match() {
        let trigger: Value = serde_yaml::from_str("event: plan.complete").unwrap();
        assert_eq!(evaluate(&trigger, "plan.complete", false), None);
    }
}
