//! 2026-07-02-006 plan U6: `on_event` primitive.
//!
//! Pure function that takes an accepted event topic and returns
//! the target phase id (if any). The engine keeps a list of
//! `on_event` rules — the primitive returns the **first match**
//! in declaration order.

use serde_yaml::Value;

/// Pure decision: given the trigger
/// `serde_yaml::Value` (`{event: "<topic>"}`) and the topic of
/// the accepted event, return the target phase id if the rule
/// matches.
///
/// The trigger MUST carry an `event` field. U2 already rejected
/// malformed `on` payloads at the declaration layer; this
/// primitive is the runtime evaluator and treats anything other
/// than `{event: "<topic>"}` as a non-match.
pub fn evaluate(trigger: &Value, event_topic: &str) -> Option<String> {
    let event_name = trigger
        .as_mapping()
        .and_then(|m| m.get(&Value::String("event".to_string())))
        .and_then(|v| v.as_str())?;
    if event_name == event_topic {
        // The target phase id is encoded in the surrounding
        // `transition.from -> transition.to` rule. The primitive
        // itself only decides whether the rule matches; the
        // target phase is propagated by `TransitionEvaluator`
        // (U10). For a stand-alone check (U6 test scenarios) we
        // surface the matched event name so callers can build
        // their own routing table.
        Some(event_name.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_work_start_trigger() {
        let trigger: Value = serde_yaml::from_str("event: work.start").unwrap();
        assert_eq!(evaluate(&trigger, "work.start"), Some("work.start".to_string()));
    }

    #[test]
    fn rejects_mismatched_topic() {
        let trigger: Value = serde_yaml::from_str("event: work.start").unwrap();
        assert_eq!(evaluate(&trigger, "review.complete"), None);
    }

    #[test]
    fn ignores_non_mapping_trigger() {
        let trigger: Value = serde_yaml::from_str("primitive: on_event").unwrap();
        assert_eq!(evaluate(&trigger, "work.start"), None);
    }

    #[test]
    fn ignores_trigger_without_event_field() {
        let trigger: Value = serde_yaml::from_str("foo: bar").unwrap();
        assert_eq!(evaluate(&trigger, "work.start"), None);
    }
}