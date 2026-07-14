use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use ralph_adapters::{
    OutputFormat as BackendOutputFormat, PiAssistantEvent, PiContentBlock, PiStreamEvent,
    PiStreamParser, TraeStreamEvent, TraeStreamParser, extract_assistant_text,
    extract_assistant_tool_calls, extract_user_tool_result_text, user_is_tool_result,
};
use ratatui::text::Line;

/// 2026-06-17-002 U5 R5: describes a single dimension-mismatch
/// slot that the merge layer detected. Returned by
/// `merge_wave_results_to_events_file` so the caller can inject
/// `task.resume` events to retry the mismatched slot.
///
/// One entry per mismatched worker index. If a worker emits
/// multiple mismatched events, only the first is recorded (the
/// `mismatch_indexes` BTreeSet in the merge body deduplicates).
#[derive(Debug, Clone)]
pub struct DimensionMismatchInfo {
    /// Worker index in the wave (0-based).
    #[cfg_attr(not(test), allow(dead_code))]
    pub wave_index: u32,
    /// The dimension the dispatcher assigned to this slot.
    #[cfg_attr(not(test), allow(dead_code))]
    pub expected_dimension: String,
    /// The dimension the worker actually emitted.
    #[cfg_attr(not(test), allow(dead_code))]
    pub actual_dimension: String,
}

/// 2026-06-17-002 U5/R5 (P0#4 fix): the merge layer returns the
/// pre-rendered `task.resume` JSONL records it WOULD inject for
/// each mismatched slot. The caller (dispatcher) decides — based on
/// the per-slot retry budget it reads from the WaveTracker — which
/// records to include in the final atomic `write_all`. Building
/// the records inside the merge function (instead of a separate
/// `inject_dimension_retry_task_resume` call that re-opens the
/// events file) eliminates the previous concurrent-append race
/// (merge + inject interleaving `writeln!` syscalls on the same
/// file descriptor) by collapsing all writes into a single
/// `write_all` per dispatch round.
#[derive(Debug, Clone)]
pub struct PendingTaskResumeRecord {
    /// Worker index this resume targets.
    pub wave_index: u32,
    /// The full JSONL line to append (without trailing newline).
    pub jsonl_line: String,
}

/// 2026-06-17-002 U4 R4: extract the `dimension` field from an
/// event payload. Returns `None` for missing / empty / non-JSON /
/// non-string / whitespace-only / absent-key payloads. The merge
/// layer treats a missing `dimension` field on a
/// `review.dimension.done` event as `dimension_missing`; events on
/// other topics with a missing `dimension` field simply pass
/// through.
fn parse_payload_dimension(payload: Option<&str>) -> Option<String> {
    let payload = payload?.trim();
    if payload.is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    value
        .get("dimension")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Push a styled line to a TUI wave worker's output buffer.
pub fn push_to_wave_worker_buffer(
    state: &Arc<std::sync::Mutex<ralph_tui::TuiState>>,
    worker_idx: usize,
    lines: &[Line<'static>],
) {
    let Ok(s) = state.lock() else { return };
    let Some(ref wave) = s.wave_active else {
        return;
    };
    let Some(buffer) = wave.worker_buffers.get(worker_idx) else {
        return;
    };
    let handle = buffer.lines_handle();
    let Ok(mut buf_lines) = handle.lock() else {
        return;
    };
    buf_lines.extend_from_slice(lines);
}
/// Push a line to the latest iteration's output in the TUI.
pub fn push_to_tui_iteration(
    state: &Arc<std::sync::Mutex<ralph_tui::TuiState>>,
    line: Line<'static>,
) {
    let Ok(s) = state.lock() else { return };
    let Some(handle) = s.latest_iteration_lines_handle() else {
        return;
    };
    let Ok(mut lines) = handle.lock() else { return };
    lines.push(line);
}
pub fn truncate_wave_worker_preview(text: &str) -> String {
    if text.len() > 200 {
        let end = ralph_core::floor_char_boundary(text, 200);
        format!("{}…", &text[..end])
    } else {
        text.to_string()
    }
}
/// Extract a human-readable text delta from a single stdout line.
pub fn extract_readable_delta(line: &str, output_format: BackendOutputFormat) -> Option<String> {
    match output_format {
        BackendOutputFormat::Text => Some(format!("{line}\n")),
        BackendOutputFormat::StreamJson => {
            use ralph_adapters::{ClaudeStreamEvent, ClaudeStreamParser, ContentBlock};
            match ClaudeStreamParser::parse_line(line) {
                Some(ClaudeStreamEvent::Assistant { message, .. }) => {
                    let mut text = String::new();
                    for block in message.content {
                        match block {
                            ContentBlock::Text { text: t } => {
                                text.push_str(&t);
                                text.push('\n');
                            }
                            ContentBlock::Thinking { thinking, .. } => {
                                text.push_str(&thinking);
                                text.push('\n');
                            }
                            ContentBlock::ToolUse { name, input, .. } => {
                                text.push_str(&format!("⚙ {name}({input})\n"));
                            }
                        }
                    }
                    if text.is_empty() { None } else { Some(text) }
                }
                Some(ClaudeStreamEvent::User { message }) => {
                    let mut text = String::new();
                    for block in message.content {
                        let ralph_adapters::UserContentBlock::ToolResult { content, .. } = block;
                        if !content.is_empty() {
                            text.push_str(&format!(
                                "→ {}\n",
                                truncate_wave_worker_preview(&content)
                            ));
                        }
                    }
                    if text.is_empty() { None } else { Some(text) }
                }
                _ => None,
            }
        }
        BackendOutputFormat::PiStreamJson => match PiStreamParser::parse_line(line) {
            Some(PiStreamEvent::MessageUpdate {
                assistant_message_event: PiAssistantEvent::TextDelta { delta },
            }) => Some(delta),
            Some(PiStreamEvent::MessageUpdate {
                assistant_message_event: PiAssistantEvent::Error { reason },
            }) => Some(format!("✗ {}\n", truncate_wave_worker_preview(&reason))),
            Some(PiStreamEvent::ToolExecutionStart {
                tool_name, args, ..
            }) => Some(format!("⚙ {tool_name}({args})\n")),
            Some(PiStreamEvent::ToolExecutionEnd {
                result, is_error, ..
            }) => {
                let output = result
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        PiContentBlock::Text { text } => Some(text.as_str()),
                        PiContentBlock::Other => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if output.is_empty() {
                    None
                } else if is_error {
                    Some(format!("✗ {}\n", truncate_wave_worker_preview(&output)))
                } else {
                    Some(format!("→ {}\n", truncate_wave_worker_preview(&output)))
                }
            }
            _ => None,
        },
        // Parse trae NDJSON: extract assistant text, tool calls, and tool results
        // for the wave worker preview pane (mirrors the pi pattern above).
        BackendOutputFormat::TraeStreamJson => match TraeStreamParser::parse_line(line) {
            Some(TraeStreamEvent::Assistant { message }) => {
                if let Some(text) = extract_assistant_text(&message) {
                    let text = if text.ends_with('\n') {
                        text
                    } else {
                        format!("{}\n", text)
                    };
                    return Some(text);
                }
                let calls = extract_assistant_tool_calls(&message);
                if let Some(call) = calls.into_iter().next() {
                    let args_display = if call.function.arguments.is_empty() {
                        String::new()
                    } else {
                        truncate_wave_worker_preview(&call.function.arguments)
                    };
                    return Some(format!("⚙ {}({})\n", call.function.name, args_display));
                }
                None
            }
            Some(TraeStreamEvent::User {
                subtype,
                tool_use_id,
                content,
                ..
            }) => {
                if user_is_tool_result(subtype.as_deref(), tool_use_id.as_deref(), &content) {
                    if let Some(output) = extract_user_tool_result_text(&content) {
                        if !output.is_empty() {
                            return Some(format!("→ {}\n", truncate_wave_worker_preview(&output)));
                        }
                    }
                }
                None
            }
            _ => None,
        },
    }
}
/// Read events from a per-worker events file.
///
/// Normalizes each line the same way `EventRecordRaw::from` does for the
/// main events file (`ts`/`timestamp` and `topic`/`type` fallbacks).  The
/// per-worker file is written by `merge_wave_results_to_events_file`
/// using a similar bridge format, but agents may also hand-write events
/// there (e.g. `ralph emit work.done --payload ...`) which uses the
/// off-spec `"type"` field instead of `"topic"`.  Without this
/// normalization step, the aggregator would silently drop those events.
pub fn read_worker_events(path: &Path) -> Vec<ralph_core::Event> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };

    content
        .lines()
        .filter_map(parse_worker_event_line)
        .collect()
}

/// Parse a single JSONL line into an `Event`, applying the same `topic`/
/// `type` fallback that `EventRecordRaw` does for the main events file.
fn parse_worker_event_line(line: &str) -> Option<ralph_core::Event> {
    let mut value: serde_json::Value = serde_json::from_str(line).ok()?;

    // Off-spec agents sometimes write `{"type": "..."}` instead of the
    // canonical `{"topic": "..."}`.  Promote `type` → `topic` only when
    // `topic` is absent or null, mirroring `EventRecordRaw`'s fallback.
    if let Some(obj) = value.as_object_mut() {
        let topic_missing = obj.get("topic").map(|v| v.is_null()).unwrap_or(true);
        if topic_missing {
            if let Some(type_val) = obj.remove("type") {
                obj.insert("topic".to_string(), type_val);
            }
        }
    }

    serde_json::from_value::<ralph_core::Event>(value).ok()
}
pub fn read_worker_events_with_retry(path: &Path, timeout: Duration) -> Vec<ralph_core::Event> {
    let start = std::time::Instant::now();
    loop {
        let events = read_worker_events(path);
        if !events.is_empty() || start.elapsed() >= timeout {
            return events;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
/// Merge wave result events into the main events file.
///
/// Appends all result events to the main JSONL file so the aggregator hat
/// picks them up on the next iteration.
///
/// 2026-06-07 plan Unit 3 (R8): every record this function writes MUST
/// carry `wave_id`, `wave_index`, `wave_total` and a `ts` field.  The
/// worker process is forbidden from writing to the main events file
/// (its `RALPH_EVENTS_FILE` env var points at a per-worker file), so
/// any record missing these fields is a bypass attempt or a stale
/// hand-written file from a historical run.
///
/// 2026-06-16-001 U2: synthetic `wave.worker.failed` records use
/// `failure_source_hat` (default: `review-synthesizer`) as the
/// provenance, NOT `default_source_hat` (the wave's target hat, e.g.
/// `review-coordinator`). The previous behaviour labelled the synthetic
/// record as `review-coordinator`, but `review-coordinator` does NOT
/// declare `wave.worker.failed` in its `publishes` list, so the origin
/// guard rejected the record and emitted a stray `task.resume` against
/// review-coordinator — a self-inflicted stall on an already-partial
/// wave. `review-synthesizer` is the wave-result aggregator and
/// declares `wave.worker.failed` in its `publishes` list (see
/// `presets/en/ce-executor-pipeline.yml`), so the synthetic record
/// now passes the origin guard and reaches the synthesizer as the
/// intended aggregated-failure signal.
///
/// The synthetic payload is a JSON object (`{reason, wave_id,
/// wave_index, error}`) instead of a free-form string, so the
/// synthesizer and downstream diagnostic tooling can parse it
/// uniformly. The legacy free-form string would still surface in
/// `recovery.jsonl` and `events.jsonl`, but structured access by
/// the synthesizer's `aggregate.wait_for_all` incomplete-wave path
/// requires a parseable payload.
/// 2026-06-17-002 U4 R4: dimension gate. When `completed.assigned_dimensions`
/// maps a worker index to a dimension string, the worker MUST emit
/// `review.dimension.done` with a matching `dimension` field. Any event
/// whose payload's `dimension` does not equal the assigned value is
/// dropped from the merge (NOT coerced / rewritten — we never mutate
/// the worker's `dimension` field, that would hide the agent error and
/// mis-attribute findings) and converted into a synthetic
/// `wave.worker.failed(reason=dimension_mismatch)` record carrying the
/// expected / actual dimensions. The synthesizer's
/// `WaveContext.missing_dimensions` therefore covers both
/// "never reported" slots and "reported wrong dimension" slots.
///
/// 2026-06-17-002 U5 R5: the function returns
/// `(Vec<DimensionMismatchInfo>, Vec<PendingTaskResumeRecord>)` —
/// the first is the human-readable summary used in tests and
/// logging, the second is the pre-rendered JSONL lines the
/// dispatcher appends to the events file together with the merged
/// records (single `write_all`, no concurrent-append race). The
/// dispatcher filters the second list through the WaveTracker's
/// per-slot retry quota before appending, so a permanently
/// mismatched worker cannot drain more than
/// `MAX_DIMENSION_RETRIES_PER_SLOT` retries across the wave's
/// lifetime.
pub fn merge_wave_results_to_events_file(
    completed: &ralph_core::CompletedWave,
    events_file: &Path,
    publish_topics: &[String],
    default_source_hat: &str,
    failure_source_hat: Option<&str>,
) -> Result<(Vec<DimensionMismatchInfo>, Vec<PendingTaskResumeRecord>)> {
    use std::io::Write;

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(events_file)
        .with_context(|| format!("Failed to open events file: {}", events_file.display()))?;

    let ts = chrono::Utc::now().to_rfc3339();

    // Build all records into a single buffer, then write_all once for atomic
    // append (consistent with EventLogger::log and write_wave_events).
    let mut buf = String::new();

    let mut merged_indexes: Vec<u32> = Vec::new();
    let mut duplicate_indexes: Vec<u32> = Vec::new();
    // U4/R4: collect synthetic `WaveFailure` records for
    // dimension mismatches so we can write `wave.worker.failed`
    // records after the per-event loop finishes.
    let mut mismatch_failures: Vec<ralph_core::WaveFailure> = Vec::new();
    // Tracks which worker indexes the dimension gate already
    // failed, so a single worker emitting multiple mismatched
    // events produces exactly one synthetic record (not N).
    let mut mismatch_indexes: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    // U5/R5: collect per-slot dimension mismatch info so the
    // caller can inject `task.resume` events. One entry per
    // mismatched index (deduplicated by `mismatch_indexes`).
    let mut mismatch_details: Vec<DimensionMismatchInfo> = Vec::new();
    // P0#4 / P1#11 fix: pre-render the `task.resume` JSONL lines
    // here so the dispatcher appends them to the SAME `buf` that
    // is flushed in the single `write_all` below. No separate
    // file open / `writeln!` race. The dispatcher filters these
    // through the WaveTracker's per-slot retry quota before
    // appending, so a permanently-mismatched worker never gets
    // more than `MAX_DIMENSION_RETRIES_PER_SLOT` retries.
    let mut pending_task_resumes: Vec<PendingTaskResumeRecord> = Vec::new();

    for result in &completed.results {
        if merged_indexes.contains(&result.index) {
            duplicate_indexes.push(result.index);
        } else {
            merged_indexes.push(result.index);
        }
        for event in &result.events {
            // 2026-06-13-004 review fix (P0 #1 / ADV-2): reject
            // worker-written `event.source` that does not match the
            // dispatcher-expected `expected_source_hat`. Without
            // this, a malicious worker can claim any hat name
            // (e.g. `review-coordinator`) in its per-worker JSONL
            // and bypass the isolated scope check in
            // `process_parse_result` (U2). When
            // `expected_source_hat` is `None` (legacy wave or
            // smoke fixture), the check is skipped so the fix is
            // non-breaking.
            if let Some(expected) = completed.expected_source_hat.as_ref() {
                match event.source.as_ref() {
                    Some(s) if s == expected => {}
                    Some(s) => {
                        tracing::warn!(
                            wave_id = %completed.wave_id,
                            worker_index = result.index,
                            expected_hat = %expected.as_str(),
                            claimed_hat = %s.as_str(),
                            topic = %event.topic,
                            "ADV-2 hat-spoofing rejected: worker's `source` does not match dispatcher's `expected_source_hat`; dropping event"
                        );
                        continue;
                    }
                    None => {
                        tracing::warn!(
                            wave_id = %completed.wave_id,
                            worker_index = result.index,
                            expected_hat = %expected.as_str(),
                            topic = %event.topic,
                            "ADV-2 hat-spoofing rejected: worker omitted `source`; dropping event"
                        );
                        continue;
                    }
                }
            }
            // U4/R4 (2026-06-17-002): dimension gate. When this
            // worker index has an assigned dimension, the
            // event's payload `dimension` field (if any) MUST
            // match. We apply the check only to events whose
            // payload declares a `dimension` — non-dimension
            // events (e.g. internal `review.wave.ready`,
            // diagnostics) pass through unchanged so the gate
            // does not block unrelated traffic. The check is also
            // skipped entirely when the wave has no
            // `assigned_dimensions` map (legacy / non-review
            // waves).
            //
            // Important: we DROP mismatched events, we do NOT
            // rewrite the `dimension` field. Mutating the field
            // would silently accept the worker's mistake and
            // mis-attribute findings to the assigned slot.
            let assigned = completed.assigned_dimensions.get(&result.index).cloned();
            if let Some(assigned_dim) = assigned.as_ref() {
                if let Some(actual_dim) = parse_payload_dimension(Some(&event.payload)) {
                    if &actual_dim != assigned_dim {
                        if mismatch_indexes.insert(result.index) {
                            tracing::warn!(
                                wave_id = %completed.wave_id,
                                worker_index = result.index,
                                expected_dimension = %assigned_dim,
                                actual_dimension = %actual_dim,
                                topic = %event.topic,
                                "U4/R4 dimension mismatch: worker emitted review.dimension.done \
                                 with a dimension that does not match its assigned slot; \
                                 dropping event and writing synthetic wave.worker.failed"
                            );
                            mismatch_failures.push(ralph_core::WaveFailure::dimension_mismatch(
                                result.index,
                                assigned_dim.clone(),
                                actual_dim.clone(),
                                Duration::ZERO,
                            ));
                            // U5/R5: record the mismatch so the
                            // caller can inject a `task.resume`
                            // event to retry the slot.
                            mismatch_details.push(DimensionMismatchInfo {
                                wave_index: result.index,
                                expected_dimension: assigned_dim.clone(),
                                actual_dimension: actual_dim.clone(),
                            });
                            // P0#4 fix: also pre-render the
                            // `task.resume` JSONL line. Same ts as
                            // the rest of the records in this
                            // dispatch round. The retry_key
                            // includes the wave id and worker
                            // index so the operation is
                            // idempotent across dispatch rounds.
                            let retry_key = format!(
                                "wave_dimension_guard:{}:{}:dimension_mismatch:dimension",
                                completed.wave_id, result.index
                            );
                            let resume_payload = serde_json::json!({
                                "stage": "WaveDimensionGuard",
                                "topic": "review.dimension.done",
                                "violation": "dimension_mismatch",
                                "allowed_topics": ["review.dimension.done"],
                                "required_fields": ["dimension"],
                                "original_trigger_topic": "review.wave.ready",
                                "retry_key": retry_key,
                                "original_hat": "dimension-reviewer",
                                "wave_id": completed.wave_id,
                                "wave_index": result.index,
                                "wave_total": completed.wave_total,
                                "reason": "dimension_mismatch",
                                "target_hat": "dimension-reviewer",
                                "expected_dimension": assigned_dim,
                                "actual_dimension": actual_dim,
                            });
                            let resume_record = serde_json::json!({
                                "topic": "task.resume",
                                "triggered": "dimension-reviewer",
                                "hat": "review-synthesizer",
                                "source": "review-synthesizer",
                                "payload": resume_payload.to_string(),
                                "ts": ts,
                                "wave_id": completed.wave_id,
                                "wave_index": result.index,
                                "wave_total": completed.wave_total,
                            });
                            pending_task_resumes.push(PendingTaskResumeRecord {
                                wave_index: result.index,
                                jsonl_line: serde_json::to_string(&resume_record)?,
                            });
                        }
                        continue;
                    }
                }
                // No `dimension` field in the payload — for
                // `review.dimension.done` events that is a
                // contract violation (the topic is supposed to
                // carry one). Treat it as a missing-dimension
                // mismatch: drop and record.
                else if event.topic.as_str() == "review.dimension.done"
                    && mismatch_indexes.insert(result.index)
                {
                    tracing::warn!(
                        wave_id = %completed.wave_id,
                        worker_index = result.index,
                        expected_dimension = %assigned_dim,
                        topic = %event.topic,
                        "U4/R4 dimension missing: review.dimension.done emitted without a \
                         `dimension` field; dropping event and writing synthetic wave.worker.failed"
                    );
                    mismatch_failures.push(ralph_core::WaveFailure::dimension_missing(
                        result.index,
                        assigned_dim.clone(),
                        Duration::ZERO,
                    ));
                    continue;
                }
            }
            // Phase 2: in isolated mode provenance is a property of the
            // worker channel, not the self-declared `hat`/`source` fields.
            // The dispatcher stamps every merged record with the wave's
            // target hat, overriding any value written by the worker. The
            // ADV-2 check above still drops records whose `source` claims
            // a different hat (when the dispatcher told the worker what to
            // set).
            let hat = default_source_hat;
            let record = serde_json::json!({
                "topic": event.topic.as_str(),
                "payload": event.payload,
                "ts": ts,
                "wave_id": completed.wave_id,
                "wave_index": result.index,
                "wave_total": completed.wave_total,
                "hat": hat,
                "source": hat,
            });
            buf.push_str(&serde_json::to_string(&record)?);
            buf.push('\n');
        }
    }

    // Also write failure events so the aggregator knows about partial results
    for failure in &completed.failures {
        write_synthetic_worker_failed(
            &mut buf,
            &ts,
            completed,
            failure,
            default_source_hat,
            publish_topics,
            failure_source_hat,
        );
    }

    // U4/R4 (2026-06-17-002): emit the synthetic
    // `wave.worker.failed(reason=dimension_mismatch|dimension_missing)`
    // records produced by the dimension gate above. Reuses the
    // failure-emission path so the synthesizer sees both flavors
    // identically.
    for failure in &mismatch_failures {
        write_synthetic_worker_failed(
            &mut buf,
            &ts,
            completed,
            failure,
            default_source_hat,
            publish_topics,
            failure_source_hat,
        );
    }

    file.write_all(buf.as_bytes())?;

    // R8 observability: log expected/merged/missing/duplicate indexes so a
    // postmortem can tell at a glance whether the wave was complete.
    let expected_indexes: std::collections::BTreeSet<u32> = (0..completed.wave_total).collect();
    let failure_indexes: Vec<u32> = completed
        .failures
        .iter()
        .map(|f| f.index)
        .chain(mismatch_failures.iter().map(|f| f.index))
        .collect();
    let accounted_indexes: std::collections::BTreeSet<u32> = merged_indexes
        .iter()
        .chain(failure_indexes.iter())
        .copied()
        .collect();
    let missing_indexes: Vec<u32> = expected_indexes
        .difference(&accounted_indexes)
        .copied()
        .collect();

    if !missing_indexes.is_empty() || !duplicate_indexes.is_empty() {
        tracing::warn!(
            wave_id = %completed.wave_id,
            wave_total = completed.wave_total,
            merged = merged_indexes.len(),
            missing = ?missing_indexes,
            duplicate = ?duplicate_indexes,
            failures = ?failure_indexes,
            "Wave merge produced incomplete or duplicate index set"
        );
    } else {
        tracing::info!(
            wave_id = %completed.wave_id,
            wave_total = completed.wave_total,
            merged = merged_indexes.len(),
            "Wave merge complete with all expected indexes present"
        );
    }

    Ok((mismatch_details, pending_task_resumes))
}

/// Serialize a single `WaveFailure` into the events-file buffer:
/// one `wave.worker.failed` record plus one synthetic event per
/// publish topic so the aggregator hat still gets a trigger.
///
/// U4/R4 (2026-06-17-002) consolidates real worker failures and
/// dimension-gate synthetic failures through this helper so the
/// two flavours share identical wire format.
fn write_synthetic_worker_failed(
    buf: &mut String,
    ts: &str,
    completed: &ralph_core::CompletedWave,
    failure: &ralph_core::WaveFailure,
    default_source_hat: &str,
    publish_topics: &[String],
    failure_source_hat: Option<&str>,
) {
    // 2026-06-16-001 U2: synthetic `wave.worker.failed` records
    // attribute to `failure_source_hat` (default `review-synthesizer`)
    // — see the function-level docstring for the rationale.
    let failure_hat = failure_source_hat.unwrap_or("review-synthesizer");
    // U4/R4 (2026-06-17-002): dimension-gate failures carry the
    // expected/actual dimensions as typed payload fields so the
    // synthesizer's WaveContext.missing_dimensions covers both
    // "never reported" and "reported wrong dimension" slots.
    let reason = if failure.expected_dimension.is_some() {
        // Strip the "dimension_mismatch: expected=X actual=Y"
        // / "dimension_missing: expected=X" prefix that
        // WaveFailure::dimension_mismatch / dimension_missing
        // baked into `error`; the structured fields below carry
        // the same information as JSON.
        if failure.actual_dimension.is_some() {
            "worker_failed:dimension_mismatch".to_string()
        } else {
            "worker_failed:dimension_missing".to_string()
        }
    } else {
        format!("worker_failed:{}", failure.error)
    };
    let mut failure_payload = serde_json::json!({
        "reason": reason,
        "wave_id": completed.wave_id,
        "wave_index": failure.index,
        "error": failure.error,
    });
    if let Some(expected) = &failure.expected_dimension {
        failure_payload["expected_dimension"] = serde_json::Value::String(expected.clone());
    }
    if let Some(actual) = &failure.actual_dimension {
        failure_payload["actual_dimension"] = serde_json::Value::String(actual.clone());
    }
    let failure_payload = failure_payload.to_string();
    let record = serde_json::json!({
        "topic": "wave.worker.failed",
        "payload": failure_payload,
        "ts": ts,
        "wave_id": completed.wave_id,
        "wave_index": failure.index,
        "wave_total": completed.wave_total,
        "hat": failure_hat,
        "source": failure_hat,
    });
    buf.push_str(&serde_json::to_string(&record).expect("serialize wave.worker.failed"));
    buf.push('\n');

    // Emit synthetic events on the hat's publish topics so downstream
    // aggregators can still trigger even when workers fail/timeout.
    // These follow-ups still use `default_source_hat` because they
    // are direct re-publications of the target hat's declared
    // publish topics (e.g. `review.dimension.done`), not the
    // dispatcher-internal `wave.worker.failed`.
    for topic in publish_topics {
        let record = serde_json::json!({
            "topic": topic,
            "payload": format!(
                "## Worker {} (FAILED)\n\nError: {}",
                failure.index, failure.error
            ),
            "ts": ts,
            "wave_id": completed.wave_id,
            "wave_index": failure.index,
            "wave_total": completed.wave_total,
            "hat": default_source_hat,
            "source": default_source_hat,
        });
        buf.push_str(&serde_json::to_string(&record).expect("serialize synthetic publish"));
        buf.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Regression test for P1-#4: per-worker event files written by
    /// off-spec agents using `{"type": "...", "payload": ...}` (instead
    /// of the canonical `{"topic": "...", "payload": ...}`) used to be
    /// silently dropped by `read_worker_events`, because the parser went
    /// straight through `serde_json::from_str::<ralph_core::Event>`
    /// without the `topic`/`type` fallback that `EventRecordRaw` applies
    /// to the main events file.  This test pins the new normalization.
    #[test]
    fn read_worker_events_promotes_type_field_to_topic() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        // Mix a canonical line, an off-spec `type` line, and a malformed
        // line.  Only the malformed one should be dropped.
        let mut f = std::fs::File::create(tmp.path()).unwrap();
        writeln!(f, r#"{{"topic": "work.done", "payload": "canonical"}}"#).unwrap();
        writeln!(
            f,
            r#"{{"type": "review.wave.ready", "payload": "off-spec"}}"#
        )
        .unwrap();
        writeln!(f, "not-json-at-all").unwrap();
        drop(f);

        let events = read_worker_events(tmp.path());
        assert_eq!(
            events.len(),
            2,
            "malformed line must be skipped, others parsed"
        );

        // Find each event by topic — order is preserved from the file.
        let canonical = events
            .iter()
            .find(|e| e.topic.as_str() == "work.done")
            .expect("canonical topic is preserved");
        assert_eq!(canonical.payload.as_deref(), Some("canonical"));

        let off_spec = events
            .iter()
            .find(|e| e.topic.as_str() == "review.wave.ready")
            .expect("off-spec `type` is promoted to `topic`");
        assert_eq!(off_spec.payload.as_deref(), Some("off-spec"));
    }

    /// Empty files and missing files are not errors — `read_worker_events`
    /// is called on a best-effort basis and the dispatcher may invoke it
    /// before the worker has produced any output.
    #[test]
    fn read_worker_events_missing_file_yields_empty() {
        let path = std::path::Path::new("/tmp/ralph-read-worker-events-missing.jsonl");
        let _ = std::fs::remove_file(path);
        let events = read_worker_events(path);
        assert!(events.is_empty());
    }

    // -------------------------------------------------------------------
    // U4/R4 + U5/R5 (2026-06-17-002): dimension gate tests for merge.
    // -------------------------------------------------------------------

    /// Helper: build a `review.dimension.done` event with a `dimension`
    /// field. The merge layer's dimension gate keys on the payload's
    /// `dimension` field, not on the topic or `wave_index` (those are
    /// stamped by the dispatcher at write time).
    fn dimension_done_event(_index: u32, dimension: &str) -> ralph_proto::Event {
        let payload = serde_json::json!({
            "plan_name": "p",
            "task_id": "t1",
            "task_key": "k1",
            "step": "1",
            "dimension": dimension,
            "findings_count": 0,
            "findings_file": "f.json",
        })
        .to_string();
        ralph_proto::Event::new("review.dimension.done", payload)
    }

    /// U4/R4: when a worker emits `review.dimension.done` with a
    /// `dimension` field that does NOT match its assigned slot, the
    /// merge layer must drop the event (no record written for it),
    /// stamp a synthetic `wave.worker.failed` record with
    /// `reason=worker_failed:dimension_mismatch` carrying
    /// `expected_dimension` / `actual_dimension` fields, and leave the
    /// other (correctly-matched) workers' events untouched.
    ///
    /// U5/R5: the function returns a `Vec<DimensionMismatchInfo>`
    /// describing the mismatched slot so the caller can inject
    /// `task.resume` retries.
    #[test]
    fn test_merge_drops_mismatched_dimension_event() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let events_file = tmp.path().join("events.jsonl");

        // 4 workers, wave_total=4. Worker 1 was assigned "testing"
        // but emitted "correctness"; the other 3 are correct.
        let mut assigned_dimensions = std::collections::HashMap::new();
        assigned_dimensions.insert(0u32, "correctness".to_string());
        assigned_dimensions.insert(1, "testing".to_string());
        assigned_dimensions.insert(2, "maintainability".to_string());
        assigned_dimensions.insert(3, "standards".to_string());

        let completed = ralph_core::CompletedWave {
            wave_id: "w-u4-dim".to_string(),
            wave_total: 4,
            results: vec![
                ralph_core::WaveResult {
                    index: 0,
                    events: vec![dimension_done_event(0, "correctness")],
                },
                ralph_core::WaveResult {
                    // Mismatch: assigned "testing", emitted "correctness".
                    index: 1,
                    events: vec![dimension_done_event(1, "correctness")],
                },
                ralph_core::WaveResult {
                    index: 2,
                    events: vec![dimension_done_event(2, "maintainability")],
                },
                ralph_core::WaveResult {
                    index: 3,
                    events: vec![dimension_done_event(3, "standards")],
                },
            ],
            failures: vec![],
            duration: Duration::from_millis(10),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions,
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };

        let (mismatches, _pending_resumes) = merge_wave_results_to_events_file(
            &completed,
            &events_file,
            &["review.dimension.done".to_string()],
            "review-coordinator",
            None,
        )
        .expect("merge must succeed");

        // U5/R5: the returned mismatch list must contain exactly
        // one entry for worker index 1 with the expected/actual
        // dimensions from the merge-layer detection.
        assert_eq!(
            mismatches.len(),
            1,
            "U5/R5: expected exactly 1 mismatch entry, got {mismatches:?}"
        );
        assert_eq!(mismatches[0].wave_index, 1);
        assert_eq!(mismatches[0].expected_dimension, "testing");
        assert_eq!(mismatches[0].actual_dimension, "correctness");

        let merged = fs::read_to_string(&events_file).expect("read merged events");
        let records: Vec<serde_json::Value> = merged
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("json event"))
            .collect();

        // 3 valid `review.dimension.done` records (correctness,
        // maintainability, standards) + 1 `wave.worker.failed`
        // synthetic record for the mismatched worker. Worker 1's
        // `review.dimension.done` event must NOT appear in the
        // events file at all.
        let valid: Vec<&serde_json::Value> = records
            .iter()
            // The merge layer writes a synthetic
            // `review.dimension.done` placeholder for each
            // `wave.worker.failed` so the aggregator hat still
            // gets a trigger on the publish topic; that
            // placeholder carries a `## Worker N (FAILED)` prefix.
            // The valid (worker-emitted) records have a JSON
            // object as the payload (the worker emitted a real
            // `{"dimension":"...","findings_count":0,...}` shape).
            .filter(|r| {
                r["topic"] == "review.dimension.done"
                    && r["payload"]
                        .as_str()
                        .is_some_and(|p| p.starts_with('{') && !p.contains("## Worker"))
            })
            .collect();
        assert_eq!(
            valid.len(),
            3,
            "expected 3 valid review.dimension.done records (mismatched one dropped), got {valid:?}"
        );

        // Verify the dropped worker's record (worker 1) is NOT
        // present. Worker 0 also emits "correctness" (its
        // assigned dimension) so we filter on the wave_index
        // field instead of the dimension value.
        for record in &valid {
            assert_ne!(
                record["wave_index"], 1,
                "worker 1's mismatched record must not appear in any valid record, got {record:?}"
            );
        }

        // Exactly one wave.worker.failed record.
        let failed: Vec<&serde_json::Value> = records
            .iter()
            .filter(|r| r["topic"] == "wave.worker.failed")
            .collect();
        assert_eq!(
            failed.len(),
            1,
            "expected exactly 1 synthetic wave.worker.failed, got {failed:?}"
        );
        let failed_payload_str = failed[0]["payload"].as_str().unwrap();
        let failed_payload: serde_json::Value =
            serde_json::from_str(failed_payload_str).expect("failed payload must be JSON");
        assert_eq!(
            failed_payload["reason"], "worker_failed:dimension_mismatch",
            "synthetic record must carry dimension_mismatch reason"
        );
        assert_eq!(failed_payload["expected_dimension"], "testing");
        assert_eq!(failed_payload["actual_dimension"], "correctness");
        assert_eq!(failed_payload["wave_index"], 1);
        assert_eq!(failed_payload["wave_id"], "w-u4-dim");
    }

    /// U4/R4: when `assigned_dimensions` is empty, the dimension
    /// gate is skipped entirely and mismatched / unmatched events
    /// pass through. This is the legacy / non-review-wave path.
    ///
    /// U5/R5: the returned mismatch list must be empty.
    #[test]
    fn test_merge_no_check_when_no_assignment() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let events_file = tmp.path().join("events.jsonl");

        // No assignment at all — empty HashMap.
        let completed = ralph_core::CompletedWave {
            wave_id: "w-legacy".to_string(),
            wave_total: 2,
            results: vec![
                ralph_core::WaveResult {
                    index: 0,
                    events: vec![dimension_done_event(0, "anything")],
                },
                ralph_core::WaveResult {
                    index: 1,
                    events: vec![dimension_done_event(1, "goes")],
                },
            ],
            failures: vec![],
            duration: Duration::from_millis(10),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: std::collections::HashMap::new(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };

        let (mismatches, _pending_resumes) = merge_wave_results_to_events_file(
            &completed,
            &events_file,
            &["review.dimension.done".to_string()],
            "review-coordinator",
            None,
        )
        .expect("merge must succeed");

        assert!(
            mismatches.is_empty(),
            "U5/R5: no assignment → no mismatches, got {mismatches:?}"
        );

        let merged = fs::read_to_string(&events_file).expect("read merged events");
        let records: Vec<serde_json::Value> = merged
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("json event"))
            .collect();

        let valid: Vec<&serde_json::Value> = records
            .iter()
            .filter(|r| r["topic"] == "review.dimension.done")
            .collect();
        assert_eq!(
            valid.len(),
            2,
            "no dimension check → both events must pass through unchanged"
        );
        let failed: Vec<&serde_json::Value> = records
            .iter()
            .filter(|r| r["topic"] == "wave.worker.failed")
            .collect();
        assert!(
            failed.is_empty(),
            "no dimension check → no synthetic failure records, got {failed:?}"
        );
    }

    /// U4/R4 happy path: all 4 workers emit `review.dimension.done`
    /// with the exact dimension they were assigned. No events are
    /// dropped, no synthetic failures written.
    ///
    /// U5/R5: the returned mismatch list must be empty.
    #[test]
    fn test_merge_passes_correct_dimension() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let events_file = tmp.path().join("events.jsonl");

        let mut assigned_dimensions = std::collections::HashMap::new();
        assigned_dimensions.insert(0u32, "correctness".to_string());
        assigned_dimensions.insert(1, "testing".to_string());
        assigned_dimensions.insert(2, "maintainability".to_string());
        assigned_dimensions.insert(3, "standards".to_string());

        let completed = ralph_core::CompletedWave {
            wave_id: "w-u4-happy".to_string(),
            wave_total: 4,
            results: vec![
                ralph_core::WaveResult {
                    index: 0,
                    events: vec![dimension_done_event(0, "correctness")],
                },
                ralph_core::WaveResult {
                    index: 1,
                    events: vec![dimension_done_event(1, "testing")],
                },
                ralph_core::WaveResult {
                    index: 2,
                    events: vec![dimension_done_event(2, "maintainability")],
                },
                ralph_core::WaveResult {
                    index: 3,
                    events: vec![dimension_done_event(3, "standards")],
                },
            ],
            failures: vec![],
            duration: Duration::from_millis(10),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions,
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };

        let (mismatches, _pending_resumes) = merge_wave_results_to_events_file(
            &completed,
            &events_file,
            &["review.dimension.done".to_string()],
            "review-coordinator",
            None,
        )
        .expect("merge must succeed");

        assert!(
            mismatches.is_empty(),
            "U5/R5: all match → no mismatches, got {mismatches:?}"
        );

        let merged = fs::read_to_string(&events_file).expect("read merged events");
        let records: Vec<serde_json::Value> = merged
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("json event"))
            .collect();

        assert_eq!(
            records.len(),
            4,
            "all 4 valid events must be merged, no synthetic failures"
        );
        for record in &records {
            assert_eq!(record["topic"], "review.dimension.done");
            assert_eq!(record["hat"], "review-coordinator");
        }
    }
}
