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
pub fn inject_precheck_event_schemas(config: &mut crate::config::RalphConfig, topic: &str) {
    use crate::config::{EventSchema, PayloadType};

    let policy = config
        .event_loop
        .event_policy
        .get_or_insert_with(crate::config::EventPolicyConfig::default);
    let schemas = &mut policy.schemas;

    let proposed = format!("{topic}.proposed");
    schemas.entry(proposed).or_insert_with(|| EventSchema {
        payload: Some(PayloadType::JsonObject),
        ..Default::default()
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
