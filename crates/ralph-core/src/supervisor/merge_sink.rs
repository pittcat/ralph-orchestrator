//! `EventMergeSink` — boundary between the supervisor
//! coordinator and the JSONL event ledger (KTD-6).
//!
//! The runtime owns the actual JSONL mutation:
//! `persist_system_injected_jsonl_event` writes through the
//! event reader's existing `system_injected` audit hook. The
//! coordinator never touches disk directly; instead it hands a
//! batch of `ralph_proto::Event`s to an `EventMergeSink`
//! implementation, and `merge_to_events` either succeeds or
//! returns an error. KTD-7 says: a failed merge must NOT
//! inject `*.wave.complete`; recovery (U11) re-runs the merge
//! against the same in-flight slot rows.

use std::fmt::Debug;

use ralph_proto::Event;

/// Trait that abstracts "append events to the runtime ledger".
///
/// U8 unit tests use an in-memory sink; the U12 dispatcher
/// bridge hands the coordinator a sink that wraps
/// `EventLoop::persist_system_injected_jsonl_event` (the
/// existing P0-3 audit hook).
pub trait EventMergeSink: Debug + Send + Sync {
    /// Append `events` to the ledger atomically. On success,
    /// the runtime guarantees the events are durably written
    /// AND past the reader cursor so the next
    /// `process_events_from_jsonl` pass does not re-ingest
    /// them.
    ///
    /// Errors are propagated to the coordinator, which then
    /// short-circuits the `*.wave.complete` injection (KTD-7).
    fn append_events(&self, events: Vec<Event>) -> Result<(), MergeSinkError>;
}

/// Error returned when the merge sink rejects a batch. The
/// coordinator treats this as "merge failed → do not advance
/// phase"; recovery (U11) re-runs the merge.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MergeSinkError {
    #[error("merge sink rejected the batch: {0}")]
    Rejected(String),
}

/// In-memory sink for tests + U8 dry-run. Holds the appended
/// batches so tests can assert what the coordinator attempted
/// to write.
#[derive(Debug, Default, Clone)]
pub struct InMemoryMergeSink {
    batches: std::sync::Arc<std::sync::Mutex<Vec<Vec<Event>>>>,
    /// When set, `append_events` returns this error instead
    /// of recording the batch. Used to exercise KTD-7.
    fail_with: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

impl InMemoryMergeSink {
    /// Build an empty sink that records batches.
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot the batches the coordinator attempted to
    /// write. Tests inspect this to confirm fan-in output.
    pub fn batches(&self) -> Vec<Vec<Event>> {
        self.batches
            .lock()
            .expect("merge sink mutex poisoned")
            .clone()
    }

    /// Force `append_events` to return `MergeSinkError::Rejected`
    /// until `clear_failure` is called. Drives KTD-7 coverage.
    pub fn fail_with(&self, msg: impl Into<String>) {
        *self.fail_with.lock().expect("merge sink mutex poisoned") = Some(msg.into());
    }

    /// Clear the forced-failure flag.
    pub fn clear_failure(&self) {
        self.fail_with
            .lock()
            .expect("merge sink mutex poisoned")
            .clone_from(&None);
    }
}

impl EventMergeSink for InMemoryMergeSink {
    fn append_events(&self, events: Vec<Event>) -> Result<(), MergeSinkError> {
        if let Some(msg) = self
            .fail_with
            .lock()
            .expect("merge sink mutex poisoned")
            .clone()
        {
            return Err(MergeSinkError::Rejected(msg));
        }
        self.batches
            .lock()
            .expect("merge sink mutex poisoned")
            .push(events);
        Ok(())
    }
}
