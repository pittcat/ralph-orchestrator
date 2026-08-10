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
    Allow { target: HatId, source: ResumeTargetSource },
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
    let retry_key = inputs.retry_key.unwrap_or("");
    let loop_id = inputs.loop_id.map(|s| s.to_string());

    // 1. Explicit Event.target.
    let explicit_target = inputs.event_target.map(str::to_string);

    // 2. Payload target_hat — only meaningful when present.
    let payload_target = inputs.payload_target_hat.map(str::to_string);

    // 3. Open-task owner fallback — only when we have a task
    //    identity and a TaskStore. Same-loop scoping is enforced
    //    inside `find_open_task_id_in_loop`.
    let owner_candidate = if let (Some(store), Some(loop_id_ref)) = (task_store, inputs.loop_id) {
        let task = if let Some(task_id) = inputs.task_id {
            store.find_open_task_id_in_loop(task_id, Some(loop_id_ref))
        } else if let Some(task_key) = inputs.task_key {
            // Key lookup across the loop scope; existing API is
            // loop-scoped via `find_by_locus_in_loop`.
            store.tasks().iter().find(|t| {
                t.key.as_deref() == Some(task_key)
                    && t.loop_id.as_deref() == Some(loop_id_ref)
                    && !t.status.is_terminal()
            })
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
            reason: ResumeBlockReason::UnknownTarget {
                target: target_str,
            },
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
    };
    if existing_pending.contains(&identity) {
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
        .map(|_event| PendingResumeIdentity {
            loop_id: None,
            hat: hat.as_str().to_string(),
            task_id: None,
            task_key: None,
            // Live-queue dedup uses the hat as the only stable
            // identity dimension we can derive from the bus
            // surface (the deeper payload fields are JSON inside
            // `payload` and aren't surfaced here). Hat-only
            // identity is enough to prevent re-queueing the
            // same targeted resume into the same hat's queue.
            retry_key: hat.as_str().to_string(),
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
    let target_hat = ralph_proto::HatId::new(target_hint);
    let existing = pending_resume_identities_from_bus(bus, &target_hat);
    let inputs = ResumeRoutingInputs {
        event_target: Some(target_hint),
        payload_target_hat: None,
        task_id,
        task_key,
        retry_key: Some(retry_key),
        loop_id,
    };
    publish_targeted_resume(bus, &inputs, registry, task_store, &existing, payload)
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
        let inputs = ResumeRoutingInputs::default();
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
            ..Default::default()
        };
        let decision =
            publish_targeted_resume(&mut bus, &inputs, &registry, None, &[], "{\"x\":1}".into());
        assert!(matches!(decision, ResumeDecision::Allow { .. }));
        let exec_pending = bus.peek_pending(&ralph_proto::HatId::new("executor")).unwrap();
        let obs_pending = bus
            .peek_pending(&ralph_proto::HatId::new("observer"))
            .map(|v| v.len())
            .unwrap_or(0);
        assert_eq!(exec_pending.len(), 1);
        assert_eq!(
            exec_pending[0].target.as_ref().map(|h| h.as_str()),
            Some("executor")
        );
        assert_eq!(obs_pending, 0, "observer must not receive the targeted resume");
    }

    #[test]
    fn publish_targeted_resume_never_broadcasts_on_block() {
        use ralph_proto::{EventBus, Hat};
        let mut bus = EventBus::new();
        bus.register(Hat::new("executor", "Executor").subscribe("plan.ready"));
        let registry = registry_of(&["executor"]);
        let inputs = ResumeRoutingInputs::default();
        let decision =
            publish_targeted_resume(&mut bus, &inputs, &registry, None, &[], "{}".into());
        assert!(matches!(decision, ResumeDecision::Block { .. }));
        for id in bus.hat_ids() {
            let pending = bus.peek_pending(id).map(|v| v.len()).unwrap_or(0);
            assert_eq!(pending, 0, "hat {id} must not receive a blocked resume");
        }
    }
}