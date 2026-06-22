//! `ValidationRule` trait + `ValidationPipeline` executor (U4).
//!
//! All rules are stateless: they receive a snapshot of the
//! configuration (`ProtocolView`) and the loop state
//! (`LedgerSnapshot`) at validation time and return a single
//! [`ValidationResult`]. The trait intentionally does not take
//! `&mut self` so rules can be shared across threads and stored
//! as `Box<dyn ValidationRule>` without reentrancy concerns.
//!
//! ## Phase split (KTD-3)
//!
//! Each rule declares its [`RulePhase`]. The pipeline runs
//! PreCommit rules before speculative commit and PostCommit
//! rules after; a PostCommit rejection triggers rollback (the
//! caller decides whether to keep or drop the commit delta).

use std::sync::Arc;

use ralph_proto::HatId;

use crate::config::EventLoopConfig;
use crate::event_reader::Event;
use crate::preset::engine::protocol::ProtocolView;

use super::context::ValidationContext;
use super::result::{ValidationResult, ValidationStage};

/// Which phase a [`ValidationRule`] runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RulePhase {
    /// Run before speculative commit. The rule sees the current
    /// `LedgerSnapshot` and the inbound event.
    PreCommit,
    /// Run after speculative commit. The rule sees the projected
    /// snapshot plus the event. Failures here require a rollback.
    PostCommit,
}

/// Single-source-of-truth for validation rules.
///
/// Every gate in the existing event-loop stack
/// (`event_origin::validate_event_origin`, the engine
/// `required_fields` check, `execution_contract`,
/// `hat_handoff::gate::evaluate_event`,
/// `step_handoff::progress_task_gate`,
/// `workflow guard`) is wrapped as a `ValidationRule`
/// in [`super::rules_*`]. The trait stays intentionally narrow
/// (no clock, no IO, no mutable state) so rules are pure functions
/// of `(ProtocolView, LedgerSnapshot, Event)`.
///
/// **Threading**: every rule is `Send + Sync`. The pipeline stores
/// them behind `Arc<dyn ValidationRule>` for cheap cloning across
/// the event loop.
pub trait ValidationRule: Send + Sync {
    /// Stable stage name (matches [`ValidationStage`]).
    fn name(&self) -> &'static str;

    /// Which phase the rule runs in.
    fn applies_to(&self) -> RulePhase;

    /// Run the rule. Rules may mutate the snapshot through
    /// [`ValidationContext::snapshot_mut`] when they need to update
    /// per-event runtime state (e.g. event-policy dedup keys).
    fn validate(
        &self,
        protocol_view: &ProtocolView,
        ctx: &mut ValidationContext<'_>,
        event: &Event,
    ) -> ValidationResult;

    /// Optional commit metadata exposed to post-commit rules. The
    /// default `None` is fine for rules that don't need the delta.
    fn commit_delta(&self) -> Option<&CommitDeltaView<'_>> {
        None
    }
}

/// Lightweight view of the commit delta passed to PostCommit rules.
/// Carries the parsed topic, payload, and source hat extracted
/// from the event envelope — the full [`CommitDelta`] is too heavy
/// to pass around as a parameter to every rule.
///
/// Currently reserved for the U6 wiring that lifts the
/// `StateLedger::commit()` delta into PostCommit rules. The
/// default `ValidationRule::commit_delta()` returns `None` so
/// existing rule implementations are unaffected.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CommitDeltaView<'a> {
    pub topic: &'a str,
    pub payload: Option<&'a str>,
    pub source_hat: Option<&'a str>,
    pub source: Option<&'a str>,
    pub triggered: Option<&'a str>,
}

#[allow(dead_code)]
impl<'a> CommitDeltaView<'a> {
    /// Build a view from a parsed event envelope.
    pub fn from_event(event: &'a Event) -> Self {
        Self {
            topic: event.topic.as_str(),
            payload: event.payload.as_deref(),
            source_hat: event.hat.as_deref(),
            source: event.source.as_deref(),
            triggered: event.triggered.as_deref(),
        }
    }
}

/// The unified validation pipeline.
///
/// Owns the (immutable) rule list and the `ProtocolView` they
/// read. The pipeline itself is cheap to construct (rules are
/// Arc-shared) and is `Clone` so the event loop can hand a copy
/// to background threads (e.g. `BddScenarioRunner`) without
/// serialising validation.
pub struct ValidationPipeline {
    /// Pre-commit rules in pipeline order. `pub` so the
    /// `from_config` test can assert the rule count.
    pub pre_commit_rules: Vec<Arc<dyn ValidationRule>>,
    /// Post-commit rules in pipeline order. `pub` so the
    /// `from_config` test can assert the rule count.
    pub post_commit_rules: Vec<Arc<dyn ValidationRule>>,
    /// Mirror of `ProtocolView::feature_flag_enabled` at
    /// construction time. Kept on the pipeline so downstream code
    /// does not have to thread the `ProtocolView` everywhere just
    /// to ask whether the new path is active.
    pub feature_enabled: bool,
    /// Source hat used as the fallback for `publisher_allowed`
    /// checks. Populated from `LedgerSnapshot::current_isolated_hat`
    /// at construction time; the pipeline does NOT re-read the
    /// snapshot per-event — the caller is expected to clone with
    /// an updated hat when the isolated hat changes mid-loop.
    pub default_source_hat: Option<HatId>,
}
impl ValidationPipeline {
    /// Build the canonical pipeline from the runtime config and
    /// the protocol view.
    ///
    /// The `feature_enabled` flag mirrors
    /// `ProtocolView::feature_enabled()`. When `false`, the
    /// constructed pipeline still works (rules are pure) but the
    /// caller is expected to short-circuit to the legacy path.
    /// We do not gate rule construction on the flag because tests
    /// exercise both paths.
    pub fn from_config(protocol_view: &ProtocolView, _config: &EventLoopConfig) -> Self {
        // P1-#4 (002-adversarial-review): the pipeline's default
        // constructor still uses the empty `HatRegistry` (solo /
        // hatless mode) for the `OriginRule` because `EventLoopConfig`
        // does not carry hat definitions. Production callers that
        // have a `RalphConfig` in hand should follow up with
        // [`Self::with_origin_registry`] so the `OriginRule` runs
        // against the real registry. The pipeline stores the
        // registry behind an `Option` so this default path keeps
        // compiling for callers without a registry.
        Self::from_registry(protocol_view, None)
    }

    /// Build a pipeline with an explicit `HatRegistry` for
    /// `OriginRule`. P1-#4 (002-adversarial-review): this is the
    /// path production callers must take so unknown-hat events
    /// are rejected. The registry is `Arc`-shared so the rule
    /// can be cloned into multiple pipelines without re-reading
    /// the config.
    pub fn from_registry(
        protocol_view: &ProtocolView,
        registry: Option<std::sync::Arc<crate::hat_registry::HatRegistry>>,
    ) -> Self {
        use super::rules_event_policy::EventPolicyRule;
        use super::rules_execution_contract::ExecutionContractRule;
        use super::rules_hat_handoff::HatHandoffRule;
        use super::rules_origin::OriginRule;
        use super::rules_publisher::PublisherRule;
        use super::rules_required_fields::RequiredFieldsRule;
        use super::rules_step_handoff::StepHandoffRule;
        use super::rules_workflow_guard::WorkflowGuardRule;

        let origin_rule: Arc<dyn ValidationRule> = match registry {
            Some(reg) => Arc::new(OriginRule::with_registry(reg)),
            None => Arc::new(OriginRule::default()),
        };

        let pre_commit_rules: Vec<Arc<dyn ValidationRule>> = vec![
            origin_rule,
            Arc::new(PublisherRule),
            Arc::new(RequiredFieldsRule),
            Arc::new(EventPolicyRule),
            Arc::new(StepHandoffRule),
            Arc::new(HatHandoffRule),
        ];
        let post_commit_rules: Vec<Arc<dyn ValidationRule>> =
            vec![Arc::new(ExecutionContractRule), Arc::new(WorkflowGuardRule)];

        Self {
            pre_commit_rules,
            post_commit_rules,
            feature_enabled: protocol_view.feature_enabled(),
            default_source_hat: None,
        }
    }

    /// Override the default isolated hat used as fallback for
    /// publisher checks. The runtime calls this once per loop
    /// start (and again on isolated hat rotation).
    pub fn with_default_source_hat(mut self, hat: Option<HatId>) -> Self {
        self.default_source_hat = hat;
        self
    }
    /// Run the pre-commit rules against the current snapshot.
    /// Returns one [`ValidationResult`] per rule, in pipeline
    /// order. The caller decides what to do with rejections
    /// (typically short-circuit on the first one).
    ///
    /// P2-#2 (002-adversarial-review): the legacy
    /// `validate_pre_commit` (without view) used an empty
    /// `ProtocolView`, which silently disabled
    /// `RequiredFieldsRule` and other view-aware checks. The
    /// method is removed; callers must use
    /// [`Self::validate_pre_commit_with_view`] and supply the
    /// runtime's real `ProtocolView` instance.
    /// Run the pre-commit rules against the current snapshot
    /// with a caller-supplied `ProtocolView`. This is the
    /// production entry point — `process_parse_result` builds
    /// the view once per batch and passes it in so every rule
    /// sees the same configuration snapshot.
    pub fn validate_pre_commit_with_view(
        &self,
        view: &ProtocolView,
        ctx: &mut ValidationContext<'_>,
        event: &Event,
    ) -> Vec<ValidationResult> {
        self.pre_commit_rules
            .iter()
            .map(|rule| rule.validate(view, ctx, event))
            .collect()
    }

    /// Run the post-commit rules against the projected snapshot.
    /// The caller passes the *projected* snapshot (the
    /// speculative commit already applied) plus the event so
    /// execution-contract / workflow-guard can see the post-state.
    pub fn validate_post_commit(
        &self,
        view: &ProtocolView,
        ctx: &mut ValidationContext<'_>,
        event: &Event,
    ) -> Vec<ValidationResult> {
        self.post_commit_rules
            .iter()
            .map(|rule| rule.validate(view, ctx, event))
            .collect()
    }

    /// Run the full pipeline (pre + post) and produce a single
    /// [`ValidationReport`].
    ///
    /// **Conservative semantics** (per the U4 task spec): the
    /// pipeline does **not** mutate the caller's `LedgerSnapshot`.
    /// The caller is responsible for applying the speculative
    /// commit, then asking the pipeline for a verdict; on
    /// `accepted = false` the caller rolls the snapshot back.
    /// This avoids borrow-checker gymnastics around `&mut self`
    /// snapshots and keeps the pipeline trivially testable.
    pub fn validate_with_preview(
        &self,
        view: &ProtocolView,
        ctx: &mut ValidationContext<'_>,
        projected_ctx: &mut ValidationContext<'_>,
        event: &Event,
    ) -> ValidationReport {
        let pre_commit = self.validate_pre_commit_with_view(view, ctx, event);
        let post_commit = self.validate_post_commit(view, projected_ctx, event);
        let post_commit_rejected = post_commit.iter().any(|r| !r.accepted);
        let accepted =
            pre_commit.iter().all(|r| r.accepted) && post_commit.iter().all(|r| r.accepted);
        ValidationReport {
            pre_commit,
            post_commit,
            accepted,
            post_commit_rejected,
        }
    }
}

/// Result of [`ValidationPipeline::validate_with_preview`].
#[derive(Debug, Clone)]
pub struct ValidationReport {
    /// Pre-commit rule verdicts, in pipeline order.
    pub pre_commit: Vec<ValidationResult>,
    /// Post-commit rule verdicts, in pipeline order.
    pub post_commit: Vec<ValidationResult>,
    /// `true` iff every pre-commit and post-commit result was
    /// accepted. The caller should commit the delta only when
    /// this is `true`.
    pub accepted: bool,
    /// `true` iff at least one post-commit rule rejected. When
    /// `true`, the caller should roll the snapshot back to the
    /// pre-commit state (the post-commit rules saw the projected
    /// state, so the rejection is *post hoc*).
    pub post_commit_rejected: bool,
}

impl ValidationReport {
    /// Returns the first rejection in pre + post commit order, or
    /// `None` if every rule accepted. Convenience for callers that
    /// short-circuit on the first failure (matches the existing
    /// event-loop behaviour).
    pub fn first_rejection(&self) -> Option<&ValidationResult> {
        self.pre_commit
            .iter()
            .chain(self.post_commit.iter())
            .find(|r| !r.accepted)
    }
}
