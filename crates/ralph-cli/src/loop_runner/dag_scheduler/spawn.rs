//! 2026-09-03-0959 plan, wiring step E1 — the `dag`-mode execution
//! face of [`DagSchedulerRuntime`].
//!
//! The seam stops being observe-only here: for every admitted Unit
//! the runtime spawns a fenced per-stage job (executor → reviewer →
//! verifier, with the reviewer-REJECTED → fixer correction leg) and
//! merges the job's result event back into the main ledger.
//!
//! Invariants:
//!   - **Journal-fenced launches.** Every spawn is preceded by
//!     `reserve_job` on the durable store; a replayed reservation
//!     (`Ok(false)`) never launches a second process. The journal's
//!     transition chain (execute|fix+accepted → review,
//!     review+accepted → verify, review|verify+rejected|failed →
//!     fix@attempt+1) is the authority on legal stage flow.
//!   - **OPAC merge.** At most one business event per tick is
//!     appended to the main ledger (`merge_one`), matching the
//!     isolated-mode one-business-event-per-pass acceptance budget.
//!   - **Post-acceptance terminals.** A successful job's journal
//!     terminal is written only when the merged event comes back
//!     through the real `EventLoop` acceptance path
//!     (`awaiting_acceptance`). Failure terminals (timeout, empty
//!     channel, worktree failure) are written at drain time; the
//!     desync window between the two is a documented E3 concern.
//!   - **No wave-path effect.** Everything in this file is gated on
//!     `mode == Dag` plus an attached [`DagExecutionContext`]; wave
//!     and `dag_shadow` never reach it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ralph_adapters::CliBackend;
use ralph_core::EventLoop;
use ralph_core::config::{EventSchema, HatConfig, RalphConfig, SchedulerMode};
use ralph_core::supervisor::dag_store::StageEvidenceRecord;
use ralph_core::supervisor::dag_store_rusqlite::jobs::JobIdentity;
use ralph_core::supervisor::{EventMergeSink, FileEventMergeSink};
use serde_json::Value;
use sha2::Digest as _;
use tracing::{debug, warn};

use super::driver::{DagSchedulerDriver, DriverOutcome, ReviewVerdict, topics};
use super::jobs::AdvanceOutcome;
use super::worktree::UnitWorktree;
use super::{DagSchedulerRuntime, now_ms};
use crate::loop_runner::runtime_job::Stage;
use crate::loop_runner::runtime_job::environment::DagEnvPolicy;
use crate::loop_runner::runtime_job::pty_kernel::{
    PtyLeaseMode, PtySpawnSpec, drive_pty_lease_loop, finish_pty_job, spawn_pty_job,
};
use crate::loop_runner::wave::heartbeat::LeaseConfig;

/// Verify-stage topics the execution face consumes (the pipeline
/// driver intentionally does not: `Stage` ends at Verify).
pub(crate) mod topics_ext {
    pub const UNIT_VERIFIED: &str = "forge.unit.verified";
    pub const UNIT_VERIFICATION_FAILED: &str = "forge.unit.verification_failed";
}

/// Hat keys (preset `hats:` map) that provide the job templates.
const HAT_EXECUTOR: &str = "executor";
const HAT_REVIEWER: &str = "reviewer";
const HAT_VERIFIER: &str = "verifier";

/// Runtime-owned payload fields: the merge overwrites them from the
/// job identity, so a forged or stale agent value cannot misroute a
/// unit. Everything else in the schema's `required_fields` is the
/// agent's responsibility.
const RUNTIME_OWNED_FIELDS: [&str; 8] = [
    "unit_id",
    "plan_key",
    "task_id",
    "task_key",
    "job_id",
    "job_token",
    "stage",
    "attempt",
];

/// Host env names a DAG child may inherit (name-only policy; values
/// are never inspected). Backend credentials the operator declared
/// in config arrive via `CliBackend.env_vars`, which is appended
/// verbatim after this filter.
const DAG_ENV_ALLOWLIST: [&str; 25] = [
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TMPDIR",
    "TERM",
    "SSH_AUTH_SOCK",
    "XDG_RUNTIME_DIR",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    // Backend credential declarations (D23 curated set).
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "MOONSHOT_API_KEY",
];

/// Execution-scoped inputs `inner` attaches once the global backend
/// exists. Constructing this never spawns anything.
pub struct DagExecutionContext {
    /// Full preset hat map; job templates are looked up by name.
    hats: HashMap<String, HatConfig>,
    /// Fallback backend when a hat declares none.
    global_backend: CliBackend,
    pub(crate) loop_id: String,
    /// Main events ledger the merge sink appends to.
    pub(crate) main_events_file: PathBuf,
    hats_source_label: Option<String>,
    config_path: Option<PathBuf>,
    /// `event_policy.schemas` — the same-source required-fields /
    /// allowed-values contract the ledger acceptance gate enforces.
    schemas: HashMap<String, EventSchema>,
}

impl DagExecutionContext {
    pub fn new(
        config: &RalphConfig,
        global_backend: &CliBackend,
        loop_id: &str,
        main_events_file: PathBuf,
        hats_source_label: Option<String>,
    ) -> Self {
        Self {
            hats: config.hats.clone(),
            global_backend: global_backend.clone(),
            loop_id: loop_id.to_string(),
            main_events_file,
            hats_source_label,
            config_path: config.config_path.clone(),
            schemas: config
                .event_loop
                .event_policy
                .as_ref()
                .map(|policy| policy.schemas.clone())
                .unwrap_or_default(),
        }
    }

    pub(crate) fn timeout_for_hat(&self, hat: &str) -> Duration {
        self.hats
            .get(hat)
            .and_then(|config| config.timeout)
            .map_or(Duration::from_secs(3600), |seconds| {
                Duration::from_secs(u64::from(seconds))
            })
    }
}

/// A fenced job's exit report, delivered over the completion channel.
#[derive(Debug)]
pub(crate) struct JobCompletion {
    identity: JobIdentity,
    events_file: PathBuf,
    exit_code: Option<i32>,
    timed_out: bool,
}

impl JobCompletion {
    /// Build a completion during restart recovery. The worker event file is
    /// read by the normal drain path, so recovered results retain exactly the
    /// same schema and journal fencing as live jobs.
    pub(crate) fn recovered(identity: JobIdentity, events_file: PathBuf) -> Self {
        Self {
            identity,
            events_file,
            exit_code: Some(0),
            timed_out: false,
        }
    }
}

/// A completed result waiting for its OPAC merge slot.
#[derive(Debug)]
pub(crate) struct PendingMerge {
    pub(crate) event: ralph_proto::Event,
    /// Present when the producing job's journal terminal must be
    /// written once this event is accepted by the real EventLoop.
    post: Option<PostAcceptance>,
}

#[derive(Debug)]
pub(crate) struct PostAcceptance {
    identity: JobIdentity,
    terminal: &'static str,
    digest: String,
}

/// Accepted worker results must still carry the runtime-owned identity that
/// was bound to the durable reservation. Agent output cannot choose these
/// values; `complete_payload` overlays them before the event enters the
/// EventLoop acceptance path.
fn accepted_identity_matches(post: &PostAcceptance, payload: &Value, source: Option<&str>) -> bool {
    source == Some(post.identity.hat.as_str()) && payload_matches_identity(&post.identity, payload)
}

fn payload_matches_identity(identity: &JobIdentity, payload: &Value) -> bool {
    payload.get("job_id").and_then(Value::as_str) == Some(identity.job_id.as_str())
        && payload.get("job_token").and_then(Value::as_str) == Some(identity.token.as_str())
        && payload.get("stage").and_then(Value::as_str) == Some(identity.stage.as_str())
        && payload.get("attempt").and_then(Value::as_u64) == Some(u64::from(identity.attempt))
}

fn has_worker_identity(payload: &Value, source: Option<&str>) -> bool {
    source.is_some()
        || ["job_id", "job_token", "stage", "attempt"]
            .iter()
            .any(|field| payload.get(*field).is_some())
}

/// A driver advance deferred by a transient pool cap.
#[derive(Debug)]
pub(crate) struct PendingAdvance {
    topic: String,
    unit_key: String,
    payload: Value,
    source: Option<String>,
}

/// Strip the `forge:{plan_key}:` namespace from a runtime-supplied
/// unit_key back to its bare `unit_id`. The runtime now registers
/// Units under plan-namespaced keys (U1, 2026-09-09-0917) but the
/// durable `JobIdentity` still stores `unit_id` bare and computes
/// the namespaced `unit_key()` on demand. This helper is the
/// single place that bridges the two views; fall back to the input
/// when the prefix does not match (legacy callers still pass bare
/// unit_ids in non-namespaced modes).
fn strip_plan_namespace<'a>(plan_key: &str, unit_key: &'a str) -> &'a str {
    let prefix = format!("forge:{plan_key}:");
    unit_key.strip_prefix(&prefix).unwrap_or(unit_key)
}

/// A follow-up spawn deferred by the D16 fixer cap.
#[derive(Debug)]
pub(crate) struct PendingSpawn {
    pub(super) unit_key: String,
    pub(super) kind: SpawnKind,
    pub(super) attempt: u32,
    pub(super) plan_key: String,
    pub(super) feedback: Option<String>,
}

/// Which job to launch. `Fix` reuses the executor hat with the
/// reviewer's rejection feedback; its pipeline slot is the Review
/// stage (the pipeline `Stage` enum has no Fix variant), so the D16
/// fixer cap is enforced here via `fixer_in_flight`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpawnKind {
    Execute,
    Review,
    Verify,
    Fix,
}

impl SpawnKind {
    pub(crate) fn stage_str(self) -> &'static str {
        match self {
            Self::Execute => "execute",
            Self::Review => "review",
            Self::Verify => "verify",
            Self::Fix => "fix",
        }
    }

    pub(crate) fn hat(self) -> &'static str {
        match self {
            Self::Execute | Self::Fix => HAT_EXECUTOR,
            Self::Review => HAT_REVIEWER,
            Self::Verify => HAT_VERIFIER,
        }
    }

    /// Success topic the job is expected to emit.
    pub(crate) fn success_topic(self) -> &'static str {
        match self {
            Self::Execute | Self::Fix => topics::UNIT_EXECUTED,
            Self::Review => topics::UNIT_REVIEWED,
            Self::Verify => topics_ext::UNIT_VERIFIED,
        }
    }

    /// Agent-emitted failure topic, when the family defines one.
    pub(crate) fn failure_topic(self) -> &'static str {
        match self {
            Self::Execute | Self::Fix => "forge.unit.execution_failed",
            Self::Review => "forge.unit.execution_failed",
            Self::Verify => topics_ext::UNIT_VERIFICATION_FAILED,
        }
    }
}

/// Pure routing table: which job follows an accepted unit event.
/// Reviewer-REJECTED re-enters as a `Fix` job at the bumped attempt.
fn follow_up_kind(topic: &str, payload: &Value) -> Option<SpawnKind> {
    match topic {
        topics::UNIT_EXECUTED => Some(SpawnKind::Review),
        topics::UNIT_REVIEWED => match ReviewVerdict::from_payload(payload) {
            Some(ReviewVerdict::Accepted) => Some(SpawnKind::Verify),
            Some(ReviewVerdict::Rejected) => Some(SpawnKind::Fix),
            None => None,
        },
        _ => None,
    }
}

fn sha256_hex(input: &str) -> String {
    format!("{:x}", sha2::Sha256::digest(input.as_bytes()))
}

impl DagSchedulerRuntime {
    /// Reattach to a child that survived the supervisor restart. A PID is
    /// only adopted after the caller has positively probed it; the child is
    /// observed until it exits and then enters the normal completion drain.
    #[cfg(feature = "supervisor-db")]
    pub(crate) fn adopt_recovered_job(
        &mut self,
        identity: JobIdentity,
        pid: u32,
        events_file: PathBuf,
        timeout: Duration,
    ) -> bool {
        let Some(stage) = super::recovery::stage_from_str(&identity.stage) else {
            return false;
        };
        if !matches!(
            self.pipeline.advance(&identity.unit_key(), stage),
            AdvanceOutcome::Admitted { .. }
        ) {
            return false;
        }
        self.active_jobs.insert(identity.job_id.clone());
        if identity.stage == SpawnKind::Fix.stage_str() {
            self.fixer_in_flight = self.fixer_in_flight.saturating_add(1);
        }
        let tx = self.completion_tx.clone();
        tokio::spawn(async move {
            let started = Instant::now();
            loop {
                match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None) {
                    Ok(()) | Err(nix::errno::Errno::EPERM) => {}
                    Err(nix::errno::Errno::ESRCH) => {
                        let _ = tx.send(JobCompletion::recovered(
                            identity.clone(),
                            events_file.clone(),
                        ));
                        break;
                    }
                    Err(_) => {
                        let _ = tx.send(JobCompletion {
                            identity: identity.clone(),
                            events_file: events_file.clone(),
                            exit_code: None,
                            timed_out: true,
                        });
                        break;
                    }
                }
                if started.elapsed() >= timeout {
                    let _ = tx.send(JobCompletion {
                        identity: identity.clone(),
                        events_file: events_file.clone(),
                        exit_code: None,
                        timed_out: true,
                    });
                    break;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
        true
    }

    /// Settle a positively dead recovered child through the same typed
    /// failure event used by live jobs. No replacement launch is attempted.
    pub(crate) fn settle_recovered_dead_job(&mut self, identity: &JobIdentity) {
        let kind = match identity.stage.as_str() {
            "execute" => SpawnKind::Execute,
            "review" => SpawnKind::Review,
            "verify" => SpawnKind::Verify,
            "fix" => SpawnKind::Fix,
            _ => return,
        };
        let reason = format!(
            "recovered DAG job {} is no longer alive and has no result",
            identity.job_id
        );
        let digest = sha256_hex(&reason);
        self.write_terminal(identity, "failed", &digest);
        let payload = self.complete_payload(
            kind,
            identity,
            serde_json::json!({
                "reason": reason,
                "failure_class": "orphan_or_empty_result",
            }),
        );
        self.queue_failure_event(kind, identity, payload);
    }

    /// True while the execution face has anything in flight or
    /// queued. Drives the `inner` keepalive: a dag-mode loop with
    /// pending DAG work must not fall into fallback recovery.
    pub(crate) fn has_pending_work(&self) -> bool {
        !self.active_jobs.is_empty()
            || !self.merge_queue.is_empty()
            || !self.pending_advances.is_empty()
            || !self.pending_spawns.is_empty()
            || !self.pending_integrations.is_empty()
            || !self.awaiting_acceptance.is_empty()
    }

    /// Keepalive pump, called from `inner` when `next_hat()` finds
    /// no hat to activate (runtime-driven hats are suppressed in dag
    /// mode, so jobs would otherwise strand the loop into fallback
    /// termination).
    ///
    /// Returns `true` when the pump owns the iteration (work is
    /// pending): the caller resets the fallback counter, sleeps
    /// briefly, and continues. Returns `false` when the pump is
    /// inactive (not dag mode / no context / no work) or when a
    /// termination signal is pending — completion and cancellation
    /// flags are *peeked*, never consumed, so the regular
    /// `recover_late_events_before_fallback` path stays the single
    /// place that honors them.
    ///
    /// The pump runs its own ledger accept pass because
    /// `poll_for_late_events` discards accepted events; merged job
    /// results would otherwise never reach the seam.
    pub fn pump_idle(&mut self, event_loop: &mut EventLoop) -> bool {
        if self.mode != SchedulerMode::Dag || self.exec.is_none() {
            return false;
        }
        if event_loop.state().cancellation_requested || event_loop.state().completion_requested {
            return false;
        }
        self.drain_completions();
        self.merge_one();
        match event_loop.process_events_from_jsonl() {
            Ok(processed) => {
                for event in &processed.accepted_events {
                    let source = event.source.as_ref().map(|source| source.as_str());
                    self.route_event(event.topic.as_str(), &event.payload, source);
                }
            }
            Err(err) => {
                warn!(error = %err, "DAG pump: ledger read failed; skipping accept pass");
            }
        }
        self.tick();
        self.has_pending_work()
    }

    /// Drain finished jobs. Each completion either queues its result
    /// event for merge or records a failure terminal plus a
    /// synthesised `forge.unit.execution_failed`.
    fn drain_completions(&mut self) {
        while let Ok(completion) = self.completion_rx.try_recv() {
            self.active_jobs.remove(&completion.identity.job_id);
            if completion.identity.stage == SpawnKind::Fix.stage_str() {
                self.fixer_in_flight = self.fixer_in_flight.saturating_sub(1);
            }
            self.handle_completion(completion);
        }
    }

    fn handle_completion(&mut self, completion: JobCompletion) {
        let kind = match completion.identity.stage.as_str() {
            "execute" => SpawnKind::Execute,
            "review" => SpawnKind::Review,
            "verify" => SpawnKind::Verify,
            _ => SpawnKind::Fix,
        };
        let events = crate::loop_runner::wave::io::read_worker_events(&completion.events_file);
        let success = events.iter().find(|e| e.topic == kind.success_topic());
        let failure = events.iter().find(|e| e.topic == kind.failure_topic());

        if let Some(agent_event) = failure {
            // The agent reported the stage as failed. The event is
            // the business record; the journal terminal is written
            // now (post-acceptance tracking only covers success
            // topics the driver routes).
            let payload = agent_event
                .payload
                .as_deref()
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                .unwrap_or(Value::Null);
            let payload = self.complete_payload(kind, &completion.identity, payload);
            let digest = sha256_hex(&payload.to_string());
            self.write_terminal(&completion.identity, "failed", &digest);
            self.queue_failure_event(kind, &completion.identity, payload);
            return;
        }

        let Some(agent_event) = success else {
            let reason = if completion.timed_out {
                format!(
                    "dag job {} timed out without emitting {}",
                    completion.identity.job_id,
                    kind.success_topic()
                )
            } else {
                format!(
                    "dag job {} exited {:?} without emitting {}",
                    completion.identity.job_id,
                    completion.exit_code,
                    kind.success_topic()
                )
            };
            let failure_class = if completion.timed_out {
                "timeout"
            } else {
                "orphan_or_empty_result"
            };
            self.fail_job(&completion.identity, kind, &reason, failure_class);
            return;
        };

        if completion.timed_out {
            // A success event written before the kill is still the
            // authoritative result — mirror the wave worker, which
            // reads back events after a timeout.
            debug!(
                job_id = %completion.identity.job_id,
                "DAG drain: success event present despite lease kill; accepting it"
            );
        }

        let payload = agent_event
            .payload
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .unwrap_or(Value::Null);
        // The agent must supply every required field the runtime does
        // not own; a thin payload is a job failure, not a merge.
        let missing = self.missing_agent_fields(kind.success_topic(), &payload);
        if !missing.is_empty() {
            let reason = format!(
                "dag job {} emitted {} missing required field(s): {}",
                completion.identity.job_id,
                kind.success_topic(),
                missing.join(",")
            );
            self.fail_job(
                &completion.identity,
                kind,
                &reason,
                "orphan_or_empty_result",
            );
            return;
        }

        let payload = self.complete_payload(kind, &completion.identity, payload);
        let terminal: &'static str = match kind {
            SpawnKind::Review => match ReviewVerdict::from_payload(&payload) {
                Some(ReviewVerdict::Accepted) => "accepted",
                Some(ReviewVerdict::Rejected) => "rejected",
                None => {
                    let reason = format!(
                        "dag job {} emitted {} without a valid verdict",
                        completion.identity.job_id,
                        kind.success_topic()
                    );
                    self.fail_job(
                        &completion.identity,
                        kind,
                        &reason,
                        "orphan_or_empty_result",
                    );
                    return;
                }
            },
            _ => "accepted",
        };
        let serialized = payload.to_string();
        let digest = sha256_hex(&serialized);

        // U3 (2026-09-09-0917): record per-stage accepted evidence
        // BEFORE queuing the result event so a crash between
        // the projection and the next-stage admission still leaves
        // a recoverable next-stage base. Only "accepted" terminals
        // write evidence — rejected reviews and orphan failures
        // surface as a no-op so the next stage (Fix on a review
        // reject, etc.) keeps reading the prior accepted evidence.
        if terminal == "accepted" {
            self.record_accepted_evidence(kind, &completion.identity, &digest);
        }

        self.queue_result_event(
            kind.success_topic(),
            completion.identity.hat.clone(),
            payload,
            Some(PostAcceptance {
                identity: completion.identity.clone(),
                terminal,
                digest,
            }),
        );
    }

    /// Overlay runtime-owned identity fields onto the agent payload.
    fn complete_payload(&self, _kind: SpawnKind, identity: &JobIdentity, payload: Value) -> Value {
        let mut obj = payload.as_object().cloned().unwrap_or_default();
        obj.insert(
            "unit_id".to_string(),
            Value::String(identity.unit_id.clone()),
        );
        obj.insert(
            "plan_key".to_string(),
            Value::String(identity.plan_key.clone()),
        );
        obj.insert("task_key".to_string(), Value::String(identity.unit_key()));
        obj.insert(
            "task_id".to_string(),
            Value::String(self.resolve_task_id(&identity.unit_key())),
        );
        obj.insert("job_id".to_string(), Value::String(identity.job_id.clone()));
        obj.insert(
            "job_token".to_string(),
            Value::String(identity.token.clone()),
        );
        obj.insert("stage".to_string(), Value::String(identity.stage.clone()));
        obj.insert("attempt".to_string(), Value::from(identity.attempt));
        Value::Object(obj)
    }

    /// Schema-required fields minus the runtime-owned identity four.
    fn missing_agent_fields(&self, topic: &str, payload: &Value) -> Vec<String> {
        let Some(schema) = self.exec.as_ref().and_then(|exec| exec.schemas.get(topic)) else {
            return Vec::new();
        };
        schema
            .required_fields
            .iter()
            .filter(|f| !RUNTIME_OWNED_FIELDS.contains(&f.as_str()))
            .filter(|f| {
                match payload.get(f.as_str()) {
                    // Present and (for strings) non-empty.
                    Some(Value::String(s)) => s.is_empty(),
                    Some(_) => false,
                    None => true,
                }
            })
            .cloned()
            .collect()
    }

    /// Live task id for a `forge:<plan>:<unit>` task key; "unresolved"
    /// when the store lookup fails so the failure stays visible
    /// instead of wedging silently.
    pub(crate) fn resolve_task_id(&self, task_key: &str) -> String {
        let Some(exec) = self.exec.as_ref() else {
            return "unresolved".to_string();
        };
        let path = self
            .workspace
            .join(".ralph")
            .join("agent")
            .join("tasks.jsonl");
        ralph_core::TaskStore::load(&path)
            .ok()
            .and_then(|store| {
                store
                    .get_by_key_in_loop(task_key, Some(&exec.loop_id))
                    .map(|task| task.id.clone())
            })
            .unwrap_or_else(|| "unresolved".to_string())
    }

    /// Failure terminal at drain time + synthesised failure event.
    fn fail_job(
        &mut self,
        identity: &JobIdentity,
        kind: SpawnKind,
        reason: &str,
        failure_class: &str,
    ) {
        let digest = sha256_hex(reason);
        self.write_terminal(identity, "failed", &digest);
        let payload = serde_json::json!({
            "reason": reason,
            "failure_class": failure_class,
        });
        let payload = self.complete_payload(kind, identity, payload);
        self.queue_failure_event(kind, identity, payload);
    }

    /// Queue a failure business event with the runtime-owned identity fields
    /// applied. Keeping this separate lets restart recovery mirror a dead
    /// child through the same failure path without fabricating a worker file.
    pub(crate) fn queue_failure_event(
        &mut self,
        kind: SpawnKind,
        identity: &JobIdentity,
        payload: Value,
    ) {
        self.queue_result_event(kind.failure_topic(), identity.hat.clone(), payload, None);
    }

    pub(crate) fn queue_result_event(
        &mut self,
        topic: &str,
        hat: String,
        payload: Value,
        post: Option<PostAcceptance>,
    ) {
        let event = ralph_proto::Event::new(topic, payload.to_string()).with_source(hat);
        self.merge_queue.push_back(PendingMerge { event, post });
    }

    /// OPAC merge: at most one queued event per tick reaches the
    /// main ledger.
    fn merge_one(&mut self) {
        let Some(exec) = self.exec.as_ref() else {
            return;
        };
        let Some(pending) = self.merge_queue.pop_front() else {
            return;
        };
        let sink = FileEventMergeSink::new(&exec.main_events_file);
        match sink.append_events(vec![pending.event.clone()]) {
            Ok(()) => {
                if let Some(post) = pending.post {
                    self.awaiting_acceptance
                        .insert(post.identity.unit_key(), post);
                }
            }
            Err(err) => {
                warn!(error = %err, "DAG merge: ledger append failed; event requeued");
                self.merge_queue.push_front(pending);
            }
        }
    }

    /// Write a journal terminal. Fail-closed: a rejected write is a
    /// diagnostic, never a panic (the reservation state machine may
    /// legitimately refuse a double terminal on replay).
    fn write_terminal(&mut self, identity: &JobIdentity, terminal: &'static str, digest: &str) {
        let Some(journal) = self.journal() else {
            return;
        };
        if let Err(err) = journal.accept_job_terminal(identity, terminal, digest, now_ms() as i64) {
            warn!(
                job_id = %identity.job_id,
                terminal,
                error = %err,
                "DAG journal: terminal write rejected"
            );
        }
    }

    #[cfg(feature = "supervisor-db")]
    pub(crate) fn journal(
        &mut self,
    ) -> Option<std::sync::Arc<ralph_core::supervisor::dag_store_rusqlite::RusqliteDagSchedulerStore>>
    {
        match self.ensure_stores() {
            Ok(stores) => stores.journal.clone(),
            Err(err) => {
                warn!(error = %err, "DAG journal: store open failed");
                None
            }
        }
    }

    #[cfg(not(feature = "supervisor-db"))]
    fn journal(&mut self) -> Option<std::sync::Arc<()>> {
        None
    }

    /// `dag`-mode unit-event handling: journal terminal for the
    /// completing job, slot release, then the driver routing that
    /// admits the next stage.
    pub(super) fn observe_unit_event_dag(
        &mut self,
        topic: &str,
        unit_key: &str,
        payload: &Value,
        source: Option<&str>,
    ) {
        let acceptance_key = payload
            .get("plan_key")
            .and_then(Value::as_str)
            .filter(|plan_key| !plan_key.trim().is_empty())
            .map(|plan_key| format!("forge:{plan_key}:{unit_key}"))
            .unwrap_or_else(|| unit_key.to_string());
        if let Some(post) = self.awaiting_acceptance.get(&acceptance_key)
            && !accepted_identity_matches(post, payload, source)
        {
            warn!(
                unit_key,
                topic, "DAG seam: accepted event identity does not match the current job"
            );
            return;
        }
        if !self.awaiting_acceptance.contains_key(&acceptance_key)
            && has_worker_identity(payload, source)
            && !self.current_job_matches_payload(payload, source)
        {
            warn!(
                unit_key,
                topic, "DAG seam: worker result does not match the durable current job"
            );
            return;
        }
        // Verify-stage completions are not driver topics: the
        // pipeline ends at Verify (integration is a later step), so
        // the seam only settles the job and frees the slot.
        if topic == topics_ext::UNIT_VERIFIED || topic == topics_ext::UNIT_VERIFICATION_FAILED {
            if let Some(post) = self.awaiting_acceptance.remove(&acceptance_key) {
                self.write_terminal(&post.identity, post.terminal, &post.digest);
            }
            self.pipeline.release(unit_key);
            debug!(unit_key, topic, "DAG seam: verify-stage result settled");
            // E2: a passed Verify stage queues the unit for lane
            // integration; verification_failed settles without one.
            if topic == topics_ext::UNIT_VERIFIED {
                let plan_key = payload
                    .get("plan_key")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.queue_integration(&plan_key, unit_key);
            }
            return;
        }

        // The completing job's terminal is written now — the merged
        // event just came back through real EventLoop acceptance,
        // which is exactly the journal's post-acceptance contract.
        if let Some(post) = self.awaiting_acceptance.remove(&acceptance_key) {
            self.write_terminal(&post.identity, post.terminal, &post.digest);
        }
        // Free the completing job's slot BEFORE the driver admits
        // the next stage, so the admission is a fresh reservation
        // (not a migration) and cap accounting reflects reality.
        self.pipeline.release(unit_key);

        let outcome = {
            let mut driver = DagSchedulerDriver::new(&mut self.pipeline);
            driver.observe_accepted(topic, unit_key, payload)
        };
        match outcome {
            DriverOutcome::Routed { token, .. } => {
                let Some(kind) = follow_up_kind(topic, payload) else {
                    debug!(
                        unit_key,
                        topic, "DAG seam: routed event has no follow-up job"
                    );
                    return;
                };
                let plan_key = payload
                    .get("plan_key")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let feedback = if kind == SpawnKind::Fix {
                    payload
                        .get("review_report_path")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                } else {
                    None
                };
                let attempt = u32::try_from(token.attempt()).unwrap_or(u32::MAX);
                self.queue_spawn(PendingSpawn {
                    unit_key: unit_key.to_string(),
                    kind,
                    attempt,
                    plan_key,
                    feedback,
                });
            }
            DriverOutcome::Blocked { unit_key, error } => match error {
                // Transient caps: retry on the next tick.
                crate::loop_runner::runtime_job::RuntimeJobError::PoolExhausted { .. }
                | crate::loop_runner::runtime_job::RuntimeJobError::GlobalCapExceeded { .. } => {
                    debug!(unit_key, error = %error, "DAG seam: advance deferred by pool cap");
                    self.pending_advances.push_back(PendingAdvance {
                        topic: topic.to_string(),
                        unit_key,
                        payload: payload.clone(),
                        source: source.map(str::to_string),
                    });
                }
                other => {
                    warn!(
                        unit_key,
                        error = %other,
                        "DAG seam: pipeline blocked the unit event"
                    );
                    // Fix-attempt budget exhausted (or illegal
                    // transition): synthesize the failure record.
                    let plan_key = payload
                        .get("plan_key")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    // U1 (2026-09-09-0917): pipeline callers pass
                    // plan-namespaced `unit_key`s; `JobIdentity.unit_id`
                    // stays bare and computes `unit_key()` on demand.
                    // `job_id` / `token` must stay within the bounded
                    // identity charset (`[A-Za-z0-9_-]` — `:` is
                    // rejected) so they reuse the bare `unit_id`.
                    let bare_unit_id = strip_plan_namespace(&plan_key, &unit_key);
                    let identity = JobIdentity {
                        plan_key: plan_key.clone(),
                        unit_id: bare_unit_id.to_string(),
                        job_id: format!("dag-{bare_unit_id}-blocked"),
                        hat: HAT_EXECUTOR.to_string(),
                        stage: "fix".to_string(),
                        attempt: 0,
                        token: format!("tok-{bare_unit_id}-blocked"),
                    };
                    let reason = format!("unit {unit_key} blocked: {other}");
                    self.fail_job(&identity, SpawnKind::Fix, &reason, "unknown");
                }
            },
            DriverOutcome::StillExecuting { unit_key, stage } => {
                debug!(unit_key, ?stage, "DAG seam: unit still executing");
            }
            DriverOutcome::Ignored { topic } => {
                debug!(topic, "DAG seam: unit event ignored by driver");
            }
        }
    }

    fn current_job_matches_payload(&mut self, payload: &Value, source: Option<&str>) -> bool {
        let Some(plan_key) = payload.get("plan_key").and_then(Value::as_str) else {
            return false;
        };
        let Some(unit_id) = payload.get("unit_id").and_then(Value::as_str) else {
            return false;
        };
        let Some(journal) = self.journal() else {
            return false;
        };
        let Ok(jobs) = journal.list_jobs(plan_key) else {
            return false;
        };
        jobs.iter().rev().any(|job| {
            job.identity.unit_id == unit_id
                && job.terminal.is_none()
                && source == Some(job.identity.hat.as_str())
                && payload_matches_identity(&job.identity, payload)
        })
    }

    /// Spawn a follow-up job now, or defer it when the D16 fixer cap
    /// is saturated.
    pub(super) fn queue_spawn(&mut self, pending: PendingSpawn) {
        if pending.kind == SpawnKind::Fix
            && self.fixer_in_flight >= self.pipeline.pools().fixer_cap()
        {
            debug!(
                unit_key = %pending.unit_key,
                "DAG seam: fixer pool saturated; spawn deferred"
            );
            self.pending_spawns.push_back(pending);
            return;
        }
        self.spawn_job(pending);
    }

    /// Retry cap-deferred advances and fixer spawns.
    pub(super) fn retry_pending(&mut self) {
        let advances = std::mem::take(&mut self.pending_advances);
        for pending in advances {
            self.observe_unit_event_dag(
                &pending.topic,
                &pending.unit_key,
                &pending.payload,
                pending.source.as_deref(),
            );
        }
        let spawns = std::mem::take(&mut self.pending_spawns);
        for pending in spawns {
            self.queue_spawn(pending);
        }
    }

    /// Admission pass: an Admitted Ready unit gets its Execute job.
    /// `executors_launched` dedups the pure snapshot (a Ready unit
    /// is re-reported every tick until it integrates).
    pub(super) fn maybe_spawn_executor(&mut self, plan_key: &str, unit_id: &str) {
        // U1 (2026-09-09-0917): pipeline callers must match the
        // namespaced `unit_key` the runtime registered Units under,
        // otherwise `advance` returns `Blocked("unit not registered")`.
        let unit_key = format!("forge:{plan_key}:{unit_id}");
        if self.executors_launched.contains(&unit_key) {
            return;
        }
        match self.pipeline.advance(&unit_key, Stage::Execute) {
            AdvanceOutcome::Admitted { token } => {
                let attempt = u32::try_from(token.attempt()).unwrap_or(0);
                self.spawn_job(PendingSpawn {
                    unit_key,
                    kind: SpawnKind::Execute,
                    attempt,
                    plan_key: plan_key.to_string(),
                    feedback: None,
                });
            }
            other => {
                debug!(unit_id, ?other, "DAG seam: executor admission deferred");
            }
        }
    }

    /// Resolve the base commit the next stage must build on
    /// (U3, 2026-09-09-0917 plan). The lookup walks three
    /// durable surfaces in order:
    ///
    /// 1. The just-prior stage's accepted evidence (the
    ///    `(plan_key, unit_key)` row in `dag_stage_evidence` for
    ///    the stage immediately preceding `kind` in the
    ///    execute → review → verify → fix graph). When found,
    ///    that accepted commit is the new base — the next stage
    ///    lands its work on top of the just-accepted head
    ///    instead of plan HEAD.
    /// 2. The existing per-Unit base pin in `dag_unit_bases`.
    ///    Already pinned on a prior admission; reuse it so two
    ///    admissions of the same stage always start from the
    ///    same commit.
    /// 3. The plan-level `verified_base_commit` (the legacy
    ///    surface). When used, also `pin_unit_base` so the next
    ///    stage finds a per-Unit pin instead of re-reading plan
    ///    HEAD.
    ///
    /// Returns the resolved base commit, or an error string
    /// suitable for `fail_job` when nothing pins the unit and
    /// the plan also has no verified base.
    fn resolve_stage_base(
        &mut self,
        plan_key: &str,
        unit_key: &str,
        kind: SpawnKind,
    ) -> std::result::Result<String, String> {
        let prior_stage = match kind {
            SpawnKind::Execute => None,
            SpawnKind::Review => Some("execute"),
            SpawnKind::Verify => Some("review"),
            SpawnKind::Fix => Some("execute"),
        };
        if let Some(prior) = prior_stage {
            if let Ok(stores) = self.ensure_stores() {
                if let Ok(Some(ev)) = stores
                    .plans
                    .latest_stage_evidence(plan_key, unit_key, prior)
                {
                    // The prior stage's accepted commit is the
                    // authoritative base for the next stage.
                    // We deliberately do NOT re-pin
                    // `dag_unit_bases` here — the next stage's
                    // worker is expected to start from this
                    // exact commit and build on top of it; the
                    // pin stays at the FIRST admission's base
                    // so a crash between spawn and the worker's
                    // first commit can recover from the same
                    // base the spawn chose.
                    return Ok(ev.accepted_commit);
                }
            } else {
                warn!(
                    plan_key,
                    unit_key, prior, "DAG evidence: store open failed; skipping prior-stage lookup"
                );
            }
        }
        if let Ok(stores) = self.ensure_stores() {
            if let Ok(Some(pin)) = stores.plans.get_unit_base(plan_key, unit_key) {
                return Ok(pin.base_commit);
            }
        }
        let plan_base = self
            .plans
            .get(plan_key)
            .and_then(|p| p.verified_base_commit.clone());
        match plan_base {
            Some(base) => {
                if let Ok(stores) = self.ensure_stores() {
                    let _ = stores
                        .plans
                        .pin_unit_base(plan_key, unit_key, &base, now_ms());
                }
                Ok(base)
            }
            None => Err("no verified base commit recorded at approval".to_string()),
        }
    }

    /// Record the per-stage accepted evidence (U3,
    /// 2026-09-09-0917 plan). Called by `handle_completion`
    /// immediately before queueing the success result event;
    /// the unit's branch tip (after the worker committed) is
    /// the `accepted_commit`, the `base_commit` is what the
    /// stage actually descended from, and `evidence_token` /
    /// `evidence_fingerprint` bind the resume identity.
    ///
    /// Fail-soft: a store write failure logs a warning and
    /// continues — the runtime path is the business
    /// projection; missing evidence surfaces as a
    /// `dag_inspect` gap but does not block the next stage.
    /// Production stores surface drift as
    /// `StageEvidenceDrift`; we log and skip rather than
    /// crash so a replay with a stale digest (e.g. recovered
    /// from disk) does not tear down the loop.
    fn record_stage_evidence_if_accepted(
        &mut self,
        plan_key: &str,
        unit_key: &str,
        stage: &str,
        attempt: u32,
        base_commit: &str,
        accepted_commit: &str,
        evidence_token: &str,
        evidence_fingerprint: &str,
    ) {
        let Ok(stores) = self.ensure_stores() else {
            return;
        };
        let store = stores.plans.clone();
        let ev = StageEvidenceRecord {
            plan_key: plan_key.to_string(),
            unit_key: unit_key.to_string(),
            stage: stage.to_string(),
            attempt,
            accepted_commit: accepted_commit.to_string(),
            base_commit: base_commit.to_string(),
            evidence_token: evidence_token.to_string(),
            evidence_fingerprint: evidence_fingerprint.to_string(),
            accepted_at_ms: now_ms(),
        };
        if let Err(err) = store.record_stage_evidence(plan_key, unit_key, stage, attempt, &ev) {
            warn!(
                plan_key,
                unit_key,
                stage,
                attempt,
                error = %err,
                "DAG evidence: stage evidence write rejected (fail-soft)"
            );
        }
    }

    /// U3 (2026-09-09-0917): wire `handle_completion`'s accepted
    /// terminal into the per-stage evidence ledger. Resolves
    /// the base the stage descended from via the same
    /// `resolve_stage_base` lookup the spawn used (deterministic
    /// given the stored evidence / pin / plan base), reads the
    /// unit branch tip as the accepted commit, then forwards
    /// the bundle to `record_stage_evidence_if_accepted`.
    ///
    /// Best-effort: a failed branch-tip read means the worker's
    /// commit never landed — fall back to the resolved base so
    /// the evidence row is still written (the next stage then
    /// resumes from the same commit the spawn chose, NOT from
    /// a non-existent tip). A failed evidence write logs a
    /// warning and continues — the next stage's spawn re-tries
    /// the lookup against plan HEAD as the last-resort fallback.
    fn record_accepted_evidence(&mut self, kind: SpawnKind, identity: &JobIdentity, digest: &str) {
        let plan_key = identity.plan_key.clone();
        let unit_key = identity.unit_id.clone();
        let stage = identity.stage.clone();
        let attempt = identity.attempt;

        let base = match self.resolve_stage_base(&plan_key, &unit_key, kind) {
            Ok(base) => base,
            Err(err) => {
                warn!(
                    plan_key,
                    unit_key,
                    stage,
                    attempt,
                    error = %err,
                    "DAG evidence: base resolution failed; skipping evidence write"
                );
                return;
            }
        };
        let accepted = match self.read_unit_branch_tip(&unit_key) {
            Ok(tip) => tip,
            Err(err) => {
                warn!(
                    plan_key,
                    unit_key,
                    stage,
                    attempt,
                    error = %err,
                    "DAG evidence: branch tip unreadable; falling back to base as accepted_commit"
                );
                base.clone()
            }
        };
        self.record_stage_evidence_if_accepted(
            &plan_key,
            &unit_key,
            &stage,
            attempt,
            &base,
            &accepted,
            &identity.token,
            digest,
        );
    }

    /// Read the unit's branch tip via `git rev-parse`. The
    /// executor branch is the only trusted source of the
    /// accepted commit; the agent payload's content_hash is
    /// advisory. On failure (missing branch, non-git path, git
    /// error) the helper returns an error string for the
    /// caller to surface — U3 evidence recording falls back to
    /// the resolved base in that case so the row still lands.
    fn read_unit_branch_tip(&self, unit_key: &str) -> Result<String, String> {
        let exec = self
            .exec
            .as_ref()
            .ok_or_else(|| "DAG evidence: no execution context".to_string())?;
        let reference = format!(
            "refs/heads/ralph/{}/{}{}",
            exec.loop_id, unit_key, "{commit}"
        );
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.workspace)
            .arg("rev-parse")
            .arg("--verify")
            .arg(&reference)
            .output()
            .map_err(|err| format!("git rev-parse spawn: {err}"))?;
        if !output.status.success() {
            return Err(format!(
                "unit branch ralph/{}/{} not resolvable: {}",
                exec.loop_id,
                unit_key,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Launch one fenced job. Every failure before `spawn_pty_job`
    /// (journal reserve conflict, missing hat template, unresolvable
    /// base commit, worktree rejection) strands the unit with a
    /// journaled `failed` terminal plus a synthesised failure event
    /// — never a panic, never a silent drop.
    fn spawn_job(&mut self, pending: PendingSpawn) {
        let PendingSpawn {
            unit_key,
            kind,
            attempt,
            plan_key,
            feedback,
        } = pending;
        if self.exec.is_none() {
            return;
        }
        // U1 (2026-09-09-0917): pipeline callers pass plan-namespaced
        // unit_keys (`forge:{plan_key}:{unit_id}`); `JobIdentity` is
        // keyed by the SSOT `unit_key()` so the journal reservation
        // stays consistent. Strip the `forge:{plan_key}:` prefix back
        // to the bare `unit_id` so `identity.unit_id` matches the
        // schema contract. `job_id` and `token` must also stay within
        // the bounded-identity character set (`[A-Za-z0-9_-]` only —
        // `:` is rejected), so they reuse the bare `unit_id` rather
        // than the namespaced `unit_key`.
        let bare_unit_id = strip_plan_namespace(&plan_key, &unit_key);
        let job_id = format!("dag-{bare_unit_id}-{}-a{attempt}", kind.stage_str());
        let identity = JobIdentity {
            plan_key: plan_key.clone(),
            unit_id: bare_unit_id.to_string(),
            job_id: job_id.clone(),
            hat: kind.hat().to_string(),
            stage: kind.stage_str().to_string(),
            attempt,
            token: format!("tok-{bare_unit_id}-{}-a{attempt}", kind.stage_str()),
        };

        // Journal reservation first (D20 fencing): replay → skip.
        if kind == SpawnKind::Execute {
            self.executors_launched.insert(unit_key.clone());
        }
        let Some(journal) = self.journal() else {
            warn!(job_id, "DAG spawn: no durable journal; refusing to launch");
            return;
        };
        let reserve_result = journal.reserve_job(&identity, now_ms() as i64, None);
        match reserve_result {
            Ok(false) => {
                debug!(
                    job_id,
                    "DAG spawn: reservation replay; not launching a second process"
                );
                return;
            }
            Err(err) => {
                warn!(job_id, error = %err, "DAG spawn: reservation rejected (fail-closed)");
                return;
            }
            Ok(true) => {}
        }
        if kind == SpawnKind::Fix {
            self.fixer_in_flight += 1;
        }

        // Snapshot the execution-context inputs so the worktree /
        // failure paths below can take `&mut self` freely.
        let snapshot = {
            let exec = self.exec.as_ref().expect("exec checked above");
            exec.hats
                .get(kind.hat())
                .map(|hat| {
                    (
                        exec.loop_id.clone(),
                        hat.clone(),
                        exec.global_backend.clone(),
                        exec.hats_source_label.clone(),
                        exec.config_path.clone(),
                        exec.schemas.get(kind.success_topic()).cloned(),
                    )
                })
                .ok_or_else(|| format!("no `{}` hat template in preset config", kind.hat()))
        };
        let (loop_id, hat_config, global_backend, hats_source_label, config_path, schema) =
            match snapshot {
                Ok(snapshot) => snapshot,
                Err(reason) => {
                    self.fail_job(&identity, kind, &reason, "unknown");
                    return;
                }
            };

        // Trusted worktree against the per-Unit base pin (U3,
        // 2026-09-09-0917). The base pin is read first so a
        // reviewer / verifier / fixer resumes from the prior
        // stage's accepted commit rather than plan HEAD. The
        // first-time admission (when no pin exists yet) falls
        // back to the plan-level verified base and pins it in
        // the same call so the next stage can find it. If
        // there's a prior accepted stage for this unit, the
        // pin is rewritten only when the prior stage is the
        // immediately-preceding one in the stage graph — this
        // keeps the first-admission pin authoritative while
        // letting the next stage slide onto the just-accepted
        // commit (NOT onto plan HEAD).
        let verified_base = match self.resolve_stage_base(&plan_key, bare_unit_id, kind) {
            Ok(base) => base,
            Err(reason) => {
                self.fail_job(&identity, kind, &reason, "unknown");
                return;
            }
        };
        let worktree =
            match UnitWorktree::acquire(&self.workspace, &loop_id, bare_unit_id, &verified_base) {
                Ok(wt) => wt,
                Err(err) => {
                    let reason = format!("unit worktree acquire failed: {err}");
                    self.fail_job(&identity, kind, &reason, "unknown");
                    return;
                }
            };

        let events_file = self
            .workspace
            .join(".ralph")
            .join("dag")
            .join(&plan_key)
            .join(bare_unit_id)
            .join(format!("{job_id}.events.jsonl"));
        if let Some(parent) = events_file.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            let reason = format!("create dag events dir: {err}");
            self.fail_job(&identity, kind, &reason, "unknown");
            return;
        }
        let _ = std::fs::remove_file(&events_file);

        let (tests, allowed_paths, forbidden_paths) = self
            .plans
            .get(&plan_key)
            .and_then(|p| p.units.iter().find(|u| u.unit_id == bare_unit_id))
            .map(|u| {
                (
                    u.tests.clone(),
                    u.allowed_paths.clone(),
                    u.forbidden_paths.clone(),
                )
            })
            .unwrap_or_default();
        let prompt = build_job_prompt(
            &identity,
            kind,
            &hat_config,
            &worktree.path,
            &events_file,
            schema.as_ref(),
            &tests,
            &allowed_paths,
            &forbidden_paths,
            feedback.as_deref(),
        );

        // Backend resolution mirrors the wave dispatcher: hat backend
        // wins, hat args extend, tool policy applies.
        let mut backend = hat_config
            .backend
            .as_ref()
            .and_then(|hb| CliBackend::from_hat_backend(hb).ok())
            .unwrap_or(global_backend);
        if let Some(ref args) = hat_config.backend_args {
            backend.args.extend(args.iter().cloned());
        }
        ralph_adapters::apply_hat_tool_policy(&mut backend, &hat_config.disallowed_tools);
        crate::loop_runner::execution::inject_hat_execution_env(
            &mut backend,
            kind.hat(),
            &loop_id,
            &worktree.path,
            &events_file,
            None,
            hats_source_label.as_deref(),
            config_path.as_deref(),
        );

        // DAG env contract (D23): clear_env + allowlisted host names
        // + operator-declared backend env + the runtime overlay.
        let host: HashMap<String, String> = std::env::vars().collect();
        let mut env: Vec<(String, String)> = DagEnvPolicy::from_declared(DAG_ENV_ALLOWLIST)
            .filter_child_env(&host)
            .into_iter()
            .collect();
        env.extend(backend.env_vars.iter().cloned());
        env.extend([
            ("RALPH_DAG_UNIT_KEY".to_string(), identity.unit_key()),
            ("RALPH_DAG_JOB_ID".to_string(), job_id.clone()),
            ("RALPH_DAG_ATTEMPT".to_string(), attempt.to_string()),
            ("RALPH_DAG_JOB_TOKEN".to_string(), identity.token.clone()),
        ]);

        let (cmd, args, stdin_input, _temp_file_guard) = backend.build_command(&prompt, false);
        let spec = PtySpawnSpec {
            cmd,
            args,
            stdin_input,
            cwd: worktree.path.clone(),
            clear_env: true,
            env,
        };
        let mut handle = match spawn_pty_job(spec) {
            Ok(handle) => handle,
            Err(err) => {
                let reason = format!("pty spawn failed: {err}");
                self.fail_job(&identity, kind, &reason, "unknown");
                return;
            }
        };
        if let Some(pid) = handle.pid() {
            if let Err(err) = journal.record_job_pid(&identity, pid, now_ms() as i64) {
                warn!(job_id, error = %err, "DAG journal: pid write rejected");
            }
        } else {
            warn!(
                job_id,
                "DAG spawn: kernel exposed no pid; journal pid stays NULL"
            );
        }

        let timeout = Duration::from_secs(u64::from(hat_config.timeout.unwrap_or(3600)));
        let lease_mode = match hat_config.idle_heartbeat_secs.filter(|s| *s > 0) {
            Some(secs) => {
                let idle_window = Duration::from_secs(u64::from(secs));
                let startup_grace = hat_config
                    .startup_grace_secs
                    .filter(|s| *s > 0)
                    .map(|s| Duration::from_secs(u64::from(s)));
                PtyLeaseMode::DualClock {
                    cfg: LeaseConfig {
                        hard_cap_ms: timeout.as_millis() as u64,
                        idle_window_ms: Some(idle_window.as_millis() as u64),
                        weak_cap: hat_config.idle_weak_signal_cap.unwrap_or(8),
                        startup_grace_ms: startup_grace.map(|d| d.as_millis() as u64),
                    },
                    idle_window,
                    startup_grace,
                    events_file: Some(events_file.clone()),
                }
            }
            None => PtyLeaseMode::Legacy { hard_cap: timeout },
        };

        self.job_seq += 1;
        let worker_index = self.job_seq;
        let output_format = backend.output_format;
        let tx = self.completion_tx.clone();
        let completion_identity = identity.clone();
        let completion_events_file = events_file.clone();
        tokio::spawn(async move {
            let start = Instant::now();
            let mut on_line = |_line: &str| {};
            let outcome = drive_pty_lease_loop(
                &mut handle,
                &lease_mode,
                output_format,
                worker_index,
                &mut on_line,
                start,
            )
            .await;
            let status = finish_pty_job(handle).await.ok();
            let _ = tx.send(JobCompletion {
                identity: completion_identity,
                events_file: completion_events_file,
                exit_code: status.map(|s| s.exit_code() as i32),
                timed_out: outcome.timed_out,
            });
        });
        self.active_jobs.insert(job_id.clone());
        debug!(job_id, stage = kind.stage_str(), "DAG spawn: job launched");
    }
}

/// Job prompt: the runtime-owned context block precedes the hat
/// template so identity, channel and emit contract are unambiguous.
#[allow(clippy::too_many_arguments)]
fn build_job_prompt(
    identity: &JobIdentity,
    kind: SpawnKind,
    hat_config: &HatConfig,
    worktree_path: &Path,
    events_file: &Path,
    schema: Option<&EventSchema>,
    tests: &[String],
    allowed_paths: &[PathBuf],
    forbidden_paths: &[PathBuf],
    feedback: Option<&str>,
) -> String {
    let required: Vec<String> = schema
        .map(|s| s.required_fields.clone())
        .unwrap_or_default();
    let mut prompt = format!(
        "## DAG JOB CONTEXT (runtime-injected)\n\
         - You are the `{hat}` hat, running as DAG job `{job}` (stage `{stage}`, attempt {attempt}) \
         for unit `{unit}` of plan `{plan}`.\n\
         - Work exclusively inside the current working directory (`{cwd}`). It is a trusted unit \
         worktree pinned to the approved base commit. Do NOT create, switch, or reuse any other \
         worktree or branch.\n\
         - task_key: `{task_key}` (task_id is looked up by the runtime at merge time).\n\
         - Allowed paths for this Unit: {allowed_paths}\n\
         - Forbidden paths for this Unit: {forbidden_paths}\n\
         - The runtime points RALPH_EVENTS_FILE at `{events}`; `ralph emit` writes there. \
         Emit EXACTLY ONE business event.\n\
         - On success emit `{success}` with a JSON payload containing: {required}. The runtime \
         overwrites unit_id/plan_key/task_id/task_key; you MUST supply the remaining fields.\n\
         - On failure emit `{failure}` instead (same identity fields; plus its own required fields).\n",
        hat = kind.hat(),
        job = identity.job_id,
        stage = kind.stage_str(),
        attempt = identity.attempt,
        unit = identity.unit_id,
        plan = identity.plan_key,
        cwd = worktree_path.display(),
        task_key = identity.unit_key(),
        events = events_file.display(),
        allowed_paths = format_path_policy(allowed_paths),
        forbidden_paths = format_path_policy(forbidden_paths),
        success = kind.success_topic(),
        failure = kind.failure_topic(),
        required = required.join(", "),
    );
    if kind == SpawnKind::Verify {
        if tests.is_empty() {
            prompt.push_str(
                "- The plan declares no targeted gate commands for this unit; run the preset's \
                 standard verification for this worktree and record every command in the log.\n",
            );
        } else {
            prompt.push_str("- Targeted gate commands declared by the plan for this unit:\n");
            for test in tests {
                prompt.push_str(&format!("  - `{test}`\n"));
            }
            prompt.push_str("  Run each one and record the outcome in the verification log.\n");
        }
    }
    if let Some(report) = feedback {
        prompt.push_str(&format!(
            "- The previous review REJECTED this unit. Read the review report at `{report}`, \
             fix every finding in this worktree, then emit `{success}` again.\n",
            success = kind.success_topic(),
        ));
    }
    prompt.push('\n');
    prompt.push_str(&hat_config.instructions);
    prompt
}

fn format_path_policy(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        "`<none declared>`".to_string()
    } else {
        paths
            .iter()
            .map(|path| format!("`{}`", path.display()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_runner::runtime_job::RuntimeJobError;

    /// Routing table: executed → reviewer, reviewed ACCEPTED →
    /// verifier, reviewed REJECTED → fixer; verify topics and
    /// malformed verdicts produce no follow-up.
    #[test]
    fn follow_up_kind_routing_table() {
        assert_eq!(
            follow_up_kind(topics::UNIT_EXECUTED, &serde_json::json!({})),
            Some(SpawnKind::Review)
        );
        assert_eq!(
            follow_up_kind(
                topics::UNIT_REVIEWED,
                &serde_json::json!({"verdict": "ACCEPTED"})
            ),
            Some(SpawnKind::Verify)
        );
        assert_eq!(
            follow_up_kind(
                topics::UNIT_REVIEWED,
                &serde_json::json!({"verdict": "REJECTED"})
            ),
            Some(SpawnKind::Fix)
        );
        assert_eq!(
            follow_up_kind(topics::UNIT_REVIEWED, &serde_json::json!({})),
            None,
            "malformed verdict never spawns"
        );
        assert_eq!(
            follow_up_kind(topics_ext::UNIT_VERIFIED, &serde_json::json!({})),
            None
        );
    }

    /// Stage/hat/topic vocabulary stays aligned with the journal's
    /// accepted stage strings and the preset's hat keys.
    #[test]
    fn spawn_kind_vocabulary_matches_journal_and_preset() {
        assert_eq!(SpawnKind::Fix.stage_str(), "fix");
        assert_eq!(SpawnKind::Fix.hat(), "executor");
        assert_eq!(SpawnKind::Fix.success_topic(), "forge.unit.executed");
        assert_eq!(
            SpawnKind::Verify.failure_topic(),
            "forge.unit.verification_failed"
        );
        for kind in [
            SpawnKind::Execute,
            SpawnKind::Review,
            SpawnKind::Verify,
            SpawnKind::Fix,
        ] {
            assert!(
                ["execute", "review", "verify", "fix"].contains(&kind.stage_str()),
                "journal only accepts execute/review/verify/fix"
            );
        }
    }

    /// sha256 helper produces the 64-hex digest the journal demands.
    #[test]
    fn digest_is_64_lowercase_hex() {
        let digest = sha256_hex("reason");
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    use ralph_core::config::{CliConfig, ResolvedDagPools};
    use tempfile::TempDir;

    const ARTIFACT_REL: &str = ".ralph/forge/pf-test/execution-plan.yml";
    const PLAN_ARTIFACT: &str = r#"version: 1
plan_key: pf-test
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1-foundation
    allowed_paths: [u1.txt]
    tests:
      - cargo nextest run -p ralph-core -- u1
  - id: U2
    title: Feature
    depends_on: []
    execution_wave: 1
    integration_order: 2
    target_branch: feat/u2-feature
    allowed_paths: [u2.txt]
"#;

    fn exec_context(workspace: &Path) -> DagExecutionContext {
        let mut config = RalphConfig::default();
        for hat in [HAT_EXECUTOR, HAT_REVIEWER, HAT_VERIFIER] {
            let hat_config = HatConfig {
                name: hat.to_string(),
                instructions: format!("{hat} instructions"),
                ..HatConfig::default()
            };
            config.hats.insert(hat.to_string(), hat_config);
        }
        let backend =
            CliBackend::from_config(&CliConfig::default()).expect("default backend builds");
        DagExecutionContext::new(
            &config,
            &backend,
            "loop-test",
            workspace.join("events.jsonl"),
            None,
        )
    }

    fn real_job_exec_context(workspace: &Path) -> DagExecutionContext {
        let mut config = RalphConfig::default();
        config.hats.insert(
            HAT_EXECUTOR.to_string(),
            HatConfig {
                name: HAT_EXECUTOR.to_string(),
                instructions: "execute one fixture unit".to_string(),
                timeout: Some(10),
                ..HatConfig::default()
            },
        );
        let mut cli = CliConfig::default();
        cli.backend = "custom".to_string();
        cli.command = Some("sh".to_string());
        cli.prompt_mode = "stdin".to_string();
        cli.args = vec![
            "-c".to_string(),
            "grep -F 'Allowed paths for this Unit: `u1.txt`' >/dev/null || exit 41; printf '%s\\n' '{\"topic\":\"forge.unit.executed\",\"payload\":\"{}\"}' >> \"$RALPH_EVENTS_FILE\"".to_string(),
        ];
        let backend = CliBackend::from_config(&cli).expect("custom fixture backend builds");
        DagExecutionContext::new(
            &config,
            &backend,
            "loop-test",
            workspace.join("events.jsonl"),
            None,
        )
    }

    #[cfg(feature = "supervisor-db")]
    fn counting_job_exec_context(workspace: &Path, counter: &Path) -> DagExecutionContext {
        let mut config = RalphConfig::default();
        config.hats.insert(
            HAT_EXECUTOR.to_string(),
            HatConfig {
                name: HAT_EXECUTOR.to_string(),
                instructions: "execute one recovery fixture unit".to_string(),
                timeout: Some(10),
                ..HatConfig::default()
            },
        );
        let mut cli = CliConfig::default();
        cli.backend = "custom".to_string();
        cli.command = Some("sh".to_string());
        cli.prompt_mode = "stdin".to_string();
        let counter = counter.to_string_lossy().replace('\'', "'\\''");
        cli.args = vec![
            "-c".to_string(),
            format!(
                "printf '%s\\n' spawn >> '{counter}'; sleep 2; printf '%s\\n' '{{\"topic\":\"forge.unit.executed\",\"payload\":\"{{}}\"}}' >> \"$RALPH_EVENTS_FILE\""
            ),
        ];
        let backend = CliBackend::from_config(&cli).expect("counting backend builds");
        DagExecutionContext::new(
            &config,
            &backend,
            "loop-test",
            workspace.join("events.jsonl"),
            None,
        )
    }

    fn init_git_fixture(workspace: &Path) -> String {
        for args in [
            &["init", "-q"][..],
            &["config", "user.email", "dag-test@example.invalid"][..],
            &["config", "user.name", "DAG Test"][..],
            &["add", "."][..],
            &["commit", "-qm", "fixture"][..],
        ] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(workspace)
                .status()
                .expect("run git fixture command");
            assert!(status.success(), "git command failed: git {args:?}");
        }
        ralph_core::get_head_sha(workspace).expect("fixture has a git head")
    }

    /// Dag-mode runtime with the plan activated through the real
    /// accepted-event path (receipt → store registration → pipeline
    /// seed). The tempdir is NOT a git repository, so
    /// `verified_base_commit` stays `None` and the admission snapshot
    /// reports no target head: tick never auto-admits, which keeps
    /// these tests deterministic.
    fn dag_fixture() -> (TempDir, DagSchedulerRuntime) {
        let tmp = TempDir::new().expect("temp workspace");
        let artifact = tmp.path().join(ARTIFACT_REL);
        std::fs::create_dir_all(artifact.parent().expect("parent")).expect("mkdir");
        std::fs::write(&artifact, PLAN_ARTIFACT).expect("write artifact");
        let pools = ResolvedDagPools {
            global: 4,
            executor: 2,
            reviewer: 2,
            verifier: 2,
            fixer: 2,
        };
        let mut runtime =
            DagSchedulerRuntime::new(SchedulerMode::Dag, pools, tmp.path().to_path_buf());
        runtime.attach_execution_context(exec_context(tmp.path()));

        let bytes = std::fs::read(&artifact).expect("read artifact");
        let digest = ralph_core::artifact_canonicalizer::canonicalize(&bytes)
            .expect("canonicalize")
            .digest;
        let plan_ready = ralph_proto::Event::new(
            super::super::seam_topics::PLAN_READY,
            serde_json::json!({
                "plan_key": "pf-test",
                "execution_plan_path": ARTIFACT_REL,
                "plan_digest": digest,
            })
            .to_string(),
        );
        let approved = ralph_proto::Event::new(
            super::super::seam_topics::CONCURRENCY_APPROVED,
            serde_json::json!({
                "execution_plan_path": ARTIFACT_REL,
                "approval_report_path": ".ralph/forge/pf-test/concurrency-approval.md",
                "approved": true,
            })
            .to_string(),
        );
        runtime.observe_accepted_events(&[plan_ready, approved]);
        (tmp, runtime)
    }

    /// The admission snapshot re-reports a Ready unit every tick; the
    /// `executors_launched` dedup plus the journal reservation must
    /// guarantee a single launch. Here the first attempt fails closed
    /// (no verified base commit in a non-git workspace), and the
    /// replayed admission must not reserve or fail the unit again.
    #[test]
    fn executor_admission_replay_never_spawns_twice() {
        let (_tmp, mut runtime) = dag_fixture();

        runtime.maybe_spawn_executor("pf-test", "U1");
        assert!(
            runtime.active_jobs.is_empty(),
            "no verified base commit: the job never reaches spawn_pty_job"
        );
        assert_eq!(
            runtime.merge_queue.len(),
            1,
            "the stranded unit gets exactly one synthesised failure event"
        );
        assert_eq!(
            runtime
                .merge_queue
                .front()
                .expect("queued")
                .event
                .topic
                .as_str(),
            "forge.unit.execution_failed"
        );

        runtime.maybe_spawn_executor("pf-test", "U1");
        assert_eq!(
            runtime.merge_queue.len(),
            1,
            "replayed admission is deduped before the journal reserve"
        );
        assert!(runtime.active_jobs.is_empty());
    }

    /// S4 facade evidence: a verify terminal releases the completing
    /// Unit's slot before the next admission pass, so a Ready sibling
    /// blocked by the global cap can be admitted immediately.
    #[test]
    fn verify_terminal_releases_slot_for_ready_sibling() {
        let tmp = TempDir::new().expect("temp workspace");
        let mut runtime = DagSchedulerRuntime::new(
            SchedulerMode::Dag,
            ResolvedDagPools {
                global: 1,
                executor: 1,
                reviewer: 1,
                verifier: 1,
                fixer: 1,
            },
            tmp.path().to_path_buf(),
        );
        runtime
            .pipeline
            .ensure_unit("U-done", "job-done", HAT_EXECUTOR, Stage::Execute);
        runtime
            .pipeline
            .ensure_unit("U-ready", "job-ready", HAT_EXECUTOR, Stage::Execute);

        assert!(matches!(
            runtime.pipeline.advance("U-done", Stage::Execute),
            AdvanceOutcome::Admitted { .. }
        ));
        assert!(matches!(
            runtime.pipeline.advance("U-ready", Stage::Execute),
            AdvanceOutcome::Blocked(RuntimeJobError::GlobalCapExceeded { .. })
        ));

        runtime.observe_unit_event_dag(
            topics_ext::UNIT_VERIFIED,
            "U-done",
            &serde_json::json!({"plan_key": "pf-test", "unit_id": "U-done"}),
            None,
        );

        assert!(matches!(
            runtime.pipeline.advance("U-ready", Stage::Execute),
            AdvanceOutcome::Admitted { .. }
        ));
    }

    /// Authoritative canary: a real DAG executor job uses the verified Git
    /// base, creates a real isolated worktree, runs a real PTY child, and
    /// returns its success event to the runtime merge queue.
    #[tokio::test]
    async fn real_dag_executor_canary_runs_in_git_worktree() {
        let (tmp, mut runtime) = dag_fixture();
        let base = init_git_fixture(tmp.path());
        runtime
            .plans
            .get_mut("pf-test")
            .expect("fixture plan registered")
            .verified_base_commit = Some(base);
        runtime.attach_execution_context(real_job_exec_context(tmp.path()));

        runtime.maybe_spawn_executor("pf-test", "U1");
        for _ in 0..100 {
            runtime.drain_completions();
            if !runtime.merge_queue.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert!(
            runtime.active_jobs.is_empty(),
            "canary child must be reaped"
        );
        let merge = runtime.merge_queue.front().expect("success event queued");
        assert_eq!(merge.event.topic.as_str(), "forge.unit.executed");
        let payload: Value = serde_json::from_str(&merge.event.payload).expect("JSON result");
        assert_eq!(
            merge
                .event
                .source
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
            Some(HAT_EXECUTOR)
        );
        assert_eq!(payload["job_id"], "dag-U1-execute-a0");
        assert_eq!(payload["job_token"], "tok-U1-execute-a0");
        assert_eq!(payload["stage"], "execute");
        assert_eq!(payload["attempt"], 0);
        assert!(
            tmp.path().join(".ralph/worktrees/loop-test-U1").is_dir(),
            "executor must run in its isolated unit worktree"
        );
    }

    /// A real worker already reserved before a runtime restart must be
    /// adopted from the durable journal, never launched a second time.
    #[cfg(feature = "supervisor-db")]
    #[tokio::test]
    async fn recovery_adopts_real_worker_without_duplicate_spawn() {
        let (tmp, mut first) = dag_fixture();
        let base = init_git_fixture(tmp.path());
        first
            .plans
            .get_mut("pf-test")
            .expect("fixture plan registered")
            .verified_base_commit = Some(base.clone());
        first
            .journal()
            .expect("durable journal")
            .record_verified_base("pf-test", &base, 1)
            .expect("pin verified base");
        let counter = tmp.path().join("spawn-count.txt");
        first.attach_execution_context(counting_job_exec_context(tmp.path(), &counter));
        first.maybe_spawn_executor("pf-test", "U1");

        for _ in 0..100 {
            let pid_recorded = first
                .journal()
                .expect("durable journal")
                .list_jobs("pf-test")
                .expect("list jobs")
                .iter()
                .any(|job| job.identity.unit_id == "U1" && job.pid.is_some());
            if pid_recorded && counter.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            std::fs::read_to_string(&counter)
                .expect("worker startup counter")
                .lines()
                .count(),
            1,
            "the first runtime launches exactly one child"
        );
        drop(first);

        let mut recovered = DagSchedulerRuntime::new(
            SchedulerMode::Dag,
            ResolvedDagPools {
                global: 4,
                executor: 2,
                reviewer: 2,
                verifier: 2,
                fixer: 2,
            },
            tmp.path().to_path_buf(),
        );
        recovered.attach_execution_context(counting_job_exec_context(tmp.path(), &counter));
        recovered.recover_after_restart();
        for _ in 0..200 {
            recovered.drain_completions();
            if !recovered.merge_queue.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert!(
            !recovered.merge_queue.is_empty(),
            "recovery adopts the worker result"
        );
        assert_eq!(
            std::fs::read_to_string(&counter)
                .expect("worker startup counter")
                .lines()
                .count(),
            1,
            "restart recovery must not spawn a duplicate child"
        );
    }

    /// A unit id outside the journal charset (`[A-Za-z0-9-_]`) is
    /// rejected by `reserve_job` — fail-closed, no launch, no event.
    #[test]
    fn illegal_unit_id_fails_closed_before_launch() {
        let (_tmp, mut runtime) = dag_fixture();

        runtime.spawn_job(PendingSpawn {
            unit_key: "U.bad".to_string(),
            kind: SpawnKind::Execute,
            attempt: 0,
            plan_key: "pf-test".to_string(),
            feedback: None,
        });

        assert!(runtime.active_jobs.is_empty());
        assert!(
            runtime.merge_queue.is_empty(),
            "a rejected reservation emits nothing and strands nothing"
        );
    }

    /// OPAC: two queued results reach the main ledger one per merge
    /// pass, in FIFO order.
    #[test]
    fn merge_one_respects_opac_budget() {
        let (tmp, mut runtime) = dag_fixture();
        runtime.queue_result_event(
            topics::UNIT_EXECUTED,
            HAT_EXECUTOR.to_string(),
            serde_json::json!({"unit_id": "U1"}),
            None,
        );
        runtime.queue_result_event(
            topics::UNIT_REVIEWED,
            HAT_REVIEWER.to_string(),
            serde_json::json!({"unit_id": "U1", "verdict": "ACCEPTED"}),
            None,
        );

        let ledger = tmp.path().join("events.jsonl");
        runtime.merge_one();
        let content = std::fs::read_to_string(&ledger).expect("ledger after first merge");
        assert_eq!(content.lines().count(), 1, "OPAC: one event per pass");
        assert!(content.contains(topics::UNIT_EXECUTED));

        runtime.merge_one();
        let content = std::fs::read_to_string(&ledger).expect("ledger after second merge");
        assert_eq!(content.lines().count(), 2);
        assert!(content.contains(topics::UNIT_REVIEWED));
    }

    /// A fourth reviewer REJECTED exceeds the three-fix-attempt
    /// budget: the pipeline blocks the advance and the seam
    /// synthesises `forge.unit.execution_failed` instead of launching
    /// another fixer.
    #[test]
    fn review_rejection_beyond_budget_synthesizes_failure() {
        let (_tmp, mut runtime) = dag_fixture();

        // U1 (2026-09-09-0917): pipeline callers must use the
        // plan-namespaced `unit_key` registered in `on_concurrency_approved`.
        assert!(matches!(
            runtime.pipeline.advance("forge:pf-test:U1", Stage::Execute),
            AdvanceOutcome::Admitted { .. }
        ));
        runtime.pipeline.release("forge:pf-test:U1");
        assert!(matches!(
            runtime.pipeline.advance("forge:pf-test:U1", Stage::Review),
            AdvanceOutcome::Admitted { .. }
        ));
        // Budget arithmetic: the initial Review admission runs at
        // attempt 0; each rejection bumps first, and `advance`
        // refuses once attempt == MAX_FIX_ATTEMPTS (3). So exactly
        // two re-admissions (attempts 1 and 2) succeed; the third
        // rejection is the budget-exhaustion path under test.
        for attempt in 1..=2 {
            runtime.pipeline.release("forge:pf-test:U1");
            let outcome = runtime
                .pipeline
                .bump_attempt_and_advance("forge:pf-test:U1", Stage::Review);
            assert!(
                matches!(outcome, AdvanceOutcome::Admitted { .. }),
                "fix attempt {attempt} stays inside the budget"
            );
        }
        runtime.pipeline.release("forge:pf-test:U1");

        let payload = serde_json::json!({
            "unit_id": "U1",
            "plan_key": "pf-test",
            "verdict": "REJECTED",
        });
        // U1 (2026-09-09-0917): pipeline callers must use the
        // plan-namespaced `unit_key` registered in `on_concurrency_approved`.
        runtime.observe_unit_event_dag(topics::UNIT_REVIEWED, "forge:pf-test:U1", &payload, None);

        assert!(
            runtime.pending_spawns.is_empty(),
            "no fixer is launched past the budget"
        );
        assert_eq!(
            runtime.merge_queue.len(),
            1,
            "budget exhaustion synthesises exactly one failure event"
        );
        let event = &runtime.merge_queue.front().expect("queued").event;
        assert_eq!(event.topic.as_str(), "forge.unit.execution_failed");
        assert!(
            event.payload.contains("exceeded"),
            "the reason names the budget: {}",
            event.payload
        );
    }

    /// The accepted-event read-back must find the post-acceptance record by
    /// the same plan-qualified key used when the result was merged.
    #[test]
    fn post_acceptance_cleanup_uses_plan_qualified_unit_key() {
        let (_tmp, mut runtime) = dag_fixture();
        let identity = JobIdentity {
            plan_key: "pf-test".to_string(),
            unit_id: "U1".to_string(),
            job_id: "job-post-acceptance".to_string(),
            hat: HAT_EXECUTOR.to_string(),
            stage: "execute".to_string(),
            attempt: 0,
            token: "token-post-acceptance".to_string(),
        };
        runtime.awaiting_acceptance.insert(
            identity.unit_key(),
            PostAcceptance {
                identity,
                terminal: "accepted",
                digest: "a".repeat(64),
            },
        );

        runtime.observe_unit_event_dag(
            "forge.unknown.accepted",
            "U1",
            &serde_json::json!({
                "plan_key": "pf-test",
                "unit_id": "U1",
                "job_id": "job-post-acceptance",
                "job_token": "token-post-acceptance",
                "stage": "execute",
                "attempt": 0,
            }),
            Some(HAT_EXECUTOR),
        );

        assert!(runtime.awaiting_acceptance.is_empty());
    }

    /// The post-acceptance seam must persist the terminal against the
    /// reserved job after the accepted event is read back with its plan key.
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn accepted_result_persists_terminal_after_plan_qualified_lookup() {
        let (_tmp, mut runtime) = dag_fixture();
        let identity = JobIdentity {
            plan_key: "pf-test".to_string(),
            unit_id: "U1".to_string(),
            job_id: "job-terminal-projection".to_string(),
            hat: HAT_EXECUTOR.to_string(),
            stage: "execute".to_string(),
            attempt: 0,
            token: "token-terminal-projection".to_string(),
        };
        runtime
            .journal()
            .expect("durable journal")
            .reserve_job(&identity, 1, None)
            .expect("reserve job");
        runtime.awaiting_acceptance.insert(
            identity.unit_key(),
            PostAcceptance {
                identity,
                terminal: "accepted",
                digest: "b".repeat(64),
            },
        );

        runtime.observe_unit_event_dag(
            "forge.unknown.accepted",
            "U1",
            &serde_json::json!({
                "plan_key": "pf-test",
                "unit_id": "U1",
                "job_id": "job-terminal-projection",
                "job_token": "token-terminal-projection",
                "stage": "execute",
                "attempt": 0,
            }),
            Some(HAT_EXECUTOR),
        );

        let jobs = runtime
            .journal()
            .expect("durable journal")
            .list_jobs("pf-test")
            .expect("list jobs");
        assert!(runtime.awaiting_acceptance.is_empty());
        assert!(jobs.iter().any(|job| {
            job.identity.job_id == "job-terminal-projection"
                && job.terminal.as_deref() == Some("accepted")
        }));
    }

    /// A stale or forged accepted event must not consume the current
    /// post-acceptance record when its token or source hat differs.
    #[test]
    fn forged_accepted_result_cannot_advance_current_job() {
        let (_tmp, mut runtime) = dag_fixture();
        let identity = JobIdentity {
            plan_key: "pf-test".to_string(),
            unit_id: "U1".to_string(),
            job_id: "job-forgery-fence".to_string(),
            hat: HAT_EXECUTOR.to_string(),
            stage: "execute".to_string(),
            attempt: 0,
            token: "token-forgery-fence".to_string(),
        };
        let key = identity.unit_key();
        runtime.awaiting_acceptance.insert(
            key.clone(),
            PostAcceptance {
                identity,
                terminal: "accepted",
                digest: "c".repeat(64),
            },
        );

        runtime.observe_unit_event_dag(
            "forge.unit.executed",
            "U1",
            &serde_json::json!({
                "plan_key": "pf-test",
                "unit_id": "U1",
                "job_id": "job-forgery-fence",
                "job_token": "wrong-token",
                "stage": "execute",
                "attempt": 0,
            }),
            Some(HAT_EXECUTOR),
        );

        assert!(runtime.awaiting_acceptance.contains_key(&key));
        assert!(runtime.merge_queue.is_empty());
    }

    /// The same identity fence must hold after restart, when the
    /// post-acceptance map is empty and the durable journal is the only
    /// source of the current job identity.
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn forged_result_cannot_advance_durable_current_job() {
        let (_tmp, mut runtime) = dag_fixture();
        let identity = JobIdentity {
            plan_key: "pf-test".to_string(),
            unit_id: "U1".to_string(),
            job_id: "job-durable-fence".to_string(),
            hat: HAT_EXECUTOR.to_string(),
            stage: "execute".to_string(),
            attempt: 0,
            token: "token-durable-fence".to_string(),
        };
        runtime
            .journal()
            .expect("durable journal")
            .reserve_job(&identity, 1, None)
            .expect("reserve job");

        runtime.observe_unit_event_dag(
            "forge.unit.executed",
            "U1",
            &serde_json::json!({
                "plan_key": "pf-test",
                "unit_id": "U1",
                "job_id": "job-durable-fence",
                "job_token": "wrong-token",
                "stage": "execute",
                "attempt": 0,
            }),
            Some(HAT_EXECUTOR),
        );

        let jobs = runtime
            .journal()
            .expect("durable journal")
            .list_jobs("pf-test")
            .expect("list jobs");
        assert!(runtime.merge_queue.is_empty());
        assert!(
            jobs.iter().any(|job| {
                job.identity.job_id == "job-durable-fence" && job.terminal.is_none()
            })
        );
    }

    /// The inner keepalive gate: a fresh dag runtime owns no work; a
    /// queued failure event flips it so the loop cannot fall into
    /// fallback termination while the result awaits merge.
    #[test]
    fn has_pending_work_gates_the_inner_keepalive() {
        let (_tmp, mut runtime) = dag_fixture();
        assert!(!runtime.has_pending_work());

        let identity = JobIdentity {
            plan_key: "pf-test".to_string(),
            unit_id: "U1".to_string(),
            job_id: "dag-U1-execute-a0".to_string(),
            hat: HAT_EXECUTOR.to_string(),
            stage: "execute".to_string(),
            attempt: 0,
            token: "tok-U1-execute-a0".to_string(),
        };
        runtime.fail_job(&identity, SpawnKind::Execute, "boom", "unknown");

        assert!(
            runtime.has_pending_work(),
            "a queued merge keeps the loop alive"
        );
    }

    #[test]
    fn job_prompt_surfaces_unit_path_policy() {
        let identity = JobIdentity {
            plan_key: "pf-test".to_string(),
            unit_id: "U1".to_string(),
            job_id: "job-prompt-policy".to_string(),
            hat: HAT_EXECUTOR.to_string(),
            stage: "execute".to_string(),
            attempt: 0,
            token: "token-prompt-policy".to_string(),
        };
        let hat = HatConfig {
            name: HAT_EXECUTOR.to_string(),
            instructions: "execute the unit".to_string(),
            ..HatConfig::default()
        };
        let prompt = build_job_prompt(
            &identity,
            SpawnKind::Execute,
            &hat,
            Path::new("/worktree/U1"),
            Path::new("/worktree/U1/events.jsonl"),
            None,
            &[],
            &[PathBuf::from("src"), PathBuf::from("tests")],
            &[PathBuf::from("secrets")],
            None,
        );

        assert!(prompt.contains("Allowed paths for this Unit: `src`, `tests`"));
        assert!(prompt.contains("Forbidden paths for this Unit: `secrets`"));
    }

    // ─────────────────────────────────────────────────────────────────
    // 2026-09-09-0917 plan U3: per-Unit base pin +
    // per-stage accepted evidence ledger — runtime driver seam.
    //
    // The store-level contract suite (in `dag_store::contract_tests`)
    // exercises the durable read/write side directly. These tests
    // exercise the runtime driver's `resolve_stage_base` lookup
    // path against the same in-memory store so a regression that
    // silently returns plan HEAD instead of the prior stage's
    // accepted commit fails fast at the seam.
    // ─────────────────────────────────────────────────────────────────

    use ralph_core::supervisor::dag_store::StageEvidenceRecord;

    fn u3_runtime() -> DagSchedulerRuntime {
        let pools = ResolvedDagPools {
            global: 4,
            executor: 2,
            reviewer: 2,
            verifier: 2,
            fixer: 1,
        };
        let mut runtime = DagSchedulerRuntime::new(
            ralph_core::config::SchedulerMode::Dag,
            pools,
            PathBuf::from("/tmp/dag-u3-test"),
        );
        runtime.attach_in_memory_stores();
        runtime
    }

    /// U3 S3: the first admission's `resolve_stage_base` returns
    /// the plan-level verified base AND pins it on the store so
    /// the next-stage spawn finds a per-Unit pin (not a plan
    /// fallback). This is the foundation for cross-stage
    /// hand-off — without it, a re-admission of the same unit
    /// would always re-read plan HEAD.
    #[test]
    fn u3_first_execute_admission_resolves_plan_base_and_pins_it() {
        let mut runtime = u3_runtime();
        runtime.register_plan_for_test("pf-u3", Some("plan-base-abc".to_string()));

        // Execute at attempt 1: no prior stage evidence, no
        // per-Unit pin → falls back to plan-level base and pins.
        let resolved = runtime
            .resolve_stage_base("pf-u3", "U1", SpawnKind::Execute)
            .expect("first admission must resolve");
        assert_eq!(resolved, "plan-base-abc");

        // Pin must have been written — next Execute at attempt 2
        // finds the per-Unit pin, NOT plan HEAD again.
        let stores = runtime.ensure_stores().expect("stores");
        let pin = stores
            .plans
            .get_unit_base("pf-u3", "U1")
            .expect("pin read")
            .expect("pin must exist after first admission");
        assert_eq!(pin.base_commit, "plan-base-abc");
        // First-pin `pinned_at_ms` wins on re-pin (the per-Unit
        // pin is authoritative across admissions of the same
        // stage — never silently rewrites with a different
        // base).
        assert_eq!(pin.pinned_at_ms, now_ms());
    }

    /// U3 S3: a Review admission reads the prior Execute's
    /// accepted evidence, NOT plan HEAD. This is the
    /// committed-output hand-off the plan calls out: the
    /// reviewer resumes from the just-accepted commit.
    #[test]
    fn u3_review_admission_reads_prior_execute_evidence() {
        let mut runtime = u3_runtime();
        runtime.register_plan_for_test("pf-u3", Some("plan-base-abc".to_string()));

        // Simulate the prior Execute having landed evidence at
        // attempt 1 with `accepted_commit = acc-exec`.
        let stores = runtime.ensure_stores().expect("stores");
        let ev = StageEvidenceRecord {
            plan_key: "pf-u3".into(),
            unit_key: "U1".into(),
            stage: "execute".into(),
            attempt: 1,
            accepted_commit: "acc-exec".into(),
            base_commit: "plan-base-abc".into(),
            evidence_token: "tok-1".into(),
            evidence_fingerprint: "fp-1".into(),
            accepted_at_ms: 1_700_000_000_000,
        };
        stores
            .plans
            .record_stage_evidence("pf-u3", "U1", "execute", 1, &ev)
            .expect("seed execute evidence");

        // Review admission: must use the prior accepted commit,
        // NOT plan HEAD or the per-Unit pin.
        let resolved = runtime
            .resolve_stage_base("pf-u3", "U1", SpawnKind::Review)
            .expect("review admission must resolve");
        assert_eq!(
            resolved, "acc-exec",
            "review must resume from the execute stage's accepted commit"
        );
    }

    /// U3: when prior stage evidence exists, it wins over the
    /// per-Unit pin (the per-Unit pin stays at the FIRST
    /// admission's base; subsequent stages advance on the
    /// prior accepted commit). Without this test the order of
    /// the resolution steps could regress and a stale plan
    /// HEAD could surface to the next stage.
    #[test]
    fn u3_prior_evidence_wins_over_per_unit_pin() {
        let mut runtime = u3_runtime();
        runtime.register_plan_for_test("pf-u3", Some("plan-base-abc".to_string()));

        // Pre-pin the per-Unit base to something DIFFERENT from
        // plan HEAD so the test fails fast on a wrong lookup
        // order.
        let stores = runtime.ensure_stores().expect("stores");
        stores
            .plans
            .pin_unit_base("pf-u3", "U1", "stale-pin", 1)
            .expect("pin");

        // Now seed PRIOR STAGE evidence (Verify looks at review's
        // evidence, not execute's) pointing at the just-accepted
        // commit (NOT the stale pin).
        let ev = StageEvidenceRecord {
            plan_key: "pf-u3".into(),
            unit_key: "U1".into(),
            stage: "review".into(),
            attempt: 1,
            accepted_commit: "acc-rev-fresh".into(),
            base_commit: "stale-pin".into(),
            evidence_token: "tok-1".into(),
            evidence_fingerprint: "fp-1".into(),
            accepted_at_ms: 1_700_000_000_000,
        };
        stores
            .plans
            .record_stage_evidence("pf-u3", "U1", "review", 1, &ev)
            .expect("seed evidence");

        // Verify admission: must pick the prior evidence,
        // ignoring the per-Unit pin.
        let resolved = runtime
            .resolve_stage_base("pf-u3", "U1", SpawnKind::Verify)
            .expect("verify admission must resolve");
        assert_eq!(
            resolved, "acc-rev-fresh",
            "verify must read review's accepted evidence, not the stale per-Unit pin"
        );
    }

    /// U3: a Fix admission reads the prior Execute's evidence
    /// (NOT review's) — the Fix stage re-runs the executor hat
    /// on the executor's prior accepted commit, picking up the
    /// reviewer's rejection feedback. This matches the
    /// `follow_up_kind` routing in the spawn pipeline.
    #[test]
    fn u3_fix_admission_reads_prior_execute_evidence() {
        let mut runtime = u3_runtime();
        runtime.register_plan_for_test("pf-u3", Some("plan-base-abc".to_string()));

        // Seed BOTH review-rejected evidence (for completeness)
        // and execute evidence (the one Fix must use).
        let stores = runtime.ensure_stores().expect("stores");
        let exec_ev = StageEvidenceRecord {
            plan_key: "pf-u3".into(),
            unit_key: "U1".into(),
            stage: "execute".into(),
            attempt: 1,
            accepted_commit: "acc-exec".into(),
            base_commit: "plan-base-abc".into(),
            evidence_token: "tok-1".into(),
            evidence_fingerprint: "fp-1".into(),
            accepted_at_ms: 1_700_000_000_000,
        };
        let rev_ev = StageEvidenceRecord {
            plan_key: "pf-u3".into(),
            unit_key: "U1".into(),
            stage: "review".into(),
            attempt: 1,
            accepted_commit: "acc-rev-rejected".into(),
            base_commit: "acc-exec".into(),
            evidence_token: "tok-2".into(),
            evidence_fingerprint: "fp-2".into(),
            accepted_at_ms: 1_700_000_000_001,
        };
        stores
            .plans
            .record_stage_evidence("pf-u3", "U1", "execute", 1, &exec_ev)
            .expect("seed execute");
        stores
            .plans
            .record_stage_evidence("pf-u3", "U1", "review", 1, &rev_ev)
            .expect("seed review");

        // Fix admission: must read execute's accepted commit,
        // not review's. Fix is the executor hat re-running on
        // the executor's prior commit + the rejection feedback.
        let resolved = runtime
            .resolve_stage_base("pf-u3", "U1", SpawnKind::Fix)
            .expect("fix admission must resolve");
        assert_eq!(
            resolved, "acc-exec",
            "fix must resume from execute's accepted commit, never review's"
        );
    }

    /// U3 fail-closed: a spawn with no plan-level verified
    /// base AND no per-Unit pin AND no prior stage evidence
    /// returns an error string (the caller surfaces it via
    /// `fail_job`). Without this the runtime would silently
    /// spawn a job whose worktree acquisition crashes on a
    /// missing `verified_base_commit`.
    #[test]
    fn u3_no_base_anywhere_returns_error() {
        let mut runtime = u3_runtime();
        // Plan has no verified base; no pin; no evidence.
        runtime.register_plan_for_test("pf-u3", None);
        let err = runtime
            .resolve_stage_base("pf-u3", "U1", SpawnKind::Execute)
            .expect_err("no base anywhere must fail closed");
        assert!(
            err.contains("no verified base commit"),
            "error must explain the missing base: {err}"
        );
    }
}
