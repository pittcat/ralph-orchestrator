//! 2026-07-02-006 plan U9: `on_loop_complete_honored` primitive.
//!
//! Pure decision over `LOOP_COMPLETE` payloads. The runtime
//! marks a `LOOP_COMPLETE` as `honored` once the verdict gate
//! has admitted it; the primitive then transitions the
//! workflow into the `terminal` phase so the loop can shut
//! down cleanly.

use serde_yaml::Value;

/// Pure decision: return the target phase id when the trigger
/// matches `LOOP_COMPLETE` with `honored == true`.
pub fn evaluate(trigger: &Value, event_topic: &str, honored: bool) -> Option<String> {
    if event_topic != "LOOP_COMPLETE" {
        return None;
    }

    let mapping = trigger.as_mapping()?;
    let primitive = mapping
        .get(Value::String("primitive".to_string()))?
        .as_str()?;
    if primitive != "on_loop_complete_honored" {
        return None;
    }

    if honored {
        Some("terminal".to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trigger() -> Value {
        serde_yaml::from_str("primitive: on_loop_complete_honored").unwrap()
    }

    #[test]
    fn honored_routes_to_terminal() {
        assert_eq!(
            evaluate(&trigger(), "LOOP_COMPLETE", true),
            Some("terminal".to_string())
        );
    }

    #[test]
    fn not_honored_does_not_match() {
        assert_eq!(evaluate(&trigger(), "LOOP_COMPLETE", false), None);
    }

    #[test]
    fn wrong_topic_does_not_match() {
        assert_eq!(evaluate(&trigger(), "work.done", true), None);
    }

    #[test]
    fn wrong_primitive_does_not_match() {
        let trigger: Value = serde_yaml::from_str("primitive: on_event").unwrap();
        assert_eq!(evaluate(&trigger, "LOOP_COMPLETE", true), None);
    }
}
