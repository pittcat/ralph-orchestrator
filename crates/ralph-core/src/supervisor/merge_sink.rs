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

/// Production merge sink (U6): appends the fan-in business
/// events to the loop's main JSONL ledger (`events.jsonl`).
///
/// The coordinator hands the sink the per-slot worker events
/// (sorted by slot index, de-duplicated by the dispatcher's
/// `run_supervisor_fan_in`); the sink serializes each event to
/// the same JSONL record shape the `EventReader` parses
/// (`topic` / `payload` / `ts` / `hat` / `source` / wave
/// correlation fields) and appends them in a single `write_all`
/// so a partial batch never lands on disk. On any I/O error the
/// sink returns `MergeSinkError::Rejected`, which the coordinator
/// maps to `CoordinatorAction::MergeFailed` — leaving
/// `merged_to_events` false so the next tick retries the merge
/// exactly once (KTD-7).
#[derive(Debug, Clone)]
pub struct FileEventMergeSink {
    path: std::path::PathBuf,
}

impl FileEventMergeSink {
    /// Build a sink that appends to `path` (the loop's main
    /// events ledger). The parent directory is created lazily on
    /// the first `append_events` call.
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The ledger path this sink appends to (diagnostics + tests).
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl EventMergeSink for FileEventMergeSink {
    fn append_events(&self, events: Vec<Event>) -> Result<(), MergeSinkError> {
        // An empty batch is a no-op success: the coordinator calls
        // `append_events` on the Integrate path even when a wave
        // produced no business events, and that must still advance
        // `merged_to_events`.
        if events.is_empty() {
            return Ok(());
        }
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|err| MergeSinkError::Rejected(format!("create_dir_all: {err}")))?;
        }
        let ts = chrono::Utc::now().to_rfc3339();
        let mut buf = String::new();
        for ev in &events {
            let mut record = serde_json::json!({
                "topic": ev.topic.as_str(),
                "payload": ev.payload,
                "ts": ts,
            });
            if let Some(ref source) = ev.source {
                let hat = source.as_str();
                record["hat"] = serde_json::json!(hat);
                record["source"] = serde_json::json!(hat);
            }
            if let Some(ref wave_id) = ev.wave_id {
                record["wave_id"] = serde_json::json!(wave_id);
                record["wave_index"] = serde_json::json!(ev.wave_index.unwrap_or(0));
                record["wave_total"] = serde_json::json!(ev.wave_total.unwrap_or(1));
            }
            if ev.system_injected == Some(true) {
                record["system_injected"] = serde_json::json!(true);
            }
            let line = serde_json::to_string(&record)
                .map_err(|err| MergeSinkError::Rejected(format!("serialize event: {err}")))?;
            buf.push_str(&line);
            buf.push('\n');
        }
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|err| MergeSinkError::Rejected(format!("open ledger: {err}")))?;
        file.write_all(buf.as_bytes())
            .map_err(|err| MergeSinkError::Rejected(format!("write ledger: {err}")))?;
        file.flush()
            .map_err(|err| MergeSinkError::Rejected(format!("flush ledger: {err}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod file_sink_tests {
    //! U6: closed-circuit tests for the production
    //! [`FileEventMergeSink`]. They write to a tempdir ledger and
    //! assert the JSONL record shape the `EventReader` parses.
    use super::*;

    #[test]
    fn file_sink_appends_parseable_jsonl_records() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let ledger = tmp.path().join(".ralph").join("events.jsonl");
        let sink = FileEventMergeSink::new(ledger.clone());

        let event = Event::new("exec.unit.done", "{\"unit\":\"u0\"}")
            .with_source("executor")
            .with_wave("w-1", 0, 2);
        sink.append_events(vec![event])
            .expect("append must succeed");

        // The parent dir is materialised lazily on first append.
        assert!(ledger.exists(), "sink must create the ledger file");
        let content = std::fs::read_to_string(&ledger).expect("read ledger");
        let lines: Vec<serde_json::Value> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("ledger line must be JSON"))
            .collect();
        assert_eq!(lines.len(), 1, "one event → one JSONL record");
        assert_eq!(lines[0]["topic"], "exec.unit.done");
        assert_eq!(lines[0]["hat"], "executor");
        assert_eq!(lines[0]["source"], "executor");
        assert_eq!(lines[0]["wave_id"], "w-1");
        assert!(
            !lines[0]["ts"].as_str().unwrap_or("").is_empty(),
            "ts must be stamped"
        );
    }

    #[test]
    fn file_sink_empty_batch_is_noop() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let ledger = tmp.path().join("events.jsonl");
        let sink = FileEventMergeSink::new(ledger.clone());
        sink.append_events(Vec::new())
            .expect("empty batch succeeds");
        assert!(
            !ledger.exists(),
            "empty batch must not create or touch the ledger"
        );
    }

    #[test]
    fn file_sink_unwritable_path_returns_rejected() {
        let tmp = tempfile::tempdir().expect("temp dir");
        // A regular file where a parent directory is required makes
        // `create_dir_all` fail → `MergeSinkError::Rejected`.
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, "not a dir").expect("write blocker file");
        let ledger = blocker.join("nested").join("events.jsonl");
        let sink = FileEventMergeSink::new(ledger);
        let err = sink
            .append_events(vec![Event::new("exec.unit.done", "{}")])
            .expect_err("unwritable path must fail");
        assert!(
            matches!(err, MergeSinkError::Rejected(_)),
            "I/O failure must surface as Rejected; got {err:?}"
        );
    }
}
