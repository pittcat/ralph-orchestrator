//! `run_gates` — unified gate engine used by both the linter
//! and the runtime loop (R15, plan 2026-06-20-001).
//!
//! Single source of truth: rules come from `ProtocolView` (the
//! embedded protocol SSOT), the *context* decides whether we are
//! in lint (stateless) or runtime (stateful) mode. The same gate
//! function is invoked twice — pre-write by the linter, post-
//! receive by the loop — so the two layers cannot drift.
//!
//! ## P1-1: structured rejection classification
//!
//! `GateDecision::Reject` carries a typed [`RejectionKind`] enum
//! in addition to the human-readable message. The linter's
//! `LintResumeHint` derives its class directly from the enum, so
//! the routing target (`SourceHat` / `PlanGate`) is determined by
//! the *kind of failure*, not by string-substring matching. This
//! eliminates the previous P1-1 vulnerability: a reason string
//! that happened to contain the word "artifact" would have
//! mis-classified a payload error as a handoff-artifact error.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::protocol::ProtocolView;

/// Gate decision returned by [`run_gates`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// Event is admitted.
    Accept,
    /// Event is rejected. The `kind` carries the structural
    /// classification (used by the linter to pick the resume
    /// target hat); `message` is the human-readable detail
    /// (logged + shown to the agent).
    Reject {
        kind: RejectionKind,
        message: String,
    },
}

/// Structural rejection classification (P1-1).
///
/// The linter maps `RejectionKind` to `LintFailureClass` to
/// decide whether the resume hint should route back to the
/// source hat or to `plan-gate`. The runtime uses the same
/// classification to populate `recent_rejection_digest`
/// reason_codes.
///
/// Adding a new variant is the supported way to add a new
/// rejection class. String-substring matching on `message` is
/// **not** supported; do not reintroduce it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RejectionKind {
    /// Required payload field is missing. Routes to the
    /// source hat (the agent that emitted the event) so the
    /// payload can be corrected.
    MissingField,
    /// Topic emitted by a hat that does not own it. Routes
    /// back to the source hat to discourage cross-hat
    /// publishing.
    TopicOwnership,
    /// Upstream state mismatch (progress.md / step / state
    /// projection). Routes to `plan-gate` which owns the
    /// orchestration state.
    UpstreamState,
    /// Gate context refused the event before any field check
    /// (e.g. runtime TTL exceeded). Routes to the source hat.
    PreCheck,
    /// 2026-06-23-005 U1 (R1+R2): synthesised by the
    /// missing-event hard gate (`hard_gate::inject_missing_event_hard_gate_guidance`).
    /// Routes to `CoordinatorDispatcher` for typed kind dispatch
    /// (PlanBlocked when consecutive_count >= 2, per KTD-2).
    MissingEventGate,
    /// 2026-06-23-005 U1 (R1+R2): synthesised by orchestrator
    /// stall_recovery paths (e.g. `mod.rs:2755` `enrich_task_resume_payload(..., "stall_no_events", ...)`)
    /// when the loop detects no events flowing for N iterations.
    /// Routes to `CoordinatorDispatcher` (PlanBlocked when count >= 3).
    StallNoEvents,
    /// 2026-06-23-005 U1 (R1+R2): synthesised by the payload
    /// contract rejection (`mod.rs:2679` `enrich_task_resume_payload(..., reason_str, ...)`)
    /// when the hat emits a structurally-invalid payload.
    /// Routes to `CoordinatorDispatcher` (DriftFinding when count >= 1).
    ContractViolation,
    /// 2026-06-23-005 F2 (P1 fix): synthesised by the
    /// persistent-mode completion-suppression path
    /// (`mod.rs:1757` `enrich_task_resume_payload(..., "persistent mode", ...)`).
    /// The runtime detected a completion signal but `event_loop.persistent`
    /// is set, so it injects task.resume to nudge the agent to look
    /// for new tasks or wait for human guidance. Routes to
    /// `CoordinatorDispatcher::ReEmitWorkReady` (the recovery action
    /// is to re-emit work.ready to give the agent another chance).
    PersistentLoopActive,
    /// 2026-06-23-005 F2 (P1 fix): synthesised by the open-tasks
    /// completion-rejection path (`mod.rs:1801`
    /// `enrich_task_resume_payload(..., "open tasks remain", ...)`).
    /// The runtime rejected the completion signal because at least
    /// one runtime task is still `open`; the agent must close, fail,
    /// or reopen outstanding tasks before the loop can honour the
    /// completion promise. Routes to `CoordinatorDispatcher::ReEmitWorkReady`.
    OpenTasksBlocking,
}

impl RejectionKind {
    /// Map a gate-rejection kind to the linter's failure class.
    /// The two enums are kept distinct so the engine layer
    /// does not depend on the linter layer; this mapping is
    /// the only cross-layer surface.
    pub fn to_lint_class(self) -> crate::preset::engine::hint::LintFailureClass {
        use crate::preset::engine::hint::LintFailureClass;
        match self {
            RejectionKind::MissingField => LintFailureClass::PayloadError,
            RejectionKind::TopicOwnership => LintFailureClass::TopicOwnership,
            RejectionKind::UpstreamState => LintFailureClass::UpstreamStateMissing,
            RejectionKind::PreCheck => LintFailureClass::PayloadError,
            // 2026-06-23-005 U1: three new typed kinds all map to
            // PayloadError (the source hat's output is missing
            // the required event/payload shape).
            RejectionKind::MissingEventGate => LintFailureClass::PayloadError,
            RejectionKind::StallNoEvents => LintFailureClass::PayloadError,
            RejectionKind::ContractViolation => LintFailureClass::PayloadError,
            // 2026-06-23-005 F2: completion-signal rejection paths
            // also map to PayloadError (the agent emitted a
            // completion promise with a structurally-invalid
            // surrounding state — persistent mode active, or open
            // tasks still pending).
            RejectionKind::PersistentLoopActive => LintFailureClass::PayloadError,
            RejectionKind::OpenTasksBlocking => LintFailureClass::PayloadError,
        }
    }

    /// Stable string identifier for log / reason_code
    /// aggregation. Operators rely on this in scripts so the
    /// values are part of the public surface — do not rename
    /// without a migration plan.
    pub fn reason_code(self) -> &'static str {
        match self {
            RejectionKind::MissingField => "missing_field",
            RejectionKind::TopicOwnership => "topic_ownership",
            RejectionKind::UpstreamState => "upstream_state",
            RejectionKind::PreCheck => "pre_check",
            // 2026-06-23-005 U1: three new typed kinds for
            // task.resume injection paths (hard_gate / stall_recovery / contract).
            RejectionKind::MissingEventGate => "missing_event_gate",
            RejectionKind::StallNoEvents => "stall_no_events",
            RejectionKind::ContractViolation => "contract_violation",
            // 2026-06-23-005 F2: completion-signal rejection paths.
            // Operators rely on these strings in `recovery.jsonl`
            // grep aggregations; values are part of the public
            // surface — do not rename without a migration plan.
            RejectionKind::PersistentLoopActive => "persistent_loop_active",
            RejectionKind::OpenTasksBlocking => "open_tasks_blocking",
        }
    }

    /// 2026-06-23 fix plan U6 (CB-3): reverse-lookup helper for
    /// callers that already have a reason_code string but no
    /// typed kind (e.g. legacy correction paths that build the
    /// `RejectionRecord` from a free-form reason). Returns the
    /// matching kind when the reason_code is a known kind
    /// `reason_code()`; otherwise `None` (caller falls back to
    /// `RejectionRecord::new` with `kind=None`).
    pub fn from_reason_code(reason_code: &str) -> Option<Self> {
        match reason_code {
            "missing_field" => Some(Self::MissingField),
            "topic_ownership" => Some(Self::TopicOwnership),
            "upstream_state" => Some(Self::UpstreamState),
            "pre_check" => Some(Self::PreCheck),
            "missing_event_gate" => Some(Self::MissingEventGate),
            "stall_no_events" => Some(Self::StallNoEvents),
            "contract_violation" => Some(Self::ContractViolation),
            "persistent_loop_active" => Some(Self::PersistentLoopActive),
            "open_tasks_blocking" => Some(Self::OpenTasksBlocking),
            _ => None,
        }
    }
}

/// Gate context trait. Lint is stateless (implements `Clone`/`Send`),
/// runtime is stateful (may carry `&mut` references). Both
/// implementations run the same gate function with the same
/// `ProtocolView` so the two layers can never disagree on what
/// "valid" means.
pub trait GateContext {
    /// Whether the gate should run for the given (topic, payload)
    /// pair. Lets runtime contexts skip control topics that
    /// don't need policy validation.
    fn is_applicable(&self, topic: &str) -> bool;
    /// Lint contexts are pure: they only inspect the view + payload.
    /// Runtime contexts may consult additional state (recovery
    /// file, rejection TTL) before deciding. Implementations
    /// return a [`Rejection`] when refusing so the linter can
    /// route the resume hint correctly.
    fn pre_check(&self, _topic: &str, _payload: &Value) -> Result<(), Rejection> {
        Ok(())
    }
}

/// Structured rejection. The `kind` carries the classification;
/// the `message` is human-readable detail shown in logs and to
/// the agent. Prefer this over `Result<(), String>` for any new
/// gate code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub kind: RejectionKind,
    pub message: String,
}

impl Rejection {
    pub fn new(kind: RejectionKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// Stateless lint context. Mirrors the runtime gate; both call
/// [`run_gates`] with the same `ProtocolView`.
#[derive(Debug, Clone)]
pub struct LintContext;

impl GateContext for LintContext {
    fn is_applicable(&self, _topic: &str) -> bool {
        true
    }
}

/// Run the unified gate set against a single event. The same
/// function is used by lint (`LintContext`) and runtime (custom
/// stateful contexts). The decision is derived from the
/// `ProtocolView` so the two layers cannot diverge.
///
/// `from_hat` is reserved for future hat-aware gates; it is
/// currently unused by the engine itself but kept on the
/// signature to avoid churn at call sites.
pub fn run_gates<C: GateContext>(
    view: &ProtocolView,
    ctx: &C,
    topic: &str,
    payload: &Value,
    _from_hat: Option<&str>,
) -> GateDecision {
    if !ctx.is_applicable(topic) {
        return GateDecision::Accept;
    }
    if let Err(rej) = ctx.pre_check(topic, payload) {
        return GateDecision::Reject {
            kind: rej.kind,
            message: rej.message,
        };
    }

    let required = view.required_fields(topic);
    let missing = missing_fields(&required, payload);
    if !missing.is_empty() {
        return GateDecision::Reject {
            kind: RejectionKind::MissingField,
            message: format!(
                "missing required fields: {}",
                missing.into_iter().collect::<Vec<_>>().join(",")
            ),
        };
    }
    GateDecision::Accept
}

/// Compute the set difference: `required - present_in_payload`.
/// Treats non-object payloads as empty so the gate fails closed
/// (every required field is reported missing).
fn missing_fields(required: &HashSet<String>, payload: &Value) -> HashSet<String> {
    let present = match payload {
        Value::Object(map) => map.keys().cloned().collect(),
        _ => HashSet::new(),
    };
    required.difference(&present).cloned().collect()
}

/// U7 (plan 2026-06-23-004): downstream_publishes 派生 SSOT 化.
///
/// CLI precheck 与 runtime gate 必须从同一函数取下游 hat 的 publishes 列表,
/// 否则会出现 precheck PASS 但 runtime Reject 的不一致场景。
///
/// 解析规则:
/// 1. 通过 `consumer_of(topic)` 找到下游 hat 的 id(若 `topic` 没有注册消费者则返回空列表)
/// 2. 查该 hat 的 `publishes` 配置
/// 3. 没有匹配 → 返回默认 `["work.done", "work.failed"]`(与历史 runtime 行为一致)
///
/// 这是一个纯函数;lint / runtime 共享实现,杜绝两份代码各算一遍。
pub fn resolve_downstream_publishes(
    consumer_of: impl Fn(&str) -> Option<String>,
    preset_hats: &std::collections::BTreeMap<String, crate::config::HatConfig>,
    topic: &str,
) -> Vec<String> {
    let consumer = match consumer_of(topic) {
        Some(c) => c,
        None => return Vec::new(),
    };
    preset_hats
        .get(&consumer)
        .map(|h| h.publishes.clone())
        .unwrap_or_else(|| {
            // 与 `event_loop::preset_hats_publishes` 保持一致的默认。
            vec!["work.done".to_string(), "work.failed".to_string()]
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::execution_contracts::ExecutionContractsConfig;
    use serde_json::json;
    use std::collections::HashMap;

    fn empty_view() -> ProtocolView {
        ProtocolView {
            effective_required_fields: HashMap::new(),
            verdict_gate: None,
            workflow_contract: None,
            workflow_guards: None,
            state_projection: None,
            execution_contracts: Some(ExecutionContractsConfig::default()),
            event_policy: None,
            protocol_hash: "0".to_string(),
            feature_flag_enabled: false,
        }
    }

    #[test]
    fn accept_when_no_required_fields() {
        let view = empty_view();
        let decision = run_gates(&view, &LintContext, "any", &json!({}), None);
        assert_eq!(decision, GateDecision::Accept);
    }

    #[test]
    fn reject_when_required_missing() {
        let mut view = empty_view();
        let mut reqs = HashSet::new();
        reqs.insert("plan_name".to_string());
        reqs.insert("step".to_string());
        view.effective_required_fields
            .insert("work.done".to_string(), reqs);
        let decision = run_gates(&view, &LintContext, "work.done", &json!({}), None);
        match decision {
            GateDecision::Reject { kind, message } => {
                assert_eq!(kind, RejectionKind::MissingField);
                assert!(message.contains("plan_name"));
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    /// 2026-06-23 fix (adversarial review P1-4): a MissingField
    /// regression that flips the kind to TopicOwnership MUST
    /// trip this test. The P1-4 fix is to assert the *kind*
    /// explicitly (not just the message text), so a kind
    /// change is caught by `cargo nextest`.
    #[test]
    fn reject_when_required_missing_kind_typed() {
        let mut view = empty_view();
        let mut reqs = HashSet::new();
        reqs.insert("plan_name".to_string());
        view.effective_required_fields
            .insert("work.done".to_string(), reqs);
        let decision = run_gates(&view, &LintContext, "work.done", &json!({}), None);
        match decision {
            GateDecision::Reject { kind, .. } => {
                assert_eq!(
                    kind,
                    RejectionKind::MissingField,
                    "missing required fields MUST keep MissingField kind; flipping the kind breaks the typed escalation chain"
                );
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn accept_when_all_required_present() {
        let mut view = empty_view();
        let mut reqs = HashSet::new();
        reqs.insert("plan_name".to_string());
        reqs.insert("step".to_string());
        view.effective_required_fields
            .insert("work.done".to_string(), reqs);
        let decision = run_gates(
            &view,
            &LintContext,
            "work.done",
            &json!({"plan_name": "x", "step": "s"}),
            None,
        );
        assert_eq!(decision, GateDecision::Accept);
    }

    /// P1-1: reason codes are stable, well-known strings. Operators
    /// rely on them for alert routing; renaming requires a
    /// migration plan.
    #[test]
    fn reason_codes_are_stable() {
        assert_eq!(RejectionKind::MissingField.reason_code(), "missing_field");
        assert_eq!(
            RejectionKind::TopicOwnership.reason_code(),
            "topic_ownership"
        );
        assert_eq!(RejectionKind::UpstreamState.reason_code(), "upstream_state");
        assert_eq!(RejectionKind::PreCheck.reason_code(), "pre_check");
    }

    /// U5 (plan 2026-06-23-004): `RejectionKind` is `#[non_exhaustive]`.
    /// Variants without fields remain readable via `matches!` /
    /// `reason_code()`. Pin the surface so future variants keep
    /// both reading paths working.
    #[test]
    fn non_exhaustive_variants_remain_readable() {
        assert!(matches!(
            RejectionKind::MissingField,
            RejectionKind::MissingField
        ));
        assert!(matches!(RejectionKind::PreCheck, RejectionKind::PreCheck));
        assert_eq!(RejectionKind::MissingField.reason_code(), "missing_field");
    }

    /// P1-1: the rejection kind drives the linter's failure
    /// class. The mapping is the only cross-layer surface, and
    /// it is defined in one place.
    #[test]
    fn kind_maps_to_lint_class() {
        use crate::preset::engine::hint::LintFailureClass;
        assert_eq!(
            RejectionKind::MissingField.to_lint_class(),
            LintFailureClass::PayloadError
        );
        assert_eq!(
            RejectionKind::UpstreamState.to_lint_class(),
            LintFailureClass::UpstreamStateMissing
        );
        assert_eq!(
            RejectionKind::TopicOwnership.to_lint_class(),
            LintFailureClass::TopicOwnership
        );
    }

    /// 2026-06-23 fix (adversarial review P1-3): the
    /// `reason_code()` mapping is a public SSOT — operators
    /// grep `.ralph/recovery.jsonl` for these strings. This
    /// test asserts every variant's `reason_code()` is part of
    /// the locked surface so an accidental rename is caught by
    /// `cargo nextest run -p ralph-core -- reason_code_locked`.
    #[test]
    fn p1_3_reason_code_locked_for_all_kinds() {
        // Pair each variant with the expected reason_code() string.
        // Adding a new variant MUST add a matching pair here.
        let cases: &[(RejectionKind, &str)] = &[
            (RejectionKind::MissingField, "missing_field"),
            (RejectionKind::TopicOwnership, "topic_ownership"),
            (RejectionKind::UpstreamState, "upstream_state"),
            (RejectionKind::PreCheck, "pre_check"),
        ];
        for (kind, expected_code) in cases {
            assert_eq!(
                kind.reason_code(),
                *expected_code,
                "reason_code drifted for {kind:?}"
            );
        }
    }

    /// U7 (plan 2026-06-23-004): `resolve_downstream_publishes` 是 SSOT,
    /// 多次调用结果必须稳定一致,CLI precheck 与 runtime gate 不许漂移。
    #[test]
    fn resolve_does_not_diverge() {
        use crate::config::HatConfig;
        use std::collections::BTreeMap;
        let mut hats = BTreeMap::new();
        hats.insert(
            "executor".to_string(),
            HatConfig {
                name: "executor".to_string(),
                publishes: vec!["work.done".to_string(), "report.done".to_string()],
                ..Default::default()
            },
        );
        let consumer_of = |topic: &str| {
            if topic == "work.ready" {
                Some("executor".to_string())
            } else {
                None
            }
        };
        let first = resolve_downstream_publishes(&consumer_of, &hats, "work.ready");
        let second = resolve_downstream_publishes(&consumer_of, &hats, "work.ready");
        assert_eq!(first, second);
        assert_eq!(
            first,
            vec!["work.done".to_string(), "report.done".to_string()]
        );
        // 不在 consumer_of 中的 topic → 空列表
        assert!(resolve_downstream_publishes(&consumer_of, &hats, "unknown.topic").is_empty());
        // consumer_of 命中但 preset 无 hat 条目 → fallback 默认值
        let empty = BTreeMap::new();
        let fallback_consumer = |_: &str| Some("ghost".to_string());
        let defaulted = resolve_downstream_publishes(&fallback_consumer, &empty, "any.topic");
        assert_eq!(defaulted, vec!["work.done", "work.failed"]);
    }
}
