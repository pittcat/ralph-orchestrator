use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

/// Opt-in event-emission precheck gate (plan 2026-07-02-004).
///
/// Each rule attaches a checklist to a target topic X. The desugar step in
/// `RalphConfig::normalize` rewrites the producers of X to emit `X.proposed`
/// and synthesizes a gate hat that consumes `X.proposed` and emits either
/// `X` (pass) or `X.rejected` (fail with structured reason). The gate is
/// off by default; even with `enabled: true` it is a strict no-op when
/// `RALPH_PRECHECK_MODE=off` is set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PrecheckConfig {
    /// Master switch. When false, the entire block is ignored.
    #[serde(default)]
    pub enabled: bool,

    /// Per-topic checklist rules, keyed by target topic (e.g. "review.complete").
    #[serde(default)]
    pub rules: BTreeMap<String, PrecheckRule>,
}

/// One precheck rule for a target topic X.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrecheckRule {
    /// Checklist items the gate hat will render into its instructions.
    #[serde(default)]
    pub prompt: Vec<String>,

    /// Failure routing: where rejected events go and how many retries are
    /// allowed before escalating.
    #[serde(default)]
    pub on_fail: PrecheckOnFail,
}

/// Failure handling for a precheck rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrecheckOnFail {
    /// Hat to receive the `X.rejected` event (route target for the next round).
    pub target: String,

    /// Number of allowed rejections before escalation. Defaults to 3
    /// (mirrors `mechanism.flow.repair_budget`).
    #[serde(default = "default_retry_budget")]
    pub retry_budget: u32,

    /// Terminal topic emitted when the retry budget is exhausted. Typically
    /// `"plan.blocked(reason=precheck_failed)"`.
    #[serde(default)]
    pub on_exhausted: String,

    /// Short human-readable reason recorded on `X.rejected` payloads and
    /// injected into the target hat's next-round prompt.
    #[serde(default)]
    pub reason: String,
}

fn default_retry_budget() -> u32 {
    3
}

/// Test-only kill-switch override (`forbid(unsafe_code)` blocks
/// `std::env::set_var` in tests). Mirrors
/// `correction::set_correction_enabled_for_test`.
static PRECHECK_KILL_SWITCH_FOR_TEST: AtomicBool = AtomicBool::new(false);

/// Force the precheck desugar/runtime path off for the current
/// test process (nextest process-per-test isolation).
#[cfg(test)]
pub fn set_precheck_kill_switch_for_test(off: bool) {
    PRECHECK_KILL_SWITCH_FOR_TEST.store(off, Ordering::SeqCst);
}

#[cfg(test)]
pub fn reset_precheck_kill_switch_for_test() {
    PRECHECK_KILL_SWITCH_FOR_TEST.store(false, Ordering::SeqCst);
}

/// RAII guard for [`set_precheck_kill_switch_for_test`]. Sets the
/// kill switch on construction and clears it on drop, so a test
/// that opts out of precheck enforcement cannot leak its state into
/// the next test in the same binary.
///
/// Returns a small owned struct; assign to `_guard` to bind its
/// lifetime to the test scope:
/// ```ignore
/// let _guard = precheck_kill_switch_guard();
/// // ...precheck_runtime_enabled() returns false for this scope...
/// // drop on scope exit auto-clears the atom.
/// ```
#[cfg(test)]
pub struct PrecheckKillSwitchGuard {
    _private: (),
}

#[cfg(test)]
pub fn precheck_kill_switch_guard() -> PrecheckKillSwitchGuard {
    PRECHECK_KILL_SWITCH_FOR_TEST.store(true, Ordering::SeqCst);
    PrecheckKillSwitchGuard { _private: () }
}

#[cfg(test)]
impl Drop for PrecheckKillSwitchGuard {
    fn drop(&mut self) {
        PRECHECK_KILL_SWITCH_FOR_TEST.store(false, Ordering::SeqCst);
    }
}

/// Whether precheck desugar / runtime wiring is allowed. False when
/// `RALPH_PRECHECK_MODE=off` or the test override is active.
pub fn precheck_runtime_enabled() -> bool {
    if PRECHECK_KILL_SWITCH_FOR_TEST.load(Ordering::SeqCst) {
        return false;
    }
    std::env::var("RALPH_PRECHECK_MODE").as_deref() != Ok("off")
}

impl Default for PrecheckOnFail {
    fn default() -> Self {
        Self {
            target: String::new(),
            retry_budget: default_retry_budget(),
            on_exhausted: String::new(),
            reason: String::new(),
        }
    }
}

/// Inject `event_policy.schemas` entries for the derived topics
/// introduced by desugar (`<X>.proposed`, `<X>.rejected`). Idempotent:
/// existing schema entries are left untouched.
///
/// 2026-07-29-006 plan U4 (R3, S3): when the guarded topic `<X>`
/// already has a schema with a non-empty `required_fields` list,
/// the synthesized `<X>.proposed` schema inherits `payload` and
/// `required_fields` so the producer emit path catches missing
/// fields BEFORE the event is written to disk. The pre-U4 shell
/// was `EventSchema { payload: JsonObject, ..Default::default() }`,
/// which meant a guard like `dead_end_confidence >= 90` never
/// fired on the proposed path — the bare payload was accepted,
/// the gate then rejected it with `X.rejected` for the same
/// reason, and every retry burned budget on a check the producer
/// should have caught in the first place.
///
/// Inheritance scope is deliberately narrow (D3): only
/// `payload` + `required_fields` flow from the guarded schema to
/// the proposed schema. `allowed_values`, `hat_allowed_values`,
/// `element_constraints`, `field_docs`, `examples`,
/// `trigger_context`, and `known_fields` are **not** copied —
/// those are topic-level concerns that the gate hat and the
/// existing `<X>` schema already own.
pub fn inject_precheck_event_schemas(config: &mut crate::config::RalphConfig, topic: &str) {
    use crate::config::{EventSchema, PayloadType};

    let policy = config
        .event_loop
        .event_policy
        .get_or_insert_with(crate::config::EventPolicyConfig::default);
    let schemas = &mut policy.schemas;

    // Capture the guarded schema's shape BEFORE we touch the
    // proposed entry. The desugar runs after the guarded entry
    // is already in the map (preset YAML + inline_schemas merge),
    // so this read is a plain lookup with no borrow juggling.
    let guarded_shape: Option<(Option<PayloadType>, Vec<String>)> =
        schemas.get(topic).map(|s| (s.payload.clone(), s.required_fields.clone()));

    let proposed = format!("{topic}.proposed");
    schemas.entry(proposed).or_insert_with(|| {
        // 2026-07-29-006 U4 (R3): copy guarded `payload` +
        // `required_fields` so missing-field validation runs on
        // the proposed path. When the guarded topic has no
        // schema (the common case for hat-derivable topics) the
        // default shell is used — the existing pre-U4 behaviour.
        match &guarded_shape {
            Some((payload, required_fields)) if !required_fields.is_empty() => EventSchema {
                payload: payload.clone().or(Some(PayloadType::JsonObject)),
                required_fields: required_fields.clone(),
                ..Default::default()
            },
            _ => EventSchema {
                payload: Some(PayloadType::JsonObject),
                ..Default::default()
            },
        }
    });

    let rejected = format!("{topic}.rejected");
    schemas.entry(rejected).or_insert_with(|| EventSchema {
        payload: Some(PayloadType::JsonObject),
        required_fields: vec!["failed_checks".into(), "reason".into()],
        ..Default::default()
    });

    // Gate hat publishes bare `<X>` on pass; ensure a schema exists
    // (idempotent — presets that already declare `<X>` are untouched).
    schemas
        .entry(topic.to_string())
        .or_insert_with(|| EventSchema {
            payload: Some(PayloadType::JsonObject),
            ..Default::default()
        });
}

/// 2026-07-29-006 plan U2 (R2, R4, S2, S4, S5): pure function that
/// decides whether a `ralph emit` topic should be transparently
/// rewritten to `<topic>.proposed` before any policy / scope / origin
/// gate runs.
///
/// Rules, in order:
/// 1. precheck runtime not enabled (`precheck_runtime_enabled()` is
///    false, the config has no enabled `precheck` block, or the
///    rules map is empty) -> return the topic unchanged.
/// 2. `hat_id` is `None` or empty (no producer identity) -> return
///    the topic unchanged. Provenance still fails-closed on its own
///    gate; this function does not extend the producer set.
/// 3. The topic already ends with `.proposed` -> return it
///    unchanged (idempotent: prevents `.proposed.proposed` from
///    leaking out).
/// 4. The topic is NOT in the rule map -> return unchanged.
/// 5. The current hat's `publishes` does NOT include
///    `<topic>.proposed` -> return unchanged. **This is the
///    scope-preserving rule**: a hat that was not already a
///    producer of the proposed variant must not be promoted to one
///    by this function. The downstream isolated-scope / origin
///    guard will reject the bare emit, exactly as it does today.
/// 6. Otherwise -> return `<topic>.proposed`.
pub fn resolve_precheck_emit_topic(
    config: &crate::config::RalphConfig,
    hat_id: Option<&str>,
    topic: &str,
) -> String {
    // Rule 1: precheck runtime gating.
    let precheck = match config.event_loop.precheck.as_ref() {
        Some(p) if p.enabled && !p.rules.is_empty() => p,
        _ => return topic.to_string(),
    };
    if !precheck_runtime_enabled() {
        return topic.to_string();
    }

    // Rule 2: no hat identity -> leave scope decision to the
    // provenance gate.
    let hat_id = match hat_id {
        Some(id) if !id.is_empty() => id,
        _ => return topic.to_string(),
    };

    // Rule 3: idempotent -- already-proposed is fine, do not
    // double-suffix.
    if topic.ends_with(".proposed") {
        return topic.to_string();
    }

    // Rule 4: precheck has no rule guarding this topic.
    if !precheck.rules.contains_key(topic) {
        return topic.to_string();
    }

    // Rule 5: scope-preserving. The hat must already be a
    // producer of `<topic>.proposed` (i.e. desugar has run and
    // the hat is a real producer). A bare `<topic>` from a hat
    // whose `publishes` does not name the proposed variant is
    // left for the existing isolated-scope / origin gate to
    // reject.
    let proposed = format!("{topic}.proposed");
    let hat_publishes_proposed = config
        .hats
        .get(hat_id)
        .map(|hat| hat.publishes.iter().any(|p| p == proposed.as_str()))
        .unwrap_or(false);
    if !hat_publishes_proposed {
        return topic.to_string();
    }

    // Rule 6: rewrite.
    proposed
}

#[cfg(test)]
mod resolve_precheck_emit_topic_tests {
    use super::*;
    use crate::config::{
        HatConfig, HatExecutionMode, PrecheckConfig, PrecheckOnFail, PrecheckRule,
    };
    use std::collections::{BTreeMap, HashMap};

    fn rule(_target: &str) -> PrecheckRule {
        PrecheckRule {
            prompt: vec!["evidence exists".into()],
            on_fail: PrecheckOnFail {
                target: "executor".into(),
                retry_budget: 3,
                on_exhausted: String::new(),
                reason: String::new(),
            },
        }
    }

    fn hat_with_publishes(publishes: &[&str]) -> HatConfig {
        let mut hat = HatConfig::default();
        hat.name = "Executor".into();
        hat.description = Some("test".into());
        hat.publishes = publishes.iter().map(|s| s.to_string()).collect();
        hat
    }

    fn config_with(
        publishes: &[&str],
        enabled: bool,
        rules: &[&str],
    ) -> crate::config::RalphConfig {
        let mut cfg = crate::config::RalphConfig::default();
        cfg.event_loop.execution_mode = HatExecutionMode::Isolated;
        if enabled {
            let mut map = BTreeMap::new();
            for topic in rules {
                map.insert((*topic).to_string(), rule(topic));
            }
            cfg.event_loop.precheck = Some(PrecheckConfig {
                enabled: true,
                rules: map,
            });
        }
        let mut hats = HashMap::new();
        hats.insert("executor".to_string(), hat_with_publishes(publishes));
        cfg.hats = hats;
        cfg
    }

    // S2: already-proposed is idempotent.
    #[test]
    fn resolve_is_idempotent_for_proposed_suffix() {
        let cfg = config_with(&["work.failed.proposed"], true, &["work.failed"]);
        let resolved =
            resolve_precheck_emit_topic(&cfg, Some("executor"), "work.failed.proposed");
        assert_eq!(
            resolved, "work.failed.proposed",
            "S2: must not double-suffix"
        );
    }

    // S4: precheck disabled -> unchanged.
    #[test]
    fn resolve_leaves_topic_unchanged_when_precheck_disabled() {
        let cfg = config_with(&["work.failed.proposed"], false, &["work.failed"]);
        let resolved = resolve_precheck_emit_topic(&cfg, Some("executor"), "work.failed");
        assert_eq!(
            resolved, "work.failed",
            "S4 disabled: no rewrite"
        );
    }

    // S4 (variant): empty rules map.
    #[test]
    fn resolve_leaves_topic_unchanged_when_rules_empty() {
        let cfg = config_with(&["work.failed.proposed"], true, &[]);
        let resolved = resolve_precheck_emit_topic(&cfg, Some("executor"), "work.failed");
        assert_eq!(
            resolved, "work.failed",
            "S4 empty rules: no rewrite"
        );
    }

    // S4 (variant): kill-switch flips `precheck_runtime_enabled` to
    // false.
    #[test]
    fn resolve_leaves_topic_unchanged_when_kill_switch_active() {
        let _guard = precheck_kill_switch_guard();
        let cfg = config_with(&["work.failed.proposed"], true, &["work.failed"]);
        let resolved = resolve_precheck_emit_topic(&cfg, Some("executor"), "work.failed");
        assert_eq!(
            resolved, "work.failed",
            "S4 kill-switch: no rewrite"
        );
    }

    // S5: non-producer hat is not promoted.
    #[test]
    fn resolve_preserves_scope_for_non_producer_hat() {
        let cfg = config_with(&["work.done"], true, &["work.failed"]);
        let resolved = resolve_precheck_emit_topic(&cfg, Some("executor"), "work.failed");
        assert_eq!(
            resolved, "work.failed",
            "S5: must not promote a non-producer hat"
        );
    }

    // S5 (variant): unknown hat id.
    #[test]
    fn resolve_preserves_scope_for_unknown_hat() {
        let cfg = config_with(&["work.failed.proposed"], true, &["work.failed"]);
        let resolved = resolve_precheck_emit_topic(&cfg, Some("ghost"), "work.failed");
        assert_eq!(
            resolved, "work.failed",
            "S5: unknown hat must not be promoted"
        );
    }

    // Rule 2: no hat identity.
    #[test]
    fn resolve_leaves_topic_unchanged_when_hat_id_is_none() {
        let cfg = config_with(&["work.failed.proposed"], true, &["work.failed"]);
        let resolved = resolve_precheck_emit_topic(&cfg, None, "work.failed");
        assert_eq!(resolved, "work.failed", "no hat -> no rewrite");
    }

    // Rule 2 (variant): empty hat id.
    #[test]
    fn resolve_leaves_topic_unchanged_when_hat_id_is_empty() {
        let cfg = config_with(&["work.failed.proposed"], true, &["work.failed"]);
        let resolved = resolve_precheck_emit_topic(&cfg, Some(""), "work.failed");
        assert_eq!(resolved, "work.failed", "empty hat -> no rewrite");
    }

    // Rule 4: precheck has no rule guarding the topic.
    #[test]
    fn resolve_leaves_topic_unchanged_when_topic_not_guarded() {
        let cfg = config_with(&["review.passed.proposed"], true, &["review.passed"]);
        let resolved = resolve_precheck_emit_topic(&cfg, Some("executor"), "work.failed");
        assert_eq!(
            resolved, "work.failed",
            "Rule 4: unguarded topic must not be rewritten"
        );
    }

    // Happy path: producer + precheck enabled + guarded topic.
    #[test]
    fn resolve_rewrites_when_hat_publishes_proposed() {
        let cfg = config_with(&["work.failed.proposed"], true, &["work.failed"]);
        let resolved = resolve_precheck_emit_topic(&cfg, Some("executor"), "work.failed");
        assert_eq!(
            resolved, "work.failed.proposed",
            "happy path: rewrite to proposed"
        );
    }
}

#[cfg(test)]
mod inject_precheck_event_schemas_tests {
    //! 2026-07-29-006 plan U4 (R3, S3): the synthesized
    //! `<X>.proposed` schema must inherit `payload` +
    //! `required_fields` from the guarded `<X>` schema so
    //! missing-field validation runs on the proposed path.
    use super::*;
    use crate::config::{EventPolicyConfig, EventSchema, PayloadType};
    use std::collections::HashMap;

    fn config_with_guarded_schema(
        topic: &str,
        required_fields: Vec<&str>,
    ) -> crate::config::RalphConfig {
        let mut cfg = crate::config::RalphConfig::default();
        let mut schemas = HashMap::new();
        schemas.insert(
            topic.to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: required_fields.into_iter().map(String::from).collect(),
                ..Default::default()
            },
        );
        cfg.event_loop.event_policy = Some(EventPolicyConfig {
            enabled: true,
            schemas,
            ..EventPolicyConfig::default()
        });
        cfg
    }

    /// Happy path (R3): the guarded schema's `payload` and
    /// `required_fields` are copied to the proposed entry.
    #[test]
    fn inject_proposed_inherits_required_fields_from_guarded() {
        let mut cfg = config_with_guarded_schema(
            "work.failed",
            vec!["plan_name", "decisions_file", "reason"],
        );
        inject_precheck_event_schemas(&mut cfg, "work.failed");
        let schemas = &cfg.event_loop.event_policy.as_ref().unwrap().schemas;
        let proposed = schemas.get("work.failed.proposed").expect("proposed");
        assert_eq!(
            proposed.payload,
            Some(PayloadType::JsonObject),
            "payload must be inherited"
        );
        assert_eq!(
            proposed.required_fields,
            vec![
                "plan_name".to_string(),
                "decisions_file".to_string(),
                "reason".to_string(),
            ],
            "required_fields must be inherited from guarded schema"
        );
    }

    /// Empty `required_fields` falls back to the default shell —
    /// no spurious inheritance that would have rejected legal
    /// payloads.
    #[test]
    fn inject_proposed_uses_default_shell_when_guarded_has_no_required() {
        let mut cfg = config_with_guarded_schema("work.failed", vec![]);
        inject_precheck_event_schemas(&mut cfg, "work.failed");
        let schemas = &cfg.event_loop.event_policy.as_ref().unwrap().schemas;
        let proposed = schemas.get("work.failed.proposed").expect("proposed");
        assert_eq!(proposed.required_fields, Vec::<String>::new());
    }

    /// Idempotency: an explicit existing proposed schema is NOT
    /// overwritten by the guarded schema. Authors can opt in to a
    /// different shape by declaring it ahead of normalize.
    #[test]
    fn inject_does_not_overwrite_existing_proposed_schema() {
        let mut cfg = crate::config::RalphConfig::default();
        let mut schemas = HashMap::new();
        schemas.insert(
            "work.failed".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec!["a".into(), "b".into()],
                ..Default::default()
            },
        );
        // Hand-author a stricter proposed schema ahead of inject.
        schemas.insert(
            "work.failed.proposed".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec!["a".into()],
                ..Default::default()
            },
        );
        cfg.event_loop.event_policy = Some(EventPolicyConfig {
            enabled: true,
            schemas,
            ..EventPolicyConfig::default()
        });
        inject_precheck_event_schemas(&mut cfg, "work.failed");
        let schemas = &cfg.event_loop.event_policy.as_ref().unwrap().schemas;
        let proposed = schemas.get("work.failed.proposed").expect("proposed");
        assert_eq!(
            proposed.required_fields,
            vec!["a".to_string()],
            "explicit proposed schema must NOT be overwritten"
        );
    }

    /// Negative inheritance: `allowed_values`, `field_docs`, and
    /// `element_constraints` are NOT copied to the proposed
    /// schema. Those are topic-level concerns owned by the
    /// guarded schema and the gate hat.
    #[test]
    fn inject_does_not_inherit_out_of_scope_fields() {
        let mut cfg = crate::config::RalphConfig::default();
        let mut allowed_values = HashMap::new();
        allowed_values.insert("reason".to_string(), vec![serde_json::json!("unreachable")]);
        let mut schemas = HashMap::new();
        schemas.insert(
            "work.failed".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec!["reason".into()],
                allowed_values: allowed_values.clone(),
                ..Default::default()
            },
        );
        cfg.event_loop.event_policy = Some(EventPolicyConfig {
            enabled: true,
            schemas,
            ..EventPolicyConfig::default()
        });
        inject_precheck_event_schemas(&mut cfg, "work.failed");
        let schemas = &cfg.event_loop.event_policy.as_ref().unwrap().schemas;
        let proposed = schemas.get("work.failed.proposed").expect("proposed");
        assert!(
            proposed.allowed_values.is_empty(),
            "allowed_values must NOT be inherited (D3 scope)"
        );
    }
}
