//! U6: Recovery Responder — soft alerts, targeted retry, escalation.
//!
//! This module is the *only* place in the orchestrator that converts a
//! [`RecoveryDiagnosisEnvelope`] into a runtime action. The drift detector
//! (U5) is a pure signal source; U4 write paths funnel envelopes into
//! `recovery.jsonl`; U6 is the policy layer that decides what to do with
//! the diagnosis.
//!
//! # Three escalation levels
//!
//! | Level | Action | Preconditions | Regression guard |
//! |---|---|---|---|
//! | Soft | prompt alert only | new finding or attempt < `max_repeated_recoveries` | does not publish new events, does not change termination |
//! | Hard | targeted `task.resume` | `safe_target == true` and same `retry_key` already escalated | target must be a registered hat, must not re-fire on the source hat |
//! | Final | pause/terminate with report hint | no safe target OR retry window exhausted | never replaces an existing [`crate::event_loop::TerminationReason::PayloadContractViolation`] |
//!
//! The thresholds come from
//! [`crate::config::RuntimeDiagnosisConfig`]:
//!
//! - `max_repeated_recoveries` controls the Soft → Hard transition.
//! - `retry_window_iterations` controls the *forget-after-N-iterations*
//!   policy so old findings do not haunt a long-running loop forever.
//!
//! # Non-regression
//!
//! - The responder never panics and never blocks on I/O.
//! - The responder does **not** write to `recovery.jsonl` or
//!   `orchestration.jsonl` directly. The caller is expected to keep
//!   using [`crate::diagnostics::DiagnosticsCollector::log_recovery`]
//!   and `log_orchestration`; the responder is a pure in-memory
//!   aggregator.
//! - The responder does **not** create a new parallel termination
//!   system. Its `TerminationHint` is *advisory*: the loop runner is
//!   free to ignore it when an existing reason already explains the
//!   loop end (e.g. `PayloadContractViolation`).
//!
//! [`crate::event_loop::TerminationReason::PayloadContractViolation`]:
//!     crate::event_loop::TerminationReason::PayloadContractViolation

use std::collections::HashMap;
use std::sync::Arc;

use ralph_proto::HatId;
use serde::Serialize;

use super::envelope::{
    DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, RecoveryDiagnosisEnvelope,
};
use crate::config::RuntimeDiagnosisConfig;

/// Header for the prompt-injection block. Stable, machine-detectable,
/// distinct from `## ROBOT GUIDANCE` so the existing guidance detector
/// does not double-count it.
pub const RUNTIME_DIAGNOSIS_ALERT_HEADER: &str = "## Runtime Diagnosis Alert";

/// Maximum number of findings the responder will surface per prompt
/// even when the config allows more — a hard cap that protects the
/// prompt from a runaway drift storm. Configured via
/// [`RuntimeDiagnosisConfig::max_prompt_findings`]; this constant is
/// the *upper* bound (it must remain an internal sanity check, not a
/// user-facing knob).
const HARD_MAX_FINDINGS: usize = 32;

/// What level of escalation a single finding triggered this iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EscalationLevel {
    /// No new escalation — either a brand-new finding or a repeated
    /// finding still under the configured threshold. The caller
    /// should fold it into the prompt alert.
    Soft,
    /// Same `retry_key` was seen more than `max_repeated_recoveries`
    /// times in the retry window AND the envelope has a safe target
    /// hat. The caller should synthesize a targeted `task.resume`.
    Hard,
    /// No safe target OR the retry window has been exhausted. The
    /// caller should surface a `TerminationHint` so the loop runner
    /// can pause / report / escalate to human guidance.
    Final,
}

/// Per-iteration decision returned by [`RecoveryResponder::record_finding`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscalationDecision {
    /// The level this finding escalated to.
    pub level: EscalationLevel,
    /// The retry key the decision applies to.
    pub retry_key: String,
    /// Total attempt count for the same retry key (1-based, includes
    /// the current observation).
    pub attempt: u32,
    /// Recommended target hat for Hard escalation, when known.
    /// `None` for Soft and Final.
    pub target_hat: Option<String>,
    /// Reason string for Final escalation — used as a short
    /// `no_retry_reason` hint by the caller. `None` for Soft and Hard.
    pub reason: Option<String>,
}

/// The action a `Hard` escalation asks the caller to perform.
///
/// We keep this struct small and POD: the responder never touches the
/// `EventBus` directly. The runner takes the action and either
/// publishes it through `bus.publish` or feeds it into the existing
/// hard-gate path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryAction {
    /// The retry key the action targets.
    pub retry_key: String,
    /// Hat to route a `task.resume` event to.
    pub target_hat: HatId,
    /// Topic hint to include in the recovery event payload.
    pub topic_hint: Option<String>,
    /// The current attempt counter (1-based).
    pub attempt: u32,
    /// The current severity bucket.
    pub severity: DiagnosisSeverity,
}

/// Advisory hint for the loop runner. The runner is free to ignore
/// this when an existing termination reason already explains the
/// outcome — in particular, it must NOT replace
/// `TerminationReason::PayloadContractViolation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminationHint {
    /// Why the responder suggests pausing / terminating.
    pub reason: String,
    /// The retry key that triggered the hint, when applicable.
    pub retry_key: Option<String>,
    /// Severity that triggered the hint. The runner may use this to
    /// weight the hint (e.g. only final-escalate on Critical).
    pub severity: DiagnosisSeverity,
}

/// Per-retry-key state.
#[derive(Debug, Clone)]
struct RetryState {
    /// Number of times this retry key was observed. Includes the
    /// current observation.
    attempt_count: u32,
    /// Loop iteration the key was first observed at.
    first_iteration: u32,
    /// Loop iteration the key was last observed at.
    last_iteration: u32,
    /// Most recent severity seen.
    last_severity: DiagnosisSeverity,
    /// Most recent outcome recorded.
    last_outcome: DiagnosisOutcome,
    /// Optional target hat from the envelope, used for Hard
    /// escalation routing.
    target_hat: Option<String>,
    /// Optional topic hint for the recovery action payload.
    topic: Option<String>,
    /// Most recent source subsystem, used for the prompt alert and the
    /// orchestration audit event.
    source: DiagnosisSource,
    /// Whether the envelope had a safe target when last seen.
    safe_target: bool,
    /// Whether the responder has already escalated this key to Hard
    /// or Final in a previous iteration. Used to avoid re-emitting a
    /// `task.resume` every iteration once the threshold is crossed.
    escalated: bool,
}

impl RetryState {
    fn from_envelope(envelope: &RecoveryDiagnosisEnvelope) -> Self {
        Self {
            attempt_count: 1,
            first_iteration: envelope.iteration,
            last_iteration: envelope.iteration,
            last_severity: envelope.severity,
            last_outcome: envelope.outcome,
            target_hat: envelope.target_hat.clone(),
            topic: envelope.topic.clone(),
            source: envelope.source,
            safe_target: envelope.safe_target,
            escalated: false,
        }
    }
}

/// Soft, Hard, and Final escalation policy for runtime diagnosis
/// findings. Owned by [`crate::event_loop::EventLoop`]; the loop
/// runner feeds new envelopes into it via
/// [`crate::event_loop::EventLoop::record_recovery_envelope`].
#[derive(Debug)]
pub struct RecoveryResponder {
    /// Effective config (cloned per responder so config mutation
    /// during the run does not race the responder).
    config: Arc<RuntimeDiagnosisConfig>,
    /// Per-retry-key state.
    state: HashMap<String, RetryState>,
    /// Findings observed this iteration, retained for prompt
    /// injection. Cleared at the start of each iteration by
    /// [`Self::begin_iteration`].
    pending_findings: Vec<RecoveryDiagnosisEnvelope>,
    /// Retry keys that escalated to Hard in the most recent
    /// `record_finding` batch. The runner reads this to publish
    /// targeted `task.resume` events and clear the bit at the end of
    /// the iteration.
    last_hard_escalations: Vec<RecoveryAction>,
    /// `TerminationHint` produced by the most recent
    /// `record_finding` batch, if any. Cleared at the start of each
    /// iteration.
    last_termination_hint: Option<TerminationHint>,
}

impl RecoveryResponder {
    /// Construct a new responder. The config is shared via `Arc` so
    /// the rest of the loop can keep the same handle.
    #[must_use]
    pub fn new(config: Arc<RuntimeDiagnosisConfig>) -> Self {
        Self {
            config,
            state: HashMap::new(),
            pending_findings: Vec::new(),
            last_hard_escalations: Vec::new(),
            last_termination_hint: None,
        }
    }

    /// Read-only access to the effective config. Useful for tests
    /// and for the prompt builder.
    #[must_use]
    pub fn config(&self) -> &RuntimeDiagnosisConfig {
        &self.config
    }

    /// Open a new iteration. Clears the per-iteration `pending_findings`,
    /// `last_hard_escalations`, and `last_termination_hint` caches.
    /// The `state` map is preserved so cross-iteration aggregation
    /// (retry counters, recovery tracking) survives.
    pub fn begin_iteration(&mut self) {
        self.pending_findings.clear();
        self.last_hard_escalations.clear();
        self.last_termination_hint = None;
    }

    /// Number of distinct retry keys currently being tracked. Useful
    /// for unit tests and for the orchestration audit.
    #[must_use]
    pub fn tracked_retry_keys(&self) -> usize {
        self.state.len()
    }

    /// True when the responder has findings to fold into a prompt
    /// alert. Used by `apply_runtime_diagnosis_prompt` to skip the
    /// injection path entirely when the cache is empty.
    #[must_use]
    pub fn has_pending_findings(&self) -> bool {
        !self.pending_findings.is_empty()
    }

    /// Number of pending findings awaiting prompt injection.
    #[must_use]
    pub fn pending_finding_count(&self) -> usize {
        self.pending_findings.len()
    }

    /// Take the most recent hard-escalation actions. The runner
    /// publishes them as `task.resume` events and calls this again at
    /// the end of the iteration to clear the queue.
    pub fn drain_hard_escalations(&mut self) -> Vec<RecoveryAction> {
        std::mem::take(&mut self.last_hard_escalations)
    }

    /// Take the most recent termination hint, if any. Cleared by
    /// [`Self::begin_iteration`].
    pub fn take_termination_hint(&mut self) -> Option<TerminationHint> {
        self.last_termination_hint.take()
    }

    /// Record a new envelope and compute the escalation level for the
    /// current iteration. Pure in-memory operation: the caller is
    /// responsible for persisting the envelope via
    /// `DiagnosticsCollector::log_recovery` and emitting the audit
    /// event via `log_orchestration`.
    pub fn record_finding(
        &mut self,
        envelope: &RecoveryDiagnosisEnvelope,
        current_iteration: u32,
    ) -> EscalationDecision {
        let retry_key = envelope.retry_key.clone();
        let target_hat = envelope.target_hat.clone();
        let topic = envelope.topic.clone();
        let source = envelope.source;
        let safe_target = envelope.safe_target;
        let severity = envelope.severity;
        let attempt = self.observe(retry_key.clone(), envelope, current_iteration);
        // Stash the envelope for prompt injection this iteration.
        self.pending_findings.push(envelope.clone());

        let level = self.classify(&retry_key, current_iteration, safe_target);
        let mut decision = EscalationDecision {
            level,
            retry_key: retry_key.clone(),
            attempt,
            target_hat: None,
            reason: None,
        };
        match level {
            EscalationLevel::Soft => {}
            EscalationLevel::Hard => {
                if let Some(hat) = target_hat.clone() {
                    let action = RecoveryAction {
                        retry_key: retry_key.clone(),
                        target_hat: HatId::new(hat),
                        topic_hint: topic.clone(),
                        attempt,
                        severity,
                    };
                    self.last_hard_escalations.push(action);
                    decision.target_hat = Some(retry_key.clone());
                }
            }
            EscalationLevel::Final => {
                let reason = if safe_target {
                    format!(
                        "retry window exhausted for retry_key={retry_key} (>= {attempts} attempts within {window} iterations)",
                        attempts = self.config.max_repeated_recoveries,
                        window = self.config.retry_window_iterations,
                    )
                } else {
                    format!("no safe retry target for retry_key={retry_key}")
                };
                self.last_termination_hint = Some(TerminationHint {
                    reason: reason.clone(),
                    retry_key: Some(retry_key.clone()),
                    severity,
                });
                decision.reason = Some(reason);
                decision.target_hat = target_hat;
                let _ = source; // Reserved for future audit fields.
            }
        }
        decision
    }

    /// Returns the most recent attempt counter for `retry_key` (1-based),
    /// or 0 when the key has not been observed.
    #[must_use]
    pub fn attempt_count(&self, retry_key: &str) -> u32 {
        self.state.get(retry_key).map_or(0, |s| s.attempt_count)
    }

    /// True when the responder has a safe target hat for `retry_key`.
    /// Used by the runner before publishing a `task.resume` to avoid
    /// targeting a hat that is not registered.
    #[must_use]
    pub fn has_safe_target(&self, retry_key: &str) -> bool {
        self.state
            .get(retry_key)
            .is_some_and(|s| s.safe_target && s.target_hat.is_some())
    }

    /// Look up the recommended target hat for a retry key, when one
    /// is known. `None` means "no safe target" — the caller must NOT
    /// synthesize a fake target.
    #[must_use]
    pub fn target_hat_for_retry(&self, retry_key: &str) -> Option<String> {
        self.state
            .get(retry_key)
            .and_then(|s| s.target_hat.clone())
            .filter(|_| self.has_safe_target(retry_key))
    }

    /// Mark a `retry_key` as recovered when the next iteration's
    /// accepted events include the expected `topic`. Returns the
    /// resulting outcome when state was updated, or `None` when the
    /// key is not tracked.
    ///
    /// `accepted_topics` should be the set of topics that the runtime
    /// successfully accepted (i.e. passed through `EventPolicy`) in
    /// the iteration that just completed.
    pub fn check_recovery(
        &mut self,
        retry_key: &str,
        accepted_topics: &[String],
        current_iteration: u32,
    ) -> Option<DiagnosisOutcome> {
        let state = self.state.get_mut(retry_key)?;
        // A key is recovered when the topic the diagnosis was about
        // is now actually flowing through the bus. If the envelope
        // has no topic, we cannot decide and leave the state alone.
        if let Some(topic) = &state.topic {
            if accepted_topics.iter().any(|t| t == topic) {
                state.last_outcome = DiagnosisOutcome::Recovered;
                state.last_iteration = current_iteration;
                return Some(DiagnosisOutcome::Recovered);
            }
        } else if let Some(target) = &state.target_hat {
            // Topic-less envelopes (e.g. workflow guard) recover
            // when the next iteration's events include a publish on
            // the target hat's expected topics. We treat any
            // accepted event whose source matches the target as the
            // recovery signal.
            if accepted_topics.iter().any(|t| !t.is_empty()) {
                // The "any accepted topic" check is a coarse proxy
                // for the per-hat publish contract; the caller
                // should pass the per-hat accepted topics when
                // possible. The responder does not inspect the bus
                // itself, by design.
                state.last_outcome = DiagnosisOutcome::Recovered;
                state.last_iteration = current_iteration;
                let _ = target;
                return Some(DiagnosisOutcome::Recovered);
            }
        }
        // Still pending or repeated.
        if state.attempt_count > 1 {
            state.last_outcome = DiagnosisOutcome::Repeated;
        } else {
            state.last_outcome = DiagnosisOutcome::Pending;
        }
        state.last_iteration = current_iteration;
        Some(state.last_outcome)
    }

    /// Build the prompt-injection block. Returns a new prompt string
    /// with the alert appended (or unchanged when no findings apply).
    ///
    /// `hat_id` is the hat the prompt is being built for. In
    /// coordinator / solo mode the responder injects every finding;
    /// the helper caller passes `None` for those paths. In isolated
    /// mode, only findings whose `target_hat` (or `source_hat` when
    /// target is `None`) matches the given hat are surfaced — the
    /// plan requires "isolated hat mode 下 alert 只注入目标 hat".
    ///
    /// `current_iteration` is the loop iteration the prompt is being
    /// built for. The responder never injects a finding that was
    /// already marked [`DiagnosisOutcome::Recovered`].
    #[must_use]
    pub fn inject_prompt_alert(
        &self,
        prompt: &str,
        hat_id: Option<&HatId>,
        current_iteration: u32,
    ) -> String {
        if !self.config.enabled || !self.config.prompt_injection_enabled {
            return prompt.to_string();
        }
        if self.pending_findings.is_empty() {
            return prompt.to_string();
        }

        let max_chars = self.config.max_prompt_chars.max(1);
        let max_findings = self.config.max_prompt_findings.clamp(1, HARD_MAX_FINDINGS);

        // Filter findings for the current hat and recover status.
        // The state map is the source of truth for recovery: an
        // envelope whose state is `Recovered` is dropped even if the
        // original envelope in `pending_findings` still carries
        // `Pending`. The state map is updated by `check_recovery`.
        let mut eligible: Vec<&RecoveryDiagnosisEnvelope> = self
            .pending_findings
            .iter()
            .filter(|env| {
                self.state
                    .get(&env.retry_key)
                    .map(|s| s.last_outcome != DiagnosisOutcome::Recovered)
                    .unwrap_or(true)
            })
            .filter(|env| match hat_id {
                None => true, // coordinator / solo: surface all
                Some(hat) => {
                    let hat_str = hat.as_str();
                    env.target_hat.as_deref() == Some(hat_str)
                        || env.source_hat.as_deref() == Some(hat_str)
                }
            })
            .collect();
        // Stable order: by iteration then by retry_key.
        eligible.sort_by(|a, b| {
            a.iteration
                .cmp(&b.iteration)
                .then_with(|| a.retry_key.cmp(&b.retry_key))
        });
        eligible.truncate(max_findings);

        if eligible.is_empty() {
            return prompt.to_string();
        }

        let mut body = String::from(RUNTIME_DIAGNOSIS_ALERT_HEADER);
        body.push_str("\n\nThe runtime diagnosis layer observed the following issues in the recent loop. Address them in priority order before producing new work.\n\n");

        for env in &eligible {
            let line = format_finding_line(env, current_iteration);
            body.push_str("- ");
            body.push_str(&line);
            body.push('\n');
            if let Some(action) = &env.expected_action {
                body.push_str("  expected action: ");
                body.push_str(action);
                body.push('\n');
            }
        }

        body.push_str(
            "\nFull details in `.ralph/diagnostics/<session>/recovery.jsonl`. \
             Do NOT re-emit the same payload without addressing the diagnosis.\n",
        );

        let truncated = truncate_to_chars(&body, max_chars);
        if truncated.is_empty() {
            return prompt.to_string();
        }

        // Append the alert to the prompt. The recommended order (per
        // the plan) is: skills prefix → base prompt → phase section
        // → diagnosis alert. The caller is responsible for the
        // skills prefix; the alert goes after the phase section,
        // which is already inside `prompt` at this point.
        let mut out = String::with_capacity(prompt.len() + truncated.len() + 2);
        out.push_str(prompt);
        if !prompt.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
        out.push_str(&truncated);
        out
    }

    /// Mark the responder as escalated for `retry_key` (Hard or
    /// Final). Prevents repeated emission of the same Hard action on
    /// the next `record_finding` call.
    fn mark_escalated(&mut self, retry_key: &str) {
        if let Some(state) = self.state.get_mut(retry_key) {
            state.escalated = true;
        }
    }

    /// Update the in-memory state for `retry_key` and return the new
    /// attempt counter.
    fn observe(
        &mut self,
        retry_key: String,
        envelope: &RecoveryDiagnosisEnvelope,
        current_iteration: u32,
    ) -> u32 {
        let window = self.config.retry_window_iterations.max(1) as u32;
        let new_count = match self.state.get_mut(&retry_key) {
            None => {
                // First observation: seed the state. We do the
                // seeding here instead of via `Entry::or_insert_with`
                // so the `attempt_count` field can be set explicitly
                // (the constructor in `from_envelope` is shared with
                // callers that do not want to bump the counter).
                let mut state = RetryState::from_envelope(envelope);
                state.first_iteration = envelope.iteration;
                state.last_iteration = current_iteration;
                state.attempt_count = 1;
                self.state.insert(retry_key.clone(), state);
                1
            }
            Some(entry) => {
                // Stale-window reset: when the gap between current
                // and first observation exceeds the configured
                // window AND the key has not already escalated, treat
                // the new observation as a fresh start. This keeps
                // the state map bounded in long loops and prevents
                // old findings from haunting the responder.
                if current_iteration.saturating_sub(entry.first_iteration) > window
                    && !entry.escalated
                {
                    entry.attempt_count = 1;
                    entry.first_iteration = envelope.iteration;
                } else {
                    entry.attempt_count = entry.attempt_count.saturating_add(1);
                }
                entry.last_iteration = current_iteration;
                entry.last_severity = envelope.severity;
                entry.target_hat = envelope.target_hat.clone();
                entry.topic = envelope.topic.clone();
                entry.source = envelope.source;
                entry.safe_target = envelope.safe_target;
                entry.attempt_count
            }
        };
        new_count
    }

    /// Classify the current observation into a Soft / Hard / Final
    /// escalation level.
    fn classify(
        &mut self,
        retry_key: &str,
        current_iteration: u32,
        safe_target: bool,
    ) -> EscalationLevel {
        let max_repeats = self.config.max_repeated_recoveries.max(1) as u32;
        let window = self.config.retry_window_iterations.max(1) as u32;
        let (attempts, escalated) = self
            .state
            .get(retry_key)
            .map_or((0_u32, false), |s| (s.attempt_count, s.escalated));
        let over_threshold = attempts >= max_repeats;
        let over_window = current_iteration.saturating_sub(
            self.state
                .get(retry_key)
                .map_or(current_iteration, |s| s.first_iteration),
        ) >= window;
        if !over_threshold {
            return EscalationLevel::Soft;
        }
        if !safe_target {
            // No registered hat to route to. Pause / report / human
            // guidance instead of synthesizing a fake `task.resume`.
            self.mark_escalated(retry_key);
            return EscalationLevel::Final;
        }
        if over_window {
            self.mark_escalated(retry_key);
            return EscalationLevel::Final;
        }
        if escalated {
            // Already escalated; re-firing the same `task.resume`
            // every iteration would spam the bus. Stay at Soft so
            // the prompt alert still surfaces the finding.
            return EscalationLevel::Soft;
        }
        self.mark_escalated(retry_key);
        EscalationLevel::Hard
    }
}

/// Format a single finding line for the prompt alert.
fn format_finding_line(env: &RecoveryDiagnosisEnvelope, current_iteration: u32) -> String {
    let retry_attempt = env.retry_attempt.max(1);
    // Repeated findings keep the original attempt counter; the
    // caller already passed it in via `retry_attempt`.
    let attempt_for_state = retry_attempt;
    let topic = env.topic.as_deref().unwrap_or("*");
    let target = env.target_hat.as_deref().unwrap_or("*");
    let source_hat = env.source_hat.as_deref().unwrap_or("*");
    let severity = env.severity.as_str();
    format!(
        "[{severity}] source={source} target={target} topic={topic} hat={source_hat} attempt={n} iter={iter} — {msg}",
        severity = severity,
        source = env.source.as_str(),
        target = target,
        topic = topic,
        source_hat = source_hat,
        n = attempt_for_state,
        iter = current_iteration,
        msg = env.message,
    )
}

/// Truncate a string to at most `max_chars` characters, appending the
/// Unicode horizontal ellipsis when truncation happens. Returns the
/// input unchanged when it already fits.
fn truncate_to_chars(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    if total <= max_chars {
        return s.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('\u{2026}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DriftConfig, MalformedJsonlPolicy};

    fn cfg_with(max_repeats: usize, window: usize, prompt: bool) -> Arc<RuntimeDiagnosisConfig> {
        Arc::new(RuntimeDiagnosisConfig {
            enabled: true,
            write_artifacts: false,
            prompt_injection_enabled: prompt,
            max_prompt_findings: 5,
            max_prompt_chars: 2000,
            retry_window_iterations: window,
            max_repeated_recoveries: max_repeats,
            artifact_retention: 10,
            malformed_jsonl_policy: MalformedJsonlPolicy::Warn,
            drift: DriftConfig::default(),
        })
    }

    fn envelope(
        retry_key: &str,
        iteration: u32,
        severity: DiagnosisSeverity,
        safe_target: bool,
        target: Option<&str>,
        source: DiagnosisSource,
    ) -> RecoveryDiagnosisEnvelope {
        let mut b = RecoveryDiagnosisEnvelope::builder()
            .source(source)
            .severity(severity)
            .iteration(iteration)
            .reason_code("test")
            .message("m")
            .retry_key(retry_key)
            .safe_target(safe_target);
        if let Some(t) = target {
            b = b.target_hat(t);
        }
        b.build()
    }

    #[test]
    fn first_finding_is_soft() {
        let mut r = RecoveryResponder::new(cfg_with(3, 5, true));
        r.begin_iteration();
        let env = envelope(
            "k:builder:work_done:r:*",
            1,
            DiagnosisSeverity::Warning,
            true,
            Some("builder"),
            DiagnosisSource::MissingEventGate,
        );
        let d = r.record_finding(&env, 1);
        assert_eq!(d.level, EscalationLevel::Soft);
        assert_eq!(d.attempt, 1);
        assert!(r.has_pending_findings());
    }

    #[test]
    fn three_repeats_stay_soft_below_threshold() {
        let mut r = RecoveryResponder::new(cfg_with(3, 5, true));
        for i in 1..=2 {
            r.begin_iteration();
            let env = envelope(
                "k:builder:work_done:r:*",
                i,
                DiagnosisSeverity::Warning,
                true,
                Some("builder"),
                DiagnosisSource::MissingEventGate,
            );
            let d = r.record_finding(&env, i);
            assert_eq!(d.level, EscalationLevel::Soft, "iter {i}");
        }
        r.begin_iteration();
        let env = envelope(
            "k:builder:work_done:r:*",
            3,
            DiagnosisSeverity::Warning,
            true,
            Some("builder"),
            DiagnosisSource::MissingEventGate,
        );
        let d = r.record_finding(&env, 3);
        assert_eq!(d.level, EscalationLevel::Hard, "iter 3 should be Hard");
        assert_eq!(d.attempt, 3);
        let actions = r.drain_hard_escalations();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].target_hat.as_str(), "builder");
    }

    #[test]
    fn no_safe_target_skips_hard_and_surfaces_final() {
        let mut r = RecoveryResponder::new(cfg_with(2, 5, true));
        for i in 1..=2 {
            r.begin_iteration();
            let env = envelope(
                "k:ralph:*:stall:*",
                i,
                DiagnosisSeverity::Error,
                false,
                None,
                DiagnosisSource::StallRecovery,
            );
            let d = r.record_finding(&env, i);
            if i < 2 {
                assert_eq!(d.level, EscalationLevel::Soft);
            } else {
                assert_eq!(d.level, EscalationLevel::Final);
            }
        }
        let actions = r.drain_hard_escalations();
        assert!(actions.is_empty());
        let hint = r.take_termination_hint();
        assert!(hint.is_some());
        assert!(!hint.unwrap().reason.is_empty());
    }

    #[test]
    fn retry_window_exhaustion_raises_final_even_with_target() {
        let mut r = RecoveryResponder::new(cfg_with(3, 2, true));
        // Three iterations with the same key, the window of 2 means
        // the 3rd observation is far past the window.
        for i in 1..=3 {
            r.begin_iteration();
            let env = envelope(
                "k:builder:*:r:*",
                i,
                DiagnosisSeverity::Error,
                true,
                Some("builder"),
                DiagnosisSource::MissingEventGate,
            );
            let d = r.record_finding(&env, i);
            if i == 3 {
                assert_eq!(d.level, EscalationLevel::Final);
            } else {
                assert_eq!(d.level, EscalationLevel::Soft);
            }
        }
    }

    #[test]
    fn recovery_marks_outcome_and_drops_finding_from_prompt() {
        let mut r = RecoveryResponder::new(cfg_with(3, 5, true));
        r.begin_iteration();
        let env = envelope(
            "k:builder:work_done:r:*",
            1,
            DiagnosisSeverity::Warning,
            true,
            Some("builder"),
            DiagnosisSource::MissingEventGate,
        );
        let _ = r.record_finding(&env, 1);

        // Pretend the next iteration accepted the topic the envelope
        // was complaining about. `check_recovery` is the source of
        // truth for "this finding no longer needs a prompt alert".
        let outcome = r.check_recovery("k:builder:work_done:r:*", &["work.done".to_string()], 2);
        assert_eq!(outcome, Some(DiagnosisOutcome::Recovered));
        // The inject filter consults the state map, not the
        // envelope's outcome field, so the alert is dropped without
        // any extra `pending_findings.retain(...)` plumbing.
        let hat = HatId::new("builder");
        let prompt = r.inject_prompt_alert("base", Some(&hat), 2);
        assert!(!prompt.contains("Runtime Diagnosis Alert"));
    }

    #[test]
    fn isolated_hat_prompt_filters_unrelated_findings() {
        let mut r = RecoveryResponder::new(cfg_with(3, 5, true));
        r.begin_iteration();
        let builder_env = envelope(
            "k:builder:work_done:r:*",
            1,
            DiagnosisSeverity::Warning,
            true,
            Some("builder"),
            DiagnosisSource::MissingEventGate,
        );
        let planner_env = envelope(
            "k:planner:plan.x:r:*",
            1,
            DiagnosisSeverity::Warning,
            true,
            Some("planner"),
            DiagnosisSource::MissingEventGate,
        );
        let _ = r.record_finding(&builder_env, 1);
        let _ = r.record_finding(&planner_env, 1);
        let builder_hat = HatId::new("builder");
        let prompt = r.inject_prompt_alert("base", Some(&builder_hat), 1);
        assert!(prompt.contains("builder"));
        assert!(!prompt.contains("plan.x"));
    }

    #[test]
    fn prompt_injection_disabled_skips_alert() {
        let mut r = RecoveryResponder::new(cfg_with(3, 5, false));
        r.begin_iteration();
        let env = envelope(
            "k:builder:work_done:r:*",
            1,
            DiagnosisSeverity::Warning,
            true,
            Some("builder"),
            DiagnosisSource::MissingEventGate,
        );
        let _ = r.record_finding(&env, 1);
        let hat = HatId::new("builder");
        let prompt = r.inject_prompt_alert("base", Some(&hat), 1);
        assert_eq!(prompt, "base");
    }

    #[test]
    fn prompt_alert_truncated_to_max_chars() {
        let mut r = RecoveryResponder::new(Arc::new(RuntimeDiagnosisConfig {
            enabled: true,
            write_artifacts: false,
            prompt_injection_enabled: true,
            max_prompt_findings: 50,
            max_prompt_chars: 80,
            retry_window_iterations: 5,
            max_repeated_recoveries: 3,
            artifact_retention: 10,
            malformed_jsonl_policy: MalformedJsonlPolicy::Warn,
            drift: DriftConfig::default(),
        }));
        r.begin_iteration();
        for i in 0..5 {
            let env = envelope(
                &format!("k:builder:work_done:long_long_message:{i}"),
                1,
                DiagnosisSeverity::Warning,
                true,
                Some("builder"),
                DiagnosisSource::MissingEventGate,
            );
            let _ = r.record_finding(&env, 1);
        }
        let hat = HatId::new("builder");
        let prompt = r.inject_prompt_alert("base", Some(&hat), 1);
        // The alert body must be at most `max_prompt_chars` chars;
        // the helper adds a small separator (one or two newlines)
        // between the original prompt and the alert.
        let added = &prompt["base".len()..];
        // Find the start of the alert header to skip the separator.
        let alert_start = added
            .find(RUNTIME_DIAGNOSIS_ALERT_HEADER)
            .expect("alert header should be present");
        let alert_body = &added[alert_start..];
        assert!(
            alert_body.chars().count() <= 80,
            "alert body len = {}",
            alert_body.chars().count()
        );
        // Sanity: the body must end with the truncation ellipsis.
        assert!(alert_body.ends_with('\u{2026}'));
    }

    #[test]
    fn hard_escalation_does_not_re_fire_after_first_escalation() {
        let mut r = RecoveryResponder::new(cfg_with(2, 5, true));
        for i in 1..=3 {
            r.begin_iteration();
            let env = envelope(
                "k:builder:work_done:r:*",
                i,
                DiagnosisSeverity::Error,
                true,
                Some("builder"),
                DiagnosisSource::MissingEventGate,
            );
            let d = r.record_finding(&env, i);
            if i == 2 {
                assert_eq!(d.level, EscalationLevel::Hard);
            } else {
                // The 3rd iteration observes the same already-escalated
                // key. The responder stays at Soft because the
                // `task.resume` was already published.
                assert_eq!(d.level, EscalationLevel::Soft);
            }
        }
    }

    #[test]
    fn final_hint_does_not_include_payload_contract_violation_reason() {
        // The U6 plan explicitly forbids overwriting
        // `TerminationReason::PayloadContractViolation`. The
        // responder surfaces a *hint* (advisory) that the runner can
        // ignore. This test asserts the hint has no
        // payload-contract-specific reason; the runner contract
        // enforces the no-overwrite rule.
        let mut r = RecoveryResponder::new(cfg_with(1, 1, true));
        r.begin_iteration();
        let env = envelope(
            "k:ralph:*:r:*",
            1,
            DiagnosisSeverity::Error,
            false,
            None,
            DiagnosisSource::StallRecovery,
        );
        let _ = r.record_finding(&env, 1);
        let hint = r.take_termination_hint();
        assert!(hint.is_some());
        let hint = hint.unwrap();
        assert!(
            !hint.reason.contains("payload_contract"),
            "hint reason must not introduce a new termination reason: {}",
            hint.reason
        );
    }

    #[test]
    fn target_hat_for_retry_returns_none_when_no_safe_target() {
        let mut r = RecoveryResponder::new(cfg_with(2, 5, true));
        r.begin_iteration();
        let env = envelope(
            "k:ralph:*:r:*",
            1,
            DiagnosisSeverity::Error,
            false,
            Some("ralph"),
            DiagnosisSource::StallRecovery,
        );
        let _ = r.record_finding(&env, 1);
        assert!(r.target_hat_for_retry("k:ralph:*:r:*").is_none());
    }
}
