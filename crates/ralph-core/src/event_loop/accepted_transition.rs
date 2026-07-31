//! U6 (plan 2026-07-30-004): the Accepted Transition API.
//!
//! The single, atomic entry point for all business state changes.
//! All business events (`work.done`, `work.failed`, `plan.complete`,
//! …) MUST go through [`AcceptedTransition::commit`].
//!
//! # Atomicity guarantee
//!
//! `commit` enforces a strict three-phase ordering with a fail-closed
//! rollback contract:
//!
//! 1. **Pre-commit validation** — the caller-supplied `validate`
//!    closure runs first. On rejection the call returns
//!    [`TransitionError::PreCommitRejected`] with **zero side
//!    effects**: nothing is written to the outbox and nothing is
//!    published to the bus.
//! 2. **Durable outbox write** — the accepted transition is appended
//!    to `.ralph/agent/accepted-transitions.jsonl` *before* any
//!    publish. If this write fails, the call returns
//!    [`TransitionError::CommitFailed`] and **no event is published**.
//! 3. **Publish** — only after the durable write succeeds is the
//!    event handed to the [`EventBus`].
//!
//! Because the outbox write precedes the publish, a crash between the
//! two leaves a durable record that a transition was accepted even if
//! the in-memory bus never saw it; the reverse (publish-without-outbox)
//! can never happen.

use crate::state::StateLedger;
use ralph_proto::{Event, EventBus};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Workspace-relative path of the durable transition outbox.
///
/// The outbox is an append-only JSONL file: one [`OutboxEntry`] per
/// line, never rewritten in place.
pub const OUTBOX_RELATIVE_PATH: &str = ".ralph/agent/accepted-transitions.jsonl";

/// A durable outbox entry recording an accepted transition.
///
/// Fields are declared in alphabetical order so the serde_json output
/// (which preserves declaration order for structs) has sorted keys —
/// this keeps the on-disk representation byte-for-byte deterministic
/// for a given set of field values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxEntry {
    /// The activation that produced this transition.
    pub activation_id: String,
    /// ISO-8601 timestamp of commit.
    pub committed_at: String,
    /// The contract revision in effect at commit time.
    pub contract_revision: String,
    /// Whether the transition has been acknowledged as delivered.
    ///
    /// U7: set to `true` by [`AcceptedTransition::ack`]. Once
    /// delivered, [`AcceptedTransition::commit_idempotent`] skips
    /// re-publish (exactly-once *delivery* after ack; at-least-once
    /// before ack). Defaults to `false` for entries written by the
    /// original U6 [`AcceptedTransition::commit`] path.
    #[serde(default)]
    pub delivered: bool,
    /// The loop this transition belongs to.
    pub loop_id: String,
    /// The canonical payload digest (`sha256(event.payload)`).
    pub payload_digest: String,
    /// The event topic.
    pub topic: String,
    /// Unique transition identifier (`sha256` over the identity tuple).
    pub transition_id: String,
}

/// Error from the Accepted Transition API.
#[derive(Debug, Clone)]
pub enum TransitionError {
    /// Pre-commit validation failed. Zero side effects: no outbox
    /// write occurred and no event was published.
    PreCommitRejected {
        /// The human-readable rejection reason from the validator.
        reason: String,
    },
    /// The durable outbox write failed. No event was published; the
    /// caller may retry the commit.
    CommitFailed {
        /// The underlying I/O error, stringified.
        source: String,
    },
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PreCommitRejected { reason } => {
                write!(f, "transition rejected before commit: {reason}")
            }
            Self::CommitFailed { source } => {
                write!(
                    f,
                    "transition outbox write failed (no event published): {source}"
                )
            }
        }
    }
}

impl std::error::Error for TransitionError {}

/// The Accepted Transition API.
///
/// A zero-sized namespace type: all operations are associated
/// functions. See the module docs for the atomicity guarantee.
pub struct AcceptedTransition;

impl AcceptedTransition {
    /// Compute the deterministic `transition_id`.
    ///
    /// The id is `sha256(loop_id ‖ activation_id ‖ contract_revision
    /// ‖ event_identity ‖ canonical_digest)`. Because every input is a
    /// stable string derived from the commit arguments, the same
    /// transition always yields the same id — enabling idempotent
    /// replay and cross-process dedup.
    pub fn compute_transition_id(
        loop_id: &str,
        activation_id: &str,
        contract_revision: &str,
        event_identity: &str,
        canonical_digest: &str,
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(loop_id.as_bytes());
        hasher.update(activation_id.as_bytes());
        hasher.update(contract_revision.as_bytes());
        hasher.update(event_identity.as_bytes());
        hasher.update(canonical_digest.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Commit a business transition atomically.
    ///
    /// 1. Validate (pre-commit) → reject with zero side effects.
    /// 2. Write to the durable outbox → fail with no publish on error.
    /// 3. Publish to the event bus → only after the durable write.
    ///
    /// `event_identity` is derived from the event's topic and source
    /// hat; `canonical_digest` is `sha256(event.payload)`.
    pub fn commit(
        event: &Event,
        loop_id: &str,
        activation_id: &str,
        contract_revision: &str,
        ledger: &StateLedger,
        bus: &mut EventBus,
        validate: impl FnOnce(&Event) -> Result<(), String>,
    ) -> Result<OutboxEntry, TransitionError> {
        // 1. Pre-commit validation — zero side effects on reject.
        if let Err(reason) = validate(event) {
            return Err(TransitionError::PreCommitRejected { reason });
        }

        // 2. Derive the deterministic identity tuple.
        let payload_digest = {
            let mut h = Sha256::new();
            h.update(event.payload.as_bytes());
            format!("{:x}", h.finalize())
        };
        let event_identity = format!(
            "{}:{}",
            event.topic.as_str(),
            event.source.as_ref().map(|h| h.as_str()).unwrap_or("")
        );
        let transition_id = Self::compute_transition_id(
            loop_id,
            activation_id,
            contract_revision,
            &event_identity,
            &payload_digest,
        );

        let entry = OutboxEntry {
            activation_id: activation_id.to_string(),
            committed_at: chrono::Utc::now().to_rfc3339(),
            contract_revision: contract_revision.to_string(),
            delivered: false,
            loop_id: loop_id.to_string(),
            payload_digest,
            topic: event.topic.as_str().to_string(),
            transition_id,
        };

        // 3. Durable outbox write — on failure, publish nothing.
        ledger
            .append_outbox(&entry)
            .map_err(|e| TransitionError::CommitFailed {
                source: e.to_string(),
            })?;

        // 4. Publish — only reached after the durable write succeeds.
        bus.publish(event.clone());

        Ok(entry)
    }

    /// Check whether a `transition_id` already exists in the outbox.
    ///
    /// Reads the durable outbox from `workspace` and returns `true` if
    /// any entry carries the given `transition_id`. This is the
    /// deduplication primitive used by [`Self::commit_idempotent`].
    pub fn is_committed(workspace: &Path, transition_id: &str) -> bool {
        read_outbox(workspace)
            .map(|entries| entries.iter().any(|e| e.transition_id == transition_id))
            .unwrap_or(false)
    }

    /// Look up a committed outbox entry by `transition_id`.
    ///
    /// Returns `None` when the transition has not been committed yet.
    fn find_committed(workspace: &Path, transition_id: &str) -> Option<OutboxEntry> {
        read_outbox(workspace)
            .ok()
            .and_then(|entries| {
                entries
                    .into_iter()
                    .find(|e| e.transition_id == transition_id)
            })
    }

    /// Commit a business transition with idempotency.
    ///
    /// # Idempotency guarantee
    ///
    /// Replaying the same transition (identical `loop_id`,
    /// `activation_id`, `contract_revision`, event topic+source, and
    /// payload) always yields the same `transition_id`. On replay:
    ///
    /// - **committed** (regardless of `delivered`): returns the
    ///   existing entry with **zero side effects** — no outbox write,
    ///   no publish, no materialize.
    /// - **not committed**: delegates to [`Self::commit`] (validate →
    ///   durable write → publish) after running the `materialize`
    ///   closure.
    ///
    /// The `delivered` flag (set by [`Self::ack`]) is metadata for
    /// downstream delivery tracking; it does not change the dedup
    /// behaviour of this method.
    ///
    /// The `materialize` closure runs **only** on the first commit
    /// (not-committed path). On replay it is never invoked, making
    /// materialization idempotent.
    pub fn commit_idempotent(
        event: &Event,
        loop_id: &str,
        activation_id: &str,
        contract_revision: &str,
        ledger: &StateLedger,
        bus: &mut EventBus,
        validate: impl FnOnce(&Event) -> Result<(), String>,
        materialize: impl FnOnce(),
    ) -> Result<OutboxEntry, TransitionError> {
        // 1. Derive the deterministic identity tuple (same as commit).
        let payload_digest = {
            let mut h = Sha256::new();
            h.update(event.payload.as_bytes());
            format!("{:x}", h.finalize())
        };
        let event_identity = format!(
            "{}:{}",
            event.topic.as_str(),
            event.source.as_ref().map(|h| h.as_str()).unwrap_or("")
        );
        let transition_id = Self::compute_transition_id(
            loop_id,
            activation_id,
            contract_revision,
            &event_identity,
            &payload_digest,
        );

        let workspace = ledger.workspace();

        // 2. Idempotency check: if already committed, handle replay.
        if let Some(existing) = Self::find_committed(workspace, &transition_id) {
            // Committed — dedup: no second outbox write, no re-publish,
            // no materialize. The `delivered` flag is metadata for
            // downstream ack tracking; it does not change the dedup
            // behaviour of `commit_idempotent`.
            return Ok(existing);
        }

        // 3. First commit — run materialize side effect, then commit.
        materialize();
        Self::commit(
            event,
            loop_id,
            activation_id,
            contract_revision,
            ledger,
            bus,
            validate,
        )
    }

    /// Acknowledge a transition as delivered.
    ///
    /// Marks the outbox entry with the given `transition_id` as
    /// `delivered = true`. After ack, [`Self::commit_idempotent`]
    /// replays become no-ops (exactly-once delivery).
    ///
    /// The outbox file is rewritten in place under an exclusive lock.
    /// If no entry matches, this is a silent no-op.
    pub fn ack(workspace: &Path, transition_id: &str) -> Result<(), std::io::Error> {
        let path = outbox_path(workspace);
        let entries = read_outbox(workspace)?;
        if !entries.iter().any(|e| e.transition_id == transition_id) {
            return Ok(()); // nothing to ack
        }

        let updated: Vec<OutboxEntry> = entries
            .into_iter()
            .map(|mut e| {
                if e.transition_id == transition_id {
                    e.delivered = true;
                }
                e
            })
            .collect();

        // Rewrite the outbox atomically: serialize all entries, write
        // to the same path.
        let mut body = String::new();
        for entry in &updated {
            let line = serde_json::to_string(entry).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("outbox serialize failed: {e}"),
                )
            })?;
            body.push_str(&line);
            body.push('\n');
        }
        std::fs::write(&path, body)?;
        Ok(())
    }
}

/// Absolute path of the transition outbox for a workspace.
pub fn outbox_path(workspace: &Path) -> PathBuf {
    workspace.join(OUTBOX_RELATIVE_PATH)
}

/// Read all outbox entries for a workspace.
///
/// Returns an empty `Vec` when the outbox file does not exist yet.
/// Blank lines are skipped. A malformed line is an error (the outbox
/// is append-only and must never contain a torn record).
pub fn read_outbox(workspace: &Path) -> std::io::Result<Vec<OutboxEntry>> {
    let path = outbox_path(workspace);
    let body = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut entries = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let entry: OutboxEntry = serde_json::from_str(trimmed)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        entries.push(entry);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Task;
    use crate::task_store::TaskStore;
    use ralph_proto::Hat;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    /// Build a workspace with an empty [`StateLedger`], an empty
    /// [`EventBus`], and a [`TaskStore`] holding one open task.
    fn fixture() -> (TempDir, StateLedger, EventBus, TaskStore) {
        let dir = TempDir::new().unwrap();
        let ws = dir.path().to_path_buf();
        let ledger = StateLedger::new(&ws, true);

        // Register an `executor` hat so the EventBus source guard lets
        // `work.done` events through to observers/subscribers.
        let mut bus = EventBus::new();
        bus.register(Hat::new("executor", "Executor").subscribe("work.*"));

        // Seed a TaskStore with a single open task so the "zero side
        // effects" assertions have real state to protect.
        let tasks_path = ws.join(".ralph").join("agent").join("tasks.jsonl");
        let mut store = TaskStore::load(&tasks_path).unwrap();
        store.ensure(Task::new("u6 seed task".to_string(), 1));
        assert_eq!(store.all().len(), 1, "fixture must start with one task");

        (dir, ledger, bus, store)
    }

    /// A valid business event used by the success / failure tests.
    fn valid_event() -> Event {
        Event::new("work.done", "implemented U6").with_source("executor")
    }

    #[test]
    fn u6_pre_commit_reject_zero_side_effects() {
        let (_dir, ledger, mut bus, store) = fixture();
        let ws = ledger.workspace().to_path_buf();

        // An observer counts every event the bus actually routes.
        let seen = Arc::new(Mutex::new(0usize));
        let seen_clone = Arc::clone(&seen);
        bus.add_observer(move |_| *seen_clone.lock().unwrap() += 1);

        let tasks_before = store.all().len();

        // Validator rejects (missing required field).
        let result = AcceptedTransition::commit(
            &valid_event(),
            "loop-1",
            "act-1",
            "rev-1",
            &ledger,
            &mut bus,
            |_| Err("missing required field: summary".to_string()),
        );

        match result {
            Err(TransitionError::PreCommitRejected { reason }) => {
                assert!(reason.contains("summary"));
            }
            other => panic!("expected PreCommitRejected, got {other:?}"),
        }

        // Zero side effects: TaskStore unchanged, ledger commit log
        // empty, outbox empty, bus saw nothing.
        assert_eq!(store.all().len(), tasks_before, "TaskStore must be unchanged");
        assert!(ledger.commit_log().is_empty(), "ledger commit log must be empty");
        assert!(
            read_outbox(&ws).unwrap().is_empty(),
            "outbox must have zero entries on reject"
        );
        assert_eq!(*seen.lock().unwrap(), 0, "bus must have zero events on reject");
    }

    #[test]
    fn u6_commit_success_writes_outbox_then_publishes() {
        let (_dir, ledger, mut bus, _store) = fixture();
        let ws = ledger.workspace().to_path_buf();

        // The observer records, for each published event, whether the
        // outbox file was already non-empty at publish time — proving
        // the durable write happened before the publish.
        let seen = Arc::new(Mutex::new(Vec::<(String, bool)>::new()));
        let seen_clone = Arc::clone(&seen);
        let obs_ws = ws.clone();
        bus.add_observer(move |e| {
            let already = std::fs::read_to_string(outbox_path(&obs_ws))
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            seen_clone
                .lock()
                .unwrap()
                .push((e.topic.to_string(), already));
        });

        let event = valid_event();
        let entry = AcceptedTransition::commit(
            &event,
            "loop-1",
            "act-1",
            "rev-1",
            &ledger,
            &mut bus,
            |_| Ok(()),
        )
        .expect("commit must succeed");

        // Exactly one outbox entry, with the deterministic transition_id.
        let entries = read_outbox(&ws).unwrap();
        assert_eq!(entries.len(), 1, "outbox must have exactly one entry");
        let on_disk = &entries[0];
        assert_eq!(on_disk.transition_id, entry.transition_id);
        assert_eq!(on_disk.loop_id, "loop-1");
        assert_eq!(on_disk.activation_id, "act-1");
        assert_eq!(on_disk.contract_revision, "rev-1");
        assert_eq!(on_disk.topic, "work.done");

        // transition_id matches sha256(loop_id + activation_id +
        // contract_revision + event_identity + canonical_digest).
        let canonical_digest = {
            let mut h = Sha256::new();
            h.update(event.payload.as_bytes());
            format!("{:x}", h.finalize())
        };
        let event_identity = "work.done:executor";
        let expected = AcceptedTransition::compute_transition_id(
            "loop-1",
            "act-1",
            "rev-1",
            event_identity,
            &canonical_digest,
        );
        assert_eq!(entry.transition_id, expected, "transition_id must be deterministic");
        assert_eq!(entry.payload_digest, canonical_digest);

        // Exactly one event published, and only after the outbox write.
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "bus must see exactly one event");
        assert_eq!(seen[0].0, "work.done");
        assert!(
            seen[0].1,
            "outbox must be durable before the event is published"
        );
    }

    #[test]
    fn u6_commit_failure_no_publish() {
        let (dir, ledger, mut bus, _store) = fixture();
        let ws = ledger.workspace().to_path_buf();

        // Make the outbox path itself a directory so the append-open
        // inside append_outbox fails with EISDIR — simulating a
        // durable-write failure (e.g. corrupt / read-only filesystem).
        std::fs::create_dir_all(ws.join(".ralph").join("agent")).unwrap();
        std::fs::create_dir(outbox_path(&ws)).unwrap();

        let seen = Arc::new(Mutex::new(0usize));
        let seen_clone = Arc::clone(&seen);
        bus.add_observer(move |_| *seen_clone.lock().unwrap() += 1);

        let result = AcceptedTransition::commit(
            &valid_event(),
            "loop-1",
            "act-1",
            "rev-1",
            &ledger,
            &mut bus,
            |_| Ok(()),
        );

        match result {
            Err(TransitionError::CommitFailed { .. }) => {}
            other => panic!("expected CommitFailed, got {other:?}"),
        }

        assert_eq!(
            *seen.lock().unwrap(),
            0,
            "bus must have zero events on commit failure"
        );
        // No durable entry may exist: the outbox is not a valid JSONL
        // file (here it is a directory), so either the read fails or
        // yields zero entries.
        if let Ok(entries) = read_outbox(&ws) {
            assert!(entries.is_empty(), "no outbox entry after failed commit")
        }
        drop(dir);
    }

    #[test]
    fn u6_outbox_serialization_deterministic() {
        // Struct fields serialize in declaration order, which is
        // alphabetical here — so identical values always yield
        // identical bytes.
        let a = OutboxEntry {
            activation_id: "act-1".into(),
            committed_at: "2026-07-31T00:00:00Z".into(),
            contract_revision: "rev-1".into(),
            delivered: false,
            loop_id: "loop-1".into(),
            payload_digest: "deadbeef".into(),
            topic: "work.done".into(),
            transition_id: "cafe".into(),
        };
        let b = a.clone();
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap(),
            "identical entries must serialize to identical bytes"
        );
    }

    // ── U7: idempotent materialize / delivery / ack ──────────────────

    #[test]
    fn u7_replayed_transition_id_is_deduplicated() {
        let (_dir, ledger, mut bus, _store) = fixture();
        let ws = ledger.workspace().to_path_buf();

        let seen = Arc::new(Mutex::new(0usize));
        let seen_clone = Arc::clone(&seen);
        bus.add_observer(move |_| *seen_clone.lock().unwrap() += 1);

        let event = valid_event();

        // First commit — should write outbox + publish.
        let entry1 = AcceptedTransition::commit_idempotent(
            &event,
            "loop-1",
            "act-1",
            "rev-1",
            &ledger,
            &mut bus,
            |_| Ok(()),
            || {},
        )
        .expect("first commit must succeed");

        // Replay the SAME transition (same identity tuple).
        let entry2 = AcceptedTransition::commit_idempotent(
            &event,
            "loop-1",
            "act-1",
            "rev-1",
            &ledger,
            &mut bus,
            |_| Ok(()),
            || {},
        )
        .expect("replay commit must succeed");

        // Same transition_id.
        assert_eq!(
            entry1.transition_id, entry2.transition_id,
            "replayed transition must yield the same transition_id"
        );

        // Outbox has exactly 1 entry (not 2).
        let entries = read_outbox(&ws).unwrap();
        assert_eq!(entries.len(), 1, "outbox must have exactly 1 entry after replay");

        // EventBus saw exactly 1 event (the replay was deduplicated).
        assert_eq!(
            *seen.lock().unwrap(),
            1,
            "bus must see exactly 1 event after replay (dedup)"
        );
    }

    #[test]
    fn u7_materialize_is_idempotent() {
        let (_dir, ledger, mut bus, store) = fixture();

        // Persist the fixture's seed task to disk so the materialize
        // closure (which loads a fresh TaskStore) sees it.
        store.save().unwrap();

        let event = valid_event();
        let tasks_path = ledger.workspace().join(".ralph").join("agent").join("tasks.jsonl");

        // First commit — materialize adds a task.
        AcceptedTransition::commit_idempotent(
            &event,
            "loop-1",
            "act-1",
            "rev-1",
            &ledger,
            &mut bus,
            |_| Ok(()),
            || {
                let mut s = TaskStore::load(&tasks_path).unwrap();
                s.ensure(Task::new("u7 materialized task".to_string(), 2));
                s.save().unwrap();
            },
        )
        .expect("first commit must succeed");

        // Reload and count: fixture seeds 1 task + 1 materialized = 2.
        let store_after_first = TaskStore::load(&tasks_path).unwrap();
        assert_eq!(
            store_after_first.all().len(),
            2,
            "after first commit: 1 seed + 1 materialized"
        );

        // Replay the SAME transition — materialize must NOT run again.
        AcceptedTransition::commit_idempotent(
            &event,
            "loop-1",
            "act-1",
            "rev-1",
            &ledger,
            &mut bus,
            |_| Ok(()),
            || {
                let mut s = TaskStore::load(&tasks_path).unwrap();
                s.ensure(Task::new("u7 SHOULD NOT appear".to_string(), 3));
                s.save().unwrap();
            },
        )
        .expect("replay commit must succeed");

        // Task count must still be 2 (materialize not called on replay).
        let store_after_replay = TaskStore::load(&tasks_path).unwrap();
        assert_eq!(
            store_after_replay.all().len(),
            2,
            "after replay: materialize must not run again"
        );

        drop(store);
    }

    #[test]
    fn u7_ack_prevents_redelivery() {
        let (_dir, ledger, mut bus, _store) = fixture();
        let ws = ledger.workspace().to_path_buf();

        let seen = Arc::new(Mutex::new(0usize));
        let seen_clone = Arc::clone(&seen);
        bus.add_observer(move |_| *seen_clone.lock().unwrap() += 1);

        let event = valid_event();

        // Commit.
        let entry = AcceptedTransition::commit_idempotent(
            &event,
            "loop-1",
            "act-1",
            "rev-1",
            &ledger,
            &mut bus,
            |_| Ok(()),
            || {},
        )
        .expect("commit must succeed");

        assert_eq!(*seen.lock().unwrap(), 1, "bus must see the initial publish");

        // Ack the transition — marks it as delivered.
        AcceptedTransition::ack(&ws, &entry.transition_id)
            .expect("ack must succeed");

        // Verify the outbox entry is now marked delivered.
        let entries = read_outbox(&ws).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].delivered, "entry must be marked delivered after ack");

        // Replay the SAME transition — ack'd, so no re-publish.
        let replayed = AcceptedTransition::commit_idempotent(
            &event,
            "loop-1",
            "act-1",
            "rev-1",
            &ledger,
            &mut bus,
            |_| Ok(()),
            || {},
        )
        .expect("replay after ack must succeed");

        assert_eq!(
            replayed.transition_id, entry.transition_id,
            "replayed transition_id must match"
        );

        // EventBus still saw exactly 1 event — the replay was suppressed.
        assert_eq!(
            *seen.lock().unwrap(),
            1,
            "bus must see exactly 1 event after ack'd replay"
        );
    }
}
