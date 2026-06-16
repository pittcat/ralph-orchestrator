//! Event reader for consuming events from `.ralph/events.jsonl`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use tracing::warn;

/// Maximum skew (in seconds) tolerated for an event's `ts` field.
///
/// `read_new_events` rejects events whose `ts` parses as RFC3339 and is more
/// than this many seconds in the future relative to wall clock at read time.
/// 5 minutes absorbs clock skew, container time drift, and PTY spawn latency
/// without being a meaningful attack or fixture-forgery window.
const MAX_FUTURE_TS_SKEW_SECS: i64 = 300;

/// Classifies an event's `ts` field for the read-time window check.
///
/// Returns `Ok(())` when the value should be accepted (empty, well-formed and
/// within the future-skew window, or a non-RFC3339 string that nonetheless
/// does not trigger the strict checks). Returns `Err(reason)` for events
/// that must be reported as `MalformedLine`:
/// - `future_timestamp` — parses as RFC3339 and is more than
///   `MAX_FUTURE_TS_SKEW_SECS` seconds ahead of `now`.
/// - `invalid_timestamp` — non-empty and not parseable as RFC3339.
///
/// Empty strings are accepted unconditionally to preserve existing fixture
/// compatibility (e.g. `crates/ralph-core/tests/fixtures/basic_session.jsonl`
/// lines that omit `ts`).
///
/// The boundary check uses strict greater-than (`>`), so a `ts` exactly at
/// `now + MAX_FUTURE_TS_SKEW_SECS` is accepted. Non-Z timezone offsets
/// (e.g. `+09:00`) are normalized via `parsed.with_timezone(&Utc)` before
/// the comparison, so equivalent instants in different offset notations
/// classify identically.
fn classify_timestamp(ts: &str) -> Result<(), &'static str> {
    if ts.is_empty() {
        return Ok(());
    }
    match DateTime::parse_from_rfc3339(ts) {
        Ok(parsed) => {
            let now = Utc::now();
            let max_future = now + chrono::Duration::seconds(MAX_FUTURE_TS_SKEW_SECS);
            if parsed.with_timezone(&Utc) > max_future {
                Err("future_timestamp")
            } else {
                Ok(())
            }
        }
        Err(_) => Err("invalid_timestamp"),
    }
}

/// Result of parsing events from a JSONL file.
///
/// Contains both successfully parsed events and information about lines
/// that failed to parse. This supports backpressure validation by allowing
/// the caller to respond to malformed events.
#[derive(Debug, Clone, Default)]
pub struct ParseResult {
    /// Successfully parsed events.
    pub events: Vec<Event>,
    /// Lines that failed to parse.
    pub malformed: Vec<MalformedLine>,
}

/// Information about a malformed JSONL line.
///
/// Used for backpressure feedback - when agents write invalid JSONL,
/// this provides details for the `event.malformed` system event.
#[derive(Debug, Clone, Serialize)]
pub struct MalformedLine {
    /// Line number in the file (1-indexed).
    pub line_number: u64,
    /// The raw content that failed to parse (truncated if very long).
    pub content: String,
    /// The parse error message.
    pub error: String,
}

impl MalformedLine {
    /// Maximum content length before truncation.
    const MAX_CONTENT_LEN: usize = 100;

    /// Creates a new MalformedLine, truncating content if needed.
    pub fn new(line_number: u64, content: &str, error: String) -> Self {
        let content = if content.len() > Self::MAX_CONTENT_LEN {
            // Truncate at a valid UTF-8 character boundary to avoid panics
            // on multi-byte content.
            let truncate_at = crate::text::floor_char_boundary(content, Self::MAX_CONTENT_LEN);
            format!("{}...", &content[..truncate_at])
        } else {
            content.to_string()
        };
        Self {
            line_number,
            content,
            error,
        }
    }
}

/// Custom deserializer that accepts both String and structured JSON payloads.
///
/// Agents sometimes write structured data as JSON objects instead of strings.
/// This deserializer accepts both formats:
/// - `"payload": "string"` → `Some("string")`
/// - `"payload": {...}` → `Some("{...}")` (serialized to JSON string)
/// - `"payload": null` → `None`
/// - missing field → `None`
fn deserialize_flexible_payload<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
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
    Ok(opt.map(|flex| match flex {
        FlexiblePayload::String(s) => s,
        FlexiblePayload::Object(obj) => {
            // Serialize the object back to a JSON string
            serde_json::to_string(&obj).unwrap_or_else(|_| obj.to_string())
        }
    }))
}

/// A simplified event for reading from JSONL.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub topic: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_flexible_payload"
    )]
    pub payload: Option<String>,
    #[serde(default, alias = "timestamp")]
    pub ts: String,

    /// Hat that published this event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hat: Option<String>,

    /// Target hat triggered by this event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triggered: Option<String>,

    /// Source identifier for this event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Wave correlation ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wave_id: Option<String>,

    /// Index of this event within the wave (0-based).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wave_index: Option<u32>,

    /// Total number of events in the wave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wave_total: Option<u32>,
}

impl Event {
    /// Returns true if this event has wave correlation metadata.
    pub fn is_wave_event(&self) -> bool {
        self.wave_id.is_some()
    }
}

impl From<Event> for ralph_proto::Event {
    fn from(e: Event) -> Self {
        // ts is a JSONL serialization concern, not carried to bus events.
        let mut pe = ralph_proto::Event::new(e.topic.as_str(), e.payload.unwrap_or_default());
        if let Some(hat) = e.hat {
            pe = pe.with_source(hat);
        }
        if let Some(triggered) = e.triggered {
            pe = pe.with_target(triggered);
        }
        if let Some(wave_id) = e.wave_id {
            // wave_index is required when wave_id is present; default to 0
            // only as a last resort (should not happen with well-formed events).
            let index = e.wave_index.unwrap_or(0);
            let total = e.wave_total.unwrap_or(1);
            pe = pe.with_wave(wave_id, index, total);
        }
        pe
    }
}

/// Reads new events from `.ralph/events.jsonl` since last read.
pub struct EventReader {
    path: PathBuf,
    position: u64,
}

impl EventReader {
    /// Creates a new event reader for the given path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            position: 0,
        }
    }

    /// Reads new events since the last read.
    ///
    /// Returns a `ParseResult` containing both successfully parsed events
    /// and information about malformed lines. This enables backpressure
    /// validation - the caller can emit `event.malformed` events and
    /// track consecutive failures.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened or read.
    pub fn read_new_events(&mut self) -> std::io::Result<ParseResult> {
        if !self.path.exists() {
            return Ok(ParseResult::default());
        }

        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(self.position))?;

        let reader = BufReader::new(file);
        let mut result = ParseResult::default();
        let mut current_pos = self.position;
        let mut line_number = self.count_lines_before_position();

        for line in reader.lines() {
            let line = line?;
            let line_bytes = line.len() as u64 + 1; // +1 for newline
            line_number += 1;

            if line.trim().is_empty() {
                current_pos += line_bytes;
                continue;
            }

            match serde_json::from_str::<Event>(&line) {
                Ok(event) => match classify_timestamp(&event.ts) {
                    Ok(()) => result.events.push(event),
                    Err(reason) => {
                        warn!(
                            reason = reason,
                            ts = %event.ts,
                            line_number = line_number,
                            "Event timestamp outside allowed window"
                        );
                        result.malformed.push(MalformedLine::new(
                            line_number,
                            &line,
                            format!("{}: {}", reason, event.ts),
                        ));
                    }
                },
                Err(e) => {
                    warn!(error = %e, line_number = line_number, "Malformed JSON line");
                    result
                        .malformed
                        .push(MalformedLine::new(line_number, &line, e.to_string()));
                }
            }

            current_pos += line_bytes;
        }

        self.position = current_pos;
        Ok(result)
    }

    /// Reads new events without advancing the internal file position.
    ///
    /// This is used by callers that need to inspect unread events before
    /// deciding whether to process them.
    pub fn peek_new_events(&self) -> std::io::Result<ParseResult> {
        let mut reader = Self {
            path: self.path.clone(),
            position: self.position,
        };
        reader.read_new_events()
    }

    /// Counts lines before the current position (for line numbering).
    fn count_lines_before_position(&self) -> u64 {
        if self.position == 0 || !self.path.exists() {
            return 0;
        }
        // Read file up to position and count newlines
        if let Ok(file) = File::open(&self.path) {
            let reader = BufReader::new(file);
            let mut count = 0u64;
            let mut bytes_read = 0u64;
            for line in reader.lines() {
                if let Ok(line) = line {
                    bytes_read += line.len() as u64 + 1;
                    if bytes_read > self.position {
                        break;
                    }
                    count += 1;
                } else {
                    break;
                }
            }
            count
        } else {
            0
        }
    }

    /// Returns the path to the events file.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Returns the current file position.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Sets the file position to a specific byte offset.
    ///
    /// Use this to skip past entries written by the EventLogger so they
    /// are not re-read by `process_events_from_jsonl`.
    pub fn set_position(&mut self, position: u64) {
        self.position = position;
    }

    /// Resets the position to the start of the file.
    pub fn reset(&mut self) {
        self.position = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_read_new_events() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"test","payload":"hello","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        writeln!(file, r#"{{"topic":"test2","ts":"2024-01-01T00:00:01Z"}}"#).unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert_eq!(result.events.len(), 2);
        assert_eq!(result.events[0].topic, "test");
        assert_eq!(result.events[0].payload, Some("hello".to_string()));
        assert_eq!(result.events[1].topic, "test2");
        assert_eq!(result.events[1].payload, None);
        assert!(result.malformed.is_empty());
    }

    #[test]
    fn test_tracks_position() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"{{"topic":"first","ts":"2024-01-01T00:00:00Z"}}"#).unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();
        assert_eq!(result.events.len(), 1);

        // Add more events
        writeln!(file, r#"{{"topic":"second","ts":"2024-01-01T00:00:01Z"}}"#).unwrap();
        file.flush().unwrap();

        // Should only read new events
        let result = reader.read_new_events().unwrap();
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.events[0].topic, "second");
    }

    #[test]
    fn test_peek_new_events_does_not_advance_position() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"{{"topic":"first","ts":"2024-01-01T00:00:00Z"}}"#).unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let peeked = reader.peek_new_events().unwrap();
        assert_eq!(peeked.events.len(), 1);
        assert_eq!(peeked.events[0].topic, "first");

        // Position should remain unchanged after peek.
        assert_eq!(reader.position(), 0);

        let consumed = reader.read_new_events().unwrap();
        assert_eq!(consumed.events.len(), 1);
        assert_eq!(consumed.events[0].topic, "first");
    }

    #[test]
    fn test_missing_file() {
        let mut reader = EventReader::new("/nonexistent/path.jsonl");
        let result = reader.read_new_events().unwrap();
        assert!(result.events.is_empty());
        assert!(result.malformed.is_empty());
    }

    #[test]
    fn test_captures_malformed_lines() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"{{"topic":"good","ts":"2024-01-01T00:00:00Z"}}"#).unwrap();
        writeln!(file, r"{{corrupt json}}").unwrap();
        writeln!(
            file,
            r#"{{"topic":"also_good","ts":"2024-01-01T00:00:01Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        // Good events should be parsed
        assert_eq!(result.events.len(), 2);
        assert_eq!(result.events[0].topic, "good");
        assert_eq!(result.events[1].topic, "also_good");

        // Malformed line should be captured
        assert_eq!(result.malformed.len(), 1);
        assert_eq!(result.malformed[0].line_number, 2);
        assert!(result.malformed[0].content.contains("corrupt json"));
        assert!(!result.malformed[0].error.is_empty());
    }

    #[test]
    fn test_empty_file() {
        let file = NamedTempFile::new().unwrap();
        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();
        assert!(result.events.is_empty());
        assert!(result.malformed.is_empty());
    }

    #[test]
    fn test_reset_position() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"{{"topic":"test","ts":"2024-01-01T00:00:00Z"}}"#).unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        reader.read_new_events().unwrap();
        assert!(reader.position() > 0);

        reader.reset();
        assert_eq!(reader.position(), 0);

        let result = reader.read_new_events().unwrap();
        assert_eq!(result.events.len(), 1);
    }

    #[test]
    fn test_structured_payload_as_object() {
        // Test that JSON objects in payload field are converted to strings
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"review.done","payload":{{"status":"approved","files":["a.rs","b.rs"]}},"ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert_eq!(result.events.len(), 1);
        assert_eq!(result.events[0].topic, "review.done");

        // Payload should be stringified JSON
        let payload = result.events[0].payload.as_ref().unwrap();
        assert!(payload.contains("\"status\""));
        assert!(payload.contains("\"approved\""));
        assert!(payload.contains("\"files\""));

        // Verify it can be parsed back as JSON
        let parsed: serde_json::Value = serde_json::from_str(payload).unwrap();
        assert_eq!(parsed["status"], "approved");
    }

    #[test]
    fn test_mixed_payload_formats() {
        // Test mixing string and object payloads in same file
        let mut file = NamedTempFile::new().unwrap();

        // String payload
        writeln!(
            file,
            r#"{{"topic":"task.start","payload":"Start work","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();

        // Object payload
        writeln!(
            file,
            r#"{{"topic":"task.done","payload":{{"result":"success"}},"ts":"2024-01-01T00:00:01Z"}}"#
        )
        .unwrap();

        // No payload
        writeln!(
            file,
            r#"{{"topic":"heartbeat","ts":"2024-01-01T00:00:02Z"}}"#
        )
        .unwrap();

        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert_eq!(result.events.len(), 3);

        // First event: string payload
        assert_eq!(result.events[0].payload, Some("Start work".to_string()));

        // Second event: object payload converted to string
        let payload2 = result.events[1].payload.as_ref().unwrap();
        assert!(payload2.contains("\"result\""));

        // Third event: no payload
        assert_eq!(result.events[2].payload, None);
    }

    #[test]
    fn test_nested_object_payload() {
        // Test deeply nested objects are handled correctly
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"analysis","payload":{{"issues":[{{"file":"test.rs","line":42,"severity":"major"}}],"approval":"conditional"}},"ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert_eq!(result.events.len(), 1);

        // Should serialize nested structure
        let payload = result.events[0].payload.as_ref().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(payload).unwrap();
        assert_eq!(parsed["issues"][0]["file"], "test.rs");
        assert_eq!(parsed["issues"][0]["line"], 42);
        assert_eq!(parsed["approval"], "conditional");
    }

    #[test]
    fn test_event_reader_parses_wave_metadata() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"review.file","payload":"src/main.rs","ts":"2024-01-01T00:00:00Z","wave_id":"w-1a2b3c4d","wave_index":0,"wave_total":3}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"topic":"review.file","payload":"src/lib.rs","ts":"2024-01-01T00:00:00Z","wave_id":"w-1a2b3c4d","wave_index":1,"wave_total":3}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert_eq!(result.events.len(), 2);
        assert!(result.events[0].is_wave_event());
        assert_eq!(result.events[0].wave_id.as_deref(), Some("w-1a2b3c4d"));
        assert_eq!(result.events[0].wave_index, Some(0));
        assert_eq!(result.events[0].wave_total, Some(3));
        assert_eq!(result.events[1].wave_index, Some(1));
    }

    #[test]
    fn test_event_reader_backwards_compat_no_wave_fields() {
        // Events written before wave support should still parse
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"build.done","payload":"ok","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert_eq!(result.events.len(), 1);
        assert!(!result.events[0].is_wave_event());
        assert!(result.events[0].wave_id.is_none());
        assert!(result.events[0].wave_index.is_none());
        assert!(result.events[0].wave_total.is_none());
    }

    #[test]
    fn test_event_reader_mixed_wave_and_non_wave() {
        let mut file = NamedTempFile::new().unwrap();
        // Non-wave event
        writeln!(
            file,
            r#"{{"topic":"task.start","payload":"begin","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        // Wave event
        writeln!(
            file,
            r#"{{"topic":"review.file","payload":"src/main.rs","ts":"2024-01-01T00:00:01Z","wave_id":"w-abc","wave_index":0,"wave_total":2}}"#
        )
        .unwrap();
        // Another non-wave event
        writeln!(
            file,
            r#"{{"topic":"build.done","ts":"2024-01-01T00:00:02Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert_eq!(result.events.len(), 3);
        assert!(!result.events[0].is_wave_event());
        assert!(result.events[1].is_wave_event());
        assert_eq!(result.events[1].wave_id.as_deref(), Some("w-abc"));
        assert!(!result.events[2].is_wave_event());
    }

    #[test]
    fn test_from_event_reader_to_proto_without_wave() {
        let event = Event {
            topic: "build.done".to_string(),
            payload: Some("success".to_string()),
            ts: "2024-01-01T00:00:00Z".to_string(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
        };
        let proto: ralph_proto::Event = event.into();
        assert_eq!(proto.topic.as_str(), "build.done");
        assert_eq!(proto.payload, "success");
        assert!(!proto.is_wave_event());
    }

    #[test]
    fn test_from_event_reader_to_proto_with_wave() {
        let event = Event {
            topic: "review.file".to_string(),
            payload: Some("src/main.rs".to_string()),
            ts: "2024-01-01T00:00:00Z".to_string(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: Some("w-abc".to_string()),
            wave_index: Some(2),
            wave_total: Some(5),
        };
        let proto: ralph_proto::Event = event.into();
        assert_eq!(proto.topic.as_str(), "review.file");
        assert_eq!(proto.payload, "src/main.rs");
        assert!(proto.is_wave_event());
        assert_eq!(proto.wave_id.as_deref(), Some("w-abc"));
        assert_eq!(proto.wave_index, Some(2));
        assert_eq!(proto.wave_total, Some(5));
    }

    #[test]
    fn test_from_event_reader_to_proto_none_payload() {
        let event = Event {
            topic: "empty.event".to_string(),
            payload: None,
            ts: "2024-01-01T00:00:00Z".to_string(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
        };
        let proto: ralph_proto::Event = event.into();
        assert_eq!(proto.payload, "");
    }

    #[test]
    fn test_mixed_valid_invalid_handling() {
        // Test that valid events are captured alongside malformed ones
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"{{"topic":"valid1","ts":"2024-01-01T00:00:00Z"}}"#).unwrap();
        writeln!(file, "not valid json at all").unwrap();
        writeln!(file, r#"{{"topic":"valid2","ts":"2024-01-01T00:00:01Z"}}"#).unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert_eq!(result.events.len(), 2);
        assert_eq!(result.malformed.len(), 1);
        assert_eq!(result.events[0].topic, "valid1");
        assert_eq!(result.events[1].topic, "valid2");
    }

    #[test]
    fn test_missing_ts_defaults_to_empty_string() {
        // Wave workers may write events without a ts field directly to the
        // events file. The reader should accept these with a default empty
        // string rather than marking them as malformed.
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"review.dimension.done","payload":"ok"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"topic":"review.dimension.done","payload":"ok","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert_eq!(result.events.len(), 2);
        assert_eq!(result.events[0].topic, "review.dimension.done");
        assert_eq!(result.events[0].ts, "");
        assert_eq!(result.events[1].ts, "2024-01-01T00:00:00Z");
        assert!(result.malformed.is_empty());
    }

    #[test]
    fn test_event_reader_backwards_compat_no_provenance_fields() {
        // Events written before provenance support should still parse
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"build.done","payload":"ok","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert_eq!(result.events.len(), 1);
        assert!(result.events[0].hat.is_none());
        assert!(result.events[0].triggered.is_none());
        assert!(result.events[0].source.is_none());
    }

    #[test]
    fn test_event_reader_parses_provenance_fields() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"experiment.planned","payload":"{{\"task_key\":\"x\"}}","ts":"2024-01-01T00:00:00Z","hat":"strategist","triggered":"implementer","source":"cli"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert_eq!(result.events.len(), 1);
        assert_eq!(result.events[0].hat.as_deref(), Some("strategist"));
        assert_eq!(result.events[0].triggered.as_deref(), Some("implementer"));
        assert_eq!(result.events[0].source.as_deref(), Some("cli"));
    }

    #[test]
    fn test_from_event_reader_to_proto_with_provenance() {
        let event = Event {
            topic: "review.file".to_string(),
            payload: Some("src/main.rs".to_string()),
            ts: "2024-01-01T00:00:00Z".to_string(),
            hat: Some("dispatcher".to_string()),
            triggered: Some("reviewer".to_string()),
            source: Some("cli".to_string()),
            wave_id: None,
            wave_index: None,
            wave_total: None,
        };
        let proto: ralph_proto::Event = event.into();
        assert_eq!(proto.topic.as_str(), "review.file");
        assert_eq!(proto.payload, "src/main.rs");
        assert_eq!(
            proto.source.as_ref().map(|s| s.as_str()),
            Some("dispatcher")
        );
        assert_eq!(proto.target.as_ref().map(|s| s.as_str()), Some("reviewer"));
    }

    // -----------------------------------------------------------------
    // Timestamp window + alias tests (see plan 2026-06-18-002).
    // -----------------------------------------------------------------

    #[test]
    fn test_timestamp_field_alias_is_accepted() {
        // Producers may use "timestamp" instead of "ts"; the alias must
        // surface the value into `event.ts` rather than dropping it.
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"x","timestamp":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert_eq!(result.events.len(), 1);
        assert_eq!(result.events[0].ts, "2024-01-01T00:00:00Z");
        assert!(result.malformed.is_empty());
    }

    #[test]
    fn test_empty_ts_remains_accepted() {
        // R3: an empty/missing `ts` must NOT trigger the future-window or
        // invalid-timestamp paths. Legacy fixtures and wave-worker output
        // omit `ts` entirely.
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"{{"topic":"x"}}"#).unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert_eq!(result.events.len(), 1);
        assert_eq!(result.events[0].ts, "");
        assert!(result.malformed.is_empty());
    }

    #[test]
    fn test_past_timestamp_accepted() {
        // No lower bound on `ts`; stale / past timestamps are not flagged.
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"x","ts":"2020-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert_eq!(result.events.len(), 1);
        assert!(result.malformed.is_empty());
    }

    #[test]
    fn test_within_5min_future_timestamp_accepted() {
        // Boundary: a `ts` 4 minutes in the future is inside the 5-minute
        // skew window and must NOT be flagged.
        let now_plus_4min = Utc::now() + chrono::Duration::seconds(4 * 60);
        let ts = now_plus_4min.to_rfc3339();
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"{{"topic":"x","ts":"{}"}}"#, ts).unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert_eq!(result.events.len(), 1);
        assert!(result.malformed.is_empty());
    }

    #[test]
    fn test_exactly_5min_future_timestamp_accepted() {
        // Boundary lock: a `ts` exactly 5 minutes (300s) in the future
        // must be accepted. classify_timestamp uses strict `>` comparison,
        // so the threshold is exclusive. A future refactor to `>=` would
        // break this test and force the operator to consider the impact.
        let now_plus_5min = Utc::now() + chrono::Duration::seconds(MAX_FUTURE_TS_SKEW_SECS);
        let ts = now_plus_5min.to_rfc3339();
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"{{"topic":"x","ts":"{}"}}"#, ts).unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert_eq!(result.events.len(), 1);
        assert!(result.malformed.is_empty());
    }

    #[test]
    fn test_future_timestamp_is_rejected() {
        // R1: a `ts` more than 5 minutes in the future lands in `malformed`
        // with a `future_timestamp` reason.
        let now_plus_10min = Utc::now() + chrono::Duration::seconds(10 * 60);
        let ts = now_plus_10min.to_rfc3339();
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"{{"topic":"x","ts":"{}"}}"#, ts).unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert!(result.events.is_empty());
        assert_eq!(result.malformed.len(), 1);
        assert_eq!(result.malformed[0].line_number, 1);
        assert!(
            result.malformed[0].error.contains("future_timestamp"),
            "expected future_timestamp reason, got: {}",
            result.malformed[0].error
        );
        assert!(result.malformed[0].error.contains(&ts));
    }

    #[test]
    fn test_position_advances_past_rejected_future_timestamp() {
        // Position-tracking contract: when a line is rejected as
        // malformed, the reader's file position must advance past it so a
        // subsequent call to `read_new_events` does not re-read the same
        // rejected line. A future refactor that moves the `current_pos +=`
        // into the success arm only would silently cause infinite re-reads.
        let now_plus_10min = Utc::now() + chrono::Duration::seconds(10 * 60);
        let future_ts = now_plus_10min.to_rfc3339();
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"{{"topic":"rejected","ts":"{}"}}"#, future_ts).unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let first = reader.read_new_events().unwrap();
        assert_eq!(first.malformed.len(), 1);
        assert!(first.events.is_empty());

        // Second call must observe zero new lines (position already past
        // the rejected line).
        let second = reader.read_new_events().unwrap();
        assert!(second.events.is_empty());
        assert!(second.malformed.is_empty());
        assert!(reader.position() > 0);
    }

    #[test]
    fn test_invalid_timestamp_is_rejected() {
        // R2: a non-RFC3339 `ts` lands in `malformed` with an
        // `invalid_timestamp` reason.
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"{{"topic":"x","ts":"not-a-date"}}"#).unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert!(result.events.is_empty());
        assert_eq!(result.malformed.len(), 1);
        assert_eq!(result.malformed[0].line_number, 1);
        assert!(
            result.malformed[0].error.contains("invalid_timestamp"),
            "expected invalid_timestamp reason, got: {}",
            result.malformed[0].error
        );
    }

    #[test]
    fn test_future_timestamp_via_timestamp_alias_is_malformed() {
        // R1 + R4 combined: the "timestamp" field name must NOT bypass the
        // window check. This is the regression test for the exact bug
        // observed in the worktree's events.jsonl (line 19).
        let now_plus_10min = Utc::now() + chrono::Duration::seconds(10 * 60);
        let ts = now_plus_10min.to_rfc3339();
        let mut file = NamedTempFile::new().unwrap();
        // Mirror the worktree line-19 shape: `timestamp` field name with a
        // forged-future RFC3339 value and a complete payload.
        writeln!(
            file,
            r#"{{"topic":"review.dimension.done","payload":{{"dimension":"agent-native","findings_count":0,"status":"ok"}},"hat":"dimension-reviewer","timestamp":"{}","source":"dimension-reviewer","wave_id":"w-1","wave_index":0,"wave_total":7}}"#,
            ts
        )
        .unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert!(
            result.events.is_empty(),
            "future-timestamp event must not reach events, got: {:?}",
            result.events
        );
        assert_eq!(result.malformed.len(), 1);
        assert_eq!(result.malformed[0].line_number, 1);
        assert!(
            result.malformed[0].error.contains("future_timestamp"),
            "expected future_timestamp reason, got: {}",
            result.malformed[0].error
        );
    }

    #[test]
    fn test_mixed_future_and_valid_events() {
        // A forged-future event in a stream of valid events must be flagged
        // without disturbing the surrounding lines.
        let now_plus_10min = Utc::now() + chrono::Duration::seconds(10 * 60);
        let future_ts = now_plus_10min.to_rfc3339();
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"before","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"topic":"forged","ts":"{}"}}"#,
            future_ts
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"topic":"after","ts":"2024-01-01T00:00:02Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();

        assert_eq!(result.events.len(), 2);
        assert_eq!(result.events[0].topic, "before");
        assert_eq!(result.events[1].topic, "after");
        assert_eq!(result.malformed.len(), 1);
        assert_eq!(result.malformed[0].line_number, 2);
        assert!(result.malformed[0].error.contains("future_timestamp"));
    }
}
