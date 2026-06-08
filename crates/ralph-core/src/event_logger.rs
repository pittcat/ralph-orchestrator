//! Event logging for debugging and post-mortem analysis.
//!
//! Logs all events to `.ralph/events.jsonl` as specified in the event-loop spec.
//! The observer pattern allows hooking into the event bus without modifying routing.

use crate::loop_context::LoopContext;
use crate::text::floor_char_boundary;
use ralph_proto::{Event, HatId};
use serde::{Deserialize, Deserializer, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Custom deserializer that accepts both String and structured JSON payloads.
///
/// Agents sometimes write structured data as JSON objects instead of strings.
/// This deserializer accepts both formats:
/// - `"payload": "string"` → `"string"`
/// - `"payload": {...}` → `"{...}"` (serialized to JSON string)
/// - `"payload": null` or missing → `""` (empty string)
fn deserialize_flexible_payload<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum FlexiblePayload {
        String(String),
        Object(serde_json::Value),
    }

    let opt = Option::<FlexiblePayload>::deserialize(deserializer)?;
    Ok(opt
        .map(|flex| match flex {
            FlexiblePayload::String(s) => s,
            FlexiblePayload::Object(serde_json::Value::Null) => String::new(),
            FlexiblePayload::Object(obj) => {
                // Serialize the object back to a JSON string
                serde_json::to_string(&obj).unwrap_or_else(|_| obj.to_string())
            }
        })
        .unwrap_or_default())
}

/// Raw deserialization shape used to normalize off-spec event lines
/// before constructing an `EventRecord`.
///
/// Background: the `ce-executor` wave worker StreamJson was observed to
/// write event lines into the events file via an output-parsing side
/// channel using non-canonical field names and a literal `null` for `ts`:
///   - `{"timestamp":"2024-01-15T10:00:00Z", ...}`  (long-form key)
///   - `{"ts": null, ...}`                          (literal null)
///   - `{"type":"review.dimension.done", ...}`     (`type` instead of `topic`)
///
/// The default `String` deserializer on `ts: String` rejects `null` and
/// `serde_json::from_str::<EventRecord>` returns `Err`, causing the read
/// path (`EventHistory::read_all`, `event_watcher.rs`) to silently drop
/// the line. This struct accepts every off-spec variant and the
/// `From`/`From` conversion normalizes them into the canonical
/// `EventRecord` form.
#[derive(Debug, Deserialize)]
pub struct EventRecordRaw {
    /// Canonical ISO 8601 timestamp (`"ts"`), or `null`.
    #[serde(default)]
    ts: Option<String>,
    /// StreamJson long-form timestamp (`"timestamp"`).
    #[serde(default)]
    timestamp: Option<String>,
    /// Loop iteration number (0 if not provided by agent-written events).
    #[serde(default)]
    iteration: u32,
    /// Hat that was active when event was published.
    #[serde(default)]
    hat: String,
    /// Canonical event topic (`"topic"`).
    #[serde(default)]
    topic: Option<String>,
    /// Off-spec event topic (`"type"`), used as a fallback when `topic` is absent.
    #[serde(rename = "type", default)]
    topic_type: Option<String>,
    /// Hat that will be triggered by this event.
    #[serde(default)]
    triggered: Option<String>,
    /// Event content (truncated if large).
    #[serde(default, deserialize_with = "deserialize_flexible_payload")]
    payload: String,
    /// How many times this task has blocked.
    #[serde(default)]
    blocked_count: Option<u32>,
    /// Wave correlation ID.
    #[serde(default)]
    wave_id: Option<String>,
    /// Index of this event within the wave (0-based).
    #[serde(default)]
    wave_index: Option<u32>,
    /// Total number of events in the wave.
    #[serde(default)]
    wave_total: Option<u32>,
    /// Current orchestration phase (warmup / production).
    #[serde(rename = "_phase", default)]
    phase: Option<String>,
}

impl From<EventRecordRaw> for EventRecord {
    fn from(raw: EventRecordRaw) -> Self {
        // Resolve `ts`:
        //   - canonical `"ts": "..."` -> as-is
        //   - long-form `"timestamp": "..."` -> prefer over null `ts`
        //   - `"ts": null` or missing -> fall back to `"timestamp"`, then `now()`
        let ts = raw
            .ts
            .filter(|s| !s.is_empty())
            .or(raw.timestamp)
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

        // Resolve `topic`:
        //   - canonical `"topic": "..."` -> as-is
        //   - off-spec `"type": "..."` -> fallback when `topic` is missing/null
        //   - both missing -> empty string
        let topic = raw
            .topic
            .or(raw.topic_type)
            .unwrap_or_default();

        Self {
            ts,
            iteration: raw.iteration,
            hat: raw.hat,
            topic,
            triggered: raw.triggered,
            payload: raw.payload,
            blocked_count: raw.blocked_count,
            wave_id: raw.wave_id,
            wave_index: raw.wave_index,
            wave_total: raw.wave_total,
            phase: raw.phase,
        }
    }
}

/// A logged event record for debugging.
///
/// Supports two schemas:
/// 1. Rich internal format (logged by Ralph):
///    `{"ts":"2024-01-15T10:23:45Z","iteration":1,"hat":"loop","topic":"task.start","triggered":"planner","payload":"..."}`
/// 2. Simple agent format (written by agents):
///    `{"topic":"build.task","payload":"...","ts":"2024-01-15T10:24:12Z"}`
///
/// In addition, the deserializer tolerates three off-spec writer shapes
/// observed from the `ce-executor` wave worker StreamJson side channel:
/// - `"timestamp"` instead of `"ts"`
/// - `"ts": null` (falls back to `now()`)
/// - `"type"` instead of `"topic"`
///
/// Fields that don't exist in the agent format default to sensible values.
#[derive(Debug, Clone, Serialize)]
pub struct EventRecord {
    /// ISO 8601 timestamp.
    pub ts: String,

    /// Loop iteration number (0 if not provided by agent-written events).
    pub iteration: u32,

    /// Hat that was active when event was published (empty string if not provided).
    pub hat: String,

    /// Event topic.
    pub topic: String,

    /// Hat that will be triggered by this event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub triggered: Option<String>,

    /// Event content (truncated if large). Defaults to empty string for agent events without payload.
    /// Accepts both string and object payloads - objects are serialized to JSON strings.
    pub payload: String,

    /// How many times this task has blocked (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_count: Option<u32>,

    /// Wave correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wave_id: Option<String>,

    /// Index of this event within the wave (0-based).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wave_index: Option<u32>,

    /// Total number of events in the wave.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wave_total: Option<u32>,

    /// Current orchestration phase (warmup / production).
    #[serde(rename = "_phase", skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
}

impl<'de> Deserialize<'de> for EventRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = EventRecordRaw::deserialize(deserializer)?;
        Ok(EventRecord::from(raw))
    }
}

impl EventRecord {
    /// Maximum payload length before truncation.
    /// Raised from 500 to 50000 to accommodate structured AutoResearch event
    /// payloads (typically 1–5 KB) without breaking JSON validity.
    const MAX_PAYLOAD_LEN: usize = 50000;

    /// Creates a new event record.
    pub fn new(
        iteration: u32,
        hat: impl Into<String>,
        event: &Event,
        triggered: Option<&HatId>,
        phase: Option<String>,
    ) -> Self {
        let payload = if event.payload.len() > Self::MAX_PAYLOAD_LEN {
            // Find a valid UTF-8 char boundary at or before MAX_PAYLOAD_LEN.
            let truncate_at = floor_char_boundary(&event.payload, Self::MAX_PAYLOAD_LEN);
            format!(
                "{}... [truncated, {} chars total]",
                &event.payload[..truncate_at],
                event.payload.chars().count()
            )
        } else {
            event.payload.clone()
        };

        Self {
            ts: chrono::Utc::now().to_rfc3339(),
            iteration,
            hat: hat.into(),
            topic: event.topic.to_string(),
            triggered: triggered.map(|h| h.to_string()),
            payload,
            blocked_count: None,
            wave_id: event.wave_id.clone(),
            wave_index: event.wave_index,
            wave_total: event.wave_total,
            phase,
        }
    }

    /// Sets the blocked count for this record.
    pub fn with_blocked_count(mut self, count: u32) -> Self {
        self.blocked_count = Some(count);
        self
    }
}

/// Logger that writes events to a JSONL file.
pub struct EventLogger {
    /// Path to the events file.
    path: PathBuf,

    /// File handle for appending.
    file: Option<File>,
}

impl EventLogger {
    /// Default path for the events file.
    pub const DEFAULT_PATH: &'static str = ".ralph/events.jsonl";

    /// Creates a new event logger.
    ///
    /// The `.ralph/` directory is created if it doesn't exist.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            file: None,
        }
    }

    /// Creates a logger with the default path.
    pub fn default_path() -> Self {
        Self::new(Self::DEFAULT_PATH)
    }

    /// Creates a logger using the events path from a LoopContext.
    ///
    /// This reads the timestamped events path from the marker file if it exists,
    /// falling back to the default events path. This ensures the logger writes
    /// to the correct location when running in a worktree or other isolated workspace.
    ///
    /// **WARNING**: This writes to the **trusted events file** consumed by
    /// `EventReader`. Only call this for paths that must be read by the
    /// orchestration loop. For raw output logging, orphan diagnostics, and
    /// termination events, use [`history_from_context`] instead.
    pub fn from_context(context: &LoopContext) -> Self {
        // Read timestamped events path from marker file, fall back to default
        // The marker file contains a relative path like ".ralph/events-20260127-123456.jsonl"
        // which we resolve relative to the workspace root
        let events_path = std::fs::read_to_string(context.current_events_marker())
            .map(|s| {
                let relative = s.trim();
                context.workspace().join(relative)
            })
            .unwrap_or_else(|_| context.events_path());
        Self::new(events_path)
    }

    /// Creates a logger for the **history/observability** file, separate from
    /// the trusted events file consumed by `EventReader`.
    ///
    /// This is the correct logger for:
    /// - Raw output parsing (via `log_events_from_output`)
    /// - Orphan event diagnostics (`event.orphaned`)
    /// - Accepted event history (`log_accepted_events`)
    /// - Termination events (`log_terminate_event`)
    ///
    /// The history file is derived from the `current-events` marker by
    /// inserting `-history` before the `.jsonl` extension. For example,
    /// if the marker points to `.ralph/events-20260127-123456.jsonl`, the
    /// history file will be `.ralph/events-history-20260127-123456.jsonl`.
    ///
    /// This ensures that fake/demo events embedded in agent text output
    /// cannot leak into the trusted event stream consumed by the orchestrator.
    pub fn history_from_context(context: &LoopContext) -> Self {
        let history_path = std::fs::read_to_string(context.current_events_marker())
            .map(|s| {
                let relative = s.trim().to_string();
                // Derive history path: ".ralph/events-{id}.jsonl" → ".ralph/events-history-{id}.jsonl"
                let history_relative = if let Some(stripped) = relative.strip_suffix(".jsonl") {
                    // Insert "-history" after "events" prefix: "events-{id}" → "events-history-{id}"
                    if let Some(pos) = stripped.rfind("events") {
                        let (before, after) = stripped.split_at(pos + 6); // "events".len() == 6
                        format!("{}-history{}.jsonl", before, after)
                    } else {
                        format!("{}.history.jsonl", stripped)
                    }
                } else {
                    format!("{}.history", relative)
                };
                context.workspace().join(history_relative)
            })
            .unwrap_or_else(|_| {
                // Fallback: use a default history file name
                context.ralph_dir().join("events-history.jsonl")
            });
        Self::new(history_path)
    }

    /// Ensures the parent directory exists and opens the file.
    fn ensure_open(&mut self) -> std::io::Result<&mut File> {
        if self.file.is_none() {
            if let Some(parent) = self.path.parent() {
                fs::create_dir_all(parent)?;
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            self.file = Some(file);
        }
        Ok(self.file.as_mut().unwrap())
    }

    /// Logs an event record.
    ///
    /// Uses a single `write_all` call to ensure the JSON line is written atomically.
    /// This prevents corruption when multiple processes append to the same file
    /// concurrently (e.g., during parallel merge queue processing).
    pub fn log(&mut self, record: &EventRecord) -> std::io::Result<()> {
        let file = self.ensure_open()?;
        let mut json = serde_json::to_string(record)?;
        json.push('\n');
        // Single write_all ensures atomic append on POSIX with O_APPEND
        file.write_all(json.as_bytes())?;
        file.flush()?;
        debug!(topic = %record.topic, iteration = record.iteration, "Event logged");
        Ok(())
    }

    /// Convenience method to log an event directly.
    pub fn log_event(
        &mut self,
        iteration: u32,
        hat: &str,
        event: &Event,
        triggered: Option<&HatId>,
        phase: Option<&str>,
    ) -> std::io::Result<()> {
        let record = EventRecord::new(iteration, hat, event, triggered, phase.map(String::from));
        self.log(&record)
    }

    /// Returns the path to the log file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Reader for event history files.
pub struct EventHistory {
    path: PathBuf,
}

impl EventHistory {
    /// Creates a new history reader.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Returns the path to the history file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Creates a reader for the default path.
    pub fn default_path() -> Self {
        Self::new(EventLogger::DEFAULT_PATH)
    }

    /// Creates a history reader using the events path from a LoopContext.
    ///
    /// This ensures the reader looks in the correct location when running
    /// in a worktree or other isolated workspace. Respects the
    /// `current-events` marker if present.
    pub fn from_context(context: &LoopContext) -> Self {
        Self::new(context.resolve_events_path())
    }

    /// Returns true if the history file exists.
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Reads all event records from the file.
    pub fn read_all(&self) -> std::io::Result<Vec<EventRecord>> {
        if !self.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&self.path)?;
        let reader = BufReader::new(file);
        let mut records = Vec::new();

        for (line_num, line) in reader.lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str(&line) {
                Ok(record) => records.push(record),
                Err(e) => {
                    warn!(line = line_num + 1, error = %e, "Failed to parse event record");
                }
            }
        }

        Ok(records)
    }

    /// Reads the last N event records.
    pub fn read_last(&self, n: usize) -> std::io::Result<Vec<EventRecord>> {
        let all = self.read_all()?;
        let start = all.len().saturating_sub(n);
        Ok(all[start..].to_vec())
    }

    /// Reads events filtered by topic.
    pub fn filter_by_topic(&self, topic: &str) -> std::io::Result<Vec<EventRecord>> {
        let all = self.read_all()?;
        Ok(all.into_iter().filter(|r| r.topic == topic).collect())
    }

    /// Reads events filtered by iteration.
    pub fn filter_by_iteration(&self, iteration: u32) -> std::io::Result<Vec<EventRecord>> {
        let all = self.read_all()?;
        Ok(all
            .into_iter()
            .filter(|r| r.iteration == iteration)
            .collect())
    }

    /// Clears the event history file.
    pub fn clear(&self) -> std::io::Result<()> {
        if self.exists() {
            fs::remove_file(&self.path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_event(topic: &str, payload: &str) -> Event {
        Event::new(topic, payload)
    }

    #[test]
    fn test_log_and_read() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("events.jsonl");

        let mut logger = EventLogger::new(&path);

        // Log some events
        let event1 = make_event("task.start", "Starting task");
        let event2 = make_event("build.done", "Build complete");

        logger
            .log_event(1, "loop", &event1, Some(&HatId::new("planner")), None)
            .unwrap();
        logger
            .log_event(2, "builder", &event2, Some(&HatId::new("planner")), None)
            .unwrap();

        // Read them back
        let history = EventHistory::new(&path);
        let records = history.read_all().unwrap();

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].topic, "task.start");
        assert_eq!(records[0].iteration, 1);
        assert_eq!(records[0].hat, "loop");
        assert_eq!(records[0].triggered, Some("planner".to_string()));
        assert_eq!(records[1].topic, "build.done");
    }

    #[test]
    fn test_read_last() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("events.jsonl");

        let mut logger = EventLogger::new(&path);

        for i in 1..=10 {
            let event = make_event("test", &format!("Event {}", i));
            logger.log_event(i, "hat", &event, None, None).unwrap();
        }

        let history = EventHistory::new(&path);
        let last_3 = history.read_last(3).unwrap();

        assert_eq!(last_3.len(), 3);
        assert_eq!(last_3[0].iteration, 8);
        assert_eq!(last_3[2].iteration, 10);
    }

    #[test]
    fn test_filter_by_topic() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("events.jsonl");

        let mut logger = EventLogger::new(&path);

        logger
            .log_event(1, "hat", &make_event("build.done", "a"), None, None)
            .unwrap();
        logger
            .log_event(2, "hat", &make_event("build.blocked", "b"), None, None)
            .unwrap();
        logger
            .log_event(3, "hat", &make_event("build.done", "c"), None, None)
            .unwrap();

        let history = EventHistory::new(&path);
        let blocked = history.filter_by_topic("build.blocked").unwrap();

        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].iteration, 2);
    }

    #[test]
    fn test_payload_truncation() {
        let long_payload = "x".repeat(EventRecord::MAX_PAYLOAD_LEN + 1);
        let event = make_event("test", &long_payload);
        let record = EventRecord::new(1, "hat", &event, None, None);

        // Verify truncation actually happened (payload changed)
        assert_ne!(record.payload, long_payload);
        assert!(record.payload.contains("[truncated"));
    }

    #[test]
    fn test_payload_truncation_with_multibyte_chars() {
        // Create a payload with multi-byte UTF-8 characters (✅ is 3 bytes)
        // Place emoji near the truncation boundary to trigger the bug
        let mut payload = "x".repeat(EventRecord::MAX_PAYLOAD_LEN - 2);
        payload.push_str("✅✅✅"); // 3 emojis near the truncation boundary
        payload.push_str(&"y".repeat(10));

        let event = make_event("test", &payload);
        // This should NOT panic
        let record = EventRecord::new(1, "hat", &event, None, None);

        assert!(record.payload.contains("[truncated"));
        // Verify the payload is valid UTF-8 (would panic on iteration if not)
        for _ in record.payload.chars() {}
    }

    #[test]
    fn test_creates_parent_directory() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nested/dir/events.jsonl");

        let mut logger = EventLogger::new(&path);
        let event = make_event("test", "payload");
        logger.log_event(1, "hat", &event, None, None).unwrap();

        assert!(path.exists());
    }

    #[test]
    fn test_empty_history() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.jsonl");

        let history = EventHistory::new(&path);
        assert!(!history.exists());

        let records = history.read_all().unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn test_agent_written_events_without_iteration() {
        // Agent events use simple format: {"topic":"...","payload":"...","ts":"..."}
        // They don't include iteration, hat, or triggered fields
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("events.jsonl");

        // Write agent-style events (without iteration field)
        let mut file = File::create(&path).unwrap();
        writeln!(
            file,
            r#"{{"topic":"build.task","payload":"Implement auth","ts":"2024-01-15T10:00:00Z"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"topic":"build.done","ts":"2024-01-15T10:30:00Z"}}"#
        )
        .unwrap();

        // Should read without warnings (iteration defaults to 0)
        let history = EventHistory::new(&path);
        let records = history.read_all().unwrap();

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].topic, "build.task");
        assert_eq!(records[0].payload, "Implement auth");
        assert_eq!(records[0].iteration, 0); // Defaults to 0
        assert_eq!(records[0].hat, ""); // Defaults to empty string
        assert_eq!(records[1].topic, "build.done");
        assert_eq!(records[1].payload, ""); // Defaults to empty when not provided
    }

    #[test]
    fn test_mixed_event_formats() {
        // Test that both agent-written and Ralph-logged events can coexist
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("events.jsonl");

        // Write a Ralph-logged event (full format)
        let mut logger = EventLogger::new(&path);
        let event = make_event("task.start", "Initial task");
        logger
            .log_event(1, "loop", &event, Some(&HatId::new("planner")), None)
            .unwrap();

        // Write an agent-style event (simple format)
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(
            file,
            r#"{{"topic":"build.task","payload":"Agent wrote this","ts":"2024-01-15T10:05:00Z"}}"#
        )
        .unwrap();

        // Should read both without warnings
        let history = EventHistory::new(&path);
        let records = history.read_all().unwrap();

        assert_eq!(records.len(), 2);
        // First is Ralph's full-format event
        assert_eq!(records[0].topic, "task.start");
        assert_eq!(records[0].iteration, 1);
        assert_eq!(records[0].hat, "loop");
        // Second is agent's simple format
        assert_eq!(records[1].topic, "build.task");
        assert_eq!(records[1].iteration, 0); // Defaulted
        assert_eq!(records[1].hat, ""); // Defaulted
    }

    #[test]
    fn test_event_record_propagates_wave_metadata() {
        let event = make_event("review.file", "src/main.rs").with_wave("w-1a2b3c4d", 1, 3);
        let record = EventRecord::new(1, "dispatcher", &event, None, None);

        assert_eq!(record.wave_id.as_deref(), Some("w-1a2b3c4d"));
        assert_eq!(record.wave_index, Some(1));
        assert_eq!(record.wave_total, Some(3));
    }

    #[test]
    fn test_event_record_no_wave_metadata() {
        let event = make_event("build.done", "success");
        let record = EventRecord::new(1, "builder", &event, None, None);

        assert!(record.wave_id.is_none());
        assert!(record.wave_index.is_none());
        assert!(record.wave_total.is_none());
    }

    #[test]
    fn test_event_record_wave_roundtrip_through_jsonl() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("events.jsonl");

        let mut logger = EventLogger::new(&path);

        // Log event with wave metadata
        let event = make_event("review.file", "src/main.rs").with_wave("w-deadbeef", 0, 5);
        logger
            .log_event(1, "dispatcher", &event, None, None)
            .unwrap();

        // Log event without wave metadata
        let plain_event = make_event("build.done", "ok");
        logger
            .log_event(2, "builder", &plain_event, None, None)
            .unwrap();

        let history = EventHistory::new(&path);
        let records = history.read_all().unwrap();

        assert_eq!(records.len(), 2);
        // First has wave metadata
        assert_eq!(records[0].wave_id.as_deref(), Some("w-deadbeef"));
        assert_eq!(records[0].wave_index, Some(0));
        assert_eq!(records[0].wave_total, Some(5));
        // Second has no wave metadata
        assert!(records[1].wave_id.is_none());
        assert!(records[1].wave_index.is_none());
        assert!(records[1].wave_total.is_none());
    }

    #[test]
    fn test_event_record_wave_fields_not_serialized_when_none() {
        let event = make_event("test", "payload");
        let record = EventRecord::new(1, "hat", &event, None, None);
        let json = serde_json::to_string(&record).unwrap();
        assert!(!json.contains("wave_id"));
        assert!(!json.contains("wave_index"));
        assert!(!json.contains("wave_total"));
    }

    #[test]
    fn test_event_record_backwards_compat_no_wave_fields() {
        // Simulate reading a JSONL line written before wave support
        let json = r#"{"ts":"2024-01-15T10:00:00Z","iteration":1,"hat":"builder","topic":"build.done","payload":"ok"}"#;
        let record: EventRecord = serde_json::from_str(json).unwrap();
        assert!(record.wave_id.is_none());
        assert!(record.wave_index.is_none());
        assert!(record.wave_total.is_none());
        assert_eq!(record.topic, "build.done");
    }

    #[test]
    fn test_object_payload_from_ralph_emit_json() {
        // Test that `ralph emit --json` object payloads are parsed correctly
        // This was the root cause of "invalid type: map, expected a string" errors
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("events.jsonl");

        let mut file = File::create(&path).unwrap();

        // String payload (normal case)
        writeln!(
            file,
            r#"{{"ts":"2024-01-15T10:00:00Z","topic":"task.start","payload":"implement feature"}}"#
        )
        .unwrap();

        // Object payload (from `ralph emit --json`)
        writeln!(
            file,
            r#"{{"topic":"task.complete","payload":{{"status":"verified","tasks":["auth","api"]}},"ts":"2024-01-15T10:30:00Z"}}"#
        )
        .unwrap();

        // Nested object payload
        writeln!(
            file,
            r#"{{"topic":"loop.recovery","payload":{{"status":"recovered","evidence":{{"tests":"pass"}}}},"ts":"2024-01-15T10:45:00Z"}}"#
        )
        .unwrap();

        let history = EventHistory::new(&path);
        let records = history.read_all().unwrap();

        assert_eq!(records.len(), 3);

        // String payload unchanged
        assert_eq!(records[0].topic, "task.start");
        assert_eq!(records[0].payload, "implement feature");

        // Object payload converted to JSON string
        assert_eq!(records[1].topic, "task.complete");
        assert!(records[1].payload.contains("\"status\""));
        assert!(records[1].payload.contains("\"verified\""));
        // Should be valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&records[1].payload).unwrap();
        assert_eq!(parsed["status"], "verified");

        // Nested object also works
        assert_eq!(records[2].topic, "loop.recovery");
        let parsed: serde_json::Value = serde_json::from_str(&records[2].payload).unwrap();
        assert_eq!(parsed["evidence"]["tests"], "pass");
    }

    #[test]
    fn test_event_record_phase_roundtrip_through_jsonl() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("events.jsonl");

        let mut logger = EventLogger::new(&path);

        // Log event with phase metadata
        let event = make_event("experiment.start", "run test");
        logger
            .log_event(1, "loop", &event, None, Some("warmup"))
            .unwrap();

        // Log event without phase metadata
        let plain_event = make_event("build.done", "ok");
        logger
            .log_event(2, "builder", &plain_event, None, None)
            .unwrap();

        let history = EventHistory::new(&path);
        let records = history.read_all().unwrap();

        assert_eq!(records.len(), 2);
        // First has phase metadata
        assert_eq!(records[0].phase.as_deref(), Some("warmup"));
        // Second has no phase metadata
        assert!(records[1].phase.is_none());
    }

    #[test]
    fn test_event_record_phase_field_not_serialized_when_none() {
        let event = make_event("test", "payload");
        let record = EventRecord::new(1, "hat", &event, None, None);
        let json = serde_json::to_string(&record).unwrap();
        assert!(!json.contains("_phase"));
    }

    #[test]
    fn test_event_record_phase_serialized_when_some() {
        let event = make_event("test", "payload");
        let record = EventRecord::new(1, "hat", &event, None, Some("production".to_string()));
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"_phase\":\"production\""));
    }

    #[test]
    fn test_event_record_backwards_compat_no_phase_field() {
        // Simulate reading a JSONL line written before phase support
        let json = r#"{"ts":"2024-01-15T10:00:00Z","iteration":1,"hat":"builder","topic":"build.done","payload":"ok"}"#;
        let record: EventRecord = serde_json::from_str(json).unwrap();
        assert!(record.phase.is_none());
        assert_eq!(record.topic, "build.done");
    }

    #[test]
    fn test_history_from_context_resolves_separate_path() {
        let tmp = TempDir::new().unwrap();
        let ctx = crate::LoopContext::primary(tmp.path().to_path_buf());

        // Write a marker file pointing to the trusted events file
        let marker = ctx.current_events_marker();
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, ".ralph/events-20260602-120000.jsonl").unwrap();

        let logger = EventLogger::history_from_context(&ctx);

        let path_str = logger.path().to_string_lossy();
        // Should contain "events-history" to distinguish from trusted file
        assert!(
            path_str.contains("events-history"),
            "history logger path should contain 'events-history', got: {}",
            path_str
        );
        // Should contain the full expected filename
        assert!(
            path_str.contains("events-history-20260602-120000.jsonl"),
            "history logger path should contain 'events-history-20260602-120000.jsonl', got: {}",
            path_str
        );
    }

    #[test]
    fn test_history_from_context_fallback_when_no_marker() {
        let tmp = TempDir::new().unwrap();
        let ctx = crate::LoopContext::primary(tmp.path().to_path_buf());
        // No marker file written

        let logger = EventLogger::history_from_context(&ctx);

        // Should fall back to ralph_dir()/events-history.jsonl
        assert!(
            logger
                .path()
                .to_string_lossy()
                .contains("events-history.jsonl")
        );
    }

    #[test]
    fn test_history_from_context_writes_to_separate_file() {
        let tmp = TempDir::new().unwrap();
        let ctx = crate::LoopContext::primary(tmp.path().to_path_buf());

        let marker = ctx.current_events_marker();
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, ".ralph/events-20260602-120000.jsonl").unwrap();

        // Write via history logger
        let mut history_logger = EventLogger::history_from_context(&ctx);
        let event = make_event("test.history", "from output parsing");
        history_logger
            .log_event(1, "loop", &event, None, None)
            .unwrap();

        // History file should exist at the logger's path
        assert!(
            history_logger.path().exists(),
            "history file should exist at {:?}",
            history_logger.path()
        );
        // Verify the path matches expectations
        let history_path = history_logger.path().to_path_buf();

        // Trusted events file should NOT exist
        let trusted_path = tmp.path().join(".ralph/events-20260602-120000.jsonl");
        assert!(
            !trusted_path.exists(),
            "history logger should not write to the trusted events file"
        );

        // Verify content is readable
        let history = EventHistory::new(&history_path);
        let records = history.read_all().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].topic, "test.history");
    }

    #[test]
    fn test_ts_deserializer_accepts_long_form_timestamp_key() {
        // StreamJson workers sometimes emit `"timestamp"` instead of `"ts"`.
        // The deserializer should read `"timestamp"` and store it in `ts`.
        let json = r#"{"timestamp":"2024-01-15T10:00:00Z","iteration":1,"hat":"dimension-reviewer","topic":"review.dimension.done","payload":"ok"}"#;
        let record: EventRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.ts, "2024-01-15T10:00:00Z");
        assert_eq!(record.topic, "review.dimension.done");
    }

    #[test]
    fn test_ts_deserializer_falls_back_to_now_when_null() {
        // Writer path observed: `"ts": null`. The deserializer should fall
        // back to a non-empty ISO 8601 timestamp rather than failing the
        // read (which would silently drop the event line in the reader).
        let json = r#"{"ts":null,"iteration":1,"hat":"dimension-reviewer","topic":"review.dimension.done","payload":"ok"}"#;
        let record: EventRecord = serde_json::from_str(json).unwrap();
        assert!(!record.ts.is_empty(), "ts should fall back to a non-empty value");
        // Must be a valid RFC3339 timestamp.
        assert!(
            chrono::DateTime::parse_from_rfc3339(&record.ts).is_ok(),
            "ts should be a valid RFC3339 string, got: {}",
            record.ts
        );
    }

    #[test]
    fn test_ts_deserializer_falls_back_to_now_when_missing() {
        // Some writers omit `ts` entirely. The deserializer should fall back
        // to a current-time timestamp instead of failing.
        let json = r#"{"iteration":1,"hat":"dimension-reviewer","topic":"review.dimension.done","payload":"ok"}"#;
        let record: EventRecord = serde_json::from_str(json).unwrap();
        assert!(!record.ts.is_empty(), "ts should fall back to a non-empty value");
        assert!(chrono::DateTime::parse_from_rfc3339(&record.ts).is_ok());
    }

    #[test]
    fn test_topic_deserializer_falls_back_to_type_key() {
        // Writer path observed: `{"type":"review.dimension.done", ...}`.
        // The deserializer should pull the topic from `type` when `topic` is
        // missing.
        let json = r#"{"type":"review.dimension.done","hat":"dimension-reviewer","payload":"ok","ts":"2024-01-15T10:00:00Z"}"#;
        let record: EventRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.topic, "review.dimension.done");
        assert_eq!(record.ts, "2024-01-15T10:00:00Z");
    }

    #[test]
    fn test_wave_worker_off_spec_event_roundtrips() {
        // Combined regression: a single off-spec event from the wave worker
        // StreamJson path (timestamp + topic, no canonical keys) should be
        // read into a usable EventRecord with both `ts` and `topic` populated.
        let json = r#"{"timestamp":"2024-01-15T10:00:00Z","hat":"dimension-reviewer","payload":"ok"}"#;
        let record: EventRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.ts, "2024-01-15T10:00:00Z");
        // topic missing and no `type` key, so topic falls back to empty.
        assert_eq!(record.topic, "");
        assert_eq!(record.hat, "dimension-reviewer");
    }

    #[test]
    fn test_history_and_trusted_loggers_are_independent() {
        let tmp = TempDir::new().unwrap();
        let ctx = crate::LoopContext::primary(tmp.path().to_path_buf());

        let marker = ctx.current_events_marker();
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, ".ralph/events-20260602-120000.jsonl").unwrap();

        // Write to trusted file
        let mut trusted_logger = EventLogger::from_context(&ctx);
        let trusted_event = make_event("ralph.emit.work.done", "real emit");
        trusted_logger
            .log_event(1, "loop", &trusted_event, None, None)
            .unwrap();

        // Write to history file
        let mut history_logger = EventLogger::history_from_context(&ctx);
        let history_event = make_event("test.parsed", "from output text");
        history_logger
            .log_event(1, "loop", &history_event, None, None)
            .unwrap();

        // Both files should exist and be separate
        let trusted_path = trusted_logger.path().to_path_buf();
        let history_path = history_logger.path().to_path_buf();
        assert!(
            trusted_path.exists(),
            "trusted file should exist at {:?}",
            trusted_path
        );
        assert!(
            history_path.exists(),
            "history file should exist at {:?}",
            history_path
        );
        assert_ne!(
            trusted_path, history_path,
            "trusted and history paths must be different"
        );

        // Trusted file only has the real emit
        let trusted_history = EventHistory::new(&trusted_path);
        let trusted_records = trusted_history.read_all().unwrap();
        assert_eq!(trusted_records.len(), 1);
        assert_eq!(trusted_records[0].topic, "ralph.emit.work.done");

        // History file only has the parsed output event
        let history_history = EventHistory::new(&history_path);
        let history_records = history_history.read_all().unwrap();
        assert_eq!(history_records.len(), 1);
        assert_eq!(history_records[0].topic, "test.parsed");
    }
}
