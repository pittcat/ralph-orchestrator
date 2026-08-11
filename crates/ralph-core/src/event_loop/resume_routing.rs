//! Plan 2026-08-10-001 Unit 2: unified `task.resume` target resolver
//! and publisher boundary.
//!
//! All runtime-generated `task.resume` events must flow through
//! this module. The resolver takes the priority chain inputs
//! (explicit Event target, JSONL `triggered`, payload `target_hat`,
//! current-loop open task `owner_hat_id`) and produces a typed
//! [`ResumeDecision`] so callers can fail-close on conflict /
//! unknown target / closed task / cross-loop task.
//!
//! Per D4 the resolver priority chain is:
//! 1. Explicit `Event.target` (already attached by the publisher).
//! 2. JSONL `triggered` already converted to `Event.target`.
//! 3. Payload `target_hat` (must be registered AND consistent with
//!    the optional `task_id` / `task_key` identity in the payload).
//! 4. Current-loop open task `owner_hat_id` (must be registered).
//!
//! At any step a conflict or non-registered target produces
//! [`ResumeDecision::Block`] — never silently re-route to an
//! arbitrary hat. Equivalent pending resumes return
//! [`ResumeDecision::Duplicate`] without enlarging the queue.
//!
//! This module does NOT touch the EventBus directly; the caller
//! publishes via `EventBus::publish` after receiving an `Allow`
//! decision so the existing routing semantics remain authoritative.

use ralph_proto::HatId;

use crate::task_store::TaskStore;

/// Typed result of resolving where a `task.resume` should land.
///
/// The decision is a single source of truth — callers MUST NOT
/// invent a hat id from `Allow(_)`; they MUST publish with the
/// resolved `target` so the bus's direct-target fast path is
/// exercised. The `Block` reason is for diagnostics / bounded
/// retry, not for the runtime to forward the resume anywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeDecision {
    /// Resolved a registered hat; publish with `Event::target = Some(target)`.
    Allow {
        target: HatId,
        source: ResumeTargetSource,
    },
    /// Same loop + hat + task identity + retry key already pending —
    /// drop without re-queueing to avoid resume storms (D6).
    Duplicate { target: HatId, retry_key: String },
    /// Target unknown / unregistered / cross-loop / closed-task /
    /// owner-mismatch. The runtime MUST NOT publish.
    Block { reason: ResumeBlockReason },
}

/// Which priority step supplied the resolved target. The dispatch
/// may surface this in diagnostics so callers can confirm the
/// metadata was preserved across the rebuild boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeTargetSource {
    /// `Event.target` was already stamped on the published event.
    EventTarget,
    /// `target_hat` from the resume payload (validated against the
    /// registry + task identity when present).
    PayloadTargetHat,
    /// `owner_hat_id` of a same-loop open task referenced by
    /// `task_id` or `task_key` in the payload.
    OpenTaskOwner,
}

/// Why a `task.resume` was rejected. Stable codes so dashboards
/// and the drift detector can group rejections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeBlockReason {
    /// The target was not provided and no safe fallback
    /// (no task identity, no current-loop open task).
    MissingTarget,
    /// The target points to a hat that is not registered.
    UnknownTarget { target: String },
    /// Payload `target_hat` and the task's `owner_hat_id` disagree.
    /// Per D4 conflicts are fail-closed; the resolver does NOT pick
    /// a winner.
    TargetOwnerConflict {
        payload_target: String,
        owner_hat: String,
    },
    /// `retry_key` was `None` or empty after filtering. Empty is
    /// a valid dedup identity, so the resolver must reject
    /// caller code that does not sign the recovery context
    /// instead of silently letting equivalent resumes
    /// collapse.
    MissingRetryKey,
    /// The registry swap between resolve and publish removed
    /// the resolved target from the registry. Recorded for
    /// diagnostics; the publish is suppressed.
    UnknownTargetRace { target: String },
    /// Two or more open tasks in the same loop carry the
    /// same `task_key`. The owner pick would be
    /// non-deterministic, so the resolver fails closed.
    DuplicateTaskKey { task_key: String },
    /// `task_id` / `task_key` reference a task that is closed,
    /// missing, or belongs to a different loop.
    UnresolvableTask {
        task_id: Option<String>,
        task_key: Option<String>,
        loop_id: Option<String>,
    },
}

/// Identity tuple used to deduplicate equivalent pending resumes
/// per D6. Two resumes that share this tuple are treated as the
/// same recovery intent; the second one returns
/// [`ResumeDecision::Duplicate`] instead of re-queueing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PendingResumeIdentity {
    pub loop_id: Option<String>,
    pub hat: String,
    pub task_id: Option<String>,
    pub task_key: Option<String>,
    pub retry_key: String,
    /// Exact payload when the identity was projected from a live queue.
    /// Runtime-generated events do not persist retry_key separately, so
    /// payload equality is the safe fallback for wrapper-level dedup.
    pub payload: Option<String>,
}

/// Inputs the resolver needs. All fields are optional except the
/// registry / task_store presence — the resolver validates
/// anything that is supplied, never fabricates a target from
/// empty input.
#[derive(Debug, Clone, Default)]
pub struct ResumeRoutingInputs<'a> {
    /// Explicit `Event.target` (or JSONL `triggered` already
    /// promoted to `target` by `From<JsonlEvent>`).
    pub event_target: Option<&'a str>,
    /// `target_hat` field from the resume payload. Must be
    /// already-validated as a non-empty string by the caller.
    pub payload_target_hat: Option<&'a str>,
    /// `task_id` / `task_key` from the resume payload. Used for
    /// the open-task fallback and the target / owner cross-check.
    pub task_id: Option<&'a str>,
    pub task_key: Option<&'a str>,
    /// The retry key recorded by the rejection / recovery envelope.
    pub retry_key: Option<&'a str>,
    /// Payload used by the publisher to compare against an already-pending
    /// event when the live EventBus surface does not expose retry_key.
    pub payload: Option<&'a str>,
    /// Current loop id (used to scope the task fallback and the
    /// dedup tuple).
    pub loop_id: Option<&'a str>,
}

/// Resolver entry point. The registry check is mandatory;
/// passing a task store is optional but enables the
/// `OpenTaskOwner` fallback. `existing_pending` lets the caller
/// short-circuit a duplicate before publishing.
pub fn resolve_resume_target<I>(
    inputs: &ResumeRoutingInputs<'_>,
    registry: &I,
    task_store: Option<&TaskStore>,
    existing_pending: &[PendingResumeIdentity],
) -> ResumeDecision
where
    I: RegisteredHats,
{
    // Plan 2026-08-10-001 U2 R2: non-empty `retry_key`
    // contract. Empty is a valid dedup identity, so failing
    // closed here prevents equivalent resumes from being
    // collapsed by an empty-key signature. Each production
    // caller must derive a deterministic `retry_key` from
    // its recovery context.
    let retry_key_owned = inputs
        .retry_key
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let Some(retry_key) = retry_key_owned else {
        return ResumeDecision::Block {
            reason: ResumeBlockReason::MissingRetryKey,
        };
    };
    let loop_id = inputs.loop_id.map(|s| s.to_string());

    // 1. Explicit Event.target.
    let explicit_target = inputs.event_target.map(str::to_string);

    // 2. Payload target_hat — only meaningful when present.
    let payload_target = inputs.payload_target_hat.map(str::to_string);

    // 3. Open-task owner fallback — only when we have a task
    //    identity and a TaskStore. Same-loop scoping is enforced
    //    inside `find_open_task_id_in_loop`.
    //
    //    Plan 2026-08-10-001 U2 R5: when `task_key` is supplied,
    //    assert uniqueness across open same-loop tasks. Two
    //    non-terminal tasks with the same `task_key` would make
    //    the owner pick non-deterministic; the resolver fails
    //    closed instead of guessing.
    let owner_candidate = if let (Some(store), Some(loop_id_ref)) = (task_store, inputs.loop_id) {
        let task = if let Some(task_id) = inputs.task_id {
            store.find_open_task_id_in_loop(task_id, Some(loop_id_ref))
        } else if let Some(task_key) = inputs.task_key {
            let matches: Vec<&crate::task::Task> = store
                .tasks()
                .iter()
                .filter(|t| {
                    t.key.as_deref() == Some(task_key)
                        && t.loop_id.as_deref() == Some(loop_id_ref)
                        && !t.status.is_terminal()
                })
                .collect();
            if matches.len() > 1 {
                return ResumeDecision::Block {
                    reason: ResumeBlockReason::DuplicateTaskKey {
                        task_key: task_key.to_string(),
                    },
                };
            }
            matches.into_iter().next()
        } else {
            None
        };
        task.and_then(|t| t.owner_hat_id.clone())
    } else {
        None
    };

    // Pick the winner per the priority chain. Any conflict
    // between two non-None candidates fails closed.
    let resolved = match (
        explicit_target.as_deref(),
        payload_target.as_deref(),
        owner_candidate.as_deref(),
    ) {
        (Some(event), _, _) => {
            // Explicit target wins outright; payload / owner must agree
            // (when supplied) but disagreement is fail-closed.
            if let Some(p) = payload_target
                && p != event
            {
                return ResumeDecision::Block {
                    reason: ResumeBlockReason::TargetOwnerConflict {
                        payload_target: p.to_string(),
                        owner_hat: event.to_string(),
                    },
                };
            }
            if let Some(o) = owner_candidate
                && o != event
            {
                return ResumeDecision::Block {
                    reason: ResumeBlockReason::TargetOwnerConflict {
                        payload_target: o.to_string(),
                        owner_hat: event.to_string(),
                    },
                };
            }
            Some((event.to_string(), ResumeTargetSource::EventTarget))
        }
        (None, Some(p), o) => {
            // No explicit target; use payload, but only when it
            // agrees with the owner (when present).
            if let Some(o) = o
                && o != p
            {
                return ResumeDecision::Block {
                    reason: ResumeBlockReason::TargetOwnerConflict {
                        payload_target: p.to_string(),
                        owner_hat: o.to_string(),
                    },
                };
            }
            Some((p.to_string(), ResumeTargetSource::PayloadTargetHat))
        }
        (None, None, Some(o)) => Some((o.to_string(), ResumeTargetSource::OpenTaskOwner)),
        (None, None, None) => None,
    };

    let (target_str, source) = match resolved {
        Some(t) => t,
        None => {
            return ResumeDecision::Block {
                reason: ResumeBlockReason::MissingTarget,
            };
        }
    };

    // Registry validation. Unregistered targets fail closed
    // (E3 / E15).
    if !registry.is_registered(&target_str) {
        return ResumeDecision::Block {
            reason: ResumeBlockReason::UnknownTarget { target: target_str },
        };
    }
    let target = HatId::new(target_str);

    // Dedup against the existing pending identity set.
    let identity = PendingResumeIdentity {
        loop_id: loop_id.clone(),
        hat: target.as_str().to_string(),
        task_id: inputs.task_id.map(str::to_string),
        task_key: inputs.task_key.map(str::to_string),
        retry_key: retry_key.to_string(),
        payload: inputs.payload.map(str::to_string),
    };
    if existing_pending.iter().any(|existing| {
        (existing.loop_id.is_none() || existing.loop_id == identity.loop_id)
            && existing.hat == identity.hat
            && (existing.task_id.is_none() || existing.task_id == identity.task_id)
            && (existing.task_key.is_none() || existing.task_key == identity.task_key)
            && (existing.retry_key.is_empty() || existing.retry_key == identity.retry_key)
            && (existing.payload.is_none()
                || identity.payload.is_none()
                || existing.payload == identity.payload)
    }) {
        return ResumeDecision::Duplicate {
            target,
            retry_key: retry_key.to_string(),
        };
    }

    ResumeDecision::Allow { target, source }
}

/// Minimal registry abstraction so the resolver stays decoupled
/// from `HatRegistry`'s full API. Production code passes
/// `&EventLoop.bus` (whose `hat_ids` matches) or a real
/// `HatRegistry`. Tests pass a tiny `HashSet<String>` adapter.
pub trait RegisteredHats {
    fn is_registered(&self, hat_id: &str) -> bool;
}

impl RegisteredHats for std::collections::HashSet<String> {
    fn is_registered(&self, hat_id: &str) -> bool {
        self.contains(hat_id)
    }
}

impl RegisteredHats for crate::hat_registry::HatRegistry {
    fn is_registered(&self, hat_id: &str) -> bool {
        self.ids().any(|id| id.as_str() == hat_id)
    }
}

impl RegisteredHats for ralph_proto::EventBus {
    fn is_registered(&self, hat_id: &str) -> bool {
        self.hat_ids().any(|id| id.as_str() == hat_id)
    }
}

/// Plan 2026-08-10-001 Unit 3: thin publisher wrapper that the
/// runtime call sites use in place of `Event::new("task.resume",
/// payload).with_target(...)`.
///
/// Resolves the target via [`resolve_resume_target`], then either
/// publishes the targeted `task.resume` to the bus (Allow),
/// records the duplicate (Duplicate), or short-circuits with
/// Block. The runtime MUST NOT publish `task.resume` without
/// passing through this helper.
///
/// `existing_pending` is supplied by the caller so the dedup
/// check uses the live queue; tests may pass `&[]`.
pub fn publish_targeted_resume(
    bus: &mut ralph_proto::EventBus,
    inputs: &ResumeRoutingInputs<'_>,
    registry: &impl RegisteredHats,
    task_store: Option<&TaskStore>,
    existing_pending: &[PendingResumeIdentity],
    payload: String,
) -> ResumeDecision {
    let decision = resolve_resume_target(inputs, registry, task_store, existing_pending);
    match &decision {
        ResumeDecision::Allow { target, .. } => {
            // Plan 2026-08-10-001 U2 R2: TOCTOU close. The
            // resolver already validated `registry.is_registered`
            // against the supplied registry — re-check immediately
            // before publish so a boundary swap (wave handler
            // unmount) cannot re-route the resume to a hat that
            // is no longer registered.
            if !registry.is_registered(target.as_str()) {
                return ResumeDecision::Block {
                    reason: ResumeBlockReason::UnknownTargetRace {
                        target: target.as_str().to_string(),
                    },
                };
            }
            let event = ralph_proto::Event::new("task.resume", payload)
                .with_source("orchestrator")
                .with_system_injected()
                .with_target(target.clone());
            bus.publish(event);
        }
        ResumeDecision::Duplicate { .. } => {
            // Drop without re-queueing (D6).
        }
        ResumeDecision::Block { .. } => {
            // Fail-closed: do not publish (E3 / E15).
        }
    }
    decision
}

/// Plan 2026-08-10-001 U1: derive a [`PendingResumeIdentity`]
/// projection from the live bus pending queue for a given hat.
/// Used to short-circuit equivalent pending resumes per D6.
pub fn pending_resume_identities_from_bus(
    bus: &ralph_proto::EventBus,
    hat: &ralph_proto::HatId,
) -> Vec<PendingResumeIdentity> {
    let Some(pending) = bus.peek_pending(hat) else {
        return Vec::new();
    };
    pending
        .iter()
        .filter(|event| event.topic.as_str() == "task.resume")
        .map(|event| PendingResumeIdentity {
            loop_id: None,
            hat: hat.as_str().to_string(),
            task_id: None,
            task_key: None,
            // EventBus does not expose retry_key separately. An empty
            // value makes the resolver use the payload equality fallback.
            retry_key: String::new(),
            payload: Some(event.payload.clone()),
        })
        .collect()
}

/// Plan 2026-08-10-001 U1: single-call-shape wrapper used by
/// every runtime `task.resume` publish site. Wraps
/// [`publish_targeted_resume`] with the live `existing_pending`
/// adapter so callers don't need to inspect `peek_pending`
/// themselves. The caller passes a `target_hint: &str` plus
/// optional identity hints; `retry_key` MUST be non-empty
/// (callers derive a deterministic one from the recovery
/// context).
pub fn publish_targeted_resume_for_hat(
    bus: &mut ralph_proto::EventBus,
    registry: &impl RegisteredHats,
    task_store: Option<&TaskStore>,
    loop_id: Option<&str>,
    target_hint: &str,
    task_id: Option<&str>,
    task_key: Option<&str>,
    retry_key: &str,
    payload: String,
) -> ResumeDecision {
    publish_targeted_resume_for_hat_in(
        bus,
        registry,
        task_store,
        loop_id,
        target_hint,
        task_id,
        task_key,
        retry_key,
        payload,
        None,
    )
}

/// Variant of [`publish_targeted_resume_for_hat`] that
/// writes the diagnostic envelope into the supplied
/// directory instead of the production
/// `.ralph/diagnostics/` default. Used by tests so they
/// can pin a temp dir without touching the repo's
/// diagnostics directory.
pub fn publish_targeted_resume_for_hat_in(
    bus: &mut ralph_proto::EventBus,
    registry: &impl RegisteredHats,
    task_store: Option<&TaskStore>,
    loop_id: Option<&str>,
    target_hint: &str,
    task_id: Option<&str>,
    task_key: Option<&str>,
    retry_key: &str,
    payload: String,
    diagnostics_dir: Option<&std::path::Path>,
) -> ResumeDecision {
    let target_hat = ralph_proto::HatId::new(target_hint);
    let existing = pending_resume_identities_from_bus(bus, &target_hat);
    let inputs = ResumeRoutingInputs {
        event_target: Some(target_hint),
        payload_target_hat: None,
        task_id,
        task_key,
        retry_key: Some(retry_key),
        loop_id,
        payload: Some(&payload),
    };
    let decision = publish_targeted_resume(
        bus,
        &inputs,
        registry,
        task_store,
        &existing,
        payload.clone(),
    );
    if let ResumeDecision::Block { reason } = &decision {
        // Plan 2026-08-10-001 U2 R4: every Block decision
        // produces a public diagnostic envelope. The
        // envelope is intentionally narrow — `target_hint`,
        // `reason_code`, `reason_description`, `retry_key`,
        // `loop_id`, `task_id`, `task_key`. No `EventBus` /
        // `Event.target` internals are surfaced because the
        // envelope flows through the same operator-visible
        // channel as recovery envelopes.
        let envelope = serde_json::json!({
            "schema_version": "task_resume_block_envelope/v1",
            "source": "task_resume_routing",
            "reason_code": reason_code(reason),
            "reason_description": reason_description(reason),
            "target_hint": target_hint,
            "retry_key": retry_key,
            "loop_id": loop_id,
            "task_id": task_id,
            "task_key": task_key,
        });
        let dir = diagnostics_dir
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from(".ralph/diagnostics"));
        let _ = write_envelope_to_dir(&dir, &envelope);
    }
    decision
}

fn reason_code(reason: &ResumeBlockReason) -> &'static str {
    match reason {
        ResumeBlockReason::MissingTarget => "missing_target",
        ResumeBlockReason::UnknownTarget { .. } => "unknown_target",
        ResumeBlockReason::TargetOwnerConflict { .. } => "target_owner_conflict",
        ResumeBlockReason::UnresolvableTask { .. } => "unresolvable_task",
        ResumeBlockReason::MissingRetryKey => "missing_retry_key",
        ResumeBlockReason::UnknownTargetRace { .. } => "unknown_target_race",
        ResumeBlockReason::DuplicateTaskKey { .. } => "duplicate_task_key",
    }
}

fn reason_description(reason: &ResumeBlockReason) -> String {
    match reason {
        ResumeBlockReason::MissingTarget => "no target supplied and no safe fallback".to_string(),
        ResumeBlockReason::UnknownTarget { target } => format!("hat `{target}` not registered"),
        ResumeBlockReason::TargetOwnerConflict {
            payload_target,
            owner_hat,
        } => format!("payload_target=`{payload_target}` disagrees with owner_hat=`{owner_hat}`"),
        ResumeBlockReason::UnresolvableTask { .. } => {
            "task reference is closed / missing / cross-loop".to_string()
        }
        ResumeBlockReason::MissingRetryKey => {
            "caller did not sign the recovery context".to_string()
        }
        ResumeBlockReason::UnknownTargetRace { target } => {
            format!("hat `{target}` unregistered between resolve and publish")
        }
        ResumeBlockReason::DuplicateTaskKey { task_key } => {
            format!("two open tasks carry the same task_key `{task_key}`")
        }
    }
}

fn write_envelope_to_dir(
    dir: &std::path::Path,
    envelope: &serde_json::Value,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = dir.join(format!("task_resume_block-{pid}-{nanos}.jsonl"));
    let line = envelope.to_string();
    std::fs::write(path, format!("{line}\n"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Unit-2 resolver tests. They exercise the priority chain,
    //! conflict / unknown / cross-loop / closed-task / duplicate
    //! paths against an in-memory `TaskStore`.
    use super::*;
    use crate::task::{Task, TaskStatus};
    use std::collections::HashSet;
    use tempfile::tempdir;

    fn registry_of(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn task_in_loop(
        id: &str,
        key: Option<&str>,
        owner: Option<&str>,
        status: TaskStatus,
        loop_id: Option<&str>,
    ) -> Task {
        let mut t = Task::new(format!("test-{id}"), 3);
        t.id = id.to_string();
        t.key = key.map(str::to_string);
        t.owner_hat_id = owner.map(str::to_string);
        t.loop_id = loop_id.map(str::to_string);
        t.status = status;
        t
    }

    fn store_with(tasks: Vec<Task>) -> TaskStore {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("tasks.jsonl");
        let mut store = TaskStore::load(&path).expect("load empty store");
        for t in tasks {
            store.add(t);
        }
        store
    }

    #[test]
    fn explicit_event_target_wins_and_is_published_with_that_target() {
        let registry = registry_of(&["executor", "observer"]);
        let inputs = ResumeRoutingInputs {
            event_target: Some("executor"),
            retry_key: Some("unit_test_explicit_target"),
            ..Default::default()
        };
        let decision = resolve_resume_target(&inputs, &registry, None, &[]);
        match decision {
            ResumeDecision::Allow { target, source } => {
                assert_eq!(target.as_str(), "executor");
                assert_eq!(source, ResumeTargetSource::EventTarget);
            }
            other => panic!("expected Allow, got {other:?}"),
        }
    }

    #[test]
    fn unknown_target_fails_closed_without_broadcast() {
        let registry = registry_of(&["executor"]);
        let inputs = ResumeRoutingInputs {
            event_target: Some("ralph"),
            retry_key: Some("unit_test_unknown_target"),
            ..Default::default()
        };
        let decision = resolve_resume_target(&inputs, &registry, None, &[]);
        match decision {
            ResumeDecision::Block {
                reason: ResumeBlockReason::UnknownTarget { target },
            } => assert_eq!(target, "ralph"),
            other => panic!("expected Block UnknownTarget, got {other:?}"),
        }
    }

    #[test]
    fn missing_target_with_no_task_identity_is_blocked() {
        let registry = registry_of(&["executor"]);
        let inputs = ResumeRoutingInputs {
            retry_key: Some("unit_test_missing_target"),
            ..Default::default()
        };
        let decision = resolve_resume_target(&inputs, &registry, None, &[]);
        assert!(matches!(
            decision,
            ResumeDecision::Block {
                reason: ResumeBlockReason::MissingTarget
            }
        ));
    }

    #[test]
    fn payload_target_hat_falls_through_when_no_event_target_and_owner_disagrees() {
        let store = store_with(vec![task_in_loop(
            "task-1",
            Some("k1"),
            Some("observer"),
            TaskStatus::Open,
            Some("loop-A"),
        )]);
        let registry = registry_of(&["executor", "observer"]);
        let inputs = ResumeRoutingInputs {
            payload_target_hat: Some("executor"),
            task_id: Some("task-1"),
            loop_id: Some("loop-A"),
            retry_key: Some("unit_test_payload_owner_conflict"),
            ..Default::default()
        };
        let decision = resolve_resume_target(&inputs, &registry, Some(&store), &[]);
        match decision {
            ResumeDecision::Block {
                reason:
                    ResumeBlockReason::TargetOwnerConflict {
                        payload_target,
                        owner_hat,
                    },
            } => {
                assert_eq!(payload_target, "executor");
                assert_eq!(owner_hat, "observer");
            }
            other => panic!("expected Block TargetOwnerConflict, got {other:?}"),
        }
    }

    #[test]
    fn payload_target_hat_agrees_with_owner_is_allowed() {
        let store = store_with(vec![task_in_loop(
            "task-1",
            Some("k1"),
            Some("executor"),
            TaskStatus::Open,
            Some("loop-A"),
        )]);
        let registry = registry_of(&["executor"]);
        let inputs = ResumeRoutingInputs {
            payload_target_hat: Some("executor"),
            task_id: Some("task-1"),
            loop_id: Some("loop-A"),
            retry_key: Some("unit_test_payload_owner_agrees"),
            ..Default::default()
        };
        let decision = resolve_resume_target(&inputs, &registry, Some(&store), &[]);
        match decision {
            ResumeDecision::Allow { target, source } => {
                assert_eq!(target.as_str(), "executor");
                assert_eq!(source, ResumeTargetSource::PayloadTargetHat);
            }
            other => panic!("expected Allow, got {other:?}"),
        }
    }

    #[test]
    fn open_task_owner_is_used_when_payload_target_absent() {
        let store = store_with(vec![task_in_loop(
            "task-2",
            Some("k2"),
            Some("executor"),
            TaskStatus::Open,
            Some("loop-A"),
        )]);
        let registry = registry_of(&["executor"]);
        let inputs = ResumeRoutingInputs {
            task_id: Some("task-2"),
            loop_id: Some("loop-A"),
            retry_key: Some("unit_test_open_task_owner"),
            ..Default::default()
        };
        let decision = resolve_resume_target(&inputs, &registry, Some(&store), &[]);
        match decision {
            ResumeDecision::Allow { target, source } => {
                assert_eq!(target.as_str(), "executor");
                assert_eq!(source, ResumeTargetSource::OpenTaskOwner);
            }
            other => panic!("expected Allow, got {other:?}"),
        }
    }

    #[test]
    fn closed_task_owner_is_not_used_as_fallback() {
        let store = store_with(vec![task_in_loop(
            "task-3",
            Some("k3"),
            Some("executor"),
            TaskStatus::Closed,
            Some("loop-A"),
        )]);
        let registry = registry_of(&["executor"]);
        let inputs = ResumeRoutingInputs {
            task_id: Some("task-3"),
            loop_id: Some("loop-A"),
            retry_key: Some("unit_test_closed_task"),
            ..Default::default()
        };
        let decision = resolve_resume_target(&inputs, &registry, Some(&store), &[]);
        assert!(
            matches!(
                decision,
                ResumeDecision::Block {
                    reason: ResumeBlockReason::MissingTarget
                }
            ),
            "closed task must not seed an owner fallback: {decision:?}"
        );
    }

    #[test]
    fn cross_loop_task_owner_is_not_used_as_fallback() {
        let store = store_with(vec![task_in_loop(
            "task-4",
            Some("k4"),
            Some("executor"),
            TaskStatus::Open,
            Some("loop-OTHER"),
        )]);
        let registry = registry_of(&["executor"]);
        let inputs = ResumeRoutingInputs {
            task_id: Some("task-4"),
            loop_id: Some("loop-A"),
            retry_key: Some("unit_test_cross_loop"),
            ..Default::default()
        };
        let decision = resolve_resume_target(&inputs, &registry, Some(&store), &[]);
        assert!(matches!(
            decision,
            ResumeDecision::Block {
                reason: ResumeBlockReason::MissingTarget
            }
        ));
    }

    #[test]
    fn duplicate_pending_resume_is_short_circuited() {
        let registry = registry_of(&["executor"]);
        let inputs = ResumeRoutingInputs {
            event_target: Some("executor"),
            task_id: Some("task-dup"),
            retry_key: Some("rk-1"),
            loop_id: Some("loop-A"),
            ..Default::default()
        };
        let existing = vec![PendingResumeIdentity {
            loop_id: Some("loop-A".to_string()),
            hat: "executor".to_string(),
            task_id: Some("task-dup".to_string()),
            task_key: None,
            retry_key: "rk-1".to_string(),
            payload: None,
        }];
        let decision = resolve_resume_target(&inputs, &registry, None, &existing);
        match decision {
            ResumeDecision::Duplicate { target, retry_key } => {
                assert_eq!(target.as_str(), "executor");
                assert_eq!(retry_key, "rk-1");
            }
            other => panic!("expected Duplicate, got {other:?}"),
        }
    }

    /// Unit-3 publisher wrapper routes Allow into the bus with the
    /// resolved `target`. Block decisions never reach the bus.
    #[test]
    fn publish_targeted_resume_routes_allow_to_target_hat_only() {
        use ralph_proto::{EventBus, Hat};
        let mut bus = EventBus::new();
        let executor = Hat::new("executor", "Executor").subscribe("plan.ready");
        let observer = Hat::new("observer", "Observer").subscribe("plan.ready");
        bus.register(executor);
        bus.register(observer);
        let registry = registry_of(&["executor", "observer"]);
        let inputs = ResumeRoutingInputs {
            event_target: Some("executor"),
            retry_key: Some("unit_test_publisher_allow"),
            ..Default::default()
        };
        let decision =
            publish_targeted_resume(&mut bus, &inputs, &registry, None, &[], "{\"x\":1}".into());
        assert!(matches!(decision, ResumeDecision::Allow { .. }));
        let exec_pending = bus
            .peek_pending(&ralph_proto::HatId::new("executor"))
            .unwrap();
        let obs_pending = bus
            .peek_pending(&ralph_proto::HatId::new("observer"))
            .map(|v| v.len())
            .unwrap_or(0);
        assert_eq!(exec_pending.len(), 1);
        assert_eq!(
            exec_pending[0].target.as_ref().map(|h| h.as_str()),
            Some("executor")
        );
        assert_eq!(
            obs_pending, 0,
            "observer must not receive the targeted resume"
        );
    }

    #[test]
    fn publish_targeted_resume_never_broadcasts_on_block() {
        use ralph_proto::{EventBus, Hat};
        let mut bus = EventBus::new();
        bus.register(Hat::new("executor", "Executor").subscribe("plan.ready"));
        let registry = registry_of(&["executor"]);
        // `MissingRetryKey` is the deterministic Block
        // path for an empty retry_key; the test target is
        // "no broadcast on Block", which MissingRetryKey
        // satisfies cleanly.
        let inputs = ResumeRoutingInputs {
            retry_key: Some("unit_test_publisher_block"),
            ..Default::default()
        };
        let decision =
            publish_targeted_resume(&mut bus, &inputs, &registry, None, &[], "{}".into());
        assert!(matches!(decision, ResumeDecision::Block { .. }));
        for id in bus.hat_ids() {
            let pending = bus.peek_pending(id).map(|v| v.len()).unwrap_or(0);
            assert_eq!(pending, 0, "hat {id} must not receive a blocked resume");
        }
    }

    // Plan 2026-08-10-001 U2 tests:

    #[test]
    fn empty_retry_key_is_rejected() {
        // U2 R2: empty `retry_key` MUST return
        // `Block { MissingRetryKey }`. The resolver filters
        // `retry_key.filter(|s| !s.is_empty())`, so both
        // `None` and `Some("")` collapse into the new
        // reason. Equivalent empty-key dedup must be
        // impossible.
        let registry = registry_of(&["executor"]);
        for empty in [None, Some(""), Some("   ")] {
            // `Some("   ")` is filtered by the empty check
            // too — whitespace is not a signature either.
            let inputs = ResumeRoutingInputs {
                event_target: Some("executor"),
                retry_key: empty,
                ..Default::default()
            };
            let decision = resolve_resume_target(&inputs, &registry, None, &[]);
            assert!(
                matches!(
                    decision,
                    ResumeDecision::Block {
                        reason: ResumeBlockReason::MissingRetryKey,
                    }
                ),
                "empty retry_key `{empty:?}` must block, got {decision:?}"
            );
        }
    }

    #[test]
    fn wrapper_deduplicates_same_pending_payload() {
        use ralph_proto::{EventBus, Hat};
        let mut bus = EventBus::new();
        bus.register(Hat::new("executor", "Executor").subscribe("task.resume"));
        let registry = registry_of(&["executor"]);
        let payload = r#"{"reason":"same-resume","target_hat":"executor"}"#;

        let first = publish_targeted_resume_for_hat(
            &mut bus,
            &registry,
            None,
            Some("loop-A"),
            "executor",
            None,
            None,
            "retry-1",
            payload.to_string(),
        );
        let second = publish_targeted_resume_for_hat(
            &mut bus,
            &registry,
            None,
            Some("loop-A"),
            "executor",
            None,
            None,
            "retry-1",
            payload.to_string(),
        );

        assert!(matches!(first, ResumeDecision::Allow { .. }));
        assert!(matches!(second, ResumeDecision::Duplicate { .. }));
        assert_eq!(
            bus.peek_pending(&ralph_proto::HatId::new("executor"))
                .map(Vec::len),
            Some(1)
        );
    }

    #[test]
    fn registry_swap_between_resolve_and_publish_fails_closed() {
        // U2 R2 TOCTOU close: the publisher wrapper re-checks
        // `registry.is_registered` immediately before
        // publishing. A `Cell`-backed registry that flips
        // from `true` to `false` between resolve and publish
        // returns `Block { UnknownTargetRace }` and does
        // NOT publish.
        use std::cell::Cell;

        struct Flippy {
            hat: &'static str,
            registered: Cell<bool>,
        }

        impl RegisteredHats for Flippy {
            fn is_registered(&self, hat_id: &str) -> bool {
                // First call (resolver) sees registered;
                // second call (publisher) flips to false.
                if hat_id != self.hat {
                    return false;
                }
                let current = self.registered.get();
                if current {
                    self.registered.set(false);
                }
                current
            }
        }

        use ralph_proto::{EventBus, Hat};
        let mut bus = EventBus::new();
        bus.register(Hat::new("victim", "Victim").subscribe("task.resume"));
        let flippy = Flippy {
            hat: "victim",
            registered: Cell::new(true),
        };
        let inputs = ResumeRoutingInputs {
            event_target: Some("victim"),
            retry_key: Some("u2_race_test"),
            ..Default::default()
        };
        let decision = publish_targeted_resume(&mut bus, &inputs, &flippy, None, &[], "{}".into());
        assert!(
            matches!(
                decision,
                ResumeDecision::Block {
                    reason: ResumeBlockReason::UnknownTargetRace { .. }
                }
            ),
            "registry swap must fail-closed, got {decision:?}"
        );
        let pending = bus
            .peek_pending(&ralph_proto::HatId::new("victim"))
            .unwrap();
        assert_eq!(
            pending.len(),
            0,
            "no task.resume must be published when registry swaps mid-publish"
        );
    }

    #[test]
    fn duplicate_task_key_returns_block() {
        // U2 R5: two open tasks with the same `task_key`
        // in the same loop must return `Block
        // { DuplicateTaskKey }`. The owner pick would be
        // non-deterministic and the contract is fail-close.
        let store = store_with(vec![
            task_in_loop(
                "task-a",
                Some("dup-key"),
                Some("executor"),
                TaskStatus::Open,
                Some("loop-A"),
            ),
            task_in_loop(
                "task-b",
                Some("dup-key"),
                Some("observer"),
                TaskStatus::Open,
                Some("loop-A"),
            ),
        ]);
        let registry = registry_of(&["executor", "observer"]);
        let inputs = ResumeRoutingInputs {
            task_key: Some("dup-key"),
            loop_id: Some("loop-A"),
            retry_key: Some("u2_duplicate_task_key"),
            ..Default::default()
        };
        let decision = resolve_resume_target(&inputs, &registry, Some(&store), &[]);
        match decision {
            ResumeDecision::Block {
                reason: ResumeBlockReason::DuplicateTaskKey { task_key },
            } => assert_eq!(task_key, "dup-key"),
            other => panic!("expected Block DuplicateTaskKey, got {other:?}"),
        }
    }

    #[test]
    fn block_decision_writes_diagnostic_envelope() {
        // U2 R4: every `Block` decision must write a
        // `<dir>/task_resume_block-<pid>-<nanos>.jsonl`
        // envelope. Use the `_in` helper to pin the
        // diagnostics directory; the production
        // `.ralph/diagnostics/` default is never written
        // during tests.
        use ralph_proto::{EventBus, Hat};
        let temp_dir = tempfile::tempdir().expect("tempdir");

        let mut bus = EventBus::new();
        bus.register(Hat::new("executor", "Executor").subscribe("task.resume"));
        let registry = registry_of(&["executor"]);
        let decision = publish_targeted_resume_for_hat_in(
            &mut bus,
            &registry,
            None,
            Some("loop-A"),
            "victim",
            None,
            None,
            "u2_block_envelope",
            "{}".to_string(),
            Some(temp_dir.path()),
        );
        assert!(matches!(decision, ResumeDecision::Block { .. }));

        let mut envelopes = Vec::new();
        for entry in std::fs::read_dir(temp_dir.path()).expect("read dir") {
            let entry = entry.expect("entry");
            let path = entry.path();
            if path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with("task_resume_block-"))
            {
                let body = std::fs::read_to_string(&path).expect("read envelope");
                envelopes.push(body);
            }
        }
        assert_eq!(
            envelopes.len(),
            1,
            "exactly one envelope must be written for a single Block decision, got {envelopes:?}"
        );
        let envelope_json: serde_json::Value =
            serde_json::from_str(envelopes[0].trim()).expect("envelope must be valid JSON");
        assert_eq!(
            envelope_json["schema_version"], "task_resume_block_envelope/v1",
            "schema_version must match"
        );
        assert_eq!(
            envelope_json["source"], "task_resume_routing",
            "source must identify the resolver"
        );
        assert_eq!(
            envelope_json["reason_code"], "unknown_target",
            "wrong-target publishes surface as unknown_target via the helper"
        );
        assert_eq!(
            envelope_json["target_hint"], "victim",
            "target_hint must surface for operator triage"
        );
        assert_eq!(envelope_json["retry_key"], "u2_block_envelope");
        assert_eq!(envelope_json["loop_id"], "loop-A");
    }
}
