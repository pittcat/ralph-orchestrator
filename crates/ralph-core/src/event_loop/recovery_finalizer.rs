//! `RecoveryFinalizer` — 2026-06-28 plan U9 (R9) — the unified
//! backstop for the recovery machinery.
//!
//! Stall escalation is owned by U6 + U8. The other "reminder
//! style" mechanisms (drift critical findings piling up;
//! repair_stream re-failing the same retry_key) all share
//! one failure mode: they emit a finding / re-emit a repair
//! hint, but never terminate the loop. The result is a tight
//! `Pending <-> Recovered` flip-flop, which is exactly the
//! 2026-06-28 diagnosis report's "no self-stop" finding.
//!
//! `RecoveryFinalizer` collects per-mechanism counts and
//! returns a terminal event when the count crosses the
//! configured `max_escalation_count`. The caller (drift
//! engine / repair stream) is responsible for actually
//! publishing the event onto the bus.
//!
//! Per the plan, the finalizer does NOT cover stall — stall
//! already has a dedicated stop path (U6 budget-exhausted +
//! U8 final-threshold). The finalizer covers drift + repair
//! stream and any future mechanism the same way.

use std::collections::HashMap;

/// Stable identifier for a "reminder-style" recovery
/// mechanism. Each variant tracks its own escalation count
/// and `max_escalation_count` (see [`RecoveryFinalizer::new`])
/// so that the per-mechanism thresholds can evolve
/// independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecoveryMechanism {
    /// Drift engine: critical findings piling up.
    Drift,
    /// Repair stream: same `retry_key` re-failing.
    RepairStream,
    /// Forward-compatible placeholder for future mechanisms.
    Custom(&'static str),
}

/// Configuration for a single mechanism.
#[derive(Debug, Clone)]
pub struct MechanismConfig {
    /// Threshold at which [`RecoveryFinalizer::record`] returns
    /// `Some(terminal)`. The terminal event is fired only on
    /// the *transition* across the threshold; subsequent
    /// `record` calls return `None` until [`Self::reset`] is
    /// called.
    pub max_escalation_count: u32,
    /// Topic to emit on terminal. Defaults to
    /// `plan.blocked`; the caller can override.
    pub final_outcome_topic: String,
    /// Reason suffix appended to the JSON payload. The
    /// final reason string is `<reason_suffix>_exhausted`
    /// (lowercased, snake-cased) so operators can grep on a
    /// stable string.
    pub reason_suffix: String,
}

impl MechanismConfig {
    /// Build a config with a 5-call threshold and the
    /// default `plan.blocked` topic.
    pub fn new(max_escalation_count: u32, reason_suffix: impl Into<String>) -> Self {
        Self {
            max_escalation_count,
            final_outcome_topic: "plan.blocked".to_string(),
            reason_suffix: reason_suffix.into(),
        }
    }
}

/// State for one mechanism. The `count` is bumped by
/// `record` and cleared by `reset`.
#[derive(Debug, Clone, Default)]
struct MechanismState {
    count: u32,
    terminal_emitted: bool,
}

/// The recovery finalizer. Pure in-memory state; no I/O.
/// Construct with [`RecoveryFinalizer::new`] and feed it
/// from each mechanism's "another reminder fired" callback.
#[derive(Debug, Clone)]
pub struct RecoveryFinalizer {
    /// Per-mechanism configuration.
    configs: HashMap<RecoveryMechanism, MechanismConfig>,
    /// Per-mechanism state (count + terminal flag).
    states: HashMap<RecoveryMechanism, MechanismState>,
}

/// Outcome of a [`RecoveryFinalizer::record`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalEvent {
    /// Topic to publish on (typically `plan.blocked`).
    pub topic: String,
    /// Stable reason code: e.g. `drift_exhausted`.
    pub reason: String,
    /// The mechanism that fired.
    pub mechanism: RecoveryMechanism,
    /// Number of reminders that accumulated before the
    /// threshold fired.
    pub count: u32,
}

impl RecoveryFinalizer {
    /// Build a new finalizer with the given per-mechanism
    /// configs. Mechanisms without a config are not tracked
    /// (their `record` calls return `None`).
    #[must_use]
    pub fn new(configs: HashMap<RecoveryMechanism, MechanismConfig>) -> Self {
        let mut states = HashMap::new();
        for k in configs.keys() {
            states.insert(*k, MechanismState::default());
        }
        Self { configs, states }
    }

    /// Record one reminder for `mechanism` and return
    /// `Some(TerminalEvent)` when the threshold fires
    /// (exactly once, until [`Self::reset`] is called).
    /// `None` otherwise.
    ///
    /// `key` is opaque to the finalizer — it is only used
    /// for diagnostics. The caller decides what counts as
    /// a "new" reminder (per-(topic, field), per-retry_key,
    /// etc.).
    pub fn record(
        &mut self,
        mechanism: RecoveryMechanism,
        _key: &str,
    ) -> Option<TerminalEvent> {
        let cfg = self.configs.get(&mechanism)?;
        let state = self.states.get_mut(&mechanism)?;
        state.count = state.count.saturating_add(1);
        if state.terminal_emitted {
            return None;
        }
        if state.count < cfg.max_escalation_count {
            return None;
        }
        state.terminal_emitted = true;
        let reason = format!("{}_exhausted", sanitize_reason(&cfg.reason_suffix));
        Some(TerminalEvent {
            topic: cfg.final_outcome_topic.clone(),
            reason,
            mechanism,
            count: state.count,
        })
    }

    /// Reset the counter and terminal flag for `mechanism`.
    /// Use this when a "healthy" observation arrives and the
    /// finalizer should forget the accumulated reminders.
    pub fn reset(&mut self, mechanism: RecoveryMechanism) {
        if let Some(state) = self.states.get_mut(&mechanism) {
            state.count = 0;
            state.terminal_emitted = false;
        }
    }

    /// Current count for `mechanism` (mainly for tests /
    /// diagnostics).
    #[must_use]
    pub fn count(&self, mechanism: RecoveryMechanism) -> u32 {
        self.states
            .get(&mechanism)
            .map_or(0, |s| s.count)
    }

    /// True when `mechanism` has already fired its terminal
    /// event and the caller has not yet reset it.
    #[must_use]
    pub fn is_terminal(&self, mechanism: RecoveryMechanism) -> bool {
        self.states
            .get(&mechanism)
            .is_some_and(|s| s.terminal_emitted)
    }
}

/// Lowercase + replace non-alphanumerics with `_` so the
/// final reason string is a stable snake_case identifier.
fn sanitize_reason(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finalizer_with(max: u32) -> RecoveryFinalizer {
        let mut configs = HashMap::new();
        configs.insert(
            RecoveryMechanism::Drift,
            MechanismConfig::new(max, "drift"),
        );
        configs.insert(
            RecoveryMechanism::RepairStream,
            MechanismConfig::new(max, "repair"),
        );
        RecoveryFinalizer::new(configs)
    }

    #[test]
    fn does_not_fire_below_threshold() {
        let mut f = finalizer_with(5);
        for _ in 0..4 {
            assert!(f.record(RecoveryMechanism::Drift, "k").is_none());
        }
        assert_eq!(f.count(RecoveryMechanism::Drift), 4);
    }

    #[test]
    fn fires_at_threshold_once() {
        let mut f = finalizer_with(3);
        assert!(f.record(RecoveryMechanism::Drift, "k").is_none());
        assert!(f.record(RecoveryMechanism::Drift, "k").is_none());
        let t = f
            .record(RecoveryMechanism::Drift, "k")
            .expect("third call fires");
        assert_eq!(t.topic, "plan.blocked");
        assert_eq!(t.reason, "drift_exhausted");
        assert_eq!(t.count, 3);
        // Subsequent calls stay silent (one-shot).
        assert!(f.record(RecoveryMechanism::Drift, "k").is_none());
        assert!(f.is_terminal(RecoveryMechanism::Drift));
    }

    #[test]
    fn reset_clears_count_and_terminal_flag() {
        let mut f = finalizer_with(2);
        let _ = f.record(RecoveryMechanism::Drift, "k");
        let _ = f.record(RecoveryMechanism::Drift, "k");
        assert!(f.is_terminal(RecoveryMechanism::Drift));
        f.reset(RecoveryMechanism::Drift);
        assert!(!f.is_terminal(RecoveryMechanism::Drift));
        assert_eq!(f.count(RecoveryMechanism::Drift), 0);
    }

    #[test]
    fn independent_mechanisms() {
        let mut f = finalizer_with(2);
        // Drift: 2nd call fires terminal.
        assert!(f.record(RecoveryMechanism::Drift, "k").is_none());
        let drift_t = f
            .record(RecoveryMechanism::Drift, "k")
            .expect("drift crosses threshold on 2nd call");
        assert_eq!(drift_t.reason, "drift_exhausted");
        // Repair stream: independent count, also fires on 2nd call.
        assert!(f.record(RecoveryMechanism::RepairStream, "k").is_none());
        let repair_t = f
            .record(RecoveryMechanism::RepairStream, "k")
            .expect("repair stream crosses threshold on its own count");
        assert_eq!(repair_t.reason, "repair_exhausted");
    }

    #[test]
    fn unknown_mechanism_is_silent() {
        let mut f = finalizer_with(1);
        assert!(f.record(RecoveryMechanism::Custom("ghost"), "k").is_none());
    }

    #[test]
    fn sanitize_reason_lowercases_and_replaces() {
        assert_eq!(sanitize_reason("Drift-Foo/Bar"), "drift_foo_bar");
    }
}
