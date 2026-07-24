//! Wave context resolution for the `review-synthesizer` aggregate hat.
//!
//! 2026-06-14-003 plan R1: the `review-synthesizer` is an aggregate hat that
//! is activated when the review wave has drained.  Before this module, the
//! agent had to scan `events.jsonl` itself to compute `received_count` and
//! `missing_dimensions` — the agent was free to miscount, which led to the
//! `review-synthesizer` not emitting `review.passed` at all (the
//! `calm-oak` worktree incident).
//!
//! This module centralises the resolution.  The caller (event loop) calls
//! [`resolve_wave_context_for_synthesizer`] with the events file path and
//! gets a [`WaveContext`] back; the loop injects it as a `## WAVE CONTEXT`
//! block in the prompt and as `RALPH_WAVE_CONTEXT` in the env.  When no
//! relevant wave events are present (non-wave presets, idle loop), the
//! function returns `None` and the caller falls back to the existing
//! behaviour — the mechanism is opt-in and never blocks the loop.

use ralph_proto::Event;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Snapshot of the most relevant review wave for the current synthesizer
/// activation.  Mirrors the fields the plan R1.1 enumerates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaveContext {
    /// Stable wave id (`review.wave.ready` / `review.dimension.done` share
    /// this).  Empty string is not a valid value — every code path that
    /// constructs a `WaveContext` must have observed a `review.wave.ready`.
    pub wave_id: String,
    /// Number of dimensions the wave fan-out expected (from
    /// `review.wave.ready.wave_total`).
    pub wave_total: u32,
    /// Number of `review.dimension.done` events observed for this wave.
    pub received_count: u32,
    /// Dimensions listed on the wave payload (subset of `expected_dimensions`).
    /// The plan's R1.1 calls these `expected_dimensions`; we keep the
    /// `dimension` field on the wire (each `review.wave.ready` carries the
    /// specific dimension for that worker), and de-duplicate to a set.
    pub expected_dimensions: Vec<String>,
    /// `expected_dimensions - received_dimensions` (deterministic order,
    /// sorted alphabetically so the prompt is stable across runs).
    pub missing_dimensions: Vec<String>,
    /// Convenience flag equal to `received_count == wave_total && wave_total > 0`.
    pub all_dimensions_received: bool,
    /// Set by the event loop when this wave was activated by an aggregate
    /// timeout (`inject_review_aggregate_timeouts`).  Default: false.
    pub aggregate_timeout: bool,
}

impl WaveContext {
    /// Serialise the context as a JSON object suitable for embedding in
    /// prompt blocks and `RALPH_WAVE_CONTEXT` env vars.  Field order is
    /// stable: callers can compare two serialised contexts to detect drift.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "wave_id": self.wave_id,
            "wave_total": self.wave_total,
            "received_count": self.received_count,
            "expected_dimensions": self.expected_dimensions,
            "missing_dimensions": self.missing_dimensions,
            "ALL_DIMENSIONS_RECEIVED": self.all_dimensions_received,
            "AGGREGATE_TIMEOUT": self.aggregate_timeout,
        })
    }

    /// Render the prompt block to be prepended to the synthesizer's prompt.
    /// The block uses a fixed `## WAVE CONTEXT` heading so the agent can
    /// grep for it and so log scrapers can match it.
    pub fn to_prompt_block(&self) -> String {
        let json =
            serde_json::to_string_pretty(&self.to_json()).unwrap_or_else(|_| "{}".to_string());
        format!(
            "## WAVE CONTEXT\n\
             The following wave metadata is injected by the runner. \
             Do not count events manually — use this context.\n\n\
             ```json\n{json}\n```\n\n"
        )
    }
}

/// Read the events file line-by-line, returning the parsed `Event` for
/// every JSON line that round-trips through `serde_json::from_str`.  Lines
/// that fail to parse are silently skipped — the resolver is best-effort
/// and must not block the loop on a single malformed JSONL row.  The reader
/// caps the input at `tail_lines` lines (counted from the end) so a
/// multi-megabyte events file does not load the whole thing into memory.
pub(crate) fn read_recent_events(events_file: &Path, tail_lines: usize) -> Vec<Event> {
    let file = match fs::File::open(events_file) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let reader = BufReader::new(file);
    // Use a ring buffer for the tail.  This avoids loading the entire
    // events file when the loop has been running for hours; a 2000-line
    // tail is enough to cover any plausible review wave.
    let mut ring: std::collections::VecDeque<String> =
        std::collections::VecDeque::with_capacity(tail_lines);
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if line.is_empty() {
            continue;
        }
        if ring.len() == tail_lines {
            ring.pop_front();
        }
        ring.push_back(line);
    }
    ring.into_iter()
        .filter_map(|line| serde_json::from_str::<Event>(&line).ok())
        .collect()
}

/// Per-wave running tally built while scanning the events tail.
#[derive(Debug, Default)]
struct WaveAccumulator {
    /// Dimensions declared on `review.wave.ready` payloads.  We de-dup
    /// across multiple ready events for the same wave (the dispatcher
    /// may emit one per worker).
    expected: BTreeSet<String>,
    /// Dimensions returned via `review.dimension.done`.
    received: BTreeSet<String>,
    /// `wave_total` from the most recent `review.wave.ready`.  When the
    /// payload omits the field we fall back to `expected.len()` so the
    /// comparator works.
    wave_total: u32,
}

/// Build a [`WaveContext`] from a list of `Event`s that are known to
/// belong to a single wave (`wave_id`).  Used by the resolver and by
/// the unit tests.
fn context_from_wave(
    wave_id: &str,
    acc: &WaveAccumulator,
    aggregate_timeout: bool,
) -> Option<WaveContext> {
    if wave_id.is_empty() {
        return None;
    }
    let wave_total = if acc.wave_total > 0 {
        acc.wave_total
    } else {
        acc.expected.len() as u32
    };
    let expected_dimensions: Vec<String> = acc.expected.iter().cloned().collect();
    let received_dimensions: BTreeSet<String> = acc.received.clone();
    let missing_dimensions: Vec<String> = acc
        .expected
        .difference(&received_dimensions)
        .cloned()
        .collect();
    let received_count = received_dimensions.len() as u32;
    let all_dimensions_received =
        wave_total > 0 && received_count >= wave_total && missing_dimensions.is_empty();
    Some(WaveContext {
        wave_id: wave_id.to_string(),
        wave_total,
        received_count,
        expected_dimensions,
        missing_dimensions,
        all_dimensions_received,
        aggregate_timeout,
    })
}

/// Resolve the wave context for the current `review-synthesizer`
/// activation.  Returns `None` when no relevant wave events are present
/// (idle loop, non-wave preset) so the caller can fall back to the
/// pre-R1 behaviour.
pub fn resolve_wave_context_for_synthesizer(
    events_file: &Path,
    tail_lines: usize,
) -> Option<WaveContext> {
    resolve_wave_context_for_synthesizer_with_aggregate_timeout(events_file, tail_lines, false)
}

/// Same as [`resolve_wave_context_for_synthesizer`] but lets the caller
/// override the `AGGREGATE_TIMEOUT` flag — used by the event loop when
/// the synthesizer was activated by `inject_review_aggregate_timeouts`.
pub fn resolve_wave_context_for_synthesizer_with_aggregate_timeout(
    events_file: &Path,
    tail_lines: usize,
    aggregate_timeout: bool,
) -> Option<WaveContext> {
    let events = read_recent_events(events_file, tail_lines);
    if events.is_empty() {
        return None;
    }

    // Per-wave accumulator.  We scan in chronological order so a later
    // `review.wave.ready` for the same wave can refresh `wave_total` /
    // `expected` and the per-dimension `done` events land in the right
    // bucket.  The fallback when a `ready` event has no `wave_id` is to
    // mint a synthetic key from the hat's source plus the topic so the
    // record is still grouped correctly within a single dispatch.
    let mut per_wave: HashMap<String, WaveAccumulator> = HashMap::new();
    // Wave ids seen in `done` events but not in any `ready` event — the
    // worker fired before we saw the dispatch (race) or the ready was
    // truncated by the tail.  We materialise an empty accumulator so the
    // agent still gets a useful context (with `wave_total = 0`).
    let mut orphan_dones: HashMap<String, WaveAccumulator> = HashMap::new();

    let mut latest_wave_id: Option<String> = None;
    for event in &events {
        match event.topic.as_str() {
            "review.wave.ready" => {
                let Some(wave_id) = event.wave_id.clone() else {
                    // No wave_id at all — the dispatcher is supposed to
                    // stamp every `ready` event, but defensive parsing
                    // means we just skip the record rather than guessing.
                    continue;
                };
                let acc = per_wave.entry(wave_id.clone()).or_default();
                if let Some(total) = event.wave_total {
                    acc.wave_total = total;
                }
                if let Ok(obj) = serde_json::from_str::<serde_json::Value>(event.payload.as_str())
                    && let Some(dim) = obj.get("dimension").and_then(|v| v.as_str())
                {
                    acc.expected.insert(dim.to_string());
                }
                latest_wave_id = Some(wave_id);
            }
            "review.dimension.done" => {
                let Some(wave_id) = event.wave_id.clone() else {
                    continue;
                };
                let acc = if per_wave.contains_key(&wave_id) {
                    per_wave.get_mut(&wave_id).expect("just checked")
                } else {
                    orphan_dones.entry(wave_id.clone()).or_default()
                };
                if let Ok(obj) = serde_json::from_str::<serde_json::Value>(event.payload.as_str())
                    && let Some(dim) = obj.get("dimension").and_then(|v| v.as_str())
                {
                    acc.received.insert(dim.to_string());
                }
            }
            _ => {}
        }
    }

    // Pick the most relevant wave:
    //   1. the most recent wave that has at least one `review.dimension.done`
    //      event but is not yet complete (so the synthesizer can finish it);
    //   2. otherwise, the most recent wave that has any `review.wave.ready`
    //      event (so the agent gets a snapshot of the latest dispatch);
    //   3. otherwise, the most recent orphan wave (wave_id only on done
    //      events) so the agent can still see partial coverage.
    let pick_wave = |per_wave: &HashMap<String, WaveAccumulator>| -> Option<String> {
        per_wave
            .iter()
            .max_by_key(|(wid, acc)| {
                let received = acc.received.len() as u32;
                let expected = if acc.wave_total > 0 {
                    acc.wave_total
                } else {
                    acc.expected.len() as u32
                };
                let pending = expected.saturating_sub(received);
                // Prefer the wave with the most received dimensions; break
                // ties by preferring the wave with the most pending
                // dimensions (i.e. the most useful to summarize); break
                // further ties by lexicographic wave_id so the result is
                // deterministic.
                (received, pending, (*wid).clone())
            })
            .map(|(wid, _)| wid.clone())
    };

    let chosen = pick_wave(&per_wave)
        .or_else(|| pick_wave(&orphan_dones))
        .or(latest_wave_id);

    let wave_id = chosen?;

    if let Some(acc) = per_wave.get(&wave_id) {
        context_from_wave(&wave_id, acc, aggregate_timeout)
    } else if let Some(acc) = orphan_dones.get(&wave_id) {
        context_from_wave(&wave_id, acc, aggregate_timeout)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_proto::Event;
    use std::io::Write;

    fn write_events(path: &Path, events: &[Event]) {
        let mut f = fs::File::create(path).expect("create");
        for event in events {
            let line = serde_json::to_string(event).expect("serialize");
            writeln!(f, "{line}").expect("write");
        }
    }

    fn ready(wave_id: &str, total: u32, dimension: &str) -> Event {
        let mut e = Event::new(
            "review.wave.ready",
            format!(
                r#"{{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"{dimension}"}}"#
            ),
        );
        e.wave_id = Some(wave_id.to_string());
        e.wave_total = Some(total);
        e
    }

    fn done(wave_id: &str, dimension: &str) -> Event {
        let mut e = Event::new(
            "review.dimension.done",
            format!(
                r#"{{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"{dimension}","findings_count":0,"findings_file":"f.json"}}"#
            ),
        );
        e.wave_id = Some(wave_id.to_string());
        e
    }

    #[test]
    fn resolve_wave_context_basic_partial() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        let events = vec![
            ready("w-1", 7, "correctness"),
            ready("w-1", 7, "testing"),
            done("w-1", "correctness"),
            done("w-1", "testing"),
            done("w-1", "maintainability"),
        ];
        write_events(&path, &events);

        let ctx = resolve_wave_context_for_synthesizer(&path, 100).expect("context");
        assert_eq!(ctx.wave_id, "w-1");
        assert_eq!(ctx.wave_total, 7);
        assert_eq!(ctx.received_count, 3);
        assert_eq!(ctx.expected_dimensions.len(), 2);
        assert_eq!(ctx.missing_dimensions, Vec::<String>::new());
        assert!(!ctx.all_dimensions_received);
        assert!(!ctx.aggregate_timeout);
    }

    #[test]
    fn resolve_wave_context_all_received() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        // Build 7 ready + 7 done events sharing wave_id="w-full".  The
        // resolver picks the wave with the most received dimensions, so
        // the all-received wave wins.
        let mut events = Vec::new();
        let dims = [
            "correctness",
            "testing",
            "maintainability",
            "standards",
            "requirements",
            "agent-native",
            "learnings",
        ];
        for d in &dims {
            events.push(ready("w-full", 7, d));
        }
        for d in &dims {
            events.push(done("w-full", d));
        }
        // Add a smaller, partial wave to make sure the resolver prefers
        // the completed one.
        events.push(ready("w-partial", 3, "sec"));
        events.push(done("w-partial", "sec"));
        write_events(&path, &events);

        let ctx = resolve_wave_context_for_synthesizer(&path, 100).expect("context");
        assert_eq!(ctx.wave_id, "w-full");
        assert_eq!(ctx.wave_total, 7);
        assert_eq!(ctx.received_count, 7);
        assert!(ctx.all_dimensions_received);
        assert!(ctx.missing_dimensions.is_empty());
    }

    #[test]
    fn resolve_wave_context_no_wave_events_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        // Only non-wave events; the resolver must return None so the
        // caller can fall back to the pre-R1 behaviour.
        let events = vec![
            Event::new("work.start", r#"{"plan_name":"p"}"#),
            Event::new("work.ready", r#"{"plan_name":"p","task_id":"t1"}"#),
        ];
        write_events(&path, &events);

        assert!(resolve_wave_context_for_synthesizer(&path, 100).is_none());
    }

    #[test]
    fn resolve_wave_context_missing_dimensions_populated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        let events = vec![
            ready("w-1", 4, "correctness"),
            ready("w-1", 4, "testing"),
            ready("w-1", 4, "maintainability"),
            ready("w-1", 4, "standards"),
            done("w-1", "correctness"),
        ];
        write_events(&path, &events);

        let ctx = resolve_wave_context_for_synthesizer(&path, 100).expect("context");
        assert_eq!(ctx.wave_total, 4);
        assert_eq!(ctx.received_count, 1);
        assert!(!ctx.all_dimensions_received);
        let missing: BTreeSet<_> = ctx.missing_dimensions.iter().cloned().collect();
        assert!(missing.contains("testing"));
        assert!(missing.contains("maintainability"));
        assert!(missing.contains("standards"));
        // Determinism: the field is sorted alphabetically.
        assert_eq!(
            ctx.missing_dimensions,
            vec!["maintainability", "standards", "testing"]
        );
    }

    #[test]
    fn resolve_wave_context_aggregate_timeout_flag() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        // Three `ready` events declare the expected dimensions; only one
        // `done` has come back so far.  The resolver must surface the
        // missing dimensions in deterministic order.
        let events = vec![
            ready("w-1", 3, "correctness"),
            ready("w-1", 3, "maintainability"),
            ready("w-1", 3, "testing"),
            done("w-1", "correctness"),
        ];
        write_events(&path, &events);

        let ctx = resolve_wave_context_for_synthesizer_with_aggregate_timeout(&path, 100, true)
            .expect("context");
        assert!(ctx.aggregate_timeout);
        assert_eq!(ctx.received_count, 1);
        assert_eq!(ctx.missing_dimensions, vec!["maintainability", "testing"]);
    }

    #[test]
    fn prompt_block_contains_stable_heading_and_json() {
        let ctx = WaveContext {
            wave_id: "w-1".into(),
            wave_total: 7,
            received_count: 7,
            expected_dimensions: vec!["correctness".into()],
            missing_dimensions: vec![],
            all_dimensions_received: true,
            aggregate_timeout: false,
        };
        let block = ctx.to_prompt_block();
        assert!(block.starts_with("## WAVE CONTEXT\n"));
        assert!(block.contains("\"ALL_DIMENSIONS_RECEIVED\": true"));
        assert!(block.contains("\"AGGREGATE_TIMEOUT\": false"));
        assert!(block.contains("\"wave_total\": 7"));
    }

    #[test]
    fn read_recent_events_respects_tail_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        let events: Vec<Event> = (0..10)
            .map(|i| Event::new("work.start", format!(r#"{{"i":{i}}}"#)))
            .collect();
        write_events(&path, &events);

        // Force the ring to be smaller than the file: 4 tail lines, only
        // the last 4 events should land in the output.  We don't introspect
        // the ring directly; instead we re-emit a context and assert that
        // none of the parser paths panic.
        let parsed = read_recent_events(&path, 4);
        assert_eq!(parsed.len(), 4);
        // The most recent 4 events have i in 6..=9.
        let last = parsed.last().expect("non-empty");
        let v: serde_json::Value = serde_json::from_str(&last.payload).expect("json");
        assert_eq!(v["i"], 9);
    }
}
