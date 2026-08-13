//! U8 (plan 2026-07-30-004): typed disposition classification.
//!
//! Every event topic is classified into one of four dispositions:
//!
//! - [`Disposition::Business`]: advances business flow
//!   (`work.done`, `work.failed`, `plan.complete`, `forge.*`, …).
//! - [`Disposition::Recovery`]: recovery / routing events
//!   (`task.resume`, `plan.blocked`, `fix.exhausted`, `*.rejected`, …).
//! - [`Disposition::DiagnosticObservation`]: diagnostic / observability
//!   events (`event.*`, `human.*`, `*.diagnostic`, `*.warning`) that do
//!   NOT advance flow.
//! - [`Disposition::LoopControl`]: loop lifecycle events
//!   (`LOOP_COMPLETE`, `REVIEW_COMPLETE`, `loop.cancel`, …) that do NOT
//!   trigger business consumers.
//!
//! # Routing contract
//!
//! Only `Business` and `Recovery` go through the Accepted Transition
//! API ([`AcceptedTransition::commit_idempotent`]): durable outbox
//! write + publish + phase-authority advance. `DiagnosticObservation`
//! and `LoopControl` use the explicit direct channel
//! ([`EventBus::publish`]) with no outbox entry and no phase-authority
//! advance — they are observations about the loop, not transitions of
//! business state.

use crate::event_loop::accepted_transition::{AcceptedTransition, OutboxEntry, TransitionError};
use crate::state::StateLedger;
use ralph_proto::{Event, EventBus};

/// The four dispositions an event topic can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// A business transition: advances flow, accepted ledger,
    /// task/progress/phase authority.
    Business,
    /// A recovery / blocked transition: routes the loop out of a
    /// stalled or rejected state. Advances flow like `Business`.
    Recovery,
    /// A diagnostic / observability notification. Published on the
    /// bus, but MUST NOT advance business flow or phase authority.
    DiagnosticObservation,
    /// A loop lifecycle control event. Published on the bus, but
    /// MUST NOT trigger business consumers.
    LoopControl,
}

impl Disposition {
    /// Whether this disposition advances business flow through the
    /// Accepted Transition API (durable outbox + phase authority).
    ///
    /// `Business` and `Recovery` advance flow; `DiagnosticObservation`
    /// and `LoopControl` are published via the explicit direct channel
    /// and never advance flow.
    pub const fn advances_flow(self) -> bool {
        matches!(self, Self::Business | Self::Recovery)
    }
}

/// Classify an event topic into its disposition.
///
/// The classifier is evaluated in priority order so more specific
/// rules win over broader ones:
///
/// 1. **LoopControl** — exact-match loop lifecycle topics
///    (`LOOP_COMPLETE`, `REVIEW_COMPLETE`, `loop.cancel`,
///    `loop.cancellation_requested`, `loop.terminate`,
///    `build.task.abandoned`).
/// 2. **DiagnosticObservation** — the `event.*` and `human.*`
///    namespaces, `*.diagnostic` / `*.warning` suffixes, and the
///    misrouted-resume telemetry topic. These never advance flow.
/// 3. **Recovery** — `task.resume` / `fallback.resume`, and the
///    `*.blocked` / `*.exhausted` / `*.rejected` families
///    (`plan.blocked`, `forge.plan.blocked`, `fix.exhausted`,
///    `review.wave.failed`'s companion `*.blocked`, …).
/// 4. **Business** — everything else (`work.*`, `plan.ready`,
///    `plan.complete`, `forge.*`, `exec.*`, `review.*`, `fix.done`,
///    `loop.stalled`, …).
///
/// The default is deliberately `Business`: an unrecognized topic fails
/// toward durability (it goes through the Accepted Transition) rather
/// than silently skipping the outbox. Diagnostic and control topics
/// are namespaced, so a new business topic cannot be misrouted to the
/// non-advancing channel.
pub fn classify(topic: &str) -> Disposition {
    // 1. LoopControl — exact loop-lifecycle topics only. `loop.stalled`
    //    is intentionally NOT here: it is a cumulative business signal
    //    (see the U1 ingress inventory), not lifecycle control.
    if matches!(
        topic,
        "LOOP_COMPLETE"
            | "REVIEW_COMPLETE"
            | "loop.cancel"
            | "loop.cancellation_requested"
            | "loop.terminate"
            | "build.task.abandoned"
    ) {
        return Disposition::LoopControl;
    }

    // 2. DiagnosticObservation — telemetry namespaces and suffixes.
    //    Checked before Recovery so `event.completion.blocked` /
    //    `event.state_machine.rejected` stay observations, and before
    //    the `task.resume` exact match so `task.resume.misrouted`
    //    (telemetry about a misrouted resume) is not a recovery route.
    if topic.starts_with("event.")
        || topic.starts_with("human.")
        || topic.ends_with(".diagnostic")
        || topic.ends_with(".warning")
        || topic == "task.resume.misrouted"
    {
        return Disposition::DiagnosticObservation;
    }

    // 3. Recovery — explicit resume routes plus the blocked /
    //    exhausted / rejected families that route the loop out of a
    //    stalled or failed state.
    if matches!(topic, "task.resume" | "fallback.resume")
        || topic.ends_with(".blocked")
        || topic.ends_with(".exhausted")
        || topic.ends_with(".rejected")
    {
        return Disposition::Recovery;
    }

    // 4. Business — the default (fail toward durability).
    Disposition::Business
}

/// Publish a synthetic event with disposition-aware routing.
///
/// - [`Disposition::Business`] / [`Disposition::Recovery`]: go through
///   [`AcceptedTransition::commit_idempotent`] (durable outbox +
///   publish). Returns `Ok(Some(entry))`.
/// - [`Disposition::DiagnosticObservation`] / [`Disposition::LoopControl`]:
///   direct [`EventBus::publish`] — no outbox entry, no phase-authority
///   advance. Returns `Ok(None)`.
///
/// The `Ok(None)` return is the caller's signal that the event took
/// the explicit non-advancing channel, so phase authority MUST NOT be
/// applied for it.
pub fn publish_synthetic(
    event: &Event,
    disposition: Disposition,
    loop_id: &str,
    activation_id: &str,
    contract_revision: &str,
    ledger: &StateLedger,
    bus: &mut EventBus,
) -> Result<Option<OutboxEntry>, TransitionError> {
    if disposition.advances_flow() {
        // Business / Recovery: durable outbox write + publish through
        // the Accepted Transition API (idempotent on replay).
        let entry = AcceptedTransition::commit_idempotent(
            event,
            loop_id,
            activation_id,
            contract_revision,
            ledger,
            bus,
            |_| Ok(()),
            || {},
        )?;
        Ok(Some(entry))
    } else {
        // DiagnosticObservation / LoopControl: explicit direct channel.
        // No outbox entry, no materialize, no phase-authority advance —
        // the event is an observation / lifecycle signal, not a
        // business state transition.
        bus.publish(event.clone());
        Ok(None)
    }
}

/// Plan GAP-02 / Unit 3 (U3-finish): same routing as
/// [`publish_synthetic`] but with an optional StateMachine projection
/// threaded into the durable outbox receipt. When the projection is
/// `Some`, the helper takes the `StateLedger` mutably so the projection
/// can be materialised to the ledger atomically *after* the outbox
/// write succeeds and *before* the bus publish (the crash-recovery
/// order pinned by the plan §3 fixed-order contract).
///
/// When the projection is `None`, this helper routes through the same
/// [`AcceptedTransition::commit_idempotent`] path as the legacy
/// `publish_synthetic` — preserving the U6/U7/U8 contract for every
/// non-StateMachine transition (the disabled / no-candidate path is
/// the common case today).
pub fn publish_synthetic_with_state_machine_projection(
    event: &Event,
    disposition: Disposition,
    loop_id: &str,
    activation_id: &str,
    contract_revision: &str,
    ledger: &mut StateLedger,
    bus: &mut EventBus,
    projection: Option<crate::state_machine::StateMachineTransitionDelta>,
) -> Result<Option<OutboxEntry>, TransitionError> {
    if disposition.advances_flow() {
        // Business / Recovery: durable outbox write + publish through
        // the Accepted Transition API (idempotent on replay).
        let entry = if projection.is_some() {
            // Projection present: take the projection-aware path that
            // commits the StateMachine delta to the ledger in addition
            // to the outbox receipt.
            AcceptedTransition::commit_idempotent_with_state_machine_projection(
                event,
                loop_id,
                activation_id,
                contract_revision,
                ledger,
                bus,
                |_| Ok(()),
                || Ok(Box::new(|| {}) as Box<dyn FnOnce()>),
                projection,
            )?
        } else {
            // No projection: legacy idempotent commit preserves
            // the U6/U7/U8 contract byte-for-byte.
            AcceptedTransition::commit_idempotent(
                event,
                loop_id,
                activation_id,
                contract_revision,
                ledger,
                bus,
                |_| Ok(()),
                || {},
            )?
        };
        Ok(Some(entry))
    } else {
        // DiagnosticObservation / LoopControl: explicit direct channel.
        // No outbox entry, no materialize, no phase-authority advance —
        // the event is an observation / lifecycle signal, not a
        // business state transition. Diagnostic / control topics never
        // carry a StateMachine projection so the projection argument is
        // intentionally ignored on this branch.
        bus.publish(event.clone());
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_loop::accepted_transition::read_outbox;
    use ralph_proto::Hat;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    /// Build a workspace with an empty [`StateLedger`] and an
    /// [`EventBus`] whose observer counts every routed event.
    fn fixture() -> (TempDir, StateLedger, EventBus, Arc<Mutex<usize>>) {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_path_buf();
        let ledger = StateLedger::new(&ws, true);

        // Register an `executor` hat so the EventBus source guard lets
        // sourced events through to observers/subscribers.
        let mut bus = EventBus::new();
        bus.register(Hat::new("executor", "Executor").subscribe("work.*"));

        let seen = Arc::new(Mutex::new(0usize));
        let seen_clone = Arc::clone(&seen);
        bus.add_observer(move |_| *seen_clone.lock().unwrap() += 1);

        (dir, ledger, bus, seen)
    }

    #[test]
    fn u8_disposition_classifier_categorizes_topics() {
        assert_eq!(classify("work.done"), Disposition::Business);
        assert_eq!(classify("work.failed"), Disposition::Business);
        assert_eq!(classify("plan.complete"), Disposition::Business);
        assert_eq!(classify("task.resume"), Disposition::Recovery);
        assert_eq!(classify("plan.blocked"), Disposition::Recovery);
        assert_eq!(
            classify("event.isolation.boundary_violation"),
            Disposition::DiagnosticObservation
        );
        assert_eq!(classify("LOOP_COMPLETE"), Disposition::LoopControl);
        assert_eq!(classify("loop.cancel"), Disposition::LoopControl);
    }

    #[test]
    fn u8_synthetic_business_goes_through_accepted_transition() {
        let (_dir, ledger, mut bus, seen) = fixture();
        let ws = ledger.workspace().to_path_buf();

        let event = Event::new("work.done", "implemented U8").with_source("executor");
        let result = publish_synthetic(
            &event,
            Disposition::Business,
            "loop-1",
            "act-1",
            "rev-1",
            &ledger,
            &mut bus,
        )
        .expect("business publish must succeed");

        // Business goes through the Accepted Transition: one durable
        // outbox entry, and the bus saw exactly one event.
        assert!(result.is_some(), "business must yield an outbox entry");
        let entries = read_outbox(&ws).unwrap();
        assert_eq!(entries.len(), 1, "outbox must have exactly 1 entry");
        assert_eq!(entries[0].topic, "work.done");
        assert_eq!(*seen.lock().unwrap(), 1, "bus must see exactly 1 event");
    }

    #[test]
    fn u8_diagnostic_does_not_advance_flow() {
        let (_dir, ledger, mut bus, seen) = fixture();
        let ws = ledger.workspace().to_path_buf();

        // No source: system-injected diagnostics bypass the EventBus
        // source guard (only set-but-unknown sources are dropped).
        let event = Event::new("event.isolation.boundary_violation", "hat=x scope=y");
        let result = publish_synthetic(
            &event,
            Disposition::DiagnosticObservation,
            "loop-1",
            "act-1",
            "rev-1",
            &ledger,
            &mut bus,
        )
        .expect("diagnostic publish must succeed");

        // Diagnostic takes the explicit direct channel: NO outbox
        // entry (so the Accepted Transition — and the phase-authority
        // advance the caller attaches to it — never ran), but the bus
        // DID see the event.
        assert!(
            result.is_none(),
            "diagnostic must not yield an outbox entry (no phase-authority advance)"
        );
        assert!(
            read_outbox(&ws).unwrap().is_empty(),
            "outbox must be empty for a diagnostic publish"
        );
        assert_eq!(
            *seen.lock().unwrap(),
            1,
            "bus must see the diagnostic via the explicit channel"
        );
    }

    #[test]
    fn u8_disposition_advances_flow_predicate() {
        assert!(Disposition::Business.advances_flow());
        assert!(Disposition::Recovery.advances_flow());
        assert!(!Disposition::DiagnosticObservation.advances_flow());
        assert!(!Disposition::LoopControl.advances_flow());
    }

    #[test]
    fn u8_classifier_covers_known_topic_vocabulary() {
        // Exhaustiveness sanity: every topic family observed in the
        // production source (per the U1 ingress inventory) classifies
        // into the expected disposition.
        let business = [
            "work.done",
            "work.failed",
            "work.ready",
            "work.start",
            "plan.ready",
            "plan.complete",
            "execution.plan.ready",
            "forge.plan.ready",
            "forge.wave.settled",
            "exec.unit.done",
            "exec.wave.complete",
            "review.dimension.ready",
            "review.wave.complete",
            "fix.done",
            "fix.unit.ready",
            "loop.stalled",
            "forge.report.done",
        ];
        for t in business {
            assert_eq!(classify(t), Disposition::Business, "topic {t}");
        }

        let recovery = [
            "task.resume",
            "fallback.resume",
            "plan.blocked",
            "forge.plan.blocked",
            "scope.blocked",
            "review.blocked",
            "experiment.blocked",
            "fix.exhausted",
            "recovery.exhausted",
            "repair.budget.exhausted",
            "work.done.rejected",
            "review.complete.rejected",
        ];
        for t in recovery {
            assert_eq!(classify(t), Disposition::Recovery, "topic {t}");
        }

        let diagnostic = [
            "event.isolation.boundary_violation",
            "event.malformed",
            "event.policy_warning",
            "event.post_terminal.rejected",
            "event.completion.blocked",
            "event.state_machine.rejected",
            "event.step_handoff.gate_rejected",
            "human.guidance",
            "task.resume.misrouted",
        ];
        for t in diagnostic {
            assert_eq!(classify(t), Disposition::DiagnosticObservation, "topic {t}");
        }

        let control = [
            "LOOP_COMPLETE",
            "REVIEW_COMPLETE",
            "loop.cancel",
            "loop.cancellation_requested",
            "loop.terminate",
            "build.task.abandoned",
        ];
        for t in control {
            assert_eq!(classify(t), Disposition::LoopControl, "topic {t}");
        }
    }
}
